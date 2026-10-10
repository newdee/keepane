//! The keepane server: owns sessions, windows and panes; talks to clients over a
//! named pipe; renders frames.

pub mod actor;
mod agents;
mod clock;
mod digits;
mod done;
mod hints;
pub mod import;
pub mod input;
mod keyhint;
pub mod layout;
mod link_in;
mod link_out;
mod link_state;
mod link_tree;
mod mail;
mod newer;
pub mod observe;
pub mod pane;
mod prices;
mod public_ip;
pub mod render;
mod shellhist;
mod web_fit;
mod web_host;

use crate::command::{Cmd, Dir, MenuItem, PaneSel, Target};
use crate::config::{Options, resolve_shell};
use crate::ipc::{ClientMsg, KeyRecord, MouseRecord, PROTOCOL_VERSION, ServerMsg, read_frame, write_frame};
use crate::keys::{Key, KeyCode, key_from_record};
use anyhow::{Context, Result};
use input::MouseButton;
use layout::{Node, PaneId, Rect};
use pane::{CopyMode, Pane, PaneEvent};
use render::{CopyView, Frame, Grid, PaneView, StatusLine};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

/// Where a new pane's PowerShell keeps its command history.
enum ShellHistory {
    /// PowerShell's own shared file: a popup, gone in a moment.
    Shared,
    /// A resumed pane's own file, by name.
    Keep(String),
    /// A file of its own, starting as a copy of this pane's (or of the shared
    /// file).
    From(Option<PaneId>),
}

/// Remove the `.txt` history files in `dir` last written before `cutoff` whose
/// name is not in `keep`. Nothing else in `dir` is touched.
fn prune_shell_history_files(dir: &std::path::Path, keep: &HashSet<String>, cutoff: SystemTime) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        let stale = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < cutoff);
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if stale && path.extension().is_some_and(|x| x == "txt") && !keep.contains(&name) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// A status line's three parts, expanded: `status-left`, each window's
/// label with whether it is the current window, `status-right`.
type StatusParts = (Vec<crate::format::Segment>, Vec<(Vec<crate::format::Segment>, bool)>, Vec<crate::format::Segment>);

pub type ClientId = u32;
pub type SessionId = u32;
pub type WindowId = u32;

const MOUSE_MOVED: u32 = 0x0001;
const MOUSE_WHEELED: u32 = 0x0004;
const MOUSE_HWHEELED: u32 = 0x0008;
const BTN_LEFT: u32 = 0x0001;
const BTN_RIGHT: u32 = 0x0002;
const BTN_MIDDLE: u32 = 0x0004;

/// Frames queued per client before we stop diffing and fall back to a full
/// redraw once the client catches up (a slow console must not make the
/// server's memory grow without bound).
const OUTPUT_QUEUE: usize = 64;

enum Event {
    Pane(PaneEvent),
    Connected(ClientId, mpsc::Sender<ServerMsg>),
    Msg(ClientId, ClientMsg),
    Gone(ClientId),
    Tick,
    /// A `run-shell` finished: deliver its output to the client that asked.
    ShellDone {
        cid: Option<ClientId>,
        output: String,
        code: i32,
    },
    /// A status-line `#(command)` finished.
    StatusShell {
        command: String,
        output: String,
    },
    /// Windows is shutting down or the user is logging off: save every
    /// session now and tell the watcher (which is holding Windows up).
    EndSession(std::sync::mpsc::Sender<()>),
    /// keepane's prompt came back in a pane a moment ago, and what the
    /// command printed has been drawn by now (see `PROMPT_SETTLE`).
    PromptSettled(PaneId),
    /// A prefix may have waited for its next key long enough for its panel
    /// (`prefix-hint`): only a frame, which draws the panel if so (the
    /// drawing checks the time, so a prefix answered since draws nothing).
    PrefixHintDue,
    /// A selection dragged to a pane's edge scrolls a line more, when the
    /// pointer is still there (`Selecting`).
    DragScroll(ClientId),
    /// The daily look for a newer keepane came back (`newer`).
    NewerChecked(Option<String>),
    /// The address the internet sees this machine at came back (`public_ip`).
    PublicIp(Option<String>),
    /// The panes' agents were looked at (`agents`).
    AgentStats(HashMap<PaneId, agents::Stats>),
    /// The price list was fetched, or could not be (`prices`).
    Prices(Option<Box<prices::Prices>>),
    /// A request to the phones' page told who asked (`web_host`).
    Web(u64, crate::web::Seen),
    /// `web-stop`'s task has ended: answer the client that asked.
    WebStopped(ClientId),
    /// Another machine answered a request of ours, or could not (`link_out`).
    LinkAnswer(Box<link_out::LinkAnswer>),
    /// `done-webhook` could not be told (`done`): why.
    DoneWebhookFailed(String),
}

/// Runs `ask` on a thread of its own and sends its answer as `answer`. A
/// panic in it answers None: a flag set while the answer is awaited
/// (`asking`) is never left set for good.
fn ask_on_thread<T: Send + 'static>(
    events: &mpsc::UnboundedSender<Event>,
    ask: impl FnOnce() -> Option<T> + Send + 'static,
    answer: fn(Option<T>) -> Event,
) {
    let tx = events.clone();
    std::thread::spawn(move || {
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ask)).unwrap_or(None);
        let _ = tx.send(answer(got));
    });
}

/// How long after keepane's prompt marker a pane's command is taken as
/// done. ConPTY passes the marker (an OSC) through at once but draws the
/// text before it on its next frame, so at the marker the command's last
/// output may not be on the screen yet (seen: keepane-cmd and 9;9 arriving
/// ahead of the output the prompt hook wrote after it).
const PROMPT_SETTLE: Duration = Duration::from_millis(60);

/// A key record that is someone typing: a key pressed, not one let go (the
/// Enter that started `keepane attach` is let go in the pane it attached to)
/// and not Shift, Ctrl, Alt, Win or Caps Lock pressed on its own.
fn typing(rec: &crate::ipc::KeyRecord) -> bool {
    rec.down && !matches!(rec.vk, 0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Hooks a plugin can attach commands to (`set-hook -g <name> <command>`).
pub const HOOKS: &[&str] = &[
    "after-new-session",
    "after-new-window",
    "after-split-window",
    "after-select-window",
    "after-select-pane",
    "after-kill-pane",
    "client-attached",
    "client-detached",
    "pane-exited",
    "pane-done",
];

enum PromptKind {
    /// Run the input as a command line, or substitute it into a template.
    Command { template: Option<String> },
    /// Boxed: a `Cmd` is many times the size of the other kinds.
    Confirm(Box<Cmd>),
    /// `x` or `D` on picker lines with a session among them: on `y`, they
    /// go (and with `forget`, the sessions' saves too).
    ConfirmPicker { targets: Vec<ChooserItem>, forget: bool },
    /// `/` or `?` in copy mode: the input is a pattern, not a command.
    Search { back: bool },
    /// `f` in a picker: the input filters its lines as it is typed;
    /// Escape puts `prev` back.
    Filter { prev: String },
}

struct Prompt {
    kind: PromptKind,
    label: String,
    input: String,
    cursor: usize,
    /// Shown in place of the label until the next key: what Tab found.
    hint: Option<String>,
}

struct Drag {
    pane: PaneId,
    horizontal: bool,
    last: i32,
}

/// A selection being dragged in a pane's copy mode: the pane keeps the
/// mouse until the button goes up, wherever the pointer is; at its top or
/// bottom row, or past it, the pane scrolls (`edge`: 1 up, -1 down, 0 not).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selecting {
    pane: PaneId,
    edge: i32,
    /// A `DragScroll` is on its way: one at a time.
    ticking: bool,
}

/// While the pointer stays at a selected pane's edge, it scrolls a line
/// this often.
const DRAG_SCROLL: Duration = Duration::from_millis(80);

/// The `choose-tree` picker (prefix `w` / `s`): a list of sessions, each
/// followed by its windows when `expand` is set. `items` and `lines` are
/// rebuilt from the live sessions at every render; the selection follows the
/// item's identity, so windows appearing or vanishing meanwhile do not move it.
/// What one line of the picker stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChooserItem {
    /// A session line, or one of its windows.
    Tree(SessionId, Option<WindowId>),
    /// A paste buffer, by position in the buffer list.
    Buffer(usize),
    /// An attached client.
    Client(ClientId),
    /// An entry of a `display-menu`, by position.
    Menu(usize),
    /// A pane on the task board (`choose-jobs`).
    Job(SessionId, WindowId, PaneId),
    /// A pane position in the history picker, or one of its days.
    Log(usize, Option<usize>),
    /// A title or separator line: shown, never selected.
    Separator,
    /// A machine in the chart: this one (None), or a paired one by its
    /// place in the table.
    Host(Option<u16>),
    /// A session, window or pane of a paired machine: its place in the
    /// table, then its numbers there.
    Remote(u16, u32, Option<u32>, Option<u32>),
}

impl ChooserItem {
    /// How deep it is in the session tree: the root (`keepane`, a
    /// separator) 0, a session 1, a window 2, a pane 3; none for the
    /// other pickers' lines.
    fn depth(&self) -> Option<usize> {
        match self {
            ChooserItem::Separator => Some(0),
            ChooserItem::Tree(_, None) => Some(1),
            ChooserItem::Tree(_, Some(_)) => Some(2),
            ChooserItem::Job(..) => Some(3),
            ChooserItem::Host(_) => Some(1),
            ChooserItem::Remote(_, _, None, _) => Some(2),
            ChooserItem::Remote(_, _, Some(_), None) => Some(3),
            ChooserItem::Remote(..) => Some(4),
            _ => None,
        }
    }

    /// The depths of a list of items: with machines in it (the chart, when
    /// this machine is paired), this machine's sessions, windows and panes
    /// are a level further down, under it.
    fn depths(items: &[ChooserItem]) -> Vec<Option<usize>> {
        let hosts = items.iter().any(|i| matches!(i, ChooserItem::Host(_)));
        items
            .iter()
            .map(|i| match i {
                ChooserItem::Tree(..) | ChooserItem::Job(..) if hosts => i.depth().map(|d| d + 1),
                _ => i.depth(),
            })
            .collect()
    }

    /// The session or window line that folds it: itself, or a pane's window.
    fn fold_target(&self) -> Option<ChooserItem> {
        match *self {
            ChooserItem::Tree(..) => Some(*self),
            ChooserItem::Job(s, w, _) => Some(ChooserItem::Tree(s, Some(w))),
            _ => None,
        }
    }
}

/// Where a picker's lines come from.
enum ChooserKind {
    /// The live session tree; `expand` shows each session's windows and
    /// their panes; `style` is how it is drawn (`v` goes to the next).
    Tree { expand: bool, style: TreeStyle },
    /// The paste buffers.
    Buffers,
    /// The attached clients.
    Clients,
    /// Every pane on the server with its state (`jobs`), to jump to, kill
    /// or restart.
    Jobs,
    /// A fixed list of windows (the `find-window` hits).
    Found,
    /// A `display-menu` and the commands its entries run.
    Menu(Vec<MenuItem>),
    /// The history log on disk (`choose-history`), read when opened; the
    /// positions in `open` show their days.
    History { kept: Vec<crate::histlog::Kept>, open: HashSet<usize> },
}

/// How the session tree is drawn (`choose-tree-style`): a chart of rows
/// under a `keepane` root (sessions, then the chosen session's windows,
/// then the chosen window's panes), a tree of blocks, or tmux's list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TreeStyle {
    Chart,
    Tree,
    List,
}

impl TreeStyle {
    fn parse(name: &str) -> TreeStyle {
        match name {
            "tree" => TreeStyle::Tree,
            "list" => TreeStyle::List,
            _ => TreeStyle::Chart,
        }
    }

    /// What `v` turns it into.
    fn next(self) -> TreeStyle {
        match self {
            TreeStyle::Chart => TreeStyle::Tree,
            TreeStyle::Tree => TreeStyle::List,
            TreeStyle::List => TreeStyle::Chart,
        }
    }
}

impl ChooserKind {
    /// Whether the list is rebuilt from the server state at every render.
    fn live(&self) -> bool {
        matches!(self, ChooserKind::Tree { .. } | ChooserKind::Buffers | ChooserKind::Clients | ChooserKind::Jobs)
    }
}

/// `hints` on screen (prefix F).
struct HintMode {
    /// Each pick: the pane it is in, what was found, its label (the same
    /// text has the same label wherever it is).
    found: Vec<(PaneId, hints::Found, String)>,
    /// The label's letters typed so far.
    typed: String,
    /// A capital was typed: open the pick rather than copy it.
    open: bool,
}

/// The digits typed while the `display-panes` numbers are up.
#[derive(Clone, Copy)]
struct PanesTyped {
    typed: digits::Typed,
    /// The active and last pane before the first digit.
    before: (PaneId, Option<PaneId>),
}

struct Chooser {
    kind: ChooserKind,
    items: Vec<ChooserItem>,
    lines: Vec<String>,
    sel: usize,
    top: usize,
    /// Only lines holding this (case-insensitively) are shown; empty shows all.
    filter: String,
    /// Items marked with `t`, which `x` (and `r`) act on instead of the
    /// current one. Kept by identity, so a live rebuild does not lose them.
    tagged: Vec<ChooserItem>,
    /// Sessions and windows folded with `-` in the tree: what is under them
    /// is not listed until `+` opens them again.
    collapsed: Vec<ChooserItem>,
    /// The line number being typed (`1` `2` is line 12); any other key ends it.
    typed: Option<digits::Typed>,
    /// For each item, whether it is its parent's current one (a session's
    /// current window, a window's active pane): the branch the chart shows.
    active: Vec<bool>,
    /// The chart shows every node, not only the selected branch (`a`).
    all: bool,
}

impl Chooser {
    fn new(kind: ChooserKind, items: Vec<ChooserItem>, lines: Vec<String>, sel: usize) -> Chooser {
        Chooser {
            kind,
            items,
            lines,
            sel,
            top: 0,
            filter: String::new(),
            tagged: Vec::new(),
            collapsed: Vec::new(),
            typed: None,
            active: Vec::new(),
            all: false,
        }
    }

    fn is_chart(&self) -> bool {
        matches!(self.kind, ChooserKind::Tree { style: TreeStyle::Chart, .. })
    }

    fn is_tagged(&self, item: &ChooserItem) -> bool {
        self.tagged.contains(item)
    }

    /// The items an action key works on: the tagged ones, else the current.
    fn targets(&self) -> Vec<ChooserItem> {
        if !self.tagged.is_empty() {
            return self.tagged.clone();
        }
        self.items.get(self.sel).copied().into_iter().collect()
    }

    fn step(&mut self, delta: i64) {
        let last = self.items.len().saturating_sub(1) as i64;
        let from = self.sel;
        self.sel = (self.sel as i64 + delta).clamp(0, last) as usize;
        if !self.settle(if delta < 0 { -1 } else { 1 }) {
            self.sel = from; // nothing selectable anywhere: do not move at all
        }
    }

    /// Move off a title or separator line, preferring direction `dir` and
    /// falling back to the other one. False when the list has nothing that
    /// can be selected.
    fn settle(&mut self, dir: i64) -> bool {
        let last = self.items.len().saturating_sub(1) as i64;
        for d in [dir, -dir] {
            let mut i = self.sel as i64;
            while (0..=last).contains(&i) {
                if !matches!(self.items.get(i as usize), Some(ChooserItem::Separator)) {
                    self.sel = i as usize;
                    return true;
                }
                i += d;
            }
        }
        false
    }
}

/// A `display-popup`: its own pane, drawn in a box over the client's window
/// and fed every key until it closes. The size is kept as it was asked for so
/// the box follows the terminal when it is resized.
/// A pane or window a command killed, kept for `undo-kill-time` seconds
/// with its programs still running, so `undo-kill` can put it back as it
/// was. Dropping it ends it for good.
enum Killed {
    Pane {
        pane: Box<Pane>,
        session: SessionId,
        window: WindowId,
        /// The window's layout with the pane still in it.
        layout: Node,
        at: Instant,
    },
    Window {
        window: Window,
        session: SessionId,
        index: usize,
        at: Instant,
    },
}

impl Killed {
    fn at(&self) -> Instant {
        match self {
            Killed::Pane { at, .. } | Killed::Window { at, .. } => *at,
        }
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Killed::Pane { pane, .. } => (pane.id == id).then_some(&mut **pane),
            Killed::Window { window, .. } => window.pane_mut(id),
        }
    }
}

struct Popup {
    pane: Pane,
    rect: Rect,
    width: Option<String>,
    height: Option<String>,
    x: Option<String>,
    y: Option<String>,
    /// `-E`: go away as soon as the command finishes.
    close_on_exit: bool,
    /// The command has finished and the next key closes the box.
    finished: bool,
}

struct Client {
    id: ClientId,
    tx: mpsc::Sender<ServerMsg>,
    cols: u16,
    rows: u16,
    cwd: String,
    interactive: bool,
    pane_env: Option<u32>,
    session: Option<SessionId>,
    /// Session this client was in before the current one (switch-client -l).
    last_session: Option<SessionId>,
    last_grid: Option<Grid>,
    last_cursor: Option<(u16, u16)>,
    prefix: bool,
    /// When the prefix was pressed, while it waits for its next key: the
    /// panel of what each key does shows from `prefix-hint-delay` after.
    prefix_at: Option<Instant>,
    /// A `bind -r` key ran; until this instant its table answers bare keys.
    repeat_until: Option<Instant>,
    /// `display-panes` is showing the pane numbers until this instant.
    panes_until: Option<Instant>,
    /// The pane number typed while they show, when another digit could
    /// still make a bigger one (`1` with pane 10 up).
    panes_typed: Option<PanesTyped>,
    /// `hints` is up: the picks on screen and what has been typed.
    hints: Option<HintMode>,
    prompt: Option<Prompt>,
    message: Option<(String, Instant)>,
    /// Multi-line command output shown over the window until a key is pressed
    /// (tmux "view mode"), e.g. `list-keys`.
    overlay: Option<Vec<String>>,
    chooser: Option<Chooser>,
    /// `display-popup`: a program in a box over the window, taking the keys.
    popup: Option<Popup>,
    mouse_buttons: u32,
    drag: Option<Drag>,
    /// A selection being dragged (`Selecting`).
    selecting: Option<Selecting>,
    /// Virtual keys whose key-down we consumed; drop the matching key-up.
    swallow_up: HashSet<u16>,
    /// A bell from a background window, to ring at the next render.
    pending_bell: bool,
    /// Control messages that did not fit in the output queue yet.
    pending: std::collections::VecDeque<ServerMsg>,
    /// Status-line column ranges of the window labels last drawn (clicks).
    window_hits: Vec<(u16, u16)>,
    /// When the client connected, and when it last sent a key
    /// (`#{client_created}`, `#{client_activity}`).
    created: Instant,
    last_activity: Instant,
    /// This client's view of a window bigger than it is: the window column
    /// and row at its top-left corner (tmux's per-client offset, moved with
    /// `refresh-client -U/-D/-L/-R`; it follows the active pane's cursor).
    view_x: u16,
    view_y: u16,
    /// The view was panned by hand: leave it there until the next key goes
    /// to a pane, when following the cursor makes sense again.
    view_pinned: bool,
    /// A key just went to the pane: the next render brings the cursor into
    /// view. Otherwise the view only moves once the cursor has been out of
    /// it for a moment (since `cursor_out`), so a program's momentary cursor
    /// hop (a prompt being redrawn from column 0) does not drag it about.
    view_follow: bool,
    cursor_out: Option<Instant>,
}

impl Client {
    /// Queue a control message. Control messages are never dropped: when the
    /// queue is full (a console frozen by a QuickEdit selection blocks our
    /// writer) they wait in `pending` and go out before any further frame.
    fn send(&mut self, m: ServerMsg) {
        if !self.pending.is_empty() {
            self.pending.push_back(m);
            return;
        }
        if let Err(mpsc::error::TrySendError::Full(m)) = self.tx.try_send(m) {
            self.pending.push_back(m);
        }
    }

    /// Queue a screen update. When the client lags, the frame is dropped and
    /// the next render is forced to be a full redraw, which supersedes it.
    fn send_output(&mut self, bytes: Vec<u8>) {
        if !self.pending.is_empty() || self.tx.try_send(ServerMsg::Output(bytes)).is_err() {
            self.last_grid = None;
            self.last_cursor = None;
        }
    }

    /// Retry queued control messages; returns true when none are left.
    fn flush_pending(&mut self) -> bool {
        while let Some(m) = self.pending.pop_front() {
            if let Err(mpsc::error::TrySendError::Full(m)) = self.tx.try_send(m) {
                self.pending.push_front(m);
                return false;
            }
        }
        true
    }
}

struct Window {
    id: WindowId,
    name: String,
    layout: Node,
    panes: Vec<Pane>,
    active: PaneId,
    last_pane: Option<PaneId>,
    zoomed: bool,
    /// Last layout applied by `select-layout`, so `-n` knows where to go next.
    layout_preset: Option<layout::Preset>,
    /// `synchronize-panes`: input goes to every pane of the window.
    synchronized: bool,
    /// Where each pane's content is drawn: its layout cell less the row a
    /// `pane-border-status` line takes.
    rects: Vec<(PaneId, Rect)>,
    /// The layout cells themselves, which is what splitting divides.
    layout_rects: Vec<(PaneId, Rect)>,
    /// Alerts waiting to be seen (tmux `#F`): a background window printed
    /// something, rang the bell, or went quiet. Cleared when the window is
    /// drawn, which is when the alert has done its job.
    alert_activity: bool,
    alert_bell: bool,
    alert_silence: bool,
    /// A pane in `normal` work mode got a message (`@` in `#F`).
    alert_mail: bool,
    /// When this window last printed anything (`monitor-silence`).
    last_output: Instant,
    /// The focus frame moving to where the eye should go (`animation`).
    anim: Option<Anim>,
}

/// A focus-frame animation (`animation`): a rounded frame that moves and
/// grows from where the eye was to where it now is, drawn over content
/// that is already there, so nothing waits for it.
#[derive(Clone, Copy, Debug)]
enum Anim {
    /// From one rectangle to another: a pane selected, a pane zoomed or
    /// unzoomed.
    Move { from: Rect, to: Rect, start: Instant },
    /// Into a window just switched to: from the whole window down to its
    /// active pane, or out from the middle when it has only the one.
    Enter { start: Instant },
    /// A pane zooming in or out: the pane itself grows from its place in
    /// the layout to the whole window, or shrinks back, over the other
    /// panes as they are laid out; an edge on the window's edge stays put.
    Zoom { pane: PaneId, from: Rect, to: Rect, start: Instant },
}

impl Anim {
    fn start(&self) -> Instant {
        match self {
            Anim::Move { start, .. } | Anim::Enter { start } | Anim::Zoom { start, .. } => *start,
        }
    }
}

impl Window {
    /// The `#F` flags: where the window sits, what it is waiting to tell you,
    /// and how it is laid out — in tmux's order.
    fn flags(&self, current: bool, last: bool) -> String {
        let mut s = String::new();
        if current {
            s.push('*');
        } else if last {
            s.push('-');
        }
        if self.alert_activity {
            s.push('#');
        }
        if self.alert_bell {
            s.push('!');
        }
        if self.alert_silence {
            s.push('~');
        }
        if self.alert_mail {
            s.push('@');
        }
        if self.zoomed {
            s.push('Z');
        }
        if self.synchronized {
            s.push('S');
        }
        s
    }

    fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }
    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }
    fn active_pane(&self) -> Option<&Pane> {
        self.pane(self.active)
    }
    fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        let id = self.active;
        self.pane_mut(id)
    }
    fn rect_of(&self, id: PaneId) -> Option<Rect> {
        self.rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    /// Start the focus frame moving from `from` to `to` (both known and
    /// not the same place).
    fn animate(&mut self, from: Option<Rect>, to: Option<Rect>) {
        if let (Some(from), Some(to)) = (from, to)
            && from != to
        {
            self.anim = Some(Anim::Move { from, to, start: Instant::now() });
        }
    }
    /// The pane's layout cell, border-status row included: what a split
    /// divides. Dividing the drawn rect instead leaves the tree one row
    /// short, and the layout hands that row to the first pane (11/9, not
    /// 10/10).
    fn layout_rect(&self, id: PaneId) -> Option<Rect> {
        self.layout_rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }
    fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.rects.iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| *id)
    }

    /// Recompute pane rectangles for a window area and resize the panes.
    /// `border`: `pane-border-status` wants a row above (`Some(true)`) or
    /// below (`Some(false)`) every pane, which is taken off the pane itself
    /// and left for the border text.
    /// The panes' rectangles as they are without the zoom, border rows
    /// taken off as `relayout` does, for drawing what a zoom grows over.
    fn unzoomed_rects(&self, area: Rect, border: Option<bool>) -> Vec<(PaneId, Rect)> {
        let mut rects = Vec::new();
        self.layout.clone().layout(area, &mut rects);
        if let Some(top) = border {
            for (_, r) in &mut rects {
                if r.h >= 2 {
                    if top {
                        r.y += 1;
                    }
                    r.h -= 1;
                }
            }
        }
        rects
    }

    /// Start a zoom animation of pane `pane` from `from` to `to`.
    fn animate_zoom(&mut self, pane: PaneId, from: Option<Rect>, to: Option<Rect>) {
        if let (Some(from), Some(to)) = (from, to)
            && from != to
        {
            self.anim = Some(Anim::Zoom { pane, from, to, start: Instant::now() });
        }
    }

    fn relayout(&mut self, area: Rect, border: Option<bool>) {
        self.rects.clear();
        if self.zoomed && self.pane(self.active).is_some() {
            self.rects.push((self.active, area));
        } else {
            self.zoomed = false;
            self.layout.layout(area, &mut self.rects);
        }
        self.layout_rects = self.rects.clone();
        // `pane-border-status`: a pane's text goes on the border line above
        // (or below) it. Between two panes that is the line that parts them,
        // as in tmux; only a pane at the window's edge, with no line there,
        // gives up a row for it. (Every pane giving one up put two border
        // rows between panes, the upper one drawn as a row of ┬.)
        if let Some(top) = border {
            for (_, r) in &mut self.rects {
                let at_edge = if top { r.y == area.y } else { r.y + r.h == area.y + area.h };
                if at_edge && r.h >= 2 {
                    if top {
                        r.y += 1;
                    }
                    r.h -= 1;
                }
            }
        }
        for (id, r) in self.rects.clone() {
            if let Some(p) = self.pane_mut(id) {
                p.resize(r.w, r.h);
            }
        }
    }
}

struct Session {
    id: SessionId,
    name: String,
    windows: Vec<Window>,
    cur: usize,
    last: Option<WindowId>,
    cols: u16,
    rows: u16,
    created: Instant,
    /// Unix time of creation, for `#{session_created}`.
    created_at: i64,
    last_used: Instant,
}

impl Session {
    fn window(&self) -> Option<&Window> {
        self.windows.get(self.cur)
    }
    fn window_mut(&mut self) -> Option<&mut Window> {
        self.windows.get_mut(self.cur)
    }
    fn select_window(&mut self, idx: usize) {
        if idx < self.windows.len() && idx != self.cur {
            self.last = self.windows.get(self.cur).map(|w| w.id);
            self.cur = idx;
            self.windows[idx].anim = Some(Anim::Enter { start: Instant::now() });
        }
    }
}

/// What a key runs, and whether it keeps running without the prefix for
/// `repeat-time` afterwards (tmux `bind -r`).
#[derive(Clone)]
struct Binding {
    cmd: Cmd,
    repeat: bool,
}

enum Outcome {
    Ok,
    Text(String),
    Attach(SessionId),
    Error(String),
    /// The result arrives later as an event (`run-shell`); nothing to send yet.
    Pending,
}

impl From<Result<(), String>> for Outcome {
    fn from(r: Result<(), String>) -> Outcome {
        match r {
            Ok(()) => Outcome::Ok,
            Err(e) => Outcome::Error(e),
        }
    }
}

/// One `wait-for` channel: a lock with its queue, and the wait/signal pair.
/// A client that is waiting has had no reply sent yet, so its `keepane wait-for`
/// is still sitting there; waking it means answering that command.
#[derive(Default)]
struct WaitChannel {
    locked: bool,
    /// Clients blocked in `wait-for -L`, in arrival order.
    lockers: std::collections::VecDeque<ClientId>,
    /// Clients blocked in a plain `wait-for`.
    waiters: Vec<ClientId>,
    /// A signal arrived with nobody waiting: the next wait returns at once
    /// (tmux keeps the same one-shot flag).
    woken: bool,
}

impl WaitChannel {
    fn idle(&self) -> bool {
        !self.locked && !self.woken && self.lockers.is_empty() && self.waiters.is_empty()
    }
}

pub struct Server {
    opts: Options,
    prefix_binds: HashMap<Key, Binding>,
    root_binds: HashMap<Key, Binding>,
    /// `bind -T copy-mode-vi`: keys in copy mode that run a command instead
    /// of copy mode's own meaning.
    copy_binds: HashMap<Key, Binding>,
    sessions: Vec<Session>,
    clients: HashMap<ClientId, Client>,
    next_id: u32,
    pane_tx: std::sync::mpsc::Sender<PaneEvent>,
    socket: String,
    had_session: bool,
    started: Instant,
    /// When old history log files were last cleared out (once a day).
    history_pruned: Option<Instant>,
    /// `log-history` at the last tick, to tell when it was turned on.
    history_was_on: bool,
    /// What `kill-pane` / `kill-window` took in the last `undo-kill-time`
    /// seconds, newest last, for `undo-kill`.
    killed: Vec<Killed>,
    quit: bool,
    /// Restore saved sessions at start even with `restore-on-start off`.
    force_restore: bool,
    /// A config file given at start, in place of the usual search.
    config_override: Option<PathBuf>,
    /// Files being sourced right now, outermost first (loop detection).
    sourcing: Vec<String>,
    /// Errors from the config file, pending display on the first attach.
    config_errors: Option<Vec<String>>,
    hooks: HashMap<String, Cmd>,
    /// What `pane-done`'s hook is about, in the environment of a
    /// `run-shell` it runs (set only while it fires).
    hook_env: Vec<(String, String)>,
    /// When panes were done, for the phone, the hook and the webhook.
    done: done::DoneLog,
    /// Re-entrancy guard: a hook must not fire hooks.
    in_hook: bool,
    /// Output of status-line `#(command)` pieces and what is being run.
    shell_cache: crate::format::ShellCache,
    shell_running: HashSet<String>,
    shell_last_refresh: Option<Instant>,
    /// Plugin directories loaded so far.
    plugins: Vec<String>,
    /// Recent status-line messages, newest last (`show-messages`).
    messages: std::collections::VecDeque<String>,
    /// Paste buffers, newest first; `buffer0` is the most recent copy.
    buffers: Vec<(String, String)>,
    next_buffer: usize,
    /// Pane marked with `select-pane -m`, the default source for `join-pane`.
    marked: Option<PaneId>,
    /// When the saved files last had their pane output refreshed.
    last_history_save: Option<Instant>,
    /// Extra environment for new panes (`set-environment`).
    env: Vec<(String, String)>,
    /// `wait-for` channels, by name.
    waits: HashMap<String, WaitChannel>,
    events: mpsc::UnboundedSender<Event>,
    /// JSON of every session as last autosaved, to detect structural change.
    last_saved: HashMap<String, String>,
    /// Sessions that ended by themselves (their last program exited) and
    /// when: their saves are forgotten a little later, unless the machine
    /// is shutting down (then it is the shutdown that ended them, and
    /// `resume` is to bring them back).
    forget_after: Vec<(String, Instant)>,
    /// When shutdown, restart or logoff was last announced (`EndSession`):
    /// for a while after, nothing is forgotten (and if it was called off,
    /// forgetting goes on after that).
    ending: Option<Instant>,
    /// Pane messages (docs/design/mailbox.md): the id the last one got.
    next_msg: actor::MsgId,
    /// What happened to every message, and the event log it is kept in.
    observe: observe::Store,
    /// Clients waiting in a `-w` for a message or an inbox.
    msg_waits: Vec<mail::MsgWait>,
    /// Messages deleted lately, for `drop-message -u`.
    msg_dropped: Vec<mail::Dropped>,
    /// When old event log files were last cleared out (once a day).
    events_pruned: Option<Instant>,
    /// The daily look for a newer keepane.
    newer: newer::Check,
    /// `#{public_ip}`, while a format uses it.
    public_ip: public_ip::PublicIp,
    /// The panes' agents, as last looked at (`agents`).
    agents: agents::Agents,
    /// What their tokens cost (`prices`), while `agent-cost` is on.
    prices: prices::PriceList,
    /// The phones' page, while it is served (`web-start`).
    web: Option<web_host::WebHost>,
    /// Starts of it so far (`web_host`: stale requests are told apart).
    web_generation: u64,
    /// The links to other machines (docs/design/link.md), once first used.
    link: Option<link_state::LinkHost>,
    /// The paired machines' panes, by address, for the chart of `choose-tree`.
    remote_views: HashMap<String, link_tree::RemoteView>,
    /// Sessions sized to a phone (`web-fit`), by session.
    web_fits: HashMap<SessionId, web_fit::WebFit>,
}

pub async fn run(socket: String) -> Result<()> {
    run_with(socket, RunOptions::default()).await
}

/// How a server is started, beyond its socket name.
#[derive(Default, Clone, Debug)]
pub struct RunOptions {
    /// Bring every saved session back at start whatever the config says
    /// (`keepane __server --restore`, which is what the logon entry runs).
    pub force_restore: bool,
    /// Read this config file instead of looking for one. Tests use it so
    /// that servers sharing a process never share a config through the
    /// environment.
    pub config: Option<PathBuf>,
}

pub async fn run_with(socket: String, options: RunOptions) -> Result<()> {
    let pipe = crate::ipc::pipe_name(&socket);
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let (pane_tx, pane_rx) = std::sync::mpsc::channel::<PaneEvent>();
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("pane-events".into())
            .spawn(move || {
                while let Ok(ev) = pane_rx.recv() {
                    if tx.send(Event::Pane(ev)).is_err() {
                        break;
                    }
                }
            })
            .context("spawn pane event bridge")?;
    }

    // Only this user may connect (the platform's listener sees to that); it
    // is up, and a second server refused, before we are considered up.
    let mut server = crate::platform::ipc::Listener::bind(&pipe)?;
    log::info!("listening on {pipe}");
    let listener = {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut next_client: ClientId = 1;
            loop {
                let conn = match server.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("accept a client: {e}");
                        break;
                    }
                };
                let id = next_client;
                next_client += 1;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (mut rd, mut wr) = tokio::io::split(conn);
                    let (out_tx, mut out_rx) = mpsc::channel::<ServerMsg>(OUTPUT_QUEUE);
                    let _ = tx.send(Event::Connected(id, out_tx));
                    let writer = tokio::spawn(async move {
                        while let Some(m) = out_rx.recv().await {
                            if write_frame(&mut wr, &m).await.is_err() {
                                break;
                            }
                        }
                    });
                    loop {
                        match read_frame::<_, ClientMsg>(&mut rd).await {
                            Ok(Some(m)) => {
                                if tx.send(Event::Msg(id, m)).is_err() {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                log::warn!("client {id}: {e}");
                                break;
                            }
                        }
                    }
                    let _ = tx.send(Event::Gone(id));
                    writer.abort();
                });
            }
        })
    };

    // Shutdown, restart, logoff: save first. The callback runs on the
    // watcher's thread and holds Windows up until the server has saved
    // (or, should the loop be gone, returns at once).
    {
        let tx = tx.clone();
        crate::shutdown::watch(&socket, move || {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            if tx.send(Event::EndSession(done_tx)).is_ok() {
                let _ = done_rx.recv_timeout(Duration::from_secs(4));
            }
        });
    }
    crate::platform::process::let_panes_take_ctrl_c();
    let mut srv = Server::new(pane_tx, socket, tx.clone());
    srv.force_restore = options.force_restore;
    srv.config_override = options.config;
    srv.load_config();
    srv.mail_start();
    srv.newer_start();
    // A persistent interval: a fresh `sleep` per iteration would never fire
    // while events keep arriving, and autosave / idle-exit hang off the tick.
    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_render = Instant::now() - FRAME;
    let mut render_due = false;
    loop {
        // A frame due later (see below) is a wake-up of its own.
        let wait = if render_due { FRAME.saturating_sub(last_render.elapsed()) } else { Duration::from_secs(3600) };
        let ev = tokio::select! {
            ev = rx.recv() => match ev { Some(ev) => Some(ev), None => break },
            _ = tick.tick() => Some(Event::Tick),
            _ = tokio::time::sleep(wait), if render_due => None,
        };
        // A bug in one command must not take every session down with it:
        // log the panic and keep serving (the panic hook writes the details).
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(ev) = ev {
                srv.handle(ev);
                while let Ok(ev) = rx.try_recv() {
                    srv.handle(ev);
                }
            }
            // At most one frame per FRAME: panes printing flat out would
            // otherwise be redrawn for every read (measured: half a core
            // for one attached client). Anything that arrives sooner is
            // drawn by the frame due at the end of the interval, so the
            // screen is never left behind; a key after a pause is drawn
            // at once.
            if last_render.elapsed() >= FRAME {
                // A moving focus frame asks for the next frame by itself.
                render_due = srv.render_all();
                last_render = Instant::now();
            } else {
                render_due = true;
            }
        }));
        if r.is_err() {
            log::error!("recovered from a panic; state may be inconsistent");
            for c in srv.clients.values_mut() {
                c.last_grid = None;
            }
            // A panic inside a hook would otherwise leave the guard set forever.
            srv.in_hook = false;
        }
        if srv.quit {
            break;
        }
    }
    // Stop accepting (releases the pipe name), let client writers flush.
    listener.abort();
    drop(srv);
    // Closed panes left their screens for the history log: see them written.
    crate::histlog::flush(Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(150)).await;
    log::info!("server exiting");
    Ok(())
}

/// The shortest time between two frames sent to the clients: 60 a second,
/// what a terminal shows anyway.
const FRAME: Duration = Duration::from_millis(16);

/// Keys that keep working without the prefix for `repeat-time`: moving
/// between panes, resizing them and walking the window list, the things one
/// presses several times in a row.
const REPEATABLE: &[&str] = &[
    "h", "j", "k", "l", "H", "J", "K", "L", "Up", "Down", "Left", "Right", "C-Up", "C-Down", "C-Left", "C-Right",
    "M-Up", "M-Down", "M-Left", "M-Right", "S-Up", "S-Down", "S-Left", "S-Right", "n", "p", "o", "{", "}",
];

fn default_bindings() -> HashMap<Key, Binding> {
    let mut m = HashMap::new();
    let lines = [
        ("c", "new-window"),
        (",", "command-prompt -p (rename-window) -I \"#W\" \"rename-window -- %%\""),
        ("$", "command-prompt -p (rename-session) -I \"#S\" \"rename-session -- %%\""),
        ("&", "confirm-before -p \"kill-window #W? (y/n)\" kill-window"),
        ("x", "confirm-before -p \"kill-pane #P? (y/n)\" kill-pane"),
        ("d", "detach-client"),
        ("%", "split-window -h"),
        ("\"", "split-window -v"),
        ("o", "select-pane -t next"),
        (";", "select-pane -l"),
        ("Up", "select-pane -U"),
        ("Down", "select-pane -D"),
        ("Left", "select-pane -L"),
        ("Right", "select-pane -R"),
        // vim keys: h/j/k/l move, H/J/K/L resize (tmux-pain-control style).
        ("h", "select-pane -L"),
        ("j", "select-pane -D"),
        ("k", "select-pane -U"),
        ("l", "select-pane -R"),
        ("H", "resize-pane -L 5"),
        ("J", "resize-pane -D 5"),
        ("K", "resize-pane -U 5"),
        ("L", "resize-pane -R 5"),
        ("C-Up", "resize-pane -U 1"),
        ("C-Down", "resize-pane -D 1"),
        ("C-Left", "resize-pane -L 1"),
        ("C-Right", "resize-pane -R 1"),
        ("M-Up", "resize-pane -U 5"),
        ("M-Down", "resize-pane -D 5"),
        ("M-Left", "resize-pane -L 5"),
        ("M-Right", "resize-pane -R 5"),
        // A window bigger than this client (`window-size` gave the session
        // another client's size): pan the view, as tmux does.
        ("S-Up", "refresh-client -U 5"),
        ("S-Down", "refresh-client -D 5"),
        ("S-Left", "refresh-client -L 10"),
        ("S-Right", "refresh-client -R 10"),
        ("n", "next-window"),
        ("p", "previous-window"),
        ("M-n", "next-window -a"),
        ("M-p", "previous-window -a"),
        ("Tab", "last-window"),
        ("z", "resize-pane -Z"),
        ("[", "copy-mode"),
        ("PPage", "copy-mode -u"),
        ("]", "paste-buffer"),
        (":", "command-prompt"),
        ("?", "list-keys"),
        ("{", "swap-pane -U"),
        ("}", "swap-pane -D"),
        ("!", "break-pane"),
        ("Space", "select-layout -n"),
        ("E", "select-layout -E"),
        ("q", "display-panes"),
        ("r", "refresh-client"),
        ("~", "show-messages"),
        ("#", "list-buffers"),
        ("f", "command-prompt -p (find-window) \"find-window -- %%\""),
        ("m", "select-pane -m"),
        ("M", "select-pane -M"),
        ("T", "command-prompt -p (pane-title) -I \"#T\" \"select-pane -T -- %%\""),
        ("'", "command-prompt -p (index) \"select-window -t :%%\""),
        (".", "command-prompt -p (move-window-to) \"move-window -t %%\""),
        ("-", "delete-buffer"),
        ("=", "choose-buffer"),
        ("t", "clock-mode"),
        ("C-o", "rotate-window"),
        ("M-o", "rotate-window -D"),
        ("M-1", "select-layout even-horizontal"),
        ("M-2", "select-layout even-vertical"),
        ("M-3", "select-layout main-horizontal"),
        ("M-4", "select-layout main-vertical"),
        ("M-5", "select-layout tiled"),
        ("(", "switch-client -p"),
        (")", "switch-client -n"),
        ("w", "choose-tree -Zw"),
        ("s", "choose-tree -Zs"),
        ("D", "choose-client"),
        ("B", "choose-jobs"),
        ("v", "dashboard"),
        ("/", "choose-history"),
        ("u", "undo-kill"),
        (
            ">",
            "display-menu -T \"pane #P\" \"Split horizontally\" h \"split-window -h\" \
             \"Split vertically\" v \"split-window -v\" \"\" \
             \"Swap up\" u \"swap-pane -U\" \"Swap down\" d \"swap-pane -D\" \
             \"Break out\" b break-pane \"\" \
             Zoom z \"resize-pane -Z\" Respawn r \"respawn-pane -k\" \
             Kill x kill-pane",
        ),
        (
            "<",
            "display-menu -T \"window #W\" \"New window\" c new-window Rename r \
             \"command-prompt -p (rename-window) -I \\\"#W\\\" \\\"rename-window -- %%\\\"\" \
             \"\" Previous p previous-window Next n next-window \"\" \
             Kill x kill-window",
        ),
        ("i", "display-message \"#S:#W.#P #T\""),
        ("C-s", "save-session -a"),
        ("C-r", "restore-session"),
        ("S", "set-option -w synchronize-panes"),
        ("C-t", "set-option -g pane-timestamps"),
        ("y", "copy-output"),
        ("F", "hints"),
    ];
    for (k, l) in lines {
        let key = Key::parse(k).expect(k);
        let cmd = crate::command::parse_line(l).expect(l).expect(l);
        // Two entries for one key would mean the later one silently won.
        assert!(
            m.insert(key, Binding { cmd, repeat: REPEATABLE.contains(&k) }).is_none(),
            "default key {k} bound twice"
        );
    }
    for d in 0..10u8 {
        let cmd = Cmd::SelectWindow { target: Target::parse(&format!(":{d}")) };
        m.insert(Key::ch((b'0' + d) as char), Binding { cmd, repeat: false });
    }
    m
}

impl Server {
    fn new(
        pane_tx: std::sync::mpsc::Sender<PaneEvent>,
        socket: String,
        events: mpsc::UnboundedSender<Event>,
    ) -> Server {
        Server {
            opts: Options::default(),
            prefix_binds: default_bindings(),
            root_binds: HashMap::new(),
            copy_binds: HashMap::new(),
            sessions: Vec::new(),
            clients: HashMap::new(),
            next_id: 1,
            pane_tx,
            socket,
            had_session: false,
            started: Instant::now(),
            history_pruned: None,
            history_was_on: true,
            killed: Vec::new(),
            quit: false,
            force_restore: false,
            config_override: None,
            sourcing: Vec::new(),
            config_errors: None,
            hooks: HashMap::new(),
            hook_env: Vec::new(),
            done: done::DoneLog::default(),
            in_hook: false,
            shell_cache: crate::format::ShellCache::default(),
            shell_running: HashSet::new(),
            shell_last_refresh: None,
            plugins: Vec::new(),
            messages: std::collections::VecDeque::new(),
            buffers: Vec::new(),
            next_buffer: 0,
            marked: None,
            last_history_save: None,
            env: Vec::new(),
            waits: HashMap::new(),
            events,
            last_saved: HashMap::new(),
            forget_after: Vec::new(),
            ending: None,
            next_msg: 0,
            observe: observe::Store::default(),
            msg_waits: Vec::new(),
            msg_dropped: Vec::new(),
            events_pruned: None,
            newer: newer::Check::default(),
            public_ip: public_ip::PublicIp::default(),
            agents: agents::Agents::default(),
            prices: prices::PriceList::default(),
            web: None,
            web_generation: 0,
            link: None,
            remote_views: HashMap::new(),
            web_fits: HashMap::new(),
        }
    }

    // ----------------------------------------------------------- log-history

    fn history_dir(&self) -> PathBuf {
        if self.opts.log_history_dir.is_empty() {
            crate::histlog::default_dir()
        } else {
            PathBuf::from(expand_home(&self.opts.log_history_dir))
        }
    }

    /// Today's history file for every pane, by where it is now (its
    /// session, and its window and pane numbers as the status line shows
    /// them).
    fn history_paths(&self) -> Vec<(PaneId, PathBuf)> {
        let dir = self.history_dir();
        let day = chrono::Local::now().date_naive();
        let mut out = Vec::new();
        for s in &self.sessions {
            for (wi, w) in s.windows.iter().enumerate() {
                for (pi, id) in w.layout.panes().into_iter().enumerate() {
                    let file = crate::histlog::file_for(
                        &dir,
                        &s.name,
                        wi + self.opts.base_index,
                        pi + self.opts.pane_base_index,
                        day,
                    );
                    out.push((id, file));
                }
            }
        }
        out
    }

    /// Once a second: point each pane at its history file (the day turns,
    /// panes move and sessions are renamed), or at none when logging is
    /// off; once a day, clear out the old files.
    fn history_tick(&mut self) {
        let on = self.opts.log_history;
        let turned_on = on && !std::mem::replace(&mut self.history_was_on, on);
        let paths: HashMap<PaneId, PathBuf> =
            if on { self.history_paths().into_iter().collect() } else { HashMap::new() };
        for w in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()) {
            for p in &mut w.panes {
                if !on {
                    // Off: what was held back still goes out, and nothing
                    // from now on is kept.
                    if p.log_to.is_some() {
                        p.log_take(false);
                        p.flush_log();
                        p.log_to = None;
                    }
                    p.logged = p.logged.max(p.screen().scrolled_total());
                    continue;
                }
                if turned_on {
                    // From what scrolls next, not what scrolled while off.
                    p.logged = p.logged.max(p.screen().scrolled_total());
                }
                // The day turned or the pane moved: what is gathered goes
                // where it was written, then the rest to the new file.
                let to = paths.get(&p.id).cloned();
                if p.log_to != to {
                    p.flush_log();
                    p.log_to = to;
                }
                p.flush_log();
            }
        }
        if self.history_pruned.is_none_or(|t| t.elapsed() >= Duration::from_secs(86_400)) {
            self.history_pruned = Some(Instant::now());
            if self.opts.log_history {
                crate::histlog::prune(self.history_dir(), self.opts.log_history_days);
            }
            self.prune_shell_histories();
        }
    }

    /// After a pane printed: what left its screen goes to its history file.
    fn log_output(&mut self, id: PaneId) {
        if !self.find_pane_mut(id).is_some_and(|p| p.log_pending()) {
            return;
        }
        // A pane newer than the last tick has no file yet: its first
        // screenful may scroll off before the tick comes round.
        let path = match self.find_pane_mut(id).and_then(|p| p.log_to.clone()) {
            Some(path) => path,
            None => match self.history_paths().into_iter().find(|(p, _)| *p == id) {
                Some((_, path)) => path,
                None => return,
            },
        };
        let Some(p) = self.find_pane_mut(id) else { return };
        p.log_to = Some(path);
        p.log_take(false);
    }

    // ------------------------------------------------------------- resurrect

    fn sessions_dir(&self) -> PathBuf {
        if self.opts.sessions_dir.is_empty() {
            crate::resurrect::default_dir()
        } else {
            PathBuf::from(expand_home(&self.opts.sessions_dir))
        }
    }

    /// Describe one live session. `history` holds what each pane had on
    /// screen, collected beforehand because reading it needs `&mut`.
    fn snapshot(&self, s: &Session, history: &HashMap<PaneId, Vec<String>>) -> crate::resurrect::SavedSession {
        use crate::resurrect::*;
        SavedSession {
            name: s.name.clone(),
            current: s.cur,
            size: Some((s.cols, s.rows)),
            windows: s
                .windows
                .iter()
                .map(|w| {
                    let lookup = |id: PaneId| {
                        w.pane(id).map(|p| SavedPane {
                            argv: p.argv.clone(),
                            cwd: p.current_path(),
                            history: history.get(&id).cloned().unwrap_or_default(),
                            name: p.actor.name.clone(),
                            work_mode: (p.actor.mode != actor::WorkMode::Normal)
                                .then(|| p.actor.mode.as_str().to_string()),
                            shell_history: p.shell_history.clone(),
                        })
                    };
                    let order = w.layout.panes();
                    SavedWindow {
                        name: w.name.clone(),
                        layout: SavedNode::from_layout(&w.layout, &lookup)
                            .unwrap_or(SavedNode::Pane { pane: SavedPane::default() }),
                        active: order.iter().position(|p| *p == w.active).unwrap_or(0),
                        zoomed: w.zoomed,
                    }
                })
                .collect(),
        }
    }

    /// The last `save-history` lines of every pane in a session (all of them
    /// for `all`), colours included.
    fn pane_histories(&mut self, sid: SessionId) -> HashMap<PaneId, Vec<String>> {
        let want = self.opts.save_history;
        let mut out = HashMap::new();
        if want == 0 {
            return out;
        }
        let ids: Vec<PaneId> = match self.session(sid) {
            Some(s) => s.windows.iter().flat_map(|w| w.panes.iter().map(|p| p.id)).collect(),
            None => return out,
        };
        for id in ids {
            let Some(p) = self.find_pane_mut(id) else { continue };
            let total = p.scrollback_len() + p.rows as usize;
            let lines = p.lines_from(total.saturating_sub(want), true, false);
            if !lines.is_empty() {
                out.insert(id, lines);
            }
        }
        out
    }

    /// Save one session to its file; returns the path.
    fn save_session_file(&mut self, sid: SessionId) -> Result<PathBuf, String> {
        let history = self.pane_histories(sid);
        let s = self.session(sid).ok_or("no such session")?;
        let saved = self.snapshot(s, &history);
        // The change key covers the structure only: pane output changes every
        // second and must not make the file be rewritten every second.
        let key = serde_json::to_string(&self.snapshot(s, &HashMap::new())).unwrap_or_default();
        let path = crate::resurrect::file_for(&self.sessions_dir(), &saved.name);
        crate::resurrect::SavedFile::new(saved).save(&path)?;
        self.last_saved.insert(path.to_string_lossy().into_owned(), key);
        Ok(path)
    }

    fn binds_mut(&mut self, table: crate::command::KeyTable) -> &mut HashMap<Key, Binding> {
        match table {
            crate::command::KeyTable::Prefix => &mut self.prefix_binds,
            crate::command::KeyTable::Root => &mut self.root_binds,
            crate::command::KeyTable::Copy => &mut self.copy_binds,
        }
    }

    /// Save every session, history included, whatever changed: the session
    /// is ending (shutdown, logoff) and there is no next tick.
    fn save_everything(&mut self) {
        if !self.opts.autosave {
            return;
        }
        let ids: Vec<SessionId> = self.sessions.iter().map(|s| s.id).collect();
        for sid in ids {
            if let Err(e) = self.save_session_file(sid) {
                log::warn!("save at session end: {e}");
            }
        }
        self.last_history_save = Some(Instant::now());
    }

    /// Autosave every session whose structure changed since its last save
    /// (called from the tick; cheap: the tree is tiny).
    fn autosave_changed(&mut self) {
        if !self.opts.autosave {
            return;
        }
        let dir = self.sessions_dir();
        let ids: Vec<SessionId> = self.sessions.iter().map(|s| s.id).collect();
        // Pane output is saved too, but refreshing it means writing files, so
        // that happens on a slow beat rather than whenever something printed.
        let refresh_history =
            self.opts.save_history > 0 && self.last_history_save.is_none_or(|t| t.elapsed() > Duration::from_secs(30));
        if refresh_history {
            self.last_history_save = Some(Instant::now());
        }
        for sid in ids {
            let Some(s) = self.session(sid) else { continue };
            let saved = self.snapshot(s, &HashMap::new());
            let key = serde_json::to_string(&saved).unwrap_or_default();
            let path = crate::resurrect::file_for(&dir, &saved.name).to_string_lossy().into_owned();
            if !refresh_history && self.last_saved.get(&path) == Some(&key) {
                continue;
            }
            match self.save_session_file(sid) {
                Ok(_) => {}
                Err(e) => {
                    log::warn!("autosave {}: {e}", saved.name);
                    // Do not retry every second.
                    self.last_saved.insert(path, key);
                }
            }
        }
    }

    /// Recreate one saved session (its name must be free). Returns its id.
    /// `size` is the terminal about to attach; without one the session
    /// comes back at the size it was saved at (80x24 for older files).
    fn restore_session_file(
        &mut self,
        path: &std::path::Path,
        size: Option<(u16, u16)>,
    ) -> Result<(SessionId, Vec<String>), String> {
        use crate::resurrect::*;
        let ss = SavedFile::load(path)?.session;
        if self.sessions.iter().any(|s| s.name == ss.name) {
            return Err(format!("session {} already exists", ss.name));
        }
        // A file is data: a hand-edited or damaged size must not make a
        // session too small to lay out (`new -x/-y` has the same floor).
        let (cols, rows) = size.or(ss.size).map(|(c, r)| (c.max(10), r.max(3))).unwrap_or((80, 24));
        let first = ss.windows.first().ok_or_else(|| format!("session {} has no windows", ss.name))?;
        let mut problems = Vec::new();
        // The session comes with its first window; the placeholder pane of
        // every window is replaced by the saved layout afterwards.
        let sid = self.new_session(Some(ss.name.clone()), Some(first.name.clone()), None, &[], cols, rows)?;
        for (i, sw) in ss.windows.iter().enumerate() {
            if i > 0
                && let Err(e) = self.new_window(sid, Some(sw.name.clone()), None, &[])
            {
                problems.push(format!("{}:{}: {e}", ss.name, sw.name));
                continue;
            }
            if let Err(e) = self.rebuild_window(sid, i, sw) {
                problems.push(format!("{}:{}: {e}", ss.name, sw.name));
            }
        }
        if let Some(s) = self.session_mut(sid) {
            s.cur = ss.current.min(s.windows.len().saturating_sub(1));
            s.last = None;
        }
        self.relayout_session(sid);
        Ok((sid, problems))
    }

    /// Restore every saved session whose name is free.
    fn restore_all(&mut self, size: Option<(u16, u16)>) -> (Vec<SessionId>, Vec<String>) {
        let mut created = Vec::new();
        let mut problems = Vec::new();
        for (name, _, path) in crate::resurrect::list(&self.sessions_dir()) {
            if self.sessions.iter().any(|s| s.name == name) {
                continue;
            }
            match self.restore_session_file(&path, size) {
                Ok((sid, p)) => {
                    created.push(sid);
                    problems.extend(p);
                }
                Err(e) => problems.push(e),
            }
        }
        (created, problems)
    }

    /// Replace window `widx`'s panes by the saved layout, spawning one pane
    /// per saved leaf (the placeholder pane created with the window is killed).
    fn rebuild_window(
        &mut self,
        sid: SessionId,
        widx: usize,
        sw: &crate::resurrect::SavedWindow,
    ) -> Result<(), String> {
        let area = {
            let s = self.session(sid).ok_or("no such session")?;
            self.window_area(s.cols, s.rows)
        };
        let border = self.border_rows();
        let mut panes = Vec::new();
        for saved in sw.layout.panes() {
            let argv: Vec<String> = saved.argv.clone();
            let cwd = saved.cwd.as_deref().filter(|d| std::path::Path::new(d).is_dir());
            // The saved output goes back into the pane's console before
            // the new shell starts printing, so a resumed pane looks like
            // it did and stays that way through repaints and resizes.
            let replay = (!saved.history.is_empty()).then(|| {
                let mut text = saved.history.join("\r\n");
                text.push_str("\r\n");
                text
            });
            // Sizes are refitted by relayout; spawn at the window size.
            let hist = saved.shell_history.clone().map_or(ShellHistory::From(None), ShellHistory::Keep);
            let mut pane = self.spawn_pane_replaying(&argv, cwd, area.w, area.h, replay, hist)?;
            // Its name and work mode come back; a name another pane has
            // taken meanwhile does not.
            pane.actor.mode = saved.work_mode.as_deref().and_then(actor::WorkMode::parse).unwrap_or_default();
            if let Some(name) = &saved.name {
                let taken = self.pane_by_name(name).is_some()
                    || panes.iter().any(|p: &Pane| p.actor.name.as_deref() == Some(name));
                if taken {
                    self.note_message(&format!("resume: pane name '{name}' is taken; %{} has none", pane.id));
                } else {
                    pane.actor.name = Some(name.clone());
                }
            }
            panes.push(pane);
        }
        let mut ids = panes.iter().map(|p| p.id);
        let layout = sw.layout.to_layout(&mut ids).ok_or("layout/pane count mismatch")?;
        let order = layout.panes();
        let active = order.get(sw.active).or(order.first()).copied().ok_or("empty layout")?;
        let s = self.session_mut(sid).ok_or("no such session")?;
        let w = s.windows.get_mut(widx).ok_or("no such window")?;
        for p in &mut w.panes {
            p.kill();
        }
        w.panes = panes;
        w.layout = layout;
        w.active = active;
        w.last_pane = None;
        w.zoomed = sw.zoomed && w.panes.len() > 1;
        w.relayout(area, border);
        Ok(())
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn load_config(&mut self) {
        if let Some(path) = self.config_override.clone().or_else(crate::config::find_config) {
            match self.source_file(&path.to_string_lossy()) {
                Ok(n) => log::info!("loaded {} ({})", path.display(), crate::format::count(n, "command")),
                Err(e) => {
                    log::error!("config {}: {e}", path.display());
                    // Shown to the first client that attaches, like tmux does.
                    self.config_errors = Some(e.lines().map(str::to_string).collect());
                }
            }
        } else if let Some(tmux) = import::tmux_config() {
            // Said where it is looked for, not on every attach.
            let note = format!(
                "{} is tmux's and is not read: `keepane import-config` brings over what keepane can use",
                tmux.display()
            );
            log::info!("{note}");
            self.note_message(&note);
        }
        if (self.opts.restore_on_start || self.force_restore) && self.sessions.is_empty() {
            let (created, problems) = self.restore_all(None);
            log::info!("restore-on-start: {} session(s)", created.len());
            for p in problems {
                log::warn!("restore-on-start: {p}");
            }
        }
    }

    /// Source a file of keepane commands, then load any `@plugin` it declared.
    ///
    /// A file named `*tmux.conf` is one written for tmux: whatever keepane
    /// cannot use in it (other key tables, TPM, `%if` blocks) is skipped
    /// with a note in `show-messages`, and the result is a one-line summary
    /// rather than a wall of errors on every attach.
    fn source_file(&mut self, path: &str) -> Result<usize, String> {
        let path = &expand_home(path);
        let lenient = crate::config::is_tmux_conf(std::path::Path::new(path));
        // A file that sources itself (directly or through another) would
        // recurse until the stack ran out and take the server with it.
        let canon = std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or(path.clone());
        if self.sourcing.contains(&canon) {
            return Err(format!("{path}: is already being sourced (a source-file loop)"));
        }
        self.sourcing.push(canon);
        let r = self.source_file_inner(path, lenient);
        self.sourcing.pop();
        r
    }

    fn source_file_inner(&mut self, path: &String, lenient: bool) -> Result<usize, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut n = 0;
        let mut errors = Vec::new();
        let (lines, unterminated) = logical_lines(&text, &|cond| self.config_condition(cond));
        if let Some(line_no) = unterminated {
            errors.push(format!("{path}:{line_no}: %if without %endif; the rest of the file was skipped"));
        }
        for (line_no, line) in lines {
            match crate::command::parse_line(&line) {
                Ok(None) => {}
                Ok(Some(cmd)) => {
                    n += 1;
                    if let Outcome::Error(e) = self.exec(cmd, None) {
                        errors.push(format!("{path}:{line_no}: {e}"));
                    }
                }
                Err(e) => errors.push(format!("{path}:{line_no}: {e}")),
            }
        }
        let pending = std::mem::take(&mut self.opts.pending_plugins);
        for p in pending {
            if let Err(e) = self.load_plugin(&p) {
                errors.push(format!("{path}: @plugin {p}: {e}"));
            }
        }
        if errors.is_empty() {
            return Ok(n);
        }
        if lenient {
            for e in &errors {
                log::warn!("{e}");
                self.note_message(&format!("skipped: {e}"));
            }
            let short = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned());
            return Err(format!(
                "{}: skipped {} keepane could not use (prefix ~ or show-messages lists them)",
                short.unwrap_or_else(|| path.clone()),
                crate::format::count(errors.len(), "line")
            ));
        }
        Err(errors.join("\n"))
    }

    /// A plugin is a directory holding `<name>.keepane` or `plugin.keepane`; `name`
    /// is a path or a directory name under `plugin-path`. Sourcing it once is
    /// all "loading" means: it binds keys, sets options, hooks and status
    /// pieces, and its scripts talk back through the `keepane` CLI.
    fn load_plugin(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim_matches(['"', '\'']);
        let direct = std::path::PathBuf::from(expand_home(name));
        let dir = if direct.is_dir() {
            direct
        } else if direct.is_file() {
            let f = direct.to_string_lossy().into_owned();
            if self.plugins.contains(&f) {
                return Ok(());
            }
            self.plugins.push(f.clone());
            return self.source_file(&f).map(|_| ());
        } else {
            let base = std::path::PathBuf::from(expand_home(&self.opts.plugin_path));
            // `user/repo` (TPM style) -> last component.
            let short = name.rsplit(['/', '\\']).next().unwrap_or(name);
            let cand = base.join(short);
            // Plugins installed under wmux's name (up to 0.13.1) are found
            // there while the default place has none.
            let old = std::path::PathBuf::from(expand_home("~/.wmux/plugins")).join(short);
            if cand.is_dir() {
                cand
            } else if self.opts.plugin_path == Options::default().plugin_path && old.is_dir() {
                old
            } else {
                return Err(format!("plugin not found: {name} (looked in {})", base.display()));
            }
        };
        let short = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let entry = [
            format!("{short}.keepane"),
            "plugin.keepane".into(),
            "plugin.conf".into(),
            format!("{short}.wmux"),
            "plugin.wmux".into(),
        ]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{}: no {short}.keepane or plugin.keepane", dir.display()))?;
        let dir_s = dir.to_string_lossy().into_owned();
        if self.plugins.contains(&dir_s) {
            return Ok(());
        }
        self.plugins.push(dir_s);
        self.source_file(&entry.to_string_lossy()).map(|_| ())
    }

    /// Fire a hook's command, if one is set. Hooks never fire from inside a hook.
    fn fire_hook(&mut self, name: &str, cid: Option<ClientId>) {
        if self.in_hook {
            return;
        }
        let Some(cmd) = self.hooks.get(name).cloned() else { return };
        self.in_hook = true;
        let out = self.exec(cmd, cid);
        self.in_hook = false;
        if let Outcome::Error(e) = out {
            log::warn!("hook {name}: {e}");
        }
    }

    /// Run `command` through the shell on a worker thread; the result comes
    /// back as `Event::ShellDone` (or `StatusShell` when `status` is set).
    fn spawn_shell(&self, command: String, pane: Option<PaneId>, cid: Option<ClientId>, status: bool) {
        let events = self.events.clone();
        let mut env = self.pane_env(pane.unwrap_or(0));
        if pane.is_none() {
            env.retain(|(k, _)| k != "KEEPANE_PANE");
        }
        env.extend(self.hook_env.iter().cloned());
        let spawned = std::thread::Builder::new().name("run-shell".into()).spawn({
            let events = events.clone();
            let command = command.clone();
            move || {
                // A status piece that hangs must not wedge its slot forever;
                // a foreground run-shell waits like tmux does.
                let timeout = if status { Some(STATUS_SHELL_TIMEOUT) } else { None };
                let (output, code) = run_shell_blocking(&command, &env, timeout);
                let ev = if status {
                    Event::StatusShell { command, output }
                } else {
                    Event::ShellDone { cid, output, code }
                };
                let _ = events.send(ev);
            }
        });
        if let Err(e) = spawned {
            // Never leave a client waiting on a Pending outcome.
            let ev = if status {
                Event::StatusShell { command, output: String::new() }
            } else {
                Event::ShellDone { cid, output: format!("cannot start a thread: {e}"), code: 127 }
            };
            let _ = events.send(ev);
        }
    }

    /// Kick off every `#(command)` the status line wants, at most once per
    /// `status-interval`, and never two copies of the same command at once.
    fn refresh_status_shells(&mut self) {
        let wanted = std::mem::take(&mut self.shell_cache.wanted);
        if wanted.is_empty() {
            return;
        }
        let due = self
            .shell_last_refresh
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(self.opts.status_interval.max(1)));
        for cmd in wanted {
            let fresh = self.shell_cache.results.contains_key(&cmd);
            if self.shell_running.contains(&cmd) || (fresh && !due) {
                continue;
            }
            self.shell_running.insert(cmd.clone());
            self.spawn_shell(cmd, None, None, true);
        }
        if due {
            self.shell_last_refresh = Some(Instant::now());
        }
    }

    // ----------------------------------------------------------------- events

    fn handle(&mut self, ev: Event) {
        match ev {
            // A respawned pane keeps its id, so the generation says whether
            // this came from the process that is running now.
            Event::Pane(PaneEvent::Input(id, bytes)) => {
                // `pipe-pane -I`: the command's output is typed into the
                // pane; an empty chunk is its end, which closes an
                // input-only pipe (an -IO one stays for the output side).
                if let Some(p) = self.find_pane_mut(id) {
                    if bytes.is_empty() {
                        if p.pipe.as_ref().is_some_and(|pipe| !pipe.output) {
                            p.pipe = None;
                        }
                    } else {
                        p.type_input(&bytes);
                    }
                }
            }
            Event::Pane(PaneEvent::Output(id, generation, bytes)) => {
                if let Some(p) = self.find_pane_mut(id)
                    && p.generation == generation
                {
                    // pipe-pane and a recording see what the program wrote,
                    // not the screen.
                    let slow = p.pipe_write(&bytes);
                    p.record_write(&bytes);
                    p.process_output(&bytes);
                    let prompted = std::mem::take(&mut p.prompted);
                    // Commands that ended; not those a resumed pane prints back.
                    let finished = std::mem::take(&mut p.finished);
                    let replaying = p.is_pending();
                    // A resumed pane: its saved output has all been printed
                    // (the printer's marker arrived); its program starts now,
                    // in the same console, while the printer still holds it.
                    if p.is_pending()
                        && p.replay_done
                        && let Err(e) = p.start_pending()
                    {
                        log::warn!("pane %{id}: cannot start after the replay: {e:#}");
                    }
                    if let Some(command) = slow {
                        // Say it once per pipe: a command that cannot keep up
                        // loses output, and silence about that is worse.
                        log::warn!("pipe-pane %{id}: {command} is not keeping up; output is being dropped");
                        self.note_message(&format!("pipe-pane %{id}: {command} is not keeping up; output dropped"));
                    }
                    self.note_output(id);
                    if self.opts.log_history {
                        self.log_output(id);
                    }
                    let after = self.opts.done_after;
                    for f in finished.into_iter().filter(|f| !replaying && f.secs >= after) {
                        let text = done::command_text(&f.command, f.secs, f.failed, f.exit);
                        self.pane_done(id, done::Kind::Command, text, Some(!f.failed), f.exit, Some(f.secs));
                    }
                    if prompted {
                        self.pane_prompt_seen(id);
                    }
                    if prompted
                        && let Some(p) = self.find_pane_mut(id)
                        && !p.settle
                    {
                        p.settle = true;
                        let tx = self.events.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(PROMPT_SETTLE).await;
                            let _ = tx.send(Event::PromptSettled(id));
                        });
                    }
                }
            }
            Event::Pane(PaneEvent::Exit(id, generation, code)) => {
                if generation == crate::server::pane::HELPER_GEN {
                    // The printer of a resumed pane's saved output is gone.
                    // Normally its program is already running (started when
                    // the printer's marker arrived); if the printer died
                    // before that, the program starts now, unadorned.
                    if let Some(p) = self.find_pane_mut(id)
                        && p.is_pending()
                        && let Err(e) = p.start_pending()
                    {
                        log::warn!("pane %{id}: cannot start after the replay: {e:#}");
                    }
                    return;
                }
                if self.find_pane_mut(id).is_some_and(|p| p.generation != generation) {
                    return; // the previous process of a respawned pane
                }
                // A pane kept for undo-kill whose program ended meanwhile:
                // nothing is left of it to bring back.
                if let Some(i) = self.killed.iter_mut().position(|k| k.pane_mut(id).is_some()) {
                    let gone = match &mut self.killed[i] {
                        Killed::Pane { .. } => true,
                        Killed::Window { window, .. } => {
                            window.layout.remove(id);
                            window.panes.retain(|p| p.id != id);
                            if window.active == id {
                                window.active = window.layout.panes().first().copied().unwrap_or(0);
                            }
                            window.panes.is_empty()
                        }
                    };
                    if gone {
                        self.killed.remove(i);
                    }
                    return;
                }
                let command = self.find_pane_mut(id).map(|p| p.command.clone()).unwrap_or_default();
                log::info!("pane %{id} ({command}) exited with {code}");
                // A popup is not part of any window: it either goes away now
                // (`-E`) or waits for a key, showing what the command left.
                if let Some(cid) = self.popup_of_pane(id) {
                    let c = self.clients.get_mut(&cid).unwrap();
                    let p = c.popup.as_mut().unwrap();
                    if p.close_on_exit {
                        c.popup = None;
                    } else {
                        p.finished = true;
                    }
                    c.last_grid = None;
                    return;
                }
                let name = Some(done::program_name(&command)).filter(|n| !n.is_empty()).unwrap_or("its program".into());
                self.pane_done(
                    id,
                    done::Kind::Exit,
                    format!("{name} exited with {code}"),
                    Some(code == 0),
                    Some(code as i32),
                    None,
                );
                if self.opts.remain_on_exit {
                    // Keep the pane and what it printed; say why it stopped.
                    // `respawn-pane` starts it again, `kill-pane` closes it.
                    if let Some(p) = self.find_pane_mut(id) {
                        p.exit_code = Some(code);
                        p.died_at = Some(Instant::now());
                        let note =
                            format!("\r\n\x1b[7m[{command} exited with {code}; respawn-pane or kill-pane]\x1b[0m\r\n");
                        p.process_output(note.as_bytes());
                    }
                    self.note_message(&format!("pane %{id} ({command}) exited with {code}"));
                    return;
                }
                // Its last program gone, the session ends by itself: its
                // save is to be forgotten (see `forget_due`).
                let session = self.session_of_pane(id).and_then(|s| self.session(s)).map(|s| s.name.clone());
                self.remove_pane(id, Some(code));
                if let Some(name) = session
                    && !self.sessions.iter().any(|s| s.name == name)
                {
                    self.forget_after.push((name, Instant::now()));
                }
            }
            Event::Connected(id, tx) => {
                self.clients.insert(
                    id,
                    Client {
                        id,
                        tx,
                        cols: 80,
                        rows: 24,
                        cwd: String::new(),
                        interactive: false,
                        pane_env: None,
                        session: None,
                        last_session: None,
                        last_grid: None,
                        last_cursor: None,
                        prefix: false,
                        prefix_at: None,
                        repeat_until: None,
                        panes_until: None,
                        panes_typed: None,
                        hints: None,
                        prompt: None,
                        message: None,
                        overlay: None,
                        chooser: None,
                        popup: None,
                        mouse_buttons: 0,
                        drag: None,
                        selecting: None,
                        swallow_up: HashSet::new(),
                        pending_bell: false,
                        pending: std::collections::VecDeque::new(),
                        window_hits: Vec::new(),
                        created: Instant::now(),
                        last_activity: Instant::now(),
                        view_x: 0,
                        view_y: 0,
                        view_pinned: false,
                        view_follow: false,
                        cursor_out: None,
                    },
                );
            }
            Event::Gone(id) => {
                self.mail_client_gone(id);
                let was_in = self.clients.remove(&id).and_then(|c| c.session);
                if let Some(sid) = was_in {
                    self.fit_session(sid, None); // smallest/largest: one client fewer
                }
                // A client that went away while waiting must not hold a
                // `wait-for` lock, or nothing could ever take it again.
                let mut freed: Vec<String> = Vec::new();
                for (name, w) in self.waits.iter_mut() {
                    w.waiters.retain(|c| *c != id);
                    w.lockers.retain(|c| *c != id);
                    if w.idle() {
                        freed.push(name.clone());
                    }
                }
                for name in freed {
                    self.waits.remove(&name);
                }
            }
            Event::Msg(id, msg) => self.handle_msg(id, msg),
            Event::ShellDone { cid, output, code } => {
                let text = output.trim_end().to_string();
                match cid {
                    Some(cid) => {
                        let out = if code == 0 {
                            if text.is_empty() { Outcome::Ok } else { Outcome::Text(text) }
                        } else {
                            Outcome::Error(if text.is_empty() { format!("shell exited with {code}") } else { text })
                        };
                        self.reply(cid, out);
                    }
                    None => {
                        if code != 0 {
                            log::warn!("run-shell exited with {code}: {text}");
                        }
                    }
                }
            }
            Event::StatusShell { command, output } => {
                self.shell_running.remove(&command);
                // First line only, like tmux.
                let line = output.lines().next().unwrap_or("").trim_end().to_string();
                self.shell_cache.results.insert(command, line);
            }
            Event::EndSession(done) => {
                log::info!("session ending: saving every session");
                // The shells dying now are the shutdown's doing: keep their saves.
                self.ending = Some(Instant::now());
                self.forget_after.clear();
                self.save_everything();
                // And what the panes show goes to their history logs.
                for w in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()) {
                    for p in &mut w.panes {
                        if p.log_to.is_some() {
                            p.log_take(true);
                        }
                    }
                }
                crate::histlog::flush(Duration::from_secs(2));
                let _ = done.send(());
            }
            Event::NewerChecked(latest) => self.newer_checked(latest),
            Event::PublicIp(ip) => self.public_ip_answered(ip),
            Event::AgentStats(stats) => self.agents_seen(stats),
            Event::Prices(p) => self.prices_fetched(p),
            Event::Web(generation, seen) => self.web_seen(generation, seen),
            Event::WebStopped(cid) => self.reply(cid, Outcome::Ok),
            Event::LinkAnswer(a) => self.link_answered(*a),
            Event::DoneWebhookFailed(why) => {
                log::warn!("done-webhook: {why}");
                self.note_message(&format!("done-webhook: {why}"));
            }
            // The frame after every event draws it.
            Event::PrefixHintDue => {}
            Event::DragScroll(cid) => {
                if let Some(sel) = self.clients.get_mut(&cid).and_then(|c| c.selecting.as_mut()) {
                    sel.ticking = false;
                }
                self.drag_scroll(cid);
            }
            Event::PromptSettled(id) => {
                if let Some(p) = self.find_pane_mut(id)
                    && std::mem::take(&mut p.settle)
                {
                    self.pane_prompted(id);
                    self.wake_waits();
                }
            }
            Event::Tick => {
                self.autosave_changed();
                self.history_tick();
                self.mail_tick();
                self.newer_tick();
                self.public_ip_tick();
                self.agents_tick();
                self.prices_tick();
                self.web_tick();
                self.web_fit_tick();
                self.forget_due();
                // An open chart keeps the paired machines' panes fresh.
                if self.a_chart_is_open() {
                    self.ask_remote_trees(false);
                }
                // Kept long enough: gone for good.
                let keep = Duration::from_secs(self.opts.undo_kill_time);
                self.killed.retain(|k| k.at().elapsed() < keep);
                // A server nobody uses has no reason to live (e.g. started by
                // `keepane attach` when there was nothing to attach to).
                if self.sessions.is_empty()
                    && self.clients.is_empty()
                    && self.started.elapsed() > Duration::from_secs(10)
                {
                    log::info!("idle with no sessions; exiting");
                    self.quit = true;
                }
            }
        }
        if self.had_session && self.sessions.is_empty() {
            self.quit = true;
        }
    }

    fn handle_msg(&mut self, cid: ClientId, msg: ClientMsg) {
        match msg {
            ClientMsg::Command { version, argv, cwd, cols, rows, interactive, pane_env } => {
                if version != PROTOCOL_VERSION {
                    if let Some(c) = self.clients.get_mut(&cid) {
                        c.send(ServerMsg::Error(format!(
                            "protocol mismatch: client {version}, server {PROTOCOL_VERSION}"
                        )));
                        c.send(ServerMsg::Done { code: 1 });
                    }
                    return;
                }
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.cols = cols.max(1);
                    c.rows = rows.max(1);
                    c.cwd = cwd;
                    c.interactive = interactive;
                    c.pane_env = pane_env;
                }
                // A bug in one command is caught here rather than only by the
                // main loop, so the client that sent it gets an error and
                // exits instead of waiting forever for a reply.
                let run =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match crate::command::parse(&argv) {
                        Ok(cmd) => self.exec(cmd, Some(cid)),
                        Err(e) => Outcome::Error(e),
                    }));
                let out = run.unwrap_or_else(|_| {
                    self.in_hook = false;
                    let name = argv.first().map(String::as_str).unwrap_or("");
                    Outcome::Error(format!("internal error running '{name}' (details in server.log)"))
                });
                self.reply(cid, out);
            }
            ClientMsg::Key(rec) => self.handle_key(cid, rec),
            ClientMsg::Mouse(rec) => self.handle_mouse(cid, rec),
            ClientMsg::Resize { cols, rows } => {
                let Some(c) = self.clients.get_mut(&cid) else { return };
                c.cols = cols.max(1);
                c.rows = rows.max(1);
                c.last_grid = None;
                if let Some(sid) = c.session {
                    self.fit_session(sid, Some(cid));
                }
            }
            ClientMsg::Detach => self.detach(cid, "detached"),
        }
    }

    /// Deliver a command outcome to a CLI client (or attach it).
    fn reply(&mut self, cid: ClientId, out: Outcome) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        match out {
            Outcome::Ok => {
                if c.session.is_some() {
                    return; // attached client running a prompt command
                }
                c.send(ServerMsg::Done { code: 0 });
            }
            Outcome::Text(t) => {
                if c.session.is_some() {
                    self.show(cid, &t);
                    return;
                }
                c.send(ServerMsg::Text(t));
                c.send(ServerMsg::Done { code: 0 });
            }
            Outcome::Error(e) => {
                if c.session.is_some() {
                    self.show(cid, &e);
                    return;
                }
                c.send(ServerMsg::Error(e));
                c.send(ServerMsg::Done { code: 1 });
            }
            Outcome::Pending => {}
            Outcome::Attach(sid) => {
                let name = self.sessions.iter().find(|s| s.id == sid).map(|s| s.name.clone()).unwrap_or_default();
                let mouse = self.opts.mouse;
                let config_errors = self.config_errors.take();
                let c = self.clients.get_mut(&cid).unwrap();
                c.session = Some(sid);
                c.last_grid = None;
                c.last_cursor = None;
                c.prefix = false;
                c.prefix_at = None;
                c.repeat_until = None;
                c.prompt = None;
                c.overlay = config_errors;
                c.chooser = None;
                c.popup = None;
                c.message = None;
                c.drag = None;
                c.mouse_buttons = 0;
                c.swallow_up.clear();
                c.send(ServerMsg::Attached { session: name });
                c.send(ServerMsg::SetMouse(mouse));
                // `window-size manual` keeps the size the session has (its
                // saved or `-x/-y` one); the rest take it from the clients.
                self.fit_session(sid, Some(cid));
                if let Some(s) = self.session_mut(sid) {
                    s.last_used = Instant::now();
                }
                self.fire_hook("client-attached", Some(cid));
            }
        }
    }

    /// One-line text goes to the status line for `display-time`; multi-line
    /// text becomes an overlay that stays until a key is pressed.
    fn show(&mut self, cid: ClientId, text: &str) {
        self.note_message(&text.replace('\n', " "));
        if let Some(c) = self.clients.get_mut(&cid) {
            if text.contains('\n') {
                c.overlay = Some(text.lines().map(str::to_string).collect());
            } else {
                c.message = Some((text.replace('\r', " "), Instant::now()));
            }
        }
    }

    fn message(&mut self, cid: ClientId, text: &str) {
        self.show(cid, text);
    }

    /// Add a line to the `show-messages` log, keeping the last 100 as tmux
    /// does. Every place that has something to report goes through here, so
    /// the log can never grow without bound.
    fn note_message(&mut self, text: &str) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        self.messages.push_back(format!("{stamp} {text}"));
        while self.messages.len() > 100 {
            self.messages.pop_front();
        }
    }

    /// A pane printed something: keep the window's silence timer fresh and,
    /// when it is not the window being looked at, raise the alerts the
    /// `monitor-*` options ask for.
    fn note_output(&mut self, pid: PaneId) {
        let Some((sid, widx)) = self.window_of_pane(pid) else { return };
        let (activity_on, bell_on) = (self.opts.monitor_activity, self.opts.monitor_bell);
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        let current = s.cur == widx;
        let Some(w) = s.windows.get_mut(widx) else { return };
        w.last_output = Instant::now();
        if let Some(p) = w.panes.iter_mut().find(|p| p.id == pid) {
            p.last_output = w.last_output;
            p.output_count = p.output_count.wrapping_add(1);
        }
        w.alert_silence = false;
        if current {
            return; // the bell of the window in view is rung by the renderer
        }
        // Nothing consumes a background pane's bell otherwise, so take it here.
        let rang = w.panes.iter().any(|p| p.bell);
        if rang {
            for p in &mut w.panes {
                p.bell = false;
            }
        }
        let (name, index) = (w.name.clone(), widx + self.opts.base_index);
        let mut fresh = Vec::new();
        if activity_on && !w.alert_activity {
            w.alert_activity = true;
            fresh.push(("Activity", false));
        }
        if bell_on && rang && !w.alert_bell {
            w.alert_bell = true;
            fresh.push(("Bell", true));
        }
        for (what, is_bell) in fresh {
            self.alert(sid, Some(pid), &format!("{what} in window {index} ({name})"), is_bell);
        }
    }

    /// Tell the clients of a session about an alert: a status-line message
    /// when `visual-bell`/`visual-activity` is on, the terminal bell otherwise.
    fn alert(&mut self, sid: SessionId, pane: Option<PaneId>, text: &str, is_bell: bool) {
        let visual = if is_bell { self.opts.visual_bell } else { self.opts.visual_activity };
        // A window flag only reaches someone who is looking at the terminal;
        // a notification reaches them when it is behind other windows, with
        // a button that brings them to the pane it is about.
        if self.opts.notify {
            let name = self.session(sid).map(|s| s.name.clone()).unwrap_or_else(|| "keepane".into());
            let go = pane.map(|p| crate::notify::go_to_pane_url(&self.socket, p));
            crate::notify::notify_with(&format!("keepane: {name}"), text, go.as_deref());
        }
        let ids: Vec<ClientId> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for cid in ids {
            if visual {
                self.show(cid, text);
            } else if is_bell && let Some(c) = self.clients.get_mut(&cid) {
                c.pending_bell = true;
            }
        }
    }

    /// Run from the render tick: the current window of a session never holds
    /// an alert (that is the one invariant of the flags, enforced here so no
    /// path that changes the current window has to remember it), and
    /// `monitor-silence` flags a background window that has gone quiet.
    fn sweep_alerts(&mut self) {
        let quiet = Duration::from_secs(self.opts.monitor_silence.max(1));
        let watch_silence = self.opts.monitor_silence > 0;
        let base = self.opts.base_index;
        let mut alerts: Vec<(SessionId, PaneId, String)> = Vec::new();
        for s in &mut self.sessions {
            let (cur, sid) = (s.cur, s.id);
            for (i, w) in s.windows.iter_mut().enumerate() {
                if i == cur {
                    w.alert_activity = false;
                    w.alert_bell = false;
                    w.alert_silence = false;
                    w.alert_mail = false;
                    continue;
                }
                if watch_silence && !w.alert_silence && w.last_output.elapsed() >= quiet {
                    w.alert_silence = true;
                    alerts.push((sid, w.active, format!("Silence in window {} ({})", i + base, w.name)));
                }
            }
        }
        for (sid, pid, text) in alerts {
            self.alert(sid, Some(pid), &text, false);
        }
    }

    fn detach(&mut self, cid: ClientId, reason: &str) {
        if let Some(c) = self.clients.get_mut(&cid)
            && let Some(sid) = c.session.take()
        {
            c.last_grid = None;
            c.popup = None; // its program goes with the client that opened it
            c.send(ServerMsg::Detached { reason: reason.to_string() });
            self.fit_session(sid, None); // smallest/largest: one client fewer
            self.fire_hook("client-detached", None);
        }
    }

    // ---------------------------------------------------------------- lookup

    fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
    fn session_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    fn find_pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        if self.sessions.iter().flat_map(|s| s.windows.iter()).any(|w| w.pane(id).is_some()) {
            return self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).find_map(|w| w.pane_mut(id));
        }
        // A killed pane kept for undo-kill still prints.
        if self.killed.iter_mut().any(|k| k.pane_mut(id).is_some()) {
            return self.killed.iter_mut().find_map(|k| k.pane_mut(id));
        }
        // A popup's pane belongs to no window, but its output still arrives.
        self.clients.values_mut().find_map(|c| c.popup.as_mut().filter(|p| p.pane.id == id).map(|p| &mut p.pane))
    }

    /// The client whose popup owns this pane, if any.
    fn popup_of_pane(&self, id: PaneId) -> Option<ClientId> {
        self.clients.values().find(|c| c.popup.as_ref().is_some_and(|p| p.pane.id == id)).map(|c| c.id)
    }

    fn session_of_pane(&self, id: PaneId) -> Option<SessionId> {
        self.sessions.iter().find(|s| s.windows.iter().any(|w| w.pane(id).is_some())).map(|s| s.id)
    }

    /// The pane a `rename-pane` name belongs to.
    fn pane_by_name(&self, name: &str) -> Option<PaneId> {
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .find(|p| p.actor.name.as_deref() == Some(name))
            .map(|p| p.id)
    }

    /// A pane's full address, `$session:@window.%pane`: ids that stay the
    /// same while the server runs, whatever else moves or is renamed.
    fn address(&self, sid: SessionId, widx: usize, pid: PaneId) -> String {
        let wid = self.session(sid).and_then(|s| s.windows.get(widx)).map_or(0, |w| w.id);
        format!("${sid}:@{wid}.%{pid}")
    }

    /// The pane a target names by id or name (`%7`, `%builder`,
    /// `$1:@3.%7`), and where it is. A session or window given with it is
    /// where the pane is expected to be: a pane that has moved since is an
    /// error, never a message to wherever it went.
    fn named_pane(&self, t: &Target) -> Result<Option<(SessionId, usize, PaneId)>, String> {
        let id = match (t.pane_id, &t.pane_name) {
            (Some(id), _) => id,
            (None, Some(name)) => self.pane_by_name(name).ok_or_else(|| format!("can't find pane: %{name}"))?,
            (None, None) => return Ok(None),
        };
        let (sid, widx) = self
            .sessions
            .iter()
            .find_map(|s| s.windows.iter().position(|w| w.pane(id).is_some()).map(|widx| (s.id, widx)))
            .ok_or_else(|| format!("can't find pane: %{id}"))?;
        if t.session.is_some() || t.window.is_some() {
            let place = Target { session: t.session.clone(), window: t.window.clone(), ..Default::default() };
            let s = if t.session.is_some() { self.resolve_session(Some(&place), None)? } else { sid };
            let w = if t.window.is_some() { self.resolve_window(s, Some(&place))? } else { widx };
            if (s, w) != (sid, widx) {
                return Err(format!("pane %{id} has moved: it is at {}", self.address(sid, widx, id)));
            }
        }
        Ok(Some((sid, widx, id)))
    }

    /// Session for a command: explicit target, else the client's attached
    /// session, else the session of the pane the client runs in, else the
    /// most recently used.
    fn resolve_session(&self, target: Option<&Target>, cid: Option<ClientId>) -> Result<SessionId, String> {
        if let Some(t) = target {
            Self::here(t)?;
        }
        if let Some(t) = target.filter(|t| t.names_pane()) {
            return Ok(self.named_pane(t)?.expect("names a pane").0);
        }
        if let Some(t) = target
            && let Some(name) = &t.session
        {
            return self
                .sessions
                .iter()
                .find(|s| s.name == *name || name.strip_prefix('$').and_then(|n| n.parse::<u32>().ok()) == Some(s.id))
                .map(|s| s.id)
                .or_else(|| {
                    // Prefix match like tmux.
                    let m: Vec<_> = self.sessions.iter().filter(|s| s.name.starts_with(name.as_str())).collect();
                    if m.len() == 1 { Some(m[0].id) } else { None }
                })
                .ok_or_else(|| format!("can't find session: {name}"));
        }
        if let Some(c) = cid.and_then(|c| self.clients.get(&c)) {
            if let Some(s) = c.session {
                return Ok(s);
            }
            if let Some(p) = c.pane_env
                && let Some(s) = self.session_of_pane(p)
            {
                return Ok(s);
            }
        }
        self.sessions.iter().max_by_key(|s| s.last_used).map(|s| s.id).ok_or_else(|| "no sessions".to_string())
    }

    fn resolve_window(&self, sid: SessionId, target: Option<&Target>) -> Result<usize, String> {
        let s = self.session(sid).ok_or("no such session")?;
        if let Some(t) = target.filter(|t| t.names_pane()) {
            let (psid, widx, id) = self.named_pane(t)?.expect("names a pane");
            return if psid == sid { Ok(widx) } else { Err(format!("can't find pane: %{id}")) };
        }
        let Some(w) = target.and_then(|t| t.window.as_ref()) else { return Ok(s.cur) };
        if let Ok(n) = w.parse::<usize>() {
            let idx = n.wrapping_sub(self.opts.base_index);
            if idx < s.windows.len() {
                return Ok(idx);
            }
            return Err(format!("no window {n}"));
        }
        match w.as_str() {
            "+" | "next" => return Ok((s.cur + 1) % s.windows.len().max(1)),
            "-" | "prev" => return Ok((s.cur + s.windows.len().max(1) - 1) % s.windows.len().max(1)),
            "!" | "last" => {
                return s
                    .last
                    .and_then(|id| s.windows.iter().position(|x| x.id == id))
                    .ok_or_else(|| "no last window".into());
            }
            _ => {}
        }
        s.windows
            .iter()
            .position(|x| x.name == *w || w.strip_prefix('@').and_then(|n| n.parse::<u32>().ok()) == Some(x.id))
            .ok_or_else(|| format!("can't find window: {w}"))
    }

    fn resolve_pane(&self, sid: SessionId, widx: usize, target: Option<&Target>) -> Result<PaneId, String> {
        let s = self.session(sid).ok_or("no such session")?;
        let w = s.windows.get(widx).ok_or("no such window")?;
        if let Some(t) = target.filter(|t| t.names_pane()) {
            let (_, _, id) = self.named_pane(t)?.expect("names a pane");
            return w.pane(id).map(|p| p.id).ok_or_else(|| format!("can't find pane: %{id}"));
        }
        match target.and_then(|t| t.pane) {
            None => Ok(w.active),
            Some(i) => w
                .layout
                .panes()
                .get(i.wrapping_sub(self.opts.pane_base_index))
                .copied()
                .ok_or_else(|| format!("no pane {i}")),
        }
    }

    /// A target of this machine: one on another (`host:port/…`,
    /// docs/design/link.md) is only for `send-message`, which never comes
    /// here with it.
    fn here(t: &Target) -> Result<(), String> {
        match &t.remote {
            Some(machine) => Err(format!(
                "{}: a pane on another machine ({machine}); only send-message goes there",
                crate::command::target_string(t)
            )),
            None => Ok(()),
        }
    }

    /// (session id, window index, pane id) for a command context.
    fn resolve(&self, target: Option<&Target>, cid: Option<ClientId>) -> Result<(SessionId, usize, PaneId), String> {
        if let Some(t) = target {
            Self::here(t)?;
        }
        if let Some(t) = target.filter(|t| t.names_pane()) {
            return Ok(self.named_pane(t)?.expect("names a pane"));
        }
        let sid = self.resolve_session(target, cid)?;
        let widx = self.resolve_window(sid, target)?;
        let pid = self.resolve_pane(sid, widx, target)?;
        Ok((sid, widx, pid))
    }

    fn window_area(&self, cols: u16, rows: u16) -> Rect {
        if self.opts.status && rows > 1 {
            Rect { x: 0, y: if self.opts.status_top { 1 } else { 0 }, w: cols, h: rows - 1 }
        } else {
            Rect { x: 0, y: 0, w: cols, h: rows }
        }
    }

    /// Whether `pane-border-status` reserves a row above (`Some(true)`) or
    /// below (`Some(false)`) each pane.
    fn border_rows(&self) -> Option<bool> {
        match self.opts.pane_border_status.as_str() {
            "top" => Some(true),
            "bottom" => Some(false),
            _ => None,
        }
    }

    fn resize_session(&mut self, sid: SessionId, cols: u16, rows: u16) {
        let area = self.window_area(cols.max(1), rows.max(1));
        let border = self.border_rows();
        if let Some(s) = self.session_mut(sid) {
            s.cols = cols.max(1);
            s.rows = rows.max(1);
            for w in &mut s.windows {
                w.relayout(area, border);
            }
        }
    }

    /// Size a session from its attached clients the way `window-size` says:
    /// `latest` takes the client that was used last (`latest` here, when it
    /// is attached), `smallest` and `largest` fold every attached client,
    /// and `manual` leaves the size to `resize-window`.
    fn fit_session(&mut self, sid: SessionId, latest: Option<ClientId>) {
        // Sized to a phone (`web-fit`): that holds until it is undone.
        if let Some((cols, rows)) = self.web_fit_size(sid) {
            self.resize_session(sid, cols, rows);
            return;
        }
        let sizes = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| (c.cols, c.rows));
        let pick = match self.opts.window_size.as_str() {
            "smallest" => sizes.reduce(|a, b| (a.0.min(b.0), a.1.min(b.1))),
            "largest" => sizes.reduce(|a, b| (a.0.max(b.0), a.1.max(b.1))),
            "manual" => None,
            _ => {
                latest.and_then(|id| self.clients.get(&id)).filter(|c| c.session == Some(sid)).map(|c| (c.cols, c.rows))
            }
        };
        if let Some((cols, rows)) = pick {
            self.resize_session(sid, cols, rows);
        }
    }

    /// `swap-pane -s a -t b` across windows: each pane takes the other's
    /// place in the other's layout, the active pane of each window follows
    /// the place (not the pane), and both windows are laid out again so the
    /// panes take their new sizes.
    fn swap_panes_across(
        &mut self,
        asid: SessionId,
        awidx: usize,
        a: PaneId,
        bsid: SessionId,
        bwidx: usize,
        b: PaneId,
    ) {
        // Take `a` out of its window, leaving `b`'s id in its place.
        let Some(pane_a) = self.session_mut(asid).and_then(|s| s.windows.get_mut(awidx)).and_then(|w| {
            let pos = w.panes.iter().position(|p| p.id == a)?;
            let order: Vec<PaneId> = w.layout.panes().into_iter().map(|p| if p == a { b } else { p }).collect();
            w.layout.set_panes(&order);
            if w.active == a {
                w.active = b;
            }
            if w.last_pane == Some(a) {
                w.last_pane = None;
            }
            Some(w.panes.remove(pos))
        }) else {
            return;
        };
        // Put it where `b` was, and take `b` out.
        let Some(pane_b) = self.session_mut(bsid).and_then(|s| s.windows.get_mut(bwidx)).and_then(|w| {
            let pos = w.panes.iter().position(|p| p.id == b)?;
            let order: Vec<PaneId> = w.layout.panes().into_iter().map(|p| if p == b { a } else { p }).collect();
            w.layout.set_panes(&order);
            if w.active == b {
                w.active = a;
            }
            if w.last_pane == Some(b) {
                w.last_pane = None;
            }
            let taken = w.panes.remove(pos);
            w.panes.push(pane_a);
            Some(taken)
        }) else {
            return; // cannot happen: `b` was resolved a moment ago
        };
        if let Some(w) = self.session_mut(asid).and_then(|s| s.windows.get_mut(awidx)) {
            w.panes.push(pane_b);
        }
        self.relayout_session(asid);
        self.relayout_session(bsid);
    }

    fn relayout_session(&mut self, sid: SessionId) {
        if let Some(s) = self.session(sid) {
            let (c, r) = (s.cols, s.rows);
            self.resize_session(sid, c, r);
        }
    }

    // ----------------------------------------------------------------- panes

    /// Environment for panes and `run-shell`: the socket, the pane, and PATH
    /// with this executable's directory first so `keepane` is callable from
    /// scripts and panes even when it was never installed.
    /// A buffer by name, or the newest one.
    fn buffer(&self, name: Option<&str>) -> Option<&(String, String)> {
        match name {
            Some(n) => self.buffers.iter().find(|(b, _)| b == n),
            None => self.buffers.first(),
        }
    }

    /// Store text as a buffer: named (replacing or appending to it) or a new
    /// automatic `bufferN` at the front, the way tmux stacks them.
    fn set_buffer(&mut self, name: Option<&str>, data: &str, append: bool) {
        if let Some(n) = name {
            if let Some(slot) = self.buffers.iter_mut().find(|(b, _)| b == n) {
                if append {
                    slot.1.push_str(data);
                } else {
                    slot.1 = data.to_string();
                }
                return;
            }
            self.buffers.insert(0, (n.to_string(), data.to_string()));
        } else {
            if append && let Some(top) = self.buffers.first_mut() {
                top.1.push_str(data);
                return;
            }
            let name = format!("buffer{}", self.next_buffer);
            self.next_buffer += 1;
            self.buffers.insert(0, (name, data.to_string()));
        }
        // 50 is what tmux keeps by default (buffer-limit).
        self.buffers.truncate(50);
    }

    fn pane_env(&self, pane_id: PaneId) -> Vec<(String, String)> {
        let mut env = vec![
            ("KEEPANE".into(), self.socket.clone()),
            ("KEEPANE_PANE".into(), pane_id.to_string()),
            ("PATH".into(), path_with_self()),
        ];
        // What the pane's programs draw on is keepane, not the terminal the
        // server was started from (tmux does the same).
        if crate::platform::shell::SETS_TERM {
            env.push(("TERM".into(), self.opts.default_terminal.clone()));
        }
        // `set-environment` entries last, so they can override even PATH.
        env.extend(self.env.iter().cloned());
        env
    }

    /// Split pane `pid` of a window in two (or, with `full`, the whole
    /// window) and start a program in the new half. Focus is the caller's.
    #[allow(clippy::too_many_arguments)]
    fn split_pane(
        &mut self,
        sid: SessionId,
        widx: usize,
        pid: PaneId,
        horizontal: bool,
        cwd: Option<&str>,
        argv: &[String],
        before: bool,
        full: bool,
    ) -> Result<PaneId, String> {
        let (scols, srows) = self.session(sid).map(|s| (s.cols, s.rows)).ok_or("no such session")?;
        let area = self.window_area(scols, srows);
        let border = self.border_rows();
        // Unzoom first so the layout rectangles are real.
        let w = &mut self.session_mut(sid).unwrap().windows[widx];
        if w.zoomed {
            w.zoomed = false;
            w.relayout(area, border);
        }
        let rect = if full { area } else { w.layout_rect(pid).ok_or("pane has no layout")? };
        // Each half needs a row of content, plus its border-status row.
        let min_rows = if border.is_some() { 5 } else { 3 };
        if (horizontal && rect.w < 3) || (!horizontal && rect.h < min_rows) {
            return Err("pane too small to split".into());
        }
        let (nw, nh) = if horizontal { ((rect.w - 1) / 2, rect.h) } else { (rect.w, (rect.h - 1) / 2) };
        let pane = self.spawn_pane(argv, cwd, nw.max(1), nh.max(1), ShellHistory::From(Some(pid)))?;
        let w = &mut self.session_mut(sid).unwrap().windows[widx];
        let nid = pane.id;
        if full {
            w.layout.split_root(horizontal, nid, rect, before);
        } else {
            w.layout.split_at(pid, horizontal, nid, rect, before);
        }
        w.panes.push(pane);
        self.relayout_session(sid);
        Ok(nid)
    }

    fn spawn_pane(
        &mut self,
        argv: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        hist: ShellHistory,
    ) -> Result<Pane, String> {
        self.spawn_pane_replaying(argv, cwd, cols, rows, None, hist)
    }

    /// As `spawn_pane`, with a resumed pane's saved output printed into its
    /// console first.
    fn spawn_pane_replaying(
        &mut self,
        argv: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        replay: Option<String>,
        hist: ShellHistory,
    ) -> Result<Pane, String> {
        let id = self.alloc_id();
        let argv = if argv.is_empty() { resolve_shell(&self.opts) } else { argv.to_vec() };
        let mut env = self.pane_env(id);
        // Only an interactive shell that gets keepane's hook (PowerShell,
        // bash, zsh) has a history file of its own.
        let hooked = crate::config::with_shell_integration(&argv) != argv;
        let name = match hist {
            _ if !hooked => None,
            ShellHistory::Shared => None,
            // A resumed pane whose file is gone starts over as a new one would.
            ShellHistory::Keep(name) => {
                self.seed_shell_history(&name, None);
                Some(name)
            }
            ShellHistory::From(from) => {
                let name = format!("{:x}-{id}", chrono::Utc::now().timestamp_millis());
                self.seed_shell_history(&name, from);
                Some(name)
            }
        };
        if let Some(n) = &name {
            env.push((
                crate::platform::shell::HISTORY_VAR.into(),
                self.shell_history_path(n).to_string_lossy().into_owned(),
            ));
        }
        let mut pane = Pane::spawn_replaying(
            id,
            &argv,
            cwd,
            cols,
            rows,
            self.opts.history_limit,
            &env,
            self.pane_tx.clone(),
            replay,
        )
        .map_err(|e| format!("{e:#}"))?;
        pane.shell_history = name;
        Ok(pane)
    }

    /// A pane's PowerShell history file, by name.
    fn shell_history_path(&self, name: &str) -> PathBuf {
        self.sessions_dir().join("psreadline").join(format!("{name}.txt"))
    }

    /// `shell-history`: pane `pid`'s own history, an entry a line (numbered
    /// by its place in the history, the lines of an entry of several under
    /// its first), a message delivered as a command marked `✉ #id from`;
    /// `commands_only` leaves the messages out.
    fn shell_history_of(&mut self, pid: PaneId, commands_only: bool) -> Result<Vec<String>, String> {
        let p = self.pane_ref(pid).ok_or("no such pane")?;
        let no =
            || format!("%{pid} keeps no shell history (not a PowerShell, bash or zsh pane); -m lists its messages");
        let shell = p.argv.first().and_then(|a| shellhist::Shell::of_program(a)).ok_or_else(no)?;
        let name = p.shell_history.clone().ok_or_else(no)?;
        let path = self.shell_history_path(&name);
        // A shell that has run nothing yet may not have written its file.
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut out = Vec::new();
        for (i, entry) in shellhist::entries(&bytes, shell).iter().enumerate() {
            let shown = match shellhist::delivered(entry) {
                Some(_) if commands_only => continue,
                Some(d) => format!("✉ #{} {}  {}", d.id, d.from, d.command),
                None => entry.clone(),
            };
            let mut lines = shown.lines();
            out.push(format!("{:>5}  {}", i + 1, lines.next().unwrap_or_default()));
            out.extend(lines.map(|l| format!("       {l}")));
        }
        Ok(out)
    }

    /// `shell-history -m`: every message sent to pane `pid` that the event
    /// log still has, oldest first: when, its id, who from, the mode it was
    /// for, where it stands, and its text's first line.
    fn messages_to(&self, pid: PaneId) -> Result<Vec<String>, String> {
        self.pane_ref(pid).ok_or("no such pane")?;
        let to_pane = |to: &str| to.rsplit_once('%').and_then(|(_, n)| n.parse::<PaneId>().ok()) == Some(pid);
        Ok(self
            .observe
            .records()
            .filter(|r| to_pane(&r.msg.to))
            .map(|r| {
                let m = &r.msg;
                let mut text = m.text.lines();
                let first = text.next().unwrap_or_default();
                let more = if text.next().is_some() { " …" } else { "" };
                format!(
                    "{}  #{}  {} → {}  {}  {first}{more}",
                    m.at.format("%m-%d %H:%M"),
                    m.id,
                    m.from.short(),
                    m.via.as_str(),
                    r.stage.as_str()
                )
            })
            .collect())
    }

    /// Give history file `name` a start when it does not exist yet: a copy of
    /// pane `from`'s, or of PowerShell's shared file. A new shell has what the
    /// one it came from had.
    fn seed_shell_history(&self, name: &str, from: Option<PaneId>) {
        let path = self.shell_history_path(name);
        if path.is_file() {
            return;
        }
        // The directory in any case: the shell writes the file itself, and
        // bash and zsh (unlike PSReadLine) make no directory for it. With
        // nothing to start from (no shared history yet) it is all there is.
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::warn!("shell history {}: {e}", dir.display());
            return;
        }
        let source = from
            .and_then(|p| self.pane_ref(p))
            .and_then(|p| p.shell_history.as_deref())
            .map(|n| self.shell_history_path(n))
            .filter(|p| p.is_file())
            .or_else(crate::platform::shell::shared_history);
        if let Some(src) = source
            && let Err(e) = std::fs::copy(&src, &path)
        {
            log::warn!("shell history {}: {e}", path.display());
        }
    }

    /// Remove the history files no pane and no saved session refers to that
    /// have not been written for `log-history-days` days (0 keeps them).
    fn prune_shell_histories(&self) {
        let days = self.opts.log_history_days;
        if days == 0 {
            return;
        }
        let mut keep: HashSet<String> = self
            .sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .filter_map(|p| p.shell_history.clone())
            .collect();
        for (_, _, path) in crate::resurrect::list(&self.sessions_dir()) {
            if let Ok(f) = crate::resurrect::SavedFile::load(&path) {
                for w in &f.session.windows {
                    keep.extend(w.layout.panes().into_iter().filter_map(|p| p.shell_history.clone()));
                }
            }
        }
        if let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(u64::from(days) * 86_400)) {
            prune_shell_history_files(&self.sessions_dir().join("psreadline"), &keep, cutoff);
        }
    }

    /// (session, window index) holding a pane.
    fn window_of_pane(&self, pid: PaneId) -> Option<(SessionId, usize)> {
        self.sessions.iter().find_map(|s| s.windows.iter().position(|w| w.pane(pid).is_some()).map(|widx| (s.id, widx)))
    }

    /// Take a live pane out of its window without killing it, collapsing the
    /// window if it was the last one (`join-pane`).
    fn take_pane(&mut self, sid: SessionId, widx: usize, pid: PaneId) -> Option<Pane> {
        let s = self.session_mut(sid)?;
        let w = s.windows.get_mut(widx)?;
        let pos = w.panes.iter().position(|p| p.id == pid)?;
        let pane = w.panes.remove(pos);
        w.layout.remove(pid);
        w.layout_preset = None;
        w.zoomed = false;
        if w.active == pid {
            w.active = w.layout.panes().first().copied().unwrap_or(0);
        }
        if w.last_pane == Some(pid) {
            w.last_pane = None;
        }
        if w.panes.is_empty() {
            let wid = w.id;
            s.windows.remove(widx);
            if s.last == Some(wid) {
                s.last = None;
            }
            if s.windows.is_empty() {
                // The session has nothing left; it goes the way it does when
                // its last pane exits.
                self.kill_session(sid, "exited");
            } else if let Some(s) = self.session_mut(sid) {
                s.cur = s.cur.min(s.windows.len() - 1);
            }
        }
        Some(pane)
    }

    /// Remove a pane (dead or killed) and collapse empty windows/sessions.
    /// `kill-pane` with `undo-kill-time` set: take pane `id` out of its
    /// window but keep it, running, for `undo-kill`. The last pane of a
    /// window keeps the whole window instead; the last one of a session is
    /// not kept (the session ends with it). False when nothing was kept.
    fn keep_killed_pane(&mut self, id: PaneId) -> bool {
        let Some(sid) = self.session_of_pane(id) else { return false };
        let s = self.session(sid).unwrap();
        let Some(widx) = s.windows.iter().position(|w| w.pane(id).is_some()) else { return false };
        let w = &s.windows[widx];
        if w.panes.len() == 1 {
            return self.keep_killed_window(sid, widx);
        }
        let (window, layout) = (w.id, w.layout.clone());
        let Some(mut pane) = self.remove_pane(id, None) else { return false };
        // Still running: if it is never brought back, dropping it must
        // end its program (a pane marked exited is not killed on drop).
        pane.exit_code = None;
        self.killed.push(Killed::Pane { pane: Box::new(pane), session: sid, window, layout, at: Instant::now() });
        self.trim_killed();
        true
    }

    /// `kill-window` with `undo-kill-time` set: take the window out of its
    /// session but keep it, panes running, for `undo-kill`. A session's
    /// only window is not kept. False when nothing was kept.
    fn keep_killed_window(&mut self, sid: SessionId, widx: usize) -> bool {
        let Some(s) = self.session(sid) else { return false };
        if s.windows.len() < 2 || widx >= s.windows.len() {
            return false;
        }
        let panes: Vec<PaneId> = s.windows[widx].panes.iter().map(|p| p.id).collect();
        for p in panes {
            self.pane_closing(p);
        }
        let s = self.session_mut(sid).expect("checked above");
        let w = s.windows.remove(widx);
        // The current window moves as it does when a window's last pane exits.
        if s.last == Some(w.id) {
            s.last = None;
        }
        if let Some(last) = s.last.and_then(|l| s.windows.iter().position(|x| x.id == l)) {
            s.cur = last;
            s.last = None;
        } else if s.cur >= s.windows.len() || s.cur > widx {
            s.cur = s.cur.saturating_sub(1).min(s.windows.len() - 1);
        }
        self.killed.push(Killed::Window { window: w, session: sid, index: widx, at: Instant::now() });
        self.trim_killed();
        self.relayout_session(sid);
        true
    }

    /// At most twenty kills are kept; older ones end now.
    fn trim_killed(&mut self) {
        while self.killed.len() > 20 {
            self.killed.remove(0);
        }
    }

    /// Tell the client that did it how to take a kill back.
    fn say_undo(&mut self, cid: Option<ClientId>, kept: usize, what: &str) {
        let Some(cid) = cid.filter(|_| kept > 0) else { return };
        let what = if kept == 1 { format!("{what} killed") } else { format!("{kept} {what}s killed") };
        let secs = self.opts.undo_kill_time;
        self.message(cid, &format!("{what}; undo-kill (prefix u) brings it back within {secs}s"));
    }

    /// `undo-kill`: put back the pane or window killed last, where it was.
    fn undo_kill(&mut self) -> Outcome {
        let Some(mut k) = self.killed.pop() else {
            return Outcome::Error("nothing to undo: no pane or window killed lately".into());
        };
        // A name another pane took while this one was away stays with that
        // one: names are unique, or `%name` would find either.
        let back: Vec<&mut Pane> = match &mut k {
            Killed::Pane { pane, .. } => vec![&mut **pane],
            Killed::Window { window, .. } => window.panes.iter_mut().collect(),
        };
        for p in back {
            if let Some(name) = p.actor.name.clone()
                && self.pane_by_name(&name).is_some()
            {
                p.actor.name = None;
                let note =
                    format!("undo-kill: pane name '{name}' was taken meanwhile; %{} comes back without it", p.id);
                self.note_message(&note);
            }
        }
        match k {
            Killed::Window { window, session, index, .. } => {
                let Some(s) = self.session_mut(session) else {
                    return Outcome::Error("its session is gone".into());
                };
                let i = index.min(s.windows.len());
                s.windows.insert(i, window);
                s.select_window(i);
                self.relayout_session(session);
            }
            Killed::Pane { pane, session, window, layout, .. } => {
                let Some(s) = self.session_mut(session) else {
                    return Outcome::Error("its session is gone".into());
                };
                match s.windows.iter().position(|w| w.id == window) {
                    Some(widx) => {
                        let w = &mut s.windows[widx];
                        // Nothing else changed: the layout it was in comes
                        // back exactly. Otherwise it splits the active pane.
                        let before: Vec<PaneId> = layout.panes().into_iter().filter(|p| *p != pane.id).collect();
                        if w.layout.panes() == before {
                            w.layout = layout;
                        } else {
                            let rect =
                                w.rects.iter().find(|(id, _)| *id == w.active).map(|(_, r)| *r).unwrap_or_default();
                            let horizontal = rect.w >= rect.h * 2;
                            w.layout.split(w.active, horizontal, pane.id, rect);
                        }
                        w.zoomed = false;
                        w.last_pane = Some(w.active);
                        w.active = pane.id;
                        w.panes.push(*pane);
                        s.select_window(widx);
                    }
                    // Its window went since: a window of its own.
                    None => {
                        self.add_window(session, *pane, None);
                    }
                }
                self.relayout_session(session);
            }
        }
        Outcome::Ok
    }

    /// Take a pane out of its window (the window out of its session when it
    /// was the last, the session off the server when that was the last) and
    /// hand it back: dropping it ends it; `kill-pane` may keep it a while
    /// for `undo-kill`.
    fn remove_pane(&mut self, id: PaneId, exit_code: Option<u32>) -> Option<Pane> {
        let sid = self.session_of_pane(id)?;
        self.pane_closing(id);
        let mut session_dead = false;
        let removed;
        {
            let s = self.session_mut(sid).unwrap();
            let widx = s.windows.iter().position(|w| w.pane(id).is_some()).unwrap();
            let w = &mut s.windows[widx];
            if let Some(p) = w.pane_mut(id) {
                p.exit_code = exit_code.or(Some(0));
            }
            w.layout.remove(id);
            removed = w.panes.iter().position(|p| p.id == id).map(|i| w.panes.remove(i));
            if w.active == id {
                w.active = w
                    .last_pane
                    .filter(|l| w.pane(*l).is_some())
                    .or_else(|| w.layout.panes().first().copied())
                    .unwrap_or(0);
                w.zoomed = false;
            }
            if w.last_pane == Some(id) {
                w.last_pane = None;
            }
            if w.panes.is_empty() {
                let wid = w.id;
                s.windows.remove(widx);
                if s.last == Some(wid) {
                    s.last = None;
                }
                if s.windows.is_empty() {
                    session_dead = true;
                } else if let Some(last) = s.last.and_then(|l| s.windows.iter().position(|w| w.id == l)) {
                    s.cur = last;
                    s.last = None;
                } else if s.cur >= s.windows.len() || s.cur > widx {
                    s.cur = s.cur.saturating_sub(1).min(s.windows.len() - 1);
                } else {
                    s.cur = s.cur.min(s.windows.len() - 1);
                }
            }
        }
        if session_dead {
            self.kill_session(sid, "exited");
        } else {
            self.relayout_session(sid);
        }
        self.fire_hook("pane-exited", None);
        // A read-message -w on this pane has nothing left to wait for.
        self.wake_waits();
        removed
    }

    fn kill_session(&mut self, sid: SessionId, reason: &str) {
        let Some(pos) = self.sessions.iter().position(|s| s.id == sid) else { return };
        let panes: Vec<PaneId> = self.sessions[pos].windows.iter().flat_map(|w| w.panes.iter().map(|p| p.id)).collect();
        for p in panes {
            self.pane_closing(p);
        }
        let mut s = self.sessions.remove(pos);
        for w in &mut s.windows {
            for p in &mut w.panes {
                p.kill();
            }
        }
        let ids: Vec<ClientId> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for cid in ids {
            self.detach(cid, reason);
        }
        log::info!("session {} killed ({reason})", s.name);
    }

    fn new_window(
        &mut self,
        sid: SessionId,
        name: Option<String>,
        cwd: Option<&str>,
        argv: &[String],
    ) -> Result<usize, String> {
        let (cols, rows) = self.session(sid).map(|s| (s.cols, s.rows)).ok_or("no such session")?;
        let area = self.window_area(cols, rows);
        // A new window's shell starts from the history of the pane in use.
        let from = self.session(sid).and_then(|s| s.window()).map(|w| w.active);
        let pane = self.spawn_pane(argv, cwd, area.w, area.h, ShellHistory::From(from))?;
        Ok(self.add_window(sid, pane, name))
    }

    /// A new window around `pane`, at the end of the session's list, made
    /// current. Returns its index.
    fn add_window(&mut self, sid: SessionId, pane: Pane, name: Option<String>) -> usize {
        let (cols, rows) = self.session(sid).map(|s| (s.cols, s.rows)).unwrap_or((80, 24));
        let area = self.window_area(cols, rows);
        let wid = self.alloc_id();
        let name = name.unwrap_or_else(|| pane.command.clone());
        let mut w = Window {
            id: wid,
            name,
            layout: Node::Leaf(pane.id),
            active: pane.id,
            panes: vec![pane],
            last_pane: None,
            zoomed: false,
            layout_preset: None,
            synchronized: false,
            rects: Vec::new(),
            layout_rects: Vec::new(),
            alert_activity: false,
            alert_bell: false,
            alert_silence: false,
            alert_mail: false,
            last_output: Instant::now(),
            anim: None,
        };
        w.relayout(area, self.border_rows());
        let s = self.session_mut(sid).unwrap();
        s.windows.push(w);
        let idx = s.windows.len() - 1;
        s.select_window(idx);
        idx
    }

    fn new_session(
        &mut self,
        name: Option<String>,
        window_name: Option<String>,
        cwd: Option<&str>,
        argv: &[String],
        cols: u16,
        rows: u16,
    ) -> Result<SessionId, String> {
        let name = match name {
            Some(n) => {
                if n.contains(':') || n.contains('.') || n.is_empty() {
                    return Err(format!("bad session name: {n}"));
                }
                if self.sessions.iter().any(|s| s.name == n) {
                    return Err(format!("duplicate session: {n}"));
                }
                n
            }
            None => {
                let mut i = 0;
                loop {
                    let n = i.to_string();
                    if !self.sessions.iter().any(|s| s.name == n) {
                        break n;
                    }
                    i += 1;
                }
            }
        };
        let sid = self.alloc_id();
        self.sessions.push(Session {
            id: sid,
            name,
            windows: Vec::new(),
            cur: 0,
            last: None,
            cols: cols.max(1),
            rows: rows.max(1),
            created: Instant::now(),
            created_at: chrono::Local::now().timestamp(),
            last_used: Instant::now(),
        });
        self.had_session = true;
        if let Err(e) = self.new_window(sid, window_name, cwd, argv) {
            self.sessions.retain(|s| s.id != sid);
            return Err(e);
        }
        self.fire_hook("after-new-session", None);
        Ok(sid)
    }

    fn client_cwd(&self, cid: Option<ClientId>) -> Option<String> {
        cid.and_then(|c| self.clients.get(&c)).map(|c| c.cwd.clone()).filter(|s| !s.is_empty())
    }

    /// Working directory for a new pane: explicit, else the active pane's
    /// starting directory, else the client's.
    fn pane_cwd(&self, explicit: Option<&str>, sid: SessionId, cid: Option<ClientId>) -> Option<String> {
        if let Some(c) = explicit {
            return Some(c.to_string());
        }
        if let Some(c) = self.client_cwd(cid) {
            return Some(c);
        }
        self.session(sid).and_then(|s| s.window()).and_then(|w| w.active_pane()).and_then(|p| p.cwd.clone())
    }

    // -------------------------------------------------------------- commands

    fn exec(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Outcome {
        match cmd {
            Cmd::Version => Outcome::Text(format!("keepane {}", env!("CARGO_PKG_VERSION"))),
            c @ (Cmd::SendMessage { .. }
            | Cmd::ReadMessage { .. }
            | Cmd::ListMessages { .. }
            | Cmd::TraceMessage { .. }
            | Cmd::DropMessage { .. }
            | Cmd::MoveMessage { .. }
            | Cmd::PaneReady { .. }
            | Cmd::PaneStatus { .. }
            | Cmd::SetWorkMode { .. }
            | Cmd::RenamePane { .. }
            | Cmd::Whoami
            | Cmd::ListTasks { .. }
            | Cmd::ShowTask { .. }
            | Cmd::ListEvents { .. }
            | Cmd::CreatePane(_)) => self.exec_mail(c, cid),
            Cmd::NewSession { name, window_name, cwd, detached, argv, attach_existing, size } => {
                let client = cid.and_then(|c| self.clients.get(&c));
                let interactive = client.is_some_and(|c| c.interactive);
                let (mut cols, mut rows) = client.map(|c| (c.cols, c.rows)).unwrap_or((80, 24));
                // A session nobody is attaching to has no terminal to size
                // it; -x/-y say instead (tmux ignores them otherwise too).
                if detached || !interactive {
                    cols = size.0.unwrap_or(cols);
                    rows = size.1.unwrap_or(rows);
                }
                let inside = client.is_some_and(|c| c.pane_env.is_some() && c.session.is_none());
                if inside && !detached && interactive {
                    return Outcome::Error("sessions should be nested with care, unset KEEPANE to force".into());
                }
                // `new -A` with no name (a bare `keepane`): into what is there
                // rather than one more session: the one used last; else the
                // saved ones back, into the one saved last; else a new one.
                if attach_existing && name.is_none() && !detached && interactive {
                    if !self.sessions.is_empty() {
                        return self.exec(Cmd::AttachSession { target: None, detach_others: false }, cid);
                    }
                    let (created, problems) = self.restore_all(Some((cols, rows)));
                    for p in &problems {
                        log::warn!("resume: {p}");
                    }
                    if let Some(&sid) = created.first() {
                        return Outcome::Attach(sid);
                    }
                }
                if attach_existing
                    && let Some(n) = &name
                    && self.sessions.iter().any(|s| s.name == *n)
                {
                    if detached || !interactive {
                        return Outcome::Ok;
                    }
                    return self.exec(Cmd::AttachSession { target: Some(Target::parse(n)), detach_others: false }, cid);
                }
                let cwd = cwd.or_else(|| self.client_cwd(cid));
                match self.new_session(name, window_name, cwd.as_deref(), &argv, cols, rows) {
                    Ok(sid) => {
                        if detached || !interactive {
                            Outcome::Ok
                        } else {
                            Outcome::Attach(sid)
                        }
                    }
                    Err(e) => Outcome::Error(e),
                }
            }
            Cmd::AttachSession { target, detach_others } => {
                let Some(cid) = cid else { return Outcome::Error("attach-session: no client".into()) };
                let sid = match self.resolve_session(target.as_ref(), Some(cid)) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                if detach_others {
                    let others: Vec<ClientId> =
                        self.clients.values().filter(|c| c.session == Some(sid) && c.id != cid).map(|c| c.id).collect();
                    for o in others {
                        self.detach(o, "detached (attach -d)");
                    }
                }
                let c = self.clients.get(&cid).unwrap();
                if !c.interactive {
                    return Outcome::Error("not a terminal".into());
                }
                if c.session.is_some() {
                    // Already attached: switch.
                    return self.exec(
                        Cmd::SwitchClient {
                            next: false,
                            prev: false,
                            last: false,
                            target: Some(Target {
                                session: self.session(sid).map(|s| s.name.clone()),
                                ..Default::default()
                            }),
                        },
                        Some(cid),
                    );
                }
                Outcome::Attach(sid)
            }
            Cmd::DetachClient { all, target } => {
                // No -a: just this client. With -a (or -s session): every
                // client, or every client of that session.
                let only: Option<SessionId> = match &target {
                    Some(t) if t.session.is_some() => match self.resolve_session(Some(t), cid) {
                        Ok(s) => Some(s),
                        Err(e) => return Outcome::Error(e),
                    },
                    _ => None,
                };
                let victims: Vec<ClientId> = if all || only.is_some() {
                    self.clients
                        .values()
                        .filter(|c| c.session.is_some() && only.is_none_or(|s| c.session == Some(s)))
                        .map(|c| c.id)
                        .collect()
                } else {
                    cid.into_iter().collect()
                };
                for v in victims {
                    self.detach(v, "detached");
                }
                Outcome::Ok
            }
            Cmd::ShowMessages => {
                if self.messages.is_empty() {
                    return Outcome::Text("no messages".into());
                }
                Outcome::Text(self.messages.iter().cloned().collect::<Vec<String>>().join("\n"))
            }
            Cmd::SetEnvironment { name, value, remove } => {
                self.env.retain(|(k, _)| *k != name);
                match (remove, value) {
                    (true, _) | (_, None) => {}
                    (false, Some(v)) => self.env.push((name, v)),
                }
                Outcome::Ok
            }
            Cmd::ShowEnvironment { name } => {
                let lines: Vec<String> = match &name {
                    Some(n) => match self.env.iter().find(|(k, _)| k == n) {
                        Some((k, v)) => vec![format!("{k}={v}")],
                        None => return Outcome::Error(format!("unknown variable: {n}")),
                    },
                    None => self.env.iter().map(|(k, v)| format!("{k}={v}")).collect(),
                };
                if lines.is_empty() {
                    return Outcome::Text("no variables set".into());
                }
                Outcome::Text(lines.join("\n"))
            }
            Cmd::RespawnPane { target, kill, argv, window } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let ids: Vec<PaneId> =
                    if window { self.session(sid).unwrap().windows[widx].layout.panes() } else { vec![pid] };
                let (history, tx) = (self.opts.history_limit, self.pane_tx.clone());
                let mut done = 0;
                let mut unfinished = Vec::new();
                for id in ids {
                    // The same environment a new pane gets: KEEPANE, KEEPANE_PANE
                    // and PATH, not just the set-environment entries.
                    let mut env = self.pane_env(id);
                    if let Some(n) = self.pane_ref(id).and_then(|p| p.shell_history.clone()) {
                        env.push((
                            crate::platform::shell::HISTORY_VAR.into(),
                            self.shell_history_path(&n).to_string_lossy().into_owned(),
                        ));
                    }
                    let Some(p) = self.find_pane_mut(id) else { continue };
                    if p.exit_code.is_none() && !kill {
                        continue; // tmux refuses a live pane without -k
                    }
                    let cmd = if argv.is_empty() { None } else { Some(argv.as_slice()) };
                    match p.respawn(cmd, history, &env, tx.clone()) {
                        Ok(m) => {
                            done += 1;
                            unfinished.extend(m);
                        }
                        Err(e) => return Outcome::Error(format!("respawn: {e:#}")),
                    }
                }
                // What the old programs were working on went with them.
                for m in unfinished {
                    self.observe.ended(m.id, actor::End::Abandoned, None, None);
                }
                if done == 0 {
                    return Outcome::Error("pane is still running (use -k)".into());
                }
                self.relayout_session(sid);
                self.autosave_changed();
                Outcome::Ok
            }
            Cmd::ListSessions => {
                let mut lines = Vec::new();
                for s in &self.sessions {
                    let attached = self.clients.values().filter(|c| c.session == Some(s.id)).count();
                    let ago = s.created.elapsed().as_secs();
                    lines.push(format!(
                        "{}: {} windows (created {}s ago) [{}x{}]{}",
                        s.name,
                        s.windows.len(),
                        ago,
                        s.cols,
                        s.rows,
                        if attached > 0 { " (attached)" } else { "" }
                    ));
                }
                if lines.is_empty() {
                    return Outcome::Error("no sessions".into());
                }
                Outcome::Text(lines.join("\n"))
            }
            Cmd::ListWindows { target } => {
                let sid = match self.resolve_session(target.as_ref(), cid) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                let s = self.session(sid).unwrap();
                let lines: Vec<String> = s
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(i, w)| {
                        format!(
                            "{}: {}{} ({} panes) [{}x{}]",
                            i + self.opts.base_index,
                            w.name,
                            w.flags(i == s.cur, Some(w.id) == s.last),
                            w.panes.len(),
                            s.cols,
                            s.rows
                        )
                    })
                    .collect();
                Outcome::Text(lines.join("\n"))
            }
            Cmd::ListPanes { target, all, session, format } => {
                // One window, the session's windows (`-s`), or every window
                // on the server (`-a`); beyond one window each line is
                // prefixed with its window the way tmux does it.
                let (sid, widx, _) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let windows: Vec<(SessionId, usize)> = if all {
                    self.sessions.iter().flat_map(|s| (0..s.windows.len()).map(move |i| (s.id, i))).collect()
                } else if session {
                    (0..self.session(sid).map(|s| s.windows.len()).unwrap_or(0)).map(|i| (sid, i)).collect()
                } else {
                    vec![(sid, widx)]
                };
                let mut lines = Vec::new();
                // -F: each pane as the format, answered for that pane.
                if let Some(fmt) = format {
                    for (sid, widx) in windows {
                        let ids = self.session(sid).unwrap().windows[widx].layout.panes();
                        for id in ids {
                            let ctx = self.context(sid, widx, Some(id), cid);
                            lines.push(self.expand_with_shells(&fmt, &ctx, id));
                        }
                    }
                    return Outcome::Text(lines.join("\n"));
                }
                for (sid, widx) in windows {
                    let s = self.session(sid).unwrap();
                    let w = &s.windows[widx];
                    let prefix = if all {
                        format!("{}:{}.", s.name, widx + self.opts.base_index)
                    } else if session {
                        format!("{}.", widx + self.opts.base_index)
                    } else {
                        String::new()
                    };
                    for (i, id) in w.layout.panes().iter().enumerate() {
                        let p = w.pane(*id);
                        // The pane's own size, not its rectangle: a zoomed
                        // window has no rectangle for the panes it hides, and
                        // those panes keep running at their previous size.
                        let (cols, rows) = p.map(|p| (p.cols, p.rows)).unwrap_or_default();
                        lines.push(format!(
                            "{prefix}{}: [{}x{}] %{id} {}{}{}",
                            i + self.opts.pane_base_index,
                            cols,
                            rows,
                            p.map(|p| p.display_title()).unwrap_or(""),
                            // Where it is now, as #{pane_current_path} says,
                            // not only where it started.
                            p.and_then(|p| p.current_path()).map(|d| format!(" [{d}]")).unwrap_or_default(),
                            if *id == w.active { " (active)" } else { "" }
                        ));
                    }
                }
                Outcome::Text(lines.join("\n"))
            }
            Cmd::HasSession { target } => match self.resolve_session(Some(&target), cid) {
                Ok(_) => Outcome::Ok,
                Err(e) => Outcome::Error(e),
            },
            Cmd::KillSession { target, all_but } => match self.resolve_session(target.as_ref(), cid) {
                Ok(sid) => {
                    let victims: Vec<SessionId> =
                        self.sessions.iter().map(|s| s.id).filter(|id| (*id == sid) != all_but).collect();
                    for v in victims {
                        self.kill_session(v, "session killed");
                    }
                    Outcome::Ok
                }
                Err(e) => Outcome::Error(e),
            },
            Cmd::KillServer { restarting } => {
                // Last chance to save before the tree is gone.
                self.autosave_changed();
                // Restarting: clients are told so, and attach to the new
                // server by themselves (the reason is the client's cue).
                let reason = if restarting { crate::client::RESTARTING } else { "server exited" };
                let ids: Vec<SessionId> = self.sessions.iter().map(|s| s.id).collect();
                for sid in ids {
                    self.kill_session(sid, reason);
                }
                self.quit = true;
                Outcome::Ok
            }
            Cmd::SaveSession { target, all } => {
                let ids: Vec<SessionId> = if all {
                    self.sessions.iter().map(|s| s.id).collect()
                } else {
                    match self.resolve_session(target.as_ref(), cid) {
                        Ok(s) => vec![s],
                        Err(e) => return Outcome::Error(e),
                    }
                };
                let mut lines = Vec::new();
                let mut names = Vec::new();
                for sid in ids {
                    match self.save_session_file(sid) {
                        Ok(p) => lines.push(format!("saved {}", p.display())),
                        Err(e) => return Outcome::Error(e),
                    }
                    names.extend(self.session(sid).map(|s| s.name.clone()));
                }
                // Every session (prefix C-s): one line, what was saved, each
                // into its own file.
                if all {
                    return Outcome::Text(format!(
                        "saved {}: {}",
                        crate::format::count(names.len(), "session"),
                        names.join(", ")
                    ));
                }
                Outcome::Text(lines.join("\n"))
            }
            Cmd::RestoreSession { name, attach } => {
                let client = cid.and_then(|c| self.clients.get(&c));
                let interactive = client.is_some_and(|c| c.interactive && c.session.is_none());
                // Only a terminal about to attach has a size worth using;
                // a script resuming from the side leaves the saved size.
                let size = if interactive && attach { client.map(|c| (c.cols, c.rows)) } else { None };
                let dir = self.sessions_dir();
                let (created, problems, target_sid) = match &name {
                    Some(n) => {
                        // Already running: resuming just means attaching.
                        if let Some(s) = self.sessions.iter().find(|s| s.name == *n) {
                            (Vec::new(), Vec::new(), Some(s.id))
                        } else {
                            let Some(path) = crate::resurrect::find(&dir, n) else {
                                return Outcome::Error(format!("no saved session named {n} (see list-saved)"));
                            };
                            match self.restore_session_file(&path, size) {
                                Ok((sid, p)) => (vec![sid], p, Some(sid)),
                                Err(e) => return Outcome::Error(e),
                            }
                        }
                    }
                    None => {
                        let (c, p) = self.restore_all(size);
                        let first = c.first().copied().or_else(|| self.resolve_session(None, cid).ok());
                        (c, p, first)
                    }
                };
                if attach
                    && interactive
                    && let Some(sid) = target_sid
                {
                    for p in &problems {
                        log::warn!("resume: {p}");
                    }
                    return Outcome::Attach(sid);
                }
                let mut lines = vec![format!("restored {} session(s)", created.len())];
                lines.extend(problems);
                if created.is_empty() && lines.len() == 1 && name.is_none() {
                    lines[0] = "nothing to restore (see list-saved)".into();
                }
                Outcome::Text(lines.join("\n"))
            }
            // Whoever asked has already started it.
            Cmd::StartServer => Outcome::Ok,
            Cmd::Record { target, path } => {
                let pid = match self.resolve(target.as_ref(), cid) {
                    Ok((_, _, p)) => p,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(path) = path else {
                    // No path: stop, and say which file was closed.
                    let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                    return match p.recorder.take() {
                        Some(r) => Outcome::Text(format!("recording stopped: {}", r.path)),
                        None => Outcome::Error("not recording".into()),
                    };
                };
                // A relative path is relative to where the client was run,
                // which is where the file will be looked for.
                let mut full = expand_home(&path);
                if std::path::Path::new(&full).is_relative()
                    && let Some(cwd) = cid.and_then(|c| self.clients.get(&c)).map(|c| c.cwd.clone())
                    && !cwd.is_empty()
                {
                    full = std::path::Path::new(&cwd).join(&full).to_string_lossy().into_owned();
                }
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                match p.record_to(&full) {
                    Ok(()) => Outcome::Text(format!("recording %{pid} to {full}")),
                    Err(e) => Outcome::Error(format!("{e:#}")),
                }
            }
            Cmd::FindText { pattern, target, case_sensitive, per_pane } => {
                // Which panes to look in: everything, or one session (or one
                // window of it) when -t says so.
                // No -t at all searches every session. A -t names one
                // session (`:1` meaning this client's, as everywhere else),
                // optionally one window of it, optionally one pane of that,
                // and says so when the target does not exist rather than
                // reporting an empty search.
                let (want_session, want_window, want_pane) = match &target {
                    Some(t) => {
                        let sid = match self.resolve_session(Some(t), cid) {
                            Ok(s) => s,
                            Err(e) => return Outcome::Error(e),
                        };
                        let widx = match t.window {
                            Some(_) => match self.resolve_window(sid, Some(t)) {
                                Ok(w) => Some(w),
                                Err(e) => return Outcome::Error(e),
                            },
                            None => None,
                        };
                        let pid = match t.pane {
                            Some(_) => {
                                let w = widx.or_else(|| self.session(sid).map(|s| s.cur)).unwrap_or(0);
                                match self.resolve_pane(sid, w, Some(t)) {
                                    Ok(p) => Some(p),
                                    Err(e) => return Outcome::Error(e),
                                }
                            }
                            None => None,
                        };
                        (Some(sid), widx, pid)
                    }
                    None => (None, None, None),
                };
                let base = self.opts.base_index;
                let pane_base = self.opts.pane_base_index;
                // (label, pane id, rows) first, so the panes can be searched
                // one at a time without holding a borrow of the tree.
                let mut targets: Vec<(String, PaneId, usize)> = Vec::new();
                for s in &self.sessions {
                    if want_session.is_some_and(|w| w != s.id) {
                        continue;
                    }
                    for (widx, w) in s.windows.iter().enumerate() {
                        if want_window.is_some_and(|i| i != widx) {
                            continue;
                        }
                        for (pidx, pid) in w.layout.panes().iter().enumerate() {
                            if want_pane.is_some_and(|p| p != *pid) {
                                continue;
                            }
                            let rows = w.pane(*pid).map(|p| p.rows as usize).unwrap_or(0);
                            targets.push((format!("{}:{}.{}", s.name, widx + base, pidx + pane_base), *pid, rows));
                        }
                    }
                }
                let mut hits: Vec<(String, usize, String)> = Vec::new();
                for (label, pid, rows) in targets {
                    let Some(p) = self.find_pane_mut(pid) else { continue };
                    let total = p.scrollback_len() + rows;
                    for (abs, text) in p.search(&pattern, per_pane, case_sensitive) {
                        // How far back from the newest line, which is what
                        // says whether this is fresh or ancient.
                        hits.push((label.clone(), total.saturating_sub(abs + 1), one_line(&text, 120)));
                    }
                }
                if hits.is_empty() {
                    return Outcome::Error(format!("no pane has: {}", one_line(&pattern, 60)));
                }
                // One column each for the pane and how far back it was, so a
                // list of hits reads down the page.
                let wide = hits.iter().map(|(l, _, _)| l.chars().count()).max().unwrap_or(0);
                let back_w = hits.iter().map(|(_, b, _)| b.to_string().len()).max().unwrap_or(1);
                let lines: Vec<String> = hits
                    .iter()
                    .map(|(label, back, text)| format!("{label:<wide$}  -{back:>back_w$}  {text}"))
                    .collect();
                Outcome::Text(lines.join("\n"))
            }
            Cmd::Notify { title, message } => {
                let title = match (title, cid) {
                    (Some(t), Some(cid)) => self.expand_format(&t, cid),
                    (Some(t), None) => t,
                    (None, _) => "keepane".to_string(),
                };
                let message = match cid {
                    Some(cid) => self.expand_format(&message, cid),
                    None => message,
                };
                if crate::notify::notify(&title, &message) {
                    Outcome::Ok
                } else {
                    Outcome::Error("no desktop to notify (a service, or session 0)".into())
                }
            }
            Cmd::ListSaved => {
                let dir = self.sessions_dir();
                let running: Vec<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
                let lines: Vec<String> = crate::resurrect::list(&dir)
                    .into_iter()
                    .map(|(n, at, _)| {
                        let at = at.get(..19).unwrap_or(&at).replace('T', " ");
                        format!("{n}: saved {at}{}", if running.contains(&n) { " (running)" } else { "" })
                    })
                    .collect();
                if lines.is_empty() {
                    Outcome::Text(format!("no saved sessions in {}", dir.display()))
                } else {
                    Outcome::Text(lines.join("\n"))
                }
            }
            Cmd::DeleteSaved { name } => {
                let dir = self.sessions_dir();
                if self.opts.autosave && self.sessions.iter().any(|s| s.name == name) {
                    // Autosave would write it right back on the next tick.
                    return Outcome::Error(format!("session {name} is running; kill it first (or set autosave off)"));
                }
                match crate::resurrect::find(&dir, &name) {
                    Some(p) => match std::fs::remove_file(&p) {
                        Ok(()) => {
                            self.last_saved.remove(&p.to_string_lossy().into_owned());
                            Outcome::Ok
                        }
                        Err(e) => Outcome::Error(format!("{}: {e}", p.display())),
                    },
                    None => Outcome::Error(format!("no saved session named {name}")),
                }
            }
            Cmd::RenameSession { target, name } => {
                if name.is_empty() || name.contains(':') || name.contains('.') {
                    return Outcome::Error(format!("bad session name: {name}"));
                }
                if self.sessions.iter().any(|s| s.name == name) {
                    return Outcome::Error(format!("duplicate session: {name}"));
                }
                match self.resolve_session(target.as_ref(), cid) {
                    Ok(sid) => {
                        let old = std::mem::replace(&mut self.session_mut(sid).unwrap().name, name.clone());
                        // The saved file follows the name; no ghost entry under the old one.
                        let dir = self.sessions_dir();
                        if let Some(p) = crate::resurrect::find(&dir, &old) {
                            let to = crate::resurrect::file_for(&dir, &name);
                            if let Err(e) = std::fs::rename(&p, &to) {
                                log::warn!("rename saved session {old} -> {name}: {e}");
                            }
                            self.last_saved.remove(&p.to_string_lossy().into_owned());
                        }
                        Outcome::Ok
                    }
                    Err(e) => Outcome::Error(e),
                }
            }
            Cmd::NewWindow { name, cwd, target, argv, detached } => {
                let sid = match self.resolve_session(target.as_ref(), cid) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                let cwd = self.pane_cwd(cwd.as_deref(), sid, cid);
                let before = self.session(sid).map(|s| (s.cur, s.last));
                let r = self.new_window(sid, name, cwd.as_deref(), &argv);
                if detached && let (Ok(_), Some((cur, last))) = (&r, before) {
                    let s = self.session_mut(sid).unwrap();
                    s.cur = cur;
                    s.last = last;
                }
                if r.is_ok() {
                    self.fire_hook("after-new-window", cid);
                }
                r.map(|_| ()).into()
            }
            Cmd::KillWindow { target, all_but } => {
                let (sid, widx, _) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let s = self.session(sid).unwrap();
                let wids: Vec<WindowId> =
                    s.windows.iter().enumerate().filter(|(i, _)| (*i == widx) != all_but).map(|(_, w)| w.id).collect();
                let mut kept = 0;
                for wid in wids {
                    // Looked up again each time: taking one renumbers the rest.
                    let Some(i) = self.session(sid).and_then(|s| s.windows.iter().position(|w| w.id == wid)) else {
                        continue;
                    };
                    if self.opts.undo_kill_time > 0 && self.keep_killed_window(sid, i) {
                        kept += 1;
                        continue;
                    }
                    let ids: Vec<PaneId> = self.session(sid).unwrap().windows[i].panes.iter().map(|p| p.id).collect();
                    for id in ids {
                        if let Some(p) = self.find_pane_mut(id) {
                            p.kill();
                        }
                        self.remove_pane(id, Some(0));
                    }
                }
                self.say_undo(cid, kept, "window");
                Outcome::Ok
            }
            Cmd::RenameWindow { target, name } => {
                let (sid, widx, _) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                self.session_mut(sid).unwrap().windows[widx].name = name;
                Outcome::Ok
            }
            Cmd::SelectWindow { target } => {
                let sid = match self.resolve_session(Some(&target), cid) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                match self.resolve_window(sid, Some(&target)) {
                    Ok(idx) => {
                        self.session_mut(sid).unwrap().select_window(idx);
                        self.fire_hook("after-select-window", cid);
                        Outcome::Ok
                    }
                    Err(e) => Outcome::Error(e),
                }
            }
            Cmd::SwapWindow { src, dst, move_it } => {
                let (ssid, si) = match self.resolve(src.as_ref(), cid) {
                    Ok((s, w, _)) => (s, w),
                    Err(e) => return Outcome::Error(e),
                };
                let (dsid, di) = match self.resolve(dst.as_ref(), cid) {
                    Ok((s, w, _)) => (s, w),
                    // `move-window -t :5` may name an index that is still
                    // free, which tmux takes as "put it there".
                    Err(e) => {
                        let free = dst
                            .as_ref()
                            .filter(|_| move_it)
                            .and_then(|t| t.window.as_deref())
                            .and_then(|w| w.parse::<usize>().ok());
                        match (free, self.resolve_session(dst.as_ref(), cid)) {
                            (Some(i), Ok(sid)) => (sid, i.saturating_sub(self.opts.base_index)),
                            _ => return Outcome::Error(e),
                        }
                    }
                };
                if ssid != dsid {
                    let Some(spos) = self.sessions.iter().position(|s| s.id == ssid) else {
                        return Outcome::Error("no such session".into());
                    };
                    let Some(dpos) = self.sessions.iter().position(|s| s.id == dsid) else {
                        return Outcome::Error("no such session".into());
                    };
                    if !move_it {
                        // Swap across sessions: each window takes the other's
                        // place, and each session keeps looking at the window
                        // it was looking at (which may have just arrived).
                        if di >= self.sessions[dpos].windows.len() {
                            return Outcome::Error(format!("no window {}", di + self.opts.base_index));
                        }
                        let curs =
                            [spos, dpos].map(|p| self.sessions[p].windows.get(self.sessions[p].cur).map(|w| w.id));
                        {
                            // Two sessions at once: split the list so both
                            // can be borrowed mutably.
                            let (lo, hi) = self.sessions.split_at_mut(spos.max(dpos));
                            let (sa, sb) =
                                if spos < dpos { (&mut lo[spos], &mut hi[0]) } else { (&mut hi[0], &mut lo[dpos]) };
                            std::mem::swap(&mut sa.windows[si], &mut sb.windows[di]);
                        }
                        for (p, cur) in [spos, dpos].into_iter().zip(curs) {
                            let s = &mut self.sessions[p];
                            s.last = None;
                            if let Some(i) = cur.and_then(|id| s.windows.iter().position(|w| w.id == id)) {
                                s.cur = i;
                            }
                        }
                        self.relayout_session(ssid);
                        self.relayout_session(dsid);
                        self.autosave_changed();
                        return Outcome::Ok;
                    }
                    // Move a window to another session: take it out, put it in
                    // at the destination index, and leave no empty session.
                    // Both sessions keep looking at the window they were
                    // looking at, whatever the move does to the indexes.
                    let src_cur = self.sessions[spos].windows.get(self.sessions[spos].cur).map(|w| w.id);
                    let dst_cur = self.sessions[dpos].windows.get(self.sessions[dpos].cur).map(|w| w.id);
                    let w = self.sessions[spos].windows.remove(si);
                    let wid = w.id;
                    {
                        let s = &mut self.sessions[spos];
                        if s.last == Some(wid) {
                            s.last = None;
                        }
                        s.cur = src_cur
                            .filter(|id| *id != wid)
                            .and_then(|id| s.windows.iter().position(|w| w.id == id))
                            .unwrap_or_else(|| s.cur.min(s.windows.len().saturating_sub(1)));
                    }
                    let at = di.min(self.sessions[dpos].windows.len());
                    self.sessions[dpos].windows.insert(at, w);
                    {
                        let s = &mut self.sessions[dpos];
                        if let Some(i) = dst_cur.and_then(|id| s.windows.iter().position(|w| w.id == id)) {
                            s.cur = i;
                        }
                    }
                    if self.sessions[spos].windows.is_empty() {
                        self.kill_session(ssid, "moved away");
                    } else {
                        self.relayout_session(ssid);
                    }
                    self.relayout_session(dsid);
                    self.autosave_changed();
                    return Outcome::Ok;
                }
                let cur_id = self.session(ssid).and_then(|s| s.windows.get(s.cur)).map(|w| w.id);
                let s = self.session_mut(ssid).unwrap();
                if si == di {
                    return Outcome::Ok;
                }
                if move_it {
                    let w = s.windows.remove(si);
                    s.windows.insert(di.min(s.windows.len()), w);
                } else {
                    s.windows.swap(si, di);
                }
                // Follow the window that was current, wherever it landed.
                if let Some(id) = cur_id
                    && let Some(i) = s.windows.iter().position(|w| w.id == id)
                {
                    s.cur = i;
                }
                self.autosave_changed();
                Outcome::Ok
            }
            Cmd::NextWindow { ref target, alert: true } | Cmd::PreviousWindow { ref target, alert: true } => {
                let forward = matches!(cmd, Cmd::NextWindow { .. });
                let sid = match self.resolve_session(target.as_ref(), cid) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(s) = self.session(sid) else { return Outcome::Error("no such session".into()) };
                let n = s.windows.len();
                // Walk the ring from where we are and stop at the first window
                // waiting to say something.
                let hit = (1..=n)
                    .map(|d| if forward { (s.cur + d) % n } else { (s.cur + n - d % n) % n })
                    .find(|i| s.windows[*i].alert_activity || s.windows[*i].alert_bell || s.windows[*i].alert_silence);
                match hit {
                    Some(i) => {
                        self.sessions.iter_mut().find(|s| s.id == sid).unwrap().select_window(i);
                        self.autosave_changed();
                        self.fire_hook("after-select-window", cid);
                        Outcome::Ok
                    }
                    None => Outcome::Error("no window with an alert".into()),
                }
            }
            Cmd::NextWindow { ref target, .. }
            | Cmd::PreviousWindow { ref target, .. }
            | Cmd::LastWindow { ref target } => {
                let w = match cmd {
                    Cmd::NextWindow { .. } => "+",
                    Cmd::PreviousWindow { .. } => "-",
                    _ => "!",
                };
                let target = target.clone();
                // `-t` names the session to move around in; the window part is
                // ours ("the next one", "the last one").
                let session = target.and_then(|t| t.session);
                self.exec(
                    Cmd::SelectWindow { target: Target { session, window: Some(w.into()), ..Default::default() } },
                    cid,
                )
            }
            Cmd::SplitWindow { horizontal, cwd, target, argv, detached, before, full, count } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let cwd = self.pane_cwd(cwd.as_deref(), sid, cid);
                let count = count.max(1);
                let was_active = self.session(sid).unwrap().windows[widx].active;
                // `-N count` (keepane's own): that many panes at once. Each
                // split takes the largest pane and the window is tiled after
                // it, so the panes stay even and none gets too small to split.
                let mut into = pid;
                let mut newest = None;
                let mut stopped = None;
                for made in 0..count {
                    match self.split_pane(sid, widx, into, horizontal, cwd.as_deref(), &argv, before, full) {
                        Ok(nid) => newest = Some(nid),
                        Err(e) if made == 0 => return Outcome::Error(e),
                        Err(e) => {
                            stopped = Some(format!(
                                "split-window: made {made} of {}: {e}",
                                crate::format::count(count, "pane")
                            ));
                            break;
                        }
                    }
                    if count > 1 {
                        let w = &mut self.session_mut(sid).unwrap().windows[widx];
                        if let Some(tree) = layout::Preset::Tiled.build(&w.layout.panes()) {
                            w.layout = tree;
                            w.layout_preset = Some(layout::Preset::Tiled);
                        }
                        self.relayout_session(sid);
                        let w = &self.session(sid).unwrap().windows[widx];
                        into = w
                            .layout
                            .panes()
                            .into_iter()
                            .max_by_key(|id| w.layout_rect(*id).map_or(0, |r| r.w as u32 * r.h as u32))
                            .unwrap_or(into);
                    }
                    self.fire_hook("after-split-window", cid);
                }
                if !detached && let Some(nid) = newest {
                    let w = &mut self.session_mut(sid).unwrap().windows[widx];
                    w.last_pane = Some(was_active);
                    w.active = nid;
                }
                match stopped {
                    Some(e) => Outcome::Error(e),
                    None => Outcome::Ok,
                }
            }
            Cmd::KillPane { target, all_but } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let ids: Vec<PaneId> = self.session(sid).unwrap().windows[widx]
                    .panes
                    .iter()
                    .map(|p| p.id)
                    .filter(|id| (*id == pid) != all_but)
                    .collect();
                let mut kept = 0;
                for id in ids {
                    if self.opts.undo_kill_time > 0 && self.keep_killed_pane(id) {
                        kept += 1;
                        continue;
                    }
                    if let Some(p) = self.find_pane_mut(id) {
                        p.kill();
                    }
                    self.remove_pane(id, Some(0));
                }
                self.fire_hook("after-kill-pane", cid);
                self.say_undo(cid, kept, "pane");
                Outcome::Ok
            }
            Cmd::SelectPane { sel } => {
                // A full target names the window too; anything else is in
                // the client's current window.
                let target = match &sel {
                    PaneSel::Target(t) => Some(t.clone()),
                    _ => None,
                };
                let (sid, widx, named) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let (scols, srows) = self.session(sid).map(|s| (s.cols, s.rows)).unwrap();
                let area = self.window_area(scols, srows);
                let pane_base = self.opts.pane_base_index;
                let keep_zoom = self.opts.keep_zoom;
                let border = self.border_rows();
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                let order = w.layout.panes();
                let cur = order.iter().position(|p| *p == w.active).unwrap_or(0);
                let next = match sel {
                    PaneSel::Dir(d) => {
                        // Zoomed, the only rectangle is the zoomed pane itself,
                        // so directions are answered from the real layout.
                        if w.zoomed {
                            let mut rects = Vec::new();
                            w.layout.layout(area, &mut rects);
                            layout::neighbour(&rects, w.active, d)
                        } else {
                            layout::neighbour(&w.rects, w.active, d)
                        }
                    }
                    PaneSel::Next => order.get((cur + 1) % order.len().max(1)).copied(),
                    PaneSel::Prev => order.get((cur + order.len().max(1) - 1) % order.len().max(1)).copied(),
                    PaneSel::Last => w.last_pane.filter(|l| w.pane(*l).is_some()),
                    PaneSel::Index(i) => order.get(i.wrapping_sub(pane_base)).copied(),
                    PaneSel::Target(_) => Some(named),
                };
                match next {
                    Some(id) if id != w.active => {
                        // The frame flies from the pane left; when the zoom moves,
                        // the new pane grows from where it sits in the layout.
                        let zoom_moves = w.zoomed && keep_zoom;
                        let from = if zoom_moves {
                            w.unzoomed_rects(area, border).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
                        } else {
                            w.rect_of(w.active)
                        };
                        w.last_pane = Some(w.active);
                        w.active = id;
                        // Zoomed, the zoom moves to the pane selected (`keep-zoom`,
                        // the default) or goes (tmux).
                        if w.zoomed {
                            w.zoomed = keep_zoom;
                            self.relayout_session(sid);
                        }
                        let w = &mut self.session_mut(sid).unwrap().windows[widx];
                        let to = w.rect_of(id);
                        if zoom_moves {
                            w.animate_zoom(id, from, to);
                        } else {
                            w.animate(from, to);
                        }
                        self.fire_hook("after-select-pane", cid);
                        Outcome::Ok
                    }
                    Some(_) => Outcome::Ok,
                    None => Outcome::Error("no such pane".into()),
                }
            }
            Cmd::ResizePane { dir, amount, zoom, target, width, height } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let (scols, srows) = self.session(sid).map(|s| (s.cols, s.rows)).unwrap();
                let area = self.window_area(scols, srows);
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                // Zooming: the frame grows from the pane to the window, or
                // shrinks back into it.
                let zoom_from = if zoom { w.rect_of(w.active).map(|r| (w.active, r)) } else { None };
                if zoom {
                    w.zoomed = !w.zoomed && w.panes.len() > 1;
                } else if let Some(d) = dir {
                    w.zoomed = false;
                    w.layout.resize(pid, d, amount.max(1));
                }
                // -x / -y: grow or shrink until the pane has that size.
                let border = self.border_rows();
                for (spec, horizontal) in [(width, true), (height, false)] {
                    let Some(spec) = spec else { continue };
                    let full = if horizontal { area.w } else { area.h };
                    let want = match parse_size(&spec, full) {
                        Some(v) => v,
                        None => return Outcome::Error(format!("resize-pane: bad size '{spec}'")),
                    };
                    let w = &mut self.session_mut(sid).unwrap().windows[widx];
                    w.zoomed = false;
                    let size = |w: &Window| w.rect_of(pid).map(|r| if horizontal { r.w } else { r.h });
                    let step = |grow: bool| match (horizontal, grow) {
                        (true, true) => Dir::Right,
                        (true, false) => Dir::Left,
                        (false, true) => Dir::Down,
                        (false, false) => Dir::Up,
                    };
                    // Moving a pane's edge right or down grows it, except for
                    // the last pane of a split, whose edge is its leading
                    // one: there the step goes the other way. A step that
                    // moves away from the size wanted is taken back and the
                    // rest are made the other way round.
                    let mut flipped = false;
                    for _ in 0..full {
                        w.relayout(area, border);
                        let Some(before) = size(w) else { break };
                        if before == want {
                            break;
                        }
                        let dir = step((before < want) != flipped);
                        w.layout.resize(pid, dir, 1);
                        w.relayout(area, border);
                        let after = size(w).unwrap_or(before);
                        if !flipped && after.abs_diff(want) > before.abs_diff(want) {
                            w.layout.resize(pid, step((before < want) == flipped), 1);
                            flipped = true;
                            continue;
                        }
                        // Nothing moved: this way is wedged against a minimum.
                        // The other way may not be (the last pane's leading
                        // edge); after that, stop rather than spin.
                        if after == before {
                            if flipped {
                                break;
                            }
                            flipped = true;
                        }
                    }
                }
                self.relayout_session(sid);
                if let Some((id, from)) = zoom_from {
                    let w = &mut self.session_mut(sid).unwrap().windows[widx];
                    let to = w.rect_of(id);
                    w.animate_zoom(id, Some(from), to);
                }
                Outcome::Ok
            }
            Cmd::PaneTitle { target, title, mark, unmark } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if unmark {
                    self.marked = None;
                }
                if mark {
                    self.marked = if self.marked == Some(pid) { None } else { Some(pid) };
                }
                if let Some(t) = title
                    && let Some(p) = self.find_pane_mut(pid)
                {
                    p.title = t;
                }
                Outcome::Ok
            }
            Cmd::SelectLayout { name, next, prev: _, spread, target } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                // `-E` evens out the panes next to this one and leaves the
                // rest of the layout as it is.
                if spread && name.is_none() {
                    let w = &mut self.session_mut(sid).unwrap().windows[widx];
                    if !w.layout.spread(pid) {
                        return Outcome::Error("pane has no panes beside it".into());
                    }
                    w.zoomed = false;
                    self.relayout_session(sid);
                    self.autosave_changed();
                    return Outcome::Ok;
                }
                let w = &self.session(sid).unwrap().windows[widx];
                let current = w.layout_preset;
                let preset = match name {
                    Some(n) => match layout::Preset::parse(&n) {
                        Some(p) => p,
                        // Not a name: a tmux layout string (`#{window_layout}`
                        // of this or another window, or one out of a script),
                        // which must hold as many cells as the window has panes.
                        None if n.contains('x') && n.contains(',') => {
                            let mut tree = match layout::parse_layout(&n) {
                                Ok(t) => t,
                                Err(e) => return Outcome::Error(format!("select-layout: {e}")),
                            };
                            let panes = w.layout.panes();
                            let cells = tree.panes().len();
                            if cells != panes.len() {
                                return Outcome::Error(format!(
                                    "select-layout: the layout has {}, the window {}",
                                    crate::format::count(cells, "pane"),
                                    panes.len()
                                ));
                            }
                            tree.set_panes(&panes);
                            let w = &mut self.session_mut(sid).unwrap().windows[widx];
                            w.layout = tree;
                            w.layout_preset = None;
                            w.zoomed = false;
                            self.relayout_session(sid);
                            self.autosave_changed();
                            return Outcome::Ok;
                        }
                        None => {
                            let all: Vec<&str> = layout::PRESETS.iter().map(|(n, _)| *n).collect();
                            return Outcome::Error(format!("unknown layout: {n} (one of {})", all.join(", ")));
                        }
                    },
                    // Cycling from nothing starts at the first layout.
                    None if next => current.map(|p| p.next()).unwrap_or(layout::PRESETS[0].1),
                    None => current.map(|p| p.prev()).unwrap_or(layout::PRESETS[0].1),
                };
                let panes = w.layout.panes();
                let Some(tree) = preset.build(&panes) else { return Outcome::Error("window has no panes".into()) };
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                w.layout = tree;
                w.layout_preset = Some(preset);
                w.zoomed = false;
                self.relayout_session(sid);
                self.message(cid.unwrap_or(0), preset.name());
                Outcome::Ok
            }
            Cmd::ResizeWindow { target, width, height, adjust, largest, smallest } => {
                let sid = match self.resolve_session(target.as_ref(), cid) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(e),
                };
                let Some((mut cols, mut rows)) = self.session(sid).map(|s| (s.cols, s.rows)) else {
                    return Outcome::Error("no such session".into());
                };
                // `-A` / `-a`: the largest or smallest attached client.
                if largest || smallest {
                    let sizes = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| (c.cols, c.rows));
                    let pick = if largest {
                        sizes.reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)))
                    } else {
                        sizes.reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
                    };
                    match pick {
                        Some(s) => (cols, rows) = s,
                        None => return Outcome::Error("resize-window: no client attached".into()),
                    }
                }
                if let Some(w) = width {
                    cols = w;
                }
                if let Some(h) = height {
                    rows = h;
                }
                match adjust {
                    Some((Dir::Left, n)) => cols = cols.saturating_sub(n),
                    Some((Dir::Right, n)) => cols = cols.saturating_add(n),
                    Some((Dir::Up, n)) => rows = rows.saturating_sub(n),
                    Some((Dir::Down, n)) => rows = rows.saturating_add(n),
                    None => {}
                }
                // Sized by hand: a phone's fit (`web-fit`) gives way.
                self.web_fit_dropped(sid);
                // The same floor `new -x/-y` has: anything smaller has no
                // room for a pane and a status line.
                self.resize_session(sid, cols.max(10), rows.max(3));
                self.autosave_changed();
                Outcome::Ok
            }
            Cmd::SwapPane { up, target, source } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if let Some(src) = source {
                    // The pair form: two named panes change places, in one
                    // window or across windows and sessions.
                    let (ssid, swidx, spid) = match self.resolve(Some(&src), cid) {
                        Ok(r) => r,
                        Err(e) => return Outcome::Error(e),
                    };
                    if spid == pid {
                        return Outcome::Ok;
                    }
                    if (ssid, swidx) == (sid, widx) {
                        self.session_mut(sid).unwrap().windows[widx].layout.swap(spid, pid);
                        self.relayout_session(sid);
                    } else {
                        self.swap_panes_across(ssid, swidx, spid, sid, widx, pid);
                    }
                    return Outcome::Ok;
                }
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                let order = w.layout.panes();
                let cur = order.iter().position(|p| *p == pid).unwrap_or(0);
                let n = order.len();
                if n < 2 {
                    return Outcome::Ok;
                }
                let other = if up { order[(cur + n - 1) % n] } else { order[(cur + 1) % n] };
                w.layout.swap(pid, other);
                self.relayout_session(sid);
                Outcome::Ok
            }
            Cmd::BreakPane { target } => {
                let (sid, widx, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let s = self.session_mut(sid).unwrap();
                let w = &mut s.windows[widx];
                if w.panes.len() < 2 {
                    return Outcome::Error("can't break with only one pane".into());
                }
                w.layout.remove(pid);
                let pos = w.panes.iter().position(|p| p.id == pid).unwrap();
                let pane = w.panes.remove(pos);
                if w.active == pid {
                    w.active = w.layout.panes()[0];
                }
                w.last_pane = None;
                w.zoomed = false;
                let wid = self.alloc_id();
                let s = self.session_mut(sid).unwrap();
                let name = pane.command.clone();
                s.windows.push(Window {
                    id: wid,
                    name,
                    layout: Node::Leaf(pid),
                    active: pid,
                    panes: vec![pane],
                    last_pane: None,
                    zoomed: false,
                    layout_preset: None,
                    synchronized: false,
                    rects: Vec::new(),
                    layout_rects: Vec::new(),
                    alert_activity: false,
                    alert_bell: false,
                    alert_silence: false,
                    alert_mail: false,
                    last_output: Instant::now(),
                    anim: None,
                });
                let idx = s.windows.len() - 1;
                s.select_window(idx);
                self.relayout_session(sid);
                Outcome::Ok
            }
            Cmd::JoinPane { src, dst, horizontal, before } => {
                // No -s: the marked pane, as in tmux.
                let marked = self.marked.filter(|p| self.session_of_pane(*p).is_some());
                let (ssid, swidx, spid) = match (&src, marked) {
                    (None, Some(m)) => match self.window_of_pane(m) {
                        Some((s, w)) => (s, w, m),
                        None => return Outcome::Error("join-pane: the marked pane is gone".into()),
                    },
                    _ => match self.resolve(src.as_ref(), cid) {
                        Ok(r) => r,
                        Err(e) => return Outcome::Error(e),
                    },
                };
                let (dsid, dwidx, dpid) = match self.resolve(dst.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if spid == dpid {
                    return Outcome::Error("join-pane: source and target are the same pane".into());
                }
                if ssid == dsid && swidx == dwidx && self.session(ssid).unwrap().windows[swidx].panes.len() < 2 {
                    return Outcome::Error("join-pane: the pane is already there".into());
                }
                // Make room in the destination before taking the pane out, so
                // a refused split leaves everything where it was.
                let (dcols, drows) = self.session(dsid).map(|s| (s.cols, s.rows)).unwrap();
                let area = self.window_area(dcols, drows);
                let border = self.border_rows();
                let dw = &mut self.session_mut(dsid).unwrap().windows[dwidx];
                if dw.zoomed {
                    dw.zoomed = false;
                    dw.relayout(area, border);
                }
                let Some(rect) = dw.layout_rect(dpid) else { return Outcome::Error("pane has no layout".into()) };
                let min_rows = if border.is_some() { 5 } else { 3 };
                if (horizontal && rect.w < 3) || (!horizontal && rect.h < min_rows) {
                    return Outcome::Error("pane too small to split".into());
                }
                let Some(pane) = self.take_pane(ssid, swidx, spid) else {
                    return Outcome::Error("join-pane: no such pane".into());
                };
                // The pane's own window goes when it was its last, and the
                // windows after it move down one: find the destination again.
                let Some((dsid, dwidx)) = self.window_of_pane(dpid) else {
                    return Outcome::Error("join-pane: the target pane is gone".into());
                };
                let dw = &mut self.session_mut(dsid).unwrap().windows[dwidx];
                dw.layout.split_at(dpid, horizontal, spid, rect, before);
                dw.panes.push(pane);
                dw.last_pane = Some(dw.active);
                dw.active = spid;
                dw.layout_preset = None;
                self.relayout_session(dsid);
                if ssid != dsid {
                    self.relayout_session(ssid);
                }
                self.fire_hook("after-split-window", cid);
                self.marked = None;
                self.autosave_changed();
                Outcome::Ok
            }
            Cmd::FindWindow { pattern } => {
                let Some(cid) = cid else { return Outcome::Error("find-window: no client".into()) };
                let needle = pattern.to_lowercase();
                let mut hits: Vec<(SessionId, WindowId, String)> = Vec::new();
                for s in &self.sessions {
                    for (i, w) in s.windows.iter().enumerate() {
                        let title = w.active_pane().map(|p| p.display_title().to_string()).unwrap_or_default();
                        let hay = format!("{} {} {}", w.name, title, i + self.opts.base_index).to_lowercase();
                        if hay.contains(&needle) {
                            hits.push((s.id, w.id, format!("{}:{}", s.name, w.name)));
                        }
                    }
                }
                match hits.len() {
                    0 => Outcome::Error(format!("no window matching: {pattern}")),
                    1 => {
                        let (sid, wid, _) = hits[0].clone();
                        self.chooser_go(cid, sid, Some(wid));
                        Outcome::Ok
                    }
                    _ => {
                        // Several: show them in the picker, filtered.
                        let items: Vec<ChooserItem> =
                            hits.iter().map(|(s, w, _)| ChooserItem::Tree(*s, Some(*w))).collect();
                        let lines: Vec<String> = hits
                            .iter()
                            .enumerate()
                            .map(|(i, (_, _, label))| format!("{} {label}", digits::label(Some(i), hits.len())))
                            .collect();
                        if let Some(c) = self.clients.get_mut(&cid) {
                            c.prompt = None;
                            c.overlay = None;
                            c.chooser = Some(Chooser::new(ChooserKind::Found, items, lines, 0));
                        }
                        Outcome::Ok
                    }
                }
            }
            Cmd::SendKeys { target, keys, literal } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                let app = p.screen().application_cursor();
                let mut bytes = Vec::new();
                for k in &keys {
                    if literal {
                        bytes.extend_from_slice(k.as_bytes());
                    } else if let Some(up) = wheel_word(k) {
                        // The wheel over the pane's middle: to a full-screen
                        // program (its own scrolling); nothing otherwise.
                        let (x, y) = (p.cols / 2, p.rows / 2);
                        bytes.extend(input::wheel(p.screen(), up, x, y, (false, false, false)).unwrap_or_default());
                    } else {
                        bytes.extend(input::encode_send_key(k, app));
                    }
                }
                // With synchronize-panes on, send-keys reaches the whole window too.
                let synced = self
                    .sessions
                    .iter()
                    .flat_map(|s| s.windows.iter())
                    .find(|w| w.pane(pid).is_some())
                    .is_some_and(|w| w.synchronized);
                if synced {
                    let w = self
                        .sessions
                        .iter_mut()
                        .flat_map(|s| s.windows.iter_mut())
                        .find(|w| w.pane(pid).is_some())
                        .unwrap();
                    for p in &mut w.panes {
                        p.type_input(&bytes);
                    }
                } else if let Some(p) = self.find_pane_mut(pid) {
                    p.type_input(&bytes);
                }
                Outcome::Ok
            }
            Cmd::RotateWindow { down, target } => {
                let (sid, widx, _) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                let mut order = w.layout.panes();
                if order.len() < 2 {
                    return Outcome::Ok;
                }
                // -U moves every pane one place towards the front.
                if down {
                    order.rotate_right(1);
                } else {
                    order.rotate_left(1);
                }
                w.layout.set_panes(&order);
                w.zoomed = false;
                self.relayout_session(sid);
                self.autosave_changed();
                Outcome::Ok
            }
            Cmd::RefreshClient { pan } => {
                let Some(cid) = cid else { return Outcome::Ok };
                let Some(c) = self.clients.get_mut(&cid) else { return Outcome::Ok };
                c.last_grid = None;
                c.last_cursor = None;
                if let Some((dir, n)) = pan {
                    // Pan the view; the render clamps it to the window and
                    // keeps the cursor in sight, so overshooting is harmless.
                    match dir {
                        Dir::Up => c.view_y = c.view_y.saturating_sub(n),
                        Dir::Down => c.view_y = c.view_y.saturating_add(n),
                        Dir::Left => c.view_x = c.view_x.saturating_sub(n),
                        Dir::Right => c.view_x = c.view_x.saturating_add(n),
                    }
                    c.view_pinned = true;
                }
                Outcome::Ok
            }
            Cmd::SendPrefix { target } => {
                let bytes = input::encode_key(self.opts.prefix, false);
                // With a popup open (and no target named), the prefix is for
                // the program in the box: `prefix prefix` is how it gets the
                // key the popup otherwise keeps for keepane.
                if target.is_none()
                    && let Some(p) = cid.and_then(|c| self.clients.get_mut(&c)).and_then(|c| c.popup.as_mut())
                    && !p.finished
                {
                    p.pane.write_input(&bytes);
                    return Outcome::Ok;
                }
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if let Some(p) = self.find_pane_mut(pid) {
                    p.type_input(&bytes);
                }
                Outcome::Ok
            }
            Cmd::ListCommands => {
                let mut names: Vec<&str> = crate::command::COMMANDS.to_vec();
                names.sort_unstable();
                Outcome::Text(names.join("\n"))
            }
            Cmd::ListClients => {
                let mut lines: Vec<String> = self
                    .clients
                    .values()
                    .filter(|c| c.session.is_some())
                    .map(|c| {
                        let name = c.session.and_then(|s| self.session(s)).map(|s| s.name.clone()).unwrap_or_default();
                        format!("client-{}: {} [{}x{}]", c.id, name, c.cols, c.rows)
                    })
                    .collect();
                lines.sort();
                if lines.is_empty() {
                    return Outcome::Text("no clients attached".into());
                }
                Outcome::Text(lines.join("\n"))
            }
            Cmd::CopyCommand { target, name, arg } => {
                let Some(cid) = cid else { return Outcome::Error("send-keys -X: no client".into()) };
                let (_, _, pid) = match self.resolve(target.as_ref(), cid.into()) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                // tmux's copy-mode commands, mapped onto the keys that already
                // do the work, so there is one implementation of each motion.
                let key = match name.as_str() {
                    "cursor-up" => Key::ch('k'),
                    "cursor-down" => Key::ch('j'),
                    "cursor-left" => Key::ch('h'),
                    "cursor-right" => Key::ch('l'),
                    "next-word" | "next-space" => Key::ch('w'),
                    "next-word-end" | "next-space-end" => Key::ch('e'),
                    "previous-word" | "previous-space" => Key::ch('b'),
                    "start-of-line" => Key::ch('0'),
                    "back-to-indentation" => Key::ch('^'),
                    "end-of-line" => Key::ch('$'),
                    "top-line" => Key::ch('H'),
                    "middle-line" => Key::ch('M'),
                    "bottom-line" => Key::ch('L'),
                    "previous-paragraph" => Key::ch('{'),
                    "next-paragraph" => Key::ch('}'),
                    "previous-prompt" => Key::ch('['),
                    "next-prompt" => Key::ch(']'),
                    "history-top" => Key::ch('g'),
                    "history-bottom" => Key::ch('G'),
                    "page-up" => Key::ctrl('b'),
                    "page-down" => Key::ctrl('f'),
                    "halfpage-up" => Key::ctrl('u'),
                    "halfpage-down" => Key::ctrl('d'),
                    "begin-selection" => Key::ch(' '),
                    "rectangle-toggle" | "rectangle-on" => Key::ctrl('v'),
                    "copy-selection" | "copy-selection-and-cancel" | "copy-pipe-and-cancel" => Key::ch('y'),
                    "cancel" | "stop-selection" => Key::ch('q'),
                    "search-forward" | "search-backward" => {
                        // With a pattern: search straight away. Without one:
                        // open the prompt, as the `/` and `?` keys do.
                        let back = name == "search-backward";
                        match arg {
                            Some(p) => {
                                if let Some(pane) = self.find_pane_mut(pid)
                                    && pane.copy.is_none()
                                {
                                    enter_copy_mode(pane);
                                }
                                self.search_in_copy_mode(cid, pid, &p, back, true);
                                return Outcome::Ok;
                            }
                            None => Key::ch(if back { '?' } else { '/' }),
                        }
                    }
                    "search-again" => Key::ch('n'),
                    "search-reverse" => Key::ch('N'),
                    other => return Outcome::Error(format!("send-keys -X: unknown command '{other}'")),
                };
                // The pane must be in copy mode for any of this to mean
                // something, which is also what tmux requires.
                if let Some(p) = self.find_pane_mut(pid)
                    && p.copy.is_none()
                {
                    enter_copy_mode(p);
                }
                self.copy_key(cid, pid, key);
                Outcome::Ok
            }
            Cmd::ClockMode { target } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if let Some(p) = self.find_pane_mut(pid) {
                    p.clock = true;
                }
                Outcome::Ok
            }
            Cmd::IfShell { format, condition, then_cmd, else_cmd } => {
                let truth = if format {
                    let v = match cid {
                        Some(c) => self.expand_format(&condition, c),
                        None => condition.clone(),
                    };
                    let v = v.trim();
                    !v.is_empty() && v != "0"
                } else {
                    // A shell command: success means the "then" branch, as in
                    // tmux. Same environment panes get, same 30s watchdog.
                    let env = self.pane_env(0);
                    let (_, code) = run_shell_blocking(&condition, &env, Some(Duration::from_secs(30)));
                    code == 0
                };
                match (truth, else_cmd) {
                    (true, _) => self.exec(*then_cmd, cid),
                    (false, Some(e)) => self.exec(*e, cid),
                    (false, None) => Outcome::Ok,
                }
            }
            Cmd::DisplayPanes => {
                let Some(cid) = cid else { return Outcome::Error("display-panes: no client".into()) };
                // tmux waits a second by default; `display-time` is ours.
                let ms = self.opts.display_time_ms.max(1000);
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.panes_until = Some(Instant::now() + Duration::from_millis(ms));
                }
                Outcome::Ok
            }
            Cmd::CopyMode { page_up, target } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                enter_copy_mode(p);
                if page_up {
                    let rows = p.rows as usize;
                    copy_scroll(p, rows as i64);
                }
                Outcome::Ok
            }
            Cmd::PasteBuffer { name, target, bracketed } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                // No -b: the Windows clipboard, which is also where copy mode
                // puts what it copies. With -b: that named buffer.
                let text = match &name {
                    Some(n) => match self.buffer(Some(n)) {
                        Some((_, data)) => data.clone(),
                        None => return Outcome::Error(format!("no buffer {n}")),
                    },
                    None => match crate::clipboard::get_text() {
                        Ok(t) => t,
                        Err(e) => return Outcome::Error(format!("clipboard: {e}")),
                    },
                };
                if text.is_empty() {
                    return Outcome::Error(
                        if name.is_some() { "buffer is empty" } else { "clipboard is empty" }.into(),
                    );
                }
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                let b = bracketed && p.screen().bracketed_paste();
                p.type_input(&input::encode_paste(&text, b));
                Outcome::Ok
            }
            Cmd::SetBuffer { name, data, append } => {
                self.set_buffer(name.as_deref(), &data, append);
                Outcome::Ok
            }
            Cmd::LoadBuffer { name, path } => match std::fs::read_to_string(expand_home(&path)) {
                Ok(data) => {
                    self.set_buffer(name.as_deref(), &data, false);
                    Outcome::Ok
                }
                Err(e) => Outcome::Error(format!("{path}: {e}")),
            },
            Cmd::SaveBuffer { name, path, append } => {
                let Some((_, data)) = self.buffer(name.as_deref()) else {
                    return Outcome::Error("no buffer".into());
                };
                let data = data.clone();
                let path = expand_home(&path);
                let r = if append {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut f| f.write_all(data.as_bytes()))
                } else {
                    std::fs::write(&path, data.as_bytes())
                };
                match r {
                    Ok(()) => Outcome::Ok,
                    Err(e) => Outcome::Error(format!("{path}: {e}")),
                }
            }
            Cmd::ShowBuffer { name } => match self.buffer(name.as_deref()) {
                Some((_, data)) => Outcome::Text(data.clone()),
                None => Outcome::Error("no buffer".into()),
            },
            Cmd::DeleteBuffer { name } => {
                let idx = match &name {
                    Some(n) => self.buffers.iter().position(|(b, _)| b == n),
                    None => (!self.buffers.is_empty()).then_some(0),
                };
                match idx {
                    Some(i) => {
                        self.buffers.remove(i);
                        Outcome::Ok
                    }
                    None => Outcome::Error("no buffer".into()),
                }
            }
            Cmd::ListBuffers => {
                if self.buffers.is_empty() {
                    return Outcome::Text("no buffers".into());
                }
                let lines: Vec<String> =
                    self.buffers.iter().map(|(n, d)| format!("{n}: {} bytes: {}", d.len(), one_line(d, 60))).collect();
                Outcome::Text(lines.join("\n"))
            }
            Cmd::CommandPrompt { prompt, initial, template } => {
                let Some(cid) = cid else { return Outcome::Error("command-prompt: no client".into()) };
                let initial = initial.map(|i| self.expand_format(&i, cid)).unwrap_or_default();
                let label = prompt.map(|p| self.expand_format(&p, cid)).unwrap_or_else(|| ":".into());
                let label = if label.ends_with(' ') || label == ":" { label } else { format!("{label} ") };
                if let Some(c) = self.clients.get_mut(&cid) {
                    let cursor = initial.chars().count();
                    c.prompt = Some(Prompt {
                        kind: PromptKind::Command { template },
                        label,
                        input: initial,
                        cursor,
                        hint: None,
                    });
                }
                Outcome::Ok
            }
            Cmd::ConfirmBefore { prompt, cmd } => {
                let Some(cid) = cid else { return Outcome::Error("confirm-before: no client".into()) };
                let label =
                    prompt.map(|p| self.expand_format(&p, cid)).unwrap_or_else(|| format!("Confirm '{cmd}'? (y/n)"));
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.prompt = Some(Prompt {
                        kind: PromptKind::Confirm(cmd),
                        label: format!("{label} "),
                        input: String::new(),
                        cursor: 0,
                        hint: None,
                    });
                }
                Outcome::Ok
            }
            Cmd::Jobs { target, format } => {
                // A session narrows it to that session, a window to that
                // window, a pane to that one pane (a script asking after one).
                let (only_sid, only_widx, only_pid) = match target.as_ref() {
                    Some(t) => match self.resolve(Some(t), cid) {
                        Ok((s, w, p)) => (Some(s), t.window.as_ref().map(|_| w), t.pane.as_ref().map(|_| p)),
                        Err(e) => return Outcome::Error(e),
                    },
                    None => (None, None, None),
                };
                let panes = self.jobs_panes(only_sid, only_widx, only_pid);
                match &format {
                    Some(f) => {
                        let mut lines = Vec::new();
                        for (sid, widx, pid) in panes {
                            let ctx = self.context(sid, widx, Some(pid), cid);
                            lines.push(self.expand_with_shells(f, &ctx, pid));
                        }
                        Outcome::Text(lines.join("\n"))
                    }
                    None if panes.is_empty() => Outcome::Text(String::new()),
                    None => {
                        // Standard view: a header and aligned columns.
                        let mut rows = vec![jobs_header()];
                        rows.extend(panes.into_iter().map(|(sid, widx, pid)| self.jobs_row(sid, widx, pid, cid)));
                        Outcome::Text(align_columns(&rows).join("\n"))
                    }
                }
            }
            Cmd::DisplayMessage { msg, target } => {
                if cid.is_none() && target.is_none() {
                    return Outcome::Ok; // from the config: nobody to show it to
                }
                if let Err(e) = self.resolve(target.as_ref(), cid) {
                    return Outcome::Error(e);
                }
                Outcome::Text(self.expand_format_at(&msg, target.as_ref(), cid))
            }
            Cmd::BindKey { table, key, repeat, cmd } => match Key::parse(&key) {
                Some(k) => {
                    let b = Binding { cmd: *cmd, repeat };
                    self.binds_mut(table).insert(k, b);
                    Outcome::Ok
                }
                // tmux's mouse "keys" (MouseDragEnd1Pane, WheelUpPane, ...):
                // keepane's mouse handling is fixed, so a config line for one
                // is taken and does nothing, instead of an error at load.
                None if is_mouse_key(&key) => Outcome::Ok,
                None => Outcome::Error(format!("unknown key: {key}")),
            },
            Cmd::UnbindKey { table, key } => match Key::parse(&key) {
                Some(k) => {
                    self.binds_mut(table).remove(&k);
                    Outcome::Ok
                }
                None if is_mouse_key(&key) => Outcome::Ok,
                None => Outcome::Error(format!("unknown key: {key}")),
            },
            Cmd::SetOption { name, value, append, target } if append && name != "synchronize-panes" => {
                // `set -a`: add to what is there (tmux appends the text).
                let current = self.opts.get(&name).unwrap_or_default();
                self.exec(Cmd::SetOption { name, value: format!("{current}{value}"), append: false, target }, cid)
            }
            Cmd::SetOption { name, value, target, .. } if name == "synchronize-panes" => {
                // Window-scoped in tmux: the `-t` window, else the current one.
                let on = match value.trim().to_ascii_lowercase().as_str() {
                    "on" | "true" | "yes" | "1" => Some(true),
                    "off" | "false" | "no" | "0" => Some(false),
                    "" => None, // toggle
                    v => return Outcome::Error(format!("bad boolean '{v}'")),
                };
                let (sid, widx, _) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let w = &mut self.session_mut(sid).unwrap().windows[widx];
                w.synchronized = on.unwrap_or(!w.synchronized);
                Outcome::Ok
            }
            Cmd::SetOption { name, value, .. } => {
                let r = self.opts.set(&name, &value);
                if r.is_ok() {
                    // The event log follows its options at once, not at the
                    // next tick: `event-log off` means nothing more is written.
                    self.sync_event_log();
                    // Status line toggles change the window area.
                    let ids: Vec<SessionId> = self.sessions.iter().map(|s| s.id).collect();
                    for sid in ids {
                        self.relayout_session(sid);
                    }
                    let mouse = self.opts.mouse;
                    for c in self.clients.values_mut() {
                        c.last_grid = None;
                        if name == "mouse" && c.session.is_some() {
                            c.send(ServerMsg::SetMouse(mouse));
                        }
                    }
                    // Flipped from a key (`prefix C-t`): say which way, since
                    // a pane with no commands to stamp shows no change.
                    if value.is_empty()
                        && let Some(cid) = cid
                        && crate::config::resolve_name(&name).as_deref() == Ok("pane-timestamps")
                    {
                        let on = if self.opts.pane_timestamps { "on" } else { "off" };
                        self.message(cid, &format!("pane-timestamps {on}"));
                    }
                }
                r.into()
            }
            Cmd::SwitchClient { next, prev, last, target } => {
                let Some(cid) = cid else { return Outcome::Error("switch-client: no client".into()) };
                let cur = self.clients.get(&cid).and_then(|c| c.session);
                let Some(cur) = cur else { return Outcome::Error("client not attached".into()) };
                let pos = self.sessions.iter().position(|s| s.id == cur).unwrap_or(0);
                let n = self.sessions.len();
                let sid = if next {
                    self.sessions[(pos + 1) % n].id
                } else if prev {
                    self.sessions[(pos + n - 1) % n].id
                } else if last {
                    let prev_session =
                        self.clients.get(&cid).and_then(|c| c.last_session).filter(|s| self.session(*s).is_some());
                    match prev_session {
                        Some(s) => s,
                        None => return Outcome::Error("no last session".into()),
                    }
                } else {
                    match self.resolve_session(target.as_ref(), None) {
                        Ok(s) => s,
                        Err(e) => return Outcome::Error(e),
                    }
                };
                if sid == cur {
                    return Outcome::Ok;
                }
                let c = self.clients.get_mut(&cid).unwrap();
                c.last_session = Some(cur);
                c.session = Some(sid);
                c.last_grid = None;
                self.fit_session(sid, Some(cid));
                self.fit_session(cur, None); // one client fewer there
                if let Some(s) = self.session_mut(sid) {
                    s.last_used = Instant::now();
                    // Another session: the frame closes in on where the keys go.
                    if let Some(w) = s.window_mut() {
                        w.anim = Some(Anim::Enter { start: Instant::now() });
                    }
                }
                Outcome::Ok
            }
            Cmd::ChooseBuffer => {
                let Some(cid) = cid else { return Outcome::Error("choose-buffer: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("choose-buffer: client not attached".into());
                }
                if self.buffers.is_empty() {
                    return Outcome::Error("no buffers".into());
                }
                let (items, lines) = self.chooser_lines(&ChooserKind::Buffers, &[]).unwrap_or_default();
                let c = self.clients.get_mut(&cid).unwrap();
                c.prompt = None;
                c.overlay = None;
                c.chooser = Some(Chooser::new(ChooserKind::Buffers, items, lines, 0));
                Outcome::Ok
            }
            Cmd::FocusPane { pane } => {
                let Some((sid, widx)) = self.window_of_pane(pane) else {
                    return Outcome::Error(format!("focus-pane: no pane %{pane}"));
                };
                let wid = self.session(sid).and_then(|s| s.windows.get(widx)).map(|w| w.id).unwrap_or(0);
                // Every terminal that is attached anywhere: the one the
                // notification's reader is looking at is among them.
                let clients: Vec<ClientId> =
                    self.clients.values().filter(|c| c.interactive && c.session.is_some()).map(|c| c.id).collect();
                if clients.is_empty() {
                    return Outcome::Error("focus-pane: no client attached (`keepane attach` first)".into());
                }
                for c in &clients {
                    self.chooser_go_pane(*c, sid, wid, pane);
                    if let Some(client) = self.clients.get_mut(c) {
                        client.send(ServerMsg::Raise);
                    }
                }
                Outcome::Text(format!("{} client(s) switched to pane %{pane}", clients.len()))
            }
            Cmd::ChooseJobs => {
                let Some(cid) = cid else { return Outcome::Error("choose-jobs: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("choose-jobs: client not attached".into());
                }
                let (items, lines) = self.chooser_lines(&ChooserKind::Jobs, &[]).unwrap_or_default();
                // Start on the client's own pane.
                let sel = self
                    .resolve(None, Some(cid))
                    .ok()
                    .and_then(|(_, _, pid)| {
                        items.iter().position(|i| matches!(i, ChooserItem::Job(_, _, p) if *p == pid))
                    })
                    .unwrap_or(1);
                let c = self.clients.get_mut(&cid).unwrap();
                c.prompt = None;
                c.overlay = None;
                c.chooser = Some(Chooser::new(ChooserKind::Jobs, items, lines, sel));
                Outcome::Ok
            }
            Cmd::UndoKill => self.undo_kill(),
            Cmd::Dashboard => {
                let Some(cid) = cid else { return Outcome::Error("dashboard: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("dashboard: client not attached (run `keepane dashboard` instead)".into());
                }
                let Some(exe) = pane::helper_exe() else {
                    return Outcome::Error("dashboard: no keepane.exe to run it with".into());
                };
                let cmd = Cmd::DisplayPopup {
                    close: false,
                    close_on_exit: true,
                    width: Some("90%".into()),
                    height: Some("90%".into()),
                    x: None,
                    y: None,
                    cwd: None,
                    argv: vec![
                        exe.to_string_lossy().into_owned(),
                        "-L".into(),
                        self.socket.clone(),
                        "dashboard".into(),
                        "--popup".into(),
                    ],
                };
                self.exec(cmd, Some(cid))
            }
            Cmd::ChooseHistory => {
                let Some(cid) = cid else { return Outcome::Error("choose-history: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("choose-history: client not attached".into());
                }
                let kept = crate::histlog::scan(&self.history_dir());
                if kept.is_empty() {
                    let why = if self.opts.log_history { "" } else { " (log-history is off)" };
                    return Outcome::Error(format!("no history yet{why}"));
                }
                // Open at the client's own pane, on its newest day.
                let here = self.resolve(None, Some(cid)).ok().and_then(|(sid, widx, pid)| {
                    let s = self.session(sid)?;
                    let pidx = s.windows.get(widx)?.layout.panes().iter().position(|p| *p == pid)?;
                    let key = format!("{}.{}", widx + self.opts.base_index, pidx + self.opts.pane_base_index);
                    let session = crate::histlog::safe_name(&s.name);
                    kept.iter().position(|k| k.session == session && k.key == key)
                });
                let open: HashSet<usize> = here.into_iter().collect();
                let (items, lines) = history_lines(&kept, &open);
                let sel = here.and_then(|i| items.iter().position(|x| *x == ChooserItem::Log(i, Some(0)))).unwrap_or(0);
                let c = self.clients.get_mut(&cid).unwrap();
                c.prompt = None;
                c.overlay = None;
                c.chooser = Some(Chooser::new(ChooserKind::History { kept, open }, items, lines, sel));
                Outcome::Ok
            }
            Cmd::ChooseClient => {
                let Some(cid) = cid else { return Outcome::Error("choose-client: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("choose-client: client not attached".into());
                }
                let (items, lines) = self.chooser_lines(&ChooserKind::Clients, &[]).unwrap_or_default();
                let c = self.clients.get_mut(&cid).unwrap();
                c.prompt = None;
                c.overlay = None;
                c.chooser = Some(Chooser::new(ChooserKind::Clients, items, lines, 0));
                Outcome::Ok
            }
            Cmd::DisplayMenu { title, items: entries } => {
                let Some(cid) = cid else { return Outcome::Error("display-menu: no client".into()) };
                if self.clients.get(&cid).and_then(|c| c.session).is_none() {
                    return Outcome::Error("display-menu: client not attached".into());
                }
                let (mut items, mut lines) = (Vec::new(), Vec::new());
                if let Some(t) = &title {
                    items.push(ChooserItem::Separator);
                    lines.push(self.expand_format(t, cid));
                }
                for (i, e) in entries.iter().enumerate() {
                    match (&e.key, &e.cmd) {
                        (Some(k), Some(_)) => {
                            items.push(ChooserItem::Menu(i));
                            lines.push(format!("({k}) {}", self.expand_format(&e.name, cid)));
                        }
                        // No key or no command: a label nobody can pick.
                        _ => {
                            items.push(ChooserItem::Separator);
                            lines.push(if e.name.is_empty() { "-".repeat(20) } else { e.name.clone() });
                        }
                    }
                }
                let c = self.clients.get_mut(&cid).unwrap();
                c.prompt = None;
                c.overlay = None;
                let mut ch = Chooser::new(ChooserKind::Menu(entries), items, lines, 0);
                ch.settle(1);
                c.chooser = Some(ch);
                Outcome::Ok
            }
            Cmd::DisplayPopup { close, close_on_exit, width, height, x, y, cwd, argv } => {
                let Some(cid) = cid else { return Outcome::Error("display-popup: no client".into()) };
                let Some((cols, rows)) = self.clients.get(&cid).map(|c| (c.cols, c.rows)) else {
                    return Outcome::Error("display-popup: no client".into());
                };
                if close {
                    // From an attached client, close its own popup; from a
                    // script or a pane, close the popup of whoever is looking
                    // at that session, which is the one the user can see.
                    let mut targets = vec![cid];
                    if self.clients.get(&cid).is_none_or(|c| c.popup.is_none())
                        && let Ok(sid) = self.resolve_session(None, Some(cid))
                    {
                        targets = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
                    }
                    for id in targets {
                        if let Some(c) = self.clients.get_mut(&id) {
                            c.popup = None;
                            c.last_grid = None;
                        }
                    }
                    return Outcome::Ok;
                }
                let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else {
                    return Outcome::Error("display-popup: client not attached".into());
                };
                let area = self.window_area(cols, rows);
                let rect = popup_rect(area, width.as_deref(), height.as_deref(), x.as_deref(), y.as_deref());
                if rect.w < 3 || rect.h < 3 {
                    return Outcome::Error("display-popup: no room for a popup".into());
                }
                let cwd = self.pane_cwd(cwd.as_deref(), sid, Some(cid));
                let pane = match self.spawn_pane(&argv, cwd.as_deref(), rect.w - 2, rect.h - 2, ShellHistory::Shared) {
                    Ok(p) => p,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(c) = self.clients.get_mut(&cid) else { return Outcome::Error("no client".into()) };
                // One thing at a time: a popup replaces the picker or an
                // overlay, whose keys it would otherwise steal.
                c.prompt = None;
                c.overlay = None;
                c.chooser = None;
                c.last_grid = None;
                c.popup = Some(Popup { pane, rect, width, height, x, y, close_on_exit, finished: false });
                Outcome::Ok
            }
            Cmd::PipePane { target, command, toggle, input, output } => {
                let pid = match self.resolve(target.as_ref(), cid) {
                    Ok((_, _, p)) => p,
                    Err(e) => return Outcome::Error(e),
                };
                let running = self.find_pane_mut(pid).is_some_and(|p| p.pipe.is_some());
                let command = match command {
                    // No command at all, or `-o` while one is running: stop.
                    None => None,
                    Some(_) if running && toggle => None,
                    Some(c) => Some(match cid {
                        Some(cid) => self.expand_format(&c, cid),
                        None => c,
                    }),
                };
                let env = self.pane_env(pid);
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                match command {
                    None => {
                        p.pipe = None;
                        Outcome::Ok
                    }
                    Some(c) => match p.pipe_to(&c, &env, input, output) {
                        Ok(()) => Outcome::Ok,
                        Err(e) => Outcome::Error(format!("{e:#}")),
                    },
                }
            }
            Cmd::WebStart(o) => self.web_start(o),
            Cmd::WebStatus => Outcome::Text(self.web_status()),
            Cmd::WebStop => self.web_stop(cid),
            Cmd::WebFit { target, size } => self.web_fit(cid, target.as_ref(), size),
            c @ (Cmd::LinkId | Cmd::LinkTrust { .. } | Cmd::LinkList | Cmd::LinkAllow { .. } | Cmd::LinkRekey) => {
                self.exec_link_local(c, cid)
            }
            c @ (Cmd::LinkAdd { .. }
            | Cmd::LinkPanes { .. }
            | Cmd::LinkRemove { .. }
            | Cmd::LinkInfo { .. }
            | Cmd::LinkCapture { .. }
            | Cmd::LinkPane(_)
            | Cmd::LinkKill { .. }) => self.exec_link_out(c, cid),
            Cmd::LinkInbound { peer, request } => self.link_inbound(cid, &peer, &request),
            Cmd::WaitFor { channel, lock, unlock, signal } => {
                let Some(cid) = cid else { return Outcome::Error("wait-for: no client".into()) };
                let mut wake: Vec<ClientId> = Vec::new();
                let w = self.waits.entry(channel.clone()).or_default();
                let out = if signal {
                    if w.waiters.is_empty() {
                        w.woken = true;
                    } else {
                        wake.append(&mut w.waiters);
                    }
                    Outcome::Ok
                } else if lock {
                    if w.locked {
                        w.lockers.push_back(cid);
                        Outcome::Pending
                    } else {
                        w.locked = true;
                        Outcome::Ok
                    }
                } else if unlock {
                    if !w.locked {
                        Outcome::Error(format!("wait-for: channel {channel} is not locked"))
                    } else {
                        // The next in line takes the lock straight over.
                        match w.lockers.pop_front() {
                            Some(next) => wake.push(next),
                            None => w.locked = false,
                        }
                        Outcome::Ok
                    }
                } else if w.woken {
                    w.woken = false;
                    Outcome::Ok
                } else {
                    w.waiters.push(cid);
                    Outcome::Pending
                };
                if self.waits.get(&channel).is_some_and(WaitChannel::idle) {
                    self.waits.remove(&channel);
                }
                for id in wake {
                    self.reply(id, Outcome::Ok);
                }
                out
            }
            Cmd::ChooseTree { sessions, windows } => {
                let Some(cid) = cid else { return Outcome::Error("choose-tree: no client".into()) };
                let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else {
                    return Outcome::Error("choose-tree: client not attached".into());
                };
                let expand = windows || !sessions;
                let style = TreeStyle::parse(&self.opts.choose_tree_style);
                // The chart asks the paired machines first, so that they are
                // in it from the first frame (their panes as they come).
                if style == TreeStyle::Chart && expand {
                    self.ask_remote_trees(true);
                }
                let kind = ChooserKind::Tree { expand, style };
                let (items, lines) = self.chooser_lines(&kind, &[]).unwrap_or_default();
                // Start on the current pane (or the session, sessions only).
                let cur = self.session(sid).and_then(|s| s.window()).filter(|_| expand).map(|w| (w.id, w.active));
                let at = match cur {
                    Some((w, p)) => ChooserItem::Job(sid, w, p),
                    None => ChooserItem::Tree(sid, None),
                };
                let sel = items.iter().position(|i| *i == at).unwrap_or(0);
                let active = items.iter().map(|item| self.current_in_tree(item)).collect();
                let c = self.clients.get_mut(&cid).unwrap();
                // One modal at a time: a picker opened from the `:` prompt
                // replaces it, or its keys would go to an invisible prompt.
                c.prompt = None;
                c.overlay = None;
                let mut ch = Chooser::new(kind, items, lines, sel);
                ch.active = active;
                c.chooser = Some(ch);
                Outcome::Ok
            }
            Cmd::ListKeys => {
                let mut lines: Vec<String> = self
                    .prefix_binds
                    .iter()
                    .map(|(k, b)| format!("bind-key {}-T prefix {k:<10} {}", if b.repeat { "-r " } else { "" }, b.cmd))
                    .collect();
                lines.extend(self.root_binds.iter().map(|(k, b)| {
                    format!("bind-key {}-T root   {k:<10} {}", if b.repeat { "-r " } else { "" }, b.cmd)
                }));
                lines
                    .extend(self.copy_binds.iter().map(|(k, b)| format!("bind-key -T copy-mode-vi {k:<10} {}", b.cmd)));
                lines.sort();
                Outcome::Text(lines.join("\n"))
            }
            Cmd::SetCwd { target, dir } => {
                // No target: the pane the client runs in (KEEPANE_PANE), else the active one.
                let pid = match (&target, cid.and_then(|c| self.clients.get(&c)).and_then(|c| c.pane_env)) {
                    (None, Some(p)) if self.find_pane_mut(p).is_some() => p,
                    _ => match self.resolve(target.as_ref(), cid) {
                        Ok((_, _, p)) => p,
                        Err(e) => return Outcome::Error(e),
                    },
                };
                let client_cwd = self.client_cwd(cid);
                let dir = match dir.clone().or_else(|| client_cwd.clone()) {
                    Some(d) => expand_home(&d),
                    None => return Outcome::Error("set-cwd: no directory given and the client has none".into()),
                };
                let dir = pane::path_from_announced(&dir).unwrap_or(dir);
                // A relative directory is relative to where the client runs, not the server.
                let dir = match (std::path::Path::new(&dir).is_relative(), &client_cwd) {
                    (true, Some(base)) => std::path::Path::new(base).join(&dir).to_string_lossy().into_owned(),
                    _ => dir,
                };
                if !std::path::Path::new(&dir).is_dir() {
                    return Outcome::Error(format!("set-cwd: not a directory: {dir}"));
                }
                match self.find_pane_mut(pid) {
                    Some(p) => {
                        // Said by the user: final, like a shell's own report.
                        p.cwd = Some(dir);
                        p.announced = true;
                        Outcome::Ok
                    }
                    None => Outcome::Error("no such pane".into()),
                }
            }
            Cmd::CapturePane { target, history, escapes, join } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                let from = p.scrollback_len().saturating_sub(history);
                Outcome::Text(p.lines_from(from, escapes, join).join("\n"))
            }
            Cmd::ClearHistory => {
                let (_, _, pid) = match self.resolve(None, cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let hist = self.opts.history_limit;
                if let Some(p) = self.find_pane_mut(pid) {
                    // The scrollback the copy cursor refers to is about to vanish.
                    exit_copy_mode(p);
                    // vt100 has no explicit clear; re-create the parser at the same size.
                    let (rows, cols) = p.screen().size();
                    let mut fresh = vt100::Parser::new_with_callbacks(rows, cols, hist, pane::Callbacks::default());
                    fresh.process(&p.screen().state_formatted());
                    // What scrolled off so far is logged by the old parser;
                    // the new one counts its lines from where it starts.
                    if p.log_to.is_some() {
                        p.log_take(false);
                        p.flush_log();
                    }
                    p.parser = fresh;
                    p.logged = p.screen().scrolled_total();
                    // Their lines were counted by the parser just replaced.
                    p.marks.clear();
                }
                Outcome::Ok
            }
            Cmd::ListDone { after, json } => Outcome::Text(self.list_done(after, json)),
            Cmd::ListMarks { target, json } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                if json {
                    return Outcome::Text(marks_json(p).to_string());
                }
                // Nothing to list prints nothing, not an empty line.
                match list_marks(p) {
                    lines if lines.is_empty() => Outcome::Ok,
                    lines => Outcome::Text(lines.join("\n")),
                }
            }
            Cmd::ListAgents { json } => self.list_agents(json),
            Cmd::StatusLine { target, json } => match self.resolve_session(target.as_ref(), cid) {
                Ok(sid) => self.status_line_out(sid, json),
                Err(e) => Outcome::Error(e),
            },
            Cmd::CopyOutput { target, print } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let Some(p) = self.find_pane_mut(pid) else { return Outcome::Error("no such pane".into()) };
                let (command, text, cut) = match p.last_output(OUTPUT_MAX) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                if print {
                    return Outcome::Text(text);
                }
                if text.is_empty() {
                    return Outcome::Error(format!("`{command}` printed nothing"));
                }
                let lines = text.lines().count();
                self.set_buffer(None, &text, false);
                let note = match crate::clipboard::set_text(&text) {
                    Err(e) => format!(" (clipboard: {e})"),
                    Ok(()) if cut => format!(" (cut at {} MB)", OUTPUT_MAX >> 20),
                    Ok(()) => String::new(),
                };
                let what = format!(
                    "copied {} `{}` printed{note}",
                    crate::format::count(lines, "line"),
                    truncate(&command, 40)
                );
                if let Some(cid) = cid {
                    self.message(cid, &what);
                }
                Outcome::Ok
            }
            Cmd::ShellHistory { target, commands, messages, last } => {
                let (_, _, pid) = match self.resolve(target.as_ref(), cid) {
                    Ok(r) => r,
                    Err(e) => return Outcome::Error(e),
                };
                let lines = if messages { self.messages_to(pid) } else { self.shell_history_of(pid, commands) };
                let lines = match lines {
                    Ok(l) => l,
                    Err(e) => return Outcome::Error(e),
                };
                let skip = last.map_or(0, |n| lines.len().saturating_sub(n));
                match lines.into_iter().skip(skip).collect::<Vec<_>>() {
                    l if l.is_empty() => Outcome::Ok,
                    l => Outcome::Text(l.join("\n")),
                }
            }
            Cmd::Hints => {
                let Some(cid) = cid.filter(|c| self.clients.get(c).is_some_and(|c| c.session.is_some())) else {
                    return Outcome::Error("hints: client not attached".into());
                };
                // A list or a popup covers the panes: labels under it could
                // not be seen, yet would take the keys.
                if self.clients.get(&cid).is_some_and(|c| c.chooser.is_some() || c.popup.is_some()) {
                    return Outcome::Error("hints: close the list or popup first".into());
                }
                let found = self.hints_on_screen(cid);
                if found.is_empty() {
                    return Outcome::Error("nothing on screen to pick (paths, addresses, hashes)".into());
                }
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.hints = Some(HintMode { found, typed: String::new(), open: false });
                }
                Outcome::Ok
            }
            Cmd::SourceFile { path } => self.source_file(&path).map(|_| ()).into(),
            // Window-scoped, so it is not in the global table: answer it from
            // the window the client is looking at.
            Cmd::ShowOptions { ref name, value_only, quiet: _ } if name.as_deref() == Some("synchronize-panes") => {
                let on = match self.resolve(None, cid) {
                    Ok((sid, widx, _)) => {
                        self.session(sid).and_then(|s| s.windows.get(widx)).is_some_and(|w| w.synchronized)
                    }
                    Err(e) => return Outcome::Error(e),
                };
                let v = if on { "on" } else { "off" };
                Outcome::Text(if value_only { v.to_string() } else { format!("synchronize-panes {v}") })
            }
            Cmd::ShowOptions { name, value_only, quiet } => match name {
                Some(n) => match self.opts.get(&n) {
                    Some(v) => Outcome::Text(if value_only { v } else { format!("{n} {}", crate::command::quote(&v)) }),
                    None if quiet => Outcome::Text(String::new()),
                    None => Outcome::Error(format!("unknown option: {n}")),
                },
                None => {
                    let mut lines: Vec<String> = crate::config::SHOWABLE
                        .iter()
                        .filter_map(|n| self.opts.get(n).map(|v| format!("{n} {}", crate::command::quote(&v))))
                        .collect();
                    // Window-scoped, so it is not in the table, but leaving it
                    // out of the listing hides an option that can be set.
                    if let Ok((sid, widx, _)) = self.resolve(None, cid)
                        && let Some(w) = self.session(sid).and_then(|s| s.windows.get(widx))
                    {
                        lines.push(format!("synchronize-panes {}", if w.synchronized { "on" } else { "off" }));
                    }
                    for (k, v) in &self.opts.user {
                        lines.push(format!("{k} {}", crate::command::quote(v)));
                    }
                    Outcome::Text(lines.join("\n"))
                }
            },
            Cmd::RunShell { command, background, target } => {
                let pane = self.resolve(target.as_ref(), cid).ok().map(|(_, _, p)| p);
                // From a config file (no client) or with -b the output goes to the log.
                let reply_to = if background { None } else { cid };
                self.spawn_shell(command, pane, reply_to, false);
                if reply_to.is_some() { Outcome::Pending } else { Outcome::Ok }
            }
            Cmd::SetHook { hook, cmd } => {
                if !HOOKS.contains(&hook.as_str()) {
                    return Outcome::Error(format!("unknown hook: {hook} (known: {})", HOOKS.join(", ")));
                }
                match cmd {
                    Some(c) => {
                        self.hooks.insert(hook, *c);
                    }
                    None => {
                        self.hooks.remove(&hook);
                    }
                }
                Outcome::Ok
            }
            Cmd::ShowHooks => {
                let mut lines: Vec<String> =
                    self.hooks.iter().map(|(h, c)| format!("{h} {}", crate::command::quote(&c.to_string()))).collect();
                lines.sort();
                Outcome::Text(lines.join("\n"))
            }
            Cmd::LoadPlugin { path } => self.load_plugin(&path).into(),
            Cmd::ListPlugins => {
                if self.plugins.is_empty() {
                    Outcome::Text("no plugins loaded".into())
                } else {
                    Outcome::Text(self.plugins.join("\n"))
                }
            }
        }
    }

    /// Expand a format string for `display-message` and the prompts, with the
    /// same engine the status line uses, so `#{...}`, `#{?...}` and `#(...)`
    /// mean the same thing everywhere.
    fn expand_format(&mut self, s: &str, cid: ClientId) -> String {
        self.expand_format_at(s, None, Some(cid))
    }

    /// `s` expanded for `target` (the client's current pane when None).
    fn expand_format_at(&mut self, s: &str, target: Option<&Target>, cid: Option<ClientId>) -> String {
        let Ok((sid, widx, pid)) = self.resolve(target, cid) else { return s.to_string() };
        let ctx = self.context(sid, widx, Some(pid), cid);
        self.expand_with_shells(s, &ctx, pid)
    }

    /// A `%if` condition of the config file: a format, true when it expands
    /// to something other than nothing or "0". The server-wide variables
    /// (host, socket, version) are what there is; a config is read before
    /// there are sessions. `#(command)` pieces are not run for it.
    fn config_condition(&self, cond: &str) -> bool {
        let ctx = crate::format::Context {
            host: crate::sysinfo::hostname(),
            socket: self.socket.clone(),
            ..Default::default()
        };
        let mut cache = crate::format::ShellCache::default();
        let text: String =
            crate::format::expand(cond, &ctx, &mut cache, render::Style::default(), chrono::Local::now())
                .into_iter()
                .map(|s| s.text)
                .collect();
        let text = text.trim();
        !text.is_empty() && text != "0"
    }

    /// `s` expanded for `ctx` as text, for a one-shot answer
    /// (`display-message -p`, `jobs -F`): a `#(command)` the status line has
    /// not run yet is run here, with a short leash, since tmux waits for it
    /// too and "empty until the next status-interval" is no answer.
    fn expand_with_shells(&mut self, s: &str, ctx: &crate::format::Context, pid: PaneId) -> String {
        let now = chrono::Local::now();
        let base = render::Style::default();
        let text = |segs: Vec<crate::format::Segment>| segs.into_iter().map(|seg| seg.text).collect::<String>();
        let first = crate::format::expand(s, ctx, &mut self.shell_cache, base, now);
        let missing: Vec<String> = std::mem::take(&mut self.shell_cache.wanted)
            .into_iter()
            .filter(|c| !self.shell_cache.results.contains_key(c))
            .collect();
        if missing.is_empty() {
            return text(first);
        }
        let mut env = self.pane_env(pid);
        env.retain(|(k, _)| k != "KEEPANE_PANE");
        for cmd in missing {
            let (output, _) = run_shell_blocking(&cmd, &env, Some(ONE_SHOT_SHELL_TIMEOUT));
            let line = output.lines().next().unwrap_or("").trim_end().to_string();
            self.shell_cache.results.insert(cmd, line);
        }
        text(crate::format::expand(s, ctx, &mut self.shell_cache, base, now))
    }

    /// Everything a format can ask about a session, one of its windows, a
    /// pane of that window (the active one when `pid` is None) and the
    /// client it is drawn for. The one place every `#{...}` is answered from.
    fn context(
        &self,
        sid: SessionId,
        widx: usize,
        pid: Option<PaneId>,
        cid: Option<ClientId>,
    ) -> crate::format::Context {
        // The machine's readings come from a once-a-second cache.
        let sys = crate::sysinfo::system();
        let mut ctx = crate::format::Context {
            host: crate::sysinfo::hostname(),
            socket: self.socket.clone(),
            cpu_percentage: sys.cpu_percentage,
            ram_percentage: sys.ram_percentage,
            ram_used: sys.ram_used,
            battery_percentage: sys.battery_percentage,
            battery_charging: sys.battery_charging,
            uptime: sys.uptime,
            keepane_update: self.newer.newer.clone().unwrap_or_default(),
            local_ip: crate::sysinfo::local_ip(),
            public_ip: self.public_ip.value.clone(),
            web_url: self.web.as_ref().map(|w| w.url().to_string()).unwrap_or_default(),
            web_clients: self.web.as_ref().map(|w| w.clients().to_string()).unwrap_or_default(),
            ..Default::default()
        };
        let unix = unix_seconds;
        let Some(sess) = self.session(sid) else { return ctx };
        ctx.session = sess.name.clone();
        ctx.session_id = sess.id;
        ctx.session_windows = sess.windows.len();
        ctx.session_attached = self.clients.values().filter(|c| c.session == Some(sid)).count();
        ctx.session_created = sess.created_at;
        ctx.session_activity = sess.windows.iter().map(|w| unix(w.last_output)).max().unwrap_or(sess.created_at);
        ctx.session_last_attached = unix(sess.last_used);
        if let Some(c) = cid.and_then(|c| self.clients.get(&c)) {
            ctx.client_width = c.cols;
            ctx.client_height = c.rows;
            ctx.client_name = format!("client-{}", c.id);
            ctx.client_session = c.session.and_then(|s| self.session(s)).map(|s| s.name.clone()).unwrap_or_default();
            ctx.client_created = unix(c.created);
            ctx.client_activity = unix(c.last_activity);
            ctx.client_prefix = c.prefix;
        }
        let Some(w) = sess.windows.get(widx) else { return ctx };
        ctx.window = w.name.clone();
        ctx.window_id = w.id;
        ctx.window_index = widx + self.opts.base_index;
        ctx.window_panes = w.panes.len();
        ctx.window_active = widx == sess.cur;
        ctx.window_last = Some(w.id) == sess.last;
        ctx.window_zoomed = w.zoomed;
        ctx.window_width = sess.cols;
        ctx.window_height = sess.rows;
        ctx.window_bell = w.alert_bell;
        ctx.window_activity = w.alert_activity;
        ctx.window_silence = w.alert_silence;
        ctx.flags = w.flags(widx == sess.cur, Some(w.id) == sess.last);
        ctx.window_activity_time = unix(w.last_output);
        ctx.window_start = widx == 0;
        ctx.window_end = widx + 1 == sess.windows.len();
        // tmux's layout string, which `select-layout` takes back.
        ctx.window_layout = layout::layout_string(&w.layout, &w.rects);
        let pid = pid.unwrap_or(w.active);
        let order = w.layout.panes();
        ctx.pane_index = order.iter().position(|p| *p == pid).unwrap_or(0) + self.opts.pane_base_index;
        ctx.pane_id = pid;
        ctx.pane_active = pid == w.active;
        ctx.pane_synchronized = w.synchronized;
        if let Some(p) = w.pane(pid) {
            ctx.pane_title = p.display_title().to_string();
            ctx.pane_command = p.command.clone();
            ctx.pane_start_command = p.argv.join(" ");
            ctx.pane_path = p.current_path().unwrap_or_default();
            ctx.pane_path_short = crate::sysinfo::short_path(&ctx.pane_path);
            ctx.pane_last_line = p.last_line();
            ctx.git_branch =
                if ctx.pane_path.is_empty() { String::new() } else { crate::sysinfo::git_branch(&ctx.pane_path) };
            ctx.pane_pid_command = p.pid.map(crate::sysinfo::program_of).unwrap_or_default();
            ctx.pane_width = p.cols;
            ctx.pane_height = p.rows;
            ctx.pane_dead = p.exit_code.is_some();
            ctx.pane_dead_status = p.exit_code;
            ctx.alternate_on = p.screen().alternate_screen();
            ctx.mouse_any_flag = p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None;
            ctx.pane_in_mode = p.copy.is_some();
            ctx.pane_pid = p.pid;
            ctx.pane_start_time = unix(p.spawned_at);
            ctx.pane_activity = unix(p.last_output);
            ctx.pane_output_count = p.output_count;
            ctx.pane_dead_time = p.died_at.map(unix).unwrap_or(0);
            ctx.pane_name = p.actor.name.clone().unwrap_or_default();
            ctx.pane_address = format!("${}:@{}.%{pid}", sess.id, w.id);
            ctx.pane_work_mode = p.actor.mode.as_str().to_string();
            ctx.pane_idle = p.actor.idle();
            ctx.pane_unheard = p.actor.unheard();
            ctx.pane_inbox = p.actor.inbox.len();
            ctx.pane_status = p.actor.status.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
            ctx.pane_message = p.actor.current.as_ref().map(|m| m.id.to_string()).unwrap_or_default();
            ctx.pane_last = w.last_pane == Some(pid);
            ctx.pane_mode = if p.copy.is_some() { "copy-mode".into() } else { String::new() };
            let (cy, cx) = p.parser.screen().cursor_position();
            ctx.cursor_x = cx;
            ctx.cursor_y = cy;
            ctx.history_size = p.parser.screen().scrollback_rows();
            ctx.history_limit = self.opts.history_limit;
            if let Some(a) = self.agents.stats.get(&pid) {
                a.fill(&mut ctx);
            }
        }
        if let Some(r) = w.rect_of(pid) {
            let area = self.window_area(sess.cols, sess.rows);
            ctx.pane_top = r.y.saturating_sub(area.y);
            ctx.pane_left = r.x.saturating_sub(area.x);
            ctx.pane_bottom = (r.y + r.h).saturating_sub(area.y + 1);
            ctx.pane_right = (r.x + r.w).saturating_sub(area.x + 1);
            ctx.pane_at_top = r.y <= area.y;
            ctx.pane_at_left = r.x <= area.x;
            ctx.pane_at_bottom = r.y + r.h >= area.y + area.h;
            ctx.pane_at_right = r.x + r.w >= area.x + area.w;
        }
        ctx
    }

    // ------------------------------------------------------------------ keys

    /// The picks for `hints` in every pane of the window on screen: what is
    /// found in the rows shown (the copy-mode view for a pane in copy mode),
    /// labelled from the bottom up, so the newest output gets the easiest
    /// keys; the same text has the same label.
    fn hints_on_screen(&mut self, cid: ClientId) -> Vec<(PaneId, hints::Found, String)> {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return Vec::new() };
        let Some(rects) = self.session(sid).and_then(|s| s.window()).map(|w| w.rects.clone()) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for (pid, rect) in rects {
            let Some(p) = self.find_pane_mut(pid) else { continue };
            let (cols, offset) = (p.cols, p.copy.as_ref().map_or(0, |c| c.offset));
            let dir = p.current_path();
            let s = p.parser.screen_mut();
            let keep = s.scrollback();
            s.set_scrollback(offset);
            let rows: Vec<String> = s.rows(0, cols).take(usize::from(rect.h)).collect();
            s.set_scrollback(keep);
            let exists = |f: &str| hints::resolve(f, dir.as_deref()).exists();
            for (i, row) in rows.iter().enumerate() {
                found.extend(hints::find(i as u16, row, &exists).into_iter().map(|f| (pid, f)));
            }
        }
        let mut texts: Vec<&str> = Vec::new();
        for (_, f) in found.iter().rev() {
            if !texts.contains(&f.text.as_str()) {
                texts.push(&f.text);
            }
        }
        let labels = hints::labels(texts.len());
        let label_of = |t: &str| texts.iter().position(|x| *x == t).and_then(|i| labels.get(i)).cloned();
        let labelled: Vec<Option<String>> = found.iter().map(|(_, f)| label_of(&f.text)).collect();
        found.into_iter().zip(labelled).filter_map(|((pid, f), l)| Some((pid, f, l?))).collect()
    }

    /// A key while `hints` is up: a label's letters pick its thing (a
    /// capital among them opens it), Backspace takes one back, a letter no
    /// label goes on with is ignored, anything else puts the labels away.
    fn hints_key(&mut self, cid: ClientId, k: Key) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let Some(hm) = c.hints.as_mut() else { return };
        match k.code {
            KeyCode::Char(ch) if ch.is_ascii_alphabetic() && !k.ctrl && !k.alt => {
                let typed = format!("{}{}", hm.typed, ch.to_ascii_lowercase());
                if !hm.found.iter().any(|(_, _, l)| l.starts_with(&typed)) {
                    return;
                }
                hm.typed = typed;
                hm.open |= ch.is_ascii_uppercase();
                let Some((pid, f, _)) = hm.found.iter().find(|(_, _, l)| *l == hm.typed).cloned() else { return };
                let open = hm.open;
                c.hints = None;
                self.hint_pick(cid, pid, f, open);
            }
            KeyCode::BSpace => {
                hm.typed.pop();
            }
            _ => c.hints = None,
        }
    }

    /// What a `hints` label does: copy its thing, or open it (an address in
    /// the browser, a path in the editor at its line). A hash has nothing to
    /// open and is copied.
    fn hint_pick(&mut self, cid: ClientId, pid: PaneId, f: hints::Found, open: bool) {
        match (&f.kind, open) {
            (hints::Kind::Url, true) => match crate::platform::open::open(&f.text) {
                Ok(()) => self.message(cid, &format!("opening {}", f.text)),
                Err(e) => self.message(cid, &e),
            },
            (hints::Kind::Path { line, col }, true) => self.hint_open_path(cid, pid, &f.text, *line, *col),
            _ => {
                self.set_buffer(None, &f.text, false);
                let note = crate::clipboard::set_text(&f.text).err().map(|e| format!(" (clipboard: {e})"));
                self.message(cid, &format!("copied {}{}", f.text, note.unwrap_or_default()));
            }
        }
    }

    /// Open `file` (relative to pane `pid`'s directory) at `line`/`col`:
    /// with `hint-open` when it is set, else VS Code, else `$VISUAL` /
    /// `$EDITOR` in a new window, else the desktop.
    fn hint_open_path(&mut self, cid: ClientId, pid: PaneId, file: &str, line: Option<u32>, col: Option<u32>) {
        let dir = self.find_pane_mut(pid).and_then(|p| p.current_path());
        let path = hints::resolve(file, dir.as_deref()).display().to_string();
        let out = if !self.opts.hint_open.is_empty() {
            match crate::command::parse_line(&hints::fill(&self.opts.hint_open, &path, line, col)) {
                Ok(Some(cmd)) => self.exec(cmd, Some(cid)),
                Ok(None) => Outcome::Ok,
                Err(e) => Outcome::Error(format!("hint-open: {e}")),
            }
        } else {
            let editor = std::env::var("VISUAL").ok().or_else(|| std::env::var("EDITOR").ok());
            match hints::plan(&path, line, col, hints::on_path("code"), editor) {
                hints::Plan::Code { code, arg } => {
                    match crate::platform::open::spawn(&code, &["-g".into(), arg.clone()]) {
                        Ok(()) => Outcome::Text(format!("opening {arg} in VS Code")),
                        Err(e) => Outcome::Error(e),
                    }
                }
                hints::Plan::Editor { argv } => {
                    self.exec(Cmd::NewWindow { name: None, cwd: dir, target: None, argv, detached: false }, Some(cid))
                }
                hints::Plan::Desktop => match crate::platform::open::open(&path) {
                    Ok(()) => Outcome::Text(format!("opening {path}")),
                    Err(e) => Outcome::Error(e),
                },
            }
        };
        match out {
            Outcome::Error(e) => self.message(cid, &e),
            Outcome::Text(t) => self.message(cid, &t),
            _ => {}
        }
    }

    fn handle_key(&mut self, cid: ClientId, rec: KeyRecord) {
        let Some(c) = self.clients.get(&cid) else { return };
        let Some(sid) = c.session else { return };
        // `window-size latest`: the client typing is the one used last, so
        // the session takes its size when another client had set it.
        let (ccols, crows) = (c.cols, c.rows);
        if rec.down
            && self.opts.window_size == "latest"
            && self.session(sid).is_some_and(|s| (s.cols, s.rows) != (ccols, crows))
        {
            self.fit_session(sid, Some(cid));
        }
        let Some(c) = self.clients.get_mut(&cid) else { return };
        c.last_activity = Instant::now();
        if !rec.down && c.swallow_up.remove(&rec.vk) {
            return;
        }
        let key = key_from_record(&rec);
        if key.is_some() && self.opts.display_time_ms == 0 {
            c.message = None;
        }

        if c.overlay.is_some() {
            if key.is_some() {
                c.overlay = None;
                c.swallow_up.insert(rec.vk);
            }
            return;
        }
        if c.prompt.is_some() {
            if let Some(k) = key {
                c.swallow_up.insert(rec.vk);
                self.prompt_key(cid, k);
            }
            return;
        }
        // `hints` labels are up: every key is theirs.
        if c.hints.is_some() {
            if let Some(k) = key {
                c.swallow_up.insert(rec.vk);
                self.hints_key(cid, crate::keys::ascii_form(k));
            }
            return;
        }
        // While `display-panes` numbers are up, a digit goes to that pane at
        // once and anything else just puts them away (tmux does the same).
        // Past nine a number takes more than one digit: while one could
        // still grow, the numbers stay up a moment longer and the next digit
        // goes on from there (1 goes to pane 1, then 2 to pane 12).
        if c.panes_until.is_some_and(|t| Instant::now() < t)
            && let Some(k) = key
        {
            c.swallow_up.insert(rec.vk);
            let typed = c.panes_typed.take();
            let until = c.panes_until.take();
            let KeyCode::Char(d @ '0'..='9') = crate::keys::ascii_form(k).code else { return };
            let d = d as usize - '0' as usize;
            let Some((count, active, last)) =
                self.session(sid).and_then(|s| s.window()).map(|w| (w.panes.len(), w.active, w.last_pane))
            else {
                return;
            };
            let base = self.opts.pane_base_index;
            let (lo, hi) = (base, (base + count).saturating_sub(1));
            let now = Instant::now();
            let Some(t) = digits::push(typed.map(|p| p.typed), d, now, lo, hi) else {
                // No such pane: select-pane says so.
                let out = self.exec(Cmd::SelectPane { sel: PaneSel::Index(d) }, Some(cid));
                self.reply(cid, out);
                return;
            };
            let before = typed.map_or((active, last), |p| p.before);
            // (With a high base, `1` can be no pane yet start 10: nothing to
            // go to until the next digit.)
            if (lo..=hi).contains(&t.n) {
                let out = self.exec(Cmd::SelectPane { sel: PaneSel::Index(t.n) }, Some(cid));
                self.reply(cid, out);
                // The digits of one display-panes are one move: `last-pane`
                // goes back to where it began, not to a pane passed through.
                if let Some(w) = self.session_mut(sid).and_then(|s| s.window_mut()) {
                    w.last_pane = if w.active == before.0 { before.1 } else { Some(before.0) };
                }
            }
            if digits::longer(t.n, lo, hi)
                && let Some(c) = self.clients.get_mut(&cid)
            {
                c.panes_typed = Some(PanesTyped { typed: t, before });
                c.panes_until = until.max(Some(now + digits::GAP));
            }
            return;
        }

        // A clock swallows the next key and goes away, as tmux does.
        if key.is_some()
            && let Some(p) = self.session_mut(sid).and_then(|s| s.window_mut()).and_then(|w| w.active_pane_mut())
            && p.clock
        {
            p.clock = false;
            if let Some(c) = self.clients.get_mut(&cid) {
                c.swallow_up.insert(rec.vk);
                c.last_grid = None;
            }
            return;
        }
        let in_copy =
            self.session(sid).and_then(|s| s.window()).and_then(|w| w.active_pane()).is_some_and(|p| p.copy.is_some());
        // The picker is a mode, like copy mode: the prefix still works (so
        // `prefix d` detaches out of it), everything else belongs to the mode.
        let in_chooser = self.clients.get(&cid).is_some_and(|c| c.chooser.is_some());

        if let Some(k) = key {
            let c = self.clients.get_mut(&cid).unwrap();
            if c.prefix {
                c.prefix = false;
                c.prefix_at = None;
                c.swallow_up.insert(rec.vk);
                if k == self.opts.prefix {
                    // Send the prefix key itself to the pane, or to the popup
                    // when one is open: that is how a program in the box gets
                    // the one key the popup keeps for keepane. In copy mode the
                    // key is copy mode's: `C-b C-b` pages up when C-b is the
                    // prefix, as the vi table says C-b does.
                    if in_copy {
                        if let Some(pid) = self.session(sid).and_then(|s| s.window()).map(|w| w.active) {
                            self.copy_key(cid, pid, k);
                        }
                        return;
                    }
                    match self.clients.get_mut(&cid).and_then(|c| c.popup.as_mut()).filter(|p| !p.finished) {
                        Some(p) => {
                            let bytes = p.pane.key_bytes(&rec);
                            p.pane.write_input(&bytes)
                        }
                        None => {
                            let bytes = self.active_key_bytes(sid, &rec);
                            self.write_active(sid, &bytes, typing(&rec))
                        }
                    }
                    return;
                }
                // A key bound as it was typed, else its ASCII form (`【` is
                // `[` with a Chinese input method on).
                let bound = self.prefix_binds.get(&k).or_else(|| self.prefix_binds.get(&crate::keys::ascii_form(k)));
                match bound.cloned() {
                    Some(b) => {
                        // `bind -r`: the same table answers bare keys for a
                        // while, so `prefix h h h` walks three panes left.
                        let until = (b.repeat && self.opts.repeat_time_ms > 0)
                            .then(|| Instant::now() + Duration::from_millis(self.opts.repeat_time_ms));
                        let out = self.exec(b.cmd, Some(cid));
                        self.reply(cid, out);
                        if let Some(c) = self.clients.get_mut(&cid) {
                            c.repeat_until = until;
                        }
                    }
                    None => self.message(cid, &format!("unbound key: {k}")),
                }
                return;
            }
            // Still inside the repeat window: a repeatable key runs again.
            if c.repeat_until.is_some_and(|t| Instant::now() < t) {
                let bound = self.prefix_binds.get(&k).or_else(|| self.prefix_binds.get(&crate::keys::ascii_form(k)));
                match bound.cloned() {
                    Some(b) if b.repeat => {
                        let c = self.clients.get_mut(&cid).unwrap();
                        c.swallow_up.insert(rec.vk);
                        c.repeat_until = Some(Instant::now() + Duration::from_millis(self.opts.repeat_time_ms));
                        let out = self.exec(b.cmd, Some(cid));
                        self.reply(cid, out);
                        return;
                    }
                    // Anything else ends the repeat and is handled normally.
                    _ => self.clients.get_mut(&cid).unwrap().repeat_until = None,
                }
            }
            let c = self.clients.get_mut(&cid).unwrap();
            if k == self.opts.prefix {
                c.prefix = true;
                c.swallow_up.insert(rec.vk);
                c.prefix_at = Some(Instant::now());
                // Woken when the panel is due: no key may come to draw it.
                let delay = self.opts.prefix_hint_delay_ms;
                if self.opts.prefix_hint && delay > 0 {
                    let tx = self.events.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        let _ = tx.send(Event::PrefixHintDue);
                    });
                }
                return;
            }
            // A popup takes every key but the prefix, so `prefix d` still
            // detaches and `prefix :` still reaches the command prompt. The
            // key-up goes through too (as it does for a pane), except for the
            // key that closes a finished popup: that one is not for the pane
            // behind it either.
            if let Some(p) = c.popup.as_ref() {
                if p.finished {
                    c.swallow_up.insert(rec.vk);
                }
                self.popup_key(cid, &rec);
                return;
            }
            if in_chooser {
                c.swallow_up.insert(rec.vk);
                self.chooser_key(cid, crate::keys::ascii_form(k));
                return;
            }
            if in_copy {
                c.swallow_up.insert(rec.vk);
                // A key bound in the copy-mode-vi table runs its command
                // (usually `send -X ...`, which goes to the built-in motions
                // below directly, so a binding never finds itself again);
                // any other key is copy mode's own.
                let ascii = crate::keys::ascii_form(k);
                if let Some(b) = self.copy_binds.get(&k).or_else(|| self.copy_binds.get(&ascii)).cloned() {
                    let out = self.exec(b.cmd, Some(cid));
                    self.reply(cid, out);
                } else if let Some(pid) = self.session(sid).and_then(|s| s.window()).map(|w| w.active) {
                    self.copy_key(cid, pid, ascii);
                }
                return;
            }
            if let Some(b) = self.root_binds.get(&k).cloned() {
                c.swallow_up.insert(rec.vk);
                let until = (b.repeat && self.opts.repeat_time_ms > 0)
                    .then(|| Instant::now() + Duration::from_millis(self.opts.repeat_time_ms));
                let out = self.exec(b.cmd, Some(cid));
                self.reply(cid, out);
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.repeat_until = until;
                }
                return;
            }
        } else if in_copy || in_chooser {
            return;
        } else if self.clients.get(&cid).is_some_and(|c| c.popup.is_some()) {
            self.popup_key(cid, &rec);
            return;
        }
        // Typing into a pane: the view follows the cursor again, right now.
        if let Some(c) = self.clients.get_mut(&cid) {
            c.view_pinned = false;
            c.view_follow = true;
        }
        let bytes = self.active_key_bytes(sid, &rec);
        self.write_active(sid, &bytes, typing(&rec));
    }

    /// A key while a popup is open: it goes to the popup's program, or closes
    /// the box when the program has already finished.
    fn popup_key(&mut self, cid: ClientId, rec: &KeyRecord) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let Some(p) = c.popup.as_mut() else { return };
        if p.finished {
            if rec.down {
                c.popup = None;
                c.last_grid = None;
            }
            return;
        }
        let bytes = p.pane.key_bytes(rec);
        p.pane.write_input(&bytes);
    }

    /// Keep a popup centred and sized as it was asked for. Called before
    /// every frame, so the box follows the window area whatever changed it:
    /// a resized terminal, or `set -g status off` giving a row back.
    fn refit_popup(&mut self, cid: ClientId) {
        let Some(c) = self.clients.get(&cid) else { return };
        if c.popup.is_none() {
            return;
        }
        let area = self.window_area(c.cols, c.rows);
        let c = self.clients.get_mut(&cid).unwrap();
        let p = c.popup.as_mut().unwrap();
        let rect = popup_rect(area, p.width.as_deref(), p.height.as_deref(), p.x.as_deref(), p.y.as_deref());
        if rect.w < 3 || rect.h < 3 {
            c.popup = None; // nowhere left to draw it
            return;
        }
        p.rect = rect;
        p.pane.resize(rect.w - 2, rect.h - 2);
    }

    /// Input for the current window: the active pane, or every pane when
    /// `synchronize-panes` is on. `typing` is whether it counts as someone
    /// typing (a key pressed, not a key let go or a modifier on its own),
    /// which makes the pane busy for its messages.
    /// A typed key as the session's active pane is to get it.
    fn active_key_bytes(&self, sid: SessionId, rec: &KeyRecord) -> Vec<u8> {
        match self.session(sid).and_then(|s| s.window()).and_then(|w| w.active_pane()) {
            Some(p) => p.key_bytes(rec),
            None => crate::platform::keys::to_pane(rec, false),
        }
    }

    fn write_active(&mut self, sid: SessionId, bytes: &[u8], typing: bool) {
        let Some(w) = self.session_mut(sid).and_then(|s| s.window_mut()) else { return };
        let write = |p: &mut Pane| if typing { p.type_input(bytes) } else { p.write_input(bytes) };
        if w.synchronized {
            for p in &mut w.panes {
                write(p);
            }
        } else if let Some(p) = w.active_pane_mut() {
            write(p);
        }
    }

    fn prompt_key(&mut self, cid: ClientId, k: Key) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let Some(p) = c.prompt.as_mut() else { return };
        // A y/n question reads `ｙ` as `y`; a prompt for text keeps what is
        // typed (a name may well hold `【`).
        let k = if matches!(p.kind, PromptKind::Confirm(_) | PromptKind::ConfirmPicker { .. }) {
            crate::keys::ascii_form(k)
        } else {
            k
        };
        if k.code != KeyCode::Tab {
            p.hint = None; // Tab's candidates stay up for one key only
        }
        match &p.kind {
            PromptKind::ConfirmPicker { .. } => {
                let prompt = c.prompt.take().unwrap();
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'))
                    && !k.ctrl
                    && !k.alt
                    && let PromptKind::ConfirmPicker { targets, forget } = prompt.kind
                {
                    self.picker_run(cid, targets, false, forget);
                }
            }
            PromptKind::Confirm(_) => {
                let prompt = c.prompt.take().unwrap();
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'))
                    && !k.ctrl
                    && !k.alt
                    && let PromptKind::Confirm(cmd) = prompt.kind
                {
                    let out = self.exec(*cmd, Some(cid));
                    self.reply(cid, out);
                }
            }
            PromptKind::Command { .. } | PromptKind::Search { .. } | PromptKind::Filter { .. } => {
                match (k.code, k.ctrl, k.alt) {
                    (KeyCode::Escape, _, _) | (KeyCode::Char('c'), true, _) | (KeyCode::Char('g'), true, _) => {
                        // A cancelled filter prompt puts the old filter back.
                        if let Some(Prompt { kind: PromptKind::Filter { prev }, .. }) = c.prompt.take()
                            && let Some(ch) = c.chooser.as_mut()
                        {
                            ch.filter = prev;
                        }
                    }
                    (KeyCode::Enter, _, _) if matches!(p.kind, PromptKind::Filter { .. }) => {
                        c.prompt = None; // the list already follows the input
                    }
                    (KeyCode::Enter, _, _) if matches!(p.kind, PromptKind::Search { .. }) => {
                        let prompt = c.prompt.take().unwrap();
                        let PromptKind::Search { back } = prompt.kind else { unreachable!() };
                        let pid = self
                            .clients
                            .get(&cid)
                            .and_then(|c| c.session)
                            .and_then(|sid| self.session(sid))
                            .and_then(|s| s.window())
                            .map(|w| w.active);
                        if let Some(pid) = pid {
                            self.search_in_copy_mode(cid, pid, &prompt.input, back, true);
                        }
                    }
                    (KeyCode::Enter, _, _) => {
                        let prompt = c.prompt.take().unwrap();
                        let PromptKind::Command { template } = prompt.kind else { unreachable!() };
                        let line = match template {
                            Some(t) => t.replace("%%", &crate::command::quote(&prompt.input)),
                            None => prompt.input,
                        };
                        match crate::command::parse_line(&line) {
                            Ok(Some(cmd)) => {
                                let out = self.exec(cmd, Some(cid));
                                self.reply(cid, out);
                            }
                            Ok(None) => {}
                            Err(e) => self.message(cid, &e),
                        }
                    }
                    // Tab at the command prompt: a command name, or a target
                    // after -t/-s, as far as it is unambiguous; the rest of
                    // the candidates go on the status line.
                    (KeyCode::Tab, false, false) if matches!(p.kind, PromptKind::Command { .. }) => {
                        self.complete_prompt(cid);
                    }
                    (KeyCode::BSpace, false, false) | (KeyCode::Char('h'), true, _) => {
                        if p.cursor > 0 {
                            p.cursor -= 1;
                            let idx = char_index(&p.input, p.cursor);
                            p.input.remove(idx);
                        }
                    }
                    (KeyCode::DC, _, _) | (KeyCode::Char('d'), true, _) => {
                        if p.cursor < p.input.chars().count() {
                            let idx = char_index(&p.input, p.cursor);
                            p.input.remove(idx);
                        }
                    }
                    (KeyCode::Left, _, _) | (KeyCode::Char('b'), true, _) => p.cursor = p.cursor.saturating_sub(1),
                    (KeyCode::Right, _, _) | (KeyCode::Char('f'), true, _) => {
                        p.cursor = (p.cursor + 1).min(p.input.chars().count())
                    }
                    (KeyCode::Home, _, _) | (KeyCode::Char('a'), true, _) => p.cursor = 0,
                    (KeyCode::End, _, _) | (KeyCode::Char('e'), true, _) => p.cursor = p.input.chars().count(),
                    (KeyCode::Char('u'), true, _) => {
                        let idx = char_index(&p.input, p.cursor);
                        p.input = p.input[idx..].to_string();
                        p.cursor = 0;
                    }
                    (KeyCode::Char('k'), true, _) => {
                        let idx = char_index(&p.input, p.cursor);
                        p.input.truncate(idx);
                    }
                    (KeyCode::Char('w'), true, _) => {
                        let idx = char_index(&p.input, p.cursor);
                        let head = &p.input[..idx];
                        let trimmed = head.trim_end();
                        let cut = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                        let removed = head[cut..].chars().count();
                        p.input = format!("{}{}", &head[..cut], &p.input[idx..]);
                        p.cursor -= removed;
                    }
                    (KeyCode::Char(ch), false, false) => {
                        let idx = char_index(&p.input, p.cursor);
                        p.input.insert(idx, ch);
                        p.cursor += 1;
                    }
                    _ => {}
                }
            }
        }
        // A filter prompt drives the picker's list as it is typed.
        if let Some(c) = self.clients.get_mut(&cid)
            && let Some(Prompt { kind: PromptKind::Filter { .. }, input, .. }) = c.prompt.as_ref()
            && let Some(ch) = c.chooser.as_mut()
        {
            ch.filter = input.clone();
        }
    }

    /// Tab at the `:` prompt. The word under the cursor is completed as a
    /// command name when it is the first word, or as a target (a session,
    /// or `session:window`) after `-t` or `-s`. One candidate is typed in
    /// whole (a command gets a space after it); several are typed as far
    /// as they agree and listed on the status line; none says so.
    fn complete_prompt(&mut self, cid: ClientId) {
        let Some((input, cursor)) =
            self.clients.get(&cid).and_then(|c| c.prompt.as_ref()).map(|p| (p.input.clone(), p.cursor))
        else {
            return;
        };
        let at = char_index(&input, cursor);
        let head = &input[..at];
        let start = head.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let word = &head[start..];
        let earlier: Vec<&str> = head[..start].split_whitespace().collect();
        // After `set` / `show`: the option's name, then (for the options
        // that take one of a few) its value. `-s` there is a flag, so the
        // word after it is the name too; only `-t` takes a value.
        let option_arg = earlier
            .first()
            .filter(|c| crate::command::takes_option_name(c))
            .filter(|_| !word.starts_with('-') && earlier.last() != Some(&"-t"))
            .map(|_| crate::command::positional_args(&earlier[1..]));
        let candidates: Vec<String> = match (earlier.last(), option_arg.as_deref()) {
            (None, _) => crate::command::complete_command(word).into_iter().map(str::to_string).collect(),
            (_, Some([])) if !word.starts_with('@') => {
                crate::config::option_candidates(word).into_iter().map(str::to_string).collect()
            }
            (_, Some([name])) => {
                let name = crate::config::resolve_name(name).unwrap_or_default();
                crate::config::option_values(&name)
                    .iter()
                    .filter(|v| v.starts_with(word))
                    .map(|v| v.to_string())
                    .collect()
            }
            // A flag being typed: the command's flags (an alias or a prefix
            // counts as the command it stands for), less those already given.
            (Some(_), _) if word.starts_with('-') => {
                let command = crate::command::canonical_name(earlier[0]).unwrap_or("");
                // Flags given so far, a combined `-gq` counting as `-g -q`.
                let given: Vec<String> = earlier[1..]
                    .iter()
                    .filter(|w| w.starts_with('-') && !w.starts_with("--"))
                    .flat_map(|w| {
                        let letters = &w[1..];
                        if letters.len() > 1 && letters.chars().all(|c| c.is_ascii_alphabetic()) {
                            letters.chars().map(|c| format!("-{c}")).collect::<Vec<_>>()
                        } else {
                            vec![w.to_string()]
                        }
                    })
                    .collect();
                crate::command::flags_of(command)
                    .iter()
                    .filter(|f| f.starts_with(word) && !given.iter().any(|g| g == *f))
                    .map(|f| f.to_string())
                    .collect()
            }
            (Some(&"-t") | Some(&"-s"), _) => {
                let mut names = Vec::new();
                for s in &self.sessions {
                    names.push(s.name.clone());
                    for (i, w) in s.windows.iter().enumerate() {
                        names.push(format!("{}:{}", s.name, i + self.opts.base_index));
                        names.push(format!("{}:{}", s.name, w.name));
                    }
                }
                names.sort();
                names.dedup();
                names.into_iter().filter(|n| n.starts_with(word)).collect()
            }
            _ => Vec::new(),
        };
        // A single command or option name gets a space after it, ready for
        // what follows; a value is the last word, so it does not.
        let ends_word = earlier.is_empty() || matches!(option_arg.as_deref(), Some([])) || word.starts_with('-');
        // The prompt's row is the status line, so the candidates take the
        // label's place until the next key.
        let (replacement, hint) = match candidates.as_slice() {
            [] => (String::new(), Some("(no completion) ".to_string())),
            [one] => (format!("{one}{}", if ends_word { " " } else { "" }), None),
            many => {
                let shown: Vec<&str> = many.iter().take(6).map(String::as_str).collect();
                let more = many.len().saturating_sub(shown.len());
                let tail = if more > 0 { format!(" +{more}") } else { String::new() };
                (
                    crate::command::common_prefix(many.iter().map(String::as_str)),
                    Some(format!("({}{tail}) ", shown.join(" "))),
                )
            }
        };
        let Some(p) = self.clients.get_mut(&cid).and_then(|c| c.prompt.as_mut()) else { return };
        p.hint = hint;
        if replacement.len() <= word.len() {
            return; // nothing more to type yet
        }
        let new_input = format!("{}{replacement}{}", &input[..start], &input[at..]);
        p.cursor = cursor + replacement.chars().count() - word.chars().count();
        p.input = new_input;
    }

    /// `/` `?` `n` `N` in copy mode: move the cursor to the next line holding
    /// `pattern`, case-insensitively, and remember it for `n`.
    ///
    /// `back` searches towards older lines, which is what `?` does (tmux's
    /// `search-backward`). `fresh` starts from the cursor line itself so that
    /// a new search can match what is already on screen.
    fn search_in_copy_mode(&mut self, cid: ClientId, pid: PaneId, pattern: &str, back: bool, fresh: bool) {
        let pattern = pattern.to_string();
        if pattern.is_empty() {
            return;
        }
        let Some(p) = self.find_pane_mut(pid) else {
            return;
        };
        if p.copy.is_none() {
            enter_copy_mode(p);
        }
        let needle = pattern.to_lowercase();
        let from = copy_abs(p);
        let last = p.scrollback_len() + p.rows as usize - 1;
        // Where to look, nearest first.
        let candidates: Vec<usize> = if back {
            let start = if fresh { from } else { from.saturating_sub(1) };
            (0..=start).rev().collect()
        } else {
            let start = if fresh { from } else { from + 1 };
            (start..=last).collect()
        };
        let hit = candidates.into_iter().find_map(|abs| {
            let text = p.line_text(abs).0.to_lowercase();
            text.find(&needle).map(|byte| {
                // The copy cursor counts columns: a wide character is two.
                let col = unicode_width::UnicodeWidthStr::width(&text[..byte]) as u16;
                (abs, col)
            })
        });
        let Some((abs, col)) = hit else {
            if let Some(c) = p.copy.as_mut() {
                c.search = Some(pattern.clone());
                c.search_back = back;
            }
            self.message(cid, &format!("no match: {pattern}"));
            return;
        };
        // Put the hit in the middle of the view when there is room above it.
        let max = p.scrollback_len();
        let rows = p.rows as usize;
        let want_row = (rows / 2).min(abs);
        let offset = (max + want_row).saturating_sub(abs).min(max);
        let cy = (abs + offset).saturating_sub(max).min(rows.saturating_sub(1));
        let cols = p.cols;
        if let Some(c) = p.copy.as_mut() {
            c.offset = offset;
            c.cy = cy as u16;
            c.cx = col.min(cols.saturating_sub(1));
            c.search = Some(pattern);
            c.search_back = back;
        }
    }

    // ----------------------------------------------------------- choose-tree

    /// The picker's lines from the live sessions, tmux `choose-tree` style:
    /// `(n) - name: 2 windows (attached)` and, when expanded,
    /// `(n)   - 0: cmd* (2 panes) "title"` under each session.
    /// The panes `jobs` lists, in session / window / layout order, narrowed
    /// to a session, a window of it, or one pane.
    fn jobs_panes(
        &self,
        only_sid: Option<SessionId>,
        only_widx: Option<usize>,
        only_pid: Option<PaneId>,
    ) -> Vec<(SessionId, usize, PaneId)> {
        let mut panes = Vec::new();
        for s in self.sessions.iter().filter(|s| only_sid.is_none_or(|id| id == s.id)) {
            for (wi, w) in s.windows.iter().enumerate().filter(|(wi, _)| only_widx.is_none_or(|i| i == *wi)) {
                panes.extend(
                    w.layout.panes().into_iter().filter(|p| only_pid.is_none_or(|id| id == *p)).map(|p| (s.id, wi, p)),
                );
            }
        }
        panes
    }

    /// One pane's row of the task board, the columns of `jobs_header`.
    fn jobs_row(&self, sid: SessionId, widx: usize, pid: PaneId, cid: Option<ClientId>) -> Vec<String> {
        let ctx = self.context(sid, widx, Some(pid), cid);
        let t = chrono::Local::now().timestamp();
        vec![
            format!("{}:{}.{}", ctx.session, ctx.window_index, ctx.pane_index),
            match ctx.pane_dead_status {
                Some(code) => format!("exit {code}"),
                None => "running".to_string(),
            },
            // A dead pane's clock stopped when it died: how long it ran.
            crate::format::human_duration(
                (if ctx.pane_dead_time > 0 { ctx.pane_dead_time } else { t }) - ctx.pane_start_time,
            ),
            crate::format::human_duration(t - ctx.pane_activity),
            ctx.pane_pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            ctx.pane_title,
            ctx.pane_path,
        ]
    }

    /// The items and bare lines of a live picker; `chooser_view` filters
    /// and numbers them. None for the fixed lists (`find-window` hits, a
    /// menu), which are built where they are opened.
    fn chooser_lines(&self, kind: &ChooserKind, collapsed: &[ChooserItem]) -> Option<(Vec<ChooserItem>, Vec<String>)> {
        let mut items = Vec::new();
        let mut lines = Vec::new();
        let expand = match kind {
            ChooserKind::Tree { expand, .. } => *expand,
            ChooserKind::Buffers => {
                for (i, (name, data)) in self.buffers.iter().enumerate() {
                    items.push(ChooserItem::Buffer(i));
                    lines.push(format!("{name}: {} bytes: {}", data.len(), one_line(data, 60)));
                }
                return Some((items, lines));
            }
            ChooserKind::Clients => {
                let mut ids: Vec<ClientId> =
                    self.clients.values().filter(|c| c.session.is_some()).map(|c| c.id).collect();
                ids.sort_unstable();
                for id in ids {
                    let Some(c) = self.clients.get(&id) else { continue };
                    let session = c.session.and_then(|s| self.session(s)).map(|s| s.name.as_str()).unwrap_or("-");
                    items.push(ChooserItem::Client(id));
                    lines.push(format!("client-{id}: {session} [{}x{}]", c.cols, c.rows));
                }
                return Some((items, lines));
            }
            ChooserKind::Jobs => {
                // The task board: the header is a title line, every other
                // line is a pane to jump to, kill or restart.
                let panes = self.jobs_panes(None, None, None);
                let mut rows = vec![jobs_header()];
                rows.extend(panes.iter().map(|(sid, widx, pid)| self.jobs_row(*sid, *widx, *pid, None)));
                items.push(ChooserItem::Separator);
                for (sid, widx, pid) in &panes {
                    let wid = self.session(*sid).and_then(|s| s.windows.get(*widx)).map(|w| w.id).unwrap_or(0);
                    items.push(ChooserItem::Job(*sid, wid, *pid));
                }
                lines = align_columns(&rows);
                return Some((items, lines));
            }
            ChooserKind::Found | ChooserKind::Menu(_) | ChooserKind::History { .. } => return None,
        };
        let style = match kind {
            ChooserKind::Tree { style, .. } => *style,
            _ => TreeStyle::List,
        };
        let tree = style != TreeStyle::List;
        let chart = style == TreeStyle::Chart;
        // A chart block has two lines: what it is, then about it.
        let sep = if chart { "\n" } else { " · " };
        // The tree hangs from a root of its own, a title that is never picked.
        if tree {
            items.push(ChooserItem::Separator);
            lines.push("keepane".into());
        }
        // Paired with other machines, the chart has a row of them under
        // the root, this one first.
        let hosts = if chart && expand { self.chart_hosts() } else { Vec::new() };
        if !hosts.is_empty() {
            items.push(ChooserItem::Host(None));
            lines.push(format!(
                "{}\nthis machine · {}",
                crate::sysinfo::hostname(),
                crate::format::count(self.sessions.len(), "session")
            ));
        }
        // The chart shows the chosen branch, so nothing in it is folded.
        let folded = |item: ChooserItem| !chart && collapsed.contains(&item);
        for s in &self.sessions {
            let attached = self.clients.values().any(|c| c.session == Some(s.id));
            let open = expand && !folded(ChooserItem::Tree(s.id, None));
            items.push(ChooserItem::Tree(s.id, None));
            lines.push(if tree {
                let fold = if expand && !open { " +" } else { "" };
                format!(
                    "{}{sep}{}{}{fold}",
                    s.name,
                    crate::format::count(s.windows.len(), "window"),
                    if attached { " · attached" } else { "" }
                )
            } else {
                format!(
                    "{} {}: {} windows{}",
                    if open { "-" } else { "+" },
                    s.name,
                    s.windows.len(),
                    if attached { " (attached)" } else { "" }
                )
            });
            if !open {
                continue;
            }
            for (i, w) in s.windows.iter().enumerate() {
                let flag = if i == s.cur {
                    "*"
                } else if Some(w.id) == s.last {
                    "-"
                } else {
                    ""
                };
                let win_open = !folded(ChooserItem::Tree(s.id, Some(w.id)));
                let index = i + self.opts.base_index;
                let title = w.active_pane().map(|p| p.title.as_str()).unwrap_or("");
                items.push(ChooserItem::Tree(s.id, Some(w.id)));
                lines.push(if tree {
                    let fold = if win_open { "" } else { " +" };
                    format!("{index}:{}{flag}{sep}{}{fold}", w.name, crate::format::count(w.panes.len(), "pane"))
                } else {
                    let mark = if win_open { "-" } else { "+" };
                    format!("  {mark} {index}: {}{flag} ({} panes) \"{title}\"", w.name, w.panes.len())
                });
                if !win_open {
                    continue;
                }
                for (n, pid) in w.layout.panes().into_iter().enumerate() {
                    let Some(p) = w.pane(pid) else { continue };
                    let active = if pid == w.active { "*" } else { "" };
                    let name = p.actor.name.as_deref().map(|n| format!(" · %{n}")).unwrap_or_default();
                    let program = truncate(p.display_title(), 40);
                    let (index, mode) = (n + self.opts.pane_base_index, p.actor.mode.as_str());
                    items.push(ChooserItem::Job(s.id, w.id, pid));
                    lines.push(if tree {
                        format!("{index}:{program}{active}{sep}{mode}{name}")
                    } else {
                        format!("      - {index}: {program}{active} · {mode}{name}")
                    });
                }
            }
        }
        for (m, addr) in hosts.iter().enumerate() {
            self.remote_lines(m as u16, addr, &mut items, &mut lines);
        }
        Some((items, lines))
    }

    /// A paired machine in the chart: its block (what is known of it), then
    /// its sessions, windows and panes as it last said them.
    fn remote_lines(&self, m: u16, addr: &str, items: &mut Vec<ChooserItem>, lines: &mut Vec<String>) {
        use crate::format::count;
        let view = self.remote_views.get(addr);
        let panes: &[link_tree::RemotePane] = view.map(|v| v.panes.as_slice()).unwrap_or_default();
        let mut sessions: Vec<u32> = Vec::new();
        for p in panes {
            if !sessions.contains(&p.session) {
                sessions.push(p.session);
            }
        }
        let about = match view {
            Some(v) if v.heard && v.error.is_some() => format!("{} · offline", count(sessions.len(), "session")),
            Some(v) if v.heard => count(sessions.len(), "session"),
            Some(v) if v.error.is_some() => {
                format!("offline: {}", truncate(v.error.as_deref().unwrap_or_default(), 60))
            }
            _ => "asking…".to_string(),
        };
        items.push(ChooserItem::Host(Some(m)));
        lines.push(format!("{addr}\n{about}"));
        for s in sessions {
            let of_s: Vec<&link_tree::RemotePane> = panes.iter().filter(|p| p.session == s).collect();
            let mut windows: Vec<u32> = Vec::new();
            for p in &of_s {
                if !windows.contains(&p.window) {
                    windows.push(p.window);
                }
            }
            let name = of_s[0].session_name.clone().unwrap_or_else(|| format!("${s}"));
            items.push(ChooserItem::Remote(m, s, None, None));
            lines.push(format!("{name}\n{}", count(windows.len(), "window")));
            for w in windows {
                let of_w: Vec<&&link_tree::RemotePane> = of_s.iter().filter(|p| p.window == w).collect();
                let first = of_w[0];
                let title = match (first.window_index, &first.window_name) {
                    (Some(i), Some(n)) => format!("{i}:{n}{}", if first.window_active { "*" } else { "" }),
                    _ => format!("@{w}"),
                };
                items.push(ChooserItem::Remote(m, s, Some(w), None));
                lines.push(format!("{title}\n{}", count(of_w.len(), "pane")));
                for p in of_w {
                    let at = p.pane_index.map(|i| i.to_string()).unwrap_or_else(|| format!("%{}", p.pane));
                    let active = if p.pane_active { "*" } else { "" };
                    let name = if p.name.is_empty() { String::new() } else { format!(" · %{}", p.name) };
                    items.push(ChooserItem::Remote(m, s, Some(w), Some(p.pane)));
                    lines.push(format!("{at}:{}{active}\n{}{name}", truncate(&p.command, 40), p.mode));
                }
            }
        }
    }

    /// Whether a line of the session tree is its parent's current one: a
    /// session's current window, a window's active pane.
    fn current_in_tree(&self, item: &ChooserItem) -> bool {
        match *item {
            ChooserItem::Tree(sid, Some(wid)) => {
                self.session(sid).and_then(|s| s.window()).is_some_and(|w| w.id == wid)
            }
            ChooserItem::Job(sid, wid, pid) => {
                self.session(sid).and_then(|s| s.windows.iter().find(|w| w.id == wid)).is_some_and(|w| w.active == pid)
            }
            ChooserItem::Host(None) => true,
            ChooserItem::Remote(m, s, Some(w), pane) => {
                let hosts = self.chart_hosts();
                let Some(view) = hosts.get(m as usize).and_then(|a| self.remote_views.get(a)) else { return false };
                view.panes.iter().any(|p| match pane {
                    None => p.session == s && p.window == w && p.window_active,
                    Some(id) => p.session == s && p.window == w && p.pane == id && p.pane_active,
                })
            }
            _ => false,
        }
    }

    /// What a live picker shows right now: its lines through the filter
    /// (a session stays when one of its windows matches, and its windows
    /// stay when it does), numbered for the digit keys, tagged ones marked.
    fn chooser_view(&self, ch: &Chooser) -> Option<(Vec<ChooserItem>, Vec<String>)> {
        let (items, lines) = self.chooser_lines(&ch.kind, &ch.collapsed)?;
        let keep = chooser_filter(&items, &lines, &ch.filter);
        let rows = keep.iter().filter(|k| **k).count();
        let tree = matches!(ch.kind, ChooserKind::Tree { style, .. } if style != TreeStyle::List);
        let mut out_items = Vec::new();
        let mut out_lines = Vec::new();
        for (item, line) in items.into_iter().zip(lines).zip(keep).filter(|(_, k)| *k).map(|(p, _)| p) {
            let n = out_items.len();
            let mark = if ch.is_tagged(&item) { '*' } else { ' ' };
            out_lines.push(if tree {
                // The number in the block, before what the line is.
                if item == ChooserItem::Separator {
                    line
                } else {
                    format!("{}{n}  {line}", if mark == '*' { "* " } else { "" })
                }
            } else {
                let number = digits::label((item != ChooserItem::Separator).then_some(n), rows);
                format!("{number}{mark}{line}")
            });
            out_items.push(item);
        }
        Some((out_items, out_lines))
    }

    fn chooser_key(&mut self, cid: ClientId, k: Key) {
        let Some((cols, rows)) = self.clients.get(&cid).map(|c| (c.cols, c.rows)) else { return };
        let page = self.window_area(cols, rows).h.saturating_sub(1).max(1) as i64; // body rows of the picker
        let Some(c) = self.clients.get_mut(&cid) else { return };
        // A menu entry's own key picks it, ahead of the movement keys, so a
        // menu is free to use `j` or `q` for something.
        let menu_hit = match c.chooser.as_ref().map(|ch| &ch.kind) {
            Some(ChooserKind::Menu(entries)) if k.code != KeyCode::Escape => entries
                .iter()
                .find(|e| e.cmd.is_some() && e.key.as_deref().and_then(Key::parse) == Some(k))
                .and_then(|e| e.cmd.clone()),
            _ => None,
        };
        if let Some(cmd) = menu_hit {
            c.chooser = None;
            let out = self.exec(*cmd, Some(cid));
            self.reply(cid, out);
            return;
        }
        let Some(ch) = c.chooser.as_mut() else { return };
        if !matches!((k.code, k.ctrl, k.alt), (KeyCode::Char('0'..='9'), false, false)) {
            ch.typed = None;
        }
        let tree_view = matches!(ch.kind, ChooserKind::Tree { style, .. } if style != TreeStyle::List);
        let chart = matches!(ch.kind, ChooserKind::Tree { style: TreeStyle::Chart, .. });
        // The chart goes by its rows: j and k down to a child and up to the
        // parent, h and l along the row, as the arrows do.
        let mut k = k;
        if chart && !k.ctrl && !k.alt {
            k.code = match k.code {
                KeyCode::Char('j') => KeyCode::Down,
                KeyCode::Char('k') => KeyCode::Up,
                KeyCode::Char('h') => KeyCode::Left,
                KeyCode::Char('l') => KeyCode::Right,
                other => other,
            };
        }
        match (k.code, k.ctrl, k.alt) {
            (KeyCode::Escape, _, _) | (KeyCode::Char('q'), false, false) | (KeyCode::Char('c'), true, _) => {
                c.chooser = None;
            }
            // (In the tree view the arrows go by the tree, below.)
            (KeyCode::Down, _, _) if !tree_view => ch.step(1),
            (KeyCode::Up, _, _) if !tree_view => ch.step(-1),
            (KeyCode::Char('j'), false, false) | (KeyCode::Char('n'), true, _) => ch.step(1),
            (KeyCode::Char('k'), false, false) | (KeyCode::Char('p'), true, _) => ch.step(-1),
            (KeyCode::Home, _, _) | (KeyCode::Char('g'), false, false) => {
                ch.sel = 0;
                ch.settle(1);
            }
            (KeyCode::End, _, _) | (KeyCode::Char('G'), false, false) => {
                ch.sel = ch.items.len().saturating_sub(1);
                ch.settle(-1);
            }
            (KeyCode::NPage, _, _) | (KeyCode::Char('f'), true, _) => ch.step(page),
            (KeyCode::PPage, _, _) | (KeyCode::Char('b'), true, _) => ch.step(-page),
            (KeyCode::Char('d'), true, _) => ch.step(page / 2),
            (KeyCode::Char('u'), true, _) => ch.step(-(page / 2)),
            // Tags: `t` marks the line and moves on, `T` clears them; an
            // action key then works on the tagged lines instead of the
            // current one.
            (KeyCode::Char('t'), false, false) if ch.kind.live() => {
                if let Some(item) = ch.items.get(ch.sel).copied()
                    && item != ChooserItem::Separator
                {
                    match ch.tagged.iter().position(|t| *t == item) {
                        Some(i) => {
                            ch.tagged.remove(i);
                        }
                        None => ch.tagged.push(item),
                    }
                    ch.step(1);
                }
            }
            (KeyCode::Char('T'), false, false) if ch.kind.live() => ch.tagged.clear(),
            // The history picker: a position folds and unfolds its days
            // (Enter too, on the position's own line).
            (KeyCode::Left | KeyCode::Right | KeyCode::Enter, _, _) | (KeyCode::Char('-' | '+'), false, false)
                if matches!(ch.kind, ChooserKind::History { .. })
                    && !(k.code == KeyCode::Enter
                        && matches!(ch.items.get(ch.sel), Some(ChooserItem::Log(_, Some(_))))) =>
            {
                let Some(ChooserItem::Log(i, _)) = ch.items.get(ch.sel).copied() else { return };
                let ChooserKind::History { kept, open } = &mut ch.kind else { return };
                let unfold = match k.code {
                    KeyCode::Left | KeyCode::Char('-') => false,
                    KeyCode::Right | KeyCode::Char('+') => true,
                    _ => !open.contains(&i),
                };
                if unfold {
                    open.insert(i);
                } else {
                    open.remove(&i);
                }
                let (items, lines) = history_lines(kept, open);
                // Unfolded: on its newest day; folded: on the position.
                let at = ChooserItem::Log(i, unfold.then_some(0));
                ch.sel = items.iter().position(|x| *x == at).unwrap_or(0);
                ch.items = items;
                ch.lines = lines;
            }
            // The tree and the chart: Up and Down go to the parent and a
            // child (the tree's first, opening a folded line; the chart's
            // shown one), Left and Right to the one before and after on the
            // same level, across parents.
            (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right, _, _) if tree_view => {
                let depths = ChooserItem::depths(&ch.items);
                let Some(d) = depths.get(ch.sel).copied().flatten() else { return };
                let to = match k.code {
                    KeyCode::Up => {
                        (0..ch.sel).rev().find(|&i| depths[i] == Some(d.saturating_sub(1))).filter(|_| d > 1)
                    }
                    KeyCode::Down if chart => {
                        let depths: Vec<usize> = depths.iter().map(|d| d.unwrap_or(0)).collect();
                        render::chart_path(&depths, &ch.active, ch.sel).get(d + 1).copied()
                    }
                    KeyCode::Down => {
                        let here = ch.items[ch.sel];
                        if let Some(at) = ch.collapsed.iter().position(|c| *c == here) {
                            // Folded: open it; the child is there at the next render.
                            ch.collapsed.remove(at);
                            None
                        } else {
                            Some(ch.sel + 1).filter(|&i| depths.get(i) == Some(&Some(d + 1)))
                        }
                    }
                    KeyCode::Left => (0..ch.sel).rev().find(|&i| depths[i] == Some(d)),
                    _ => (ch.sel + 1..depths.len()).find(|&i| depths[i] == Some(d)),
                };
                if let Some(i) = to {
                    ch.sel = i;
                }
            }
            // `a`: the chart opens every node, or only the selected branch.
            (KeyCode::Char('a'), false, false) if chart => ch.all = !ch.all,
            // `v`: the chart, the tree of blocks, the plain list, in turn.
            (KeyCode::Char('v'), false, false) if matches!(ch.kind, ChooserKind::Tree { .. }) => {
                if let ChooserKind::Tree { style, .. } = &mut ch.kind {
                    *style = style.next();
                }
            }
            // Fold a session or a window (a pane folds its window), and
            // unfold it; the cursor goes to the folded line, since what was
            // under it is gone from the list. In the plain list Left and
            // Right do it too, as before.
            (KeyCode::Left, _, _) | (KeyCode::Char('-'), false, false)
                if matches!(ch.kind, ChooserKind::Tree { expand: true, .. }) && !chart =>
            {
                // An open line folds itself; a folded window (or a pane)
                // folds the line above it, as a file tree does.
                let target = match ch.items.get(ch.sel).copied() {
                    Some(here @ ChooserItem::Tree(s, Some(_))) if ch.collapsed.contains(&here) => {
                        Some(ChooserItem::Tree(s, None))
                    }
                    other => other.as_ref().and_then(ChooserItem::fold_target),
                };
                if let Some(target) = target {
                    if !ch.collapsed.contains(&target) {
                        ch.collapsed.push(target);
                    }
                    if let Some(i) = ch.items.iter().position(|it| *it == target) {
                        ch.sel = i;
                    }
                }
            }
            (KeyCode::Right, _, _) | (KeyCode::Char('+'), false, false) | (KeyCode::Char('='), false, false)
                if matches!(ch.kind, ChooserKind::Tree { expand: true, .. }) && !chart =>
            {
                if let Some(target) = ch.items.get(ch.sel).and_then(ChooserItem::fold_target) {
                    ch.collapsed.retain(|c| *c != target);
                }
            }
            // `f` types a filter on the status line; the list follows it as
            // it is typed, Enter keeps it, Escape puts the old one back.
            (KeyCode::Char('f'), false, false) if ch.kind.live() => {
                let prev = ch.filter.clone();
                let cursor = prev.chars().count();
                c.prompt = Some(Prompt {
                    kind: PromptKind::Filter { prev: prev.clone() },
                    label: "(filter) ".into(),
                    input: prev,
                    cursor,
                    hint: None,
                });
            }
            // The task board and the tree act on the tagged lines, else the
            // one under the cursor, and stay open (the list is live, so the
            // rows change in place). A session asks first.
            (KeyCode::Char(key @ ('x' | 'r')), false, false)
                if matches!(ch.kind, ChooserKind::Jobs)
                    || (key == 'x' && matches!(ch.kind, ChooserKind::Tree { .. })) =>
            {
                let targets = ch.targets();
                self.picker_act(cid, targets, key == 'r', false);
            }
            // `D` or Delete in the tree: killed, and a session's save
            // forgotten, so that `resume` does not bring it back.
            (KeyCode::Char('D'), false, false) | (KeyCode::DC, false, false)
                if matches!(ch.kind, ChooserKind::Tree { .. }) =>
            {
                let targets = ch.targets();
                self.picker_act(cid, targets, false, true);
            }
            // Past nine a line number takes more than one digit (`1` `2`).
            (KeyCode::Char(d @ '0'..='9'), false, false) => {
                let d = d as usize - '0' as usize;
                ch.typed =
                    ch.items.len().checked_sub(1).and_then(|hi| digits::push(ch.typed, d, Instant::now(), 0, hi));
                // A line that is a title (the board's header) is no pick.
                if let Some(t) = ch.typed
                    && ch.items.get(t.n).is_some_and(|it| *it != ChooserItem::Separator)
                {
                    ch.sel = t.n;
                }
            }
            (KeyCode::Enter, _, _) => {
                let target = ch.items.get(ch.sel).copied();
                let menu = match (&ch.kind, target) {
                    (ChooserKind::Menu(entries), Some(ChooserItem::Menu(i))) => {
                        entries.get(i).and_then(|e| e.cmd.clone())
                    }
                    _ => None,
                };
                let log = match (&ch.kind, target) {
                    (ChooserKind::History { kept, .. }, Some(ChooserItem::Log(i, Some(d)))) => {
                        kept.get(i).and_then(|k| k.days.get(d)).map(|(_, path, _)| path.clone())
                    }
                    _ => None,
                };
                // A machine in the chart: another machine's pane shows its
                // screen over the chart, which stays; the rest says how.
                match target {
                    Some(ChooserItem::Remote(m, s, Some(w), Some(p))) => {
                        if let Some(addr) = self.chart_hosts().get(m as usize).cloned() {
                            self.peek_remote(cid, &addr, &format!("${s}:@{w}.%{p}"));
                        }
                        return;
                    }
                    Some(ChooserItem::Host(_) | ChooserItem::Remote(..)) => {
                        self.message(cid, "Enter on a pane: one here goes there, another machine's shows its screen");
                        return;
                    }
                    _ => {}
                }
                c.chooser = None;
                if let Some(cmd) = menu {
                    let out = self.exec(*cmd, Some(cid));
                    self.reply(cid, out);
                    return;
                }
                if let Some(path) = log {
                    self.view_log(cid, &path);
                    return;
                }
                match target {
                    Some(ChooserItem::Tree(sid, wid)) => self.chooser_go(cid, sid, wid),
                    Some(ChooserItem::Client(other)) => {
                        if self.clients.contains_key(&other) {
                            self.detach(other, "detached");
                        } else {
                            self.message(cid, "client is gone");
                        }
                    }
                    Some(ChooserItem::Job(sid, wid, pid)) => self.chooser_go_pane(cid, sid, wid, pid),
                    Some(ChooserItem::Menu(_))
                    | Some(ChooserItem::Separator)
                    | Some(ChooserItem::Log(..))
                    | Some(ChooserItem::Host(_))
                    | Some(ChooserItem::Remote(..)) => {}
                    Some(ChooserItem::Buffer(i)) => {
                        let name = self.buffers.get(i).map(|(n, _)| n.clone());
                        match name {
                            Some(n) => {
                                let out = self
                                    .exec(Cmd::PasteBuffer { name: Some(n), target: None, bracketed: true }, Some(cid));
                                self.reply(cid, out);
                            }
                            None => self.message(cid, "buffer is gone"),
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// `x` (or `r`, `D`) on picker lines: a session among them is asked
    /// about first, since killing it cannot be undone; the rest goes at once.
    fn picker_act(&mut self, cid: ClientId, targets: Vec<ChooserItem>, restart: bool, forget: bool) {
        let names: Vec<String> = targets
            .iter()
            .filter_map(|t| match t {
                ChooserItem::Tree(sid, None) => self.session(*sid).map(|s| s.name.clone()),
                _ => None,
            })
            .collect();
        if restart || names.is_empty() {
            return self.picker_run(cid, targets, restart, forget);
        }
        let what = if forget {
            format!(
                "kill and forget {} {} (resume will not bring it back)? (y/n) ",
                if names.len() == 1 { "session" } else { "sessions" },
                names.join(", ")
            )
        } else {
            format!("kill {} {}? (y/n) ", if names.len() == 1 { "session" } else { "sessions" }, names.join(", "))
        };
        if let Some(c) = self.clients.get_mut(&cid) {
            c.prompt = Some(Prompt {
                kind: PromptKind::ConfirmPicker { targets, forget },
                label: what,
                input: String::new(),
                cursor: 0,
                hint: None,
            });
        }
    }

    /// What `picker_act` does (after the question, for a session).
    fn picker_run(&mut self, cid: ClientId, targets: Vec<ChooserItem>, restart: bool, forget: bool) {
        if let Some(ch) = self.clients.get_mut(&cid).and_then(|c| c.chooser.as_mut()) {
            ch.tagged.clear();
        }
        // Each target is looked up by identity just before its command
        // runs: killing one window renumbers the rest.
        for item in targets {
            let mut forget_name = None;
            let cmd = match item {
                ChooserItem::Job(sid, wid, pid) => match self.pane_target(sid, wid, pid) {
                    Some(target) if !restart => Cmd::KillPane { target: Some(target), all_but: false },
                    Some(target) => {
                        Cmd::RespawnPane { target: Some(target), kill: true, argv: Vec::new(), window: false }
                    }
                    None => {
                        self.message(cid, "pane is gone");
                        continue;
                    }
                },
                ChooserItem::Tree(sid, wid) => {
                    let Some(s) = self.session(sid) else {
                        self.message(cid, "session is gone");
                        continue;
                    };
                    let mut target = Target { session: Some(s.name.clone()), ..Default::default() };
                    match wid {
                        None => {
                            forget_name = forget.then(|| s.name.clone());
                            Cmd::KillSession { target: Some(target), all_but: false }
                        }
                        Some(w) => match s.windows.iter().position(|x| x.id == w) {
                            Some(i) => {
                                target.window = Some((i + self.opts.base_index).to_string());
                                Cmd::KillWindow { target: Some(target), all_but: false }
                            }
                            None => {
                                self.message(cid, "window is gone");
                                continue;
                            }
                        },
                    }
                }
                ChooserItem::Host(_) | ChooserItem::Remote(..) => {
                    self.message(cid, "x closes what is on this machine only");
                    continue;
                }
                _ => continue,
            };
            if let Outcome::Error(e) = self.exec(cmd, Some(cid)) {
                self.message(cid, &e);
                continue;
            }
            // Its save goes too (none there, say with autosave off: nothing to do).
            if let Some(name) = forget_name {
                self.forget_saved(&name);
            }
        }
    }

    /// The saves of sessions that ended by themselves `FORGET_AFTER` ago,
    /// unless a shutdown was announced in the last `ENDING_HOLDS`, a session of that name runs
    /// again, or autosave is off (a save made by hand is kept).
    fn forget_due(&mut self) {
        if self.ending.is_some_and(|t| t.elapsed() < ENDING_HOLDS) {
            return;
        }
        let (due, wait): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.forget_after).into_iter().partition(|(_, at)| at.elapsed() >= FORGET_AFTER);
        self.forget_after = wait;
        for (name, _) in due {
            if self.opts.autosave && !self.sessions.iter().any(|s| s.name == name) {
                self.forget_saved(&name);
            }
        }
    }

    /// Remove a session's save file, so that `resume` does not bring it back.
    fn forget_saved(&mut self, name: &str) {
        if let Some(p) = crate::resurrect::find(&self.sessions_dir(), name) {
            match std::fs::remove_file(&p) {
                Ok(()) => {
                    self.last_saved.remove(&p.to_string_lossy().into_owned());
                    log::info!("forgot the save of session {name}");
                }
                Err(e) => log::warn!("forget {}: {e}", p.display()),
            }
        }
    }

    /// A day of the history log in `keepane view`, in a popup over the window.
    fn view_log(&mut self, cid: ClientId, path: &std::path::Path) {
        // Gone since the list was read (cleared out, deleted by hand): the
        // viewer would fail and its popup vanish before it could say so.
        if !path.is_file() {
            self.message(cid, &format!("{} is gone", path.display()));
            return;
        }
        let Some(exe) = pane::helper_exe() else {
            self.message(cid, "no keepane.exe to view the log with");
            return;
        };
        let cmd = Cmd::DisplayPopup {
            close: false,
            close_on_exit: true,
            width: Some("90%".into()),
            height: Some("90%".into()),
            x: None,
            y: None,
            cwd: None,
            argv: vec![exe.to_string_lossy().into_owned(), "view".into(), path.to_string_lossy().into_owned()],
        };
        if let Outcome::Error(e) = self.exec(cmd, Some(cid)) {
            self.message(cid, &e);
        }
    }

    /// Switch the client to `sid` and, when given, make `wid` its window.
    fn chooser_go(&mut self, cid: ClientId, sid: SessionId, wid: Option<WindowId>) {
        let Some(s) = self.session(sid) else {
            self.message(cid, "session is gone");
            return;
        };
        let widx = match wid {
            Some(w) => match s.windows.iter().position(|x| x.id == w) {
                Some(i) => Some(i),
                None => {
                    self.message(cid, "window is gone");
                    return;
                }
            },
            None => None,
        };
        let name = s.name.clone();
        let out = self.exec(
            Cmd::SwitchClient {
                next: false,
                prev: false,
                last: false,
                target: Some(Target { session: Some(name), ..Default::default() }),
            },
            Some(cid),
        );
        if let Outcome::Error(e) = out {
            self.message(cid, &e);
            return;
        }
        // Same effect as `select-window -t`, hook included.
        if let Some(i) = widx
            && let Some(s) = self.session_mut(sid)
        {
            s.select_window(i);
            self.fire_hook("after-select-window", Some(cid));
        }
    }

    /// `session:window.pane` for a pane, as the commands want it; None
    /// when it is gone.
    fn pane_target(&self, sid: SessionId, wid: WindowId, pid: PaneId) -> Option<Target> {
        let s = self.session(sid)?;
        let widx = s.windows.iter().position(|w| w.id == wid)?;
        let w = &s.windows[widx];
        let pidx = w.layout.panes().iter().position(|p| *p == pid)?;
        Some(Target {
            session: Some(s.name.clone()),
            window: Some((widx + self.opts.base_index).to_string()),
            pane: Some(pidx + self.opts.pane_base_index),
            ..Default::default()
        })
    }

    /// Switch the client to a pane: its session, its window, then the pane.
    fn chooser_go_pane(&mut self, cid: ClientId, sid: SessionId, wid: WindowId, pid: PaneId) {
        if self.pane_target(sid, wid, pid).is_none() {
            self.message(cid, "pane is gone");
            return;
        }
        self.chooser_go(cid, sid, Some(wid));
        let keep_zoom = self.opts.keep_zoom;
        let area = self.session(sid).map(|s| self.window_area(s.cols, s.rows)).unwrap_or_default();
        let border = self.border_rows();
        if let Some(s) = self.session_mut(sid)
            && let Some(w) = s.windows.iter_mut().find(|w| w.id == wid)
            && w.active != pid
            && w.pane(pid).is_some()
        {
            let zoom_moves = w.zoomed && keep_zoom;
            let from = if zoom_moves {
                w.unzoomed_rects(area, border).into_iter().find(|(p, _)| *p == pid).map(|(_, r)| r)
            } else {
                w.rect_of(w.active)
            };
            w.last_pane = Some(w.active);
            w.active = pid;
            // As select-pane: a zoomed window zooms the pane gone to (or,
            // with keep-zoom off, unzooms). Either way the rectangles
            // change: the old ones would show the pane left.
            if w.zoomed {
                w.zoomed = keep_zoom;
                self.relayout_session(sid);
            }
            if let Some(w) = self.session_mut(sid).and_then(|s| s.windows.iter_mut().find(|w| w.id == wid)) {
                let to = w.rect_of(pid);
                if zoom_moves {
                    w.animate_zoom(pid, from, to);
                } else {
                    w.animate(from, to);
                }
            }
            self.fire_hook("after-select-pane", Some(cid));
        }
    }

    // ------------------------------------------------------------- copy mode

    fn copy_key(&mut self, cid: ClientId, pid: PaneId, k: Key) {
        let Some(p) = self.find_pane_mut(pid) else {
            return;
        };
        let rows = p.rows as i64;
        let cols = p.cols;
        // vi counts: digits before a motion repeat it ("3j"), except a 0 with
        // no count yet, which is "start of line".
        let has_count = p.copy.as_ref().and_then(|c| c.count).is_some();
        if let KeyCode::Char(d @ '0'..='9') = k.code
            && !k.ctrl
            && !k.alt
            && (d != '0' || has_count)
            && let Some(c) = p.copy.as_mut()
        {
            let n = c.count.unwrap_or(0).saturating_mul(10) + (d as usize - '0' as usize);
            c.count = Some(n.min(10_000));
            return;
        }
        let count = p.copy.as_mut().and_then(|c| c.count.take()).unwrap_or(1).max(1);
        // Motions that repeat with a count do so here, once each.
        if count > 1
            && matches!(
                (k.code, k.ctrl),
                (KeyCode::Char('j' | 'k' | 'h' | 'l' | 'w' | 'b' | 'e' | 'W' | 'B' | 'E'), false)
                    | (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right, _)
            )
        {
            for _ in 0..count {
                self.copy_key(cid, pid, k);
            }
            return;
        }
        let Some(c) = p.copy.as_mut() else { return };
        match (k.code, k.ctrl, k.alt) {
            (KeyCode::Escape, _, _) => {
                if c.anchor.is_some() {
                    c.anchor = None;
                } else {
                    exit_copy_mode(p);
                }
            }
            (KeyCode::Char('q'), false, false) | (KeyCode::Char('c'), true, _) => exit_copy_mode(p),
            (KeyCode::Up, _, _) | (KeyCode::Char('k'), false, false) => {
                if c.cy > 0 {
                    c.cy -= 1;
                } else {
                    copy_scroll(p, 1);
                }
            }
            (KeyCode::Down, _, _) | (KeyCode::Char('j'), false, false) => {
                if (c.cy as i64) < rows - 1 {
                    c.cy += 1;
                } else {
                    copy_scroll(p, -1);
                }
            }
            (KeyCode::Left, _, _) | (KeyCode::Char('h'), false, false) => c.cx = c.cx.saturating_sub(1),
            (KeyCode::Right, _, _) | (KeyCode::Char('l'), false, false) => {
                c.cx = (c.cx + 1).min(cols.saturating_sub(1))
            }
            (KeyCode::PPage, _, _) | (KeyCode::Char('b'), true, _) => copy_scroll(p, rows),
            (KeyCode::NPage, _, _) | (KeyCode::Char('f'), true, _) | (KeyCode::Char(' '), true, _) => {
                copy_scroll(p, -rows)
            }
            (KeyCode::Char('u'), true, _) => copy_scroll(p, rows / 2),
            (KeyCode::Char('d'), true, _) => copy_scroll(p, -rows / 2),
            (KeyCode::Char('g'), false, false) => {
                let max = p.scrollback_len();
                let c = p.copy.as_mut().unwrap();
                c.offset = max;
                c.cy = 0;
            }
            (KeyCode::Char('G'), false, false) => {
                c.offset = 0;
                c.cy = (rows - 1) as u16;
            }
            (KeyCode::Home, _, _) | (KeyCode::Char('0'), false, false) | (KeyCode::Char('a'), true, _) => c.cx = 0,
            (KeyCode::End, _, _) | (KeyCode::Char('$'), false, false) | (KeyCode::Char('e'), true, _) => {
                c.cx = cols.saturating_sub(1)
            }
            (KeyCode::Char(' '), false, false) | (KeyCode::Char('v'), false, false) => {
                let abs = copy_abs(p);
                let c = p.copy.as_mut().unwrap();
                c.anchor = Some((abs, c.cx));
            }
            // A rectangle instead of whole lines, as vi's C-v does.
            (KeyCode::Char('v'), true, _) => {
                let abs = copy_abs(p);
                let c = p.copy.as_mut().unwrap();
                c.rect = !c.rect;
                if c.anchor.is_none() {
                    c.anchor = Some((abs, c.cx));
                }
            }
            // Word motions.
            (KeyCode::Char(w @ ('w' | 'W' | 'b' | 'B' | 'e' | 'E')), false, false) => {
                copy_word_motion(p, w);
            }
            // First non-blank of the line.
            (KeyCode::Char('^'), false, false) => {
                let abs = copy_abs(p);
                let (line, _) = p.line_text(abs);
                let blank = &line[..line.len() - line.trim_start().len()];
                let col = unicode_width::UnicodeWidthStr::width(blank) as u16;
                let c = p.copy.as_mut().unwrap();
                c.cx = col.min(cols.saturating_sub(1));
            }
            // Top, middle and bottom of what is on screen.
            (KeyCode::Char('H'), false, false) => c.cy = 0,
            (KeyCode::Char('M'), false, false) => c.cy = (rows / 2) as u16,
            (KeyCode::Char('L'), false, false) => c.cy = (rows - 1).max(0) as u16,
            // Paragraphs: the next or previous blank line.
            (KeyCode::Char(dir @ ('{' | '}')), false, false) => {
                let back = dir == '{';
                let from = copy_abs(p);
                let last = p.scrollback_len() + p.rows as usize - 1;
                let mut abs = from;
                loop {
                    if back {
                        if abs == 0 {
                            break;
                        }
                        abs -= 1;
                    } else {
                        if abs >= last {
                            break;
                        }
                        abs += 1;
                    }
                    if p.line_text(abs).0.trim().is_empty() && abs != from {
                        break;
                    }
                }
                copy_goto(p, abs);
            }
            // Commands: the previous or next one the shell ran, the cursor
            // where it was typed (the prompt's end).
            (KeyCode::Char(dir @ ('[' | ']')), false, false) => {
                let back = dir == '[';
                let from = copy_abs(p);
                let marks = p.readable_marks();
                let at: Vec<(usize, u16)> =
                    marks.iter().filter_map(|m| Some((p.abs_of_line(m.line)?, m.col))).collect();
                let to = if back {
                    at.iter().filter(|(a, _)| *a < from).max_by_key(|(a, _)| *a)
                } else {
                    at.iter().filter(|(a, _)| *a > from).min_by_key(|(a, _)| *a)
                };
                match to {
                    Some(&(abs, col)) => {
                        copy_goto(p, abs);
                        if let Some(c) = p.copy.as_mut() {
                            c.cx = col.min(cols.saturating_sub(1));
                        }
                    }
                    None => {
                        let which = if back { "earlier" } else { "later" };
                        self.message(cid, &format!("no {which} command"));
                    }
                }
            }
            // Search as tmux does: / looks forward (towards the newest line),
            // ? looks back through the scrollback, n and N repeat.
            (KeyCode::Char('/'), false, false) | (KeyCode::Char('?'), false, false) => {
                let back = k.code == KeyCode::Char('?');
                if let Some(cl) = self.clients.get_mut(&cid) {
                    cl.prompt = Some(Prompt {
                        kind: PromptKind::Search { back },
                        label: if back { "?".into() } else { "/".into() },
                        input: String::new(),
                        cursor: 0,
                        hint: None,
                    });
                }
            }
            (KeyCode::Char('n'), false, false) | (KeyCode::Char('N'), false, false) => {
                let Some((pattern, back)) = c.search.clone().map(|s| (s, c.search_back)) else {
                    self.message(cid, "no previous search");
                    return;
                };
                let back = if k.code == KeyCode::Char('N') { !back } else { back };
                self.search_in_copy_mode(cid, pid, &pattern, back, false);
            }
            (KeyCode::Enter, _, _) | (KeyCode::Char('y'), false, false) | (KeyCode::Char('w'), true, _) => {
                if c.anchor.is_some() {
                    match copy_selection(p) {
                        Ok((n, text, clipboard)) => {
                            // Both places, as tmux with set-clipboard does:
                            // a new paste buffer and the Windows clipboard.
                            self.set_buffer(None, &text, false);
                            let note = clipboard.map(|e| format!(" ({e})")).unwrap_or_default();
                            self.message(cid, &format!("copied {n} characters{note}"));
                        }
                        Err(e) => self.message(cid, &e),
                    }
                }
                if let Some(p) = self.find_pane_mut(pid) {
                    exit_copy_mode(p);
                }
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------- mouse

    fn handle_mouse(&mut self, cid: ClientId, m: MouseRecord) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let Some(sid) = c.session else { return };
        // A popup owns the keyboard but not the mouse: the click would land
        // on a pane it covers, so drop it instead.
        if c.prompt.is_some() || c.overlay.is_some() || c.popup.is_some() {
            return;
        }
        let x = m.x.max(0) as u16;
        let y = m.y.max(0) as u16;
        let buttons = m.buttons & 0x7;
        let prev = c.mouse_buttons;
        c.mouse_buttons = buttons;
        // The status line is drawn at this client's size, the panes at the
        // session's: positions in the window area are shifted by the view.
        let (crows, view_x, view_y) = (c.rows, c.view_x, c.view_y);
        if c.chooser.is_some() {
            // Wheel moves the selection; a click puts it on that line.
            let (cols, rows) = (c.cols, c.rows);
            let area = self.window_area(cols, rows);
            let ch = self.clients.get_mut(&cid).unwrap().chooser.as_mut().unwrap();
            if m.flags & MOUSE_WHEELED != 0 {
                ch.typed = None;
                ch.step(if (m.buttons >> 16) as i16 > 0 { -1 } else { 1 });
            } else if buttons & 1 != 0 && prev & 1 == 0 && y >= area.y && y + 1 < area.y + area.h {
                ch.typed = None;
                let i = ch.top + (y - area.y) as usize;
                if i < ch.items.len() {
                    ch.sel = i;
                }
            }
            return;
        }
        let shift = m.ctrl & crate::keys::SHIFT_PRESSED != 0;
        let alt = m.ctrl & (crate::keys::LEFT_ALT_PRESSED | crate::keys::RIGHT_ALT_PRESSED) != 0;
        let ctrl = m.ctrl & (crate::keys::LEFT_CTRL_PRESSED | crate::keys::RIGHT_CTRL_PRESSED) != 0;
        let wheel = if m.flags & MOUSE_WHEELED != 0 {
            let delta = (m.buttons >> 16) as i16;
            Some(delta > 0)
        } else {
            None
        };
        if m.flags & MOUSE_HWHEELED != 0 {
            return;
        }
        let pressed = buttons & !prev;
        let released = prev & !buttons;
        let motion = m.flags & MOUSE_MOVED != 0 && wheel.is_none();

        // Border drag in progress?
        if let Some(d) = c.drag.take()
            && buttons & BTN_LEFT != 0
        {
            let pos = if d.horizontal { (x + view_x) as i32 } else { (y + view_y) as i32 };
            let delta = pos - d.last;
            if delta != 0 {
                let dir = match (d.horizontal, delta > 0) {
                    (true, true) => Dir::Right,
                    (true, false) => Dir::Left,
                    (false, true) => Dir::Down,
                    (false, false) => Dir::Up,
                };
                if let Some(w) = self.session_mut(sid).and_then(|s| s.window_mut()) {
                    w.layout.resize(d.pane, dir, delta.unsigned_abs() as u16);
                }
                self.relayout_session(sid);
            }
            self.clients.get_mut(&cid).unwrap().drag = Some(Drag { pane: d.pane, horizontal: d.horizontal, last: pos });
            return;
        }

        // A selection being dragged: its pane has the mouse until the left
        // button goes up, wherever the pointer is (another pane, a border,
        // the status line); a wheel or another button meanwhile ends it.
        if let Some(sel) = self.clients.get(&cid).and_then(|c| c.selecting) {
            let release = released & BTN_LEFT != 0;
            if release || (buttons & BTN_LEFT != 0 && wheel.is_none()) {
                let at = (i32::from(m.x) + i32::from(view_x), i32::from(m.y) + i32::from(view_y));
                self.select_drag(cid, sid, sel.pane, at, release);
                return;
            }
            self.clients.get_mut(&cid).unwrap().selecting = None;
        }

        let status_y =
            if self.opts.status { Some(if self.opts.status_top { 0 } else { crows.saturating_sub(1) }) } else { None };
        if Some(y) == status_y {
            if pressed & BTN_LEFT != 0 {
                self.status_click(cid, sid, x);
            }
            return;
        }
        let (x, y) = (x.saturating_add(view_x), y.saturating_add(view_y));

        let mouse_opt = self.opts.mouse;
        let area = match self.session(sid) {
            Some(s) => self.window_area(s.cols, s.rows),
            None => return,
        };
        let Some(s) = self.session_mut(sid) else { return };
        let Some(w) = s.window_mut() else { return };
        let Some(pid) = w.pane_at(x, y) else {
            // On a border: start a drag from the pane whose edge this is.
            // The outer edge of the window is not a border.
            if pressed & BTN_LEFT != 0
                && mouse_opt
                && area.contains(x, y)
                && let Some((pane, horizontal)) = border_owner(&w.rects, x, y)
            {
                self.clients.get_mut(&cid).unwrap().drag =
                    Some(Drag { pane, horizontal, last: if horizontal { x as i32 } else { y as i32 } });
            }
            return;
        };
        let rect = w.rect_of(pid).unwrap();
        let (px, py) = (x - rect.x, y - rect.y);

        // Select the pane on any button press, and on the wheel: scrolling
        // a pane back puts it in copy mode, whose keys go to the active
        // pane (tmux's WheelUpPane binding selects the pane first too).
        if (pressed != 0 || wheel.is_some()) && mouse_opt && pid != w.active {
            let from = w.rect_of(w.active);
            w.last_pane = Some(w.active);
            w.active = pid;
            w.animate(from, Some(rect));
        }
        let pane = w.pane_mut(pid).unwrap();
        let app_mouse = pane.screen().mouse_protocol_mode();
        let in_copy = pane.copy.is_some();

        if app_mouse != vt100::MouseProtocolMode::None && !in_copy && !shift {
            use vt100::MouseProtocolMode as M;
            let btn_of = |b: u32| {
                if b & BTN_LEFT != 0 {
                    MouseButton::Left
                } else if b & BTN_MIDDLE != 0 {
                    MouseButton::Middle
                } else if b & BTN_RIGHT != 0 {
                    MouseButton::Right
                } else {
                    MouseButton::None
                }
            };
            let mut out = Vec::new();
            if let Some(up) = wheel {
                out.extend(input::wheel(pane.screen(), up, px, py, (shift, alt, ctrl)).unwrap_or_default());
            } else if pressed != 0 {
                out.extend(input::encode_mouse_sgr(btn_of(pressed), false, false, shift, alt, ctrl, px, py));
            } else if released != 0 {
                if app_mouse != M::Press {
                    out.extend(input::encode_mouse_sgr(btn_of(released), false, true, shift, alt, ctrl, px, py));
                }
            } else if motion {
                let report = match app_mouse {
                    M::AnyMotion => true,
                    M::ButtonMotion => buttons != 0,
                    _ => false,
                };
                if report {
                    out.extend(input::encode_mouse_sgr(btn_of(buttons), true, false, shift, alt, ctrl, px, py));
                }
            }
            if !out.is_empty() {
                pane.write_input(&out);
            }
            return;
        }

        if !mouse_opt {
            return;
        }

        if let Some(up) = wheel {
            if pane.screen().alternate_screen() && !in_copy {
                if let Some(b) = input::wheel(pane.screen(), up, px, py, (shift, alt, ctrl)) {
                    pane.write_input(&b);
                }
            } else {
                if !in_copy {
                    enter_copy_mode(pane);
                }
                copy_scroll(pane, if up { 3 } else { -3 });
                if pane.copy.as_ref().is_some_and(|c| c.offset == 0 && c.anchor.is_none()) && !up {
                    exit_copy_mode(pane);
                }
            }
            return;
        }
        // Right click pastes the clipboard, as the terminal itself would
        // were the mouse not ours (Windows Terminal, conhost); copy mode
        // ends first, the text is for the program, not the scrollback.
        if pressed & BTN_RIGHT != 0 {
            if in_copy {
                exit_copy_mode(pane);
            }
            if let Outcome::Error(e) =
                self.exec(Cmd::PasteBuffer { name: None, target: None, bracketed: true }, Some(cid))
            {
                self.message(cid, &e);
            }
            return;
        }
        if pressed & BTN_LEFT != 0 {
            if !in_copy {
                enter_copy_mode(pane);
            }
            let c = pane.copy.as_mut().unwrap();
            c.cx = px.min(pane.cols - 1);
            c.cy = py.min(pane.rows - 1);
            let abs = copy_abs(pane);
            let c = pane.copy.as_mut().unwrap();
            c.anchor = Some((abs, c.cx));
            c.dragging = true;
            // From now on the drag is this pane's (`select_drag`).
            self.clients.get_mut(&cid).unwrap().selecting = Some(Selecting { pane: pid, edge: 0, ticking: false });
        }
    }

    /// The pointer of a selection being dragged in pane `pid`, at `at`
    /// (window cells, maybe outside the pane); `release`: the button went
    /// up there. The selection ends where the pointer is, kept within the
    /// pane. At its top or bottom row, or past it, the pane scrolls a line
    /// now and every `DRAG_SCROLL` while the pointer stays (`drag_scroll`).
    /// Let go, the selection is copied, wherever the pointer is.
    fn select_drag(&mut self, cid: ClientId, sid: SessionId, pid: PaneId, at: (i32, i32), release: bool) {
        let Some(w) = self.session_mut(sid).and_then(|s| s.window_mut()) else {
            self.clients.get_mut(&cid).unwrap().selecting = None;
            return;
        };
        let (Some(rect), Some(pane)) = (w.rect_of(pid), w.pane_mut(pid)) else {
            self.clients.get_mut(&cid).unwrap().selecting = None;
            return;
        };
        if !pane.copy.as_ref().is_some_and(|c| c.dragging) {
            self.clients.get_mut(&cid).unwrap().selecting = None;
            return;
        }
        let (cols, rows) = (i32::from(pane.cols), i32::from(pane.rows));
        let (x, y) = (at.0 - i32::from(rect.x), at.1 - i32::from(rect.y));
        let c = pane.copy.as_mut().unwrap();
        c.cx = x.clamp(0, cols - 1) as u16;
        c.cy = y.clamp(0, rows - 1) as u16;
        if release {
            self.clients.get_mut(&cid).unwrap().selecting = None;
            self.finish_selection(cid, sid, pid);
            return;
        }
        let edge = if y <= 0 {
            1
        } else if y >= rows - 1 {
            -1
        } else {
            0
        };
        let sel = self.clients.get_mut(&cid).unwrap().selecting.as_mut().unwrap();
        let reached = edge != 0 && sel.edge == 0;
        sel.edge = edge;
        if reached {
            self.drag_scroll(cid);
        }
    }

    /// A selection held at its pane's edge: a line further that way, and
    /// again in `DRAG_SCROLL` while it is still there.
    fn drag_scroll(&mut self, cid: ClientId) {
        let Some(c) = self.clients.get(&cid) else { return };
        let (Some(sel), Some(sid)) = (c.selecting, c.session) else { return };
        if sel.edge == 0 {
            return;
        }
        let Some(pane) = self.session_mut(sid).and_then(|s| s.window_mut()).and_then(|w| w.pane_mut(sel.pane)) else {
            return;
        };
        copy_scroll(pane, i64::from(sel.edge));
        if !sel.ticking {
            if let Some(s) = self.clients.get_mut(&cid).and_then(|c| c.selecting.as_mut()) {
                s.ticking = true;
            }
            let tx = self.events.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DRAG_SCROLL).await;
                let _ = tx.send(Event::DragScroll(cid));
            });
        }
    }

    /// A drag let go: what it covers is copied (to a paste buffer and the
    /// clipboard), or, when it covers nothing (a plain click), copy mode
    /// just ends.
    fn finish_selection(&mut self, cid: ClientId, sid: SessionId, pid: PaneId) {
        let Some(pane) = self.session_mut(sid).and_then(|s| s.window_mut()).and_then(|w| w.pane_mut(pid)) else {
            return;
        };
        let Some(c) = pane.copy.as_mut() else { return };
        c.dragging = false;
        let (anchor, cx) = (c.anchor, c.cx);
        if anchor == Some((copy_abs(pane), cx)) {
            exit_copy_mode(pane);
            return;
        }
        match copy_selection(pane) {
            Ok((n, text, clipboard)) => {
                self.set_buffer(None, &text, false);
                let note = clipboard.map(|e| format!(" ({e})")).unwrap_or_default();
                self.message(cid, &format!("copied {n} characters{note}"));
            }
            Err(e) => self.message(cid, &e),
        }
        if let Some(p) = self.session_mut(sid).and_then(|s| s.window_mut()).and_then(|w| w.pane_mut(pid)) {
            exit_copy_mode(p);
        }
    }

    fn status_click(&mut self, cid: ClientId, sid: SessionId, x: u16) {
        // Hit ranges recorded by the last render of this client.
        let hit = self.clients.get(&cid).and_then(|c| c.window_hits.iter().position(|(a, b)| x >= *a && x < *b));
        if let Some(i) = hit
            && let Some(s) = self.session_mut(sid)
        {
            s.select_window(i);
            self.fire_hook("after-select-window", Some(cid));
        }
    }

    // ---------------------------------------------------------------- render

    /// Where the focus frame of session `spos`'s window is now, while it
    /// moves (`animation`).
    fn anim_frame(&self, spos: usize) -> Option<Rect> {
        if !self.opts.animation || self.opts.animation_time == 0 {
            return None;
        }
        let s = &self.sessions[spos];
        let w = s.windows.get(s.cur)?;
        let a = w.anim?;
        let t = a.start().elapsed().as_secs_f32() * 1000.0 / self.opts.animation_time as f32;
        if t >= 1.0 {
            return None;
        }
        let (from, to) = match a {
            Anim::Move { from, to, .. } => (from, to),
            // Drawn as the pane itself growing (`zoom_frame`), not as a frame.
            Anim::Zoom { .. } => return None,
            // Into a window: from all of it down to the active pane, or,
            // with nothing smaller to close in on, out from the middle.
            Anim::Enter { .. } => {
                let area = self.window_area(s.cols, s.rows);
                let to = w.rect_of(w.active)?;
                let from = if to == area || w.rects.len() < 2 {
                    Rect { x: area.x + area.w / 4, y: area.y + area.h / 4, w: area.w / 2, h: area.h / 2 }
                } else {
                    area
                };
                (from, to)
            }
        };
        let area = self.window_area(s.cols, s.rows);
        Some(render::frame_at(frame_around(from, area), frame_around(to, area), t))
    }

    /// A zoom in motion in session `spos`'s window: the pane zooming, the
    /// rectangle it has grown (or shrunk) to by now, and the panes as they
    /// are laid out without the zoom, which it grows over.
    fn zoom_frame(&self, spos: usize) -> Option<ZoomFrame> {
        if !self.opts.animation || self.opts.animation_time == 0 {
            return None;
        }
        let s = &self.sessions[spos];
        let w = s.windows.get(s.cur)?;
        let Some(Anim::Zoom { pane, from, to, start }) = w.anim else { return None };
        let t = start.elapsed().as_secs_f32() * 1000.0 / self.opts.animation_time as f32;
        if t >= 1.0 || w.pane(pane).is_none() {
            return None;
        }
        let area = self.window_area(s.cols, s.rows);
        Some(ZoomFrame {
            pane,
            rect: render::frame_at(from, to, t),
            layout: w.unzoomed_rects(area, self.border_rows()),
        })
    }

    /// Whether a window on some client's screen has its focus frame
    /// moving, so the next frame is due soon. A little past the end, so
    /// the frame after the last one takes it off the screen.
    fn animating(&self) -> bool {
        if !self.opts.animation || self.opts.animation_time == 0 {
            return false;
        }
        let last = Duration::from_millis(self.opts.animation_time) + FRAME * 2;
        self.clients
            .values()
            .filter_map(|c| c.session)
            .filter_map(|sid| self.session(sid).and_then(|s| s.windows.get(s.cur)))
            .any(|w| w.anim.is_some_and(|a| a.start().elapsed() < last))
    }

    /// Draw every attached client; true while an animation wants the next
    /// frame soon.
    fn render_all(&mut self) -> bool {
        let now = Instant::now();
        self.sweep_alerts();
        // display-time 0 means "until the next key press" (see handle_key).
        if self.opts.display_time_ms > 0 {
            let ttl = Duration::from_millis(self.opts.display_time_ms);
            for c in self.clients.values_mut() {
                if c.message.as_ref().is_some_and(|(_, t)| now.duration_since(*t) > ttl) {
                    c.message = None;
                }
            }
        }
        for c in self.clients.values_mut() {
            c.flush_pending();
        }
        let ids: Vec<ClientId> = self.clients.values().filter(|c| c.session.is_some()).map(|c| c.id).collect();
        for cid in ids {
            self.render_client(cid);
        }
        self.animating()
    }

    /// The status line of session `spos` as drawn for client `cid` (None:
    /// for no client, the phone page): `status-left`, each window's label
    /// (and whether it is the current one), `status-right`, expanded.
    fn status_parts(&mut self, spos: usize, cid: Option<ClientId>) -> StatusParts {
        let sid = self.sessions[spos].id;
        let base = render::Style::colors(self.opts.status_fg, self.opts.status_bg);
        let now = chrono::Local::now();
        // One context per window, built while nothing is borrowed
        // mutably; the status line's title is clipped so a long path
        // in it cannot push the window list off the line.
        let (cur, n) = (self.sessions[spos].cur, self.sessions[spos].windows.len());
        let ctxs: Vec<crate::format::Context> = (0..n)
            .map(|i| {
                let mut c = self.context(sid, i, None, cid);
                c.pane_title = truncate(&c.pane_title, 30);
                c
            })
            .collect();
        let cache = &mut self.shell_cache;
        let cur_ctx = ctxs.get(cur).cloned().unwrap_or_default();
        let mut left = crate::format::expand(&self.opts.status_left, &cur_ctx, cache, base, now);
        let mut right = crate::format::expand(&self.opts.status_right, &cur_ctx, cache, base, now);
        clip_segments(&mut left, self.opts.status_left_length);
        clip_segments(&mut right, self.opts.status_right_length);
        let windows = ctxs
            .iter()
            .enumerate()
            .map(|(i, ctx)| {
                let fmt =
                    if i == cur { &self.opts.window_status_current_format } else { &self.opts.window_status_format };
                (crate::format::expand(fmt, ctx, cache, base, now), i == cur)
            })
            .collect();
        (left, windows, right)
    }

    /// `status-line`: session `sid`'s status line as text, or (`json`) its
    /// parts: each segment's text and style, each window's label with its
    /// number, whether it is current and its active pane (the phone page
    /// opens that pane when the label is tapped).
    fn status_line_out(&mut self, sid: SessionId, json: bool) -> Outcome {
        let Some(spos) = self.sessions.iter().position(|s| s.id == sid) else {
            return Outcome::Error("no such session".into());
        };
        let (left, windows, right) = self.status_parts(spos, None);
        let text = |segs: &[crate::format::Segment]| segs.iter().map(|s| s.text.as_str()).collect::<String>();
        if !json {
            let list: Vec<String> = windows.iter().map(|(l, _)| text(l)).collect();
            let line = format!("{}{}  {}", text(&left), list.join(&self.opts.window_status_separator), text(&right));
            return Outcome::Text(line.trim_end().to_string());
        }
        fn colour(c: vt100::Color) -> serde_json::Value {
            match c {
                vt100::Color::Default => serde_json::Value::Null,
                vt100::Color::Idx(n) => n.into(),
                vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}").into(),
            }
        }
        let segs = |segs: &[crate::format::Segment]| -> serde_json::Value {
            segs.iter()
                .filter(|s| !s.text.is_empty())
                .map(|s| {
                    serde_json::json!({
                        "text": s.text, "fg": colour(s.style.fg), "bg": colour(s.style.bg),
                        "bold": s.style.bold, "dim": s.style.dim, "italic": s.style.italic,
                        "underline": s.style.underline, "inverse": s.style.inverse,
                    })
                })
                .collect()
        };
        let s = &self.sessions[spos];
        let base = self.opts.base_index;
        let list: Vec<serde_json::Value> = windows
            .iter()
            .enumerate()
            .map(|(i, (label, current))| {
                let pane = s.windows.get(i).map(|w| format!("%{}", w.active)).unwrap_or_default();
                serde_json::json!({ "index": base + i, "current": current, "pane": pane, "segments": segs(label) })
            })
            .collect();
        Outcome::Text(
            serde_json::json!({
                "session": s.name,
                "on": self.opts.status,
                "fg": colour(self.opts.status_fg),
                "bg": colour(self.opts.status_bg),
                "separator": self.opts.window_status_separator,
                "left": segs(&left),
                "windows": list,
                "right": segs(&right),
            })
            .to_string(),
        )
    }

    fn render_client(&mut self, cid: ClientId) {
        self.refit_popup(cid);
        let Some(c) = self.clients.get(&cid) else { return };
        let Some(sid) = c.session else { return };
        let (cols, rows) = (c.cols, c.rows);
        // A picker of live things (the tree, buffers, clients) is rebuilt every
        // render; the selection follows the item's identity, so things
        // appearing or vanishing meanwhile do not move it. A fixed list (the
        // find-window hits, a menu) is left alone.
        let fresh = match c.chooser.as_ref() {
            Some(ch) if ch.kind.live() => self.chooser_view(ch).map(|(i, l)| {
                let active = i.iter().map(|item| self.current_in_tree(item)).collect::<Vec<_>>();
                (i, l, active, ch.items.get(ch.sel).copied(), ch.sel)
            }),
            _ => None,
        };
        if let Some((items, lines, active, want, sel)) = fresh {
            let ch = self.clients.get_mut(&cid).unwrap().chooser.as_mut().unwrap();
            ch.active = active;
            ch.sel =
                want.and_then(|w| items.iter().position(|i| *i == w)).unwrap_or(sel.min(items.len().saturating_sub(1)));
            ch.items = items;
            ch.lines = lines;
            ch.settle(1); // a filter may have taken the selected line away
        }
        let Some(spos) = self.sessions.iter().position(|s| s.id == sid) else { return };
        let pane_base_index = self.opts.pane_base_index;
        let (status_top, border_fg, active_fg, opts_status) =
            (self.opts.status_top, self.opts.pane_border_fg, self.opts.pane_border_active_fg, self.opts.status);
        let (message, prompt) = {
            let c = self.clients.get(&cid).unwrap();
            (
                c.message.as_ref().map(|(m, _)| m.clone()),
                c.prompt
                    .as_ref()
                    .map(|p| (p.hint.clone().unwrap_or_else(|| p.label.clone()), p.input.clone(), p.cursor)),
            )
        };
        // (The alerts of the window being drawn were cleared by the sweep at
        // the top of render_all.)
        let mut bell = self.clients.get_mut(&cid).is_some_and(|c| std::mem::take(&mut c.pending_bell));
        // With `status off` there is no line for them: drawn over the bottom
        // row at the end instead (as tmux does).
        let notice = (!opts_status).then(|| (prompt.clone(), message.clone()));
        let status_line = if opts_status {
            let (left, windows, right) = self.status_parts(spos, Some(cid));
            Some(StatusLine {
                left,
                windows,
                separator: self.opts.window_status_separator.clone(),
                justify: render::Justify::parse(&self.opts.status_justify),
                right,
                message,
                prompt,
                fg: self.opts.status_fg,
                bg: self.opts.status_bg,
            })
        } else {
            None
        };
        self.refresh_status_shells();
        // `pane-border-status`: the text for each pane's border row, expanded
        // before the window is borrowed for drawing.
        let border_top = self.border_rows();
        let mut border_texts: HashMap<PaneId, Vec<crate::format::Segment>> = HashMap::new();
        if border_top.is_some() {
            let widx = self.sessions[spos].cur;
            let ids: Vec<PaneId> = self.sessions[spos]
                .windows
                .get(widx)
                .map(|w| w.rects.iter().map(|(id, _)| *id).collect())
                .unwrap_or_default();
            let now = chrono::Local::now();
            let active_id = self.sessions[spos].windows.get(widx).map(|w| w.active);
            for id in ids {
                let ctx = self.context(sid, widx, Some(id), Some(cid));
                let fg = if Some(id) == active_id { active_fg } else { border_fg };
                let base = render::Style::colors(fg, vt100::Color::Default);
                let segs = crate::format::expand(&self.opts.pane_border_format, &ctx, &mut self.shell_cache, base, now);
                border_texts.insert(id, segs);
            }
        }
        let sess_size = (self.sessions[spos].cols, self.sessions[spos].rows);
        // A zoom in motion: the window is drawn as it is without the zoom,
        // and the zooming pane over it at its size of the moment (below).
        let zooming = self.zoom_frame(spos);
        let stamps_on = self.opts.pane_timestamps && zooming.is_none();
        let s = &mut self.sessions[spos];
        let Some(w) = s.window_mut() else { return };
        let active = w.active;
        // Apply copy-mode scroll offsets for rendering and precompute the
        // copy views (they need mutable access to the scrollback).
        let mut copy_views: HashMap<PaneId, CopyView> = HashMap::new();
        // `pane-timestamps`: the commands on each pane's view, by row.
        let mut stamps: Vec<(PaneId, u16, pane::Mark)> = Vec::new();
        for p in &mut w.panes {
            if p.bell {
                p.bell = false;
                bell = true;
            }
            if stamps_on {
                let off = p.copy.as_ref().map_or(0, |c| c.offset);
                stamps.extend(p.visible_marks(off).into_iter().map(|(row, m)| (p.id, row, m)));
            }
            if p.copy.is_some() {
                let cur_abs = copy_abs(p);
                let cm = p.copy.as_ref().unwrap();
                let sel = copy_sel_view(p.rows, p.cols, cm, cur_abs);
                copy_views.insert(p.id, CopyView { cx: cm.cx, cy: cm.cy, sel, rect: cm.rect, offset: cm.offset });
                let off = cm.offset;
                p.parser.screen_mut().set_scrollback(off);
            }
        }
        let mut views = Vec::new();
        for (id, rect) in zooming.as_ref().map_or(&w.rects, |z| &z.layout) {
            let Some(p) = w.pane(*id) else { continue };
            views.push(PaneView {
                rect: *rect,
                screen: p.screen(),
                active: *id == active,
                copy: copy_views.get(id).copied(),
                border_text: border_texts.remove(id).zip(border_top),
            });
        }
        // A client the size of the session draws straight into its grid. A
        // client of another size (`window-size` gave the session someone
        // else's) draws the whole window at the session's size and shows
        // the part its view is on; the status line is its own width.
        let viewport = sess_size != (cols, rows);
        let (mut grid, mut cursor, window_hits, mut crop) = if !viewport {
            let frame = Frame {
                cols,
                rows,
                panes: views,
                status: status_line,
                status_top,
                border_fg,
                active_border_fg: active_fg,
            };
            let (g, c, h) = render::compose(&frame);
            (g, c, h, None)
        } else {
            let own = Frame {
                cols,
                rows,
                panes: Vec::new(),
                status: status_line,
                status_top,
                border_fg,
                active_border_fg: active_fg,
            };
            let (own_grid, _, hits) = render::compose(&own);
            let whole = Frame {
                cols: sess_size.0,
                rows: sess_size.1,
                panes: views,
                status: None,
                status_top,
                border_fg,
                active_border_fg: active_fg,
            };
            let (g, c, _) = render::compose(&whole);
            (g, c, hits, Some(own_grid))
        };
        for p in &mut w.panes {
            if p.copy.is_some() {
                p.parser.screen_mut().set_scrollback(0);
            }
        }
        // `pane-timestamps`: each command's time in the blank end of its
        // line, the short form when the long one does not fit, nothing when
        // neither does. Nothing moves: the pane keeps its width.
        if !stamps.is_empty() {
            let now = chrono::Local::now();
            let w = &self.sessions[spos].windows[self.sessions[spos].cur];
            for (id, row, m) in &stamps {
                let Some((_, rect)) = w.rects.iter().find(|(i, _)| i == id) else { continue };
                if !render::draw_stamp(&mut grid, *rect, *row, &stamp_parts(m, now, false)) {
                    render::draw_stamp(&mut grid, *rect, *row, &stamp_parts(m, now, true));
                }
            }
        }
        // `animation`: the focus frame on its way to where the keys now go,
        // or the zooming pane at its size of the moment, framed.
        let frame_style = render::Style {
            bold: true,
            ..render::Style::colors(self.opts.pane_border_active_fg, vt100::Color::Default)
        };
        if let Some(r) = self.anim_frame(spos) {
            render::draw_frame(&mut grid, r, frame_style);
        }
        if let Some(z) = &zooming {
            let w = &self.sessions[spos].windows[self.sessions[spos].cur];
            if let Some(p) = w.pane(z.pane) {
                grid.fill(z.rect, render::Style::default());
                grid.blit_screen(z.rect, p.screen());
                let area = self.window_area(sess_size.0, sess_size.1);
                render::draw_frame(&mut grid, frame_around(z.rect, area), frame_style);
            }
            cursor = None;
        }
        // `clock-mode`: a big clock over the panes that asked for one, and
        // under it what is worth a glance (`clock-mode-info`, `clock`).
        {
            let now = chrono::Local::now();
            let w = &self.sessions[spos].windows[self.sessions[spos].cur];
            let colour = self.opts.clock_mode_colour.unwrap_or(self.opts.pane_border_active_fg);
            for (id, rect) in w.rects.iter().filter(|(id, _)| w.pane(*id).is_some_and(|p| p.clock)) {
                let lines = match w.pane(*id) {
                    Some(p) if self.opts.clock_mode_info => self.clock_info(p, *id, now),
                    _ => Vec::new(),
                };
                render::draw_clock(&mut grid, *rect, now, self.opts.clock_mode_style, colour, &lines);
                cursor = None;
            }
        }
        // `display-panes`: each pane wears its number until the timer runs out.
        if self.clients.get(&cid).is_some_and(|c| c.panes_until.is_some_and(|t| Instant::now() < t)) {
            let w = &self.sessions[spos].windows[self.sessions[spos].cur];
            let order = w.layout.panes();
            // Zoomed, only one pane is on screen, but every pane can be
            // picked (the zoom goes with it): each number where its pane
            // would be, over the zoomed one.
            let mut real = Vec::new();
            if w.zoomed {
                w.layout.clone().layout(self.window_area(sess_size.0, sess_size.1), &mut real);
            }
            for (id, rect) in if w.zoomed { &real } else { &w.rects } {
                let n = order.iter().position(|p| p == id).unwrap_or(0) + pane_base_index;
                let colour = if *id == w.active {
                    self.opts.display_panes_active_colour
                } else {
                    self.opts.display_panes_colour
                };
                let p = w.pane(*id);
                let name = p.and_then(|p| p.actor.name.as_deref());
                let mode = p.map_or("", |p| p.actor.mode.as_str());
                render::draw_pane_number(&mut grid, *rect, n, colour, name, mode);
            }
            cursor = None;
        }
        // `hints`: each pick lit up with what is left of its label to type.
        if let Some(hm) = self.clients.get(&cid).and_then(|c| c.hints.as_ref()) {
            let w = &self.sessions[spos].windows[self.sessions[spos].cur];
            for (pid, f, label) in &hm.found {
                if let (Some(rest), Some(rect)) = (label.strip_prefix(hm.typed.as_str()), w.rect_of(*pid)) {
                    render::draw_hint(&mut grid, rect, f.row, f.col, f.width, rest);
                }
            }
            cursor = None;
        }
        let area = self.window_area(cols, rows);
        // The view: clamp it to the window, follow the active pane's cursor
        // unless it was panned by hand, and cut that part out of the whole.
        if let Some(mut own) = crop.take() {
            let whole_area = self.window_area(sess_size.0, sess_size.1);
            let c = self.clients.get_mut(&cid).unwrap();
            let max_x = whole_area.w.saturating_sub(area.w);
            let max_y = whole_area.h.saturating_sub(area.h);
            let (mut vx, mut vy) = (c.view_x.min(max_x), c.view_y.min(max_y));
            let seen = cursor.map(|(cx, cy)| (cx.saturating_sub(whole_area.x), cy.saturating_sub(whole_area.y)));
            let outside = seen.is_some_and(|(cx, cy)| {
                cx < vx || cy < vy || (area.w > 0 && cx >= vx + area.w) || (area.h > 0 && cy >= vy + area.h)
            });
            let now = Instant::now();
            if !outside {
                c.cursor_out = None;
            }
            let out_for = *c.cursor_out.get_or_insert(now);
            let long_enough = outside && now.duration_since(out_for) >= Duration::from_millis(150);
            let follow = !c.view_pinned && (c.view_follow || long_enough);
            c.view_follow = false;
            if follow && let Some((cx, cy)) = seen {
                if cx < vx {
                    vx = cx;
                } else if area.w > 0 && cx >= vx + area.w {
                    vx = cx + 1 - area.w;
                }
                if cy < vy {
                    vy = cy;
                } else if area.h > 0 && cy >= vy + area.h {
                    vy = cy + 1 - area.h;
                }
                vx = vx.min(max_x);
                vy = vy.min(max_y);
                c.cursor_out = None;
            }
            c.view_x = vx;
            c.view_y = vy;
            for y in 0..area.h {
                for x in 0..area.w {
                    let (sx, sy) = (whole_area.x + vx + x, whole_area.y + vy + y);
                    if sx < grid.cols && sy < grid.rows {
                        own.set(area.x + x, area.y + y, grid.get(sx, sy).clone());
                    }
                }
            }
            cursor = cursor.and_then(|(cx, cy)| {
                let (cx, cy) = (cx.checked_sub(whole_area.x + vx)?, cy.checked_sub(whole_area.y + vy)?);
                (cx < area.w && cy < area.h).then_some((area.x + cx, area.y + cy))
            });
            grid = own;
        }
        let c = self.clients.get_mut(&cid).unwrap();
        c.window_hits = window_hits;
        if let Some(ch) = c.chooser.as_mut() {
            let body_h = area.h.saturating_sub(1) as usize;
            if ch.sel < ch.top {
                ch.top = ch.sel;
            } else if body_h > 0 && ch.sel >= ch.top + body_h {
                ch.top = ch.sel + 1 - body_h;
            }
            let actions = match ch.kind {
                ChooserKind::Jobs => "Enter go  x kill  r restart  t tag  f filter",
                ChooserKind::Tree { style: TreeStyle::Chart, .. } if ch.all => {
                    "x kill  D delete  t tag  f filter  a branch  v tree"
                }
                ChooserKind::Tree { style: TreeStyle::Chart, .. } => "x kill  D delete  t tag  f filter  a all  v tree",
                ChooserKind::Tree { style: TreeStyle::Tree, .. } => {
                    "x kill  D delete  t tag  f filter  -/+ fold  v list"
                }
                ChooserKind::Tree { .. } => "x kill  D delete  f filter  v chart",
                ChooserKind::Clients => "Enter detach  f filter",
                ChooserKind::Buffers => "Enter paste  f filter",
                ChooserKind::History { .. } => "Enter open  + - fold",
                _ => "Enter select",
            };
            let mut status = Vec::new();
            if !ch.tagged.is_empty() {
                status.push(format!("[{} tagged]", ch.tagged.len()));
            }
            if !ch.filter.trim().is_empty() {
                status.push(format!("[filter: {}]", ch.filter.trim()));
            }
            if matches!(ch.kind, ChooserKind::Tree { style: TreeStyle::Chart, .. }) {
                let depths = ChooserItem::depths(&ch.items);
                let nodes: Vec<render::ChartNode> = ch
                    .items
                    .iter()
                    .zip(&ch.lines)
                    .enumerate()
                    .map(|(i, (item, line))| {
                        let (title, info) = line.split_once('\n').unwrap_or((line, ""));
                        let kind = match item {
                            ChooserItem::Host(_) => "host",
                            ChooserItem::Tree(_, None) | ChooserItem::Remote(_, _, None, _) => "session",
                            ChooserItem::Tree(_, Some(_)) | ChooserItem::Remote(_, _, Some(_), None) => "window",
                            ChooserItem::Job(..) | ChooserItem::Remote(..) => "pane",
                            _ => "",
                        };
                        render::ChartNode {
                            kind,
                            depth: depths[i].unwrap_or(0),
                            title: title.to_string(),
                            info: info.to_string(),
                            active: ch.active.get(i).copied().unwrap_or(false),
                        }
                    })
                    .collect();
                render::draw_chart(&mut grid, area, &nodes, ch.sel, ch.all, actions, &status.join(" "));
            } else if matches!(ch.kind, ChooserKind::Tree { style: TreeStyle::Tree, .. }) {
                let depths: Vec<usize> = ch.items.iter().map(|i| i.depth().unwrap_or(0)).collect();
                let rows: Vec<(String, String, usize)> = render::tree_leads(&depths)
                    .into_iter()
                    .zip(&ch.lines)
                    .zip(depths)
                    .map(|((lead, text), d)| (lead, text.clone(), d))
                    .collect();
                render::draw_tree(&mut grid, area, &rows, ch.sel, ch.top, actions, &status.join(" "));
            } else {
                render::draw_chooser(&mut grid, area, &ch.lines, ch.sel, ch.top, actions, &status.join(" "));
            }
            cursor = None;
        }
        if let Some((prompt, message)) = &notice
            && rows > 0
            && let Some(at) = render::draw_notice(&mut grid, rows - 1, prompt.as_ref(), message.as_deref())
        {
            cursor = Some(at);
        }
        // An overlay (a hook's message, a `run-shell` result) draws over the
        // picker, because the next key goes to the overlay, not the picker.
        if let Some(lines) = &c.overlay {
            render::draw_overlay(&mut grid, area, lines);
            cursor = None;
        }
        // The popup is the thing with the keyboard, so it draws last.
        if let Some(p) = c.popup.as_ref() {
            let hint = p.finished.then_some("press any key");
            cursor = render::draw_popup(&mut grid, p.rect, p.pane.screen(), active_fg, hint);
        }
        // The prefix waits for its next key, `prefix-hint-delay` already: what
        // each key does. Last, over a popup too: the next key is keepane's.
        if self.opts.prefix_hint
            && c.prefix
            && c.prefix_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(self.opts.prefix_hint_delay_ms))
        {
            static DEFAULTS: std::sync::OnceLock<HashMap<Key, Binding>> = std::sync::OnceLock::new();
            let groups = keyhint::groups(&self.prefix_binds, DEFAULTS.get_or_init(default_bindings));
            let footer = keyhint::footer(&self.prefix_binds);
            render::draw_key_hint(&mut grid, area, &self.opts.prefix.to_string(), &groups, &footer);
            cursor = None;
        }
        // The theme's colours, over everything drawn.
        let bar = (opts_status && rows > 1).then(|| if status_top { 0 } else { rows - 1 });
        render::recolor(&mut grid, bar, self.opts.window_fg, self.opts.window_bg, &self.opts.pane_colours);
        let changed = c.last_grid.as_ref() != Some(&grid) || c.last_cursor != cursor;
        if !changed && !bell {
            return;
        }
        let mut bytes = render::diff(c.last_grid.as_ref(), &grid, cursor);
        if bell {
            bytes.insert(0, 0x07);
        }
        c.last_grid = Some(grid);
        c.last_cursor = cursor;
        c.send_output(bytes);
    }
}

/// The lines of a config file the way tmux reads them: a `\` at the end of
/// a line continues it on the next, and `%if` / `%elif` / `%else` /
/// `%endif` pick branches by their condition, which `eval` decides (a
/// format, as in tmux). Each logical line carries the
/// number of its first physical line, for error messages. The second value
/// is the line of a `%if` that was never closed, which swallows everything
/// after it and must be said rather than silently dropped.
fn logical_lines(text: &str, eval: &dyn Fn(&str) -> bool) -> (Vec<(usize, String)>, Option<usize>) {
    let mut out = Vec::new();
    let mut joined = String::new();
    let mut start = 0;
    // One frame per open %if: whether its current branch is being read,
    // whether any branch of it has been taken yet, and where it began.
    struct Frame {
        active: bool,
        taken: bool,
        opened_at: usize,
    }
    let mut frames: Vec<Frame> = Vec::new();
    // Notepad writes a byte-order mark; without this the first command
    // would be "\u{feff}set", which is nothing.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let trimmed = raw.trim_start();
        let reading = frames.last().is_none_or(|f| f.active);
        if trimmed.starts_with('%') {
            // %if / %elif / %else / %endif, as tmux reads them: the
            // condition is a format, true when it expands to something
            // other than nothing or "0". %hidden is not ours.
            let (word, cond) = match trimmed.split_once(char::is_whitespace) {
                Some((w, c)) => (w, c.trim()),
                None => (trimmed, ""),
            };
            match word {
                "%if" => {
                    let active = reading && eval(cond);
                    frames.push(Frame { active, taken: active, opened_at: n });
                }
                "%elif" => {
                    if !frames.is_empty() {
                        let parent = frames.len() < 2 || frames[frames.len() - 2].active;
                        let f = frames.last_mut().unwrap();
                        f.active = parent && !f.taken && eval(cond);
                        f.taken |= f.active;
                    }
                }
                "%else" => {
                    if !frames.is_empty() {
                        let parent = frames.len() < 2 || frames[frames.len() - 2].active;
                        let f = frames.last_mut().unwrap();
                        f.active = parent && !f.taken;
                        f.taken = true;
                    }
                }
                "%endif" => {
                    frames.pop();
                }
                _ => {}
            }
            joined.clear();
            continue;
        }
        if !reading {
            continue;
        }
        if joined.is_empty() {
            start = n;
        }
        if let Some(head) = raw.strip_suffix('\\') {
            joined.push_str(head);
            continue;
        }
        joined.push_str(raw);
        out.push((start, std::mem::take(&mut joined)));
    }
    if !joined.is_empty() {
        out.push((start, joined));
    }
    (out, frames.first().map(|f| f.opened_at))
}

/// `i` as Unix seconds, counted from one wall-clock reading the process
/// takes once: the same instant always gives the same second, so a
/// difference of two (how long a dead pane ran) does not change from one
/// asking to the next as each is rounded at another moment.
fn unix_seconds(i: Instant) -> i64 {
    static ANCHOR: std::sync::OnceLock<(Instant, f64)> = std::sync::OnceLock::new();
    let (at, secs) = *ANCHOR.get_or_init(|| (Instant::now(), chrono::Utc::now().timestamp_millis() as f64 / 1000.0));
    let since = if i >= at { (i - at).as_secs_f64() } else { -(at - i).as_secs_f64() };
    (secs + since).floor() as i64
}

/// Expand a leading `~` or `~/` to the user's home directory.
fn expand_home(path: &str) -> String {
    if (path == "~" || path.starts_with("~/") || path.starts_with("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return format!("{}{}", home.display(), &path[1..]);
    }
    path.to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).chain(std::iter::once('…')).collect()
    }
}

/// Truncate a segment list to `max` display cells (tmux `status-*-length`).
fn clip_segments(segs: &mut Vec<crate::format::Segment>, max: usize) {
    use unicode_width::UnicodeWidthChar;
    let mut left = max;
    for s in segs.iter_mut() {
        let mut keep = String::new();
        for ch in s.text.chars() {
            let w = ch.width().unwrap_or(0);
            if w > left {
                break;
            }
            left -= w;
            keep.push(ch);
        }
        s.text = keep;
    }
    segs.retain(|s| !s.text.is_empty());
}

/// PATH with the directory of the running executable prepended (once).
fn path_with_self() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return path;
    };
    let already = std::env::split_paths(&path).any(|p| p == dir);
    if already {
        return path;
    }
    // The platform's separator (`;` on Windows, `:` elsewhere).
    let parts = std::iter::once(dir).chain(std::env::split_paths(&path));
    std::env::join_paths(parts).map(|p| p.to_string_lossy().into_owned()).unwrap_or(path)
}

/// Longest a status-line `#(command)` may run before it is killed.
/// The columns of the task board (`jobs`, `choose-jobs`).
fn jobs_header() -> Vec<String> {
    ["PANE", "STATE", "UP", "IDLE", "PID", "COMMAND", "DIR"].iter().map(|s| s.to_string()).collect()
}

/// Rows as lines with the columns lined up on screen: padded by display
/// width, since a CJK session name is two cells a character.
fn align_columns(rows: &[Vec<String>]) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    let ncol = rows.first().map(|r| r.len()).unwrap_or(0);
    let widths: Vec<usize> =
        (0..ncol).map(|c| rows.iter().map(|r| r.get(c).map(|s| s.width()).unwrap_or(0)).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                line.push_str(cell);
                if c + 1 < ncol {
                    line.extend(std::iter::repeat_n(' ', widths[c].saturating_sub(cell.width()) + 2));
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// tmux's names for mouse events as keys (`MouseDown1Pane`,
/// `MouseDragEnd1Pane`, `WheelUpPane`, `DoubleClick1Pane`, ...).
fn is_mouse_key(name: &str) -> bool {
    let n = name.trim_start_matches("C-").trim_start_matches("M-").trim_start_matches("S-");
    ["Mouse", "Wheel", "DoubleClick", "TripleClick", "SecondClick"].iter().any(|p| n.starts_with(p))
}

/// Which picker lines a filter keeps: those holding it, case-insensitively,
/// plus title lines. In the tree a session line stays when one of its
/// windows matches, and its windows stay when the session line does, so
/// filtering by a session name shows the whole session.
fn chooser_filter(items: &[ChooserItem], lines: &[String], filter: &str) -> Vec<bool> {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return vec![true; items.len()];
    }
    let hit: Vec<bool> = lines.iter().map(|l| l.to_lowercase().contains(&needle)).collect();
    let mut keep = hit.clone();
    // The session, window (and pane) lines above this one in the tree: a
    // line stays when one of them matches, and they stay when it does. A
    // separator (the tree's root, the board's header) always stays and is
    // no one's parent.
    // (Depths as the tree has them: under a row of machines in the chart.)
    let depths = ChooserItem::depths(items);
    let mut above: Vec<(usize, usize)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let depth = match item {
            ChooserItem::Separator => {
                keep[i] = true;
                continue;
            }
            ChooserItem::Tree(..) | ChooserItem::Job(..) | ChooserItem::Host(_) | ChooserItem::Remote(..) => {
                depths[i].unwrap_or(0)
            }
            _ => continue,
        };
        while above.last().is_some_and(|&(d, _)| d >= depth) {
            above.pop();
        }
        for &(_, a) in &above {
            keep[i] |= hit[a];
            keep[a] |= hit[i];
        }
        above.push((depth, i));
    }
    keep
}

/// How long after a session ends by itself its save is forgotten: long
/// enough for a shutdown that killed its shells to say so first.
const FORGET_AFTER: Duration = Duration::from_secs(10);
/// How long after a shutdown is announced nothing is forgotten: the
/// machine is expected to be gone by then; if it is not, it was called off.
const ENDING_HOLDS: Duration = Duration::from_secs(60);

const STATUS_SHELL_TIMEOUT: Duration = Duration::from_secs(30);
/// A `#(command)` run for a one-shot `display-message -p` or `jobs -F`
/// blocks the server while it runs, so it gets less rope than the status
/// line's background ones.
const ONE_SHOT_SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Run a shell command to completion, returning (stdout+stderr, exit code).
/// Uses the platform's shell (`platform::process::shell_command`). With a
/// `timeout` the process tree is killed when it expires (exit code 124).
fn run_shell_blocking(command: &str, env: &[(String, String)], timeout: Option<Duration>) -> (String, i32) {
    let (mut c, exe) = crate::platform::process::shell_command(command);
    c.stdin(std::process::Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return (format!("{exe}: {e}"), 127),
    };
    // Killing the tree takes grandchildren too.
    let mut job = crate::platform::process::Tree::of(&child).ok();
    // Watchdog: close the job (killing the tree) when the timeout expires.
    // Without a timeout the job simply lives until the command is done.
    let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(t) = timeout
        && let Some(j) = job.take()
    {
        let (timed_out, done) = (timed_out.clone(), done.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + t;
            while Instant::now() < deadline {
                if done.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(j);
        });
    }
    let out = child.wait_with_output();
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(job);
    match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            if timed_out.load(std::sync::atomic::Ordering::SeqCst) {
                return (format!("{text}\n[timed out after {}s]", timeout.map_or(0, |t| t.as_secs())), 124);
            }
            (text, o.status.code().unwrap_or(1))
        }
        Err(e) => (format!("{exe}: {e}"), 127),
    }
}

fn char_index(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(s.len())
}

/// Which pane's trailing edge a border cell belongs to, and whether that
/// edge is vertical (horizontal drag).
fn border_owner(rects: &[(PaneId, Rect)], x: u16, y: u16) -> Option<(PaneId, bool)> {
    for (id, r) in rects {
        if r.x + r.w == x && y >= r.y && y < r.y + r.h {
            return Some((*id, true));
        }
        if r.y + r.h == y && x >= r.x && x < r.x + r.w {
            return Some((*id, false));
        }
    }
    None
}

/// `WheelUp` / `WheelDown` among `send-keys`' words: which way.
fn wheel_word(k: &str) -> Option<bool> {
    match k {
        "WheelUp" => Some(true),
        "WheelDown" => Some(false),
        _ => None,
    }
}

fn enter_copy_mode(p: &mut Pane) {
    if p.copy.is_none() {
        let (row, col) = p.screen().cursor_position();
        p.copy = Some(CopyMode {
            offset: 0,
            cx: col.min(p.cols.saturating_sub(1)),
            cy: row.min(p.rows.saturating_sub(1)),
            anchor: None,
            dragging: false,
            search: None,
            search_back: true,
            count: None,
            rect: false,
        });
    }
}

/// Move the copy cursor to an absolute line, scrolling if it is off screen.
fn copy_goto(p: &mut Pane, abs: usize) {
    let max = p.scrollback_len();
    let rows = p.rows as usize;
    let Some(c) = p.copy.as_ref() else { return };
    let top = max - c.offset; // absolute line at the top of the view
    let (offset, cy) = if abs < top {
        (max - abs, 0)
    } else if abs >= top + rows {
        ((max + rows - 1).saturating_sub(abs), rows - 1)
    } else {
        (c.offset, abs - top)
    };
    if let Some(c) = p.copy.as_mut() {
        c.offset = offset.min(max);
        c.cy = cy.min(rows.saturating_sub(1)) as u16;
    }
}

/// vi's `w`, `b` and `e` (and their WORD variants) on the copy cursor.
fn copy_word_motion(p: &mut Pane, key: char) {
    let cols = p.cols as usize;
    let big = key.is_uppercase();
    let class = |ch: char| -> u8 {
        if ch.is_whitespace() {
            0
        } else if big || ch.is_alphanumeric() || ch == '_' {
            1
        } else {
            2
        }
    };
    // A row as its columns, each the character drawn there (a wide one in
    // both of its): the cursor moves a column at a time, as it is drawn.
    let columns = |p: &mut Pane, abs: usize| -> Vec<char> {
        let text = p.line_text(abs).0;
        let chars: Vec<char> = text.chars().collect();
        let mut row: Vec<char> = pane::column_spans(&text).iter().map(|s| chars[s.start]).collect();
        row.resize(cols.max(1), ' ');
        row
    };
    let abs = copy_abs(p);
    let last = p.scrollback_len() + p.rows as usize - 1;
    let mut line = columns(p, abs);
    let mut x = p.copy.as_ref().map(|c| c.cx as usize).unwrap_or(0).min(cols.saturating_sub(1));
    let mut cur = abs;
    let forward = key != 'b' && key != 'B';
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > 4096 {
            break;
        }
        let moved = if forward {
            if x + 1 < line.len() {
                x += 1;
                true
            } else if cur < last {
                cur += 1;
                line = columns(p, cur);
                x = 0;
                true
            } else {
                false
            }
        } else if x > 0 {
            x -= 1;
            true
        } else if cur > 0 {
            cur -= 1;
            line = columns(p, cur);
            x = line.len().saturating_sub(1);
            true
        } else {
            false
        };
        if !moved {
            break;
        }
        let here = class(line[x]);
        if here == 0 {
            continue;
        }
        // `w`/`b` stop at the start of a word, `e` at its end.
        let neighbour = match key {
            'e' | 'E' => line.get(x + 1).copied().map(class).unwrap_or(0),
            _ if x == 0 => 0,
            _ => class(line[x - 1]),
        };
        if neighbour != here {
            break;
        }
    }
    if cur != abs {
        copy_goto(p, cur);
    }
    if let Some(c) = p.copy.as_mut() {
        c.cx = x.min(cols.saturating_sub(1)) as u16;
    }
}

/// A `-x`/`-y` size: cells, or a percentage of the window.
/// Where a `display-popup` box goes: the asked-for size (`80`, `50%`),
/// defaulting to four fifths of the window, at the asked-for position
/// (`-x`/`-y`: a column or row, a percentage of the window, `C` centred,
/// `R`/`B` against the right or bottom edge), centred when not given. A
/// box that would stick out is pulled back in.
fn popup_rect(area: Rect, width: Option<&str>, height: Option<&str>, x: Option<&str>, y: Option<&str>) -> Rect {
    let size = |spec: Option<&str>, full: u16| -> u16 {
        spec.and_then(|s| parse_size(s, full)).unwrap_or((full as u32 * 4 / 5) as u16).clamp(1, full)
    };
    let w = size(width, area.w);
    let h = size(height, area.h);
    let place = |spec: Option<&str>, full: u16, size: u16| -> u16 {
        let room = full - size;
        let at = match spec.map(str::trim) {
            None | Some("C") | Some("c") => room / 2,
            Some("R") | Some("r") | Some("B") | Some("b") => room,
            Some(s) if s.ends_with('%') => parse_size(s, full).unwrap_or(room / 2),
            Some(s) => s.parse::<u16>().unwrap_or(room / 2),
        };
        at.min(room)
    };
    Rect { x: area.x + place(x, area.w, w), y: area.y + place(y, area.h, h), w, h }
}

fn parse_size(spec: &str, full: u16) -> Option<u16> {
    let spec = spec.trim();
    if let Some(p) = spec.strip_suffix('%') {
        let pct: u32 = p.trim().parse().ok()?;
        return Some(((full as u32 * pct.min(100)) / 100).max(1) as u16);
    }
    spec.parse::<u16>().ok().map(|v| v.max(1))
}

/// One line of a buffer for a listing, clipped to `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
    let flat = flat.trim().to_string();
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

fn exit_copy_mode(p: &mut Pane) {
    p.copy = None;
    p.parser.screen_mut().set_scrollback(0);
}

/// Scroll the copy-mode view by `delta` lines (positive = older).
fn copy_scroll(p: &mut Pane, delta: i64) {
    let max = p.scrollback_len() as i64;
    if let Some(c) = p.copy.as_mut() {
        c.offset = (c.offset as i64 + delta).clamp(0, max) as usize;
    }
}

/// What `pane-timestamps` writes at the end of a command's line: when it
/// started (the date too when not today), how long it took and how it
/// ended. `short` leaves only the time and the ending, for a narrow line.
fn stamp_parts(m: &pane::Mark, now: chrono::DateTime<chrono::Local>, short: bool) -> Vec<(String, render::Style)> {
    let gray = render::Style::colors(vt100::Color::Idx(8), vt100::Color::Default);
    let Some(end) = m.end else { return Vec::new() };
    let at = m.start.unwrap_or(end);
    let when = if at.date_naive() == now.date_naive() { at.format("%H:%M:%S") } else { at.format("%m-%d %H:%M:%S") };
    let mut parts = vec![(when.to_string(), gray)];
    if !short && let Some(start) = m.start {
        parts.push((format!(" {}", crate::format::command_duration((end - start).num_milliseconds())), gray));
    }
    match m.exit {
        Some(0) => parts.push((" ✓".into(), render::Style::colors(vt100::Color::Idx(2), vt100::Color::Default))),
        Some(code) => {
            let tail = if code == 1 { String::new() } else { code.to_string() };
            parts.push((format!(" ✗{tail}"), render::Style::colors(vt100::Color::Idx(1), vt100::Color::Default)));
        }
        None => {}
    }
    parts
}

/// A zoom in motion, as one frame draws it (`Server::zoom_frame`).
struct ZoomFrame {
    /// The pane zooming in or out.
    pane: PaneId,
    /// Where it is drawn now, between its place and the whole window.
    rect: Rect,
    /// The other panes' places without the zoom, drawn beneath it.
    layout: Vec<(PaneId, Rect)>,
}

/// The rectangle a focus frame takes around `r`: on the borders around a
/// pane, where there are any, rather than on its text; at the window's
/// edge, just inside it.
fn frame_around(r: Rect, area: Rect) -> Rect {
    let x = r.x.saturating_sub(1).max(area.x);
    let y = r.y.saturating_sub(1).max(area.y);
    let x1 = (r.x + r.w + 1).min(area.x + area.w);
    let y1 = (r.y + r.h + 1).min(area.y + area.h);
    Rect { x, y, w: x1.saturating_sub(x), h: y1.saturating_sub(y) }
}

/// A size as people read it: `812 B`, `14 KB`, `1.2 MB`.
fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// The history picker's lines: each pane position with a log, and under
/// an open one its days, newest first.
fn history_lines(kept: &[crate::histlog::Kept], open: &HashSet<usize>) -> (Vec<ChooserItem>, Vec<String>) {
    let today = chrono::Local::now().date_naive();
    let (mut items, mut lines) = (Vec::new(), Vec::new());
    for (i, k) in kept.iter().enumerate() {
        let is_open = open.contains(&i);
        let n = k.days.len();
        items.push(ChooserItem::Log(i, None));
        lines.push(format!(
            "{} {}:{}  {}, newest {}",
            if is_open { "-" } else { "+" },
            k.session,
            k.key,
            crate::format::count(n, "day"),
            k.days.first().map(|d| d.0.to_string()).unwrap_or_default()
        ));
        if !is_open {
            continue;
        }
        for (d, (day, _, size)) in k.days.iter().enumerate() {
            let when = match (today - *day).num_days() {
                0 => "  today",
                1 => "  yesterday",
                _ => "",
            };
            items.push(ChooserItem::Log(i, Some(d)));
            lines.push(format!("    {day} {}  {:>8}{when}", day.format("%a"), human_size(*size)));
        }
    }
    (items, lines)
}

/// `list-marks`: one line per finished command still readable, oldest
/// first: its row counted from the top of the screen (negative in the
/// scrollback, as `capture-pane -S` counts), start and end in Unix
/// milliseconds, the exit code (`-` when the shell did not say) and the
/// line's text as it reads now (cut short in a pane narrowed since).
fn list_marks(p: &mut Pane) -> Vec<String> {
    let top = p.screen().scrolled_total() as i64;
    let marks: Vec<pane::Mark> = p.marks.iter().filter(|m| m.end.is_some()).cloned().collect();
    let mut out = Vec::new();
    for m in marks {
        let Some(now) = p.text_at(m.line).filter(|now| pane::still_reads(now, &m.text, p.cols)) else { continue };
        let ms = |t: Option<chrono::DateTime<chrono::Local>>| {
            t.map(|t| t.timestamp_millis().to_string()).unwrap_or("-".into())
        };
        out.push(format!(
            "{} {} {} {} {}",
            m.line as i64 - top,
            ms(m.start),
            ms(m.end),
            m.exit.map(|c| c.to_string()).unwrap_or("-".into()),
            now
        ));
    }
    out
}

/// `list-marks -J`: the same marks as `list_marks`, as JSON, each with the
/// lines it covers, counted as `row` is (0 = the screen's first line,
/// negative in the scrollback): `last`, where what was typed ends (it may
/// wrap), and `stop`, the line what it printed stops before (the next
/// prompt's first, which may be above that prompt's own `row`); and
/// `command`, all of what was typed, the prompt left out.
fn marks_json(p: &mut Pane) -> serde_json::Value {
    let top = p.screen().scrolled_total() as i64;
    let marks: Vec<pane::Mark> = p.marks.iter().filter(|m| m.end.is_some()).cloned().collect();
    let ms = |t: Option<chrono::DateTime<chrono::Local>>| t.map(|t| t.timestamp_millis());
    let mut out = Vec::new();
    for m in marks {
        let Some(now) = p.text_at(m.line).filter(|now| pane::still_reads(now, &m.text, p.cols)) else { continue };
        let (last, stop) = p.span_of(&m);
        let command = p.command_at(&m, last);
        out.push(serde_json::json!({
            "row": m.line as i64 - top,
            "start": ms(m.start),
            "end": ms(m.end),
            "exit": m.exit,
            "last": last as i64 - top,
            "stop": stop as i64 - top,
            "text": now,
            "command": command,
        }));
    }
    serde_json::Value::Array(out)
}

/// The most of a command's output `copy-output` takes.
const OUTPUT_MAX: usize = 4 << 20;

/// Absolute line number under the copy-mode cursor. The scrollback can shrink
/// under us (resize, clear-history), so the offset is re-clamped here.
fn copy_abs(p: &mut Pane) -> usize {
    let max = p.scrollback_len();
    let rows = p.rows;
    let c = p.copy.as_mut().unwrap();
    c.offset = c.offset.min(max);
    c.cy = c.cy.min(rows.saturating_sub(1));
    max - c.offset + c.cy as usize
}

/// Selection as inclusive screen coordinates for rendering. `cursor_abs` is
/// the absolute line under the copy cursor.
fn copy_sel_view(rows: u16, cols: u16, cm: &CopyMode, cursor_abs: usize) -> Option<((u16, u16), (u16, u16))> {
    let (abs_a, col_a) = cm.anchor?;
    let ay = cm.cy as i64 + (abs_a as i64 - cursor_abs as i64);
    let rows = rows as i64;
    let mut a = (ay, col_a as i64);
    let mut b = (cm.cy as i64, cm.cx as i64);
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    if b.0 < 0 || a.0 >= rows {
        return None;
    }
    let (sy, sx) = if a.0 < 0 { (0, 0) } else { (a.0 as u16, a.1 as u16) };
    let (ey, ex) = if b.0 >= rows { ((rows - 1) as u16, cols.saturating_sub(1)) } else { (b.0 as u16, b.1 as u16) };
    Some(((sy, sx), (ey, ex)))
}

/// Copy the selected text to the clipboard; returns the character count.
/// Returns the number of characters copied and the text, which the caller
/// also stores as a paste buffer.
fn copy_selection(p: &mut Pane) -> Result<(usize, String, Option<String>), String> {
    let text = selected_text(p)?;
    let n = text.chars().count();
    // The clipboard can be busy (another program holding it, a clipboard
    // manager reacting to the last change): the paste buffer is set either
    // way, and the trouble is reported beside the count.
    let clipboard = crate::clipboard::set_text(&text).err().map(|e| format!("clipboard: {e}"));
    Ok((n, text, clipboard))
}

/// What copy mode's selection covers: from the anchor to the cursor (or
/// the rectangle between them), lines joined where the terminal wrapped
/// them, blanks at their ends dropped.
fn selected_text(p: &mut Pane) -> Result<String, String> {
    let cur_abs = copy_abs(p);
    let c = p.copy.as_ref().unwrap();
    let Some((abs_a, col_a)) = c.anchor else { return Err("no selection".into()) };
    let mut a = (abs_a, col_a);
    let mut b = (cur_abs, c.cx);
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    let rect = p.copy.as_ref().is_some_and(|c| c.rect);
    let (rx0, rx1) = (a.1.min(b.1) as usize, a.1.max(b.1) as usize);
    let mut text = String::new();
    for abs in a.0..=b.0 {
        let (line, wrapped) = p.line_text(abs);
        let chars: Vec<char> = line.chars().collect();
        // The selection's ends are columns; what is copied is the
        // characters drawn in them (a wide character in its two).
        let spans = pane::column_spans(&line);
        let from = |col: usize| spans.get(col).map_or(chars.len(), |s| s.start);
        let upto = |col: usize| spans.get(col).map_or(chars.len(), |s| s.end);
        // A rectangle takes the same columns from every line.
        let start = if rect {
            from(rx0)
        } else if abs == a.0 {
            from(a.1 as usize)
        } else {
            0
        };
        let end = if rect {
            upto(rx1)
        } else if abs == b.0 {
            upto(b.1 as usize)
        } else {
            chars.len()
        };
        let seg: String = if start < end { chars[start..end].iter().collect() } else { String::new() };
        if abs == b.0 {
            text.push_str(seg.trim_end());
        } else {
            text.push_str(seg.trim_end());
            if !wrapped {
                text.push('\n');
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A selection copies the characters drawn in the columns it covers:
    /// wide ones (Chinese, an emoji) two columns each, an accent with its
    /// letter, half of a wide character as all of it; over lines and as a
    /// rectangle; plain text as ever.
    #[test]
    fn a_selection_copies_the_columns_it_covers() {
        let argv: Vec<String> = if cfg!(windows) {
            ["cmd.exe", "/c", "exit"].map(String::from).to_vec()
        } else {
            ["/bin/sh", "-c", "exit"].map(String::from).to_vec()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut p = Pane::spawn(1, &argv, None, 40, 6, 100, &[], tx).unwrap();
        p.process_output("plain text here\r\n中文 ab😀cd e\u{301}f\r\n宽字一\r\n宽字二".as_bytes());
        // (row, column) of the anchor and of the cursor, the rectangle or not.
        let mut pick = |a: (usize, u16), b: (usize, u16), rect: bool| {
            enter_copy_mode(&mut p);
            let c = p.copy.as_mut().unwrap();
            c.offset = 0;
            (c.cy, c.cx) = (b.0 as u16, b.1);
            c.anchor = Some(a);
            c.rect = rect;
            let got = selected_text(&mut p).unwrap();
            exit_copy_mode(&mut p);
            got
        };
        assert_eq!(pick((0, 6), (0, 9), false), "text", "plain: a column a character, as before");
        // 中文 is columns 0-3, ` ` 4, ab 5-6, 😀 7-8, cd 9-10, ` ` 11, é 12, f 13.
        assert_eq!(pick((1, 5), (1, 10), false), "ab😀cd");
        assert_eq!(pick((1, 1), (1, 2), false), "中文", "half of each: all of each");
        assert_eq!(pick((1, 8), (1, 8), false), "😀", "the emoji's second column");
        assert_eq!(pick((1, 12), (1, 13), false), "e\u{301}f", "an accent with its letter");
        assert_eq!(pick((1, 5), (2, 1), false), "ab😀cd e\u{301}f\n宽", "over two lines");
        assert_eq!(pick((2, 2), (3, 3), true), "字\n字", "a rectangle: the same columns of each line");
        assert_eq!(pick((1, 30), (1, 39), false), "", "past the end of the text: nothing");
    }

    /// An ask on a thread answers, even when it panics: None then.
    #[test]
    fn an_ask_on_a_thread_always_answers() {
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        ask_on_thread(&tx, || Some("203.0.113.7".to_string()), Event::PublicIp);
        assert!(matches!(rx.blocking_recv(), Some(Event::PublicIp(Some(ip))) if ip == "203.0.113.7"));
        ask_on_thread(&tx, || -> Option<String> { panic!("a bug in the ask") }, Event::PublicIp);
        assert!(matches!(rx.blocking_recv(), Some(Event::PublicIp(None))), "a panic answers None");
    }

    #[test]
    fn old_shell_history_files_go_unless_something_refers_to_them() {
        let dir = std::env::temp_dir().join(format!("keepane-prune-shell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["old", "kept", "new", "notes"] {
            std::fs::write(dir.join(format!("{name}.txt")), "x").unwrap();
        }
        std::fs::write(dir.join("old.log"), "x").unwrap();
        let keep: HashSet<String> = ["kept".to_string()].into_iter().collect();
        // A cutoff in the past: nothing is that old, nothing goes.
        prune_shell_history_files(&dir, &keep, SystemTime::now() - Duration::from_secs(3600));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 5);
        // Everything is older than a cutoff in the future: what is not kept goes,
        // and only .txt files.
        prune_shell_history_files(&dir, &keep, SystemTime::now() + Duration::from_secs(3600));
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["kept.txt", "old.log"]);
        // No directory at all: nothing happens.
        prune_shell_history_files(&dir.join("missing"), &keep, SystemTime::now());
        let _ = std::fs::remove_dir_all(&dir);
    }
    fn client(cap: usize) -> (Client, mpsc::Receiver<ServerMsg>) {
        let (tx, rx) = mpsc::channel(cap);
        let c = Client {
            id: 1,
            tx,
            cols: 10,
            rows: 2,
            cwd: String::new(),
            interactive: true,
            pane_env: None,
            session: Some(1),
            last_session: None,
            last_grid: Some(Grid::new(10, 2)),
            last_cursor: Some((0, 0)),
            prefix: false,
            prefix_at: None,
            repeat_until: None,
            panes_until: None,
            panes_typed: None,
            hints: None,
            prompt: None,
            message: None,
            overlay: None,
            chooser: None,
            popup: None,
            mouse_buttons: 0,
            drag: None,
            selecting: None,
            swallow_up: HashSet::new(),
            pending_bell: false,
            pending: std::collections::VecDeque::new(),
            window_hits: Vec::new(),
            created: Instant::now(),
            last_activity: Instant::now(),
            view_x: 0,
            view_y: 0,
            view_pinned: false,
            view_follow: false,
            cursor_out: None,
        };
        (c, rx)
    }

    /// Every default binding must survive the trip through `list-keys`:
    /// printed as a command line and parsed back into the same command.
    #[test]
    fn only_a_key_pressed_is_typing() {
        let r = |down, vk| crate::ipc::KeyRecord { down, repeat: 1, vk, sc: 0, ch: 0, ctrl: 0 };
        assert!(typing(&r(true, 0x41)) && typing(&r(true, 0x26)), "a letter, an arrow");
        assert!(!typing(&r(false, 0x0D)), "an Enter let go");
        assert!(!typing(&r(true, 0x10)) && !typing(&r(true, 0xA2)) && !typing(&r(true, 0x5B)), "a modifier alone");
    }

    #[test]
    fn default_bindings_round_trip() {
        for (key, b) in default_bindings() {
            let printed = b.cmd.to_string();
            let words = crate::command::tokenize(&printed).unwrap_or_else(|e| panic!("{key}: {printed}: {e}"));
            let back = crate::command::parse(&words).unwrap_or_else(|e| panic!("{key}: {printed}: {e}"));
            assert_eq!(back.to_string(), printed, "{key} does not round-trip");
        }
    }

    /// Every menu the default bindings open must be usable: unique keys, a
    /// command behind each entry, and separators that cannot be selected.
    #[test]
    fn default_menus_have_unique_keys() {
        let mut menus = 0;
        for (key, b) in default_bindings() {
            let Cmd::DisplayMenu { items, .. } = &b.cmd else { continue };
            menus += 1;
            let keys: Vec<&str> = items.iter().filter_map(|i| i.cmd.as_ref().and(i.key.as_deref())).collect();
            let mut seen = std::collections::HashSet::new();
            for k in &keys {
                assert!(crate::keys::Key::parse(k).is_some(), "{key}: menu key {k} is not a key");
                assert!(seen.insert(*k), "{key}: menu key {k} is used twice");
            }
            assert!(keys.len() >= 3, "{key}: a menu worth opening");
            assert!(items.iter().any(|i| i.cmd.is_none()), "{key}: menus are grouped by separators");
        }
        assert_eq!(menus, 2, "the pane menu and the window menu");
    }

    #[test]
    fn menu_selection_skips_the_separators() {
        let items = vec![
            ChooserItem::Separator, // title
            ChooserItem::Menu(0),
            ChooserItem::Separator,
            ChooserItem::Menu(1),
        ];
        let mut ch =
            Chooser::new(ChooserKind::Menu(Vec::new()), items, vec!["t".into(), "a".into(), "-".into(), "b".into()], 0);
        ch.settle(1);
        assert_eq!(ch.sel, 1, "opening a menu lands on the first real entry");
        ch.step(1);
        assert_eq!(ch.sel, 3, "j jumps over the separator");
        ch.step(1);
        assert_eq!(ch.sel, 3, "and stops at the end");
        ch.sel = ch.items.len() - 1;
        ch.settle(-1);
        assert_eq!(ch.sel, 3, "G lands on a real entry");
        ch.step(-1);
        assert_eq!(ch.sel, 1, "k jumps back over it");
        ch.step(-1);
        assert_eq!(ch.sel, 1, "and never onto the title");

        // A list of nothing but separators leaves the selection alone.
        let mut empty =
            Chooser::new(ChooserKind::Menu(Vec::new()), vec![ChooserItem::Separator; 3], vec!["-".into(); 3], 1);
        empty.step(1);
        assert_eq!(empty.sel, 1);
    }

    #[test]
    fn a_picker_filter_keeps_a_session_with_its_matching_windows() {
        let items = vec![
            ChooserItem::Separator,
            ChooserItem::Tree(1, None),
            ChooserItem::Tree(1, Some(10)),
            ChooserItem::Tree(1, Some(11)),
            ChooserItem::Tree(2, None),
            ChooserItem::Tree(2, Some(20)),
            ChooserItem::Buffer(0),
        ];
        let lines: Vec<String> =
            ["HEAD", "- alpha", "  - 0: cmd", "  - 1: Beta", "- gamma", "  - 0: cmd", "beta: 3 bytes"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(chooser_filter(&items, &lines, ""), vec![true; 7], "no filter keeps everything");
        assert_eq!(
            chooser_filter(&items, &lines, "beta"),
            vec![true, true, false, true, false, false, true],
            "case-insensitive; the title stays, a session stays for its window"
        );
        assert_eq!(
            chooser_filter(&items, &lines, "gamma"),
            vec![true, false, false, false, true, true, false],
            "a session's windows stay with it"
        );
        assert_eq!(chooser_filter(&items, &lines, "zzz"), vec![true, false, false, false, false, false, false]);
    }

    #[test]
    fn tagged_lines_are_what_an_action_key_works_on() {
        let items = vec![ChooserItem::Tree(1, None), ChooserItem::Tree(1, Some(10)), ChooserItem::Tree(2, None)];
        let mut ch =
            Chooser::new(ChooserKind::Tree { expand: true, style: TreeStyle::List }, items, vec!["a".into(); 3], 1);
        assert_eq!(ch.targets(), vec![ChooserItem::Tree(1, Some(10))], "nothing tagged: the current line");
        ch.tagged.push(ChooserItem::Tree(2, None));
        ch.tagged.push(ChooserItem::Tree(1, None));
        assert_eq!(ch.targets(), vec![ChooserItem::Tree(2, None), ChooserItem::Tree(1, None)], "tagged, in tag order");
        assert!(ch.is_tagged(&ChooserItem::Tree(2, None)) && !ch.is_tagged(&ChooserItem::Tree(1, Some(10))));
    }

    #[test]
    fn popups_are_centred_and_sized_as_asked() {
        let area = Rect { x: 0, y: 0, w: 80, h: 23 };
        let centred = |w, h| popup_rect(area, w, h, None, None);
        // Default: four fifths of the window, centred.
        let r = centred(None, None);
        assert_eq!((r.w, r.h), (64, 18));
        assert_eq!((r.x, r.y), ((80 - 64) / 2, (23 - 18) / 2));
        // Cells and percentages, and never bigger than the window.
        assert_eq!(centred(Some("40"), Some("10")).w, 40);
        assert_eq!(centred(Some("50%"), Some("50%")).h, 11);
        let big = centred(Some("500"), Some("500"));
        assert_eq!((big.w, big.h, big.x, big.y), (80, 23, 0, 0));
        // Nonsense falls back to the default instead of vanishing.
        assert_eq!(centred(Some("wide"), None).w, 64);
        // A window with no room gives a box too small to draw, which the
        // caller refuses rather than drawing a broken border.
        let tiny = popup_rect(Rect { x: 0, y: 0, w: 2, h: 2 }, None, None, None, None);
        assert!(tiny.w < 3 || tiny.h < 3, "{tiny:?}");
    }

    #[test]
    fn popups_go_where_x_and_y_say() {
        // The window area starts below a top status line, so positions are
        // relative to it, not to the terminal.
        let area = Rect { x: 0, y: 1, w: 80, h: 23 };
        let at = |x, y| {
            let r = popup_rect(area, Some("20"), Some("5"), x, y);
            (r.x, r.y)
        };
        assert_eq!(at(None, None), (30, 1 + 9), "centred without -x/-y");
        assert_eq!(at(Some("0"), Some("0")), (0, 1), "a column and a row");
        assert_eq!(at(Some("10"), Some("2")), (10, 3));
        assert_eq!(at(Some("C"), Some("C")), (30, 10), "C centres");
        assert_eq!(at(Some("R"), Some("B")), (60, 1 + 18), "R and B hug the edges");
        assert_eq!(at(Some("50%"), Some("50%")), (40, 1 + 11), "a percentage of the window");
        assert_eq!(at(Some("79"), Some("22")), (60, 19), "pulled back in when it would stick out");
        assert_eq!(at(Some("500"), Some("500")), (60, 19));
        assert_eq!(at(Some("left"), Some("top")), (30, 10), "nonsense centres");
    }

    /// The reading with every `%if` false: blocks left out whole.
    fn lines_skipping(text: &str) -> (Vec<(usize, String)>, Option<usize>) {
        logical_lines(text, &|_| false)
    }

    /// `%if` / `%elif` / `%else` pick branches by their condition, nested
    /// blocks included; only what is in a taken branch is read.
    #[test]
    fn if_blocks_pick_their_branch() {
        let text = "a\n%if yes\nb\n%elif yes\nc\n%else\nd\n%endif\n\
                    %if no\ne\n%elif no\nf\n%elif yes\ng\n%else\nh\n%endif\n\
                    %if no\ni\n%else\nj\n%endif\n\
                    %if yes\n%if no\nk\n%else\nl\n%endif\nm\n%endif\n\
                    %if no\n%if yes\nn\n%endif\no\n%endif\np\n";
        let (got, open) = logical_lines(text, &|c| c == "yes");
        let names: Vec<&str> = got.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "g", "j", "l", "m", "p"]);
        assert_eq!(open, None);
        // The condition is handed over as written, spaces trimmed.
        let seen = std::cell::RefCell::new(Vec::new());
        let _ = logical_lines("%if  #{==:#{host},box} \nx\n%endif\n", &|c| {
            seen.borrow_mut().push(c.to_string());
            true
        });
        assert_eq!(seen.into_inner(), vec!["#{==:#{host},box}"]);
        // %elif/%else with no %if open are ignored, not a crash.
        assert_eq!(logical_lines("%else\nx\n%elif y\nz\n%endif\n", &|_| true).0.len(), 2);
    }

    /// A config file the way tmux reads it: continuations joined, `%if`
    /// blocks left out whole, line numbers pointing at where a line began.
    #[test]
    fn logical_lines_follow_tmux_rules() {
        let text = "set -g a 1\n\
                    bind x \\\n  send-keys \\\n  hi\n\
                    %if #{==:#{host},box}\n\
                    set -g never 1\n\
                    %else\n\
                    set -g on-other-hosts 1\n\
                    %endif\n\
                    %hidden foo=1\n\
                    set -g b 2\n\
                    last \\";
        // Every condition false here: the %else branch is the one read.
        let (got, open) = lines_skipping(text);
        assert_eq!(
            got,
            vec![
                (1, "set -g a 1".to_string()),
                (2, "bind x   send-keys   hi".to_string()),
                (8, "set -g on-other-hosts 1".to_string()),
                (11, "set -g b 2".to_string()),
                (12, "last ".to_string()),
            ]
        );
        assert_eq!(open, None);
        // Nested blocks with no branch taken come out whole.
        let nested = "%if a\n%if b\nx\n%endif\ny\n%endif\nz";
        assert_eq!(lines_skipping(nested).0, vec![(7, "z".to_string())]);
        assert!(lines_skipping("").0.is_empty());
        // Notepad's BOM and CRLF endings, as a file written on Windows has.
        let notepad = "\u{feff}set -g a 1\r\nset -g b \\\r\n 2\r\n";
        assert_eq!(lines_skipping(notepad).0, vec![(1, "set -g a 1".to_string()), (2, "set -g b  2".to_string())]);
        // A %if that never closes swallows the rest, and says where it began.
        let (lines, open) = lines_skipping("set -g a 1\n%if x\nset -g b 2\nset -g c 3\n");
        assert_eq!(lines, vec![(1, "set -g a 1".to_string())]);
        assert_eq!(open, Some(2));
    }

    #[test]
    fn lagging_client_forces_full_redraw() {
        let (mut c, mut rx) = client(1);
        c.send_output(b"frame1".to_vec());
        assert!(c.last_grid.is_some(), "first frame queued, diff state kept");
        c.send_output(b"frame2".to_vec());
        assert!(c.last_grid.is_none() && c.last_cursor.is_none(), "dropped frame invalidates the diff base");
        // Only the first frame is in the queue; nothing else was allocated.
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::Output(b)) if b == b"frame1"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn control_messages_survive_a_full_queue_in_order() {
        let (mut c, mut rx) = client(1);
        c.send_output(b"frame1".to_vec());
        c.send(ServerMsg::SetMouse(false));
        c.send(ServerMsg::Detached { reason: "x".into() });
        assert_eq!(c.pending.len(), 2);
        // A frame must not overtake pending control messages.
        c.last_grid = Some(Grid::new(10, 2));
        c.send_output(b"frame2".to_vec());
        assert!(c.last_grid.is_none());
        assert!(!c.flush_pending(), "queue still full");
        // The client drains one slot at a time; each drain lets one message through.
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::Output(_))));
        assert!(!c.flush_pending());
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::SetMouse(false))));
        assert!(c.flush_pending());
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::Detached { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn path_with_self_contains_our_directory_exactly_once() {
        // (cargo already puts target/debug/deps on PATH for tests, so the
        // interesting property is "present, and not duplicated".)
        let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let p = path_with_self();
        let count = std::env::split_paths(&p).filter(|x| *x == dir).count();
        let before = std::env::split_paths(&std::env::var("PATH").unwrap()).filter(|x| *x == dir).count();
        assert_eq!(count, before.max(1));
        if before == 0 {
            assert_eq!(std::env::split_paths(&p).next().unwrap(), dir);
        }
    }

    #[test]
    fn run_shell_reports_exit_code_and_utf8() {
        let (ok, echo) = if cfg!(windows) {
            ("Write-Output 中文-ok; exit 3", "Write-Output $env:KEEPANE_TEST_VAR")
        } else {
            ("printf '中文-ok\n'; exit 3", "echo $KEEPANE_TEST_VAR")
        };
        let (out, code) = run_shell_blocking(ok, &[], None);
        assert_eq!(code, 3);
        assert!(out.contains("中文-ok"), "{out:?}");
        let (out, code) = run_shell_blocking(echo, &[("KEEPANE_TEST_VAR".into(), "v1".into())], None);
        assert_eq!(code, 0);
        assert_eq!(out.trim(), "v1");
    }

    #[test]
    fn run_shell_timeout_kills_the_tree() {
        let start = Instant::now();
        let (out, code) = run_shell_blocking(
            if cfg!(windows) {
                "Write-Output first-line; ping -n 30 127.0.0.1 > $null; Write-Output never-reached"
            } else {
                "echo first-line; sleep 30; echo never-reached"
            },
            &[],
            Some(Duration::from_secs(2)),
        );
        assert_eq!(code, 124, "{out:?}");
        assert!(out.contains("first-line") && !out.contains("never-reached"), "{out:?}");
        assert!(start.elapsed() < Duration::from_secs(10), "took {:?}", start.elapsed());
    }

    /// An instant is the same second whenever it is asked about: a dead
    /// pane's running time does not flicker between two values.
    #[test]
    fn an_instant_is_always_the_same_unix_second() {
        let born = Instant::now() - Duration::from_millis(400);
        let died = born + Duration::from_millis(700);
        let first = (unix_seconds(born), unix_seconds(died));
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(37));
            assert_eq!((unix_seconds(born), unix_seconds(died)), first);
        }
        let now = chrono::Utc::now().timestamp();
        assert!((unix_seconds(Instant::now()) - now).abs() <= 1, "wall time");
        assert!(unix_seconds(born - Duration::from_secs(90)) <= now - 90, "before the anchor too");
    }

    #[test]
    fn expand_home_forms() {
        let home = dirs::home_dir().unwrap().display().to_string();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/.keepane.conf"), format!("{home}/.keepane.conf"));
        assert_eq!(expand_home("~\\x"), format!("{home}\\x"));
        assert_eq!(expand_home("~user/x"), "~user/x");
        assert_eq!(expand_home("C:\\x"), "C:\\x");
    }
}
