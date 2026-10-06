//! Encoding of client input for a pane's ConPTY.

use crate::ipc::KeyRecord;
use crate::keys::{Key, KeyCode};

/// win32-input-mode sequence understood by ConPTY: `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`.
pub fn encode_key_record(r: &KeyRecord) -> Vec<u8> {
    format!("\x1b[{};{};{};{};{};{}_", r.vk, r.sc, r.ch, u8::from(r.down), r.ctrl, r.repeat.max(1)).into_bytes()
}

/// Plain VT (xterm) encoding of a logical key, for `send-keys` and keys
/// synthesized by keepane itself.
pub fn encode_key(k: Key, app_cursor: bool) -> Vec<u8> {
    let modifier = 1 + u8::from(k.shift) + 2 * u8::from(k.alt) + 4 * u8::from(k.ctrl);
    let with_mod = |code: &str, final_: char| -> Vec<u8> {
        if modifier == 1 {
            format!("\x1b[{code}{final_}").into_bytes()
        } else if code.is_empty() {
            format!("\x1b[1;{modifier}{final_}").into_bytes()
        } else {
            format!("\x1b[{code};{modifier}{final_}").into_bytes()
        }
    };
    let cursor = |c: char| -> Vec<u8> {
        if modifier == 1 && app_cursor { format!("\x1bO{c}").into_bytes() } else { with_mod("", c) }
    };
    match k.code {
        KeyCode::Char(c) => {
            let mut out = Vec::new();
            if k.alt {
                out.push(0x1b);
            }
            if k.ctrl {
                let b = match c {
                    ' ' | '@' | '2' => 0u8,
                    'a'..='z' => c as u8 - b'a' + 1,
                    'A'..='Z' => c.to_ascii_lowercase() as u8 - b'a' + 1,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '7' | '-' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => {
                        let mut s = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut s).as_bytes());
                        return out;
                    }
                };
                out.push(b);
            } else {
                let mut s = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut s).as_bytes());
            }
            out
        }
        KeyCode::Enter => {
            if modifier == 1 {
                b"\r".to_vec()
            } else {
                // xterm modifyOtherKeys form.
                format!("\x1b[27;{modifier};13~").into_bytes()
            }
        }
        KeyCode::Escape => b"\x1b".to_vec(),
        KeyCode::Tab => {
            if k.shift {
                b"\x1b[Z".to_vec()
            } else if k.alt {
                b"\x1b\t".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        KeyCode::BSpace => {
            if k.alt {
                b"\x1b\x7f".to_vec()
            } else if k.ctrl {
                b"\x08".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::PPage => with_mod("5", '~'),
        KeyCode::NPage => with_mod("6", '~'),
        KeyCode::IC => with_mod("2", '~'),
        KeyCode::DC => with_mod("3", '~'),
        KeyCode::F(n) => match n {
            1..=4 if modifier == 1 => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
            1..=4 => with_mod("1", (b'P' + n - 1) as char),
            5 => with_mod("15", '~'),
            6 => with_mod("17", '~'),
            7 => with_mod("18", '~'),
            8 => with_mod("19", '~'),
            9 => with_mod("20", '~'),
            10 => with_mod("21", '~'),
            11 => with_mod("23", '~'),
            12 => with_mod("24", '~'),
            _ => Vec::new(),
        },
    }
}

/// Encode a `send-keys` argument: a key name if it parses as one and is not a
/// single character, else the literal text.
pub fn encode_send_key(word: &str, app_cursor: bool) -> Vec<u8> {
    if word.chars().count() > 1
        && let Some(k) = Key::parse(word)
    {
        return encode_key(k, app_cursor);
    }
    word.as_bytes().to_vec()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    None,
}

/// SGR (1006) mouse report. `release` selects the `m` final byte.
#[allow(clippy::too_many_arguments)]
pub fn encode_mouse_sgr(
    button: MouseButton,
    motion: bool,
    release: bool,
    shift: bool,
    alt: bool,
    ctrl: bool,
    col: u16,
    row: u16,
) -> Vec<u8> {
    let mut b: u16 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if shift {
        b += 4;
    }
    if alt {
        b += 8;
    }
    if ctrl {
        b += 16;
    }
    if motion {
        b += 32;
    }
    format!("\x1b[<{b};{};{}{}", col + 1, row + 1, if release { 'm' } else { 'M' }).into_bytes()
}

/// Text paste, wrapped in bracketed-paste markers if the pane asked for them.
/// What the mouse wheel over a pane at (`x`, `y`) sends its program: a
/// wheel event when it asked for the mouse; in the alternate screen
/// without that, three arrow keys (a full-screen program scrolls its own
/// view; there is no scrollback there). None on the normal screen: the
/// wheel scrolls keepane's history then, not the program.
pub fn wheel(s: &vt100::Screen, up: bool, x: u16, y: u16, (shift, alt, ctrl): (bool, bool, bool)) -> Option<Vec<u8>> {
    if s.mouse_protocol_mode() != vt100::MouseProtocolMode::None {
        let b = if up { MouseButton::WheelUp } else { MouseButton::WheelDown };
        return Some(encode_mouse_sgr(b, false, false, shift, alt, ctrl, x, y));
    }
    if !s.alternate_screen() {
        return None;
    }
    let k = if up { KeyCode::Up } else { KeyCode::Down };
    Some((0..3).flat_map(|_| encode_key(Key::plain(k), s.application_cursor())).collect())
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{body}\x1b[201~").into_bytes() } else { body.into_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_record() {
        let r = KeyRecord { down: true, repeat: 1, vk: 0x41, sc: 30, ch: 97, ctrl: 0 };
        assert_eq!(encode_key_record(&r), b"\x1b[65;30;97;1;0;1_");
        let up = KeyRecord { down: false, ..r };
        assert_eq!(encode_key_record(&up), b"\x1b[65;30;97;0;0;1_");
    }

    #[test]
    fn vt_keys() {
        assert_eq!(encode_key(Key::ch('a'), false), b"a");
        assert_eq!(encode_key(Key::ctrl('c'), false), b"\x03");
        assert_eq!(encode_key(Key::ctrl(' '), false), b"\x00");
        assert_eq!(encode_key(Key::alt('x'), false), b"\x1bx");
        assert_eq!(encode_key(Key::plain(KeyCode::Enter), false), b"\r");
        assert_eq!(encode_key(Key::plain(KeyCode::Up), false), b"\x1b[A");
        assert_eq!(encode_key(Key::plain(KeyCode::Up), true), b"\x1bOA");
        assert_eq!(encode_key(Key::with_ctrl(KeyCode::Right), false), b"\x1b[1;5C");
        assert_eq!(encode_key(Key::with_shift(KeyCode::Tab), false), b"\x1b[Z");
        assert_eq!(encode_key(Key::plain(KeyCode::PPage), false), b"\x1b[5~");
        assert_eq!(encode_key(Key::with_shift(KeyCode::DC), false), b"\x1b[3;2~");
        assert_eq!(encode_key(Key::plain(KeyCode::F(1)), false), b"\x1bOP");
        assert_eq!(encode_key(Key::plain(KeyCode::F(5)), false), b"\x1b[15~");
        assert_eq!(encode_key(Key::with_alt(KeyCode::F(12)), false), b"\x1b[24;3~");
        assert_eq!(encode_key(Key::ch('é'), false), "é".as_bytes());
    }

    #[test]
    fn send_keys_words() {
        assert_eq!(encode_send_key("ls", false), b"ls");
        assert_eq!(encode_send_key("Enter", false), b"\r");
        assert_eq!(encode_send_key("C-c", false), b"\x03");
        assert_eq!(encode_send_key("x", false), b"x");
        assert_eq!(encode_send_key("Up", false), b"\x1b[A");
    }

    #[test]
    fn sgr_mouse() {
        assert_eq!(encode_mouse_sgr(MouseButton::Left, false, false, false, false, false, 0, 0), b"\x1b[<0;1;1M");
        assert_eq!(encode_mouse_sgr(MouseButton::Left, false, true, false, false, false, 9, 4), b"\x1b[<0;10;5m");
        assert_eq!(encode_mouse_sgr(MouseButton::WheelUp, false, false, false, false, true, 0, 0), b"\x1b[<80;1;1M");
        assert_eq!(encode_mouse_sgr(MouseButton::None, true, false, false, false, false, 2, 2), b"\x1b[<35;3;3M");
    }

    /// The wheel: to a program that asked for the mouse as a wheel event;
    /// to a full-screen one that did not as arrow keys (its cursor mode
    /// kept); on the normal screen, nothing (keepane's history scrolls).
    #[test]
    fn the_wheel_goes_where_a_terminal_sends_it() {
        let screen = |bytes: &str| {
            let mut p = vt100::Parser::new(24, 80, 100);
            p.process(bytes.as_bytes());
            p
        };
        let none = (false, false, false);
        assert_eq!(wheel(screen("").screen(), true, 4, 5, none), None, "the normal screen");
        assert_eq!(wheel(screen("\x1b[?1049h").screen(), true, 4, 5, none), Some(b"\x1b[A\x1b[A\x1b[A".to_vec()));
        assert_eq!(wheel(screen("\x1b[?1049h").screen(), false, 4, 5, none), Some(b"\x1b[B\x1b[B\x1b[B".to_vec()));
        assert_eq!(
            wheel(screen("\x1b[?1049h\x1b[?1h").screen(), true, 0, 0, none),
            Some(b"\x1bOA\x1bOA\x1bOA".to_vec())
        );
        let mouse = screen("\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        assert_eq!(wheel(mouse.screen(), true, 4, 5, none), Some(b"\x1b[<64;5;6M".to_vec()));
        assert_eq!(wheel(mouse.screen(), false, 4, 5, none), Some(b"\x1b[<65;5;6M".to_vec()));
        // Asked for the mouse on the normal screen too: the wheel is its.
        assert_eq!(wheel(screen("\x1b[?1000h").screen(), true, 0, 0, none), Some(b"\x1b[<64;1;1M".to_vec()));
    }

    #[test]
    fn paste() {
        assert_eq!(encode_paste("a\r\nb\n", false), b"a\rb\r");
        assert_eq!(encode_paste("x", true), b"\x1b[200~x\x1b[201~");
    }
}
