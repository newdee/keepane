//! Records a scripted keepane session and writes one JSON file per frame.
//!
//! The real `keepane.exe` runs inside a ConPTY, exactly as under Windows
//! Terminal; this program plays a fixed sequence of keystrokes into it, parses
//! the VT stream it sends back and dumps the screen as coloured text runs.
//! `installer/../tools/render-frames.ps1` turns those into PNGs, and ffmpeg
//! turns the PNGs into the GIF used by the README and the site.
//!
//! It is a test so that it runs the same way the console tests do, and it is
//! ignored by default because it is a recording, not an assertion. The whole
//! job (the three recordings, the frames, the GIF and MP4, and every still that
//! `still()` marks, into docs/img) is one command:
//!
//! ```powershell
//! pwsh -File tools/make-demos.ps1
//! ```

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const COLS: u16 = 96;
const ROWS: u16 = 26;
/// Wall-clock distance between recorded frames.
const FRAME_MS: u64 = 200;

#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_demo() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT").unwrap_or_else(|_| "target/demo-frames".into());
    let mut d = Demo::start("demo", &out_dir, "");
    let (rec, socket) = (&mut d.rec, d.socket.clone());

    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(3);

    // It starts as one command in an ordinary terminal.
    rec.type_line(&format!("keepane -L {socket} new -s dev"));
    rec.wait_for("session", |s| s.contents().contains("0:pwsh*"), 30);
    rec.hold(4);

    // Two shells side by side. Inside a pane, keepane commands need no -L.
    rec.key("\x02%");
    rec.hold(3);
    rec.type_line("keepane list-panes");
    rec.hold(5);

    // Three.
    rec.key("\x02\"");
    rec.hold(3);
    rec.type_line("1..3 | ForEach-Object { \"build step $_ ok\" }");
    rec.hold(5);
    rec.still("panes");

    // One line typed into every pane at once: `set sync` at the command
    // prompt, the option's name completed with Tab.
    rec.key("\x02:");
    rec.hold(2);
    rec.type_text("set sync");
    rec.hold(2);
    rec.key("\t");
    rec.wait_for("sync completed", |s| s.contents().contains("set synchronize-panes"), 10);
    rec.hold(4);
    rec.key("\r");
    rec.hold(2);
    rec.type_line("echo 'typed once, run in every pane'");
    rec.wait_for("three echoes", |s| s.contents().matches("typed once, run in every pane").count() >= 6, 20);
    rec.hold(6);
    rec.still("sync");
    // An on/off option given no value flips: sync is off again.
    rec.key("\x02:");
    rec.hold(1);
    rec.type_line("set sync");
    rec.hold(3);

    // vim keys move between them, and repeat without the prefix again:
    // left, back right, then down into the pane below.
    rec.key("\x02h");
    rec.hold(3);
    rec.key("\x02l");
    rec.hold(1);
    // Within repeat-time, so this bare j moves a pane instead of typing one.
    rec.key("j");
    rec.hold(4);

    // Zoom one pane full screen and come back.
    rec.key("\x02z");
    rec.hold(4);
    rec.still("zoom");
    rec.key("\x02z");
    rec.hold(3);

    // The pane menu: every pane command behind one key, no cheat sheet.
    rec.key("\x02>");
    rec.hold(5);
    rec.still("menu");
    rec.key("\x1b");
    rec.hold(2);

    // A second window, and the picker that switches between them.
    rec.key("\x02c");
    rec.hold(3);
    rec.type_line("cmd.exe /c ver");
    rec.hold(4);
    rec.key("\x02w");
    rec.hold(5);
    rec.still("picker");
    rec.key("j");
    rec.hold(3);
    rec.key("\r");
    rec.hold(4);

    // Detach: back to the plain shell, everything still running.
    rec.key("\x02d");
    rec.hold(5);
    rec.still("detach");

    // Attach again, exactly where it was left.
    rec.type_line(&format!("keepane -L {socket} attach"));
    rec.hold(4);
    rec.key("\x020");
    rec.hold(8);
    rec.still("attach");

    d.finish();
}

