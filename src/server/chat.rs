//! The group chat (docs/design/chat.md): channels, each with a fixed id, a
//! name, a topic and tags; members, each a pane that joined, with a handle
//! and a role it gave and who and where it is, stamped by the server; and
//! what was said, kept a file a channel a day. What is said stays in the
//! channel until a member reads it (`chat-read`); one named in it
//! (`@handle`) is told at once, by a message into its pane's inbox that
//! carries what was said before it since it last read; a member that
//! joined to hear all (`--hear all`) is told of every line, a few at a
//! time. Members stay members when their pane goes (or the server
//! restarts): offline, until a pane of the same name comes back or one
//! joins with the handle.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::actor::{Message, MsgId, WorkMode};
use super::{ClientId, Outcome, PaneId, Server};
use crate::command::ChatCmd;

/// Lines of a channel kept in memory (and the most one read gives).
pub const KEEP: usize = 1000;
/// What one read gives when it does not say (`-n`).
const READ_DEFAULT: usize = 50;
/// The most a member says in a minute (a person is not counted).
const RATE: usize = 20;
const MEMBERS_MOST: usize = 64;
const CHANNELS_MOST: usize = 64;
const TOPIC_MOST: usize = 200;
const ROLE_MOST: usize = 40;
const TAGS_MOST: usize = 16;
/// The characters of a channel's last line `chat-list -J` gives (`latest`).
const LATEST: usize = 200;
/// The lines said before it that a message to one named carries, and those
/// a message to one that hears all carries, at most.
const CONTEXT_MOST: usize = 20;
/// The characters of each of those lines, at most.
const CONTEXT_LINE_MOST: usize = 300;
/// How long lines gather before one that hears all is told of them.
const HEAR_SETTLE: Duration = Duration::from_secs(2);

/// What a member is told of: the lines that name it (the default), or all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hear {
    #[default]
    Mentions,
    All,
}

impl Hear {
    pub fn word(self) -> &'static str {
        match self {
            Hear::Mentions => "mentions",
            Hear::All => "all",
        }
    }

    pub fn parse(s: &str) -> Option<Hear> {
        match s {
            "mentions" => Some(Hear::Mentions),
            "all" => Some(Hear::All),
            _ => None,
        }
    }
}
/// The channel there always is.
pub const GENERAL: &str = "general";

/// Who and where a member is, as the server knows it (none of it is the
/// member's to say): its machine, its pane's place and address, the pane's
/// name, the agent in it and its model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ident {
    pub machine: String,
    /// Its machine's `host:port` when it is another machine; empty here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub addr: String,
    /// `session:window.pane`, as it was then.
    pub place: String,
    /// `$1:@3.%7`.
    pub address: String,
    pub pane: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Ident {
    /// One line of it: `DFINE work:0.0 %3 claude opus-5-5`.
    pub fn said(&self) -> String {
        let mut s = format!("{} {} %{}", self.machine, self.place, self.pane);
        for part in [&self.agent, &self.model].into_iter().flatten() {
            s.push(' ');
            s.push_str(part);
        }
        s
    }
}

/// Who said something: a member (its handle and role then, and who and
/// where it was), or the person (`you@machine`, no `ident`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Who {
    pub handle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ident: Option<Ident>,
}

/// One line of a channel: something said, or what happened to it (a
/// member came or went, its topic, tags or name changed).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub at: String,
    /// `say`, `new`, `join`, `leave`, `topic`, `tags`, `name`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Who>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Member {
    /// Its pane while it is online; none while it is offline (its pane
    /// gone, or the server started again and no pane is it yet).
    pub pane: Option<PaneId>,
    pub handle: String,
    pub role: String,
    /// Who and where it is, or was last.
    pub ident: Ident,
    pub joined: String,
    /// The last line it read.
    pub read: u64,
    pub hear: Hear,
    /// When it came online, went offline or left last.
    pub since: String,
    /// It left the channel (`chat leave`, or the person took it out): kept,
    /// marked so, until it joins again or is older than `chat-days`; no
    /// pane is it again by itself, and it is not counted or told.
    pub left: bool,
    /// It hears all and lines came since it was last told: when they are
    /// to go to it (`HEAR_SETTLE` after the first).
    pub due: Option<Instant>,
    /// The chain the message telling it goes on: of the speaker of those
    /// lines furthest along one (its task and hop), so the hop limit stops
    /// two that hear all from answering each other for ever.
    pub due_chain: Option<(MsgId, u32)>,
}

/// What `channels.json` keeps of a member: it comes back offline. Read
/// leniently (a member is no reason to lose the channels): what is missing
/// takes its default, a `hear` not known (a later keepane's) is `mentions`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedMember {
    handle: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    ident: Ident,
    #[serde(default)]
    joined: String,
    #[serde(default)]
    read: u64,
    #[serde(default)]
    hear: String,
    #[serde(default)]
    since: String,
    #[serde(default)]
    left: bool,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub topic: String,
    pub tags: Vec<String>,
    pub created: String,
    pub by: Option<Who>,
    pub members: Vec<Member>,
    /// The number of its last line.
    pub last: u64,
    /// Its last lines, `KEEP` at most, oldest first.
    pub recent: VecDeque<Line>,
    /// The last line the person read.
    pub you_read: u64,
}

/// What `channels.json` keeps of a channel: its members too, offline
/// until their panes are back.
#[derive(Serialize, Deserialize)]
struct Saved {
    /// Each read on its own (`SavedMember`): one not readable is left out,
    /// not the whole index.
    #[serde(default)]
    members: Vec<serde_json::Value>,
    id: String,
    name: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    tags: Vec<String>,
    created: String,
    #[serde(default)]
    by: Option<Who>,
    #[serde(default)]
    last: u64,
    #[serde(default)]
    you_read: u64,
}

/// Who reads: a pane (its read position is its membership's), or the person.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reader {
    Pane(PaneId),
    You,
}

/// A `chat-read --wait` not answered yet.
pub struct Wait {
    pub cid: ClientId,
    pub until: Instant,
    pub channel: String,
    pub reader: Reader,
    pub n: usize,
    pub json: bool,
}

#[derive(Default)]
pub struct Chat {
    /// Where its files go; None until the server starts it (tests).
    pub dir: Option<PathBuf>,
    pub channels: Vec<Channel>,
    /// When each pane last said something, for `RATE`.
    said: HashMap<PaneId, VecDeque<Instant>>,
    pub waits: Vec<Wait>,
    pruned: Option<Instant>,
    /// A line added since `channels.json` was written: its `last` is
    /// written at the next tick (the day files have the line itself).
    dirty: bool,
}

fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

