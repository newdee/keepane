//! keepane: a terminal multiplexer whose panes keep running and pass messages to each other.

use anyhow::Result;
use keepane::{client, logger, server};

const USAGE: &str = "\
usage: keepane [-L socket-name] [command [flags]]

Sessions:
  new-session   (new)     [-s name] [-n window] [-c dir] [-d [-x cols] [-y rows]] [command...]
  attach-session (attach) [-t target] [-d]
  list-sessions (ls)
  kill-session  [-t target]      kill-server      has-session -t target
  rename-session [-t target] name
Windows / panes (from inside a session, or with -t):
  new-window (neww) [-n name] [-c dir] [command...]   kill-window   rename-window name
  select-window -t N   next-window   previous-window   last-window   list-windows
  swap-window -s A -t B   move-window -s A -t B   select-layout [-n|-p] [name]
  split-window (splitw) [-h|-v] [-c dir] [command...]  kill-pane   select-pane -L|-R|-U|-D
  resize-pane -L|-R|-U|-D [n] | -Z   swap-pane -U|-D   break-pane   list-panes
  send-keys [-t target] keys... | -X copy-command   capture-pane -p [-e] [-S -N]   copy-mode
  join-pane [-h|-v] -s A -t B   rotate-window   respawn-pane [-k]   display-panes   list-keys
  choose-tree [-s|-w]  (prefix s / w: pick a session / window with j k g G Enter q)
  pipe-pane [-o] [-t target] [command]   (no command stops the pipe)
Paste buffers:  set-buffer   list-buffers   show-buffer   delete-buffer   choose-buffer
  load-buffer file   save-buffer file   paste-buffer [-b name]
