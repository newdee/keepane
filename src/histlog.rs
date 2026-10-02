//! `log-history`: what panes print, kept on disk after the scrollback has
//! let it go, one plain-text file per pane position per day:
//!
//! ```text
//! <dir>\<session>\<window>.<pane>\2026-09-25.log
//! ```
//!
//! A line is written when it scrolls off the top of its pane's screen, so
//! a program that redraws in place (a progress bar, a prompt being edited)
//! leaves its final text, not every frame; a full-screen program (vim,
//! less) on the alternate screen leaves nothing. What is still on screen
//! when the pane goes is written then. A command the shell reported gets a
//! line of its times before it (`── 14:03:22 · 3.2s · ✓ ──`).
//!
//! Writing is done by a thread of its own (another one writes the event
//! log), so a slow disk never holds up the server. A file stops growing at `DAY_CAP` bytes (with a note saying
//! so); files older than `log-history-days` are removed.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Most bytes one pane position writes in one day.
pub const DAY_CAP: u64 = 20 * 1024 * 1024;

/// Where the files go when `log-history-dir` is not set: `KEEPANE_HISTORY_DIR`,
/// else beside the saved sessions when `KEEPANE_SESSIONS_DIR` moves those
/// (the tests do, and so stay out of the real directory), else
/// `%LOCALAPPDATA%\keepane\history`.
pub fn default_dir() -> PathBuf {
    if let Some(d) = crate::legacy::var_os("KEEPANE_HISTORY_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = crate::legacy::var_os("KEEPANE_SESSIONS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("history");
    }
    crate::logger::log_dir().join("history")
}

/// Where the event log goes (docs/design/mailbox.md §10): `KEEPANE_EVENTS_DIR`,
/// else beside moved saved sessions or history (the tests move those, and
/// so stay out of the real directory), else `%LOCALAPPDATA%\keepane\events`.
pub fn events_dir() -> PathBuf {
    if let Some(d) = crate::legacy::var_os("KEEPANE_EVENTS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = crate::legacy::var_os("KEEPANE_SESSIONS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("events");
    }
    if let Some(d) = crate::legacy::var_os("KEEPANE_HISTORY_DIR").filter(|d| !d.is_empty()) {
        let mut d = d;
        d.push("-events");
        return PathBuf::from(d);
    }
    crate::logger::log_dir().join("events")
}

/// A name made safe to be a directory: characters Windows refuses become
/// `_`, as do trailing dots and spaces; nothing at all becomes `_`.
pub fn safe_name(name: &str) -> String {
    let mut s: String =
        name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() { "_".into() } else { s }
}

/// The file a pane position writes on `day`.
pub fn file_for(dir: &Path, session: &str, window: usize, pane: usize, day: chrono::NaiveDate) -> PathBuf {
    dir.join(safe_name(session)).join(format!("{window}.{pane}")).join(format!("{}.log", day.format("%Y-%m-%d")))
}

enum Msg {
    /// Text for a file, and the most that file may hold.
    Append(PathBuf, String, u64),
    /// A directory, how many days to keep, and whether its day files are
    /// right in it (the event log) rather than two levels down (history).
    Prune(PathBuf, u32, bool),
    Flush(Sender<()>),
    /// A writer dying of a bug, for the test of its replacement.
    #[cfg(test)]
    Crash,
    /// A writer held up (a slow disk, a big prune), for the test of the
    /// lanes.
    #[cfg(test)]
    Stall(Duration),
}

/// The two writers, each a thread of its own: what panes print, and the
/// event log. Apart, so that reading the event log back (`list-events`,
/// `show-task`) never waits behind a flood of pane output or a pruning of
/// the history directory, which can take seconds on a busy machine.
#[derive(Clone, Copy, Debug)]
enum Lane {
    History,
    Events,
}

fn start(lane: Lane) -> Sender<Msg> {
    let (tx, rx) = channel();
    let name = match lane {
        Lane::History => "history-log",
        Lane::Events => "event-log",
    };
    std::thread::Builder::new().name(name.into()).spawn(move || run(rx)).expect("log writer thread");
    tx
}

fn writer(lane: Lane) -> &'static Mutex<Sender<Msg>> {
    static HISTORY: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();
    static EVENTS: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();
    let cell = match lane {
        Lane::History => &HISTORY,
        Lane::Events => &EVENTS,
    };
    cell.get_or_init(|| Mutex::new(start(lane)))
}

