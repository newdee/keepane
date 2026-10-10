//! Server options and the config file (`%USERPROFILE%\.keepane.conf`), which is
//! a list of keepane commands, one per line, like `.tmux.conf`.

use crate::keys::Key;
use std::path::PathBuf;
use vt100::Color;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub prefix: Key,
    /// Program used for new panes when no command is given ("pwsh", "wsl", a path...).
    pub default_shell: String,
    /// Full command line overriding `default_shell` when non-empty.
    pub default_command: Vec<String>,
    pub mouse: bool,
    pub history_limit: usize,
    pub status: bool,
    pub status_top: bool,
    pub status_fg: Color,
    pub status_bg: Color,
    /// Keep a pane after its program exits, showing why, so a background job
    /// that died leaves its output and can be restarted (tmux `remain-on-exit`).
    pub remain_on_exit: bool,
    /// Lines of each pane's scrollback written into the session file, so
    /// `resume` brings the output back too. 0 saves nothing; `all`
    /// (`usize::MAX`) saves everything the pane still holds.
    pub save_history: usize,
    /// Flag a window in the status line when a background window prints
    /// anything (`#`), rings the bell (`!`), or goes quiet for
    /// `monitor-silence` seconds (`~`). 0 seconds turns silence off.
    pub monitor_activity: bool,
    pub monitor_bell: bool,
    pub monitor_silence: u64,
    /// Say it on the status line instead of ringing the terminal bell.
    pub visual_bell: bool,
    pub visual_activity: bool,
    /// Also raise a desktop notification for an alert, so a job that ends
    /// while keepane is not on screen still reaches you.
    pub notify: bool,
    /// Ask GitHub once a day whether a newer keepane is out; the status
    /// line says so (`#{keepane_update}`).
    pub update_check: bool,
    /// Give each pane's agent its cost in dollars (`#{agent_cost}`), from a
    /// public price list fetched once a day while an agent runs.
    pub agent_cost: bool,
    /// A line of text on every pane's top or bottom border ("off", "top",
    /// "bottom"), from `pane-border-format`.
    pub pane_border_status: String,
    pub pane_border_format: String,
    pub base_index: usize,
    /// First pane number shown to the user (tmux pane-base-index).
    pub pane_base_index: usize,
    pub display_time_ms: u64,
    /// How long after a `bind -r` key another one counts without the prefix
    /// (tmux `repeat-time`); 0 turns repeating off.
    pub repeat_time_ms: u64,
    /// After the prefix, a panel of what the next key does, once the next
    /// key is `prefix_hint_delay_ms` late (0: at once).
    pub prefix_hint: bool,
    pub prefix_hint_delay_ms: u64,
    pub pane_border_active_fg: Color,
    /// `display-panes`: the other panes' numbers, and the active pane's.
    pub display_panes_colour: Color,
    pub display_panes_active_colour: Color,
    /// `clock-mode` (prefix t): the hours as 12 or 24, the big digits'
    /// colour (None: the active pane border's), and the lines under them
    /// (the date, the pane, its last command, its agent, the machine).
    pub clock_mode_style: u8,
    pub clock_mode_colour: Option<Color>,
    pub clock_mode_info: bool,
    pub pane_border_fg: Color,
    /// Status line formats (see `format.rs`).
    pub status_left: String,
    pub status_right: String,
    pub status_left_length: usize,
    pub status_right_length: usize,
    /// Where the window list sits on the status line: "left", "centre",
    /// "right" or "absolute-centre" (tmux `status-justify`).
    pub status_justify: String,
    /// Text between window labels (tmux `window-status-separator`).
    pub window_status_separator: String,
    /// Seconds between refreshes of `#(command)` pieces.
    pub status_interval: u64,
    pub window_status_format: String,
    pub window_status_current_format: String,
    /// Directory searched by `@plugin name` / `load-plugin name`.
    pub plugin_path: String,
    /// `set -g @plugin x` entries not yet loaded.
    pub pending_plugins: Vec<String>,
    /// tmux-style user options (`@name`), readable by plugins via
    /// `show-options -gv @name`.
    pub user: Vec<(String, String)>,
    /// Save the session tree after every structural change and on exit.
    pub autosave: bool,
    /// Restore every saved session when the server starts with no sessions.
    pub restore_on_start: bool,
    /// Directory holding one file per saved session (`sessions-dir`); empty = default.
    pub sessions_dir: String,
    /// Which attached client a session takes its size from (tmux
    /// `window-size`): "latest" (the one used last), "smallest", "largest"
    /// or "manual" (only `resize-window` changes it).
    pub window_size: String,
    /// `TERM` in every pane on Linux and macOS (tmux `default-terminal`):
    /// what the programs there are told they draw on, not what the server
    /// happened to be started from. ConPTY sets up its own on Windows.
    pub default_terminal: String,
    /// Show when each command ran, and how it went, at the right end of
    /// its line (tmux has no such thing): needs a shell that reports its
    /// commands, which keepane's PowerShell hook does.
    pub pane_timestamps: bool,
    /// Keep what panes print on disk, a file per pane position per day,
    /// for choose-history (see histlog.rs).
    pub log_history: bool,
    /// Days the history log keeps; 0 keeps everything.
    pub log_history_days: u32,
    /// Where it goes; empty is `histlog::default_dir()`.
    pub log_history_dir: String,
    /// Seconds a pane or window killed by a command is kept, running, for
    /// `undo-kill`; 0 ends it at once.
    pub undo_kill_time: u64,
    /// A zoomed window stays zoomed when another of its panes is selected
    /// (the zoom moves to it); only `resize-pane -Z` (prefix z) undoes it.
    /// Off: selecting unzooms, as tmux does.
    pub keep_zoom: bool,
    /// A frame flies to the pane the keys now go to: on selecting a pane,
    /// zooming, and switching windows or sessions. Drawn over content that
    /// is already in place, so nothing waits for it.
    pub animation: bool,
    /// How long it takes, in milliseconds (at most 10000); 0 is the same as off.
    pub animation_time: u64,
    /// Pane messages (docs/design/mailbox.md): the most hops a chain of
    /// messages may take, so two agents cannot answer each other for ever.
    pub message_hop_limit: u32,
    /// How a message's header reads where people and programs see it
    /// (`message-envelope`): `fields` (`[keepane id=… from=…]`) or `json`.
    pub message_envelope: crate::server::actor::EnvelopeStyle,
    /// The most messages one inbox holds.
    pub message_inbox_limit: usize,
    /// The most bytes one message (and a shell command's kept output) holds.
    pub message_max_size: usize,
    /// The longest `-w` waits, in seconds.
    pub message_wait_max: u64,
    /// The most panes one pane and the panes it created may create.
    pub agent_pane_limit: u32,
    /// The programs a pane may start in a pane it creates.
    pub agent_commands: String,
    /// What counts as a pane being done, for the phone, `notify`, the
    /// `pane-done` hook and the webhook: any of `command` (a command that ran
    /// `done_after` seconds or more ended), `agent` (an agent's turn ended),
    /// `task` (a message's task ended), `exit` (the pane's program exited).
    pub done_events: String,
    /// The seconds a command must run to count as done.
    pub done_after: u64,
    /// How many of the last lines a pane printed go with the telling.
    pub done_lines: usize,
    /// Which panes: `named` (a name, or work mode `ai` or `shell`) or `all`.
    pub done_panes: String,
    /// An HTTP(S) address told each time, and how its body is written.
    pub done_webhook: String,
    pub done_webhook_format: String,
    /// Keep what happens to messages and panes in a JSON Lines file a day.
    pub event_log: bool,
    /// Days the event log keeps.
    pub event_log_days: u32,
    /// The most bytes one day's event log holds.
    pub event_log_max: u64,
    /// What opens a path picked in `hints` (a keepane command with
    /// `{file}`, `{line}`, `{col}`); empty: VS Code, else `$EDITOR`, else
    /// the desktop.
    pub hint_open: String,
    /// The built-in theme last set (`theme`): its options were set then.
    pub theme: String,
    /// `window-style`: the colours a pane's default ones are drawn in;
    /// `Default` is the terminal's own.
    pub window_fg: Color,
    pub window_bg: Color,
    /// `pane-colours`: the 16 colours drawn for the palette's first 16 (a
    /// program's red, blue...); empty, the terminal's own.
    pub pane_colours: Vec<Color>,
    /// How `choose-tree` (prefix w, s) shows the sessions at first: `chart`
    /// (rows of blocks under a centred `keepane` root), `tree` (blocks under
    /// a `keepane` root, one a line) or `list` (tmux's lines).
    pub choose_tree_style: String,
}

/// What a pane can be done with (`done-events`).
pub const DONE_EVENTS: &[&str] = &["command", "agent", "task", "exit"];

/// The bodies `done-webhook` can be sent: keepane's JSON, plain text (ntfy,
/// Bark), or the shape a chat's incoming webhook takes.
pub const DONE_WEBHOOK_FORMATS: &[&str] = &["json", "text", "feishu", "wecom", "dingtalk", "slack", "discord"];

