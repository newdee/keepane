//! When a pane is done, and who is told (`done-events`): a command that ran
//! `done-after` seconds or more ended, an agent's turn ended, a message's
//! task ended, or the pane's program exited. Kept for the phone's page
//! (`list-done`), raised on the desktop with `notify`, handed to the
//! `pane-done` hook (in `KEEPANE_DONE_*` variables) and sent to
//! `done-webhook`, as keepane's JSON, plain text, or the body a chat's
//! incoming webhook takes (Feishu, WeCom, DingTalk, Slack, Discord).
//!
//! The webhook goes out through `curl`, as `update` and `public_ip` do: its
//! address and body on curl's standard input (`-K -`), so neither shows in
//! the list of processes, on a thread of its own, so a slow chat never holds
//! the server up.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::actor::WorkMode;
use super::{Event, PaneId, Server};

/// How many are kept for `list-done` and the phone.
pub const KEPT: usize = 100;

/// One telling per pane in this long: an agent's turn and the task it
/// finished end together, and are one thing to the person told.
const GATE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Command,
    Agent,
    Task,
    Exit,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Agent => "agent",
            Kind::Task => "task",
            Kind::Exit => "exit",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub id: u64,
    pub at: chrono::DateTime<chrono::Local>,
    pub kind: Kind,
    pub pane: PaneId,
    /// `session:window.pane`, as it was then.
    pub place: String,
    pub name: String,
    pub text: String,
    /// Whether it went well, when that is known.
    pub ok: Option<bool>,
    /// The exit code itself, when it is known.
    pub exit: Option<i32>,
    pub secs: Option<u64>,
}

impl Done {
    /// The pane: its name and where it is, or where it is.
    pub fn who(&self) -> String {
        if self.name.is_empty() { self.place.clone() } else { format!("{} ({})", self.name, self.place) }
    }

    /// One line that says it all.
    pub fn line(&self) -> String {
        format!("{}: {}", self.who(), self.text)
    }

    pub fn json(&self, host: &str) -> Value {
        json!({
            "id": self.id,
            "at": self.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            "kind": self.kind.as_str(),
            "host": host,
            "pane": format!("%{}", self.pane),
            "name": self.name,
            "place": self.place,
            "text": self.line(),
            "ok": self.ok,
            "exit": self.exit,
            "seconds": self.secs,
        })
    }
}

/// What is kept: the latest ones, the next number, when each pane was last
/// told of.
#[derive(Default)]
pub struct DoneLog {
    pub list: VecDeque<Done>,
    next: u64,
    last: HashMap<PaneId, Instant>,
}

/// A span as a person reads it: `45s`, `5m 12s`, `1h 3m`.
pub fn took(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// What a person reads when a command ends: its exit code only when the
/// shell gave the code itself.
pub fn command_text(command: &str, secs: u64, failed: bool, exit: Option<i32>) -> String {
    let command = if command.is_empty() { "a command" } else { command };
    match (failed, exit) {
        (true, Some(code)) => format!("{command} failed (exit {code}) after {}", took(secs)),
        (true, None) => format!("{command} failed after {}", took(secs)),
        _ => format!("{command} finished in {}", took(secs)),
    }
}

/// A pane's command line as a person names the program: `pwsh` for
/// `"C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo`, `bash` for
/// `/bin/bash -l`. Either separator, on any system (the line may be
/// Windows' read on Linux, or the other way round).
pub fn program_name(command: &str) -> String {
    let c = command.trim();
    let program = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else if let Some(i) = c.to_ascii_lowercase().find(".exe") {
        // An unquoted Windows path may hold spaces: up to its `.exe`.
        &c[..i + 4]
    } else {
        c.split_whitespace().next().unwrap_or(c)
    };
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower = name.to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(_) => name[..name.len() - 4].to_string(),
        None => name.to_string(),
    }
}

