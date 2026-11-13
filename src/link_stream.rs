//! A link stream (docs/design/link.md, decision 17): one TCP connection
//! between two paired machines, encrypted and both ways, that carries what
//! one machine needs to keep a copy of the other's sessions.
//!
//! It starts as an HTTP request on the other machine's web port
//! (`GET /link/stream`, `Upgrade: keepane-stream`), answered `101`; from
//! then on the connection carries frames: two bytes of length (big-endian),
//! then a Noise message of at most 65535 bytes.
//!
//! Noise `IK` (25519, ChaCha20-Poly1305, BLAKE2s): the machine that copies
//! knows the other's static key from pairing and sends its own, encrypted,
//! in the first message; the other learns who it is from that and checks
//! it against its table. Both static keys are the machines' link keys
//! (Ed25519), turned into X25519 (`Identity::x25519_secret`,
//! `x25519_public`). Each connection has keys of its own (the ephemeral
//! ones): what was recorded cannot be read later with the static keys.
//!
//! The payloads are protobuf (`crate::link_proto`): `Hello` and `Answer` in
//! the handshake, then length-delimited messages, which may span frames.

use anyhow::{Context, Result, bail};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The Noise pattern and primitives.
pub const NOISE: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// What both handshakes are bound to: another protocol's messages are not
/// taken for this one's.
pub const PROLOGUE: &[u8] = b"keepane link stream 1";
/// The HTTP upgrade token.
pub const UPGRADE: &str = "keepane-stream";
/// The path asked for.
pub const PATH: &str = "/link/stream";
/// A Noise message at most (the spec's limit), and what it can carry.
const NOISE_MAX: usize = 65535;
const TAG: usize = 16;
const PAYLOAD_MAX: usize = NOISE_MAX - TAG;
/// The biggest message a stream takes: a snapshot of many panes with their
/// scrollback, deflated, fits.
pub const MESSAGE_MAX: usize = 64 << 20;

/// `public` (an Ed25519 link key, as the table holds it) as X25519.
pub fn x25519_public(public: &str) -> Option<[u8; 32]> {
    let bytes: [u8; 32] = crate::link::unb64(public)?.try_into().ok()?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()?;
    Some(key.to_montgomery().to_bytes())
}

fn builder() -> snow::Builder<'static> {
    snow::Builder::new(NOISE.parse().expect("a known Noise pattern")).prologue(PROLOGUE)
}

/// The responder's side: the other machine's first message read, what it
/// says (its X25519 key, its `Hello`), and the state to answer from.
pub struct Opening {
    pub remote: [u8; 32],
    pub hello: crate::link_proto::Hello,
    state: snow::HandshakeState,
}

/// Read the first handshake message as the machine copied.
pub fn open(secret: &[u8; 32], msg1: &[u8]) -> Result<Opening> {
    let mut state = builder().local_private_key(secret).build_responder().context("noise")?;
    let mut payload = vec![0u8; NOISE_MAX];
    let n = state.read_message(msg1, &mut payload).context("the handshake does not check out")?;
    let hello = crate::link_proto::Hello::decode(&payload[..n]).context("the handshake's hello")?;
    let remote: [u8; 32] =
        state.get_remote_static().and_then(|k| k.try_into().ok()).context("the handshake names no key")?;
    Ok(Opening { remote, hello, state })
}

impl Opening {
    /// The second handshake message, carrying `answer`; and the
    /// connection's keys when it is a welcome.
    pub fn answer(mut self, answer: &crate::link_proto::Answer) -> Result<(Vec<u8>, Option<snow::TransportState>)> {
        let mut msg2 = vec![0u8; NOISE_MAX];
        let n = self.state.write_message(&answer.encode_to_vec(), &mut msg2).context("noise")?;
        msg2.truncate(n);
        let keys = if answer.refused.is_none() { Some(self.state.into_transport_mode()?) } else { None };
        Ok((msg2, keys))
    }
}

/// The initiator's side, before the answer.
pub struct Calling {
    state: snow::HandshakeState,
}

