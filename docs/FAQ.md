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
its prompt. For Claude Code, Codex and pi keepane also reads the agent's
transcript, and frees the pane about 3 seconds at the most after a turn there is over;
any other agent without that hook never counts as free. keepane says so:
the reply to `send-message` gets a second line, `%N has not said it is free
since its agent started ...`, and the dashboard and the phone mark the pane.

**What to do.**

```sh
keepane setup                    # each agent: on this machine? hook? MCP server?
keepane setup codex --install    # or claude, gemini, cursor, opencode
```

Every file is backed up first and only keepane's entries are added. Then
start the agent again in the pane. Codex runs a new hook only once you trust
it: in Codex, `/hooks`, and trust keepane's. Another agent works if it
can run a command at the end of each turn: have it run `keepane pane-ready
-q` there.

## A pane stays busy and nothing is delivered

**What you see.** The pane's agent or shell is plainly waiting, but keepane
does not count it as free (`#{pane_state}` reads `typing` or `busy`).

**Why.** Typing into a pane holds its messages until its next signal (the
shell's prompt, the agent's hook or, for Claude Code, Codex and pi, a turn
over in its transcript), so a message never lands in the middle of a line
being typed: keys with no Enter read `typing`. Text typed and then deleted,
or a prompt that was not redrawn, can leave it waiting for a signal that
does not come. An `ai` pane whose agent has exited reads `no-agent`, and
waits for an agent to be started there.

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

## Another machine says "no pane by that name is shared"

**Why.** Pairing gives nothing. A paired machine sees and messages only
what the machine it reaches shares with it, and a pane not shared is
answered as one that is not there. The machine being reached decides.

**What to do.** On the machine whose pane is to be reached, in a terminal
outside keepane (not in a pane, so an agent cannot grant it to itself):

```sh
keepane link share 192.168.1.20:7681 %worker   # or a session (work), a window (work:logs)
keepane link allow 192.168.1.20:7681 --full    # or all of this machine
```

Or press `s` on it in `C-b w`. A message to a shared `shell` pane runs as a
command. `--full` also lets that machine start panes here and send commits
and projects; it is taken only over Tailscale, unless `--full --lan`.
`keepane link list` shows, both ways, what each machine has of the other.

## `keepane link allow --shell` says the permissions are two levels now

**Why.** Up to 0.55, `link allow` had five permissions (`--shell`,
`--screen`, `--panes`, `--push`, `--agents`, and `--all`). Since 0.56
there are two levels: what is shared (`keepane link share`), or all of the
machine (`--full`). A machine that had all five has `--full` after the
update; one that had some of them has nothing, and the status line names
it once.

**What to do.** Share what it needs (`keepane link share <host:port>
<session|window|pane>`), or give it `--full`.

## `keepane link share` says it is "not run from inside a pane"

**Why.** Since 0.54 keepane checks where `keepane link share`, `keepane
link allow` (and `keepane link trust --full`, and `keepane feishu allow`)
run. In a keepane pane they are refused, because an agent in that pane
could otherwise let another machine in. Before 0.54, `keepane link allow`
typed in a pane was let through by mistake. Taking back (`keepane link
unshare`) works anywhere.

**What to do.** Run it in one of two places:

- A terminal that is not a keepane pane (a plain Windows Terminal tab,
  another terminal app).
- The `C-b :` prompt of any keepane client. There the command has its
  server name, `link-share` or `link-allow`:

```sh
link-share 192.168.1.20:7681 %worker
```

Or press `s` in `C-b w`: a key you press is not a pane's program.

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

## A shared link says "this link has ended"

- Its time was up (24 hours unless `--for` said), it was stopped, or it was
  `--once` and went once. `keepane share` lists what is still shared.
- Shares last only as long as the server: after `restart-server` or a
  reboot, share it again.
- The link is on the phone page's port: after `keepane web stop` none of
  them answers; `keepane web` serves them again (on its new port, if it
  changed: `keepane share` lists the links as they are now).

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

What counts is `done-events` (`command agent wait` by default: a command
that ran `done-after` seconds, 30 by default, an agent's turn, and an agent
waiting for you: a permission or a question, once its waiting hook is set
up with `keepane setup <agent> --install`), for panes with a name or in
`ai`/`shell` mode (`done-panes all` for every pane). Put the lines in your
config file to keep them.

## The Feishu or DingTalk bot refuses keepane's messages

`show-messages` says `done-webhook: refused (...)` with what the chat
answered. A bot's security setting decides:

- **Keywords**: every message keepane sends a chat starts with `keepane`;
  add `keepane` as the bot's keyword.
- **Signature** (Feishu's "signature verification", DingTalk's "sign"):
  put the secret the bot shows in `set -g done-webhook-secret <secret>`,
  and keepane signs each message. Feishu says `refused (19021)` for a
  wrong secret, or a clock more than an hour off.
- **IP allow list**: the address the computer reaches the internet from
  must be on it.

## Feishu: the app does not connect, or does not answer

`keepane feishu` says where it stands:

- **`off`**: `feishu-app-id` or `feishu-app-secret` is not set. Set both
  in `keepane.conf`: `set -g` alone is gone when the server restarts.
- **`Feishu refused (1000040346): app_id is invalid`**, or another code:
  the App ID or App Secret is wrong, or the app is not an enterprise
  self-built one. Lark's apps need `set -g feishu-domain lark`.
- **`connected`, and the bot says nothing**: the app's events are not set
  to the long connection, `im.message.receive_v1` is not added, or the
  version with them is not published. Feishu saves the long connection
  only while keepane is connected.
- **The bot answers with your open_id**: you are not let in yet. Run the
  command it gives in a terminal outside keepane (`keepane feishu allow
  ou_...`); from inside a pane it is refused.
- **`events come encrypted`** on the status line: clear the Encrypt Key
  in the app's event settings.
- **Messages land on another machine**: Feishu gives each event to one
  connection of an app. Use one app for each machine.

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
