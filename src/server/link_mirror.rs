//! The machine that copies: a copy of another machine's sessions, kept up
//! to date over a link stream (docs/design/link.md, decision 17;
//! `crate::link_stream`). `keepane link mirror <machine>` starts one; a task
//! holds the connection (and makes it again when it drops, from the last
//! change it had), and hands what comes to the server (`Event::Mirror`),
//! which keeps the copy: the shape, and a terminal model per pane fed the
//! same bytes as the original. A change out of order, or bytes that do not
//! start where the last ones ended, start the copy again from a snapshot.
//! `show-mirror` shows it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use super::{Event, Outcome, Server};
use crate::link_proto::{self as proto, Hello, Ping, ToClient, ToServer, to_client};
use crate::link_stream;

/// How long a connection may take to be made and answered.
const CONNECT: Duration = Duration::from_secs(10);
const PING: Duration = Duration::from_secs(15);
const SILENCE: Duration = Duration::from_secs(45);
/// Waits between tries, doubling, and the longest.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_LONGEST: Duration = Duration::from_secs(30);
/// A pane's model, inflated, at most (a long scrollback, a big screen).
const MODEL_MAX: usize = 256 << 20;

pub(super) enum MirrorNews {
    Connecting,
    Live,
    Down(String),
    Msg(Box<ToClient>, usize),
}

enum State {
    Connecting,
    Live(Instant),
    Down(String, Instant),
}

struct Pane {
    parser: vt100::Parser,
    /// Bytes it has been fed: where the next output starts.
    at: u64,
    fullscreen: bool,
}

pub(super) struct Mirror {
    addr: String,
    round: u64,
    /// None in a test's copy, which takes messages by hand.
    task: Option<tokio::task::JoinHandle<()>>,
    /// Set: the next connection asks for a snapshot.
    resync: Arc<AtomicBool>,
    /// The newest change applied (what the next connection asks after).
    last: Arc<AtomicU64>,
    state: State,
    shape: Option<proto::Shape>,
    panes: HashMap<u32, Pane>,
    bytes_in: u64,
    changes: u64,
    snapshots: u64,
    connects: u64,
    restarts: u64,
    /// It no longer follows the original: changes are let go until a
    /// snapshot comes.
    lost: bool,
}

impl Drop for Mirror {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

impl Server {
    /// `link-mirror <machine> [--stop]`.
    pub(super) fn link_mirror(&mut self, addr: &str, stop: bool) -> Outcome {
        let key = match self.peer_key(addr) {
            Ok(k) => k,
            Err(e) => return Outcome::Error(e),
        };
        if stop {
            return match self.mirrors.remove(&key) {
                Some(m) => Outcome::Text(format!("link: stopped copying {}\n", m.addr)),
                None => Outcome::Error(format!("link: {addr} is not being copied")),
            };
        }
        if let Some(m) = self.mirrors.get(&key) {
            return Outcome::Text(format!("link: {} is copied already (`show-mirror {}`)\n", m.addr, m.addr));
        }
        let Some(theirs) = link_stream::x25519_public(&key) else {
            return Outcome::Error(format!("link: the key paired with {addr} is not one"));
        };
        let mine = match self.link() {
            Ok(l) => l.id.x25519_secret(),
            Err(e) => return Outcome::Error(e),
        };
        let addr = self.link.as_ref().and_then(|l| l.peer_by_key(&key)).map_or(addr.to_string(), |p| p.addr.clone());
        self.next_mirror += 1;
        let round = self.next_mirror;
        let resync = Arc::new(AtomicBool::new(false));
        let last = Arc::new(AtomicU64::new(0));
        let task = Some(tokio::spawn(mirror_task(
            addr.clone(),
            theirs,
            mine,
            key.clone(),
            round,
            self.events.clone(),
            resync.clone(),
            last.clone(),
        )));
        self.mirrors.insert(
            key,
            Mirror {
                addr: addr.clone(),
                round,
                task,
                resync,
                last,
                state: State::Connecting,
                shape: None,
                panes: HashMap::new(),
                bytes_in: 0,
                changes: 0,
                snapshots: 0,
                connects: 0,
                restarts: 0,
                lost: false,
            },
        );
        Outcome::Text(format!("link: copying {addr}'s sessions (`show-mirror {addr}`)\n"))
    }