/// The second recording: what a background job looks like when it finishes
/// somewhere you are not looking, and the two windows over the window.
#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_alerts() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT2").unwrap_or_else(|_| "target/demo-frames-2".into());
    // The alert flags are off by default, as in tmux; this is the recording
    // of what turning them on looks like.
    let mut d = Demo::start("demo2", &out_dir, "set -g monitor-activity on\nset -g remain-on-exit on\n");
    let (rec, socket) = (&mut d.rec, d.socket.clone());

    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(2);
    rec.type_line(&format!("keepane -L {socket} new -s ops"));
    rec.wait_for("session", |s| s.contents().contains("0:pwsh*"), 30);
    rec.hold(3);

    // A second window with a job in it that takes a while.
    rec.key("\x02c");
    rec.wait_for("window 1", |s| s.contents().contains("1:pwsh*"), 20);
    rec.hold(2);
    rec.type_line("Start-Sleep -Seconds 6; 'deploy finished'");
    rec.hold(2);

    // Walk away from it: back to window 0, carry on working.
    rec.key("\x020");
    rec.hold(3);
    rec.type_line("keepane list-windows");
    rec.hold(4);

    // The job finishes over there: the status line grows a # on window 1,
    // and so does the listing.
    rec.wait_for("activity flag", |s| s.contents().contains("1:pwsh-#"), 30);
    rec.hold(4);
    rec.type_line("keepane list-windows");
    rec.hold(6);
    rec.still("alert");

    // C-b M-n goes straight to the window that has something to say.
    rec.key("\x02\x1bn");
    rec.wait_for("the finished job", |s| s.contents().contains("deploy finished"), 20);
    rec.hold(6);

    // A job in a pane of its own that falls over: remain-on-exit keeps the
    // pane, what it printed, and the code it died with.
    rec.type_line("keepane split-window -v cmd.exe /c \"echo tests failed & exit 3\"");
    rec.wait_for("the exit note", |s| s.contents().contains("exited with 3"), 20);
    rec.hold(7);

    // A popup: a program in a box over the window, gone when it is done.
    rec.key("\x02:");
    rec.hold(2);
    rec.type_line("display-popup -w 60% -h 40% cmd.exe /c keepane list-windows");
    rec.wait_for("the popup", |s| s.contents().contains("press any key"), 20);
    rec.hold(7);
    rec.still("popup");
    rec.key(" ");
    rec.hold(3);

    d.finish();
}

/// The third recording: each command's time at the end of its line, the
/// history log read back a day at a time, and a pane closed by mistake
/// coming back.
#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_history() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT3").unwrap_or_else(|_| "target/demo-frames-3".into());
    let mut d = Demo::start("demo3", &out_dir, "");
    // Earlier days, so the picker shows what a few days of use leave: a
    // second session's pane over three days, another pane yesterday.
    let history = d.tmp.join("sessions").join("history");
    let today = chrono::Local::now().date_naive();
    let seed = |session: &str, key: &str, days_ago: u64, text: &str| {
        let dir = history.join(session).join(key);
        std::fs::create_dir_all(&dir).expect("history dir");
        let day = today - chrono::Days::new(days_ago);
        std::fs::write(dir.join(format!("{}.log", day.format("%Y-%m-%d"))), text).expect("history file");
    };
    let build = "── 09:12:03 · 3m41s · ✓ ──\nPS> cargo build --release\n   Compiling keepane v0.12.0\n    \
                 Finished `release` profile [optimized] target(s) in 3m 41s\n";
    seed("ops", "0.0", 1, &build.repeat(40));
    seed("ops", "0.0", 2, &build.repeat(25));
    seed("ops", "0.0", 5, &build.repeat(60));
    seed("dev", "0.1", 1, "── 17:40:12 · 1.2s · ✗ 1 ──\nPS> npm test\n2 tests failed\n");
    let (rec, socket) = (&mut d.rec, d.socket.clone());

    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(2);
    rec.type_line(&format!("keepane -L {socket} new -s dev"));
    rec.wait_for("session", |s| s.contents().contains("0:pwsh*"), 30);
    rec.hold(3);

    // C-b C-t: when each command started, how long it took, how it ended,
    // in the blank end of its line.
    rec.key("\x02\x14");
    rec.wait_for("times on", |s| s.contents().contains("pane-timestamps on"), 10);
    rec.hold(4);
    let stamped = |n: usize| move |s: &vt100::Screen| s.contents().matches(['✓', '✗']).count() >= n;
    rec.type_line("Start-Sleep 2; 'build ok'");
    rec.wait_for("first time", stamped(1), 20);
    rec.hold(4);
    rec.type_line("cmd /c \"echo 2 tests failed & exit 1\"");
    rec.wait_for("second time", stamped(2), 20);
    rec.hold(4);
    rec.type_line("1..4 | ForEach-Object { \"step $_ done\" }");
    rec.wait_for("third time", stamped(3), 20);
    rec.hold(6);
    rec.still("timestamps");

    // Enough output to scroll: what leaves the screen goes to today's file.
    rec.type_line("1..40 | ForEach-Object { \"log line $_\" }");
    rec.wait_for(
        "the long one",
        |s| {
            let rows: Vec<String> = s.rows(0, COLS).collect();
            rows.iter().position(|r| r.trim() == "log line 40").is_some_and(|i| rows[i + 1].starts_with("PS>"))
        },
        20,
    );
    rec.hold(4);

    // C-b /: the pane positions with history, and their days.
    rec.key("\x02/");
    rec.wait_for("the history picker", |s| s.contents().contains("today"), 10);
    rec.hold(6);
    // Enter: that day in the viewer, at the end; [ goes back a command.
    rec.key("\r");
    rec.wait_for("the viewer", |s| s.contents().contains("q quit"), 10);
    rec.hold(5);
    rec.key("[");
    rec.hold(4);
    rec.key("[");
    rec.hold(6);
    rec.still("viewer");
    rec.key("q");
    rec.hold(3);

    // A pane closed by mistake: C-b u within ten seconds brings it back,
    // with what it was running.
    rec.type_line("Clear-Host");
    rec.hold(2);
    rec.key("\x02%");
    rec.hold(3);
    rec.type_line("'a job worth keeping'");
    rec.hold(3);
    rec.key("\x02x");
    rec.wait_for("the confirmation", |s| s.contents().contains("(y/n)"), 10);
    rec.hold(3);
    rec.key("y");
    rec.wait_for("the undo hint", |s| s.contents().contains("undo-kill"), 10);
    rec.hold(5);
    rec.key("\x02u");
    rec.wait_for("the pane back", |s| s.contents().contains("a job worth keeping"), 10);
    rec.hold(8);

    d.finish();
}

