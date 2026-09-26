//! Starting the server at logon: on Windows a per-user `Run` value. Here it
//! is not done yet (a systemd user unit, a launchd agent); the command says so.

use anyhow::Result;

/// `keepane startup ...`.
pub fn run(_socket: &str, _args: &[String]) -> Result<i32> {
    eprintln!("keepane startup: not available here yet; start the server from your login files (keepane new -d)");
    Ok(1)
}
