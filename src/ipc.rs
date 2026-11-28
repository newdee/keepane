//! Client <-> server wire protocol over a Windows named pipe.
//!
//! Framing: `u32` little-endian payload length followed by a bincode payload.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME: u32 = 64 * 1024 * 1024;

/// Where the server for socket `socket_name` listens: the platform's
/// address for it (a named pipe on Windows, a Unix socket elsewhere).
pub fn pipe_name(socket_name: &str) -> String {
    crate::platform::ipc::address(socket_name)
}

pub use crate::keys::KeyRecord;

/// The address `web-status` (or `web-start`) answers on its first line, when
/// the page is served: `serving <url> · ...`.
pub fn status_url(status: &str) -> Option<&str> {
    let first = status.lines().next()?.strip_prefix("serving ")?;
    Some(first.split(" · ").next().unwrap_or(first))
}

/// Raw Windows `MOUSE_EVENT_RECORD`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseRecord {
    pub x: i16,
    pub y: i16,
    pub buttons: u32,
    pub ctrl: u32,
    pub flags: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientMsg {
    /// Run a command (the argv of the `keepane` CLI, or a `:` prompt line).
    /// If the command attaches, the server answers `Attached` and the stream
    /// switches into interactive mode.
    Command {
        version: u32,
        argv: Vec<String>,
        cwd: String,
        cols: u16,
        rows: u16,
        /// Client is running on a real console and may be attached.
        interactive: bool,
        /// Value of `KEEPANE_PANE` in the client's environment (running inside a pane).
        pane_env: Option<u32>,
    },
    Key(KeyRecord),
    Mouse(MouseRecord),
    Resize {
        cols: u16,
        rows: u16,
    },
    Detach,
    /// What the attached client's terminal can do: whether it shows kitty's
    /// pictures (kitty.rs). Sent only to a server of the client's own
    /// version (an older one does not know this message), after `Attached`.
    Terminal {
        kitty: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ServerMsg {
    /// The client is now attached to `session`; raw terminal output follows.
    Attached {
        session: String,
    },
    /// Raw VT bytes for the client console.
    Output(Vec<u8>),
    /// Plain text for a non-interactive command (list-sessions, ...).
    Text(String),
    /// A non-interactive command finished.
    Done {
        code: i32,
    },
    /// The attached client was detached (prefix-d, session killed, ...).
    Detached {
        reason: String,
    },
    Error(String),
    /// Whether the console should capture mouse events for keepane (`mouse`
    /// option). When false the host terminal keeps its native selection.
    SetMouse(bool),
    /// Bring the client's terminal window to the front (`focus-pane`, the
    /// "Go to pane" button of a notification).
    Raise,
}

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let payload = bincode::serialize(msg).context("serialize frame")?;
    let len = u32::try_from(payload.len()).context("frame too large")?;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&payload);
    w.write_all(&buf).await.context("write frame")?;
    Ok(())
}

pub async fn read_frame<R, T>(r: &mut R) -> Result<Option<T>>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.raw_os_error() == Some(109) => return Ok(None), // ERROR_BROKEN_PIPE
        Err(e) => return Err(e).context("read frame header"),
    }
    let len = u32::from_le_bytes(hdr);
    if len > MAX_FRAME {
        bail!("frame too large: {len}");
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await.context("read frame body")?;
    let msg = bincode::deserialize(&payload).context("deserialize frame")?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let msg = ClientMsg::Key(KeyRecord { down: true, repeat: 1, vk: 0x42, sc: 48, ch: 2, ctrl: 8 });
        write_frame(&mut a, &msg).await.unwrap();
        let got: ClientMsg = read_frame(&mut b).await.unwrap().unwrap();
        match got {
            ClientMsg::Key(k) => assert_eq!(k.vk, 0x42),
            _ => panic!("wrong message"),
        }
        drop(a);
        let eof: Option<ClientMsg> = read_frame(&mut b).await.unwrap();
        assert!(eof.is_none());
    }
}
