//! The shell a pane starts with, and keepane's hook in it: PowerShell (pwsh,
//! else Windows PowerShell), with the prompt hook that reports the directory,
//! each command's times and the prompt itself.

use std::path::PathBuf;

/// `default-shell`'s default.
pub const DEFAULT_SHELL: &str = "pwsh";

/// `default-shell`'s default, as the platforms give it.
pub fn default_shell() -> String {
    DEFAULT_SHELL.into()
}

/// The hook, for `keepane __shell-hook`.
pub use self::POWERSHELL_PROMPT_HOOK as PROMPT_HOOK;
/// ConPTY sets up the terminal its programs see: `TERM` is left as it is.
pub const SETS_TERM: bool = false;
/// `agent-commands`'s default: what an agent may start in a pane it makes.
pub const DEFAULT_AGENT_COMMANDS: &str = "pwsh powershell claude codex";
/// The variable that tells a pane's shell its own history file (read by the hook).
pub const HISTORY_VAR: &str = "KEEPANE_SHELL_HISTORY";

/// The command line for a shell named in `default-shell`.
pub fn named(s: &str) -> Vec<String> {
    match s.to_ascii_lowercase().as_str() {
        "pwsh" | "pwsh.exe" => {
            if which("pwsh.exe").is_some() {
                vec!["pwsh.exe".into(), "-NoLogo".into()]
            } else {
                vec!["powershell.exe".into(), "-NoLogo".into()]
            }
        }
        "powershell" | "powershell.exe" => vec!["powershell.exe".into(), "-NoLogo".into()],
        "wsl" | "wsl.exe" => vec!["wsl.exe".into()],
        "cmd" | "cmd.exe" => vec!["cmd.exe".into()],
        _ => vec![s.to_string()],
    }
}

/// Whether a pane running the program `stem` is a shell keepane hands
/// commands to (its hook says when it is at a prompt).
pub fn takes_commands(stem: &str) -> bool {
    matches!(stem, "pwsh" | "powershell")
}

/// PSReadLine's default history file, shared by every PowerShell of the user:
/// where a pane with no history of its own yet starts from.
pub fn shared_history() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("APPDATA")?)
        .join(r"Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt");
    p.is_file().then_some(p)
}
/// The prompt hook keepane gives PowerShell: whatever the prompt was, plus an
/// invisible OSC 9;9 with the current directory after it, so `#{pane_
/// current_path}`, `split-window -c '#{pane_current_path}'` and the saved
/// session follow `cd` without anyone editing a profile. `Set-Location`
/// does not move the process's directory, so nothing else can know it.
///
/// It also says when the last command ran and whether it failed (`OSC
/// 7777;keepane-cmd;start;end;ok`, the times from the shell's own history,
/// once per history entry), before the prompt, and where the prompt ends
/// (`OSC 133;B`), after it: `pane-timestamps` and `list-marks` read them.
/// Last, at every prompt, keepane's own word that the shell is at it
/// (`OSC 7777;keepane-prompt`): what a pane in `shell` work mode waits for
/// before the next message goes in, and which a remote shell never sends.
/// `$global:?` is read first, before anything else can change it.
pub const POWERSHELL_PROMPT_HOOK: &str = "if ($env:KEEPANE_SHELL_HISTORY -and (Get-Command Set-PSReadLineOption -ErrorAction Ignore)) { \
     Set-PSReadLineOption -HistorySavePath $env:KEEPANE_SHELL_HISTORY }; \
     $global:__keepane_prompt = $function:prompt; \
     $global:__keepane_hid = (Get-History -Count 1).Id; \
     function global:prompt { \
     $__ok = $global:?; \
     $__h = Get-History -Count 1; $__c = ''; \
     if ($__h -and $__h.Id -ne $global:__keepane_hid) { $global:__keepane_hid = $__h.Id; \
     $__c = [char]27 + ']7777;keepane-cmd;' + ([DateTimeOffset]$__h.StartExecutionTime).ToUnixTimeMilliseconds() + ';' \
     + ([DateTimeOffset]$__h.EndExecutionTime).ToUnixTimeMilliseconds() + ';' + [int]$__ok + [char]27 + '\\' }; \
     $__p = if ($global:__keepane_prompt) { & $global:__keepane_prompt } else { 'PS ' + $PWD.Path + '> ' }; \
     $__c + \"$__p\" + [char]27 + ']9;9;' + $PWD.ProviderPath + [char]27 + '\\' + [char]27 + ']133;B' + [char]27 + '\\' \
     + [char]27 + ']7777;keepane-prompt' + [char]27 + '\\' }";

