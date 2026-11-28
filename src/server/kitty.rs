//! kitty's graphics protocol in a pane (docs/design/kitty-graphics.md).
//!
//! A program in a pane sends a picture as `ESC _ G <keys> ; <base64> ESC \`.
//! The pane's terminal model (`vt100`) drops such sequences, so they are
//! taken out of the output before it (`Scanner`), and the picture becomes
//! a kind of text: kitty's Unicode placeholders. Each cell of the picture
//! is the character U+10EEEE, its foreground colour the picture's number,
//! a combining mark for its row and one for its column. From then on the
//! picture is cells like any other: it scrolls, goes into the history, is
//! cut by a split, saved and restored, copied to other machines, with
//! nothing new in any of those.
//!
//! What goes into the model, and into the journal, is the output with the
//! sequences replaced by those placeholders ("normalised"): the rows an
//! image takes are worked out once, here, so every copy of the screen is
//! the same.
//!
//! The picture's bytes are kept on disk by their hash (`images.rs`); each
//! client whose terminal speaks the protocol is sent a picture the first
//! time one of its cells is drawn for it (`transmit`), under a number of
//! that terminal's own, with a "virtual placement" so the placeholders
//! show it. A terminal that does not is shown a label in its place.
//!
//! Understood, as UnifyTerm understands them (its graphics.rs):
//!
//! | key | values |
//! |-----|--------|
//! | `a` | `t` send, `T` send and place, `p` place, `d` delete, `q` ask |
//! | `f` | `100` a picture file (PNG; JPEG, GIF, WebP are kept and passed on as they are), `24` RGB, `32` RGBA |
//! | `t` | `d`: the data is in the sequence (the only one: a file path means nothing on another machine) |
//! | `m` | `1`: more pieces follow |
//! | `i` | the image's number |
//! | `s`, `v` | width and height of raw RGB or RGBA data |
//! | `c`, `r` | columns and rows the image takes |
//! | `C` | `1`: the cursor stays where it was |
//! | `U` | `1`: a virtual placement (the program writes the placeholders) |
//! | `d` | with `a=d`: `a`/`A` every image, `i`/`I` the one numbered `i` |
//! | `q` | `1`: no answer when all went well; `2`: no answer at all |

use super::layout::{PaneId, Rect};
use super::render::{Cell, Grid, Style};
use sha2::Digest as _;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use vt100::Color;

/// The character that stands for a cell of an image.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The marks for row and column numbers, in order: the Nth is N.
static DIACRITICS: LazyLock<Vec<char>> = LazyLock::new(|| {
    include_str!("rowcolumn-diacritics.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter_map(|hex| u32::from_str_radix(hex.trim(), 16).ok().and_then(char::from_u32))
        .collect()
});
static NUMBERS: LazyLock<HashMap<char, usize>> =
    LazyLock::new(|| DIACRITICS.iter().enumerate().map(|(i, c)| (*c, i)).collect());

/// The most rows or columns an image is placed on: one mark a number.
pub fn most_cells() -> usize {
    DIACRITICS.len()
}

/// The most bytes of one image's data, decoded, before it is complete.
const MOST_RECEIVED: usize = 96 * 1024 * 1024;
/// The longest one sequence may be (its base64 text); longer is dropped.
const MOST_SEQUENCE: usize = MOST_RECEIVED / 3 * 4 + 4096;
/// Numbers given to images sent without one: from the top of the range,
/// where a program's own are unlikely.
const OWN_FROM: u32 = 0x00F0_0000;
/// A cell's size in pixels when nothing better is known: the shape the
/// rows of an image placed by its columns alone are worked out with.
pub const CELL: (u32, u32) = (10, 20);
/// How long a picture no client sees stays in a terminal (and in memory).
pub const UNSEEN: Duration = Duration::from_secs(60);
/// Base64 bytes a piece of a picture sent to a terminal holds.
const PIECE: usize = 4096;
/// The most pictures a pane keeps track of: past it the oldest is
/// forgotten (its cells, if still on screen, show a label).
pub const MOST_IMAGES: usize = 256;

fn diacritic(n: usize) -> char {
    DIACRITICS.get(n).copied().unwrap_or(DIACRITICS[0])
}

// ---------------------------------------------------------------------------
// Base64, both ways (standard alphabet, padding optional on the way in).

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_decode(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in text {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let sym = |shift: u32| B64[((n >> shift) & 63) as usize] as char;
        out.push(sym(18));
        out.push(sym(12));
        out.push(if chunk.len() > 1 { sym(6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(0) } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------
// Taking the sequences out of a pane's output.

/// A piece of a pane's output: text for the model, or one graphics
/// command (the bytes after `ESC _`, from its `G`, without the `ESC \`).
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Text(Vec<u8>),
    Graphics(Vec<u8>),
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ScanState {
    #[default]
    Text,
    /// An ESC ended the last read: what follows decides.
    Esc,
    /// Inside `ESC _ …`, kept so far.
    Apc,
    /// Inside one, an ESC: `\` ends it.
    ApcEsc,
    /// Inside one too long to keep: dropped until its end.
    Skip,
    SkipEsc,
}

/// Finds `ESC _ … ESC \` in output that comes in reads of any size: a
/// sequence split across reads is kept until its end comes.
#[derive(Debug, Default)]
pub struct Scanner {
    state: ScanState,
    apc: Vec<u8>,
}

impl Scanner {
    /// Not inside a sequence: what comes next starts as text.
    pub fn idle(&self) -> bool {
        self.state == ScanState::Text
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut text = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match self.state {
                ScanState::Text => {
                    // Copy up to the next ESC in one go.
                    let rest = &bytes[i..];
                    match rest.iter().position(|&c| c == 0x1b) {
                        Some(n) => {
                            text.extend_from_slice(&rest[..n]);
                            i += n;
                            self.state = ScanState::Esc;
                        }
                        None => {
                            text.extend_from_slice(rest);
                            i = bytes.len();
                            continue;
                        }
                    }
                }
                ScanState::Esc => {
                    if b == b'_' {
                        self.state = ScanState::Apc;
                        self.apc.clear();
                    } else {
                        text.push(0x1b);
                        self.state = ScanState::Text;
                        continue; // this byte is text (or another ESC)
                    }
                }
                ScanState::Apc => {
                    if b == 0x1b {
                        self.state = ScanState::ApcEsc;
                    } else if self.apc.len() >= MOST_SEQUENCE {
                        self.apc = Vec::new();
                        self.state = ScanState::Skip;
                    } else {
                        // Copy up to the next ESC in one go.
                        let rest = &bytes[i..];
                        let n = rest.iter().position(|&c| c == 0x1b).unwrap_or(rest.len());
                        self.apc.extend_from_slice(&rest[..n]);
                        i += n;
                        continue;
                    }
                }
                ScanState::ApcEsc => {
                    if b == b'\\' {
                        let body = std::mem::take(&mut self.apc);
                        if body.first() == Some(&b'G') {
                            if !text.is_empty() {
                                out.push(Piece::Text(std::mem::take(&mut text)));
                            }
                            out.push(Piece::Graphics(body));
                        } else {
                            // Not ours: as it came (the model drops it).
                            text.extend_from_slice(b"\x1b_");
                            text.extend_from_slice(&body);
                            text.extend_from_slice(b"\x1b\\");
                        }
                        self.state = ScanState::Text;
                    } else {
                        // An ESC inside, not its end: kept, and looked at again.
                        self.apc.push(0x1b);
                        self.state = ScanState::Apc;
                        continue;
                    }
                }
                ScanState::Skip => {
                    if b == 0x1b {
                        self.state = ScanState::SkipEsc;
                    }
                }
                ScanState::SkipEsc => {
                    self.state = if b == b'\\' { ScanState::Text } else { ScanState::Skip };
                }
            }
            i += 1;
        }
        if !text.is_empty() {
            out.push(Piece::Text(text));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// One command.

/// One command's keys and its data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Command {
    keys: HashMap<char, String>,
    data: Vec<u8>,
}

impl Command {
    fn parse(body: &[u8]) -> Option<Command> {
        let body = body.strip_prefix(b"G")?;
        let (keys, data) = match body.iter().position(|&b| b == b';') {
            Some(n) => (&body[..n], &body[n + 1..]),
            None => (body, &[][..]),
        };
        let keys = std::str::from_utf8(keys).ok()?;
        let keys = keys
            .split(',')
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                let mut chars = key.chars();
                let key = chars.next().filter(|_| chars.next().is_none())?;
                Some((key, value.to_owned()))
            })
            .collect();
        Some(Command { keys, data: data.to_vec() })
    }

    fn get(&self, key: char) -> Option<&str> {
        self.keys.get(&key).map(String::as_str)
    }

    fn number(&self, key: char) -> Option<u32> {
        self.get(key).and_then(|v| v.parse().ok())
    }
}

// ---------------------------------------------------------------------------
// Pictures.

/// What an image's bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
    /// A picture file of a kind not recognised (`f=100`): passed on as it is.
    File,
    Rgb,
    Rgba,
}

impl Format {
    /// The file name's ending it is kept under.
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Gif => "gif",
            Format::Webp => "webp",
            Format::File => "img",
            Format::Rgb => "rgb",
            Format::Rgba => "rgba",
        }
    }

    /// kitty's `f` for it.
    fn f(self) -> u32 {
        match self {
            Format::Rgb => 24,
            Format::Rgba => 32,
            _ => 100,
        }
    }

    /// By the file ending it is kept under (`ext`).
    pub fn from_ext(ext: &str) -> Option<Format> {
        [Format::Png, Format::Jpeg, Format::Gif, Format::Webp, Format::File, Format::Rgb, Format::Rgba]
            .into_iter()
            .find(|f| f.ext() == ext)
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Gif => "GIF",
            Format::Webp => "WebP",
            Format::File => "picture",
            Format::Rgb => "RGB",
            Format::Rgba => "RGBA",
        }
    }
}

