//! `keepane link`: panes on different machines passing messages
//! (docs/design/link.md). What is here holds no server state: remote
//! addresses, a server's key pair, what is signed and how it is checked,
//! the table of machines allowed in, and the plain HTTP call one server
//! makes to another. The server keeps the state (`server/link_host.rs`);
//! the other end's answers come in through `keepane web`'s listener.
//!
//! Like SSH: each server has an Ed25519 key pair that never leaves it, and
//! a table of the machines it lets send messages in (their public keys, the
//! address they were last reached at, and whether their messages may run as
//! commands). Pairing puts each side's key in the other's table, once, with
//! the other side's web key as the proof; from then on every request is
//! signed by its sender and every answer by the one answering.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What a request says it speaks (`x-keepane-link`); an answer to another
/// number says which versions each side runs.
pub const PROTOCOL: u32 = 1;
/// How far a request's time may be from this machine's clock, in seconds.
pub const MAX_SKEW: i64 = 120;
/// How long a request's nonce is remembered: longer than the window a
/// request is taken in, so none is taken twice.
const NONCE_KEPT: i64 = 2 * MAX_SKEW;
/// Nonces remembered at most (a peer sending faster than this over the
/// window loses the oldest first, which only matters for a replay of those).
const NONCES_MAX: usize = 100_000;
/// Connecting to another machine: one that does not answer by then is off.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The largest answer read back.
const MAX_ANSWER: usize = 4 * 1024 * 1024;

pub const H_PROTOCOL: &str = "x-keepane-link";
/// The keepane version at the other end, for the message when the
/// protocols differ.
pub const H_VERSION: &str = "x-keepane-version";
pub const H_FROM: &str = "x-keepane-from";
pub const H_PORT: &str = "x-keepane-port";
pub const H_TIME: &str = "x-keepane-time";
pub const H_NONCE: &str = "x-keepane-nonce";
pub const H_SIGN: &str = "x-keepane-sign";
/// Pairing: the HMAC made with the other side's web key.
pub const H_PROOF: &str = "x-keepane-proof";

/// Where a server's key and table live: `KEEPANE_LINK_DIR`, else beside
/// moved saved sessions or history (the tests move those, and so stay out
/// of the real directory), else `%LOCALAPPDATA%\keepane\link`; one
/// directory per socket, since each server is an end of its own on the
/// network (it has its own web port).
pub fn dir(socket: &str) -> PathBuf {
    let base = if let Some(d) = crate::legacy::var_os("KEEPANE_LINK_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(d)
    } else if let Some(d) = crate::legacy::var_os("KEEPANE_SESSIONS_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(d).join("link")
    } else if let Some(d) = crate::legacy::var_os("KEEPANE_HISTORY_DIR").filter(|d| !d.is_empty()) {
        let mut d = d;
        d.push("-link");
        PathBuf::from(d)
    } else {
        crate::logger::log_dir().join("link")
    };
    base.join(crate::histlog::safe_name(socket))
}