/// The first handshake message, as the machine that copies: `secret` its
/// own X25519 key, `remote` the other's.
pub fn call(secret: &[u8; 32], remote: &[u8; 32], hello: &crate::link_proto::Hello) -> Result<(Calling, Vec<u8>)> {
    let mut state = builder().local_private_key(secret).remote_public_key(remote).build_initiator().context("noise")?;
    let mut msg1 = vec![0u8; NOISE_MAX];
    let n = state.write_message(&hello.encode_to_vec(), &mut msg1).context("noise")?;
    msg1.truncate(n);
    Ok((Calling { state }, msg1))
}

impl Calling {
    /// The answer read: what it says, and the connection's keys.
    pub fn read(mut self, msg2: &[u8]) -> Result<(crate::link_proto::Answer, snow::TransportState)> {
        let mut payload = vec![0u8; NOISE_MAX];
        let n = self
            .state
            .read_message(msg2, &mut payload)
            .context("the answer does not check out (not the key paired with that machine?)")?;
        let answer = crate::link_proto::Answer::decode(&payload[..n]).context("the answer")?;
        Ok((answer, self.state.into_transport_mode()?))
    }
}

/// One frame: its length, then it.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, frame: &[u8]) -> Result<()> {
    let len = u16::try_from(frame.len()).context("a frame over 65535 bytes")?;
    let mut buf = Vec::with_capacity(2 + frame.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(frame);
    w.write_all(&buf).await?;
    Ok(())
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).await?;
    let mut frame = vec![0u8; usize::from(u16::from_be_bytes(len))];
    r.read_exact(&mut frame).await?;
    Ok(frame)
}

/// The encrypted side of a connection: messages out, in frames.
pub struct Sealer {
    keys: std::sync::Arc<std::sync::Mutex<snow::TransportState>>,
}

/// The decrypting side: frames in, messages out.
pub struct Opener {
    keys: std::sync::Arc<std::sync::Mutex<snow::TransportState>>,
    /// What came in, decrypted, not yet a whole message.
    inbox: Vec<u8>,
}

/// The connection's keys, split to write on one task and read on another
/// (Noise keeps a nonce each way, so the two never wait on each other's
/// keys for long).
pub fn split(keys: snow::TransportState) -> (Sealer, Opener) {
    let keys = std::sync::Arc::new(std::sync::Mutex::new(keys));
    (Sealer { keys: keys.clone() }, Opener { keys, inbox: Vec::new() })
}

impl Sealer {
    /// `msg`, encrypted, as the frames to write.
    pub fn seal(&self, msg: &impl Message) -> Result<Vec<Vec<u8>>> {
        let plain = msg.encode_length_delimited_to_vec();
        let mut keys = self.keys.lock().map_err(|_| anyhow::anyhow!("keys poisoned"))?;
        let mut frames = Vec::with_capacity(plain.len() / PAYLOAD_MAX + 1);
        for chunk in plain.chunks(PAYLOAD_MAX) {
            let mut out = vec![0u8; chunk.len() + TAG];
            let n = keys.write_message(chunk, &mut out).context("noise")?;
            out.truncate(n);
            frames.push(out);
        }
        Ok(frames)
    }

    pub async fn send<W: AsyncWriteExt + Unpin>(&self, w: &mut W, msg: &impl Message) -> Result<usize> {
        let frames = self.seal(msg)?;
        let mut sent = 0;
        for f in &frames {
            write_frame(w, f).await?;
            sent += f.len() + 2;
        }
        Ok(sent)
    }
}

impl Opener {
    /// Take one frame in; the whole messages it completes.
    pub fn open<M: Message + Default>(&mut self, frame: &[u8]) -> Result<Vec<M>> {
        let mut plain = vec![0u8; frame.len()];
        let n = {
            let mut keys = self.keys.lock().map_err(|_| anyhow::anyhow!("keys poisoned"))?;
            keys.read_message(frame, &mut plain).context("a frame that does not check out")?
        };
        self.inbox.extend_from_slice(&plain[..n]);
        let mut out = Vec::new();
        loop {
            let mut at = &self.inbox[..];
            let Ok(len) = prost::encoding::decode_varint(&mut at) else {
                // Not even the length yet: wait, unless it can never be one.
                if self.inbox.len() > 10 {
                    bail!("a message length that is not one");
                }
                break;
            };
            let head = self.inbox.len() - at.len();
            let len = usize::try_from(len).unwrap_or(usize::MAX);
            if len > MESSAGE_MAX {
                bail!("a message of {len} bytes, more than a stream takes");
            }
            if at.len() < len {
                break;
            }
            let msg = M::decode(&at[..len]).context("a message that is not one")?;
            self.inbox.drain(..head + len);
            out.push(msg);
        }
        Ok(out)
    }
}

