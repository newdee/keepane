//! What this server sends to another machine (docs/design/link.md): the
//! pairing request of `link-add`, a message for a pane there, `link-panes`,
//! `link-remove`. Each goes out in a task, signed with this server's key
//! (pairing: proved with the other side's web key instead), and its answer
//! comes back as `Event::LinkAnswer`, checked against the key paired with
//! that machine, and only then told to the client that asked.

use std::time::Duration;

use super::actor::{Message, MsgId, Sender, WorkMode};
use super::link_state::{Origin, js};
use super::{ClientId, Event, Outcome, Server};
use crate::link::{self, Answer, SendAnswer, SendBody};

/// A request under way: what its answer is for.
pub(super) enum LinkOp {
    /// `link-add`: their key comes back, proved with their web key.
    Pair { addr: String, webkey: String },
    /// A message handed over: its record here ends with what they said.
    Send { id: MsgId, addr: String, key: String },
    /// `link-panes`: their text, shown as it is.
    Panes { addr: String, key: String },
    /// `link-remove`: told them; done here whatever they say.
    Remove { addr: String, key: String },
}

/// The answer to a request, or why there is none.
pub(super) struct LinkAnswer {
    pub cid: Option<ClientId>,
    pub nonce: String,
    pub op: LinkOp,
    pub result: Result<Answer, String>,
}

/// Waiting for an answer beyond what the other side may itself wait.
const ANSWER_GRACE: Duration = Duration::from_secs(5);

/// The headers every request carries.
fn common_headers(from: &str, port: u16, time: i64, nonce: &str) -> Vec<(&'static str, String)> {
    vec![
        (link::H_PROTOCOL, link::PROTOCOL.to_string()),
        (link::H_VERSION, env!("CARGO_PKG_VERSION").to_string()),
        (link::H_FROM, from.to_string()),
        (link::H_PORT, port.to_string()),
        (link::H_TIME, time.to_string()),
        (link::H_NONCE, nonce.to_string()),
    ]
}

impl Server {
    /// Send `body` to `addr` in a task; the answer comes back as an event.
    #[allow(clippy::too_many_arguments)]
    fn link_dispatch(
        &self,
        cid: Option<ClientId>,
        addr: String,
        method: &'static str,
        path: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        longest: Duration,
        nonce: String,
        op: LinkOp,
    ) {
        let events = self.events.clone();
        tokio::spawn(async move {
            let result = link::call(&addr, method, path, &headers, &body, longest).await.map_err(|e| format!("{e:#}"));
            let _ = events.send(Event::LinkAnswer(Box::new(LinkAnswer { cid, nonce, op, result })));
        });
    }

    /// A request signed for the machine whose key is `to`.
    #[allow(clippy::too_many_arguments)]
    fn link_signed(
        &mut self,
        cid: Option<ClientId>,
        addr: &str,
        to: &str,
        method: &'static str,
        path: &'static str,
        body: Vec<u8>,
        longest: Duration,
        op: LinkOp,
    ) -> Result<(), String> {
        let (port, _) = self.own_web()?;
        let nonce = link::nonce().map_err(|e| format!("link: {e:#}"))?;
        let time = link::now();
        let l = self.link()?;
        let from = l.id.public();
        let text = link::request_text(method, path, &from, port, to, time, &nonce, &body);
        let mut headers = common_headers(&from, port, time, &nonce);
        headers.push((link::H_SIGN, l.id.sign(&text)));
        self.link_dispatch(cid, addr.to_string(), method, path, headers, body, longest, nonce, op);
        Ok(())
    }

    /// The commands that reach out to another machine.
    pub(super) fn exec_link_out(&mut self, cmd: crate::command::Cmd, cid: Option<ClientId>) -> Outcome {
        use crate::command::Cmd;
        let r = match cmd {
            Cmd::LinkAdd { url } => self.link_add(cid, &url),
            Cmd::LinkPanes { addr } => self.link_panes(cid, &addr),
            Cmd::LinkRemove { addr } => return self.link_remove(cid, &addr),
            other => Err(format!("not a link command: {other}")),
        };
        match r {
            Ok(()) => Outcome::Pending,
            Err(e) => Outcome::Error(e),
        }
    }

    /// `link-add`: ask the machine at the address the phone scans to pair,
    /// proving with its web key that we may.
    fn link_add(&mut self, cid: Option<ClientId>, url: &str) -> Result<(), String> {
        let (addr, webkey) = link::parse_invite(url)?;
        let (port, _) = self.own_web()?;
        let nonce = link::nonce().map_err(|e| format!("link: {e:#}"))?;
        let time = link::now();
        let from = self.link()?.id.public();
        let proof = link::b64(&link::hmac(webkey.as_bytes(), link::pair_text(&from, port, time, &nonce).as_bytes()));
        let body = format!("{{\"key\":{},\"port\":{port}}}", js(&from)).into_bytes();
        let mut headers = common_headers(&from, port, time, &nonce);
        headers.push((link::H_PROOF, proof));
        let op = LinkOp::Pair { addr: addr.clone(), webkey };
        self.link_dispatch(cid, addr, "POST", "/link/pair", headers, body, ANSWER_GRACE, nonce, op);
        Ok(())
    }

