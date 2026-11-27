//! The group chat between machines (docs/design/chat.md §10). A channel is
//! one machine's; a pane of a paired machine joins it, says things in it and
//! reads it through that machine's server, which asks this one over link
//! (`POST /link/chat`, signed as every link request is). What this machine
//! asks of another, and what it answers when asked:
//!
//! - `chat-join/leave/say/read/who MACHINE/CHANNEL` and `chat-list MACHINE`,
//!   run here, are sent to that machine (`chat_elsewhere`); its answer is the
//!   command's. `--cached` (the board) answers with what was last heard, at
//!   once, and asks again behind it.
//! - Asked by a machine (`link_chat_in`): it may look at a channel, and
//!   join it, when it was given all of this one (`full`) or the channel is
//!   shared with it (`link share MACHINE #channel`). A pane of it is stamped
//!   as that machine said, with the machine as this one knows it.
//! - A pane here that joined a channel there is kept, in memory only (a
//!   pane's number is another pane's after a restart: after one, it joins
//!   again): what that machine sends it of the channel is taken without
//!   more (it asked to join), and every `HERE_EVERY` that machine is told
//!   which of them are still here (`here`), which keeps them online there.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::chat::{Caller, Ident};
use super::layout::PaneId;
use super::link_out::LinkOp;
use super::{ClientId, Outcome, Server};
use crate::command::ChatCmd;
use crate::link::{Answer, Peer};

/// How often a machine says which of its panes in another's channels are
/// still there.
pub const HERE_EVERY: Duration = Duration::from_secs(30);

/// How long what `--cached` keeps of another machine stays fresh: a list,
/// and a channel's lines or members.
const LIST_FRESH: Duration = Duration::from_secs(5);
const READ_FRESH: Duration = Duration::from_secs(1);

/// Waiting for an answer beyond what the other side may itself wait.
const ANSWER_GRACE: Duration = Duration::from_secs(5);

/// A pane here in a channel of another machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joined {
    /// That machine: where it is, and its key (who it is).
    pub addr: String,
    pub key: String,
    /// The channel's id there.
    pub channel: String,
    pub pane: PaneId,
}

/// What this machine keeps of the chat between machines.
#[derive(Default)]
pub struct ChatLink {
    pub joined: Vec<Joined>,
    /// When the machines with panes here in their channels were last told.
    pub here_sent: Option<Instant>,
    /// `--cached` answers: what was last heard of each, and when.
    pub cache: HashMap<String, (Instant, Result<String, String>)>,
    /// Asked again and not answered yet.
    pub asking: std::collections::HashSet<String>,
}

/// A chat request under way: what its answer is for.
pub struct ChatAsk {
    pub addr: String,
    pub key: String,
    pub op: String,
    /// A pane here that joins or leaves: kept in (or out of) `joined`.
    pub pane: Option<PaneId>,
    /// `--cached`: the answer kept under this.
    pub cache: Option<String>,
    /// `here`: nobody waits for it, and a failure is said nowhere.
    pub quiet: bool,
}

/// The body of a `POST /link/chat`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ChatBody {
    op: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    channel: String,
    /// The pane that asks, as its machine stamps it; none: the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<Ident>,
    /// The machine's name, for the person (`you@name`).
    #[serde(default)]
    machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hear: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<usize>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    peek: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait: Option<u64>,
    #[serde(default)]
    json: bool,
    /// `say --re`: the line it replies to (a machine before replies sends
    /// none; one before them leaves it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    re: Option<u64>,
    /// `read --thread`: the line whose thread is asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread: Option<u64>,
    /// `here`: the panes of the asking machine in this one's channels that
    /// are still there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    panes: Vec<PaneId>,
}

/// The most characters of a field another machine says of its pane.
const FIELD_MOST: usize = 120;

/// A field another machine said, as it is kept and shown here: one line,
/// no control character (it goes into lines, `chat-who` and the board, and
/// so to terminals), `FIELD_MOST` characters at most.
fn plain(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(FIELD_MOST).collect()
}

