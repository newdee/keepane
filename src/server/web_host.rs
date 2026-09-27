//! The phones' page (`crate::web`), served by the server in the background:
//! `web-start` binds and serves, `web-stop` ends it (the phones connected
//! are cut off), `web-status` says how it serves and who is connected, and
//! `#{web_url}` / `#{web_clients}` put that on the status line. It ends with
//! the server.
//!
//! Who is connected comes from the requests themselves (`web::Seen`): the
//! page asks for the pane list every two seconds while it is shown and keeps
//! a stream open while it shows a pane, and asks nothing while the phone is
//! locked or the tab is in the background. So a phone is connected while it
//! streams, or asked within `CONNECTED_FOR`.

use super::{Event, Outcome, Server};
use crate::web::{Options, Seen};
use chrono::{DateTime, Local};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// A phone that asked this lately is taken as still there.
const CONNECTED_FOR: Duration = Duration::from_secs(10);
/// Phones remembered, connected or not (the oldest gone ones are forgotten).
const PEERS_KEPT: usize = 32;
/// Refused addresses remembered.
const REFUSED_KEPT: usize = 16;

pub(super) struct WebHost {
    /// Which start this is: a request still under way when an earlier one
    /// was stopped is not counted against this one.
    generation: u64,
    options: Options,
    url: String,
    since: DateTime<Local>,
    task: tokio::task::JoinHandle<()>,
    peers: Vec<Peer>,
    refused: Vec<IpAddr>,
    /// The count the status line was last drawn with.
    drawn: usize,
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // The listener and every connection go with the task.
        self.task.abort();
    }
}

struct Peer {
    addr: IpAddr,
    since: DateTime<Local>,
    last: Instant,
    last_at: DateTime<Local>,
    /// The panes streamed to it now (one entry per stream).
    watching: Vec<String>,
}

impl Peer {
    fn connected(&self) -> bool {
        !self.watching.is_empty() || self.last.elapsed() < CONNECTED_FOR
    }
}

impl WebHost {
    pub(super) fn clients(&self) -> usize {
        self.peers.iter().filter(|p| p.connected()).count()
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }

    /// `web-status`'s text. The first line is read back by `keepane web`
    /// (`web::status_url`) and the dashboard: `serving <url> · ... · N connected`.
    fn status(&self) -> String {
        let mut first = format!("serving {}", self.url);
        if self.options.read_only {
            first.push_str(" · read-only");
        }
        if self.options.keep_key {
            first.push_str(" · kept key");
        }
        first.push_str(&format!(" · since {} · {} connected", self.since.format("%H:%M"), self.clients()));
        let mut lines = vec![first];
        let mut peers: Vec<&Peer> = self.peers.iter().collect();
        // Connected first; each group in the order they came.
        peers.sort_by_key(|p| (!p.connected(), p.since));
        for p in peers {
            let what = if !p.watching.is_empty() {
                format!("watching {}", p.watching.join(" "))
            } else if p.connected() {
                "on the pane list".to_string()
            } else {
                format!("gone, last seen {}", p.last_at.format("%H:%M:%S"))
            };
            lines.push(format!("  {:<15}  {what}  (since {})", p.addr.to_string(), p.since.format("%H:%M")));
        }
        if !self.refused.is_empty() {
            let who: Vec<String> = self.refused.iter().map(IpAddr::to_string).collect();
            lines.push(format!("refused (no key or a wrong one): {}", who.join(", ")));
        }
        lines.join("\n")
    }
}

