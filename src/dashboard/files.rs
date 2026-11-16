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

/// What marks each entry (`dashboard-icons`): an emoji, which the
/// system's own emoji font draws, or a Nerd Font's icon, which only a
/// terminal drawn in one has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Icons {
    #[default]
    Emoji,
    Nerd,
}

impl Icons {
    /// `show-options -gv dashboard-icons`'s answer; anything else (an
    /// older server that has no such option) the default.
    pub fn parse(s: &str) -> Icons {
        if s.trim() == "nerd" { Icons::Nerd } else { Icons::Emoji }
    }
}

/// A kind of entry: its emoji, its Nerd Font icon and that icon's colour
/// (an SGR colour, one of the 16 the theme maps). Each emoji is one shown
/// as a picture without a variation selector, so it is two columns
/// wherever it is drawn.
struct Kind {
    emoji: &'static str,
    nerd: char,
    colour: &'static str,
}

const fn kind(emoji: &'static str, nerd: char, colour: &'static str) -> Kind {
    Kind { emoji, nerd, colour }
}

const FOLDER: Kind = kind("📁", '\u{f07b}', "34");
/// A directory whose entries are on the right.
const FOLDER_OPEN: Kind = kind("📂", '\u{f07c}', "34");
/// A file of no kind below: no extension, or one not known.
const FILE: Kind = kind("📄", '\u{f15b}', "90");

/// Kinds by whole name (case aside), before the extension is looked at.
const BY_NAME: &[(&[&str], Kind)] = &[
    (&["readme"], kind("📝", '\u{e73e}', "37")),
    (&["license", "licence", "copying"], kind("📜", '\u{f0219}', "33")),
    (&["makefile", "justfile"], kind("🔨", '\u{e779}', "33")),
    (&["dockerfile", "containerfile"], kind("🐳", '\u{f308}', "34")),
    (&[".gitignore", ".gitattributes", ".gitmodules"], kind("🌱", '\u{e702}', "31")),
    (&["package-lock.json", "yarn.lock", "pnpm-lock.yaml"], kind("🔒", '\u{f023}', "90")),
    (&[".env", ".editorconfig"], kind("🔧", '\u{e615}', "36")),
];

/// Kinds by extension (case aside).
const BY_EXT: &[(&[&str], Kind)] = &[
    (&["py", "pyi", "pyw", "ipynb"], kind("🐍", '\u{e73c}', "33")),
    (&["ts", "tsx", "mts", "cts"], kind("🟦", '\u{e628}', "34")),
    (&["js", "jsx", "mjs", "cjs"], kind("🟨", '\u{e74e}', "33")),
    (&["rs"], kind("🦀", '\u{e7a8}', "31")),
    (&["go"], kind("🐹", '\u{e627}', "36")),
    (&["c", "h"], kind("🔷", '\u{e61e}', "34")),
    (&["cpp", "cc", "cxx", "hpp", "hh"], kind("🔷", '\u{e61d}', "34")),
    (&["sh", "bash", "zsh", "fish", "ps1", "psm1", "bat", "cmd"], kind("🐚", '\u{f489}', "32")),
    (&["md", "mdx", "markdown"], kind("📝", '\u{e73e}', "37")),
    (&["txt", "log"], kind("📄", '\u{f15c}', "90")),
    (&["json", "jsonc", "json5"], kind("🧾", '\u{e60b}', "33")),
    (&["toml", "yaml", "yml", "ini", "cfg", "conf", "env"], kind("🔧", '\u{e615}', "36")),
    (&["html", "htm", "xml", "vue", "svelte"], kind("🌐", '\u{e736}', "31")),
    (&["css", "scss", "sass", "less"], kind("🎨", '\u{e749}', "34")),
    (&["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp"], kind("🌄", '\u{f1c5}', "35")),
    (&["zip", "tar", "gz", "tgz", "xz", "bz2", "7z", "rar", "zst"], kind("📦", '\u{f1c6}', "31")),
    (&["csv", "tsv", "xlsx", "xls"], kind("📊", '\u{f1c3}', "32")),
    (&["db", "sqlite", "sqlite3"], kind("💽", '\u{f1c0}', "36")),
    (&["pdf"], kind("📕", '\u{f1c1}', "31")),
    (&["exe", "dll", "so", "dylib", "wasm", "bin"], kind("⚡", '\u{eae8}', "32")),
    (&["lock"], kind("🔒", '\u{f023}', "90")),
];

