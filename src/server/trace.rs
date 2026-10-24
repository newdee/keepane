//! What the agent in a pane did, step by step, as its transcripts tell it
//! (`trace-pane`, the dashboard's Trace): each turn (the person's prompt to
//! the agent's last reply), and in it each call of the model and each tool
//! it ran, from when to when, and whether it failed; a Claude subagent's
//! own steps under the call that started it.
//!
//! A transcript line is written when what it tells is over: a model call
//! shows once its reply is written, from the step before it. While a turn
//! runs and no tool does, the model is at work: from the last step to now.

use serde_json::Value;
use std::collections::HashMap;

/// What a step is, by the tool it ran: the dashboard colours each kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Turn,
    Model,
    /// Reading and searching: files, the web.
    Read,
    /// Changing files.
    Edit,
    /// Running a command.
    Run,
    /// Another agent (a Claude subagent).
    Agent,
    /// Any other tool (a to-do list, an MCP server's).
    Tool,
}

impl SpanKind {
    pub fn word(self) -> &'static str {
        match self {
            SpanKind::Turn => "turn",
            SpanKind::Model => "model",
            SpanKind::Read => "read",
            SpanKind::Edit => "edit",
            SpanKind::Run => "run",
            SpanKind::Agent => "agent",
            SpanKind::Tool => "tool",
        }
    }

    /// A tool's kind by its name, as Claude Code, Codex and pi call their
    /// tools (an MCP tool by its own name, after the server's).
    pub fn of_tool(name: &str) -> SpanKind {
        let lower = name.to_ascii_lowercase();
        let n = lower.rsplit("__").next().unwrap_or(&lower);
        match n {
            "read" | "grep" | "glob" | "ls" | "find" | "search" | "webfetch" | "websearch" | "web_search"
            | "notebookread" | "read_file" | "list_dir" | "grep_files" | "view_image" | "toolsearch" => SpanKind::Read,
            "edit" | "multiedit" | "write" | "notebookedit" | "apply_patch" | "write_file" | "str_replace" => {
                SpanKind::Edit
            }
            "bash" | "bashoutput" | "killshell" | "powershell" | "shell" | "exec_command" | "local_shell"
            | "local_shell_call" | "write_stdin" | "unified_exec" => SpanKind::Run,
            "task" | "agent" => SpanKind::Agent,
            _ => SpanKind::Tool,
        }
    }
}

/// One step, its times in milliseconds since 1970; `end` none while it runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub id: String,
    /// The turn it is a step of, or the agent call it is a subagent's step of.
    pub parent: Option<String>,
    pub kind: SpanKind,
    /// The tool's name, the model's, or a turn's prompt.
    pub name: String,
    /// What it was given, short: a command, a file, a pattern.
    pub detail: String,
    pub start: i64,
    pub end: Option<i64>,
    pub failed: bool,
    /// Tokens the model wrote (a model call).
    pub tokens: u64,
}

impl Span {
    pub fn json(&self) -> Value {
        serde_json::json!({
            "id": self.id, "parent": self.parent, "kind": self.kind.word(), "name": self.name,
            "detail": self.detail, "start": self.start, "end": self.end, "failed": self.failed,
            "tokens": self.tokens,
        })
    }
}

/// The most steps kept of one transcript: the oldest turns go first.
const MOST: usize = 3000;

/// One transcript's steps, read line by line as it grows.
#[derive(Clone, Debug, Default)]
pub struct Steps {
    pub spans: Vec<Span>,
    index: HashMap<String, usize>,
    /// The turn running, by its id.
    turn: Option<String>,
    turns: u32,
    /// When the last step was (a model call runs from there).
    last: i64,
    /// A tool call's id to the subagent it started (Claude's `agentId:`).
    pub agent_of: HashMap<String, String>,
    /// What the next turn begun without a prompt answers: a line the agent
    /// was given that is no prompt (a background task done, a message from
    /// another agent).
    pending: Option<String>,
}

