//! The machine copied: a link stream's other end (docs/design/link.md,
//! decision 17; `crate::link_stream`). `keepane web` takes the connection
//! and hands the first handshake message here (`Event::LinkStream`): the
//! server holds the key, so it reads the message, checks the machine that
//! sent it (paired, and it has all of this machine as the request comes:
//! `--full`, over Tailscale or with `--lan`), answers, and gives the web
//! task the connection's keys and a feed of the journal's changes. The
//! feed starts after the number the other machine has, or, when those are
//! no longer kept (or it has none), with a snapshot.
//!
//! The server sends each batch of changes to every feed (`stream_push`).
//! A feed whose web task has more than `FEED_LIMIT` waiting (the network is
//! slower than the panes) is ended, as is one for a machine that no longer
//! has all of this one: the other machine connects again and goes on from
//! where it is.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::link_state::js;
use super::{Server, journal};
use crate::link;
use crate::link_proto::Answer;

/// What may wait for a feed's web task before it is ended.
pub const FEED_LIMIT: usize = 16 << 20;

/// A connection's first handshake message, from `keepane web`.
pub struct StreamAsk {
    pub peer: IpAddr,
    pub msg1: Vec<u8>,
    pub reply: oneshot::Sender<StreamReply>,
}

/// What to answer, and, for a welcome, the keys and the feed.
pub struct StreamReply {
    pub msg2: Vec<u8>,
    pub open: Option<(snow::TransportState, Feed)>,
}

pub struct Feed {
    pub rx: mpsc::UnboundedReceiver<FeedItem>,
    /// What waits in `rx`, as `FeedItem::cost` counts it.
    pub queued: Arc<AtomicUsize>,
    /// What the machine typed into its copy of a pane (`link_proto::Input`):
    /// to the server, which puts it into the pane.
    pub input: Box<dyn Fn(u32, Vec<u8>) + Send + Sync>,
    /// A command it sends about one of this machine's sessions
    /// (`link_proto::Command`): to the server, which runs it if it is one
    /// that changes a session's shape, and replies on this feed.
    pub command: Box<dyn Fn(u64, String) + Send + Sync>,
}

pub enum FeedItem {
    Entry(journal::Entry),
    Snapshot(journal::Snapshot),
    /// What became of one of its commands, after the changes it made.
    Reply(crate::link_proto::Reply),
    /// The feed ends, and why.
    End(String),
}

impl FeedItem {
    pub fn cost(&self) -> usize {
        match self {
            FeedItem::Entry(e) => e.cost(),
            FeedItem::Snapshot(s) => s.panes.iter().map(|p| p.screen.len() + 64).sum::<usize>() + 256,
            FeedItem::Reply(r) => r.text.len() + 32,
            FeedItem::End(_) => 0,
        }
    }
}

/// A machine fed.
pub(super) struct Subscriber {
    key: String,
    ip: IpAddr,
    addr: String,
    tx: mpsc::UnboundedSender<FeedItem>,
    queued: Arc<AtomicUsize>,
    /// The number of the next change it gets.
    next: u64,
}

impl Subscriber {
    fn send(&self, item: FeedItem) -> bool {
        self.queued.fetch_add(item.cost(), Ordering::Relaxed);
        self.tx.send(item).is_ok()
    }
}

