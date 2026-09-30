//! A pane: a ConPTY-backed process plus an in-memory terminal (vt100).

use super::layout::PaneId;
use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;

/// Events a pane reports back to the server loop. The generation tells a
/// respawned pane's output and exit apart from its predecessor's, whose
/// threads may still be finishing.
pub enum PaneEvent {
    Output(PaneId, u32, Vec<u8>),
    Exit(PaneId, u32, u32),
    /// What a `pipe-pane -I` command printed: input for the pane.
    Input(PaneId, Vec<u8>),
}

/// What the printer of a resumed pane's saved output (`keepane __replay`)
/// writes after the text: a private OSC that ConPTY passes through and the
/// screen model turns into `replay_done`, the cue to start the pane's own
/// program in that console while the printer still holds it open.
pub const REPLAY_MARKER: &str = "\x1b]7777;keepane-replayed\x1b\\";

/// The generation tag of the printer's exit event, which is never the
/// pane's own: its exit is not the pane's program exiting.
pub const HELPER_GEN: u32 = u32::MAX;

/// vt100 callbacks: collects terminal replies ConPTY expects from a real
/// terminal, the window title and bells.
#[derive(Default)]
pub struct Callbacks {
    pub responses: Vec<u8>,
    pub title: Option<String>,
    pub bell: bool,
    /// Working directory the shell announced (OSC 7 / OSC 9;9), as a Windows path.
    pub cwd: Option<String>,
    /// The `REPLAY_MARKER` arrived: a resumed pane's saved output is all in.
    pub replayed: bool,
    /// What the shell said about its prompts and commands, in order.
    pub marks: Vec<MarkEvent>,
    /// keepane's own prompt hook said the shell is at its prompt (`OSC
    /// 7777;keepane-prompt`): the one signal a `shell` pane trusts. With the
    /// language that shell takes commands in.
    pub prompted: Option<super::actor::Syntax>,
}

/// A shell's word about its prompt and its commands: FTCS (OSC 133, and
/// VS Code's OSC 633, which reads the same) from bash, zsh, fish and
/// profiles that print it, stamped on arrival; and the PowerShell prompt
/// hook's own report, whose times come from the shell's history.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkEvent {
    /// A prompt is on the line `line` (`scrolled_total() + row`), what is
    /// typed at it starting in column `col`.
    Prompt(u64, u16),
    /// The command typed at it started.
    Start(chrono::DateTime<chrono::Local>),
    /// It finished, with this exit code when the shell gave one.
    End(chrono::DateTime<chrono::Local>, Option<i32>),
    /// PowerShell: the last command ran from `start` to `end`, and did or
    /// did not succeed.
    Ran { start: chrono::DateTime<chrono::Local>, end: chrono::DateTime<chrono::Local>, ok: bool },
}

/// A command that ended: what was typed (the prompt left out), how long it
/// ran, whether it failed, and its exit code when the shell gave the code
/// itself (PowerShell says only whether it failed).
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub command: String,
    pub secs: u64,
    pub failed: bool,
    pub exit: Option<i32>,
}

/// What was typed at a prompt: the line from column `col` on, a cell a
/// column (a wide character takes two), blanks trimmed; nothing when the
/// line ends before that column (the prompt and nothing after it).
pub fn command_of(line: &str, col: u16) -> String {
    let mut width = 0;
    for (i, c) in line.char_indices() {
        if width >= usize::from(col) {
            return line[i..].trim().to_string();
        }
        width += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    }
    String::new()
}

/// One command a shell ran, pinned to the line it was typed on.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// The prompt's line, as `scrolled_total() + row`: it keeps naming the
    /// same line however far it scrolls.
    pub line: u64,
    /// The column its command starts in: where the prompt ends.
    pub col: u16,
    /// When Enter first went into the pane at this prompt: the command's
    /// start for a shell that does not say it (bash before 4.4, macOS's
    /// own, has no `PS0`). For `done-events` only; the stamps want the
    /// shell's word.
    pub entered: Option<chrono::DateTime<chrono::Local>>,
    pub start: Option<chrono::DateTime<chrono::Local>>,
    pub end: Option<chrono::DateTime<chrono::Local>>,
    /// The exit code, when the shell said; PowerShell says only whether it
    /// failed (1) or not (0).
    pub exit: Option<i32>,
    /// What the line read when the command finished. A line that no longer
    /// reads so was cleared or written over, and the mark is no longer shown.
    pub text: String,
}

/// Whether a line reading `now` is still the line a mark saw as `then`:
/// the same text or, in a pane narrowed since, the part of it that fits in
/// `cols` columns (both without trailing blanks).
pub fn still_reads(now: &str, then: &str, cols: u16) -> bool {
    if now == then {
        return true;
    }
    let mut width = 0;
    let end = then
        .char_indices()
        .find(|(_, c)| {
            width += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
            width > usize::from(cols)
        })
        .map_or(then.len(), |(i, _)| i);
    end < then.len() && !now.is_empty() && now == then[..end].trim_end()
}

/// Log text a pane gathers before handing it to the writer.
const LOG_BATCH: usize = 32 * 1024;

/// Most lines of a running command's output held back for its times, and
/// for how long, before they go to the history log without them.
const HOLD_LINES: usize = 5000;
const HOLD_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// The line the history log puts before a command: when it started (with
/// the date when not today), how long it took, how it ended.
fn log_header(m: &Mark, end: chrono::DateTime<chrono::Local>, now: chrono::DateTime<chrono::Local>) -> String {
    let at = m.start.unwrap_or(end);
    let fmt = if at.date_naive() == now.date_naive() { "%H:%M:%S" } else { "%Y-%m-%d %H:%M:%S" };
    let mut h = format!("── {}", at.format(fmt));
    if let Some(start) = m.start {
        h.push_str(&format!(" · {}", crate::format::command_duration((end - start).num_milliseconds())));
    }
    match m.exit {
        Some(0) => h.push_str(" · ✓"),
        Some(code) => h.push_str(&format!(" · ✗ {code}")),
        None => {}
    }
    h.push_str(" ──\n");
    h
}

/// Most marks a pane keeps; older ones have long scrolled out of reach.
const MAX_MARKS: usize = 2000;

fn local_ms(ms: &[u8]) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    let ms: i64 = std::str::from_utf8(ms).ok()?.parse().ok()?;
    chrono::Local.timestamp_millis_opt(ms).single()
}

/// Turn a shell-announced directory (OSC 7 `file://host/path`, OSC 9;9
/// `path`) into a path usable as a process working directory here, or None.
pub fn path_from_announced(raw: &str) -> Option<String> {
    // Windows Terminal's own PowerShell snippet quotes the path
    // (`ESC]9;9;"C:\x"ESC\`); the quotes are not part of it.
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    if let Some(rest) = s.strip_prefix("file://") {
        // file://host/C:/x or file:///C:/x
        let path = &rest[rest.find('/')?..];
        s = percent_decode(path);
    }
    native_path(s)
}

/// An absolute path that is a directory on this machine (a shell on another
/// one, over SSH, announces its own).
#[cfg(unix)]
fn native_path(s: String) -> Option<String> {
    (s.starts_with('/') && std::path::Path::new(&s).is_dir()).then_some(s)
}

/// `file:///C:/x`, `file://host/C:/x`, `C:\x`, `/mnt/c/x` (WSL) all become
/// `C:\x`; other Linux paths are not usable and yield None.
#[cfg(windows)]
fn native_path(mut s: String) -> Option<String> {
    // /C:/x  or  /c/x (some shells) -> C:/x
    if s.len() >= 3 && s.as_bytes()[0] == b'/' && s.as_bytes()[2] == b':' {
        s = s[1..].to_string();
    }
    if let Some(rest) = s.strip_prefix("/mnt/")
        && !rest.is_empty()
        && rest.as_bytes()[0].is_ascii_alphabetic()
        && (rest.len() == 1 || rest.as_bytes()[1] == b'/')
    {
        let drive = rest.as_bytes()[0].to_ascii_uppercase() as char;
        s = format!("{drive}:{}", &rest[1..]);
    }
    if s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() {
        let mut p = s.replace('/', "\\");
        if p.len() == 2 {
            p.push('\\');
        }
        return Some(p);
    }
    None
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl vt100::Callbacks for Callbacks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn visual_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }
    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        if self.title.is_none() {
            self.title = Some(String::from_utf8_lossy(title).into_owned());
        }
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        match (i1, c) {
            // DSR: cursor position report. ConPTY (with INHERIT_CURSOR) asks
            // for this at startup and waits for the answer.
            (None, 'n') if params.first().is_some_and(|p| p.first() == Some(&6)) => {
                let (row, col) = screen.cursor_position();
                self.responses.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            (None, 'n') if params.first().is_some_and(|p| p.first() == Some(&5)) => {
                self.responses.extend_from_slice(b"\x1b[0n");
            }
            // Primary device attributes: claim a VT220-class terminal.
            (None, 'c') => self.responses.extend_from_slice(b"\x1b[?62;22c"),
            (Some(b'>'), 'c') => self.responses.extend_from_slice(b"\x1b[>0;10;1c"),
            _ => {}
        }
    }
    fn unhandled_osc(&mut self, screen: &mut vt100::Screen, params: &[&[u8]]) {
        // A full-screen program's screen is not the shell's: nothing there
        // is a prompt.
        let shell = !screen.alternate_screen();
        let now = chrono::Local::now;
        // OSC 7 ; file://host/path   (bash/zsh/fish shell integration)
        // OSC 9 ; 9 ; path           (ConEmu / Windows Terminal "current directory")
        let raw = match params {
            [b"7", p] => Some(*p),
            [b"9", b"9", p] => Some(*p),
            [b"7777", b"keepane-replayed"] => {
                self.replayed = true;
                None
            }
            // OSC 7777 ; keepane-cmd ; start ; end ; ok   (the PowerShell hook,
            // times in Unix milliseconds)
            // OSC 7777 ; keepane-prompt [; sh]   (keepane's hook, every prompt:
            // PowerShell's, or bash's and zsh's; a remote shell's OSC 133 is
            // not keepane's word)
            [b"7777", b"keepane-prompt"] if shell => {
                self.prompted = Some(super::actor::Syntax::PowerShell);
                None
            }
            [b"7777", b"keepane-prompt", b"sh"] if shell => {
                self.prompted = Some(super::actor::Syntax::Posix);
                None
            }
            [b"7777", b"keepane-cmd", start, end, ok] if shell => {
                if let (Some(start), Some(end)) = (local_ms(start), local_ms(end)) {
                    self.marks.push(MarkEvent::Ran { start, end, ok: *ok != b"0" });
                }
                None
            }
            // OSC 133 ; A|B|C|D[;code]
            [b"133" | b"633", kind, rest @ ..] if shell => {
                match *kind {
                    b"A" | b"B" => {
                        let (row, col) = screen.cursor_position();
                        self.marks.push(MarkEvent::Prompt(screen.scrolled_total() + u64::from(row), col));
                    }
                    b"C" => self.marks.push(MarkEvent::Start(now())),
                    b"D" => {
                        let code = rest.first().and_then(|c| std::str::from_utf8(c).ok()?.trim().parse().ok());
                        self.marks.push(MarkEvent::End(now(), code));
                    }
                    _ => {}
                }
                None
            }
            _ => None,
        };
        if let Some(raw) = raw
            && let Some(p) = path_from_announced(&String::from_utf8_lossy(raw))
        {
            self.cwd = Some(p);
        }
    }
}

