//! Opening a file or an address with what Windows uses for it
//! (`ShellExecuteW`, the "open" verb: the default browser, the program a
//! file's type belongs to).

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Start `prog` with `args` and leave it running (VS Code's `code.cmd`):
/// without a console window of its own (the server has none to lend, so a
/// script would flash one up), waited for on a thread.
pub fn spawn(prog: &std::path::Path, args: &[String]) -> Result<(), String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{}: {e}", prog.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Hand `what` (an absolute path or a URL) to Windows. Returns at once: the
/// shell can take a moment to start a browser, so it runs on a thread of
/// its own, and what goes wrong there is logged.
pub fn open(what: &str) -> Result<(), String> {
    if what.starts_with('-') {
        return Err(format!("not a path or an address: {what}"));
    }
    let what = what.to_string();
    std::thread::spawn(move || {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (verb, file) = (wide("open"), wide(&what));
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // Above 32 is success (the documented convention for this call).
        if (r as isize) <= 32 {
            log::warn!("open {what}: ShellExecuteW returned {}", r as isize);
        }
    });
    Ok(())
}
