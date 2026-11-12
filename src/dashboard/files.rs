//! `[0] Files`: the chosen pane's directory, a directory at a time, as
//! mini.files and lf walk one (j/k move, h up, l into), with what is under
//! the cursor shown on the right: a directory's entries, a file's text. It
//! starts where the chosen pane is, and goes there again when the pane
//! changes or moves (`cd`); `-` goes back there by hand.

use std::path::{Path, PathBuf};

use super::{clip, paint, width_of};

/// The most entries a directory is listed with (the rest are counted).
pub const LIST_MAX: usize = 5000;
/// The most of a file read for its preview.
pub const PREVIEW_MAX: u64 = 256 * 1024;
/// How much of a file is looked at to tell text from binary.
const SNIFF: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

#[derive(Default)]
pub struct Files {
    /// Where the browsing is.
    pub dir: Option<PathBuf>,
    pub entries: Vec<Entry>,
    /// Entries not listed (past `LIST_MAX`).
    pub more: usize,
    pub pick: usize,
    pub hidden: bool,
    /// Why the directory could not be read.
    pub error: Option<String>,
    /// The pane followed and its directory then: a new pane or a `cd`
    /// there takes the browsing along.
    followed: Option<(String, String)>,
}

/// `dir`'s entries, directories first, each part by name (case aside);
/// hidden ones (`is_hidden`) only with `hidden`; at most `LIST_MAX`, and
/// how many more there are.
pub fn list(dir: &Path, hidden: bool) -> std::io::Result<(Vec<Entry>, usize)> {
    let mut entries = Vec::new();
    let mut more = 0;
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if !hidden && is_hidden(&name, &e) {
            continue;
        }
        if entries.len() >= LIST_MAX {
            more += 1;
            continue;
        }
        // Through a link to what it names; a broken one is a file.
        let meta = std::fs::metadata(e.path()).ok();
        let dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(Entry { name, dir, size: meta.map_or(0, |m| if m.is_dir() { 0 } else { m.len() }) });
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok((entries, more))
}

/// Hidden: a name with a leading dot; on Windows also what the system marks
/// hidden (`.Bin`, `System Volume Information`), as Explorer hides it.
fn is_hidden(name: &str, e: &std::fs::DirEntry) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if e.metadata().is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0) {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = e;
    false
}

impl Files {
    /// The chosen pane (`id`) is in `path`: when that is not the pane or
    /// the directory last followed, the browsing goes there.
    pub fn follow(&mut self, id: &str, path: &str) {
        let now = Some((id.to_string(), path.to_string()));
        if self.followed != now {
            self.followed = now;
            self.go(PathBuf::from(path), None);
        }
    }

    /// Back to the followed pane's directory (`-`).
    pub fn back(&mut self) {
        if let Some((_, path)) = self.followed.clone() {
            self.go(PathBuf::from(path), None);
        }
    }

    /// Browse `dir`, the cursor on `on` (by name) when given.
    fn go(&mut self, dir: PathBuf, on: Option<&str>) {
        self.dir = Some(dir);
        self.pick = 0;
        self.read();
        if let Some(on) = on {
            self.pick = self.entries.iter().position(|e| e.name == on).unwrap_or(0);
        }
    }

    /// Read the directory again (each refresh): the cursor stays on the
    /// entry it was on, by name.
    pub fn read(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        let on = self.entries.get(self.pick).map(|e| e.name.clone());
        match list(&dir, self.hidden) {
            Ok((entries, more)) => {
                self.entries = entries;
                self.more = more;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.more = 0;
                self.error = Some(e.to_string());
            }
        }
        self.pick = on
            .and_then(|on| self.entries.iter().position(|e| e.name == on))
            .unwrap_or_else(|| self.pick.min(self.entries.len().saturating_sub(1)));
    }

    pub fn step(&mut self, by: isize) {
        let last = self.entries.len().saturating_sub(1) as isize;
        self.pick = (self.pick as isize + by).clamp(0, last.max(0)) as usize;
    }

    /// Up to the parent directory, the cursor on the one left.
    pub fn up(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        let Some(parent) = dir.parent().map(Path::to_path_buf) else { return };
        let left = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        self.go(parent, left.as_deref());
    }

