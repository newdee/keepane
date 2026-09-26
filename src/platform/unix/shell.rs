//! The shell a pane starts with, and keepane's hook in it: bash and zsh get
//! a start file of keepane's that first reads the user's own (`~/.bashrc`,
//! `$ZDOTDIR/.zshrc`), then reports the directory (OSC 7), each command's
//! start and end with its status (OSC 133), and the prompt itself (`OSC
//! 7777;keepane-prompt;sh`, at its end, as the PowerShell hook does, `;sh`
//! saying commands are typed to it in POSIX syntax), and keeps
//! the pane's own history file (`KEEPANE_SHELL_HISTORY`), written command by
//! command.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `default-shell`'s default: the user's login shell.
pub fn default_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())
}

/// `agent-commands`'s default: what an agent may start in a pane it makes.
pub const DEFAULT_AGENT_COMMANDS: &str = "bash zsh sh claude codex";
/// The variable that tells a pane's shell its own history file (read by the hook).
pub const HISTORY_VAR: &str = "KEEPANE_SHELL_HISTORY";
/// Panes are told the terminal they draw on (`TERM`, from `default-terminal`).
pub const SETS_TERM: bool = true;

/// The command line for a shell named in `default-shell`.
pub fn named(s: &str) -> Vec<String> {
    if s.trim().is_empty() { vec![default_shell()] } else { vec![s.to_string()] }
}

/// Whether a pane running the program `stem` is a shell keepane hands
/// commands to (its hook says when it is at a prompt).
pub fn takes_commands(stem: &str) -> bool {
    matches!(stem, "bash" | "zsh")
}

/// The history file a pane with no history of its own yet starts from.
pub fn shared_history() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [".bash_history", ".zsh_history"].iter().map(|f| home.join(f)).find(|p| p.is_file())
}

/// The bash hook, for `keepane __shell-hook` (to `eval` in a shell keepane
/// did not start).
pub const PROMPT_HOOK: &str = r#"
if [ -n "$KEEPANE_SHELL_HISTORY" ]; then HISTFILE="$KEEPANE_SHELL_HISTORY"; history -c; history -r; fi
__keepane_status() { __keepane_ok=$?; }
__keepane_prompt() {
  history -a
  printf '\e]133;D;%s\e\\\e]7;file://%s%s\e\\' "$__keepane_ok" "${HOSTNAME:-localhost}" "$PWD"
  case "$PS1" in
    *keepane-prompt*) ;;
    *) PS1="\[$(printf '\033]133;A\007')\]$PS1\[$(printf '\033]133;B\007\033]7777;keepane-prompt;sh\007')\]" ;;
  esac
}
PS0="$(printf '\033]133;C\007')"
PROMPT_COMMAND="__keepane_status;${PROMPT_COMMAND:+$PROMPT_COMMAND;}__keepane_prompt"
"#;

/// The zsh hook, in `.zshrc` of keepane's own `ZDOTDIR`.
const ZSH_HOOK: &str = r#"
if [ -n "$KEEPANE_SHELL_HISTORY" ]; then
  HISTFILE="$KEEPANE_SHELL_HISTORY"
  # zsh saves nothing unless told how much (SAVEHIST is 0 by default).
  (( SAVEHIST )) || SAVEHIST=10000
  (( HISTSIZE >= SAVEHIST )) || HISTSIZE=$SAVEHIST
  fc -R "$HISTFILE" 2>/dev/null
fi
setopt INC_APPEND_HISTORY
__keepane_precmd() {
  local ok=$?
  printf '\e]133;D;%s\e\\\e]7;file://%s%s\e\\' "$ok" "${HOST:-localhost}" "$PWD"
  case "$PS1" in
    *keepane-prompt*) ;;
    *) PS1="%{"$'\e]133;A\a'"%}$PS1%{"$'\e]133;B\a\e]7777;keepane-prompt;sh\a'"%}" ;;
  esac
}
__keepane_preexec() { printf '\e]133;C\e\\'; }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __keepane_precmd
add-zsh-hook preexec __keepane_preexec
"#;

