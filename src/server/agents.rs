//! The agents in the panes: what each has used and cost so far, how full
//! its context is, how busy its processes are (`list-agents`, the `agent_*`
//! formats, the dashboard, the page). Only agents keepane runs: a pane's
//! process, or one under it, that is Claude Code, Codex or pi.
//!
//! The numbers come from the transcripts each agent writes for its work in
//! a directory (read as they grow, from where the last read stopped) and
//! from the process table; a token's price from `prices` (pi says its own
//! cost). A look takes a moment (files, processes), so it runs on a thread
//! of its own, every two seconds, and the server keeps the last answer.

use super::layout::PaneId;
use super::prices::Prices;
use super::{Event, Server};
use crate::format::{human_count, human_dollars};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the panes' agents are looked at.
const EVERY: Duration = Duration::from_secs(2);

/// The agents keepane reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    #[default]
    Claude,
    Codex,
    Pi,
}

impl Kind {
    /// As the formats and `list-agents` write it.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Pi => "pi",
        }
    }

    /// The agent a process is: by its program's name, or, for one a script
    /// runner runs (node, bun), by what its command line runs. Either path
    /// separator, on any system.
    pub fn of(name: &str, command_line: impl FnOnce() -> Option<String>) -> Option<Kind> {
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
        let stem = base.strip_suffix(".exe").unwrap_or(&base);
        match stem {
            "claude" => return Some(Kind::Claude),
            "codex" => return Some(Kind::Codex),
            "pi" => return Some(Kind::Pi),
            "node" | "bun" | "deno" => {}
            _ => return None,
        }
        let line = command_line()?.to_ascii_lowercase().replace('\\', "/");
        if line.contains("@anthropic-ai/claude-code") || line.contains("/claude-code/") {
            Some(Kind::Claude)
        } else if line.contains("@openai/codex") {
            Some(Kind::Codex)
        } else if line.contains("pi-coding-agent") {
            Some(Kind::Pi)
        } else {
            None
        }
    }
}

/// Tokens by what was done with them: sent fresh, read from the cache,
/// written to it (of those, kept an hour), received.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
}

impl Tokens {
    /// All of them, each counted once.
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }

    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_write_1h += o.cache_write_1h;
        self.output += o.output;
    }
}

/// What an agent's transcripts add up to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// The model of the last reply.
    pub model: String,
    pub tokens: Tokens,
    /// The same tokens, by the model that used them (each at its price).
    pub by_model: BTreeMap<String, Tokens>,
    /// What the agent itself says its replies cost (pi), summed; None for
    /// one that does not say.
    pub cost: Option<f64>,
    /// The tokens the last reply was given (its context), and the model's
    /// window when the agent says it (Codex).
    pub context: u64,
    pub window: Option<u64>,
    /// Tools called, replies made.
    pub tools: u64,
    pub turns: u64,
}

impl Usage {
    /// `other`'s counts added to these; with `latest`, its model and
    /// context taken too (it is the newer of the agent's own transcripts,
    /// not a helper's).
    fn add(&mut self, other: &Usage, latest: bool) {
        self.tokens.add(&other.tokens);
        for (m, t) in &other.by_model {
            self.by_model.entry(m.clone()).or_default().add(t);
        }
        self.tools += other.tools;
        self.turns += other.turns;
        self.cost = match (self.cost, other.cost) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
        if latest && !other.model.is_empty() {
            self.model = other.model.clone();
            self.context = other.context;
            self.window = other.window;
        }
    }

    /// Dollars: the agent's own figure, else each model's tokens at its
    /// price (nothing used yet: 0); None when it says none and no model it
    /// used is priced (or there is no list). With it, whether some tokens
    /// went unpriced (a model not on the list): the cost is then of the
    /// rest, at least that much.
    pub fn priced(&self, prices: Option<&Prices>) -> (Option<f64>, bool) {
        if self.cost.is_some() {
            return (self.cost, false);
        }
        let used: Vec<(&String, &Tokens)> = self.by_model.iter().filter(|(_, t)| t.total() > 0).collect();
        if used.is_empty() {
            return (Some(0.0), false);
        }
        let Some(prices) = prices else { return (None, false) };
        let priced: Vec<f64> = used.iter().filter_map(|(m, t)| prices.of(m).map(|p| p.cost(t))).collect();
        let partial = priced.len() < used.len();
        if priced.is_empty() { (None, false) } else { (Some(priced.iter().sum()), partial) }
    }

    /// One reply's tokens, under its model (or the last one named).
    fn count(&mut self, model: Option<&str>, t: Tokens) {
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            self.model = m.to_string();
        }
        self.tokens.add(&t);
        self.by_model.entry(self.model.clone()).or_default().add(&t);
    }
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// One line of a transcript into `u`. `seen` keeps the replies counted: a
/// Claude reply comes as several lines (its thinking, its text, each tool
/// call), each carrying the same usage.
pub fn feed(kind: Kind, line: &str, u: &mut Usage, seen: &mut HashSet<String>) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    match kind {
        Kind::Claude => claude(&v, u, seen),
        Kind::Codex => codex(&v, u),
        Kind::Pi => pi(&v, u),
    }
}

