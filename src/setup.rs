//! `keepane setup [agent] [--install]`: what a coding agent needs to work
//! in a pane that takes messages (docs/design/mailbox.md §9): a hook that
//! says when the agent is free (at the start of a session and at the end of
//! each turn it runs `keepane pane-ready -q`), and keepane's MCP server.
//!
//! Without an agent it says, for each agent it knows, whether it is on this
//! machine and whether both are set up. With one it prints what that agent
//! needs; `--install` writes it, each file backed up first, only keepane's
//! own entries added, everything else left as it was. A user's settings are
//! theirs: nothing is written unless asked.
//!
//! Each agent's hooks are set up from its own documentation (2026-09):
//! Claude Code `Stop`/`SessionStart` in `~/.claude/settings.json`; Codex the
//! same two in `~/.codex/hooks.json` (its `notify` holds one program only
//! and is left alone), run once the user trusts them (`/hooks`); Gemini CLI
//! `AfterAgent`/`SessionStart` in `~/.gemini/settings.json`; Cursor CLI
//! `stop`/`sessionStart` in `~/.cursor/hooks.json`; opencode has no such
//! hook but plugins, and a small one runs the command on `session.idle`.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What the hooks run: quiet outside a pane, ignored in a pane that is not
/// in `ai` work mode, so it is safe in every session of every agent.
pub const HOOK_COMMAND: &str = "keepane pane-ready -q";

/// How a hook is recognised as keepane's, whatever path it was written with.
const HOOK_MARK: &str = "keepane pane-ready";

/// The agents `setup` knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Gemini,
    Cursor,
    Opencode,
}

pub const AGENTS: [Agent; 5] = [Agent::Claude, Agent::Codex, Agent::Gemini, Agent::Cursor, Agent::Opencode];

impl Agent {
    /// The word `keepane setup` takes.
    pub fn word(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Cursor => "cursor",
            Agent::Opencode => "opencode",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Gemini => "Gemini CLI",
            Agent::Cursor => "Cursor CLI",
            Agent::Opencode => "opencode",
        }
    }

    /// The program it runs as.
    pub fn program(self) -> &'static str {
        match self {
            Agent::Cursor => "cursor-agent",
            a => a.word(),
        }
    }

    fn parse(word: &str) -> Option<Agent> {
        AGENTS.into_iter().find(|a| a.word() == word || a.program() == word)
    }

    /// The agent a program is, by its name (`codex`, `codex.exe`, a path).
    /// Either separator, on any system: a Windows path read on Linux (or a
    /// name from the other machine) is still split.
    pub fn of_program(program: &str) -> Option<Agent> {
        let name = program.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
        let stem = name.strip_suffix(".exe").unwrap_or(&name);
        AGENTS.into_iter().find(|a| a.program() == stem)
    }
}

/// Where each agent keeps what `setup` touches, from the home directory and
/// the environment (`env` is the process's, or a test's).
struct Places {
    /// The file the hooks go in (opencode: the plugin itself).
    hooks: PathBuf,
    /// The file its MCP servers are listed in.
    mcp: PathBuf,
}

