//! `keepane opencli`: keepane's commands, their flags and what each does,
//! as an OpenCLI 0.1 document (JSON; github.com/spectreconsole/open-cli,
//! draft.md, MIT), for a terminal that offers and explains them as they
//! are typed: UnifyTerm takes it with `keepane opencli | ut program add -`,
//! which `keepane opencli --install` runs.
//!
//! The commands and flags are the parser's own tables (`command::COMMANDS`,
//! `command::FLAGS`, `command::ALIASES`, `completion::LOCAL`); what each
//! one does is written here, once (`DOCS`). A test checks every command and
//! every flag has its line, and that a flag said to take a value is one the
//! parser asks a value of: one added without a line fails the tests rather
//! than going unexplained.

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// A command: what it does, in one sentence that fits a completion list
/// (`ABOUT_MOST`), and each flag: its name, the name of the value it takes
/// ("" for none) and what it does.
struct Doc {
    name: &'static str,
    about: &'static str,
    flags: &'static [(&'static str, &'static str, &'static str)],
}

const fn d(
    name: &'static str,
    about: &'static str,
    flags: &'static [(&'static str, &'static str, &'static str)],
) -> Doc {
    Doc { name, about, flags }
}

/// The program's one line.
const SUMMARY: &str = "Persistent terminal panes, each with its own inbox.";

/// What a pane, window or session target looks like, for `-t` and `-s`.
const TARGET: &str = "target";

