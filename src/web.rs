//! `keepane web`: the panes on a phone, over the local network.
//!
//! A client of the server like any other (it asks the server with the same
//! commands the CLI sends), which also answers HTTP on the LAN: a page that
//! lists every pane, shows one as it is on the screen (colours included),
//! and sends what is typed on the phone to it. Everything runs on this
//! machine; the phone only shows and types.
//!
//! Off unless started. Every request but the page itself needs the key: 128
//! random bits, made anew each start (or kept, with `--keep-key`), handed to
//! the phone in the address a QR code in the terminal carries, after the `#`
//! so that it is never part of a request line. The phone can only look,
//! type into a pane, and run the few fixed actions of the page's + menu
//! (new window, split, close a pane): no command of its own reaches the
//! server. Plain HTTP, so for a network you trust; over anything else, a
//! private network such as Tailscale in between.

use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
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

/// What the page's + menu can do, and nothing else.
const ACTIONS: &[&str] = &["new-window", "split-h", "split-v", "kill-pane"];

struct Options {
    port: u16,
    bind: Option<IpAddr>,
    read_only: bool,
    keep_key: bool,
}

fn parse_args(args: &[String]) -> Result<Options> {
    let mut o = Options { port: DEFAULT_PORT, bind: None, read_only: false, keep_key: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" | "--port" => {
                let v = it.next().context("--port: a port number")?;
                o.port = v.parse().ok().filter(|p| *p > 0).with_context(|| format!("--port: not a port: {v}"))?;
            }
            "-b" | "--bind" => {
                let v = it.next().context("--bind: an address of this machine")?;
                o.bind = Some(v.parse().with_context(|| format!("--bind: not an IP address: {v}"))?);
            }
            "-r" | "--read-only" => o.read_only = true,
            "-k" | "--keep-key" => o.keep_key = true,
            other => bail!("web: unknown argument '{other}' (--port N, --bind IP, --read-only, --keep-key)"),
        }
    }
    Ok(o)
}

/// Everything a request handler needs.
pub struct State {
    pub socket: String,
    pub key: String,
    pub read_only: bool,
    /// Addresses already announced in the terminal, and refused ones.
    seen: Mutex<HashSet<(IpAddr, bool)>>,
    /// Print who connects (off in tests).
    pub announce: bool,
}

impl State {
    pub fn new(socket: &str, key: &str, read_only: bool, announce: bool) -> State {
        State { socket: socket.into(), key: key.into(), read_only, seen: Mutex::new(HashSet::new()), announce }
    }
}

/// `keepane web [--port N] [--bind IP] [--read-only] [--keep-key]`.
pub async fn run(socket: &str, args: &[String]) -> Result<i32> {
    let o = parse_args(args)?;
    if !crate::client::server_running(&crate::ipc::pipe_name(socket)) {
        bail!("no keepane server is running (socket '{socket}'): start a session first, then `keepane web`");
    }
    let ip = o.bind.unwrap_or_else(lan_ip);
    let key = if o.keep_key { kept_key()? } else { new_key()? };
    let listener = TcpListener::bind((ip, o.port))
        .await
        .with_context(|| format!("listen on {ip}:{} (in use? another --port)", o.port))?;
    let url = format!("http://{ip}:{}/#k={key}", o.port);
    println!("{}", qr_text(&url)?);
    println!("Scan with the phone's camera, or open: {url}");
    if ip.is_loopback() {
        println!("(No network address found: this works on this machine only; --bind picks one.)");
    }
    println!(
        "Anyone with this code can {} your panes. Ctrl+C stops it{}.",
        if o.read_only { "see" } else { "see and type into" },
        if o.keep_key { "; the key stays for next time (--keep-key)" } else { "; the next start makes a new key" }
    );
    println!("Windows may ask to let keepane onto the network: allow it for private networks.");
    serve(listener, Arc::new(State::new(socket, &key, o.read_only, true))).await
}