    /// What a mirror's task says.
    pub(super) fn mirror_news(&mut self, round: u64, key: String, news: MirrorNews) {
        let history = self.opts.history_limit;
        let Some(m) = self.mirrors.get_mut(&key).filter(|m| m.round == round) else { return };
        match news {
            MirrorNews::Connecting => {
                if !matches!(m.state, State::Down(..)) {
                    m.state = State::Connecting;
                }
            }
            MirrorNews::Live => {
                m.connects += 1;
                m.state = State::Live(Instant::now());
            }
            MirrorNews::Down(why) => {
                if !matches!(&m.state, State::Down(w, _) if *w == why) {
                    log::info!("link mirror {}: {why}", m.addr);
                }
                m.state = State::Down(why, Instant::now());
            }
            MirrorNews::Msg(msg, len) => {
                m.bytes_in += len as u64;
                m.receive(*msg, history);
            }
        }
    }

    /// `show-mirror [machine] [-t pane] [-J]`.
    pub(super) fn show_mirror(&mut self, addr: Option<&str>, pane: Option<u32>, json: bool) -> Outcome {
        let Some(addr) = addr else {
            if self.mirrors.is_empty() {
                return Outcome::Text(String::new());
            }
            let mut lines: Vec<String> = self.mirrors.values().map(|m| m.summary()).collect();
            lines.sort();
            return Outcome::Text(lines.join("\n") + "\n");
        };
        let key = match self.peer_key(addr) {
            Ok(k) => k,
            Err(e) => return Outcome::Error(e),
        };
        let Some(m) = self.mirrors.get(&key) else {
            return Outcome::Error(format!("{addr} is not being copied: `keepane link mirror {addr}`"));
        };
        if let Some(id) = pane {
            let Some(p) = m.panes.get(&id) else {
                return Outcome::Error(format!("no pane %{id} in the copy of {}", m.addr));
            };
            let (rows, cols) = p.parser.screen().size();
            let text: Vec<String> = p.parser.screen().rows(0, cols).map(|r| r.trim_end().to_string()).collect();
            if json {
                return Outcome::Text(
                    serde_json::json!({ "id": id, "cols": cols, "rows": rows, "at": p.at, "fullscreen": p.fullscreen, "text": text })
                        .to_string(),
                );
            }
            let mut out = text.join("\n");
            while out.ends_with('\n') {
                out.pop();
            }
            return Outcome::Text(out + "\n");
        }
        if json {
            return Outcome::Text(m.json().to_string());
        }
        let mut out = m.summary() + "\n";
        if let Some(shape) = &m.shape {
            for s in &shape.sessions {
                out.push_str(&format!("  {} ({}x{})\n", s.name, s.cols, s.rows));
                for w in &s.windows {
                    let current = if w.index == s.current { "*" } else { "" };
                    out.push_str(&format!("    {}: {}{current}\n", w.index, w.name));
                    for p in &w.panes {
                        let title = if p.title.is_empty() { &p.command } else { &p.title };
                        out.push_str(&format!("      %{} {}x{} {title}\n", p.id, p.cols, p.rows));
                    }
                }
            }
        }
        Outcome::Text(out)
    }
}

impl Mirror {
    #[cfg(test)]
    fn detached() -> Mirror {
        Mirror {
            addr: "test:1".into(),
            round: 1,
            task: None,
            resync: Arc::new(AtomicBool::new(false)),
            last: Arc::new(AtomicU64::new(0)),
            state: State::Connecting,
            shape: None,
            panes: HashMap::new(),
            bytes_in: 0,
            changes: 0,
            snapshots: 0,
            connects: 0,
            restarts: 0,
            lost: false,
        }
    }