/// Its pane as another machine said it, kept plain (`plain`), at that
/// machine as this one knows it.
fn plain_ident(mut i: Ident, addr: &str) -> Ident {
    i.machine = plain(&i.machine);
    i.place = plain(&i.place);
    i.address = plain(&i.address);
    i.pane_name = i.pane_name.as_deref().map(plain).filter(|n| !n.is_empty());
    i.agent = i.agent.as_deref().map(plain).filter(|n| !n.is_empty());
    i.model = i.model.as_deref().map(plain).filter(|n| !n.is_empty());
    i.addr = addr.to_string();
    i
}

/// A machine's name as it says it, for `you@name`: what a host name is
/// made of (letters, digits, `-`, `.`, `_`), 64 at most; "?" for none.
fn plain_machine(s: &str) -> String {
    let n: String = s.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_')).take(64).collect();
    if n.is_empty() { "?".into() } else { n }
}

/// A channel word naming another machine's: `MACHINE/CHANNEL`.
fn split_machine(word: &str) -> Option<(&str, &str)> {
    let (machine, rest) = word.split_once('/')?;
    (!machine.is_empty() && !rest.is_empty()).then_some((machine, rest))
}

impl Server {
    /// The paired machine a word names: its `host:port`, or its name as
    /// this one knows it (`link list`; the name before the first dot will
    /// do); its address and key.
    fn chat_machine(&self, machine: &str) -> Result<(String, String), String> {
        let l = self.link.as_ref().ok_or_else(|| format!("{machine}: no machine is paired with this one"))?;
        if let Some(p) = l.peer_by_addr(machine) {
            return Ok((p.addr.clone(), p.key.clone()));
        }
        let lower = machine.to_lowercase();
        let named: Vec<&Peer> = l
            .peers
            .iter()
            .filter(|p| {
                self.machine_names.get(&p.addr).and_then(|n| n.name.as_deref()).is_some_and(|n| {
                    let n = n.to_lowercase();
                    n == lower || n.split('.').next() == Some(lower.as_str())
                })
            })
            .collect();
        match named.as_slice() {
            [p] => Ok((p.addr.clone(), p.key.clone())),
            [] => Err(format!("{machine}: no paired machine of that address or name (`keepane link list`)")),
            _ => Err(format!("{machine}: more than one paired machine has that name: give its host:port")),
        }
    }