/// A picture file's kind and size in pixels, from its first bytes only
/// (nothing is decoded).
pub fn sniff(data: &[u8]) -> Option<(Format, u32, u32)> {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let le16 = |b: &[u8]| u32::from(u16::from_le_bytes([b[0], b[1]]));
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        return Some((Format::Png, be32(&data[16..20]), be32(&data[20..24])));
    }
    if data.len() >= 10 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return Some((Format::Gif, le16(&data[6..8]), le16(&data[8..10])));
    }
    if data.len() >= 30 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        let chunk = &data[12..16];
        let size = match chunk {
            b"VP8 " if data.len() >= 30 => (le16(&data[26..28]) & 0x3fff, le16(&data[28..30]) & 0x3fff),
            b"VP8L" if data.len() >= 25 => {
                let b = &data[21..25];
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => {
                let w = u32::from(data[24]) | (u32::from(data[25]) << 8) | (u32::from(data[26]) << 16);
                let h = u32::from(data[27]) | (u32::from(data[28]) << 8) | (u32::from(data[29]) << 16);
                (w + 1, h + 1)
            }
            _ => return Some((Format::Webp, 0, 0)),
        };
        return Some((Format::Webp, size.0, size.1));
    }
    if data.len() >= 4 && data.starts_with(&[0xff, 0xd8]) {
        // Walk the segments to a start of frame.
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if marker == 0xff {
                i += 1;
                continue;
            }
            let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
            let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
            if sof {
                let h = u32::from(u16::from_be_bytes([data[i + 5], data[i + 6]]));
                let w = u32::from(u16::from_be_bytes([data[i + 7], data[i + 8]]));
                return Some((Format::Jpeg, w, h));
            }
            if len < 2 {
                break;
            }
            i += 2 + len;
        }
        return Some((Format::Jpeg, 0, 0));
    }
    None
}

/// A picture a pane has, by its number there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Image {
    /// SHA-256 of its bytes, in hex: its file's name.
    pub hash: String,
    pub format: Format,
    /// In pixels (0 when its kind did not say).
    pub width: u32,
    pub height: u32,
    /// The columns and rows its last placement took (the virtual placement
    /// a terminal is given).
    pub cells: Option<(u16, u16)>,
}

/// A picture received, for the server to keep on disk.
#[derive(Debug)]
pub struct Received {
    pub hash: String,
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// What to do after a command, besides keeping what it sent.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Write this back to the program (an answer).
    Reply(Vec<u8>),
    /// Feed these bytes to the model, as output (the placeholders).
    Output(Vec<u8>),
}

/// An image whose pieces are still arriving.
#[derive(Debug)]
struct Arriving {
    first: Command,
    /// The base64 of the pieces so far.
    text: Vec<u8>,
}

