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
/// nothing comes between them.
#[test]
fn a_codex_turn() {
    let line = |t: f64, ty: &str, payload: Value| {
        serde_json::json!({"timestamp": at(t), "type": ty, "payload": payload}).to_string()
    };
    let s = fed(
        Kind::Codex,
        &[
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
            ("model", "", "", ms_at(0.1), Some(ms_at(4.0)), false),
            ("run", "shell", "cargo test", ms_at(4.0), Some(ms_at(30.0)), true),
            ("model", "", "", ms_at(30.0), Some(ms_at(31.0)), false),
            ("edit", "apply_patch", "src/lib.rs", ms_at(31.0), Some(ms_at(31.5)), false),
            ("model", "", "", ms_at(31.5), Some(ms_at(33.0)), false),
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
    for i in 0..1600 {
        let t = f64::from(i) * 3.0;
        s.feed(Kind::Claude, &claude_prompt(t, &format!("turn {i}")));
        s.feed(Kind::Claude, &claude_reply(t + 1.0, &format!("r{i}"), serde_json::json!([]), Some("end_turn")));
    }
    assert!(s.spans.len() <= MOST);
    assert_eq!(s.spans.last().unwrap().parent.as_deref(), Some("turn-1600"));
    let first = s.spans.iter().find(|x| x.kind == SpanKind::Turn).unwrap();
    assert!(first.name != "turn 0", "the oldest let go");
    assert!(
        s.spans.iter().all(|x| x.kind == SpanKind::Turn || s.spans.iter().any(|t| Some(&t.id) == x.parent.as_ref()))
    );
}

/// Each agent's tools by kind; what a tool was given, short; durations.
#[test]
fn tools_by_kind_and_what_they_were_given() {
    for (name, kind) in [
        ("Read", SpanKind::Read),
        ("WebSearch", SpanKind::Read),
        ("grep_files", SpanKind::Read),
        ("Edit", SpanKind::Edit),
        ("apply_patch", SpanKind::Edit),
        ("Bash", SpanKind::Run),
        ("PowerShell", SpanKind::Run),
        ("exec_command", SpanKind::Run),
        ("Task", SpanKind::Agent),
        ("TodoWrite", SpanKind::Tool),
        ("mcp__xerj__xerj_search", SpanKind::Tool),
        ("mcp__fs__read", SpanKind::Read),
    ] {
        assert_eq!(SpanKind::of_tool(name), kind, "{name}");
    }
    assert_eq!(detail_of(&serde_json::json!({"command": "cargo test\n--more"})), "cargo test");
    assert_eq!(detail_of(&serde_json::json!({"cmd": ["pwsh", "-c", "ls"]})), "ls");
    assert_eq!(detail_of(&serde_json::json!({"file_path": "a.rs", "old_string": "x"})), "a.rs");
    assert_eq!(detail_of(&serde_json::json!("{\"path\":\"b.rs\"}")), "b.rs");
    assert_eq!(detail_of(&serde_json::json!({"unknown": 1})), "");
    assert_eq!(detail_of(&serde_json::json!({"command": "x".repeat(300)})).chars().count(), 120);
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
/// line the agent adds itself begins none either.
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
        ]
    );
    assert_eq!(given("<command-name>/model</command-name>\n<command-args></command-args>"), Some("/model".into()));
    assert_eq!(given("<system-reminder>be brief</system-reminder>"), None);
    assert_eq!(
        given("<task-notification><status>failed</status></task-notification>"),
        Some("a background task done".into())
    );
    assert_eq!(given("<bash-input>ls</bash-input>"), Some("bash input".into()));
}
