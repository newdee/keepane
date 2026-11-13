use super::*;
use crate::server::agents::Kind;

/// A step as the tests compare it: kind, name, what it was given, start,
/// end, failed.
type Row<'a> = (&'a str, &'a str, &'a str, i64, Option<i64>, bool);

/// `2026-10-23T10:00:SS.mmmZ`, `s` seconds after ten.
fn at(s: f64) -> String {
    let ten = chrono::DateTime::parse_from_rfc3339("2026-10-23T10:00:00.000Z").unwrap();
    let t = ten + chrono::Duration::milliseconds((s * 1000.0).round() as i64);
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn ms_at(s: f64) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&at(s)).unwrap().timestamp_millis()
}

fn fed(kind: Kind, lines: &[String]) -> Steps {
    let mut s = Steps::default();
    for l in lines {
        s.feed(kind, l);
    }
    s
}

fn claude_prompt(t: f64, text: &str) -> String {
    serde_json::json!({"type": "user", "timestamp": at(t), "message": {"role": "user", "content": text}}).to_string()
}

fn claude_reply(t: f64, id: &str, blocks: Value, stop: Option<&str>) -> String {
    serde_json::json!({"type": "assistant", "timestamp": at(t), "message": {
        "id": id, "model": "claude-x", "content": blocks, "stop_reason": stop, "usage": {"output_tokens": 120}
    }})
    .to_string()
}

fn claude_result(t: f64, id: &str, error: bool, text: &str) -> String {
    serde_json::json!({"type": "user", "timestamp": at(t), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": id, "is_error": error, "content": [{"type": "text", "text": text}]}
    ]}})
    .to_string()
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

/// A Claude turn: the prompt; the model thinking, then calling a tool (its
/// lines one reply, one call); the tool from its call to its answer,
/// failed when the answer says so; the model again; the turn over at the
/// reply that ends it. Each step under its turn, by its kind.
#[test]
fn a_claude_turn_is_its_model_calls_and_tools() {
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "fix the share test\nplease"),
            claude_reply(4.0, "m1", serde_json::json!([{"type": "thinking"}]), None),
            claude_reply(
                5.0,
                "m1",
                serde_json::json!([tool_use("t1", "Bash", serde_json::json!({"command": "cargo test"}))]),
                Some("tool_use"),
            ),
            claude_result(44.0, "t1", true, "1 failed"),
            claude_reply(
                47.0,
                "m2",
                serde_json::json!([tool_use("t2", "Edit", serde_json::json!({"file_path": "tests/e2e.rs"}))]),
                Some("tool_use"),
            ),
            claude_result(47.1, "t2", false, "ok"),
            claude_reply(50.0, "m3", serde_json::json!([{"type": "text"}]), Some("end_turn")),
        ],
    );
    let got: Vec<Row> =
        s.spans.iter().map(|s| (s.kind.word(), s.name.as_str(), s.detail.as_str(), s.start, s.end, s.failed)).collect();
    assert_eq!(
        got,
        [
            ("turn", "fix the share test", "", ms_at(0.0), Some(ms_at(50.0)), false),
            ("model", "claude-x", "", ms_at(0.0), Some(ms_at(5.0)), false),
            ("run", "Bash", "cargo test", ms_at(5.0), Some(ms_at(44.0)), true),
            ("model", "claude-x", "", ms_at(44.0), Some(ms_at(47.0)), false),
            ("edit", "Edit", "tests/e2e.rs", ms_at(47.0), Some(ms_at(47.1)), false),
            ("model", "claude-x", "", ms_at(47.1), Some(ms_at(50.0)), false),
        ]
    );
    assert!(s.spans[1..].iter().all(|x| x.parent.as_deref() == Some("turn-1")));
    assert_eq!(s.spans[1].tokens, 120);
    assert_eq!(s.running(), None);
    assert_eq!(s.thinking(), None);
}