/// A message's words, short: its first line, at most 60 characters.
pub fn gist(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(60).collect();
    if line.chars().count() > 60 || text.lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

/// The webhook's body, and its content type, in the shape `format` names.
/// A chat's message starts `keepane`, which a Feishu or DingTalk bot's
/// keyword check can be set to.
pub fn webhook_body(format: &str, d: &Done, host: &str) -> (&'static str, String) {
    let text = format!("keepane · {host}\n{}", d.line());
    let body = match format {
        "text" => return ("text/plain; charset=utf-8", d.line()),
        "feishu" => json!({ "msg_type": "text", "content": { "text": text } }),
        "wecom" | "dingtalk" => json!({ "msgtype": "text", "text": { "content": text } }),
        "slack" => json!({ "text": text }),
        "discord" => json!({ "content": text }),
        _ => d.json(host),
    };
    ("application/json; charset=utf-8", body.to_string())
}

/// A string as curl's config file quotes it.
fn curl_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What curl reads on its standard input (`-K -`): the whole request.
pub fn curl_config(url: &str, content_type: &str, body: &str) -> String {
    format!(
        "url = {}\nheader = {}\ndata-binary = {}\nmax-time = 10\nsilent\nshow-error\nfail\n",
        curl_quote(url),
        curl_quote(&format!("Content-Type: {content_type}")),
        curl_quote(body)
    )
}

/// A chat that answers 200 and still refuses (Feishu, WeCom, DingTalk say
/// so in their JSON: a `code`, `errcode` or `StatusCode` that is not 0):
/// what it said.
pub fn chat_refused(answer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(answer.trim()).ok()?;
    let code = ["code", "errcode", "StatusCode"].iter().find_map(|k| v.get(*k).and_then(Value::as_i64))?;
    if code == 0 {
        return None;
    }
    let msg = ["msg", "errmsg", "StatusMessage"].iter().find_map(|k| v.get(*k).and_then(Value::as_str)).unwrap_or("");
    Some(format!("refused ({code}): {msg}"))
}

/// Send one request through curl; what went wrong, if anything.
fn run_curl(config: &str) -> Result<(), String> {
    let mut c = crate::platform::process::quiet_command("curl");
    c.args(["-K", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("curl could not be run: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(config.as_bytes()).map_err(|e| format!("curl: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("curl exited with {}", out.status) } else { err });
    }
    match chat_refused(&String::from_utf8_lossy(&out.stdout)) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

impl Server {
    /// Where a pane is, as a person names it: `session:window.pane`.
    fn place_text(&self, pid: PaneId) -> String {
        let Some((sid, widx)) = self.place_of(pid) else { return format!("%{pid}") };
        let Some(s) = self.session(sid) else { return format!("%{pid}") };
        let pidx = s.windows[widx].layout.panes().iter().position(|p| *p == pid).unwrap_or(0);
        format!("{}:{}.{}", s.name, widx + self.opts.base_index, pidx + self.opts.pane_base_index)
    }

    /// A pane is done: kept and told, if `done-events` has this kind, the
    /// pane is one `done-panes` covers, and it was not told of a moment ago.
    pub(super) fn pane_done(
        &mut self,
        pane: PaneId,
        kind: Kind,
        text: String,
        ok: Option<bool>,
        exit: Option<i32>,
        secs: Option<u64>,
    ) {
        if !self.opts.done_events.split_whitespace().any(|e| e == kind.as_str()) {
            return;
        }
        let Some(p) = self.pane_ref(pane) else { return };
        let name = p.actor.name.clone().unwrap_or_default();
        if self.opts.done_panes != "all" && name.is_empty() && p.actor.mode == WorkMode::Normal {
            return;
        }
        let now = Instant::now();
        // Only the last few seconds matter: the table stays as small as that.
        self.done.last.retain(|_, t| now.duration_since(*t) < GATE);
        if self.done.last.contains_key(&pane) {
            return;
        }
        self.done.last.insert(pane, now);
        self.done.next += 1;
        let d = Done {
            id: self.done.next,
            at: chrono::Local::now(),
            kind,
            pane,
            place: self.place_text(pane),
            name,
            text,
            ok,
            exit,
            secs,
        };
        self.done.list.push_back(d.clone());
        while self.done.list.len() > KEPT {
            self.done.list.pop_front();
        }
        let host = crate::sysinfo::hostname();
        let addr = self.address_of(pane);
        self.observe.pane(&addr, "done", &format!(",\"kind\":\"{}\",\"text\":{}", kind.as_str(), json!(d.text)));
        // The hook, with what it is about in its environment.
        self.hook_env = vec![
            ("KEEPANE_DONE_KIND".into(), kind.as_str().into()),
            ("KEEPANE_DONE_TEXT".into(), d.line()),
            ("KEEPANE_DONE_PANE".into(), format!("%{pane}")),
            ("KEEPANE_DONE_NAME".into(), d.name.clone()),
            ("KEEPANE_DONE_OK".into(), d.ok.map(|o| if o { "1" } else { "0" }).unwrap_or_default().into()),
            ("KEEPANE_DONE_EXIT".into(), d.exit.map(|c| c.to_string()).unwrap_or_default()),
            ("KEEPANE_DONE_SECONDS".into(), d.secs.map(|s| s.to_string()).unwrap_or_default()),
            ("KEEPANE_DONE_JSON".into(), d.json(&host).to_string()),
        ];
        self.fire_hook("pane-done", None);
        self.hook_env.clear();
        if self.opts.notify {
            let go = crate::notify::go_to_pane_url(&self.socket, pane);
            crate::notify::notify_with(&format!("keepane: {}", d.who()), &d.text, Some(&go));
        }
        if !self.opts.done_webhook.is_empty() {
            let (content_type, body) = webhook_body(&self.opts.done_webhook_format, &d, &host);
            let config = curl_config(&self.opts.done_webhook, content_type, &body);
            let events = self.events.clone();
            let spawned = std::thread::Builder::new().name("done-webhook".into()).spawn(move || {
                if let Err(why) = run_curl(&config) {
                    let _ = events.send(Event::DoneWebhookFailed(why));
                }
            });
            if let Err(e) = spawned {
                log::warn!("done-webhook: {e}");
            }
        }
    }

    /// `list-done`: the ones after number `after`, a line each, or JSON
    /// with the newest number there is (`last`: smaller than what a page
    /// saw means the server started again, and its numbers with it).
    pub(super) fn list_done(&self, after: u64, json_out: bool) -> String {
        let host = crate::sysinfo::hostname();
        let newer = self.done.list.iter().filter(|d| d.id > after);
        if json_out {
            let all: Vec<Value> = newer.map(|d| d.json(&host)).collect();
            return json!({ "last": self.done.next, "done": all }).to_string();
        }
        newer
            .map(|d| format!("#{} {} {:<7} {}", d.id, d.at.format("%H:%M:%S"), d.kind.as_str(), d.line()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(kind: Kind, name: &str, text: &str) -> Done {
        Done {
            id: 3,
            at: chrono::Local::now(),
            kind,
            pane: 4,
            place: "work:0.1".into(),
            name: name.into(),
            text: text.into(),
            ok: Some(true),
            exit: Some(0),
            secs: Some(312),
        }
    }

    #[test]
    fn spans_and_lines_read_as_a_person_would() {
        assert_eq!(took(45), "45s");
        assert_eq!(took(312), "5m 12s");
        assert_eq!(took(3780), "1h 3m");
        assert_eq!(command_text("cargo test", 312, false, Some(0)), "cargo test finished in 5m 12s");
        assert_eq!(command_text("cargo test", 312, false, None), "cargo test finished in 5m 12s");
        assert_eq!(command_text("cargo test", 40, true, Some(101)), "cargo test failed (exit 101) after 40s");
        assert_eq!(command_text("cargo test", 40, true, None), "cargo test failed after 40s", "PowerShell: no code");
        assert_eq!(command_text("", 40, false, None), "a command finished in 40s");
        assert_eq!(program_name("\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -NoLogo"), "pwsh");
        assert_eq!(program_name("C:\\Program Files\\PowerShell\\7\\pwsh.exe -NoLogo"), "pwsh");
        assert_eq!(program_name("C:\\Windows\\System32\\cmd.EXE"), "cmd");
        assert_eq!(program_name("/bin/bash -l"), "bash");
        assert_eq!(program_name("claude"), "claude");
        assert_eq!(program_name("  "), "");
        assert_eq!(gist("review the diff\nand the tests"), "review the diff…");
        assert_eq!(gist(&"x".repeat(70)).chars().count(), 61);
        assert_eq!(done(Kind::Command, "build", "t").line(), "build (work:0.1): t");
        assert_eq!(done(Kind::Command, "", "t").line(), "work:0.1: t");
    }

    #[test]
    fn each_chat_gets_its_own_body() {
        let d = done(Kind::Command, "build", "cargo test finished in 5m 12s");
        let body = |f| serde_json::from_str::<Value>(&webhook_body(f, &d, "desk").1).unwrap();
        let said = "keepane · desk\nbuild (work:0.1): cargo test finished in 5m 12s";
        assert_eq!(body("feishu"), json!({"msg_type": "text", "content": {"text": said}}));
        assert_eq!(body("wecom"), json!({"msgtype": "text", "text": {"content": said}}));
        assert_eq!(body("dingtalk"), json!({"msgtype": "text", "text": {"content": said}}));
        assert_eq!(body("slack"), json!({"text": said}));
        assert_eq!(body("discord"), json!({"content": said}));
        let j = body("json");
        assert_eq!(
            (j["kind"].as_str(), j["pane"].as_str(), j["seconds"].as_u64()),
            (Some("command"), Some("%4"), Some(312))
        );
        assert_eq!(j["text"], "build (work:0.1): cargo test finished in 5m 12s");
        let (ct, text) = webhook_body("text", &d, "desk");
        assert_eq!(
            (ct, text.as_str()),
            ("text/plain; charset=utf-8", "build (work:0.1): cargo test finished in 5m 12s")
        );
    }

    /// curl reads its config quoted: a quote, a backslash or a line break
    /// in the body (or the address) stays what it was.
    #[test]
    fn curl_config_quotes_what_it_carries() {
        let cfg = curl_config("https://x/hook?a=1&b=\"2\"", "application/json", "{\"t\":\"a\\b\nc\"}");
        assert_eq!(
            cfg,
            "url = \"https://x/hook?a=1&b=\\\"2\\\"\"\nheader = \"Content-Type: application/json\"\n\
             data-binary = \"{\\\"t\\\":\\\"a\\\\b\\nc\\\"}\"\nmax-time = 10\nsilent\nshow-error\nfail\n"
        );
    }

    #[test]
    fn a_chat_that_says_no_is_heard() {
        assert_eq!(
            chat_refused("{\"code\":19021,\"msg\":\"sign match fail\"}").as_deref(),
            Some("refused (19021): sign match fail")
        );
        assert_eq!(
            chat_refused("{\"errcode\":310000,\"errmsg\":\"keywords not in content\"}").as_deref(),
            Some("refused (310000): keywords not in content")
        );
        assert_eq!(chat_refused("{\"StatusCode\":0,\"StatusMessage\":\"success\",\"code\":0}"), None);
        assert_eq!(chat_refused("{\"errcode\":0,\"errmsg\":\"ok\"}"), None);
        assert_eq!(chat_refused("ok"), None, "not JSON: taken as fine");
        assert_eq!(chat_refused(""), None);
    }
}
