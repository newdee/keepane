# What keepane 1.x keeps

From 1.0, keepane's version says what an upgrade may do:

- **1.x.y → 1.x.(y+1)**: fixes only.
- **1.x → 1.(x+1)**: additions. Nothing that worked stops working.
- **2.0**: the only place something may be taken away or change meaning.

## Kept through 1.x

What scripts, configs, agents and other machines rely on. The manual
(`keepane man`, docs/keepane.1.md) lists each of these; for commands,
options, format variables, `KEEPANE_*` variables and MCP tools the tests
check the lists are whole:

- **Commands and their flags**: every command under COMMANDS, by its name
  and its abbreviations as tmux takes them, with the meaning of each flag.
- **Options**: every option in "Every option", by name, with its values.
  A default may change in a 1.x only where nothing changes for a config
  that sets the option.
- **Formats**: the `#{…}` variables and their meaning.
- **Keys**: the default bindings (a binding of yours always wins).
- **MCP tools**: their names and arguments (new arguments are optional).
- **JSON**: the fields of every `-J` answer, of the event log
  (`list-events`, the files a day), of the done-events' webhook and
  `pane-done` hook; new fields may be added, none removed or renamed.
- **Files**: the config and the settings file (`settings-file`), themes of
  your own (`NAME.conf` in `theme-path`, their `@` tokens), agent
  definitions (`~/.keepane/agents/*.toml`), saved sessions, the history
  and event logs, the link keys and tables, `feishu-users`; a 1.x reads
  what any earlier 1.x (and 0.77) wrote.
- **Environment**: every `KEEPANE_*` variable under ENVIRONMENT.
- **Between machines**: any two 1.x talk to each other (`link`); a newer
  one says plainly what an older one cannot do yet, as now.
- **Exit codes**: 0 for done, non-zero for refused or failed.

## Not kept

- **Words meant for people**: what a command prints without `-J`, the
  status line's and the dashboard's layout, error messages. Read `-J`.
- **Experimental**: the group chat (`chat-*`, its MCP tools, its group
  files and its files under `chat/`, panes of other machines in a
  channel included), until a release says it is not.
- **Judgements**: what an agent's screen is read as (screen rules), the
  cost of its tokens (a public price list), how its transcript is found.
  These get better; they are not a format.
- **Internal**: `__`-prefixed commands, the `KEEPANE_` variables keepane
  sets for itself or its tests, logs (`server.log`).

## Taking something away

Only in 2.0, and only after a 1.x said so: the old form keeps working
through every 1.x, and from the release that retires it, using it says
what replaces it (on the status line once, and in `show-messages`).

Before 1.0 (0.77) these went, each years older than its replacement, with
a way across in the README: keepane's old name wmux (`WMUX_*`,
`~/.wmux.conf`, `~/.wmux/plugins`, `keepane migrate`; up to 0.13.1), the
five link permissions of 0.55, and the clock animations of 0.45.
