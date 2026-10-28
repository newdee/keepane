//! The machines you reach with `ssh` (`choose-ssh`, prefix @; `ssh-hosts`):
//! the ones a pane connected to (seen as `ssh` processes under the panes, a
//! use each time one starts), the ones `~/.ssh/config` names, the ones
//! `known_hosts` has met, and, once, the `ssh` lines of the shells' history.
//! Kept in `ssh-hosts.json` in the data directory: names, users, ports, when
//! and how often; never a password or a key.

use super::agents::PaneInfo;
use super::layout::PaneId;
use super::shellhist::{self, Shell};
use super::{Outcome, Server};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where `ssh` was pointed: the name typed (an alias of `~/.ssh/config`, a
/// host name or an address), and the user and port when given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl Target {
    /// As it reads: `root@web1`, `web1:2222` (port 22 left out).
    pub fn label(&self) -> String {
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        let port = self.port.filter(|p| *p != 22).map(|p| format!(":{p}")).unwrap_or_default();
        format!("{user}{}{port}", self.name)
    }

    /// One machine is one entry: by its user, name and port, case aside.
    pub fn key(&self) -> String {
        self.label().to_lowercase()
    }

    /// Whether `other` may be the same machine: the same name, and users
    /// and ports that agree where both are given (`ssh web1` and
    /// `ssh root@web1` are; `alice@web1` and `bob@web1` are not).
    fn agrees(&self, other: &Target) -> bool {
        let port = |t: &Target| t.port.filter(|p| *p != 22);
        self.name.eq_ignore_ascii_case(&other.name)
            && (self.user.is_none() || other.user.is_none() || self.user == other.user)
            && (port(self).is_none() || port(other).is_none() || port(self) == port(other))
    }

    /// The `ssh` command line that reaches it again.
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec!["ssh".to_string()];
        if let Some(p) = self.port.filter(|p| *p != 22) {
            v.push("-p".into());
            v.push(p.to_string());
        }
        v.push(match &self.user {
            Some(u) => format!("{u}@{}", self.name),
            None => self.name.clone(),
        });
        v
    }
}

/// `ssh`'s options that take a value (the next word, or the rest of the
/// same word).
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// Where an `ssh` command line goes, from its words (the program first);
/// none for one that names no machine (`ssh -V`), or that is not `ssh`.
pub fn parse(words: &[String]) -> Option<Target> {
    let (prog, args) = words.split_first()?;
    let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog).to_ascii_lowercase();
    if base != "ssh" && base != "ssh.exe" {
        return None;
    }
    let (mut user, mut port, mut dest) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            dest = it.next().cloned();
            break;
        }
        if let Some(flags) = a.strip_prefix('-').filter(|f| !f.is_empty()) {
            // Flags run together (`-tt`, `-p2222`): the first that takes a
            // value takes the rest of the word, or the next one.
            for (i, f) in flags.char_indices() {
                if !WITH_VALUE.contains(f) {
                    continue;
                }
                let rest = &flags[i + f.len_utf8()..];
                let value = if rest.is_empty() { it.next().cloned() } else { Some(rest.to_string()) };
                match (f, value) {
                    ('p', Some(v)) => port = v.parse().ok(),
                    ('l', Some(v)) => user = Some(v),
                    ('o', Some(v)) => {
                        let (k, v) = v.split_once(['=', ' ']).unwrap_or((&v, ""));
                        match k.to_ascii_lowercase().as_str() {
                            "port" => port = v.trim().parse().ok(),
                            "user" => user = Some(v.trim().to_string()),
                            _ => {}
                        }
                    }
                    _ => {}
                }
                break;
            }
            continue;
        }
        dest = Some(a.clone());
        break;
    }
    let dest = dest?;
    // `ssh://user@host:port`, else `user@host`.
    let (dest, url_port) = match dest.strip_prefix("ssh://") {
        Some(rest) => {
            let rest = rest.trim_end_matches('/');
            match rest.rsplit_once(':') {
                Some((h, p)) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) => {
                    (h.to_string(), p.parse().ok())
                }
                _ => (rest.to_string(), None),
            }
        }
        None => (dest, None),
    };
    let (name, user) = match dest.rsplit_once('@') {
        Some((u, h)) => (h.to_string(), Some(u.to_string()).filter(|u| !u.is_empty())),
        None => (dest, user),
    };
    let ok = !name.is_empty() && !name.starts_with('-') && !name.contains(['/', '\\', '|', ';', '&', '<', '>']);
    ok.then_some(Target { name, user, port: url_port.or(port) })
}