/// A Tailscale address: 100.64.0.0/10 (CGNAT space, which Tailscale uses)
/// or fd7a:115c:a1e0::/48.
pub fn tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// The addresses `keepane web` listens on: the one given, else the one
/// this machine reaches the network through and its Tailscale addresses.
pub fn listen_addrs(bind: Option<IpAddr>) -> Vec<IpAddr> {
    if let Some(b) = bind {
        return vec![b];
    }
    let mut out = vec![crate::web::lan_ip()];
    for ip in crate::platform::netif::addresses() {
        if tailscale(ip) && !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// `keepane link <what> ...`: each is the server command `link-<what>`
/// with the same words (`id`, `add`, `trust`, `list`, `panes`, `allow`,
/// `remove`, `rekey`).
pub async fn run(socket: &str, args: &[String]) -> Result<i32> {
    const WORDS: &[&str] = &["id", "add", "trust", "list", "panes", "allow", "remove", "rekey"];
    const USAGE: &str = "usage: keepane link id | add <the other machine's web address> | \
                         trust <host:port> <key> [--shell] | list | panes <host:port> | \
                         allow <host:port> --shell|--no-shell | remove <host:port> | rekey";
    let Some(what) = args.first().map(String::as_str).filter(|w| WORDS.contains(w)) else {
        bail!("{USAGE}");
    };
    if !crate::client::server_running(&crate::ipc::pipe_name(socket)) {
        bail!("no keepane server is running (socket '{socket}'): start a session first, then `keepane web`");
    }
    let argv: Vec<String> = std::iter::once(format!("link-{what}")).chain(args[1..].iter().cloned()).collect();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    let (code, out, mut err) = crate::client::query(socket, &argv).await?;
    // A server from before this (still running after an upgrade) does not
    // know these commands: say why, and what to do.
    if code != 0
        && err.contains("unknown command: link-")
        && let Some(v) = crate::client::server_version(socket).await
        && v != env!("CARGO_PKG_VERSION")
    {
        err.push_str(&format!("note: {}\n", crate::client::mismatch_note(&v)));
    }
    print!("{out}");
    eprint!("{err}");
    Ok(code)
}

/// `host:port/rest` split into the machine and what names the pane there.
/// None for a target of this machine: those never hold a `/` after a
/// `host:port` whose port is a number.
pub fn split_remote(s: &str) -> Option<(&str, &str)> {
    let (machine, rest) = s.split_once('/')?;
    valid_hostport(machine).then_some((machine, rest))
}

/// `host:port`, `[v6]:port`: a name or address, and a port from 1 up.
pub fn valid_hostport(s: &str) -> bool {
    let (host, port) = if let Some(r) = s.strip_prefix('[') {
        let Some((inner, port)) = r.split_once("]:") else { return false };
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        (inner, port)
    } else {
        let Some((host, port)) = s.rsplit_once(':') else { return false };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
            return false;
        }
        (host, port)
    };
    !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0) && port.bytes().all(|b| b.is_ascii_digit())
}

/// How an address and a port are written together (`[v6]:port`).
pub fn hostport(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        IpAddr::V4(v4) => format!("{v4}:{port}"),
    }
}

/// What `link add` is given: the address the phone scans,
/// `http://host:port/#k=KEY`, read as the machine and its web key.
pub fn parse_invite(url: &str) -> Result<(String, String), String> {
    let bad = || format!("link add: '{url}' is not a `keepane web` address (http://host:port/#k=...)");
    let rest = url.trim().strip_prefix("http://").ok_or_else(bad)?;
    let (machine, after) = rest.split_once('/').ok_or_else(bad)?;
    let key = after.strip_prefix("#k=").ok_or_else(bad)?;
    if !valid_hostport(machine) || key.len() != 22 || !key.bytes().all(is_b64url) {
        return Err(bad());
    }
    Ok((machine.to_string(), key.to_string()))
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64, URL-safe alphabet, no padding.
pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// `b64` back; None for anything it would not have written.
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let val = |c: u8| ALPHABET.iter().position(|a| *a == c).map(|v| v as u32);
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // Bits past the last byte are zero in what `b64` writes: what does
    // not come back the same was not written by it.
    (b64(&out) == s).then_some(out)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104), for the pairing's proof of the web key.
pub fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |x: u8| k.map(|b| b ^ x);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// Compare without stopping at the first difference.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A server's key pair.
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    /// The key kept in `dir`, or a new one kept there from now, readable by
    /// this user alone.
    pub fn load_or_make(dir: &Path) -> Result<Identity> {
        let path = dir.join("key");
        // Only a key that is not there is made: one that cannot be read or
        // is damaged is an error, never replaced (that would void every
        // pairing made with it without a word).
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::make(dir),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let seed =
            std::str::from_utf8(&bytes).ok().and_then(|t| unb64(t.trim())).filter(|s| s.len() == 32).with_context(
                || format!("{} is not a keepane link key (move it away to make a new one)", path.display()),
            )?;
        Ok(Identity { key: SigningKey::from_bytes(&seed.try_into().expect("32 bytes")) })
    }

    /// A new key pair in `dir`, in place of any before it.
    pub fn make(dir: &Path) -> Result<Identity> {
        let mut seed = [0u8; 32];
        crate::platform::random::fill(&mut seed)?;
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
        let path = dir.join("key");
        write_private(&path, &format!("{}\n", b64(&seed)))?;
        Ok(Identity { key: SigningKey::from_bytes(&seed) })
    }

    /// The public key, as the other side's table holds it.
    pub fn public(&self) -> String {
        b64(self.key.verifying_key().as_bytes())
    }

    pub fn sign(&self, text: &str) -> String {
        b64(&self.key.sign(text.as_bytes()).to_bytes())
    }
}