/// Every command the parser takes and every one the client runs itself.
#[rustfmt::skip]
const DOCS: &[Doc] = &[
    d("attach-session", "Attach this terminal to a session.", &[
        ("-d", "", "Detach the session's other clients."),
        ("-t", TARGET, "The session to attach to."),
    ]),
    d("break-pane", "Move a pane into a window of its own.", &[
        ("-t", TARGET, "The pane to move."),
    ]),
    d("bind-key", "Bind a key to a command.", &[
        ("-n", "", "Bind it without the prefix (the root table)."),
        ("-r", "", "The key may repeat without the prefix again."),
        ("-T", "table", "The key table: root, prefix or copy-mode-vi."),
    ]),
    d("capture-pane", "Print what a pane shows and holds.", &[
        ("-p", "", "Print it (always done)."),
        ("-e", "", "Keep the colours as escape sequences."),
        ("-J", "", "Join lines that were wrapped."),
        ("-S", "line", "Start this many lines back (-N)."),
        ("-t", TARGET, "The pane."),
    ]),
    d("choose-buffer", "Pick a paste buffer from a list.", &[]),
    d("choose-client", "Pick an attached client to detach.", &[]),
    d("choose-history", "Browse what panes printed, a day a file.", &[
        ("-s", "text", "Only the lines of every day that hold it."),
    ]),
    d("choose-marks", "Pick a command a pane ran, newest first.", &[]),
    d("choose-ssh", "Pick a machine ssh reached to go to.", &[]),
    d("choose-jobs", "Pick a pane from the jobs board.", &[]),
    d("choose-session", "Pick a session from the tree.", &[
        ("-s", "", "Start with the sessions folded."),
        ("-w", "", "Start with the windows shown."),
        ("-Z", "", "Zoom the pane while picking."),
    ]),
    d("clock-mode", "Show a big clock in a pane.", &[
        ("-t", TARGET, "The pane."),
    ]),
    d("choose-tree", "Pick a session, window or pane.", &[
        ("-s", "", "Start with the sessions folded."),
        ("-w", "", "Start with the windows shown."),
        ("-Z", "", "Zoom the pane while picking."),
    ]),
    d("choose-window", "Pick a window from the tree.", &[
        ("-s", "", "Start with the sessions folded."),
        ("-w", "", "Start with the windows shown."),
        ("-Z", "", "Zoom the pane while picking."),
    ]),
    d("clear-history", "Forget the lines scrolled off this pane.", &[]),
    d("clear-prompt-history", "Forget what the : or / prompt took.", &[
        ("-T", "type", "Only this prompt: command or search."),
    ]),
    d("command-prompt", "Ask for a command at the prompt.", &[
        ("-p", "prompt", "The prompt's text."),
        ("-I", "text", "The text the prompt starts with."),
    ]),
    d("copy-mode", "Enter copy mode in a pane.", &[
        ("-u", "", "Scroll up a page at once."),
        ("-t", TARGET, "The pane."),
    ]),
    d("confirm-before", "Ask y/n before running a command.", &[
        ("-p", "prompt", "The question asked."),
    ]),
    d("delete-buffer", "Delete a paste buffer.", &[
        ("-b", "name", "The buffer (the newest without it)."),
    ]),
    d("delete-saved", "Delete a saved session.", &[]),
    d("detach-client", "Detach a client from its session.", &[
        ("-a", "", "Every client but this one."),
        ("-t", TARGET, "The client's session."),
    ]),
    d("display-menu", "Show a menu of commands.", &[
        ("-T", "title", "The menu's title."),
    ]),
    d("display-message", "Show or print a message or a format.", &[
        ("-p", "", "Print it (always done to a script)."),
        ("-t", TARGET, "The pane formats are read from."),
    ]),
    d("display-popup", "Run a command in a popup window.", &[
        ("-C", "", "Close the open popup."),
        ("-E", "", "Close it when the command exits."),
        ("-w", "width", "Its width, in columns or a percentage."),
        ("-h", "height", "Its height, in rows or a percentage."),
        ("-x", "column", "Its left edge."),
        ("-y", "row", "Its top edge."),
        ("-d", "dir", "The directory the command starts in."),
    ]),
    d("display-panes", "Show each pane's number over it.", &[
        ("-d", "ms", "How long the numbers stay."),
    ]),
    d("find-text", "Search what every pane printed.", &[
        ("-C", "", "Match the case."),
        ("-H", "", "Every day of the history log too."),
        ("-n", "hits", "At most this many hits a pane."),
        ("-t", TARGET, "Only this session, window or pane."),
    ]),
    d("find-window", "Find a window by its name or text.", &[
        ("-t", TARGET, "The session searched."),
    ]),
    d("focus-pane", "Bring every client to a pane.", &[]),
    d("has-session", "Exit 0 when a session exists.", &[
        ("-t", TARGET, "The session."),
    ]),
    d("if-shell", "Run one command or another by a test.", &[
        ("-b", "", "In the background (inline either way)."),
        ("-F", "", "The test is a format, not a shell command."),
        ("-t", TARGET, "The pane formats are read from."),
    ]),
    d("jobs", "List every pane: running or exited.", &[
        ("-t", TARGET, "Only this session."),
        ("-F", "format", "Print each pane in this format."),
    ]),
    d("join-pane", "Move a pane into another window.", &[
        ("-h", "", "Beside the target pane."),
        ("-v", "", "Below the target pane."),
        ("-b", "", "Before the target pane, not after."),
        ("-d", "", "Keep the current pane active."),
        ("-s", TARGET, "The pane moved."),
        ("-t", TARGET, "The pane it goes next to."),
    ]),
    d("kill-pane", "Close a pane.", &[
        ("-a", "", "Every pane but this one."),
        ("-t", TARGET, "The pane."),
    ]),
    d("kill-server", "Stop the server and every pane.", &[
        ("-r", "", "Tell clients it comes back (restart-server)."),
    ]),
    d("kill-session", "Close a session; its save is put aside.", &[
        ("-a", "", "Every session but this one."),
        ("-t", TARGET, "The session."),
    ]),
    d("kill-window", "Close a window.", &[
        ("-a", "", "Every window but this one."),
        ("-t", TARGET, "The window."),
    ]),
    d("last-pane", "Go to the pane used before.", &[
        ("-t", TARGET, "The window."),
    ]),
    d("last-window", "Go to the window used before.", &[
        ("-t", TARGET, "The session."),
    ]),
    d("link-add", "Pair with another machine's keepane.", &[]),
    d("link-allow", "Say what a paired machine may do here.", &[
        ("--full", "", "All of this machine, over Tailscale."),
        ("--lan", "", "With --full: over any network."),
        ("--none", "", "Take back what it was given."),
    ]),
    d("link-attach", "Attach to a session on another machine.", &[
        ("-t", "session", "The session there."),
        ("-x", "cols", "The width asked for."),
        ("-y", "rows", "The height asked for."),
    ]),
    d("link-capture", "Print a pane of another machine.", &[
        ("-S", "lines", "This many lines back."),
    ]),
    d("link-clone", "Clone a repository from another machine.", &[
        ("--tell", "pane", "Tell this pane when it is done."),
    ]),
    d("link-kill", "Close a pane on another machine.", &[]),
    d("link-push", "Send this repository's commits there.", &[
        ("-l", "", "List where pushes went."),
        ("--tell", "pane", "Tell this pane there when it lands."),
    ]),
    d("link-send-agents", "Send this project and its agents' chats.", &[
        ("-l", "", "List what went where."),
        ("-n", "", "Show what would go; send nothing."),
        ("-s", "chat-id", "Only the chats whose id starts so."),
    ]),
    d("link-start", "Start a pane on another machine.", &[
        ("-s", "session", "The session there."),
        ("-n", "name", "The pane's name."),
        ("-m", "mode", "Its work mode: normal, shell or ai."),
        ("-c", "dir", "The directory it starts in."),
    ]),
    d("link-id", "Print this machine's link key.", &[]),
    d("link-inbound", "List the machines that reach this one.", &[]),
    d("link-info", "Ask a paired machine about itself.", &[]),
    d("link-keys", "Send keys to a pane watched there.", &[
        ("-c", "client", "The watching client."),
    ]),
    d("link-list", "List the paired machines.", &[
        ("-J", "", "As JSON."),
    ]),
    d("link-panes", "List a paired machine's panes.", &[]),
    d("link-rekey", "Make this machine a new link key.", &[]),
    d("link-share", "Let a paired machine see these panes.", &[]),
    d("link-unshare", "Stop sharing panes with a machine.", &[
        ("--all", "", "Everything shared with it."),
    ]),
    d("link-view", "Watch a pane of another machine live.", &[
        ("-s", "since", "Start from this point of its output."),
        ("-c", "client", "The watching client."),
        ("-x", "cols", "The width asked for."),
        ("-y", "rows", "The height asked for."),
    ]),
    d("feishu-status", "Show the Feishu connection and its users.", &[]),
    d("feishu-allow", "Let a Feishu user reach the panes.", &[
        ("--shell", "", "It may send to shell panes."),
        ("--no-shell", "", "It may not send to shell panes."),
        ("--screen", "", "It may read the panes' screens."),
        ("--no-screen", "", "It may not read the screens."),
        ("--remote", "", "It may reach other machines' panes."),
        ("--no-remote", "", "It may reach this machine's only."),
    ]),
    d("feishu-remove", "Take a Feishu user's access away.", &[]),
    d("link-remove", "Unpair a machine.", &[]),
    d("link-connect", "Ask a paired machine again.", &[]),
    d("link-disconnect", "Stop asking a paired machine.", &[]),
    d("link-trust", "Pair with a machine by its key.", &[
        ("--full", "", "All of this machine, over Tailscale."),
        ("--lan", "", "With --full: over any network."),
    ]),
    d("list-buffers", "List the paste buffers.", &[]),
    d("list-clients", "List the attached clients.", &[]),
    d("list-commands", "List every command.", &[]),
    d("list-keys", "List the key bindings.", &[]),
    d("list-marks", "List the commands a pane ran.", &[
        ("-J", "", "As JSON, with each one's lines."),
        ("-t", TARGET, "The pane."),
    ]),
    d("show-changes", "List this server's changes, numbered.", &[
        ("-J", "", "As JSON."),
        ("-S", "", "Every pane's screen now instead."),
        ("-s", "N", "Only the changes after change N."),
        ("-t", TARGET, "Only this pane's."),
    ]),
    d("show-mirror", "Show the copies of other machines kept.", &[
        ("-J", "", "As JSON."),
        ("-t", TARGET, "A pane's screen in the copy."),
    ]),
    d("link-mirror", "Copy a machine's sessions again now.", &[
        ("--stop", "", "Stop copying its sessions."),
        ("-k", "", "Stop copying its sessions."),
    ]),
    d("list-agents", "List the panes' agents: model, cost, use.", &[
        ("-J", "", "As JSON."),
    ]),
    d("ssh-hosts", "List the machines ssh reached.", &[
        ("-J", "", "As JSON."),
    ]),
    d("trace-pane", "Show what a pane's agent did, step by step.", &[
        ("-J", "", "As JSON."),
        ("-t", TARGET, "The pane."),
    ]),
    d("status-line", "Print a status line as drawn.", &[
        ("-J", "", "Its parts and colours, as JSON."),
        ("-t", TARGET, "The session or pane."),
    ]),
    d("copy-output", "Copy what the last command printed.", &[
        ("-p", "", "Print it instead."),
        ("-t", TARGET, "The pane."),
    ]),
    d("hints", "Label paths and addresses on screen.", &[]),
    d("shell-history", "Print a pane's own shell history.", &[
        ("-c", "", "The commands only."),
        ("-m", "", "Every message sent to it."),
        ("-n", "count", "The last this many."),
        ("-t", TARGET, "The pane."),
    ]),
    d("list-done", "List when panes were done.", &[
        ("-a", "time", "Only those after this time."),
        ("-J", "", "As JSON."),
    ]),
    d("send-message", "Send a message to a pane's inbox.", &[
        ("-t", "pane", "The pane it goes to."),
        ("-r", "", "Reply to the message last read."),
        ("-w", "secs", "Wait this long for the answer."),
        ("--to", "pane", "The pane it goes to."),
        ("--re", "id", "The message it answers."),
        ("--task", "id", "The task it belongs to."),
        ("--in", "time", "Send it after this long (30m, 2h)."),
        ("--at", "time", "Send it at this time (14:30)."),
    ]),
    d("read-message", "Take the oldest message from the inbox.", &[
        ("-t", "pane", "The pane whose inbox."),
        ("-w", "secs", "Wait this long for one."),
    ]),
    d("list-messages", "List what waits in an inbox.", &[
        ("-t", "pane", "The pane whose inbox."),
        ("-a", "", "Every pane's inbox."),
        ("-J", "", "As JSON."),
    ]),
    d("trace-message", "Show a message's life: queued to done.", &[
        ("-w", "secs", "Wait until it is done with."),
        ("-J", "", "As JSON."),
    ]),
    d("drop-message", "Delete a queued message.", &[
        ("-u", "", "Bring back the one dropped last."),
    ]),
    d("move-message", "Move a queued message up or down.", &[]),
    d("pane-ready", "Say the pane's agent is free.", &[
        ("-q", "", "Say nothing outside a pane."),
        ("-t", "pane", "The pane."),
        ("--session", "id", "The agent's session id."),
        ("--transcript", "path", "The agent's transcript file."),
        ("--model", "name", "The agent's model."),
        ("--status", "status", "How its turn ended (aborted)."),
    ]),
    d("pane-wait", "Say the pane waits for you.", &[
        ("-q", "", "Say nothing outside a pane."),
        ("-t", "pane", "The pane."),
    ]),
    d("pane-status", "Set the pane's status text.", &[]),
    d("set-work-mode", "Set a pane's work mode.", &[
        ("-t", "pane", "The pane."),
    ]),
    d("rename-pane", "Name a pane; -t %name then finds it.", &[
        ("-t", "pane", "The pane."),
    ]),
    d("whoami", "Print this pane's address, name and mode.", &[]),
    d("list-tasks", "List the tasks messages belong to.", &[
        ("-t", TARGET, "Only this session's."),
    ]),
    d("dashboard", "Every pane at a glance, to act on.", &[]),
    d("create-pane", "Make a pane, as an agent asks.", &[
        ("-k", "kind", "What to make: session, window or split."),
        ("-t", TARGET, "Where it goes."),
        ("-s", "name", "The new session's name."),
        ("-h", "", "Split beside, not below."),
        ("-c", "dir", "The directory it starts in."),
        ("-n", "name", "The pane's name."),
        ("-m", "mode", "Its work mode: normal, shell or ai."),
        ("-M", "text", "A message for it to start with."),
    ]),
    d("show-task", "Show a task and its messages.", &[
        ("-J", "", "As JSON."),
    ]),
    d("list-events", "Print the event log.", &[
        ("-t", TARGET, "Only about this pane or session."),
        ("-S", "time", "Only this far back (1h, 2d)."),
        ("-n", "lines", "Only the last this many."),
    ]),
    d("list-panes", "List the panes.", &[
        ("-a", "", "Every pane on the server."),
        ("-s", "", "Every pane in the session."),
        ("-t", TARGET, "The window or session."),
        ("-F", "format", "Print each pane in this format."),
    ]),
    d("list-plugins", "List the plugins loaded.", &[]),
    d("list-saved", "List the saved sessions.", &[]),
    d("list-sessions", "List the sessions.", &[]),
    d("list-windows", "List the windows.", &[
        ("-a", "", "Every window on the server."),
        ("-t", TARGET, "The session."),
    ]),
    d("load-buffer", "Load a file into a paste buffer.", &[
        ("-b", "name", "The buffer's name."),
    ]),
    d("load-plugin", "Load a plugin.", &[]),
    d("move-pane", "Move a pane into another window.", &[
        ("-h", "", "Beside the target pane."),
        ("-v", "", "Below the target pane."),
        ("-b", "", "Before the target pane, not after."),
        ("-d", "", "Keep the current pane active."),
        ("-s", TARGET, "The pane moved."),
        ("-t", TARGET, "The pane it goes next to."),
    ]),
    d("move-window", "Move a window to another place.", &[
        ("-s", TARGET, "The window moved."),
        ("-t", TARGET, "Where it goes."),
    ]),
    d("new-session", "Make a new session.", &[
        ("-d", "", "Do not attach to it."),
        ("-s", "name", "Its name."),
        ("-n", "name", "Its first window's name."),
        ("-c", "dir", "The directory it starts in."),
        ("-A", "", "Attach to it when it exists."),
        ("-x", "cols", "Its width, detached."),
        ("-y", "rows", "Its height, detached."),
    ]),
    d("new-window", "Make a new window.", &[
        ("-d", "", "Do not go to it."),
        ("-n", "name", "Its name."),
        ("-c", "dir", "The directory it starts in."),
        ("-t", TARGET, "The session it goes in."),
        ("-a", "", "After the current one (always at the end)."),
    ]),
    d("next-layout", "Lay the panes out the next way.", &[
        ("-t", TARGET, "The window."),
    ]),
    d("next-window", "Go to the next window.", &[
        ("-a", "", "The next one with an alert."),
        ("-t", TARGET, "The session."),
    ]),
    d("notify", "Show a desktop notification.", &[
        ("-T", "title", "Its title."),
    ]),
    d("passthrough", "Send every key to the pane, prefix too.", &[
        ("-t", TARGET, "The pane."),
    ]),
    d("paste-buffer", "Paste a buffer into a pane.", &[
        ("-b", "name", "The buffer (the newest without it)."),
        ("-p", "", "As a bracketed paste."),
        ("-t", TARGET, "The pane."),
    ]),
    d("pipe-pane", "Pipe a pane's output to a command.", &[
        ("-o", "", "Only when no pipe is open (a toggle)."),
        ("-I", "", "The command's output goes into the pane."),
        ("-O", "", "The pane's output goes to the command."),
        ("-t", TARGET, "The pane."),
    ]),
    d("previous-layout", "Lay the panes out the previous way.", &[
        ("-t", TARGET, "The window."),
    ]),
    d("previous-window", "Go to the previous window.", &[
        ("-a", "", "The previous one with an alert."),
        ("-t", TARGET, "The session."),
    ]),
    d("record", "Record a pane's output as asciinema.", &[
        ("-t", TARGET, "The pane."),
    ]),
    d("refresh-client", "Redraw, or pan the view.", &[
        ("-U", "", "Pan the view up."),
        ("-D", "", "Pan the view down."),
        ("-L", "", "Pan the view left."),
        ("-R", "", "Pan the view right."),
        ("-S", "", "Accepted for tmux; does nothing."),
        ("-C", "size", "Accepted for tmux; does nothing."),
        ("-t", TARGET, "Accepted for tmux; does nothing."),
    ]),
    d("respawn-pane", "Start a pane's program again.", &[
        ("-k", "", "Kill the one running first."),
        ("-t", TARGET, "The pane."),
        ("-c", "dir", "Accepted for tmux; not used."),
    ]),
    d("respawn-window", "Start a window's program again.", &[
        ("-k", "", "Kill the one running first."),
        ("-t", TARGET, "The window."),
        ("-c", "dir", "Accepted for tmux; not used."),
    ]),
    d("rotate-window", "Rotate the panes of a window.", &[
        ("-D", "", "Downward."),
        ("-U", "", "Upward."),
        ("-t", TARGET, "The window."),
    ]),
    d("rename-session", "Rename a session.", &[
        ("-t", TARGET, "The session."),
    ]),
    d("rename-window", "Rename a window.", &[
        ("-t", TARGET, "The window."),
    ]),
    d("resize-pane", "Resize or zoom a pane.", &[
        ("-D", "", "Down by the amount given."),
        ("-U", "", "Up by the amount given."),
        ("-L", "", "Left by the amount given."),
        ("-R", "", "Right by the amount given."),
        ("-Z", "", "Zoom it, or unzoom it."),
        ("-x", "width", "To this width."),
        ("-y", "height", "To this height."),
        ("-t", TARGET, "The pane."),
    ]),
    d("resize-window", "Resize a window.", &[
        ("-x", "width", "To this width."),
        ("-y", "height", "To this height."),
        ("-U", "", "Up by the amount given."),
        ("-D", "", "Down by the amount given."),
        ("-L", "", "Left by the amount given."),
        ("-R", "", "Right by the amount given."),
        ("-A", "", "To the largest client's size."),
        ("-a", "", "To the smallest client's size."),
        ("-t", TARGET, "The window."),
    ]),
    d("restore-session", "Bring back a saved session.", &[
        ("-a", "", "Attach to it."),
        ("--prev", "", "The save before its last shape change."),
    ]),
    d("resume", "Bring back saved sessions after a reboot.", &[
        ("-a", "", "Attach to it."),
        ("-t", "name", "The session."),
        ("--prev", "", "The save before its last shape change."),
    ]),
    d("run-shell", "Run a shell command.", &[
        ("-b", "", "In the background."),
        ("-t", TARGET, "The pane it runs for."),
    ]),
    d("save-buffer", "Write a paste buffer to a file.", &[
        ("-a", "", "Append to the file."),
        ("-b", "name", "The buffer (the newest without it)."),
    ]),
    d("save-session", "Save a session now.", &[
        ("-a", "", "Every session."),
        ("-t", TARGET, "The session."),
    ]),
    d("select-layout", "Lay a window's panes out a way.", &[
        ("-n", "", "The next layout."),
        ("-p", "", "The previous layout."),
        ("-E", "", "Spread the panes out evenly."),
        ("-t", TARGET, "The window."),
    ]),
    d("select-pane", "Go to a pane, name it or mark it.", &[
        ("-U", "", "The pane above."),
        ("-D", "", "The pane below."),
        ("-L", "", "The pane to the left."),
        ("-R", "", "The pane to the right."),
        ("-l", "", "The pane used before."),
        ("-m", "", "Mark the pane."),
        ("-M", "", "Clear the mark."),
        ("-T", "title", "Set the pane's title."),
        ("-t", TARGET, "The pane."),
    ]),
    d("select-window", "Go to a window.", &[
        ("-t", TARGET, "The window."),
    ]),
    d("send-keys", "Type keys into a pane.", &[
        ("-l", "", "The words as text, not key names."),
        ("-R", "", "Accepted for tmux; does nothing."),
        ("-t", TARGET, "The pane."),
    ]),
    d("send-prefix", "Send the prefix key to a pane.", &[
        ("-t", TARGET, "The pane."),
    ]),
    d("set-buffer", "Set a paste buffer's text.", &[
        ("-b", "name", "The buffer's name."),
        ("-a", "", "Append to it."),
    ]),
    d("set-cwd", "Record the directory a pane resumes in.", &[
        ("-t", TARGET, "The pane."),
    ]),
    d("set-environment", "Set a variable for every pane.", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-u", "", "Take the variable out."),
    ]),
    d("set-hook", "Run a command when something happens.", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-u", "", "Take the hook out."),
    ]),
    d("set-option", "Set an option.", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-a", "", "Append to the value."),
        ("-w", "", "Accepted for tmux: always global."),
        ("-s", "", "Accepted for tmux: always global."),
        ("-t", TARGET, "The pane or window it is for."),
    ]),
    d("set-window-option", "Set an option (as set-option).", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-a", "", "Append to the value."),
        ("-t", TARGET, "The window it is for."),
    ]),
    d("show-buffer", "Print a paste buffer.", &[
        ("-b", "name", "The buffer (the newest without it)."),
    ]),
    d("show-environment", "Print the panes' environment.", &[
        ("-g", "", "Accepted for tmux: always global."),
    ]),
    d("show-hooks", "List the hooks set.", &[
        ("-g", "", "Accepted for tmux: always global."),
    ]),
    d("show-messages", "List the messages shown lately.", &[]),
    d("show-options", "Print the options and their values.", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-v", "", "The value only."),
        ("-q", "", "No error for an unknown option."),
        ("-w", "", "Accepted for tmux: always global."),
        ("-s", "", "Accepted for tmux: always global."),
    ]),
    d("show-prompt-history", "List what the : or / prompt took.", &[
        ("-T", "type", "Only this prompt: command or search."),
    ]),
    d("show-window-options", "Print the options (as show-options).", &[
        ("-g", "", "Accepted for tmux: always global."),
        ("-v", "", "The value only."),
    ]),
    d("source-file", "Run the commands in a config file.", &[
        ("-q", "", "No error for a missing file."),
    ]),
    d("split-window", "Split a pane in two.", &[
        ("-h", "", "Side by side."),
        ("-v", "", "One above the other."),
        ("-b", "", "The new pane before, not after."),
        ("-d", "", "Do not go to the new pane."),
        ("-f", "", "Across the whole window."),
        ("-c", "dir", "The directory it starts in."),
        ("-t", TARGET, "The pane split."),
        ("-N", "count", "Make this many panes (1 to 64)."),
    ]),
    d("start-server", "Start the server.", &[]),
    d("swap-pane", "Swap two panes.", &[
        ("-U", "", "With the one before."),
        ("-D", "", "With the one after."),
        ("-d", "", "Keep the active pane where it is."),
        ("-s", TARGET, "The pane swapped."),
        ("-t", TARGET, "The pane it swaps with."),
    ]),
    d("swap-window", "Swap two windows.", &[
        ("-d", "", "Keep the current window."),
        ("-s", TARGET, "The window swapped."),
        ("-t", TARGET, "The window it swaps with."),
    ]),
    d("switch-client", "Show another session in this client.", &[
        ("-n", "", "The next session."),
        ("-p", "", "The previous session."),
        ("-l", "", "The session used before."),
        ("-t", TARGET, "The session."),
    ]),
    d("unbind-key", "Take a key binding out.", &[
        ("-n", "", "From the root table."),
        ("-T", "table", "The key table: root, prefix or copy-mode-vi."),
    ]),
    d("undo-kill", "Bring back the pane or window just killed.", &[]),
    d("version", "Print this keepane's version and the server's.", &[]),
    d("wait-for", "Wait for, or signal, a channel.", &[
        ("-L", "", "Lock the channel."),
        ("-U", "", "Unlock the channel."),
        ("-S", "", "Signal the channel."),
    ]),
    d("wait-output", "Wait until a pane prints some text.", &[
        ("-a", "", "Count what it already holds."),
        ("-i", "", "In any case."),
        ("-t", TARGET, "The pane."),
        ("-w", "time", "Give up after this long."),
    ]),
    d("web-fit", "Fit a pane to a phone's screen.", &[
        ("-t", TARGET, "The pane."),
        ("-x", "cols", "The phone's width."),
        ("-y", "rows", "The phone's height."),
        ("-u", "", "Back to its own size."),
    ]),
    d("web-resume", "Serve the phone page again after a restart.", &[]),
    d("web-start", "Serve the panes to a phone.", &[
        ("-p", "port", "The port."),
        ("-b", "ip", "The addresses to listen on."),
        ("-r", "", "Read-only."),
        ("-k", "", "Keep the key it had."),
    ]),
    d("web-status", "Show where the phone page is served.", &[]),
    d("web-stop", "Stop serving the phone page.", &[]),
    d("web-tails", "The panes' last lines, for the phone page.", &[]),
    d("share", "Share a file or folder by a link.", &[
        ("-l", "", "List what is shared."),
        ("--for", "time", "For this long (24h without it)."),
        ("--once", "", "For one download."),
        ("--all", "", "With stop: everything shared."),
    ]),
    d("chat-new", "Make a group chat channel.", &[
        ("--topic", "text", "Its topic."),
        ("--tag", "tag", "A tag (as many as wanted)."),
    ]),
    d("chat-list", "List the channels.", &[
        ("--tag", "tag", "Only those with this tag."),
        ("-J", "", "As JSON."),
        ("--cached", "", "Another machine's as last heard."),
        ("--machines", "", "The paired machines' too."),
    ]),
    d("chat-join", "Join a channel as this pane.", &[
        ("--as", "handle", "The handle it goes by."),
        ("--role", "role", "Its role there."),
        ("--hear", "what", "Told of: all, or mentions."),
    ]),
    d("chat-leave", "Leave a channel.", &[]),
    d("chat-say", "Say something in a channel.", &[
        ("--file", "path", "Send a file with it (up to 10)."),
        ("--re", "N", "Reply in line N's thread."),
    ]),
    d("chat-read", "Read what was said in a channel.", &[
        ("-n", "N", "At most N lines."),
        ("--all", "", "The last N, read or not."),
        ("--peek", "", "Without moving the read position."),
        ("--wait", "secs", "Wait this long for something new."),
        ("--thread", "N", "Line N's thread."),
        ("-J", "", "As JSON."),
        ("--cached", "", "Another machine's as last heard."),
    ]),
    d("chat-who", "List a channel's members.", &[
        ("-J", "", "As JSON."),
        ("--cached", "", "Another machine's as last heard."),
    ]),
    d("chat-topic", "Set a channel's topic.", &[]),
    d("chat-tag", "Add or take away a channel's tags.", &[]),
    d("chat-rename", "Rename a channel.", &[]),
    d("chat-delete", "Delete a channel; its lines put aside.", &[]),
    d("chat-dir", "Print the folder a channel's files go in.", &[]),
    d("web-share-open", "A share's link was opened (the page's).", &[]),
    d("web-share-sent", "A share was downloaded (the page's).", &[]),
    d("web-uploaded", "A file came from the phone (the page's).", &[]),
    // Those the client runs itself (`completion::LOCAL`).
    d("chat", "Group chat: channels where panes talk.", &[]),
    d("completion", "Print a Tab completer for a shell.", &[]),
    d("feishu", "Panes from a Feishu (Lark) app of yours.", &[
        ("--shell", "", "With allow: it may send to shell panes."),
        ("--no-shell", "", "With allow: not to shell panes."),
        ("--screen", "", "With allow: it may read the screens."),
        ("--no-screen", "", "With allow: not the screens."),
    ]),
    d("import-config", "Bring a tmux config's settings in.", &[
        ("-n", "", "Show what it would write; write nothing."),
        ("-o", "file", "Write to this file."),
    ]),
    d("link", "Panes on other machines.", &[]),
    d("man", "Print the manual as Markdown.", &[
        ("--roff", "", "As roff, for man(1)."),
    ]),
    d("mcp", "Serve MCP to the agent in this pane.", &[]),
    d("opencli", "Print the commands as OpenCLI JSON.", &[
        ("--install", "", "Give them to UnifyTerm's ut."),
    ]),
    d("restart-server", "Move the sessions to this version's server.", &[]),
    d("setup", "Hook an agent up to keepane.", &[
        ("--install", "", "Install what it lacks."),
    ]),
    d("show-keys", "Print each key the terminal sends.", &[]),
    d("startup", "Start the server at logon.", &[]),
    d("update", "Update keepane the way it was installed.", &[
        ("--check", "", "Only say whether there is a newer one."),
    ]),
    d("view", "Read a history log file in the pager.", &[
        ("-n", "line", "Open at this line."),
        ("-s", "text", "Search for this text."),
    ]),
    d("web", "Serve the panes to a phone.", &[
        ("--port", "port", "The port."),
        ("--bind", "ip", "The addresses to listen on."),
        ("--read-only", "", "Nothing can be typed or changed."),
        ("--keep-key", "", "Keep the key it had."),
    ]),
    d("windows-terminal", "A keepane profile in Windows Terminal.", &[]),
];

