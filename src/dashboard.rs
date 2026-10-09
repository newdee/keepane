//! `keepane dashboard`: panels in the manner of lazygit
//! (docs/design/dashboard.md). On the left, [1] every pane (its mode,
//! whether it is free, its inbox, how long it has been quiet, its
//! program), [2] the chosen pane's inbox and [3] the tasks; on the right,
//! [0] the chosen pane: its screen live, its scrollback, its events, or one
//! message or task in full. Opened by prefix v in a popup, or run in any
//! terminal.
//!
//! It can act as well as watch: send a pane a message, rename it, change
//! its work mode, mark it ready, go to it, close it, and delete or move
//! queued messages. What cannot be taken back lightly (deleting a message,
//! closing a pane, switching a pane to `shell`, where what it gets is run)
//! waits for a yes. Everything it asks the server is in `QUERIES`, and
//! everything it may change in `ACTIONS`. The board is kept apart from the
//! console, so it can be tested without one.

use crate::ipc::MouseRecord;
use crate::keys::{Key, KeyCode};
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

/// The only server commands the dashboard sends to look.
pub const QUERIES: &[&str] = &[
    "list-panes",
    "list-events",
    "list-messages",
    "list-tasks",
    "capture-pane",
    "trace-message",
    "show-task",
    "web-status",
];
/// The only ones it sends to change anything.
pub const ACTIONS: &[&str] = &[
    "send-message",
    "rename-pane",
    "set-work-mode",
    "pane-ready",
    "focus-pane",
    "kill-pane",
    "drop-message",
    "move-message",
];

/// Whether `argv` is a change that waits for a yes: deleting a queued
/// message, closing a pane, or letting a pane run what it is sent.
pub fn needs_yes(argv: &[String]) -> bool {
    match argv.first().map(String::as_str) {
        Some("kill-pane") => true,
        Some("drop-message") => argv.get(1).is_some_and(|a| a != "-u"),
        Some("set-work-mode") => argv.last().is_some_and(|m| m == "shell"),
        _ => false,
    }
}

/// `list-panes -F` for the board: one pane a line, tab-separated.
pub const PANE_FORMAT: &str = "#{pane_address}\t#{session_name}\t#{window_index}.#{pane_index}\t#{pane_id}\t#{pane_name}\t#{pane_work_mode}\t#{pane_idle}\t#{pane_inbox}\t#{pane_status}\t#{pane_current_command}\t#{pane_message}\t#{pane_dead}\t#{pane_current_path}\t#{pane_pid}\t#{pane_start_time}\t#{pane_activity}\t#{pane_width}\t#{pane_height}\t#{pane_dead_status}\t#{pane_unheard}\t#{agent}\t#{agent_model}\t#{agent_cost}\t#{agent_tokens}\t#{agent_context}\t#{agent_cpu}\t#{agent_mem}\t#{agent_tools}\t#{agent_turns}";

/// The agent a pane runs, its figures as the `agent_*` formats write them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AgentRow {
    pub kind: String,
    pub model: String,
    /// `$1.23` (`$1.23+`: some tokens unpriced), empty when not known.
    pub cost: String,
    pub tokens: String,
    pub context: String,
    pub cpu: String,
    pub mem: String,
    pub tools: String,
    pub turns: String,
}

impl AgentRow {
    /// From the fields after `pane_unheard`; None for a pane without an
    /// agent (or a line from a server that does not know them).
    fn parse(f: &[&str]) -> Option<AgentRow> {
        let g = |i: usize| f.get(i).copied().unwrap_or_default().to_string();
        let kind = g(0);
        (!kind.is_empty()).then(|| AgentRow {
            kind,
            model: g(1),
            cost: g(2),
            tokens: g(3),
            context: g(4),
            cpu: g(5),
            mem: g(6),
            tools: g(7),
            turns: g(8),
        })
    }

    /// In the pane table, where room is short: `claude $2.10 34%` (the
    /// share of its context window in use).
    fn short(&self) -> String {
        let mut s = paint("35", &self.kind);
        if !self.cost.is_empty() {
            s.push_str(&format!(" {}", paint("32", &self.cost)));
        }
        if !self.context.is_empty() {
            s.push_str(&format!(" {}", paint_context(&self.context)));
        }
        s
    }

    /// Over the chosen pane: everything, each figure after the word that
    /// says what it is (the words dim).
    fn line(&self) -> String {
        let sep = paint("2", " · ");
        let model = if self.model.is_empty() { String::new() } else { format!(" {}", paint("36", &self.model)) };
        let cost = if self.cost.is_empty() { String::new() } else { format!("{sep}{}", paint("32", &self.cost)) };
        let context = if self.context.is_empty() {
            String::new()
        } else {
            format!("{sep}{} {}", paint("2", "context"), paint_context(&self.context))
        };
        // What matters most first: a narrow panel cuts the end.
        format!(
            "{}{model}{cost}{context}{sep}{} {}{sep}{} {}{sep}{} {}{sep}{} {}{sep}{} {}",
            paint("35", &self.kind),
            self.tokens,
            paint("2", "tokens"),
            paint("2", "cpu"),
            self.cpu,
            paint("2", "mem"),
            self.mem,
            self.tools,
            paint("2", "tools"),
            self.turns,
            paint("2", "turns"),
        )
    }

    /// The cost in dollars, when known.
    fn dollars(&self) -> Option<f64> {
        self.cost.strip_prefix('$')?.trim_end_matches('+').parse().ok()
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PaneRow {
    pub address: String,
    pub session: String,
    pub place: String,
    pub id: String,
    pub name: String,
    pub mode: String,
    pub idle: bool,
    pub inbox: usize,
    pub status: String,
    pub command: String,
    pub message: String,
    pub dead: bool,
    pub path: String,
    pub pid: String,
    /// Unix seconds: when its program started, and when it last printed.
    pub started: i64,
    pub activity: i64,
    pub size: String,
    pub exit: String,
    /// An agent's pane whose agent has not said it is free since it started.
    pub unheard: bool,
    /// The agent it runs, if any (`list-agents`).
    pub agent: Option<AgentRow>,
}

impl PaneRow {
    pub fn parse(line: &str) -> Option<PaneRow> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 19 {
            return None;
        }
        Some(PaneRow {
            address: f[0].into(),
            session: f[1].into(),
            place: f[2].into(),
            id: f[3].into(),
            name: f[4].into(),
            mode: f[5].into(),
            idle: f[6] == "1",
            inbox: f[7].parse().unwrap_or(0),
            status: f[8].into(),
            command: f[9].into(),
            message: f[10].into(),
            dead: f[11] == "1",
            path: f[12].into(),
            pid: f[13].into(),
            started: f[14].parse().unwrap_or(0),
            activity: f[15].parse().unwrap_or(0),
            size: format!("{}x{}", f[16], f[17]),
            exit: f[18].into(),
            unheard: f.get(19) == Some(&"1"),
            agent: f.get(20..).and_then(AgentRow::parse),
        })
    }

    fn state(&self) -> &'static str {
        if self.dead {
            "exited"
        } else if self.mode == "normal" {
            "-"
        } else if self.idle {
            "idle"
        } else {
            "busy"
        }
    }

    /// The state, coloured: free green, busy yellow, exited red; a
    /// `normal` pane (never free or busy) nothing.
    fn state_styled(&self) -> String {
        let colour = match self.state() {
            "idle" => "32",
            "busy" => "33",
            "exited" => "31",
            _ => return String::new(),
        };
        paint(colour, self.state())
    }

    /// The work mode, coloured: `ai` magenta, `shell` (it runs what it
    /// gets) yellow; `normal` nothing.
    fn mode_styled(&self) -> String {
        match self.mode.as_str() {
            "normal" => String::new(),
            "ai" => paint("35", "ai"),
            "shell" => paint("33", "shell"),
            m => m.to_string(),
        }
    }

    /// What it is doing: what it said, else the message it works on, else
    /// its program. Before all that, messages waiting on an agent that has
    /// never said it is free: the hook it lacks.
    fn doing(&self) -> String {
        if self.unheard && self.inbox > 0 {
            "never said it is free: keepane setup".into()
        } else if !self.status.is_empty() {
            format!("\"{}\"", self.status)
        } else if !self.message.is_empty() {
            format!("working on #{}", self.message)
        } else {
            self.command.clone()
        }
    }

    fn matches(&self, filter: &str) -> bool {
        let f = filter.to_lowercase();
        let agent = self.agent.as_ref().map(|a| format!("{} {}", a.kind, a.model)).unwrap_or_default();
        [&self.address, &self.session, &self.name, &self.mode, &self.status, &self.command, &self.path, &agent]
            .iter()
            .any(|x| x.to_lowercase().contains(&f))
    }
}

/// One queued message, as `list-messages` gives it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Queued {
    pub id: String,
    pub from: String,
    pub waiting: String,
    pub text: String,
}

impl Queued {
    /// `  #5  from user  waiting 1s  the text`.
    fn parse(line: &str) -> Option<Queued> {
        let rest = line.strip_prefix("  #")?;
        let mut parts = rest.split("  ");
        let id = parts.next()?.trim().to_string();
        let from = parts.next()?.strip_prefix("from ")?.to_string();
        let waiting = parts.next()?.strip_prefix("waiting ")?.to_string();
        let text = parts.collect::<Vec<_>>().join("  ");
        Some(Queued { id, from, waiting, text })
    }
}

/// One task, as `list-tasks` gives it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TaskRow {
    pub id: String,
    pub status: String,
    /// Where its first message went, as the pane's part of the address
    /// (`%4 (ai)`).
    pub at: String,
    pub took: String,
    pub steps: String,
    pub title: String,
}

impl TaskRow {
    /// `list-tasks` lines: fixed columns, where its heading puts them; a
    /// line without a heading goes by words (id, status, the rest).
    fn parse_all(lines: &[String]) -> Vec<TaskRow> {
        const HEADS: [&str; 6] = ["TASK", "STATUS", "AT", "TOOK", "STEPS", "TITLE"];
        let cols: Option<Vec<usize>> = lines.iter().find(|l| l.starts_with("TASK")).and_then(|head| {
            let mut from = 0;
            HEADS
                .iter()
                .map(|w| {
                    let at = from + head.get(from..)?.find(w)?;
                    from = at + w.len();
                    Some(at)
                })
                .collect()
        });
        lines
            .iter()
            .filter(|l| l.starts_with('#'))
            .map(|l| {
                let by_cols = cols.as_ref().and_then(|c| {
                    let cell = |i: usize| -> Option<String> {
                        let end = c.get(i + 1).copied().unwrap_or(l.len()).min(l.len());
                        Some(l.get(c[i].min(end)..end)?.trim().to_string())
                    };
                    Some(TaskRow {
                        id: cell(0)?.trim_start_matches('#').to_string(),
                        status: cell(1)?,
                        at: cell(2)?.rsplit('.').next().unwrap_or_default().to_string(),
                        took: cell(3)?,
                        steps: cell(4)?,
                        title: cell(5)?,
                    })
                });
                by_cols.unwrap_or_else(|| {
                    let mut w = l.split_whitespace();
                    TaskRow {
                        id: w.next().unwrap_or_default().trim_start_matches('#').to_string(),
                        status: w.next().unwrap_or_default().to_string(),
                        title: w.collect::<Vec<_>>().join(" "),
                        ..Default::default()
                    }
                })
            })
            .collect()
    }

    fn line(&self) -> String {
        let colour = match self.status.as_str() {
            "running" => "33",
            "failed" => "31",
            "done" => "32",
            _ => "2",
        };
        // The title before where it went: in a narrow panel the title is
        // what is read.
        format!(
            "#{:<3} \x1b[{colour}m{:<8}\x1b[0m {:>5}  {}  \x1b[2m{}\x1b[0m",
            self.id, self.status, self.took, self.title, self.at
        )
    }
}

/// The panels, in the order Tab goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Panes,
    Inbox,
    Tasks,
    Main,
}

/// What the main panel shows of the chosen pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Screen,
    Scrollback,
    Events,
    Detail,
}

const TABS: [Tab; 4] = [Tab::Screen, Tab::Scrollback, Tab::Events, Tab::Detail];

impl Tab {
    fn word(self) -> &'static str {
        match self {
            Tab::Screen => "Screen",
            Tab::Scrollback => "Scrollback",
            Tab::Events => "Events",
            Tab::Detail => "Detail",
        }
    }

    /// Read from the bottom up (a screen, a log) rather than from the top.
    fn read_up(self) -> bool {
        self != Tab::Detail
    }
}

/// What Detail shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Message(String),
    Task(String),
}

/// A line being typed at the bottom.
#[derive(Clone, Debug, PartialEq)]
enum Input {
    Filter,
    Send { to: String },
    Rename { to: String },
}

/// What waits for an answer at the bottom.
#[derive(Clone, Debug, PartialEq)]
enum Dialog {
    /// `y` runs it, anything else does not.
    Confirm { question: String, argv: Vec<String> },
    /// A work mode for a pane, picked with j/k.
    Mode { to: String, pick: usize },
    /// Every key, in the main panel.
    Help,
}

const MODES: [&str; 3] = ["normal", "shell", "ai"];

#[derive(Debug, PartialEq)]
pub enum Action {
    Redraw,
    Quit,
    /// A server command to run (a change; queries come with the refresh).
    Run(Vec<String>),
    /// Go to a pane: run it, and when in a popup, close the dashboard.
    Go(Vec<String>),
}