    /// A chat command about another machine's channel, asked of it; None
    /// for one about a channel here.
    pub(super) fn chat_elsewhere(&mut self, cmd: &ChatCmd, cid: Option<ClientId>) -> Option<Outcome> {
        let (op, word, cached) = match cmd {
            ChatCmd::List { machine: Some(m), cached, .. } => ("list", m.clone(), *cached),
            ChatCmd::Join { channel, .. } => ("join", channel.clone(), false),
            ChatCmd::Leave { channel, .. } => ("leave", channel.clone(), false),
            ChatCmd::Say { channel, files, .. } => {
                // Files go to this machine's channels only, for now.
                if !files.is_empty() {
                    let (machine, _) = split_machine(channel)?;
                    return Some(Outcome::Error(format!(
                        "chat-say: files are sent to this machine's channels only, not yet to {machine}'s"
                    )));
                }
                ("say", channel.clone(), false)
            }
            ChatCmd::Dir { channel } => {
                let (machine, _) = split_machine(channel)?;
                return Some(Outcome::Error(format!("chat-dir: the files of a channel of {machine} are at {machine}")));
            }
            ChatCmd::Read { channel, cached, .. } => ("read", channel.clone(), *cached),
            ChatCmd::GroupFiles { channel, cached, .. } => ("group-files", channel.clone(), *cached),
            ChatCmd::GroupFileRm { channel, .. } => ("group-file-rm", channel.clone(), false),
            ChatCmd::Who { channel, cached, .. } => ("who", channel.clone(), *cached),
            ChatCmd::Topic { channel, .. }
            | ChatCmd::Tag { channel, .. }
            | ChatCmd::Rename { channel, .. }
            | ChatCmd::Delete { channel } => {
                let (machine, _) = split_machine(channel)?;
                return Some(Outcome::Error(format!("a channel of {machine} is changed at {machine}, by its person")));
            }
            _ => return None,
        };
        let (machine, channel) = if op == "list" { (word.as_str(), "") } else { split_machine(&word)? };
        let (addr, key) = match self.chat_machine(machine) {
            Ok(m) => m,
            Err(e) => return Some(Outcome::Error(format!("chat-{op}: {e}"))),
        };
        let me = self.caller_pane(cid);
        let mut body = ChatBody {
            op: op.into(),
            channel: channel.to_string(),
            pane: me.map(|p| self.chat_ident(p)),
            machine: crate::sysinfo::hostname(),
            ..ChatBody::default()
        };
        match cmd {
            ChatCmd::List { tags, json, .. } => {
                body.tags = tags.clone();
                body.json = *json;
            }
            ChatCmd::Join { handle, role, hear, .. } => {
                if me.is_none() {
                    return Some(Outcome::Error(
                        "chat-join: run it inside the pane that joins (a person says with chat-say)".into(),
                    ));
                }
                (body.handle, body.role, body.hear) = (handle.clone(), role.clone(), hear.clone());
            }
            ChatCmd::Leave { handle, .. } => body.handle = handle.clone(),
            ChatCmd::Say { text, re, .. } => (body.text, body.re) = (text.clone(), *re),
            ChatCmd::Read { n, all, peek, wait, json, thread, .. } => {
                (body.n, body.all, body.peek, body.json, body.thread) = (*n, *all, *peek, *json, *thread);
                body.wait = wait.map(|w| w.min(self.opts.message_wait_max));
            }
            ChatCmd::Who { json, .. } => body.json = *json,
            ChatCmd::GroupFiles { json, .. } => body.json = *json,
            // The file asked for, in the text (a machine before group files
            // answers that it is not a chat request).
            ChatCmd::GroupFileRm { file, .. } => body.text = file.clone(),
            _ => {}
        }
        // The board: what was last heard, at once; asked again behind it
        // when it is old.
        let cache = cached.then(|| format!("{addr} {cmd:?}"));
        if let Some(k) = &cache {
            let fresh = if op == "list" { LIST_FRESH } else { READ_FRESH };
            if let Some((at, kept)) = self.chat_link.cache.get(k) {
                let out = match kept.clone() {
                    Ok(t) => Outcome::Text(t),
                    Err(e) => Outcome::Error(e),
                };
                if at.elapsed() >= fresh
                    && self.chat_link.asking.insert(k.clone())
                    && self.chat_ask(None, &addr, &key, body, None, cache.clone(), false).is_err()
                {
                    self.chat_link.asking.remove(k);
                }
                return Some(out);
            }
            self.chat_link.asking.insert(k.clone());
        }
        let pane = if matches!(op, "join" | "leave") { me } else { None };
        Some(match self.chat_ask(cid, &addr, &key, body, pane, cache.clone(), false) {
            Ok(()) => Outcome::Pending,
            Err(e) => {
                if let Some(k) = cache {
                    self.chat_link.asking.remove(&k);
                }
                Outcome::Error(format!("chat-{op}: {e}"))
            }
        })
    }

