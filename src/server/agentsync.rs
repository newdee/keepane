//! `send-agents` (docs/design/agent-sync.md): a project and its agents'
//! chats sent to a paired machine, what moves and how it lands. No server
//! state: the transport is `link_agents`, this is what both ends do with
//! files.
//!
//! What goes: for a repository, the commits of its branch (a git bundle,
//! as `push` makes), the files that differ from them (changed or new, not
//! ignored) and the chats; for any other folder, its files. Chats are the
//! transcripts Claude Code, Codex and pi keep for the folder, their `cwd`
//! rewritten to the folder there; a chat the other side has the start of
//! goes as its new end only.
//!
//! Everything for one send is one gzip stream (a "pack"): a manifest, the
//! bundle, the files, the chats, each a header and its bytes.
//!
//! Each side keeps a record of each folder it sent or took in, with the
//! other machine (`Record`): where it is there, and the files' digests as
//! they were after the last send, which say which side changed a file
//! since.

use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The agents whose chats go along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
    Pi,
}

impl Agent {
    pub fn word(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
        }
    }

    /// How to go on with a chat of its in the folder.
    pub fn resume(self, id: &str) -> String {
        match self {
            Agent::Claude => format!("claude --resume {id}"),
            Agent::Codex => format!("codex resume {id}"),
            Agent::Pi => format!("pi --session {id}"),
        }
    }
}

/// Where the agents keep their chats on a machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Places {
    /// Claude Code's directory (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
    pub claude: PathBuf,
    /// Codex's (`CODEX_HOME`, else `~/.codex`).
    pub codex: PathBuf,
    /// pi's sessions (`~/.pi/agent/sessions`).
    pub pi: PathBuf,
}