fn places(agent: Agent, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Places {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    match agent {
        Agent::Claude => match set("KEEPANE_CLAUDE_DIR").or_else(|| set("CLAUDE_CONFIG_DIR")) {
            // A directory of its own holds both.
            Some(dir) => Places { hooks: dir.join("settings.json"), mcp: dir.join(".claude.json") },
            // Its user-scope MCP servers are in ~/.claude.json, beside ~/.claude.
            None => Places { hooks: home.join(".claude").join("settings.json"), mcp: home.join(".claude.json") },
        },
        Agent::Codex => {
            let dir = set("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
            Places { hooks: dir.join("hooks.json"), mcp: dir.join("config.toml") }
        }
        Agent::Gemini => {
            let file = home.join(".gemini").join("settings.json");
            Places { hooks: file.clone(), mcp: file }
        }
        Agent::Cursor => {
            let dir = home.join(".cursor");
            Places { hooks: dir.join("hooks.json"), mcp: dir.join("mcp.json") }
        }
        Agent::Opencode => {
            let dir = set("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("opencode");
            // Its config may be JSONC instead: that one, when it is the only one.
            let (json, jsonc) = (dir.join("opencode.json"), dir.join("opencode.jsonc"));
            let mcp = if !json.exists() && jsonc.exists() { jsonc } else { json };
            Places { hooks: dir.join("plugins").join("keepane.js"), mcp }
        }
    }
}

/// The events at a session's start and a turn's end, as the agent names
/// them.
fn events(agent: Agent) -> [&'static str; 2] {
    match agent {
        Agent::Claude | Agent::Codex => ["SessionStart", "Stop"],
        Agent::Gemini => ["SessionStart", "AfterAgent"],
        Agent::Cursor => ["sessionStart", "stop"],
        Agent::Opencode => ["plugin start", "session.idle"],
    }
}

/// opencode's plugin: the command when opencode starts and each time its
/// session goes idle (its turn is over). Bun's shell; a failure is nobody's
/// business but keepane's.
pub const OPENCODE_PLUGIN: &str = "\
// keepane: tells keepane when opencode is free for its next message
// (`keepane setup opencode`). Outside a keepane pane it does nothing.
export const KeepanePlugin = async ({ $ }) => {
  const ready = () => $`keepane pane-ready -q`.nothrow().quiet()
  await ready()
  return {
    event: async ({ event }) => {
      if (event.type === \"session.idle\") await ready()
    },
  }
}
";

fn obj<'a>(v: &'a mut Value, what: &str) -> Result<&'a mut serde_json::Map<String, Value>> {
    v.as_object_mut().with_context(|| format!("{what} is not a JSON object"))
}

/// Settings with keepane's hooks added where they are missing, in the shape
/// Claude Code, Codex and Gemini CLI share (`hooks.<event>` a list of
/// groups, each with its `hooks`); and whether anything was added.
/// Everything else is left as it was, in its order.
pub fn with_hooks(mut settings: Value, names: [&str; 2]) -> Result<(Value, bool)> {
    let root = obj(&mut settings, "the settings")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = obj(hooks, "\"hooks\"")?;
    let mut changed = false;
    for event in names {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(list) = list.as_array_mut() else { bail!("\"hooks\".\"{event}\" is not a list") };
        let there = list.iter().any(|group| {
            group["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(HOOK_MARK))))
        });
        if !there {
            list.push(json!({ "hooks": [{ "type": "command", "command": HOOK_COMMAND }] }));
            changed = true;
        }
    }
    Ok((settings, changed))
}

/// Cursor's hooks file with keepane's added: `version` 1, and
/// `hooks.<event>` a list of `{command}`.
pub fn with_cursor_hooks(mut file: Value) -> Result<(Value, bool)> {
    let root = obj(&mut file, "hooks.json")?;
    let mut changed = false;
    if !root.contains_key("version") {
        root.insert("version".into(), json!(1));
        changed = true;
    }
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = obj(hooks, "\"hooks\"")?;
    for event in events(Agent::Cursor) {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(list) = list.as_array_mut() else { bail!("\"hooks\".\"{event}\" is not a list") };
        if !list.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(HOOK_MARK))) {
            list.push(json!({ "command": HOOK_COMMAND }));
            changed = true;
        }
    }
    Ok((file, changed))
}

/// keepane's MCP server as each agent lists one; the key its list is under.
fn mcp_entry(agent: Agent) -> (&'static str, Value) {
    match agent {
        Agent::Opencode => ("mcp", json!({ "type": "local", "command": ["keepane", "mcp"], "enabled": true })),
        _ => ("mcpServers", json!({ "command": "keepane", "args": ["mcp"] })),
    }
}

/// A settings file with keepane's MCP server added, unless one named
/// `keepane` is there already (it is left as the user has it).
pub fn with_mcp(mut file: Value, agent: Agent) -> Result<(Value, bool)> {
    let (key, entry) = mcp_entry(agent);
    let root = obj(&mut file, "the settings")?;
    let list = root.entry(key).or_insert_with(|| json!({}));
    let list = obj(list, &format!("\"{key}\""))?;
    if list.contains_key("keepane") {
        return Ok((file, false));
    }
    list.insert("keepane".into(), entry);
    Ok((file, true))
}

/// A file read, or None when it is not there.
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Write a file, the old one (if any) copied beside it first. A backup is
/// never written over: one made the same second gets a number.
fn write_backed_up(path: &Path, old: Option<&str>, new: &str) -> Result<()> {
    if let Some(t) = old {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut backup = path.with_file_name(format!("{name}.bak-keepane-{stamp}"));
        for n in 2.. {
            if !backup.exists() {
                break;
            }
            backup = path.with_file_name(format!("{name}.bak-keepane-{stamp}-{n}"));
        }
        std::fs::write(&backup, t).with_context(|| format!("back up to {}", backup.display()))?;
        println!("backed up {} to {}", path.display(), backup.display());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, new).with_context(|| format!("write {}", path.display()))
}

/// Change a JSON file the way `f` says, backed up first; a missing or
/// empty file is `{}`. A file that is not JSON (or has comments) is left
/// alone. Whether it changed.
fn edit_json(path: &Path, f: impl FnOnce(Value) -> Result<(Value, bool)>) -> Result<bool> {
    let old = read(path)?;
    let current: Value = match &old {
        Some(t) if !t.trim().is_empty() => serde_json::from_str(t)
            .with_context(|| format!("{} is not plain JSON; left alone (add it by hand)", path.display()))?,
        _ => json!({}),
    };
    let (new, changed) = f(current).with_context(|| format!("{}; left alone", path.display()))?;
    if changed {
        write_backed_up(path, old.as_deref(), &(serde_json::to_string_pretty(&new)? + "\n"))?;
    }
    Ok(changed)
}

/// Whether a file is there and holds keepane's hook.
fn hooked(agent: Agent, p: &Places) -> bool {
    let Ok(Some(t)) = read(&p.hooks) else { return false };
    match agent {
        Agent::Opencode => t.contains(HOOK_MARK),
        Agent::Cursor => {
            serde_json::from_str(&t).ok().and_then(|v| with_cursor_hooks(v).ok()).is_some_and(|(_, changed)| !changed)
        }
        _ => serde_json::from_str(&t)
            .ok()
            .and_then(|v| with_hooks(v, events(agent)).ok())
            .is_some_and(|(_, changed)| !changed),
    }
}

/// Whether keepane's MCP server is registered with the agent.
fn registered(agent: Agent, p: &Places) -> bool {
    let Ok(Some(t)) = read(&p.mcp) else { return false };
    match agent {
        // TOML: a table of its own.
        Agent::Codex => t.lines().any(|l| l.trim() == "[mcp_servers.keepane]"),
        _ => {
            let key = mcp_entry(agent).0;
            match serde_json::from_str::<Value>(&t) {
                Ok(v) => v[key].get("keepane").is_some(),
                // JSONC (opencode's may be), comments and all: by its name.
                Err(_) => t.contains("\"keepane\""),
            }
        }
    }
}

/// An agent's own CLI, run the way Windows runs a `.cmd` shim.
fn run_cli(program: &str, args: &[&str]) -> Result<std::process::ExitStatus> {
    let exe = crate::config::which(program).with_context(|| format!("`{program}` is not on PATH"))?;
    let is_script = exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut c = if is_script {
        let mut c = std::process::Command::new("cmd.exe");
        c.arg("/c").arg(&exe);
        c
    } else {
        std::process::Command::new(&exe)
    };
    Ok(c.args(args).status()?)
}

/// What registers the MCP server for an agent that does it with its own
/// command, or the entry to add to its file.
fn mcp_by_hand(agent: Agent, p: &Places) -> String {
    match agent {
        Agent::Claude => "claude mcp add --scope user keepane -- keepane mcp".into(),
        Agent::Codex => format!(
            "codex mcp add keepane -- keepane mcp\n   (or in {}:\n   [mcp_servers.keepane]\n   command = \"keepane\"\n   args = [\"mcp\"])",
            p.mcp.display()
        ),
        _ => {
            let (key, entry) = mcp_entry(agent);
            let snippet = serde_json::to_string_pretty(&json!({ key: { "keepane": entry } })).unwrap_or_default();
            format!("in {}:\n{snippet}", p.mcp.display())
        }
    }
}

fn hooks_snippet(agent: Agent) -> String {
    let v = match agent {
        Agent::Opencode => return OPENCODE_PLUGIN.to_string(),
        Agent::Cursor => with_cursor_hooks(json!({})).map(|(v, _)| v),
        _ => with_hooks(json!({}), events(agent)).map(|(v, _)| v),
    };
    v.map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()).unwrap_or_default()
}

