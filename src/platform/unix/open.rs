//! Opening a file or an address with what the desktop uses for it: `open`
//! on macOS, `xdg-open` elsewhere.

use std::process::{Command, Stdio};

/// Start `prog` with `args` and leave it running (VS Code's `code`),
/// waited for on a thread.
pub fn spawn(prog: &std::path::Path, args: &[String]) -> Result<(), String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", prog.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Hand `what` (an absolute path or a URL) to the desktop. Returns at once;
/// the program it starts is waited for on a thread of its own.
pub fn open(what: &str) -> Result<(), String> {
    // Never an option to the opener.
    if what.starts_with('-') {
        return Err(format!("not a path or an address: {what}"));
    }
    let prog = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    spawn(std::path::Path::new(prog), &[what.to_string()])
}
