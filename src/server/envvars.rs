//! The environment `set-environment` gives every pane. A new pane gets the
//! table when it starts. A shell already at its prompt takes each change at
//! its next prompt, through keepane's prompt hook reading a script written
//! here, so what it runs from then on has it; a program already running
//! keeps the environment it started with, as no process can change
//! another's. The table outlives the server: it is saved beside the
//! sessions, and read back at start after the config (which wins).

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::render::Style;
use super::{Outcome, PaneId, Server};
use crate::format::Segment;
use vt100::Color;

/// What the scripts tell the shells: the last change to each name.
pub(super) struct EnvState {
    /// The number of the last change. A pane starts knowing the changes up
    /// to it (`KEEPANE_ENV_SEQ`). It starts at the server's start time in
    /// milliseconds, so what a script left by an earlier server says is
    /// older than any pane of this one.
    seq: u64,
    /// Each name's last change: its number, and the value (none: unset).
    changes: Vec<(String, u64, Option<String>)>,
    /// Where the scripts go: every place a pane was told of (the sessions
    /// directory can change while panes run).
    stems: RefCell<Vec<PathBuf>>,
    /// The saved table has been read back. Until then (the config at
    /// start) nothing is saved over it.
    loaded: bool,
}

impl Default for EnvState {
    fn default() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        EnvState { seq: now.as_millis() as u64, changes: Vec::new(), stems: RefCell::new(Vec::new()), loaded: false }
    }
}

/// Whether two names are one variable: on Windows any case is.
pub(super) fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// `stem` and an extension after it (a socket name may have dots of its own).
fn with_ext(stem: &Path, ext: &str) -> PathBuf {
    let mut s = OsString::from(stem.as_os_str());
    s.push(ext);
    PathBuf::from(s)
}

/// The PowerShell script: each change a shell has not had yet.
fn ps_script(changes: &[(String, u64, Option<String>)], seq: u64) -> String {
    // In single quotes PowerShell ends a string at any of these, so each is
    // doubled.
    let quote = |s: &str| {
        let mut q = String::from("'");
        for c in s.chars() {
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                q.push(c);
            }
            q.push(c);
        }
        q.push('\'');
        q
    };
    let mut out = String::from(
        "# keepane: set-environment's changes, read by the prompt hook. Do not edit.\n\
         $__keepane_s = [int64]0; [void][int64]::TryParse(\"$env:KEEPANE_ENV_SEQ\", [ref]$__keepane_s)\n",
    );
    for (name, n, value) in changes {
        let what = match value {
            Some(v) => {
                let name: String = name
                    .chars()
                    .flat_map(|c| if matches!(c, '{' | '}' | '`') { vec!['`', c] } else { vec![c] })
                    .collect();
                format!("${{env:{name}}} = {}", quote(v))
            }
            None => format!("Remove-Item -LiteralPath {} -ErrorAction Ignore", quote(&format!("Env:{name}"))),
        };
        out.push_str(&format!("if ($__keepane_s -lt {n}) {{ {what} }}\n"));
    }
    out.push_str(&format!("if ($__keepane_s -lt {seq}) {{ $env:KEEPANE_ENV_SEQ = '{seq}' }}\n"));
    out.push_str("Remove-Variable __keepane_s\n");
    out
}

/// The POSIX shell script (bash, zsh): the same. A name a shell cannot
/// export is left out.
fn sh_script(changes: &[(String, u64, Option<String>)], seq: u64) -> String {
    let mut out = String::from(
        "# keepane: set-environment's changes, read by the prompt hook. Do not edit.\n\
         __keepane_s=${KEEPANE_ENV_SEQ:-0}\n\
         case $__keepane_s in ''|*[!0-9]*) __keepane_s=0 ;; esac\n",
    );
    for (name, n, value) in changes {
        let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            out.push_str(&format!("# {}: not a name a shell can export\n", name.replace('\n', " ")));
            continue;
        }
        let what = match value {
            Some(v) => format!("export {name}='{}'", v.replace('\'', "'\\''")),
            None => format!("unset {name}"),
        };
        out.push_str(&format!("if [ \"$__keepane_s\" -lt {n} ]; then {what}; fi\n"));
    }
    out.push_str(&format!("if [ \"$__keepane_s\" -lt {seq} ]; then export KEEPANE_ENV_SEQ={seq}; fi\n"));
    out.push_str("unset __keepane_s\n");
    out
}

