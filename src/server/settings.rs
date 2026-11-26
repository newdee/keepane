//! The settings board's side of the server (docs/design/settings.md): who
//! set each option last, the settings file (run after the config, written
//! by `save-settings`), `show-settings`, `reload-settings`, and `set -u`.
//!
//! The settings file is the board's own: the config file the person wrote
//! is never written to, so its comments, `%if` blocks and order stay theirs.

use super::Binding;
use super::{Outcome, Server, expand_home};
use crate::command::{Cmd, parse_line, quote};
use crate::config::{ACCEPTED, Options, PERSONS, SHOWABLE};
use crate::keys::Key;
use std::path::{Path, PathBuf};

/// The first line of a settings file the board starts.
const HEADER: &str = "# keepane's settings board writes this file (keepane settings, prefix g); it is run after the config file. Edit it by hand too, then press R on the board.";

/// `path` with the home directory as `~`: how a source or the settings file
/// is shown.
pub(super) fn shown_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref())
            && (rest.is_empty() || rest.starts_with(['/', '\\']))
        {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// The option a config line sets (`set -g NAME …`, `set -gu NAME`), if it
/// sets one; `set -a` adds to it rather than saying it, and is not one.
fn sets(line: &str) -> Option<String> {
    match parse_line(line) {
        Ok(Some(Cmd::SetOption { name, append: false, .. } | Cmd::UnsetOption { name })) => Some(name),
        _ => None,
    }
}

/// Options whose line must come before another's in a settings file: the
/// second reads the first as it is set.
const READ_BY: &[(&str, &str)] = &[("theme-path", "theme")];

/// `old` (a settings file) with each `(name, line)` in it: the first line
/// that sets the option replaced, any later one taken out (it would win
/// over the one written), else the line added at the end. Every other line
/// stays as it was. With the line number (from 1) each option is on now.
pub(super) fn merge(old: &str, updates: &[(String, String)]) -> (String, Vec<(String, usize)>) {
    let mut lines: Vec<String> = old.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        lines = vec![HEADER.to_string()];
    }
    for (name, new) in updates {
        let mut found = false;
        let mut i = 0;
        while i < lines.len() {
            // A line continued (`\` at its end) is not read alone.
            let continued = i > 0 && lines[i - 1].ends_with('\\');
            if !continued && !lines[i].ends_with('\\') && sets(&lines[i]).as_deref() == Some(name) {
                if found {
                    lines.remove(i);
                    continue;
                }
                lines[i] = new.clone();
                found = true;
            }
            i += 1;
        }
        if !found {
            lines.push(new.clone());
        }
    }
    // A line another reads goes before it, wherever each was: `theme NAME`
    // looks for NAME.conf in `theme-path` as it runs.
    for (first, then) in READ_BY {
        let at = |lines: &[String], n: &str| {
            lines.iter().position(|l| sets(l).and_then(|s| crate::config::resolve_name(&s).ok()).as_deref() == Some(n))
        };
        if let (Some(f), Some(t)) = (at(&lines, first), at(&lines, then))
            && f > t
        {
            let line = lines.remove(f);
            lines.insert(t, line);
        }
    }
    let at = updates
        .iter()
        .filter_map(|(name, new)| lines.iter().position(|l| l == new).map(|i| (name.clone(), i + 1)))
        .collect();
    (lines.join("\n") + "\n", at)
}

/// Whether a config line binds or unbinds a key of the prefix table (the
/// keys `save-settings -k` writes are all of these the file has).
fn prefix_key_line(line: &str) -> bool {
    use crate::command::KeyTable::Prefix;
    matches!(parse_line(line), Ok(Some(Cmd::BindKey { table: Prefix, .. } | Cmd::UnbindKey { table: Prefix, .. })))
}

