//! Processes keepane starts outside a pane: itself again, detached (the
//! server, `restart-server` finishing outside a pane), and one shell command
//! for `run-shell` and `pipe-pane`; and the guard that ends a process's whole
//! tree with it: its process group.

use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;

/// A process and everything it starts (its process group): all of it is
/// sent SIGHUP when this is dropped, and SIGKILL a moment later, as tmux
/// does to a pane it closes.
pub struct Tree {
    pgid: libc::pid_t,
}

impl Tree {
    /// The tree of a pane's process: portable-pty makes each pane's child the
    /// leader of a session of its own, so its pid is its process group.
    pub fn of_pty(child: &(dyn portable_pty::Child + Send + Sync)) -> Result<Tree> {
        let pid = child.process_id().context("no process id")?;
        Ok(Tree { pgid: pid as libc::pid_t })
    }

    /// The tree of a child `shell_command` or `pipe_command` started (in a
    /// process group of its own).
    pub fn of(child: &std::process::Child) -> Result<Tree> {
        Ok(Tree { pgid: child.id() as libc::pid_t })
    }
}

/// Every process in session `sid` (Linux: `/proc/<pid>/stat`'s sixth
/// field). An interactive shell puts each job in a process group of its
/// own, so the pane's whole tree is its session, not its group.
#[cfg(target_os = "linux")]
fn session_members(sid: libc::pid_t) -> Vec<libc::pid_t> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<libc::pid_t>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        let Some(close) = stat.rfind(')') else { continue };
        // After "(comm)": state ppid pgrp session ...
        if stat[close + 1..].split_whitespace().nth(3).and_then(|s| s.parse::<libc::pid_t>().ok()) == Some(sid) {
            out.push(pid);
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn session_members(_sid: libc::pid_t) -> Vec<libc::pid_t> {
    Vec::new()
}

/// `sig` to the group `pgid` and to every process of the session it leads.
fn signal_tree(pgid: libc::pid_t, sig: libc::c_int) {
    unsafe { libc::kill(-pgid, sig) };
    for pid in session_members(pgid) {
        unsafe { libc::kill(pid, sig) };
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let pgid = self.pgid;
        if pgid <= 1 {
            return;
        }
        signal_tree(pgid, libc::SIGHUP);
        // What ignores the hang-up goes anyway, shortly.
        let _ = std::thread::Builder::new().name("tree-kill".into()).spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            signal_tree(pgid, libc::SIGKILL);
        });
    }
}

/// A command line run through `/bin/sh -c`, in a process group of its own so
/// a `Tree` can end it with everything it started. Returns the command and
/// the program's name.
/// Nothing to do here: a pane's program gets SIGINT from its terminal as
/// usual (Windows keeps a flag that needs turning back on).
pub fn let_panes_take_ctrl_c() {}

pub fn shell_command(command: &str) -> (std::process::Command, &'static str) {
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(command).process_group(0);
    (c, "/bin/sh")
}

/// A program the server runs for itself (`curl` for `done-webhook`): no
/// window to keep away here.
pub fn quiet_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

/// The command a `pipe-pane` runs: the same, through `/bin/sh -c`.
pub fn pipe_command(command: &str) -> (std::process::Command, &'static str) {
    shell_command(command)
}

/// Leaving a job is a Windows matter; nothing here refuses it.
pub fn leave_job_denied(_e: &anyhow::Error) -> bool {
    false
}

/// Run this program again, detached: a session of its own (so a terminal's
/// or an SSH connection's hang-up does not reach it), stdio on /dev/null.
/// `_breakaway` is the Windows job; a new session leaves everything here.
pub fn spawn_self(args: &[&str], _breakaway: bool) -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let mut c = std::process::Command::new(exe);
    c.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().with_context(|| format!("spawn {}", args.join(" ")))?;
    Ok(())
}