/// What `setup <agent>` prints without `--install`.
fn print_plan(agent: Agent, p: &Places) {
    let [start, end] = events(agent);
    println!("{} in a keepane pane (work mode ai) needs:", agent.label());
    println!();
    println!("1. A hook that says when it is free ({start} and {end}), in {}:", p.hooks.display());
    println!("{}", hooks_snippet(agent));
    println!();
    println!("2. keepane's MCP server:");
    println!("   {}", mcp_by_hand(agent, p));
    println!();
    if agent == Agent::Codex {
        println!("Codex runs a new hook only once you trust it: in Codex, /hooks, and trust keepane's two.");
    }
    println!("`keepane setup {} --install` does both (each file is backed up first).", agent.word());
}

fn install(agent: Agent, p: &Places) -> Result<()> {
    let [start, end] = events(agent);
    // Whether the MCP server was added along with the hooks (one file).
    let mut together = None;
    let hooks_changed = match agent {
        Agent::Opencode => {
            if read(&p.hooks)?.is_some_and(|t| t.contains(HOOK_MARK)) {
                false
            } else {
                // Its own file: an old one of another making is kept aside.
                let old = read(&p.hooks)?;
                write_backed_up(&p.hooks, old.as_deref(), OPENCODE_PLUGIN)?;
                true
            }
        }
        Agent::Cursor => edit_json(&p.hooks, with_cursor_hooks)?,
        // Hooks and MCP servers in one file (Gemini CLI): one edit, one
        // backup of the file as the user had it.
        _ if p.hooks == p.mcp => {
            let (mut hooks, mut mcp) = (false, false);
            edit_json(&p.hooks, |v| {
                let (v, h) = with_hooks(v, events(agent))?;
                let (v, m) = with_mcp(v, agent)?;
                (hooks, mcp) = (h, m);
                Ok((v, h || m))
            })?;
            together = Some(mcp);
            hooks
        }
        _ => edit_json(&p.hooks, |v| with_hooks(v, events(agent)))?,
    };
    if hooks_changed {
        println!("added the {start} and {end} hooks ({HOOK_COMMAND}) to {}", p.hooks.display());
    } else {
        println!("the hooks are already in {}", p.hooks.display());
    }
    if let Some(added) = together {
        if added {
            println!("registered the keepane MCP server in {}", p.mcp.display());
        } else {
            println!("the keepane MCP server is already registered");
        }
    } else if registered(agent, p) {
        println!("the keepane MCP server is already registered");
    } else {
        match agent {
            Agent::Claude | Agent::Codex => {
                let args: &[&str] = match agent {
                    Agent::Claude => &["mcp", "add", "--scope", "user", "keepane", "--", "keepane", "mcp"],
                    _ => &["mcp", "add", "keepane", "--", "keepane", "mcp"],
                };
                match run_cli(agent.program(), args) {
                    Ok(s) if s.success() => println!("registered the keepane MCP server"),
                    Ok(_) => bail!("`{} mcp add` failed; add it yourself: {}", agent.program(), mcp_by_hand(agent, p)),
                    Err(e) => println!("{e:#}; add the MCP server yourself: {}", mcp_by_hand(agent, p)),
                }
            }
            _ => {
                // A file with comments is theirs to edit: said, not failed.
                match edit_json(&p.mcp, |v| with_mcp(v, agent)) {
                    Ok(_) => println!("registered the keepane MCP server in {}", p.mcp.display()),
                    Err(e) => println!("{e:#}; add the MCP server yourself, {}", mcp_by_hand(agent, p)),
                }
            }
        }
    }
    if agent == Agent::Codex {
        println!("Codex runs a new hook only once you trust it: in Codex, /hooks, and trust keepane's two.");
    }
    Ok(())
}