/// `done-events`: some of `DONE_EVENTS`, by spaces or commas, `all`, or
/// `none`; kept in `DONE_EVENTS`' order.
fn parse_done_events(value: &str) -> Result<String, String> {
    let words: Vec<&str> = value.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        ["all"] => return Ok(DONE_EVENTS.join(" ")),
        ["none"] | [] => return Ok("none".into()),
        _ => {}
    }
    if let Some(bad) = words.iter().find(|w| !DONE_EVENTS.contains(w)) {
        return Err(format!("bad done-events word '{bad}' (any of {}, or all, or none)", DONE_EVENTS.join(" ")));
    }
    Ok(DONE_EVENTS.iter().filter(|e| words.contains(e)).copied().collect::<Vec<_>>().join(" "))
}

/// A value that must be one of a few words.
fn one_of(name: &str, value: &str, words: &[&str]) -> Result<String, String> {
    let v = value.trim();
    if words.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(format!("bad {name} '{value}' (one of {})", words.join(", ")))
    }
}

/// The built-in themes (`theme`), each the `set -g` lines of its file in
/// themes/: the same options in each, so one replaces another whole.
pub const THEMES: &[(&str, &str)] = &[
    ("tokyo-night", include_str!("../themes/tokyo-night.conf")),
    ("tokyo-day", include_str!("../themes/tokyo-day.conf")),
];
pub const THEME_NAMES: &[&str] = &["tokyo-night", "tokyo-day"];

/// The options a theme sets (the same in each).
fn theme_options() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let names: Vec<String> = theme_settings(THEMES[0].1).unwrap_or_default().into_iter().map(|(n, _)| n).collect();
        names.into_iter().map(|n| &*Box::leak(n.into_boxed_str())).collect()
    })
}

/// The `set -g <option> <value>` lines of a theme file, comments and blank
/// lines left out.
pub fn theme_settings(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        match crate::command::tokenize(line)?.as_slice() {
            [set, g, name, value] if set == "set" && g == "-g" => out.push((name.clone(), value.clone())),
            _ => return Err(format!("a theme line is `set -g <option> <value>`: {line}")),
        }
    }
    Ok(out)
}

/// `pane-colours`: 16 colours (names, `colourN` or `#rrggbb`, apart by
/// blanks or commas), or nothing for the terminal's own.
fn parse_colours(value: &str) -> Result<Vec<Color>, String> {
    let words: Vec<&str> = value.split([',', ' ', '\t']).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }
    if words.len() != 16 {
        return Err(format!("pane-colours: 16 colours, or none for the terminal's own; got {}", words.len()));
    }
    words.into_iter().map(parse_color).collect()
}

/// A number option within its bounds; out of them it is an error, never
/// quietly clamped.
fn ranged<T: std::str::FromStr + PartialOrd + std::fmt::Display>(
    name: &str,
    value: &str,
    lo: T,
    hi: T,
    unit: &str,
) -> Result<T, String> {
    match value.trim().parse::<T>() {
        Ok(v) if v >= lo && v <= hi => Ok(v),
        _ => Err(format!("bad {name} '{value}' ({unit}, {lo} to {hi})")),
    }
}

/// A size in bytes, written plainly or with K or M.
fn parse_size(name: &str, value: &str, lo: u64, hi: u64) -> Result<u64, String> {
    let v = value.trim();
    let (digits, mult) = match v.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&v[..v.len() - 1], 1024),
        Some('M') => (&v[..v.len() - 1], 1024 * 1024),
        _ => (v, 1),
    };
    match digits.parse::<u64>().ok().and_then(|n| n.checked_mul(mult)) {
        Some(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err(format!("bad {name} '{value}' (bytes, K or M; {} to {})", size_name(lo), size_name(hi))),
    }
}

/// A size the way `parse_size` reads it back, in the largest unit it fits.
fn size_name(n: u64) -> String {
    if n >= 1024 * 1024 && n.is_multiple_of(1024 * 1024) {
        format!("{}M", n / (1024 * 1024))
    } else if n >= 1024 && n.is_multiple_of(1024) {
        format!("{}K", n / 1024)
    } else {
        n.to_string()
    }
}

/// Options `show-options` can print, in display order.
pub const SHOWABLE: &[&str] = &[
    "prefix",
    "default-shell",
    "default-command",
    "mouse",
    "history-limit",
    "status",
    "status-position",
    "status-left",
    "status-right",
    "status-left-length",
    "status-right-length",
    "status-justify",
    "status-style",
    "window-status-separator",
    "status-interval",
    "window-status-format",
    "window-status-current-format",
    "pane-border-style",
    "pane-active-border-style",
    "display-panes-colour",
    "display-panes-active-colour",
    "clock-mode-style",
    "clock-mode-colour",
    "clock-mode-info",
    "base-index",
    "remain-on-exit",
    "save-history",
    "monitor-activity",
    "monitor-bell",
    "monitor-silence",
    "visual-bell",
    "visual-activity",
    "notify",
    "update-check",
    "agent-cost",
    "pane-border-status",
    "pane-border-format",
    "pane-base-index",
    "display-time",
    "repeat-time",
    "prefix-hint",
    "prefix-hint-delay",
    "plugin-path",
    "autosave",
    "restore-on-start",
    "sessions-dir",
    "window-size",
    "default-terminal",
    "pane-timestamps",
    "log-history",
    "log-history-days",
    "log-history-dir",
    "undo-kill-time",
    "keep-zoom",
    "animation",
    "animation-time",
    "message-hop-limit",
    "message-envelope",
    "message-inbox-limit",
    "message-max-size",
    "message-wait-max",
    "agent-pane-limit",
    "agent-commands",
    "done-events",
    "done-after",
    "done-lines",
    "done-panes",
    "done-webhook",
    "done-webhook-format",
    "event-log",
    "event-log-days",
    "event-log-max",
    "hint-open",
    "theme",
    "window-style",
    "pane-colours",
    "choose-tree-style",
];

impl Default for Options {
    fn default() -> Self {
        Options {
            prefix: Key::ctrl('b'),
            default_shell: crate::platform::shell::default_shell(),
            default_command: Vec::new(),
            mouse: true,
            history_limit: 5000,
            status: true,
            status_top: false,
            // The look is Tokyo Night (themes/tokyo-night.conf); themes/plain.conf
            // is tmux's plain one. A bar a shade darker than Tokyo Night's
            // background (#1a1b26), so it reads as its own strip.
            status_fg: Color::Rgb(0xa9, 0xb1, 0xd6),
            status_bg: Color::Rgb(0x16, 0x16, 0x1e),
            remain_on_exit: false,
            save_history: 500,
            monitor_activity: false,
            monitor_bell: true,
            monitor_silence: 0,
            visual_bell: false,
            visual_activity: false,
            notify: false,
            update_check: true,
            agent_cost: true,
            pane_border_status: "off".into(),
            pane_border_format: " #{?pane_active,#[bold],}#{pane_index}: #{pane_title}#[default] ".into(),
            base_index: 0,
            pane_base_index: 0,
            display_time_ms: 1500,
            repeat_time_ms: 500,
            prefix_hint: true,
            prefix_hint_delay_ms: 500,
            // Quiet borders, the active pane outlined in blue.
            pane_border_active_fg: Color::Rgb(0x7a, 0xa2, 0xf7),
            // tmux's defaults.
            display_panes_colour: Color::Idx(4),
            display_panes_active_colour: Color::Idx(1),
            clock_mode_style: 24,
            clock_mode_colour: None,
            clock_mode_info: true,
            pane_border_fg: Color::Rgb(0x3b, 0x42, 0x61),
            // The session name on a blue block.
            status_left: "#[fg=#1a1b26,bg=#7aa2f7,bold] #S #[default] ".into(),
            // What is useful at a glance and costs nothing to read: a newer
            // keepane when there is one, the branch when in a repository,
            // where the pane is, the machine's load, the battery when there
            // is one, then the time on a blue block.
            status_right: "#{?keepane_update,#[fg=#1a1b26,bg=#e0af68,bold] ⇡ #{keepane_update} #[default] ,}#{?web_url,#[fg=#1a1b26,bg=#9ece6a,bold] web #{web_clients} #[default] ,}#{?#{==:#{pane_work_mode},normal},#[fg=#565f89]normal ,#[fg=#1a1b26,bg=#bb9af7,bold] #{pane_work_mode} #[default] }#{?git_branch,#[fg=#bb9af7]#{git_branch} ,}#[fg=#7dcfff]#{pane_current_path_short} #[fg=#9ece6a]CPU #{cpu_percentage} #[fg=#e0af68]MEM #{ram_percentage} #{?battery_percentage,#[fg=#9ece6a]BAT #{battery_percentage} ,}#[fg=#1a1b26,bg=#7aa2f7,bold] %H:%M ".into(),
            status_left_length: 40,
            status_right_length: 100, // the default right side is a long one; it gives way only to the current window
            status_justify: "left".into(),
            // Muted tabs, the current one on a purple block.
            window_status_separator: "".into(),
            status_interval: 15,
            window_status_format: "#[fg=#565f89] #I:#W#F ".into(),
            window_status_current_format: "#[fg=#1a1b26,bg=#bb9af7,bold] #I:#W#F #[default]".into(),
            plugin_path: "~/.keepane/plugins".into(),
            pending_plugins: Vec::new(),
            user: Vec::new(),
            autosave: true,
            restore_on_start: false,
            sessions_dir: String::new(),
            window_size: "latest".into(),
            default_terminal: "xterm-256color".into(),
            pane_timestamps: false,
            log_history: true,
            log_history_days: 30,
            log_history_dir: String::new(),
            undo_kill_time: 10,
            keep_zoom: true,
            animation: true,
            animation_time: 160,
            message_hop_limit: 8,
            message_envelope: Default::default(),
            message_inbox_limit: 100,
            message_max_size: 64 * 1024,
            message_wait_max: 600,
            agent_pane_limit: 8,
            agent_commands: crate::platform::shell::DEFAULT_AGENT_COMMANDS.into(),
            done_events: "command agent".into(),
            done_after: 30,
            done_lines: 5,
            done_panes: "named".into(),
            done_webhook: String::new(),
            done_webhook_format: "json".into(),
            event_log: true,
            event_log_days: 30,
            event_log_max: 20 * 1024 * 1024,
            hint_open: String::new(),
            theme: "tokyo-night".into(),
            window_fg: Color::Default,
            window_bg: Color::Default,
            pane_colours: Vec::new(),
            choose_tree_style: "chart".into(),
        }
    }
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        other => Err(format!("bad boolean '{other}'")),
    }
}