/// Answer HTTP on `listener` until the process ends.
pub async fn serve(listener: TcpListener, state: Arc<State>) -> Result<i32> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = connection(stream, peer.ip(), &state).await;
        });
    }
}

/// The address the phone should use: the one this machine reaches the
/// network through. Nothing is sent: connecting a UDP socket only picks the
/// route.
fn lan_ip() -> IpAddr {
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
fn kept_key() -> Result<String> {
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
    let (mut length, mut key) = (0usize, None);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().context("bad Content-Length")?;
        } else if name.eq_ignore_ascii_case("x-keepane-key") {
            key = Some(value.to_string());
        }
    }
    if length > MAX_BODY {
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
    Ok(Some(Request { method, path: percent_decode(path), query, key, body }))
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
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response { status, content_type, body: body.into() }
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
        return watch(&mut stream, state, pane, history).await;
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
async fn watch(stream: &mut TcpStream, state: &State, pane: &str, history: u32) -> Result<()> {
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
            if let Ok(json) = screen_json(&state.socket, pane, history).await {
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
/// milliseconds, null where unknown).
async fn screen_json(socket: &str, pane: &str, history: u32) -> Result<String, (u16, String)> {
    let q = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        crate::client::query(socket, &argv).await
    };
    let mut argv = vec!["capture-pane".into(), "-p".into(), "-e".into(), "-t".into(), pane.to_string()];
    if history > 0 {
        argv.extend(["-S".into(), format!("-{history}")]);
    }
    let text = match q(argv).await {
        Ok((0, out, _)) => out,
        Ok((_, _, err)) => return Err((404, err.trim().to_string())),
        Err(e) => return Err((500, format!("{e:#}"))),
    };
    let marks = q(vec!["list-marks".into(), "-t".into(), pane.into()]).await;
    let size = q(vec!["display-message".into(), "-p".into(), "-t".into(), pane.into(), "#{history_size}".into()]).await;
    let marks = match (marks, size) {
        (Ok((0, marks, _)), Ok((0, size, _))) => marks_json(&text, &marks, history, size.trim().parse().unwrap_or(0)),
        _ => "[]".into(),
    };
    Ok(format!("{{\"text\":{},\"marks\":{marks}}}", json_str(&text)))
}

/// `list-marks` lines placed on the lines of `text`, a capture holding the
/// last `history` of `scrollback` lines above the screen. A mark whose line
/// does not read as it did (output arrived between the two questions) is
/// left out rather than put against the wrong line.
fn marks_json(text: &str, marks: &str, history: u32, scrollback: usize) -> String {
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
            out.push(format!("[{i},{},{},{}]", num(start), num(end), num(exit)));
        }
    }
    format!("[{}]", out.join(","))
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
/// The first request from each address, let in or not, is announced in the
/// terminal.
fn check_key(req: &Request, peer: IpAddr, state: &State) -> Option<Response> {
    let ok = req.key.as_deref().is_some_and(|k| same_key(k, &state.key));
    if state.announce && state.seen.lock().map(|mut s| s.insert((peer, ok))).unwrap_or(false) {
        if ok {
            println!("{peer} connected");
        } else {
            println!("{peer} refused: no key or a wrong one");
        }
    }
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
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        r.status,
        r.content_type,
        r.body.len()
    );
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
    if !req.path.starts_with("/api/") {
        return Response::text(404, "not found");
    }
    if let Some(refused) = check_key(req, peer, state) {
        return refused;
    }
    if get && matches!(req.path.as_str(), "/api/send" | "/api/action") {
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
                                  #{window_activity_flag}\t#{window_bell_flag}\t#{window_silence_flag}";
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
            match screen_json(&state.socket, pane, history).await {
                Ok(json) => Response::json(json),
                Err((status, msg)) => Response::text(status, &msg),
            }
        }
        (false, "/api/send") | (false, "/api/action") if state.read_only => Response::text(403, "read-only"),
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
            let mut argv: Vec<String> = match req.param("do") {
                Some("new-window") => vec!["new-window".into(), "-t".into(), pane.into()],
                Some("split-h") => vec!["split-window".into(), "-h".into(), "-t".into(), pane.into()],
                Some("split-v") => vec!["split-window".into(), "-v".into(), "-t".into(), pane.into()],
                Some("kill-pane") => vec!["kill-pane".into(), "-t".into(), pane.into()],
                _ => return Response::text(400, &format!("do: one of {}", ACTIONS.join(", "))),
            };
            // A new pane starts where the pane it came from is, not where
            // `keepane web` was started (the directory this client would give).
            if argv[0] != "kill-pane"
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
                Ok((_, _, err)) => Response::text(404, err.trim()),
                Err(e) => Response::text(500, &format!("{e:#}")),
            }
        }
        _ => Response::text(404, "not found"),
    }
}