/// A transcript line's time, milliseconds since 1970.
fn ms(v: &Value) -> Option<i64> {
    let t = v["timestamp"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(t).ok().map(|t| t.timestamp_millis())
}

/// The first line of `s`, trimmed, at most `n` characters.
fn short(s: &str, n: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() > n { line.chars().take(n - 1).collect::<String>() + "…" } else { line.to_string() }
}

/// What a tool was given, short: the command, else the file, the pattern,
/// the address, what it is for. A patch: the first file it changes.
pub fn detail_of(input: &Value) -> String {
    let input = match input {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                let file = s.lines().find_map(|l| {
                    ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|p| l.strip_prefix(p))
                });
                return short(file.unwrap_or(s), 120);
            }
        },
        v => v.clone(),
    };
    for key in ["command", "cmd", "file_path", "path", "pattern", "query", "url", "description", "prompt"] {
        match &input[key] {
            Value::String(s) if !s.trim().is_empty() => return short(s, 120),
            Value::Array(a) if !a.is_empty() => {
                let words: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
                // `["bash", "-lc", "cargo test"]`: what the shell runs.
                let run = match words.as_slice() {
                    [_, flag, rest @ ..] if flag.starts_with('-') && !rest.is_empty() => rest.join(" "),
                    _ => words.join(" "),
                };
                return short(&run, 120);
            }
            _ => {}
        }
    }
    String::new()
}