/// The sixteen colours by name, in index order (tmux's names).
const COLOR_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightblack",
    "brightred",
    "brightgreen",
    "brightyellow",
    "brightblue",
    "brightmagenta",
    "brightcyan",
    "brightwhite",
];

/// A colour as `show-options` prints it, in a form `parse_color` reads back.
pub fn color_name(c: Color) -> String {
    match c {
        Color::Default => "default".into(),
        Color::Idx(i) if (i as usize) < COLOR_NAMES.len() => COLOR_NAMES[i as usize].into(),
        Color::Idx(i) => format!("colour{i}"),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

pub fn parse_color(v: &str) -> Result<Color, String> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("default") {
        return Ok(Color::Default);
    }
    if let Some(i) = COLOR_NAMES.iter().position(|n| n.eq_ignore_ascii_case(v)) {
        return Ok(Color::Idx(i as u8));
    }
    if let Some(n) = v.strip_prefix("colour").or_else(|| v.strip_prefix("color")) {
        return n.parse::<u8>().map(Color::Idx).map_err(|_| format!("bad colour '{v}'"));
    }
    if let Some(hex) = v.strip_prefix('#')
        && hex.len() == 6
    {
        let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| format!("bad colour '{v}'"));
        return Ok(Color::Rgb(p(0)?, p(2)?, p(4)?));
    }
    Err(format!("bad colour '{v}'"))
}

/// Parse "fg=green,bg=black" style values into (fg, bg). As in tmux, the
/// parts are separated by commas or spaces ("bg=#1e1e2e fg=#cdd6f4").
fn parse_style(v: &str) -> Result<(Option<Color>, Option<Color>), String> {
    let mut fg = None;
    let mut bg = None;
    for part in v.split([',', ' ', '\t', '\n']).filter(|p| !p.is_empty()) {
        if let Some(c) = part.strip_prefix("fg=") {
            fg = Some(parse_color(c)?);
        } else if let Some(c) = part.strip_prefix("bg=") {
            bg = Some(parse_color(c)?);
        } else if part == "default" {
            fg = Some(Color::Default);
            bg = Some(Color::Default);
        } else {
            return Err(format!("bad style '{part}'"));
        }
    }
    Ok((fg, bg))
}

/// Every option keepane actually does something with, plus `synchronize-panes`
/// (which the server handles itself). Used to expand an abbreviation.
pub const KNOWN: &[&str] = &[
    "agent-commands",
    "agent-cost",
    "agent-pane-limit",
    "animation",
    "animation-time",
    "autosave",
    "base-index",
    "choose-tree-style",
    "clock-mode-colour",
    "clock-mode-info",
    "clock-mode-style",
    "default-command",
    "default-shell",
    "default-terminal",
    "display-panes-active-colour",
    "display-panes-colour",
    "display-time",
    "done-after",
    "done-events",
    "done-lines",
    "done-panes",
    "done-webhook",
    "done-webhook-format",
    "event-log",
    "event-log-days",
    "event-log-max",
    "hint-open",
    "history-limit",
    "keep-zoom",
    "log-history",
    "log-history-days",
    "log-history-dir",
    "message-envelope",
    "message-hop-limit",
    "message-inbox-limit",
    "message-max-size",
    "message-wait-max",
    "monitor-activity",
    "monitor-bell",
    "monitor-silence",
    "mouse",
    "notify",
    "pane-active-border-style",
    "pane-base-index",
    "pane-border-format",
    "pane-border-status",
    "pane-border-style",
    "pane-colours",
    "pane-timestamps",
    "plugin-path",
    "prefix",
    "prefix-hint",
    "prefix-hint-delay",
    "remain-on-exit",
    "repeat-time",
    "restore-on-start",
    "save-history",
    "sessions-dir",
    "status",
    "status-bg",
    "status-fg",
    "status-interval",
    "status-justify",
    "status-left",
    "status-left-length",
    "status-position",
    "status-right",
    "status-right-length",
    "status-style",
    "synchronize-panes",
    "theme",
    "undo-kill-time",
    "update-check",
    "visual-activity",
    "visual-bell",
    "window-size",
    "window-status-current-format",
    "window-status-format",
    "window-status-separator",
    "window-style",
];

/// Names taken only so that a `.tmux.conf` loads; setting them does nothing.
/// They can be spelled out in full, but they never win an abbreviation from
/// an option that has an effect (`hist` is `history-limit`, not
/// `history-file`).
pub const ACCEPTED: &[&str] = &[
    "aggressive-resize",
    "allow-rename",
    "automatic-rename",
    "bell-action",
    "escape-time",
    "focus-events",
    "history-file",
    "mode-keys",
    "renumber-windows",
    "set-clipboard",
    "set-titles",
    "set-titles-string",
    "terminal-overrides",
    "window-status-current-style",
];

/// Options that are on or off, so `set -g mouse` with no value flips them.
const BOOLEAN: &[&str] = &[
    "agent-cost",
    "animation",
    "clock-mode-info",
    "autosave",
    "event-log",
    "keep-zoom",
    "log-history",
    "notify",
    "monitor-activity",
    "pane-timestamps",
    "monitor-bell",
    "mouse",
    "prefix-hint",
    "remain-on-exit",
    "restore-on-start",
    "status",
    "synchronize-panes",
    "update-check",
    "visual-activity",
    "visual-bell",
];

/// Expand an option name the way command names expand: an exact name wins,
/// otherwise an unambiguous prefix (`sync` is `synchronize-panes`). A name
/// that matches nothing is returned as it was, so the caller reports it as
/// unknown; `@user` options are never touched.
pub fn resolve_name(name: &str) -> Result<String, String> {
    // An empty name (or an empty word in one) is a prefix of everything, so
    // it is not an abbreviation of anything: let the caller call it unknown.
    if name.is_empty() || name.starts_with('@') || KNOWN.contains(&name) || ACCEPTED.contains(&name) {
        return Ok(name.to_string());
    }
    match option_candidates(name).as_slice() {
        [] => Ok(name.to_string()),
        [one] => Ok(one.to_string()),
        many => {
            // Enough to see what went wrong, not a wall of text.
            let shown = many.iter().take(4).copied().collect::<Vec<_>>().join(", ");
            let rest = many.len().saturating_sub(4);
            let tail = if rest > 0 { format!(" and {rest} more") } else { String::new() };
            Err(format!("ambiguous option: {name} (could be {shown}{tail})"))
        }
    }
}

/// The option names `name` could stand for, in order: what `resolve_name`
/// picks from and what a Tab offers. A prefix of the whole name (`sync`,
/// `rem`) first, then a prefix of each dash-separated word, which is how
/// these names are read aloud (`mon-act`, `w-s-f`). Options that do
/// something are matched first, so the compatibility names never shadow
/// them. An empty name is a prefix of every option that does something.
pub fn option_candidates(name: &str) -> Vec<&'static str> {
    let want: Vec<&str> = name.split('-').collect();
    let by_words = !want.iter().any(|w| w.is_empty());
    let by_word = |o: &&'static str| {
        let parts: Vec<&str> = o.split('-').collect();
        parts.len() == want.len() && parts.iter().zip(&want).all(|(p, w)| p.starts_with(w))
    };
    for table in [KNOWN, ACCEPTED] {
        let hits: Vec<&'static str> = table.iter().copied().filter(|o| o.starts_with(name)).collect();
        if !hits.is_empty() {
            return hits;
        }
        if by_words {
            let hits: Vec<&'static str> = table.iter().copied().filter(by_word).collect();
            if !hits.is_empty() {
                return hits;
            }
        }
    }
    Vec::new()
}

