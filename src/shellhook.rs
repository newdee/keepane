//! keepane's prompt hooks for the POSIX shells, and the start files that
//! give them to a shell keepane starts: bash and zsh on Linux and macOS,
//! and the user's own shell in WSL on Windows (`wslstart.sh`). Each hook
//! reports the directory (OSC 7), each command's start and end with its
//! status (OSC 133; fish 4 says those itself), and that a prompt is up
//! (`OSC 7777;keepane-prompt;sh`, `;fish` from fish's,
//! `;sh` saying commands are typed to it in POSIX syntax: once a prompt,
//! from `PROMPT_COMMAND` / `precmd`, never in PS1, which the line editor
//! draws again as a line wraps or text is typed ahead); it keeps the
//! pane's own history file (`KEEPANE_SHELL_HISTORY`), written command by
//! command, and takes `set-environment`'s changes at each prompt.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The bash hook, for `keepane __shell-hook` (to `eval` in a shell keepane
/// did not start).
pub const BASH_HOOK: &str = r#"
if [ -n "$KEEPANE_SHELL_HISTORY" ]; then HISTFILE="$KEEPANE_SHELL_HISTORY"; history -c; history -r; fi
__keepane_status() { __keepane_ok=$?; }
__keepane_prompt() {
  history -a
  # set-environment's changes this shell has not had yet.
  if [ -n "$KEEPANE_ENV_FILE" ] && [ -r "$KEEPANE_ENV_FILE.sh" ]; then . "$KEEPANE_ENV_FILE.sh"; fi
  case "$PS1" in
    *']133;B'*) ;;
    *) PS1="\[$(printf '\033]133;A\007')\]$PS1\[$(printf '\033]133;B\007')\]" ;;
  esac
  # keepane's word that a prompt is up: once a prompt, from here, not in
  # PS1, which readline draws again (a wrapped line, text typed ahead).
  printf '\e]133;D;%s\e\\\e]7;file://%s%s\e\\\e]7777;keepane-prompt;sh\a' "$__keepane_ok" "${HOSTNAME:-localhost}" "$PWD"
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
  # set-environment's changes this shell has not had yet.
  if [ -n "$KEEPANE_ENV_FILE" ] && [ -r "$KEEPANE_ENV_FILE.sh" ]; then . "$KEEPANE_ENV_FILE.sh"; fi
  case "$PS1" in
    *$'\e]133;B'*) ;;
    *) PS1="%{"$'\e]133;A\a'"%}$PS1%{"$'\e]133;B\a'"%}" ;;
  esac
  # Once a prompt, not in PS1, which zle draws again.
  printf '\e]133;D;%s\e\\\e]7;file://%s%s\e\\\e]7777;keepane-prompt;sh\a' "$ok" "${HOST:-localhost}" "$PWD"
}
__keepane_preexec() { printf '\e]133;C\e\\'; }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __keepane_precmd
add-zsh-hook preexec __keepane_preexec
"#;

/// The fish hook, sourced by `fish --init-command` after the user's own
/// config. fish 4 marks its prompts and commands itself (OSC 133 A, C, D);
/// an older one gets them here. Every fish gets where its prompt ends (B),
/// the directory (OSC 7), keepane's word that a prompt is up (`;fish`: what
/// it is typed to in), and `set-environment`'s changes.
const FISH_HOOK: &str = r#"# keepane's prompt hook for fish.
set -g __keepane_old (string match -qr '^[0-3]\.' -- $version; and echo 1)
function __keepane_postexec --on-event fish_postexec
    set -l ok $status
    test -n "$__keepane_old"; and printf '\e]133;D;%s\a' $ok
end
function __keepane_preexec --on-event fish_preexec
    test -n "$__keepane_old"; and printf '\e]133;C\a'
end
function __keepane_prompt --on-event fish_prompt
    # set-environment's changes this shell has not had yet.
    if set -q KEEPANE_ENV_FILE; and test -r "$KEEPANE_ENV_FILE.fish"
        source "$KEEPANE_ENV_FILE.fish"
    end
    printf '\e]7;file://%s%s\a\e]7777;keepane-prompt;fish\a' (prompt_hostname) $PWD
end
if functions -q fish_prompt
    functions -c fish_prompt __keepane_user_prompt
    function fish_prompt
        test -n "$__keepane_old"; and printf '\e]133;A\a'
        __keepane_user_prompt
        printf '\e]133;B\a'
    end
