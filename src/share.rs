//! `keepane share`: a file or a folder given out by a link (docs/design/share.md).
//!
//! The server keeps what is shared (`server::share_host`); the phones' page
//! answers `/s/<token>` with `serve` here: a file as it is (one range of it,
//! for a download carried on), a folder as a zip written while it is read.
//! Whatever is shared is sent as a download, never shown: a page of HTML
//! shared would otherwise run on the phone page's own address.

use anyhow::Result;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// How long a share lasts unless `--for` says.
pub const DEFAULT_FOR: Duration = Duration::from_secs(24 * 3600);
/// `--for` at most.
pub const LONGEST: Duration = Duration::from_secs(365 * 24 * 3600);
/// What a link that ended (or never was) is answered.
pub const ENDED: &str = "this link has ended";
/// Read and sent at a time.
const CHUNK: usize = 256 * 1024;

/// `--for`: `90s`, `30m`, `2h`, `7d`; `0` is no end (None).
pub fn parse_for(s: &str) -> Result<Option<Duration>, String> {
    if s == "0" {
        return Ok(None);
    }
    let bad = || format!("--for: '{s}' is not a time (30m, 2h, 7d; 0 for no end)");
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().map_err(|_| bad())?;
    let each = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(bad()),
    };
    let secs = n.checked_mul(each).filter(|s| *s > 0).ok_or_else(bad)?;
    if secs > LONGEST.as_secs() {
        return Err(format!("--for: {s} is longer than 365d (0 for no end)"));
    }
    Ok(Some(Duration::from_secs(secs)))
}

/// A time as `--for` takes it, in the largest unit it is a whole number of.
pub fn for_text(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        _ if s >= 86400 && s.is_multiple_of(86400) => format!("{}d", s / 86400),
        _ if s >= 3600 && s.is_multiple_of(3600) => format!("{}h", s / 3600),
        _ if s >= 60 && s.is_multiple_of(60) => format!("{}m", s / 60),
        _ => format!("{s}s"),
    }
}

/// A share's token: 128 random bits, URL-safe (`web::new_key`'s kind).
pub fn valid_token(t: &str) -> bool {
    t.len() == 22 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-32 (the zip one, IEEE), fed a piece at a time.
pub struct Crc(u32);

impl Default for Crc {
    fn default() -> Crc {
        Crc(0xFFFF_FFFF)
    }
}

impl Crc {
    pub fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = CRC_TABLE[((self.0 ^ u32::from(b)) & 0xFF) as usize] ^ (self.0 >> 8);
        }
    }

    pub fn value(&self) -> u32 {
        !self.0
    }
}

/// A time as a zip keeps it: MS-DOS time and date, local, 1980 to 2107.
pub fn dos_time(t: SystemTime) -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let d: chrono::DateTime<chrono::Local> = t.into();
    let year = d.year();
    if year < 1980 {
        return (0, (1 << 5) | 1);
    }
    let year = year.min(2107) as u16;
    let time = (d.hour() as u16) << 11 | (d.minute() as u16) << 5 | (d.second() as u16 / 2);
    let date = (year - 1980) << 9 | (d.month() as u16) << 5 | d.day() as u16;
    (time, date)
}

/// Beyond what 32 bits hold: ZIP64 for that.
const MAX32: u64 = 0xFFFF_FFFF;

/// What the central directory says of an entry.
struct Entry {
    name: Vec<u8>,
    dos: (u16, u16),
    crc: u32,
    size: u64,
    offset: u64,
    dir: bool,
    /// Its local header carries ZIP64 sizes (and its data descriptor 64 bits).
    big: bool,
}

/// A zip written front to back, never sought in: STORED entries, each one's
/// CRC and size after its data (general purpose bit 3), names in UTF-8
/// (bit 11), ZIP64 where 32 bits do not hold a size, an offset or the count.
/// Each call gives the bytes to write next and counts them.
pub struct Zip {
    offset: u64,
    entries: Vec<Entry>,
    open: Option<Entry>,
    /// Every entry and the end in ZIP64 form (the tests: that form, small).
    all64: bool,
}

