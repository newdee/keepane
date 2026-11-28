//! The keepane client: sends one command to the server and, if that attaches,
//! forwards console input and writes server output until detached.

use crate::console::{Console, InputEvent};
use crate::ipc::{ClientMsg, PROTOCOL_VERSION, ServerMsg, pipe_name, read_frame, write_frame};
use crate::platform::ipc::{self as pipe_ipc, Stream};
use crate::platform::process::{leave_job_denied, spawn_self};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

async fn connect(pipe: &str, autostart: bool, socket: &str) -> Result<Stream> {
    let mut started = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match pipe_ipc::connect(pipe) {
            Ok(c) => return Ok(c),
            Err(e) if pipe_ipc::is_busy(&e) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) if pipe_ipc::is_absent(&e) => {
                if !autostart {
                    bail!("no server running on {pipe}");
                }
                if !started {
                    start_server(socket)?;
                    started = true;
                }
                if Instant::now() > deadline {
                    bail!("server did not start (see {})", server_log().display());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e).with_context(|| format!("connect to the server at {pipe}")),
        }
    }
}

/// Start the server as a detached process that inherits *no* handles.
///
/// `std::process::Command` always passes `bInheritHandles = TRUE`, so a
/// server started from a client whose stdout is a pipe (e.g. `$x = keepane new
/// -d` in a script, or `Command::output()`) would hold that pipe open for
/// its whole life and the caller would never see EOF. Call CreateProcessW
/// directly instead.
fn start_server(socket: &str) -> Result<()> {
    spawn_server(socket)
}

/// The server process, out of the caller's job when it may leave: OpenSSH
/// runs each session in a kill-on-close job, and a server left in it would
/// end with the connection. A job that does not let it go (CreateProcess
/// says access denied) keeps it, as before, rather than keep it from
/// starting.
fn spawn_server(socket: &str) -> Result<()> {
    let args = ["-L", socket, "__server"];
    match spawn_self(&args, true) {
        Err(e) if leave_job_denied(&e) => spawn_self(&args, false),
        r => r,
    }
}

/// Commands that must not start a server just to answer.
fn needs_server(argv: &[String]) -> bool {
    matches!(
        argv.first().map(String::as_str),
        Some(
            "new-session"
                | "new"
                | "attach-session"
                | "attach"
                | "a"
                | "at"
                | "source-file"
                | "source"
                | "bind-key"
                | "bind"
                | "set-option"
                | "set"
                | "restore-session"
                | "restore"
                | "resume"
                | "list-saved"
                | "saved"
                | "delete-saved"
                | "forget"
        )
    )
}

/// The pane this program runs in, when it runs in one of `socket`'s panes:
/// `KEEPANE_PANE`, if `KEEPANE` (the socket that pane belongs to, `default`
/// when empty) is `socket`. A pane of another server is not one of this
/// server's, whatever its number.
pub fn own_pane(socket: &str) -> Option<u32> {
    pane_of(socket, std::env::var("KEEPANE").ok(), std::env::var("KEEPANE_PANE").ok())
}

/// Whether `argv` makes a session and attaches to it (`new` without `-d`).
fn attaches_new(argv: &[String]) -> bool {
    matches!(crate::command::parse(argv), Ok(crate::command::Cmd::NewSession { detached: false, .. }))
}

fn pane_of(socket: &str, keepane: Option<String>, pane: Option<String>) -> Option<u32> {
    let home = keepane.filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into());
    (home == socket).then_some(()).and(pane).and_then(|p| p.trim().trim_start_matches('%').parse().ok())
}

/// What a server says when it goes away to come back (`kill-server -r`,
/// sent by `restart-server`): an attached client waits and attaches again.
pub const RESTARTING: &str = "server restarting";

/// Whether a server is listening on `pipe` (without starting one).
pub fn server_running(pipe: &str) -> bool {
    pipe_ipc::server_running(pipe)
}

/// Run one command against the running server, the way a script would:
/// its exit code, what it printed, what it complained about. Never starts
/// a server.
pub async fn query(socket: &str, argv: &[&str]) -> Result<(i32, String, String)> {
    query_pipe(&pipe_name(socket), socket, argv, None).await
}