/// Commands keepane runs for itself, not for a person to type.
const HIDDEN: &[&str] = &[
    "web-fit",
    "web-resume",
    "web-start",
    "web-tails",
    "web-share-open",
    "web-share-sent",
    "web-uploaded",
    "link-keys",
    "link-inbound",
];

/// The commands typed as a group's words (`keepane chat say`): the group,
/// and the server commands its words stand for, each with the word.
const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "chat",
        &[
            ("new", "chat-new"),
            ("list", "chat-list"),
            ("join", "chat-join"),
            ("leave", "chat-leave"),
            ("say", "chat-say"),
            ("read", "chat-read"),
            ("who", "chat-who"),
            ("topic", "chat-topic"),
            ("tag", "chat-tag"),
            ("rename", "chat-rename"),
            ("delete", "chat-delete"),
            ("dir", "chat-dir"),
        ],
    ),
    ("feishu", &[("status", "feishu-status"), ("allow", "feishu-allow"), ("remove", "feishu-remove")]),
    (
        "link",
        &[
            ("add", "link-add"),
            ("allow", "link-allow"),
            ("attach", "link-attach"),
            ("capture", "link-capture"),
            ("clone", "link-clone"),
            ("connect", "link-connect"),
            ("disconnect", "link-disconnect"),
            ("id", "link-id"),
            ("info", "link-info"),
            ("kill", "link-kill"),
            ("list", "link-list"),
            ("mirror", "link-mirror"),
            ("panes", "link-panes"),
            ("push", "link-push"),
            ("rekey", "link-rekey"),
            ("remove", "link-remove"),
            ("send-agents", "link-send-agents"),
            ("share", "link-share"),
            ("start", "link-start"),
            ("trust", "link-trust"),
            ("unshare", "link-unshare"),
            ("watch", "link-view"),
        ],
    ),
    ("web", &[("status", "web-status"), ("stop", "web-stop")]),
];