impl Server {
    /// A connection's first handshake message: answer it, and feed the
    /// machine when it may be.
    pub(super) fn link_stream_ask(&mut self, ask: StreamAsk) {
        let refused_here = match &self.web {
            None => Some("keepane web is off on this machine".to_string()),
            Some(w) if w.read_only() => Some("read-only: this machine takes nothing from other machines".into()),
            Some(_) => None,
        };
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("link stream: {e}");
                return;
            }
        };
        // A first message that does not check out gets no answer at all:
        // it may not be from a keepane, and nothing is learnt from it.
        let opening = match crate::link_stream::open(&l.id.x25519_secret(), &ask.msg1) {
            Ok(o) => o,
            Err(e) => {
                log::info!("link stream from {}: {e:#}", ask.peer);
                return;
            }
        };
        let peer = l.peers.iter().find(|p| crate::link_stream::x25519_public(&p.key) == Some(opening.remote)).cloned();
        // A first message recorded and sent again: too old, or its ephemeral
        // key (its first 32 bytes, new for every connection) seen before.
        let now = link::now();
        let replayed = link::check_time(opening.hello.time, now).or_else(|| {
            (!l.nonces.fresh(&format!("stream:{}", link::b64(&ask.msg1[..32.min(ask.msg1.len())])), now))
                .then(|| "this handshake was seen before".to_string())
        });
        let port_hint = peer.as_ref().and_then(|p| p.addr.rsplit_once(':').map(|(_, port)| port.to_string()));
        let addr = match &port_hint {
            Some(port) => format!("{}:{port}", ask.peer),
            None => ask.peer.to_string(),
        };
        // A paired machine that may not copy this one asks now and then by
        // itself (`mirror_peers` there): nothing to tell anyone about.
        let quiet = peer.is_some() && replayed.is_none();
        let refused = refused_here.or(replayed).or_else(|| may_stream(peer.as_ref(), ask.peer, &addr).err());
        let (since, epoch) = (opening.hello.since, opening.hello.epoch);
        let answer = Answer {
            version: env!("CARGO_PKG_VERSION").into(),
            refused: refused.clone(),
            epoch: self.journal.epoch,
            commands: true,
        };
        let (msg2, keys) = match opening.answer(&answer) {
            Ok(a) => a,
            Err(e) => {
                log::warn!("link stream: {e:#}");
                return;
            }
        };
        let (Some(keys), Some(peer)) = (keys, peer) else {
            match refused {
                Some(why) if quiet => log::info!("link stream from {addr} refused: {why}"),
                Some(why) => self.link_refused(&addr, &why),
                None => {}
            }
            let _ = ask.reply.send(StreamReply { msg2, open: None });
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let mut sub =
            Subscriber { key: peer.key.clone(), ip: ask.peer, addr: addr.clone(), tx, queued: queued.clone(), next: 0 };
        // Where it starts: after what it has, or a snapshot.
        self.journal_look();
        let (ours, last) = (self.journal.epoch, self.journal.last());
        let from = resume_from(since, epoch, ours, last, |n| self.journal.since(n, None).gap);
        match from {
            Some(n) => {
                for e in self.journal.since(n, None).entries {
                    sub.send(FeedItem::Entry(e.clone()));
                }
                sub.next = self.journal.last() + 1;
            }
            None => {
                let snap = self.journal_snapshot();
                sub.next = snap.seq + 1;
                sub.send(FeedItem::Snapshot(snap));
            }
        }
        let how = match from {
            Some(n) => format!("from #{}", n + 1),
            None => "from a snapshot".into(),
        };
        self.link_note(
            &format!("link: {addr} copies this machine's sessions ({how})"),
            "stream",
            &format!(",\"addr\":{},\"key\":{}", js(&addr), js(&peer.key)),
        );
        self.feeds.push(sub);
        let (events, key) = (self.events.clone(), peer.key.clone());
        let input = Box::new(move |pane, bytes| {
            let _ = events.send(super::Event::StreamInput(key.clone(), pane, bytes));
        });
        let (events, key) = (self.events.clone(), peer.key.clone());
        let command = Box::new(move |id, line| {
            let _ = events.send(super::Event::StreamCommand(key.clone(), id, line));
        });
        let _ = ask.reply.send(StreamReply { msg2, open: Some((keys, Feed { rx, queued, input, command })) });
    }

    /// Send every feed what changed since it was last sent to; end those
    /// that fell behind or whose machine no longer has all of this one.
    pub(super) fn stream_push(&mut self) {
        if self.feeds.is_empty() {
            return;
        }
        let last = self.journal.last();
        let mut feeds = std::mem::take(&mut self.feeds);
        feeds.retain_mut(|sub| {
            if sub.tx.is_closed() {
                return false;
            }
            let peer = self.link.as_ref().and_then(|l| l.peer_by_key(&sub.key));
            let behind = sub.next <= last && self.journal.since(sub.next.saturating_sub(1), None).gap;
            let why = may_stream(peer, sub.ip, &sub.addr)
                .err()
                .or_else(|| fed_too_much(sub.queued.load(Ordering::Relaxed), behind));
            if let Some(why) = why {
                log::info!("link stream to {}: {why}", sub.addr);
                let _ = sub.tx.send(FeedItem::End(why));
                return false;
            }
            if sub.next <= last {
                for e in self.journal.since(sub.next - 1, None).entries {
                    if !sub.send(FeedItem::Entry(e.clone())) {
                        return false;
                    }
                }
                sub.next = last + 1;
            }
            true
        });
        self.feeds = feeds;
    }

    /// Keys typed into a machine's copy of pane `pane` (`link_proto::Input`):
    /// into the pane, as if typed here, while that machine is fed (it has
    /// all of this one: `stream_push` ends the feed of one that no longer
    /// has). A pane of a third machine copied here is not this one's to
    /// give: no further.
    pub(super) fn stream_input(&mut self, key: &str, pane: u32, bytes: &[u8]) {
        if !self.feeds.iter().any(|s| s.key == key && !s.tx.is_closed()) {
            return;
        }
        let Some(p) = self.find_pane_mut(pane).filter(|p| p.remote.is_none()) else { return };
        if p.exit_code.is_none() {
            p.type_input(bytes);
        }
    }

    /// A command a machine fed sends about one of this machine's sessions
    /// (`link_proto::Command`), while it is fed (it has all of this one):
    /// run when it changes a session's shape and names this machine's own
    /// places only (`mirrored::command_from_machine`); then, after the changes it
    /// made, its reply on that machine's feed. Kept in the event log.
    pub(super) fn stream_command(&mut self, key: &str, id: u64, line: &str) {
        let Some(addr) = self.feeds.iter().find(|s| s.key == key && !s.tx.is_closed()).map(|s| s.addr.clone()) else {
            return;
        };
        let (ok, text, pane) = match self.command_from_machine(line) {
            Err(why) => (false, why, None),
            Ok((cmd, sid)) => {
                let (ok, text) = match self.exec(cmd, None) {
                    super::Outcome::Error(e) => (false, e),
                    super::Outcome::Text(t) => (true, t),
                    _ => (true, String::new()),
                };
                let pane = self.session(sid).and_then(|s| s.window()).map(|w| w.active);
                (ok, text, pane)
            }
        };
        self.observe
            .link("command", &format!(",\"addr\":{},\"key\":{},\"line\":{},\"ok\":{ok}", js(&addr), js(key), js(line)));
        log::info!("link: {addr} ran `{line}`: {}", if ok { "done" } else { text.as_str() });
        // Its changes first: the machine has them when it reads the reply.
        self.journal_look();
        self.stream_push();
        let reply = crate::link_proto::Reply { id, ok, text, pane };
        if let Some(sub) = self.feeds.iter().find(|s| s.key == key) {
            sub.send(FeedItem::Reply(reply));
        }
    }
    /// A machine fed, for a test: its feed's other end.
    #[cfg(test)]
    pub(super) fn feed_for_test(&mut self, key: &str) -> mpsc::UnboundedReceiver<FeedItem> {
        let (tx, rx) = mpsc::unbounded_channel();
        let ip = IpAddr::from([127, 0, 0, 1]);
        let queued = Arc::new(AtomicUsize::new(0));
        self.feeds.push(Subscriber { key: key.into(), ip, addr: "127.0.0.1:1".into(), tx, queued, next: 0 });
        rx
    }

    /// The keys of the machines fed now, for `link list`.
    pub(super) fn feeds_to(&self) -> Vec<String> {
        self.feeds.iter().filter(|s| !s.tx.is_closed()).map(|s| s.key.clone()).collect()
    }
}