/// While a turn runs: a tool not answered yet runs (no end), and the model
/// is not thinking; once answered, the model is at work from the answer.
/// Broken off by the person: the turn and what ran in it end there,
/// `interrupted`. A line Claude Code wrote itself (`<command-name>`) and
/// one it marks `isMeta` begin no turn.
#[test]
fn a_running_turn_and_one_broken_off() {
    let mut s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "build it"),
            claude_reply(
                2.0,
                "m1",
                serde_json::json!([tool_use("t1", "Bash", serde_json::json!({"command": "make"}))]),
                Some("tool_use"),
            ),
        ],
    );
    assert_eq!(s.running(), Some("turn-1"));
    assert_eq!(s.thinking(), None, "a tool runs");
    assert_eq!(s.spans.last().unwrap().end, None);
    s.feed(Kind::Claude, &claude_result(9.0, "t1", false, "done"));
    assert_eq!(s.thinking(), Some(ms_at(9.0)));
    s.feed(Kind::Claude, &claude_prompt(9.5, "<command-name>/model</command-name>"));
    let meta =
        serde_json::json!({"type": "user", "isMeta": true, "timestamp": at(9.6), "message": {"content": "caveat"}});
    s.feed(Kind::Claude, &meta.to_string());
    assert_eq!(s.spans.iter().filter(|x| x.kind == SpanKind::Turn).count(), 1);
    s.feed(
        Kind::Claude,
        &claude_reply(
            10.0,
            "m2",
            serde_json::json!([tool_use("t2", "Grep", serde_json::json!({"pattern": "fn main"}))]),
            Some("tool_use"),
        ),
    );
    s.feed(Kind::Claude, &claude_prompt(12.0, "[Request interrupted by user for tool use]"));
    let turn = &s.spans[0];
    assert_eq!((turn.end, turn.detail.as_str()), (Some(ms_at(12.0)), "interrupted"));
    let grep = s.spans.iter().find(|x| x.id == "t2").unwrap();
    assert_eq!((grep.kind, grep.detail.as_str(), grep.end), (SpanKind::Read, "fn main", Some(ms_at(12.0))));
    assert_eq!(s.running(), None);
}

/// A tool's answer after its turn ended (or a reply's last line) begins no
/// empty turn of its own.
#[test]
fn a_stray_line_begins_no_turn() {
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "one"),
            claude_reply(1.0, "m1", serde_json::json!([{"type": "text"}]), Some("end_turn")),
            claude_reply(1.5, "m1", serde_json::json!([{"type": "text"}]), None),
            claude_result(2.0, "gone", false, ""),
            claude_prompt(3.0, "two"),
        ],
    );
    let turns: Vec<&str> = s.spans.iter().filter(|x| x.kind == SpanKind::Turn).map(|x| x.name.as_str()).collect();
    assert_eq!(turns, ["one", "two"]);
}

/// Codex: a turn from `task_started` to `task_complete`, its prompt from
/// `user_message`; a call and its output by `call_id`, failed by its exit
/// code; the model's reasoning and messages between, one call while
/// nothing comes between them, on the model `turn_context` names.
#[test]
fn a_codex_turn() {
    let line = |t: f64, ty: &str, payload: Value| {
        serde_json::json!({"timestamp": at(t), "type": ty, "payload": payload}).to_string()
    };
    let s = fed(
        Kind::Codex,
        &[
            line(0.0, "turn_context", serde_json::json!({"model": "gpt-5.5"})),
            line(0.0, "event_msg", serde_json::json!({"type": "task_started"})),
            line(0.1, "event_msg", serde_json::json!({"type": "user_message", "message": "run the tests"})),
            line(3.0, "response_item", serde_json::json!({"type": "reasoning"})),
            line(
                4.0,
                "response_item",
                serde_json::json!({"type": "function_call", "name": "shell", "call_id": "c1",
                "arguments": "{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}"}),
            ),
            line(
                30.0,
                "response_item",
                serde_json::json!({"type": "function_call_output", "call_id": "c1",
                "output": "{\"output\":\"1 failed\",\"metadata\":{\"exit_code\":101}}"}),
            ),
            line(
                31.0,
                "response_item",
                serde_json::json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "c2",
                "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@"}),
            ),
            line(
                31.5,
                "response_item",
                serde_json::json!({"type": "custom_tool_call_output", "call_id": "c2", "output": "Exit code: 0\nDone"}),
            ),
            line(33.0, "response_item", serde_json::json!({"type": "message", "role": "assistant"})),
            line(33.1, "event_msg", serde_json::json!({"type": "task_complete"})),
        ],
    );
    let got: Vec<Row> =
        s.spans.iter().map(|s| (s.kind.word(), s.name.as_str(), s.detail.as_str(), s.start, s.end, s.failed)).collect();
    assert_eq!(
        got,
        [
            ("turn", "run the tests", "", ms_at(0.0), Some(ms_at(33.1)), false),
            ("model", "gpt-5.5", "", ms_at(0.1), Some(ms_at(4.0)), false),
            ("run", "shell", "cargo test", ms_at(4.0), Some(ms_at(30.0)), true),
            ("model", "gpt-5.5", "", ms_at(30.0), Some(ms_at(31.0)), false),
            ("edit", "apply_patch", "src/lib.rs", ms_at(31.0), Some(ms_at(31.5)), false),
            ("model", "gpt-5.5", "", ms_at(31.5), Some(ms_at(33.0)), false),
        ]
    );
}

