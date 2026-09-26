//! The Windows Terminal profile is Windows's.

use anyhow::Result;

/// `keepane windows-terminal ...`.
pub fn run(_socket: &str, _args: &[String]) -> Result<i32> {
    eprintln!("keepane windows-terminal: Windows Terminal is on Windows only");
    Ok(1)
}