fn put16(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, n: u64) {
    v.extend_from_slice(&n.to_le_bytes());
}

impl Zip {
    pub fn new(all64: bool) -> Zip {
        Zip { offset: 0, entries: Vec::new(), open: None, all64 }
    }

    /// A folder's entry (`name` ends in `/`): no data.
    pub fn folder(&mut self, name: &str, dos: (u16, u16)) -> Vec<u8> {
        let e =
            Entry { name: name.as_bytes().to_vec(), dos, crc: 0, size: 0, offset: self.offset, dir: true, big: false };
        let mut v = Vec::new();
        put32(&mut v, 0x0403_4b50);
        put16(&mut v, 20);
        put16(&mut v, 0x0800);
        put16(&mut v, 0);
        put16(&mut v, dos.0);
        put16(&mut v, dos.1);
        put32(&mut v, 0);
        put32(&mut v, 0);
        put32(&mut v, 0);
        put16(&mut v, e.name.len() as u16);
        put16(&mut v, 0);
        v.extend_from_slice(&e.name);
        self.offset += v.len() as u64;
        self.entries.push(e);
        v
    }

    /// A file's local header; `size` is at most what will follow (whether
    /// ZIP64 is needed is decided by it).
    pub fn begin(&mut self, name: &str, dos: (u16, u16), size: u64) -> Vec<u8> {
        let big = self.all64 || size >= MAX32;
        let e = Entry { name: name.as_bytes().to_vec(), dos, crc: 0, size: 0, offset: self.offset, dir: false, big };
        let mut v = Vec::new();
        put32(&mut v, 0x0403_4b50);
        put16(&mut v, if big { 45 } else { 20 });
        put16(&mut v, 0x0808);
        put16(&mut v, 0);
        put16(&mut v, dos.0);
        put16(&mut v, dos.1);
        put32(&mut v, 0);
        let sizes = if big { u32::MAX } else { 0 };
        put32(&mut v, sizes);
        put32(&mut v, sizes);
        put16(&mut v, e.name.len() as u16);
        put16(&mut v, if big { 20 } else { 0 });
        v.extend_from_slice(&e.name);
        if big {
            put16(&mut v, 0x0001);
            put16(&mut v, 16);
            put64(&mut v, 0);
            put64(&mut v, 0);
        }
        self.offset += v.len() as u64;
        self.open = Some(e);
        v
    }

    /// The file's data went out: `size` bytes, this CRC. Its data descriptor.
    pub fn end(&mut self, crc: u32, size: u64) -> Vec<u8> {
        let mut e = self.open.take().expect("begin before end");
        self.offset += size;
        e.crc = crc;
        e.size = size;
        let mut v = Vec::new();
        put32(&mut v, 0x0807_4b50);
        put32(&mut v, crc);
        if e.big {
            put64(&mut v, size);
            put64(&mut v, size);
        } else {
            put32(&mut v, size as u32);
            put32(&mut v, size as u32);
        }
        self.offset += v.len() as u64;
        self.entries.push(e);
        v
    }

