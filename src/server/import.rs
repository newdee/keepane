//! `keepane import-config [-n] [-o file] [file]`: bring what keepane can use
//! from another config (a `.tmux.conf`, an old `.wmux.conf`, anyone's) into
//! keepane's own, once, when asked. keepane reads only its own config; this
//! is how a tmux one comes over.
//!
//! Each command is tried on a server of its own that nothing connects to,
//! so what is imported is exactly what keepane would take at start. Only
//! settings come over (options, key bindings, hooks, the environment);
//! commands that run programs are neither imported nor run, and tmux
//! plugins (TPM, `@` options) stay tmux's. `%if` blocks keep their shape,
//! every branch checked. Skipped lines go in too, commented out, with why.

use super::{Event, Outcome, PaneEvent, Server};
use crate::command::Cmd;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The line the imported block begins with; also how a second import of
/// the same file is caught.
const MARK: &str = "# keepane import-config: from ";

pub fn run(args: &[String]) -> Result<i32> {
    let (mut dry, mut out, mut from) = (false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-n" => dry = true,
            "-o" => out = Some(PathBuf::from(it.next().context("import-config: -o takes a file")?)),
            f if f.starts_with('-') && f.len() > 1 => bail!("import-config: unknown flag {f} (-n, -o file)"),
            f if from.is_none() => from = Some(PathBuf::from(f)),
            _ => bail!("import-config: one file at a time"),
        }
    }
    let from = match from {
        Some(f) => expand(&f),
        None => tmux_config().context(
            "import-config: no tmux config here (~/.tmux.conf, ~/.config/tmux/tmux.conf); name the file to import",
        )?,
    };
    let to = match out {
        Some(o) => expand(&o),
        None => keepane_config(),
    };
    let text = std::fs::read_to_string(&from).with_context(|| format!("import-config: {}", from.display()))?;
    if text.trim_start_matches('\u{feff}').trim().is_empty() {
        println!("{} is empty: nothing to import", from.display());
        return Ok(0);
    }
    if same_file(&from, &to) {
        bail!("import-config: {} is the config keepane reads already; -o says where to import it", from.display());
    }
    let existing = std::fs::read_to_string(&to).unwrap_or_default();
    let mark = format!("{MARK}{}", from.display());
    if existing.lines().any(|l| l == mark) {
        bail!(
            "import-config: {} was imported into {} already (the block beginning `{mark}`); edit it there",
            from.display(),
            to.display()
        );
    }
    let report = import(&text)?;
    let block = format!("{mark}\n{}", report.text);
    let skipped = report.skipped.len();
    if dry {
        print!("{block}");
        eprintln!(
            "import-config -n: {} lines would go into {} ({skipped} of them commented out, why above each)",
            report.imported + skipped,
            to.display()
        );
        return Ok(0);
    }
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str(&block);
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("import-config: {}", dir.display()))?;
    }
    std::fs::write(&to, body).with_context(|| format!("import-config: {}", to.display()))?;
    println!("{} -> {}: {} imported, {skipped} skipped", from.display(), to.display(), report.imported);
    for (n, line, why) in &report.skipped {
        println!("  {n}: {line}  ({why})");
    }
    println!(
        "A running server read its config when it started: `keepane source-file {}` applies this now.",
        to.display()
    );
    Ok(0)
}

/// What an import wrote, and what it left out (line number, line, why).
#[derive(Debug)]
pub struct Imported {
    pub text: String,
    pub imported: usize,
    pub skipped: Vec<(usize, String, String)>,
}

/// The config `text`, as keepane's own: every line that keepane takes as
/// it is, every other one commented out under the reason.
pub fn import(text: &str) -> Result<Imported> {
    let (pane_tx, _pane_rx) = std::sync::mpsc::channel::<PaneEvent>();
    let (events, _events_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let mut server = Server::new(pane_tx, "import-config".into(), events);
    let mut out = Imported { text: String::new(), imported: 0, skipped: Vec::new() };
    let mut open_ifs = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let n = i + 1;
        // A logical line: `\` at the end continues it on the next.
        let mut raw = vec![lines[i]];
        while raw.last().is_some_and(|l| l.ends_with('\\')) && i + 1 < lines.len() {
            i += 1;
            raw.push(lines[i]);
        }
        i += 1;
        let trimmed = raw[0].trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            push_lines(&mut out.text, &raw);
            continue;
        }
        if let Some(word) = trimmed.strip_prefix('%').map(|r| r.split_whitespace().next().unwrap_or_default()) {
            match word {
                "if" => open_ifs.push(n),
                "elif" | "else" if !open_ifs.is_empty() => {}
                "endif" if open_ifs.pop().is_some() => {}
                "elif" | "else" | "endif" => bail!("import-config: line {n}: %{word} without %if"),
                _ => {
                    skip(&mut out, n, &raw, "tmux's own directive, not keepane's");
                    continue;
                }
            }
            push_lines(&mut out.text, &raw);
            continue;
        }
        let joined: String = raw.iter().map(|l| l.strip_suffix('\\').unwrap_or(l)).collect();
        match check(&mut server, &joined) {
            Ok(()) => {
                push_lines(&mut out.text, &raw);
                out.imported += 1;
            }
            Err(why) => skip(&mut out, n, &raw, &why),
        }
    }
    if let Some(n) = open_ifs.last() {
        bail!("import-config: the %if at line {n} is never closed (%endif); fix that first");
    }
    Ok(out)
}