impl Steps {
    fn add(&mut self, s: Span) {
        if let Some(&i) = self.index.get(&s.id) {
            self.spans[i] = s;
            return;
        }
        self.index.insert(s.id.clone(), self.spans.len());
        self.spans.push(s);
        if self.spans.len() > MOST {
            self.forget_oldest_turn();
        }
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Span> {
        self.index.get(id).copied().map(|i| &mut self.spans[i])
    }

    /// The oldest turn and its steps let go.
    fn forget_oldest_turn(&mut self) {
        let Some(first) = self.spans.iter().find(|s| s.kind == SpanKind::Turn).map(|s| s.id.clone()) else {
            self.spans.drain(..self.spans.len() / 2);
            self.reindex();
            return;
        };
        self.spans.retain(|s| s.id != first && s.parent.as_deref() != Some(&first));
        self.reindex();
    }

    fn reindex(&mut self) {
        self.index = self.spans.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
    }

    /// A turn begins at `t`, with the person's prompt; the one running ends.
    fn begin(&mut self, t: i64, prompt: &str) {
        // One begun by a stray line (a tool's answer after its turn ended)
        // with no prompt and nothing done in it: not a turn.
        if let Some(id) = self.turn.clone()
            && self.get_mut(&id).is_some_and(|s| s.name.is_empty())
            && !self.spans.iter().any(|s| s.parent.as_deref() == Some(&id))
        {
            self.spans.retain(|s| s.id != id);
            self.reindex();
            self.turn = None;
        }
        self.end_turn(t, false);
        self.turns += 1;
        let id = format!("turn-{}", self.turns);
        self.add(Span {
            id: id.clone(),
            parent: None,
            kind: SpanKind::Turn,
            name: short(prompt, 100),
            detail: String::new(),
            start: t,
            end: None,
            failed: false,
            tokens: 0,
        });
        self.turn = Some(id);
        self.last = t;
    }

    /// The turn running ends at `t` (broken off by the person: `cut`), and
    /// whatever of it still runs with it.
    fn end_turn(&mut self, t: i64, cut: bool) {
        let Some(id) = self.turn.take() else { return };
        for s in self.spans.iter_mut().filter(|s| s.end.is_none() && (s.id == id || s.parent.as_deref() == Some(&id))) {
            s.end = Some(t.max(s.start));
            if cut && s.id == id {
                s.detail = "interrupted".into();
            }
        }
        self.last = t;
    }

    /// The turn running, begun now when there is none (a transcript taken
    /// up again, or read from its middle).
    fn turn_at(&mut self, t: i64) -> String {
        if self.turn.is_none() {
            let why = self.pending.take().unwrap_or_default();
            self.begin(t, &why);
        }
        self.turn.clone().unwrap_or_default()
    }

    /// The model's reply came at `t` (`id`: the reply's, its lines one
    /// call): its call ran from the step before. Nothing between two
    /// replies: one call, longer.
    fn model(&mut self, t: i64, id: Option<&str>, model: &str, tokens: u64) {
        let key = id.map(|i| format!("m-{i}"));
        if let Some(s) = key.as_deref().and_then(|k| self.get_mut(k)) {
            s.end = Some(t.max(s.start));
            s.tokens = s.tokens.max(tokens);
            self.last = t;
            return;
        }
        let (last, running) = (self.last, self.turn.clone());
        if let Some(s) = self.spans.last_mut().filter(|s| {
            s.kind == SpanKind::Model && s.end.is_some_and(|e| e >= last) && running.is_some() && s.parent == running
        }) {
            s.end = Some(t.max(s.start));
            s.tokens += tokens;
            self.last = t;
            return;
        }
        let turn = self.turn_at(t);
        let n = self.spans.len();
        self.add(Span {
            id: key.unwrap_or_else(|| format!("m-{t}-{n}")),
            parent: Some(turn),
            kind: SpanKind::Model,
            name: model.to_string(),
            detail: String::new(),
            start: self.last.min(t),
            end: Some(t),
            failed: false,
            tokens,
        });
        self.last = t;
    }

    /// A tool called at `t`.
    fn tool(&mut self, t: i64, id: &str, name: &str, input: &Value) {
        let turn = self.turn_at(t);
        self.add(Span {
            id: id.to_string(),
            parent: Some(turn),
            kind: SpanKind::of_tool(name),
            name: name.to_string(),
            detail: detail_of(input),
            start: t,
            end: None,
            failed: false,
            tokens: 0,
        });
        self.last = t;
    }

    /// A tool's answer came at `t`.
    fn answered(&mut self, t: i64, id: &str, failed: bool) {
        if let Some(s) = self.get_mut(id) {
            s.end = Some(t.max(s.start));
            s.failed = failed;
        }
        self.last = t;
    }

    /// One transcript line.
    pub fn feed(&mut self, kind: super::agents::Kind, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let Some(t) = ms(&v) else { return };
        match kind {
            super::agents::Kind::Claude => self.claude(&v, t),
            super::agents::Kind::Codex => self.codex(&v, t),
            super::agents::Kind::Pi => self.pi(&v, t),
        }
    }

    fn claude(&mut self, v: &Value, t: i64) {
        let m = &v["message"];
        match v["type"].as_str() {
            // A line the agent adds itself (`isMeta`) is no prompt, unless it
            // starts a turn of its own: a scheduled one.
            Some("user") if v["isMeta"].as_bool() != Some(true) || v["turnOrigin"].is_string() => {
                if let Value::String(s) = &m["content"] {
                    match v["turnOrigin"].as_str() {
                        Some(origin) if !s.trim_start().starts_with('<') => {
                            self.pending = None;
                            self.begin(t, &format!("({origin}) {}", s.trim()));
                        }
                        _ => self.prompt(t, s),
                    }
                    return;
                }
                for b in m["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("tool_result") => {
                            let id = b["tool_use_id"].as_str().unwrap_or_default();
                            let text = match &b["content"] {
                                Value::String(s) => s.clone(),
                                Value::Array(a) => {
                                    a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n")
                                }
                                _ => String::new(),
                            };
                            if let Some(agent) = text.split("agentId: ").nth(1) {
                                let agent: String = agent.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                                self.agent_of.insert(id.to_string(), agent);
                            }
                            self.answered(t, id, b["is_error"].as_bool() == Some(true));
                        }
                        Some("text") => {
                            if let Some(s) = b["text"].as_str() {
                                self.prompt(t, s);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                let model = m["model"].as_str().unwrap_or_default();
                let tokens = m["usage"]["output_tokens"].as_u64().unwrap_or(0);
                let blocks: Vec<&Value> = m["content"].as_array().into_iter().flatten().collect();
                // Its thinking and text are the model's; a tool call is the
                // model's up to it, then the tool's.
                if !model.starts_with('<') {
                    self.model(t, m["id"].as_str(), model, tokens);
                }
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    self.tool(
                        t,
                        b["id"].as_str().unwrap_or_default(),
                        b["name"].as_str().unwrap_or("tool"),
                        &b["input"],
                    );
                }
                if matches!(m["stop_reason"].as_str(), Some("end_turn" | "stop_sequence" | "max_tokens")) {
                    self.end_turn(t, false);
                }
            }
            // Queued while it worked (an older Claude Code): what the turn
            // it begins answers.
            Some("queue-operation") if v["operation"] == "enqueue" => {
                let text = v["content"].as_str().unwrap_or_default().trim_start();
                self.pending =
                    if text.starts_with('<') { given(text) } else { Some(short(text, 100)) }.filter(|s| !s.is_empty());
            }
            // What came while the agent worked, given to it once it could
            // take it (your message, a background task done): it begins the
            // turn when none runs; one that runs goes on.
            Some("attachment") if v["attachment"]["type"] == "queued_command" && self.turn.is_none() => {
                let text = v["attachment"]["prompt"].as_str().unwrap_or_default().trim_start();
                if text.starts_with('<') {
                    if let Some(why) = given(text) {
                        self.begin(t, &why);
                    }
                } else if !text.is_empty() {
                    self.begin(t, text);
                }
            }
            _ => {}
        }
    }

    /// A person's text: a prompt begins a turn; their breaking it off ends
    /// one; what the agent wrote itself (`<command-name>…`) is neither.
    fn prompt(&mut self, t: i64, text: &str) {
        let first = text.trim_start();
        if first.starts_with("[Request interrupted by user") {
            self.end_turn(t, true);
        } else if first.starts_with('<') {
            // Not the person's: what a turn it starts answers.
            if let Some(why) = given(first) {
                self.pending = Some(why);
            }
        } else if !first.is_empty() {
            self.pending = None;
            self.begin(t, first);
        }
    }

    fn codex(&mut self, v: &Value, t: i64) {
        let p = &v["payload"];
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("event_msg"), Some("task_started")) => self.begin(t, ""),
            (Some("event_msg"), Some("user_message")) => {
                let text = p["message"].as_str().unwrap_or_default();
                let turn = self.turn_at(t);
                if let Some(s) = self.get_mut(&turn).filter(|s| s.name.is_empty()) {
                    s.name = short(text, 100);
                }
                // The model works on it from here.
                self.last = t;
            }
            (Some("event_msg"), Some(end @ ("task_complete" | "turn_aborted"))) => {
                self.end_turn(t, end == "turn_aborted")
            }
            (Some("response_item"), Some("reasoning")) => self.model(t, None, "", 0),
            (Some("response_item"), Some("message")) if p["role"] == "assistant" => self.model(t, None, "", 0),
            (Some("response_item"), Some(call @ ("function_call" | "custom_tool_call" | "local_shell_call"))) => {
                self.model(t, None, "", 0);
                let id = p["call_id"].as_str().or_else(|| p["id"].as_str()).unwrap_or_default();
                let name = p["name"].as_str().unwrap_or(if call == "local_shell_call" { "shell" } else { "tool" });
                let input = if call == "local_shell_call" {
                    &p["action"]
                } else if p["arguments"].is_null() {
                    &p["input"]
                } else {
                    &p["arguments"]
                };
                self.tool(t, id, name, input);
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                let id = p["call_id"].as_str().unwrap_or_default();
                self.answered(t, id, codex_failed(&p["output"]));
            }
            _ => {}
        }
    }

