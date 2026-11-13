//! The machine that copies: a copy of another machine's sessions, kept up
//! to date over a link stream (docs/design/link.md, decision 17;
//! `crate::link_stream`). Every paired machine is copied by itself
//! (`mirror_peers`; `keepane link mirror <machine>` asks again now, `--stop`
//! stops); a task holds the connection (and makes it again when it drops,
//! from the last change it had), and hands what comes to the server
//! (`Event::Mirror`).
//! The machine's sessions are sessions here (`mirrored`): each shape makes
//! them what they are there, and each pane's model is fed the same bytes as
//! the original. A change out of order, or bytes that do not start where
//! the last ones ended, start the copy again from a snapshot. What is typed
//! into one of its panes goes back over the same connection. `show-mirror`
//! shows it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::mirrored::{dim, model_of};
use super::pane::Callbacks;
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
/// The wait after a refusal (the machine does not give this one all of
/// itself, or its `keepane web` is off): asked again once a minute, not
/// after a second. `link mirror <machine>` asks again at once.
const RETRY_REFUSED: Duration = Duration::from_secs(60);
/// A pane's model, inflated, at most (a long scrollback, a big screen).
const MODEL_MAX: usize = 256 << 20;
/// How long a command sent there waits for its reply.
const REPLY_WAIT: Duration = Duration::from_secs(10);

/// A command sent there, waiting for its reply.
struct Ask {
    /// Who asked, told the outcome (none: a key binding, a hook).
    cid: Option<super::ClientId>,
    /// The session (here) whose eye goes where the command put it there.
    follow: Option<super::SessionId>,
    at: Instant,
}

pub(super) enum MirrorNews {
    Connecting,
    /// Connected; whether that machine runs commands sent to it
    /// (`link_proto::Answer::commands`).
    Live(bool),
    Down(String),
    Msg(Box<ToClient>, usize),
}