/// Whether `sig` is `public`'s signature of `text`.
pub fn verify(public: &str, text: &str, sig: &str) -> bool {
    let Some(pk) = unb64(public).and_then(|b| <[u8; 32]>::try_from(b).ok()) else { return false };
    let Some(sig) = unb64(sig).and_then(|b| <[u8; 64]>::try_from(b).ok()) else { return false };
    let Ok(pk) = VerifyingKey::from_bytes(&pk) else { return false };
    pk.verify_strict(text.as_bytes(), &Signature::from_bytes(&sig)).is_ok()
}

/// A public key well formed: 32 bytes that make a point.
pub fn valid_public(s: &str) -> bool {
    unb64(s).and_then(|b| <[u8; 32]>::try_from(b).ok()).is_some_and(|b| VerifyingKey::from_bytes(&b).is_ok())
}

/// A short form of a public key to show, `SHA256:` and the first bytes of
/// its digest (as `ssh-keygen -l` does, shorter).
pub fn fingerprint(public: &str) -> String {
    format!("SHA256:{}", &b64(&Sha256::digest(public.as_bytes()))[..16])
}

/// A file only this user may read and write.
fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    crate::platform::private::restrict(path).with_context(|| format!("keep {} to this user", path.display()))
}

/// What a request signs: its protocol, method and path, who sends it (key
/// and the port its answers come back to), whom it is for (their key: a
/// request taken off the wire is no good at another machine), when, its
/// nonce, and a digest of its body.
#[allow(clippy::too_many_arguments)]
pub fn request_text(
    method: &str,
    path: &str,
    from: &str,
    port: u16,
    to: &str,
    time: i64,
    nonce: &str,
    body: &[u8],
) -> String {
    format!("keepane-link/{PROTOCOL}\n{method}\n{path}\n{from}\n{port}\n{to}\n{time}\n{nonce}\n{}", sha256_hex(body))
}

/// What an answer signs: the request's nonce (so it answers that request
/// and no other), its status, a digest of its body.
pub fn answer_text(nonce: &str, status: u16, body: &[u8]) -> String {
    format!("keepane-link-answer/{PROTOCOL}\n{nonce}\n{status}\n{}", sha256_hex(body))
}

/// What pairing proves with the web key: the key asking in, its port, the
/// time and nonce; the answer proves the same with the key it gives back.
pub fn pair_text(from: &str, port: u16, time: i64, nonce: &str) -> String {
    format!("keepane-link-pair/{PROTOCOL}\n{from}\n{port}\n{time}\n{nonce}")
}

pub fn pair_answer_text(nonce: &str, key: &str) -> String {
    format!("keepane-link-paired/{PROTOCOL}\n{nonce}\n{key}")
}

/// Now, in Unix seconds.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A new nonce: 128 random bits.
pub fn nonce() -> Result<String> {
    let mut b = [0u8; 16];
    crate::platform::random::fill(&mut b)?;
    Ok(b64(&b))
}

/// None when `time` is close enough to `now`; the refusal otherwise.
pub fn check_time(time: i64, now: i64) -> Option<String> {
    let off = time - now;
    (off.abs() > MAX_SKEW).then(|| {
        format!(
            "the clocks of the two machines are {}s apart (more than {MAX_SKEW}s): set both to the network's time",
            off.abs()
        )
    })
}

/// Nonces seen lately, so a request is taken once.
#[derive(Default)]
pub struct Nonces {
    seen: HashMap<String, i64>,
}

impl Nonces {
    /// True the first time `nonce` comes (and it is remembered); false again.
    pub fn fresh(&mut self, nonce: &str, now: i64) -> bool {
        if self.seen.len() >= NONCES_MAX || self.seen.len().is_multiple_of(1024) {
            self.seen.retain(|_, at| now - *at <= NONCE_KEPT);
        }
        if self.seen.len() >= NONCES_MAX
            && let Some(oldest) = self.seen.iter().min_by_key(|(_, at)| **at).map(|(n, _)| n.clone())
        {
            self.seen.remove(&oldest);
        }
        if nonce.is_empty() || self.seen.contains_key(nonce) {
            return false;
        }
        self.seen.insert(nonce.to_string(), now);
        true
    }
}