/// The words a few commands take, of a fixed set.
const VALUES: &[(&str, &str, &[&str])] = &[
    ("completion", "shell", &["bash", "fish", "powershell", "zsh"]),
    ("set-work-mode", "mode", &["normal", "shell", "ai"]),
    ("startup", "action", &["on", "off", "status"]),
    ("windows-terminal", "action", &["install", "remove", "status"]),
];

/// Values a flag takes, of a fixed set.
const FLAG_VALUES: &[(&str, &str, &[&str])] = &[
    ("bind-key", "-T", &["root", "prefix", "copy-mode-vi"]),
    ("unbind-key", "-T", &["root", "prefix", "copy-mode-vi"]),
    ("show-prompt-history", "-T", crate::command::PROMPT_TYPES),
    ("clear-prompt-history", "-T", crate::command::PROMPT_TYPES),
    ("chat-join", "--hear", &["all", "mentions"]),
    ("create-pane", "-m", &["normal", "shell", "ai"]),
    ("create-pane", "-k", &["session", "window", "split"]),
    ("link-start", "-m", &["normal", "shell", "ai"]),
];

/// Other names of commands the client knows, besides `command::ALIASES`.
const LOCAL_ALIASES: &[(&str, &str)] = &[("wt", "windows-terminal")];

fn doc(name: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|d| d.name == name)
}

