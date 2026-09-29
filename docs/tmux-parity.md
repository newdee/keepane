# tmux parity checklist

What tmux has, what keepane has, and what is deliberately left out. Kept next to
the code so "is that in yet?" has one answer.

Source of truth: the tmux manual (`tmux.1`) and the tables in `cmd.c`,
`key-bindings.c` and `options-table.c` from tmux master, read on 2026-09-21.
Anything marked *master only* (floating panes, `new-pane`, `move-pane -P`,
`switch-mode`) is newer than tmux 3.5 and not a goal.

Status: **yes** = works, **part** = works with a documented limit,
**todo** = planned, **no** = deliberately not doing it (reason given).

## Commands

| tmux | keepane | note |
| --- | --- | --- |
| attach-session | yes | |
| bind-key | yes | `-n`, `-r`, `-T root/prefix/copy-mode-vi` (`copy-mode` = `copy-mode-vi`); mouse key names accepted, no effect |
| break-pane | yes | `-t`; no `-W` |
| capture-pane | part | `-p`, `-S`, `-e`, `-J`; no buffer output (`-b`) |
| choose-buffer | yes | `prefix =`; Enter pastes |
| choose-client | yes | `prefix D`; Enter detaches the client picked |
| choose-tree | part | `-s`, `-w`; `f` filters by a substring (not a format), `t`/`T` tag, `x` kills the tagged, `-`/`+` (Left/Right) fold and unfold a session |
| clear-history | yes | |
| clear-prompt-history | no | keepane keeps no prompt history |
| clock-mode | yes | `prefix t`, any key leaves |
| command-prompt | part | `-p`, `-I`, `%%` template, Tab completes the command name, its flags, a `-t`/`-s` target, and after `set`/`show` the option name (abbreviations included) and a few-valued option's value; no `-k`, no numbered `%1` |
| confirm-before | yes | `-p` |
| copy-mode | yes | `-u`, `-t` |
| customize-mode | no | a whole options UI; `show-options` covers the need |
| delete-buffer | yes | `prefix -` |
| detach-client | yes | `-a`, `-s`, `-t` |
| display-menu | part | `-T` title, name/key/command triples, `""` separators; always drawn over the window (no `-x`/`-y`) |
| display-message | yes | `-p`, `-t`, formats |
| display-panes | yes | `prefix q`, digit selects |
| display-popup | yes | `-E`, `-C`, `-w`, `-h`, `-x`, `-y` (a column/row, `N%`, `C`, `R`/`B`), `-d`; the prefix key stays keepane's, `prefix prefix` sends it to the program in the box |
| find-window | yes | `prefix f`; one hit jumps, several open the picker |
| has-session | yes | |
| if-shell | yes | `-F` formats and shell exit status; `-b` runs inline |
| join-pane / move-pane | yes | `-h`, `-v`, `-b`, `-s`, `-t`, and the marked pane |
| kill-pane / kill-server / kill-session / kill-window | yes | `-a`, `-t` |
| last-pane / last-window | yes | |
| link-window / unlink-window | no | a window belongs to one session here; `move-window` covers the common use |
| list-buffers | yes | `prefix #` |
| list-clients | yes | |
| list-commands | yes | |
| list-keys | part | no `-N` notes, no `-T` filter |
| list-panes | yes | `-s` (the session's windows, `window.` prefixed), `-a` (every pane, `session:window.` prefixed), `-F` format per pane |
| list-sessions / list-windows | yes | |
| load-buffer / save-buffer | yes | |
| lock-client / lock-server / lock-session | no | no equivalent of a Unix screen lock here |
| move-window | yes | inside a session and across sessions; a free index is taken as given |
| new-session | yes | `-s`, `-n`, `-c`, `-d`, `-A`, `-x`, `-y` |
| new-window | yes | `-n`, `-c`, `-d`, `-t` |
| next-layout / previous-layout | yes | also `select-layout -n` / `-p`, `prefix Space` |
| next-window / previous-window | yes | `-t`, `-a` (next window with an alert) |
| paste-buffer | yes | `-b`, `-p`, `-t`; no `-s` separator |
| pipe-pane | yes | `-o`, `-t`, `-O` (the pane's output to the command, the default), `-I` (the command's output typed into the pane), both together |
| refresh-client | part | redraws; `-U`/`-D`/`-L`/`-R [n]` pan this client's view of a window bigger than it (the view follows the cursor again at the next key); no `-C` size |
| rename-session / rename-window | yes | |
| resize-pane | yes | `-L/-R/-U/-D`, `-x`, `-y`, `-Z`, `-t` |
| resize-window | yes | `-x`, `-y`, `-U`/`-D`/`-L`/`-R n`, `-A` (largest client), `-a` (smallest); sizes the whole session (its windows are one size), which `window-size manual` then keeps |
| respawn-pane / respawn-window | yes | `-k`, `-t`, a command to run |
| rotate-window | yes | `prefix C-o`, `M-o` |
| run-shell | yes | `-b`, `-t` |
| select-layout | yes | the five named layouts, `-E`, `-n`/`-p`, and tmux layout strings (what `#{window_layout}` shows; the checksum is checked when present) |
| select-pane | yes | direction, `next`/`last`/index, `-t session:window.pane`, `-T`, `-m`, `-M` |
| select-window | yes | index, name, `+`, `-`, `!` |
| send-keys | part | key names, `-l`, `-X` copy commands; no `-H` (hex) |
| send-prefix | yes | |
| server-access | no | single user by design; the pipe is owner-only |
| set-buffer / show-buffer | yes | `-a`, `-b` |
| set-environment / show-environment | yes | `-r` to remove |
| set-hook / show-hooks | part | nine hooks |
| set-option / show-options | part | the options keepane implements, colours and styles included, each printed in a form `set` reads back; unknown ones accepted and ignored |
| set-window-option / show-window-options | part | folded into `set-option`; `synchronize-panes` is per window and takes `-t` |
| show-messages | yes | `prefix ~`, the last 100 |
| source-file | yes | |
| split-window | yes | `-h`, `-v`, `-c`, `-d`, `-b`, `-f`, `-t`; keepane's own `-N count` makes that many panes and tiles the window |
| start-server | part | any client starts it |
| suspend-client | no | no SIGTSTP on Windows |
| swap-pane | yes | `-U` / `-D`, and the `-s src -t dst` pair (across windows and sessions too) |
| swap-window | yes | inside one session or across two; each session keeps its current index |
| switch-client | yes | `-n`, `-p`, `-l`, `-t` |
| unbind-key | yes | |
| wait-for | yes | `-L`, `-U`, `-S`; a waiting client is answered when the channel is signalled |
| start-server | yes | accepted; any command starts the server |
| **keepane only** | | `find-text` (search what every pane printed), `choose-history` (what panes printed, kept a file per pane position per day: pick one, read it in `view`), `view FILE` (a less-like pager), `list-marks` (the commands a pane's shell reported, with their times), `undo-kill` (bring back the pane or window killed in the last `undo-kill-time` seconds, programs still running), `jobs` (every pane: running or exited, up for how long, idle since when), `choose-jobs` (that board as a picker: go there, kill, restart), `record` (a pane's output as an asciinema file), `notify` (a desktop toast; an alert's has a Go-to-pane button), `focus-pane` (every attached client goes to a pane), `startup on/off/status` (start at logon and restore, via the user's Run key), `windows-terminal install/remove/status` (a keepane profile in the Windows Terminal dropdown, as a fragment file), `completion powershell` (a PowerShell completer to load from `$PROFILE`), `migrate` (from wmux, keepane's name up to 0.13.1: sessions, saved data, logon start, Windows Terminal profile), `restart-server` (move the running sessions to a server of this version; `kill-server -r` tells attached clients to attach again), `update [--check]` (install a newer release the way this one was installed), `version` (this program's and the server's), `show-keys` (each key as the console hands it over and as keepane reads it, for a key that does nothing), `web` (the panes on a phone: a QR code, a page that lists, shows and types into panes over the local network), `resume`, `save-session`, `restore-session`, `list-saved`, `delete-saved`, `set-cwd`, `load-plugin`, `list-plugins`, `version` (tmux has `-V`), and `choose-window` / `choose-session` as names for `choose-tree -w` / `-s`. Panes that talk (docs/design/mailbox.md): `rename-pane` (a name `-t %name` finds; a full address `$1:@3.%7` is refused once the pane has moved), `whoami`, `set-work-mode normal/shell/ai` (from a pane, only that pane), `send-message`, `read-message`, `list-messages`, `trace-message`, `drop-message`, `move-message`, `pane-ready`, `pane-status`, `create-pane` (what an agent uses: `agent-commands`, `agent-pane-limit`), `list-tasks`, `show-task`, `list-events` (the event log), `dashboard` (every pane at a glance), `mcp` (an MCP server for an agent in a pane) and `setup claude [--install]` |

## Default prefix keys

tmux's table, with what keepane does today.

| Key | tmux | keepane |
| --- | --- | --- |
| `C-b` | send-prefix | yes |
| `C-o` / `M-o` | rotate-window | yes |
| `C-z` | suspend-client | no (see above) |
| `Space` | next-layout | yes |
| `!` | break-pane | yes |
| `"` / `%` | split-window | yes |
| `#` | list-buffers | yes |
| `$` | rename-session prompt | yes |
| `&` | kill-window (confirm) | yes |
| `'` | prompt for window index | yes |
| `(` / `)` | switch-client -p / -n | yes |
| `,` | rename-window prompt | yes |
| `-` | delete-buffer | yes |
| `.` | move-window prompt | yes |
| `/` | describe key | keepane: choose-history (`?` lists the keys) |
| `0`-`9` | select-window | yes |
| `:` | command-prompt | yes |
| `;` | last-pane | yes |
| `=` | choose-buffer | yes |
| `?` | list-keys | yes |
| `D` | choose-client | yes |
| `B` | choose-jobs | keepane only: the task board as a picker (Enter goes there, `x` kills, `r` restarts) |
| `E` | select-layout -E (spread) | yes |
| `L` | switch-client -l | keepane uses `L` to resize; `:switch-client -l` works |
| `M` / `m` | select-pane -M / -m | yes |
| `T` | pane title prompt | yes |
| `[` / `PPage` | copy-mode | yes |
| `]` | paste-buffer | yes |
| `c` / `d` | new-window / detach | yes |
| `f` | find-window | yes |
| `i` | display-message | yes |
| `l` | last-window | `l` is select-pane -R in keepane; `Tab` is last-window |
| `n` / `p` | next / previous window | yes |
| `o` | select-pane -t :.+ | yes |
| `q` | display-panes | yes |
| `r` | refresh-client | yes |
| `s` / `w` | choose-tree -Zs / -Zw | yes |
| `t` | clock-mode | yes |
| `x` | kill-pane (confirm) | yes |
| `z` | resize-pane -Z | yes; while zoomed, selecting another pane of the window moves the zoom to it (tmux unzooms; `keep-zoom off` does too), and `q` numbers every pane |
| `{` / `}` | swap-pane -U / -D | yes |
| `~` | show-messages | yes |
| arrows | select-pane (repeat) | yes |
| `M-1`..`M-5` | select-layout presets | yes |
| `M-6` / `M-7` | mirrored layouts | no (master only) |
| `M-n` / `M-p` | next/previous window with alert | yes |
| `C-`/`M-` arrows | resize-pane | yes |
| `S-`arrows | refresh-client -U/-D/-L/-R | yes (5 rows / 10 columns a press, repeatable) |
| `<` / `>` | display-menu | yes (window menu / pane menu) |

keepane adds `h` `j` `k` `l` (move), `H` `J` `K` `L` (resize), `C-s` / `C-r`
(save / restore), `S` (synchronize-panes), `u` (undo-kill), `C-t` (pane-timestamps) and `v` (dashboard) on top of that table.

## Copy mode

Movement: arrows and `h j k l`, `w` `b` `e` and their `W` `B` `E` forms,
`0` `^` `$`, `H` `M` `L`, `{` `}`, `g` `G`, `C-b` `C-f` `C-u` `C-d`, PgUp and
PgDn. A count works in front of a motion (`3j`, `2w`).

Selecting and copying: `Space` or `v` starts a selection, `C-v` makes it a
rectangle, `Enter` or `y` copies to both a new paste buffer and the Windows
clipboard, `q` or Escape leaves.

Searching: `/` forward (towards the newest line), `?` back through the scrollback, `n` and `N` to repeat, case-insensitive.

`send-keys -X <command>` drives all of it from a script or a binding:
`cursor-up/-down/-left/-right`, `next-word`, `next-word-end`,
`previous-word` (and the `-space` forms), `start-of-line`,
`back-to-indentation`, `end-of-line`, `top-line`, `middle-line`,
`bottom-line`, `previous-paragraph`, `next-paragraph`, `history-top`,
`history-bottom`, `page-up`, `page-down`, `halfpage-up`, `halfpage-down`,
`begin-selection`, `rectangle-toggle`, `copy-selection`, `search-forward`,
`search-backward`, `search-again`, `search-reverse` and `cancel`.

Missing: `copy-pipe` to a command (`copy-pipe-and-cancel` copies as
`copy-selection` does), and the jump commands (`jump-to-forward`, `f`/`t`).

## Config file

`~/.keepane.conf`, read as tmux reads its own: `\` continues a line,
`%if` / `%elif` / `%else` / `%endif` pick their branch by the condition (a
format, true when it expands to something other than nothing or `0`;
`#{==:#{host},box}` and the other comparisons work). `~/.tmux.conf` is not
read: `keepane import-config` brings what keepane can use from it (or from
any config) into keepane's, once, with the rest commented out and why
(TPM and `@` plugin settings, options keepane has no use for, `run` and
`if-shell`, anything it rejects). A tmux config read with `source-file`
still skips what keepane cannot use, with a note in `show-messages` plus a
one-line count on the first attach. `bind -T` takes
`root`, `prefix` and `copy-mode-vi` (`copy-mode` is taken as the same
table, keepane's copy mode being vi-style); any other table is refused rather
than bound somewhere else. Mouse key names (`MouseDragEnd1Pane`,
`WheelUpPane`, ...) are accepted and do nothing.

## Options

`show-options` prints what keepane implements; anything else common in a
`.tmux.conf` is accepted and ignored if set anyway, and left out by
`import-config`.
The ones with tmux meaning: `prefix`, `default-shell`, `default-command`,
`mouse`, `history-limit`, `status`, `status-position`, `status-style`,
`status-left`, `status-right`, `status-left-length`,
`status-right-length`, `status-justify`, `window-status-separator`, `status-interval`, `window-status-format`,
`window-status-current-format`, `pane-border-style`,
`pane-active-border-style`, `display-panes-colour`,
`display-panes-active-colour`, `base-index`, `pane-base-index`,
`display-time`, `repeat-time`, `remain-on-exit`, `monitor-activity`,
`monitor-bell`, `monitor-silence`, `visual-bell`, `visual-activity`,
`pane-border-status`, `pane-border-format`, `window-size` (`latest`,
`smallest`, `largest`, `manual`: which attached client sizes the session).

## Formats

`#{...}` names: the session (`session_name` `session_id` `session_windows`
`session_attached` `session_created`), the window (`window_name`
`window_id` `window_index` `window_panes` `window_active`
`window_last_flag` `window_zoomed_flag` `window_width` `window_height`
`window_bell_flag` `window_activity_flag` `window_silence_flag`
`window_flags`), the pane (`pane_index` `pane_id` `pane_title`
`pane_current_command` `pane_start_command` `pane_current_path`
`pane_width` `pane_height` `pane_active` `pane_dead` `pane_dead_status`
`pane_synchronized` `pane_in_mode` `pane_pid` `pane_start_time` `pane_activity` `pane_dead_time`
`pane_last` `pane_mode` `pane_top` `pane_left` `pane_bottom` `pane_right` `pane_at_top` `pane_at_bottom`
`pane_at_left` `pane_at_right` `cursor_x` `cursor_y` `history_size` `history_limit`), the session and window
times and flags (`session_activity` `session_last_attached` `window_activity` `window_start_flag`
`window_end_flag` `window_layout`), the client
(`client_width` `client_height` `client_name` `client_session` `client_created` `client_activity`
`client_prefix`) and the server (`host` `host_short`
`socket_path` `version` `pid`), plus the one-letter forms `#S #W #I #P #T
#H #F #D #h`. keepane's own, read in-process rather than through `#()`:
`cpu_percentage` `ram_percentage` `ram_used` `battery_percentage`
`battery_charging` `uptime` `git_branch` `pane_current_path_short`
`pane_pid_command` (tmux users get these from plugins such as tmux-cpu
and tmux-battery), and `pane_output_count` (how many times the pane has
printed, which `keepane web` watches), and for pane messages `pane_name` `pane_address` `pane_work_mode`
`pane_idle` `pane_unheard` `pane_inbox` `pane_status` `pane_message`. Modifiers: `=N:` `=-N:` `b:` `d:` `t:`
`s/a/b/:`, nestable. Conditionals: `#{?name,yes,no}`, `#{?name==value,…}`,
`#{?name!=value,…}`; comparisons `#{==:a,b}` `#{!=:a,b}` `#{<:a,b}`
`#{>:a,b}` `#{<=:a,b}` `#{>=:a,b}` `#{&&:a,b}` `#{||:a,b}` and
`#{m:pattern,text}` (`m/i:` ignores case).
Not there: `#{e|…}` arithmetic, `#{l:…}` literals, `#{a:…}`, `#{C:…}`
search, loops (`#{S:…}` `#{W:…}` `#{P:…}`), and the mouse variables.
`default-terminal` sets `TERM` in panes on Linux and macOS (ConPTY's own on Windows).
Accepted and ignored: `bell-action`, `escape-time`,
`terminal-overrides`, `focus-events`, `set-clipboard`, `renumber-windows`,
`allow-rename`, `automatic-rename`, `window-status-current-style`,
`mode-keys`, `aggressive-resize`, `set-titles`, `set-titles-string`,
`history-file`. keepane only: `save-history` (lines of each pane written into
the session file, colours kept; `all` for the whole scrollback), `autosave`, `restore-on-start`, `sessions-dir`,
`plugin-path`, `pane-timestamps` (each reported command's time at the end of its
line), `log-history`, `log-history-days`, `log-history-dir` (the history
log), `undo-kill-time`, `keep-zoom`, `animation`, `animation-time` (a zoom grows the pane to the window, and a
frame flies to where the keys go otherwise), `message-hop-limit`, `message-inbox-limit`, `message-max-size`,
`message-wait-max`, `agent-pane-limit`, `agent-commands`, `event-log`, `event-log-days`, `event-log-max`
(pane messages), and `@user` options.

Not a global here: tmux's per-window and per-pane option scopes. `set -w`
and `set -p` are accepted and set the option for the server.

`notify` is a keepane option: with it on, every alert also raises a desktop
notification (a tray balloon, which Windows 10 and 11 turn into a real
notification), so a job that finishes while the terminal is behind other
windows still reaches you. tmux has nothing like it, having no desktop to
notify.

Two things keepane adds that tmux does not have: an option name may be
abbreviated as long as it stays unambiguous (`set sync`, `set mon-act on`,
`set w-s-f ...`, each dash-separated word taking a prefix), and an on/off
option with no value flips (`set mouse`, `set sync`). tmux only abbreviates
command names, and requires a value.

## Alerts

`monitor-activity`, `monitor-bell` (on by default) and `monitor-silence`
flag a window nobody is looking at: `#` for output, `!` for a bell, `~` for
silence (and, keepane only, `@` for a message waiting in a pane in `normal` work mode), shown by `#F` in the status line and by `list-windows`. Looking at
the window clears its flags; `prefix M-n` / `M-p` walk to the next window
that has one. `visual-bell` / `visual-activity` put the alert on the status
line instead of ringing the terminal. Not there: `bell-action`,
`activity-action` and `silence-action` (accepted and ignored), and the
`alert-*` hooks.

## Still to do

Nothing from tmux 3.5's command table is outstanding; what is left are the
limits marked **part** above, none of them large: the `choose-tree` filter
is a substring rather than a format, `refresh-client` has no `-C`, and the
prefix key inside a popup stays keepane's (`prefix prefix` passes it on).
