//! A key, as a pane's program is to get it: the VT sequence a terminal
//! sends for it (xterm's), text as UTF-8.

use crate::ipc::KeyRecord;
use std::cell::Cell;

thread_local! {
    /// The first half of a character outside the BMP, which arrives as two
    /// records (as a Windows console gives it).
    static HIGH: Cell<Option<u16>> = const { Cell::new(None) };
}

/// The bytes that give `rec` to a pane whose program is in cursor-key
/// application mode or not (`ESC O A` or `ESC [ A` for Up).
pub fn to_pane(rec: &KeyRecord, app_cursor: bool) -> Vec<u8> {
    if !rec.down {
        return Vec::new();
    }
    if (0xD800..0xDC00).contains(&rec.ch) {
        HIGH.with(|h| h.set(Some(rec.ch)));
        return Vec::new();
    }
    if (0xDC00..0xE000).contains(&rec.ch) {
        let Some(hi) = HIGH.with(|h| h.take()) else { return Vec::new() };
        return char::decode_utf16([hi, rec.ch])
            .next()
            .and_then(|r| r.ok())
            .map(|c| c.to_string().into_bytes())
            .unwrap_or_default();
    }
    let Some(key) = crate::keys::key_from_record(rec) else { return Vec::new() };
    crate::server::input::encode_key(key, app_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, VK_UP};

    fn rec(vk: u16, ch: u16, ctrl: u32) -> KeyRecord {
        KeyRecord { down: true, repeat: 1, vk, sc: 0, ch, ctrl }
    }

    #[test]
    fn keys_become_what_a_terminal_sends() {
        assert_eq!(to_pane(&rec(0, 'a' as u16, 0), false), b"a");
        assert_eq!(to_pane(&rec(0, '中' as u16, 0), false), "中".as_bytes());
        assert_eq!(to_pane(&rec(0, 0x02, 0), false), b"\x02", "C-b as its byte");
        assert_eq!(to_pane(&rec(0x42, 0, LEFT_CTRL_PRESSED), false), b"\x02", "Ctrl+B from a key code");
        assert_eq!(to_pane(&rec(0, 'x' as u16, LEFT_ALT_PRESSED), false), b"\x1bx", "Alt is ESC first");
        assert_eq!(to_pane(&rec(VK_UP, 0, 0), false), b"\x1b[A");
        assert_eq!(to_pane(&rec(VK_UP, 0, 0), true), b"\x1bOA", "application cursor keys");
        let up = KeyRecord { down: false, ..rec(0, 'a' as u16, 0) };
        assert!(to_pane(&up, false).is_empty(), "a key-up is nothing");
        // A character outside the BMP, in two halves.
        let mut units = [0u16; 2];
        '😀'.encode_utf16(&mut units);
        assert!(to_pane(&rec(0, units[0], 0), false).is_empty());
        assert_eq!(to_pane(&rec(0, units[1], 0), false), "😀".as_bytes());
    }
}
