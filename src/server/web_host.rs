//! The phones' page (`crate::web`), served by the server in the background:
//! `web-start` binds and serves, `web-stop` ends it (the phones connected
//! are cut off), `web-status` says how it serves and who is connected, and
//! `#{web_url}` / `#{web_clients}` put that on the status line. It ends with
//! the server, and comes back the same way when the sessions are restored
//! (`web-resume`), unless it was stopped.
//!
//! Who is connected comes from the requests themselves (`web::Seen`): the
//! page asks for the pane list every two seconds while it is shown and keeps
//! a stream open while it shows a pane, and asks nothing while the phone is
//! locked or the tab is in the background. So a phone is connected while it
//! streams, or asked within `CONNECTED_FOR`.

use super::{ClientId, Event, Outcome, Server};
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
/// How often this machine's addresses are looked at again while it serves
/// on them (`--bind` not given), and a start on restore that could not be
/// made is tried again: at logon the network may come up after keepane.
const LOOK_AGAIN: Duration = Duration::from_secs(10);

pub(super) struct WebHost {
    /// Which start this is: a request still under way when an earlier one
    /// was stopped is not counted against this one.
    generation: u64,
    options: Options,
    /// The address to scan: on the first of the addresses served.
    url: String,
    /// Each address served and its listener, the first the one scanned;
    /// the others (this machine's Tailscale ones) the same port and key.
    /// The tasks are taken by `web-stop`, which waits for them to end.
    served: Vec<(IpAddr, tokio::task::JoinHandle<()>)>,
    port: u16,
    key: String,
    since: DateTime<Local>,
    /// For a listener on an address that comes later.
    state: std::sync::Arc<crate::web::State>,
    /// When the addresses were last looked at, and those that could not be
    /// listened on then (said once each).
    looked: Instant,
    unbound: Vec<IpAddr>,
    peers: Vec<Peer>,
    refused: Vec<IpAddr>,
    /// The count the status line was last drawn with.
    drawn: usize,
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // The listeners and every connection go with the tasks.
        for (_, task) in &self.served {
            task.abort();
        }
    }
}

/// A start on restore that could not be made yet, tried again.
pub(super) struct WebRetry {
    options: Options,
    error: String,
    next: Instant,
}

fn bind(ip: IpAddr, port: u16) -> std::io::Result<tokio::net::TcpListener> {
    std::net::TcpListener::bind((ip, port))
        .and_then(|l| l.set_nonblocking(true).map(|_| l))
        .and_then(tokio::net::TcpListener::from_std)
}

/// The same addresses, the same one first (the one scanned): the others
/// in whatever order the system lists them.
fn same_addrs(a: &[IpAddr], b: &[IpAddr]) -> bool {
    let sorted = |x: &[IpAddr]| {
        let mut v = x.to_vec();
        v.sort();
        v
    };
    a.first() == b.first() && sorted(a) == sorted(b)
}

