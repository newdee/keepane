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
            None => l.peers.push(link::Peer { key: from.to_string(), addr: addr.to_string(), shell: false }),
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