/// A pane's side of the protocol: what it is sent, and the pictures it has.
#[derive(Debug, Default)]
pub struct PaneGraphics {
    scanner: Scanner,
    arriving: Option<Arriving>,
    /// The pictures, by their number in this pane.
    pub images: HashMap<u32, Image>,
    /// The numbers in the order they were sent, oldest first (`MOST_IMAGES`).
    order: std::collections::VecDeque<u32>,
    next_own: u32,
    /// Pictures received since the server last took them.
    pub received: Vec<Received>,
    /// Numbers whose picture or placement changed since the pane last told
    /// its journal (a copy on another machine follows them).
    pub changed: Vec<u32>,
}

/// What went wrong with a command, as the protocol says it.
fn refusal(code: &str, why: &str) -> String {
    format!("{code}:{why}")
}

/// Where the cursor is after an image is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// After the image's last cell, as after text.
    Moves,
    /// Where it was (`C=1`).
    Stays,
}

impl PaneGraphics {
    /// Not inside a sequence (`Scanner::idle`).
    pub fn idle(&self) -> bool {
        self.scanner.idle()
    }

    /// Keep picture `id` (`MOST_IMAGES` at most: the oldest goes).
    pub fn keep(&mut self, id: u32, image: Image) {
        self.images.insert(id, image);
        self.order.retain(|n| *n != id);
        self.order.push_back(id);
        while self.order.len() > MOST_IMAGES {
            if let Some(old) = self.order.pop_front() {
                self.images.remove(&old);
            }
        }
    }

    /// Split a read of output into text and commands (`Scanner`).
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<Piece> {
        self.scanner.feed(bytes)
    }

    /// Take one command. `col` is the cursor's column and `width` the
    /// screen's, for the placeholders; `cell` a cell's size in pixels.
    pub fn command(&mut self, body: &[u8], col: u16, width: u16, cell: (u32, u32)) -> Vec<Effect> {
        let Some(command) = Command::parse(body) else { return Vec::new() };
        // A piece of an image that is arriving: only `m` and the data count.
        // The pieces' base64 is put together and read once at the end, so a
        // piece that does not end on a whole group of 4 still reads right.
        if let Some(arriving) = self.arriving.as_mut() {
            let more = command.get('m') == Some("1");
            if arriving.text.len() + command.data.len() > MOST_SEQUENCE {
                let first = self.arriving.take().map(|a| a.first).unwrap_or_default();
                return answer(&first, Err(refusal("EINVAL", "too much data")));
            }
            arriving.text.extend_from_slice(&command.data);
            if more {
                return Vec::new();
            }
            let Arriving { first, text } = self.arriving.take().expect("checked above");
            let Some(data) = base64_decode(&text) else {
                return answer(&first, Err(refusal("EINVAL", "the data is not base64")));
            };
            return self.received(&first, data, col, width, cell);
        }
        match command.get('a').unwrap_or("t") {
            "t" | "T" | "q" => {
                if command.get('t').is_some_and(|medium| medium != "d") {
                    return answer(&command, Err(refusal("ENOTSUP", "only data in the sequence (t=d)")));
                }
                if command.get('m') == Some("1") {
                    let text = command.data.clone();
                    self.arriving = Some(Arriving { first: command, text });
                    return Vec::new();
                }
                let Some(bytes) = base64_decode(&command.data) else {
                    return answer(&command, Err(refusal("EINVAL", "the data is not base64")));
                };
                self.received(&command, bytes, col, width, cell)
            }
            "p" => {
                let Some(id) = command.number('i') else {
                    return answer(&command, Err(refusal("EINVAL", "which image (i)")));
                };
                if !self.images.contains_key(&id) {
                    return answer(&command, Err(refusal("ENOENT", "no such image")));
                }
                let mut effects = answer(&command, Ok(()));
                effects.extend(self.place(&command, id, col, width, cell));
                effects
            }
            "d" => {
                match command.get('d').unwrap_or("a") {
                    "a" | "A" => self.images.clear(),
                    "i" | "I" => {
                        if let Some(id) = command.number('i') {
                            self.images.remove(&id);
                        }
                    }
                    _ => {}
                }
                Vec::new()
            }
            _ => answer(&command, Err(refusal("ENOTSUP", "this action is not supported"))),
        }
    }

    /// All of an image's data is here: check it, keep it (unless only
    /// asked), place it.
    fn received(&mut self, command: &Command, data: Vec<u8>, col: u16, width: u16, cell: (u32, u32)) -> Vec<Effect> {
        let checked = check(command, &data);
        if command.get('a') == Some("q") {
            return answer(command, checked.map(|_| ()));
        }
        let (format, w, h) = match checked {
            Ok(v) => v,
            Err(why) => return answer(command, Err(why)),
        };
        let id = command.number('i').map(|i| i & 0x00ff_ffff).unwrap_or_else(|| {
            self.next_own = self.next_own.wrapping_add(1) & 0x000f_ffff;
            OWN_FROM + self.next_own
        });
        let hash = hex(&sha2::Sha256::digest(&data));
        self.keep(id, Image { hash: hash.clone(), format, width: w, height: h, cells: None });
        self.received.push(Received { hash, format, bytes: data });
        self.changed.push(id);
        let mut effects = answer(command, Ok(()));
        if command.get('a') == Some("T") {
            effects.extend(self.place(command, id, col, width, cell));
        }
        effects
    }

    /// Place image `id` as the command says: how many cells, and where the
    /// cursor ends up. The placeholders come back as output for the model.
    fn place(&mut self, command: &Command, id: u32, col: u16, width: u16, cell: (u32, u32)) -> Vec<Effect> {
        let Some(image) = self.images.get_mut(&id) else { return Vec::new() };
        let (cw, ch) = (f64::from(cell.0.max(1)), f64::from(cell.1.max(1)));
        let (pw, ph) = (f64::from(image.width.max(1)), f64::from(image.height.max(1)));
        let given = |key| command.number(key).filter(|n| *n > 0).map(|n| n as usize);
        let known = image.width > 0 && image.height > 0;
        // Whatever is not given keeps the image's shape.
        let (columns, rows) = match (given('c'), given('r')) {
            (Some(c), Some(r)) => (c, r),
            (Some(c), None) if known => (c, cells_for(c as f64 * cw * ph / pw / ch)),
            (None, Some(r)) if known => (cells_for(r as f64 * ch * pw / ph / cw), r),
            (None, None) if known => (cells_for(pw / cw), cells_for(ph / ch)),
            (Some(c), None) => (c, cells_for(c as f64 / 2.0)),
            (None, Some(r)) => (r * 2, r),
            (None, None) => (20, 10),
        };
        let most = most_cells();
        let cells = (columns.clamp(1, most) as u16, rows.clamp(1, most) as u16);
        image.cells = Some(cells);
        self.changed.push(id);
        if command.get('U') == Some("1") {
            return Vec::new(); // the program writes the placeholders
        }
        let cursor = if command.get('C') == Some("1") { Cursor::Stays } else { Cursor::Moves };
        // Only what fits on the line: the rest of each row is cut off.
        let shown = cells.0.min(width.saturating_sub(col)).max(1);
        vec![Effect::Output(placeholders(id, (shown, cells.1), col, cursor).into_bytes())]
    }
}

