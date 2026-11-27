//! UnifyTerm's `ut`, when this machine has it: its pictures (`ut view`)
//! and its markdown (`ut md`) in `[0] Files`' preview, in place of
//! "binary: not shown" and the raw text. Found once, when the board starts
//! (`find`); what it gave for a file kept while the file and the panel's
//! width stay as they are, since the preview is drawn at every refresh.

use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long `ut` may take for one file.
const TIMEOUT: Duration = Duration::from_secs(2);
/// The widest picture asked for.
const PICTURE_MOST: usize = 120;

/// What `ut` shows of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Picture,
    Markdown,
}

/// What `ut` shows of a file, by its extension; None for one it does not.
pub fn what(path: &Path) -> Option<What> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Some(What::Picture),
        "md" | "markdown" => Some(What::Markdown),
        _ => None,
    }
}

/// The file, as it was when shown, and the width: what `ut` gave is kept
/// under it.
type Shown = (PathBuf, Option<SystemTime>, u64, usize);

/// What `ut` gave: its lines, or why there are none.
type Given = Result<Vec<String>, String>;

/// `ut`, found, and what it last gave.
pub struct Ut {
    pub program: PathBuf,
    last: RefCell<Option<(Shown, Given)>>,
}

impl Ut {
    pub fn new(program: PathBuf) -> Ut {
        Ut { program, last: RefCell::new(None) }
    }

    /// What `ut` shows of `path` in `width` columns: its lines, or why
    /// there are none; None for a file it does not show.
    pub fn preview(&self, path: &Path, width: usize) -> Option<Given> {
        let what = what(path)?;
        let meta = std::fs::metadata(path).ok()?;
        // Markdown past what the plain preview reads is left to it (its
        // first `PREVIEW_MAX`): `ut md` would give every line of it. A
        // picture's lines are as many as its width makes them.
        if what == What::Markdown && meta.len() > super::files::PREVIEW_MAX {
            return None;
        }
        let shown: Shown = (path.to_path_buf(), meta.modified().ok(), meta.len(), width);
        if let Some((was, out)) = self.last.borrow().as_ref()
            && *was == shown
        {
            return Some(out.clone());
        }
        let w = width.max(10).to_string();
        let file = path.to_string_lossy().to_string();
        let args: Vec<String> = match what {
            What::Picture => {
                let w = width.clamp(10, PICTURE_MOST).to_string();
                vec!["view".into(), "--mode".into(), "half".into(), "--width".into(), w, file]
            }
            What::Markdown => vec!["md".into(), "--color=always".into(), "--width".into(), w, file],
        };
        let out = run(&self.program, &args).map(|text| text.lines().map(keep_sgr).collect::<Vec<String>>());
        *self.last.borrow_mut() = Some((shown, out.clone()));
        Some(out)
    }
}

/// UnifyTerm's `ut` on this machine: on the PATH, in the directory
/// `UNIFYTERM_COMMANDS` names (set in UnifyTerm's tabs), or where it is
/// installed on Windows (`%LOCALAPPDATA%\unifyterm\bin`); one that says it
/// is UnifyTerm's (`ut --version`: "ut 0.2.4 (…, 99 commands)"), not
/// another program of that name.
pub fn find() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ut.exe" } else { "ut" };
    let mut places: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect()).unwrap_or_default();
    if let Some(d) = std::env::var_os("UNIFYTERM_COMMANDS") {
        places.push(PathBuf::from(d).join(exe));
    }
    if let Some(d) = std::env::var_os("LOCALAPPDATA") {
        places.push(PathBuf::from(d).join("unifyterm").join("bin").join(exe));
    }
    find_in(places)
}

/// The first of `places` that is UnifyTerm's `ut`.
fn find_in(places: Vec<PathBuf>) -> Option<PathBuf> {
    places.into_iter().find(|p| p.is_file() && is_unifyterms(p))
}

/// Whether the program says it is UnifyTerm's `ut`.
fn is_unifyterms(program: &Path) -> bool {
    run(program, &["--version".to_string()]).is_ok_and(|v| v.starts_with("ut ") && v.contains("commands"))
}

