//! `keepane view FILE`: a pager for the history log, which `choose-history`
//! opens in a popup. It starts at the end (the newest lines), moves like
//! `less` and vi (j/k, Space/b, g/G, `/` and `?` to search, n/N), jumps
//! from command to command with `[` and `]` (the `── time ──` lines the log
//! puts before each), and quits with q.
//!
//! The view is kept apart from the console, so it can be tested without one.

use crate::keys::{Key, KeyCode};
use unicode_width::UnicodeWidthChar;

/// What a key asks of the program.
#[derive(Debug, PartialEq)]
pub enum Action {
    Redraw,
    Quit,
}

pub struct View {
    pub name: String,
    pub lines: Vec<String>,
    /// First line shown.
    pub top: usize,
    /// Columns scrolled off the left.
    pub left: usize,
    pub cols: u16,
    pub rows: u16,
    /// A search being typed at the bottom (`/` or `?`, and the text).
    typing: Option<(bool, String)>,
    /// The last search and its direction (true: towards the end).
    search: Option<(bool, String)>,
    /// A word for the status line until the next key (not found, ...).
    note: Option<String>,
}

/// Whether a line is the log's line before a command.
fn is_header(line: &str) -> bool {
    line.starts_with("── ")
}

impl View {
    pub fn new(name: &str, text: &str, cols: u16, rows: u16) -> View {
        let lines: Vec<String> = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        let mut v =
            View { name: name.to_string(), lines, top: 0, left: 0, cols, rows, typing: None, search: None, note: None };
        v.top = v.last_top();
        v
    }

    /// Open at line `line` (from 1) instead of the end, a third of the way
    /// down, with `search` the last search (marked, and `n` `N` go on).
    pub fn at(&mut self, line: usize, search: Option<&str>) {
        self.show(line.saturating_sub(1));
        if let Some(s) = search.filter(|s| !s.is_empty()) {
            self.search = Some((true, s.to_string()));
        }
    }

    /// Rows of text: all but the status line.
    fn body(&self) -> usize {
        usize::from(self.rows.saturating_sub(1)).max(1)
    }

    fn last_top(&self) -> usize {
        self.lines.len().saturating_sub(self.body())
    }

    fn scroll(&mut self, by: i64) {
        self.top = (self.top as i64 + by).clamp(0, self.last_top() as i64) as usize;
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.top = self.top.min(self.last_top());
    }

    /// Show line `i` a third of the way down, where there is room.
    fn show(&mut self, i: usize) {
        self.top = i.saturating_sub(self.body() / 3).min(self.last_top());
    }

    /// The next line after (or before) the one at the top a third of the
    /// way down that `hit` accepts.
    fn find(&self, forward: bool, hit: impl Fn(&str) -> bool) -> Option<usize> {
        let from = (self.top + self.body() / 3).min(self.lines.len().saturating_sub(1));
        if forward {
            (from + 1..self.lines.len()).find(|&i| hit(&self.lines[i]))
        } else {
            (0..from).rev().find(|&i| hit(&self.lines[i]))
        }
    }

    fn search_again(&mut self, forward: bool) {
        let Some((_, text)) = self.search.clone() else {
            self.note = Some("no search yet".into());
            return;
        };
        let want = text.to_lowercase();
        match self.find(forward, |l| l.to_lowercase().contains(&want)) {
            Some(i) => self.show(i),
            None => self.note = Some(format!("not found: {text}")),
        }
    }

    pub fn key(&mut self, k: Key) -> Action {
        self.note = None;
        if let Some((forward, text)) = self.typing.as_mut() {
            match k.code {
                KeyCode::Escape => self.typing = None,
                KeyCode::Char('c') if k.ctrl => self.typing = None,
                KeyCode::Enter => {
                    let (forward, text) = (*forward, std::mem::take(text));
                    self.typing = None;
                    if !text.is_empty() {
                        self.search = Some((forward, text));
                    }
                    self.search_again(forward);
                }
                KeyCode::BSpace => {
                    if text.pop().is_none() {
                        self.typing = None;
                    }
                }
                KeyCode::Char(c) if !k.ctrl && !k.alt => text.push(c),
                _ => {}
            }
            return Action::Redraw;
        }
        let page = self.body() as i64;
        match (k.code, k.ctrl) {
            (KeyCode::Char('q'), false) | (KeyCode::Escape, _) | (KeyCode::Char('c'), true) => return Action::Quit,
            (KeyCode::Down | KeyCode::Enter, _)
            | (KeyCode::Char('j' | 'e'), false)
            | (KeyCode::Char('n' | 'e'), true) => self.scroll(1),
            (KeyCode::Up, _) | (KeyCode::Char('k' | 'y'), false) | (KeyCode::Char('p' | 'y'), true) => self.scroll(-1),
            (KeyCode::NPage, _) | (KeyCode::Char(' ' | 'f'), false) | (KeyCode::Char('f'), true) => self.scroll(page),
            (KeyCode::PPage, _) | (KeyCode::Char('b'), false) | (KeyCode::Char('b'), true) => self.scroll(-page),
            (KeyCode::Char('d'), _) => self.scroll(page / 2),
            (KeyCode::Char('u'), _) => self.scroll(-(page / 2)),
            (KeyCode::Home, _) | (KeyCode::Char('g'), false) => self.top = 0,
            (KeyCode::End, _) | (KeyCode::Char('G'), false) => self.top = self.last_top(),
            (KeyCode::Right, _) | (KeyCode::Char('l'), false) => self.left += 8,
            (KeyCode::Left, _) | (KeyCode::Char('h'), false) => self.left = self.left.saturating_sub(8),
            (KeyCode::Char('/'), false) => self.typing = Some((true, String::new())),
            (KeyCode::Char('?'), false) => self.typing = Some((false, String::new())),
            (KeyCode::Char('n'), false) => {
                let forward = self.search.as_ref().is_none_or(|(f, _)| *f);
                self.search_again(forward);
            }
            (KeyCode::Char('N'), false) => {
                let forward = self.search.as_ref().is_none_or(|(f, _)| *f);
                self.search_again(!forward);
            }
            (KeyCode::Char(c @ ('[' | ']')), false) => match self.find(c == ']', is_header) {
                Some(i) => self.show(i),
                None => self.note = Some(if c == ']' { "no later command" } else { "no earlier command" }.into()),
            },
            _ => {}
        }
        Action::Redraw
    }

