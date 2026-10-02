//! `hints` (prefix F): the things on screen worth copying or opening, found
//! in the text of each row: web addresses, paths (with the line and column a
//! compiler gives, `src/main.rs:12:5`, `Foo.cs(12,5)`) and git hashes. Each
//! gets a label of one or two letters; typing it copies the thing, typing
//! it in capitals opens it.
//!
//! Only what fits in a row is found: a path the terminal wrapped onto the
//! next row is not.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Url,
    Path { line: Option<u32>, col: Option<u32> },
    Hash,
}

/// One thing found: its row, its first column and width in cells, the text
/// to copy (for a path, without the line and column) and what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub text: String,
    pub kind: Kind,
}

/// What ends a word: blanks, quotes and brackets (a path in parentheses or
/// quotes is found without them).
fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '|')
}

/// Punctuation that ends a sentence or a compiler's line, not the thing.
fn trailing(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?')
}

/// The things in `line`, row `row`, left to right. `exists` says whether a
/// path names a file or directory (relative ones from the pane's directory):
/// a word with no slash and no line number is taken only when it does.
pub fn find(row: u16, line: &str, exists: &dyn Fn(&str) -> bool) -> Vec<Found> {
    let chars: Vec<char> = line.chars().collect();
    // The cell each character starts in (a wide one takes two).
    let mut cell = Vec::with_capacity(chars.len() + 1);
    let mut w = 0u16;
    for c in &chars {
        cell.push(w);
        w = w.saturating_add(unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0) as u16);
    }
    cell.push(w);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if separator(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !separator(chars[i]) {
            i += 1;
        }
        let mut end = i;
        while end > start && trailing(chars[end - 1]) {
            end -= 1;
        }
        let word: String = chars[start..end].iter().collect();
        // `Foo.cs(12,5)` (MSBuild, the C# and TypeScript compilers): the
        // line and column in parentheses right after the path.
        let paren = (end == i).then(|| paren_position(&chars[i..])).flatten();
        if let Some(t) = classify(&word, paren.map(|(l, c, _)| (l, c)), exists) {
            let (text, kind) = t;
            // An address found inside a word (`href=https://…`) starts
            // where the address does.
            let skip = word.find(&text).map_or(0, |b| word[..b].chars().count());
            let last = match (&kind, paren) {
                (Kind::Path { .. }, Some((_, _, len))) => end + len,
                _ => end,
            };
            let first = start + skip;
            out.push(Found { row, col: cell[first], width: cell[last] - cell[first], text, kind });
            if last > i {
                i = last;
            }
        }
    }
    out
}

