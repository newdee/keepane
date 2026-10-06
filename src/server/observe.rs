//! The observability store (docs/design/mailbox.md §10): what happened to
//! every message, as events. Kept in memory for queries and waits, and
//! appended to a JSON Lines file a day (through the history log's writer)
//! so it outlives the server. Platform-neutral: files and text only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::actor::{End, EnvelopeStyle, Message, MsgId, Sender, WorkMode};

/// Where a message is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// In an inbox, waiting for its pane to be free.
    Queued,
    /// Typed into its pane; the pane is working on it.
    Delivered,
    /// Taken by the program with `read-message`.
    Read,
    Done,
    /// A shell command that reported failure.
    Failed,
    /// Its agent exited before it was done.
    Abandoned,
    /// Deleted from the inbox, or its pane closed, or the server restarted.
    Dropped,
    /// Never queued: over the hop limit, a full inbox, not allowed.
    Rejected,
    /// Handed to another machine (docs/design/link.md): what became of it
    /// there is in that machine's event log; `why` holds what it said.
    Forwarded,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Queued => "queued",
            Stage::Delivered => "delivered",
            Stage::Read => "read",
            Stage::Done => "done",
            Stage::Failed => "failed",
            Stage::Abandoned => "abandoned",
            Stage::Dropped => "dropped",
            Stage::Rejected => "rejected",
            Stage::Forwarded => "forwarded",
        }
    }

    fn parse(s: &str) -> Option<Stage> {
        [
            Stage::Queued,
            Stage::Delivered,
            Stage::Read,
            Stage::Done,
            Stage::Failed,
            Stage::Abandoned,
            Stage::Dropped,
            Stage::Rejected,
            Stage::Forwarded,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
    }

    /// Nothing more will happen to it. A message a program took with
    /// `read-message` is its business from then on: no signal ever says
    /// more, so it counts as done with.
    pub fn finished(self) -> bool {
        !matches!(self, Stage::Queued | Stage::Delivered)
    }
}

type Time = chrono::DateTime<chrono::Local>;

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub msg: Message,
    pub stage: Stage,
    pub delivered: Option<Time>,
    pub ended: Option<Time>,
    /// A shell command's success, as its prompt hook reported it.
    pub ok: Option<bool>,
    /// What a shell command printed, cut at `message-max-size`.
    pub output: Option<String>,
    pub cut: bool,
    /// Why it was rejected or dropped, and by whom.
    pub why: Option<String>,
}

/// Records kept in memory; older finished ones are only in the files.
const KEPT: usize = 10_000;

/// Event lines kept in memory for `list-events -n`.
const RECENT: usize = 5_000;

pub struct Store {
    records: BTreeMap<MsgId, Record>,
    /// The last lines written, newest last: what a watcher asks for every
    /// second is answered from here, never by reading the files.
    recent: std::collections::VecDeque<String>,
    /// Where the day files go; None when `event-log` is off.
    pub dir: Option<PathBuf>,
    /// The most one day file holds (`event-log-max`).
    pub cap: u64,
}

fn now() -> Time {
    chrono::Local::now()
}

fn stamp(t: Time) -> String {
    serde_json::to_string(&t.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)).expect("a string serializes")
}

fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

impl Default for Store {
    fn default() -> Self {
        Store {
            records: BTreeMap::new(),
            recent: std::collections::VecDeque::new(),
            dir: None,
            cap: crate::histlog::DAY_CAP,
        }
    }
}

impl Store {
    /// One event line: the time first, then its kind, then its fields
    /// (already JSON, in a fixed order).
    fn write(&mut self, at: Time, ev: &str, fields: &str) {
        let Some(dir) = &self.dir else { return };
        let line = format!("{{\"at\":{},\"ev\":\"{ev}\"{fields}}}", stamp(at));
        let path = dir.join(format!("{}.jsonl", at.format("%Y-%m-%d")));
        crate::histlog::append_event(path, format!("{line}\n"), self.cap);
        self.recent.push_back(line);
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
    }

