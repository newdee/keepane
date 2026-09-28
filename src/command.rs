//! The keepane command language, shared by the `keepane` CLI, the `:` prompt,
//! key bindings and the config file.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Target {
    pub session: Option<String>,
    /// Window index or name.
    pub window: Option<String>,
    pub pane: Option<usize>,
    /// `%N`: a pane by its id (as `list-panes` shows it), wherever it is.
    /// It stays the same pane when others come and go, which an index does
    /// not.
    pub pane_id: Option<u32>,
    /// `%name`: a pane by the name `rename-pane` gave it, wherever it is.
    pub pane_name: Option<String>,
    /// `host:port/…`: the rest names a pane of the keepane server on that
    /// machine (docs/design/link.md). Only `send-message` goes there.
    pub remote: Option<String>,
}

impl Target {
    /// Parse `session`, `session:window`, `:window`, `session:window.pane`,
    /// `%N` for a pane by its id, `%name` for a pane by its name, or a
    /// pane's full address `$1:@3.%7`, where the session and window say
    /// where the pane is expected to be; `host:port/` in front of any of
    /// these names it on another machine.
    pub fn parse(s: &str) -> Target {
        if let Some((machine, rest)) = crate::link::split_remote(s) {
            return Target { remote: Some(machine.to_string()), ..Target::parse(rest) };
        }
        if let Some(p) = s.strip_prefix('%') {
            let (pane_id, pane_name) = pane_ref(p);
            return Target { pane_id, pane_name, ..Default::default() };
        }
        let (sess, rest) = match s.split_once(':') {
            Some((a, b)) => (if a.is_empty() { None } else { Some(a.to_string()) }, Some(b)),
            None => (if s.is_empty() { None } else { Some(s.to_string()) }, None),
        };
        let (mut pane_id, mut pane_name) = (None, None);
        let (win, pane) = match rest {
            Some(r) => match r.split_once('.') {
                Some((w, p)) => {
                    let w = if w.is_empty() { None } else { Some(w.to_string()) };
                    match p.strip_prefix('%') {
                        Some(p) => {
                            (pane_id, pane_name) = pane_ref(p);
                            (w, None)
                        }
                        None => (w, p.parse().ok()),
                    }
                }
                None => (if r.is_empty() { None } else { Some(r.to_string()) }, None),
            },
            None => (None, None),
        };
        Target { session: sess, window: win, pane, pane_id, pane_name, remote: None }
    }

    /// The pane is named by id or name, not by its place in a window.
    pub fn names_pane(&self) -> bool {
        self.pane_id.is_some() || self.pane_name.is_some()
    }
}

/// What follows `%`: a pane id when it is all digits, else a pane name.
fn pane_ref(p: &str) -> (Option<u32>, Option<String>) {
    match p.parse() {
        Ok(id) => (Some(id), None),
        Err(_) => (None, Some(p.to_string())),
    }
}

/// What `create-pane` makes (boxed in `Cmd`, which it would otherwise
/// double in size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatePane {
    /// `session`, `window` or `split`.
    pub kind: String,
    pub target: Option<Target>,
    pub session_name: Option<String>,
    pub horizontal: bool,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub message: Option<String>,
    pub argv: Vec<String>,
}

/// Where `move-message` puts a queued message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTo {
    Up,
    Down,
    Top,
}