    /// One line cut to the view: from column `left`, at most `cols` wide.
    fn clip(&self, line: &str) -> String {
        let (mut col, mut out, mut used) = (0usize, String::new(), 0usize);
        for c in line.chars() {
            let w = if c == '\t' { 1 } else { c.width().unwrap_or(0) };
            if col >= self.left {
                if used + w > usize::from(self.cols) {
                    break;
                }
                out.push(if c == '\t' || c.is_control() { ' ' } else { c });
                used += w;
            }
            col += w;
        }
        out
    }

    /// Everything on screen, as the bytes that draw it.
    pub fn frame(&self) -> String {
        let mut s = String::from("\x1b[?25l\x1b[H");
        let want = self.search.as_ref().map(|(_, t)| t.to_lowercase());
        for row in 0..self.body() {
            s.push_str("\x1b[2K");
            if let Some(line) = self.lines.get(self.top + row) {
                let text = self.clip(line);
                let hit = want.as_ref().is_some_and(|w| !w.is_empty() && line.to_lowercase().contains(w.as_str()));
                if is_header(line) {
                    let colour = if line.contains('✗') { "31" } else { "90" };
                    s.push_str(&format!("\x1b[{colour}m{text}\x1b[0m"));
                } else if hit {
                    s.push_str(&format!("\x1b[1;33m{text}\x1b[0m"));
                } else {
                    s.push_str(&text);
                }
            } else {
                s.push_str("\x1b[90m~\x1b[0m");
            }
            s.push_str("\r\n");
        }
        // The quiet bar, as keepane's own hint rows are (white on black, which
        // the theme makes its own), not reversed: no glare under the text.
        s.push_str("\x1b[2K\x1b[0;37;40m");
        let status = match (&self.typing, &self.note) {
            (Some((forward, text)), _) => format!("{}{text}", if *forward { '/' } else { '?' }),
            (None, Some(note)) => format!(" {note}"),
            (None, None) => {
                let n = self.lines.len();
                let last = (self.top + self.body()).min(n);
                let pct = (last * 100).checked_div(n).unwrap_or(100);
                format!(" {}  {last}/{n} {pct}%  q quit  / ? search  n N  [ ] commands", self.name)
            }
        };
        let mut status: String = self.clip_status(&status);
        let pad = usize::from(self.cols).saturating_sub(unicode_width::UnicodeWidthStr::width(status.as_str()));
        status.push_str(&" ".repeat(pad));
        s.push_str(&status);
        s.push_str("\x1b[0m");
        if self.typing.is_some() {
            s.push_str("\x1b[?25h");
        }
        s
    }

    fn clip_status(&self, text: &str) -> String {
        let mut used = 0;
        text.chars()
            .take_while(|c| {
                used += c.width().unwrap_or(0);
                used <= usize::from(self.cols)
            })
            .collect()
    }
}