impl Server {
    pub(super) fn web_start(&mut self, o: Options) -> Outcome {
        if let Some(w) = &self.web {
            // Asked again as it serves (`keepane web` for the code again, a
            // config read again): how it serves.
            if o == Options::default() || o == w.options {
                return Outcome::Text(w.status());
            }
            return Outcome::Error(format!(
                "web: already serving {} (`keepane web stop` first, to serve it another way)",
                w.url
            ));
        }
        let ip = o.bind.unwrap_or_else(crate::web::lan_ip);
        let port = o.port.unwrap_or(crate::web::DEFAULT_PORT);
        let key = match if o.keep_key { crate::web::kept_key() } else { crate::web::new_key() } {
            Ok(k) => k,
            Err(e) => return Outcome::Error(format!("web: {e:#}")),
        };
        let listener = match std::net::TcpListener::bind((ip, port))
            .and_then(|l| l.set_nonblocking(true).map(|_| l))
            .and_then(tokio::net::TcpListener::from_std)
        {
            Ok(l) => l,
            Err(e) => return Outcome::Error(format!("web: listen on {ip}:{port}: {e} (in use? --port picks another)")),
        };
        // `-p 0`: whichever port the system gave.
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        self.web_generation += 1;
        let generation = self.web_generation;
        let events = self.events.clone();
        let state = crate::web::State::new(&self.socket, &key, o.read_only).notify(move |seen| {
            let _ = events.send(Event::Web(generation, seen));
        });
        let task = tokio::spawn(crate::web::serve(listener, std::sync::Arc::new(state)));
        let host = match ip {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let url = format!("http://{host}:{port}/#k={key}");
        log::info!("web: serving on {ip}:{port}");
        self.note_message(&format!("web: serving on {ip}:{port}"));
        self.web = Some(WebHost {
            generation,
            options: o,
            url,
            since: Local::now(),
            task,
            peers: Vec::new(),
            refused: Vec::new(),
            drawn: 0,
        });
        self.web_redraw();
        Outcome::Text(self.web.as_ref().map(WebHost::status).unwrap_or_default())
    }

    pub(super) fn web_stop(&mut self) -> Outcome {
        if self.web.take().is_none() {
            return Outcome::Error("web: not running".into());
        }
        log::info!("web: stopped");
        self.note_message("web: stopped");
        self.web_redraw();
        Outcome::Ok
    }

    pub(super) fn web_status(&self) -> String {
        match &self.web {
            Some(w) => w.status(),
            None => "off: `keepane web` starts it".into(),
        }
    }

    /// A request told us who asked. A new phone, or one back after it was
    /// gone, and a refused address are said on the status line: someone
    /// looking at the panes should never go unnoticed.
    pub(super) fn web_seen(&mut self, generation: u64, seen: Seen) {
        let Some(w) = self.web.as_mut().filter(|w| w.generation == generation) else { return };
        let (now, at) = (Instant::now(), Local::now());
        let mut say = None;
        match seen {
            Seen::Asked { peer, ok: false } => {
                if !w.refused.contains(&peer) {
                    if w.refused.len() == REFUSED_KEPT {
                        w.refused.remove(0);
                    }
                    w.refused.push(peer);
                    say = Some(format!("web: {peer} refused (no key or a wrong one)"));
                }
            }
            Seen::Asked { peer, ok: true } | Seen::Watching { peer, open: true, .. } => {
                match w.peers.iter_mut().find(|p| p.addr == peer) {
                    Some(p) => {
                        if !p.connected() {
                            say = Some(format!("web: {peer} connected again"));
                        }
                        p.last = now;
                        p.last_at = at;
                    }
                    None => {
                        if w.peers.len() == PEERS_KEPT
                            && let Some(i) = w.peers.iter().position(|p| !p.connected())
                        {
                            w.peers.remove(i);
                        }
                        w.peers.push(Peer { addr: peer, since: at, last: now, last_at: at, watching: Vec::new() });
                        say = Some(format!("web: {peer} connected"));
                    }
                }
                if let Seen::Watching { pane, .. } = seen
                    && let Some(p) = w.peers.iter_mut().find(|p| p.addr == peer)
                {
                    p.watching.push(pane);
                }
            }
            Seen::Watching { peer, pane, open: false } => {
                if let Some(p) = w.peers.iter_mut().find(|p| p.addr == peer) {
                    if let Some(i) = p.watching.iter().position(|x| *x == pane) {
                        p.watching.remove(i);
                    }
                    // A stream that just ended was the phone there just now.
                    p.last = now;
                    p.last_at = at;
                }
            }
        }
        if let Some(text) = say {
            self.note_message(&text);
            for c in self.clients.values_mut().filter(|c| c.session.is_some()) {
                c.message = Some((text.clone(), now));
            }
        }
        self.web_tick();
    }

    /// Every tick (and after each request): a phone that went quiet no
    /// longer counts; the status line is drawn again when the count moved.
    pub(super) fn web_tick(&mut self) {
        let Some(w) = self.web.as_mut() else { return };
        let n = w.clients();
        if n != w.drawn {
            w.drawn = n;
            self.web_redraw();
        }
    }

    fn web_redraw(&mut self) {
        for c in self.clients.values_mut() {
            c.last_grid = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        Server::new(pane_tx, "t".into(), events)
    }

    fn here() -> Options {
        Options { port: Some(0), bind: Some(IpAddr::from([127, 0, 0, 1])), ..Options::default() }
    }

    fn text(o: Outcome) -> String {
        match o {
            Outcome::Text(t) => t,
            Outcome::Error(e) => format!("error: {e}"),
            Outcome::Ok => "ok".into(),
            _ => "other".into(),
        }
    }

    /// Started once; asked again with no flag it says how it serves, with a
    /// flag it refuses (stop first); stopped, it is off and `#{web_url}` is
    /// empty.
    #[tokio::test]
    async fn it_starts_once_says_how_and_stops() {
        let mut s = server();
        assert_eq!(s.web_status(), "off: `keepane web` starts it");
        let first = text(s.web_start(here()));
        let url = crate::web::status_url(&first).expect(&first).to_string();
        assert!(url.starts_with("http://127.0.0.1:") && !url.contains(":0/"), "{url}");
        assert!(url.contains("/#k=") && first.ends_with(" · 0 connected"), "{first}");
        assert_eq!(crate::web::status_url(&text(s.web_start(Options::default()))), Some(url.as_str()));
        // The same flags again (a config read again): how it serves, too.
        assert_eq!(crate::web::status_url(&text(s.web_start(here()))), Some(url.as_str()));
        let again = text(s.web_start(Options { read_only: true, ..here() }));
        assert!(again.starts_with("error: web: already serving"), "{again}");
        assert_eq!(s.web.as_ref().map(|w| w.url().to_string()), Some(url));
        assert_eq!(text(s.web_stop()), "ok");
        assert!(s.web.is_none());
        assert_eq!(text(s.web_stop()), "error: web: not running");
        assert!(text(s.web_start(Options { read_only: true, ..here() })).contains(" · read-only · "));
    }

    /// A phone counts while it asks (within `CONNECTED_FOR`) or watches; one
    /// that went quiet is listed as gone; a new one, one back, and a refused
    /// address are said once each; the lists stay bounded.
    #[tokio::test]
    async fn phones_count_while_they_ask_or_watch() {
        let mut s = server();
        text(s.web_start(here()));
        let g = s.web_generation;
        let phone = IpAddr::from([192, 168, 1, 20]);
        // A request from an earlier start, still under way: not this one's.
        s.web_seen(g - 1, Seen::Asked { peer: phone, ok: true });
        assert_eq!(s.web.as_ref().unwrap().peers.len(), 0);
        let clients = |s: &Server| s.web.as_ref().unwrap().clients();
        s.web_seen(g, Seen::Asked { peer: phone, ok: true });
        s.web_seen(g, Seen::Asked { peer: phone, ok: true });
        assert_eq!(clients(&s), 1);
        assert!(s.web_status().contains("192.168.1.20     on the pane list"), "{}", s.web_status());
        let said = |s: &Server, what: &str| s.messages.iter().filter(|m| m.ends_with(what)).count();
        assert_eq!(said(&s, "web: 192.168.1.20 connected"), 1);

        // Quiet for longer than CONNECTED_FOR: gone, and the count drops.
        let ago = Instant::now() - CONNECTED_FOR - Duration::from_secs(1);
        s.web.as_mut().unwrap().peers[0].last = ago;
        s.web_tick();
        assert_eq!(clients(&s), 0);
        assert!(s.web_status().contains("gone, last seen"), "{}", s.web_status());
        assert!(s.web_status().lines().next().unwrap().ends_with(" · 0 connected"));

        // Watching a pane keeps it connected however long the stream lasts.
        s.web_seen(g, Seen::Watching { peer: phone, pane: "%3".into(), open: true });
        assert_eq!(said(&s, "web: 192.168.1.20 connected again"), 1);
        s.web.as_mut().unwrap().peers[0].last = ago;
        assert_eq!(clients(&s), 1);
        assert!(s.web_status().contains("watching %3"), "{}", s.web_status());
        s.web_seen(g, Seen::Watching { peer: phone, pane: "%3".into(), open: false });
        assert!(s.web.as_ref().unwrap().peers[0].watching.is_empty());

        // Refused: listed, said once.
        let stranger = IpAddr::from([10, 0, 0, 9]);
        s.web_seen(g, Seen::Asked { peer: stranger, ok: false });
        s.web_seen(g, Seen::Asked { peer: stranger, ok: false });
        assert_eq!(said(&s, "web: 10.0.0.9 refused (no key or a wrong one)"), 1);
        assert!(s.web_status().ends_with("refused (no key or a wrong one): 10.0.0.9"), "{}", s.web_status());

        // Bounded: gone phones make room; refused addresses roll over.
        for i in 0..(PEERS_KEPT as u8 + 8) {
            s.web_seen(g, Seen::Asked { peer: IpAddr::from([10, 1, 0, i]), ok: true });
            s.web.as_mut().unwrap().peers.iter_mut().for_each(|p| p.last = ago);
            s.web_seen(g, Seen::Asked { peer: IpAddr::from([10, 2, 0, i]), ok: false });
        }
        let w = s.web.as_ref().unwrap();
        assert!(
            w.peers.len() <= PEERS_KEPT && w.refused.len() == REFUSED_KEPT,
            "{} {}",
            w.peers.len(),
            w.refused.len()
        );
    }
}
