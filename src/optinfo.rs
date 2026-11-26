//! What each option is, for the settings board (docs/design/settings.md):
//! the group it is shown in, the kind of value it takes (so a value that
//! can be picked is picked, not typed), and one line on what it does, the
//! same words as its row in the man page's "Every option" (a test reads
//! them side by side). Every option `show-options -g` lists is here once.

use crate::config::option_values;

/// The settings board's boxes, in the order they stand (fixed: they are
/// never moved, hidden or taken away).
pub const GROUPS: [&str; 9] = [
    "Look",
    "Status line",
    "Keys",
    "Windows and panes",
    "Sessions and history",
    "Alerts",
    "Agents and messages",
    "Machines, web, Feishu",
    "System",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `on` or `off`.
    Bool,
    /// One of `option_values(name)`; another value may still be typed
    /// (a combination, an address).
    Choice,
    /// A whole number, stepped by this much.
    Number(u64),
    /// Bytes, written with a suffix (`64K`, `2G`).
    Size,
    /// One colour: a name, `colourN`, `#rrggbb`, `default`.
    Colour,
    /// `fg=…,bg=…`.
    Style,
    /// One line of text: a path, a command, a name.
    Text,
    /// A format (`#{…}`, `#[…]`).
    Format,
    /// Several values on one line (colours, zones).
    List,
    /// Not shown: `(set)` or nothing.
    Secret,
    /// A key (`C-b`).
    Key,
}

pub struct OptInfo {
    pub name: &'static str,
    /// Its box: an index into `GROUPS`.
    pub group: usize,
    pub kind: Kind,
    pub doc: &'static str,
}

const fn o(name: &'static str, group: usize, kind: Kind, doc: &'static str) -> OptInfo {
    OptInfo { name, group, kind, doc }
}

use Kind::*;