pub struct CopyMode {
    /// Lines of scrollback shown above the top of the screen.
    pub offset: usize,
    pub cx: u16,
    pub cy: u16,
    /// Selection anchor as (absolute line, column).
    pub anchor: Option<(usize, u16)>,
    /// A mouse drag selection is in progress.
    pub dragging: bool,
    /// Last `/` or `?` pattern, repeated by `n` and `N`.
    pub search: Option<String>,
    /// Direction of that search: `/` is towards older lines.
    pub search_back: bool,
    /// Digits typed before a motion (`3j`), vi style.
    pub count: Option<usize>,
    /// `C-v`: the selection is a rectangle, not a run of lines.
    pub rect: bool,
}

pub struct Pane {
    pub id: PaneId,
    /// Bumped by `respawn`; events from an older generation are ignored.
    pub generation: u32,
    pub parser: vt100::Parser<Callbacks>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Kill-on-close job holding the shell and everything it started, so a
    /// dead pane never leaves orphans (tmux's SIGHUP-the-process-group).
    job: Option<crate::platform::process::Tree>,
    pub title: String,
    /// The last title the program itself sent. Sending the same one again
    /// (prompts that set the title on every prompt do) is not a new title
    /// and does not undo a `select-pane -T`; a different one does, as in tmux.
    program_title: Option<String>,
    pub command: String,
    /// The command line this pane was started with (for save/restore).
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// `cwd` came from the shell itself (OSC 7 / 9;9), not from where the
    /// pane was started; from then on the shell's word is final.
    pub announced: bool,
    /// A resumed pane's saved output has all been printed (the printer's
    /// marker came through); the pane's program can start.
    pub replay_done: bool,
    pub exit_code: Option<u32>,
    /// `clock-mode`: a clock is drawn over this pane until a key arrives.
    pub clock: bool,
    pub copy: Option<CopyMode>,
    pub bell: bool,
    pub cols: u16,
    pub rows: u16,
    /// `pipe-pane`: everything this pane writes is copied here as well.
    pub pipe: Option<Pipe>,
    /// `record`: an asciinema file being written from this pane's output.
    pub recorder: Option<Recorder>,
    /// Process id of the pane's program (`#{pane_pid}`).
    pub pid: Option<u32>,
    /// When the current program was started (`jobs`, uptime).
    pub spawned_at: std::time::Instant,
    /// When it last printed anything (`jobs`, idle time).
    pub last_output: std::time::Instant,
    /// How many times it has printed (`#{pane_output_count}`): unlike a
    /// time, it changes with every output, so a watcher can tell whether
    /// the screen may have changed without reading it.
    pub output_count: u64,
    /// The commands its shell ran, oldest first (`pane-timestamps`,
    /// `list-marks`); empty for a shell that does not report them.
    pub marks: std::collections::VecDeque<Mark>,
    /// Commands that ended since the server last looked, for `done-events`.
    pub finished: Vec<Finished>,
    /// `log-history`: the file this pane writes to now (set by the server,
    /// which knows where the pane is), and so where what is left on its
    /// screen goes when it closes. None while logging is off.
    pub log_to: Option<std::path::PathBuf>,
    /// The first line (`scrolled_total() + row`) not yet taken for the log.
    pub logged: u64,
    /// Lines taken but not yet written: the command they follow is still
    /// running, and its times go before it once it is done.
    log_hold: std::collections::VecDeque<(u64, String)>,
    log_hold_since: Option<std::time::Instant>,
    /// Log text not yet handed to the writer: it goes in batches (a pane
    /// prints a line or two at a time), at `LOG_BATCH` bytes or each second.
    log_buf: String,
    /// When its program exited, while `remain-on-exit` keeps the pane
    /// (`#{pane_dead_time}`; `jobs` stops the clock there).
    pub died_at: Option<std::time::Instant>,
    /// Its name, work mode and inbox (docs/design/mailbox.md).
    pub actor: super::actor::Actor,
    /// keepane's prompt came through since the server last looked; the
    /// server takes it and tells the actor.
    pub prompted: bool,
    /// The language of the shell whose prompt came last: a shell message
    /// is typed in it.
    pub syntax: super::actor::Syntax,
    /// Where the message delivered last was typed, and the first line after
    /// it as typed (both `scrolled_total() + row`): what the shell printed
    /// from there to its next prompt is the command's output.
    pub delivered_line: Option<(u64, u64)>,
    /// What was typed, when on one line: its end is looked for on the
    /// screen when the command is over (`command_end`).
    pub delivered_text: Option<String>,
    /// keepane's prompt came and the server waits a moment before taking
    /// the command as done (`PROMPT_SETTLE`); nothing is typed in meanwhile.
    pub settle: bool,
    /// The file its PowerShell keeps command history in (`KEEPANE_SHELL_HISTORY`),
    /// by name under the sessions directory: its own, so Up in a resumed pane
    /// gives what that pane ran. None for a pane that is not an interactive
    /// PowerShell.
    pub shell_history: Option<String>,
    /// The pty's slave side, kept only while a program is still to be
    /// started in it (a resumed pane printing its saved output first).
    slave: Option<Box<dyn portable_pty::SlavePty + Send>>,
    /// That program, started by `start_pending` when the printer exits.
    pending: Option<PendingStart>,
    tx: Sender<PaneEvent>,
}

/// An asciinema v2 recording in progress: a header line, then one JSON
/// event per output chunk or resize, stamped with seconds since the start.
/// Writing goes through a thread like `Pipe`, so a slow disk never stalls
/// the server; dropping the recorder closes the file.
pub struct Recorder {
    pub path: String,
    started: std::time::Instant,
    tx: std::sync::mpsc::SyncSender<String>,
}

impl Recorder {
    fn event(&mut self, kind: &str, data: &str) {
        let t = self.started.elapsed().as_secs_f64();
        let line =
            format!("[{t:.6}, {}, {}]\n", serde_json::to_string(kind).unwrap(), serde_json::to_string(data).unwrap());
        // A recording that cannot keep up loses events rather than output.
        let _ = self.tx.try_send(line);
    }
}

/// A running `pipe-pane` command. The bytes go through a bounded channel to a
/// writer thread, so a command that stops reading can never wedge the server;
/// dropping the `Pipe` closes the command's standard input.
pub struct Pipe {
    pub command: String,
    tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    /// Output dropped because the command was not keeping up.
    pub dropped: u64,
    /// `-O`: the pane's output is sent to the command at all.
    pub output: bool,
}

/// The keepane.exe that runs `__replay` and `view`: this executable when it is keepane,
/// `KEEPANE_EXE` when set (the tests, whose own binary is not keepane), else the
/// keepane.exe two directories up from a test binary (cargo's layout).
pub fn helper_exe() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("KEEPANE_EXE").map(std::path::PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let me = std::env::current_exe().ok()?;
    if me.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("keepane")) {
        return Some(me);
    }
    let sibling = me.parent()?.parent()?.join("keepane.exe");
    sibling.is_file().then_some(sibling)
}

/// The command that prints `text` into the pane's console: `keepane __replay
/// file`, which writes the file's text to the console and removes it.
fn replay_argv(id: PaneId, text: &str) -> Result<(Vec<String>, std::path::PathBuf)> {
    let exe = helper_exe().context("no keepane.exe to print the saved output with")?;
    // Unique per file, not per pane: several servers in one process (the
    // tests) hand out the same pane ids.
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("keepane-replay-{}-{id}-{n}.txt", std::process::id()));
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok((vec![exe.to_string_lossy().into_owned(), "__replay".into(), path.to_string_lossy().into_owned()], path))
}

/// The program a resumed pane starts once its saved output has been
/// printed: kept with the pty's slave side until then.
struct PendingStart {
    argv: Vec<String>,
    dir: Option<String>,
    env: Vec<(String, String)>,
    /// The printer's file; removing it is what tells the printer to exit.
    path: std::path::PathBuf,
}

type Child = Box<dyn portable_pty::Child + Send + Sync>;
type Killer = Box<dyn ChildKiller + Send + Sync>;

/// Start `run` in the pane's console: the process, its id, a way to kill
/// it, and the job object that takes its process tree down with it.
fn launch(
    slave: &(dyn portable_pty::SlavePty + Send),
    id: PaneId,
    run: &[String],
    dir: Option<&str>,
    env: &[(String, String)],
) -> Result<(Child, Option<u32>, Killer, Option<crate::platform::process::Tree>)> {
    let mut cmd = CommandBuilder::new(&run[0]);
    cmd.args(&run[1..]);
    if let Some(d) = dir {
        cmd.cwd(d);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = slave.spawn_command(cmd).with_context(|| format!("spawn {:?}", run))?;
    let pid = child.process_id();
    let killer = child.clone_killer();
    let job = match crate::platform::process::Tree::of_pty(child.as_ref()) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("pane {id}: its process tree is not held: {e}");
            None
        }
    };
    Ok((child, pid, killer, job))
}

/// Report the process's exit as an event, from its own thread.
fn watch(mut child: Child, id: PaneId, generation: u32, tx: Sender<PaneEvent>) -> Result<()> {
    std::thread::Builder::new()
        .name(format!("pane-{id}-wait"))
        .spawn(move || {
            let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
            let _ = tx.send(PaneEvent::Exit(id, generation, code));
        })
        .context("spawn waiter thread")?;
    Ok(())
}

