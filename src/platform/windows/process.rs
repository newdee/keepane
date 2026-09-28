//! Processes keepane starts outside a pane: itself again, detached (the
//! server, `restart-server` finishing outside a pane), and one shell command
//! for `run-shell` and the like; and the guard that ends a process's whole
//! tree with it (a kill-on-close job object).

use super::winsec::KillOnCloseJob;
use anyhow::{Context, Result};

/// A process and everything it starts: all of it ends when this is dropped.
pub struct Tree {
    _job: KillOnCloseJob,
}

impl Tree {
    /// The tree of a pane's process (portable-pty's child).
    pub fn of_pty(child: &(dyn portable_pty::Child + Send + Sync)) -> Result<Tree> {
        let job = KillOnCloseJob::new()?;
        let h = child.as_raw_handle().context("no process handle")?;
        // The handle comes straight from CreateProcessW inside portable-pty.
        unsafe { job.assign(h as _) }?;
        Ok(Tree { _job: job })
    }

    /// The tree of a child `shell_command` started.
    pub fn of(child: &std::process::Child) -> Result<Tree> {
        use std::os::windows::io::AsRawHandle;
        let job = KillOnCloseJob::new()?;
        // Straight from CreateProcess; killing the job takes grandchildren too.
        unsafe { job.assign(child.as_raw_handle() as _) }?;
        Ok(Tree { _job: job })
    }
}

/// A command line run through the shell, to completion, with no window
/// (the server has no console): pwsh when there is one, else Windows
/// PowerShell, else cmd. Returns the command and the program's name.
pub fn shell_command(command: &str) -> (std::process::Command, &'static str) {
    use std::os::windows::process::CommandExt;
    // `-Command` alone reports 0/1; make a native command's exit code
    // propagate like `sh -c` does for tmux.
    // Without a console PowerShell writes the ANSI code page; ask for UTF-8 so
    // non-ASCII output survives the trip into the status line / overlay.
    let ps_script = format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8\n\
         {command}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}"
    );
    let (exe, args): (&'static str, Vec<&str>) = if crate::config::which("pwsh.exe").is_some() {
        ("pwsh.exe", vec!["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &ps_script])
    } else if crate::config::which("powershell.exe").is_some() {
        ("powershell.exe", vec!["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &ps_script])
    } else {
        ("cmd.exe", vec!["/d", "/c", command])
    };
    let mut c = std::process::Command::new(exe);
    c.args(&args);
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: the server has no console
    (c, exe)
}

/// The command a `pipe-pane` runs, reading the pane's output and/or
/// typing into it, with no window: pwsh, else Windows PowerShell, else cmd.
/// Returns the command and the program's name.
pub fn pipe_command(command: &str) -> (std::process::Command, &'static str) {
    use std::os::windows::process::CommandExt;
    let (exe, args): (&'static str, Vec<String>) = match crate::config::which("pwsh.exe") {
        Some(_) => ("pwsh.exe", vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into(), command.into()]),
        None => match crate::config::which("powershell.exe") {
            Some(_) => {
                ("powershell.exe", vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into(), command.into()])
            }
            None => ("cmd.exe", vec!["/d".into(), "/c".into(), command.into()]),
        },
    };
    let mut c = std::process::Command::new(exe);
    c.args(&args);
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: the server has no console
    (c, exe)
}

/// Let the panes' programs take Ctrl+C. The server is started in a process
/// group of its own (`spawn_self`), and a process so started ignores Ctrl+C,
/// which its children inherit: a pane's shell, and a `ping` or a build in
/// it, would never stop at Ctrl+C (only programs that read the key
/// themselves, as PSReadLine or an agent's prompt, would). Turned back on
/// here, for the panes started from now on, as Windows documents it
/// (`SetConsoleCtrlHandler(NULL, FALSE)`). The server has no console, so
/// no Ctrl+C reaches it.
pub fn let_panes_take_ctrl_c() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
        log::warn!("could not let the panes take Ctrl+C: {}", std::io::Error::last_os_error());
    }
}

/// Whether starting with `breakaway` failed only because the caller's job
/// does not allow leaving it (CreateProcess says access denied).
pub fn leave_job_denied(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().and_then(|e| e.raw_os_error()) == Some(5)
}
/// Run this program again, detached from the console and inheriting no
/// handles. `breakaway` also takes it out of the job the caller is in (a
/// pane's kill-on-close job), which only a job allowing it permits.
pub fn spawn_self(args: &[&str], breakaway: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };
    let exe = std::env::current_exe().context("current_exe")?;
    let quote = |s: &str| -> String {
        if s.is_empty() || s.contains([' ', '\t', '"']) {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            s.to_string()
        }
    };
    let mut cmdline = quote(&exe.to_string_lossy());
    for a in args {
        cmdline.push(' ');
        cmdline.push_str(&quote(a));
    }
    let flags = DETACHED_PROCESS
        | CREATE_NEW_PROCESS_GROUP
        | CREATE_UNICODE_ENVIRONMENT
        | if breakaway { CREATE_BREAKAWAY_FROM_JOB } else { 0 };
    let mut exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut cmd_w: Vec<u16> = std::ffi::OsStr::new(&cmdline).encode_wide().chain(std::iter::once(0)).collect();
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            exe_w.as_mut_ptr(),
            cmd_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // bInheritHandles = FALSE
            flags,
            std::ptr::null(), // inherit our environment block
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("spawn {}", args.join(" ")));
    }
    unsafe {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    Ok(())
}