/// Whether keepane takes `line` as a setting, and why not.
fn check(server: &mut Server, line: &str) -> Result<(), String> {
    let cmd = crate::command::parse_line(line)?.ok_or("empty")?;
    match &cmd {
        Cmd::SetOption { name, .. } if name == "@plugin" => {
            return Err("a tmux plugin (TPM): tmux's plugins do not run in keepane".into());
        }
        Cmd::SetOption { name, .. } if name.starts_with('@') => {
            return Err("a tmux plugin's setting".into());
        }
        Cmd::SetOption { name, .. } => {
            let full = crate::config::resolve_name(name)?;
            if crate::config::ACCEPTED.contains(&full.as_str()) {
                return Err(format!("{full}: tmux's; keepane has no use for it"));
            }
        }
        Cmd::BindKey { key, .. } | Cmd::UnbindKey { key, .. } if super::is_mouse_key(key) => {
            return Err(format!("{key}: a mouse key; keepane's mouse handling is fixed"));
        }
        Cmd::BindKey { .. } | Cmd::UnbindKey { .. } | Cmd::SetHook { .. } | Cmd::SetEnvironment { .. } => {}
        Cmd::RunShell { .. } | Cmd::IfShell { .. } => {
            return Err("runs a program; an import runs nothing".into());
        }
        Cmd::SourceFile { .. } => {
            return Err("reads another file: import that one on its own".into());
        }
        _ => return Err("not a setting".into()),
    }
    match server.exec(cmd, None) {
        Outcome::Error(e) => Err(e),
        _ => Ok(()),
    }
}

fn push_lines(out: &mut String, raw: &[&str]) {
    for l in raw {
        out.push_str(l);
        out.push('\n');
    }
}

fn skip(out: &mut Imported, n: usize, raw: &[&str], why: &str) {
    // One line of reason: an error message may run over several.
    let why = why.lines().next().unwrap_or_default().to_string();
    out.text.push_str(&format!("# skipped, {why}:\n"));
    for l in raw {
        out.text.push_str(&format!("# {l}\n"));
    }
    out.skipped.push((n, raw.join(" ").trim().to_string(), why));
}

fn expand(p: &Path) -> PathBuf {
    PathBuf::from(super::expand_home(&p.to_string_lossy()))
}

/// The tmux config on this machine, where tmux looks for one.
pub(super) fn tmux_config() -> Option<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".tmux.conf"));
        v.push(home.join(".config").join("tmux").join("tmux.conf"));
    }
    if let Some(cfg) = dirs::config_dir() {
        v.push(cfg.join("tmux").join("tmux.conf"));
    }
    v.into_iter().find(|p| p.is_file())
}