/// pi: the person's message begins a turn; a reply's tool calls run until
/// their `toolResult`, failed by `isError`; a reply that calls none ends
/// the turn (`aborted`: broken off).
#[test]
fn a_pi_turn() {
    let msg = |t: f64, m: Value| serde_json::json!({"type": "message", "timestamp": at(t), "message": m}).to_string();
    let s = fed(
        Kind::Pi,
        &[
            msg(0.0, serde_json::json!({"role": "user", "content": [{"type": "text", "text": "read the readme"}]})),
            msg(
                2.0,
                serde_json::json!({"role": "assistant", "model": "pi-m", "stopReason": "toolUse", "usage": {"output": 40},
                "content": [{"type": "toolCall", "id": "p1", "name": "read", "arguments": {"path": "README.md"}}]}),
            ),
            msg(2.2, serde_json::json!({"role": "toolResult", "toolCallId": "p1", "isError": true})),
            msg(5.0, serde_json::json!({"role": "assistant", "model": "pi-m", "stopReason": "aborted", "content": []})),
        ],
    );
    let got: Vec<(&str, &str, i64, Option<i64>, bool)> =
        s.spans.iter().map(|s| (s.kind.word(), s.detail.as_str(), s.start, s.end, s.failed)).collect();
    assert_eq!(
        got,
        [
            ("turn", "interrupted", ms_at(0.0), Some(ms_at(5.0)), false),
            ("model", "", ms_at(0.0), Some(ms_at(2.0)), false),
            ("read", "README.md", ms_at(2.0), Some(ms_at(2.2)), true),
            ("model", "", ms_at(2.2), Some(ms_at(5.0)), false),
        ]
    );
    assert_eq!(s.spans[1].tokens, 40);
}

/// Merged: each file's ids made its own; only the latest turns; a
/// subagent's steps under the call that started it (its `agentId:`), else
/// under the agent call running when it began; while a turn runs and no
/// tool does, the model at work from its last step to now.
#[test]
fn the_steps_merged_for_the_pane() {
    let older = fed(
        Kind::Claude,
        &[claude_prompt(0.0, "first"), claude_reply(1.0, "a", serde_json::json!([]), Some("end_turn"))],
    );
    let newer = fed(
        Kind::Claude,
        &[
            claude_prompt(10.0, "second"),
            claude_reply(
                11.0,
                "b",
                serde_json::json!([tool_use("k1", "Task", serde_json::json!({"description": "find it"}))]),
                Some("tool_use"),
            ),
            claude_result(12.0, "k1", false, "Async agent launched.\nagentId: a42 (internal)"),
        ],
    );
    let helper = fed(
        Kind::Claude,
        &[
            claude_prompt(12.0, "find it"),
            claude_reply(
                14.0,
                "h1",
                serde_json::json!([tool_use("g1", "Glob", serde_json::json!({"pattern": "**/*.rs"}))]),
                Some("tool_use"),
            ),
            claude_result(14.5, "g1", false, ""),
        ],
    );
    let now = ms_at(20.0);
    let spans = merge(&[&older, &newer], &[("a42".into(), &helper)], 20, now);
    let ids: Vec<&str> = spans.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"0:turn-1") && ids.contains(&"1:turn-1"), "{ids:?}");
    let task = spans.iter().find(|s| s.id == "1:k1").unwrap();
    assert_eq!(task.kind, SpanKind::Agent);
    let under: Vec<&str> =
        spans.iter().filter(|s| s.parent.as_deref() == Some("1:k1")).map(|s| s.kind.word()).collect();
    assert_eq!(under, ["model", "read"], "the subagent's steps, not its turn");
    let thinking = spans.iter().find(|s| s.id == "1:thinking").unwrap();
    assert_eq!((thinking.start, thinking.end, thinking.parent.as_deref()), (ms_at(12.0), None, Some("1:turn-1")));
    // The latest turn only: the older file's steps go with its turn.
    let one = merge(&[&older, &newer], &[], 1, now);
    assert!(one.iter().all(|s| s.id.starts_with("1:")), "{:?}", one.iter().map(|s| &s.id).collect::<Vec<_>>());
    // A subagent no call names: under the agent call running when it began.
    let spans = merge(&[&newer], &[("zz".into(), &helper)], 20, now);
    assert_eq!(spans.iter().filter(|s| s.parent.as_deref() == Some("0:k1")).count(), 2);
    // The text tree: indented under their parents, the running ones said.
    let lines = tree(&spans, now);
    assert_eq!(lines[0], "turn second 10s running");
    assert!(lines.contains(&"    read Glob **/*.rs 500ms".to_string()), "{lines:#?}");
}

