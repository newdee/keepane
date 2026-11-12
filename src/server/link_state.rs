//! What this server keeps for the links to other machines
//! (docs/design/link.md): its key pair, the machines it lets in, the
//! nonces seen lately, and where the messages that came from another
//! machine came from. The commands that touch only this machine are here
//! too (`link-id`, `link-trust`, `link-allow`, `link-rekey`); what is
//! shared, and `link-list`, which asks the others, are `link_share`; those
//! that reach out or take a request in are `link_out` and `link_in`.

use std::collections::HashMap;

use chrono::{DateTime, Local};

use super::actor::MsgId;
use super::{ClientId, Outcome, Server};
use crate::link::{self, Identity, Nonces, Peer};

pub(super) struct LinkHost {
    /// Where all of it is kept, decided once, at load (`link::dir`).
    pub(super) dir: std::path::PathBuf,
    pub(super) id: Identity,
    pub(super) peers: Vec<Peer>,
    pub(super) nonces: Nonces,
    /// When each machine (by key) last got a request through.
    pub(super) seen: HashMap<String, DateTime<Local>>,
    /// Where a message that came from another machine came from: that
    /// machine's key, and the message's number and task there, so that an
    /// answer goes back and is filed under them.
    pub(super) origin: HashMap<MsgId, Origin>,
    /// When each address was last said to be refused: said once a while,
    /// so that a flood of bad requests fills neither the status line nor
    /// the day's event log (whose cap would then keep real events out).
    pub(super) refused: HashMap<String, std::time::Instant>,
    /// Messages handed to another machine, and how they are known there.
    pub(super) sent: HashMap<MsgId, super::link_out::SentThere>,
    /// The panes another machine started here (`link-start`), by the key of
    /// that machine: the ones it may close, and what counts toward its
    /// `agent-pane-limit`. Not kept past this server.
    pub(super) made: HashMap<super::PaneId, String>,
    /// Bundles of commits arriving from other machines (`link_git`), by
    /// their transfer's name.
    pub(super) incoming: HashMap<String, super::link_git::Incoming>,
}

/// Origins remembered at most (as many as message records, `observe`).
pub(super) const ORIGINS_KEPT: usize = 10_000;
/// A refusal from the same address is said again after this.
pub(super) const REFUSED_QUIET: std::time::Duration = std::time::Duration::from_secs(60);
/// Addresses whose refusal is remembered at most.
const REFUSED_KEPT: usize = 1024;

impl LinkHost {
    /// Whether a refusal from `addr` is to be said now (and remember that it was).
    pub(super) fn say_refused(&mut self, addr: &str) -> bool {
        let now = std::time::Instant::now();
        if self.refused.get(addr).is_some_and(|t| now.duration_since(*t) < REFUSED_QUIET) {
            return false;
        }
        if self.refused.len() >= REFUSED_KEPT {
            self.refused.retain(|_, t| now.duration_since(*t) < REFUSED_QUIET);
        }
        if self.refused.len() >= REFUSED_KEPT {
            return false;
        }
        self.refused.insert(addr.to_string(), now);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Origin {
    pub key: String,
    pub id: u64,
    pub task: u64,
}

impl LinkHost {
    pub(super) fn peer_by_addr(&self, addr: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.addr == addr)
    }

    pub(super) fn peer_by_key(&self, key: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.key == key)
    }
}

/// The refusal a pane gets for what only a person outside every pane may
/// do (the rule `set-work-mode` follows too).
pub(super) fn not_from_a_pane(what: &str) -> String {
    format!("{what} is not run from inside a pane: run it in a terminal outside keepane, or at the C-b : prompt")
}

pub(super) fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

impl Server {
    /// The link directory: the one the state was loaded from (the keys,
    /// the table, where repositories went, bundles on their way), else where
    /// it would be.
    pub(super) fn link_dir(&self) -> std::path::PathBuf {
        self.link.as_ref().map_or_else(|| link::dir(&self.socket), |l| l.dir.clone())
    }