/// What the program prints, given `TIMEOUT`; why not otherwise.
fn run(program: &Path, args: &[String]) -> Result<String, String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ut: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let until = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| format!("ut: {e}"))? {
            Some(s) => break s,
            None if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("ut took more than {}s", TIMEOUT.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let out = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    if status.success() {
        Ok(out)
    } else {
        let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        let said = err.lines().next().unwrap_or_default().trim().to_string();
        Err(if said.is_empty() { format!("ut ended with {status}") } else { keep_sgr(&said) })
    }
}

/// A line of `ut`'s as the board may draw it: its colours and styles (SGR,
/// `ESC [ … m`) kept; every other escape (links, `ESC ] 8`; cursor moves;
/// titles) and control character taken out, so nothing restyles or moves
/// the board but the colours of the line itself.
pub fn keep_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut end = None;
                    for x in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&x) {
                            end = Some(x);
                            break;
                        }
                        params.push(x);
                    }
                    if end == Some('m') && params.chars().all(|x| x.is_ascii_digit() || x == ';') {
                        out.push_str("\x1b[");
                        out.push_str(&params);
                        out.push('m');
                    }
                }
                // OSC, DCS and the like: to their end (BEL, or ESC \).
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(x) = it.next() {
                        if x == '\x07' {
                            break;
                        }
                        if x == '\x1b' && it.peek() == Some(&'\\') {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colours and styles kept; a link, a title, a cursor move and a bare
    /// control character taken out, their text kept.
    #[test]
    fn only_colours_and_styles_are_kept_of_what_ut_prints() {
        let line = "\x1b[1;4mkeepane\x1b[0m \x1b]8;;https://x.y\x1b\\\x1b[4mCI\x1b[0m\x1b]8;;\x1b\\ \x1b]0;t\x07\x1b[2J\x1b[38;2;1;2;3m▀\x07";
        assert_eq!(keep_sgr(line), "\x1b[1;4mkeepane\x1b[0m \x1b[4mCI\x1b[0m \x1b[38;2;1;2;3m▀");
        assert_eq!(keep_sgr("a\tb\rc"), "a    bc");
    }

    /// A stand-in program at `dir/name` running `body` (a batch file on
    /// Windows, a shell script elsewhere; `%1` / `$1` its first argument).
    fn fake(dir: &Path, name: &str, windows: &str, unix: &str) -> PathBuf {
        let path = if cfg!(windows) { dir.join(format!("{name}.cmd")) } else { dir.join(name) };
        let body = if cfg!(windows) {
            format!("@echo off\r\n{windows}\r\n")
        } else {
            format!("#!/bin/sh\n[ \"$1\" = settle ] && exit 0\n{unix}\n")
        };
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            // Another test's fork may still hold it open for writing (ETXTBSY).
            crate::platform::process::wait_runnable(&path, &["settle"]);
        }
        path
    }

    /// Only UnifyTerm's `ut` is taken: another program of that name (it
    /// says something else to `--version`), or none there, is passed over.
    #[test]
    fn only_unifyterms_ut_is_taken() {
        let dir = std::env::temp_dir().join(format!("keepane-ut-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let other = fake(&dir, "other", "echo ut: no such command", "echo 'ut: no such command'");
        let real = fake(&dir, "real", "echo ut 0.2.4 (019e25b, 99 commands)", "echo 'ut 0.2.4 (019e25b, 99 commands)'");
        let none = dir.join("nothing-here");
        assert_eq!(find_in(vec![none.clone(), other.clone(), real.clone()]), Some(real.clone()));
        assert_eq!(find_in(vec![none, other]), None, "another program of that name: not taken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The preview with `ut`: markdown as it shows it, said so; when it
    /// fails, the plain preview, and why.
    #[test]
    fn the_preview_shows_what_ut_gives_or_says_why_not() {
        let dir = std::env::temp_dir().join(format!("keepane-ut-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ut = Ut::new(fake(
            &dir,
            "ut",
            "if \"%1\"==\"md\" (echo styled) else (echo broken 1>&2 & exit /b 2)",
            "if [ \"$1\" = md ]; then echo styled; else echo broken >&2; exit 2; fi",
        ));
        let md = dir.join("a.md");
        std::fs::write(&md, "# a\n").unwrap();
        let png = dir.join("b.png");
        std::fs::write(&png, b"\x89PNG\x00\x00").unwrap();
        let icons = super::super::files::Icons::Emoji;
        let strip = |v: Vec<String>| v.iter().map(|l| super::super::strip(l)).collect::<Vec<_>>();
        let p = strip(super::super::files::preview(&md, 60, false, icons, Some(&ut), None));
        assert!(p.iter().any(|l| l == "as ut md shows it") && p.iter().any(|l| l == "styled"), "{p:?}");
        let p = strip(super::super::files::preview(&png, 60, false, icons, Some(&ut), None));
        assert!(p.iter().any(|l| l.contains("ut could not show it")), "said why: {p:?}");
        assert!(p.iter().any(|l| l.contains("binary")), "the plain preview after it: {p:?}");
        let p = strip(super::super::files::preview(&md, 60, false, icons, None, None));
        assert!(p.iter().any(|l| l.contains("# a")), "no ut: the text as it is: {p:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `ut` that hangs is stopped at `TIMEOUT`, said so; the preview goes
    /// on without it.
    #[test]
    fn a_ut_that_hangs_is_stopped() {
        let dir = std::env::temp_dir().join(format!("keepane-ut-hang-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ut = Ut::new(fake(&dir, "ut", "ping -n 6 127.0.0.1 >nul", "sleep 5"));
        let md = dir.join("a.md");
        std::fs::write(&md, "# a\n").unwrap();
        let started = Instant::now();
        let err = ut.preview(&md, 60).unwrap().unwrap_err();
        let took = started.elapsed();
        assert!(err.contains("more than 2s"), "{err}");
        assert!(took >= TIMEOUT && took < TIMEOUT + Duration::from_secs(2), "stopped at its time: {took:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UnifyTerm's own `ut`, where this machine has it: found, a picture
    /// drawn in colours, markdown styled, each within its time.
    #[test]
    #[ignore = "needs UnifyTerm's ut on this machine; run with --ignored"]
    fn the_real_ut_shows_a_picture_and_markdown() {
        let program = find().expect("UnifyTerm's ut on this machine");
        let ut = Ut::new(program.clone());
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (file, want) in [("docs/img/chat.png", "\x1b[38;2;"), ("README.md", "\x1b[1")] {
            let started = Instant::now();
            let lines = ut.preview(&root.join(file), 60).unwrap().unwrap();
            eprintln!("{}: {file}: {} lines in {:?}", program.display(), lines.len(), started.elapsed());
            assert!(lines.iter().any(|l| l.contains(want)), "{file}: {:?}", lines.first());
            assert!(lines.iter().all(|l| !l.contains("\x1b]")), "{file}: no link escapes left");
        }
        // A path with a space and Chinese in it reaches ut as it is.
        let dir = std::env::temp_dir().join(format!("keepane-ut-中 文-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("说明 文件.md");
        std::fs::write(&md, "# 标题\n\n正文\n").unwrap();
        let lines = ut.preview(&md, 60).unwrap().unwrap();
        assert!(lines.iter().any(|l| l.contains("标题")), "{lines:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pictures_and_markdown_are_what_ut_shows() {
        assert_eq!(what(Path::new("a/logo.PNG")), Some(What::Picture));
        assert_eq!(what(Path::new("photo.jpeg")), Some(What::Picture));
        assert_eq!(what(Path::new("README.md")), Some(What::Markdown));
        assert_eq!(what(Path::new("main.rs")), None);
        assert_eq!(what(Path::new("Makefile")), None);
        assert_eq!(what(Path::new("clip.mp4")), None, "no video");
    }

    /// A stand-in `ut`: what it prints is shown, its escapes cleaned, kept
    /// while the file is as it was (asked once), asked again once it
    /// changes; one that fails says why.
    #[test]
    fn what_ut_prints_is_shown_kept_and_a_failure_said() {
        let dir = std::env::temp_dir().join(format!("keepane-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let count = dir.join("count");
        let program = fake(
            &dir,
            "ut",
            &format!(
                "echo x>>\"{}\"\r\nif \"%1\"==\"md\" echo \x1b[1mtitle\x1b[0m \x1b]8;;u\x1b\\link\x1b]8;;\x1b\\\r\nif \"%1\"==\"view\" exit /b 3",
                count.display()
            ),
            &format!(
                "echo x >> '{}'\nif [ \"$1\" = md ]; then printf '\\033[1mtitle\\033[0m \\033]8;;u\\033\\\\link\\033]8;;\\033\\\\\\n'; fi\nif [ \"$1\" = view ]; then echo 'no such picture' >&2; exit 3; fi",
                count.display()
            ),
        );
        let md = dir.join("notes.md");
        std::fs::write(&md, "# title\n").unwrap();
        let ut = Ut::new(program.clone());
        let asked = || std::fs::read_to_string(&count).unwrap_or_default().lines().count();
        let lines = ut.preview(&md, 60).unwrap().unwrap();
        assert_eq!(lines, ["\x1b[1mtitle\x1b[0m link"], "colours kept, the link's escapes gone");
        ut.preview(&md, 60).unwrap().unwrap();
        assert_eq!(asked(), 1, "kept while the file and the width are as they were");
        ut.preview(&md, 40).unwrap().unwrap();
        assert_eq!(asked(), 2, "another width: asked again");
        let png = dir.join("logo.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let err = ut.preview(&png, 40).unwrap().unwrap_err();
        assert!(!err.is_empty(), "a failure says why: {err}");
        assert!(ut.preview(&dir.join("main.rs"), 40).is_none(), "not one ut shows");
        let big = dir.join("big.md");
        let line = "some markdown text\n";
        std::fs::write(&big, line.repeat(super::super::files::PREVIEW_MAX as usize / line.len() + 1)).unwrap();
        let before = asked();
        assert!(ut.preview(&big, 40).is_none(), "markdown past PREVIEW_MAX: left to the plain preview");
        assert_eq!(asked(), before, "ut not asked for it");
        let at = dir.join("at.md");
        std::fs::write(&at, "x".repeat(super::super::files::PREVIEW_MAX as usize)).unwrap();
        assert!(ut.preview(&at, 40).is_some(), "markdown of PREVIEW_MAX itself: ut's");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