/// A flag as OpenCLI writes it.
fn option(command: &str, (name, value, about): (&str, &str, &str)) -> Value {
    let mut o = json!({ "name": name, "description": about });
    if !value.is_empty() {
        let mut arg = json!({ "name": value });
        if let Some((_, _, accepted)) = FLAG_VALUES.iter().find(|(c, f, _)| *c == command && *f == name) {
            arg["acceptedValues"] = json!(accepted);
        }
        o["arguments"] = json!([arg]);
    }
    o
}

/// A command as OpenCLI writes it, under the name it is typed as.
fn command(typed: &str, d: &Doc, hidden: bool) -> Value {
    let mut c = json!({ "name": typed, "description": d.about });
    let aliases: Vec<&str> = crate::command::ALIASES
        .iter()
        .chain(LOCAL_ALIASES)
        .filter(|(alias, full)| *full == d.name && *alias != d.name && typed == d.name)
        .map(|(alias, _)| *alias)
        .collect();
    if !aliases.is_empty() {
        c["aliases"] = json!(aliases);
    }
    if hidden {
        c["hidden"] = json!(true);
    }
    if !d.flags.is_empty() {
        c["options"] = d.flags.iter().map(|f| option(d.name, *f)).collect();
    }
    if let Some((_, name, accepted)) = VALUES.iter().find(|(c, _, _)| *c == d.name) {
        c["arguments"] = json!([{ "name": name, "acceptedValues": accepted }]);
    }
    if d.name == "setup" {
        let agents: Vec<&str> = crate::setup::AGENTS.iter().map(|a| a.word()).collect();
        c["arguments"] =
            json!([{ "name": "agent", "acceptedValues": agents, "arity": { "minimum": 0, "maximum": 1 } }]);
    }
    if let Some((_, words)) = GROUPS.iter().find(|(g, _)| *g == d.name) {
        c["commands"] =
            words.iter().filter_map(|(word, full)| doc(full).map(|sub| command(word, sub, false))).collect();
    }
    c
}