    /// The link state, loaded on first use (the key made then, if none).
    pub(super) fn link(&mut self) -> Result<&mut LinkHost, String> {
        if self.link.is_none() {
            let dir = self.link_dir();
            let id = Identity::load_or_make(&dir).map_err(|e| format!("link: {e:#}"))?;
            // Bundles left half sent or taken by a server before this one.
            for d in ["incoming", "outgoing"] {
                let _ = std::fs::remove_dir_all(dir.join(d));
            }
            // A table of keepane 0.55 or before: written again in the words
            // of now, and what no longer reads as a level said.
            let (peers, dropped) = link::load_table_noting(&dir);
            let was = std::fs::read_to_string(dir.join("authorized")).unwrap_or_default();
            let mut notice = None;
            if !was.is_empty() && was != link::format_table(&peers) {
                if let Err(e) = link::save_table(&dir, &peers) {
                    log::warn!("link: {e:#}");
                }
                if !dropped.is_empty() {
                    let text = format!(
                        "link: permissions are two levels now (0.56); {} had some of the old five and now sees nothing here: \
                         keepane link share <host:port> <session|window|pane>, or link allow <host:port> --full",
                        dropped.join(", ")
                    );
                    let list: Vec<String> = dropped.iter().map(|a| js(a)).collect();
                    notice = Some((text, format!(",\"dropped\":[{}]", list.join(","))));
                }
            }
            self.link = Some(LinkHost {
                dir: dir.clone(),
                id,
                peers,
                nonces: Nonces::default(),
                seen: HashMap::new(),
                origin: HashMap::new(),
                refused: HashMap::new(),
                sent: HashMap::new(),
                made: HashMap::new(),
                incoming: HashMap::new(),
            });
            if let Some((text, fields)) = notice {
                self.link_note(&text, "levels", &fields);
            }
        }
        Ok(self.link.as_mut().expect("just made"))
    }

    pub(super) fn save_peers(&mut self) -> Result<(), String> {
        let dir = self.link_dir();
        let peers = self.link()?.peers.clone();
        link::save_table(&dir, &peers).map_err(|e| format!("link: {e:#}"))
    }

    /// This machine's web port and key: what another machine answers to,
    /// and what pairing is proved with.
    pub(super) fn own_web(&self) -> Result<(u16, String), String> {
        match &self.web {
            Some(w) => Ok((w.port(), w.key().to_string())),
            None => {
                Err("link needs `keepane web` running here: the other machine answers to this machine's web address"
                    .into())
            }
        }
    }

    /// A machine paired with this one is said on the status line, like a
    /// phone connecting, and the event log keeps it.
    pub(super) fn link_note(&mut self, text: &str, what: &str, fields: &str) {
        self.observe.link(what, fields);
        self.note_message(text);
        let now = std::time::Instant::now();
        for c in self.clients.values_mut().filter(|c| c.session.is_some()) {
            c.message = Some((text.to_string(), now));
        }
    }

    /// The commands that touch only this machine.
    pub(super) fn exec_link_local(&mut self, cmd: crate::command::Cmd, cid: Option<ClientId>) -> Outcome {
        use crate::command::Cmd;
        match cmd {
            Cmd::LinkId => match self.link() {
                Ok(l) => Outcome::Text(format!("{}  {}", l.id.public(), link::fingerprint(&l.id.public()))),
                Err(e) => Outcome::Error(e),
            },
            Cmd::LinkList => self.link_list(cid),
            Cmd::LinkTrust { addr, key, full, lan } => self.link_trust(cid, addr, key, full, lan),
            Cmd::LinkAllow { addr, full, lan } => self.link_allow(cid, &addr, full, lan),
            Cmd::LinkShare { addr, targets, share } => self.link_share(cid, &addr, &targets, share),
            Cmd::LinkRekey => self.link_rekey(),
            other => Outcome::Error(format!("not a link command: {other}")),
        }
    }

    /// A machine let in by hand, by its key (what `keepane link id` prints
    /// there): the same table `link-add` fills through the web key; with
    /// `full`, as `link-allow --full` too.
    fn link_trust(&mut self, cid: Option<ClientId>, addr: String, key: String, full: bool, lan: bool) -> Outcome {
        if full && self.caller_pane(cid).is_some() {
            return Outcome::Error(not_from_a_pane("link-trust --full"));
        }
        let by = self.by(cid);
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        if l.id.public() == key {
            return Outcome::Error("link-trust: that is this machine's own key".into());
        }
        match l.peers.iter_mut().find(|p| p.key == key) {
            Some(p) => {
                p.addr = addr.clone();
                p.full = full;
                p.lan = lan;
            }
            None => l.peers.push(Peer { key: key.clone(), addr: addr.clone(), full, lan }),
        }
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        let fp = link::fingerprint(&key);
        self.link_note(
            &format!("link: {addr} ({fp}) trusted by {by}"),
            "trusted",
            &format!(",\"addr\":{},\"key\":{},\"full\":{full},\"lan\":{lan},\"by\":{}", js(&addr), js(&key), js(&by)),
        );
        Outcome::Text(format!("trusted {addr}  {fp}"))
    }

