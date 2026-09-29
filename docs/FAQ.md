# keepane FAQ

[中文](FAQ.zh-CN.md) · [README](../README.md) · [Feature tour](https://dfine.tech/keepane/)

Each answer says what you see, why, and what to do.

- [Codex (or another full-screen program): I can't scroll back through the conversation](#codex-or-another-full-screen-program-i-cant-scroll-back-through-the-conversation)
- [An agent's pane never takes its messages; they stay queued](#an-agents-pane-never-takes-its-messages-they-stay-queued)
- [A pane stays busy and nothing is delivered](#a-pane-stays-busy-and-nothing-is-delivered)
- [How do I change a pane's work mode?](#how-do-i-change-a-panes-work-mode)
- [I swapped two panes: where do messages go now?](#i-swapped-two-panes-where-do-messages-go-now)
- [A message from another machine is refused by a shell pane](#a-message-from-another-machine-is-refused-by-a-shell-pane)
- [The phone can't open the page](#the-phone-cant-open-the-page)
- [Tell me on my phone when a job finishes, even when it is locked](#tell-me-on-my-phone-when-a-job-finishes-even-when-it-is-locked)
- [The Feishu or DingTalk bot refuses keepane's messages](#the-feishu-or-dingtalk-bot-refuses-keepanes-messages)
- [Windows: a hook with a quoted keepane path fails](#windows-a-hook-with-a-quoted-keepane-path-fails)
- [macOS: "Operation not permitted" in some folders](#macos-operation-not-permitted-in-some-folders)

## Codex (or another full-screen program): I can't scroll back through the conversation

**What you see.** In a pane running Claude Code you can scroll up (`C-b [`, the
phone, `C-b /` for the day's history) and read the whole conversation. In a
pane running Codex, what went by is gone.

**Why.** Codex draws in the terminal's *alternate screen*, like vim or htop:
a screen of its own that it redraws in place, so nothing ever scrolls off
into the terminal's history. keepane, like tmux and every terminal, keeps
history only from the normal screen. Claude Code prints its conversation
down the normal screen, so its lines scroll into history.

**What to do.** Run Codex on the normal screen:

```sh
codex --no-alt-screen
```

or for good, in `~/.codex/config.toml`:

```toml
[tui]
alternate_screen = "never"
```

(Codex's default, `auto`, turns the alternate screen off only inside Zellij.)
Without changing anything, `Ctrl+T` in Codex opens its own transcript.

Codex's issue tracker has reports that on the normal screen it still redraws
the whole screen now and then, which leaves repeated lines in the history
(seen in Zellij and the VS Code terminal). If the history looks muddled,
that is Codex's redrawing; its transcript (`Ctrl+T`) is the clean record.
The same holds for any full-screen program: keepane cannot keep what the
program never lets scroll.

## An agent's pane never takes its messages; they stay queued

**What you see.** `send-message` to a pane in `ai` work mode says `queued
... (ai, busy, ...)`, and the message never arrives; the dashboard or the
phone shows it waiting. It looks as if the agent forgets to check its inbox.

**Why.** An agent does not read its inbox itself. At the end of each turn
its hook runs `keepane pane-ready`, and keepane types the next message in as
its prompt. Without that hook the pane never counts as free. keepane says so:
the reply to `send-message` gets a second line, `%N has not said it is free
since its agent started ...`, and the dashboard and the phone mark the pane.

**What to do.**

```sh
keepane setup                    # each agent: on this machine? hook? MCP server?
keepane setup codex --install    # or claude, gemini, cursor, opencode
```

Every file is backed up first and only keepane's entries are added. Then
start the agent again in the pane. Codex runs a new hook only once you trust
it: in Codex, `/hooks`, and trust keepane's two. Another agent works if it
can run a command at the end of each turn: have it run `keepane pane-ready
-q` there.

## A pane stays busy and nothing is delivered

**What you see.** The pane's agent or shell is plainly waiting, but keepane
still counts it as busy.

**Why.** Typing into a pane makes it busy until its next signal (the shell's
prompt, the agent's hook), so a message never lands in the middle of a line
being typed. Text typed and then deleted, or a prompt that was not redrawn,
can leave it waiting for a signal that does not come.

**What to do.** From outside the pane (another terminal, or the `C-b :`
prompt):

```sh
keepane pane-ready -t %agent
```

## How do I change a pane's work mode?

In that pane, run `keepane set-work-mode ai` (or `shell`, or `normal`). From
outside keepane (a terminal, the `C-b :` prompt, a key) you can name the
pane: `keepane set-work-mode -t %builder shell`. A command run in one pane
cannot change another pane's mode, so nothing in one pane can turn another
into a shell that runs what it is sent.

## I swapped two panes: where do messages go now?

A pane's id (`%7`) and name (`%builder`) go with the pane, wherever it is
moved or swapped (`C-b {`, `C-b }`, `swap-pane`). A message to `%7` or
`%builder` reaches the same pane as before. A position (`work:0.1`) names
whatever pane is there now. A full address (`$1:@2.%7`) also says where the
pane is: if the pane has moved to another window, the delivery is refused
rather than sent to the wrong place.

## A message from another machine is refused by a shell pane

**Why.** What comes from a paired machine goes only to `ai` and `normal`
panes. A shell pane would run it as a command, so that is off until you allow
it for that machine.

**What to do.** On the receiving machine, in a terminal outside keepane (not
in a pane, so an agent cannot grant it to itself):

```sh
keepane link allow 192.168.1.20:7681 --shell
```

`--screen` lets that machine read a pane's screen (`keepane link capture`).

## The phone can't open the page

- The phone and the computer must be on the same network, or both on
  Tailscale. `keepane web status` shows the address being served and who is
  connected.
- Windows asks the first time whether keepane may use the network: allow it
  for private networks.
- The code changes each time `keepane web` starts (unless `--keep-key`):
  scan the new one.
- It is plain HTTP, fine at home; from outside, go through Tailscale or a
  similar private network, not a port opened to the internet.

## Tell me on my phone when a job finishes, even when it is locked

The phone's page tells you (a banner, a sound, a buzz on Android) while it
is open. A locked phone stops its pages, and a page on plain HTTP cannot
raise a system notification, so for a locked phone let keepane post to a
service that has an app:

- **ntfy** (free, open source, iOS and Android; or your own server):
  install the app, subscribe to a topic of your own (anyone who knows a
  topic's name on ntfy.sh can read it: pick one hard to guess, or run your
  own server), and
  ```sh
  set -g done-webhook https://ntfy.sh/<your-topic>
  set -g done-webhook-format text
  ```
- **A chat you already use**: Feishu, WeCom, DingTalk, Slack or Discord. Add
  an incoming-webhook bot to a group, then
  ```sh
  set -g done-webhook <the bot's address>
  set -g done-webhook-format feishu    # or wecom, dingtalk, slack, discord
  ```
- **Anything else**: the `pane-done` hook runs a command of yours with the
  details in `KEEPANE_DONE_*` variables (see the README).

What counts is `done-events` (`command agent` by default: a command that ran
`done-after` seconds, 30 by default, and an agent's turn), for panes with a
name or in `ai`/`shell` mode (`done-panes all` for every pane). Put the
lines in your config file to keep them.

## The Feishu or DingTalk bot refuses keepane's messages

`show-messages` says `done-webhook: refused (...)` with what the chat
answered. A bot's security setting decides:

- **Keywords**: every message keepane sends a chat starts with `keepane`;
  add `keepane` as the bot's keyword.
- **Signature** (Feishu's "signature verification", DingTalk's "sign"):
  keepane does not sign; turn it off, or use keywords or an IP allow list
  instead.
- **IP allow list**: the address the computer reaches the internet from
  must be on it.
## Windows: a hook with a quoted keepane path fails

An agent on Windows may run its hooks in PowerShell, where a quoted path
followed by arguments is a syntax error. Write the program unquoted
(`keepane pane-ready -q`, with keepane on the PATH), or with the call
operator: `& "C:\path\keepane.exe" pane-ready -q`. `keepane setup` writes the
unquoted form.

## macOS: "Operation not permitted" in some folders

**What you see.** `ls` in Documents, Desktop, Downloads or another protected
folder says `Operation not permitted`, sometimes only after reconnecting.

**Why.** macOS privacy protection (TCC) decides per terminal application
which folders its programs may read. keepane's server and every pane inherit
the permission of the terminal that started them. If the same `ls` fails in
iTerm2 or Terminal outside keepane too, keepane is not involved.

**What to do.** System Settings → Privacy & Security → Full Disk Access (or
Files and Folders): turn it on for your terminal application, then quit and
reopen that application. A keepane server started before the change may
still run with the old permission: `keepane kill-server`, start it again
from the terminal, and `keepane resume` brings the sessions back (their
layout and directories; the programs in them start afresh).
