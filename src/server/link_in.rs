//! A request from another machine (docs/design/link.md), as `keepane web`
//! forwards it whole (`link-inbound`): pairing, a message for a pane here,
//! the pane list, unpairing. Everything is checked here, since the keys
//! are the server's: the pairing's proof (the web key), or the signature
//! of a machine in the table, the clocks, and that the request is new.
//! The answer is JSON for the web to send back: status, this server's
//! signature of it, and the body.

use super::actor::{Message, Sender, WorkMode};
use super::link_state::{Origin, js};
use super::observe::Stage;
use super::{ClientId, Outcome, Server};
use crate::command::{Cmd, Target};
use crate::link::{self, SendAnswer, SendBody};

/// What `keepane web` forwards.
#[derive(serde::Deserialize)]
struct Inbound {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Inbound {
    fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).unwrap_or_default()
    }
}

/// The sender's fields as keepane writes them: `from` is `user` or a full
/// address (`$1:@3.%7`), `name` a pane name, `mode` a work mode.
fn sender_fields_ok(body: &SendBody) -> Result<(), String> {
    let full_address = |s: &str| {
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        let Some(rest) = s.strip_prefix('$') else { return false };
        let Some((sid, rest)) = rest.split_once(":@") else { return false };
        let Some((wid, pid)) = rest.split_once(".%") else { return false };
        digits(sid) && digits(wid) && digits(pid)
    };
    if body.from != "user" && !full_address(&body.from) {
        return Err(format!("from: '{}' is not a pane's full address or `user`", body.from));
    }
    if let Some(n) = &body.name {
        super::actor::check_name(n).map_err(|e| format!("name: {e}"))?;
    }
    if let Some(m) = &body.mode
        && WorkMode::parse(m).is_none()
    {
        return Err(format!("mode: '{m}' is not a work mode"));
    }
    Ok(())
}

/// The pane list as another machine sees it.
const PANES: &str =
    "#{pane_address}\t#{pane_name}\t#{pane_work_mode}\t#{?pane_idle,idle,busy}\t#{pane_inbox}\t#{pane_current_command}";

/// The session another machine's panes go in, from its host name: what
/// keepane takes in a name (letters, digits, `-`, `_`; others become `-`),
/// `remote` when nothing of it is left.
pub(super) fn session_for(host: &str) -> String {
    let s: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let s = s.trim_matches('-');
    if s.is_empty() { "remote".into() } else { s.to_string() }
}
impl Server {
    /// The JSON answer: signed by this server for the request's nonce.
    fn link_answer(&mut self, nonce: &str, status: u16, body: String) -> Outcome {
        let sign = match self.link() {
            Ok(l) => l.id.sign(&link::answer_text(nonce, status, body.as_bytes())),
            Err(_) => String::new(),
        };
        Outcome::Text(format!("{{\"status\":{status},\"sign\":{},\"body\":{}}}", js(&sign), js(&body)))
    }

    /// What a message another machine sent here has come to, for it.
    pub(super) fn link_send_answer(&mut self, id: super::actor::MsgId, nonce: &str) -> Outcome {
        let Some(r) = self.observe.get(id) else {
            return self.link_answer(nonce, 500, format!("#{id} is not on record"));
        };
        let (to, via) = (r.msg.to.clone(), r.msg.via.as_str().to_string());
        let stand = self.stand(id);
        let stage = self.observe.get(id).map(|r| r.stage.as_str().to_string()).unwrap_or_default();
        let body = serde_json::to_string(&SendAnswer { id, to, via, stand, stage }).expect("an answer serializes");
        self.link_answer(nonce, 200, body)
    }

