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
/// The widest a picture's frame is, in columns.
pub const MOST_COLUMNS: usize = 64;
/// The narrowest (`ut view` draws no narrower).
pub const LEAST_COLUMNS: usize = 10;
/// A frame's shape in pixels, width to height (4:3, a screen's).
const FRAME_SHAPE: (u32, u32) = (4, 3);

/// Where a picture is drawn in a preview: as wide as the panel lets it (up
/// to `MOST_COLUMNS`) and `FRAME_SHAPE` in pixels, the same for every
/// picture in that panel; the picture fitted inside, centred, the rest
/// filled. So a wide picture and a tall one take the same room, and what
/// comes after stays where it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
}

impl Frame {
    /// The frame of a preview `width` columns wide.
    pub fn for_width(width: usize) -> Frame {
        let cols = width.clamp(LEAST_COLUMNS, MOST_COLUMNS) as u32;
        let rows = cols * kitty::CELL.0 * FRAME_SHAPE.1 / (FRAME_SHAPE.0 * kitty::CELL.1);
        Frame { cols: cols as u16, rows: rows.max(1) as u16 }
    }

    /// A picture `width` × `height` pixels fitted inside, its shape kept:
    /// the columns and rows it takes (the whole frame when its size is
    /// not known).
    pub fn fit(self, (width, height): (u32, u32)) -> (u16, u16) {
        if width == 0 || height == 0 {
            return (self.cols, self.rows);
        }
        let rows = kitty::rows_for(self.cols, (width, height), kitty::CELL);
        if rows <= self.rows {
            return (self.cols, rows);
        }
        // Taller than the frame: as tall as it, narrower.
        let cols = f64::from(self.rows) * f64::from(kitty::CELL.1) * f64::from(width)
            / (f64::from(height) * f64::from(kitty::CELL.0));
        let cols = (cols.floor() as u16).clamp(1, self.cols);
        (cols, kitty::rows_for(cols, (width, height), kitty::CELL).min(self.rows))
    }

    /// `lines` (a picture `cols` wide) in the middle of the frame, cut to
    /// it; the room around in `fill` (an SGR ground, `ground`; empty: blank).
    pub fn place(self, mut lines: Vec<String>, cols: u16, fill: &str) -> Vec<String> {
        let cols = cols.min(self.cols);
        lines.truncate(usize::from(self.rows));
        let pad = |n: u16| {
            let blank = " ".repeat(usize::from(n));
            if fill.is_empty() || n == 0 { blank } else { format!("{fill}{blank}\x1b[0m") }
        };
        let left = (self.cols - cols) / 2;
        let right = self.cols - cols - left;
        let top = (usize::from(self.rows) - lines.len()) / 2;
        let bottom = usize::from(self.rows) - lines.len() - top;
        let empty = pad(self.cols);
        std::iter::repeat_n(empty.clone(), top)
            .chain(lines.iter().map(|l| format!("{}{}{}", pad(left), super::fit(l, usize::from(cols)), pad(right))))
            .chain(std::iter::repeat_n(empty, bottom))
            .collect()
    }
}

/// The SGR ground a frame is filled with, from a colour (the theme's
/// `@bar`); nothing for the terminal's own or one that does not read.
pub fn ground(colour: &str) -> String {
    match crate::config::parse_ui_style(&format!("bg={}", colour.trim())) {
        Ok(s) if s.bg != vt100::Color::Default => {
            crate::server::render::Style::colors(vt100::Color::Default, s.bg).sgr()
        }
        _ => String::new(),
    }
}