/// keepane's commands as an OpenCLI 0.1 document.
pub fn document() -> Value {
    let grouped = |name: &str| GROUPS.iter().any(|(_, words)| words.iter().any(|(_, full)| *full == name));
    let mut names: Vec<&str> = crate::command::COMMANDS.iter().chain(crate::completion::LOCAL).copied().collect();
    names.sort_unstable();
    names.dedup();
    let commands: Vec<Value> =
        names.iter().filter_map(|n| doc(n).map(|d| command(n, d, HIDDEN.contains(n) || grouped(n)))).collect();
    json!({
        "opencli": "0.1",
        "info": { "title": "keepane", "summary": SUMMARY, "version": env!("CARGO_PKG_VERSION") },
        "command": {
            "name": "keepane",
            "options": [
                { "name": "-L", "arguments": [{ "name": "socket-name" }], "description": "The server to talk to (default: default)." },
                { "name": "--help", "aliases": ["-h"], "description": "Print the commands and their flags." },
                { "name": "--version", "aliases": ["-V"], "description": "Print this keepane's version." },
            ],
            "commands": commands,
        },
    })
}

/// `keepane opencli [--install]`: the document on standard output, or
/// given to UnifyTerm's `ut program add -`.
pub fn run(args: &[String]) -> Result<i32> {
    let text = serde_json::to_string_pretty(&document())?;
    match args {
        [] => {
            println!("{text}");
            Ok(0)
        }
        [a] if a == "--install" => install(&text),
        _ => bail!("opencli: takes nothing, or --install"),
    }
}

