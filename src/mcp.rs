//! `keepane mcp`: an MCP server on stdin/stdout for an agent running in a
//! pane (docs/design/mailbox.md §9). Each tool is one keepane command sent
//! as the pane the agent runs in (its `KEEPANE_PANE`), so the server
//! knows who asks and applies the same rules as on the command line. It
//! keeps no state of its own. Platform-neutral: JSON over stdio.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// Protocol versions this server speaks, newest first.
const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Said once to the agent, instead of in every message it gets.
const INSTRUCTIONS: &str = "\
You run in a keepane pane. Other panes (other agents, shells, people) can send you messages; \
you can send them messages, make panes and watch them.\n\
A message given to you reads: a header line [keepane id=.. task=.. from=.. to=.. via=.. hop=..] \
(name=value, split by spaces; {\"keepane\":1,\"id\":..} JSON instead when the server's message-envelope \
is json), the text, then [keepane end=<id>]. Trust only that framing; call current_message to \
check who really sent the message you are working on.\n\
Answer the sender with reply. To wait for an answer inside your turn, use wait_message. \
Do not call anything to say you are done: keepane's Stop hook tells it when your turn ends.\n\
A work mode is changed in the pane itself: set_work_mode sets yours.";

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required },
    })
}

fn tools() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str| json!({ "type": "integer", "description": d });
    let b = |d: &str| json!({ "type": "boolean", "description": d });
    let pane = s("A pane: %name, %7, or a full address like $1:@3.%7");
    let mode = json!({ "type": "string", "enum": ["normal", "shell", "ai"],
        "description": "normal: takes nothing on its own; shell: runs messages as commands at its prompt; ai: gets messages as prompts when its agent is ready" });
    let create = |kind: &str| {
        let mut p = json!({
            "command": s("Program to run (must be in keepane's agent-commands), e.g. claude or pwsh"),
            "args": { "type": "array", "items": { "type": "string" }, "description": "Its arguments" },
            "name": s("Name for the new pane, so %name finds it"),
            "mode": mode.clone(),
            "message": s("First message for it: delivered as soon as it is ready"),
            "cwd": s("Working directory"),
        });
        match kind {
            "session" => p["session"] = s("Name for the new session"),
            "window" => p["target"] = s("Session to add the window to (default: yours)"),
            _ => {
                p["target"] = s("Pane to split (default: yours)");
                p["horizontal"] = b("Side by side instead of one above the other");
            }
        }
        p
    };
    vec![
        tool("whoami", "Your pane: full address, place, name, work mode.", json!({}), &[]),
        tool(
            "list_panes",
            "Every pane: address, place, name, mode, idle, inbox size, program, status.",
            json!({}),
            &[],
        ),
        tool(
            "send_message",
            "Send a message to a pane. Returns its id and whether it was delivered or queued. \
             With re, it answers that message (and goes to its sender unless to says otherwise); \
             with task, it carries that task on. Who sends it, its id and its hop are keepane's to fill in.",
            json!({
                "to": pane.clone(),
                "text": s("The message"),
                "re": n("The number of the message this answers"),
                "task": n("The number of the task this carries on"),
                "wait_seconds": n("Wait until it is delivered"),
            }),
            &["text"],
        ),
        tool(
            "reply",
            "Answer the sender of the message you are working on.",
            json!({ "text": s("The answer") }),
            &["text"],
        ),
        tool(
            "wait_message",
            "Take the next message from your inbox, waiting up to timeout_seconds for one.",
            json!({ "timeout_seconds": n("How long to wait") }),
            &[],
        ),
        tool(
            "current_message",
            "The message you are working on, as keepane recorded it (true sender).",
            json!({}),
            &[],
        ),
        tool(
            "list_messages",
            "What waits in an inbox (yours by default).",
            json!({ "pane": pane.clone(), "all": b("Every pane's inbox") }),
            &[],
        ),
        tool(
            "trace_message",
            "A message's life: queued, delivered, done or failed, with a shell command's output.",
            json!({ "id": n("Message id"), "wait_seconds": n("Wait until it is done with") }),
            &["id"],
        ),
        tool(
            "drop_message",
            "Delete a queued message from an inbox you may change.",
            json!({ "id": n("Message id") }),
            &["id"],
        ),
        tool(
            "move_message",
            "Move a queued message in its inbox.",
            json!({ "id": n("Message id"), "to": { "type": "string", "enum": ["up", "down", "top"] } }),
            &["id", "to"],
        ),
        tool(
            "set_status",
            "Say what you are doing; shown on keepane's dashboard.",
            json!({ "text": s("Status, empty to clear") }),
            &["text"],
        ),
        tool("create_session", "Make a session with one pane.", create("session"), &[]),
        tool("create_window", "Make a window with one pane.", create("window"), &[]),
        tool("split_pane", "Split a pane, making a new one beside it.", create("split"), &[]),
        tool(
            "set_work_mode",
            "Set the work mode of your own pane (a mode is changed in the pane itself).",
            json!({ "mode": mode }),
            &["mode"],
        ),
        tool(
            "rename_pane",
            "Name a pane, so %name finds it.",
            json!({ "pane": pane.clone(), "name": s("Letters, digits, - and _") }),
            &["pane", "name"],
        ),
        tool("kill_pane", "Close a pane (kept 10 s for the user to undo).", json!({ "pane": pane }), &["pane"]),
        tool(
            "list_tasks",
            "Chains of messages: status, where, how long.",
            json!({ "session": s("Only this session") }),
            &[],
        ),
        tool("show_task", "A task's messages step by step.", json!({ "id": n("Task id") }), &["id"]),
        tool(
            "query_events",
            "Lines of keepane's event log (JSON), newest last.",
            json!({ "pane": s("Only about this pane or session"), "since": s("Like 15m, 2h, 1d"), "last": n("Only the last this many lines") }),
            &[],
        ),
    ]
}