fn claude(v: &Value, u: &mut Usage, seen: &mut HashSet<String>) {
    if v["type"] != "assistant" {
        return;
    }
    let m = &v["message"];
    // Each line holds its own blocks: its tool calls count, whatever line.
    u.tools += m["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "tool_use").count() as u64);
    let id = m["id"].as_str().or_else(|| v["uuid"].as_str()).unwrap_or_default();
    if !seen.insert(id.to_string()) {
        return;
    }
    // The real counts are per iteration when there are any (the top level
    // reads 0 then); the context is the last one's. A reply keepane's agent
    // made up itself (`<synthetic>`) used no model.
    let usage = &m["usage"];
    let parts: Vec<&Value> = match usage["iterations"].as_array() {
        Some(it) if !it.is_empty() => it.iter().collect(),
        _ => vec![usage],
    };
    let mut t = Tokens::default();
    for p in &parts {
        t.input += n(&p["input_tokens"]);
        t.cache_read += n(&p["cache_read_input_tokens"]);
        t.cache_write += n(&p["cache_creation_input_tokens"]);
        t.cache_write_1h += n(&p["cache_creation"]["ephemeral_1h_input_tokens"]);
        t.output += n(&p["output_tokens"]);
    }
    let model = m["model"].as_str().filter(|m| !m.starts_with('<'));
    if model.is_none() && m["model"].is_string() {
        return;
    }
    u.count(model, t);
    if let Some(last) = parts.last() {
        u.context =
            n(&last["input_tokens"]) + n(&last["cache_read_input_tokens"]) + n(&last["cache_creation_input_tokens"]);
    }
    u.turns += 1;
}

fn codex(v: &Value, u: &mut Usage) {
    let p = &v["payload"];
    match (v["type"].as_str(), p["type"].as_str()) {
        (Some("turn_context"), _) => {
            if let Some(m) = p["model"].as_str() {
                u.model = m.to_string();
            }
        }
        // Running totals: what they say now is all of it, put down to the
        // model of the moment. Its input counts the cached part too.
        (Some("event_msg"), Some("token_count")) if !p["info"].is_null() => {
            let total = &p["info"]["total_token_usage"];
            let cached = n(&total["cached_input_tokens"]);
            u.tokens = Tokens {
                input: n(&total["input_tokens"]).saturating_sub(cached),
                cache_read: cached,
                output: n(&total["output_tokens"]),
                ..Tokens::default()
            };
            u.by_model = BTreeMap::from([(u.model.clone(), u.tokens)]);
            u.context = n(&p["info"]["last_token_usage"]["input_tokens"]);
            u.window = p["info"]["model_context_window"].as_u64();
        }
        (Some("event_msg"), Some("task_started")) => u.turns += 1,
        (Some("response_item"), Some("function_call" | "custom_tool_call" | "local_shell_call")) => u.tools += 1,
        _ => {}
    }
}

fn pi(v: &Value, u: &mut Usage) {
    if v["type"] == "model_change" {
        if let Some(m) = v["modelId"].as_str() {
            u.model = m.to_string();
        }
        return;
    }
    let m = &v["message"];
    if v["type"] != "message" || m["role"] != "assistant" {
        return;
    }
    let us = &m["usage"];
    let t = Tokens {
        input: n(&us["input"]),
        cache_read: n(&us["cacheRead"]),
        cache_write: n(&us["cacheWrite"]),
        cache_write_1h: 0,
        output: n(&us["output"]),
    };
    u.count(m["model"].as_str(), t);
    if let Some(c) = us["cost"]["total"].as_f64() {
        u.cost = Some(u.cost.unwrap_or(0.0) + c);
    }
    u.context = t.input + t.cache_read + t.cache_write;
    u.tools += m["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "toolCall").count() as u64);
    u.turns += 1;
}

/// One transcript, read as it grows.
pub struct Tail {
    kind: Kind,
    pub path: PathBuf,
    /// A helper's (a Claude subagent's): its tokens count, its model and
    /// context are not the agent's.
    helper: bool,
    offset: u64,
    partial: Vec<u8>,
    seen: HashSet<String>,
    pub usage: Usage,
}

impl Tail {
    pub fn new(kind: Kind, path: PathBuf) -> Tail {
        Tail {
            kind,
            path,
            helper: false,
            offset: 0,
            partial: Vec::new(),
            seen: HashSet::new(),
            usage: Usage::default(),
        }
    }

    /// What was written since the last read, whole lines only. A file that
    /// got shorter was written anew: read from the start.
    pub fn read(&mut self) {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            let helper = self.helper;
            *self = Tail::new(self.kind, std::mem::take(&mut self.path));
            self.helper = helper;
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if (&mut f).take(len - self.offset).read_to_end(&mut buf).is_err() {
            return;
        }
        self.offset += buf.len() as u64;
        self.partial.extend(buf);
        // One pass over the lines, then what is left of the last one kept
        // (a line taken off the front each time would move the rest each
        // time: a long transcript read in minutes, not a moment).
        let mut start = 0;
        while let Some(i) = self.partial[start..].iter().position(|b| *b == b'\n') {
            if let Ok(s) = std::str::from_utf8(&self.partial[start..start + i]) {
                feed(self.kind, s.trim_end(), &mut self.usage, &mut self.seen);
            }
            start += i + 1;
        }
        self.partial.drain(..start);
    }
}

/// How Claude Code names a project's folder: each character of the path
/// that is not a letter or a digit a `-`.
pub fn claude_folder(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// How pi names it: the path without its first separator, each separator
/// and `:` a `-`, between `--` and `--`.
pub fn pi_folder(cwd: &str) -> String {
    let rest = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    format!("--{}--", rest.replace(['/', '\\', ':'], "-"))
}

/// A directory as compared with another: case matters only off Windows; a
/// trailing separator never.
fn dir_key(d: &str) -> String {
    let d = d.trim_end_matches(['/', '\\']);
    if cfg!(windows) { d.to_ascii_lowercase() } else { d.to_string() }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The `.jsonl` files right in `dir` written to since `since` (all of them
/// with None).
fn jsonl_in(dir: &Path, since: Option<SystemTime>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter(|p| since.is_none_or(|s| modified(p).is_some_and(|m| m >= s)))
        .collect()
}

/// The transcripts `kind` wrote for work in `cwd` since `since`, oldest
/// write first. `home` and `env` say where the agents keep them.
pub fn transcripts(
    kind: Kind,
    cwd: &str,
    since: SystemTime,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut files = match kind {
        Kind::Claude => {
            let dir = set("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
            jsonl_in(&dir.join("projects").join(claude_folder(cwd)), Some(since))
        }
        Kind::Pi => jsonl_in(&home.join(".pi").join("agent").join("sessions").join(pi_folder(cwd)), Some(since)),
        Kind::Codex => {
            // A folder a day (local dates): the days since `since`, a week
            // at most; each session says its directory on its first line.
            let dir = set("CODEX_HOME").unwrap_or_else(|| home.join(".codex")).join("sessions");
            let today = chrono::Local::now().date_naive();
            let from: chrono::DateTime<chrono::Local> = since.into();
            let mut day = from.date_naive().max(today - chrono::Days::new(7));
            let want = dir_key(cwd);
            let mut out = Vec::new();
            while day <= today {
                let d = dir
                    .join(day.format("%Y").to_string())
                    .join(day.format("%m").to_string())
                    .join(day.format("%d").to_string());
                out.extend(
                    jsonl_in(&d, Some(since)).into_iter().filter(|p| codex_cwd(p).is_some_and(|c| dir_key(&c) == want)),
                );
                day = day + chrono::Days::new(1);
            }
            out
        }
    };
    files.sort_by_key(|p| modified(p));
    files
}

/// The directory a Codex session worked in: its first line, `session_meta`.
fn codex_cwd(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(f), &mut first).ok()?;
    let v: Value = serde_json::from_str(first.trim()).ok()?;
    v["payload"]["cwd"].as_str().map(str::to_string)
}

/// The transcripts of a Claude session's helpers (its subagents): beside
/// the session's file, in a folder of its name.
fn helpers_of(session: &Path) -> Vec<PathBuf> {
    let Some(stem) = session.file_stem() else { return Vec::new() };
    jsonl_in(&session.with_file_name(stem).join("subagents"), None)
}

/// A helper's transcript among a session's own: older Claude Code kept its
/// subagents' beside the sessions, as `agent-<id>.jsonl` (a session's own
/// is named by its id).
fn is_helper(kind: Kind, path: &Path) -> bool {
    kind == Kind::Claude && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("agent-"))
}

/// A pane's agent, as the formats and `list-agents` give it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub kind: Kind,
    /// The agent's process.
    pub pid: u32,
    pub usage: Usage,
    /// Dollars so far; None when not known (or `agent-cost off`).
    pub cost: Option<f64>,
    /// Some tokens were of a model the price list does not have: `cost` is
    /// of the rest, the least it can be.
    pub cost_partial: bool,
    /// The context's share of the window, percent, when the window is known.
    pub context_pct: Option<u32>,
    /// The agent's processes (it and those under it): CPU, percent of one
    /// core, since the last look; memory held, bytes.
    pub cpu: f64,
    pub mem: u64,
    /// Transcripts read (its own and its helpers').
    pub files: usize,
}

impl Stats {
    /// The cost as it reads: `$2.10`, `$2.10+` when some tokens went
    /// unpriced, empty when not known.
    pub fn cost_text(&self) -> String {
        let more = if self.cost_partial { "+" } else { "" };
        self.cost.map(|c| format!("{}{more}", human_dollars(c))).unwrap_or_default()
    }

    /// How full the context is: `34%`, or its size when the window is not
    /// known, empty before the first reply.
    pub fn context_text(&self) -> String {
        match self.context_pct {
            Some(p) => format!("{p}%"),
            None if self.usage.context > 0 => human_count(self.usage.context),
            None => String::new(),
        }
    }

    /// As `list-agents -J` gives it: every figure a number (cost and the
    /// context's share null when not known).
    pub fn json(&self, pane: &str, target: &str) -> Value {
        let t = &self.usage.tokens;
        serde_json::json!({
            "pane": pane,
            "target": target,
            "agent": self.kind.word(),
            "pid": self.pid,
            "model": self.usage.model,
            // To a millionth of a dollar: a sum of many products has a
            // float's noise in its last digits (223.07633699999997).
            "cost": self.cost.map(|c| (c * 1e6).round() / 1e6),
            "cost_partial": self.cost_partial,
            "tokens": {
                "input": t.input, "cache_read": t.cache_read, "cache_write": t.cache_write,
                "output": t.output, "total": t.total(),
            },
            "context": self.usage.context,
            "context_pct": self.context_pct,
            "cpu": (self.cpu * 10.0).round() / 10.0,
            "mem": self.mem,
            "tools": self.usage.tools,
            "turns": self.usage.turns,
            "transcripts": self.files,
        })
    }

    /// The `agent_*` formats.
    pub fn fill(&self, ctx: &mut crate::format::Context) {
        ctx.agent = self.kind.word().to_string();
        ctx.agent_model = self.usage.model.clone();
        ctx.agent_cost = self.cost_text();
        ctx.agent_tokens = human_count(self.usage.tokens.total());
        ctx.agent_context = self.context_text();
        ctx.agent_cpu = format!("{:.0}%", self.cpu);
        ctx.agent_mem = crate::sysinfo::human_bytes(self.mem);
        ctx.agent_tools = self.usage.tools.to_string();
        ctx.agent_turns = self.usage.turns.to_string();
    }
}

/// A pane as the watcher needs it.
#[derive(Clone, Debug)]
pub struct PaneInfo {
    pub id: PaneId,
    /// The pane's own process.
    pub pid: u32,
    /// Its directory, when the agent's own cannot be read.
    pub path: String,
}

struct Track {
    kind: Kind,
    agent: u32,
    started: u64,
    cwd: String,
    tails: Vec<Tail>,
    cpu_at: Option<(u64, Instant)>,
    cpu: f64,
    mem: u64,
}

/// Looks at the panes' agents, keeping what it read between looks.
#[derive(Default)]
pub struct Watcher {
    tracks: HashMap<PaneId, Track>,
    /// What each process seen is (a node running something else is asked
    /// its command line once, not every look).
    kinds: HashMap<u32, Option<Kind>>,
}

impl Watcher {
    /// Each pane's agent now, if it runs one. `prices` prices the tokens of
    /// an agent that does not say its cost; without it, such an agent's
    /// cost is not known (pi's own figure, and nothing used yet, still are).
    pub fn look(&mut self, panes: &[PaneInfo], prices: Option<&Prices>) -> HashMap<PaneId, Stats> {
        let home = dirs::home_dir().unwrap_or_default();
        let env = |k: &str| std::env::var(k).ok();
        self.find(panes);
        self.sample();
        let owner = self.owners(&home, &env);
        let mut out = HashMap::new();
        for (id, t) in self.tracks.iter_mut() {
            let mine: HashSet<&PathBuf> = owner.iter().filter(|(_, o)| *o == id).map(|(f, _)| f).collect();
            let mut want: Vec<(PathBuf, bool)> = mine.iter().map(|f| ((*f).clone(), is_helper(t.kind, f))).collect();
            if t.kind == Kind::Claude {
                want.extend(mine.iter().flat_map(|f| helpers_of(f)).map(|h| (h, true)));
            }
            t.tails.retain(|tail| want.iter().any(|(p, _)| *p == tail.path));
            for (p, helper) in want {
                if !t.tails.iter().any(|tail| tail.path == p) {
                    let mut tail = Tail::new(t.kind, p);
                    tail.helper = helper;
                    t.tails.push(tail);
                }
            }
            for tail in &mut t.tails {
                tail.read();
            }
            // Oldest first, so the newest of its own gives the model and
            // the context.
            t.tails.sort_by_key(|tail| modified(&tail.path));
            let mut usage = Usage::default();
            for tail in &t.tails {
                usage.add(&tail.usage, !tail.helper);
            }
            let price = prices.and_then(|p| p.of(&usage.model));
            let window = usage.window.or_else(|| price.and_then(|p| p.window));
            let context_pct = window.filter(|w| *w > 0).map(|w| (usage.context.min(w) * 100 / w) as u32);
            let (cost, cost_partial) = usage.priced(prices);
            let stats = Stats {
                kind: t.kind,
                pid: t.agent,
                cost,
                cost_partial,
                context_pct,
                cpu: t.cpu,
                mem: t.mem,
                files: t.tails.len(),
                usage,
            };
            out.insert(*id, stats);
        }
        out
    }

    /// Each pane's agent: the one found before while it still runs there,
    /// else the first agent process in the pane's tree, if any.
    fn find(&mut self, panes: &[PaneInfo]) {
        let alive: HashSet<PaneId> = panes.iter().map(|p| p.id).collect();
        self.tracks.retain(|id, _| alive.contains(id));
        let mut present = HashSet::new();
        for p in panes {
            let tree = crate::sysinfo::tree_of(p.pid);
            present.extend(tree.iter().map(|(pid, _)| *pid));
            if self.tracks.get(&p.id).is_some_and(|t| tree.iter().any(|(pid, _)| *pid == t.agent)) {
                continue;
            }
            self.tracks.remove(&p.id);
            let kinds = &mut self.kinds;
            let found = tree.iter().find_map(|(pid, name)| {
                let kind = match kinds.get(pid) {
                    Some(k) => *k,
                    None => {
                        // Kept only when known: a command line not read yet
                        // (a process just started) is asked again next look.
                        let mut read = true;
                        let k = Kind::of(name, || {
                            let line = crate::proccwd::process_command_line(*pid);
                            read = line.is_some();
                            line
                        });
                        if read {
                            kinds.insert(*pid, k);
                        }
                        k
                    }
                };
                kind.map(|k| (*pid, k))
            });
            let Some((agent, kind)) = found else { continue };
            let Some((_, _, started)) = crate::sysinfo::usage_of(agent) else { continue };
            let cwd = crate::proccwd::process_cwd(agent).unwrap_or_else(|| p.path.clone());
            let track = Track { kind, agent, started, cwd, tails: Vec::new(), cpu_at: None, cpu: 0.0, mem: 0 };
            self.tracks.insert(p.id, track);
        }
        self.kinds.retain(|pid, _| present.contains(pid));
    }

    /// The CPU and memory of each agent's processes.
    fn sample(&mut self) {
        for t in self.tracks.values_mut() {
            let (mut cpu_ns, mut mem) = (0u64, 0u64);
            for (pid, _) in crate::sysinfo::tree_of(t.agent) {
                if let Some((c, m, _)) = crate::sysinfo::usage_of(pid) {
                    cpu_ns += c;
                    mem += m;
                }
            }
            let now = Instant::now();
            if let Some((before, at)) = t.cpu_at {
                let wall = now.duration_since(at).as_nanos() as f64;
                if wall > 0.0 {
                    t.cpu = cpu_ns.saturating_sub(before) as f64 * 100.0 / wall;
                }
            }
            t.cpu_at = Some((cpu_ns, now));
            t.mem = mem;
        }
    }

    /// Each transcript to one agent: of those of its kind working in its
    /// directory, the one started last before the transcript began (one
    /// begun before any of them started was resumed: the first one's).
    fn owners(&self, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> HashMap<PathBuf, PaneId> {
        let mut groups: BTreeMap<(u8, String), Vec<PaneId>> = BTreeMap::new();
        for (id, t) in &self.tracks {
            groups.entry((t.kind as u8, dir_key(&t.cwd))).or_default().push(*id);
        }
        let mut owner = HashMap::new();
        for ids in groups.values_mut() {
            ids.sort_by_key(|id| (self.tracks[id].started, *id));
            let first = &self.tracks[&ids[0]];
            let since = UNIX_EPOCH + Duration::from_secs(first.started.saturating_sub(5));
            for file in transcripts(first.kind, &first.cwd, since, home, env) {
                let began =
                    std::fs::metadata(&file).and_then(|m| m.created().or_else(|_| m.modified())).map_or(0, secs);
                let pick = ids.iter().rev().find(|id| self.tracks[*id].started <= began + 5).unwrap_or(&ids[0]);
                owner.insert(file, *pick);
            }
        }
        owner
    }
}

/// What the watcher's thread is asked: the panes, and the prices to use.
struct Ask {
    panes: Vec<PaneInfo>,
    prices: Option<Arc<Prices>>,
}

#[derive(Default)]
pub(super) struct Agents {
    /// The last look's answer.
    pub stats: HashMap<PaneId, Stats>,
    to_watcher: Option<std::sync::mpsc::Sender<Ask>>,
    asking: bool,
    last: Option<Instant>,
}

impl Server {
    /// Every tick: ask the watcher to look again when it is time.
    pub(super) fn agents_tick(&mut self) {
        if self.agents.asking || self.agents.last.is_some_and(|l| l.elapsed() < EVERY) {
            return;
        }
        let panes: Vec<PaneInfo> = self
            .sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .filter(|p| p.exit_code.is_none())
            .filter_map(|p| Some(PaneInfo { id: p.id, pid: p.pid?, path: p.cwd.clone().unwrap_or_default() }))
            .collect();
        // The list gives a model's window too: kept for that with cost off
        // (cost is then left out when the answer comes).
        let prices = self.prices.prices.clone();
        let tx = match &self.agents.to_watcher {
            Some(tx) => tx.clone(),
            None => {
                let (tx, rx) = std::sync::mpsc::channel::<Ask>();
                let events = self.events.clone();
                std::thread::spawn(move || {
                    let mut w = Watcher::default();
                    while let Ok(ask) = rx.recv() {
                        // A look that panics (a bug) answers nothing and
                        // starts over, rather than leave the server waiting.
                        let look = std::panic::AssertUnwindSafe(|| w.look(&ask.panes, ask.prices.as_deref()));
                        let stats = match std::panic::catch_unwind(look) {
                            Ok(stats) => stats,
                            Err(_) => {
                                log::error!("agents: a look failed; starting over");
                                w = Watcher::default();
                                HashMap::new()
                            }
                        };
                        if events.send(Event::AgentStats(stats)).is_err() {
                            break;
                        }
                    }
                });
                self.agents.to_watcher = Some(tx.clone());
                tx
            }
        };
        self.agents.last = Some(Instant::now());
        self.agents.asking = tx.send(Ask { panes, prices }).is_ok();
        if !self.agents.asking {
            // Its thread is gone: a new one next time.
            self.agents.to_watcher = None;
        }
    }

    /// `list-agents [-J]`: each pane's agent, in the order of the sessions,
    /// their windows and the windows' panes.
    pub(super) fn list_agents(&self, json: bool) -> super::Outcome {
        let mut rows = Vec::new();
        for s in &self.sessions {
            for (wi, w) in s.windows.iter().enumerate() {
                for (pi, id) in w.layout.panes().iter().enumerate() {
                    if let Some(a) = self.agents.stats.get(id) {
                        let at = format!("{}:{}.{}", s.name, wi + self.opts.base_index, pi + self.opts.pane_base_index);
                        rows.push((format!("%{id}"), at, a));
                    }
                }
            }
        }
        if json {
            let list: Vec<Value> = rows.iter().map(|(pane, at, a)| a.json(pane, at)).collect();
            return super::Outcome::Text(Value::Array(list).to_string());
        }
        if rows.is_empty() {
            return super::Outcome::Ok;
        }
        let dash = |s: String| if s.is_empty() { "-".to_string() } else { s };
        let lines: Vec<String> = rows
            .iter()
            .map(|(pane, at, a)| {
                format!(
                    "{pane} {at} {} {} cost {} tokens {} context {} cpu {:.0}% mem {} tools {} turns {}",
                    a.kind.word(),
                    dash(a.usage.model.clone()),
                    dash(a.cost_text()),
                    human_count(a.usage.tokens.total()),
                    dash(a.context_text()),
                    a.cpu,
                    crate::sysinfo::human_bytes(a.mem),
                    a.usage.tools,
                    a.usage.turns,
                )
            })
            .collect();
        super::Outcome::Text(lines.join("\n"))
    }

    pub(super) fn agents_seen(&mut self, stats: HashMap<PaneId, Stats>) {
        self.agents.asking = false;
        if !self.opts.agent_cost {
            // Cost off: none shown, the agent's own figure neither.
            let stats = stats.into_iter().map(|(id, s)| (id, Stats { cost: None, cost_partial: false, ..s }));
            self.agents.stats = stats.collect();
        } else {
            self.agents.stats = stats;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(kind: Kind, lines: &[&str]) -> Usage {
        let (mut u, mut seen) = (Usage::default(), HashSet::new());
        for l in lines {
            feed(kind, l, &mut u, &mut seen);
        }
        u
    }

    fn claude_reply(id: &str, model: &str, blocks: &[&str], usage: &str) -> String {
        let blocks: Vec<String> = blocks.iter().map(|b| format!(r#"{{"type":"{b}"}}"#)).collect();
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"{model}","content":[{}],"usage":{usage}}}}}"#,
            blocks.join(",")
        )
    }

    #[test]
    fn claude_counts_each_reply_once_from_its_iterations() {
        // A reply in three lines (thinking, text, a tool call), each with the
        // same usage; the top level reads 0, the iteration holds the counts.
        let usage = concat!(
            r#"{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"#,
            r#""iterations":[{"input_tokens":2,"output_tokens":300,"cache_read_input_tokens":1000,"#,
            r#""cache_creation_input_tokens":50,"cache_creation":{"ephemeral_1h_input_tokens":50}}]}"#
        );
        let line = |b: &str| claude_reply("msg_1", "claude-opus-5-5", &[b], usage);
        let (a, b, c) = (line("thinking"), line("text"), line("tool_use"));
        let second = claude_reply(
            "msg_2",
            "claude-haiku-4-5",
            &["tool_use", "tool_use"],
            r#"{"input_tokens":3,"output_tokens":10,"cache_read_input_tokens":1300,"cache_creation_input_tokens":0}"#,
        );
        let synthetic = claude_reply("msg_3", "<synthetic>", &["text"], r#"{"input_tokens":0,"output_tokens":0}"#);
        let u = feed_all(Kind::Claude, &[&a, &b, &c, r#"{"type":"user"}"#, "not json", &second, &synthetic]);
        let t = u.tokens;
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write, t.cache_write_1h), (5, 310, 2300, 50, 50));
        assert_eq!(u.turns, 2, "two replies; the made-up one is none");
        assert_eq!(u.tools, 3, "a tool call in one line of the first, two in the second");
        assert_eq!(u.context, 1303, "the last reply's: fresh, cached, written");
        assert_eq!(u.model, "claude-haiku-4-5");
        assert_eq!(u.by_model["claude-opus-5-5"].output, 300, "each model's own");
        assert_eq!(u.by_model["claude-haiku-4-5"].output, 10);
        assert_eq!(t.total(), 5 + 310 + 2300 + 50);
    }

    #[test]
    fn codex_takes_its_running_totals() {
        let tc = |input: u64, cached: u64, out: u64, last: u64| {
            format!(
                concat!(
                    r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":"#,
                    r#"{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"#,
                    r#""last_token_usage":{{"input_tokens":{}}},"model_context_window":258400}}}}}}"#
                ),
                input, cached, out, last
            )
        };
        let u = feed_all(
            Kind::Codex,
            &[
                r#"{"type":"session_meta","payload":{"cwd":"C:\\src"}}"#,
                r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
                &tc(1000, 600, 50, 1000),
                r#"{"type":"response_item","payload":{"type":"function_call"}}"#,
                &tc(3000, 2000, 120, 2000),
            ],
        );
        let t = u.tokens;
        assert_eq!((t.input, t.cache_read, t.output), (1000, 2000, 120), "the last totals, input less its cached part");
        assert_eq!((u.context, u.window), (2000, Some(258400)));
        assert_eq!((u.turns, u.tools), (1, 1));
        assert_eq!(u.model, "gpt-5.5");
        assert_eq!(u.by_model, BTreeMap::from([("gpt-5.5".to_string(), t)]), "all of it, once");
    }

    #[test]
    fn pi_says_its_own_cost() {
        let reply = |cost: f64| {
            format!(
                concat!(
                    r#"{{"type":"message","message":{{"role":"assistant","model":"claude-sonnet-5","#,
                    r#""content":[{{"type":"toolCall"}}],"#,
                    r#""usage":{{"input":10,"output":20,"cacheRead":100,"cacheWrite":5,"cost":{{"total":{}}}}}}}}}"#
                ),
                cost
            )
        };
        let u = feed_all(Kind::Pi, &[r#"{"type":"session","cwd":"/src"}"#, &reply(0.25), &reply(0.5)]);
        assert_eq!(u.cost, Some(0.75));
        let t = u.tokens;
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (20, 40, 200, 10));
        assert_eq!((u.turns, u.tools, u.context), (2, 2, 115));
        let none = Prices::default();
        assert_eq!(u.priced(Some(&none)), (Some(0.75), false), "its own figure, priced or not");
        assert_eq!(u.priced(None), (Some(0.75), false), "with no list at all too");
    }

    #[test]
    fn cost_is_nothing_before_a_reply_and_marked_when_partly_unpriced() {
        let list = r#"{"m": {"input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06}}"#;
        let prices = Prices::from_litellm(list, 0).unwrap();
        let fresh = Usage::default();
        assert_eq!(fresh.priced(Some(&prices)), (Some(0.0), false));
        assert_eq!(fresh.priced(None), (Some(0.0), false), "nothing used costs nothing, list or not");
        let mut u = Usage::default();
        u.count(Some("m"), Tokens { input: 1_000_000, ..Tokens::default() });
        assert_eq!(u.priced(Some(&prices)), (Some(1.0), false));
        assert_eq!(u.priced(None), (None, false), "no list: not known");
        u.count(Some("unknown"), Tokens { input: 5, ..Tokens::default() });
        assert_eq!(u.priced(Some(&prices)), (Some(1.0), true), "the rest priced, marked");
        let s = Stats { cost: Some(1.0), cost_partial: true, ..Stats::default() };
        assert_eq!(s.cost_text(), "$1.00+");
        assert_eq!(Stats::default().cost_text(), "");
        let mut only = Usage::default();
        only.count(Some("unknown"), Tokens { output: 5, ..Tokens::default() });
        assert_eq!(only.priced(Some(&prices)), (None, false), "nothing priced: not known");
    }

    #[test]
    fn helpers_add_their_tokens_but_not_their_context() {
        let mut main = Usage { model: "claude-opus-5-5".into(), context: 900, ..Usage::default() };
        main.count(None, Tokens { input: 10, ..Tokens::default() });
        let mut helper = Usage { model: "claude-haiku-4-5".into(), context: 50, turns: 3, ..Usage::default() };
        helper.count(None, Tokens { output: 7, ..Tokens::default() });
        let mut sum = Usage::default();
        sum.add(&main, true);
        sum.add(&helper, false);
        assert_eq!((sum.model.as_str(), sum.context), ("claude-opus-5-5", 900));
        assert_eq!((sum.tokens.input, sum.tokens.output, sum.turns), (10, 7, 3));
        assert_eq!(sum.by_model.len(), 2);
        assert_eq!(sum.cost, None, "neither says its cost");
    }

    #[test]
    fn a_tail_reads_whole_lines_as_they_come() {
        let dir = std::env::temp_dir().join(format!("keepane-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.jsonl");
        let reply = |id: &str| claude_reply(id, "m", &[], r#"{"input_tokens":1,"output_tokens":2}"#);
        std::fs::write(&f, format!("{}\n{}", reply("a"), &reply("b")[..20])).unwrap();
        let mut t = Tail::new(Kind::Claude, f.clone());
        t.read();
        assert_eq!(t.usage.turns, 1, "the half-written line waits");
        std::fs::write(&f, format!("{}\n{}\n", reply("a"), reply("b"))).unwrap();
        t.read();
        assert_eq!(t.usage.turns, 2);
        t.read();
        assert_eq!(t.usage.turns, 2, "nothing new, nothing counted again");
        std::fs::write(&f, format!("{}\n", reply("c"))).unwrap();
        t.read();
        assert_eq!(t.usage.turns, 1, "written anew: read from the start");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transcripts_are_found_where_each_agent_keeps_them() {
        let home = std::env::temp_dir().join(format!("keepane-agents-home-{}", std::process::id()));
        let cwd = if cfg!(windows) { r"C:\work\proj" } else { "/work/proj" };
        let claude = home.join(".claude").join("projects").join(claude_folder(cwd));
        let pi = home.join(".pi").join("agent").join("sessions").join(pi_folder(cwd));
        let today = chrono::Local::now().date_naive();
        let codex = home.join(".codex").join("sessions").join(today.format("%Y/%m/%d").to_string());
        for d in [&claude, &pi, &codex, &claude.join("s1").join("subagents")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(claude.join("s1.jsonl"), "").unwrap();
        std::fs::write(claude.join("notes.txt"), "").unwrap();
        std::fs::write(claude.join("s1").join("subagents").join("agent-a.jsonl"), "").unwrap();
        std::fs::write(pi.join("p.jsonl"), "").unwrap();
        let meta = |dir: &str| format!("{}\n", serde_json::json!({"type":"session_meta","payload":{"cwd": dir}}));
        std::fs::write(codex.join("rollout-mine.jsonl"), meta(&format!("{cwd}/"))).unwrap();
        std::fs::write(codex.join("rollout-other.jsonl"), meta("/elsewhere")).unwrap();
        let none = |_: &str| None;
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        let names = |v: Vec<PathBuf>| -> Vec<String> {
            v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
        };
        assert_eq!(names(transcripts(Kind::Claude, cwd, hour_ago, &home, &none)), ["s1.jsonl"]);
        assert_eq!(names(helpers_of(&claude.join("s1.jsonl"))), ["agent-a.jsonl"]);
        // Older Claude Code: a helper's beside the sessions.
        assert!(is_helper(Kind::Claude, &claude.join("agent-1f2e.jsonl")));
        assert!(
            !is_helper(Kind::Claude, &claude.join("0c6d-4b1a.jsonl"))
                && !is_helper(Kind::Pi, &pi.join("agent-x.jsonl"))
        );
        assert_eq!(names(transcripts(Kind::Pi, cwd, hour_ago, &home, &none)), ["p.jsonl"]);
        assert_eq!(names(transcripts(Kind::Codex, cwd, hour_ago, &home, &none)), ["rollout-mine.jsonl"]);
        let later = SystemTime::now() + Duration::from_secs(3600);
        assert!(transcripts(Kind::Claude, cwd, later, &home, &none).is_empty(), "none written since");
        // CLAUDE_CONFIG_DIR moves Claude's.
        let moved = |k: &str| (k == "CLAUDE_CONFIG_DIR").then(|| home.join("nowhere").to_string_lossy().into_owned());
        assert!(transcripts(Kind::Claude, cwd, hour_ago, &home, &moved).is_empty());
        std::fs::remove_dir_all(&home).ok();
    }

    /// Two agents of a kind in one directory: a transcript begun after both
    /// started is the one started last's (the one started first had begun
    /// its own before).
    #[test]
    fn a_new_transcript_is_the_latest_started_agents() {
        let home = std::env::temp_dir().join(format!("keepane-agents-owner-{}", std::process::id()));
        let cwd = if cfg!(windows) { r"C:\work\two" } else { "/work/two" };
        let dir = home.join(".claude").join("projects").join(claude_folder(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("new.jsonl"), "").unwrap();
        let now = secs(SystemTime::now());
        let track = |started: u64| Track {
            kind: Kind::Claude,
            agent: 0,
            started,
            cwd: cwd.to_string(),
            tails: Vec::new(),
            cpu_at: None,
            cpu: 0.0,
            mem: 0,
        };
        let mut w = Watcher::default();
        w.tracks.insert(1, track(now - 100));
        w.tracks.insert(2, track(now - 50));
        let owner = w.owners(&home, &|_| None);
        assert_eq!(owner.get(&dir.join("new.jsonl")), Some(&2), "{owner:?}");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_formats_say_it_as_it_reads() {
        let mut usage =
            Usage { model: "claude-opus-5-5".into(), tools: 42, turns: 10, context: 340_000, ..Usage::default() };
        usage.tokens = Tokens { input: 1_000, cache_read: 1_200_000, output: 3_000, ..Tokens::default() };
        let s = Stats {
            usage,
            cost: Some(2.1),
            cost_partial: false,
            context_pct: Some(34),
            cpu: 12.4,
            mem: 812 * 1024 * 1024,
            ..Stats::default()
        };
        let mut ctx = crate::format::Context::default();
        s.fill(&mut ctx);
        let got = [
            &ctx.agent,
            &ctx.agent_model,
            &ctx.agent_cost,
            &ctx.agent_tokens,
            &ctx.agent_context,
            &ctx.agent_cpu,
            &ctx.agent_mem,
            &ctx.agent_tools,
            &ctx.agent_turns,
        ];
        assert_eq!(got, ["claude", "claude-opus-5-5", "$2.10", "1.2M", "34%", "12%", "812M", "42", "10"]);
        let no_window = Stats { context_pct: None, ..s };
        assert_eq!(no_window.context_text(), "340k", "its size when the window is not known");
    }

    #[test]
    fn list_agents_json_gives_numbers() {
        let s = Stats { cost: Some(221.948615 + 1.127722), context_pct: Some(30), ..Stats::default() };
        let v = s.json("%2", "a:0.0");
        assert_eq!(v["cost"], serde_json::json!(223.076337), "to a millionth, no float noise");
        assert_eq!((v["agent"].as_str(), v["context_pct"].as_u64()), (Some("claude"), Some(30)));
        let none = Stats::default().json("%2", "a:0.0");
        assert!(none["cost"].is_null() && none["context_pct"].is_null(), "not known: null");
    }

    #[test]
    fn folder_names_and_directories() {
        assert_eq!(claude_folder(r"C:\Users\me\src\x.y"), "C--Users-me-src-x-y");
        assert_eq!(claude_folder("/home/me/src"), "-home-me-src");
        assert_eq!(pi_folder(r"C:\Users\me\src"), "--C--Users-me-src--");
        assert_eq!(pi_folder("/home/me/src"), "--home-me-src--");
        assert_eq!(dir_key(r"C:\src\"), dir_key(r"C:\src"));
        assert_eq!(dir_key("/a/b/"), dir_key("/a/b"));
    }

    #[test]
    fn which_process_is_an_agent() {
        let none = || None;
        assert_eq!(Kind::of("claude.exe", none), Some(Kind::Claude));
        assert_eq!(Kind::of("/usr/local/bin/codex", none), Some(Kind::Codex));
        assert_eq!(Kind::of(r"C:\bin\pi.EXE", none), Some(Kind::Pi));
        assert_eq!(Kind::of("pwsh", || Some("anything".into())), None);
        let cl = |s: &str| {
            let s = s.to_string();
            move || Some(s)
        };
        let claude = r"node C:\npm\node_modules\@anthropic-ai\claude-code\cli.js";
        assert_eq!(Kind::of("node.exe", cl(claude)), Some(Kind::Claude));
        let pi = "node /usr/lib/node_modules/@mariozechner/pi-coding-agent/dist/cli.js";
        assert_eq!(Kind::of("node", cl(pi)), Some(Kind::Pi));
        assert_eq!(Kind::of("node", cl("node /x/@openai/codex/bin/codex.js")), Some(Kind::Codex));
        assert_eq!(
            Kind::of("bun", cl("bun x @anthropic-ai/claude-code")),
            Some(Kind::Claude),
            "run by its package name"
        );
        assert_eq!(Kind::of("node", cl("node server.js")), None);
        assert_eq!(Kind::of("node", none), None, "a command line not read: not an agent");
    }

    /// The watcher on this machine's processes: a pane whose process (this
    /// test) runs no agent has none.
    #[test]
    fn a_pane_without_an_agent_has_none() {
        let mut w = Watcher::default();
        let me = PaneInfo { id: 1, pid: std::process::id(), path: String::new() };
        assert!(w.look(&[me], None).is_empty());
        assert!(w.look(&[], None).is_empty(), "no panes, no agents");
    }
}