/// A machine allowed to send messages here, and where it was last reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub key: String,
    /// `host:port`: where its answers and replies go. Updated whenever a
    /// request of its comes from somewhere else (its key says who it is).
    pub addr: String,
    /// Its messages may run in `shell` panes here.
    pub shell: bool,
}

const TABLE_HEAD: &str = "# keepane link: the machines allowed to send messages to this server's panes\n\
                          # (docs/design/link.md). One a line: public key, address, and `shell` when\n\
                          # its messages may run as commands.\n";

/// The table's lines back; comments, blank and malformed lines skipped.
pub fn parse_table(text: &str) -> Vec<Peer> {
    let mut out: Vec<Peer> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split_whitespace();
        let (Some(key), Some(addr)) = (f.next(), f.next()) else { continue };
        if !valid_public(key) || !valid_hostport(addr) || out.iter().any(|p| p.key == key) {
            continue;
        }
        let shell = f.any(|w| w == "shell");
        out.push(Peer { key: key.to_string(), addr: addr.to_string(), shell });
    }
    out
}

pub fn format_table(peers: &[Peer]) -> String {
    let mut s = TABLE_HEAD.to_string();
    for p in peers {
        s.push_str(&format!("{} {}{}\n", p.key, p.addr, if p.shell { " shell" } else { "" }));
    }
    s
}

pub fn load_table(dir: &Path) -> Vec<Peer> {
    std::fs::read_to_string(dir.join("authorized")).map(|t| parse_table(&t)).unwrap_or_default()
}

pub fn save_table(dir: &Path, peers: &[Peer]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    write_private(&dir.join("authorized"), &format_table(peers))
}

/// An answer from the other machine.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

/// One HTTP request to `addr` (`host:port`), the answer read to its end.
/// `longest`: how long the whole may take (a `-w` wait on the other side).
pub async fn call(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
    longest: Duration,
) -> Result<Answer> {
    let mut stream = match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => bail!("{addr} is not reachable: {e}"),
        Err(_) => bail!("{addr} did not answer within {}s", CONNECT_TIMEOUT.as_secs()),
    };
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let exchange = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > MAX_ANSWER {
                bail!("the answer from {addr} is too large");
            }
        }
        anyhow::Ok(buf)
    };
    let buf = match tokio::time::timeout(longest, exchange).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => bail!("{addr} dropped the connection: {e}"),
        Err(_) => bail!("{addr} did not answer within {}s", longest.as_secs()),
    };
    parse_answer(&buf).with_context(|| format!("{addr} answered with something that is not HTTP"))
}

fn parse_answer(buf: &[u8]) -> Result<Answer> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").context("no end of headers")?;
    let head = std::str::from_utf8(&buf[..end])?;
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).context("status")?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let mut body = buf[end + 4..].to_vec();
    if let Some(n) = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()) {
        if body.len() < n {
            bail!("the answer was cut short");
        }
        body.truncate(n);
    }
    Ok(Answer { status, headers, body })
}

/// A message's body on its way to another machine.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SendBody {
    /// The pane there, as its machine reads a target (`$1:@3.%7`, `%name`).
    pub to: String,
    pub text: String,
    /// Who sends it: the sender's full address on its own machine, or `user`.
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub hop: u32,
    /// Its number and task on the sender's machine, for the answers.
    pub id: u64,
    pub task: u64,
    /// The message it answers, numbered as the receiver numbers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    /// The receiver's task it carries on, numbered as the receiver does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<u64>,
    /// `-w`: wait there until it is delivered, this many seconds at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,
}