/// A long transcript keeps its latest steps: the oldest turns go whole.
#[test]
fn a_long_transcript_keeps_its_latest_turns() {
    let mut s = Steps::default();
    // The first turn starts a server in the background that runs on.
    s.feed(Kind::Claude, &claude_prompt(0.0, "serve"));
    s.feed(
        Kind::Claude,
        &claude_reply(
            0.5,
            "r-bg",
            serde_json::json!([tool_use(
                "dev",
                "Bash",
                serde_json::json!({"command": "pnpm dev", "run_in_background": true})
            )]),
            Some("tool_use"),
        ),
    );
    s.feed(Kind::Claude, &claude_result(0.6, "dev", false, "Command running in background with ID: b1"));
    for i in 0..1600 {
        let t = 1.0 + f64::from(i) * 3.0;
        s.feed(Kind::Claude, &claude_prompt(t, &format!("turn {i}")));
        s.feed(Kind::Claude, &claude_reply(t + 1.0, &format!("r{i}"), serde_json::json!([]), Some("end_turn")));
    }
    assert!(s.spans.len() <= MOST);
    assert_eq!(s.spans.last().unwrap().parent.as_deref(), Some("turn-1601"));
    let first = s.spans.iter().find(|x| x.kind == SpanKind::Turn).unwrap();
    assert!(first.name != "turn 0" && first.name != "serve", "the oldest let go");
    // What still runs in the background stays, its turn gone.
    let dev = s.spans.iter().find(|x| x.id == "dev").expect("still listed");
    assert!(dev.end.is_none() && !s.spans.iter().any(|t| Some(&t.id) == dev.parent.as_ref()));
    assert!(s.spans.iter().all(|x| x.kind == SpanKind::Turn
        || x.id == "dev"
        || s.spans.iter().any(|t| Some(&t.id) == x.parent.as_ref())));
}

/// Each agent's tools by kind; what a tool was given, short; durations.
#[test]
fn tools_by_kind_and_what_they_were_given() {
    for (name, kind) in [
        ("Read", SpanKind::Read),
        ("WebSearch", SpanKind::Net),
        ("WebFetch", SpanKind::Net),
        ("web_search", SpanKind::Net),
        ("grep_files", SpanKind::Read),
        ("Edit", SpanKind::Edit),
        ("apply_patch", SpanKind::Edit),
        ("Bash", SpanKind::Run),
        ("PowerShell", SpanKind::Run),
        ("exec_command", SpanKind::Run),
        ("Task", SpanKind::Agent),
        ("Agent", SpanKind::Agent),
        ("Workflow", SpanKind::Agent),
        ("Skill", SpanKind::Skill),
        ("TodoWrite", SpanKind::Tool),
        ("mcp__xerj__xerj_search", SpanKind::Tool),
        ("mcp__xerj__xerj_memory_store", SpanKind::Memory),
        ("mcp__memory__create_entities", SpanKind::Memory),
        ("mcp__claude-in-chrome__navigate", SpanKind::Browser),
        ("mcp__playwright__browser_click", SpanKind::Browser),
        ("mcp__fs__read", SpanKind::Read),
        ("mcp__fetch__fetch", SpanKind::Net),
        // Only an MCP server's name says browser or memory: a tool of
        // Claude Code's own by that word is not.
        ("memory", SpanKind::Tool),
    ] {
        assert_eq!(SpanKind::of_tool(name), kind, "{name}");
    }
    assert_eq!(detail_of(&serde_json::json!({"command": "cargo test\n--more"})), "cargo test");
    assert_eq!(detail_of(&serde_json::json!({"cmd": ["pwsh", "-c", "ls"]})), "ls");
    assert_eq!(detail_of(&serde_json::json!({"file_path": "a.rs", "old_string": "x"})), "a.rs");
    assert_eq!(detail_of(&serde_json::json!("{\"path\":\"b.rs\"}")), "b.rs");
    assert_eq!(detail_of(&serde_json::json!({"unknown": 1})), "");
    assert_eq!(detail_of(&serde_json::json!({"command": "x".repeat(300)})).chars().count(), 120);
    // A long path keeps its end, the file's name.
    let deep = format!(r"C:\{}\tasks\b1.output", "d\\".repeat(100));
    let kept = detail_of(&serde_json::json!({ "file_path": deep }));
    assert!(kept.starts_with('…') && kept.ends_with(r"\tasks\b1.output") && kept.chars().count() == 120, "{kept}");
    assert_eq!(
        (took(45), took(4_100), took(39_000), took(192_000)),
        ("45ms".into(), "4.1s".into(), "39s".into(), "3m12s".into())
    );
    assert!(codex_failed(&serde_json::json!("Process exited with code 2")));
    assert!(!codex_failed(&serde_json::json!("no code said")));
}

