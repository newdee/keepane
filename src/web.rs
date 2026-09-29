//! `keepane web`: the panes on a phone, over the local network.
//!
//! HTTP on the LAN: a page that lists every pane, shows one as it is on the
//! screen (colours included), and sends what is typed on the phone to it.
//! Everything runs on this machine; the phone only shows and types. The
//! server hosts it (`web-start`, `web-status`, `web-stop`), so it runs in the
//! background for as long as the server does; each request is still asked
//! of the server as a client would ask it, with the same commands the CLI
//! sends. `keepane web` starts it and prints the code to scan.
//!
//! Off unless started. Every request but the page itself needs the key: 128
//! random bits, made anew each start (or kept, with `--keep-key`), handed to
//! the phone in the address a QR code in the terminal carries, after the `#`
//! so that it is never part of a request line. The phone can only look,
//! type into a pane, and run the few fixed actions of the page (new
//! window, split, close a pane; rename a session or window): no
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

const PAGE: &str = include_str!("web_page.html");
const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#1a1b26"/><rect x="10" y="12" width="44" height="40" rx="5" fill="none" stroke="#7aa2f7" stroke-width="4"/><path d="M32 12v40M32 32h22" stroke="#7aa2f7" stroke-width="4"/><path d="M16 22l6 5-6 5" fill="none" stroke="#9ece6a" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const MANIFEST: &str = r##"{"name":"keepane","short_name":"keepane","start_url":"/","display":"standalone","background_color":"#1a1b26","theme_color":"#16161e","icons":[{"src":"/icon.svg","sizes":"any","type":"image/svg+xml"}]}"##;

/// Named keys the page's buttons send; anything else is typed as text.
const KEYS: &[&str] = &[
    "Enter", "Escape", "Tab", "BTab", "BSpace", "Space", "Up", "Down", "Left", "Right", "Home", "End", "PPage",
    "NPage", "DC",
];

/// What the page's + menu and its names can do, and nothing else.
const ACTIONS: &[&str] = &["new-window", "split-h", "split-v", "kill-pane", "rename-session", "rename-window"];

/// How `web-start` serves: its flags (`keepane web` passes its own on).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// None: `DEFAULT_PORT`; 0, any free one.
    pub port: Option<u16>,
    /// None: the address this machine reaches the network through.
    pub bind: Option<IpAddr>,
    pub read_only: bool,
    pub keep_key: bool,
}

impl Options {
    /// The flags, long or short, one value after each of `-p` and `-b`.
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
                    o.bind = Some(v.parse().map_err(|_| format!("--bind: not an IP address: {v}"))?);
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
        if let Some(b) = self.bind {
            f.extend(["-b".to_string(), b.to_string()]);
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
}

impl State {
    pub fn new(socket: &str, key: &str, read_only: bool) -> State {
        State { socket: socket.into(), key: key.into(), read_only, notify: None }
    }