/// Every option, by box, in the order each box lists them.
pub const OPTIONS: &[OptInfo] = &[
    // 1 Look
    o("theme", 0, Choice, "The built-in theme last set: `paper`, `paper-dark`, `tokyo-night`, `tokyo-day`."),
    o("window-style", 0, Style, "The panes' own colours."),
    o("pane-colours", 0, List, "The 16 colours drawn for a program's first 16."),
    o("pane-border-style", 0, Style, "The lines between panes."),
    o("pane-active-border-style", 0, Style, "The active pane's lines."),
    o("pane-border-lines", 0, Choice, "`heavy`, `single`, `double` or `simple` (ASCII)."),
    o("pane-border-status", 0, Choice, "A line of text on each pane's `top` or `bottom` border."),
    o("pane-border-format", 0, Format, "That line (a format)."),
    o("pane-frames", 0, Bool, "Each pane in a rounded frame of its own."),
    o("display-panes-colour", 0, Colour, "`display-panes`: the other panes' numbers."),
    o("display-panes-active-colour", 0, Colour, "`display-panes`: the active pane's number."),
    o("clock-mode-style", 0, Choice, "`clock-mode`: hours as `12` or `24`."),
    o("clock-mode-colour", 0, Colour, "The big digits' colour (`default`: the active border's)."),
    o("clock-mode-info", 0, Bool, "The lines under the clock (date, waits, pane, machine)."),
    o("clock-mode-zones", 0, List, "Other places' time under the date (IANA names, 8 at most)."),
    o("clock-mode-animation", 0, Choice, "What moves under the clock (a name, `random`, `cycle`, `off`)."),
    o("clock-mode-look", 0, Choice, "The characters it is drawn in: `plain`, `dots`, `blocks`, `binary`."),
    o("choose-tree-style", 0, Choice, "How `choose-tree` shows at first: `chart`, `tree` or `list`."),
    o("dashboard-icons", 0, Choice, "The dashboard's `[0] Files` marks: `emoji` or `nerd`."),
    o("animation", 0, Bool, "A frame flies to the pane the keys now go to."),
    o("animation-time", 0, Number(20), "Its milliseconds (0: off)."),
    // 2 Status line
    o("status", 1, Choice, "The status line."),
    o("status-position", 1, Choice, "`top` or `bottom`."),
    o("status-justify", 1, Choice, "Where the window list sits: `left`, `centre`, `right`, `absolute-centre`."),
    o("status-style", 1, Style, "The status line's colours."),
    o("status-left", 1, Format, "The status line's left part (a format)."),
    o("status-right", 1, Format, "Its right part (a format)."),
    o("status-left-length", 1, Number(5), "The most columns of the left part."),
    o("status-right-length", 1, Number(5), "The most columns of the right part."),
    o("status-interval", 1, Number(5), "Seconds between runs of `#(command)` pieces."),
    o("window-status-format", 1, Format, "A window's label (a format)."),
    o("window-status-current-format", 1, Format, "The current window's label."),
    o("window-status-separator", 1, Text, "Text between window labels."),
    // 3 Keys
    o("prefix", 2, Key, "The prefix key."),
    o("repeat-time", 2, Number(50), "Milliseconds a repeatable key may follow without the prefix."),
    o("prefix-hint", 2, Bool, "After the prefix, a panel of what the next key does."),
    o("prefix-hint-delay", 2, Number(100), "Milliseconds before that panel shows (0: at once)."),
    // 4 Windows and panes
    o("mouse", 3, Bool, "Clicks select panes and windows, drag resizes, the wheel scrolls."),
    o("history-limit", 3, Number(1000), "Lines of scrollback a pane keeps."),
    o("base-index", 3, Number(1), "The first window's number."),
    o("pane-base-index", 3, Number(1), "The first pane's number."),
    o("remain-on-exit", 3, Bool, "A pane stays, saying why, when its program exits."),
    o("keep-zoom", 3, Bool, "A zoomed window stays zoomed when another of its panes is selected."),
    o("undo-kill-time", 3, Number(5), "Seconds a killed pane or window waits for `undo-kill`."),
    o("window-size", 3, Choice, "Whose size a session takes: `latest`, `smallest`, `largest`, `manual`."),
    o("pane-timestamps", 3, Bool, "When each command ran, and how it went, at its line's end."),
    o("default-shell", 3, Text, "What a new pane runs when no program is given."),
    o("default-command", 3, Text, "A whole command line that replaces `default-shell` when set."),
    o("default-terminal", 3, Text, "`TERM` in panes on Linux and macOS."),
    // 5 Sessions and history
    o("autosave", 4, Bool, "Save the sessions after every change of shape and on exit."),
    o("restore-on-start", 4, Bool, "Restore every saved session when the server starts with none."),
    o("sessions-dir", 4, Text, "Where sessions are saved."),
    o("save-history", 4, Text, "Scrollback lines saved with a session (`all`, or 0 for none)."),
    o("log-history", 4, Bool, "Keep what panes print on disk (`choose-history`)."),
    o("log-history-days", 4, Number(1), "Days kept; 0 all."),
    o("log-history-dir", 4, Text, "Where."),
    // 6 Alerts
    o("monitor-activity", 5, Bool, "Mark a background window that prints (`#`)."),
    o("monitor-bell", 5, Bool, "Mark a window that rings the bell (`!`)."),
    o("monitor-silence", 5, Number(5), "Mark a window quiet this many seconds (`~`); 0 never."),
    o("visual-bell", 5, Bool, "Say a bell on the status line instead of ringing."),
    o("visual-activity", 5, Bool, "Say activity on the status line too."),
    o("notify", 5, Bool, "A desktop notification for each alert and done pane."),
    o("display-time", 5, Number(250), "Milliseconds a message stays on the status line."),
    o("done-events", 5, Choice, "What counts as done: `command`, `agent`, `wait`, `task`, `exit`, `bell`."),
    o("done-after", 5, Number(5), "Seconds a command must run to count."),
    o("done-lines", 5, Number(1), "Last lines that go with a telling."),
    o("done-panes", 5, Choice, "Which panes: `named` (a name, or `ai`/`shell` mode) or `all`."),
    o("done-webhook", 5, Text, "An HTTP(S) address told each time."),
    o("done-webhook-format", 5, Choice, "Its body: `json`, `text`, `feishu`, `wecom`, `dingtalk`, `slack`, `discord`."),
    o("done-webhook-secret", 5, Secret, "The secret a Feishu or DingTalk bot checks signatures with."),
    // 7 Agents and messages
    o(
        "agent-pane-limit",
        6,
        Number(1),
        "The most panes a pane (and those it made) may make, and a paired machine may start here (`link start`), each counted alone; 0 to 256.",
    ),
    o("agent-commands", 6, Text, "What `create-pane` may start."),
    o("agent-cost", 6, Bool, "Price agents' tokens by LiteLLM's list (fetched once a day)."),
    o("message-hop-limit", 6, Number(1), "The most hops a chain of messages may take."),
    o("message-envelope", 6, Choice, "A message's header: `fields` or `json`."),
    o("message-inbox-limit", 6, Number(10), "The most messages one inbox holds."),
    o("message-max-size", 6, Size, "The most bytes one message holds."),
    o("message-wait-max", 6, Number(30), "The longest `-w`, in seconds."),
    o("chat-days", 6, Number(1), "Days a group chat line is kept (0: all)."),
    o("chat-file-max-size", 6, Size, "The most bytes one file sent with a chat line holds (`chat-say --file`)."),
    // 8 Machines, web, Feishu
    o("link-clone-dir", 7, Text, "Where a repository another machine clones here goes."),
    o("link-push-limit", 7, Size, "The largest bundle of commits sent or taken."),
    o("link-agents-limit", 7, Size, "The largest `send-agents` pack."),
    o("web-upload-dir", 7, Text, "Where a file sent from the phone goes (under the pane's directory)."),
    o("web-upload-max", 7, Size, "The largest file the phone page takes."),
    o("feishu-app-id", 7, Text, "A Feishu (Lark) app of one's own: its id."),
    o("feishu-app-secret", 7, Secret, "Its secret (`show-options` prints `(set)`)."),
    o("feishu-domain", 7, Choice, "`feishu`, `lark`, or an address."),
    o("feishu-notify", 7, Bool, "Tellings go to the people let in."),
    // 9 System
    o("update-check", 8, Bool, "Ask GitHub once a day whether a newer keepane is out."),
    o("plugin-path", 8, Text, "Where `@plugin` and `load-plugin` look."),
    o("event-log", 8, Bool, "What happens to messages and panes, a JSON Lines file a day."),
    o("event-log-days", 8, Number(1), "Days kept."),
    o("event-log-max", 8, Size, "The most bytes one day's file holds."),
    o("git-command", 8, Text, "The git to run (empty: the one found)."),
    o("hint-open", 8, Text, "What opens a path picked in `hints` (empty: VS Code, `$EDITOR`, the desktop)."),
    o(
        "settings-file",
        8,
        Text,
        "Where the settings board writes, run after the config file (empty: `~/.keepane/settings.conf`; none when `KEEPANE_CONFIG` or `-f` names the config).",
    ),
];

