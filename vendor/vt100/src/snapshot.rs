//! (keepane) A screen's whole state as bytes, and back: both grids (the
//! alternate one and the scrollback included), the cursor and the cursor
//! saved, the scroll region, the modes, the colours. Another copy of the
//! same crate restores it exactly, so a copy started from it and fed the
//! same bytes after stays the same as the original, cell for cell.
//!
//! The format: `KPS` and a version byte, then the screen, little-endian.
//! A run of the same cell is written once with its count; a cell's
//! attributes only when they differ from the cell before. What a terminal
//! parser holds between two reads (an escape sequence cut in half) is not
//! part of it: take it between whole sequences.

const MAGIC: &[u8; 3] = b"KPS";
const VERSION: u8 = 1;

pub(crate) struct Writer {
    pub(crate) out: Vec<u8>,
}

impl Writer {
    pub(crate) fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    pub(crate) fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn bytes(&mut self, v: &[u8]) {
        self.out.extend_from_slice(v);
    }
}

pub(crate) struct Reader<'a> {
    pub(crate) at: &'a [u8],
}

impl Reader<'_> {
    pub(crate) fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.at.len() < n {
            return None;
        }
        let (head, rest) = self.at.split_at(n);
        self.at = rest;
        Some(head)
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a)
        })
    }
}

impl crate::Screen {
    /// (keepane) This screen's whole state as bytes (`snapshot` module).
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        let mut w = Writer { out: Vec::new() };
        w.bytes(MAGIC);
        w.u8(VERSION);
        self.write_snap(&mut w);
        w.out
    }

    /// (keepane) Become the screen `bytes` hold (`snapshot`), its size
    /// and scrollback limit included. False, and nothing changed, for
    /// bytes it did not write.
    pub fn restore(&mut self, bytes: &[u8]) -> bool {
        let mut r = Reader { at: bytes };
        if r.take(3) != Some(&MAGIC[..]) || r.u8() != Some(VERSION) {
            return false;
        }
        match Self::read_snap(&mut r) {
            Some(s) if r.at.is_empty() => {
                *self = s;
                true
            }
            _ => false,
        }
    }
}