#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_messages() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT4").unwrap_or_else(|_| "target/demo-frames-4".into());
    let mut d = Demo::start("demo4", &out_dir, "");
    let (rec, socket) = (&mut d.rec, d.socket.clone());

    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(2);
    rec.type_line(&format!("keepane -L {socket} new -s work"));
    rec.wait_for("session", |s| s.contents().contains("0:pwsh*"), 30);
    rec.hold(2);
    // A second pane; the first one becomes "builder", taking commands.
    rec.key("\x02%");
    rec.hold(3);
    rec.type_line("keepane rename-pane -t :.0 builder");
    rec.hold(2);
    // A pane's mode is set in it, or by a person: at the C-b : prompt.
    rec.key("\x02:");
    rec.hold(1);
    rec.type_text("set-work-mode -t %builder shell");
    rec.hold(2);
    rec.key("\r");
    rec.hold(3);
    rec.type_line("keepane send-message -t %builder \"git status --short; 'tests: 42 passed'\"");
    rec.wait_for("it ran there", |s| s.contents().contains("tests: 42 passed"), 20);
    rec.hold(5);
    rec.type_line("keepane trace-message 1");
    rec.wait_for("its record", |s| s.contents().contains("output:"), 10);
    rec.hold(8);
    rec.still("messages");

    // An agent's pane with work waiting for it (no agent says ready here).
    rec.type_line("keepane create-pane -k window -n agent -m ai");
    rec.type_line(
        "keepane send-message -t %agent 'review the diff'; keepane send-message -t %agent 'then run the tests'",
    );
    rec.hold(4);

    // C-b v: every pane at a glance.
    rec.key("\x02v");
    rec.wait_for("the dashboard", |s| s.contents().contains("[1] Panes"), 15);
    rec.hold(6);
    rec.key("j");
    rec.hold(2);
    rec.key("j");
    rec.wait_for("its inbox", |s| s.contents().contains("review the diff"), 10);
    rec.hold(6);
    // The inbox: the second message first.
    rec.key("2");
    rec.hold(2);
    rec.key("j");
    rec.hold(2);
    rec.key("t");
    rec.wait_for("moved", |s| s.contents().contains("done: move-message"), 10);
    rec.hold(4);
    // Read in full on the right.
    rec.key("\r");
    rec.wait_for("its record", |s| s.contents().contains("text:"), 10);
    rec.hold(6);
    rec.still("dashboard");
    rec.key("q");
    rec.hold(4);

    d.finish();
}

/// The sixth recording: `C-b w`, the chart of every session, window and
/// pane, moved through with hjkl; `a` opens all of it, `v` turns it into
/// the tree and the list.
#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_chart() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT6").unwrap_or_else(|_| "target/demo-frames-6".into());
    let mut d = Demo::start("demo6", &out_dir, "");
    let (rec, socket) = (&mut d.rec, d.socket.clone());
    let kp = |args: &[&str]| {
        let out = std::process::Command::new(&d.exe).args(["-L", &socket]).args(args).output().expect("keepane");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(2);
    rec.type_line(&format!("keepane -L {socket} new -s dev -n edit"));
    rec.wait_for("session", |s| s.contents().contains("0:edit*"), 30);
    // Something to look at: a second pane with a name, a second window, two
    // more sessions.
    kp(&["split-window", "-h", "-t", "dev:edit"]);
    kp(&["rename-pane", "-t", "dev:edit.1", "builder"]);
    kp(&["set-work-mode", "-t", "dev:edit.1", "shell"]);
    kp(&["new-window", "-d", "-t", "dev", "-n", "logs"]);
    kp(&["new", "-d", "-s", "ops", "-n", "deploy"]);
    kp(&["split-window", "-d", "-t", "ops:deploy"]);
    kp(&["new", "-d", "-s", "notes", "-n", "todo"]);
    rec.hold(3);
    // C-b w: the chart, on the pane you are in.
    rec.key("\x02w");
    rec.wait_for("the chart", |s| s.contents().contains("hjkl move"), 10);
    rec.hold(6);
    rec.still("chart");
    // h and l along the panes, k up to the window and the session, l to the
    // next session (its rows come along), j down again.
    for k in ["h", "l", "k", "k", "l", "j", "j", "h", "k", "k"] {
        rec.key(k);
        rec.hold(3);
    }
    rec.hold(2);
    // a: every node at once.
    rec.key("a");
    rec.wait_for("all of it", |s| s.contents().contains("a branch"), 10);
    rec.hold(8);
    rec.still("chart-all");
    rec.key("a");
    rec.hold(3);
    // v: the tree, the list, the chart again.
    rec.key("v");
    rec.wait_for("the tree", |s| s.contents().contains("v list"), 10);
    rec.hold(6);
    rec.key("v");
    rec.wait_for("the list", |s| s.contents().contains("v chart"), 10);
    rec.hold(6);
    rec.key("v");
    rec.wait_for("the chart again", |s| s.contents().contains("hjkl move"), 10);
    rec.hold(4);
    rec.key("q");
    rec.hold(3);

    d.finish();
}

