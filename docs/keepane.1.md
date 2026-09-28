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

With no command, `keepane` starts a new session (`new-session`) and attaches
to it. A command name can be cut to any unambiguous prefix (`keepane att`,
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
`choose-tree`, `choose-client`, `list-clients`.

### Windows and panes

`new-window` (`neww`), `kill-window`, `rename-window`, `select-window`,
`next-window`, `previous-window`, `last-window`, `list-windows`,
`swap-window`, `move-window`, `rotate-window`, `resize-window`,
`respawn-window`, `choose-window`, `find-window`, `select-layout`,
`next-layout`, `previous-layout`, `split-window` (`splitw`), `kill-pane`,
`select-pane`, `last-pane`, `resize-pane`, `swap-pane`, `break-pane`,
`join-pane`, `move-pane`, `respawn-pane`, `list-panes`, `display-panes`,
`capture-pane`, `send-keys`, `send-prefix`, `pipe-pane`, `clear-history`,
`record` (the pane's output as an asciinema file), `focus-pane`,
`undo-kill` (the pane or window killed in the last `undo-kill-time`
seconds comes back).

### Copy mode and buffers

`copy-mode`, `set-buffer`, `list-buffers`, `show-buffer`, `delete-buffer`,
`choose-buffer`, `load-buffer`, `save-buffer`, `paste-buffer`.

### Seeing what happened

`jobs` and `choose-jobs` (every pane: running or exited, for how long),
`find-text` (search what every pane printed), `choose-history` and `view`
(what panes printed, a file a day), `list-marks` (the commands a pane ran),
`show-messages`, `clock-mode`, `dashboard` (panes, inboxes, tasks and the
chosen pane's screen, to act on).

### Panes that pass messages

`rename-pane` (then `-t %name` finds it), `whoami`, `set-work-mode`
(`normal`, `shell` or `ai`), `send-message` (`--to` a pane, `--re` a message,
`--task`), `read-message`, `list-messages`, `trace-message`, `drop-message`,
`move-message`, `pane-ready`, `pane-status`, `list-tasks`, `show-task`,
`list-events`, `create-pane`, `mcp` (MCP for an agent in a pane), `setup`
(`setup claude` connects Claude Code).

### Resuming after a reboot

Sessions save themselves to the data directory. `resume`, `list-saved`,
`save-session`, `restore-session`, `delete-saved`, `set-cwd`.

### Options, keys and scripting

`set-option` (`set`), `set-window-option` (`setw`), `show-options`
(`show`), `show-window-options`, `bind-key` (`bind`), `unbind-key`
(`unbind`), `list-keys`, `source-file`, `set-hook`, `show-hooks`,
`set-environment`, `show-environment`, `run-shell`, `if-shell`,
`wait-for`, `command-prompt`, `confirm-before`, `display-message`,
`display-menu`, `display-popup`, `refresh-client`, `load-plugin`,
`list-plugins`, `list-commands`, `notify` (a desktop notification).

### The program itself

`version` (this keepane's and the server's), `update` (a newer keepane, the
way this one was installed), `restart-server` (sessions move to a server of
this version), `import-config` (bring what keepane can use from a tmux
config), `completion` (a Tab completer for PowerShell, bash, zsh or fish), `man`
(this page),
`web` (the panes on a phone), `link` (panes on other machines), `show-keys`
(what the terminal sends for each key), `migrate` (from wmux, keepane's old
name). On Windows: `startup`
(start the server at logon) and `windows-terminal` (a Windows Terminal
profile).

### The panes on a phone

`keepane web [--port N] [--bind IP] [--read-only] [--keep-key]` has the
server serve a page for a phone on the local network, in the background, and
prints a QR code carrying its address and a key. `keepane web status` (the
server command `web-status`) says how it serves and who is connected;
`keepane web stop` (`web-stop`) ends it, cutting off the phones on it. The
server command `web-start` takes the same flags as `-p`, `-b`, `-r`, `-k`
(`-p 0`: any free port). It stops with the server; a `web-start -k` line in the
config has it on whenever the server runs, with the same code each time. `#{web_url}` is its
address (empty while it is off) and `#{web_clients}` how many are on it; the
default status line shows `web` and that number while it serves, and a phone
connecting, or a wrong key, is said on the status line.

The page's ⤢ button sizes the pane shown to the phone (`web-fit`): `web-fit -t pane -x
cols -y rows` zooms the pane in its window and gives its session the phone's
columns and rows, so a full-screen program draws for the phone. A session has
one size, so meanwhile the computer and any other phone see that session at
the phone's size too (the page and the status line say so). It ends with
`web-fit -t pane -u` (the button again, another pane, the list, the phone
put away), when no phone has shown the pane for 10 seconds, when `keepane
web` stops, or when the session is sized by hand (`resize-window`); the
session gets its size back.

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
(`link-panes`) their panes; `keepane link info <host:port>` (`link-info`)
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
panes here show: `--screen` (`--no-screen`). Both are given from a terminal
outside keepane or the `C-b :` prompt, never from inside a pane
(`link-trust` takes `--shell` and `--screen` too). A message for a
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

## KEYS

Keys are pressed after the prefix, `C-b` (`set -g prefix C-a` changes it).
`C-b ?` lists every binding. The defaults:

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
| `,` / `$` | rename the window / the session |
| `:` | the command prompt |
| `~` | recent messages |
| `?` | list the keys |

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
the rest commented out with why. The default look is Tokyo Night; the themes
in the repository's `themes/` directory (`plain.conf` is tmux's look) are
ordinary config files to source. `show-options -g` lists every option and
its value.

Once a day the server asks GitHub for the latest release's version (one
request with `curl`, nothing about you in it); a newer one shows on the
status line (`#{keepane_update}`) and in `show-messages`.
`set -g update-check off` stops it.

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

`COLORTERM`, `TERM_PROGRAM`
: A terminal that says `TERM_PROGRAM=Apple_Terminal` without
  `COLORTERM=truecolor` gets the nearest of the 256 colours for every
  24-bit one.

## FILES

`~/.keepane.conf`
: The config.

`~/.local/share/keepane/` (Linux), `~/Library/Application Support/keepane/` (macOS), `%LOCALAPPDATA%\keepane\` (Windows)
: The data directory: logs, saved sessions, what panes printed, the event
  log.

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
