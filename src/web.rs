//! `keepane web`: the panes on a phone, over the local network.
//!
//! HTTP on the LAN: a page that lists every pane, shows one as it is on the
//! screen (colours included), and sends what is typed on the phone to it.
//! Everything runs on this machine; the phone only shows and types. The
//! server hosts it (`web-start`, `web-status`, `web-stop`, `web-resume`), so
//! it runs in the background for as long as the server does (and again when
//! the sessions are restored, until stopped); each request is still asked
//! of the server as a client would ask it, with the same commands the CLI
//! sends. `keepane web` starts it and prints the code to scan.
//!
//! Off unless started. Every request but the page itself needs the key: 128
//! random bits, made anew each start (or kept, with `--keep-key`), handed to
//! the phone in the address a QR code in the terminal carries, after the `#`
//! so that it is never part of a request line. The phone can only look,
//! type into a pane, and run the few fixed actions of the page (new
//! window, split, close a pane; rename a session, window or pane): no
//! command of its own reaches the server. Plain HTTP, so for a network you trust; over anything else, a
//! private network such as Tailscale in between.

use anyhow::{Context, Result, bail};
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const DEFAULT_PORT: u16 = 7681;
/// Request line and headers together.
const MAX_HEAD: usize = 16 * 1024;
/// What can be typed in one go.
const MAX_BODY: usize = 64 * 1024;
/// Scrollback a screen request may ask for, in lines.
const MAX_HISTORY: u32 = 2000;
/// A message from another machine (`/link/`): `message-max-size` goes up
/// to 1 MB, plus its envelope.
const MAX_LINK_BODY: usize = 1024 * 1024 + 4096;
/// A connection that has not sent its request by then is dropped.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// The page, built from web/ (`npm run build` there), and the same gzipped
/// for a browser that takes it.
const PAGE: &str = include_str!("../web/dist/index.html");
const PAGE_GZ: &[u8] = include_bytes!("../web/dist/index.html.gz");
const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#1a1b26"/><rect x="10" y="12" width="44" height="40" rx="5" fill="none" stroke="#7aa2f7" stroke-width="4"/><path d="M32 12v40M32 32h22" stroke="#7aa2f7" stroke-width="4"/><path d="M16 22l6 5-6 5" fill="none" stroke="#9ece6a" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const MANIFEST: &str = r##"{"name":"keepane","short_name":"keepane","start_url":"/","display":"standalone","background_color":"#1a1b26","theme_color":"#16161e","icons":[{"src":"/icon.svg","sizes":"any","type":"image/svg+xml"}]}"##;

/// Named keys the page's buttons send; anything else is typed as text.
const KEYS: &[&str] = &[
    "Enter",
    "Escape",
    "Tab",
    "BTab",
    "BSpace",
    "Space",
    "Up",
    "Down",
    "Left",
    "Right",
    "Home",
    "End",
    "PPage",
    "NPage",
    "DC",
    "WheelUp",
    "WheelDown",
];

/// What the page's ⋯ menu and its names can do, and nothing else.
const ACTIONS: &[&str] = &[
    "new-window",
    "split-h",
    "split-v",
    "kill-pane",
    "rename-session",
    "rename-window",
    "rename-pane",
    "mode",
    "respawn",
];

/// How `web-start` serves: its flags (`keepane web` passes its own on).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// None: `DEFAULT_PORT`; 0, any free one.
    pub port: Option<u16>,
    /// Empty: the address this machine reaches the network through, and
    /// its Tailscale ones. Given: those, every one (a machine with more
    /// than one network card).
    pub bind: Vec<IpAddr>,
    pub read_only: bool,
    pub keep_key: bool,
}

/// Addresses from `-b`'s value (commas between them), each once.
pub fn add_binds(to: &mut Vec<IpAddr>, value: &str) -> Result<(), String> {
    for v in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
        let ip: IpAddr = v.parse().map_err(|_| format!("not an IP address: {v}"))?;
        if !to.contains(&ip) {
            to.push(ip);
        }
    }
    Ok(())
}

impl Options {
    /// The flags, long or short, one value after each of `-p` and `-b`;
    /// `-b` again, or with commas, for more addresses.
    pub fn parse(args: &[&str]) -> Result<Options, String> {
        let mut o = Options::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match *a {
                "-p" | "--port" => {
                    let v = it.next().ok_or("--port: a port number")?;
                    o.port = Some(v.parse().map_err(|_| format!("--port: not a port: {v}"))?);
                }
                "-b" | "--bind" => {
                    let v = it.next().ok_or("--bind: an address of this machine")?;
                    add_binds(&mut o.bind, v).map_err(|e| format!("--bind: {e}"))?;
                }
                "-r" | "--read-only" => o.read_only = true,
                "-k" | "--keep-key" => o.keep_key = true,
                other => {
                    return Err(format!(
                        "web: unknown argument '{other}' (--port N, --bind IP, --read-only, --keep-key; or status, stop)"
                    ));
                }
            }
        }
        Ok(o)
    }

    /// As `web-start`'s flags, the way `parse` reads them back.
    pub fn flags(&self) -> Vec<String> {
        let mut f = Vec::new();
        if let Some(p) = self.port {
            f.extend(["-p".to_string(), p.to_string()]);
        }
        if !self.bind.is_empty() {
            let all: Vec<String> = self.bind.iter().map(IpAddr::to_string).collect();
            f.extend(["-b".to_string(), all.join(",")]);
        }
        if self.read_only {
            f.push("-r".into());
        }
        if self.keep_key {
            f.push("-k".into());
        }
        f
    }
}

/// What a request tells the server about who is asking, for `web-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// A request for anything behind the key, with the key (`ok`) or not.
    Asked { peer: IpAddr, ok: bool },
    /// A pane's screen streamed to `peer` from now (`open`) or no longer.
    Watching { peer: IpAddr, pane: String, open: bool },
}

/// Everything a request handler needs.
pub struct State {
    pub socket: String,
    pub key: String,
    pub read_only: bool,
    /// Told who asks and what they watch (the server, to list the phones).
    notify: Option<Box<dyn Fn(Seen) + Send + Sync>>,
    /// Handed a link stream's first handshake message (the server, which
    /// holds the link key); none: streams are not served.
    stream: Option<Box<dyn Fn(crate::server::link_feed::StreamAsk) + Send + Sync>>,
}

impl State {
    pub fn new(socket: &str, key: &str, read_only: bool) -> State {
        State { socket: socket.into(), key: key.into(), read_only, notify: None, stream: None }
    }

    pub fn notify(mut self, f: impl Fn(Seen) + Send + Sync + 'static) -> State {
        self.notify = Some(Box::new(f));
        self
    }

    pub fn stream(mut self, f: impl Fn(crate::server::link_feed::StreamAsk) + Send + Sync + 'static) -> State {
        self.stream = Some(Box::new(f));
        self
    }

    fn tell(&self, seen: Seen) {
        if let Some(f) = &self.notify {
            f(seen);
        }
    }
}

/// `keepane web [--port N] [--bind IP] [--read-only] [--keep-key]`: have the
/// server serve (or keep serving) and print the code to scan;
/// `keepane web status` and `keepane web stop`.
pub async fn run(socket: &str, args: &[String]) -> Result<i32> {
    let ask = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        // As the pane it runs in, if it runs in one, as every command does.
        let (code, out, mut err) = crate::client::query_as(socket, &argv, crate::client::own_pane(socket)).await?;
        // A server from before 0.20 (still running after an upgrade) does
        // not know these commands: say why, and what to do.
        if code != 0
            && err.contains("unknown command: web-")
            && let Some(v) = crate::client::server_version(socket).await
            && v != env!("CARGO_PKG_VERSION")
        {
            err.push_str(&format!("note: {}\n", crate::client::mismatch_note(&v)));
        }
        anyhow::Ok((code, out, err))
    };
    let say = |(code, out, err): (i32, String, String)| {
        print!("{out}");
        eprint!("{err}");
        code
    };
    let running = crate::client::server_running(&crate::ipc::pipe_name(socket));
    if let Some(sub @ ("status" | "stop")) = args.first().map(String::as_str) {
        if args.len() > 1 {
            bail!("web {sub}: takes nothing more");
        }
        if !running {
            // Stopped with no server: not served again when one restores.
            let note = if sub == "stop" && forget_on(&on_path(socket)) {
                "; it will not be served again on restore"
            } else {
                ""
            };
            println!("off: no keepane server is running (socket '{socket}'){note}");
            return Ok(if sub == "stop" { 1 } else { 0 });
        }
        let (code, out, err) = ask(vec![format!("web-{sub}")]).await?;
        let out = if sub == "stop" && code == 0 { "stopped\n".into() } else { out };
        return Ok(say((code, out, err)));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let o = Options::parse(&args).map_err(anyhow::Error::msg)?;
    if !running {
        bail!("no keepane server is running (socket '{socket}'): start a session first, then `keepane web`");
    }
    let (code, status, err) = ask([vec!["web-start".to_string()], o.flags()].concat()).await?;
    if code != 0 {
        return Ok(say((code, status, err)));
    }
    let Some(url) = status_url(&status) else { bail!("web-start said: {status}") };
    println!("{}", qr_text(url)?);
    println!("Scan with the phone's camera, or open: {url}");
    // Only when this machine's address was looked for and not found: one
    // bound on purpose (`--bind 127.0.0.1`) needs no word.
    if o.bind.is_empty() && (url.starts_with("http://127.") || url.starts_with("http://[::1]")) {
        println!("(No network address found: this works on this machine only; --bind picks one.)");
    }
    println!(
        "Anyone with this code can {} your panes.",
        if status.lines().next().unwrap_or("").contains(" · read-only") { "see" } else { "see and type into" }
    );
    println!("It runs in the background: `keepane web status` shows who is connected, `keepane web stop` ends it.");
    println!("Windows may ask to let keepane onto the network: allow it for private networks.");
    Ok(0)
}

/// The address `web-status` (or `web-start`) gives on its first line, when
/// serving: `serving <url> · ...`.
pub fn status_url(status: &str) -> Option<&str> {
    let first = status.lines().next()?.strip_prefix("serving ")?;
    Some(first.split(" · ").next().unwrap_or(first))
}