/// What `name` is, when it is a listed option.
pub fn info(name: &str) -> Option<&'static OptInfo> {
    OPTIONS.iter().find(|o| o.name == name)
}

/// The values to step through with ←/→: a choice's, on/off, or the colour
/// names (`default` first); empty for a value that is typed.
pub fn steps(info: &OptInfo) -> &'static [&'static str] {
    const COLOURS: &[&str] = &[
        "default",
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
    match info.kind {
        Kind::Bool => &["off", "on"],
        Kind::Choice => option_values(info.name),
        Kind::Colour => COLOURS,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Options, SHOWABLE, parse_color};

    /// Every option `show-options -g` lists is on the board once, and
    /// nothing else is.
    #[test]
    fn every_option_is_listed_once() {
        for name in SHOWABLE {
            assert_eq!(OPTIONS.iter().filter(|o| o.name == *name).count(), 1, "{name}");
        }
        for o in OPTIONS {
            assert!(SHOWABLE.contains(&o.name), "{} is not an option", o.name);
            assert!(o.group < GROUPS.len(), "{}", o.name);
            assert!(!o.doc.is_empty(), "{}", o.name);
        }
        for (g, title) in GROUPS.iter().enumerate() {
            assert!(OPTIONS.iter().any(|o| o.group == g), "box {title} is empty");
        }
        // Each box lists its options together, in the order they show.
        let groups: Vec<usize> = OPTIONS.iter().map(|o| o.group).collect();
        assert!(groups.windows(2).all(|w| w[0] <= w[1]), "boxes in order: {groups:?}");
    }

    /// A value that can be picked is picked: whatever has a fixed few
    /// values is a choice or on/off, and every step a colour takes is one.
    #[test]
    fn what_can_be_picked_is_a_choice() {
        let d = Options::default();
        for o in OPTIONS {
            let fixed = option_values(o.name);
            if !fixed.is_empty() {
                assert!(matches!(o.kind, Kind::Choice | Kind::Bool), "{} has values {fixed:?}", o.name);
            }
            if o.kind == Kind::Bool {
                assert!(matches!(d.get(o.name).as_deref(), Some("on" | "off")), "{}", o.name);
            }
            if o.kind == Kind::Choice {
                assert!(!fixed.is_empty(), "{} is a choice of nothing", o.name);
            }
        }
        for c in steps(info("clock-mode-colour").unwrap()) {
            assert!(parse_color(c).is_ok(), "{c}");
        }
        // And each step is a value `set` takes.
        for o in OPTIONS {
            for v in steps(o) {
                let mut opts = Options::default();
                assert!(opts.set(o.name, v).is_ok(), "set {} {v}", o.name);
            }
        }
    }

    /// Every option's default goes back in through `set` and prints the
    /// same: what `set -u` and `reload-settings` put back is the default,
    /// and `save-settings` writes what reads back.
    #[test]
    fn every_default_reads_back_through_set() {
        let d = Options::default();
        for name in SHOWABLE {
            let v = d.get_to_save(name).unwrap_or_else(|| panic!("{name}: no value"));
            let mut o = Options::default();
            o.set(name, &v).unwrap_or_else(|e| panic!("set {name} {v:?}: {e}"));
            assert_eq!(o.get(name), d.get(name), "{name}");
        }
    }

    /// The line on each option is its line in the man page, word for word.
    #[test]
    fn each_option_says_what_the_man_page_says() {
        let man = include_str!("../docs/keepane.1.md");
        for o in OPTIONS {
            let head = format!("| `{}` |", o.name);
            let row = man
                .lines()
                .map(|l| l.trim_end_matches('\r'))
                .find(|l| l.starts_with(&head))
                .unwrap_or_else(|| panic!("no man page row for {}", o.name));
            assert!(row.ends_with(&format!("| {} |", o.doc)), "{}:\n  man: {row}\n  board: {}", o.name, o.doc);
        }
    }
}