    /// The central directory and the end records.
    pub fn finish(&mut self) -> Vec<u8> {
        let start = self.offset;
        let mut v = Vec::new();
        for e in &self.entries {
            let sizes64 = e.big || e.size >= MAX32;
            let offset64 = self.all64 || e.offset >= MAX32;
            let mut extra = Vec::new();
            if sizes64 {
                put64(&mut extra, e.size);
                put64(&mut extra, e.size);
            }
            if offset64 {
                put64(&mut extra, e.offset);
            }
            let need = if sizes64 || offset64 { 45 } else { 20 };
            put32(&mut v, 0x0201_4b50);
            put16(&mut v, 45);
            put16(&mut v, need);
            put16(&mut v, if e.dir { 0x0800 } else { 0x0808 });
            put16(&mut v, 0);
            put16(&mut v, e.dos.0);
            put16(&mut v, e.dos.1);
            put32(&mut v, e.crc);
            let size = if sizes64 { u32::MAX } else { e.size as u32 };
            put32(&mut v, size);
            put32(&mut v, size);
            put16(&mut v, e.name.len() as u16);
            put16(&mut v, if extra.is_empty() { 0 } else { extra.len() as u16 + 4 });
            put16(&mut v, 0);
            put16(&mut v, 0);
            put16(&mut v, 0);
            // MS-DOS attributes: a folder's bit.
            put32(&mut v, if e.dir { 0x10 } else { 0 });
            put32(&mut v, if offset64 { u32::MAX } else { e.offset as u32 });
            v.extend_from_slice(&e.name);
            if !extra.is_empty() {
                put16(&mut v, 0x0001);
                put16(&mut v, extra.len() as u16);
                v.extend_from_slice(&extra);
            }
        }
        let size = v.len() as u64;
        let count = self.entries.len() as u64;
        let big = self.all64 || count >= 0xFFFF || size >= MAX32 || start >= MAX32;
        if big {
            let at = start + size;
            put32(&mut v, 0x0606_4b50);
            put64(&mut v, 44);
            put16(&mut v, 45);
            put16(&mut v, 45);
            put32(&mut v, 0);
            put32(&mut v, 0);
            put64(&mut v, count);
            put64(&mut v, count);
            put64(&mut v, size);
            put64(&mut v, start);
            put32(&mut v, 0x0706_4b50);
            put32(&mut v, 0);
            put64(&mut v, at);
            put32(&mut v, 1);
        }
        put32(&mut v, 0x0605_4b50);
        put16(&mut v, 0);
        put16(&mut v, 0);
        let n = if big { u16::MAX } else { count as u16 };
        put16(&mut v, n);
        put16(&mut v, n);
        put32(&mut v, if big { u32::MAX } else { size as u32 });
        put32(&mut v, if big { u32::MAX } else { start as u32 });
        put16(&mut v, 0);
        self.offset += v.len() as u64;
        v
    }
}

/// What a `Range` header asks of a file `len` bytes long.
#[derive(Debug, PartialEq, Eq)]
pub enum Span {
    /// No range, or one not understood (several, a wrong form): all of it.
    Whole,
    /// From the first byte to the last, both included.
    Part(u64, u64),
    /// Past the end: 416.
    Outside,
}

pub fn span(header: Option<&str>, len: u64) -> Span {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else { return Span::Whole };
    if spec.contains(',') {
        return Span::Whole;
    }
    let Some((a, b)) = spec.split_once('-') else { return Span::Whole };
    let (a, b) = (a.trim(), b.trim());
    let num = |s: &str| s.parse::<u64>().ok().filter(|_| s.bytes().all(|c| c.is_ascii_digit()));
    match (a.is_empty(), b.is_empty()) {
        // The last n bytes.
        (true, false) => match num(b) {
            Some(0) => Span::Outside,
            Some(_) if len == 0 => Span::Outside,
            Some(n) => Span::Part(len.saturating_sub(n), len - 1),
            None => Span::Whole,
        },
        (false, _) => {
            let Some(first) = num(a) else { return Span::Whole };
            let last = if b.is_empty() {
                len.saturating_sub(1)
            } else {
                match num(b) {
                    Some(l) if l >= first => l.min(len.saturating_sub(1)),
                    _ => return Span::Whole,
                }
            };
            if first >= len { Span::Outside } else { Span::Part(first, last) }
        }
        (true, true) => Span::Whole,
    }
}

/// `Content-Disposition` for a download named `name`: a plain-ASCII name
/// for an old browser, the real one in UTF-8 (`filename*`). Nothing of the
/// name can end the header line.
pub fn disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| if (c.is_ascii_graphic() || c == ' ') && c != '"' && c != '\\' { c } else { '_' })
        .collect();
    let utf8: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{utf8}")
}