/// The tour's terminal: room for four panes, and for the QR code.
const TOUR_COLS: u16 = 100;
const TOUR_ROWS: u16 = 30;

/// A small MCP client, as an agent (Claude Code, Codex...) would call keepane:
/// `keepane mcp` over stdio, one JSON-RPC call at a time, each shown as it
/// goes and what came back.
const MCP_CLIENT: &str = r#"# What an agent does over MCP, one call at a time.
$p = [System.Diagnostics.Process]::new()
$p.StartInfo.FileName = (Get-Command keepane).Source
$p.StartInfo.Arguments = 'mcp'
$p.StartInfo.RedirectStandardInput = $true
$p.StartInfo.RedirectStandardOutput = $true
$p.StartInfo.UseShellExecute = $false
[void]$p.Start()
$script:id = 0
function Call($method, $params) {
    $script:id++
    $p.StandardInput.WriteLine((@{ jsonrpc = '2.0'; id = $script:id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 6))
    ($p.StandardOutput.ReadLine() | ConvertFrom-Json).result
}
function Tool($name, $arguments, [scriptblock]$show) {
    Write-Host "-> $name " -NoNewline -ForegroundColor Cyan
    Write-Host (($arguments.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' ') -ForegroundColor DarkGray
    $r = Call 'tools/call' @{ name = $name; arguments = $arguments }
    Write-Host "<- $(& $show $r.content[0].text)" -ForegroundColor Green
    Start-Sleep -Milliseconds 900
}
$init = Call 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'agent'; version = '1' } }
$p.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
Write-Host "-> initialize" -ForegroundColor Cyan
Write-Host "<- $($init.serverInfo.name) $($init.serverInfo.version), $((Call 'tools/list' @{}).tools.Count) tools" -ForegroundColor Green
Start-Sleep -Milliseconds 900
Tool split_pane ([ordered]@{ target = '%lead'; name = 'tests'; mode = 'shell' }) { param($t) ($t -split "`n")[0] }
Tool send_message ([ordered]@{ to = '%tests'; text = 'git status -sb'; wait_seconds = 10 }) { param($t) ($t -split "`n")[0] }
Tool list_panes ([ordered]@{}) { param($t) (($t -split "`n") | Where-Object { $_ } | ForEach-Object { $f = $_ -split "`t"; "%$($f[2]) $($f[3])" }) -join ', ' }
$p.StandardInput.Close()
[void]$p.WaitForExit(3000)
"#;