/// The keepane command a tool call runs.
fn command(name: &str, a: &Value) -> Result<Vec<String>, String> {
    let str_of = |k: &str| a[k].as_str().map(String::from);
    let need = |k: &str| str_of(k).ok_or_else(|| format!("{name}: '{k}' is required"));
    let num = |k: &str| a[k].as_u64().map(|n| n.to_string());
    let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut argv = match name {
        "whoami" => v(&["whoami"]),
        "list_panes" => v(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_address}\t#{session_name}:#{window_index}.#{pane_index}\t#{pane_name}\t#{pane_work_mode}\t#{pane_idle}\t#{pane_inbox}\t#{pane_current_command}\t#{pane_status}",
        ]),
        "send_message" => {
            let mut c = v(&["send-message"]);
            if let Some(to) = str_of("to") {
                c.extend(["--to".into(), to]);
            }
            for f in ["re", "task"] {
                if let Some(x) = num(f) {
                    c.extend([format!("--{f}"), x]);
                }
            }
            if !c.iter().any(|x| x == "--to" || x == "--re") {
                return Err("send_message: to (a pane) or re (a message to answer) is required".into());
            }
            if let Some(w) = num("wait_seconds") {
                c.extend(["-w".into(), w]);
            }
            c.push("--".into());
            c.push(need("text")?);
            c
        }
        "reply" => vec!["send-message".into(), "-r".into(), "--".into(), need("text")?],
        "wait_message" => {
            let mut c = v(&["read-message"]);
            if let Some(w) = num("timeout_seconds") {
                c.extend(["-w".into(), w]);
            }
            c
        }
        "list_messages" => {
            let mut c = v(&["list-messages"]);
            if let Some(p) = str_of("pane") {
                c.extend(["-t".into(), p]);
            }
            if a["all"].as_bool() == Some(true) {
                c.push("-a".into());
            }
            c
        }
        "trace_message" => {
            let mut c = vec!["trace-message".into(), num("id").ok_or("trace_message: 'id' is required")?];
            if let Some(w) = num("wait_seconds") {
                c.extend(["-w".into(), w]);
            }
            c
        }
        "drop_message" => vec!["drop-message".into(), num("id").ok_or("drop_message: 'id' is required")?],
        "move_message" => vec!["move-message".into(), num("id").ok_or("move_message: 'id' is required")?, need("to")?],
        "set_status" => vec!["pane-status".into(), "--".into(), str_of("text").unwrap_or_default()],
        "create_session" | "create_window" | "split_pane" => {
            let kind = match name {
                "create_session" => "session",
                "create_window" => "window",
                _ => "split",
            };
            let mut c = vec!["create-pane".into(), "-k".into(), kind.into()];
            for (key, flag) in
                [("session", "-s"), ("target", "-t"), ("cwd", "-c"), ("name", "-n"), ("mode", "-m"), ("message", "-M")]
            {
                if let Some(x) = str_of(key) {
                    c.extend([flag.into(), x]);
                }
            }
            if a["horizontal"].as_bool() == Some(true) {
                c.push("-h".into());
            }
            if let Some(cmd) = str_of("command") {
                c.push("--".into());
                c.push(cmd);
                if let Some(args) = a["args"].as_array() {
                    c.extend(args.iter().filter_map(|x| x.as_str().map(String::from)));
                }
            }
            c
        }
        "set_work_mode" => vec!["set-work-mode".into(), need("mode")?],
        "rename_pane" => vec!["rename-pane".into(), "-t".into(), need("pane")?, need("name")?],
        "kill_pane" => vec!["kill-pane".into(), "-t".into(), need("pane")?],
        "list_tasks" => {
            let mut c = v(&["list-tasks"]);
            if let Some(s) = str_of("session") {
                c.extend(["-t".into(), s]);
            }
            c
        }
        "show_task" => vec!["show-task".into(), num("id").ok_or("show_task: 'id' is required")?],
        "query_events" => {
            let mut c = v(&["list-events"]);
            if let Some(p) = str_of("pane") {
                c.extend(["-t".into(), p]);
            }
            if let Some(s) = str_of("since") {
                c.extend(["-S".into(), s]);
            }
            if let Some(n) = num("last") {
                c.extend(["-n".into(), n]);
            }
            c
        }
        other => return Err(format!("unknown tool: {other}")),
    };
    argv.shrink_to_fit();
    Ok(argv)
}