    /// `link-inbound peer-ip request`.
    pub(super) fn link_inbound(&mut self, cid: Option<ClientId>, peer: &str, request: &str) -> Outcome {
        let Ok(req) = serde_json::from_str::<Inbound>(request) else {
            return Outcome::Error("link-inbound: the request is not what keepane web forwards".into());
        };
        let nonce = req.header(link::H_NONCE).to_string();
        let Ok(peer_ip) = peer.parse::<std::net::IpAddr>() else {
            return self.link_answer(&nonce, 400, format!("{peer} is not an address"));
        };
        // Read-only: this machine takes nothing from the network.
        match &self.web {
            None => return self.link_answer(&nonce, 503, "keepane web is off on this machine".into()),
            Some(w) if w.read_only() => {
                return self.link_answer(
                    &nonce,
                    403,
                    "read-only: this machine takes no messages from other machines".into(),
                );
            }
            Some(_) => {}
        }
        if req.header(link::H_PROTOCOL) != link::PROTOCOL.to_string() {
            return self.link_answer(
                &nonce,
                400,
                format!(
                    "keepane link protocol {} (keepane {}) at your end; {} (keepane {}) here: update the older one",
                    req.header(link::H_PROTOCOL),
                    req.header(link::H_VERSION),
                    link::PROTOCOL,
                    env!("CARGO_PKG_VERSION")
                ),
            );
        }
        let now = link::now();
        let time: i64 = req.header(link::H_TIME).parse().unwrap_or(0);
        if let Some(why) = link::check_time(time, now) {
            return self.link_answer(&nonce, 401, why);
        }
        let from = req.header(link::H_FROM).to_string();
        let port: u16 = req.header(link::H_PORT).parse().unwrap_or(0);
        if port == 0 || !link::valid_public(&from) {
            return self.link_answer(&nonce, 400, "the request names no key or no port".into());
        }
        let addr = link::hostport(peer_ip, port);
        if req.path == "/link/pair" {
            return self.link_pair_in(&req, &nonce, now, time, &from, port, &addr);
        }
        // Signed by a machine in the table, for this machine, for this request.
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        let mine = l.id.public();
        let Some(peer) = l.peer_by_key(&from).cloned() else {
            let why = format!(
                "{addr} is not paired with this machine: here, `keepane link add http://{addr}/#k=...` (its web address), or `keepane link trust`"
            );
            self.link_refused(&addr, &why);
            return self.link_answer(&nonce, 401, why);
        };
        let text = link::request_text(&req.method, &req.path, &from, port, &mine, time, &nonce, req.body.as_bytes());
        if !link::verify(&from, &text, req.header(link::H_SIGN)) {
            let why = format!("the request from {addr} is not signed by the key paired with it");
            self.link_refused(&addr, &why);
            return self.link_answer(&nonce, 401, why);
        }
        if !l.nonces.fresh(&nonce, now) {
            let why = format!("the request from {addr} was seen before");
            self.link_refused(&addr, &why);
            return self.link_answer(&nonce, 401, why);
        }
        l.seen.insert(from.clone(), chrono::Local::now());
        // The key says who it is; the address is where it is now.
        if peer.addr != addr {
            if let Some(p) = l.peers.iter_mut().find(|p| p.key == from) {
                p.addr = addr.clone();
            }
            let was = peer.addr.clone();
            if let Err(e) = self.save_peers() {
                log::warn!("link: {e}");
            }
            self.link_note(
                &format!("link: {was} is now at {addr}"),
                "moved",
                &format!(",\"from\":{},\"to\":{},\"key\":{}", js(&was), js(&addr), js(&from)),
            );
        }
        let peer = link::Peer { addr: addr.clone(), ..peer };
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/link/send") => self.link_send_in(cid, &req, &nonce, &peer),
            ("GET", "/link/info") => {
                let text = self.machine_info();
                self.link_answer(&nonce, 200, text)
            }
            ("POST", "/link/capture") => self.link_capture_in(&req, &nonce, &peer),
            ("POST", "/link/pane") => self.link_pane_in(&req, &nonce, &peer),
            ("POST", "/link/kill") => self.link_kill_in(&req, &nonce, &peer),
            ("POST", "/link/trace") => self.link_trace_in(cid, &req, &nonce, &peer),
            ("GET", "/link/panes") => {
                let cmd = Cmd::ListPanes { target: None, all: true, session: false, format: Some(PANES.into()) };
                match self.exec(cmd, None) {
                    Outcome::Text(t) => self.link_answer(&nonce, 200, t),
                    Outcome::Error(e) if e.contains("no sessions") => self.link_answer(&nonce, 200, String::new()),
                    Outcome::Error(e) => self.link_answer(&nonce, 500, e),
                    _ => self.link_answer(&nonce, 200, String::new()),
                }
            }
            ("POST", "/link/remove") => {
                if let Ok(l) = self.link() {
                    l.peers.retain(|p| p.key != from);
                }
                if let Err(e) = self.save_peers() {
                    return self.link_answer(&nonce, 500, e);
                }
                self.link_note(
                    &format!("link: {addr} unpaired (it asked)"),
                    "removed",
                    &format!(",\"addr\":{},\"key\":{},\"by\":\"peer\"", js(&addr), js(&from)),
                );
                self.link_answer(&nonce, 200, "removed".into())
            }
            _ => self.link_answer(&nonce, 404, "not a keepane link path".into()),
        }
    }

    /// A refused request is said on the status line and kept in the event
    /// log: something on the network is trying, and should not go unnoticed.
    /// Once a minute per address at most (`LinkHost::say_refused`).
    fn link_refused(&mut self, addr: &str, why: &str) {
        if !self.link().is_ok_and(|l| l.say_refused(addr)) {
            return;
        }
        self.link_note(
            &format!("link: {addr} refused: {why}"),
            "refused",
            &format!(",\"addr\":{},\"why\":{}", js(addr), js(why)),
        );
    }

    /// `POST /link/pair`: a machine with our web key asks to be let in, and
    /// gets our key back, proved the same way.
    #[allow(clippy::too_many_arguments)]
    fn link_pair_in(
        &mut self,
        req: &Inbound,
        nonce: &str,
        now: i64,
        time: i64,
        from: &str,
        port: u16,
        addr: &str,
    ) -> Outcome {
        let webkey = match self.own_web() {
            Ok((_, k)) => k,
            Err(e) => return self.link_answer(nonce, 503, e),
        };
        let want = link::hmac(webkey.as_bytes(), link::pair_text(from, port, time, nonce).as_bytes());
        let proof = link::unb64(req.header(link::H_PROOF)).is_some_and(|p| link::same(&p, &want));
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        if !proof {
            let why = format!("{addr} asked to pair without this machine's web key");
            self.link_refused(addr, &why);
            return self.link_answer(nonce, 401, why);
        }
        if !l.nonces.fresh(nonce, now) {
            return self.link_answer(nonce, 401, "seen before".into());
        }
        if l.id.public() == from {
            return self.link_answer(nonce, 400, "that is this server itself".into());
        }
        let mine = l.id.public();
        let mut shell = false;
        match l.peers.iter_mut().find(|p| p.key == from) {
            Some(p) => {
                p.addr = addr.to_string();
                shell = p.shell;
            }
            None => l.peers.push(link::Peer {
                key: from.to_string(),
                addr: addr.to_string(),
                shell: false,
                screen: false,
                panes: false,
            }),
        }
        l.seen.insert(from.to_string(), chrono::Local::now());
        if let Err(e) = self.save_peers() {
            return self.link_answer(nonce, 500, e);
        }
        let fp = link::fingerprint(from);
        self.link_note(
            &format!(
                "link: {addr} ({fp}) paired with this machine; `keepane link allow {addr} --shell` lets it run commands"
            ),
            "paired",
            &format!(",\"addr\":{},\"key\":{},\"shell\":{shell},\"how\":\"asked\"", js(addr), js(from)),
        );
        let answer_proof = link::b64(&link::hmac(webkey.as_bytes(), link::pair_answer_text(nonce, &mine).as_bytes()));
        self.link_answer(nonce, 200, format!("{{\"key\":{},\"proof\":{}}}", js(&mine), js(&answer_proof)))
    }

    /// This machine, for another one (`link-info`): what the status line
    /// knows of it, and what runs here.
    fn machine_info(&self) -> String {
        let sys = crate::sysinfo::system();
        let panes: usize = self.sessions.iter().flat_map(|s| s.windows.iter()).map(|w| w.panes.len()).sum();
        let mut rows = vec![
            ("host", crate::sysinfo::hostname()),
            ("system", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
            ("keepane", env!("CARGO_PKG_VERSION").to_string()),
            ("up", crate::format::human_duration(sys.uptime)),
            ("cpu", sys.cpu_percentage.clone()),
            ("memory", format!("{} ({})", sys.ram_percentage, sys.ram_used)),
        ];
        if !sys.battery_percentage.is_empty() {
            let charging = if sys.battery_charging { ", charging" } else { "" };
            rows.push(("battery", format!("{}{charging}", sys.battery_percentage)));
        }
        rows.push(("panes", format!("{panes} in {} sessions", self.sessions.len())));
        rows.push(("time", chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string()));
        rows.iter().map(|(k, v)| format!("{k:<8} {v}")).collect::<Vec<_>>().join("\n")
    }

    /// `POST /link/capture`: what a pane here shows, for a machine allowed
    /// to read the panes (`link-allow --screen`).
    fn link_capture_in(&mut self, req: &Inbound, nonce: &str, peer: &link::Peer) -> Outcome {
        if !peer.screen {
            let why = format!(
                "{} may not read the panes here: on this machine, keepane link allow {} --screen",
                peer.addr, peer.addr
            );
            self.link_refused(&peer.addr, &why);
            return self.link_answer(nonce, 403, why);
        }
        let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        let Some(to) = v["to"].as_str() else {
            return self.link_answer(nonce, 400, "to: the pane".into());
        };
        let t = Target::parse(to);
        if t.remote.is_some() {
            return self.link_answer(nonce, 400, format!("{to}: a pane is named as this machine knows it"));
        }
        let pid = match self.resolve(Some(&t), None) {
            Ok((_, _, p)) => p,
            Err(e) => return self.link_answer(nonce, 404, e),
        };
        let history = v["history"].as_u64().unwrap_or(0).min(2000);
        let mut argv = vec!["capture-pane".to_string(), "-p".into(), "-t".into(), format!("%{pid}")];
        if history > 0 {
            argv.extend(["-S".into(), format!("-{history}")]);
        }
        let cmd = match crate::command::parse(&argv) {
            Ok(c) => c,
            Err(e) => return self.link_answer(nonce, 500, e),
        };
        match self.exec(cmd, None) {
            Outcome::Text(t) => self.link_answer(nonce, 200, t),
            Outcome::Error(e) => self.link_answer(nonce, 404, e),
            _ => self.link_answer(nonce, 200, String::new()),
        }
    }

    /// Why a machine not allowed to start panes here is refused (the helper
    /// `session_for`, below, names where its panes go).
    fn no_panes(&mut self, nonce: &str, peer: &link::Peer) -> Outcome {
        let why = format!(
            "{} may not start panes here: on this machine, keepane link allow {} --panes",
            peer.addr, peer.addr
        );
        self.link_refused(&peer.addr, &why);
        self.link_answer(nonce, 403, why)
    }

    /// `POST /link/pane`: a pane started here for a machine allowed to
    /// (`link-allow --panes`): one of this machine's `agent-commands`,
    /// within its `agent-pane-limit` (counted for that machine alone), in
    /// the session it asks for or one named after it. Its address back.
    fn link_pane_in(&mut self, req: &Inbound, nonce: &str, peer: &link::Peer) -> Outcome {
        if !peer.panes {
            return self.no_panes(nonce, peer);
        }
        let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        let argv: Vec<String> = v["argv"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| w.as_str().map(String::from)).collect())
            .unwrap_or_default();
        // The program: the one asked for, else what a new pane runs here.
        let program = argv.first().cloned().unwrap_or_else(|| {
            self.opts.default_command.first().cloned().unwrap_or_else(|| self.opts.default_shell.clone())
        });
        let stem = super::done::program_name(&program).to_ascii_lowercase();
        let allowed: Vec<String> = self.opts.agent_commands.split_whitespace().map(str::to_ascii_lowercase).collect();
        if !allowed.contains(&stem) {
            let why = format!("'{stem}' is not in agent-commands here ({})", self.opts.agent_commands);
            return self.link_answer(nonce, 403, why);
        }
        if let Some(m) = text("mode")
            && super::actor::WorkMode::parse(&m).is_none()
        {
            return self.link_answer(nonce, 400, format!("mode: normal, shell or ai, not '{m}'"));
        }
        // Its budget: the panes it started that are still here.
        let limit = self.opts.agent_pane_limit as usize;
        let alive: Vec<super::PaneId> = self.all_panes().into_iter().collect();
        let made = match self.link() {
            Ok(l) => {
                l.made.retain(|p, _| alive.contains(p));
                l.made.values().filter(|k| **k == peer.key).count()
            }
            Err(e) => return self.link_answer(nonce, 500, e),
        };
        if made >= limit {
            let why = format!("agent-pane-limit {limit} reached here ({} started {made})", peer.addr);
            return self.link_answer(nonce, 403, why);
        }
        // Where: the session it asks for, else one named after it (a name
        // keepane takes: letters, digits, `-` and `_`).
        let session = text("session").unwrap_or_else(|| session_for(v["host"].as_str().unwrap_or_default()));
        let exists = self.sessions.iter().any(|s| s.name == session);
        let c = crate::command::CreatePane {
            kind: if exists { "window" } else { "session" }.into(),
            target: exists.then(|| Target::parse(&session)),
            session_name: (!exists).then(|| session.clone()),
            horizontal: false,
            cwd: text("cwd"),
            name: text("name"),
            mode: text("mode"),
            message: None,
            argv,
        };
        let made_text = match self.create_pane(None, c) {
            Outcome::Text(t) => t,
            Outcome::Error(e) => return self.link_answer(nonce, 400, e),
            _ => return self.link_answer(nonce, 500, "no pane".into()),
        };
        // `$1:@3.%7  laptop:0.0  name  mode`: the pane is the `%` number.
        let first = made_text.lines().next().unwrap_or_default().to_string();
        let pid = first
            .split_whitespace()
            .next()
            .and_then(|a| a.rsplit_once('%'))
            .and_then(|(_, n)| n.parse::<super::PaneId>().ok());
        let Some(pid) = pid else { return self.link_answer(nonce, 500, format!("no pane in '{first}'")) };
        if let Ok(l) = self.link() {
            l.made.insert(pid, peer.key.clone());
        }
        let address = self.address_of(pid);
        self.link_note(
            &format!("link: {} started {address} ({stem}) here", peer.addr),
            "pane",
            &format!(",\"addr\":{},\"pane\":{},\"program\":{}", js(&peer.addr), js(&address), js(&stem)),
        );
        self.link_answer(nonce, 200, first)
    }

    /// `POST /link/kill`: a pane that machine started here, closed; no
    /// other pane.
    fn link_kill_in(&mut self, req: &Inbound, nonce: &str, peer: &link::Peer) -> Outcome {
        if !peer.panes {
            return self.no_panes(nonce, peer);
        }
        let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        let Some(to) = v["to"].as_str() else {
            return self.link_answer(nonce, 400, "to: the pane".into());
        };
        let t = Target::parse(to);
        if t.remote.is_some() {
            return self.link_answer(nonce, 400, format!("{to}: a pane is named as this machine knows it"));
        }
        let pid = match self.resolve(Some(&t), None) {
            Ok((_, _, p)) => p,
            Err(e) => return self.link_answer(nonce, 404, e),
        };
        let theirs = self.link.as_ref().is_some_and(|l| l.made.get(&pid) == Some(&peer.key));
        if !theirs {
            let why = format!("%{pid} was not started by {}: it may close only the panes it started here", peer.addr);
            return self.link_answer(nonce, 403, why);
        }
        let address = self.address_of(pid);
        let cmd = match crate::command::parse(&["kill-pane".to_string(), "-t".into(), format!("%{pid}")]) {
            Ok(c) => c,
            Err(e) => return self.link_answer(nonce, 500, e),
        };
        match self.exec(cmd, None) {
            Outcome::Error(e) => self.link_answer(nonce, 500, e),
            _ => {
                if let Ok(l) = self.link() {
                    l.made.remove(&pid);
                }
                self.link_note(
                    &format!("link: {} closed {address} here", peer.addr),
                    "pane-closed",
                    &format!(",\"addr\":{},\"pane\":{}", js(&peer.addr), js(&address)),
                );
                self.link_answer(nonce, 200, format!("closed {address}"))
            }
        }
    }

    /// `POST /link/trace`: what became of a message that machine sent here;
    /// only its own messages (its key made them).
    fn link_trace_in(&mut self, cid: Option<ClientId>, req: &Inbound, nonce: &str, peer: &link::Peer) -> Outcome {
        let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        let Some(id) = v["id"].as_u64() else { return self.link_answer(nonce, 400, "id: a message number".into()) };
        let theirs = self.link.as_ref().and_then(|l| l.origin.get(&id)).is_some_and(|o| o.key == peer.key);
        if !theirs || self.observe.get(id).is_none() {
            return self.link_answer(nonce, 404, format!("no message #{id} from {} here", peer.addr));
        }
        match (v["wait"].as_u64(), cid) {
            (Some(secs), Some(cid)) if self.observe.get(id).is_some_and(|r| !r.stage.finished()) => {
                self.wait_link_trace(cid, secs, id, nonce.to_string());
                Outcome::Pending
            }
            _ => self.link_trace_answer(id, nonce),
        }
    }

    /// The trace of a message another machine sent here, as it reads it.
    pub(super) fn link_trace_answer(&mut self, id: super::actor::MsgId, nonce: &str) -> Outcome {
        let trace = self.observe.trace(id, self.opts.message_envelope).unwrap_or_default();
        let finished = self.observe.get(id).is_none_or(|r| r.stage.finished());
        let body = serde_json::json!({ "finished": finished, "trace": trace }).to_string();
        self.link_answer(nonce, 200, body)
    }

    /// `POST /link/send`: a message for a pane here, from a paired machine.
    fn link_send_in(&mut self, cid: Option<ClientId>, req: &Inbound, nonce: &str, peer: &link::Peer) -> Outcome {
        let Ok(body) = serde_json::from_str::<SendBody>(&req.body) else {
            return self.link_answer(nonce, 400, "the message is not what keepane sends".into());
        };
        // What goes into the envelope is held to what keepane itself writes
        // there: no value with a space or a `]` (docs/design/mailbox.md §5),
        // so no header can be forged inside one.
        if let Err(why) = sender_fields_ok(&body) {
            return self.link_answer(nonce, 400, why);
        }
        let t = Target::parse(&body.to);
        if t.remote.is_some() {
            return self.link_answer(nonce, 400, format!("{}: a pane is named as this machine knows it", body.to));
        }
        let to = match self.resolve(Some(&t), None) {
            Ok((_, _, p)) => p,
            Err(e) => return self.link_answer(nonce, 404, e),
        };
        let via = self.pane_ref(to).map_or(WorkMode::Normal, |p| p.actor.mode);
        if via == WorkMode::Shell && !peer.shell {
            let why = format!(
                "{} may not run commands here (%{to} is in shell mode): on this machine, keepane link allow {} --shell",
                peer.addr, peer.addr
            );
            self.link_refused(&peer.addr, &why);
            return self.link_answer(nonce, 403, why);
        }
        self.next_msg += 1;
        let id = self.next_msg;
        let known = |n: Option<u64>| n.filter(|n| self.observe.get(*n).is_some());
        let m = Message {
            id,
            task: known(body.into).unwrap_or(id),
            from: Sender::Remote {
                addr: peer.addr.clone(),
                address: body.from.clone(),
                name: body.name.clone(),
                mode: body.mode.as_deref().and_then(WorkMode::parse),
            },
            to: self.address_of(to),
            via,
            hop: body.hop,
            re: known(body.re),
            text: body.text.clone(),
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        if let Err(why) = self.enqueue(to, m) {
            return self.link_answer(nonce, 409, format!("#{id} rejected: {why}"));
        }
        if let Ok(l) = self.link() {
            l.origin.insert(id, Origin { key: peer.key.clone(), id: body.id, task: body.task });
            // Bounded like the message records: the oldest go first (an
            // answer to one of those has no way back, and says so).
            while l.origin.len() > super::link_state::ORIGINS_KEPT {
                let oldest = l.origin.keys().min().copied().expect("not empty");
                l.origin.remove(&oldest);
            }
        }
        match (body.wait, cid) {
            (Some(secs), Some(cid)) if self.observe.get(id).is_some_and(|r| r.stage == Stage::Queued) => {
                self.wait_link(cid, secs, id, nonce.to_string());
                Outcome::Pending
            }
            _ => self.link_send_answer(id, nonce),
        }
    }
}
