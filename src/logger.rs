//! Minimal file logger for the (console-less) server process.
//!
//! The server runs for weeks; its log must not grow for as long. Past
//! `MAX_BYTES` the file becomes `<name>.log.1` (replacing the one before)
//! and a new one is started, so a log takes at most about twice that.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Size at which the log is rotated: 5 MB, or `KEEPANE_LOG_MAX` bytes.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

struct State {
    file: File,
    path: PathBuf,
    written: u64,
    max: u64,
}

struct FileLogger {
    state: Mutex<State>,
    level: log::LevelFilter,
}

impl State {
    /// Move the full log aside and start a new one. Any failure leaves the
    /// current file in use: a log that cannot rotate still logs.
    fn rotate(&mut self) {
        let old = self.path.with_extension("log.1");
        // std opens files shareable for deletion, so an open log can be
        // renamed; the handle follows the renamed file until reopened.
        if std::fs::rename(&self.path, &old).is_err() {
            return;
        }
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            self.file = f;
            self.written = 0;
        }
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        if let Ok(mut s) = self.state.lock() {
            let line = format!(
                "{} {:<5} [{}] {}\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                r.level(),
                r.target(),
                r.args()
            );
            if s.written + line.len() as u64 > s.max {
                s.rotate();
            }
            if s.file.write_all(line.as_bytes()).is_ok() {
                s.written += line.len() as u64;
            }
        }
    }
    fn flush(&self) {}
}

/// Whether this process uses the real data directory: no test or user
/// override of where keepane keeps things. What touches the real
/// registrations (taking them over after an update, ssh's known hosts) is
/// only done then.
pub fn uses_real_dirs() -> bool {
    ["KEEPANE_LOG_DIR", "KEEPANE_SESSIONS_DIR", "KEEPANE_HISTORY_DIR", "KEEPANE_CONFIG"]
        .iter()
        .all(|v| std::env::var_os(v).is_none_or(|x| x.is_empty()))
}

pub fn log_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("KEEPANE_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("keepane")
}

pub fn init(name: &str) {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.log"));
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else { return };
    let level = match std::env::var("KEEPANE_LOG").ok().as_deref() {
        Some("trace") => log::LevelFilter::Trace,
        Some("debug") => log::LevelFilter::Debug,
        Some("warn") => log::LevelFilter::Warn,
        Some("error") => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    };
    let max =
        std::env::var("KEEPANE_LOG_MAX").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(MAX_BYTES);
    let written = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut state = State { file, path, written, max };
    // Already over the limit (from before rotation existed): start fresh.
    if state.written > state.max {
        state.rotate();
    }
    let logger = Box::leak(Box::new(FileLogger { state: Mutex::new(state), level }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_log_moves_aside_and_a_new_one_starts() {
        let dir = std::env::temp_dir().join(format!("keepane-logrot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.log");
        let file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        let logger = FileLogger {
            state: Mutex::new(State { file, path: path.clone(), written: 0, max: 1000 }),
            level: log::LevelFilter::Info,
        };
        use log::Log;
        for i in 0..100 {
            logger.log(
                &log::Record::builder()
                    .args(format_args!("line {i:04} {}", "x".repeat(20)))
                    .level(log::Level::Info)
                    .target("t")
                    .build(),
            );
        }
        let now = std::fs::metadata(&path).unwrap().len();
        let old = std::fs::metadata(dir.join("t.log.1")).unwrap().len();
        assert!(now <= 1000 && old <= 1000, "both under the limit: {now} {old}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("line 0099"), "the newest line is in the current file");
        assert!(!std::fs::read_to_string(dir.join("t.log.1")).unwrap().contains("line 0000"), "the oldest is gone");
        assert!(!dir.join("t.log.2").exists(), "one old file, not a pile");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