end
"#;

/// Bash in WSL: what a login shell reads (which `wsl.exe` alone starts),
/// then the hook.
const WSL_BASHRC: &str = r#"[ -f /etc/profile ] && . /etc/profile
if [ -f ~/.bash_profile ]; then . ~/.bash_profile
elif [ -f ~/.bash_login ]; then . ~/.bash_login
elif [ -f ~/.profile ]; then . ~/.profile
fi
"#;

/// Run by `sh` in WSL (`KEEPANE_WSL_START`, a path WSL translated): the
/// user's own login shell, with keepane's hook where keepane has one. Only
/// finding the shell is left to `sh`: the user's start files are their
/// shell's to read (a `~/.profile` written for bash may not parse in dash).
const WSL_START: &str = r#"# keepane: the WSL user's own shell, with keepane's prompt hook.
d=${KEEPANE_WSL_START%/*}
s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
if [ -z "$s" ] || [ ! -x "$s" ]; then s=/bin/sh; fi
case ${s##*/} in
  bash) exec "$s" --rcfile "$d/wslbashrc" -i ;;
  zsh)
    export KEEPANE_ZDOTDIR="${ZDOTDIR:-$HOME}"
    export ZDOTDIR="$d/zdotdir"
    exec "$s" -l ;;
  fish) exec "$s" -l --init-command "source '$d/keepane.fish'" ;;
  *) exec "$s" -l ;;
esac
"#;

/// `text` with `\n` line ends: a checkout with CRLF ones (Git's
/// `core.autocrlf` on Windows) puts `\r` in the texts above, which WSL's
/// shells would take as part of a word.
fn lf(text: &str) -> String {
    text.replace('\r', "")
}

/// keepane's start files, written once per process under its data
/// directory: `bashrc`, `zdotdir/.zshrc` (with the other zsh start files
/// passing through to the user's), and for WSL `wslbashrc` and
/// `wslstart.sh`. Written with `\n` line ends, which WSL's shells need.
pub fn start_files() -> &'static Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = crate::logger::log_dir().join("shell");
        let z = dir.join("zdotdir");
        std::fs::create_dir_all(&z).ok()?;
        let write = |f: PathBuf, text: String| std::fs::write(f, lf(&text)).ok();
        write(dir.join("bashrc"), format!("[ -f ~/.bashrc ] && . ~/.bashrc\n{BASH_HOOK}"))?;
        write(dir.join("wslbashrc"), format!("{WSL_BASHRC}{BASH_HOOK}"))?;
        write(dir.join("wslstart.sh"), WSL_START.to_string())?;
        write(dir.join("keepane.fish"), FISH_HOOK.to_string())?;
        // zsh reads these from $ZDOTDIR; each reads the user's own first.
        let user = r#"${KEEPANE_ZDOTDIR:-$HOME}"#;
        for f in [".zshenv", ".zprofile", ".zlogin"] {
            write(z.join(f), format!("[ -f \"{user}/{f}\" ] && . \"{user}/{f}\"\n"))?;
        }
        write(
            z.join(".zshrc"),
            format!("ZDOTDIR=\"{user}\"\n[ -f \"$ZDOTDIR/.zshrc\" ] && . \"$ZDOTDIR/.zshrc\"\n{ZSH_HOOK}"),
        )?;
        Some(dir)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files are there, each reading the user's own first, with no
    /// `\r` in them (WSL's shells take a `\r` as part of a word).
    #[test]
    fn the_start_files_read_the_users_own_first() {
        let dir = start_files().clone().unwrap();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
        assert!(read("bashrc").starts_with("[ -f ~/.bashrc ]"));
        assert!(read("zdotdir/.zshrc").contains("keepane-prompt"));
        let wsl = read("wslbashrc");
        assert!(wsl.starts_with("[ -f /etc/profile ]") && wsl.contains("~/.profile") && wsl.contains("keepane-prompt"));
        assert!(read("wslstart.sh").contains("--rcfile \"$d/wslbashrc\""));
        assert!(read("keepane.fish").contains("keepane-prompt;fish"));
        for f in ["bashrc", "wslbashrc", "wslstart.sh", "keepane.fish", "zdotdir/.zshrc", "zdotdir/.zprofile"] {
            assert!(!read(f).contains('\r'), "{f}");
        }
        assert_eq!(lf("a\r\nb\r\n"), "a\nb\n", "as a CRLF checkout has them");
    }
}