enum State {
    Connecting,
    Live(Instant),
    Down(String, Instant),
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
    /// What is typed into its panes, for the connection to send.
    input: mpsc::UnboundedSender<ToServer>,
    /// Keys typed into it while it had no connection, let go (`Typed`).
    dropped: Arc<AtomicU64>,
    /// That machine runs the commands sent to it (keepane 0.63 on).
    commands: bool,
    /// Commands sent there, by number, waiting for their replies.
    asks: HashMap<u64, Ask>,
    next_ask: u64,
    /// The resize a border drag sent last, while it waits for its reply.
    drag: Option<u64>,
    state: State,
    /// The machine's shape as last told (its sessions are made from it).
    shape: Option<proto::Shape>,
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
            self.mirrors_stopped.insert(key.clone());
            return match self.mirror_remove(&key, "is no longer copied") {
                Some(addr) => Outcome::Text(format!("link: stopped copying {addr}\n")),
                None => Outcome::Error(format!("link: {addr} is not being copied")),
            };
        }
        self.mirrors_stopped.remove(&key);
        // A copy that is down is asked to connect again at once: the
        // machine may give this one all of itself now.
        if let Some(m) = self.mirrors.get(&key).filter(|m| !matches!(m.state, State::Down(..))) {
            return Outcome::Text(format!("link: {} is copied already (`show-mirror {}`)\n", m.addr, m.addr));
        }
        match self.start_mirror(&key) {
            Ok(addr) => Outcome::Text(format!("link: copying {addr}'s sessions (`show-mirror {addr}`)\n")),
            Err(e) => Outcome::Error(e),
        }
    }

    /// Every machine paired with this one copied (docs/design/link.md,
    /// decision 17), but those whose copy was stopped: one that gives this
    /// machine all of itself answers, and its sessions are sessions here; one
    /// that does not refuses, and is asked again now and then. A machine no
    /// longer paired is no longer copied; one at a new address is copied
    /// from there. Nothing until this machine has paired with one.
    pub(super) fn mirror_peers(&mut self) {
        self.mirror_asks_due();
        if self.link.is_none() && !self.link_dir().join("authorized").exists() {
            return;
        }
        let peers: Vec<(String, String)> = match self.link() {
            Ok(l) => l.peers.iter().map(|p| (p.key.clone(), p.addr.clone())).collect(),
            Err(_) => return,
        };
        let gone: Vec<String> =
            self.mirrors.keys().filter(|k| !peers.iter().any(|(key, _)| key == *k)).cloned().collect();
        for key in gone {
            self.mirror_remove(&key, "is no longer paired");
        }
        for (key, addr) in peers {
            let moved = self.mirrors.get(&key).is_some_and(|m| m.addr != addr);
            if self.mirrors_stopped.contains(&key) || (self.mirrors.contains_key(&key) && !moved) {
                continue;
            }
            if let Err(e) = self.start_mirror(&key) {
                log::warn!("link mirror {addr}: {e}");
                self.mirrors_stopped.insert(key);
            }
        }
    }

    /// A copy of the machine paired by `key`, started (again: one that was
    /// there is replaced, its sessions kept until the new one's snapshot
    /// says what they are). Its address back.
    fn start_mirror(&mut self, key: &str) -> Result<String, String> {
        let key = key.to_string();
        let mine = self.link()?.id.x25519_secret();
        let addr = self.link.as_ref().and_then(|l| l.peer_by_key(&key)).map(|p| p.addr.clone()).ok_or("not paired")?;
        let Some(theirs) = link_stream::x25519_public(&key) else {
            return Err(format!("link: the key paired with {addr} is not one"));
        };
        self.next_mirror += 1;
        let round = self.next_mirror;
        let resync = Arc::new(AtomicBool::new(false));
        let last = Arc::new(AtomicU64::new(0));
        let (input, rx) = mpsc::unbounded_channel();
        let dropped = Arc::new(AtomicU64::new(0));
        let typed = Typed { rx, dropped: dropped.clone() };
        let task = Some(tokio::spawn(mirror_task(
            addr.clone(),
            theirs,
            mine,
            key.clone(),
            round,
            self.events.clone(),
            resync.clone(),
            last.clone(),
            typed,
        )));
        let mut m = Mirror::new(addr.clone(), round, task, resync, last, input);
        m.dropped = dropped;
        self.mirror_put(key, m);
        Ok(addr)
    }

    /// A copy kept for machine `key`; the one it replaces (the machine
    /// moved): what that sent is not answered on the new connection, and
    /// its askers are told so.
    fn mirror_put(&mut self, key: String, m: Mirror) {
        if let Some(mut old) = self.mirrors.insert(key, m) {
            let why = format!("the connection to {} was made again: what became of it shows in its sessions", old.addr);
            self.mirror_unanswered(std::mem::take(&mut old.asks), &why);
        }
    }

    /// Machine `key` no longer copied (`why`: after its address): its
    /// sessions go, and whoever waits for a reply from it is told. Its
    /// address back, when it was copied.
    fn mirror_remove(&mut self, key: &str, why: &str) -> Option<String> {
        let mut m = self.mirrors.remove(key)?;
        let why = format!("{} {why}", m.addr);
        self.mirror_unanswered(std::mem::take(&mut m.asks), &why);
        self.mirror_forget(key, &why);
        Some(m.addr.clone())
    }

    /// Commands sent there that will get no reply: their askers are told.
    fn mirror_unanswered(&mut self, asks: HashMap<u64, Ask>, why: &str) {
        for cid in asks.into_values().filter_map(|a| a.cid) {
            self.reply(cid, Outcome::Error(why.to_string()));
        }
    }

    /// Send a command to machine `key` about one of its sessions (`line`
    /// names its places by its ids: `mirrored::mirror_route`). Its outcome
    /// goes to `cid` when the reply comes; `follow`: the session here whose
    /// eye then goes where the command put it there.
    pub(super) fn mirror_send(
        &mut self,
        key: &str,
        line: String,
        cid: Option<super::ClientId>,
        follow: Option<super::SessionId>,
    ) -> Outcome {
        let Some(m) = self.mirrors.get_mut(key) else {
            return Outcome::Error("that machine is not copied any more".into());
        };
        if !matches!(m.state, State::Live(_)) {
            return Outcome::Error(format!(
                "no connection to {} now: its sessions are as last seen, and only it can change them",
                m.addr
            ));
        }
        if !m.commands {
            return Outcome::Error(format!(
                "{}'s keepane takes no commands from here (0.62 or before): upgrade it, or do it there",
                m.addr
            ));
        }
        m.next_ask += 1;
        let id = m.next_ask;
        let cmd = ToServer {
            kind: Some(crate::link_proto::to_server::Kind::Command(crate::link_proto::Command { id, line })),
        };
        if m.input.send(cmd).is_err() {
            return Outcome::Error(format!("no connection to {} now", m.addr));
        }
        m.asks.insert(id, Ask { cid, follow, at: Instant::now() });
        match cid {
            Some(_) => Outcome::Pending,
            None => Outcome::Ok,
        }
    }

    /// A border of another machine's window dragged here: a resize of pane
    /// `pane` sent there (`mirror_send`), unless the last one is not
    /// answered yet (false: the way dragged adds up for the next); `last`
    /// (the button went up) sends it anyway.
    pub(super) fn mirror_drag(
        &mut self,
        sid: super::SessionId,
        pane: super::PaneId,
        dir: crate::command::Dir,
        amount: u16,
        last: bool,
    ) -> bool {
        let Some(key) = self.session(sid).and_then(|s| s.remote.as_ref()).map(|r| r.key.clone()) else { return true };
        if !last && self.mirrors.get(&key).is_some_and(|m| m.drag.is_some_and(|id| m.asks.contains_key(&id))) {
            return false;
        }
        let target = Some(crate::command::Target { pane_id: Some(pane), ..Default::default() });
        let cmd =
            crate::command::Cmd::ResizePane { dir: Some(dir), amount, zoom: false, target, width: None, height: None };
        if let Some(Outcome::Ok) = self.mirror_route(&cmd, None)
            && let Some(m) = self.mirrors.get_mut(&key)
        {
            m.drag = Some(m.next_ask);
        }
        true
    }
    /// The reply to a command sent there: after the changes it made, which
    /// the copy has; the eye follows where it asked to; whoever asked is
    /// told.
    fn mirror_reply(&mut self, key: &str, r: crate::link_proto::Reply) {
        let Some(ask) = self.mirrors.get_mut(key).and_then(|m| m.asks.remove(&r.id)) else { return };
        if r.ok
            && let (Some(sid), Some(pane)) = (ask.follow, r.pane)
            && let Some(local) = self.mirror_pane_mut(key, pane).map(|p| p.id)
            && let Some(s) = self.session_mut(sid)
            && let Some(widx) = s.windows.iter().position(|w| w.pane(local).is_some())
        {
            s.select_window(widx);
            let w = &mut s.windows[widx];
            if w.active != local {
                w.last_pane = Some(w.active);
                w.active = local;
            }
            super::mirrored::zoom_on_active(w);
        }
        if let Some(cid) = ask.cid {
            let out = match (r.ok, r.text.trim_end().is_empty()) {
                (false, _) => Outcome::Error(r.text),
                (true, true) => Outcome::Ok,
                (true, false) => Outcome::Text(r.text),
            };
            self.reply(cid, out);
        }
    }

    /// Commands sent there and not answered in `REPLY_WAIT`: their askers
    /// are told (what became of them shows in the shape).
    fn mirror_asks_due(&mut self) {
        let mut due = Vec::new();
        for m in self.mirrors.values_mut() {
            let late: Vec<u64> =
                m.asks.iter().filter(|(_, a)| a.at.elapsed() > REPLY_WAIT).map(|(id, _)| *id).collect();
            for id in late {
                if let Some(a) = m.asks.remove(&id) {
                    due.push((a.cid, m.addr.clone()));
                }
            }
        }
        for (cid, addr) in due {
            if let Some(cid) = cid {
                let why = format!(
                    "no reply from {addr} in {} s: what became of it shows in its sessions",
                    REPLY_WAIT.as_secs()
                );
                self.reply(cid, Outcome::Error(why));
            }
        }
    }
    /// What a mirror's task says.
    pub(super) fn mirror_news(&mut self, round: u64, key: String, news: MirrorNews) {
        let Some(m) = self.mirrors.get_mut(&key).filter(|m| m.round == round) else { return };
        match news {
            MirrorNews::Connecting => {
                if !matches!(m.state, State::Down(..)) {
                    m.state = State::Connecting;
                }
            }
            MirrorNews::Live(commands) => {
                m.connects += 1;
                m.commands = commands;
                m.state = State::Live(Instant::now());
            }
            MirrorNews::Down(why) => {
                if !matches!(&m.state, State::Down(w, _) if *w == why) {
                    log::info!("link mirror {}: {why}", m.addr);
                }
                m.state = State::Down(why, Instant::now());
                // What was sent and not answered: what became of it shows
                // when the connection is back (the shape says).
                let why = format!("the connection to {} dropped: what became of it shows once it is back", m.addr);
                let asks = std::mem::take(&mut m.asks);
                self.mirror_unanswered(asks, &why);
            }
            MirrorNews::Msg(msg, len) => {
                m.bytes_in += len as u64;
                self.mirror_receive(&key, *msg);
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
            return Outcome::Error(format!("{addr} is not being copied: `keepane link mirror {addr}` copies it"));
        };
        let panes = self.mirror_panes(&key);
        if let Some(id) = pane {
            let Some(p) = panes.iter().find(|p| p.remote.is_some_and(|r| r.id == id)) else {
                return Outcome::Error(format!("no pane %{id} in the copy of {}", m.addr));
            };
            let screen = p.parser.screen();
            let (rows, cols) = screen.size();
            let at = p.remote.map_or(0, |r| r.at);
            let text: Vec<String> = screen.rows(0, cols).map(|r| r.trim_end().to_string()).collect();
            if json {
                return Outcome::Text(
                    serde_json::json!({
                        "id": id, "local": format!("%{}", p.id), "cols": cols, "rows": rows, "at": at,
                        "fullscreen": screen.alternate_screen(), "text": text,
                    })
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
            let panes: Vec<serde_json::Value> = panes
                .iter()
                .map(|p| {
                    let (rows, cols) = p.parser.screen().size();
                    serde_json::json!({
                        "id": p.remote.map_or(0, |r| r.id), "local": format!("%{}", p.id), "cols": cols, "rows": rows,
                        "at": p.remote.map_or(0, |r| r.at), "fullscreen": p.parser.screen().alternate_screen(),
                    })
                })
                .collect();
            return Outcome::Text(m.json(panes).to_string());
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

    /// Take one message in; one the copy cannot follow from loses it: it
    /// asks for a snapshot, and lets what comes before that go.
    pub(super) fn mirror_receive(&mut self, key: &str, msg: ToClient) {
        // A reply comes outside the numbered changes, and is taken however
        // the copy stands.
        if let Some(to_client::Kind::Reply(r)) = msg.kind {
            return self.mirror_reply(key, r);
        }
        let snapshot = matches!(msg.kind, Some(to_client::Kind::Snapshot(_)));
        if self.mirrors.get(key).is_none_or(|m| m.lost && !snapshot) {
            return;
        }
        let result = self.mirror_apply(key, msg);
        let Some(m) = self.mirrors.get_mut(key) else { return };
        match result {
            Ok(()) if snapshot => m.lost = false,
            Ok(()) => {}
            Err(why) => {
                log::warn!("link mirror {}: {why}: starting again from a snapshot", m.addr);
                m.restarts += 1;
                m.lost = true;
                m.resync.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Take one message into the copy. An error: the copy no longer follows
    /// the original, and starts again from a snapshot; a snapshot that does
    /// not read changes nothing.
    fn mirror_apply(&mut self, key: &str, msg: ToClient) -> Result<(), String> {
        use proto::pane_change::Kind;
        let history = self.opts.history_limit;
        let m = self.mirrors.get(key).ok_or("not copied")?;
        let (addr, input, last) = (m.addr.clone(), m.input.clone(), m.last.load(Ordering::Relaxed));
        match msg.kind {
            None | Some(to_client::Kind::Ping(_)) | Some(to_client::Kind::Reply(_)) => Ok(()),
            Some(to_client::Kind::Snapshot(s)) => {
                let mut models = HashMap::new();
                for p in s.panes {
                    let bytes = proto::inflate(&p.deflated, MODEL_MAX).ok_or("a pane's model that does not inflate")?;
                    let parser = model_of(&bytes)
                        .ok_or_else(|| format!("pane %{}'s model is not one this keepane reads", p.id))?;
                    models.insert(p.id, (parser, p.at));
                }
                let shape = s.shape.unwrap_or_default();
                self.mirror_shape(key, &addr, &input, &shape, models);
                let m = self.mirrors.get_mut(key).expect("found above");
                m.shape = Some(shape);
                m.last.store(s.seq, Ordering::Relaxed);
                m.snapshots += 1;
                Ok(())
            }
            Some(to_client::Kind::Entry(e)) => {
                if e.seq != last + 1 {
                    return Err(format!("change #{} after #{last}", e.seq));
                }
                match e.change {
                    None => {}
                    Some(proto::entry::Change::Shape(shape)) => {
                        self.mirror_shape(key, &addr, &input, &shape, HashMap::new());
                        self.mirrors.get_mut(key).expect("found above").shape = Some(shape);
                    }
                    Some(proto::entry::Change::Pane(pc)) => {
                        let p = self
                            .mirror_pane_mut(key, pc.pane)
                            .ok_or_else(|| format!("a change of pane %{}, not in the copy", pc.pane))?;
                        match pc.kind {
                            None | Some(Kind::Fullscreen(_) | Kind::CommandStart(_) | Kind::CommandEnd(_)) => {}
                            Some(Kind::Reset(r)) => {
                                let (rows, cols) = (dim(r.rows)?, dim(r.cols)?);
                                let parser =
                                    vt100::Parser::new_with_callbacks(rows, cols, history, Callbacks::default());
                                p.set_remote_model(parser, 0);
                                p.feed_remote(&r.bytes);
                                p.set_remote_at(r.at);
                            }
                            Some(Kind::Screen(model)) => {
                                let bytes = proto::inflate(&model.deflated, MODEL_MAX)
                                    .ok_or("a model that does not inflate")?;
                                let parser = model_of(&bytes).ok_or_else(|| {
                                    format!("pane %{}'s model is not one this keepane reads", pc.pane)
                                })?;
                                p.set_remote_model(parser, model.at);
                            }
                            Some(Kind::Output(o)) => {
                                let at = p.remote.map_or(0, |r| r.at);
                                if o.at != at {
                                    return Err(format!("pane %{}'s bytes from {} after {at}", pc.pane, o.at));
                                }
                                p.feed_remote(&o.bytes);
                            }
                            Some(Kind::Size(s)) => p.set_remote_size(dim(s.cols)?, dim(s.rows)?),
                        }
                    }
                }
                let m = self.mirrors.get_mut(key).expect("found above");
                m.changes += 1;
                m.last.store(e.seq, Ordering::Relaxed);
                Ok(())
            }
        }
    }
}

impl Mirror {
    fn new(
        addr: String,
        round: u64,
        task: Option<tokio::task::JoinHandle<()>>,
        resync: Arc<AtomicBool>,
        last: Arc<AtomicU64>,
        input: mpsc::UnboundedSender<ToServer>,
    ) -> Mirror {
        Mirror {
            addr,
            round,
            task,
            resync,
            last,
            input,
            dropped: Arc::default(),
            commands: false,
            asks: HashMap::new(),
            next_ask: 0,
            drag: None,
            state: State::Connecting,
            shape: None,
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
        let dropped = match self.dropped.load(Ordering::Relaxed) {
            0 => String::new(),
            n => format!("; {n} keys let go (typed with no connection)"),
        };
        format!(
            "{}: {state}; #{}; {s} sessions, {w} windows, {p} panes; {} KiB in, {} changes, {} snapshots, {} connections, {} restarts{dropped}",
            self.addr,
            self.last.load(Ordering::Relaxed),
            self.bytes_in / 1024,
            self.changes,
            self.snapshots,
            self.connects,
            self.restarts
        )
    }

    fn json(&self, panes: Vec<serde_json::Value>) -> serde_json::Value {
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
        serde_json::json!({
            "addr": self.addr, "status": state, "seq": self.last.load(Ordering::Relaxed),
            "sessions": sessions,
            "panes": panes,
            "bytes_in": self.bytes_in, "changes": self.changes, "snapshots": self.snapshots,
            "connects": self.connects, "restarts": self.restarts,
            "keys_dropped": self.dropped.load(Ordering::Relaxed),
        })
    }
}

/// The machine would not be copied (its answer said so, or its web did):
/// asked again after `RETRY_REFUSED`.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

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
    mut typed: Typed,
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
        let (why, refused) = match connect_once(&addr, &theirs, &mine, &resync, &last, &epoch, &tell, &mut typed).await
        {
            Ok(()) => ("the connection ended".to_string(), false),
            Err(e) => (format!("{e:#}"), e.downcast_ref::<Refused>().is_some()),
        };
        if !tell(MirrorNews::Down(why)) {
            return;
        }
        // A connection that lived a while was not the trouble: try soon.
        if started.elapsed() > RETRY_LONGEST {
            wait = RETRY_FIRST;
        }
        if refused {
            typed.wait(RETRY_REFUSED).await;
            continue;
        }
        typed.wait(wait).await;
        wait = (wait * 2).min(RETRY_LONGEST);
    }
}

/// What is typed into a copy's panes, for its connection to send; and how
/// many were let go because there was no connection (`show-mirror`).
struct Typed {
    rx: mpsc::UnboundedReceiver<ToServer>,
    dropped: Arc<AtomicU64>,
}

impl Typed {
    /// Let go what was typed while there was no connection: it is never
    /// typed later.
    fn let_go(&mut self) {
        while self.rx.try_recv().is_ok() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Wait `d`, letting go what is typed meanwhile: there is no connection
    /// to send it on, and it is never sent later (nor kept, however long the
    /// machine is away).
    async fn wait(&mut self, d: Duration) {
        let end = tokio::time::sleep(d);
        tokio::pin!(end);
        loop {
            tokio::select! {
                _ = &mut end => return,
                keys = self.rx.recv() => match keys {
                    Some(_) => { self.dropped.fetch_add(1, Ordering::Relaxed); }
                    // No copy left to type into: the task ends with it.
                    None => return end.await,
                },
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn connect_once(
    addr: &str,
    theirs: &[u8; 32],
    mine: &[u8; 32],
    resync: &AtomicBool,
    last: &AtomicU64,
    epoch: &AtomicU64,
    tell: &impl Fn(MirrorNews) -> bool,
    typed: &mut Typed,
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
        return Err(Refused(head.refusal()).into());
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
        return Err(Refused(format!("refused: {why}")).into());
    }
    epoch.store(answer.epoch, Ordering::Relaxed);
    // Keys typed while there was no connection are let go: typed into a
    // copy that was not there, they are never typed later.
    typed.let_go();
    tell(MirrorNews::Live(answer.commands));
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
                keys = typed.rx.recv() => {
                    let Some(keys) = keys else { return Ok(()) };
                    sealer.send(&mut wr, &keys).await?;
                    sent = Instant::now();
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
    use crate::command::Target;
    use crate::link_proto::{
        Entry, Output, PaneChange, PaneScreen, PaneShape, SessionShape, Shape, Snapshot, WindowShape, entry,
        pane_change, to_server,
    };
    use crate::server::layout;
    use crate::server::{ClientId, MOUSE_MOVED};

    const KEY: &str = "k";

    /// A server with a copy of machine `KEY`, taking messages by hand; and
    /// what its panes are typed.
    fn copying() -> (Server, mpsc::UnboundedReceiver<ToServer>) {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        let (input, typed) = mpsc::unbounded_channel();
        let m = Mirror::new("10.0.0.2:7681".into(), 1, None, Default::default(), Default::default(), input);
        s.mirrors.insert(KEY.into(), m);
        (s, typed)
    }

    /// One session `work` (id 1), one window (id 2) of `panes` side by side.
    fn shape(panes: &[u32]) -> Shape {
        let panes = panes
            .iter()
            .enumerate()
            .map(|(i, &id)| PaneShape {
                id,
                x: 21 * i as u32,
                y: 0,
                cols: 20,
                rows: 5,
                command: "sh".into(),
                mode: "shell".into(),
                ..Default::default()
            })
            .collect();
        let window = WindowShape { id: 2, index: 0, name: "w".into(), active: 0, panes, ..Default::default() };
        Shape {
            sessions: vec![SessionShape {
                id: 1,
                name: "work".into(),
                cols: 80,
                rows: 6,
                current: 0,
                windows: vec![window],
            }],
        }
    }

    fn snapshot(seq: u64, panes: &[(u32, &str, u64)]) -> ToClient {
        let ids: Vec<u32> = panes.iter().map(|p| p.0).collect();
        let panes = panes
            .iter()
            .map(|&(id, text, at)| {
                let mut p = vt100::Parser::new(5, 20, 10);
                p.process(text.as_bytes());
                PaneScreen { id, fullscreen: false, deflated: proto::deflate(&p.screen().snapshot()), at }
            })
            .collect();
        ToClient { kind: Some(to_client::Kind::Snapshot(Snapshot { seq, shape: Some(shape(&ids)), panes })) }
    }

    fn output(seq: u64, pane: u32, at: u64, bytes: &str) -> ToClient {
        let pc = PaneChange { pane, kind: Some(pane_change::Kind::Output(Output { at, bytes: bytes.into() })) };
        ToClient { kind: Some(to_client::Kind::Entry(Entry { seq, at: 0, change: Some(entry::Change::Pane(pc)) })) }
    }

    fn text(s: &mut Server, pane: u32) -> String {
        s.mirror_pane_mut(KEY, pane).map(|p| p.parser.screen().contents().trim_end().to_string()).unwrap_or_default()
    }

    fn last(s: &Server) -> u64 {
        s.mirrors[KEY].last.load(Ordering::Relaxed)
    }

    #[test]
    fn a_copy_follows_its_changes_in_order_and_starts_again_on_any_doubt() {
        let (mut s, _typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "hello", 5)])).unwrap();
        assert_eq!((last(&s), text(&mut s, 1)), (10, "hello".into()));
        s.mirror_apply(KEY, output(11, 1, 5, " world")).unwrap();
        assert_eq!(text(&mut s, 1), "hello world");
        assert_eq!(s.mirror_pane_mut(KEY, 1).unwrap().remote.unwrap().at, 11);
        // Out of order: a change missed.
        assert!(s.mirror_apply(KEY, output(13, 1, 11, "!")).unwrap_err().contains("#13 after #11"));
        // Bytes that do not start where the last ended.
        assert!(s.mirror_apply(KEY, output(12, 1, 99, "!")).unwrap_err().contains("from 99 after 11"));
        // A pane the copy does not have.
        assert!(s.mirror_apply(KEY, output(12, 7, 0, "!")).unwrap_err().contains("not in the copy"));
        // A model it cannot read.
        let bad = ToClient {
            kind: Some(to_client::Kind::Snapshot(Snapshot {
                seq: 20,
                shape: Some(shape(&[1])),
                panes: vec![PaneScreen { id: 1, fullscreen: false, deflated: proto::deflate(b"junk"), at: 0 }],
            })),
        };
        assert!(s.mirror_apply(KEY, bad).is_err());
        assert_eq!(text(&mut s, 1), "hello world", "a refused snapshot changes nothing");
        // A snapshot puts it right whatever came before.
        s.mirror_apply(KEY, snapshot(30, &[(2, "fresh", 0)])).unwrap();
        assert_eq!(last(&s), 30);
        assert!(s.mirror_pane_mut(KEY, 1).is_none());
        s.mirror_apply(KEY, output(31, 2, 0, "!")).unwrap();
        assert_eq!(text(&mut s, 2), "fresh!");
    }

    #[test]
    fn a_lost_copy_asks_once_and_waits_for_its_snapshot() {
        let (mut s, _typed) = copying();
        s.mirror_receive(KEY, snapshot(10, &[(1, "x", 1)]));
        s.mirror_receive(KEY, output(12, 1, 1, "!"));
        let m = &s.mirrors[KEY];
        assert!(m.lost && m.resync.load(Ordering::Relaxed));
        // What comes before the snapshot is let go, not counted again.
        for seq in 13..20 {
            s.mirror_receive(KEY, output(seq, 1, 1, "!"));
        }
        assert_eq!(s.mirrors[KEY].restarts, 1);
        s.mirror_receive(KEY, snapshot(30, &[(1, "y", 1)]));
        assert!(!s.mirrors[KEY].lost);
        s.mirror_receive(KEY, output(31, 1, 1, "z"));
        assert_eq!((s.mirrors[KEY].restarts, text(&mut s, 1)), (1, "yz".to_string()));
    }

    #[test]
    fn the_machines_sessions_are_sessions_here_kept_as_they_change_there() {
        let (mut s, mut typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "one", 3), (4, "four", 4)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").expect("made");
        let session = s.session(sid).unwrap();
        assert_eq!(session.name, "work@10.0.0.2:7681");
        assert_eq!((session.cols, session.rows, session.windows.len()), (80, 6, 1));
        let w = &session.windows[0];
        let (p1, p4) = (w.panes[0].id, w.panes[1].id);
        assert_eq!(w.layout.panes(), vec![p1, p4], "in the order there");
        assert_eq!(w.rects.iter().map(|(_, r)| (r.x, r.w)).collect::<Vec<_>>(), [(0, 20), (21, 20)]);
        // A bare name never names it; `host:port/name` and the pane's id
        // there do.
        assert!(s.resolve_session(Some(&Target::parse("work")), None).is_err());
        assert_eq!(s.resolve(Some(&Target::parse("10.0.0.2:7681/%4")), None).unwrap(), (sid, 0, p4));
        assert_eq!(s.resolve(Some(&Target::parse("10.0.0.2:7681/work:0.0")), None).unwrap(), (sid, 0, p1));
        // Looking at it never resizes it: the session is the size it is
        // there, and so are its panes.
        s.resize_session(sid, 200, 50);
        let session = s.session(sid).unwrap();
        assert_eq!((session.cols, session.rows, session.windows[0].panes[0].cols), (80, 6, 20));
        // What changes its shape is that machine's to do: with no connection
        // to it, nothing is done here.
        let out = s.exec(crate::command::parse_line("kill-pane -t 10.0.0.2:7681/%1").unwrap().unwrap(), None);
        assert!(matches!(&out, Outcome::Error(e) if e.contains("no connection to 10.0.0.2:7681")));
        assert_eq!(s.mirror_panes(KEY).len(), 2, "nothing was killed");
        // What is typed into a pane goes there, by its id there.
        s.mirror_pane_mut(KEY, 4).unwrap().type_input(b"ls\r");
        match typed.try_recv().unwrap().kind {
            Some(to_server::Kind::Input(i)) => assert_eq!((i.pane, i.bytes.as_slice()), (4, &b"ls\r"[..])),
            k => panic!("{k:?}"),
        }
        // A pane chosen here stays chosen as the shape changes; a pane gone
        // there goes; one new there comes, the panes kept keep what they show.
        s.session_mut(sid).unwrap().windows[0].active = p4;
        // (Their active pane is the first there: 9, not the one chosen here.)
        let mut next = shape(&[9, 4]);
        next.sessions[0].windows[0].name = "renamed".into();
        next.sessions[0].windows[0].active = 9;
        let change = ToClient {
            kind: Some(to_client::Kind::Entry(Entry { seq: 11, at: 0, change: Some(entry::Change::Shape(next)) })),
        };
        s.mirror_apply(KEY, change).unwrap();
        let w = &s.session(sid).unwrap().windows[0];
        assert_eq!((w.name.as_str(), w.active, w.panes[1].id), ("renamed", p4, p4));
        // A pane resized there: its model here takes the size.
        let size =
            PaneChange { pane: 4, kind: Some(pane_change::Kind::Size(crate::link_proto::Size { cols: 30, rows: 7 })) };
        let change = ToClient {
            kind: Some(to_client::Kind::Entry(Entry { seq: 12, at: 0, change: Some(entry::Change::Pane(size)) })),
        };
        s.mirror_apply(KEY, change).unwrap();
        let p = s.mirror_pane_mut(KEY, 4).unwrap();
        assert_eq!((p.cols, p.rows, p.parser.screen().size()), (30, 7, (7, 30)));
        assert_eq!(text(&mut s, 4), "four");
        assert!(s.mirror_pane_mut(KEY, 1).is_none() && s.mirror_pane_mut(KEY, 9).is_some());
        // Its sessions are not this machine's: not in what is told to the
        // machines that copy this one.
        assert!(s.journal_shape().sessions.is_empty());
        // The session gone there is gone here; so is every one when the copy
        // stops.
        let empty = ToClient {
            kind: Some(to_client::Kind::Entry(Entry {
                seq: 13,
                at: 0,
                change: Some(entry::Change::Shape(Shape::default())),
            })),
        };
        s.mirror_apply(KEY, empty).unwrap();
        assert!(s.session(sid).is_none());
        s.mirror_apply(KEY, snapshot(20, &[(1, "x", 1)])).unwrap();
        s.mirror_forget(KEY, "stopped");
        assert!(s.sessions.is_empty());
    }

    #[test]
    fn a_client_smaller_than_the_session_there_sees_part_of_it_and_it_stays_its_size() {
        let (mut s, _typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "left", 4), (4, "right-side", 10)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        s.client_connected(7, tx);
        let c = s.clients.get_mut(&7).unwrap();
        (c.cols, c.rows, c.interactive, c.session) = (30, 4, true, Some(sid));
        s.fit_session(sid, Some(7));
        s.render_client(7);
        let mut screen = vt100::Parser::new(4, 30, 0);
        while let Ok(m) = rx.try_recv() {
            if let crate::ipc::ServerMsg::Output(b) = m {
                screen.process(&b);
            }
        }
        let shown = screen.screen().contents();
        assert!(shown.contains("left"), "{shown}");
        let session = s.session(sid).unwrap();
        assert_eq!((session.cols, session.rows), (80, 6), "the size it is there");
        assert_eq!(session.windows[0].panes[1].cols, 20);
    }

    #[test]
    fn a_third_machines_sessions_are_never_given_on() {
        let (mut s, mut typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "x", 1)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let local = s.mirror_pane_mut(KEY, 1).unwrap().id;
        // A machine with all of this one reaches none of its panes, sees
        // none listed, attaches to none of its sessions.
        let peer = crate::link::Peer { key: "c".into(), addr: "10.0.0.3:7681".into(), full: true, lan: true };
        assert!(!s.may_reach(&peer, local));
        assert!(s.reachable_panes(&peer).is_empty());
        assert_eq!(s.session_to_attach(None), Err("no sessions".into()), "the one used last is not this machine's");
        assert!(s.session_to_attach(Some(&Target::parse(&format!("${sid}")))).is_err());
        // Its history is kept there, not here.
        assert!(s.history_paths().iter().all(|(id, _)| *id != local));
        // Nor shared with it, whole or in part.
        use super::super::link_share::Spot;
        for spot in [Spot::Session(sid), Spot::Pane(local)] {
            match s.share_spots(None, &peer, &[spot], true, false) {
                Outcome::Error(e) => assert!(e.contains("only that machine can share it"), "{e}"),
                _ => panic!("shared"),
            }
        }
        assert!(s.link_shares.is_empty());
        // Typed in by a machine this one feeds: a copied pane is not this
        // machine's to type into.
        let _fed = s.feed_for_test("c");
        s.stream_input("c", local, b"rm -rf /\r");
        assert!(typed.try_recv().is_err());
        // What its model answers (a cursor report, the terminal's name) is
        // the program there's, answered there: nothing goes back.
        s.mirror_apply(KEY, output(11, 1, 1, "\x1b[6n\x1b[c\x1b]10;?\x07")).unwrap();
        assert!(typed.try_recv().is_err());
        // A message goes to its inbox there, by that machine's name for it.
        let err =
            match s.exec(crate::command::parse_line(&format!("send-message -t %{local} hi")).unwrap().unwrap(), None) {
                Outcome::Error(e) => e,
                _ => String::new(),
            };
        assert!(err.contains("send it to 10.0.0.2:7681/%1"), "{err}");
        // Not saved here.
        assert!(s.save_session_file(sid).unwrap_err().contains("saved there"));
        // Laid out or resized from here: nothing moves.
        let area = layout::Rect { x: 0, y: 0, w: 200, h: 50 };
        let w = &mut s.session_mut(sid).unwrap().windows[0];
        let before = w.rects.clone();
        w.relayout(area, super::super::Chrome::Lines);
        assert_eq!(w.rects, before);
        w.panes[0].resize(100, 40);
        assert_eq!((w.panes[0].cols, w.panes[0].rows), (20, 5));
    }

    #[test]
    fn zoomed_there_the_zoomed_pane_is_looked_at_and_a_pane_chosen_here_is_drawn_in_its_place() {
        let (mut s, _typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "one", 3), (4, "four", 4)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let (p1, p4) = {
            let w = &s.session(sid).unwrap().windows[0];
            (w.panes[0].id, w.panes[1].id)
        };
        // Zoomed there on pane 4 (40x5): looked at here too, drawn alone.
        let mut zoomed = shape(&[1, 4]);
        let win = &mut zoomed.sessions[0].windows[0];
        (win.zoomed, win.active) = (true, 4);
        (win.panes[1].x, win.panes[1].cols) = (0, 40);
        let change = |seq, shape| ToClient {
            kind: Some(to_client::Kind::Entry(Entry { seq, at: 0, change: Some(entry::Change::Shape(shape)) })),
        };
        s.mirror_apply(KEY, change(11, zoomed.clone())).unwrap();
        let w = &s.session(sid).unwrap().windows[0];
        assert_eq!((w.active, w.zoomed), (p4, true));
        assert_eq!(w.rects.iter().map(|(id, r)| (*id, r.w)).collect::<Vec<_>>(), [(p4, 20)], "its size: 20 till told");
        // Pane 1 chosen here: drawn where the zoomed one is, at its size;
        // unzooming here (select-pane with keep-zoom off) unzooms nothing.
        let area = s.window_area(80, 6);
        let w = &mut s.session_mut(sid).unwrap().windows[0];
        (w.active, w.zoomed) = (p1, false);
        w.relayout(area, super::super::Chrome::Lines);
        assert_eq!((w.zoomed, w.rects.len(), w.rects[0].0), (true, 1, p1));
        // The same zoom told again: the choice here stays.
        s.mirror_apply(KEY, change(12, zoomed)).unwrap();
        assert_eq!(s.session(sid).unwrap().windows[0].active, p1);
        // Nothing is added to it from here, however it is asked.
        assert!(s.new_window(sid, None, None, &[]).unwrap_err().contains("only that machine can"));
        assert!(s.split_pane(sid, 0, p1, true, None, &[], false, false).unwrap_err().contains("only that machine can"));
    }

    /// A machine that says no (or is away): what is typed into its copy
    /// meanwhile is let go, counted, never kept for later.
    #[tokio::test]
    async fn keys_typed_while_the_machine_says_no_are_let_go_and_counted() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ =
                    s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno").await;
            }
        });
        let (events, mut news) = mpsc::unbounded_channel::<Event>();
        let (input, rx) = mpsc::unbounded_channel();
        let dropped = Arc::new(AtomicU64::new(0));
        let typed = Typed { rx, dropped: dropped.clone() };
        let (resync, last) = (Default::default(), Default::default());
        let task = tokio::spawn(mirror_task(addr, [1; 32], [2; 32], KEY.into(), 1, events, resync, last, typed));
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), news.recv()).await.expect("told").expect("open");
            if let Event::Mirror(_, _, MirrorNews::Down(why)) = event {
                assert!(!why.is_empty());
                break;
            }
        }
        // Waiting a minute to ask again: these go nowhere, now.
        for _ in 0..3 {
            input.send(ToServer { kind: Some(to_server::Kind::Ping(Ping {})) }).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while dropped.load(Ordering::Relaxed) < 3 {
            assert!(Instant::now() < deadline, "kept: {} let go", dropped.load(Ordering::Relaxed));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        let (input, _rx) = mpsc::unbounded_channel();
        let mut m = Mirror::new("10.0.0.2:7681".into(), 1, None, Default::default(), Default::default(), input);
        m.dropped = dropped;
        assert!(m.summary().ends_with("; 3 keys let go (typed with no connection)"), "{}", m.summary());
        assert_eq!(m.json(vec![])["keys_dropped"], 3);
    }
    #[test]
    fn a_shape_that_makes_no_sense_makes_nothing_that_breaks() {
        let (mut s, _typed) = copying();
        let mut odd = shape(&[1, 1, 2]);
        let w = &mut odd.sessions[0].windows[0];
        w.layout = "not a layout".into();
        (w.panes[2].cols, w.panes[2].rows) = (0, 99_999);
        // A session with no windows, a window with no panes, a session with
        // the same id twice.
        odd.sessions.push(SessionShape { id: 5, name: "empty".into(), cols: 0, rows: 0, current: 0, windows: vec![] });
        let mut bare = odd.sessions[0].clone();
        bare.windows[0].panes.clear();
        bare.id = 6;
        odd.sessions.push(bare);
        odd.sessions.push(odd.sessions[0].clone());
        let msg =
            ToClient { kind: Some(to_client::Kind::Snapshot(Snapshot { seq: 1, shape: Some(odd), panes: vec![] })) };
        s.mirror_apply(KEY, msg).unwrap();
        assert!(s.mirror_session("10.0.0.2:7681", "empty").is_none(), "nothing to show: not made");
        assert!(s.mirror_session("10.0.0.2:7681", "$6").is_none());
        for session in &s.sessions {
            let w = &session.windows[session.cur];
            // Every pane in the layout, in the window, and placed.
            assert_eq!(w.layout.panes().len(), w.panes.len());
            assert!(w.layout.panes().iter().all(|id| w.pane(*id).is_some() && w.rect_of(*id).is_some()));
            assert!(w.pane(w.active).is_some());
            assert!(w.panes.iter().all(|p| (1..=1000).contains(&p.cols) && (1..=1000).contains(&p.rows)));
        }
        // Drawn on a client without trouble.
        let sid = s.sessions[0].id;
        let (tx, _rx) = mpsc::channel(64);
        s.client_connected(7, tx);
        let c = s.clients.get_mut(&7).unwrap();
        (c.cols, c.rows, c.interactive, c.session) = (1, 1, true, Some(sid));
        s.render_client(7);
        // Then a sensible shape puts it right.
        s.mirror_apply(KEY, snapshot(2, &[(1, "fine", 4)])).unwrap();
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(text(&mut s, 1), "fine");
    }

    #[test]
    fn a_border_dragged_here_resizes_there_one_resize_at_a_time() {
        let (mut s, mut typed) = copying();
        s.opts.mouse = true;
        s.mirror_apply(KEY, snapshot(10, &[(1, "a", 1), (4, "b", 1)])).unwrap();
        live(&mut s);
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let (tx, _rx) = mpsc::channel(64);
        s.client_connected(7, tx);
        let c = s.clients.get_mut(&7).unwrap();
        (c.cols, c.rows, c.interactive, c.session) = (80, 6, true, Some(sid));
        let mouse = |s: &mut Server, x: i16, buttons: u32, flags: u32| {
            s.handle_mouse(7, crate::ipc::MouseRecord { x, y: 1, buttons, ctrl: 0, flags });
        };
        // The column between the two panes (0-19 and 21-40), dragged right.
        mouse(&mut s, 20, 1, 0);
        assert!(s.clients[&7].drag.is_some());
        mouse(&mut s, 22, 1, MOUSE_MOVED);
        let (id, line) = sent(&mut typed);
        assert_eq!(line, "resize-pane -R 2 -t $1:@2.%1");
        // Not answered yet: the way dragged adds up, nothing more is sent.
        mouse(&mut s, 23, 1, MOUSE_MOVED);
        mouse(&mut s, 25, 1, MOUSE_MOVED);
        assert!(typed.try_recv().is_err());
        assert_eq!(s.mirror_panes(KEY)[0].cols, 20, "nothing resized here");
        reply(&mut s, id, true, "", None);
        mouse(&mut s, 26, 1, MOUSE_MOVED);
        let (id, line) = sent(&mut typed);
        assert_eq!(line, "resize-pane -R 4 -t $1:@2.%1", "all the way since the last one sent");
        // The button up while one is out: what is left goes anyway.
        mouse(&mut s, 24, 0, 0);
        assert_eq!(sent(&mut typed).1, "resize-pane -L 2 -t $1:@2.%1");
        assert!(s.clients[&7].drag.is_none());
        let _ = id;
    }
    #[test]
    fn a_copy_is_placed_again_when_the_window_area_here_changes() {
        let (mut s, _typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "a", 1), (4, "b", 1)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let ys =
            |s: &Server| s.session(sid).unwrap().windows[0].rects.iter().map(|(_, r)| (r.x, r.y)).collect::<Vec<_>>();
        assert_eq!(ys(&s), [(0, 0), (21, 0)]);
        // The status line moved to the top: the panes start a row down,
        // where the window area now starts; the session keeps its size.
        let set = |s: &mut Server, line: &str| s.exec(crate::command::parse_line(line).unwrap().unwrap(), None);
        set(&mut s, "set -g status-position top");
        assert_eq!(ys(&s), [(0, 1), (21, 1)]);
        set(&mut s, "set -g status off");
        assert_eq!(ys(&s), [(0, 0), (21, 0)]);
        let session = s.session(sid).unwrap();
        assert_eq!((session.cols, session.rows, session.windows[0].panes[0].cols), (80, 6, 20));
    }

    /// Live, taking commands (as a keepane of 0.63 on answers).
    fn live(s: &mut Server) {
        let m = s.mirrors.get_mut(KEY).unwrap();
        (m.state, m.commands) = (State::Live(Instant::now()), true);
    }

    fn said(o: &Outcome) -> String {
        match o {
            Outcome::Error(e) | Outcome::Text(e) => e.clone(),
            Outcome::Ok => "ok".into(),
            Outcome::Pending => "pending".into(),
            _ => "something else".into(),
        }
    }

    fn run(s: &mut Server, line: &str, cid: Option<ClientId>) -> Outcome {
        s.exec(crate::command::parse_line(line).unwrap().unwrap(), cid)
    }

    fn sent(typed: &mut mpsc::UnboundedReceiver<ToServer>) -> (u64, String) {
        match typed.try_recv().expect("sent").kind {
            Some(to_server::Kind::Command(c)) => (c.id, c.line),
            k => panic!("{k:?}"),
        }
    }

    fn reply(s: &mut Server, id: u64, ok: bool, text: &str, pane: Option<u32>) {
        let r = crate::link_proto::Reply { id, ok, text: text.into(), pane };
        s.mirror_receive(KEY, ToClient { kind: Some(to_client::Kind::Reply(r)) });
    }

    /// What a client not attached is told: its text, then its exit code.
    fn told(rx: &mut mpsc::Receiver<crate::ipc::ServerMsg>) -> (String, i32) {
        let mut text = String::new();
        while let Ok(m) = rx.try_recv() {
            match m {
                crate::ipc::ServerMsg::Text(t) | crate::ipc::ServerMsg::Error(t) => text.push_str(&t),
                crate::ipc::ServerMsg::Done { code } => return (text, code),
                _ => {}
            }
        }
        (text, -1)
    }

    #[test]
    fn what_changes_its_shape_is_sent_there_by_its_ids_and_answered() {
        let (mut s, mut typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "one", 3), (4, "four", 4)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        // A keepane there of 0.62 or before takes none.
        s.mirrors.get_mut(KEY).unwrap().state = State::Live(Instant::now());
        let out = run(&mut s, "split-window -t 10.0.0.2:7681/work", None);
        assert!(matches!(&out, Outcome::Error(e) if e.contains("takes no commands")), "{}", said(&out));
        live(&mut s);
        // Its places by its ids: session 1, window 2, pane 4.
        assert!(matches!(run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", None), Outcome::Ok));
        assert_eq!(sent(&mut typed).1, "kill-pane -t $1:@2.%4");
        assert!(matches!(run(&mut s, "rename-window -t 10.0.0.2:7681/work:0 logs", None), Outcome::Ok));
        assert_eq!(sent(&mut typed).1, "rename-window -t $1:@2 logs");
        assert!(matches!(run(&mut s, "new-window -d -t 10.0.0.2:7681/work", None), Outcome::Ok));
        assert_eq!(sent(&mut typed).1, "new-window -d -t $1");
        assert_eq!(s.mirror_panes(KEY).len(), 2, "nothing done here: it is done there");
        // A key's command (no target): the pane the client looks at.
        let (tx, mut rx) = mpsc::channel(64);
        s.client_connected(9, tx);
        s.clients.get_mut(&9).unwrap().pane_env = Some(s.mirror_pane_mut(KEY, 1).unwrap().id);
        assert!(matches!(run(&mut s, "split-window -h", Some(9)), Outcome::Pending));
        let (id, line) = sent(&mut typed);
        assert_eq!(line, "split-window -h -t $1:@2.%1");
        // The reply comes after the changes: the eye goes to the new pane
        // (9 there), and the client is told.
        let mut next = shape(&[1, 4, 9]);
        next.sessions[0].windows[0].active = 9;
        let change = ToClient {
            kind: Some(to_client::Kind::Entry(Entry { seq: 11, at: 0, change: Some(entry::Change::Shape(next)) })),
        };
        s.mirror_apply(KEY, change).unwrap();
        reply(&mut s, id, true, "", Some(9));
        let p9 = s.mirror_pane_mut(KEY, 9).unwrap().id;
        assert_eq!(s.session(sid).unwrap().windows[0].active, p9);
        assert_eq!(told(&mut rx), (String::new(), 0));
        // Failed there: said here.
        run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", Some(9));
        let (id, _) = sent(&mut typed);
        reply(&mut s, id, false, "can't find pane: %4", None);
        let (text, code) = told(&mut rx);
        assert!(code != 0 && text.contains("can't find pane"), "{text}");
        // No reply in time, or the connection gone: said too.
        run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", Some(9));
        sent(&mut typed);
        for a in s.mirrors.get_mut(KEY).unwrap().asks.values_mut() {
            a.at -= REPLY_WAIT + Duration::from_secs(1);
        }
        s.mirror_asks_due();
        assert!(told(&mut rx).0.contains("no reply from 10.0.0.2:7681"));
        run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", Some(9));
        sent(&mut typed);
        s.mirror_news(1, KEY.into(), MirrorNews::Down("gone".into()));
        assert!(told(&mut rx).0.contains("dropped"));
        assert!(s.mirrors[KEY].asks.is_empty());
        // The copy made again (the machine moved), or stopped: whoever waits
        // for a reply is told, never left waiting.
        live(&mut s);
        run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", Some(9));
        sent(&mut typed);
        let (input, mut again) = mpsc::unbounded_channel();
        s.mirror_put(
            KEY.into(),
            Mirror::new("10.0.0.2:7681".into(), 3, None, Default::default(), Default::default(), input),
        );
        assert!(told(&mut rx).0.contains("was made again"));
        s.mirror_apply(KEY, snapshot(30, &[(1, "one", 3), (4, "four", 4)])).unwrap();
        live(&mut s);
        run(&mut s, "kill-pane -t 10.0.0.2:7681/%4", Some(9));
        sent(&mut again);
        assert_eq!(s.mirror_remove(KEY, "is no longer copied").as_deref(), Some("10.0.0.2:7681"));
        assert!(told(&mut rx).0.contains("no longer copied"));
        assert!(s.mirror_session("10.0.0.2:7681", "work").is_none());
        s.mirrors.insert(
            KEY.into(),
            Mirror::new(
                "10.0.0.2:7681".into(),
                4,
                None,
                Default::default(),
                Default::default(),
                mpsc::unbounded_channel().0,
            ),
        );
        s.mirror_apply(KEY, snapshot(40, &[(1, "one", 3), (4, "four", 4)])).unwrap();
        // What would do something of this machine's to it stays refused;
        // a pane never moves between machines.
        live(&mut s);
        let out = run(&mut s, "pipe-pane -t 10.0.0.2:7681/%1 cat", None);
        assert!(matches!(&out, Outcome::Error(e) if e.contains("only that machine can pipe")), "{}", said(&out));
        let (input, _rx2) = mpsc::unbounded_channel();
        let mut other = Mirror::new("10.0.0.3:7681".into(), 2, None, Default::default(), Default::default(), input);
        (other.state, other.commands) = (State::Live(Instant::now()), true);
        s.mirrors.insert("k2".into(), other);
        s.mirror_apply("k2", snapshot(5, &[(1, "x", 1)])).unwrap();
        let out = run(&mut s, "join-pane -s 10.0.0.3:7681/%1 -t 10.0.0.2:7681/%1", None);
        assert!(matches!(&out, Outcome::Error(e) if e.contains("between machines")), "{}", said(&out));
        assert!(typed.try_recv().is_err(), "nothing sent");
    }

    /// The machine copied runs only what changes a session's shape, about
    /// its own places, each named.
    #[test]
    fn a_machine_sends_only_what_changes_a_shape_of_this_ones_own() {
        let (mut s, _typed) = copying();
        s.mirror_apply(KEY, snapshot(10, &[(1, "x", 1)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let refused = |s: &Server, line: &str| s.command_from_machine(line).err().unwrap_or_default();
        assert!(refused(&s, "run-shell 'rm -rf /'").contains("not a command another machine runs here"));
        assert!(refused(&s, "set -g status off").contains("not a command"));
        assert!(refused(&s, "kill-server").contains("not a command"));
        assert!(refused(&s, "kill-pane").contains("every place is named"));
        assert!(refused(&s, "kill-pane -t 10.0.0.9:7681/%1").contains("on another machine"));
        // Named by its address, even a machine copied here: not this one's.
        assert!(refused(&s, "kill-pane -t 10.0.0.2:7681/%1").contains("names a place on another machine"));
        // A third machine's session copied here is not this machine's.
        assert!(refused(&s, &format!("kill-session -t ${sid}")).contains("copied here"));
    }

    #[test]
    fn a_size_from_the_wire_is_held_to_what_a_pane_can_be() {
        assert_eq!(dim(80), Ok(80));
        assert!(dim(0).is_err() && dim(1001).is_err() && dim(u32::MAX).is_err());
    }
}
