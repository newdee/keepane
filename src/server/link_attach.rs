//! `keepane link attach` (docs/design/link.md, decision 16): a session of
//! this machine, attached for another machine that has all of this one
//! (`full`), as a terminal there would be if it were here. The client is
//! made here like any other, of the size that machine's terminal is, and
//! takes that machine's keys as a terminal's: the prefix, the bindings, copy
//! mode and the prompts are this machine's. What it is drawn goes into a
//! screen of its own (a VT parser), which that machine asks for the way it
//! asks for a pane's (`/link/view`, with `client` instead of `to`). A client
//! not asked for in `UNASKED` is let go: the program there is gone, or the
//! network is, and it asks for a new one when it is back.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::{ClientId, OUTPUT_QUEUE, Outcome, Server, SessionId};
use crate::command::{Cmd, Target};
use crate::ipc::{ClientMsg, KeyRecord, ServerMsg};
use crate::link::Peer;

/// The first id of a client made for another machine: out of the way of
/// the connections', which count up from 1.
pub(super) const FIRST_ID: ClientId = 1 << 31;

/// How long a client may go unasked for before it is let go (the program
/// there asks at least every 2 seconds, even while its answers fail).
const UNASKED: Duration = Duration::from_secs(15);

/// The size a client may be made: about what a terminal can be (a 4K
/// screen, full, is some 550 x 135). Each frame of a client is drawn whole
/// into its screen and sent whole when it changes, so a bigger one would
/// cost the server for nothing; a terminal there bigger than this shows
/// the client in its top-left corner.
const MIN_SIZE: u16 = 2;
const MAX_COLS: u16 = 500;
const MAX_ROWS: u16 = 200;

/// A client attached here for another machine.
pub(super) struct RemoteClient {
    /// The machine it is for: only that machine may ask for it or type in it.
    key: String,
    addr: String,
    /// What the server sends the client, and the screen it is drawn into.
    rx: mpsc::Receiver<ServerMsg>,
    parser: vt100::Parser,
    /// Bytes drawn into the screen so far: what changes when it does.
    drawn: u64,
    asked: Instant,
    /// Detached (`prefix d`, its session gone): why. Kept until it goes
    /// unasked, so the program there hears it even when an answer is lost,
    /// and does not take a missing client for a dropped one to attach again.
    ended: Option<String>,
    session: String,
}

impl RemoteClient {
    /// What the server sent since last asked, drawn into the screen.
    fn take(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                ServerMsg::Output(bytes) => {
                    self.parser.process(&bytes);
                    self.drawn += bytes.len() as u64;
                }
                ServerMsg::Detached { reason } => self.ended = Some(reason),
                _ => {}
            }
        }
    }
}

/// The size asked for, held to what a terminal can be.
fn size_of(v: &serde_json::Value) -> Option<(u16, u16)> {
    let n = |x: &serde_json::Value, max: u16| x.as_u64().map(|n| n.clamp(u64::from(MIN_SIZE), u64::from(max)) as u16);
    Some((n(&v["cols"], MAX_COLS)?, n(&v["rows"], MAX_ROWS)?))
}

impl Server {
    /// `POST /link/attach`: a client attached to a session here (the one
    /// named, else the one used last) for a machine with all of this one.
    /// Its id comes back, with the session's name.
    /// The session another machine attaches to: the one named, else the
    /// one used last; this machine's own, never a third machine's copied
    /// here (`mirrored`), which is not this one's to give.
    pub(super) fn session_to_attach(&self, target: Option<&Target>) -> Result<SessionId, String> {
        let sid = match target {
            None => self
                .sessions
                .iter()
                .filter(|s| s.remote.is_none())
                .max_by_key(|s| s.last_used)
                .map(|s| s.id)
                .ok_or_else(|| "no sessions".to_string())?,
            Some(_) => self.resolve_session(target, None)?,
        };
        match self.session(sid) {
            Some(s) if s.remote.is_none() => Ok(sid),
            _ => Err("no such session".into()),
        }
    }