fn is_pane_id(p: &str) -> bool {
    p.len() > 1 && p.starts_with('%') && p[1..].bytes().all(|b| b.is_ascii_digit())
}

/// A button's key: one of the named keys, or Ctrl with a letter (`C-c`).
fn is_named_key(k: &str) -> bool {
    KEYS.contains(&k) || (k.len() == 3 && k.starts_with("C-") && k.as_bytes()[2].is_ascii_lowercase())
}

/// The list-panes lines (tab-separated, in FIELDS order) as a JSON array.
fn panes_json(out: &str) -> String {
    let items: Vec<String> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 15 {
                return None;
            }
            let num = |s: &str| s.parse::<u64>().unwrap_or(0);
            // The window's alerts, as the status line marks them: it printed
            // (#), rang (!), or went quiet (~) while nobody looked.
            Some(format!(
                "{{\"id\":{},\"session\":{},\"window\":{},\"windowName\":{},\"pane\":{},\"command\":{},\
                 \"active\":{},\"windowActive\":{},\"cols\":{},\"rows\":{},\"dead\":{},\"attached\":{},\
                 \"activity\":{},\"bell\":{},\"silence\":{}}}",
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
                f[14] == "1"
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

    fn req(method: &str, path: &str, key: Option<&str>) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            query: Vec::new(),
            key: key.map(String::from),
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
    }

    #[tokio::test]
    async fn the_key_guards_everything_but_the_page() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        // No server behind this state: every answer here comes before one
        // would be asked.
        let s = State::new("keepane-web-test-no-server", "sekrit", false, false);
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
        let ro = State::new("keepane-web-test-no-server", "sekrit", true, false);
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
        assert_eq!(marks_json(text, marks, 300, 2), "[[0,1000,1500,0],[2,2000,2100,1]]");
        // With less history asked for than there is, lines shift with it.
        assert_eq!(marks_json("PS> \x1b[1mbad\nerr\nPS>", marks, 0, 2), "[[0,2000,2100,1]]");
        // Unknown times and codes are null; a torn line is skipped.
        assert_eq!(marks_json("PS> x", "0 - 5 - PS> x\nnonsense", 0, 0), "[[0,null,5,null]]");
        assert_eq!(without_escapes("\x1b[38;2;1;2;3ma\x1b[0mb"), "ab");
    }

    #[test]
    fn the_code_and_the_pane_list() {
        let qr = qr_text("http://192.168.1.23:7681/#k=AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert!(qr.lines().count() > 10 && qr.contains('█'), "{qr}");
        let json = panes_json("%3\tdev\t0\tbuild\t1\tcargo\t1\t0\t80\t24\t0\t1\t1\t0\t1\nshort line\n");
        assert_eq!(
            json,
            r#"[{"id":"%3","session":"dev","window":0,"windowName":"build","pane":1,"command":"cargo","active":true,"windowActive":false,"cols":80,"rows":24,"dead":false,"attached":true,"activity":true,"bell":false,"silence":true}]"#
        );
        assert_eq!(panes_json(""), "[]");
    }
}
