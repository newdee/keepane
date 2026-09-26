//! A terminal's input, as bytes (what a Unix terminal, or any host that is
//! not a Windows console, hands over), made into the key and mouse records
//! the rest of keepane reads: the same records a Windows console gives, so
//! the server needs no second way of reading keys.
//!
//! Keys: text, control bytes (0x02 is C-b, read by `key_from_record` as
//! tmux reads it), ESC plus a key for Alt, the CSI and SS3 sequences xterm
//! sends for arrows, Home/End, PgUp/PgDn, Insert/Delete, F1-F12 and
//! Shift+Tab, with xterm's modifier parameter. Bracketed paste comes as its
//! characters. Mouse: SGR reports (`ESC [ < b ; x ; y M/m`), as Windows
//! mouse events (buttons 1 left, 2 right, 4 middle; the wheel's delta in
//! the high word, positive up). A lone ESC is only a key once nothing else
//! follows it (`flush`, after a short wait).

use crate::ipc::{KeyRecord, MouseRecord};
use crate::keys::{
    LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, SHIFT_PRESSED, VK_DELETE, VK_DOWN, VK_END, VK_F1, VK_HOME, VK_INSERT, VK_LEFT,
    VK_NEXT, VK_PRIOR, VK_RIGHT, VK_TAB, VK_UP,
};

/// One thing the terminal said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Key(KeyRecord),
    Mouse(MouseRecord),
}

const MOUSE_MOVED: u32 = 0x0001;
const MOUSE_WHEELED: u32 = 0x0004;
const BTN_LEFT: u32 = 1;
const BTN_RIGHT: u32 = 2;
const BTN_MIDDLE: u32 = 4;

/// Bytes in, records out; a sequence split across reads is kept until the
/// rest comes.
#[derive(Default)]
pub struct Parser {
    pending: Vec<u8>,
    /// The mouse buttons held, which an SGR release report does not repeat.
    held: u32,
    /// Inside a bracketed paste (after ESC [ 200 ~, until ESC [ 201 ~).
    pasting: bool,
}

const PASTE_END: &[u8] = b"\x1b[201~";

fn key(vk: u16, ch: u16, ctrl: u32) -> Input {
    Input::Key(KeyRecord { down: true, repeat: 1, vk, sc: 0, ch, ctrl })
}

/// xterm's modifier parameter (1 + shift 1 + alt 2 + ctrl 4) as Windows'
/// control-key state.
fn modifiers(param: u32) -> u32 {
    let m = param.saturating_sub(1);
    let mut s = 0;
    if m & 1 != 0 {
        s |= SHIFT_PRESSED;
    }
    if m & 2 != 0 {
        s |= LEFT_ALT_PRESSED;
    }
    if m & 4 != 0 {
        s |= LEFT_CTRL_PRESSED;
    }
    s
}

/// The key of a `CSI <n> ~` sequence.
fn tilde_key(n: u32) -> Option<u16> {
    Some(match n {
        1 | 7 => VK_HOME,
        2 => VK_INSERT,
        3 => VK_DELETE,
        4 | 8 => VK_END,
        5 => VK_PRIOR,
        6 => VK_NEXT,
        11..=15 => VK_F1 + (n - 11) as u16,
        17..=21 => VK_F1 + 5 + (n - 17) as u16,
        23 | 24 => VK_F1 + 10 + (n - 23) as u16,
        _ => return None,
    })
}

/// The key of a final letter (`CSI A`, `SS3 P`, ...).
fn letter_key(b: u8) -> Option<u16> {
    Some(match b {
        b'A' => VK_UP,
        b'B' => VK_DOWN,
        b'C' => VK_RIGHT,
        b'D' => VK_LEFT,
        b'H' => VK_HOME,
        b'F' => VK_END,
        b'P' => VK_F1,
        b'Q' => VK_F1 + 1,
        b'R' => VK_F1 + 2,
        b'S' => VK_F1 + 3,
        _ => return None,
    })
}

/// What a complete sequence at the start of `b` is, and its length; `None`
/// when `b` is only its beginning.
enum Seq {
    Done(Vec<Input>, usize),
    Partial,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Read `bytes`; what they complete comes back.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Input> {
        let mut pending = std::mem::take(&mut self.pending);
        pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while at < pending.len() {
            match self.one(&pending[at..]) {
                Seq::Done(mut v, n) => {
                    out.append(&mut v);
                    at += n;
                }
                Seq::Partial => break,
            }
        }
        pending.drain(..at);
        self.pending = pending;
        out
    }