    /// Whether a machine has all of this one (`--full`, over Tailscale only
    /// unless `--lan`), or nothing (`--none`, what was shared with it taken
    /// back too). Only a person outside every pane decides: otherwise a
    /// program in a pane could let another machine run commands on this
    /// one, or read its screens.
    fn link_allow(&mut self, cid: Option<ClientId>, addr: &str, full: bool, lan: bool) -> Outcome {
        if self.caller_pane(cid).is_some() {
            return Outcome::Error(not_from_a_pane("link-allow"));
        }
        let by = self.by(cid);
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        let Some(p) = l.peers.iter_mut().find(|p| p.addr == addr) else {
            return Outcome::Error(format!("link-allow: {addr} is not paired with this machine (`keepane link list`)"));
        };
        p.full = full;
        p.lan = full && lan;
        let key = p.key.clone();
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        let text = if full {
            let from = if lan {
                "from any network (plain HTTP: anyone on the network can read what goes between the two)"
            } else {
                "over Tailscale or from this machine"
            };
            let mut said = format!(
                "{addr} has all of this machine {from}: it sees every pane and may type into it, attach to every \
                 session and keep a copy of them all, its messages run in shell panes, it may start panes and \
                 send commits and projects here (--none takes it all back)"
            );
            // Where it is now, said when that is not where `full` comes from.
            let now =
                addr.rsplit_once(':').and_then(|(h, _)| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok());
            if !lan && now.is_some_and(|ip| !link::full_network(ip)) {
                said.push_str(&format!(
                    "; {addr} is not a Tailscale address: from there it has only what is shared \
                     (--full --lan gives it all from any network)"
                ));
            }
            said
        } else {
            let n = self.unshare_everything(&key);
            let shared = if n == 0 { String::new() } else { ", and nothing is shared with it any more".to_string() };
            format!("{addr} has nothing of this machine{shared}: it may still answer what panes here send it")
        };
        self.link_note(
            &format!("link: {text} (by {by})"),
            "allowed",
            &format!(",\"addr\":{},\"key\":{},\"full\":{full},\"lan\":{lan},\"by\":{}", js(addr), js(&key), js(&by)),
        );
        Outcome::Text(text)
    }