/// A UUID of version 7: the time in milliseconds first, so ids sort by when
/// they were made, then random bits.
pub fn uuid_v7() -> String {
    let ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let mut b = [0u8; 16];
    let _ = crate::platform::random::fill(&mut b[6..]);
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A channel's id as a line shows it: its last 8 characters, which are
/// random (its first ones are the time it was made, alike for channels
/// made in the same minute).
pub fn short_id(id: &str) -> &str {
    &id[id.len().saturating_sub(8)..]
}

/// A name, handle or tag: letters, digits, `-` and `_`, 1 to 64 of them.
fn valid_word(s: &str) -> bool {
    (1..=64).contains(&s.chars().count()) && s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// The handles named in a text: `@` and a word.
fn mentions_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split('@').skip(1) {
        let word: String = part.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
        if !word.is_empty() && !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

/// `14:02` of an `at`.
fn clock_of(at: &str) -> &str {
    at.get(11..16).unwrap_or(at)
}

impl Channel {
    fn new(name: String, topic: String, tags: Vec<String>, by: Option<Who>) -> Channel {
        Channel {
            id: uuid_v7(),
            name,
            topic,
            tags,
            created: now(),
            by,
            members: Vec::new(),
            last: 0,
            recent: VecDeque::new(),
            you_read: 0,
        }
    }

    /// The member pane `pane` is (online).
    fn member(&self, pane: PaneId) -> Option<&Member> {
        self.members.iter().find(|m| m.pane == Some(pane))
    }

    fn member_mut(&mut self, pane: PaneId) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.pane == Some(pane))
    }

    /// Lines after `after`, the first `n` of them.
    fn since(&self, after: u64, n: usize) -> Vec<Line> {
        self.recent.iter().filter(|l| l.seq > after).take(n).cloned().collect()
    }

    /// Its last `n` lines.
    fn last_lines(&self, n: usize) -> Vec<Line> {
        let skip = self.recent.len().saturating_sub(n);
        self.recent.iter().skip(skip).cloned().collect()
    }

    fn unread(&self, reader: Reader) -> u64 {
        let read = match reader {
            Reader::You => self.you_read,
            Reader::Pane(p) => match self.member(p) {
                Some(m) => m.read,
                None => return 0,
            },
        };
        // What one said oneself is not unread: the person's lines for the
        // person, a pane's for that pane.
        let you = format!("you@{}", crate::sysinfo::hostname());
        let own = |l: &Line| match (reader, &l.from) {
            (Reader::You, Some(w)) => w.ident.is_none() && w.handle == you,
            (Reader::Pane(p), Some(w)) => w.ident.as_ref().is_some_and(|i| i.pane == p && i.addr.is_empty()),
            _ => false,
        };
        self.recent.iter().filter(|l| l.seq > read && l.kind == "say" && !own(l)).count() as u64
    }

    fn json(&self, unread: u64) -> serde_json::Value {
        // The last thing said, for a list that shows it under the name: who,
        // when, and the start of what (`LATEST` characters at most).
        let latest = self.recent.iter().rev().find(|l| l.kind == "say").map(|l| {
            serde_json::json!({
                "at": l.at,
                "handle": l.from.as_ref().map_or("", |w| w.handle.as_str()),
                "text": l.text.chars().take(LATEST).collect::<String>(),
            })
        });
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "topic": self.topic,
            "tags": self.tags,
            // Those that left are kept, marked, not counted.
            "members": self.members.iter().filter(|m| !m.left).count(),
            "last": self.last,
            "unread": unread,
            "created": self.created,
            "latest": latest,
        })
    }
}

/// A line as one reads it: `#12 14:02 lead (planner) [DFINE work:0.0 %3
/// claude]: text`; what happened, after a `·`.
pub fn line_text(l: &Line) -> String {
    let who = |w: &Who| {
        let role = if w.role.is_empty() { String::new() } else { format!(" ({})", w.role) };
        let at = w.ident.as_ref().map(|i| format!(" [{}]", i.said())).unwrap_or_default();
        format!("{}{role}{at}", w.handle)
    };
    let head = format!("#{} {}", l.seq, clock_of(&l.at));
    match (l.kind.as_str(), &l.from) {
        ("say", Some(w)) => format!("{head} {}: {}", who(w), l.text),
        (_, Some(w)) => format!("{head} · {} {}", who(w), l.text),
        (_, None) => format!("{head} · {}", l.text),
    }
}

impl Server {
    /// The chat's files: `chat/<socket>` beside the event log's.
    fn chat_dir(&self) -> PathBuf {
        crate::histlog::chat_dir().join(crate::histlog::safe_name(&self.socket))
    }