pub struct Board {
    pub rows: Vec<PaneRow>,
    /// The chosen pane, by id, so it stays chosen when others come and go.
    pub chosen: Option<String>,
    pub focus: Panel,
    /// The left panel focus goes back to from the main one.
    left: Panel,
    pub tab: Tab,
    pub inbox: Vec<Queued>,
    pub inbox_pick: usize,
    /// The tasks, and the one picked.
    pub tasks: Vec<TaskRow>,
    pub task_pick: usize,
    /// What the main panel's tab shows, as its query gave it.
    pub main: Vec<String>,
    pub detail: Option<Detail>,
    /// Detail shown because the cursor is on a message or task ([2], [3]):
    /// the tab to go back to on leaving, and what was followed last (a
    /// tab changed by hand stays changed until the cursor moves).
    tab_before: Tab,
    followed: bool,
    last_follow: Option<(Panel, Option<Detail>)>,
    /// Lines the main panel is scrolled by, away from where it is read from.
    pub scroll: usize,
    pub filter: String,
    input: Option<(Input, String)>,
    dialog: Option<Dialog>,
    pub note: Option<String>,
    pub cols: u16,
    pub height: u16,
    /// Unix seconds, from the refresh (how long ago panes started, went quiet).
    pub now: i64,
    /// How many are connected to the phones' page, while it is served.
    pub web: Option<usize>,
}

/// Visible width of text that may hold SGR sequences.
fn width_of(s: &str) -> usize {
    clip(s, usize::MAX).1
}