/// The request that opens a stream, as written to the other machine.
pub fn request(host: &str) -> String {
    format!("GET {PATH} HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE}\r\n\r\n")
}

/// The answer that takes it.
pub const SWITCHING: &str =
    "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: keepane-stream\r\n\r\n";

/// An HTTP answer's head, read: its status, the keepane version it names
/// (`X-Keepane-Version`), and, for one that is not 101, its body as far as
/// it came (a reason).
pub struct AnswerHead {
    pub status: u16,
    pub version: Option<String>,
    pub body: String,
}

impl AnswerHead {
    /// Why a stream was not taken, said plainly: a keepane from before
    /// streams (0.59 and older) answers 404, or 400 for a link request it
    /// does not know.
    pub fn refusal(&self) -> String {
        let before_streams = |v: &str| crate::takeover::parse_version(v).is_some_and(|v| v < (0, 60, 0));
        let older = self.status == 404 || self.version.as_deref().is_some_and(before_streams);
        if older {
            let which = self.version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default();
            return format!("keepane there{which} is older than 0.60: update it there to copy its sessions");
        }
        format!("{}: {}", self.status, self.body)
    }
}

/// Read an HTTP answer's head (up to the blank line) from `r`.
pub async fn read_answer_head<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<AnswerHead> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            bail!("an answer head over 16 KiB");
        }
        if r.read(&mut byte).await? == 0 {
            bail!("the connection closed before an answer");
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).context("not an HTTP answer")?;
    let version = text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("x-keepane-version").then(|| v.trim().to_string())
    });
    if status == 101 {
        return Ok(AnswerHead { status, version, body: String::new() });
    }
    let length: usize = text
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .min(16 * 1024);
    let mut body = vec![0u8; length];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), r.read_exact(&mut body)).await;
    Ok(AnswerHead { status, version, body: String::from_utf8_lossy(&body).trim().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_proto::{Answer, Hello, Ping, ToClient, to_client};

    #[test]
    fn a_keepane_from_before_streams_is_named_as_such() {
        let head = |status, version: Option<&str>| AnswerHead {
            status,
            version: version.map(String::from),
            body: "why".into(),
        };
        assert!(head(404, None).refusal().contains("older than 0.60"));
        // 0.56 to 0.59 take /link/ requests and refuse this one (400).
        let r = head(400, Some("0.58.0")).refusal();
        assert!(r.contains("(0.58.0) is older than 0.60"), "{r}");
        assert_eq!(
            head(403, Some("0.60.0")).refusal(),
            "403: why",
            "a refusal of a keepane with streams says its own reason"
        );
        assert_eq!(head(500, None).refusal(), "500: why");
    }

    fn key(n: u8) -> crate::link::Identity {
        crate::link::Identity::from_seed([n; 32])
    }

    #[test]
    fn two_machines_agree_on_keys_and_each_learns_who_the_other_is() {
        let (a, b) = (key(1), key(2));
        let hello = Hello { since: Some(41), version: "1.0".into(), time: 0, epoch: Some(3) };
        let (calling, msg1) = call(&b.x25519_secret(), &x25519_public(&a.public()).unwrap(), &hello).unwrap();
        // The hello is not readable on the wire.
        assert!(!msg1.windows(3).any(|w| w == b"1.0"));
        let opening = open(&a.x25519_secret(), &msg1).unwrap();
        assert_eq!(opening.remote, x25519_public(&b.public()).unwrap(), "who called");
        assert_eq!(opening.hello, hello);
        let (msg2, keys_a) =
            opening.answer(&Answer { version: "2.0".into(), refused: None, epoch: 7, commands: true }).unwrap();
        let (answer, keys_b) = calling.read(&msg2).unwrap();
        assert_eq!(answer.version, "2.0");
        // Messages both ways, one bigger than a frame.
        let (seal_a, _) = split(keys_a.unwrap());
        let (_, mut open_b) = split(keys_b);
        let big =
            ToClient { kind: Some(to_client::Kind::Entry(crate::link_proto::Entry { seq: 1, at: 2, change: None })) };
        let mut msgs = Vec::new();
        let ping = ToClient { kind: Some(to_client::Kind::Ping(Ping {})) };
        for m in [&ping, &big, &ping] {
            for f in seal_a.seal(m).unwrap() {
                msgs.extend(open_b.open::<ToClient>(&f).unwrap());
            }
        }
        assert_eq!(msgs, [ping.clone(), big, ping]);
    }

    #[test]
    fn a_message_bigger_than_a_frame_spans_frames() {
        let (a, b) = (key(3), key(4));
        let (calling, msg1) =
            call(&b.x25519_secret(), &x25519_public(&a.public()).unwrap(), &Hello::default()).unwrap();
        let (msg2, keys_a) = open(&a.x25519_secret(), &msg1).unwrap().answer(&Answer::default()).unwrap();
        let (_, keys_b) = calling.read(&msg2).unwrap();
        let (seal, _) = split(keys_a.unwrap());
        let (_, mut opener) = split(keys_b);
        let pane = crate::link_proto::PaneChange {
            pane: 1,
            kind: Some(crate::link_proto::pane_change::Kind::Output(crate::link_proto::Output {
                at: 0,
                bytes: vec![7u8; 200_000],
            })),
        };
        let msg =
            crate::link_proto::Entry { seq: 9, at: 0, change: Some(crate::link_proto::entry::Change::Pane(pane)) };
        let frames = seal.seal(&msg).unwrap();
        assert!(frames.len() >= 4, "{}", frames.len());
        let mut got = Vec::new();
        for f in &frames {
            got.extend(opener.open::<crate::link_proto::Entry>(f).unwrap());
        }
        assert_eq!(got, [msg]);
    }

    #[test]
    fn the_wrong_key_or_a_changed_byte_is_refused() {
        let (a, b, c) = (key(5), key(6), key(7));
        // Called with c's key for a: a cannot read it.
        let (_, msg1) = call(&b.x25519_secret(), &x25519_public(&c.public()).unwrap(), &Hello::default()).unwrap();
        assert!(open(&a.x25519_secret(), &msg1).is_err());
        // A byte changed in the first message.
        let (_, mut msg1) = call(&b.x25519_secret(), &x25519_public(&a.public()).unwrap(), &Hello::default()).unwrap();
        let last = msg1.len() - 1;
        msg1[last] ^= 1;
        assert!(open(&a.x25519_secret(), &msg1).is_err());
        // A byte changed in a frame after.
        let (calling, msg1) =
            call(&b.x25519_secret(), &x25519_public(&a.public()).unwrap(), &Hello::default()).unwrap();
        let (msg2, keys_a) = open(&a.x25519_secret(), &msg1).unwrap().answer(&Answer::default()).unwrap();
        let (_, keys_b) = calling.read(&msg2).unwrap();
        let (seal, _) = split(keys_a.unwrap());
        let (_, mut opener) = split(keys_b);
        let mut f = seal.seal(&Ping {}).unwrap().remove(0);
        f[0] ^= 1;
        assert!(opener.open::<Ping>(&f).is_err());
        // An answer from another machine than the one called.
        let (calling, msg1) =
            call(&b.x25519_secret(), &x25519_public(&a.public()).unwrap(), &Hello::default()).unwrap();
        let _ = open(&a.x25519_secret(), &msg1).unwrap();
        let (_, msg1c) = call(&b.x25519_secret(), &x25519_public(&c.public()).unwrap(), &Hello::default()).unwrap();
        let (msg2c, _) = open(&c.x25519_secret(), &msg1c).unwrap().answer(&Answer::default()).unwrap();
        assert!(calling.read(&msg2c).is_err());
    }
}