/// Whether `peer` (the machine, if paired, asking from `ip`, shown as
/// `addr`) may have a stream now: all of this machine, as the request
/// comes (over Tailscale or from this machine, or `--lan`). Judged when it
/// connects and again with each batch of changes (taken back: it ends).
pub(super) fn may_stream(peer: Option<&link::Peer>, ip: IpAddr, addr: &str) -> Result<(), String> {
    match peer {
        None => Err(format!(
            "{ip} is not paired with this machine: `keepane link add` its web address here, or `keepane link trust`"
        )),
        Some(p) if !p.full => Err(format!(
            "{addr} does not have all of this machine, which a copy of its sessions takes: `keepane link allow {addr} --full` here (outside keepane)"
        )),
        Some(p) if !p.lan && !link::full_network(ip) => Err(format!(
            "all of this machine is given to {addr} over Tailscale only, and this comes from another network (`--full --lan` gives it from any)"
        )),
        Some(_) => Ok(()),
    }
}

/// Where a stream starts: after `since` when that is a number of this run
/// of the server (`epoch` the one it names, `ours` this one), not past the
/// newest, and what came after it is still kept (`gap` says it is not);
/// else (None) from a snapshot.
pub(super) fn resume_from(
    since: Option<u64>,
    epoch: Option<u64>,
    ours: u64,
    last: u64,
    gap: impl Fn(u64) -> bool,
) -> Option<u64> {
    since.filter(|n| epoch == Some(ours) && *n <= last && !gap(*n))
}

