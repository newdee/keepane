//! The server's address and the connection to it: a Unix socket in a
//! directory only this user may enter (`<runtime>/keepane-<uid>/<socket>`,
//! mode 0700), tmux's model.

use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// A client's end of the connection.
pub type Stream = tokio::net::UnixStream;
/// The server's end of one client's connection.
pub type Connection = tokio::net::UnixStream;

/// Where this user's sockets live: `$KEEPANE_TMPDIR`, else
/// `$XDG_RUNTIME_DIR`, else `/tmp`, then `keepane-<uid>`.
fn socket_dir() -> PathBuf {
    let base = crate::legacy::var_os("KEEPANE_TMPDIR")
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("keepane-{}", unsafe { libc::getuid() }))
}

/// The socket for `socket_name`. The name is reduced to `[A-Za-z0-9_.-]`
/// so a `-L` value can never leave the directory.
pub fn address(socket_name: &str) -> String {
    let clean: String = socket_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' { c } else { '_' })
        .collect();
    let clean = if clean.is_empty() || clean.chars().all(|c| c == '.') { "default".to_string() } else { clean };
    socket_dir().join(clean).to_string_lossy().into_owned()
}

/// Open a connection to the server at `addr`.
pub fn connect(addr: &str) -> std::io::Result<Stream> {
    let s = std::os::unix::net::UnixStream::connect(addr)?;
    s.set_nonblocking(true)?;
    Stream::from_std(s)
}

/// A Unix socket is never busy: it queues connections.
pub fn is_busy(_e: &std::io::Error) -> bool {
    false
}

/// Whether a failed `connect` means no server listens there: no socket
/// file, or a file left behind by a server that is gone.
pub fn is_absent(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused)
}

/// Whether a server is listening at `addr`.
pub fn server_running(addr: &str) -> bool {
    std::os::unix::net::UnixStream::connect(addr).is_ok()
}

/// The server's side.
pub struct Listener {
    inner: tokio::net::UnixListener,
}

impl Listener {
    /// Start listening at `addr`; fails when a server already listens there.
    /// A socket file no server answers on is left from one that died, and
    /// goes.
    pub fn bind(addr: &str) -> Result<Listener> {
        let path = std::path::Path::new(addr);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            let meta = std::fs::metadata(dir)?;
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != unsafe { libc::getuid() } {
                anyhow::bail!("{} belongs to another user", dir.display());
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            if server_running(addr) {
                anyhow::bail!("a server is already listening on {addr}");
            }
            let _ = std::fs::remove_file(path);
        }
        let inner = tokio::net::UnixListener::bind(path).with_context(|| format!("listen on {addr}"))?;
        Ok(Listener { inner })
    }

    /// The next client's connection.
    pub async fn accept(&mut self) -> std::io::Result<Connection> {
        self.inner.accept().await.map(|(s, _)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_stay_in_their_directory() {
        let dir = socket_dir().to_string_lossy().into_owned();
        assert!(address("default").starts_with(&dir));
        assert!(address("default").ends_with("/default"));
        assert!(address("../evil").ends_with("/.._evil"));
        assert!(address("").ends_with("/default"));
        assert!(address("..").ends_with("/default"));
        assert!(address("a b/c").ends_with("/a_b_c"));
    }
}