/// What `e` is: a folder (open when its entries are on the right), else
/// by its whole name, then by its name without the extension (`README.txt`
/// is a readme), then by the extension.
fn kind_of(e: &Entry, open: bool) -> &'static Kind {
    if e.dir {
        return if open { &FOLDER_OPEN } else { &FOLDER };
    }
    let name = e.name.to_lowercase();
    let named = |n: &str| BY_NAME.iter().find(|(names, _)| names.contains(&n)).map(|(_, k)| k);
    let ext = name.rfind('.').map(|i| (&name[..i], &name[i + 1..]));
    named(&name)
        .or_else(|| ext.and_then(|(stem, _)| named(stem)))
        .or_else(|| ext.and_then(|(_, x)| BY_EXT.iter().find(|(exts, _)| exts.contains(&x)).map(|(_, k)| k)))
        .unwrap_or(&FILE)
}

/// `e`'s mark and the space after it: an emoji (two columns), or a Nerd
/// Font icon in its colour (one).
pub fn icon(icons: Icons, e: &Entry, open: bool) -> String {
    let k = kind_of(e, open);
    match icons {
        Icons::Emoji => format!("{} ", k.emoji),
        Icons::Nerd => format!("{} ", paint(k.colour, &k.nerd.to_string())),
    }
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
    /// What marks each entry (`dashboard-icons`).
    pub icons: Icons,
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
    /// in view; each marked by its kind (`icon`), directories blue with a
    /// `/`. `open`: the entry under the cursor is shown on the right, so a
    /// directory there is an open folder.
    pub fn rows(&self, rows: usize, width: usize, open: bool) -> Vec<(bool, String)> {
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
                let mark = icon(self.icons, e, open && i == self.pick);
                let width = width.saturating_sub(width_of(&mark));
                let line = if e.dir {
                    paint("1;34", &format!("{}/", e.name))
                } else {
                    let size = size_text(e.size);
                    let name_w = width.saturating_sub(size.len() + 1);
                    let name = clip(&e.name, name_w).0;
                    let pad = " ".repeat(width.saturating_sub(width_of(&name) + size.len()));
                    format!("{name}{pad}{}", paint("2", &size))
                };
                (i == self.pick, format!("{mark}{line}"))
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
/// directory's entries (hidden ones with `hidden`, each marked with
/// `icons`, as on the left), a file's text with its line numbers (at most
/// `PREVIEW_MAX` of it), or what it is when it is not text.
pub fn preview(path: &Path, width: usize, hidden: bool, icons: Icons) -> Vec<String> {
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
                    let mark = icon(icons, e, false);
                    out.push(if e.dir {
                        format!("{mark}{}", paint("1;34", &format!("{}/", e.name)))
                    } else {
                        format!("{mark}{}  {}", e.name, paint("2", &size_text(e.size)))
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
    use unicode_width::UnicodeWidthChar;

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
        assert!(f.rows(5, 30, false)[0].1.contains("cannot read"));
    }

    #[test]
    fn a_file_is_previewed_as_text_with_its_lines_or_named_as_binary() {
        let d = tree();
        let text = d.join("t.txt");
        std::fs::write(&text, "a\tb\r\nescape \x1b[31mred\n中文\n").unwrap();
        let p = preview(&text, 40, false, Icons::Emoji);
        let body: Vec<String> = p.iter().map(|l| super::super::strip(l)).collect();
        assert!(body[1].contains("3 lines"), "{body:?}");
        assert_eq!(body[3], "1 a   b");
        assert_eq!(body[4], "2 escape ␛[31mred", "an escape is shown, not obeyed");
        assert_eq!(body[5], "3 中文");
        let bin = d.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        assert!(preview(&bin, 40, false, Icons::Emoji).iter().any(|l| l.contains("binary")));
        let big = d.join("big.txt");
        std::fs::write(&big, "x\n".repeat((PREVIEW_MAX as usize) / 2 + 100)).unwrap();
        let p = preview(&big, 40, false, Icons::Emoji);
        assert!(super::super::strip(&p[1]).contains("the first 256.0K shown"), "{:?}", p[1]);
        assert_eq!(p.len(), 3 + (PREVIEW_MAX as usize) / 2, "only what was read");
        std::fs::write(d.join("src/.secret"), "").unwrap();
        let dirp = preview(&d.join("src"), 40, false, Icons::Emoji);
        assert!(super::super::strip(&dirp[1]).contains("1 directories, 1 files"), "{dirp:?}");
        let all = preview(&d.join("src"), 40, true, Icons::Emoji);
        assert!(super::super::strip(&all[1]).contains("1 directories, 2 files"), "hidden as on the left: {all:?}");
        assert!(preview(&d.join("nope"), 40, false, Icons::Emoji)[1].contains("cannot read"));
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
        let rows = f.rows(4, 30, false);
        assert_eq!(rows.len(), 4);
        assert!(rows[3].1.contains("7 more"));
        let _ = std::fs::remove_dir_all(d);
    }

    fn entry(name: &str, dir: bool) -> Entry {
        Entry { name: name.into(), dir, size: 0 }
    }

    /// A name's kind: by its whole name, then its name without the
    /// extension, then the extension, case aside; a folder by being one.
    #[test]
    fn an_entry_is_marked_by_its_kind() {
        let emoji = |name: &str| icon(Icons::Emoji, &entry(name, false), false);
        let cases = [
            ("main.py", "🐍"),
            ("MAIN.PY", "🐍"),
            ("index.ts", "🟦"),
            ("app.jsx", "🟨"),
            ("lib.rs", "🦀"),
            ("README", "📝"),
            ("readme.txt", "📝"),
            ("LICENSE", "📜"),
            ("Dockerfile", "🐳"),
            (".gitignore", "🌱"),
            (".env", "🔧"),
            (".env.local", "🔧"),
            ("Cargo.toml", "🔧"),
            ("package-lock.json", "🔒"),
            ("package.json", "🧾"),
            ("release.tar.gz", "📦"),
            ("logo.png", "🌄"),
            ("model.onnx", "📄"),
            ("run", "📄"),
            ("trailing.", "📄"),
        ];
        for (name, want) in cases {
            assert_eq!(emoji(name), format!("{want} "), "{name}");
        }
        assert_eq!(icon(Icons::Emoji, &entry("src", true), false), "📁 ");
        assert_eq!(icon(Icons::Emoji, &entry("src", true), true), "📂 ", "its entries on the right");
        assert_eq!(icon(Icons::Emoji, &entry("main.py", false), true), "🐍 ", "a file opens no folder");
        // A Nerd Font's icons, each in its colour.
        assert_eq!(icon(Icons::Nerd, &entry("src", true), false), "\x1b[34m\u{f07b}\x1b[0m ");
        assert_eq!(icon(Icons::Nerd, &entry("src", true), true), "\x1b[34m\u{f07c}\x1b[0m ");
        assert_eq!(icon(Icons::Nerd, &entry("main.py", false), false), "\x1b[33m\u{e73c}\x1b[0m ");
        assert_eq!(icon(Icons::Nerd, &entry("model.onnx", false), false), "\x1b[90m\u{f15b}\x1b[0m ");
        assert_eq!(
            (Icons::parse("nerd\n"), Icons::parse("emoji"), Icons::parse("")),
            (Icons::Nerd, Icons::Emoji, Icons::Emoji)
        );
    }

    /// Odd names are marked without a fault; a panel too narrow for a mark
    /// and a size gets rows all the same (`row_line` cuts them, as it cut
    /// the size alone before), and one wide enough rows no wider than it.
    #[test]
    fn odd_names_and_narrow_panels_stay_in_bounds() {
        for name in ["", ".", "..", "a..b", "x.", ".PY", "中文.md", "🐍.py", "a.b.c.d.rs", "über.RS"] {
            assert!(icon(Icons::Emoji, &entry(name, false), false).ends_with(' '), "{name:?}");
        }
        assert_eq!(icon(Icons::Emoji, &entry("中文.md", false), false), "📝 ");
        assert_eq!(icon(Icons::Emoji, &entry("über.RS", false), false), "🦀 ");
        let d = tree();
        std::fs::write(d.join(format!("{}🐍中文.py", "long-".repeat(12))), "x").unwrap();
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        for icons in [Icons::Emoji, Icons::Nerd] {
            f.icons = icons;
            let mark = if icons == Icons::Emoji { 3 } else { 2 };
            for width in 0..=12 {
                let rows = f.rows(10, width, true);
                assert_eq!(rows.len(), 5, "{icons:?} {width}");
                for (_, line) in &rows {
                    let plain = super::super::strip(line);
                    // A file row fits once there is room for its mark and
                    // its size (`4B` here); a folder's name is cut where
                    // the row is drawn (`row_line`).
                    if !plain.contains('/') && width >= mark + 4 {
                        assert!(width_of(&plain) <= width, "{icons:?} {width} {plain:?}");
                    }
                }
            }
            let wide = f.rows(10, 30, false);
            assert!(wide.iter().all(|(_, l)| width_of(l) <= 30), "{wide:?}");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    /// Every emoji is one character two columns wide (none needs a
    /// variation selector, which terminals count differently), every Nerd
    /// Font icon one column: so a row's width is known wherever it is drawn.
    #[test]
    fn every_mark_has_a_known_width() {
        let all = BY_NAME.iter().chain(BY_EXT).map(|(_, k)| k).chain([&FOLDER, &FOLDER_OPEN, &FILE]);
        for k in all {
            let mut chars = k.emoji.chars();
            let (c, rest) = (chars.next().unwrap(), chars.next());
            assert_eq!((c.width(), rest), (Some(2), None), "{}", k.emoji);
            assert_eq!(k.nerd.width(), Some(1), "{:x}", k.nerd as u32);
        }
    }

    /// The marks take their columns from the name, not from the row: each
    /// row is as wide as before, the size still against the right edge.
    #[test]
    fn a_row_keeps_its_width_with_its_mark() {
        let d = tree();
        let mut f = Files::default();
        f.follow("%1", &d.to_string_lossy());
        for icons in [Icons::Emoji, Icons::Nerd] {
            f.icons = icons;
            for open in [false, true] {
                let rows = f.rows(10, 24, open);
                assert_eq!(rows.len(), 4);
                for (_, line) in &rows {
                    assert!(width_of(line) <= 24, "{icons:?} {line:?}");
                }
                // Files fill the row, their size at its end.
                let plain: Vec<String> = rows.iter().map(|(_, l)| super::super::strip(l)).collect();
                assert_eq!(width_of(&plain[2]), 24, "{plain:?}");
                assert!(plain[2].ends_with("4B"), "{plain:?}");
                let folder = if icons == Icons::Emoji { ["📂", "📁"] } else { ["\u{f07c}", "\u{f07b}"] };
                assert!(plain[0].starts_with(folder[usize::from(!open)]), "the picked folder, {open}: {plain:?}");
                assert!(plain[1].starts_with(folder[1]), "another folder stays shut: {plain:?}");
            }
        }
        // The right side's listing of a directory marks its entries too.
        let p: Vec<String> = preview(&d, 40, false, Icons::Emoji).iter().map(|l| super::super::strip(l)).collect();
        assert!(p.contains(&"📁 Docs/".to_string()) && p.contains(&"📝 A.md  4B".to_string()), "{p:?}");
        let _ = std::fs::remove_dir_all(d);
    }
}