    /// The last `n` event lines that `keep` accepts, oldest first.
    pub fn recent(&self, n: usize, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let mut out: Vec<String> = self.recent.iter().rev().filter(|l| keep(l)).take(n).cloned().collect();
        out.reverse();
        out
    }

    fn trim(&mut self) {
        while self.records.len() > KEPT {
            let old = self.records.iter().find(|(_, r)| r.stage.finished()).map(|(id, _)| *id);
            let Some(id) = old.or_else(|| self.records.keys().next().copied()) else { break };
            self.records.remove(&id);
        }
    }

    /// A message was sent: queued, or rejected (and so never queued).
    pub fn sent(&mut self, m: &Message, rejected: Option<&str>) {
        let why = rejected.map(|w| format!(",\"why\":{}", js(w))).unwrap_or_default();
        let ev = if rejected.is_some() { "rejected" } else { "sent" };
        self.write(m.at, ev, &format!(",\"msg\":{},\"text\":{}{why}", m.envelope(), js(&m.text)));
        let stage = if rejected.is_some() { Stage::Rejected } else { Stage::Queued };
        self.records.insert(
            m.id,
            Record {
                msg: m.clone(),
                stage,
                delivered: None,
                ended: rejected.map(|_| m.at),
                ok: None,
                output: None,
                cut: false,
                why: rejected.map(String::from),
            },
        );
        self.trim();
    }

    fn update(&mut self, id: MsgId, f: impl FnOnce(&mut Record)) {
        if let Some(r) = self.records.get_mut(&id) {
            f(r);
        }
    }

    pub fn delivered(&mut self, id: MsgId) {
        let t = now();
        self.write(t, "delivered", &format!(",\"id\":{id}"));
        self.update(id, |r| {
            r.stage = Stage::Delivered;
            r.delivered = Some(t);
        });
    }

    pub fn read(&mut self, id: MsgId) {
        let t = now();
        self.write(t, "read", &format!(",\"id\":{id}"));
        self.update(id, |r| {
            r.stage = Stage::Read;
            r.delivered = Some(t);
        });
    }

    /// The pane is done with it: `ok` and `output` for a shell command.
    pub fn ended(&mut self, id: MsgId, end: End, ok: Option<bool>, output: Option<(String, bool)>) {
        let t = now();
        let stage = match (end, ok) {
            (End::Abandoned, _) => Stage::Abandoned,
            (End::Done, Some(false)) => Stage::Failed,
            (End::Done, _) => Stage::Done,
        };
        let mut fields = format!(",\"id\":{id}");
        if let Some(ok) = ok {
            fields.push_str(&format!(",\"ok\":{ok}"));
        }
        if let Some((text, cut)) = &output {
            fields.push_str(&format!(",\"output\":{},\"cut\":{cut}", js(text)));
        }
        self.write(t, stage.as_str(), &fields);
        self.update(id, |r| {
            r.stage = stage;
            r.ended = Some(t);
            r.ok = ok;
            if let Some((text, cut)) = output {
                r.output = Some(text);
                r.cut = cut;
            }
        });
    }

    pub fn dropped(&mut self, id: MsgId, why: &str) {
        let t = now();
        self.write(t, "dropped", &format!(",\"id\":{id},\"why\":{}", js(why)));
        self.update(id, |r| {
            r.stage = Stage::Dropped;
            r.ended = Some(t);
            r.why = Some(why.to_string());
        });
    }

    /// Another machine would not take it (or could not be reached), after
    /// it was recorded as sent.
    pub fn refused(&mut self, id: MsgId, why: &str) {
        let t = now();
        self.write(t, "rejected", &format!(",\"id\":{id},\"why\":{}", js(why)));
        self.update(id, |r| {
            r.stage = Stage::Rejected;
            r.ended = Some(t);
            r.why = Some(why.to_string());
        });
    }

    /// Another machine took it: `stand` is what it said of it there.
    pub fn forwarded(&mut self, id: MsgId, stand: &str) {
        let t = now();
        self.write(t, "forwarded", &format!(",\"id\":{id},\"why\":{}", js(stand)));
        self.update(id, |r| {
            r.stage = Stage::Forwarded;
            r.ended = Some(t);
            r.why = Some(stand.to_string());
        });
    }

