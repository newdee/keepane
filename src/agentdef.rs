//! Agent definition files (docs/design/agent-defs.md): what keepane knows of
//! an agent it has no code for, as data. One TOML file per agent: how to
//! tell its process, which of its hook events say it is free, busy or
//! waiting for you, where its hooks' input says what, what `setup` writes
//! where, the words its screen shows, and how to take a conversation up
//! again.
//!
//! Definitions come from the files shipped in `agents/` (built in) and from
//! `~/.keepane/agents/*.toml`; a file there cannot take a name keepane
//! already has. A file that is wrong is skipped, and why is kept
//! (`Registry::errors`); the others load.

use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// The format version this keepane reads.
pub const FORMAT: u32 = 1;
/// A definition file's size at most.
const MOST_BYTES: u64 = 64 * 1024;
/// Items in a list (programs, events, screen words…) at most.
const MOST_ITEMS: usize = 32;
/// Bytes in one item, a title or a command at most.
const MOST_ITEM_BYTES: usize = 200;
/// Bytes of a file's text for `setup` at most.
const MOST_TEXT_BYTES: usize = 16 * 1024;

/// The definitions shipped with keepane, from the repository's `agents/`.
const SHIPPED: &[(&str, &str)] = &[("agents/codewhale.toml", include_str!("../agents/codewhale.toml"))];

/// A program no definition may claim: a script runner (its command line
/// says which agent), a shell, or the program of an agent keepane has code
/// for.
fn not_an_agents_own(stem: &str) -> bool {
    const RUNNERS_AND_SHELLS: &[&str] = &[
        "node",
        "bun",
        "deno",
        "cmd",
        "python",
        "python3",
        "pwsh",
        "powershell",
        "bash",
        "zsh",
        "fish",
        "sh",
        "dash",
        "ksh",
        "nu",
        "wsl",
        "cursor-agent",
        "agent",
    ];
    RUNNERS_AND_SHELLS.contains(&stem) || crate::setup::AGENTS.iter().any(|a| a.program() == stem)
}

/// The names keepane's own code has (`setup`'s words and Cursor's other
/// names): a file cannot take them over.
fn built_in(name: &str) -> bool {
    crate::setup::AGENTS.iter().any(|a| a.word() == name) || matches!(name, "cursor-agent" | "agent")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Def {
    /// The word for `keepane setup NAME` and the hooks' `--agent NAME`.
    pub name: String,
    /// Shown where the agent is named; the name when not given.
    pub title: Option<String>,
    pub format: u32,
    pub detect: Detect,
    pub events: Events,
    #[serde(default)]
    pub hook_input: HookInput,
    #[serde(default)]
    pub screen: Screen,
    /// The command that takes a conversation up again: `{id}` is its id.
    /// Without `{id}` it is run as it is, for an agent whose hooks are not
    /// given the id it resumes by (`codewhale --continue`: the latest in the
    /// pane's folder).
    pub resume: Option<String>,
    #[serde(default)]
    pub setup: Setup,
    /// Where it was read from: a file's path, or the shipped file's name.
    #[serde(skip)]
    pub from: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Process names, as `program_stem` makes them.
    pub programs: Vec<String>,
    /// Words in the command line of a node, bun, deno or cmd process.
    #[serde(default)]
    pub command_line: Vec<String>,
}

/// The agent's own event names for each keepane command its hooks run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Events {
    pub ready: Vec<String>,
    #[serde(default)]
    pub busy: Vec<String>,
    #[serde(default)]
    pub wait: Vec<String>,
    /// A tool call begins and ends (`pane-step`, a later step: read, not used).
    #[serde(default)]
    pub step_start: Vec<String>,
    #[serde(default)]
    pub step_end: Vec<String>,
}

/// Where a hook's input says what: `/a/b`, a JSON pointer into what it gets
/// on standard input, or `env:NAME`, a variable of its environment.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookInput {
    pub session: Option<String>,
    pub transcript: Option<String>,
    pub model: Option<String>,
    pub tool: Option<String>,
}