    fn summary(&self) -> String {
        let state = match &self.state {
            State::Connecting => "connecting".to_string(),
            State::Live(since) => {
                format!("live {}", crate::format::command_duration(since.elapsed().as_millis() as i64))
            }
            State::Down(why, at) => {
                format!("down {} ago: {why}", crate::format::command_duration(at.elapsed().as_millis() as i64))
            }
        };
        let (s, w, p) = self.shape.as_ref().map_or((0, 0, 0), |sh| {
            let w: usize = sh.sessions.iter().map(|s| s.windows.len()).sum();
            let p: usize = sh.sessions.iter().flat_map(|s| &s.windows).map(|w| w.panes.len()).sum();
            (sh.sessions.len(), w, p)
        });
        format!(
            "{}: {state}; #{}; {s} sessions, {w} windows, {p} panes; {} KiB in, {} changes, {} snapshots, {} connections, {} restarts",
            self.addr,
            self.last.load(Ordering::Relaxed),
            self.bytes_in / 1024,
            self.changes,
            self.snapshots,
            self.connects,
            self.restarts
        )
    }

    fn json(&self) -> serde_json::Value {
        let state = match &self.state {
            State::Connecting => serde_json::json!({ "state": "connecting" }),
            State::Live(since) => serde_json::json!({ "state": "live", "secs": since.elapsed().as_secs() }),
            State::Down(why, at) => serde_json::json!({ "state": "down", "why": why, "secs": at.elapsed().as_secs() }),
        };
        let sessions: Vec<serde_json::Value> = self
            .shape
            .iter()
            .flat_map(|sh| &sh.sessions)
            .map(|s| {
                serde_json::json!({
                    "name": s.name, "cols": s.cols, "rows": s.rows, "current": s.current,
                    "windows": s.windows.iter().map(|w| serde_json::json!({
                        "index": w.index, "name": w.name, "layout": w.layout, "active": w.active, "zoomed": w.zoomed,
                        "panes": w.panes.iter().map(|p| serde_json::json!({
                            "id": p.id, "x": p.x, "y": p.y, "cols": p.cols, "rows": p.rows,
                            "title": p.title, "command": p.command, "cwd": p.cwd, "name": p.name, "mode": p.mode,
                        })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut panes: Vec<_> = self.panes.iter().collect();
        panes.sort_by_key(|(id, _)| **id);
        serde_json::json!({
            "addr": self.addr, "status": state, "seq": self.last.load(Ordering::Relaxed),
            "sessions": sessions,
            "panes": panes.iter().map(|(id, p)| {
                let (rows, cols) = p.parser.screen().size();
                serde_json::json!({ "id": id, "cols": cols, "rows": rows, "at": p.at, "fullscreen": p.fullscreen })
            }).collect::<Vec<_>>(),
            "bytes_in": self.bytes_in, "changes": self.changes, "snapshots": self.snapshots,
            "connects": self.connects, "restarts": self.restarts,
        })
    }

    /// Take one message in; one the copy cannot follow from loses it: it
    /// asks for a snapshot, and lets what comes before that go.
    fn receive(&mut self, msg: ToClient, history: usize) {
        let snapshot = matches!(msg.kind, Some(to_client::Kind::Snapshot(_)));
        if self.lost && !snapshot {
            return;
        }
        match self.apply(msg, history) {
            Ok(()) if snapshot => self.lost = false,
            Ok(()) => {}
            Err(why) => {
                log::warn!("link mirror {}: {why}: starting again from a snapshot", self.addr);
                self.restarts += 1;
                self.lost = true;
                self.resync.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Take one message into the copy. An error: the copy no longer follows
    /// the original, and starts again from a snapshot.
    fn apply(&mut self, msg: ToClient, history: usize) -> Result<(), String> {
        use proto::pane_change::Kind;
        match msg.kind {
            None | Some(to_client::Kind::Ping(_)) => Ok(()),
            Some(to_client::Kind::Snapshot(s)) => {
                let mut panes = HashMap::new();
                for p in s.panes {
                    let model = proto::inflate(&p.deflated, MODEL_MAX).ok_or("a pane's model that does not inflate")?;
                    let mut parser = vt100::Parser::new(1, 1, 0);
                    if !parser.screen_mut().restore(&model) {
                        return Err(format!("pane %{}'s model is not one this keepane reads", p.id));
                    }
                    panes.insert(p.id, Pane { parser, at: p.at, fullscreen: p.fullscreen });
                }
                self.panes = panes;
                self.shape = s.shape;
                self.last.store(s.seq, Ordering::Relaxed);
                self.snapshots += 1;
                Ok(())
            }
            Some(to_client::Kind::Entry(e)) => {
                let last = self.last.load(Ordering::Relaxed);
                if e.seq != last + 1 {
                    return Err(format!("change #{} after #{last}", e.seq));
                }
                self.changes += 1;
                match e.change {
                    None => {}
                    Some(proto::entry::Change::Shape(shape)) => {
                        let ids: std::collections::HashSet<u32> = shape
                            .sessions
                            .iter()
                            .flat_map(|s| &s.windows)
                            .flat_map(|w| &w.panes)
                            .map(|p| p.id)
                            .collect();
                        self.panes.retain(|id, _| ids.contains(id));
                        self.shape = Some(shape);
                    }
                    Some(proto::entry::Change::Pane(pc)) => match pc.kind {
                        None => {}
                        Some(Kind::Reset(r)) => {
                            let (rows, cols) = (dim(r.rows)?, dim(r.cols)?);
                            let mut parser = vt100::Parser::new(rows, cols, history);
                            parser.process(&r.bytes);
                            self.panes.insert(pc.pane, Pane { parser, at: r.at, fullscreen: false });
                        }
                        Some(Kind::Screen(model)) => {
                            let bytes =
                                proto::inflate(&model.deflated, MODEL_MAX).ok_or("a model that does not inflate")?;
                            let mut parser = vt100::Parser::new(1, 1, 0);
                            if !parser.screen_mut().restore(&bytes) {
                                return Err(format!("pane %{}'s model is not one this keepane reads", pc.pane));
                            }
                            let fullscreen = parser.screen().alternate_screen();
                            self.panes.insert(pc.pane, Pane { parser, at: model.at, fullscreen });
                        }
                        Some(kind) => {
                            let p = self
                                .panes
                                .get_mut(&pc.pane)
                                .ok_or_else(|| format!("a change of pane %{}, not in the copy", pc.pane))?;
                            match kind {
                                Kind::Output(o) => {
                                    if o.at != p.at {
                                        return Err(format!("pane %{}'s bytes from {} after {}", pc.pane, o.at, p.at));
                                    }
                                    p.parser.process(&o.bytes);
                                    p.at += o.bytes.len() as u64;
                                }
                                Kind::Size(s) => p.parser.screen_mut().set_size(dim(s.rows)?, dim(s.cols)?),
                                Kind::Fullscreen(f) => p.fullscreen = f.on,
                                Kind::CommandStart(_) | Kind::CommandEnd(_) => {}
                                Kind::Reset(_) | Kind::Screen(_) => unreachable!(),
                            }
                        }
                    },
                }
                self.last.store(e.seq, Ordering::Relaxed);
                Ok(())
            }
        }
    }
}

/// A size from the wire: 1 to 1000.
fn dim(v: u32) -> Result<u16, String> {
    u16::try_from(v).ok().filter(|v| (1..=1000).contains(v)).ok_or_else(|| format!("a size of {v}"))
}

/// The connection to `addr`, made again whenever it drops.
#[allow(clippy::too_many_arguments)]
async fn mirror_task(
    addr: String,
    theirs: [u8; 32],
    mine: [u8; 32],
    key: String,
    round: u64,
    events: tokio::sync::mpsc::UnboundedSender<Event>,
    resync: Arc<AtomicBool>,
    last: Arc<AtomicU64>,
) {
    let tell = |news: MirrorNews| events.send(Event::Mirror(round, key.clone(), news)).is_ok();
    // The run of that server the numbers are of (`Answer::epoch`).
    let epoch = AtomicU64::new(0);
    let mut wait = RETRY_FIRST;
    loop {
        if !tell(MirrorNews::Connecting) {
            return;
        }
        let started = Instant::now();
        let why = match connect_once(&addr, &theirs, &mine, &resync, &last, &epoch, &tell).await {
            Ok(()) => "the connection ended".to_string(),
            Err(e) => format!("{e:#}"),
        };
        if !tell(MirrorNews::Down(why)) {
            return;
        }
        // A connection that lived a while was not the trouble: try soon.
        if started.elapsed() > RETRY_LONGEST {
            wait = RETRY_FIRST;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(RETRY_LONGEST);
    }
}

async fn connect_once(
    addr: &str,
    theirs: &[u8; 32],
    mine: &[u8; 32],
    resync: &AtomicBool,
    last: &AtomicU64,
    epoch: &AtomicU64,
    tell: &impl Fn(MirrorNews) -> bool,
) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let mut s = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(addr))
        .await
        .context("no answer")?
        .context("cannot connect")?;
    let _ = s.set_nodelay(true);
    s.write_all(link_stream::request(addr).as_bytes()).await?;
    let head = tokio::time::timeout(CONNECT, link_stream::read_answer_head(&mut s)).await.context("no answer")??;
    if head.status != 101 {
        bail!("{}", head.refusal());
    }
    let since = if resync.swap(false, Ordering::Relaxed) {
        None
    } else {
        Some(last.load(Ordering::Relaxed)).filter(|n| *n > 0)
    };
    let known = epoch.load(Ordering::Relaxed);
    let hello = Hello {
        since,
        version: env!("CARGO_PKG_VERSION").into(),
        time: crate::link::now(),
        epoch: (known != 0).then_some(known),
    };
    let (calling, msg1) = link_stream::call(mine, theirs, &hello)?;
    link_stream::write_frame(&mut s, &msg1).await?;
    let msg2 = tokio::time::timeout(CONNECT, link_stream::read_frame(&mut s))
        .await
        .context("no answer to the handshake")??;
    let (answer, keys) = calling.read(&msg2)?;
    if let Some(why) = answer.refused {
        bail!("refused: {why}");
    }
    epoch.store(answer.epoch, Ordering::Relaxed);
    tell(MirrorNews::Live);
    let (sealer, mut opener) = link_stream::split(keys);
    let (mut rd, mut wr) = s.into_split();
    let (frames_tx, mut frames) = tokio::sync::mpsc::channel::<anyhow::Result<Vec<u8>>>(64);
    let reader = tokio::spawn(async move {
        loop {
            let f = link_stream::read_frame(&mut rd).await;
            let end = f.is_err();
            if frames_tx.send(f).await.is_err() || end {
                break;
            }
        }
    });
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let (mut sent, mut heard) = (Instant::now(), Instant::now());
    let result = async {
        loop {
            tokio::select! {
                frame = frames.recv() => {
                    let Some(frame) = frame else { return Ok(()) };
                    let frame = frame?;
                    let len = frame.len() + 2;
                    heard = Instant::now();
                    for (i, msg) in opener.open::<ToClient>(&frame)?.into_iter().enumerate() {
                        if !tell(MirrorNews::Msg(Box::new(msg), if i == 0 { len } else { 0 })) {
                            return Ok(());
                        }
                    }
                }
                _ = tick.tick() => {
                    if resync.load(Ordering::Relaxed) {
                        bail!("the copy starts again");
                    }
                    if heard.elapsed() > SILENCE {
                        bail!("nothing heard for {} s", SILENCE.as_secs());
                    }
                    if sent.elapsed() > PING {
                        sealer.send(&mut wr, &ToServer { kind: Some(crate::link_proto::to_server::Kind::Ping(Ping {})) }).await?;
                        sent = Instant::now();
                    }
                }
            }
        }
    }
    .await;
    reader.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_proto::{Entry, Output, PaneChange, PaneScreen, Snapshot, entry, pane_change};

    fn snapshot(seq: u64, panes: &[(u32, &str, u64)]) -> ToClient {
        let panes = panes
            .iter()
            .map(|&(id, text, at)| {
                let mut p = vt100::Parser::new(5, 20, 10);
                p.process(text.as_bytes());
                PaneScreen { id, fullscreen: false, deflated: proto::deflate(&p.screen().snapshot()), at }
            })
            .collect();
        ToClient { kind: Some(to_client::Kind::Snapshot(Snapshot { seq, shape: None, panes })) }
    }

    fn output(seq: u64, pane: u32, at: u64, bytes: &str) -> ToClient {
        let pc = PaneChange { pane, kind: Some(pane_change::Kind::Output(Output { at, bytes: bytes.into() })) };
        ToClient { kind: Some(to_client::Kind::Entry(Entry { seq, at: 0, change: Some(entry::Change::Pane(pc)) })) }
    }

    fn text(m: &Mirror, pane: u32) -> String {
        m.panes[&pane].parser.screen().contents().trim_end().to_string()
    }

    #[test]
    fn a_copy_follows_its_changes_in_order_and_starts_again_on_any_doubt() {
        let mut m = Mirror::detached();
        m.apply(snapshot(10, &[(1, "hello", 5)]), 100).unwrap();
        assert_eq!((m.last.load(Ordering::Relaxed), text(&m, 1)), (10, "hello".into()));
        m.apply(output(11, 1, 5, " world"), 100).unwrap();
        assert_eq!(text(&m, 1), "hello world");
        assert_eq!(m.panes[&1].at, 11);
        // Out of order: a change missed.
        assert!(m.apply(output(13, 1, 11, "!"), 100).unwrap_err().contains("#13 after #11"));
        // Bytes that do not start where the last ended.
        assert!(m.apply(output(12, 1, 99, "!"), 100).unwrap_err().contains("from 99 after 11"));
        // A pane the copy does not have.
        assert!(m.apply(output(12, 7, 0, "!"), 100).unwrap_err().contains("not in the copy"));
        // A model it cannot read.
        let bad = ToClient {
            kind: Some(to_client::Kind::Snapshot(Snapshot {
                seq: 20,
                shape: None,
                panes: vec![PaneScreen { id: 1, fullscreen: false, deflated: proto::deflate(b"junk"), at: 0 }],
            })),
        };
        assert!(m.apply(bad, 100).is_err());
        assert_eq!(text(&m, 1), "hello world", "a refused snapshot changes nothing");
        // A snapshot puts it right whatever came before.
        m.apply(snapshot(30, &[(2, "fresh", 0)]), 100).unwrap();
        assert_eq!(m.last.load(Ordering::Relaxed), 30);
        assert!(!m.panes.contains_key(&1));
        m.apply(output(31, 2, 0, "!"), 100).unwrap();
        assert_eq!(text(&m, 2), "fresh!");
    }

    #[test]
    fn a_lost_copy_asks_once_and_waits_for_its_snapshot() {
        let mut m = Mirror::detached();
        m.receive(snapshot(10, &[(1, "x", 1)]), 100);
        m.receive(output(12, 1, 1, "!"), 100);
        assert!(m.lost && m.resync.load(Ordering::Relaxed));
        // What comes before the snapshot is let go, not counted again.
        for seq in 13..20 {
            m.receive(output(seq, 1, 1, "!"), 100);
        }
        assert_eq!(m.restarts, 1);
        m.receive(snapshot(30, &[(1, "y", 1)]), 100);
        assert!(!m.lost);
        m.receive(output(31, 1, 1, "z"), 100);
        assert_eq!((m.restarts, text(&m, 1)), (1, "yz".to_string()));
    }

    #[test]
    fn a_size_from_the_wire_is_held_to_what_a_pane_can_be() {
        assert_eq!(dim(80), Ok(80));
        assert!(dim(0).is_err() && dim(1001).is_err() && dim(u32::MAX).is_err());
    }
}