    /// `chat-list --machines` (the board): these channels, then, of each
    /// paired machine this one asks (not disconnected), those it may look at
    /// there, as last heard (asked again behind it when older than
    /// `LIST_FRESH`; nothing of a machine before its first answer). Theirs
    /// are `MACHINE/ID`, with the machine's name.
    pub(super) fn chat_list_everywhere(&mut self, me: &Caller, tags: &[String], json: bool) -> Outcome {
        let here = match self.chat_list(me, tags, true, None) {
            Outcome::Text(t) => t,
            other => return other,
        };
        let mut all: Vec<serde_json::Value> = serde_json::from_str(&here).unwrap_or_default();
        let peers: Vec<(String, String)> = self
            .link
            .as_ref()
            .map(|l| l.peers.iter().filter(|p| !p.off).map(|p| (p.addr.clone(), p.key.clone())).collect())
            .unwrap_or_default();
        for (addr, key) in peers {
            let k = format!("{addr} --machines {tags:?}");
            let kept = self.chat_link.cache.get(&k).cloned();
            if kept.as_ref().is_none_or(|(at, _)| at.elapsed() >= LIST_FRESH) && self.chat_link.asking.insert(k.clone())
            {
                let body = ChatBody {
                    op: "list".into(),
                    json: true,
                    tags: tags.to_vec(),
                    machine: crate::sysinfo::hostname(),
                    ..ChatBody::default()
                };
                if self.chat_ask(None, &addr, &key, body, None, Some(k.clone()), false).is_err() {
                    self.chat_link.asking.remove(&k);
                }
            }
            let Some((_, Ok(text))) = kept else { continue };
            let name = self.machine_names.get(&addr).and_then(|n| n.name.clone()).unwrap_or_else(|| addr.clone());
            for mut c in serde_json::from_str::<Vec<serde_json::Value>>(&text).unwrap_or_default() {
                let id = c["id"].as_str().unwrap_or_default().to_string();
                c["id"] = format!("{addr}/{id}").into();
                c["machine"] = name.clone().into();
                all.push(c);
            }
        }
        if json {
            return Outcome::Text(serde_json::Value::Array(all).to_string());
        }
        let rows: Vec<Vec<String>> = all
            .iter()
            .map(|c| {
                vec![
                    format!("#{}", c["name"].as_str().unwrap_or_default()),
                    c["machine"].as_str().unwrap_or_default().to_string(),
                    crate::format::count(c["members"].as_u64().unwrap_or(0) as usize, "member"),
                    c["topic"].as_str().unwrap_or_default().to_string(),
                ]
            })
            .collect();
        Outcome::Text(super::align_columns(&rows).join("\n"))
    }

    /// A chat request sent to the machine at `addr`.
    #[allow(clippy::too_many_arguments)]
    fn chat_ask(
        &mut self,
        cid: Option<ClientId>,
        addr: &str,
        key: &str,
        body: ChatBody,
        pane: Option<PaneId>,
        cache: Option<String>,
        quiet: bool,
    ) -> Result<(), String> {
        let longest = ANSWER_GRACE + Duration::from_secs(body.wait.unwrap_or(0));
        let ask = ChatAsk { addr: addr.into(), key: key.into(), op: body.op.clone(), pane, cache, quiet };
        let body = serde_json::to_vec(&body).expect("a body serializes");
        self.link_signed(cid, addr, key, "POST", "/link/chat", body, longest, LinkOp::Chat(Box::new(ask)))
    }