    fn pi(&mut self, v: &Value, t: i64) {
        if v["type"] != "message" {
            return;
        }
        let m = &v["message"];
        let text = |c: &Value| match c {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        match m["role"].as_str() {
            Some("user") => self.begin(t, &text(&m["content"])),
            Some("assistant") => {
                self.model(
                    t,
                    None,
                    m["model"].as_str().unwrap_or_default(),
                    m["usage"]["output"].as_u64().unwrap_or(0),
                );
                for b in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "toolCall") {
                    self.tool(
                        t,
                        b["id"].as_str().unwrap_or_default(),
                        b["name"].as_str().unwrap_or("tool"),
                        &b["arguments"],
                    );
                }
                match m["stopReason"].as_str() {
                    Some("toolUse") | None => {}
                    Some(stop) => self.end_turn(t, stop == "aborted"),
                }
            }
            Some("toolResult") => {
                self.answered(t, m["toolCallId"].as_str().unwrap_or_default(), m["isError"].as_bool() == Some(true))
            }
            _ => {}
        }
    }

    /// Whether a turn runs and no tool does: the model is at work since
    /// the last step.
    pub fn thinking(&self) -> Option<i64> {
        let turn = self.turn.as_deref()?;
        let tool_running =
            self.spans.iter().any(|s| s.end.is_none() && s.kind != SpanKind::Turn && s.parent.as_deref() == Some(turn));
        (!tool_running).then_some(self.last)
    }

    /// The turn running, by its id.
    pub fn running(&self) -> Option<&str> {
        self.turn.as_deref()
    }
}