/// `keepane setup`: each agent, whether it is here and set up.
fn print_status(home: &Path, env: &dyn Fn(&str) -> Option<String>) {
    let yes = |b: bool| if b { "yes" } else { "no" };
    println!("{:<12} {:<13} {:<6} {:<5}", "agent", "program", "hook", "MCP");
    let mut missing = Vec::new();
    for a in AGENTS {
        let p = places(a, home, env);
        let here = crate::config::which(a.program()).is_some();
        let (h, m) = (hooked(a, &p), registered(a, &p));
        println!("{:<12} {:<13} {:<6} {:<5}", a.label(), if here { a.program() } else { "-" }, yes(h), yes(m));
        if (here || h || m) && !(h && m) {
            missing.push(a);
        }
    }
    println!();
    if missing.is_empty() {
        println!(
            "An agent in a pane in work mode ai says when it is free through its hook; `keepane setup <agent>` shows what it adds."
        );
    }
    for a in missing {
        println!("{} is not set up: keepane setup {} --install", a.label(), a.word());
    }
    println!("agents: {}", AGENTS.map(Agent::word).join(" "));
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()).or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

pub fn run(args: &[String]) -> Result<i32> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(home) = home() else { bail!("no home directory") };
    let env = |k: &str| std::env::var(k).ok();
    let usage = || format!("usage: keepane setup [{}] [--install]", AGENTS.map(Agent::word).join("|"));
    match words.as_slice() {
        [] => {
            print_status(&home, &env);
            Ok(0)
        }
        [word] | [word, "--install"] => {
            let Some(agent) = Agent::parse(word) else { bail!("{}", usage()) };
            let p = places(agent, &home, &env);
            if words.len() == 2 {
                install(agent, &p)?;
            } else {
                print_plan(agent, &p);
            }
            Ok(0)
        }
        _ => bail!("{}", usage()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keepane-setup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn backups(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-keepane-"))
            .map(|e| e.path())
            .collect()
    }

    #[test]
    fn hooks_are_added_once_and_nothing_else_moves() {
        let before = json!({
            "model": "opus",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-me" }] }] },
            "env": { "A": "1" },
        });
        let (after, changed) = with_hooks(before.clone(), events(Agent::Claude)).unwrap();
        assert!(changed);
        let keys: Vec<&String> = after.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "env"], "the user's order stays");
        assert_eq!(after["hooks"]["Stop"][0], before["hooks"]["Stop"][0], "their hook stays first");
        assert_eq!(after["hooks"]["Stop"][1]["hooks"][0]["command"], HOOK_COMMAND);
        assert_eq!(after["hooks"]["SessionStart"][0]["hooks"][0]["command"], HOOK_COMMAND);
        let (again, changed) = with_hooks(after.clone(), events(Agent::Claude)).unwrap();
        assert!(!changed, "a second run adds nothing");
        assert_eq!(again, after);
        assert!(with_hooks(json!([]), events(Agent::Claude)).is_err());
        assert!(
            with_hooks(json!({"hooks": {"Stop": {}}}), events(Agent::Claude)).is_err(),
            "a shape it does not know is left alone"
        );
        // A hook written with a full path is keepane's too.
        let full = json!({"hooks": {"AfterAgent": [{"hooks": [{"type": "command", "command": "C:/k/keepane pane-ready -q"}]}]}});
        let (g, _) = with_hooks(full, events(Agent::Gemini)).unwrap();
        assert_eq!(g["hooks"]["AfterAgent"].as_array().unwrap().len(), 1);
        assert_eq!(g["hooks"]["SessionStart"][0]["hooks"][0]["command"], HOOK_COMMAND);
    }

    #[test]
    fn each_agent_gets_its_own_events_and_files() {
        let home = Path::new("/h");
        let codex_home = |k: &str| (k == "CODEX_HOME").then(|| "/c".to_string());
        let files = |a, env: &dyn Fn(&str) -> Option<String>| {
            let p = places(a, home, env);
            (p.hooks, p.mcp)
        };
        assert_eq!(files(Agent::Claude, &no_env), (home.join(".claude/settings.json"), home.join(".claude.json")));
        assert_eq!(files(Agent::Codex, &no_env), (home.join(".codex/hooks.json"), home.join(".codex/config.toml")));
        assert_eq!(files(Agent::Codex, &codex_home).0, Path::new("/c").join("hooks.json"), "CODEX_HOME");
        assert_eq!(files(Agent::Gemini, &no_env).0, home.join(".gemini/settings.json"));
        assert_eq!(files(Agent::Cursor, &no_env), (home.join(".cursor/hooks.json"), home.join(".cursor/mcp.json")));
        assert_eq!(files(Agent::Opencode, &no_env).0, home.join(".config/opencode/plugins/keepane.js"));
        assert_eq!(events(Agent::Codex), ["SessionStart", "Stop"]);
        assert_eq!(events(Agent::Gemini), ["SessionStart", "AfterAgent"]);
        assert_eq!(events(Agent::Cursor), ["sessionStart", "stop"]);
        // The programs, and the words for them.
        assert_eq!(Agent::of_program("C:\\x\\codex.exe"), Some(Agent::Codex));
        assert_eq!(Agent::of_program("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(Agent::of_program("pwsh"), None);
        assert_eq!(Agent::parse("cursor"), Some(Agent::Cursor));
        assert_eq!(Agent::parse("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(Agent::parse("vim"), None);
    }

    #[test]
    fn cursor_hooks_and_mcp_entries_have_their_own_shapes() {
        let (c, changed) = with_cursor_hooks(json!({"hooks": {"stop": [{"command": "./notify.sh"}]}})).unwrap();
        assert!(changed);
        assert_eq!(c["version"], 1);
        assert_eq!(c["hooks"]["stop"][0]["command"], "./notify.sh", "theirs stays first");
        assert_eq!(c["hooks"]["stop"][1]["command"], HOOK_COMMAND);
        assert_eq!(c["hooks"]["sessionStart"][0]["command"], HOOK_COMMAND);
        assert!(!with_cursor_hooks(c).unwrap().1, "a second run adds nothing");
        let (g, changed) = with_mcp(json!({"mcpServers": {"other": {}}}), Agent::Gemini).unwrap();
        assert!(changed);
        assert_eq!(g["mcpServers"]["keepane"], json!({"command": "keepane", "args": ["mcp"]}));
        assert!(g["mcpServers"].get("other").is_some());
        let (o, _) = with_mcp(json!({"$schema": "x"}), Agent::Opencode).unwrap();
        assert_eq!(o["mcp"]["keepane"]["command"], json!(["keepane", "mcp"]));
        // One of theirs named keepane is left as they have it.
        let mine = json!({"mcpServers": {"keepane": {"command": "C:/k/keepane.exe", "args": ["mcp"]}}});
        assert_eq!(with_mcp(mine.clone(), Agent::Cursor).unwrap(), (mine, false));
    }

    #[test]
    fn install_backs_up_and_writes_only_what_changes() {
        let dir = temp("claude");
        let settings = dir.join("settings.json");
        std::fs::write(&settings, "{\"model\": \"opus\"}").unwrap();
        assert!(edit_json(&settings, |v| with_hooks(v, events(Agent::Claude))).unwrap());
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(written["model"], "opus");
        assert_eq!(written["hooks"]["Stop"][0]["hooks"][0]["command"], HOOK_COMMAND);
        let b = backups(&dir);
        assert_eq!(b.len(), 1);
        assert_eq!(std::fs::read_to_string(&b[0]).unwrap(), "{\"model\": \"opus\"}");
        assert!(!edit_json(&settings, |v| with_hooks(v, events(Agent::Claude))).unwrap(), "nothing new");
        assert_eq!(backups(&dir).len(), 1, "and no second backup");
        // Two backups within a second: the first is kept, the second numbered.
        write_backed_up(&settings, Some("one"), "x").unwrap();
        write_backed_up(&settings, Some("two"), "y").unwrap();
        let mut kept: Vec<String> = backups(&dir).iter().map(|b| std::fs::read_to_string(b).unwrap()).collect();
        kept.sort();
        assert!(kept.len() == 3 && kept.contains(&"one".to_string()) && kept.contains(&"two".to_string()), "{kept:?}");
        // Broken JSON (or JSON with comments) is never overwritten.
        std::fs::write(&settings, "{ // mine\n}").unwrap();
        assert!(edit_json(&settings, |v| with_hooks(v, events(Agent::Claude))).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ // mine\n}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every agent but the two registered through their own CLI, installed
    /// into an empty home: its files written, then found set up; a second
    /// install changes nothing.
    #[test]
    fn install_sets_up_gemini_cursor_and_opencode() {
        let home = temp("home");
        for a in [Agent::Gemini, Agent::Cursor, Agent::Opencode] {
            let p = places(a, &home, &no_env);
            assert!(!hooked(a, &p) && !registered(a, &p), "{a:?} starts bare");
            install(a, &p).unwrap();
            assert!(hooked(a, &p), "{a:?} hook");
            assert!(registered(a, &p), "{a:?} MCP");
            let before = std::fs::read_to_string(&p.hooks).unwrap();
            install(a, &p).unwrap();
            assert_eq!(std::fs::read_to_string(&p.hooks).unwrap(), before, "{a:?}: a second run changes nothing");
        }
        let plugin = std::fs::read_to_string(home.join(".config/opencode/plugins/keepane.js")).unwrap();
        assert_eq!(plugin, OPENCODE_PLUGIN);
        // Gemini CLI keeps both in one file: one backup, of the file as it was.
        let gemini_dir = home.join(".gemini");
        assert!(backups(&gemini_dir).is_empty(), "there was no file to back up");
        let settings = gemini_dir.join("settings.json");
        std::fs::write(&settings, "{ \"theme\": \"dark\" }").unwrap();
        install(Agent::Gemini, &places(Agent::Gemini, &home, &no_env)).unwrap();
        let b = backups(&gemini_dir);
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(
            std::fs::read_to_string(&b[0]).unwrap(),
            "{ \"theme\": \"dark\" }",
            "the user's own, not a half-way one"
        );
        let gemini: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(gemini["theme"], "dark");
        assert_eq!(gemini["hooks"]["AfterAgent"][0]["hooks"][0]["command"], HOOK_COMMAND);
        assert_eq!(gemini["mcpServers"]["keepane"]["args"], json!(["mcp"]));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Claude Code's own directory (`CLAUDE_CONFIG_DIR`) holds both files;
    /// opencode's JSONC config, with comments, is found and left alone.
    #[test]
    fn claude_config_dir_and_opencode_jsonc() {
        let conf = |k: &str| (k == "CLAUDE_CONFIG_DIR").then(|| "/cc".to_string());
        let p = places(Agent::Claude, Path::new("/h"), &conf);
        assert_eq!((p.hooks, p.mcp), (Path::new("/cc").join("settings.json"), Path::new("/cc").join(".claude.json")));
        let home = temp("jsonc");
        let dir = home.join(".config/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let jsonc = "{\n  // mine\n  \"theme\": \"x\"\n}\n";
        std::fs::write(dir.join("opencode.jsonc"), jsonc).unwrap();
        let p = places(Agent::Opencode, &home, &no_env);
        assert_eq!(p.mcp, dir.join("opencode.jsonc"));
        install(Agent::Opencode, &p).unwrap();
        assert!(hooked(Agent::Opencode, &p), "the plugin is written all the same");
        assert_eq!(std::fs::read_to_string(dir.join("opencode.jsonc")).unwrap(), jsonc, "comments kept: not touched");
        assert!(!dir.join("opencode.json").exists(), "and no second config beside it");
        assert!(!registered(Agent::Opencode, &p));
        std::fs::write(dir.join("opencode.jsonc"), "{ // mine\n \"mcp\": { \"keepane\": {} } }").unwrap();
        assert!(registered(Agent::Opencode, &p), "added by hand, found by its name");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Codex's MCP servers are TOML tables; `setup` reads, never writes, it.
    #[test]
    fn codex_is_registered_by_its_table() {
        let dir = temp("codex");
        let p = places(Agent::Codex, &dir, &no_env);
        std::fs::create_dir_all(p.mcp.parent().unwrap()).unwrap();
        std::fs::write(&p.mcp, "notify = [\"x\"]\n[mcp_servers.node_repl]\ncommand = 'n'\n").unwrap();
        assert!(!registered(Agent::Codex, &p));
        std::fs::write(&p.mcp, "[mcp_servers.node_repl]\n\n[mcp_servers.keepane]\ncommand = \"keepane\"\n").unwrap();
        assert!(registered(Agent::Codex, &p));
        // Its hooks go in hooks.json; config.toml (and its notify) untouched.
        let toml = std::fs::read_to_string(&p.mcp).unwrap();
        edit_json(&p.hooks, |v| with_hooks(v, events(Agent::Codex))).unwrap();
        assert!(hooked(Agent::Codex, &p));
        assert_eq!(std::fs::read_to_string(&p.mcp).unwrap(), toml);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