/// The values an option takes when they are a fixed few, for a Tab after
/// the option's name; empty when the value is free (a number, a format).
pub fn option_values(name: &str) -> &'static [&'static str] {
    match name {
        n if BOOLEAN.contains(&n) && n != "status" => &["off", "on"],
        "status" => &["bottom", "off", "on", "top"],
        "status-position" => &["bottom", "top"],
        "pane-border-status" => &["bottom", "off", "top"],
        "status-justify" => &["absolute-centre", "centre", "left", "right"],
        "window-size" => &["largest", "latest", "manual", "smallest"],
        "done-panes" => &["all", "named"],
        "done-webhook-format" => DONE_WEBHOOK_FORMATS,
        "theme" => THEME_NAMES,
        "choose-tree-style" => &["chart", "list", "tree"],
        "clock-mode-style" => &["12", "24"],
        "done-events" => &["all", "none", "command agent"],
        _ => &[],
    }
}

impl Options {
    /// Set every option of built-in theme `name`. Checked first and set
    /// after, so a theme that does not read leaves the look as it was.
    fn apply_theme(&mut self, name: &str) -> Result<(), String> {
        let text = THEMES.iter().find(|(n, _)| *n == name).map(|(_, t)| *t).ok_or(format!("no theme {name}"))?;
        let settings = theme_settings(text)?;
        let mut next = self.clone();
        for (option, value) in &settings {
            next.set_one(option, value).map_err(|e| format!("theme {name}: {e}"))?;
        }
        *self = next;
        Ok(())
    }