/// A command line's words, as a shell splits them: on spaces, quotes kept
/// together and dropped.
pub fn words(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (_, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Where an entry came from, when no pane has connected to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A pane connected to it, or the shells' history named it.
    #[default]
    Used,
    /// `~/.ssh/config` names it.
    Config,
    /// `known_hosts` has met it.
    Seen,
}

/// One machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Host {
    #[serde(flatten)]
    pub target: Target,
    /// What `~/.ssh/config` says it is (`HostName`), for an alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real: Option<String>,
    /// When a pane last connected to it, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<i64>,
    /// How many times it was connected to (the history's lines included).
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub source: Source,
}

/// The most machines kept: past it, the least used go.
const MOST: usize = 300;

/// Every machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub hosts: Vec<Host>,
    /// The shells' history has been read (once, the first time).
    #[serde(default)]
    pub imported: bool,
    /// The names taken off the list (`d`), in lower case: the config and
    /// `known_hosts` do not bring them back; connecting to one does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forgotten: Vec<String>,
}

impl Book {
    fn is_forgotten(&self, name: &str) -> bool {
        self.forgotten.iter().any(|f| f.eq_ignore_ascii_case(name))
    }

    /// Entry `i` off the list, its name not to come back from the config
    /// or `known_hosts`.
    pub fn forget(&mut self, i: usize) {
        if i >= self.hosts.len() {
            return;
        }
        let h = self.hosts.remove(i);
        if !self.is_forgotten(&h.target.name) {
            self.forgotten.push(h.target.name.to_lowercase());
        }
    }

    /// The entry of `t`: the one of the same key, else one that agrees.
    fn index_of(&self, t: &Target) -> Option<usize> {
        let key = t.key();
        self.hosts
            .iter()
            .position(|h| h.target.key() == key)
            .or_else(|| self.hosts.iter().position(|h| h.target.agrees(t)))
    }

    /// The entry of `t`, as `index_of`; one that agrees takes the user and
    /// port `t` gives and it lacks (a name `known_hosts` met, then `ssh
    /// root@` it: root's), so another user of the same machine is an entry
    /// of its own.
    fn adopt(&mut self, t: &Target) -> Option<usize> {
        let i = self.index_of(t)?;
        let h = &mut self.hosts[i].target;
        if h.user.is_none() {
            h.user.clone_from(&t.user);
        }
        if h.port.is_none() {
            h.port = t.port;
        }
        Some(i)
    }

    /// A pane connected to `t` at `now`.
    pub fn used(&mut self, t: &Target, now: i64) {
        self.forgotten.retain(|f| !f.eq_ignore_ascii_case(&t.name));
        match self.adopt(t) {
            Some(i) => {
                let h = &mut self.hosts[i];
                h.count += 1;
                h.last = Some(now);
                h.source = Source::Used;
            }
            None => self.hosts.push(Host {
                target: t.clone(),
                last: Some(now),
                count: 1,
                source: Source::Used,
                ..Host::default()
            }),
        }
        self.trim();
    }