/// The document given to `ut program add -`: what ut says, and its code.
fn install(text: &str) -> Result<i32> {
    let Some(ut) = crate::dashboard::ut::find() else {
        bail!("opencli --install: UnifyTerm's ut is not on this machine (keepane opencli prints the list)")
    };
    let (code, said) = add_to(&ut, text)?;
    print!("{said}");
    if code != 0 {
        eprintln!("keepane: opencli --install: {} program add failed (a ut older than `ut program`?)", ut.display());
    }
    Ok(code)
}

/// `text` handed to `ut program add -` of the `ut` at `ut`: its exit code
/// and what it said.
fn add_to(ut: &std::path::Path, text: &str) -> Result<(i32, String)> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(ut)
        .args(["program", "add", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("opencli: {}: {e}", ut.display()))?;
    // A ut that does not read it (one older than `ut program` exits at
    // once, saying so) closes the pipe: what it said tells why, not that.
    if let Err(e) = child.stdin.take().expect("piped").write_all(text.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e.into());
    }
    let out = child.wait_with_output()?;
    let said = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    Ok((out.status.code().unwrap_or(1), said))
}

/// The version of keepane's list in what `ut program` prints ("keepane
/// 0.89.2: 237 commands, 430 options"); none when it lists none.
fn listed_version(said: &str) -> Option<&str> {
    said.lines().find_map(|l| l.strip_prefix("keepane ")).and_then(|r| r.split(':').next()).map(str::trim)
}