impl Places {
    pub fn of(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Places {
        let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        Places {
            claude: set("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex: set("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            pi: home.join(".pi").join("agent").join("sessions"),
        }
    }

    /// The directory a chat file of `agent` for the folder `cwd` goes
    /// under; its `rel` is from there.
    pub fn dir(&self, agent: Agent, cwd: &str) -> PathBuf {
        match agent {
            Agent::Claude => self.claude.join("projects").join(super::agents::claude_folder(cwd)),
            Agent::Codex => self.codex.join("sessions"),
            Agent::Pi => self.pi.join(super::agents::pi_folder(cwd)),
        }
    }
}

/// One file of a chat here.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatFile {
    pub agent: Agent,
    /// The chat's id (a UUID), as its files are named.
    pub id: String,
    /// Its path under `Places::dir`, `/` between its parts.
    pub rel: String,
    pub path: PathBuf,
    /// A transcript (lines of JSON whose `cwd` is rewritten), not a file
    /// kept beside one (a tool's output).
    pub transcript: bool,
}

/// The first UUID in a name (`rollout-2026-…-<uuid>.jsonl`, `<uuid>.jsonl`).
pub fn uuid_in(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    (0..b.len().saturating_sub(35)).find_map(|i| {
        let s = &name.get(i..i + 36)?;
        let ok = s.bytes().enumerate().all(|(j, c)| match j {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
        ok.then_some(*s)
    })
}

/// The entries of a directory, and whether each is a directory.
fn entries(dir: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, bool)> = rd
        .flatten()
        .filter_map(|e| Some((e.path(), e.file_type().ok().filter(|t| !t.is_symlink())?.is_dir())))
        .collect();
    out.sort();
    out
}

/// Every file under `dir`, with its path from there (`/` between).
fn files_under(dir: &Path, rel: &str, out: &mut Vec<(PathBuf, String)>) {
    for (p, is_dir) in entries(dir) {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if is_dir {
            files_under(&p, &r, out);
        } else {
            out.push((p, r));
        }
    }
}

/// The directory named `name` in `parent`, Windows' way (any case) when
/// that is how this machine reads names.
fn child_named(parent: &Path, name: &str) -> Option<PathBuf> {
    let exact = parent.join(name);
    if exact.is_dir() {
        return Some(exact);
    }
    if !cfg!(windows) {
        return None;
    }
    entries(parent)
        .into_iter()
        .find(|(p, d)| *d && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
        .map(|(p, _)| p)
}

/// The chats of the folder `cwd` here: Claude Code's sessions begun in it
/// (each with the files beside it: its subagents, its tools' output),
/// Codex's whose first line names it, pi's.
pub fn chats_of(cwd: &str, places: &Places) -> Vec<ChatFile> {
    let mut out = Vec::new();
    if let Some(dir) = child_named(&places.claude.join("projects"), &super::agents::claude_folder(cwd)) {
        for (p, is_dir) in entries(&dir) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if is_dir || !name.ends_with(".jsonl") {
                continue;
            }
            let Some(id) = uuid_in(&name).filter(|id| name == format!("{id}.jsonl")).map(str::to_string) else {
                continue;
            };
            out.push(ChatFile { agent: Agent::Claude, id: id.clone(), rel: name, path: p, transcript: true });
            let mut beside = Vec::new();
            files_under(&dir.join(&id), &id, &mut beside);
            for (p, rel) in beside {
                let transcript = rel.ends_with(".jsonl");
                out.push(ChatFile { agent: Agent::Claude, id: id.clone(), rel, path: p, transcript });
            }
        }
    }
    let want = super::agents::dir_key(cwd);
    let mut codex = Vec::new();
    files_under(&places.codex.join("sessions"), "", &mut codex);
    for (p, rel) in codex {
        if !rel.ends_with(".jsonl") || super::agents::codex_cwd(&p).is_none_or(|c| super::agents::dir_key(&c) != want) {
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or_default();
        let Some(id) = uuid_in(name).map(str::to_string) else { continue };
        out.push(ChatFile { agent: Agent::Codex, id, rel, path: p, transcript: true });
    }
    if let Some(dir) = child_named(&places.pi, &super::agents::pi_folder(cwd)) {
        for (p, is_dir) in entries(&dir) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if is_dir || !name.ends_with(".jsonl") {
                continue;
            }
            let id = uuid_in(&name).map_or_else(|| name.trim_end_matches(".jsonl").to_string(), str::to_string);
            out.push(ChatFile { agent: Agent::Pi, id, rel: name, path: p, transcript: true });
        }
    }
    out
}

/// A Windows path (a drive or a backslash), as opposed to a Unix one.
fn windows_path(p: &str) -> bool {
    p.contains('\\') || (p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic())
}

/// A path under `from` (the folder here) as the same place under `to`
/// (the folder there), in that machine's separators; None for one not
/// under it. A Windows `from` matches in any case.
pub fn moved(p: &str, from: &str, to: &str) -> Option<String> {
    let from = from.trim_end_matches(['/', '\\']);
    let rest = if windows_path(from) {
        let head = p.get(..from.len())?;
        let same = head.replace('/', "\\").eq_ignore_ascii_case(&from.replace('/', "\\"));
        same.then(|| &p[from.len()..])?
    } else {
        p.strip_prefix(from)?
    };
    if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
        return None;
    }
    let rest = rest.trim_end_matches(['/', '\\']);
    let sep = if windows_path(to) { '\\' } else { '/' };
    let rest: String = rest.chars().map(|c| if c == '/' || c == '\\' { sep } else { c }).collect();
    Some(format!("{}{rest}", to.trim_end_matches(['/', '\\'])))
}

/// A transcript line with each `"cwd":"…"` under `from` moved under `to`;
/// everything else as it was, byte for byte.
pub fn rewrite_line(line: &str, from: &str, to: &str) -> String {
    const KEY: &str = "\"cwd\":\"";
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find(KEY) {
        let (before, after) = rest.split_at(i + KEY.len());
        out.push_str(&before[..before.len() - 1]);
        // The string's end: the first quote not escaped.
        let bytes = after.as_bytes();
        let mut j = 0;
        while j < bytes.len() && bytes[j] != b'"' {
            j += if bytes[j] == b'\\' { 2 } else { 1 };
        }
        if j >= bytes.len() {
            out.push('"');
            out.push_str(after);
            return out;
        }
        let raw = &after[..j];
        let new = serde_json::from_str::<String>(&format!("\"{raw}\""))
            .ok()
            .and_then(|p| moved(&p, from, to))
            .and_then(|p| serde_json::to_string(&p).ok());
        match new {
            Some(quoted) => out.push_str(&quoted),
            None => {
                out.push('"');
                out.push_str(raw);
                out.push('"');
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// A chat file's bytes as they are to be there: complete lines only (one
/// being written stops at its last line break), each transcript line's
/// `cwd` moved; other files as they are.
pub fn as_there(f: &ChatFile, from: &str, to: &str) -> std::io::Result<Vec<u8>> {
    let bytes = std::fs::read(&f.path)?;
    if !f.transcript {
        return Ok(bytes);
    }
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let text = String::from_utf8_lossy(&bytes[..end]);
    let mut out = Vec::with_capacity(end);
    for line in text.split_inclusive('\n') {
        let (body, nl) = line.strip_suffix('\n').map_or((line, ""), |b| (b, "\n"));
        out.extend_from_slice(rewrite_line(body, from, to).as_bytes());
        out.extend_from_slice(nl.as_bytes());
    }
    Ok(out)
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(p: &Path) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut d = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => d.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(d.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What the other side says it has of a chat file: its length and the
/// digest of all of it, its own id put back to the one sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Have {
    pub len: u64,
    pub sha: String,
    /// The id it goes by there, when not the one sent (a fork's).
    pub id: String,
}

/// How a chat file goes, given what the other side has of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    /// It has all of it.
    Nothing,
    /// It has none: all of it.
    Whole,
    /// It has the start: the rest, from this byte.
    From(u64),
    /// It has something else (it went on there): all of it, as a new chat.
    Fork,
}

pub fn how_to_send(mine: &[u8], have: &Have) -> Send {
    let len = have.len as usize;
    if have.len == 0 {
        return Send::Whole;
    }
    if len > mine.len() || sha256(&mine[..len]) != have.sha {
        return Send::Fork;
    }
    if len == mine.len() { Send::Nothing } else { Send::From(have.len) }
}

/// The ways a file that came is put down beside what is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Put {
    /// Written over what is there (nothing, or what was there unchanged
    /// since the last send).
    Write,
    /// The same is there already.
    Keep,
    /// Changed on both sides: put beside it as `<name>.from-<machine>`.
    Aside,
}

/// `local`: the file's digest here (None: no such file); `base`: as it was
/// after the last send; `incoming`: the one that came; `dirty`: with no
/// last send, whether the file here is a change of this side's own (not
/// as its repository has it).
pub fn decide(local: Option<&str>, base: Option<&str>, incoming: &str, dirty: bool) -> Put {
    match (local, base) {
        (None, _) => Put::Write,
        (Some(l), _) if l == incoming => Put::Keep,
        (Some(l), Some(b)) if l == b => Put::Write,
        (Some(_), Some(_)) => Put::Aside,
        (Some(_), None) if dirty => Put::Aside,
        (Some(_), None) => Put::Write,
    }
}

/// A path that came, as one under a folder here: parts between `/`, none
/// empty, `.` or `..`, none a drive or holding what Windows will not have
/// in a name, nothing inside `.git`.
pub fn safe_rel(rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel.split('/').collect();
    let bad = |p: &str| {
        p.is_empty()
            || p == "."
            || p == ".."
            || p.contains(['\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
            || p.ends_with(['.', ' '])
    };
    if parts.iter().any(|p| bad(p)) || parts[0].eq_ignore_ascii_case(".git") {
        return None;
    }
    Some(parts.iter().collect())
}

/// One part of a pack: what it is, and where its bytes go.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// `manifest`, `bundle`, `file` or `chat`.
    pub t: String,
    /// A file: its path in the folder.
    pub path: String,
    pub sha: String,
    pub agent: Option<Agent>,
    pub id: String,
    pub rel: String,
    /// A chat file: where its bytes go on (0: a whole file).
    pub at: u64,
    /// A chat that went on there too: it lands as a new chat.
    pub fork: bool,
}

/// The files of the folder that go, as listed in a pack's manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// The branch and its commit here (a repository; empty otherwise).
    pub branch: String,
    pub head: String,
    /// Every file that goes (changed or new in a repository; all of them
    /// otherwise): its path and digest. A file the other side has already
    /// is listed without coming.
    pub files: Vec<(String, String)>,
}

const MAGIC: &[u8] = b"keepane-pack 1\n";
/// A header longer than this is not one.
const HEADER_MAX: u32 = 1 << 20;

/// A pack being written.
pub struct PackWriter {
    gz: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
}

impl PackWriter {
    pub fn create(path: &Path) -> std::io::Result<PackWriter> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let f = BufWriter::new(std::fs::File::create(path)?);
        let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        gz.write_all(MAGIC)?;
        Ok(PackWriter { gz })
    }

    pub fn add(&mut self, e: &Entry, data: &[u8]) -> std::io::Result<()> {
        let head = serde_json::to_vec(e).map_err(std::io::Error::other)?;
        self.gz.write_all(&(head.len() as u32).to_le_bytes())?;
        self.gz.write_all(&head)?;
        self.gz.write_all(&(data.len() as u64).to_le_bytes())?;
        self.gz.write_all(data)
    }

    /// A part whose bytes are a file's, read as they are written.
    pub fn add_file(&mut self, e: &Entry, path: &Path) -> std::io::Result<()> {
        let head = serde_json::to_vec(e).map_err(std::io::Error::other)?;
        let len = std::fs::metadata(path)?.len();
        self.gz.write_all(&(head.len() as u32).to_le_bytes())?;
        self.gz.write_all(&head)?;
        self.gz.write_all(&len.to_le_bytes())?;
        let copied = std::io::copy(&mut std::fs::File::open(path)?.take(len), &mut self.gz)?;
        if copied != len {
            return Err(std::io::Error::other(format!("{} changed while it was read", path.display())));
        }
        Ok(())
    }

    /// Done: the pack's size.
    pub fn finish(self) -> std::io::Result<u64> {
        let mut f = self.gz.finish()?;
        f.flush()?;
        f.get_ref().metadata().map(|m| m.len())
    }
}

/// A pack being read: each part's header, then its bytes (read them
/// all, or `skip` them, before the next).
pub struct PackReader {
    gz: flate2::read::GzDecoder<BufReader<std::fs::File>>,
    left: u64,
}

impl PackReader {
    pub fn open(path: &Path) -> Result<PackReader, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("the pack: {e}"))?;
        let mut gz = flate2::read::GzDecoder::new(BufReader::new(f));
        let mut magic = vec![0u8; MAGIC.len()];
        gz.read_exact(&mut magic).map_err(|_| "not a keepane pack".to_string())?;
        if magic != MAGIC {
            return Err("not a keepane pack (or of another version)".into());
        }
        Ok(PackReader { gz, left: 0 })
    }

    /// The next part's header and the length of its bytes; None at the end.
    pub fn next(&mut self) -> Result<Option<(Entry, u64)>, String> {
        self.skip()?;
        let mut n = [0u8; 4];
        match self.gz.read_exact(&mut n) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(format!("the pack: {e}")),
        }
        let n = u32::from_le_bytes(n);
        if n > HEADER_MAX {
            return Err("the pack is broken (a header too long)".into());
        }
        let mut head = vec![0u8; n as usize];
        let mut len = [0u8; 8];
        self.gz
            .read_exact(&mut head)
            .and_then(|_| self.gz.read_exact(&mut len))
            .map_err(|e| format!("the pack: {e}"))?;
        let e: Entry = serde_json::from_slice(&head).map_err(|_| "the pack is broken (a header)".to_string())?;
        self.left = u64::from_le_bytes(len);
        Ok(Some((e, self.left)))
    }