/// `s` cut to `width` columns, escape sequences kept (a CSI whole, an OSC
/// up to its end), other control characters as spaces; with the width it
/// takes. A cut string ends its styles.
fn clip(s: &str, width: usize) -> (String, usize) {
    let mut out = String::new();
    let mut used = 0;
    let mut styled = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            let mut seq = String::from(c);
            match chars.peek() {
                Some('[') => {
                    seq.push(chars.next().unwrap());
                    for d in chars.by_ref() {
                        seq.push(d);
                        if ('\x40'..='\x7e').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // An OSC (a title, a link): not for this board; dropped.
                    for d in chars.by_ref() {
                        if d == '\x07' {
                            break;
                        }
                        if d == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
                Some(_) => {
                    seq.push(chars.next().unwrap());
                }
                None => {}
            }
            styled = true;
            out.push_str(&seq);
            continue;
        }
        let (c, w) = if c.is_control() { (' ', 1) } else { (c, c.width().unwrap_or(0)) };
        if used + w > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    (out, used)
}

/// `text` in the style `sgr` (`32` green, `1` bold, `2` dim...), then
/// plain again; nothing for no text. Colour says what a thing is, never
/// only decorates: the 16 colours the terminal's theme maps, so a light
/// theme reads as well as a dark one.
fn paint(sgr: &str, text: &str) -> String {
    if text.is_empty() { String::new() } else { format!("\x1b[{sgr}m{text}\x1b[0m") }
}

/// Keys and what they do (`j/k pane  Enter screen`, two spaces between),
/// each key cyan and bold so the eye finds it, what it does plain.
fn key_hints(hints: &str) -> String {
    hints
        .split("  ")
        .map(|h| match h.split_once(' ') {
            Some((key, what)) => format!("{} {what}", paint("1;36", key)),
            None => paint("1;36", h),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// Whether a wait as it reads (`85ms`, `7.3s`, `4m05s`, `2h13m`, `3d2h`) is
/// a minute or more: worth the eye.
fn waited_long(waiting: &str) -> bool {
    let w = waiting.trim_end_matches("ms");
    w.len() == waiting.len() && w.contains(['m', 'h', 'd'])
}

/// The pane table's lines, each with the pane it is (None: a heading).
type Rows = Vec<(Option<String>, String)>;

/// A column of the pane table: its heading, a cell a pane (styled),
/// whether it reads against its right edge (a number), and how much it is
/// kept when room is short (the lowest goes first; `u8::MAX` never).
struct Column {
    head: &'static str,
    cells: Vec<String>,
    right: bool,
    keep: u8,
}

impl Column {
    /// As wide as its widest cell or its heading.
    fn width(&self) -> usize {
        self.cells.iter().map(|c| width_of(c)).max().unwrap_or(0).max(self.head.len())
    }

    fn cell(&self, text: &str) -> String {
        if self.right { fit_right(text, self.width()) } else { fit(text, self.width()) }
    }
}

/// A context's share in the colour of how full it is: green, yellow from
/// 60%, red from 80%; a size (no window known) plain.
fn paint_context(context: &str) -> String {
    match context.strip_suffix('%').and_then(|p| p.parse::<u32>().ok()) {
        Some(p) if p >= 80 => paint("31", context),
        Some(p) if p >= 60 => paint("33", context),
        Some(_) => paint("32", context),
        None => context.to_string(),
    }
}

/// `s` in `width` columns, against their right edge (a number).
fn fit_right(s: &str, width: usize) -> String {
    let (out, used) = clip(s, width);
    format!("{}{out}", " ".repeat(width - used))
}

/// `s` in exactly `width` columns: cut, or filled with spaces.
fn fit(s: &str, width: usize) -> String {
    let (mut out, used) = clip(s, width);
    out.push_str(&" ".repeat(width - used));
    out
}

/// Where each panel is on the screen: its top row, its height (borders
/// included), and its left column and width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn holds(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    /// Rows inside the borders.
    fn inner_h(&self) -> usize {
        self.h.saturating_sub(2)
    }
    fn inner_w(&self) -> usize {
        self.w.saturating_sub(2)
    }
}

struct Layout {
    panes: Option<Rect>,
    inbox: Option<Rect>,
    tasks: Option<Rect>,
    main: Option<Rect>,
}

/// Below this width, only the focused side is shown.
const NARROW: usize = 70;

impl Board {
    pub fn new(cols: u16, height: u16) -> Board {
        Board {
            rows: Vec::new(),
            chosen: None,
            focus: Panel::Panes,
            left: Panel::Panes,
            tab: Tab::Screen,
            inbox: Vec::new(),
            inbox_pick: 0,
            tasks: Vec::new(),
            task_pick: 0,
            main: Vec::new(),
            detail: None,
            tab_before: Tab::Screen,
            followed: false,
            last_follow: None,
            scroll: 0,
            filter: String::new(),
            input: None,
            dialog: None,
            note: None,
            cols,
            height,
            now: 0,
            web: None,
        }
    }

    pub fn resize(&mut self, cols: u16, height: u16) {
        self.cols = cols;
        self.height = height;
    }

    /// The panes shown, after the filter.
    fn shown(&self) -> Vec<&PaneRow> {
        self.rows.iter().filter(|r| self.filter.is_empty() || r.matches(&self.filter)).collect()
    }

    pub fn chosen_row(&self) -> Option<&PaneRow> {
        let shown = self.shown();
        self.chosen.as_ref().and_then(|id| shown.iter().find(|r| &r.id == id).copied()).or(shown.first().copied())
    }

    fn chosen_id(&self) -> Option<String> {
        self.chosen_row().map(|r| r.id.clone())
    }

    /// New pane rows from the server; the chosen pane stays chosen.
    pub fn set_rows(&mut self, rows: Vec<PaneRow>) {
        self.rows = rows;
        self.chosen = self.chosen_id();
    }

    /// The chosen pane's inbox, as `list-messages -t` gives it.
    pub fn set_inbox(&mut self, lines: &[String]) {
        self.inbox = lines.iter().filter_map(|l| Queued::parse(l)).collect();
        self.inbox_pick = self.inbox_pick.min(self.inbox.len().saturating_sub(1));
        self.follow();
    }

    /// The tasks, as `list-tasks` gives them (its heading left out).
    pub fn set_tasks(&mut self, lines: &[String]) {
        self.tasks = TaskRow::parse_all(lines);
        self.task_pick = self.task_pick.min(self.tasks.len().saturating_sub(1));
        self.follow();
    }

    /// `web-status`'s first line: `serving <url> · ... · N connected`, or off.
    pub fn set_web(&mut self, status: &str) {
        let first = status.lines().next().unwrap_or("");
        self.web = first
            .starts_with("serving ")
            .then(|| first.rsplit(" · ").next()?.strip_suffix(" connected")?.parse().ok())
            .flatten();
    }

    pub fn set_main(&mut self, lines: Vec<String>) {
        let mut lines = lines;
        // A screen's blank rows below its last line say nothing.
        if matches!(self.tab, Tab::Screen | Tab::Scrollback) {
            while lines.last().is_some_and(|l| width_of(l) == 0 || clip(l, usize::MAX).0.trim().is_empty()) {
                lines.pop();
            }
        }
        self.main = lines;
    }

    /// What to ask for the chosen pane's inbox.
    pub fn inbox_query(&self) -> Option<Vec<String>> {
        self.chosen_id().map(|id| owned(&["list-messages", "-t", &id]))
    }

    pub fn tasks_query(&self) -> Vec<String> {
        owned(&["list-tasks"])
    }

    /// What to ask for the main panel's tab.
    pub fn main_query(&self) -> Option<Vec<String>> {
        if self.tab == Tab::Detail {
            return match &self.detail {
                Some(Detail::Message(m)) => Some(owned(&["trace-message", m, "-J"])),
                Some(Detail::Task(t)) => Some(owned(&["show-task", t, "-J"])),
                None => None,
            };
        }
        let id = self.chosen_id()?;
        Some(match self.tab {
            Tab::Screen => owned(&["capture-pane", "-p", "-e", "-t", &id]),
            Tab::Scrollback => owned(&["capture-pane", "-p", "-e", "-S", "-2000", "-t", &id]),
            _ => owned(&["list-events", "-t", &id, "-n", "300"]),
        })
    }

    fn step_pane(&mut self, by: isize) {
        let shown = self.shown();
        if shown.is_empty() {
            return;
        }
        let at = self.chosen_row().and_then(|c| shown.iter().position(|r| r.id == c.id)).unwrap_or(0);
        let to = (at as isize + by).clamp(0, shown.len() as isize - 1) as usize;
        let id = shown[to].id.clone();
        if Some(&id) != self.chosen.as_ref() {
            self.chosen = Some(id);
            self.scroll = 0;
            self.inbox_pick = 0;
            if self.tab == Tab::Detail && matches!(self.detail, Some(Detail::Message(_))) {
                self.tab = Tab::Screen;
            }
        }
    }

    fn focus_on(&mut self, p: Panel) {
        if p != Panel::Main {
            self.left = p;
        }
        self.focus = p;
    }

    fn cycle(&mut self, back: bool) {
        const ORDER: [Panel; 4] = [Panel::Panes, Panel::Inbox, Panel::Tasks, Panel::Main];
        let at = ORDER.iter().position(|p| *p == self.focus).unwrap_or(0);
        let to = if back { (at + ORDER.len() - 1) % ORDER.len() } else { (at + 1) % ORDER.len() };
        self.focus_on(ORDER[to]);
    }

    fn switch_tab(&mut self, by: isize) {
        let at = TABS.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        self.tab = TABS[(at + by).rem_euclid(TABS.len() as isize) as usize];
        self.scroll = 0;
        // Chosen by hand: it stays when the cursor leaves the list.
        self.followed = false;
    }

    fn scroll_by(&mut self, up: isize) {
        // Up is away from the bottom for what is read from the bottom, and
        // back towards the top for what is read from the top.
        let by = if self.tab.read_up() { up } else { -up };
        self.scroll = (self.scroll as isize + by).clamp(0, self.main.len() as isize) as usize;
    }

    fn page(&self) -> isize {
        (usize::from(self.height) / 2).max(1) as isize
    }

    fn picked_message(&self) -> Option<&Queued> {
        self.inbox.get(self.inbox_pick)
    }

    /// Ask before `argv` when it needs a yes; else run it.
    fn act(&mut self, question: String, argv: Vec<String>) -> Action {
        if needs_yes(&argv) {
            self.dialog = Some(Dialog::Confirm { question, argv });
            Action::Redraw
        } else {
            Action::Run(argv)
        }
    }

    /// The cursor on a message ([2]) or a task ([3]): it is shown on the
    /// right at once, no Enter. Back on the panes, the tab that was there.
    fn follow(&mut self) {
        let want = match self.focus {
            Panel::Inbox => self.picked_message().map(|m| Detail::Message(m.id.clone())),
            Panel::Tasks => {
                self.tasks.get(self.task_pick).filter(|t| !t.id.is_empty()).map(|t| Detail::Task(t.id.clone()))
            }
            Panel::Panes => {
                if self.followed {
                    self.followed = false;
                    self.tab = self.tab_before;
                    self.scroll = 0;
                }
                self.last_follow = None;
                return;
            }
            Panel::Main => return,
        };
        let now = Some((self.focus, want.clone()));
        if self.last_follow == now {
            return;
        }
        self.last_follow = now;
        if want.is_none() {
            return;
        }
        if self.tab != Tab::Detail {
            self.tab_before = self.tab;
            self.followed = true;
            self.tab = Tab::Detail;
        }
        if self.detail != want {
            self.detail = want;
            self.scroll = 0;
        }
    }

    pub fn key(&mut self, k: Key) -> Action {
        let a = self.key_inner(k);
        self.follow();
        a
    }

    fn key_inner(&mut self, k: Key) -> Action {
        self.note = None;
        // Text being typed (a message, a name, the filter) is kept as typed;
        // a key of the board's own, `【` for `[` with a Chinese input method
        // on, is read as its ASCII form.
        if let Some(d) = self.dialog.take() {
            return self.answer(d, crate::keys::ascii_form(k));
        }
        if let Some((what, text)) = self.input.take() {
            return self.typed(what, text, k);
        }
        let k = crate::keys::ascii_form(k);
        let plain = !k.ctrl && !k.alt;
        // Everywhere.
        match k.code {
            KeyCode::Char('q') if plain => return Action::Quit,
            KeyCode::Char('c') if k.ctrl => return Action::Quit,
            KeyCode::Escape => return Action::Quit,
            KeyCode::Tab => {
                self.cycle(k.shift);
                return Action::Redraw;
            }
            KeyCode::Char(c @ ('1' | '2' | '3' | '0')) if plain => {
                self.focus_on(match c {
                    '1' => Panel::Panes,
                    '2' => Panel::Inbox,
                    '3' => Panel::Tasks,
                    _ => Panel::Main,
                });
                return Action::Redraw;
            }
            KeyCode::Char('l') | KeyCode::Right if plain => {
                self.focus_on(Panel::Main);
                return Action::Redraw;
            }
            KeyCode::Char('h') | KeyCode::Left if plain => {
                self.focus_on(self.left);
                return Action::Redraw;
            }
            KeyCode::Char('[') if plain => {
                self.switch_tab(-1);
                return Action::Redraw;
            }
            KeyCode::Char(']') if plain => {
                self.switch_tab(1);
                return Action::Redraw;
            }
            KeyCode::Char('?') if plain => {
                self.dialog = Some(Dialog::Help);
                return Action::Redraw;
            }
            KeyCode::Char('/') if plain => {
                self.input = Some((Input::Filter, self.filter.clone()));
                return Action::Redraw;
            }
            // Paging as in vi and less: C-b / C-f a page, C-u / C-d half
            // (in the prefix-v popup, C-b is the prefix: C-b C-b sends it).
            KeyCode::PPage => {
                self.scroll_by(self.page());
                return Action::Redraw;
            }
            KeyCode::Char('b') if k.ctrl && !k.alt => {
                self.scroll_by(self.page());
                return Action::Redraw;
            }
            KeyCode::NPage => {
                self.scroll_by(-self.page());
                return Action::Redraw;
            }
            KeyCode::Char('f') if k.ctrl && !k.alt => {
                self.scroll_by(-self.page());
                return Action::Redraw;
            }
            KeyCode::Char('u') if k.ctrl && !k.alt => {
                self.scroll_by((self.page() / 2).max(1));
                return Action::Redraw;
            }
            KeyCode::Char('d') if k.ctrl && !k.alt => {
                self.scroll_by(-(self.page() / 2).max(1));
                return Action::Redraw;
            }
            _ => {}
        }
        if !plain {
            return Action::Redraw;
        }
        match self.focus {
            Panel::Panes => self.pane_key(k.code),
            Panel::Inbox => self.inbox_key(k.code),
            Panel::Tasks => self.task_key(k.code),
            Panel::Main => self.main_key(k.code),
        }
    }

    fn pane_key(&mut self, code: KeyCode) -> Action {
        let Some(row) = self.chosen_row().cloned() else { return Action::Redraw };
        let id = row.id.clone();
        let who = if row.name.is_empty() { id.clone() } else { format!("{id} ({})", row.name) };
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.step_pane(1),
            KeyCode::Up | KeyCode::Char('k') => self.step_pane(-1),
            // The first pane, the last (as vi, and Home / End).
            KeyCode::Home | KeyCode::Char('g') => self.step_pane(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.step_pane(isize::MAX / 2),
            KeyCode::Enter => {
                self.tab = Tab::Screen;
                self.scroll = 0;
                self.focus_on(Panel::Main);
            }
            KeyCode::Char('s') => self.input = Some((Input::Send { to: id }, String::new())),
            KeyCode::Char('r') => self.input = Some((Input::Rename { to: id }, row.name.clone())),
            KeyCode::Char('m') => {
                let pick = MODES.iter().position(|m| *m == row.mode).unwrap_or(0);
                self.dialog = Some(Dialog::Mode { to: id, pick });
            }
            KeyCode::Char('R') => return Action::Run(owned(&["pane-ready", "-t", &id])),
            KeyCode::Char('o') => return Action::Go(owned(&["focus-pane", &id])),
            KeyCode::Char('x') => {
                return self.act(
                    format!("close {who}? (prefix u within 10s brings it back)"),
                    owned(&["kill-pane", "-t", &id]),
                );
            }
            _ => {}
        }
        Action::Redraw
    }

    fn inbox_key(&mut self, code: KeyCode) -> Action {
        let last = self.inbox.len().saturating_sub(1);
        let picked = self.picked_message().cloned();
        match (code, picked) {
            (KeyCode::Down | KeyCode::Char('j'), _) => self.inbox_pick = (self.inbox_pick + 1).min(last),
            (KeyCode::Up | KeyCode::Char('k'), _) => self.inbox_pick = self.inbox_pick.saturating_sub(1),
            (KeyCode::Home | KeyCode::Char('g'), _) => self.inbox_pick = 0,
            (KeyCode::End | KeyCode::Char('G'), _) => self.inbox_pick = last,
            (KeyCode::Char('u'), _) => return Action::Run(owned(&["drop-message", "-u"])),
            (KeyCode::Enter, Some(m)) => {
                self.detail = Some(Detail::Message(m.id));
                self.tab = Tab::Detail;
                self.scroll = 0;
                self.focus_on(Panel::Main);
            }
            (KeyCode::Char('d'), Some(m)) => {
                let question = format!("delete #{} from {} (\"{}\")? (u brings it back)", m.id, m.from, m.text);
                return self.act(question, owned(&["drop-message", &m.id]));
            }
            // The pick goes with the message it moves.
            (KeyCode::Char('K'), Some(m)) => {
                self.inbox_pick = self.inbox_pick.saturating_sub(1);
                return Action::Run(owned(&["move-message", &m.id, "up"]));
            }
            (KeyCode::Char('J'), Some(m)) => {
                self.inbox_pick = (self.inbox_pick + 1).min(last);
                return Action::Run(owned(&["move-message", &m.id, "down"]));
            }
            (KeyCode::Char('t'), Some(m)) => {
                self.inbox_pick = 0;
                return Action::Run(owned(&["move-message", &m.id, "top"]));
            }
            (KeyCode::Char('d' | 'K' | 'J' | 't') | KeyCode::Enter, None) => {
                self.note = Some("no queued message".into());
            }
            _ => {}
        }
        Action::Redraw
    }

    fn task_key(&mut self, code: KeyCode) -> Action {
        let last = self.tasks.len().saturating_sub(1);
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.task_pick = (self.task_pick + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.task_pick = self.task_pick.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.task_pick = 0,
            KeyCode::End | KeyCode::Char('G') => self.task_pick = last,
            KeyCode::Enter => {
                let id = self.tasks.get(self.task_pick).map(|t| t.id.clone()).filter(|id| !id.is_empty());
                match id {
                    Some(id) => {
                        self.detail = Some(Detail::Task(id));
                        self.tab = Tab::Detail;
                        self.scroll = 0;
                        self.focus_on(Panel::Main);
                    }
                    None => self.note = Some("no task".into()),
                }
            }
            _ => {}
        }
        Action::Redraw
    }

    fn main_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(-1),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_by(self.main.len() as isize),
            KeyCode::End | KeyCode::Char('G') => self.scroll_by(-(self.main.len() as isize)),
            KeyCode::Enter => self.focus_on(self.left),
            _ => {}
        }
        Action::Redraw
    }

    fn typed(&mut self, what: Input, mut text: String, k: Key) -> Action {
        match k.code {
            KeyCode::Escape => {
                if what == Input::Filter {
                    self.filter.clear();
                }
                return Action::Redraw;
            }
            KeyCode::Enter => {
                return match what {
                    Input::Filter => Action::Redraw,
                    _ if text.trim().is_empty() => Action::Redraw,
                    Input::Send { to } => Action::Run(owned(&["send-message", "-t", &to, "--", &text])),
                    Input::Rename { to } => Action::Run(owned(&["rename-pane", "-t", &to, "--", text.trim()])),
                };
            }
            KeyCode::BSpace => {
                text.pop();
            }
            KeyCode::Char(c) if !k.ctrl && !k.alt => text.push(c),
            _ => {}
        }
        if what == Input::Filter {
            self.filter = text.clone();
            self.chosen = self.chosen_id();
        }
        self.input = Some((what, text));
        Action::Redraw
    }

    fn answer(&mut self, d: Dialog, k: Key) -> Action {
        match d {
            Dialog::Help => Action::Redraw,
            Dialog::Confirm { argv, .. } => {
                if k.code == KeyCode::Char('y') && !k.ctrl && !k.alt {
                    Action::Run(argv)
                } else {
                    self.note = Some("left as it was".into());
                    Action::Redraw
                }
            }
            Dialog::Mode { to, pick } => match k.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.dialog = Some(Dialog::Mode { to, pick: (pick + 1).min(MODES.len() - 1) });
                    Action::Redraw
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.dialog = Some(Dialog::Mode { to, pick: pick.saturating_sub(1) });
                    Action::Redraw
                }
                KeyCode::Enter => self.act(
                    format!("switch {to} to shell? What it is sent will be run as commands."),
                    owned(&["set-work-mode", "-t", &to, MODES[pick]]),
                ),
                _ => Action::Redraw,
            },
        }
    }

    /// A mouse event: a click focuses a panel and picks the row under it,
    /// the wheel scrolls the panel under the pointer.
    pub fn mouse(&mut self, m: &MouseRecord) -> Action {
        let a = self.mouse_inner(m);
        self.follow();
        a
    }

    fn mouse_inner(&mut self, m: &MouseRecord) -> Action {
        const MOVED: u32 = 0x1;
        const WHEELED: u32 = 0x4;
        let (x, y) = (m.x.max(0) as usize, m.y.max(0) as usize);
        let layout = self.layout();
        let at = [
            (Panel::Panes, layout.panes),
            (Panel::Inbox, layout.inbox),
            (Panel::Tasks, layout.tasks),
            (Panel::Main, layout.main),
        ]
        .into_iter()
        .find_map(|(p, r)| r.filter(|r| r.holds(x, y)).map(|r| (p, r)));
        let Some((panel, rect)) = at else { return Action::Redraw };
        if m.flags & WHEELED != 0 {
            let up = (m.buttons >> 16) as i16 > 0;
            match panel {
                Panel::Main => self.scroll_by(if up { 3 } else { -3 }),
                Panel::Panes => self.step_pane(if up { -1 } else { 1 }),
                Panel::Inbox => {
                    self.inbox_pick = if up {
                        self.inbox_pick.saturating_sub(1)
                    } else {
                        (self.inbox_pick + 1).min(self.inbox.len().saturating_sub(1))
                    }
                }
                Panel::Tasks => {
                    self.task_pick = if up {
                        self.task_pick.saturating_sub(1)
                    } else {
                        (self.task_pick + 1).min(self.tasks.len().saturating_sub(1))
                    }
                }
            }
            return Action::Redraw;
        }
        if m.flags & MOVED != 0 || m.buttons & 1 == 0 {
            return Action::Redraw;
        }
        self.focus_on(panel);
        let row = y.checked_sub(rect.y + 1).filter(|r| *r < rect.inner_h());
        let Some(row) = row else { return Action::Redraw };
        match panel {
            Panel::Panes => {
                let (start, heading, table) = self.pane_table(rect.inner_h(), rect.inner_w());
                // The heading row is not a pane.
                let row = row.checked_sub(usize::from(heading.is_some()));
                if let Some((Some(id), _)) = row.and_then(|row| table.get(start + row)) {
                    let id = id.clone();
                    if Some(&id) != self.chosen.as_ref() {
                        self.chosen = Some(id);
                        self.scroll = 0;
                        self.inbox_pick = 0;
                    }
                }
            }
            Panel::Inbox => {
                let start = window_start(self.inbox_pick, self.inbox.len(), rect.inner_h());
                if start + row < self.inbox.len() {
                    self.inbox_pick = start + row;
                }
            }
            Panel::Tasks => {
                let start = window_start(self.task_pick, self.tasks.len(), rect.inner_h());
                if start + row < self.tasks.len() {
                    self.task_pick = start + row;
                }
            }
            Panel::Main => {}
        }
        Action::Redraw
    }

    fn layout(&self) -> Layout {
        let (w, h) = (usize::from(self.cols), usize::from(self.height));
        let body = h.saturating_sub(1);
        if w < 12 || body < 6 {
            return Layout { panes: None, inbox: None, tasks: None, main: None };
        }
        let narrow = w < NARROW;
        let (left_w, main) = if narrow {
            (w, (self.focus == Panel::Main).then_some(Rect { x: 0, y: 0, w, h: body }))
        } else {
            // Half, up to 64: the pane list needs the room more than the
            // screen beside it, in a popup above all.
            let lw = (w / 2).clamp(30, 64).min(w - 20);
            (lw, Some(Rect { x: lw, y: 0, w: w - lw, h: body }))
        };
        if narrow && self.focus == Panel::Main {
            return Layout { panes: None, inbox: None, tasks: None, main };
        }
        // Inbox and tasks a quarter each (3 rows at least), the panes the rest.
        let small = (body / 4).max(3);
        let panes_h = body.saturating_sub(2 * small).max(3);
        let inbox_h = small.min(body.saturating_sub(panes_h));
        let tasks_h = body.saturating_sub(panes_h + inbox_h);
        let rect = |y, h| (h >= 3).then_some(Rect { x: 0, y, w: left_w, h });
        Layout { panes: rect(0, panes_h), inbox: rect(panes_h, inbox_h), tasks: rect(panes_h + inbox_h, tasks_h), main }
    }

    /// The pane table in `width` columns: what each column is (a row that
    /// stays on top), the session headings and panes, each line with its
    /// pane, and the first line shown in the `rows` below the heading so
    /// the chosen pane is in view.
    ///
    /// Each column is as wide as what is in it. A column every row shows
    /// only its blank in (no pane named, every pane `normal` and running, no
    /// message waiting) is left out; when the rest do not fit, the ones
    /// least needed go first (quiet, mode, inbox, name, state), so what each
    /// pane runs, an agent's cost and context, keeps 16 columns.
    fn pane_table(&self, rows: usize, width: usize) -> (usize, Option<String>, Rows) {
        const RUNNING: usize = 16;
        let chosen = self.chosen_id();
        let shown = self.shown();
        let quiet_of = |r: &PaneRow| {
            if r.activity > 0 { crate::format::human_duration(self.now - r.activity) } else { String::new() }
        };
        let inbox_of = |r: &PaneRow| if r.inbox > 0 { paint("1;34", &r.inbox.to_string()) } else { String::new() };
        let col = |head, cells, right, keep| Column { head, cells, right, keep };
        let mut cols = vec![col(
            "pane",
            shown.iter().map(|r| format!("{} {}", r.place, paint("2", &r.id))).collect(),
            false,
            u8::MAX,
        )];
        if shown.iter().any(|r| !r.name.is_empty()) {
            cols.push(col("name", shown.iter().map(|r| paint("1", &r.name)).collect(), false, 3));
        }
        if shown.iter().any(|r| r.mode != "normal" || r.dead) {
            cols.push(col("mode", shown.iter().map(|r| r.mode_styled()).collect(), false, 1));
            cols.push(col("state", shown.iter().map(|r| r.state_styled()).collect(), false, 4));
        }
        if shown.iter().any(|r| r.inbox > 0) {
            cols.push(col("inbox", shown.iter().map(|r| inbox_of(r)).collect(), true, 2));
        }
        cols.push(col("quiet", shown.iter().map(|r| quiet_of(r)).collect(), true, 0));
        // The row's marker, then each column and a space; two before what
        // runs, which keeps what its widest needs, up to 16 columns.
        let what_of = |r: &PaneRow| r.agent.as_ref().map(AgentRow::short).unwrap_or_else(|| r.command.clone());
        let running = shown.iter().map(|r| width_of(&what_of(r))).max().unwrap_or(0).clamp(7, RUNNING);
        let used = |cols: &[Column]| 1 + cols.iter().map(|c| c.width() + 1).sum::<usize>() + 1;
        while used(&cols) + running > width {
            let Some(least) = cols.iter().enumerate().filter(|(_, c)| c.keep != u8::MAX).min_by_key(|(_, c)| c.keep)
            else {
                break;
            };
            cols.remove(least.0);
        }
        // What each column is, over it, dim; a blank cell is nothing there,
        // not a sign standing for nothing.
        let heading = (!shown.is_empty()).then(|| {
            let mut head: String = cols.iter().map(|c| c.cell(c.head) + " ").collect();
            head.push_str(" running");
            paint("2", &head)
        });
        let mut table: Rows = Vec::new();
        let mut session = None;
        for (i, r) in shown.iter().enumerate() {
            if session != Some(&r.session) {
                session = Some(&r.session);
                table.push((None, paint("1;34", &r.session)));
            }
            let mut line: String = cols.iter().map(|c| c.cell(&c.cells[i]) + " ").collect();
            line.push_str(&format!(" {}", what_of(r)));
            table.push((Some(r.id.clone()), line));
        }
        let at = table.iter().position(|(id, _)| id.is_some() && *id == chosen).unwrap_or(0);
        let room = rows.saturating_sub(usize::from(heading.is_some()));
        (window_start(at, table.len(), room), heading, table)
    }

    fn panes_box(&self, r: Rect) -> Vec<String> {
        let shown = self.shown();
        let busy = shown.iter().filter(|r| r.mode != "normal" && !r.idle && !r.dead).count();
        let queued: usize = shown.iter().map(|r| r.inbox).sum();
        let filtered = if self.filter.is_empty() { String::new() } else { format!(" · /{}", self.filter) };
        let web = self.web.map(|n| format!(" · web {n} connected")).unwrap_or_default();
        let costs: Vec<f64> = shown.iter().filter_map(|r| r.agent.as_ref()?.dollars()).collect();
        let spent = if costs.is_empty() {
            String::new()
        } else {
            format!(" · {}", crate::format::human_dollars(costs.iter().sum()))
        };
        let title = format!("[1] Panes  {}{spent} · {busy} busy · {queued} queued{web}{filtered}", shown.len());
        let (start, heading, table) = self.pane_table(r.inner_h(), r.inner_w());
        let chosen = self.chosen_id();
        let focused = self.focus == Panel::Panes;
        let mut lines: Vec<String> = heading.iter().map(|h| row_line(h, false, focused, r.inner_w())).collect();
        let room = r.inner_h().saturating_sub(lines.len());
        lines.extend(
            table
                .iter()
                .skip(start)
                .take(room)
                .map(|(id, line)| row_line(line, id.is_some() && *id == chosen, focused, r.inner_w())),
        );
        boxed(&title, "", lines, r, focused)
    }

    fn inbox_box(&self, r: Rect) -> Vec<String> {
        let who = self.chosen_row().map(|p| if p.name.is_empty() { p.id.clone() } else { format!("%{}", p.name) });
        let title = format!("[2] Inbox {} · {}", who.unwrap_or_default(), self.inbox.len());
        let focused = self.focus == Panel::Inbox;
        let start = window_start(self.inbox_pick, self.inbox.len(), r.inner_h());
        let mut lines: Vec<String> = self
            .inbox
            .iter()
            .enumerate()
            .skip(start)
            .take(r.inner_h())
            .map(|(i, m)| {
                // From a pane: its part of the address (a name is short already).
                let from = m.from.split(' ').next().unwrap_or_default().rsplit('.').next().unwrap_or_default();
                let waiting = if waited_long(&m.waiting) { paint("33", &m.waiting) } else { m.waiting.clone() };
                let line = format!(
                    "{} {} {}{} {}{} {}",
                    paint("2", &format!("#{}", m.id)),
                    paint("2", "from"),
                    paint("36", from),
                    paint("2", " ·"),
                    waiting,
                    paint("2", " ·"),
                    m.text
                );
                row_line(&line, i == self.inbox_pick, focused, r.inner_w())
            })
            .collect();
        if lines.is_empty() {
            lines.push("\x1b[2mnothing queued\x1b[0m".into());
        }
        boxed(&title, "", lines, r, focused)
    }

    fn tasks_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Tasks;
        let start = window_start(self.task_pick, self.tasks.len(), r.inner_h());
        let mut lines: Vec<String> = self
            .tasks
            .iter()
            .enumerate()
            .skip(start)
            .take(r.inner_h())
            .map(|(i, t)| row_line(&t.line(), i == self.task_pick, focused, r.inner_w()))
            .collect();
        if lines.is_empty() {
            lines.push("\x1b[2mno tasks\x1b[0m".into());
        }
        boxed(&format!("[3] Tasks · {}", self.tasks.len()), "", lines, r, focused)
    }

    fn main_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Main;
        let tabs = TABS
            .iter()
            .map(|t| if *t == self.tab { format!("\x1b[1;32m{}\x1b[0m", t.word()) } else { t.word().to_string() })
            .collect::<Vec<_>>()
            .join("│");
        let (title, mut head) = match self.chosen_row() {
            Some(p) => {
                let name = if p.name.is_empty() { String::new() } else { format!(" {}", p.name) };
                let up = if p.started > 0 { crate::format::human_duration(self.now - p.started) } else { "?".into() };
                let quiet =
                    if p.activity > 0 { crate::format::human_duration(self.now - p.activity) } else { "?".into() };
                let sep = paint("2", " · ");
                let exit =
                    if p.dead { format!("{sep}{}", paint("31", &format!("exited {}", p.exit))) } else { String::new() };
                let label = |w: &str| paint("2", w);
                let mut head = vec![
                    // Where it is as a person says it (session:window.pane),
                    // what runs, where; the full address last.
                    format!(
                        "{}  {}  {}  {}",
                        paint("1", &format!("{}:{}", p.session, p.place)),
                        p.command,
                        paint("36", &p.path),
                        paint("2", &p.address)
                    ),
                    format!(
                        "{} {}{sep}{} {up}{sep}{} {quiet}{sep}{}{exit}",
                        label("pid"),
                        p.pid,
                        label("up"),
                        label("quiet"),
                        p.size
                    ),
                ];
                head.extend(p.agent.as_ref().map(AgentRow::line));
                head.push(format!("{} {}", label("doing:"), p.doing()));
                // The mode and the state when they say something.
                let mut title = format!("[0] {}{name}", p.id);
                for word in [p.mode.as_str(), p.state()] {
                    if !matches!(word, "normal" | "-" | "") {
                        title.push_str(&format!(" · {word}"));
                    }
                }
                (title, head)
            }
            None => ("[0] no panes".to_string(), Vec::new()),
        };
        if self.tab == Tab::Detail {
            // The view has its own heading.
            head = match &self.detail {
                Some(_) => Vec::new(),
                None => vec!["\x1b[2mPick a message ([2]) or a task ([3]): it shows here\x1b[0m".into()],
            };
        }
        if !(self.tab == Tab::Detail && self.detail.is_some()) {
            head.push(format!("\x1b[2m{}\x1b[0m", "─".repeat(r.inner_w())));
        }
        let room = r.inner_h().saturating_sub(head.len());
        let body: Vec<String> = match (&self.dialog, self.tab) {
            (Some(Dialog::Help), _) => help_lines(),
            (_, Tab::Events) => self.main.iter().map(|l| event_line(l)).collect(),
            // Read in full, laid out: a message's life, a task's steps.
            (_, Tab::Detail) => self.detail_view(r.inner_w()),
            _ => self.main.clone(),
        };
        let shown: Vec<String> = if matches!(self.dialog, Some(Dialog::Help)) || !self.tab.read_up() {
            let from = if matches!(self.dialog, Some(Dialog::Help)) { 0 } else { self.scroll.min(body.len()) };
            body.into_iter().skip(from).take(room).collect()
        } else {
            let end = body.len().saturating_sub(self.scroll);
            let from = end.saturating_sub(room);
            body[from..end].to_vec()
        };
        let mut lines = head;
        lines.extend(shown.into_iter().map(|l| fit(&l, r.inner_w())));
        let title =
            if matches!(self.dialog, Some(Dialog::Help)) { "[?] Keys (any key closes)".to_string() } else { title };
        boxed(&title, &tabs, lines, r, focused)
    }

    /// The bottom line: what is being typed, the question, a note, or the
    /// keys of the focused panel.
    fn foot(&self) -> String {
        if let Some((what, text)) = &self.input {
            let label = match what {
                Input::Filter => "filter panes".to_string(),
                Input::Send { to } => format!("message to {to}"),
                Input::Rename { to } => format!("name for {to}"),
            };
            return format!("\x1b[1m{label}:\x1b[0m {text}\x1b[7m \x1b[0m   Enter ok · Esc cancel");
        }
        match &self.dialog {
            Some(Dialog::Confirm { question, .. }) => {
                return format!("\x1b[1;37;41m {question} \x1b[0m  y yes · any other key no");
            }
            Some(Dialog::Mode { to, pick }) => {
                let modes = MODES
                    .iter()
                    .enumerate()
                    .map(|(i, m)| if i == *pick { format!("\x1b[7m {m} \x1b[0m") } else { format!(" {m} ") })
                    .collect::<String>();
                return format!("\x1b[1mwork mode for {to}:\x1b[0m{modes}  j/k · Enter · Esc");
            }
            _ => {}
        }
        if let Some(n) = &self.note {
            return format!("\x1b[1m {n}\x1b[0m");
        }
        let keys = match self.focus {
            Panel::Panes => {
                "j/k pane  g/G first/last  Enter screen  s send  r rename  m mode  R ready  o go there  x close"
            }
            Panel::Inbox => {
                "j/k pick (shown right)  g/G first/last  Enter scroll it  d delete  K/J move  t to top  u undo delete"
            }
            Panel::Tasks => "j/k pick (shown right)  g/G first/last  Enter scroll it",
            Panel::Main => "j/k scroll  g/G top/bottom  [/] tab  h back",
        };
        let all = "Tab/1230 panel  / filter  ? keys  q quit";
        format!(" {}  {}  {}", key_hints(keys), paint("2", "│"), key_hints(all))
    }

    /// Everything on screen, as the bytes that draw it: every row exactly
    /// the window's width, each put at its row (never a line feed, which on
    /// a window shorter than this board thinks would scroll it).
    pub fn frame(&self) -> String {
        let (w, h) = (usize::from(self.cols), usize::from(self.height));
        let layout = self.layout();
        let body = h.saturating_sub(1);
        let mut rows: Vec<String> = vec![String::new(); body];
        let mut place = |r: Option<Rect>, lines: Vec<String>| {
            if let Some(r) = r {
                for (i, l) in lines.into_iter().enumerate().take(r.h) {
                    if let Some(row) = rows.get_mut(r.y + i) {
                        row.push_str(&l);
                    }
                }
            }
        };
        // Left column first, then the main panel to its right.
        let (p, i, t, m) = (layout.panes, layout.inbox, layout.tasks, layout.main);
        place(p, p.map(|r| self.panes_box(r)).unwrap_or_default());
        place(i, i.map(|r| self.inbox_box(r)).unwrap_or_default());
        place(t, t.map(|r| self.tasks_box(r)).unwrap_or_default());
        place(m, m.map(|r| self.main_box(r)).unwrap_or_default());
        let mut s = String::from("\x1b[?25l");
        for (y, row) in rows.iter().chain(std::iter::once(&self.foot())).enumerate() {
            s.push_str(&format!("\x1b[{};1H", y + 1));
            s.push_str(&fit(row, w));
        }
        s
    }
}

