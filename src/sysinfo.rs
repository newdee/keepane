//! What the status line can show about the machine and the pane's program
//! without running anything: CPU and memory in use, the battery, the time
//! since boot, the git branch of a directory, and the program a pane is
//! running right now. All of it is a system call or a small file read, so
//! the status line can ask every second. The machine and the processes are
//! the platform's (`platform::sysinfo`); the rest is here.

use std::path::{Path, PathBuf};

pub use crate::platform::sysinfo::{System, hostname, program_of, short_path, system};

/// The address this machine reaches the network from (IPv4), empty when it
/// has none. The system is asked which address a UDP socket "connected" to
/// a public address would use; connecting a UDP socket sends nothing. Kept
/// for ten seconds.
pub fn local_ip() -> String {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, ip)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(10)
    {
        return ip.clone();
    }
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_unspecified())
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    *cache = Some((Instant::now(), ip.clone()));
    ip
}

/// Bytes as people write them: "812M", "6.2G".
pub fn human_bytes(b: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    let b = b as f64;
    if b >= G { format!("{:.1}G", b / G) } else { format!("{}M", (b / M).round() as u64) }
}

/// The branch of the git repository `dir` is in ("main"), a short commit
/// when the head is detached ("a1b2c3d"), or empty outside a repository.
/// Reads `.git/HEAD` (following a `.git` file's `gitdir:`, as worktrees
/// have); never runs git. A relative `dir` is no pane's directory (it would
/// be looked up from wherever this process runs): empty.
pub fn git_branch(dir: &str) -> String {
    if !Path::new(dir).is_absolute() {
        return String::new();
    }
    let mut at = Some(Path::new(dir));
    while let Some(d) = at {
        let dot = d.join(".git");
        if dot.is_dir() {
            return head_of(&dot);
        }
        if let Ok(text) = std::fs::read_to_string(&dot)
            && let Some(rest) = text.trim().strip_prefix("gitdir:")
        {
            let gitdir = PathBuf::from(rest.trim());
            let gitdir = if gitdir.is_absolute() { gitdir } else { d.join(gitdir) };
            return head_of(&gitdir);
        }
        at = d.parent();
    }
    String::new()
}

fn head_of(gitdir: &Path) -> String {
    let Ok(head) = std::fs::read_to_string(gitdir.join("HEAD")) else { return String::new() };
    let head = head.trim();
    match head.strip_prefix("ref: ") {
        Some(r) => r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
        None => head.chars().take(7).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_is_read_from_the_head_file() {
        let dir = std::env::temp_dir().join(format!("keepane-git-{}", std::process::id()));
        let repo = dir.join("repo");
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&sub.to_string_lossy()), "feature/x", "found from a subdirectory");
        std::fs::write(repo.join(".git").join("HEAD"), "a1b2c3d4e5f6a7b8\n").unwrap();
        assert_eq!(git_branch(&repo.to_string_lossy()), "a1b2c3d", "detached: the short commit");
        // A worktree: .git is a file pointing at the real git dir.
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let gd = dir.join("gitdir");
        std::fs::create_dir_all(&gd).unwrap();
        std::fs::write(gd.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gd.display())).unwrap();
        assert_eq!(git_branch(&wt.to_string_lossy()), "wt-branch");
        assert_eq!(git_branch(&dir.to_string_lossy()), "", "no repository above");
        let nowhere = if cfg!(windows) { r"Z:\no\such\place" } else { "/no/such/place" };
        assert_eq!(git_branch(nowhere), "");
        assert_eq!(git_branch("src"), "", "relative: not looked up from this process's directory");
        assert_eq!(git_branch(""), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