    /// `~/.ssh/config`'s hosts: each alias (not a pattern), with its user,
    /// port and real name.
    pub fn config(&mut self, text: &str) {
        let mut block: Vec<String> = Vec::new();
        let mut fields: HashMap<String, String> = HashMap::new();
        let flush = |block: &mut Vec<String>, fields: &mut HashMap<String, String>, book: &mut Book| {
            for alias in block.drain(..) {
                let target = Target {
                    name: alias,
                    user: fields.get("user").cloned(),
                    port: fields.get("port").and_then(|p| p.parse().ok()),
                };
                let real = fields.get("hostname").cloned().filter(|r| !r.eq_ignore_ascii_case(&target.name));
                if book.is_forgotten(&target.name) {
                    continue;
                }
                match book.index_of(&target) {
                    Some(i) => {
                        let h = &mut book.hosts[i];
                        h.real = real.or(h.real.take());
                        if h.source == Source::Seen {
                            h.source = Source::Config;
                        }
                    }
                    None => book.hosts.push(Host { target, real, source: Source::Config, ..Host::default() }),
                }
            }
            fields.clear();
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once(|c: char| c.is_whitespace() || c == '=').unwrap_or((line, ""));
            let (k, v) = (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim());
            match k.as_str() {
                "host" => {
                    flush(&mut block, &mut fields, self);
                    block = v
                        .split_whitespace()
                        .map(|a| a.trim_matches('"'))
                        .filter(|a| !a.is_empty() && !a.contains(['*', '?', '!']))
                        .map(String::from)
                        .collect();
                }
                "match" => flush(&mut block, &mut fields, self),
                // The first value given is the one ssh takes.
                _ => {
                    fields.entry(k).or_insert_with(|| v.trim_matches('"').to_string());
                }
            }
        }
        flush(&mut block, &mut fields, self);
    }