    /// The part's bytes, all of them.
    pub fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(self.left.min(1 << 26) as usize);
        (&mut self.gz).take(self.left).read_to_end(&mut out).map_err(|e| format!("the pack: {e}"))?;
        if out.len() as u64 != self.left {
            return Err("the pack ends short".into());
        }
        self.left = 0;
        Ok(out)
    }

    /// The part's bytes, into a file.
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        let mut f = BufWriter::new(std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?);
        let n = std::io::copy(&mut (&mut self.gz).take(self.left), &mut f).map_err(|e| format!("the pack: {e}"))?;
        f.flush().map_err(|e| format!("{}: {e}", path.display()))?;
        if n != self.left {
            return Err("the pack ends short".into());
        }
        self.left = 0;
        Ok(())
    }

    pub fn skip(&mut self) -> Result<(), String> {
        if self.left > 0 {
            std::io::copy(&mut (&mut self.gz).take(self.left), &mut std::io::sink())
                .map_err(|e| format!("the pack: {e}"))?;
            self.left = 0;
        }
        Ok(())
    }
}

/// What a machine keeps of a folder it sent or took in, with another
/// machine: where it is there, and each file's digest after the last
/// send (both sides had it so then). `forks`: the chats that went on on
/// both sides, by the id sent, the id they took here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The folder here.
    pub here: String,
    /// The other machine's key.
    pub key: String,
    /// The folder there, as that machine wrote it.
    pub there: String,
    /// That machine's name when this began (the folder `from/<name>`).
    pub host: String,
    pub base: BTreeMap<String, String>,
    pub forks: BTreeMap<String, String>,
    /// When the last send was (local time, RFC 3339).
    pub at: String,
}

