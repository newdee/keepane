//! The chat page's `[0]`: the picked channel's group files (docs/design/chat.md
//! §13), in `[0] Files`' place. The one under the cursor shows in `[4]` as
//! `[0] Files` shows a file (UnifyTerm's `ut` for pictures and markdown
//! when this machine has it); one of another machine's channel is fetched
//! by its link first, into a folder all boards share under the temporary
//! one (`keepane-board-files`).

use super::{Action, Board, Panel, Rect, boxed, files, fit, owned, paint, row_line, window_start};
use crate::keys::KeyCode;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The largest group file fetched from another machine to be shown.
const FETCH_MOST: u64 = 50 * 1024 * 1024;
/// How long one fetch may take, in seconds.
const FETCH_SECS: &str = "60";
/// A fetch that failed is tried again when shown this long after.
const RETRY_AFTER: Duration = Duration::from_secs(10);
/// How many fetches run at once; a file shown while that many are on
/// their way is fetched when shown again after one ends.
const AT_ONCE: usize = 2;
/// A `.part-` this old is no fetch's any more (each ends within
/// `FETCH_SECS`): one a board left when it closed mid-fetch.
const STALE_PART: Duration = Duration::from_secs(120);

/// Group files of other machines' channels fetched by their links: where
/// the copy is, or why there is none; `None` while one is on its way.
#[derive(Clone, Default)]
pub struct Fetches(Arc<Mutex<HashMap<String, (Fetched, Instant)>>>);

/// One fetch: its copy, or why there is none; None while there is
/// neither yet (on its way, or waiting its turn: `AT_ONCE`).
pub type Fetched = Option<Result<PathBuf, String>>;

impl Fetches {
    /// What came of fetching `url` (saved as `name`): its copy, or why
    /// not; None while it is on its way, or waits for one of the
    /// `AT_ONCE` on their way to end. It starts the first time asked, and
    /// again once `RETRY_AFTER` passed since it failed; a copy already
    /// there (fetched by a board before) is taken as it is.
    pub(super) fn get_or_start(&self, url: &str, name: &str) -> Fetched {
        let Ok(mut map) = self.0.lock() else { return Some(Err("the board's fetches broke".into())) };
        match map.get(url) {
            Some((Some(Err(_)), at)) if at.elapsed() >= RETRY_AFTER => {}
            Some((done, _)) => return done.clone(),
            None => {}
        }
        let path = cache_path(url, name);
        if path.is_file() {
            map.insert(url.to_string(), (Some(Ok(path.clone())), Instant::now()));
            return Some(Ok(path));
        }
        if map.values().filter(|(f, _)| f.is_none()).count() >= AT_ONCE {
            return None;
        }
        map.insert(url.to_string(), (None, Instant::now()));
        let (url, map) = (url.to_string(), self.0.clone());
        std::thread::spawn(move || {
            let got = fetch(&url, &path);
            if let Ok(mut m) = map.lock() {
                m.insert(url, (Some(got), Instant::now()));
            }
        });
        None
    }

    /// What came of `url` as it stands, `ago` (tests: put without fetching).
    #[cfg(test)]
    pub(super) fn put(&self, url: &str, got: Fetched, ago: Duration) {
        self.0.lock().unwrap().insert(url.to_string(), (got, Instant::now() - ago));
    }

    /// Whether `url` was started or put (tests).
    #[cfg(test)]
    pub(super) fn knows(&self, url: &str) -> bool {
        self.0.lock().unwrap().contains_key(url)
    }
}

/// Where the copy of `url` (named `name`) goes: a folder of the link's own
/// under the temporary one, so two files of one name stay apart.
pub(super) fn cache_path(url: &str, name: &str) -> PathBuf {
    // FNV-1a: a name for the link's folder, the same each time.
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    std::env::temp_dir().join("keepane-board-files").join(format!("{hash:016x}")).join(crate::histlog::safe_name(name))
}

/// The `.part-` files under `root` (one level of link folders down) older
/// than `STALE_PART`: what boards closed mid-fetch left (their curl ran
/// on without them and was never put in place).
fn sweep_parts(root: &Path) {
    let dirs = std::fs::read_dir(root).into_iter().flatten().flatten();
    for f in dirs.flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten()) {
        let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if f.file_name().to_string_lossy().starts_with(".part-") && old.is_some_and(|o| o >= STALE_PART) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}