/// The config keepane reads (an old `.wmux.conf` included: a new
/// `.keepane.conf` next to it would hide it), else a new `~/.keepane.conf`.
fn keepane_config() -> PathBuf {
    crate::config::find_config()
        .or_else(|| {
            crate::config::config_paths().into_iter().find(|p| p.file_name().is_some_and(|f| f == ".keepane.conf"))
        })
        .unwrap_or_else(|| PathBuf::from(".keepane.conf"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config as people have them: what keepane takes comes over as it
    /// was written; TPM, plugin settings, options keepane has no use for,
    /// commands that run programs and typos are commented out, each with
    /// why; the %if blocks keep their shape, both branches checked.
    #[test]
    fn a_tmux_conf_comes_over_with_the_rest_commented_out() {
        let conf = "# my tmux.conf\n\
                    set -g prefix C-a\n\
                    unbind C-b\n\
                    set -g mouse on\n\
                    setw -g mode-keys vi\n\
                    set -ga terminal-overrides \",xterm-256color:Tc\"\n\
                    bind -T copy-mode-vi v send-keys -X begin-selection\n\
                    bind | split-window -h \\\n  -c \"#{pane_current_path}\"\n\
                    \n\
                    %if #{==:#{host},nowhere}\n\
                    set -g status off\n\
                    %else\n\
                    set -g display-tim 4321\n\
                    %endif\n\
                    set -g @plugin 'tmux-plugins/tpm'\n\
                    set -g @resurrect-strategy-vim 'session'\n\
                    set -g no-such-option 1\n\
                    source-file ~/.tmux.local\n\
                    run '~/.tmux/plugins/tpm/tpm'\n";
        let r = import(conf).unwrap();
        let want = "# my tmux.conf\n\
                    set -g prefix C-a\n\
                    unbind C-b\n\
                    set -g mouse on\n\
                    # skipped, mode-keys: tmux's; keepane has no use for it:\n\
                    # setw -g mode-keys vi\n\
                    # skipped, terminal-overrides: tmux's; keepane has no use for it:\n\
                    # set -ga terminal-overrides \",xterm-256color:Tc\"\n\
                    bind -T copy-mode-vi v send-keys -X begin-selection\n\
                    bind | split-window -h \\\n  -c \"#{pane_current_path}\"\n\
                    \n\
                    %if #{==:#{host},nowhere}\n\
                    set -g status off\n\
                    %else\n\
                    set -g display-tim 4321\n\
                    %endif\n\
                    # skipped, a tmux plugin (TPM): tmux's plugins do not run in keepane:\n\
                    # set -g @plugin 'tmux-plugins/tpm'\n\
                    # skipped, a tmux plugin's setting:\n\
                    # set -g @resurrect-strategy-vim 'session'\n";
        assert!(r.text.starts_with(want), "{}", r.text);
        let rest = &r.text[want.len()..];
        let rest: Vec<&str> = rest.lines().collect();
        assert!(rest[0].starts_with("# skipped, ") && rest[0].contains("no-such-option"), "{rest:?}");
        assert_eq!(rest[1], "# set -g no-such-option 1");
        assert_eq!(rest[2], "# skipped, reads another file: import that one on its own:");
        assert_eq!(rest[3], "# source-file ~/.tmux.local");
        assert_eq!(rest[4], "# skipped, runs a program; an import runs nothing:");
        assert_eq!(rest[5], "# run '~/.tmux/plugins/tpm/tpm'");
        assert_eq!(rest.len(), 6);
        assert_eq!(r.imported, 7);
        let lines: Vec<usize> = r.skipped.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(lines, vec![5, 6, 16, 17, 18, 19, 20]);
        // The same text, the same result.
        assert_eq!(import(conf).unwrap().text, r.text);
    }

    /// What keepane would read is what it reads back: the imported text
    /// loads with no errors, and a skipped line is nowhere but a comment.
    #[test]
    fn what_is_imported_loads_cleanly() {
        let r = import(
            "set -g base-index 1\nbind h select-pane -L\nset -g bogus 1\nnew-session -d\n\
             bind -n MouseDragEnd1Pane send -X copy-selection\nunbind -n WheelUpPane\n",
        )
        .unwrap();
        assert_eq!(r.imported, 2);
        // Taken by keepane and doing nothing there: not worth a line.
        assert!(r.skipped.iter().any(|(n, _, w)| *n == 5 && w.starts_with("MouseDragEnd1Pane: a mouse key")));
        assert!(r.skipped.iter().any(|(n, _, w)| *n == 6 && w.starts_with("WheelUpPane: a mouse key")));
        let (pane_tx, _p) = std::sync::mpsc::channel::<PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        for line in r.text.lines() {
            if let Some(cmd) = crate::command::parse_line(line).unwrap() {
                assert!(!matches!(s.exec(cmd, None), Outcome::Error(_)), "{line}");
            }
        }
        assert!(r.skipped.iter().any(|(_, l, w)| l == "new-session -d" && w == "not a setting"));
    }

    #[test]
    fn a_broken_if_block_is_refused() {
        assert!(import("%if 1\nset -g a 1\n").unwrap_err().to_string().contains("line 1 is never closed"));
        assert!(import("%endif\n").unwrap_err().to_string().contains("line 1: %endif without %if"));
        // Notepad's BOM and CRLF endings come out as lines.
        let r = import("\u{feff}set -g base-index 1\r\nset -g mouse on\r\n").unwrap();
        assert_eq!(r.text, "set -g base-index 1\nset -g mouse on\n");
    }
}
