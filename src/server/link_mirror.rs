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
            return match self.mirrors.remove(&key) {
                Some(m) => {
                    self.mirror_forget(&key, &format!("{} is no longer copied", m.addr));
                    Outcome::Text(format!("link: stopped copying {}\n", m.addr))
                }
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
            if let Some(m) = self.mirrors.remove(&key) {
                self.mirror_forget(&key, &format!("{} is no longer paired", m.addr));
            }
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
        self.mirrors.insert(key, m);
        Ok(addr)
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
            None | Some(to_client::Kind::Ping(_)) => Ok(()),
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
        // What changes its shape is that machine's to do.
        let refused =
            |s: &mut Server, line: &str| match s.exec(crate::command::parse_line(line).unwrap().unwrap(), None) {
                Outcome::Error(e) => e,
                _ => String::new(),
            };
        for line in [
            "split-window -t 10.0.0.2:7681/work",
            "kill-session -t 10.0.0.2:7681/work",
            "kill-pane -t 10.0.0.2:7681/%1",
        ] {
            assert!(refused(&mut s, line).contains("only that machine can"), "{line}");
        }
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
    fn a_border_between_panes_of_another_machine_is_not_dragged() {
        let (mut s, _typed) = copying();
        s.opts.mouse = true;
        s.mirror_apply(KEY, snapshot(10, &[(1, "a", 1), (4, "b", 1)])).unwrap();
        let sid = s.mirror_session("10.0.0.2:7681", "work").unwrap();
        let (tx, _rx) = mpsc::channel(64);
        s.client_connected(7, tx);
        let c = s.clients.get_mut(&7).unwrap();
        (c.cols, c.rows, c.interactive, c.session) = (80, 6, true, Some(sid));
        // The column between the two panes (0-19 and 21-40).
        s.handle_mouse(7, crate::ipc::MouseRecord { x: 20, y: 1, buttons: 1, ctrl: 0, flags: 0 });
        assert!(s.clients[&7].drag.is_none());
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

    #[test]
    fn a_size_from_the_wire_is_held_to_what_a_pane_can_be() {
        assert_eq!(dim(80), Ok(80));
        assert!(dim(0).is_err() && dim(1001).is_err() && dim(u32::MAX).is_err());
    }
}