fn send(lane: Lane, m: Msg) {
    let Ok(mut tx) = writer(lane).lock() else { return };
    // A writer that died of a bug must not take the log with it for the
    // rest of the server's life: another one takes over.
    if let Err(back) = tx.send(m) {
        log::error!("{lane:?} log writer stopped; starting another");
        *tx = start(lane);
        let _ = tx.send(back.0);
    }
}

/// Add `text` to the end of the history file `path`, from its writer.
pub fn append(path: PathBuf, text: String) {
    if !text.is_empty() {
        send(Lane::History, Msg::Append(path, text, DAY_CAP));
    }
}

/// Add an event log line to `path`, a file that may hold at most `cap` bytes.
pub fn append_event(path: PathBuf, text: String, cap: u64) {
    if !text.is_empty() {
        send(Lane::Events, Msg::Append(path, text, cap));
    }
}

/// Remove the history files under `dir` older than `days` days (0 keeps
/// them all), and the directories that leaves empty.
pub fn prune(dir: PathBuf, days: u32) {
    if days > 0 {
        send(Lane::History, Msg::Prune(dir, days, false));
    }
}

/// `prune` for an event log directory, whose day files are right in it.
pub fn prune_events(dir: PathBuf, days: u32) {
    if days > 0 {
        send(Lane::Events, Msg::Prune(dir, days, true));
    }
}

/// Wait (at most `wait` in all) until everything asked for so far, of
/// both logs, is on disk.
pub fn flush(wait: Duration) {
    let deadline = std::time::Instant::now() + wait;
    let asked: Vec<Receiver<()>> = [Lane::History, Lane::Events]
        .into_iter()
        .map(|lane| {
            let (tx, rx) = channel();
            send(lane, Msg::Flush(tx));
            rx
        })
        .collect();
    for rx in asked {
        let _ = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()));
    }
}

/// `flush` for the event log alone: what reading it back waits for.
pub fn flush_events(wait: Duration) {
    let (tx, rx) = channel();
    send(Lane::Events, Msg::Flush(tx));
    let _ = rx.recv_timeout(wait);
}

struct Open {
    file: std::io::BufWriter<std::fs::File>,
    size: u64,
}

/// How long the writer waits for more before what it holds goes to disk:
/// output comes a few lines at a time, and a write per few lines costs more
/// than everything else the log does.
const SETTLE: Duration = Duration::from_millis(500);

fn flush_all(open: &mut HashMap<PathBuf, Open>) {
    open.retain(|path, o| match o.file.flush() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("history log {}: {e}", path.display());
            false
        }
    });
}

fn run(rx: Receiver<Msg>) {
    let mut open: HashMap<PathBuf, Open> = HashMap::new();
    // A directory that cannot be written is said once, not every second.
    let mut failed: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    loop {
        let m = match rx.recv_timeout(SETTLE) {
            Ok(m) => m,
            // Quiet: to disk, and the files closed, so one deleted or moved
            // meanwhile is made again rather than written into unseen.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                flush_all(&mut open);
                open.clear();
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match m {
            Msg::Append(path, text, cap) => {
                // A few dozen panes at most write at once; a day's worth of
                // stale handles is not kept.
                if open.len() > 64 && !open.contains_key(&path) {
                    flush_all(&mut open);
                    open.clear();
                }
                if !open.contains_key(&path) {
                    let opened = path
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(&path));
                    match opened {
                        Ok(file) => {
                            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
                            let file = std::io::BufWriter::with_capacity(64 * 1024, file);
                            open.insert(path.clone(), Open { file, size });
                        }
                        Err(e) => {
                            if failed.insert(path.clone()) {
                                log::warn!("history log {}: {e}", path.display());
                            }
                            continue;
                        }
                    }
                }
                let o = open.get_mut(&path).unwrap();
                if o.size >= cap {
                    continue;
                }
                // What would pass the cap is replaced by a note saying so,
                // after which the file counts as full. A JSON Lines file
                // gets it as a line of JSON.
                let (bytes, size) = if o.size + text.len() as u64 > cap {
                    let note = format!("this file reached {} MB; the rest of the day is not kept", cap >> 20);
                    let line = if path.extension().is_some_and(|e| e == "jsonl") {
                        format!("{{\"note\":{}}}\n", serde_json::to_string(&note).expect("a string serializes"))
                    } else {
                        format!("[keepane: {note}]\n")
                    };
                    (line, cap)
                } else {
                    let n = o.size + text.len() as u64;
                    (text, n)
                };
                match o.file.write_all(bytes.as_bytes()) {
                    Ok(()) => o.size = size,
                    Err(e) => {
                        log::warn!("history log {}: {e}", path.display());
                        open.remove(&path);
                    }
                }
            }
            Msg::Prune(dir, days, flat) => {
                // Handles into files about to go would keep them.
                flush_all(&mut open);
                open.clear();
                // More days than the calendar goes back: nothing is that old.
                if let Some(before) =
                    chrono::Local::now().date_naive().checked_sub_days(chrono::Days::new(u64::from(days)))
                {
                    if flat { prune_days(&dir, before) } else { prune_dir(&dir, before) }
                }
            }
            Msg::Flush(done) => {
                flush_all(&mut open);
                let _ = done.send(());
            }
            #[cfg(test)]
            Msg::Crash => panic!("history-log test: told to crash"),
            #[cfg(test)]
            Msg::Stall(d) => std::thread::sleep(d),
        }
    }
}

