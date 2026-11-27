//! Pictures in the board's previews (docs/design/kitty-graphics.md 4.7):
//! a PNG file shown with kitty's graphics protocol when the board runs in a
//! keepane pane that takes it (`KEEPANE_GRAPHICS=kitty`, `kitty-graphics`
//! on). The board writes the placeholders itself (a virtual placement), as
//! lines of the preview, and the picture once, before the frame that first
//! shows it (`take`): keepane keeps it, and shows it in a terminal that
//! shows pictures or a label in one that does not. Any other file, or a
//! board outside keepane, is left to `ut view` or the plain preview.

use crate::server::kitty::{self, Format, Image};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The largest picture file sent (bigger ones are left to `ut view`).
const MOST_BYTES: u64 = 20 * 1024 * 1024;
/// The widest a picture is drawn, in columns.
pub const MOST_COLUMNS: usize = 80;
/// Numbers the board gives its pictures start here (the pane's own range,
/// below what keepane gives pictures sent without one).
const FIRST: u32 = 0x00E0_0000;

/// A picture file as it was when sent, and the width drawn: sent again
/// when it changes or is drawn at another width.
type Key = (PathBuf, Option<SystemTime>, u64, u16);

/// A picture sent: its number, and the columns and rows it takes.
type Sent = (u32, (u16, u16));

pub struct Pictures {
    /// The pane takes kitty's pictures.
    pub on: bool,
    sent: RefCell<HashMap<Key, Sent>>,
    next: Cell<u32>,
    pending: RefCell<Vec<u8>>,
}

impl Default for Pictures {
    fn default() -> Self {
        Pictures::new(false)
    }
}

impl Pictures {
    pub fn new(on: bool) -> Pictures {
        Pictures { on, sent: RefCell::new(HashMap::new()), next: Cell::new(FIRST), pending: RefCell::new(Vec::new()) }
    }

    /// From the board's environment: in a keepane pane with them on.
    pub fn from_env() -> Pictures {
        let get = |k: &str| std::env::var(k).ok();
        Pictures::new(
            get("TERM_PROGRAM").as_deref() == Some("keepane") && get("KEEPANE_GRAPHICS").as_deref() == Some("kitty"),
        )
    }

    /// The preview lines that show the PNG file `path` in `width` columns
    /// (placeholders, a row a line), its picture queued for `take` the
    /// first time; None for anything else (not on, not a PNG, too big,
    /// unreadable).
    pub fn lines(&self, path: &Path, width: usize) -> Option<Vec<String>> {
        if !self.on {
            return None;
        }
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MOST_BYTES {
            return None;
        }
        let columns = width.clamp(10, MOST_COLUMNS) as u16;
        let key: Key = (path.to_path_buf(), meta.modified().ok(), meta.len(), columns);
        let known = self.sent.borrow().get(&key).copied();
        let (id, (cols, rows)) = match known {
            Some(k) => k,
            None => {
                let data = std::fs::read(path).ok()?;
                let (Format::Png, w, h) = kitty::sniff(&data)? else { return None };
                let rows = kitty::rows_for(columns, (w, h), kitty::CELL);
                let id = self.next.get();
                self.next.set(if id >= 0x00EF_FFFF { FIRST } else { id + 1 });
                let image = Image {
                    hash: String::new(),
                    format: Format::Png,
                    width: w,
                    height: h,
                    cells: Some((columns, rows)),
                };
                self.pending.borrow_mut().extend(kitty::transmission(id, &image, &data));
                self.sent.borrow_mut().insert(key, (id, (columns, rows)));
                (id, (columns, rows))
            }
        };
        Some((0..rows).map(|row| kitty::placeholder_row(id, row, cols)).collect())
    }

    /// What to write before the next frame: the pictures it shows for the
    /// first time.
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.pending.borrow_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(b"\x08\x06\0\0\0rest");
        v
    }

    #[test]
    fn a_png_is_sent_once_and_drawn_as_placeholder_rows() {
        let dir = std::env::temp_dir().join(format!("kp-board-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        std::fs::write(&file, png(100, 50)).unwrap();
        let p = Pictures::new(true);
        // 20 columns of 10×20 cells: 20 × 10 × 50 / 100 / 20 = 5 rows.
        let lines = p.lines(&file, 20).unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(super::super::width_of(&lines[0]), 20, "as wide as asked");
        assert_eq!(lines[1].matches(kitty::PLACEHOLDER).count(), 20);
        let sent = String::from_utf8(p.take()).unwrap();
        assert!(sent.starts_with(&format!("\x1b_Ga=t,t=d,f=100,i={FIRST},q=2,m=0;")), "{sent}");
        assert!(sent.ends_with(&format!("\x1b_Ga=p,U=1,i={FIRST},c=20,r=5,q=2\x1b\\")));
        // Drawn again: nothing more to send; at another width, sent again.
        assert_eq!(p.lines(&file, 20).unwrap(), lines);
        assert!(p.take().is_empty());
        assert_eq!(p.lines(&file, 30).unwrap().len(), 8, "30 × 10 × 50 / 100 / 20 = 7.5: 8 rows");
        assert!(!p.take().is_empty());
        // Not a PNG, or off: left to the others.
        std::fs::write(dir.join("b.jpg"), [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        assert!(p.lines(&dir.join("b.jpg"), 20).is_none());
        assert!(Pictures::new(false).lines(&file, 20).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