/// `(12,5)` or `(12)` at the start of `rest`: line, column and how many
/// characters it takes.
fn paren_position(rest: &[char]) -> Option<(u32, Option<u32>, usize)> {
    if rest.first() != Some(&'(') {
        return None;
    }
    let close = rest.iter().position(|&c| c == ')')?;
    let inner: String = rest[1..close].iter().collect();
    let mut parts = inner.split(',');
    let line = parts.next()?.trim().parse().ok()?;
    let col = match parts.next() {
        Some(c) => Some(c.trim().parse().ok()?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((line, col, close + 1))
}

/// What `word` is, if anything: the text to copy and its kind.
fn classify(word: &str, paren: Option<(u32, Option<u32>)>, exists: &dyn Fn(&str) -> bool) -> Option<(String, Kind)> {
    for scheme in ["https://", "http://"] {
        if let Some(at) = word.find(scheme) {
            let url = &word[at..];
            return (url.len() > scheme.len()).then(|| (url.to_string(), Kind::Url));
        }
    }
    if word.contains("://") {
        return None;
    }
    if let Some((path, line, col)) = split_position(word) {
        let (line, col) = match paren {
            Some((l, c)) if line.is_none() => (Some(l), c),
            _ => (line, col),
        };
        if is_path(path, line.is_some(), exists) {
            return Some((path.to_string(), Kind::Path { line, col }));
        }
    }
    let hex = word.len() >= 7
        && word.len() <= 40
        && word.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && word.bytes().any(|b| b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_alphabetic());
    hex.then(|| (word.to_string(), Kind::Hash))
}

/// `path:12:5` into the path, line and column (a drive's colon, `C:\`, is
/// part of the path).
fn split_position(word: &str) -> Option<(&str, Option<u32>, Option<u32>)> {
    let mut path = word;
    let mut numbers = Vec::new();
    while numbers.len() < 2 {
        let Some(at) = path.rfind(':') else { break };
        let tail = &path[at + 1..];
        if at <= 1 || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        numbers.push(tail.parse().ok()?);
        path = &path[..at];
    }
    numbers.reverse();
    Some((path, numbers.first().copied(), numbers.get(1).copied()))
}

/// Whether `p` reads as a path: one that says so by its shape (a root, a
/// home, a drive, `./`), one with a slash and a file's extension or a line
/// number, or one that exists.
fn is_path(p: &str, has_line: bool, exists: &dyn Fn(&str) -> bool) -> bool {
    if p.is_empty() || !p.chars().any(|c| c.is_alphabetic()) || p.contains(':') && !drive(p) {
        return false;
    }
    let sep = p.contains('/') || p.contains('\\');
    let rooted = p.starts_with('/')
        || p.starts_with("./")
        || p.starts_with("../")
        || p.starts_with("~/")
        || p.starts_with(".\\")
        || p.starts_with("..\\")
        || p.starts_with("\\\\")
        || drive(p);
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let ext = name.rsplit_once('.').is_some_and(|(stem, e)| {
        !stem.is_empty()
            && (1..=10).contains(&e.len())
            && e.chars().all(|c| c.is_ascii_alphanumeric())
            && e.chars().any(|c| c.is_ascii_alphabetic())
    });
    if sep && (rooted || ext || has_line) {
        return true;
    }
    if !sep && ext && has_line {
        return true;
    }
    (sep || ext) && exists(p)
}

/// `C:\…` or `C:/…`.
fn drive(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Labels for `n` things, the easiest keys first: one letter each while
/// they fit, else two letters each, so no label starts another.
pub fn labels(n: usize) -> Vec<String> {
    const KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";
    let keys: Vec<char> = KEYS.chars().collect();
    if n <= keys.len() {
        return keys.iter().take(n).map(|c| c.to_string()).collect();
    }
    keys.iter().flat_map(|a| keys.iter().map(move |b| format!("{a}{b}"))).take(n).collect()
}

/// How to open a path at a line.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// VS Code: `code -g file:line:col`.
    Code { code: PathBuf, arg: String },
    /// A terminal editor in a new window: `editor +line file`.
    Editor { argv: Vec<String> },
    /// Whatever the desktop opens it with (no line).
    Desktop,
}

/// How to open `file` at `line`/`col`: VS Code when it is on the PATH, else
/// `$VISUAL` / `$EDITOR` in a new window, else the desktop.
pub fn plan(file: &str, line: Option<u32>, col: Option<u32>, code: Option<PathBuf>, editor: Option<String>) -> Plan {
    let (line, col) = (line.unwrap_or(1), col.unwrap_or(1));
    if let Some(code) = code {
        return Plan::Code { code, arg: format!("{file}:{line}:{col}") };
    }
    if let Some(editor) = editor.filter(|e| !e.trim().is_empty()) {
        // `EDITOR="code --wait"` and the like: the program and its words.
        let mut argv: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
        argv.push(format!("+{line}"));
        argv.push(file.to_string());
        return Plan::Editor { argv };
    }
    Plan::Desktop
}

/// A program on the PATH (with Windows's extensions for scripts, so
/// `code` finds `code.cmd`).
pub fn on_path(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".cmd", ".exe", ".bat"] } else { &[""] };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| exts.iter().map(move |e| dir.join(format!("{name}{e}"))))
        .find(|p| p.is_file())
}