    /// At start: the channels and their last lines, as the files keep
    /// them; `general` when there is none.
    pub(super) fn chat_start(&mut self) {
        let dir = self.chat.dir.clone().unwrap_or_else(|| self.chat_dir());
        self.chat.dir = Some(dir.clone());
        let path = dir.join("channels.json");
        let saved: Vec<Saved> = match std::fs::read_to_string(&path) {
            Err(_) => Vec::new(),
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                // Kept aside, not written over by `general` made next: it
                // names every channel (as `share.json`, `shares_load`).
                let bad = path.with_extension("json.bad");
                let aside = std::fs::rename(&path, &bad).map_or_else(
                    |r| format!("; it could not be moved aside ({r})"),
                    |()| format!("; kept as {}", bad.display()),
                );
                let said = format!("chat: {} is not a list keepane reads ({e}){aside}", path.display());
                log::warn!("{said}");
                self.note_message(&said);
                Vec::new()
            }),
        };
        for s in saved {
            if !valid_word(&s.name) || self.chat.channels.iter().any(|c| c.id == s.id || c.name == s.name) {
                continue;
            }
            let recent = read_back(&dir.join(&s.id), KEEP);
            let last = recent.back().map_or(0, |l| l.seq).max(s.last);
            // Back offline: a pane is them again when it is (`chat_presence`).
            let members = s
                .members
                .into_iter()
                .filter_map(|v| serde_json::from_value::<SavedMember>(v).ok())
                .filter(|m| valid_word(&m.handle))
                .map(|m| Member {
                    pane: None,
                    handle: m.handle,
                    role: m.role,
                    ident: m.ident,
                    joined: m.joined,
                    read: m.read.min(last),
                    hear: Hear::parse(&m.hear).unwrap_or_default(),
                    since: m.since,
                    left: m.left,
                    due: None,
                    due_chain: None,
                })
                .collect();
            self.chat.channels.push(Channel {
                id: s.id,
                name: s.name,
                topic: s.topic,
                tags: s.tags,
                created: s.created,
                by: s.by,
                members,
                last,
                recent,
                you_read: s.you_read.min(last),
            });
        }
        if self.chat.channels.is_empty() {
            let c = Channel::new(GENERAL.into(), String::new(), Vec::new(), None);
            self.chat.channels.push(c);
            let at = self.chat.channels.len() - 1;
            self.chat_line(at, "new", None, format!("{GENERAL} made"), Vec::new());
            self.chat_save();
        }
    }

    /// What waits for the tick written now: the server is going.
    pub(super) fn chat_flush(&mut self) {
        if self.chat.dirty {
            self.chat_save();
        }
    }

    /// `channels.json`, written again whole: when a channel is made,
    /// changed or deleted, read by the person, or a member joins, leaves,
    /// comes online or goes offline (a restart right after must find it);
    /// and at the tick after lines were added or a member read (their
    /// numbers: many and often, and the day files keep the lines anyway).
    fn chat_save(&mut self) {
        self.chat.dirty = false;
        let Some(dir) = &self.chat.dir else { return };
        let saved: Vec<Saved> = self
            .chat
            .channels
            .iter()
            .map(|c| Saved {
                members: c
                    .members
                    .iter()
                    .map(|m| {
                        serde_json::to_value(SavedMember {
                            handle: m.handle.clone(),
                            role: m.role.clone(),
                            ident: m.ident.clone(),
                            joined: m.joined.clone(),
                            read: m.read,
                            hear: m.hear.word().to_string(),
                            since: m.since.clone(),
                            left: m.left,
                        })
                        .expect("it serializes")
                    })
                    .collect(),
                id: c.id.clone(),
                name: c.name.clone(),
                topic: c.topic.clone(),
                tags: c.tags.clone(),
                created: c.created.clone(),
                by: c.by.clone(),
                last: c.last,
                you_read: c.you_read,
            })
            .collect();
        let text = serde_json::to_string_pretty(&saved).expect("it serializes");
        if let Err(e) = write_whole(&dir.join("channels.json"), &text) {
            log::warn!("chat: {e}");
        }
    }

    /// A line added to channel `at`: kept, written to today's file, and
    /// whoever waits for one there told. Its number back.
    fn chat_line(&mut self, at: usize, kind: &str, from: Option<Who>, text: String, mentions: Vec<String>) -> u64 {
        let c = &mut self.chat.channels[at];
        c.last += 1;
        let line = Line { seq: c.last, at: now(), kind: kind.into(), from, text, mentions };
        if let Some(dir) = &self.chat.dir {
            let day = line.at.get(..10).unwrap_or("day").to_string();
            let path = dir.join(&c.id).join(format!("{day}.jsonl"));
            let text = format!("{}\n", serde_json::to_string(&line).expect("it serializes"));
            crate::histlog::append_event(path, text, crate::histlog::DAY_CAP);
        }
        c.recent.push_back(line);
        while c.recent.len() > KEEP {
            c.recent.pop_front();
        }
        let seq = c.last;
        self.chat.dirty = true;
        self.chat_wake();
        seq
    }

    /// The channel a word names: its name (`#` optional), its id, or 8 or
    /// more of its id's last characters (`short_id`; one channel's only).
    /// Not its first ones: an id of version 7 starts with the time, so ids
    /// made in the same minute start alike.
    fn chat_find(&self, word: &str) -> Result<usize, String> {
        let w = word.strip_prefix('#').unwrap_or(word);
        if let Some(i) = self.chat.channels.iter().position(|c| c.name == w || c.id == w) {
            return Ok(i);
        }
        let by_id: Vec<usize> =
            (0..self.chat.channels.len()).filter(|&i| w.len() >= 8 && self.chat.channels[i].id.ends_with(w)).collect();
        match by_id.as_slice() {
            [i] => Ok(*i),
            [] => Err(format!("no channel {word} (`chat-list` lists them)")),
            _ => Err(format!("{word} is the end of more than one channel's id")),
        }
    }

    /// Who and where pane `p` is, now.
    pub(super) fn chat_ident(&self, p: PaneId) -> Ident {
        let stats = self.agents.stats.get(&p);
        Ident {
            machine: crate::sysinfo::hostname(),
            addr: String::new(),
            place: self.place_text(p),
            address: self.address_of(p),
            pane: p,
            pane_name: self.pane_ref(p).and_then(|x| x.actor.name.clone()),
            agent: stats.map(|s| s.kind.word().to_string()),
            model: stats.map(|s| s.usage.model.clone()).filter(|m| !m.is_empty()),
        }
    }

    /// The person, as a line says them.
    fn chat_you() -> Who {
        Who { handle: format!("you@{}", crate::sysinfo::hostname()), role: String::new(), ident: None }
    }

    /// Who is online: a member whose pane is gone goes offline (it stays a
    /// member); an offline one whose pane had a name comes back online when
    /// a pane of that name is here again (a session resumed after a
    /// restart brings the names back) and is no other member of the
    /// channel. Each change said in the channel.
    fn chat_presence(&mut self) {
        let mut changed = false;
        for at in 0..self.chat.channels.len() {
            let gone: Vec<usize> = (0..self.chat.channels[at].members.len())
                .filter(|&i| self.chat.channels[at].members[i].pane.is_some_and(|p| self.place_of(p).is_none()))
                .collect();
            for i in gone {
                let m = &mut self.chat.channels[at].members[i];
                m.pane = None;
                m.since = now();
                m.due = None;
                let who = Who { handle: m.handle.clone(), role: m.role.clone(), ident: Some(m.ident.clone()) };
                self.chat_line(at, "away", Some(who), "went offline (its pane is gone)".into(), Vec::new());
                changed = true;
            }
            for i in 0..self.chat.channels[at].members.len() {
                let m = &self.chat.channels[at].members[i];
                // One that left chose to: no pane is it again by itself.
                let Some(name) = m.ident.pane_name.clone().filter(|_| m.pane.is_none() && !m.left) else { continue };
                let Some(p) = self.pane_by_name(&name) else { continue };
                if self.chat.channels[at].member(p).is_some() {
                    continue;
                }
                let ident = self.chat_ident(p);
                let m = &mut self.chat.channels[at].members[i];
                m.pane = Some(p);
                m.ident = ident.clone();
                m.since = now();
                let who = Who { handle: m.handle.clone(), role: m.role.clone(), ident: Some(ident) };
                self.chat_line(at, "back", Some(who), format!("is back (pane {name})"), Vec::new());
                changed = true;
            }
        }
        // Written at once, as a join or a leave is.
        if changed {
            self.chat_save();
        }
    }

    pub(super) fn exec_chat(&mut self, cmd: ChatCmd, cid: Option<ClientId>) -> Outcome {
        self.chat_presence();
        let me = self.caller_pane(cid);
        match cmd {
            ChatCmd::New { name, topic, tags } => self.chat_new(me, name, topic, tags),
            ChatCmd::List { tags, json } => self.chat_list(me, &tags, json),
            ChatCmd::Join { channel, handle, role, hear } => self.chat_join(me, &channel, handle, role, hear),
            ChatCmd::Leave { channel, handle } => self.chat_leave(me, &channel, handle),
            ChatCmd::Say { channel, text } => self.chat_say(cid, me, &channel, text),
            ChatCmd::Read { channel, n, all, peek, wait, json } => {
                self.chat_read(cid, me, &channel, n, all, peek, wait, json)
            }
            ChatCmd::Who { channel, json } => self.chat_who(&channel, json),
            ChatCmd::Topic { channel, topic } => self.chat_topic(me, &channel, topic),
            ChatCmd::Tag { channel, add, remove } => self.chat_tag(me, &channel, add, remove),
            ChatCmd::Rename { channel, name } => self.chat_rename(me, &channel, name),
            ChatCmd::Delete { channel } => self.chat_delete(me, &channel),
        }
    }

    /// Who did a change: the member (or the pane, joined or not), or the
    /// person.
    fn chat_by(&self, at: usize, me: Option<PaneId>) -> Who {
        match me {
            Some(p) => match self.chat.channels[at].member(p) {
                Some(m) => Who { handle: m.handle.clone(), role: m.role.clone(), ident: Some(self.chat_ident(p)) },
                None => Who { handle: format!("pane-{p}"), role: String::new(), ident: Some(self.chat_ident(p)) },
            },
            None => Self::chat_you(),
        }
    }

    fn chat_new(&mut self, me: Option<PaneId>, name: String, topic: String, tags: Vec<String>) -> Outcome {
        let name = name.strip_prefix('#').unwrap_or(&name).to_string();
        if !valid_word(&name) {
            return Outcome::Error(format!(
                "chat-new: '{name}' is no channel name (letters, digits, - and _, 1 to 64 of them)"
            ));
        }
        if self.chat.channels.iter().any(|c| c.name == name) {
            return Outcome::Error(format!("chat-new: there is a channel {name} already"));
        }
        if self.chat.channels.len() >= CHANNELS_MOST {
            return Outcome::Error(format!("chat-new: {CHANNELS_MOST} channels is the most (chat-delete one)"));
        }
        if let Err(e) = check_topic(&topic).and_then(|_| check_tags(&tags)) {
            return Outcome::Error(format!("chat-new: {e}"));
        }
        let by = match me {
            Some(p) => Who { handle: format!("pane-{p}"), role: String::new(), ident: Some(self.chat_ident(p)) },
            None => Self::chat_you(),
        };
        let mut tags_sorted = tags;
        tags_sorted.sort();
        tags_sorted.dedup();
        self.chat.channels.push(Channel::new(name.clone(), topic, tags_sorted, Some(by.clone())));
        let at = self.chat.channels.len() - 1;
        self.chat_line(at, "new", Some(by), format!("made {name}"), Vec::new());
        self.chat_save();
        Outcome::Text(self.chat.channels[at].id.clone())
    }

    fn chat_list(&mut self, me: Option<PaneId>, tags: &[String], json: bool) -> Outcome {
        let reader = me.map_or(Reader::You, Reader::Pane);
        let shown: Vec<&Channel> =
            self.chat.channels.iter().filter(|c| tags.iter().all(|t| c.tags.contains(t))).collect();
        if json {
            let all: Vec<serde_json::Value> = shown.iter().map(|c| c.json(c.unread(reader))).collect();
            return Outcome::Text(serde_json::Value::Array(all).to_string());
        }
        if shown.is_empty() {
            return Outcome::Text(if tags.is_empty() {
                "no channels".into()
            } else {
                "no channel has those tags".into()
            });
        }
        let rows: Vec<Vec<String>> = shown
            .iter()
            .map(|c| {
                let unread = c.unread(reader);
                vec![
                    format!("#{}", c.name),
                    short_id(&c.id).to_string(),
                    crate::format::count(c.members.iter().filter(|m| !m.left).count(), "member"),
                    if unread > 0 { format!("{unread} unread") } else { String::new() },
                    if c.tags.is_empty() { String::new() } else { format!("[{}]", c.tags.join(",")) },
                    c.topic.clone(),
                ]
            })
            .collect();
        Outcome::Text(super::align_columns(&rows).join("\n"))
    }

    fn chat_join(
        &mut self,
        me: Option<PaneId>,
        word: &str,
        handle: Option<String>,
        role: Option<String>,
        hear: Option<String>,
    ) -> Outcome {
        let hear = match hear.as_deref().map(|h| Hear::parse(h).ok_or(h)) {
            None => None,
            Some(Ok(h)) => Some(h),
            Some(Err(h)) => return Outcome::Error(format!("chat-join: --hear {h}: all or mentions")),
        };
        let Some(p) = me else {
            return Outcome::Error("chat-join: run it inside the pane that joins (a person says with chat-say)".into());
        };
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-join: {e}")),
        };
        if let Some(h) = &handle
            && !valid_word(h)
        {
            return Outcome::Error(format!("chat-join: '{h}' is no handle (letters, digits, - and _, 1 to 64)"));
        }
        if let Some(r) = &role
            && r.chars().count() > ROLE_MOST
        {
            return Outcome::Error(format!("chat-join: a role is {ROLE_MOST} characters at most"));
        }
        // One line, as a topic: it goes into the lines and the @ messages.
        if role.as_ref().is_some_and(|r| r.chars().any(char::is_control)) {
            return Outcome::Error("chat-join: a role is one line".into());
        }
        let ident = self.chat_ident(p);
        let c = &self.chat.channels[at];
        let was = c.member(p).cloned();
        // This pane left under a handle (the same pane, by its id and its
        // name, on this machine): joining again with none given is it again.
        let left_here = c
            .members
            .iter()
            .find(|m| {
                m.left
                    && m.ident.pane == p
                    && m.ident.pane_name == ident.pane_name
                    && m.ident.addr.is_empty()
                    && m.ident.machine == ident.machine
            })
            .map(|m| m.handle.clone());
        let handle = handle.or_else(|| was.as_ref().map(|m| m.handle.clone())).or(left_here).unwrap_or_else(|| {
            // A pane's name is a handle already (`actor::check_name`).
            ident.pane_name.clone().unwrap_or_else(|| match &ident.agent {
                Some(a) => format!("{a}-{p}"),
                None => format!("pane-{p}"),
            })
        });
        if c.members.iter().any(|m| m.handle == handle && m.pane.is_some() && m.pane != Some(p)) {
            return Outcome::Error(format!("chat-join: {handle} is taken in #{} (--as another)", c.name));
        }
        // An offline member of that handle (or one that left): this pane is
        // it now, where it read to, its role and what it hears kept unless
        // given.
        let back = c.members.iter().find(|m| m.handle == handle && m.pane.is_none()).cloned();
        let earlier = was.clone().or_else(|| back.clone());
        if earlier.as_ref().is_none_or(|m| m.left) && c.members.iter().filter(|m| !m.left).count() >= MEMBERS_MOST {
            return Outcome::Error(format!("chat-join: #{} has {MEMBERS_MOST} members, the most", c.name));
        }
        let role = role.or_else(|| earlier.as_ref().map(|m| m.role.clone())).unwrap_or_default();
        let hear = hear.or_else(|| earlier.as_ref().map(|m| m.hear)).unwrap_or_default();
        let read = earlier.as_ref().map_or(c.last, |m| m.read);
        let joined = earlier.as_ref().map_or_else(now, |m| m.joined.clone());
        let name = c.name.clone();
        let c = &mut self.chat.channels[at];
        c.members.retain(|m| m.pane != Some(p) && !(m.handle == handle && m.pane.is_none()));
        c.members.push(Member {
            pane: Some(p),
            handle: handle.clone(),
            role: role.clone(),
            ident: ident.clone(),
            joined,
            read,
            hear,
            since: now(),
            left: false,
            due: None,
            due_chain: None,
        });
        let what = match (&was, &back) {
            (Some(_), _) => "is now",
            (None, Some(m)) if m.left => "joined again",
            (None, Some(_)) => "is back",
            (None, None) => "joined",
        };
        let who = Who { handle: handle.clone(), role: role.clone(), ident: Some(ident.clone()) };
        let seq = self.chat_line(at, "join", Some(who), what.into(), Vec::new());
        // A new member reads from after its own joining on.
        if earlier.is_none()
            && let Some(m) = self.chat.channels[at].member_mut(p)
        {
            m.read = seq;
        }
        // Written at once: a restart right after finds the member as it is.
        self.chat_save();
        let role = if role.is_empty() { String::new() } else { format!(" ({role})") };
        let hears = if hear == Hear::All { ", hears all" } else { "" };
        Outcome::Text(format!("#{name}: {handle}{role}{hears}, {}", ident.said()))
    }

    /// A pane leaves; the person (outside every pane) takes a member out
    /// by its handle, online or offline. Either way it stays, marked left
    /// (until it joins again, or is older than `chat-days`).
    fn chat_leave(&mut self, me: Option<PaneId>, word: &str, handle: Option<String>) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-leave: {e}")),
        };
        let c = &self.chat.channels[at];
        let (m, said) = match (me, handle) {
            (Some(p), None) => match c.member(p).cloned() {
                Some(m) => (m, "left".to_string()),
                None => return Outcome::Error(format!("chat-leave: %{p} is not in #{}", c.name)),
            },
            (Some(_), Some(_)) => {
                return Outcome::Error("chat-leave: a pane leaves itself; the person takes another out".into());
            }
            (None, Some(h)) => match c.members.iter().find(|m| m.handle == h).cloned() {
                Some(m) if m.left => return Outcome::Error(format!("chat-leave: {h} left #{} already", c.name)),
                Some(m) => (m, format!("taken out by {}", Self::chat_you().handle)),
                None => return Outcome::Error(format!("chat-leave: no {h} in #{}", c.name)),
            },
            (None, None) => {
                return Outcome::Error(
                    "chat-leave: inside the pane that leaves, or give the handle to take out".into(),
                );
            }
        };
        let ident = m.pane.map_or_else(|| m.ident.clone(), |p| self.chat_ident(p));
        if let Some(x) = self.chat.channels[at].members.iter_mut().find(|x| x.handle == m.handle) {
            x.pane = None;
            x.left = true;
            x.since = now();
            x.ident = ident.clone();
            x.due = None;
            x.due_chain = None;
        }
        let who = Who { handle: m.handle, role: m.role, ident: Some(ident) };
        self.chat_line(at, "leave", Some(who), said, Vec::new());
        // Written at once: a restart right after finds it left.
        self.chat_save();
        Outcome::Ok
    }

    fn chat_say(&mut self, cid: Option<ClientId>, me: Option<PaneId>, word: &str, text: String) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-say: {e}")),
        };
        let max = self.opts.message_max_size;
        if text.len() > max {
            return Outcome::Error(format!("chat-say: {} bytes; message-max-size is {max}", text.len()));
        }
        let name = self.chat.channels[at].name.clone();
        let who = match me {
            Some(p) => {
                let Some(m) = self.chat.channels[at].member(p).cloned() else {
                    return Outcome::Error(format!("chat-say: %{p} is not in #{name} (chat-join {name})"));
                };
                // Not more than `RATE` a minute: one agent cannot drown it.
                let said = self.chat.said.entry(p).or_default();
                while said.front().is_some_and(|t| t.elapsed() >= Duration::from_secs(60)) {
                    said.pop_front();
                }
                if said.len() >= RATE {
                    return Outcome::Error(format!("chat-say: {RATE} a minute is the most a member says"));
                }
                said.push_back(Instant::now());
                let ident = self.chat_ident(p);
                if let Some(x) = self.chat.channels[at].member_mut(p) {
                    x.ident = ident.clone();
                }
                Who { handle: m.handle, role: m.role, ident: Some(ident) }
            }
            None => Self::chat_you(),
        };
        let members = self.chat.channels[at].members.clone();
        let words = mentions_in(&text);
        let named: Vec<String> = words.iter().filter(|h| members.iter().any(|m| &m.handle == *h)).cloned().collect();
        let before = self.chat.channels[at].last;
        let seq = self.chat_line(at, "say", Some(who.clone()), text.clone(), named.clone());
        // Its own line is not news to the speaker: read, when it had read
        // all before it. Not otherwise: saying something is no reading of
        // what others said meanwhile (unread lines would be lost).
        if let Some(p) = me
            && let Some(x) = self.chat.channels[at].member_mut(p)
            && x.read >= before
        {
            x.read = seq;
        }
        // Each one named is told at once, through its inbox, with what was
        // said before it since it last read.
        let mut not_told = Vec::new();
        for h in &named {
            let Some(m) = members.iter().find(|m| &m.handle == h) else { continue };
            if m.left {
                not_told.push(format!("{h} not told: it left #{name}"));
                continue;
            }
            let Some(to) = m.pane else {
                not_told.push(format!("{h} not told: offline (it reads this when it is back)"));
                continue;
            };
            if Some(to) == me {
                continue;
            }
            let earlier = self.chat_since(at, m.read, before, Some(to));
            match self.chat_tell(cid, me, to, &name, &who, &text, &earlier) {
                Ok(()) => {
                    if let Some(x) = self.chat.channels[at].member_mut(to) {
                        x.read = x.read.max(seq);
                    }
                }
                Err(why) => not_told.push(format!("{h} not told: {why}")),
            }
        }
        // The person, named (`@you`) by a member: told on the desktop and
        // by the webhook, as when an agent is done.
        if me.is_some() && words.iter().any(|w| w == "you") {
            self.chat_call_person(&name, &who, &text, me);
        }
        // Those that hear all: told of it, with what else comes, shortly, on
        // the chain of the speaker furthest along one.
        let chain = me.and_then(|p| self.pane_ref(p)).and_then(|p| p.actor.carry(false));
        for m in self.chat.channels[at].members.iter_mut() {
            if m.hear == Hear::All && m.pane.is_some() && m.pane != me && m.read < seq {
                m.due.get_or_insert_with(|| Instant::now() + HEAR_SETTLE);
                if chain.is_some_and(|(_, hop)| m.due_chain.is_none_or(|(_, was)| hop > was)) {
                    m.due_chain = chain;
                }
            }
        }
        self.chat.dirty = true;
        let mut out = format!("#{name} #{seq}");
        for n in not_told {
            out.push_str(&format!("\n{n}"));
        }
        Outcome::Text(out)
    }

    /// The lines said in channel `at` after `from` up to `to` (both line
    /// numbers), not by pane `own`, as a message lists them: the last
    /// `CONTEXT_MOST`, each cut to `CONTEXT_LINE_MOST` characters, and how
    /// many earlier ones were left out.
    fn chat_since(&self, at: usize, from: u64, to: u64, own: Option<PaneId>) -> (Vec<String>, usize) {
        let c = &self.chat.channels[at];
        let said: Vec<&Line> = c
            .recent
            .iter()
            .filter(|l| l.seq > from && l.seq <= to && l.kind == "say")
            .filter(|l| own.is_none() || l.from.as_ref().and_then(|w| w.ident.as_ref()).map(|i| i.pane) != own)
            .collect();
        let left = said.len().saturating_sub(CONTEXT_MOST);
        let lines = said[left..]
            .iter()
            .map(|l| {
                let who = l.from.as_ref().map_or("", |w| w.handle.as_str());
                let mut text: String = l.text.chars().take(CONTEXT_LINE_MOST).collect();
                if l.text.chars().count() > CONTEXT_LINE_MOST {
                    text.push('…');
                }
                format!("#{} {} {who}: {text}", l.seq, clock_of(&l.at))
            })
            .collect();
        (lines, left)
    }

    /// One named in a channel told: a message into its pane's inbox, on
    /// the chain the speaker is on (so the hop limit stops a loop), with
    /// what was said before it since it last read. Never into a `shell`
    /// pane, where a message runs as a command.
    #[allow(clippy::too_many_arguments)]
    fn chat_tell(
        &mut self,
        cid: Option<ClientId>,
        me: Option<PaneId>,
        to: PaneId,
        channel: &str,
        who: &Who,
        text: &str,
        earlier: &(Vec<String>, usize),
    ) -> Result<(), String> {
        let role = if who.role.is_empty() { String::new() } else { format!(" ({})", who.role) };
        let mut body = format!("[chat #{channel}] {}{role}: {text}", who.handle);
        let (lines, left) = earlier;
        if !lines.is_empty() {
            body.push_str("\n\nSaid before it, since you last read:");
            if *left > 0 {
                body.push_str(&format!(" ({left} earlier not shown: chat-read {channel} --all)"));
            }
            for l in lines {
                body.push('\n');
                body.push_str(l);
            }
        }
        let chain = me.and_then(|p| self.pane_ref(p)).and_then(|p| p.actor.carry(false));
        self.chat_send(cid, to, body, chain)
    }

    /// A chat message into pane `to`'s inbox, on `chain` (a task and hop:
    /// the speaker's), or a chain of its own.
    fn chat_send(
        &mut self,
        cid: Option<ClientId>,
        to: PaneId,
        text: String,
        carry: Option<(MsgId, u32)>,
    ) -> Result<(), String> {
        let via = self.pane_ref(to).map_or(WorkMode::Normal, |p| p.actor.mode);
        if via == WorkMode::Shell {
            return Err(format!("%{to} is in shell mode, where a message runs as a command"));
        }
        self.next_msg += 1;
        let id = self.next_msg;
        let (task, hop) = carry.unwrap_or((id, 0));
        let m = Message {
            id,
            task,
            from: self.sender_of(cid),
            to: self.address_of(to),
            via,
            hop,
            re: None,
            text,
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        self.enqueue(to, m)
    }

    /// The person named in a channel (`@you`): a desktop notification (its
    /// button goes to the speaker's pane) and the `done-webhook`, as when an
    /// agent is done.
    fn chat_call_person(&mut self, channel: &str, who: &Who, text: &str, from: Option<PaneId>) {
        let role = if who.role.is_empty() { String::new() } else { format!(" ({})", who.role) };
        let line = format!("[chat #{channel}] {}{role}: {text}", who.handle);
        if self.opts.notify {
            let go = from.map(|p| crate::notify::go_to_pane_url(&self.socket, p));
            crate::notify::notify_with(&format!("keepane: #{channel} {} asks you", who.handle), text, go.as_deref());
        }
        let host = crate::sysinfo::hostname();
        let json = serde_json::json!({
            "kind": "chat", "host": host, "channel": channel, "from": who.handle, "role": who.role, "text": text,
        });
        self.send_webhook(&format!("keepane · {host}\n{line}"), &line, json);
    }

    /// Those that hear all and whose lines have gathered long enough: told
    /// of what was said since they last read, in one message, once nothing
    /// of the channel waits in their inbox already.
    fn chat_hear(&mut self) {
        let now = Instant::now();
        for at in 0..self.chat.channels.len() {
            for i in 0..self.chat.channels[at].members.len() {
                let m = &self.chat.channels[at].members[i];
                let (Some(due), Some(to)) = (m.due, m.pane) else { continue };
                if due > now {
                    continue;
                }
                let name = self.chat.channels[at].name.clone();
                let head = format!("[chat #{name}]");
                if self.pane_ref(to).is_some_and(|p| p.actor.inbox.iter().any(|q| q.text.starts_with(&head))) {
                    continue;
                }
                let last = self.chat.channels[at].last;
                let (lines, left) = self.chat_since(at, m.read, last, Some(to));
                let m = &mut self.chat.channels[at].members[i];
                m.due = None;
                let chain = m.due_chain.take();
                if lines.is_empty() {
                    m.read = last;
                    continue;
                }
                let n = lines.len() + left;
                let mut body = format!("{head} {}:", crate::format::count(n, "new line"));
                if left > 0 {
                    body.push_str(&format!(" ({left} earlier not shown: chat-read {name} --all)"));
                }
                for l in &lines {
                    body.push('\n');
                    body.push_str(l);
                }
                if self.chat_send(None, to, body, chain).is_ok()
                    && let Some(m) = self.chat.channels[at].member_mut(to)
                {
                    m.read = last;
                    self.chat.dirty = true;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn chat_read(
        &mut self,
        cid: Option<ClientId>,
        me: Option<PaneId>,
        word: &str,
        n: Option<usize>,
        all: bool,
        peek: bool,
        wait: Option<u64>,
        json: bool,
    ) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-read: {e}")),
        };
        let n = n.unwrap_or(READ_DEFAULT).min(KEEP);
        let c = &self.chat.channels[at];
        // A pane not in it reads it whole, as a person might look.
        let reader = match me {
            Some(p) if c.member(p).is_some() => Reader::Pane(p),
            Some(_) => return self.chat_answer(at, c.last_lines(n), false, json),
            None => Reader::You,
        };
        if all || peek {
            let lines = if all { c.last_lines(n) } else { c.since(read_of(c, reader), n) };
            let more = !all && c.recent.iter().filter(|l| l.seq > read_of(c, reader)).count() > n;
            return self.chat_answer(at, lines, more, json);
        }
        let lines = c.since(read_of(c, reader), n);
        if lines.is_empty()
            && let (Some(secs), Some(cid)) = (wait, cid)
        {
            let secs = secs.min(self.opts.message_wait_max);
            let channel = c.id.clone();
            self.chat.waits.push(Wait {
                cid,
                until: Instant::now() + Duration::from_secs(secs),
                channel,
                reader,
                n,
                json,
            });
            return Outcome::Pending;
        }
        self.chat_take(at, reader, lines, n, json)
    }

    /// Lines read by `reader`: its position moved past them.
    fn chat_take(&mut self, at: usize, reader: Reader, lines: Vec<Line>, n: usize, json: bool) -> Outcome {
        let c = &mut self.chat.channels[at];
        if let Some(last) = lines.last().map(|l| l.seq) {
            match reader {
                Reader::You => c.you_read = last,
                Reader::Pane(p) => {
                    if let Some(m) = c.member_mut(p) {
                        m.read = last;
                        // Kept with the member (at the tick): it reads on
                        // from there after a restart.
                        self.chat.dirty = true;
                    }
                }
            }
        }
        let more = c.recent.iter().filter(|l| l.seq > read_of(c, reader)).count() > 0 && lines.len() == n;
        if reader == Reader::You && !lines.is_empty() {
            self.chat_save();
        }
        self.chat_answer(at, lines, more, json)
    }

    fn chat_answer(&self, at: usize, lines: Vec<Line>, more: bool, json: bool) -> Outcome {
        let c = &self.chat.channels[at];
        if json {
            return Outcome::Text(
                serde_json::json!({
                    "channel": { "id": c.id, "name": c.name, "topic": c.topic, "tags": c.tags },
                    "lines": lines,
                    "more": more,
                })
                .to_string(),
            );
        }
        if lines.is_empty() {
            return Outcome::Text(format!("nothing new in #{}", c.name));
        }
        let mut out: Vec<String> = lines.iter().map(line_text).collect();
        if more {
            out.push(format!("(more: chat-read {} again)", c.name));
        }
        Outcome::Text(out.join("\n"))
    }

    /// Waits on a channel with something new for their reader: answered.
    fn chat_wake(&mut self) {
        let mut i = 0;
        while i < self.chat.waits.len() {
            let w = &self.chat.waits[i];
            let Some(at) = self.chat.channels.iter().position(|c| c.id == w.channel) else {
                let w = self.chat.waits.remove(i);
                self.reply(w.cid, Outcome::Error("the channel was deleted".into()));
                continue;
            };
            let c = &self.chat.channels[at];
            let lines = c.since(read_of(c, w.reader), w.n);
            if lines.is_empty() {
                i += 1;
                continue;
            }
            let w = self.chat.waits.remove(i);
            let out = self.chat_take(at, w.reader, lines, w.n, w.json);
            self.reply(w.cid, out);
        }
    }

    /// From the tick: waits that ran out answered (nothing new), members
    /// whose pane went left, and days past `chat-days` deleted once a day.
    pub(super) fn chat_tick(&mut self) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.chat.waits.len() {
            if self.chat.waits[i].until > now {
                i += 1;
                continue;
            }
            let w = self.chat.waits.remove(i);
            let name =
                self.chat.channels.iter().find(|c| c.id == w.channel).map(|c| c.name.clone()).unwrap_or_default();
            let out = if w.json {
                Outcome::Text(
                    serde_json::json!({ "channel": { "id": w.channel, "name": name }, "lines": [], "more": false })
                        .to_string(),
                )
            } else {
                Outcome::Text(format!("nothing new in #{name}"))
            };
            self.reply(w.cid, out);
        }
        self.chat_presence();
        self.chat_hear();
        if self.chat.pruned.is_none_or(|t| t.elapsed() >= Duration::from_secs(86_400))
            && let Some(dir) = self.chat.dir.clone()
        {
            self.chat.pruned = Some(now);
            for c in &self.chat.channels {
                crate::histlog::prune_events(dir.join(&c.id), self.opts.chat_days);
            }
            // Members offline longer than what is said is kept go too.
            if self.opts.chat_days > 0 {
                let limit = chrono::Local::now() - chrono::Duration::days(i64::from(self.opts.chat_days));
                let stale = |m: &Member| {
                    m.pane.is_none()
                        && chrono::DateTime::parse_from_rfc3339(&m.since).is_ok_and(|t| t < limit.fixed_offset())
                };
                for c in &mut self.chat.channels {
                    let n = c.members.len();
                    c.members.retain(|m| !stale(m));
                    self.chat.dirty |= c.members.len() != n;
                }
            }
        }
        if self.chat.dirty {
            self.chat_save();
        }
    }

    fn chat_who(&self, word: &str, json: bool) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-who: {e}")),
        };
        let c = &self.chat.channels[at];
        if json {
            let all: Vec<serde_json::Value> = c
                .members
                .iter()
                .map(|m| {
                    let busy = m.pane.and_then(|p| self.pane_ref(p)).map(|p| !p.actor.idle());
                    serde_json::json!({
                        "handle": m.handle, "role": m.role, "ident": m.ident, "joined": m.joined,
                        "read": m.read, "busy": busy, "online": m.pane.is_some(), "since": m.since,
                        "hear": m.hear.word(), "left": m.left,
                    })
                })
                .collect();
            return Outcome::Text(serde_json::Value::Array(all).to_string());
        }
        if c.members.is_empty() {
            return Outcome::Text(format!("no one is in #{}", c.name));
        }
        let rows: Vec<Vec<String>> = c
            .members
            .iter()
            .map(|m| {
                let i = &m.ident;
                // Offline: where it was last.
                let state = match (m.left, m.pane.is_some()) {
                    (true, _) => "left",
                    (false, true) => "online",
                    (false, false) => "offline",
                };
                vec![
                    m.handle.clone(),
                    m.role.clone(),
                    state.into(),
                    i.machine.clone(),
                    i.place.clone(),
                    format!("%{}", i.pane),
                    i.address.clone(),
                    i.agent.clone().unwrap_or_default(),
                    i.model.clone().unwrap_or_default(),
                    if m.hear == Hear::All { "hears all".into() } else { String::new() },
                ]
            })
            .collect();
        Outcome::Text(super::align_columns(&rows).join("\n"))
    }

    fn chat_topic(&mut self, me: Option<PaneId>, word: &str, topic: String) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-topic: {e}")),
        };
        if let Err(e) = check_topic(&topic) {
            return Outcome::Error(format!("chat-topic: {e}"));
        }
        self.chat.channels[at].topic = topic.clone();
        let by = self.chat_by(at, me);
        let said = if topic.is_empty() { "cleared the topic".into() } else { format!("topic: {topic}") };
        self.chat_line(at, "topic", Some(by), said, Vec::new());
        self.chat_save();
        Outcome::Ok
    }

    fn chat_tag(&mut self, me: Option<PaneId>, word: &str, add: Vec<String>, remove: Vec<String>) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-tag: {e}")),
        };
        let mut tags = self.chat.channels[at].tags.clone();
        tags.retain(|t| !remove.contains(t));
        for t in add {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        tags.sort();
        if let Err(e) = check_tags(&tags) {
            return Outcome::Error(format!("chat-tag: {e}"));
        }
        self.chat.channels[at].tags = tags.clone();
        let by = self.chat_by(at, me);
        let said = if tags.is_empty() { "no tags".into() } else { format!("tags: {}", tags.join(", ")) };
        self.chat_line(at, "tags", Some(by), said, Vec::new());
        self.chat_save();
        Outcome::Text(if tags.is_empty() { String::new() } else { tags.join(",") })
    }

    fn chat_rename(&mut self, me: Option<PaneId>, word: &str, name: String) -> Outcome {
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-rename: {e}")),
        };
        let name = name.strip_prefix('#').unwrap_or(&name).to_string();
        if !valid_word(&name) {
            return Outcome::Error(format!("chat-rename: '{name}' is no channel name"));
        }
        if self.chat.channels.iter().enumerate().any(|(i, c)| i != at && c.name == name) {
            return Outcome::Error(format!("chat-rename: there is a channel {name} already"));
        }
        let was = std::mem::replace(&mut self.chat.channels[at].name, name.clone());
        let by = self.chat_by(at, me);
        self.chat_line(at, "name", Some(by), format!("renamed from {was}"), Vec::new());
        self.chat_save();
        Outcome::Ok
    }

    fn chat_delete(&mut self, me: Option<PaneId>, word: &str) -> Outcome {
        if me.is_some() {
            return Outcome::Error(
                "chat-delete: from outside every pane only (a terminal, or the C-b : prompt)".into(),
            );
        }
        let at = match self.chat_find(word) {
            Ok(i) => i,
            Err(e) => return Outcome::Error(format!("chat-delete: {e}")),
        };
        let c = self.chat.channels.remove(at);
        if let Some(dir) = &self.chat.dir {
            let aside = dir.join("deleted");
            let _ = std::fs::create_dir_all(&aside);
            let _ = std::fs::rename(dir.join(&c.id), aside.join(&c.id));
        }
        self.chat_save();
        self.chat_wake();
        Outcome::Text(format!("deleted #{} ({}); its lines are in chat/deleted", c.name, c.id))
    }
}