/// Why a feed ends though its machine may still have it: more waits for
/// its web task than `FEED_LIMIT` (the network is slower than the panes),
/// or what it was not sent is no longer kept.
pub(super) fn fed_too_much(queued: usize, behind: bool) -> Option<String> {
    if queued > FEED_LIMIT {
        Some(format!("more than {} MiB waited for the network: it starts again", FEED_LIMIT >> 20))
    } else if behind {
        Some("what it had not been sent is no longer kept: it starts again".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(full: bool, lan: bool) -> link::Peer {
        link::Peer { key: "k".into(), addr: "a:1".into(), full, lan }
    }

    #[test]
    fn only_a_machine_with_all_of_this_one_as_it_asks_may_have_a_stream() {
        let (ts, lan, here) =
            ("100.64.0.9".parse().unwrap(), "192.168.1.9".parse().unwrap(), "127.0.0.1".parse().unwrap());
        assert!(may_stream(None, ts, "x").unwrap_err().contains("not paired"));
        assert!(may_stream(Some(&peer(false, false)), ts, "x").unwrap_err().contains("--full"));
        assert!(may_stream(Some(&peer(true, false)), ts, "x").is_ok(), "over Tailscale");
        assert!(may_stream(Some(&peer(true, false)), here, "x").is_ok(), "from this machine");
        assert!(may_stream(Some(&peer(true, false)), lan, "x").unwrap_err().contains("--full --lan"));
        assert!(may_stream(Some(&peer(true, true)), lan, "x").is_ok(), "--lan: any network");
    }

    #[test]
    fn a_stream_goes_on_only_from_a_number_of_this_run_still_kept() {
        let no_gap = |_| false;
        assert_eq!(resume_from(Some(5), Some(7), 7, 9, no_gap), Some(5));
        assert_eq!(resume_from(Some(9), Some(7), 7, 9, no_gap), Some(9), "has everything");
        assert_eq!(resume_from(Some(5), Some(8), 7, 9, no_gap), None, "another run of the server");
        assert_eq!(resume_from(Some(5), None, 7, 9, no_gap), None, "no run named");
        assert_eq!(resume_from(Some(10), Some(7), 7, 9, no_gap), None, "past the newest");
        assert_eq!(resume_from(Some(5), Some(7), 7, 9, |_| true), None, "no longer kept");
        assert_eq!(resume_from(None, Some(7), 7, 9, no_gap), None, "nothing: a snapshot");
    }

    #[test]
    fn a_feed_too_far_behind_ends() {
        assert_eq!(fed_too_much(FEED_LIMIT, false), None, "up to the limit");
        assert!(fed_too_much(FEED_LIMIT + 1, false).unwrap().contains("MiB"));
        assert!(fed_too_much(0, true).unwrap().contains("no longer kept"));
        assert_eq!(fed_too_much(0, false), None);
    }
}