/// What a line the agent was given (not the person's prompt) is, to name the
/// turn it starts: a background task done (its summary), another agent's
/// message, a command; none for what starts no work of its own (a reminder
/// the agent adds, a command's output).
fn given(text: &str) -> Option<String> {
    let inner = |tag: &str| {
        let open = format!("<{tag}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{tag}>"))? + start;
        Some(text[start..end].trim().to_string())
    };
    let tag: String = text[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    match tag.as_str() {
        "task-notification" => Some(match inner("summary") {
            Some(s) if !s.is_empty() => short(&s, 100),
            _ => "a background task done".into(),
        }),
        "agent-message" => {
            let from = text.split("from=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("an agent");
            Some(format!("a message from {from}"))
        }
        "command-name" => inner("command-name"),
        "system-reminder" | "local-command-stdout" | "local-command-stderr" | "command-message" | "command-args" => {
            None
        }
        "" => None,
        other => Some(other.replace('-', " ")),
    }
}

/// Whether a Codex tool's output says it failed: an exit code not 0.
fn codex_failed(output: &Value) -> bool {
    let text = match output {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    if let Ok(v) = serde_json::from_str::<Value>(&text)
        && let Some(code) = v["metadata"]["exit_code"].as_i64()
    {
        return code != 0;
    }
    text.lines()
        .find_map(|l| {
            l.trim().strip_prefix("Exit code: ").or_else(|| l.trim().strip_prefix("Process exited with code "))
        })
        .and_then(|c| c.trim().parse::<i64>().ok())
        .is_some_and(|c| c != 0)
}

/// A pane's steps from its agent's transcripts (`own`, oldest first) and
/// its subagents' (`helpers`, by their agent's id), the latest `turns`
/// turns, ids made one each across the files; a subagent's steps under the
/// call that started it (else under the agent call running when it began).
/// While a turn runs and no tool does, a model call running from its last
/// step to `now`.
pub fn merge(own: &[&Steps], helpers: &[(String, &Steps)], turns: usize, now: i64) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let renamed = |file: usize, id: &str| format!("{file}:{id}");
    for (fi, steps) in own.iter().enumerate() {
        for s in &steps.spans {
            out.push(Span { id: renamed(fi, &s.id), parent: s.parent.as_ref().map(|p| renamed(fi, p)), ..s.clone() });
        }
        if let (Some(turn), Some(since)) = (steps.running(), steps.thinking()) {
            out.push(Span {
                id: renamed(fi, "thinking"),
                parent: Some(renamed(fi, turn)),
                kind: SpanKind::Model,
                name: String::new(),
                detail: String::new(),
                start: since.min(now),
                end: None,
                failed: false,
                tokens: 0,
            });
        }
    }
    // The latest turns only, and their steps.
    let mut keep: Vec<String> = out.iter().filter(|s| s.kind == SpanKind::Turn).map(|s| s.id.clone()).collect();
    let cut = keep.len().saturating_sub(turns);
    keep.drain(..cut);
    out.retain(|s| match s.kind {
        SpanKind::Turn => keep.contains(&s.id),
        _ => s.parent.as_ref().is_some_and(|p| keep.contains(p)),
    });
    // Each agent call's id, by the subagent it started.
    let mut call_of: HashMap<String, String> = HashMap::new();
    for (fi, steps) in own.iter().enumerate() {
        for (call, agent) in &steps.agent_of {
            call_of.insert(agent.clone(), renamed(fi, call));
        }
    }
    for (hi, (agent, steps)) in helpers.iter().enumerate() {
        let first = steps.spans.first().map_or(i64::MAX, |s| s.start);
        let parent = call_of.get(agent).filter(|c| out.iter().any(|s| &s.id == *c)).cloned().or_else(|| {
            out.iter()
                .filter(|s| s.kind == SpanKind::Agent && s.start <= first)
                .max_by_key(|s| s.start)
                .map(|s| s.id.clone())
        });
        let Some(parent) = parent else { continue };
        let file = format!("h{hi}");
        for s in steps.spans.iter().filter(|s| s.kind != SpanKind::Turn) {
            out.push(Span { id: format!("{file}:{}", s.id), parent: Some(parent.clone()), ..s.clone() });
        }
    }
    out.sort_by_key(|s| (s.start, s.kind != SpanKind::Turn));
    out
}

/// How long a step took, short: `45ms`, `4.1s`, `39s`, `3m12s`.
pub fn took(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 10_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        let s = ms / 1000;
        if s < 60 { format!("{s}s") } else { format!("{}m{:02}s", s / 60, s % 60) }
    } else {
        crate::format::human_duration(ms / 1000)
    }
}

/// The steps as a tree, a line each: a turn, then its steps indented (a
/// subagent's under its call): kind, name, what it was given, how long,
/// `running` for one not over, `failed`.
pub fn tree(spans: &[Span], now: i64) -> Vec<String> {
    fn walk(spans: &[Span], parent: Option<&str>, depth: usize, now: i64, out: &mut Vec<String>) {
        for s in spans.iter().filter(|s| s.parent.as_deref() == parent) {
            let mut line = format!("{}{} ", "  ".repeat(depth), s.kind.word());
            for part in [s.name.as_str(), s.detail.as_str()].into_iter().filter(|p| !p.is_empty()) {
                line.push_str(part);
                line.push(' ');
            }
            line.push_str(&took(s.end.unwrap_or(now) - s.start));
            if s.end.is_none() {
                line.push_str(" running");
            }
            if s.failed {
                line.push_str(" failed");
            }
            out.push(line);
            walk(spans, Some(&s.id), depth + 1, now, out);
        }
    }
    let mut out = Vec::new();
    walk(spans, None, 0, now, &mut out);
    out
}
#[cfg(test)]
mod tests;