/// What `web-share-open` answers: what to send.
#[derive(Debug, serde::Deserialize)]
struct Opened {
    path: PathBuf,
    dir: bool,
    name: String,
}

async fn head(stream: &mut TcpStream, status: u16, headers: &[(&str, String)]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "Error",
    };
    let mut h = format!(
        "HTTP/1.1 {status} {reason}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n"
    );
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    stream.write_all(h.as_bytes()).await?;
    Ok(())
}

async fn plain(stream: &mut TcpStream, status: u16, text: &str, head_only: bool) -> Result<()> {
    let headers =
        [("Content-Type", "text/plain; charset=utf-8".to_string()), ("Content-Length", text.len().to_string())];
    head(stream, status, &headers).await?;
    if !head_only {
        stream.write_all(text.as_bytes()).await?;
    }
    stream.shutdown().await.ok();
    Ok(())
}

/// `GET` or `HEAD /s/<token>`: what it shares, sent; the server told when
/// all of it went (`web-share-sent`), a `HEAD` aside.
pub async fn serve(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    range: Option<&str>,
    peer: IpAddr,
    socket: &str,
) -> Result<()> {
    let head_only = method == "HEAD";
    if method != "GET" && !head_only {
        return plain(stream, 405, "GET", false).await;
    }
    let token = path.strip_prefix("/s/").unwrap_or("");
    if !valid_token(token) {
        return plain(stream, 404, ENDED, head_only).await;
    }
    let peer = peer.to_string();
    let opened = match crate::client::query(socket, &["web-share-open", token, &peer]).await {
        Ok((0, out, _)) => serde_json::from_str::<Opened>(&out).ok(),
        _ => None,
    };
    let Some(o) = opened else { return plain(stream, 404, ENDED, head_only).await };
    let sent =
        if o.dir { send_folder(stream, &o, head_only).await? } else { send_file(stream, &o, range, head_only).await? };
    if let Some(note) = sent {
        let _ = crate::client::query(socket, &["web-share-sent", token, &peer, &note]).await;
    }
    Ok(())
}

/// A file, or the range of it asked for. Some(note) when the end of the
/// file went out.
async fn send_file(stream: &mut TcpStream, o: &Opened, range: Option<&str>, head_only: bool) -> Result<Option<String>> {
    let mut f = match tokio::fs::File::open(&o.path).await {
        Ok(f) => f,
        Err(e) => return plain(stream, 404, &format!("{ENDED}: {e}"), head_only).await.map(|_| None),
    };
    let len = f.metadata().await?.len();
    let (status, first, count) = match span(range, len) {
        Span::Whole => (200, 0, len),
        Span::Part(a, b) => (206, a, b - a + 1),
        Span::Outside => {
            head(stream, 416, &[("Content-Range", format!("bytes */{len}")), ("Content-Length", "0".into())]).await?;
            stream.shutdown().await.ok();
            return Ok(None);
        }
    };
    let mut headers = vec![
        ("Content-Type", "application/octet-stream".to_string()),
        ("Content-Disposition", disposition(&o.name)),
        ("Content-Length", count.to_string()),
        ("Accept-Ranges", "bytes".to_string()),
    ];
    if status == 206 {
        headers.push(("Content-Range", format!("bytes {first}-{}/{len}", first + count - 1)));
    }
    head(stream, status, &headers).await?;
    if head_only {
        stream.shutdown().await.ok();
        return Ok(None);
    }
    f.seek(std::io::SeekFrom::Start(first)).await?;
    let mut left = count;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let n = f.read(&mut buf[..CHUNK.min(left as usize)]).await?;
        if n == 0 {
            break; // shorter now than it was a moment ago
        }
        stream.write_all(&buf[..n]).await?;
        left -= n as u64;
    }
    stream.shutdown().await.ok();
    Ok((left == 0 && first + count == len).then(String::new))
}