/// `argv` with keepane's shell integration added where it applies: an
/// interactive PowerShell (pwsh or Windows PowerShell) gets the prompt hook
/// through `-NoExit -Command`. A PowerShell running a command or file of
/// its own, and every other program, is left exactly as given.
pub fn with_shell_integration(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else { return Vec::new() };
    let stem = std::path::Path::new(first).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase());
    if !matches!(stem.as_deref(), Some("pwsh" | "powershell")) {
        return argv.to_vec();
    }
    // PowerShell takes any unambiguous prefix of a parameter name.
    let runs_its_own = argv[1..].iter().any(|a| {
        let a = a.to_ascii_lowercase();
        let a = a.strip_prefix('-').or_else(|| a.strip_prefix('/')).unwrap_or("");
        !a.is_empty()
            && ("command".starts_with(a)
                || "file".starts_with(a)
                || "encodedcommand".starts_with(a)
                || a == "ec"
                || "noexit".starts_with(a) && a.len() >= 3)
    });
    if runs_its_own {
        return argv.to_vec();
    }
    let mut run = argv.to_vec();
    run.push("-NoExit".into());
    run.push("-Command".into());
    run.push(POWERSHELL_PROMPT_HOOK.into());
    run
}

/// Minimal PATH lookup for an executable name.
pub fn which(name: &str) -> Option<PathBuf> {
    let p = std::path::Path::new(name);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
        .split(';')
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let has_ext = exts.iter().any(|e| name.to_ascii_lowercase().ends_with(e));
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if has_ext && cand.is_file() {
            return Some(cand);
        }
        if !has_ext {
            for e in &exts {
                let c = dir.join(format!("{name}{e}"));
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Options, resolve_shell};

    #[test]
    fn shell_integration_goes_to_interactive_powershell_only() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // pwsh and Windows PowerShell, by any path or case, get the hook
        // after their own arguments.
        for argv in
            [s(&["pwsh.exe", "-NoLogo"]), s(&["C:\\Program Files\\PowerShell\\7\\pwsh.exe"]), s(&["POWERSHELL"])]
        {
            let run = with_shell_integration(&argv);
            assert_eq!(&run[..argv.len()], &argv[..]);
            assert_eq!(&run[argv.len()..], &s(&["-NoExit", "-Command", POWERSHELL_PROMPT_HOOK])[..], "{argv:?}");
        }
        // One running its own command, file or encoded command is left
        // alone, with any prefix PowerShell itself accepts.
        for argv in [
            s(&["pwsh", "-c", "Get-Date"]),
            s(&["pwsh", "-Command", "Get-Date"]),
            s(&["pwsh", "-NoLogo", "-File", "x.ps1"]),
            s(&["pwsh", "-f", "x.ps1"]),
            s(&["pwsh", "-e", "ZQBj"]),
            s(&["pwsh", "-EncodedCommand", "ZQBj"]),
            s(&["pwsh", "-NoExit", "-c", "1"]),
        ] {
            assert_eq!(with_shell_integration(&argv), argv, "{argv:?}");
        }
        // -ExecutionPolicy is not -EncodedCommand; -NoLogo is not -NoExit.
        assert_eq!(with_shell_integration(&s(&["pwsh", "-ExecutionPolicy", "Bypass", "-NoLogo"])).len(), 7);
        // Everything else is untouched.
        for argv in [s(&["cmd.exe", "/q"]), s(&["wsl.exe"]), s(&["C:\\tools\\pwshell.exe"]), Vec::new()] {
            assert_eq!(with_shell_integration(&argv), argv);
        }
        // The hook is one PowerShell statement list with balanced braces,
        // single quotes only where it must quote, and an OSC 9;9 in it.
        let h = POWERSHELL_PROMPT_HOOK;
        assert_eq!(h.matches('{').count(), h.matches('}').count());
        assert!(
            h.contains("']9;9;'") && h.contains("$PWD.ProviderPath") && h.contains("function global:prompt"),
            "{h}"
        );
    }

    #[test]
    fn shell_resolution() {
        let mut o = Options { default_shell: "wsl".into(), ..Default::default() };
        assert_eq!(resolve_shell(&o), vec!["wsl.exe"]);
        o.default_shell = "cmd".into();
        assert_eq!(resolve_shell(&o), vec!["cmd.exe"]);
        o.default_command = vec!["nu.exe".into()];
        assert_eq!(resolve_shell(&o), vec!["nu.exe"]);
        assert!(which("cmd.exe").is_some());
        assert!(which("cmd").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}
