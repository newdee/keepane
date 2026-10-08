//! The daily look for a newer keepane (`update-check`, on by default): once
//! a day the server asks GitHub for the latest release, on a thread of its
//! own, with the `curl` every Windows 10, macOS and Linux has. A newer one
//! shows on the status line (`#{keepane_update}`) and in `show-messages`.
//! When it last asked, and what it heard, are kept in the data directory,
//! so a restarted server does not ask again the same day.

use super::{Event, Server};
use std::time::{Duration, SystemTime};

/// How often to ask.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// A failed ask (offline) is tried again this much later, not a day.
const RETRY: Duration = Duration::from_secs(60 * 60);
/// Not in a server's first moments: a short-lived one never asks.
const FIRST_AFTER: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Check {
    /// A newer release's version, when the last answer had one.
    pub newer: Option<String>,
    /// When to ask next; None: as soon as allowed.
    due: Option<SystemTime>,
    asking: bool,
    /// Where the answer is kept; None: the data directory's `update-check`.
    cache: Option<std::path::PathBuf>,
}

impl Check {
    fn cache(&self) -> std::path::PathBuf {
        self.cache.clone().unwrap_or_else(|| crate::logger::log_dir().join("update-check"))
    }
}

impl Server {
    /// At start: what an earlier server heard today.
    pub(super) fn newer_start(&mut self) {
        let Some((at, latest)) = std::fs::read_to_string(self.newer.cache()).ok().and_then(|t| parse_cache(&t)) else {
            return;
        };
        self.newer.due = Some(at + EVERY);
        self.newer.newer = latest.filter(|v| is_newer(v, env!("CARGO_PKG_VERSION")));
    }

    /// Every tick: ask when it is time (and allowed).
    pub(super) fn newer_tick(&mut self) {
        let off = !self.opts.update_check || crate::legacy::var_os("KEEPANE_NO_UPDATE_CHECK").is_some();
        let now = SystemTime::now();
        if off || self.newer.asking || self.started.elapsed() < FIRST_AFTER || self.newer.due.is_some_and(|d| now < d) {
            return;
        }
        self.newer.asking = true;
        super::ask_on_thread(&self.events, latest_release, Event::NewerChecked);
    }

    /// The answer: `latest` is the latest release's version, None when
    /// GitHub could not be asked.
    pub(super) fn newer_checked(&mut self, latest: Option<String>) {
        self.newer.asking = false;
        let now = SystemTime::now();
        let Some(latest) = latest else {
            self.newer.due = Some(now + RETRY);
            return;
        };
        self.newer.due = Some(now + EVERY);
        let _ = std::fs::write(self.newer.cache(), cache_text(now, &latest));
        let newer = is_newer(&latest, env!("CARGO_PKG_VERSION")).then_some(latest);
        if newer.is_some() && newer != self.newer.newer {
            let v = newer.as_deref().unwrap_or_default();
            self.note_message(&format!(
                "keepane {v} is out (this is {}): `keepane update` says how to get it",
                env!("CARGO_PKG_VERSION")
            ));
        }
        self.newer.newer = newer;
        for c in self.clients.values_mut() {
            c.last_grid = None;
        }
    }
}

/// `<unix seconds> <version>`: when it was asked, and the answer.
fn cache_text(at: SystemTime, latest: &str) -> String {
    let secs = at.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs} {latest}\n")
}

fn parse_cache(text: &str) -> Option<(SystemTime, Option<String>)> {
    let mut words = text.split_whitespace();
    let secs: u64 = words.next()?.parse().ok()?;
    let latest = words.next().map(str::to_string);
    Some((SystemTime::UNIX_EPOCH + Duration::from_secs(secs), latest))
}

/// The latest release's version, asked of GitHub, or None.
fn latest_release() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-m",
            "10",
            "-H",
            "User-Agent: keepane-update-check",
            "-H",
            "Accept: application/vnd.github+json",
        ])
        .arg("https://api.github.com/repos/newdee/keepane/releases/latest")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let tag = json["tag_name"].as_str()?.trim_start_matches('v');
    version_parts(tag).map(|_| tag.to_string())
}

fn version_parts(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// Whether `a` is a later version than `b` (`0.10.0` after `0.9.9`).
fn is_newer(a: &str, b: &str) -> bool {
    match (version_parts(a), version_parts(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.19.0", "0.18.0"));
        assert!(is_newer("0.10.0", "0.9.9"), "not as text");
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.18.0", "0.18.0"));
        assert!(!is_newer("0.17.1", "0.18.0"));
        assert!(!is_newer("0.19.0-rc1", "0.18.0"), "not a release version: not offered");
        assert!(!is_newer("", "0.18.0"));
    }

    /// A newer release is said once, shows on the status line, and is kept;
    /// the same answer again says nothing more; no answer (offline) asks
    /// again in an hour and keeps what was known; a new server reads it.
    #[test]
    fn an_answer_is_said_once_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("keepane-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("update-check");
        let server = || {
            let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
            let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
            let mut s = Server::new(pane_tx, "t".into(), events);
            s.newer.cache = Some(cache.clone());
            s
        };
        let said = |s: &Server| s.messages.iter().filter(|m| m.contains("keepane 99.0.0 is out")).count();
        let mut s = server();
        s.newer_checked(Some("99.0.0".into()));
        assert_eq!(s.newer.newer.as_deref(), Some("99.0.0"));
        assert_eq!(said(&s), 1);
        s.newer_checked(Some("99.0.0".into()));
        assert_eq!(said(&s), 1, "once");
        let before = s.newer.due;
        s.newer_checked(None);
        assert_eq!(s.newer.newer.as_deref(), Some("99.0.0"), "kept through a failed ask");
        assert!(s.newer.due < before, "a failed ask is tried again sooner than a day");
        // A new server knows it from the file, without asking.
        let mut again = server();
        again.newer_start();
        assert_eq!(again.newer.newer.as_deref(), Some("99.0.0"));
        assert!(again.newer.due.is_some_and(|d| d > SystemTime::now()), "not due again today");
        // An older (or this) version is nothing to say.
        let mut s = server();
        s.newer_checked(Some("0.0.1".into()));
        assert_eq!(s.newer.newer, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `update-check off` asks nothing, however due; nor does a server in
    /// its first moments. (Only the paths that ask nothing: a test must not
    /// reach GitHub.)
    #[test]
    fn switched_off_or_just_started_it_asks_nothing() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        s.newer.cache = Some(std::env::temp_dir().join(format!("keepane-newer-off-{}", std::process::id())));
        s.newer_tick();
        assert!(!s.newer.asking, "not in the first moments");
        let Some(earlier) = std::time::Instant::now().checked_sub(FIRST_AFTER * 2) else { return };
        s.started = earlier;
        s.opts.update_check = false;
        s.newer_tick();
        assert!(!s.newer.asking, "switched off");
    }

    #[test]
    fn the_cache_says_when_and_what() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let text = cache_text(at, "0.19.0");
        assert_eq!(text, "1790000000 0.19.0\n");
        assert_eq!(parse_cache(&text), Some((at, Some("0.19.0".into()))));
        assert_eq!(parse_cache("garbage"), None, "a spoilt file is as none");
        assert_eq!(parse_cache(""), None);
    }
}