    /// Something about the links to other machines: `what` it was (paired,
    /// removed, allowed, refused) and its fields, already JSON.
    pub fn link(&mut self, what: &str, fields: &str) {
        self.write(now(), "link", &format!(",\"what\":\"{what}\"{fields}"));
    }

    /// A deleted message put back in its inbox.
    pub fn restored(&mut self, id: MsgId, by: &str) {
        self.write(now(), "restored", &format!(",\"id\":{id},\"by\":{}", js(by)));
        self.update(id, |r| {
            r.stage = Stage::Queued;
            r.ended = None;
            r.why = None;
        });
    }

    pub fn moved(&mut self, id: MsgId, from: usize, to: usize, by: &str) {
        self.write(now(), "moved", &format!(",\"id\":{id},\"from\":{from},\"to\":{to},\"by\":{}", js(by)));
    }

    /// Something about a pane: `what` it was (mode, name, status, idle,
    /// busy, created, exited) and its fields, already JSON (`,"k":v...`).
    pub fn pane(&mut self, address: &str, what: &str, fields: &str) {
        self.write(now(), "pane", &format!(",\"pane\":{},\"what\":\"{what}\"{fields}", js(address)));
    }

    pub fn get(&self, id: MsgId) -> Option<&Record> {
        self.records.get(&id)
    }

    pub fn records(&self) -> impl DoubleEndedIterator<Item = &Record> {
        self.records.values()
    }

    /// The highest message id known, so ids go on after a restart.
    pub fn last_id(&self) -> MsgId {
        self.records.keys().next_back().copied().unwrap_or(0)
    }

    /// Read back what the day files from `days` days ago to today say,
    /// after a restart. Messages that were still queued or being worked on
    /// are gone with the server that held them: they are dropped, and how
    /// many is returned.
    pub fn load(&mut self, days: u64) -> usize {
        let Some(dir) = self.dir.clone() else { return 0 };
        let today = now().date_naive();
        for back in (0..=days).rev() {
            let Some(day) = today.checked_sub_days(chrono::Days::new(back)) else { continue };
            self.load_file(&dir.join(format!("{}.jsonl", day.format("%Y-%m-%d"))));
        }
        let open: Vec<MsgId> = self.records.iter().filter(|(_, r)| !r.stage.finished()).map(|(id, _)| *id).collect();
        for id in &open {
            self.dropped(*id, "the server restarted");
        }
        self.trim();
        open.len()
    }

    fn load_file(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else { return };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let at = v["at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.into());
            let Some(at) = at else { continue };
            let ev = v["ev"].as_str().unwrap_or_default();
            // A message sent, or refused as it was sent, carries itself; one
            // refused later (another machine would not take it: `refused`)
            // carries its id, and ends the record that is there.
            if ev == "sent" || (ev == "rejected" && v.get("msg").is_some()) {
                if let Some(m) = message_of(&v["msg"], v["text"].as_str().unwrap_or_default(), at) {
                    let rejected = ev == "rejected";
                    self.records.insert(
                        m.id,
                        Record {
                            stage: if rejected { Stage::Rejected } else { Stage::Queued },
                            delivered: None,
                            ended: rejected.then_some(at),
                            ok: None,
                            output: None,
                            cut: false,
                            why: v["why"].as_str().map(String::from),
                            msg: m,
                        },
                    );
                }
                continue;
            }
            let Some(r) = v["id"].as_u64().and_then(|id| self.records.get_mut(&id)) else { continue };
            match ev {
                "delivered" | "read" => {
                    r.stage = if ev == "read" { Stage::Read } else { Stage::Delivered };
                    r.delivered = Some(at);
                }
                "restored" => {
                    r.stage = Stage::Queued;
                    r.ended = None;
                    r.why = None;
                }
                _ => {
                    if let Some(stage) = Stage::parse(ev).filter(|s| s.finished()) {
                        r.stage = stage;
                        r.ended = Some(at);
                        r.ok = v["ok"].as_bool();
                        r.output = v["output"].as_str().map(String::from);
                        r.cut = v["cut"].as_bool().unwrap_or(false);
                        r.why = v["why"].as_str().map(String::from);
                    }
                }
            }
        }
    }