    fn peer_key(&mut self, addr: &str) -> Result<String, String> {
        self.link()?.peer_by_addr(addr).map(|p| p.key.clone()).ok_or_else(|| {
            format!(
                "{addr} is not paired with this machine: `keepane link add http://{addr}/#k=...` (its `keepane web` address), or `keepane link trust`"
            )
        })
    }

    fn link_panes(&mut self, cid: Option<ClientId>, addr: &str) -> Result<(), String> {
        let key = self.peer_key(addr)?;
        let op = LinkOp::Panes { addr: addr.to_string(), key: key.clone() };
        self.link_signed(cid, addr, &key, "GET", "/link/panes", Vec::new(), ANSWER_GRACE, op)
    }

    /// `link-remove`: gone from the table here at once; the other machine
    /// is told when it can be reached.
    fn link_remove(&mut self, cid: Option<ClientId>, addr: &str) -> Outcome {
        let key = match self.peer_key(addr) {
            Ok(k) => k,
            Err(e) => return Outcome::Error(e),
        };
        let by = self.by(cid);
        if let Ok(l) = self.link() {
            l.peers.retain(|p| p.key != key);
        }
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        self.link_note(
            &format!("link: {addr} removed by {by}"),
            "removed",
            &format!(",\"addr\":{},\"key\":{},\"by\":{}", js(addr), js(&key), js(&by)),
        );
        let op = LinkOp::Remove { addr: addr.to_string(), key: key.clone() };
        match self.link_signed(cid, addr, &key, "POST", "/link/remove", Vec::new(), ANSWER_GRACE, op) {
            Ok(()) => Outcome::Pending,
            // No web here: nothing to answer to, so they are not told.
            Err(e) => Outcome::Text(format!("removed {addr} here; it was not told ({e})")),
        }
    }