    /// The answer to a chat request: told to whoever asked, kept for
    /// `--cached`; a pane that joined kept (or, left, let go).
    pub(super) fn chat_answered(
        &mut self,
        cid: Option<ClientId>,
        nonce: &str,
        ask: ChatAsk,
        result: Result<Answer, String>,
    ) {
        let checked = Self::checked(&ask.addr, &ask.key, nonce, result);
        if let Some(k) = &ask.cache {
            self.chat_link.asking.remove(k);
        }
        if ask.quiet {
            if let Err(e) = &checked {
                log::debug!("chat: {} not told which panes are here: {e}", ask.addr);
            }
            return;
        }
        let out = match checked {
            Ok(ans) if ans.status == 200 => {
                let text = ans.text();
                if ask.op == "join" {
                    // The channel's id there comes first, on a line of its own.
                    let (id, said) = text.split_once('\n').unwrap_or(("", text.as_str()));
                    if let Some(pane) = ask.pane
                        && !id.is_empty()
                    {
                        let j = Joined { addr: ask.addr.clone(), key: ask.key.clone(), channel: id.into(), pane };
                        if !self.chat_link.joined.contains(&j) {
                            self.chat_link.joined.push(j);
                        }
                    }
                    Outcome::Text(said.to_string())
                } else {
                    if ask.op == "leave"
                        && let Some(pane) = ask.pane
                    {
                        // That machine answers with the channel's id.
                        let id = text.trim().to_string();
                        self.chat_link.joined.retain(|j| !(j.key == ask.key && j.pane == pane && j.channel == id));
                    }
                    if ask.op == "leave" { Outcome::Ok } else { Outcome::Text(text) }
                }
            }
            Ok(ans) if ans.status == 404 && ans.text().contains("not a keepane link path") => Outcome::Error(format!(
                "keepane on {} is older than {}: update it there for its group chat",
                ask.addr,
                env!("CARGO_PKG_VERSION")
            )),
            Ok(ans) => Outcome::Error(ans.text()),
            Err(e) => Outcome::Error(e),
        };
        if let Some(k) = ask.cache {
            let kept = match &out {
                Outcome::Text(t) => Ok(t.clone()),
                Outcome::Error(e) => Err(e.clone()),
                _ => Ok(String::new()),
            };
            self.chat_link.cache.insert(k, (Instant::now(), kept));
        }
        if let Some(cid) = cid {
            self.reply(cid, out);
        }
    }

    /// `POST /link/chat`: a paired machine (`peer`, as its request's network
    /// leaves it) asks of a channel here.
    pub(super) fn link_chat_in(&mut self, cid: Option<ClientId>, body: &str, nonce: &str, peer: &Peer) -> Outcome {
        let Ok(b) = serde_json::from_str::<ChatBody>(body) else {
            return self.link_answer(nonce, 400, "not a chat request".into());
        };
        self.chat_presence();
        if b.op == "here" {
            self.chat_here(peer, &b.panes);
            return self.link_answer(nonce, 200, String::new());
        }
        // A pane of that machine as it stamped it, at that machine as this
        // one knows it; else its person.
        let me = match b.pane.clone() {
            Some(ident) => Caller::There { key: peer.key.clone(), ident: plain_ident(ident, &peer.addr) },
            None => Caller::YouThere { machine: plain_machine(&b.machine) },
        };
        if b.op == "list" {
            let out = self.chat_list(&me, &b.tags, b.json, Some(peer));
            return self.chat_link_out(nonce, out);
        }
        // What it may not look at is not there for it.
        let at = match self.chat_find_for(&b.channel, peer) {
            Ok(i) => i,
            Err(e) => return self.link_answer(nonce, 404, format!("chat-{}: {e}", b.op)),
        };
        let id = self.chat.channels[at].id.clone();
        let out = match b.op.as_str() {
            "join" => match self.chat_join(&me, &id, b.handle, b.role, b.hear) {
                Outcome::Text(t) => Outcome::Text(format!("{id}\n{t}")),
                other => other,
            },
            "leave" => match self.chat_leave(&me, &id, b.handle) {
                Outcome::Ok => Outcome::Text(id.clone()),
                other => other,
            },
            "say" => self.chat_say(None, &me, &id, b.text, Vec::new(), b.re),
            "read" if b.thread.is_some() => {
                self.chat_thread(at, b.thread.unwrap_or(0), b.json, Some(peer.addr.clone()))
            }
            "read" => {
                let out = self.chat_read(cid, &me, &id, b.n, b.all, b.peek, b.wait, b.json);
                // Waiting: answered when something comes, or the wait runs
                // out, signed as an answer to this request.
                if matches!(out, Outcome::Pending)
                    && let Some(w) = self.chat.waits.last_mut()
                {
                    w.link = Some(nonce.to_string());
                    return Outcome::Pending;
                }
                out
            }
            "who" => self.chat_who(&id, b.json),
            "group-files" => self.chat_group_files(&me, at, b.json),
            "group-file-rm" => self.chat_group_file_rm(&me, at, &b.text),
            other => Outcome::Error(format!("not a chat request: {other}")),
        };
        self.chat_link_out(nonce, out)
    }