/// `keepane view [-n LINE] [-s TEXT] FILE`: at line LINE (else the end),
/// TEXT the search.
pub fn run(path: &str, line: Option<usize>, search: Option<&str>) -> anyhow::Result<i32> {
    use crate::console::{Console, InputEvent};
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let mut c = Console::open()?;
    c.enter_raw()?;
    let (cols, rows) = c.size();
    let mut view = View::new(&name, &text, cols, rows);
    if let Some(n) = line {
        view.at(n, search);
    }
    c.write_str(&view.frame());
    let result = (|| -> anyhow::Result<()> {
        loop {
            for ev in c.read_events()? {
                match ev {
                    InputEvent::Key(k) => {
                        if let Some(key) = crate::keys::key_from_record(&k)
                            && view.key(key) == Action::Quit
                        {
                            return Ok(());
                        }
                    }
                    InputEvent::Resize => {
                        let (cols, rows) = c.size();
                        view.resize(cols, rows);
                        c.write_str("\x1b[2J");
                    }
                    // The wheel: three lines a notch.
                    InputEvent::Mouse(m) if m.flags & 0x4 != 0 => {
                        view.scroll(if (m.buttons as i32) < 0 { 3 } else { -3 });
                    }
                    InputEvent::Mouse(_) => continue,
                }
            }
            c.write_str(&view.frame());
        }
    })();
    c.restore();
    result.map(|()| 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            if i % 10 == 0 {
                s.push_str(&format!("── 12:00:{:02} · 1.0s · {} ──\n", i / 10, if i == 20 { "✗ 1" } else { "✓" }));
            }
            s.push_str(&format!("line {i}\n"));
        }
        s
    }

    #[test]
    fn it_opens_at_the_end_and_moves_like_less() {
        let mut v = View::new("x.log", &text(50), 40, 11);
        assert_eq!(v.lines.len(), 55);
        assert_eq!(v.top, 45, "the last ten lines show");
        v.key(Key::ch('g'));
        assert_eq!(v.top, 0);
        v.key(Key::ch(' '));
        assert_eq!(v.top, 10);
        v.key(Key::ch('k'));
        assert_eq!(v.top, 9);
        v.key(Key::ch('G'));
        v.key(Key::ch('j'));
        assert_eq!(v.top, 45, "not past the end");
        v.resize(40, 31);
        assert_eq!(v.top, 25, "a taller view still ends at the end");
        assert_eq!(v.key(Key::ch('q')), Action::Quit);
    }

    /// Opened at a line (a hit of a search over the log): that line a third
    /// of the way down, the search marked and going on from there.
    #[test]
    fn it_opens_at_a_line_with_its_search() {
        let mut v = View::new("x.log", &text(50), 40, 11);
        // The file's line 31 is "line 27" (a header every ten lines).
        v.at(31, Some("line 2"));
        assert_eq!(v.lines[v.top + 10 / 3], "line 27", "a third of the way down");
        assert!(v.frame().contains("\x1b[1;33mline 27"), "the search marked");
        v.key(Key::ch('n'));
        assert_eq!(v.lines[v.top + 10 / 3], "line 28", "n goes on from there");
        // Past the end: the end; no search: none.
        let mut w = View::new("x.log", &text(50), 40, 11);
        w.at(9999, None);
        assert_eq!(w.top, w.last_top());
        assert_eq!(w.key(Key::ch('n')), Action::Redraw);
        assert!(w.frame().contains("no search yet"));
    }

    #[test]
    fn it_searches_and_jumps_between_commands() {
        let mut v = View::new("x.log", &text(50), 40, 11);
        v.key(Key::ch('g'));
        for c in "/LINE 3".chars() {
            v.key(Key::ch(c));
        }
        v.key(Key::plain(KeyCode::Enter));
        // "line 3" first, a third of the way down.
        assert_eq!(v.lines[v.top + 3], "line 3");
        v.key(Key::ch('n'));
        assert_eq!(v.lines[v.top + 3], "line 30");
        v.key(Key::ch('N'));
        assert_eq!(v.lines[v.top + 3], "line 3");
        for c in "/nothing".chars() {
            v.key(Key::ch(c));
        }
        v.key(Key::plain(KeyCode::Enter));
        assert!(v.frame().contains("not found: nothing"));
        // ] and [ go from command line to command line.
        v.key(Key::ch('g'));
        v.key(Key::ch(']'));
        assert_eq!(v.lines[v.top + 3], "── 12:00:01 · 1.0s · ✓ ──");
        v.key(Key::ch(']'));
        assert!(v.lines[v.top + 3].contains("✗ 1"));
        assert!(v.frame().contains("\x1b[31m── 12:00:02"), "a failed command is red");
        v.key(Key::ch('['));
        assert_eq!(v.lines[v.top + 3], "── 12:00:01 · 1.0s · ✓ ──");
    }

    #[test]
    fn lines_are_cut_to_the_view_by_width() {
        let mut v = View::new("x.log", "中文abc\n", 5, 3);
        assert_eq!(v.clip("中文abc"), "中文a");
        v.key(Key::ch('l'));
        assert_eq!(v.clip("0123456789abc"), "89abc");
        let v = View::new("empty.log", "", 20, 3);
        assert!(v.frame().contains("0/0 100%"));
        // Windows line ends and tabs: no stray \r, a tab one blank cell.
        let v = View::new("crlf.log", "a\tb\r\nc\r\n", 20, 3);
        assert_eq!(v.lines, ["a\tb", "c"]);
        assert_eq!(v.clip(&v.lines[0]), "a b");
    }
}
