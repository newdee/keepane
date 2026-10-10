//! End-to-end tests: run the server in-process, talk to it over the named
//! pipe exactly like the real client, and check the rendered frames.

use keepane::ipc::{
    ClientMsg, KeyRecord, MouseRecord, PROTOCOL_VERSION, ServerMsg, pipe_name, read_frame, write_frame,
};
use keepane::keys::{LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, SHIFT_PRESSED, VK_ESCAPE, VK_RETURN};
use keepane::platform::ipc::{Stream as NamedPipeClient, connect as open_pipe};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};

const COLS: u16 = 80;
const ROWS: u16 = 24;

/// A shell with the prompt `keepane>` and nothing else of the user's (on
/// Unix the prompt comes from `PS1`, which `Harness::start` sets).
#[cfg(windows)]
const PROMPT_SHELL: &str = "cmd.exe /q /k prompt keepane$g";
#[cfg(unix)]
const PROMPT_SHELL: &str = "/bin/sh";
/// The name a window running `PROMPT_SHELL` gets.
#[cfg(windows)]
const SH: &str = "cmd";
#[cfg(unix)]
const SH: &str = "sh";

/// A shell like `PROMPT_SHELL` with the prompt `<p>>`.
#[cfg(windows)]
fn shell(p: &str) -> Vec<String> {
    ["cmd.exe", "/q", "/k", &format!("prompt {p}$g")].map(String::from).to_vec()
}
#[cfg(unix)]
fn shell(p: &str) -> Vec<String> {
    ["/bin/sh", "-c", &format!("PS1='{p}>' exec /bin/sh")].map(String::from).to_vec()
}

/// `shell(p)` as one command line, for `display-popup -E`.
#[cfg(windows)]
fn shell_line(p: &str) -> String {
    format!("cmd.exe /q /k \"prompt {p}$g\"")
}
#[cfg(unix)]
fn shell_line(p: &str) -> String {
    // `'p''ip>'` is `pip>` to sh, and the typed line never shows `pip>`
    // itself, so seeing it means the popup's shell is up.
    let (first, rest) = p.split_at(1);
    format!("/bin/sh -c \"PS1='{first}''{rest}>' exec /bin/sh\"")
}

/// A program that exits at once with `code`.
#[cfg(windows)]
fn exits(code: u32) -> Vec<String> {
    ["cmd.exe", "/c", "exit", &code.to_string()].map(String::from).to_vec()
}
#[cfg(unix)]
fn exits(code: u32) -> Vec<String> {
    ["/bin/sh", "-c", &format!("exit {code}")].map(String::from).to_vec()
}

/// A program that exits with `code` after about a second. On Windows it is
/// cmd.exe waiting on ping's one-second interval: Windows PowerShell can take
/// longer than that just to start on a busy runner.
#[cfg(windows)]
fn exits_after_a_second(code: u32) -> Vec<String> {
    ["cmd.exe", "/c", "ping", "-n", "2", "127.0.0.1", ">nul", "&", "exit", &code.to_string()].map(String::from).to_vec()
}
#[cfg(unix)]
fn exits_after_a_second(code: u32) -> Vec<String> {
    ["/bin/sh", "-c", &format!("sleep 1; exit {code}")].map(String::from).to_vec()
}

/// A program that prints `text` and exits.
#[cfg(windows)]
fn says(text: &str) -> String {
    format!("cmd.exe /c echo {text}")
}
#[cfg(unix)]
fn says(text: &str) -> String {
    format!("/bin/sh -c 'echo {text}'")
}

/// A command line for `run-shell` and `#()` that prints `text` (`text` may
/// hold the shell's own variables).
#[cfg(windows)]
fn prints(text: &str) -> String {
    format!("pwsh -NoProfile -Command Write-Output {text}")
}
#[cfg(unix)]
fn prints(text: &str) -> String {
    format!("echo {text}")
}

/// A command line that prints `<word>1` to `<word><n>`, a line each.
#[cfg(windows)]
fn count_to(n: u32, word: &str) -> String {
    format!("for /l %i in (1,1,{n}) do @echo {word}%i")
}
#[cfg(unix)]
fn count_to(n: u32, word: &str) -> String {
    format!("for i in $(seq 1 {n}); do echo {word}$i; done")
}

/// A command line that does nothing and shows `text`.
fn remark(text: &str) -> String {
    if cfg!(windows) { format!("rem {text}") } else { format!(": {text}") }
}

/// A command line that makes the prompt `<word>>` with `<word>` in red.
#[cfg(windows)]
fn red_prompt(word: &str) -> String {
    format!("prompt $e[31m{word}$e[0m$g")
}
#[cfg(unix)]
fn red_prompt(word: &str) -> String {
    format!("PS1=\"$(printf '\\033[31m{word}\\033[0m>')\"")
}

/// The environment variable `name` as `PROMPT_SHELL` expands it.
#[cfg(windows)]
fn var(name: &str) -> String {
    format!("%{name}%")
}
#[cfg(unix)]
fn var(name: &str) -> String {
    format!("${name}")
}

/// `pre` followed by `tail`, as a command's argv.
fn args<'a>(pre: &[&'a str], tail: &'a [String]) -> Vec<&'a str> {
    pre.iter().copied().chain(tail.iter().map(String::as_str)).collect()
}

struct Harness {
    socket: String,
    _server: tokio::task::JoinHandle<()>,
    sessions_dir: std::path::PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.sessions_dir);
    }
}

/// The history log of every server in this process goes to a directory of
/// the test run's own, never the real one (`KEEPANE_HISTORY_DIR` is read when
/// a pane first logs, long after this). One directory for every run, not
/// one per run: the servers' own clearing out of old days keeps it small.
fn keep_history_out() {
    let dir = std::env::temp_dir().join("keepane-test-history");
    unsafe { std::env::set_var("KEEPANE_HISTORY_DIR", dir) };
}

impl Harness {
    async fn start(name: &str) -> Harness {
        // No toasts from tests: a toast registers the running binary as the
        // keepane:// handler on the developer's machine.
        unsafe { std::env::set_var("KEEPANE_NO_TOAST", "1") };
        // Nor asks of GitHub whether a newer keepane is out.
        unsafe { std::env::set_var("KEEPANE_NO_UPDATE_CHECK", "1") };
        // The replay helper is keepane.exe; this test binary is not it.
        unsafe { std::env::set_var("KEEPANE_EXE", env!("CARGO_BIN_EXE_keepane")) };
        keep_history_out();
        // `PROMPT_SHELL`'s prompt, and none of the user's start-up file.
        #[cfg(unix)]
        unsafe {
            std::env::set_var("PS1", "keepane>");
            std::env::remove_var("ENV");
        }
        let socket = format!("test-{name}-{}", std::process::id());
        let s = socket.clone();
        // Process ids come round again, and with them the names below: what
        // an earlier run left under them (saved sessions, link keys and
        // permissions) must not leak into this one.
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("keepane-test-sessions-{}-{name}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(keepane::link::dir(&socket));
        // An empty config, not the machine's `~/.keepane.conf`: a theme there
        // changes the status line these tests read.
        let config = std::env::temp_dir().join(format!("keepane-test-empty-{}.conf", std::process::id()));
        if !config.exists() {
            std::fs::write(&config, "").unwrap();
        }
        let options = keepane::server::RunOptions { force_restore: false, config: Some(config) };
        let server = tokio::spawn(async move {
            if let Err(e) = keepane::server::run_with(s, options).await {
                panic!("server: {e:#}");
            }
        });
        let pipe = pipe_name(&socket);
        let deadline = Instant::now() + Duration::from_secs(5);
        while open_pipe(&pipe).is_err() {
            assert!(Instant::now() < deadline, "server did not come up");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Autosave must never touch the real sessions directory from a test.
        let dir = std::env::temp_dir().join(format!("keepane-test-sessions-{}-{name}", std::process::id()));
        let h = Harness { socket, _server: server, sessions_dir: dir.clone() };
        // Make every implicitly spawned pane a predictable `keepane>` prompt.
        let (code, _, err) = h.cli(&["set", "-g", "default-command", PROMPT_SHELL]).await;
        assert_eq!(code, 0, "{err}");
        let (code, _, err) = h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
        assert_eq!(code, 0, "{err}");
        // tmux's plain status line, which these tests read as text: the
        // default look (Tokyo Night) is tests/console.rs's to check.
        let plain = concat!(env!("CARGO_MANIFEST_DIR"), "/themes/plain.conf");
        let (code, _, err) = h.cli(&["source-file", plain]).await;
        assert_eq!(code, 0, "{err}");
        h
    }

    async fn connect(&self) -> Conn {
        let pipe = pipe_name(&self.socket);
        let deadline = Instant::now() + Duration::from_secs(10);
        let c = loop {
            match open_pipe(&pipe) {
                Ok(c) => break c,
                Err(e) => {
                    assert!(Instant::now() < deadline, "server {} not reachable: {e}", self.socket);
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
            }
        };
        let (rd, wr) = tokio::io::split(c);
        Conn { rd, wr, screen: vt100::Parser::new(ROWS, COLS, 0) }
    }

    /// Poll `capture-pane` until the pane's text satisfies `pred`.
    async fn wait_capture(&self, target: &str, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let (_, out, _) = self.cli(&["capture-pane", "-p", "-t", target]).await;
            if pred(&out) {
                return out;
            }
            assert!(Instant::now() < deadline, "timeout waiting for {what} in {target}; pane:\n{out}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Poll `show-buffer` until the newest buffer satisfies `pred` (keys
    /// that copy and the query travel different connections).
    async fn wait_buffer(&self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, out, _) = self.cli(&["show-buffer"]).await;
            if pred(&out) {
                return out;
            }
            assert!(Instant::now() < deadline, "timeout waiting for {what}; buffer: {out:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Poll `list-panes -t target` until its text satisfies `pred`.
    async fn wait_list(&self, target: &str, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, out, _) = self.cli(&["list-panes", "-t", target]).await;
            if pred(&out) {
                return out;
            }
            assert!(Instant::now() < deadline, "timeout waiting for {what} in {target}:\n{out}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    /// Run a command until its (code, stdout) satisfy `ok` (10 s at most).
    async fn wait_for_cli(&self, what: &str, argv: &[&str], ok: impl Fn(i32, &str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (code, out, _) = self.cli(argv).await;
            if ok(code, &out) {
                return;
            }
            assert!(Instant::now() < deadline, "timeout waiting for {what}: {code} {out}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Run a CLI-style command; returns (code, stdout text, stderr text).
    async fn cli(&self, argv: &[&str]) -> (i32, String, String) {
        self.cli_in(None, argv).await
    }

    /// `cli`, as run by a program inside pane `pane` (`KEEPANE_PANE`).
    async fn cli_in(&self, pane: Option<u32>, argv: &[&str]) -> (i32, String, String) {
        let mut c = self.connect().await;
        c.command_from(argv, false, pane).await;
        let mut out = String::new();
        let mut err = String::new();
        loop {
            match c.next().await {
                ServerMsg::Text(t) => out.push_str(&t),
                ServerMsg::Error(e) => err.push_str(&e),
                ServerMsg::Done { code } => return (code, out, err),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

struct Conn {
    rd: ReadHalf<NamedPipeClient>,
    wr: WriteHalf<NamedPipeClient>,
    screen: vt100::Parser,
}

impl Conn {
    async fn send(&mut self, m: ClientMsg) {
        write_frame(&mut self.wr, &m).await.unwrap();
    }

    async fn command(&mut self, argv: &[&str], interactive: bool) {
        self.command_from(argv, interactive, None).await;
    }

    /// A command as a program in pane `pane_env` runs it (`KEEPANE_PANE`).
    async fn command_from(&mut self, argv: &[&str], interactive: bool, pane_env: Option<u32>) {
        self.send(ClientMsg::Command {
            version: PROTOCOL_VERSION,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            cols: COLS,
            rows: ROWS,
            interactive,
            pane_env,
        })
        .await;
    }

    /// The next message, however long the command it answers may wait
    /// (`-w 30` is the longest any test asks for).
    async fn next(&mut self) -> ServerMsg {
        tokio::time::timeout(Duration::from_secs(40), read_frame::<_, ServerMsg>(&mut self.rd))
            .await
            .expect("timeout waiting for server")
            .unwrap()
            .expect("server closed")
    }

    /// Attach with a command and pump until `Attached`, then the `SetMouse`
    /// that every attach must be followed by; returns (session, mouse).
    async fn attach_full(&mut self, argv: &[&str]) -> (String, bool) {
        self.command(argv, true).await;
        let session = loop {
            match self.next().await {
                ServerMsg::Attached { session } => break session,
                ServerMsg::Error(e) => panic!("attach failed: {e}"),
                ServerMsg::Output(b) => self.screen.process(&b),
                _ => {}
            }
        };
        let mouse = loop {
            match self.next().await {
                ServerMsg::SetMouse(m) => break m,
                ServerMsg::Output(b) => self.screen.process(&b),
                other => panic!("expected SetMouse after Attached, got {other:?}"),
            }
        };
        (session, mouse)
    }

    async fn attach(&mut self, argv: &[&str]) -> String {
        self.attach_full(argv).await.0
    }

    /// Pump until a `SetMouse` arrives; returns its value.
    async fn wait_set_mouse(&mut self) -> bool {
        loop {
            match self.next().await {
                ServerMsg::SetMouse(m) => return m,
                ServerMsg::Output(b) => self.screen.process(&b),
                _ => {}
            }
        }
    }

    /// Pump output until the rendered screen satisfies `pred`.
    async fn wait_for(&mut self, what: &str, pred: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !pred(self.screen.screen()) {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for {what}; screen:\n{}",
                self.screen.screen().contents()
            );
            match tokio::time::timeout(Duration::from_millis(500), read_frame::<_, ServerMsg>(&mut self.rd)).await {
                Ok(Ok(Some(ServerMsg::Output(b)))) => self.screen.process(&b),
                Ok(Ok(Some(ServerMsg::Detached { reason }))) => panic!("detached: {reason}"),
                Ok(Ok(Some(_))) => {}
                Ok(Ok(None)) => panic!("server closed"),
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => {}
            }
        }
    }

    async fn wait_detached(&mut self) -> String {
        loop {
            match self.next().await {
                ServerMsg::Detached { reason } => return reason,
                ServerMsg::Output(b) => self.screen.process(&b),
                _ => {}
            }
        }
    }

    async fn key(&mut self, vk: u16, ch: char, ctrl: u32) {
        let rec = KeyRecord { down: true, repeat: 1, vk, sc: 0, ch: ch as u16, ctrl };
        self.send(ClientMsg::Key(rec)).await;
        self.send(ClientMsg::Key(KeyRecord { down: false, ..rec })).await;
    }

    async fn type_str(&mut self, s: &str) {
        for c in s.chars() {
            let vk = if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as u16
            } else if c == ' ' {
                0x20
            } else {
                0
            };
            self.key(vk, c, 0).await;
        }
    }

    async fn enter(&mut self) {
        self.key(VK_RETURN, '\r', 0).await;
    }

    /// Prefix (C-b) followed by a key.
    async fn prefix(&mut self, ch: char) {
        self.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
        let (vk, ctrl) = match ch {
            '\t' => (0x09, 0),
            c if c.is_ascii_uppercase() => (c as u16, SHIFT_PRESSED),
            c if c.is_ascii_alphabetic() => (c.to_ascii_uppercase() as u16, 0),
            _ => (0, 0),
        };
        self.key(vk, ch, ctrl).await;
    }

    fn text(&self) -> String {
        self.screen.screen().contents()
    }

    fn row(&self, y: u16) -> String {
        self.screen.screen().rows(0, COLS).nth(y as usize).unwrap_or_default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_lifecycle() {
    let h = Harness::start("cli").await;
    let (code, _, err) = h.cli(&["ls"]).await;
    assert_eq!(code, 1);
    assert_eq!(err, "no sessions");

    let (code, _, err) = h.cli(&args(&["new", "-d", "-s", "main"], &shell(""))).await;
    assert_eq!(code, 0, "{err}");
    let (code, out, _) = h.cli(&["ls"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("main: 1 windows"), "{out}");

    let (code, _, err) = h.cli(&["new", "-d", "-s", "main"]).await;
    assert_eq!(code, 1);
    assert_eq!(err, "duplicate session: main");

    let (code, _, _) = h.cli(&["has-session", "-t", "main"]).await;
    assert_eq!(code, 0);
    let (code, _, _) = h.cli(&["has-session", "-t", "nope"]).await;
    assert_eq!(code, 1);

    // A program that cannot be started is an error, not a dead session.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "bad", "definitely-not-a-program-xyz.exe"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("definitely-not-a-program-xyz.exe"), "{err}");
    let (code, _, _) = h.cli(&["has-session", "-t", "bad"]).await;
    assert_eq!(code, 1);

    // Non-ASCII session and window names survive the round trip.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "会话", "-n", "窗口"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["list-windows", "-t", "会话"]).await;
    assert!(out.starts_with("0: 窗口*"), "{out}");
    let (code, _, _) = h.cli(&["kill-session", "-t", "会话"]).await;
    assert_eq!(code, 0);

    let (code, _, err) = h.cli(&args(&["new-window", "-t", "main", "-n", "second"], &exits(0))).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = h.cli(&["rename-session", "-t", "main", "renamed"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.starts_with("renamed:"), "{out}");

    let (code, _, _) = h.cli(&["kill-session", "-t", "renamed"]).await;
    assert_eq!(code, 0);
    // The server exits when its last session dies.
    let deadline = Instant::now() + Duration::from_secs(5);
    while open_pipe(&pipe_name(&h.socket)).is_ok() {
        assert!(Instant::now() < deadline, "server should have exited");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_type_split_detach() {
    let h = Harness::start("attach").await;
    let mut c = h.connect().await;
    let session = c.attach(&["new", "-s", "w"]).await;
    assert_eq!(session, "w");

    // Status line at the bottom names the session and window.
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let status = c.row(ROWS - 1);
    assert!(status.starts_with(&format!("[w] 0:{SH}*")), "status: {status:?}");

    // Typing reaches the shell via win32-input-mode.
    c.type_str("echo hello-from-keepane").await;
    c.enter().await;
    c.wait_for("echo output", |s| s.contents().matches("hello-from-keepane").count() >= 2).await;

    // Split: a vertical border appears and both halves get a prompt.
    c.prefix('%').await;
    c.wait_for("split border", |s| (0..ROWS - 1).all(|y| s.cell(y, COLS / 2).is_some_and(|c| c.contents() == "│")))
        .await;
    c.wait_for("second prompt", |s| s.contents().matches("keepane>").count() >= 2).await;

    // New window: status shows two windows, second is current.
    c.prefix('c').await;
    c.wait_for("second window", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("1:{SH}*")))
        .await;
    // Back to window 0 (which is still split).
    c.prefix('p').await;
    c.wait_for("window 0 current", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*")))
        .await;
    c.wait_for("split border again", |s| s.cell(0, COLS / 2).is_some_and(|c| c.contents() == "│")).await;

    // Zoom hides the border.
    c.prefix('z').await;
    c.wait_for("zoomed", |s| {
        !s.cell(0, COLS / 2).is_some_and(|c| c.contents() == "│")
            && s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*Z"))
    })
    .await;
    c.prefix('z').await;
    c.wait_for("unzoomed", |s| s.cell(0, COLS / 2).is_some_and(|c| c.contents() == "│")).await;

    // Prefix ? shows the key table as an overlay; any key dismisses it.
    c.prefix('?').await;
    c.wait_for("overlay", |s| s.contents().contains("bind-key -T prefix") && s.contents().contains("press any key"))
        .await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("overlay gone", |s| !s.contents().contains("press any key")).await;

    // split-window -d keeps the current pane; -b puts the new pane first.
    c.prefix(':').await;
    c.type_str("split-window -v -d -b").await;
    c.enter().await;
    // (Counting prompts on the screen would not do: the left pane already
    // shows two.)
    let out = h.wait_list("w", "three panes", |out| out.lines().count() == 3).await;
    // The new pane is index 0 (before) and the previously active pane stays active.
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert!(!lines[0].contains("(active)"), "{out}");

    // Prefix , opens a rename prompt pre-filled with the window name; the
    // template is "rename-window -- %%" so `--` must end flag parsing.
    c.prefix(',').await;
    c.wait_for("rename prompt", |s| {
        s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().starts_with(&format!("(rename-window) {SH}"))
    })
    .await;
    for _ in SH.chars() {
        c.key(0x08, '\x08', 0).await; // backspace over the name
    }
    c.type_str("via-comma").await;
    c.enter().await;
    c.wait_for("renamed via ,", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("0:via-comma*")).await;

    // A multi-line error (bad source-file) is shown as an overlay, not flattened.
    // Each message starts with the file's path and long lines are clipped at
    // the window width, so the path must be short on every machine: relative
    // to the package root, which is this process's (and the server's) cwd.
    // (A target directory elsewhere, CARGO_TARGET_DIR, puts it in the
    // package root instead.)
    let name = format!("bad-{}.conf", std::process::id());
    let tmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bad = if tmp.starts_with(root) { tmp.join(&name) } else { root.join(&name) };
    std::fs::write(&bad, "set -g mouse maybe\nfrobnicate\n").unwrap();
    let rel = bad.strip_prefix(root).unwrap();
    c.prefix(':').await;
    c.type_str(&format!("source-file {}", rel.display())).await;
    c.enter().await;
    c.wait_for("config errors overlay", |s| {
        let t = s.contents();
        t.contains(":1: bad boolean 'maybe'") && t.contains(":2: unknown command") && t.contains("press any key")
    })
    .await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("overlay gone again", |s| !s.contents().contains("press any key")).await;
    let _ = std::fs::remove_file(&bad);

    // Command prompt: rename the window.
    c.prefix(':').await;
    c.type_str("rename-window shell").await;
    c.enter().await;
    c.wait_for("renamed", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("0:shell*")).await;

    // Detach.
    c.prefix('d').await;
    let reason = c.wait_detached().await;
    assert_eq!(reason, "detached");

    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.starts_with("w: 2 windows"), "{out}");
    let (_, out, _) = h.cli(&["list-windows", "-t", "w"]).await;
    assert!(out.contains("0: shell* (3 panes)"), "{out}");

    // -A attaches to an existing session instead of failing on the duplicate.
    let (code, _, err) = h.cli(&["new", "-A", "-d", "-s", "w"]).await;
    assert_eq!(code, 0, "{err}");
    // new-window -d does not change the current window; kill-window -a keeps only the target.
    let (code, _, _) = h.cli(&["new-window", "-d", "-t", "w", "-n", "bg"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["list-windows", "-t", "w"]).await;
    assert!(out.contains("0: shell*") && out.contains("2: bg ("), "{out}");
    let (code, _, _) = h.cli(&["kill-window", "-a", "-t", "w:0"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["list-windows", "-t", "w"]).await;
    assert_eq!(out.lines().count(), 1, "{out}");
    // kill-pane -a leaves one pane.
    let (code, _, _) = h.cli(&["kill-pane", "-a", "-t", "w:0.1"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["list-panes", "-t", "w"]).await;
    assert_eq!(out.lines().count(), 1, "{out}");
    // send-keys -l sends the words literally: "Enter" is text, not a key.
    let (code, _, _) = h.cli(&["send-keys", "-t", "w", "-l", "rem literal-Enter-word"]).await;
    assert_eq!(code, 0);
    let (code, _, _) = h.cli(&["send-keys", "-t", "w", "Enter"]).await;
    assert_eq!(code, 0);
    // capture-pane prints the pane text from the CLI.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, out, _) = h.cli(&["capture-pane", "-p", "-t", "w"]).await;
        assert_eq!(code, 0);
        if out.contains("rem literal-Enter-word") && out.ends_with("keepane>") {
            break;
        }
        assert!(Instant::now() < deadline, "capture-pane: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Re-attach: full redraw restores the view; the literal send-keys text is there.
    let mut c2 = h.connect().await;
    c2.attach(&["attach", "-t", "w"]).await;
    c2.wait_for("restored prompt", |s| s.contents().contains("keepane>")).await;
    c2.wait_for("literal text", |s| s.contents().contains("literal-Enter-word")).await;
    // Same-session CLI command from outside while attached shows up as a message.
    let (code, _, _) = h.cli(&["send-keys", "-t", "w", "echo via-send-keys", "Enter"]).await;
    assert_eq!(code, 0);
    c2.wait_for("send-keys echoed", |s| s.contents().matches("via-send-keys").count() >= 2).await;

    let (code, _, _) = h.cli(&["kill-server"]).await;
    assert_eq!(code, 0);
    let reason = c2.wait_detached().await;
    assert_eq!(reason, "server exited");
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_exit_closes_window_and_session() {
    let h = Harness::start("exit").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "x"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix('"').await;
    c.wait_for("horizontal border", |s| {
        (0..COLS).all(|x| {
            s.cell(ROWS / 2 - 1, x).is_some_and(|c| c.contents() == "─")
                || s.cell(ROWS / 2, x).is_some_and(|c| c.contents() == "─")
        })
    })
    .await;
    // Exit the new (active) pane: window collapses back to one pane.
    c.type_str("exit").await;
    c.enter().await;
    c.wait_for("border gone", |s| !s.contents().contains('─')).await;
    // Exit the last pane: session ends and the client is detached.
    c.type_str("exit").await;
    c.enter().await;
    let reason = c.wait_detached().await;
    assert_eq!(reason, "exited");
}

#[tokio::test(flavor = "multi_thread")]
async fn mouse_selects_pane_and_copy_mode_scrolls() {
    let h = Harness::start("mouse").await;
    let mut c = h.connect().await;
    let (_, mouse) = c.attach_full(&["new", "-s", "m"]).await;
    assert!(mouse, "mouse is on by default");
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Toggling the option reaches the attached client's console.
    let (code, _, _) = h.cli(&["set", "-g", "mouse", "off"]).await;
    assert_eq!(code, 0);
    assert!(!c.wait_set_mouse().await);
    let (code, _, _) = h.cli(&["set", "-g", "mouse", "on"]).await;
    assert_eq!(code, 0);
    assert!(c.wait_set_mouse().await);
    c.prefix('%').await;
    c.wait_for("split", |s| s.contents().matches("keepane>").count() >= 2).await;
    // Right pane is active after the split (green border on the right side).
    // Click in the left pane, then type: text must land on the left.
    c.send(ClientMsg::Mouse(MouseRecord { x: 2, y: 2, buttons: 1, ctrl: 0, flags: 0 })).await;
    c.send(ClientMsg::Mouse(MouseRecord { x: 2, y: 2, buttons: 0, ctrl: 0, flags: 0 })).await;
    c.type_str(&remark("left-side-marker")).await;
    c.wait_for("typed on the left", |s| s.rows(0, COLS / 2).any(|r| r.contains("left-side-marker"))).await;
    assert!(
        !c.text().contains("marker")
            || c.screen.screen().rows(COLS / 2 + 1, COLS / 2 - 1).all(|r| !r.contains("left-side-marker"))
    );

    // Fill scrollback, then wheel up: the [n/m] indicator of copy mode shows.
    c.enter().await;
    c.type_str(&count_to(60, "line")).await;
    c.enter().await;
    c.wait_for("output", |s| s.contents().contains("line60")).await;
    c.send(ClientMsg::Mouse(MouseRecord { x: 2, y: 2, buttons: (120u32) << 16, ctrl: 0, flags: 4 })).await;
    c.wait_for("copy mode indicator", |s| s.rows(0, COLS).next().unwrap().contains("[3/")).await;
    // Escape leaves copy mode.
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("copy mode left", |s| !s.rows(0, COLS).next().unwrap().contains("[3/")).await;
    h.cli(&["kill-server"]).await;
}

/// A drag over a shell's text selects it and copies it on release (the
/// paste buffer gets it); a plain click selects nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_drag_in_a_shell_copies_what_it_covers() {
    let h = Harness::start("dragcopy").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "d"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.type_str(&prints("dragme-1234")).await;
    c.enter().await;
    c.wait_for("the text", |s| s.contents().matches("dragme-1234").count() >= 2).await;
    // The row with the text, and where it starts.
    let (row, col) = (0..24u16)
        .find_map(|r| {
            let line: String = c.screen.screen().rows(0, COLS).nth(r as usize).unwrap_or_default();
            line.find("dragme-1234").map(|i| (r, line[..i].chars().count() as u16))
        })
        .expect("the text on screen");
    let m = |x: u16, buttons: u32, flags: u32| MouseRecord { x: x as i16, y: row as i16, buttons, ctrl: 0, flags };
    c.send(ClientMsg::Mouse(m(col, 1, 0))).await;
    c.send(ClientMsg::Mouse(m(col + 5, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(col + 10, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(col + 10, 0, 0))).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, out, _) = h.cli(&["show-buffer"]).await;
        if code == 0 && out.contains("dragme-1234") {
            break;
        }
        assert!(Instant::now() < deadline, "nothing copied: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["kill-server"]).await;
}

/// A drag copies exactly the columns it covers, wide characters (Chinese,
/// two columns each) before, inside and after the selection included.
#[tokio::test(flavor = "multi_thread")]
async fn a_drag_over_wide_characters_copies_what_it_covers() {
    let h = Harness::start("dragwide").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "w"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.type_str(&prints("'中文前面 dragme-宽字-5678 后面'")).await;
    c.enter().await;
    c.wait_for("the text", |s| s.contents().matches("dragme-").count() >= 2).await;
    // The printed row (not the typed one), and the column its word starts at.
    let (row, col) = (0..24u16)
        .rev()
        .find_map(|r| {
            let line: String = c.screen.screen().rows(0, COLS).nth(r as usize).unwrap_or_default();
            let i = line.find("dragme-")?;
            line.starts_with("中文前面").then(|| (r, unicode_width::UnicodeWidthStr::width(&line[..i]) as u16))
        })
        .unwrap_or_else(|| panic!("the text on screen:\n{}", c.screen.screen().contents()));
    // `dragme-宽字-5678` is 17 columns: from its first to its last.
    let m = |x: u16, buttons: u32, flags: u32| MouseRecord { x: x as i16, y: row as i16, buttons, ctrl: 0, flags };
    c.send(ClientMsg::Mouse(m(col, 1, 0))).await;
    c.send(ClientMsg::Mouse(m(col + 8, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(col + 16, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(col + 16, 0, 0))).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, out, _) = h.cli(&["show-buffer"]).await;
        if code == 0 && out.contains("dragme") {
            assert_eq!(out.trim_end(), "dragme-宽字-5678", "exactly what the drag covered");
            break;
        }
        assert!(Instant::now() < deadline, "nothing copied: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["kill-server"]).await;
}

/// `status-line`: a session's status line as text, or (`-J`) its parts,
/// each window with its number, whether it is current and its active pane;
/// `on` says whether `status` is on (the phone page then hides it).
#[tokio::test(flavor = "multi_thread")]
async fn the_status_line_is_there_for_a_script_and_the_page() {
    let h = Harness::start("statusline").await;
    h.cli(&["new", "-d", "-s", "st", "-n", "first"]).await;
    h.cli(&["new-window", "-d", "-t", "st", "-n", "second"]).await;
    let (code, text, err) = h.cli(&["status-line", "-t", "st"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(text.contains("[st]") && text.contains("0:first*") && text.contains("1:second"), "{text}");
    let (_, json, _) = h.cli(&["status-line", "-J", "-t", "st"]).await;
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let second = h.cli(&["display-message", "-p", "-t", "st:1", "#{pane_id}"]).await.1;
    assert_eq!((v["session"].as_str(), v["on"].as_bool()), (Some("st"), Some(true)));
    assert_eq!(v["windows"][0]["current"], true);
    assert_eq!((v["windows"][1]["index"].as_u64(), v["windows"][1]["pane"].as_str()), (Some(1), Some(second.trim())));
    let segs = |part: &serde_json::Value| {
        part.as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap().to_string()).collect::<String>()
    };
    assert!(segs(&v["left"]).contains("[st]") && segs(&v["windows"][1]["segments"]).contains("1:second"), "{json}");
    h.cli(&["set", "-g", "status", "off"]).await;
    let (_, json, _) = h.cli(&["status-line", "-J", "-t", "st"]).await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&json).unwrap()["on"], false);
    let (code, _, err) = h.cli(&["status-line", "-t", "nosuch"]).await;
    assert!(code != 0 && !err.is_empty());
    h.cli(&["kill-server"]).await;
}

/// With a Chinese input method on, `[` arrives as `【`: after the prefix,
/// in copy mode and at a y/n question keepane reads such a key as its ASCII
/// form; a prompt for text keeps it as typed.
#[tokio::test(flavor = "multi_thread")]
async fn full_width_keys_work_where_keepane_reads_keys() {
    let h = Harness::start("fullwidth").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "f"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // C-b 【 is C-b [: copy mode; ｑ is q: out again.
    c.prefix('【').await;
    h.wait_for_cli("copy mode", &["display-message", "-p", "-t", "f", "#{pane_in_mode}"], |_, o| o.trim() == "1").await;
    c.type_str("ｑ").await;
    h.wait_for_cli("out of copy mode", &["display-message", "-p", "-t", "f", "#{pane_in_mode}"], |_, o| {
        o.trim() == "0"
    })
    .await;
    // A name typed at the rename prompt keeps its full-width brackets.
    c.prefix('，').await;
    c.wait_for("the rename prompt", |s| s.contents().contains("(rename-window)")).await;
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("【工作】").await;
    c.enter().await;
    h.wait_for_cli("renamed", &["display-message", "-p", "-t", "f", "#{window_name}"], |_, o| o.trim() == "【工作】")
        .await;
    // A y/n question: ｙ is y (C-b ＆ kills the window; a second one keeps
    // the session).
    h.cli(&["new-window", "-d", "-t", "f"]).await;
    c.prefix('＆').await;
    c.wait_for("the question", |s| s.contents().contains("(y/n)")).await;
    c.type_str("ｙ").await;
    h.wait_for_cli("one window left", &["list-windows", "-t", "f"], |_, o| o.lines().count() == 1).await;
    h.cli(&["kill-server"]).await;
}

/// A selection dragged to the pane's top row scrolls on while the pointer
/// stays there, so it takes lines that were above the screen; let go on
/// the status line (outside the pane), it is still copied.
#[tokio::test(flavor = "multi_thread")]
async fn a_drag_at_the_edge_scrolls_and_is_copied_wherever_let_go() {
    let h = Harness::start("dragscroll").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "s"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.type_str(&count_to(80, "row-")).await;
    c.enter().await;
    c.wait_for("output", |s| s.contents().contains("row-80")).await;
    // The first `row-N` on screen: what is above it is only in the scrollback.
    let first = |s: &vt100::Screen| {
        s.rows(0, COLS).find_map(|r| r.trim().strip_prefix("row-").and_then(|n| n.parse::<u32>().ok()))
    };
    let top = first(c.screen.screen()).expect("rows on screen");
    let row_of =
        |s: &vt100::Screen, n: u32| s.rows(0, COLS).position(|r| r.trim() == format!("row-{n}")).unwrap() as i16;
    let from = row_of(c.screen.screen(), top + 5);
    let m = |y: i16, buttons: u32, flags: u32| MouseRecord { x: 2, y, buttons, ctrl: 0, flags };
    c.send(ClientMsg::Mouse(m(from, 1, 0))).await;
    c.send(ClientMsg::Mouse(m(from - 1, 1, 1))).await;
    // Up to the top row, and held there: it scrolls a line every 80 ms.
    c.send(ClientMsg::Mouse(m(0, 1, 1))).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    c.send(ClientMsg::Mouse(m(0, 0, 0))).await;
    let copied = |want: String| {
        let h = &h;
        async move {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let (code, out, _) = h.cli(&["show-buffer"]).await;
                if code == 0 && out.contains(&want) {
                    return out;
                }
                assert!(Instant::now() < deadline, "{want} not copied: {out:?}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    let out = copied(format!("row-{}", top + 4)).await;
    let earliest = out.lines().find_map(|l| l.trim().strip_prefix("row-")?.parse::<u32>().ok()).unwrap();
    assert!(earliest + 3 <= top, "scrolled past the screen's top (row-{top}): from row-{earliest}\n{out}");
    // Let go on the status line, outside the pane: still copied (the
    // pointer kept to the pane's last row).
    c.key(0x1B, '\x1b', 0).await;
    // Back at the bottom, the whole frame drawn: a frame comes in pieces, and
    // `row-80` alone may show before row 78 has moved to its place.
    c.wait_for("back at the bottom", |s| {
        let rows: Vec<String> = s.rows(0, COLS).map(|r| r.trim().to_string()).collect();
        let at = rows.windows(3).position(|w| w[0] == "row-78" && w[1] == "row-79" && w[2] == "row-80");
        at.is_some_and(|i| rows[i + 3..].iter().any(|r| r.starts_with("keepane>")))
    })
    .await;
    let from = row_of(c.screen.screen(), 78);
    c.send(ClientMsg::Mouse(m(from, 1, 0))).await;
    c.send(ClientMsg::Mouse(m(from + 1, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(ROWS as i16 - 1, 1, 1))).await;
    c.send(ClientMsg::Mouse(m(ROWS as i16 - 1, 0, 0))).await;
    let out = copied("w-79".into()).await;
    assert!(out.starts_with("w-78"), "from where it was pressed: {out:?}");
    h.cli(&["kill-server"]).await;
}

/// Copy mode by keys over wide characters: a search lands on the word, `E`
/// goes to its end and `y` copies exactly it (the cursor counts columns, a
/// Chinese character two).
#[tokio::test(flavor = "multi_thread")]
async fn copy_mode_keys_count_wide_characters_as_two_columns() {
    let h = Harness::start("copywide").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "k"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.type_str(&prints("'中文前面 dragme-宽字-5678 后面'")).await;
    c.enter().await;
    c.wait_for("the text", |s| s.rows(0, COLS).any(|r| r.starts_with("中文前面"))).await;
    c.prefix('[').await;
    c.type_str("?dragme").await;
    c.enter().await;
    c.type_str(" ").await;
    c.type_str("E").await;
    c.type_str("y").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, out, _) = h.cli(&["show-buffer"]).await;
        if code == 0 && out.contains("dragme") {
            assert_eq!(out.trim_end(), "dragme-宽字-5678", "from the search's hit to the word's end");
            break;
        }
        assert!(Instant::now() < deadline, "nothing copied: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // `^` past blanks two columns wide (the ideographic space).
    c.type_str(&prints("'\u{3000}\u{3000}indented-9'")).await;
    c.enter().await;
    c.wait_for("the indented line", |s| s.rows(0, COLS).any(|r| r.starts_with("\u{3000}\u{3000}indented-9"))).await;
    c.prefix('[').await;
    c.type_str("?indented-9").await;
    c.enter().await;
    c.type_str("0^ Ey").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, out, _) = h.cli(&["show-buffer"]).await;
        if code == 0 && out.contains("indented-9") {
            assert_eq!(out.trim_end(), "indented-9", "from the first that is not blank");
            break;
        }
        assert!(Instant::now() < deadline, "nothing copied: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resize_and_two_clients() {
    let h = Harness::start("resize").await;
    let mut a = h.connect().await;
    a.attach(&["new", "-s", "r"]).await;
    a.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Shrink the client: the status line moves up.
    a.send(ClientMsg::Resize { cols: 60, rows: 12 }).await;
    a.screen = vt100::Parser::new(12, 60, 0);
    a.wait_for("status at row 11", |s| s.rows(0, 60).nth(11).unwrap().starts_with(&format!("[r] 0:{SH}*"))).await;
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.contains("[60x12]"), "{out}");

    // Second client attaches: it sees the same session; -d kicks the first.
    let mut b = h.connect().await;
    b.attach(&["attach", "-d", "-t", "r"]).await;
    let reason = a.wait_detached().await;
    assert_eq!(reason, "detached (attach -d)");
    b.wait_for("prompt on b", |s| s.contents().contains("keepane>")).await;
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.contains("[80x24]") && out.contains("(attached)"), "{out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_hooks_status_formats_and_run_shell() {
    let h = Harness::start("plugin").await;
    // A plugin directory: <plugin-path>/demo/demo.keepane
    let root = std::env::temp_dir().join(format!("keepane-plugins-{}", std::process::id()));
    let dir = root.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("demo.keepane"),
        format!(
            "set -g status-right \"#[fg=red]#({})#[default] %H\"\n\
             set -g status-interval 1\n\
             bind P run-shell \"{}\"\n\
             set-hook -g after-new-window \"rename-window hooked\"\n\
             set -g @demo-option yes\n",
            prints("plugged"),
            prints("hello-from-plugin")
        ),
    )
    .unwrap();
    // Declared the tmux way, from a config file.
    let conf = root.join("keepane.conf");
    std::fs::write(&conf, format!("set -g plugin-path \"{}\"\nset -g @plugin demo\n", root.display())).unwrap();
    let (code, _, err) = h.cli(&["source-file", &conf.to_string_lossy()]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["list-plugins"]).await;
    assert!(out.contains("demo"), "{out}");
    let (_, out, _) = h.cli(&["show-hooks"]).await;
    assert!(out.contains("after-new-window \"rename-window hooked\""), "{out}");
    // show-hooks output is valid command syntax: feeding it back reproduces the hook.
    let line = out.lines().find(|l| l.starts_with("after-new-window")).unwrap();
    let words = keepane::command::tokenize(line).unwrap();
    assert_eq!(words, vec!["after-new-window", "rename-window hooked"]);
    let (code, _, err) = h.cli(&["load-plugin", "nope"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("plugin not found"), "{err}");
    // Loading the same plugin twice is a no-op, so bindings are not duplicated.
    let (code, _, _) = h.cli(&["load-plugin", "demo"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["list-plugins"]).await;
    assert_eq!(out.lines().count(), 1, "{out}");
    // Plugins read their options the tmux way.
    let (code, out, _) = h.cli(&["show-options", "-gqv", "@demo-option"]).await;
    assert_eq!((code, out.as_str()), (0, "yes"));
    let (code, out, _) = h.cli(&["show-options", "-gqv", "@absent"]).await;
    assert_eq!((code, out.as_str()), (0, ""));
    let (code, _, err) = h.cli(&["show-options", "-g", "@absent"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("unknown option"), "{err}");
    let (_, out, _) = h.cli(&["show-options", "-g"]).await;
    assert!(out.contains("status-interval 1") && out.contains("@demo-option yes"), "{out}");
    let (code, _, err) = h.cli(&["set-hook", "-g", "no-such-hook", "list-keys"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("unknown hook"), "{err}");

    // run-shell from the CLI: output comes back when the command finishes; a
    // non-zero exit is an error.
    let (code, out, _) = h.cli(&["run-shell", &prints("from-cli")]).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "from-cli");
    let exit3 = if cfg!(windows) { "pwsh -NoProfile -Command exit 3" } else { "exit 3" };
    let (code, _, err) = h.cli(&["run-shell", exit3]).await;
    assert_eq!(code, 1);
    assert!(err.contains("exited with 3"), "{err}");
    // KEEPANE is set for the child, so plugin scripts can call back.
    let (_, out, _) = h.cli(&["run-shell", &prints(if cfg!(windows) { "$env:KEEPANE" } else { "$KEEPANE" })]).await;
    assert_eq!(out.trim(), h.socket);

    // Attached: the #(command) piece shows up on the status line (styled),
    // prefix P runs the plugin's command and shows its output, the hook renames
    // new windows.
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "p"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.wait_for("status #() piece", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("plugged")).await;
    let sr = c.screen.screen();
    let row = ROWS - 1;
    let col = (0..COLS).find(|&x| sr.rows(x, COLS - x).nth(row as usize).unwrap().starts_with("plugged")).unwrap();
    assert_eq!(sr.cell(row, col).unwrap().fgcolor(), vt100::Color::Idx(1), "#[fg=red] applied");
    c.prefix('P').await;
    c.wait_for("run-shell output", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("hello-from-plugin"))
        .await;
    c.prefix('c').await;
    c.wait_for("hook renamed the window", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("1:hooked*"))
        .await;
    let (_, out, _) = h.cli(&["list-windows", "-t", "p"]).await;
    assert!(out.contains("1: hooked*"), "{out}");
    h.cli(&["kill-server"]).await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A bare `keepane` (`new -A` with no name) goes into what is there rather
/// than adding one more session each time: a new one only when there is
/// nothing; the session used last when some run; the saved ones back (into
/// the one saved last) when none run. prefix C-s saves every session, each
/// into its own file. Starting again and again does not pile sessions up.
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_keepane_goes_into_what_is_there() {
    let dir = std::env::temp_dir().join(format!("keepane-bare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = || std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    let names = async |h: &Harness| {
        let mut v: Vec<String> =
            h.cli(&["ls"]).await.1.lines().map(|l| l.split(':').next().unwrap().to_string()).collect();
        v.sort();
        v
    };
    let h = Harness::start("bare").await;
    h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
    // Nothing at all: a new session.
    let mut c = h.connect().await;
    assert_eq!(c.attach(&["new-session", "-A"]).await, "0");
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Sessions running: into the one used last, none added.
    h.cli(&["new", "-d", "-s", "work"]).await;
    h.cli(&["new", "-d", "-s", "notes"]).await;
    let mut c2 = h.connect().await;
    assert_eq!(c2.attach(&["new-session", "-A"]).await, "notes", "used last (made last), as `attach` picks");
    assert_eq!(names(&h).await, ["0", "notes", "work"]);
    // prefix C-s: every session, each into its own file, said in one line.
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.key(b'S' as u16, '\x13', LEFT_CTRL_PRESSED).await;
    c.wait_for("saved, all three", |s| s.contents().contains("saved 3 sessions: ")).await;
    assert_eq!(files(), 3);
    // notes saved last.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.cli(&["save-session", "-t", "notes"]).await;
    h.cli(&["kill-server"]).await;

    // A new server: nothing runs, three are saved: all back, into notes.
    for round in 0..2 {
        let h = Harness::start(&format!("bare{round}")).await;
        h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
        let mut c = h.connect().await;
        let into = c.attach(&["new-session", "-A"]).await;
        assert_eq!(names(&h).await, ["0", "notes", "work"], "round {round}: none added");
        if round == 0 {
            assert_eq!(into, "notes", "the one saved last");
        }
        // And a second start meanwhile: into the one in use, still three.
        let mut c2 = h.connect().await;
        c2.attach(&["new-session", "-A"]).await;
        assert_eq!(names(&h).await, ["0", "notes", "work"]);
        assert_eq!(files(), 3, "round {round}: no more saved sessions");
        h.cli(&["save-session", "-a"]).await;
        h.cli(&["kill-server"]).await;
    }
    // `new` still makes one when asked.
    let h = Harness::start("bare-new").await;
    h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
    let mut c = h.connect().await;
    assert_eq!(c.attach(&["new-session", "-s", "fresh"]).await, "fresh");
    h.cli(&["kill-server"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn save_and_resume_sessions() {
    let h = Harness::start("resume").await;
    let dir = std::env::temp_dir().join(format!("keepane-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (code, _, err) = h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
    assert_eq!(code, 0, "{err}");

    // A second session keeps the server alive while "work" is killed below.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "keeper"]).await;
    assert_eq!(code, 0, "{err}");
    // Build a session: two windows, the first split in three, second window renamed.
    let (code, _, err) = h.cli(&args(&["new", "-d", "-s", "work", "-n", "edit"], &shell("A"))).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&args(&["split-window", "-h", "-t", "work:0"], &shell("B"))).await;
    h.cli(&args(&["split-window", "-v", "-t", "work:0"], &shell("C"))).await;
    h.cli(&args(&["new-window", "-t", "work", "-n", "logs"], &shell("D"))).await;
    h.cli(&["select-window", "-t", "work:0"]).await;
    // Autosave happens on the tick; the explicit command is immediate.
    let (code, out, err) = h.cli(&["save-session", "-t", "work"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("saved "), "{out}");
    let (_, out, _) = h.cli(&["list-saved"]).await;
    assert!(out.contains("work: saved") && out.contains("(running)"), "{out}");

    // Kill it: the file survives; resuming by name brings it back with the
    // same shape and commands.
    let (code, _, _) = h.cli(&["kill-session", "-t", "work"]).await;
    assert_eq!(code, 0);
    let (code, _, err) = h.cli(&["has-session", "-t", "work"]).await;
    assert_eq!(code, 1, "{err}");
    let (_, out, _) = h.cli(&["list-saved"]).await;
    let work_line = out.lines().find(|l| l.starts_with("work:")).unwrap_or_default();
    assert!(work_line.contains("saved") && !work_line.contains("(running)"), "{out}");
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(!out.contains("work") && out.contains("keeper"), "{out}");

    let mut c = h.connect().await;
    let session = c.attach(&["resume", "work"]).await;
    assert_eq!(session, "work");
    let (_, out, _) = h.cli(&["list-windows", "-t", "work"]).await;
    assert!(out.contains("0: edit* (3 panes)") && out.contains("1: logs"), "{out}");
    let (_, out, _) = h.cli(&["list-panes", "-t", "work:0"]).await;
    assert_eq!(out.lines().count(), 3, "{out}");
    // Each pane came back with its own command line: three different prompts.
    c.wait_for("restored prompts", |s| ["A>", "B>", "C>"].iter().all(|p| s.contents().contains(p))).await;
    // The layout (left | (top / bottom)) is the same: a vertical border and a
    // horizontal one in the right half.
    assert!(c.screen.screen().cell(0, COLS / 2).is_some_and(|x| x.contents() == "│"), "{}", c.text());
    assert!(
        (COLS / 2 + 1..COLS).any(|x| c.screen.screen().cell((ROWS - 1) / 2, x).is_some_and(|c| c.contents() == "─")),
        "{}",
        c.text()
    );

    // Resuming a running session is just an attach for a second client.
    let mut c2 = h.connect().await;
    assert_eq!(c2.attach(&["resume", "work"]).await, "work");
    c2.wait_for("attached", |s| s.contents().contains("A>")).await;

    // `resume` with no name restores everything that is not running.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "other"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["save-session", "-t", "other"]).await;
    h.cli(&["kill-session", "-t", "other"]).await;
    let (code, out, _) = h.cli(&["restore-session"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("restored 1 session"), "{out}");
    let (code, _, _) = h.cli(&["has-session", "-t", "other"]).await;
    assert_eq!(code, 0);
    // Unknown names and deletion.
    let (code, _, err) = h.cli(&["resume", "nope"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no saved session"), "{err}");
    // A running session cannot be forgotten (autosave would bring it back).
    let (code, _, err) = h.cli(&["delete-saved", "other"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("is running"), "{err}");
    h.cli(&["kill-session", "-t", "other"]).await;
    let (code, _, _) = h.cli(&["delete-saved", "other"]).await;
    assert_eq!(code, 0);
    let (_, out, _) = h.cli(&["list-saved"]).await;
    assert!(!out.contains("other:"), "{out}");
    // Renaming a session moves its saved file: no ghost under the old name.
    h.cli(&["rename-session", "-t", "work", "work2"]).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, out, _) = h.cli(&["list-saved"]).await;
        if out.contains("work2:") && !out.lines().any(|l| l.starts_with("work:")) {
            break;
        }
        assert!(Instant::now() < deadline, "{out}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    h.cli(&["rename-session", "-t", "work2", "work"]).await;

    // set-cwd records the directory a pane will be resumed in: explicit, or
    // the calling client's own (the harness sends temp_dir as its cwd).
    let (real, gone, announced) = if cfg!(windows) {
        ("C:\\Windows", "C:\\definitely\\not\\here", "C:\\Users")
    } else {
        ("/usr", "/definitely/not/here", "/var")
    };
    let (code, _, err) = h.cli(&["set-cwd", "-t", "work:0.1", real]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = h.cli(&["set-cwd", "-t", "work:0.2"]).await;
    assert_eq!(code, 0);
    let (code, _, err) = h.cli(&["set-cwd", "-t", "work:0.0", gone]).await;
    assert_eq!(code, 1);
    assert!(err.contains("not a directory"), "{err}");
    // Relative directories resolve against the client's cwd (temp_dir here).
    let sub = std::env::temp_dir().join(format!("keepane-rel-{}", std::process::id()));
    std::fs::create_dir_all(&sub).unwrap();
    let rel = sub.file_name().unwrap().to_string_lossy().into_owned();
    let (code, _, err) = h.cli(&["set-cwd", "-t", "work:0.0", &rel]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["list-panes", "-t", "work:0"]).await;
    assert!(out.lines().next().is_some_and(|l| l.contains(&rel)), "{out}");
    let _ = std::fs::remove_dir_all(&sub);
    let (_, out, _) = h.cli(&["list-panes", "-t", "work:0"]).await;
    let tmp = std::env::temp_dir().to_string_lossy().trim_end_matches(['\\', '/']).to_string();
    assert!(out.contains(&format!("[{real}]")) && out.contains(&format!("[{tmp}")), "{out}");
    // The shell itself can announce its directory (OSC 9;9, OSC 7), as a
    // prompt function would; it travels through the pty like a title does.
    let announce = if cfg!(windows) {
        "pwsh -NoProfile -Command \"Write-Host ([char]27+']9;9;C:\\Users'+[char]7)\"".to_string()
    } else {
        format!("printf '\\033]7;file://here{announced}\\007'")
    };
    h.cli(&["send-keys", "-t", "work:0.0", &announce, "Enter"]).await;
    // pwsh has to start first, which on a busy runner takes seconds.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, out, _) = h.cli(&["list-panes", "-t", "work:0"]).await;
        if out.lines().next().is_some_and(|l| l.contains(&format!("[{announced}]"))) {
            break;
        }
        assert!(Instant::now() < deadline, "OSC cwd not picked up: {out}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // ...and all of that lands in the saved file.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let f = keepane::resurrect::SavedFile::load(&keepane::resurrect::find(&dir, "work").unwrap()).unwrap();
        let cwds: Vec<Option<String>> = f.session.windows[0].layout.panes().iter().map(|p| p.cwd.clone()).collect();
        if cwds[0].as_deref() == Some(announced) && cwds[1].as_deref() == Some(real) {
            break;
        }
        assert!(Instant::now() < deadline, "saved cwds: {cwds:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Autosave: a structural change is on disk within a couple of ticks.
    h.cli(&["rename-window", "-t", "work:1", "renamed-by-autosave"]).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let path = keepane::resurrect::find(&dir, "work").unwrap();
        let f = keepane::resurrect::SavedFile::load(&path).unwrap();
        if f.session.windows.iter().any(|w| w.name == "renamed-by-autosave") {
            break;
        }
        assert!(Instant::now() < deadline, "autosave did not pick up the rename");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    h.cli(&["kill-server"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn vim_keys_and_synchronize_panes() {
    let h = Harness::start("vim").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "v"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix('%').await; // left | right, right active
    c.wait_for("split", |s| s.contents().matches("keepane>").count() >= 2).await;
    // prefix h -> left pane active, prefix l -> right again. (The keys and
    // list-panes come over different connections: wait for the change.)
    c.prefix('h').await;
    h.wait_list("v", "left active", |out| out.lines().next().is_some_and(|l| l.contains("(active)"))).await;
    c.prefix('l').await;
    h.wait_list("v", "right active", |out| out.lines().nth(1).is_some_and(|l| l.contains("(active)"))).await;
    // prefix H shrinks the right pane's left edge... i.e. resizes; widths change.
    c.prefix('H').await;
    // 80 columns: 35 + border + 44.
    h.wait_list("v", "resized", |out| out.contains("[44x") && out.contains("[35x")).await;
    // prefix Tab is last-window now that l is taken.
    c.prefix('c').await;
    c.wait_for("window 1", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("1:{SH}*"))).await;
    c.prefix('\t').await;
    c.wait_for("back to 0", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*"))).await;

    // synchronize-panes: typing lands in both panes; the S flag shows.
    c.prefix('S').await;
    c.wait_for("S flag", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*S"))).await;
    c.type_str("echo both-panes").await;
    c.enter().await;
    c.wait_for("echoed twice", |s| s.contents().matches("both-panes").count() >= 4).await;
    let (_, a, _) = h.cli(&["capture-pane", "-p", "-t", "v:0.0"]).await;
    let (_, b, _) = h.cli(&["capture-pane", "-p", "-t", "v:0.1"]).await;
    assert!(a.contains("both-panes") && b.contains("both-panes"), "{a}\n---\n{b}");
    // send-keys follows the flag too; then off again.
    h.cli(&["send-keys", "-t", "v:0.0", "echo via-send", "Enter"]).await;
    c.wait_for("send-keys to both", |s| s.contents().matches("via-send").count() >= 4).await;
    let (code, _, _) = h.cli(&["set", "-w", "synchronize-panes", "off"]).await;
    assert_eq!(code, 0);
    c.wait_for("S flag gone", |s| !s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*S")))
        .await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_picker() {
    let h = Harness::start("choose").await;
    // The plain list, tmux's look (the tree of blocks has its own test).
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "a"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix('c').await;
    c.wait_for("window 1", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("1:{SH}*"))).await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "b"]).await;
    assert_eq!(code, 0, "{err}");
    // The shell in the detached session must be up before its keystrokes matter.
    h.wait_capture("b:0", "shell prompt", |t| t.contains("keepane>")).await;

    // prefix w: every session expanded down to its panes, the cursor on the
    // current pane (item 5 of 8).
    c.prefix('w').await;
    // The pane title arrives over OSC, so wait for the fully drawn tree.
    c.wait_for("picker", |s| {
        let t = s.contents();
        // The title is cmd's own path, with "Administrator: " (localized)
        // in front of it when the test runs elevated; sh sets none.
        t.contains("[5/8] j/k move")
            && t.contains(&format!("(1)   - 0: {SH}- (1 panes) \""))
            && (cfg!(unix) || t.contains("cmd.exe\""))
    })
    .await;
    let text = c.text();
    assert!(text.contains("(0) - a: 2 windows (attached)"), "{text}");
    assert!(text.contains(&format!("(3)   - 1: {SH}* (1 panes)")), "{text}");
    assert!(text.contains("(5) - b: 1 windows"), "{text}");
    // Every session's current window carries the *, as in tmux.
    assert!(text.contains(&format!("(6)   - 0: {SH}* (1 panes)")), "{text}");
    // Under each window its panes: index, program, the active one *, mode.
    let pane_line = |n: usize| text.lines().find(|l| l.starts_with(&format!("({n})       - 0: "))).map(str::trim_end);
    for n in [2, 4, 7] {
        assert!(pane_line(n).is_some_and(|l| l.ends_with("* · normal")), "pane line {n}: {text}");
    }
    // vim motions: k up, g top, G bottom, j clamps at the end, digits jump.
    c.type_str("k").await;
    c.wait_for("k", |s| s.contents().contains("[4/8]")).await;
    c.type_str("g").await;
    c.wait_for("g", |s| s.contents().contains("[1/8]")).await;
    c.key(b'G' as u16, 'G', SHIFT_PRESSED).await;
    c.wait_for("G", |s| s.contents().contains("[8/8]")).await;
    c.type_str("j").await;
    c.key(b'Z' as u16, 'z', 0).await; // unbound key: ignored, picker stays
    c.wait_for("clamped", |s| s.contents().contains("[8/8]")).await;
    c.key(b'1' as u16, '1', 0).await;
    c.wait_for("digit", |s| s.contents().contains("[2/8]")).await;
    // Enter on "a:0" selects window 0 and closes the picker.
    c.enter().await;
    c.wait_for("window 0", |s| {
        let t = s.contents();
        !t.contains("j/k move") && s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*"))
    })
    .await;

    // prefix s: sessions only; Enter switches the client to "b".
    c.prefix('s').await;
    c.wait_for("sessions", |s| s.contents().contains("[1/2] j/k move")).await;
    let text = c.text();
    assert!(text.contains("(0) + a: 2 windows (attached)") && text.contains("(1) + b: 1 windows"), "{text}");
    assert!(!text.contains(&format!("0: {SH}")), "collapsed: {text}");
    c.type_str("j").await;
    c.enter().await;
    c.wait_for("switched to b", |s| {
        s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().starts_with(&format!("[b] 0:{SH}*"))
    })
    .await;

    // The picker is a mode, not a keyboard trap: the prefix still works, so
    // `prefix ?` (list-keys) overlays it and the next key dismisses the
    // overlay and leaves the picker standing.
    c.prefix('w').await;
    c.wait_for("picker", |s| s.contents().contains("j/k move")).await;
    c.prefix('?').await;
    c.wait_for("keys overlay", |s| s.contents().contains("bind-key -T prefix")).await;
    c.type_str("q").await; // dismisses the overlay only
    c.wait_for("picker back", |s| {
        let t = s.contents();
        t.contains("j/k move") && !t.contains("bind-key -T prefix")
    })
    .await;

    // q and Escape cancel without touching anything; keys never reach the pane.
    c.type_str("q").await;
    c.wait_for("closed", |s| !s.contents().contains("j/k move")).await;
    c.prefix('w').await;
    c.wait_for("picker again", |s| s.contents().contains("[8/8] j/k move")).await;
    // The tree is live: a window created meanwhile shows up, the cursor stays
    // on the same item (b:0's pane, still 8, of 10 now).
    let (code, _, err) = h.cli(&["new-window", "-d", "-t", "b"]).await;
    assert_eq!(code, 0, "{err}");
    c.wait_for("live", |s| {
        s.contents().contains("[8/10] j/k move") && s.contents().contains(&format!("(8)   - 1: {SH}"))
    })
    .await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("closed", |s| !s.contents().contains("j/k move")).await;
    // Nothing the picker consumed reached the shell: one untouched prompt.
    let out = h.cli(&["capture-pane", "-p", "-t", "b:0"]).await.1;
    assert_eq!(out.trim(), "keepane>", "picker keys leaked into the pane: {out:?}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_scrolls_and_follows_the_live_tree() {
    let h = Harness::start("choose-scroll").await;
    // base-index 1 must show through, and with the status line off the picker
    // gets the whole screen.
    h.cli(&["set", "-g", "base-index", "1"]).await;
    h.cli(&["set", "-g", "status", "off"]).await;
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "many"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    for _ in 0..29 {
        let (code, _, err) = h.cli(&["new-window", "-d", "-t", "many"]).await;
        assert_eq!(code, 0, "{err}");
    }

    // 1 session + 30 windows + their 30 panes = 61 items (window k is item
    // 2k-1, its pane 2k), 23 body rows (24 rows, no status line); the
    // cursor starts on the current pane.
    c.prefix('w').await;
    c.wait_for("picker", |s| s.contents().contains("[3/61] j/k move")).await;
    // Every line has its number, past nine too, padded so the text lines up.
    assert!(c.row(0).starts_with("(0)  - many: 30 windows (attached)"), "{:?}", c.row(0));
    assert!(c.row(1).starts_with(&format!("(1)    - 1: {SH}*")), "base-index 1: {:?}", c.row(1));
    assert!(c.row(2).starts_with("(2)        - 0: "), "the pane: {:?}", c.row(2));
    // Nothing scrolled yet; the last body row is item 22, window 11's pane.
    assert!(c.row(22).starts_with("(22)       - 0: "), "{:?}", c.row(22));
    assert!(c.row(21).starts_with("(21)   - 11:"), "{:?}", c.row(21));
    // G: the last item is visible on the last body row, the list scrolled.
    c.key(b'G' as u16, 'G', SHIFT_PRESSED).await;
    c.wait_for("bottom", |s| s.contents().contains("[61/61]")).await;
    // Item 38 is now the top line; its "(38)" jump tag travels with it.
    assert!(c.row(0).starts_with("(38)       - 0: "), "scrolled: {:?}", c.row(0));
    assert!(c.row(21).starts_with("(59)   - 30:"), "{:?}", c.row(21));
    // g: back to the top, scrolled back.
    c.type_str("g").await;
    c.wait_for("top", |s| s.contents().contains("[1/61]")).await;
    assert!(c.row(0).starts_with("(0)  - many: 30 windows"), "{:?}", c.row(0));
    // Two digits are one number: 1 2 is line 12.
    c.type_str("12").await;
    c.wait_for("line 12", |s| s.contents().contains("[13/61]")).await;
    // Another key ends the number: 1, j, 2 is line 2, not 12.
    c.type_str("1j2").await;
    c.wait_for("line 2", |s| s.contents().contains("[3/61]")).await;
    // So does a pause: 1, a pause, 3 is line 3, not 13.
    c.type_str("g1").await;
    c.wait_for("line 1", |s| s.contents().contains("[2/61]")).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    c.type_str("3").await;
    c.wait_for("line 3", |s| s.contents().contains("[4/61]")).await;
    // A number past the end starts over at its last digit: 6 9 is line 9.
    c.type_str("69").await;
    c.wait_for("line 9", |s| s.contents().contains("[10/61]")).await;
    c.type_str("g").await;
    c.wait_for("top again", |s| s.contents().contains("[1/61]")).await;

    // The tree is live under the cursor: kill a window and the count drops
    // while the selection stays on the session line.
    c.key(0x22, '\0', 0).await; // PageDown (VK_NEXT): one body page down
    c.wait_for("paged", |s| s.contents().contains("[24/61]")).await;
    let (code, _, err) = h.cli(&["kill-window", "-t", "many:30"]).await;
    assert_eq!(code, 0, "{err}");
    c.wait_for("59 items", |s| s.contents().contains("[24/59]")).await;
    // Enter on window 12 (item 23 = 2 * 12 - 1) selects it.
    c.enter().await;
    c.wait_for("selected", |s| !s.contents().contains("j/k move")).await;
    let (_, out, _) = h.cli(&["list-windows", "-t", "many"]).await;
    assert!(out.lines().nth(11).unwrap().starts_with(&format!("12: {SH}*")), "{out}");
    // prefix 0-9 is one key, as in tmux; past 9, prefix ' asks for the index.
    // The status line is off here: the prompt shows over the bottom row.
    c.prefix('\'').await;
    c.wait_for("index prompt", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().starts_with("(index)")).await;
    c.type_str("15").await;
    c.enter().await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, out, _) = h.cli(&["list-windows", "-t", "many"]).await;
        if out.lines().nth(14).is_some_and(|l| l.starts_with(&format!("15: {SH}*"))) {
            break;
        }
        assert!(Instant::now() < deadline, "window 15 not current: {out}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A message shows there too: no window 99 says so.
    c.prefix('\'').await;
    c.wait_for("index prompt", |s| s.contents().contains("(index)")).await;
    c.type_str("99").await;
    c.enter().await;
    c.wait_for("error message", |s| {
        let last = s.rows(0, COLS).nth(ROWS as usize - 1).unwrap();
        !last.starts_with("(index)") && last.contains("99")
    })
    .await;
    // find-window's list of hits is numbered past nine the same way.
    c.prefix('f').await;
    c.wait_for("find prompt", |s| s.contents().contains("(find-window)")).await;
    c.type_str(SH).await;
    c.enter().await;
    c.wait_for("hits", |s| s.contents().contains("[1/29] j/k move")).await;
    assert!(c.row(0).starts_with(&format!("(0)  many:{SH}")), "{:?}", c.row(0));
    assert!(c.text().contains(&format!("(12) many:{SH}")), "{}", c.text());
    c.type_str("12").await;
    c.wait_for("hit 12", |s| s.contents().contains("[13/29]")).await;
    c.enter().await;
    c.wait_for("picked", |s| !s.contents().contains("j/k move")).await;
    let (_, out, _) = h.cli(&["list-windows", "-t", "many"]).await;
    assert!(out.lines().nth(12).is_some_and(|l| l.starts_with(&format!("13: {SH}*"))), "{out}");
    h.cli(&["kill-server"]).await;
}

/// The tree view (`choose-tree-style tree`): blocks under a keepane root;
/// Up and Down go to the parent and the first child, Left and Right along
/// the level; `-` folds, Down opens; `v` gives the plain list, then the
/// chart, then the tree again; `choose-tree-style` picks the view it opens in.
#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_as_a_tree_of_blocks() {
    let h = Harness::start("treeview").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "s"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["split-window", "-h", "-t", "s"]).await;
    h.cli(&["new-window", "-d", "-t", "s"]).await;
    h.cli(&["new", "-d", "-s", "t"]).await;
    assert_eq!(h.cli(&["show", "-gv", "choose-tree-style"]).await.1.trim(), "chart");
    h.cli(&["set", "-g", "choose-tree-style", "tree"]).await;
    // root, s, s:0, its 2 panes, s:1, its pane, t, t:0, its pane: 10 lines;
    // the cursor on the current pane, s:0's second (line 5).
    c.prefix('w').await;
    let at = |n: usize, of: usize| move |s: &vt100::Screen| s.contents().contains(&format!("[{n}/{of}] ↑↓"));
    c.wait_for("tree", at(5, 10)).await;
    let text = c.text();
    let row = |i: usize| text.lines().nth(i).unwrap_or_default().trim_end().to_string();
    assert_eq!(row(0), " keepane");
    assert!(row(1).starts_with("├─  1  s · 2 windows · attached"), "{text}");
    assert!(row(2).starts_with("│  ├─  2  0:") && row(2).ends_with("· 2 panes"), "{text}");
    assert!(row(3).starts_with("│  │  ├─  3  0:") && row(4).starts_with("│  │  └─  4  1:"), "{text}");
    assert!(row(5).starts_with("│  └─  5  1:") && row(6).starts_with("│     └─  6  0:"), "{text}");
    assert!(
        row(7).starts_with("└─  7  t · 1 window")
            && !row(7).contains("windows")
            && row(9).starts_with("      └─  9  0:"),
        "{text}"
    );
    // The blocks' colours: a session blue, the selected one yellow.
    let bg = |s: &vt100::Screen, r: u16| s.cell(r, 4).map(|c| c.bgcolor());
    assert_eq!(bg(c.screen.screen(), 1), Some(vt100::Color::Idx(4)), "session block");
    assert_eq!(c.screen.screen().cell(4, 10).map(|c| c.bgcolor()), Some(vt100::Color::Idx(3)), "selected block");
    let key = |vk: u16| (vk, '\0');
    let (up, down, left, right) = (key(0x26), key(0x28), key(0x25), key(0x27));
    // Up: the window, the session; never the root.
    for (k, want) in [(up, 3), (up, 2), (up, 2)] {
        c.key(k.0, k.1, 0).await;
        c.wait_for("up", at(want, 10)).await;
    }
    // Down: the first child, and its first child.
    for (k, want) in [(down, 3), (down, 4)] {
        c.key(k.0, k.1, 0).await;
        c.wait_for("down", at(want, 10)).await;
    }
    // Right along the panes, across windows and sessions; stops at the end.
    for (k, want) in [(right, 5), (right, 7), (right, 10), (right, 10), (left, 7)] {
        c.key(k.0, k.1, 0).await;
        c.wait_for("along", at(want, 10)).await;
    }
    // Up to s:1, fold it (its pane goes), Down opens it again.
    c.key(up.0, up.1, 0).await;
    c.wait_for("s:1", at(6, 10)).await;
    c.type_str("-").await;
    c.wait_for("folded", |s| s.contents().contains("[6/9] ↑↓") && s.contents().contains("· 1 pane + ")).await;
    c.key(down.0, down.1, 0).await;
    c.wait_for("opened", at(6, 10)).await;
    // v: the plain list (no root, so s:1 is line 5 of 9), the chart, the
    // tree again.
    c.type_str("v").await;
    c.wait_for("list", |s| s.contents().contains("[5/9] j/k move") && s.contents().contains("(0) - s: 2 windows"))
        .await;
    c.type_str("v").await;
    c.wait_for("chart", |s| s.contents().contains("[6/10] hjkl move") && s.contents().contains("a all  v tree")).await;
    c.type_str("v").await;
    c.wait_for("tree again", |s| s.contents().contains("[6/10] ↑↓") && s.contents().contains("v list")).await;
    // Enter on a pane goes to it: s:0's first.
    c.key(left.0, left.1, 0).await;
    c.wait_for("s:0", at(3, 10)).await;
    c.key(down.0, down.1, 0).await;
    c.wait_for("its first pane", at(4, 10)).await;
    c.enter().await;
    c.wait_for("gone", |s| !s.contents().contains("[4/10]")).await;
    assert_eq!(h.cli(&["display-message", "-p", "-t", "s", "#{window_index}.#{pane_index}"]).await.1.trim(), "0.0");
    // A tagged block says so in front of its number; T clears it.
    c.prefix('w').await;
    c.wait_for("tree on s:0.0", at(4, 10)).await;
    c.type_str("t").await;
    c.wait_for("tagged", |s| s.contents().contains("[1 tagged]") && s.contents().contains("├─  * 3  0:")).await;
    c.key(b'T' as u16, 'T', SHIFT_PRESSED).await;
    c.wait_for("untagged", |s| !s.contents().contains("tagged]") && s.contents().contains("├─  3  0:")).await;
    c.type_str("q").await;
    // The view it opens in is an option: the plain list.
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    c.prefix('w').await;
    c.wait_for("opens as a list", |s| s.contents().contains("[3/9] j/k move")).await;
    c.type_str("q").await;
    h.cli(&["kill-server"]).await;
}

/// The chart (the default for prefix w): the keepane root centred, the
/// sessions on the next row, the windows of the session on the branch, its
/// window's panes; Left and Right move along a row and the rows under it
/// follow; Down goes to the current window and the active pane, not the
/// first; `-` folds nothing here; Enter goes to the pane.
#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_as_a_chart() {
    // The link directory the link tests use: a machine that never paired
    // must not get a key pair from opening the chart.
    let dir = std::env::temp_dir().join(format!("keepane-test-link-{}", std::process::id()));
    unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
    let h = Harness::start("chartview").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "s"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["split-window", "-h", "-t", "s"]).await;
    h.cli(&["new-window", "-d", "-t", "s"]).await;
    h.cli(&["new", "-d", "-s", "t"]).await;
    assert_eq!(h.cli(&["show", "-gv", "choose-tree-style"]).await.1.trim(), "chart");
    // Items as in the tree: root, s, s:0, its 2 panes, s:1, its pane, t,
    // t:0, its pane; the cursor on s:0's second pane (5 of 10).
    c.prefix('w').await;
    let at = |n: usize, of: usize| move |s: &vt100::Screen| s.contents().contains(&format!("[{n}/{of}] hjkl move"));
    c.wait_for("chart", at(5, 10)).await;
    assert!(!keepane::link::dir(&h.socket).join("key").exists(), "never paired, so no key pair made");
    let text = c.text();
    let rows: Vec<&str> = text.lines().collect();
    let find = |s: &str| rows.iter().position(|r| r.contains(s));
    // The root in a box on top, about the middle of 80 columns.
    let root = rows[1].find("keepane").unwrap_or(0);
    assert!(rows[0].contains('┌') && rows[1].contains("│ keepane │"), "{text}");
    assert!((30..=42).contains(&root), "{text}");
    // Then the sessions, the windows of s, the panes of s:0, row under row.
    let (sessions, windows, panes) = (find("1  s").unwrap(), find("2  0:").unwrap(), find("3  0:").unwrap());
    assert!(rows[sessions].contains("7  t"), "{text}");
    assert!(rows[windows].contains("5  1:") && !text.contains("8  0:"), "{text}");
    assert!(rows[panes].contains("4  1:") && !text.contains("6  0:"), "{text}");
    assert!(sessions < windows && windows < panes, "{text}");
    assert!(rows[sessions + 1].contains("2 windows · attached"), "{text}");
    // The keys fit in 80 columns, down to `q quit`.
    assert!(text.contains("hjkl move  x kill  D delete  t tag  f filter  a all  v tree  q quit"), "{text}");
    // a: every node, then the branch again.
    c.type_str("a").await;
    c.wait_for("all of it", |s| {
        let t = s.contents();
        at(5, 10)(s) && t.contains("a branch") && t.contains("6  0:") && t.contains("8  0:") && t.contains("9  0:")
    })
    .await;
    c.type_str("a").await;
    c.wait_for("the branch", |s| at(5, 10)(s) && s.contents().contains("a all") && !s.contents().contains("8  0:"))
        .await;
    let key = |vk: u16| (vk, '\0');
    let (up, down, left, right) = (key(0x26), key(0x28), key(0x25), key(0x27));
    // Right along the panes, across windows and sessions: the rows above
    // the selection follow it.
    c.key(right.0, right.1, 0).await;
    c.wait_for("s:1's pane", |s| at(7, 10)(s) && s.contents().contains("6  0:") && !s.contents().contains("3  0:"))
        .await;
    c.key(right.0, right.1, 0).await;
    c.wait_for("t's pane", |s| at(10, 10)(s) && s.contents().contains("8  0:") && !s.contents().contains("5  1:"))
        .await;
    // Up to the window, the session; never the root.
    for want in [9, 8, 8] {
        c.key(up.0, up.1, 0).await;
        c.wait_for("up", at(want, 10)).await;
    }
    // Left to s; Down to its current window (s:0) and that window's active
    // pane (its second, 4, not its first, 3).
    c.key(left.0, left.1, 0).await;
    c.wait_for("s", at(2, 10)).await;
    for want in [3, 5] {
        c.key(down.0, down.1, 0).await;
        c.wait_for("down", at(want, 10)).await;
    }
    // Nothing folds in the chart: `-` leaves all 10. h and l go along a
    // row, k and j up to the parent and down to the current child, as the
    // arrows do (not line by line, as in the tree).
    c.type_str("-").await;
    c.type_str("h").await;
    c.wait_for("not folded; h: the pane before", at(4, 10)).await;
    c.type_str("l").await;
    c.wait_for("l: back", at(5, 10)).await;
    for (key, want) in [("k", 3), ("k", 2), ("k", 2), ("j", 3), ("j", 5)] {
        c.type_str(key).await;
        c.wait_for(key, at(want, 10)).await;
    }
    // A fold made in the tree hides nothing in the chart.
    c.type_str("v").await;
    c.wait_for("tree", |s| s.contents().contains("v list")).await;
    c.type_str("-").await;
    c.wait_for("folded in the tree", |s| s.contents().contains("[3/8] ↑↓")).await;
    c.type_str("v").await;
    c.type_str("v").await;
    c.wait_for("chart, all there", |s| {
        at(3, 10)(s) && s.contents().contains("3  0:") && s.contents().contains("v tree  q quit")
    })
    .await;
    c.key(down.0, down.1, 0).await;
    c.wait_for("its active pane", at(5, 10)).await;
    // Enter on s:0's first pane goes there.
    c.key(left.0, left.1, 0).await;
    c.wait_for("first pane", at(4, 10)).await;
    c.enter().await;
    c.wait_for("gone", |s| !s.contents().contains("[4/10]")).await;
    assert_eq!(h.cli(&["display-message", "-p", "-t", "s", "#{window_index}.#{pane_index}"]).await.1.trim(), "0.0");
    // Sessions only (choose-tree -s): the root and the sessions.
    c.prefix('s').await;
    c.wait_for("sessions", |s| s.contents().contains("[2/3] hjkl move") && s.contents().contains("2  t")).await;
    c.type_str("q").await;
    // Thirty windows in t on a small screen: the row of windows scrolls to
    // keep the selected one in view, `›` saying there is more.
    for _ in 0..29 {
        h.cli(&["new-window", "-d", "-t", "t"]).await;
    }
    c.send(ClientMsg::Resize { cols: 40, rows: 14 }).await;
    c.screen = vt100::Parser::new(14, 40, 0);
    c.prefix('w').await;
    c.wait_for("small chart", |s| s.contents().contains("/68] hjkl move")).await;
    for _ in 0..3 {
        c.key(right.0, right.1, 0).await;
    }
    c.wait_for("along the panes to t:0's", |s| s.contents().contains("[10/68] hjkl move")).await;
    c.key(up.0, up.1, 0).await;
    c.wait_for("its window", |s| s.contents().contains("[9/68] hjkl move")).await;
    for _ in 0..20 {
        c.key(right.0, right.1, 0).await;
    }
    c.wait_for("t's 21st window, in view, more both sides", |s| {
        let rows: Vec<String> = s.rows(0, 40).collect();
        let at = rows.iter().find(|r| r.contains("20:"));
        s.contents().contains("[49/68] hjkl move")
            && at.is_some_and(|r| r.starts_with('‹') && r.trim_end().ends_with('›'))
    })
    .await;
    // Degenerate sizes: nothing panics, and it comes back.
    for (cols, rows) in [(1u16, 1u16), (20, 3), (3, 20)] {
        c.send(ClientMsg::Resize { cols, rows }).await;
        c.screen = vt100::Parser::new(rows, cols, 0);
    }
    c.send(ClientMsg::Resize { cols: COLS, rows: ROWS }).await;
    c.screen = vt100::Parser::new(ROWS, COLS, 0);
    c.wait_for("back", |s| s.contents().contains("[49/68] hjkl move")).await;
    c.type_str("q").await;
    // Seconds of an open chart later (the tick asks every second): still none.
    assert!(!keepane::link::dir(&h.socket).join("key").exists(), "never paired, so no key pair made");
    h.cli(&["kill-server"]).await;
}

/// In `C-b w`, `x` kills and `D` (or Delete) kills and forgets: a session
/// asks first, `n` leaves it; one killed with `x` can be resumed, one with
/// `D` cannot; a window goes without a question. A session whose last
/// program exits forgets its save a moment later; `kill-server` keeps them.
#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_kills_or_forgets_a_session() {
    let h = Harness::start("forget").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "here"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    for s in ["keepme", "gone"] {
        let (code, _, err) = h.cli(&["new", "-d", "-s", s]).await;
        assert_eq!(code, 0, "{err}");
    }
    h.cli(&["new-window", "-d", "-t", "keepme"]).await;
    h.cli(&["save-session", "-a"]).await;
    let saved = async || h.cli(&["list-saved"]).await.1;
    let names = |out: &str| out.lines().filter_map(|l| l.split(':').next()).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(names(&saved().await).len(), 3, "{}", saved().await);
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    // The picker on one session (filtered to it), then a key.
    let pick = async |c: &mut Conn, name: &str| {
        c.prefix('w').await;
        c.wait_for("picker", |s| s.contents().contains("q quit")).await;
        c.type_str("f").await;
        c.type_str(name).await;
        c.enter().await;
        c.wait_for("filtered", |s| s.contents().contains(&format!("[filter: {name}]"))).await;
        // The session: the first line.
        c.type_str("g").await;
        c.wait_for("on the session", |s| s.contents().contains("[1/")).await;
    };
    // x on a session: asked; n leaves it.
    pick(&mut c, "keepme").await;
    c.type_str("x").await;
    c.wait_for("asked", |s| s.contents().contains("kill session keepme? (y/n)")).await;
    c.type_str("n").await;
    c.wait_for("not asked any more", |s| !s.contents().contains("(y/n)")).await;
    assert_eq!(h.cli(&["has-session", "-t", "keepme"]).await.0, 0, "n kept it");
    // y: gone, its save kept (resume brings it back).
    c.type_str("x").await;
    c.wait_for("asked again", |s| s.contents().contains("kill session keepme? (y/n)")).await;
    c.type_str("y").await;
    h.wait_for_cli("keepme killed", &["has-session", "-t", "keepme"], |code, _| code == 1).await;
    assert!(names(&saved().await).contains(&"keepme".to_string()), "{}", saved().await);
    c.type_str("q").await;
    // D on a session: asked, then gone with its save.
    pick(&mut c, "gone").await;
    c.type_str("D").await;
    c.wait_for("asked", |s| {
        s.contents().contains("kill and forget session gone (resume will not bring it back)? (y/n)")
    })
    .await;
    c.type_str("y").await;
    h.wait_for_cli("gone killed", &["has-session", "-t", "gone"], |code, _| code == 1).await;
    assert!(!names(&saved().await).contains(&"gone".to_string()), "{}", saved().await);
    c.type_str("q").await;
    // D (here the Delete key) on a window: no question, gone at once.
    h.cli(&["new-window", "-d", "-t", "here", "-n", "spare"]).await;
    c.prefix('w').await;
    c.wait_for("picker", |s| s.contents().contains("q quit")).await;
    c.type_str("f").await;
    c.type_str("spare").await;
    c.enter().await;
    c.wait_for("filtered", |s| s.contents().contains("[filter: spare]")).await;
    c.key(0x28, '\0', 0).await; // Down, from the session to its window
    c.key(0x2E, '\0', 0).await; // Delete
    h.wait_for_cli("window gone", &["list-windows", "-t", "here"], |_, out| !out.contains("spare")).await;
    assert!(!c.text().contains("(y/n)"), "{}", c.text());
    c.type_str("q").await;
    // A session whose last program exits forgets its save, a moment later.
    // (On Windows a test here that announces a shutdown reaches every
    // server in this process, and a shutdown keeps the saves: an attempt
    // with one in it does not count.)
    #[cfg(windows)]
    let shutdowns = keepane::shutdown::fired;
    #[cfg(not(windows))]
    let shutdowns = || 0usize;
    let mut forgotten = false;
    for attempt in 0..3 {
        let name = format!("ends{attempt}");
        h.cli(&["new", "-d", "-s", &name]).await;
        h.cli(&["save-session", "-t", &name]).await;
        let before = shutdowns();
        h.cli(&["send-keys", "-t", &name, "exit", "Enter"]).await;
        h.wait_for_cli("it ended", &["has-session", "-t", &name], |code, _| code == 1).await;
        // Not at once (a shutdown may be what ended it): still there 3 s later.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(names(&saved().await).contains(&name), "not at once: a shutdown may be what ended it");
        // 10 s; a shutdown announced in the last minute holds it a minute.
        let wait = if shutdowns() > 0 { 80 } else { 25 };
        let deadline = Instant::now() + Duration::from_secs(wait);
        while names(&saved().await).contains(&name) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !names(&saved().await).contains(&name) {
            forgotten = true;
            break;
        }
        assert_ne!(shutdowns(), before, "the save of a session that ended stayed: {}", saved().await);
    }
    assert!(forgotten, "a shutdown came in every attempt"); // kill-server keeps every save: here and keepme come back.
    h.cli(&["kill-server"]).await;
    let out = std::fs::read_dir(&h.sessions_dir)
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(out.iter().any(|f| f.starts_with("here")) && out.iter().any(|f| f.starts_with("keepme")), "{out:?}");
    assert!(!out.iter().any(|f| f.starts_with("gone")), "{out:?}");
}
#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_degenerate_sizes_and_wide_names() {
    let h = Harness::start("choose-edge").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "会话", "-n", "编辑器"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "gone"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["set", "-g", "choose-tree-style", "tree"]).await;

    c.prefix('w').await;
    // The tree: the keepane root, 2 sessions, their
    // windows and panes = 7 items; the cursor starts on 会话's pane.
    c.wait_for("picker", |s| s.contents().contains("[4/7]")).await;
    // Double-width names survive the layout, each block under its tree lines.
    assert_eq!(c.row(0).trim_end(), " keepane", "{:?}", c.row(0));
    assert!(c.row(1).starts_with("├─  1  会话 · 1 window · attached "), "{:?}", c.row(1));
    assert!(c.row(2).starts_with("│  └─  2  0:编辑器* · 1 pane "), "{:?}", c.row(2));
    assert!(c.row(3).starts_with("│     └─  3  0:") && c.row(3).contains("* · normal"), "{:?}", c.row(3));
    assert!(c.row(4).starts_with("└─  4  gone · 1 window "), "{:?}", c.row(4));

    // A terminal with room for a single body row still renders hint and all.
    c.send(ClientMsg::Resize { cols: 20, rows: 3 }).await;
    c.screen = vt100::Parser::new(3, 20, 0);
    c.wait_for("tiny", |s| s.rows(0, 20).nth(1).unwrap().starts_with("[4/7]")).await;
    assert!(c.row(0).starts_with("│     └─  3  0:"), "the selection stays visible: {:?}", c.row(0));
    // One column wide is degenerate but must not panic or wedge the server.
    c.send(ClientMsg::Resize { cols: 1, rows: 1 }).await;
    c.screen = vt100::Parser::new(1, 1, 0);
    c.send(ClientMsg::Resize { cols: 80, rows: 24 }).await;
    c.screen = vt100::Parser::new(ROWS, COLS, 0);
    c.wait_for("back", |s| s.contents().contains("[4/7]")).await;

    // The item under the cursor vanishing clamps the selection instead of
    // pointing past the end.
    c.key(0x23, '\0', 0).await; // End: the last item, the pane of "gone"
    c.wait_for("last", |s| s.contents().contains("[7/7]")).await;
    let (code, _, err) = h.cli(&["kill-session", "-t", "gone"]).await;
    assert_eq!(code, 0, "{err}");
    c.wait_for("clamped", |s| s.contents().contains("[4/4]")).await;
    c.enter().await;
    c.wait_for("still alive", |s| {
        !s.contents().contains("q quit") && s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("0:编辑器*")
    })
    .await;
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.starts_with("会话: 1 windows") && !out.contains("gone"), "{out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_mode_vi_motions_and_modes() {
    let h = Harness::start("motions").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "v"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.type_str("echo alpha beta gamma delta").await;
    c.enter().await;
    c.wait_for("output", |s| s.contents().contains("alpha beta gamma delta")).await;

    // Search puts the cursor on a known word; w and e then walk from there.
    c.prefix('[').await;
    c.type_str("?alpha beta").await;
    c.enter().await;
    c.type_str("v").await; // start a selection at the match
    c.type_str("e").await; // to the end of "alpha"
    c.enter().await; // copy
    c.wait_for("copied", |s| s.contents().contains("copied")).await;
    let (_, out, _) = h.cli(&["show-buffer"]).await;
    assert_eq!(out.trim_end(), "alpha", "e stops at the end of the word: {out:?}");

    c.prefix('[').await;
    c.type_str("?alpha beta").await;
    c.enter().await;
    c.type_str("ww").await; // over "alpha" and "beta" to "gamma"
    c.type_str("v").await;
    c.type_str("e").await;
    c.enter().await;
    // The copy and the show-buffer travel different pipes: wait for the
    // buffer to change rather than assuming the Enter landed first.
    let deadline = Instant::now() + Duration::from_secs(5);
    let out = loop {
        let (_, out, _) = h.cli(&["show-buffer"]).await;
        if out.trim_end() != "alpha" {
            break out;
        }
        assert!(Instant::now() < deadline, "the second copy never replaced the buffer: {out:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(out.trim_end(), "gamma", "w moves a word at a time: {out:?}");
    c.prefix('[').await;
    c.type_str("?gamma").await;
    c.enter().await;
    c.type_str("b").await; // back one word, to "beta"
    c.type_str("v").await;
    c.type_str("e").await;
    c.enter().await;
    h.wait_buffer("b goes back a word", |out| out.trim_end() == "beta").await;

    // A count repeats a motion: 3j moves three lines.
    c.prefix('[').await;
    c.type_str("gv").await; // top of the scrollback, start selecting
    c.type_str("3j").await;
    c.enter().await;
    let three = h.wait_buffer("3j selected three lines", |out| out.lines().count() >= 3).await;

    // C-v makes the selection a rectangle: same columns on every line.
    c.prefix('[').await;
    c.key(b'V' as u16, '\x16', LEFT_CTRL_PRESSED).await; // C-v
    c.type_str("jjll").await;
    c.enter().await;
    let out = h.wait_buffer("the rectangle copied", |out| out != three).await;
    assert!(out.lines().all(|l| l.chars().count() <= 3), "a rectangle is narrow: {out:?}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clock_conditionals_and_client_commands() {
    let h = Harness::start("clock").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "c1"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;

    // prefix t draws a clock, with what is worth a glance under it: the
    // date and UTC, the pane (its program, how long it ran and has been
    // quiet), the machine; any key puts it away.
    let weekday = chrono::Local::now().format("%A").to_string();
    c.prefix('t').await;
    c.wait_for("clock", |s| s.contents().contains("███")).await;
    c.wait_for("the lines under it", |s| {
        let t = s.contents();
        t.contains(" · UTC ") && t.contains(" · up ") && t.contains(" · quiet ")
    })
    .await;
    assert!(c.text().contains(&weekday) || !chrono::Local::now().format("%A").to_string().eq(&weekday), "{}", c.text());
    c.type_str("x").await;
    c.wait_for("clock gone", |s| !s.contents().contains("███")).await;
    // clock-mode-info off: the time alone, as tmux draws it; style 12: AM / PM.
    h.cli(&["set", "-g", "clock-mode-info", "off"]).await;
    h.cli(&["set", "-g", "clock-mode-style", "12"]).await;
    c.prefix('t').await;
    c.wait_for("the bare clock", |s| {
        s.contents().contains("███") && (s.contents().contains("AM") || s.contents().contains("PM"))
    })
    .await;
    assert!(!c.text().contains(" · UTC "), "{}", c.text());
    c.type_str("x").await;
    c.wait_for("clock gone", |s| !s.contents().contains("███")).await;
    let (code, _, err) = h.cli(&["set", "-g", "clock-mode-style", "13"]).await;
    assert!(code != 0 && err.contains("12 or 24"), "{err}");
    // clock-mode-colour: the digits in it.
    h.cli(&["set", "-g", "clock-mode-colour", "red"]).await;
    c.prefix('t').await;
    c.wait_for("a red clock", |s| {
        (0..ROWS).any(|y| {
            (0..COLS).any(|x| {
                s.cell(y, x).is_some_and(|cell| cell.contents() == "█" && cell.fgcolor() == vt100::Color::Idx(1))
            })
        })
    })
    .await;
    c.type_str("x").await;

    // if-shell -F takes the branch the format says.
    h.cli(&["if-shell", "-F", "#{?session_name,yes,}", "set -g @cond true", "set -g @cond false"]).await;
    let (_, out, _) = h.cli(&["show-options", "-gv", "@cond"]).await;
    assert_eq!(out.trim(), "true");
    h.cli(&["if-shell", "-F", "", "set -g @cond then", "set -g @cond else"]).await;
    let (_, out, _) = h.cli(&["show-options", "-gv", "@cond"]).await;
    assert_eq!(out.trim(), "else");
    // Without -F the shell's exit status decides.
    let exit = |code: u32| if cfg!(windows) { format!("cmd /c exit {code}") } else { format!("exit {code}") };
    h.cli(&["if-shell", &exit(0), "set -g @sh ok", "set -g @sh bad"]).await;
    let (_, out, _) = h.cli(&["show-options", "-gv", "@sh"]).await;
    assert_eq!(out.trim(), "ok");
    h.cli(&["if-shell", &exit(1), "set -g @sh ok", "set -g @sh bad"]).await;
    let (_, out, _) = h.cli(&["show-options", "-gv", "@sh"]).await;
    assert_eq!(out.trim(), "bad");

    // The attached client shows up, and messages are remembered.
    let (_, out, _) = h.cli(&["list-clients"]).await;
    assert!(out.contains("c1") && out.contains("[80x24]"), "{out}");
    let (_, out, _) = h.cli(&["show-messages"]).await;
    assert!(out.contains("copied") || out.lines().count() >= 1, "{out}");

    // switch-client acts on the client that runs it, so this goes through the
    // session's own keyboard: prefix ) to the next session, then -l back.
    h.cli(&["new", "-d", "-s", "c2"]).await;
    c.prefix(')').await;
    c.wait_for("on c2", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().starts_with("[c2]")).await;
    c.prefix(':').await;
    c.type_str("switch-client -l").await;
    c.enter().await;
    c.wait_for("back on c1", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().starts_with("[c1]")).await;

    // detach-client -a sends everyone home.
    let (code, _, err) = h.cli(&["detach-client", "-a"]).await;
    assert_eq!(code, 0, "{err}");
    let reason = c.wait_detached().await;
    assert_eq!(reason, "detached");
    let (_, out, _) = h.cli(&["list-clients"]).await;
    assert_eq!(out.trim(), "no clients attached");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_small_tmux_commands() {
    let h = Harness::start("small").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "a"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["split-window", "-h", "-t", "a"]).await;
    h.cli(&["split-window", "-v", "-t", "a"]).await;

    // rotate-window moves the panes around the layout, keeping its shape:
    // the geometry stays, the pane ids in those places move.
    async fn ids(h: &Harness) -> Vec<String> {
        let (_, out, _) = h.cli(&["list-panes", "-t", "a"]).await;
        out.lines().map(|l| l.split_whitespace().nth(2).unwrap().to_string()).collect()
    }
    async fn geometry(h: &Harness) -> Vec<String> {
        let (_, out, _) = h.cli(&["list-panes", "-t", "a"]).await;
        out.lines().map(|l| l.split_whitespace().nth(1).unwrap().to_string()).collect()
    }
    let before = ids(&h).await;
    let shape = geometry(&h).await;
    let (code, _, err) = h.cli(&["rotate-window", "-t", "a"]).await;
    assert_eq!(code, 0, "{err}");
    let after = ids(&h).await;
    assert_eq!(after.len(), before.len());
    assert_ne!(after, before, "the panes moved: {before:?} -> {after:?}");
    assert_eq!(geometry(&h).await, shape, "the layout itself did not change");
    h.cli(&["rotate-window", "-D", "-t", "a"]).await;
    assert_eq!(ids(&h).await, before, "-D undoes -U");

    // next-layout and previous-layout are the names for select-layout -n/-p.
    let (code, _, err) = h.cli(&["next-layout", "-t", "a"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["previous-layout", "-t", "a"]).await;
    assert_eq!(code, 0, "{err}");

    // Listing commands and clients.
    let (_, out, _) = h.cli(&["list-commands"]).await;
    assert!(out.lines().count() > 50, "every command name: {}", out.lines().count());
    assert!(out.contains("rotate-window") && out.contains("attach-session"), "{out}");
    let (_, out, _) = h.cli(&["list-clients"]).await;
    assert_eq!(out.trim(), "no clients attached", "{out}");

    // The environment new panes get.
    h.cli(&["set-environment", "KEEPANE_TEST_VAR", "hello"]).await;
    let (_, out, _) = h.cli(&["show-environment"]).await;
    assert!(out.contains("KEEPANE_TEST_VAR=hello"), "{out}");
    let (code, _, err) = h.cli(&["new-window", "-d", "-t", "a"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["send-keys", "-t", "a:1", &format!("echo {}", var("KEEPANE_TEST_VAR")), "Enter"]).await;
    let pane = h.wait_capture("a:1", "the variable", |t| t.contains("hello")).await;
    assert!(pane.contains("hello"), "{pane}");
    h.cli(&["set-environment", "-r", "KEEPANE_TEST_VAR"]).await;
    let (_, out, _) = h.cli(&["show-environment"]).await;
    assert!(!out.contains("KEEPANE_TEST_VAR"), "{out}");

    // respawn-pane restarts a live pane only with -k.
    let (code, _, err) = h.cli(&["respawn-pane", "-t", "a:1"]).await;
    assert_eq!(code, 1, "a live pane needs -k");
    assert!(err.contains("still running"), "{err}");
    h.cli(&["send-keys", "-t", "a:1", "echo before-respawn", "Enter"]).await;
    h.wait_capture("a:1", "the marker", |t| t.contains("before-respawn")).await;
    // It is the same pane afterwards: its name and what waits in its inbox stay.
    h.cli(&["rename-pane", "-t", "a:1", "stays"]).await;
    let (code, _, err) = h.cli(&["send-message", "-t", "%stays", "for later"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["respawn-pane", "-k", "-t", "a:1"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, name, _) = h.cli(&["display-message", "-p", "-t", "a:1", "#{pane_name} #{pane_inbox}"]).await;
    assert_eq!(name.trim(), "stays 1", "name and inbox kept");
    let pane =
        h.wait_capture("a:1", "a fresh shell", |t| !t.contains("before-respawn") && t.contains("keepane>")).await;
    assert!(!pane.contains("before-respawn"), "the pane started over: {pane}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn paste_buffers() {
    let h = Harness::start("buffers").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "b"]).await;
    assert_eq!(code, 0, "{err}");

    let (_, out, _) = h.cli(&["list-buffers"]).await;
    assert_eq!(out.trim(), "no buffers");
    h.cli(&["set-buffer", "hello from keepane"]).await;
    h.cli(&["set-buffer", "-b", "named", "second buffer"]).await;
    let (_, out, _) = h.cli(&["list-buffers"]).await;
    assert!(out.contains("named: 13 bytes: second buffer"), "{out}");
    assert!(out.contains("buffer0: 18 bytes: hello from keepane"), "{out}");
    let (_, out, _) = h.cli(&["show-buffer", "-b", "named"]).await;
    assert_eq!(out.trim(), "second buffer");
    // No -b: the newest buffer.
    let (_, out, _) = h.cli(&["show-buffer"]).await;
    assert_eq!(out.trim(), "second buffer");

    // -a appends to a named buffer.
    h.cli(&["set-buffer", "-a", "-b", "named", " and more"]).await;
    let (_, out, _) = h.cli(&["show-buffer", "-b", "named"]).await;
    assert_eq!(out.trim(), "second buffer and more");

    // Buffers go to and come from files.
    let file = std::env::temp_dir().join(format!("keepane-buffer-{}.txt", std::process::id()));
    let (code, _, err) = h.cli(&["save-buffer", "-b", "named", &file.to_string_lossy()]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "second buffer and more");
    std::fs::write(&file, "from a file").unwrap();
    let (code, _, err) = h.cli(&["load-buffer", "-b", "loaded", &file.to_string_lossy()]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["show-buffer", "-b", "loaded"]).await;
    assert_eq!(out.trim(), "from a file");

    // Pasting a named buffer reaches the pane.
    let (code, _, err) = h.cli(&["paste-buffer", "-b", "loaded", "-t", "b"]).await;
    assert_eq!(code, 0, "{err}");
    let pane = h.wait_capture("b", "the pasted text", |t| t.contains("from a file")).await;
    assert!(pane.contains("from a file"), "{pane}");

    // delete-buffer drops the newest, then the named one.
    h.cli(&["delete-buffer"]).await;
    h.cli(&["delete-buffer", "-b", "named"]).await;
    let (_, out, _) = h.cli(&["list-buffers"]).await;
    assert!(!out.contains("named:"), "{out}");
    let (code, _, err) = h.cli(&["paste-buffer", "-b", "nosuch", "-t", "b"]).await;
    assert_eq!(code, 1);
    assert_eq!(err, "no buffer nosuch");
    let _ = std::fs::remove_file(&file);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn join_pane_marks_and_exact_sizes() {
    let h = Harness::start("join").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "j"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["new-window", "-d", "-t", "j"]).await;
    async fn panes(h: &Harness, w: &str) -> usize {
        let (_, out, _) = h.cli(&["list-panes", "-t", w]).await;
        out.lines().count()
    }
    assert_eq!(panes(&h, "j:0").await, 1);
    assert_eq!(panes(&h, "j:1").await, 1);

    // Move the pane of window 1 next to the pane of window 0.
    let (code, _, err) = h.cli(&["join-pane", "-h", "-s", "j:1", "-t", "j:0"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(panes(&h, "j:0").await, 2, "the pane moved across");
    let (_, out, _) = h.cli(&["list-windows", "-t", "j"]).await;
    assert_eq!(out.lines().count(), 1, "the empty window went away: {out}");

    // A marked pane is what join-pane takes when there is no -s.
    h.cli(&["new-window", "-d", "-t", "j"]).await;
    let (code, _, err) = h.cli(&["select-pane", "-m", "-t", "j:1"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["join-pane", "-v", "-t", "j:0"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(panes(&h, "j:0").await, 3);

    // The other way: a window's only pane into a window after it. Its own
    // window goes, the one after moves down, and the pane lands there.
    h.cli(&["new-window", "-d", "-t", "j"]).await;
    h.cli(&["new-window", "-d", "-t", "j"]).await;
    let (code, _, err) = h.cli(&["join-pane", "-d", "-s", "j:1", "-t", "j:2"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["list-windows", "-t", "j"]).await;
    assert_eq!(out.lines().count(), 2, "{out}");
    assert_eq!(panes(&h, "j:1").await, 2, "the pane moved across");

    // Exact sizes.
    async fn width(h: &Harness) -> u16 {
        let (_, out, _) = h.cli(&["list-panes", "-t", "j:0"]).await;
        out.lines().next().unwrap().split(['[', 'x']).nth(1).unwrap().parse::<u16>().unwrap()
    }
    let (code, _, err) = h.cli(&["resize-pane", "-t", "j:0.0", "-x", "30"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(width(&h).await, 30);
    h.cli(&["resize-pane", "-t", "j:0.0", "-x", "50%"]).await;
    assert_eq!(width(&h).await, 40, "half of 80 columns");
    let (code, _, err) = h.cli(&["resize-pane", "-t", "j:0.0", "-x", "nonsense"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("bad size"), "{err}");

    // A pane can be given a title, which the format strings pick up. The
    // program's own title comes first: one that arrives after -T replaces
    // it, as in tmux (on a slow CI runner cmd's startup title came late and
    // did exactly that, so wait for it; sh sets none).
    let deadline = Instant::now() + Duration::from_secs(10);
    while cfg!(windows) {
        let (_, out, _) = h.cli(&["list-panes", "-t", "j:0"]).await;
        if out.lines().next().is_some_and(|l| l.to_lowercase().contains("cmd.exe")) {
            break;
        }
        assert!(Instant::now() < deadline, "cmd never set its title: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["select-pane", "-t", "j:0.0", "-T", "logs"]).await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "j:0"]).await;
    assert!(out.lines().next().unwrap().contains("logs"), "{out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn window_order_formats_and_short_names() {
    let h = Harness::start("order").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "w", "-n", "one"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["new-window", "-d", "-t", "w", "-n", "two"]).await;
    h.cli(&["new-window", "-d", "-t", "w", "-n", "three"]).await;
    let names = || async {
        let (_, out, _) = h.cli(&["list-windows", "-t", "w"]).await;
        // "0: one* (1 panes) [80x24]" -> "one"
        out.lines()
            .map(|l| {
                let after = l.split_once(": ").unwrap().1;
                let word = after.split_whitespace().next().unwrap_or("");
                word.trim_end_matches(['*', '-']).to_string()
            })
            .collect::<Vec<String>>()
    };
    assert_eq!(names().await, ["one", "two", "three"]);

    // swap exchanges two windows, move takes one out and re-inserts it.
    let (code, _, err) = h.cli(&["swap-window", "-s", "w:0", "-t", "w:2"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(names().await, ["three", "two", "one"]);
    let (code, _, err) = h.cli(&["move-window", "-s", "w:2", "-t", "w:0"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(names().await, ["one", "three", "two"]);
    // Short forms of the command names work as in tmux.
    let (code, out, _) = h.cli(&["lsw", "-t", "w"]).await;
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 3);
    let (code, _, err) = h.cli(&["kill"]).await;
    assert_eq!(code, 1);
    assert!(err.starts_with("ambiguous command: kill"), "{err}");

    // `set -a` appends, and formats understand conditionals everywhere.
    h.cli(&["set", "-g", "status-right", "A"]).await;
    h.cli(&["set", "-ag", "status-right", "B"]).await;
    let (_, out, _) = h.cli(&["show-options", "-gv", "status-right"]).await;
    assert_eq!(out.trim(), "AB");
    let (_, out, _) = h.cli(&["display-message", "-p", "#{?window_flags,busy,idle}|#{?session_name==w,yes,no}"]).await;
    assert_eq!(out.trim(), "busy|yes");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn layouts_and_pane_numbers() {
    let h = Harness::start("layout").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "g"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    for _ in 0..3 {
        h.cli(&["split-window", "-h", "-t", "g"]).await;
    }
    let widths = || async {
        let (_, out, _) = h.cli(&["list-panes", "-t", "g"]).await;
        out.lines().map(|l| l.split(['[', 'x']).nth(1).unwrap().parse::<u16>().unwrap()).collect::<Vec<u16>>()
    };

    // prefix Space cycles: the first layout is even-horizontal, four columns.
    c.prefix(' ').await;
    c.wait_for("even-horizontal", |s| s.contents().contains("even-horizontal")).await;
    let w = widths().await;
    assert_eq!(w.len(), 4);
    assert!(w.iter().all(|x| (19..=20).contains(x)), "four equal columns: {w:?}");
    // Again: even-vertical, so every pane is full width.
    c.prefix(' ').await;
    c.wait_for("even-vertical", |s| s.contents().contains("even-vertical")).await;
    assert!(widths().await.iter().all(|x| *x == COLS), "full width rows");
    // By name, with a prefix of the name and a target.
    let (code, _, err) = h.cli(&["select-layout", "-t", "g", "til"]).await;
    assert_eq!(code, 0, "{err}");
    let w = widths().await;
    assert!(w.iter().all(|x| (39..=40).contains(x)), "a 2x2 grid: {w:?}");
    let (code, _, err) = h.cli(&["select-layout", "-t", "g", "nope"]).await;
    assert_eq!(code, 1);
    assert!(err.starts_with("unknown layout: nope"), "{err}");

    // prefix q shows the numbers; a digit then picks that pane.
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.key(b'2' as u16, '2', 0).await;
    h.wait_list("g", "pane 2 active", |out| out.lines().nth(2).is_some_and(|l| l.contains("(active)"))).await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn display_panes_takes_numbers_past_nine() {
    let h = Harness::start("panes12").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "g"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    for _ in 0..11 {
        let (code, _, err) = h.cli(&["split-window", "-t", "g"]).await;
        assert_eq!(code, 0, "{err}");
        h.cli(&["select-layout", "-t", "g", "tiled"]).await;
    }
    assert_eq!(h.cli(&["list-panes", "-t", "g"]).await.1.lines().count(), 12);
    let active = |n: usize| move |out: &str| out.lines().nth(n).is_some_and(|l| l.contains("(active)"));
    // Each case: show the numbers, type, then the pane that should be active.
    for (keys, pane) in [("10", 10), ("11", 11), ("2", 2)] {
        c.prefix('q').await;
        c.wait_for("numbers", |s| s.contents().contains("███")).await;
        c.type_str(keys).await;
        h.wait_list("g", &format!("{keys}: pane {pane}"), active(pane)).await;
        c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    }
    // A digit goes there at once, even when 10 and 11 are still possible;
    // the numbers stay up a moment for a second digit, then go by themselves.
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.type_str("1").await;
    let t = Instant::now();
    h.wait_list("g", "pane 1 at once", active(1)).await;
    let took = t.elapsed();
    assert!(took < Duration::from_millis(700), "waited before going: {took:?}");
    assert!(c.text().contains("███"), "the numbers went before a second digit could come");
    c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    // A first digit late in the numbers' time still leaves time for the
    // second: they stay up a second from the digit, not only display-time
    // (1.5s) from prefix q. The margins are a few hundred ms either side, so
    // a loaded machine may miss them: up to three tries (without the second
    // from the digit, every try misses).
    let mut late = String::new();
    for _ in 0..3 {
        c.prefix('q').await;
        c.wait_for("numbers", |s| s.contents().contains("███")).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        c.type_str("1").await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        c.type_str("0").await;
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            late = h.cli(&["list-panes", "-t", "g"]).await.1;
            if active(10)(&late) || Instant::now() > end {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
        if active(10)(&late) {
            break;
        }
    }
    assert!(active(10)(&late), "late 1 0 did not reach pane 10 in three tries:\n{late}");
    // From pane 2, 1 1 passes pane 1 on its way to 11: `last-pane` goes
    // back to 2, where it began.
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.type_str("2").await;
    h.wait_list("g", "pane 2", active(2)).await;
    c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.type_str("11").await;
    h.wait_list("g", "pane 11", active(11)).await;
    c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    c.prefix(';').await;
    h.wait_list("g", "back to pane 2", active(2)).await;
    // pane-base-index 5: the panes are 5..16, so 1 is no pane, yet 1 3 is 13;
    // 1 alone goes nowhere.
    h.cli(&["set", "-g", "pane-base-index", "5"]).await;
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.type_str("13").await;
    h.wait_list("g", "pane 13 (base 5)", active(13 - 5)).await;
    c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    c.prefix('q').await;
    c.wait_for("numbers", |s| s.contents().contains("███")).await;
    c.type_str("1").await;
    c.wait_for("numbers gone", |s| !s.contents().contains("███")).await;
    assert!(active(13 - 5)(&h.cli(&["list-panes", "-t", "g"]).await.1), "1 went somewhere");
    h.cli(&["kill-server"]).await;
}

/// `trace-message -J` and `show-task -J` (the dashboard's form): the record
/// as JSON, the text and what the shell printed whole and apart, the steps'
/// times and how it ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_and_its_task_as_json() {
    let h = Harness::start("tracej").await;
    let (code, _, err) = h.cli(&[&["new", "-d", "-s", "j"], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let p = pane_id(&h, "j:0.0").await;
    let t = format!("%{p}");
    h.cli(&["rename-pane", "-t", &t, "runner"]).await;
    h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let cmd = if cfg!(windows) { "Write-Output kpJ1 kpJ2" } else { "printf '%s\\n' kpJ1 kpJ2" };
    let (code, said, err) = h.cli(&["send-message", "-t", &t, cmd]).await;
    assert_eq!(code, 0, "{err}");
    let id: u64 = said.trim_start_matches('#').split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let v = loop {
        let (code, out, err) = h.cli(&["trace-message", &id.to_string(), "-J"]).await;
        assert_eq!(code, 0, "{err}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
        if v["stage"] == "done" {
            break v;
        }
        assert!(Instant::now() < deadline, "never done: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!((v["id"].as_u64(), v["task"].as_u64(), v["hop"].as_u64()), (Some(id), Some(id), Some(0)), "{v}");
    assert_eq!((v["from"].as_str(), v["via"].as_str(), v["text"].as_str()), (Some("user"), Some("shell"), Some(cmd)));
    assert!(v["to"].as_str().is_some_and(|to| to.ends_with(&t)), "{v}");
    assert_eq!(
        v["output"].as_str().map(|o| o.lines().map(str::trim).collect::<Vec<_>>()),
        Some(vec!["kpJ1", "kpJ2"]),
        "{v}"
    );
    assert_eq!((v["ok"].as_bool(), v["cut"].as_bool(), v["read"].as_bool()), (Some(true), Some(false), Some(false)));
    for k in ["sent", "delivered", "ended", "queued", "took"] {
        assert!(v[k].as_str().is_some_and(|s| !s.is_empty()), "{k}: {v}");
    }
    assert!(v["why"].is_null() && v["re"].is_null(), "{v}");
    // The task: its one step, the same record.
    let (code, out, err) = h.cli(&["show-task", &id.to_string(), "-J"]).await;
    assert_eq!(code, 0, "{err}");
    let steps: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(steps.as_array().map(Vec::len), Some(1), "{steps}");
    assert_eq!(steps[0], v);
    // Unknown: an error, as without -J.
    assert_ne!(h.cli(&["trace-message", "999", "-J"]).await.0, 0);
    assert_ne!(h.cli(&["show-task", "999", "-J"]).await.0, 0);
    h.cli(&["kill-server"]).await;
}

/// `copy-output` (prefix y) takes what the last command printed, and copy
/// mode's `[` / `]` step from one command to the next.
#[tokio::test(flavor = "multi_thread")]
async fn a_commands_output_is_copied_and_copy_mode_steps_between_commands() {
    let h = Harness::start("copyout").await;
    let mut c = h.connect().await;
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    c.attach(&[&["new", "-s", "o", "-c", root], HOOKED_SHELL].concat()).await;
    let p = pane_id(&h, "o:0.0").await;
    let t = format!("%{p}");
    // Idle means at its prompt for a pane in shell mode: ready for keys.
    h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    // Nothing has run yet.
    let (code, _, err) = h.cli(&["copy-output", "-p", "-t", &t]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no finished command"), "{err}");
    let (first, second) = if cfg!(windows) {
        ("Write-Output first-1 first-2 first-3", "Write-Output second-1 second-2")
    } else {
        ("printf '%s\\n' first-1 first-2 first-3", "printf '%s\\n' second-1 second-2")
    };
    for (cmd, want) in [(first, "first-1\nfirst-2\nfirst-3"), (second, "second-1\nsecond-2")] {
        h.cli(&["send-keys", "-t", &t, cmd, "Enter"]).await;
        // The command is done once its shell says so: its mark is finished.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (code, out, _) = h.cli(&["copy-output", "-p", "-t", &t]).await;
            if code == 0 && out.trim_end() == want {
                break;
            }
            assert!(Instant::now() < deadline, "{cmd}: {out:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    // Each step below changes the buffer, so waiting for the new text is
    // waiting for the step (a status-line "copied" may be the last one's).
    let buffer_is = async |want: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = h.cli(&["show-buffer"]).await.1;
            if out.trim_end() == want {
                return;
            }
            assert!(Instant::now() < deadline, "buffer {out:?}, waiting for {want:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    // prefix y: the same into a paste buffer, said on the status line.
    c.prefix('y').await;
    c.wait_for("copied", |s| s.contents().contains("copied 2 lines")).await;
    buffer_is("second-1\nsecond-2").await;
    // Copy mode: [ goes to the last command where it was typed; selecting
    // to the end of that line copies the command.
    let copy_line = async |c: &mut Conn, keys: &str| {
        c.prefix('[').await;
        c.type_str(keys).await;
        c.type_str("v$").await;
        c.enter().await;
    };
    copy_line(&mut c, "[").await;
    buffer_is(second).await;
    // [ [ is the one before; [ [ ] the last again.
    copy_line(&mut c, "[[").await;
    buffer_is(first).await;
    copy_line(&mut c, "[[]").await;
    buffer_is(second).await;
    // tmux's names for them, from a script.
    c.prefix('[').await;
    for name in ["previous-prompt", "previous-prompt", "begin-selection", "end-of-line", "copy-selection"] {
        let (code, _, err) = h.cli(&["send-keys", "-X", "-t", &t, name]).await;
        assert_eq!(code, 0, "{name}: {err}");
    }
    buffer_is(first).await;
    // Past the last there is nothing: it says so and stays.
    c.prefix('[').await;
    c.type_str("]").await;
    c.wait_for("no later", |s| s.contents().contains("no later command")).await;
    c.type_str("q").await;
    h.cli(&["kill-server"]).await;
}

/// `theme`: tokyo-day draws the panes light (their default colours and the
/// palette's first 16) whatever the terminal's are; tokyo-night leaves them
/// to the terminal. The phone's page reads it and sets it at `/api/theme`.
#[tokio::test(flavor = "multi_thread")]
async fn a_theme_draws_the_panes_and_the_page_can_set_it() {
    let h = Harness::start("theme").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "t"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // In red: palette colour 1, which a theme may redraw.
    c.type_str(&red_prompt("red")).await;
    c.enter().await;
    c.wait_for("red prompt", |s| s.contents().contains("red>")).await;
    let red_cell = |s: &vt100::Screen| {
        (0..ROWS).find_map(|r| {
            let row = s.rows(0, COLS).nth(r as usize)?;
            let col = row.rfind("red>")?;
            Some(s.cell(r, col as u16)?.fgcolor())
        })
    };
    let bg_at = |s: &vt100::Screen| s.cell(0, COLS - 1).map(|c| c.bgcolor());
    use vt100::Color;
    assert_eq!(bg_at(c.screen.screen()), Some(Color::Default), "tokyo-night leaves the panes to the terminal");
    assert_eq!(red_cell(c.screen.screen()), Some(Color::Idx(1)));
    let (code, _, err) = h.cli(&["set", "-g", "theme", "tokyo-day"]).await;
    assert_eq!(code, 0, "{err}");
    c.wait_for("light panes", |s| bg_at(s) == Some(Color::Rgb(0xe1, 0xe2, 0xe7))).await;
    c.wait_for("the day's red", |s| red_cell(s) == Some(Color::Rgb(0xb2, 0x1e, 0x49))).await;
    assert_eq!(h.cli(&["show", "-gv", "theme"]).await.1.trim(), "tokyo-day");
    // The page: what it is, set from there, refused when read-only.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, "k", false))));
    let (code, body) = http(addr, "GET", "/api/theme", "k", "").await;
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (v["name"].as_str(), v["bg"].as_str(), v["fg"].as_str()),
        (Some("tokyo-day"), Some("#e1e2e7"), Some("#3358b0"))
    );
    assert_eq!(v["palette"][1].as_str(), Some("#b21e49"));
    assert_eq!(v["names"], serde_json::json!(["tokyo-night", "tokyo-day"]));
    let (code, body) = http(addr, "POST", "/api/theme?name=solarized", "k", "").await;
    assert_eq!(code, 400, "{body}");
    let (code, body) = http(addr, "POST", "/api/theme?name=tokyo-night", "k", "").await;
    assert_eq!(code, 200, "{body}");
    c.wait_for("the terminal's own again", |s| bg_at(s) == Some(Color::Default) && red_cell(s) == Some(Color::Idx(1)))
        .await;
    let (_, body) = http(addr, "GET", "/api/theme", "k", "").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Left to the terminal: the page draws in Tokyo Night's own.
    assert_eq!(
        (v["name"].as_str(), v["bg"].as_str(), v["palette"][1].as_str()),
        (Some("tokyo-night"), Some("#1a1b26"), Some("#f7768e"))
    );
    let ro = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ro_addr = ro.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(ro, std::sync::Arc::new(keepane::web::State::new(&h.socket, "k", true))));
    let (code, _) = http(ro_addr, "POST", "/api/theme?name=tokyo-day", "k", "").await;
    assert_eq!(code, 403);
    assert_eq!(h.cli(&["show", "-gv", "theme"]).await.1.trim(), "tokyo-night");
    h.cli(&["kill-server"]).await;
}

/// `shell-history` reads a pane's own history back: commands and the
/// messages delivered as commands (marked), only the commands (`-c`), or
/// every message sent to the pane, from the event log (`-m`).
#[tokio::test(flavor = "multi_thread")]
async fn shell_history_shows_commands_messages_or_both() {
    let h = Harness::start("shellhist").await;
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let (code, _, err) = h.cli(&[&["new", "-d", "-s", "s", "-c", root], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let p = pane_id(&h, "s:0.0").await;
    let t = format!("%{p}");
    at_hooked_prompt(&h, &t).await;
    let say = if cfg!(windows) { "Write-Output" } else { "echo" };
    let typed = format!("{say} typed-{}", std::process::id());
    let sent = format!("{say} sent-{}", std::process::id());
    h.cli(&["send-keys", "-t", &t, &typed, "Enter"]).await;
    // Typed first, so it is first in the history: wait for it to be there.
    let history = async |flags: &[&str]| h.cli(&[&["shell-history", "-t", &t], flags].concat()).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !history(&[]).await.1.contains(&typed) {
        assert!(Instant::now() < deadline, "the typed command never reached the history");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (code, out, err) = h.cli(&["send-message", "--to", &t, &sent]).await;
    assert_eq!(code, 0, "{err}");
    let id: u32 =
        out.split_whitespace().find_map(|w| w.strip_prefix('#')?.parse().ok()).unwrap_or_else(|| panic!("{out}"));
    let marked = format!("✉ #{id} user  {sent}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, out, _) = history(&[]).await;
        if out.contains(&marked) {
            break;
        }
        assert!(Instant::now() < deadline, "the message never reached the history: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A command after the message, to see the numbers stay with -c.
    let after = format!("{say} after-{}", std::process::id());
    h.cli(&["send-keys", "-t", &t, &after, "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    let both = loop {
        let (_, out, _) = history(&[]).await;
        if out.contains(&after) {
            break out;
        }
        assert!(Instant::now() < deadline, "{out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    // Both, in order, numbered by their place in the history.
    let lines: Vec<&str> = both.lines().collect();
    let at = |s: &str| lines.iter().position(|l| l.ends_with(s)).unwrap_or_else(|| panic!("{s} in {both}"));
    assert!(at(&typed) < at(&marked) && at(&marked) < at(&after), "{both}");
    let number = |l: &str| l.split_whitespace().next().unwrap().parse::<usize>().unwrap();
    assert_eq!(number(lines[at(&marked)]), number(lines[at(&typed)]) + 1, "{both}");
    assert_eq!(number(lines[at(&after)]), number(lines[at(&marked)]) + 1, "{both}");
    // Only the commands: the message's line is not there, and the command
    // after it keeps its number.
    let (_, only, _) = history(&["-c"]).await;
    assert!(!only.contains('✉'), "{only}");
    assert!(only.lines().any(|l| l == lines[at(&typed)]) && only.lines().any(|l| l == lines[at(&after)]), "{only}");
    // -n: the last so many.
    let (_, last, _) = history(&["-n", "1"]).await;
    assert_eq!(last.trim_end(), lines.last().unwrap().trim_end());
    // Only the messages to this pane: one to another pane is not among them.
    h.cli(&["new-window", "-d", "-t", "s"]).await;
    h.cli(&["set-work-mode", "-t", "s:1", "ai"]).await;
    let (code, _, err) = h.cli(&["send-message", "--to", "s:1", "for-the-other-pane"]).await;
    assert_eq!(code, 0, "{err}");
    // From the event log, with how each went.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, msgs, _) = history(&["-m"]).await;
        let line = msgs.lines().find(|l| l.contains(&format!("#{id} ")));
        if line.is_some_and(|l| l.contains("user → shell") && l.contains(" done ") && l.ends_with(&sent)) {
            assert_eq!(msgs.lines().count(), 1, "{msgs}");
            break;
        }
        assert!(Instant::now() < deadline, "{msgs}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Not both at once; a pane with no shell history says what to use.
    let (code, _, err) = history(&["-c", "-m"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("not both"), "{err}");
    let (code, _, err) = h.cli(&["shell-history", "-t", "s:1"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("keeps no shell history") && err.contains("-m"), "{err}");
    // That pane's own messages are its own.
    let (code, out, _) = h.cli(&["shell-history", "-m", "-t", "s:1"]).await;
    assert_eq!(code, 0);
    assert!(out.lines().count() == 1 && out.trim_end().ends_with("for-the-other-pane"), "{out}");
    h.cli(&["kill-server"]).await;
}

/// `hints` (prefix F) labels paths, addresses and hashes; a label copies,
/// in capitals it opens (`hint-open` here, to see what it was given).
#[tokio::test(flavor = "multi_thread")]
async fn hints_copy_or_open_what_is_on_screen() {
    let h = Harness::start("hints").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "h"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let line = "src/server/mod.rs:6454:13 https://example.com/a af9af7e";
    c.type_str(&format!("echo {line}")).await;
    c.enter().await;
    c.wait_for("printed", |s| s.rows(0, COLS).any(|r| r.trim_end() == line)).await;
    // Labelled from the bottom up, one label per text: the hash a, the
    // address s, the path d, each over the start of its thing.
    let labelled = |s: &vt100::Screen| {
        s.rows(0, COLS).any(|r| r.trim_end() == "drc/server/mod.rs:6454:13 sttps://example.com/a af9af7e")
    };
    // Escape puts them away.
    c.prefix('F').await;
    c.wait_for("labels", labelled).await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("labels gone", |s| s.rows(0, COLS).any(|r| r.trim_end() == line)).await;
    // A label copies its thing (the path without its line and column); a
    // letter no label has before it is ignored, the labels staying up.
    c.prefix('F').await;
    c.wait_for("labels", labelled).await;
    c.type_str("zd").await;
    c.wait_for("copied", |s| s.contents().contains("copied src/server/mod.rs")).await;
    assert_eq!(h.cli(&["show-buffer"]).await.1.trim_end(), "src/server/mod.rs");
    // In capitals it opens: the path from the pane's directory, its line
    // and column, handed to hint-open.
    let (code, _, err) = h.cli(&["set", "-g", "hint-open", "set-buffer -b opened '{file}|{line}|{col}'"]).await;
    assert_eq!(code, 0, "{err}");
    c.prefix('F').await;
    c.wait_for("labels", labelled).await;
    c.key(b'D' as u16, 'D', SHIFT_PRESSED).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let opened = loop {
        let (code, out, _) = h.cli(&["show-buffer", "-b", "opened"]).await;
        if code == 0 {
            break out.trim_end().to_string();
        }
        assert!(Instant::now() < deadline, "hint-open never ran");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let (file, rest) = opened.split_once('|').unwrap();
    assert_eq!(rest, "6454|13");
    assert!(
        std::path::Path::new(file).is_absolute() && file.replace('\\', "/").ends_with("src/server/mod.rs"),
        "{file}"
    );
    // A hash has nothing to open: in capitals it is copied all the same.
    c.prefix('F').await;
    c.wait_for("labels", labelled).await;
    c.key(b'A' as u16, 'A', SHIFT_PRESSED).await;
    c.wait_for("copied", |s| s.contents().contains("copied af9af7e")).await;
    assert_eq!(h.cli(&["show-buffer"]).await.1.trim_end(), "af9af7e");
    // Not over a list, where the labels could not be seen.
    c.prefix('w').await;
    c.wait_for("list", |s| s.contents().contains("q quit")).await;
    c.prefix('F').await;
    c.wait_for("refused", |s| s.contents().contains("close the list")).await;
    c.type_str("q").await;
    // From a script there is no screen to label.
    let (code, _, err) = h.cli(&["hints"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("not attached"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_mode_search_finds_scrolled_off_lines() {
    let h = Harness::start("search").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "f"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // More output than fits, so the early lines are only in the scrollback.
    c.type_str(&count_to(60, "marker-")).await;
    c.enter().await;
    c.wait_for("output", |s| s.contents().contains("marker-60")).await;
    assert!(!c.text().contains("marker-3 "), "line 3 has scrolled away: {}", c.text());

    // prefix [ enters copy mode; ? searches back through the scrollback, as
    // tmux does (/ looks the other way, towards the newest line).
    c.prefix('[').await;
    c.type_str("?marker-3 ").await;
    c.enter().await;
    c.wait_for("found", |s| s.contents().contains("marker-3 ")).await;

    // n repeats the search further back; N turns around.
    c.type_str("n").await;
    c.wait_for("earlier hit", |s| s.contents().contains("marker-3 ")).await;
    c.type_str("?nothing-like-this").await;
    c.enter().await;
    c.wait_for("miss reported", |s| s.contents().contains("no match: nothing-like-this")).await;
    // Escape leaves copy mode; the pane is still usable.
    c.key(0x1B, '\x1b', 0).await;
    c.key(0x1B, '\x1b', 0).await;
    c.type_str("echo after-search").await;
    c.enter().await;
    let pane = h.wait_capture("f", "the echo after copy mode", |t| t.contains("after-search")).await;
    assert!(pane.contains("after-search"), "{pane}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_base_index_shifts_every_pane_number() {
    let h = Harness::start("panebase").await;
    h.cli(&["set", "-g", "pane-base-index", "1"]).await;
    h.cli(&["set", "-g", "base-index", "1"]).await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "b"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["split-window", "-h", "-t", "b"]).await;
    h.cli(&["split-window", "-v", "-t", "b"]).await;

    // Listed, targeted and formatted with the same numbers.
    let (_, out, _) = h.cli(&["list-panes", "-t", "b"]).await;
    let nums: Vec<&str> = out.lines().map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(nums, ["1", "2", "3"], "{out}");
    let (code, _, err) = h.cli(&["send-keys", "-t", "b:1.1", "echo first-pane", "Enter"]).await;
    assert_eq!(code, 0, "{err}");
    let text = h.wait_capture("b:1.1", "the echo", |t| t.contains("first-pane")).await;
    assert!(text.contains("first-pane"), "{text}");
    // Pane 0 no longer exists when the base is 1.
    let (code, _, err) = h.cli(&["send-keys", "-t", "b:1.0", "x"]).await;
    assert_eq!(code, 1, "pane 0 should be gone");
    assert_eq!(err, "no pane 0");
    let (_, out, _) = h.cli(&["display-message", "-p", "#P"]).await;
    assert_eq!(out.trim(), "3", "#P follows the base too: {out}");
    h.cli(&["kill-server"]).await;
}

/// `prefix-hint`: when the key after the prefix is late, a panel says what
/// each key does; the key then does it as ever and the panel goes. A key in
/// time never shows it, and `prefix-hint off` never does.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_key_after_the_prefix_gets_a_panel() {
    let h = Harness::start("keyhint").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "k"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let panel = |s: &vt100::Screen| s.contents().contains("split side by side");
    // The prefix alone: after half a second, the panel, with the prefix on it.
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.wait_for("the panel", |s| panel(s) && s.contents().contains("C-b") && s.contents().contains("? every key")).await;
    // Its key does what it always did, and the panel goes.
    c.type_str("c").await;
    c.wait_for("the panel gone", |s| !panel(s)).await;
    h.wait_for_cli("a second window", &["list-windows", "-t", "k"], |_, out| out.lines().count() == 2).await;

    // On time: never before the delay, and two of three soon after it. The
    // server's second-long tick would draw it too, but only next to a tick:
    // three tries a third of a second apart cannot have two there, and one
    // slow frame under load is not a late panel.
    h.cli(&["set", "-g", "prefix-hint-delay", "200"]).await;
    let mut took = Vec::new();
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(333)).await;
        let t = Instant::now();
        c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
        c.wait_for("the panel", panel).await;
        took.push(t.elapsed());
        c.type_str("r").await;
        c.wait_for("the panel gone", |s| !panel(s)).await;
    }
    assert!(took.iter().all(|t| *t >= Duration::from_millis(180)), "a panel before the delay: {took:?}");
    let on_time = took.iter().filter(|t| **t < Duration::from_millis(360)).count();
    assert!(on_time >= 2, "the panel late: {took:?}");

    // A key in time: no panel, not even later. What the screen shows once a
    // change made after the delay arrived has every frame before it.
    let seen = |c: &Conn| panel(c.screen.screen());
    h.cli(&["set", "-g", "prefix-hint-delay", "300"]).await;
    c.prefix('p').await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    h.cli(&["rename-window", "-t", "k:1", "after-in-time"]).await;
    c.wait_for("the new name", |s| s.contents().contains("after-in-time")).await;
    assert!(!seen(&c), "a key in time showed the panel:\n{}", c.text());

    // Off: the prefix waits, and no panel comes.
    h.cli(&["set", "-g", "prefix-hint", "off"]).await;
    h.cli(&["set", "-g", "prefix-hint-delay", "0"]).await;
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    h.cli(&["rename-window", "-t", "k:1", "after-off"]).await;
    c.wait_for("the new name", |s| s.contents().contains("after-off")).await;
    assert!(!seen(&c), "off showed the panel:\n{}", c.text());
    // The prefix still waited: its key works.
    c.type_str("n").await;
    h.wait_for_cli("the next window after the prefix", &["list-windows", "-t", "k"], |_, out| {
        out.lines().any(|l| l.starts_with("1:") && l.contains("*"))
    })
    .await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repeatable_keys_chain_without_the_prefix() {
    let h = Harness::start("repeat").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "r"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Three panes side by side: 0 | 1 | 2, with 2 active.
    c.prefix('%').await;
    c.wait_for("split", |s| s.contents().matches("keepane>").count() >= 2).await;
    c.prefix('%').await;
    c.wait_for("split again", |s| s.contents().matches("keepane>").count() >= 3).await;
    let active = |out: &str| out.lines().position(|l| l.contains("(active)")).unwrap();
    let (_, out, _) = h.cli(&["list-panes", "-t", "r"]).await;
    assert_eq!(active(&out), 2, "{out}");

    // prefix h, then a bare h: two panes left in one go.
    c.prefix('h').await;
    c.type_str("h").await;
    h.wait_list("r", "bare h repeated the binding", |out| active(out) == 0).await;

    // The window closes: after repeat-time a bare h is just text again.
    let (code, _, err) = h.cli(&["set", "-g", "repeat-time", "150"]).await;
    assert_eq!(code, 0, "{err}");
    c.prefix('l').await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    c.type_str("hhh").await;
    c.enter().await;
    // Typed text in the pane: the keys were all handled by then.
    let pane = h.wait_capture("r:0.1", "the typed text", |t| t.contains("hhh")).await;
    assert!(pane.contains("hhh"), "{pane}");
    let (_, out, _) = h.cli(&["list-panes", "-t", "r"]).await;
    assert_eq!(active(&out), 1, "the late h's must not move the pane: {out}");

    // repeat-time 0 turns it off entirely.
    h.cli(&["set", "-g", "repeat-time", "0"]).await;
    c.prefix('h').await;
    c.type_str("h").await;
    h.wait_list("r", "the prefixed h still moves", |out| active(out) == 0).await;
    let pane = h.wait_capture("r:0.0", "the second h as text", |t| t.contains("h")).await;
    assert!(pane.contains("h"), "{pane}");
    h.cli(&["kill-server"]).await;
}

/// `animation`: a frame flies to the pane the keys now go to (selecting a
/// pane, zooming, switching windows), then leaves the screen as it was; off,
/// there is none.
#[tokio::test(flavor = "multi_thread")]
async fn a_focus_frame_flies_to_where_the_keys_go() {
    let h = Harness::start("anim").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "a"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Slow enough to be caught between two looks at the screen.
    h.cli(&["set", "-g", "animation-time", "1500"]).await;
    let corners = |s: &vt100::Screen| s.contents().matches(['╭', '╮', '╰', '╯']).count();
    c.prefix('%').await;
    c.wait_for("split", |s| s.contents().matches("keepane>").count() >= 2).await;
    // It moves: frame after frame while it lasts, not one now and then.
    c.prefix('h').await;
    let (t0, mut frames) = (Instant::now(), 0);
    while t0.elapsed() < Duration::from_millis(1000) {
        if let Ok(Ok(Some(ServerMsg::Output(b)))) =
            tokio::time::timeout(Duration::from_millis(100), read_frame::<_, ServerMsg>(&mut c.rd)).await
        {
            c.screen.process(&b);
            frames += 1;
        }
    }
    // Several frames, not one jump: how many a busy machine (the whole
    // suite at once) draws is not what this is about.
    assert!(frames >= 4, "a moving frame, {frames} frames in a second");
    c.wait_for("gone again", |s| corners(s) == 0).await;
    c.prefix('l').await;
    c.wait_for("back right", |s| corners(s) == 0).await;
    c.prefix('h').await;
    c.wait_for("select-pane", |s| corners(s) == 4).await;
    c.wait_for("gone again", |s| corners(s) == 0).await;
    // Zooming the left pane: the pane itself grows, its corners heading for
    // the window's, the left edge (on the window's edge) staying put, over
    // the right pane, which shows until it is covered.
    h.cli(&["send-keys", "-t", "a:0.1", "echo right-side-mark", "Enter"]).await;
    c.wait_for("the mark", |s| s.contents().contains("right-side-mark")).await;
    c.prefix('z').await;
    c.wait_for("growing over the right pane", |s| {
        corners(s) == 4 && s.contents().contains("right-side-mark") && s.cell(0, 0).is_some_and(|c| c.contents() == "╭")
    })
    .await;
    c.wait_for("grown", |s| corners(s) == 0 && !s.contents().contains("right-side-mark")).await;
    c.prefix('z').await;
    c.wait_for("unzoom", |s| corners(s) == 4).await;
    c.wait_for("gone again", |s| corners(s) == 0 && s.contents().contains("right-side-mark")).await;
    // Another window: the frame closes in on it.
    c.prefix('c').await;
    c.wait_for("new window", |s| corners(s) == 4).await;
    c.wait_for("gone again", |s| corners(s) == 0).await;
    // Off: nothing drawn, however long one looks.
    h.cli(&["set", "-g", "animation", "off"]).await;
    c.prefix('p').await;
    let t0 = Instant::now();
    let mut frames = 0;
    while t0.elapsed() < Duration::from_millis(600) {
        if let Ok(Ok(Some(ServerMsg::Output(b)))) =
            tokio::time::timeout(Duration::from_millis(50), read_frame::<_, ServerMsg>(&mut c.rd)).await
        {
            c.screen.process(&b);
            frames += 1;
        }
        assert_eq!(corners(c.screen.screen()), 0, "no frame with animation off");
    }
    assert!(frames > 0, "the window did change");
    assert!(c.screen.screen().contents().matches("keepane>").count() >= 2, "back on the split window");
    h.cli(&["kill-server"]).await;
}

/// A zoom in motion when the ground moves under it: border rows above the
/// panes, the terminal resized half way, another pane closed half way, down
/// to a terminal too small for anything. The server goes on answering and
/// ends where it should.
#[tokio::test(flavor = "multi_thread")]
async fn a_zoom_in_motion_survives_what_happens_meanwhile() {
    let h = Harness::start("anim-edge").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "e"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["set", "-g", "animation-time", "2000"]).await;
    h.cli(&["set", "-g", "pane-border-status", "top"]).await;
    h.cli(&["split-window", "-h", "-t", "e"]).await;
    h.cli(&["split-window", "-v", "-t", "e"]).await;
    let corners = |s: &vt100::Screen| s.contents().matches(['╭', '╮', '╰', '╯']).count();
    let zoomed =
        async || h.cli(&["display-message", "-p", "-t", "e", "#{window_zoomed_flag}"]).await.1.trim().to_string();
    // Border rows: the zoom starts and grows as usual.
    c.prefix('z').await;
    c.wait_for("zooming under border rows", |s| corners(s) == 4).await;
    // Resized half way: drawn at the new size, then settled there.
    c.send(ClientMsg::Resize { cols: 60, rows: 16 }).await;
    c.wait_for("settled after the resize", |s| corners(s) == 0).await;
    assert_eq!(zoomed().await, "1");
    // Unzooming, and the pane it grows out of closes half way.
    c.prefix('z').await;
    c.wait_for("unzooming", |s| corners(s) == 4).await;
    let (_, ids, _) = h.cli(&["list-panes", "-t", "e", "-F", "#{pane_id} #{pane_active}"]).await;
    let other = ids.lines().find(|l| l.ends_with(" 0")).unwrap().split(' ').next().unwrap().to_string();
    h.cli(&["set", "-g", "undo-kill-time", "0"]).await;
    assert_eq!(h.cli(&["kill-pane", "-t", &other]).await.0, 0);
    c.wait_for("settled with two panes", |s| corners(s) == 0).await;
    assert_eq!(h.cli(&["list-panes", "-t", "e"]).await.1.lines().count(), 2);
    // A terminal too small for anything, a zoom in motion in it.
    c.send(ClientMsg::Resize { cols: 3, rows: 2 }).await;
    c.prefix('z').await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.send(ClientMsg::Resize { cols: 80, rows: 24 }).await;
    c.wait_for("settled at 80x24", |s| corners(s) == 0 && s.contents().contains("keepane>")).await;
    assert_eq!(h.cli(&["display-message", "-p", "ok"]).await.1.trim(), "ok", "the server still answers");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn zoomed_pane_still_navigates_by_direction() {
    let h = Harness::start("zoom-nav").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "z"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix('%').await; // left | right, the right one active
    c.wait_for("split", |s| s.contents().matches("keepane>").count() >= 2).await;
    c.prefix('z').await; // zoom the right pane
    c.wait_for("Z flag", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}*Z"))).await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "z"]).await;
    assert!(out.lines().nth(1).unwrap().contains("[80x23]"), "zoomed pane fills the window: {out}");
    // The hidden pane keeps running at its own size; it is not 0x0.
    assert!(out.lines().next().unwrap().contains("[40x23]"), "hidden pane keeps its size: {out}");
    h.cli(&["send-keys", "-t", "z:0.0", "echo hidden-alive", "Enter"]).await;
    let hidden = h.wait_capture("z:0.0", "the hidden pane's echo", |t| t.contains("hidden-alive")).await;
    assert!(hidden.contains("hidden-alive"), "{hidden}");

    // The whole point: h moves to the left pane instead of "no such pane",
    // and the zoom goes with it (`keep-zoom`, the default): the left pane
    // now fills the window.
    let status = |s: &vt100::Screen| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap();
    c.prefix('h').await;
    c.wait_for("the left pane, zoomed", |s| {
        status(s).contains(&format!("0:{SH}*Z")) && s.contents().contains("hidden-alive")
    })
    .await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "z"]).await;
    let first = out.lines().next().unwrap();
    assert!(first.contains("(active)") && first.contains("[80x23]"), "left pane active, filling the window: {out}");
    assert!(!c.text().contains("no such pane"), "{}", c.text());

    // display-panes and a number: the same, the zoom follows.
    // Zoomed, every pane still shows its number, where it would be; under
    // it a named pane's name and mode, a pane with no name its mode.
    let right = h.cli(&["list-panes", "-t", "z", "-F", "#{pane_id}"]).await.1.lines().nth(1).unwrap().to_string();
    assert_eq!(h.cli(&["rename-pane", "-t", &right, "tests"]).await.0, 0);
    c.prefix('q').await;
    let blocks = |s: &vt100::Screen, from: u16, to: u16| {
        (0..ROWS - 1)
            .flat_map(|y| (from..to).map(move |x| (y, x)))
            .filter(|&(y, x)| s.cell(y, x).is_some_and(|c| c.contents() == "█"))
            .count()
    };
    c.wait_for("both numbers, the name under the right one", |s| {
        blocks(s, 0, 40) > 0
            && blocks(s, 41, 80) > 0
            && s.rows(41, 39).any(|r| r.trim() == "%tests · normal")
            && s.rows(0, 40).any(|r| r.trim() == "normal")
    })
    .await;
    c.key(b'1' as u16, '1', 0).await;
    c.wait_for("the right pane, zoomed", |s| {
        status(s).contains(&format!("0:{SH}*Z")) && !s.contents().contains("hidden-alive")
    })
    .await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "z"]).await;
    assert!(out.lines().nth(1).unwrap().contains("(active)"), "right pane is active: {out}");
    // Going to a pane from outside (a notification's button, the task
    // board's Enter) does the same, and shows the pane gone to, not the
    // one left.
    let (_, ids, _) = h.cli(&["list-panes", "-t", "z", "-F", "#{pane_id}"]).await;
    let left = ids.lines().next().unwrap().trim_start_matches('%').to_string();
    assert_eq!(h.cli(&["focus-pane", &format!("%{left}")]).await.0, 0);
    c.wait_for("the left pane, zoomed, on screen", |s| {
        status(s).contains(&format!("0:{SH}*Z")) && s.contents().contains("hidden-alive")
    })
    .await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "z"]).await;
    assert!(out.lines().next().unwrap().contains("[80x23]"), "{out}");
    c.prefix('l').await;
    c.wait_for("back right", |s| status(s).contains(&format!("0:{SH}*Z")) && !s.contents().contains("hidden-alive"))
        .await;
    // Only z itself undoes the zoom.
    c.prefix('z').await;
    c.wait_for("unzoomed", |s| status(s).contains(&format!("0:{SH}*")) && !status(s).contains("*Z")).await;

    // keep-zoom off: selecting unzooms, as tmux does.
    h.cli(&["set", "-g", "keep-zoom", "off"]).await;
    c.prefix('z').await;
    c.wait_for("Z flag again", |s| status(s).contains(&format!("0:{SH}*Z"))).await;
    c.prefix('h').await;
    c.wait_for("unzoomed by moving", |s| status(s).contains(&format!("0:{SH}*")) && !status(s).contains("*Z")).await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "z"]).await;
    assert!(out.lines().next().unwrap().contains("(active)"), "left pane is active: {out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_keys_is_reproducible_across_servers() {
    // Two independent servers (different HashMap seeds) must print the key
    // table byte-for-byte identically.
    let a = Harness::start("repro-a").await;
    let b = Harness::start("repro-b").await;
    let (_, ka, _) = a.cli(&["list-keys"]).await;
    let (_, kb, _) = b.cli(&["list-keys"]).await;
    assert_eq!(ka, kb);
    assert!(ka.lines().count() >= 40, "{}", ka.lines().count());
    a.cli(&["kill-server"]).await;
    b.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_protocol_version_is_rejected() {
    let h = Harness::start("proto").await;
    let mut c = h.connect().await;
    c.send(ClientMsg::Command {
        version: 999,
        argv: vec!["ls".into()],
        cwd: String::new(),
        cols: 80,
        rows: 24,
        interactive: false,
        pane_env: None,
    })
    .await;
    match c.next().await {
        ServerMsg::Error(e) => assert!(e.contains("protocol mismatch"), "{e}"),
        other => panic!("{other:?}"),
    }
    // Server without sessions keeps running; kill it explicitly.
    let (code, _, _) = h.cli(&["kill-server"]).await;
    assert_eq!(code, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn send_keys_dash_x_drives_copy_mode() {
    let h = Harness::start("sendx").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "x"]).await;
    assert_eq!(code, 0, "{err}");
    h.wait_capture("x:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "x:0", "echo alpha beta gamma", "Enter"]).await;
    h.wait_capture("x:0", "echo output", |t| t.matches("alpha beta gamma").count() >= 2).await;

    // The copy-mode commands work on the pane named by -t, with no client
    // attached anywhere: search back to the word, select it, copy it.
    for argv in [
        vec!["send-keys", "-t", "x:0", "-X", "search-backward", "alpha"],
        vec!["send-keys", "-t", "x:0", "-X", "begin-selection"],
        vec!["send-keys", "-t", "x:0", "-X", "next-word-end"],
        vec!["send-keys", "-t", "x:0", "-X", "copy-selection"],
    ] {
        let (code, _, err) = h.cli(&argv).await;
        assert_eq!(code, 0, "{argv:?}: {err}");
    }
    let (_, out, _) = h.cli(&["show-buffer"]).await;
    assert_eq!(out.trim_end(), "alpha", "send-keys -X copied the searched word: {out:?}");

    // An unknown copy command is refused instead of being ignored.
    let (code, _, err) = h.cli(&["send-keys", "-t", "x:0", "-X", "fly-to-the-moon"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("unknown command 'fly-to-the-moon'"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remain_on_exit_keeps_the_pane_and_history_survives_resume() {
    let h = Harness::start("remain").await;
    let (code, _, err) = h.cli(&["set", "-g", "remain-on-exit", "on"]).await;
    assert_eq!(code, 0, "{err}");
    // A second session keeps the server alive while "r" is killed below.
    h.cli(&["new", "-d", "-s", "keeper"]).await;
    h.cli(&["new", "-d", "-s", "r"]).await;
    h.wait_capture("r:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "r:0", "echo keepme-42", "Enter"]).await;
    // The output line itself, not the typed command that also says it.
    h.wait_capture("r:0", "output", |t| t.lines().any(|l| l.trim() == "keepme-42")).await;

    // save-history: the pane's text goes into the saved file...
    let (code, _, err) = h.cli(&["save-session", "-t", "r"]).await;
    assert_eq!(code, 0, "{err}");
    let file = std::fs::read_dir(&h.sessions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .find(|t| t.contains("\"name\": \"r\""))
        .expect("a saved file for r");
    assert!(file.contains("keepme-42"), "the pane output is saved: {file}");

    // ...and comes back on the screen when the session is resumed.
    h.cli(&["kill-session", "-t", "r"]).await;
    let (code, _, err) = h.cli(&["resume", "r"]).await;
    assert_eq!(code, 0, "{err}");
    h.wait_capture("r:0", "the restored output", |t| t.contains("keepme-42")).await;
    // The replayed history holds the prompts that were saved; the shell is
    // up when one more appears. Typing before that can be lost while the
    // shell starts.
    let saved_prompts = file.matches("keepane>").count();
    h.wait_capture("r:0", "the restored shell", |t| t.matches("keepane>").count() > saved_prompts).await;

    // The shell exits; with remain-on-exit the pane, the window and the
    // session all stay, and the pane says what happened.
    h.cli(&["send-keys", "-t", "r:0", "exit", "Enter"]).await;
    let pane = h.wait_capture("r:0", "the exit note", |t| t.contains("exited with")).await;
    assert!(pane.contains("keepme-42"), "the output is still there: {pane}");
    let (code, out, _) = h.cli(&["ls"]).await;
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l.starts_with("r: 1 windows")), "the session outlives its shell: {out}");
    let (_, msgs, _) = h.cli(&["show-messages"]).await;
    assert!(msgs.contains("exited with"), "the exit is logged: {msgs}");

    // respawn-pane starts the shell again in the same pane.
    let (code, _, err) = h.cli(&["respawn-pane", "-t", "r:0"]).await;
    assert_eq!(code, 0, "{err}");
    // The old text (prompts included) may still be on the screen: the new
    // shell is up when the last line is a prompt again, not the exit note.
    h.wait_capture("r:0", "a fresh prompt", |t| {
        t.lines().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim() == "keepane>")
    })
    .await;
    // ...with the pane environment a new pane gets, so `keepane` inside it
    // still talks to this server (respawn used to pass set-environment only).
    let line = format!("echo KEEPANE={} PANE={}", var("KEEPANE"), var("KEEPANE_PANE"));
    h.cli(&["send-keys", "-t", "r:0", &line, "Enter"]).await;
    // The typed line still names the variables; the output line has the real values.
    let want = format!("KEEPANE={} PANE=", h.socket);
    h.wait_capture("r:0", "the socket name and pane id from inside", |t| {
        t.lines().any(|l| l.strip_prefix(&want).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())))
    })
    .await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn move_window_between_sessions() {
    let h = Harness::start("movew").await;
    h.cli(&["new", "-d", "-s", "a"]).await;
    h.cli(&["new", "-d", "-s", "b"]).await;
    h.cli(&["new-window", "-d", "-t", "a", "-n", "travels"]).await;
    let (_, out, _) = h.cli(&["list-windows", "-t", "a"]).await;
    assert_eq!(out.lines().count(), 2, "{out}");

    // b is looking at its only window; the arrival must not steal that.
    h.cli(&["new-window", "-t", "b", "-n", "watched"]).await;
    let (_, before, _) = h.cli(&["list-windows", "-t", "b"]).await;
    assert!(before.lines().any(|l| l.contains("watched*")), "{before}");

    let (code, _, err) = h.cli(&["move-window", "-s", "a:1", "-t", "b:0"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, a, _) = h.cli(&["list-windows", "-t", "a"]).await;
    assert_eq!(a.lines().count(), 1, "the window left a: {a}");
    let (_, b, _) = h.cli(&["list-windows", "-t", "b"]).await;
    assert_eq!(b.lines().count(), 3, "and arrived in b: {b}");
    assert!(b.lines().next().unwrap().contains("travels"), "at the index asked for: {b}");
    assert!(b.lines().any(|l| l.contains("watched*")), "b still looks at the same window: {b}");
    assert!(!b.lines().next().unwrap().contains('*'), "the newcomer is not made current: {b}");
    h.cli(&["kill-window", "-t", "b:2"]).await;

    // Moving the last window of a session takes the session with it.
    let (code, _, err) = h.cli(&["move-window", "-s", "a:0", "-t", "b:2"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["ls"]).await;
    assert_eq!(out.lines().count(), 1, "only b is left: {out}");
    assert!(out.starts_with("b: 3 windows"), "{out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_client_detaches_the_one_picked() {
    let h = Harness::start("chooseclient").await;
    let mut a = h.connect().await;
    a.attach(&["new", "-s", "c"]).await;
    a.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let mut b = h.connect().await;
    b.attach(&["attach", "-t", "c"]).await;
    b.wait_for("prompt", |s| s.contents().contains("keepane>")).await;

    a.prefix('D').await;
    a.wait_for("client list", |s| {
        let t = s.contents();
        t.contains("[1/2] j/k move") && t.matches("client-").count() == 2
    })
    .await;
    // The first line is this client; j moves to the other one and Enter
    // detaches it, leaving us attached.
    a.type_str("j").await;
    a.wait_for("second client", |s| s.contents().contains("[2/2]")).await;
    a.enter().await;
    assert_eq!(b.wait_detached().await, "detached");
    a.wait_for("picker gone", |s| !s.contents().contains("j/k move")).await;
    let (_, out, _) = h.cli(&["list-clients"]).await;
    assert_eq!(out.lines().count(), 1, "one client left: {out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pipe_pane_copies_pane_output_into_a_command() {
    let h = Harness::start("pipe").await;
    h.cli(&["new", "-d", "-s", "p"]).await;
    h.wait_capture("p:0", "shell prompt", |t| t.contains("keepane>")).await;
    let out_file = std::env::temp_dir().join(format!("keepane-pipe-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out_file);
    let cmd = if cfg!(windows) {
        format!("$input | Set-Content -Path '{}'", out_file.display())
    } else {
        format!("cat > '{}'", out_file.display())
    };
    let (code, _, err) = h.cli(&["pipe-pane", "-t", "p:0", &cmd]).await;
    assert_eq!(code, 0, "{err}");

    h.cli(&["send-keys", "-t", "p:0", "echo piped-hello", "Enter"]).await;
    h.wait_capture("p:0", "output", |t| t.contains("piped-hello")).await;
    // No command stops the pipe, which closes the command's input and makes
    // it write the file.
    let (code, _, err) = h.cli(&["pipe-pane", "-t", "p:0"]).await;
    assert_eq!(code, 0, "{err}");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if std::fs::read_to_string(&out_file).is_ok_and(|t| t.contains("piped-hello")) {
            break;
        }
        assert!(Instant::now() < deadline, "pipe-pane never wrote {}", out_file.display());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = std::fs::remove_file(&out_file);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_channels_signal_and_lock() {
    let h = Harness::start("waitfor").await;
    // A client waiting on a channel gets its answer when another signals it.
    let mut w = h.connect().await;
    w.command(&["wait-for", "chan"], false).await;
    let (code, _, err) = h.cli(&["wait-for", "-S", "chan"]).await;
    assert_eq!(code, 0, "{err}");
    match w.next().await {
        ServerMsg::Done { code } => assert_eq!(code, 0),
        other => panic!("waiting client: {other:?}"),
    }

    // A signal with nobody waiting is remembered for the next waiter.
    h.cli(&["wait-for", "-S", "later"]).await;
    let (code, _, err) = h.cli(&["wait-for", "later"]).await;
    assert_eq!(code, 0, "a remembered signal returns at once: {err}");

    // Lock, queue behind it, hand it over.
    let (code, _, err) = h.cli(&["wait-for", "-L", "mutex"]).await;
    assert_eq!(code, 0, "{err}");
    let mut l = h.connect().await;
    l.command(&["wait-for", "-L", "mutex"], false).await;
    let (code, _, err) = h.cli(&["wait-for", "-U", "mutex"]).await;
    assert_eq!(code, 0, "{err}");
    match l.next().await {
        ServerMsg::Done { code } => assert_eq!(code, 0),
        other => panic!("locker: {other:?}"),
    }
    // Unlocking a channel nobody holds says so.
    let (code, _, err) = h.cli(&["wait-for", "-U", "free"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("not locked"), "{err}");
    let (code, _, err) = h.cli(&["wait-for", "-L", "-S", "x"]).await;
    assert_eq!(code, 1, "exclusive flags are refused: {err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn display_menu_runs_an_entry_by_key_and_by_enter() {
    let h = Harness::start("menu").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "m"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;

    // prefix > is the pane menu; `h` splits the window horizontally.
    c.prefix('>').await;
    c.wait_for("menu", |s| s.contents().contains("(h) Split horizontally")).await;
    let text = c.text();
    assert!(text.contains("pane 0"), "the title is shown: {text}");
    assert!(text.contains("(x) Kill"), "{text}");
    c.type_str("h").await;
    c.wait_for("two panes", |s| s.contents().matches("keepane>").count() >= 2).await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "m:0"]).await;
    assert_eq!(out.lines().count(), 2, "{out}");

    // G lands on the last entry that can be picked (the separators are
    // skipped), and Enter runs it: kill-pane leaves one pane.
    c.prefix('>').await;
    c.wait_for("menu again", |s| s.contents().contains("(h) Split horizontally")).await;
    c.key(b'G' as u16, 'G', SHIFT_PRESSED).await;
    c.enter().await;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, out, _) = h.cli(&["list-panes", "-t", "m:0"]).await;
        if out.lines().count() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "Enter on the last entry did not kill a pane: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Escape closes a menu without running anything.
    c.prefix('>').await;
    c.wait_for("menu", |s| s.contents().contains("(h) Split horizontally")).await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("menu gone", |s| !s.contents().contains("Split horizontally")).await;
    let (_, out, _) = h.cli(&["list-panes", "-t", "m:0"]).await;
    assert_eq!(out.lines().count(), 1, "{out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn display_popup_takes_the_keys_and_closes_with_its_command() {
    let h = Harness::start("popup").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "pop"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;

    c.prefix(':').await;
    c.type_str(&format!("display-popup -E {}", shell_line("pip"))).await;
    c.enter().await;
    c.wait_for("popup", |s| s.contents().contains("pip>")).await;
    assert!(c.text().contains('┌'), "the popup has a border: {}", c.text());

    // Keys go to the popup's program, not to the pane behind it.
    c.type_str("echo inside-popup").await;
    c.enter().await;
    c.wait_for("popup output", |s| s.contents().contains("inside-popup")).await;
    let pane = h.cli(&["capture-pane", "-p", "-t", "pop:0"]).await.1;
    assert!(!pane.contains("inside-popup"), "the pane behind is untouched: {pane}");

    // -E: the box goes away when the command does.
    c.type_str("exit").await;
    c.enter().await;
    c.wait_for("popup closed", |s| !s.contents().contains("pip>")).await;
    // The pane behind still takes keys afterwards.
    c.type_str("echo after-popup").await;
    c.enter().await;
    h.wait_capture("pop:0", "the echo after the popup", |t| t.contains("after-popup")).await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn alerts_flag_background_windows() {
    let h = Harness::start("alerts").await;
    let (code, _, err) = h.cli(&["set", "-g", "monitor-activity", "on"]).await;
    assert_eq!(code, 0, "{err}");
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "al"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix('c').await;
    c.wait_for("window 1", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("1:{SH}*"))).await;
    h.wait_capture("al:1", "second shell", |t| t.contains("keepane>")).await;

    // Output in the window nobody is looking at raises the activity flag.
    h.cli(&["send-keys", "-t", "al:0", "echo background-noise", "Enter"]).await;
    // The flag shows up in the status line and in list-windows, after the
    // "last window" mark, as tmux orders them.
    c.wait_for("activity flag", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("0:{SH}-#")))
        .await;
    let (_, out, _) = h.cli(&["list-windows", "-t", "al"]).await;
    assert!(out.lines().next().is_some_and(|l| l.contains(&format!("{SH}-#"))), "{out}");

    // prefix M-n goes to the window with the alert; looking at it clears it.
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.key(b'N' as u16, 'n', LEFT_ALT_PRESSED).await;
    c.wait_for("switched and cleared", |s| {
        let status = s.rows(0, COLS).nth(ROWS as usize - 1).unwrap();
        status.contains(&format!("0:{SH}*")) && !status.contains(&format!("0:{SH}#"))
    })
    .await;
    // With no alert left, the key says so instead of moving.
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.key(b'N' as u16, 'n', LEFT_ALT_PRESSED).await;
    c.wait_for("no alert message", |s| s.contents().contains("no window with an alert")).await;

    // The same in a session nobody is attached to: the flag goes up on the
    // background window and comes down when that window becomes current,
    // with no client to do the looking.
    h.cli(&["new", "-d", "-s", "far"]).await;
    h.cli(&["new-window", "-t", "far"]).await;
    h.wait_capture("far:1", "second shell", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "far:0", "echo quiet-noise", "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, out, _) = h.cli(&["list-windows", "-t", "far"]).await;
        if out.lines().next().is_some_and(|l| l.contains('#')) {
            break;
        }
        assert!(Instant::now() < deadline, "no flag in a detached session: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["select-window", "-t", "far:0"]).await;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, out, _) = h.cli(&["list-windows", "-t", "far"]).await;
        if out.lines().next().is_some_and(|l| !l.contains('#')) {
            assert!(out.lines().next().unwrap().contains(&format!("{SH}*")), "{out}");
            break;
        }
        assert!(Instant::now() < deadline, "the current window kept a stale flag: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn spread_layout_and_capture_with_colours() {
    let h = Harness::start("spread").await;
    h.cli(&["new", "-d", "-s", "sp"]).await;
    h.wait_capture("sp:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["split-window", "-h", "-d", "-t", "sp:0"]).await;
    let (code, _, err) = h.cli(&["resize-pane", "-t", "sp:0.0", "-x", "60"]).await;
    assert_eq!(code, 0, "{err}");
    let widths = |out: &str| -> Vec<u16> {
        out.lines()
            .filter_map(|l| l.split('[').nth(1).and_then(|s| s.split('x').next()).and_then(|w| w.parse().ok()))
            .collect()
    };
    let (_, out, _) = h.cli(&["list-panes", "-t", "sp:0"]).await;
    let before = widths(&out);
    assert_eq!(before.len(), 2, "{out}");
    assert!(before[0] > before[1] + 5, "the panes are lopsided to start with: {out}");

    let (code, _, err) = h.cli(&["select-layout", "-E", "-t", "sp:0.0"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["list-panes", "-t", "sp:0"]).await;
    let after = widths(&out);
    assert!(after[0].abs_diff(after[1]) <= 1, "-E evened them out: {out} (was {before:?})");

    // capture-pane -e keeps the colours; without it the text is plain.
    h.cli(&["send-keys", "-t", "sp:0.0", &red_prompt("RED"), "Enter"]).await;
    h.wait_capture("sp:0.0", "the coloured prompt", |t| t.contains("RED>")).await;
    let (_, plain, _) = h.cli(&["capture-pane", "-p", "-t", "sp:0.0"]).await;
    assert!(!plain.contains('\u{1b}'), "plain capture has no escapes: {plain:?}");
    let (_, coloured, _) = h.cli(&["capture-pane", "-p", "-e", "-t", "sp:0.0"]).await;
    assert!(coloured.contains("RED"), "{coloured:?}");
    assert!(coloured.contains("\u{1b}[31m"), "-e keeps the colour: {coloured:?}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn menus_and_popups_survive_degenerate_sizes() {
    let h = Harness::start("degenerate").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "d"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;

    // A menu with nothing that can be picked: Enter does nothing, Escape
    // closes it, and the pane behind is untouched.
    c.prefix(':').await;
    c.type_str("display-menu \"\"").await;
    c.enter().await;
    c.wait_for("separator-only menu", |s| s.contents().contains("j/k move")).await;
    c.enter().await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("menu gone", |s| !s.contents().contains("j/k move")).await;

    // A terminal with no room for a popup says so instead of drawing a
    // broken box.
    c.send(ClientMsg::Resize { cols: 8, rows: 3 }).await;
    c.prefix(':').await;
    c.type_str(&format!("display-popup -E {PROMPT_SHELL}")).await;
    c.enter().await;
    // Eight columns cannot show the message, so read it out of the log.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, msgs, _) = h.cli(&["show-messages"]).await;
        if msgs.contains("no room") {
            break;
        }
        assert!(Instant::now() < deadline, "no complaint about the size: {msgs}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // A menu at that size draws what fits and still closes.
    c.prefix('>').await;
    // Eight columns clip the labels; the picker still works.
    c.wait_for("tiny menu", |s| s.contents().contains("(h) Spli")).await;
    c.key(0x1B, '\x1b', 0).await;
    c.wait_for("tiny menu gone", |s| !s.contents().contains("(h) Spli")).await;

    // Back to a usable size: a popup opens, survives a resize down to
    // nothing, and closing one that is not there is not an error.
    c.send(ClientMsg::Resize { cols: 80, rows: 24 }).await;
    c.prefix(':').await;
    c.type_str(&format!("display-popup -E {}", shell_line("tiny"))).await;
    c.enter().await;
    c.wait_for("popup", |s| s.contents().contains("tiny>")).await;
    c.send(ClientMsg::Resize { cols: 6, rows: 4 }).await;
    c.send(ClientMsg::Resize { cols: 80, rows: 24 }).await;
    c.type_str("echo still-alive").await;
    c.enter().await;
    // Either the popup survived the squeeze (and took the keys) or it was
    // dropped (and the pane took them); both are fine, a panic is not. (A
    // prompt cut by the squeeze stays cut where the pty does not reflow.)
    h.wait_capture("d:0", "the session still works", |t| t.lines().any(|l| l.starts_with("keepan"))).await;

    // -C from a script closes the popup the user is looking at, and doing it
    // again with none open is not an error.
    let (code, _, err) = h.cli(&["display-popup", "-C"]).await;
    assert_eq!(code, 0, "{err}");
    c.wait_for("popup closed from outside", |s| !s.contents().contains("tiny>")).await;
    let (code, _, err) = h.cli(&["display-popup", "-C"]).await;
    assert_eq!(code, 0, "closing nothing is fine: {err}");
    let (code, out, _) = h.cli(&["ls"]).await;
    assert_eq!(code, 0, "the server is still healthy: {out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn degenerate_targets_for_the_new_commands() {
    let h = Harness::start("degen2").await;
    h.cli(&["new", "-d", "-s", "one"]).await;
    h.wait_capture("one:0", "shell prompt", |t| t.contains("keepane>")).await;

    // select-layout -E needs something beside the pane.
    let (code, _, err) = h.cli(&["select-layout", "-E", "-t", "one:0"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no panes beside it"), "{err}");

    // Commands that need a client say so instead of doing half the work.
    for argv in
        [vec!["display-menu", "x", "k", "kill-pane"], vec!["choose-client"], vec!["display-popup", PROMPT_SHELL]]
    {
        let (code, _, err) = h.cli(&argv).await;
        assert_eq!(code, 1, "{argv:?}");
        assert!(err.contains("not attached"), "{argv:?}: {err}");
    }

    // A pipe whose command is gone ends quietly, and output keeps flowing.
    let (code, _, err) = h.cli(&["pipe-pane", "-t", "one:0", "exit"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["send-keys", "-t", "one:0", "echo after-dead-pipe", "Enter"]).await;
    h.wait_capture("one:0", "output after the pipe died", |t| t.contains("after-dead-pipe")).await;
    // Stopping a pipe that is not running is not an error either.
    let (code, _, err) = h.cli(&["pipe-pane", "-t", "one:0"]).await;
    assert_eq!(code, 0, "{err}");
    // -o with nothing running starts one; -o again stops it.
    let drain = if cfg!(windows) { "$input | Out-Null" } else { "cat > /dev/null" };
    h.cli(&["pipe-pane", "-o", "-t", "one:0", drain]).await;
    let (code, _, err) = h.cli(&["pipe-pane", "-o", "-t", "one:0", drain]).await;
    assert_eq!(code, 0, "{err}");

    // capture-pane -e on a pane that has printed nothing is empty, not junk.
    let blank: &[&str] =
        if cfg!(windows) { &["cmd.exe", "/q", "/k", "prompt $h$h$h"] } else { &["/bin/sh", "-c", "PS1= exec /bin/sh"] };
    h.cli(&[&["new-window", "-d", "-t", "one"], blank].concat()).await;
    let (code, out, _) = h.cli(&["capture-pane", "-p", "-e", "-t", "one:1"]).await;
    assert_eq!(code, 0);
    assert!(out.trim().is_empty() || !out.contains("\u{1b}[0m\u{1b}[0m"), "{out:?}");

    // move-window onto itself and to a free index.
    let (code, _, err) = h.cli(&["move-window", "-s", "one:0", "-t", "one:0"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["move-window", "-s", "one:1", "-t", "one:7"]).await;
    assert_eq!(code, 0, "a free index is where it goes: {err}");
    let (_, out, _) = h.cli(&["list-windows", "-t", "one"]).await;
    assert_eq!(out.lines().count(), 2, "{out}");

    // wait-for on a client that goes away leaves nothing behind: the lock
    // can still be taken afterwards.
    {
        let mut gone = h.connect().await;
        gone.command(&["wait-for", "-L", "held"], false).await;
        // Take the lock away from under it, then drop the connection.
        let _ = gone;
    }
    let (code, _, err) = h.cli(&["wait-for", "-L", "held2"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["wait-for", "-U", "held2"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn option_names_take_abbreviations_and_flip() {
    let h = Harness::start("optnames").await;
    h.cli(&["new", "-d", "-s", "o"]).await;
    h.wait_capture("o:0", "shell prompt", |t| t.contains("keepane>")).await;

    // `set sync` is `set synchronize-panes`, and no value flips it.
    assert_eq!(h.cli(&["show", "-gv", "sync"]).await.1.trim(), "off");
    let (code, _, err) = h.cli(&["set", "sync"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(h.cli(&["show", "-gv", "sync"]).await.1.trim(), "on");
    let (_, out, _) = h.cli(&["list-windows", "-t", "o"]).await;
    assert!(out.contains("*S"), "the status flag follows: {out}");
    h.cli(&["set", "sync", "off"]).await;
    assert_eq!(h.cli(&["show", "-gv", "sync"]).await.1.trim(), "off");

    // Each dash-separated word may be shortened too.
    let (code, _, err) = h.cli(&["set", "mon-act", "on"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(h.cli(&["show", "-gv", "monitor-activity"]).await.1.trim(), "on");
    assert_eq!(h.cli(&["show", "-gv", "mon-act"]).await.1.trim(), "on", "show takes them as well");

    // Flipping works for any on/off option, and a number still needs a value.
    let before = h.cli(&["show", "-gv", "mouse"]).await.1.trim().to_string();
    h.cli(&["set", "mouse"]).await;
    assert_ne!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), before);
    h.cli(&["set", "mou"]).await;
    assert_eq!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), before);
    let (code, _, err) = h.cli(&["set", "history-limit"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("bad number"), "{err}");

    // An abbreviation that could mean several things says which.
    let (code, _, err) = h.cli(&["set", "mon", "on"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("ambiguous option: mon") && err.contains("monitor-bell"), "{err}");
    // And one that means nothing is still an unknown option.
    let (code, _, err) = h.cli(&["set", "frobnicate", "on"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("unknown option"), "{err}");

    // Inside a session the `:` prompt takes the same short names.
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "o"]).await;
    c.wait_for("attached", |s| s.contents().contains("keepane>")).await;
    c.prefix(':').await;
    c.type_str("set sync").await;
    c.enter().await;
    c.wait_for("sync on from the prompt", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains("*S")).await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn find_text_looks_through_every_pane() {
    let h = Harness::start("findtext").await;
    h.cli(&["new", "-d", "-s", "ft"]).await;
    h.wait_capture("ft:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["split-window", "-d", "-t", "ft:0"]).await;
    h.wait_capture("ft:0.1", "second shell", |t| t.contains("keepane>")).await;
    h.cli(&["new-window", "-d", "-t", "ft", "-n", "build"]).await;
    h.wait_capture("ft:1", "third shell", |t| t.contains("keepane>")).await;

    h.cli(&["send-keys", "-t", "ft:0.0", "echo REDIS-TIMEOUT-here", "Enter"]).await;
    h.cli(&["send-keys", "-t", "ft:0.1", "echo nothing-to-see", "Enter"]).await;
    h.cli(&["send-keys", "-t", "ft:1", "echo compile-failed-badly", "Enter"]).await;
    // Each pane on its own: one being done says nothing of the others.
    h.wait_capture("ft:0.0", "the first pane's output", |t| t.matches("REDIS-TIMEOUT-here").count() >= 2).await;
    h.wait_capture("ft:0.1", "the second pane's output", |t| t.matches("nothing-to-see").count() >= 2).await;
    h.wait_capture("ft:1", "the third pane's output", |t| t.matches("compile-failed-badly").count() >= 2).await;

    // A pattern is looked for in what every pane printed, and the hit says
    // which pane and how far back it was.
    let (code, out, err) = h.cli(&["find-text", "redis-timeout"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.lines().all(|l| l.starts_with("ft:0.0")), "only the first pane has it: {out}");
    assert!(out.contains("REDIS-TIMEOUT-here"), "{out}");
    assert!(out.contains("  -"), "each hit says how many lines back: {out}");

    let (_, out, _) = h.cli(&["find-text", "compile-failed"]).await;
    assert!(out.lines().all(|l| l.starts_with("ft:1.0")), "{out}");

    // -C makes it match case; -t narrows to one window; -n caps the hits.
    let (code, _, err) = h.cli(&["find-text", "-C", "redis-timeout"]).await;
    assert_eq!(code, 1, "the text is upper case, so this must miss");
    assert!(err.contains("no pane has"), "{err}");
    let (_, out, _) = h.cli(&["find-text", "-C", "REDIS-TIMEOUT"]).await;
    assert!(out.contains("REDIS-TIMEOUT-here"), "{out}");

    let (_, out, _) = h.cli(&["find-text", "-t", "ft:1", "echo"]).await;
    assert!(out.lines().all(|l| l.starts_with("ft:1.0")), "-t limits the search: {out}");
    // -t down to a single pane, and a target that is not there says so
    // instead of reporting an empty search.
    let (_, out, _) = h.cli(&["find-text", "-t", "ft:0.1", "echo"]).await;
    assert!(out.lines().all(|l| l.starts_with("ft:0.1")), "-t takes a pane too: {out}");
    let (code, _, err) = h.cli(&["find-text", "-t", "ft:99", "echo"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no window 99"), "{err}");
    let (code, _, err) = h.cli(&["find-text", "-t", "ft:0.9", "echo"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no pane 9"), "{err}");
    // Whitespace is not a search.
    let (code, _, err) = h.cli(&["find-text", " "]).await;
    assert_eq!(code, 1);
    assert!(err.contains("pattern required"), "{err}");
    let (_, out, _) = h.cli(&["find-text", "-n", "1", "echo"]).await;
    assert_eq!(out.lines().count(), 3, "one hit from each of the three panes: {out}");

    // A pattern nobody printed is an error, not an empty success.
    let (code, _, err) = h.cli(&["find-text", "zzz-nobody-printed-this"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no pane has"), "{err}");
    // And the short name works.
    let (code, _, _) = h.cli(&["findt", "redis"]).await;
    assert_eq!(code, 0);
    h.cli(&["kill-server"]).await;
}

/// `import-config` writes what keepane takes from a tmux config into
/// keepane's own, the rest commented out; once per file; `-n` writes
/// nothing; and what it wrote loads with no errors at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_tmux_conf_is_imported_when_asked() {
    let dir = std::env::temp_dir().join(format!("keepane-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tmux = dir.join("tmux.conf");
    let out = dir.join("keepane.conf");
    let _ = std::fs::remove_file(&out);
    std::fs::write(
        &tmux,
        "set -g prefix C-a\n\
         set -g base-index 1\n\
         set -g escape-time 0\n\
         set -g @plugin 'tmux-plugins/tpm'\n\
         bind r source-file ~/.tmux.conf\n\
         run '~/.tmux/plugins/tpm/tpm'\n",
    )
    .unwrap();
    let import = |args: &[&str]| {
        let o =
            std::process::Command::new(env!("CARGO_BIN_EXE_keepane")).arg("import-config").args(args).output().unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let (t, o) = (tmux.to_string_lossy().into_owned(), out.to_string_lossy().into_owned());

    let (code, shown, err) = import(&["-n", "-o", &o, &t]);
    assert_eq!(code, Some(0), "{err}");
    assert!(!out.exists(), "-n writes nothing");
    assert!(shown.contains("set -g prefix C-a") && shown.contains("# set -g escape-time 0"), "{shown}");

    let (code, said, err) = import(&["-o", &o, &t]);
    assert_eq!(code, Some(0), "{err}");
    assert!(said.contains("3 imported, 3 skipped"), "{said}");
    assert!(said.contains("4: set -g @plugin 'tmux-plugins/tpm'  (a tmux plugin (TPM)"), "{said}");
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(written.starts_with(&format!("# keepane import-config: from {t}\nset -g prefix C-a\n")), "{written}");
    assert!(written.contains("\n# run '~/.tmux/plugins/tpm/tpm'\n"), "{written}");
    assert_eq!(written, shown, "what -n shows is what is written");

    let (code, _, err) = import(&["-o", &o, &t]);
    assert_eq!(code, Some(1), "a second import of the same file is refused");
    assert!(err.contains("was imported into") && err.contains("already"), "{err}");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), written, "and changes nothing");
    let (code, _, err) = import(&["-o", &o, &o]);
    assert_eq!(code, Some(1));
    assert!(err.contains("is the config keepane reads already"), "{err}");

    // Read the way a server reads its config: nothing to report.
    let h = Harness::start("import").await;
    let (code, _, err) = h.cli(&["source-file", &o]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(h.cli(&["show", "-gv", "prefix"]).await.1.trim(), "C-a");
    let (_, keys, _) = h.cli(&["list-keys"]).await;
    assert!(keys.lines().any(|l| l.contains("-T prefix r ") && l.contains("source-file")), "{keys}");
    h.cli(&["kill-server"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_tmux_conf_loads_with_the_rest_skipped() {
    // A config as people actually have them: TPM, copy-mode-vi bindings, a
    // %if block, continuation lines, options tmux has and keepane does not.
    let dir = std::env::temp_dir().join(format!("keepane-tmuxconf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let conf = dir.join("tmux.conf");
    std::fs::write(
        &conf,
        "# my tmux.conf\n\
         set -g prefix C-a\n\
         unbind C-b\n\
         set -g mouse on\n\
         set -g base-index 1\n\
         setw -g mode-keys vi\n\
         set -g default-terminal \"screen-256color\"\n\
         set -ga terminal-overrides \",xterm-256color:Tc\"\n\
         bind -T copy-mode-vi v send-keys -X begin-selection\n\
         bind -T copy-mode-vi y send-keys -X copy-selection-and-cancel\n\
         bind | split-window -h \\\n  -c \"#{pane_current_path}\"\n\
         %if #{==:#{host},nowhere}\n\
         set -g status off\n\
         %elif #{==:#{host},#{host}}\n\
         set -g display-time 4321\n\
         %else\n\
         set -g display-time 1\n\
         %endif\n\
         set -g @plugin 'tmux-plugins/tpm'\n\
         set -g @plugin 'tmux-plugins/tmux-sensible'\n\
         set -g status-right '#{pane_current_path}'\n",
    )
    .unwrap();
    // Started by hand with the config given directly: the other tests run
    // in this same process, so nothing may go through the environment.
    keep_history_out();
    let socket = format!("test-tmuxconf-{}", std::process::id());
    let s = socket.clone();
    let options = keepane::server::RunOptions { force_restore: false, config: Some(conf.clone()) };
    let server = tokio::spawn(async move {
        if let Err(e) = keepane::server::run_with(s, options).await {
            panic!("server: {e:#}");
        }
    });
    let pipe = pipe_name(&socket);
    let deadline = Instant::now() + Duration::from_secs(5);
    while open_pipe(&pipe).is_err() {
        assert!(Instant::now() < deadline, "server did not come up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let h = Harness { socket, _server: server, sessions_dir: dir.clone() };
    h.cli(&["set", "-g", "default-command", PROMPT_SHELL]).await;
    // Autosave must never touch the real sessions directory from a test.
    h.cli(&["set", "-g", "sessions-dir", &dir.to_string_lossy()]).await;
    h.cli(&["new", "-d", "-s", "t"]).await;

    // What keepane understands is applied...
    assert_eq!(h.cli(&["show", "-gv", "prefix"]).await.1.trim(), "C-a");
    assert_eq!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), "on");
    assert_eq!(h.cli(&["show", "-gv", "base-index"]).await.1.trim(), "1");
    let (_, keys, _) = h.cli(&["list-keys"]).await;
    assert!(
        keys.contains("-T prefix | split-window -h -c \"#{pane_current_path}\""),
        "the continued line was joined: {keys}"
    );
    // ...the copy-mode-vi lines are in the copy-mode table, not the prefix one...
    // (prefix v is keepane's own dashboard; what matters is that the
    // copy-mode commands did not land there.)
    assert!(!keys.lines().any(|l| l.contains("-T prefix v ") && l.contains("selection")), "{keys}");
    // (prefix y is keepane's own copy-output, as v is the dashboard.)
    assert!(!keys.lines().any(|l| l.contains("-T prefix y ") && l.contains("selection")), "{keys}");
    assert!(
        keys.lines().any(|l| l.starts_with("bind-key -T copy-mode-vi v") && l.contains("begin-selection")),
        "{keys}"
    );
    assert!(
        keys.lines().any(|l| l.starts_with("bind-key -T copy-mode-vi y") && l.contains("copy-selection")),
        "{keys}"
    );
    // ...the %if chain is evaluated: the host is not "nowhere" (status
    // stays on), it is itself (display-time 4321), the %else is not taken...
    assert_eq!(h.cli(&["show", "-gv", "status"]).await.1.trim(), "on");
    assert_eq!(h.cli(&["show", "-gv", "display-time"]).await.1.trim(), "4321");
    // ...and what was skipped is listed, not thrown at every attach.
    let (_, msgs, _) = h.cli(&["show-messages"]).await;
    assert!(!msgs.contains("copy-mode-vi"), "copy-mode-vi lines are applied now, not skipped: {msgs}");
    assert!(msgs.contains("@plugin tmux-plugins/tpm"), "the missing plugin is named: {msgs}");
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "t"]).await;
    c.wait_for("the one-line summary", |s| {
        let t = s.contents();
        t.contains("tmux.conf:") && t.contains("skipped ") && t.contains("lines keepane could not use")
    })
    .await;
    assert!(!c.text().contains("@plugin"), "the details stay in show-messages: {}", c.text());

    // A file that sources itself is refused, not recursed into, and one
    // with an unclosed %if says so instead of quietly dropping the rest.
    let looping = dir.join("loop.conf");
    std::fs::write(
        &looping,
        format!("set -g mouse off\nsource-file \"{}\"\n", looping.display().to_string().replace('\\', "/")),
    )
    .unwrap();
    let (code, _, err) = h.cli(&["source-file", &looping.to_string_lossy()]).await;
    assert_eq!(code, 1);
    assert!(err.contains("source-file loop"), "{err}");
    assert_eq!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), "off", "the lines before the loop still applied");
    // (`x` is a format that expands to "x", which is true, so the branch is
    // read up to where the file ends; the missing %endif is still said.)
    let open = dir.join("open.conf");
    std::fs::write(&open, "set -g mouse on\n%if x\nset -g mouse off\n").unwrap();
    let (code, _, err) = h.cli(&["source-file", &open.to_string_lossy()]).await;
    assert_eq!(code, 1);
    assert!(err.contains("%if without %endif"), "{err}");
    assert_eq!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), "off");
    // A false condition with no %endif: nothing after it is read.
    std::fs::write(&open, "set -g mouse on\n%if #{==:1,2}\nset -g mouse off\n").unwrap();
    let (code, _, _) = h.cli(&["source-file", &open.to_string_lossy()]).await;
    assert_eq!(code, 1);
    assert_eq!(h.cli(&["show", "-gv", "mouse"]).await.1.trim(), "on");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn record_writes_an_asciinema_file() {
    let h = Harness::start("record").await;
    h.cli(&["new", "-d", "-s", "r"]).await;
    h.wait_capture("r:0", "shell prompt", |t| t.contains("keepane>")).await;
    let cast = std::env::temp_dir().join(format!("keepane-record-{}.cast", std::process::id()));
    let _ = std::fs::remove_file(&cast);

    let (code, _, err) = h.cli(&["record", "-t", "r:0"]).await;
    assert_eq!(code, 1, "nothing to stop yet");
    assert!(err.contains("not recording"), "{err}");
    let (code, out, err) = h.cli(&["record", "-t", "r:0", &cast.to_string_lossy()]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("recording %"), "{out}");
    h.cli(&["send-keys", "-t", "r:0", "echo captured-in-the-cast", "Enter"]).await;
    h.wait_capture("r:0", "the echo", |t| t.matches("captured-in-the-cast").count() >= 2).await;
    // A split resizes the pane, which the recording notes as an "r" event.
    h.cli(&["split-window", "-d", "-t", "r:0"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (code, out, err) = h.cli(&["record", "-t", "r:0.0"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("recording stopped"), "{out}");
    // The writer thread flushes on close: wait for the resize event to be
    // in the file rather than for a fixed time (slow runners).
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let text = std::fs::read_to_string(&cast).unwrap_or_default();
        if text.lines().skip(1).any(|l| l.contains("\"r\"")) && text.ends_with('\n') {
            break text;
        }
        assert!(Instant::now() < deadline, "the cast never got its resize event:\n{text}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let mut lines = text.lines();
    let header: serde_json::Value = serde_json::from_str(lines.next().expect("header")).expect("header is JSON");
    assert_eq!(header["version"], 2, "{header}");
    assert!(header["width"].as_u64().unwrap() > 0 && header["height"].as_u64().unwrap() > 0, "{header}");
    let events: Vec<serde_json::Value> = lines.map(|l| serde_json::from_str(l).expect("event is JSON")).collect();
    assert!(!events.is_empty());
    let mut last_t = 0.0;
    for e in &events {
        let t = e[0].as_f64().expect("time");
        assert!(t >= last_t, "times never go backwards: {e}");
        last_t = t;
        assert!(matches!(e[1].as_str(), Some("o") | Some("r")), "{e}");
    }
    assert!(events.iter().any(|e| e[1] == "o" && e[2].as_str().is_some_and(|d| d.contains("captured-in-the-cast"))));
    assert!(events.iter().any(|e| e[1] == "r" && e[2].as_str().is_some_and(|d| d.contains('x'))), "a resize event");
    let _ = std::fs::remove_file(&cast);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_border_status_reserves_a_row_for_its_text() {
    let h = Harness::start("borderstatus").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "b"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // cmd.exe starts with a blank line, so the prompt is on row 1; with a
    // top border line it moves to row 2 and row 0 becomes the label.
    let prompt_row = c.screen.screen().rows(0, COLS).position(|r| r.contains("keepane>")).unwrap();

    // top: the first row becomes the border text, the pane moves down one.
    h.cli(&["set", "-g", "pane-border-status", "top"]).await;
    c.wait_for("border text on top", |s| {
        let r0 = s.rows(0, COLS).next().unwrap();
        r0.contains("0:") && s.rows(0, COLS).nth(prompt_row + 1).unwrap().contains("keepane>")
    })
    .await;
    // The format is a format: pane variables and modifiers work in it.
    h.cli(&["set", "-g", "pane-border-format", " [#{pane_index}] #{pane_width}x#{pane_height} "]).await;
    // 24 rows less the status line and the border row: 22.
    c.wait_for("custom format", |s| s.rows(0, COLS).next().unwrap().contains(&format!("[0] {}x{}", COLS, ROWS - 2)))
        .await;
    // Two panes side by side: each gets its own text on its own columns.
    c.prefix('%').await;
    c.wait_for("two border texts", |s| s.rows(0, COLS).next().unwrap().matches("] ").count() == 2).await;
    // Splitting while the border row is on divides the layout cell, not the
    // drawn rect: the 23-row cell -> 11 + 1 + 11. The upper pane gives up
    // its top row for its text (10); the lower one's text goes on the line
    // between them, as in tmux (11), not on a second border row.
    h.cli(&["split-window", "-v", "-d", "-t", "b:0.1"]).await;
    let (_, a, _) = h.cli(&["display-message", "-p", "-t", "b:0.1", "#{pane_height}"]).await;
    let (_, b, _) = h.cli(&["display-message", "-p", "-t", "b:0.2", "#{pane_height}"]).await;
    assert_eq!((a.trim(), b.trim()), ("10", "11"), "the lower pane's text on the parting line");
    c.wait_for("the lower pane's text on the line between", |s| {
        s.rows(0, COLS).nth(11).is_some_and(|r| r.contains("[2] ")) && !s.contents().contains("┬┬")
    })
    .await;

    // bottom: the row just above the status line.
    h.cli(&["set", "-g", "pane-border-status", "bottom"]).await;
    c.wait_for("border text at the bottom", |s| {
        s.rows(0, COLS).nth(prompt_row).unwrap().contains("keepane>")
            && s.rows(0, COLS).nth(ROWS as usize - 2).unwrap().matches("] ").count() == 2
    })
    .await;
    // off: everything back.
    h.cli(&["set", "-g", "pane-border-status", "off"]).await;
    c.wait_for("back to normal", |s| !s.rows(0, COLS).nth(ROWS as usize - 2).unwrap().contains("] ")).await;
    let (code, _, err) = h.cli(&["set", "-g", "pane-border-status", "sideways"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("off, top or bottom"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn format_variables_answer_from_the_live_tree() {
    let h = Harness::start("formats").await;
    h.cli(&["new", "-d", "-s", "fmt"]).await;
    h.wait_capture("fmt:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["split-window", "-d", "-t", "fmt:0"]).await;
    // A pane's path is what the shell announced or set-cwd recorded.
    let dir = std::env::temp_dir();
    h.cli(&["set-cwd", "-t", "fmt:0.0", &dir.to_string_lossy()]).await;
    let (code, out, err) = h
        .cli(&[
            "display-message",
            "-p",
            "#{session_windows}|#{window_panes}|#{pane_pid}|#{client_width}x#{client_height}|#{=2:session_name}|#{session_id}|#{window_id}|#{pane_id}|#{pane_active}|#{pane_dead}|#{b:pane_current_path}|#{version}",
        ])
        .await;
    assert_eq!(code, 0, "{err}");
    let parts: Vec<&str> = out.trim().split('|').collect();
    assert_eq!(parts.len(), 12, "{out}");
    assert_eq!(parts[0], "1");
    assert_eq!(parts[1], "2");
    assert!(parts[2].parse::<u32>().is_ok_and(|p| p > 0), "pane_pid: {out}");
    assert_eq!(parts[3], "80x24");
    assert_eq!(parts[4], "fm");
    assert!(parts[5].starts_with('$') && parts[6].starts_with('@') && parts[7].starts_with('%'), "{out}");
    assert_eq!(parts[8], "1");
    assert_eq!(parts[9], "0");
    let base = dir.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(parts[10], base, "basename only: {out}");
    assert_eq!(parts[11], env!("CARGO_PKG_VERSION"));
    // Conditionals on the new variables.
    let (_, out, _) = h.cli(&["display-message", "-p", "#{?window_zoomed_flag,Z,-}#{?pane_synchronized,S,-}"]).await;
    assert_eq!(out.trim(), "--");
    h.cli(&["set", "sync"]).await;
    let (_, out, _) = h.cli(&["display-message", "-p", "#{?pane_synchronized,S,-}"]).await;
    assert_eq!(out.trim(), "S");
    h.cli(&["kill-server"]).await;
}

// Keep the unused-import lint quiet for helper traits used through split().
#[allow(dead_code)]
fn _assert_traits<T: AsyncRead + AsyncWrite>() {}

#[tokio::test(flavor = "multi_thread")]
async fn save_history_all_keeps_the_whole_scrollback_with_colours() {
    let h = Harness::start("savehist").await;
    h.cli(&["new", "-d", "-s", "h"]).await;
    h.wait_capture("h:0", "shell prompt", |t| t.contains("keepane>")).await;
    // A coloured prompt, then more lines than the screen holds.
    h.cli(&["send-keys", "-t", "h:0", &red_prompt("red"), "Enter"]).await;
    // The new prompt first: typed ahead, the loop's first line would follow
    // it on its row where the terminal echoes typing (a pty), not the shell.
    h.wait_capture("h:0", "the red prompt", |t| t.lines().any(|l| l.trim_end() == "red>")).await;
    h.cli(&["send-keys", "-t", "h:0", &count_to(60, "scroll-line-"), "Enter"]).await;
    h.wait_capture("h:0", "the last line", |t| t.contains("scroll-line-60")).await;
    let saved = |h: &Harness| {
        std::fs::read_dir(&h.sessions_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .find(|t| t.contains("\"name\": \"h\""))
            .expect("a saved file for h")
    };

    // A number keeps that many lines from the bottom...
    h.cli(&["set", "-g", "save-history", "5"]).await;
    let (code, _, err) = h.cli(&["save-session", "-t", "h"]).await;
    assert_eq!(code, 0, "{err}");
    let file = saved(&h);
    assert!(file.contains("\"scroll-line-60\""), "{file}");
    assert!(!file.contains("\"scroll-line-1\""), "only the last 5 lines: {file}");
    // ...and `all` keeps everything, colours as escape sequences.
    let (code, _, err) = h.cli(&["set", "-g", "save-history", "all"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(h.cli(&["show", "-gv", "save-history"]).await.1.trim(), "all");
    h.cli(&["save-session", "-t", "h"]).await;
    let file = saved(&h);
    assert!(file.contains("\"scroll-line-1\""), "the first line, long scrolled off: {file}");
    assert!(file.contains("\\u001b[31mred"), "the red prompt: {file}");
    let (code, _, err) = h.cli(&["set", "-g", "save-history", "lots"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("or all"), "{err}");

    // Resumed, the whole scrollback is back and the prompt is still red.
    h.cli(&["new", "-d", "-s", "keeper"]).await;
    h.cli(&["kill-session", "-t", "h"]).await;
    let (code, _, err) = h.cli(&["resume", "h"]).await;
    assert_eq!(code, 0, "{err}");
    // The saved output is printed back first and the shell starts after
    // it, so the new shell's prompt (plain "keepane>", the saved one is red)
    // is the last line once everything is in.
    let deadline = Instant::now() + Duration::from_secs(15);
    let out = loop {
        let (_, out, _) = h.cli(&["capture-pane", "-p", "-e", "-S", "-", "-t", "h:0"]).await;
        let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        if last.trim_end() == "keepane>" && out.lines().any(|l| l.trim_end() == "scroll-line-1") {
            break out;
        }
        assert!(Instant::now() < deadline, "restored scrollback then a fresh prompt: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(out.contains("\x1b[31mred"), "{out}");

    // A detached session takes its size from -x/-y (there is no terminal
    // to take it from); one row goes to the status line.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "small", "-x", "30", "-y", "5"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["display-message", "-p", "-t", "small:0", "#{pane_width}x#{pane_height}"]).await;
    assert_eq!(out.trim(), "30x4");
    let (code, _, err) = h.cli(&["new", "-d", "-s", "tooSmall", "-x", "3"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("at least 10"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_lists_every_pane_with_its_state() {
    let h = Harness::start("jobs").await;
    h.cli(&["set", "-g", "remain-on-exit", "on"]).await;
    h.cli(&["new", "-d", "-s", "build"]).await;
    h.cli(&["new", "-d", "-s", "web"]).await;
    h.cli(&["split-window", "-d", "-t", "web:0"]).await;
    let (code, _, err) = h.cli(&args(&["new-window", "-d", "-t", "build", "-n", "dies"], &exits(4))).await;
    assert_eq!(code, 0, "{err}");
    // The board notices the exit (remain-on-exit keeps the pane to show it).
    let deadline = Instant::now() + Duration::from_secs(10);
    let out = loop {
        let (code, out, err) = h.cli(&["jobs"]).await;
        assert_eq!(code, 0, "{err}");
        if out.contains("exit 4") {
            break out;
        }
        assert!(Instant::now() < deadline, "the exited pane never showed: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5, "a header and four panes: {out}");
    assert!(lines[0].starts_with("PANE") && lines[0].contains("STATE") && lines[0].contains("IDLE"), "{out}");
    for want in ["build:0.0", "build:1.0", "web:0.0", "web:0.1"] {
        assert!(lines.iter().any(|l| l.starts_with(want)), "{want} listed: {out}");
    }
    let dead = lines.iter().find(|l| l.starts_with("build:1.0")).unwrap();
    assert!(dead.contains("exit 4"), "{dead}");
    let live = lines.iter().find(|l| l.starts_with("web:0.1")).unwrap();
    assert!(live.contains("running"), "{live}");
    // pid, then the command: a number in the PID column.
    assert!(live.split_whitespace().nth(4).is_some_and(|p| p.parse::<u32>().is_ok()), "{live}");

    // A dead pane's UP stops at its death; pane_dead_time says when, and is
    // empty for a live one.
    let up = |out: &str| {
        out.lines().find(|l| l.starts_with("build:1.0")).unwrap().split_whitespace().nth(3).unwrap().to_string()
    };
    let first = up(&out);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (_, later, _) = h.cli(&["jobs"]).await;
    assert_eq!(up(&later), first, "the clock of an exited pane does not run on: {later}");
    // Not even by rounding: its start and its end are seconds that stay
    // what they are, however often and whenever they are asked for (the
    // pane below starts and ends wherever in a second the clock happens to be).
    let (code, _, err) =
        h.cli(&args(&["new-window", "-d", "-t", "build", "-n", "brief"], &exits_after_a_second(3))).await;
    assert_eq!(code, 0, "{err}");
    let ran = async || {
        let (_, t, _) = h.cli(&["jobs", "-t", "build:2.0", "-F", "#{pane_dead_time} #{pane_start_time}"]).await;
        let v: Vec<i64> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        (v.len() == 2 && v[0] > 0).then(|| v[0] - v[1])
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let once = loop {
        if let Some(d) = ran().await {
            break d;
        }
        if Instant::now() >= deadline {
            let (_, all, _) = h.cli(&["jobs", "-t", "build"]).await;
            panic!("the brief pane never ended: {all}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(ran().await, Some(once), "the same two seconds every time");
    }
    let (_, dead, _) = h.cli(&["jobs", "-t", "build:1.0", "-F", "#{pane_dead_time}"]).await;
    assert!(dead.trim().parse::<i64>().is_ok_and(|t| t > 1_600_000_000), "{dead:?}");
    let (_, live, _) = h.cli(&["jobs", "-t", "build:0.0", "-F", "[#{pane_dead_time}]"]).await;
    assert_eq!(live.trim(), "[]");

    // -t narrows to a session, a window or one pane; -F says what to print.
    let (_, out, _) = h.cli(&["jobs", "-t", "web"]).await;
    assert_eq!(out.lines().count(), 3, "{out}");
    assert!(out.lines().skip(1).all(|l| l.starts_with("web:")), "{out}");
    let (_, out, _) = h.cli(&["jobs", "-t", "web:0.1", "-F", "#{pane_index}"]).await;
    assert_eq!(out.trim(), "1", "one pane asked for, one answered: {out:?}");
    // Columns line up on screen even with a double-width session name.
    h.cli(&["new", "-d", "-s", "中文"]).await;
    let (_, out, _) = h.cli(&["jobs"]).await;
    let offsets: std::collections::HashSet<usize> = out
        .lines()
        .map(|l| {
            let at = l.find("running").or_else(|| l.find("STATE")).or_else(|| l.find("exit")).unwrap();
            unicode_width::UnicodeWidthStr::width(&l[..at])
        })
        .collect();
    assert_eq!(offsets.len(), 1, "the STATE column starts at one screen column on every row: {out}");
    let (_, out, _) =
        h.cli(&["jobs", "-t", "build:1", "-F", "#{session_name}/#{window_name} #{pane_dead_status}"]).await;
    assert_eq!(out.trim(), "build/dies 4");
    let (_, out, _) = h.cli(&["jobs", "-t", "web:0.0", "-F", "#{pane_start_time} #{pane_activity}"]).await;
    let mut it = out.split_whitespace().map(|n| n.parse::<i64>().expect("unix seconds"));
    let (start, activity) = (it.next().unwrap(), it.next().unwrap());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((now - start).abs() < 60 && activity >= start && activity <= now, "{out} vs now {now}");

    let (code, _, err) = h.cli(&["jobs", "-t", "nosuch"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("nosuch"), "{err}");
    let (code, _, err) = h.cli(&["jobs", "extra"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("unexpected argument"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_justify_and_separator_move_the_window_list() {
    let h = Harness::start("justify").await;
    // A right side that starts with the quoted pane title, whatever the
    // default is these days: the assertions below look for the quote.
    h.cli(&["set", "-g", "status-right", "\"#T\" %H:%M"]).await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "j", "-n", "aa"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["new-window", "-d", "-t", "j", "-n", "bb"]).await;
    let status = |s: &vt100::Screen| s.rows(0, COLS).last().unwrap();
    // The separator goes between the labels, the list starts after "[j] ".
    h.cli(&["set", "-g", "window-status-separator", " | "]).await;
    c.wait_for("separator", |s| status(s).contains("0:aa") && status(s).contains(" | 1:bb")).await;
    assert_eq!(status(c.screen.screen()).find("0:aa"), Some(4), "{}", status(c.screen.screen()));
    // right: the list ends one gap before the right side, which starts
    // with the quoted pane title.
    h.cli(&["set", "-g", "status-justify", "right"]).await;
    c.wait_for("right-justified", |s| status(s).contains("1:bb \"")).await;
    // centre: away from both sides, and `center` spells it too.
    h.cli(&["set", "-g", "status-justify", "center"]).await;
    assert_eq!(h.cli(&["show", "-gv", "status-justify"]).await.1.trim(), "centre");
    c.wait_for("centred", |s| {
        let row = status(s);
        row.find("0:aa").is_some_and(|at| at > 6) && !row.contains("1:bb \"") && row.contains("1:bb  ")
    })
    .await;
    let (code, _, err) = h.cli(&["set", "-g", "status-justify", "sideways"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("left, centre, right or absolute-centre"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_shot_formats_run_their_shell_pieces() {
    let h = Harness::start("oneshot").await;
    h.cli(&["new", "-d", "-s", "os"]).await;
    // display-message -p is answered now: a #(command) nobody has run yet
    // runs here, not "next status-interval".
    let (code, out, err) = h.cli(&["display-message", "-p", "#(echo one-shot-ok)|#{session_name}"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "one-shot-ok|os");
    // ...with a leash: a command that hangs is given up on, and the
    // server answers anyway.
    let started = Instant::now();
    let hang = if cfg!(windows) { "[#(Start-Sleep 20; echo late)]" } else { "[#(sleep 20; echo late)]" };
    let (code, out, _) = h.cli(&["display-message", "-p", hang]).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "[]");
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    // The server is fine after that, and jobs -F runs its pieces the same way.
    let (_, out, _) = h.cli(&["display-message", "-p", "#{session_name}"]).await;
    assert_eq!(out.trim(), "os");
    let (_, out, _) = h.cli(&["jobs", "-t", "os:0.0", "-F", "#(echo in-jobs) #{pane_index}"]).await;
    assert_eq!(out.trim(), "in-jobs 0");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_session_keeps_its_saved_size() {
    let h = Harness::start("savesize").await;
    h.cli(&["new", "-d", "-s", "keeper"]).await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "sz", "-x", "100", "-y", "30"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["save-session", "-t", "sz"]).await;
    let file = std::fs::read_dir(&h.sessions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .find(|t| t.contains("\"name\": \"sz\""))
        .expect("a saved file for sz");
    assert!(file.contains("\"size\""), "the size is in the file: {file}");
    h.cli(&["kill-session", "-t", "sz"]).await;
    // Resumed from a script (nothing attaching), it is the size it was.
    let (code, _, err) = h.cli(&["resume", "sz"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = h.cli(&["display-message", "-p", "-t", "sz:0", "#{window_width}x#{window_height}"]).await;
    assert_eq!(out.trim(), "100x30");
    h.cli(&["kill-server"]).await;
}

/// `split-window -N count` makes that many panes at once and tiles them;
/// `-d` keeps the focus; a window too small for all of them says how many
/// it made and keeps them.
/// `undo-kill`: a pane or window killed by a command is kept, its program
/// running, for `undo-kill-time` seconds, and comes back where it was;
/// after that, or when its program ends meanwhile, it is gone for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_pane_or_window_comes_back_with_undo_kill() {
    let h = Harness::start("undokill").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "u", "-x", "120", "-y", "40"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["split-window", "-h", "-d", "-t", "u"]).await;
    h.cli(&["split-window", "-v", "-d", "-t", "u:0.1"]).await;
    let panes = async || {
        h.cli(&["list-panes", "-t", "u:0", "-F", "#{pane_id} #{pane_pid} #{pane_width}x#{pane_height}"]).await.1
    };
    let before = panes().await;
    assert_eq!(before.lines().count(), 3, "{before}");
    let middle = before.lines().nth(1).unwrap().to_string();
    let id = middle.split(' ').next().unwrap().to_string();

    // Killed, then back: same pane (id and process), same layout.
    assert_eq!(h.cli(&["kill-pane", "-t", &id]).await.0, 0);
    assert_eq!(panes().await.lines().count(), 2);
    let (code, _, err) = h.cli(&["undo-kill"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(panes().await, before, "back as it was");
    assert_eq!(h.cli(&["display-message", "-p", "-t", "u", "#{pane_id}"]).await.1.trim(), id, "and active");
    let (code, _, err) = h.cli(&["undo-kill"]).await;
    assert!(code != 0 && err.contains("nothing to undo"), "{err}");

    // A window: back at its place, with its name.
    h.cli(&["new-window", "-d", "-t", "u", "-n", "second"]).await;
    h.cli(&["new-window", "-d", "-t", "u", "-n", "third"]).await;
    let windows = async || h.cli(&["list-windows", "-t", "u"]).await.1;
    assert_eq!(h.cli(&["kill-window", "-t", "u:1"]).await.0, 0);
    assert!(!windows().await.contains("second"));
    assert_eq!(h.cli(&["undo-kill"]).await.0, 0);
    let list = windows().await;
    assert!(list.lines().nth(1).is_some_and(|l| l.starts_with("1: second")), "{list}");
    assert!(list.lines().nth(2).is_some_and(|l| l.starts_with("2: third")), "{list}");

    // A kept pane whose program ends is gone.
    let ids =
        async || -> Vec<String> { panes().await.lines().map(|l| l.split(' ').next().unwrap().to_string()).collect() };
    let old = ids().await;
    let brief: &[&str] =
        if cfg!(windows) { &["cmd.exe", "/c", "ping -n 2 127.0.0.1 >nul"] } else { &["/bin/sh", "-c", "sleep 1"] };
    let (code, _, err) = h.cli(&[&["split-window", "-d", "-t", "u:0"], brief].concat()).await;
    assert_eq!(code, 0, "{err}");
    let short = ids().await.into_iter().find(|i| !old.contains(i)).unwrap();
    let running = |pid: &str| {
        if cfg!(windows) {
            let out =
                std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} "))
        } else {
            std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        }
    };
    // Waited for rather than slept on: a loaded machine is slow to start
    // and to end processes.
    let gone = async |pid: &str, what: &str| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while running(pid) {
            assert!(Instant::now() < deadline, "{what}: process {pid} still running");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let pid_of = async |id: &str| {
        let out = h.cli(&["display-message", "-p", "-t", id, "#{pane_pid}"]).await.1;
        out.trim().to_string()
    };
    let short_pid = pid_of(&short).await;
    assert_eq!(h.cli(&["kill-pane", "-t", &short]).await.0, 0);
    gone(&short_pid, "the short job").await;
    // Its exit reaches the server a moment after the process is gone.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (code, _, err) = h.cli(&["undo-kill"]).await;
    assert!(code != 0 && err.contains("nothing to undo"), "{err}");

    // Past the time: gone. And 0 keeps nothing.
    h.cli(&["set", "-g", "undo-kill-time", "1"]).await;
    let line = panes().await.lines().nth(1).unwrap().to_string();
    let (id, pid) =
        line.split_once(' ').map(|(i, rest)| (i.to_string(), rest.split(' ').next().unwrap().to_string())).unwrap();
    h.cli(&["kill-pane", "-t", &id]).await;
    assert!(running(&pid), "kept, the program still runs");
    gone(&pid, "past the time, its program is ended").await;
    assert_ne!(h.cli(&["undo-kill"]).await.0, 0);
    h.cli(&["set", "-g", "undo-kill-time", "0"]).await;
    let id = panes().await.lines().nth(1).unwrap().split(' ').next().unwrap().to_string();
    h.cli(&["kill-pane", "-t", &id]).await;
    assert_ne!(h.cli(&["undo-kill"]).await.0, 0);
    // The last pane of a session takes the session with it: not kept.
    h.cli(&["set", "-g", "undo-kill-time", "10"]).await;
    h.cli(&["new", "-d", "-s", "solo"]).await;
    h.cli(&["kill-pane", "-t", "solo"]).await;
    assert_ne!(h.cli(&["has-session", "-t", "solo"]).await.0, 0);
    assert_ne!(h.cli(&["undo-kill"]).await.0, 0);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn split_window_makes_many_panes_at_once() {
    let h = Harness::start("splitmany").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "g", "-x", "160", "-y", "48"]).await;
    assert_eq!(code, 0, "{err}");
    let panes = async || h.cli(&["list-panes", "-t", "g"]).await.1;
    let (code, _, err) = h.cli(&["split-window", "-d", "-N", "5", "-t", "g"]).await;
    assert_eq!(code, 0, "{err}");
    let list = panes().await;
    assert_eq!(list.lines().count(), 6, "{list}");
    // Tiled: 3 columns of 2 rows, all within a cell of each other.
    let sizes: Vec<(u16, u16)> = list
        .lines()
        .filter_map(|l| l.split_once('[')?.1.split_once(']')?.0.split_once('x'))
        .map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap()))
        .collect();
    let (wmin, wmax) = (sizes.iter().map(|s| s.0).min().unwrap(), sizes.iter().map(|s| s.0).max().unwrap());
    let (hmin, hmax) = (sizes.iter().map(|s| s.1).min().unwrap(), sizes.iter().map(|s| s.1).max().unwrap());
    assert!(wmax - wmin <= 1 && hmax - hmin <= 1, "even: {sizes:?}");
    assert_eq!(h.cli(&["display-message", "-p", "-t", "g", "#{window_layout}"]).await.0, 0);
    // -d: the first pane is still the active one.
    assert!(list.lines().next().unwrap().contains("(active)"), "{list}");
    // Without -d the newest pane takes the focus.
    let (code, _, err) = h.cli(&["split-window", "-N", "2", "-t", "g"]).await;
    assert_eq!(code, 0, "{err}");
    let list = panes().await;
    assert_eq!(list.lines().count(), 8, "{list}");
    assert!(!list.lines().next().unwrap().contains("(active)"), "focus moved: {list}");
    // Too many for a small window: an error naming how many were made,
    // which stay.
    let (code, _, err) = h.cli(&["new", "-d", "-s", "tiny", "-x", "40", "-y", "12"]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = h.cli(&["split-window", "-d", "-N", "64", "-t", "tiny"]).await;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("of 64 panes"), "{err}");
    let made: usize = err.split("made ").nth(1).and_then(|s| s.split(' ').next()).unwrap().parse().unwrap();
    assert!(made > 0 && made < 64, "{err}");
    assert_eq!(h.cli(&["list-panes", "-t", "tiny"]).await.1.lines().count(), 1 + made);
    h.cli(&["kill-server"]).await;
}

/// A window squeezed to a few rows and grown back gets its pane split back,
/// and `show-options` prints the styles a theme sets.
#[tokio::test(flavor = "multi_thread")]
async fn a_squeezed_window_gets_its_split_back() {
    let h = Harness::start("squeeze").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "q", "-x", "80", "-y", "41"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["split-window", "-v", "-t", "q"]).await;
    // Top pane 30 rows, bottom 9 (40 rows of panes, one border).
    let heights = async || -> Vec<u16> {
        h.cli(&["list-panes", "-t", "q"])
            .await
            .1
            .lines()
            .filter_map(|l| l.split_once('x')?.1.split_once(']')?.0.parse().ok())
            .collect()
    };
    let now = heights().await;
    let (code, _, err) = h.cli(&["resize-pane", "-t", "q", "-y", "9"]).await;
    assert_eq!(code, 0, "{err} from {now:?}");
    assert_eq!(heights().await, [30, 9]);
    for y in ["4", "41"] {
        let (code, _, err) = h.cli(&["resize-window", "-t", "q", "-y", y]).await;
        assert_eq!(code, 0, "{err}");
    }
    assert_eq!(heights().await, [30, 9], "the split survives the squeeze");
    // Styles read back in the form `set` takes.
    h.cli(&["set", "-g", "status-style", "fg=#c0caf5,bg=#16161e"]).await;
    assert_eq!(h.cli(&["show-options", "-gv", "status-style"]).await.1.trim(), "fg=#c0caf5,bg=#16161e");
    assert!(h.cli(&["show-options", "-g"]).await.1.contains("pane-active-border-style"));
    h.cli(&["kill-server"]).await;
}

/// `resize-pane -x/-y` reaches the size asked for whichever pane of a split
/// it is given: the first, a middle one, or the last (whose edge is its
/// leading one, which once sent it the wrong way to the far end).
#[tokio::test(flavor = "multi_thread")]
async fn resize_pane_reaches_the_size_from_any_position() {
    let h = Harness::start("resizeany").await;
    for (session, split, flag, dim) in [("tall", "-v", "-y", 1usize), ("wide", "-h", "-x", 0usize)] {
        let (code, _, err) = h.cli(&["new", "-d", "-s", session, "-x", "40", "-y", "41"]).await;
        assert_eq!(code, 0, "{err}");
        h.cli(&["split-window", split, "-t", session]).await;
        h.cli(&["split-window", split, "-t", session]).await;
        // 40 cells of panes along the split, two borders: 38 to share, and
        // the other two keep at least one each.
        let total = 38;
        let sizes = async || -> Vec<u16> {
            h.cli(&["list-panes", "-t", session])
                .await
                .1
                .lines()
                .filter_map(|l| {
                    let wh = l.split_once('[')?.1.split_once(']')?.0;
                    let (w, h) = wh.split_once('x')?;
                    [w, h][dim].parse().ok()
                })
                .collect()
        };
        for pane in 0..3 {
            for want in [1u16, 5, 12, 20, total - 2] {
                let target = format!("{session}:0.{pane}");
                let (code, _, err) = h.cli(&["resize-pane", "-t", &target, flag, &want.to_string()]).await;
                assert_eq!(code, 0, "{err}");
                let got = sizes().await;
                assert_eq!(got[pane], want, "{session} pane {pane} {flag} {want}: {got:?}");
                assert_eq!(got.iter().sum::<u16>(), total, "{got:?}");
            }
        }
    }
    h.cli(&["kill-server"]).await;
}

/// One HTTP request, the way the phone's page makes it: (status, body).
async fn http(addr: std::net::SocketAddr, method: &str, path: &str, key: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: phone\r\nX-Keepane-Key: {key}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// More than one network card: `-b` with each address serves on every
/// one, the same key and port; an address this machine does not have is
/// an error, not left out quietly.
#[tokio::test(flavor = "multi_thread")]
async fn web_serves_on_every_address_it_is_given() {
    let h = Harness::start("web-many").await;
    h.cli(&["new", "-d", "-s", "m"]).await;
    // 192.0.2.1 is a documentation address: no machine has it.
    let (code, _, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1,192.0.2.1"]).await;
    assert!(code != 0 && err.contains("192.0.2.1") && err.contains("an address of this machine"), "{err}");
    // First or not, the same word: the address, not the port.
    let (code, _, err) = h.cli(&["web-start", "-p", "0", "-b", "192.0.2.1"]).await;
    assert!(code != 0 && err.contains("an address of this machine") && !err.contains("--port"), "{err}");
    let (code, status, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1", "-b", "::1"]).await;
    assert_eq!(code, 0, "{err}");
    let url = keepane::web::status_url(&status).expect(&status).to_string();
    let port: u16 = url["http://127.0.0.1:".len()..].split('/').next().unwrap().parse().unwrap();
    let key = url.split("#k=").nth(1).unwrap();
    assert!(status.contains(&format!("also http://[::1]:{port}/#k={key}")), "{status}");
    for ip in ["127.0.0.1", "::1"] {
        let addr = std::net::SocketAddr::new(ip.parse().unwrap(), port);
        let (code, body) = http(addr, "GET", "/api/info", key, "").await;
        assert_eq!(code, 200, "{ip}: {body}");
    }
    h.cli(&["kill-server"]).await;
}

/// The phone's inbox: each waiting message whole, in order; to the top,
/// deleted and brought back; what the pane works on and what it finished.
/// Only POST changes anything, and not on a read-only page.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_page_shows_and_orders_an_inbox() {
    let h = Harness::start("web-inbox").await;
    h.cli(&["new", "-d", "-s", "ib"]).await;
    let p = pane_id(&h, "ib:0.0").await;
    let t = format!("%{p}");
    h.cli(&["rename-pane", "-t", &t, "agent"]).await;
    h.cli(&["set-work-mode", "-t", &t, "ai"]).await;
    for text in ["one", "two\nsecond line", "three"] {
        h.cli(&["send-message", "-t", &t, text]).await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = std::sync::Arc::new(keepane::web::State::new(&h.socket, "k3y", false));
    tokio::spawn(keepane::web::serve(listener, state));
    let q = format!("/api/inbox?pane=%25{p}");
    let inbox = async || -> serde_json::Value {
        let (code, body) = http(addr, "GET", &q, "k3y", "").await;
        assert_eq!(code, 200, "{body}");
        serde_json::from_str(&body).unwrap()
    };
    let ids = |v: &serde_json::Value| -> Vec<u64> {
        v["queued"].as_array().unwrap().iter().map(|m| m["id"].as_u64().unwrap()).collect()
    };
    let v = inbox().await;
    assert_eq!(ids(&v), [1, 2, 3]);
    assert_eq!(v["queued"][1]["text"], "two\nsecond line", "whole, not its first line");
    assert_eq!((v["name"].as_str(), v["current"].is_null()), (Some("agent"), true));
    // To the top; deleted; brought back.
    assert_eq!(http(addr, "POST", "/api/message?do=top&id=3", "k3y", "").await.0, 200);
    assert_eq!(ids(&inbox().await), [3, 1, 2]);
    assert_eq!(http(addr, "POST", "/api/message?do=drop&id=1", "k3y", "").await.0, 200);
    assert_eq!(ids(&inbox().await), [3, 2]);
    assert_eq!(http(addr, "POST", "/api/message?do=undo", "k3y", "").await.0, 200);
    assert_eq!(ids(&inbox().await), [3, 1, 2]);
    // What is not an action, or not by POST, or no such message.
    assert_eq!(http(addr, "POST", "/api/message?do=kill&id=1", "k3y", "").await.0, 400);
    assert_eq!(http(addr, "POST", "/api/message?do=top", "k3y", "").await.0, 400, "no id");
    assert_eq!(http(addr, "GET", "/api/message?do=drop&id=1", "k3y", "").await.0, 405);
    assert_eq!(http(addr, "POST", "/api/message?do=drop&id=99", "k3y", "").await.0, 404);
    // Its agent takes the first: working on it; then done, and finished.
    h.cli_in(Some(p), &["pane-ready"]).await;
    let v = inbox().await;
    assert_eq!((v["current"]["id"].as_u64(), ids(&v)), (Some(3), vec![1, 2]));
    let (_, list) = http(addr, "GET", "/api/panes", "k3y", "").await;
    assert!(list.contains("\"working\":3"), "{list}");
    h.cli(&["send-keys", "-t", &t, "x"]).await;
    h.cli_in(Some(p), &["pane-ready"]).await;
    let v = inbox().await;
    assert_eq!((v["recent"][0]["id"].as_u64(), v["recent"][0]["stage"].as_str()), (Some(3), Some("done")), "{v}");
    // Read-only: looking, yes; ordering, no.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ro = listener.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, "k3y", true))));
    assert_eq!(http(ro, "GET", &q, "k3y", "").await.0, 200);
    assert_eq!(http(ro, "POST", "/api/message?do=top&id=2", "k3y", "").await.0, 403);
    h.cli(&["kill-server"]).await;
}

/// `keepane web` end to end over real HTTP: the key is asked for, the list
/// names the panes, text typed on the phone runs in the pane (`-` first
/// included), the screen comes back with it, and the ⋯ menu splits.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_page_lists_shows_types_and_splits() {
    let h = Harness::start("web").await;
    h.cli(&["new", "-d", "-s", "w"]).await;
    h.wait_capture("w:0", "shell prompt", |t| t.contains("keepane>")).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = std::sync::Arc::new(keepane::web::State::new(&h.socket, "k3y-for-the-test", false));
    tokio::spawn(keepane::web::serve(listener, state));
    let key = "k3y-for-the-test";

    assert_eq!(http(addr, "GET", "/api/panes", "wrong", "").await.0, 401);
    let (code, page) = http(addr, "GET", "/", "", "").await;
    assert_eq!(code, 200);
    assert!(page.contains("<title>keepane</title>"));
    let (code, list) = http(addr, "GET", "/api/panes", key, "").await;
    assert_eq!(code, 200, "{list}");
    assert!(list.contains("\"session\":\"w\""), "{list}");
    let id = list.split("\"id\":\"%").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
    let pane = format!("%25{id}"); // %N, as a query writes it

    // Typed on the phone, run here. A leading `-` is text, not a flag.
    let (code, err) = http(addr, "POST", &format!("/api/send?pane={pane}"), key, "echo -from-the-phone-7").await;
    assert_eq!(code, 200, "{err}");
    assert_eq!(http(addr, "POST", &format!("/api/send?pane={pane}&key=Enter"), key, "").await.0, 200);
    h.wait_capture("w:0", "the command's output", |t| t.lines().any(|l| l.trim() == "-from-the-phone-7")).await;
    let (code, screen) = http(addr, "GET", &format!("/api/screen?pane={pane}&history=20"), key, "").await;
    assert_eq!(code, 200);
    assert!(screen.contains("-from-the-phone-7"), "{screen}");

    // A line wider than the pane: as the pane shows it, broken at its edge;
    // joined (`join=1`, for a phone that wraps at its own width), whole.
    let long = format!("L{}", "z".repeat(COLS as usize + 20));
    http(addr, "POST", &format!("/api/send?pane={pane}"), key, &format!("echo {long}")).await;
    http(addr, "POST", &format!("/api/send?pane={pane}&key=Enter"), key, "").await;
    h.wait_capture("w:0", "the long line", |t| {
        t.contains("zzz\n") || t.lines().filter(|l| l.contains("zzz")).count() >= 3
    })
    .await;
    let (_, split) = http(addr, "GET", &format!("/api/screen?pane={pane}&history=20"), key, "").await;
    assert!(!split.contains(&long), "the pane breaks it: {split}");
    let (code, joined) = http(addr, "GET", &format!("/api/screen?pane={pane}&history=20&join=1"), key, "").await;
    assert_eq!(code, 200);
    assert!(joined.contains(&long), "{joined}");

    // The ⋯ menu; the new pane starts in the directory of the pane it came
    // from, not where the web client runs.
    let (cd, there) = if cfg!(windows) { ("cd /d C:\\Windows", "[c:\\windows]") } else { ("cd /usr", "[/usr]") };
    h.cli(&["send-keys", "-t", "w:0", cd, "Enter"]).await;
    h.wait_capture("w:0", "the cd", |t| t.contains("C:\\Windows>") || t.lines().any(|l| l.trim() == "keepane>")).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.cli(&["list-panes", "-t", "w"]).await.1.to_ascii_lowercase().contains(there) {
        assert!(Instant::now() < deadline, "the pane never reported its new directory");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (code, err) = http(addr, "POST", &format!("/api/action?pane={pane}&do=split-h"), key, "").await;
    assert_eq!(code, 200, "{err}");
    let list = h.cli(&["list-panes", "-t", "w"]).await.1;
    assert_eq!(list.lines().count(), 2, "{list}");
    assert!(list.lines().nth(1).unwrap().to_ascii_lowercase().contains(there), "{list}");
    assert_eq!(http(addr, "POST", &format!("/api/action?pane={pane}&do=kill-server"), key, "").await.0, 400);
    assert_eq!(h.cli(&["ls"]).await.0, 0, "the server is still there");
    h.cli(&["kill-server"]).await;
}

/// The phone renames the session and window a pane is in: the name is
/// the body (one starting with `-` included), keepane's rules apply, and a
/// name it does not take comes back as the phone's mistake (400).
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_renames_sessions_windows_and_panes() {
    let h = Harness::start("webrename").await;
    h.cli(&["new", "-d", "-s", "r"]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let key = "rename-key";
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, key, false))));
    let p = pane_id(&h, "r:0.0").await;
    let at = |what: &str| format!("/api/action?pane=%25{p}&do=rename-{what}");

    let (code, err) = http(addr, "POST", &at("window"), key, "logs on phone").await;
    assert_eq!(code, 200, "{err}");
    let (code, err) = http(addr, "POST", &at("session"), key, "-phone").await;
    assert_eq!(code, 200, "{err}");
    let (_, list) = http(addr, "GET", "/api/panes", key, "").await;
    for want in ["\"windowName\":\"logs on phone\"", "\"session\":\"-phone\""] {
        assert!(list.contains(want), "{want}: {list}");
    }
    assert!(h.cli(&["ls"]).await.1.contains("-phone:"));

    // keepane's rules, and the page's: 400 with the reason.
    let (code, err) = http(addr, "POST", &at("session"), key, "").await;
    assert_eq!(code, 400);
    assert!(err.contains("bad session name"), "{err}");
    assert_eq!(http(addr, "POST", &at("window"), key, &"x".repeat(65)).await.0, 400);
    assert_eq!(http(addr, "POST", &at("window"), key, "two\nlines").await.0, 400);
    // A pane is not renamed from the phone.
    let (code, err) = http(addr, "POST", &at("pane"), key, "from_phone").await;
    assert_eq!(code, 400, "{err}");
    h.cli(&["kill-server"]).await;
}

/// A full-screen program (the alternate screen) keeps no history: the
/// wheel, from `send-keys WheelUp` or the phone, reaches it instead, as a
/// terminal sends it (arrow keys to one that did not ask for the mouse);
/// `#{alternate_on}` and the page's list say it is full screen.
#[tokio::test(flavor = "multi_thread")]
async fn the_wheel_reaches_a_full_screen_program() {
    let h = Harness::start("wheel").await;
    h.cli(&["new", "-d", "-s", "wh"]).await;
    // A program that goes full screen and says the three keys it reads.
    let reader: &[&str] = if cfg!(windows) {
        &[
            "powershell.exe",
            "-NoProfile",
            "-Command",
            "[Console]::Write([char]27 + '[?1049h'); $k = 1..3 | % { [Console]::ReadKey($true).Key }; 'got ' + ($k -join ','); Start-Sleep 30",
        ]
    } else {
        &[
            "sh",
            "-c",
            "printf '\\033[?1049h'; stty raw -echo; k=$(dd bs=1 count=9 2>/dev/null | od -An -c | tr -s ' '); stty sane; echo got $k; sleep 30",
        ]
    };
    let (code, _, err) = h.cli(&[&["new-window", "-d", "-t", "wh", "-n", "full"], reader].concat()).await;
    assert_eq!(code, 0, "{err}");
    let alt = async |t: &str| h.cli(&["display", "-p", "-t", t, "#{alternate_on}"]).await.1.trim().to_string();
    let deadline = Instant::now() + Duration::from_secs(15);
    while alt("wh:full").await != "1" {
        assert!(Instant::now() < deadline, "never went full screen");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(alt("wh:0").await, "0", "a shell at its prompt");
    tokio::time::sleep(Duration::from_millis(1500)).await; // the reader's start-up
    // The page's list says which is which.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, "wk", false))));
    let (_, list) = http(addr, "GET", "/api/panes", "wk", "").await;
    let v: serde_json::Value = serde_json::from_str(&list).unwrap();
    let full = pane_id(&h, "wh:full.0").await;
    let by = |id: u32| v.as_array().unwrap().iter().find(|e| e["id"] == format!("%{id}")).cloned().unwrap();
    assert_eq!(by(full)["alt"], true, "{list}");
    assert_eq!(by(pane_id(&h, "wh:0.0").await)["alt"], false, "{list}");
    // One wheel step up, from the page: three Up keys reach the program.
    let (code, err) = http(addr, "POST", &format!("/api/send?pane=%25{full}&key=WheelUp"), "wk", "").await;
    assert_eq!(code, 200, "{err}");
    let want = if cfg!(windows) { "got UpArrow,UpArrow,UpArrow" } else { "got 033 [ A 033 [ A 033 [ A" };
    h.wait_capture("wh:full", "the program got the wheel as Up keys", |t| t.contains(want)).await;
    // The same word from the command line; on a normal screen it is nothing
    // (keepane's history is what scrolls there).
    assert_eq!(h.cli(&["send-keys", "-t", "wh:0", "WheelUp"]).await.0, 0);
    h.cli(&["kill-server"]).await;
}

/// The phone sends a message into a pane's inbox (from the user, queued as
/// any other), sets what a pane does with its messages, starts a session in
/// the home directory, and runs again a pane whose program ended (the list
/// says how it ended). Read-only, none of it.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_sends_messages_sets_modes_starts_sessions_and_runs_again() {
    let h = Harness::start("webmore").await;
    h.cli(&["new", "-d", "-s", "m"]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let key = "more-key";
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, key, false))));
    let p = pane_id(&h, "m:0.0").await;
    let t = format!("%{p}");
    let mode = async || h.cli(&["display", "-p", "-t", &t, "#{pane_work_mode}"]).await.1.trim().to_string();

    // Modes.
    let (code, err) = http(addr, "POST", &format!("/api/action?pane=%25{p}&do=mode&mode=ai"), key, "").await;
    assert_eq!(code, 200, "{err}");
    assert_eq!(mode().await, "ai");
    assert_eq!(http(addr, "POST", &format!("/api/action?pane=%25{p}&do=mode&mode=root"), key, "").await.0, 400);
    assert_eq!(http(addr, "POST", &format!("/api/action?pane=%25{p}&do=mode"), key, "").await.0, 400);
    assert_eq!(mode().await, "ai", "unchanged by the bad ones");

    // A message: queued in its inbox, from the user.
    let (code, said) = http(addr, "POST", &format!("/api/tell?pane=%25{p}"), key, "look at the logs").await;
    assert_eq!(code, 200, "{said}");
    let id = said.trim_start_matches('#').split(|c: char| !c.is_ascii_digit()).next().unwrap().to_string();
    assert!(!id.is_empty(), "the answer names the message: {said}");
    let trace = h.cli(&["trace-message", &id]).await.1;
    assert!(trace.contains("from=user") && trace.contains("look at the logs"), "{trace}");
    assert_eq!(http(addr, "POST", &format!("/api/tell?pane=%25{p}"), key, "  ").await.0, 400, "nothing to send");
    assert_eq!(http(addr, "POST", "/api/tell?pane=%25999", key, "x").await.0, 400, "no such pane");

    // A session of its own, named or not, in the home directory.
    let (code, err) = http(addr, "POST", "/api/new-session", key, "fromphone").await;
    assert_eq!(code, 200, "{err}");
    assert!(h.cli(&["ls"]).await.1.contains("fromphone:"));
    let home = dirs::home_dir().unwrap().to_string_lossy().to_lowercase();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let dir = h.cli(&["display", "-p", "-t", "fromphone:0.0", "#{pane_current_path}"]).await.1;
        if dir.trim().to_lowercase().trim_end_matches(['/', '\\']) == home.trim_end_matches(['/', '\\']) {
            break;
        }
        assert!(Instant::now() < deadline, "started in {dir:?}, not {home:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let before = h.cli(&["ls"]).await.1.lines().count();
    assert_eq!(http(addr, "POST", "/api/new-session", key, "").await.0, 200, "keepane names it");
    assert_eq!(h.cli(&["ls"]).await.1.lines().count(), before + 1);
    assert_eq!(http(addr, "POST", "/api/new-session", key, "two\nlines").await.0, 400);
    assert_eq!(http(addr, "POST", "/api/new-session", key, "fromphone").await.0, 400, "taken");

    // A program that ended: the list says how; run again.
    h.cli(&["set", "-g", "remain-on-exit", "on"]).await;
    let ends: &[&str] = if cfg!(windows) { &["cmd", "/c", "exit 3"] } else { &["sh", "-c", "exit 3"] };
    let (code, _, err) = h.cli(&[&["new-window", "-d", "-t", "m", "-n", "ends"], ends].concat()).await;
    assert_eq!(code, 0, "{err}");
    let dead = pane_id(&h, "m:ends.0").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let entry = loop {
        let (_, list) = http(addr, "GET", "/api/panes", key, "").await;
        let v: serde_json::Value = serde_json::from_str(&list).unwrap();
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == format!("%{dead}")).cloned().unwrap();
        if e["dead"] == true {
            break e;
        }
        assert!(Instant::now() < deadline, "never ended: {e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(entry["exit"], 3, "{entry}");
    let alive = |e: &serde_json::Value| e["exit"].is_null() && e["dead"] == false;
    let (_, list) = http(addr, "GET", "/api/panes", key, "").await;
    let v: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert!(v.as_array().unwrap().iter().any(|e| e["id"] == t && alive(e)), "a live pane: exit null: {list}");
    let pid = async || h.cli(&["display", "-p", "-t", &format!("%{dead}"), "#{pane_pid}"]).await.1.trim().to_string();
    let first = pid().await;
    let (code, err) = http(addr, "POST", &format!("/api/action?pane=%25{dead}&do=respawn"), key, "").await;
    assert_eq!(code, 200, "{err}");
    assert_ne!(pid().await, first, "a new program");
    let (code, err) = http(addr, "POST", &format!("/api/action?pane=%25{p}&do=respawn"), key, "").await;
    assert_eq!(code, 400, "a pane still running is not run again: {err}");

    // Read-only: none of it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ro = listener.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, key, true))));
    assert_eq!(http(ro, "POST", &format!("/api/tell?pane=%25{p}"), key, "x").await.0, 403);
    assert_eq!(http(ro, "POST", "/api/new-session", key, "ro").await.0, 403);
    assert_eq!(http(ro, "POST", &format!("/api/action?pane=%25{p}&do=mode&mode=shell"), key, "").await.0, 403);
    assert_eq!(http(ro, "POST", &format!("/api/action?pane=%25{dead}&do=respawn"), key, "").await.0, 403);
    assert_eq!(mode().await, "ai");
    h.cli(&["kill-server"]).await;
}

/// Ctrl+C stops what a pane runs. On Windows the server's own process group
/// ignores Ctrl+C and its children inherited that, so a `ping` in a pane ran
/// on through every Ctrl+C (from a key, `send-keys C-c` or the phone).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_stops_what_a_pane_runs() {
    // As `keepane` starts the server (`spawn_self`, a process group of its
    // own): Ctrl+C ignored, which the panes would inherit. The server must
    // turn it back on (this test's servers run in this process).
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1) };
    let h = Harness::start("ctrlc").await;
    let (code, _, err) = h.cli(&[&["new", "-d", "-s", "c"], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let p = pane_id(&h, "c:0.0").await;
    let t = format!("%{p}");
    h.wait_capture("c:0", "the prompt", |s| s.contains('>')).await;
    let running = async |yes: bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let (_, prog, _) = h.cli(&["display", "-p", "-t", &t, "#{pane_pid_command}"]).await;
            if prog.to_ascii_lowercase().contains("ping") == yes {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "ping {} (the pane runs {prog:?})",
                if yes { "never ran" } else { "ran on" }
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    h.cli(&["send-keys", "-t", &t, "ping -n 120 127.0.0.1", "Enter"]).await;
    running(true).await;
    h.cli(&["send-keys", "-t", &t, "C-c"]).await;
    running(false).await;
    h.cli(&["kill-server"]).await;
}

/// A phone that wraps lines at its own width asks for them joined; a
/// command's time then goes on the joined line that holds the whole
/// command, not on a piece of it.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_gets_joined_lines_with_their_command_times() {
    let h = Harness::start("webjoin").await;
    // A short prompt (`PS C:\> `), as on any machine: the command, not a
    // long path, is what wraps.
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let (code, _, err) =
        h.cli(&[&["new", "-d", "-s", "j", "-x", "30", "-y", "20", "-c", root], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let p = pane_id(&h, "j:0.0").await;
    let say = if cfg!(windows) { "Write-Output" } else { "echo" };
    let command = format!("{say} joined-{}-end", "w".repeat(40));
    h.cli(&["set-work-mode", "-t", &format!("%{p}"), "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["send-keys", "-t", &format!("%{p}"), &command, "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    // Marked as done: a mark with its end time (`row start end exit said`;
    // `said` is the command's first row, which may hold little of it).
    let done = |marks: &str| marks.lines().any(|l| l.split(' ').nth(2).is_some_and(|e| e != "-"));
    let output = format!("joined-{}-end", "w".repeat(40));
    let printed = async || {
        let (_, screen, _) = h.cli(&["capture-pane", "-p", "-J", "-t", &format!("%{p}")]).await;
        screen.lines().any(|l| l.trim_end() == output)
    };
    // Every shell's command is timed: bash before 4.4 (macOS's own) from
    // Enter at its prompt.
    let timed = true;
    while !(printed().await && (!timed || done(&h.cli(&["list-marks", "-t", &format!("%{p}")]).await.1))) {
        assert!(Instant::now() < deadline, "the command was never marked");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, "k", false))));
    let (code, body) = http(addr, "GET", &format!("/api/screen?pane=%25{p}&history=50&join=1"), "k", "").await;
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let lines: Vec<&str> = v["text"].as_str().unwrap().split('\n').collect();
    let marks = v["marks"].as_array().unwrap();
    // The line as read: its colours taken out.
    let plain = |s: &str| {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                it.by_ref().take_while(|c| !c.is_ascii_alphabetic()).for_each(drop);
            } else {
                out.push(c);
            }
        }
        out
    };
    let on = |m: &serde_json::Value| plain(lines.get(m[0].as_u64().unwrap() as usize).copied().unwrap_or(""));
    assert!(lines.iter().any(|l| plain(l).contains(&command)), "the command is not one line:\n{lines:#?}");
    if timed {
        assert!(marks.iter().any(|m| on(m).contains(&command)), "no mark on the whole command:\n{lines:#?}\n{marks:?}");
    }
    h.cli(&["kill-server"]).await;
}

/// `keepane web` pushes a watched pane's screen when it changes, says when the
/// pane is gone, and the list carries each window's alert marks.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_page_is_pushed_changes_and_sees_alerts() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Harness::start("webpush").await;
    h.cli(&["set", "-g", "monitor-activity", "on"]).await;
    h.cli(&["new", "-d", "-s", "p"]).await;
    h.cli(&["new-window", "-d", "-t", "p"]).await;
    h.wait_capture("p:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.wait_capture("p:1", "shell prompt", |t| t.contains("keepane>")).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let key = "push-test-key";
    tokio::spawn(keepane::web::serve(listener, std::sync::Arc::new(keepane::web::State::new(&h.socket, key, false))));
    let (_, list) = http(addr, "GET", "/api/panes", key, "").await;
    let ids: Vec<String> = list.split("\"id\":\"%").skip(1).map(|s| s.split('"').next().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 2, "{list}");
    let (front, back) = (format!("%25{}", ids[0]), format!("%25{}", ids[1]));

    // Watching needs the key too.
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET /api/watch?pane={front} HTTP/1.1\r\nX-Keepane-Key: nope\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut refused = String::new();
    s.read_to_string(&mut refused).await.unwrap();
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");

    // The stream opens with the screen as it is.
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET /api/watch?pane={front} HTTP/1.1\r\nX-Keepane-Key: {key}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut got = String::new();
    let mut buf = [0u8; 8192];
    let mut read_until =
        async |s: &mut tokio::net::TcpStream, got: &mut String, what: &str, pred: &dyn Fn(&str) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !pred(got) {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "timeout waiting for {what}: {got}");
                match tokio::time::timeout(left, s.read(&mut buf)).await {
                    Ok(Ok(0)) | Err(_) => panic!("stream ended waiting for {what}: {got}"),
                    Ok(Ok(n)) => got.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Ok(Err(e)) => panic!("{e}"),
                }
            }
        };
    read_until(&mut s, &mut got, "the first screen", &|g| g.contains("text/event-stream") && g.contains("data: "))
        .await;
    assert!(got.contains("keepane>"), "{got}");

    // Something typed shows up without asking again, and soon.
    let sent = Instant::now();
    http(addr, "POST", &format!("/api/send?pane={front}"), key, "echo pushed-9").await;
    http(addr, "POST", &format!("/api/send?pane={front}&key=Enter"), key, "").await;
    read_until(&mut s, &mut got, "the pushed output", &|g| g.contains("pushed-9\\n")).await;
    let latency = sent.elapsed();
    assert!(latency < Duration::from_secs(3), "pushed after {latency:?}");

    // A background window that prints is marked in the list.
    h.cli(&["send-keys", "-t", "p:1", "echo busy", "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, list) = http(addr, "GET", "/api/panes", key, "").await;
        if list.contains("\"activity\":true") {
            break;
        }
        assert!(Instant::now() < deadline, "no activity mark: {list}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Closing the watched pane ends the stream with `gone`.
    h.cli(&["kill-pane", "-t", &front.replace("%25", "%")]).await;
    read_until(&mut s, &mut got, "gone", &|g| g.contains("event: gone")).await;
    let _ = back;
    h.cli(&["kill-server"]).await;
}

/// The server serves the page in the background (`web-start`), says who
/// is on it (`web-status`, `#{web_clients}`, the dashboard's line), and
/// `web-stop` closes the port and cuts off a phone that was watching.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_page_runs_in_the_server_and_says_who_is_on_it() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Harness::start("webhost").await;
    h.cli(&["new", "-d", "-s", "b"]).await;
    h.wait_capture("b:0", "shell prompt", |t| t.contains("keepane>")).await;
    assert_eq!(h.cli(&["web-status"]).await.1.trim(), "off: `keepane web` starts it");
    let (code, status, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1"]).await;
    assert_eq!(code, 0, "{err}");
    let url = keepane::web::status_url(&status).expect(&status).to_string();
    let addr: std::net::SocketAddr = url["http://".len()..].split('/').next().unwrap().parse().unwrap();
    let key = url.split("#k=").nth(1).unwrap().to_string();
    assert_eq!(h.cli(&["display", "-p", "#{web_clients} #{web_url}"]).await.1.trim(), format!("0 {url}"));

    // The pane list alone makes a phone connected; a wrong key is listed.
    let (code, list) = http(addr, "GET", "/api/panes", &key, "").await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(http(addr, "GET", "/api/panes", "wrong", "").await.0, 401);
    let status_until = async |what: &str, pred: &dyn Fn(&str) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, status, _) = h.cli(&["web-status"]).await;
            if pred(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "{what}: {status}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let status = status_until("the list asked for", &|s| s.contains("refused")).await;
    assert!(status.lines().next().unwrap().ends_with(" · 1 connected"), "{status}");
    assert!(status.contains("127.0.0.1        on the pane list"), "{status}");
    assert!(status.ends_with("refused (no key or a wrong one): 127.0.0.1"), "{status}");
    assert_eq!(h.cli(&["display", "-p", "#{web_clients}"]).await.1.trim(), "1");
    let (_, said, _) = h.cli(&["show-messages"]).await;
    assert!(said.contains("web: 127.0.0.1 connected"), "{said}");

    // A pane's stream is listed while it is open, and not once it closes.
    let id = list.split("\"id\":\"%").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
    let watching = format!("watching %{id}");
    let watch = async || {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET /api/watch?pane=%25{id} HTTP/1.1\r\nX-Keepane-Key: {key}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        s
    };
    let s = watch().await;
    status_until("the stream listed", &|s| s.contains(&watching)).await;
    drop(s);
    status_until("the closed stream no longer listed", &|s| !s.contains(&watching)).await;
    let mut s = watch().await;
    status_until("the second stream listed", &|s| s.contains(&watching)).await;

    // Stopped: the watching phone is cut off, and nothing answers.
    assert_eq!(h.cli(&["web-stop"]).await.0, 0);
    let mut buf = [0u8; 65536];
    let cut = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(cut.is_ok(), "the stream outlived web-stop");
    // Closed; a pane another test forks just then may hold the socket a
    // moment (until it execs), so a while rather than at once.
    let deadline = Instant::now() + Duration::from_secs(2);
    while tokio::net::TcpStream::connect(addr).await.is_ok() {
        assert!(Instant::now() < deadline, "the port is still open");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(h.cli(&["display", "-p", "[#{web_url}]"]).await.1.trim(), "[]");
    // Answered once the port is closed: the same port at once is free.
    let port = addr.port().to_string();
    for _ in 0..3 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (code, _, err) = h.cli(&["web-start", "-p", &port, "-b", "127.0.0.1"]).await;
            if code == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "the port was not free again: {err}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(h.cli(&["web-stop"]).await.0, 0);
    }
    assert_eq!(h.cli(&["web-stop"]).await.0, 1);
    h.cli(&["kill-server"]).await;
}

/// The phone's fit (`web-fit`, the page's ⤢): the pane fills its window and
/// the session takes the phone's size; undone, both come back; a fit no phone
/// shows any more ends by itself; a size set by hand takes over; read-only
/// fits nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_pane_fitted_to_a_phone_gets_its_size_back() {
    use tokio::io::AsyncWriteExt;
    let h = Harness::start("webfit").await;
    h.cli(&["new", "-d", "-s", "f", "-x", "100", "-y", "30"]).await;
    h.wait_capture("f:0", "shell prompt", |t| t.contains("keepane>")).await;
    h.cli(&["split-window", "-h", "-d", "-t", "f:0"]).await;
    let p = pane_id(&h, "f:0.1").await;
    let size = async || {
        h.cli(&[
            "display",
            "-p",
            "-t",
            &format!("%{p}"),
            "#{window_width}x#{window_height} #{pane_width}x#{pane_height} #{window_zoomed_flag}",
        ])
        .await
        .1
        .trim()
        .to_string()
    };
    let before = size().await;
    assert!(before.starts_with("100x30 ") && before.ends_with(" 0"), "{before}");
    let (code, _, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1"]).await;
    assert_eq!(code, 0, "{err}");
    let status = h.cli(&["web-status"]).await.1;
    let url = keepane::web::status_url(&status).unwrap().to_string();
    let addr: std::net::SocketAddr = url["http://".len()..].split('/').next().unwrap().parse().unwrap();
    let key = url.split("#k=").nth(1).unwrap().to_string();

    // Fitted: zoomed, 40 columns, 20 rows for the pane (the status line on top).
    let (code, body) = http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await;
    assert!(code == 200 && body.contains("fitted to 40x20"), "{code} {body}");
    assert_eq!(size().await, "40x21 40x20 1");
    let (_, said, _) = h.cli(&["show-messages"]).await;
    assert!(said.contains("web: a phone fitted session f to 40x20"), "{said}");
    // Undone: the size and the layout it had.
    let (code, _) = http(addr, "POST", &format!("/api/fit?pane=%25{p}&off=1"), &key, "").await;
    assert_eq!(code, 200);
    assert_eq!(size().await, before);
    // The window zoomed on its other pane before: after, it is again.
    let other = pane_id(&h, "f:0.0").await;
    assert_eq!(h.cli(&["resize-pane", "-Z", "-t", &format!("%{other}")]).await.0, 0);
    let state = async || ask_pane(&h, other, "#{window_zoomed_flag} #{pane_active}").await;
    assert_eq!(state().await, "1 1");
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await;
    assert_eq!(state().await, "1 0", "the fitted pane is the one shown");
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&off=1"), &key, "").await;
    assert_eq!(state().await, "1 1", "zoomed on the other pane again");
    h.cli(&["resize-pane", "-Z", "-t", &format!("%{other}")]).await;
    assert_eq!(size().await, before);

    // Left: fitted while a phone watches it, back once none has for a while.
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET /api/watch?pane=%25{p} HTTP/1.1\r\nX-Keepane-Key: {key}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(size().await, "40x21 40x20 1", "watched: it holds");
    drop(s);
    let deadline = Instant::now() + Duration::from_secs(20);
    while size().await != before {
        assert!(Instant::now() < deadline, "not back: {}", size().await);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // A terminal attaching meanwhile (its own size, 80x24) does not undo it:
    // the fit holds the session until it ends.
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await;
    let mut term = h.connect().await;
    term.attach(&["attach", "-t", "f"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(size().await, "40x21 40x20 1", "a terminal attaching keeps the phone's size");
    drop(term);
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&off=1"), &key, "").await;

    // Sized by hand: the fit gives way, and ends no size later.
    http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await;
    assert_eq!(h.cli(&["resize-window", "-t", "f", "-x", "90", "-y", "25"]).await.0, 0);
    assert!(size().await.starts_with("90x25 "), "{}", size().await);
    tokio::time::sleep(Duration::from_secs(12)).await;
    assert!(size().await.starts_with("90x25 "), "the hand's size stays: {}", size().await);

    // Bad sizes, and read-only.
    assert_eq!(http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=0&rows=20"), &key, "").await.0, 400);
    assert_eq!(http(addr, "GET", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await.0, 405);
    assert_eq!(h.cli(&["web-stop"]).await.0, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        let (code, status, _) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1", "-r"]).await;
        if code == 0 {
            break status;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let url = keepane::web::status_url(&status).unwrap().to_string();
    let addr: std::net::SocketAddr = url["http://".len()..].split('/').next().unwrap().parse().unwrap();
    let key = url.split("#k=").nth(1).unwrap().to_string();
    assert_eq!(http(addr, "POST", &format!("/api/fit?pane=%25{p}&cols=40&rows=20"), &key, "").await.0, 403);
    // The command's own words.
    for (argv, why) in [
        (vec!["web-fit", "-t", "f:0.1"], "-x cols -y rows, or -u"),
        (vec!["web-fit", "-x", "40", "-y", "20", "-u"], "-x cols -y rows, or -u"),
        (vec!["web-fit", "-x", "0", "-y", "20"], "takes a size"),
    ] {
        let (code, _, err) = h.cli(&argv).await;
        assert!(code != 0 && err.contains(why), "{argv:?}: {err}");
    }
    h.cli(&["kill-server"]).await;
}

/// One request to another server's `/link/` paths the way its keepane
/// would make it, signed with `id`: (status, body).
#[allow(clippy::too_many_arguments)]
async fn link_call(
    id: &keepane::link::Identity,
    to_key: &str,
    addr: &str,
    method: &'static str,
    path: &'static str,
    time: i64,
    nonce: &str,
    body: &str,
) -> (u16, String) {
    use keepane::link as l;
    let from = id.public();
    let text = l::request_text(method, path, &from, 9, to_key, time, nonce, body.as_bytes());
    let headers = vec![
        (l::H_PROTOCOL, l::PROTOCOL.to_string()),
        (l::H_VERSION, "test".to_string()),
        (l::H_FROM, from),
        (l::H_PORT, "9".to_string()),
        (l::H_TIME, time.to_string()),
        (l::H_NONCE, nonce.to_string()),
        (l::H_SIGN, id.sign(&text)),
    ];
    let a = l::call(addr, method, path, &headers, body.as_bytes(), Duration::from_secs(5)).await.unwrap();
    (a.status, a.text())
}

/// Two servers in this process stand for two machines (docs/design/link.md):
/// paired once through one's web address, their panes message each other
/// and answer, a `-w` waits there, the hop limit holds across, a `shell`
/// pane takes commands only from a machine allowed to, a stranger's or a
/// stale or repeated request is refused, a new web key changes nothing,
/// unpairing works on both, a read-only web takes nothing in, and a machine
/// that is off is an error at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_allowed_to_starts_panes_on_another_and_closes_only_those() {
    let dir = std::env::temp_dir().join(format!("keepane-test-link-{}", std::process::id()));
    unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
    let a = Harness::start("lpa").await;
    let b = Harness::start("lpb").await;
    a.cli(&["new", "-d", "-s", "wa"]).await;
    b.cli(&["new", "-d", "-s", "wb"]).await;
    b.wait_capture("wb:0", "shell prompt", |t| t.contains("keepane>")).await;
    let own = pane_id(&b, "wb:0.0").await;
    let web = async |h: &Harness| {
        let (code, status, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1"]).await;
        assert_eq!(code, 0, "{err}");
        let url = keepane::web::status_url(&status).expect(&status).to_string();
        let addr = url["http://".len()..].split('/').next().unwrap().to_string();
        (url, addr)
    };
    let (_, addr_a) = web(&a).await;
    let (ub, addr_b) = web(&b).await;
    let (code, _, err) = a.cli(&["link-add", &ub]).await;
    assert_eq!(code, 0, "{err}");
    // Not allowed yet: said how to allow it.
    let (code, _, err) = a.cli(&["link-start", &addr_b]).await;
    assert!(code != 0 && err.contains(&format!("keepane link allow {addr_a} --panes")), "{err}");
    // Only from outside every pane.
    let (code, _, err) = b.cli_in(Some(own), &["link-allow", &addr_a, "--panes"]).await;
    assert!(code != 0 && err.contains("pane"), "{err}");
    let (code, out, err) = b.cli(&["link-allow", &addr_a, "--panes"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("may start panes here"), "{out}");
    // Started there, in a session named after this machine; its address back.
    // A program on its agent-commands (its default shell here is cmd, which is not).
    let (code, out, err) =
        a.cli(&[&["link-start", &addr_b, "-n", "helper", "-m", "ai", "--"], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let address = out.split_whitespace().next().unwrap().to_string();
    assert!(address.starts_with(&format!("{addr_b}/$")) && out.contains("helper"), "{out}");
    let pid: u32 = address.rsplit_once('%').unwrap().1.parse().unwrap();
    let (_, sessions, _) = b.cli(&["list-sessions"]).await;
    let host = keepane::sysinfo::hostname();
    assert!(sessions.lines().count() == 2, "{sessions}");
    let (_, session, _) = b.cli(&["display", "-p", "-t", &format!("%{pid}"), "#{session_name}"]).await;
    assert!(!session.trim().is_empty() && session.trim() != "wb", "a session of its own for {host}: {session}");
    // It takes messages at that address (in ai mode: a shell pane would
    // need --shell as well, a permission of its own).
    let (code, _, err) = a.cli(&["send-message", "--to", &address, "hello"]).await;
    assert_eq!(code, 0, "{err}");
    // Only this machine's agent-commands, and within agent-pane-limit.
    let (code, _, err) = a.cli(&["link-start", &addr_b, "--", "notepad-not-allowed"]).await;
    assert!(code != 0 && err.contains("not in agent-commands"), "{err}");
    b.cli(&["set", "-g", "agent-pane-limit", "1"]).await;
    let (code, _, err) = a.cli(&[&["link-start", &addr_b, "--"], HOOKED_SHELL].concat()).await;
    assert!(code != 0 && err.contains("agent-pane-limit 1 reached"), "{err}");
    // It closes the pane it started, and no other.
    let (code, _, err) = a.cli(&["link-kill", &format!("{addr_b}/%{own}")]).await;
    assert!(code != 0 && err.contains("may close only the panes it started"), "{err}");
    let (code, out, err) = a.cli(&["link-kill", &address]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("closed "), "{out}");
    assert!(b.cli(&["display", "-p", "-t", &format!("%{pid}"), "#{pane_id}"]).await.0 != 0, "gone there");
    a.cli(&["kill-server"]).await;
    b.cli(&["kill-server"]).await;
}

/// The chart with another machine paired: a row of machines under the
/// root (this one first), the other's sessions, windows and panes under it
/// by their names, Enter on one of its panes shows that pane's screen,
/// `x` touches nothing there, and a machine that went off says so.
#[tokio::test(flavor = "multi_thread")]
async fn the_chart_shows_the_paired_machines() {
    let dir = std::env::temp_dir().join(format!("keepane-test-link-{}", std::process::id()));
    unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
    let a = Harness::start("lca").await;
    let b = Harness::start("lcb").await;
    let mut c = a.connect().await;
    c.attach(&["new", "-s", "wa"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    b.cli(&["new", "-d", "-s", "wb", "-n", "edit"]).await;
    b.cli(&["split-window", "-t", "wb"]).await;
    b.wait_capture("wb:0.1", "shell prompt", |t| t.contains("keepane>")).await;
    let web = async |h: &Harness| {
        let (code, status, err) = h.cli(&["web-start", "-p", "0", "-b", "127.0.0.1"]).await;
        assert_eq!(code, 0, "{err}");
        let url = keepane::web::status_url(&status).expect(&status).to_string();
        let addr = url["http://".len()..].split('/').next().unwrap().to_string();
        (url, addr)
    };
    let (_, addr_a) = web(&a).await;
    let (ub, addr_b) = web(&b).await;
    let (code, _, err) = a.cli(&["link-add", &ub]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = b.cli(&["link-allow", &addr_a, "--screen"]).await;
    assert_eq!(code, 0, "{err}");
    // root, this machine, wa, wa:0, its pane; b, wb, wb:0 (edit), its two
    // panes: 10, the cursor on wa's pane (5).
    c.prefix('w').await;
    let at = |n: usize| move |s: &vt100::Screen| s.contents().contains(&format!("[{n}/10] hjkl move"));
    c.wait_for("the machines", |s| {
        let t = s.contents();
        at(5)(s)
            && t.matches("host").count() == 2
            && t.contains(&addr_b)
            && t.contains("1 session")
            && !t.contains("asking")
    })
    .await;
    assert!(c.text().contains("this machine · 1 session"), "{}", c.text());
    // Up to this machine (never the root), along to the other.
    for want in [4, 3, 2, 2] {
        c.key(0x26, '\0', 0).await;
        c.wait_for("up", at(want)).await;
    }
    c.key(0x27, '\0', 0).await;
    c.wait_for("the other machine: its session, by name", |s| at(6)(s) && s.contents().contains("6  wb")).await;
    // Down: its session, its current window, that window's active pane
    // (the second, made by the split).
    for want in [7, 8, 10] {
        c.key(0x28, '\0', 0).await;
        c.wait_for("down", at(want)).await;
    }
    assert!(c.text().contains("7  0:edit*"), "{}", c.text());
    // x leaves it alone.
    c.type_str("x").await;
    c.wait_for("refused", |s| s.contents().contains("x closes what is on this machine only")).await;
    assert_eq!(b.cli(&["list-panes", "-t", "wb"]).await.1.lines().count(), 2);
    // Enter: its screen over the chart; a key and the chart is back.
    c.enter().await;
    c.wait_for("its screen", |s| {
        let t = s.contents();
        t.contains(&format!("{addr_b}/$")) && t.contains("keepane>")
    })
    .await;
    c.type_str("q").await;
    c.wait_for("the chart again", at(10)).await;
    // A filter keeps what matches with the machine, session and window
    // above it: root, b, wb, edit, its two panes (this machine has none).
    c.type_str("f").await;
    c.type_str("edit").await;
    c.enter().await;
    c.wait_for("filtered", |s| {
        let t = s.contents();
        t.contains("[6/6] hjkl move") && t.contains("2  wb") && t.contains("3  0:edit*") && !t.contains("this machine")
    })
    .await;
    // The other machine goes: said on its block, its panes as last heard.
    b.cli(&["kill-server"]).await;
    c.wait_for("offline", |s| s.contents().contains("1 session · offline")).await;
    c.type_str("q").await;
    a.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn panes_on_two_machines_pass_messages_once_paired() {
    let dir = std::env::temp_dir().join(format!("keepane-test-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
    let a = Harness::start("linka").await;
    let b = Harness::start("linkb").await;
    a.cli(&["new", "-d", "-s", "wa"]).await;
    b.cli(&["new", "-d", "-s", "wb"]).await;
    a.wait_capture("wa:0", "shell prompt", |t| t.contains("keepane>")).await;
    b.wait_capture("wb:0", "shell prompt", |t| t.contains("keepane>")).await;
    let (pa, pb) = (pane_id(&a, "wa:0.0").await, pane_id(&b, "wb:0.0").await);
    let (ta, tb) = (format!("%{pa}"), format!("%{pb}"));
    a.cli(&["rename-pane", "-t", &ta, "lead"]).await;
    b.cli(&["rename-pane", "-t", &tb, "worker"]).await;
    assert_eq!(a.cli(&["set-work-mode", "-t", &ta, "ai"]).await.0, 0);
    assert_eq!(b.cli(&["set-work-mode", "-t", &tb, "ai"]).await.0, 0);

    // The link goes through `keepane web`: without it, nothing to answer to.
    let (code, _, err) = a.cli(&["link-add", "http://127.0.0.1:1/#k=AAAAAAAAAAAAAAAAAAAAAA"]).await;
    assert!(code != 0 && err.contains("keepane web") && err.contains("running here"), "{err}");
    let web = async |h: &Harness, flags: &[&str]| {
        let argv: Vec<&str> =
            ["web-start", "-p", "0", "-b", "127.0.0.1"].into_iter().chain(flags.iter().copied()).collect();
        let (code, status, err) = h.cli(&argv).await;
        assert_eq!(code, 0, "{err}");
        let url = keepane::web::status_url(&status).expect(&status).to_string();
        let addr = url["http://".len()..].split('/').next().unwrap().to_string();
        (url, addr)
    };
    let (_ua, addr_a) = web(&a, &[]).await;
    let (ub, addr_b) = web(&b, &[]).await;
    let worker = format!("{addr_b}/%worker");
    let (code, _, err) = a.cli(&["send-message", "--to", &worker, "hi"]).await;
    assert!(code != 0 && err.contains("not paired"), "{err}");
    // Only send-message goes to another machine: nothing else acts on the
    // local pane the rest of the address would name.
    for argv in [
        vec!["send-keys", "-t", &worker, "x"],
        vec!["kill-pane", "-t", &worker],
        vec!["rename-pane", "-t", &worker, "w"],
    ] {
        let (code, _, err) = a.cli(&argv).await;
        assert!(code != 0 && err.contains("another machine"), "{argv:?}: {err}");
    }

    // Pairing needs the other side's web key; with it, once, both ways.
    let wrong = format!("{}#k=AAAAAAAAAAAAAAAAAAAAAA", ub.split("#k=").next().unwrap());
    let (code, _, err) = a.cli(&["link-add", &wrong]).await;
    assert!(code != 0 && err.contains("web key"), "{err}");
    // Refused there, not only doubted here: nothing got into either table.
    assert!(err.contains("without this machine's web key"), "refused by the other side: {err}");
    assert!(b.cli(&["link-list"]).await.1.starts_with("no machines paired"));
    assert!(a.cli(&["link-list"]).await.1.starts_with("no machines paired"));
    let (code, out, err) = a.cli(&["link-add", &ub]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with(&format!("paired with {addr_b}  SHA256:")), "{out}");
    let (_, list, _) = a.cli(&["link-list"]).await;
    assert!(list.contains(&addr_b) && list.contains("no"), "{list}");
    let (_, list, _) = b.cli(&["link-list"]).await;
    assert!(list.contains(&addr_a), "{list}");
    let (_, said, _) = b.cli(&["show-messages"]).await;
    assert!(said.contains(&format!("link: {addr_a} (SHA256:")), "{said}");
    let (code, panes, err) = a.cli(&["link-panes", &addr_b]).await;
    assert!(code == 0 && panes.contains("worker") && panes.contains("\tai\t"), "{panes} {err}");

    // A message goes over, queued there until the agent is ready; its
    // record here says so, and there it reads from=<this machine>/<pane>.
    let (code, out, err) = a.cli_in(Some(pa), &["send-message", "--to", &worker, "hello worker"]).await;
    assert_eq!(code, 0, "{err}");
    let first = msg_id(&out);
    assert!(
        out.contains("forwarded: #")
            && out.contains("queued for")
            && out.contains(&format!("(ai, busy, 0 ahead) on {addr_b}")),
        "{out}"
    );
    let (_, trace, _) = a.cli(&["trace-message", &first]).await;
    assert!(
        trace.starts_with(&format!("#{first} forwarded")) && trace.contains(&format!("to={addr_b}/%worker")),
        "{trace}"
    );
    let (_, inbox, _) = b.cli(&["list-messages", "-t", &tb]).await;
    assert!(
        inbox.contains(&format!("from {addr_a}/$")) && inbox.contains(" lead (ai)") && inbox.contains("hello worker"),
        "{inbox}"
    );
    assert_eq!(b.cli_in(Some(pb), &["pane-ready"]).await.0, 0);
    wait_format(&b, pb, "#{pane_inbox}", "0").await;
    // The envelope line is wider than the pane, so it is read with -J.
    b.wait_capture("wb:0", "the message", |t| t.contains("hello worker")).await;
    let (_, joined, _) = b.cli(&["capture-pane", "-p", "-J", "-t", "wb:0"]).await;
    assert!(joined.contains(&format!("from={addr_a}/$")) && joined.contains(" name=lead mode=ai to="), "{joined}");

    // The answer goes back to the sender's machine and joins its task, found
    // by its pane's id, even after that pane moved to another window.
    let window_before = ask_pane(&a, pa, "#{window_id}").await;
    a.cli(&["new-window", "-d", "-t", "wa"]).await;
    let (code, _, err) = a.cli(&["join-pane", "-d", "-s", &ta, "-t", "wa:1"]).await;
    assert_eq!(code, 0, "{err}");
    assert_ne!(ask_pane(&a, pa, "#{window_id}").await, window_before, "moved to another window");
    let (code, out, err) = b.cli_in(Some(pb), &["send-message", "-r", "got it"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("on {addr_a}")), "{out}");
    let (_, task, _) = a.cli(&["show-task", &first]).await;
    assert!(task.contains("hello worker") && task.contains("got it") && task.contains("%worker → "), "{task}");
    let (_, inbox, _) = a.cli(&["list-messages", "-t", &ta]).await;
    assert!(inbox.contains("got it") && inbox.contains(&format!("from {addr_b}/$")), "{inbox}");
    let reply_on_a = inbox.split("  #").nth(1).and_then(|s| s.split_whitespace().next()).unwrap().to_string();

    // `-w` waits on the other machine (busy: its agent has not said ready).
    let started = Instant::now();
    // Still queued when it runs out: a time-out, as for a local `-w`; the
    // message is there all the same, and the record here says so.
    let (code, _, err) = a.cli(&["send-message", "--to", &worker, "-w", "1", "more"]).await;
    assert!(code != 0 && err.starts_with("timed out: #") && err.contains("queued for"), "{err}");
    assert!(started.elapsed() >= Duration::from_millis(900), "waited there");
    let (_, trace, _) = a.cli(&["trace-message", &msg_id(err.trim_start_matches("timed out: "))]).await;
    assert!(trace.contains(" forwarded "), "{trace}");
    // The hop limit counts across machines: a third hop is refused here.
    a.cli(&["set", "-g", "message-hop-limit", "1"]).await;
    let (code, _, err) = a.cli(&["send-message", "--re", &reply_on_a, "and again"]).await;
    assert!(code != 0 && err.contains("message-hop-limit"), "{err}");
    a.cli(&["set", "-g", "message-hop-limit", "8"]).await;

    // A shell pane there takes commands from here only once this machine is
    // allowed to, by a person outside every pane.
    #[cfg(windows)]
    let (shell, echo): (&[&str], &str) = (&["pwsh", "-NoLogo", "-NoProfile"], "Write-Output linked-ok");
    #[cfg(unix)]
    let (shell, echo): (&[&str], &str) = (&["bash"], "echo linked-ok");
    let argv: Vec<&str> = ["new-window", "-d", "-t", "wb"].into_iter().chain(shell.iter().copied()).collect();
    b.cli(&argv).await;
    let ps = pane_id(&b, "wb:1.0").await;
    b.cli(&["rename-pane", "-t", &format!("%{ps}"), "sh"]).await;
    assert_eq!(b.cli(&["set-work-mode", "-t", &format!("%{ps}"), "shell"]).await.0, 0);
    wait_format(&b, ps, "#{pane_idle}", "1").await;
    let sh = format!("{addr_b}/%sh");
    let (code, _, err) = a.cli(&["send-message", "--to", &sh, echo]).await;
    assert!(code != 0 && err.contains(&format!("keepane link allow {addr_a} --shell")), "{err}");
    let (code, _, err) = b.cli_in(Some(pb), &["link-allow", &addr_a, "--shell"]).await;
    assert!(code != 0 && err.contains("not run from inside a pane"), "{err}");
    let (code, out, err) = b.cli(&["link-allow", &addr_a, "--shell"]).await;
    assert!(code == 0 && out.contains("may run commands"), "{out} {err}");
    // `-w` waits there until the shell is free and has it (a fresh shell may
    // still be busy drawing its prompt when the message lands).
    let (code, out, err) = a.cli(&["send-message", "--to", &sh, "-w", "30", echo]).await;
    assert!(code == 0 && out.contains("delivered to"), "{out} {err}");
    b.wait_capture("wb:1.0", "the command's output", |t| t.contains("linked-ok")).await;
    // What became of it there, asked from here: done, with what it printed.
    let ran = msg_id(&out);
    let ran_there = out.split("forwarded: #").nth(1).and_then(|s| s.split_whitespace().next()).unwrap().to_string();
    let (code, trace, err) = a.cli(&["trace-message", &ran, "-w", "30"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(trace.starts_with(&format!("#{ran} forwarded")) && trace.contains(&format!("on {addr_b}:\n#")), "{trace}");
    assert!(trace.contains(" done ") && trace.contains("output:\nlinked-ok"), "{trace}");

    // The machine, as it says it is.
    let (code, info, err) = a.cli(&["link-info", &addr_b]).await;
    assert_eq!(code, 0, "{err}");
    for field in ["host ", "system ", "keepane ", "up ", "cpu ", "memory ", "panes ", "time "] {
        assert!(info.lines().any(|l| l.starts_with(field)), "{field}: {info}");
    }
    assert!(info.contains(&format!("keepane  {}", env!("CARGO_PKG_VERSION"))), "{info}");
    // A pane's screen: only once that machine allows this one to read them.
    let (code, _, err) = a.cli(&["link-capture", &sh]).await;
    assert!(code != 0 && err.contains(&format!("keepane link allow {addr_a} --screen")), "{err}");
    assert_eq!(b.cli(&["link-allow", &addr_a, "--screen"]).await.0, 0);
    let (code, screen, err) = a.cli(&["link-capture", "-S", "100", &sh]).await;
    assert!(code == 0 && screen.contains("linked-ok"), "{screen} {err}");
    // Another machine's message is not this one's to ask after.
    let (_, _, err) = b.cli(&["trace-message", "999"]).await;
    assert!(err.contains("no message #999"), "{err}");

    // A stranger, a stale clock, a repeat, a signature for another request:
    // refused, and said on the status line.
    let stranger = keepane::link::Identity::make(&dir.join("stranger")).unwrap();
    let key_b = b.cli(&["link-id"]).await.1.split_whitespace().next().unwrap().to_string();
    let now = keepane::link::now();
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "GET", "/link/panes", now, "n1", "").await;
    assert!(status == 401 && body.contains("not paired"), "{status} {body}");
    let (code, _, err) = b.cli(&["link-trust", "127.0.0.1:9", &stranger.public()]).await;
    assert_eq!(code, 0, "{err}");
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "GET", "/link/panes", now - 300, "n2", "").await;
    assert!(status == 401 && body.contains("clocks"), "{status} {body}");
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "GET", "/link/panes", now, "n3", "").await;
    assert!(status == 200 && body.contains("worker"), "{status} {body}");
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "GET", "/link/panes", now, "n3", "").await;
    assert!(status == 401 && body.contains("seen before"), "{status} {body}");
    let (status, body) = link_call(&stranger, "not-their-key", &addr_b, "GET", "/link/panes", now, "n4", "").await;
    assert!(status == 401 && body.contains("not signed"), "{status} {body}");
    // Paired, but asking after another machine's message: not its to know;
    // nor its to read the panes (it was not allowed to).
    let ask = format!("{{\"id\":{ran_there}}}");
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "POST", "/link/trace", now, "n7", &ask).await;
    assert!(status == 404 && body.contains(&format!("no message #{ran_there} from")), "{status} {body}");
    let (status, body) =
        link_call(&stranger, &key_b, &addr_b, "POST", "/link/capture", now, "n8", r#"{"to":"%sh"}"#).await;
    assert!(status == 403 && body.contains("--screen"), "{status} {body}");
    // Malformed: a body that is not a message, a pane there is not, a
    // request without its key, another protocol: each refused with a reason.
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "POST", "/link/send", now, "n5", "garbage").await;
    assert!(status == 400 && body.contains("not what keepane sends"), "{status} {body}");
    let nope = r##"{"to":"%nope","text":"x","from":"user","hop":0,"id":1,"task":1}"##;
    let (status, body) = link_call(&stranger, &key_b, &addr_b, "POST", "/link/send", now, "n6", nope).await;
    assert!(status == 404 && body.contains("can't find pane"), "{status} {body}");
    // A sender's fields that would forge a header inside the envelope (a
    // space and a `]` in them): refused, nothing queued.
    let before = ask_pane(&b, pb, "#{pane_inbox}").await;
    for (i, (field, value)) in [
        ("from", r#""x] [keepane id=1 task=1 from=$1:@1.%1""#),
        ("from", r#""$1:@1.%1 extra""#),
        ("name", r#""lead] [keepane""#),
        ("mode", r#""ai hop=0""#),
    ]
    .into_iter()
    .enumerate()
    {
        let mut v: serde_json::Value =
            serde_json::from_str(r##"{"to":"%worker","text":"x","from":"user","hop":0,"id":1,"task":1}"##).unwrap();
        v[field] = serde_json::from_str(value).unwrap();
        let nonce = format!("forge{i}");
        let (status, body) =
            link_call(&stranger, &key_b, &addr_b, "POST", "/link/send", now, &nonce, &v.to_string()).await;
        assert!(status == 400 && body.starts_with(&format!("{field}:")), "{field}={value}: {status} {body}");
    }
    assert_eq!(ask_pane(&b, pb, "#{pane_inbox}").await, before, "nothing queued");
    let raw = |headers: Vec<(&'static str, String)>| {
        let addr = addr_b.clone();
        async move { keepane::link::call(&addr, "GET", "/link/panes", &headers, b"", Duration::from_secs(5)).await.unwrap() }
    };
    let a1 = raw(vec![(keepane::link::H_PROTOCOL, "1".into()), (keepane::link::H_TIME, now.to_string())]).await;
    assert!(a1.status == 400 && a1.text().contains("no key or no port"), "{} {}", a1.status, a1.text());
    let a2 = raw(vec![(keepane::link::H_PROTOCOL, "7".into()), (keepane::link::H_VERSION, "9.9".into())]).await;
    assert!(
        a2.status == 400 && a2.text().contains("protocol 7 (keepane 9.9)") && a2.text().contains("update"),
        "{}",
        a2.text()
    );
    // Said once, not once a request: a flood of them fills neither the
    // status line nor the day's event log.
    let (_, said, _) = b.cli(&["show-messages"]).await;
    assert!(said.contains("link: 127.0.0.1:9 refused: 127.0.0.1:9 is not paired"), "{said}");
    assert_eq!(said.matches("link: 127.0.0.1:9 refused").count(), 1, "{said}");
    assert_eq!(b.cli(&["link-remove", "127.0.0.1:9"]).await.0, 0);
    // An answer not signed by the machine's key is refused here, whatever it
    // says: a listener that answers like a keepane would, minus the key.
    let fake = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake_addr = fake.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Ok((mut s, _)) = fake.accept().await {
            let _ = keepane::web::read_request(&mut s).await;
            let body = r##"{"id":1,"to":"$1:@0.%1","via":"ai","stand":"#1 delivered to $1:@0.%1 (ai)"}"##;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(format!("{head}{body}").as_bytes()).await;
            let _ = s.shutdown().await;
        }
    });
    assert_eq!(a.cli(&["link-trust", &fake_addr, &stranger.public()]).await.0, 0);
    let (code, _, err) = a.cli(&["send-message", "--to", &format!("{fake_addr}/%x"), "to a fake"]).await;
    assert!(code != 0 && err.contains("not signed by the key paired with it") && err.contains("it said 200"), "{err}");
    assert_eq!(a.cli(&["link-remove", &fake_addr]).await.0, 0);

    // A new web key there changes nothing: the pairing is by key pair.
    let port_b = addr_b.rsplit(':').next().unwrap().to_string();
    assert_eq!(b.cli(&["web-stop"]).await.0, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (code, status, _) = b.cli(&["web-start", "-p", &port_b, "-b", "127.0.0.1"]).await;
        if code == 0 {
            assert_ne!(keepane::web::status_url(&status), Some(ub.as_str()), "a new key");
            break;
        }
        assert!(Instant::now() < deadline, "the port was not free again");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (code, out, err) = a.cli(&["send-message", "--to", &worker, "still here"]).await;
    assert!(code == 0 && out.contains("forwarded"), "{out} {err}");

    // Unpaired from either side, on both.
    let (code, out, err) = a.cli(&["link-remove", &addr_b]).await;
    assert!(code == 0 && out.contains("both machines"), "{out} {err}");
    assert!(b.cli(&["link-list"]).await.1.starts_with("no machines paired"));
    let (code, _, err) = a.cli(&["send-message", "--to", &worker, "gone?"]).await;
    assert!(code != 0 && err.contains("not paired"), "{err}");
    // A stranger's key in the table by hand (`link trust`), both ways.
    let key_a = a.cli(&["link-id"]).await.1.split_whitespace().next().unwrap().to_string();
    let (code, _, err) = a.cli(&["link-trust", &addr_b, &key_b]).await;
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = b.cli(&["link-trust", &addr_a, &key_a]).await;
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = a.cli(&["send-message", "--to", &worker, "trusted"]).await;
    assert!(code == 0 && out.contains("forwarded"), "{out} {err}");

    // Read-only takes nothing from other machines; a machine that moved (a
    // new port here stands for a new address) is known by its key.
    assert_eq!(b.cli(&["web-stop"]).await.0, 0);
    let (_, addr_b2) = web(&b, &["-r"]).await;
    assert_ne!(addr_b2, addr_b);
    let (code, out, err) = b.cli(&["send-message", "--to", &format!("{addr_a}/%lead"), "from a new port"]).await;
    assert!(code == 0 && out.contains("forwarded"), "{out} {err}");
    let (_, list, _) = a.cli(&["link-list"]).await;
    assert!(list.contains(&addr_b2) && !list.contains(&format!("{addr_b} ")), "{list}");
    let (code, _, err) = a.cli(&["send-message", "--to", &format!("{addr_b2}/%worker"), "in?"]).await;
    assert!(code != 0 && err.contains("read-only"), "{err}");

    // Off: an error at once, nothing queued here. (The port closes a moment
    // after kill-server answers; a connection cut while it closes is an
    // error of its own wording.)
    b.cli(&["kill-server"]).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while tokio::net::TcpStream::connect(&addr_b2).await.is_ok() {
        assert!(Instant::now() < deadline, "the port is still open");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (code, _, err) = a.cli(&["send-message", "--to", &format!("{addr_b2}/%worker"), "anyone?"]).await;
    assert!(code != 0 && err.contains("not reachable"), "{err}");
    let (_, trace, _) = a.cli(&["trace-message", &msg_id(&err.trim_start_matches('#').replace("rejected", ""))]).await;
    assert!(trace.contains("rejected"), "nothing queued: {trace}");
    a.cli(&["kill-server"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The prefix handed over as a bare byte (character 0x02, no Ctrl flag, no
/// key code), as hosts that pass input on as bytes do: it is still C-b.
#[tokio::test(flavor = "multi_thread")]
async fn the_prefix_as_a_bare_byte_still_works() {
    let h = Harness::start("bareprefix").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "bp"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let bare = |ch: u16| KeyRecord { down: true, repeat: 1, vk: 0, sc: 0, ch, ctrl: 0 };
    c.send(ClientMsg::Key(bare(0x02))).await;
    c.send(ClientMsg::Key(KeyRecord { down: false, ..bare(0x02) })).await;
    c.send(ClientMsg::Key(bare(b'c' as u16))).await;
    c.send(ClientMsg::Key(KeyRecord { down: false, ..bare(b'c' as u16) })).await;
    c.wait_for("a second window", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap().contains(&format!("1:{SH}*")))
        .await;
    h.cli(&["kill-server"]).await;
}

/// `bind -T copy-mode-vi`: a key bound there runs its command in copy mode
/// (a `send -X` motion or any other command) and nowhere else; unbinding
/// gives the key back to copy mode; tmux's mouse-key lines are taken.
#[tokio::test(flavor = "multi_thread")]
async fn the_copy_mode_vi_table_binds_keys_in_copy_mode() {
    let h = Harness::start("copytable").await;
    for line in [
        "bind -T copy-mode-vi i send -X cancel",
        "bind -T copy-mode-vi C-t display-message from-the-copy-table",
        "bind-key -T copy-mode-vi MouseDragEnd1Pane send -X copy-selection-and-cancel",
        "unbind -T copy-mode-vi MouseDown1Pane",
    ] {
        let argv: Vec<String> = keepane::command::tokenize(line).unwrap();
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (code, _, err) = h.cli(&args).await;
        assert_eq!(code, 0, "{line}: {err}");
    }
    let (_, keys, _) = h.cli(&["list-keys"]).await;
    assert!(
        keys.lines().any(|l| l.starts_with("bind-key -T copy-mode-vi i") && l.ends_with("send-keys -X cancel")),
        "{keys}"
    );
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "ct"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let in_copy = |s: &vt100::Screen| s.rows(0, COLS).next().unwrap().contains("[0/");
    // Outside copy mode, i is just typed.
    c.type_str("i").await;
    c.wait_for("i typed into the shell", |s| s.contents().contains("keepane>i")).await;
    if cfg!(windows) {
        c.key(VK_ESCAPE, '\x1b', 0).await; // cmd clears its line
    } else {
        c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await; // the terminal's line kill
    }
    c.wait_for("the line cleared", |s| !s.contents().contains("keepane>i")).await;
    // A second prompt line, so the copy-mode cursor has a row above it
    // whatever row the shell's first prompt was on.
    c.enter().await;
    c.wait_for("a second prompt", |s| s.contents().matches("keepane>").count() >= 2).await;
    // In copy mode, i is bound: it leaves copy mode.
    c.prefix('[').await;
    c.wait_for("copy mode", in_copy).await;
    c.type_str("i").await;
    c.wait_for("i left copy mode", |s| !in_copy(s)).await;
    assert!(!c.text().contains("keepane>i"), "i did not reach the shell: {}", c.text());
    // Any command can be bound, not only send -X.
    c.prefix('[').await;
    c.wait_for("copy mode", in_copy).await;
    c.key(b'T' as u16, '\x14', LEFT_CTRL_PRESSED).await;
    c.wait_for("the bound message", |s| s.contents().contains("from-the-copy-table")).await;
    // A key bound to the motion it already has does not loop: send -X
    // goes to copy mode's own motions, never back through the table.
    h.cli(&["bind", "-T", "copy-mode-vi", "k", "send", "-X", "cursor-up"]).await;
    let cursor_row = |s: &vt100::Screen| -> Option<u16> {
        (0..ROWS - 1).find(|y| (0..COLS).any(|x| s.cell(*y, x).is_some_and(|c| c.inverse())))
    };
    let before = cursor_row(c.screen.screen()).expect("a copy-mode cursor");
    c.type_str("k").await;
    c.wait_for("k moved up once", |s| cursor_row(s).is_some_and(|r| r + 1 == before)).await;
    assert_eq!(h.cli(&["display-message", "-p", "ok"]).await.1.trim(), "ok", "the server answers");
    // Unbound again, i is copy mode's own (not a way out).
    h.cli(&["unbind", "-T", "copy-mode-vi", "i"]).await;
    c.type_str("i").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.wait_for("still in copy mode", in_copy).await;
    c.type_str("q").await;
    c.wait_for("q leaves", |s| !in_copy(s)).await;
    h.cli(&["kill-server"]).await;
}

/// The status line's machine variables are read in-process: CPU, memory,
/// uptime answer at once, the git branch follows the pane's directory, the
/// path is shortened with `~`, and `pane_pid_command` names what the pane
/// is running right now. The default status-right shows them.
#[tokio::test(flavor = "multi_thread")]
async fn the_machine_variables_answer_without_a_command() {
    let h = Harness::start("sysvars").await;
    let repo = env!("CARGO_MANIFEST_DIR");
    h.cli(&["new", "-d", "-s", "sv", "-c", repo]).await;
    h.wait_capture("sv:0", "prompt", |t| t.contains("keepane>")).await;
    let hr = &h;
    let ask =
        |f: &'static str| async move { hr.cli(&["display-message", "-p", "-t", "sv:0", f]).await.1.trim().to_string() };
    // Two readings a second apart: the second has a CPU figure.
    ask("#{cpu_percentage}").await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let cpu = ask("#{cpu_percentage}").await;
    assert!(cpu.ends_with('%') && cpu.trim_end_matches('%').parse::<u32>().is_ok_and(|n| n <= 100), "{cpu}");
    let ram = ask("#{ram_percentage}|#{ram_used}|#{uptime}").await;
    let parts: Vec<&str> = ram.split('|').collect();
    assert!(parts[0].ends_with('%') && (parts[1].ends_with('G') || parts[1].ends_with('M')), "{ram}");
    assert!(!parts[2].is_empty() && parts[2] != "0s", "uptime: {ram}");
    // The pane sits in this repository: a branch, and a path under ~ when
    // the repository is (it is, on the developer's machine; elsewhere the
    // full path).
    let git = ask("#{git_branch}").await;
    assert!(!git.is_empty(), "a branch or a commit: {git:?}");
    let short = ask("#{pane_current_path_short}").await;
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    if !home.is_empty() && repo.to_lowercase().starts_with(&home.to_lowercase()) {
        assert!(short.starts_with(&format!("~{}", std::path::MAIN_SEPARATOR)), "{short}");
    } else {
        assert_eq!(short, repo);
    }
    // Idle, the pane runs its shell; while a program runs, that is the program.
    // (macOS's /bin/sh starts bash in its place: the process is bash.)
    let idle = ask("#{pane_pid_command}").await;
    assert!(idle == SH || (cfg!(target_os = "macos") && idle == "bash"), "{idle}");
    let (busy, name) = if cfg!(windows) { ("ping -n 4 127.0.0.1", "ping") } else { ("sleep 3", "sleep") };
    h.cli(&["send-keys", "-t", "sv:0", busy, "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let p = ask("#{pane_pid_command}").await;
        if p.eq_ignore_ascii_case(name) {
            break;
        }
        assert!(Instant::now() < deadline, "{name} never showed as the program: {p:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Battery: a figure with a percent sign, or nothing on a desktop; the
    // charging flag is 0 or 1 either way.
    let bat = ask("[#{battery_percentage}] #{battery_charging}").await;
    assert!(bat == "[] 0" || (bat.ends_with("% 0") || bat.ends_with("% 1")), "{bat}");
    // The default status-right, drawn: CPU, MEM and the clock are there.
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "sv"]).await;
    c.wait_for("status with the machine on it", |s| {
        let row = s.rows(0, COLS).nth(ROWS as usize - 1).unwrap_or_default();
        row.contains("CPU ") && row.contains("% MEM ") && row.contains(" | ")
    })
    .await;
    // Turned off, the line is gone; a custom one shows what it is told.
    h.cli(&["set", "-g", "status", "off"]).await;
    c.wait_for("no status line", |s| !s.rows(0, COLS).nth(ROWS as usize - 1).unwrap_or_default().contains("CPU "))
        .await;
    h.cli(&["set", "-g", "status", "on"]).await;
    h.cli(&["set", "-g", "status-right", "up #{uptime} on #{host_short}"]).await;
    c.wait_for("custom right side", |s| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap_or_default().contains("up "))
        .await;
    h.cli(&["kill-server"]).await;
}

/// Scrolling an inactive pane back with the wheel makes it the active pane,
/// so the copy-mode keys (j, k, C-f, PageUp) work in it; and with C-b as
/// the prefix, `C-b C-b` in copy mode is copy mode's page-up.
#[tokio::test(flavor = "multi_thread")]
async fn the_wheel_selects_the_pane_it_scrolls_and_the_prefix_twice_pages_up() {
    let h = Harness::start("wheelsel").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "ws"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // A second pane on the right, which becomes active; fill the LEFT one.
    h.cli(&["split-window", "-h", "-t", "ws:0"]).await;
    h.wait_capture("ws:0.1", "right prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "ws:0.0", &count_to(60, "left-"), "Enter"]).await;
    h.wait_capture("ws:0.0", "left filled", |t| t.contains("left-60")).await;
    assert_eq!(h.cli(&["display-message", "-p", "-t", "ws:0", "#{pane_index}"]).await.1.trim(), "1", "right is active");
    // Wheel up over the left pane (x=2): it scrolls back and is the active
    // pane now, with the copy-mode indicator in its top-right corner.
    c.send(ClientMsg::Mouse(MouseRecord { x: 2, y: 5, buttons: (120u32) << 16, ctrl: 0, flags: 4 })).await;
    c.wait_for("left in copy mode", |s| s.rows(0, COLS / 2).next().unwrap().contains("[3/")).await;
    assert_eq!(h.cli(&["display-message", "-p", "-t", "ws:0", "#{pane_index}"]).await.1.trim(), "0", "left is active");
    // Keys reach that pane's copy mode: C-f pages down (to the bottom), k
    // scrolls... no: k moves the cursor; C-b C-b pages up again.
    c.key(b'F' as u16, '\x06', LEFT_CTRL_PRESSED).await;
    c.wait_for("C-f paged down", |s| s.rows(0, COLS / 2).next().unwrap().contains("[0/")).await;
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await;
    c.wait_for("C-b C-b paged up", |s| {
        let top = s.rows(0, COLS / 2).next().unwrap();
        top.contains("[") && !top.contains("[0/")
    })
    .await;
    // j and k move the cursor in that pane (the inverted cell moves).
    let cursor_row = |s: &vt100::Screen| -> Option<u16> {
        (0..ROWS - 1).find(|y| (0..COLS / 2).any(|x| s.cell(*y, x).is_some_and(|c| c.inverse())))
    };
    let before = cursor_row(c.screen.screen()).expect("a cursor in copy mode");
    c.type_str("k").await;
    c.wait_for("k moved up", |s| cursor_row(s).is_some_and(|r| r + 1 == before)).await;
    c.type_str("j").await;
    c.wait_for("j moved down", |s| cursor_row(s) == Some(before)).await;
    // g goes to the oldest line: the indicator reads how far up of how many
    // lines there are, [N/N] (it once read [N/2N]).
    let history = h.cli(&["display-message", "-p", "-t", "ws:0.0", "#{history_size}"]).await.1.trim().to_string();
    assert!(history.parse::<usize>().is_ok_and(|n| n > 0), "{history}");
    c.type_str("g").await;
    let want = format!("[{history}/{history}]");
    c.wait_for(&want.clone(), |s| s.rows(0, COLS / 2).next().unwrap().contains(&want)).await;
    c.key(VK_ESCAPE, '\x1b', 0).await;
    c.wait_for("copy mode left", |s| !s.rows(0, COLS / 2).next().unwrap().contains("[")).await;
    h.cli(&["kill-server"]).await;
}

/// A right click pastes the clipboard into the pane, the way the terminal
/// itself would were the mouse not keepane's; from copy mode too, which it
/// leaves first.
#[tokio::test(flavor = "multi_thread")]
async fn a_right_click_pastes_the_clipboard() {
    // The Windows clipboard is shared with everything else on the machine;
    // a runner without one (a service session) cannot run this.
    if keepane::clipboard::set_text("echo pasted-by-right-click").is_err() {
        eprintln!("no clipboard here; skipping");
        return;
    }
    let h = Harness::start("rclick").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "rc"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    // Other tests copy text in parallel and the clipboard is one for the
    // whole machine, so what lands may be theirs: set, click, look, and
    // try again (after clearing cmd's line) when it was something else.
    // The clipboard may be busy for a moment as well.
    async fn set_clipboard(text: &str) {
        let mut set = keepane::clipboard::set_text(text);
        for _ in 0..20 {
            if set.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            set = keepane::clipboard::set_text(text);
        }
        set.expect("the clipboard stayed busy");
    }
    async fn right_click_pastes(c: &mut Conn, text: &str) {
        for attempt in 0..8 {
            set_clipboard(text).await;
            // Right button down and up (bit 2 of the buttons mask).
            c.send(ClientMsg::Mouse(MouseRecord { x: 10, y: 5, buttons: 2, ctrl: 0, flags: 0 })).await;
            c.send(ClientMsg::Mouse(MouseRecord { x: 10, y: 5, buttons: 0, ctrl: 0, flags: 0 })).await;
            let want = format!("keepane>{text}");
            let seen =
                tokio::time::timeout(Duration::from_secs(2), c.wait_for("the paste", |s| s.contents().contains(&want)));
            if seen.await.is_ok() {
                return;
            }
            eprintln!("attempt {attempt}: the clipboard held something else; clearing the line");
            c.key(VK_ESCAPE, '\x1b', 0).await; // cmd clears its input line on Escape
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("a right click never pasted {text:?}:\n{}", c.text());
    }
    right_click_pastes(&mut c, "echo pasted-by-right-click").await;
    c.enter().await;
    c.wait_for("and it ran", |s| s.contents().matches("pasted-by-right-click").count() >= 2).await;
    // From copy mode (the wheel scrolled back): copy mode ends, the paste lands.
    h.cli(&["send-keys", "-t", "rc:0", &count_to(40, "fill-"), "Enter"]).await;
    c.wait_for("filled", |s| s.contents().contains("fill-40")).await;
    c.send(ClientMsg::Mouse(MouseRecord { x: 2, y: 2, buttons: (120u32) << 16, ctrl: 0, flags: 4 })).await;
    c.wait_for("copy mode", |s| s.rows(0, COLS).next().unwrap().contains("[3/")).await;
    right_click_pastes(&mut c, "echo second-paste").await;
    assert!(!c.row(0).contains("[3/"), "copy mode ended: {:?}", c.row(0));
    c.key(VK_ESCAPE, '\x1b', 0).await; // cmd clears its input line on Escape
    // On the status line a right click is not a paste.
    set_clipboard("echo not-this-one").await;
    c.send(ClientMsg::Mouse(MouseRecord { x: 10, y: ROWS as i16 - 1, buttons: 2, ctrl: 0, flags: 0 })).await;
    c.send(ClientMsg::Mouse(MouseRecord { x: 10, y: ROWS as i16 - 1, buttons: 0, ctrl: 0, flags: 0 })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.type_str("echo after").await;
    c.wait_for("typing still works", |s| s.contents().contains("echo after")).await;
    assert!(!c.text().contains("not-this-one"), "{}", c.text());
    h.cli(&["kill-server"]).await;
}

/// Tab at the `:` prompt completes a command name (one candidate typed in
/// whole, several typed as far as they agree and shown in the label) and
/// a target after -t; `keepane completion powershell` prints the shell's
/// completer with every command in it.
#[tokio::test(flavor = "multi_thread")]
async fn tab_completes_at_the_prompt_and_the_shell_gets_a_completer() {
    let h = Harness::start("complete").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "alpha"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["new", "-d", "-s", "beta", "-n", "build"]).await;
    let status = |s: &vt100::Screen| s.rows(0, COLS).nth(ROWS as usize - 1).unwrap_or_default();
    // One candidate: typed in whole, with a space after it.
    c.prefix(':').await;
    c.type_str("spl").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("spl completed", |s| status(s).starts_with(":split-window ")).await;
    // Several: the common part is typed, the candidates take the label.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await; // C-u: clear
    c.type_str("list-s").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("candidates shown", |s| status(s).starts_with("(list-saved list-sessions) list-s")).await;
    c.type_str("e").await;
    c.wait_for("label back after a key", |s| status(s).starts_with(":list-se")).await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("then one", |s| status(s).starts_with(":list-sessions ")).await;
    // None: says so, types nothing.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("zzz").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("no completion", |s| status(s).starts_with("(no completion) zzz")).await;
    // A target after -t: sessions and session:window.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("select-window -t b").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("beta targets", |s| status(s).starts_with("(beta beta:0 beta:build) select-window -t beta")).await;
    c.type_str(":b").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("beta:build", |s| status(s).starts_with(":select-window -t beta:build")).await;
    // After `set`: the option's name, abbreviations included, past flags.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("set sync").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("sync completed", |s| status(s).starts_with(":set synchronize-panes ")).await;
    // Then its value, when it is one of a few.
    c.type_str("o").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("on/off offered", |s| status(s).starts_with("(off on) set synchronize-panes o")).await;
    c.type_str("f").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("off typed", |s| status(s).starts_with(":set synchronize-panes off")).await;
    // -g before the name, and a name spelled by its words.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("set -g mon-act").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("mon-act by words", |s| status(s).starts_with(":set -g monitor-activity ")).await;
    // `show -s` is a flag, not a target: the word after it is a name.
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
    c.type_str("show -s status-l").await;
    c.key(0x09, '\t', 0).await;
    c.wait_for("status-left offered", |s| {
        status(s).starts_with("(status-left status-left-length) show -s status-left")
    })
    .await;
    // Edges: nothing typed yet lists the first options; an unknown or
    // ambiguous name, a user @option, and a third word have nothing to offer.
    for (typed, want) in [
        ("set ", "(agent-commands agent-cost agent-pane-limit animation animation-time autosave +"),
        ("set zzz o", "(no completion) set zzz o"),
        ("set mo o", "(no completion) set mo o"),
        ("set @my", "(no completion) set @my"),
        ("set mouse on o", "(no completion) set mouse on o"),
        // Flags: an alias reads as its command, a flag given is not offered
        // again, one match is typed with a space, no flags says so.
        ("splitw -", "(-h -v -b -d -f -c +2) splitw -"),
        ("set -g -", "(-a -w -s -t) set -g -"),
        ("set -ga -", "(-w -s -t) set -ga -"),
        ("neww -n", ":neww -n "),
        ("ls -", "(no completion) ls -"),
    ] {
        c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await;
        c.type_str(typed).await;
        c.key(0x09, '\t', 0).await;
        c.wait_for(typed, |s| status(s).starts_with(want)).await;
    }
    c.key(VK_ESCAPE, '\x1b', 0).await;
    // The shell completer is a script listing every command and the flags.
    let out =
        std::process::Command::new(env!("CARGO_BIN_EXE_keepane")).args(["completion", "powershell"]).output().unwrap();
    let script = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(script.contains("Register-ArgumentCompleter -Native -CommandName keepane"), "{script}");
    assert!(script.contains("'split-window'") && script.contains("'completion'"), "{script}");
    // bash, zsh and fish get a few lines that ask `keepane __complete`,
    // which answers from the same tables and the running server.
    let run = |args: &[&str]| {
        let o = std::process::Command::new(env!("CARGO_BIN_EXE_keepane")).args(args).output().unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    for shell in ["bash", "zsh", "fish"] {
        let (ok, script, err) = run(&["completion", shell]);
        assert!(ok && script.contains("keepane __complete"), "{shell}: {err}");
    }
    let (ok, _, err) = run(&["completion", "tcsh"]);
    assert!(!ok && err.contains("no script for 'tcsh'"), "{err}");
    let (_, offered, _) = run(&["__complete", "splitw", "-"]);
    assert!(offered.lines().any(|l| l == "-h"), "{offered}");
    let session = h.cli(&["display-message", "-p", "#{session_name}"]).await.1;
    let (_, offered, _) = run(&["__complete", "-L", &h.socket, "attach", "-t", ""]);
    assert!(offered.lines().any(|l| l == session.trim()), "the running server's sessions: {offered:?}");
    h.cli(&["kill-server"]).await;
}

/// A client smaller than the session's window (`window-size largest` gave
/// the session the big client's size) sees a part of it: its own view,
/// panned with `refresh-client -L/-R/-U/-D`, following the cursor when a
/// key goes to the pane. The big client is not affected.
#[tokio::test(flavor = "multi_thread")]
async fn a_small_client_has_its_own_view_of_a_big_window() {
    let h = Harness::start("viewport").await;
    h.cli(&["set", "-g", "window-size", "largest"]).await;
    let mut big = h.connect().await;
    big.attach(&["new", "-s", "vp"]).await;
    big.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    let mut small = h.connect().await;
    small.attach(&["attach", "-t", "vp"]).await;
    small.screen = vt100::Parser::new(12, 40, 0);
    small.send(ClientMsg::Resize { cols: 40, rows: 12 }).await;
    small.wait_for("small prompt", |s| s.contents().contains("keepane>")).await;
    // The session stays 80x24 (largest): the small client shows 40 columns.
    assert_eq!(
        h.cli(&["display-message", "-p", "-t", "vp:0", "#{window_width}x#{window_height}"]).await.1.trim(),
        "80x24"
    );
    let long = "0123456789abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ";
    big.type_str(&format!("echo {long}")).await;
    big.enter().await;
    // Twice (the command and its output) and the prompt after them: until
    // then the cursor is wherever the shell is printing, and following it
    // below would go there.
    big.wait_for("the long line", |s| {
        let t = s.contents();
        t.matches(long).count() >= 2 && t.lines().any(|l| l.trim_end() == "keepane>")
    })
    .await;
    small
        .wait_for("the first 40 columns", |s| {
            let t = s.contents();
            t.contains("0123456789abcdefghijklmnopqrstuvwxyz0123") && !t.contains("ABCDE")
        })
        .await;
    // Pan right by 20: columns 20..60 of the window.
    small.prefix(':').await;
    small.type_str("refresh-client -R 20").await;
    small.enter().await;
    small
        .wait_for("panned", |s| {
            let t = s.contents();
            t.contains("klmnopqrstuvwxyz0123456789ABCDEFGHIJ") && !t.lines().any(|l| l.starts_with("0123456789abc"))
        })
        .await;
    // The status line is still the client's own width, whole.
    let status = small.screen.screen().rows(0, 40).nth(11).unwrap_or_default();
    assert!(status.starts_with("[vp] 0:"), "status at 40 columns: {status:?}");
    // The big client saw none of that.
    assert!(big.text().contains(long), "{}", big.text());
    // A key to the pane brings the cursor back into view: the view moves
    // left to the cursor (after the prompt), no further.
    small.type_str("x").await;
    small
        .wait_for("follows the cursor", |s| {
            let t = s.contents();
            // The cursor sat at column 8 (after "keepane>") or, when cmd had
            // echoed the x before the render, at 9: the view moved to it.
            t.lines()
                .any(|l| l.starts_with("89abcdefghijklmnopqrstuvwxyz") || l.starts_with("9abcdefghijklmnopqrstuvwxyz"))
        })
        .await;
    // S-Right is bound to a pan of 10; the bindings survive list-keys.
    let (_, keys, _) = h.cli(&["list-keys"]).await;
    assert!(
        keys.lines().any(|l| l.contains("-r ") && l.contains(" S-Right ") && l.ends_with("refresh-client -R 10")),
        "{keys}"
    );
    // Panning past the edge is clamped: the far right end stays put.
    small.prefix(':').await;
    small.type_str("refresh-client -R 500").await;
    small.enter().await;
    // (80 - 40 = 40: the row shows the line from its 41st character.)
    small
        .wait_for("clamped at the right edge", |s| s.contents().lines().any(|l| l.starts_with("456789ABCDEFGHIJ")))
        .await;
    h.cli(&["kill-server"]).await;
}

/// `swap-window` across sessions, `pipe-pane -I`, the prefix key reaching a
/// popup's program, and folding a session in the tree.
#[tokio::test(flavor = "multi_thread")]
async fn the_last_small_tmux_gaps_are_closed() {
    let h = Harness::start("gaps2").await;
    h.cli(&["new", "-d", "-s", "a", "-n", "a0"]).await;
    h.cli(&["new-window", "-d", "-t", "a", "-n", "a1"]).await;
    h.cli(&["new", "-d", "-s", "b", "-n", "b0"]).await;
    h.cli(&["new-window", "-d", "-t", "b", "-n", "b1"]).await;
    // swap-window across sessions: a:1 and b:0 change places, both sessions
    // keep their current window, no window is lost.
    let (code, _, err) = h.cli(&["swap-window", "-s", "a:1", "-t", "b:0"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, wa, _) = h.cli(&["list-windows", "-t", "a"]).await;
    let (_, wb, _) = h.cli(&["list-windows", "-t", "b"]).await;
    // a was looking at a0 (which stayed); b was looking at b0, whose place
    // a1 took, so b looks at a1 now (the index keeps the mark, as in tmux).
    assert!(wa.lines().any(|l| l.starts_with("0: a0*")) && wa.lines().any(|l| l.starts_with("1: b0 ")), "{wa}");
    assert!(wb.lines().any(|l| l.starts_with("0: a1*")) && wb.lines().any(|l| l.starts_with("1: b1 ")), "{wb}");
    assert_eq!(h.cli(&["list-panes", "-a"]).await.1.lines().count(), 4);
    // pipe-pane -I: what the command prints is typed into the pane.
    h.wait_capture("a:0", "prompt", |t| t.contains("keepane>")).await;
    let (code, _, err) = h.cli(&["pipe-pane", "-I", "-t", "a:0", &says("echo typed-by-the-pipe")]).await;
    assert_eq!(code, 0, "{err}");
    h.wait_capture("a:0", "the piped input ran", |t| t.matches("typed-by-the-pipe").count() >= 2).await;
    // ...and, its output over, the input-only pipe is gone: a plain
    // pipe-pane with a command starts a new one rather than reporting
    // the old one (which `-o` would toggle off).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (code, _, _) = h.cli(&["pipe-pane", "-o", "-I", "-t", "a:0", &says("echo second-pipe")]).await;
        assert_eq!(code, 0);
        let (_, out, _) = h.cli(&["capture-pane", "-p", "-t", "a:0"]).await;
        if out.matches("second-pipe").count() >= 2 {
            break; // -o started (not stopped) one: the old pipe had ended
        }
        assert!(Instant::now() < deadline, "the input-only pipe never ended: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.cli(&["pipe-pane", "-t", "a:0"]).await;
    // The prefix reaches a popup's program when pressed twice: a program
    // reading one key sees the prefix (C-b is 2).
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "b"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix(':').await;
    c.type_str(&format!("display-popup -E {}", shell_line("pip"))).await;
    c.enter().await;
    c.wait_for("popup", |s| s.contents().contains("pip>")).await;
    // cmd prints ^B for a C-b typed at its prompt only on some builds, so
    // ask a program inside the popup to read one key and say its code.
    c.type_str(if cfg!(windows) {
        "powershell -NoProfile -Command \"$k=[Console]::ReadKey($true); 'code=' + [int]$k.KeyChar\""
    } else {
        "stty raw -echo; k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' '); stty sane; echo code=$k"
    })
    .await;
    c.enter().await;
    tokio::time::sleep(Duration::from_millis(1500)).await; // the reader's start-up
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await; // the prefix...
    c.key(b'B' as u16, '\x02', LEFT_CTRL_PRESSED).await; // ...and the prefix again
    c.wait_for("the popup saw C-b", |s| s.contents().contains("code=2")).await;
    let (_, behind, _) = h.cli(&["capture-pane", "-p", "-t", "b:1"]).await;
    assert!(!behind.contains("code="), "the pane behind saw nothing: {behind}");
    c.type_str("exit").await;
    c.enter().await;
    c.wait_for("popup closed", |s| !s.contents().contains("pip>")).await;
    // Folding a session in the tree: `-` hides its windows, `+` shows them.
    // (The plain list: 2 sessions, 4 windows, 4 panes.)
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    c.prefix('w').await;
    c.wait_for("tree", |s| s.contents().contains("/10] j/k move")).await;
    c.type_str("g").await; // session a's line
    c.key(0xBD, '-', 0).await;
    c.wait_for("a folded", |s| {
        let t = s.contents();
        t.contains("/6] j/k move") && t.contains("(0) + a: 2 windows") && t.contains("(1) - b: 2 windows")
    })
    .await;
    assert!(!c.text().contains("- 0: a0"), "{}", c.text());
    c.key(0xBB, '+', SHIFT_PRESSED).await;
    c.wait_for("a open again", |s| {
        s.contents().contains("/10] j/k move") && s.contents().contains("(0) - a: 2 windows")
    })
    .await;
    // a:0: Left folds the window (its pane goes), Left again its session.
    c.type_str("j").await;
    c.key(0x25, '\0', 0).await;
    c.wait_for("window folded", |s| s.contents().contains("[2/9] j/k move") && s.contents().contains("  + 0: a0"))
        .await;
    c.key(0x25, '\0', 0).await;
    c.wait_for("then its session", |s| s.contents().contains("[1/6] j/k move")).await;
    c.type_str("q").await;
    h.cli(&["kill-server"]).await;
}

/// `list-panes -s/-a`, `swap-pane -s A -t B` (in one window and across
/// windows), `set -t` for synchronize-panes, and where `display-popup
/// -x/-y` puts the box.
#[tokio::test(flavor = "multi_thread")]
async fn the_smaller_tmux_gaps_are_closed() {
    let h = Harness::start("gaps").await;
    h.cli(&["new", "-d", "-s", "g"]).await;
    h.cli(&["split-window", "-d", "-t", "g:0"]).await;
    h.cli(&["new-window", "-d", "-t", "g", "-n", "two"]).await;
    h.cli(&["split-window", "-d", "-t", "g:1"]).await;
    h.cli(&["new", "-d", "-s", "other"]).await;
    let ids = |out: &str| -> Vec<String> {
        out.lines().filter_map(|l| l.split_whitespace().find(|w| w.starts_with('%')).map(str::to_string)).collect()
    };
    // list-panes: one window, the session (window-prefixed), the server
    // (session:window-prefixed).
    let (_, one, _) = h.cli(&["list-panes", "-t", "g:0"]).await;
    assert_eq!(one.lines().count(), 2, "{one}");
    assert!(one.starts_with("0: ["), "{one}");
    let (_, sess, _) = h.cli(&["list-panes", "-s", "-t", "g"]).await;
    assert_eq!(sess.lines().count(), 4, "{sess}");
    assert!(sess.lines().filter(|l| l.starts_with("0.")).count() == 2, "{sess}");
    assert!(sess.lines().filter(|l| l.starts_with("1.")).count() == 2, "{sess}");
    let (_, all, _) = h.cli(&["list-panes", "-a"]).await;
    assert_eq!(all.lines().count(), 5, "{all}");
    assert!(
        all.lines().any(|l| l.starts_with("g:1.1: [")) && all.lines().any(|l| l.starts_with("other:0.0: [")),
        "{all}"
    );
    // swap-pane pair form, in one window: the ids change places.
    let before = ids(&one);
    h.cli(&["swap-pane", "-s", "g:0.0", "-t", "g:0.1"]).await;
    let (_, after, _) = h.cli(&["list-panes", "-t", "g:0"]).await;
    assert_eq!(ids(&after), vec![before[1].clone(), before[0].clone()], "{after}");
    // ...and across windows: g:0.0 goes to window 1, its pane comes here.
    let w1_before = ids(&h.cli(&["list-panes", "-t", "g:1"]).await.1);
    let (code, _, err) = h.cli(&["swap-pane", "-s", "g:0.0", "-t", "g:1.1"]).await;
    assert_eq!(code, 0, "{err}");
    let w0 = ids(&h.cli(&["list-panes", "-t", "g:0"]).await.1);
    let w1 = ids(&h.cli(&["list-panes", "-t", "g:1"]).await.1);
    assert_eq!(w0, vec![w1_before[1].clone(), before[0].clone()], "window 0 after the cross swap: {w0:?}");
    assert_eq!(w1, vec![w1_before[0].clone(), before[1].clone()], "window 1 after the cross swap: {w1:?}");
    assert_eq!(h.cli(&["list-panes", "-a"]).await.1.lines().count(), 5, "no pane was lost");
    // Both moved panes still answer.
    h.cli(&["send-keys", "-t", "g:0.1", "echo moved-here", "Enter"]).await;
    h.wait_capture("g:0.1", "the moved pane's echo", |t| t.contains("moved-here")).await;
    // synchronize-panes with -t: that window only.
    let (code, _, err) = h.cli(&["set", "-w", "-t", "g:1", "synchronize-panes", "on"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(h.cli(&["display-message", "-p", "-t", "g:1", "#{pane_synchronized}"]).await.1.trim(), "1");
    assert_eq!(h.cli(&["display-message", "-p", "-t", "g:0", "#{pane_synchronized}"]).await.1.trim(), "0");
    // display-popup -x/-y: the box's corner is where it was asked to be.
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "other"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    c.prefix(':').await;
    c.type_str(&format!("display-popup -x 0 -y 0 -w 20 -h 5 -E {}", shell_line("pip"))).await;
    c.enter().await;
    c.wait_for("popup", |s| s.contents().contains("pip>")).await;
    assert!(c.row(0).starts_with('┌'), "top-left corner at 0,0: {:?}", c.row(0));
    assert_eq!(c.row(0).chars().filter(|ch| *ch == '─').count(), 18, "20 wide: {:?}", c.row(0));
    c.type_str("exit").await;
    c.enter().await;
    c.wait_for("popup closed", |s| !s.contents().contains("pip>")).await;
    c.prefix(':').await;
    c.type_str(&format!("display-popup -x R -y B -w 20 -h 5 -E {}", shell_line("pip"))).await;
    c.enter().await;
    c.wait_for("popup", |s| s.contents().contains("pip>")).await;
    // Bottom-right of the window area: the row above the status line ends
    // with the box's bottom-right corner.
    let last = c.row(ROWS - 2);
    assert!(last.trim_end().ends_with('┘'), "bottom-right corner against the edge: {last:?}");
    assert!(c.row(ROWS - 6).trim_end().ends_with('┐'), "5 tall: {:?}", c.row(ROWS - 6));
    h.cli(&["kill-server"]).await;
}

/// Windows asks every top-level window WM_QUERYENDSESSION before a shutdown
/// or logoff; the server keeps a hidden one for that and saves everything
/// when asked. The server runs in this process, so the test can ask the
/// same way Windows would.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_saves_every_session_first() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_QUERYENDSESSION};
    let h = Harness::start("endsession").await;
    h.cli(&["new", "-d", "-s", "bye"]).await;
    h.wait_capture("bye:0", "prompt", |t| t.contains("keepane>")).await;
    let file = || {
        std::fs::read_dir(&h.sessions_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .find(|t| t.contains("\"name\": \"bye\""))
            .unwrap_or_default()
    };
    // Let the first autosave happen (the tick after the session was made,
    // history included): for the next 30 seconds the tick writes nothing
    // more unless the tree changes, so what the pane prints now can only
    // reach the file through the shutdown path.
    let deadline = Instant::now() + Duration::from_secs(5);
    while file().is_empty() {
        assert!(Instant::now() < deadline, "the session was never autosaved");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.cli(&["send-keys", "-t", "bye:0", "echo typed-just-before-shutdown", "Enter"]).await;
    h.wait_capture("bye:0", "the echo", |t| t.matches("typed-just-before-shutdown").count() >= 2).await;
    assert!(!file().contains("typed-just-before-shutdown"), "not saved yet: {}", file());
    let deadline = Instant::now() + Duration::from_secs(5);
    let hwnd = loop {
        if let Some(w) = keepane::shutdown::window(&h.socket) {
            break w as usize; // a handle is a number; usize crosses threads
        }
        assert!(Instant::now() < deadline, "the server's shutdown window never came up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // SendMessage runs the window's handler on its thread and returns when
    // it has answered, which it does once the server has saved.
    let allowed = tokio::task::spawn_blocking(move || unsafe { SendMessageW(hwnd as _, WM_QUERYENDSESSION, 0, 0) })
        .await
        .unwrap();
    assert_eq!(allowed, 1, "the shutdown may go on");
    let saved = file();
    assert!(saved.contains("typed-just-before-shutdown"), "saved on the way out: {saved}");
    // A session whose shell ends now (the shutdown killing it) keeps its
    // save: `resume` after the reboot is to bring it back.
    h.cli(&["new", "-d", "-s", "dies"]).await;
    h.cli(&["save-session", "-t", "dies"]).await;
    let allowed = tokio::task::spawn_blocking(move || unsafe { SendMessageW(hwnd as _, WM_QUERYENDSESSION, 0, 0) })
        .await
        .unwrap();
    assert_eq!(allowed, 1);
    h.cli(&["send-keys", "-t", "dies", "exit", "Enter"]).await;
    h.wait_for_cli("dies ended", &["has-session", "-t", "dies"], |code, _| code == 1).await;
    tokio::time::sleep(Duration::from_secs(13)).await;
    let (_, out, _) = h.cli(&["list-saved"]).await;
    assert!(out.lines().any(|l| l.starts_with("dies:")), "kept through the shutdown: {out}");
    // `autosave off` means off at shutdown too: nothing is written.
    h.cli(&["set", "-g", "autosave", "off"]).await;
    h.cli(&["send-keys", "-t", "bye:0", "echo after-autosave-off", "Enter"]).await;
    h.wait_capture("bye:0", "the second echo", |t| t.matches("after-autosave-off").count() >= 2).await;
    let allowed = tokio::task::spawn_blocking(move || unsafe { SendMessageW(hwnd as _, WM_QUERYENDSESSION, 0, 0) })
        .await
        .unwrap();
    assert_eq!(allowed, 1);
    assert!(!file().contains("after-autosave-off"), "autosave off is respected: {}", file());
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn window_size_picks_which_client_sizes_the_session() {
    let h = Harness::start("winsize").await;
    async fn size(h: &Harness) -> String {
        h.cli(&["display-message", "-p", "-t", "sz:0", "#{window_width}x#{window_height}"]).await.1.trim().to_string()
    }
    let mut big = h.connect().await;
    big.attach(&["new", "-s", "sz"]).await;
    big.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    assert_eq!(size(&h).await, "80x24");
    let mut small = h.connect().await;
    small.attach(&["attach", "-t", "sz"]).await;
    small.send(ClientMsg::Resize { cols: 60, rows: 20 }).await;
    // latest (the default): the client that attached or resized last.
    let deadline = Instant::now() + Duration::from_secs(5);
    while size(&h).await != "60x20" {
        assert!(Instant::now() < deadline, "latest: {}", size(&h).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // ...and the one that types: a key from the big client takes it back.
    big.type_str("x").await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while size(&h).await != "80x24" {
        assert!(Instant::now() < deadline, "latest after a key: {}", size(&h).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // smallest: the small client wins whoever types.
    let (code, _, err) = h.cli(&["set", "-g", "window-size", "smallest"]).await;
    assert_eq!(code, 0, "{err}");
    big.send(ClientMsg::Resize { cols: 80, rows: 24 }).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while size(&h).await != "60x20" {
        assert!(Instant::now() < deadline, "smallest: {}", size(&h).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    big.type_str("x").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(size(&h).await, "60x20", "a key does not change smallest");
    // largest: the big one; and when it goes, the small one is all there is.
    h.cli(&["set", "-g", "window-size", "largest"]).await;
    small.send(ClientMsg::Resize { cols: 60, rows: 20 }).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while size(&h).await != "80x24" {
        assert!(Instant::now() < deadline, "largest: {}", size(&h).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    big.send(ClientMsg::Detach).await;
    assert_eq!(big.wait_detached().await, "detached");
    let deadline = Instant::now() + Duration::from_secs(5);
    while size(&h).await != "60x20" {
        assert!(Instant::now() < deadline, "largest after a detach: {}", size(&h).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // manual: nothing a client does moves it; resize-window does.
    h.cli(&["set", "-g", "window-size", "manual"]).await;
    small.send(ClientMsg::Resize { cols: 70, rows: 22 }).await;
    small.type_str("x").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(size(&h).await, "60x20", "manual ignores the client");
    // resize-window is what sets the size then: absolute, relative, and
    // from the attached client (-A / -a), with the same floor as new -x/-y.
    let (code, _, err) = h.cli(&["resize-window", "-t", "sz", "-x", "100", "-y", "30"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(size(&h).await, "100x30");
    h.cli(&["resize-window", "-t", "sz", "-L", "10"]).await;
    h.cli(&["resize-window", "-t", "sz", "-D", "2"]).await;
    assert_eq!(size(&h).await, "90x32");
    h.cli(&["resize-window", "-t", "sz", "-a"]).await;
    assert_eq!(size(&h).await, "70x22", "the (only) attached client's size");
    h.cli(&["resize-window", "-t", "sz", "-x", "1", "-y", "1"]).await;
    assert_eq!(size(&h).await, "10x3", "the floor");
    let (code, _, err) = h.cli(&["resize-window", "-t", "sz", "-x", "wide"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("bad -x"), "{err}");
    let (code, _, err) = h.cli(&["set", "-g", "window-size", "sideways"]).await;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("bad window-size"), "{err}");
    assert_eq!(h.cli(&["show-options", "-gv", "window-size"]).await.1.trim(), "manual");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_tree_filters_and_tags() {
    let h = Harness::start("treetags").await;
    h.cli(&["set", "-g", "choose-tree-style", "list"]).await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "alpha"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["new-window", "-d", "-t", "alpha", "-n", "beta"]).await;
    h.cli(&["new", "-d", "-s", "gamma"]).await;
    h.wait_capture("gamma:0", "shell prompt", |t| t.contains("keepane>")).await;
    c.prefix('w').await;
    // The cursor starts on the current pane, alpha:0's (line 3 of 8: the
    // sessions, their windows and their panes).
    c.wait_for("picker", |s| s.contents().contains("[3/8] j/k move")).await;
    // f types a filter; the list follows it, and a session stays with its
    // matching window, and the window with its panes. Escape puts the old
    // (empty) filter back.
    c.type_str("f").await;
    c.wait_for("filter prompt", |s| s.contents().contains("(filter)")).await;
    c.type_str("beta").await;
    c.wait_for("filtered", |s| s.contents().contains("/3] j/k move") && s.contents().contains("[filter: beta]")).await;
    let text = c.text();
    assert!(text.contains("(0) - alpha: 2 windows (attached)"), "{text}");
    assert!(text.contains("(1)   - 1: beta"), "{text}");
    assert!(text.contains("(2)       - 0: "), "beta's pane: {text}");
    assert!(!text.contains("gamma"), "{text}");
    assert!(text.contains("[filter: beta]"), "{text}");
    c.key(VK_ESCAPE, '\x1b', 0).await;
    c.wait_for("filter cancelled", |s| s.contents().contains("/8] j/k move") && !s.contents().contains("[filter"))
        .await;
    // A filter by session name keeps its windows; Enter keeps the filter.
    c.type_str("f").await;
    c.wait_for("filter prompt", |s| s.contents().contains("(filter)")).await;
    c.type_str("GAMMA").await;
    c.wait_for("filtered", |s| s.contents().contains("/3] j/k move") && s.contents().contains("[filter: GAMMA]")).await;
    c.enter().await;
    c.wait_for("filter kept", |s| {
        let t = s.contents();
        !t.contains("(filter)") && t.contains("[filter: GAMMA]") && t.contains(&format!("(1)   - 0: {SH}*"))
    })
    .await;
    c.type_str("f").await;
    c.wait_for("filter prompt", |s| s.contents().contains("(filter)")).await;
    c.key(b'U' as u16, '\x15', LEFT_CTRL_PRESSED).await; // C-u: clear the input
    c.enter().await;
    c.wait_for("no filter", |s| s.contents().contains("/8] j/k move") && !s.contents().contains("[filter")).await;
    // t tags a line (marked *) and moves down; x kills every tagged line.
    c.type_str("g").await; // alpha
    c.type_str("jjj").await; // alpha:0, its pane, alpha:1 (beta)
    c.wait_for("on beta", |s| s.contents().contains("[4/8] j/k move")).await;
    c.type_str("t").await; // beta (the cursor goes on to its pane)
    c.wait_for("tagged", |s| s.contents().contains("(3)*  - 1: beta") && s.contents().contains("[1 tagged]")).await;
    c.type_str("j").await; // gamma (session)
    c.type_str("t").await;
    c.wait_for("two tagged", |s| s.contents().contains("(5)*- gamma") && s.contents().contains("[2 tagged]")).await;
    c.type_str("t").await; // gamma's window
    c.wait_for("three", |s| s.contents().contains("[3 tagged]")).await;
    c.type_str("k").await; // back on it...
    c.type_str("t").await; // ...and untag it again
    c.wait_for("back to two", |s| {
        s.contents().contains("[2 tagged]") && s.contents().contains(&format!("(6)   - 0: {SH}*"))
    })
    .await;
    c.key(b'X' as u16, 'x', 0).await;
    // A session among them (gamma): asked first, and it says which.
    c.wait_for("asked", |s| s.contents().contains("kill session gamma? (y/n)")).await;
    c.type_str("y").await;
    c.wait_for("killed", |s| {
        let t = s.contents();
        t.contains("/3] j/k move") && !t.contains("beta") && !t.contains("gamma") && !t.contains("tagged]")
    })
    .await;
    let (_, out, _) = h.cli(&["ls"]).await;
    assert!(out.contains("alpha: 1 windows") && !out.contains("gamma"), "{out}");
    // T clears the tags without acting.
    c.type_str("t").await;
    c.wait_for("tagged", |s| s.contents().contains("[1 tagged]")).await;
    c.key(b'T' as u16, 'T', SHIFT_PRESSED).await;
    c.wait_for("cleared", |s| !s.contents().contains("tagged]")).await;
    c.type_str("q").await;
    c.wait_for("closed", |s| !s.contents().contains("j/k move")).await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_jobs_is_the_board_you_can_act_on() {
    let h = Harness::start("choosejobs").await;
    h.cli(&["set", "-g", "remain-on-exit", "on"]).await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "j"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&args(&["new-window", "-d", "-t", "j", "-n", "dies"], &exits(4))).await;
    h.cli(&["new", "-d", "-s", "other"]).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.cli(&["jobs"]).await.1.contains("exit 4") {
        assert!(Instant::now() < deadline, "the exited pane never showed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let pid_before = h.cli(&["jobs", "-t", "j:0.0", "-F", "#{pane_pid}"]).await.1.trim().to_string();

    // prefix J: the board, cursor on this client's own pane (item 1 of
    // header + 3 panes), with its own action keys in the hint.
    c.prefix('B').await;
    c.wait_for("the board", |s| {
        let t = s.contents();
        t.contains("[2/4] j/k move") && t.contains("Enter go  x kill  r restart") && t.contains("exit 4")
    })
    .await;
    let text = c.text();
    assert!(text.contains("PANE") && text.contains("STATE") && text.contains("IDLE"), "{text}");
    assert!(text.contains("(1) j:0.0") && text.contains("(2) j:1.0") && text.contains("(3) other:0.0"), "{text}");

    // r restarts the pane under the cursor: a new pid, the board stays open.
    c.type_str("r").await;
    c.wait_for("restarted", |s| s.contents().contains("[2/4]")).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pid = h.cli(&["jobs", "-t", "j:0.0", "-F", "#{pane_pid}"]).await.1.trim().to_string();
        if !pid.is_empty() && pid != pid_before {
            break;
        }
        assert!(Instant::now() < deadline, "pid did not change after r: {pid} vs {pid_before}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // x kills the exited one: its row goes, the count follows.
    c.type_str("j").await;
    c.wait_for("on the dead pane", |s| s.contents().contains("[3/4]")).await;
    c.type_str("x").await;
    c.wait_for("killed", |s| !s.contents().contains("exit 4") && s.contents().contains("/3]")).await;
    let (_, wins, _) = h.cli(&["list-windows", "-t", "j"]).await;
    assert_eq!(wins.lines().count(), 1, "the window of the killed pane is gone: {wins}");
    // Enter on the other session's pane switches the client there.
    c.type_str("G").await;
    c.wait_for("last item", |s| s.contents().contains("[3/3]")).await;
    c.key(0x0D, '\r', 0).await;
    c.wait_for("switched", |s| {
        let t = s.contents();
        !t.contains("j/k move") && t.contains("[other]")
    })
    .await;
    // Nothing leaked into a shell (once its prompt is there to see).
    let out = h.wait_capture("other:0", "other's prompt", |t| t.contains("keepane>")).await;
    assert_eq!(out.trim(), "keepane>", "picker keys leaked into the pane: {out:?}");
    // From a script there is no client to draw it for.
    let (code, _, err) = h.cli(&["choose-jobs"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("not attached"), "{err}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn choose_jobs_edges() {
    let h = Harness::start("choosejobs2").await;
    let mut c = h.connect().await;
    c.attach(&["new", "-s", "e"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    for _ in 0..11 {
        h.cli(&["new-window", "-d", "-t", "e"]).await;
    }
    c.prefix('B').await;
    c.wait_for("the board", |s| s.contents().contains("[2/13] j/k move")).await;
    let text = c.text();
    // Jump tags run 1..12 (items, the header being 0 and untagged), padded
    // to the widest so the columns stay lined up.
    assert!(text.contains("(9)  e:8.0"), "{text}");
    assert!(text.contains("(12) e:11.0"), "{text}");
    assert!(!text.contains("(0)"), "{text}");
    // g never lands on the header: it is a title, not a pane.
    c.type_str("g").await;
    c.wait_for("first pane", |s| s.contents().contains("[2/13]")).await;
    // A digit jumps to that item; two make one number; the header's 0 is
    // no item, so it leaves the cursor where it is.
    c.type_str("12").await;
    c.wait_for("item 12", |s| s.contents().contains("[13/13]")).await;
    // (On the header the j after it would land on item 1, not 2.)
    c.type_str("g0j").await;
    c.wait_for("0 stayed", |s| s.contents().contains("[3/13]")).await;
    c.type_str("7").await;
    c.wait_for("item 7", |s| s.contents().contains("[8/13]")).await;
    // The pane under the cursor is killed from outside: the board notices
    // (12 rows), and acting on what vanished says so instead of guessing.
    h.cli(&["kill-window", "-t", "e:6"]).await;
    c.wait_for("one fewer", |s| s.contents().contains("/12]")).await;
    c.type_str("q").await;
    c.wait_for("closed", |s| !s.contents().contains("j/k move")).await;
    // Keys never reached the shell.
    let out = h.cli(&["capture-pane", "-p", "-t", "e:0"]).await.1;
    assert_eq!(out.trim(), "keepane>", "{out:?}");
    // The binding is an ordinary one: rebound and unbound like any other.
    h.cli(&["unbind-key", "B"]).await;
    let (_, keys, _) = h.cli(&["list-keys"]).await;
    assert!(!keys.contains("choose-jobs"), "{keys}");
    h.cli(&["bind-key", "Y", "choose-jobs"]).await;
    c.prefix('Y').await;
    c.wait_for("the board again", |s| s.contents().contains("j/k move")).await;
    c.type_str("q").await;
    c.wait_for("closed", |s| !s.contents().contains("j/k move")).await;
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_pane_brings_every_attached_client_to_that_pane() {
    let h = Harness::start("focus").await;
    // Nobody attached yet: nothing to switch, said plainly.
    h.cli(&["new", "-d", "-s", "a"]).await;
    let (_, first, _) = h.cli(&["jobs", "-t", "a:0", "-F", "#{pane_id}"]).await;
    let (code, _, err) = h.cli(&["focus-pane", first.trim()]).await;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no client attached"), "{err}");
    let mut c = h.connect().await;
    c.attach(&["attach", "-t", "a"]).await;
    c.wait_for("prompt", |s| s.contents().contains("keepane>")).await;
    h.cli(&["new", "-d", "-s", "b", "-n", "target"]).await;
    h.cli(&["split-window", "-d", "-t", "b:0"]).await;
    let (_, ids, _) = h.cli(&["jobs", "-t", "b:0", "-F", "#{pane_id}"]).await;
    let bottom = ids.lines().last().unwrap().trim().to_string();
    assert!(bottom.starts_with('%'), "{ids}");

    // The client is looking at a; focus-pane on b's second pane takes it
    // there: session b, window target, that pane active.
    let (code, out, err) = h.cli(&["focus-pane", &bottom]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("1 client(s) switched"), "{out}");
    c.wait_for("switched to b", |s| s.contents().contains("[b]")).await;
    let (_, active, _) = h.cli(&["display-message", "-p", "-t", "b:0", "#{pane_id}"]).await;
    assert_eq!(active.trim(), bottom, "the pane became the active one");
    // Wrong ids are refused; a bare number is the same as %number.
    let (code, _, err) = h.cli(&["focus-pane", "%999"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no pane %999"), "{err}");
    let (code, _, _) = h.cli(&["focus-pane", bottom.trim_start_matches('%')]).await;
    assert_eq!(code, 0);
    h.cli(&["kill-server"]).await;
}

/// bash with keepane's hook reports every cd (OSC 7); sh says nothing, and
/// its process's own directory is read instead. Both land in the saved file.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_pane_knows_where_its_shell_went() {
    let h = Harness::start("cwd").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "cw", "-c", "/", "bash"]).await;
    assert_eq!(code, 0, "{err}");
    let follows = async |target: &str, cd: &str, want: &str| {
        h.cli(&["send-keys", "-t", target, cd, "Enter"]).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, out, _) = h.cli(&["display-message", "-p", "-t", target, "#{pane_current_path}"]).await;
            if out.trim() == want {
                break;
            }
            assert!(Instant::now() < deadline, "{target}: {cd} was not followed: {out:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    follows("cw:0", "cd /usr", "/usr").await;
    h.cli(&["new-window", "-d", "-t", "cw", "-n", "s"]).await;
    h.wait_capture("cw:1", "sh prompt", |t| t.contains("keepane>")).await;
    follows("cw:1", "cd /usr/bin", "/usr/bin").await;
    h.cli(&["save-session", "-t", "cw"]).await;
    let file = std::fs::read_dir(&h.sessions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .find(|t| t.contains("\"name\": \"cw\""))
        .expect("a saved file for cw");
    assert!(file.contains("\"cwd\": \"/usr\"") && file.contains("\"cwd\": \"/usr/bin\""), "{file}");
    h.cli(&["kill-server"]).await;
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_pane_knows_where_its_shell_went() {
    let h = Harness::start("cwd").await;
    let (code, _, err) = h.cli(&["new", "-d", "-s", "cw", "pwsh.exe", "-NoLogo"]).await;
    assert_eq!(code, 0, "{err}");
    h.wait_capture("cw:0", "a pwsh prompt", |t| t.contains("PS ")).await;
    // PowerShell, no profile of the user's: the prompt hook keepane gives it
    // reports every cd through OSC 9;9.
    h.cli(&["send-keys", "-t", "cw:0", "cd C:\\Windows", "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, out, _) = h.cli(&["display-message", "-p", "-t", "cw:0", "#{pane_current_path}"]).await;
        if out.trim().eq_ignore_ascii_case("C:\\Windows") {
            break;
        }
        assert!(Instant::now() < deadline, "pwsh's cd was not followed: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The prompt itself still reads as before (the hook adds only an
    // invisible sequence after it).
    let (_, screen, _) = h.cli(&["capture-pane", "-p", "-t", "cw:0"]).await;
    assert!(screen.contains("PS C:\\Windows>"), "{screen}");
    // cmd.exe says nothing; its process's own directory is read instead.
    h.cli(&["new-window", "-d", "-t", "cw", "-n", "c"]).await;
    h.wait_capture("cw:1", "cmd prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "cw:1", "cd /d C:\\Windows\\System32", "Enter"]).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, out, _) = h.cli(&["display-message", "-p", "-t", "cw:1", "#{pane_current_path}"]).await;
        if out.trim().eq_ignore_ascii_case("C:\\Windows\\System32") {
            break;
        }
        assert!(Instant::now() < deadline, "cmd's cd was not followed: {out:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // ...and both land in the session file, so a resumed pane starts there.
    h.cli(&["save-session", "-t", "cw"]).await;
    let file = std::fs::read_dir(&h.sessions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .find(|t| t.contains("\"name\": \"cw\""))
        .expect("a saved file for cw");
    assert!(
        file.contains("\"cwd\": \"C:\\\\Windows\"") && file.contains("\"cwd\": \"C:\\\\Windows\\\\System32\""),
        "{file}"
    );
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_pane_keeps_its_history_through_a_resize() {
    let h = Harness::start("resizeresume").await;
    h.cli(&["new", "-d", "-s", "keeper"]).await;
    h.cli(&["new", "-d", "-s", "r"]).await;
    h.wait_capture("r:0", "prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "r:0", &count_to(40, "keep-"), "Enter"]).await;
    h.wait_capture("r:0", "the last line", |t| t.contains("keep-40")).await;
    h.cli(&["save-session", "-t", "r"]).await;
    h.cli(&["kill-session", "-t", "r"]).await;
    h.cli(&["resume", "r"]).await;
    // Everything back, then a fresh prompt below it.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, out, _) = h.cli(&["capture-pane", "-p", "-S", "-", "-t", "r:0"]).await;
        let n = out.lines().filter(|l| l.trim_end().starts_with("keep-")).count();
        let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        if n == 40 && last.trim_end() == "keepane>" {
            break;
        }
        assert!(Instant::now() < deadline, "{n} keep-lines, last {last:?}: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The pane shrinks (a split) and grows again (the split closed): the
    // 40 lines are all still there both times.
    h.cli(&["split-window", "-d", "-t", "r:0"]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, out, _) = h.cli(&["capture-pane", "-p", "-S", "-", "-t", "r:0.0"]).await;
    assert_eq!(out.lines().filter(|l| l.trim_end().starts_with("keep-")).count(), 40, "after the shrink: {out}");
    h.cli(&["kill-pane", "-t", "r:0.1"]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, out, _) = h.cli(&["capture-pane", "-p", "-S", "-", "-t", "r:0.0"]).await;
    assert_eq!(out.lines().filter(|l| l.trim_end().starts_with("keep-")).count(), 40, "after the grow: {out}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_pane_dash_j_joins_wrapped_lines() {
    let h = Harness::start("joinlines").await;
    h.cli(&["new", "-d", "-s", "j", "-x", "30", "-y", "10"]).await;
    h.wait_capture("j:0", "prompt", |t| t.contains("keepane>")).await;
    // 50 x's in a 30-column pane: two screen rows, one line.
    let long = "x".repeat(50);
    h.cli(&["send-keys", "-t", "j:0", &format!("echo {long}"), "Enter"]).await;
    // Wait for the echo's own second row (20 x's alone on a row): the typed
    // command has the 50 x's too, wrapped as 20 after the prompt then 30, so
    // counting x's would be satisfied before cmd has answered (CI did that).
    h.wait_capture("j:0", "the echo", |t| t.lines().any(|l| l.trim_end() == "x".repeat(20))).await;
    let (_, split, _) = h.cli(&["capture-pane", "-p", "-t", "j:0"]).await;
    let (_, joined, _) = h.cli(&["capture-pane", "-p", "-J", "-t", "j:0"]).await;
    assert!(split.lines().any(|l| l.trim_end() == "x".repeat(30)), "wrapped at 30: {split}");
    assert!(split.lines().any(|l| l.trim_end() == "x".repeat(20)), "the rest on the next row: {split}");
    assert!(joined.lines().any(|l| l.trim_end() == long), "joined back: {joined}");
    assert!(!joined.lines().any(|l| l.trim_end() == "x".repeat(20)), "{joined}");
    // The typed command wrapped too (prompt + 55 chars), and comes back whole.
    assert!(joined.lines().any(|l| l.trim_end() == format!("keepane>echo {long}")), "{joined}");
    // -J and -e together: the reset goes at the end of the joined line only.
    let (_, coloured, _) = h.cli(&["capture-pane", "-p", "-J", "-e", "-S", "-", "-t", "j:0"]).await;
    assert!(coloured.lines().any(|l| l.trim_end().trim_end_matches("\x1b[0m") == long), "{coloured:?}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_newer_variables_come_from_the_live_tree() {
    let h = Harness::start("morevars").await;
    h.cli(&["new", "-d", "-s", "v"]).await;
    h.wait_capture("v:0", "prompt", |t| t.contains("keepane>")).await;
    h.cli(&["split-window", "-d", "-t", "v:0"]).await;
    h.cli(&["new-window", "-d", "-t", "v"]).await;
    let ask = |t: &'static str, f: &'static str| {
        let h = &h;
        async move { h.cli(&["display-message", "-p", "-t", t, f]).await.1.trim().to_string() }
    };
    // Two panes one above the other in 23 rows: the top one touches the top
    // and left and right edges but not the bottom; rows 0..10.
    assert_eq!(ask("v:0.0", "#{pane_at_top}#{pane_at_bottom}#{pane_at_left}#{pane_at_right}").await, "1011");
    assert_eq!(ask("v:0.1", "#{pane_at_top}#{pane_at_bottom}").await, "01");
    assert_eq!(ask("v:0.0", "#{pane_top},#{pane_left},#{pane_bottom},#{pane_right}").await, "0,0,10,79");
    assert_eq!(ask("v:0.1", "#{pane_top}").await, "12");
    // The window flags: first of two, not last; the second the other way.
    assert_eq!(ask("v:0", "#{window_start_flag}#{window_end_flag}").await, "10");
    assert_eq!(ask("v:1", "#{window_start_flag}#{window_end_flag}").await, "01");
    // window_layout is tmux's layout string: checksum, then the cells, a
    // top-to-bottom split in [] (the window was split that way)...
    let before = ask("v:0", "#{window_layout}").await;
    assert!(
        before.len() > 5
            && before[..4].chars().all(|c| c.is_ascii_hexdigit())
            && before.starts_with(&format!("{},80x", &before[..4])),
        "{before}"
    );
    assert!(before.contains('[') && !before.contains('{'), "{before}");
    // ...and a left-to-right one in {} after even-horizontal.
    h.cli(&["select-layout", "-t", "v:0", "even-horizontal"]).await;
    let even = ask("v:0", "#{window_layout}").await;
    assert!(even.contains('{') && !even.contains('['), "{even}");
    // select-layout takes the string back: the old arrangement returns, and
    // one with the wrong number of panes is refused.
    let (code, _, err) = h.cli(&["select-layout", "-t", "v:0", &before]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(ask("v:0", "#{window_layout}").await, before, "the layout string round-trips");
    let (code, _, err) = h.cli(&["select-layout", "-t", "v:0", "80x24,0,0,0"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("has 1 pane, the window 2"), "{err}");
    let (code, _, err) = h.cli(&["select-layout", "-t", "v:0", "0000,80x24,0,0{40x24,0,0,0,39x24,41,0,1}"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("checksum"), "{err}");
    // The cursor sits after the prompt; the scrollback grows with output.
    h.wait_capture("v:1", "prompt", |t| t.contains("keepane>")).await;
    assert_eq!(ask("v:1", "#{cursor_x},#{history_size},#{history_limit}").await, "8,0,5000");
    h.cli(&["send-keys", "-t", "v:1", &count_to(40, "hs-"), "Enter"]).await;
    h.wait_capture("v:1", "the loop", |t| t.contains("hs-40")).await;
    let n: usize = ask("v:1", "#{history_size}").await.parse().unwrap();
    assert!(n >= 18, "40 lines in a 22-row pane leave some in the scrollback: {n}");
    // pane_last: the pane active before the current one.
    h.cli(&["select-pane", "-t", "v:0.1"]).await;
    assert_eq!(ask("v:0.0", "#{pane_last}").await, "1");
    assert_eq!(ask("v:0.1", "#{pane_last}").await, "0");
    // pane_mode says copy-mode while in it.
    assert_eq!(ask("v:0.1", "[#{pane_mode}]").await, "[]");
    h.cli(&["copy-mode", "-t", "v:0.1"]).await;
    assert_eq!(ask("v:0.1", "#{pane_mode}").await, "copy-mode");
    // Times are unix seconds around now; session_activity is the newest output.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    for f in ["#{session_activity}", "#{session_last_attached}", "#{window_activity}"] {
        let t: i64 = ask("v:1", f).await.parse().unwrap_or(0);
        assert!((now - t).abs() < 120, "{f} = {t} vs now {now}");
    }
    // A script's client has a name and no session; client_prefix is 0.
    let (_, out, _) = h.cli(&["display-message", "-p", "#{client_name}|[#{client_session}]|#{client_prefix}"]).await;
    assert!(out.trim().starts_with("client-") && out.trim().ends_with("|[]|0"), "{out}");
    h.cli(&["kill-server"]).await;
}

// ------------------------------------------------------------ pane messages
// docs/design/mailbox.md: names and addresses, work modes, the envelope,
// hops, permissions, the inbox and the event log.

async fn pane_id(h: &Harness, target: &str) -> u32 {
    let (_, out, err) = h.cli(&["display-message", "-p", "-t", target, "#{pane_id}"]).await;
    out.trim().trim_start_matches('%').parse().unwrap_or_else(|_| panic!("pane id of {target}: {out:?} {err}"))
}

async fn ask_pane(h: &Harness, pane: u32, format: &str) -> String {
    let (_, out, _) = h.cli(&["display-message", "-p", "-t", &format!("%{pane}"), format]).await;
    out.trim().to_string()
}

/// The id `send-message` reported (`#12 ...`).
fn msg_id(out: &str) -> String {
    out.split_whitespace().next().unwrap_or_default().trim_start_matches('#').to_string()
}

/// Poll a format until it reads `want`.
async fn wait_format(h: &Harness, pane: u32, format: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got = ask_pane(h, pane, format).await;
        if got == want {
            return;
        }
        assert!(Instant::now() < deadline, "%{pane} {format}: {got:?}, waiting for {want:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A command that ends exactly at the pane's right edge keeps the first
/// line of its output: the rows counted for the command are the ones it
/// fills, however the shell moves down after it. A pane 30 wide and a
/// command one longer each time: one of them ends at the edge.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_ends_at_the_right_edge_keeps_its_first_line_of_output() {
    let h = Harness::start("edge").await;
    // A short prompt (`PS C:\> `): one longer than the pane wraps itself.
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let (code, _, err) =
        h.cli(&[&["new", "-d", "-s", "edge", "-x", "30", "-y", "40", "-c", root], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    let p = pane_id(&h, "edge:0.0").await;
    let t = format!("%{p}");
    h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    let say = if cfg!(windows) { "Write-Output" } else { "echo" };
    for pad in 0..30 {
        wait_format(&h, p, "#{pane_idle}", "1").await;
        let text = format!("{say} first-line; {say} second #{}", "x".repeat(pad));
        let (_, out, _) = h.cli(&["send-message", "-t", &t, "--", &text]).await;
        let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
        assert!(trace.contains("output:\nfirst-line\nsecond"), "padded by {pad}:\n{trace}");
    }
    h.cli(&["kill-server"]).await;
}

/// A message sent the moment shell pane `pane` is back at its prompt (the
/// marker seen, the screen not read yet) waits until the command before it
/// has been taken as done: typed in then, it would become the pane's current
/// message and take that command's end for its own. `first` prints
/// `first-out` after half a second, `second` prints `second-out`.
async fn sent_as_the_prompt_comes_back(h: &Harness, pane: u32, first: &str, second: &str) {
    let t = format!("%{pane}");
    wait_format(h, pane, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, first]).await;
    let a = msg_id(&out);
    // Taken at once: busy now. Free again at the marker; send right then.
    let deadline = Instant::now() + Duration::from_secs(20);
    while ask_pane(h, pane, "#{pane_idle}").await != "1" {
        assert!(Instant::now() < deadline, "the first command never ended");
    }
    let (_, out, _) = h.cli(&["send-message", "-t", &t, second]).await;
    let b = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &a, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{a} done")) && trace.contains("output:\nfirst-out"), "{trace}");
    let (_, trace, _) = h.cli(&["trace-message", &b, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{b} done")) && trace.contains("output:\nsecond-out"), "{trace}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_finds_its_pane_by_name_or_address_and_waits_to_be_read() {
    let h = Harness::start("mail-basic").await;
    h.cli(&["new", "-d", "-s", "m"]).await;
    h.cli(&["split-window", "-t", "m:0"]).await;
    let (a, b) = (pane_id(&h, "m:0.0").await, pane_id(&h, "m:0.1").await);
    let (code, _, err) = h.cli(&["rename-pane", "-t", &format!("%{a}"), "lead"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["rename-pane", "-t", &format!("%{b}"), "builder"]).await;
    // A name is unique, and never one a target would read as an id.
    let (code, _, err) = h.cli(&["rename-pane", "-t", "%lead", "builder"]).await;
    assert!(code != 0 && err.contains("taken"), "{err}");
    let (code, _, err) = h.cli(&["rename-pane", "-t", "%lead", "12"]).await;
    assert!(code != 0 && err.contains("pane id"), "{err}");
    // whoami, from inside the pane: its full address, place, name, mode.
    let (_, who, _) = h.cli_in(Some(b), &["whoami"]).await;
    let words: Vec<&str> = who.split_whitespace().collect();
    assert_eq!(words[1..], ["m:0.1", "builder", "normal"], "{who}");
    let addr_b = words[0].to_string();
    assert!(addr_b.starts_with('$') && addr_b.ends_with(&format!(".%{b}")), "{addr_b}");
    let addr_a = ask_pane(&h, a, "#{pane_address}").await;

    // By name, from the other pane: a normal pane keeps it for read-message.
    let (code, out, err) = h.cli_in(Some(a), &["send-message", "-t", "%builder", "hello there"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("queued for") && out.contains("waits for read-message"), "{out}");
    let id = msg_id(&out);
    // By full address, from outside every pane.
    let (code, _, err) = h.cli(&["send-message", "-t", &addr_b, "second"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(ask_pane(&h, b, "#{pane_name} #{pane_inbox} #{pane_work_mode}").await, "builder 2 normal");
    let (_, list, _) = h.cli(&["list-messages", "-t", "%builder"]).await;
    assert!(list.contains(&format!("#{id}  from {addr_a} lead (normal)")) && list.contains("hello there"), "{list}");
    assert!(list.contains("from user"), "{list}");

    // A pane that takes nothing on its own flags its window (`@`) while
    // that window is not the one in view.
    h.cli(&["new-window", "-d", "-t", "m", "-n", "aside"]).await;
    let aside = pane_id(&h, "m:aside").await;
    h.cli(&["send-message", "-t", &format!("%{aside}"), "for later"]).await;
    let (_, flags, _) = h.cli(&["display-message", "-p", "-t", "m:aside", "#{window_flags}"]).await;
    assert!(flags.contains('@'), "{flags:?}");
    // Taken by the pane itself: the header line (its fields), then the text.
    let (code, out, err) = h.cli_in(Some(b), &["read-message"]).await;
    assert_eq!(code, 0, "{err}");
    let (env, text) = out.split_once('\n').unwrap();
    assert_eq!(
        env,
        format!("[keepane id={id} task={id} from={addr_a} name=lead mode=normal to={addr_b} via=normal hop=0]")
    );
    assert_eq!(text.trim(), "hello there");
    let (_, trace, _) = h.cli(&["trace-message", &id]).await;
    assert!(trace.starts_with(&format!("#{id} read")), "{trace}");
    let (_, out, _) = h.cli_in(Some(b), &["read-message"]).await;
    assert!(out.contains(" from=user ") && out.ends_with("second"), "{out}");
    // `message-envelope json`: the JSON envelope, as before 0.17.
    h.cli(&["set", "-g", "message-envelope", "json"]).await;
    h.cli(&["send-message", "-t", "%builder", "in json"]).await;
    let (_, out, _) = h.cli_in(Some(b), &["read-message"]).await;
    assert!(out.starts_with(r#"{"keepane":1,"id":"#) && out.contains(r#""from":"user""#), "{out}");
    let (code, _, err) = h.cli(&["set", "-g", "message-envelope", "xml"]).await;
    assert!(code != 0 && err.contains("fields or json"), "{err}");
    h.cli(&["set", "-g", "message-envelope", "fields"]).await;
    let (code, _, err) = h.cli_in(Some(b), &["read-message"]).await;
    assert!(code != 0 && err.contains("no message"), "{err}");

    // read-message -w waits for the next one to arrive.
    let ((code, out, err), _) = tokio::join!(h.cli_in(Some(b), &["read-message", "-w", "15"]), async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        h.cli(&["send-message", "-t", "%builder", "third"]).await
    });
    assert_eq!(code, 0, "{err}");
    assert!(out.ends_with("third"), "{out}");

    // Waiting on a pane that closes meanwhile ends then, not at the timeout.
    h.cli(&["split-window", "-t", "m"]).await;
    let gone = pane_id(&h, "m:0.2").await;
    let started = Instant::now();
    let ((code, _, err), _) = tokio::join!(h.cli_in(Some(gone), &["read-message", "-w", "30"]), async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        h.cli(&["kill-pane", "-t", &format!("%{gone}")]).await
    });
    assert!(code != 0 && err.contains("gone"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5), "answered when the pane went: {:?}", started.elapsed());
    // An address is where the pane was: once it has moved, it is refused.
    h.cli(&["break-pane", "-t", "%builder"]).await;
    let (code, _, err) = h.cli(&["send-message", "-t", &addr_b, "lost?"]).await;
    assert!(code != 0 && err.contains("has moved"), "{err}");
    let (code, _, err) = h.cli(&["send-message", "-t", "%builder", "found"]).await;
    assert_eq!(code, 0, "the name still finds it: {err}");
    h.cli(&["kill-server"]).await;
}

/// A pane's TERM is `default-terminal`, whatever the server's own was (a
/// server started from a dumb terminal must not hand that on).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn panes_are_told_the_terminal_they_draw_on() {
    let h = Harness::start("term").await;
    h.cli(&["new", "-d", "-s", "t"]).await;
    // At the prompt first: typed ahead, the output would share its line.
    h.wait_capture("t:0", "a prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "t:0", "echo \"term=$TERM\"", "Enter"]).await;
    h.wait_capture("t:0", "the default", |t| t.lines().any(|l| l.trim() == "term=xterm-256color")).await;
    h.cli(&["set", "-g", "default-terminal", "screen-256color"]).await;
    h.cli(&["new-window", "-d", "-t", "t"]).await;
    h.wait_capture("t:1", "a prompt", |t| t.contains("keepane>")).await;
    h.cli(&["send-keys", "-t", "t:1", "echo \"term=$TERM\"", "Enter"]).await;
    h.wait_capture("t:1", "the option", |t| t.lines().any(|l| l.trim() == "term=screen-256color")).await;
    h.cli(&["kill-server"]).await;
}

/// The same in bash: POSIX syntax for the envelope and for several lines.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_pane_runs_what_it_is_sent_and_its_result_is_kept() {
    let h = Harness::start("mail-shell").await;
    h.cli(&["new", "-d", "-s", "sh", "bash"]).await;
    let p = pane_id(&h, "sh:0.0").await;
    let t = format!("%{p}");
    let (code, _, err) = h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    assert_eq!(code, 0, "{err}");
    // bash before 4.4 (macOS's own) does not say when a command starts:
    // Enter at the prompt stands in, so a failure is known there too.
    let failed = "failed";
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (code, out, err) = h.cli(&["send-message", "-t", &t, "-w", "30", "printf '%s%s\\n' ab cd"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("delivered"), "{out}");
    let id = msg_id(&out);
    let (code, trace, err) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(trace.starts_with(&format!("#{id} done")), "{trace}");
    assert!(trace.contains("output:\nabcd"), "{trace}");
    // A command wider than the pane: the line editor draws its prompt again
    // as the line wraps. That is not a prompt: the command runs on, busy.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let long = format!("sleep 1; echo {}", "x".repeat(160));
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "--", &long]).await;
    let id_long = msg_id(&out);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, trace, _) = h.cli(&["trace-message", &id_long]).await;
    assert!(trace.starts_with(&format!("#{id_long} delivered")), "still running at 0.5s: {trace}");
    assert_eq!(ask_pane(&h, p, "#{pane_idle}").await, "0");
    let (_, trace, _) = h.cli(&["trace-message", &id_long, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id_long} done")) && trace.contains(&"x".repeat(160)), "{trace}");
    // The envelope goes in front as an argument of `:`, which runs nothing.
    let screen = h
        .wait_capture("sh:0.0", "the envelope", |t| {
            t.replace('\n', "").contains(&format!(": 'keepane #{id} from user'; printf"))
        })
        .await;
    assert!(screen.contains("abcd"), "{screen}");
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "ls /keepane-not-here-xyz"]).await;
    let id = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id} {failed}")), "{trace}");
    // Several lines run as one script in the shell; its status is the last line's.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let multi = "kp_a=20\nkp_b=\"2'2\"\necho $((kp_a + ${kp_b%\\'*}${kp_b#*\\'}))\nls /keepane-not-here-xyz";
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "--", multi]).await;
    let id = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id} {failed}")), "{trace}");
    assert!(trace.contains("output:\n42\n"), "{trace}");
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "echo \"$kp_b\""]).await;
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    assert!(trace.contains("output:\n2'2"), "its variables stay in the shell: {trace}");
    sent_as_the_prompt_comes_back(&h, p, "sleep 0.5; echo first-out", "echo second-out").await;
    // Typed while its command runs: the pane is not free afterwards.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "sleep 0.8"]).await;
    let slow = msg_id(&out);
    h.cli(&["send-keys", "-t", &t, "-l", "ech"]).await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "echo next"]).await;
    let next = msg_id(&out);
    h.cli(&["trace-message", &slow, "-w", "30"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ask_pane(&h, p, "#{pane_idle}").await, "0");
    let (_, trace, _) = h.cli(&["trace-message", &next]).await;
    assert!(trace.starts_with(&format!("#{next} queued")), "{trace}");
    // The person clears the line and presses Enter: a clean prompt, free.
    h.cli(&["send-keys", "-t", &t, "C-u", "Enter"]).await;
    let (_, trace, _) = h.cli(&["trace-message", &next, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{next} done")) && trace.contains("output:\nnext"), "{trace}");
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["send-keys", "-t", &t, "-l", "ech"]).await;
    assert_eq!(ask_pane(&h, p, "#{pane_idle}").await, "0");
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "echo later"]).await;
    assert!(out.contains("busy"), "{out}");

    // A marker ahead of the text before it: done a moment after, with it.
    // The command's own marker is followed by its prompt's, a second one,
    // which on a busy machine would end the next message: its own pane, last.
    // (Only builtins after the marker: a pty keeps order, so the moment is
    // ConPTY's, and the PowerShell test times it; an external `sleep` here
    // can take longer than the moment just to start, on a macOS runner.)
    h.cli(&["new-window", "-d", "-t", "sh", "bash"]).await;
    let q = pane_id(&h, "sh:1.0").await;
    h.cli(&["set-work-mode", "-t", &format!("%{q}"), "shell"]).await;
    wait_format(&h, q, "#{pane_idle}", "1").await;
    let early = "printf '\\033]7777;keepane-prompt;sh\\007'; echo late-output";
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{q}"), "--", early]).await;
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    assert!(trace.contains("output:\nlate-output"), "{trace}");
    h.cli(&["kill-server"]).await;
}

/// zsh (macOS's own shell) gets the hook through its own start files: it
/// takes messages, several lines with quotes included, reports its
/// directory, and keeps a history file of its own. Skipped without zsh.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_zsh_pane_takes_messages_and_reports_its_directory() {
    if keepane::config::which("zsh").is_none() {
        eprintln!("no zsh here: skipped");
        return;
    }
    let h = Harness::start("mail-zsh").await;
    h.cli(&["new", "-d", "-s", "z", "-c", "/", "zsh"]).await;
    let p = pane_id(&h, "z:0.0").await;
    let t = format!("%{p}");
    h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let multi = "cd /usr\nkp=\"it's\"\necho \"$kp $PWD\"";
    let (code, out, err) = h.cli(&["send-message", "-t", &t, "-w", "30", "--", multi]).await;
    assert_eq!(code, 0, "{err}");
    let id = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id} done")), "{trace}");
    assert!(trace.contains("output:\nit's /usr"), "{trace}");
    wait_format(&h, p, "#{pane_current_path}", "/usr").await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &t, "ls /keepane-not-here-xyz"]).await;
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    assert!(trace.contains("failed"), "{trace}");
    // Its own history file, under the sessions directory.
    let dir = h.sessions_dir.join("psreadline");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("keepane-not-here-xyz")))
    {
        assert!(Instant::now() < deadline, "no history file in {} has the command", dir.display());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    h.cli(&["kill-server"]).await;
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_pane_runs_what_it_is_sent_and_its_result_is_kept() {
    let h = Harness::start("mail-shell").await;
    h.cli(&["new", "-d", "-s", "sh", "pwsh", "-NoLogo", "-NoProfile"]).await;
    let p = pane_id(&h, "sh:0.0").await;
    let (code, _, err) = h.cli(&["set-work-mode", "-t", &format!("%{p}"), "shell"]).await;
    assert_eq!(code, 0, "{err}");
    // keepane's own prompt hook says when it is free.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (code, out, err) =
        h.cli(&["send-message", "-t", &format!("%{p}"), "-w", "30", "Write-Output ('ab' + 'cd')"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("delivered"), "{out}");
    let id = msg_id(&out);
    let (code, trace, err) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(trace.starts_with(&format!("#{id} done")), "{trace}");
    assert!(trace.contains("output:\nabcd"), "{trace}");
    // The command ran with its envelope in front, as a comment.
    let screen = h
        .wait_capture("sh:0.0", "the envelope", |t| {
            t.replace('\n', "").contains(&format!("<# keepane #{id} from user #> Write-Output"))
        })
        .await;
    assert!(screen.contains("abcd"), "{screen}");
    // A command that fails is on record as failed.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "Get-Item C:\\keepane-not-here-xyz"]).await;
    let id = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id} failed")), "{trace}");
    // Several lines run as one command: one prompt, all the output, and a
    // failure inside it is the command's.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let multi = "$kp_a = 20\n$kp_b = 22\nWrite-Output ($kp_a + $kp_b)\nGet-Item C:\\keepane-not-here-xyz";
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "--", multi]).await;
    let id = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &id, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{id} failed")), "{trace}");
    if !trace.contains("output:\n42\n") {
        // Where the command sat on the screen tells which rows were taken
        // for it (it has once lost the first line of output on CI).
        let (_, screen, _) = h.cli(&["capture-pane", "-p", "-t", &format!("%{p}"), "-S", "-30"]).await;
        let (_, size, _) =
            h.cli(&["display-message", "-p", "-t", &format!("%{p}"), "#{pane_width}x#{pane_height}"]).await;
        panic!("{trace}\npane {}:\n{screen}", size.trim());
    }
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "Write-Output $kp_b"]).await;
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    assert!(trace.contains("output:\n22"), "its variables stay in the shell: {trace}");
    sent_as_the_prompt_comes_back(
        &h,
        p,
        "Start-Sleep -Milliseconds 500; Write-Output first-out",
        "Write-Output second-out",
    )
    .await;
    // Typed while its command runs: that text is on the next prompt's line,
    // so the pane is not free and the next message does not join it.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "Start-Sleep -Milliseconds 800"]).await;
    let slow = msg_id(&out);
    h.cli(&["send-keys", "-t", &format!("%{p}"), "-l", "Get-Da"]).await;
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "Write-Output next"]).await;
    let next = msg_id(&out);
    h.cli(&["trace-message", &slow, "-w", "30"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ask_pane(&h, p, "#{pane_idle}").await, "0");
    let (_, trace, _) = h.cli(&["trace-message", &next]).await;
    assert!(trace.starts_with(&format!("#{next} queued")), "{trace}");
    // The person clears the line and presses Enter: a clean prompt, free.
    // (Apart: ESC and CR together read as Alt+Enter.)
    h.cli(&["send-keys", "-t", &format!("%{p}"), "Escape"]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.cli(&["send-keys", "-t", &format!("%{p}"), "Enter"]).await;
    let (_, trace, _) = h.cli(&["trace-message", &next, "-w", "30"]).await;
    assert!(trace.starts_with(&format!("#{next} done")) && trace.contains("output:\nnext"), "{trace}");
    // Typing makes it busy: nothing lands in a half-typed line.
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["send-keys", "-t", &format!("%{p}"), "-l", "Get-Da"]).await;
    assert_eq!(ask_pane(&h, p, "#{pane_idle}").await, "0");
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{p}"), "Write-Output later"]).await;
    assert!(out.contains("busy"), "{out}");

    // ConPTY may pass the prompt marker ahead of the text before it: the
    // command is taken as done a moment after the marker, with its output.
    // Here the command itself sends a marker before its last line, and its
    // prompt then sends the real one: a second marker ConPTY never sends. On
    // a busy machine the two come further apart than the moment, and the
    // second would end whatever message went in after the first; so this
    // is in a pane of its own, and last.
    h.cli(&["new-window", "-d", "-t", "sh", "pwsh", "-NoLogo", "-NoProfile"]).await;
    let q = pane_id(&h, "sh:1.0").await;
    h.cli(&["set-work-mode", "-t", &format!("%{q}"), "shell"]).await;
    wait_format(&h, q, "#{pane_idle}", "1").await;
    // A shell that has run these commands before, as the one above had: a
    // fresh PowerShell takes its time over the first of each.
    let (_, out, _) =
        h.cli(&["send-message", "-t", &format!("%{q}"), "Start-Sleep -Milliseconds 10; Write-Output warm"]).await;
    h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    wait_format(&h, q, "#{pane_idle}", "1").await;
    let early = "[Console]::Write([char]27 + ']7777;keepane-prompt' + [char]27 + '\\'); Start-Sleep -Milliseconds 10; Write-Output late-output";
    let (_, out, _) = h.cli(&["send-message", "-t", &format!("%{q}"), "--", early]).await;
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out), "-w", "30"]).await;
    assert!(trace.contains("output:\nlate-output"), "{trace}");
    h.cli(&["kill-server"]).await;
}

/// `list-done` until a line has `needle`, or the test fails with what it had.
async fn wait_done(h: &Harness, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, out, _) = h.cli(&["list-done"]).await;
        if let Some(l) = out.lines().find(|l| l.contains(needle)) {
            return l.to_string();
        }
        assert!(Instant::now() < deadline, "no done with {needle:?}; list-done:\n{out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// One HTTP request taken on `listener`, answered 200 with `answer`: its
/// body.
async fn take_request(listener: &tokio::net::TcpListener, answer: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut s, _) =
        tokio::time::timeout(Duration::from_secs(20), listener.accept()).await.expect("no request").unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let body_at = loop {
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the connection closed early: {}", String::from_utf8_lossy(&buf));
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..body_at]).to_lowercase();
    let len: usize =
        head.lines().find_map(|l| l.strip_prefix("content-length:")).map(|v| v.trim().parse().unwrap()).unwrap_or(0);
    while buf.len() < body_at + len {
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        buf.extend_from_slice(&chunk[..n]);
    }
    let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
    s.write_all(reply.as_bytes()).await.unwrap();
    String::from_utf8_lossy(&buf[body_at..body_at + len]).into_owned()
}

/// A command that ran long enough in a named pane is told: kept for the
/// phone (`list-done`), its words and kind in the `pane-done` hook's
/// environment, and sent to the webhook in the chat's shape; a chat that
/// says no is heard. An agent's turn is told too, but not its first word.
#[tokio::test(flavor = "multi_thread")]
async fn a_pane_done_is_told_to_the_page_the_hook_and_a_webhook() {
    let h = Harness::start("done").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_url = format!("http://127.0.0.1:{}/hook", listener.local_addr().unwrap().port());
    let said = std::env::temp_dir().join(format!("keepane-done-hook-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&said);
    let write = if cfg!(windows) {
        format!(
            "Set-Content -LiteralPath '{}' -Value ($env:KEEPANE_DONE_KIND + '|' + $env:KEEPANE_DONE_TEXT)",
            said.display()
        )
    } else {
        format!("printf '%s|%s' \"$KEEPANE_DONE_KIND\" \"$KEEPANE_DONE_TEXT\" > '{}'", said.display())
    };
    for (k, v) in [("done-after", "0"), ("done-webhook", hook_url.as_str()), ("done-webhook-format", "feishu")] {
        let (code, _, err) = h.cli(&["set", "-g", k, v]).await;
        assert_eq!(code, 0, "{k}: {err}");
    }
    assert_eq!(h.cli(&["set-hook", "-g", "pane-done", "run-shell", &write]).await.0, 0);
    h.cli(&[&["new", "-d", "-s", "dn"], HOOKED_SHELL].concat()).await;
    let p = pane_id(&h, "dn:0.0").await;
    h.cli(&["rename-pane", "-t", &format!("%{p}"), "build"]).await;
    h.cli(&["set-work-mode", "-t", &format!("%{p}"), "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["send-keys", "-t", &format!("%{p}"), "echo done-here", "Enter"]).await;
    let line = wait_done(&h, " command ").await;
    assert!(line.contains("build (dn:0.0): echo done-here finished in 0s"), "{line}");
    // The chat's shape, the words starting with keepane (a keyword check).
    let body: serde_json::Value =
        serde_json::from_str(&take_request(&listener, "{\"code\":0,\"msg\":\"success\"}").await).unwrap();
    assert_eq!(body["msg_type"], "text", "{body}");
    let text = body["content"]["text"].as_str().unwrap();
    // What the command printed comes under the line.
    assert!(
        text.starts_with("keepane · ") && text.ends_with("build (dn:0.0): echo done-here finished in 0s\ndone-here"),
        "{text}"
    );
    // The hook, told in its environment.
    let deadline = Instant::now() + Duration::from_secs(20);
    let hooked = loop {
        if let Ok(t) = std::fs::read_to_string(&said)
            && !t.is_empty()
        {
            break t;
        }
        assert!(Instant::now() < deadline, "the hook never wrote {}", said.display());
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(hooked.trim(), "command|build (dn:0.0): echo done-here finished in 0s");
    // JSON for the phone: the newest number, and each with its pane.
    let (_, json, _) = h.cli(&["list-done", "-J"]).await;
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["done"][0]["pane"], format!("%{p}"), "{v}");
    assert_eq!(v["last"], v["done"][0]["id"]);
    let (_, none, _) = h.cli(&["list-done", "-a", &v["last"].to_string(), "-J"]).await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&none).unwrap()["done"], serde_json::json!([]));
    // An agent: its first word is its session starting, not a turn; the
    // next, after it was busy, is. A chat that answers 200 and says no is
    // heard in the messages.
    h.cli(&["new-window", "-d", "-t", "dn"]).await;
    let a = pane_id(&h, "dn:1.0").await;
    h.cli(&["rename-pane", "-t", &format!("%{a}"), "agent"]).await;
    h.cli(&["set-work-mode", "-t", &format!("%{a}"), "ai"]).await;
    assert_eq!(h.cli_in(Some(a), &["pane-ready"]).await.0, 0);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, list, _) = h.cli(&["list-done"]).await;
    assert!(!list.contains(" agent "), "a session starting is not a turn ending: {list}");
    h.cli(&["send-keys", "-t", &format!("%{a}"), "x"]).await;
    assert_eq!(h.cli_in(Some(a), &["pane-ready"]).await.0, 0);
    let line = wait_done(&h, " agent ").await;
    assert!(line.ends_with("agent (dn:1.0): the agent finished its turn"), "{line}");
    take_request(&listener, "{\"code\":19021,\"msg\":\"sign match fail\"}").await;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, msgs, _) = h.cli(&["show-messages"]).await;
        if msgs.contains("done-webhook: refused (19021): sign match fail") {
            break;
        }
        assert!(Instant::now() < deadline, "the refusal was not heard: {msgs}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = std::fs::remove_file(&said);
    h.cli(&["kill-server"]).await;
}

/// What counts: a pane with no name in `normal` mode only with `done-panes
/// all`; a command shorter than `done-after` not at all; a program exiting
/// only when `done-events` has `exit`; nothing with `none`.
#[tokio::test(flavor = "multi_thread")]
async fn what_counts_as_done_is_up_to_the_options() {
    let h = Harness::start("done-opts").await;
    h.cli(&[&["new", "-d", "-s", "do"], HOOKED_SHELL].concat()).await;
    let p = pane_id(&h, "do:0.0").await;
    let t = format!("%{p}");
    // Wait for the shell's first prompt, then back to normal.
    h.cli(&["set-work-mode", "-t", &t, "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["set-work-mode", "-t", &t, "normal"]).await;
    h.cli(&["set", "-g", "done-after", "0"]).await;
    h.cli(&["send-keys", "-t", &t, "echo one", "Enter"]).await;
    h.wait_capture("do:0.0", "one's output", |s| s.lines().any(|l| l.trim() == "one")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.cli(&["list-done"]).await.1.trim(), "", "a pane with no name, in normal mode");
    h.cli(&["set", "-g", "done-panes", "all"]).await;
    h.cli(&["send-keys", "-t", &t, "echo two", "Enter"]).await;
    let line = wait_done(&h, "echo two").await;
    assert!(line.contains(" command do:0.0: echo two finished in 0s"), "{line}");
    // Too short for done-after.
    h.cli(&["set", "-g", "done-after", "60"]).await;
    tokio::time::sleep(Duration::from_secs(3)).await; // past the per-pane gate
    h.cli(&["send-keys", "-t", &t, "echo three", "Enter"]).await;
    h.wait_capture("do:0.0", "three's output", |s| s.lines().any(|l| l.trim() == "three")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!h.cli(&["list-done"]).await.1.contains("echo three"), "shorter than done-after");
    // A program exiting, with exit in done-events; bad values refused.
    assert_ne!(h.cli(&["set", "-g", "done-events", "command bogus"]).await.0, 0);
    assert_ne!(h.cli(&["set", "-g", "done-panes", "some"]).await.0, 0);
    assert_ne!(h.cli(&["set", "-g", "done-webhook", "ftp://x"]).await.0, 0);
    assert_ne!(h.cli(&["set", "-g", "done-webhook-format", "xml"]).await.0, 0);
    h.cli(&["set", "-g", "done-events", "all"]).await;
    assert_eq!(h.cli(&["show", "-gv", "done-events"]).await.1.trim(), "command agent task exit");
    h.cli(&["set", "-g", "remain-on-exit", "on"]).await;
    h.cli(&["send-keys", "-t", &t, "exit", "Enter"]).await;
    let line = wait_done(&h, " exit ").await;
    assert!(line.contains("exited with 0"), "{line}");
    // An agent that finishes the message it was given: its turn and the
    // task end together, and are told once, as the turn. (A shell with no
    // prompt hook plays the agent: its prompt would read as the agent gone.)
    h.cli(&["set", "-g", "done-events", "agent task"]).await;
    h.cli(&["new-window", "-d", "-t", "do"]).await;
    let a = pane_id(&h, "do:1.0").await;
    h.cli(&["rename-pane", "-t", &format!("%{a}"), "agent2"]).await;
    h.cli(&["set-work-mode", "-t", &format!("%{a}"), "ai"]).await;
    assert_eq!(h.cli_in(Some(a), &["pane-ready"]).await.0, 0);
    let (_, sent, _) = h.cli(&["send-message", "-t", &format!("%{a}"), "task one"]).await;
    let id = msg_id(&sent);
    let (_, trace, _) = h.cli(&["trace-message", &id]).await;
    assert!(trace.starts_with(&format!("#{id} delivered")), "{trace}");
    assert_eq!(h.cli_in(Some(a), &["pane-ready"]).await.0, 0);
    let line = wait_done(&h, "agent2").await;
    assert!(line.contains(&format!(" agent   agent2 (do:1.0): the agent finished #{id}: task one")), "{line}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, list, _) = h.cli(&["list-done"]).await;
    assert_eq!(list.lines().filter(|l| l.contains("agent2")).count(), 1, "one telling, not two: {list}");
    h.cli(&["kill-window", "-t", "do:1"]).await;
    // None: nothing more.
    h.cli(&["set", "-g", "done-events", "none"]).await;
    let before = h.cli(&["list-done"]).await.1;
    h.cli(&["new-window", "-d", "-t", "do"]).await;
    let q = pane_id(&h, "do:1.0").await;
    h.cli(&["send-keys", "-t", &format!("%{q}"), "exit", "Enter"]).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(h.cli(&["list-done"]).await.1, before, "done-events none");
    h.cli(&["kill-server"]).await;
}

/// An agent that never says it is free (no turn-end hook) would leave its
/// messages waiting for ever, silently: the sender is told why, and how
/// to set the hook up, until the agent's first `pane-ready`.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_for_an_agent_never_heard_from_says_what_it_lacks() {
    let h = Harness::start("mail-unheard").await;
    h.cli(&[&["new", "-d", "-s", "uh"], HOOKED_SHELL].concat()).await;
    let p = pane_id(&h, "uh:0.0").await;
    let pp = format!("%{p}");
    // The shell at its prompt first: a late first prompt would read as the
    // agent gone.
    h.cli(&["set-work-mode", "-t", &pp, "shell"]).await;
    wait_format(&h, p, "#{pane_idle}", "1").await;
    h.cli(&["set-work-mode", "-t", &pp, "ai"]).await;
    assert_eq!(ask_pane(&h, p, "#{pane_unheard}").await, "1");
    let (code, out, err) = h.cli(&["send-message", "-t", &pp, "first"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("(ai, busy, 0 ahead)"), "{out}");
    assert!(out.contains(&format!("{pp} has not said it is free")) && out.contains("keepane setup"), "{out}");
    // Its agent's word, with nothing to deliver: heard from now on.
    h.cli(&["drop-message", &msg_id(&out)]).await;
    assert_eq!(h.cli_in(Some(p), &["pane-ready"]).await.0, 0);
    assert_eq!(ask_pane(&h, p, "#{pane_unheard}").await, "0");
    // Busy again (someone typing), but heard from: no advice.
    h.cli(&["send-keys", "-t", &pp, "x"]).await;
    let (_, out, _) = h.cli(&["send-message", "-t", &pp, "second"]).await;
    assert!(out.contains("(ai, busy, 0 ahead)") && !out.contains("has not said"), "{out}");
    // Into ai anew: not heard from until it says so again.
    h.cli(&["set-work-mode", "-t", &pp, "normal"]).await;
    h.cli(&["set-work-mode", "-t", &pp, "ai"]).await;
    assert_eq!(ask_pane(&h, p, "#{pane_unheard}").await, "1");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_takes_work_when_it_says_so_and_answers_along_the_chain() {
    let h = Harness::start("mail-ai").await;
    h.cli(&["new", "-d", "-s", "ai"]).await;
    // A window each, so the delivery fits on the agent's screen.
    h.cli(&["new-window", "-d", "-t", "ai"]).await;
    let (a, b) = (pane_id(&h, "ai:0.0").await, pane_id(&h, "ai:1.0").await);
    let (pa, pb) = (format!("%{a}"), format!("%{b}"));
    h.cli(&["set-work-mode", "-t", &pb, "ai"]).await;
    let (_, out, _) = h.cli_in(Some(a), &["send-message", "-t", &pb, "task one"]).await;
    assert!(out.contains("busy"), "not ready yet: {out}");
    let id = msg_id(&out);
    // The agent's word: in it goes, envelope first and end line last.
    let (code, _, err) = h.cli_in(Some(b), &["pane-ready"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, trace, _) = h.cli(&["trace-message", &id]).await;
    assert!(trace.starts_with(&format!("#{id} delivered")), "{trace}");
    h.wait_capture("ai:1.0", "the delivery", |t| {
        t.contains(&format!("[keepane id={id} ")) && t.contains(&format!("[keepane end={id}]"))
    })
    .await;
    // Its answer carries the chain on: same task, one hop more, re the task.
    let (code, _, err) = h.cli_in(Some(b), &["send-message", "-r", "result one"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, got, _) = h.cli_in(Some(a), &["read-message"]).await;
    assert!(
        got.contains(&format!(" task={id} ")) && got.contains(" hop=1") && got.contains(&format!(" re={id}]")),
        "{got}"
    );
    assert!(got.contains(" mode=ai ") && got.ends_with("result one"), "{got}");
    // Ready again: that one is done.
    h.cli_in(Some(b), &["pane-ready"]).await;
    let (_, trace, _) = h.cli(&["trace-message", &id]).await;
    assert!(trace.starts_with(&format!("#{id} done")), "{trace}");
    // The chain is one task: the order, the answer, how it went.
    let (_, tasks, _) = h.cli(&["list-tasks", "-t", "ai"]).await;
    assert!(
        tasks.lines().any(|l| l.starts_with(&format!("#{id} ")) && l.contains("done") && l.contains("task one")),
        "{tasks}"
    );
    let (_, steps, _) = h.cli(&["show-task", &id]).await;
    assert_eq!(steps.lines().count(), 2, "{steps}");
    assert!(steps.contains("\"result one\"") && steps.contains(" read "), "{steps}");
    let (_, events, _) = h.cli(&["list-events", "-t", &pb, "-S", "1h"]).await;
    assert!(events.contains(r#""what":"mode","value":"ai","by":"user""#), "{events}");
    // Having read the answer, something new from a starts a new chain:
    // reading is not working on it (a person would reach the hop limit
    // after a few exchanges otherwise).
    h.cli(&["set", "-g", "message-hop-limit", "1"]).await;
    let (code, out, err) = h.cli_in(Some(a), &["send-message", "-t", &pb, "and again"]).await;
    assert_eq!(code, 0, "{err}");
    let fresh = msg_id(&out);
    let (_, trace, _) = h.cli(&["trace-message", &fresh]).await;
    assert!(trace.contains(&format!("task #{fresh} · hop 0")), "{trace}");
    // Answering what it read carries that chain on, past the hop limit:
    // refused, so two agents cannot answer each other for ever.
    let (code, _, err) = h.cli_in(Some(a), &["send-message", "-r", "thanks"]).await;
    assert!(code != 0 && err.contains("message-hop-limit"), "{err}");
    // The fields a sender may give by name: what it answers (--re: to that
    // message's sender unless --to says otherwise) and the task it carries
    // on (--task); the hop is still keepane's, so the limit still holds.
    let (code, out, err) = h.cli(&["send-message", "--re", &fresh, "about that"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, got, _) = h.cli_in(Some(a), &["read-message"]).await;
    assert!(
        got.contains(&format!(" task={fresh} ")) && got.contains(" hop=1") && got.contains(&format!(" re={fresh}]")),
        "{out}: {got}"
    );
    let (code, _, err) = h.cli(&["send-message", "--to", &pb, "--task", &id, "more of it"]).await;
    assert!(code != 0 && err.contains("message-hop-limit"), "past the furthest hop of task #{id}: {err}");
    h.cli(&["set", "-g", "message-hop-limit", "8"]).await;
    let (code, out, err) = h.cli(&["send-message", "--to", &pb, "--task", &id, "more of it"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, trace, _) = h.cli(&["trace-message", &msg_id(&out)]).await;
    assert!(trace.contains(&format!("task #{id} · hop 2")), "{trace}");
    // Answering what came from outside any pane needs a place to go.
    let (_, out, _) = h.cli(&["send-message", "--to", &pb, "from outside"]).await;
    let outside = msg_id(&out);
    let (code, _, err) = h.cli(&["send-message", "--re", &outside, "x"]).await;
    assert!(code != 0 && err.contains("--re") && err.contains("say where with --to"), "{err}");
    let (code, _, err) = h.cli(&["send-message", "--re", &outside, "--to", &pb, "x"]).await;
    assert_eq!(code, 0, "with --to it goes: {err}");
    for (argv, why) in [
        (vec!["send-message", "--re", "999999", "x"], "no message #999999"),
        (vec!["send-message", "--to", &pb, "--task", "999999", "x"], "no task #999999"),
        (vec!["send-message", "--to", &pb, "--re", &fresh, "--task", &id, "x"], "is in task"),
        (vec!["send-message", "--to", &pb, "--from", "%x", "x"], "keepane's to fill in"),
        (vec!["send-message", "--to", &pb, "--hop", "0", "x"], "keepane's to fill in"),
        (vec!["send-message", "--re", "x1", "x"], "a message number"),
    ] {
        let (code, _, err) = h.cli(&argv).await;
        assert!(code != 0 && err.contains(why), "{argv:?}: {err}");
    }
    // pane-ready counts only in an ai pane; outside any pane -q is quiet.
    assert_eq!(h.cli_in(Some(a), &["pane-ready"]).await.0, 0);
    let (code, _, err) = h.cli(&["pane-ready"]).await;
    assert!(code != 0 && err.contains("not run inside"), "{err}");
    assert_eq!(h.cli(&["pane-ready", "-q"]).await.0, 0);
    // A work mode is changed in the pane itself: nothing run in one pane
    // turns another into a shell that runs what it is sent...
    let (code, _, err) = h.cli_in(Some(a), &["set-work-mode", "-t", &pb, "shell"]).await;
    assert!(code != 0 && err.contains("changes only the pane it runs in"), "{err}");
    assert_eq!(ask_pane(&h, b, "#{pane_work_mode}").await, "ai");
    // ...in the pane itself, whatever the pane is, and from outside every
    // pane (a terminal, the C-b : prompt).
    assert_eq!(h.cli_in(Some(b), &["set-work-mode", "normal"]).await.0, 0);
    assert_eq!(h.cli_in(Some(b), &["set-work-mode", "-t", &pb, "ai"]).await.0, 0, "-t naming itself");
    assert_eq!(h.cli(&["set-work-mode", "-t", &pb, "shell"]).await.0, 0);
    // Names are anyone's to give.
    assert_eq!(h.cli_in(Some(a), &["rename-pane", "-t", &pb, "worker"]).await.0, 0);
    assert_eq!(h.cli_in(Some(b), &["rename-pane", "-t", &pa, "boss"]).await.0, 0);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_messages_are_managed_limited_and_logged() {
    let h = Harness::start("mail-queue").await;
    h.cli(&["new", "-d", "-s", "q"]).await;
    // Another session keeps the server up when q's only pane closes.
    h.cli(&["new", "-d", "-s", "keep"]).await;
    let p = pane_id(&h, "q:0.0").await;
    let t = format!("%{p}");
    let mut ids = Vec::new();
    for text in ["one", "two", "three"] {
        let (_, out, _) = h.cli(&["send-message", "-t", &t, text]).await;
        ids.push(msg_id(&out));
    }
    let order = |list: &str| -> Vec<String> {
        list.lines()
            .filter_map(|l| l.trim().strip_prefix('#'))
            .map(|l| l.split_whitespace().next().unwrap().to_string())
            .collect()
    };
    h.cli(&["move-message", &ids[2], "top"]).await;
    let (_, list, _) = h.cli(&["list-messages", "-t", &t]).await;
    assert_eq!(order(&list), [ids[2].clone(), ids[0].clone(), ids[1].clone()], "{list}");
    // Deleted, then brought back where it was.
    assert_eq!(h.cli(&["drop-message", &ids[0]]).await.0, 0);
    let (_, trace, _) = h.cli(&["trace-message", &ids[0]]).await;
    assert!(trace.contains("dropped") && trace.contains("deleted by user"), "{trace}");
    let (code, _, err) = h.cli(&["drop-message", "-u"]).await;
    assert_eq!(code, 0, "{err}");
    let (_, list, _) = h.cli(&["list-messages", "-t", &t]).await;
    assert_eq!(order(&list), [ids[2].clone(), ids[0].clone(), ids[1].clone()], "{list}");
    // Limits are options, and say so when they refuse.
    h.cli(&["set", "-g", "message-inbox-limit", "3"]).await;
    let (code, _, err) = h.cli(&["send-message", "-t", &t, "four"]).await;
    assert!(code != 0 && err.contains("inbox is full"), "{err}");
    h.cli(&["set", "-g", "message-max-size", "1K"]).await;
    let (code, _, err) = h.cli(&["send-message", "-t", "q", &"x".repeat(2000)]).await;
    assert!(code != 0 && err.contains("message-max-size"), "{err}");
    let (code, _, err) = h.cli(&["set", "-g", "message-hop-limit", "0"]).await;
    assert!(code != 0 && err.contains("1 to 100"), "{err}");
    // A pane that closes drops what waited for it.
    h.cli(&["kill-pane", "-t", &t]).await;
    let (_, trace, _) = h.cli(&["trace-message", &ids[1]]).await;
    assert!(trace.contains("dropped") && trace.contains("its pane closed"), "{trace}");
    // Everything above is in the event log, envelope and all.
    keepane::histlog::flush(Duration::from_secs(5));
    let dir = keepane::histlog::events_dir().join(keepane::histlog::safe_name(&h.socket));
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log = std::fs::read_to_string(dir.join(format!("{day}.jsonl"))).unwrap_or_default();
    // The last lines, as a watcher asks for them every second: from memory.
    let (_, tail, _) = h.cli(&["list-events", "-n", "2"]).await;
    let tail: Vec<&str> = tail.lines().collect();
    assert_eq!(tail.len(), 2, "{tail:?}");
    assert!(tail.iter().all(|l| log.contains(l)), "the same lines as the file: {tail:?}");
    // Off is off at once: nothing more is written.
    let lines = log.lines().count();
    h.cli(&["set", "-g", "event-log", "off"]).await;
    h.cli(&["send-message", "-t", "keep", "unlogged"]).await;
    keepane::histlog::flush(Duration::from_secs(5));
    let after = std::fs::read_to_string(dir.join(format!("{day}.jsonl"))).unwrap_or_default();
    assert_eq!(after.lines().count(), lines, "event-log off still wrote:\n{after}");
    for want in [
        format!(r#""ev":"sent","msg":{{"keepane":1,"id":{},"#, ids[0]),
        format!(r#""ev":"moved","id":{},"from":2,"to":0"#, ids[2]),
        format!(r#""ev":"dropped","id":{},"why":"deleted by user""#, ids[0]),
        format!(r#""ev":"restored","id":{}"#, ids[0]),
        r#""ev":"rejected""#.to_string(),
    ] {
        assert!(log.contains(&want), "{want} not in the event log:\n{log}");
    }
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_keeps_its_name_and_work_mode_through_save_and_restore() {
    let h = Harness::start("mail-save").await;
    h.cli(&["new", "-d", "-s", "keep"]).await;
    h.cli(&["new", "-d", "-s", "sv"]).await;
    let p = pane_id(&h, "sv:0.0").await;
    h.cli(&["rename-pane", "-t", &format!("%{p}"), "saved-one"]).await;
    h.cli(&["set-work-mode", "-t", "%saved-one", "ai"]).await;
    h.cli(&["send-message", "-t", "%saved-one", "not kept"]).await;
    let (code, _, err) = h.cli(&["save-session", "-t", "sv"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["kill-session", "-t", "sv"]).await;
    let (code, _, err) = h.cli(&["restore-session", "sv"]).await;
    assert_eq!(code, 0, "{err}");
    let q = pane_id(&h, "%saved-one").await;
    assert_ne!(p, q, "a new pane, found by the old name");
    assert_eq!(ask_pane(&h, q, "#{pane_work_mode} #{pane_inbox}").await, "ai 0", "the mode is back, the inbox is not");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_makes_panes_within_its_limits_and_owns_them() {
    let h = Harness::start("mail-create").await;
    h.cli(&["new", "-d", "-s", "boss"]).await;
    h.cli(&["new", "-d", "-s", "other"]).await;
    let (a, o) = (pane_id(&h, "boss:0.0").await, pane_id(&h, "other:0.0").await);
    // Both are agents' panes: what they make is theirs, within limits.
    h.cli(&["set-work-mode", "-t", &format!("%{a}"), "ai"]).await;
    h.cli(&["set-work-mode", "-t", &format!("%{o}"), "ai"]).await;
    // A plain program not in the default agent-commands, and a shell
    // keepane hands commands to.
    let plain = if cfg!(windows) { "cmd.exe" } else { "cat" };
    let shell: &[&str] = if cfg!(windows) { &["pwsh", "-NoLogo", "-NoProfile"] } else { &["bash"] };
    // Only agent-commands programs, from inside a pane.
    let (code, _, err) = h.cli_in(Some(a), &["create-pane", "-k", "window", "--", plain]).await;
    assert!(code != 0 && err.contains("agent-commands"), "{err}");
    h.cli(&["set", "-g", "agent-commands", if cfg!(windows) { "cmd pwsh" } else { "cat bash" }]).await;
    let (code, out, err) = h
        .cli_in(Some(a), &["create-pane", "-k", "window", "-n", "child", "-m", "ai", "-M", "first task", "--", plain])
        .await;
    assert_eq!(code, 0, "{err}");
    let mut lines = out.lines();
    let who: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
    assert_eq!(who[2..], ["child", "ai"], "{out}");
    assert!(lines.next().unwrap_or_default().contains("queued"), "the first task waits for it: {out}");
    // Once made, its mode is changed in it: not even its creator switches
    // it (to a shell that runs what it is sent) from elsewhere.
    let (code, _, err) = h.cli_in(Some(a), &["set-work-mode", "-t", "%child", "shell"]).await;
    assert!(code != 0 && err.contains("changes only the pane it runs in"), "{err}");
    // A shell made for an agent takes commands unless told otherwise.
    let (_, out, err) = h.cli_in(Some(a), &[&["create-pane", "-n", "sh1", "--"], shell].concat()).await;
    assert!(out.contains(" sh1  shell"), "{out} {err}");
    // The line of creators has one budget.
    h.cli(&["set", "-g", "agent-pane-limit", "3"]).await;
    let c = pane_id(&h, "%child").await;
    assert_eq!(h.cli_in(Some(c), &["create-pane", "-k", "window", "--", plain]).await.0, 0, "grandchild");
    let (code, _, err) = h.cli_in(Some(a), &["create-pane", "-k", "window", "--", plain]).await;
    assert!(code != 0 && err.contains("agent-pane-limit 3"), "{err}");
    // A person is not an agent: no list, no budget, no owner.
    assert_eq!(h.cli(&["create-pane", "-k", "window", "-t", "other", "--", plain]).await.0, 0);
    // Closing what it made frees the budget.
    assert_eq!(h.cli_in(Some(a), &["kill-pane", "-t", "%sh1"]).await.0, 0);
    assert_eq!(h.cli_in(Some(a), &["create-pane", "-k", "window", "--", plain]).await.0, 0);
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_talks_to_keepane_over_mcp_as_its_pane() {
    let h = Harness::start("mcp").await;
    h.cli(&["new", "-d", "-s", "m"]).await;
    h.cli(&["new-window", "-d", "-t", "m"]).await;
    let (a, b) = (pane_id(&h, "m:0.0").await, pane_id(&h, "m:1.0").await);
    h.cli(&["rename-pane", "-t", &format!("%{b}"), "peer"]).await;
    // The agent's pane.
    h.cli(&["set-work-mode", "-t", &format!("%{a}"), "ai"]).await;
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"whoami","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"send_message","arguments":{"to":"%peer","text":"-from mcp"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_messages","arguments":{"pane":"%peer"}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"kill_pane","arguments":{"pane":"%peer"}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"current_message","arguments":{}}}"#,
    ];
    let socket = h.socket.clone();
    let lines = tokio::task::spawn_blocking(move || {
        use std::io::{BufRead, Write};
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_keepane"))
            .args(["-L", &socket, "mcp"])
            // A pane's program has both: its server's socket and its pane.
            .env("KEEPANE", &socket)
            .env("KEEPANE_PANE", a.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
        drop(stdin); // the agent is done: the server ends
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        let lines: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
        assert!(child.wait().unwrap().success());
        lines
    })
    .await
    .unwrap();
    assert_eq!(lines.len(), 6, "a reply for every request, none for the notification: {lines:?}");
    let text = |i: usize| lines[i]["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let error = |i: usize| lines[i]["result"]["isError"].as_bool().unwrap_or(false);
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "keepane");
    assert!(text(1).contains("m:0.0") && !error(1), "it is the pane it runs in: {}", text(1));
    assert!(text(2).contains("queued for") && !error(2), "{}", text(2));
    assert!(text(3).contains("-from mcp") && text(3).contains(&format!(".%{a} (ai)")), "sent as its pane: {}", text(3));
    assert!(!error(4), "a pane closed (kept a moment for undo-kill): {}", text(4));
    assert!(text(5).contains("not working on a message"), "{}", text(5));
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_taken_while_its_pane_was_killed_stays_with_the_taker() {
    let h = Harness::start("mail-undo").await;
    h.cli(&["new", "-d", "-s", "u"]).await;
    h.cli(&["split-window", "-t", "u"]).await;
    let (a, b) = (pane_id(&h, "u:0.0").await, pane_id(&h, "u:0.1").await);
    h.cli(&["rename-pane", "-t", &format!("%{b}"), "solo"]).await;
    h.cli(&["kill-pane", "-t", "%solo"]).await;
    // While it is kept for undo-kill, its name looks free, and is taken.
    assert_eq!(h.cli(&["rename-pane", "-t", &format!("%{a}"), "solo"]).await.0, 0);
    let (code, _, err) = h.cli(&["undo-kill"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(ask_pane(&h, b, "[#{pane_name}]").await, "[]", "the pane that came back has none");
    assert_eq!(pane_id(&h, "%solo").await, a, "the name stays with the one that took it");
    let (_, names, _) = h.cli(&["list-panes", "-a", "-F", "#{pane_name}"]).await;
    assert_eq!(names.lines().filter(|n| *n == "solo").count(), 1, "{names}");
    h.cli(&["kill-server"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_made_for_an_agent_is_the_one_it_asked_for_whatever_hooks_do() {
    let h = Harness::start("mail-hook").await;
    h.cli(&["new", "-d", "-s", "hk"]).await;
    // A hook that makes one more pane in every new window.
    h.cli(&["set-hook", "-g", "after-new-window", "split-window -d"]).await;
    let (code, out, err) =
        h.cli(&["create-pane", "-k", "window", "-t", "hk", "-n", "mine", "-m", "ai", "--", SH]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("hk:1.0  mine  ai"), "the window's own pane, not the hook's: {out}");
    // (The hook split the window in view, hk:0.)
    assert_eq!(ask_pane(&h, pane_id(&h, "hk:0.1").await, "[#{pane_name}] #{pane_work_mode}").await, "[] normal");
    h.cli(&["kill-server"]).await;
}

/// The last line of a pane's text that is not blank.
fn last_line(screen: &str) -> String {
    screen.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim_end().to_string()
}

/// A shell keepane gives its hook to (PowerShell; bash).
#[cfg(windows)]
const HOOKED_SHELL: &[&str] = &["pwsh", "-NoLogo", "-NoProfile"];
#[cfg(unix)]
const HOOKED_SHELL: &[&str] = &["bash"];

/// The history file `HOOKED_SHELL` shares between its sessions.
fn shared_history_file() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(|a| std::path::Path::new(&a).join(r"Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt"))
    } else {
        std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".bash_history"))
    }
}

/// Wait until `HOOKED_SHELL` in `target` is at its prompt, as keepane's
/// prompt marker says. Not by what the screen shows: a resumed pane shows
/// the prompt it was saved at before its new shell has even started.
async fn at_hooked_prompt(h: &Harness, target: &str) {
    h.cli(&["set-work-mode", "-t", target, "shell"]).await;
    wait_format(h, pane_id(h, target).await, "#{pane_idle}", "1").await;
}

/// Every hooked shell pane keeps its own command history, in a file under the
/// sessions directory: a pane split off starts from the one it came from,
/// the two go their own ways, a resumed pane has what it had, and the
/// shell's shared file is not written.
#[tokio::test(flavor = "multi_thread")]
async fn each_shell_pane_keeps_its_own_command_history() {
    let h = Harness::start("shell-history").await;
    // Marks of this run's own: a run that failed half way leaves its marks
    // behind, and those must not fail the next one.
    let (one, two) = (format!("kp-hist-one-{}", std::process::id()), format!("kp-hist-two-{}", std::process::id()));
    h.cli(&["new", "-d", "-s", "keeper"]).await;
    let (code, _, err) = h.cli(&[&["new", "-d", "-s", "hist", "-x", "100", "-y", "30"], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    at_hooked_prompt(&h, "hist:0.0").await;
    h.cli(&["send-keys", "-t", "hist:0.0", &format!("echo {one}"), "Enter"]).await;
    h.wait_capture("hist:0.0", "the command", |t| t.matches(one.as_str()).count() >= 2).await;
    // Its file, under the sessions directory.
    let dir = h.sessions_dir.join("psreadline");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let written =
            std::fs::read_dir(&dir).into_iter().flatten().flatten().any(|e| {
                std::fs::read_to_string(e.path()).is_ok_and(|t| t.lines().any(|l| l == format!("echo {one}")))
            });
        if written {
            break;
        }
        assert!(Instant::now() < deadline, "no history file in {} has the command", dir.display());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // A pane split off has it too.
    let (code, _, err) = h.cli(&[&["split-window", "-t", "hist:0.0"], HOOKED_SHELL].concat()).await;
    assert_eq!(code, 0, "{err}");
    at_hooked_prompt(&h, "hist:0.1").await;
    h.cli(&["send-keys", "-t", "hist:0.1", "Up"]).await;
    h.wait_capture("hist:0.1", "the first pane's command", |t| last_line(t).ends_with(&format!("echo {one}"))).await;
    // From here on, each its own.
    h.cli(&["send-keys", "-t", "hist:0.1", "C-c"]).await;
    // The shell takes C-c in its own time: what is typed before its new
    // prompt can be cut into.
    at_hooked_prompt(&h, "hist:0.1").await;
    h.cli(&["send-keys", "-t", "hist:0.1", &format!("echo {two}"), "Enter"]).await;
    h.wait_capture("hist:0.1", "the command", |t| t.matches(two.as_str()).count() >= 2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.cli(&["send-keys", "-t", "hist:0.0", "Up"]).await;
    let first =
        h.wait_capture("hist:0.0", "its own last command", |t| last_line(t).ends_with(&format!("echo {one}"))).await;
    assert!(!first.contains(two.as_str()), "the other pane's command is not in this one's:\n{first}");

    // Started again in place, a pane keeps its file.
    h.cli(&["send-keys", "-t", "hist:0.0", "C-c"]).await;
    let (code, _, err) = h.cli(&["respawn-pane", "-k", "-t", "hist:0.0"]).await;
    assert_eq!(code, 0, "{err}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    at_hooked_prompt(&h, "hist:0.0").await;
    h.cli(&["send-keys", "-t", "hist:0.0", "Up"]).await;
    h.wait_capture("hist:0.0", "its command after respawn", |t| last_line(t).ends_with(&format!("echo {one}"))).await;
    h.cli(&["send-keys", "-t", "hist:0.0", "C-c"]).await;

    // Saved, killed and resumed: each pane has its own history back.
    let (code, _, err) = h.cli(&["save-session", "-t", "hist"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["kill-session", "-t", "hist"]).await;
    let mut c = h.connect().await;
    assert_eq!(c.attach(&["resume", "hist"]).await, "hist");
    at_hooked_prompt(&h, "hist:0.1").await;
    h.cli(&["send-keys", "-t", "hist:0.1", "Up"]).await;
    h.wait_capture("hist:0.1", "its own last command", |t| last_line(t).ends_with(&format!("echo {two}"))).await;
    at_hooked_prompt(&h, "hist:0.0").await;
    h.cli(&["send-keys", "-t", "hist:0.0", "Up"]).await;
    h.wait_capture("hist:0.0", "its own last command", |t| last_line(t).ends_with(&format!("echo {one}"))).await;

    // A saved pane whose file is gone starts over from PowerShell's shared
    // file, under the same name, rather than from nothing.
    let names: Vec<std::path::PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(names.len(), 2, "one file per pane: {names:?}");
    let (code, _, err) = h.cli(&["save-session", "-t", "hist"]).await;
    assert_eq!(code, 0, "{err}");
    h.cli(&["kill-session", "-t", "hist"]).await;
    for p in &names {
        std::fs::remove_file(p).unwrap();
    }
    let mut c = h.connect().await;
    assert_eq!(c.attach(&["resume", "hist"]).await, "hist");
    at_hooked_prompt(&h, "hist:0.1").await;
    let shared = shared_history_file();
    if shared.as_ref().is_some_and(|f| f.is_file()) {
        assert!(names.iter().all(|p| p.is_file()), "both files are back: {names:?}");
    }
    // The shell's shared history has none of it.
    if let Some(shared) = shared {
        let text = std::fs::read_to_string(shared).unwrap_or_default();
        assert!(!text.contains(one.as_str()) && !text.contains(two.as_str()), "the shared history file was written");
    }
}

/// `list-agents` and the `agent_*` formats: a pane running an agent (here a
/// stand-in program named `claude`) is read from the transcript it writes,
/// as the transcript grows; its tokens priced by the list given; `agent-cost
/// off` leaves cost out; a pane whose agent has gone has none.
#[tokio::test(flavor = "multi_thread")]
async fn a_panes_agent_is_read_from_its_transcript() {
    let root = std::env::temp_dir().join(format!("keepane-agent-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (bin, work, cfg) = (root.join("bin"), root.join("work"), root.join("cfg"));
    for d in [&bin, &work, &cfg] {
        std::fs::create_dir_all(d).unwrap();
    }
    // A program that waits, under the agent's name.
    #[cfg(windows)]
    let argv = {
        let exe = bin.join("claude.exe");
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        vec![exe.to_string_lossy().into_owned(), "-n".into(), "600".into(), "127.0.0.1".into()]
    };
    // Linux: a script (its process is named after it; a copy of `sleep` may
    // be one program for all of coreutils, which goes by the name it is run as).
    #[cfg(target_os = "linux")]
    let argv = {
        use std::os::unix::fs::PermissionsExt;
        let exe = bin.join("claude");
        std::fs::write(&exe, "#!/bin/sh\nsleep 600\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        vec![exe.to_string_lossy().into_owned()]
    };
    #[cfg(target_os = "macos")]
    let argv = {
        let exe = bin.join("claude");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        vec![exe.to_string_lossy().into_owned(), "600".into()]
    };
    let prices = root.join("prices.json");
    std::fs::write(
        &prices,
        r#"{"claude-opus-5-5": {"input_cost_per_token": 4e-06, "output_cost_per_token": 2e-05,
            "cache_read_input_token_cost": 2e-07, "max_input_tokens": 100000}}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &cfg);
        std::env::set_var("KEEPANE_PRICE_LIST", &prices);
    }
    let h = Harness::start("agents").await;
    let w = work.to_string_lossy().into_owned();
    let (code, _, err) = h.cli(&args(&["new-session", "-d", "-s", "ag", "-c", &w], &argv)).await;
    assert_eq!(code, 0, "{err}");
    // Claude Code's folder for the directory, as the system names it.
    #[cfg(unix)]
    let w = work.canonicalize().unwrap().to_string_lossy().into_owned();
    let folder: String = w.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let project = cfg.join("projects").join(folder);
    std::fs::create_dir_all(&project).unwrap();
    let reply = |id: &str, input: u64, output: u64, cached: u64| {
        format!(
            "{}\n",
            serde_json::json!({"type": "assistant", "message": {"id": id, "model": "claude-opus-5-5",
                "content": [{"type": "tool_use"}],
                "usage": {"input_tokens": input, "output_tokens": output, "cache_read_input_tokens": cached}}})
        )
    };
    let transcript = project.join("s.jsonl");
    std::fs::write(&transcript, reply("m1", 1000, 2000, 10000)).unwrap();
    let agents = || h.cli(&["list-agents", "-J"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let first = loop {
        let (_, out, _) = agents().await;
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
        if v[0]["turns"] == 1 && v[0]["cost"].is_number() {
            break v;
        }
        assert!(Instant::now() < deadline, "the agent and its cost: {out}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let a = &first[0];
    assert_eq!(
        (a["agent"].as_str(), a["model"].as_str(), a["target"].as_str()),
        (Some("claude"), Some("claude-opus-5-5"), Some("ag:0.0"))
    );
    assert_eq!(
        (a["tokens"]["total"].as_u64(), a["context_pct"].as_u64(), a["tools"].as_u64()),
        (Some(13000), Some(11), Some(1))
    );
    let cost = a["cost"].as_f64().unwrap();
    assert!((cost - 0.046).abs() < 1e-9, "1000 in, 2000 out, 10000 read from the cache: {cost}");
    let (_, out, _) = h
        .cli(&[
            "display-message",
            "-p",
            "-t",
            "ag",
            "#{agent} #{agent_cost} #{agent_tokens} #{agent_context} #{agent_turns}",
        ])
        .await;
    assert_eq!(out.trim(), "claude $0.05 13k 11% 1");
    let (_, plain, _) = h.cli(&["list-agents"]).await;
    assert!(
        plain.starts_with("%") && plain.contains(" ag:0.0 claude claude-opus-5-5 cost $0.05 tokens 13k context 11%"),
        "{plain}"
    );

    // It grows: only what is new is read, and counted once.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .and_then(|mut f| std::io::Write::write_all(&mut f, reply("m2", 500, 100, 40000).as_bytes()))
        .unwrap();
    h.wait_for_cli(
        "the second reply",
        &["display-message", "-p", "-t", "ag", "#{agent_turns} #{agent_context}"],
        |_, out| out.trim() == "2 40%",
    )
    .await;

    // Cost off: none said; the context still a share of the window.
    h.cli(&["set", "-g", "agent-cost", "off"]).await;
    h.wait_for_cli(
        "no cost",
        &["display-message", "-p", "-t", "ag", "[#{agent_cost}]#{agent} #{agent_context}"],
        |_, out| out.trim() == "[]claude 40%",
    )
    .await;

    // The agent gone: no agent.
    let shell: Vec<String> = PROMPT_SHELL.split(' ').map(String::from).collect();
    let (code, _, err) = h.cli(&args(&["respawn-pane", "-k", "-t", "ag"], &shell)).await;
    assert_eq!(code, 0, "{err}");
    h.wait_for_cli("no agent", &["list-agents", "-J"], |_, out| out.trim() == "[]").await;
    unsafe {
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::remove_var("KEEPANE_PRICE_LIST");
    }
    let _ = std::fs::remove_dir_all(&root);
}