    /// The message's life, for `trace-message`.
    pub fn trace(&self, id: MsgId, style: EnvelopeStyle) -> Option<String> {
        let r = self.records.get(&id)?;
        let m = &r.msg;
        let t = |x: Time| x.format("%H:%M:%S").to_string();
        let mut out = vec![format!("#{id} {} · task #{} · hop {}", r.stage.as_str(), m.task, m.hop), m.header(style)];
        out.push(format!("  sent       {}", t(m.at)));
        if let Some(d) = r.delivered {
            let how = if r.stage == Stage::Read { "read" } else { "delivered" };
            out.push(format!("  {how:<10} {}  (queued {})", t(d), since(m.at, d)));
        }
        if let Some(e) = r.ended {
            let took = r.delivered.map(|d| format!("  (took {})", since(d, e))).unwrap_or_default();
            out.push(format!("  {:<10} {}{took}", r.stage.as_str(), t(e)));
        }
        if let Some(why) = &r.why {
            out.push(format!("  why        {why}"));
        }
        out.push("text:".into());
        out.push(m.text.clone());
        if let Some(o) = &r.output {
            out.push(format!("output{}:", if r.cut { " (cut)" } else { "" }));
            out.push(o.clone());
        }
        Some(out.join("\n"))
    }

    /// A record as JSON (`trace-message -J`, `show-task -J`): what
    /// `trace` says, each part a field, the times as the clock shows them.
    pub fn record_json(r: &Record) -> serde_json::Value {
        let m = &r.msg;
        let t = |x: Time| x.format("%H:%M:%S").to_string();
        serde_json::json!({
            "id": m.id,
            "task": m.task,
            "hop": m.hop,
            "stage": r.stage.as_str(),
            "from": m.from.from_field(),
            "name": m.from.name(),
            "to": m.to,
            "via": m.via.as_str(),
            "re": m.re,
            "sent": t(m.at),
            "delivered": r.delivered.map(t),
            "read": r.stage == Stage::Read,
            "ended": r.ended.map(t),
            "queued": r.delivered.map(|d| since(m.at, d)),
            "took": r.delivered.zip(r.ended).map(|(d, e)| since(d, e)),
            "ok": r.ok,
            "why": r.why,
            "text": m.text,
            "output": r.output,
            "cut": r.cut,
        })
    }

    /// Every task (a chain of messages), newest first: (task, stage word,
    /// where it is now, how long, how many messages, its title).
    pub fn tasks(&self) -> Vec<TaskSummary> {
        let mut by: BTreeMap<MsgId, Vec<&Record>> = BTreeMap::new();
        for r in self.records.values() {
            by.entry(r.msg.task).or_default().push(r);
        }
        let mut out: Vec<TaskSummary> = by
            .into_iter()
            .map(|(task, rs)| {
                let first = rs.iter().map(|r| r.msg.at).min().unwrap_or_else(now);
                let open: Vec<&&Record> = rs.iter().filter(|r| !r.stage.finished()).collect();
                let last = rs.iter().filter_map(|r| r.ended).max();
                let status = if !open.is_empty() {
                    "running"
                } else if rs.iter().any(|r| matches!(r.stage, Stage::Rejected | Stage::Failed)) {
                    "failed"
                } else if rs.iter().any(|r| matches!(r.stage, Stage::Dropped | Stage::Abandoned)) {
                    "cancelled"
                } else {
                    "done"
                };
                let at = open.last().map(|r| format!("{} ({})", r.msg.to, r.msg.via.as_str()));
                let root = rs.iter().find(|r| r.msg.id == task).unwrap_or(&rs[0]);
                TaskSummary {
                    task,
                    status,
                    at,
                    took: since(first, if open.is_empty() { last.unwrap_or(first) } else { now() }),
                    steps: rs.len(),
                    title: root.msg.text.lines().next().unwrap_or_default().to_string(),
                    started: first,
                }
            })
            .collect();
        out.sort_by(|a, b| b.started.cmp(&a.started).then(b.task.cmp(&a.task)));
        out
    }