    /// An outcome as a signed answer.
    pub(super) fn chat_link_out(&mut self, nonce: &str, out: Outcome) -> Outcome {
        match out {
            Outcome::Text(t) => self.link_answer(nonce, 200, t),
            Outcome::Ok => self.link_answer(nonce, 200, String::new()),
            Outcome::Error(e) => self.link_answer(nonce, 400, e),
            other => other,
        }
    }

    /// A channel a machine may look at, by the word it gave; one it may not
    /// is answered as one that is not there.
    fn chat_find_for(&self, word: &str, peer: &Peer) -> Result<usize, String> {
        match self.chat_find(word) {
            Ok(i) if super::chat::chat_may_look(&self.chat.channels[i], peer) => Ok(i),
            Ok(_) => Err(format!("no channel {word} (`chat-list` lists them)")),
            Err(e) => Err(e),
        }
    }

    /// A machine says which of its panes in channels here are still there:
    /// those online (back, if they were not), the others of it offline.
    pub(super) fn chat_here(&mut self, peer: &Peer, panes: &[PaneId]) {
        let mut changed = false;
        for at in 0..self.chat.channels.len() {
            for i in 0..self.chat.channels[at].members.len() {
                let m = &mut self.chat.channels[at].members[i];
                let Some(t) = m.there.as_mut().filter(|t| t.key == peer.key) else { continue };
                if m.left {
                    continue;
                }
                let here = panes.contains(&t.pane);
                let was = t.said_online;
                t.seen = here.then(Instant::now);
                t.said_online = here;
                if here == was {
                    continue;
                }
                m.since = super::chat::now();
                if !here {
                    m.due = None;
                }
                let who =
                    super::chat::Who { handle: m.handle.clone(), role: m.role.clone(), ident: Some(m.ident.clone()) };
                let (kind, said) = if here {
                    ("back", "is back".to_string())
                } else {
                    ("away", "went offline (its pane is gone)".into())
                };
                self.chat_line(at, kind, Some(who), said, Vec::new());
                changed = true;
            }
        }
        if changed {
            self.chat_save();
        }
    }

    /// A chat message for a pane of another machine: handed to it as any
    /// message is, marked with the channel, which that machine takes for a
    /// pane that joined it. Nobody waits for its answer.
    pub(super) fn chat_send_there(
        &mut self,
        cid: Option<ClientId>,
        addr: &str,
        pane: PaneId,
        channel_id: &str,
        text: String,
        carry: Option<(super::actor::MsgId, u32)>,
    ) -> Result<(), String> {
        let from = self.sender_of(cid);
        match self.send_remote_chat(addr, &format!("%{pane}"), text, from, carry, channel_id) {
            Outcome::Error(e) => Err(e),
            _ => Ok(()),
        }
    }

    /// Whether a message of `peer` for pane `pane` here is one of a channel
    /// that pane joined there: taken without more.
    pub(super) fn chat_joined_there(&self, peer_key: &str, pane: PaneId, channel: &str) -> bool {
        self.chat_link.joined.iter().any(|j| j.key == peer_key && j.pane == pane && j.channel == channel)
    }

    /// From the tick: every `HERE_EVERY`, each machine with panes here in its
    /// channels told which are still here; those gone let go of here after.
    pub(super) fn chat_link_tick(&mut self) {
        if self.chat_link.joined.is_empty() || self.chat_link.here_sent.is_some_and(|t| t.elapsed() < HERE_EVERY) {
            return;
        }
        self.chat_link.here_sent = Some(Instant::now());
        let mut machines: Vec<(String, String)> = Vec::new();
        for j in &self.chat_link.joined {
            if !machines.iter().any(|(_, k)| *k == j.key) {
                machines.push((j.addr.clone(), j.key.clone()));
            }
        }
        for (addr, key) in machines {
            let mut panes: Vec<PaneId> = self
                .chat_link
                .joined
                .iter()
                .filter(|j| j.key == key && self.place_of(j.pane).is_some())
                .map(|j| j.pane)
                .collect();
            panes.sort_unstable();
            panes.dedup();
            let body = ChatBody { op: "here".into(), panes, ..ChatBody::default() };
            let _ = self.chat_ask(None, &addr, &key, body, None, None, true);
        }
        let alive: Vec<Joined> =
            self.chat_link.joined.iter().filter(|j| self.place_of(j.pane).is_some()).cloned().collect();
        self.chat_link.joined = alive;
    }