Other:  clock-mode   show-messages   list-clients   list-commands   if-shell   find-window
  set-environment name value   show-environment   refresh-client   select-layout [-n|-p|-E]
  choose-client (prefix D)   display-menu [-T title] name key command ...   (prefix > / <)
  display-popup [-E] [-C] [-w W] [-h H] [-d dir] [command...]   wait-for [-L|-U|-S] channel
  find-text [-C] [-n hits] [-t target] pattern   (search what every pane printed)
  jobs [-t session] [-F format]   (every pane: running or exited, up for how long, idle since when)
  choose-jobs   (prefix B: the same board as a picker; Enter goes there, x kills, r restarts)
  record [-t target] [out.cast]   (write the pane's output as asciinema v2; no path stops)
  notify [-T title] message   (a desktop notification; `set -g notify on` for alerts; on Windows with a Go-to-pane button)
  focus-pane %N   (every attached client switches to that pane and comes to the front)
Resume after a reboot (sessions autosave to the data directory's sessions folder):
  resume [name]   list-saved   save-session [-t target|-a]   restore-session [-a] [name]   delete-saved name
  set-cwd [-t target] [dir]   (record the directory a pane resumes in; default: caller's cwd)
  startup on|off|status   (Windows: start the server at logon and restore every saved session)
  windows-terminal install|remove|status   (Windows: a keepane profile in the Windows Terminal dropdown)
Upgrading:  version (this keepane and the server's)   update [--check]   restart-server (sessions move to this version)
  migrate   (from wmux, keepane's old name: sessions, saved data, logon start, Windows Terminal profile)
On a phone:  web [--port N] [--bind IP[,IP...]] [--read-only] [--keep-key]   (prints a QR code; scan it on the same network)
History (what panes printed, a file a day, 30 days):  choose-history (prefix /)   view FILE
  list-marks [-t pane]   (the commands a pane ran, with their times; prefix C-t shows them on the lines)
  copy-output [-p] [-t pane]   (what the last command printed, to a buffer and the clipboard; -p prints it; prefix y)
  hints   (prefix F: label paths, addresses and hashes on screen; a label copies, in capitals opens; hint-open)
  shell-history [-t pane] [-c | -m] [-n count]   (a pane's own shell history, messages marked; -c commands only, -m every message sent to it)
  undo-kill   (prefix u: the pane or window killed in the last 10 seconds comes back; undo-kill-time)
Panes that talk (a name, a work mode, an inbox; docs/design/mailbox.md):
  rename-pane [-t pane] name   (then -t %name finds it; a full address $1:@3.%7 works too)   whoami
  set-work-mode [-t pane] normal|shell|ai   (shell: runs what it gets; ai: its agent says pane-ready; from a pane, only that pane)
  send-message [--to pane] [-r | --re id] [--task id] [-w secs] text   read-message [-w secs]   list-messages [-t pane] [-a]
  trace-message id [-w secs] [-J]   drop-message id | -u   move-message id up|down|top   pane-status [text]
  list-tasks [-t session]   show-task id [-J]   list-events [-t target] [-S 1h] [-n lines]   (the event log, 30 days)
  create-pane   (what agents use: agent-commands, agent-pane-limit)   mcp   (MCP for an agent)
  dashboard (prefix v: panes, inboxes, tasks and the chosen pane's screen; act on them; ? keys)   setup [claude|codex|gemini|cursor|opencode] [--install]
Panes on other machines (both run `keepane web`; docs/design/link.md):
  link add <its keepane web address>   (pair, like SSH keys, once, both ways)   link list   link panes host:port
  send-message --to host:port/%name text   trace-message id (asks there too)   link info host:port
  link capture [-S lines] host:port/%name   link allow host:port [--shell|--no-shell] [--screen|--no-screen] [--panes|--no-panes]
  link start host:port [-s session] [-n name] [-m mode] [-c dir] [-- program args]   link kill host:port/%pane
  link id   link trust host:port key [--shell] [--screen] [--panes]   link remove host:port   link rekey
Keys not arriving?  show-keys   (prints each key as the console hands it over and as keepane reads it; q quits)
Plugins / scripting:
  run-shell [-b] command   set-hook -g hook command   show-hooks   load-plugin name   list-plugins
  show-options [-gqv] [name]
Config: ~/.keepane.conf (tmux syntax: set -g prefix C-a, bind h select-pane -L, set -g @plugin name).
  import-config [-n] [-o file] [file]   (bring what keepane can use from ~/.tmux.conf or any config into it;
  the rest goes in commented out, with why; -n shows it and writes nothing; tmux.conf is never read on its own)
Tab completion:  completion powershell|bash|zsh|fish   (prints the completer to load; see README)
The manual:  man   (every command, key, option and file, as Markdown; man --roff gives it to man(1))
Any unambiguous prefix of a command name works: `keepane att`, `keepane lsp`, `keepane splitw -h`.
Option names too (`set sync`, `set mon-act on`); an on/off option with no value flips it.
Default prefix: C-b.  Prefix ? lists key bindings, prefix q shows pane numbers.";

fn main() {
    // args() panics on non-UTF-8 (unpaired surrogates in a path); be lossy instead.
    let mut args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    // Inside a pane, talk to the server that owns it (tmux does the same with
    // $TMUX), so `keepane ls` from a script or a plugin means "this server".
    let mut socket = std::env::var("KEEPANE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into());
    // Global flags.
    loop {
        match args.first().map(String::as_str) {
            Some("-L") => {
                args.remove(0);
                if args.is_empty() {
                    eprintln!("-L: missing socket name");
                    std::process::exit(1);
                }
                socket = args.remove(0);
            }
            Some(s) if s.starts_with("-L") => {
                socket = args.remove(0)[2..].to_string();
            }
            // These answer locally, so they take nothing else: a typo must not
            // look like it worked.
            Some("-h" | "--help" | "help") => {
                if args.len() > 1 {
                    eprintln!("{}: takes no arguments", args[0]);
                    std::process::exit(1);
                }
                println!("{USAGE}");
                return;
            }
            // tmux's -V: this program's version and nothing else.
            Some("-V" | "--version") => {
                if args.len() > 1 {
                    eprintln!("{}: takes no arguments", args[0]);
                    std::process::exit(1);
                }
                println!("keepane {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => break,
        }
    }
    // `keepane __replay file`: the helper a resumed pane runs first. It prints
    // the pane's saved output into the console it was started in (the
    // pane's ConPTY) and removes the file, so the text is in the console's
    // own buffer before the shell starts.
    if args.first().map(String::as_str) == Some("__replay") {
        let Some(path) = args.get(1) else {
            eprintln!("__replay: file required");
            std::process::exit(2);
        };
        let code = match std::fs::read_to_string(path) {
            Ok(text) => match keepane::console::write_to_console(&text)
                .and_then(|()| keepane::console::write_to_console(keepane::server::pane::REPLAY_MARKER))
            {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("__replay: {e:#}");
                    1
                }
            },
            Err(e) => {
                eprintln!("__replay: {path}: {e}");
                1
            }
        };
        // The console lives as long as its first process: stay until the
        // server has started the pane's program in it, which it signals by
        // removing the file. A server that never does is not waited on
        // forever.
        if code == 0 && std::env::var_os("KEEPANE_REPLAY_NO_WAIT").is_none() {
            let started = std::time::Instant::now();
            while std::path::Path::new(path).exists() && started.elapsed() < std::time::Duration::from_secs(60) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let _ = std::fs::remove_file(path);
        std::process::exit(code);
    }
    // `keepane __shell-hook`: the prompt hook keepane gives an interactive
    // shell (PowerShell on Windows, bash elsewhere), for a shell started
    // with a script of its own (which gets none): `Invoke-Expression
    // (keepane __shell-hook)` or `eval "$(keepane __shell-hook)"` in that
    // script. The picture scripts in tools/ use it.
    if args.first().map(String::as_str) == Some("__shell-hook") {
        println!("{}", keepane::config::PROMPT_HOOK);
        return;
    }
    // A notification's "Go to pane" button opens a keepane:// URL, which the
    // protocol registration hands to us as the one argument.
    if let Some(url) = args.first().filter(|a| a.starts_with("keepane://")).cloned() {
        match keepane::notify::parse_go_url(&url) {
            Some((sock, pane)) => {
                socket = sock;
                args = vec!["focus-pane".into(), format!("%{pane}")];
            }
            None => {
                eprintln!("keepane: not a keepane://go/<socket>/<pane> link: {url}");
                std::process::exit(1);
            }
        }
    }
    if args.is_empty() {
        args.push("new-session".into());
    }
    // Up from wmux (keepane's old name): its data directory moves here the
    // first time, unless a wmux server still uses it (`migrate` then).
    if args[0] != "__server" && args[0] != "migrate" {
        keepane::legacy::move_data_dir_at_start();
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    // Starting at logon is about the server, and the Windows Terminal
    // profile is a file of ours: both are settled without a server.
    let local = match args[0].as_str() {
        "startup" => Some(keepane::startup::run(&socket, &args[1..])),
        "windows-terminal" | "wt" => Some(keepane::wt::run(&socket, &args[1..])),
        "completion" => Some(keepane::completion::run(&args[1..])),
        // The manual as Markdown (docs/keepane.1.md), or as roff for `man`.
        "man" => match &args[1..] {
            [] => {
                print!("{}", keepane::man::MARKDOWN);
                Some(Ok(0))
            }
            [f] if f == "--roff" => {
                print!("{}", keepane::man::roff(keepane::man::MARKDOWN));
                Some(Ok(0))
            }
            _ => Some(Err(anyhow::anyhow!("man: takes nothing, or --roff"))),
        },
        // What the bash / zsh / fish completers ask, on every Tab.
        "__complete" => Some(keepane::completion::run_complete(&args[1..])),
        "setup" => Some(keepane::setup::run(&args[1..])),
        // A file of ours from a file of someone else's: no server needed.
        "import-config" => Some(rt.block_on(async { keepane::server::import::run(&args[1..]) })),
        // The hook every Claude Code session runs: outside a pane there is
        // nothing to tell and no server to ask (or to start).
        "pane-ready" if args.iter().any(|a| a == "-q") && std::env::var_os("KEEPANE_PANE").is_none() => Some(Ok(0)),
        "version" | "restart-server" | "show-keys" if args.len() > 1 => {
            Some(Err(anyhow::anyhow!("{}: takes no arguments", args[0])))
        }
        "show-keys" => Some(keepane::console::show_keys()),
        "view" => match args.get(1..) {
            Some([file]) => Some(keepane::pager::run(file)),
            _ => Some(Err(anyhow::anyhow!("usage: keepane view FILE"))),
        },
        // These talk to the server, but as a client of their own: the
        // version of this program next to the server's, and moving the
        // sessions to a server of this version.
        "version" => Some(rt.block_on(client::version(&socket))),
        "restart-server" => Some(rt.block_on(client::restart_server(&socket))),
        // From wmux (keepane's old name): sessions, data and registrations.
        "migrate" if args.len() > 1 => Some(Err(anyhow::anyhow!("migrate: takes no arguments"))),
        "migrate" => Some(rt.block_on(client::migrate(&socket))),
        "update" => Some(rt.block_on(keepane::update::run(&socket, &args[1..]))),
        // The panes on a phone: a client of the server that also answers
        // HTTP on the local network, until Ctrl+C.
        "web" => Some(rt.block_on(keepane::web::run(&socket, &args[1..]))),
        // Panes on other machines (docs/design/link.md): pairing, the
        // machines paired, what each may do.
        "link" => Some(rt.block_on(keepane::link::run(&socket, &args[1..]))),
        // MCP for an agent in a pane: stdin/stdout, until the agent closes it.
        "mcp" if args.len() > 1 => Some(Err(anyhow::anyhow!("mcp: takes no arguments"))),
        "mcp" => Some(rt.block_on(keepane::mcp::run(&socket))),
        // Every pane at a glance, in this terminal (prefix v opens it in a
        // popup, with `--popup`: going to a pane closes it).
        "dashboard" | "dash" => match &args[1..] {
            [] => Some(keepane::dashboard::run(&socket, &rt, false)),
            [p] if p == "--popup" => Some(keepane::dashboard::run(&socket, &rt, true)),
            _ => Some(Err(anyhow::anyhow!("dashboard: takes no arguments"))),
        },
        _ => None,
    };
    if let Some(result) = local {
        let code = match result {
            Ok(c) => c,
            Err(e) => {
                eprintln!("keepane: {e:#}");
                1
            }
        };
        std::process::exit(code);
    }
    let code = if args[0] == "__server" {
        logger::init("server");
        // The server has no console; a panic would otherwise vanish.
        std::panic::set_hook(Box::new(|info| {
            log::error!("panic: {info}");
        }));
        let restore = args.iter().skip(1).any(|a| a == "--restore");
        let options = server::RunOptions { force_restore: restore, config: None };
        match rt.block_on(server::run_with(socket, options)) {
            Ok(()) => 0,
            Err(e) => {
                log::error!("{e:#}");
                1
            }
        }
    } else {
        match rt.block_on(run_client(socket, args)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("keepane: {e:#}");
                1
            }
        }
    };
    // Do not wait for blocked console-input threads.
    std::process::exit(code);
}

async fn run_client(socket: String, args: Vec<String>) -> Result<i32> {
    client::run(socket, args).await
}
