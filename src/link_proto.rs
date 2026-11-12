//! What two machines say over a link stream (docs/design/link.md, decision
//! 17), as protobuf (`prost`): the handshake's payloads (`Hello`, `Answer`),
//! then `ToClient` from the machine copied and `ToServer` back. Field
//! numbers are for good: a field is added with a new number, never reused,
//! so keepane versions on the two sides read each other.
//!
//! A pane's terminal model (`Screen`, a snapshot's screens) goes deflated:
//! most of a screen is blank or repeated.

use prost::Message;

use crate::server::journal;

/// The first handshake message's payload: from the machine that copies.
#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    /// The newest change it has (it wants those after), or none: a snapshot.
    #[prost(uint64, optional, tag = "1")]
    pub since: Option<u64>,
    #[prost(string, tag = "2")]
    pub version: String,
    /// When it was sent (Unix seconds): a first message recorded and sent
    /// again later is refused (and one sent twice, by its ephemeral key).
    #[prost(int64, tag = "3")]
    pub time: i64,
    /// Which run of that server `since` counts in (`Answer::epoch`): the
    /// numbers start again when it restarts.
    #[prost(uint64, optional, tag = "4")]
    pub epoch: Option<u64>,
}

/// The second handshake message's payload: welcome, or why not.
#[derive(Clone, PartialEq, Message)]
pub struct Answer {
    #[prost(string, tag = "1")]
    pub version: String,
    /// Set: refused, and why; the connection then ends.
    #[prost(string, optional, tag = "2")]
    pub refused: Option<String>,
    /// This run of the server: its changes are numbered within it.
    #[prost(uint64, tag = "3")]
    pub epoch: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct ToServer {
    #[prost(oneof = "to_server::Kind", tags = "1, 2")]
    pub kind: Option<to_server::Kind>,
}

pub mod to_server {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Ping(super::Ping),
        #[prost(message, tag = "2")]
        Input(super::Input),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ToClient {
    #[prost(oneof = "to_client::Kind", tags = "1, 2, 3")]
    pub kind: Option<to_client::Kind>,
}

pub mod to_client {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Entry(super::Entry),
        #[prost(message, tag = "2")]
        Snapshot(super::Snapshot),
        #[prost(message, tag = "3")]
        Ping(super::Ping),
    }
}

/// Still there (each side says it when it has said nothing for a while).
#[derive(Clone, PartialEq, Message)]
pub struct Ping {}

/// What is typed into a pane of the copy, for the pane it copies (by its
/// id there): bytes as the program reads them, put in by the copy's own
/// keys (the copy's model has the same modes as the original's). A keepane
/// of 0.61 or before reads it as nothing.
#[derive(Clone, PartialEq, Message)]
pub struct Input {
    #[prost(uint32, tag = "1")]
    pub pane: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Entry {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    #[prost(int64, tag = "2")]
    pub at: i64,
    #[prost(oneof = "entry::Change", tags = "3, 4")]
    pub change: Option<entry::Change>,
}

pub mod entry {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Change {
        #[prost(message, tag = "3")]
        Shape(super::Shape),
        #[prost(message, tag = "4")]
        Pane(super::PaneChange),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Shape {
    #[prost(message, repeated, tag = "1")]
    pub sessions: Vec<SessionShape>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SessionShape {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(uint32, tag = "3")]
    pub cols: u32,
    #[prost(uint32, tag = "4")]
    pub rows: u32,
    #[prost(uint64, tag = "5")]
    pub current: u64,
    #[prost(message, repeated, tag = "6")]
    pub windows: Vec<WindowShape>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WindowShape {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(uint64, tag = "2")]
    pub index: u64,
    #[prost(string, tag = "3")]
    pub name: String,
    #[prost(string, tag = "4")]
    pub layout: String,
    #[prost(uint32, tag = "5")]
    pub active: u32,
    #[prost(bool, tag = "6")]
    pub zoomed: bool,
    #[prost(message, repeated, tag = "7")]
    pub panes: Vec<PaneShape>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PaneShape {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(uint32, tag = "2")]
    pub x: u32,
    #[prost(uint32, tag = "3")]
    pub y: u32,
    #[prost(uint32, tag = "4")]
    pub cols: u32,
    #[prost(uint32, tag = "5")]
    pub rows: u32,
    #[prost(string, tag = "6")]
    pub title: String,
    #[prost(string, tag = "7")]
    pub command: String,
    #[prost(string, optional, tag = "8")]
    pub cwd: Option<String>,
    #[prost(string, optional, tag = "9")]
    pub name: Option<String>,
    #[prost(string, tag = "10")]
    pub mode: String,
    #[prost(uint32, optional, tag = "11")]
    pub exited: Option<u32>,
    #[prost(uint32, tag = "12")]
    pub generation: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct PaneChange {
    #[prost(uint32, tag = "1")]
    pub pane: u32,
    #[prost(oneof = "pane_change::Kind", tags = "2, 3, 4, 5, 6, 7, 8")]
    pub kind: Option<pane_change::Kind>,
}

pub mod pane_change {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "2")]
        Output(super::Output),
        #[prost(message, tag = "3")]
        Size(super::Size),
        #[prost(message, tag = "4")]
        Reset(super::Reset),
        #[prost(message, tag = "5")]
        Screen(super::Model),
        #[prost(message, tag = "6")]
        Fullscreen(super::Fullscreen),
        #[prost(message, tag = "7")]
        CommandStart(super::CommandStart),
        #[prost(message, tag = "8")]
        CommandEnd(super::CommandEnd),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Output {
    #[prost(uint64, tag = "1")]
    pub at: u64,
    #[prost(bytes = "vec", tag = "2")]
    pub bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Size {
    #[prost(uint32, tag = "1")]
    pub cols: u32,
    #[prost(uint32, tag = "2")]
    pub rows: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Reset {
    #[prost(uint32, tag = "1")]
    pub cols: u32,
    #[prost(uint32, tag = "2")]
    pub rows: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub bytes: Vec<u8>,
    /// Where the pane's next output starts.
    #[prost(uint64, tag = "4")]
    pub at: u64,
}

/// A pane's terminal model whole (vt100's `Screen::snapshot`), deflated.
#[derive(Clone, PartialEq, Message)]
pub struct Model {
    #[prost(bytes = "vec", tag = "1")]
    pub deflated: Vec<u8>,
    /// Where the pane's next output starts.
    #[prost(uint64, tag = "2")]
    pub at: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Fullscreen {
    #[prost(bool, tag = "1")]
    pub on: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommandStart {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(string, optional, tag = "2")]
    pub cwd: Option<String>,
    #[prost(uint64, tag = "3")]
    pub at: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommandEnd {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(sint32, optional, tag = "2")]
    pub exit: Option<i32>,
    #[prost(uint64, tag = "3")]
    pub secs: u64,
    #[prost(uint64, tag = "4")]
    pub at: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Snapshot {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    #[prost(message, optional, tag = "2")]
    pub shape: Option<Shape>,
    #[prost(message, repeated, tag = "3")]
    pub panes: Vec<PaneScreen>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PaneScreen {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(bool, tag = "2")]
    pub fullscreen: bool,
    /// Its terminal model whole, deflated (`Model`).
    #[prost(bytes = "vec", tag = "3")]
    pub deflated: Vec<u8>,
    #[prost(uint64, tag = "4")]
    pub at: u64,
}

pub fn deflate(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = e.write_all(bytes);
    e.finish().unwrap_or_default()
}

/// `deflate` back, at most `max` bytes; None for what it did not write.
pub fn inflate(bytes: &[u8], max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(bytes).take(max as u64 + 1).read_to_end(&mut out).ok()?;
    (out.len() <= max).then_some(out)
}

impl From<&journal::Shape> for Shape {
    fn from(s: &journal::Shape) -> Self {
        Shape {
            sessions: s
                .sessions
                .iter()
                .map(|s| SessionShape {
                    id: s.id,
                    name: s.name.clone(),
                    cols: s.cols.into(),
                    rows: s.rows.into(),
                    current: s.current as u64,
                    windows: s
                        .windows
                        .iter()
                        .map(|w| WindowShape {
                            id: w.id,
                            index: w.index as u64,
                            name: w.name.clone(),
                            layout: w.layout.clone(),
                            active: w.active,
                            zoomed: w.zoomed,
                            panes: w
                                .panes
                                .iter()
                                .map(|p| PaneShape {
                                    id: p.id,
                                    x: p.x.into(),
                                    y: p.y.into(),
                                    cols: p.cols.into(),
                                    rows: p.rows.into(),
                                    title: p.title.clone(),
                                    command: p.command.clone(),
                                    cwd: p.cwd.clone(),
                                    name: p.name.clone(),
                                    mode: p.mode.to_string(),
                                    exited: p.exited,
                                    generation: p.generation,
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl PaneChange {
    pub fn from_journal(pane: u32, c: &journal::PaneChange) -> Self {
        use pane_change::Kind;
        let kind = match c {
            journal::PaneChange::Output { at, bytes } => Kind::Output(Output { at: *at, bytes: bytes.clone() }),
            journal::PaneChange::Size { cols, rows } => Kind::Size(Size { cols: (*cols).into(), rows: (*rows).into() }),
            journal::PaneChange::Reset { cols, rows, bytes, at } => {
                Kind::Reset(Reset { cols: (*cols).into(), rows: (*rows).into(), bytes: bytes.clone(), at: *at })
            }
            journal::PaneChange::Screen { snapshot, at } => {
                Kind::Screen(Model { deflated: deflate(snapshot), at: *at })
            }
            journal::PaneChange::Fullscreen { on } => Kind::Fullscreen(Fullscreen { on: *on }),
            journal::PaneChange::CommandStart { command, cwd, at } => {
                Kind::CommandStart(CommandStart { command: command.clone(), cwd: cwd.clone(), at: *at })
            }
            journal::PaneChange::CommandEnd { command, exit, secs, at } => {
                Kind::CommandEnd(CommandEnd { command: command.clone(), exit: *exit, secs: *secs, at: *at })
            }
        };
        PaneChange { pane, kind: Some(kind) }
    }
}

impl From<&journal::Entry> for Entry {
    fn from(e: &journal::Entry) -> Self {
        let change = match &e.change {
            journal::Change::Shape { shape } => entry::Change::Shape(shape.into()),
            journal::Change::Pane { pane, change } => entry::Change::Pane(PaneChange::from_journal(*pane, change)),
        };
        Entry { seq: e.seq, at: e.at, change: Some(change) }
    }
}

impl From<&journal::Snapshot> for Snapshot {
    fn from(s: &journal::Snapshot) -> Self {
        Snapshot {
            seq: s.seq,
            shape: Some((&s.shape).into()),
            panes: s
                .panes
                .iter()
                .map(|p| PaneScreen { id: p.id, fullscreen: p.fullscreen, deflated: deflate(&p.screen), at: p.at })
                .collect(),
        }
    }
}

/// `msg` as protobuf, its length first (what a stream carries).
pub fn framed(msg: &impl Message) -> Vec<u8> {
    msg.encode_length_delimited_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_goes_and_comes_back_the_same() {
        let e = journal::Entry {
            seq: 7,
            at: 1_700_000_000_000,
            change: journal::Change::Pane {
                pane: 3,
                change: journal::PaneChange::CommandEnd { command: "make".into(), exit: Some(-2), secs: 4, at: 99 },
            },
        };
        let wire = ToClient { kind: Some(to_client::Kind::Entry((&e).into())) };
        let bytes = framed(&wire);
        let back = ToClient::decode_length_delimited(&bytes[..]).unwrap();
        assert_eq!(back, wire);
        // A field this side does not know (a newer keepane's) is skipped.
        let mut newer = Hello { since: Some(5), version: "9.9.9".into(), time: 0, epoch: None }.encode_to_vec();
        newer.extend_from_slice(&[0x9a, 0x06, 0x02, b'h', b'i']); // field 99, a string
        assert_eq!(Hello::decode(&newer[..]).unwrap().since, Some(5));
    }

    #[test]
    fn a_screen_goes_deflated_and_comes_back_whole() {
        let mut p = vt100::Parser::new(50, 200, 1000);
        for i in 0..300 {
            p.process(format!("\x1b[3{}mline {i} of a long listing\x1b[0m\r\n", i % 8).as_bytes());
        }
        let snap = p.screen().snapshot();
        let small = deflate(&snap);
        assert!(small.len() * 4 < snap.len(), "{} -> {}", snap.len(), small.len());
        assert_eq!(inflate(&small, snap.len()).unwrap(), snap);
        assert!(inflate(&small, snap.len() - 1).is_none(), "bigger than asked: refused");
        assert!(inflate(b"not deflate", 100).is_none() || inflate(b"not deflate", 100).is_some_and(|v| v.len() <= 100));
    }
}