    pub(super) fn link_attach_in(&mut self, body: &str, nonce: &str, peer: &Peer) -> Outcome {
        if let Some(no) = self.need_full(nonce, peer, "attach to sessions") {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let (cols, rows) = size_of(&v).unwrap_or((80, 24));
        let target = v["session"].as_str().filter(|s| !s.is_empty()).map(Target::parse);
        if target.as_ref().is_some_and(|t| t.remote.is_some()) {
            return self.link_answer(nonce, 400, "session: a session is named as this machine knows it".into());
        }
        let sid = match self.session_to_attach(target.as_ref()) {
            Ok(s) => s,
            Err(e) => return self.link_answer(nonce, 404, e),
        };
        let Some(name) = self.session(sid).map(|s| s.name.clone()) else {
            return self.link_answer(nonce, 404, "no such session".into());
        };
        let id = self.next_remote_client;
        self.next_remote_client = self.next_remote_client.checked_add(1).unwrap_or(FIRST_ID);
        let (tx, rx) = mpsc::channel(OUTPUT_QUEUE);
        self.client_connected(id, tx);
        if let Some(c) = self.clients.get_mut(&id) {
            (c.cols, c.rows, c.interactive) = (cols, rows, true);
        }
        self.remote_clients.insert(
            id,
            RemoteClient {
                key: peer.key.clone(),
                addr: peer.addr.clone(),
                rx,
                parser: vt100::Parser::new(rows, cols, 0),
                drawn: 0,
                asked: Instant::now(),
                ended: None,
                session: name.clone(),
            },
        );
        let at = Target { session: Some(format!("${sid}")), ..Default::default() };
        match self.exec(Cmd::AttachSession { target: Some(at), detach_others: false }, Some(id)) {
            out @ Outcome::Attach(_) => self.reply(id, out),
            Outcome::Error(e) => {
                self.client_gone(id);
                return self.link_answer(nonce, 500, e);
            }
            _ => {
                self.client_gone(id);
                return self.link_answer(nonce, 500, "the client was not attached".into());
            }
        }
        self.link_note(
            &format!("link: {} attached to {name} here", peer.addr),
            "attached",
            &format!(
                ",\"addr\":{},\"session\":{},\"client\":{id}",
                super::link_state::js(&peer.addr),
                super::link_state::js(&name)
            ),
        );
        let body = serde_json::json!({ "client": id, "session": name }).to_string();
        self.link_answer(nonce, 200, body)
    }

    /// The client a request of that machine names, if it is one made for
    /// it; the answer to give otherwise.
    fn remote_client(&mut self, v: &serde_json::Value, nonce: &str, peer: &Peer) -> Result<ClientId, Outcome> {
        let id = v["client"].as_u64().and_then(|n| ClientId::try_from(n).ok());
        match id.filter(|id| self.remote_clients.get(id).is_some_and(|r| r.key == peer.key)) {
            Some(id) => {
                if let Some(r) = self.remote_clients.get_mut(&id) {
                    r.asked = Instant::now();
                }
                Ok(id)
            }
            None => {
                let what = id.map_or_else(|| v["client"].to_string(), |id| id.to_string());
                let why = format!(
                    "no client {what} here for {}: it ended, or went unasked for {} seconds",
                    peer.addr,
                    UNASKED.as_secs()
                );
                Err(self.link_answer(nonce, 404, why))
            }
        }
    }

    /// `/link/view` with `client`: what that client is drawn, as a pane's
    /// screen is answered; at the size asked for, which the client takes.
    pub(super) fn link_client_view(&mut self, v: &serde_json::Value, nonce: &str, peer: &Peer) -> Outcome {
        if let Some(no) = self.need_full(nonce, peer, "attach to sessions") {
            return no;
        }
        let id = match self.remote_client(v, nonce, peer) {
            Ok(id) => id,
            Err(out) => return out,
        };
        let r = self.remote_clients.get_mut(&id).expect("just found");
        r.take();
        // A new size: what was drawn at the old one stays until the client
        // is drawn again, whole, at the new one.
        let (rows, cols) = r.parser.screen().size();
        if let Some((c, w)) = size_of(v).filter(|s| *s != (cols, rows))
            && r.ended.is_none()
        {
            r.parser.screen_mut().set_size(w, c);
            self.handle_msg(id, ClientMsg::Resize { cols: c, rows: w });
        }
        let r = self.remote_clients.get(&id).expect("still there");
        let s = r.parser.screen();
        let (rows, cols) = s.size();
        let stamp = format!("{} {cols}x{rows} {}", r.drawn, r.ended.is_some());
        let body = if v["since"].as_str() == Some(stamp.as_str()) {
            serde_json::json!({ "stamp": stamp, "same": true })
        } else {
            let (row, col) = s.cursor_position();
            serde_json::json!({
                "stamp": stamp,
                "cols": cols,
                "rows": rows,
                "cursor": [col, row],
                "cursor_shown": !s.hide_cursor(),
                "screen": String::from_utf8_lossy(&s.contents_formatted()),
                "title": r.session,
                "dead": r.ended.is_some(),
                "ended": r.ended,
                "typing": true,
            })
        };
        self.link_answer(nonce, 200, body.to_string())
    }

    /// `/link/keys` with `client`: keys typed into that client, as a
    /// terminal's are (the prefix and every binding are this machine's).
    pub(super) fn link_client_keys(
        &mut self,
        v: &serde_json::Value,
        keys: Vec<KeyRecord>,
        nonce: &str,
        peer: &Peer,
    ) -> Outcome {
        let id = match self.remote_client(v, nonce, peer) {
            Ok(id) => id,
            Err(out) => return out,
        };
        for k in keys {
            self.handle_key(id, k);
        }
        self.link_answer(nonce, 200, String::new())
    }

    /// Clients no one asked for in `UNASKED`: let go, and the session they
    /// were in takes its size from the clients left.
    pub(super) fn remote_clients_tick(&mut self) {
        let gone: Vec<(ClientId, String, String, bool)> = self
            .remote_clients
            .iter()
            .filter(|(_, r)| r.asked.elapsed() >= UNASKED)
            .map(|(id, r)| (*id, r.addr.clone(), r.session.clone(), r.ended.is_some()))
            .collect();
        for (id, addr, session, ended) in gone {
            self.client_gone(id);
            if !ended {
                self.link_note(
                    &format!("link: {addr} left {session} here (not heard from in {} seconds)", UNASKED.as_secs()),
                    "detached",
                    &format!(
                        ",\"addr\":{},\"session\":{},\"client\":{id}",
                        super::link_state::js(&addr),
                        super::link_state::js(&session)
                    ),
                );
            }
        }
    }

    /// A client made for another machine: which one, for `list-clients`.
    pub(super) fn remote_client_addr(&self, id: ClientId) -> Option<&str> {
        self.remote_clients.get(&id).map(|r| r.addr.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_asked_for_is_held_to_what_a_terminal_can_be() {
        let v = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(size_of(&v(r#"{"cols":120,"rows":40}"#)), Some((120, 40)));
        assert_eq!(size_of(&v(r#"{"cols":0,"rows":1}"#)), Some((MIN_SIZE, MIN_SIZE)));
        assert_eq!(size_of(&v(r#"{"cols":99999,"rows":5000}"#)), Some((MAX_COLS, MAX_ROWS)));
        assert_eq!(size_of(&v(r#"{"cols":500,"rows":200}"#)), Some((500, 200)), "the largest, as asked");
        assert_eq!(size_of(&v(r#"{"cols":80}"#)), None, "no height: no size");
        assert_eq!(size_of(&v(r#"{"cols":"80","rows":24}"#)), None, "not a number");
    }

    #[test]
    fn what_a_client_is_sent_is_drawn_and_one_no_one_asks_for_is_let_go() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        let make = |s: &mut Server, id: ClientId, unasked: Duration, ended: Option<&str>| {
            let (tx, rx) = mpsc::channel(4);
            s.client_connected(id, tx.clone());
            s.remote_clients.insert(
                id,
                RemoteClient {
                    key: "k".into(),
                    addr: "10.0.0.1:7681".into(),
                    rx,
                    parser: vt100::Parser::new(3, 10, 0),
                    drawn: 0,
                    asked: Instant::now() - unasked,
                    ended: ended.map(String::from),
                    session: "w".into(),
                },
            );
            tx
        };
        // Frames are drawn in order; a detach is heard; the rest is not a
        // screen's (a command's answer) and leaves it alone.
        let tx = make(&mut s, FIRST_ID, Duration::ZERO, None);
        tx.try_send(ServerMsg::Output(b"hello".to_vec())).unwrap();
        tx.try_send(ServerMsg::Text("not drawn".into())).unwrap();
        tx.try_send(ServerMsg::Output(b"\r\nthere".to_vec())).unwrap();
        tx.try_send(ServerMsg::Detached { reason: "detached".into() }).unwrap();
        let r = s.remote_clients.get_mut(&FIRST_ID).unwrap();
        r.take();
        assert_eq!(r.drawn, 12);
        assert_eq!(r.parser.screen().contents(), "hello\nthere");
        assert_eq!(r.ended.as_deref(), Some("detached"));
        // Unasked past `UNASKED`: gone, from the clients too; asked: kept.
        make(&mut s, FIRST_ID + 1, UNASKED + Duration::from_secs(1), Some("detached"));
        s.remote_clients_tick();
        assert!(!s.remote_clients.contains_key(&(FIRST_ID + 1)) && !s.clients.contains_key(&(FIRST_ID + 1)));
        assert!(s.remote_clients.contains_key(&FIRST_ID) && s.clients.contains_key(&FIRST_ID));
        assert_eq!(s.remote_client_addr(FIRST_ID), Some("10.0.0.1:7681"));
        assert_eq!(s.remote_client_addr(1), None, "a connection is not one");
    }
}