/// As `query`, as a program in pane `pane` asks (its `KEEPANE_PANE`), so
/// the server knows who is sending and whose inbox is meant.
pub async fn query_as(socket: &str, argv: &[&str], pane: Option<u32>) -> Result<(i32, String, String)> {
    query_pipe(&pipe_name(socket), socket, argv, pane).await
}

/// As `query`, to the server listening on `pipe`; `socket` names it in
/// errors.
async fn query_pipe(pipe: &str, socket: &str, argv: &[&str], pane_env: Option<u32>) -> Result<(i32, String, String)> {
    let conn = connect(pipe, false, socket).await?;
    let (mut rd, mut wr) = tokio::io::split(conn);
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    write_frame(
        &mut wr,
        &ClientMsg::Command {
            version: PROTOCOL_VERSION,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd,
            cols: 80,
            rows: 24,
            interactive: false,
            pane_env,
        },
    )
    .await?;
    let (mut out, mut err) = (String::new(), String::new());
    loop {
        match read_frame::<_, ServerMsg>(&mut rd).await? {
            None => bail!("server closed the connection"),
            Some(ServerMsg::Text(t)) => {
                out.push_str(&t);
                out.push('\n');
            }
            Some(ServerMsg::Error(e)) => {
                err.push_str(&e);
                err.push('\n');
            }
            Some(ServerMsg::Done { code }) => return Ok((code, out, err)),
            Some(_) => {}
        }
    }
}

/// The version of the server on `socket` ("0.9.0"), None when none runs.
pub async fn server_version(socket: &str) -> Option<String> {
    if !server_running(&pipe_name(socket)) {
        return None;
    }
    let (code, out, _) = query(socket, &["version"]).await.ok()?;
    (code == 0).then(|| out.trim().trim_start_matches("keepane").trim().to_string())
}

/// The line that says the server is a different keepane from this one.
pub fn mismatch_note(server: &str) -> String {
    format!(
        "the running server is keepane {server}, this is keepane {}: `keepane restart-server` moves your sessions to it",
        env!("CARGO_PKG_VERSION")
    )
}

/// `keepane version`: this program's version, and the server's when one runs
/// and it differs.
pub async fn version(socket: &str) -> Result<i32> {
    println!("keepane {}", env!("CARGO_PKG_VERSION"));
    if let Some(v) = server_version(socket).await {
        if v == env!("CARGO_PKG_VERSION") {
            println!("server: the same");
        } else {
            println!("server: keepane {v}");
            eprintln!("note: {}", mismatch_note(&v));
        }
    }
    Ok(0)
}

/// `keepane restart-server`: move every running session to a new server of
/// this version. The sessions are saved, the old server is told to go
/// (clients of 0.10 and later attach again by themselves), a new one is
/// started, and exactly the sessions that were running are restored in it
/// with their layout, history and directories.
pub async fn restart_server(socket: &str) -> Result<i32> {
    let pipe = pipe_name(socket);
    let Some(old) = server_version(socket).await else {
        println!("no server running on socket {socket}: nothing to restart");
        return Ok(0);
    };
    // Inside one of this server's panes, stopping the server stops us
    // too, halfway. Go on as a copy of ourselves outside the pane's job;
    // it writes what it did to restart.log, and this terminal attaches to
    // the new server like every other.
    let detached = std::env::var_os("KEEPANE_RESTART_DETACHED").is_some();
    let in_pane = own_pane(socket).is_some();
    if in_pane && !detached {
        unsafe { std::env::set_var("KEEPANE_RESTART_DETACHED", "1") };
        return match spawn_self(&["-L", socket, "restart-server"], true) {
            Ok(()) => {
                println!(
                    "restarting from outside this pane (it goes with the old server); \
                     the result is in {}",
                    restart_log().display()
                );
                Ok(0)
            }
            Err(e) => bail!(
                "this pane cannot outlive its server (keepane {old} does not let it): \
                 run `keepane restart-server` from a terminal outside keepane ({e:#})"
            ),
        };
    }
    let (code, summary) = restart_server_here(socket, &pipe, &old).await?;
    if detached {
        let _ = std::fs::write(restart_log(), format!("{}\n{summary}\n", chrono::Local::now().to_rfc3339()));
    }
    println!("{summary}");
    Ok(code)
}

/// Where a restart run outside a pane leaves its result.
/// (Beside the sessions when `KEEPANE_SESSIONS_DIR` moves them: tests do.)
/// The server's log, where a server that did not come up says why.
fn server_log() -> std::path::PathBuf {
    crate::logger::log_dir().join("server.log")
}

