//! A Feishu (Lark) app of one's own as a way to the panes
//! (docs/design/feishu.md). With `feishu-app-id` and `feishu-app-secret`
//! set, keepane keeps the app's long connection (a WebSocket it opens
//! itself: no address of this machine is reachable from outside), and:
//!
//! - the tellings of `done-events` go to the people let in, by the app
//!   (`feishu-notify`); each one remembers its pane, so an answer to it
//!   goes there: to a pane that waits for the person, keys pressed now;
//!   to others, a message, as `send-message` sends one;
//! - a few commands come in: `/panes`, `/to`, `/key`, `/screen`, `/help`;
//! - an agent's answer to a message from Feishu (`send-message -r`) goes
//!   back to the person, under what they sent.
//!
//! Only people let in from outside every pane (`feishu-allow`) are heard,
//! only in a chat with the bot alone. What they may reach is what another
//! machine may (`link-allow`): agents' panes and `normal` ones; keys into a
//! pane with no agent, or messages to a `shell` pane, run commands, and
//! need `--shell`; reading a screen needs `--screen`. Every request is in
//! the event log.
//!
//! The connection and the sending run on tasks of their own; what they
//! hear comes to the server as `Event::Feishu`. HTTP goes through curl,
//! with the secret and the token on its standard input, as `done` does.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::actor::{Message, MsgId, Sender, WorkMode};
use super::link_state::{js, not_from_a_pane};
use super::{ClientId, Event, Outcome, PaneId, Server};
use crate::command::{Cmd, Target};
use crate::feishu::{self, ChatCmd, Frame, Incoming, Typed};

/// The sender of a message that came from Feishu: `feishu/user`.
pub(super) const ADDR: &str = "feishu";

/// Message ids handled lately: an event Feishu sends again is not acted on twice.
const SEEN_KEPT: usize = 1000;
/// Messages sent about a pane, remembered for the answers to them.
const ABOUT_KEPT: usize = 500;
/// Messages from Feishu remembered for an agent's answer to go back.
const ASKED_KEPT: usize = 500;
/// A message older than this when it arrives (sent while keepane was not
/// connected) is not acted on: keys pressed now for a question of then.
const STALE: Duration = Duration::from_secs(600);
/// Someone not let in is told how to be, once in this long.
const STRANGER_QUIET: Duration = Duration::from_secs(600);
/// The lines `/screen` sends.
const SCREEN_LINES: usize = 40;

/// What a message sent through the app is about: its pane, and for a
/// pane that waits for the person, which telling it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct About {
    pub pane: PaneId,
    pub wait: Option<u64>,
}

/// A pane that waits for the person: the telling, and when.
struct Wait {
    done: u64,
    at: chrono::DateTime<chrono::Local>,
}

/// Where an agent's answer to a message from Feishu goes: the person, and
/// the message of theirs it answers.
struct Asked {
    user: String,
    message: String,
}

/// Someone let in, and what more they may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct User {
    pub open_id: String,
    pub shell: bool,
    pub screen: bool,
}

/// The people let in, a line each: `ou_… shell=no screen=no`.
fn parse_users(text: &str) -> Vec<User> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let open_id = w.next().filter(|id| feishu::valid_open_id(id))?.to_string();
            let mut u = User { open_id, shell: false, screen: false };
            for kv in w {
                match kv {
                    "shell=yes" => u.shell = true,
                    "screen=yes" => u.screen = true,
                    _ => {}
                }
            }
            Some(u)
        })
        .collect()
}

fn format_users(users: &[User]) -> String {
    let yes = |b: bool| if b { "yes" } else { "no" };
    users.iter().map(|u| format!("{} shell={} screen={}\n", u.open_id, yes(u.shell), yes(u.screen))).collect()
}

/// Where a message goes: to a person, or under a message of theirs.
#[derive(Clone, Debug)]
pub(super) enum To {
    User(String),
    Reply(String),
}

/// A message for the sending task.
#[derive(Clone, Debug)]
pub(super) struct Outgoing {
    pub to: To,
    pub text: String,
    pub about: Option<About>,
}

/// What the tasks tell the server.
pub(super) enum FromChat {
    /// The state of connection number `.0`, in words; `connected` when it
    /// is up. One replaced since may still say something: not heeded.
    State(u64, String),
    Message(Incoming),
    /// A message went out: its id, and what it was about.
    Sent(String, Option<About>),
    /// A message could not go out, or an event could not be read: why
    /// (said on the status line).
    Failed(String),
    /// A frame that does not read: kept in the log only (it could come
    /// often, and there is nothing for the person to do).
    Bad(String),
}