/// A folder as a zip. Some(note) once all of it went: how many files, and
/// what was left out.
async fn send_folder(stream: &mut TcpStream, o: &Opened, head_only: bool) -> Result<Option<String>> {
    let zip_name = format!("{}.zip", o.name);
    let headers =
        [("Content-Type", "application/octet-stream".to_string()), ("Content-Disposition", disposition(&zip_name))];
    head(stream, 200, &headers).await?;
    if head_only {
        stream.shutdown().await.ok();
        return Ok(None);
    }
    let mut out = tokio::io::BufWriter::with_capacity(CHUNK, &mut *stream);
    let (files, left_out) = write_zip(&mut out, &o.path, &o.name, false).await?;
    out.flush().await?;
    drop(out);
    stream.shutdown().await.ok();
    let mut note = format!("{files} file(s)");
    if left_out > 0 {
        note.push_str(&format!(", {left_out} left out or cut short (links, or not readable)"));
    }
    Ok(Some(note))
}

/// `dir` as a zip whose paths start with `root/`, onto `out`: the files
/// found, and what was left out or cut short (links, what could not be
/// read, all or part of the way). Each
/// folder's entries in name order; a file is read up to the size it had
/// when its folder was listed.
pub async fn write_zip<W: AsyncWrite + Unpin>(out: &mut W, dir: &Path, root: &str, all64: bool) -> Result<(u64, u64)> {
    let mut zip = Zip::new(all64);
    let (mut files, mut left_out) = (0u64, 0u64);
    let when = |m: &std::fs::Metadata| dos_time(m.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    let top = tokio::fs::metadata(dir).await?;
    out.write_all(&zip.folder(&format!("{root}/"), when(&top))).await?;
    let mut todo = vec![(dir.to_path_buf(), format!("{root}/"))];
    let mut buf = vec![0u8; CHUNK];
    while let Some((at, name)) = todo.pop() {
        let Ok(mut list) = tokio::fs::read_dir(&at).await else {
            left_out += 1;
            continue;
        };
        let mut here = Vec::new();
        while let Ok(Some(e)) = list.next_entry().await {
            here.push(e);
        }
        here.sort_by_key(|e| e.file_name());
        let mut folders = Vec::new();
        for e in here {
            let Ok(kind) = e.file_type().await else {
                left_out += 1;
                continue;
            };
            let entry = format!("{name}{}", e.file_name().to_string_lossy());
            // A link (a Windows junction too) is a kind of its own here, not
            // followed: neither a folder nor a file, so left out below.
            if kind.is_dir() {
                let Ok(m) = e.metadata().await else {
                    left_out += 1;
                    continue;
                };
                out.write_all(&zip.folder(&format!("{entry}/"), when(&m))).await?;
                folders.push((e.path(), format!("{entry}/")));
            } else if kind.is_file() {
                let (Ok(m), Ok(f)) = (e.metadata().await, tokio::fs::File::open(e.path()).await) else {
                    left_out += 1;
                    continue;
                };
                let size = m.len();
                out.write_all(&zip.begin(&entry, when(&m), size)).await?;
                let (mut crc, mut sent) = (Crc::default(), 0u64);
                let mut f = f.take(size);
                loop {
                    // Not readable part of the way (a region another
                    // program locked): the entry ends where the reading did.
                    let n = match f.read(&mut buf).await {
                        Ok(n) => n,
                        Err(_) => {
                            left_out += 1;
                            0
                        }
                    };
                    if n == 0 {
                        break;
                    }
                    crc.update(&buf[..n]);
                    out.write_all(&buf[..n]).await?;
                    sent += n as u64;
                }
                out.write_all(&zip.end(crc.value(), sent)).await?;
                files += 1;
            } else {
                left_out += 1;
            }
        }
        // Depth first, in name order.
        todo.extend(folders.into_iter().rev());
    }
    out.write_all(&zip.finish()).await?;
    Ok((files, left_out))
}

#[cfg(test)]
mod tests;