impl MoveTo {
    fn as_str(self) -> &'static str {
        match self {
            MoveTo::Up => "up",
            MoveTo::Down => "down",
            MoveTo::Top => "top",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneSel {
    Dir(Dir),
    Next,
    Prev,
    Last,
    /// A pane named in full (`session:window.pane`), in whatever window.
    Target(Target),
    Index(usize),
}

/// One line of a `display-menu`: a label, the key that picks it, and what it
/// runs. A label with no command is a separator (tmux writes it as `""`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub name: String,
    pub key: Option<String>,
    pub cmd: Option<Box<Cmd>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    NewSession {
        name: Option<String>,
        window_name: Option<String>,
        cwd: Option<String>,
        detached: bool,
        argv: Vec<String>,
        /// `-A`: attach to the session instead if it already exists.
        attach_existing: bool,
        /// `-x cols -y rows`: the size of a session made with `-d` (or from
        /// a script), which has no terminal to take it from.
        size: (Option<u16>, Option<u16>),
    },
    AttachSession {
        target: Option<Target>,
        detach_others: bool,
    },
    DetachClient {
        /// `-a`: every client but this one; with a session, every client of it.
        all: bool,
        target: Option<Target>,
    },
    ListSessions,
    ListWindows {
        target: Option<Target>,
    },
    /// `list-marks [-t pane]`: the commands the pane's shell ran that are
    /// still on screen or in its scrollback.
    ListMarks {
        target: Option<Target>,
    },
    /// `send-message [-t|--to pane] [-r | --re id] [--task id] [-w seconds]
    /// text`: into a pane's inbox (docs/design/mailbox.md). `-r` answers the
    /// sender of the message the calling pane is working on; `--re` answers
    /// message `id` (its sender, unless `--to` says otherwise); `--task`
    /// carries task `id` on. `-w` waits until it is delivered. Who sends it,
    /// its number and its hop are keepane's to fill in, never the sender's.
    SendMessage {
        target: Option<Target>,
        reply: bool,
        re: Option<u64>,
        task: Option<u64>,
        wait: Option<u64>,
        text: String,
    },
    /// `read-message [-t pane] [-w seconds]`: take the oldest message.
    ReadMessage {
        target: Option<Target>,
        wait: Option<u64>,
    },
    /// `list-messages [-t pane] [-a]`: what waits in an inbox (all with -a).
    ListMessages {
        target: Option<Target>,
        all: bool,
    },
    /// `trace-message id [-w seconds]`: where a message is; -w waits until
    /// it is done with.
    TraceMessage {
        id: u64,
        wait: Option<u64>,
    },
    /// `drop-message id` / `drop-message -u`: delete a queued message, or
    /// bring back the one deleted last.
    DropMessage {
        id: Option<u64>,
        undo: bool,
    },
    /// `move-message id up|down|top`: its place in the queue.
    MoveMessage {
        id: u64,
        to: MoveTo,
    },
    /// `pane-ready [-q] [-t pane]`: an agent is free for its next message.
    PaneReady {
        quiet: bool,
        target: Option<Target>,
    },
    /// `pane-status [text]`: what the calling pane says it is doing.
    PaneStatus {
        text: String,
    },
    /// `set-work-mode [-t pane] normal|shell|ai`.
    SetWorkMode {
        target: Option<Target>,
        mode: String,
    },
    /// `rename-pane [-t pane] name`: the name `%name` finds; "" clears it.
    RenamePane {
        target: Option<Target>,
        name: String,
    },
    /// `whoami`: the calling pane's address, place, name and mode.
    Whoami,
    /// `create-pane [-k session|window|split] [-t target] [-s session-name]
    /// [-h] [-c dir] [-n pane-name] [-m mode] [-M message] [-- command]`: a
    /// pane made by an agent (what MCP's create tools run). From inside a
    /// pane it is recorded as that pane's, counts against
    /// `agent-pane-limit`, and may run only an `agent-commands` program.
    CreatePane(Box<CreatePane>),

    /// `dashboard`: every pane at a glance, in a popup (prefix v).
    Dashboard,
    /// `list-tasks [-t session]`: every chain of messages, newest first.
    ListTasks {
        target: Option<Target>,
    },
    /// `show-task id`: a task's messages, step by step.
    ShowTask {
        id: u64,
    },
    /// `list-events [-t target] [-S duration] [-n lines]`: the event log's
    /// lines; -n takes the last ones from memory, without reading the files.
    ListEvents {
        target: Option<Target>,
        since: Option<u64>,
        last: Option<usize>,
    },
    ListPanes {
        target: Option<Target>,
        /// `-a`: every pane on the server; `-s`: every pane in the session.
        all: bool,
        session: bool,
        /// `-F`: each pane as this format instead of the usual line.
        format: Option<String>,
    },
    KillSession {
        target: Option<Target>,
        /// `-a`: kill every session except the target.
        all_but: bool,
    },
    KillServer {
        restarting: bool,
    },
    HasSession {
        target: Target,
    },
    RenameSession {
        target: Option<Target>,
        name: String,
    },
    NewWindow {
        name: Option<String>,
        cwd: Option<String>,
        target: Option<Target>,
        argv: Vec<String>,
        /// `-d`: do not make the new window current.
        detached: bool,
    },
    KillWindow {
        target: Option<Target>,
        all_but: bool,
    },
    RenameWindow {
        target: Option<Target>,
        name: String,
    },
    SelectWindow {
        target: Target,
    },
    /// `swap-window [-s src] [-t dst]` / `move-window [-s src] [-t dst]`:
    /// reorder the windows of a session.
    SwapWindow {
        src: Option<Target>,
        dst: Option<Target>,
        /// `move-window`: take the source out and put it at the destination,
        /// shifting the rest, instead of exchanging the two.
        move_it: bool,
    },
    NextWindow {
        target: Option<Target>,
        /// `-a`: the next window with an alert (activity, bell or silence).
        alert: bool,
    },
    PreviousWindow {
        target: Option<Target>,
        alert: bool,
    },
    LastWindow {
        target: Option<Target>,
    },
    SplitWindow {
        horizontal: bool,
        cwd: Option<String>,
        target: Option<Target>,
        argv: Vec<String>,
        /// `-d`: keep the current pane active.
        detached: bool,
        /// `-b`: put the new pane before (left of / above) the target.
        before: bool,
        /// `-f`: span the full window width/height instead of the target pane.
        full: bool,
        /// `-N count` (keepane's own): make that many new panes and tile the
        /// window. 1 is an ordinary split.
        count: u16,
    },
    KillPane {
        target: Option<Target>,
        all_but: bool,
    },
    SelectPane {
        sel: PaneSel,
    },
    /// `select-pane -T title` / `-m` / `-M`: name a pane, or mark it so
    /// `join-pane` and `swap-pane` know where to take a pane from.
    PaneTitle {
        target: Option<Target>,
        title: Option<String>,
        mark: bool,
        unmark: bool,
    },
    /// `resize-window [-t target] [-x width] [-y height] [-U|-D|-L|-R n]
    /// [-A|-a]`: the session's window size (every window of a session is
    /// one size here), which `window-size manual` then keeps; `-A` takes
    /// the largest attached client's size, `-a` the smallest.
    ResizeWindow {
        target: Option<Target>,
        width: Option<u16>,
        height: Option<u16>,
        /// `-U/-D/-L/-R n`: fewer or more rows / columns.
        adjust: Option<(Dir, u16)>,
        largest: bool,
        smallest: bool,
    },
    ResizePane {
        dir: Option<Dir>,
        amount: u16,
        zoom: bool,
        target: Option<Target>,
        /// `-x`/`-y`: an exact width/height in cells, or a percentage of the
        /// window ("50%").
        width: Option<String>,
        height: Option<String>,
    },
    SwapPane {
        up: bool,
        target: Option<Target>,
        /// `-s`: the pair form, tmux style: this pane and `target` change
        /// places (any two panes, even in different windows); `-U`/`-D`
        /// are then not looked at.
        source: Option<Target>,
    },
    /// `select-layout [-n|-p] [-t target] [name]`: rearrange a window's panes
    /// into one of the named layouts (tmux `select-layout`).
    SelectLayout {
        /// A layout name, or None with `next`/`prev` to cycle.
        name: Option<String>,
        next: bool,
        prev: bool,
        /// `-E`: even out the panes beside the target, keeping the layout.
        spread: bool,
        target: Option<Target>,
    },
    BreakPane {
        target: Option<Target>,
    },
    /// `join-pane [-h|-v] [-b] [-s src] [-t dst]` (and `move-pane`, the same
    /// command): take a pane out of its window and split another with it.
    JoinPane {
        src: Option<Target>,
        dst: Option<Target>,
        horizontal: bool,
        before: bool,
    },
    /// `find-window [-t target] pattern`: pick from the windows whose name,
    /// title or visible text matches.
    FindWindow {
        pattern: String,
    },
    SendKeys {
        target: Option<Target>,
        keys: Vec<String>,
        /// `-l`: every argument is literal text, never a key name.
        literal: bool,
    },
    /// `send-keys -X <name> [arg]`: run a copy-mode command by name, so a
    /// binding can drive copy mode (tmux's `-X`).
    CopyCommand {
        target: Option<Target>,
        name: String,
        arg: Option<String>,
    },
    CopyMode {
        page_up: bool,
        /// `-t`: the pane, else the client's current one.
        target: Option<Target>,
    },
    /// `display-panes`: show each pane's number for `display-time`; a digit
    /// pressed while they are up selects that pane.
    DisplayPanes,
    /// `clock-mode [-t]`: a big clock in the pane until a key is pressed.
    ClockMode {
        target: Option<Target>,
    },
    /// `if-shell [-b] [-F] condition command [command]`
    IfShell {
        /// `-F`: the condition is a format string, not a shell command.
        format: bool,
        condition: String,
        then_cmd: Box<Cmd>,
        else_cmd: Option<Box<Cmd>>,
    },
    /// `rotate-window [-D|-U] [-t]`: move every pane one place around the
    /// layout, keeping the layout itself.
    RotateWindow {
        down: bool,
        target: Option<Target>,
    },
    /// `refresh-client`: redraw everything this client shows.
    /// `refresh-client [-U|-D|-L|-R [n]]`: redraw; with a direction, pan
    /// this client's view of a window bigger than the client by `n` cells.
    RefreshClient {
        pan: Option<(Dir, u16)>,
    },
    /// `send-prefix [-t]`: send the prefix key itself to the pane.
    SendPrefix {
        target: Option<Target>,
    },
    /// `list-commands`: the command names this keepane knows.
    ListCommands,
    /// `list-clients`: who is attached, to what, at what size.
    ListClients,
    /// `paste-buffer [-b name] [-t target] [-p]`: send a buffer (or, with no
    /// buffer named, the Windows clipboard) to a pane.
    PasteBuffer {
        name: Option<String>,
        target: Option<Target>,
        /// `-p`: bracketed paste, if the program asked for it.
        bracketed: bool,
    },
    /// `set-buffer [-b name] [-a] data`
    SetBuffer {
        name: Option<String>,
        data: String,
        append: bool,
    },
    /// `load-buffer [-b name] path` / `save-buffer [-b name] path`
    LoadBuffer {
        name: Option<String>,
        path: String,
    },
    SaveBuffer {
        name: Option<String>,
        path: String,
        append: bool,
    },
    ShowBuffer {
        name: Option<String>,
    },
    DeleteBuffer {
        name: Option<String>,
    },
    ListBuffers,
    /// `choose-buffer`: pick a buffer from a list and paste it.
    ChooseBuffer,
    /// `choose-client`: pick an attached client from a list and detach it
    /// (tmux binds this to prefix `D`).
    ChooseClient,
    /// `choose-jobs`: the task board as a picker (prefix `B`): Enter goes
    /// to the pane, `x` kills it, `r` restarts it.
    ChooseJobs,
    /// `choose-history`: what panes printed, kept a file a day (prefix `/`):
    /// a pane position, then a day, opened in `keepane view` in a popup.
    ChooseHistory,
    /// `undo-kill`: the pane or window killed last comes back where it
    /// was, programs still running, within `undo-kill-time` seconds.
    UndoKill,
    /// `focus-pane %N`: every attached client switches to that pane and
    /// its terminal window is brought forward (a notification's button).
    FocusPane {
        pane: u32,
    },
    /// `display-menu [-T title] name key command ...`: a menu over the window;
    /// an empty name is a separator, `key` selects the entry directly.
    DisplayMenu {
        title: Option<String>,
        items: Vec<MenuItem>,
    },
    /// `display-popup [-C] [-E] [-d dir] [-x pos] [-y pos] [-w width]
    /// [-h height] [command]`: a program in a box over the window, with its
    /// own terminal.
    DisplayPopup {
        /// `-C`: close the popup this client has open.
        close: bool,
        /// `-E`: close the popup as soon as the command finishes.
        close_on_exit: bool,
        width: Option<String>,
        height: Option<String>,
        /// `-x` / `-y`: where the box goes (a column or row, a percentage,
        /// `C` for centred, `R`/`B` for the right or bottom edge); centred
        /// when not given.
        x: Option<String>,
        y: Option<String>,
        cwd: Option<String>,
        argv: Vec<String>,
    },
    /// `pipe-pane [-o] [-t target] [command]`: copy everything a pane writes
    /// into the standard input of a shell command. No command stops the pipe.
    PipePane {
        target: Option<Target>,
        command: Option<String>,
        /// `-o`: stop instead of starting if this pane is already piped.
        toggle: bool,
        /// `-I`: what the command prints goes to the pane as input;
        /// `-O`: what the pane prints goes to the command (the default
        /// when neither is given, as in tmux). Both may be set.
        input: bool,
        output: bool,
    },
    /// `web-start [-p port] [-b address] [-r] [-k]`: serve the panes to a
    /// phone from the server, in the background (`keepane web`); when it
    /// already serves and no flag is given, say how it serves.
    WebStart(crate::web::Options),
    /// `web-status`: how the phones' page is served, and who is connected.
    WebStatus,
    /// `web-stop`: stop serving it, the connected phones cut off.
    WebStop,
    /// `link-id`: this server's public key, for another machine to trust
    /// (docs/design/link.md).
    LinkId,
    /// `link-add url`: pair with the machine whose `keepane web` address
    /// (with its key) `url` is, so panes on both can message each other.
    LinkAdd {
        url: String,
    },
    /// `link-trust host:port key [--shell]`: let the machine with that
    /// public key send messages here, by hand (no web key needed).
    LinkTrust {
        addr: String,
        key: String,
        shell: bool,
    },
    /// `link-list`: the machines paired with this server.
    LinkList,
    /// `link-panes host:port`: the panes of that machine.
    LinkPanes {
        addr: String,
    },
    /// `link-allow host:port --shell|--no-shell`: whether that machine's
    /// messages may run as commands in `shell` panes here.
    LinkAllow {
        addr: String,
        shell: bool,
    },
    /// `link-remove host:port`: unpair (both sides, when it can be reached).
    LinkRemove {
        addr: String,
    },
    /// `link-rekey`: a new key pair; every pairing has to be made again.
    LinkRekey,
    /// `link-inbound peer-ip request`: a request another machine made to
    /// the `/link/` paths of `keepane web`, which forwards it whole (as
    /// JSON) since the keys are the server's. The answer is JSON too:
    /// its status, signature and body.
    LinkInbound {
        peer: String,
        request: String,
    },
    /// `wait-for [-L|-S|-U] channel`: block a client until another one signals
    /// (or unlocks) the channel, so scripts can wait for each other.
    WaitFor {
        channel: String,
        lock: bool,
        unlock: bool,
        signal: bool,
    },
    /// `%%` in `template` is replaced by the prompt input; `#S`/`#W` in
    /// `initial` expand to the session/window name.
    CommandPrompt {
        prompt: Option<String>,
        initial: Option<String>,
        template: Option<String>,
    },
    DisplayMessage {
        msg: String,
        /// `-t`: the pane the format is expanded for (default: the client's).
        target: Option<Target>,
    },
    ConfirmBefore {
        prompt: Option<String>,
        cmd: Box<Cmd>,
    },
    BindKey {
        table: KeyTable,
        key: String,
        /// `-r`: the key repeats without the prefix for `repeat-time`.
        repeat: bool,
        cmd: Box<Cmd>,
    },
    UnbindKey {
        table: KeyTable,
        key: String,
    },
    SetOption {
        name: String,
        value: String,
        /// `-a`: add to the option's current value instead of replacing it.
        append: bool,
        /// `-t`: the window a window option (`synchronize-panes`) applies
        /// to; the current one without it.
        target: Option<Target>,
    },
    /// `show-options [-g] [-v] [-q] [name]`: one option, or all of them.
    ShowOptions {
        name: Option<String>,
        value_only: bool,
        quiet: bool,
    },
    SwitchClient {
        next: bool,
        prev: bool,
        /// `-l`: back to the session this client came from.
        last: bool,
        target: Option<Target>,
    },
    /// `show-messages`: the recent status-line messages, newest last.
    ShowMessages,
    /// `set-environment [-r] name [value]`: the environment new panes get.
    SetEnvironment {
        name: String,
        value: Option<String>,
        remove: bool,
    },
    ShowEnvironment {
        name: Option<String>,
    },
    /// `respawn-pane [-k] [-t] [command]` / `respawn-window`: start the
    /// command again in a pane that has exited (or, with `-k`, in a live one).
    RespawnPane {
        target: Option<Target>,
        kill: bool,
        argv: Vec<String>,
        /// Every pane of the window instead of one.
        window: bool,
    },
    ListKeys,
    /// `choose-tree [-s|-w]`: interactive session/window picker (tmux
    /// `choose-tree`); `choose-session` = `-s`, `choose-window` = `-w`.
    ChooseTree {
        /// `-s`: sessions only (collapsed).
        sessions: bool,
        /// `-w`: every session expanded to its windows.
        windows: bool,
    },
    /// `run-shell [-b] [-t target] command`: run a shell command with
    /// `KEEPANE`/`KEEPANE_PANE` set; output is shown (or printed) when it finishes.
    RunShell {
        command: String,
        background: bool,
        target: Option<Target>,
    },
    /// `set-hook [-g] hook command` / `set-hook -u hook`.
    SetHook {
        hook: String,
        cmd: Option<Box<Cmd>>,
    },
    ShowHooks,
    /// `load-plugin name-or-path`: source `<dir>/<name>.keepane` (or `plugin.keepane`).
    LoadPlugin {
        path: String,
    },
    ListPlugins,
    /// `save-session [-t target]`: write one session (or, with `-a`, all).
    SaveSession {
        target: Option<Target>,
        all: bool,
    },
    /// `restore-session [-a] [name]` / `resume [name]`: recreate a saved
    /// session (or every saved one); `-a` attaches to it afterwards.
    RestoreSession {
        name: Option<String>,
        attach: bool,
    },
    /// `list-saved`: saved sessions, newest first.
    ListSaved,
    /// `start-server`: the server is already running by the time anything
    /// reads this, so it only has to be accepted.
    StartServer,
    /// `notify [-T title] message`: a desktop notification, so a script can
    /// reach you when the terminal is not on screen.
    Notify {
        title: Option<String>,
        message: String,
    },
    /// `record [-t target] [path]`: write a pane's output to an asciinema
    /// file from now on; no path stops the recording.
    Record {
        target: Option<Target>,
        path: Option<String>,
    },
    /// `find-text [-C] [-n hits] [-t target] pattern`: look through what
    /// every pane has printed, not just the window names, and say where the
    /// matches are.
    FindText {
        pattern: String,
        /// Limit to a session, or to one window of it.
        target: Option<Target>,
        /// `-C`: match case, which is otherwise ignored.
        case_sensitive: bool,
        /// `-n`: how many hits to report per pane.
        per_pane: usize,
    },
    /// `jobs`: every pane on the server as one line: where it is, whether
    /// its program is still running, for how long, when it last printed.
    Jobs {
        /// Limit to a session (or a window of it).
        target: Option<Target>,
        /// `-F`: a format instead of the standard columns.
        format: Option<String>,
    },
    /// `delete-saved name`: forget a saved session.
    DeleteSaved {
        name: String,
    },
    ClearHistory,
    /// `capture-pane -p`: print the visible text of a pane (`-S -N` adds N
    /// lines of scrollback above it).
    CapturePane {
        target: Option<Target>,
        history: usize,
        /// `-e`: keep the colours and attributes as escape sequences.
        escapes: bool,
        /// `-J`: join the rows of a wrapped line back into one line.
        join: bool,
    },
    /// `set-cwd [-t target] [dir]`: record a pane's working directory (used
    /// by save/resume and `#{pane_current_path}`); no `dir` means the
    /// calling client's current directory.
    SetCwd {
        target: Option<Target>,
        dir: Option<String>,
    },
    SourceFile {
        path: String,
    },
    Version,
}

impl fmt::Display for Cmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cmd::NewSession { name, window_name, cwd, detached, argv, attach_existing, size } => {
                f.write_str("new-session")?;
                if let Some(n) = name {
                    write!(f, " -s {}", quote(n))?;
                }
                if let Some(n) = window_name {
                    write!(f, " -n {}", quote(n))?;
                }
                if let Some(c) = cwd {
                    write!(f, " -c {}", quote(c))?;
                }
                if *detached {
                    f.write_str(" -d")?;
                }
                if *attach_existing {
                    f.write_str(" -A")?;
                }
                if let Some(x) = size.0 {
                    write!(f, " -x {x}")?;
                }
                if let Some(y) = size.1 {
                    write!(f, " -y {y}")?;
                }
                for a in argv {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::AttachSession { target, detach_others } => {
                f.write_str("attach-session")?;
                if *detach_others {
                    f.write_str(" -d")?;
                }
                fmt_target(f, target)
            }
            Cmd::DetachClient { all, target } => {
                f.write_str("detach-client")?;
                if *all {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::ShowMessages => f.write_str("show-messages"),
            Cmd::SetEnvironment { name, value, remove } => {
                f.write_str("set-environment -g")?;
                if *remove {
                    f.write_str(" -r")?;
                }
                write!(f, " {}", quote(name))?;
                if let Some(v) = value {
                    write!(f, " {}", quote(v))?;
                }
                Ok(())
            }
            Cmd::ShowEnvironment { name } => {
                f.write_str("show-environment -g")?;
                if let Some(n) = name {
                    write!(f, " {}", quote(n))?;
                }
                Ok(())
            }
            Cmd::RespawnPane { target, kill, argv, window } => {
                f.write_str(if *window { "respawn-window" } else { "respawn-pane" })?;
                if *kill {
                    f.write_str(" -k")?;
                }
                fmt_target(f, target)?;
                for a in argv {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::ListSessions => f.write_str("list-sessions"),
            Cmd::ListMarks { target } => {
                f.write_str("list-marks")?;
                fmt_target(f, target)
            }
            Cmd::SendMessage { target, reply, re, task, wait, text } => {
                f.write_str("send-message")?;
                fmt_target(f, target)?;
                if *reply {
                    f.write_str(" -r")?;
                }
                if let Some(r) = re {
                    write!(f, " --re {r}")?;
                }
                if let Some(t) = task {
                    write!(f, " --task {t}")?;
                }
                if let Some(w) = wait {
                    write!(f, " -w {w}")?;
                }
                write!(f, " -- {}", quote(text))
            }
            Cmd::ReadMessage { target, wait } => {
                f.write_str("read-message")?;
                fmt_target(f, target)?;
                if let Some(w) = wait {
                    write!(f, " -w {w}")?;
                }
                Ok(())
            }
            Cmd::ListMessages { target, all } => {
                f.write_str("list-messages")?;
                fmt_target(f, target)?;
                if *all {
                    f.write_str(" -a")?;
                }
                Ok(())
            }
            Cmd::TraceMessage { id, wait } => {
                write!(f, "trace-message {id}")?;
                if let Some(w) = wait {
                    write!(f, " -w {w}")?;
                }
                Ok(())
            }
            Cmd::DropMessage { id, undo } => {
                f.write_str("drop-message")?;
                if *undo {
                    f.write_str(" -u")?;
                }
                if let Some(id) = id {
                    write!(f, " {id}")?;
                }
                Ok(())
            }
            Cmd::MoveMessage { id, to } => write!(f, "move-message {id} {}", to.as_str()),
            Cmd::PaneReady { quiet, target } => {
                f.write_str("pane-ready")?;
                if *quiet {
                    f.write_str(" -q")?;
                }
                fmt_target(f, target)
            }
            Cmd::PaneStatus { text } => write!(f, "pane-status -- {}", quote(text)),
            Cmd::SetWorkMode { target, mode } => {
                f.write_str("set-work-mode")?;
                fmt_target(f, target)?;
                write!(f, " {mode}")
            }
            Cmd::RenamePane { target, name } => {
                f.write_str("rename-pane")?;
                fmt_target(f, target)?;
                write!(f, " -- {}", quote(name))
            }
            Cmd::Whoami => f.write_str("whoami"),
            Cmd::CreatePane(c) => {
                let CreatePane { kind, target, session_name, horizontal, cwd, name, mode, message, argv } = &**c;
                write!(f, "create-pane -k {kind}")?;
                fmt_target(f, target)?;
                if let Some(s) = session_name {
                    write!(f, " -s {}", quote(s))?;
                }
                if *horizontal {
                    f.write_str(" -h")?;
                }
                if let Some(c) = cwd {
                    write!(f, " -c {}", quote(c))?;
                }
                if let Some(n) = name {
                    write!(f, " -n {}", quote(n))?;
                }
                if let Some(m) = mode {
                    write!(f, " -m {m}")?;
                }
                if let Some(m) = message {
                    write!(f, " -M {}", quote(m))?;
                }
                if !argv.is_empty() {
                    f.write_str(" --")?;
                    for a in argv {
                        write!(f, " {}", quote(a))?;
                    }
                }
                Ok(())
            }
            Cmd::Dashboard => f.write_str("dashboard"),
            Cmd::ListTasks { target } => {
                f.write_str("list-tasks")?;
                fmt_target(f, target)
            }
            Cmd::ShowTask { id } => write!(f, "show-task {id}"),
            Cmd::ListEvents { target, since, last } => {
                f.write_str("list-events")?;
                fmt_target(f, target)?;
                if let Some(s) = since {
                    write!(f, " -S {s}s")?;
                }
                if let Some(n) = last {
                    write!(f, " -n {n}")?;
                }
                Ok(())
            }
            Cmd::ListWindows { target } => {
                f.write_str("list-windows")?;
                fmt_target(f, target)
            }
            Cmd::ListPanes { target, all, session, format } => {
                f.write_str("list-panes")?;
                if *all {
                    f.write_str(" -a")?;
                } else if *session {
                    f.write_str(" -s")?;
                }
                if let Some(fmt) = format {
                    write!(f, " -F {}", quote(fmt))?;
                }
                fmt_target(f, target)
            }
            Cmd::KillSession { target, all_but } => {
                f.write_str("kill-session")?;
                if *all_but {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::KillServer { restarting } => f.write_str(if *restarting { "kill-server -r" } else { "kill-server" }),
            Cmd::HasSession { target } => {
                f.write_str("has-session")?;
                fmt_target(f, &Some(target.clone()))
            }
            Cmd::RenameSession { target, name } => {
                f.write_str("rename-session")?;
                fmt_target(f, target)?;
                write!(f, " {}", quote(name))
            }
            Cmd::NewWindow { name, cwd, target, argv, detached } => {
                f.write_str("new-window")?;
                if *detached {
                    f.write_str(" -d")?;
                }
                if let Some(n) = name {
                    write!(f, " -n {}", quote(n))?;
                }
                if let Some(c) = cwd {
                    write!(f, " -c {}", quote(c))?;
                }
                fmt_target(f, target)?;
                for a in argv {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::KillWindow { target, all_but } => {
                f.write_str("kill-window")?;
                if *all_but {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::RenameWindow { target, name } => {
                f.write_str("rename-window")?;
                fmt_target(f, target)?;
                write!(f, " {}", quote(name))
            }
            Cmd::SelectWindow { target } => {
                f.write_str("select-window")?;
                fmt_target(f, &Some(target.clone()))
            }
            Cmd::SwapWindow { src, dst, move_it } => {
                f.write_str(if *move_it { "move-window" } else { "swap-window" })?;
                if let Some(s) = src {
                    f.write_str(" -s")?;
                    fmt_target(f, &Some(s.clone()))?;
                }
                if let Some(d) = dst {
                    f.write_str(" -t")?;
                    fmt_target(f, &Some(d.clone()))?;
                }
                Ok(())
            }
            Cmd::NextWindow { target, alert } => {
                f.write_str("next-window")?;
                if *alert {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::PreviousWindow { target, alert } => {
                f.write_str("previous-window")?;
                if *alert {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::LastWindow { target } => {
                f.write_str("last-window")?;
                fmt_target(f, target)
            }
            Cmd::SplitWindow { horizontal, cwd, target, argv, detached, before, full, count } => {
                f.write_str("split-window")?;
                f.write_str(if *horizontal { " -h" } else { " -v" })?;
                if *count > 1 {
                    write!(f, " -N {count}")?;
                }
                if *detached {
                    f.write_str(" -d")?;
                }
                if *before {
                    f.write_str(" -b")?;
                }
                if *full {
                    f.write_str(" -f")?;
                }
                if let Some(c) = cwd {
                    write!(f, " -c {}", quote(c))?;
                }
                fmt_target(f, target)?;
                for a in argv {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::KillPane { target, all_but } => {
                f.write_str("kill-pane")?;
                if *all_but {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::SelectPane { sel } => {
                f.write_str("select-pane")?;
                match sel {
                    PaneSel::Dir(Dir::Left) => f.write_str(" -L"),
                    PaneSel::Dir(Dir::Right) => f.write_str(" -R"),
                    PaneSel::Dir(Dir::Up) => f.write_str(" -U"),
                    PaneSel::Dir(Dir::Down) => f.write_str(" -D"),
                    PaneSel::Next => f.write_str(" -t next"),
                    PaneSel::Prev => f.write_str(" -t prev"),
                    PaneSel::Last => f.write_str(" -l"),
                    PaneSel::Index(i) => write!(f, " -t {i}"),
                    PaneSel::Target(t) => fmt_target(f, &Some(t.clone())),
                }
            }
            Cmd::PaneTitle { target, title, mark, unmark } => {
                f.write_str("select-pane")?;
                if *mark {
                    f.write_str(" -m")?;
                }
                if *unmark {
                    f.write_str(" -M")?;
                }
                fmt_target(f, target)?;
                if let Some(t) = title {
                    write!(f, " -T {}", quote(t))?;
                }
                Ok(())
            }
            Cmd::ResizeWindow { target, width, height, adjust, largest, smallest } => {
                f.write_str("resize-window")?;
                if *largest {
                    f.write_str(" -A")?;
                }
                if *smallest {
                    f.write_str(" -a")?;
                }
                if let Some((dir, n)) = adjust {
                    let flag = match dir {
                        Dir::Up => "-U",
                        Dir::Down => "-D",
                        Dir::Left => "-L",
                        Dir::Right => "-R",
                    };
                    write!(f, " {flag} {n}")?;
                }
                if let Some(w) = width {
                    write!(f, " -x {w}")?;
                }
                if let Some(h) = height {
                    write!(f, " -y {h}")?;
                }
                fmt_target(f, target)
            }
            Cmd::ResizePane { dir, amount, zoom, target, width, height } => {
                f.write_str("resize-pane")?;
                if *zoom {
                    f.write_str(" -Z")?;
                }
                if let Some(x) = width {
                    write!(f, " -x {x}")?;
                }
                if let Some(y) = height {
                    write!(f, " -y {y}")?;
                }
                match dir {
                    Some(Dir::Left) => write!(f, " -L {amount}")?,
                    Some(Dir::Right) => write!(f, " -R {amount}")?,
                    Some(Dir::Up) => write!(f, " -U {amount}")?,
                    Some(Dir::Down) => write!(f, " -D {amount}")?,
                    None => {}
                }
                fmt_target(f, target)
            }
            Cmd::SwapPane { up, target, source } => {
                f.write_str("swap-pane")?;
                match source {
                    Some(s) => write!(f, " -s {}", quote(&target_string(s)))?,
                    None => write!(f, " {}", if *up { "-U" } else { "-D" })?,
                }
                fmt_target(f, target)
            }
            Cmd::SelectLayout { name, next, prev, spread, target } => {
                f.write_str("select-layout")?;
                if *next {
                    f.write_str(" -n")?;
                }
                if *prev {
                    f.write_str(" -p")?;
                }
                if *spread {
                    f.write_str(" -E")?;
                }
                fmt_target(f, target)?;
                if let Some(n) = name {
                    write!(f, " {}", quote(n))?;
                }
                Ok(())
            }
            Cmd::BreakPane { target } => {
                f.write_str("break-pane")?;
                fmt_target(f, target)
            }
            Cmd::JoinPane { src, dst, horizontal, before } => {
                f.write_str("join-pane")?;
                f.write_str(if *horizontal { " -h" } else { " -v" })?;
                if *before {
                    f.write_str(" -b")?;
                }
                if let Some(s) = src {
                    f.write_str(" -s")?;
                    fmt_target(f, &Some(s.clone()))?;
                }
                if let Some(d) = dst {
                    f.write_str(" -t")?;
                    fmt_target(f, &Some(d.clone()))?;
                }
                Ok(())
            }
            Cmd::FindWindow { pattern } => write!(f, "find-window {}", quote(pattern)),
            Cmd::SendKeys { target, keys, literal } => {
                f.write_str("send-keys")?;
                if *literal {
                    f.write_str(" -l")?;
                }
                fmt_target(f, target)?;
                for k in keys {
                    write!(f, " {}", quote(k))?;
                }
                Ok(())
            }
            Cmd::CopyCommand { target, name, arg } => {
                f.write_str("send-keys -X")?;
                fmt_target(f, target)?;
                write!(f, " {}", quote(name))?;
                if let Some(a) = arg {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::CopyMode { page_up, target } => {
                f.write_str(if *page_up { "copy-mode -u" } else { "copy-mode" })?;
                fmt_target(f, target)
            }
            Cmd::DisplayPanes => f.write_str("display-panes"),
            Cmd::ClockMode { target } => {
                f.write_str("clock-mode")?;
                fmt_target(f, target)
            }
            Cmd::IfShell { format, condition, then_cmd, else_cmd } => {
                f.write_str("if-shell")?;
                if *format {
                    f.write_str(" -F")?;
                }
                write!(f, " {} {}", quote(condition), quote(&then_cmd.to_string()))?;
                if let Some(e) = else_cmd {
                    write!(f, " {}", quote(&e.to_string()))?;
                }
                Ok(())
            }
            Cmd::RotateWindow { down, target } => {
                write!(f, "rotate-window {}", if *down { "-D" } else { "-U" })?;
                fmt_target(f, target)
            }
            Cmd::RefreshClient { pan } => {
                f.write_str("refresh-client")?;
                if let Some((dir, n)) = pan {
                    let flag = match dir {
                        Dir::Up => "-U",
                        Dir::Down => "-D",
                        Dir::Left => "-L",
                        Dir::Right => "-R",
                    };
                    write!(f, " {flag} {n}")?;
                }
                Ok(())
            }
            Cmd::SendPrefix { target } => {
                f.write_str("send-prefix")?;
                fmt_target(f, target)
            }
            Cmd::ListCommands => f.write_str("list-commands"),
            Cmd::ListClients => f.write_str("list-clients"),
            Cmd::PasteBuffer { name, target, bracketed } => {
                f.write_str("paste-buffer")?;
                if *bracketed {
                    f.write_str(" -p")?;
                }
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                fmt_target(f, target)
            }
            Cmd::SetBuffer { name, data, append } => {
                f.write_str("set-buffer")?;
                if *append {
                    f.write_str(" -a")?;
                }
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                write!(f, " {}", quote(data))
            }
            Cmd::LoadBuffer { name, path } => {
                f.write_str("load-buffer")?;
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                write!(f, " {}", quote(path))
            }
            Cmd::SaveBuffer { name, path, append } => {
                f.write_str("save-buffer")?;
                if *append {
                    f.write_str(" -a")?;
                }
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                write!(f, " {}", quote(path))
            }
            Cmd::ShowBuffer { name } => {
                f.write_str("show-buffer")?;
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                Ok(())
            }
            Cmd::DeleteBuffer { name } => {
                f.write_str("delete-buffer")?;
                if let Some(b) = name {
                    write!(f, " -b {}", quote(b))?;
                }
                Ok(())
            }
            Cmd::ListBuffers => f.write_str("list-buffers"),
            Cmd::ChooseBuffer => f.write_str("choose-buffer"),
            Cmd::ChooseClient => f.write_str("choose-client"),
            Cmd::ChooseJobs => f.write_str("choose-jobs"),
            Cmd::ChooseHistory => f.write_str("choose-history"),
            Cmd::UndoKill => f.write_str("undo-kill"),
            Cmd::FocusPane { pane } => write!(f, "focus-pane %{pane}"),
            Cmd::DisplayMenu { title, items } => {
                f.write_str("display-menu")?;
                if let Some(t) = title {
                    write!(f, " -T {}", quote(t))?;
                }
                for it in items {
                    write!(f, " {}", quote(&it.name))?;
                    match (&it.key, &it.cmd) {
                        (Some(k), Some(c)) => write!(f, " {} {}", quote(k), quote(&c.to_string()))?,
                        (Some(k), None) => write!(f, " {} {}", quote(k), quote(""))?,
                        _ => {}
                    }
                }
                Ok(())
            }
            Cmd::DisplayPopup { close, close_on_exit, width, height, x, y, cwd, argv } => {
                f.write_str("display-popup")?;
                if *close {
                    f.write_str(" -C")?;
                }
                if *close_on_exit {
                    f.write_str(" -E")?;
                }
                if let Some(w) = width {
                    write!(f, " -w {}", quote(w))?;
                }
                if let Some(h) = height {
                    write!(f, " -h {}", quote(h))?;
                }
                if let Some(x) = x {
                    write!(f, " -x {}", quote(x))?;
                }
                if let Some(y) = y {
                    write!(f, " -y {}", quote(y))?;
                }
                if let Some(d) = cwd {
                    write!(f, " -d {}", quote(d))?;
                }
                for a in argv {
                    write!(f, " {}", quote(a))?;
                }
                Ok(())
            }
            Cmd::PipePane { target, command, toggle, input, output } => {
                f.write_str("pipe-pane")?;
                if *toggle {
                    f.write_str(" -o")?;
                }
                if *input {
                    f.write_str(" -I")?;
                }
                if *output && *input {
                    f.write_str(" -O")?; // -O alone is the default and is left out
                }
                fmt_target(f, target)?;
                if let Some(c) = command {
                    write!(f, " {}", quote(c))?;
                }
                Ok(())
            }
            Cmd::WebStart(o) => {
                f.write_str("web-start")?;
                for w in o.flags() {
                    write!(f, " {w}")?;
                }
                Ok(())
            }
            Cmd::WebStatus => f.write_str("web-status"),
            Cmd::WebStop => f.write_str("web-stop"),
            Cmd::LinkId => f.write_str("link-id"),
            Cmd::LinkAdd { url } => write!(f, "link-add {}", quote(url)),
            Cmd::LinkTrust { addr, key, shell } => {
                write!(f, "link-trust {} {}", quote(addr), quote(key))?;
                if *shell {
                    f.write_str(" --shell")?;
                }
                Ok(())
            }
            Cmd::LinkList => f.write_str("link-list"),
            Cmd::LinkPanes { addr } => write!(f, "link-panes {}", quote(addr)),
            Cmd::LinkAllow { addr, shell } => {
                write!(f, "link-allow {} {}", quote(addr), if *shell { "--shell" } else { "--no-shell" })
            }
            Cmd::LinkRemove { addr } => write!(f, "link-remove {}", quote(addr)),
            Cmd::LinkRekey => f.write_str("link-rekey"),
            Cmd::LinkInbound { peer, request } => write!(f, "link-inbound {} {}", quote(peer), quote(request)),
            Cmd::WaitFor { channel, lock, unlock, signal } => {
                f.write_str("wait-for")?;
                if *lock {
                    f.write_str(" -L")?;
                }
                if *unlock {
                    f.write_str(" -U")?;
                }
                if *signal {
                    f.write_str(" -S")?;
                }
                write!(f, " {}", quote(channel))
            }
            Cmd::CommandPrompt { prompt, initial, template } => {
                f.write_str("command-prompt")?;
                if let Some(p) = prompt {
                    write!(f, " -p {}", quote(p))?;
                }
                if let Some(i) = initial {
                    write!(f, " -I {}", quote(i))?;
                }
                if let Some(t) = template {
                    write!(f, " {}", quote(t))?;
                }
                Ok(())
            }
            Cmd::DisplayMessage { msg, target } => {
                f.write_str("display-message")?;
                fmt_target(f, target)?;
                write!(f, " {}", quote(msg))
            }
            Cmd::ConfirmBefore { prompt, cmd } => {
                f.write_str("confirm-before")?;
                if let Some(p) = prompt {
                    write!(f, " -p {}", quote(p))?;
                }
                write!(f, " {}", quote(&cmd.to_string()))
            }
            Cmd::BindKey { table, key, repeat, cmd } => {
                f.write_str("bind-key")?;
                f.write_str(table.flag())?;
                if *repeat {
                    f.write_str(" -r")?;
                }
                write!(f, " {} {}", quote(key), cmd)
            }
            Cmd::UnbindKey { table, key } => {
                f.write_str("unbind-key")?;
                f.write_str(table.flag())?;
                write!(f, " {}", quote(key))
            }
            Cmd::SetOption { name, value, append, target } => {
                write!(f, "set-option{}", if *append { " -a" } else { "" })?;
                fmt_target(f, target)?;
                write!(f, " {} {}", quote(name), quote(value))
            }
            Cmd::SwitchClient { next, prev, last, target } => {
                f.write_str("switch-client")?;
                if *next {
                    f.write_str(" -n")?;
                }
                if *prev {
                    f.write_str(" -p")?;
                }
                if *last {
                    f.write_str(" -l")?;
                }
                fmt_target(f, target)
            }
            Cmd::ListKeys => f.write_str("list-keys"),
            Cmd::ChooseTree { sessions, windows } => {
                f.write_str("choose-tree")?;
                if *sessions {
                    f.write_str(" -s")?;
                }
                if *windows {
                    f.write_str(" -w")?;
                }
                Ok(())
            }
            Cmd::ShowOptions { name, value_only, quiet } => {
                f.write_str("show-options -g")?;
                if *value_only {
                    f.write_str(" -v")?;
                }
                if *quiet {
                    f.write_str(" -q")?;
                }
                if let Some(n) = name {
                    write!(f, " {}", quote(n))?;
                }
                Ok(())
            }
            Cmd::RunShell { command, background, target } => {
                f.write_str("run-shell")?;
                if *background {
                    f.write_str(" -b")?;
                }
                fmt_target(f, target)?;
                write!(f, " {}", quote(command))
            }
            Cmd::SetHook { hook, cmd } => match cmd {
                Some(c) => write!(f, "set-hook -g {} {}", quote(hook), quote(&c.to_string())),
                None => write!(f, "set-hook -gu {}", quote(hook)),
            },
            Cmd::ShowHooks => f.write_str("show-hooks -g"),
            Cmd::LoadPlugin { path } => write!(f, "load-plugin {}", quote(path)),
            Cmd::ListPlugins => f.write_str("list-plugins"),
            Cmd::SaveSession { target, all } => {
                f.write_str("save-session")?;
                if *all {
                    f.write_str(" -a")?;
                }
                fmt_target(f, target)
            }
            Cmd::RestoreSession { name, attach } => {
                f.write_str("restore-session")?;
                if *attach {
                    f.write_str(" -a")?;
                }
                if let Some(n) = name {
                    write!(f, " {}", quote(n))?;
                }
                Ok(())
            }
            Cmd::ListSaved => f.write_str("list-saved"),
            Cmd::DeleteSaved { name } => write!(f, "delete-saved {}", quote(name)),
            Cmd::StartServer => f.write_str("start-server"),
            Cmd::Notify { title, message } => {
                f.write_str("notify")?;
                if let Some(t) = title {
                    write!(f, " -T {}", quote(t))?;
                }
                write!(f, " {}", quote(message))
            }
            Cmd::Record { target, path } => {
                f.write_str("record")?;
                fmt_target(f, target)?;
                if let Some(p) = path {
                    write!(f, " {}", quote(p))?;
                }
                Ok(())
            }
            Cmd::FindText { pattern, target, case_sensitive, per_pane } => {
                f.write_str("find-text")?;
                if *case_sensitive {
                    f.write_str(" -C")?;
                }
                if *per_pane != 3 {
                    write!(f, " -n {per_pane}")?;
                }
                fmt_target(f, target)?;
                write!(f, " {}", quote(pattern))
            }
            Cmd::Jobs { target, format } => {
                f.write_str("jobs")?;
                if let Some(fm) = format {
                    write!(f, " -F {}", quote(fm))?;
                }
                fmt_target(f, target)
            }
            Cmd::ClearHistory => f.write_str("clear-history"),
            Cmd::SetCwd { target, dir } => {
                f.write_str("set-cwd")?;
                fmt_target(f, target)?;
                if let Some(d) = dir {
                    write!(f, " {}", quote(d))?;
                }
                Ok(())
            }
            Cmd::CapturePane { target, history, escapes, join } => {
                f.write_str("capture-pane -p")?;
                if *escapes {
                    f.write_str(" -e")?;
                }
                if *join {
                    f.write_str(" -J")?;
                }
                if *history > 0 {
                    write!(f, " -S -{history}")?;
                }
                fmt_target(f, target)
            }
            Cmd::SourceFile { path } => write!(f, "source-file {}", quote(path)),
            Cmd::Version => f.write_str("version"),
        }
    }
}

/// A target as `session:window.pane`, the way `-t` takes it back.
fn target_string(t: &Target) -> String {
    let pane = t.pane_id.map(|id| format!("%{id}")).or_else(|| t.pane_name.as_ref().map(|n| format!("%{n}")));
    let mut s = t.remote.as_ref().map(|r| format!("{r}/")).unwrap_or_default();
    if let Some(p) = &pane
        && t.session.is_none()
        && t.window.is_none()
    {
        s.push_str(p);
        return s;
    }
    s.push_str(t.session.as_deref().unwrap_or_default());
    if let Some(w) = &t.window {
        s.push(':');
        s.push_str(w);
    } else if t.pane.is_some() || pane.is_some() {
        s.push(':');
    }
    if let Some(p) = pane {
        s.push('.');
        s.push_str(&p);
    } else if let Some(p) = t.pane {
        s.push('.');
        s.push_str(&p.to_string());
    }
    s
}

fn fmt_target(f: &mut fmt::Formatter<'_>, t: &Option<Target>) -> fmt::Result {
    if let Some(t) = t {
        write!(f, " -t {}", quote(&target_string(t)))?;
    }
    Ok(())
}

/// Quote a word for the command language if it needs it.
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && !s.starts_with('#')
        && s.chars().all(|c| !c.is_whitespace() && c != '"' && c != '\'' && c != '\\' && c != ';')
    {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Split a command line into words, honouring double/single quotes and
/// backslash escapes. A `#` at the start of a word begins a comment.
///
/// Backslashes are only escapes before a quote, another backslash,
/// whitespace, `#` or `;`, so Windows paths like `C:\src` survive unquoted.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '#' if !in_word => break,
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err("unterminated double quote".into()),
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some(&e @ ('"' | '\\')) => {
                                chars.next();
                                cur.push(e);
                            }
                            _ => cur.push('\\'),
                        },
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err("unterminated single quote".into()),
                        Some('\'') => break,
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.peek() {
                    Some(&e) if e == '"' || e == '\'' || e == '\\' || e == '#' || e == ';' || e.is_whitespace() => {
                        chars.next();
                        cur.push(e);
                    }
                    _ => cur.push('\\'),
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

struct Args<'a> {
    words: &'a [String],
    pos: usize,
}

impl<'a> Args<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.words.get(self.pos).map(String::as_str)
    }
    fn next(&mut self) -> Option<&'a str> {
        let w = self.peek();
        if w.is_some() {
            self.pos += 1;
        }
        w
    }
    fn rest(&mut self) -> Vec<String> {
        let r = self.words[self.pos..].to_vec();
        self.pos = self.words.len();
        r
    }
    /// True if the next word is a flag. A bare `--` ends flag parsing and is
    /// consumed, so `rename-window -- -x` names the window `-x`.
    fn is_flag(&mut self) -> bool {
        match self.peek() {
            Some("--") => {
                self.pos += 1;
                false
            }
            Some(w) => w.len() > 1 && w.starts_with('-'),
            None => false,
        }
    }
    fn value(&mut self, flag: &str) -> Result<&'a str, String> {
        self.next().ok_or_else(|| format!("{flag}: missing value"))
    }
    fn none_flags(&mut self, name: &str) -> Result<(), String> {
        if self.is_flag() { Err(format!("{name}: unknown flag '{}'", self.words[self.pos])) } else { Ok(()) }
    }
    fn none_left(&self, name: &str) -> Result<(), String> {
        if self.pos < self.words.len() {
            Err(format!("{name}: unexpected argument '{}'", self.words[self.pos]))
        } else {
            Ok(())
        }
    }
}

fn bad_flag(name: &str, flag: &str) -> String {
    format!("{name}: unknown flag '{flag}'")
}

fn seconds(name: &str, v: &str) -> Result<u64, String> {
    v.parse().map_err(|_| format!("{name}: bad number of seconds '{v}'"))
}

/// A span of time: `90`, `90s`, `15m`, `2h`, `3d`, in seconds.
fn duration(name: &str, v: &str) -> Result<u64, String> {
    let (digits, mult) = match v.chars().last() {
        Some('s') => (&v[..v.len() - 1], 1),
        Some('m') => (&v[..v.len() - 1], 60),
        Some('h') => (&v[..v.len() - 1], 3600),
        Some('d') => (&v[..v.len() - 1], 86_400),
        _ => (v, 1),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .ok_or_else(|| format!("{name}: bad time span '{v}' (like 90s, 15m, 2h, 3d)"))
}

/// A message id as `send-message` printed it: `12` or `#12`.
fn message_id(name: &str, v: &str) -> Result<u64, String> {
    v.trim_start_matches('#').parse().map_err(|_| format!("{name}: bad message id '{v}'"))
}

/// The key tables: the keys after the prefix, keys without it, and the
/// keys of copy mode (tmux's `copy-mode-vi`; `copy-mode`, its emacs table,
/// is taken as the same one, keepane's copy mode being vi-style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyTable {
    Prefix,
    Root,
    Copy,
}

impl KeyTable {
    /// How `list-keys` and a printed command spell the table.
    pub fn flag(self) -> &'static str {
        match self {
            KeyTable::Prefix => "",
            KeyTable::Root => " -n",
            KeyTable::Copy => " -T copy-mode-vi",
        }
    }
}

/// `-T table` for bind-key / unbind-key. An unknown table is refused, not
/// quietly bound under the prefix.
fn key_table(name: &str, table: &str) -> Result<KeyTable, String> {
    match table {
        "root" => Ok(KeyTable::Root),
        "prefix" => Ok(KeyTable::Prefix),
        "copy-mode-vi" | "copy-mode" => Ok(KeyTable::Copy),
        other => {
            Err(format!("{name}: key table '{other}' is not supported (keepane has root, prefix and copy-mode-vi)"))
        }
    }
}

/// Every command name, for the unambiguous-prefix lookup below.
pub const COMMANDS: &[&str] = &[
    "attach-session",
    "break-pane",
    "bind-key",
    "capture-pane",
    "choose-buffer",
    "choose-client",
    "choose-history",
    "choose-jobs",
    "choose-session",
    "clock-mode",
    "choose-tree",
    "choose-window",
    "clear-history",
    "command-prompt",
    "copy-mode",
    "confirm-before",
    "delete-buffer",
    "delete-saved",
    "detach-client",
    "display-menu",
    "display-message",
    "display-popup",
    "display-panes",
    "find-text",
    "find-window",
    "focus-pane",
    "has-session",
    "if-shell",
    "jobs",
    "join-pane",
    "kill-pane",
    "kill-server",
    "kill-session",
    "kill-window",
    "last-pane",
    "last-window",
    "link-add",
    "link-allow",
    "link-id",
    "link-inbound",
    "link-list",
    "link-panes",
    "link-rekey",
    "link-remove",
    "link-trust",
    "list-buffers",
    "list-clients",
    "list-commands",
    "list-keys",
    "list-marks",
    "send-message",
    "read-message",
    "list-messages",
    "trace-message",
    "drop-message",
    "move-message",
    "pane-ready",
    "pane-status",
    "set-work-mode",
    "rename-pane",
    "whoami",
    "list-tasks",
    "dashboard",
    "create-pane",
    "show-task",
    "list-events",
    "list-panes",
    "list-plugins",
    "list-saved",
    "list-sessions",
    "list-windows",
    "load-buffer",
    "load-plugin",
    "move-pane",
    "move-window",
    "new-session",
    "new-window",
    "next-layout",
    "next-window",
    "notify",
    "paste-buffer",
    "pipe-pane",
    "previous-layout",
    "previous-window",
    "record",
    "refresh-client",
    "respawn-pane",
    "respawn-window",
    "rotate-window",
    "rename-session",
    "rename-window",
    "resize-pane",
    "resize-window",
    "restore-session",
    "resume",
    "run-shell",
    "save-buffer",
    "save-session",
    "select-layout",
    "select-pane",
    "select-window",
    "send-keys",
    "send-prefix",
    "set-buffer",
    "set-cwd",
    "set-environment",
    "set-hook",
    "set-option",
    "set-window-option",
    "show-buffer",
    "show-environment",
    "show-hooks",
    "show-messages",
    "show-options",
    "show-window-options",
    "source-file",
    "split-window",
    "start-server",
    "swap-pane",
    "swap-window",
    "switch-client",
    "unbind-key",
    "undo-kill",
    "version",
    "wait-for",
    "web-start",
    "web-status",
    "web-stop",
];

/// The flags each command takes, for completion (the shell's and the `:`
/// prompt's). A test checks every entry against the parser, so a flag
/// added there without being listed here, or listed here without being
/// taken there, fails the build's tests rather than misleading a Tab.
pub const FLAGS: &[(&str, &[&str])] = &[
    ("attach-session", &["-d", "-t"]),
    ("break-pane", &["-t"]),
    ("bind-key", &["-n", "-r", "-T"]),
    ("capture-pane", &["-p", "-e", "-J", "-S", "-t"]),
    ("choose-buffer", &[]),
    ("choose-client", &[]),
    ("choose-history", &[]),
    ("choose-jobs", &[]),
    ("choose-session", &["-s", "-w", "-Z"]),
    ("clock-mode", &["-t"]),
    ("choose-tree", &["-s", "-w", "-Z"]),
    ("choose-window", &["-s", "-w", "-Z"]),
    ("clear-history", &["-t"]),
    ("command-prompt", &["-p", "-I"]),
    ("copy-mode", &["-u", "-t"]),
    ("confirm-before", &["-p"]),
    ("delete-buffer", &["-b"]),
    ("delete-saved", &[]),
    ("detach-client", &["-a", "-t"]),
    ("display-menu", &["-T"]),
    ("display-message", &["-p", "-t"]),
    ("display-popup", &["-C", "-E", "-w", "-h", "-x", "-y", "-d"]),
    ("display-panes", &["-d"]),
    ("find-text", &["-t"]),
    ("find-window", &["-t"]),
    ("focus-pane", &[]),
    ("has-session", &["-t"]),
    ("if-shell", &["-b", "-F", "-t"]),
    ("jobs", &["-t", "-F"]),
    ("join-pane", &["-h", "-v", "-b", "-d", "-s", "-t"]),
    ("kill-pane", &["-a", "-t"]),
    ("kill-server", &["-r"]),
    ("kill-session", &["-a", "-t"]),
    ("kill-window", &["-a", "-t"]),
    ("last-pane", &["-t"]),
    ("last-window", &["-t"]),
    ("list-buffers", &[]),
    ("list-clients", &[]),
    ("list-commands", &[]),
    ("list-keys", &[]),
    ("list-marks", &["-t"]),
    ("send-message", &["-t", "-r", "-w", "--to", "--re", "--task"]),
    ("read-message", &["-t", "-w"]),
    ("list-messages", &["-t", "-a"]),
    ("trace-message", &["-w"]),
    ("drop-message", &["-u"]),
    ("move-message", &[]),
    ("pane-ready", &["-q", "-t"]),
    ("pane-status", &[]),
    ("set-work-mode", &["-t"]),
    ("rename-pane", &["-t"]),
    ("whoami", &[]),
    ("list-tasks", &["-t"]),
    ("dashboard", &[]),
    ("create-pane", &["-k", "-t", "-s", "-h", "-c", "-n", "-m", "-M"]),
    ("show-task", &[]),
    ("list-events", &["-t", "-S", "-n"]),
    ("list-panes", &["-a", "-s", "-t", "-F"]),
    ("list-plugins", &[]),
    ("list-saved", &[]),
    ("list-sessions", &[]),
    ("list-windows", &["-a", "-t"]),
    ("load-buffer", &["-b"]),
    ("load-plugin", &[]),
    ("move-pane", &["-h", "-v", "-b", "-d", "-s", "-t"]),
    ("move-window", &["-s", "-t"]),
    ("new-session", &["-d", "-s", "-n", "-c", "-A", "-x", "-y"]),
    ("new-window", &["-d", "-n", "-c", "-t", "-a"]),
    ("next-layout", &["-t"]),
    ("next-window", &["-a", "-t"]),
    ("notify", &["-T"]),
    ("paste-buffer", &["-b", "-p", "-t"]),
    ("pipe-pane", &["-o", "-I", "-O", "-t"]),
    ("previous-layout", &["-t"]),
    ("previous-window", &["-a", "-t"]),
    ("record", &["-t"]),
    ("refresh-client", &["-U", "-D", "-L", "-R", "-S", "-C", "-t"]),
    ("respawn-pane", &["-k", "-t", "-c"]),
    ("respawn-window", &["-k", "-t", "-c"]),
    ("rotate-window", &["-D", "-U", "-t"]),
    ("rename-session", &["-t"]),
    ("rename-window", &["-t"]),
    ("resize-pane", &["-D", "-U", "-L", "-R", "-Z", "-x", "-y", "-t"]),
    ("resize-window", &["-x", "-y", "-U", "-D", "-L", "-R", "-A", "-a", "-t"]),
    ("restore-session", &["-a"]),
    ("resume", &[]),
    ("run-shell", &["-b", "-t"]),
    ("save-buffer", &["-a", "-b"]),
    ("save-session", &["-a", "-t"]),
    ("select-layout", &["-n", "-p", "-E", "-t"]),
    ("select-pane", &["-U", "-D", "-L", "-R", "-l", "-m", "-M", "-T", "-t"]),
    ("select-window", &["-t"]),
    ("send-keys", &["-l", "-R", "-t"]),
    ("send-prefix", &["-t"]),
    ("set-buffer", &["-b", "-a"]),
    ("set-cwd", &["-t"]),
    ("set-environment", &["-g", "-u"]),
    ("set-hook", &["-g", "-u"]),
    ("set-option", &["-g", "-a", "-w", "-s", "-t"]),
    ("set-window-option", &["-g", "-a", "-t"]),
    ("show-buffer", &["-b"]),
    ("show-environment", &["-g"]),
    ("show-hooks", &["-g"]),
    ("show-messages", &[]),
    ("show-options", &["-g", "-v", "-q", "-w", "-s"]),
    ("show-window-options", &["-g", "-v"]),
    ("source-file", &["-q"]),
    ("split-window", &["-h", "-v", "-b", "-d", "-f", "-c", "-t", "-N"]),
    ("start-server", &[]),
    ("swap-pane", &["-U", "-D", "-d", "-s", "-t"]),
    ("swap-window", &["-d", "-s", "-t"]),
    ("switch-client", &["-n", "-p", "-l", "-t"]),
    ("unbind-key", &["-n", "-T"]),
    ("undo-kill", &[]),
    ("version", &[]),
    ("wait-for", &["-L", "-U", "-S"]),
    ("web-start", &["-p", "-b", "-r", "-k"]),
    ("web-status", &[]),
    ("web-stop", &[]),
    ("link-add", &[]),
    ("link-allow", &["--shell", "--no-shell"]),
    ("link-id", &[]),
    ("link-inbound", &[]),
    ("link-list", &[]),
    ("link-panes", &[]),
    ("link-rekey", &[]),
    ("link-remove", &[]),
    ("link-trust", &["--shell"]),
];

/// The flags of a command, for completion.
pub fn flags_of(command: &str) -> &'static [&'static str] {
    FLAGS.iter().find(|(c, _)| *c == command).map(|(_, f)| *f).unwrap_or(&[])
}

/// Every command name starting with `prefix`, sorted: what a Tab offers.
pub fn complete_command(prefix: &str) -> Vec<&'static str> {
    let mut hits: Vec<&'static str> = COMMANDS.iter().copied().filter(|c| c.starts_with(prefix)).collect();
    hits.sort_unstable();
    hits
}

/// Whether `name`, as typed (a full name, a short alias or a prefix), is a
/// command whose first argument is an option name: `set-option`,
/// `show-options` and their window-option spellings.
pub fn takes_option_name(name: &str) -> bool {
    const ALIASES: &[&str] = &["set", "setw", "show", "showw", "show-option", "show-window-option"];
    ALIASES.contains(&name)
        || matches!(
            resolve_prefix(name),
            Ok("set-option" | "set-window-option" | "show-options" | "show-window-options")
        )
}

/// The positional arguments among `args` (the words after a command's
/// name): flags are left out, and so is the word after `-t`, its value.
pub fn positional_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-t" => i += 2,
            a if a.starts_with('-') => i += 1,
            a => {
                out.push(a);
                i += 1;
            }
        }
    }
    out
}

/// The longest prefix every candidate shares: what a Tab can type when
/// there are several.
pub fn common_prefix<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let mut it = items.into_iter();
    let Some(first) = it.next() else { return String::new() };
    let mut end = first.len();
    for s in it {
        end = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .map(|((i, a), _)| i + a.len_utf8())
            .last()
            .unwrap_or(0)
            .min(end);
    }
    first[..end].to_string()
}

/// Short names and other spellings of commands, each with the command it
/// stands for. Window- and pane-scoped option commands are the server's
/// here, so tmux's window forms are the same command under another name.
pub const ALIASES: &[(&str, &str)] = &[
    ("new", "new-session"),
    ("attach", "attach-session"),
    ("a", "attach-session"),
    ("at", "attach-session"),
    ("detach", "detach-client"),
    ("showmsgs", "show-messages"),
    ("setenv", "set-environment"),
    ("showenv", "show-environment"),
    ("respawnp", "respawn-pane"),
    ("respawnw", "respawn-window"),
    ("ls", "list-sessions"),
    ("lsw", "list-windows"),
    ("lsp", "list-panes"),
    ("has", "has-session"),
    ("rename", "rename-session"),
    ("neww", "new-window"),
    ("killw", "kill-window"),
    ("renamew", "rename-window"),
    ("selectw", "select-window"),
    ("swapw", "swap-window"),
    ("movew", "move-window"),
    ("next", "next-window"),
    ("prev", "previous-window"),
    ("last", "last-window"),
    ("splitw", "split-window"),
    ("killp", "kill-pane"),
    ("selectp", "select-pane"),
    ("resizep", "resize-pane"),
    ("resizew", "resize-window"),
    ("swapp", "swap-pane"),
    ("breakp", "break-pane"),
    ("joinp", "join-pane"),
    ("move-pane", "join-pane"),
    ("movep", "join-pane"),
    ("findw", "find-window"),
    ("findt", "find-text"),
    ("selectl", "select-layout"),
    ("nextl", "next-layout"),
    ("prevl", "previous-layout"),
    ("rotatew", "rotate-window"),
    ("refresh", "refresh-client"),
    ("lscm", "list-commands"),
    ("lsc", "list-clients"),
    ("lastp", "last-pane"),
    ("send", "send-keys"),
    ("pasteb", "paste-buffer"),
    ("setb", "set-buffer"),
    ("loadb", "load-buffer"),
    ("saveb", "save-buffer"),
    ("showb", "show-buffer"),
    ("deleteb", "delete-buffer"),
    ("lsb", "list-buffers"),
    ("focusp", "focus-pane"),
    ("pipep", "pipe-pane"),
    ("wait", "wait-for"),
    ("menu", "display-menu"),
    ("popup", "display-popup"),
    ("display", "display-message"),
    ("displayp", "display-panes"),
    ("if", "if-shell"),
    ("confirm", "confirm-before"),
    ("bind", "bind-key"),
    ("unbind", "unbind-key"),
    ("set", "set-option"),
    ("set-window-option", "set-option"),
    ("setw", "set-option"),
    // tmux's prefix for set-window-option, which set-work-mode would
    // otherwise make ambiguous.
    ("set-w", "set-option"),
    ("dash", "dashboard"),
    ("show-option", "show-options"),
    ("show", "show-options"),
    ("show-window-options", "show-options"),
    ("show-window-option", "show-options"),
    ("showw", "show-options"),
    ("switchc", "switch-client"),
    ("lsk", "list-keys"),
    ("run", "run-shell"),
    ("save", "save-session"),
    ("restore", "restore-session"),
    ("saved", "list-saved"),
    ("forget", "delete-saved"),
    ("clearhist", "clear-history"),
    ("capturep", "capture-pane"),
    ("cwd", "set-cwd"),
    ("source", "source-file"),
    ("-V", "version"),
    ("--version", "version"),
];

/// The command a typed name stands for: an alias, a full name, or an
/// unambiguous prefix of one (which may itself be an alias's spelling, as
/// `set-w` is of `set-window-option`).
pub fn canonical_name(name: &str) -> Result<&'static str, String> {
    let alias = |n: &str| ALIASES.iter().find(|(a, _)| *a == n).map(|(_, c)| *c);
    if let Some(c) = alias(name) {
        return Ok(c);
    }
    if let Some(c) = COMMANDS.iter().find(|c| **c == name) {
        return Ok(alias(c).unwrap_or(c));
    }
    let c = resolve_prefix(name)?;
    Ok(alias(c).unwrap_or(c))
}

/// tmux lets any unambiguous prefix stand for a command name, so `att` is
/// `attach-session` and `splitw` is `split-window`. Several matches is an
/// error rather than a guess.
fn resolve_prefix(name: &str) -> Result<&'static str, String> {
    let hits: Vec<&&str> = COMMANDS.iter().filter(|c| c.starts_with(name)).collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(format!("unknown command: {name}")),
        _ => {
            let mut names: Vec<&str> = hits.into_iter().copied().collect();
            names.sort_unstable();
            Err(format!("ambiguous command: {name} (could be {})", names.join(", ")))
        }
    }
}

/// Parse an argv (first word is the command name) into a `Cmd`.
pub fn parse(words: &[String]) -> Result<Cmd, String> {
    let name = words.first().ok_or_else(|| "empty command".to_string())?.as_str();
    let mut a = Args { words, pos: 1 };
    let canonical = canonical_name(name)?;
    let n = canonical;
    let cmd = match n {
        "new-session" => {
            let (mut name, mut window_name, mut cwd, mut detached, mut attach_existing) =
                (None, None, None, false, false);
            let mut size = (None, None);
            // Small enough to be a mistake rather than a wish.
            let dim = |flag: &str, v: &str, min: u16| -> Result<u16, String> {
                match v.parse::<u16>() {
                    Ok(n) if n >= min => Ok(n),
                    _ => Err(format!("new-session: {flag} wants a number of at least {min}, not '{v}'")),
                }
            };
            while a.is_flag() {
                match a.next().unwrap() {
                    "-s" => name = Some(a.value("-s")?.to_string()),
                    "-n" => window_name = Some(a.value("-n")?.to_string()),
                    "-c" => cwd = Some(a.value("-c")?.to_string()),
                    "-d" => detached = true,
                    "-A" => attach_existing = true,
                    "-Ad" | "-dA" => {
                        attach_existing = true;
                        detached = true;
                    }
                    "-x" => size.0 = Some(dim("-x", a.value("-x")?, 10)?),
                    "-y" => size.1 = Some(dim("-y", a.value("-y")?, 3)?),
                    f => return Err(bad_flag(n, f)),
                }
            }
            Cmd::NewSession { name, window_name, cwd, detached, argv: a.rest(), attach_existing, size }
        }
        "attach-session" => {
            let (mut target, mut detach_others) = (None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-d" => detach_others = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::AttachSession { target, detach_others }
        }
        "detach-client" => {
            let (mut all, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-a" => all = true,
                    "-s" => {
                        all = true;
                        target = Some(Target::parse(a.value("-s")?));
                    }
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-P" | "-E" => {} // tmux: what to do with the pane afterwards
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::DetachClient { all, target }
        }
        "show-messages" | "showmsgs" => {
            while a.is_flag() {
                a.next();
            }
            a.none_left(n)?;
            Cmd::ShowMessages
        }
        "set-environment" => {
            let mut remove = false;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-r" | "-u" => remove = true,
                    "-g" | "-h" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().ok_or("set-environment: name required")?.to_string();
            let rest = a.rest();
            let value = if rest.is_empty() { None } else { Some(rest.join(" ")) };
            Cmd::SetEnvironment { name, value, remove }
        }
        "show-environment" => {
            while a.is_flag() {
                a.next();
            }
            let name = a.next().map(str::to_string);
            a.none_left(n)?;
            Cmd::ShowEnvironment { name }
        }
        "respawn-pane" | "respawn-window" => {
            let (mut kill, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-k" => kill = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-c" | "-e" => {
                        a.value("-c")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            Cmd::RespawnPane { target, kill, argv: a.rest(), window: n == "respawn-window" }
        }
        "list-sessions" => {
            a.none_left(n)?;
            Cmd::ListSessions
        }
        "list-marks" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ListMarks { target }
        }
        "send-message" => {
            let (mut target, mut reply, mut wait, mut re, mut task) = (None, false, None, None, None);
            let id = |flag: &str, v: &str| -> Result<u64, String> {
                v.trim_start_matches('#')
                    .parse::<u64>()
                    .ok()
                    .filter(|i| *i > 0)
                    .ok_or_else(|| format!("{n}: {flag} takes a message number, not '{v}'"))
            };
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" | "--to" => target = Some(Target::parse(a.value("--to")?)),
                    "-r" => reply = true,
                    "--re" => re = Some(id("--re", a.value("--re")?)?),
                    "--task" => task = Some(id("--task", a.value("--task")?)?),
                    "-w" => wait = Some(seconds(n, a.value("-w")?)?),
                    f @ ("--from" | "--name" | "--mode" | "--id" | "--hop" | "--via") => {
                        return Err(format!(
                            "{n}: {f} is keepane's to fill in (who sends, its number, its hop and how it goes are never the sender's word)"
                        ));
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            if reply && (target.is_some() || re.is_some()) {
                return Err(format!("{n}: -r answers the message being worked on; it takes no --to or --re"));
            }
            let text = a.rest().join(" ");
            if text.is_empty() {
                return Err(format!("{n}: text required"));
            }
            Cmd::SendMessage { target, reply, re, task, wait, text }
        }
        "read-message" => {
            let (mut target, mut wait) = (None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-w" => wait = Some(seconds(n, a.value("-w")?)?),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ReadMessage { target, wait }
        }
        "list-messages" => {
            let (mut target, mut all) = (None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-a" => all = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ListMessages { target, all }
        }
        "trace-message" => {
            let (mut id, mut wait) = (None, None);
            while a.peek().is_some() {
                if a.is_flag() {
                    match a.next().unwrap() {
                        "-w" => wait = Some(seconds(n, a.value("-w")?)?),
                        f => return Err(bad_flag(n, f)),
                    }
                } else if id.is_none() {
                    id = Some(message_id(n, a.next().unwrap())?);
                } else {
                    a.none_left(n)?;
                }
            }
            Cmd::TraceMessage { id: id.ok_or_else(|| format!("{n}: message id required"))?, wait }
        }
        "drop-message" => {
            let mut undo = false;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-u" => undo = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            let id = a.next().map(|w| message_id(n, w)).transpose()?;
            a.none_left(n)?;
            if undo == id.is_some() {
                return Err(format!("{n}: a message id, or -u to bring back the one deleted last"));
            }
            Cmd::DropMessage { id, undo }
        }
        "move-message" => {
            a.none_flags(n)?;
            let id = message_id(n, a.next().ok_or_else(|| format!("{n}: message id required"))?)?;
            let to = match a.next() {
                Some("up") => MoveTo::Up,
                Some("down") => MoveTo::Down,
                Some("top") => MoveTo::Top,
                other => return Err(format!("{n}: where to: up, down or top (not {})", other.unwrap_or("nothing"))),
            };
            a.none_left(n)?;
            Cmd::MoveMessage { id, to }
        }
        "pane-ready" => {
            let (mut quiet, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-q" => quiet = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::PaneReady { quiet, target }
        }
        "pane-status" => {
            a.none_flags(n)?;
            Cmd::PaneStatus { text: a.rest().join(" ") }
        }
        "set-work-mode" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let mode = a.next().ok_or_else(|| format!("{n}: normal, shell or ai"))?.to_string();
            if !matches!(mode.as_str(), "normal" | "shell" | "ai") {
                return Err(format!("{n}: bad mode '{mode}' (normal, shell or ai)"));
            }
            a.none_left(n)?;
            Cmd::SetWorkMode { target, mode }
        }
        "rename-pane" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().ok_or_else(|| format!("{n}: name required (\"\" clears it)"))?.to_string();
            a.none_left(n)?;
            Cmd::RenamePane { target, name }
        }
        "whoami" => {
            a.none_left(n)?;
            Cmd::Whoami
        }
        "create-pane" => {
            let (mut kind, mut target, mut session_name, mut horizontal) = ("split".to_string(), None, None, false);
            let (mut cwd, mut name, mut mode, mut message) = (None, None, None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-k" => kind = a.value("-k")?.to_string(),
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-s" => session_name = Some(a.value("-s")?.to_string()),
                    "-h" => horizontal = true,
                    "-c" => cwd = Some(a.value("-c")?.to_string()),
                    "-n" => name = Some(a.value("-n")?.to_string()),
                    "-m" => mode = Some(a.value("-m")?.to_string()),
                    "-M" => message = Some(a.value("-M")?.to_string()),
                    f => return Err(bad_flag(n, f)),
                }
            }
            if !matches!(kind.as_str(), "session" | "window" | "split") {
                return Err(format!("{n}: bad kind '{kind}' (session, window or split)"));
            }
            if let Some(m) = &mode
                && !matches!(m.as_str(), "normal" | "shell" | "ai")
            {
                return Err(format!("{n}: bad mode '{m}' (normal, shell or ai)"));
            }
            let argv = a.rest();
            Cmd::CreatePane(Box::new(CreatePane {
                kind,
                target,
                session_name,
                horizontal,
                cwd,
                name,
                mode,
                message,
                argv,
            }))
        }
        "dashboard" => {
            a.none_left(n)?;
            Cmd::Dashboard
        }
        "list-tasks" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ListTasks { target }
        }
        "show-task" => {
            a.none_flags(n)?;
            let id = message_id(n, a.next().ok_or_else(|| format!("{n}: task id required"))?)?;
            a.none_left(n)?;
            Cmd::ShowTask { id }
        }
        "list-events" => {
            let (mut target, mut since, mut last) = (None, None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-S" => since = Some(duration(n, a.value("-S")?)?),
                    "-n" => {
                        let v = a.value("-n")?;
                        last = Some(v.parse().map_err(|_| format!("{n}: bad number of lines '{v}'"))?);
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ListEvents { target, since, last }
        }
        "list-windows" | "list-panes" | "kill-session" | "kill-window" | "kill-pane" => {
            let (mut target, mut all_but) = (None, false);
            let (mut all, mut session, mut format) = (false, false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-a" if n.starts_with("kill-") => all_but = true,
                    "-a" => all = true,     // list-panes: every pane on the server
                    "-s" => session = true, // list-panes: every pane in the session
                    "-F" if n == "list-panes" => format = Some(a.value("-F")?.to_string()),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            match n {
                "list-windows" => Cmd::ListWindows { target },
                "list-panes" => Cmd::ListPanes { target, all, session, format },
                "kill-session" => Cmd::KillSession { target, all_but },
                "kill-window" => Cmd::KillWindow { target, all_but },
                _ => Cmd::KillPane { target, all_but },
            }
        }
        "kill-server" => {
            // `-r` (keepane's own, used by `restart-server`): attached clients
            // are told the server is coming back, and attach again.
            let mut restarting = false;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-r" => restarting = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::KillServer { restarting }
        }
        "has-session" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::HasSession { target: target.ok_or("has-session: -t required")? }
        }
        "rename-session" | "rename-window" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().ok_or_else(|| format!("{n}: name required"))?.to_string();
            a.none_left(n)?;
            if n == "rename-session" { Cmd::RenameSession { target, name } } else { Cmd::RenameWindow { target, name } }
        }
        "new-window" => {
            let (mut name, mut cwd, mut target, mut detached) = (None, None, None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-n" => name = Some(a.value("-n")?.to_string()),
                    "-c" => cwd = Some(a.value("-c")?.to_string()),
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-d" => detached = true,
                    "-a" => {} // windows are always appended at the end
                    f => return Err(bad_flag(n, f)),
                }
            }
            Cmd::NewWindow { name, cwd, target, argv: a.rest(), detached }
        }
        "select-window" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            if target.is_none()
                && let Some(w) = a.next()
            {
                target = Some(Target::parse(&format!(":{w}")));
            }
            a.none_left(n)?;
            Cmd::SelectWindow { target: target.ok_or("select-window: -t required")? }
        }
        "swap-window" | "move-window" => {
            let (mut src, mut dst) = (None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-s" => src = Some(Target::parse(a.value("-s")?)),
                    "-t" => dst = Some(Target::parse(a.value("-t")?)),
                    "-d" | "-k" | "-a" | "-b" | "-r" => {} // tmux flags without a keepane meaning
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SwapWindow { src, dst, move_it: n == "move-window" }
        }
        "next-window" | "previous-window" | "last-window" => {
            let (mut target, mut alert) = (None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-a" if n != "last-window" => alert = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            match n {
                "next-window" => Cmd::NextWindow { target, alert },
                "previous-window" => Cmd::PreviousWindow { target, alert },
                _ => Cmd::LastWindow { target },
            }
        }
        "split-window" => {
            let (mut horizontal, mut cwd, mut target) = (false, None, None);
            let (mut detached, mut before, mut full) = (false, false, false);
            let mut count: u16 = 1;
            while a.is_flag() {
                let flag = a.next().unwrap();
                // Allow tmux-style combined single-letter flags: -hd, -bf, ...
                if flag.len() > 2 && flag[1..].chars().all(|c| "hvdbf".contains(c)) {
                    for c in flag[1..].chars() {
                        match c {
                            'h' => horizontal = true,
                            'v' => horizontal = false,
                            'd' => detached = true,
                            'b' => before = true,
                            _ => full = true,
                        }
                    }
                    continue;
                }
                match flag {
                    "-h" => horizontal = true,
                    "-v" => horizontal = false,
                    "-c" => cwd = Some(a.value("-c")?.to_string()),
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-d" => detached = true,
                    "-b" => before = true,
                    "-f" => full = true,
                    "-N" => {
                        let v = a.value("-N")?;
                        count =
                            v.parse().ok().filter(|n| (1..=64).contains(n)).ok_or_else(|| {
                                format!("split-window: -N takes a number of panes, 1 to 64 (not '{v}')")
                            })?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            Cmd::SplitWindow { horizontal, cwd, target, argv: a.rest(), detached, before, full, count }
        }
        "select-pane" => {
            let mut sel = None;
            // -T/-m/-M are a different command underneath: they name or mark
            // a pane rather than moving to one.
            let (mut title, mut mark, mut unmark, mut mark_target) = (None, false, false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-T" => title = Some(a.value("-T")?.to_string()),
                    "-m" => mark = true,
                    "-M" => unmark = true,
                    "-L" => sel = Some(PaneSel::Dir(Dir::Left)),
                    "-R" => sel = Some(PaneSel::Dir(Dir::Right)),
                    "-U" => sel = Some(PaneSel::Dir(Dir::Up)),
                    "-D" => sel = Some(PaneSel::Dir(Dir::Down)),
                    "-l" => sel = Some(PaneSel::Last),
                    "-t" => {
                        // With -m/-M/-T the value is a pane target; on its own
                        // it selects a pane (a direction word or an index).
                        let v = a.value("-t")?;
                        mark_target = Some(Target::parse(v));
                        sel = match v {
                            "next" | ":.+" | "+" => Some(PaneSel::Next),
                            "prev" | "previous" | ":.-" | "-" => Some(PaneSel::Prev),
                            "last" | ":.!" | "!" => Some(PaneSel::Last),
                            // A full name reaches into any window; a bare
                            // number is a pane of the current one.
                            other if other.contains(':') || other.contains('.') => {
                                Some(PaneSel::Target(Target::parse(other)))
                            }
                            other => other.trim_start_matches('%').parse().ok().map(PaneSel::Index),
                        };
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            if title.is_some() || mark || unmark {
                return Ok(Cmd::PaneTitle { target: mark_target, title, mark, unmark });
            }
            match sel {
                Some(sel) => Cmd::SelectPane { sel },
                None if mark_target.is_some() => {
                    return Err("select-pane: bad target (use a pane index, next, prev or last)".into());
                }
                None => return Err("select-pane: direction or -t required".into()),
            }
        }
        "resize-pane" => {
            let (mut dir, mut amount, mut zoom, mut target) = (None, 1u16, false, None);
            let (mut width, mut height) = (None, None);
            while a.is_flag() {
                let f = a.next().unwrap();
                match f {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-x" => width = Some(a.value("-x")?.to_string()),
                    "-y" => height = Some(a.value("-y")?.to_string()),
                    "-M" => {} // tmux: resize with the mouse
                    "-L" | "-R" | "-U" | "-D" => {
                        dir = Some(match f {
                            "-L" => Dir::Left,
                            "-R" => Dir::Right,
                            "-U" => Dir::Up,
                            _ => Dir::Down,
                        });
                        if let Some(v) = a.peek()
                            && let Ok(v) = v.parse::<u16>()
                        {
                            amount = v;
                            a.next();
                        }
                    }
                    "-Z" => zoom = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            if let Some(v) = a.peek()
                && let Ok(v) = v.parse::<u16>()
            {
                amount = v;
                a.next();
            }
            a.none_left(n)?;
            if dir.is_none() && !zoom && width.is_none() && height.is_none() {
                return Err("resize-pane: direction, -x/-y or -Z required".into());
            }
            Cmd::ResizePane { dir, amount, zoom, target, width, height }
        }
        "resize-window" => {
            let (mut target, mut width, mut height, mut adjust) = (None, None, None, None);
            let (mut largest, mut smallest) = (false, false);
            let size = |flag: &str, v: &str| -> Result<u16, String> {
                v.parse::<u16>().ok().filter(|n| *n > 0).ok_or_else(|| format!("resize-window: bad {flag} '{v}'"))
            };
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-x" => width = Some(size("-x", a.value("-x")?)?),
                    "-y" => height = Some(size("-y", a.value("-y")?)?),
                    f @ ("-U" | "-D" | "-L" | "-R") => {
                        let dir = match f {
                            "-U" => Dir::Up,
                            "-D" => Dir::Down,
                            "-L" => Dir::Left,
                            _ => Dir::Right,
                        };
                        let n = if a.is_flag() {
                            1
                        } else {
                            match a.next() {
                                None => 1,
                                Some(v) => size(f, v)?,
                            }
                        };
                        adjust = Some((dir, n));
                    }
                    "-A" => largest = true,
                    "-a" => smallest = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ResizeWindow { target, width, height, adjust, largest, smallest }
        }
        "swap-pane" => {
            let (mut up, mut target, mut source) = (false, None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-U" => up = true,
                    "-D" => up = false,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-s" => source = Some(Target::parse(a.value("-s")?)),
                    "-d" | "-Z" => {} // the active pane stays put either way; zoom is not touched
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SwapPane { up, target, source }
        }
        "next-layout" | "previous-layout" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SelectLayout {
                name: None,
                next: n == "next-layout",
                prev: n == "previous-layout",
                spread: false,
                target,
            }
        }
        "rotate-window" => {
            let (mut down, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-D" => down = true,
                    "-U" => down = false,
                    "-Z" => {}
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::RotateWindow { down, target }
        }
        "refresh-client" => {
            let mut pan = None;
            while a.is_flag() {
                let f = a.next().unwrap();
                match f {
                    // Pan the view by the amount that follows (1 without one).
                    "-U" | "-D" | "-L" | "-R" => {
                        let dir = match f {
                            "-U" => Dir::Up,
                            "-D" => Dir::Down,
                            "-L" => Dir::Left,
                            _ => Dir::Right,
                        };
                        let n = if a.is_flag() {
                            1
                        } else {
                            match a.next() {
                                None => 1,
                                Some(v) => v.parse::<u16>().map_err(|_| format!("refresh-client: bad amount '{v}'"))?,
                            }
                        };
                        pan = Some((dir, n.max(1)));
                    }
                    // tmux's other flags (client size, flags, targets) mean
                    // nothing here: every client draws itself.
                    "-C" | "-t" | "-A" | "-B" | "-f" | "-l" => {
                        a.value(f)?;
                    }
                    "-c" | "-S" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::RefreshClient { pan }
        }
        "send-prefix" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-2" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SendPrefix { target }
        }
        "list-commands" => {
            while a.is_flag() {
                a.next();
            }
            a.none_left(n)?;
            Cmd::ListCommands
        }
        "list-clients" => {
            while a.is_flag() {
                let f = a.next().unwrap();
                if matches!(f, "-t" | "-F") {
                    a.value(f)?;
                }
            }
            a.none_left(n)?;
            Cmd::ListClients
        }
        "last-pane" => {
            while a.is_flag() {
                let f = a.next().unwrap();
                if f == "-t" {
                    a.value(f)?;
                }
            }
            a.none_left(n)?;
            Cmd::SelectPane { sel: PaneSel::Last }
        }
        "select-layout" => {
            let (mut next, mut prev, mut spread, mut target) = (false, false, false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-n" => next = true,
                    "-p" => prev = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-E" => spread = true,
                    "-o" => {} // tmux: back to the previous layout
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().map(str::to_string);
            a.none_left(n)?;
            if name.is_none() && !next && !prev && !spread {
                return Err("select-layout: layout name, -n, -p or -E required".into());
            }
            Cmd::SelectLayout { name, next, prev, spread, target }
        }
        "join-pane" => {
            let (mut src, mut dst, mut horizontal, mut before) = (None, None, false, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-h" => horizontal = true,
                    "-v" => horizontal = false,
                    "-b" => before = true,
                    "-s" => src = Some(Target::parse(a.value("-s")?)),
                    "-t" => dst = Some(Target::parse(a.value("-t")?)),
                    "-d" | "-f" => {}
                    "-l" | "-p" => {
                        a.value("-l")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::JoinPane { src, dst, horizontal, before }
        }
        "find-window" => {
            while a.is_flag() {
                let f = a.next().unwrap();
                if f == "-t" {
                    a.value(f)?;
                }
            }
            let pattern = a.rest().join(" ");
            if pattern.is_empty() {
                return Err("find-window: pattern required".into());
            }
            Cmd::FindWindow { pattern }
        }
        "break-pane" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" | "-s" => target = Some(Target::parse(a.value("-t")?)),
                    "-d" | "-P" => {} // tmux flags without a keepane meaning
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::BreakPane { target }
        }
        "send-keys" => {
            let (mut target, mut literal, mut copy_cmd) = (None, false, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-l" => literal = true,
                    "-X" => copy_cmd = true,
                    "-R" | "-M" | "-N" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            if copy_cmd {
                let mut rest = a.rest().into_iter();
                let name = rest.next().ok_or("send-keys -X: command name required")?;
                return Ok(Cmd::CopyCommand { target, name, arg: rest.next() });
            }
            Cmd::SendKeys { target, keys: a.rest(), literal }
        }
        "copy-mode" => {
            let (mut page_up, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-u" => page_up = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-e" | "-M" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::CopyMode { page_up, target }
        }
        "paste-buffer" => {
            let (mut name, mut target, mut bracketed) = (None, None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-b" => name = Some(a.value("-b")?.to_string()),
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-p" => bracketed = true,
                    "-d" | "-r" => {}
                    "-s" => {
                        a.value("-s")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::PasteBuffer { name, target, bracketed }
        }
        "set-buffer" | "load-buffer" | "save-buffer" | "show-buffer" | "delete-buffer" => {
            let (mut name, mut append) = (None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-b" => name = Some(a.value("-b")?.to_string()),
                    "-a" => append = true,
                    "-w" | "-n" => {} // tmux: also set the clipboard / new name
                    f => return Err(bad_flag(n, f)),
                }
            }
            match n {
                "set-buffer" => {
                    let data = a.rest().join(" ");
                    Cmd::SetBuffer { name, data, append }
                }
                "load-buffer" => {
                    let path = a.next().ok_or("load-buffer: path required")?.to_string();
                    a.none_left(n)?;
                    Cmd::LoadBuffer { name, path }
                }
                "save-buffer" => {
                    let path = a.next().ok_or("save-buffer: path required")?.to_string();
                    a.none_left(n)?;
                    Cmd::SaveBuffer { name, path, append }
                }
                "show-buffer" => {
                    a.none_left(n)?;
                    Cmd::ShowBuffer { name }
                }
                _ => {
                    a.none_left(n)?;
                    Cmd::DeleteBuffer { name }
                }
            }
        }
        "list-buffers" => {
            while a.is_flag() {
                let f = a.next().unwrap();
                if f == "-F" {
                    a.value(f)?;
                }
            }
            a.none_left(n)?;
            Cmd::ListBuffers
        }
        "choose-buffer" => {
            while a.is_flag() {
                a.next();
            }
            a.none_left(n)?;
            Cmd::ChooseBuffer
        }
        "choose-jobs" => {
            a.none_left(n)?;
            Cmd::ChooseJobs
        }
        "choose-history" => {
            a.none_left(n)?;
            Cmd::ChooseHistory
        }
        "undo-kill" => {
            a.none_left(n)?;
            Cmd::UndoKill
        }
        "focus-pane" => {
            let id = a.next().ok_or("focus-pane: pane id required (%N, as list-panes shows it)")?;
            let pane: u32 = id
                .strip_prefix('%')
                .unwrap_or(id)
                .parse()
                .map_err(|_| format!("focus-pane: bad pane id '{id}' (%N, as list-panes shows it)"))?;
            a.none_left(n)?;
            Cmd::FocusPane { pane }
        }
        "choose-client" => {
            while a.is_flag() {
                let f = a.next().unwrap();
                if matches!(f, "-t" | "-F" | "-f" | "-O") {
                    a.value(f)?;
                }
            }
            a.none_left(n)?;
            Cmd::ChooseClient
        }
        "display-menu" => {
            let mut title = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-T" => title = Some(a.value("-T")?.to_string()),
                    // Placement and the client to show it on: keepane draws the
                    // menu over the window of the client that asked for it.
                    "-x" | "-y" | "-c" | "-t" => {
                        a.value("-x")?;
                    }
                    "-O" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            let words = a.rest();
            if words.is_empty() {
                return Err("display-menu: at least one item required".into());
            }
            let mut items = Vec::new();
            let mut i = 0;
            while i < words.len() {
                let name = words[i].clone();
                if name.is_empty() {
                    // A separator takes no key and no command.
                    items.push(MenuItem { name, key: None, cmd: None });
                    i += 1;
                    continue;
                }
                let key = words.get(i + 1).ok_or("display-menu: item needs a key and a command")?;
                let raw = words.get(i + 2).ok_or("display-menu: item needs a command")?;
                let key = if key.is_empty() { None } else { Some(key.clone()) };
                let cmd = if raw.is_empty() { None } else { Some(Box::new(parse(&tokenize(raw)?)?)) };
                items.push(MenuItem { name, key, cmd });
                i += 3;
            }
            Cmd::DisplayMenu { title, items }
        }
        "display-popup" => {
            let (mut close, mut close_on_exit) = (false, false);
            let (mut width, mut height, mut x, mut y, mut cwd) = (None, None, None, None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-C" => close = true,
                    "-E" | "-EE" => close_on_exit = true,
                    "-w" => width = Some(a.value("-w")?.to_string()),
                    "-h" => height = Some(a.value("-h")?.to_string()),
                    "-x" => x = Some(a.value("-x")?.to_string()),
                    "-y" => y = Some(a.value("-y")?.to_string()),
                    "-d" => cwd = Some(a.value("-d")?.to_string()),
                    // Whose client it is, its title and styles: the popup is
                    // the asking client's, drawn in the pane border style.
                    "-c" | "-t" | "-T" | "-s" | "-S" | "-b" | "-e" => {
                        a.value("-c")?;
                    }
                    "-B" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            let argv = a.rest();
            Cmd::DisplayPopup { close, close_on_exit, width, height, x, y, cwd, argv }
        }
        "pipe-pane" => {
            let (mut target, mut toggle, mut input, mut output) = (None, false, false, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-o" => toggle = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-O" => output = true,
                    "-I" => input = true,
                    "-IO" | "-OI" => (input, output) = (true, true),
                    f => return Err(bad_flag(n, f)),
                }
            }
            if !input {
                output = true; // tmux: -O when neither is given
            }
            let command = a.rest().join(" ");
            Cmd::PipePane {
                target,
                command: if command.is_empty() { None } else { Some(command) },
                toggle,
                input,
                output,
            }
        }
        "wait-for" => {
            let (mut lock, mut unlock, mut signal) = (false, false, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-L" => lock = true,
                    "-U" => unlock = true,
                    "-S" => signal = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            let channel = a.next().ok_or("wait-for: channel required")?.to_string();
            a.none_left(n)?;
            if [lock, unlock, signal].iter().filter(|x| **x).count() > 1 {
                return Err("wait-for: -L, -S and -U are exclusive".into());
            }
            Cmd::WaitFor { channel, lock, unlock, signal }
        }
        "web-start" => {
            let mut o = crate::web::Options::default();
            while a.is_flag() {
                match a.next().unwrap() {
                    "-p" => {
                        let v = a.value("-p")?;
                        o.port = Some(v.parse().map_err(|_| format!("{n}: not a port: {v}"))?);
                    }
                    "-b" => {
                        let v = a.value("-b")?;
                        o.bind = Some(v.parse().map_err(|_| format!("{n}: not an IP address: {v}"))?);
                    }
                    "-r" => o.read_only = true,
                    "-k" => o.keep_key = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::WebStart(o)
        }
        "web-status" | "web-stop" => {
            a.none_left(n)?;
            if n == "web-status" { Cmd::WebStatus } else { Cmd::WebStop }
        }
        "link-id" | "link-list" | "link-rekey" => {
            a.none_left(n)?;
            match n {
                "link-id" => Cmd::LinkId,
                "link-list" => Cmd::LinkList,
                _ => Cmd::LinkRekey,
            }
        }
        "link-add" => {
            let url =
                a.next().ok_or("link-add: the other machine's `keepane web` address (http://host:port/#k=...)")?;
            let url = url.to_string();
            a.none_left(n)?;
            Cmd::LinkAdd { url }
        }
        "link-panes" | "link-remove" => {
            let addr = a.next().ok_or_else(|| format!("{n}: host:port required"))?.to_string();
            a.none_left(n)?;
            if !crate::link::valid_hostport(&addr) {
                return Err(format!("{n}: '{addr}' is not host:port"));
            }
            if n == "link-panes" { Cmd::LinkPanes { addr } } else { Cmd::LinkRemove { addr } }
        }
        "link-allow" => {
            let (mut addr, mut shell) = (None, None);
            while let Some(w) = a.next() {
                match w {
                    "--shell" => shell = Some(true),
                    "--no-shell" => shell = Some(false),
                    f if f.starts_with('-') => return Err(bad_flag(n, f)),
                    v if addr.is_none() => addr = Some(v.to_string()),
                    v => return Err(format!("{n}: what is '{v}'?")),
                }
            }
            let Some(addr) = addr else { return Err(format!("{n}: host:port required")) };
            if !crate::link::valid_hostport(&addr) {
                return Err(format!("{n}: '{addr}' is not host:port"));
            }
            let Some(shell) = shell else { return Err(format!("{n}: --shell or --no-shell required")) };
            Cmd::LinkAllow { addr, shell }
        }
        "link-trust" => {
            let (mut words, mut shell) = (Vec::new(), false);
            while let Some(w) = a.next() {
                match w {
                    "--shell" => shell = true,
                    f if f.starts_with('-') && !f.starts_with("--") || f.starts_with("--") => {
                        return Err(bad_flag(n, f));
                    }
                    v => words.push(v.to_string()),
                }
            }
            let [addr, key] = <[String; 2]>::try_from(words)
                .map_err(|_| format!("{n}: host:port and the machine's public key (its `keepane link id`)"))?;
            if !crate::link::valid_hostport(&addr) {
                return Err(format!("{n}: '{addr}' is not host:port"));
            }
            if !crate::link::valid_public(&key) {
                return Err(format!("{n}: '{key}' is not a keepane link public key"));
            }
            Cmd::LinkTrust { addr, key, shell }
        }
        "link-inbound" => {
            let peer = a.next().ok_or("link-inbound: peer address required")?.to_string();
            let request = a.next().ok_or("link-inbound: the request (JSON) required")?.to_string();
            a.none_left(n)?;
            Cmd::LinkInbound { peer, request }
        }
        "clock-mode" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::ClockMode { target }
        }
        "if-shell" => {
            let mut format = false;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-F" => format = true,
                    "-b" => {} // background: keepane runs it inline either way
                    "-t" => {
                        a.value("-t")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            let condition = a.next().ok_or("if-shell: condition required")?.to_string();
            let then_raw = a.next().ok_or("if-shell: command required")?.to_string();
            let else_raw = a.next().map(str::to_string);
            a.none_left(n)?;
            let parse_sub = |s: &str| -> Result<Cmd, String> { parse(&tokenize(s)?) };
            Cmd::IfShell {
                format,
                condition,
                then_cmd: Box::new(parse_sub(&then_raw)?),
                else_cmd: match else_raw {
                    Some(e) => Some(Box::new(parse_sub(&e)?)),
                    None => None,
                },
            }
        }
        "display-panes" => {
            while a.is_flag() {
                match a.next().unwrap() {
                    "-b" | "-N" => {} // tmux: background / no key wait
                    "-d" => {
                        a.value("-d")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::DisplayPanes
        }
        "command-prompt" => {
            let (mut prompt, mut initial) = (None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-p" => prompt = Some(a.value("-p")?.to_string()),
                    "-I" => initial = Some(a.value("-I")?.to_string()),
                    "-1" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            let rest = a.rest();
            let template = if rest.is_empty() { None } else { Some(rest.join(" ")) };
            Cmd::CommandPrompt { prompt, initial, template }
        }
        "display-message" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    // -c names a client; there is one per connection here.
                    "-c" => {
                        a.value("-c")?;
                    }
                    // -p (print), -d (delay), -I (stdin): every message is
                    // printed to a script and shown to a terminal.
                    _ => {}
                }
            }
            Cmd::DisplayMessage { msg: a.rest().join(" "), target }
        }
        "confirm-before" => {
            let mut prompt = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-p" => prompt = Some(a.value("-p")?.to_string()),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let rest = a.rest();
            let inner = if rest.len() == 1 { tokenize(&rest[0])? } else { rest };
            Cmd::ConfirmBefore { prompt, cmd: Box::new(parse(&inner)?) }
        }
        "bind-key" => {
            let (mut table, mut repeat) = (KeyTable::Prefix, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-n" => table = KeyTable::Root,
                    "-r" => repeat = true,
                    "-T" => table = key_table(n, a.value("-T")?)?,
                    f => return Err(bad_flag(n, f)),
                }
            }
            let key = a.next().ok_or("bind-key: key required")?.to_string();
            let rest = a.rest();
            let inner = if rest.len() == 1 { tokenize(&rest[0])? } else { rest };
            Cmd::BindKey { table, key, repeat, cmd: Box::new(parse(&inner)?) }
        }
        "unbind-key" => {
            let mut table = KeyTable::Prefix;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-n" => table = KeyTable::Root,
                    "-T" => table = key_table(n, a.value("-T")?)?,
                    f => return Err(bad_flag(n, f)),
                }
            }
            let key = a.next().ok_or("unbind-key: key required")?.to_string();
            a.none_left(n)?;
            Cmd::UnbindKey { table, key }
        }
        "set-option" => {
            // -g / -s / -w are accepted and ignored; -a appends, and tmux
            // users write it combined ("set -ag status-right ...").
            let (mut append, mut target) = (false, None);
            while a.is_flag() {
                let flag = a.next().unwrap();
                if flag == "-t" {
                    target = Some(Target::parse(a.value("-t")?));
                    continue;
                }
                if flag.contains('a') {
                    append = true;
                }
            }
            // An option name may be abbreviated the way a command name is:
            // `set sync` is `set synchronize-panes`. Expanding it here means
            // everything downstream sees the real name.
            let name = crate::config::resolve_name(a.next().ok_or("set-option: option name required")?)?;
            let value = a.rest().join(" ");
            Cmd::SetOption { name, value, append, target }
        }
        "switch-client" => {
            let (mut next, mut prev, mut last, mut target) = (false, false, false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-n" => next = true,
                    "-p" => prev = true,
                    "-l" => last = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-Z" | "-E" | "-r" => {}
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SwitchClient { next, prev, last, target }
        }
        "list-keys" => {
            a.none_left(n)?;
            Cmd::ListKeys
        }
        "choose-tree" | "choose-window" | "choose-session" => {
            let (mut sessions, mut windows) = (n == "choose-session", n == "choose-window");
            while a.is_flag() {
                // Combined flags as tmux configs write them: -Zw, -Zs.
                let flag = a.next().unwrap();
                if flag.len() < 2 || !flag[1..].chars().all(|c| "swZNGr".contains(c)) {
                    return Err(bad_flag(n, flag));
                }
                for c in flag[1..].chars() {
                    match c {
                        's' => sessions = true,
                        'w' => windows = true,
                        _ => {} // -Z -N -G -r: tmux flags without a keepane meaning
                    }
                }
            }
            a.none_left(n)?;
            Cmd::ChooseTree { sessions, windows }
        }
        "show-options" => {
            let (mut value_only, mut quiet) = (false, false);
            while a.is_flag() {
                let flag = a.next().unwrap();
                // Combined flags as tmux users write them: -gqv, -gv, ...
                if flag.len() > 2 && flag[1..].chars().all(|c| "gswpqv".contains(c)) {
                    value_only |= flag.contains('v');
                    quiet |= flag.contains('q');
                    continue;
                }
                match flag {
                    "-v" => value_only = true,
                    "-q" => quiet = true,
                    "-g" | "-s" | "-w" | "-p" => {}
                    "-t" => {
                        a.value("-t")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().map(crate::config::resolve_name).transpose()?;
            a.none_left(n)?;
            Cmd::ShowOptions { name, value_only, quiet }
        }
        "run-shell" => {
            let (mut background, mut target) = (false, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-b" => background = true,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-C" => {} // tmux: run as a keepane command; here everything is a shell command
                    f => return Err(bad_flag(n, f)),
                }
            }
            let rest = a.rest();
            if rest.is_empty() {
                return Err("run-shell: command required".into());
            }
            Cmd::RunShell { command: rest.join(" "), background, target }
        }
        "set-hook" => {
            let mut unset = false;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-u" | "-gu" | "-ug" => unset = true,
                    "-g" | "-a" | "-R" => {}
                    "-t" => {
                        a.value("-t")?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            let hook = a.next().ok_or("set-hook: hook name required")?.to_string();
            if unset {
                a.none_left(n)?;
                Cmd::SetHook { hook, cmd: None }
            } else {
                let rest = a.rest();
                if rest.is_empty() {
                    return Err("set-hook: command required".into());
                }
                let inner = if rest.len() == 1 { tokenize(&rest[0])? } else { rest };
                Cmd::SetHook { hook, cmd: Some(Box::new(parse(&inner)?)) }
            }
        }
        "show-hooks" => {
            while a.is_flag() {
                a.next();
            }
            Cmd::ShowHooks
        }
        "load-plugin" => {
            let path = a.next().ok_or("load-plugin: name or path required")?.to_string();
            a.none_left(n)?;
            Cmd::LoadPlugin { path }
        }
        "list-plugins" => {
            a.none_left(n)?;
            Cmd::ListPlugins
        }
        "save-session" => {
            let (mut target, mut all) = (None, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-a" => all = true,
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::SaveSession { target, all }
        }
        "restore-session" | "resume" => {
            let mut attach = n == "resume";
            while a.is_flag() {
                match a.next().unwrap() {
                    "-a" => attach = true,
                    "-t" => {
                        // `resume -t name` reads naturally next to attach -t.
                        let v = a.value("-t")?.to_string();
                        a.none_left(n)?;
                        return Ok(Cmd::RestoreSession { name: Some(v), attach });
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            let name = a.next().map(str::to_string);
            a.none_left(n)?;
            Cmd::RestoreSession { name, attach }
        }
        "list-saved" => {
            a.none_flags(n)?;
            a.none_left(n)?;
            Cmd::ListSaved
        }
        "delete-saved" => {
            a.none_flags(n)?;
            let name = a.next().ok_or("delete-saved: name required")?.to_string();
            a.none_left(n)?;
            Cmd::DeleteSaved { name }
        }
        "clear-history" => {
            a.none_left(n)?;
            Cmd::ClearHistory
        }
        "set-cwd" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let dir = a.next().map(str::to_string);
            a.none_left(n)?;
            Cmd::SetCwd { target, dir }
        }
        "capture-pane" => {
            let (mut target, mut history, mut escapes, mut join) = (None, 0usize, false, false);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-S" => {
                        let v = a.value("-S")?;
                        history = match v {
                            "-" => usize::MAX,
                            v => {
                                v.trim_start_matches('-').parse().map_err(|_| format!("capture-pane: bad -S '{v}'"))?
                            }
                        };
                    }
                    "-e" => escapes = true,
                    "-J" => join = true,
                    "-p" => {} // always printed
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::CapturePane { target, history, escapes, join }
        }
        "source-file" => {
            let path = a.next().ok_or("source-file: path required")?.to_string();
            a.none_left(n)?;
            Cmd::SourceFile { path }
        }
        "version" => {
            a.none_left(n)?;
            Cmd::Version
        }
        "start-server" => {
            // Any command starts the server; this one does nothing else, so
            // that a `.tmux.conf` beginning with it still loads.
            while a.is_flag() {
                a.next();
            }
            a.none_left(n)?;
            Cmd::StartServer
        }
        "record" => {
            let mut target = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let path = a.next().map(str::to_string);
            a.none_left(n)?;
            Cmd::Record { target, path }
        }
        "find-text" => {
            let (mut target, mut case_sensitive, mut per_pane) = (None, false, 3usize);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-C" => case_sensitive = true,
                    "-i" => case_sensitive = false,
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-n" => {
                        per_pane = a.value("-n")?.parse().map_err(|_| "find-text: -n takes a number".to_string())?;
                    }
                    f => return Err(bad_flag(n, f)),
                }
            }
            let pattern = a.rest().join(" ");
            // Whitespace alone would match most lines of most panes, which
            // is never what anyone meant to ask for.
            if pattern.trim().is_empty() {
                return Err("find-text: pattern required".into());
            }
            Cmd::FindText { pattern, target, case_sensitive, per_pane: per_pane.clamp(1, 100) }
        }
        "jobs" => {
            let (mut target, mut format) = (None, None);
            while a.is_flag() {
                match a.next().unwrap() {
                    "-t" => target = Some(Target::parse(a.value("-t")?)),
                    "-F" => format = Some(a.value("-F")?.to_string()),
                    f => return Err(bad_flag(n, f)),
                }
            }
            a.none_left(n)?;
            Cmd::Jobs { target, format }
        }
        "notify" => {
            let mut title = None;
            while a.is_flag() {
                match a.next().unwrap() {
                    "-T" => title = Some(a.value("-T")?.to_string()),
                    f => return Err(bad_flag(n, f)),
                }
            }
            let message = a.rest().join(" ");
            if message.is_empty() {
                return Err("notify: message required".into());
            }
            Cmd::Notify { title, message }
        }
        _ => unreachable!(),
    };
    Ok(cmd)
}

/// Parse a full command line (as typed at the `:` prompt or in a config file).
pub fn parse_line(line: &str) -> Result<Option<Cmd>, String> {
    let words = tokenize(line)?;
    if words.is_empty() {
        return Ok(None);
    }
    parse(&words).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name a command can be typed as reaches a parser arm: a name
    /// that resolves but has no arm used to panic the server (`set-w` was
    /// `set-window-option`, which only the alias table knew). Missing
    /// arguments are an error, never a panic.
    #[test]
    fn every_command_name_and_prefix_parses_without_panic() {
        let mut names: Vec<String> = ALIASES.iter().map(|(a, _)| a.to_string()).collect();
        for c in COMMANDS {
            for i in 1..=c.len() {
                names.push(c[..i].to_string());
            }
        }
        for n in &names {
            let words = vec![n.clone()];
            let r = std::panic::catch_unwind(|| parse(&words));
            assert!(r.is_ok(), "parse({n:?}) panicked");
        }
        assert_eq!(canonical_name("set-w").unwrap(), "set-option");
        assert_eq!(canonical_name("show-w").unwrap(), "show-options");
        assert_eq!(canonical_name("move-p").unwrap(), "join-pane");
        assert!(matches!(p("set-w -g mouse on"), Cmd::SetOption { .. }));
    }

    /// Which commands take an option name, and where it sits among flags.
    #[test]
    fn option_name_position() {
        for c in ["set", "setw", "set-option", "set-o", "show", "showw", "show-options", "show-window-options"] {
            assert!(takes_option_name(c), "{c}");
        }
        for c in ["set-hook", "set-buffer", "show-buffer", "split-window", "s"] {
            assert!(!takes_option_name(c), "{c}");
        }
        assert!(positional_args(&[]).is_empty());
        assert!(positional_args(&["-g", "-s"]).is_empty());
        assert_eq!(positional_args(&["-g", "-t", "work", "mouse"]), vec!["mouse"]);
        assert_eq!(positional_args(&["-gq", "status", "on"]), vec!["status", "on"]);
    }

    fn p(s: &str) -> Cmd {
        parse_line(s).unwrap().unwrap()
    }

    #[test]
    fn tokenizer() {
        assert_eq!(tokenize(r#"a "b c" 'd e' f\ g # comment"#).unwrap(), vec!["a", "b c", "d e", "f g"]);
        assert_eq!(tokenize(r#""x\"y""#).unwrap(), vec!["x\"y"]);
        assert_eq!(tokenize(r"C:\src\new C:\ok").unwrap(), vec![r"C:\src\new", r"C:\ok"]);
        assert_eq!(tokenize(r#""C:\src\new" "a\\b""#).unwrap(), vec![r"C:\src\new", r"a\b"]);
        assert_eq!(tokenize(r"trailing\").unwrap(), vec![r"trailing\"]);
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
        assert_eq!(tokenize("   # only comment").unwrap(), Vec::<String>::new());
        assert!(tokenize("\"open").is_err());
    }

    #[test]
    fn quote_roundtrip() {
        for s in ["plain", "with space", "q\"uote", "", "semi;colon", "back\\slash", "#W", "C:\\src\\new", "a\\\\b"] {
            let q = quote(s);
            assert_eq!(tokenize(&q).unwrap(), vec![s], "quote({s:?}) = {q}");
        }
    }

    #[test]
    fn targets() {
        let t = |session: Option<&str>, window: Option<&str>, pane: Option<usize>| Target {
            session: session.map(String::from),
            window: window.map(String::from),
            pane,
            pane_id: None,
            pane_name: None,
            remote: None,
        };
        assert_eq!(Target::parse("main"), t(Some("main"), None, None));
        assert_eq!(Target::parse("main:2"), t(Some("main"), Some("2"), None));
        assert_eq!(Target::parse(":2"), t(None, Some("2"), None));
        assert_eq!(Target::parse("s:w.3"), t(Some("s"), Some("w"), Some(3)));
        // A pane by its id, and it prints back the same way.
        assert_eq!(Target::parse("%12"), Target { pane_id: Some(12), ..Default::default() });
        assert_eq!(target_string(&Target::parse("%12")), "%12");
        // Not a number after %: a pane by its name.
        assert_eq!(Target::parse("%builder"), Target { pane_name: Some("builder".into()), ..Default::default() });
        // A full address: where the pane is expected, and the pane.
        let full = Target::parse("$1:@3.%7");
        assert_eq!(
            full,
            Target { session: Some("$1".into()), window: Some("@3".into()), pane_id: Some(7), ..Default::default() }
        );
        for s in ["%12", "%builder", "$1:@3.%7", "work:2.%tester", "s:w.3", ":2"] {
            assert_eq!(target_string(&Target::parse(s)), s);
        }
    }

    #[test]
    fn parse_new_session() {
        assert_eq!(
            p("new -s main -n shell -d -- wsl.exe -d Ubuntu"),
            Cmd::NewSession {
                name: Some("main".into()),
                window_name: Some("shell".into()),
                cwd: None,
                detached: true,
                argv: vec!["wsl.exe".into(), "-d".into(), "Ubuntu".into()],
                attach_existing: false,
                size: (None, None),
            }
        );
        assert_eq!(
            p("new-session"),
            Cmd::NewSession {
                name: None,
                window_name: None,
                cwd: None,
                detached: false,
                argv: vec![],
                attach_existing: false,
                size: (None, None),
            }
        );
        assert!(matches!(p("new -A -s x"), Cmd::NewSession { attach_existing: true, detached: false, .. }));
        assert!(matches!(p("new -Ad -s x"), Cmd::NewSession { attach_existing: true, detached: true, .. }));
        // -x/-y size a detached session; either alone is fine, nonsense is not.
        assert!(matches!(p("new -d -x 20 -y 4"), Cmd::NewSession { size: (Some(20), Some(4)), .. }));
        assert!(matches!(p("new -d -y 50"), Cmd::NewSession { size: (None, Some(50)), .. }));
        let err = parse_line("new -d -x 2").unwrap_err();
        assert!(err.contains("at least 10"), "{err}");
        let err = parse_line("new -d -y big").unwrap_err();
        assert!(err.contains("at least 3"), "{err}");
        assert_eq!(p("new -d -x 20 -y 4").to_string(), "new-session -d -x 20 -y 4");
    }

    #[test]
    fn double_dash_ends_flags() {
        // The default `,` binding expands to exactly this.
        assert_eq!(p("rename-window -- shell"), Cmd::RenameWindow { target: None, name: "shell".into() });
        assert_eq!(p("rename-session -- -weird"), Cmd::RenameSession { target: None, name: "-weird".into() });
        assert!(matches!(p("new-window -- -x"), Cmd::NewWindow { argv, .. } if argv == vec!["-x".to_string()]));
        assert!(
            matches!(p("send-keys -- -l"), Cmd::SendKeys { keys, literal: false, .. } if keys == vec!["-l".to_string()])
        );
        assert!(
            matches!(p("split-window -h -- pwsh -NoLogo"), Cmd::SplitWindow { horizontal: true, argv, .. } if argv.len() == 2)
        );
    }

    #[test]
    fn parse_plugin_commands() {
        assert_eq!(
            p("run-shell -b pwsh -c \"Get-Date\""),
            Cmd::RunShell { command: "pwsh -c Get-Date".into(), background: true, target: None }
        );
        assert_eq!(p("run \"echo hi\""), Cmd::RunShell { command: "echo hi".into(), background: false, target: None });
        assert!(parse_line("run-shell").is_err());
        assert_eq!(
            p("set-hook -g after-new-window rename-window hooked"),
            Cmd::SetHook {
                hook: "after-new-window".into(),
                cmd: Some(Box::new(Cmd::RenameWindow { target: None, name: "hooked".into() }))
            }
        );
        assert_eq!(
            p("set-hook -g after-new-window \"rename-window hooked\""),
            Cmd::SetHook {
                hook: "after-new-window".into(),
                cmd: Some(Box::new(Cmd::RenameWindow { target: None, name: "hooked".into() }))
            }
        );
        assert_eq!(p("set-hook -gu after-new-window"), Cmd::SetHook { hook: "after-new-window".into(), cmd: None });
        assert_eq!(p("load-plugin demo"), Cmd::LoadPlugin { path: "demo".into() });
        assert_eq!(
            p("show-options -gqv @theme"),
            Cmd::ShowOptions { name: Some("@theme".into()), value_only: true, quiet: true }
        );
        assert_eq!(p("show -g"), Cmd::ShowOptions { name: None, value_only: false, quiet: false });
        assert_eq!(p("resume"), Cmd::RestoreSession { name: None, attach: true });
        assert_eq!(p("resume work"), Cmd::RestoreSession { name: Some("work".into()), attach: true });
        assert_eq!(p("resume -t work"), Cmd::RestoreSession { name: Some("work".into()), attach: true });
        assert_eq!(p("restore-session"), Cmd::RestoreSession { name: None, attach: false });
        assert_eq!(p("save -t work"), Cmd::SaveSession { target: Some(Target::parse("work")), all: false });
        assert_eq!(p("save -a"), Cmd::SaveSession { target: None, all: true });
        assert_eq!(p("saved"), Cmd::ListSaved);
        assert_eq!(p("forget old"), Cmd::DeleteSaved { name: "old".into() });
        assert!(parse_line("save-session -x").is_err());
        assert!(parse_line("delete-saved").is_err());
        assert_eq!(
            p("show-option -v mouse"),
            Cmd::ShowOptions { name: Some("mouse".into()), value_only: true, quiet: false }
        );
    }

    #[test]
    fn parse_flags_that_change_behaviour() {
        assert!(matches!(p("neww -d"), Cmd::NewWindow { detached: true, .. }));
        assert!(matches!(
            p("splitw -hdb"),
            Cmd::SplitWindow { horizontal: true, detached: true, before: true, full: false, .. }
        ));
        assert!(matches!(p("splitw -f -v"), Cmd::SplitWindow { horizontal: false, full: true, .. }));
        assert!(matches!(p("killw -a"), Cmd::KillWindow { all_but: true, .. }));
        assert!(matches!(p("killp -a -t :.1"), Cmd::KillPane { all_but: true, .. }));
        assert!(matches!(p("kill-session -a"), Cmd::KillSession { all_but: true, .. }));
        assert!(matches!(p("send -l Enter"), Cmd::SendKeys { literal: true, .. }));
    }

    /// The completion table and the parser agree: every command is in the
    /// table, and every flag in it is one the parser takes (the parser may
    /// still complain about what follows, never about the flag itself).
    #[test]
    fn the_flag_table_matches_the_parser() {
        for c in COMMANDS {
            assert!(FLAGS.iter().any(|(n, _)| n == c), "{c} has no entry in FLAGS");
        }
        let mut bad = Vec::new();
        for (cmd, flags) in FLAGS {
            assert!(COMMANDS.contains(cmd), "{cmd} is in FLAGS but not a command");
            for f in *flags {
                // `-x`, or a word for a field (`--re`).
                assert!(f.len() == 2 || (f.starts_with("--") && f.len() > 3), "{cmd}: flag {f}");
                assert!(f.starts_with('-'), "{cmd}: flag {f}");
                let words: Vec<String> = [cmd, f, "1", "x"].iter().map(|s| s.to_string()).collect();
                if let Err(e) = parse(&words)
                    && e.contains("unknown flag")
                {
                    bad.push(format!("{cmd} {f}"));
                }
            }
        }
        assert!(bad.is_empty(), "flags the parser does not take:\n{}", bad.join("\n"));
        assert_eq!(complete_command("spl"), vec!["split-window"]);
        assert_eq!(complete_command("list-s"), vec!["list-saved", "list-sessions"]);
        assert!(complete_command("zzz").is_empty());
        assert_eq!(common_prefix(["list-saved", "list-sessions"]), "list-s");
        assert_eq!(common_prefix(["abc"]), "abc");
        assert_eq!(common_prefix(["abc", "xyz"]), "");
        assert_eq!(common_prefix(std::iter::empty::<&str>()), "");
        assert_eq!(flags_of("split-window").len(), 8);
        assert!(flags_of("nope").is_empty());
    }

    #[test]
    fn unambiguous_prefixes_are_command_names() {
        // tmux habits: att, splitw, lsp, neww, ...
        assert!(matches!(p("att"), Cmd::AttachSession { .. }));
        assert!(matches!(p("attach-s"), Cmd::AttachSession { .. }));
        assert!(matches!(p("spl -h"), Cmd::SplitWindow { horizontal: true, .. }));
        assert!(matches!(p("resize-p -Z"), Cmd::ResizePane { zoom: true, .. }));
        assert!(matches!(p("resizew -x 100"), Cmd::ResizeWindow { width: Some(100), .. }));
        assert!(parse_line("resi -Z").unwrap_err().starts_with("ambiguous command: resi"), "pane or window");
        assert!(matches!(p("choose-t"), Cmd::ChooseTree { .. }));
        assert!(matches!(p("swap-p -U"), Cmd::SwapPane { up: true, .. }));
        // The short aliases still win over the prefix rule.
        assert!(matches!(p("ls"), Cmd::ListSessions));
        assert!(matches!(p("new -d"), Cmd::NewSession { detached: true, .. }));
        // Ambiguity is an error, not a guess.
        let e = parse_line("kill").unwrap_err();
        assert!(e.starts_with("ambiguous command: kill (could be kill-pane, kill-server"), "{e}");
        assert!(parse_line("list-s").unwrap_err().starts_with("ambiguous command"));
        assert_eq!(parse_line("nosuchthing").unwrap_err(), "unknown command: nosuchthing");
    }

    #[test]
    fn parse_choose_tree() {
        assert_eq!(p("choose-tree"), Cmd::ChooseTree { sessions: false, windows: false });
        assert_eq!(p("choose-tree -Zw"), Cmd::ChooseTree { sessions: false, windows: true });
        assert_eq!(p("choose-window"), Cmd::ChooseTree { sessions: false, windows: true });
        assert_eq!(p("choose-session"), Cmd::ChooseTree { sessions: true, windows: false });
        assert_eq!(p("choose-tree -s -w").to_string(), "choose-tree -s -w");
        assert!(parse_line("choose-tree -x").is_err());
        assert!(parse_line("choose-tree extra").is_err());
    }

    #[test]
    fn parse_pane_commands() {
        assert_eq!(p("select-pane -L"), Cmd::SelectPane { sel: PaneSel::Dir(Dir::Left) });
        assert_eq!(p("selectp -t next"), Cmd::SelectPane { sel: PaneSel::Next });
        assert_eq!(
            p("resize-pane -R 5"),
            Cmd::ResizePane { dir: Some(Dir::Right), amount: 5, zoom: false, target: None, width: None, height: None }
        );
        assert_eq!(
            p("resizep -Z"),
            Cmd::ResizePane { dir: None, amount: 1, zoom: true, target: None, width: None, height: None }
        );
        assert_eq!(
            p("splitw -h -c C:\\src"),
            Cmd::SplitWindow {
                horizontal: true,
                cwd: Some("C:\\src".into()),
                target: None,
                argv: vec![],
                detached: false,
                before: false,
                full: false,
                count: 1
            }
        );
        assert!(matches!(p("splitw -N 3 -d"), Cmd::SplitWindow { count: 3, detached: true, .. }));
        assert_eq!(p("splitw -N 3 -d").to_string(), "split-window -v -N 3 -d");
        for bad in ["splitw -N 0", "splitw -N 65", "splitw -N x", "splitw -N"] {
            assert!(parse_line(bad).is_err(), "{bad}");
        }
        assert!(parse_line("resize-pane").is_err());
        assert!(parse_line("select-pane -X").is_err());
    }

    #[test]
    fn pane_commands_take_a_target() {
        // tmux scripts address a pane from outside the session; every pane
        // command must accept -t, and print it back.
        let t = |s: &str| Some(Target::parse(s));
        assert_eq!(
            p("resize-pane -Z -t work:1"),
            Cmd::ResizePane { dir: None, amount: 1, zoom: true, target: t("work:1"), width: None, height: None }
        );
        assert_eq!(
            p("resizep -L 5 -t work:1.0"),
            Cmd::ResizePane {
                dir: Some(Dir::Left),
                amount: 5,
                zoom: false,
                target: t("work:1.0"),
                width: None,
                height: None
            }
        );
        assert_eq!(p("swap-pane -U -t work"), Cmd::SwapPane { up: true, target: t("work"), source: None });
        // A tmux layout string is one word to the parser, braces and all.
        let s = "bf3a,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]}";
        assert!(matches!(p(&format!("select-layout {s}")), Cmd::SelectLayout { name: Some(n), .. } if n == s));
        // The pair form: -s and -t are two panes; it prints back without -U/-D.
        let pair = p("swap-pane -s v:0.1 -t v:1.0");
        assert_eq!(pair, Cmd::SwapPane { up: false, target: t("v:1.0"), source: t("v:0.1") });
        assert_eq!(pair.to_string(), "swap-pane -s v:0.1 -t v:1.0");
        assert_eq!(p(&pair.to_string()), pair);
        assert_eq!(p("break-pane -t work:2"), Cmd::BreakPane { target: t("work:2") });
        assert_eq!(p("break-pane"), Cmd::BreakPane { target: None });
        assert_eq!(p("resize-pane -Z -t work:1").to_string(), "resize-pane -Z -t work:1");
        assert_eq!(p("swap-pane -D -t w:0.1").to_string(), "swap-pane -D -t w:0.1");
        assert_eq!(p("break-pane -t work:2").to_string(), "break-pane -t work:2");
        // Unknown flags are still rejected rather than silently dropped.
        assert!(parse_line("break-pane -x").is_err());
        assert!(parse_line("break-pane extra").is_err());
        assert!(parse_line("swap-pane -t").is_err());
    }

    #[test]
    fn window_navigation_takes_a_session_and_rejects_junk() {
        assert_eq!(p("next-window"), Cmd::NextWindow { target: None, alert: false });
        assert_eq!(p("next -t work"), Cmd::NextWindow { target: Some(Target::parse("work")), alert: false });
        assert_eq!(p("prev -t work"), Cmd::PreviousWindow { target: Some(Target::parse("work")), alert: false });
        assert_eq!(p("next-window -a"), Cmd::NextWindow { target: None, alert: true });
        assert_eq!(p("previous-window -a").to_string(), "previous-window -a");
        assert!(parse_line("last-window -a").is_err(), "last-window has no alert search");
        assert_eq!(p("last -t work"), Cmd::LastWindow { target: Some(Target::parse("work")) });
        assert_eq!(p("next-window -t work").to_string(), "next-window -t work");
        // Commands that take nothing say so instead of ignoring the argument.
        for junk in ["next-window -x", "paste-buffer -x", "list-keys extra", "version -v", "clear-history now"] {
            assert!(parse_line(junk).is_err(), "{junk} should be rejected");
        }
    }

    #[test]
    fn parse_nested_commands() {
        assert_eq!(
            p("confirm-before -p \"kill-window? (y/n)\" kill-window"),
            Cmd::ConfirmBefore {
                prompt: Some("kill-window? (y/n)".into()),
                cmd: Box::new(Cmd::KillWindow { target: None, all_but: false })
            }
        );
        assert_eq!(
            p("bind-key -n M-h select-pane -L"),
            Cmd::BindKey {
                table: KeyTable::Root,
                key: "M-h".into(),
                repeat: false,
                cmd: Box::new(Cmd::SelectPane { sel: PaneSel::Dir(Dir::Left) })
            }
        );
        assert_eq!(
            p("bind h \"select-pane -L\""),
            Cmd::BindKey {
                table: KeyTable::Prefix,
                key: "h".into(),
                repeat: false,
                cmd: Box::new(Cmd::SelectPane { sel: PaneSel::Dir(Dir::Left) })
            }
        );
        assert_eq!(
            p("bind -r C-h resize-pane -L 5"),
            Cmd::BindKey {
                table: KeyTable::Prefix,
                key: "C-h".into(),
                repeat: true,
                cmd: Box::new(Cmd::ResizePane {
                    dir: Some(Dir::Left),
                    amount: 5,
                    zoom: false,
                    target: None,
                    width: None,
                    height: None
                })
            }
        );
        assert_eq!(p("bind -r C-h resize-pane -L 5").to_string(), "bind-key -r C-h resize-pane -L 5");
        assert_eq!(
            p("set -g prefix C-a"),
            Cmd::SetOption { name: "prefix".into(), value: "C-a".into(), append: false, target: None }
        );
        assert_eq!(
            p("set -ag status-right \" | x\""),
            Cmd::SetOption { name: "status-right".into(), value: " | x".into(), append: true, target: None }
        );
        // `set -w -t v:1 synchronize-panes on`: the window it applies to.
        assert_eq!(
            p("set -w -t v:1 synchronize-panes on"),
            Cmd::SetOption {
                name: "synchronize-panes".into(),
                value: "on".into(),
                append: false,
                target: Some(Target::parse("v:1"))
            }
        );
        assert_eq!(p("set -w -t v:1 synchronize-panes on").to_string(), "set-option -t v:1 synchronize-panes on");
    }

    #[test]
    fn display_roundtrip() {
        let cmds = [
            "new-session -s main -n shell -d wsl.exe -d Ubuntu",
            "attach-session -d -t main",
            "split-window -h -c C:\\src",
            "select-pane -L",
            "resize-pane -Z",
            "resize-pane -U 3",
            "send-keys -t main:1 ls Enter",
            "send-keys -l Enter",
            "new-session -s x -d -A",
            "new-window -d -n w",
            "split-window -v -d -b -f",
            "kill-window -a -t :2",
            "kill-pane -a",
            "kill-session -a -t x",
            "capture-pane -p -S -100 -t w:1",
            "run-shell -b -t w:1 \"pwsh -c Get-Date\"",
            "set-hook -g after-new-window \"rename-window hooked\"",
            "set-hook -gu after-new-window",
            "load-plugin \"C:\\plugins\\demo\"",
            "list-plugins",
            "show-hooks -g",
            "show-options -g -v -q @x",
            "show-options -g",
            "save-session -a",
            "save-session -t work",
            "restore-session -a",
            "restore-session \"my session\"",
            "list-saved",
            "delete-saved old",
            "set-cwd",
            "set-cwd -t w:0.1 \"C:\\my dir\"",
            "confirm-before -p \"kill? (y/n)\" \"kill-window\"",
            "bind-key -n M-h select-pane -L",
            "set-option prefix C-a",
            "display-message \"hello world\"",
            "select-window -t :3",
            "rename-window \"my win\"",
            "copy-mode -u",
            "command-prompt -p (rename-window) -I \"#W\" \"rename-window -- %%\"",
            "capture-pane -p -e -t w:1",
            "capture-pane -p -e -J -t w:1",
            "select-layout -E -t w:1",
            "choose-client",
            "pipe-pane -o -t w:1 \"cat > log\"",
            "pipe-pane",
            "wait-for -S done",
            "wait-for -L lock",
            "wait-for chan",
            "display-popup -E -w 50% -h 20 -d C:\\src pwsh.exe",
            "display-popup -C",
            "display-menu -T pane Split h \"split-window -h\" \"\" Kill x kill-pane",
            "next-window -a",
            "previous-window -a",
        ];
        for c in cmds {
            let parsed = p(c);
            let shown = parsed.to_string();
            assert_eq!(p(&shown), parsed, "roundtrip of {c:?} via {shown:?}");
        }
    }

    #[test]
    fn parse_menus_pipes_and_waiting() {
        // A menu is triples of name/key/command; an empty name on its own is
        // a separator, as in tmux.
        let Cmd::DisplayMenu { title, items } =
            p("display-menu -T \"pane #P\" Split h \"split-window -h\" \"\" Kill x kill-pane")
        else {
            panic!("not a menu")
        };
        assert_eq!(title.as_deref(), Some("pane #P"));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "Split");
        assert_eq!(items[0].key.as_deref(), Some("h"));
        assert_eq!(
            items[0].cmd.as_deref(),
            Some(&Cmd::SplitWindow {
                horizontal: true,
                cwd: None,
                target: None,
                argv: Vec::new(),
                detached: false,
                before: false,
                full: false,
                count: 1,
            })
        );
        assert!(items[1].cmd.is_none() && items[1].name.is_empty(), "the separator: {:?}", items[1]);
        assert_eq!(items[2].name, "Kill");
        // A half-written item is an error, not a silently dropped entry.
        assert!(parse_line("display-menu Split h").is_err());
        assert!(parse_line("display-menu").is_err());
        // The nested command is parsed now, not when the key is pressed.
        assert!(parse_line("display-menu Bad k frobnicate").is_err());

        assert_eq!(
            p("pipe-pane -o -t x cat"),
            Cmd::PipePane {
                target: Some(Target::parse("x")),
                command: Some("cat".into()),
                toggle: true,
                input: false,
                output: true
            }
        );
        assert_eq!(
            p("pipe-pane"),
            Cmd::PipePane { target: None, command: None, toggle: false, input: false, output: true }
        );
        // -I, -O and both; -O alone is the default and prints back as nothing.
        let i = p("pipe-pane -I cmd.exe /c echo hi");
        assert!(matches!(i, Cmd::PipePane { input: true, output: false, .. }), "{i:?}");
        assert_eq!(i.to_string(), "pipe-pane -I \"cmd.exe /c echo hi\"");
        assert!(matches!(p("pipe-pane -O x"), Cmd::PipePane { input: false, output: true, .. }));
        let both = p("pipe-pane -IO x");
        assert!(matches!(both, Cmd::PipePane { input: true, output: true, .. }));
        assert_eq!(both.to_string(), "pipe-pane -I -O x");
        assert_eq!(p(&both.to_string()), both);

        assert_eq!(p("wait-for -S x"), Cmd::WaitFor { channel: "x".into(), lock: false, unlock: false, signal: true });
        assert!(parse_line("wait-for").is_err());
        assert!(parse_line("wait-for -L -U x").unwrap_err().contains("exclusive"));
        assert!(parse_line("wait-for a b").is_err());

        // Prefixes and aliases reach the new commands.
        assert!(matches!(p("menu Kill x kill-pane"), Cmd::DisplayMenu { .. }));
        assert!(matches!(p("popup -E pwsh.exe"), Cmd::DisplayPopup { close_on_exit: true, .. }));
        assert!(matches!(p("pipep cat"), Cmd::PipePane { .. }));
        assert!(matches!(p("wait x"), Cmd::WaitFor { .. }));
        assert!(matches!(p("choose-c"), Cmd::ChooseClient));
        assert!(matches!(p("choose-j"), Cmd::ChooseJobs));
        assert!(matches!(p("focus-pane %7"), Cmd::FocusPane { pane: 7 }));
        // select-pane takes a full target as well as a bare pane number.
        assert!(matches!(p("select-pane -t 2"), Cmd::SelectPane { sel: PaneSel::Index(2) }));
        match p("select-pane -t v:0.1") {
            Cmd::SelectPane { sel: PaneSel::Target(t) } => assert_eq!(t, Target::parse("v:0.1")),
            other => panic!("{other:?}"),
        }
        assert_eq!(p("select-pane -t v:0.1").to_string(), "select-pane -t v:0.1");
        assert!(matches!(p("select-pane -t :.1"), Cmd::SelectPane { sel: PaneSel::Target(_) }));
        assert!(matches!(p("focusp 7"), Cmd::FocusPane { pane: 7 }));
        assert_eq!(p("focus-pane 7").to_string(), "focus-pane %7");
        assert!(parse_line("focus-pane").unwrap_err().contains("pane id required"));
        assert!(parse_line("focus-pane %x").unwrap_err().contains("bad pane id"));
        assert!(parse_line("focus-pane %1 %2").unwrap_err().contains("unexpected argument"));
        assert!(parse_line("choose-jobs extra").unwrap_err().contains("unexpected argument"));
        // Commands that exist must be in the table, or a prefix of them is
        // "unknown" and `list-commands` does not mention them.
        assert!(matches!(p("copy-m"), Cmd::CopyMode { .. }));
        assert!(matches!(p("unb x"), Cmd::UnbindKey { .. }));
        assert!(matches!(p("start-s"), Cmd::StartServer));
        // tmux's window-scoped spellings are the same command here.
        assert_eq!(p("setw -g mode-keys vi"), p("set -g mode-keys vi"));
        // The three key tables; copy-mode (emacs) is taken as copy-mode-vi,
        // and a table keepane does not have is refused, not bound elsewhere.
        assert!(matches!(p("bind -T root M-x kill-pane"), Cmd::BindKey { table: KeyTable::Root, .. }));
        assert!(matches!(p("bind -T prefix x kill-pane"), Cmd::BindKey { table: KeyTable::Prefix, .. }));
        let vi = p("bind -T copy-mode-vi v send -X begin-selection");
        assert!(
            matches!(&vi, Cmd::BindKey { table: KeyTable::Copy, cmd, .. } if matches!(**cmd, Cmd::CopyCommand { .. }))
        );
        assert_eq!(vi.to_string(), "bind-key -T copy-mode-vi v send-keys -X begin-selection");
        assert_eq!(p(&vi.to_string()), vi, "list-keys output reads back");
        assert!(matches!(p("bind -T copy-mode y send -X copy-selection"), Cmd::BindKey { table: KeyTable::Copy, .. }));
        assert!(matches!(p("unbind -T copy-mode-vi v"), Cmd::UnbindKey { table: KeyTable::Copy, .. }));
        let e = parse_line("bind -T off-pane x kill-pane").unwrap_err();
        assert!(e.contains("off-pane") && e.contains("not supported"), "{e}");
        assert!(matches!(p("showw -gv mouse"), Cmd::ShowOptions { .. }));
        for name in COMMANDS {
            // Listed means the parser knows it (it may still want arguments)
            // and the name resolves to itself rather than being ambiguous.
            let err = parse_line(name).err().unwrap_or_default();
            assert!(!err.contains("unknown command"), "{name}: {err}");
            assert_eq!(resolve_prefix(name).unwrap(), *name, "{name} does not resolve to itself");
        }
        // "display" alone is the message command, as in tmux.
        assert!(matches!(p("display hi"), Cmd::DisplayMessage { .. }));
        // -t names the pane the format is answered for; -p and -c x are
        // taken (with -c's value) and never end up in the message.
        match p("display-message -p -c c0 -t small:0 '#{pane_width}'") {
            Cmd::DisplayMessage { msg, target: Some(t) } => {
                assert_eq!(msg, "#{pane_width}");
                assert_eq!(t, Target::parse("small:0"));
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_line("display-message -t").unwrap_err().contains("-t"));
        assert!(parse_line("displ hi").unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn unknown_command() {
        assert!(parse_line("frobnicate").is_err());
        assert_eq!(parse_line("").unwrap(), None);
    }
}