/// A turn not begun by the person's typed prompt is named by what began it:
/// a message given once the agent could take it (`queued_command`: yours,
/// or a background task done, by its summary); one queued by an older
/// Claude Code (`queue-operation`); a scheduled task's prompt (`isMeta`
/// with `turnOrigin`). A message queued while a turn runs begins none. A
/// line the agent adds itself begins none either. With `turnOrigin` on
/// every line: a typed one (`human`, `sdk`) as it is, a task done by its
/// summary, another session's message by who sent it.
#[test]
fn every_turn_is_named_by_what_began_it() {
    let queued = |t: f64, prompt: &str| {
        serde_json::json!({"type": "attachment", "timestamp": at(t), "attachment": {"type": "queued_command", "prompt": prompt}}).to_string()
    };
    let done = |t: f64, id: &str| claude_reply(t, id, serde_json::json!([{"type": "text"}]), Some("end_turn"));
    let notice = "<task-notification>\n<status>completed</status>\n<summary>Background command \"tests\" completed (exit code 0)</summary>\n</task-notification>";
    let scheduled =
        serde_json::json!({"type": "user", "isMeta": true, "turnOrigin": "scheduled", "timestamp": at(40.0),
        "message": {"role": "user", "content": "check the release run"}})
        .to_string();
    let meta = serde_json::json!({"type": "user", "isMeta": true, "timestamp": at(50.0), "message": {"role": "user", "content": "caveat"}}).to_string();
    let origin = |t: f64, origin: &str, meta: bool, text: &str| {
        serde_json::json!({"type": "user", "isMeta": meta.then_some(true), "turnOrigin": origin, "timestamp": at(t),
            "message": {"role": "user", "content": text}})
        .to_string()
    };
    let enqueue = serde_json::json!({"type": "queue-operation", "operation": "enqueue", "timestamp": at(60.0),
        "content": "<agent-message from=\"helper\">\nfound it\n</agent-message>"})
    .to_string();
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "build it"),
            queued(1.0, "and then test it"),
            done(5.0, "a"),
            queued(10.0, "and then test it"),
            done(15.0, "b"),
            queued(20.0, notice),
            done(25.0, "c"),
            scheduled,
            done(45.0, "d"),
            meta,
            enqueue,
            done(65.0, "e"),
            // Claude Code 2.1.3xx says what began each turn: typed, a task
            // done, another session's message.
            origin(70.0, "human", false, "ship it"),
            done(71.0, "f"),
            origin(72.0, "task_notification", false, notice),
            done(73.0, "g"),
            origin(
                74.0,
                "peer",
                true,
                "Another Claude session sent a message:\n<agent-message from=\"a27\">\nhanded back\n</agent-message>",
            ),
            done(75.0, "h"),
            origin(
                76.0,
                "peer",
                true,
                "Another Claude session sent a message:\n<cross-session-message from=\"uds:x\">hi",
            ),
            done(77.0, "i"),
            origin(78.0, "sdk", false, "say OK"),
            done(79.0, "j"),
            origin(80.0, "peer", true, "no tag in it"),
            done(81.0, "k"),
        ],
    );
    let turns: Vec<&str> = s.spans.iter().filter(|x| x.kind == SpanKind::Turn).map(|x| x.name.as_str()).collect();
    assert_eq!(
        turns,
        [
            "build it",
            "and then test it",
            "Background command \"tests\" completed (exit code 0)",
            "(scheduled) check the release run",
            "a message from helper",
            "ship it",
            "Background command \"tests\" completed (exit code 0)",
            "a message from a27",
            "a message from another session",
            "say OK",
            "(peer) no tag in it",
        ]
    );
    assert_eq!(given("<command-name>/model</command-name>\n<command-args></command-args>"), Some("/model".into()));
    assert_eq!(given("<system-reminder>be brief</system-reminder>"), None);
    assert_eq!(
        given("<task-notification><status>failed</status></task-notification>"),
        Some("a background task done".into())
    );
    assert_eq!(given("<bash-input>ls -la</bash-input>"), Some("! ls -la".into()));
    assert_eq!(given("<bash-stdout>total 0</bash-stdout><bash-stderr></bash-stderr>"), None);
    assert_eq!(given("<ide-opened-file>x</ide-opened-file>"), Some("ide opened file".into()));
}