impl Board {
    /// Where an address is, by the pane's name when it has one:
    /// `worker %4` (the name bold), else the address.
    fn who(&self, address: &str) -> String {
        if address == "user" {
            return "\x1b[36muser\x1b[0m".into();
        }
        match self.rows.iter().find(|r| r.address == address) {
            Some(r) if !r.name.is_empty() => format!("\x1b[1m{}\x1b[0m \x1b[2m{}\x1b[0m", r.name, r.id),
            Some(r) => format!("\x1b[1m{}\x1b[0m", r.id),
            None => address.to_string(),
        }
    }

    /// The Detail tab: the record (`-J`) laid out; anything else (an
    /// error) as it came.
    fn detail_view(&self, width: usize) -> Vec<String> {
        let raw = self.main.join("\n");
        let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok();
        match (&self.detail, parsed) {
            (Some(Detail::Message(_)), Some(v)) if v.is_object() => message_view(&v, width, &|a| self.who(a)),
            (Some(Detail::Task(id)), Some(serde_json::Value::Array(steps))) => {
                let row = self.tasks.iter().find(|t| &t.id == id);
                task_view(&steps, row, width, &|a| self.who(a))
            }
            _ => self
                .main
                .iter()
                .flat_map(|l| envelope_fields(l).unwrap_or_else(|| vec![l.clone()]))
                .flat_map(|l| if width_of(&l) > width { wrap(&strip(&l), width) } else { vec![l] })
                .collect(),
        }
    }
}