/// Write `text` to `path` whole: through a file beside it, renamed over
/// it, so a shell never reads half of it; straight in when the rename is
/// refused (a shell holding the old one open).
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = with_ext(path, ".tmp");
    std::fs::write(&tmp, text)?;
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, text)?;
    }
    Ok(())
}

impl Server {
    /// `set-environment`: `value` none unsets the name.
    pub(super) fn set_environment(&mut self, name: String, value: Option<String>) {
        self.env.retain(|(k, _)| !same_name(k, &name));
        if let Some(v) = &value {
            self.env.push((name.clone(), v.clone()));
        }
        let st = &mut self.env_state;
        st.seq += 1;
        st.changes.retain(|(k, ..)| !same_name(k, &name));
        st.changes.push((name, st.seq, value));
        self.write_env_scripts();
        self.save_environment();
    }

    /// What a pane is told besides the table: where its shell's hook reads
    /// the changes, and the last one it has had.
    pub(super) fn env_hook_vars(&self) -> [(String, String); 2] {
        let stem = self.env_stem();
        {
            let mut stems = self.env_state.stems.borrow_mut();
            if !stems.contains(&stem) {
                stems.push(stem.clone());
            }
        }
        [
            ("KEEPANE_ENV_FILE".into(), stem.to_string_lossy().into_owned()),
            ("KEEPANE_ENV_SEQ".into(), self.env_state.seq.to_string()),
        ]
    }

    /// This server's environment files, without their extensions: a name a
    /// socket, in the sessions directory, so servers side by side (`-L`)
    /// keep their own.
    fn env_stem(&self) -> PathBuf {
        self.sessions_dir().join("environment").join(crate::histlog::safe_name(&self.socket))
    }

    fn write_env_scripts(&self) {
        let st = &self.env_state;
        let (ps, sh) = (ps_script(&st.changes, st.seq), sh_script(&st.changes, st.seq));
        for stem in st.stems.borrow().iter() {
            if let Some(dir) = stem.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            for (ext, text) in [(".ps1", &ps), (".sh", &sh)] {
                if let Err(e) = write_whole(&with_ext(stem, ext), text) {
                    log::warn!("set-environment: {}: {e}", with_ext(stem, ext).display());
                }
            }
        }
    }

