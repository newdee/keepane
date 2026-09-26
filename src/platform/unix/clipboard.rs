//! The desktop's clipboard through the tool it has: wl-copy / wl-paste on
//! Wayland, xclip or xsel on X11, pbcopy / pbpaste on macOS. None of them
//! there (a server over SSH): an error, and copy mode keeps its buffer.

use anyhow::{Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

const SETTERS: &[(&str, &[&str])] = &[
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
];
const GETTERS: &[(&str, &[&str])] = &[
    ("pbpaste", &[]),
    ("wl-paste", &["--no-newline"]),
    ("xclip", &["-selection", "clipboard", "-o"]),
    ("xsel", &["--clipboard", "--output"]),
];

pub fn set_text(text: &str) -> Result<()> {
    for (prog, args) in SETTERS {
        if crate::config::which(prog).is_none() {
            continue;
        }
        let Ok(mut child) = Command::new(prog).args(*args).stdin(Stdio::piped()).stdout(Stdio::null()).spawn() else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }
    bail!("no clipboard tool (wl-copy, xclip, xsel, pbcopy)")
}

pub fn get_text() -> Result<String> {
    for (prog, args) in GETTERS {
        if crate::config::which(prog).is_none() {
            continue;
        }
        if let Ok(o) = Command::new(prog).args(*args).stderr(Stdio::null()).output()
            && o.status.success()
        {
            return Ok(String::from_utf8_lossy(&o.stdout).into_owned());
        }
    }
    bail!("no clipboard tool (wl-paste, xclip, xsel, pbpaste)")
}