/// `file` from the pane's directory `dir`, `~` for the home directory.
pub fn resolve(file: &str, dir: Option<&str>) -> PathBuf {
    if let Some(rest) = file.strip_prefix("~/").or_else(|| file.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    let p = Path::new(file);
    // `has_root`, not `is_relative`: on Windows `/abs/a.rs` has a root but
    // no drive, so it counts as relative, yet it is not the pane's.
    match dir {
        Some(d) if !p.has_root() && p.is_relative() => Path::new(d).join(p),
        _ => p.to_path_buf(),
    }
}

/// The `hint-open` command for one path: `{file}`, `{line}` and `{col}`
/// replaced (line and column 1 when the screen gave none).
pub fn fill(template: &str, file: &str, line: Option<u32>, col: Option<u32>) -> String {
    template
        .replace("{file}", file)
        .replace("{line}", &line.unwrap_or(1).to_string())
        .replace("{col}", &col.unwrap_or(1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    fn texts(line: &str) -> Vec<(String, Kind)> {
        find(0, line, &none).into_iter().map(|f| (f.text, f.kind)).collect()
    }

    fn path(p: &str, line: Option<u32>, col: Option<u32>) -> (String, Kind) {
        (p.to_string(), Kind::Path { line, col })
    }

    #[test]
    fn compilers_paths_come_with_their_line_and_column() {
        assert_eq!(texts("  --> src/server/mod.rs:6454:13"), vec![path("src/server/mod.rs", Some(6454), Some(13))]);
        assert_eq!(texts("main.c:10:5: error: expected ';'"), vec![path("main.c", Some(10), Some(5))]);
        assert_eq!(texts("src\\App.cs(12,5): error CS1002"), vec![path("src\\App.cs", Some(12), Some(5))]);
        assert_eq!(texts("app.ts(7): warning"), vec![path("app.ts", Some(7), None)]);
        assert_eq!(
            texts("C:\\src\\wmux\\src\\lib.rs:3 and /home/me/a.py:9"),
            vec![path("C:\\src\\wmux\\src\\lib.rs", Some(3), None), path("/home/me/a.py", Some(9), None)]
        );
        assert_eq!(texts("at ./tests/e2e.rs:364:13,"), vec![path("./tests/e2e.rs", Some(364), Some(13))]);
    }

    #[test]
    fn addresses_and_hashes() {
        assert_eq!(
            texts("see https://github.com/newdee/keepane/pull/12. or (http://x.io/a)"),
            vec![
                ("https://github.com/newdee/keepane/pull/12".to_string(), Kind::Url),
                ("http://x.io/a".to_string(), Kind::Url)
            ]
        );
        assert_eq!(texts("href=https://a.b/c"), vec![("https://a.b/c".to_string(), Kind::Url)]);
        assert_eq!(texts("af9af7e fix: numbers"), vec![("af9af7e".to_string(), Kind::Hash)]);
        // Hex that is all letters or all digits, or too short, is a word.
        assert!(texts("deadbeef 1234567 abc12 cafe").is_empty());
    }

    #[test]
    fn prose_is_not_a_path() {
        assert!(texts("and/or 1/2 e.g. v0.24.1 done. ok: yes").is_empty());
        // A bare name is a path only when it exists, or carries a line.
        assert!(texts("README.md Cargo.toml").is_empty());
        let here = |p: &str| p == "Cargo.toml";
        let found: Vec<String> = find(0, "README.md Cargo.toml", &here).into_iter().map(|f| f.text).collect();
        assert_eq!(found, vec!["Cargo.toml"]);
        assert_eq!(texts("lib.rs:12"), vec![path("lib.rs", Some(12), None)]);
        // A directory with a slash exists or says it is one.
        assert!(texts("src/server").is_empty());
        assert_eq!(find(0, "src/server", &|p| p == "src/server").len(), 1);
        assert_eq!(texts("~/notes ./run"), vec![path("~/notes", None, None), path("./run", None, None)]);
    }

    #[test]
    fn columns_count_cells_not_characters() {
        let f = find(3, "错误 src/a.rs:1 x", &none);
        assert_eq!((f[0].row, f[0].col, f[0].width), (3, 5, 10));
        let f = find(0, "  App.cs(1,2): x", &none);
        assert_eq!((f[0].col, f[0].width, f[0].text.as_str()), (2, 11, "App.cs"));
    }

    #[test]
    fn labels_are_one_letter_until_they_run_out() {
        assert!(labels(0).is_empty());
        assert_eq!(labels(3), vec!["a", "s", "d"]);
        assert_eq!(labels(26).len(), 26);
        let two = labels(27);
        assert!(two.iter().all(|l| l.len() == 2) && two[0] == "aa" && two[26] == "sa");
        // No label starts another: every one is the same length.
        let all = labels(676);
        assert_eq!(all.iter().collect::<std::collections::HashSet<_>>().len(), 676);
        assert_eq!(labels(1000).len(), 676);
    }

    #[test]
    fn opening_prefers_code_then_the_editor_then_the_desktop() {
        let code = PathBuf::from("/usr/bin/code");
        assert_eq!(
            plan("/a/b.rs", Some(3), None, Some(code.clone()), Some("vim".into())),
            Plan::Code { code, arg: "/a/b.rs:3:1".into() }
        );
        assert_eq!(
            plan("/a/b.rs", Some(3), Some(7), None, Some("nvim -p".into())),
            Plan::Editor { argv: vec!["nvim".into(), "-p".into(), "+3".into(), "/a/b.rs".into()] }
        );
        assert_eq!(plan("/a/b.rs", None, None, None, Some("  ".into())), Plan::Desktop);
        assert_eq!(fill("run-shell \"x {file}:{line}:{col}\"", "a b.rs", Some(4), None), "run-shell \"x a b.rs:4:1\"");
    }

    /// The README's `hint-open` lines, with a Windows path holding a blank:
    /// single quotes keep it one word, backslashes and all.
    #[test]
    fn the_readmes_hint_open_examples_parse() {
        use crate::command::{Cmd, parse_line};
        let file = "C:\\My Code\\src\\a.rs";
        let cmd = parse_line(&fill("new-window hx '{file}:{line}:{col}'", file, Some(12), Some(5))).unwrap().unwrap();
        match cmd {
            Cmd::NewWindow { argv, .. } => assert_eq!(argv, ["hx", "C:\\My Code\\src\\a.rs:12:5"]),
            other => panic!("{other:?}"),
        }
        let cmd = parse_line(&fill("run-shell 'idea --line {line} {file}'", "/home/me/a.rs", Some(3), None))
            .unwrap()
            .unwrap();
        match cmd {
            Cmd::RunShell { command, .. } => assert_eq!(command, "idea --line 3 /home/me/a.rs"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn relative_paths_are_the_panes() {
        assert_eq!(resolve("src/a.rs", Some("/w")), Path::new("/w").join("src/a.rs"));
        assert_eq!(resolve("/abs/a.rs", Some("/w")), PathBuf::from("/abs/a.rs"));
        assert_eq!(resolve("a.rs", None), PathBuf::from("a.rs"));
    }
}
