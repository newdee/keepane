# KEEPANE 1

## NAME

keepane - persistent terminal panes, each with its own inbox

## SYNOPSIS

`keepane` [`-L` *socket-name*] [*command* [*flags*] ...]

`keepane -V` | `keepane --help`

## DESCRIPTION

keepane runs programs in panes, inside windows, inside sessions, in a server
that keeps them running after the terminal that showed them is closed. A
client attaches to a session to show it and type into it, from any terminal.
Its keys, commands and config are tmux's: `C-b` is the prefix, `C-b %`
splits, `C-b d` detaches, and `keepane attach` comes back.

Panes can also be named and pass messages to each other: a message sent to
a pane waits in its inbox until the pane is free, then goes in; a pane in
*shell* work mode runs it as a command and its output goes on record, and an
AI agent in a pane in *ai* work mode reads it and answers. Everything a
message does is in an event log.

With no command, `keepane` goes into what is there (`new-session -A` with no
name): the session used last; with none running, every saved session back
(not those killed by hand) and the one saved last; with nothing at all, a
new session. `keepane new` always starts a new one. A command name can be
cut to any unambiguous prefix (`keepane att`, `keepane splitw -h`). Run
inside a pane, a command goes to the server that pane belongs to, as
tmux's do with `$TMUX`.

This page is `docs/keepane.1.md` in keepane's repository, and
`keepane man` prints it as it is written (Markdown, for people and for
programs that read it); `man keepane` shows it where the manual page is
installed (`keepane man --roff` writes one). The README has every
command's flags and examples.

## OPTIONS

`-L` *socket-name*
: Talk to (or start) the server with this name instead of `default`. Servers
  with different names are separate: their sessions, options and key
  bindings are their own.

`-V`, `--version`
: Print keepane's version.

`-h`, `--help`, `help`
: Print a summary of every command.

## COMMANDS

Each command below is also what the `:` prompt (`C-b :`), key bindings and
the config file take. `keepane list-commands` lists them, and
`keepane --help` gives each one's flags.

### Sessions

`new-session` (`new`), `attach-session` (`attach`), `list-sessions` (`ls`),
`has-session`, `kill-session`, `kill-server`, `rename-session`,
`switch-client`, `detach-client`, `start-server`, `choose-session`,
`choose-tree` (sessions, windows and panes, and the paired machines' in the chart, each other machine's block its name over its address; a chart of rows by default, a tree of blocks with `choose-tree-style tree`, the plain list with `choose-tree-style list`; `v` goes to the next), `choose-client`, `list-clients`.

### Windows and panes