    /// Whether a machine may see the channels here at all (`link list`).
    pub(super) fn chat_shared_with(&self, key: &str) -> Vec<String> {
        self.chat
            .channels
            .iter()
            .filter(|c| c.shared.iter().any(|k| k == key))
            .map(|c| format!("#{}", c.name))
            .collect()
    }

    /// `link share MACHINE #channel` (or unshare): the channel marked for
    /// that machine, or no more; what it has now said.
    pub(super) fn chat_share(&mut self, peer: &Peer, words: &[String], share: bool) -> Result<String, String> {
        let mut names = Vec::new();
        for w in words {
            let at = self.chat_find(w)?;
            let c = &mut self.chat.channels[at];
            if share {
                if !c.shared.contains(&peer.key) {
                    c.shared.push(peer.key.clone());
                }
            } else {
                c.shared.retain(|k| *k != peer.key);
            }
            names.push(format!("#{}", c.name));
        }
        self.chat_save();
        let addr = &peer.addr;
        Ok(if share {
            format!("{} shared with {addr}: it may look at them and join them", names.join(", "))
        } else {
            format!("{} no longer shared with {addr}", names.join(", "))
        })
    }

    /// Every channel taken back from that machine; how many.
    pub(super) fn chat_unshare_all(&mut self, key: &str) -> usize {
        let mut n = 0;
        for c in &mut self.chat.channels {
            let before = c.shared.len();
            c.shared.retain(|k| k != key);
            n += before - c.shared.len();
        }
        if n > 0 {
            self.chat_save();
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What another machine says of its pane is kept plain: no control
    /// character (an escape sequence would reach terminals here), one line,
    /// at most `FIELD_MOST`; its address is this machine's knowledge of it,
    /// not what it said. Its person's machine name, a host name's letters.
    #[test]
    fn what_another_machine_says_of_its_pane_is_kept_plain() {
        let said = Ident {
            machine: "mac\x1b]0;owned\x07".into(),
            addr: "1.2.3.4:1".into(),
            place: "blog:0.0\nforged line".into(),
            address: "x".repeat(500),
            pane: 7,
            pane_name: Some("\x1b[2J".into()),
            agent: Some("claude\r".into()),
            model: None,
        };
        let kept = plain_ident(said, "10.0.0.2:7681");
        assert_eq!(kept.machine, "mac]0;owned");
        assert_eq!(kept.place, "blog:0.0forged line");
        assert_eq!(kept.address.chars().count(), FIELD_MOST);
        assert_eq!(kept.pane_name.as_deref(), Some("[2J"), "the escape gone");
        assert_eq!(kept.agent.as_deref(), Some("claude"));
        assert_eq!(kept.addr, "10.0.0.2:7681", "as this machine knows it");
        assert_eq!(plain_machine("mac\x1b[31m.local"), "mac31m.local");
        assert_eq!(plain_machine("\x07"), "?");
    }

    #[test]
    fn a_word_names_another_machines_channel_with_a_slash() {
        assert_eq!(split_machine("DFINE/#review"), Some(("DFINE", "#review")));
        assert_eq!(split_machine("100.64.0.7:7681/review"), Some(("100.64.0.7:7681", "review")));
        assert_eq!(split_machine("#review"), None);
        assert_eq!(split_machine("/review"), None);
        assert_eq!(split_machine("DFINE/"), None);
    }
}