/// keepane's start files, written once per process under its data
/// directory: `bashrc`, and `zdotdir/.zshrc` (with the other zsh start
/// files passing through to the user's).
fn start_files() -> &'static Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = crate::logger::log_dir().join("shell");
        let z = dir.join("zdotdir");
        std::fs::create_dir_all(&z).ok()?;
        let bashrc = format!("[ -f ~/.bashrc ] && . ~/.bashrc\n{PROMPT_HOOK}");
        std::fs::write(dir.join("bashrc"), bashrc).ok()?;
        // zsh reads these from $ZDOTDIR; each reads the user's own first.
        let user = r#"${KEEPANE_ZDOTDIR:-$HOME}"#;
        for f in [".zshenv", ".zprofile", ".zlogin"] {
            std::fs::write(z.join(f), format!("[ -f \"{user}/{f}\" ] && . \"{user}/{f}\"\n")).ok()?;
        }
        let zshrc = format!("ZDOTDIR=\"{user}\"\n[ -f \"$ZDOTDIR/.zshrc\" ] && . \"$ZDOTDIR/.zshrc\"\n{ZSH_HOOK}");
        std::fs::write(z.join(".zshrc"), zshrc).ok()?;
        Some(dir)
    })
}

/// Whether `argv` is an interactive shell of its own (no command, no
/// script): only options follow the program.
fn interactive(argv: &[String]) -> bool {
    argv[1..].iter().all(|a| matches!(a.as_str(), "-i" | "-l" | "--login" | "--noprofile"))
}

/// `argv` with keepane's shell integration where it applies: an interactive
/// bash or zsh. Anything else, and a shell running a command or a script, is
/// left exactly as given.
pub fn with_shell_integration(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else { return Vec::new() };
    let stem = Path::new(first).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if !interactive(argv) {
        return argv.to_vec();
    }
    let Some(dir) = start_files() else { return argv.to_vec() };
    match stem.as_str() {
        "bash" => {
            // --rcfile is read by an interactive shell that is not a login one.
            let mut run = vec![first.clone(), "--rcfile".into(), dir.join("bashrc").to_string_lossy().into_owned()];
            run.extend(argv[1..].iter().filter(|a| !matches!(a.as_str(), "-l" | "--login")).cloned());
            if !run.iter().any(|a| a == "-i") {
                run.push("-i".into());
            }
            run
        }
        "zsh" => {
            let mut run = vec![
                "/usr/bin/env".into(),
                format!("ZDOTDIR={}", dir.join("zdotdir").display()),
                format!(
                    "KEEPANE_ZDOTDIR={}",
                    std::env::var("ZDOTDIR").ok().filter(|z| !z.is_empty()).unwrap_or_else(|| {
                        dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
                    })
                ),
            ];
            run.extend(argv.iter().cloned());
            run
        }
        _ => argv.to_vec(),
    }
}

/// Minimal PATH lookup for an executable name.
pub fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    let p = Path::new(name);
    if name.contains('/') {
        return executable(p).then(|| p.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|c| executable(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn integration_goes_to_interactive_bash_and_zsh_only() {
        let run = with_shell_integration(&s(&["/bin/bash"]));
        assert_eq!(run[0], "/bin/bash");
        assert_eq!(run[1], "--rcfile");
        assert!(run[2].ends_with("bashrc"), "{run:?}");
        assert!(run.contains(&"-i".to_string()));
        let z = with_shell_integration(&s(&["zsh", "-l"]));
        assert_eq!(z[0], "/usr/bin/env");
        assert!(z[1].starts_with("ZDOTDIR=") && z[1].ends_with("zdotdir"), "{z:?}");
        assert_eq!(&z[3..], &s(&["zsh", "-l"])[..]);
        for argv in [s(&["bash", "-c", "ls"]), s(&["bash", "x.sh"]), s(&["sh"]), s(&["fish"]), Vec::new()] {
            assert_eq!(with_shell_integration(&argv), argv, "{argv:?}");
        }
        // The files are there, each reading the user's own first.
        let dir = start_files().clone().unwrap();
        assert!(std::fs::read_to_string(dir.join("bashrc")).unwrap().starts_with("[ -f ~/.bashrc ]"));
        assert!(std::fs::read_to_string(dir.join("zdotdir/.zshrc")).unwrap().contains("keepane-prompt"));
    }

    #[test]
    fn shells_are_named_and_found() {
        assert_eq!(named("zsh"), vec!["zsh"]);
        assert_eq!(named(""), vec![default_shell()]);
        assert!(which("sh").is_some());
        assert!(which("/bin/sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
        assert!(takes_commands("bash") && takes_commands("zsh") && !takes_commands("sh"));
    }
}