/// Remove day files dated before `before` under `dir` (two levels down),
/// then the directories left empty. Nothing that is not a day file is
/// touched.
fn prune_dir(dir: &Path, before: chrono::NaiveDate) {
    for session in read_dirs(dir) {
        for key in read_dirs(&session) {
            prune_days(&key, before);
            let _ = std::fs::remove_dir(&key); // only if empty
        }
        let _ = std::fs::remove_dir(&session);
    }
}

/// Remove the day files right in `dir` dated before `before`.
fn prune_days(dir: &Path, before: chrono::NaiveDate) {
    for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if day_of(&f.path()).is_some_and(|d| d < before) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// The day a `YYYY-MM-DD.log` (history) or `YYYY-MM-DD.jsonl` (event log)
/// file is for.
pub fn day_of(path: &Path) -> Option<chrono::NaiveDate> {
    if !matches!(path.extension()?.to_str()?, "log" | "jsonl") {
        return None;
    }
    chrono::NaiveDate::parse_from_str(path.file_stem()?.to_str()?, "%Y-%m-%d").ok()
}

/// One pane position that has history: its session directory's name, its
/// `window.pane`, and its day files, newest first, with their sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub session: String,
    pub key: String,
    pub days: Vec<(chrono::NaiveDate, PathBuf, u64)>,
}