/// The tour for the README's top and the announcement: four chapters, the
/// terminal on the left and, beside it, what the chapter shows, then the
/// phone itself.
#[test]
#[ignore = "recording, not an assertion; run with --ignored"]
fn record_tour() {
    let out_dir = std::env::var("KEEPANE_DEMO_OUT5").unwrap_or_else(|_| "target/demo-frames-5".into());
    // Each pane's name, work mode and inbox on its top border.
    let conf = "set -g pane-border-status top\n\
                set -g pane-border-format \" #{pane_name} · #{pane_work_mode} · inbox #{pane_inbox} \"\n";
    let mut d = Demo::start_sized("tour", &out_dir, conf, TOUR_COLS, TOUR_ROWS);
    // A project with a little history, and the MCP client in it.
    let project = d.tmp.join("project");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "user.name=demo", "-c", "user.email=demo@example.com"])
            .args(args)
            .current_dir(&project)
            .status()
            .expect("git");
    };
    for (file, message) in
        [("README.md", "init"), ("greet.ps1", "add greet"), ("greet.tests.ps1", "add tests for greet")]
    {
        std::fs::write(project.join(file), format!("# {message}\n")).unwrap();
        git(&["add", file]);
        git(&["commit", "-q", "-m", message]);
    }
    std::fs::write(project.join("agent.ps1"), MCP_CLIENT).unwrap();
    let (rec, socket) = (&mut d.rec, d.socket.clone());

    // 1. Panes are actors.
    rec.caption(
        1,
        "Panes are actors",
        &[
            "Each pane has an inbox,",
            "and panes send each other",
            "messages. Its work mode",
            "says what a message does:",
            "",
            "normal  you type in it",
            "shell   it runs as a command",
            "ai      its agent gets a prompt",
        ],
    );
    rec.wait_for("shell", |s| s.contents().contains("PS>"), 30);
    rec.hold(2);
    rec.type_line(&format!("keepane -L {socket} new -s work"));
    rec.wait_for("session", |s| s.contents().contains("0:pwsh*"), 30);
    rec.hold(2);
    rec.type_line("keepane rename-pane lead");
    rec.hold(2);
    rec.type_line("keepane create-pane -h -n build -m shell");
    rec.hold(3);
    rec.type_line("keepane create-pane -t %build -n agent -m ai");
    rec.hold(4);
    rec.type_line("keepane send-message --to %build 'git log --oneline -3'");
    rec.until("it ran there", |s| s.contents().contains("add tests for greet"), 20);
    rec.hold(5);
    rec.type_line("keepane send-message --to %agent 'review the last commit'");
    rec.until("queued for the agent", |s| s.contents().contains("ai · inbox 1"), 10);
    rec.hold(5);
    rec.type_line("keepane trace-message 1");
    rec.until("its record", |s| s.contents().contains("output:"), 10);
    rec.hold(10);

    // 2. MCP.
    rec.caption(
        2,
        "MCP for agents",
        &[
            "keepane mcp gives an agent",
            "tools to make sessions,",
            "windows and panes, and to",
            "send, read and trace",
            "messages between them.",
            "",
            "Here a small MCP client",
            "does what an agent would.",
        ],
    );
    rec.type_line("clear");
    rec.hold(1);
    rec.type_line("./agent.ps1");
    rec.until("the new pane", |s| s.contents().contains("tests · shell"), 20);
    rec.until("the list", |s| s.contents().contains("%tests shell"), 20);
    rec.hold(10);

    // 3. Watching.
    rec.caption(
        3,
        "See every pane",
        &[
            "prefix v: the dashboard.",
            "Every pane's state now:",
            "mode, busy or idle, inbox,",
            "what it is doing.",
            "",
            "And what happened before:",
            "each pane's events, each",
            "message from send to done.",
        ],
    );
    rec.key("\x02v");
    rec.wait_for("the dashboard", |s| s.contents().contains("[1] Panes"), 15);
    rec.hold(5);
    // Down the list to a pane: the main panel's title names the chosen one.
    let choose = |rec: &mut Recorder, name: &str| {
        // Row by row: ConPTY marks its rows wrapped, so `contents()` runs
        // the whole screen together.
        let chosen = |s: &vt100::Screen| {
            s.rows(0, TOUR_COLS).any(|l| l.split("[0] %").nth(1).is_some_and(|t| t.contains(&format!(" {name} "))))
        };
        for _ in 0..8 {
            if chosen(rec.parser.screen()) {
                return;
            }
            rec.key("j");
            rec.hold(1);
        }
        rec.wait_for(name, chosen, 5);
    };
    // The builder: what happened to it.
    choose(rec, "build");
    rec.hold(2);
    rec.key("]");
    rec.key("]");
    rec.hold(1);
    rec.hold(9);
    // The agent: its inbox.
    rec.key("[");
    rec.key("[");
    choose(rec, "agent");
    rec.wait_for("its inbox", |s| s.contents().contains("review the last"), 10);
    rec.hold(8);
    rec.key("q");
    rec.hold(2);

    // 4. The phone.
    rec.caption(
        4,
        "On your phone",
        &[
            "keepane web, then scan.",
            "On the Wi-Fi, or through",
            "Tailscale from anywhere:",
            "every window and pane,",
            "live. Type on the phone;",
            "it runs on the computer.",
        ],
    );
    rec.key("\x02z");
    rec.type_line("clear");
    rec.type_line("keepane web --bind 127.0.0.1 --port 7690");
    rec.wait_for("the code", |s| s.contents().contains("Scan with"), 20);
    rec.hold(8);
    let status =
        std::process::Command::new(&d.exe).args(["-L", &socket, "web", "status"]).output().expect("web status");
    let status = String::from_utf8_lossy(&status.stdout).to_string();
    let url = keepane::web::status_url(&status).expect("serving").to_string();
    let rec = &mut d.rec;
    let mut phone = Phone::start(&out_dir);
    let title = "On your phone";
    phone.step(&format!("{{\"do\":\"open\",\"url\":{}}}", json_string(&url)));
    let shot = phone.shot();
    rec.phone(4, title, &shot);
    rec.hold(8);
    phone.step("{\"do\":\"tap\",\"name\":\"build\"}");
    let shot = phone.shot();
    rec.phone(4, title, &shot);
    rec.hold(6);
    for part in ["git log ", "--oneline ", "-1"] {
        phone.step(&format!("{{\"do\":\"type\",\"text\":{}}}", json_string(part)));
        let shot = phone.shot();
        rec.phone(4, title, &shot);
        rec.hold(2);
    }
    // Send puts the text in; Send again (the box empty) is the Enter.
    phone.step("{\"do\":\"send\"}");
    phone.step("{\"do\":\"send\"}");
    phone.step("{\"do\":\"wait\",\"text\":\"git log --oneline -1\"}");
    std::thread::sleep(Duration::from_millis(600));
    let shot = phone.shot();
    rec.phone(4, title, &shot);
    rec.hold(8);
    // Back on the computer: it ran in %build.
    rec.key("\x02z");
    rec.hold(6);
    rec.type_line("clear");
    rec.type_line("keepane web status");
    rec.wait_for("who is on it", |s| s.contents().contains("watching"), 15);
    rec.hold(10);
    drop(phone);
    d.finish();
}