    /// Into the directory under the cursor; true when it is one.
    pub fn enter(&mut self) -> bool {
        match (self.dir.clone(), self.entries.get(self.pick)) {
            (Some(dir), Some(e)) if e.dir => {
                let to = dir.join(&e.name);
                self.go(to, None);
                true
            }
            _ => false,
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.hidden = !self.hidden;
        self.read();
    }

    /// What is under the cursor.
    pub fn picked(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(&self.entries.get(self.pick)?.name))
    }

    /// The directory as a person reads it: the home directory as `~`.
    pub fn where_(&self) -> String {
        let Some(dir) = &self.dir else { return String::new() };
        let shown = dir.to_string_lossy().into_owned();
        match dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) {
            Some(h) if !h.is_empty() && shown.starts_with(&h) => format!("~{}", &shown[h.len()..]),
            _ => shown,
        }
    }

    /// The panel's rows: `rows` of entries in `width` columns, the cursor's
    /// in view; directories blue with a `/`.
    pub fn rows(&self, rows: usize, width: usize) -> Vec<(bool, String)> {
        if let Some(e) = &self.error {
            return vec![(false, paint("31", &clip(&format!("cannot read: {e}"), width).0))];
        }
        if self.dir.is_none() {
            return vec![(false, paint("2", "no pane chosen"))];
        }
        if self.entries.is_empty() {
            let hint = if self.hidden { "empty" } else { "empty (. shows hidden)" };
            return vec![(false, paint("2", hint))];
        }
        let room = rows.saturating_sub(usize::from(self.more > 0));
        let start = super::window_start(self.pick, self.entries.len(), room);
        let mut out: Vec<(bool, String)> = self
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(room)
            .map(|(i, e)| {
                let line = if e.dir {
                    paint("1;34", &format!("{}/", e.name))
                } else {
                    let size = size_text(e.size);
                    let name_w = width.saturating_sub(size.len() + 1);
                    let name = clip(&e.name, name_w).0;
                    let pad = " ".repeat(width.saturating_sub(width_of(&name) + size.len()));
                    format!("{name}{pad}{}", paint("2", &size))
                };
                (i == self.pick, line)
            })
            .collect();
        if self.more > 0 {
            out.push((false, paint("2", &format!("… {} more", self.more))));
        }
        out
    }
}

/// A file's size as a person reads it: `512B`, `4.0K`, `1.2M`, `3.4G`.
pub fn size_text(b: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "G"), (1 << 20, "M"), (1 << 10, "K")];
    for (n, unit) in UNITS {
        if b >= n {
            return format!("{:.1}{unit}", b as f64 / n as f64);
        }
    }
    format!("{b}B")
}

/// What `path` holds, for the main panel in `width` columns: a
/// directory's entries (hidden ones with `hidden`, as on the left), a file's text with its line numbers (at most
/// `PREVIEW_MAX` of it), or what it is when it is not text.
pub fn preview(path: &Path, width: usize, hidden: bool) -> Vec<String> {
    let name = path.to_string_lossy();
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return vec![paint("1", &name), paint("31", &format!("cannot read: {e}"))],
    };
    if meta.is_dir() {
        let mut out = vec![paint("1;34", &format!("{name}/"))];
        match list(path, hidden) {
            Ok((entries, more)) => {
                let (dirs, files) =
                    (entries.iter().filter(|e| e.dir).count(), entries.iter().filter(|e| !e.dir).count());
                out.push(paint(
                    "2",
                    &format!(
                        "{dirs} directories, {files} files{}",
                        if more > 0 { format!(", {more} more") } else { String::new() }
                    ),
                ));
                out.push(String::new());
                for e in &entries {
                    out.push(if e.dir {
                        paint("1;34", &format!("{}/", e.name))
                    } else {
                        format!("{}  {}", e.name, paint("2", &size_text(e.size)))
                    });
                }
            }
            Err(e) => out.push(paint("31", &format!("cannot read: {e}"))),
        }
        return out;
    }
    let size = meta.len();
    let mut head = vec![format!("{}  {}", paint("1", &name), paint("2", &size_text(size)))];
    let bytes = match read_head(path, PREVIEW_MAX) {
        Ok(b) => b,
        Err(e) => {
            head.push(paint("31", &format!("cannot read: {e}")));
            return head;
        }
    };
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        head.push(paint("2", "binary: not shown"));
        return head;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.split('\n').collect();
    // The last line of a file that ends in a line break is no line.
    let lines = if lines.last() == Some(&"") { &lines[..lines.len() - 1] } else { &lines[..] };
    let cut = size > PREVIEW_MAX;
    head.push(paint(
        "2",
        &format!(
            "{} lines{}",
            lines.len(),
            if cut { format!(", the first {} shown", size_text(PREVIEW_MAX)) } else { String::new() }
        ),
    ));
    head.push(String::new());
    let num_w = lines.len().to_string().len();
    let text_w = width.saturating_sub(num_w + 1);
    head.extend(lines.iter().enumerate().map(|(i, l)| {
        let l = plain(l);
        format!("{} {}", paint("2", &format!("{:>num_w$}", i + 1)), clip(&l, text_w).0)
    }));
    head
}