/// A count of cells for a size in cells that may have a fraction: rounded up, at least 1.
fn cells_for(size: f64) -> usize {
    if size.is_finite() { (size.ceil() as usize).max(1) } else { 1 }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// An image's kind and size from its data, or why it cannot be one.
fn check(command: &Command, data: &[u8]) -> Result<(Format, u32, u32), String> {
    let raw = |channels: u64, format: Format| {
        let (Some(w), Some(h)) = (command.number('s'), command.number('v')) else {
            return Err(refusal("EINVAL", "raw pixels need their width (s) and height (v)"));
        };
        if u64::from(w) * u64::from(h) * channels != data.len() as u64 {
            return Err(refusal("EINVAL", "the data is not width × height pixels"));
        }
        Ok((format, w, h))
    };
    match command.number('f').unwrap_or(32) {
        32 => raw(4, Format::Rgba),
        24 => raw(3, Format::Rgb),
        100 => match sniff(data) {
            Some(found) => Ok(found),
            None if data.is_empty() => Err(refusal("EINVAL", "no data")),
            None => Ok((Format::File, 0, 0)),
        },
        _ => Err(refusal("ENOTSUP", "this format is not supported")),
    }
}

/// The answer to a command, as `q` allows; none when it gave no number.
fn answer(command: &Command, outcome: Result<(), String>) -> Vec<Effect> {
    let Some(id) = command.get('i') else { return Vec::new() };
    let quiet = command.number('q').unwrap_or(0);
    let text = match outcome {
        Ok(()) if quiet >= 1 => return Vec::new(),
        Ok(()) => "OK".to_owned(),
        Err(_) if quiet >= 2 => return Vec::new(),
        Err(why) => why,
    };
    vec![Effect::Reply(format!("\x1b_Gi={id};{text}\x1b\\").into_bytes())]
}

/// The placeholder text that shows image `id` on `cells` = (columns, rows)
/// from column `start` (counted from 0): each row in the image's number as
/// its colour, one line down and back to that column between rows; the
/// cursor as `cursor` says. As UnifyTerm writes them.
pub fn placeholders(id: u32, cells: (u16, u16), start: u16, cursor: Cursor) -> String {
    let [_, red, green, blue] = id.to_be_bytes();
    let (columns, rows) = cells;
    let mut text = String::new();
    if cursor == Cursor::Stays {
        text.push_str("\x1b7");
    }
    for row in 0..rows {
        let _ = write!(text, "\x1b[38;2;{red};{green};{blue}m");
        for column in 0..columns {
            text.push(PLACEHOLDER);
            text.push(diacritic(usize::from(row)));
            text.push(diacritic(usize::from(column)));
        }
        text.push_str("\x1b[39m");
        if row + 1 < rows {
            // One line down (scrolling at the bottom), then to the column
            // itself: a move back by the columns written would miss by one
            // when the row ends at the screen's last column.
            let _ = write!(text, "\n\x1b[{}G", start + 1);
        }
    }
    if cursor == Cursor::Stays {
        text.push_str("\x1b8");
    }
    text
}

/// A placeholder cell's row and column in its image (None for a mark
/// missing: kitty takes it from the cell before, keepane only labels).
pub fn place_of(text: &str) -> Option<(Option<usize>, Option<usize>)> {
    let mut chars = text.chars();
    if chars.next() != Some(PLACEHOLDER) {
        return None;
    }
    let row = chars.next().and_then(|c| NUMBERS.get(&c).copied());
    let col = chars.next().and_then(|c| NUMBERS.get(&c).copied());
    Some((row, col))
}

/// `text` with each placeholder (U+10EEEE and the marks after it) a space:
/// a picture read as text is blank, its colour sequences kept.
pub fn blank_placeholders(text: &str) -> String {
    if !text.contains(PLACEHOLDER) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == PLACEHOLDER {
            out.push(' ');
            while chars.peek().is_some_and(|m| NUMBERS.contains_key(m)) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The image number a placeholder cell's colour carries.
fn number_of(fg: Color) -> Option<u32> {
    match fg {
        Color::Rgb(r, g, b) => Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
        Color::Idx(n) => Some(u32::from(n)),
        Color::Default => None,
    }
}

// ---------------------------------------------------------------------------
// Each client's side: numbers in its terminal, what it was sent.

/// A picture as a client's terminal knows it.
#[derive(Debug, Clone)]
struct Sent {
    /// Drawn in the last frame, or when it last was.
    seen: Instant,
    /// Its bytes have gone to the terminal.
    sent: bool,
    image: Image,
}

/// What one client's terminal has been given.
#[derive(Debug)]
pub struct ClientImages {
    /// The terminal shows kitty's pictures: Some(true) it does, Some(false)
    /// it does not (a label), None not said yet (blank: no label that
    /// flashes before a terminal that shows them says so).
    pub kitty: Option<bool>,
    /// (pane, number there) → number in the terminal: one for each, so a
    /// picture sent again under the same number (an animation's next frame)
    /// replaces the last in the terminal instead of piling up there.
    numbers: HashMap<(PaneId, u32), u32>,
    by_number: HashMap<u32, Sent>,
    next: u32,
}

impl ClientImages {
    /// A client whose terminal has not said yet what it shows.
    pub fn unknown() -> ClientImages {
        ClientImages { kitty: None, ..ClientImages::new(false) }
    }

    pub fn new(kitty: bool) -> ClientImages {
        // Numbers from a random point high in the range: the terminal's
        // numbers are shared by everything in its window, a picture shown
        // before keepane attached among them.
        let seed =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        ClientImages {
            kitty: Some(kitty),
            numbers: HashMap::new(),
            by_number: HashMap::new(),
            next: 0x0080_0000 | (seed & 0x003f_f000),
        }
    }

    fn number_for(&mut self, pane: PaneId, id: u32, image: &Image, now: Instant) -> u32 {
        let key = (pane, id);
        if let Some(&n) = self.numbers.get(&key)
            && let Some(s) = self.by_number.get_mut(&n)
        {
            s.seen = now;
            if s.image != *image {
                // Another picture (or placement) under that number: sent
                // again, under the terminal's same number, replacing it.
                s.image = image.clone();
                s.sent = false;
            }
            return n;
        }
        self.next = if self.next >= 0x00ef_ffff { 0x0080_0000 } else { self.next + 1 };
        let n = self.next;
        self.numbers.insert(key, n);
        self.by_number.insert(n, Sent { seen: now, sent: false, image: image.clone() });
        n
    }
}

/// The pictures of the panes in a frame, for `map`: each pane's rectangle
/// in the grid and its pictures.
pub struct Region<'a> {
    pub pane: PaneId,
    pub rect: Rect,
    pub images: &'a HashMap<u32, Image>,
}

/// Rewrite the placeholder cells `regions` cover for this client: in a
/// terminal that shows pictures, each picture's colour becomes its number
/// there; elsewhere (or for a picture not known or not kept) a label.
/// Before `recolor`, which must not see the numbers as colours.
pub fn map(grid: &mut Grid, regions: &[Region], client: &mut ClientImages, kept: &mut dyn FnMut(&Image) -> bool) {
    let now = Instant::now();
    for region in regions {
        let r = region.rect;
        // Label cells: the image's top-left cell starts the label, which
        // runs on over the cells of the same image in that row.
        let mut label: Option<(u16, u16, Vec<char>, usize, u32)> = None;
        for y in r.y..r.y.saturating_add(r.h).min(grid.rows) {
            for x in r.x..r.x.saturating_add(r.w).min(grid.cols) {
                let cell = grid.get(x, y);
                let Some((row, col)) = place_of(cell.text()) else {
                    if label.as_ref().is_some_and(|l| l.1 == y) {
                        label = None;
                    }
                    continue;
                };
                let style = cell.style;
                let id = number_of(style.fg);
                let image = id.and_then(|i| region.images.get(&i));
                if client.kitty == Some(true)
                    && let (Some(id), Some(image)) = (id, image)
                    && kept(image)
                {
                    let n = client.number_for(region.pane, id, image, now);
                    let [_, rr, gg, bb] = n.to_be_bytes();
                    let mut c = cell.clone();
                    c.style.fg = Color::Rgb(rr, gg, bb);
                    grid.set(x, y, c);
                    continue;
                }
                // Not said yet what its terminal shows: blank for now.
                if client.kitty.is_none() {
                    grid.set(x, y, Cell::blank(Style::default()));
                    continue;
                }
                // A label instead.
                let plain = Style { fg: Color::Default, bg: Color::Default, dim: true, ..Style::default() };
                let starts = row == Some(0) && col == Some(0);
                if starts {
                    let text = match image {
                        Some(i) if i.width > 0 => format!("▧ image {}×{}", i.width, i.height),
                        _ => "▧ image".to_string(),
                    };
                    label = Some((x, y, text.chars().collect(), 0, id.unwrap_or(0)));
                }
                let ch = match label.as_mut() {
                    Some(l) if l.1 == y && l.4 == id.unwrap_or(0) && l.3 < l.2.len() => {
                        l.3 += 1;
                        Some(l.2[l.3 - 1])
                    }
                    _ => None,
                };
                let mut buf = [0u8; 4];
                let cell = match ch {
                    Some(c) if unicode_width::UnicodeWidthChar::width(c) == Some(1) => {
                        Cell::new(c.encode_utf8(&mut buf), false, plain)
                    }
                    _ => Cell::blank(plain),
                };
                grid.set(x, y, cell);
            }
            label = None;
        }
    }
}

/// What to send this client's terminal before the frame: each picture a
/// cell in `grid` shows that it does not have yet (its bytes, then a
/// virtual placement), and the deletion of those it has not been shown
/// for `UNSEEN`. `bytes` gives a picture's bytes (from disk).
pub fn transmit(
    grid: &Grid,
    client: &mut ClientImages,
    bytes: &mut dyn FnMut(&Image) -> Option<std::sync::Arc<Vec<u8>>>,
) -> Vec<u8> {
    if client.kitty != Some(true) {
        return Vec::new();
    }
    let now = Instant::now();
    let mut shown: HashSet<u32> = HashSet::new();
    for y in 0..grid.rows {
        for x in 0..grid.cols {
            let cell = grid.get(x, y);
            if cell.text().starts_with(PLACEHOLDER)
                && let Some(n) = number_of(cell.style.fg)
                && client.by_number.contains_key(&n)
            {
                shown.insert(n);
            }
        }
    }
    let mut out = Vec::new();
    let mut numbers: Vec<u32> = shown.iter().copied().collect();
    numbers.sort_unstable();
    for n in numbers {
        let Some(s) = client.by_number.get_mut(&n) else { continue };
        s.seen = now;
        if s.sent {
            continue;
        }
        let Some(data) = bytes(&s.image) else { continue };
        s.sent = true;
        out.extend(transmission(n, &s.image, &data));
    }
    // Pictures not drawn for a while leave the terminal.
    let gone: Vec<u32> = client
        .by_number
        .iter()
        .filter(|(n, s)| !shown.contains(n) && now.duration_since(s.seen) >= UNSEEN)
        .map(|(n, _)| *n)
        .collect();
    for n in gone {
        if client.by_number.remove(&n).is_some_and(|s| s.sent) {
            let _ = write!(&mut StrOut(&mut out), "\x1b_Ga=d,d=I,i={n},q=2\x1b\\");
        }
        client.numbers.retain(|_, v| *v != n);
    }
    out
}

/// What gives a terminal picture `n`: its bytes in pieces (`a=t`, quiet),
/// then a virtual placement over the cells `image.cells` says, which the
/// placeholders show.
pub fn transmission(n: u32, image: &Image, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let size = match image.format {
        Format::Rgb | Format::Rgba => format!(",s={},v={}", image.width, image.height),
        _ => String::new(),
    };
    let text = base64_encode(data);
    let pieces: Vec<&str> = if text.is_empty() {
        vec![""]
    } else {
        text.as_bytes().chunks(PIECE).map(|c| std::str::from_utf8(c).unwrap_or("")).collect()
    };
    for (i, piece) in pieces.iter().enumerate() {
        let more = u8::from(i + 1 < pieces.len());
        if i == 0 {
            let _ = write!(
                &mut StrOut(&mut out),
                "\x1b_Ga=t,t=d,f={},i={n},q=2{size},m={more};{piece}\x1b\\",
                image.format.f()
            );
        } else {
            let _ = write!(&mut StrOut(&mut out), "\x1b_Gm={more};{piece}\x1b\\");
        }
    }
    let (c, r) = image.cells.unwrap_or((1, 1));
    let _ = write!(&mut StrOut(&mut out), "\x1b_Ga=p,U=1,i={n},c={c},r={r},q=2\x1b\\");
    out
}

/// One row of the placeholders of picture `id`, `columns` wide, in its
/// colour: for a program that writes its own (a virtual placement).
pub fn placeholder_row(id: u32, row: u16, columns: u16) -> String {
    let [_, red, green, blue] = id.to_be_bytes();
    let mut text = format!("\x1b[38;2;{red};{green};{blue}m");
    for column in 0..columns {
        text.push(PLACEHOLDER);
        text.push(diacritic(usize::from(row)));
        text.push(diacritic(usize::from(column)));
    }
    text.push_str("\x1b[39m");
    text
}

/// The rows and columns a picture `width` × `height` pixels takes when
/// placed `columns` wide on cells of `cell` pixels (its shape kept).
pub fn rows_for(columns: u16, (width, height): (u32, u32), cell: (u32, u32)) -> u16 {
    if width == 0 || height == 0 {
        return (columns / 2).max(1);
    }
    let rows =
        f64::from(columns) * f64::from(cell.0.max(1)) * f64::from(height) / f64::from(width) / f64::from(cell.1.max(1));
    (cells_for(rows).min(most_cells())) as u16
}

/// `write!` into a byte vector.
struct StrOut<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for StrOut<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4]);
        v
    }

    fn texts(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|p| match p {
                Piece::Text(t) => format!("T[{}]", String::from_utf8_lossy(t)),
                Piece::Graphics(g) => format!("G[{}]", String::from_utf8_lossy(g)),
            })
            .collect()
    }

    #[test]
    fn base64_goes_both_ways() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7]] {
            let text = base64_encode(data);
            assert_eq!(base64_decode(text.as_bytes()).as_deref(), Some(data), "{text}");
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_decode(b"Zm9vYg"), Some(b"foob".to_vec()), "padding may be left out");
        assert_eq!(base64_decode(b"Zm9v!"), None);
    }

    #[test]
    fn the_scanner_takes_graphics_out_of_the_text() {
        let mut s = Scanner::default();
        let got = s.feed(b"ab\x1b_Ga=t;QUJD\x1b\\cd\x1b[1mx");
        assert_eq!(texts(&got), "T[ab]G[Ga=t;QUJD]T[cd\x1b[1mx]");
        // Another APC is passed on as it came; the model drops it.
        assert_eq!(texts(&s.feed(b"\x1b_Xyz\x1b\\.")), "T[\x1b_Xyz\x1b\\.]");
    }

    #[test]
    fn a_sequence_split_anywhere_is_put_together() {
        let whole = b"left\x1b_Gi=5,a=T;QUJDRA==\x1b\\right\x1b[0m";
        for cut in 0..=whole.len() {
            for cut2 in cut..=whole.len() {
                let mut s = Scanner::default();
                let mut all = s.feed(&whole[..cut]);
                all.extend(s.feed(&whole[cut..cut2]));
                all.extend(s.feed(&whole[cut2..]));
                // Text pieces may come in more parts: join them.
                let (mut text, mut graphics) = (Vec::new(), Vec::new());
                for p in all {
                    match p {
                        Piece::Text(t) => text.extend(t),
                        Piece::Graphics(g) => graphics.push(g),
                    }
                }
                assert_eq!(text, b"leftright\x1b[0m", "cut at {cut}, {cut2}");
                assert_eq!(graphics, vec![b"Gi=5,a=T;QUJDRA==".to_vec()], "cut at {cut}, {cut2}");
            }
        }
    }

    #[test]
    fn an_esc_inside_a_sequence_that_is_not_its_end_is_kept() {
        let mut s = Scanner::default();
        assert_eq!(texts(&s.feed(b"\x1b_Gx\x1by\x1b\\z")), "G[Gx\x1by]T[z]");
    }

    #[test]
    fn sizes_are_read_from_the_first_bytes() {
        assert_eq!(sniff(&png(640, 480)), Some((Format::Png, 640, 480)));
        let gif = b"GIF89a\x80\x02\xe0\x01rest";
        assert_eq!(sniff(gif), Some((Format::Gif, 640, 480)));
        // JPEG: SOI, an APP0 segment, then SOF0 with height 480, width 640.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0xe0, 0x02, 0x80, 0x03, 0, 0, 0, 0]);
        assert_eq!(sniff(&jpeg), Some((Format::Jpeg, 640, 480)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x7f, 0x02, 0x00, 0xdf, 0x01, 0x00]);
        assert_eq!(sniff(&webp), Some((Format::Webp, 640, 480)));
        assert_eq!(sniff(b"plain text"), None);
    }

    fn body(keys: &str, data: &[u8]) -> Vec<u8> {
        format!("G{keys};{}", base64_encode(data)).into_bytes()
    }

    #[test]
    fn a_picture_sent_and_placed_becomes_placeholders() {
        let mut g = PaneGraphics::default();
        // 40×30 pixels on 10×20 cells, 4 columns: 4 × 10 × 30 / 40 / 20 = 1.5, two rows.
        let effects = g.command(&body("a=T,f=100,i=7,c=4", &png(40, 30)), 3, 80, CELL);
        let image = &g.images[&7];
        assert_eq!((image.format, image.width, image.height, image.cells), (Format::Png, 40, 30, Some((4, 2))));
        assert_eq!(g.received.len(), 1, "kept for the disk");
        assert_eq!(g.received[0].hash, image.hash);
        assert_eq!(effects[0], Effect::Reply(b"\x1b_Gi=7;OK\x1b\\".to_vec()));
        let Effect::Output(text) = &effects[1] else { panic!("{effects:?}") };
        let text = String::from_utf8(text.clone()).unwrap();
        assert_eq!(text.matches(PLACEHOLDER).count(), 8, "4 × 2 cells");
        assert!(text.starts_with("\x1b[38;2;0;0;7m"), "the number as the colour: {text:?}");
        assert!(text.contains("\n\x1b[4G"), "back to column 4 for the second row");
    }

    #[test]
    fn rows_follow_the_shape_and_columns_are_cut_at_the_edge() {
        let mut g = PaneGraphics::default();
        // 100×100 pixels, 30 columns: 30 × 10 × 100 / 100 / 20 = 15 rows.
        let effects = g.command(&body("a=T,f=100,q=2,c=30", &png(100, 100)), 70, 80, CELL);
        let image = g.images.values().next().unwrap();
        assert_eq!(image.cells, Some((30, 15)), "the placement keeps all its columns");
        let Effect::Output(text) = &effects[0] else { panic!("{effects:?}") };
        let text = String::from_utf8(text.clone()).unwrap();
        assert_eq!(text.matches(PLACEHOLDER).count(), 10 * 15, "only the 10 columns left on the line");
        assert!(g.images.keys().all(|k| *k >= OWN_FROM), "a number of its own when none was given");
    }

    #[test]
    fn pieces_make_one_picture() {
        let data = png(8, 8);
        let text = base64_encode(&data);
        let (a, b) = text.split_at(10);
        let mut g = PaneGraphics::default();
        assert!(g.command(format!("Ga=t,f=100,i=2,m=1;{a}").as_bytes(), 0, 80, CELL).is_empty());
        let done = g.command(format!("Gm=0;{b}").as_bytes(), 0, 80, CELL);
        assert_eq!(done, vec![Effect::Reply(b"\x1b_Gi=2;OK\x1b\\".to_vec())]);
        assert_eq!(g.received[0].bytes, data);
    }

    #[test]
    fn answers_follow_q_and_a_query_keeps_nothing() {
        let mut g = PaneGraphics::default();
        let asked = g.command(&body("a=q,i=31,s=1,v=1,f=24", &[0, 0, 0]), 0, 80, CELL);
        assert_eq!(asked, vec![Effect::Reply(b"\x1b_Gi=31;OK\x1b\\".to_vec())]);
        assert!(g.images.is_empty() && g.received.is_empty());
        let bad = g.command(&body("a=t,i=4,f=24,s=2,v=2", &[0, 0, 0]), 0, 80, CELL);
        assert!(matches!(&bad[0], Effect::Reply(r) if r.starts_with(b"\x1b_Gi=4;EINVAL")), "{bad:?}");
        assert!(g.command(&body("a=t,i=4,f=24,s=2,v=2,q=2", &[0, 0, 0]), 0, 80, CELL).is_empty());
        assert!(g.command(&body("a=t,f=100,q=1,i=9", &png(1, 1)), 0, 80, CELL).is_empty());
        let file = g.command(b"Ga=t,t=f,i=3;L3RtcC94", 0, 80, CELL);
        assert!(matches!(&file[0], Effect::Reply(r) if r.starts_with(b"\x1b_Gi=3;ENOTSUP")));
        assert!(g.command(&body("a=t,f=100", &png(1, 1)), 0, 80, CELL).is_empty(), "no number, no answer");
    }

    #[test]
    fn a_virtual_placement_writes_nothing_and_delete_forgets() {
        let mut g = PaneGraphics::default();
        g.command(&body("a=t,f=100,i=5,q=2", &png(20, 20)), 0, 80, CELL);
        assert!(g.command(b"Ga=p,U=1,i=5,c=6,r=3,q=2", 0, 80, CELL).is_empty());
        assert_eq!(g.images[&5].cells, Some((6, 3)));
        let placed = g.command(b"Ga=p,i=5,c=2,r=1", 0, 80, CELL);
        assert!(matches!(&placed[1], Effect::Output(_)));
        let missing = g.command(b"Ga=p,i=6", 0, 80, CELL);
        assert!(matches!(&missing[0], Effect::Reply(r) if r.starts_with(b"\x1b_Gi=6;ENOENT")));
        g.command(b"Ga=d,d=I,i=5", 0, 80, CELL);
        assert!(g.images.is_empty());
    }

    #[test]
    fn a_pane_keeps_track_of_its_latest_pictures_only() {
        let mut g = PaneGraphics::default();
        for i in 0..(MOST_IMAGES as u32 + 4) {
            g.command(&body(&format!("a=t,f=100,q=2,i={}", i + 1), &png(i + 1, 1)), 0, 80, CELL);
        }
        assert_eq!(g.images.len(), MOST_IMAGES);
        assert!(!g.images.contains_key(&1) && !g.images.contains_key(&4), "the oldest forgotten");
        assert!(g.images.contains_key(&5) && g.images.contains_key(&(MOST_IMAGES as u32 + 4)));
        // Sent again under the same number: it is the newest now, kept.
        g.command(&body("a=t,f=100,q=2,i=5", &png(9, 9)), 0, 80, CELL);
        g.command(&body("a=t,f=100,q=2,i=9999", &png(9, 9)), 0, 80, CELL);
        assert!(g.images.contains_key(&5) && !g.images.contains_key(&6));
    }

    #[test]
    fn placeholders_carry_the_number_row_and_column() {
        let text = placeholders(0x01_02_03, (2, 2), 0, Cursor::Stays);
        assert!(text.starts_with("\x1b7\x1b[38;2;1;2;3m") && text.ends_with("\x1b8"));
        let cells: Vec<String> = text
            .split(PLACEHOLDER)
            .skip(1)
            .map(|rest| format!("{PLACEHOLDER}{}", rest.chars().take(2).collect::<String>()))
            .collect();
        let places: Vec<_> = cells.iter().map(|c| place_of(c).unwrap()).collect();
        assert_eq!(
            places,
            [(Some(0), Some(0)), (Some(0), Some(1)), (Some(1), Some(0)), (Some(1), Some(1))],
            "row and column read back"
        );
        assert_eq!(place_of("x"), None);
    }

    fn grid_with(text: &str, fg: Color) -> Grid {
        let mut g = Grid::new(20, 3);
        let style = Style { fg, ..Style::default() };
        for (y, line) in text.split('\n').enumerate() {
            let mut x = 0;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                let mut s = c.to_string();
                while chars.peek().is_some_and(|n| unicode_width::UnicodeWidthChar::width(*n) == Some(0)) {
                    s.push(chars.next().unwrap());
                }
                g.set(x, y as u16, Cell::new(&s, false, style));
                x += 1;
            }
        }
        g
    }

    fn image(w: u32, h: u32, cells: (u16, u16)) -> Image {
        Image { hash: "ab".into(), format: Format::Png, width: w, height: h, cells: Some(cells) }
    }

    #[test]
    fn a_terminal_with_pictures_gets_its_own_numbers_then_the_picture_once() {
        let text = placeholders(7, (3, 2), 0, Cursor::Moves);
        // Drawn as the model would: the two rows of cells.
        let plain: String = text.replace("\x1b[38;2;0;0;7m", "").replace("\x1b[39m", "").replace("\n\x1b[1G", "\n");
        let mut grid = grid_with(&plain, Color::Rgb(0, 0, 7));
        let images = HashMap::from([(7, image(30, 40, (3, 2)))]);
        let mut client = ClientImages::new(true);
        let regions = [Region { pane: 1, rect: Rect { x: 0, y: 0, w: 20, h: 3 }, images: &images }];
        map(&mut grid, &regions, &mut client, &mut |_| true);
        let n = number_of(grid.get(0, 0).style.fg).unwrap();
        assert!(n >= 0x0080_0000, "the terminal's own number: {n:#x}");
        assert!(grid.get(2, 1).text().starts_with(PLACEHOLDER) && number_of(grid.get(2, 1).style.fg) == Some(n));
        let mut loads = 0;
        let mut data = |_: &Image| {
            loads += 1;
            Some(std::sync::Arc::new(png(30, 40)))
        };
        let first = String::from_utf8(transmit(&grid, &mut client, &mut data)).unwrap();
        assert!(first.starts_with(&format!("\x1b_Ga=t,t=d,f=100,i={n},q=2,m=0;")), "{first}");
        assert!(first.ends_with(&format!("\x1b_Ga=p,U=1,i={n},c=3,r=2,q=2\x1b\\")), "{first}");
        let again = transmit(&grid, &mut client, &mut data);
        assert!(again.is_empty(), "sent once");
        assert_eq!(loads, 1);
    }

    #[test]
    fn a_new_frame_under_the_same_number_replaces_the_last_in_the_terminal() {
        let text = placeholders(7, (3, 2), 0, Cursor::Moves);
        let plain: String = text.replace("\x1b[38;2;0;0;7m", "").replace("\x1b[39m", "").replace("\n\x1b[1G", "\n");
        let mut client = ClientImages::new(true);
        let mut numbers = Vec::new();
        let mut sent = 0;
        for frame in 0..5u8 {
            let mut grid = grid_with(&plain, Color::Rgb(0, 0, 7));
            let one = Image { hash: format!("{frame:064}"), ..image(30, 40, (3, 2)) };
            let images = HashMap::from([(7, one)]);
            let regions = [Region { pane: 1, rect: Rect { x: 0, y: 0, w: 20, h: 3 }, images: &images }];
            map(&mut grid, &regions, &mut client, &mut |_| true);
            numbers.push(number_of(grid.get(0, 0).style.fg).unwrap());
            let out = String::from_utf8(transmit(&grid, &mut client, &mut |_| Some(std::sync::Arc::new(png(30, 40)))))
                .unwrap();
            sent += out.matches("\x1b_Ga=t,").count();
        }
        assert!(numbers.windows(2).all(|w| w[0] == w[1]), "one number in the terminal: {numbers:?}");
        assert_eq!(sent, 5, "each frame sent, under it");
        assert_eq!(client.by_number.len(), 1, "one picture in the terminal, not five");
    }

    #[test]
    fn a_terminal_not_heard_from_yet_gets_blank_cells_not_a_label() {
        let text = placeholders(7, (16, 2), 0, Cursor::Moves);
        let plain: String = text.replace("\x1b[38;2;0;0;7m", "").replace("\x1b[39m", "").replace("\n\x1b[1G", "\n");
        let mut grid = grid_with(&plain, Color::Rgb(0, 0, 7));
        let images = HashMap::from([(7, image(640, 480, (16, 2)))]);
        let mut client = ClientImages::unknown();
        let regions = [Region { pane: 1, rect: Rect { x: 0, y: 0, w: 20, h: 3 }, images: &images }];
        map(&mut grid, &regions, &mut client, &mut |_| true);
        assert!((0..16).all(|x| grid.get(x, 0).text() == " " && grid.get(x, 1).text() == " "), "blank, no label");
        assert!(transmit(&grid, &mut client, &mut |_| None).is_empty(), "nothing sent");
    }

    #[test]
    fn a_sequence_begun_is_seen_to_its_end() {
        let mut g = PaneGraphics::default();
        assert!(g.idle());
        assert_eq!(g.scan(b"x\x1b_Ga=t;QUJD"), vec![Piece::Text(b"x".to_vec())]);
        assert!(!g.idle(), "inside one");
        assert_eq!(
            g.scan(b"RA==\x1b\\y"),
            vec![Piece::Graphics(b"Ga=t;QUJDRA==".to_vec()), Piece::Text(b"y".to_vec())]
        );
        assert!(g.idle());
    }

    #[test]
    fn a_terminal_without_pictures_gets_a_label() {
        let text = placeholders(7, (16, 2), 0, Cursor::Moves);
        let plain: String = text.replace("\x1b[38;2;0;0;7m", "").replace("\x1b[39m", "").replace("\n\x1b[1G", "\n");
        let mut grid = grid_with(&plain, Color::Rgb(0, 0, 7));
        let images = HashMap::from([(7, image(640, 480, (16, 2)))]);
        let mut client = ClientImages::new(false);
        let regions = [Region { pane: 1, rect: Rect { x: 0, y: 0, w: 20, h: 3 }, images: &images }];
        map(&mut grid, &regions, &mut client, &mut |_| true);
        let row0: String = (0..16).map(|x| grid.get(x, 0).text().to_string()).collect();
        assert_eq!(row0, "▧ image 640×480 ");
        assert!((0..16).all(|x| !grid.get(x, 1).text().starts_with(PLACEHOLDER)), "no placeholder left");
        assert!(transmit(&grid, &mut client, &mut |_| None).is_empty());
        // Not kept (its file gone): a label in a terminal with pictures too.
        let mut grid = grid_with(&plain, Color::Rgb(0, 0, 7));
        map(&mut grid, &regions, &mut ClientImages::new(true), &mut |_| false);
        assert_eq!(grid.get(0, 0).text(), "▧");
    }
}