/// Everything a recording needs: a pty running a plain shell with keepane on
/// the PATH, a scratch directory, and the frame recorder itself.
struct Demo {
    rec: Recorder,
    tmp: std::path::PathBuf,
    exe: String,
    socket: String,
    /// Where the panes work; given back when the demo is dropped.
    _drive: Drive,
}

/// A drive letter standing for the recording's project directory (`subst`,
/// no administrator rights needed), so the paths on screen are short and
/// say nothing about this machine. Given back when the recording ends,
/// however it ends.
struct Drive(String);

impl Drive {
    fn map(dir: &std::path::Path) -> Drive {
        for letter in ['W', 'V', 'U', 'T', 'S', 'R', 'Q'] {
            let d = format!("{letter}:");
            if std::path::Path::new(&format!("{d}\\")).exists() {
                continue;
            }
            let ok = std::process::Command::new("subst").arg(&d).arg(dir).status().is_ok_and(|s| s.success());
            if ok {
                return Drive(d);
            }
        }
        panic!("no free drive letter for the recording");
    }
}

impl Drop for Drive {
    fn drop(&mut self) {
        let _ = std::process::Command::new("subst").args([self.0.as_str(), "/d"]).status();
    }
}

impl Demo {
    fn start(socket: &str, out_dir: &str, extra_conf: &str) -> Demo {
        Demo::start_sized(socket, out_dir, extra_conf, COLS, ROWS)
    }

    fn start_sized(socket: &str, out_dir: &str, extra_conf: &str, cols: u16, rows: u16) -> Demo {
        let exe = env!("CARGO_BIN_EXE_keepane").to_string();
        std::fs::create_dir_all(out_dir).expect("create out dir");
        // A recording that failed half way leaves its server behind; start
        // from nothing so the take is the same every time.
        let _ = std::process::Command::new(&exe).args(["-L", socket, "kill-server"]).status();
        let tmp = std::env::temp_dir().join(format!("keepane-{socket}-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("temp dir");
        let sessions = tmp.join("sessions");
        // A project to work in: a git repository (its branch is on the status
        // line) on a drive of its own.
        let project = tmp.join("project");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("README.md"), "# demo\n").expect("project file");
        let _ = std::process::Command::new("git").args(["init", "-q", "-b", "main"]).current_dir(&project).status();
        let drive = Drive::map(&project);

        // A short prompt, so the recording shows keepane rather than path names.
        let prompt = tmp.join("prompt.ps1");
        // No prediction and no shared history: a recording must show keepane,
        // never whatever this machine's shell history happens to hold.
        // A shell started with a script of its own gets no prompt hook from
        // keepane, so the script installs it: the panes report their commands
        // (`pane-timestamps`, the history log) as a plain pwsh does.
        std::fs::write(
            &prompt,
            // The project's drive, so that `list-panes` (which prints each
            // pane's directory) and the status line show a short path.
            format!(
                "function global:prompt {{ 'PS> ' }}\n\
                 $Host.UI.RawUI.WindowTitle = 'pwsh'\n\
                 try {{ Set-PSReadLineOption -PredictionSource None -HistorySaveStyle SaveNothing }} catch {{}}\n\
                 Set-Location {}\\\n\
                 {}\n\
                 Clear-Host\n",
                drive.0,
                keepane::config::PROMPT_HOOK
            ),
        )
        .expect("prompt script");
        let shell = format!("pwsh.exe -NoLogo -NoProfile -NoExit -File {}", prompt.display());
        let conf = tmp.join("keepane.conf");
        // The recordings wear the Tokyo Night theme from themes/, status line
        // and all: the branch, the directory, the machine's load, the clock.
        // The focus frame is slowed down so that the pictures, taken five
        // times a second, catch it moving (160 ms, the default, falls
        // between two of them).
        // The look: KEEPANE_DEMO_THEME names a theme file (tokyo-day for the light
        // pictures), tokyo-night when it is not set.
        let name = std::env::var("KEEPANE_DEMO_THEME").unwrap_or_else(|_| "tokyo-night".into());
        let theme = std::fs::read_to_string(format!("{}/themes/{name}.conf", env!("CARGO_MANIFEST_DIR")))
            .unwrap_or_else(|e| panic!("themes/{name}.conf: {e}"));
        std::fs::write(
            &conf,
            format!("set -g default-command \"{shell}\"\n{theme}\nset -g animation-time 600\n{extra_conf}"),
        )
        .expect("config");

        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).expect("openpty");
        // The recording starts in a plain shell: keepane is started from it, and
        // detaching comes back to it.
        let mut cmd = CommandBuilder::new("pwsh.exe");
        cmd.args(["-NoLogo", "-NoProfile", "-NoExit", "-File", &prompt.to_string_lossy()]);
        let exe_dir = std::path::Path::new(&exe).parent().unwrap().to_string_lossy().into_owned();
        cmd.env("PATH", format!("{exe_dir};{}", std::env::var("PATH").unwrap_or_default()));
        cmd.env("KEEPANE_SESSIONS_DIR", sessions.to_string_lossy().to_string());
        cmd.env("KEEPANE_CONFIG", conf.to_string_lossy().to_string());
        cmd.env_remove("KEEPANE");
        cmd.env_remove("KEEPANE_PANE");
        let child = pair.slave.spawn_command(cmd).expect("spawn keepane");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        // Never drop the master on this thread: ClosePseudoConsole waits for
        // conhost, which may be blocked writing to us.
        std::mem::forget(pair.master);

        let rec = Recorder {
            parser: vt100::Parser::new(rows, cols, 200),
            rx,
            writer,
            out_dir: out_dir.to_string(),
            frame: 0,
            raw: Vec::new(),
            child,
            side: String::new(),
        };
        Demo { rec, tmp, exe, socket: socket.to_string(), _drive: drive }
    }