/// Why a tool failed, when the transcript says: the person said no
/// (`denied`), a `PreToolUse` hook stopped it (`hook: …`, its first line);
/// any other failure says nothing more.
#[test]
fn a_refusal_says_who_stopped_the_tool() {
    let reject = serde_json::json!({"type": "user", "timestamp": at(3.0), "toolUseResult": "User rejected tool use",
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true,
        "content": "The user doesn't want to proceed with this tool use. The tool use was rejected."}]}})
    .to_string();
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "change it"),
            claude_reply(
                1.0,
                "m1",
                serde_json::json!([
                    tool_use("t1", "Edit", serde_json::json!({"file_path": "a.rs"})),
                    tool_use("t2", "Edit", serde_json::json!({"file_path": "CLAUDE.md"})),
                    tool_use("t3", "Bash", serde_json::json!({"command": "false"}))
                ]),
                Some("tool_use"),
            ),
            reject,
            claude_result(3.1, "t2", true, "PreToolUse:Edit hook error: CLAUDE.md is protected\nsecond line"),
            claude_result(3.2, "t3", true, "exit code 1"),
        ],
    );
    let why: Vec<(&str, bool, &str)> = s
        .spans
        .iter()
        .filter(|x| x.kind == SpanKind::Edit || x.kind == SpanKind::Run)
        .map(|x| (x.id.as_str(), x.failed, x.why.as_str()))
        .collect();
    assert_eq!(why, [("t1", true, "denied"), ("t2", true, "hook: CLAUDE.md is protected"), ("t3", true, "")]);
    assert!(tree(&s.spans, ms_at(9.0)).iter().any(|l| l.contains("failed (denied)")));
}

