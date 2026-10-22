//! The shell a pane starts with, and keepane's hook in it: bash and zsh get
//! a start file of keepane's that first reads the user's own (`~/.bashrc`,
//! `$ZDOTDIR/.zshrc`), then the hook (`crate::shellhook`).

use std::path::{Path, PathBuf};

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
    matches!(stem, "bash" | "zsh" | "fish")
}

/// The history file a pane with no history of its own yet starts from.
pub fn shared_history() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [".bash_history", ".zsh_history"].iter().map(|f| home.join(f)).find(|p| p.is_file())
}

/// The bash hook, for `keepane __shell-hook`.
pub use crate::shellhook::BASH_HOOK as PROMPT_HOOK;
use crate::shellhook::start_files;

/// Whether a shell `argv` starts keeps a history file of its own (its hook
/// reads `KEEPANE_SHELL_HISTORY`): a bash or zsh that gets the hook. fish
/// keeps its own (it names a session, not a file).
pub fn keeps_history(argv: &[String]) -> bool {
    let stem = argv.first().and_then(|f| Path::new(f).file_name()).map(|s| s.to_string_lossy().into_owned());
    matches!(stem.as_deref(), Some("bash" | "zsh")) && with_shell_integration(argv) != argv
}

/// Whether `argv` is an interactive shell of its own (no command, no
/// script): only options follow the program.
fn interactive(argv: &[String]) -> bool {
    argv[1..].iter().all(|a| matches!(a.as_str(), "-i" | "-l" | "--login" | "--noprofile"))
}

/// `argv` with keepane's shell integration where it applies: an interactive
/// bash, zsh or fish. Anything else, and a shell running a command or a
/// script, is left exactly as given.
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
        "fish" => {
            // After the user's own config, keepane's hook.
            let hook = dir.join("keepane.fish").to_string_lossy().replace('\\', r"\\").replace('\'', r"\'");
            let mut run = argv.to_vec();
            run.extend(["--init-command".into(), format!("source '{hook}'")]);
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
        for argv in [s(&["bash", "-c", "ls"]), s(&["bash", "x.sh"]), s(&["sh"]), s(&["fish", "x.fish"]), Vec::new()] {
            assert_eq!(with_shell_integration(&argv), argv, "{argv:?}");
        }
        // fish: its own arguments, then keepane's hook after its config.
        let f = with_shell_integration(&s(&["/usr/bin/fish", "-l"]));
        assert_eq!(&f[..3], &s(&["/usr/bin/fish", "-l", "--init-command"])[..]);
        assert!(f[3].starts_with("source '") && f[3].ends_with("keepane.fish'"), "{f:?}");
        assert!(keeps_history(&s(&["bash"])) && !keeps_history(&s(&["fish"])) && !keeps_history(&s(&["sh"])));
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
        assert!(takes_commands("bash") && takes_commands("zsh") && takes_commands("fish") && !takes_commands("sh"));
    }
}