/// The running connection and sender, with what they were started with.
struct Conn {
    key: (String, String, String),
    /// Its number: what its state is told with.
    generation: u64,
    out: mpsc::UnboundedSender<Outgoing>,
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl Drop for Conn {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[derive(Default)]
pub(super) struct FeishuHost {
    conn: Option<Conn>,
    /// The number the next connection gets.
    generation: u64,
    /// The connection's last word, and since when it is up.
    state: String,
    since: Option<chrono::DateTime<chrono::Local>>,
    /// The people let in; None until first needed (read from the link
    /// directory, `feishu-users`).
    users: Option<Vec<User>>,
    seen: VecDeque<String>,
    seen_set: HashSet<String>,
    about: VecDeque<(String, About)>,
    waits: HashMap<PaneId, Wait>,
    asked: HashMap<MsgId, Asked>,
    strangers: HashMap<String, Instant>,
}

impl FeishuHost {
    /// Whether message `id` is new (and remember it).
    fn first_time(&mut self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }
        if !self.seen_set.insert(id.to_string()) {
            return false;
        }
        self.seen.push_back(id.to_string());
        while self.seen.len() > SEEN_KEPT {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        true
    }

    fn about(&self, id: &str) -> Option<About> {
        if id.is_empty() {
            return None;
        }
        self.about.iter().rev().find(|(m, _)| m == id).map(|(_, a)| *a)
    }

    fn remember(&mut self, id: String, about: About) {
        self.about.push_back((id, about));
        while self.about.len() > ABOUT_KEPT {
            self.about.pop_front();
        }
    }
}

/// curl's standard input for a POST of JSON: the address, the headers, the
/// body; never on its command line, where any process could read them.
fn curl_post_config(url: &str, headers: &[String], body: &str) -> String {
    let q = super::done::curl_quote;
    let mut c = format!("url = {}\nrequest = \"POST\"\n", q(url));
    for h in std::iter::once(&"Content-Type: application/json; charset=utf-8".to_string()).chain(headers) {
        c.push_str(&format!("header = {}\n", q(h)));
    }
    c.push_str(&format!("data-binary = {}\nmax-time = 15\nsilent\nshow-error\n", q(body)));
    c
}

/// A POST through curl, off the async threads: what came back.
async fn post(url: String, headers: Vec<String>, body: String) -> Result<String, String> {
    let config = curl_post_config(&url, &headers, &body);
    tokio::task::spawn_blocking(move || {
        let mut c = crate::platform::process::quiet_command("curl");
        c.args(["-K", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = c.spawn().map_err(|e| format!("curl could not be run: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(config.as_bytes()).map_err(|e| format!("curl: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { format!("curl exited with {}", out.status) } else { err });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .await
    .map_err(|e| format!("curl: {e}"))?
}

/// A refused WebSocket handshake, as Feishu explains it in its headers.
fn handshake_error(e: tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match e {
        Error::Http(r) => {
            let h = |k: &str| r.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let (status, msg) = (h("handshake-status"), h("handshake-msg"));
            if status.is_empty() {
                format!("Feishu refused the connection ({})", r.status())
            } else {
                format!("Feishu refused the connection ({status}): {msg}")
            }
        }
        e => format!("could not connect: {e}"),
    }
}

/// One connection, until it ends: its address asked for, then frames:
/// pings on Feishu's interval, every event answered first and then handed
/// to the server.
async fn connect_once(
    base: &str,
    app_id: &str,
    secret: &str,
    generation: u64,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), String> {
    let tell = |c: FromChat| {
        let _ = events.send(Event::Feishu(Box::new(c)));
    };
    let req = feishu::endpoint_request(base, app_id, secret);
    let body = post(req.url, vec!["locale: zh".into()], req.body).await?;
    let (url, mut cfg) = feishu::parse_endpoint(&body)?;
    let service = feishu::service_id(&url);
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| format!("no connection to Feishu in {}s", CONNECT_TIMEOUT.as_secs()))?
        .map_err(handshake_error)?;
    let (mut tx, mut rx) = ws.split();
    tell(FromChat::State(generation, "connected".into()));
    let mut parts = feishu::Parts::default();
    let mut every = cfg.ping_interval;
    let mut ping = tokio::time::interval(Duration::from_secs(every));
    // When anything last came: Feishu answers each ping, so a connection
    // that went quiet is gone, though nothing said so.
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if silent(heard.elapsed(), every) {
                    return Err(format!("nothing from Feishu for {}s", heard.elapsed().as_secs()));
                }
                let f = Frame::ping(service).encode();
                tx.send(WsMessage::Binary(f.into())).await.map_err(|e| format!("connection lost: {e}"))?;
            }
            m = rx.next() => {
                heard = Instant::now();
                match m {
                    None => return Err("Feishu closed the connection".into()),
                    Some(Err(e)) => return Err(format!("connection lost: {e}")),
                    Some(Ok(WsMessage::Close(_))) => return Err("Feishu closed the connection".into()),
                    Some(Ok(WsMessage::Binary(b))) => {
                        let start = Instant::now();
                        let f = match Frame::decode(&b) {
                            Ok(f) => f,
                            Err(why) => {
                                tell(FromChat::Bad(why));
                                continue;
                            }
                        };
                        match f.method {
                            feishu::METHOD_CONTROL => {
                                // A pong may carry new intervals.
                                if f.header("type") == Some("pong")
                                    && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&f.payload)
                                {
                                    cfg.update(&v);
                                    if cfg.ping_interval != every {
                                        every = cfg.ping_interval;
                                        ping = tokio::time::interval_at(
                                            tokio::time::Instant::now() + Duration::from_secs(every),
                                            Duration::from_secs(every),
                                        );
                                    }
                                }
                            }
                            feishu::METHOD_DATA => {
                                let n = |k: &str| f.header(k).and_then(|v| v.parse::<usize>().ok());
                                let id = f.header("message_id").unwrap_or_default().to_string();
                                let Some(payload) = parts.add(&id, n("sum").unwrap_or(1), n("seq").unwrap_or(0), f.payload.clone())
                                else {
                                    continue;
                                };
                                // Answered first: Feishu sends an event again
                                // unless it hears back within 3 seconds.
                                let ack = f.ack(start.elapsed().as_millis()).encode();
                                tx.send(WsMessage::Binary(ack.into())).await.map_err(|e| format!("connection lost: {e}"))?;
                                if f.header("type") == Some("event") {
                                    match feishu::parse_event(&payload) {
                                        Ok(Some(m)) => tell(FromChat::Message(m)),
                                        Ok(None) => {}
                                        Err(why) => tell(FromChat::Failed(why)),
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    // Pings and pongs of WebSocket itself are answered by it.
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// How long the connection's handshake (TCP, TLS, WebSocket) may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a connection heard from `heard` ago is gone: nothing through
/// two pings and half a minute more.
fn silent(heard: Duration, ping_every: u64) -> bool {
    heard > Duration::from_secs(ping_every.saturating_mul(2).saturating_add(30))
}

/// The connection, kept: made again after it ends, a little later each
/// time it fails in a row, at most two minutes apart (Feishu's own
/// reconnect interval by default).
async fn keep_connected(
    base: String,
    app_id: String,
    secret: String,
    generation: u64,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut fails = 0u32;
    loop {
        let started = Instant::now();
        let why = match connect_once(&base, &app_id, &secret, generation, &events).await {
            Ok(()) => "disconnected".to_string(),
            Err(e) => e,
        };
        // One that held a while was no failure in a row.
        fails = if started.elapsed() > Duration::from_secs(60) { 0 } else { fails + 1 };
        let wait = [2, 5, 15, 30, 60, 120][(fails as usize).min(5)];
        let _ = events
            .send(Event::Feishu(Box::new(FromChat::State(generation, format!("{why}; trying again in {wait}s")))));
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// Feishu's codes for a token that is no good any more.
fn token_refused(why: &str) -> bool {
    ["99991661", "99991663", "99991668"].iter().any(|c| why.contains(c))
}

/// The sending task: one message at a time, with the app's token (asked
/// for when there is none or it is about to end, and again when Feishu
/// says it is no good).
async fn send_loop(
    base: String,
    app_id: String,
    secret: String,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut token: Option<(String, Instant)> = None;
    while let Some(o) = rx.recv().await {
        let mut result = Err(String::new());
        for _ in 0..2 {
            if token.as_ref().is_none_or(|(_, until)| Instant::now() >= *until) {
                let req = feishu::token_request(&base, &app_id, &secret);
                match post(req.url, Vec::new(), req.body).await.and_then(|b| feishu::parse_token(&b)) {
                    Ok((t, secs)) => {
                        token = Some((t, Instant::now() + Duration::from_secs(secs.saturating_sub(300).max(60))));
                    }
                    Err(why) => {
                        result = Err(why);
                        break;
                    }
                }
            }
            let t = token.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
            let req = match &o.to {
                To::User(id) => feishu::send_request(&base, id, &o.text),
                To::Reply(m) => feishu::reply_request(&base, m, &o.text),
            };
            result = post(req.url, vec![format!("Authorization: Bearer {t}")], req.body)
                .await
                .and_then(|b| feishu::parse_sent(&b));
            match &result {
                Err(why) if token_refused(why) => token = None,
                _ => break,
            }
        }
        let said = match result {
            Ok(id) => FromChat::Sent(id, o.about),
            Err(why) => FromChat::Failed(format!("a message could not be sent: {why}")),
        };
        let _ = events.send(Event::Feishu(Box::new(said)));
    }
}

impl Server {
    /// The connection follows the options: started when the app's id and
    /// secret are both set, made again when they or the domain change,
    /// stopped when one is cleared. Nothing without a runtime (tests).
    pub(super) fn feishu_sync(&mut self) {
        let o = &self.opts;
        let want = (!o.feishu_app_id.is_empty() && !o.feishu_app_secret.is_empty())
            .then(|| (feishu::base_url(&o.feishu_domain), o.feishu_app_id.clone(), o.feishu_app_secret.clone()));
        if want.as_ref() == self.feishu.conn.as_ref().map(|c| &c.key) {
            return;
        }
        let was = self.feishu.conn.take().is_some();
        self.feishu.since = None;
        let Some(key) = want else {
            if was {
                self.feishu.state = "off".into();
                self.observe.feishu("stopped", "");
            }
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let (out, rx) = mpsc::unbounded_channel();
        let (base, id, secret) = key.clone();
        self.feishu.generation += 1;
        let generation = self.feishu.generation;
        let tasks = [
            rt.spawn(keep_connected(base.clone(), id.clone(), secret.clone(), generation, self.events.clone())),
            rt.spawn(send_loop(base.clone(), id.clone(), secret, rx, self.events.clone())),
        ];
        self.feishu.conn = Some(Conn { key, generation, out, tasks });
        self.feishu.state = "connecting".into();
        self.observe.feishu("started", &format!(",\"app\":{},\"domain\":{}", js(&id), js(&base)));
    }

    /// What the tasks heard.
    pub(super) fn feishu_event(&mut self, e: FromChat) {
        match e {
            FromChat::State(generation, s) => {
                if self.feishu.conn.as_ref().is_none_or(|c| c.generation != generation) {
                    return;
                }
                let up = s == "connected";
                if up {
                    self.feishu.since = Some(chrono::Local::now());
                    self.note_message("feishu: connected");
                } else {
                    self.feishu.since = None;
                    log::warn!("feishu: {s}");
                }
                self.observe.feishu("state", &format!(",\"state\":{}", js(&s)));
                self.feishu.state = s;
            }
            FromChat::Message(m) => self.feishu_message(m),
            FromChat::Sent(id, about) => {
                if let Some(a) = about {
                    let pane = js(&self.address_of(a.pane));
                    self.observe.feishu("sent", &format!(",\"message\":{},\"pane\":{pane}", js(&id)));
                    self.feishu.remember(id, a);
                }
            }
            FromChat::Bad(why) => {
                log::warn!("feishu: {why}");
                self.observe.feishu("bad-frame", &format!(",\"why\":{}", js(&why)));
            }
            FromChat::Failed(why) => {
                log::warn!("feishu: {why}");
                self.observe.feishu("failed", &format!(",\"why\":{}", js(&why)));
                self.note_message(&format!("feishu: {why}"));
            }
        }
    }

    /// The people let in, read on first use.
    fn feishu_users(&mut self) -> &mut Vec<User> {
        if self.feishu.users.is_none() {
            let path = self.link_dir().join("feishu-users");
            let users = std::fs::read_to_string(path).map(|t| parse_users(&t)).unwrap_or_default();
            self.feishu.users = Some(users);
        }
        self.feishu.users.as_mut().expect("just read")
    }

    fn save_feishu_users(&mut self) -> Result<(), String> {
        let dir = self.link_dir();
        let text = format_users(self.feishu_users());
        std::fs::create_dir_all(&dir).map_err(|e| format!("feishu: make {}: {e}", dir.display()))?;
        crate::link::write_private(&dir.join("feishu-users"), &text).map_err(|e| format!("feishu: {e:#}"))
    }

    /// A message to the sending task, if there is a connection.
    fn feishu_send(&self, to: To, text: String, about: Option<About>) -> bool {
        self.feishu.conn.as_ref().is_some_and(|c| c.out.send(Outgoing { to, text, about }).is_ok())
    }

    /// A pane is done (`done::pane_done`, past its filters): told to the
    /// people let in, when `feishu-notify` is on; a pane that waits for
    /// the person is remembered as waiting.
    pub(super) fn feishu_tell(&mut self, d: &super::done::Done, host: &str) {
        use super::done::Kind;
        if d.kind == Kind::Wait {
            self.feishu.waits.insert(d.pane, Wait { done: d.id, at: d.at });
        }
        if !self.opts.feishu_notify || self.feishu.conn.is_none() {
            return;
        }
        let how = if d.kind == Kind::Wait {
            "Reply to answer it: a key (1, 2, y, n, esc) or text, typed there now."
        } else {
            "Reply to send this pane a message."
        };
        let text = format!("keepane · {host}\n{}\n\n{how}", d.full());
        let about = About { pane: d.pane, wait: (d.kind == Kind::Wait).then_some(d.id) };
        let users: Vec<String> = self.feishu_users().iter().map(|u| u.open_id.clone()).collect();
        for u in users {
            self.feishu_send(To::User(u), text.clone(), Some(about));
        }
    }

    /// The agent's turn is over, or the pane's program: it waits no more.
    pub(super) fn feishu_wait_over(&mut self, pane: PaneId) {
        self.feishu.waits.remove(&pane);
    }

    /// Whether pane `pid` still waits for the person on telling `done`: it
    /// is the latest one, nothing went in since, its agent is not free.
    fn waiting_on(&self, pid: PaneId, done: u64) -> bool {
        let Some(w) = self.feishu.waits.get(&pid).filter(|w| w.done == done) else { return false };
        let Some(p) = self.pane_ref(pid) else { return false };
        let typed_since = p.actor.last_input.is_some_and(|i| i > w.at);
        let free = p.actor.mode == WorkMode::Ai && p.actor.idle();
        !typed_since && !free && p.exit_code.is_none()
    }

    /// An agent runs in the pane: keys go to it, not to a shell. The look
    /// at the panes' programs says so (`agents`), or the pane still waits
    /// on its agent's question and its shell's prompt has not come back
    /// (an agent gone since, with its shell at the prompt, is not one).
    fn has_agent(&self, pid: PaneId) -> bool {
        if self.agents.stats.contains_key(&pid) {
            return true;
        }
        let asking = self.feishu.waits.get(&pid).is_some_and(|w| self.waiting_on(pid, w.done));
        asking && self.pane_ref(pid).is_some_and(|p| !p.actor.shell_waiting())
    }

    /// A message someone sent the bot.
    fn feishu_message(&mut self, m: Incoming) {
        if !self.feishu.first_time(&m.message_id) || m.sender_type != "user" {
            return;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        if m.create_ms > 0 && now_ms - m.create_ms > STALE.as_millis() as i64 {
            self.feishu_log(&m.sender, "stale", &m.text, None);
            return;
        }
        // In a group, anyone there could act on this machine.
        if m.chat_type != "p2p" {
            self.feishu_log(&m.sender, "group", &m.text, None);
            return;
        }
        let Some(user) = self.feishu_users().iter().find(|u| u.open_id == m.sender).cloned() else {
            self.feishu_stranger(&m);
            return;
        };
        if m.message_type != "text" {
            self.feishu_reply(&m, "Only text goes to the panes. /help for the commands.".into(), None);
            return;
        }
        // Nothing (a mention alone): typed as nothing and Enter, it would
        // take whatever a question's first choice is.
        if m.text.trim().is_empty() {
            self.feishu_reply(&m, "Nothing to send. /help for the commands.".into(), None);
            return;
        }
        let about = self.feishu.about(&m.parent_id).or_else(|| self.feishu.about(&m.root_id));
        let (answer, about) = match about {
            Some(a) => self.feishu_answer_to(&user, &m, a),
            None => self.feishu_command(&user, &m),
        };
        self.feishu_reply(&m, answer, about);
    }

    /// Under the message it answers.
    fn feishu_reply(&self, m: &Incoming, text: String, about: Option<About>) {
        self.feishu_send(To::Reply(m.message_id.clone()), text, about);
    }

    /// What came in and what was done, in the event log.
    fn feishu_log(&mut self, user: &str, what: &str, text: &str, pane: Option<PaneId>) {
        let pane = pane.map_or(String::new(), |p| format!(",\"pane\":{}", js(&self.address_of(p))));
        let gist = super::done::gist(text);
        self.observe.feishu(what, &format!(",\"user\":{},\"text\":{}{pane}", js(user), js(&gist)));
    }

    /// Someone not let in: told who they are and what lets them in, once
    /// in a while; and the person at this machine, on the status line.
    fn feishu_stranger(&mut self, m: &Incoming) {
        self.feishu_log(&m.sender, "refused", &m.text, None);
        let now = Instant::now();
        self.feishu.strangers.retain(|_, t| now.duration_since(*t) < STRANGER_QUIET);
        if self.feishu.strangers.contains_key(&m.sender) || self.feishu.strangers.len() >= 100 {
            return;
        }
        self.feishu.strangers.insert(m.sender.clone(), now);
        let id = &m.sender;
        self.note_message(&format!("feishu: {id} is not let in (keepane feishu allow {id})"));
        self.feishu_reply(
            m,
            format!(
                "This bot answers only the people let in on its computer.\nYour open_id: {id}\n\
                 To let you in, run this there, outside keepane's panes:\nkeepane feishu allow {id}"
            ),
            None,
        );
    }

    /// The pane a person names, as this machine knows it.
    fn feishu_pane(&self, name: &str) -> Result<PaneId, String> {
        let t = Target::parse(name);
        if t.remote.is_some() {
            return Err(format!("{name}: a pane of this machine (%3, %name, session:window.pane)"));
        }
        self.resolve(Some(&t), None).map(|(_, _, p)| p)
    }

    /// A pane as the person reads it: its name, else where it is.
    fn feishu_who(&self, pid: PaneId) -> String {
        match self.pane_ref(pid).and_then(|p| p.actor.name.clone()) {
            Some(n) => format!("%{n}"),
            None => format!("%{pid}"),
        }
    }

    /// An answer to something keepane sent about a pane.
    fn feishu_answer_to(&mut self, user: &User, m: &Incoming, a: About) -> (String, Option<About>) {
        if self.pane_ref(a.pane).is_none() {
            self.feishu_log(&user.open_id, "gone", &m.text, None);
            return (format!("%{} is gone.", a.pane), None);
        }
        let Some(done) = a.wait else { return self.feishu_message_to(user, m, a.pane, &m.text) };
        let who = self.feishu_who(a.pane);
        if self.waiting_on(a.pane, done) {
            return self.feishu_keys(user, a.pane, &m.text);
        }
        let newer = self.feishu.waits.get(&a.pane).is_some_and(|w| w.done != done);
        let keys = matches!(feishu::typed(&m.text), Typed::Keys(_));
        if newer && self.feishu.waits.get(&a.pane).is_some_and(|w| self.waiting_on(a.pane, w.done)) {
            self.feishu_log(&user.open_id, "not-pressed", &m.text, Some(a.pane));
            return (format!("{who} waits on a newer question: answer that one. Nothing was pressed."), None);
        }
        if keys {
            self.feishu_log(&user.open_id, "not-pressed", &m.text, Some(a.pane));
            return (format!("{who} no longer waits (answered there?): nothing was pressed."), None);
        }
        // Text for a question that is over: a message, as to any pane.
        let (said, about) = self.feishu_message_to(user, m, a.pane, &m.text);
        (format!("{who} no longer waits; sent as a message instead.\n{said}"), about)
    }

    /// A command, or text with none.
    fn feishu_command(&mut self, user: &User, m: &Incoming) -> (String, Option<About>) {
        let cmd = match feishu::parse_chat(&m.text) {
            Ok(c) => c,
            Err(why) => {
                self.feishu_log(&user.open_id, "bad", &m.text, None);
                return (why, None);
            }
        };
        match cmd {
            ChatCmd::Help => (feishu::HELP.into(), None),
            ChatCmd::Panes => {
                self.feishu_log(&user.open_id, "panes", &m.text, None);
                (self.feishu_panes(), None)
            }
            ChatCmd::Text(_) => (
                "Reply to a notification to answer its pane, or /to <pane> <text>. /help for the commands.".into(),
                None,
            ),
            ChatCmd::To { pane: p, text } => match self.feishu_pane(&p) {
                Ok(pid) => self.feishu_message_to(user, m, pid, &text),
                Err(e) => (e, None),
            },
            ChatCmd::Key { pane: p, keys } => match self.feishu_pane(&p) {
                Ok(pid) => self.feishu_keys(user, pid, &keys),
                Err(e) => (e, None),
            },
            ChatCmd::Screen { pane: p } => match self.feishu_pane(&p) {
                Ok(pid) => self.feishu_screen(user, m, pid),
                Err(e) => (e, None),
            },
        }
    }

    /// `/panes`: each pane, where it is, its name, mode, state and
    /// program; the ones that wait for the person said so.
    fn feishu_panes(&self) -> String {
        let mut lines = Vec::new();
        for s in &self.sessions {
            for w in &s.windows {
                for pid in w.layout.panes() {
                    let Some(p) = self.pane_ref(pid) else { continue };
                    let name = p.actor.name.clone().unwrap_or_else(|| "-".into());
                    let program = p.pid.map(crate::sysinfo::program_of).unwrap_or_default();
                    let waits = self.feishu.waits.get(&pid).is_some_and(|w| self.waiting_on(pid, w.done));
                    lines.push(format!(
                        "%{pid} {} {name} {} {} {program}{}",
                        self.place_text(pid),
                        p.actor.mode.as_str(),
                        p.actor.state(p.exit_code.is_some()),
                        if waits { "  ← waits for you" } else { "" }
                    ));
                }
            }
        }
        if lines.is_empty() { "No panes.".into() } else { lines.join("\n") }
    }

    /// A message for pane `pid`, from the person, as `send-message` sends
    /// one: an agent's pane gets it when its agent is free; a `shell` pane
    /// runs it, which needs `--shell`.
    fn feishu_message_to(&mut self, user: &User, m: &Incoming, pid: PaneId, text: &str) -> (String, Option<About>) {
        let via = self.pane_ref(pid).map_or(WorkMode::Normal, |p| p.actor.mode);
        let who = self.feishu_who(pid);
        if via == WorkMode::Shell && !user.shell {
            self.feishu_log(&user.open_id, "refused-shell", text, Some(pid));
            return (
                format!(
                    "{who} is in shell mode: a message runs there as a command. On that computer: keepane feishu allow {} --shell",
                    user.open_id
                ),
                None,
            );
        }
        self.next_msg += 1;
        let id = self.next_msg;
        let msg = Message {
            id,
            task: id,
            from: Sender::Remote { addr: ADDR.into(), address: "user".into(), name: None, mode: None },
            to: self.address_of(pid),
            via,
            hop: 0,
            re: None,
            text: text.to_string(),
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        if let Err(why) = self.enqueue(pid, msg) {
            self.feishu_log(&user.open_id, "rejected", text, Some(pid));
            return (format!("#{id} rejected: {why}"), None);
        }
        self.feishu.asked.insert(id, Asked { user: user.open_id.clone(), message: m.message_id.clone() });
        while self.feishu.asked.len() > ASKED_KEPT {
            let oldest = *self.feishu.asked.keys().min().expect("not empty");
            self.feishu.asked.remove(&oldest);
        }
        self.feishu_log(&user.open_id, "message", text, Some(pid));
        let mut said = self.stand_noted(id);
        if via == WorkMode::Normal {
            said.push_str(&format!(
                "\n{who} is in normal mode: the message waits for read-message. To have it typed in when its agent is free: keepane set-work-mode -t %{pid} ai"
            ));
        }
        (said, Some(About { pane: pid, wait: None }))
    }

    /// Keys pressed in pane `pid` now (`feishu::typed`): into an agent;
    /// into a pane with none (a shell runs them), only with `--shell`.
    fn feishu_keys(&mut self, user: &User, pid: PaneId, keys: &str) -> (String, Option<About>) {
        let who = self.feishu_who(pid);
        if !self.has_agent(pid) && !user.shell {
            self.feishu_log(&user.open_id, "refused-shell", keys, Some(pid));
            return (
                format!(
                    "No agent runs in {who}: keys there could run commands. On that computer: keepane feishu allow {} --shell",
                    user.open_id
                ),
                None,
            );
        }
        if keys.len() > self.opts.message_max_size {
            return (
                format!("That is {} bytes; message-max-size is {}.", keys.len(), self.opts.message_max_size),
                None,
            );
        }
        let typed = feishu::typed(keys);
        let Some(p) = self.find_pane_mut(pid) else { return (format!("{who} is gone."), None) };
        if p.exit_code.is_some() {
            return (format!("{who} has exited."), None);
        }
        let said = match &typed {
            Typed::Keys(ks) => {
                let app = p.screen().application_cursor();
                let bytes: Vec<u8> = ks.iter().flat_map(|k| super::input::encode_send_key(k, app)).collect();
                p.type_input(&bytes);
                format!("Pressed {} in {who}.", ks.join(" "))
            }
            Typed::Line(line) => {
                let bytes = super::input::encode_paste(line, p.screen().bracketed_paste());
                p.type_input(&bytes);
                // Enter once the paste has settled, as for a message to an agent.
                p.enter = Some(super::pane::Enter { typed: Instant::now(), sent: None, again: false });
                self.enter_soon(pid, super::pane::Enter::AFTER);
                format!("Typed into {who}, then Enter.")
            }
        };
        self.feishu.waits.remove(&pid);
        self.feishu_log(&user.open_id, "keys", keys, Some(pid));
        (said, Some(About { pane: pid, wait: None }))
    }

    /// `/screen`: what the pane shows, its last lines, for someone allowed
    /// to read the panes (`--screen`): it leaves this machine for Feishu's.
    fn feishu_screen(&mut self, user: &User, m: &Incoming, pid: PaneId) -> (String, Option<About>) {
        let who = self.feishu_who(pid);
        if !user.screen {
            self.feishu_log(&user.open_id, "refused-screen", &m.text, Some(pid));
            return (
                format!(
                    "Reading the panes is off: what a pane shows would go to Feishu's servers. On that computer: keepane feishu allow {} --screen",
                    user.open_id
                ),
                None,
            );
        }
        let shown =
            match crate::command::parse(&["capture-pane".to_string(), "-p".into(), "-t".into(), format!("%{pid}")]) {
                Ok(cmd) => match self.exec(cmd, None) {
                    Outcome::Text(t) => t,
                    Outcome::Error(e) => return (e, None),
                    _ => String::new(),
                },
                Err(e) => return (e, None),
            };
        let lines: Vec<&str> = shown.lines().map(str::trim_end).collect();
        let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
        let start = end.saturating_sub(SCREEN_LINES);
        self.feishu_log(&user.open_id, "screen", &m.text, Some(pid));
        (format!("{who}:\n{}", lines[start..end].join("\n")), Some(About { pane: pid, wait: None }))
    }

    /// An agent's answer (`send-message -r`) to a message that came from
    /// Feishu: back to the person, under what they sent.
    pub(super) fn feishu_answer_message(&mut self, cid: Option<ClientId>, to: MsgId, text: String) -> Outcome {
        let Some(asked) = self.feishu.asked.get(&to) else {
            return Outcome::Error(format!(
                "send-message -r: #{to} came from Feishu, and the way back is gone (the server restarted since)"
            ));
        };
        let (user, message) = (asked.user.clone(), asked.message.clone());
        if !self.feishu_users().iter().any(|u| u.open_id == user) {
            return Outcome::Error(format!(
                "send-message -r: #{to} came from {user} on Feishu, who is no longer let in"
            ));
        }
        let me = self.caller_pane(cid);
        let who = me.map_or_else(|| "keepane".to_string(), |p| self.feishu_who(p));
        let about = me.map(|pane| About { pane, wait: None });
        if !self.feishu_send(To::Reply(message), format!("{who}: {text}"), about) {
            return Outcome::Error("send-message -r: the Feishu app is not connected (keepane feishu)".into());
        }
        let by = self.by(cid);
        self.observe.feishu(
            "answer",
            &format!(
                ",\"user\":{},\"re\":{to},\"by\":{},\"text\":{}",
                js(&user),
                js(&by),
                js(&super::done::gist(&text))
            ),
        );
        Outcome::Text(format!("sent to Feishu (the answer to #{to})"))
    }

    /// `feishu-status`, `feishu-allow`, `feishu-remove`.
    pub(super) fn exec_feishu(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Outcome {
        match cmd {
            Cmd::FeishuStatus => Outcome::Text(self.feishu_status()),
            Cmd::FeishuAllow { user, shell, screen } => self.feishu_allow(cid, user, shell, screen),
            Cmd::FeishuRemove { user } => {
                let by = self.by(cid);
                let users = self.feishu_users();
                let before = users.len();
                users.retain(|u| u.open_id != user);
                if users.len() == before {
                    return Outcome::Error(format!("feishu-remove: {user} is not let in (keepane feishu)"));
                }
                if let Err(e) = self.save_feishu_users() {
                    return Outcome::Error(e);
                }
                self.observe.feishu("removed", &format!(",\"user\":{},\"by\":{}", js(&user), js(&by)));
                Outcome::Text(format!("{user} is no longer let in"))
            }
            other => Outcome::Error(format!("not a feishu command: {other}")),
        }
    }

    fn feishu_status(&mut self) -> String {
        let o = &self.opts;
        let mut out = Vec::new();
        if self.feishu.conn.is_none() {
            out.push(
                "feishu: off (set -g feishu-app-id and feishu-app-secret in keepane.conf; docs/design/feishu.md)"
                    .to_string(),
            );
        } else {
            let at = feishu::base_url(&o.feishu_domain);
            let state = match self.feishu.since {
                Some(t) => format!("connected since {}", t.format("%Y-%m-%d %H:%M:%S")),
                None => self.feishu.state.clone(),
            };
            out.push(format!("feishu: {state} ({at}, app {})", o.feishu_app_id));
            let notify = if o.feishu_notify { "on" } else { "off" };
            out.push(format!("notify: {notify} (done-events: {})", o.done_events));
        }
        let users = self.feishu_users().clone();
        if users.is_empty() {
            out.push("nobody let in: send the bot a message, and it answers with the command that lets you in".into());
        } else {
            out.push("let in:".into());
            let yes = |b: bool| if b { "yes" } else { "no" };
            for u in users {
                out.push(format!("  {}  shell: {}  screen: {}", u.open_id, yes(u.shell), yes(u.screen)));
            }
        }
        out.join("\n")
    }

    /// A Feishu account let in, and what more it may do. Only a person
    /// outside every pane decides: otherwise a program in a pane could
    /// let anyone at it.
    fn feishu_allow(
        &mut self,
        cid: Option<ClientId>,
        user: String,
        shell: Option<bool>,
        screen: Option<bool>,
    ) -> Outcome {
        if self.caller_pane(cid).is_some() {
            return Outcome::Error(not_from_a_pane("feishu-allow"));
        }
        let by = self.by(cid);
        let users = self.feishu_users();
        let u = match users.iter().position(|u| u.open_id == user) {
            Some(i) => &mut users[i],
            None => {
                users.push(User { open_id: user.clone(), shell: false, screen: false });
                users.last_mut().expect("just pushed")
            }
        };
        if let Some(s) = shell {
            u.shell = s;
        }
        if let Some(s) = screen {
            u.screen = s;
        }
        let now = u.clone();
        if let Err(e) = self.save_feishu_users() {
            return Outcome::Error(e);
        }
        let may = |b: bool| if b { "may" } else { "may not" };
        let text = format!(
            "{user} is let in: messages to agents' panes and normal ones, keys to agents; it {} run commands (shell panes, keys where no agent runs); it {} read what the panes show",
            may(now.shell),
            may(now.screen)
        );
        self.observe.feishu(
            "allowed",
            &format!(",\"user\":{},\"shell\":{},\"screen\":{},\"by\":{}", js(&user), now.shell, now.screen, js(&by)),
        );
        self.note_message(&format!("feishu: {user} let in (by {by})"));
        Outcome::Text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table of people reads back as written; lines that are not one
    /// are passed over.
    #[test]
    fn the_people_let_in_read_back() {
        let users = vec![
            User { open_id: "ou_a1".into(), shell: true, screen: false },
            User { open_id: "ou_b2".into(), shell: false, screen: true },
        ];
        let text = format_users(&users);
        assert_eq!(text, "ou_a1 shell=yes screen=no\nou_b2 shell=no screen=yes\n");
        assert_eq!(parse_users(&text), users);
        assert_eq!(parse_users("# a note\nnot-an-id shell=yes\nou_c3\n").len(), 1);
    }

    /// curl gets the address, both headers and the body on its standard
    /// input, quoted; the secret is nowhere else.
    #[test]
    fn the_post_goes_on_curls_input() {
        let c = curl_post_config("https://x/a?b=1", &["Authorization: Bearer t-1".into()], "{\"s\":\"q\\\"\"}");
        assert!(c.starts_with("url = \"https://x/a?b=1\"\nrequest = \"POST\"\n"), "{c}");
        assert!(c.contains("header = \"Content-Type: application/json; charset=utf-8\"\n"));
        assert!(c.contains("header = \"Authorization: Bearer t-1\"\n"));
        assert!(c.contains("data-binary = \"{\\\"s\\\":\\\"q\\\\\\\"\\\"}\"\n"), "{c}");
        assert!(!c.contains("\nfail\n"), "Feishu's refusals come as JSON with a 4xx");
    }

    /// Each message id is acted on once; the oldest are forgotten past
    /// the limit; what was sent about a pane is found by its id, the
    /// newest first.
    #[test]
    fn seen_and_about() {
        let mut h = FeishuHost::default();
        assert!(h.first_time("om_1"));
        assert!(!h.first_time("om_1"));
        for i in 0..SEEN_KEPT {
            h.first_time(&format!("x{i}"));
        }
        assert!(h.first_time("om_1"), "forgotten past the limit");
        assert_eq!(h.seen.len(), SEEN_KEPT);
        h.remember("om_9".into(), About { pane: 3, wait: Some(7) });
        h.remember("om_9".into(), About { pane: 4, wait: None });
        assert_eq!(h.about("om_9"), Some(About { pane: 4, wait: None }));
        assert_eq!(h.about(""), None);
        for i in 0..ABOUT_KEPT {
            h.remember(format!("m{i}"), About { pane: 1, wait: None });
        }
        assert_eq!(h.about("om_9"), None);
    }

    /// Against the real Feishu, by hand (`cargo test --lib real_feishu --
    /// --ignored`; it needs the network): a made-up app is refused at the
    /// endpoint, in Feishu's words; the connection's TLS (rustls, ring,
    /// Mozilla's roots) reaches Feishu's WebSocket host, which refuses a
    /// handshake that has no ticket.
    #[tokio::test]
    #[ignore]
    async fn real_feishu_refuses_a_made_up_app() {
        let req = feishu::endpoint_request(feishu::FEISHU, "cli_keepane_made_up", "x");
        let body = post(req.url, vec!["locale: zh".into()], req.body).await.unwrap();
        let err = feishu::parse_endpoint(&body).unwrap_err();
        assert!(err.starts_with("Feishu refused ("), "{err}: {body}");
        let e = tokio_tungstenite::connect_async("wss://msg-frontier.feishu.cn/ws/v2?service_id=1").await.unwrap_err();
        let why = handshake_error(e);
        assert!(why.starts_with("Feishu refused the connection"), "{why}");
        println!("endpoint: {err}\nhandshake: {why}");
    }

    /// What a connection replaced since says of itself is not heeded: the
    /// status is the current one's. Its options cleared, there is none.
    #[tokio::test]
    async fn only_the_current_connection_says_how_it_stands() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "feishu-generation".into(), events);
        // Nothing listens there: the tasks keep trying, and say nothing here.
        s.opts.feishu_domain = "http://127.0.0.1:9".into();
        s.opts.feishu_app_id = "cli_a".into();
        s.opts.feishu_app_secret = "x".into();
        s.feishu_sync();
        s.opts.feishu_app_id = "cli_b".into();
        s.feishu_sync();
        assert_eq!(s.feishu.conn.as_ref().map(|c| c.generation), Some(2), "made again for the new id");
        s.feishu_event(FromChat::State(1, "connected".into()));
        assert!(s.feishu.since.is_none() && s.feishu.state == "connecting", "the old one's word");
        s.feishu_event(FromChat::State(2, "connected".into()));
        assert!(s.feishu.since.is_some());
        s.opts.feishu_app_secret.clear();
        s.feishu_sync();
        assert!(s.feishu.conn.is_none() && s.feishu.state == "off");
        s.feishu_event(FromChat::State(2, "connected".into()));
        assert!(s.feishu.since.is_none(), "none now");
    }

    /// A connection is taken as gone after two pings and half a minute
    /// with nothing from Feishu, not before.
    #[test]
    fn a_quiet_connection_is_gone() {
        assert!(!silent(Duration::from_secs(270), 120));
        assert!(silent(Duration::from_secs(271), 120));
        assert!(!silent(Duration::from_secs(0), 0));
        assert!(silent(Duration::from_secs(31), 0));
    }

    #[test]
    fn a_stale_token_is_told_apart() {
        assert!(token_refused("Feishu refused (99991663): Invalid access token for authorization"));
        assert!(!token_refused("Feishu refused (230002): bot is not in the chat"));
    }
}
