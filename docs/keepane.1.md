# KEEPANE 1

## NAME

keepane - a terminal multiplexer whose panes keep running and pass messages to each other

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
and the one saved last; with nothing at all, a new session. `keepane new`
always starts a new one. A command name can be cut to any unambiguous prefix (`keepane att`,
`keepane splitw -h`). Run inside a pane, a command goes to the server that
pane belongs to, as tmux's do with `$TMUX`.

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
`choose-tree` (sessions, windows and panes, and the paired machines' in the chart; a chart of rows by default, a tree of blocks with `choose-tree-style tree`, the plain list with `choose-tree-style list`; `v` goes to the next), `choose-client`, `list-clients`.

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
`choose-marks` (the commands every pane's shell ran, newest first: Enter
goes to one in copy mode, `y` copies what it printed, `r` runs it again),
`shell-history` (a pane's own shell history, the messages delivered to it
marked; `-c` the commands only, `-m` every message sent to it),
`list-done` (when panes were done: `done-events`, told to the phone, `notify`,
the `pane-done` hook and `done-webhook`),
`show-messages`, `clock-mode`, `dashboard` (panes, inboxes, tasks and the
chosen pane's screen, to act on), `list-agents` (the Claude Code, Codex and
pi the panes run: model, cost, tokens, context, CPU, memory; `-J` as JSON),
`trace-pane` (what a pane's agent did, step by step: its latest turns, each
call of the model and each tool, from when to when, failed or not; a tree,
or `-J` JSON; the dashboard's Trace tab draws it as a waterfall, each kind
in its colour, `e` opening every turn),
`status-line` (a session's status line as drawn; `-J` its parts, their colours
and each window's pane, which the phone page draws under the screen).

### Panes that pass messages

`rename-pane` (then `-t %name` finds it), `whoami`, `set-work-mode`
(`normal`, `shell` or `ai`), `send-message` (`--to` a pane, `--re` a message,
`--task`), `read-message`, `list-messages`, `trace-message` (`-J`: as JSON), `drop-message`,
`move-message`, `pane-ready`, `pane-status`, `list-tasks`, `show-task` (`-J`),
`list-events`, `create-pane`, `mcp` (MCP for an agent in a pane), `setup`
(`setup` alone lists the agents and what each lacks; `setup claude`, `codex`,
`gemini`, `cursor` or `opencode` with `--install` adds the hook that says when
the agent is free, and keepane's MCP server). Claude Code's, Codex's and pi's
transcripts say it too, about 3 seconds late at the most, with no hook. A message to an
`ai` pane is pasted, and its Enter comes on its own once the paste has
settled. What a pane is up to is `#{pane_state}`: idle, busy, typing (keys
with no Enter), no-agent (an `ai` pane back at its shell), unknown (a shell
whose prompt hook was never heard), exited, or `-`.

### Resuming after a reboot

Sessions save themselves to the data directory. `resume`, `list-saved`,
`save-session`, `restore-session`, `delete-saved`, `set-cwd`.

### Options, keys and scripting

`set-option` (`set`), `set-window-option` (`setw`), `show-options`
(`show`), `show-window-options`, `bind-key` (`bind`), `unbind-key`
(`unbind`), `list-keys`, `source-file`, `set-hook`, `show-hooks`,
`set-environment` (every pane: a new one at its start, a shell already
open at its next prompt; kept across restarts), `show-environment`
(attached: in colour, with what keepane sets), `run-shell`, `if-shell`,
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
config), `completion` (a Tab completer for PowerShell, bash, zsh or fish), `man`
(this page),
`web` (the panes on a phone), `link` (panes on other machines), `show-keys`
(what the terminal sends for each key), `migrate` (from wmux, keepane's old
name), `startup` (`on`, `off`, `status`: start the server at logon, every
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
does), for as long as the server runs (not kept for a restore). `keepane
share` (or `-l`) lists what is shared; `share stop ID` or `share stop
--all` ends them. Each share, the first download from each address and
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
trust <host:port> <key> [--shell]` (`link-trust`) lets a machine in by hand,
by the key `keepane link id` (`link-id`) prints there. `keepane link list`
(`link-list`) shows the machines paired; `keepane link panes <host:port>`
(`link-panes`) their panes (address, name, mode, idle or busy, inbox, program, then session name, window index, window active, pane index, pane active, window name); `keepane link info <host:port>` (`link-info`)
the machine: its name, system, keepane version, uptime, CPU, memory, panes;
`keepane link capture [-S lines] <host:port/pane>` (`link-capture`) what one
of its panes shows, as text, which that machine must allow (`--screen`,
below); `trace-message` of a message sent there asks that machine what became
of it (queued, delivered, done or failed, a shell command's output; `-w`
waits there). `keepane link remove <host:port>` (`link-remove`) unpairs;
`keepane link rekey` (`link-rekey`) makes a new key, after which every
pairing has to be made again.

A paired machine's messages go to `ai` and `normal` panes only. To let them
run as commands in `shell` panes here: `keepane link allow <host:port>
--shell` (`link-allow`; `--no-shell` takes it back); to let it read what the
panes here show: `--screen` (`--no-screen`); to let it start panes here:
`--panes` (`--no-panes`); to let it send commits into repositories here:
`--push` (`--no-push`). All are given from a terminal outside keepane or
the `C-b :` prompt, never from inside a pane (`link-trust` takes `--shell`,
`--screen`, `--panes` and `--push` too). `keepane link start <host:port> [-s session]
[-n name] [-m mode] [-c dir] [-- program args]` (`link-start`) starts a pane
on a machine that allowed this one to: one of its `agent-commands` (with any
arguments, so a shell on that list makes `--panes` as much as `--shell`), within
its `agent-pane-limit` counted for this machine alone, in a session named
after this machine unless `-s` names one; its address there comes back.
`keepane link kill <host:port/pane>` (`link-kill`) closes a pane this
machine started there, and no other (that machine knows which only while
its server runs). A message for a
`shell` pane from a machine not allowed is refused, and the sender told how
to allow it. A message from another machine reads `from=host:port/…` in its
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
sends the repository the command runs in to a paired machine that allowed
this one to (`--push`, above), as a new repository at `path` there: nothing
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
to source. Four are built in: `set -g theme paper` switches to the light
one, `paper-dark` back; `tokyo-night` (the default up to 0.37, the panes
in the terminal's own colours) and its light `tokyo-day`. All but
`tokyo-night` draw the panes with `window-style` and `pane-colours`,
whatever the terminal's colours. The lines between panes are heavy
(`┃` `━`); `set -g pane-border-lines single` draws tmux's thin ones,
`double` or `simple` (ASCII) the others. `show-options -g` lists every
option and its value.

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
day is not this one), the pane (its program, folder and branch, how long
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
the minutes are yellow and the colon faint.

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
`radar`, taken out in 0.46, give `random`). Under a clock, `/` turns to
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
`agent_tools` and `agent_turns`. Cost prices the tokens by LiteLLM's public
list, fetched with `curl` once a day (an hour later after a failed try)
while an agent runs (pi says its own); these are API list prices, not what a subscription is billed. A
cost with `+` after it left out a model the list does not have.
`set -g agent-cost off` stops the fetch and leaves cost out.

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
  `shell` and `screen` for what they may do, one machine a line). Beside
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