/// What the receiver says of it.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SendAnswer {
    /// Its number there.
    pub id: u64,
    /// The pane's full address there.
    pub to: String,
    /// The pane's work mode there when it came.
    pub via: String,
    /// Where it stands there, as `send-message` says it.
    pub stand: String,
    /// Its stage there (`queued`, `delivered`, ...): a `-w` that ran out
    /// with it still queued is a time-out here, as it is for a local one.
    #[serde(default)]
    pub stage: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_targets_are_told_from_local_ones() {
        assert_eq!(split_remote("100.64.0.3:7681/$1:@3.%7"), Some(("100.64.0.3:7681", "$1:@3.%7")));
        assert_eq!(split_remote("mac.tail1234.ts.net:7681/%builder"), Some(("mac.tail1234.ts.net:7681", "%builder")));
        assert_eq!(split_remote("[fd7a:115c::3]:7681/%7"), Some(("[fd7a:115c::3]:7681", "%7")));
        for local in ["$1:@3.%7", "work:2.1", "%builder", "work:src/lib", "a:0/x", "host:/x", "[zz]:1/x", "h:70000/x"] {
            assert_eq!(split_remote(local), None, "{local}");
        }
        assert!(valid_hostport("localhost:1") && !valid_hostport("localhost") && !valid_hostport(":80"));
    }

    #[test]
    fn tailscale_addresses_are_listened_on_beside_the_lan_one() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for yes in ["100.64.0.3", "100.127.255.255", "fd7a:115c:a1e0::1"] {
            assert!(tailscale(ip(yes)), "{yes}");
        }
        for no in ["100.63.255.255", "100.128.0.0", "10.0.0.1", "fd7b:115c:a1e0::1", "fd7a:115c:a1e1::1"] {
            assert!(!tailscale(ip(no)), "{no}");
        }
        assert_eq!(listen_addrs(Some(ip("192.168.1.5"))), vec![ip("192.168.1.5")], "--bind: that one alone");
        let all = listen_addrs(None);
        assert_eq!(all[0], crate::web::lan_ip());
        assert!(all[1..].iter().all(|a| tailscale(*a)), "{all:?}");
        assert_eq!(hostport(ip("fd7a:115c:a1e0::1"), 7681), "[fd7a:115c:a1e0::1]:7681");
    }

    #[test]
    fn an_invite_is_the_address_the_phone_scans() {
        assert_eq!(
            parse_invite("http://100.64.0.3:7681/#k=AAAAAAAAAAAAAAAAAAAAAA"),
            Ok(("100.64.0.3:7681".into(), "AAAAAAAAAAAAAAAAAAAAAA".into()))
        );
        assert!(parse_invite("http://100.64.0.3:7681/").is_err());
        assert!(parse_invite("https://100.64.0.3:7681/#k=AAAAAAAAAAAAAAAAAAAAAA").is_err());
        assert!(parse_invite("http://100.64.0.3:7681/#k=short").is_err());
    }

    #[test]
    fn base64_goes_there_and_back() {
        for n in 0..70 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + n) as u8).collect();
            assert_eq!(unb64(&b64(&bytes)), Some(bytes), "{n} bytes");
        }
        assert_eq!(b64(b"\xfb\xff"), "-_8");
        assert_eq!(unb64("A"), None);
        assert_eq!(unb64("A*=="), None);
        // Stray bits past the last byte: not what b64 writes.
        assert_eq!(unb64("-_9"), None);
    }

    /// RFC 4231 test cases 1 and 6 (a key longer than a block).
    #[test]
    fn hmac_is_rfc_4231() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_signature_holds_for_its_text_and_key_only() {
        let dir = std::env::temp_dir().join(format!("keepane-link-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Identity::load_or_make(&dir).unwrap();
        let again = Identity::load_or_make(&dir).unwrap();
        assert_eq!(a.public(), again.public(), "kept");
        assert!(valid_public(&a.public()) && !valid_public("abc"));
        let text = request_text("POST", "/link/send", &a.public(), 7681, "them", 1000, "n1", b"body");
        let sig = a.sign(&text);
        assert!(verify(&a.public(), &text, &sig));
        assert!(!verify(&a.public(), &text.replace("/link/send", "/link/sent"), &sig), "the text changed");
        let other = Identity::make(&dir.join("b")).unwrap();
        assert!(!verify(&other.public(), &text, &sig), "another key");
        assert!(!verify(&a.public(), &text, "junk"));
        assert!(fingerprint(&a.public()).starts_with("SHA256:"));
        let made = Identity::make(&dir).unwrap();
        assert_ne!(made.public(), a.public(), "rekey makes a new one");
        // A damaged key, text or not, is an error and stays as it was.
        for junk in [&b"not a key"[..], &[0xff, 0xfe, 0x00, 0x80][..]] {
            std::fs::write(dir.join("key"), junk).unwrap();
            let e = Identity::load_or_make(&dir).err().expect("an error").to_string();
            assert!(e.contains("is not a keepane link key"), "{e}");
            assert_eq!(std::fs::read(dir.join("key")).unwrap(), junk, "not replaced");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_time_and_the_nonce_let_a_request_in_once() {
        assert_eq!(check_time(1000, 1000 + MAX_SKEW), None);
        assert!(check_time(1000, 1001 + MAX_SKEW).unwrap().contains("clocks"));
        let mut n = Nonces::default();
        assert!(n.fresh("a", 0) && !n.fresh("a", 1) && n.fresh("b", 1) && !n.fresh("", 1));
        // Long after, the old ones are forgotten (by then their time is refused anyway).
        for i in 0..2048 {
            n.fresh(&format!("x{i}"), 10_000);
        }
        assert!(!n.seen.contains_key("a"));
    }

    /// A key is base64url, so one in 64 starts with `-`: it is still a key,
    /// not a flag, to `link-trust` (only `--shell` is one).
    #[test]
    fn a_key_that_starts_with_a_dash_is_a_key() {
        let key = (0u8..=255)
            .map(|n| Identity { key: SigningKey::from_bytes(&[n; 32]) }.public())
            .chain((0u16..10_000).map(|n| {
                let mut seed = [0u8; 32];
                seed[..2].copy_from_slice(&n.to_le_bytes());
                seed[2] = 7;
                Identity { key: SigningKey::from_bytes(&seed) }.public()
            }))
            .find(|k| k.starts_with('-'))
            .expect("one in 64 keys starts with -");
        let parse = |w: &[&str]| crate::command::parse(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let want = |shell| crate::command::Cmd::LinkTrust { addr: "10.0.0.1:7681".into(), key: key.clone(), shell };
        assert_eq!(parse(&["link-trust", "10.0.0.1:7681", &key]), Ok(want(false)));
        assert_eq!(parse(&["link-trust", "10.0.0.1:7681", &key, "--shell"]), Ok(want(true)));
        assert_eq!(parse(&["link-trust", "--shell", "10.0.0.1:7681", &key]), Ok(want(true)));
        // Written back, it reads the same.
        let line = want(true).to_string();
        let words = crate::command::tokenize(&line).unwrap();
        assert_eq!(crate::command::parse(&words), Ok(want(true)), "{line}");
        // Anything else is a word, checked as one: a flag mistyped is not a key.
        let e = parse(&["link-trust", "10.0.0.1:7681", &key, "--shel"]).unwrap_err();
        assert!(e.contains("host:port and the machine's public key"), "{e}");
    }

    #[test]
    fn the_table_reads_back_and_skips_what_is_not_a_line_of_it() {
        let dir = std::env::temp_dir().join(format!("keepane-link-table-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let k1 = Identity::make(&dir.join("1")).unwrap().public();
        let k2 = Identity::make(&dir.join("2")).unwrap().public();
        let peers = vec![
            Peer { key: k1.clone(), addr: "100.64.0.3:7681".into(), shell: true },
            Peer { key: k2.clone(), addr: "[fd7a::1]:7681".into(), shell: false },
        ];
        save_table(&dir, &peers).unwrap();
        assert_eq!(load_table(&dir), peers);
        let hand = format!("# mine\n\n{k1} 1.2.3.4:5 shell\nnot a line\n{k2} nothost\n{k1} 9.9.9.9:9\n");
        assert_eq!(parse_table(&hand), vec![Peer { key: k1, addr: "1.2.3.4:5".into(), shell: true }]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_answer_is_read_to_its_length() {
        let a =
            parse_answer(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 3\r\nX-Keepane-Sign: s\r\n\r\nno!extra").unwrap();
        assert_eq!((a.status, a.text().as_str(), a.header("x-keepane-sign")), (403, "no!", Some("s")));
        assert!(parse_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort").is_err());
    }
}