    /// Set an option. One a theme sets, set some other way, makes the look
    /// no built-in theme's any more: `theme` says `custom` then.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.set_one(name, value)?;
        // By its full name: `set status-sty …` is `status-style`.
        let full = resolve_name(name).unwrap_or_else(|_| name.to_string());
        if full != "theme" && theme_options().contains(&full.as_str()) {
            self.theme = "custom".into();
        }
        Ok(())
    }

    fn set_one(&mut self, name: &str, value: &str) -> Result<(), String> {
        let name = &resolve_name(name)?;
        // No value at all flips an on/off option, which is what makes
        // `set mouse` and `set -w sync` worth typing.
        let flipped;
        let value = if value.is_empty() && BOOLEAN.contains(&name.as_str()) {
            flipped = if self.get(name).as_deref() == Some("on") { "off" } else { "on" };
            flipped
        } else {
            value
        };
        match name.as_str() {
            "prefix" => self.prefix = Key::parse(value).ok_or_else(|| format!("bad key '{value}'"))?,
            "default-shell" => self.default_shell = value.to_string(),
            "default-command" => self.default_command = crate::command::tokenize(value)?,
            "mouse" => self.mouse = parse_bool(value)?,
            "history-limit" => self.history_limit = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "status" => {
                self.status = match value {
                    "top" => {
                        self.status_top = true;
                        true
                    }
                    "bottom" => {
                        self.status_top = false;
                        true
                    }
                    v => parse_bool(v)?,
                }
            }
            "status-position" => {
                self.status_top = match value {
                    "top" => true,
                    "bottom" => false,
                    v => return Err(format!("bad status-position '{v}'")),
                }
            }
            "status-style" => {
                let (fg, bg) = parse_style(value)?;
                if let Some(c) = fg {
                    self.status_fg = c;
                }
                if let Some(c) = bg {
                    self.status_bg = c;
                }
            }
            "status-fg" => self.status_fg = parse_color(value)?,
            "status-bg" => self.status_bg = parse_color(value)?,
            "display-panes-colour" => self.display_panes_colour = parse_color(value)?,
            "clock-mode-style" => {
                self.clock_mode_style = match value.trim() {
                    "12" => 12,
                    "24" => 24,
                    _ => return Err(format!("clock-mode-style is 12 or 24, not '{value}'")),
                }
            }
            "clock-mode-colour" => {
                self.clock_mode_colour = match value.trim() {
                    "default" => None,
                    v => Some(parse_color(v)?),
                }
            }
            "clock-mode-info" => self.clock_mode_info = parse_bool(value)?,
            "display-panes-active-colour" => self.display_panes_active_colour = parse_color(value)?,
            "pane-active-border-style" => {
                if let (Some(c), _) = parse_style(value)? {
                    self.pane_border_active_fg = c;
                }
            }
            "pane-border-style" => {
                if let (Some(c), _) = parse_style(value)? {
                    self.pane_border_fg = c;
                }
            }
            "base-index" => self.base_index = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "remain-on-exit" => self.remain_on_exit = parse_bool(value)?,
            "save-history" => {
                self.save_history = match value.trim() {
                    v if v.eq_ignore_ascii_case("all") => usize::MAX,
                    v => v.parse().map_err(|_| format!("bad save-history '{v}' (a number of lines, or all)"))?,
                }
            }
            "monitor-activity" => self.monitor_activity = parse_bool(value)?,
            "monitor-bell" => self.monitor_bell = parse_bool(value)?,
            "monitor-silence" => self.monitor_silence = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "visual-bell" => self.visual_bell = parse_bool(value)?,
            "visual-activity" => self.visual_activity = parse_bool(value)?,
            "notify" => self.notify = parse_bool(value)?,
            "update-check" => self.update_check = parse_bool(value)?,
            "agent-cost" => self.agent_cost = parse_bool(value)?,
            "pane-border-status" => {
                self.pane_border_status = match value {
                    "off" | "top" | "bottom" => value.to_string(),
                    v => return Err(format!("bad pane-border-status '{v}' (off, top or bottom)")),
                }
            }
            "pane-border-format" => self.pane_border_format = value.to_string(),
            "pane-base-index" => self.pane_base_index = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "display-time" => self.display_time_ms = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "repeat-time" => self.repeat_time_ms = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "status-left" => self.status_left = value.to_string(),
            "status-right" => self.status_right = value.to_string(),
            "status-left-length" => {
                self.status_left_length = value.parse().map_err(|_| format!("bad number '{value}'"))?
            }
            "status-right-length" => {
                self.status_right_length = value.parse().map_err(|_| format!("bad number '{value}'"))?
            }
            "status-justify" => {
                // Both spellings of centre, stored as tmux spells it.
                self.status_justify = match value.trim() {
                    "left" | "right" | "absolute-centre" => value.trim().to_string(),
                    "centre" | "center" => "centre".to_string(),
                    "absolute-center" => "absolute-centre".to_string(),
                    v => return Err(format!("bad status-justify '{v}' (left, centre, right or absolute-centre)")),
                }
            }
            "window-status-separator" => self.window_status_separator = value.to_string(),
            "status-interval" => self.status_interval = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "window-status-format" => self.window_status_format = value.to_string(),
            "window-status-current-format" => self.window_status_current_format = value.to_string(),
            "plugin-path" => self.plugin_path = value.to_string(),
            "autosave" => self.autosave = parse_bool(value)?,
            "restore-on-start" => self.restore_on_start = parse_bool(value)?,
            "sessions-dir" => self.sessions_dir = value.to_string(),
            "pane-timestamps" => self.pane_timestamps = parse_bool(value)?,
            "log-history" => self.log_history = parse_bool(value)?,
            "keep-zoom" => self.keep_zoom = parse_bool(value)?,
            "animation" => self.animation = parse_bool(value)?,
            "prefix-hint" => self.prefix_hint = parse_bool(value)?,
            "prefix-hint-delay" => {
                self.prefix_hint_delay_ms = match value.trim().parse::<u64>() {
                    Ok(ms) if ms <= 10_000 => ms,
                    _ => return Err(format!("bad prefix-hint-delay '{value}' (milliseconds, 0 to 10000)")),
                }
            }
            // An animation that never ends would redraw for ever.
            "animation-time" => {
                self.animation_time = match value.trim().parse::<u64>() {
                    Ok(ms) if ms <= 10_000 => ms,
                    _ => return Err(format!("bad animation-time '{value}' (milliseconds, 0 to 10000)")),
                }
            }
            "log-history-days" => self.log_history_days = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "log-history-dir" => self.log_history_dir = value.to_string(),
            "undo-kill-time" => self.undo_kill_time = value.parse().map_err(|_| format!("bad number '{value}'"))?,
            "message-hop-limit" => self.message_hop_limit = ranged(name, value, 1, 100, "hops")?,
            "message-envelope" => {
                self.message_envelope = crate::server::actor::EnvelopeStyle::parse(value.trim())
                    .ok_or_else(|| format!("bad message-envelope '{value}' (fields or json)"))?;
            }
            "message-inbox-limit" => self.message_inbox_limit = ranged(name, value, 1, 10_000, "messages")?,
            "message-max-size" => {
                self.message_max_size = parse_size(name, value, 1024, 1024 * 1024)? as usize;
            }
            "message-wait-max" => self.message_wait_max = ranged(name, value, 1, 86_400, "seconds")?,
            "agent-pane-limit" => self.agent_pane_limit = ranged(name, value, 0, 256, "panes")?,
            "agent-commands" => self.agent_commands = value.trim().to_string(),
            "done-events" => self.done_events = parse_done_events(value)?,
            "done-after" => self.done_after = ranged(name, value, 0, 86_400, "seconds")?,
            "done-lines" => self.done_lines = ranged(name, value, 0, 50, "lines")?,
            "done-panes" => {
                self.done_panes = one_of(name, value, &["named", "all"])?;
            }
            "done-webhook" => {
                let v = value.trim();
                let ok = v.is_empty()
                    || ((v.starts_with("http://") || v.starts_with("https://"))
                        && !v.contains(|c: char| c.is_whitespace() || c == '"' || c == '\\'));
                if !ok {
                    return Err(format!(
                        "bad done-webhook '{value}' (an http:// or https:// address, or empty for none)"
                    ));
                }
                self.done_webhook = v.to_string();
            }
            "done-webhook-format" => {
                self.done_webhook_format = one_of(name, value, DONE_WEBHOOK_FORMATS)?;
            }
            "event-log" => self.event_log = parse_bool(value)?,
            "event-log-days" => self.event_log_days = ranged(name, value, 1, 3650, "days")?,
            "event-log-max" => self.event_log_max = parse_size(name, value, 1024 * 1024, 1024 * 1024 * 1024)?,
            "hint-open" => self.hint_open = value.trim().to_string(),
            // `custom` is what `theme` says once a theme's options were set
            // some other way; set back, it leaves the look as it is.
            "theme" if value.trim() == "custom" => self.theme = "custom".into(),
            "theme" => {
                let name = one_of(name, value, THEME_NAMES)?;
                self.apply_theme(&name)?;
                self.theme = name;
            }
            "window-style" => {
                let (fg, bg) = parse_style(value)?;
                if let Some(c) = fg {
                    self.window_fg = c;
                }
                if let Some(c) = bg {
                    self.window_bg = c;
                }
            }
            "pane-colours" => self.pane_colours = parse_colours(value)?,
            "choose-tree-style" => self.choose_tree_style = one_of(name, value, &["chart", "tree", "list"])?,
            "default-terminal" => {
                let v = value.trim();
                if v.is_empty() || v.contains(char::is_whitespace) {
                    return Err(format!("bad default-terminal '{value}' (a terminal name, such as xterm-256color)"));
                }
                self.default_terminal = v.to_string();
            }
            "window-size" => {
                self.window_size = match value.trim() {
                    v @ ("latest" | "smallest" | "largest" | "manual") => v.to_string(),
                    v => return Err(format!("bad window-size '{v}' (latest, smallest, largest or manual)")),
                }
            }
            "@plugin" => {
                self.pending_plugins.push(value.to_string());
                self.user.push(("@plugin".into(), value.to_string()));
            }
            n if n.starts_with('@') => {
                self.user.retain(|(k, _)| k != n);
                self.user.push((n.to_string(), value.to_string()));
            }
            // Accepted for .tmux.conf compatibility; no effect on Windows.
            "escape-time"
            | "terminal-overrides"
            | "focus-events"
            | "set-clipboard"
            | "renumber-windows"
            | "allow-rename"
            | "automatic-rename"
            | "window-status-current-style"
            | "mode-keys"
            | "aggressive-resize"
            | "bell-action"
            | "set-titles"
            | "set-titles-string"
            | "history-file" => {}
            other => return Err(format!("unknown option '{other}'")),
        }
        Ok(())
    }

    /// Current value of an option as `show-options` prints it. Takes the
    /// same abbreviations `set` does.
    pub fn get(&self, name: &str) -> Option<String> {
        if name.starts_with('@') {
            return self.user.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        }
        let full = resolve_name(name).ok()?;
        let name = full.as_str();
        let onoff = |b: bool| if b { "on" } else { "off" }.to_string();
        Some(match name {
            "prefix" => self.prefix.to_string(),
            "default-shell" => self.default_shell.clone(),
            "default-command" => self.default_command.join(" "),
            "mouse" => onoff(self.mouse),
            "history-limit" => self.history_limit.to_string(),
            "status" => onoff(self.status),
            "status-position" => if self.status_top { "top" } else { "bottom" }.into(),
            "status-left" => self.status_left.clone(),
            "status-right" => self.status_right.clone(),
            "status-left-length" => self.status_left_length.to_string(),
            "status-right-length" => self.status_right_length.to_string(),
            "status-justify" => self.status_justify.clone(),
            "window-status-separator" => self.window_status_separator.clone(),
            "status-interval" => self.status_interval.to_string(),
            "window-status-format" => self.window_status_format.clone(),
            "window-status-current-format" => self.window_status_current_format.clone(),
            "base-index" => self.base_index.to_string(),
            "remain-on-exit" => onoff(self.remain_on_exit),
            "save-history" => {
                if self.save_history == usize::MAX {
                    "all".to_string()
                } else {
                    self.save_history.to_string()
                }
            }
            "monitor-activity" => onoff(self.monitor_activity),
            "monitor-bell" => onoff(self.monitor_bell),
            "monitor-silence" => self.monitor_silence.to_string(),
            "visual-bell" => onoff(self.visual_bell),
            "visual-activity" => onoff(self.visual_activity),
            "notify" => onoff(self.notify),
            "update-check" => onoff(self.update_check),
            "agent-cost" => onoff(self.agent_cost),
            "pane-border-status" => self.pane_border_status.clone(),
            "pane-border-format" => self.pane_border_format.clone(),
            "pane-base-index" => self.pane_base_index.to_string(),
            "display-time" => self.display_time_ms.to_string(),
            "repeat-time" => self.repeat_time_ms.to_string(),
            "prefix-hint" => onoff(self.prefix_hint),
            "prefix-hint-delay" => self.prefix_hint_delay_ms.to_string(),
            "plugin-path" => self.plugin_path.clone(),
            "autosave" => onoff(self.autosave),
            "restore-on-start" => onoff(self.restore_on_start),
            "sessions-dir" => {
                if self.sessions_dir.is_empty() {
                    crate::resurrect::default_dir().to_string_lossy().into_owned()
                } else {
                    self.sessions_dir.clone()
                }
            }
            "window-size" => self.window_size.clone(),
            "default-terminal" => self.default_terminal.clone(),
            "pane-timestamps" => onoff(self.pane_timestamps),
            "log-history" => onoff(self.log_history),
            "keep-zoom" => onoff(self.keep_zoom),
            "animation" => onoff(self.animation),
            "animation-time" => self.animation_time.to_string(),
            "log-history-days" => self.log_history_days.to_string(),
            "undo-kill-time" => self.undo_kill_time.to_string(),
            "message-hop-limit" => self.message_hop_limit.to_string(),
            "message-envelope" => self.message_envelope.as_str().to_string(),
            "message-inbox-limit" => self.message_inbox_limit.to_string(),
            "message-max-size" => size_name(self.message_max_size as u64),
            "message-wait-max" => self.message_wait_max.to_string(),
            "agent-pane-limit" => self.agent_pane_limit.to_string(),
            "agent-commands" => self.agent_commands.clone(),
            "done-events" => self.done_events.clone(),
            "done-after" => self.done_after.to_string(),
            "done-lines" => self.done_lines.to_string(),
            "done-panes" => self.done_panes.clone(),
            "done-webhook" => self.done_webhook.clone(),
            "done-webhook-format" => self.done_webhook_format.clone(),
            "event-log" => onoff(self.event_log),
            "event-log-days" => self.event_log_days.to_string(),
            "event-log-max" => size_name(self.event_log_max),
            "hint-open" => self.hint_open.clone(),
            "theme" => self.theme.clone(),
            "window-style" => format!("fg={},bg={}", color_name(self.window_fg), color_name(self.window_bg)),
            "pane-colours" => self.pane_colours.iter().map(|c| color_name(*c)).collect::<Vec<_>>().join(" "),
            "choose-tree-style" => self.choose_tree_style.clone(),
            "log-history-dir" => {
                if self.log_history_dir.is_empty() {
                    crate::histlog::default_dir().to_string_lossy().into_owned()
                } else {
                    self.log_history_dir.clone()
                }
            }
            "status-style" => format!("fg={},bg={}", color_name(self.status_fg), color_name(self.status_bg)),
            "status-fg" => color_name(self.status_fg),
            "status-bg" => color_name(self.status_bg),
            "pane-border-style" => format!("fg={}", color_name(self.pane_border_fg)),
            "pane-active-border-style" => format!("fg={}", color_name(self.pane_border_active_fg)),
            "display-panes-colour" => color_name(self.display_panes_colour),
            "clock-mode-style" => self.clock_mode_style.to_string(),
            "clock-mode-colour" => self.clock_mode_colour.map_or("default".into(), color_name),
            "clock-mode-info" => onoff(self.clock_mode_info),
            "display-panes-active-colour" => color_name(self.display_panes_active_colour),
            _ => return None,
        })
    }
}

