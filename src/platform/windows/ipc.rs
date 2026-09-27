//! The server's address and the connection to it: a named pipe,
//! `\\.\pipe\keepane-<user>-<socket>`, that only this user (and SYSTEM) may
//! open, the Windows form of tmux's mode-0700 socket directory.

use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};

/// A client's end of the connection.
pub type Stream = NamedPipeClient;
/// The server's end of one client's connection.
pub type Connection = NamedPipeServer;

/// Every instance of the pipe is taken: the server is there, only busy.
const ERROR_PIPE_BUSY: i32 = 231;
/// No such pipe: no server.
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// Name of the per-user named pipe the server listens on. Both the user name
/// and the socket name are reduced to `[A-Za-z0-9_.-]` so a `-L` value can
/// never escape the `keepane-<user>-` namespace (pipe names accept `\`).
pub fn address(socket_name: &str) -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let clean = |s: &str| -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' { c } else { '_' }).collect()
    };
    let (user, socket) = (clean(&user), clean(socket_name));
    let socket = if socket.is_empty() { "default".to_string() } else { socket };
    format!(r"\\.\pipe\keepane-{user}-{socket}")
}

/// The socket names (`-L`) that servers of this user listen on: the pipes
/// named like `address` makes them.
pub fn sockets() -> Vec<String> {
    let prefix = address("");
    let prefix = prefix.trim_end_matches("default").trim_start_matches(r"\\.\pipe\");
    let mut v: Vec<String> = std::fs::read_dir(r"\\.\pipe\")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_prefix(prefix).map(str::to_string))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Open a connection to the server at `addr`.
pub fn connect(addr: &str) -> std::io::Result<Stream> {
    ClientOptions::new().open(addr)
}

/// Whether a failed `connect` means the server is there but every instance
/// of its pipe is taken for the moment (try again shortly).
pub fn is_busy(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_PIPE_BUSY)
}

/// Whether a failed `connect` means no server listens there at all.
pub fn is_absent(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND)
}

/// Whether a server is listening at `addr`.
pub fn server_running(addr: &str) -> bool {
    match connect(addr) {
        Ok(_) => true,
        Err(e) => is_busy(&e),
    }
}

/// The server's side: one pipe instance waits for the next client, and a new
/// one is made as each client comes, so there is always one waiting.
pub struct Listener {
    addr: String,
    sec: super::winsec::OwnerOnly,
    next: NamedPipeServer,
}

impl Listener {
    /// Start listening at `addr`; fails when a server already listens there.
    pub fn bind(addr: &str) -> Result<Listener> {
        // Only this user (and SYSTEM) may open the pipe: tmux's 0700 socket.
        let mut sec = super::winsec::OwnerOnly::new().context("pipe security descriptor")?;
        // The first instance must be created before we are considered up.
        let next = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(addr, sec.as_ptr() as *mut _)
        }
        .with_context(|| format!("create pipe {addr} (server already running?)"))?;
        Ok(Listener { addr: addr.to_string(), sec, next })
    }

    /// The next client's connection.
    pub async fn accept(&mut self) -> std::io::Result<Connection> {
        self.next.connect().await?;
        let fresh = unsafe {
            ServerOptions::new()
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(&self.addr, self.sec.as_ptr() as *mut _)
        }?;
        Ok(std::mem::replace(&mut self.next, fresh))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_sanitized() {
        let n = address("default");
        assert!(n.starts_with(r"\\.\pipe\keepane-"));
        assert!(n.ends_with("-default"));
        assert!(address(r"..\evil").ends_with("-.._evil"));
        assert!(address("").ends_with("-default"));
        assert!(address("a b/c").ends_with("-a_b_c"));
    }
}
