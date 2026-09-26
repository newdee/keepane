//! keepane was called wmux up to 0.13.1. What an upgrade from then needs, so
//! it loses nothing:
//!
//! - settings under the old names, read when the new ones are absent:
//!   `WMUX_*` environment variables, `~/.wmux.conf`, `~/.wmux/plugins`,
//!   `<name>.wmux` plugin files;
//! - the data directory (saved sessions, the history log, the phone key,
//!   the logs): `%LOCALAPPDATA%\wmux` moves to `%LOCALAPPDATA%\keepane`;
//! - a wmux server still running with sessions: `keepane migrate` asks it
//!   to save them and go, and restores them in keepane (see `client`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The old name.
pub const OLD: &str = "wmux";

/// An environment variable a user may still have set under the old name:
/// `KEEPANE_X` when set, else `WMUX_X`.
pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| {
        let rest = name.strip_prefix("KEEPANE_")?;
        std::env::var_os(format!("WMUX_{rest}"))
    })
}

/// As `var_os`, as a string.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|v| v.into_string().ok())
}

/// The pipe an old wmux server of `socket` listens on. wmux ran on Windows
/// only: elsewhere there is none (an address nothing answers on), never
/// keepane's own.
#[cfg(windows)]
pub fn pipe_name(socket: &str) -> String {
    crate::ipc::pipe_name(socket).replacen(r"\\.\pipe\keepane-", r"\\.\pipe\wmux-", 1)
}

#[cfg(not(windows))]
pub fn pipe_name(_socket: &str) -> String {
    String::new()
}

/// The old data directory beside the new one.
pub fn old_data_dir(new: &Path) -> Option<PathBuf> {
    Some(new.parent()?.join(OLD))
}

/// Move what `old` holds into `new`, file by file: a file `new` already has
/// is left where it was (the newer keepane one wins), and so is one that
/// cannot be moved (in use). Directories left empty go. Returns how many
/// files moved and how many stayed.
pub fn move_tree(old: &Path, new: &Path) -> (usize, usize) {
    let (mut moved, mut stayed) = (0, 0);
    let Ok(entries) = std::fs::read_dir(old) else { return (0, 0) };
    let _ = std::fs::create_dir_all(new);
    for e in entries.flatten() {
        let (from, to) = (e.path(), new.join(e.file_name()));
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let (m, s) = move_tree(&from, &to);
            moved += m;
            stayed += s;
        } else if to.exists() || std::fs::rename(&from, &to).is_err() {
            stayed += 1;
        } else {
            moved += 1;
        }
    }
    let _ = std::fs::remove_dir(old); // only when nothing stayed
    (moved, stayed)
}

/// Whether this process uses the real data directory: no test or user
/// override of where keepane keeps things. Moving the real one is only
/// done then.
pub fn uses_real_dirs() -> bool {
    ["KEEPANE_LOG_DIR", "KEEPANE_SESSIONS_DIR", "KEEPANE_HISTORY_DIR", "KEEPANE_CONFIG"]
        .iter()
        .all(|v| var_os(v).is_none_or(|x| x.is_empty()))
}

/// At start: the old data directory moves to the new place, once, when
/// the new one does not exist yet and no old server is running (an old
/// server still writes there; `keepane migrate` handles that case).
pub fn move_data_dir_at_start() {
    if !uses_real_dirs() {
        return;
    }
    let new = crate::logger::log_dir();
    let Some(old) = old_data_dir(&new) else { return };
    if new.exists() || !old.is_dir() || crate::client::server_running(&pipe_name("default")) {
        return;
    }
    let (moved, stayed) = move_tree(&old, &new);
    if moved > 0 {
        eprintln!(
            "keepane: moved {moved} files from {} (wmux, keepane's old name) to {}",
            old.display(),
            new.display()
        );
    }
    if stayed > 0 {
        eprintln!("keepane: {stayed} files stayed in {}", old.display());
    }
}

/// Whether a registry command line is one an old wmux wrote: its own
/// wmux.exe, run headless (the logon command and the notification link's).
pub fn is_our_old_command(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    c.starts_with("conhost.exe --headless \"") && c.contains("\\wmux.exe\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_names_answer_when_new_ones_are_silent() {
        // Names no one sets, so the test does not depend on the machine.
        unsafe {
            std::env::remove_var("KEEPANE_LEGACY_PROBE");
            std::env::set_var("WMUX_LEGACY_PROBE", "old");
        }
        assert_eq!(var("KEEPANE_LEGACY_PROBE").as_deref(), Some("old"));
        unsafe { std::env::set_var("KEEPANE_LEGACY_PROBE", "new") };
        assert_eq!(var("KEEPANE_LEGACY_PROBE").as_deref(), Some("new"), "the new name wins");
        unsafe {
            std::env::remove_var("KEEPANE_LEGACY_PROBE");
            std::env::remove_var("WMUX_LEGACY_PROBE");
        }
        assert_eq!(var("KEEPANE_LEGACY_PROBE"), None);
        assert_eq!(var("NOT_PREFIXED_AT_ALL_PROBE"), None);
        let p = pipe_name("work");
        if cfg!(windows) {
            assert!(p.starts_with(r"\\.\pipe\wmux-") && p.ends_with("-work"), "{p}");
        } else {
            assert!(p.is_empty() && !crate::client::server_running(&p), "no wmux here: {p}");
        }
        assert!(is_our_old_command(
            r#"conhost.exe --headless "C:\Program Files\wmux\wmux.exe" -L default __server --restore"#
        ));
        assert!(is_our_old_command(r#"conhost.exe --headless "C:\x\wmux.exe" "%1""#));
        assert!(!is_our_old_command(r#""C:\Program Files\Other\wmux.exe" "%1""#), "someone else's wmux");
        assert!(!is_our_old_command(r#"conhost.exe --headless "C:\x\keepane.exe" "%1""#));
    }

    #[test]
    fn the_old_directory_moves_without_overwriting() {
        let root = std::env::temp_dir().join(format!("keepane-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (old, new) = (root.join("wmux"), root.join("keepane"));
        std::fs::create_dir_all(old.join("sessions")).unwrap();
        std::fs::create_dir_all(old.join("history").join("dev").join("0.0")).unwrap();
        std::fs::write(old.join("sessions").join("dev.json"), "old dev").unwrap();
        std::fs::write(old.join("sessions").join("ops.json"), "old ops").unwrap();
        std::fs::write(old.join("history").join("dev").join("0.0").join("2026-09-25.log"), "a day").unwrap();
        std::fs::write(old.join("web.key"), "key").unwrap();
        // keepane already saved "ops" itself: that one is kept.
        std::fs::create_dir_all(new.join("sessions")).unwrap();
        std::fs::write(new.join("sessions").join("ops.json"), "new ops").unwrap();
        assert_eq!(old_data_dir(&new).as_deref(), Some(old.as_path()));
        let (moved, stayed) = move_tree(&old, &new);
        assert_eq!((moved, stayed), (3, 1));
        let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(new.join("sessions").join("dev.json")), "old dev");
        assert_eq!(read(new.join("sessions").join("ops.json")), "new ops", "not overwritten");
        assert_eq!(read(new.join("history").join("dev").join("0.0").join("2026-09-25.log")), "a day");
        assert_eq!(read(new.join("web.key")), "key");
        assert!(old.join("sessions").join("ops.json").exists(), "what stayed is still there");
        assert!(!old.join("history").exists(), "emptied directories go");
        let _ = std::fs::remove_dir_all(&root);
    }
}