    /// Whatever is waiting for more, taken as it is: a lone ESC is the
    /// Escape key once nothing has followed it for a moment.
    pub fn flush(&mut self) -> Vec<Input> {
        let rest = std::mem::take(&mut self.pending);
        let mut out = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            if rest[i] == 0x1b {
                out.push(key(0, 0x1b, 0));
                i += 1;
                continue;
            }
            // An incomplete UTF-8 character or sequence tail: its bytes as
            // they are, readable ones as text.
            out.push(key(0, rest[i] as u16, 0));
            i += 1;
        }
        out
    }

    /// Whether something waits for more bytes (then `flush` after a pause).
    /// Not inside a paste: what is held there is waiting for the rest of
    /// the paste, not for a key's timeout.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty() && !self.pasting
    }

    fn one(&mut self, b: &[u8]) -> Seq {
        if self.pasting {
            return self.pasted(b);
        }
        let first = b[0];
        if first != 0x1b {
            return text(b);
        }
        let Some(&second) = b.get(1) else { return Seq::Partial };
        match second {
            b'[' => self.csi(b),
            b'O' => {
                let Some(&f) = b.get(2) else { return Seq::Partial };
                match letter_key(f) {
                    Some(vk) => Seq::Done(vec![key(vk, 0, 0)], 3),
                    None => Seq::Done(vec![key(0, 0x1b, 0)], 1),
                }
            }
            0x1b => Seq::Done(vec![key(0, 0x1b, 0)], 1),
            _ => {
                // ESC then a key: that key with Alt.
                match text(&b[1..]) {
                    Seq::Done(v, n) => Seq::Done(
                        v.into_iter()
                            .map(|i| match i {
                                Input::Key(mut k) => {
                                    k.ctrl |= LEFT_ALT_PRESSED;
                                    Input::Key(k)
                                }
                                m => m,
                            })
                            .collect(),
                        n + 1,
                    ),
                    Seq::Partial => Seq::Partial,
                }
            }
        }
    }

    /// Inside a bracketed paste: the text as it comes, as its characters,
    /// however it is cut into reads and however long between them (a big
    /// paste over SSH). Held back only: what may be the start of the end
    /// marker, and a character not all there yet.
    fn pasted(&mut self, b: &[u8]) -> Seq {
        if b.starts_with(PASTE_END) {
            self.pasting = false;
            return Seq::Done(Vec::new(), PASTE_END.len());
        }
        let (stop, whole) = match b.windows(PASTE_END.len()).position(|w| w == PASTE_END) {
            Some(at) => (at, true),
            None => {
                let maybe_end = (1..PASTE_END.len()).rev().find(|&k| b.ends_with(&PASTE_END[..k])).unwrap_or(0);
                (b.len() - maybe_end, false)
            }
        };
        let text = &b[..stop];
        // Before the end marker everything is there; otherwise a character
        // cut short waits for its other bytes.
        let take = match std::str::from_utf8(text) {
            Err(e) if !whole && e.error_len().is_none() => e.valid_up_to(),
            _ => text.len(),
        };
        if take == 0 {
            return Seq::Partial;
        }
        let mut out = Vec::new();
        for c in String::from_utf8_lossy(&text[..take]).chars() {
            push_char(&mut out, c, 0);
        }
        Seq::Done(out, take)
    }

    fn csi(&mut self, b: &[u8]) -> Seq {
        // ESC [ <params> <final>, the final byte in 0x40..=0x7e.
        let Some(end) = b[2..].iter().position(|c| (0x40..=0x7e).contains(c)) else {
            // Nothing but parameter bytes so far: wait, unless it is too long
            // to be a sequence at all.
            return if b.len() > 32 { Seq::Done(vec![key(0, 0x1b, 0)], 1) } else { Seq::Partial };
        };
        let end = end + 2;
        let fin = b[end];
        let body = &b[2..end];
        let len = end + 1;
        if body.first() == Some(&b'<') && (fin == b'M' || fin == b'm') {
            return Seq::Done(self.sgr_mouse(&body[1..], fin == b'M').into_iter().collect(), len);
        }
        let params: Vec<u32> =
            String::from_utf8_lossy(body).split(';').map(|p| p.parse().unwrap_or(0)).collect::<Vec<_>>();
        if fin == b'~' {
            let n = params.first().copied().unwrap_or(0);
            if n == 200 {
                // Bracketed paste: from here to ESC [ 201 ~ is text.
                self.pasting = true;
                return Seq::Done(Vec::new(), len);
            }
            let mods = modifiers(params.get(1).copied().unwrap_or(1));
            return match tilde_key(n) {
                Some(vk) => Seq::Done(vec![key(vk, 0, mods)], len),
                None => Seq::Done(Vec::new(), len),
            };
        }
        if fin == b'Z' {
            return Seq::Done(vec![key(VK_TAB, 0x09, SHIFT_PRESSED)], len);
        }
        // F1-F4 with modifiers are `CSI 1 ; m P..S`; a cursor position
        // report, `CSI row ; col R`, ends in R too, and is not a key.
        if matches!(fin, b'P' | b'Q' | b'R' | b'S') && params.first().is_some_and(|&p| p != 1) {
            return Seq::Done(Vec::new(), len);
        }
        let mods = modifiers(params.get(1).copied().unwrap_or(1));
        match letter_key(fin) {
            Some(vk) => Seq::Done(vec![key(vk, 0, mods)], len),
            // Anything else (focus reports, cursor position replies): nothing.
            None => Seq::Done(Vec::new(), len),
        }
    }

    fn sgr_mouse(&mut self, body: &[u8], press: bool) -> Option<Input> {
        let s = String::from_utf8_lossy(body);
        let mut it = s.split(';').map(|p| p.parse::<u32>().ok());
        let (b, x, y) = (it.next()??, it.next()??, it.next()??);
        let mut ctrl = 0;
        if b & 4 != 0 {
            ctrl |= SHIFT_PRESSED;
        }
        if b & 8 != 0 {
            ctrl |= LEFT_ALT_PRESSED;
        }
        if b & 16 != 0 {
            ctrl |= LEFT_CTRL_PRESSED;
        }
        let (x, y) = (x.saturating_sub(1) as i16, y.saturating_sub(1) as i16);
        if b & 64 != 0 {
            // The wheel: 64 up, 65 down; the delta in the high word.
            let delta: i16 = if b & 1 == 0 { 120 } else { -120 };
            return Some(Input::Mouse(MouseRecord {
                x,
                y,
                buttons: ((delta as u16 as u32) << 16) | self.held,
                ctrl,
                flags: MOUSE_WHEELED,
            }));
        }
        let button = match b & 3 {
            0 => BTN_LEFT,
            1 => BTN_MIDDLE,
            2 => BTN_RIGHT,
            _ => 0,
        };
        let motion = b & 32 != 0;
        if !motion {
            if press {
                self.held |= button;
            } else {
                self.held &= !button;
            }
        }
        Some(Input::Mouse(MouseRecord { x, y, buttons: self.held, ctrl, flags: if motion { MOUSE_MOVED } else { 0 } }))
    }
}