/// A file's line as it may be drawn: tabs as spaces to the next stop of 4,
/// a carriage return dropped, any other control character (an escape that
/// would restyle the board above all) as the picture of it.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                let n = 4 - col % 4;
                out.push_str(&" ".repeat(n));
                col += n;
            }
            '\r' => {}
            c if (c as u32) < 0x20 => {
                out.push(char::from_u32(0x2400 + c as u32).unwrap_or('?'));
                col += 1;
            }
            '\x7f' => {
                out.push('␡');
                col += 1;
            }
            c => {
                out.push(c);
                col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            }
        }
    }
    out
}

fn read_head(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    std::fs::File::open(path)?.take(max).read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("keepane-files-{}-{}", std::process::id(), rand_tag()));
        std::fs::create_dir_all(d.join("src/inner")).unwrap();
        std::fs::create_dir_all(d.join("Docs")).unwrap();
        std::fs::write(d.join("b.txt"), "two\n").unwrap();
        std::fs::write(d.join("A.md"), "one\n").unwrap();
        std::fs::write(d.join(".hidden"), "").unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
        d
    }

    fn rand_tag() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn names(f: &Files) -> Vec<String> {
        f.entries.iter().map(|e| if e.dir { format!("{}/", e.name) } else { e.name.clone() }).collect()
    }

    #[test]
    fn directories_come_first_and_hidden_ones_only_when_asked() {
        let d = tree();
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        assert_eq!(names(&f), ["Docs/", "src/", "A.md", "b.txt"]);
        f.toggle_hidden();
        assert_eq!(names(&f), ["Docs/", "src/", ".hidden", "A.md", "b.txt"]);
        let _ = std::fs::remove_dir_all(d);
    }

    /// On Windows, what the system marks hidden is hidden too.
    #[cfg(windows)]
    #[test]
    fn a_file_windows_marks_hidden_is_hidden() {
        let d = tree();
        let marked = d.join("marked.txt");
        std::fs::write(&marked, "").unwrap();
        let ok = std::process::Command::new("attrib").arg("+h").arg(&marked).status().is_ok_and(|s| s.success());
        assert!(ok, "attrib +h");
        let (shown, _) = list(&d, false).unwrap();
        assert!(!shown.iter().any(|e| e.name == "marked.txt"));
        let (all, _) = list(&d, true).unwrap();
        assert!(all.iter().any(|e| e.name == "marked.txt"));
        let _ = std::process::Command::new("attrib").arg("-h").arg(&marked).status();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn h_and_l_walk_down_and_back_up_to_where_it_was() {
        let d = tree();
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        f.step(1);
        assert_eq!(f.picked(), Some(d.join("src")));
        assert!(f.enter());
        assert_eq!(names(&f), ["inner/", "main.rs"]);
        f.step(1);
        assert!(!f.enter(), "a file is not gone into");
        assert_eq!(f.picked(), Some(d.join("src").join("main.rs")));
        f.up();
        assert_eq!(f.dir.as_deref(), Some(d.as_path()));
        assert_eq!(f.picked(), Some(d.join("src")), "the cursor on the directory left");
        // The same pane and directory again: the browsing stays.
        f.step(1);
        f.follow("%1", &d.to_string_lossy());
        assert_eq!(f.picked(), Some(d.join("A.md")));
        // A cd there, or another pane: it goes along. `-` goes back by hand.
        f.follow("%1", &d.join("src").to_string_lossy());
        assert_eq!(f.dir.as_deref(), Some(d.join("src").as_path()));
        f.up();
        f.back();
        assert_eq!(f.dir.as_deref(), Some(d.join("src").as_path()));
        // Stepping stays within the list.
        f.step(-99);
        assert_eq!(f.pick, 0);
        f.step(99);
        assert_eq!(f.pick, 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_directory_read_again_keeps_the_cursor_on_its_entry() {
        let d = tree();
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        f.step(3);
        assert_eq!(f.picked(), Some(d.join("b.txt")));
        std::fs::write(d.join("0-new.txt"), "").unwrap();
        f.read();
        assert_eq!(f.picked(), Some(d.join("b.txt")), "{:?}", names(&f));
        std::fs::remove_file(d.join("b.txt")).unwrap();
        f.read();
        assert!(f.pick < f.entries.len(), "still inside the list");
        // A directory that went away says so.
        let _ = std::fs::remove_dir_all(&d);
        f.read();
        assert!(f.error.is_some() && f.entries.is_empty());
        assert!(f.rows(5, 30)[0].1.contains("cannot read"));
    }

    #[test]
    fn a_file_is_previewed_as_text_with_its_lines_or_named_as_binary() {
        let d = tree();
        let text = d.join("t.txt");
        std::fs::write(&text, "a\tb\r\nescape \x1b[31mred\n中文\n").unwrap();
        let p = preview(&text, 40, false);
        let body: Vec<String> = p.iter().map(|l| super::super::strip(l)).collect();
        assert!(body[1].contains("3 lines"), "{body:?}");
        assert_eq!(body[3], "1 a   b");
        assert_eq!(body[4], "2 escape ␛[31mred", "an escape is shown, not obeyed");
        assert_eq!(body[5], "3 中文");
        let bin = d.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        assert!(preview(&bin, 40, false).iter().any(|l| l.contains("binary")));
        let big = d.join("big.txt");
        std::fs::write(&big, "x\n".repeat((PREVIEW_MAX as usize) / 2 + 100)).unwrap();
        let p = preview(&big, 40, false);
        assert!(super::super::strip(&p[1]).contains("the first 256.0K shown"), "{:?}", p[1]);
        assert_eq!(p.len(), 3 + (PREVIEW_MAX as usize) / 2, "only what was read");
        std::fs::write(d.join("src/.secret"), "").unwrap();
        let dirp = preview(&d.join("src"), 40, false);
        assert!(super::super::strip(&dirp[1]).contains("1 directories, 1 files"), "{dirp:?}");
        let all = preview(&d.join("src"), 40, true);
        assert!(super::super::strip(&all[1]).contains("1 directories, 2 files"), "hidden as on the left: {all:?}");
        assert!(preview(&d.join("nope"), 40, false)[1].contains("cannot read"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(
            [size_text(0), size_text(512), size_text(4096), size_text(1_300_000), size_text(5 << 30)],
            ["0B", "512B", "4.0K", "1.2M", "5.0G"]
        );
    }

    #[test]
    fn a_listing_is_cut_at_its_limit_and_counts_the_rest() {
        let d = std::env::temp_dir().join(format!("keepane-files-many-{}-{}", std::process::id(), rand_tag()));
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..LIST_MAX + 7 {
            std::fs::write(d.join(format!("f{i}")), "").unwrap();
        }
        let (entries, more) = list(&d, false).unwrap();
        assert_eq!((entries.len(), more), (LIST_MAX, 7));
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        let rows = f.rows(4, 30);
        assert_eq!(rows.len(), 4);
        assert!(rows[3].1.contains("7 more"));
        let _ = std::fs::remove_dir_all(d);
    }
}