impl Pane {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        id: PaneId,
        argv: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        history: usize,
        env: &[(String, String)],
        tx: Sender<PaneEvent>,
    ) -> Result<Pane> {
        Pane::spawn_gen(id, 0, argv, cwd, cols, rows, history, env, tx, None)
    }

    /// As `spawn`, with `replay` (a resumed pane's saved output) printed
    /// into the console before the program starts.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_replaying(
        id: PaneId,
        argv: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        history: usize,
        env: &[(String, String)],
        tx: Sender<PaneEvent>,
        replay: Option<String>,
    ) -> Result<Pane> {
        Pane::spawn_gen(id, 0, argv, cwd, cols, rows, history, env, tx, replay)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_gen(
        id: PaneId,
        generation: u32,
        argv: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        history: usize,
        env: &[(String, String)],
        tx: Sender<PaneEvent>,
        replay: Option<String>,
    ) -> Result<Pane> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let pty = native_pty_system();
        let pair =
            pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).context("CreatePseudoConsole")?;
        // The reader goes first: what is printed below must be drained
        // while it is printed, or a long replay fills the pipe and the
        // printer never finishes.
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let tx_out = tx.clone();
        std::thread::Builder::new()
            .name(format!("pane-{id}-read"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx_out.send(PaneEvent::Output(id, generation, buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .context("spawn reader thread")?;
        let dir = cwd.filter(|d| std::path::Path::new(d).is_dir()).map(str::to_string);
        // A resumed pane's saved output is printed into the console by a
        // helper process first, so it sits in the console's own buffer and
        // survives every later repaint and resize (ConPTY reflows it), as
        // text fed to our screen model alone did not. The program itself
        // starts when the helper exits (`start_pending`, from the exit
        // event): waiting here would block the server, and with it the
        // answers ConPTY expects to its own queries while a process starts.
        //
        // A pty with no screen of its own (Unix) leaves keepane's screen model
        // the only one: the saved output goes straight into it instead.
        //
        // What runs is the command plus keepane's shell integration (a prompt
        // hook that reports the directory); what is remembered and shown
        // is the command as given.
        let (replay, direct) = match replay.filter(|t| !t.is_empty()) {
            Some(t) if !crate::platform::console::PTY_HAS_SCREEN => (None, Some(t)),
            r => (r, None),
        };
        let (run, pending) = match replay.map(|t| replay_argv(id, &t)) {
            Some(Ok((helper, path))) => {
                (helper, Some(PendingStart { argv: argv.to_vec(), dir: dir.clone(), env: env.to_vec(), path }))
            }
            Some(Err(e)) => {
                log::warn!("pane {id}: replay skipped: {e:#}");
                (crate::config::with_shell_integration(argv), None)
            }
            None => (crate::config::with_shell_integration(argv), None),
        };
        let (child, pid, killer, job) = launch(pair.slave.as_ref(), id, &run, dir.as_deref(), env)?;
        // The printer's exit is tagged apart: it is not the pane's program.
        watch(child, id, if pending.is_some() { HELPER_GEN } else { generation }, tx.clone())?;
        // The slave side stays open while a program is still to be started in it.
        let slave = pending.is_some().then_some(pair.slave);
        let writer = pair.master.take_writer().context("pty writer")?;

        let mut pane = Pane {
            slave,
            pending,
            tx,
            id,
            generation,
            parser: vt100::Parser::new_with_callbacks(rows, cols, history, Callbacks::default()),
            master: pair.master,
            writer,
            killer,
            job,
            title: String::new(),
            program_title: None,
            command: std::path::Path::new(&argv[0])
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| argv[0].clone()),
            argv: argv.to_vec(),
            cwd: cwd.map(str::to_string),
            announced: false,
            replay_done: false,
            exit_code: None,
            clock: false,
            copy: None,
            bell: false,
            cols,
            rows,
            pipe: None,
            recorder: None,
            pid,
            spawned_at: std::time::Instant::now(),
            last_output: std::time::Instant::now(),
            output_count: 0,
            marks: std::collections::VecDeque::new(),
            finished: Vec::new(),
            log_to: None,
            logged: 0,
            log_hold: std::collections::VecDeque::new(),
            log_hold_since: None,
            log_buf: String::new(),
            died_at: None,
            actor: Default::default(),
            prompted: false,
            syntax: Default::default(),
            delivered_line: None,
            delivered_text: None,
            settle: false,
            shell_history: None,
        };
        // Before anything the program prints: the server reads that only
        // after this returns.
        if let Some(text) = direct {
            pane.process_output(text.as_bytes());
        }
        Ok(pane)
    }

    /// Whether the process that just ran was the printer of a resumed pane's
    /// saved output, with the pane's own program still to start.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Start the pane's program in the same console the printer just
    /// filled. Same generation: the reader thread tags output with the one
    /// it was started with, and the printer, having exited, has no more.
    pub fn start_pending(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { anyhow::bail!("nothing pending") };
        let Some(slave) = self.slave.take() else { anyhow::bail!("no console to start in") };
        let run = crate::config::with_shell_integration(&p.argv);
        let (child, pid, killer, job) = launch(slave.as_ref(), self.id, &run, p.dir.as_deref(), &p.env)?;
        watch(child, self.id, self.generation, self.tx.clone())?;
        // The program is in; the printer may go (it waits for this).
        let _ = std::fs::remove_file(&p.path);
        self.pid = pid;
        self.killer = killer;
        self.job = job;
        self.exit_code = None;
        self.died_at = None;
        self.spawned_at = std::time::Instant::now();
        Ok(())
    }

    /// Start recording this pane's output to `path` as asciinema v2,
    /// replacing any recording in progress.
    pub fn record_to(&mut self, path: &str) -> Result<()> {
        use std::io::Write as _;
        self.recorder = None;
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(path).with_context(|| format!("record: cannot create {path}"))?,
        );
        let header = serde_json::json!({
            "version": 2,
            "width": self.cols,
            "height": self.rows,
            "timestamp": chrono::Local::now().timestamp(),
            "env": { "TERM": "xterm-256color", "SHELL": self.command },
            "title": self.display_title(),
        });
        writeln!(file, "{header}").context("record: write header")?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1024);
        std::thread::Builder::new()
            .name(format!("pane-{}-record", self.id))
            .spawn(move || {
                for line in rx {
                    if file.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                }
                let _ = file.flush();
            })
            .context("record: spawn writer thread")?;
        self.recorder = Some(Recorder { path: path.to_string(), started: std::time::Instant::now(), tx });
        Ok(())
    }

    /// Output for the recording, if one is running.
    pub fn record_write(&mut self, bytes: &[u8]) {
        if let Some(r) = self.recorder.as_mut() {
            r.event("o", &String::from_utf8_lossy(bytes));
        }
    }

    /// Start a `pipe-pane` command for this pane, replacing any running one.
    /// The old pipe stops first, so a failed start never leaves the previous
    /// command quietly running instead of the one that was asked for.
    ///
    /// `output` feeds the pane's output to the command (`-O`); `input`
    /// feeds what the command prints to the pane as if typed (`-I`).
    pub fn pipe_to(&mut self, command: &str, env: &[(String, String)], input: bool, output: bool) -> Result<()> {
        use std::process::Stdio;
        self.pipe = None;
        let (mut c, exe) = crate::platform::process::pipe_command(command);
        c.stdin(if output { Stdio::piped() } else { Stdio::null() })
            .stdout(if input { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().with_context(|| format!("pipe-pane: {exe}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let id = self.id;
        if let Some(mut out) = stdout {
            // `-I`: the command's output arrives as pane input through the
            // event channel, on the server thread like every other write.
            let events = self.tx.clone();
            std::thread::Builder::new()
                .name(format!("pane-{id}-pipe-in"))
                .spawn(move || {
                    let mut buf = [0u8; 4096];
                    loop {
                        match out.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if events.send(PaneEvent::Input(id, buf[..n].to_vec())).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    // Nothing more to come: an empty chunk says so, and an
                    // input-only pipe ends with it.
                    let _ = events.send(PaneEvent::Input(id, Vec::new()));
                })
                .context("pipe-pane: spawn reader thread")?;
        }
        // Bounded: a command that stops reading costs at most this much
        // memory (each chunk is one read from the pty, up to 64 KiB) before
        // output starts being dropped, which is reported. Without `-O` the
        // channel only holds the command's lifetime.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
        std::thread::Builder::new()
            .name(format!("pane-{id}-pipe"))
            .spawn(move || {
                let mut stdin = stdin;
                for b in rx {
                    if let Some(s) = stdin.as_mut()
                        && s.write_all(&b).is_err()
                    {
                        break;
                    }
                }
                drop(stdin); // the command sees end of input and can finish
                let _ = child.wait();
            })
            .context("pipe-pane: spawn writer thread")?;
        self.pipe = Some(Pipe { command: command.to_string(), tx, dropped: 0, output });
        Ok(())
    }

    /// Copy pane output into the `pipe-pane` command, if one is running.
    /// A command that has stopped reading loses bytes instead of blocking the
    /// server, and one that has exited ends the pipe. Returns the command the
    /// first time output is lost, so the loss is reported rather than silent.
    pub fn pipe_write(&mut self, bytes: &[u8]) -> Option<String> {
        let p = self.pipe.as_mut()?;
        if !p.output {
            return None; // `-I` only: nothing goes to the command
        }
        match p.tx.try_send(bytes.to_vec()) {
            Ok(()) => None,
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                let first = p.dropped == 0;
                p.dropped += bytes.len() as u64;
                first.then(|| p.command.clone())
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                self.pipe = None;
                None
            }
        }
    }

    /// Start the pane's command again in place, keeping the pane id, its
    /// position in the layout and its size (tmux `respawn-pane`), and its
    /// name, work mode and inbox. The old process and its children are
    /// killed first. Returns the message the old program was working on,
    /// which ends with it.
    pub fn respawn(
        &mut self,
        argv: Option<&[String]>,
        history: usize,
        env: &[(String, String)],
        tx: Sender<PaneEvent>,
    ) -> Result<Option<super::actor::Message>> {
        let argv = argv.map(|a| a.to_vec()).unwrap_or_else(|| self.argv.clone());
        if argv.is_empty() {
            anyhow::bail!("pane has no command to respawn");
        }
        let fresh = Pane::spawn_gen(
            self.id,
            self.generation.wrapping_add(1),
            &argv,
            self.cwd.as_deref(),
            self.cols,
            self.rows,
            history,
            env,
            tx,
            None,
        )?;
        // Putting the new pane in place drops the old one, whose Drop closes
        // the job object and takes the old process tree with it.
        let mut old = std::mem::replace(self, fresh);
        // The same shell history file, name, mode and inbox: it is the same pane.
        self.shell_history = old.shell_history.clone();
        self.actor = std::mem::take(&mut old.actor);
        let unfinished = self.actor.restarted();
        drop(old);
        Ok(unfinished)
    }

    /// Feed process output into the terminal model; answer any queries.
    pub fn process_output(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let cb = self.parser.callbacks_mut();
        if let Some(t) = cb.title.take()
            && self.program_title.as_deref() != Some(t.as_str())
        {
            self.title = t.clone();
            self.program_title = Some(t);
        }
        if let Some(d) = cb.cwd.take() {
            self.cwd = Some(d);
            self.announced = true;
        }
        let replayed = std::mem::take(&mut cb.replayed);
        if cb.bell {
            cb.bell = false;
            self.bell = true;
        }
        let marks = std::mem::take(&mut cb.marks);
        if let Some(syntax) = cb.prompted.take() {
            self.prompted = true;
            self.syntax = syntax;
        }
        if !cb.responses.is_empty() {
            let resp = std::mem::take(&mut cb.responses);
            let _ = self.writer.write_all(&resp);
        }
        if !marks.is_empty() {
            self.apply_marks(marks);
        }
        if replayed {
            self.replay_done = true;
            // What a resumed pane printed back is in the log already, from
            // before: its log starts where the program does.
            let s = self.parser.screen();
            self.logged = self.logged.max(s.scrolled_total() + u64::from(s.cursor_position().0));
        }
    }

    /// Fold what the shell said into the pane's marks.
    pub fn apply_marks(&mut self, events: Vec<MarkEvent>) {
        for e in events {
            match e {
                MarkEvent::Prompt(line, col) => {
                    // A prompt with nothing run at it yet is the same prompt
                    // again (133;A then B, an empty Enter): it moves rather
                    // than piling up.
                    let mut mark = match self.marks.back() {
                        Some(m) if m.start.is_none() && m.end.is_none() => self.marks.pop_back().unwrap(),
                        _ => Mark { line, col, entered: None, start: None, end: None, exit: None, text: String::new() },
                    };
                    mark.line = line;
                    mark.col = col;
                    mark.entered = None;
                    // A mark at or below a new prompt's line was cleared or
                    // written over (`cls`): lines only move on otherwise.
                    self.marks.retain(|m| m.line < line);
                    self.marks.push_back(mark);
                }
                MarkEvent::Start(t) => {
                    if let Some(m) = self.marks.back_mut()
                        && m.end.is_none()
                    {
                        m.start = Some(t);
                    }
                }
                MarkEvent::End(t, code) => {
                    // A D with no C before it is an empty Enter or a Ctrl+C
                    // at the prompt: nothing ran.
                    if self.marks.back().is_some_and(|m| m.start.is_some() && m.end.is_none()) {
                        self.finish_mark(None, t, code, true);
                    } else if let Some(m) = self.marks.back_mut().filter(|m| m.start.is_none() && m.end.is_none())
                        && let Some(entered) = m.entered.take()
                    {
                        // No word of its start from the shell: a command was
                        // typed, so it ran from Enter. Closed as one that ran:
                        // left open, the next prompt would take the mark for
                        // itself (the same prompt again), and the command's
                        // line would lose it.
                        let (line, col) = (m.line, m.col);
                        let command = self.text_at(line).map(|text| command_of(&text, col)).unwrap_or_default();
                        if !command.is_empty() {
                            self.finish_mark(Some(entered), t, code, true);
                        }
                    }
                }
                MarkEvent::Ran { start, end, ok } => {
                    if self.marks.back().is_some_and(|m| m.end.is_none()) {
                        // Only whether it failed: 1 stands for any failure.
                        self.finish_mark(Some(start), end, Some(if ok { 0 } else { 1 }), false);
                    }
                }
            }
        }
        // Lines the scrollback no longer holds can never be shown again.
        let s = self.parser.screen();
        let oldest = s.scrolled_total().saturating_sub(s.scrollback_rows() as u64);
        while self.marks.front().is_some_and(|m| m.line < oldest) || self.marks.len() > MAX_MARKS {
            self.marks.pop_front();
        }
    }

    /// Close the newest mark: its times, and its line as it reads now.
    /// `exact`: `exit` is the command's own code, not just 0 or 1 for
    /// success or failure (PowerShell's word).
    fn finish_mark(
        &mut self,
        start: Option<chrono::DateTime<chrono::Local>>,
        end: chrono::DateTime<chrono::Local>,
        exit: Option<i32>,
        exact: bool,
    ) {
        let Some(line) = self.marks.back().map(|m| m.line) else { return };
        match self.text_at(line) {
            Some(text) if !text.is_empty() => {
                let m = self.marks.back_mut().unwrap();
                if start.is_some() {
                    m.start = start;
                }
                m.end = Some(end);
                m.exit = exit;
                m.text = text;
                // A command with its start known, told once.
                if let Some(start) = m.start {
                    let secs = (end - start).num_milliseconds().max(0) as u64 / 1000;
                    let command = command_of(&m.text, m.col);
                    let failed = exit.is_some_and(|c| c != 0);
                    self.finished.push(Finished { command, secs, failed, exit: exit.filter(|_| exact) });
                }
            }
            // Gone already (scrolled past the history, or cleared): the
            // command cannot be shown against its line.
            _ => {
                self.marks.pop_back();
            }
        }
    }

    /// `take_log` into the batch for the history file; the batch goes
    /// out when it is big, or at once when `closing`.
    pub fn log_take(&mut self, closing: bool) {
        let text = self.take_log(closing);
        self.log_buf.push_str(&text);
        if closing || self.log_buf.len() >= LOG_BATCH {
            self.flush_log();
        }
    }

    /// Hand the batch to the history file's writer (dropped when logging
    /// is off).
    pub fn flush_log(&mut self) {
        if self.log_buf.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.log_buf);
        if let Some(path) = &self.log_to {
            crate::histlog::append(path.clone(), text);
        }
    }

    /// Whether `take_log` has anything to do.
    pub fn log_pending(&self) -> bool {
        let s = self.parser.screen();
        !self.log_hold.is_empty() || (!s.alternate_screen() && s.scrolled_total() > self.logged)
    }

    /// The text for the history log: the lines that left the screen since
    /// the last call, each reported command after a line of its times.
    /// Lines from a command still running wait for it, so its times can go
    /// first (at most `HOLD_LINES` lines for `HOLD_FOR`). `closing` takes
    /// everything, what is on the screen as well.
    pub fn take_log(&mut self, closing: bool) -> String {
        let rows = u64::from(self.rows);
        let cols = self.cols;
        let s = self.parser.screen_mut();
        if !s.alternate_screen() {
            let top = s.scrolled_total();
            let oldest = top.saturating_sub(s.scrollback_rows() as u64);
            if s.scrollback_rows() == 0 {
                // `history-limit 0`: nothing that scrolls off can be read
                // back (with any other limit, a scroll leaves at least one
                // line behind). Only the screen, at the close, is kept.
                self.logged = self.logged.max(top);
            } else if self.logged < oldest {
                let lost = oldest - self.logged;
                self.log_hold.push_back((
                    u64::MAX,
                    format!("[keepane: {lost} lines scrolled past the history before they were kept]"),
                ));
                self.logged = oldest;
            }
            let end = if closing { top + rows } else { top };
            let keep = s.scrollback();
            // A row the terminal wrapped continues on the next one: one
            // line in the log, as the program printed it. A wrapped row
            // whose rest has not scrolled off yet waits for it.
            let mut line = self.logged;
            let mut first = line;
            let mut joined = String::new();
            // A screenful at a time: finding a row in the scrollback walks
            // it from the top, so it is done once per view, not per line.
            'read: while line < end {
                let (offset, row0) = if line < top { ((top - line) as usize, 0) } else { (0, (line - top) as usize) };
                s.set_scrollback(offset);
                let n = (rows - row0 as u64).min(end - line) as usize;
                let view: Vec<(String, bool)> = s.rows_wrapped(0, cols).skip(row0).take(n).collect();
                if view.is_empty() {
                    break;
                }
                for (text, wrapped) in view {
                    joined.push_str(&text);
                    line += 1;
                    if wrapped && line < end {
                        continue;
                    }
                    if wrapped && !closing {
                        line = first;
                        break 'read;
                    }
                    self.log_hold.push_back((first, joined.trim_end().to_string()));
                    joined.clear();
                    first = line;
                }
            }
            s.set_scrollback(keep);
            self.logged = self.logged.max(line);
            if closing {
                // The screen's blank bottom is not output.
                while self.log_hold.back().is_some_and(|(l, t)| *l >= top && t.is_empty()) {
                    self.log_hold.pop_back();
                }
            }
        }
        if self.log_hold.is_empty() {
            self.log_hold_since = None;
            return String::new();
        }
        let since = *self.log_hold_since.get_or_insert_with(std::time::Instant::now);
        let give_up = closing || self.log_hold.len() > HOLD_LINES || since.elapsed() > HOLD_FOR;
        let now = chrono::Local::now();
        let mut out = String::new();
        while let Some((line, _)) = self.log_hold.front() {
            // Marks are in line order (a new prompt drops any at or below
            // its line), so the one for a line is found by halving.
            let at = self.marks.binary_search_by_key(line, |m| m.line).ok();
            if let Some(m) = at.map(|i| &self.marks[i]) {
                match m.end {
                    Some(end) => out.push_str(&log_header(m, end, now)),
                    None if !give_up => break, // still running
                    None => {}
                }
            }
            let (_, text) = self.log_hold.pop_front().unwrap();
            out.push_str(&text);
            out.push('\n');
        }
        if self.log_hold.is_empty() {
            self.log_hold_since = None;
        }
        out
    }

    /// The text of line `line` (`scrolled_total() + row`), trailing blanks
    /// dropped, while the screen or the scrollback still holds it.
    pub fn text_at(&mut self, line: u64) -> Option<String> {
        let (rows, cols) = (self.rows, self.cols);
        let s = self.parser.screen_mut();
        let top = s.scrolled_total();
        let (offset, row) = if line >= top { (0, line - top) } else { (top - line, 0) };
        if row >= u64::from(rows) || offset > s.scrollback_rows() as u64 {
            return None;
        }
        let keep = s.scrollback();
        s.set_scrollback(offset as usize);
        let text = s.rows(0, cols).nth(row as usize);
        s.set_scrollback(keep);
        text.map(|t| t.trim_end().to_string())
    }

    /// The finished commands whose lines the view `offset` lines up into
    /// the scrollback shows, still reading as they did: (row in the view,
    /// the mark). The newest mark wins a line.
    pub fn visible_marks(&mut self, offset: usize) -> Vec<(u16, Mark)> {
        if self.parser.screen().alternate_screen() || self.marks.is_empty() {
            return Vec::new();
        }
        let (rows, cols) = (self.rows, self.cols);
        let s = self.parser.screen_mut();
        let keep = s.scrollback();
        s.set_scrollback(offset);
        let top = s.scrolled_total().saturating_sub(s.scrollback() as u64);
        let bottom = top + u64::from(rows);
        let mut out: Vec<(u16, Mark)> = Vec::new();
        let texts: Vec<String> = if self.marks.iter().any(|m| m.line >= top && m.line < bottom) {
            s.rows(0, cols).collect()
        } else {
            Vec::new()
        };
        s.set_scrollback(keep);
        for m in self.marks.iter().filter(|m| m.end.is_some() && m.line >= top && m.line < bottom) {
            let row = (m.line - top) as u16;
            if texts.get(row as usize).is_some_and(|t| still_reads(t.trim_end(), &m.text, cols)) {
                out.retain(|(r, _)| *r != row);
                out.push((row, m.clone()));
            }
        }
        out
    }

    /// The text of lines `from..to` (`scrolled_total() + row`), rows the
    /// terminal wrapped joined back into the line the program printed, at
    /// most `max` bytes; and whether it was cut there.
    pub fn text_between(&mut self, from: u64, to: u64, max: usize) -> (String, bool) {
        let (rows, cols) = (u64::from(self.rows), self.cols);
        let s = self.parser.screen_mut();
        let top = s.scrolled_total();
        let from = from.max(top.saturating_sub(s.scrollback_rows() as u64));
        let to = to.min(top + rows);
        let keep = s.scrollback();
        let mut out = String::new();
        let mut line = from;
        // A screenful at a time, as `take_log` reads.
        while line < to {
            let (offset, row0) = if line < top { ((top - line) as usize, 0) } else { (0, (line - top) as usize) };
            s.set_scrollback(offset);
            let n = (rows - row0 as u64).min(to - line) as usize;
            let view: Vec<(String, bool)> = s.rows_wrapped(0, cols).skip(row0).take(n).collect();
            if view.is_empty() {
                break;
            }
            for (text, wrapped) in view {
                // A row the terminal wrapped goes on in the next one; but
                // ConPTY pads a line to the width with blanks, which marks it
                // wrapped too: a row ending in a blank ends its line.
                if wrapped && !text.ends_with(' ') {
                    out.push_str(&text);
                } else {
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
                line += 1;
            }
            if out.len() > max {
                break;
            }
        }
        s.set_scrollback(keep);
        let cut = out.len() > max;
        if cut {
            let mut end = max;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
        }
        // ConPTY pads a line with blanks to the width (and so wraps it): the
        // blanks at the end of each line are not the program's.
        let out: Vec<&str> = out.lines().map(str::trim_end).collect();
        (out.join("\n").trim_end_matches('\n').to_string(), cut)
    }

    /// Where the cursor is, as a line that keeps its number when it
    /// scrolls (`scrolled_total() + row`).
    /// The last line with something on it above the cursor: what the pane
    /// printed last, not the line being typed at (a shell at its prompt
    /// gives its last command's last line). A prompt's line (its shell
    /// marks it) is not output: a command that printed nothing gives what
    /// came before it, and a shell that printed only its prompt gives
    /// nothing. `#{pane_last_line}`, at most 200 characters.
    pub fn last_line(&self) -> String {
        self.last_lines(1, false).pop().unwrap_or_default()
    }

    /// The last `n` lines with something on them above the cursor, oldest
    /// first, prompts passed over as in `last_line`, each at most 200
    /// characters. `own`: only what the last command printed, stopping at
    /// its prompt's line (what `done-events` shows with a command).
    pub fn last_lines(&self, n: usize, own: bool) -> Vec<String> {
        let s = self.screen();
        let (row, _) = s.cursor_position();
        let top = s.scrolled_total();
        let rows: Vec<String> = s.rows(0, s.size().1).take(usize::from(row) + 1).collect();
        // The prompts' lines on the screen (marks run oldest first).
        let prompts: Vec<u64> = self.marks.iter().rev().map(|m| m.line).take_while(|&l| l >= top).collect();
        let marked = |i: usize| prompts.contains(&(top + i as u64));
        // An empty Enter's prompt is left unmarked (its mark moved down to
        // the next): it reads as the prompt waiting at the cursor now, with
        // what was typed at that since after a blank.
        let waiting = rows.last().filter(|_| marked(usize::from(row))).map(|l| l.trim());
        let empty_enter = |l: &str| {
            waiting.and_then(|w| w.strip_prefix(l.trim())).is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        };
        let mut out = Vec::new();
        for (i, l) in rows[..usize::from(row)].iter().enumerate().rev() {
            if out.len() >= n || (own && marked(i)) {
                break;
            }
            if !l.trim().is_empty() && !marked(i) && !empty_enter(l) {
                out.push(l.trim_end().replace('\t', " ").chars().take(200).collect());
            }
        }
        out.reverse();
        out
    }

    /// The row a command typed from row `from` ended on, found on the screen
    /// (up to row `to`): the first row by which the rows from `from`, read
    /// together, hold all of `text`. Blanks are left out on both sides, so
    /// where the rows broke the text does not matter. None when it is not
    /// there as typed (a shell that drew something in between).
    ///
    /// Counting the rows from the cursor at delivery (`deliver`) is not
    /// enough on Windows: when a command fills a row exactly, ConPTY may send
    /// the row and the shell's newline in two frames, and then keeps one row
    /// more than a terminal does; the next command is drawn a row lower than
    /// the prompt was seen, and its last row was taken for its output.
    pub fn command_end(&mut self, from: u64, to: u64, text: &str) -> Option<u64> {
        let want: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if want.is_empty() {
            return None;
        }
        let mut seen = String::new();
        for row in from..=to {
            let (line, _) = self.text_between(row, row + 1, 64 * 1024);
            seen.extend(line.chars().filter(|c| !c.is_whitespace()));
            if seen.contains(&want) {
                return Some(row);
            }
        }
        None
    }

    pub fn cursor_line(&self) -> u64 {
        let s = self.parser.screen();
        s.scrolled_total() + u64::from(s.cursor_position().0)
    }

    /// Input from a person or a command acting for one (keys, a paste,
    /// `send-keys`): it also makes the pane busy for its messages.
    pub fn type_input(&mut self, bytes: &[u8]) {
        self.actor.input();
        self.write_input(bytes);
    }

    /// Type a message into the pane and press Enter: as a paste when the
    /// program asked for bracketed paste, so a text of several lines
    /// arrives as one. (A shell's command of several lines comes here as
    /// one line already: `Message::wrapped`.)
    pub fn deliver(&mut self, text: &str) {
        // The rows the command takes as typed, from the cursor on: what the
        // shell prints starts after them. Counted, not read back from the
        // screen, where a row's wrap cannot be told from ConPTY's padding.
        let col = usize::from(self.screen().cursor_position().1);
        let width = unicode_width::UnicodeWidthStr::width(text);
        // The rows it fills. One that ends exactly at the right edge fills
        // just those: its Enter then goes to the next row, where the output
        // begins; but PSReadLine sometimes moves down a row of its own
        // first, so the output may begin one lower, after a blank row, which
        // is not the command's and is dropped from the output
        // (`pane_prompted`; seen on CI: the first line of output lost to a
        // row counted for the command).
        let cols = usize::from(self.cols.max(1));
        let rows = (col + width).div_ceil(cols).max(1) as u64;
        self.delivered_line = Some((self.cursor_line(), self.cursor_line() + rows));
        self.delivered_text = (!text.contains('\n')).then(|| text.to_string());
        let mut bytes = super::input::encode_paste(text, self.screen().bracketed_paste());
        bytes.push(b'\r');
        self.write_input(&bytes);
    }
    /// The bytes a typed key is to this pane's program (the platform's
    /// encoding; VT sequences follow the program's cursor-key mode).
    pub fn key_bytes(&self, rec: &crate::ipc::KeyRecord) -> Vec<u8> {
        crate::platform::keys::to_pane(rec, self.parser.screen().application_cursor())
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        if self.exit_code.is_some() {
            return;
        }
        // Enter at a prompt: when its command started, should the shell not
        // say so itself.
        if bytes.contains(&b'\r')
            && let Some(m) = self.marks.back_mut().filter(|m| m.start.is_none() && m.end.is_none())
        {
            m.entered.get_or_insert_with(chrono::Local::now);
        }
        if let Err(e) = self.writer.write_all(bytes) {
            log::warn!("pane {} write failed: {e}", self.id);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(r) = self.recorder.as_mut() {
            r.event("r", &format!("{cols}x{rows}"));
        }
        if let Err(e) = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }) {
            log::warn!("pane {} resize failed: {e}", self.id);
        }
        if let Some(c) = &mut self.copy {
            c.cx = c.cx.min(cols - 1);
            c.cy = c.cy.min(rows - 1);
        }
    }

    /// Kill the shell and its whole process tree.
    pub fn kill(&mut self) {
        // Dropping the job terminates every process in it; the killer covers
        // the (unlikely) case where the job could not be created.
        self.job = None;
        let _ = self.killer.kill();
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Total lines of scrollback currently available.
    pub fn scrollback_len(&mut self) -> usize {
        let s = self.parser.screen_mut();
        let cur = s.scrollback();
        s.set_scrollback(usize::MAX);
        let max = s.scrollback();
        s.set_scrollback(cur);
        max
    }

    /// Text of the absolute line `abs` (0 = oldest scrollback line), joined
    /// across soft-wrapped rows is *not* done here; one visual row per call.
    pub fn line_text(&mut self, abs: usize) -> (String, bool) {
        let max = self.scrollback_len();
        let rows = self.rows as usize;
        let cols = self.cols;
        // Choose an offset so that `abs` is visible: offset = max - (abs - row).
        let (offset, row) = if abs < max { (max - abs, 0usize) } else { (0, abs - max) };
        if row >= rows {
            return (String::new(), false);
        }
        let s = self.parser.screen_mut();
        let cur = s.scrollback();
        s.set_scrollback(offset);
        let text = s.rows(0, cols).nth(row).unwrap_or_default();
        let wrapped = s.row_wrapped(row as u16);
        s.set_scrollback(cur);
        (text, wrapped)
    }

    /// Find `needle` in everything this pane has on screen and in its
    /// scrollback, newest line first, at most `max` hits.
    ///
    /// Written as one pass rather than a loop over `line_text` because that
    /// recomputes the scrollback length for every line, which turns a search
    /// over a few thousand lines into a few thousand full scans.
    pub fn search(&mut self, needle: &str, max: usize, case_sensitive: bool) -> Vec<(usize, String)> {
        let mut hits = Vec::new();
        if needle.is_empty() || max == 0 {
            return hits;
        }
        let want = if case_sensitive { needle.to_string() } else { needle.to_lowercase() };
        let total = self.scrollback_len();
        let rows = self.rows as usize;
        let cols = self.cols;
        let s = self.parser.screen_mut();
        let keep = s.scrollback();
        // `abs` counts from the oldest scrolled-off line; walk from the
        // newest so the first hits reported are the ones just printed.
        for abs in (0..total + rows).rev() {
            let (offset, row) = if abs < total { (total - abs, 0usize) } else { (0, abs - total) };
            s.set_scrollback(offset);
            let Some(text) = s.rows(0, cols).nth(row) else { continue };
            let hay = if case_sensitive { text.clone() } else { text.to_lowercase() };
            if hay.contains(&want) {
                hits.push((abs, text.trim_end().to_string()));
                if hits.len() >= max {
                    break;
                }
            }
        }
        s.set_scrollback(keep);
        hits
    }

    /// Every line from absolute line `start` to the bottom of the screen,
    /// trailing blanks dropped; with `escapes`, colours and attributes are
    /// kept as escape sequences (`capture-pane -e`, `save-history`). One
    /// pass, for the same reason as `search`: `capture-pane -S -` and
    /// `save-history all` ask for thousands of lines.
    pub fn lines_from(&mut self, start: usize, escapes: bool, join: bool) -> Vec<String> {
        let total = self.scrollback_len();
        let rows = self.rows as usize;
        let cols = self.cols;
        let s = self.parser.screen_mut();
        let keep = s.scrollback();
        let mut out: Vec<String> = Vec::with_capacity((total + rows).saturating_sub(start));
        // With `join`, a row the terminal wrapped continues the row above
        // it (`capture-pane -J`): one line, as the program printed it.
        let mut continues = false;
        for abs in start..total + rows {
            let (offset, row) = if abs < total { (total - abs, 0usize) } else { (0, abs - total) };
            s.set_scrollback(offset);
            let mut text = if escapes {
                String::from_utf8_lossy(&s.rows_formatted(0, cols).nth(row).unwrap_or_default()).into_owned()
            } else {
                s.rows(0, cols).nth(row).unwrap_or_default()
            };
            let wrapped = join && s.row_wrapped(row as u16);
            if !wrapped {
                text.truncate(text.trim_end().len());
            }
            if text.contains('\x1b') && !wrapped {
                text.push_str("\x1b[0m"); // never leak a colour into the next line
            }
            if continues && let Some(last) = out.last_mut() {
                last.push_str(&text);
            } else {
                out.push(text);
            }
            continues = wrapped;
        }
        s.set_scrollback(keep);
        // Trailing blank lines are noise; a line that is only a reset is blank too.
        while out.last().is_some_and(|l| l.is_empty() || l == "\x1b[0m") {
            out.pop();
        }
        out
    }

    /// The directory the pane is in: what its shell announced (the
    /// PowerShell prompt hook, a profile's OSC 7 / 9;9), else the
    /// program's own current directory read from the process, else where
    /// it was started.
    pub fn current_path(&self) -> Option<String> {
        if self.announced {
            return self.cwd.clone();
        }
        self.pid.filter(|_| self.exit_code.is_none()).and_then(crate::proccwd::process_cwd).or_else(|| self.cwd.clone())
    }

    /// Display name for the status line.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() { &self.command } else { &self.title }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // What it still shows goes to its history log, with anything held.
        if self.log_to.is_some() {
            self.log_take(true);
            self.log_to = None;
        }
        // Field drop order would close the ConPTY before the job; kill first so
        // ClosePseudoConsole never waits on a live process tree.
        self.job = None;
        if self.exit_code.is_none() {
            let _ = self.killer.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell whose prompt is `>`: cmd, or sh.
    fn prompt_shell() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd.exe".into(), "/q".into(), "/k".into(), "prompt $g".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "PS1='> ' exec /bin/sh -i".into()]
        }
    }

    /// A program that exits at once.
    fn exits_at_once() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd.exe".into(), "/c".into(), "exit".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "exit".into()]
        }
    }
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    fn pump(
        pane: &mut Pane,
        rx: &std::sync::mpsc::Receiver<PaneEvent>,
        until: impl Fn(&Pane) -> bool,
        timeout: Duration,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if until(pane) {
                return true;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(PaneEvent::Output(_, _, b)) => pane.process_output(&b),
                Ok(PaneEvent::Exit(_, _, c)) => pane.exit_code = Some(c),
                Ok(PaneEvent::Input(_, b)) => pane.write_input(&b),
                Err(_) => {}
            }
        }
        until(pane)
    }

    /// Two processes one after the other in one ConPTY: the first (a
    /// printer) exits and the second sees a console with its text in it.
    /// ConPTY asks the terminal where the cursor is (`ESC[6n`) while a
    /// process starts and holds it until answered: that answer is what the
    /// server's screen model gives while it keeps running, and what this
    /// test gives by hand. (Waiting for the printer on the server thread
    /// deadlocked exactly there.)
    #[cfg(windows)]
    #[test]
    fn a_first_process_can_exit_and_a_second_can_follow() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::io::{Read as _, Write as _};
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = got.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        let mut first = CommandBuilder::new("cmd.exe");
        first.args(["/d", "/c", "echo hello-from-first"]);
        let mut child = pair.slave.spawn_command(first).unwrap();
        let started = std::time::Instant::now();
        let mut exited = None;
        let mut answered = false;
        while started.elapsed() < Duration::from_secs(5) {
            if !answered && got.lock().unwrap().windows(4).any(|w| w == b"\x1b[6n") {
                writer.write_all(b"\x1b[1;1R").unwrap();
                answered = true;
            }
            match child.try_wait() {
                Ok(Some(s)) => {
                    exited = Some(s.exit_code());
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("try_wait: {e}"),
            }
        }
        assert!(answered, "ConPTY asked for the cursor position");
        assert_eq!(
            exited,
            Some(0),
            "the first process exits ({:?} so far)",
            String::from_utf8_lossy(&got.lock().unwrap())
        );
        let mut second = CommandBuilder::new("cmd.exe");
        second.args(["/d", "/c", "echo hello-from-second"]);
        let mut child2 = pair.slave.spawn_command(second).unwrap();
        let _ = child2.wait();
        std::thread::sleep(Duration::from_millis(300));
        let text = String::from_utf8_lossy(&got.lock().unwrap()).to_string();
        assert!(text.contains("hello-from-first") && text.contains("hello-from-second"), "{text:?}");
    }

    /// What ConPTY passes through of what a process prints: the marker a
    /// resumed pane's printer ends with must come out the other side.
    #[cfg(windows)]
    #[test]
    fn conpty_passes_the_replay_marker_through() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::io::{Read as _, Write as _};
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = got.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        let text = format!("before{REPLAY_MARKER}after\r\n");
        let path = std::env::temp_dir().join(format!("keepane-marker-test-{}.txt", std::process::id()));
        std::fs::write(&path, &text).unwrap();
        let mut cmd = CommandBuilder::new(helper_exe().expect("keepane.exe (KEEPANE_EXE or beside the tests)"));
        cmd.arg("__replay");
        cmd.arg(&path);
        cmd.env("KEEPANE_REPLAY_NO_WAIT", "1");
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        let started = std::time::Instant::now();
        let mut answered = false;
        while started.elapsed() < Duration::from_secs(10) {
            if !answered && got.lock().unwrap().windows(4).any(|w| w == b"\x1b[6n") {
                writer.write_all(b"\x1b[1;1R").unwrap();
                answered = true;
            }
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(200));
        let out = got.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("before") && text.contains("after"), "{text:?}");
        assert!(
            out.windows(REPLAY_MARKER.len()).any(|w| w == REPLAY_MARKER.as_bytes()),
            "marker through ConPTY: {text:?}"
        );
    }

    /// Where the pty keeps no screen, a resumed pane's saved output is on
    /// keepane's screen before the program starts, and the program starts
    /// at once (no printer in between).
    #[cfg(unix)]
    #[test]
    fn saved_output_goes_straight_onto_the_screen() {
        let (tx, _rx) = channel();
        let argv = prompt_shell();
        let replay = Some("saved-one\r\nsaved-two\r\n".to_string());
        let p = Pane::spawn_replaying(14, &argv, None, 30, 6, 10, &[], tx, replay).unwrap();
        assert!(!p.is_pending());
        let text = p.screen().contents();
        assert!(text.starts_with("saved-one\nsaved-two"), "{text:?}");
        assert_eq!(p.screen().cursor_position(), (2, 0));
    }

    /// The screen model keeps what a resize pushes off the screen: shrunk,
    /// the top lines go to the scrollback; grown, they come back.
    #[test]
    fn shrinking_keeps_lines_in_scrollback_and_growing_brings_them_back() {
        let mut p = vt100::Parser::new(10, 40, 100);
        for i in 1..=8 {
            p.process(format!("line-{i}\r\n").as_bytes());
        }
        p.process(b"prompt>");
        assert_eq!(p.screen().cursor_position(), (8, 7));
        // 10 -> 4 rows: the cursor (row 8) needs 5 rows above it gone.
        p.screen_mut().set_size(4, 40);
        assert_eq!(p.screen().cursor_position(), (3, 7));
        assert_eq!(p.screen().contents().lines().filter(|l| !l.trim().is_empty()).count(), 4);
        assert!(p.screen().contents().contains("line-8") && p.screen().contents().contains("prompt>"));
        {
            let s = p.screen_mut();
            s.set_scrollback(usize::MAX);
            assert_eq!(s.scrollback(), 5, "five lines went to the scrollback, none were lost");
            s.set_scrollback(0);
        }
        {
            let s = p.screen_mut();
            s.set_scrollback(5);
            assert_eq!(s.rows(0, 40).next().unwrap().trim_end(), "line-1", "the oldest line is at the top of it");
            s.set_scrollback(0);
            assert_eq!(s.rows(0, 40).next().unwrap().trim_end(), "line-6", "the screen starts where it left off");
        }
        // 4 -> 10 rows: blank rows below, as the console behind a ConPTY
        // does it; nothing is lost, the scrollback keeps the five.
        p.screen_mut().set_size(10, 40);
        assert_eq!(p.screen().cursor_position(), (3, 7));
        assert_eq!(p.screen().rows(0, 40).next().unwrap().trim_end(), "line-6");
        {
            let s = p.screen_mut();
            s.set_scrollback(usize::MAX);
            assert_eq!(s.scrollback(), 5);
            s.set_scrollback(0);
        }
        // Typing goes on where it left off.
        p.process(b" typed");
        assert!(
            p.screen().contents().lines().nth(3).unwrap().starts_with("prompt> typed"),
            "{}",
            p.screen().contents()
        );
    }

    #[cfg(windows)]
    #[test]
    fn announced_paths_may_be_quoted() {
        // Windows Terminal's PowerShell snippet: ESC]9;9;"C:\Users\me"ESC\
        assert_eq!(path_from_announced("\"C:\\Users\\me\"").as_deref(), Some("C:\\Users\\me"));
        assert_eq!(path_from_announced(" \"D:/x\" ").as_deref(), Some("D:\\x"));
        assert_eq!(path_from_announced("C:\\x").as_deref(), Some("C:\\x"));
        assert_eq!(path_from_announced("\"\""), None);
    }

    #[cfg(unix)]
    #[test]
    fn announced_directories_are_taken_when_they_are_here() {
        let tmp = std::env::temp_dir().join(format!("keepane-announced {}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let t = tmp.to_string_lossy().into_owned();
        let url = format!("file://some-host{}", t.replace(' ', "%20"));
        assert_eq!(path_from_announced(&url).as_deref(), Some(t.as_str()));
        assert_eq!(path_from_announced(&format!("\"{t}\"")).as_deref(), Some(t.as_str()));
        // Not a directory here (a remote shell's), not absolute, not a path.
        assert_eq!(path_from_announced("file://far/definitely/not/here/xyz"), None);
        assert_eq!(path_from_announced("C:\\src"), None);
        assert_eq!(path_from_announced(""), None);
        assert_eq!(path_from_announced("file://"), None);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn cmd_echo_roundtrip() {
        let (tx, rx) = channel();
        let argv = prompt_shell();
        let mut p = Pane::spawn(1, &argv, None, 80, 24, 100, &[], tx).unwrap();
        assert!(pump(&mut p, &rx, |p| p.screen().contents().contains('>'), Duration::from_secs(10)), "no prompt");
        p.write_input(b"echo hello-keepane\r");
        assert!(
            pump(&mut p, &rx, |p| p.screen().contents().matches("hello-keepane").count() >= 2, Duration::from_secs(10)),
            "no echo: {:?}",
            p.screen().contents()
        );
        p.write_input(b"exit\r");
        assert!(pump(&mut p, &rx, |p| p.exit_code.is_some(), Duration::from_secs(10)), "no exit");
        assert_eq!(p.exit_code, Some(0));
    }

    #[test]
    fn resize_reaches_child() {
        let (tx, rx) = channel();
        let argv = prompt_shell();
        let mut p = Pane::spawn(2, &argv, None, 80, 24, 100, &[], tx).unwrap();
        assert!(pump(&mut p, &rx, |p| p.screen().contents().contains('>'), Duration::from_secs(10)));
        p.resize(100, 30);
        assert_eq!(p.screen().size(), (30, 100));
        // The size the program sees: `mode con` prints it, and `stty size`.
        p.write_input(if cfg!(windows) { b"mode con\r".as_slice() } else { b"stty size\r".as_slice() });
        // `mode con` output is localized; check the numbers at line ends.
        let has = |p: &Pane, n: &str| p.screen().contents().lines().any(|l| l.trim_end().ends_with(n));
        assert!(
            // `mode con`: rows and columns on lines of their own; `stty size`: "30 100".
            pump(
                &mut p,
                &rx,
                |p| if cfg!(windows) { has(p, "100") && has(p, "30") } else { has(p, "30 100") },
                Duration::from_secs(10)
            ),
            "{:?}",
            p.screen().contents()
        );
        p.kill();
        assert!(pump(&mut p, &rx, |p| p.exit_code.is_some(), Duration::from_secs(10)));
    }

    #[test]
    fn kill_takes_grandchildren_down() {
        let (tx, rx) = channel();
        // The shell starts ping (a grandchild of keepane) and waits for it.
        let argv = prompt_shell();
        let mut p = Pane::spawn(9, &argv, None, 80, 24, 100, &[], tx).unwrap();
        assert!(pump(&mut p, &rx, |p| p.screen().contents().contains('>'), Duration::from_secs(10)));
        let marker = std::env::temp_dir().join(format!("keepane-job-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // A grandchild that writes the marker three seconds from now.
        let wait = if cfg!(windows) { "ping -n 4 127.0.0.1 > nul & echo done >" } else { "sleep 3; echo done >" };
        p.write_input(format!("{wait} \"{}\"\r", marker.display()).as_bytes());
        std::thread::sleep(Duration::from_millis(500));
        p.kill();
        assert!(pump(&mut p, &rx, |p| p.exit_code.is_some(), Duration::from_secs(10)), "shell did not die");
        // ping would finish after ~3s and write the marker; it must not.
        std::thread::sleep(Duration::from_secs(4));
        assert!(!marker.exists(), "grandchild survived the pane kill");
    }

    /// A wide character straddling the new right edge after a shrink used to
    /// leave a half-wide cell in the last column; the next erase then indexed
    /// past the row (vt100 0.16.2 `Row::clear_wide`). Seen live with a CJK
    /// An inline interface (Codex CLI) keeps a box at the bottom and pushes
    /// its output up through a scroll region that starts at the top row:
    /// what leaves the top goes to the scrollback, in order, and the count of
    /// rows that left moves with it; the box stays. A region lower down
    /// scrolls part of the screen and keeps nothing.
    #[test]
    fn a_region_from_the_top_row_keeps_what_scrolls_off() {
        let mut p = quiet_pane(20, 6, 100);
        p.process_output(b"\x1b[6;1Hbox\x1b[1;4r\x1b[4;1H");
        for i in 1..=10 {
            p.process_output(format!("\r\nline-{i}").as_bytes());
        }
        let s = p.parser.screen_mut();
        // Four blank rows went first, then line-1 to line-6.
        assert_eq!(s.scrollback_rows(), 10);
        assert_eq!(s.scrolled_total(), 10, "the count moves with the scrollback");
        s.set_scrollback(6);
        assert_eq!(s.rows(0, 20).next().unwrap().trim_end(), "line-1");
        s.set_scrollback(0);
        let rows: Vec<String> = s.rows(0, 20).map(|r| r.trim_end().to_string()).collect();
        assert_eq!(rows, ["line-7", "line-8", "line-9", "line-10", "", "box"]);
        // A region from row 3: nothing kept.
        p.process_output(b"\x1b[3;5r\x1b[5;1H");
        for _ in 0..5 {
            p.process_output(b"\r\nmiddle");
        }
        assert_eq!(p.parser.screen_mut().scrollback_rows(), 10);
    }

    /// IME in a 50-column pane.
    #[test]
    fn shrink_through_wide_char_then_erase_does_not_panic() {
        let mut p = vt100::Parser::new(5, 100, 0);
        let line = format!("{}中x", "a".repeat(49)); // 中 occupies cols 49-50
        p.process(line.as_bytes());
        p.screen_mut().set_size(5, 50);
        // Cursor to the last column, erase to end of line, then overwrite.
        p.process(b"\x1b[1;50H\x1b[K\x1b[1;49H\x1b[2X\x1b[1;1H\x1b[P");
        let row: String = p.screen().rows(0, 50).next().unwrap();
        assert!(!row.contains('中'), "{row:?}");
        // The same through a Pane resize (what a split does).
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut pane = Pane::spawn(11, &argv, None, 100, 5, 10, &[], tx).unwrap();
        pane.parser.process(line.as_bytes());
        pane.resize(50, 5);
        pane.parser.process(b"\x1b[1;50H\x1b[K");
        assert_eq!(pane.screen().size(), (5, 50));
    }

    /// A pane fed by hand: its own program exits at once and its output is
    /// never read, so only what the test writes reaches the screen.
    fn quiet_pane(cols: u16, rows: u16, history: usize) -> Pane {
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        Pane::spawn(12, &argv, None, cols, rows, history, &[], tx).unwrap()
    }

    #[test]
    fn a_delivered_command_takes_the_rows_it_is_typed_over() {
        let mut p = quiet_pane(10, 5, 100);
        p.process_output(b"PS> ");
        // Up to the right edge exactly: the one row it fills; the output
        // begins on the next (a blank row PSReadLine may add before it is
        // dropped from the output, not counted here).
        p.deliver("abcdef");
        assert_eq!(p.delivered_line, Some((0, 1)));
        p.process_output(b"\r\n\r\nPS> ");
        p.deliver("ab");
        assert_eq!(p.delivered_line, Some((2, 3)), "short of the edge: its own row");
        p.deliver(&"x".repeat(17));
        assert_eq!(p.delivered_line, Some((2, 5)), "4 + 17 columns: over three rows");
    }

    /// What a pane printed last, for the phone's list: above the line being
    /// typed at, blank lines passed over; nothing on a blank screen or one
    /// with only the line being typed at.
    #[test]
    fn the_last_line_is_what_was_printed_last() {
        let mut p = quiet_pane(20, 6, 100);
        assert_eq!(p.last_line(), "");
        p.process_output(b"PS> ");
        assert_eq!(p.last_line(), "", "only the prompt");
        p.process_output(b"cargo test\r\ntests: 42 passed\r\n\r\nPS> ");
        assert_eq!(p.last_line(), "tests: 42 passed");
        p.process_output(b"echo a\tb\r\na\tb\r\nPS> x");
        assert_eq!(p.last_line(), "a       b", "a tab as the screen shows it");
    }

    /// What was typed: the line after the prompt's column, wide characters
    /// two columns each; nothing when the line ends before the column.
    #[test]
    fn the_command_is_what_follows_the_prompt() {
        assert_eq!(command_of("PS C:\\src> cargo test", 11), "cargo test");
        assert_eq!(command_of("中文> ls -l ", 5), "ls -l", "two wide characters, then '> '");
        assert_eq!(command_of("$ make", 0), "$ make", "no column known: the whole line");
        assert_eq!(command_of("$", 2), "", "the prompt and nothing typed after it");
        // Marked by a real prompt: the column comes with it.
        let mut p = quiet_pane(40, 6, 100);
        p.process_output(format!("PS C:\\> {B}echo hi\r\nhi\r\n{}PS C:\\> {B}", ran(1_000, 3_500, true)).as_bytes());
        assert_eq!(p.finished, [Finished { command: "echo hi".into(), secs: 2, failed: false, exit: None }]);
        // PowerShell's failure is a failure with no code; FTCS gives the code.
        p.process_output(format!("bad\r\n{}PS C:\\> {B}", ran(4_000, 5_000, false)).as_bytes());
        assert_eq!(p.finished[1], Finished { command: "bad".into(), secs: 1, failed: true, exit: None });
        let mut q = quiet_pane(40, 6, 100);
        q.process_output(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\false\x1b]133;C\x1b\\\r\n\x1b]133;D;3\x1b\\");
        assert_eq!((q.finished[0].failed, q.finished[0].exit), (true, Some(3)));
    }

    /// The last lines: `own` stops at the last command's prompt, so what
    /// an earlier command printed is not taken for this one's.
    #[test]
    fn the_last_lines_are_the_last_commands_own() {
        let mut p = quiet_pane(40, 10, 100);
        p.process_output(format!("PS> {B}first\r\nold output\r\n{}PS> {B}", ran(1, 2, true)).as_bytes());
        p.process_output(format!("second\r\none\r\n\r\ntwo\r\nthree\r\n{}PS> {B}", ran(3, 4, true)).as_bytes());
        assert_eq!(p.last_lines(2, true), ["two", "three"]);
        assert_eq!(p.last_lines(9, true), ["one", "two", "three"], "not the first command's output");
        assert_eq!(p.last_lines(9, false), ["old output", "one", "two", "three"]);
        p.process_output(format!("cd x\r\n{}PS> {B}", ran(5, 6, true)).as_bytes());
        assert!(p.last_lines(5, true).is_empty(), "a command that printed nothing");
        assert_eq!(p.last_line(), "three");
    }

    /// bash before 4.4 (macOS's own) has no `PS0`, so no word of a
    /// command's start: Enter at its prompt stands in, for `done-events`
    /// only. An empty Enter is nothing; the stamps stay the shell's.
    #[test]
    fn enter_stands_in_for_a_start_the_shell_does_not_give() {
        let (a, b) = ("\x1b]133;A\x07", "\x1b]133;B\x07");
        let d = |code: u8| format!("\x1b]133;D;{code}\x1b\\");
        let mut p = quiet_pane(40, 8, 100);
        p.process_output(format!("{a}$ {b}").as_bytes());
        p.write_input(b"\r");
        p.process_output(format!("\r\n{}{a}$ {b}", d(0)).as_bytes());
        assert!(p.finished.is_empty(), "an empty Enter ran nothing");
        p.write_input(b"make\r");
        p.write_input(b"\r"); // typed ahead: the first Enter is the start
        p.process_output(format!("make\r\nbuilt\r\n{}{a}$ {b}", d(2)).as_bytes());
        assert_eq!(p.finished, [Finished { command: "make".into(), secs: 0, failed: true, exit: Some(2) }]);
        // The command's line keeps its mark (stamped, from Enter): the next
        // prompt does not take it, and a command's own lines end there.
        let ran: Vec<&Mark> = p.marks.iter().filter(|m| m.end.is_some()).collect();
        assert_eq!(ran.len(), 1, "one stamp, the command's, not the empty Enter's: {:?}", p.marks);
        assert_eq!(p.last_lines(5, true), ["built"], "its own output, not what came before its prompt");
        // A shell that does say (bash 4.4+, zsh): its word, as before.
        p.finished.clear();
        p.write_input(b"ls\r");
        p.process_output(format!("ls\x1b]133;C\x1b\\\r\nx\r\n{}{a}$ {b}", d(0)).as_bytes());
        assert_eq!(p.finished.len(), 1);
        assert!(p.marks.iter().any(|m| m.end.is_some()), "stamped as before");
    }

    /// With the shell marking its prompts, a prompt's line is not output:
    /// a command that printed nothing, an empty Enter, a fresh shell.
    #[test]
    fn a_prompt_is_not_what_was_printed_last() {
        let mut p = quiet_pane(30, 8, 100);
        p.process_output(format!("PS C:\\> {B}").as_bytes());
        assert_eq!(p.last_line(), "", "a fresh shell: only its prompt");
        p.process_output(b"echo hi\r\nhi\r\n");
        p.process_output(format!("{}PS C:\\> {B}", ran(1, 2, true)).as_bytes());
        assert_eq!(p.last_line(), "hi");
        p.process_output(b"cd src\r\n");
        p.process_output(format!("{}PS C:\\src> {B}", ran(3, 4, true)).as_bytes());
        assert_eq!(p.last_line(), "hi", "cd printed nothing: its prompt line is passed over");
        p.process_output(format!("\r\nPS C:\\src> {B}").as_bytes());
        assert_eq!(p.last_line(), "hi", "an empty Enter's prompt too");
        p.process_output(b"ab");
        assert_eq!(p.last_line(), "hi", "typing at the prompt changes nothing");
        // Output that is only the start of the prompt's first word is still
        // output (a prompt `user@host:~$`, a command printing `user`).
        let mut p = quiet_pane(30, 8, 100);
        p.process_output(format!("user@host:~$ {B}whoami\r\nuser\r\n{}user@host:~$ {B}", ran(1, 2, true)).as_bytes());
        assert_eq!(p.last_line(), "user");
    }

    /// The command's end read off the screen, wherever it was drawn: here a
    /// row lower than the prompt was seen at delivery (ConPTY, after a
    /// command that filled a row exactly), where counting from the cursor
    /// took its last row for the output.
    #[test]
    fn a_command_is_found_where_it_was_drawn() {
        let mut p = quiet_pane(10, 8, 100);
        p.process_output(b"PS> ");
        p.deliver("abcdefghij");
        assert_eq!(p.delivered_line, Some((0, 2)), "4 + 10 columns, counted from the cursor");
        // Drawn from row 1, not 0; then its output and the next prompt.
        p.process_output(b"\x1b[2;5Habcdefghij\r\nout1\r\nPS> ");
        let to = p.cursor_line();
        assert_eq!(p.text_between(2, to, 1000).0, "ghij\nout1", "what counting took");
        assert_eq!(p.command_end(0, to, "abcdefghij"), Some(2));
        assert_eq!(p.text_between(3, to, 1000).0, "out1");
        // Broken at a blank between rows, found all the same.
        let mut p = quiet_pane(10, 8, 100);
        p.process_output(b"PS> echo a\r\nbcd\r\nout\r\nPS> ");
        assert_eq!(p.command_end(0, p.cursor_line(), "echo a bcd"), Some(1));
        // Not there as typed: none, and the count stands.
        assert_eq!(p.command_end(0, p.cursor_line(), "something else"), None);
        assert_eq!(p.command_end(0, p.cursor_line(), " "), None);
    }

    #[test]
    fn a_line_padded_to_the_width_ends_where_a_wrapped_one_goes_on() {
        let mut p = quiet_pane(10, 5, 100);
        // ConPTY's way: a short line padded with blanks to the width (the
        // row counts as wrapped), then a line that really wraps.
        p.process_output(b"PS> cmd   22\r\nabcdefghijklmn\r\nend");
        assert_eq!(p.text_between(0, 4, 1000), ("PS> cmd\n22\nabcdefghijklmn".to_string(), false));
        // Cut at the size asked for, on a character boundary.
        let (text, cut) = p.text_between(0, 4, 5);
        assert!(cut && text.len() <= 5, "{text:?}");
    }

    const B: &str = "\x1b]133;B\x1b\\";

    fn ran(start: i64, end: i64, ok: bool) -> String {
        format!("\x1b]7777;keepane-cmd;{start};{end};{}\x1b\\", ok as u8)
    }

    #[test]
    fn a_command_is_marked_on_its_line_and_follows_it_up() {
        let mut p = quiet_pane(40, 5, 100);
        // Prompt, a command typed at it, its output, the next prompt with
        // the report of the one before.
        p.process_output(format!("PS> {B}").as_bytes());
        p.process_output(b"echo hi\r\nhi\r\n");
        p.process_output(format!("{}PS> {B}", ran(1_000, 4_200, true)).as_bytes());
        assert_eq!(p.marks.len(), 2, "{:?}", p.marks);
        let m = &p.marks[0];
        assert_eq!((m.line, m.exit, m.text.as_str()), (0, Some(0), "PS> echo hi"));
        assert_eq!(m.end.unwrap().timestamp_millis() - m.start.unwrap().timestamp_millis(), 3_200);
        assert_eq!(p.marks[1].end, None, "the prompt now waiting has no command yet");
        let rows: Vec<u16> = p.visible_marks(0).iter().map(|(r, _)| *r).collect();
        assert_eq!(rows, [0]);
        // Output pushes the line into the scrollback: off the live view,
        // on the view scrolled back to it, still the same line.
        p.process_output(b"\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert!(p.visible_marks(0).is_empty());
        let top = p.screen().scrolled_total();
        let back = p.visible_marks(top as usize);
        assert_eq!(back.len(), 1, "{back:?}");
        assert_eq!(back[0].0, 0);
        assert_eq!(p.text_at(0).as_deref(), Some("PS> echo hi"));
    }

    #[test]
    fn empty_enters_and_shells_saying_less_leave_no_marks() {
        let mut p = quiet_pane(40, 10, 100);
        // An empty Enter: a second prompt with nothing reported. The first
        // prompt moves rather than a second one piling up.
        p.process_output(format!("PS> {B}\r\nPS> {B}").as_bytes());
        assert_eq!(p.marks.len(), 1);
        assert_eq!(p.marks[0].line, 1);
        // FTCS: a D with no C before it ran nothing; with C it did, and the
        // exit code is kept.
        p.process_output(b"\x1b]133;D;0\x1b\\");
        assert_eq!(p.marks[0].end, None);
        p.process_output(b"false\x1b]133;C\x1b\\\r\n\x1b]133;D;2\x1b\\\x1b]133;A\x1b\\$ ");
        assert_eq!(p.marks[0].exit, Some(2));
        assert_eq!(p.marks[0].text, "PS> false");
        assert!(p.marks[0].start.is_some() && p.marks[0].end.is_some());
        // A full-screen program's OSCs are not the shell's.
        let n = p.marks.len();
        p.process_output(format!("\x1b[?1049h{B}{}", ran(1, 2, true)).as_bytes());
        assert_eq!(p.marks.len(), n);
        assert!(p.visible_marks(0).is_empty(), "nothing is stamped over a full-screen program");
        p.process_output(b"\x1b[?1049l");
        assert_eq!(p.visible_marks(0).len(), 1);
    }

    #[test]
    fn a_line_cleared_or_written_over_loses_its_mark() {
        let mut p = quiet_pane(40, 10, 100);
        p.process_output(format!("PS> {B}ls\r\n{}PS> {B}", ran(1, 2, false)).as_bytes());
        assert_eq!(p.visible_marks(0).len(), 1);
        assert_eq!(p.marks[0].exit, Some(1));
        // Written over in place: still in the list, no longer shown.
        p.process_output(b"\x1b[1;1Hsomething else\x1b[K\x1b[2;1H");
        assert!(p.visible_marks(0).is_empty());
        // `cls` and a prompt at the top again: the old mark goes.
        p.process_output(format!("\x1b[2J\x1b[HPS> {B}").as_bytes());
        assert_eq!(p.marks.len(), 1, "{:?}", p.marks);
        assert_eq!(p.marks[0].end, None);
        // Narrowed, a line shows only its start: still the line when the
        // row is full; a shorter row that happens to begin it is not.
        assert!(still_reads("PS> ec", "PS> echo hi", 6));
        assert!(!still_reads("PS>", "PS> echo hi", 6));
        assert!(!still_reads("PS> xx", "PS> echo hi", 6));
        assert!(still_reads("PS> echo", "PS> echo hi", 9), "cut at a blank");
        assert!(still_reads("PS> 中", "PS> 中文", 7), "cut by width, not chars");
        // A line that scrolled past the whole scrollback cannot be marked.
        let mut p = quiet_pane(40, 3, 2);
        p.process_output(format!("PS> {B}x\r\n1\r\n2\r\n3\r\n4\r\n5\r\n{}PS> {B}", ran(1, 2, true)).as_bytes());
        assert!(p.marks.iter().all(|m| m.end.is_none()), "{:?}", p.marks);
    }

    #[test]
    fn the_history_log_takes_what_scrolls_off_with_command_times() {
        let mut p = quiet_pane(20, 4, 100);
        assert!(!p.log_pending());
        // Output of no command: written as it leaves the screen.
        p.process_output(b"a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(p.take_log(false), "a\n");
        assert!(!p.log_pending());
        // A command whose output scrolls its own line off: held until it
        // is done, then written after a line of its times.
        p.process_output(format!("\r\nPS> {B}run\r\n1\r\n2\r\n3\r\n4").as_bytes());
        assert_eq!(p.take_log(false), "b\nc\nd\ne\n", "the command line waits");
        p.process_output(format!("\r\n{}PS> {B}", ran(0, 1500, false)).as_bytes());
        let text = p.take_log(false);
        assert!(text.starts_with("── ") && text.ends_with(" · 1.5s · ✗ 1 ──\nPS> run\n1\n"), "{text:?}");
        // A long line the terminal wrapped is one line again; a wrapped
        // row whose rest is still on screen waits for it.
        p.process_output(format!("{}\r\n0123456789012345678901234\r\nz\r\ny\r\nx", ran(2000, 2100, true)).as_bytes());
        let text = p.take_log(false);
        assert!(text.ends_with(" · 100ms · ✓ ──\nPS>\n"), "{text:?}");
        p.process_output(b"\r\nw");
        assert_eq!(p.take_log(false), "0123456789012345678901234\n");
        // Closing takes the screen too, without its blank bottom.
        p.process_output(b"\r\nlast\r\n\r\n");
        let text = p.take_log(true);
        assert_eq!(text, "z\ny\nx\nw\nlast\n");
        assert_eq!(p.take_log(true), "", "nothing twice");
        // A full-screen program's screen is not logged.
        p.process_output(b"\x1b[?1049h1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert!(!p.log_pending());
        // No scrollback at all: nothing can be read back once scrolled,
        // and that is not reported line after line; the screen still is.
        let mut p = quiet_pane(20, 3, 0);
        for i in 0..10 {
            p.process_output(format!("{i}\r\n").as_bytes());
            assert_eq!(p.take_log(false), "");
        }
        assert_eq!(p.take_log(true), "8\n9\n");
    }

    #[cfg(windows)]
    #[test]
    fn announced_directories_become_windows_paths() {
        let w = path_from_announced;
        assert_eq!(w("file:///C:/Users/x").as_deref(), Some("C:\\Users\\x"));
        assert_eq!(w("file://BOX/C:/Users/x%20y").as_deref(), Some("C:\\Users\\x y"));
        assert_eq!(w("C:\\src").as_deref(), Some("C:\\src"));
        assert_eq!(w("C:/src/a").as_deref(), Some("C:\\src\\a"));
        assert_eq!(w("D:").as_deref(), Some("D:\\"));
        assert_eq!(w("/mnt/c/Users/x").as_deref(), Some("C:\\Users\\x"));
        assert_eq!(w("/mnt/d").as_deref(), Some("D:\\"));
        assert_eq!(w("file://HOST/mnt/c/x").as_deref(), Some("C:\\x"));
        assert_eq!(w("/home/user"), None);
        assert_eq!(w("/mnt/wsl/x"), None);
        assert_eq!(w(""), None);
        assert_eq!(w("file://"), None);
    }

    #[cfg(windows)]
    #[test]
    fn osc_cwd_updates_the_pane() {
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut p = Pane::spawn(12, &argv, Some("C:\\"), 20, 5, 10, &[], tx).unwrap();
        assert_eq!(p.cwd.as_deref(), Some("C:\\"));
        p.process_output(b"\x1b]9;9;C:\\Users\x07");
        assert_eq!(p.cwd.as_deref(), Some("C:\\Users"));
        p.process_output(b"\x1b]7;file://HOST/mnt/c/Windows\x1b\\");
        assert_eq!(p.cwd.as_deref(), Some("C:\\Windows"));
        // A Linux-only path cannot be a Windows working directory: keep the last one.
        p.process_output(b"\x1b]7;file://HOST/home/user\x07");
        assert_eq!(p.cwd.as_deref(), Some("C:\\Windows"));
    }

    #[cfg(unix)]
    #[test]
    fn osc_cwd_updates_the_pane() {
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut p = Pane::spawn(12, &argv, Some("/"), 20, 5, 10, &[], tx).unwrap();
        assert_eq!(p.cwd.as_deref(), Some("/"));
        p.process_output(b"\x1b]7;file://HOST/usr\x1b\\");
        assert_eq!(p.cwd.as_deref(), Some("/usr"));
        p.process_output(b"\x1b]9;9;/tmp\x07");
        assert_eq!(p.cwd.as_deref(), Some("/tmp"));
        // Not a directory here: keep the last one.
        p.process_output(b"\x1b]7;file://far/definitely/not/here/xyz\x07");
        assert_eq!(p.cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn the_prompt_marker_says_which_syntax_the_shell_takes() {
        use super::super::actor::Syntax;
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut p = Pane::spawn(13, &argv, None, 20, 5, 10, &[], tx).unwrap();
        assert_eq!(p.syntax, Syntax::PowerShell);
        p.process_output(b"$ \x1b]7777;keepane-prompt;sh\x07");
        assert!(std::mem::take(&mut p.prompted));
        assert_eq!(p.syntax, Syntax::Posix);
        p.process_output(b"PS> \x1b]7777;keepane-prompt\x1b\\");
        assert!(std::mem::take(&mut p.prompted));
        assert_eq!(p.syntax, Syntax::PowerShell, "the shell at the prompt now");
        // Something else after the word is not keepane's marker.
        p.process_output(b"\x1b]7777;keepane-prompt;fish\x07");
        assert!(!p.prompted);
    }

    #[test]
    fn dsr_is_answered() {
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut p = Pane::spawn(3, &argv, None, 20, 5, 10, &[], tx).unwrap();
        p.parser.process(b"abc\x1b[6n");
        assert_eq!(p.parser.callbacks().responses, b"\x1b[1;4R");
        p.parser.process(b"\x1b[c");
        assert!(p.parser.callbacks().responses.ends_with(b"\x1b[?62;22c"));
        p.parser.process(b"\x1b]0;My Title\x07\x07");
        p.process_output(b"");
        assert_eq!(p.title, "My Title");
        assert!(p.bell);
    }

    /// A title given by hand (`select-pane -T`) stays when the program sends
    /// the same title again (a prompt that sets it every time); a new title
    /// from the program still wins, as in tmux.
    #[test]
    fn a_repeated_program_title_does_not_undo_a_title_given_by_hand() {
        let (tx, _rx) = channel();
        let argv = prompt_shell();
        let mut p = Pane::spawn(13, &argv, None, 20, 5, 10, &[], tx).unwrap();
        p.process_output(b"\x1b]0;program title\x07");
        assert_eq!(p.title, "program title");
        p.title = "logs".into(); // select-pane -T logs
        p.process_output(b"\x1b]0;program title\x07"); // the next prompt's copy
        assert_eq!(p.title, "logs", "the same title again is not a new one");
        p.process_output(b"\x1b]0;something else\x07");
        assert_eq!(p.title, "something else", "the program changing its title still wins");
    }

    #[test]
    fn scrollback_lines() {
        let (tx, _rx) = channel();
        let argv = exits_at_once();
        let mut p = Pane::spawn(4, &argv, None, 10, 3, 100, &[], tx).unwrap();
        for i in 0..10 {
            p.parser.process(format!("line{i}\r\n").as_bytes());
        }
        // 11 rows written (10 lines + empty), 3 visible -> 8 in scrollback.
        assert_eq!(p.scrollback_len(), 8);
        assert_eq!(p.line_text(0).0.trim_end(), "line0");
        assert_eq!(p.line_text(7).0.trim_end(), "line7");
        assert_eq!(p.line_text(8).0.trim_end(), "line8");
        assert_eq!(p.line_text(10).0.trim_end(), "");
        assert_eq!(p.screen().scrollback(), 0);
    }
}