    /// A message for a pane of another machine (`send-message --to
    /// host:port/…`, or an answer to one that came from there). Recorded
    /// here as sent to `addr/to`; what they say of it ends the record.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_remote(
        &mut self,
        cid: Option<ClientId>,
        addr: &str,
        to: &str,
        text: String,
        from: Sender,
        carry: Option<(MsgId, u32)>,
        answer: Option<(MsgId, Origin)>,
        wait: Option<u64>,
    ) -> Outcome {
        let key = match self.peer_key(addr) {
            Ok(k) => k,
            Err(e) => return Outcome::Error(e),
        };
        if let Err(e) = self.own_web() {
            return Outcome::Error(e);
        }
        self.next_msg += 1;
        let id = self.next_msg;
        let (task, hop) = carry.unwrap_or((id, 0));
        let m = Message {
            id,
            task,
            from,
            to: format!("{addr}/{to}"),
            via: WorkMode::Normal,
            hop,
            re: answer.as_ref().map(|(local, _)| *local),
            text,
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        let refuse = if m.text.len() > self.opts.message_max_size {
            Some(format!("the message is {} bytes; message-max-size is {}", m.text.len(), self.opts.message_max_size))
        } else if m.hop > self.opts.message_hop_limit {
            Some(format!(
                "hop {} is over message-hop-limit {} (a loop between panes?)",
                m.hop, self.opts.message_hop_limit
            ))
        } else {
            None
        };
        if let Some(why) = refuse {
            self.observe.sent(&m, Some(&why));
            return Outcome::Error(format!("#{id} rejected: {why}"));
        }
        self.observe.sent(&m, None);
        let wait = wait.map(|w| w.min(self.opts.message_wait_max));
        let body = SendBody {
            to: to.to_string(),
            text: m.text.clone(),
            from: m.from.from_field(),
            name: m.from.name().map(String::from),
            mode: m.from.mode().map(|w| w.as_str().to_string()),
            hop,
            id,
            task,
            re: answer.as_ref().map(|(_, o)| o.id),
            into: answer.as_ref().map(|(_, o)| o.task),
            wait,
        };
        let body = serde_json::to_vec(&body).expect("a body serializes");
        let longest = ANSWER_GRACE + Duration::from_secs(wait.unwrap_or(0));
        let op = LinkOp::Send { id, addr: addr.to_string(), key: key.clone() };
        match self.link_signed(cid, addr, &key, "POST", "/link/send", body, longest, op) {
            Ok(()) => Outcome::Pending,
            Err(e) => {
                self.observe.refused(id, &e);
                Outcome::Error(format!("#{id} rejected: {e}"))
            }
        }
    }

    /// The answer came (or did not): check it, act on it, tell the client.
    pub(super) fn link_answered(&mut self, a: LinkAnswer) {
        let LinkAnswer { cid, nonce, op, result } = a;
        let out = match op {
            LinkOp::Pair { addr, webkey } => self.paired(&addr, &webkey, &nonce, result),
            LinkOp::Send { id, addr, key } => match Self::checked(&addr, &key, &nonce, result) {
                Ok(ans) if ans.status == 200 => match serde_json::from_slice::<SendAnswer>(&ans.body) {
                    Ok(said) => {
                        let stand = format!("{} on {addr}", said.stand);
                        self.observe.forwarded(id, &stand);
                        Outcome::Text(format!("#{id} forwarded: {stand}"))
                    }
                    Err(_) => {
                        let why = format!("{addr} answered with something that is not a keepane answer");
                        self.observe.refused(id, &why);
                        Outcome::Error(format!("#{id} rejected: {why}"))
                    }
                },
                Ok(ans) => {
                    let why = format!("{addr} said: {}", ans.text());
                    self.observe.refused(id, &why);
                    Outcome::Error(format!("#{id} rejected: {why}"))
                }
                Err(e) => {
                    self.observe.refused(id, &e);
                    Outcome::Error(format!("#{id} rejected: {e}"))
                }
            },
            LinkOp::Panes { addr, key } => match Self::checked(&addr, &key, &nonce, result) {
                Ok(ans) if ans.status == 200 => Outcome::Text(ans.text()),
                Ok(ans) => Outcome::Error(format!("{addr} said: {}", ans.text())),
                Err(e) => Outcome::Error(e),
            },
            LinkOp::Remove { addr, key } => match Self::checked(&addr, &key, &nonce, result) {
                Ok(ans) if ans.status == 200 => Outcome::Text(format!("removed {addr}, on both machines")),
                Ok(ans) => Outcome::Text(format!("removed {addr} here; it said: {}", ans.text())),
                Err(e) => Outcome::Text(format!("removed {addr} here; it was not told ({e})")),
            },
        };
        if let Some(cid) = cid {
            self.reply(cid, out);
        } else if let Outcome::Error(e) = out {
            self.note_message(&format!("link: {e}"));
        }
    }

    /// An answer as it is when its signature is the paired machine's, for
    /// this request; the reason otherwise.
    fn checked(addr: &str, key: &str, nonce: &str, result: Result<Answer, String>) -> Result<Answer, String> {
        let ans = result?;
        let sig = ans.header(link::H_SIGN).unwrap_or_default();
        if !link::verify(key, &link::answer_text(nonce, ans.status, &ans.body), sig) {
            return Err(format!(
                "the answer from {addr} is not signed by the key paired with it (another machine at that address? \
                 `keepane link remove {addr}`, then pair again); it said {}: {}",
                ans.status,
                ans.text()
            ));
        }
        Ok(ans)
    }

    /// Pairing's answer: their key, proved with their web key.
    fn paired(&mut self, addr: &str, webkey: &str, nonce: &str, result: Result<Answer, String>) -> Outcome {
        let ans = match result {
            Ok(a) => a,
            Err(e) => return Outcome::Error(format!("link-add: {e}")),
        };
        if ans.status != 200 {
            return Outcome::Error(format!("link-add: {addr} said: {}", ans.text()));
        }
        let v: serde_json::Value = match serde_json::from_slice(&ans.body) {
            Ok(v) => v,
            Err(_) => return Outcome::Error(format!("link-add: {addr} answered with something that is not keepane")),
        };
        let (Some(key), Some(proof)) = (v["key"].as_str(), v["proof"].as_str()) else {
            return Outcome::Error(format!("link-add: {addr} answered without its key"));
        };
        let want = link::hmac(webkey.as_bytes(), link::pair_answer_text(nonce, key).as_bytes());
        if !link::valid_public(key) || !link::unb64(proof).is_some_and(|p| link::same(&p, &want)) {
            return Outcome::Error(format!(
                "link-add: the answer from {addr} does not prove its web key: scan the code there again"
            ));
        }
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        if l.id.public() == key {
            return Outcome::Error(format!("link-add: {addr} is this server itself"));
        }
        let mut shell = false;
        match l.peers.iter_mut().find(|p| p.key == key) {
            Some(p) => {
                p.addr = addr.to_string();
                shell = p.shell;
            }
            None => l.peers.push(link::Peer { key: key.to_string(), addr: addr.to_string(), shell: false }),
        }
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        let fp = link::fingerprint(key);
        self.link_note(
            &format!("link: paired with {addr} ({fp})"),
            "paired",
            &format!(",\"addr\":{},\"key\":{},\"shell\":{shell},\"how\":\"added\"", js(addr), js(key)),
        );
        Outcome::Text(format!(
            "paired with {addr}  {fp}\nits panes: keepane link panes {addr}; a message: send-message --to {addr}/%name ...\n\
             its messages run as commands in shell panes here only after: keepane link allow {addr} --shell"
        ))
    }
}