fn read_of(c: &Channel, reader: Reader) -> u64 {
    match reader {
        Reader::You => c.you_read,
        Reader::Pane(p) => c.member(p).map_or(c.last, |m| m.read),
    }
}

fn check_topic(topic: &str) -> Result<(), String> {
    if topic.chars().count() > TOPIC_MOST {
        return Err(format!("a topic is {TOPIC_MOST} characters at most"));
    }
    if topic.chars().any(char::is_control) {
        return Err("a topic is one line".into());
    }
    Ok(())
}

fn check_tags(tags: &[String]) -> Result<(), String> {
    if tags.len() > TAGS_MOST {
        return Err(format!("{TAGS_MOST} tags is the most"));
    }
    match tags.iter().find(|t| !valid_word(t)) {
        Some(t) => Err(format!("'{t}' is no tag (letters, digits, - and _, 1 to 64 of them)")),
        None => Ok(()),
    }
}

/// A file written whole: to a file beside it, then moved over it, so a
/// crash leaves the old one or the new one, never half of one.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// A channel's last `n` lines, from its day files, newest day back.
fn read_back(dir: &Path, n: usize) -> VecDeque<Line> {
    let mut days: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    days.sort();
    let mut out: VecDeque<Line> = VecDeque::new();
    for day in days.iter().rev() {
        let Ok(text) = std::fs::read_to_string(day) else { continue };
        let lines: Vec<Line> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        for l in lines.into_iter().rev() {
            out.push_front(l);
            if out.len() >= n {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(dir: &Path) -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "chat-test".into(), events);
        s.chat.dir = Some(dir.to_path_buf());
        s.chat_start();
        s
    }

    fn text(o: Outcome) -> String {
        match o {
            Outcome::Text(t) => t,
            Outcome::Error(e) => format!("error: {e}"),
            Outcome::Ok => "ok".into(),
            _ => "other".into(),
        }
    }

    /// Days past `chat-days` are deleted at the first tick (and once a day
    /// after); today's stays.
    #[test]
    fn days_past_chat_days_go() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-days-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        let id = a.chat.channels[0].id.clone();
        let old = dir.join(&id).join("2000-01-01.jsonl");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "{}\n").unwrap();
        a.opts.chat_days = 30;
        a.chat_tick();
        // Both are the event log's writer's to do: wait for it.
        crate::histlog::flush_events(Duration::from_secs(5));
        assert!(!old.exists(), "past 30 days: gone");
        let today = chrono::Local::now().format("%Y-%m-%d.jsonl").to_string();
        let kept = dir.join(&id).join(today);
        assert!(kept.exists(), "today's stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `channels.json` keepane cannot read is kept aside as
    /// `channels.json.bad`, not written over; the chat starts with `general`.
    #[test]
    fn an_unreadable_index_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("channels.json"), "{ not a list").unwrap();
        let a = server(&dir);
        assert_eq!(std::fs::read_to_string(dir.join("channels.json.bad")).unwrap(), "{ not a list");
        assert_eq!(a.chat.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), [GENERAL]);
        assert!(std::fs::read_to_string(dir.join("channels.json")).unwrap().contains("\"general\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What is said goes to the day file at once; `channels.json` is not
    /// written for it then, only at the next tick (for the line's number),
    /// but at once for a change to a channel.
    #[test]
    fn the_index_is_written_for_a_change_at_once_and_for_lines_at_the_tick() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        let last_saved = || -> u64 {
            let t = std::fs::read_to_string(dir.join("channels.json")).unwrap();
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            v[0]["last"].as_u64().unwrap()
        };
        assert_eq!(last_saved(), 1, "general made: written");
        let say = ChatCmd::Say { channel: GENERAL.into(), text: "hi".into() };
        assert!(text(a.exec_chat(say, None)).starts_with("#general #2"));
        assert_eq!(last_saved(), 1, "a line: not written yet");
        a.chat_tick();
        assert_eq!(last_saved(), 2, "the tick writes it");
        let topic = ChatCmd::Topic { channel: GENERAL.into(), topic: "anything".into() };
        assert_eq!(text(a.exec_chat(topic, None)), "ok");
        let t = std::fs::read_to_string(dir.join("channels.json")).unwrap();
        assert!(t.contains("\"anything\""), "a change: written at once: {t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Members are kept across a restart: back offline (no pane is them
    /// yet), with their handle, role, where they read to and what they
    /// hear; `chat-who` says so.
    #[test]
    fn members_come_back_offline_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-members-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        a.exec_chat(ChatCmd::New { name: "review".into(), topic: String::new(), tags: vec![] }, None);
        let at = a.chat_find("review").unwrap();
        let ident = Ident {
            machine: "M".into(),
            place: "s:0.0".into(),
            pane: 7,
            pane_name: Some("worker".into()),
            ..Default::default()
        };
        a.chat.channels[at].members.push(Member {
            pane: Some(7),
            handle: "lead".into(),
            role: "planner".into(),
            ident,
            joined: now(),
            read: 1,
            hear: Hear::All,
            since: now(),
            left: false,
            due: None,
            due_chain: None,
        });
        a.chat_save();
        drop(a);
        let b = server(&dir);
        let at = b.chat_find("review").unwrap();
        let m = &b.chat.channels[at].members[0];
        assert_eq!(
            (m.handle.as_str(), m.role.as_str(), m.pane, m.read, m.hear, m.ident.pane_name.as_deref()),
            ("lead", "planner", None, 1, Hear::All, Some("worker"))
        );
        let who = text(b.chat_who("review", false));
        assert!(who.contains("lead") && who.contains("offline") && who.contains("hears all"), "{who}");
        let json: serde_json::Value = serde_json::from_str(&text(b.chat_who("review", true))).unwrap();
        assert_eq!((json[0]["online"].as_bool(), json[0]["hear"].as_str()), (Some(false), Some("all")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A member taken out stays, marked left: not counted, not told when
    /// named, still left after a restart; taken out again: refused.
    #[test]
    fn one_that_left_is_kept_marked_left() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-left-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        a.exec_chat(ChatCmd::New { name: "review".into(), topic: String::new(), tags: vec![] }, None);
        let at = a.chat_find("review").unwrap();
        let ident = Ident { machine: "M".into(), place: "s:0.0".into(), pane: 7, ..Default::default() };
        for h in ["lead", "tester"] {
            a.chat.channels[at].members.push(Member {
                pane: None,
                handle: h.into(),
                role: String::new(),
                ident: ident.clone(),
                joined: now(),
                read: 0,
                hear: Hear::Mentions,
                since: now(),
                left: false,
                due: None,
                due_chain: None,
            });
        }
        let leave = |h: &str| ChatCmd::Leave { channel: "review".into(), handle: Some(h.into()) };
        assert!(matches!(a.exec_chat(leave("tester"), None), Outcome::Ok));
        let m = a.chat.channels[at].members.iter().find(|m| m.handle == "tester").unwrap();
        assert!(m.left && m.pane.is_none(), "kept, marked left");
        let list: serde_json::Value =
            serde_json::from_str(&text(a.exec_chat(ChatCmd::List { tags: vec![], json: true }, None))).unwrap();
        let review = list.as_array().unwrap().iter().find(|c| c["name"] == "review").unwrap().clone();
        assert_eq!(review["members"], 1, "not counted: {review}");
        let said = text(a.exec_chat(ChatCmd::Say { channel: "review".into(), text: "@tester @lead hi".into() }, None));
        assert!(said.contains("tester not told: it left #review"), "{said}");
        assert!(said.contains("lead not told: offline"), "{said}");
        assert!(text(a.exec_chat(leave("tester"), None)).contains("left #review already"));
        a.chat_save();
        drop(a);
        let b = server(&dir);
        let at = b.chat_find("review").unwrap();
        assert!(b.chat.channels[at].members.iter().any(|m| m.handle == "tester" && m.left), "still left");
        let who = text(b.chat_who("review", false));
        assert!(who.lines().any(|l| l.starts_with("tester") && l.contains("left")), "{who}");
        let json: serde_json::Value = serde_json::from_str(&text(b.chat_who("review", true))).unwrap();
        let t = json.as_array().unwrap().iter().find(|m| m["handle"] == "tester").unwrap().clone();
        assert_eq!((t["left"].as_bool(), t["online"].as_bool()), (Some(true), Some(false)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A member `channels.json` keeps that this keepane cannot read whole
    /// costs that member at most, never the channels: a `hear` of a later
    /// keepane reads as `mentions`, what is missing takes its default, and
    /// what is no member at all is left out.
    #[test]
    fn a_member_not_readable_costs_no_channel() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-odd-members-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        a.exec_chat(ChatCmd::New { name: "review".into(), topic: String::new(), tags: vec![] }, None);
        drop(a);
        let path = dir.join("channels.json");
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let review = v.as_array_mut().unwrap().iter_mut().find(|c| c["name"] == "review").unwrap();
        review["members"] = serde_json::json!([
            {"handle": "later", "hear": "loudly", "ident": {"machine": "M", "place": "s:0.0", "address": "", "pane": 3}, "joined": "x"},
            {"handle": "bare"},
            42,
            {"handle": "bad handle!"}
        ]);
        std::fs::write(&path, v.to_string()).unwrap();
        let b = server(&dir);
        assert!(!dir.join("channels.json.bad").exists(), "the index was read");
        let at = b.chat_find("review").expect("the channel is still there");
        let got: Vec<(&str, Hear)> = b.chat.channels[at].members.iter().map(|m| (m.handle.as_str(), m.hear)).collect();
        assert_eq!(got, [("later", Hear::Mentions), ("bare", Hear::Mentions)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `chat-list -J` gives each channel's last line said (what happened to
    /// it since is not one), cut to `LATEST` characters; none said: null.
    #[test]
    fn the_list_gives_each_channel_s_last_line() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-latest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        a.exec_chat(ChatCmd::New { name: "review".into(), topic: String::new(), tags: vec![] }, None);
        let say = |t: &str| ChatCmd::Say { channel: "review".into(), text: t.into() };
        a.exec_chat(say("one"), None);
        a.exec_chat(say("two"), None);
        a.exec_chat(ChatCmd::Topic { channel: "review".into(), topic: "t".into() }, None);
        let channel = |a: &mut Server, name: &str| -> serde_json::Value {
            let v: serde_json::Value =
                serde_json::from_str(&text(a.exec_chat(ChatCmd::List { tags: vec![], json: true }, None))).unwrap();
            v.as_array().unwrap().iter().find(|c| c["name"] == name).unwrap().clone()
        };
        let review = channel(&mut a, "review");
        let you = format!("you@{}", crate::sysinfo::hostname());
        assert_eq!(
            (review["latest"]["text"].as_str(), review["latest"]["handle"].as_str()),
            (Some("two"), Some(you.as_str())),
            "{review}"
        );
        assert!(review["latest"]["at"].as_str().is_some_and(|t| t.len() > 16), "{review}");
        assert_eq!(review["unread"], 0, "what the person said is not unread for the person: {review}");
        assert!(channel(&mut a, "general")["latest"].is_null(), "nothing said there");
        a.exec_chat(say(&"长".repeat(LATEST + 50)), None);
        let cut = channel(&mut a, "review")["latest"]["text"].as_str().unwrap().chars().count();
        assert_eq!(cut, LATEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restart keeps the channels (their id, topic and tags), what was
    /// said, and where the person had read to; `general` is made once.
    #[test]
    fn a_restart_keeps_the_channels_what_was_said_and_where_one_read_to() {
        let dir = std::env::temp_dir().join(format!("keepane-chat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        assert_eq!(a.chat.channels.len(), 1, "general, made");
        let new =
            ChatCmd::New { name: "review".into(), topic: "the 0.72 release".into(), tags: vec!["release".into()] };
        let id = text(a.exec_chat(new, None));
        assert_eq!(id.len(), 36, "{id}");
        let say = |t: &str| ChatCmd::Say { channel: "review".into(), text: t.into() };
        assert!(text(a.exec_chat(say("one"), None)).starts_with("#review #"));
        let read =
            ChatCmd::Read { channel: "review".into(), n: None, all: false, peek: false, wait: None, json: false };
        assert!(text(a.exec_chat(read.clone(), None)).contains(": one"));
        a.exec_chat(say("two"), None);
        // Named by the end of its id (its start is the time: alike for
        // channels made in the same minute, as these two were).
        assert_eq!(a.chat.channels[0].id[..8], id[..8], "made the same minute: the same start");
        let by_start = ChatCmd::Topic { channel: id[..8].to_string(), topic: "x".into() };
        assert!(text(a.exec_chat(by_start, None)).contains("no channel"), "not by its start");
        let tag = ChatCmd::Tag { channel: short_id(&id).to_string(), add: vec!["ci".into()], remove: vec![] };
        assert_eq!(text(a.exec_chat(tag, None)), "ci,release", "by the end of its id");
        crate::histlog::flush(Duration::from_secs(5));
        drop(a);

        let mut b = server(&dir);
        assert_eq!(b.chat.channels.len(), 2, "general not made again");
        let c = &b.chat.channels[b.chat_find("review").unwrap()];
        assert_eq!(
            (c.id.as_str(), c.topic.as_str(), c.tags.clone()),
            (id.as_str(), "the 0.72 release", vec!["ci".to_string(), "release".to_string()])
        );
        let said: Vec<&str> = c.recent.iter().filter(|l| l.kind == "say").map(|l| l.text.as_str()).collect();
        assert_eq!(said, ["one", "two"]);
        // The person had read "one", not "two".
        let next = text(b.exec_chat(read, None));
        assert!(next.contains(": two") && !next.contains(": one"), "{next}");
        // New lines go on after the old numbers.
        let n = text(b.exec_chat(say("three"), None));
        let last = c_last(&b);
        assert!(n.ends_with(&format!("#{last}")) && last > 2, "{n}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn c_last(s: &Server) -> u64 {
        s.chat.channels[s.chat_find("review").unwrap()].last
    }

    #[test]
    fn a_uuid_is_version_7_and_sorts_by_time() {
        let a = uuid_v7();
        std::thread::sleep(Duration::from_millis(3));
        let b = uuid_v7();
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'7', "{a}");
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'), "{a}");
        assert!(a < b, "{a} {b}");
        assert_ne!(uuid_v7(), uuid_v7());
    }

    #[test]
    fn small_pieces() {
        assert_eq!(mentions_in("@lead and @tester, then @lead again; mail a@b"), ["lead", "tester", "b"]);
        assert!(mentions_in("no one").is_empty());
        assert!(valid_word("ci-test_2") && !valid_word("") && !valid_word("a b") && !valid_word(&"x".repeat(65)));
        assert!(check_topic(&"x".repeat(TOPIC_MOST)).is_ok() && check_topic(&"x".repeat(TOPIC_MOST + 1)).is_err());
        assert!(check_topic("two\nlines").is_err());
        assert!(check_tags(&["a".into(), "b c".into()]).is_err());
        assert_eq!(clock_of("2026-11-17T14:02:33.120+08:00"), "14:02");
        let l = Line {
            seq: 3,
            at: "2026-11-17T14:02:33.120+08:00".into(),
            kind: "say".into(),
            from: Some(Who {
                handle: "lead".into(),
                role: "planner".into(),
                ident: Some(Ident {
                    machine: "DFINE".into(),
                    place: "work:0.0".into(),
                    pane: 3,
                    agent: Some("claude".into()),
                    ..Default::default()
                }),
            }),
            text: "hi".into(),
            mentions: vec![],
        };
        assert_eq!(line_text(&l), "#3 14:02 lead (planner) [DFINE work:0.0 %3 claude]: hi");
        let back: Line = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l, "a line reads back as it was written");
    }
}