/// Answer HTTP on `listener` until the task is dropped: the connections go
/// with it (a watching phone included).
pub async fn serve(listener: TcpListener, state: Arc<State>) {
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            r = listener.accept() => match r {
                Ok((stream, peer)) => {
                    let state = state.clone();
                    conns.spawn(async move {
                        let _ = connection(stream, peer.ip(), &state).await;
                    });
                }
                // Out of handles, a connection reset before it was taken:
                // the next one may do; a moment's pause, not a spin.
                Err(e) => {
                    log::warn!("web: accept: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
}

/// The address the phone should use: the one this machine reaches the
/// network through. Nothing is sent: connecting a UDP socket only picks the
/// route.
pub fn lan_ip() -> IpAddr {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:53")?;
            s.local_addr()
        })
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// 128 random bits from the system's generator, URL-safe.
pub fn new_key() -> Result<String> {
    let mut bytes = [0u8; 16];
    crate::platform::random::fill(&mut bytes)?;
    Ok(base64url(&bytes))
}

/// The key kept from last time (`--keep-key`), or a new one kept from now.
pub fn kept_key() -> Result<String> {
    let dir = dirs::data_local_dir().context("no local app data folder")?.join("keepane");
    let path = dir.join("web.key");
    if let Ok(k) = std::fs::read_to_string(&path) {
        let k = k.trim();
        if k.len() == 22 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Ok(k.to_string());
        }
    }
    let k = new_key()?;
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(&path, &k).with_context(|| format!("keep the key in {}", path.display()))?;
    Ok(k)
}

/// Where a server keeps how its page is served, from a `web-start` until a
/// `web-stop`: it is served the same way again when the sessions are
/// restored (`keepane startup`, `restart-server`).
pub fn on_path(socket: &str) -> std::path::PathBuf {
    crate::link::dir(socket).join("web")
}

/// How the page was served when the server ended, when it was not stopped.
pub fn kept_on(path: &std::path::Path) -> Option<Options> {
    let text = std::fs::read_to_string(path).ok()?;
    Options::parse(&text.split_whitespace().collect::<Vec<_>>()).ok()
}

pub fn keep_on(path: &std::path::Path, o: &Options) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(path, o.flags().join(" ") + "\n").with_context(|| format!("write {}", path.display()))
}

/// Not served again; whether it would have been.
pub fn forget_on(path: &std::path::Path) -> bool {
    std::fs::remove_file(path).is_ok()
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// The QR code as text, light on dark (a terminal's usual colours), two
/// rows of modules per line.
pub fn qr_text(data: &str) -> Result<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(data.as_bytes()).context("make the QR code")?;
    Ok(code.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).quiet_zone(true).build())
}

/// Compare without stopping at the first difference.
pub fn same_key(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub key: Option<String>,
    /// The `x-keepane-*` headers (names in lower case): what another
    /// machine's request carries (docs/design/link.md).
    pub headers: Vec<(String, String)>,
    /// The browser takes a gzip body (`Accept-Encoding: gzip`).
    pub gzip: bool,
    /// What of a shared file a download carried on asks for (`Range`).
    pub range: Option<String>,
    pub body: Vec<u8>,
    /// An upload's (`POST /api/upload`) whole length (`Content-Length`):
    /// `body` holds only what came with the head, the rest is read as it is
    /// written to its file. 0 for any other request (its body is whole).
    pub length: usize,
    /// It said how long its body is (`Content-Length`): an upload must, or
    /// what it sends could not be told from nothing.
    pub sized: bool,
}

impl Request {
    fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Read one request: None when the peer closed first, Err for one that is
/// too large or not HTTP.
pub async fn read_request<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if i > MAX_HEAD {
                bail!("request head too large");
            }
            break i;
        }
        if buf.len() > MAX_HEAD {
            bail!("request head too large");
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).context("request head is not text")?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = match (first.next(), first.next()) {
        (Some(m), Some(t)) if !m.is_empty() && t.starts_with('/') => (m.to_string(), t),
        _ => bail!("not an HTTP request"),
    };
    let (mut length, mut key, mut headers, mut gzip, mut range) = (0usize, None, Vec::new(), false, None);
    let mut sized = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().context("bad Content-Length")?;
            sized = true;
        } else if name.eq_ignore_ascii_case("accept-encoding") {
            gzip = value.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")));
        } else if name.eq_ignore_ascii_case("range") {
            range = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("x-keepane-key") {
            key = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("upgrade") {
            // What a link stream asks for (`crate::link_stream`).
            headers.push(("upgrade".to_string(), value.to_string()));
        } else if name.len() > 10 && name[..10].eq_ignore_ascii_case("x-keepane-") {
            headers.push((name.to_ascii_lowercase(), value.to_string()));
        }
    }
    // A file sent in (`/api/upload`): its body is read as it is written,
    // never whole; its limit is the server's (`web-upload-max`).
    let upload = method == "POST" && (target == "/api/upload" || target.starts_with("/api/upload?"));
    if !upload && length > if target.starts_with("/link/") { MAX_LINK_BODY } else { MAX_BODY } {
        bail!("request body too large");
    }
    let mut body = buf[head_end + 4..].to_vec();
    while !upload && body.len() < length {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            bail!("body cut short");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    let length = if upload { length } else { 0 };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    Ok(Some(Request { method, path: percent_decode(path), query, key, headers, gzip, range, body, length, sized }))
}

/// `%XX` escapes and `+` for a space, as a browser writes a query; a `%`
/// not followed by two hex digits stays as it is.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    /// Beyond the usual ones: what another machine reads (`/link/`).
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response { status, content_type, headers: Vec::new(), body: body.into() }
    }
    fn text(status: u16, body: &str) -> Response {
        Response::new(status, "text/plain; charset=utf-8", body)
    }
    fn json(body: String) -> Response {
        Response::new(200, "application/json; charset=utf-8", body)
    }
}

async fn connection(mut stream: TcpStream, peer: IpAddr, state: &State) -> Result<()> {
    let req = match tokio::time::timeout(READ_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) | Err(_) => return Ok(()),
        Ok(Err(e)) => {
            write_response(&mut stream, &Response::text(400, &e.to_string())).await?;
            return Ok(());
        }
    };
    // A file sent in for a pane: its body read as it is written.
    if req.method == "POST" && req.path == "/api/upload" {
        let (r, read) = upload(&mut stream, &req, peer, state).await;
        // Refused before its body was read: what is left of a small one is
        // read first, else closing on it unread resets the connection and
        // the browser never sees why (a large one: it does not, the page
        // asks `upload-info` first).
        let left = req.length.saturating_sub(req.body.len());
        if !read && left > 0 && left <= DRAIN_MAX {
            let mut sink = vec![0u8; 16 * 1024];
            let mut left = left;
            while left > 0 {
                match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut sink[..left.min(16 * 1024)])).await
                {
                    Ok(Ok(n)) if n > 0 => left -= n,
                    _ => break,
                }
            }
        }
        return write_response(&mut stream, &r).await;
    }
    // The one answer that is not a single response: a pane's screen, sent
    // again each time it changes, for as long as the phone keeps looking.
    if req.method == "GET" && req.path == "/api/watch" {
        if let Some(refused) = check_key(&req, peer, state) {
            return write_response(&mut stream, &refused).await;
        }
        let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
            return write_response(&mut stream, &Response::text(400, "pane: %N")).await;
        };
        let history = req.param("history").and_then(|h| h.parse().ok()).unwrap_or(0).min(MAX_HISTORY);
        // Told when it ends however it ends, `web-stop` dropping it included.
        struct Watching<'a>(&'a State, IpAddr, String);
        impl Drop for Watching<'_> {
            fn drop(&mut self) {
                self.0.tell(Seen::Watching { peer: self.1, pane: std::mem::take(&mut self.2), open: false });
            }
        }
        state.tell(Seen::Watching { peer, pane: pane.to_string(), open: true });
        let _watching = Watching(state, peer, pane.to_string());
        return watch(&mut stream, state, pane, history, req.param("join") == Some("1")).await;
    }
    // Another machine keeping a copy of this one's sessions: the connection
    // is a link stream from here on (`crate::link_stream`).
    if req.method == "GET" && req.path == crate::link_stream::PATH {
        return link_stream(stream, peer, state, &req).await;
    }
    // A file or a folder shared (`keepane share`): its own token, not the
    // key; sent as it is read.
    if req.path.starts_with("/s/") {
        return crate::share::serve(&mut stream, &req.method, &req.path, req.range.as_deref(), peer, &state.socket)
            .await;
    }
    let resp = handle(&req, peer, state).await;
    write_response(&mut stream, &resp).await
}

/// How long the other machine has to send its first handshake message, and
/// the server to answer it.
const STREAM_HANDSHAKE: Duration = Duration::from_secs(10);
/// A stream that sent nothing for this long says it is still there (`Ping`);
/// one that heard nothing for `STREAM_SILENCE` is taken as gone.
const STREAM_PING: Duration = Duration::from_secs(15);
const STREAM_SILENCE: Duration = Duration::from_secs(45);

/// A link stream, the machine copied's end (`crate::link_stream`,
/// `crate::server::link_feed`): the handshake goes to the server, which
/// holds the key; then the feed's changes go out, as protobuf, encrypted.
async fn link_stream(mut stream: TcpStream, peer: IpAddr, state: &State, req: &Request) -> Result<()> {
    use crate::link_proto::{Ping, ToClient, ToServer, to_client, to_server};
    use crate::server::link_feed::{FeedItem, StreamAsk};
    let upgrade = req
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("upgrade") && v.eq_ignore_ascii_case(crate::link_stream::UPGRADE));
    if !upgrade {
        return write_response(&mut stream, &Response::text(400, "a link stream is an upgrade to keepane-stream"))
            .await;
    }
    let Some(ask_server) = &state.stream else {
        return write_response(
            &mut stream,
            &Response::text(503, "link streams are served by the server's own web (`keepane web`)"),
        )
        .await;
    };
    stream.write_all(crate::link_stream::SWITCHING.as_bytes()).await?;
    let msg1 = tokio::time::timeout(STREAM_HANDSHAKE, crate::link_stream::read_frame(&mut stream)).await??;
    let (tx, rx) = tokio::sync::oneshot::channel();
    ask_server(StreamAsk { peer, msg1, reply: tx });
    // No answer (a first message that did not check out): the connection
    // just ends.
    let Ok(Ok(reply)) = tokio::time::timeout(STREAM_HANDSHAKE, rx).await else { return Ok(()) };
    crate::link_stream::write_frame(&mut stream, &reply.msg2).await?;
    let Some((keys, mut feed)) = reply.open else { return Ok(()) };
    let (sealer, mut opener) = crate::link_stream::split(keys);
    let (mut rd, mut wr) = stream.into_split();
    // Frames read on a task of their own: a read cut short by the select
    // below would lose the frame's start.
    let (frames_tx, mut frames) = tokio::sync::mpsc::channel::<Result<Vec<u8>>>(16);
    let reader = tokio::spawn(async move {
        loop {
            let f = crate::link_stream::read_frame(&mut rd).await;
            let end = f.is_err();
            if frames_tx.send(f).await.is_err() || end {
                break;
            }
        }
    });
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let (mut sent, mut heard) = (std::time::Instant::now(), std::time::Instant::now());
    let result: Result<()> = async {
        loop {
            tokio::select! {
                item = feed.rx.recv() => {
                    let Some(item) = item else { return Ok(()) };
                    feed.queued.fetch_sub(item.cost(), std::sync::atomic::Ordering::Relaxed);
                    let kind = match &item {
                        FeedItem::Entry(e) => to_client::Kind::Entry(e.into()),
                        FeedItem::Snapshot(s) => to_client::Kind::Snapshot(s.into()),
                        FeedItem::Reply(r) => to_client::Kind::Reply(r.clone()),
                        FeedItem::End(why) => {
                            log::info!("link stream to {peer} ends: {why}");
                            return Ok(());
                        }
                    };
                    sealer.send(&mut wr, &ToClient { kind: Some(kind) }).await?;
                    sent = std::time::Instant::now();
                }
                frame = frames.recv() => {
                    let Some(frame) = frame else { return Ok(()) };
                    // That it is there, and what is typed into its copy.
                    for msg in opener.open::<ToServer>(&frame?)? {
                        match msg.kind {
                            Some(to_server::Kind::Input(i)) => (feed.input)(i.pane, i.bytes),
                            Some(to_server::Kind::Command(c)) => (feed.command)(c.id, c.line),
                            _ => {}
                        }
                    }
                    heard = std::time::Instant::now();
                }
                _ = tick.tick() => {
                    if heard.elapsed() > STREAM_SILENCE {
                        anyhow::bail!("nothing heard for {} s", STREAM_SILENCE.as_secs());
                    }
                    if sent.elapsed() > STREAM_PING {
                        sealer.send(&mut wr, &ToClient { kind: Some(to_client::Kind::Ping(Ping {})) }).await?;
                        sent = std::time::Instant::now();
                    }
                }
            }
        }
    }
    .await;
    reader.abort();
    if let Err(e) = &result {
        log::info!("link stream to {peer}: {e:#}");
    }
    Ok(())
}