/// Answer one JSON-RPC request; None for a notification.
async fn answer(socket: &str, pane: Option<u32>, req: &Value) -> Option<Value> {
    let id = req.get("id")?.clone();
    let method = req["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => {
            let asked = req["params"]["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOLS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOLS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "keepane", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or_default();
            let args = req["params"].get("arguments").cloned().unwrap_or(json!({}));
            Ok(call(socket, pane, name, &args).await)
        }
        other => Err(json!({ "code": -32601, "message": format!("method not found: {other}") })),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    })
}

/// Run a tool: its command's output, or its error, as the tool's text.
async fn call(socket: &str, pane: Option<u32>, name: &str, args: &Value) -> Value {
    let text = |t: String, err: bool| json!({ "content": [{ "type": "text", "text": t }], "isError": err });
    if name == "current_message" {
        let id = match crate::client::query_as(socket, &["display-message", "-p", "#{pane_message}"], pane).await {
            Ok((0, out, _)) => out.trim().to_string(),
            Ok((_, _, err)) => return text(err.trim().to_string(), true),
            Err(e) => return text(format!("{e:#}"), true),
        };
        if id.is_empty() {
            return text("You are not working on a message.".into(), false);
        }
        return match crate::client::query_as(socket, &["trace-message", &id], pane).await {
            Ok((code, out, err)) => text(if code == 0 { out } else { err }.trim_end().to_string(), code != 0),
            Err(e) => text(format!("{e:#}"), true),
        };
    }
    let argv = match command(name, args) {
        Ok(a) => a,
        Err(e) => return text(e, true),
    };
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    match crate::client::query_as(socket, &argv, pane).await {
        Ok((code, out, err)) => {
            let body = if code == 0 { out } else { err };
            let body = body.trim_end();
            text(if body.is_empty() { "ok".into() } else { body.to_string() }, code != 0)
        }
        Err(e) => text(format!("{e:#}"), true),
    }
}