/// A picture file's size in pixels, from its first bytes.
pub fn size_of(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    // A JPEG's size comes after its other segments: its first 256 KiB.
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(256 * 1024).read_to_end(&mut head).ok()?;
    let (_, w, h) = kitty::sniff(&head)?;
    (w > 0 && h > 0).then_some((w, h))
}
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
    /// The ground of a picture's frame (`ground` of the theme's `@bar`).
    pub fill: String,
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
        Pictures {
            on,
            fill: String::new(),
            sent: RefCell::new(HashMap::new()),
            next: Cell::new(FIRST),
            pending: RefCell::new(Vec::new()),
        }
    }

    /// From the board's environment: in a keepane pane with them on.
    pub fn from_env() -> Pictures {
        let get = |k: &str| std::env::var(k).ok();
        Pictures::new(
            get("TERM_PROGRAM").as_deref() == Some("keepane") && get("KEEPANE_GRAPHICS").as_deref() == Some("kitty"),
        )
    }

    /// The preview lines that show the PNG file `path` in the frame of a
    /// preview `width` columns wide (`Frame`: placeholders, a row a line,
    /// in the middle), its picture queued for `take` the first time; None
    /// for anything else (not on, not a PNG, too big, unreadable).
    pub fn lines(&self, path: &Path, width: usize) -> Option<Vec<String>> {
        if !self.on {
            return None;
        }
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MOST_BYTES {
            return None;
        }
        let frame = Frame::for_width(width);
        let key: Key = (path.to_path_buf(), meta.modified().ok(), meta.len(), frame.cols);
        let known = self.sent.borrow().get(&key).copied();
        let (id, (cols, rows)) = match known {
            Some(k) => k,
            None => {
                let data = std::fs::read(path).ok()?;
                let (Format::Png, w, h) = kitty::sniff(&data)? else { return None };
                let (columns, rows) = frame.fit((w, h));
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
        let rows: Vec<String> = (0..rows).map(|row| kitty::placeholder_row(id, row, cols)).collect();
        Some(frame.place(rows, cols, &self.fill))
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
        // A frame 20 columns wide is 7 rows (4:3 on 10×20 cells); the
        // picture, 20 × 10 × 50 / 100 / 20 = 5 rows, in its middle.
        let lines = p.lines(&file, 20).unwrap();
        assert_eq!(lines.len(), 7);
        assert!(lines.iter().all(|l| super::super::width_of(l) == 20), "as wide as asked");
        let drawn: Vec<usize> = lines.iter().map(|l| l.matches(kitty::PLACEHOLDER).count()).collect();
        assert_eq!(drawn, [0, 20, 20, 20, 20, 20, 0]);
        let sent = String::from_utf8(p.take()).unwrap();
        assert!(sent.starts_with(&format!("\x1b_Ga=t,t=d,f=100,i={FIRST},q=2,m=0;")), "{sent}");
        assert!(sent.ends_with(&format!("\x1b_Ga=p,U=1,i={FIRST},c=20,r=5,q=2\x1b\\")));
        // Drawn again: nothing more to send; at another width, sent again.
        assert_eq!(p.lines(&file, 30).unwrap().len(), 11, "a frame 30 wide: 11 rows");
        assert!(!p.take().is_empty());
        assert_eq!(p.lines(&file, 20).unwrap(), lines);
        assert!(p.take().is_empty());
        // A tall picture takes the same frame: as tall as it, narrower, centred.
        std::fs::write(dir.join("tall.png"), png(50, 200)).unwrap();
        let tall = p.lines(&dir.join("tall.png"), 20).unwrap();
        assert_eq!(tall.len(), 7, "the same frame");
        let row = &tall[3];
        assert_eq!(row.matches(kitty::PLACEHOLDER).count(), 3, "7 rows × 20 × 50 / 200 / 10 = 3.5: 3 columns");
        assert!(row.starts_with("        "), "8 columns of room on the left: {row:?}");
        assert!(String::from_utf8(p.take()).unwrap().ends_with("c=3,r=6,q=2\x1b\\"));
        // Not a PNG, or off: left to the others.
        std::fs::write(dir.join("b.jpg"), [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        assert!(p.lines(&dir.join("b.jpg"), 20).is_none());
        assert!(Pictures::new(false).lines(&file, 20).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_keeps_its_size_and_centres_what_it_holds() {
        // 4:3 in pixels on 10×20 cells; at most 64 columns, at least 10.
        assert_eq!(Frame::for_width(40), Frame { cols: 40, rows: 15 });
        assert_eq!(Frame::for_width(200), Frame { cols: 64, rows: 24 });
        assert_eq!(Frame::for_width(3), Frame { cols: 10, rows: 3 });
        let f = Frame::for_width(40);
        // Wide: the whole width; tall: the whole height; unknown: the frame.
        assert_eq!(f.fit((1116, 814)), (40, 15));
        assert_eq!(f.fit((1600, 400)), (40, 5));
        assert_eq!(f.fit((1170, 2532)), (13, 15));
        assert_eq!(f.fit((0, 0)), (40, 15));
        // Placed: always the frame's rows and columns; the room in the ground.
        let ground = ground("#121212");
        assert_eq!(ground, "\x1b[0;48;2;18;18;18m");
        let lines = f.place(vec!["ab".into(); 3], 2, &ground);
        assert_eq!(lines.len(), 15);
        assert!(lines.iter().all(|l| super::super::width_of(l) == 40));
        assert_eq!(lines[6], format!("{ground}{}\x1b[0mab{ground}{}\x1b[0m", " ".repeat(19), " ".repeat(19)));
        assert_eq!(lines[0], format!("{ground}{}\x1b[0m", " ".repeat(40)));
        // More lines than rows: cut; no ground (the terminal's own): blank.
        let lines = f.place(vec!["x".into(); 30], 40, "");
        assert_eq!(lines.len(), 15);
        assert!(lines.iter().all(|l| !l.contains('\x1b')));
        assert_eq!(super::ground("default"), "");
        assert_eq!(super::ground("nonsense"), "");
    }
}