/// `old` with every prefix-table key line taken out and `keys` added at the
/// end (the board's keys are the file's: one written by hand is in the
/// table, so it comes back among `keys`). Other lines stay as they were.
pub(super) fn merge_keys(old: &str, keys: &[String]) -> String {
    let lines: Vec<&str> = old.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut out: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let continued = i > 0 && lines[i - 1].ends_with('\\');
        if !continued && !l.ends_with('\\') && prefix_key_line(l) {
            continue;
        }
        out.push(l.to_string());
    }
    out.extend(keys.iter().cloned());
    if out.iter().all(|l| l.trim().is_empty()) {
        return String::new();
    }
    out.join("\n") + "\n"
}

impl Server {
    /// The prefix table's lines that make it from where the config left it
    /// (`base_binds`): a key the config had and the table has not unbound,
    /// a key bound otherwise (or new) bound; by the key's name, so the same
    /// table always writes the same lines.
    fn key_lines(&self) -> Vec<String> {
        use crate::command::KeyTable::Prefix;
        let mut keys: Vec<&Key> = self.base_binds.keys().chain(self.prefix_binds.keys()).collect();
        keys.sort_by_key(|k| k.to_string());
        keys.dedup();
        let mut out = Vec::new();
        for k in keys {
            match (self.base_binds.get(k), self.prefix_binds.get(k)) {
                (Some(_), None) => out.push(Cmd::UnbindKey { table: Prefix, key: k.to_string() }.to_string()),
                (was, Some(now)) if was != Some(now) => out.push(
                    Cmd::BindKey {
                        table: Prefix,
                        key: k.to_string(),
                        repeat: now.repeat,
                        cmd: Box::new(now.cmd.clone()),
                    }
                    .to_string(),
                ),
                _ => {}
            }
        }
        out
    }

    /// `rebind-key [-s] FROM TO`: what FROM runs, moved to TO. TO bound
    /// already: refused, saying to what, or with `-s` the two swapped.
    pub(super) fn rebind_key(&mut self, from: &str, to: &str, swap: bool) -> Outcome {
        let (Some(f), Some(t)) = (Key::parse(from), Key::parse(to)) else {
            return Outcome::Error(format!("rebind-key: unknown key {from} or {to}"));
        };
        if f == t {
            return Outcome::Ok;
        }
        let Some(moved) = self.prefix_binds.get(&f).cloned() else {
            return Outcome::Error(format!("rebind-key: {from} is bound to nothing"));
        };
        match self.prefix_binds.get(&t).cloned() {
            Some(there) if !swap => {
                Outcome::Error(format!("rebind-key: {to} is bound to {}: -s swaps the two", there.cmd))
            }
            Some(there) => {
                self.prefix_binds.insert(t, moved);
                self.prefix_binds.insert(f, there);
                Outcome::Ok
            }
            None => {
                self.prefix_binds.remove(&f);
                self.prefix_binds.insert(t, moved);
                Outcome::Ok
            }
        }
    }