/// Words in a pane's last lines, any case.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    #[serde(default)]
    pub waiting: Vec<String>,
    #[serde(default)]
    pub working: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    /// The agent's directory: `~` is the home directory.
    pub dir: Option<String>,
    /// A variable that, when set, names that directory instead.
    pub dir_env: Option<String>,
    #[serde(default)]
    pub files: Vec<SetupFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupFile {
    /// Under the directory.
    pub path: String,
    pub how: How,
    /// There already: installed (`append`, `own-file`).
    pub mark: Option<String>,
    /// There: the agent's hooks may be off (`setup` says so).
    #[serde(default)]
    pub off_when: Vec<String>,
    /// What is appended or written (`append`, `own-file`), with `{ready}`,
    /// `{busy}` and `{wait}` for keepane's commands.
    pub text: Option<String>,
    /// Appended before the text only when the file has no line that is
    /// `head_mark` (`append`): a section header such as `[hooks]`, which a
    /// TOML file may hold once.
    pub head: Option<String>,
    pub head_mark: Option<String>,
    /// Where the value goes (`json-merge`): the first of these whose
    /// object above is there, else the first.
    pub at: Option<OneOrMany>,
    pub value: Option<toml::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum How {
    Append,
    JsonMerge,
    OwnFile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn all(&self) -> Vec<&str> {
        match self {
            OneOrMany::One(s) => vec![s.as_str()],
            OneOrMany::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// The keepane command a hook runs for an event of `kind` (`ready`, `busy`,
/// `wait`), for the agent `name`. Not quoted: an agent may run its hooks in
/// PowerShell, where a quoted path followed by arguments does not parse.
pub fn hook_command(kind: &str, name: &str) -> String {
    format!("keepane pane-{kind} -q --agent {name}")
}

/// A program's name as definitions compare it: its last part (either
/// separator, on any system), lower case, without `.exe`, `.cmd` or `.bat`.
pub fn program_stem(program: &str) -> String {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat"] {
        if let Some(stem) = base.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    base
}

impl Def {
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// Whether a process of this name (and, for a node, bun, deno or cmd,
    /// this command line, lower case) is this agent.
    pub fn is(&self, stem: &str, command_line: impl FnOnce() -> Option<String>) -> bool {
        if self.detect.programs.iter().any(|p| p.eq_ignore_ascii_case(stem)) {
            return true;
        }
        if self.detect.command_line.is_empty() || !matches!(stem, "node" | "bun" | "deno" | "cmd") {
            return false;
        }
        let Some(line) = command_line() else { return false };
        let line = line.to_ascii_lowercase().replace('\\', "/");
        self.detect.command_line.iter().any(|w| line.contains(&w.to_ascii_lowercase()))
    }

    /// What a hook's input says at `spec` (`/pointer` or `env:NAME`): a
    /// word with no control character, at most a path's length, else none.
    pub fn take(spec: &str, input: Option<&Value>, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        let raw = match spec.strip_prefix("env:") {
            Some(name) => env(name)?,
            None => match input?.pointer(spec)? {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            },
        };
        let s = raw.trim();
        (!s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)).then(|| s.to_string())
    }

    /// The agent's directory, from the home directory and the environment.
    pub fn dir(&self, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        if let Some(v) = self.setup.dir_env.as_deref().and_then(env).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
        let d = self.setup.dir.as_deref()?;
        Some(match d.strip_prefix('~') {
            Some(rest) => home.join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(d),
        })
    }

    /// The text for a file, its placeholders made keepane's commands.
    pub fn text_of(&self, f: &SetupFile) -> String {
        self.fill(f.text.as_deref().unwrap_or_default())
    }

    /// The head for a file (`append`), its placeholders filled the same way.
    pub fn head_of(&self, f: &SetupFile) -> Option<String> {
        f.head.as_deref().map(|h| self.fill(h))
    }

    fn fill(&self, s: &str) -> String {
        let mut t = s.to_string();
        for kind in ["ready", "busy", "wait"] {
            t = t.replace(&format!("{{{kind}}}"), &hook_command(kind, &self.name));
        }
        t
    }

    /// Everything a definition must hold to, or why not.
    fn check(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || n.len() > 32 || !n.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(format!("name '{n}': 1 to 32 characters, starting with a letter"));
        }
        if !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("name '{n}': lower-case letters, digits and - only"));
        }
        if self.format != FORMAT {
            return Err(format!("format {} (this keepane reads format {FORMAT})", self.format));
        }
        let word = |what: &str, s: &str| -> Result<(), String> {
            if s.trim().is_empty() || s.len() > MOST_ITEM_BYTES || s.chars().any(char::is_control) {
                return Err(format!("{what} '{s}': not empty, at most {MOST_ITEM_BYTES} bytes, one line"));
            }
            Ok(())
        };
        let list = |what: &str, v: &[String]| -> Result<(), String> {
            if v.len() > MOST_ITEMS {
                return Err(format!("{what}: {} items, at most {MOST_ITEMS}", v.len()));
            }
            v.iter().try_for_each(|s| word(what, s))
        };
        if let Some(t) = &self.title {
            word("title", t)?;
        }
        if self.detect.programs.is_empty() {
            return Err("detect.programs: at least one program".into());
        }
        list("detect.programs", &self.detect.programs)?;
        if let Some(p) =
            self.detect.programs.iter().find(|p| p.contains(['/', '\\']) || p.contains(char::is_whitespace))
        {
            return Err(format!("detect.programs '{p}': a program's name, no path"));
        }
        // A runner, a shell or an agent keepane has code for would make every
        // such process this agent (a PowerShell pane taking prompts).
        if let Some(p) = self.detect.programs.iter().find(|p| not_an_agents_own(&program_stem(p))) {
            return Err(format!(
                "detect.programs '{p}': a runner, a shell or one of keepane's own agents, not this one's"
            ));
        }
        list("detect.command_line", &self.detect.command_line)?;
        if let Some(w) = self.detect.command_line.iter().find(|w| w.trim().len() < 3) {
            return Err(format!(
                "detect.command_line '{w}': at least 3 characters (it is looked for in whole command lines)"
            ));
        }
        if self.events.ready.is_empty() {
            return Err("events.ready: at least one event (the one at a turn's end)".into());
        }
        let e = &self.events;
        for (what, v) in [
            ("events.ready", &e.ready),
            ("events.busy", &e.busy),
            ("events.wait", &e.wait),
            ("events.step_start", &e.step_start),
            ("events.step_end", &e.step_end),
        ] {
            list(what, v)?;
        }
        let h = &self.hook_input;
        for (what, spec) in
            [("session", &h.session), ("transcript", &h.transcript), ("model", &h.model), ("tool", &h.tool)]
        {
            let Some(spec) = spec else { continue };
            let env_ok = spec
                .strip_prefix("env:")
                .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            if !(env_ok || spec.starts_with('/')) || spec.len() > MOST_ITEM_BYTES {
                return Err(format!("hook_input.{what} '{spec}': a JSON pointer (/a/b) or env:NAME"));
            }
        }
        list("screen.waiting", &self.screen.waiting)?;
        list("screen.working", &self.screen.working)?;
        if let Some(r) = &self.resume {
            word("resume", r)?;
        }
        let s = &self.setup;
        if let Some(v) = &s.dir_env
            && (v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err(format!("setup.dir_env '{v}': a variable's name"));
        }
        if let Some(d) = &s.dir {
            word("setup.dir", d)?;
        }
        if !s.files.is_empty() && s.dir.is_none() && s.dir_env.is_none() {
            return Err("setup.files: setup.dir says where they are".into());
        }
        for f in &s.files {
            self.check_file(f)?;
        }
        Ok(())
    }

    fn check_file(&self, f: &SetupFile) -> Result<(), String> {
        let p = &f.path;
        let parts: Vec<&str> = p.split(['/', '\\']).collect();
        if p.is_empty()
            || Path::new(p).is_absolute()
            || p.starts_with(['/', '\\'])
            || p.contains(':')
            || parts.iter().any(|x| x.is_empty() || *x == "." || *x == "..")
        {
            return Err(format!("setup.files path '{p}': a file under setup.dir (no .., no drive)"));
        }
        let off: Vec<String> = f.off_when.clone();
        if off.len() > MOST_ITEMS || off.iter().any(|w| w.trim().is_empty() || w.len() > MOST_ITEM_BYTES) {
            return Err(format!(
                "setup.files '{p}' off_when: at most {MOST_ITEMS} words of at most {MOST_ITEM_BYTES} bytes"
            ));
        }
        match f.how {
            How::Append | How::OwnFile => {
                let (Some(text), Some(mark)) = (&f.text, &f.mark) else {
                    return Err(format!("setup.files '{p}': {:?} needs text and mark", f.how));
                };
                if mark.trim().is_empty() || mark.len() > MOST_ITEM_BYTES {
                    return Err(format!("setup.files '{p}' mark: not empty, at most {MOST_ITEM_BYTES} bytes"));
                }
                if text.len() > MOST_TEXT_BYTES {
                    return Err(format!("setup.files '{p}' text: {} bytes, at most {MOST_TEXT_BYTES}", text.len()));
                }
                if f.at.is_some() || f.value.is_some() {
                    return Err(format!("setup.files '{p}': at and value are for json-merge"));
                }
                match (&f.head, &f.head_mark) {
                    (None, None) => {}
                    (Some(head), Some(hm)) if f.how == How::Append => {
                        if head.len() > MOST_TEXT_BYTES || hm.trim().is_empty() || hm.contains('\n') {
                            return Err(format!("setup.files '{p}' head_mark: one line, not empty"));
                        }
                        if !head.lines().any(|l| l.trim() == hm.trim()) {
                            return Err(format!("setup.files '{p}': the head does not hold its head_mark '{hm}'"));
                        }
                        self.check_placeholders(p, head)?;
                    }
                    (Some(_), Some(_)) => return Err(format!("setup.files '{p}': head is for append")),
                    _ => return Err(format!("setup.files '{p}': head and head_mark go together")),
                }
                self.check_placeholders(p, text)?;
                if !self.text_of(f).contains(mark.as_str()) {
                    return Err(format!("setup.files '{p}': the text does not hold its mark '{mark}'"));
                }
            }
            How::JsonMerge => {
                let (Some(at), Some(_)) = (&f.at, &f.value) else {
                    return Err(format!("setup.files '{p}': json-merge needs at and value"));
                };
                let all = at.all();
                if all.is_empty() || all.len() > MOST_ITEMS {
                    return Err(format!("setup.files '{p}' at: 1 to {MOST_ITEMS} places"));
                }
                if let Some(bad) = all.iter().find(|a| !a.starts_with('/') || a.len() < 2 || a.ends_with('/')) {
                    return Err(format!("setup.files '{p}' at '{bad}': a JSON pointer such as /servers/keepane"));
                }
                if f.text.is_some() || f.mark.is_some() || f.head.is_some() || f.head_mark.is_some() {
                    return Err(format!("setup.files '{p}': text, mark and head are for append and own-file"));
                }
            }
        }
        Ok(())
    }

    /// `{word}` in a text: only keepane's commands for events the file has.
    fn check_placeholders(&self, path: &str, text: &str) -> Result<(), String> {
        let mut rest = text;
        while let Some(i) = rest.find('{') {
            let after = &rest[i + 1..];
            let Some(j) = after.find('}') else { break };
            let word = &after[..j];
            let simple = !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '_');
            if simple {
                let events = match word {
                    "ready" => &self.events.ready,
                    "busy" => &self.events.busy,
                    "wait" => &self.events.wait,
                    _ => {
                        return Err(format!(
                            "setup.files '{path}': {{{word}}} is not one of {{ready}}, {{busy}}, {{wait}}"
                        ));
                    }
                };
                if events.is_empty() {
                    return Err(format!("setup.files '{path}': {{{word}}} is used, but events.{word} is empty"));
                }
            }
            rest = &after[j + 1..];
        }
        Ok(())
    }
}