fn restart_log() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("KEEPANE_SESSIONS_DIR").filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(d).join("restart.log");
    }
    crate::logger::log_dir().join("restart.log")
}

async fn restart_server_here(socket: &str, pipe: &str, old: &str) -> Result<(i32, String)> {
    let (_, list, _) = query(socket, &["list-sessions"]).await?;
    let sessions: Vec<String> =
        list.lines().filter_map(|l| l.split_once(':').map(|(n, _)| n.to_string())).filter(|n| !n.is_empty()).collect();
    let (code, _, err) = query(socket, &["save-session", "-a"]).await?;
    if code != 0 && !sessions.is_empty() {
        bail!("saving the sessions failed, the server is left running: {}", err.trim());
    }
    // `-r` tells attached clients to wait and attach again; a server older
    // than that flag only knows plain kill-server, and its clients detach.
    // Any refusal of -r means an older server (0.9 says "unexpected
    // argument"); the sessions are saved, so a plain kill-server is safe.
    let (code, _, _) = query(socket, &["kill-server", "-r"]).await?;
    let told_clients = code == 0;
    if !told_clients {
        let (code, _, err) = query(socket, &["kill-server"]).await?;
        if code != 0 {
            bail!("the server would not stop: {}", err.trim());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while server_running(pipe) {
        if Instant::now() > deadline {
            bail!("the old server did not exit");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    start_server(socket)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !server_running(pipe) {
        if Instant::now() > deadline {
            bail!("the new server did not start (see {})", server_log().display());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut restored = Vec::new();
    let mut lines = Vec::new();
    for s in &sessions {
        let (code, _, err) = query(socket, &["restore-session", s]).await?;
        if code == 0 {
            restored.push(s.clone());
        } else {
            lines.push(format!("{s}: not restored: {}", err.trim()));
        }
    }
    lines.insert(
        0,
        format!(
            "server keepane {old} -> keepane {}; {} of {} session(s) restored{}",
            env!("CARGO_PKG_VERSION"),
            restored.len(),
            sessions.len(),
            if restored.is_empty() { String::new() } else { format!(": {}", restored.join(", ")) }
        ),
    );
    if !told_clients && !sessions.is_empty() {
        lines.push("terminals that were attached were detached: `keepane attach` to go back".into());
    }
    // The phones' page, served again as it was (when it was on), or why
    // not yet.
    if let Ok((0, said, _)) = query(socket, &["web-resume"]).await {
        let first = said.lines().next().unwrap_or("");
        if let Some(url) = crate::ipc::status_url(&said) {
            lines.push(format!("the phone page is served again: {url}"));
        } else if first.starts_with("off: to be served again") {
            lines.push(format!("the phone page is {}", &first["off: ".len()..]));
        }
    }
    // The old server cannot tell its defaults from what was set, so its
    // options are not copied (that would pin the old defaults on the new
    // version): the new server reads the config file, as after kill-server.
    lines.push(
        "options and key bindings come from your config file; any set with `set`/`bind` since are not carried over"
            .into(),
    );
    Ok((if restored.len() == sessions.len() { 0 } else { 1 }, lines.join("\n")))
}

pub async fn run(socket: String, argv: Vec<String>) -> Result<i32> {
    let pipe = pipe_name(&socket);
    let mut console = Console::open().ok();
    let (cols, rows) = console.as_ref().map(|c| c.size()).unwrap_or((80, 24));
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let pane_env = own_pane(&socket);
    // Inside a pane of another server: its number means nothing to this one
    // and is not sent, but a session made here and attached to at once would
    // still be nested, which tmux refuses whatever server `TMUX` names. (For
    // this server's own panes the server says so.)
    if pane_env.is_none() && std::env::var_os("KEEPANE_PANE").is_some() && console.is_some() && attaches_new(&argv) {
        eprintln!("{}", crate::command::NESTED);
        return Ok(1);
    }
    // A terminal about to attach to a server of another version is told
    // so (in its title while attached, and when it detaches): the usual
    // cause is an upgrade with the old server still running.
    let mut mismatch = match &console {
        Some(_) => server_version(&socket).await.filter(|v| v != env!("CARGO_PKG_VERSION")),
        None => None,
    };

    let conn = connect(&pipe, needs_server(&argv), &socket).await?;
    let (mut rd, mut wr) = tokio::io::split(conn);
    write_frame(
        &mut wr,
        &ClientMsg::Command {
            version: PROTOCOL_VERSION,
            argv,
            cwd,
            cols,
            rows,
            interactive: console.is_some(),
            pane_env,
        },
    )
    .await?;

    // Non-interactive phase.
    loop {
        let Some(msg) = read_frame::<_, ServerMsg>(&mut rd).await? else {
            bail!("server closed the connection");
        };
        match msg {
            ServerMsg::Text(t) => println!("{t}"),
            ServerMsg::Error(e) => eprintln!("{e}"),
            ServerMsg::Done { code } => return Ok(code),
            ServerMsg::Attached { session } => {
                let Some(mut c) = console.take() else { bail!("attached without a console") };
                c.enter_raw()?;
                let c = Arc::new(c);
                // One reader of the console for the life of the process: an
                // attach after a server restart takes it over.
                let mut input = spawn_input(Arc::clone(&c))?;
                let (mut rd, mut wr, mut session) = (rd, wr, session);
                // `rd` moves into the reader task and never comes back: this
                // arm always returns.
                let reason = loop {
                    let title = match &mismatch {
                        Some(v) => format!("keepane: {session} [server {v}: run keepane restart-server]"),
                        None => format!("keepane: {session}"),
                    };
                    c.set_title(&title);
                    // What the terminal shows, told only to a server of this
                    // version: an older one does not know the message.
                    let kitty = mismatch.is_none().then(|| outer_has_kitty(|k| std::env::var(k).ok()));
                    match attached(Arc::clone(&c), &session, rd, &mut wr, &mut input, kitty).await {
                        Ok(Detach::Restarting) => match reattach(&socket, &session, &c).await {
                            Ok((r, w, s)) => {
                                (rd, wr, session) = (r, w, s);
                                mismatch = server_version(&socket).await.filter(|v| v != env!("CARGO_PKG_VERSION"));
                            }
                            Err(e) => break Err(e),
                        },
                        Ok(Detach::Reason(r)) => break Ok(r),
                        Err(e) => break Err(e),
                    }
                };
                // The input thread still holds a reference; restore explicitly.
                c.restore();
                if let Some(v) = &mismatch {
                    eprintln!("note: {}", mismatch_note(v));
                }
                match reason {
                    Ok(r) => {
                        println!("[{r}]");
                        return Ok(0);
                    }
                    Err(e) => {
                        eprintln!("[{e:#}]");
                        return Ok(1);
                    }
                }
            }
            ServerMsg::Output(_) | ServerMsg::Detached { .. } | ServerMsg::SetMouse(_) | ServerMsg::Raise => {}
        }
    }
}

/// Read frames on their own task and hand them over one at a time.
///
/// `read_frame` is two reads, the header and then the body, so awaiting it
/// directly in a `select!` loses the header whenever another branch wins the
/// race: the next read then takes four bytes out of the middle of a frame and
/// calls them a length. (That is where "frame too large: 538976288" came
/// from: 0x20202020, four spaces of a full-screen redraw.) A channel receive
/// is cancel-safe, so the reading happens where nothing can cancel it.
fn spawn_frame_reader<R>(mut rd: R) -> mpsc::Receiver<Result<Option<ServerMsg>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    // Bounded: a client that cannot keep up should slow the pipe down rather
    // than grow without limit.
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        loop {
            let msg = read_frame::<_, ServerMsg>(&mut rd).await;
            let done = !matches!(msg, Ok(Some(_)));
            if tx.send(msg).await.is_err() || done {
                return;
            }
        }
    });
    rx
}

/// How an attach ended.
enum Detach {
    /// Detached, the server gone, the session killed: the reason, shown.
    Reason(String),
    /// The server is restarting (`restart-server`): attach again to the new one.
    Restarting,
}

/// Read the console on a thread of its own for the rest of the process.
/// A re-attach after a server restart reuses the receiver: a second reader
/// would take keys meant for the first.
fn spawn_input(console: Arc<Console>) -> Result<mpsc::UnboundedReceiver<ClientMsg>> {
    let (tx, rx) = mpsc::unbounded_channel::<ClientMsg>();
    std::thread::Builder::new()
        .name("console-input".into())
        .spawn(move || {
            let mut size = console.size();
            loop {
                let events = match console.read_events() {
                    Ok(e) => e,
                    Err(_) => break,
                };
                for ev in events {
                    let m = match ev {
                        InputEvent::Key(k) => ClientMsg::Key(k),
                        InputEvent::Mouse(m) => ClientMsg::Mouse(m),
                        InputEvent::Resize => {
                            let s = console.size();
                            if s == size {
                                continue;
                            }
                            size = s;
                            ClientMsg::Resize { cols: s.0, rows: s.1 }
                        }
                    };
                    if tx.send(m).is_err() {
                        return;
                    }
                }
            }
        })
        .context("spawn input thread")?;
    Ok(rx)
}

type Halves = (tokio::io::ReadHalf<Stream>, tokio::io::WriteHalf<Stream>, String);

/// The server went away to come back: wait for the new one (up to half a
/// minute) and attach to the same session again.
async fn reattach(socket: &str, session: &str, console: &Console) -> Result<Halves> {
    console.write_bytes(format!("\x1b[H\x1b[2J[{RESTARTING}: attaching to {session} again...]").as_bytes());
    let pipe = pipe_name(socket);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > deadline {
            bail!("{RESTARTING}, and no new server came up");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let Ok(conn) = connect(&pipe, false, socket).await else { continue };
        let (mut rd, mut wr) = tokio::io::split(conn);
        let (cols, rows) = console.size();
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        write_frame(
            &mut wr,
            &ClientMsg::Command {
                version: PROTOCOL_VERSION,
                argv: vec!["attach-session".into(), "-t".into(), session.to_string()],
                cwd,
                cols,
                rows,
                interactive: true,
                pane_env: None,
            },
        )
        .await?;
        loop {
            match read_frame::<_, ServerMsg>(&mut rd).await? {
                Some(ServerMsg::Attached { session }) => return Ok((rd, wr, session)),
                // Up, but the session is not back yet: ask again shortly.
                Some(ServerMsg::Error(_)) | Some(ServerMsg::Done { .. }) | None => break,
                Some(_) => {}
            }
        }
    }
}

async fn attached<R, W>(
    console: Arc<Console>,
    session: &str,
    rd: R,
    wr: &mut W,
    rx: &mut mpsc::UnboundedReceiver<ClientMsg>,
    kitty: Option<bool>,
) -> Result<Detach>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin,
{
    // The server sized the session from our Command; re-check in case the
    // window changed while connecting.
    let s = console.size();
    write_frame(wr, &ClientMsg::Resize { cols: s.0, rows: s.1 }).await?;
    if let Some(kitty) = kitty {
        write_frame(wr, &ClientMsg::Terminal { kitty }).await?;
    }

    let reason: Option<String>;
    // Poll the window size too: not every host reports WINDOW_BUFFER_SIZE_EVENT.
    let mut poll = tokio::time::interval(Duration::from_millis(500));
    let mut last_size = s;
    let mut frames = spawn_frame_reader(rd);
    loop {
        tokio::select! {
            m = frames.recv() => {
                match m {
                    None | Some(Ok(None)) => { reason = Some("server exited".into()); break; }
                    Some(Err(e)) => return Err(e),
                    Some(Ok(Some(ServerMsg::Output(b)))) => console.write_bytes(&b),
                    Some(Ok(Some(ServerMsg::Detached { reason: r }))) => {
                        if r == RESTARTING {
                            return Ok(Detach::Restarting);
                        }
                        reason = Some(format!("{r} (from session {session})"));
                        break;
                    }
                    Some(Ok(Some(ServerMsg::Error(e)))) => { reason = Some(format!("error: {e}")); break; }
                    Some(Ok(Some(ServerMsg::SetMouse(on)))) => console.set_mouse(on),
                    Some(Ok(Some(ServerMsg::Raise))) => { crate::notify::raise_console_window(); }
                    Some(Ok(Some(ServerMsg::Text(_) | ServerMsg::Done { .. } | ServerMsg::Attached { .. }))) => {}
                }
            }
            Some(m) = rx.recv() => {
                write_frame(wr, &m).await?;
            }
            _ = poll.tick() => {
                let s = console.size();
                if s != last_size {
                    last_size = s;
                    write_frame(wr, &ClientMsg::Resize { cols: s.0, rows: s.1 }).await?;
                }
            }
        }
    }
    // The input thread is blocked in ReadConsoleInput; it dies with the process.
    Ok(Detach::Reason(reason.unwrap_or_else(|| "detached".into())))
}

/// Whether the terminal a client runs in shows kitty's pictures, from the
/// names it sets (as `ut view` decides): UnifyTerm, kitty, WezTerm,
/// ghostty, or a keepane pane that has them on. The client tells the
/// server (`ClientMsg::Terminal`).
pub fn outer_has_kitty(get: impl Fn(&str) -> Option<String>) -> bool {
    let program = get("TERM_PROGRAM").unwrap_or_default();
    let term = get("TERM").unwrap_or_default();
    matches!(program.to_ascii_lowercase().as_str(), "unifyterm" | "wezterm" | "ghostty")
        || get("KITTY_WINDOW_ID").is_some_and(|v| !v.is_empty())
        || matches!(term.as_str(), "xterm-kitty" | "xterm-ghostty")
        || (program == "keepane" && get("KEEPANE_GRAPHICS").as_deref() == Some("kitty"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_outside_is_known_by_its_names() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert!(outer_has_kitty(env(&[("TERM_PROGRAM", "unifyterm")])));
        assert!(outer_has_kitty(env(&[("TERM_PROGRAM", "WezTerm")])));
        assert!(outer_has_kitty(env(&[("KITTY_WINDOW_ID", "3")])));
        assert!(outer_has_kitty(env(&[("TERM", "xterm-ghostty")])));
        assert!(outer_has_kitty(env(&[("TERM_PROGRAM", "keepane"), ("KEEPANE_GRAPHICS", "kitty")])));
        assert!(!outer_has_kitty(env(&[("TERM_PROGRAM", "keepane")])), "a keepane pane with them off");
        assert!(!outer_has_kitty(env(&[("WT_SESSION", "x")])), "Windows Terminal has none");
        assert!(!outer_has_kitty(env(&[])));
    }

    #[test]
    fn a_pane_counts_only_for_its_own_server() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(pane_of("default", s("default"), s("4")), Some(4));
        assert_eq!(pane_of("default", s(""), s("4")), Some(4), "an empty KEEPANE is the default socket");
        assert_eq!(pane_of("default", None, s("4")), Some(4));
        assert_eq!(pane_of("work", s("default"), s("4")), None, "a pane of another server");
        assert_eq!(pane_of("work", s("work"), s("%7")), Some(7));
        assert_eq!(pane_of("default", s("default"), None), None, "not in a pane");
        assert_eq!(pane_of("default", s("default"), s("x")), None);
    }
    use tokio::io::AsyncWriteExt;

    /// Frames must survive a `select!` loop that keeps waking up for other
    /// reasons. Reading them inline used to drop a half-read frame and read
    /// the next length out of the middle of a payload.
    #[tokio::test(flavor = "current_thread")]
    async fn frames_survive_a_select_loop() {
        let (mut server, client) = tokio::io::duplex(64 * 1024);
        // Send each frame in two writes with a gap between them, which is what
        // a big screen redraw looks like on a pipe.
        let sent: Vec<String> = (0..20).map(|i| format!("frame {i} {}", "x".repeat(2000))).collect();
        let expect = sent.clone();
        tokio::spawn(async move {
            for text in sent {
                let mut buf = Vec::new();
                let payload = bincode::serialize(&ServerMsg::Text(text)).unwrap();
                buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                // header first, then the body a moment later
                server.write_all(&buf).await.unwrap();
                server.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(2)).await;
                server.write_all(&payload).await.unwrap();
                server.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });

        let mut frames = spawn_frame_reader(client);
        // The branch that used to cancel the read, firing constantly.
        let mut ticker = tokio::time::interval(Duration::from_millis(1));
        let mut got: Vec<String> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while got.len() < expect.len() && tokio::time::Instant::now() < deadline {
            tokio::select! {
                m = frames.recv() => match m {
                    Some(Ok(Some(ServerMsg::Text(t)))) => got.push(t),
                    Some(Ok(Some(other))) => panic!("unexpected message {other:?}"),
                    Some(Ok(None)) | None => break,
                    Some(Err(e)) => panic!("{e:#}"),
                },
                _ = ticker.tick() => {}
            }
        }
        assert_eq!(got, expect, "every frame arrives, in order and intact");
    }
}