/// How often a watched pane is asked whether it changed. The question is a
/// few bytes over the local pipe; the screen is read only when the answer
/// changes.
const WATCH_EVERY: Duration = Duration::from_millis(100);
/// A comment line now and then, so a phone that went away is noticed (the
/// write fails) and a proxy does not close a quiet connection.
const WATCH_PING: Duration = Duration::from_secs(15);
/// The phone opens a new stream after this; nothing lives forever.
const WATCH_LONGEST: Duration = Duration::from_secs(30 * 60);

/// Stream a pane's screen as server-sent events: `data: {"text":...}`
/// whenever it may have changed (its output count or size moved), a
/// comment to keep the line alive, and `event: gone` when the pane is.
async fn watch(stream: &mut TcpStream, state: &State, pane: &str, history: u32, join: bool) -> Result<()> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-store\r\n\
                X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    let query = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        crate::client::query(&state.socket, &argv).await
    };
    let started = std::time::Instant::now();
    let mut last_stamp = String::new();
    let mut last_write = std::time::Instant::now();
    while started.elapsed() < WATCH_LONGEST {
        let stamp = match query(vec![
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            pane.into(),
            "#{pane_output_count} #{pane_width}x#{pane_height} #{pane_dead}".into(),
        ])
        .await
        {
            Ok((0, s, _)) => s,
            // The pane (or the server) is gone: say so and end.
            _ => {
                stream.write_all(b"event: gone\ndata: {}\n\n").await.ok();
                break;
            }
        };
        if stamp != last_stamp {
            if let Ok(json) = screen_json(&state.socket, pane, history, join).await {
                let event = format!("data: {json}\n\n");
                if stream.write_all(event.as_bytes()).await.is_err() {
                    return Ok(()); // the phone went away
                }
                last_stamp = stamp;
                last_write = std::time::Instant::now();
            }
        } else if last_write.elapsed() >= WATCH_PING {
            if stream.write_all(b": ping\n\n").await.is_err() {
                return Ok(());
            }
            last_write = std::time::Instant::now();
        }
        // Wait for the next look, and meanwhile notice at once when the phone
        // closes the connection (it sends nothing more, so a read ending is
        // that), rather than at the next write, up to WATCH_PING later.
        let mut probe = [0u8; 64];
        tokio::select! {
            _ = tokio::time::sleep(WATCH_EVERY) => {}
            r = stream.read(&mut probe) => {
                if matches!(r, Ok(0) | Err(_)) {
                    return Ok(());
                }
            }
        }
    }
    stream.shutdown().await.ok();
    Ok(())
}

/// A pane's screen as the page reads it: `{"text": ..., "marks": [...]}`,
/// the text `capture-pane -e` gives (the last `history` lines of the
/// scrollback first) and, for each command the pane's shell ran on one of
/// those lines, `[line, start, end, exit, last, stop, command]` (`list-marks
/// -J`, its times in Unix milliseconds, null where unknown): what was typed
/// runs to line `last`, what it printed from there to before line `stop`.
/// `join`: lines the pane wrapped come as one (`capture-pane -J`), for a
/// phone that wraps them at its own width; each mark then goes on the
/// joined line its row is part of.
async fn screen_json(socket: &str, pane: &str, history: u32, join: bool) -> Result<String, (u16, String)> {
    let q = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        crate::client::query(socket, &argv).await
    };
    let capture = |join: bool| {
        let mut argv = vec!["capture-pane".into(), "-p".into(), "-e".into(), "-t".into(), pane.to_string()];
        if history > 0 {
            argv.extend(["-S".into(), format!("-{history}")]);
        }
        if join {
            argv.push("-J".into());
        }
        async move {
            match q(argv).await {
                Ok((0, out, _)) => Ok(out),
                Ok((_, _, err)) => Err((404, err.trim().to_string())),
                Err(e) => Err((500, format!("{e:#}"))),
            }
        }
    };
    // The rows as the pane has them: the marks are placed on those.
    let rows = capture(false).await?;
    let text = if join { capture(true).await? } else { rows.clone() };
    let marks = q(vec!["list-marks".into(), "-J".into(), "-t".into(), pane.into()]).await;
    let size = q(vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        pane.into(),
        "#{history_size} #{pane_width}".into(),
    ])
    .await;
    let marks = match (marks, size) {
        (Ok((0, marks, _)), Ok((0, size, _))) => {
            let mut size = size.split_whitespace().map(|n| n.parse::<usize>().unwrap_or(0));
            let (scrollback, cols) = (size.next().unwrap_or(0), size.next().unwrap_or(0));
            let placed = marks_placed(&rows, &marks, history, scrollback);
            if join {
                // Row to joined line; a count that does not add up (output
                // arrived between the captures) places none.
                match joined_rows(&text, rows.split('\n').count(), cols) {
                    Some(line_of) => marks_list(on_joined(placed, &line_of, text.split('\n').count())),
                    None => "[]".into(),
                }
            } else {
                marks_list(placed)
            }
        }
        _ => "[]".into(),
    };
    Ok(format!("{{\"text\":{},\"marks\":{marks}}}", json_str(&text)))
}

/// For each row of a capture, the line of the joined capture (`-J`) it is
/// part of: a joined line of width W took ⌈W / cols⌉ rows (one at least).
/// None when the rows do not add up to `rows`.
fn joined_rows(joined: &str, rows: usize, cols: usize) -> Option<Vec<usize>> {
    use unicode_width::UnicodeWidthStr;
    if cols == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(rows);
    for (j, line) in joined.split('\n').enumerate() {
        let width = without_escapes(line).trim_end().width();
        out.extend(std::iter::repeat_n(j, width.div_ceil(cols).max(1)));
    }
    (out.len() == rows).then_some(out)
}

/// A command placed on the lines of a capture: the line it was typed on,
/// the last line of what was typed, the line what it printed stops before,
/// its times and exit code (`start,end,exit`, JSON), and the command.
#[derive(Debug, PartialEq)]
struct Placed {
    line: usize,
    last: usize,
    stop: usize,
    times: String,
    command: String,
}