    /// A task's messages in the order they were sent.
    pub fn task(&self, task: MsgId) -> Vec<&Record> {
        let mut v: Vec<&Record> = self.records.values().filter(|r| r.msg.task == task).collect();
        v.sort_by_key(|r| (r.msg.at, r.msg.id));
        v
    }
}

pub struct TaskSummary {
    pub task: MsgId,
    pub status: &'static str,
    pub at: Option<String>,
    pub took: String,
    pub steps: usize,
    pub title: String,
    pub started: Time,
}

/// How long from `a` to `b`, the way the status line says it.
pub fn since(a: Time, b: Time) -> String {
    crate::format::command_duration((b - a).num_milliseconds().max(0))
}

/// A message back from its envelope, as the event log holds it.
fn message_of(env: &serde_json::Value, text: &str, at: Time) -> Option<Message> {
    let mode = |k: &str| env[k].as_str().and_then(WorkMode::parse);
    let name = env["name"].as_str().map(String::from);
    let from = match env["from"].as_str()? {
        "user" => Sender::User,
        remote if crate::link::split_remote(remote).is_some() => {
            let (addr, address) = crate::link::split_remote(remote).expect("checked");
            Sender::Remote { addr: addr.to_string(), address: address.to_string(), name, mode: mode("mode") }
        }
        address => Sender::Pane {
            id: address.rsplit_once('%').and_then(|(_, n)| n.parse().ok()).unwrap_or(0),
            address: address.to_string(),
            name,
            mode: mode("mode").unwrap_or_default(),
        },
    };
    Some(Message {
        id: env["id"].as_u64()?,
        task: env["task"].as_u64()?,
        from,
        to: env["to"].as_str()?.to_string(),
        via: mode("via")?,
        hop: env["hop"].as_u64()? as u32,
        re: env["re"].as_u64(),
        text: text.to_string(),
        at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: MsgId, task: MsgId, text: &str) -> Message {
        Message {
            id,
            task,
            from: Sender::Pane { id: 3, address: "$1:@1.%3".into(), name: Some("lead".into()), mode: WorkMode::Ai },
            to: "$1:@2.%9".into(),
            via: WorkMode::Shell,
            hop: 1,
            re: Some(4),
            text: text.into(),
            at: chrono::SubsecRound::trunc_subsecs(now(), 3),
        }
    }

    #[test]
    fn a_message_goes_through_its_stages() {
        let mut s = Store::default();
        s.sent(&msg(5, 5, "cargo test\nsecond line"), None);
        assert_eq!(s.get(5).unwrap().stage, Stage::Queued);
        s.delivered(5);
        assert_eq!(s.get(5).unwrap().stage, Stage::Delivered);
        s.ended(5, End::Done, Some(false), Some(("error: 1 failed".into(), false)));
        let r = s.get(5).unwrap();
        assert_eq!((r.stage, r.ok, r.output.as_deref()), (Stage::Failed, Some(false), Some("error: 1 failed")));
        let t = s.trace(5, EnvelopeStyle::Json).unwrap();
        assert!(t.starts_with("#5 failed · task #5 · hop 1"), "{t}");
        assert!(t.contains("error: 1 failed") && t.contains("cargo test"), "{t}");
        assert_eq!(s.last_id(), 5);
    }

    #[test]
    fn the_last_lines_come_from_memory() {
        let dir = std::env::temp_dir().join(format!("keepane-observe-recent-{}", std::process::id()));
        let mut s = Store { dir: Some(dir.clone()), ..Default::default() };
        for i in 1..=RECENT as u64 + 10 {
            s.delivered(i);
        }
        assert_eq!(s.recent.len(), RECENT, "bounded");
        let last = s.recent(3, |_| true);
        assert_eq!(last.len(), 3);
        assert!(last[2].contains(&format!("\"id\":{}", RECENT + 10)), "newest last: {last:?}");
        assert!(s.recent(10, |l| l.contains("\"id\":5}")).is_empty(), "the oldest have left memory");
        assert_eq!(s.recent(10, |l| l.contains("\"id\":42}")).len(), 1, "a filter looks further back");
        // Off: nothing is kept, in memory or on disk.
        s.dir = None;
        s.delivered(1);
        assert_eq!(s.recent(1, |_| true), last[2..]);
        crate::histlog::flush(std::time::Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tasks_group_the_chain_and_say_how_it_went() {
        let mut s = Store::default();
        s.sent(&msg(1, 1, "run the tests\nand report"), None);
        s.delivered(1);
        s.sent(&msg(2, 1, "cargo test"), None);
        s.sent(&msg(3, 3, "other"), Some("inbox is full"));
        let tasks = s.tasks();
        let one = tasks.iter().find(|t| t.task == 1).unwrap();
        assert_eq!((one.status, one.steps, one.title.as_str()), ("running", 2, "run the tests"));
        assert_eq!(tasks.iter().find(|t| t.task == 3).unwrap().status, "failed");
        s.ended(1, End::Done, None, None);
        s.dropped(2, "deleted by user");
        assert_eq!(s.tasks().iter().find(|t| t.task == 1).unwrap().status, "cancelled");
        assert_eq!(s.task(1).iter().map(|r| r.msg.id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn the_files_bring_it_back_and_what_was_open_is_dropped() {
        let dir = std::env::temp_dir().join(format!("keepane-observe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store { dir: Some(dir.clone()), ..Default::default() };
        s.sent(&msg(7, 7, "done one"), None);
        s.delivered(7);
        s.ended(7, End::Done, Some(true), Some(("ok".into(), false)));
        s.sent(&msg(8, 7, "still queued"), None);
        s.sent(&msg(9, 9, "refused"), Some("hop limit 8"));
        // From another machine, and one handed to another machine (link.md).
        let mut remote = msg(10, 10, "from afar");
        remote.from = Sender::Remote {
            addr: "100.64.0.3:7681".into(),
            address: "$2:@5.%8".into(),
            name: Some("lead".into()),
            mode: Some(WorkMode::Ai),
        };
        s.sent(&remote, None);
        s.delivered(10);
        s.ended(10, End::Done, None, None);
        let mut away = msg(11, 11, "far away");
        away.to = "100.64.0.3:7681/%worker".into();
        s.sent(&away, None);
        s.forwarded(11, "#4 queued for $1:@0.%1 (ai, busy, 0 ahead) on 100.64.0.3:7681");
        s.sent(&msg(12, 12, "unreachable"), None);
        s.refused(12, "100.64.0.3:7681 is not reachable");
        crate::histlog::flush(std::time::Duration::from_secs(5));

        let mut back = Store { dir: Some(dir.clone()), ..Default::default() };
        assert_eq!(back.load(1), 1, "the queued one is gone with the old server");
        assert_eq!(back.last_id(), 12);
        assert_eq!(back.get(10).unwrap().msg, remote, "the remote sender reads back as it was");
        let r = back.get(11).unwrap();
        assert_eq!(r.stage, Stage::Forwarded, "handed over is done with here, not dropped at a restart");
        assert!(r.why.as_deref().unwrap().starts_with("#4 queued"), "{:?}", r.why);
        assert_eq!(back.get(12).unwrap().stage, Stage::Rejected);
        let r = back.get(7).unwrap();
        assert_eq!((r.stage, r.ok, r.output.as_deref()), (Stage::Done, Some(true), Some("ok")));
        assert_eq!(r.msg, s.get(7).unwrap().msg.clone(), "the envelope reads back as the message");
        assert_eq!(back.get(8).unwrap().stage, Stage::Dropped);
        assert_eq!(back.get(8).unwrap().why.as_deref(), Some("the server restarted"));
        assert_eq!(back.get(9).unwrap().stage, Stage::Rejected);
        crate::histlog::flush(std::time::Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