    /// `show-bindings [-J]`: each key of the prefix table and what it runs,
    /// with the key that command had when the config had run (`base`; none
    /// for one bound since); then what the config bound and nothing runs
    /// now (no key). By the key's name.
    pub(super) fn show_bindings(&self, json: bool) -> Outcome {
        let base_of = |k: &Key, b: &Binding| -> Option<String> {
            if self.base_binds.get(k) == Some(b) {
                return Some(k.to_string());
            }
            // A command the config bound to more than one key (`Left` and
            // `h`): the key that no longer runs it is the one it came from.
            let mut found: Vec<(bool, String)> = self
                .base_binds
                .iter()
                .filter(|(_, x)| x.cmd == b.cmd)
                .map(|(k2, x)| (self.prefix_binds.get(k2) == Some(x), k2.to_string()))
                .collect();
            found.sort();
            found.into_iter().next().map(|(_, k)| k)
        };
        let mut rows: Vec<(String, String, bool, Option<String>)> = self
            .prefix_binds
            .iter()
            .map(|(k, b)| (k.to_string(), b.cmd.to_string(), b.repeat, base_of(k, b)))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut gone: Vec<(String, String, bool, Option<String>)> = self
            .base_binds
            .iter()
            .filter(|(_, b)| !self.prefix_binds.values().any(|x| x.cmd == b.cmd))
            .map(|(k, b)| (String::new(), b.cmd.to_string(), b.repeat, Some(k.to_string())))
            .collect();
        gone.sort_by(|a, b| a.3.cmp(&b.3));
        rows.extend(gone);
        if json {
            let v: Vec<serde_json::Value> = rows
                .iter()
                .map(|(k, c, r, b)| serde_json::json!({ "key": k, "command": c, "repeat": r, "base": b }))
                .collect();
            return Outcome::Text(serde_json::Value::Array(v).to_string());
        }
        Outcome::Text(
            rows.iter()
                .map(|(k, c, _, b)| match b {
                    Some(b) if b != k => format!("{:<10} {c}  # was {b}", if k.is_empty() { "-" } else { k }),
                    Some(_) => format!("{k:<10} {c}"),
                    None => format!("{k:<10} {c}  # new"),
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Where the settings file is: `settings-file`; else
    /// `~/.keepane/settings.conf`, unless the config was named
    /// (`KEEPANE_CONFIG`, `-f`), which keeps a test (or anyone who names
    /// one) apart from this machine's settings.
    pub(super) fn settings_path(&self) -> Option<PathBuf> {
        let set = self.opts.settings_file.trim();
        if !set.is_empty() {
            return Some(PathBuf::from(expand_home(set)));
        }
        if self.config_override.is_some() || std::env::var_os("KEEPANE_CONFIG").is_some() {
            return None;
        }
        dirs::home_dir().map(|h| h.join(".keepane").join("settings.conf"))
    }

    /// Run the settings file, when there is one.
    pub(super) fn source_settings_file(&mut self) -> Result<(), String> {
        match self.settings_path() {
            Some(path) if path.is_file() => self.source_file(&path.to_string_lossy()).map(|_| ()),
            _ => Ok(()),
        }
    }

    /// Why this caller may not change option `name`: it is the person's
    /// (`config::PERSONS`) and the caller an agent's pane, or a file such a
    /// pane sources.
    pub(super) fn persons_refused(&self, name: &str, cid: Option<super::ClientId>) -> Option<String> {
        if !PERSONS.contains(&name) {
            return None;
        }
        let pane = self.agent_caller(cid).or(self.agent_sourcing)?;
        Some(format!("{name} is the person's to change: an agent's pane (%{pane}) may read it, not change it"))
    }

    /// Why this caller may not write or reload the settings: an agent's
    /// pane (they are the person's).
    fn settings_refused(&self, what: &str, cid: Option<super::ClientId>) -> Option<String> {
        let pane = self.agent_caller(cid).or(self.agent_sourcing)?;
        Some(format!("{what}: the settings are the person's: an agent's pane (%{pane}) may read them (show-settings)"))
    }

    /// `source-file PATH`. When it is the config file run at the start (and
    /// not from inside another file being run), the settings file runs
    /// after it, as at the start: what the board wrote wins over the
    /// config again, whichever way the config is run.
    /// Run by an agent's pane, what the file sets is that pane's: the
    /// person's options stay as they are.
    pub(super) fn source_command(&mut self, path: &str, cid: Option<super::ClientId>) -> Outcome {
        let is_config = self.sourcing.is_empty()
            && self.config_loaded.as_ref().is_some_and(|c| same_file(c, Path::new(&expand_home(path))));
        let outer = self.agent_sourcing;
        self.agent_sourcing = outer.or(self.agent_caller(cid));
        let ran = self.source_file(path).map(|_| ());
        if is_config {
            self.base_binds = self.prefix_binds.clone();
        }
        let then = if is_config { self.source_settings_file() } else { Ok(()) };
        self.agent_sourcing = outer;
        match (ran, then) {
            (Ok(()), Ok(())) => Outcome::Ok,
            (Err(a), Err(b)) => Outcome::Error(format!("{a}\n{b}")),
            (Err(e), _) | (_, Err(e)) => Outcome::Error(e),
        }
    }

    /// `name` was set to `value`: by the config line being run, else by
    /// `set`. A theme sets its options too: those are the theme's.
    pub(super) fn option_set_by(&mut self, name: &str, value: &str) {
        let by = self.config_at.clone().unwrap_or_else(|| "set".into());
        if name == "theme" && value.trim() != "custom" {
            let theme = format!("theme {}", self.opts.theme);
            for o in crate::config::theme_options() {
                self.option_source.insert(o.to_string(), theme.clone());
            }
        }
        self.option_source.insert(name.to_string(), by);
    }

    /// `set -u NAME`: back to the default, the way `set` would put it (so
    /// whatever follows a change follows this one, and an agent's pane is
    /// refused the person's options there, with the same caller); the
    /// default is nobody's.
    pub(super) fn unset_option(&mut self, name: &str, cid: Option<super::ClientId>) -> Outcome {
        if name.starts_with('@') {
            self.opts.user.retain(|(k, _)| k != name);
            self.option_source.remove(name);
            return Outcome::Ok;
        }
        if ACCEPTED.contains(&name) {
            return Outcome::Ok;
        }
        let Some(value) = Options::default().get_to_save(name) else {
            return Outcome::Error(format!("unknown option: {name}"));
        };
        let r = self.exec(Cmd::SetOption { name: name.to_string(), value, append: false, target: None }, cid);
        if !matches!(r, Outcome::Error(_)) {
            self.option_source.remove(name);
            if name == "theme" {
                self.option_source.retain(|_, by| !by.starts_with("theme "));
            }
        }
        r
    }

    /// `show-settings [-J]`: the settings file, and each option's value,
    /// default and source. A secret shows as `(set)`, as `show-options`
    /// prints it.
    pub(super) fn show_settings(&self, json: bool) -> Outcome {
        let d = Options::default();
        let file = self.settings_path().map(|p| shown_path(&p.to_string_lossy()));
        let rows: Vec<(&str, String, String, String)> = SHOWABLE
            .iter()
            .map(|n| {
                let source = self.option_source.get(*n).cloned().unwrap_or_else(|| "default".into());
                (*n, self.opts.get(n).unwrap_or_default(), d.get(n).unwrap_or_default(), source)
            })
            .collect();
        if json {
            let options: Vec<serde_json::Value> = rows
                .iter()
                .map(|(n, v, d, s)| serde_json::json!({ "name": n, "value": v, "default": d, "source": s }))
                .collect();
            // `themes`: what `theme` takes now (`list-themes`), the user's own too.
            let themes = self.opts.theme_names();
            return Outcome::Text(
                serde_json::json!({ "file": file, "options": options, "themes": themes }).to_string(),
            );
        }
        let mut out = vec![match &file {
            Some(f) => format!("# settings file: {f}"),
            None => "# no settings file: the config was named (KEEPANE_CONFIG or -f); set settings-file in it".into(),
        }];
        out.extend(rows.iter().map(|(n, v, _, s)| format!("{n} {}  # {s}", quote(v))));
        Outcome::Text(out.join("\n"))
    }

    /// `save-settings NAME...`: each option's value on a `set -g` line of
    /// the settings file (a secret's own text: the file is kept to this
    /// user). The file and its folder are made when missing.
    pub(super) fn settings_save(&mut self, names: &[String], keys: bool, cid: Option<super::ClientId>) -> Outcome {
        if let Some(why) = self.settings_refused("save-settings", cid) {
            return Outcome::Error(why);
        }
        let Some(path) = self.settings_path() else {
            return Outcome::Error(
                "save-settings: no settings file: the config was named (KEEPANE_CONFIG or -f); set settings-file in it"
                    .into(),
            );
        };
        let mut updates = Vec::new();
        for n in names {
            let Some(v) = self.opts.get_to_save(n) else {
                return Outcome::Error(format!("save-settings: no option {n}"));
            };
            // A config line is one line: a value broken over two would be
            // two lines, the second not a command (and nothing is written).
            if v.contains(['\n', '\r', '\0']) {
                return Outcome::Error(format!(
                    "save-settings: {n}'s value has a line break, which a line of the file cannot hold; nothing written"
                ));
            }
            updates.push((n.clone(), format!("set -g {} {}", quote(n), quote(&v))));
        }
        let old = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Outcome::Error(format!("save-settings: {}: {e}", path.display())),
        };
        // The keys first (their old lines out, the table's difference at the
        // end), then the options: so each option's line number is its own.
        let key_lines = if keys { self.key_lines() } else { Vec::new() };
        // A new file says whose it is first, whatever goes in it.
        let old = if old.trim().is_empty() { format!("{HEADER}\n") } else { old };
        let old = if keys { merge_keys(&old, &key_lines) } else { old };
        let (text, at) = merge(&old, &updates);
        if let Err(e) = write_private(&path, &text) {
            return Outcome::Error(format!("save-settings: {}: {e:#}", path.display()));
        }
        let shown = shown_path(&path.to_string_lossy());
        for (n, line) in at {
            self.option_source.insert(n, format!("{shown}:{line}"));
        }
        let what = match (names.len(), keys) {
            (_, false) => crate::format::count(names.len(), "option"),
            (0, true) => crate::format::count(key_lines.len(), "key line"),
            (n, true) => {
                format!(
                    "{} and {}",
                    crate::format::count(n, "option"),
                    crate::format::count(key_lines.len(), "key line")
                )
            }
        };
        Outcome::Text(format!("saved {what} to {shown}"))
    }

    /// `reload-settings`: every option back to its default, then the config
    /// file and the settings file run again, as at the start, plugins and
    /// all; the prefix table back to its defaults first too. Other key
    /// tables, hooks and `@` options are not undone first: their lines run
    /// again, as with `source-file`.
    pub(super) fn settings_reload(&mut self, cid: Option<super::ClientId>) -> Outcome {
        if let Some(why) = self.settings_refused("reload-settings", cid) {
            return Outcome::Error(why);
        }
        let d = Options::default();
        // The theme first: put back, it sets its own options to theirs.
        let mut names: Vec<&str> = Vec::new();
        if self.opts.get("theme") != d.get("theme") {
            names.push("theme");
        }
        let mut errors = Vec::new();
        for n in names {
            if let Outcome::Error(e) = self.unset_option(n, None) {
                errors.push(format!("{n}: {e}"));
            }
        }
        let rest: Vec<&str> = SHOWABLE.iter().copied().filter(|n| self.opts.get(n) != d.get(n)).collect();
        for n in rest {
            if let Outcome::Error(e) = self.unset_option(n, None) {
                errors.push(format!("{n}: {e}"));
            }
        }
        self.option_source.retain(|k, _| k.starts_with('@'));
        // The plugins run again with their `@plugin` lines: loaded once
        // otherwise, what they set would stay at its default. Their
        // `@plugin` entries come back with the lines, not twice.
        self.plugins.clear();
        self.opts.user.retain(|(k, _)| k != "@plugin");
        // The prefix table too: the keys the board moved come back from the
        // files, not from where they were moved to.
        self.prefix_binds = super::default_bindings();
        let mut ran = Vec::new();
        if let Some(config) = self.config_loaded.clone().filter(|p| p.is_file()) {
            match self.source_file(&config.to_string_lossy()) {
                Ok(_) => ran.push(shown_path(&config.to_string_lossy())),
                Err(e) => errors.push(e),
            }
        }
        self.base_binds = self.prefix_binds.clone();
        if let Some(file) = self.settings_path().filter(|p| p.is_file()) {
            match self.source_file(&file.to_string_lossy()) {
                Ok(_) => ran.push(shown_path(&file.to_string_lossy())),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            return Outcome::Error(errors.join("\n"));
        }
        let then = if ran.is_empty() { String::new() } else { format!(", then ran {}", ran.join(" and ")) };
        Outcome::Text(format!("settings back to the defaults{then}"))
    }

    /// `settings` (prefix g): the board in a popup on the client's terminal.
    pub(super) fn settings_popup(&mut self, cid: Option<super::ClientId>) -> Outcome {
        let Some(cid) = cid else { return Outcome::Error("settings: no client".into()) };
        if self.clients.get(&cid).and_then(|c| c.session).is_none() {
            return Outcome::Error("settings: client not attached (run `keepane settings` instead)".into());
        }
        let Some(exe) = super::pane::helper_exe() else {
            return Outcome::Error("settings: no keepane.exe to run it with".into());
        };
        let cmd = Cmd::DisplayPopup {
            close: false,
            close_on_exit: true,
            width: Some("90%".into()),
            height: Some("90%".into()),
            x: None,
            y: None,
            cwd: None,
            argv: vec![
                exe.to_string_lossy().into_owned(),
                "-L".into(),
                self.socket.clone(),
                "settings".into(),
                "--popup".into(),
            ],
        };
        self.exec(cmd, Some(cid))
    }
}

/// Whether two paths are one file (as the system resolves them; as written
/// when one cannot be resolved).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// `text` written to `path` whole (a file beside it, then renamed over it),
/// its folder made, the file kept to this user: it may hold a secret.
fn write_private(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("conf.new");
    let r = std::fs::write(&tmp, text)
        .map_err(anyhow::Error::from)
        .and_then(|()| crate::platform::private::restrict(&tmp))
        .and_then(|()| std::fs::rename(&tmp, path).map_err(anyhow::Error::from));
    // Not written: the file beside it goes too.
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, l)| (n.to_string(), l.to_string())).collect()
    }

    /// A new file: a line saying whose it is, then the options.
    #[test]
    fn a_new_file_says_whose_it_is() {
        let (text, at) =
            merge("", &upd(&[("mouse", "set -g mouse off"), ("history-limit", "set -g history-limit 9000")]));
        assert_eq!(text, format!("{HEADER}\nset -g mouse off\nset -g history-limit 9000\n"));
        assert_eq!(at, vec![("mouse".into(), 2), ("history-limit".into(), 3)]);
    }

    /// An option written again replaces its line where it is; what the
    /// person wrote by hand (comments, other commands, `%if`) stays, and a
    /// later line for the same option, which would win, goes.
    #[test]
    fn a_line_is_replaced_in_place_and_the_rest_kept() {
        let old = "# mine\r\nset -g mouse on\r\nbind-key g display-message hi\r\nset-option -g hist 100\r\n%if 1\r\nset -g status off\r\n%endif\r\nset -gu mouse\r\nset -ga status-right x\r\n";
        let (text, at) =
            merge(old, &upd(&[("mouse", "set -g mouse off"), ("history-limit", "set -g history-limit 9000")]));
        assert_eq!(
            text,
            "# mine\nset -g mouse off\nbind-key g display-message hi\nset -g history-limit 9000\n%if 1\nset -g status off\n%endif\nset -ga status-right x\n"
        );
        assert_eq!(at, vec![("mouse".into(), 2), ("history-limit".into(), 4)]);
    }

    /// `theme-path` goes before `theme`, which reads it as it is set:
    /// written together in either order, or the path added to a file that
    /// has the theme already; nothing else moves, and the line numbers
    /// said are where the lines are now.
    #[test]
    fn the_theme_path_goes_before_the_theme() {
        let both = upd(&[("theme", "set -g theme mine"), ("theme-path", "set -g theme-path ~/t")]);
        let (text, at) = merge("", &both);
        assert_eq!(text, format!("{HEADER}\nset -g theme-path ~/t\nset -g theme mine\n"));
        assert_eq!(at, vec![("theme".into(), 3), ("theme-path".into(), 2)]);
        let old = "set -g mouse on\nset -g theme mine\nset -g status off\n";
        let (text, _) = merge(old, &upd(&[("theme-path", "set -g theme-path ~/t")]));
        assert_eq!(text, "set -g mouse on\nset -g theme-path ~/t\nset -g theme mine\nset -g status off\n");
        // Already in order, or only one of them: as before.
        let (text, _) = merge("set -g theme-path ~/t\nset -g mouse on\n", &upd(&[("theme", "set -g theme mine")]));
        assert_eq!(text, "set -g theme-path ~/t\nset -g mouse on\nset -g theme mine\n");
        let (text, _) = merge("set -g mouse on\n", &upd(&[("theme", "set -g theme paper")]));
        assert_eq!(text, "set -g mouse on\nset -g theme paper\n");
    }

    /// A continued line is not taken for the option it starts to set.
    #[test]
    fn a_continued_line_is_left_alone() {
        let old = "set -g status-right \\\n  \"x\"\n";
        let (text, _) = merge(old, &upd(&[("status-right", "set -g status-right y")]));
        assert_eq!(text, "set -g status-right \\\n  \"x\"\nset -g status-right y\n");
    }

    /// What is written reads back: each line sets its option to the value.
    #[test]
    fn what_is_written_reads_back() {
        for (name, value) in [
            ("status-right", "#[fg=#dd625e]x #{pane_title} \"q\" C:\\a b"),
            ("window-status-separator", ""),
            ("default-command", "pwsh -NoLogo"),
            ("@mine", "a;b"),
        ] {
            let line = format!("set -g {} {}", quote(name), quote(value));
            let (text, _) = merge("", &[(name.to_string(), line)]);
            let last = text.lines().last().unwrap();
            match parse_line(last).unwrap().unwrap() {
                Cmd::SetOption { name: n, value: v, .. } => assert_eq!((n.as_str(), v.as_str()), (name, value)),
                other => panic!("{other:?}"),
            }
        }
    }

    /// A write that fails after the file beside it was written (renaming it
    /// over a folder fails everywhere) leaves nothing beside it.
    #[test]
    fn a_write_that_fails_leaves_nothing_beside_it() {
        let dir = std::env::temp_dir().join(format!("keepane-settings-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(write_private(&dir, "set -g mouse off\n").is_err());
        assert!(!dir.with_extension("conf.new").exists(), "the file beside it is gone");
        std::fs::remove_dir_all(&dir).unwrap();
        // One that works leaves only the file.
        let ok = std::env::temp_dir().join(format!("keepane-settings-write-{}", std::process::id())).join("s.conf");
        write_private(&ok, "x\n").unwrap();
        assert_eq!(std::fs::read_to_string(&ok).unwrap(), "x\n");
        assert!(!ok.with_extension("conf.new").exists());
        std::fs::remove_dir_all(ok.parent().unwrap()).unwrap();
    }

    /// The board's key lines replace every prefix-table key line of the
    /// file; another table's keys, comments, options and continued lines
    /// stay.
    #[test]
    fn key_lines_replace_the_prefix_tables_and_nothing_else() {
        let old = "# mine\nbind-key c new-window\nunbind-key x\nbind -n C-a send-keys a\nset -g mouse off\nbind-key q \\\n  display-panes\n";
        let text = merge_keys(old, &["bind-key C-n new-window".to_string()]);
        assert_eq!(
            text,
            "# mine\nbind -n C-a send-keys a\nset -g mouse off\nbind-key q \\\n  display-panes\nbind-key C-n new-window\n"
        );
        assert_eq!(merge_keys("bind-key c new-window\n", &[]), "", "nothing left");
    }

    fn server() -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        Server::new(pane_tx, "t".into(), events)
    }

    /// A key moved, swapped, refused; the lines written for the table, run
    /// on a fresh one, make the same table; `show-bindings` says where each
    /// key was.
    #[test]
    fn moved_keys_are_written_as_lines_that_make_the_same_table() {
        let mut s = server();
        assert!(matches!(s.rebind_key("c", "C-n", false), Outcome::Ok));
        match s.rebind_key("C-n", "x", false) {
            Outcome::Error(e) => assert!(e.contains("x is bound to") && e.contains("-s swaps"), "{e}"),
            _ => panic!("taken key not refused"),
        }
        assert!(matches!(s.rebind_key("C-n", "x", true), Outcome::Ok));
        assert!(matches!(s.rebind_key("nosuch", "y", false), Outcome::Error(_)));
        match s.rebind_key("F9", "y", false) {
            Outcome::Error(e) => assert!(e.contains("bound to nothing"), "{e}"),
            _ => panic!("an empty key moved"),
        }
        let lines = s.key_lines();
        assert!(!lines.is_empty());
        let mut fresh = server();
        for l in &lines {
            let cmd = parse_line(l).unwrap().unwrap();
            assert!(!matches!(fresh.exec(cmd, None), Outcome::Error(_)), "{l}");
        }
        let table = |s: &Server| {
            let mut v: Vec<String> =
                s.prefix_binds.iter().map(|(k, b)| format!("{k} {} {}", b.repeat, b.cmd)).collect();
            v.sort();
            v
        };
        assert_eq!(table(&fresh), table(&s), "the lines make the same table");
        // Where each key was: new-window now on x, was c.
        let Outcome::Text(json) = s.show_bindings(true) else { panic!() };
        let rows: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        let row = rows.iter().find(|r| r["command"] == "new-window").unwrap();
        assert_eq!((row["key"].as_str(), row["base"].as_str()), (Some("x"), Some("c")));
        // The same table writes the same lines.
        assert_eq!(s.key_lines(), lines);
        // A command the config bound to two keys (Left and h): moved off h,
        // it came from h, not from Left, which still runs it.
        assert!(matches!(s.rebind_key("h", "F2", false), Outcome::Ok));
        let Outcome::Text(json) = s.show_bindings(true) else { panic!() };
        let rows: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        let f2 = rows.iter().find(|r| r["key"] == "F2").unwrap();
        assert_eq!(f2["base"].as_str(), Some("h"), "{f2}");
        let left = rows.iter().find(|r| r["key"] == "Left").unwrap();
        assert_eq!(left["base"].as_str(), Some("Left"));
    }

    /// A command a paired machine sends (refused or not) is kept in the
    /// event log with a secret's value as `(set)`.
    #[test]
    fn a_machines_command_is_kept_without_its_secret() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        let dir = std::env::temp_dir().join(format!("keepane-settings-feed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let peer =
            crate::link::Peer { key: "c".into(), addr: "10.0.0.3:7681".into(), full: true, lan: true, off: false };
        s.link = Some(super::super::link_state::LinkHost {
            dir: dir.clone(),
            id: crate::link::Identity::make(&dir.join("key")).unwrap(),
            peers: vec![peer],
            nonces: Default::default(),
            seen: Default::default(),
            origin: Default::default(),
            refused: Default::default(),
            sent: Default::default(),
            made: Default::default(),
            incoming: Default::default(),
        });
        // The event log on, in a folder of its own.
        s.observe.dir = Some(dir.join("events"));
        let _fed = s.feed_for_test("c");
        s.stream_command("c", 1, "set -g feishu-app-secret s3cret-z");
        let kept = s.observe.recent(20, |_| true).join("\n");
        assert!(kept.contains("feishu-app-secret (set)"), "{kept}");
        assert!(!kept.contains("s3cret-z"), "{kept}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn home_reads_as_a_tilde() {
        let home = dirs::home_dir().unwrap();
        let p = home.join(".keepane").join("settings.conf");
        assert!(shown_path(&p.to_string_lossy()).starts_with("~"));
        assert_eq!(shown_path("/elsewhere/x.conf"), "/elsewhere/x.conf");
        let sibling = format!("{}x", home.to_string_lossy());
        assert_eq!(shown_path(&sibling), sibling, "a folder that only starts like home");
    }
}