/// `url` saved at `path` with `curl` (as keepane asks the web elsewhere),
/// at most `FETCH_MOST` bytes and `FETCH_SECS` seconds. It is written
/// beside it under this process's own name first and then put in place,
/// so `path` is only ever a whole copy, whichever board fetched it.
fn fetch(url: &str, path: &Path) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or_else(|| format!("{}: no folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Some(root) = dir.parent() {
        sweep_parts(root);
    }
    let part = dir.join(format!(".part-{}", std::process::id()));
    let out = std::process::Command::new("curl")
        // A link from another machine: the web only (no `file:` of this one's).
        .args(["-fsSL", "--proto", "=http,https", "--proto-redir", "=http,https", "-m", FETCH_SECS])
        .args(["--max-filesize", &FETCH_MOST.to_string(), "-o"])
        .arg(&part)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        let why = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or_default().trim().to_string();
        return Err(if why.is_empty() { format!("curl ended with {}", out.status) } else { why });
    }
    // Another board put it in place first: its copy is the same file.
    if let Err(e) = std::fs::rename(&part, path) {
        let _ = std::fs::remove_file(&part);
        if !path.is_file() {
            return Err(format!("{}: {e}", path.display()));
        }
    }
    Ok(path.to_path_buf())
}
impl Board {
    /// The chat page's `[0]`: a group file a line, the newest first: its name, its
    /// size, who sent it; one deleted since, or out of reach, faint.
    pub(super) fn group_files_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::GroupFiles;
        let shown = self.chat.files_shown();
        let n = shown.len();
        let title = if n == 0 { "[0] Group files".to_string() } else { format!("[0] Group files {n}") };
        let read = self.chat.picked().is_some_and(|c| self.chat.files_for.as_ref() == Some(&c.id));
        let note = match (self.chat.picked(), n) {
            (None, _) => Some("no channel"),
            (Some(_), _) if !read => Some("reading..."),
            (Some(_), 0) => Some("none yet (chat-say --file sends one)"),
            _ => None,
        };
        if let Some(note) = note {
            return boxed(&title, "", vec![paint("2", note)], r, focused);
        }
        let (rows, w) = (r.inner_h(), r.inner_w());
        let start = window_start(self.chat.file_pick, n, rows);
        let lines = shown
            .iter()
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(i, g)| {
                let from = if g.from.is_empty() { String::new() } else { format!("  {}", g.from) };
                let line = format!("{}  {}{}", g.name, paint("2", &files::size_text(g.size)), paint("2", &from));
                let line = if g.missing.is_some() { paint("2", &super::strip(&line)) } else { line };
                row_line(&fit(&line, w.saturating_sub(1)), i == self.chat.file_pick, focused, w)
            })
            .collect();
        boxed(&title, "", lines, r, focused)
    }

    /// `[4]` while `[0]` has the group files: the picked one, from its top
    /// (scrolled by `scroll`).
    pub(super) fn group_file_box(&self, r: Rect) -> Vec<String> {
        let (title, line, all) = self.group_file_shown(r.inner_w());
        let rows = r.inner_h();
        let most = all.len().saturating_sub(rows);
        self.scroll_most.set(most);
        let start = self.scroll.min(most);
        boxed(&title, &line, all.into_iter().skip(start).take(rows).collect(), r, self.focus == Panel::Main)
    }

    /// `[4]` while `[0]` has the group files: its title, the line it came
    /// with (on the right: `#9 2026-11-27`), and the picked one as `[0]
    /// Files` shows a file; another machine's, once fetched by its link.
    fn group_file_shown(&self, w: usize) -> (String, String, Vec<String>) {
        let Some(g) = self.chat.picked_file() else {
            return ("[4] Group files".into(), String::new(), vec![paint("2", "no group file")]);
        };
        let title = format!("[4] {}", g.name);
        let line = format!("#{} {}", g.seq, g.day).trim_end().to_string();
        let preview = |p: &Path| {
            files::preview(p, w, false, self.files.icons, self.files.ut.as_ref(), Some(&self.files.pictures))
        };
        let lines = match (&g.missing, &g.path, &g.url) {
            (Some(why), _, _) => vec![paint("2", why)],
            (None, Some(p), _) => preview(Path::new(p)),
            (None, None, Some(url)) => match self.group_fetches.get_or_start(url, &g.name) {
                Some(Ok(p)) => {
                    let mut v = vec![paint("2", &fit(&format!("from {url}"), w))];
                    v.extend(preview(&p));
                    v
                }
                Some(Err(e)) => vec![paint("33", &format!("could not fetch it: {e}")), paint("2", &fit(url, w))],
                None => vec![paint("2", "fetching it from its machine..."), paint("2", &fit(url, w))],
            },
            (None, None, None) => vec![paint("2", "nowhere to open it")],
        };
        (title, line, lines)
    }

    /// The chat page's `[0]`: j/k pick (shown in `[4]`), Enter into `[4]` to
    /// scroll it, `y` its path or link copied, `d` deleted (asked first).
    pub(super) fn group_file_key(&mut self, code: KeyCode) -> Action {
        let n = self.chat.files_shown().len();
        let was = self.chat.file_pick;
        let last = n.saturating_sub(1);
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.chat.file_pick = (self.chat.file_pick + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.chat.file_pick = self.chat.file_pick.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.chat.file_pick = 0,
            KeyCode::End | KeyCode::Char('G') => self.chat.file_pick = last,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if n > 0 => self.focus_on(Panel::Main),
            KeyCode::Char('y') => {
                let place = self.chat.picked_file().and_then(|g| g.place().map(str::to_string));
                self.note = Some(match place {
                    Some(p) => match crate::clipboard::set_text(&p) {
                        Ok(()) => format!("copied {p}"),
                        Err(e) => format!("could not copy: {e:#}"),
                    },
                    None => "nothing to copy".into(),
                });
            }
            KeyCode::Char('d') => {
                let (Some(c), Some(g)) = (self.chat.picked(), self.chat.picked_file()) else {
                    self.note = Some("no group file here".into());
                    return Action::Redraw;
                };
                let question = format!("delete group file {}? Its copy goes, and its link", g.name);
                let argv = owned(&["chat-group-file-rm", &c.id, &g.file]);
                return self.act(question, argv);
            }
            _ => {}
        }
        if self.chat.file_pick != was {
            self.scroll = 0;
        }
        Action::Redraw
    }
}