/// A stage's or status's colour (an SGR number): done green, failed red,
/// on its way yellow, handed on cyan, waiting grey.
fn tone(word: &str) -> &'static str {
    match word {
        "done" => "32",
        "failed" | "rejected" | "dropped" | "abandoned" => "31",
        "running" | "delivered" | "read" => "33",
        "forwarded" => "36",
        _ => "90",
    }
}

/// The word on its colour: ` done ` black on green.
fn badge(word: &str) -> String {
    let bg = match tone(word) {
        "32" => "42",
        "31" => "41",
        "33" => "43",
        "36" => "46",
        _ => "100",
    };
    format!("\x1b[1;30;{bg}m {word} \x1b[0m")
}

/// A section's heading, ruled to the width: `Text ─────`.
fn section(title: &str, width: usize) -> String {
    format!("\x1b[1m{title}\x1b[0m \x1b[2m{}\x1b[0m", "─".repeat(width.saturating_sub(width_of(title) + 1)))
}

/// Text in a block: each line wrapped, set in by two columns.
fn block(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec!["  \x1b[2m(nothing)\x1b[0m".into()];
    }
    text.lines().flat_map(|l| wrap(l, width.saturating_sub(2).max(1))).map(|l| format!("  {l}")).collect()
}

/// A message's life (`trace-message -J`): its number and stage, who sent
/// it where, when each step came, then what it said and what came of it.
fn message_view(v: &serde_json::Value, width: usize, who: &dyn Fn(&str) -> String) -> Vec<String> {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let stage = s("stage");
    let mut out = Vec::new();
    let re = v["re"].as_u64().map(|r| format!(" · answers #{r}")).unwrap_or_default();
    out.push(format!(
        "\x1b[1mMessage #{}\x1b[0m  {}  \x1b[2mtask #{} · hop {}{re}\x1b[0m",
        v["id"],
        badge(&stage),
        v["task"],
        v["hop"]
    ));
    let from = match v["name"].as_str() {
        Some(n) if s("from") != "user" => format!("{} \x1b[2m({n})\x1b[0m", who(&s("from"))),
        _ => who(&s("from")),
    };
    out.push(format!("\x1b[2mfrom\x1b[0m  {from}"));
    out.push(format!("\x1b[2mto  \x1b[0m  {}  \x1b[2mvia {}\x1b[0m", who(&s("to")), s("via")));
    out.push(String::new());
    // The steps it went through, the last in its colour.
    let mut steps: Vec<(String, String, String)> = vec![("sent".into(), s("sent"), String::new())];
    if let Some(d) = v["delivered"].as_str() {
        let how = if v["read"].as_bool() == Some(true) { "read" } else { "delivered" };
        steps.push((how.into(), d.into(), format!("queued {}", s("queued"))));
    }
    if let Some(e) = v["ended"].as_str() {
        let mut note = v["took"].as_str().map(|t| format!("took {t}")).unwrap_or_default();
        match v["ok"].as_bool() {
            Some(true) => note.push_str("  \x1b[32m✓ ok\x1b[0m"),
            Some(false) => note.push_str("  \x1b[31m✗ not ok\x1b[0m"),
            None => {}
        }
        steps.push((stage.clone(), e.into(), note));
    }
    let last = steps.len() - 1;
    for (i, (what, at, note)) in steps.iter().enumerate() {
        let dot = if i == last { format!("\x1b[{}m●\x1b[0m", tone(what)) } else { "\x1b[2m●\x1b[0m".into() };
        let word = if i == last { format!("\x1b[1m{what:<10}\x1b[0m") } else { format!("{what:<10}") };
        out.push(format!(" {dot} {word} {at}  \x1b[2m{note}\x1b[0m"));
    }
    if let Some(why) = v["why"].as_str() {
        out.push(format!("   \x1b[31mwhy\x1b[0m  {why}"));
    }
    out.push(String::new());
    out.push(section("Text", width));
    out.extend(block(&s("text"), width));
    if let Some(o) = v["output"].as_str() {
        out.push(String::new());
        let cut = if v["cut"].as_bool() == Some(true) { " (cut)" } else { "" };
        out.push(section(&format!("Output{cut}"), width));
        out.extend(block(o, width));
    }
    out
}

/// A task's steps (`show-task -J`), each message on a line of its own
/// with what came of it, joined down the left.
fn task_view(
    steps: &[serde_json::Value],
    row: Option<&TaskRow>,
    width: usize,
    who: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    let task = steps.first().map(|v| v["task"].to_string()).unwrap_or_default();
    let (status, took) = row.map(|r| (r.status.clone(), r.took.clone())).unwrap_or_default();
    out.push(format!(
        "\x1b[1mTask #{task}\x1b[0m  {}  \x1b[2m{} step{} · {took}\x1b[0m",
        badge(if status.is_empty() { "?" } else { &status }),
        steps.len(),
        if steps.len() == 1 { "" } else { "s" }
    ));
    if let Some(title) = row.map(|r| r.title.as_str()).filter(|t| !t.is_empty()) {
        out.extend(wrap(title, width).into_iter().map(|l| format!("\x1b[1m{l}\x1b[0m")));
    }
    out.push(String::new());
    for (i, v) in steps.iter().enumerate() {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let stage = s("stage");
        let bar = if i + 1 < steps.len() { "\x1b[2m│\x1b[0m" } else { " " };
        out.push(format!(
            "\x1b[{}m●\x1b[0m {}  \x1b[1m#{}\x1b[0m  {} → {}  {}",
            tone(&stage),
            s("sent"),
            v["id"],
            who(&s("from")),
            who(&s("to")),
            badge(&stage)
        ));
        let mut facts: Vec<String> = Vec::new();
        if let Some(q) = v["queued"].as_str() {
            facts.push(format!("queued {q}"));
        }
        if let Some(t) = v["took"].as_str() {
            facts.push(format!("took {t}"));
        }
        match v["ok"].as_bool() {
            Some(true) => facts.push("\x1b[32m✓\x1b[0m\x1b[2m".into()),
            Some(false) => facts.push("\x1b[31m✗\x1b[0m\x1b[2m".into()),
            None => {}
        }
        facts.push(format!("via {}", s("via")));
        out.push(format!("{bar} \x1b[2m{}\x1b[0m", facts.join(" · ")));
        let room = width.saturating_sub(4).max(1);
        let text = s("text");
        for l in text.lines().take(3).flat_map(|l| wrap(l, room)).take(3) {
            out.push(format!("{bar}   {l}"));
        }
        if text.lines().count() > 3 {
            out.push(format!("{bar}   \x1b[2m…\x1b[0m"));
        }
        if let Some(o) = v["output"].as_str().filter(|o| !o.trim().is_empty()) {
            let first = o.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
            out.push(format!("{bar}   \x1b[2m→ {}\x1b[0m", clip(first, room.saturating_sub(2)).0));
        }
        if let Some(why) = v["why"].as_str() {
            out.push(format!("{bar}   \x1b[31m{why}\x1b[0m"));
        }
        if i + 1 < steps.len() {
            out.push(bar.to_string());
        }
    }
    out
}

/// A message's header, in either of its forms (`[keepane id=3 from=… …]`,
/// or the JSON envelope), one field a line: the name, then the value. The
/// protocol's version (`keepane`) is left out; None for any other line.
fn envelope_fields(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let pairs: Vec<(String, String)> =
        if let Some(inner) = line.strip_prefix("[keepane ").and_then(|l| l.strip_suffix(']')) {
            inner
                .split(' ')
                .map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect::<Option<_>>()?
        } else if line.starts_with('{') {
            let serde_json::Value::Object(map) = serde_json::from_str::<serde_json::Value>(line).ok()? else {
                return None;
            };
            map.into_iter()
                .filter(|(k, _)| k != "keepane")
                .map(|(k, v)| {
                    let value = match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, value)
                })
                .collect()
        } else {
            return None;
        };
    let width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    Some(pairs.into_iter().map(|(k, v)| format!("\x1b[36m{k:<width$}\x1b[0m  {v}")).collect())
}

/// A plain line in rows of at most `width` columns.
fn wrap(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut rows = vec![String::new()];
    let mut used = 0;
    for c in line.chars() {
        let w = if c.is_control() { 1 } else { c.width().unwrap_or(0) };
        if used + w > width {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(c);
        used += w;
    }
    rows
}

fn owned(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|x| x.to_string()).collect()
}

/// The first of `len` lines to show in `rows` so that line `at` is in view.
fn window_start(at: usize, len: usize, rows: usize) -> usize {
    if rows == 0 || len <= rows {
        return 0;
    }
    at.saturating_sub(rows - 1).min(len - rows)
}

/// A list line, marked when it is the one picked: reversed in the focused
/// panel, bold with a marker otherwise.
fn row_line(line: &str, picked: bool, focused: bool, width: usize) -> String {
    let text = format!("{}{line}", if picked && !focused { ">" } else { " " });
    if picked && focused {
        // Reverse video through the whole row; a reset inside the line
        // would end it, so the line's own styles go.
        let plain = strip(&text);
        format!("\x1b[7m{}\x1b[0m", fit(&plain, width))
    } else if picked {
        format!("\x1b[1m{}\x1b[0m", fit(&text, width))
    } else {
        fit(&text, width)
    }
}

/// `s` without its escape sequences.
fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&d) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// A panel: a border with its title (and, on the right, `right`), `lines`
/// inside, all in `r`. The focused panel's border is green.
fn boxed(title: &str, right: &str, lines: Vec<String>, r: Rect, focused: bool) -> Vec<String> {
    let (w, inner) = (r.w, r.inner_w());
    let border = if focused { "\x1b[1;32m" } else { "\x1b[2m" };
    // The right-hand text (the tabs) only where the title keeps some room.
    let right_w = width_of(right);
    let with_right = right_w > 0 && inner >= right_w + 12;
    let title_room = if with_right { inner - right_w - 2 } else { inner };
    let (text, text_w) = clip(&format!("─{title} "), title_room);
    let fill = "─".repeat(title_room - text_w);
    // The title in full colour whatever the border: its number blue, the
    // rest bold.
    let text = match text.strip_prefix('─').and_then(|t| t.split_once(']')) {
        Some((num, rest)) => format!("─\x1b[0m{}{}{border}", paint("1;34", &format!("{num}]")), paint("1", rest)),
        None => text,
    };
    let top = if with_right {
        format!("{border}┌{text}{fill}\x1b[0m {right} {border}┐\x1b[0m")
    } else {
        format!("{border}┌{text}{fill}┐\x1b[0m")
    };
    let mut out = vec![fit(&top, w)];
    for i in 0..r.inner_h() {
        let l = lines.get(i).map(String::as_str).unwrap_or("");
        out.push(format!("{border}│\x1b[0m{}{border}│\x1b[0m", fit(l, inner)));
    }
    out.push(format!("{border}└{}┘\x1b[0m", "─".repeat(inner)));
    out
}