    /// A new key pair: every pairing made with the old one is void.
    fn link_rekey(&mut self) -> Outcome {
        let dir = self.link_dir();
        let id = match Identity::make(&dir) {
            Ok(id) => id,
            Err(e) => return Outcome::Error(format!("link-rekey: {e:#}")),
        };
        let public = id.public();
        match self.link() {
            Ok(l) => l.id = id,
            Err(e) => return Outcome::Error(e),
        }
        self.observe.link("rekeyed", &format!(",\"key\":{}", js(&public)));
        Outcome::Text(format!(
            "{public}  {}\nevery machine paired with this one has to pair again (`keepane link add`, or `link trust` with this key)",
            link::fingerprint(&public)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Outcome, Server};
    use crate::command::Cmd;

    fn server(name: &str) -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        Server::new(pane_tx, name.into(), events)
    }

    fn text(o: Outcome) -> String {
        match o {
            Outcome::Text(t) => t,
            Outcome::Error(e) => format!("error: {e}"),
            Outcome::Ok => "ok".into(),
            _ => "other".into(),
        }
    }

    /// Each server has a key of its own, kept between loads; a machine
    /// trusted by hand is listed with what it has here; `--full` and
    /// `--none` are the two levels, said in words; the table is what a new
    /// load reads back; a table of 0.55 is read as the levels and written
    /// again.
    #[test]
    fn the_key_is_kept_and_the_table_is_what_is_trusted() {
        let dir = std::env::temp_dir().join(format!("keepane-link-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Each socket its own directory under it (`link::dir`).
        unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
        let mut a = server("link-state-a");
        let mut b = server("link-state-b");
        let key_a = text(a.exec_link_local(Cmd::LinkId, None));
        let key_b = text(b.exec_link_local(Cmd::LinkId, None));
        assert_ne!(key_a, key_b);
        let (ka, fp) = key_a.split_once("  ").unwrap();
        assert!(crate::link::valid_public(ka) && fp.starts_with("SHA256:"), "{key_a}");
        assert_eq!(text(server("link-state-a").exec_link_local(Cmd::LinkId, None)), key_a, "kept");

        assert!(text(b.exec_link_local(Cmd::LinkList, None)).starts_with("no machines paired"));
        let trust =
            |addr: &str, key: &str, full| Cmd::LinkTrust { addr: addr.into(), key: key.into(), full, lan: false };
        assert_eq!(
            text(b.exec_link_local(trust("10.0.0.1:7681", ka, false), None)),
            format!("trusted 10.0.0.1:7681  {fp}")
        );
        let own = trust("10.0.0.2:7681", key_b.split("  ").next().unwrap(), false);
        assert!(text(b.exec_link_local(own, None)).contains("own key"));
        // Without a client, the list does not ask the other machine.
        let list = text(b.exec_link_local(Cmd::LinkList, None));
        let head = list.lines().next().unwrap();
        assert!(head.contains("IT HAS HERE") && head.contains("THIS HAS THERE"), "{list}");
        assert!(
            list.contains("10.0.0.1:7681") && list.contains(fp) && list.contains("nothing") && list.contains("never"),
            "{list}"
        );

        let parsed = |line: &str| crate::command::parse(&crate::command::tokenize(line).unwrap()).unwrap();
        assert!(text(b.exec_link_local(parsed("link-allow 10.0.0.9:1 --full"), None)).contains("not paired"));
        let said = text(b.exec_link_local(parsed("link-allow 10.0.0.1:7681 --full"), None));
        assert!(
            said.starts_with("10.0.0.1:7681 has all of this machine over Tailscale or from this machine:"),
            "{said}"
        );
        assert!(
            said.ends_with(
                "10.0.0.1:7681 is not a Tailscale address: from there it has only what is shared \
                 (--full --lan gives it all from any network)"
            ),
            "a LAN address is told, last, that it has only what is shared: {said}"
        );
        assert!(text(b.exec_link_local(Cmd::LinkList, None)).contains("all of it (full, over Tailscale)"));
        let said = text(b.exec_link_local(parsed("link-allow 10.0.0.1:7681 --full --lan"), None));
        assert!(said.contains("from any network") && said.contains("plain HTTP"), "{said}");
        assert!(text(b.exec_link_local(Cmd::LinkList, None)).contains("all of it (full, any network)"));
        let said = text(b.exec_link_local(parsed("link-allow 10.0.0.1:7681 --none"), None));
        assert!(said.starts_with("10.0.0.1:7681 has nothing of this machine"), "{said}");
        let peer = b.link().unwrap().peers[0].clone();
        assert!(!peer.full && !peer.lan, "{peer:?}");
        // Unsharing what was never shared says so.
        let said = text(b.exec_link_local(parsed("link-unshare 10.0.0.1:7681 --all"), None));
        assert_eq!(said, "nothing here was shared with 10.0.0.1:7681");
        assert!(text(b.exec_link_local(parsed("link-share 10.0.0.9:1 work"), None)).contains("not paired"));
        assert!(
            text(b.exec_link_local(parsed("link-share 10.0.0.1:7681 nosuch"), None)).starts_with("error: link-share:")
        );

        // Trusted again by hand with --full: read back by a new load.
        b.exec_link_local(parsed(&format!("link-trust 10.0.0.1:7681 {ka} --full")), None);
        let mut again = server("link-state-b");
        let peers = &again.link().unwrap().peers;
        assert_eq!(peers.len(), 1);
        assert!(peers[0].full && !peers[0].lan && peers[0].key == ka, "read back");

        // A table of 0.55: all five old permissions are full; some of them
        // nothing, said; the file is written in the words of now.
        let table = dir.join(crate::histlog::safe_name("link-state-c"));
        std::fs::create_dir_all(&table).unwrap();
        let k2 = crate::link::Identity::make(&dir.join("k2")).unwrap().public();
        std::fs::write(
            table.join("authorized"),
            format!("{ka} 10.0.0.1:7681 shell screen panes push agents\n{k2} 10.0.0.2:7681 shell screen\n"),
        )
        .unwrap();
        let mut c = server("link-state-c");
        let peers = c.link().unwrap().peers.clone();
        assert_eq!(peers.iter().map(|p| p.full).collect::<Vec<_>>(), [true, false]);
        let now = std::fs::read_to_string(table.join("authorized")).unwrap();
        assert!(now.contains(&format!("{ka} 10.0.0.1:7681 full\n")) && !now.contains("shell"), "{now}");
        assert!(
            c.messages.iter().any(|m| m.contains("two levels now") && m.contains("10.0.0.2:7681")),
            "the machine that lost its permissions is named"
        );

        let rekey = text(b.exec_link_local(Cmd::LinkRekey, None));
        assert!(rekey.contains("pair again"), "{rekey}");
        assert_ne!(text(b.exec_link_local(Cmd::LinkId, None)), key_b);
        unsafe { std::env::remove_var("KEEPANE_LINK_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