const RECORDS: &str = "agents-links.json";

pub fn load_records(dir: &Path) -> Vec<Record> {
    std::fs::read_to_string(dir.join(RECORDS)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Kept: `r` in place of what was there for its folder and machine.
pub fn save_record(dir: &Path, r: Record) -> Result<(), String> {
    let mut all = load_records(dir);
    all.retain(|q| !(q.here == r.here && q.key == r.key));
    all.push(r);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    crate::link::write_private(&dir.join(RECORDS), &(text + "\n")).map_err(|e| format!("{e:#}"))
}

/// A name for a directory: what may not be in one is a `-`.
pub fn dir_name(s: &str) -> String {
    let name: String =
        s.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    let name = name.trim_matches(['.', '-']).to_string();
    if name.is_empty() { "machine".into() } else { name }
}

/// A new chat id (a random UUID, version 4).
pub fn new_id() -> String {
    let mut b = [0u8; 16];
    if crate::platform::random::fill(&mut b).is_err() {
        // A clock is no randomness, but two forks of one chat in the same
        // nanosecond do not happen.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        b = t.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Every `from` in `bytes` made `to` (ids are the same length, so a
/// length taken before stays right).
pub fn swap_id(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    if from == to || from.is_empty() {
        return bytes.to_vec();
    }
    let (f, t) = (from.as_bytes(), to.as_bytes());
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(f) {
            out.extend_from_slice(t);
            i += f.len();
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Each line of a transcript's bytes is JSON.
pub fn lines_are_json(bytes: &[u8]) -> bool {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .all(|l| serde_json::from_slice::<serde::de::IgnoredAny>(l).is_ok())
}

/// What a machine says it has of what is to come (`/link/agents/look`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The folder there.
    pub path: String,
    /// It was there already, from an earlier send (or it is where this
    /// folder came from).
    pub known: bool,
    /// What git says there (a repository being sent).
    pub git: crate::gitsync::Look,
    /// The files (by their place in the list asked about) it needs.
    pub need: Vec<usize>,
    /// What it has of each chat file asked about, in order.
    pub chats: Vec<Have>,
}

/// One file of the folder asked about: its path, digest, size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileAsk {
    pub p: String,
    pub s: String,
    pub n: u64,
}

/// One chat file asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatAsk {
    pub agent: Option<Agent>,
    pub id: String,
    pub rel: String,
}

/// Where a chat file asked about is here, and the id it goes by here (a
/// fork's, or its own).
pub fn chat_target(
    places: &Places,
    folder: &str,
    c: &ChatAsk,
    forks: &BTreeMap<String, String>,
) -> Option<(PathBuf, String)> {
    let agent = c.agent?;
    if uuid_in(&c.id) != Some(c.id.as_str()) && agent != Agent::Pi {
        return None;
    }
    let id = forks.get(&c.id).cloned().unwrap_or_else(|| c.id.clone());
    let rel = safe_rel(&c.rel.replace(&c.id, &id))?;
    // A Codex session's place is by its date: three parts, then the file.
    if agent == Agent::Codex && rel.components().count() != 4 {
        return None;
    }
    Some((places.dir(agent, folder).join(rel), id))
}

/// What this machine has of a chat file asked about: its length, and the
/// digest of it with its own id put back to the one sent.
pub fn have_of(path: &Path, id_here: &str, id_sent: &str) -> Have {
    match std::fs::read(path) {
        Ok(b) => Have {
            len: b.len() as u64,
            sha: sha256(&swap_id(&b, id_here, id_sent)),
            id: if id_here == id_sent { String::new() } else { id_here.to_string() },
        },
        Err(_) => Have::default(),
    }
}

/// Whether `rel` is a file git takes as changed or new in the repository
/// at `root` (`git status` lists it).
pub fn dirty_set(git: &Path, root: &Path) -> std::collections::HashSet<String> {
    let out =
        crate::git::run(git, root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"], crate::git::LIMIT)
            .unwrap_or_default();
    porcelain_paths(&out).into_iter().map(|(_, p)| p).collect()
}

/// `git status --porcelain=v1 -z`: each entry's two-letter state and path
/// (a rename's new path).
pub fn porcelain_paths(out: &[u8]) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut it = out.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(e) = it.next() {
        if e.len() < 4 {
            continue;
        }
        let state = String::from_utf8_lossy(&e[..2]).into_owned();
        let path = String::from_utf8_lossy(&e[3..]).into_owned();
        // A rename or copy: the old path follows; the new one is this.
        if state.starts_with(['R', 'C']) {
            it.next();
        }
        v.push((state, path));
    }
    v
}

/// The folder's files that go, from here: in a repository those git
/// lists as changed or new (not ignored, not deleted); elsewhere every
/// file but what tools make again (`node_modules`, `target`, …).
pub fn files_to_send(git: Option<&Path>, root: &Path, repo: bool) -> Result<Vec<(String, PathBuf)>, String> {
    if repo {
        let git = git.ok_or("no git here (git-command, or git on the PATH)")?;
        let out =
            crate::git::run(git, root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"], crate::git::LIMIT)
                .ok_or("git status failed here")?;
        let mut v: Vec<(String, PathBuf)> = porcelain_paths(&out)
            .into_iter()
            .filter(|(s, _)| !s.contains('D'))
            .map(|(_, p)| (p.clone(), root.join(&p)))
            .filter(|(p, f)| safe_rel(p).is_some() && f.is_file())
            .collect();
        v.sort();
        return Ok(v);
    }
    const MADE: &[&str] = &["node_modules", "target", ".venv", "venv", "__pycache__", ".git", ".DS_Store"];
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
        for (p, is_dir) in entries(dir) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if MADE.contains(&name.as_str()) {
                continue;
            }
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if is_dir {
                walk(&p, &r, out);
            } else if safe_rel(&r).is_some() {
                out.push((r, p));
            }
        }
    }
    let mut v = Vec::new();
    walk(root, "", &mut v);
    Ok(v)
}

/// What this side sends of a folder: the files that go and their
/// digests (in the manifest, in the same order), and the chats.
pub struct Plan {
    /// The folder here, as the other side keeps it (and as the chats'
    /// `cwd` start).
    pub folder: String,
    pub manifest: Manifest,
    pub files: Vec<PathBuf>,
    pub chats: Vec<ChatFile>,
}

/// The folder at `root` and its chats, as they are now; `only`: just the
/// chats whose id starts with one of these.
pub fn plan(
    git: Option<&Path>,
    root: &Path,
    repo: Option<(&str, &str)>,
    places: &Places,
    only: &[String],
) -> Result<Plan, String> {
    let folder = root.to_string_lossy().into_owned();
    let mut manifest = Manifest::default();
    if let Some((branch, head)) = repo {
        (manifest.branch, manifest.head) = (branch.to_string(), head.to_string());
    }
    let mut files = Vec::new();
    for (rel, path) in files_to_send(git, root, repo.is_some())? {
        let Some(sha) = sha256_file(&path) else { continue };
        manifest.files.push((rel, sha));
        files.push(path);
    }
    let mut chats = chats_of(&folder, places);
    if !only.is_empty() {
        chats.retain(|c| only.iter().any(|o| c.id.starts_with(o.as_str())));
        let missing: Vec<&String> =
            only.iter().filter(|o| !chats.iter().any(|c| c.id.starts_with(o.as_str()))).collect();
        if !missing.is_empty() {
            let m: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return Err(format!("no chat of this folder is {}", m.join(", ")));
        }
    }
    Ok(Plan { folder, manifest, files, chats })
}

impl Plan {
    /// What the other side is asked it has.
    pub fn asks(&self) -> (Vec<FileAsk>, Vec<ChatAsk>) {
        let files = self
            .manifest
            .files
            .iter()
            .zip(&self.files)
            .map(|((p, s), f)| FileAsk { p: p.clone(), s: s.clone(), n: std::fs::metadata(f).map_or(0, |m| m.len()) })
            .collect();
        let chats =
            self.chats.iter().map(|c| ChatAsk { agent: Some(c.agent), id: c.id.clone(), rel: c.rel.clone() }).collect();
        (files, chats)
    }
}

/// What a pack carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Made {
    pub size: u64,
    pub files: usize,
    /// The chats that go: new there, grown there, going as a new chat
    /// (they went on there too), and what goes of them, in bytes.
    pub new: Vec<(Agent, String)>,
    pub grown: Vec<(Agent, String)>,
    pub forked: Vec<(Agent, String)>,
    pub chat_bytes: u64,
    pub bundle: bool,
}

impl Made {
    /// Nothing to send at all.
    pub fn nothing(&self) -> bool {
        self.files == 0 && self.new.is_empty() && self.grown.is_empty() && self.forked.is_empty() && !self.bundle
    }
}

/// The pack for `plan` at `out`, given what the other side said it has
/// (`look`), to land in the folder `to` there.
pub fn make_pack(plan: &Plan, look: &Look, bundle: Option<&Path>, to: &str, out: &Path) -> Result<Made, String> {
    if look.chats.len() != plan.chats.len() {
        return Err("the other side answered for other chats than asked".into());
    }
    let io = |e: std::io::Error| format!("the pack: {e}");
    let mut w = PackWriter::create(out).map_err(io)?;
    let mut made = Made::default();
    let m = serde_json::to_vec(&plan.manifest).map_err(|e| e.to_string())?;
    w.add(&Entry { t: "manifest".into(), ..Entry::default() }, &m).map_err(io)?;
    if let Some(b) = bundle {
        w.add_file(&Entry { t: "bundle".into(), ..Entry::default() }, b).map_err(io)?;
        made.bundle = true;
    }
    for &i in &look.need {
        let (Some((p, sha)), Some(f)) = (plan.manifest.files.get(i), plan.files.get(i)) else { continue };
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        if sha256(&bytes) != *sha {
            return Err(format!("{p} changed while it was being sent: send again"));
        }
        w.add(&Entry { t: "file".into(), path: p.clone(), sha: sha.clone(), ..Entry::default() }, &bytes)
            .map_err(io)?;
        made.files += 1;
    }
    // Each chat's files as they are to be there, and how each would go;
    // one that went on there goes whole, every file of it, as a new chat.
    let mut there: Vec<(Vec<u8>, Send)> = Vec::new();
    for (c, have) in plan.chats.iter().zip(&look.chats) {
        let bytes = as_there(c, &plan.folder, to).map_err(|e| format!("{}: {e}", c.path.display()))?;
        let send = how_to_send(&bytes, have);
        there.push((bytes, send));
    }
    let forked: std::collections::HashSet<&str> =
        plan.chats.iter().zip(&there).filter(|(_, (_, s))| *s == Send::Fork).map(|(c, _)| c.id.as_str()).collect();
    for ((c, (bytes, send)), have) in plan.chats.iter().zip(&there).zip(&look.chats) {
        let fork = forked.contains(c.id.as_str());
        let (at, data): (u64, &[u8]) = match (fork, send) {
            (true, _) => (0, bytes),
            (false, Send::Nothing) => continue,
            (false, Send::Whole) => (0, bytes),
            (false, Send::From(n)) => (*n, &bytes[*n as usize..]),
            (false, Send::Fork) => unreachable!("a fork is in `forked`"),
        };
        let e = Entry {
            t: "chat".into(),
            agent: Some(c.agent),
            id: c.id.clone(),
            rel: c.rel.clone(),
            at,
            fork,
            ..Entry::default()
        };
        w.add(&e, data).map_err(io)?;
        made.chat_bytes += data.len() as u64;
        // As it is known there: a fork's id when it went on as one.
        let who = (c.agent, if have.id.is_empty() || fork { c.id.clone() } else { have.id.clone() });
        let list = if fork {
            &mut made.forked
        } else if at > 0 {
            &mut made.grown
        } else {
            &mut made.new
        };
        if c.transcript && !list.contains(&who) {
            list.push(who);
        }
    }
    // A chat both new and grown (a new tool output beside an old one) is grown.
    made.new.retain(|w| !made.grown.contains(w) && !made.forked.contains(w));
    made.grown.retain(|w| !made.forked.contains(w));
    made.size = w.finish().map_err(io)?;
    Ok(made)
}

/// What the receiving side does with a pack, everything it needs.
pub struct Apply {
    pub pack: PathBuf,
    pub sha: String,
    /// The folder here it lands in.
    pub dest: PathBuf,
    /// Where the bundle is unpacked to.
    pub scratch: PathBuf,
    /// The sending machine's name, its remote's, its address for the remote.
    pub host: String,
    pub remote: String,
    pub url: String,
    pub git: Option<PathBuf>,
    pub places: Places,
    /// The folder there (the sender's), for the record.
    pub there: String,
    pub record: Record,
}

/// How it went: said, and the record to keep.
pub struct Applied {
    pub text: String,
    pub record: Record,
}

/// The pack taken in: checked whole, then the commits, the files, the
/// chats. Nothing of a file is ever lost: one changed here since the last
/// send is kept, and the one that came put beside it.
pub fn apply(a: Apply) -> Result<Applied, String> {
    if sha256_file(&a.pack).as_deref() != Some(a.sha.as_str()) {
        return Err("the pack that came is not the one sent (its SHA-256 differs): nothing taken".into());
    }
    let mut r = PackReader::open(&a.pack)?;
    let mut record = a.record;
    let mut said: Vec<String> = Vec::new();
    let dest_s = a.dest.to_string_lossy().into_owned();
    let Some((first, _)) = r.next()? else { return Err("the pack is empty".into()) };
    if first.t != "manifest" {
        return Err("the pack is broken (no manifest first)".into());
    }
    let manifest: Manifest = serde_json::from_slice(&r.bytes()?).map_err(|_| "the pack is broken (its manifest)")?;
    let is_repo = !manifest.branch.is_empty();
    // The files as they are here now, and those this side has not changed
    // since the last send ("ours"): those may be put back to the commit
    // (or taken away) so that the branch can move on, and are written
    // again after.
    let mut ours: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (p, base) in &record.base {
        let Some(rel) = safe_rel(p) else { continue };
        let f = a.dest.join(rel);
        if let Ok(b) = std::fs::read(&f)
            && sha256(&b) == *base
        {
            ours.insert(p.clone(), b);
        }
    }
    let mut dirty = std::collections::HashSet::new();
    let mut overlay = true;
    let mut pending: Option<(Entry, u64)> = r.next()?;
    if is_repo {
        let git = a.git.as_deref().ok_or("there is no git on this machine (git-command)")?;
        if let Some((e, _)) = &pending
            && e.t == "bundle"
        {
            // Commits come: ours go back to the commit first (a tracked one
            // as the commit has it, a new one away), so that the branch can
            // move on over them.
            if crate::gitsync::look(git, &a.dest, &manifest.branch).repo {
                for p in ours.keys() {
                    let at_head = format!("HEAD:{p}");
                    let tracked =
                        crate::gitsync::git(git, &a.dest, &["cat-file", "-e", &at_head], crate::git::LIMIT).is_ok();
                    if tracked {
                        let args = ["checkout", "-q", "HEAD", "--", p.as_str()];
                        let _ = crate::gitsync::git(git, &a.dest, &args, crate::gitsync::WRITE_LIMIT);
                    } else if let Some(rel) = safe_rel(p) {
                        let _ = std::fs::remove_file(a.dest.join(rel));
                    }
                }
            }
            std::fs::create_dir_all(&a.scratch).map_err(|e| format!("{}: {e}", a.scratch.display()))?;
            let bundle = a.scratch.join("commits.bundle");
            r.save_as(&bundle)?;
            let look = crate::gitsync::look(git, &a.dest, &manifest.branch);
            let text = if look.repo {
                let (landed, came) = crate::gitsync::take(git, &bundle, &a.dest, &manifest.branch, &a.remote)?;
                crate::gitsync::said(&dest_s, &manifest.branch, &a.remote, &landed, came)
            } else if look.free {
                crate::gitsync::clone(git, &bundle, &a.dest, &manifest.branch, &a.remote, &a.url)?
            } else {
                let _ = std::fs::remove_file(&bundle);
                return Err(format!("{dest_s} is not empty and not a repository: nothing taken"));
            };
            let _ = std::fs::remove_file(&bundle);
            said.push(text);
            pending = r.next()?;
        }
        // The files go on the commit they were changed from: only when the
        // branch here is at it, and checked out.
        let head = crate::gitsync::git(git, &a.dest, &["rev-parse", "HEAD"], crate::git::LIMIT).unwrap_or_default();
        let current = crate::gitsync::git(git, &a.dest, &["symbolic-ref", "-q", "--short", "HEAD"], crate::git::LIMIT)
            .unwrap_or_default();
        overlay = head.trim() == manifest.head && current.trim() == manifest.branch;
        if !overlay {
            // Ours as they were: nothing here changes but what is put beside.
            for (p, b) in &ours {
                if let Some(rel) = safe_rel(p) {
                    let f = a.dest.join(rel);
                    if std::fs::read(&f).ok().as_deref() != Some(b.as_slice()) {
                        let _ = f.parent().map(std::fs::create_dir_all);
                        let _ = std::fs::write(&f, b);
                    }
                }
            }
            said.push(format!(
                "the branch here is not at {}'s {} {}: the files that came are put beside, as *.from-{}",
                a.host,
                manifest.branch,
                crate::git::short(&manifest.head),
                dir_name(&a.host)
            ));
        }
        dirty = dirty_set(git, &a.dest);
    } else {
        std::fs::create_dir_all(&a.dest).map_err(|e| format!("{dest_s}: {e}"))?;
    }
    // The files.
    let suffix = format!(".from-{}", dir_name(&a.host));
    let (mut wrote, mut kept, mut aside) = (0usize, 0usize, Vec::new());
    let mut came: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    while let Some((e, _)) = &pending {
        if e.t != "file" {
            break;
        }
        let e = e.clone();
        let bytes = r.bytes()?;
        if sha256(&bytes) != e.sha {
            return Err(format!("{}: broken on its way", e.path));
        }
        came.insert(e.path.clone(), bytes);
        pending = r.next()?;
    }
    let mut base = BTreeMap::new();
    for (p, sha) in &manifest.files {
        let Some(rel) = safe_rel(p) else { continue };
        let f = a.dest.join(&rel);
        // What came, or (not sent: the other side saw it was here) ours.
        let bytes = came.remove(p).or_else(|| ours.get(p).filter(|b| sha256(b) == *sha).cloned());
        base.insert(p.clone(), sha.clone());
        // Ours stand as they were at the last send, even put back to the
        // commit for the branch to move: unchanged here.
        let local = match ours.get(p) {
            Some(b) => Some(sha256(b)),
            None => std::fs::read(&f).ok().map(|b| sha256(&b)),
        };
        let Some(bytes) = bytes else {
            // Not sent and not ours: here already as it is there.
            if local.as_deref() == Some(sha.as_str()) {
                kept += 1;
            }
            continue;
        };
        let put = if overlay {
            // Outside a repository nothing says what is this side's own:
            // a file that differs, with no last send, is.
            let own = !is_repo || dirty.contains(p.as_str());
            decide(local.as_deref(), record.base.get(p).map(String::as_str), sha, own)
        } else {
            Put::Aside
        };
        let target = match put {
            Put::Keep => {
                kept += 1;
                // Put back to the commit for the branch to move: as it was.
                if std::fs::read(&f).ok().map(|b| sha256(&b)).as_deref() != Some(sha.as_str()) {
                    if let Some(d) = f.parent() {
                        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                    }
                    std::fs::write(&f, &bytes).map_err(|e| format!("{}: {e}", f.display()))?;
                }
                continue;
            }
            Put::Write => f,
            Put::Aside => {
                let name =
                    format!("{}{suffix}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                aside.push(format!("{p}{suffix}"));
                f.with_file_name(name)
            }
        };
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        if put == Put::Write {
            wrote += 1;
        }
    }
    // Ours that the other side no longer sends (put back to its commit,
    // or deleted there) go the same way here: unchanged here since, so
    // nothing of this side's is lost.
    let listed: std::collections::HashSet<&str> = manifest.files.iter().map(|(p, _)| p.as_str()).collect();
    let mut put_back = 0usize;
    if overlay {
        for p in ours.keys().filter(|p| !listed.contains(p.as_str())) {
            let Some(rel) = safe_rel(p) else { continue };
            let tracked = is_repo
                && a.git.as_deref().is_some_and(|git| {
                    let at_head = format!("HEAD:{p}");
                    crate::gitsync::git(git, &a.dest, &["cat-file", "-e", &at_head], crate::git::LIMIT).is_ok()
                });
            let done = if tracked {
                let args = ["checkout", "-q", "HEAD", "--", p.as_str()];
                a.git
                    .as_deref()
                    .is_some_and(|git| crate::gitsync::git(git, &a.dest, &args, crate::gitsync::WRITE_LIMIT).is_ok())
            } else {
                let f = a.dest.join(rel);
                !f.exists() || std::fs::remove_file(f).is_ok()
            };
            put_back += usize::from(done);
        }
    }
    if !manifest.files.is_empty() || wrote + kept + aside.len() + put_back > 0 {
        let mut s = format!("files: {wrote} written, {kept} already here");
        if put_back > 0 {
            s.push_str(&format!(", {put_back} put back as there"));
        }
        if !aside.is_empty() {
            s.push_str(&format!(
                ", {} changed on both sides (put beside: {})",
                aside.len(),
                aside.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        said.push(s);
    }
    // The chats.
    let folder = dest_s.clone();
    let (mut new_chats, mut grown, mut forked) = (BTreeMap::<String, Agent>::new(), 0usize, Vec::new());
    let mut forks_made: BTreeMap<String, String> = BTreeMap::new();
    while let Some((e, _)) = pending.take() {
        if e.t != "chat" {
            r.skip()?;
            pending = r.next()?;
            continue;
        }
        let bytes = r.bytes()?;
        pending = r.next()?;
        let ask = ChatAsk { agent: e.agent, id: e.id.clone(), rel: e.rel.clone() };
        // A fork takes a new id, the same for every file of its chat.
        let mut forks = record.forks.clone();
        if e.fork {
            let id = forks_made.entry(e.id.clone()).or_insert_with(new_id).clone();
            forks.insert(e.id.clone(), id);
        }
        let Some((path, id)) = chat_target(&a.places, &folder, &ask, &forks) else { continue };
        let bytes = swap_id(&bytes, &e.id, &id);
        let transcript = e.rel.ends_with(".jsonl");
        if transcript && !lines_are_json(&bytes) {
            said.push(format!("{}: not lines of JSON once rewritten; left out", e.rel));
            continue;
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        if e.at > 0 && !e.fork {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len != e.at {
                said.push(format!("{}: changed here while it came; left as it is", e.rel));
                continue;
            }
            let mut f =
                std::fs::OpenOptions::new().append(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            f.write_all(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            grown += 1;
        } else {
            let tmp = path.with_extension("keepane-part");
            std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if e.fork {
            let who = (e.agent.unwrap_or(Agent::Claude), id.clone());
            if !forked.contains(&who) {
                forked.push(who);
            }
        } else if transcript && e.at == 0 {
            new_chats.insert(id.clone(), e.agent.unwrap_or(Agent::Claude));
        }
    }
    for (sent, id) in forks_made {
        record.forks.insert(sent, id);
    }
    if !new_chats.is_empty() || grown > 0 || !forked.is_empty() {
        let mut s = format!("chats: {} new, {grown} grown", new_chats.len());
        if !forked.is_empty() {
            s.push_str(&format!(", {} went on here too (kept; it came as a new chat)", forked.len()));
        }
        said.push(s);
        // The new chats' ids are made here: how to go on with each.
        for (agent, id) in &forked {
            said.push(format!("  new chat: {}", agent.resume(id)));
        }
    }
    record.base = base;
    record.at = chrono::Local::now().to_rfc3339();
    record.here = dest_s;
    record.there = a.there;
    Ok(Applied { text: said.join("\n"), record })
}

#[cfg(test)]
mod tests;