    /// `known_hosts`: each machine named in plain text (`[host]:port` too);
    /// hashed lines (`|1|…`) say nothing. One already listed (by its name,
    /// or an alias's real name) is not listed again.
    pub fn known_hosts(&mut self, text: &str) {
        for line in text.lines() {
            let Some(names) = line.split_whitespace().next().filter(|n| !n.starts_with(['|', '#', '@'])) else {
                continue;
            };
            let Some(first) = names.split(',').next().filter(|n| !n.is_empty()) else { continue };
            let target = match first.strip_prefix('[').and_then(|r| r.split_once("]:")) {
                Some((h, p)) => Target { name: h.to_string(), user: None, port: p.parse().ok() },
                None => Target { name: first.to_string(), user: None, port: None },
            };
            let known = self.hosts.iter().any(|h| {
                h.target.name.eq_ignore_ascii_case(&target.name)
                    || h.real.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(&target.name))
            });
            if !known && !self.is_forgotten(&target.name) {
                self.hosts.push(Host { target, source: Source::Seen, ..Host::default() });
            }
        }
    }

    /// The `ssh` commands of a shell's history, each a use (with no time).
    pub fn history(&mut self, entries: impl IntoIterator<Item = String>) {
        let mut counts: Vec<(Target, u64)> = Vec::new();
        for entry in entries {
            // fish writes `- cmd: ssh …`; keepane's messages carry an envelope.
            let entry = entry.trim();
            let entry = entry.strip_prefix("- cmd: ").unwrap_or(entry);
            let command = shellhist::delivered(entry).map(|d| d.command).unwrap_or_else(|| entry.to_string());
            let Some(t) = parse(&words(&command)) else { continue };
            match counts.iter_mut().find(|(c, _)| c.key() == t.key()) {
                Some((_, n)) => *n += 1,
                None => counts.push((t, 1)),
            }
        }
        counts.retain(|(t, _)| !self.is_forgotten(&t.name));
        for (t, n) in counts {
            match self.adopt(&t) {
                Some(i) => {
                    let h = &mut self.hosts[i];
                    h.count += n;
                    h.source = Source::Used;
                }
                None => self.hosts.push(Host { target: t, count: n, ..Host::default() }),
            }
        }
        self.trim();
    }

    /// Past the most kept, the least used let go.
    fn trim(&mut self) {
        if self.hosts.len() > MOST {
            self.hosts.sort_by_key(|h| std::cmp::Reverse((h.last.unwrap_or(0), h.count)));
            self.hosts.truncate(MOST);
        }
    }

    /// The positions of the entries in the order they are listed: by when
    /// last used, then how often, then the config's before the ones only
    /// seen, then by name.
    pub fn order(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.hosts.len()).collect();
        let rank = |s: Source| match s {
            Source::Used => 0,
            Source::Config => 1,
            Source::Seen => 2,
        };
        v.sort_by(|&a, &b| {
            let (a, b) = (&self.hosts[a], &self.hosts[b]);
            (b.last.unwrap_or(0), b.count)
                .cmp(&(a.last.unwrap_or(0), a.count))
                .then(rank(a.source).cmp(&rank(b.source)))
                .then_with(|| a.target.name.to_lowercase().cmp(&b.target.name.to_lowercase()))
        });
        v
    }

    pub fn load(path: &Path) -> Book {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Kept: the machines connected to (the config's and `known_hosts`'
    /// others are read again each time).
    pub fn save(&self, path: &Path) {
        let kept = Book {
            hosts: self.hosts.iter().filter(|h| h.count > 0 || h.last.is_some()).cloned().collect(),
            imported: self.imported,
            forgotten: self.forgotten.clone(),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if serde_json::to_vec_pretty(&kept).is_ok_and(|b| std::fs::write(&tmp, b).is_ok()) {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Where the book is kept: the data directory; or, when the saved sessions
/// are moved (`KEEPANE_SESSIONS_DIR`, `sessions-dir`: tests do), in an
/// `ssh` folder of theirs (not beside them: every `.json` there is taken
/// for a saved session).
pub fn book_path(sessions: &Path, moved: bool) -> PathBuf {
    if moved { sessions.join("ssh").join("ssh-hosts.json") } else { crate::logger::log_dir().join("ssh-hosts.json") }
}

/// The shells' history files: PowerShell's (PSReadLine), bash's, zsh's,
/// fish's (read as bash's: a line an entry).
fn history_files(home: &Path) -> Vec<(PathBuf, Shell)> {
    let mut v = vec![
        (home.join(".bash_history"), Shell::Bash),
        (home.join(".zsh_history"), Shell::Zsh),
        (home.join(".local/share/fish/fish_history"), Shell::Bash),
        (home.join(".local/share/powershell/PSReadLine/ConsoleHost_history.txt"), Shell::Pwsh),
    ];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let ps = PathBuf::from(appdata).join(r"Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt");
        v.push((ps, Shell::Pwsh));
    }
    v
}

/// Each pane's `ssh`, where one runs in it: its process and where it goes
/// (the first found: a `ProxyJump`'s own `ssh` runs under it). `known`
/// keeps what each process was found to be, so a command line is read
/// once; one not readable yet (just started) is asked again next time.
pub fn seen(panes: &[PaneInfo], known: &mut HashMap<u32, Option<Target>>) -> HashMap<PaneId, (u32, Target)> {
    let mut out = HashMap::new();
    let mut present = Vec::new();
    for p in panes {
        for (pid, name) in crate::sysinfo::tree_of(p.pid) {
            present.push(pid);
            if !name.eq_ignore_ascii_case("ssh") && !name.eq_ignore_ascii_case("ssh.exe") {
                continue;
            }
            let target = match known.get(&pid) {
                Some(t) => t.clone(),
                None => {
                    let Some(line) = crate::proccwd::process_command_line(pid) else { continue };
                    let t = parse(&words(&line));
                    known.insert(pid, t.clone());
                    t
                }
            };
            if let Some(t) = target {
                out.entry(p.id).or_insert((pid, t));
            }
        }
    }
    known.retain(|pid, _| present.contains(pid));
    out
}

/// The server's part: the book (read when first needed) and each pane's
/// `ssh` now.
#[derive(Default)]
pub(super) struct Ssh {
    book: Option<Book>,
    of: HashMap<PaneId, (u32, Target)>,
}

/// A machine as the picker and `ssh-hosts` show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Its place in the book: the picker's item.
    pub index: usize,
    /// The name typed: an alias, a host name, an address.
    pub name: String,
    /// Who and where, when it says more than the name: `root@10.0.0.5:2222`.
    pub detail: String,
    /// When last connected to: `now`, `5m ago`; empty when not known.
    pub when: String,
    pub count: u64,
    /// The panes connected to it now, as `session:window.pane`.
    pub panes: Vec<(PaneId, String)>,
    /// Where it was found, when no pane has connected to it: `config`,
    /// `seen`, `history`; else empty.
    pub source: &'static str,
    pub argv: Vec<String>,
}

impl Row {
    /// What the filter (`f`) looks through.
    pub fn text(&self) -> String {
        let panes: Vec<&str> = self.panes.iter().map(|(_, a)| a.as_str()).collect();
        format!("{} {} {} {}", self.name, self.detail, self.source, panes.join(" "))
    }

    pub fn command(&self) -> String {
        self.argv.join(" ")
    }
}

/// How long ago `then` was, at `now` (Unix seconds), short: `just now`,
/// `5m ago`, `2h ago`, `3d ago`, `5w ago`, `4mo ago`, `2y ago`.
pub fn ago(then: i64, now: i64) -> String {
    let s = (now - then).max(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        86_400..1_209_600 => format!("{}d ago", s / 86_400),
        1_209_600..5_184_000 => format!("{}w ago", s / 604_800),
        5_184_000..31_536_000 => format!("{}mo ago", s / 2_592_000),
        _ => format!("{}y ago", s / 31_536_000),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The rows a picker shows, in its order: its items (what the filter left)
/// found among `all`.
pub(super) fn shown<'a>(all: &'a [Row], items: &[super::ChooserItem]) -> Vec<&'a Row> {
    items
        .iter()
        .filter_map(|it| match it {
            super::ChooserItem::Ssh(i) => all.iter().find(|r| r.index == *i),
            _ => None,
        })
        .collect()
}

impl Server {
    fn ssh_book_path(&self) -> PathBuf {
        let moved = !self.opts.sessions_dir.is_empty()
            || crate::legacy::var_os("KEEPANE_SESSIONS_DIR").is_some_and(|d| !d.is_empty());
        book_path(&self.sessions_dir(), moved)
    }

    fn ssh_book(&mut self) -> &mut Book {
        let path = self.ssh_book_path();
        self.ssh.book.get_or_insert_with(|| Book::load(&path))
    }

    /// The book read again: `~/.ssh/config` and `known_hosts`, and the
    /// shells' history the first time (only with keepane's real places, not
    /// under a test's own directories).
    pub(super) fn ssh_refresh(&mut self) {
        let path = self.ssh_book_path();
        let book = self.ssh.book.get_or_insert_with(|| Book::load(&path));
        if !crate::legacy::uses_real_dirs() {
            return;
        }
        let home = dirs::home_dir().unwrap_or_default();
        let ssh = home.join(".ssh");
        if let Ok(t) = std::fs::read_to_string(ssh.join("config")) {
            book.config(&t);
        }
        if let Ok(t) = std::fs::read_to_string(ssh.join("known_hosts")) {
            book.known_hosts(&t);
        }
        if !book.imported {
            for (file, shell) in history_files(&home) {
                if let Ok(bytes) = std::fs::read(&file) {
                    book.history(shellhist::entries(&bytes, shell));
                }
            }
            book.imported = true;
            book.save(&path);
        }
    }

    /// The watcher's look: an `ssh` that was not there before (a new
    /// process) is a use of its machine.
    pub(super) fn ssh_seen(&mut self, seen: HashMap<PaneId, (u32, Target)>) {
        let new: Vec<Target> = seen
            .iter()
            .filter(|(pane, (pid, _))| self.ssh.of.get(pane).is_none_or(|(was, _)| was != pid))
            .map(|(_, (_, t))| t.clone())
            .collect();
        self.ssh.of = seen;
        if new.is_empty() {
            return;
        }
        let (at, path) = (now(), self.ssh_book_path());
        let book = self.ssh_book();
        for t in &new {
            book.used(t, at);
        }
        book.save(&path);
    }

    /// Where pane `id`'s `ssh` goes, as it reads (`#{pane_ssh_host}`);
    /// empty when it runs none.
    pub(super) fn ssh_host_of(&self, id: PaneId) -> String {
        self.ssh.of.get(&id).map(|(_, t)| t.label()).unwrap_or_default()
    }

    /// Every machine in the book, as listed: the ones connected to now
    /// first, then in the book's order.
    pub(super) fn ssh_rows(&self) -> Vec<Row> {
        let Some(book) = &self.ssh.book else { return Vec::new() };
        let (base, pane_base) = (self.opts.base_index, self.opts.pane_base_index);
        let mut address: HashMap<PaneId, String> = HashMap::new();
        for s in &self.sessions {
            for (widx, w) in s.windows.iter().enumerate() {
                for (pidx, pid) in w.layout.panes().iter().enumerate() {
                    address.insert(*pid, format!("{}:{}.{}", s.name, widx + base, pidx + pane_base));
                }
            }
        }
        let mut live: HashMap<usize, Vec<(PaneId, String)>> = HashMap::new();
        let mut of: Vec<(&PaneId, &(u32, Target))> = self.ssh.of.iter().collect();
        of.sort_by_key(|(pane, _)| address.get(pane).cloned());
        for (pane, (_, t)) in of {
            if let (Some(i), Some(a)) = (book.index_of(t), address.get(pane)) {
                live.entry(i).or_default().push((*pane, a.clone()));
            }
        }
        let at = now();
        let mut rows: Vec<Row> = book
            .order()
            .into_iter()
            .map(|i| {
                let h = &book.hosts[i];
                let panes = live.remove(&i).unwrap_or_default();
                let t = &h.target;
                let shown = Target { name: h.real.clone().unwrap_or_else(|| t.name.clone()), ..t.clone() };
                let detail = if h.real.is_some() || t.user.is_some() || t.port.is_some_and(|p| p != 22) {
                    shown.label()
                } else {
                    String::new()
                };
                let when = match (panes.is_empty(), h.last) {
                    (false, _) => "now".to_string(),
                    (true, Some(l)) => ago(l, at),
                    (true, None) => String::new(),
                };
                let source = match h.source {
                    _ if h.last.is_some() => "",
                    Source::Used => "history",
                    Source::Config => "config",
                    Source::Seen => "seen",
                };
                Row { index: i, name: t.name.clone(), detail, when, count: h.count, panes, source, argv: t.argv() }
            })
            .collect();
        // Connected now: first (sort_by_key keeps the order otherwise).
        rows.sort_by_key(|r| r.panes.is_empty());
        rows
    }

    /// `choose-ssh`, Enter (`split` false): to the pane connected to the
    /// machine, else a new window named after it running `ssh`; `s` (`split`
    /// true): `ssh` in a new pane beside the current one.
    pub(super) fn ssh_go(&mut self, cid: super::ClientId, index: usize, split: bool) {
        let Some(row) = self.ssh_rows().into_iter().find(|r| r.index == index) else {
            return self.message(cid, "that machine is gone from the list");
        };
        if !split && let Some((pane, _)) = row.panes.first() {
            let Some((sid, widx)) = self.place_of(*pane) else { return self.message(cid, "its pane is gone") };
            let Some(wid) = self.session(sid).and_then(|s| s.windows.get(widx)).map(|w| w.id) else { return };
            return self.chooser_go_pane(cid, sid, wid, *pane);
        }
        let cmd = if split {
            crate::command::Cmd::SplitWindow {
                horizontal: true,
                cwd: None,
                target: None,
                argv: row.argv,
                detached: false,
                before: false,
                full: false,
                count: 1,
            }
        } else {
            crate::command::Cmd::NewWindow {
                name: Some(row.name),
                cwd: None,
                target: None,
                argv: row.argv,
                detached: false,
            }
        };
        let out = self.exec(cmd, Some(cid));
        self.reply(cid, out);
    }

    /// `choose-ssh`, `d`: the machine off the list (the picker stays).
    pub(super) fn ssh_forget(&mut self, cid: super::ClientId, index: usize) {
        let Some(row) = self.ssh_rows().into_iter().find(|r| r.index == index) else { return };
        if !row.panes.is_empty() {
            return self.message(cid, &format!("{} is connected to now: not taken off", row.name));
        }
        let path = self.ssh_book_path();
        let book = self.ssh_book();
        book.forget(index);
        book.save(&path);
        self.message(cid, &format!("{} is off the list; connecting to it brings it back", row.name));
    }

    /// `choose-ssh`, `y`: the machine's `ssh` command to a buffer and the
    /// clipboard.
    pub(super) fn ssh_copy(&mut self, cid: super::ClientId, index: usize) {
        let Some(row) = self.ssh_rows().into_iter().find(|r| r.index == index) else {
            return self.message(cid, "that machine is gone from the list");
        };
        let command = row.command();
        self.set_buffer(None, &command, false);
        let note = match crate::clipboard::set_text(&command) {
            Err(e) => format!(" (clipboard: {e})"),
            Ok(()) => String::new(),
        };
        self.message(cid, &format!("copied `{command}`{note}"));
    }

    /// `ssh-hosts [-J]`: every machine, a line each (or a JSON list).
    pub(super) fn list_ssh_hosts(&mut self, json: bool) -> Outcome {
        self.ssh_refresh();
        let rows = self.ssh_rows();
        if json {
            let list: Vec<Value> = rows
                .iter()
                .map(|r| {
                    let h = &self.ssh.book.as_ref().unwrap().hosts[r.index];
                    json!({
                        "name": r.name,
                        "user": h.target.user,
                        "host": h.real.clone().unwrap_or_else(|| h.target.name.clone()),
                        "port": h.target.port,
                        "last": h.last,
                        "count": r.count,
                        "source": match h.source { Source::Used => "used", Source::Config => "config", Source::Seen => "seen" },
                        "panes": r.panes.iter().map(|(id, a)| json!({ "pane": format!("%{id}"), "at": a })).collect::<Vec<_>>(),
                        "command": r.command(),
                    })
                })
                .collect();
            return Outcome::Text(Value::Array(list).to_string());
        }
        if rows.is_empty() {
            return Outcome::Ok;
        }
        let w = |f: &dyn Fn(&Row) -> usize| rows.iter().map(f).max().unwrap_or(0);
        let (nw, dw, ww) = (w(&|r| r.name.chars().count()), w(&|r| r.detail.chars().count()), w(&|r| r.when.len()));
        let lines: Vec<String> = rows
            .iter()
            .map(|r| {
                let count = if r.count > 0 { format!("×{}", r.count) } else { String::new() };
                let tail = if r.panes.is_empty() {
                    r.source.to_string()
                } else {
                    r.panes.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>().join(" ")
                };
                let dot = if r.panes.is_empty() { ' ' } else { '●' };
                format!("{dot} {:<nw$}  {:<dw$}  {:>ww$}  {count:>5}  {tail}", r.name, r.detail, r.when)
                    .trim_end()
                    .to_string()
            })
            .collect();
        Outcome::Text(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests;