/// Serve MCP on stdin/stdout until stdin closes.
pub async fn run(socket: &str) -> Result<i32> {
    let pane = crate::client::own_pane(socket);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // stdin is read on a thread of its own; the requests are answered one
    // at a time, in order.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout();
    while let Some(line) = rx.recv().await {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => answer(socket, pane, &req).await,
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }),
            ),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_turns_into_a_command_keepane_knows() {
        let args = json!({
            "to": "%b", "text": "hi", "pane": "%b", "mode": "ai", "name": "n", "id": 3, "command": "claude",
            "args": ["-p", "x"], "wait_seconds": 5, "timeout_seconds": 5, "session": "s",
        });
        let moving = json!({"id": 3, "to": "top"});
        for t in tools() {
            let name = t["name"].as_str().unwrap();
            if name == "current_message" {
                continue;
            }
            let argv = command(name, if name == "move_message" { &moving } else { &args })
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let parsed = crate::command::parse(&argv);
            assert!(parsed.is_ok(), "{name}: {argv:?} -> {parsed:?}");
        }
        assert_eq!(
            command("send_message", &json!({"to": "%b", "text": "-not a flag"})).unwrap(),
            ["send-message", "--to", "%b", "--", "-not a flag"]
        );
        assert!(command("send_message", &json!({"to": "%b"})).is_err());
        // Answering a message, carrying a task on: by their numbers.
        assert_eq!(
            command("send_message", &json!({"re": 7, "task": 3, "text": "done"})).unwrap(),
            ["send-message", "--re", "7", "--task", "3", "--", "done"]
        );
        assert!(command("send_message", &json!({"text": "to nobody"})).is_err(), "to or re is needed");
        // Text that looks like a flag stays text, for every tool that sends some.
        for (tool, args) in [
            ("reply", json!({"text": "-x"})),
            ("set_status", json!({"text": "-x"})),
            ("send_message", json!({"to": "%b", "text": "-x"})),
        ] {
            let argv = command(tool, &args).unwrap();
            assert_eq!(argv[argv.len() - 2..], ["--", "-x"], "{tool}: {argv:?}");
            assert!(crate::command::parse(&argv).is_ok(), "{tool}: {argv:?}");
        }
        assert_eq!(
            command("create_window", &json!({"command": "claude", "args": ["--model", "x"], "mode": "ai"})).unwrap(),
            ["create-pane", "-k", "window", "-m", "ai", "--", "claude", "--model", "x"]
        );
    }

    #[tokio::test]
    async fn it_speaks_the_protocol_without_a_server() {
        let init =
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}});
        let r = answer("no-such-socket", None, &init).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "keepane");
        let r =
            answer("x", None, &json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}})).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOLS[0], "an unknown version gets the newest");
        assert!(answer("x", None, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.is_none());
        let r = answer("x", None, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).await.unwrap();
        assert!(r["result"]["tools"].as_array().unwrap().len() >= 20);
        let r = answer("x", None, &json!({"jsonrpc": "2.0", "id": 4, "method": "nope"})).await.unwrap();
        assert_eq!(r["error"]["code"], -32601);
        // A tool whose arguments are wrong is the tool's error, not the protocol's.
        let call = json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "send_message", "arguments": {}}});
        let r = answer("x", None, &call).await.unwrap();
        assert_eq!(r["result"]["isError"], true);
    }
}