/// Candidate config file locations, first existing wins.
pub fn config_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = crate::legacy::var("KEEPANE_CONFIG") {
        v.push(PathBuf::from(p));
    }
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".keepane.conf"));
        v.push(home.join(".config").join("keepane").join("keepane.conf"));
    }
    if let Some(cfg) = dirs::config_dir() {
        v.push(cfg.join("keepane").join("keepane.conf"));
    }
    // keepane was wmux up to 0.13.1: a config under the old name still counts.
    // A tmux config does not: `keepane import-config` brings over what
    // keepane can use from one, when asked.
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".wmux.conf"));
        v.push(home.join(".config").join("wmux").join("wmux.conf"));
    }
    v
}

pub fn find_config() -> Option<PathBuf> {
    config_paths().into_iter().find(|p| p.is_file())
}

/// A config written for tmux rather than keepane: lines it cannot use are
/// skipped with a note instead of being reported as errors.
pub fn is_tmux_conf(path: &std::path::Path) -> bool {
    path.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.ends_with("tmux.conf"))
}

/// Resolve the executable to run for a new pane when no command is given.
pub fn resolve_shell(opts: &Options) -> Vec<String> {
    if !opts.default_command.is_empty() {
        return opts.default_command.clone();
    }
    crate::platform::shell::named(&opts.default_shell)
}