/// One character of text (a control byte included) at the start of `b`.
fn text(b: &[u8]) -> Seq {
    let first = b[0];
    let need = match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A stray continuation byte: skip it.
        _ => return Seq::Done(Vec::new(), 1),
    };
    if b.len() < need {
        return Seq::Partial;
    }
    let mut out = Vec::new();
    match std::str::from_utf8(&b[..need]) {
        Ok(s) => {
            for c in s.chars() {
                push_char(&mut out, c, 0);
            }
        }
        Err(_) => return Seq::Done(Vec::new(), 1),
    }
    Seq::Done(out, need)
}

/// A character as key records: one, or a surrogate pair's two halves (as a
/// Windows console gives a character outside the BMP).
fn push_char(out: &mut Vec<Input>, c: char, ctrl: u32) {
    let mut units = [0u16; 2];
    for u in c.encode_utf16(&mut units) {
        out.push(key(0, *u, ctrl));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{Key, KeyCode, key_from_record};

    fn keys(p: &mut Parser, b: &[u8]) -> Vec<Key> {
        p.feed(b)
            .into_iter()
            .filter_map(|i| match i {
                Input::Key(k) => key_from_record(&k),
                _ => None,
            })
            .collect()
    }

    fn k(code: KeyCode, ctrl: bool, alt: bool, shift: bool) -> Key {
        Key { code, ctrl, alt, shift }
    }

    /// Whatever a terminal sends, cut wherever the reads fall, the parser
    /// never panics, never holds more than a sequence's worth, and gives
    /// every byte back once the input stops (`flush`). A fixed generator,
    /// so a failure repeats; bytes weighted towards what sequences are
    /// made of.
    #[test]
    fn any_bytes_in_any_pieces_are_survived() {
        let mut seed: u64 = 0x5eed_cafe_f00d_d00d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        const LIKELY: &[u8] = b"\x1b[;0123456789<>?~ABCDHFPQRSMmOZu\x07\\]\xc3\xa9\xe4\xb8\xad\xf0\x9f\x98\x80\r\x7f";
        let mut p = Parser::new();
        let mut fed = 0usize;
        while fed < 200_000 {
            let len = (next() % 24) as usize + 1;
            let chunk: Vec<u8> = (0..len)
                .map(|_| {
                    let r = next();
                    if r % 4 == 0 { (r >> 8) as u8 } else { LIKELY[(r >> 8) as usize % LIKELY.len()] }
                })
                .collect();
            fed += chunk.len();
            let _ = p.feed(&chunk);
            assert!(p.pending.len() <= 4096, "held {} bytes", p.pending.len());
            if next() % 50 == 0 {
                let _ = p.flush();
                assert!(p.pending.is_empty() && !p.waiting(), "flush leaves nothing behind");
            }
        }
    }

    #[test]
    fn text_control_bytes_and_alt() {
        let mut p = Parser::new();
        assert_eq!(
            keys(&mut p, b"ab"),
            vec![k(KeyCode::Char('a'), false, false, false), k(KeyCode::Char('b'), false, false, false)]
        );
        // 0x02 is C-b, the prefix; Enter, Tab, Backspace as tmux reads them.
        assert_eq!(keys(&mut p, b"\x02"), vec![k(KeyCode::Char('b'), true, false, false)]);
        assert_eq!(
            keys(&mut p, b"\r\t\x7f"),
            vec![
                k(KeyCode::Enter, false, false, false),
                k(KeyCode::Tab, false, false, false),
                k(KeyCode::BSpace, false, false, false)
            ]
        );
        assert_eq!(keys(&mut p, b"\x1bx"), vec![k(KeyCode::Char('x'), false, true, false)], "ESC then x is M-x");
        assert_eq!(keys(&mut p, "中".as_bytes()), vec![k(KeyCode::Char('中'), false, false, false)]);
    }

    #[test]
    fn cursor_and_function_keys_with_modifiers() {
        let mut p = Parser::new();
        assert_eq!(
            keys(&mut p, b"\x1b[A\x1bOB\x1b[1;5C\x1b[1;3D"),
            vec![
                k(KeyCode::Up, false, false, false),
                k(KeyCode::Down, false, false, false),
                k(KeyCode::Right, true, false, false),
                k(KeyCode::Left, false, true, false),
            ]
        );
        assert_eq!(
            keys(&mut p, b"\x1b[5~\x1b[6~\x1b[3~\x1b[H\x1b[F"),
            vec![
                k(KeyCode::PPage, false, false, false),
                k(KeyCode::NPage, false, false, false),
                k(KeyCode::DC, false, false, false),
                k(KeyCode::Home, false, false, false),
                k(KeyCode::End, false, false, false),
            ]
        );
        assert_eq!(
            keys(&mut p, b"\x1bOP\x1b[15~\x1b[24~"),
            vec![
                k(KeyCode::F(1), false, false, false),
                k(KeyCode::F(5), false, false, false),
                k(KeyCode::F(12), false, false, false),
            ]
        );
        assert_eq!(keys(&mut p, b"\x1b[Z"), vec![k(KeyCode::Tab, false, false, true)], "Shift+Tab");
    }

    #[test]
    fn a_sequence_split_across_reads_and_a_lone_escape() {
        let mut p = Parser::new();
        assert!(keys(&mut p, b"\x1b[1;").is_empty());
        assert!(p.waiting());
        assert_eq!(keys(&mut p, b"5A"), vec![k(KeyCode::Up, true, false, false)]);
        assert!(!p.waiting());
        // ESC alone waits; once nothing follows it is Escape.
        assert!(keys(&mut p, b"\x1b").is_empty());
        let flushed: Vec<Key> = p
            .flush()
            .into_iter()
            .filter_map(|i| match i {
                Input::Key(k) => key_from_record(&k),
                _ => None,
            })
            .collect();
        assert_eq!(flushed, vec![k(KeyCode::Escape, false, false, false)]);
        // Half a UTF-8 character waits for the rest.
        let bytes = "é".as_bytes();
        assert!(keys(&mut p, &bytes[..1]).is_empty());
        assert_eq!(keys(&mut p, &bytes[1..]), vec![k(KeyCode::Char('é'), false, false, false)]);
        // Replies the terminal sends that are not keys come to nothing.
        assert!(keys(&mut p, b"\x1b[12;40R\x1b[I").is_empty());
    }

    #[test]
    fn bracketed_paste_comes_as_its_characters() {
        let mut p = Parser::new();
        let got = keys(&mut p, b"\x1b[200~hi\r\x1b[201~z");
        assert_eq!(
            got,
            vec![
                k(KeyCode::Char('h'), false, false, false),
                k(KeyCode::Char('i'), false, false, false),
                k(KeyCode::Enter, false, false, false),
                k(KeyCode::Char('z'), false, false, false),
            ]
        );
        // In pieces, with pauses a key's timeout would end (a big paste over
        // SSH): each piece's text comes at once, nothing waits for the
        // timeout, and the end marker and a character cut in two are
        // put together across the pieces.
        let c = |ch| k(KeyCode::Char(ch), false, false, false);
        assert_eq!(keys(&mut p, b"\x1b[200~ab"), vec![c('a'), c('b')]);
        assert!(!p.waiting(), "a paste does not time out");
        assert_eq!(
            keys(&mut p, b"\x1bx"),
            vec![k(KeyCode::Escape, false, false, false), c('x')],
            "ESC in a paste is text"
        );
        let zhong = "中".as_bytes();
        assert_eq!(keys(&mut p, &[b"c".as_slice(), &zhong[..1]].concat()), vec![c('c')]);
        assert!(!p.waiting());
        assert_eq!(keys(&mut p, &[&zhong[1..], b"\x1b[20".as_slice()].concat()), vec![c('中')]);
        assert!(!p.waiting(), "the start of the end marker waits for its rest, not a timeout");
        assert_eq!(keys(&mut p, b"1~\x1b[A"), vec![k(KeyCode::Up, false, false, false)], "after the paste, keys again");
        // Something that only starts like the end marker is text.
        assert_eq!(
            keys(&mut p, b"\x1b[200~\x1b[2x\x1b[201~"),
            vec![k(KeyCode::Escape, false, false, false), c('['), c('2'), c('x')]
        );
        assert!(!p.pasting);
    }

    #[test]
    fn sgr_mouse_reports_become_windows_mouse_events() {
        let mut p = Parser::new();
        let m = |i: &Input| match i {
            Input::Mouse(m) => *m,
            _ => panic!("not a mouse event: {i:?}"),
        };
        let v = p.feed(b"\x1b[<0;10;5M");
        assert_eq!(m(&v[0]), MouseRecord { x: 9, y: 4, buttons: 1, ctrl: 0, flags: 0 }, "left press, 0-based");
        let v = p.feed(b"\x1b[<32;11;5M");
        assert_eq!(
            m(&v[0]),
            MouseRecord { x: 10, y: 4, buttons: 1, ctrl: 0, flags: MOUSE_MOVED },
            "drag keeps it held"
        );
        let v = p.feed(b"\x1b[<0;11;5m");
        assert_eq!(m(&v[0]).buttons, 0, "released");
        let v = p.feed(b"\x1b[<2;1;1M");
        assert_eq!(m(&v[0]).buttons, 2, "right");
        p.feed(b"\x1b[<2;1;1m");
        let up = m(&p.feed(b"\x1b[<64;3;3M")[0]);
        assert_eq!(up.flags, MOUSE_WHEELED);
        assert!((up.buttons >> 16) as i16 > 0, "wheel up is positive");
        let down = m(&p.feed(b"\x1b[<65;3;3M")[0]);
        assert!(((down.buttons >> 16) as i16) < 0, "wheel down is negative");
        let ctrl = m(&p.feed(b"\x1b[<16;1;1M")[0]);
        assert_eq!(ctrl.ctrl, LEFT_CTRL_PRESSED);
    }
}