/// A definition from its text; `from` says where it was read.
pub fn parse(text: &str, from: &str) -> Result<Def, String> {
    // Line breaks as LF whatever the file has (a Windows editor's CRLF, the
    // shipped file checked out on Windows): the text `setup` writes is the
    // same everywhere, and adds no CR to an agent's LF file.
    let text = text.replace("\r\n", "\n");
    let mut d: Def = toml::from_str(&text).map_err(|e| format!("{from}: {}", e.to_string().trim_end()))?;
    d.from = from.to_string();
    d.check().map_err(|e| format!("{from}: {e}"))?;
    Ok(d)
}

/// Every definition keepane has, and why the files it skipped were skipped.
#[derive(Debug, Default)]
pub struct Registry {
    pub defs: Vec<Def>,
    pub errors: Vec<String>,
}

impl Registry {
    /// The shipped definitions, then each `*.toml` in `dirs` in the order of
    /// its name. A name already taken (by keepane's code, a shipped file or
    /// an earlier file) is not taken again.
    pub fn load(dirs: &[PathBuf]) -> Registry {
        let mut r = Registry::default();
        for (from, text) in SHIPPED {
            r.add(parse(text, from));
        }
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml")) && p.is_file())
                .collect();
            files.sort();
            for p in files {
                let from = p.display().to_string();
                let read = match std::fs::metadata(&p) {
                    Ok(m) if m.len() > MOST_BYTES => Err(format!("{from}: {} bytes, at most {MOST_BYTES}", m.len())),
                    _ => std::fs::read_to_string(&p).map_err(|e| format!("{from}: {e}")),
                };
                r.add(read.and_then(|t| parse(&t, &from)));
            }
        }
        r
    }

    fn add(&mut self, d: Result<Def, String>) {
        match d {
            Err(e) => self.errors.push(e),
            Ok(d) if built_in(&d.name) => {
                self.errors.push(format!("{}: '{}' is keepane's own agent; a file cannot take it over", d.from, d.name))
            }
            Ok(d) if self.get(&d.name).is_some() => {
                let first = &self.get(&d.name).expect("there").from;
                self.errors.push(format!("{}: '{}' is defined already, in {first}", d.from, d.name))
            }
            Ok(d) => self.defs.push(d),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Def> {
        self.defs.iter().find(|d| d.name == name)
    }

    /// The definition a process is, by its name and (for a node, bun, deno or
    /// cmd) its command line.
    pub fn of_process(&self, name: &str, command_line: impl FnOnce() -> Option<String>) -> Option<&Def> {
        let stem = program_stem(name);
        if let Some(d) = self.defs.iter().find(|d| d.detect.programs.iter().any(|p| p.eq_ignore_ascii_case(&stem))) {
            return Some(d);
        }
        if !matches!(stem.as_str(), "node" | "bun" | "deno" | "cmd") {
            return None;
        }
        let line = command_line()?;
        self.defs.iter().find(|d| d.is(&stem, || Some(line.clone())))
    }
}

/// What `agent-defs` prints: each definition (its name, programs, where it
/// was read), then each file skipped and why.
pub fn listing(reg: &Registry) -> String {
    let mut out: Vec<String> =
        reg.defs.iter().map(|d| format!("{:<14} {:<24} {}", d.name, d.detect.programs.join(","), d.from)).collect();
    if out.is_empty() {
        out.push("no agent definitions".into());
    }
    out.extend(reg.errors.iter().map(|e| format!("skipped: {e}")));
    if let Some(dir) = user_dir() {
        out.push(format!("(yours go in {})", dir.display()));
    }
    out.join("\n")
}

/// Where a person's own definitions are: `KEEPANE_AGENTS_DIR`, else
/// `~/.keepane/agents`.
pub fn user_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("KEEPANE_AGENTS_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    dirs::home_dir().map(|h| h.join(".keepane").join("agents"))
}

/// The names of the defined agents ever seen by this process, in the order
/// first seen: an agent's kind (`agents::Kind::Defined`) is its number here,
/// which a reload never changes. A few dozen at most: each kept for good.
static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

/// The number for a defined agent's name.
pub fn intern(name: &str) -> u16 {
    let mut names = NAMES.lock().expect("not poisoned");
    if let Some(i) = names.iter().position(|n| *n == name) {
        return i as u16;
    }
    names.push(Box::leak(name.to_string().into_boxed_str()));
    (names.len() - 1) as u16
}

/// The name for a number `intern` gave.
pub fn interned(i: u16) -> &'static str {
    NAMES.lock().expect("not poisoned").get(i as usize).copied().unwrap_or("agent")
}

static CURRENT: RwLock<Option<Arc<Registry>>> = RwLock::new(None);

/// The definitions in use, read the first time they are asked for.
pub fn current() -> Arc<Registry> {
    if let Some(r) = CURRENT.read().expect("not poisoned").as_ref() {
        return r.clone();
    }
    reload()
}

/// Read the definitions again (`agent-defs -r`).
pub fn reload() -> Arc<Registry> {
    let r = Arc::new(Registry::load(&user_dir().into_iter().collect::<Vec<_>>()));
    *CURRENT.write().expect("not poisoned") = Some(r.clone());
    r
}

#[cfg(test)]
mod tests;
