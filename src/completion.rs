//! `keepane completion <shell>`: a completer for the shell, printed as a
//! script to load.
//!
//! bash, zsh and fish get a few lines that hand the words on the command
//! line to `keepane __complete` and show what it prints: what to offer is
//! worked out here, once, from the parser's own tables and the running
//! server (`complete`). PowerShell's is a script of its own
//! (`Register-ArgumentCompleter`) carrying the same tables.

use anyhow::{Result, bail};

/// Commands the client handles itself, which `list-commands` does not show.
pub(crate) const LOCAL: &[&str] = &[
    "completion",
    "import-config",
    "man",
    "mcp",
    "migrate",
    "restart-server",
    "setup",
    "show-keys",
    "startup",
    "update",
    "view",
    "web",
    "windows-terminal",
];

/// The flags of the commands the client handles itself.
const LOCAL_FLAGS: &[(&str, &[&str])] = &[
    ("import-config", &["-n", "-o"]),
    ("man", &["--roff"]),
    ("update", &["--check"]),
    ("web", &["--bind", "--keep-key", "--port", "--read-only"]),
];

/// Values `completion` takes: the shells there is a script for.
const SHELLS: &[&str] = &["bash", "fish", "powershell", "zsh"];

/// What to offer for the word being typed (`current`) after the words
/// before it (`words`, the program's name left out), as the shell should
/// show them: those that begin with `current`. `socket` is the server a
/// command goes to without `-L` (the pane's own, inside one);
/// `targets(socket)` lists a server's sessions and windows, `sockets()`
/// the running servers.
pub fn complete(
    words: &[String],
    current: &str,
    socket: &str,
    targets: &dyn Fn(&str) -> Vec<String>,
    sockets: &dyn Fn() -> Vec<String>,
) -> Vec<String> {
    // Global flags come first: -L socket, -f config, others alone.
    let mut socket = socket.to_string();
    let mut i = 0;
    while i < words.len() && words[i].starts_with('-') {
        match words[i].as_str() {
            "-L" | "-f" if i + 1 < words.len() => {
                if words[i] == "-L" {
                    socket = words[i + 1].clone();
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    let rest = &words[i..];
    let prev = words.last().map(String::as_str).unwrap_or_default();
    let commands = || {
        let mut c: Vec<&str> = crate::command::COMMANDS.iter().chain(LOCAL).copied().collect();
        c.sort_unstable();
        c.into_iter().map(str::to_string).collect::<Vec<_>>()
    };
    let owned = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let offered: Vec<String> = if rest.is_empty() {
        if prev == "-L" { sockets() } else { commands() }
    } else {
        let canon = canonical(&rest[0]);
        if canon == "completion" {
            owned(SHELLS)
        } else if (canon == "set-option" || canon == "show-options") && prev != "-t" && !current.starts_with('-') {
            // The option's name, then its value when it is one of a few;
            // flags are skipped, and so is the word after -t.
            let mut positional = Vec::new();
            let mut j = 1;
            while j < rest.len() {
                if rest[j] == "-t" {
                    j += 1;
                } else if !rest[j].starts_with('-') {
                    positional.push(rest[j].as_str());
                }
                j += 1;
            }
            match positional.as_slice() {
                [] => owned(crate::config::KNOWN),
                [name] => owned(crate::config::option_values(name)),
                _ => Vec::new(),
            }
        } else if prev == "-t" || (prev == "-s" && canon != "new-session") {
            // -s is a source target (join-pane, move-window), except that
            // new-session's is the new session's name.
            targets(&socket)
        } else if current.starts_with('-') {
            let flags = crate::command::FLAGS.iter().chain(LOCAL_FLAGS).find(|(c, _)| *c == canon);
            flags.map(|(_, f)| owned(f)).unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    offered.into_iter().filter(|c| c.starts_with(current)).collect()
}

/// A command as the server reads it: an alias (`splitw`, `set`) or an
/// unambiguous prefix (`split-w`) stands for its full name.
fn canonical(word: &str) -> String {
    let alias = |w: &str| crate::command::ALIASES.iter().find(|(a, _)| *a == w).map(|(_, c)| c.to_string());
    if let Some(c) = alias(word) {
        return c;
    }
    let all = || crate::command::COMMANDS.iter().chain(LOCAL);
    if all().any(|c| *c == word) {
        return word.to_string();
    }
    let hits: Vec<&&str> = all().filter(|c| c.starts_with(word)).collect();
    match hits.as_slice() {
        [one] => alias(one).unwrap_or_else(|| one.to_string()),
        _ => word.to_string(),
    }
}

/// The running server's sessions, and each window as `session:index` and
/// `session:name`, asked of that server by this same program.
fn server_targets(socket: &str) -> Vec<String> {
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    let ask = |args: &[&str]| {
        std::process::Command::new(&exe)
            .args(["-L", socket])
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let sessions: Vec<String> =
        ask(&["list-sessions"]).lines().filter_map(|l| l.split_once(':').map(|(s, _)| s.to_string())).collect();
    let mut out = sessions.clone();
    for s in &sessions {
        for l in ask(&["list-windows", "-t", s]).lines() {
            // "0: name* (2 panes) ..."
            let Some((index, rest)) = l.split_once(": ") else { continue };
            let name = rest.split(' ').next().unwrap_or_default().trim_end_matches(['*', '-']);
            out.push(format!("{s}:{index}"));
            if !name.is_empty() {
                out.push(format!("{s}:{name}"));
            }
        }
    }
    out
}

/// `keepane __complete words... current`: what `complete` offers, a line
/// each, for the shell scripts below.
pub fn run_complete(args: &[String]) -> Result<i32> {
    let (current, words) = args.split_last().map_or(("", &[][..]), |(c, w)| (c.as_str(), w));
    let socket = std::env::var("KEEPANE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into());
    for c in complete(words, current, &socket, &server_targets, &crate::platform::ipc::sockets) {
        println!("{c}");
    }
    Ok(0)
}

/// bash (3.2, macOS's own, too). The words are split from the line up to
/// the cursor, not taken from `COMP_WORDS`, which breaks `session:window`
/// at the colon; what bash replaces then is only what follows it.
pub fn bash() -> String {
    r#"# keepane completion for bash. Generated by `keepane completion bash`.
# Load it from ~/.bashrc:  eval "$(keepane completion bash)"
_keepane() {
  local line="${COMP_LINE:0:COMP_POINT}" IFS=$' \t\n'
  local -a words
  read -r -a words <<< "$line"
  case "$line" in *[[:space:]]) words+=("") ;; esac
  local n=${#words[@]}
  local cur="${words[n-1]}"
  local IFS=$'\n'
  COMPREPLY=($(keepane __complete "${words[@]:1:n-2}" "$cur" 2>/dev/null))
  if [[ "$cur" == *:* && "$COMP_WORDBREAKS" == *:* ]]; then
    local colon="${cur%"${cur##*:}"}"
    COMPREPLY=("${COMPREPLY[@]#"$colon"}")
  fi
}
complete -F _keepane keepane
"#
    .to_string()
}

/// zsh: loads as a completion function file (`_keepane` on `$fpath`, as
/// Homebrew installs it) or sourced.
pub fn zsh() -> String {
    r#"#compdef keepane
# keepane completion for zsh. Generated by `keepane completion zsh`.
# Load it from ~/.zshrc (after compinit):  source <(keepane completion zsh)
_keepane() {
  local -a offered
  offered=(${(f)"$(keepane __complete "${(@)words[2,CURRENT-1]}" "${words[CURRENT]}" 2>/dev/null)"})
  compadd -- "${offered[@]}"
}
if [ "$funcstack[1]" = "_keepane" ]; then
  _keepane "$@"
else
  compdef _keepane keepane
fi
"#
    .to_string()
}

/// fish.
pub fn fish() -> String {
    r#"# keepane completion for fish. Generated by `keepane completion fish`.
# Load it:  keepane completion fish > ~/.config/fish/completions/keepane.fish
complete -c keepane -f -a '(keepane __complete (commandline -opc)[2..-1] (commandline -ct))'
"#
    .to_string()
}

/// The PowerShell completer, ready to `Invoke-Expression`.
pub fn powershell() -> String {
    let mut commands: Vec<&str> = crate::command::COMMANDS.iter().copied().chain(LOCAL.iter().copied()).collect();
    commands.sort_unstable();
    let command_list = commands.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(",");
    let flag_table = crate::command::FLAGS
        .iter()
        .filter(|(_, f)| !f.is_empty())
        .map(|(c, f)| format!("  '{c}' = @({})", f.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(",")))
        .collect::<Vec<_>>()
        .join("\n");
    let quoted = |items: &[&str]| items.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(",");
    // PowerShell hashtables ignore case, so only one of `-V` / `-v` style
    // pairs could be a key; the aliases have none.
    let alias_table = crate::command::ALIASES
        .iter()
        .filter(|(a, _)| !a.starts_with('-'))
        .map(|(a, c)| format!("  '{a}' = '{c}'"))
        .collect::<Vec<_>>()
        .join("\n");
    let option_list = quoted(crate::config::KNOWN);
    let shells = quoted(SHELLS);
    let value_table = crate::config::KNOWN
        .iter()
        .filter(|o| !crate::config::option_values(o).is_empty())
        .map(|o| format!("  '{o}' = @({})", quoted(crate::config::option_values(o))))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"# keepane completion for PowerShell. Generated by `keepane completion powershell`.
# Completes command names, each command's flags, `-t` targets from the
# running server, option names and values after `set` / `show`, and the
# values of `-L` (sockets) and `completion`. Registered for `tmux` too, for
# those who alias it to keepane.
Register-ArgumentCompleter -Native -CommandName keepane,tmux -ScriptBlock {{
  param($wordToComplete, $commandAst, $cursorPosition)
  $commands = @({command_list})
  $flags = @{{
{flag_table}
  }}
  $options = @({option_list})
  $values = @{{
{value_table}
  }}
  $aliases = @{{
{alias_table}
  }}
  $words = @($commandAst.CommandElements | ForEach-Object {{ $_.Extent.Text }})
  # The words before the cursor, the program's own name dropped.
  $before = @($words | Select-Object -Skip 1)
  $typing = $wordToComplete -ne ''
  if ($typing -and $before.Count -gt 0) {{ $before = @($before | Select-Object -SkipLast 1) }}
  # Global flags come first: -L socket, -f config, -q.
  $socket = 'default'
  $i = 0
  while ($i -lt $before.Count -and $before[$i] -like '-*') {{
    if ($before[$i] -eq '-L' -and $i + 1 -lt $before.Count) {{ $socket = $before[$i + 1]; $i += 2 }}
    elseif ($before[$i] -eq '-f' -and $i + 1 -lt $before.Count) {{ $i += 2 }}
    else {{ $i += 1 }}
  }}
  $rest = @($before | Select-Object -Skip $i)
  $prev = if ($before.Count -gt 0) {{ $before[-1] }} else {{ '' }}
  # The command as the server reads it: an alias (`splitw`, `set`) or an
  # unambiguous prefix (`split-w`) stands for its full name.
  $canon = ''
  if ($rest.Count -gt 0) {{
    $canon = $rest[0]
    if (-not $aliases.ContainsKey($canon) -and $commands -notcontains $canon) {{
      $hits = @($commands | Where-Object {{ $_ -like "$canon*" }})
      if ($hits.Count -eq 1) {{ $canon = $hits[0] }}
    }}
    if ($aliases.ContainsKey($canon)) {{ $canon = $aliases[$canon] }}
  }}
  $out = @()
  if ($rest.Count -eq 0) {{
    if ($prev -eq '-L') {{
      # Sockets: the named pipes of running servers.
      $out = Get-ChildItem '\\.\pipe\' -ErrorAction SilentlyContinue | ForEach-Object {{ $_.Name }} |
        Where-Object {{ $_ -match '^keepane-[^-]+-(.+)$' }} | ForEach-Object {{ $Matches[1] }} | Sort-Object -Unique
    }} else {{
      $out = $commands
    }}
  }} elseif ($rest[0] -eq 'completion') {{
    $out = @({shells})
  }} elseif ($canon -in @('set-option','show-options') -and $prev -ne '-t' -and $wordToComplete -notlike '-*') {{
    # After set / show: the option's name, then its value when it is one
    # of a few. Flags are skipped, and so is the word after -t.
    $positional = @()
    for ($j = 1; $j -lt $rest.Count; $j++) {{
      if ($rest[$j] -eq '-t') {{ $j++ }} elseif ($rest[$j] -notlike '-*') {{ $positional += $rest[$j] }}
    }}
    if ($positional.Count -eq 0) {{ $out = $options }}
    elseif ($positional.Count -eq 1 -and $values.ContainsKey($positional[0])) {{ $out = $values[$positional[0]] }}
  }} elseif ($prev -eq '-t' -or ($prev -eq '-s' -and $canon -ne 'new-session')) {{
    # Targets: session names and session:window from the server.
    $sessions = & keepane -L $socket list-sessions 2>$null | ForEach-Object {{ if ($_ -match '^([^:]+):') {{ $Matches[1] }} }}
    $out = @($sessions)
    foreach ($s in $sessions) {{
      $out += & keepane -L $socket list-windows -t $s 2>$null | ForEach-Object {{ if ($_ -match '^(\d+): (\S+?)[*-]? ') {{ "${{s}}:$($Matches[1])"; "${{s}}:$($Matches[2])" }} }}
    }}
  }} elseif ($wordToComplete -like '-*') {{
    # Flags only when one is being typed: after a flag that takes a value
    # (or a positional), nothing is known to offer.
    $out = $flags[$canon]
  }}
  $out | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
  }}
}}
"#
    )
}

/// `keepane completion <shell>`: print the completer for that shell.
pub fn run(args: &[String]) -> Result<i32> {
    let script = match args.first().map(String::as_str) {
        Some("powershell" | "pwsh") => Some(powershell()),
        Some("bash") => Some(bash()),
        Some("zsh") => Some(zsh()),
        Some("fish") => Some(fish()),
        _ => None,
    };
    match args {
        [_] if script.is_some() => {
            print!("{}", script.unwrap_or_default());
            Ok(0)
        }
        [shell] => bail!("completion: no script for '{shell}' (one of {})", SHELLS.join(", ")),
        [] => bail!("completion: which shell? (one of {})", SHELLS.join(", ")),
        _ => bail!("completion: one shell name, nothing else"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The script carries what the parser knows: every command, every
    /// flagged command's flags, and nothing that would break a here-string.
    #[test]
    fn the_script_lists_every_command_and_flag() {
        let s = powershell();
        assert!(s.starts_with("# keepane completion"));
        assert!(s.contains("Register-ArgumentCompleter -Native -CommandName keepane"));
        for c in crate::command::COMMANDS.iter().chain(LOCAL) {
            assert!(s.contains(&format!("'{c}'")), "{c} is not offered");
        }
        for (c, flags) in crate::command::FLAGS {
            if flags.is_empty() {
                continue;
            }
            let line =
                s.lines().find(|l| l.trim_start().starts_with(&format!("'{c}' = @("))).unwrap_or_else(|| panic!("{c}"));
            for f in *flags {
                assert!(line.contains(&format!("'{f}'")), "{c} lacks {f}: {line}");
            }
        }
        // Balanced: what Invoke-Expression needs before anything else.
        assert_eq!(s.matches('{').count(), s.matches('}').count(), "braces");
        assert_eq!(s.matches('(').count(), s.matches(')').count(), "parens");
    }

    /// What each shell is offered, for the words already there: the same
    /// answers the PowerShell script gives, from the same tables.
    #[test]
    fn what_is_offered_follows_the_words_before_it() {
        let targets = |socket: &str| vec![format!("{socket}-s"), format!("{socket}-s:0"), "work:1".to_string()];
        let sockets = || vec!["default".to_string(), "lab".to_string()];
        let offer = |words: &[&str], current: &str| {
            let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            complete(&words, current, "here", &targets, &sockets)
        };
        let all = offer(&[], "");
        for c in crate::command::COMMANDS.iter().chain(LOCAL) {
            assert!(all.iter().any(|a| a == c), "{c} is not offered");
        }
        assert_eq!(offer(&[], "attach-s"), ["attach-session"]);
        assert_eq!(offer(&[], "import"), ["import-config"]);
        // -L: the running servers; after it, that server's targets.
        assert_eq!(offer(&["-L"], ""), ["default", "lab"]);
        assert_eq!(offer(&["-L", "lab", "attach", "-t"], ""), ["lab-s", "lab-s:0", "work:1"]);
        // Without -L, the server a command goes to by itself.
        assert_eq!(offer(&["attach", "-t"], "here-s:"), ["here-s:0"]);
        assert_eq!(offer(&["switchc", "-t"], "w"), ["work:1"]);
        // Flags, by alias or unambiguous prefix, and the client's own.
        let split = crate::command::FLAGS.iter().find(|(c, _)| *c == "split-window").unwrap().1;
        assert_eq!(offer(&["splitw"], "-"), split);
        assert_eq!(offer(&["split-w"], "-"), split);
        assert_eq!(offer(&["import-config"], "-"), ["-n", "-o"]);
        assert_eq!(offer(&["update"], "--"), ["--check"]);
        // Options, then their values; flags and -t's word do not count.
        assert_eq!(offer(&["set", "-g"], "mou"), ["mouse"]);
        assert_eq!(offer(&["set", "-g", "mouse"], ""), ["off", "on"]);
        assert_eq!(offer(&["set", "-t", "x"], "mou"), ["mouse"]);
        assert!(offer(&["set", "-g", "mouse", "on"], "").is_empty());
        assert_eq!(offer(&["completion"], ""), SHELLS);
        // -s names a source, except new-session's, which names the new one.
        assert_eq!(offer(&["join-pane", "-s"], "w"), ["work:1"]);
        // Nothing to offer is nothing, not everything.
        assert!(offer(&["new", "-s"], "").is_empty());
        assert!(offer(&["no-such-command"], "-").is_empty());
    }

    /// The bash script runs on macOS's bash 3.2: no negative subscripts, no
    /// `mapfile`, no `${var,,}`; zsh's loads both as a file and sourced.
    #[test]
    fn the_small_scripts_ask_keepane_and_stay_portable() {
        for s in [bash(), zsh(), fish()] {
            assert!(s.contains("keepane __complete"), "{s}");
            assert!(!s.contains("tmux"), "tmux's own completion is tmux's: {s}");
        }
        let b = bash();
        for newer in ["[-1]", "mapfile", "readarray", ",,}", "^^}"] {
            assert!(!b.contains(newer), "bash 3.2 has no {newer}");
        }
        assert!(b.contains("complete -F _keepane keepane"));
        let z = zsh();
        assert!(z.starts_with("#compdef keepane\n"), "an autoloadable file begins so");
        assert!(z.contains("compdef _keepane keepane"));
    }

    #[test]
    fn only_shells_with_a_script_are_taken() {
        assert_eq!(run(&["powershell".into()]).unwrap(), 0);
        assert_eq!(run(&["pwsh".into()]).unwrap(), 0);
        for s in ["bash", "zsh", "fish"] {
            assert_eq!(run(&[s.into()]).unwrap(), 0, "{s}");
        }
        assert!(run(&["tcsh".into()]).unwrap_err().to_string().contains("no script for 'tcsh'"));
        assert!(run(&[]).unwrap_err().to_string().contains("which shell"));
        assert!(run(&["a".into(), "b".into()]).is_err());
    }
}