/// `list-marks -J` placed on the lines of `text`, a capture holding the
/// last `history` of `scrollback` lines above the screen. A mark whose line
/// does not read as it did (output arrived between the two questions) is
/// left out rather than put against the wrong line.
fn marks_placed(text: &str, marks: &str, history: u32, scrollback: usize) -> Vec<Placed> {
    let lines: Vec<&str> = text.split('\n').collect();
    let above = i64::from(history).min(scrollback as i64);
    let Ok(serde_json::Value::Array(marks)) = serde_json::from_str::<serde_json::Value>(marks) else {
        return Vec::new();
    };
    let num = |v: &serde_json::Value| v.as_i64().map_or("null".to_string(), |n| n.to_string());
    let at = |v: &serde_json::Value| v.as_i64().map(|r| r + above).filter(|i| *i >= 0).map(|i| i as usize);
    let mut out = Vec::new();
    for m in &marks {
        let (Some(line), Some(last), Some(stop)) = (at(&m["row"]), at(&m["last"]), at(&m["stop"])) else { continue };
        let said = m["text"].as_str().unwrap_or("");
        if lines.get(line).is_some_and(|l| without_escapes(l).trim_end() == said) {
            let last = last.clamp(line, lines.len() - 1);
            out.push(Placed {
                line,
                last,
                stop: stop.clamp(last + 1, lines.len().max(last + 1)),
                times: format!("{},{},{}", num(&m["start"]), num(&m["end"]), num(&m["exit"])),
                command: m["command"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    out
}

/// Marks placed on rows, moved onto the joined lines those rows are part of
/// (`line_of`, from `joined_rows`; `lines` joined lines in all): one mark a
/// line, and a stop past the last row stops at the end.
fn on_joined(placed: Vec<Placed>, line_of: &[usize], lines: usize) -> Vec<Placed> {
    let to = |r: usize| line_of.get(r).copied().unwrap_or(lines);
    let mut seen = std::collections::HashSet::new();
    placed
        .into_iter()
        .filter_map(|p| {
            let line = *line_of.get(p.line)?;
            seen.insert(line).then(|| Placed { line, last: to(p.last), stop: to(p.stop), ..p })
        })
        .collect()
}

/// `[[line, start, end, exit, last, stop, command], ...]`.
fn marks_list(placed: impl IntoIterator<Item = Placed>) -> String {
    let items: Vec<String> = placed
        .into_iter()
        .map(|p| format!("[{},{},{},{},{}]", p.line, p.times, p.last, p.stop, json_str(&p.command)))
        .collect();
    format!("[{}]", items.join(","))
}

/// A captured line without its colour sequences (`ESC [ ... m`).
fn without_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// None when the request carries the key; otherwise the refusal to send.
/// Either way the server hears of it (who is connected, who was refused).
fn check_key(req: &Request, peer: IpAddr, state: &State) -> Option<Response> {
    let ok = req.key.as_deref().is_some_and(|k| same_key(k, &state.key));
    state.tell(Seen::Asked { peer, ok });
    (!ok).then(|| Response::text(401, "wrong key: scan the code again"))
}

async fn write_response(stream: &mut TcpStream, r: &Response) -> Result<()> {
    let reason = match r.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n",
        r.status,
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&r.body).await?;
    stream.shutdown().await.ok();
    Ok(())
}

/// Answer one request. Public for the tests, which drive it without a
/// network.
/// The colours a page draws panes in when the theme leaves them to the
/// terminal (tokyo-night): Tokyo Night's, as the page always had.
const PAGE_FG: &str = "#c0caf5";
const PAGE_BG: &str = "#1a1b26";
const PAGE_PALETTE: [&str; 16] = [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#f7768e",
    "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
];

/// `/api/theme`: the theme's name, every name there is, and the colours a
/// page draws a pane's text in: `fg`, `bg` and the 16 of the palette, from
/// `window-style` (`fg=…,bg=…`) and `pane-colours`, the page's own where the
/// theme leaves them to the terminal.
fn theme_json(name: &str, style: &str, colours: &str) -> String {
    let part = |k: &str| {
        style.split(',').find_map(|p| p.trim().strip_prefix(k)).filter(|v| v.starts_with('#')).map(String::from)
    };
    let given: Vec<&str> = colours.split_whitespace().filter(|c| c.starts_with('#')).collect();
    let palette: Vec<&str> = if given.len() == 16 { given } else { PAGE_PALETTE.to_vec() };
    serde_json::json!({
        "name": name,
        "names": crate::config::THEME_NAMES,
        "fg": part("fg=").unwrap_or_else(|| PAGE_FG.into()),
        "bg": part("bg=").unwrap_or_else(|| PAGE_BG.into()),
        "palette": palette,
    })
    .to_string()
}

pub async fn handle(req: &Request, peer: IpAddr, state: &State) -> Response {
    let get = req.method == "GET";
    // The page and its bits carry no secret: the key comes from the address.
    match (get, req.path.as_str()) {
        (true, "/") if req.gzip => {
            let mut r = Response::new(200, "text/html; charset=utf-8", PAGE_GZ);
            r.headers = vec![("Content-Encoding", "gzip".into()), ("Vary", "Accept-Encoding".into())];
            return r;
        }
        (true, "/") => {
            let mut r = Response::new(200, "text/html; charset=utf-8", PAGE);
            r.headers = vec![("Vary", "Accept-Encoding".into())];
            return r;
        }
        (true, "/icon.svg") => return Response::new(200, "image/svg+xml", ICON),
        (true, "/manifest.webmanifest") => return Response::new(200, "application/manifest+json", MANIFEST),
        _ => {}
    }
    if req.path.starts_with("/link/") {
        return link_forward(req, peer, state).await;
    }
    if !req.path.starts_with("/api/") {
        return Response::text(404, "not found");
    }
    if let Some(refused) = check_key(req, peer, state) {
        return refused;
    }
    if get && matches!(req.path.as_str(), "/api/send" | "/api/action" | "/api/fit" | "/api/message") {
        return Response::text(405, "POST");
    }
    let q = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        crate::client::query(&state.socket, &argv).await
    };
    match (get, req.path.as_str()) {
        (true, "/api/info") => Response::json(format!(
            "{{\"readOnly\":{},\"host\":{},\"version\":{}}}",
            state.read_only,
            json_str(&crate::sysinfo::hostname()),
            json_str(env!("CARGO_PKG_VERSION"))
        )),
        (true, "/api/panes") => {
            // `tail=N`: each pane's last N lines too (`web-tails`), for the
            // grid of panes; asked together with the list.
            let tail = req
                .param("tail")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| (1..=crate::command::MAX_TAILS).contains(n));
            let list = q(vec!["list-panes".into(), "-a".into(), "-F".into(), PANE_FIELDS.into()]);
            let (list, tails) = match tail {
                Some(n) => {
                    let (list, tails) = tokio::join!(list, q(vec!["web-tails".into(), n.to_string()]));
                    // Not given: no tail (the page shows the last line).
                    let tails = match tails {
                        Ok((0, out, _)) => serde_json::from_str(out.trim()).ok(),
                        _ => None,
                    };
                    (list, tails)
                }
                None => (list.await, None),
            };
            match list {
                Ok((0, out, _)) => Response::json(panes_json(&out, tails.as_ref())),
                Ok((_, _, err)) => Response::text(500, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        // Where a file sent for this pane would go and how big it may be,
        // before it is sent; why not, for another machine's pane.
        (true, "/api/upload-info") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            if state.read_only {
                return Response::text(403, "read-only: nothing is written from here");
            }
            match upload_place(state, pane).await {
                Ok((dir, max)) => {
                    Response::json(format!("{{\"dir\":{},\"max\":{max}}}", json_str(&dir.to_string_lossy())))
                }
                Err(refused) => refused,
            }
        }
        (true, "/api/status") => {
            // A session's status line as the terminal draws it (`status-line
            // -J`): its parts, their colours, each window's label and pane.
            // Only a session's name: no window or pane after it.
            let Some(session) =
                req.param("session").filter(|s| !s.is_empty() && s.len() <= 200 && !s.contains([':', '.']))
            else {
                return Response::text(400, "session: a session's name");
            };
            match q(vec!["status-line".into(), "-J".into(), "-t".into(), session.into()]).await {
                Ok((0, out, _)) => Response::json(out.trim().to_string()),
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        (true, "/api/done") => {
            // Panes done since number `after` (`done-events`), for the page
            // to tell whoever holds it.
            let after = req.param("after").and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
            match q(vec!["list-done".into(), "-a".into(), after.to_string(), "-J".into()]).await {
                Ok((0, out, _)) => Response::json(out.trim().to_string()),
                Ok((_, _, err)) => Response::text(500, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        (true, "/api/inbox") => {
            // A pane's inbox, every message whole: what it works on, what
            // waits, what it finished lately.
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            match q(vec!["list-messages".into(), "-t".into(), pane.into(), "-J".into()]).await {
                Ok((0, out, _)) => {
                    let one = serde_json::from_str::<serde_json::Value>(out.trim())
                        .ok()
                        .and_then(|v| v.get(0).cloned())
                        .unwrap_or(serde_json::Value::Null);
                    Response::json(one.to_string())
                }
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        (true, "/api/screen") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            let history: u32 = req.param("history").and_then(|h| h.parse().ok()).unwrap_or(0).min(MAX_HISTORY);
            match screen_json(&state.socket, pane, history, req.param("join") == Some("1")).await {
                Ok(json) => Response::json(json),
                Err((status, msg)) => Response::text(status, &msg),
            }
        }
        (true, "/api/theme") => {
            let show = |o: &str| q(vec!["show-options".into(), "-gv".into(), o.into()]);
            match (show("theme").await, show("window-style").await, show("pane-colours").await) {
                (Ok((0, name, _)), Ok((0, style, _)), Ok((0, colours, _))) => {
                    Response::json(theme_json(name.trim(), style.trim(), colours.trim()))
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Response::text(500, &format!("{e:#}")),
                _ => Response::text(500, "theme: the server would not say"),
            }
        }
        (false, "/api/send")
        | (false, "/api/tell")
        | (false, "/api/new-session")
        | (false, "/api/action")
        | (false, "/api/fit")
        | (false, "/api/message")
        | (false, "/api/theme")
            if state.read_only =>
        {
            Response::text(403, "read-only")
        }
        // The terminal's theme, from the phone: `set -g theme`.
        (false, "/api/theme") => {
            let Some(name) = req.param("name").filter(|n| crate::config::THEME_NAMES.contains(n)) else {
                return Response::text(400, &format!("name: {}", crate::config::THEME_NAMES.join(", ")));
            };
            match q(vec!["set-option".into(), "-g".into(), "theme".into(), name.into()]).await {
                Ok((0, _, _)) => Response::text(200, name),
                Ok((_, _, err)) => Response::text(500, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        // The pane sized to the phone (`cols`, `rows`: what fits on its
        // screen), or back (`off`): `web-fit`.
        (false, "/api/fit") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            let mut argv: Vec<String> = vec!["web-fit".into(), "-t".into(), pane.into()];
            if req.param("off").is_some() {
                argv.push("-u".into());
            } else {
                let num = |k: &str| req.param(k).and_then(|v| v.parse::<u16>().ok()).filter(|n| (1..=1000).contains(n));
                let (Some(cols), Some(rows)) = (num("cols"), num("rows")) else {
                    return Response::text(400, "cols and rows: 1 to 1000");
                };
                argv.extend(["-x".into(), cols.to_string(), "-y".into(), rows.to_string()]);
            }
            match q(argv).await {
                Ok((0, out, _)) => Response::text(200, out.trim()),
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        // A queued message: to the top, up, down, deleted, or the last one
        // deleted brought back.
        (false, "/api/message") => {
            let id = req.param("id").filter(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()));
            let argv: Vec<String> = match (req.param("do").unwrap_or(""), id) {
                ("undo", _) => vec!["drop-message".into(), "-u".into()],
                ("drop", Some(id)) => vec!["drop-message".into(), id.into()],
                (to @ ("top" | "up" | "down"), Some(id)) => vec!["move-message".into(), id.into(), to.into()],
                _ => return Response::text(400, "do: top, up, down or drop, with id; or undo"),
            };
            match q(argv).await {
                Ok((0, out, _)) => Response::text(200, out.trim()),
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        (false, "/api/send") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            let mut argv: Vec<String> = vec!["send-keys".into(), "-t".into(), pane.into()];
            if let Some(key) = req.param("key") {
                // A named key (a button): only those the page has.
                if !is_named_key(key) {
                    return Response::text(400, "not a key the page sends");
                }
                argv.extend(["--".into(), key.into()]);
            } else {
                // Typed text, sent as it is: `--` so that text starting with
                // `-` is not read as a flag.
                let text = String::from_utf8_lossy(&req.body).into_owned();
                if text.is_empty() {
                    return Response::text(400, "nothing to send");
                }
                argv.extend(["-l".into(), "--".into(), text]);
            }
            match q(argv).await {
                Ok((0, _, _)) => Response::text(200, "sent"),
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        // A message into a pane's inbox, from the user: `send-message`, so
        // it waits its turn as any other.
        (false, "/api/tell") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            let text = String::from_utf8_lossy(&req.body).into_owned();
            if text.trim().is_empty() {
                return Response::text(400, "nothing to send");
            }
            let argv = vec!["send-message".into(), "-t".into(), pane.into(), "--".into(), text];
            match q(argv).await {
                Ok((0, out, _)) => Response::text(200, out.trim()),
                Ok((_, _, err)) => Response::text(400, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        // A session of its own, in the home directory; the name is the body
        // (empty: keepane names it).
        (false, "/api/new-session") => {
            let name = match rename_body(&req.body) {
                Ok(n) => n,
                Err(e) => return Response::text(400, e),
            };
            let mut argv: Vec<String> = vec!["new-session".into(), "-d".into()];
            if !name.is_empty() {
                argv.extend(["-s".into(), name]);
            }
            if let Some(home) = dirs::home_dir() {
                argv.extend(["-c".into(), home.to_string_lossy().into_owned()]);
            }
            match q(argv).await {
                Ok((0, _, _)) => Response::text(200, "done"),
                Ok((_, _, err)) => Response::text(400, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        (false, "/api/action") => {
            let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
                return Response::text(400, "pane: %N");
            };
            let what = req.param("do").unwrap_or("");
            let mut argv: Vec<String> = match what {
                "new-window" => vec!["new-window".into(), "-t".into(), pane.into()],
                "split-h" => vec!["split-window".into(), "-h".into(), "-t".into(), pane.into()],
                "split-v" => vec!["split-window".into(), "-v".into(), "-t".into(), pane.into()],
                "kill-pane" => vec!["kill-pane".into(), "-t".into(), pane.into()],
                // Its program again, once it has ended.
                "respawn" => vec!["respawn-pane".into(), "-t".into(), pane.into()],
                "mode" => match req.param("mode") {
                    Some(m @ ("normal" | "shell" | "ai")) => {
                        vec!["set-work-mode".into(), "-t".into(), pane.into(), m.into()]
                    }
                    _ => return Response::text(400, "mode: normal, shell or ai"),
                },
                // The pane, its session or its window, by the pane: the
                // new name is the body, after `--` so that one starting with
                // `-` is a name. keepane says what a name may be (a pane's
                // empty one takes its name away).
                "rename-session" | "rename-window" | "rename-pane" => {
                    let name = match rename_body(&req.body) {
                        Ok(n) => n,
                        Err(e) => return Response::text(400, e),
                    };
                    vec![what.into(), "-t".into(), pane.into(), "--".into(), name]
                }
                _ => return Response::text(400, &format!("do: one of {}", ACTIONS.join(", "))),
            };
            // What the user asked for that keepane will not do is theirs to change.
            let renaming = what.starts_with("rename-") || what == "mode" || what == "respawn";
            // A new pane starts where the pane it came from is, not where
            // `keepane web` was started (the directory this client would give).
            if matches!(what, "new-window" | "split-h" | "split-v")
                && let Ok((0, dir, _)) = q(vec![
                    "display-message".into(),
                    "-p".into(),
                    "-t".into(),
                    pane.into(),
                    "#{pane_current_path}".into(),
                ])
                .await
                && !dir.trim().is_empty()
            {
                argv.extend(["-c".into(), dir.trim().to_string()]);
            }
            match q(argv).await {
                Ok((0, _, _)) => Response::text(200, "done"),
                // A name keepane does not take is the phone's to change.
                Ok((_, _, err)) => Response::text(if renaming { 400 } else { 404 }, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        _ => Response::text(404, "not found"),
    }
}

/// A request from another machine's keepane (docs/design/link.md), handed
/// whole to the server (`link-inbound`), which holds the keys: it checks
/// the request and signs the answer (a read-only refusal included, so the
/// other side can tell it from an impostor's); what comes back is the
/// status, signature and body to send.
async fn link_forward(req: &Request, peer: IpAddr, state: &State) -> Response {
    let forwarded = serde_json::json!({
        "method": req.method,
        "path": req.path,
        "headers": req.headers,
        "body": String::from_utf8_lossy(&req.body),
    })
    .to_string();
    let argv = ["link-inbound", &peer.to_string(), &forwarded];
    let answer = match crate::client::query(&state.socket, &argv).await {
        Ok((0, out, _)) => out,
        Ok((_, _, err)) => return Response::text(500, err.trim()),
        Err(e) => return Response::text(500, &format!("{e:#}")),
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&answer) else {
        return Response::text(500, "the server's answer is not what keepane web sends on");
    };
    let status = v["status"].as_u64().unwrap_or(500) as u16;
    let mut r = Response::text(status, v["body"].as_str().unwrap_or_default());
    r.headers = vec![
        ("X-Keepane-Sign", v["sign"].as_str().unwrap_or_default().to_string()),
        ("X-Keepane-Link", crate::link::PROTOCOL.to_string()),
        ("X-Keepane-Version", env!("CARGO_PKG_VERSION").to_string()),
    ];
    r
}

/// The name a rename's body carries: text on one line, not too long. What
/// else a name may be is keepane's to say.
fn rename_body(body: &[u8]) -> Result<String, &'static str> {
    let name = std::str::from_utf8(body).map_err(|_| "the name is not text")?.trim();
    if name.chars().count() > 64 {
        return Err("the name is too long (64 at most)");
    }
    if name.chars().any(char::is_control) {
        return Err("the name is one line of text");
    }
    Ok(name.to_string())
}

fn is_pane_id(p: &str) -> bool {
    p.len() > 1 && p.starts_with('%') && p[1..].bytes().all(|b| b.is_ascii_digit())
}

/// How long an upload may go without a byte before it counts as cut short.
const UPLOAD_IDLE: Duration = Duration::from_secs(60);

/// The most of a refused upload's body read (and dropped) before the
/// answer, so that the answer gets through.
const DRAIN_MAX: usize = 1024 * 1024;

/// A file's name as it may be written here: its last part only (`/` and
/// `\` both separate), no control characters nor those Windows refuses, no
/// dots or spaces at its end; a name Windows keeps for a device (`CON`,
/// `nul.txt`) with `_` before it. None for no name left (`..`, empty).
fn upload_name(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if last.is_empty() || last.chars().all(|c| c == '.') {
        return None;
    }
    let mut name = crate::histlog::safe_name(last);
    if name.chars().all(|c| c == '_' || c == '.') {
        return None;
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        name.insert(0, '_');
    }
    // A name every file system takes (Linux: 255 bytes; ` (12)` may come):
    // 200 bytes at most, cut at a character, its extension kept when short.
    const MOST: usize = 200;
    if name.len() > MOST {
        let ext = name.rfind('.').map(|i| &name[i..]).filter(|e| e.len() <= 16).unwrap_or("");
        let mut stem = String::new();
        for c in name[..name.len() - ext.len()].chars() {
            if stem.len() + c.len_utf8() + ext.len() > MOST {
                break;
            }
            stem.push(c);
        }
        name = format!("{stem}{ext}");
    }
    Some(name)
}

/// The names to try for `name`, the first free one taken: `report.pdf`,
/// then `report (1).pdf`, `report (2).pdf`…
fn upload_names(name: &str) -> impl Iterator<Item = String> + '_ {
    // The extension: after the last dot, not a leading one (`.env` has none).
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    std::iter::once(name.to_string()).chain((1..).map(move |n| format!("{stem} ({n}){ext}")))
}

/// Where a pane's uploads go: `web-upload-dir` under its directory (or as
/// it is, when absolute), and how big one may be (`web-upload-max`). A
/// refusal for a pane of another machine, or one whose directory is not
/// known.
async fn upload_place(state: &State, pane: &str) -> Result<(std::path::PathBuf, u64), Response> {
    let s = state.socket.as_str();
    let ask_pane = ["display-message", "-p", "-t", pane, "#{pane_current_path}\t#{pane_machine}"];
    let ask_dir = ["show-options", "-gv", "web-upload-dir"];
    let ask_max = ["show-options", "-gv", "web-upload-max"];
    let (pane_info, dir, max) = tokio::join!(
        crate::client::query(s, &ask_pane),
        crate::client::query(s, &ask_dir),
        crate::client::query(s, &ask_max),
    );
    let pane_info = match pane_info {
        Ok((0, out, _)) => out,
        Ok((_, _, err)) => return Err(Response::text(404, err.trim())),
        Err(e) => return Err(Response::text(500, &format!("{e:#}"))),
    };
    let mut fields = pane_info.trim_end_matches(['\r', '\n']).split('\t');
    let (cwd, machine) = (fields.next().unwrap_or_default(), fields.next().unwrap_or_default());
    let dir = match dir {
        Ok((0, out, _)) => out,
        _ => String::new(),
    };
    let max = match max {
        Ok((0, out, _)) => out,
        _ => String::new(),
    };
    upload_dir(pane, cwd, machine, &dir, &max).map_err(|(status, why)| Response::text(status, &why))
}

/// Where `pane`'s uploads go, from its directory (`cwd`), the machine it is
/// on (`machine`: empty for this one), `web-upload-dir` and
/// `web-upload-max` as `show-options` gives them (empty: their defaults,
/// `keepane-uploads` and 2G). A refusal (its status and words) for another
/// machine's pane, or a relative folder under a directory not known.
fn upload_dir(
    pane: &str,
    cwd: &str,
    machine: &str,
    dir: &str,
    max: &str,
) -> Result<(std::path::PathBuf, u64), (u16, String)> {
    if !machine.is_empty() {
        return Err((400, format!("{pane} is a pane of {machine}: a file is written only to this machine's")));
    }
    let dir = if dir.trim().is_empty() {
        crate::config::WEB_UPLOAD_DIR.to_string()
    } else {
        crate::server::expand_home(dir.trim())
    };
    let max = crate::config::size_bytes(max.trim()).filter(|m| *m > 0).unwrap_or(2 << 30);
    let dir = std::path::PathBuf::from(dir);
    if dir.is_absolute() {
        return Ok((dir, max));
    }
    if cwd.is_empty() {
        return Err((400, format!("{pane}: its directory is not known (web-upload-dir is relative)")));
    }
    Ok((std::path::Path::new(cwd).join(dir), max))
}

/// What a folder keepane makes for uploads holds besides them: a
/// `.gitignore` that leaves the whole folder out of git (`git status`
/// stays as it was, nothing sent from a phone is committed by mistake).
const UPLOAD_GITIGNORE: &str = "# Files sent from keepane's web page (web-upload-dir): left out of git.\n*\n";

/// `dir` made ready for uploads: there already, it is left as it is; made
/// here, it gets [`UPLOAD_GITIGNORE`] (never over one there: two uploads
/// may make it at once). The `.gitignore` is a help, not the upload: one
/// that cannot be written fails nothing.
async fn upload_folder(dir: &std::path::Path) -> std::io::Result<()> {
    if tokio::fs::metadata(dir).await.is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir).await?;
    let new = tokio::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(".gitignore")).await;
    if let Ok(mut f) = new {
        use tokio::io::AsyncWriteExt;
        // Flushed: a tokio file dropped with a write still in flight may
        // leave it unwritten (an empty `.gitignore`, git seeing everything).
        let _: std::io::Result<()> = async {
            f.write_all(UPLOAD_GITIGNORE.as_bytes()).await?;
            f.flush().await
        }
        .await;
    }
    Ok(())
}

/// `POST /api/upload?pane=%N&name=…`: the body a file, written under the
/// pane's `web-upload-dir`, never over another (`name (1).ext`), read as
/// it is written; refused on a read-only page, past `web-upload-max`
/// (before a byte is read), for another machine's pane. Cut short: what
/// was written goes. The server says it on the status line.
async fn upload(stream: &mut TcpStream, req: &Request, peer: IpAddr, state: &State) -> (Response, bool) {
    let mut read = false;
    let r = upload_into(stream, req, peer, state, &mut read).await;
    (r, read)
}

/// [`upload`]'s work; `read` set once its body is being read.
async fn upload_into(stream: &mut TcpStream, req: &Request, peer: IpAddr, state: &State, read: &mut bool) -> Response {
    if let Some(refused) = check_key(req, peer, state) {
        return refused;
    }
    if state.read_only {
        return Response::text(403, "read-only: nothing is written from here");
    }
    // Its length said (a body sent in chunks has none): else an empty file
    // would stand for whatever was sent.
    if !req.sized {
        return Response::text(411, "Content-Length: a file is sent with its length");
    }
    let Some(pane) = req.param("pane").filter(|p| is_pane_id(p)) else {
        return Response::text(400, "pane: %N");
    };
    let Some(name) = req.param("name").and_then(upload_name) else {
        return Response::text(400, "name: a file's name");
    };
    let (dir, max) = match upload_place(state, pane).await {
        Ok(p) => p,
        Err(refused) => return refused,
    };
    if req.length as u64 > max {
        return Response::text(
            413,
            &format!(
                "{} is more than web-upload-max ({})",
                crate::sysinfo::human_bytes(req.length as u64),
                crate::sysinfo::human_bytes(max)
            ),
        );
    }
    if let Err(e) = upload_folder(&dir).await {
        return Response::text(500, &format!("{}: {e}", dir.display()));
    }
    // The first free name, made new (never one there, even one another
    // upload has just taken).
    let mut taken = None;
    for candidate in upload_names(&name).take(1000) {
        let path = dir.join(&candidate);
        match tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await {
            Ok(f) => {
                taken = Some((path, candidate, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Response::text(500, &format!("{}: {e}", path.display())),
        }
    }
    let Some((path, written_as, mut file)) = taken else {
        return Response::text(409, &format!("{name}: no free name for it in {}", dir.display()));
    };
    *read = true;
    let wrote = async {
        use tokio::io::AsyncWriteExt;
        let first = req.body.len().min(req.length);
        file.write_all(&req.body[..first]).await?;
        let mut left = req.length - first;
        let mut buf = vec![0u8; 64 * 1024];
        while left > 0 {
            let want = left.min(buf.len());
            let n = match tokio::time::timeout(UPLOAD_IDLE, stream.read(&mut buf[..want])).await {
                Ok(r) => r?,
                Err(_) => anyhow::bail!("nothing came for {} s", UPLOAD_IDLE.as_secs()),
            };
            if n == 0 {
                anyhow::bail!("cut short: {} of {} bytes came", req.length - left, req.length);
            }
            file.write_all(&buf[..n]).await?;
            left -= n;
        }
        file.flush().await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = wrote {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Response::text(400, &format!("{e:#}"));
    }
    drop(file);
    let shown = path.to_string_lossy().into_owned();
    let _ = crate::client::query(
        &state.socket,
        &["web-uploaded", pane, &shown, &req.length.to_string(), &peer.to_string()],
    )
    .await;
    Response::json(format!(
        "{{\"path\":{},\"name\":{},\"bytes\":{}}}",
        json_str(&shown),
        json_str(&written_as),
        req.length
    ))
}

/// A button's key: one of the named keys, or Ctrl with a letter (`C-c`).
fn is_named_key(k: &str) -> bool {
    // Ctrl with a letter, Alt with a letter or digit (the page's Ctrl and
    // Alt keys, then a character typed).
    let with = |prefix: &str, ok: fn(&u8) -> bool| k.len() == 3 && k.starts_with(prefix) && ok(&k.as_bytes()[2]);
    KEYS.contains(&k)
        || with("C-", u8::is_ascii_lowercase)
        || with("M-", |b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || held_with_a_button(k)
}

/// A button's key with the page's Ctrl, Alt and Shift held (`S-Left`,
/// `C-S-Right`): each at most once, then a key the page has (not the wheel).
fn held_with_a_button(k: &str) -> bool {
    let mut rest = k;
    let mut seen = String::new();
    while let Some((m, after)) = rest.split_once('-').filter(|(m, _)| matches!(*m, "C" | "M" | "S")) {
        if seen.contains(m) {
            return false;
        }
        seen.push_str(m);
        rest = after;
    }
    !seen.is_empty() && KEYS.contains(&rest) && !rest.starts_with("Wheel")
}

/// `list-panes -F` for `/api/panes`, read by `panes_json`. The program
/// running now (`cargo` under the shell during a build), or the one the pane
/// started with once that is gone.
const PANE_FIELDS: &str = "#{pane_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{pane_index}\t\
                           #{?pane_pid_command,#{pane_pid_command},#{pane_current_command}}\t\
                           #{pane_active}\t#{window_active}\t#{pane_width}\t\
                           #{pane_height}\t#{pane_dead}\t#{session_attached}\t\
                           #{window_activity_flag}\t#{window_bell_flag}\t#{window_silence_flag}\t\
                           #{pane_name}\t#{pane_work_mode}\t#{pane_current_path_short}\t\
                           #{pane_activity}\t#{pane_last_line}\t#{pane_idle}\t#{pane_inbox}\t#{pane_unheard}\t#{pane_message}\t\
                           #{pane_dead_status}\t#{alternate_on}\t\
                           #{agent}\t#{agent_model}\t#{agent_cost}\t#{agent_context}\t#{agent_tokens}\t\
                           #{agent_cpu}\t#{agent_mem}\t#{pane_state}\t#{pane_title}";

/// The list-panes lines (tab-separated, in `PANE_FIELDS` order) as a JSON
/// array; with `tails` (`web-tails`), each pane's as its `tail` (none for
/// a pane not in them).
fn panes_json(out: &str, tails: Option<&serde_json::Value>) -> String {
    panes_json_at(out, chrono::Utc::now().timestamp().max(0) as u64, tails)
}

/// `panes_json` with the time now given, for the tests.
fn panes_json_at(out: &str, now: u64, tails: Option<&serde_json::Value>) -> String {
    let items: Vec<String> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 35 {
                return None;
            }
            // The title the program set (last: one with a tab in it is still
            // whole, the tab a space).
            let title = f[34..].join(" ");
            // The agent it runs (`list-agents`), its figures as they read;
            // null for none.
            let agent = if f[26].is_empty() {
                "null".to_string()
            } else {
                let keys = ["kind", "model", "cost", "context", "tokens", "cpu", "mem"];
                let o: serde_json::Map<String, serde_json::Value> =
                    keys.iter().zip(&f[26..33]).map(|(k, v)| (k.to_string(), (*v).into())).collect();
                serde_json::Value::Object(o).to_string()
            };
            let num = |s: &str| s.parse::<u64>().unwrap_or(0);
            let tail = tails
                .and_then(|t| t.get(f[0]))
                .filter(|v| v.is_array())
                .map_or(String::new(), |v| format!(",\"tail\":{v}"));
            // The window's alerts, as the status line marks them: it printed
            // (#), rang (!), or went quiet (~) while nobody looked.
            Some(format!(
                "{{\"id\":{},\"session\":{},\"window\":{},\"windowName\":{},\"pane\":{},\"command\":{},\
                 \"active\":{},\"windowActive\":{},\"cols\":{},\"rows\":{},\"dead\":{},\"attached\":{},\
                 \"activity\":{},\"bell\":{},\"silence\":{},\"name\":{},\"mode\":{},\"path\":{},\"quiet\":{},\"last\":{},\
                 \"idle\":{},\"state\":{},\"inbox\":{},\"unheard\":{},\"working\":{},\"exit\":{},\"alt\":{},\"agent\":{},\"title\":{}{tail}}}",
                json_str(f[0]),
                json_str(f[1]),
                num(f[2]),
                json_str(f[3]),
                num(f[4]),
                json_str(f[5]),
                f[6] == "1",
                f[7] == "1",
                num(f[8]),
                num(f[9]),
                f[10] == "1",
                num(f[11]) > 0,
                f[12] == "1",
                f[13] == "1",
                f[14] == "1",
                json_str(f[15]),
                json_str(f[16]),
                json_str(f[17]),
                // Seconds since it last printed, by this machine's clock (the
                // phone's may be off).
                now.saturating_sub(num(f[18])),
                json_str(f[19]),
                // Free for its next message (a shell or agent pane: a
                // normal one is never), and how many wait in its inbox.
                f[20] == "1",
                // What it is up to, in a word (idle, busy, typing, no-agent,
                // unknown, exited, `-`).
                json_str(f[33]),
                num(f[21]),
                // An agent's pane never heard from: the hook it lacks.
                f[22] == "1",
                // The message it works on (0: none).
                num(f[23]),
                // How its program ended, once it has (else null).
                f[24].parse::<u32>().map_or("null".to_string(), |c| c.to_string()),
                // A full-screen program (no scrollback): the page scrolls it
                // with the wheel instead.
                f[25] == "1",
                agent,
                json_str(&title)
            ))
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn req(method: &str, path: &str, key: Option<&str>) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            query: Vec::new(),
            key: key.map(String::from),
            headers: Vec::new(),
            gzip: false,
            range: None,
            body: Vec::new(),
            length: 0,
            sized: false,
        }
    }

    #[test]
    fn keys_are_random_and_url_safe() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        let keys: HashSet<String> = (0..200).map(|_| new_key().unwrap()).collect();
        assert_eq!(keys.len(), 200, "no repeats");
        for k in &keys {
            assert_eq!(k.len(), 22, "{k}");
            assert!(k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{k}");
        }
        assert!(same_key("abc", "abc") && !same_key("abc", "abd") && !same_key("abc", "ab"));
    }

    /// The rows a joined line took, by its width: a mark on any of them goes
    /// on that line; colours take no room, wide characters two columns; a
    /// count that does not add up places nothing.
    #[test]
    fn rows_find_their_joined_line() {
        let long = "x".repeat(25);
        assert_eq!(joined_rows(&format!("abc\n{long}\n"), 5, 10), Some(vec![0, 1, 1, 1, 2]));
        assert_eq!(joined_rows(&format!("\x1b[31m{}\x1b[0m\nb", "y".repeat(10)), 2, 10), Some(vec![0, 1]));
        assert_eq!(joined_rows(&"中".repeat(6), 2, 10), Some(vec![0, 0]));
        assert_eq!(joined_rows("trailing spaces    \nb", 2, 16), Some(vec![0, 1]));
        assert_eq!(joined_rows(&long, 2, 10), None, "25 columns are 3 rows, not 2");
        assert_eq!(joined_rows("a", 1, 0), None);
    }

    /// `keepane web`'s flags, long or short, reach `web-start` as they were
    /// given; the address comes back out of what it says.
    #[test]
    fn the_flags_pass_through_and_the_address_comes_back() {
        let o = Options::parse(&["--port", "8080", "-b", "192.168.1.23", "--read-only", "-k"]).unwrap();
        let want =
            Options { port: Some(8080), bind: vec!["192.168.1.23".parse().unwrap()], read_only: true, keep_key: true };
        assert_eq!(o, want);
        let flags = o.flags();
        assert_eq!(Options::parse(&flags.iter().map(String::as_str).collect::<Vec<_>>()).unwrap(), want);
        let argv: Vec<String> = [vec!["web-start".to_string()], flags].concat();
        assert_eq!(crate::command::parse(&argv), Ok(crate::command::Cmd::WebStart(want.clone())));
        assert_eq!(crate::command::Cmd::WebStart(want).to_string(), "web-start -p 8080 -b 192.168.1.23 -r -k");
        assert_eq!(Options::parse(&[]).unwrap(), Options::default());
        // More than one card: -b again, or commas; each once, in order, and
        // through web-start and back the same.
        let many =
            Options::parse(&["-b", "192.168.1.23,10.0.0.7", "--bind", "fd7a:115c:a1e0::1", "-b", "10.0.0.7"]).unwrap();
        let ips: Vec<IpAddr> =
            ["192.168.1.23", "10.0.0.7", "fd7a:115c:a1e0::1"].iter().map(|s| s.parse().unwrap()).collect();
        assert_eq!(many.bind, ips);
        let argv: Vec<String> = [vec!["web-start".to_string()], many.flags()].concat();
        assert_eq!(crate::command::parse(&argv), Ok(crate::command::Cmd::WebStart(many.clone())));
        assert_eq!(many.flags(), ["-b", "192.168.1.23,10.0.0.7,fd7a:115c:a1e0::1"]);
        for bad in
            [&["--port"][..], &["-p", "x"], &["-p", "70000"], &["-b", "somewhere"], &["-b", "10.0.0.7,x"], &["--nope"]]
        {
            assert!(Options::parse(bad).is_err(), "{bad:?}");
        }
        let status = "serving http://10.0.0.2:7681/#k=abc · kept key · since 09:00 · 1 connected\n  10.0.0.3 ...";
        assert_eq!(status_url(status), Some("http://10.0.0.2:7681/#k=abc"));
        assert_eq!(status_url("off: `keepane web` starts it"), None);
    }

    #[test]
    fn queries_decode_as_a_browser_writes_them() {
        assert_eq!(percent_decode("%25"), "%");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        assert_eq!(percent_decode("%zz"), "%zz", "not an escape: as it is");
        assert_eq!(percent_decode("%2"), "%2");
        assert_eq!(percent_decode("x%"), "x%");
        assert!(is_pane_id("%12") && !is_pane_id("%") && !is_pane_id("12") && !is_pane_id("%1a"));
        assert!(is_named_key("Enter") && is_named_key("BTab") && is_named_key("C-c"));
        assert!(!is_named_key("-X") && !is_named_key("C-") && !is_named_key("C-cc") && !is_named_key("kill-server"));
        assert_eq!(json_str("a\"b\\c\nd\u{1}"), r#""a\"b\\c\nd\u0001""#);
    }

    /// A file sent in (`POST /api/upload`): its head read, its body not
    /// (only what came with the head), its length kept, past the 64 KB a
    /// request may otherwise hold; a GET of `/api/upload-info` is a request
    /// like any other.
    #[tokio::test]
    async fn an_uploads_body_is_left_to_read_as_it_is_written() {
        let raw = b"POST /api/upload?pane=%252&name=a.bin HTTP/1.1\r\nContent-Length: 1000000\r\nX-Keepane-Key: k\r\n\r\nfirst";
        let r = read_request(&mut &raw[..]).await.unwrap().unwrap();
        assert_eq!((r.path.as_str(), r.param("name"), r.length), ("/api/upload", Some("a.bin"), 1_000_000));
        assert_eq!(r.body, b"first", "only what came with the head");
        let small =
            read_request(&mut &b"POST /api/send HTTP/1.1\r\nContent-Length: 2\r\n\r\nhi"[..]).await.unwrap().unwrap();
        assert_eq!((small.body.as_slice(), small.length), (&b"hi"[..], 0));
        let info = format!("POST /api/upload-info HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read_request(&mut info.as_bytes()).await.is_err(), "not an upload: bounded");
    }

    /// What a sent file may be called here: its last part, safe on Windows,
    /// never a device's name nor nothing; and the names tried when it is
    /// taken.
    #[test]
    fn an_uploaded_file_gets_a_safe_name_never_over_another() {
        for (raw, want) in [
            ("report.pdf", Some("report.pdf")),
            (r"C:\Users\x\photo.jpg", Some("photo.jpg")),
            ("../../etc/passwd", Some("passwd")),
            ("a:b*c?.txt", Some("a_b_c_.txt")),
            ("notes. ", Some("notes")),
            ("CON", Some("_CON")),
            ("nul.txt", Some("_nul.txt")),
            ("com1.log", Some("_com1.log")),
            ("console.txt", Some("console.txt")),
            ("中文 文件.md", Some("中文 文件.md")),
            ("..", None),
            ("dir/", None),
            ("  ", None),
            ("\u{1}\u{2}", None),
        ] {
            assert_eq!(upload_name(raw).as_deref(), want, "{raw:?}");
        }
        // Too long for a file system: 200 bytes at most, cut at a
        // character, the extension kept.
        let cut = upload_name(&format!("{}.pdf", "长".repeat(300))).unwrap();
        assert_eq!(cut, format!("{}.pdf", "长".repeat(65)), "65 × 3 bytes + 4");
        assert_eq!(upload_name(&"x".repeat(400)).unwrap(), "x".repeat(200));
        assert_eq!(upload_name(&format!("{}.pdf", "a".repeat(199))).unwrap().len(), 200);
        let tries: Vec<String> = upload_names("report.pdf").take(3).collect();
        assert_eq!(tries, ["report.pdf", "report (1).pdf", "report (2).pdf"]);
        let tries: Vec<String> = upload_names(".env").take(2).collect();
        assert_eq!(tries, [".env", ".env (1)"]);
        let tries: Vec<String> = upload_names("archive.tar.gz").take(2).collect();
        assert_eq!(tries, ["archive.tar.gz", "archive.tar (1).gz"]);
    }

    /// Where a pane's uploads go: `keepane-uploads` under its directory
    /// unless told, an absolute folder as it is (its directory then not
    /// needed), the limit as shown or 2G; another machine's pane, or a
    /// relative folder with no directory known, refused.
    #[test]
    fn a_panes_uploads_go_under_its_directory_and_never_to_another_machine() {
        let here = std::env::temp_dir();
        let abs = here.join("keepane-in").to_string_lossy().into_owned();
        let cwd = here.join("proj").to_string_lossy().into_owned();
        assert_eq!(upload_dir("%1", &cwd, "", "", ""), Ok((here.join("proj").join("keepane-uploads"), 2 << 30)));
        assert_eq!(upload_dir("%1", &cwd, "", " \n", ""), Ok((here.join("proj").join("keepane-uploads"), 2 << 30)));
        assert_eq!(upload_dir("%1", &cwd, "", "in", "300M\n"), Ok((here.join("proj").join("in"), 300 << 20)));
        assert_eq!(upload_dir("%1", "", "", &abs, "1G"), Ok((here.join("keepane-in"), 1 << 30)));
        let (status, why) = upload_dir("%1", &cwd, "10.0.0.2:7681", "", "").unwrap_err();
        assert!(status == 400 && why.contains("pane of 10.0.0.2:7681"), "{why}");
        let (status, why) = upload_dir("%1", "", "", "uploads", "").unwrap_err();
        assert!(status == 400 && why.contains("directory is not known"), "{why}");
    }

    /// A folder keepane makes for uploads (nested too) gets a `.gitignore`
    /// that leaves it out of git; a folder there already is left as it is,
    /// and a `.gitignore` there is never written over.
    #[tokio::test]
    async fn a_folder_made_for_uploads_is_left_out_of_git_and_one_there_is_left_alone() {
        let base = std::env::temp_dir().join(format!("keepane-upfolder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let made = base.join("a").join("keepane-uploads");
        upload_folder(&made).await.unwrap();
        assert_eq!(std::fs::read_to_string(made.join(".gitignore")).unwrap(), UPLOAD_GITIGNORE);
        assert!(UPLOAD_GITIGNORE.lines().any(|l| l == "*"), "the whole folder");
        assert!(!base.join("a").join(".gitignore").exists(), "only in the folder for uploads");
        // Again: the one there is kept as the user left it.
        std::fs::write(made.join(".gitignore"), "mine\n").unwrap();
        upload_folder(&made).await.unwrap();
        assert_eq!(std::fs::read_to_string(made.join(".gitignore")).unwrap(), "mine\n");
        // A folder there already (a project's own): nothing added to it.
        let theirs = base.join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        upload_folder(&theirs).await.unwrap();
        assert!(!theirs.join(".gitignore").exists());
        // A file where the folder would be: an error, nothing written.
        let file = base.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(upload_folder(&file).await.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn requests_are_read_whole_and_bounded() {
        let raw = b"POST /api/send?pane=%252&key=Enter HTTP/1.1\r\nHost: x\r\nX-Keepane-Key: abc\r\ncontent-length: 5\r\n\r\nhello";
        let r = read_request(&mut &raw[..]).await.unwrap().unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/api/send");
        assert_eq!(r.param("pane"), Some("%2"));
        assert_eq!(r.param("key"), Some("Enter"));
        assert_eq!(r.key.as_deref(), Some("abc"));
        assert_eq!(r.body, b"hello");
        // The peer went away before a whole head: nothing, not an error.
        assert!(read_request(&mut &b"GET / HTT"[..]).await.unwrap().is_none());
        // Not HTTP, too big, a body cut short: errors.
        assert!(read_request(&mut &b"hello\r\n\r\n"[..]).await.is_err());
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
        assert!(read_request(&mut huge.as_bytes()).await.is_err());
        let big = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read_request(&mut big.as_bytes()).await.is_err());
        assert!(read_request(&mut &b"POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc"[..]).await.is_err());
        // Another machine's message (`/link/`) may be as big as
        // message-max-size goes, and no more; its x-keepane-* headers come
        // along, names in lower case.
        let body = "x".repeat(MAX_BODY + 1);
        let link = format!(
            "POST /link/send HTTP/1.1\r\nX-Keepane-Link: 1\r\nX-Keepane-From: k\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let r = read_request(&mut link.as_bytes()).await.unwrap().unwrap();
        assert_eq!(r.headers, [("x-keepane-link".to_string(), "1".to_string()), ("x-keepane-from".into(), "k".into())]);
        assert_eq!(r.body.len(), MAX_BODY + 1);
        let too_big = format!("POST /link/send HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_LINK_BODY + 1);
        assert!(read_request(&mut too_big.as_bytes()).await.is_err());
    }

    /// The page goes gzipped to a browser that takes it, as it is otherwise.
    #[tokio::test]
    async fn the_page_is_gzipped_for_a_browser_that_takes_it() {
        let head = |enc: &str| format!("GET / HTTP/1.1\r\nHost: x\r\n{enc}\r\n\r\n");
        for (enc, gzip) in [
            ("Accept-Encoding: gzip, deflate, br", true),
            ("accept-encoding: br, GZIP;q=1.0", true),
            ("Accept-Encoding: br", false),
            ("X-None: 1", false),
        ] {
            let r = read_request(&mut head(enc).as_bytes()).await.unwrap().unwrap();
            assert_eq!(r.gzip, gzip, "{enc}");
            let state = State::new("nowhere", "k", false);
            let resp = handle(&r, "127.0.0.1".parse().unwrap(), &state).await;
            assert_eq!(resp.status, 200);
            let encoding = resp.headers.iter().find(|(k, _)| *k == "Content-Encoding").map(|(_, v)| v.as_str());
            assert_eq!(encoding, gzip.then_some("gzip"), "{enc}");
            if gzip {
                assert_eq!(&resp.body[..2], [0x1f, 0x8b], "gzip's magic");
                assert!(resp.body.len() * 3 < PAGE.len(), "smaller by far");
            } else {
                assert!(String::from_utf8_lossy(&resp.body).contains("<title>keepane</title>"));
            }
        }
    }

    #[tokio::test]
    async fn the_key_guards_everything_but_the_page() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        // No server behind this state: every answer here comes before one
        // would be asked.
        let s = State::new("keepane-web-test-no-server", "sekrit", false);
        assert_eq!(handle(&req("GET", "/", None), ip, &s).await.status, 200);
        assert_eq!(handle(&req("GET", "/icon.svg", None), ip, &s).await.status, 200);
        assert_eq!(handle(&req("GET", "/manifest.webmanifest", None), ip, &s).await.status, 200);
        assert_eq!(handle(&req("GET", "/nope", None), ip, &s).await.status, 404);
        for path in ["/api/info", "/api/panes", "/api/screen", "/api/send", "/api/action"] {
            assert_eq!(handle(&req("GET", path, None), ip, &s).await.status, 401, "{path} without a key");
            assert_eq!(handle(&req("POST", path, Some("wrong")), ip, &s).await.status, 401, "{path} wrong key");
        }
        assert_eq!(handle(&req("GET", "/api/info", Some("sekrit")), ip, &s).await.status, 200);
        assert_eq!(handle(&req("GET", "/api/send", Some("sekrit")), ip, &s).await.status, 405);
        let mut bad = req("POST", "/api/action", Some("sekrit"));
        bad.query = vec![("pane".into(), "%1".into()), ("do".into(), "kill-server".into())];
        assert_eq!(handle(&bad, ip, &s).await.status, 400, "only the menu's actions");
        let mut badkey = req("POST", "/api/send", Some("sekrit"));
        badkey.query = vec![("pane".into(), "%1".into()), ("key".into(), "-X".into())];
        assert_eq!(handle(&badkey, ip, &s).await.status, 400, "only the page's keys");
        // Read-only: looking is allowed, typing and the menu are not.
        let ro = State::new("keepane-web-test-no-server", "sekrit", true);
        assert_eq!(handle(&req("POST", "/api/send", Some("sekrit")), ip, &ro).await.status, 403);
        assert_eq!(handle(&req("POST", "/api/action", Some("sekrit")), ip, &ro).await.status, 403);
        let info = handle(&req("GET", "/api/info", Some("sekrit")), ip, &ro).await;
        assert!(String::from_utf8_lossy(&info.body).contains("\"readOnly\":true"));
    }

    #[test]
    fn marks_land_on_the_captured_lines_that_still_read_so() {
        let mark = |row: i64, times: (i64, i64, i64), last: i64, stop: i64, text: &str, command: &str| {
            serde_json::json!({"row": row, "start": times.0, "end": times.1, "exit": times.2,
                "last": last, "stop": stop, "text": text, "command": command})
        };
        // Two lines of scrollback above a three-line screen, all captured.
        let text = "PS> ls\nfile\n\x1b[32mPS> \x1b[0mbad\x1b[0m\nerr\nPS>";
        let marks = serde_json::json!([
            mark(-2, (1000, 1500, 0), -2, 0, "PS> ls", "ls"),
            mark(0, (2000, 2100, 1), 0, 2, "PS> bad", "bad"),
            mark(1, (3000, 3100, 0), 1, 2, "PS> gone", "gone"),
        ])
        .to_string();
        assert_eq!(
            marks_list(marks_placed(text, &marks, 300, 2)),
            r#"[[0,1000,1500,0,0,2,"ls"],[2,2000,2100,1,2,4,"bad"]]"#
        );
        // With less history asked for than there is, lines shift with it.
        assert_eq!(
            marks_list(marks_placed("PS> \x1b[1mbad\nerr\nPS>", &marks, 0, 2)),
            r#"[[0,2000,2100,1,0,2,"bad"]]"#
        );
        // Unknown times and codes are null; one not JSON is no marks; a stop
        // past the capture ends with it.
        let unknown = r#"[{"row":0,"start":null,"end":5,"exit":null,"last":0,"stop":9,"text":"PS> x","command":"x"}]"#;
        assert_eq!(marks_list(marks_placed("PS> x\ny", unknown, 0, 0)), r#"[[0,null,5,null,0,2,"x"]]"#);
        assert!(marks_placed("PS> x", "nonsense", 0, 0).is_empty());
        assert_eq!(without_escapes("\x1b[38;2;1;2;3ma\x1b[0mb"), "ab");
    }

    #[test]
    fn marks_go_onto_the_joined_lines_with_their_spans() {
        let p = |line, last, stop| Placed { line, last, stop, times: "1,2,0".into(), command: "c".into() };
        // Rows: a prompt of two (0, 1), a command typed over two (1, 2),
        // two printed (3, 4), the next prompt (5, 6). Joined: the prompt and
        // command are one line, each printed line one, the prompt one.
        let line_of = [0, 0, 0, 1, 2, 3, 3];
        assert_eq!(on_joined(vec![p(1, 2, 5)], &line_of, 4), vec![p(0, 0, 3)]);
        // A stop past the rows: the end; two marks on one joined line: the first.
        assert_eq!(on_joined(vec![p(3, 3, 7), p(4, 4, 7)], &[0, 0, 0, 1, 1], 2), vec![p(1, 1, 2)]);
    }

    #[test]
    fn the_code_and_the_pane_list() {
        let qr = qr_text("http://192.168.1.23:7681/#k=AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert!(qr.lines().count() > 10 && qr.contains('█'), "{qr}");
        let json = panes_json_at(
            "%3\tdev\t0\tbuild\t1\tcargo\t1\t0\t80\t24\t0\t1\t1\t0\t1\tbuilder\tshell\t~/src\t1000\ttests: 42 passed\t0\t2\t0\t7\t\t0\t\t\t\t\t\t\t\ttyping\t✳ fix\tthe login\n\
             %4\tdev\t0\tbuild\t2\tclaude\t0\t0\t80\t24\t1\t1\t0\t0\t0\t\tai\t~/src\t1060\t\t0\t3\t1\t\t2\t1\tclaude\tclaude-opus-5-5\t$2.10\t34%\t1.2M\t12%\t812M\texited\t\nshort line\n",
            1060,
            None,
        );
        assert_eq!(
            json,
            r#"[{"id":"%3","session":"dev","window":0,"windowName":"build","pane":1,"command":"cargo","active":true,"windowActive":false,"cols":80,"rows":24,"dead":false,"attached":true,"activity":true,"bell":false,"silence":true,"name":"builder","mode":"shell","path":"~/src","quiet":60,"last":"tests: 42 passed","idle":false,"state":"typing","inbox":2,"unheard":false,"working":7,"exit":null,"alt":false,"agent":null,"title":"✳ fix the login"},{"id":"%4","session":"dev","window":0,"windowName":"build","pane":2,"command":"claude","active":false,"windowActive":false,"cols":80,"rows":24,"dead":true,"attached":true,"activity":false,"bell":false,"silence":false,"name":"","mode":"ai","path":"~/src","quiet":0,"last":"","idle":false,"state":"exited","inbox":3,"unheard":true,"working":0,"exit":2,"alt":true,"agent":{"kind":"claude","model":"claude-opus-5-5","cost":"$2.10","context":"34%","tokens":"1.2M","cpu":"12%","mem":"812M"},"title":""}]"#
        );
        assert_eq!(panes_json("", None), "[]");
        // Asked with each pane's last lines (`tail=N`): each its own, after
        // the rest; a pane `web-tails` did not give (made in between), none
        // (the page shows its last line); not asked, the list as it was.
        let two = "%3\tdev\t0\tw\t0\tsh\t1\t1\t80\t24\t0\t1\t0\t0\t0\t\t\t~\t0\t\t0\t0\t0\t\t\t0\t\t\t\t\t\t\t\t-\t\n\
                   %4\tdev\t0\tw\t1\tsh\t0\t1\t80\t24\t0\t1\t0\t0\t0\t\t\t~\t0\t\t0\t0\t0\t\t\t0\t\t\t\t\t\t\t\t-\t\n";
        let tails = serde_json::json!({"%3": ["one", "two \"2\""], "%4": "not a list", "%9": ["gone"]});
        let with = panes_json_at(two, 0, Some(&tails));
        let v: serde_json::Value = serde_json::from_str(&with).unwrap();
        assert_eq!(v[0]["tail"], serde_json::json!(["one", "two \"2\""]));
        assert!(v[1].get("tail").is_none() && v.as_array().unwrap().len() == 2, "{with}");
        assert!(with.contains(r#""title":"","tail":["one","two \"2\""]}"#), "{with}");
        let without = panes_json_at(two, 0, None);
        assert_eq!(panes_json_at(two, 0, Some(&serde_json::json!({}))), without);
        assert!(!without.contains("tail"), "not asked: no tail");
        // The fields as the server fills them read back: the agent's in
        // their places.
        let ctx = crate::format::Context {
            pane_id: 9,
            pane_title: "a title".into(),
            agent: "pi".into(),
            agent_model: "m".into(),
            agent_cost: "$1.00".into(),
            agent_context: "5%".into(),
            agent_tokens: "2k".into(),
            agent_cpu: "1%".into(),
            agent_mem: "9M".into(),
            ..Default::default()
        };
        let line = crate::format::plain(&crate::format::expand(
            PANE_FIELDS,
            &ctx,
            &mut crate::format::ShellCache::default(),
            Default::default(),
            chrono::Local::now(),
        ));
        let v: serde_json::Value = serde_json::from_str(&panes_json_at(&line, 0, None)).unwrap();
        assert_eq!(
            (&v[0]["id"], &v[0]["title"], &v[0]["agent"]),
            (
                &serde_json::json!("%9"),
                &serde_json::json!("a title"),
                &serde_json::json!({"kind": "pi", "model": "m", "cost": "$1.00", "context": "5%",
                    "tokens": "2k", "cpu": "1%", "mem": "9M"})
            )
        );
        // The keys the page sends: named ones, Ctrl with a letter, Alt with
        // a letter or digit; nothing else.
        for k in ["Enter", "C-c", "C-x", "M-x", "M-1"] {
            assert!(is_named_key(k), "{k}");
        }
        // Shift (and Ctrl, Alt) held for a button: Shift with an arrow.
        for k in ["S-Left", "S-Right", "C-S-Right", "M-Up", "C-Left", "S-Tab", "C-M-S-End"] {
            assert!(is_named_key(k), "{k}");
        }
        for k in
            ["C-X", "M-X", "M-", "C-cc", "x", "send-keys", "S-x", "S-S-Left", "X-Left", "S-WheelUp", "S-", "Left-S"]
        {
            assert!(!is_named_key(k), "{k}");
        }
    }
}
