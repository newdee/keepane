# Platforms: what differs, and where it lives

keepane began on Windows. Most of it never cared: the layout, the terminal
model (vt100), rendering, the command language, the options, the pane
messages and the event log, the dashboard, MCP, the phone page. What does
care is gathered under `src/platform/`, one module per platform with the
same names inside, chosen at compile time:

```rust
// src/platform/mod.rs
#[cfg(windows)] mod windows;
#[cfg(windows)] pub use windows::*;
#[cfg(unix)] mod unix;
#[cfg(unix)] pub use unix::*;
```

No traits: the rest of keepane calls `crate::platform::ipc::connect(..)`
and the like, and a function one platform lacks fails the build of that
platform. The step that moved the Windows code there changed no behaviour;
its test run is the same, test by test.

## The seams

| Seam | Windows | Linux / macOS |
|---|---|---|
| `ipc`: server listens, client connects | named pipe `\\.\pipe\keepane-<user>-<socket>`, DACL for the user and SYSTEM | Unix socket `<runtime>/keepane-<uid>/<socket>`, the directory 0700 (tmux's model) |
| `process`: start the server detached; run a shell command (`shell_command` for `run-shell`, `pipe_command` for `pipe-pane`) | `CreateProcessW`, no inherited handles, out of the caller's job when allowed; pwsh, else Windows PowerShell, else cmd | `setsid`, stdio to /dev/null (leaves a session's process group, so SSH hang-up does not reach it); `/bin/sh -c` |
| `process::Tree`: a pane's (or a `run-shell`'s) process tree ends with it | kill-on-close job object | the pane's session (`setsid` by portable-pty; an interactive shell puts each job in a group of its own): `SIGHUP`, then `SIGKILL` half a second later, to `-pgid` and, on Linux, to every member of the session (`/proc`) |
| `console`: the client's terminal | console API: raw mode, key and mouse records, resize events | termios raw mode, a VT input parser that makes the same key records, `SIGWINCH` |
| key records to a pane | win32-input-mode sequences (what ConPTY wants) | plain VT sequences (`encode_key`), text as UTF-8 |
| `shell`: the default shell and keepane's hook | pwsh, else Windows PowerShell; prompt hook through `-NoExit -Command` | `$SHELL`, else `/bin/sh`; bash through `--rcfile`, zsh through `ZDOTDIR`, each sourcing the user's own file first |
| per-pane shell history | `KEEPANE_SHELL_HISTORY` read by the hook (PSReadLine) | `HISTFILE` set per pane |
| `random`: the phone page's key | `BCryptGenRandom` | `/dev/urandom` |
| `clipboard` | Win32 clipboard | `pbcopy`, `wl-copy`, `xclip` or `xsel`, whichever is there; none (a server over SSH): an error, the paste buffer keeps the text |
| `notify` | toast with a "go to pane" button | `notify-send` / `osascript`, no button |
| `proccwd`, `sysinfo::program_of`: a pane's current directory and program | process environment block, toolhelp | `/proc/<pid>/cwd` and `/proc/<pid>/stat`; macOS `proc_pidinfo` |
| `sysinfo`: CPU, memory, battery, the host's name, the home as `~` | Win32, `COMPUTERNAME` | `/proc/stat`, `/proc/meminfo`, `/sys/class/power_supply`, `gethostname`; macOS `sysctl`, the host's CPU ticks (mach), no battery (IOKit) yet |
| `shutdown`: save before the machine goes | `WM_QUERYENDSESSION` | `SIGTERM` / `SIGHUP` to the server |
| `startup`: the server at logon | per-user `Run` value, `conhost --headless` | later: a systemd user unit / a launchd agent |
| `shell::SETS_TERM`: `TERM` in panes | no: ConPTY sets up its own | yes: `default-terminal` (`xterm-256color`), never the server's own `TERM` |
| `console::PTY_HAS_SCREEN`: a resumed pane's saved output | ConPTY keeps a screen of its own and repaints from it: a helper process (`keepane __replay`) prints the text into the console before the program starts | the pty keeps none: the text goes straight into keepane's screen model |

Directories need no module of their own: the `dirs` crate gives each
platform's (`%LOCALAPPDATA%\keepane` on Windows, `~/.local/share/keepane`
on Linux, `~/Library/Application Support/keepane` on macOS), and the home
directory for `~/.keepane.conf`.

Behind `ipc` and `process` on Windows sits `winsec`: the user's SID and the
pipe's security descriptor, and the kill-on-close job object.

Windows only, with nothing to port: the Windows Terminal profile (`wt`),
the MSI and Scoop updater (`update`), the move from wmux (`legacy`) and
the PowerShell tab completer (`completion`, until bash and zsh have one).

## Shells, apart from platforms

What a shell pane is typed in follows the shell, not the system: keepane's
prompt marker says which (`OSC 7777;keepane-prompt` from the PowerShell hook,
`OSC 7777;keepane-prompt;sh` from the bash and zsh hooks), and a message for
a `shell` pane goes in that syntax (`server::actor::Syntax`): the envelope
as a `<# … #>` comment and several lines through a script block in
PowerShell; the envelope as the argument of `:` and several lines through
`eval` in bash and zsh. A pwsh on Linux, or a bash keepane gave its hook
on Windows, gets its own.

## Order, and where it stands

1. Move the Windows code under `src/platform/windows/`, the rest calling
   `crate::platform::…`. No behaviour changes; the same tests pass, the
   same number, the same names. Done.
2. `src/platform/unix/`, built and tested on Linux (WSL, and CI on
   Ubuntu): the unit tests, then the end-to-end tests, whose harness starts
   `/bin/sh` panes there instead of `cmd.exe` (`PROMPT_SHELL` and friends
   in `tests/e2e.rs`); tests about Windows itself (PSReadLine, `C:\`
   paths, ConPTY, the shutdown window) are `cfg(windows)`, and bash has
   its own versions of the shell-message and directory tests.
3. macOS: the same Unix code, with `proccwd`, `sysinfo` and the process
   list from macOS. Built for both Mac targets here; run by CI on
   `macos-latest`.