fn help_lines() -> Vec<String> {
    [
        "Everywhere",
        "  Tab / Shift+Tab, 1 2 3 0, h / l   change panel",
        "  [ / ]                             change the tab on the right",
        "  PgUp / PgDn, C-b / C-f            page the right panel (C-u / C-d half a page;",
        "                                    in the prefix-v popup, C-b C-b sends C-b)",
        "  g / G, Home / End                 the first / last row of a list; top / bottom on the right",
        "  /                                 filter the panes",
        "  q, Esc                            quit (Esc first cancels a question)",
        "  mouse                             click a panel or a row; the wheel scrolls",
        "",
        "[1] Panes",
        "  j / k      pick a pane             Enter   its screen",
        "  s          send it a message       r       rename it",
        "  m          its work mode           R       mark it ready (unstick it)",
        "  o          go there                x       close it (asks; prefix u undoes)",
        "",
        "[2] Inbox (the chosen pane's queued messages)",
        "  j / k      pick one (shown right)  Enter   scroll it",
        "  d          delete it (asks)        u       bring the last deleted back",
        "  K / J      move it up / down       t       put it first",
        "",
        "[3] Tasks    j / k pick (its steps shown right), Enter scroll them",
        "",
        "[0] The chosen pane: Screen, Scrollback, Events, Detail",
        "  j / k      scroll                  g / G   top / bottom",
        "",
        "Asks first: deleting a message, closing a pane, switching a pane to shell.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// An event log line, short: time, what, the gist.
pub fn event_line(json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return json.to_string() };
    let at = v["at"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_default();
    let ev = v["ev"].as_str().unwrap_or_default();
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let gist = match ev {
        "sent" | "rejected" => {
            let m = &v["msg"];
            let from = m["name"]
                .as_str()
                .map(|n| format!("%{n}"))
                .unwrap_or_else(|| m["from"].as_str().unwrap_or("").to_string());
            let text = s("text");
            let first = text.lines().next().unwrap_or_default();
            let why = if ev == "rejected" { format!("  ({})", s("why")) } else { String::new() };
            format!(
                "#{} {from} → {} ({}): {first}{why}",
                m["id"],
                m["to"].as_str().unwrap_or(""),
                m["via"].as_str().unwrap_or("")
            )
        }
        "pane" => {
            let what = s("what");
            let value = [s("value"), s("text")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
            let by = s("by");
            format!("{what} {value}{}", if by.is_empty() { String::new() } else { format!("  by {by}") })
        }
        _ => {
            let mut g = format!("#{}", v["id"]);
            if let Some(ok) = v["ok"].as_bool() {
                g.push_str(if ok { " ok" } else { " failed" });
            }
            for k in ["why", "by"] {
                if !s(k).is_empty() {
                    g.push_str(&format!("  {}", s(k)));
                }
            }
            g
        }
    };
    let colour = match ev {
        "failed" | "rejected" | "dropped" | "abandoned" => "\x1b[31m",
        "done" => "\x1b[32m",
        "sent" | "delivered" => "\x1b[36m",
        _ => "",
    };
    let reset = if colour.is_empty() { "" } else { "\x1b[0m" };
    format!("{at}  {colour}{ev:<9}{reset} {gist}")
}

/// `keepane dashboard [--popup]` (`--popup`: opened by prefix v, so going
/// to a pane closes it).
pub fn run(socket: &str, rt: &tokio::runtime::Runtime, popup: bool) -> anyhow::Result<i32> {
    use crate::console::{Console, InputEvent};
    let mut console = Console::open()?;
    console.enter_raw()?;
    console.set_mouse(true);
    let console = std::sync::Arc::new(console);
    let (tx, rx) = std::sync::mpsc::channel::<InputEvent>();
    {
        let c = console.clone();
        std::thread::spawn(move || {
            while let Ok(events) = c.read_events() {
                for e in events {
                    if tx.send(e).is_err() {
                        return;
                    }
                }
            }
        });
    }
    let ask = |argv: &[String]| -> (i32, String, String) {
        debug_assert!(argv.first().is_some_and(|c| QUERIES.contains(&c.as_str()) || ACTIONS.contains(&c.as_str())));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        rt.block_on(crate::client::query(socket, &argv)).unwrap_or_else(|e| (1, String::new(), format!("{e:#}")))
    };
    let lines = |(_, out, err): (i32, String, String)| -> Vec<String> {
        (if out.is_empty() { err } else { out }).lines().map(String::from).collect()
    };
    let (cols, rows) = console.size();
    let mut board = Board::new(cols, rows);
    let result = (|| -> anyhow::Result<()> {
        loop {
            let (code, out, err) = ask(&owned(&["list-panes", "-a", "-F", PANE_FORMAT]));
            if code != 0 {
                anyhow::bail!("{}", err.trim());
            }
            board.now = chrono::Utc::now().timestamp();
            board.set_rows(out.lines().filter_map(PaneRow::parse).collect());
            match board.inbox_query() {
                Some(q) => board.set_inbox(&lines(ask(&q))),
                None => board.set_inbox(&[]),
            }
            board.set_tasks(&lines(ask(&board.tasks_query())));
            board.set_web(&ask(&owned(&["web-status"])).1);
            // The window's size, every time: a resize can pass unannounced
            // (a popup laid out again).
            let (cols, rows) = console.size();
            if (cols, rows) != (board.cols, board.height) {
                board.resize(cols, rows);
                console.write_str("\x1b[2J");
            }
            match board.main_query() {
                Some(q) => board.set_main(lines(ask(&q))),
                None => board.set_main(Vec::new()),
            }
            console.write_str(&board.frame());
            // A key, or a second without one: then the board is read again.
            let first = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(e) => Some(e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return Ok(()),
            };
            for ev in first.into_iter().chain(rx.try_iter()) {
                let action = match ev {
                    InputEvent::Key(k) => match crate::keys::key_from_record(&k) {
                        Some(key) => board.key(key),
                        None => continue,
                    },
                    InputEvent::Mouse(m) => board.mouse(&m),
                    InputEvent::Resize => {
                        let (cols, rows) = console.size();
                        board.resize(cols, rows);
                        console.write_str("\x1b[2J");
                        Action::Redraw
                    }
                };
                match action {
                    Action::Quit => return Ok(()),
                    Action::Run(argv) | Action::Go(argv) if !popup || argv[0] != "focus-pane" => {
                        let (code, out, err) = ask(&argv);
                        board.note = Some(if code == 0 {
                            let out = out.trim();
                            if out.is_empty() {
                                format!("done: {}", argv.join(" "))
                            } else {
                                out.lines().next().unwrap_or(out).to_string()
                            }
                        } else {
                            err.trim().to_string()
                        });
                    }
                    Action::Run(argv) | Action::Go(argv) => {
                        // From the popup: go there, and out of the way.
                        let (code, _, err) = ask(&argv);
                        if code == 0 {
                            return Ok(());
                        }
                        board.note = Some(err.trim().to_string());
                    }
                    Action::Redraw => {}
                }
            }
        }
    })();
    console.restore();
    result.map(|()| 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, session: &str, name: &str, mode: &str, idle: bool, inbox: usize) -> PaneRow {
        PaneRow {
            address: format!("$1:@1.{id}"),
            session: session.into(),
            place: "0.0".into(),
            id: id.into(),
            name: name.into(),
            mode: mode.into(),
            idle,
            inbox,
            command: "pwsh".into(),
            path: "/home/me/src".into(),
            pid: "4242".into(),
            started: 1_000,
            activity: 1_900,
            size: "80x24".into(),
            ..Default::default()
        }
    }

    fn board() -> Board {
        let mut b = Board::new(120, 32);
        b.now = 2_000;
        b.set_rows(vec![
            row("%1", "work", "lead", "ai", false, 0),
            row("%2", "work", "tester", "ai", true, 2),
            row("%3", "ops", "", "normal", false, 0),
        ]);
        b
    }

    fn inbox(b: &mut Board) {
        b.set_inbox(&[
            "$1:@1.%2 tester (ai, idle) · 2 queued".into(),
            "  #5  from user  waiting 1s  first".into(),
            "  #6  from %lead  waiting 3s  second  with  gaps".into(),
        ]);
    }

    fn every_key() -> Vec<Key> {
        let mut keys: Vec<Key> = (' '..='~').map(Key::ch).collect();
        for code in [
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Tab,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::PPage,
            KeyCode::NPage,
            KeyCode::BSpace,
            KeyCode::Home,
            KeyCode::End,
        ] {
            keys.push(Key::plain(code));
        }
        keys.push(Key::with_shift(KeyCode::Tab));
        keys.extend(['b', 'f', 'u', 'd'].map(Key::ctrl));
        keys
    }

    /// Every row of a frame, without its escape sequences.
    fn screen(b: &Board) -> Vec<String> {
        let f = b.frame();
        // Rows start where the frame puts the cursor: ESC [ row ; 1 H, in order.
        let mut rows = Vec::new();
        let mut rest = f.trim_start_matches("\x1b[?25l");
        let mut n = 1;
        while let Some(r) = rest.strip_prefix(&format!("\x1b[{n};1H")) {
            let end = r.find(&format!("\x1b[{};1H", n + 1)).unwrap_or(r.len());
            rows.push(strip(&r[..end]));
            rest = &r[end..];
            n += 1;
        }
        assert!(rest.is_empty(), "every row put at its place: {rest:?}");
        assert!(!f.contains('\n'), "no line feed");
        rows
    }

    #[test]
    fn a_pane_line_reads_back() {
        let line =
            "$1:@2.%7\twork\t1.0\t%7\ttester\tai\t1\t2\trunning 3/10\tclaude\t12\t0\t/src\t99\t100\t150\t80\t24\t";
        let r = PaneRow::parse(line).unwrap();
        assert_eq!((r.id.as_str(), r.idle, r.inbox, r.doing().as_str()), ("%7", true, 2, "\"running 3/10\""));
        assert_eq!(
            (r.path.as_str(), r.pid.as_str(), r.started, r.activity, r.size.as_str()),
            ("/src", "99", 100, 150, "80x24")
        );
        assert!(!r.unheard, "a line from an older server has no such field");
        // Messages waiting on an agent never heard from: the hook it lacks,
        // before what it said; with none waiting, as before.
        let unheard = format!("{line}\t1");
        let r = PaneRow::parse(&unheard).unwrap();
        assert_eq!(r.doing(), "never said it is free: keepane setup");
        let r = PaneRow::parse(&unheard.replace("\t1\t2\t", "\t1\t0\t")).unwrap();
        assert_eq!(r.doing(), "\"running 3/10\"");
        assert_eq!(PaneRow::parse("too\tshort"), None);
        let q = Queued::parse("  #6  from %lead  waiting 3s  second  with  gaps").unwrap();
        assert_eq!(
            (q.id.as_str(), q.from.as_str(), q.waiting.as_str(), q.text.as_str()),
            ("6", "%lead", "3s", "second  with  gaps")
        );
        assert_eq!(Queued::parse("$1:@1.%2 tester (ai) · 2 queued"), None);
    }

    /// The one rule of what it may send: only its queries and actions, and
    /// what needs a yes never without one, whatever is pressed.
    #[test]
    fn nothing_leaves_but_its_actions_and_nothing_harmful_without_a_yes() {
        for focus in [Panel::Panes, Panel::Inbox, Panel::Tasks, Panel::Main] {
            for k in every_key() {
                let mut b = board();
                inbox(&mut b);
                b.set_tasks(&["TASK  STATUS".into(), "#5  running  x".into()]);
                b.focus = focus;
                match b.key(k) {
                    Action::Run(argv) | Action::Go(argv) => {
                        assert!(ACTIONS.contains(&argv[0].as_str()), "{focus:?} {k:?} -> {argv:?}");
                        assert!(!needs_yes(&argv), "{focus:?} {k:?} ran {argv:?} without asking");
                    }
                    _ => {}
                }
                // Whatever question it asked: only `y` goes on.
                for answer in every_key().into_iter().filter(|a| *a != Key::ch('y')) {
                    let mut again = board();
                    inbox(&mut again);
                    again.focus = focus;
                    again.key(k);
                    if again.dialog.is_some() && matches!(again.dialog, Some(Dialog::Confirm { .. })) {
                        assert_eq!(again.key(answer), Action::Redraw, "{k:?} then {answer:?}");
                    }
                }
            }
        }
        for q in [board().main_query().unwrap(), board().inbox_query().unwrap(), board().tasks_query()] {
            assert!(QUERIES.contains(&q[0].as_str()), "{q:?}");
        }
    }

    #[test]
    fn a_pane_is_sent_renamed_moded_readied_and_closed() {
        let mut b = board();
        b.key(Key::ch('j'));
        assert_eq!(b.chosen.as_deref(), Some("%2"));
        // A message, typed at the bottom.
        b.key(Key::ch('s'));
        for c in "run tests".chars() {
            b.key(Key::ch(c));
        }
        assert!(strip(&b.frame()).contains("message to %2: run tests"));
        assert_eq!(
            b.key(Key::plain(KeyCode::Enter)),
            Action::Run(owned(&["send-message", "-t", "%2", "--", "run tests"]))
        );
        // A name, starting from the one it has; Esc changes nothing.
        b.key(Key::ch('r'));
        b.key(Key::plain(KeyCode::BSpace));
        assert_eq!(b.key(Key::plain(KeyCode::Escape)), Action::Redraw);
        b.key(Key::ch('r'));
        b.key(Key::ch('2'));
        assert_eq!(
            b.key(Key::plain(KeyCode::Enter)),
            Action::Run(owned(&["rename-pane", "-t", "%2", "--", "tester2"]))
        );
        // A mode: ai at once; shell only after a yes.
        b.key(Key::ch('m'));
        assert_eq!(b.key(Key::plain(KeyCode::Enter)), Action::Run(owned(&["set-work-mode", "-t", "%2", "ai"])));
        b.key(Key::ch('m'));
        b.key(Key::ch('k'));
        assert_eq!(b.key(Key::plain(KeyCode::Enter)), Action::Redraw, "shell asks first");
        assert!(strip(&b.frame()).contains("switch %2 to shell?"));
        assert_eq!(b.key(Key::ch('y')), Action::Run(owned(&["set-work-mode", "-t", "%2", "shell"])));
        assert_eq!(b.key(Key::ch('R')), Action::Run(owned(&["pane-ready", "-t", "%2"])));
        assert_eq!(b.key(Key::ch('o')), Action::Go(owned(&["focus-pane", "%2"])));
        // Closing asks; no leaves it.
        assert_eq!(b.key(Key::ch('x')), Action::Redraw);
        assert!(strip(&b.frame()).contains("close %2 (tester)?"));
        assert_eq!(b.key(Key::ch('n')), Action::Redraw);
        assert_eq!(b.note.as_deref(), Some("left as it was"));
        b.key(Key::ch('x'));
        assert_eq!(b.key(Key::ch('y')), Action::Run(owned(&["kill-pane", "-t", "%2"])));
    }

    #[test]
    fn queued_messages_are_read_moved_and_deleted_with_a_yes() {
        let mut b = board();
        b.key(Key::ch('j'));
        assert_eq!(b.inbox_query().unwrap(), owned(&["list-messages", "-t", "%2"]));
        inbox(&mut b);
        b.key(Key::ch('2'));
        assert_eq!(b.focus, Panel::Inbox);
        b.key(Key::ch('j'));
        assert_eq!(b.key(Key::ch('t')), Action::Run(owned(&["move-message", "6", "top"])));
        assert_eq!(b.inbox_pick, 0, "the pick goes with the message to the top");
        b.inbox_pick = 1;
        assert_eq!(b.key(Key::ch('K')), Action::Run(owned(&["move-message", "6", "up"])));
        assert_eq!(b.key(Key::ch('J')), Action::Run(owned(&["move-message", "5", "down"])));
        assert_eq!(b.key(Key::ch('d')), Action::Redraw);
        assert_eq!(b.key(Key::ch('y')), Action::Run(owned(&["drop-message", "6"])));
        assert_eq!(b.key(Key::ch('u')), Action::Run(owned(&["drop-message", "-u"])));
        // On it: shown on the right; Enter goes there, to scroll it.
        b.inbox_pick = 0;
        b.key(Key::plain(KeyCode::Enter));
        assert_eq!((b.focus, b.tab), (Panel::Main, Tab::Detail));
        assert_eq!(b.main_query().unwrap(), owned(&["trace-message", "5", "-J"]));
        // A task likewise.
        b.set_tasks(&["TASK  STATUS   AT".into(), "#12   running  $1:@1.%2 (ai)  2s  2  run".into()]);
        b.key(Key::ch('3'));
        b.key(Key::plain(KeyCode::Enter));
        assert_eq!(b.main_query().unwrap(), owned(&["show-task", "12", "-J"]));
    }

    /// The cursor on a message or a task shows it on the right at once;
    /// back on the panes, the tab that was there. A tab picked by hand stays.
    #[test]
    fn the_cursor_shows_its_message_or_task_on_the_right() {
        let mut b = board();
        b.key(Key::ch('j'));
        inbox(&mut b);
        assert_eq!(b.tab, Tab::Screen);
        b.key(Key::ch('2'));
        assert_eq!((b.focus, b.tab), (Panel::Inbox, Tab::Detail), "no Enter needed");
        assert_eq!(b.main_query().unwrap(), owned(&["trace-message", "5", "-J"]));
        b.key(Key::ch('j'));
        assert_eq!(b.main_query().unwrap(), owned(&["trace-message", "6", "-J"]), "it follows the cursor");
        b.key(Key::ch('1'));
        assert_eq!((b.focus, b.tab), (Panel::Panes, Tab::Screen), "back to the screen");
        // A tab changed by hand while on the list stays.
        b.key(Key::ch('2'));
        assert_eq!(b.tab, Tab::Detail);
        b.key(Key::ch(']'));
        b.key(Key::ch(']'));
        assert_eq!(b.tab, Tab::Scrollback);
        b.key(Key::ch('1'));
        assert_eq!(b.tab, Tab::Scrollback);
        // Tasks likewise.
        b.set_tasks(&[
            "TASK  STATUS   AT".into(),
            "#12   running  $1:@1.%2 (ai)  2s  2  run".into(),
            "#9    done     -  1s  1  ls".into(),
        ]);
        b.key(Key::ch('3'));
        assert_eq!(b.main_query().unwrap(), owned(&["show-task", "12", "-J"]));
        b.key(Key::ch('j'));
        assert_eq!(b.main_query().unwrap(), owned(&["show-task", "9", "-J"]));
        // An inbox emptied under the cursor: nothing to follow, nothing breaks.
        b.key(Key::ch('2'));
        b.set_inbox(&[]);
        assert!(strip(&b.frame()).contains("nothing queued"));
    }

    /// The Detail tab lays a record out: a message's stage, who to whom, its
    /// steps, then Text and Output apart (a line `output:` in the text stays
    /// text); a task's steps, joined.
    #[test]
    fn a_message_and_a_task_read_as_laid_out() {
        let mut b = board();
        b.key(Key::ch('j'));
        inbox(&mut b);
        b.key(Key::ch('2'));
        let msg = serde_json::json!({
            "id": 5, "task": 5, "hop": 0, "stage": "done", "from": "user", "name": null,
            "to": "$1:@1.%2", "via": "shell", "re": null, "sent": "12:00:01", "delivered": "12:00:02",
            "read": false, "ended": "12:00:03", "queued": "1s", "took": "1s", "ok": true, "why": null,
            "text": "echo one\noutput:\necho two", "output": "one\ntwo", "cut": false
        });
        b.set_main(vec![msg.to_string()]);
        let screen = strip(&b.frame());
        for want in [
            "Message #5",
            " done ",
            "task #5",
            "from  user",
            "tester %2",
            "via shell",
            "● sent",
            "● delivered",
            "● done",
            "took 1s",
            "✓ ok",
            "Text ─",
            "Output ─",
        ] {
            assert!(screen.contains(want), "{want}:\n{screen}");
        }
        let text_at = screen.find("Text ─").unwrap();
        let out_at = screen.find("Output ─").unwrap();
        let inside = &screen[text_at..out_at];
        assert!(inside.contains("output:") && inside.contains("echo two"), "the text whole, before Output:\n{screen}");
        assert!(screen[out_at..].contains("two"));
        // Colour: the stage on green, nothing raw left.
        assert!(b.frame().contains("\x1b[1;30;42m done "));
        assert!(!screen.contains("{\"id\""), "no JSON shown");
        // A task.
        b.set_tasks(&["TASK  STATUS   AT".into(), "#12   failed   -  2s  2  review it".into()]);
        b.key(Key::ch('3'));
        let step = |id: u64, stage: &str, ok: bool| {
            serde_json::json!({
                "id": id, "task": 12, "hop": 0, "stage": stage, "from": "user", "name": null, "to": "$1:@1.%2",
                "via": "shell", "re": null, "sent": "12:00:01", "delivered": "12:00:01", "read": false,
                "ended": "12:00:02", "queued": "0ms", "took": "1s", "ok": ok, "why": null,
                "text": format!("step {id}"), "output": "first line\nsecond", "cut": false
            })
        };
        b.set_main(vec![serde_json::Value::Array(vec![step(12, "done", true), step(13, "failed", false)]).to_string()]);
        let screen = strip(&b.frame());
        for want in [
            "Task #12",
            " failed ",
            "2 steps",
            "review it",
            "#12",
            "#13",
            "user → tester %2",
            "step 12",
            "→ first line",
            "✗",
        ] {
            assert!(screen.contains(want), "{want}:\n{screen}");
        }
        // An error instead of a record: shown as it came.
        b.set_main(vec!["no task #99".into()]);
        assert!(strip(&b.frame()).contains("no task #99"));
    }

    /// `g` / `G` (and Home / End) go to the first / last row of whichever
    /// list has the focus, as they go to the top / bottom on the right.
    #[test]
    fn g_and_shift_g_go_to_the_first_and_last_row() {
        let mut b = board();
        inbox(&mut b);
        b.set_tasks(&["#1  running  one".into(), "#2  done  two".into(), "#3  failed  three".into()]);
        for (last, first) in [(Key::ch('G'), Key::ch('g')), (Key::plain(KeyCode::End), Key::plain(KeyCode::Home))] {
            b.focus = Panel::Panes;
            b.key(last);
            assert_eq!(b.chosen_row().unwrap().id, "%3", "{last:?}");
            b.key(first);
            assert_eq!(b.chosen_row().unwrap().id, "%1", "{first:?}");
            b.focus = Panel::Tasks;
            b.key(last);
            assert_eq!(b.task_pick, 2);
            b.key(first);
            assert_eq!(b.task_pick, 0);
        }
        // The inbox is the chosen pane's: %2 has the two messages.
        b.focus = Panel::Panes;
        b.key(Key::ch('j'));
        inbox(&mut b);
        b.focus = Panel::Inbox;
        b.key(Key::ch('Ｇ'));
        assert_eq!(b.inbox_pick, 1, "Ｇ from an input method is G");
        b.key(Key::ch('g'));
        assert_eq!(b.inbox_pick, 0);
        assert!(strip(&b.frame()).contains("g/G first/last"), "the bottom line says so");
    }

    /// With a Chinese input method on: `】` is `]` (the next tab), `ｑ` is
    /// `q`; a message being typed keeps its `【` as typed.
    #[test]
    fn full_width_keys_are_the_boards_keys_but_text_stays() {
        let mut b = board();
        b.focus = Panel::Main;
        b.key(Key::ch('】'));
        assert_eq!(b.tab, Tab::Scrollback, "】 went to the next tab");
        b.focus = Panel::Panes;
        b.key(Key::ch('ｓ'));
        for c in "【急】ｔｅｓｔ".chars() {
            b.key(Key::ch(c));
        }
        let sent = b.key(Key::plain(KeyCode::Enter));
        assert!(
            matches!(&sent, Action::Run(argv) if argv.last().map(String::as_str) == Some("【急】ｔｅｓｔ")),
            "the message as typed: {sent:?}"
        );
        assert!(matches!(b.key(Key::ch('ｑ')), Action::Quit));
    }

    #[test]
    fn panels_and_tabs_are_gone_through() {
        let mut b = board();
        let order: Vec<Panel> = (0..4)
            .map(|_| {
                b.key(Key::plain(KeyCode::Tab));
                b.focus
            })
            .collect();
        assert_eq!(order, [Panel::Inbox, Panel::Tasks, Panel::Main, Panel::Panes]);
        b.key(Key::with_shift(KeyCode::Tab));
        assert_eq!(b.focus, Panel::Main);
        b.key(Key::ch('h'));
        assert_eq!(b.focus, Panel::Panes, "h goes back to the left panel it came from");
        b.key(Key::ch('3'));
        b.key(Key::ch('l'));
        b.key(Key::ch('h'));
        assert_eq!(b.focus, Panel::Tasks);
        for (k, want) in [(']', "capture-pane"), (']', "list-events"), ('[', "capture-pane"), ('[', "capture-pane")] {
            b.key(Key::ch(k));
            assert_eq!(b.main_query().unwrap()[0], want, "{:?}", b.tab);
        }
        assert_eq!(b.tab, Tab::Screen);
        assert!(b.main_query().unwrap().contains(&"-e".to_string()), "the screen with its colours");
        // The filter narrows the panes; the chosen one follows.
        b.key(Key::ch('/'));
        for c in "ops".chars() {
            b.key(Key::ch(c));
        }
        b.key(Key::plain(KeyCode::Enter));
        assert_eq!(b.chosen_row().unwrap().id, "%3");
        assert!(strip(&b.frame()).contains("[1] Panes  1 · 0 busy"));
    }

    /// Whatever the size, every row is exactly the window's width, nothing
    /// panics, and what fits is there.
    #[test]
    fn every_size_draws_rows_of_the_window_width() {
        for (cols, rows) in [(120, 32), (80, 24), (69, 20), (40, 10), (12, 7), (11, 5), (1, 1), (0, 0), (300, 90)] {
            for focus in [Panel::Panes, Panel::Main] {
                let mut b = board();
                inbox(&mut b);
                b.set_tasks(&["#1  running  a very long title ".repeat(8)]);
                b.set_main(vec!["\x1b[31mred\x1b[0m 中文字符 wide".repeat(10), "\x1b]0;title\x07plain".into()]);
                b.resize(cols, rows);
                b.focus = focus;
                // The same board, the same bytes.
                assert_eq!(b.frame(), b.frame());
                let lines = screen(&b);
                assert_eq!(lines.len(), usize::from(rows).max(1), "{cols}x{rows}");
                for l in &lines {
                    assert_eq!(width_of(l), usize::from(cols), "{cols}x{rows} {focus:?}: {l:?}");
                }
            }
        }
        let b = board();
        let text = screen(&b).join("\n");
        for want in [
            "[1] Panes  3 · 1 busy · 2 queued",
            "[2] Inbox %lead",
            "[3] Tasks",
            "[0] %1 lead · ai · busy",
            "Screen│Scrollback",
            "pid 4242 · up 16m · quiet 1m",
            "tester",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
    }

    /// A pane's agent: its cost and context in the table, all of it over
    /// the chosen pane, the costs summed in the title; a line without the
    /// fields (an older server) or with none (no agent) has no agent.
    #[test]
    fn agents_show_their_cost_and_context() {
        let line = "$1:@2.%7\twork\t1.0\t%7\ttester\tai\t1\t0\t\tclaude\t\t0\t/src\t99\t100\t150\t80\t24\t\t0";
        assert_eq!(PaneRow::parse(line).unwrap().agent, None, "an older server");
        let none = format!("{line}\t\t\t\t\t\t\t\t\t");
        assert_eq!(PaneRow::parse(&none).unwrap().agent, None, "no agent");
        let with = format!("{line}\tclaude\tclaude-opus-5-5\t$2.10\t1.2M\t34%\t12%\t812M\t42\t10");
        let a = PaneRow::parse(&with).unwrap().agent.unwrap();
        assert_eq!((a.kind.as_str(), a.cost.as_str(), a.dollars()), ("claude", "$2.10", Some(2.1)));
        assert_eq!(strip(&a.short()), "claude $2.10 34%");
        let unknown = AgentRow { kind: "codex".into(), ..AgentRow::default() };
        assert_eq!(
            (strip(&unknown.short()).as_str(), unknown.dollars()),
            ("codex", None),
            "nothing known, nothing said"
        );
        let partial = AgentRow { cost: "$1.25+".into(), ..AgentRow::default() };
        assert_eq!(partial.dollars(), Some(1.25), "at least: counted in the total");
        let mut b = board();
        b.resize(200, 30);
        let mut rows = b.rows.clone();
        rows[0].agent = Some(a.clone());
        rows[1].agent = Some(AgentRow { cost: "$0.40".into(), ..a.clone() });
        rows[2].agent = Some(unknown);
        b.set_rows(rows);
        let text = screen(&b).join("\n");
        for want in [
            "[1] Panes  3 · $2.50 · 1 busy · 2 queued",
            "claude $2.10 34%",
            "claude claude-opus-5-5 · $2.10 · context 34% · 1.2M tokens · cpu 12% · mem 812M · 42 tools · 10 turns",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
        assert!(b.rows[2].matches("codex") && b.rows[0].matches("opus"), "found by agent and model");
    }

    /// Panes all unnamed and `normal`: those columns are left out, and in
    /// a narrow popup each agent's cost and context still show.
    #[test]
    fn a_narrow_board_keeps_room_for_the_agents() {
        let mut b = board();
        let agent =
            AgentRow { kind: "claude".into(), cost: "$0.58".into(), context: "9%".into(), ..AgentRow::default() };
        let mut rows = b.rows.clone();
        for r in &mut rows {
            r.name.clear();
            r.mode = "normal".into();
            r.agent = Some(agent.clone());
        }
        b.set_rows(rows);
        // The popup in a 96x26 terminal: prefix v opens it at 90%, inside
        // its own border (86 - 2 by 23 - 2).
        b.resize(84, 21);
        let text = screen(&b).join("\n");
        assert_eq!(text.matches("claude $0.58 9%").count(), 3, "{text}");
        // Named or moded panes keep their columns, as before.
        let b = board();
        let text = screen(&b).join("\n");
        assert!(text.contains("lead") && text.contains(" ai "), "{text}");
    }

    /// Readable at a glance: the columns are named over them, a blank is a
    /// blank (no `·` or `-` standing for nothing), and each colour says one
    /// thing: free, busy, the mode, messages waiting, how full a context is.
    #[test]
    fn the_board_says_what_its_columns_and_colours_are() {
        let mut b = board();
        b.resize(200, 30);
        let text = screen(&b).join("\n");
        assert!(text.contains("pane   name   mode state inbox quiet  running"), "{text}");
        // The pane rows of the left panel (between its borders).
        let rows: Vec<&str> = text.lines().filter_map(|l| l.split('│').nth(1)).filter(|l| l.contains(" %")).collect();
        assert_eq!(rows.len(), 3, "{text}");
        assert!(rows.iter().all(|l| !l.contains(" · ") && !l.contains(" - ")), "{rows:#?}");
        // The picked row is reversed, its colours gone: pick one that has none.
        b.chosen = Some("%3".into());
        let f = b.frame();
        for (sgr, what) in [
            ("32m", "idle"),
            ("33m", "busy"),
            ("35m", "ai"),
            ("1;34m", "2"),
            ("1;34m", "[1]"),
            ("1;34m", "work"),
            ("1;36m", "j/k"),
            ("1;36m", "Tab/1230"),
        ] {
            assert!(f.contains(&format!("\x1b[{sgr}{what}\x1b[0m")), "{what} in \\x1b[{sgr}");
        }
        assert_eq!(paint_context("12%"), "\x1b[32m12%\x1b[0m");
        assert_eq!(paint_context("60%"), "\x1b[33m60%\x1b[0m");
        assert_eq!(paint_context("80%"), "\x1b[31m80%\x1b[0m");
        assert_eq!(paint_context("340k"), "340k", "a size, not a share: plain");
        assert_eq!(paint("1", ""), "", "no text, no codes");
        for (w, long) in [
            ("85ms", false),
            ("7.3s", false),
            ("59s", false),
            ("1m", true),
            ("4m05s", true),
            ("2h13m", true),
            ("3d2h", true),
        ] {
            assert_eq!(waited_long(w), long, "{w}");
        }
        // The inbox says which field is which.
        inbox(&mut b);
        let text = screen(&b).join("\n");
        assert!(text.contains("#5 from user · 1s · first"), "{text}");
    }

    /// Named agent panes in `ai` mode with messages waiting, in the popup
    /// of a 96x26 terminal: the least needed columns go, each agent's cost
    /// and context stay whole.
    #[test]
    fn a_popup_drops_columns_before_the_agents_figures() {
        let mut b = board();
        let agent =
            AgentRow { kind: "claude".into(), cost: "$2.10".into(), context: "34%".into(), ..AgentRow::default() };
        let mut rows = b.rows.clone();
        for r in &mut rows {
            r.agent = Some(agent.clone());
        }
        b.set_rows(rows);
        b.resize(84, 21);
        let text = screen(&b).join("\n");
        assert_eq!(text.matches("claude $2.10 34%").count(), 3, "{text}");
        assert!(text.contains("pane   name   state  running"), "quiet, mode and inbox go first: {text}");
        assert!(text.contains("lead") && text.contains("busy"), "name and state stay: {text}");
        // Wide: every column.
        b.resize(200, 30);
        assert!(screen(&b).join("\n").contains("quiet  running"));
        // Panes running short things (`pwsh`) leave the room to the other
        // columns: messages waiting still show in the popup.
        let mut b = board();
        b.resize(84, 21);
        let text = screen(&b).join("\n");
        assert!(text.contains("inbox") && text.contains("running"), "{text}");
        // A `normal` pane's title has no placeholders for its mode and state.
        b.chosen = Some("%3".into());
        let text = screen(&b).join("\n");
        assert!(text.contains("[0] %3 ─") && !text.contains("normal ·"), "{text}");
    }

    /// The board's own format, as the server fills it, reads back: the
    /// agent's figures land in their fields.
    #[test]
    fn its_format_reads_back_with_the_agent() {
        let ctx = crate::format::Context {
            pane_id: 7,
            agent: "codex".into(),
            agent_model: "gpt-5.5".into(),
            agent_cost: "$0.06".into(),
            agent_tokens: "27k".into(),
            agent_context: "5%".into(),
            agent_cpu: "3%".into(),
            agent_mem: "120M".into(),
            agent_tools: "4".into(),
            agent_turns: "2".into(),
            ..Default::default()
        };
        let segs = crate::format::expand(
            PANE_FORMAT,
            &ctx,
            &mut crate::format::ShellCache::default(),
            Default::default(),
            chrono::Local::now(),
        );
        let r = PaneRow::parse(&crate::format::plain(&segs)).unwrap();
        assert_eq!(r.id, "%7");
        let a = r.agent.unwrap();
        let got = [&a.kind, &a.model, &a.cost, &a.tokens, &a.context, &a.cpu, &a.mem, &a.tools, &a.turns];
        assert_eq!(got, ["codex", "gpt-5.5", "$0.06", "27k", "5%", "3%", "120M", "4", "2"]);
        let none = crate::format::Context { pane_id: 8, ..Default::default() };
        let segs = crate::format::expand(
            PANE_FORMAT,
            &none,
            &mut crate::format::ShellCache::default(),
            Default::default(),
            chrono::Local::now(),
        );
        assert_eq!(PaneRow::parse(&crate::format::plain(&segs)).unwrap().agent, None);
    }

    /// While the phones' page is served, the pane panel says how many are
    /// on it; off, nothing.
    #[test]
    fn the_phones_on_the_web_page_are_counted() {
        let mut b = board();
        b.resize(200, 30);
        b.set_web("serving http://192.168.1.5:7681/#k=abc · read-only · since 14:02 · 2 connected\n  192.168.1.20 ...");
        assert_eq!(b.web, Some(2));
        let text = screen(&b).join("\n");
        assert!(text.contains("[1] Panes  3 · 1 busy · 2 queued · web 2 connected"), "{text}");
        for off in ["off: `keepane web` starts it", "", "serving nonsense"] {
            b.set_web(off);
            assert_eq!(b.web, None, "{off}");
        }
        assert!(!screen(&b).join("\n").contains("web "));
    }

    #[test]
    fn narrow_windows_show_one_side() {
        let mut b = board();
        b.resize(60, 20);
        assert!(screen(&b).join("\n").contains("[1] Panes") && !screen(&b).join("\n").contains("[0] "));
        b.key(Key::ch('0'));
        let text = screen(&b).join("\n");
        assert!(text.contains("[0] %1") && !text.contains("[1] Panes"), "{text}");
    }

    #[test]
    fn the_mouse_picks_and_scrolls() {
        let mut b = board();
        inbox(&mut b);
        let layout = b.layout();
        let click = |x: usize, y: usize| MouseRecord { x: x as i16, y: y as i16, buttons: 1, ctrl: 0, flags: 0 };
        // The second message of the inbox: the row below its border, and one more.
        let r = layout.inbox.unwrap();
        b.mouse(&click(r.x + 3, r.y + 2));
        assert_eq!((b.focus, b.inbox_pick), (Panel::Inbox, 1));
        // A pane: its line in the table (the column heading and session
        // headings take lines too: heading, work, %1, %2, ops, %3).
        let p = layout.panes.unwrap();
        b.mouse(&click(p.x + 3, p.y + 1 + 5));
        assert_eq!((b.focus, b.chosen.as_deref()), (Panel::Panes, Some("%3")));
        // The column heading is no pane: the choice stays.
        b.mouse(&click(p.x + 3, p.y + 1));
        assert_eq!((b.focus, b.chosen.as_deref()), (Panel::Panes, Some("%3")));
        // The main panel takes focus; the wheel scrolls it.
        b.set_main((0..100).map(|i| format!("line {i}")).collect());
        let m = layout.main.unwrap();
        b.mouse(&click(m.x + 5, m.y + 5));
        assert_eq!(b.focus, Panel::Main);
        b.mouse(&MouseRecord { x: (m.x + 5) as i16, y: 5, buttons: 120 << 16, ctrl: 0, flags: 4 });
        assert_eq!(b.scroll, 3);
        // C-b / C-f page it as PgUp / PgDn do, C-u / C-d half a page.
        let page = b.page() as usize;
        b.key(Key::ctrl('b'));
        assert_eq!(b.scroll, 3 + page);
        b.key(Key::ctrl('f'));
        assert_eq!(b.scroll, 3);
        b.key(Key::ctrl('u'));
        assert_eq!(b.scroll, 3 + (page / 2).max(1));
        b.key(Key::ctrl('d'));
        assert_eq!(b.scroll, 3);
        b.key(Key::plain(KeyCode::PPage));
        assert_eq!(b.scroll, 3 + page, "the same as PgUp");
        b.key(Key::plain(KeyCode::NPage));
        // A release or a move does nothing.
        b.mouse(&MouseRecord { x: (p.x + 3) as i16, y: 2, buttons: 0, ctrl: 0, flags: 0 });
        assert_eq!(b.focus, Panel::Main);
    }

    #[test]
    fn tasks_read_by_their_columns() {
        // As list-tasks prints them (the heading's AT is also inside STATUS).
        let lines: Vec<String> = [
            "TASK  STATUS   AT             TOOK  STEPS  TITLE",
            "#2    running  $1:@3.%4 (ai)  15ms  1      then report back",
            "#10   failed   $1:@3.%12 (shell)  3s  2      a  title  with gaps",
        ]
        .map(String::from)
        .to_vec();
        let t = TaskRow::parse_all(&lines);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].id.as_str(), t[0].status.as_str(), t[0].at.as_str()), ("2", "running", "%4 (ai)"));
        assert_eq!((t[0].took.as_str(), t[0].steps.as_str(), t[0].title.as_str()), ("15ms", "1", "then report back"));
        // A wider address than the heading's column: still its own cells.
        assert_eq!(t[1].id, "10");
        // No heading: by words.
        let t = TaskRow::parse_all(&["#7 done the title".to_string()]);
        assert_eq!((t[0].id.as_str(), t[0].status.as_str(), t[0].title.as_str()), ("7", "done", "the title"));
        assert!(strip(&t[0].line()).starts_with("#7   done"), "{}", strip(&t[0].line()));
        assert_eq!(wrap("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(wrap("中文字", 4), ["中文", "字"]);
        assert_eq!(wrap("", 5), [""]);
    }

    #[test]
    fn an_envelope_reads_as_its_fields() {
        let env =
            r#"{"keepane":1,"id":3,"task":3,"from":"$1:@3.%4","mode":"normal","to":"$1:@6.%5","via":"ai","hop":0}"#;
        let f: Vec<String> = envelope_fields(env).unwrap().iter().map(|l| strip(l)).collect();
        assert_eq!(
            f,
            ["id    3", "task  3", "from  $1:@3.%4", "mode  normal", "to    $1:@6.%5", "via   ai", "hop   0"]
        );
        // The fields form reads the same.
        let fields = "[keepane id=3 task=3 from=$1:@3.%4 mode=normal to=$1:@6.%5 via=ai hop=0]";
        assert_eq!(envelope_fields(fields).unwrap().iter().map(|l| strip(l)).collect::<Vec<_>>(), f);
        assert_eq!(envelope_fields("text: {not json"), None);
        assert_eq!(envelope_fields("[1,2]"), None);
        assert_eq!(envelope_fields("[keepane nonsense]"), None);
        // In Detail, the envelope comes as its fields and the rest as it is.
        let mut b = board();
        b.tab = Tab::Detail;
        b.detail = Some(Detail::Message("3".into()));
        b.set_main(vec!["#3 queued · task #3 · hop 0".into(), env.into(), "text:".into(), "hi".into()]);
        let text = screen(&b).join("\n");
        assert!(
            text.contains("from  $1:@3.%4") && text.contains("via   ai") && !text.contains("{\"keepane\""),
            "{text}"
        );
    }

    #[test]
    fn text_is_cut_to_width_with_its_styles() {
        assert_eq!(clip("abcdef", 3), ("abc".into(), 3));
        assert_eq!(clip("\x1b[31mred\x1b[0m", 2), ("\x1b[31mre\x1b[0m".into(), 2), "a cut string ends its styles");
        assert_eq!(clip("中文", 3).1, 2, "a wide character that does not fit is left out");
        assert_eq!(clip("\x1b]0;t\x07x", 5), ("x".into(), 1), "an OSC is dropped");
        assert_eq!(width_of("a\tb"), 3, "a control character is a space");
        assert_eq!(width_of(&fit("\x1b[1mab", 6)), 6);
        assert_eq!(window_start(9, 20, 5), 5);
        assert_eq!(window_start(0, 20, 5), 0);
        assert_eq!(window_start(19, 20, 5), 15);
    }

    #[test]
    fn events_read_short() {
        let sent = r#"{"at":"2026-09-26T14:28:02.000+08:00","ev":"sent","msg":{"keepane":1,"id":12,"task":12,"from":"$1:@1.%3","name":"lead","mode":"ai","to":"$1:@2.%7","via":"ai","hop":0},"text":"run the tests\nand report"}"#;
        let line = strip(&event_line(sent));
        assert!(line.ends_with("sent      #12 %lead → $1:@2.%7 (ai): run the tests"), "{line}");
        let status = r#"{"at":"2026-09-26T14:31:12.000+08:00","ev":"pane","pane":"$1:@2.%7","what":"status","name":"tester","mode":"ai","program":"claude","text":"tests 3/10"}"#;
        assert!(strip(&event_line(status)).ends_with("pane      status tests 3/10"), "{}", event_line(status));
        let done =
            r#"{"at":"2026-09-26T14:31:12.000+08:00","ev":"failed","id":14,"ok":false,"output":"x","cut":false}"#;
        assert!(strip(&event_line(done)).ends_with("failed    #14 failed"));
        assert!(event_line(done).contains("\x1b[31m"), "a failure in red");
        assert_eq!(event_line("not json"), "not json");
    }
}