    /// Kill the server (and with it every pane) and the scratch directory.
    fn finish(mut self) {
        self.kill_server();
        let _ = self.rec.child.kill();
        let _ = std::fs::remove_dir_all(&self.tmp);
        println!("wrote {} frames to {}", self.rec.frame, self.rec.out_dir);
    }

    fn kill_server(&self) {
        let _ = std::process::Command::new(&self.exe)
            .args(["-L", &self.socket, "kill-server"])
            .env("KEEPANE_SESSIONS_DIR", self.tmp.join("sessions").to_string_lossy().to_string())
            .status();
    }
}

/// A recording that fails half way still takes its server down: left
/// running, it holds target\release\keepane.exe and the next build fails.
impl Drop for Demo {
    fn drop(&mut self) {
        self.kill_server();
    }
}

struct Recorder {
    parser: vt100::Parser,
    rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    out_dir: String,
    frame: u32,
    raw: Vec<u8>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// What the panel beside the terminal shows in each frame from now (a
    /// JSON object, "side" in the frame), for the tour; empty: no panel.
    side: String,
}

impl Recorder {
    /// Drain the pty for `d`, answering the cursor-position requests ConPTY
    /// makes before it lets a child run.
    fn pump(&mut self, d: Duration) {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.rx.recv_timeout(left) {
                Ok(b) => {
                    self.raw.extend_from_slice(&b);
                    self.parser.process(&b);
                    let n = b.windows(4).filter(|w| *w == b"\x1b[6n").count();
                    for _ in 0..n {
                        let (r, c) = self.parser.screen().cursor_position();
                        let reply = format!("\x1b[{};{}R", r + 1, c + 1);
                        self.writer.write_all(reply.as_bytes()).expect("answer DSR");
                        self.writer.flush().expect("flush DSR");
                    }
                }
                Err(_) => break,
            }
        }
    }

    fn wait_for(&mut self, what: &str, pred: impl Fn(&vt100::Screen) -> bool, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !pred(self.parser.screen()) {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for {what}:\n{}\nraw ({} bytes): {:?}\nchild: {:?}",
                self.parser.screen().contents(),
                self.raw.len(),
                String::from_utf8_lossy(&self.raw[..self.raw.len().min(600)]),
                self.child.try_wait()
            );
            self.pump(Duration::from_millis(100));
        }
    }

    /// Wait as `wait_for` does, recording the frames meanwhile: what shows
    /// up while waiting (a script printing line by line) is in the picture.
    fn until(&mut self, what: &str, pred: impl Fn(&vt100::Screen) -> bool, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !pred(self.parser.screen()) {
            assert!(Instant::now() < deadline, "timeout waiting for {what}:\n{}", self.parser.screen().contents());
            self.hold(1);
        }
    }

    /// Record `n` frames, one every FRAME_MS.
    fn hold(&mut self, n: u32) {
        for _ in 0..n {
            self.pump(Duration::from_millis(FRAME_MS));
            self.snapshot();
        }
    }

    fn key(&mut self, s: &str) {
        let _ = self.writer.write_all(s.as_bytes());
        let _ = self.writer.flush();
        self.pump(Duration::from_millis(120));
    }

    /// Type text the way a person does, without pressing Enter.
    fn type_text(&mut self, text: &str) {
        for (i, ch) in text.chars().enumerate() {
            let mut buf = [0u8; 4];
            let _ = self.writer.write_all(ch.encode_utf8(&mut buf).as_bytes());
            let _ = self.writer.flush();
            self.pump(Duration::from_millis(45));
            // A frame every few characters keeps the typing visible.
            if i % 4 == 0 {
                self.snapshot();
            }
        }
        self.snapshot();
    }

    /// Type a line the way a person does, then press Enter.
    fn type_line(&mut self, text: &str) {
        self.type_text(text);
        let _ = self.writer.write_all(b"\r");
        let _ = self.writer.flush();
        self.pump(Duration::from_millis(120));
    }

    /// One frame: every row as runs of identically styled text.
    fn snapshot(&mut self) {
        self.frame += 1;
        let path = format!("{}/f{:04}.json", self.out_dir, self.frame);
        std::fs::write(path, self.frame_json()).expect("write frame");
    }

    /// The screen as it is now, kept as the still `name` (the site's and
    /// the README's pictures are cut here, so a new take re-cuts them).
    fn still(&mut self, name: &str) {
        let path = format!("{}/still-{name}.json", self.out_dir);
        std::fs::write(path, self.frame_json()).expect("write still");
    }

    fn frame_json(&self) -> String {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut json = String::with_capacity(8192);
        let _ = write!(json, "{{\"cols\":{cols},\"rows\":{rows},\"lines\":[");
        for y in 0..rows {
            if y > 0 {
                json.push(',');
            }
            json.push('[');
            let mut first = true;
            let mut run = String::new();
            let mut run_style: Option<(String, String, bool, bool)> = None;
            let mut x = 0;
            while x < cols {
                let Some(cell) = screen.cell(y, x) else {
                    x += 1;
                    continue;
                };
                let wide = cell.is_wide();
                let style = (color(cell.fgcolor()), color(cell.bgcolor()), cell.bold(), cell.inverse());
                let text = cell.contents();
                let text = if text.is_empty() { " ".to_string() } else { text.to_string() };
                match &run_style {
                    Some(s) if *s == style => run.push_str(&text),
                    Some(s) => {
                        push_run(&mut json, &mut first, s, &run);
                        run.clear();
                        run.push_str(&text);
                        run_style = Some(style);
                    }
                    None => {
                        run.push_str(&text);
                        run_style = Some(style);
                    }
                }
                x += if wide { 2 } else { 1 };
            }
            if let Some(s) = &run_style {
                push_run(&mut json, &mut first, s, &run);
            }
            json.push(']');
        }
        let (cy, cx) = screen.cursor_position();
        let _ = write!(json, "],\"cursor\":[{cx},{cy}],\"cursor_visible\":{}", !screen.hide_cursor());
        if !self.side.is_empty() {
            let _ = write!(json, ",\"side\":{}", self.side);
        }
        json.push('}');
        json
    }

    /// The panel beside the terminal from the next frame on: a chapter's
    /// number, title and a few lines saying what it shows.
    fn caption(&mut self, n: u32, title: &str, lines: &[&str]) {
        let lines: Vec<String> = lines.iter().map(|l| json_string(l)).collect();
        self.side = format!("{{\"n\":{n},\"title\":{},\"lines\":[{}]}}", json_string(title), lines.join(","));
    }

    /// The panel shows the phone: the chapter's title over its picture.
    fn phone(&mut self, n: u32, title: &str, picture: &str) {
        self.side = format!("{{\"n\":{n},\"title\":{},\"phone\":{}}}", json_string(title), json_string(picture));
    }
}

