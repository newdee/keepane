//! Starting the server at logon: on Windows a per-user `Run` value. Here it
//! is not done yet (a systemd user unit, a launchd agent); the command says so.

use anyhow::Result;

/// Nothing is registered here that runs an exe by its path (no logon
/// command yet, no Windows Terminal), so nothing goes stale on an update.
pub fn refresh_registrations(_socket: &str) -> Vec<String> {
    Vec::new()
}

/// `keepane startup ...`.
pub fn run(_socket: &str, _args: &[String]) -> Result<i32> {
    eprintln!("keepane startup: not available here yet; start the server from your login files (keepane new -d)");
    Ok(1)
}