/// At a server's start: UnifyTerm's list of keepane's commands made this
/// version's when it holds another version's (keepane was updated, or
/// moved back, in whatever way), on a thread of its own. Nothing when
/// there is no ut, no `ut program` or no list of keepane's (one is only
/// given by `opencli --install`), nor for a server of the tests
/// (`KEEPANE_SESSIONS_DIR`), which must not write into the person's
/// UnifyTerm.
pub fn refresh_in_background() {
    if std::env::var_os("KEEPANE_SESSIONS_DIR").is_some_and(|d| !d.is_empty()) {
        return;
    }
    let spawned = std::thread::Builder::new().name("opencli".into()).spawn(|| {
        let Some(ut) = crate::dashboard::ut::find() else { return };
        let Ok(out) = std::process::Command::new(&ut).arg("program").stdin(std::process::Stdio::null()).output() else {
            return;
        };
        let said = String::from_utf8_lossy(&out.stdout);
        let this = env!("CARGO_PKG_VERSION");
        match listed_version(&said) {
            Some(v) if out.status.success() && v != this => {
                let text = serde_json::to_string(&document()).expect("a JSON value");
                match add_to(&ut, &text) {
                    Ok((0, _)) => log::info!("opencli: UnifyTerm's list of keepane {v} brought up to {this}"),
                    Ok((code, said)) => log::warn!("opencli: ut program add: {code}: {}", said.trim()),
                    Err(e) => log::warn!("opencli: {e:#}"),
                }
            }
            _ => {}
        }
    });
    if let Err(e) = spawned {
        log::warn!("opencli: no thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest first sentence a completion list shows (UnifyTerm's).
    const ABOUT_MOST: usize = 56;

    /// What `ut program` lists: keepane's version, or none.
    #[test]
    fn the_listed_version_is_read() {
        assert_eq!(
            listed_version("git 2.47.0: 170 commands, 900 options\nkeepane 0.89.2: 237 commands, 430 options\n"),
            Some("0.89.2")
        );
        assert_eq!(listed_version("git 2.47.0: 170 commands, 900 options\n"), None);
        assert_eq!(listed_version(""), None);
    }

    /// Every command has its line, and only commands do.
    #[test]
    fn every_command_is_explained() {
        let all: Vec<&str> = crate::command::COMMANDS.iter().chain(crate::completion::LOCAL).copied().collect();
        for n in &all {
            assert_eq!(DOCS.iter().filter(|d| d.name == *n).count(), 1, "{n}: one line in DOCS");
        }
        for d in DOCS {
            assert!(all.contains(&d.name), "{} is in DOCS but not a command", d.name);
        }
        for h in HIDDEN {
            assert!(all.contains(h), "hidden {h} is not a command");
        }
        for (g, words) in GROUPS {
            assert!(crate::completion::LOCAL.contains(g), "group {g}");
            for (_, full) in *words {
                assert!(crate::command::COMMANDS.contains(full), "{g}: {full}");
            }
        }
    }

    /// What a list shows: one sentence, short enough; a flag's too.
    #[test]
    fn each_line_fits_a_list() {
        for d in DOCS {
            assert!(d.about.chars().count() <= ABOUT_MOST, "{}: {:?} is longer than {ABOUT_MOST}", d.name, d.about);
            assert!(
                d.about.ends_with('.') && !d.about.trim_end_matches('.').contains(". "),
                "{}: one sentence",
                d.name
            );
            for (f, _, about) in d.flags {
                assert!(about.chars().count() <= ABOUT_MOST, "{} {f}: {about:?}", d.name);
                assert!(about.ends_with('.'), "{} {f}: a sentence", d.name);
            }
        }
    }

    /// The server's commands' flags are the parser's table, and a flag
    /// said to take a value is one the parser asks a value of.
    #[test]
    fn the_flags_are_the_parsers() {
        let mut bad = Vec::new();
        for (cmd, flags) in crate::command::FLAGS {
            let d = doc(cmd).unwrap_or_else(|| panic!("{cmd}: no line"));
            let mut ours: Vec<&str> = d.flags.iter().map(|(f, _, _)| *f).collect();
            let mut theirs: Vec<&str> = flags.to_vec();
            ours.sort_unstable();
            theirs.sort_unstable();
            assert_eq!(ours, theirs, "{cmd}: the flags explained are the flags taken");
            for (f, value, _) in d.flags {
                // The flag last, after none, one or two words (a channel,
                // a machine come first in some).
                // (`link-start`'s first word is a machine's host:port.)
                let asked = [&[][..], &["x"][..], &["x", "y"][..], &["h.test:1"][..]].iter().any(|before| {
                    let words: Vec<String> =
                        std::iter::once(*cmd).chain(before.iter().copied()).chain([*f]).map(String::from).collect();
                    // `chat-say` says what its two take: "--file takes a path"
                    crate::command::parse(&words)
                        .is_err_and(|e| e.contains("missing value") || e.contains(&format!("{f} takes a")))
                });
                if asked == value.is_empty() {
                    bad.push(format!("{cmd} {f}: the parser asks a value: {asked}"));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The flags of those the client runs itself are completion's.
    #[test]
    fn the_local_flags_are_completions() {
        for (cmd, flags) in crate::completion::LOCAL_FLAGS {
            if *cmd == "link" {
                continue; // its subcommands' (the server's link-* flags)
            }
            let d = doc(cmd).unwrap_or_else(|| panic!("{cmd}: no line"));
            for f in *flags {
                assert!(d.flags.iter().any(|(g, _, _)| g == f), "{cmd} {f}: explained");
            }
        }
    }

    /// The document UnifyTerm reads: its root, a command with its alias,
    /// flags and a value; a group's words; no name it would drop.
    #[test]
    fn the_document_is_what_unifyterm_reads() {
        let v = document();
        assert_eq!(v["opencli"], "0.1");
        assert_eq!(v["info"]["title"], "keepane");
        assert_eq!(v["info"]["version"], env!("CARGO_PKG_VERSION"));
        let commands = v["command"]["commands"].as_array().unwrap();
        let find =
            |list: &[Value], n: &str| list.iter().find(|c| c["name"] == n).cloned().unwrap_or_else(|| panic!("{n}"));
        let split = find(commands, "split-window");
        assert!(split["aliases"].as_array().unwrap().iter().any(|a| a == "splitw"));
        let c = split["options"].as_array().unwrap().iter().find(|o| o["name"] == "-c").unwrap().clone();
        assert_eq!(c["arguments"][0]["name"], "dir");
        let h = split["options"].as_array().unwrap().iter().find(|o| o["name"] == "-h").unwrap().clone();
        assert!(h.get("arguments").is_none(), "-h takes no value");
        assert_eq!(
            find(commands, "completion")["arguments"][0]["acceptedValues"],
            json!(["bash", "fish", "powershell", "zsh"])
        );
        let chat = find(commands, "chat");
        let say = find(chat["commands"].as_array().unwrap(), "say");
        assert_eq!(say["description"], doc("chat-say").unwrap().about);
        assert_eq!(find(commands, "chat-say")["hidden"], true, "typed as `chat say`");
        assert_eq!(find(commands, "web-share-open")["hidden"], true);
        let watch = find(find(commands, "link")["commands"].as_array().unwrap(), "watch");
        assert_eq!(watch["description"], doc("link-view").unwrap().about);
        // UnifyTerm drops a name with whitespace or a shell character.
        const BAD: &str = ";|&$`<>(){}'\"\\!*?";
        fn names(c: &Value, out: &mut Vec<String>) {
            out.push(c["name"].as_str().unwrap_or_default().to_string());
            for a in c["aliases"].as_array().into_iter().flatten() {
                out.push(a.as_str().unwrap().to_string());
            }
            for o in c["options"].as_array().into_iter().flatten() {
                names(o, out);
            }
            for s in c["commands"].as_array().into_iter().flatten() {
                names(s, out);
            }
        }
        let mut all = Vec::new();
        names(&v["command"], &mut all);
        for n in &all {
            assert!(!n.is_empty() && !n.chars().any(|ch| ch.is_whitespace() || BAD.contains(ch)), "{n:?}");
        }
        assert!(all.len() > 600, "{} names", all.len());
    }
}