// The shell and its hook are the platform's.
pub use crate::platform::shell::{PROMPT_HOOK, which, with_shell_integration};

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything `show-options` lists has a value, and that value is
    /// something `set` takes back unchanged: the output is a valid config.
    #[test]
    fn every_shown_option_reads_back() {
        let mut o = Options::default();
        o.set("status-style", "fg=#a9b1d6,bg=colour234").unwrap();
        o.set("pane-active-border-style", "fg=brightblue").unwrap();
        for name in SHOWABLE {
            let v = o.get(name).unwrap_or_else(|| panic!("{name} has no value"));
            let mut again = o.clone();
            again.set(name, &v).unwrap_or_else(|e| panic!("set {name} {v:?}: {e}"));
            assert_eq!(again.get(name).as_deref(), Some(v.as_str()), "{name}");
        }
        assert_eq!(o.get("status-style").unwrap(), "fg=#a9b1d6,bg=colour234");
        assert_eq!(o.get("status-bg").unwrap(), "colour234");
        assert_eq!(o.get("pane-active-border-style").unwrap(), "fg=brightblue");
        assert_eq!(o.get("pane-border-style").unwrap(), "fg=#3b4261", "Tokyo Night's by default");
        // tmux separates a style's parts with spaces as well as commas.
        o.set("status-style", "bg=#1e1e2e fg=#cdd6f4").unwrap();
        assert_eq!(o.get("status-style").unwrap(), "fg=#cdd6f4,bg=#1e1e2e");
        o.set("status-style", " fg=red ,  bg=blue").unwrap();
        assert_eq!(o.get("status-style").unwrap(), "fg=red,bg=blue");
        assert!(o.set("status-style", "fg=red bold").unwrap_err().contains("bad style 'bold'"));
        for c in [Color::Default, Color::Idx(3), Color::Idx(15), Color::Idx(200), Color::Rgb(0, 0x1a, 0xff)] {
            assert_eq!(parse_color(&color_name(c)).unwrap(), c, "{}", color_name(c));
        }
    }

    /// `done-events` keeps its words in one order, takes `all` and `none`,
    /// and refuses a word it does not know; the webhook takes http(s) only.
    #[test]
    fn done_options_take_what_they_can_use() {
        let mut o = Options::default();
        assert_eq!(o.get("done-events").unwrap(), "command agent");
        o.set("done-events", "exit, command").unwrap();
        assert_eq!(o.get("done-events").unwrap(), "command exit");
        o.set("done-events", "all").unwrap();
        assert_eq!(o.get("done-events").unwrap(), "command agent task exit");
        o.set("done-events", "none").unwrap();
        assert_eq!(o.get("done-events").unwrap(), "none");
        assert!(o.set("done-events", "command beep").unwrap_err().contains("'beep'"));
        o.set("done-webhook", "https://open.feishu.cn/open-apis/bot/v2/hook/abc").unwrap();
        o.set("done-webhook", "").unwrap();
        assert_eq!(o.get("done-webhook").unwrap(), "");
        for bad in ["ftp://x", "https://x y", "https://x\"y", "x"] {
            assert!(o.set("done-webhook", bad).is_err(), "{bad}");
        }
        assert!(o.set("done-after", "86401").is_err());
        assert!(o.set("done-panes", "some").is_err());
    }

    /// What a Tab offers after `set`: the same names `resolve_name` picks
    /// from, and for the few-valued options values the setter takes.
    #[test]
    fn option_candidates_and_values() {
        assert_eq!(option_candidates("sync"), vec!["synchronize-panes"]);
        assert_eq!(option_candidates("mo"), vec!["monitor-activity", "monitor-bell", "monitor-silence", "mouse"]);
        assert_eq!(option_candidates("mon-act"), vec!["monitor-activity"]);
        assert_eq!(option_candidates("w-s-f"), vec!["window-status-format"]);
        // Compatibility names only when nothing that works matches.
        assert_eq!(option_candidates("mode-k"), vec!["mode-keys"]);
        assert!(!option_candidates("hist").contains(&"history-file"));
        assert_eq!(option_candidates(""), KNOWN.to_vec());
        assert!(option_candidates("zzz").is_empty());
        // resolve_name still decides the same way.
        assert_eq!(resolve_name("sync").unwrap(), "synchronize-panes");
        assert!(resolve_name("mo").unwrap_err().contains("could be monitor-activity"));
        // Every offered value is one the option takes (synchronize-panes is
        // the server's own, not a stored option).
        for name in KNOWN.iter().filter(|n| **n != "synchronize-panes") {
            for v in option_values(name) {
                let mut o = Options::default();
                assert!(o.set(name, v).is_ok(), "set {name} {v}");
            }
        }
        assert_eq!(option_values("mouse"), ["off", "on"]);
        assert!(option_values("status-left").is_empty());
    }

    /// The abbreviation table has to agree with what `set` actually accepts,
    /// or a short name would expand to something the setter then rejects.
    #[test]
    fn known_option_names_line_up_with_the_setter() {
        for n in SHOWABLE {
            assert!(KNOWN.contains(n), "{n} is showable but cannot be abbreviated");
        }
        for n in BOOLEAN {
            assert!(KNOWN.contains(n), "{n} flips but is not a known name");
        }
        for n in ACCEPTED {
            assert!(!KNOWN.contains(n), "{n} is in both tables");
            assert_eq!(resolve_name(n).unwrap(), *n, "{n} must still be settable in full");
        }
        // A name that does something wins over a compatibility name.
        assert_eq!(resolve_name("hist").unwrap(), "history-limit");
        assert_eq!(resolve_name("esc").unwrap(), "escape-time", "a compatibility name still expands on its own");
        let mut o = Options::default();
        for n in KNOWN {
            // A full name always resolves to itself, whatever else starts
            // with it (`status` must never become `status-style`).
            assert_eq!(resolve_name(n).unwrap(), *n, "{n} does not resolve to itself");
            // And the setter knows it: it may reject the value, but never
            // the name. (synchronize-panes is the server's, not the table's.)
            if *n == "synchronize-panes" {
                continue;
            }
            for v in ["on", "1", "x"] {
                if let Err(e) = o.set(n, v) {
                    assert!(!e.contains("unknown option"), "{n}: {e}");
                }
            }
            assert!(o.get(n).is_some() || !SHOWABLE.contains(n), "{n} is showable but reads back as nothing");
        }
        for n in BOOLEAN {
            if *n == "synchronize-panes" {
                continue;
            }
            assert!(matches!(o.get(n).as_deref(), Some("on" | "off")), "{n} should read back on or off");
        }
    }

    #[test]
    fn set_options() {
        let mut o = Options::default();
        o.set("prefix", "C-a").unwrap();
        assert_eq!(o.prefix, Key::ctrl('a'));
        o.set("mouse", "off").unwrap();
        assert!(!o.mouse);
        o.set("status", "top").unwrap();
        assert!(o.status && o.status_top);
        o.set("status-style", "fg=white,bg=colour234").unwrap();
        assert_eq!(o.status_fg, Color::Idx(7));
        assert_eq!(o.status_bg, Color::Idx(234));
        o.set("status-bg", "#1a2b3c").unwrap();
        assert_eq!(o.status_bg, Color::Rgb(0x1a, 0x2b, 0x3c));
        o.set("default-command", "wsl.exe -d Ubuntu").unwrap();
        assert_eq!(o.default_command, vec!["wsl.exe", "-d", "Ubuntu"]);
        o.set("status-right", "#(uptime) %H:%M").unwrap();
        assert_eq!(o.status_right, "#(uptime) %H:%M");
        o.set("@plugin", "demo").unwrap();
        o.set("@plugin", "other").unwrap();
        o.set("@theme", "dark").unwrap();
        o.set("@theme", "light").unwrap();
        assert_eq!(o.pending_plugins, vec!["demo", "other"]);
        assert_eq!(o.user.iter().filter(|(k, _)| k == "@theme").count(), 1);
        assert!(o.user.iter().any(|(k, v)| k == "@theme" && v == "light"));
        assert_eq!(o.get("@theme").as_deref(), Some("light"));
        assert_eq!(o.get("@missing"), None);
        assert_eq!(o.get("prefix").as_deref(), Some("C-a"));
        assert_eq!(o.get("mouse").as_deref(), Some("off"));
        for name in SHOWABLE {
            assert!(o.get(name).is_some(), "{name} is listed but not showable");
        }
        // Option names take an unambiguous prefix, like command names do.
        assert_eq!(resolve_name("sync").unwrap(), "synchronize-panes");
        assert_eq!(resolve_name("rem").unwrap(), "remain-on-exit");
        assert_eq!(resolve_name("mouse").unwrap(), "mouse", "an exact name wins over any prefix");
        assert_eq!(resolve_name("status").unwrap(), "status");
        assert_eq!(resolve_name("@theme").unwrap(), "@theme", "user options are never expanded");
        assert_eq!(resolve_name("nonsense").unwrap(), "nonsense", "unknown names pass through to the caller");
        let amb = resolve_name("mon").unwrap_err();
        assert!(amb.contains("ambiguous") && amb.contains("monitor-silence"), "{amb}");
        // A name that is a prefix of everything is not an abbreviation, and
        // a long list of candidates is cut short.
        assert_eq!(resolve_name("").unwrap(), "");
        assert_eq!(resolve_name("-").unwrap(), "-");
        assert_eq!(resolve_name("s-").unwrap(), "s-");
        let many = resolve_name("s").unwrap_err();
        assert!(many.contains("and 10 more"), "{many}");
        assert!(many.matches(", ").count() <= 4, "{many}");
        assert!(resolve_name("vis").is_err(), "visual-bell and visual-activity are both there");
        // Each dash-separated word may be abbreviated too.
        assert_eq!(resolve_name("mon-act").unwrap(), "monitor-activity");
        assert_eq!(resolve_name("w-s-f").unwrap(), "window-status-format");
        assert_eq!(resolve_name("s-r").unwrap(), "status-right", "status-right-length has one word more");
        assert_eq!(resolve_name("m-b").unwrap(), "monitor-bell");
        let amb2 = resolve_name("s-p").unwrap_err();
        assert!(amb2.contains("status-position") && amb2.contains("synchronize-panes"), "{amb2}");
        o.set("mon-sil", "30").unwrap();
        assert_eq!(o.monitor_silence, 30);
        assert_eq!(o.get("mon-sil").as_deref(), Some("30"), "show takes the same abbreviation");

        // No value flips an on/off option, and only an on/off option.
        let before = o.mouse;
        o.set("mouse", "").unwrap();
        assert_eq!(o.mouse, !before);
        o.set("mou", "").unwrap();
        assert_eq!(o.mouse, before, "twice is back where it started");
        o.set("monitor-activity", "").unwrap();
        assert!(o.monitor_activity, "off by default, so one flip turns it on");
        assert!(o.set("history-limit", "").is_err(), "a number still needs a value");
        assert!(o.set("nonsense", "1").is_err());
        assert!(o.set("mouse", "maybe").is_err());
        // The prefix's panel: on, half a second; its delay 0 to ten seconds;
        // the new names leave `prefix` itself as it was.
        let d = Options::default();
        assert!(d.prefix_hint);
        assert_eq!(d.get("prefix-hint-delay").as_deref(), Some("500"));
        assert_eq!(resolve_name("prefix").unwrap(), "prefix");
        assert_eq!(resolve_name("prefix-hint").unwrap(), "prefix-hint");
        assert_eq!(resolve_name("pre-h-d").unwrap(), "prefix-hint-delay");
        assert!(o.set("prefix-hint-delay", "0").is_ok() && o.set("prefix-hint-delay", "10000").is_ok());
        for bad in ["10001", "-1", "soon"] {
            assert!(o.set("prefix-hint-delay", bad).unwrap_err().contains("0 to 10000"), "{bad}");
        }
        o.set("prefix-hint", "").unwrap();
        assert_eq!(o.get("prefix-hint").as_deref(), Some("off"), "on/off: no value flips it");
        // An animation has an end: at most ten seconds.
        assert!(o.set("animation-time", "10000").is_ok() && o.set("animation-time", "0").is_ok());
        for bad in ["10001", "18446744073709551615", "-1", "fast"] {
            let e = o.set("animation-time", bad).unwrap_err();
            assert!(e.contains("0 to 10000"), "{bad}: {e}");
        }
        assert_eq!(o.get("animation-time").as_deref(), Some("0"), "a refused value leaves the old one");
        assert!(o.set("history-limit", "x").is_err());
        assert_eq!(o.prefix.to_string(), "C-a");
    }

    /// tmux's `clock-mode-style` and `clock-mode-colour`, and keepane's
    /// `clock-mode-info`: set, shown back, refused when wrong.
    #[test]
    fn the_clock_options_read_back() {
        let mut o = Options::default();
        assert_eq!(
            (
                o.get("clock-mode-style").as_deref(),
                o.get("clock-mode-colour").as_deref(),
                o.get("clock-mode-info").as_deref()
            ),
            (Some("24"), Some("default"), Some("on"))
        );
        o.set("clock-mode-style", "12").unwrap();
        o.set("clock-mode-colour", "red").unwrap();
        o.set("clock-mode-info", "off").unwrap();
        assert_eq!((o.clock_mode_style, o.clock_mode_colour, o.clock_mode_info), (12, Some(Color::Idx(1)), false));
        assert_eq!(o.get("clock-mode-colour").as_deref(), Some("red"));
        o.set("clock-mode-colour", "default").unwrap();
        assert_eq!(o.clock_mode_colour, None, "back to the active border's");
        assert!(o.set("clock-mode-style", "13").unwrap_err().contains("12 or 24"));
        assert!(o.set("clock-mode-colour", "nocolour").is_err());
        assert_eq!(option_values("clock-mode-style"), ["12", "24"]);
    }

    #[test]
    fn status_justify_takes_both_spellings_and_the_separator_anything() {
        let mut o = Options::default();
        assert_eq!(o.status_justify, "left");
        assert_eq!(o.window_status_separator, "", "the default tabs carry their own padding");
        for (given, stored) in [
            ("centre", "centre"),
            ("center", "centre"),
            ("absolute-centre", "absolute-centre"),
            ("absolute-center", "absolute-centre"),
            (" right ", "right"),
            ("left", "left"),
        ] {
            o.set("status-justify", given).unwrap();
            assert_eq!(o.get("status-justify").as_deref(), Some(stored), "{given}");
        }
        for bad in ["", "middle", "CENTRE", "left right"] {
            let e = o.set("status-justify", bad).unwrap_err();
            assert!(e.contains("left, centre, right or absolute-centre"), "{bad}: {e}");
        }
        for sep in ["", " | ", "｜", "#[bold]x", "🙂"] {
            o.set("window-status-separator", sep).unwrap();
            assert_eq!(o.get("window-status-separator").as_deref(), Some(sep));
        }
        assert_eq!(o.set("s-j", "right").map(|_| o.status_justify.clone()).unwrap(), "right", "abbreviates");
    }

    #[test]
    fn default_terminal_is_a_name() {
        let mut o = Options::default();
        assert_eq!(o.get("default-terminal").as_deref(), Some("xterm-256color"));
        o.set("default-terminal", "screen-256color").unwrap();
        assert_eq!(o.default_terminal, "screen-256color");
        for bad in ["", "  ", "xterm 256"] {
            assert!(o.set("default-terminal", bad).is_err(), "{bad:?}");
        }
        assert_eq!(o.default_terminal, "screen-256color", "a refused value changes nothing");
        assert!(SHOWABLE.contains(&"default-terminal") && KNOWN.contains(&"default-terminal"));
        assert!(!ACCEPTED.contains(&"default-terminal"), "it has an effect now");
    }

    #[test]
    fn window_size_takes_the_four_tmux_words() {
        let mut o = Options::default();
        assert_eq!(o.get("window-size").as_deref(), Some("latest"));
        for v in ["smallest", "largest", "manual", " latest "] {
            o.set("window-size", v).unwrap();
            assert_eq!(o.get("window-size").as_deref(), Some(v.trim()), "{v}");
        }
        for bad in ["", "Smallest", "biggest", "0"] {
            let e = o.set("window-size", bad).unwrap_err();
            assert!(e.contains("latest, smallest, largest or manual"), "{bad}: {e}");
        }
        assert!(o.set("window-si", "largest").is_ok(), "abbreviates");
        assert!(SHOWABLE.contains(&"window-size") && KNOWN.contains(&"window-size"));
    }

    #[test]
    fn save_history_all_means_everything() {
        let mut o = Options::default();
        o.set("save-history", "all").unwrap();
        assert_eq!(o.save_history, usize::MAX);
        assert_eq!(o.get("save-history").as_deref(), Some("all"), "shown as the word, not a number");
        o.set("save-hist", "20").unwrap();
        assert_eq!(o.get("save-history").as_deref(), Some("20"));
        o.set("save-history", " ALL ").unwrap();
        assert_eq!(o.save_history, usize::MAX, "case and blanks do not matter, as for on/off");
        let err = o.set("save-history", "some").unwrap_err();
        assert!(err.contains("or all"), "{err}");
        assert!(o.set("save-history", "").is_err(), "not an on/off option");
    }

    /// The light theme reads clearly: each colour text is drawn in (the
    /// panes' text, the 15 colours after black, which a light theme keeps
    /// light for what is drawn on colour) at 4.5:1 or more on the panes and
    /// on the status bar (WCAG AA), the pane borders at 3:1 on the panes.
    #[test]
    fn the_light_theme_reads_clearly() {
        fn lum(c: Color) -> f64 {
            let Color::Rgb(r, g, b) = c else { panic!("an RGB colour: {c:?}") };
            let ch = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
        }
        let ratio = |a: Color, b: Color| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let mut o = Options::default();
        o.set("theme", "tokyo-day").unwrap();
        let (pane, bar) = (o.window_bg, o.status_bg);
        let text: Vec<(String, Color)> = std::iter::once(("text".to_string(), o.window_fg))
            .chain(o.pane_colours.iter().enumerate().skip(1).map(|(i, c)| (format!("colour {i}"), *c)))
            .collect();
        for (name, c) in text {
            assert!(ratio(c, pane) >= 4.5, "{name} on the panes: {:.2}:1", ratio(c, pane));
            assert!(ratio(c, bar) >= 4.5, "{name} on the bar: {:.2}:1", ratio(c, bar));
        }
        assert!(ratio(o.pane_border_fg, pane) >= 3.0, "the border: {:.2}:1", ratio(o.pane_border_fg, pane));
        assert!(ratio(o.pane_border_active_fg, pane) >= 3.0, "the active border");
        // The status line's own runs (`#[fg=…,bg=…]`), each on its block or on the bar.
        let base = crate::server::render::Style::colors(o.status_fg, bar);
        let formats = [&o.status_left, &o.window_status_format, &o.window_status_current_format, &o.status_right];
        let mut runs = 0;
        for f in formats {
            for spec in f.split("#[").skip(1).filter_map(|s| s.split_once(']')).map(|(spec, _)| spec) {
                if !spec.contains("fg=") {
                    continue;
                }
                let s = crate::format::style_from_spec(spec, base);
                assert!(ratio(s.fg, s.bg) >= 4.5, "#[{spec}] (no bg: the bar's): {:.2}:1", ratio(s.fg, s.bg));
                runs += 1;
            }
        }
        assert_eq!(runs, 13, "every coloured run of the status line checked");
    }

    /// The built-in themes set the same options, so one replaces another
    /// whole: tokyo-day and back is the default look again.
    #[test]
    fn a_theme_replaces_the_other_whole() {
        let names = |t: &str| theme_settings(t).unwrap().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        let night = names(THEMES[0].1);
        for (name, text) in THEMES {
            assert_eq!(names(text), night, "{name} sets other options than tokyo-night");
        }
        assert_eq!(THEMES.iter().map(|(n, _)| *n).collect::<Vec<_>>(), THEME_NAMES);
        let default = Options::default();
        let mut o = Options::default();
        o.set("theme", "tokyo-day").unwrap();
        assert_eq!(o.get("theme").as_deref(), Some("tokyo-day"));
        assert_eq!((o.window_fg, o.window_bg), (Color::Rgb(0x33, 0x58, 0xb0), Color::Rgb(0xe1, 0xe2, 0xe7)));
        assert_eq!(o.pane_colours.len(), 16);
        assert_ne!(o.get("status-right"), default.get("status-right"));
        o.set("theme", "tokyo-night").unwrap();
        for n in night.iter().map(String::as_str).chain(["theme"]) {
            assert_eq!(o.get(n), default.get(n), "{n} after tokyo-day and back");
        }
        // No such theme: an error, and nothing changed.
        let before = o.get("status-style");
        assert!(o.set("theme", "solarized").unwrap_err().contains("tokyo-day"));
        assert_eq!((o.get("status-style"), o.get("theme").as_deref()), (before, Some("tokyo-night")));
        // A theme's option set some other way (a file, by hand, abbreviated):
        // the look is no built-in theme's, and says so.
        o.set("status-sty", "bg=red").unwrap();
        assert_eq!(o.get("theme").as_deref(), Some("custom"));
        o.set("theme", "tokyo-day").unwrap();
        assert_eq!(o.get("theme").as_deref(), Some("tokyo-day"));
        // An option no theme sets leaves it.
        o.set("mouse", "off").unwrap();
        assert_eq!(o.get("theme").as_deref(), Some("tokyo-day"));
        for (name, value) in theme_settings(include_str!("../themes/nord.conf")).unwrap() {
            o.set(&name, &value).unwrap();
        }
        assert_eq!(o.get("theme").as_deref(), Some("custom"));
    }

    /// Every theme file in themes/ reads and sets, and after tokyo-day each
    /// gives the panes back to the terminal.
    #[test]
    fn every_theme_file_sets_and_gives_the_panes_back() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut seen = 0;
        for f in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()) {
            let text = std::fs::read_to_string(&f).unwrap();
            let mut o = Options::default();
            o.set("theme", "tokyo-day").unwrap();
            for (name, value) in theme_settings(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display())) {
                o.set(&name, &value).unwrap_or_else(|e| panic!("{}: {name}: {e}", f.display()));
            }
            if !f.ends_with("tokyo-day.conf") {
                assert!(
                    o.window_bg == Color::Default && o.pane_colours.is_empty(),
                    "{} leaves the panes drawn as tokyo-day did",
                    f.display()
                );
            }
            seen += 1;
        }
        assert_eq!(seen, 7);
    }

    #[test]
    fn pane_colours_are_sixteen_or_none() {
        let mut o = Options::default();
        assert_eq!(o.get("pane-colours").as_deref(), Some(""));
        let fifteen = vec!["red"; 15].join(" ");
        assert!(o.set("pane-colours", &fifteen).unwrap_err().contains("16"));
        let sixteen = format!("{fifteen},#112233");
        o.set("pane-colours", &sixteen).unwrap();
        assert_eq!(o.pane_colours[0], Color::Idx(1));
        assert_eq!(o.pane_colours[15], Color::Rgb(0x11, 0x22, 0x33));
        assert!(o.get("pane-colours").unwrap().ends_with("#112233"));
        o.set("pane-colours", "").unwrap();
        assert!(o.pane_colours.is_empty());
        o.set("window-style", "bg=#ffffff").unwrap();
        assert_eq!(o.get("window-style").as_deref(), Some("fg=default,bg=#ffffff"));
        o.set("window-style", "default").unwrap();
        assert_eq!((o.window_fg, o.window_bg), (Color::Default, Color::Default));
    }

    /// themes/tokyo-night.conf is the built-in look written out: every
    /// option it sets comes out the same as the default.
    #[test]
    fn the_tokyo_night_theme_is_the_default_look() {
        let theme = include_str!("../themes/tokyo-night.conf");
        let mut o = Options::default();
        let mut set = Vec::new();
        for line in theme.lines() {
            if let Ok(Some(crate::command::Cmd::SetOption { name, value, append: false, .. })) =
                crate::command::parse_line(line)
            {
                o.set(&name, &value).unwrap_or_else(|e| panic!("{line}: {e}"));
                set.push(name);
            }
        }
        assert!(set.len() >= 9, "the theme sets the look: {set:?}");
        let default = Options::default();
        for name in &set {
            assert_eq!(o.get(name), default.get(name), "{name} differs from the default");
        }
    }

    /// keepane's own config first, then one under the old name (wmux, up to
    /// 0.13.1); never tmux's (`import-config` brings that over when asked).
    #[test]
    fn a_config_under_the_old_name_still_counts() {
        let paths: Vec<String> = config_paths()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".conf"))
            .collect();
        let at = |name: &str| paths.iter().position(|p| p == name).unwrap_or_else(|| panic!("{name} in {paths:?}"));
        assert!(at(".keepane.conf") < at(".wmux.conf"), "{paths:?}");
        assert!(paths.iter().any(|p| p == "wmux.conf"), "~/.config/wmux/wmux.conf too: {paths:?}");
        assert!(!paths.iter().any(|p| p.ends_with("tmux.conf")), "tmux's is not read: {paths:?}");
    }
}