    pub fn notify(mut self, f: impl Fn(Seen) + Send + Sync + 'static) -> State {
        self.notify = Some(Box::new(f));
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
        let (code, out, mut err) = crate::client::query(socket, &argv).await?;
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
            println!("off: no keepane server is running (socket '{socket}')");
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
    if o.bind.is_none() && (url.starts_with("http://127.") || url.starts_with("http://[::1]")) {
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
fn qr_text(data: &str) -> Result<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(data.as_bytes()).context("make the QR code")?;
    Ok(code.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).quiet_zone(true).build())
}

/// Compare without stopping at the first difference.
fn same_key(a: &str, b: &str) -> bool {
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
    pub body: Vec<u8>,
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
    let (mut length, mut key, mut headers) = (0usize, None, Vec::new());
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().context("bad Content-Length")?;
        } else if name.eq_ignore_ascii_case("x-keepane-key") {
            key = Some(value.to_string());
        } else if name.len() > 10 && name[..10].eq_ignore_ascii_case("x-keepane-") {
            headers.push((name.to_ascii_lowercase(), value.to_string()));
        }
    }
    if length > if target.starts_with("/link/") { MAX_LINK_BODY } else { MAX_BODY } {
        bail!("request body too large");
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            bail!("body cut short");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    Ok(Some(Request { method, path: percent_decode(path), query, key, headers, body }))
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
    let resp = handle(&req, peer, state).await;
    write_response(&mut stream, &resp).await
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
/// those lines, `[line, start, end, exit]` (`list-marks`, its times in Unix
/// milliseconds, null where unknown). `join`: lines the pane wrapped come
/// as one (`capture-pane -J`), for a phone that wraps them at its own
/// width; each mark then goes on the joined line its row is part of.
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
    let marks = q(vec!["list-marks".into(), "-t".into(), pane.into()]).await;
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
                    Some(line_of) => {
                        let mut seen = std::collections::HashSet::new();
                        let placed = placed
                            .into_iter()
                            .filter_map(|(i, rest)| line_of.get(i).filter(|j| seen.insert(**j)).map(|j| (*j, rest)));
                        marks_list(placed)
                    }
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

/// `list-marks` lines placed on the lines of `text`, a capture holding the
/// last `history` of `scrollback` lines above the screen: the line, and
/// `start,end,exit`. A mark whose line does not read as it did (output
/// arrived between the two questions) is left out rather than put against
/// the wrong line.
fn marks_placed(text: &str, marks: &str, history: u32, scrollback: usize) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let above = i64::from(history).min(scrollback as i64);
    let num = |s: &str| if s.parse::<i64>().is_ok() { s.to_string() } else { "null".to_string() };
    let mut out = Vec::new();
    for m in marks.lines() {
        let mut f = m.splitn(5, ' ');
        let (Some(row), Some(start), Some(end), Some(exit)) = (f.next(), f.next(), f.next(), f.next()) else {
            continue;
        };
        let said = f.next().unwrap_or("");
        let Some(i) = row.parse::<i64>().ok().map(|r| r + above).filter(|i| *i >= 0) else { continue };
        if lines.get(i as usize).is_some_and(|l| without_escapes(l).trim_end() == said) {
            out.push((i as usize, format!("{},{},{}", num(start), num(end), num(exit))));
        }
    }
    out
}

/// `[[line, start, end, exit], ...]`.
fn marks_list(placed: impl IntoIterator<Item = (usize, String)>) -> String {
    let items: Vec<String> = placed.into_iter().map(|(i, rest)| format!("[{i},{rest}]")).collect();
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
pub async fn handle(req: &Request, peer: IpAddr, state: &State) -> Response {
    let get = req.method == "GET";
    // The page and its bits carry no secret: the key comes from the address.
    match (get, req.path.as_str()) {
        (true, "/") => return Response::new(200, "text/html; charset=utf-8", PAGE),
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
    if get && matches!(req.path.as_str(), "/api/send" | "/api/action" | "/api/fit") {
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
            // The program running now (`cargo` under the shell during a
            // build), or the one the pane started with once that is gone.
            const FIELDS: &str = "#{pane_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{pane_index}\t\
                                  #{?pane_pid_command,#{pane_pid_command},#{pane_current_command}}\t\
                                  #{pane_active}\t#{window_active}\t#{pane_width}\t\
                                  #{pane_height}\t#{pane_dead}\t#{session_attached}\t\
                                  #{window_activity_flag}\t#{window_bell_flag}\t#{window_silence_flag}\t\
                                  #{pane_name}\t#{pane_work_mode}\t#{pane_current_path_short}\t\
                                  #{pane_activity}\t#{pane_last_line}\t#{pane_idle}\t#{pane_inbox}\t#{pane_unheard}\t#{pane_title}";
            match q(vec!["list-panes".into(), "-a".into(), "-F".into(), FIELDS.into()]).await {
                Ok((0, out, _)) => Response::json(panes_json(&out)),
                Ok((_, _, err)) => Response::text(500, err.trim()),
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
        (false, "/api/send") | (false, "/api/action") | (false, "/api/fit") if state.read_only => {
            Response::text(403, "read-only")
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
                // The pane's session or its window, by the pane: the
                // new name is the body, after `--` so that one starting with
                // `-` is a name. keepane says what a name may be.
                "rename-session" | "rename-window" => {
                    let name = match rename_body(&req.body) {
                        Ok(n) => n,
                        Err(e) => return Response::text(400, e),
                    };
                    vec![what.into(), "-t".into(), pane.into(), "--".into(), name]
                }
                _ => return Response::text(400, &format!("do: one of {}", ACTIONS.join(", "))),
            };
            let renaming = what.starts_with("rename-");
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

/// A button's key: one of the named keys, or Ctrl with a letter (`C-c`).
fn is_named_key(k: &str) -> bool {
    // Ctrl with a letter, Alt with a letter or digit (the page's Ctrl and
    // Alt keys, then a character typed).
    let with = |prefix: &str, ok: fn(&u8) -> bool| k.len() == 3 && k.starts_with(prefix) && ok(&k.as_bytes()[2]);
    KEYS.contains(&k)
        || with("C-", u8::is_ascii_lowercase)
        || with("M-", |b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The list-panes lines (tab-separated, in FIELDS order) as a JSON array.
fn panes_json(out: &str) -> String {
    panes_json_at(out, chrono::Utc::now().timestamp().max(0) as u64)
}

/// `panes_json` with the time now given, for the tests.
fn panes_json_at(out: &str, now: u64) -> String {
    let items: Vec<String> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 24 {
                return None;
            }
            // The title the program set (last: one with a tab in it is still
            // whole, the tab a space).
            let title = f[23..].join(" ");
            let num = |s: &str| s.parse::<u64>().unwrap_or(0);
            // The window's alerts, as the status line marks them: it printed
            // (#), rang (!), or went quiet (~) while nobody looked.
            Some(format!(
                "{{\"id\":{},\"session\":{},\"window\":{},\"windowName\":{},\"pane\":{},\"command\":{},\
                 \"active\":{},\"windowActive\":{},\"cols\":{},\"rows\":{},\"dead\":{},\"attached\":{},\
                 \"activity\":{},\"bell\":{},\"silence\":{},\"name\":{},\"mode\":{},\"path\":{},\"quiet\":{},\"last\":{},\
                 \"idle\":{},\"inbox\":{},\"unheard\":{},\"title\":{}}}",
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
                num(f[21]),
                // An agent's pane never heard from: the hook it lacks.
                f[22] == "1",
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
            body: Vec::new(),
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
            Options { port: Some(8080), bind: Some("192.168.1.23".parse().unwrap()), read_only: true, keep_key: true };
        assert_eq!(o, want);
        let flags = o.flags();
        assert_eq!(Options::parse(&flags.iter().map(String::as_str).collect::<Vec<_>>()).unwrap(), want);
        let argv: Vec<String> = [vec!["web-start".to_string()], flags].concat();
        assert_eq!(crate::command::parse(&argv), Ok(crate::command::Cmd::WebStart(want.clone())));
        assert_eq!(crate::command::Cmd::WebStart(want).to_string(), "web-start -p 8080 -b 192.168.1.23 -r -k");
        assert_eq!(Options::parse(&[]).unwrap(), Options::default());
        for bad in [&["--port"][..], &["-p", "x"], &["-p", "70000"], &["-b", "somewhere"], &["--nope"]] {
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
        // Two lines of scrollback above a three-line screen, all captured.
        let text = "PS> ls\nfile\n\x1b[32mPS> \x1b[0mbad\x1b[0m\nerr\nPS>";
        let marks = "-2 1000 1500 0 PS> ls\n0 2000 2100 1 PS> bad\n1 3000 3100 0 PS> gone";
        assert_eq!(marks_list(marks_placed(text, marks, 300, 2)), "[[0,1000,1500,0],[2,2000,2100,1]]");
        // With less history asked for than there is, lines shift with it.
        assert_eq!(marks_list(marks_placed("PS> \x1b[1mbad\nerr\nPS>", marks, 0, 2)), "[[0,2000,2100,1]]");
        // Unknown times and codes are null; a torn line is skipped.
        assert_eq!(marks_list(marks_placed("PS> x", "0 - 5 - PS> x\nnonsense", 0, 0)), "[[0,null,5,null]]");
        assert_eq!(without_escapes("\x1b[38;2;1;2;3ma\x1b[0mb"), "ab");
    }

    #[test]
    fn the_code_and_the_pane_list() {
        let qr = qr_text("http://192.168.1.23:7681/#k=AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert!(qr.lines().count() > 10 && qr.contains('█'), "{qr}");
        let json = panes_json_at(
            "%3\tdev\t0\tbuild\t1\tcargo\t1\t0\t80\t24\t0\t1\t1\t0\t1\tbuilder\tshell\t~/src\t1000\ttests: 42 passed\t0\t2\t0\t✳ fix\tthe login\n\
             %4\tdev\t0\tbuild\t2\tclaude\t0\t0\t80\t24\t0\t1\t0\t0\t0\t\tai\t~/src\t1060\t\t0\t3\t1\t\nshort line\n",
            1060,
        );
        assert_eq!(
            json,
            r#"[{"id":"%3","session":"dev","window":0,"windowName":"build","pane":1,"command":"cargo","active":true,"windowActive":false,"cols":80,"rows":24,"dead":false,"attached":true,"activity":true,"bell":false,"silence":true,"name":"builder","mode":"shell","path":"~/src","quiet":60,"last":"tests: 42 passed","idle":false,"inbox":2,"unheard":false,"title":"✳ fix the login"},{"id":"%4","session":"dev","window":0,"windowName":"build","pane":2,"command":"claude","active":false,"windowActive":false,"cols":80,"rows":24,"dead":false,"attached":true,"activity":false,"bell":false,"silence":false,"name":"","mode":"ai","path":"~/src","quiet":0,"last":"","idle":false,"inbox":3,"unheard":true,"title":""}]"#
        );
        assert_eq!(panes_json(""), "[]");
        // The keys the page sends: named ones, Ctrl with a letter, Alt with
        // a letter or digit; nothing else.
        for k in ["Enter", "C-c", "C-x", "M-x", "M-1"] {
            assert!(is_named_key(k), "{k}");
        }
        for k in ["C-X", "M-X", "M-", "C-cc", "x", "send-keys", "C-Left"] {
            assert!(!is_named_key(k), "{k}");
        }
    }
}