/// `keepane web` in a phone-sized Edge (tools/phone-driver.mjs), one step at
/// a time, with a picture after each for the tour's panel.
struct Phone {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    dir: String,
    shots: u32,
}

impl Phone {
    fn start(dir: &str) -> Phone {
        // puppeteer-core lives where tools/make-demos.ps1 put it; the driver
        // is copied beside it so that its import resolves there.
        let npm = std::env::var("KEEPANE_PHONE_NPM")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/target/phone-shots/npm").into());
        let driver = std::path::Path::new(&npm).join("phone-driver.mjs");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tools/phone-driver.mjs"), &driver)
            .expect("copy the phone driver (run tools/make-demos.ps1, which installs puppeteer-core)");
        let edge = [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("Microsoft Edge");
        let mut child = std::process::Command::new("node")
            .arg(&driver)
            .arg(edge)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node");
        let stdin = child.stdin.take();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        Phone { child, stdin, stdout, dir: dir.to_string(), shots: 0 }
    }

    /// One step; the driver answers when it is done.
    fn step(&mut self, json: &str) {
        use std::io::BufRead;
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{json}").expect("to the phone driver");
        stdin.flush().unwrap();
        let mut answer = String::new();
        self.stdout.read_line(&mut answer).expect("from the phone driver");
        assert_eq!(answer.trim(), "ok", "phone: {json}");
    }

    /// A picture of the phone's screen now; its path.
    fn shot(&mut self) -> String {
        self.shots += 1;
        let path = format!("{}/phone-{:03}.png", self.dir, self.shots);
        self.step(&format!("{{\"do\":\"shot\",\"path\":{}}}", json_string(&path)));
        path
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "{{\"do\":\"quit\"}}");
        }
        // Closed now; a driver that still does not end is ended.
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
    }
}

fn push_run(json: &mut String, first: &mut bool, style: &(String, String, bool, bool), text: &str) {
    if !*first {
        json.push(',');
    }
    *first = false;
    let _ = write!(
        json,
        "{{\"fg\":\"{}\",\"bg\":\"{}\",\"b\":{},\"i\":{},\"t\":{}}}",
        style.0,
        style.1,
        style.2,
        style.3,
        json_string(text)
    );
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// "default", "0".."255" or "#rrggbb".
fn color(c: vt100::Color) -> String {
    match c {
        vt100::Color::Default => "default".into(),
        vt100::Color::Idx(i) => i.to_string(),
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}