/// A tool run in the background: its answer only says it began, and its
/// turn ending does not end it; the task's notification does (from the
/// person's line, or one queued while the agent worked), one not completed
/// failed with its state. The model is at work meanwhile, not held up.
#[test]
fn a_background_task_runs_until_its_notification() {
    let note = |t: f64, id: &str, status: &str| {
        serde_json::json!({"type": "user", "timestamp": at(t), "turnOrigin": "task_notification", "message": {
            "role": "user", "content": format!("<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>{id}</tool-use-id>\n<status>{status}</status>\n<summary>Background command \"tests\" {status}</summary>\n</task-notification>")}})
        .to_string()
    };
    let queued =
        serde_json::json!({"type": "attachment", "timestamp": at(30.0), "attachment": {"type": "queued_command",
        "prompt": "<task-notification>\n<tool-use-id>t2</tool-use-id>\n<status>killed</status>\n</task-notification>"}})
        .to_string();
    let mut s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "run the tests"),
            claude_reply(
                1.0,
                "m1",
                serde_json::json!([
                    tool_use("t1", "Bash", serde_json::json!({"command": "cargo test", "run_in_background": true})),
                    tool_use("t2", "Bash", serde_json::json!({"command": "serve", "run_in_background": true}))
                ]),
                Some("tool_use"),
            ),
            claude_result(1.2, "t1", false, "Command running in background with ID: b1"),
            claude_result(1.3, "t2", false, "Command running in background with ID: b2"),
        ],
    );
    let t1 = |s: &Steps| s.spans.iter().find(|x| x.id == "t1").cloned().unwrap();
    assert!(t1(&s).background && t1(&s).end.is_none(), "began, goes on");
    assert_eq!(s.thinking(), Some(ms_at(1.3)), "the model works meanwhile");
    s.feed(Kind::Claude, &claude_reply(2.0, "m2", serde_json::json!([{"type": "text"}]), Some("end_turn")));
    assert_eq!((s.running(), t1(&s).end), (None, None), "the turn is over, the task is not");
    s.feed(Kind::Claude, &note(20.0, "t1", "completed"));
    assert_eq!((t1(&s).end, t1(&s).failed), (Some(ms_at(20.0)), false));
    // It still names the turn the agent then begins.
    s.feed(Kind::Claude, &claude_reply(21.0, "m3", serde_json::json!([{"type": "text"}]), None));
    let turn = s.spans.iter().find(|x| Some(x.id.as_str()) == s.running()).unwrap();
    assert_eq!(turn.name, "Background command \"tests\" completed");
    s.feed(Kind::Claude, &queued);
    let t2 = s.spans.iter().find(|x| x.id == "t2").unwrap();
    assert_eq!((t2.end, t2.failed, t2.why.as_str()), (Some(ms_at(30.0)), true, "killed"));
    // One that failed to begin ends at its answer.
    let mut b = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "x"),
            claude_reply(
                1.0,
                "m1",
                serde_json::json!([tool_use(
                    "t9",
                    "Bash",
                    serde_json::json!({"command": "nope", "run_in_background": true})
                )]),
                Some("tool_use"),
            ),
        ],
    );
    b.feed(Kind::Claude, &claude_result(1.5, "t9", true, "could not start"));
    assert_eq!(b.spans.last().unwrap().end, Some(ms_at(1.5)));
}

/// Still running in the background, its turn too old to be shown: kept,
/// and the text tree says it.
#[test]
fn a_background_task_outlives_the_turns_shown() {
    let mut lines = vec![
        claude_prompt(0.0, "first"),
        claude_reply(
            1.0,
            "m1",
            serde_json::json!([tool_use(
                "bg",
                "Bash",
                serde_json::json!({"command": "watch", "run_in_background": true})
            )]),
            Some("tool_use"),
        ),
        claude_result(1.1, "bg", false, "running"),
        claude_reply(2.0, "m2", serde_json::json!([{"type": "text"}]), Some("end_turn")),
    ];
    for i in 0..3 {
        let t = 10.0 + f64::from(i) * 10.0;
        lines.push(claude_prompt(t, &format!("turn {i}")));
        lines.push(claude_reply(t + 1.0, &format!("r{i}"), serde_json::json!([{"type": "text"}]), Some("end_turn")));
    }
    let s = fed(Kind::Claude, &lines);
    let spans = merge(&[&s], &[], 2, ms_at(60.0));
    assert!(!spans.iter().any(|x| x.kind == SpanKind::Turn && x.name == "first"), "an old turn");
    assert!(spans.iter().any(|x| x.name == "Bash" && x.background), "{spans:?}");
    assert!(tree(&spans, ms_at(60.0)).iter().any(|l| l.contains("watch") && l.ends_with("background running")));
}