`new-window` (`neww`), `kill-window`, `rename-window`, `select-window`,
`next-window`, `previous-window`, `last-window`, `list-windows`,
`swap-window`, `move-window`, `rotate-window`, `resize-window`,
`respawn-window`, `choose-window`, `find-window`, `select-layout`,
`next-layout`, `previous-layout`, `split-window` (`splitw`), `kill-pane`,
`select-pane`, `last-pane`, `resize-pane`, `swap-pane`, `break-pane`,
`join-pane`, `move-pane`, `respawn-pane`, `list-panes`, `display-panes`,
`capture-pane`, `send-keys` (`WheelUp`, `WheelDown`: what the wheel sends a full-screen or mouse-reading program; nothing otherwise), `send-prefix`, `pipe-pane`, `clear-history`,
`record` (the pane's output as an asciinema file), `focus-pane`,
`passthrough` (`[-t pane] [on|off]`: every key typed in the pane goes to
it, the prefix too; see KEYS),
`undo-kill` (the pane or window killed in the last `undo-kill-time`
seconds comes back).

### Copy mode and buffers

`copy-mode`, `set-buffer`, `list-buffers`, `show-buffer`, `delete-buffer`,
`choose-buffer`, `load-buffer`, `save-buffer`, `paste-buffer`. `copy-output`
(what the last command printed, to a buffer and the clipboard; `-p` prints
it) and `hints` (label the paths, addresses and hashes on screen to copy or,
in capitals, open one; `hint-open`).

### Seeing what happened

`jobs` and `choose-jobs` (every pane: running or exited, for how long),
`find-text` (search what every pane printed; `-H` every day of the history
log too, each hit with its day and line), `choose-history` and `view`
(what panes printed, a file a day; `choose-history -s text`, `/` in it,
the lines of every day that hold it, each opened at its line, which `view -n
LINE -s TEXT` does), `list-marks` (the commands a pane ran; `-J` as JSON, with the lines each takes),
`show-changes` (this server's changes, numbered: the sessions' shape,
what each pane printed, and, as labels on that, the commands its shell
ran and the full-screen programs; `-s N` those after change N, `-t` one
pane's, `-J` as JSON, `-S` every pane's screen now instead),
`choose-marks` (the commands every pane's shell ran, newest first: Enter
goes to one in copy mode, `y` copies what it printed, `r` runs it again),
`shell-history` (a pane's own shell history, the messages delivered to it
marked; `-c` the commands only, `-m` every message sent to it),
`list-done` (when panes were done: `done-events`, told to the phone, `notify`,
the `pane-done` hook and `done-webhook`, signed with `done-webhook-secret`
for a Feishu or DingTalk bot that checks), `pane-wait` (the pane waits for
you: `done-events` `wait`; an agent's hook gives what it asks on standard
input),
`show-messages`, `clock-mode`, `dashboard` (panes, inboxes, tasks and the
chosen pane's screen, to act on; the panes by session, and first by
machine, its name and address, while another's sessions are here;
its Git tab, the pane's repository or
files; from 150 columns, on the left, `[0]` the pane's directory, each
entry an emoji of its kind (`dashboard-icons nerd`: a Nerd Font's icon),
what is under the cursor on the right (a picture drawn by `ut view` and
markdown by `ut md` where UnifyTerm's `ut` is on the PATH, in
`UNIFYTERM_COMMANDS` or in `%LOCALAPPDATA%\unifyterm\bin`),
over the machine's CPU, memory, disk and network; `>` and `<` turn to
page 2, the group chat (its `[0]` the picked channel's group files, the
one under the cursor shown on the right, another machine's fetched by its
link first; `y` copies its path or link, `d` deletes it after a yes), and
page 3, what is shared; it opens on the page
and row the last one was left on, `board-pick` below), `list-agents` (the
Claude Code, Codex, pi
and Cursor CLI the panes run: model, cost, tokens, context, CPU, memory;
Cursor's CPU and memory only; `-J` as JSON),
`trace-pane` (what a pane's agent did, step by step: its latest turns, each
call of the model, each tool and hook, from when to when, failed or not
and why (`denied`, `hook: …`), what runs in the background, what it waits
on you for; a tree, or `-J` JSON; the dashboard's Trace tab (`T`) draws it
as a waterfall, each kind in its colour, three or more finished tool
steps of a kind in a row as one row (`+N`, `×N` at once; not a failed or
background one), each turn's steps summed, the latest step in sight, `e`
opening every turn),
`status-line` (a session's status line as drawn; `-J` its parts, their colours
and each window's pane, which the phone page draws under the screen; `-t` a
pane: drawn where that pane is shown, its window the current one),
`choose-ssh` (prefix `@`: the machines `ssh` reached, connected ones first,
then by last use; Enter goes to the pane on one, else connects in a new
window named after it; `s` connects in a split, `y` copies the command,
`d` removes one, `f` filters), `ssh-hosts` (the same list; `-J` as JSON).

### Panes that pass messages

`rename-pane` (then `-t %name` finds it), `whoami`, `set-work-mode`
(`normal`, `shell` or `ai`; what waits in the inbox for `normal` or `ai`
goes with the pane into the other, what waits for `shell` stays, none
becomes a command, and the status line says what still waits for another
mode), `send-message` (`--to` a pane, `--re` a message,
`--task`; `--in 30m` or `--at 14:30` sends it then: listed under its pane
(`list-messages`), cancelled with `drop-message`, found by its address when
it is due, its sender told when no pane is there; kept over a restart, one
due meanwhile sent at once saying how late; 30 days ahead at most, 100 bytes short of `message-max-size` (room to say it is late), 1000 at
once), `read-message`, `list-messages`, `trace-message` (`-J`: as JSON), `drop-message`,
`move-message`, `pane-ready`, `pane-busy` (the agent began a turn of its
own: no message goes in until its next `pane-ready`), `agent-defs` (the
agent definition files the server has, and those it skipped; `-r` reads
them again), `pane-status`, `list-tasks`, `show-task` (`-J`),
`list-events`, `create-pane`, `mcp` (MCP for an agent in a pane), `setup`
(`setup` alone lists the agents and what each lacks; `setup claude`, `codex`,
`gemini`, `cursor`, `opencode` or `pi` (an extension; pi takes no MCP
server) with `--install` adds the hook that says when
the agent is free, the one that says when it waits for you (Claude Code,
Codex, Gemini CLI: `pane-wait`), and keepane's MCP server; an agent of a
definition file, such as the shipped `codewhale`, or one of yours in
`~/.keepane/agents/*.toml`, the same way: what its file says). Claude
Code's, Codex's and pi's transcripts say it too, about 3 seconds late at
the most, with no hook. Run by a hook, `pane-ready` reads the hook's JSON
on standard input and passes on its session, transcript and model
(`--session`, `--transcript`, `--model`; `--status aborted`, from Cursor's
stop hook, ends the message it worked on unfinished): that transcript is
the pane's
agent's, no guess made. Codex's and Cursor's screens say when they wait for
you, and Cursor's when it works ("ctrl+c to stop"). A message to an
`ai` pane is pasted, and its Enter comes on its own once the paste has
settled. What a pane is up to is `#{pane_state}`: idle, busy, typing (keys
with no Enter), no-agent (an `ai` pane back at its shell), unknown (a shell
whose prompt hook was never heard), exited, or `-`.

The tools `keepane mcp` offers an agent (each one of the commands above,
as the pane it serves): `list_panes`, `read_screen`, `send_message`,
`reply`, `wait_message`, `current_message`, `list_messages`,
`trace_message`, `wait_output`, `drop_message`, `move_message`,
`set_status`, `create_session`, `create_window`, `split_pane`,
`set_work_mode`, `rename_pane`, `kill_pane`, `list_tasks`, `show_task`,
`query_events`, `list_ssh_hosts`; with other machines, `list_links`,
`link_info`, `create_remote_pane`, `push_repo`; the group chat's
`chat_list`, `chat_new`, `chat_join`, `chat_say`, `chat_read`,
`chat_who`, `chat_group_files`.

### Group chat

Channels where panes talk together (docs/design/chat.md): `chat-new`,
`chat-list`, `chat-join`, `chat-leave`, `chat-say`, `chat-read`,
`chat-who`, `chat-topic`, `chat-tag`, `chat-rename`, `chat-delete`,
`chat-dir`, `chat-group-files`, `chat-group-file-rm`, `chat-pick`.
`chat-new NAME
[--topic TEXT] [--tag T]...` makes one and answers its id, a UUID that
never changes; `general` is always there. `chat-list [MACHINE] [--tag T]... [-J]`
lists them: name, the last 8 of the id, members, unread, tags, topic.
`chat-join CHAN [--as HANDLE] [--role ROLE] [--hear all|mentions]`, run
in a pane, makes that pane a member (or the offline member of that handle): its handle and role are its own to give, and who and where
it is (the machine, the pane's place and address, the agent and its
model) the server stamps on it and on each line it says. `chat-say CHAN
TEXT` says something there; a member named by `@handle` is told at once
by a message in its inbox, with what was said before since it last read
(never into a `shell` pane, and on the speaker's chain, so
`message-hop-limit` stops a loop); one that hears all is told of every
line, a few at a time; the others read it when they read; `@you` from a
member tells the person (a notification and `done-webhook`).
`chat-say CHAN --file PATH [--file PATH]... [TEXT]` sends files with the
line (10 at most, each at most `chat-file-max-size`, 25M; a relative path
is from where the command runs): each is copied into the channel's
folder (`chat-dir CHAN` prints it; `chat-list -J` and `chat-read -J` give
it as `dir`, each line's files as `files`, name, size and `file` under
that folder), the line names them with their sizes, a read and the
message to one named say where each copy is. They are the channel's group files, kept until
deleted (not with `chat-days`): `chat-group-files CHAN [-J]` lists them,
the newest first; `chat-group-file-rm CHAN FILE` (its `day/line/name`, or its
name) deletes one, by the pane that sent it or the person. One reading
from another machine, or told there, is given each file's link (a
`share` with no end, listed as for the group file, on the address nearest
that machine; `-J`: `url`), not a path. Files go to this machine's channels only; the
board shows them in the bubble and the web page saves one tapped.
`chat-say CHAN --re N TEXT` replies in line N's thread (a reply to a reply
goes in the thread's first line's; the line reads `↩ #N`): who said line N
and who started the thread are told as when named (the person as with
`@you`), the line it answers quoted first; the board and the web page
quote it above the reply, and the web page has Reply under each line.
`chat-read CHAN --thread N` gives line N's thread (its first line and
every reply, of the last 1000) without moving the read position. `chat-read CHAN [-n N] [--all] [--peek] [--wait SECS]
[-J]` gives what came since the reader last read (50 at most unless `-n`
says, oldest first), `--all` the last N, `--peek` without moving the
read position, `--wait` waits for something new. `chat-who CHAN [-J]`
lists the members, `chat-topic CHAN TEXT`, `chat-tag CHAN +T -T` and
`chat-rename CHAN NEW` change it, `chat-leave CHAN` leaves it (outside
every pane, `chat-leave CHAN HANDLE` takes a member out; either way it is
kept, marked left, until it joins again), and
`chat-delete CHAN` (outside every pane) puts its lines aside. A channel is
named by its name or its id (8 or more of its last characters). Outside
every pane a person says and reads as `you@machine`. A member whose pane
goes, or after a restart, is offline until a pane of the name its pane had
is back or one joins with its handle. A member says 20
lines a minute at most. A channel is one machine's: a pane of a paired
machine names it `MACHINE/CHAN` (`MACHINE` its `host:port`, or its name as
`link list` shows it) in `chat-join`, `chat-leave`, `chat-say`, `chat-read`
and `chat-who`, and `chat-list MACHINE` lists the channels it may look at
there; its server asks that machine over link. A machine given `--full`
may look at and join any channel; another, those shared with it (`link
share <host:port> #CHAN`). Its pane is stamped as its machine stamps it,
with the machine as this one knows it; it is told in its inbox there when
named; it is online while its machine says, every 30 seconds, that the
pane is there. Its person looks and says things as `you@thatmachine`;
changing a channel is its own machine's person's. `chat-list --machines`
lists these channels and then the other machines' (`MACHINE/ID`), and
`--cached` answers with what was last heard of another machine, asking
again behind it (the board's page 2 shows them both). `chat-pick` prints
the channel the person picked last on the board or the web page (its id,
`MACHINE/ID` for another machine's; nothing when none), and `chat-pick CHAN`
keeps that one: every board, and the page on a wide screen, opens on it,
on any device. It is kept in `picked.json` under `chat`; an agent's pane
(`ai`, or made by another pane) may read it but not change it, and a
read-only page changes nothing. `board-pick` prints the page the last
dashboard was left on and the row picked there (`1 %7`: page 1, pane %7;
`3 12`: page 3, share 12; `2`: page 2, whose channel is `chat-pick`'s;
nothing when none), and `board-pick PAGE [ROW]` keeps those: the next
dashboard opens there, on the pane it was asked from when that pane is
gone. The server keeps it until it restarts; an agent's pane may read it
but not change it. Each channel keeps a file a day under `chat` in
the data directory, for `chat-days` days (30; 0 keeps all); its last 1000
lines are in memory. `keepane chat <what> ...` runs `chat-<what> ...`.

`settings` (prefix `g`, a popup; `keepane settings` in any terminal) is
the settings board: every option in nine boxes that stay where they are
(Look, Status line, Keys, Windows and panes, Sessions and history, Alerts,
Agents and messages, Machines, web, Feishu, System); `z` zooms the box in
use to the whole board, with each option's default, who set it and what
it takes, and back. `←`/`→` steps what can be picked (on and off, a
choice, a number, a colour), Enter types the rest on the bottom line (a
secret as dots, the prefix by pressing it), `r` puts the default back;
each change is set at once and written only by `w`, to the
`settings-file`, never to the config file, which runs before it (also
when `source-file` runs the config again). `R` puts every option back to
its default and runs the config and the settings file again (asked
first). `show-settings` (`-J`: as JSON) prints each option's value,
default and who set it last (a config file's line, `theme NAME`, `set`);
`save-settings` *NAME*... writes options' values to the settings file,
each on its own `set -g` line, replacing the one there and keeping every
other line; `reload-settings` is `R`. Under its options the Keys box lists
the prefix table's keys: Enter, then the key it should be on (one bound
already turns both lines red and asks; Enter swaps the two), `r` back
where the config left it. `rebind-key` [`-s`] *FROM* *TO* does that from
a command line (`-s` swaps); `show-bindings` (`-J`: as JSON) prints each
prefix key, what it runs and where the config left it; `save-settings -k`
writes the keys as they differ from where the config left them, as
`unbind-key` and `bind-key` lines in place of the file's own; `R` puts
the prefix table back to its defaults before the files run again. A
format typed in a zoomed box (`status-left`, `pane-border-format`, ...)
shows under it what it comes to, in its colours, at every key:
`show-format` [`-J`] [`-t` *PANE*] *FORMAT* prints that from a command line
(`-J`: each run with its style); a `#(command)` in it is not run.

### Resuming after a reboot

Sessions save themselves to the data directory (`sessions`; a server
started with `-L NAME` to `sessions-NAME`, each its own), and a server
that starts brings them all back (`restore-on-start`, on by default; then
a new or renamed session may not take a saved name that is not running,
which `new -A` resumes instead). `kill-session` puts a session's save aside
(`killed/` there): not back at the next start nor with a bare `keepane`,
`resume` or `resume NAME` brings it back, `list-saved` marks it
killed. A pane whose program Windows ended (DBG_TERMINATE_PROCESS, as a
shutdown does, sometimes before it tells keepane) stays in its window for
10 seconds, so the shutdown's save has it. Before a save whose windows or
panes differ from the last one's, that one is kept as `.json.prev`;
`resume --prev NAME` (`restore-session --prev`) brings it back, the session
not running. `resume`, `list-saved`, `save-session`, `restore-session`,
`delete-saved`, `set-cwd`. Sessions, windows and panes come back with
their numbers; the counter is kept in `next-id` there, so no number is
given out twice.

### Options, keys and scripting

`set-option` (`set`), `set-window-option` (`setw`), `show-options`
(`show`), `show-window-options`, `bind-key` (`bind`), `unbind-key`
(`unbind`), `list-keys`, `source-file`, `set-hook`, `show-hooks`,
`set-environment` (every pane: a new one at its start, a shell already
open at its next prompt; kept across restarts; `-u` takes a name out of
new panes even when the server was started with it), `show-environment`
(attached: in colour, with what keepane sets and the names taken away),
`run-shell`, `if-shell`,
`wait-for`, `wait-output` (block until a pane prints one of some texts,
and print that line; `-a` counts what it already holds, `-i` any case,
`-w` a time limit), `command-prompt`, `confirm-before`, `display-message`,
`display-menu`, `display-popup`, `refresh-client`, `load-plugin`,
`list-plugins`, `list-commands`, `notify` (a desktop notification).

### The program itself

`version` (this keepane's and the server's), `update` (a newer keepane, the
way this one was installed; from Scoop, which skips an app while any of its
programs runs, a copy outside Scoop's folder saves the sessions, stops the
server, runs Scoop and starts the server again with them, and says what it
did in `update.log`), `restart-server` (sessions move to a server of
this version), `import-config` (bring what keepane can use from a tmux
config), `completion` (a Tab completer for PowerShell, bash, zsh or fish), `opencli` (every command, its flags and what each does as OpenCLI JSON; `--install` gives it to UnifyTerm's `ut program add -`, and a server that starts replaces a list there of another version with its own), `man`
(this page),
`web` (the panes on a phone), `link` (panes on other machines), `show-keys`
(what the terminal sends for each key), `startup` (`on`, `off`, `status`:
start the server at logon, every
saved session restored: on Windows a `Run` value, on macOS a launchd agent in
`~/Library/LaunchAgents`, on Linux a systemd user unit enabled for
`default.target`, with the `PATH`, `SHELL` and locale it ran with). On
Windows: `windows-terminal` (a Windows Terminal profile). Both run keepane
by its full path (on macOS and Linux the name the `PATH` finds, such as
Homebrew's `bin/keepane`); when that is an older keepane or one that is
gone (an update into a folder of its own, as Scoop with `no_junction` or
Homebrew's Cellar does), the next keepane to start takes them over and
says so. One run from cargo's target folder, or under test directories,
never does.

### The panes on a phone

`keepane web [--port N] [--bind IP[,IP...]] [--read-only] [--keep-key]` has
the server serve a page for a phone on the local network, in the background,
and prints a QR code carrying its address and a key. Without `--bind` it
listens on the address this machine reaches the network through and on its
Tailscale addresses; with it, on each address given (more than one card:
commas, or `--bind` again), and an address this machine does not have is an
error. The code is for the first; `web status` lists the others. `keepane web status` (the
server command `web-status`) says how it serves and who is connected;
`keepane web stop` (`web-stop`) ends it, cutting off the phones on it. The
server command `web-start` takes the same flags as `-p`, `-b`, `-r`, `-k`
(`-p 0`: any free port). It stops with the server; a `web-start -k` line in the
config has it on whenever the server runs, with the same code each time.
From `web-start` to `web-stop` the server keeps how it is served (the file
`web` in its link directory): when the sessions are restored
(`__server --restore`, which `keepane startup` registers, or
`restore-on-start on`) it is served the same way again, and `web-resume`
(which `restart-server` sends) does the same at once; what cannot be
served yet (an address not up so soon after logon) is tried again every
10 seconds, `web-status` saying why meanwhile. Without `--bind`, this
machine's addresses are looked at again every 10 seconds while it serves:
it is served on those that came and no longer on those gone. `#{web_url}`
is its
address (empty while it is off) and `#{web_clients}` how many are on it; the
default status line shows `web` and that number while it serves, and a phone
connecting, or a wrong key, is said on the status line.

The page's Fit to this screen (in the pane's view menu) sizes the pane
shown to the phone (`web-fit`): `web-fit -t pane -x cols -y rows` zooms
the pane in its window and gives its session the phone's columns and
rows, so a full-screen program draws for the phone. A session has one
size, so meanwhile the computer and any other phone see that session at
the phone's size too (the page and the status line say so). It ends with
`web-fit -t pane -u` (the button again, another pane, the list, the phone
put away), when no phone has shown the pane for 10 seconds, when `keepane
web` stops, or when the session is sized by hand (`resize-window`); the
session gets its size back.

The page's list shown as a grid asks for each pane's last lines with it
(`/api/panes?tail=4`, 1 to 8), which the server gives as `web-tails` (with
N): `{"%3": [...]}`, the last N lines each pane printed, oldest first,
prompts passed over as `#{pane_last_line}` passes them: a keepane command
of the page's, not for people.

The page's paper clip, by the box, sends files into the pane's folder:
`web-upload-dir` (default `keepane-uploads`) under the pane's directory,
or that folder when it is absolute, made when it is not there, with a
`.gitignore` of `*` (a folder there already gets none). Each file is the
body of `POST /api/upload?pane=%N&name=…` with its `Content-Length`,
written as it comes, never over a file there (`report (1).pdf`), its
name's last part only, at most 200 bytes; one past
`web-upload-max` (default 2G) is refused before a byte is read, one cut
short is not kept, a `--read-only` page and a pane copied from another
machine take none. `GET /api/upload-info?pane=%N` says where they would
go and how big one may be. The server is told each with `web-uploaded`
(a keepane command of the page's) and says it on the status line.

`keepane share PATH [--for TIME] [--once]` (the server command `share`)
gives out a file or a folder by a link on the phone page's address and port,
`http://host:port/s/TOKEN`, and prints it with a QR code: one link a
machine address, the code for the first. The path is the command's
(relative to where it ran, `~` home). The token is 128 random bits of its
own, not the page's key; whoever has the link can download that one thing.
`--for` takes `90s`, `30m`, `2h`, `7d` (365d at most), `0` for no end; 24
hours when not given. `--once` ends it after one whole download. A file is
sent as it is, with one range of it on request (`Range`, 206; past its end,
416); a folder as a zip written while it is read (STORED, ZIP64 when
needed, UTF-8 names under `NAME/`), links in it and what cannot be read
left out (a file that stops reading part of the way, cut there). Every
download is `application/octet-stream` with `Content-Disposition:
attachment`, and every answer carries `Content-Security-Policy: sandbox`:
nothing is shown as a page on the page's address. A link that ended, was
stopped or never was answers 404; a download under way goes on. With the
page off, `share` serves it, as it was last served (else as `keepane web`
does). `keepane share` (or `-l`) lists what is shared, `-l -J` every
share as JSON with the ended ones (state `active`, `missing` while its
file is gone, `stopped`, `expired`, `used`; its links, who opened it,
what happened to it); `share stop ID` or `share stop --all` ends them;
`share extend ID [--for TIME]` sets its time from now on the same link,
an expired one coming back; `share again ID` shares the same thing on a
new link, the old one ending. The list is kept in `share/shares.json` in
the sessions directory, written on every change: a server that restores
the sessions at start (`restore-on-start`) brings each share whose time
is not up back on its token, serving the page for them, and says so (and
when the page's port is not the one a link was given on). The last 100
ended shares are kept. Each share, the first download from each address and
each whole download are said on the status line and kept in
`show-messages`. The page asks the server `web-share-open` (a token and the
address asking) for what a link shares, and tells it `web-share-sent` (the
same, and a note) when all of it went: keepane's own commands, not for
people.

### Panes on other machines

`send-message --to host:port/$1:@3.%7` (or `host:port/%name`) reaches a pane
of the keepane server on another machine, over the port `keepane web` serves
on there, once the two servers are paired; the answer comes back the same
way (`send-message -r`). Both machines need `keepane web` running. Pairing
is like SSH keys: each server has a key pair of its own, and a table of the
machines it lets in. `keepane link add <address>` (`link-add`), given the
address `keepane web` prints on the other machine (the one the phone scans,
key included), puts each server's key in the other's table, once; from then
on every request is signed, and the web key plays no part. `keepane link
trust <host:port> <key> [--full [--lan]]` (`link-trust`) lets a machine in
by hand, by the key `keepane link id` (`link-id`) prints there. `keepane
link list` (`link-list`) shows the machines paired, what each has here,
and what this machine has there (asked of it, a few seconds at most);
`keepane link panes <host:port>` (`link-panes`) the panes of it this machine
may reach (address, name, mode, idle or busy, inbox, program, then session name, window index, window active, pane index, pane active, window name); `keepane link info <host:port>` (`link-info`)
the machine: its name, system, keepane version, uptime, CPU, memory, and
the panes of it this machine may reach;
`keepane link capture [-S lines] <host:port/pane>` (`link-capture`) what one
of those panes shows, as text; `keepane link watch <host:port/pane>` that
pane live in the terminal it runs in (Enter on it in `C-b w` opens a window
running it, named after the pane's name there, else its window's, else its
number, and the machine's short name: `raqs@macbook`, which
`#{pane_watch}` gives too), typed into only when that machine gave this
one `--full`, else
read-only (`q` closes it); it asks through `link-view` (the screen, or
"same" since a stamp) and types through `link-keys` (key records).
`keepane link attach <host:port> [session]` attaches to a session there
(the one used last when none is named), for a machine that gave this one
`--full`: that machine makes a client of this terminal's size
(`link-attach`), draws it, and takes every key as its own, the prefix too
(the pane here is put in `passthrough`; F12 gives the keys back);
`link-view -c` and `link-keys -c` name that client. `prefix d` there ends
it; a client not asked for in 15 seconds is let go there and made again.
Every paired machine is followed by itself, and one that gave this one
`--full` answers: one connection to its web port, encrypted (Noise IK
with the two machines' link keys), protobuf inside; it starts from a
snapshot of every pane and takes each change after it, and after a
dropped connection goes on from the last change it had. One that says no
is asked again once a minute; `keepane link mirror <host:port>`
(`link-mirror`) asks again now, `--stop` stops until it is asked again.
Its sessions are worked in from here, named `name@host:port`, but they
are that machine's and are shown under it alone: under its block in the
chart of `C-b w` and `C-b s` (a heading in the tree and the list), by
machine in `C-b v`; `list-sessions` lists this machine's only, and
`switch-client -n` / `-p` go round the sessions of the machine the
current one is of. They are looked at and typed into (the keys go to the
pane there; while the connection is down they are let go and counted),
named in commands as `host:port/name` and `host:port/%N` (its pane N);
`#{pane_machine}` is that `host:port` in them and `#{pane_host}` that
machine's name (asked of it once). Their panes keep their names there
(`#{pane_name}`), but `%name` always names a pane of this machine. A
split, a kill, a resize, a zoom or a rename of one (keys, a border
dragged, a command) is sent to that machine, done there and shown here
when done (keepane 0.63 or later there). A kill by a key (`C-b x`,
`C-b &`, `x` or `D` in `C-b w`) asks first and names the machine it is
done on; a session killed from another machine keeps its save there
(only one killed by hand there is put aside). A split or a new window
puts the eye here on the new pane and leaves that machine's terminals
where they were (0.69 or later there); a zoom zooms the pane asked for.
The pane looked at here is chosen here: what that machine looks at (a
zoom moved there included) changes it only when it is gone. A
pane never moves between machines, and pipe-pane, record and
save-session of one are refused. Looking at it never resizes it there.
They are not saved here, and a third machine does not see them through
this one. `show-mirror` (`[host:port] [-t pane] [-J]`) lists the machines
followed, one's sessions, or a pane's screen as kept here. In the chart
of `C-b w`, on a machine's block, the key line says what it has here
(`has full`, `has 2 shared`, `has nothing`); `c` connects it, `x`
disconnects it, `R` takes back what it has here (`link allow --none`) and
`D` forgets its pairing (`R` and `D` asked first); the dashboard's `L`
lists the machines for the same.
`trace-message` of a message sent there asks
that machine what became of it (queued, delivered, done or failed, a shell
command's output; `-w` waits there). `keepane link remove <host:port>`
(`link-remove`) unpairs, on both machines, and what was shared goes with
it; `keepane link rekey` (`link-rekey`) makes a new key, after which every
pairing has to be made again. `keepane link disconnect <host:port>`
(`link-disconnect`) stops this machine asking a paired one: its sessions
go from here and nothing is asked of it or sent to it, across restarts
too; `keepane link connect <host:port>` (`link-connect`, from outside a
pane) asks it again. It is this machine's side only: the other machine
still connects here and has what it was given (`link allow --none` takes
that back, `link remove` unpairs). `link list` says which machines are
disconnected; `link list -J` gives the paired machines as JSON at once,
none of them asked, each with `connected`, `full` and `shared` (how many
things are shared with it).

A paired machine has one of two levels here, and nothing until one is
given. `keepane link share <host:port> <target>...` (`link-share`) shares
sessions (`work`), windows (`work:logs`), panes (`%worker`) and group chat
channels (`#review`: it may look at and join them) with it: it
sees them (its chart, `link panes`, `link capture`, `link watch`) and may
send them messages, and nothing else of this machine exists for it; a
session or a
window shared covers what comes to be in it later too. A message goes by the
pane's work mode, as anyone's, but a `shell` pane, where a message runs as
a command, takes none from a machine without `--full` (the answer to `link
share` names such panes; in 0.56 and 0.57 sharing was enough). `keepane
link unshare <host:port>
<target>...` (`link-unshare`; `--all` for everything) takes it back. In the
tree (`C-b w`), `s` shares what is under the cursor (or tagged) with the
machine paired, or takes it back; with several machines paired, a menu asks
which; a shared session, window or pane reads `shared` there. What is
shared is marked on the session, window or pane itself, not on its name: a
name used again by something else is not shared; the marks go into the
session's save and come back with `resume`. `keepane link allow <host:port>
--full` (`link-allow`) gives it all of this machine, as if it sat here:
every pane, typed into too (`link watch`), every session, attached to
(`link attach`) and worked in from there (`link mirror`), its messages
run in `shell` panes, it may start panes and send commits and projects
here. Over plain
HTTP anyone on the network could read what goes between the two, so
`--full` is taken only over Tailscale
(100.64.0.0/10, fd7a:115c:a1e0::/48) or from this machine; from any other
address the machine has only what is shared, and is told why. `--full
--lan` takes it from any network. `keepane link allow <host:port> --none`
takes everything back, what was shared too. Sharing and `--full` are given
from a terminal outside keepane or the `C-b :` prompt, never from inside a
pane (`link-trust --full` too); taking back (`link unshare`) is anyone's.
An answer to a message a pane here sent goes back to that pane, shared or
not. The five permissions of keepane 0.55 and before (`--shell`,
`--screen`, `--panes`, `--push`, `--agents`, `--all`) are gone, and from
0.77 a table with them is not read in their words: give such a machine
`--full` or share with it again.
`keepane link start <host:port> [-s session] [-n name] [-m mode]
[-c dir] [-- program args]` (`link-start`) starts a pane
on a machine that gave this one `--full`: one of its `agent-commands` (with
any arguments), within
its `agent-pane-limit` counted for this machine alone, in a session named
after this machine unless `-s` names one; its address there comes back.
`keepane link kill <host:port/pane>` (`link-kill`) closes a pane this
machine made there, and no other. A pane is closed only by the machine it
runs on and the one that made it: each machine records which paired
machine made a pane (`link start`, or a command or key of that machine's
in a session here: a session's copy, a client attached with `link
attach`), saved with the session; another machine's `kill-pane`,
`kill-window`, `kill-session` or `kill-server` that takes a pane it did
not make is refused, naming the machine that made it. (It guards against
mistakes: a machine with `--full` can still type `keepane kill-pane` into
a shell here, which runs as this machine's.) A message for a pane that is not shared with the sender
is answered as one for a pane that is not there, and the sender told how
it could be shared. A message from another machine reads
`from=host:port/…` in its
envelope. A machine is known by its key, not its address: when it turns up
from a new address (another network, Tailscale), its entry follows. The two
machines' clocks must agree to within 2 minutes. When the other machine
cannot be reached the message fails at once; nothing is queued to go later.
`-w` waits on the other machine, and a message still queued there when it
runs out is a time-out here. Pairing and unpairing are said on the status line
and kept in the event log, as are refused requests, once a minute for each
address. `keepane web` listens on this machine's network address and on its
Tailscale addresses, so either network works; `web-status` lists them.
`link-inbound` is how `keepane web` hands a request from another machine to
the server, which holds the keys.

`keepane clone <host:port> [path] [--tell pane]` (`clone`, `link-clone`)
sends the repository the command runs in to a paired machine that gave
this one `--full` (above), as a new repository at `path` there: nothing
may be there yet, or an empty directory. The path is that machine's: `~` is
its home directory, and a relative path, or none, is under its
`link-clone-dir` (default `~/keepane`), with none the name of this
repository's directory. The current branch is checked out there, and its
remote is named after this machine. `keepane push [<host:port> [path]]
[--tell pane]` (`push`, `link-push`) sends the commits of the current branch
that machine lacks to where the repository was cloned or pushed before (this
machine keeps where, by that machine's key, so a new address changes
nothing); with no machine, to the one it went to (named when it went to more
than one); with a path, to a repository already there, which must share a
first commit with this one (or have none yet), kept from then on. There the
commits go to `<this machine>/<branch>`; the branch itself moves on when
that loses nothing of that side's: a fast-forward, its files too when it is
checked out and no tracked file is changed. Otherwise it is left as it was
and the answer says why (commits of its own there, files changed), for that
side to merge. Only committed work goes; files not committed here are
counted in the answer. `--tell pane` sends that pane there a message once
the commits are in; the status line there says so either way, and the event
log keeps it. `push -l` lists where this machine's repositories go. The
commits go as a git bundle, in pieces over the link, checked whole (SHA-256)
before git takes them; `link-push-limit` (default 200M) is the most a bundle
may be, here when it is made and there as it arrives. Both machines need git
(`git-command`). Git LFS contents are not sent (their pointer files are); a
shallow clone is refused (its early history is not there to send: `git fetch
--unshallow` first).

`keepane send-agents [<host:port>] [dir] [-s chat-id]... [-n]`
(`send-agents`, `link-send-agents`; docs/design/agent-sync.md) sends the
project the client is in (its repository, or `dir`) with its agents'
chats to a machine that gave this one `--full` (above). A
repository's commits go as `push` sends them, and with them its files
that git lists as changed or new (ignored ones stay); another folder goes
whole, without `node_modules`, `target` and the like. The chats are those
Claude Code, Codex and pi keep for the folder, their `cwd` set to the
folder there, so `claude --resume`, `codex resume` and `pi --session` go
on with them there. The first time it lands in
`<link-clone-dir>/from/<this machine>/<name>` there; each side keeps where
(`send-agents -l` lists it), so sending again goes to the same folder, a
folder sent back goes back where it came from, and the machine can be
left out. Sent again, only what changed goes: a chat the other side has
the start of goes as its new end. Nothing of the other side's is lost: a
file changed there since the last send is kept, and the one sent put
beside it as `<name>.from-<this machine>`; a chat that went on there too
comes as a new chat. `-s` sends only the chats whose id starts so; `-n`
says what would go and sends nothing. It goes as one gzip stream, in
pieces, checked whole; `link-agents-limit` (default 1G) is the most it may
be, here and there.

### Panes from Feishu

With `feishu-app-id` and `feishu-app-secret` set (a Feishu or Lark
enterprise self-built app with its bot on; docs/design/feishu.md), the
server keeps the app's long connection, a WebSocket it opens itself, and
the tellings of `done-events` also come from the app, to the people let
in (`feishu-notify`). A reply to a pane that waits for you (`wait`) goes
in at once: one key name or character is pressed (`1`, `y`, `esc`,
`enter`, `up`...), other text typed and then Enter; a reply to a question
that is over presses nothing. A reply to any other telling is a message
to that pane, as `send-message` sends one, from `feishu/user`; an agent's
`send-message -r` to it goes back to the chat, under what was sent. In
the chat: `/panes`, `/to <pane> <text>`, `/key <pane> <keys>`, `/screen
<pane>` and `/help`. `keepane feishu` (`feishu`, `feishu-status`) says
whether the app is connected and who is let in. `keepane feishu allow ou_...`
(`feishu-allow`) lets an account in; the bot tells a stranger its open_id
and this command. It is given from a terminal outside keepane or the `C-b
:` prompt, never from inside a pane. `--shell` (`--no-shell`) lets the
account's keys go where no agent runs and its messages to `shell` panes,
which both run commands; `--screen` (`--no-screen`) lets it read what a
pane shows; `--remote` (`--no-remote`) lets it see and reach the panes of
other machines here (those that gave this one `--full`), which are left
out otherwise. `keepane feishu remove ou_...` (`feishu-remove`) lets it in
no more. Only a chat with the bot alone is heard, a message sent more than
10 minutes before it arrives is passed over, and each request is kept in
the event log. Feishu gives each event to one connection of an app: one
app for one machine.

## KEYS

Keys are pressed after the prefix, `C-b` (`set -g prefix C-a` changes it).
`C-b ?` lists every binding. A key not pressed within half a second of the
prefix brings up a panel of what the keys do, by group, read from the
bindings as they are; the key then works as ever (`prefix-hint off` hides
it, `prefix-hint-delay` sets the wait in milliseconds). A key an input
method gives in its full-width form (`【`, `：`, `ｙ`) is read as the ASCII
key wherever keepane reads keys itself; what is typed into a pane or a
text prompt stays as typed. The defaults:

| Key | Does |
|---|---|
| `c` | new window |
| `%` / `"` | split side by side / one above the other |
| `h` `j` `k` `l`, arrows | go to the pane that way (repeatable) |
| `H` `J` `K` `L`, `M-`arrows | resize the pane by 5 (repeatable) |
| `z` | zoom the pane in and out |
| `x` / `&` | kill the pane / the window (asks first) |
| `u` | bring back what was killed a moment ago |
| `0` to `9`, `n`, `p`, `Tab` | window 0 to 9, next, previous, last |
| `s` / `w` | pick a session / a window |
| `d` | detach |
| `[` / `]` | copy mode / paste |
| `/` | what panes printed, by day |
| `v` | the dashboard |
| `g` | the settings board |
| `B` | every pane at a glance (jobs) |
| `S` | send what is typed to every pane of the window |
| `C-t` | each command's time at the end of its line |
| `e` | the environment every pane gets, in colour |
| `Y` | pick a command any pane ran: go there, copy its output, run it again |
| `,` / `$` | rename the window / the session |
| `:` | the command prompt (Up / Down, `C-p` / `C-n`: the last 100 lines run there; `show-prompt-history` lists them, `clear-prompt-history` forgets them) |
| `~` | recent messages |
| `?` | list the keys (output longer than the window opens at its end and scrolls: Up / Down, `j` / `k`, PgUp / PgDn, `g` / `G`, the wheel; `/` `?` search it, `n` `N` go on; any other key closes it) |

`F12`, without the prefix, turns `passthrough` on and off for the pane:
while it is on, every key typed there goes to its program, the prefix and
the root table's keys too, for a keepane running inside the pane (over
ssh). The status line says `passthrough`. Other panes keep the prefix.
`C-b C-b` sends a single `C-b` on without it.

## CONFIGURATION

At start the server reads `~/.keepane.conf` (or
`~/.config/keepane/keepane.conf`, or the file `KEEPANE_CONFIG` names): one
command per line, as tmux's config is written.

    set -g prefix C-a
    set -g mouse on
    bind | split-window -h
    source-file ~/.keepane/themes/plain.conf

A tmux config is not read on its own: `keepane import-config` brings over
what keepane can use from `~/.tmux.conf` (or any config), once, and writes
the rest commented out with why. The default look is Paper Dark, ink on
dark paper with one accent in cinnabar; the themes in the repository's
`themes/` directory (`plain.conf` is tmux's look) are ordinary config files
to source. Eight are built in: `set -g theme paper` switches to the light
one, `paper-dark` back; `tokyo-night` (the default up to 0.37) and its
light `tokyo-day`; `nord`, `gruvbox-dark`, `catppuccin-mocha`, `dracula`.
A theme of your own is a `NAME.conf` in `theme-path` (`~/.keepane/themes`),
lines of `set -g` as the built-in ones are, of the options a theme sets;
what it leaves out is `paper-dark`'s, and `set -g theme NAME` reads the
file each time (a built-in name there is not taken), so `theme-path` is set before it (the settings file is written so). `list-themes` prints
every theme; `pick-theme` lists them and shows each as the cursor (or the
mouse) reaches it, Enter keeps it, Esc puts back the look it opened on.
Each theme names its colours once at its top (`@paper`, `@ink`, `@bar`,
`@soft`, `@muted`, `@line`, `@accent`, `@warn`) and writes `#{@accent}`
below: `#{@name}` in an option's value is put as that `@` option is when
the option is set (one not set stays as written; a secret is kept as typed).
Each draws the panes with `window-style` and `pane-colours`, whatever the
terminal's colours, every text colour at 4.5:1 or more on the panes and
the bar and the lines at 3:1 (a test checks each theme). Under a theme
the panes the keys are not in sit on a background a shade darker
(`pane-dim`, on). The built-in status lines say the work mode only when it
is not `normal`, and on a client narrower than 120 columns leave out CPU,
memory and the battery, under 100 the directory too; the default
`pane-border-format` adds what an `ai` or `shell` pane, or one whose
program has exited, is up to (`pane_state`, in the dashboard's colours). keepane's own colours have
options apart from the 16: `mode-style` (a picked line, the selected
block), `message-style` (a prompt, a message), `popup-border-style` (a
popup's, a menu's, a panel's frame; empty: each its own), in tmux's style
words. The lines between panes are heavy
(`┃` `━`); `set -g pane-border-lines single` draws tmux's thin ones,
`double` or `simple` (ASCII) the others. `set -g pane-frames on` puts
each pane in a rounded frame of its own instead (`╭` `╮` `╰` `╯`), the
active one's in the active border colour, the pane's `pane-border-format`
text on its top (or its bottom with `pane-border-status bottom`); the
frame takes a column and a row on each side, so `resize-pane -x`/`-y`
size the pane with its frame. `show-options -g` lists every option and
its value.

Once a day the server asks GitHub for the latest release's version (one
request with `curl`, nothing about you in it); a newer one shows on the
status line (`#{keepane_update}`) and in `show-messages`.
`set -g update-check off` stops it.

In a git repository the status line and the clock show, after the branch,
what git last said is there: `main ●3 ↑2 ↓1 !1` (files changed, commits
ahead of and behind the upstream, files in conflict; the formats
`git_head`, `git_changed`, `git_ahead`, `git_behind`, `git_conflicts`,
`git_marks`). In an `ai` pane, a message's end and the agent's turn carry
what the work did in the repository since it began: the commits made, the
files changed with their lines, the new files (under `git:` in
`trace-message`, `git` in its JSON and in `list-done -J`,
`KEEPANE_DONE_GIT` for the `pane-done` hook). git runs on a thread of its
own, at most 10 seconds a run, without the index's lock, and without the
file watcher, external diff, text conversion or pager a repository's
config may name; `git-command` names
the git to run (by default the one on the PATH, else where Git for
Windows, GitHub Desktop or Homebrew put it). Without git, the branch and
the commit ids are read from the repository's files and the rest is left out.

`clock-mode` (prefix t) draws the time in big digits and under it the date
(and UTC), other places' time (`clock-mode-zones`: IANA names, commas
between them, 8 at most; a name in any case, or a city alone when one zone
ends with it, as `tokyo`; each shown by its city, its weekday too when its
day is not this one), what waits for you in every session (in yellow:
each agent that waits on you, `work:1.0 claude waits`, while its Trace
has nothing newer than its `pane-wait`; then each window marked `!`, `@`,
`#` or `~` as `bell`, `message`, `activity` or `silence`; no line when
nothing waits), the pane (its program, folder and branch, how long
it has run and been quiet), its last command (how long it took, how it
ended), its agent (model, cost, context), the machine (CPU, memory,
battery, time since boot) and keepane (its sessions and panes, the
messages it has delivered since it started); then, when all of them are
shown and it fits whole, the month (Monday first, the weekend red, today
reversed in the hours' colour; an animation under the clock keeps 10 rows
before it). `clock-mode-info off` leaves the time alone, as tmux draws it;
`clock-mode-style 12` counts the hours to 12; `clock-mode-colour` colours
all the digits. Unset, the hours take the active pane border's colour
and, with a theme that draws the panes (paper, paper-dark, tokyo-day),
the minutes are yellow and the colon faint. While the phone page is
served, the last line ends `p: phone page`: `p` under the clock puts the
page's QR code over it, white on black, for 60 seconds or until another
key (the code carries the page's key); without a page it says how to
start one.

Under that, in the rows left over, something moves with the machine and
with keepane (`clock-mode-animation`). `pulse` is a monitor: a heartbeat at
60 beats a minute with the cores idle, 150 with them all busy, once early for
each message keepane delivers (at most four waiting); a network wave as tall as the traffic is
heavy; a disk wave that breathes faster and deeper the more the disks do; a
system wave that swings faster with the context switches and taller with the
interrupts. Beside the heart: the CPU package's watts, the temperature, a
battery's watts while the machine runs on it (`PWR`), and the messages
delivered while the clock is up (`✉`); beside the others, packets a second,
processes and threads. `matrix` is falling characters: faster with the
cores, more columns with memory, cyan ones with the network, quicker flicker
with the disks, stray magenta characters with the interrupts, and a white
drop headed by `✉` for each message delivered. `bars` is an equalizer: a bar
for each core, memory, the disks read and written, the network in and out,
the temperature, and a bar for each pane of the window by what it prints;
then, where they fit beside those, messages, the CPU's watts, the battery's,
swap, and context switches with interrupts. `fire` is a flame for each
core, as tall as the core is busy, its tip whiter the hotter the CPU, sparks
with the interrupts. `starfield` is a flight through stars, more with the
processes, faster with the network. `sharks` is a sea seen from above: a
school of fish for each pane (six at most), faster the more it prints,
turning more with the context switches; a shark hunting them, hungry
sooner with the cores, longer with memory; all of it drifting with the
network, glints with the disks. `bonsai` is a tree growing through the
day, a limb for each pane (six at most) shaking with its output, its
leaves stirring quicker with the context switches, more leaves with
memory, leaning in the network's wind, a leaf falling now and then, more
with the disks. `sea` is a moonlit sea: the moon crossing the sky with the
hours, stars by night with memory, waves with the cores, the moon's path
glittering with the network, a boat for each pane faster the more it
prints. `cat` is a cat asleep, breathing with the cores, a `z` with each
breath, its tail with the network, an ear with the disks, a ball of yarn
rolling faster the more the panes print. `dino` is a dinosaur running a
desert, faster with the cores, jumping every cactus, more cacti the more
panes, clouds with the network, its score the distance. `penguins` is an
ice shelf, a penguin for each pane waddling on its own stretch, sliding on
its belly while its pane prints a lot, the sea with the cores, snow with
the network, an aurora with memory. `synthwave` is a neon grid running
towards you, faster with the cores, under a striped sun whose stripes
slide with the panes' output, mountains taller with memory, stars with
the network. `fireworks` is a city at night, its windows lit with memory,
rockets going up more often the more the panes print, bursting bigger the
busier the cores. In each of these 10 a message delivered shows as an `✉`
(rising, a comet, in a ripple the shark races to, from a leaf, drifting
ashore, falling beside the cat, which wakes, falling to the dinosaur, on a
fish leaping onto the ice, a meteor, a rocket twice the size). In `pulse`,
`matrix` and `starfield`, and in the shark's eyes and the bonsai's leaves,
the colour goes from green through yellow to red with the CPU's
temperature, or with the cores' load when there is no temperature; the
bars are green at the bottom, yellow higher up and red at the top.
`random` (the default) picks one of the 13 each time the clock is put on
a pane, never the one it had before; `cycle` takes them in turn; a name
sets that one; `off`, or `animation off`, leaves the clock still
(`aquarium`, `life`, `donut`, `weather`, `hourglass`, `orbits` and
`radar` were taken out in 0.46, and from 0.77 are refused as any other
name). Under a clock, `/` turns to
the next kind and `?` to the one before, its name shown for a second;
that holds until the clock goes, the option is not changed (`/` starts one
under `clock-mode-animation off`, none under `animation off`). The clock
stays until `q` (tmux's goes at any key); the prefix still works under it.

`clock-mode-look` draws any of them in a set of characters instead of its
own: `dots`, `blocks` or `binary`, each cell the character with as much
ink as it had (how much of the cell its own character covered, how bright
it was). The colours stay, and so do the figures, the names and each
message's `✉`. `plain`, the default, keeps its own characters.

It is drawn in the 16 colours a theme sets with `pane-colours` (the
terminal's own colours when the theme sets none), so each theme colours it.
The machine is read twice a second, and only while a clock is on screen.
Not every figure can be read everywhere: macOS gives no interrupts and no
power, and counts the context switches and threads of the processes you may
look into, not the whole system's; a figure the system does not give is
left out, never made up. The temperature comes from
a real CPU sensor only: on Linux the thermal zones and hwmon sensors; on
Windows from LibreHardwareMonitor or OpenHardwareMonitor while one runs
(Windows has none of its own: its ACPI thermal zone is a fixed figure on many
machines); not on macOS. The CPU's power is its own meter (RAPL): Windows
reads it for any user where the CPU has one; Linux (`/sys/class/powercap`)
lets only root read it on most systems, so there it shows only when readable.

While a pane runs an agent (Claude Code, Codex, pi), the server reads the
transcript the agent writes and gives its figures to `list-agents`, the
dashboard, the phone page and the formats `agent`, `agent_model`,
`agent_cost`, `agent_tokens`, `agent_context`, `agent_cpu`, `agent_mem`,
`agent_tools`, `agent_turns` and `agent_limit` (Codex's plan limits used,
`34%/12%`; `list-agents -J` gives each window's share, length and reset).
Codex's context share leaves out the 12,000 tokens any session starts
with, as Codex shows it. Cost prices the tokens by LiteLLM's public
list, fetched with `curl` once a day (an hour later after a failed try)
while an agent runs (pi says its own); these are API list prices, not what a subscription is billed. A
cost with `+` after it left out a model the list does not have.
`set -g agent-cost off` stops the fetch and leaves cost out.

The machines `choose-ssh` and `ssh-hosts` list come from four places:
an `ssh` running in a pane (each new one counts a use, with its time;
`#{pane_ssh_host}` says where it goes), the `Host` lines of
`~/.ssh/config` (not patterns), the names in `~/.ssh/known_hosts` (not
hashed ones), and, once, the `ssh` commands in the shells' history files.
`ssh-hosts.json` in the data directory (in an `ssh` folder of the saved
sessions when `sessions-dir` or `KEEPANE_SESSIONS_DIR` moves them) keeps
the used ones: names, users, ports, times, counts, and the names `d` took
off. No password or key is read or kept.

### Every option

The person's options (`agent-pane-limit`, `agent-commands`,
`message-hop-limit`, the `done-webhook` ones, the `feishu-` ones,
`settings-file`) an agent's pane (`ai` work mode, or one another pane
made) may read and not change, by `set` or by a file it sources; it may
not `save-settings` or `reload-settings` either. A secret's value
(`done-webhook-secret`, `feishu-app-secret`) shows as `(set)` and is kept
out of the event log, the log and the `:` prompt's history.
Set with `set -g` (`set -gu` back to the default) or on the settings board (prefix `g`), read with
`show-options -g`. A name may be shortened while it stays one option's.
*(theme)* is the built-in theme's value (`theme`).

| Option | Default | What |
|---|---|---|
| `prefix` | `C-b` | The prefix key. |
| `default-shell` | `pwsh` on Windows, else `$SHELL` (or `/bin/sh`) | What a new pane runs when no program is given. |
| `default-command` | empty | A whole command line that replaces `default-shell` when set. |
| `mouse` | `on` | Clicks select panes and windows, drag resizes, the wheel scrolls. |
| `history-limit` | `5000` | Lines of scrollback a pane keeps. |
| `status` | `on` | The status line. |
| `status-position` | `bottom` | `top` or `bottom`. |
| `status-left` | *(theme)* | The status line's left part (a format). |
| `status-right` | *(theme)* | Its right part (a format). |
| `status-left-length` | `40` | The most columns of the left part. |
| `status-right-length` | `100` | The most columns of the right part. |
| `status-justify` | `left` | Where the window list sits: `left`, `centre`, `right`, `absolute-centre`. |
| `status-style` | *(theme)* | The status line's colours. |
| `status-bg` | *(theme)* | tmux's: `status-style`'s background alone. |
| `status-fg` | *(theme)* | tmux's: `status-style`'s foreground alone. |
| `window-status-separator` | empty | Text between window labels. |
| `status-interval` | `15` | Seconds between runs of `#(command)` pieces. |
| `window-status-format` | *(theme)* | A window's label (a format). |
| `window-status-current-format` | *(theme)* | The current window's label. |
| `pane-border-style` | *(theme)* | The lines between panes. |
| `pane-active-border-style` | *(theme)* | The active pane's lines. |
| `pane-border-lines` | `heavy` | `heavy`, `single`, `double` or `simple` (ASCII). |
| `pane-frames` | `off` | Each pane in a rounded frame of its own. |
| `pane-dim` | `on` | The panes the keys are not in on a background a shade off the theme's. |
| `mode-style` | `fg=yellow,bg=black,bold` | keepane's highlight: a picked line, the selected block (its colours swapped). |
| `message-style` | `fg=brightwhite,bg=black` | A prompt or a message on the status line. |
| `popup-border-style` | *(empty)* | The frame of a popup, a menu or a panel (empty: each its own colour). |
| `display-panes-colour` | `blue` | `display-panes`: the other panes' numbers. |
| `display-panes-active-colour` | `red` | `display-panes`: the active pane's number. |
| `clock-mode-style` | `24` | `clock-mode`: hours as `12` or `24`. |
| `clock-mode-colour` | `default` | The big digits' colour (`default`: the active border's). |
| `clock-mode-info` | `on` | The lines under the clock (date, waits, pane, machine). |
| `clock-mode-zones` | empty | Other places' time under the date (IANA names, 8 at most). |
| `clock-mode-animation` | `random` | What moves under the clock (a name, `random`, `cycle`, `off`). |
| `clock-mode-look` | `plain` | The characters it is drawn in: `plain`, `dots`, `blocks`, `binary`. |
| `base-index` | `0` | The first window's number. |
| `remain-on-exit` | `off` | A pane stays, saying why, when its program exits. |
| `save-history` | `500` | Scrollback lines saved with a session (`all`, or 0 for none). |
| `monitor-activity` | `off` | Mark a background window that prints (`#`). |
| `monitor-bell` | `on` | Mark a window that rings the bell (`!`). |
| `monitor-silence` | `0` | Mark a window quiet this many seconds (`~`); 0 never. |
| `visual-bell` | `off` | Say a bell on the status line instead of ringing. |
| `visual-activity` | `off` | Say activity on the status line too. |
| `notify` | `off` | A desktop notification for each alert and done pane. |
| `update-check` | `on` | Ask GitHub once a day whether a newer keepane is out. |
| `agent-cost` | `on` | Price agents' tokens by LiteLLM's list (fetched once a day). |
| `pane-border-status` | `off` | A line of text on each pane's `top` or `bottom` border. |
| `pane-border-format` | *(theme)* | That line (a format). |
| `pane-base-index` | `0` | The first pane's number. |
| `display-time` | `1500` | Milliseconds a message stays on the status line. |
| `repeat-time` | `500` | Milliseconds a repeatable key may follow without the prefix. |
| `prefix-hint` | `on` | After the prefix, a panel of what the next key does. |
| `prefix-hint-delay` | `500` | Milliseconds before that panel shows (0: at once). |
| `plugin-path` | `~/.keepane/plugins` | Where `@plugin` and `load-plugin` look. |
| `autosave` | `on` | Save the sessions after every change of shape and on exit. |
| `restore-on-start` | `on` | Restore every saved session when the server starts with none. |
| `sessions-dir` | *(data directory)*`/sessions` | Where sessions are saved. |
| `window-size` | `latest` | Whose size a session takes: `latest`, `smallest`, `largest`, `manual`. |
| `default-terminal` | `xterm-256color` | `TERM` in panes on Linux and macOS. |
| `pane-timestamps` | `off` | When each command ran, and how it went, at its line's end. |
| `log-history` | `on` | Keep what panes print on disk (`choose-history`). |
| `log-history-days` | `30` | Days kept; 0 all. |
| `log-history-dir` | *(data directory)*`/history` | Where. |
| `undo-kill-time` | `10` | Seconds a killed pane or window waits for `undo-kill`. |
| `keep-zoom` | `on` | A zoomed window stays zoomed when another of its panes is selected. |
| `animation` | `on` | A frame flies to the pane the keys now go to. |
| `animation-time` | `160` | Its milliseconds (0: off). |
| `message-hop-limit` | `8` | The most hops a chain of messages may take. |
| `message-envelope` | `fields` | A message's header: `fields` or `json`. |
| `message-inbox-limit` | `100` | The most messages one inbox holds. |
| `message-max-size` | `64K` | The most bytes one message holds. |
| `message-wait-max` | `600` | The longest `-w`, in seconds. |
| `agent-pane-limit` | `32` | The most panes a pane (and those it made) may make, and a paired machine may start here (`link start`), each counted alone; 0 to 256. |
| `agent-commands` | `pwsh powershell claude codex` on Windows, else `bash zsh sh claude codex` | What `create-pane` may start. |
| `done-events` | `command agent wait` | What counts as done: `command`, `agent`, `wait`, `task`, `exit`, `bell`. |
| `done-after` | `30` | Seconds a command must run to count. |
| `done-lines` | `5` | Last lines that go with a telling. |
| `done-panes` | `named` | Which panes: `named` (a name, or `ai`/`shell` mode) or `all`. |
| `done-webhook` | empty | An HTTP(S) address told each time. |
| `done-webhook-format` | `json` | Its body: `json`, `text`, `feishu`, `wecom`, `dingtalk`, `slack`, `discord`. |
| `done-webhook-secret` | empty | The secret a Feishu or DingTalk bot checks signatures with. |
| `feishu-app-id` | empty | A Feishu (Lark) app of one's own: its id. |
| `feishu-app-secret` | empty | Its secret (`show-options` prints `(set)`). |
| `feishu-domain` | `feishu` | `feishu`, `lark`, or an address. |
| `feishu-notify` | `on` | Tellings go to the people let in. |
| `event-log` | `on` | What happens to messages and panes, a JSON Lines file a day. |
| `event-log-days` | `30` | Days kept. |
| `event-log-max` | `20M` | The most bytes one day's file holds. |
| `git-command` | empty | The git to run (empty: the one found). |
| `link-clone-dir` | `~/keepane` | Where a repository another machine clones here goes. |
| `link-push-limit` | `200M` | The largest bundle of commits sent or taken. |
| `link-agents-limit` | `1G` | The largest `send-agents` pack. |
| `web-upload-dir` | `keepane-uploads` | Where a file sent from the phone goes (under the pane's directory). |
| `web-upload-max` | `2G` | The largest file the phone page takes. |
| `hint-open` | empty | What opens a path picked in `hints` (empty: VS Code, `$EDITOR`, the desktop). |
| `theme` | `paper-dark` | The theme last set: a built-in one (`paper`, `paper-dark`, `tokyo-night`, `tokyo-day`, `nord`, `gruvbox-dark`, `catppuccin-mocha`, `dracula`) or a `NAME.conf` of `theme-path`. |
| `theme-path` | `~/.keepane/themes` | Where `theme` finds themes of your own: `NAME.conf` files. |
| `window-style` | *(theme)* | The panes' own colours. |
| `pane-colours` | *(theme)* | The 16 colours drawn for a program's first 16. |
| `choose-tree-style` | `chart` | How `choose-tree` shows at first: `chart`, `tree` or `list`. |
| `dashboard-icons` | `emoji` | The dashboard's `[0] Files` marks: `emoji` or `nerd`. |
| `chat-days` | `30` | Days a group chat line is kept (0: all). |
| `chat-file-max-size` | `25M` | The most bytes one file sent with a chat line holds (`chat-say --file`). |
| `settings-file` | empty | Where the settings board writes, run after the config file (empty: `~/.keepane/settings.conf`; none when `KEEPANE_CONFIG` or `-f` names the config). |
| `synchronize-panes` | `off` | Keys typed go to every pane of the window. |
### Every format variable

`#{name}` in a format (the status line, `display-message -p`, `-F`);
flags are `1` or `0`. `#{?name,yes,no}` chooses, `#{=N:name}` cuts,
`#{b:name}` takes the last part of a path, as in tmux.

| Variable | What |
|---|---|
| `session_name` (`S`), `session_id` | The session's name; its id (`$`*N*). |
| `session_windows`, `session_attached` | Its windows; the clients attached to it. |
| `session_created`, `session_activity`, `session_last_attached` | When it was made, last active, last attached (seconds since 1970). |
| `window_name` (`W`), `window_id`, `window_index` (`I`) | The window's name, id (`@`*N*), number. |
| `window_panes`, `window_width`, `window_height`, `window_layout` | Its panes; its size; its layout (tmux's form). |
| `window_flags` (`F`) | Its marks: `*` current, `-` last, `#` activity, `!` bell, `~` silence, `@` message, `Z` zoomed, `S` keys go to all its panes. |
| `window_active`, `window_last_flag`, `window_zoomed_flag` | Flags: current, the last one, zoomed. |
| `window_bell_flag`, `window_activity_flag`, `window_silence_flag` | Flags: rang, printed, went quiet. |
| `window_start_flag`, `window_end_flag`, `window_activity` | First, last in the list; when it last printed. |
| `pane_index` (`P`), `pane_id` (`D`), `pane_title` (`T`), `pane_name` | The pane's number, id (`%`*N*), title, name. |
| `pane_address` | Where it is: `$`*s*`:@`*w*`.%`*p*. |
| `pane_current_command`, `pane_pid_command`, `pane_start_command` | What runs in front; what its process runs; what it was started with. |
| `pane_current_path`, `pane_current_path_short` | Its directory; the same, shortened. |
| `pane_work_mode`, `pane_state`, `pane_idle`, `pane_unheard` | Its work mode; `idle`, `busy`, `typing`, `no-agent`, `unknown`, `exited` or `-`; idle; its agent never said it is free. |
| `pane_inbox`, `pane_message`, `pane_status` | Messages waiting; the one it works on; what it said it does (`pane-status`). |
| `pane_width`, `pane_height`, `pane_top`, `pane_left`, `pane_bottom`, `pane_right` | Its size and where its edges are. |
| `pane_at_top`, `pane_at_bottom`, `pane_at_left`, `pane_at_right` | Flags: at the window's edge. |
| `pane_active`, `pane_last`, `pane_dead`, `pane_in_mode`, `pane_synchronized`, `pane_passthrough` | Flags: active, the last one, its program exited, in copy mode, keys go to all, passthrough allowed. |
| `pane_dead_status`, `pane_dead_time` | How its program exited, and when. |
| `pane_pid`, `pane_start_time`, `pane_activity`, `pane_output_count` | Its process; when it started; when it last printed; how many times it has printed. |
| `pane_mode`, `alternate_on`, `mouse_any_flag`, `cursor_x`, `cursor_y` | Its mode's name; flags: alternate screen, mouse wanted; the cursor. |
| `history_size`, `history_limit` | Scrollback lines kept, and the most. |
| `pane_last_line` | The last line its last command printed. |
| `pane_ssh_host` | The machine its `ssh` is on, when it runs one. |
| `pane_machine`, `pane_host`, `pane_watch` | Another machine's pane: its `host:port`, its name, the watch's label. |
| `agent`, `agent_model`, `agent_cost`, `agent_tokens`, `agent_context` | The agent it runs, its model, cost, tokens, context used. |
| `agent_limit`, `agent_cpu`, `agent_mem`, `agent_tools`, `agent_turns` | Its plan's limits used (Codex), CPU, memory, tools called, replies. |
| `client_name`, `client_session`, `client_width`, `client_height` | The client: its name, its session, its size. |
| `client_created`, `client_activity`, `client_prefix` | When it attached, last typed; flag: the prefix was pressed. |
| `host` (`H`), `host_short` (`h`), `socket_path`, `pid`, `version` | This machine's name, short; the server's socket, process, version. |
| `cpu_percentage`, `ram_percentage`, `ram_used`, `uptime` | This machine's CPU, memory used, how long it has run. |
| `battery_percentage`, `battery_charging` | Its battery, and whether it charges. |
| `local_ip`, `public_ip`, `web_url`, `web_clients` | Its addresses; the phone page's address and how many look at it. |
| `git_branch`, `git_head`, `git_changed`, `git_ahead`, `git_behind`, `git_conflicts`, `git_marks` | The pane's repository: branch, commit, files changed, commits ahead and behind, conflicts, all in one. |
| `keepane_update` | A newer keepane, when one is out. |
## ENVIRONMENT

`KEEPANE`
: The socket name of the server a pane belongs to, set in every pane.

`KEEPANE_PANE`
: The pane's id (the *N* of `%`*N*), set in every pane.

`KEEPANE_CONFIG`
: The config file to read instead of `~/.keepane.conf`.

`KEEPANE_SESSIONS_DIR`
: Where sessions are saved, instead of the data directory's `sessions`.

`KEEPANE_TMPDIR`
: Where the servers' sockets go, instead of `$XDG_RUNTIME_DIR` (or `/tmp`).

`KEEPANE_NO_UPDATE_CHECK`
: Set, the server never asks whether a newer keepane is out.

`KEEPANE_PRICE_LIST`
: A price list (LiteLLM's form) to price the agents' tokens by, instead of
  the one fetched; nothing is fetched.

`KEEPANE_LOG_DIR`
: The data directory, instead of the system's (logs, and where the
  others below go when they are not set).

`KEEPANE_LOG`
: How much `server.log` says: `error`, `warn`, `info` (the default),
  `debug` or `trace`.

`KEEPANE_LOG_MAX`
: Bytes `server.log` grows to before it moves aside (a new one starts).

`KEEPANE_HISTORY_DIR`
: Where what panes print is kept, unless `log-history-dir` says.

`KEEPANE_EVENTS_DIR`
: Where the event log goes.

`KEEPANE_AGENTS_DIR`
: Where your agent definition files are (`*.toml`), instead of
  `~/.keepane/agents`.

`KEEPANE_LINK_DIR`
: Where the link keys, the paired machines and the Feishu accounts are
  kept (a folder a socket under it).

`KEEPANE_NO_TOAST`
: Set, no desktop notification is raised.

`KEEPANE_SCOOP_UPDATE`
: The command `keepane update` runs for a Scoop install, instead of
  `scoop update keepane`.

`KEEPANE_CLAUDE_DIR`
: Where `keepane setup claude` writes, instead of `~/.claude` (or
  `CLAUDE_CONFIG_DIR`).

`CLAUDE_CONFIG_DIR`, `CODEX_HOME`
: Where Claude Code and Codex keep their transcripts, as the agents
  themselves read them (the server's own environment).

`COLORTERM`, `TERM_PROGRAM`
: A terminal that says `TERM_PROGRAM=Apple_Terminal` without
  `COLORTERM=truecolor` gets the nearest of the 256 colours for every
  24-bit one.

## FILES

`~/.keepane.conf`
: The config.

`~/.local/share/keepane/` (Linux), `~/Library/Application Support/keepane/` (macOS), `%LOCALAPPDATA%\keepane\` (Windows)
: The data directory: logs, saved sessions, what panes printed, the event
  log, `prices.json` (the agents' price list, as last fetched), `web.key` (the phone's code kept by `keepane web --keep-key`), and
  `link/`*socket*`/`: a server's key pair (`key`, readable by its user alone)
  and the machines it lets in (`authorized`: a public key, an address, and
  `full`, `lan` and `off`, one machine a line), and the Feishu accounts let
  in (`feishu-users`: an open_id, `shell=`, `screen=` and `remote=`, one
  account a line, readable by its user alone). Beside
  the saved sessions, `environment/`*socket*`.json` keeps
  `set-environment`'s table, and `.ps1`, `.sh` and `.fish` beside it its
  changes for the shells' prompt hook. `shell/` holds the start files that
  give bash, zsh, fish and a `wsl.exe` pane's shell keepane's hook.

`$XDG_RUNTIME_DIR/keepane-`*uid*`/`*socket-name* (or under `/tmp`)
: A server's socket; only its user can open the directory.

## EXAMPLES

Start a session named *work* in the background, split it, and attach:

    keepane new -d -s work
    keepane split-window -h -t work
    keepane attach -t work

Name a pane and send it a command to run:

    keepane rename-pane -t work:0.1 build
    keepane set-work-mode -t %build shell
    keepane send-message --to %build -w 60 "cargo test"

## SEE ALSO

tmux(1). keepane's README and design notes:
<https://github.com/newdee/keepane>