fn scanned(ip: IpAddr, port: u16, key: &str) -> String {
    format!("http://{}/#k={key}", crate::link::hostport(ip, port))
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

    /// How it serves (for the tests of what turns it on).
    #[cfg(test)]
    pub(super) fn options(&self) -> &Options {
        &self.options
    }

    /// The addresses served on, the one scanned first.
    pub(super) fn addrs(&self) -> Vec<IpAddr> {
        self.served.iter().map(|(ip, _)| *ip).collect()
    }

    /// The port served on: what another machine's keepane answers to.
    pub(super) fn port(&self) -> u16 {
        self.port
    }

    /// The key in the address the phone scans: what pairing with another
    /// machine is proved with (docs/design/link.md).
    pub(super) fn key(&self) -> &str {
        &self.key
    }

    pub(super) fn read_only(&self) -> bool {
        self.options.read_only
    }

    /// Some phone shows pane `pane` (`%N`) now.
    pub(super) fn watching(&self, pane: &str) -> bool {
        self.peers.iter().any(|p| p.watching.iter().any(|w| w == pane))
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
        for (ip, _) in self.served.iter().skip(1) {
            lines.push(format!("also {}", scanned(*ip, self.port, &self.key)));
        }
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
    /// `web-start`: served, and kept so (`web::keep_on`) until `web-stop`.
    pub(super) fn web_start(&mut self, o: Options) -> Outcome {
        let out = self.web_serve(o);
        if let (Outcome::Text(_), Some(w)) = (&out, &self.web) {
            // Asked to serve: a start still waiting to be tried is not.
            self.web_retry = None;
            if let Err(e) = crate::web::keep_on(&self.web_on, &w.options) {
                log::warn!("web: {e:#}");
            }
        }
        out
    }

    /// `web-resume`: served as it was when the server ended, unless stopped
    /// (with the sessions restored: `keepane startup`, `restart-server`).
    /// What cannot be served yet (an address not up so soon after logon) is
    /// tried again every `LOOK_AGAIN`, until served or `web-stop`.
    pub(super) fn web_resume(&mut self) -> Outcome {
        if self.web.is_some() {
            return Outcome::Text(self.web_status());
        }
        let Some(o) = crate::web::kept_on(&self.web_on) else {
            return Outcome::Text("off: it was not served when the server ended".into());
        };
        match self.web_serve(o.clone()) {
            Outcome::Error(e) => {
                log::warn!("{e}; tried again every {} s", LOOK_AGAIN.as_secs());
                self.note_message(&format!("{e}; tried again every {} s", LOOK_AGAIN.as_secs()));
                self.web_retry = Some(WebRetry { options: o, error: e, next: Instant::now() + LOOK_AGAIN });
                Outcome::Text(self.web_status())
            }
            out => out,
        }
    }

    pub(super) fn web_serve(&mut self, o: Options) -> Outcome {
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
        // The address this machine reaches the network through and its
        // Tailscale addresses (or the one asked for), one port for all.
        let addrs = crate::link::listen_addrs(&o.bind);
        let ip = addrs[0];
        let port = o.port.unwrap_or(crate::web::DEFAULT_PORT);
        let key = match if o.keep_key { crate::web::kept_key() } else { crate::web::new_key() } {
            Ok(k) => k,
            Err(e) => return Outcome::Error(format!("web: {e:#}")),
        };
        let listener = match bind(ip, port) {
            Ok(l) => l,
            // What to look at: the address, or the port.
            Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {
                return Outcome::Error(format!("web: listen on {ip}:{port}: {e} (an address of this machine?)"));
            }
            Err(e) => return Outcome::Error(format!("web: listen on {ip}:{port}: {e} (in use? --port picks another)")),
        };
        // `-p 0`: whichever port the system gave; the other addresses take
        // the same one (one that cannot is left out, and said).
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let mut listeners = vec![(ip, listener)];
        let mut unbound = Vec::new();
        for other in addrs.iter().skip(1) {
            match bind(*other, port) {
                Ok(l) => listeners.push((*other, l)),
                // One asked for (`-b`) is not left out quietly: an address
                // this machine does not have, say.
                Err(e) if !o.bind.is_empty() => {
                    return Outcome::Error(format!("web: listen on {other}:{port}: {e} (an address of this machine?)"));
                }
                Err(e) => {
                    log::warn!("web: not on {other}:{port}: {e}");
                    unbound.push(*other);
                }
            }
        }
        self.web_generation += 1;
        let generation = self.web_generation;
        let events = self.events.clone();
        let state = std::sync::Arc::new(crate::web::State::new(&self.socket, &key, o.read_only).notify(move |seen| {
            let _ = events.send(Event::Web(generation, seen));
        }));
        let served: Vec<_> =
            listeners.into_iter().map(|(ip, l)| (ip, tokio::spawn(crate::web::serve(l, state.clone())))).collect();
        let url = scanned(ip, port, &key);
        let where_ = served.iter().map(|(ip, _)| crate::link::hostport(*ip, port)).collect::<Vec<_>>().join(", ");
        log::info!("web: serving on {where_}");
        self.note_message(&format!("web: serving on {where_}"));
        self.web = Some(WebHost {
            generation,
            options: o,
            url,
            served,
            port,
            key,
            since: Local::now(),
            state,
            looked: Instant::now(),
            unbound,
            peers: Vec::new(),
            refused: Vec::new(),
            drawn: 0,
        });
        self.web_redraw();
        Outcome::Text(self.web.as_ref().map(WebHost::status).unwrap_or_default())
    }

    /// Stop serving. A client that asked is answered once the task has
    /// ended, the port closed with it: `keepane web stop` followed at once by
    /// `keepane web` on the same port finds it free.
    pub(super) fn web_stop(&mut self, cid: Option<ClientId>) -> Outcome {
        // Not served again on restore, nor tried again.
        crate::web::forget_on(&self.web_on);
        let waiting = self.web_retry.take().is_some();
        let Some(mut host) = self.web.take() else {
            if waiting {
                self.note_message("web: stopped (it was waiting to be served again)");
                return Outcome::Ok;
            }
            return Outcome::Error("web: not running".into());
        };
        log::info!("web: stopped");
        self.note_message("web: stopped");
        self.web_redraw();
        let tasks: Vec<_> = std::mem::take(&mut host.served).into_iter().map(|(_, t)| t).collect();
        let Some(cid) = cid else { return Outcome::Ok };
        for task in &tasks {
            task.abort();
        }
        let events = self.events.clone();
        tokio::spawn(async move {
            // Cancelled: the futures, the listeners in them, are dropped by now.
            for task in tasks {
                let _ = task.await;
            }
            let _ = events.send(Event::WebStopped(cid));
        });
        Outcome::Pending
    }

    pub(super) fn web_status(&self) -> String {
        match (&self.web, &self.web_retry) {
            (Some(w), _) => w.status(),
            (None, Some(r)) => format!(
                "off: to be served again (web-start {}), tried every {} s: {}",
                r.options.flags().join(" "),
                LOOK_AGAIN.as_secs(),
                r.error
            ),
            (None, None) => "off: `keepane web` starts it".into(),
        }
    }

    /// Every tick: a start on restore that could not be made, tried again
    /// when due; serving on this machine's addresses, those looked at again
    /// when due (served on the ones that came, no longer on the ones gone).
    pub(super) fn web_look_again(&mut self) {
        if let Some(r) = self.web_retry.as_mut().filter(|r| Instant::now() >= r.next) {
            r.next = Instant::now() + LOOK_AGAIN;
            let o = r.options.clone();
            match self.web_serve(o) {
                Outcome::Error(e) => {
                    if let Some(r) = self.web_retry.as_mut() {
                        r.error = e;
                    }
                }
                _ => self.web_retry = None,
            }
            return;
        }
        let Some(w) = self.web.as_mut().filter(|w| w.options.bind.is_empty() && w.looked.elapsed() >= LOOK_AGAIN)
        else {
            return;
        };
        w.looked = Instant::now();
        let want = crate::link::listen_addrs(&[]);
        let have: Vec<IpAddr> = w.served.iter().map(|(ip, _)| *ip).collect();
        if same_addrs(&want, &have) {
            return;
        }
        let mut served = Vec::new();
        let mut old = std::mem::take(&mut w.served);
        let mut unbound = Vec::new();
        for ip in &want {
            if let Some(i) = old.iter().position(|(o, _)| o == ip) {
                served.push(old.remove(i));
                continue;
            }
            match bind(*ip, w.port) {
                Ok(l) => served.push((*ip, tokio::spawn(crate::web::serve(l, w.state.clone())))),
                Err(e) => {
                    if !w.unbound.contains(ip) {
                        log::warn!("web: not on {}: {e}", crate::link::hostport(*ip, w.port));
                    }
                    unbound.push(*ip);
                }
            }
        }
        w.unbound = unbound;
        // Nothing to serve on (no listener could be made): left as it was,
        // looked at again next time.
        if served.is_empty() {
            w.served = old;
            return;
        }
        // Gone from this machine (a loopback stand-in for no network
        // included): no longer listened on.
        for (_, task) in &old {
            task.abort();
        }
        w.served = served;
        let now: Vec<IpAddr> = w.served.iter().map(|(ip, _)| *ip).collect();
        if same_addrs(&now, &have) {
            return;
        }
        w.url = scanned(now[0], w.port, &w.key);
        let where_ = now.iter().map(|ip| crate::link::hostport(*ip, w.port)).collect::<Vec<_>>().join(", ");
        log::info!("web: now serving on {where_}");
        self.note_message(&format!("web: now serving on {where_}"));
        self.web_redraw();
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

/// A unit test's server: its own file of how it served goes with it.
#[cfg(test)]
impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.web_on);
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
        Options { port: Some(0), bind: vec![IpAddr::from([127, 0, 0, 1])], ..Options::default() }
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
        assert_eq!(text(s.web_stop(None)), "ok");
        assert!(s.web.is_none());
        assert_eq!(text(s.web_stop(None)), "error: web: not running");
        assert!(text(s.web_start(Options { read_only: true, ..here() })).contains(" · read-only · "));
    }

    /// Served from `web-start` to `web-stop`: a server after it (the
    /// sessions restored) serves it the same way again; stopped, not.
    #[tokio::test]
    async fn it_is_served_again_as_it_was_until_stopped() {
        let mut a = server();
        assert_eq!(text(a.web_resume()), "off: it was not served when the server ended");
        assert!(a.web.is_none() && a.web_retry.is_none());
        let o = Options { read_only: true, ..here() };
        text(a.web_start(o.clone()));
        assert_eq!(std::fs::read_to_string(&a.web_on).unwrap(), "-p 0 -b 127.0.0.1 -r\n");
        // Asked again with no flag: still kept as it serves.
        text(a.web_start(Options::default()));
        assert_eq!(crate::web::kept_on(&a.web_on), Some(o.clone()));
        // The server ends; the file stays (as a real one's does).
        let file = std::mem::take(&mut a.web_on);
        drop(a);

        let mut b = server();
        b.web_on = file.clone();
        let said = text(b.web_resume());
        assert!(said.starts_with("serving http://127.0.0.1:") && said.contains(" · read-only · "), "{said}");
        assert_eq!(b.web.as_ref().unwrap().options, o);
        // Served already (the config did): how it serves, nothing else.
        assert_eq!(text(b.web_resume()), b.web_status());
        assert_eq!(text(b.web_stop(None)), "ok");
        assert!(!file.exists(), "stopped: not served again");

        let mut c = server();
        c.web_on = file.clone();
        assert_eq!(text(c.web_resume()), "off: it was not served when the server ended");
        assert!(c.web.is_none());
        // A file not written by keepane: nothing served from it.
        std::fs::write(&file, "--nope\n").unwrap();
        assert!(text(c.web_resume()).starts_with("off: it was not served"));
        std::fs::remove_file(&file).ok();
    }

    /// An address not up yet (at logon, before the network): said, tried
    /// again every `LOOK_AGAIN`, served once it can be; a `web-stop` while
    /// it waits ends the waiting and the keeping, a `web-start` the waiting.
    #[tokio::test]
    async fn a_start_on_restore_that_cannot_be_made_is_tried_again() {
        let mut s = server();
        // 192.0.2.1: an address kept for documentation, on no machine.
        let away = Options { bind: vec![IpAddr::from([192, 0, 2, 1])], ..here() };
        crate::web::keep_on(&s.web_on, &away).unwrap();
        let said = text(s.web_resume());
        assert!(
            said.starts_with(
                "off: to be served again (web-start -p 0 -b 192.0.2.1), tried every 10 s: web: listen on 192.0.2.1:0"
            ),
            "{said}"
        );
        assert!(s.messages.iter().any(|m| m.contains("tried again every 10 s")));
        assert!(crate::web::status_url(&said).is_none(), "keepane web reads no address from it");
        // Not due yet: not tried.
        s.web_retry.as_mut().unwrap().options = here();
        s.web_look_again();
        assert!(s.web.is_none());
        // Due, and the address up now (here, the loopback stands in): served.
        s.web_retry.as_mut().unwrap().next = Instant::now();
        s.web_look_again();
        assert!(s.web.is_some() && s.web_retry.is_none(), "{}", s.web_status());
        assert!(s.web_status().starts_with("serving http://127.0.0.1:"));
        text(s.web_stop(None));

        // Stopped while it waits: nothing more is tried, nothing kept.
        crate::web::keep_on(&s.web_on, &away).unwrap();
        text(s.web_resume());
        assert!(s.web_retry.is_some());
        assert_eq!(text(s.web_stop(None)), "ok");
        assert!(s.web_retry.is_none() && !s.web_on.exists());
        assert_eq!(s.web_status(), "off: `keepane web` starts it");
        assert_eq!(text(s.web_stop(None)), "error: web: not running");

        // Started by hand while it waits: that one is served, the wait ends.
        crate::web::keep_on(&s.web_on, &away).unwrap();
        text(s.web_resume());
        text(s.web_start(here()));
        assert!(s.web_retry.is_none() && s.web.is_some());
        assert_eq!(crate::web::kept_on(&s.web_on), Some(here()));
        text(s.web_stop(None));
    }

    /// Served on this machine's addresses (no `--bind`): looked at again
    /// every `LOOK_AGAIN`, served on what they are now, the address to scan
    /// the first of them; nothing said when nothing moved.
    #[tokio::test]
    async fn it_follows_the_addresses_of_this_machine() {
        let mut s = server();
        text(s.web_start(here()));
        // As though served with no --bind while only the loopback was up.
        s.web.as_mut().unwrap().options.bind.clear();
        s.web_look_again();
        assert_eq!(addrs(&s), vec![IpAddr::from([127, 0, 0, 1])], "not due yet: as it was");
        s.web.as_mut().unwrap().looked = Instant::now() - LOOK_AGAIN;
        let want = crate::link::listen_addrs(&[]);
        let moved = |s: &Server| s.messages.iter().filter(|m| m.contains("web: now serving on")).count();
        s.web_look_again();
        let w = s.web.as_ref().unwrap();
        // Each address there now, unless its port was taken there.
        let now = addrs(&s);
        assert!(
            now.iter().all(|ip| want.contains(ip)) && now.len() + w.unbound.len() == want.len(),
            "{now:?} {want:?}"
        );
        assert!(w.url.starts_with(&format!("http://{}/", crate::link::hostport(now[0], w.port))), "{}", w.url);
        let said = usize::from(now != [IpAddr::from([127, 0, 0, 1])]);
        assert_eq!(moved(&s), said, "{:?}", s.messages);
        // Nothing moved: nothing said.
        s.web.as_mut().unwrap().looked = Instant::now() - LOOK_AGAIN;
        s.web_look_again();
        assert_eq!(moved(&s), said);
        assert_eq!(addrs(&s), now);
    }

    /// The system listing the Tailscale addresses in another order moves
    /// nothing; another first address (the one scanned) does.
    #[test]
    fn addresses_are_the_same_in_any_order_after_the_first() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let (lan, t1, t2) = (ip("192.168.1.5"), ip("100.64.0.3"), ip("fd7a:115c:a1e0::3"));
        assert!(same_addrs(&[lan, t1, t2], &[lan, t2, t1]));
        assert!(!same_addrs(&[lan, t1], &[t1, lan]), "another one scanned");
        assert!(!same_addrs(&[lan, t1], &[lan]));
        assert!(!same_addrs(&[lan], &[ip("127.0.0.1")]));
        assert!(same_addrs(&[], &[]));
    }

    fn addrs(s: &Server) -> Vec<IpAddr> {
        s.web.as_ref().unwrap().served.iter().map(|(ip, _)| *ip).collect()
    }

    /// Asked by a client, `web-stop` answers only once the task has ended:
    /// by then the port is free for the next `keepane web`.
    #[tokio::test]
    async fn stopping_answers_once_the_port_is_free() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        let url = crate::web::status_url(&text(s.web_start(here()))).unwrap().to_string();
        let port: u16 = url.split(':').nth(2).unwrap().split('/').next().unwrap().parse().unwrap();
        assert!(matches!(s.web_stop(Some(7)), Outcome::Pending), "answered before the task ended");
        let stopped = loop {
            match rx.recv().await {
                Some(Event::WebStopped(cid)) => break cid,
                Some(_) => continue,
                None => panic!("no answer"),
            }
        };
        assert_eq!(stopped, 7);
        // Nothing listens there any more. Checked a while rather than once:
        // a pane forked by another test at that moment holds a copy of every
        // open socket until it execs, ours included, so the kernel may keep
        // the port a moment after this process let go of it.
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(Instant::now() < deadline, "still listening once stopped");
            std::thread::sleep(Duration::from_millis(20));
        }
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