/// The hooks the agent ran, as steps of the turn with no time of their
/// own: one that failed says why (without its colours); one that added
/// context says so; none before a turn (the session's start).
#[test]
fn hooks_are_steps_of_their_turn() {
    let hook =
        |t: f64, a: Value| serde_json::json!({"type": "attachment", "timestamp": at(t), "attachment": a}).to_string();
    let s = fed(
        Kind::Claude,
        &[
            hook(
                0.0,
                serde_json::json!({"type": "hook_success", "hookName": "SessionStart:startup", "hookEvent": "SessionStart"}),
            ),
            claude_prompt(1.0, "go"),
            hook(
                1.1,
                serde_json::json!({"type": "hook_additional_context", "hookName": "UserPromptSubmit", "content": ["be brief"]}),
            ),
            hook(
                2.0,
                serde_json::json!({"type": "hook_non_blocking_error", "hookName": "PostToolUse:Write",
                "stderr": "Failed with non-blocking status code: \u{1b}[31;1mOut-File: no path\u{1b}[0m\r\nmore"}),
            ),
            hook(3.0, serde_json::json!({"type": "hook_cancelled", "hookName": "Stop"})),
        ],
    );
    let hooks: Vec<(&str, &str, bool, &str, Option<i64>)> = s
        .spans
        .iter()
        .filter(|x| x.kind == SpanKind::Hook)
        .map(|x| (x.name.as_str(), x.detail.as_str(), x.failed, x.why.as_str(), x.end))
        .collect();
    assert_eq!(
        hooks,
        [
            ("UserPromptSubmit", "added context", false, "", Some(ms_at(1.1))),
            (
                "PostToolUse:Write",
                "",
                true,
                "Failed with non-blocking status code: Out-File: no path",
                Some(ms_at(2.0))
            ),
            ("Stop", "", true, "cancelled", Some(ms_at(3.0))),
        ]
    );
    assert!(s.spans.iter().filter(|x| x.kind == SpanKind::Hook).all(|x| x.parent.as_deref() == Some("turn-1")));
    assert_eq!(s.thinking(), Some(ms_at(1.0)), "a hook does not move where the model began");
}

/// A model call on another model than the one before says from which.
#[test]
fn a_model_switch_says_from_which() {
    let reply = |t: f64, id: &str, model: &str, stop: Option<&str>| {
        serde_json::json!({"type": "assistant", "timestamp": at(t), "message": {
            "id": id, "model": model, "content": [{"type": "text"}], "stop_reason": stop, "usage": {"output_tokens": 1}}})
        .to_string()
    };
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "a"),
            reply(1.0, "m1", "claude-opus-5-5", Some("end_turn")),
            claude_prompt(2.0, "b"),
            reply(3.0, "m2", "claude-opus-5-5", Some("end_turn")),
            claude_prompt(4.0, "c"),
            reply(5.0, "m3", "claude-fable-5-2", Some("end_turn")),
        ],
    );
    let models: Vec<&str> = s.spans.iter().filter(|x| x.kind == SpanKind::Model).map(|x| x.detail.as_str()).collect();
    assert_eq!(models, ["", "", "from claude-opus-5-5"]);
}

/// A background task stopped by the agent (`TaskStop`, naming the task's
/// id its tool's answer gave) ends there, not failed: no notification
/// comes for it. The agent started again (`SessionStart`): what ran in the
/// background of the run before ended with it.
#[test]
fn a_background_task_ends_when_stopped_or_the_agent_restarts() {
    let hook = |t: f64, event: &str| {
        serde_json::json!({"type": "attachment", "timestamp": at(t), "attachment": {
            "type": "hook_success", "hookName": format!("{event}:resume"), "hookEvent": event}})
        .to_string()
    };
    let s = fed(
        Kind::Claude,
        &[
            claude_prompt(0.0, "serve it"),
            claude_reply(
                1.0,
                "m1",
                serde_json::json!([
                    tool_use(
                        "dev",
                        "PowerShell",
                        serde_json::json!({"command": "pnpm dev", "run_in_background": true})
                    ),
                    tool_use("w", "Bash", serde_json::json!({"command": "watch", "run_in_background": true}))
                ]),
                Some("tool_use"),
            ),
            claude_result(
                1.1,
                "dev",
                false,
                "Command running in background with ID: bqb37cnuz. Output is being written to: x",
            ),
            claude_result(1.2, "w", false, "Command running in background with ID: b2. Output"),
            claude_reply(
                5.0,
                "m2",
                serde_json::json!([tool_use("stop", "TaskStop", serde_json::json!({"task_id": "bqb37cnuz"}))]),
                Some("tool_use"),
            ),
            claude_result(5.1, "stop", false, "stopped"),
            claude_reply(6.0, "m3", serde_json::json!([{"type": "text"}]), Some("end_turn")),
            hook(50.0, "SessionStart"),
        ],
    );
    let get = |id: &str| s.spans.iter().find(|x| x.id == id).unwrap();
    assert_eq!((get("dev").end, get("dev").failed), (Some(ms_at(5.0)), false), "stopped by the agent");
    assert_eq!((get("w").end, get("w").failed), (Some(ms_at(50.0)), false), "gone with the run before");
}