    /// Save the table (once it has been read back at start).
    fn save_environment(&self) {
        if !self.env_state.loaded {
            return;
        }
        let path = with_ext(&self.env_stem(), ".json");
        let json = serde_json::json!({ "vars": self.env });
        let saved =
            path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| write_whole(&path, &json.to_string()));
        if let Err(e) = saved {
            log::warn!("set-environment: saving {}: {e}", path.display());
        }
    }

    /// At start, after the config: the saved table, each name the config
    /// did not set itself. The scripts an earlier server left go: their
    /// changes are older than any pane to come, but only while the clock
    /// never went back.
    pub(super) fn load_environment(&mut self) {
        for ext in [".ps1", ".sh"] {
            let _ = std::fs::remove_file(with_ext(&self.env_stem(), ext));
        }
        let path = with_ext(&self.env_stem(), ".json");
        let saved: Vec<(String, String)> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v["vars"].clone()).ok())
            .unwrap_or_default();
        for (k, v) in saved {
            if !self.env.iter().any(|(n, _)| same_name(n, &k)) {
                self.env.push((k, v));
            }
        }
        self.env_state.loaded = true;
        self.save_environment();
    }

    /// `show-environment` for an attached client: the table and what
    /// keepane sets in every pane besides, with the values pane `pid` gets,
    /// coloured, as an overlay.
    pub(super) fn environment_overlay(&self, pid: PaneId) -> (Vec<String>, Vec<Vec<Segment>>) {
        let head = Style { bold: true, ..Style::colors(Color::Idx(3), Color::Default) };
        let name = Style { bold: true, ..Style::colors(Color::Idx(6), Color::Default) };
        let faint = Style::colors(Color::Idx(8), Color::Default);
        let plain = Style::default();
        let mut lines: Vec<Vec<Segment>> = Vec::new();
        let seg = |text: &str, style: Style| Segment { text: text.to_string(), style };
        let var = |k: &str, v: &str, note: &str| {
            let mut l = vec![seg("  ", plain), seg(k, name), seg("=", faint), seg(v, plain)];
            if !note.is_empty() {
                l.push(seg(&format!("   {note}"), faint));
            }
            l
        };
        lines.push(vec![seg("set-environment", head)]);
        lines.push(vec![seg(
            "  every new pane gets these; a shell at its prompt takes a change at its next one",
            faint,
        )]);
        if self.env.is_empty() {
            lines.push(vec![seg("  (none: set-environment NAME value adds one)", faint)]);
        }
        for (k, v) in &self.env {
            lines.push(var(k, v, ""));
        }
        lines.push(Vec::new());
        lines.push(vec![seg("keepane sets in every pane", head)]);
        for (k, v) in self.keepane_env(pid) {
            let note = match k.as_str() {
                "KEEPANE" => "this server (its -L name)",
                "KEEPANE_PANE" => "the pane's own id",
                "PATH" => "keepane's folder in it",
                "TERM" => "default-terminal",
                "KEEPANE_ENV_FILE" => "where its shell reads set-environment's changes",
                "KEEPANE_ENV_SEQ" => "the last change its shell has had",
                _ => "",
            };
            let overridden = self.env.iter().any(|(n, _)| same_name(n, &k));
            lines.push(var(&k, &v, if overridden { "(set-environment's value wins)" } else { note }));
        }
        let text = lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>()).collect();
        (text, lines)
    }

    /// `show-environment`: the table as `NAME=value` lines, or one name.
    pub(super) fn show_environment(&self, name: Option<&str>) -> Outcome {
        let lines: Vec<String> = match name {
            Some(n) => match self.env.iter().find(|(k, _)| same_name(k, n)) {
                Some((k, v)) => vec![format!("{k}={v}")],
                None => return Outcome::Error(format!("unknown variable: {n}")),
            },
            None => self.env.iter().map(|(k, v)| format!("{k}={v}")).collect(),
        };
        if lines.is_empty() {
            return Outcome::Text("no variables set".into());
        }
        Outcome::Text(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(list: &[(&str, u64, Option<&str>)]) -> Vec<(String, u64, Option<String>)> {
        list.iter().map(|(k, n, v)| (k.to_string(), *n, v.map(String::from))).collect()
    }

    /// Each change goes in guarded by its number, the values quoted so the
    /// shell takes them as they are; the last number goes to the shell.
    #[test]
    fn the_scripts_quote_and_number_each_change() {
        let c = changes(&[("FOO", 5, Some("it's $HOME `x`")), ("GONE", 7, None), ("weird name", 8, Some("v"))]);
        let sh = sh_script(&c, 9);
        assert!(sh.contains("if [ \"$__keepane_s\" -lt 5 ]; then export FOO='it'\\''s $HOME `x`'; fi\n"), "{sh}");
        assert!(sh.contains("if [ \"$__keepane_s\" -lt 7 ]; then unset GONE; fi\n"), "{sh}");
        assert!(sh.contains("# weird name: not a name a shell can export\n"), "{sh}");
        assert!(sh.contains("then export KEEPANE_ENV_SEQ=9; fi\n"), "{sh}");
        let ps = ps_script(&c, 9);
        assert!(ps.contains("if ($__keepane_s -lt 5) { ${env:FOO} = 'it''s $HOME `x`' }\n"), "{ps}");
        assert!(
            ps.contains("if ($__keepane_s -lt 7) { Remove-Item -LiteralPath 'Env:GONE' -ErrorAction Ignore }\n"),
            "{ps}"
        );
        assert!(ps.contains("${env:weird name} = 'v'"), "{ps}");
        assert!(ps.contains("{ $env:KEEPANE_ENV_SEQ = '9' }\n"), "{ps}");
        // PowerShell's other single quotes are doubled too; braces in a name escaped.
        let ps = ps_script(&changes(&[("A}B", 1, Some("x\u{2019}y"))]), 1);
        assert!(ps.contains("${env:A`}B} = 'x\u{2019}\u{2019}y'"), "{ps}");
    }

    /// Run by a real shell: a shell that knows change 5 takes 6 and 7 only.
    #[test]
    fn a_shell_takes_only_the_changes_it_has_not_had() {
        let c = changes(&[("KP_OLD", 5, Some("old")), ("KP_NEW", 6, Some("a b'c")), ("KP_GONE", 7, None)]);
        let dir = std::env::temp_dir().join(format!("keepane-env-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (out, code) = if cfg!(windows) {
            let script = dir.join("env.ps1");
            std::fs::write(&script, ps_script(&c, 7)).unwrap();
            let run = format!(
                "$env:KEEPANE_ENV_SEQ='5'; $env:KP_GONE='here'; . ([ScriptBlock]::Create([IO.File]::ReadAllText('{}'))); \
                 \"[$env:KP_OLD][$env:KP_NEW][$env:KP_GONE][$env:KEEPANE_ENV_SEQ]\"",
                script.display()
            );
            super::super::run_shell_blocking(&run, &[], None)
        } else {
            let script = dir.join("env.sh");
            std::fs::write(&script, sh_script(&c, 7)).unwrap();
            let run = format!(
                "KEEPANE_ENV_SEQ=5; KP_GONE=here; export KP_GONE; . '{}'; printf '[%s][%s][%s][%s]' \"$KP_OLD\" \"$KP_NEW\" \"$KP_GONE\" \"$KEEPANE_ENV_SEQ\"",
                script.display()
            );
            super::super::run_shell_blocking(&run, &[], None)
        };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0, "{out}");
        assert_eq!(out.trim(), "[][a b'c][][7]");
    }

    fn server(name: &str, dir: &Path) -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, name.into(), events);
        s.opts.sessions_dir = dir.to_string_lossy().into_owned();
        s
    }

    fn set(s: &mut Server, line: &str) {
        let cmd = crate::command::parse_line(line).unwrap().unwrap();
        assert!(matches!(s.exec(cmd, None), Outcome::Ok), "{line}");
    }

    /// The table is saved as it changes, and a new server reads it back
    /// after its config, the config's value winning; nothing the config
    /// sets at start is saved over the table before that. A pane told of
    /// the scripts finds each change in them, numbered after its own.
    #[test]
    fn the_table_is_saved_and_read_back_after_the_config() {
        let dir = std::env::temp_dir().join(format!("keepane-envsave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = server("env.sock", &dir);
        s.load_environment();
        let told = s.keepane_env(5);
        let seq: u64 = told.iter().find(|(k, _)| k == "KEEPANE_ENV_SEQ").unwrap().1.parse().unwrap();
        let file = told.iter().find(|(k, _)| k == "KEEPANE_ENV_FILE").unwrap().1.clone();
        assert_eq!(PathBuf::from(&file), dir.join("environment").join("env.sock"));
        set(&mut s, "set-environment A one");
        set(&mut s, "set-environment B two");
        set(&mut s, "set-environment -u B");
        set(&mut s, "set-environment C three");
        let sh = std::fs::read_to_string(format!("{file}.sh")).unwrap();
        assert!(sh.contains(&format!("-lt {} ]; then export A='one'", seq + 1)), "{sh}");
        assert!(sh.contains(&format!("-lt {} ]; then unset B", seq + 3)), "{sh}");
        assert!(!sh.contains("two"), "only each name's last change: {sh}");
        let ps = std::fs::read_to_string(format!("{file}.ps1")).unwrap();
        assert!(ps.contains("${env:C} = 'three'"), "{ps}");
        let saved = std::fs::read_to_string(dir.join("environment").join("env.sock.json")).unwrap();
        assert_eq!(saved, r#"{"vars":[["A","one"],["C","three"]]}"#);

        // The next server: its config sets C, which wins, and adds D. The
        // scripts the last one left are gone once it has read the table.
        let mut t = server("env.sock", &dir);
        assert!(Path::new(&format!("{file}.sh")).exists() && Path::new(&format!("{file}.ps1")).exists());
        set(&mut t, "set-environment C from-config");
        set(&mut t, "set-environment D four");
        let before = std::fs::read_to_string(dir.join("environment").join("env.sock.json")).unwrap();
        assert_eq!(before, saved, "the config at start saves nothing over it");
        t.load_environment();
        assert!(!Path::new(&format!("{file}.sh")).exists() && !Path::new(&format!("{file}.ps1")).exists());
        let got: Vec<String> = t.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(got, ["C=from-config", "D=four", "A=one"]);
        let after = std::fs::read_to_string(dir.join("environment").join("env.sock.json")).unwrap();
        assert!(after.contains(r#"["A","one"]"#) && after.contains(r#"["C","from-config"]"#), "{after}");
        // -u unsets, a value after it or not.
        set(&mut t, "set-environment -u D ignored");
        assert!(!t.env.iter().any(|(k, _)| k == "D"), "{:?}", t.env);
        // A server of another name keeps its own.
        let mut u = server("other", &dir);
        u.load_environment();
        assert!(u.env.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What `prefix e` shows: the table, then what keepane sets, each name
    /// in its colour; a name the table sets over keepane's says so.
    #[test]
    fn the_overlay_shows_both_in_colour() {
        let dir = std::env::temp_dir().join(format!("keepane-envshow-{}", std::process::id()));
        let mut s = server("show", &dir);
        let (lines, styles) = s.environment_overlay(3);
        assert!(lines.contains(&"  (none: set-environment NAME value adds one)".to_string()), "{lines:?}");
        set(&mut s, "set-environment EDITOR vim");
        set(&mut s, "set-environment PATH /only/here");
        let (lines, styles2) = s.environment_overlay(3);
        assert_eq!(lines.len(), styles2.len());
        assert!(styles.len() < styles2.len());
        let at = |l: &str| lines.iter().position(|x| x.starts_with(l)).unwrap_or_else(|| panic!("{l}: {lines:?}"));
        assert!(at("set-environment") < at("  EDITOR=vim") && at("  EDITOR=vim") < at("keepane sets in every pane"));
        assert_eq!(lines[at("  KEEPANE_PANE=")], "  KEEPANE_PANE=3   the pane's own id");
        let keepanes_path = lines.iter().rposition(|x| x.starts_with("  PATH=")).unwrap();
        assert!(keepanes_path > at("keepane sets in every pane"));
        assert!(lines[keepanes_path].ends_with("(set-environment's value wins)"), "{lines:?}");
        assert_eq!(lines[at("  PATH=")], "  PATH=/only/here", "the table's own, first");
        let editor = &styles2[at("  EDITOR=vim")];
        assert_eq!(editor[1].text, "EDITOR");
        assert_eq!(editor[1].style.fg, Color::Idx(6));
        assert!(styles2[at("set-environment")][0].style.bold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_one_variable_in_any_case_on_windows_only() {
        assert!(same_name("PATH", "PATH"));
        assert_eq!(same_name("Path", "PATH"), cfg!(windows));
        assert!(!same_name("PATH", "PATHS"));
        assert_eq!(with_ext(Path::new("a/sock.v1"), ".ps1"), PathBuf::from("a/sock.v1.ps1"));
    }
}