/// Everything under `dir`, sessions in name order, positions in window
/// then pane order.
pub fn scan(dir: &Path) -> Vec<Kept> {
    let mut out = Vec::new();
    for session in read_dirs(dir) {
        let mut keys: Vec<(Vec<u64>, Kept)> = Vec::new();
        for key in read_dirs(&session) {
            let mut days: Vec<(chrono::NaiveDate, PathBuf, u64)> = std::fs::read_dir(&key)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|f| f.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|f| {
                    let p = f.path();
                    let d = day_of(&p)?;
                    Some((d, p, f.metadata().map(|m| m.len()).unwrap_or(0)))
                })
                .collect();
            if days.is_empty() {
                continue;
            }
            days.sort_by_key(|d| std::cmp::Reverse(d.0));
            let name = key.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let order = name.split('.').map(|n| n.parse().unwrap_or(u64::MAX)).collect();
            keys.push((
                order,
                Kept {
                    session: session.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    key: name,
                    days,
                },
            ));
        }
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        out.extend(keys.into_iter().map(|(_, k)| k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer is one for the process: these tests take turns with it.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn turn() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A writer that died is replaced: what is sent after it went is
    /// written (what was on its way when it died is lost with it).
    #[test]
    fn a_dead_writer_is_replaced() {
        let _turn = turn();
        let dir = std::env::temp_dir().join(format!("keepane-histlog-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        send(Lane::History, Msg::Crash);
        // Until another writer is there: a flush sent after the crash can
        // only be answered by one (other tests write here too, so the crash
        // may wait behind them; a flush lost with the dead writer is sent
        // again).
        let end = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let (tx, rx) = channel();
            send(Lane::History, Msg::Flush(tx));
            if rx.recv_timeout(Duration::from_secs(5)).is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < end, "no writer took over");
        }
        let f = file_for(&dir, "s", 0, 0, chrono::Local::now().date_naive());
        append(f.clone(), "after the crash\n".into());
        flush(Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "after the crash\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The event log does not wait behind the history log: with the history
    /// writer held up, an event line is still on disk at once.
    #[test]
    fn the_event_log_is_not_held_up_by_the_history_log() {
        let _turn = turn();
        let dir = std::env::temp_dir().join(format!("keepane-histlog-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        send(Lane::History, Msg::Stall(Duration::from_secs(3)));
        let history = file_for(&dir, "s", 0, 0, chrono::Local::now().date_naive());
        append(history.clone(), "behind the stall\n".into());
        let events = dir.join("events").join("2026-09-27.jsonl");
        let started = std::time::Instant::now();
        append_event(events.clone(), "{\"ev\":\"x\"}\n".into(), DAY_CAP);
        flush_events(Duration::from_secs(2));
        assert_eq!(std::fs::read_to_string(&events).unwrap_or_default(), "{\"ev\":\"x\"}\n");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert!(!history.exists(), "the history writer is still held up");
        flush(Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "behind the stall\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_become_paths_and_days_are_read_back() {
        assert_eq!(safe_name("a:b/c\\d?"), "a_b_c_d_");
        assert_eq!(safe_name("dots. "), "dots");
        assert_eq!(safe_name(""), "_");
        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let p = file_for(Path::new("C:\\h"), "work", 1, 0, day);
        assert_eq!(p, Path::new("C:\\h").join("work").join("1.0").join("2026-09-25.log"));
        assert_eq!(day_of(&p), Some(day));
        assert_eq!(day_of(Path::new("notes.txt")), None);
        assert_eq!(day_of(Path::new("2026-13-01.log")), None);
    }

    /// Odd things in the directory and odd settings: nothing panics, and
    /// the writer keeps writing.
    #[test]
    fn junk_and_extreme_settings_are_survived() {
        let _turn = turn();
        let dir = std::env::temp_dir().join(format!("keepane-histlog-junk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let today = chrono::Local::now().date_naive();
        // A file where a session directory would be, a directory where a
        // day file would be, a file that is not a day, an empty day.
        std::fs::create_dir_all(dir.join("s").join("0.0").join("2026-01-01.log")).unwrap();
        std::fs::write(dir.join("not-a-session"), "x").unwrap();
        std::fs::write(dir.join("s").join("0.0").join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.join("s").join("1.0")).unwrap();
        std::fs::write(dir.join("s").join("1.0").join("2026-02-30.log"), "").unwrap();
        std::fs::write(dir.join("s").join("1.0").join(format!("{}.log", today.format("%Y-%m-%d"))), "").unwrap();
        let kept = scan(&dir);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!((kept[0].key.as_str(), kept[0].days.len(), kept[0].days[0].2), ("1.0", 1, 0));
        // Keep more days than the calendar has: nothing to remove, and the
        // writer is still there afterwards.
        prune(dir.clone(), u32::MAX);
        prune(dir.clone(), 1);
        let f = file_for(&dir, "s", 1, 0, today);
        append(f.clone(), "still writing\n".into());
        flush(Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "still writing\n");
        assert!(dir.join("not-a-session").exists() && dir.join("s").join("0.0").join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_scanned_capped_and_pruned() {
        let _turn = turn();
        let dir = std::env::temp_dir().join(format!("keepane-histlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let today = chrono::Local::now().date_naive();
        let old = today - chrono::Days::new(40);
        append(file_for(&dir, "s", 0, 0, today), "one\n".into());
        append(file_for(&dir, "s", 0, 0, today), "two\n".into());
        append(file_for(&dir, "s", 10, 0, today), "ten\n".into());
        append(file_for(&dir, "s", 2, 1, old), "old\n".into());
        flush(Duration::from_secs(5));
        let kept = scan(&dir);
        let keys: Vec<&str> = kept.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(keys, ["0.0", "2.1", "10.0"], "window order, not text order");
        assert_eq!(std::fs::read_to_string(&kept[0].days[0].1).unwrap(), "one\ntwo\n");
        // Past the cap: one note, then nothing.
        let big = file_for(&dir, "s", 3, 0, today);
        append(big.clone(), "x".repeat(DAY_CAP as usize - 10));
        append(big.clone(), "this line does not fit\n".into());
        append(big.clone(), "nor this one\n".into());
        flush(Duration::from_secs(5));
        let text = std::fs::read_to_string(&big).unwrap();
        assert!(text.ends_with("the rest of the day is not kept]\n"), "{}", &text[text.len() - 80..]);
        assert!(!text.contains("nor this"));
        // Thirty days kept: the forty-day-old file and its directory go.
        prune(dir.clone(), 30);
        flush(Duration::from_secs(5));
        let keys: Vec<String> = scan(&dir).into_iter().map(|k| k.key).collect();
        assert_eq!(keys, ["0.0", "3.0", "10.0"]);
        assert!(!dir.join("s").join("2.1").exists());
        // A file deleted while keepane runs is made again after a quiet
        // moment, not written into unseen.
        let f = file_for(&dir, "s", 0, 0, today);
        std::thread::sleep(SETTLE * 3);
        std::fs::remove_file(&f).unwrap();
        append(f.clone(), "after\n".into());
        flush(Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "after\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
