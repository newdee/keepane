//! `keepane startup on|off|status` on macOS and Linux: start the server at
//! login and bring every saved session back, so after a reboot there is
//! nothing to do but attach. macOS: a launchd agent,
//! `~/Library/LaunchAgents/tech.dfine.keepane.plist`. Linux: a systemd user
//! unit, `~/.config/systemd/user/keepane.service`, enabled for
//! `default.target` by the link `systemctl --user enable` would make. Both
//! run the server directly, `keepane -L <socket> __server --restore`, as the
//! Windows `Run` value does. Neither is started when it is written: a server
//! most likely runs already, and the next login is what it is for (a second
//! server on the same socket would only fail to listen).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::takeover::{self, Version};

/// launchd's name for the agent (with `.<socket>` for another socket).
const LABEL: &str = "tech.dfine.keepane";

/// What starts the server at login.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Manager {
    /// macOS: a launchd agent.
    Launchd,
    /// Linux: a systemd user unit.
    Systemd,
}

impl Manager {
    fn here() -> Manager {
        if cfg!(target_os = "macos") { Manager::Launchd } else { Manager::Systemd }
    }

    /// The folder its file goes in.
    fn dir(self) -> Result<PathBuf> {
        Ok(match self {
            Manager::Launchd => dirs::home_dir().context("no home directory")?.join("Library/LaunchAgents"),
            Manager::Systemd => dirs::config_dir().context("no config directory")?.join("systemd/user"),
        })
    }

    /// The file of `socket`'s, one per socket so two servers can both come up.
    fn file_name(self, socket: &str) -> String {
        match (self, socket) {
            (Manager::Launchd, "default") => format!("{LABEL}.plist"),
            (Manager::Launchd, s) => format!("{LABEL}.{s}.plist"),
            (Manager::Systemd, "default") => "keepane.service".to_string(),
            (Manager::Systemd, s) => format!("keepane-{s}.service"),
        }
    }
}

fn label(socket: &str) -> String {
    if socket == "default" { LABEL.to_string() } else { format!("{LABEL}.{socket}") }
}

/// A socket name goes into a file name, a launchd label and a systemd unit
/// name: letters, digits, `-`, `_` and `.` only, and not first a `.`.
fn check_socket(socket: &str) -> Result<()> {
    if socket.is_empty()
        || socket.starts_with('.')
        || !socket.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("startup: the socket name '{socket}' cannot name a file here (letters, digits, - _ and . only)");
    }
    Ok(())
}

/// What the server gets from the environment `startup on` runs in: where
/// keepane keeps things, and what a pane starts with. A launchd agent gets a
/// bare PATH and no LANG, and a systemd unit the user manager's
/// environment, not the login shell's: panes would not find what the shell
/// finds (Homebrew's programs), nor speak UTF-8, nor the client find the
/// server when `KEEPANE_TMPDIR` or `XDG_RUNTIME_DIR` moves its socket
/// (`kept_env` always writes the first).
const KEPT_ENV: [&str; 13] = [
    "PATH",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "KEEPANE_TMPDIR",
    "KEEPANE_CONFIG",
    "KEEPANE_SESSIONS_DIR",
    "KEEPANE_LOG_DIR",
    "KEEPANE_HISTORY_DIR",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
];

fn kept_env() -> Vec<(String, String)> {
    // The sockets' folder as this client finds it, said outright: the
    // server would look for `XDG_RUNTIME_DIR` itself, which systemd gives a
    // unit and a login shell may not have, and the two would not meet.
    let base = super::ipc::socket_base().to_string_lossy().into_owned();
    kept_from(|k| if k == "KEEPANE_TMPDIR" { Some(base.clone()) } else { std::env::var(k).ok() })
}

/// `KEPT_ENV` as `var` gives them. One empty, or with a line break or another
/// control character (which neither a unit's line nor a plist's XML can
/// hold), is left out.
fn kept_from(var: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    KEPT_ENV.iter().filter_map(|k| var(k).filter(|v| !v.is_empty() && fits(v)).map(|v| (k.to_string(), v))).collect()
}

/// Whether a unit's line and a plist's XML can hold `s`: no line break or
/// other control character.
fn fits(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// The path to register for `me`: the name on `path` that is this same
/// program, when there is one (Homebrew's `bin/keepane`, a link each
/// upgrade points at the new version, rather than this version's folder in
/// the Cellar, which `brew cleanup` deletes); else `me`.
fn stable_exe(me: &Path, path: Option<OsString>) -> PathBuf {
    let Some(name) = me.file_name() else { return me.to_path_buf() };
    path.iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join(name))
        .find(|c| c.is_absolute() && takeover::same_file(c, me))
        .unwrap_or_else(|| me.to_path_buf())
}

// --- launchd ---------------------------------------------------------------

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn unxml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn plist(socket: &str, exe: &str, env: &[(String, String)]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    s += &format!("\t<key>Label</key>\n\t<string>{}</string>\n", xml(&label(socket)));
    s += "\t<key>ProgramArguments</key>\n\t<array>\n";
    for a in [exe, "-L", socket, "__server", "--restore"] {
        s += &format!("\t\t<string>{}</string>\n", xml(a));
    }
    s += "\t</array>\n";
    if !env.is_empty() {
        s += "\t<key>EnvironmentVariables</key>\n\t<dict>\n";
        for (k, v) in env {
            s += &format!("\t\t<key>{}</key>\n\t\t<string>{}</string>\n", xml(k), xml(v));
        }
        s += "\t</dict>\n";
    }
    s += "\t<key>RunAtLoad</key>\n\t<true/>\n";
    // Not the light limits launchd puts on a background job: panes are what
    // someone types into.
    s += "\t<key>ProcessType</key>\n\t<string>Interactive</string>\n";
    // What the server started stays when it ends, as it does when a
    // terminal started it.
    s += "\t<key>AbandonProcessGroup</key>\n\t<true/>\n";
    s + "</dict>\n</plist>\n"
}

/// Where the program's path is in a plist: the first string of
/// `ProgramArguments`, as a byte range.
fn plist_program(text: &str) -> Option<(usize, usize)> {
    let at = text.find("<key>ProgramArguments</key>")?;
    let start = at + text[at..].find("<string>")? + "<string>".len();
    let len = text[start..].find("</string>")?;
    Some((start, start + len))
}

// --- systemd ---------------------------------------------------------------

/// A word of a systemd command line, quoted: `\` and `"` escaped, and `%` (a
/// specifier) and `$` (a variable) doubled.
fn exec_word(s: &str) -> String {
    let s = s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{s}\"")
}

/// An `Environment=` assignment, quoted: as `exec_word`, but `$` means
/// nothing there.
fn env_word(k: &str, v: &str) -> String {
    let s = format!("{k}={v}").replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    format!("\"{s}\"")
}

/// The first word of a systemd command line, read back, and where it ends.
fn exec_first_word(line: &str) -> Option<(String, usize)> {
    let quoted = line.starts_with('"');
    let mut out = String::new();
    let mut chars = line.char_indices().skip(usize::from(quoted)).peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' if quoted => return Some((out, i + 1)),
            c if !quoted && c.is_whitespace() => return Some((out, i)),
            '\\' => out.push(chars.next()?.1),
            '%' | '$' if chars.peek().is_some_and(|&(_, n)| n == c) => {
                chars.next();
                out.push(c);
            }
            c => out.push(c),
        }
    }
    (!quoted).then_some((out, line.len()))
}

fn unit(socket: &str, exe: &str, env: &[(String, String)]) -> String {
    let which = if socket == "default" { String::new() } else { format!(" (socket {socket})") };
    let mut s = format!(
        "[Unit]\nDescription=keepane server{which}, every saved session restored\n\
         Documentation=https://github.com/newdee/keepane\n\n[Service]\n\
         ExecStart={} -L {socket} __server --restore\n",
        exec_word(exe)
    );
    for (k, v) in env {
        s += &format!("Environment={}\n", env_word(k, v));
    }
    // When the server goes, only it: the copy of `restart-server` that goes
    // on outside a pane, and the new server it starts, are in this unit's
    // group too, and stopping the whole group would take them with it.
    s += "KillMode=process\n\n[Install]\nWantedBy=default.target\n";
    s
}

/// Where the program's path is in a unit: the first word of `ExecStart=`,
/// as a byte range.
fn unit_program(text: &str) -> Option<(usize, usize)> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let lead = line.len() - line.trim_start().len();
        if let Some(cmd) = line[lead..].strip_prefix("ExecStart=") {
            let start = at + lead + "ExecStart=".len();
            let (_, end) = exec_first_word(cmd)?;
            return Some((start, start + end));
        }
        at += line.len();
    }
    None
}

// --- both ------------------------------------------------------------------

fn program(m: Manager, text: &str) -> Option<PathBuf> {
    let p = match m {
        Manager::Launchd => plist_program(text).map(|(a, b)| unxml(&text[a..b])),
        Manager::Systemd => unit_program(text).and_then(|(a, b)| exec_first_word(&text[a..b]).map(|w| w.0)),
    }?;
    Some(PathBuf::from(p)).filter(|p| !p.as_os_str().is_empty())
}

/// `text` with `exe` as the program and nothing else changed (what the user
/// edited in it stays).
fn with_program(m: Manager, text: &str, exe: &str) -> Option<String> {
    if !fits(exe) {
        return None;
    }
    let ((a, b), word) = match m {
        Manager::Launchd => (plist_program(text)?, xml(exe)),
        Manager::Systemd => (unit_program(text)?, exec_word(exe)),
    };
    Some(format!("{}{word}{}", &text[..a], &text[b..]))
}

/// The link that enables a systemd unit for `default.target`.
fn wants_link(dir: &Path, socket: &str) -> PathBuf {
    dir.join("default.target.wants").join(Manager::Systemd.file_name(socket))
}

/// A registration found: its file, the program it runs, and whether it is
/// on (a systemd unit can be there and not enabled).
struct Registered {
    file: PathBuf,
    exe: PathBuf,
    enabled: bool,
}

fn status_in(m: Manager, dir: &Path, socket: &str) -> Result<Option<Registered>> {
    check_socket(socket)?;
    let file = dir.join(m.file_name(socket));
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", file.display())),
    };
    let exe = program(m, &text).with_context(|| format!("{}: no program to run in it", file.display()))?;
    let enabled = m == Manager::Launchd || wants_link(dir, socket).symlink_metadata().is_ok();
    Ok(Some(Registered { file, exe, enabled }))
}

/// Written whole or not at all: a file half written would start nothing.
/// The temp file is this process's own: two keepanes started at once
/// after an update both take the registration over.
fn write_file(file: &Path, text: &str) -> Result<()> {
    let name = file.file_name().context("no file name")?.to_string_lossy();
    let tmp = file.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::write(&tmp, text)
        .with_context(|| format!("write {}", tmp.display()))
        .and_then(|()| std::fs::rename(&tmp, file).with_context(|| format!("write {}", file.display())));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn install_in(m: Manager, dir: &Path, socket: &str, exe: &str, env: &[(String, String)]) -> Result<String> {
    check_socket(socket)?;
    if !fits(exe) {
        bail!(
            "startup: keepane's path has a control character in it, which {} cannot hold: {exe:?}",
            m.file_name(socket)
        );
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let file = dir.join(m.file_name(socket));
    let text = match m {
        Manager::Launchd => plist(socket, exe, env),
        Manager::Systemd => unit(socket, exe, env),
    };
    write_file(&file, &text)?;
    if m == Manager::Systemd {
        let link = wants_link(dir, socket);
        if let Some(d) = link.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&file, &link).with_context(|| format!("link {}", link.display()))?;
    }
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    let with = if names.is_empty() { String::new() } else { format!("\nwith:  {} as they are now", names.join(", ")) };
    let linger = if m == Manager::Systemd {
        "\nIt stops at logout (saving first) unless `loginctl enable-linger` keeps your\n\
         user manager running; then it also starts at boot."
    } else {
        ""
    };
    Ok(format!(
        "keepane will start at login and restore every saved session,\n\
         and serve the phone page again if it was on (`keepane web stop` turns that off).\n\
         where: {}\n\
         runs:  {exe} -L {socket} __server --restore{with}{linger}\n\
         `keepane startup off` removes it; `keepane startup status` shows it.",
        file.display()
    ))
}

fn remove_in(m: Manager, dir: &Path, socket: &str) -> Result<String> {
    check_socket(socket)?;
    let file = dir.join(m.file_name(socket));
    let mut removed = false;
    let mut gone = |p: &Path| match std::fs::remove_file(p) {
        Ok(()) => {
            removed = true;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", p.display())),
    };
    if m == Manager::Systemd {
        gone(&wants_link(dir, socket))?;
    }
    gone(&file)?;
    Ok(if removed {
        format!("{}: removed; keepane will not start at login", file.display())
    } else {
        format!("{}: not installed", file.display())
    })
}

/// Whether systemd runs this machine (what `sd_booted` asks).
fn systemd_runs() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

/// systemd reads a unit file again now rather than at the next login; a
/// user manager out of reach (no session bus) reads it then anyway.
fn reload(m: Manager) {
    if m == Manager::Systemd && systemd_runs() {
        use std::process::Stdio;
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// `keepane startup [on|off|status]`, run on the client side: nothing here
/// needs a server, and the server is exactly what this is about starting.
pub fn run(socket: &str, args: &[String]) -> Result<i32> {
    // One verb and nothing after it: a typo must not look like it worked.
    if args.len() > 1 {
        bail!("startup: unexpected argument '{}'", args[1]);
    }
    let m = Manager::here();
    match args.first().map(String::as_str) {
        Some("on") | Some("install") | Some("enable") => {
            if m == Manager::Systemd && !systemd_runs() {
                bail!(
                    "startup: systemd does not run here (a container, WSL without systemd), so nothing \
                     would start the unit: start the server from your login files instead"
                );
            }
            let me = std::env::current_exe().context("current_exe")?;
            let exe = stable_exe(&me, std::env::var_os("PATH"));
            let msg = install_in(m, &m.dir()?, socket, &exe.to_string_lossy(), &kept_env())?;
            reload(m);
            println!("{msg}");
            Ok(0)
        }
        Some("off") | Some("uninstall") | Some("disable") => {
            let msg = remove_in(m, &m.dir()?, socket)?;
            reload(m);
            println!("{msg}");
            Ok(0)
        }
        None | Some("status") => {
            let dir = m.dir()?;
            match status_in(m, &dir, socket)? {
                Some(r) if r.enabled => {
                    println!(
                        "{}: installed, runs at login:\n  {} -L {socket} __server --restore",
                        r.file.display(),
                        r.exe.display()
                    );
                    Ok(0)
                }
                Some(r) => {
                    println!(
                        "{}: there, but not enabled for default.target (`keepane startup on` enables it)",
                        r.file.display()
                    );
                    Ok(1)
                }
                None => {
                    println!(
                        "{}: not installed (`keepane startup on` to start at login)",
                        dir.join(m.file_name(socket)).display()
                    );
                    Ok(1)
                }
            }
        }
        Some(other) => bail!("startup: expected on, off or status, got '{other}'"),
    }
}

/// The logon command of `socket`, moved to this keepane where it runs an
/// older one or one that is gone (crate::takeover); a line when it was.
/// Only from an installed keepane (`takeover::installed_self`), and by the
/// name on the PATH that is this program (`stable_exe`).
pub fn refresh_registrations(socket: &str) -> Vec<String> {
    let Some(me) = takeover::installed_self() else { return Vec::new() };
    let m = Manager::here();
    let Ok(dir) = m.dir() else { return Vec::new() };
    let me = stable_exe(&me, std::env::var_os("PATH"));
    let cache = takeover::cache_file();
    refresh_with(m, &dir, socket, &me, takeover::this_version(), |exe| {
        takeover::version_cached(exe, &cache, takeover::version_of)
    })
}

/// `refresh_registrations`, with the folder, this program, its version and
/// the way to ask a version given (the tests give their own).
fn refresh_with(
    m: Manager,
    dir: &Path,
    socket: &str,
    me: &Path,
    mine: Version,
    version_of: impl Fn(&Path) -> Option<Version>,
) -> Vec<String> {
    let Ok(Some(r)) = status_in(m, dir, socket) else { return Vec::new() };
    let Some(why) = takeover::stale(&r.exe, me, mine, version_of) else { return Vec::new() };
    let moved = || -> Result<()> {
        let text = std::fs::read_to_string(&r.file).with_context(|| format!("read {}", r.file.display()))?;
        let text = with_program(m, &text, &me.to_string_lossy()).context("no program to run in it")?;
        write_file(&r.file, &text)
    };
    vec![match moved() {
        Ok(()) => takeover::said("start at login", me, &r.exe, &why),
        Err(e) => format!("start at login still runs {}: {e:#}", r.exe.display()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Manager; 2] = [Manager::Launchd, Manager::Systemd];

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keepane-startup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A path with every character the two formats escape.
    const AWKWARD: &str = "/tmp/a & <b> \"c\" 'd' %h $HOME \\e/keepane";

    #[test]
    fn names_and_socket_names() {
        assert_eq!(Manager::Launchd.file_name("default"), "tech.dfine.keepane.plist");
        assert_eq!(Manager::Launchd.file_name("work"), "tech.dfine.keepane.work.plist");
        assert_eq!(Manager::Systemd.file_name("default"), "keepane.service");
        assert_eq!(Manager::Systemd.file_name("work"), "keepane-work.service");
        assert_eq!(label("default"), "tech.dfine.keepane");
        assert_eq!(label("e2e-1"), "tech.dfine.keepane.e2e-1");
        for ok in ["default", "work", "e2e-1.2_x"] {
            assert!(check_socket(ok).is_ok(), "{ok}");
        }
        for bad in ["", "../x", "a/b", "a b", ".hidden", "a%i", "ä"] {
            assert!(check_socket(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Manager::here(), if cfg!(target_os = "macos") { Manager::Launchd } else { Manager::Systemd });
    }

    #[test]
    fn the_agent_runs_the_server_with_the_environment_given() {
        let p = plist("default", AWKWARD, &env(&[("PATH", "/opt/homebrew/bin:/usr/bin"), ("LANG", "en_US.UTF-8")]));
        let args = [
            "<string>/tmp/a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos; %h $HOME \\e/keepane</string>",
            "<string>-L</string>",
            "<string>default</string>",
            "<string>__server</string>",
            "<string>--restore</string>",
        ];
        let mut at = p.find("<key>ProgramArguments</key>").unwrap();
        for a in args {
            at += p[at..].find(a).unwrap_or_else(|| panic!("{a} in order:\n{p}"));
        }
        for part in [
            "<key>Label</key>\n\t<string>tech.dfine.keepane</string>",
            "<key>PATH</key>\n\t\t<string>/opt/homebrew/bin:/usr/bin</string>",
            "<key>LANG</key>\n\t\t<string>en_US.UTF-8</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>ProcessType</key>\n\t<string>Interactive</string>",
        ] {
            assert!(p.contains(part), "{part}\n{p}");
        }
        assert_eq!(program(Manager::Launchd, &p).unwrap(), Path::new(AWKWARD));
        assert!(!plist("default", "/k", &[]).contains("EnvironmentVariables"), "none given, none written");
        assert!(plist("work", "/k", &[]).contains("<string>tech.dfine.keepane.work</string>"));
    }

    #[test]
    fn the_unit_runs_the_server_with_the_environment_given() {
        let u = unit("default", AWKWARD, &env(&[("PATH", "/a b:/c%d$e"), ("LANG", "C.UTF-8")]));
        let exec = u.lines().find(|l| l.starts_with("ExecStart=")).unwrap();
        assert_eq!(exec, r#"ExecStart="/tmp/a & <b> \"c\" 'd' %%h $$HOME \\e/keepane" -L default __server --restore"#);
        assert!(u.contains("Environment=\"PATH=/a b:/c%%d$e\"\n"), "{u}");
        assert!(u.contains("Environment=\"LANG=C.UTF-8\"\n"), "{u}");
        for part in ["KillMode=process\n", "[Install]\nWantedBy=default.target\n", "Description=keepane server, "] {
            assert!(u.contains(part), "{part}\n{u}");
        }
        assert!(unit("work", "/k", &[]).contains("Description=keepane server (socket work), "));
        assert_eq!(program(Manager::Systemd, &u).unwrap(), Path::new(AWKWARD));
        // As a person may write it: unquoted, indented, after other lines.
        let hand = "[Service]\n  ExecStart=/usr/local/bin/keepane -L default __server --restore\n";
        assert_eq!(program(Manager::Systemd, hand).unwrap(), Path::new("/usr/local/bin/keepane"));
        assert_eq!(program(Manager::Systemd, "[Service]\nExecStart=\"/unclosed\n"), None);
        assert_eq!(program(Manager::Systemd, "[Service]\nType=simple\n"), None);
        assert_eq!(program(Manager::Launchd, "<plist><dict></dict></plist>"), None);
    }

    /// Taking a registration over changes the program and nothing else.
    #[test]
    fn only_the_program_is_changed() {
        let given = env(&[("PATH", "/x")]);
        for m in BOTH {
            let text = match m {
                Manager::Launchd => plist("default", "/old/keepane", &given),
                Manager::Systemd => unit("default", "/old/keepane", &given),
            };
            let moved = with_program(m, &text, AWKWARD).unwrap();
            assert_eq!(program(m, &moved).unwrap(), Path::new(AWKWARD), "{m:?}");
            let back = with_program(m, &moved, "/old/keepane").unwrap();
            assert_eq!(back, text, "{m:?}: the rest stays as it was");
        }
        let hand = "[Service]\n  ExecStart=/usr/bin/keepane -L default __server --restore\nKillMode=process\n";
        assert_eq!(
            with_program(Manager::Systemd, hand, "/new/keepane").unwrap(),
            "[Service]\n  ExecStart=\"/new/keepane\" -L default __server --restore\nKillMode=process\n"
        );
    }

    /// In a folder of the test's own: on, status, off, status, off again;
    /// for systemd, the unit with its link and without.
    #[test]
    fn install_status_remove_round_trip() {
        for m in BOTH {
            let dir = scratch(&format!("{m:?}"));
            let sock = "unit";
            assert!(status_in(m, &dir, sock).unwrap().is_none());
            let msg =
                install_in(m, &dir, sock, "/opt/k/keepane", &env(&[("PATH", "/x"), ("SHELL", "/bin/zsh")])).unwrap();
            assert!(msg.contains("runs:  /opt/k/keepane -L unit __server --restore"), "{msg}");
            assert!(msg.contains("with:  PATH, SHELL as they are now"), "{msg}");
            assert_eq!(msg.contains("enable-linger"), m == Manager::Systemd, "{msg}");
            let r = status_in(m, &dir, sock).unwrap().expect("installed");
            assert_eq!(r.exe, Path::new("/opt/k/keepane"));
            assert_eq!(r.file, dir.join(m.file_name(sock)));
            assert!(r.enabled);
            if m == Manager::Systemd {
                let link = wants_link(&dir, sock);
                assert_eq!(std::fs::read_link(&link).unwrap(), r.file);
                std::fs::remove_file(&link).unwrap();
                assert!(!status_in(m, &dir, sock).unwrap().unwrap().enabled, "disabled by hand");
            }
            // Again over it: replaced, enabled again, no file left behind.
            install_in(m, &dir, sock, "/opt/k2/keepane", &[]).unwrap();
            let r = status_in(m, &dir, sock).unwrap().unwrap();
            assert!(r.enabled && r.exe == Path::new("/opt/k2/keepane"));
            let tmp_left =
                std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp"));
            assert!(!tmp_left);
            assert!(remove_in(m, &dir, sock).unwrap().contains("removed"));
            assert!(status_in(m, &dir, sock).unwrap().is_none());
            assert!(!wants_link(&dir, sock).exists());
            assert!(remove_in(m, &dir, sock).unwrap().contains("not installed"), "removing twice is not an error");
            assert!(install_in(m, &dir, "../out", "/k", &[]).is_err(), "a socket name that leaves the folder");
            let e = install_in(m, &dir, sock, "/a\nb/keepane", &[]).unwrap_err().to_string();
            assert!(e.contains("control character"), "{e}");
            assert!(status_in(m, &dir, sock).unwrap().is_none(), "nothing written");
            let text = install_in(m, &dir, sock, "/k", &[])
                .and_then(|_| Ok(std::fs::read_to_string(dir.join(m.file_name(sock)))?));
            assert_eq!(
                with_program(m, &text.unwrap(), "/a\nb/keepane"),
                None,
                "not taken over to a path a file cannot hold"
            );
            remove_in(m, &dir, sock).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The variables kept, in their order; an empty one, one not set, and one
    /// a file cannot hold are left out.
    #[test]
    fn the_environment_kept() {
        let kept = kept_from(|k| match k {
            "PATH" => Some("/opt/homebrew/bin:/usr/bin".into()),
            "LANG" => Some("en_US.UTF-8".into()),
            "SHELL" => Some(String::new()),
            "LC_ALL" => Some("a\nb".into()),
            "KEEPANE_TMPDIR" => Some("/t\u{7}".into()),
            "XDG_DATA_HOME" => Some("/d".into()),
            "HOME" | "KEEPANE" => Some("/never".into()),
            _ => None,
        });
        assert_eq!(
            kept,
            env(&[("PATH", "/opt/homebrew/bin:/usr/bin"), ("LANG", "en_US.UTF-8"), ("XDG_DATA_HOME", "/d")])
        );
    }

    /// What may already be where the files go: a folder in the file's place
    /// is an error to `status` and `on`, and nothing to the takeover; a wants
    /// link that is dangling or a plain file is replaced.
    #[test]
    fn odd_things_where_the_files_go() {
        let dir = scratch("odd");
        let sock = "unit";
        for m in BOTH {
            std::fs::create_dir_all(dir.join(m.file_name(sock))).unwrap();
            assert!(status_in(m, &dir, sock).is_err(), "{m:?}");
            assert!(install_in(m, &dir, sock, "/k", &[]).is_err(), "{m:?}");
            let left =
                std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp"));
            assert!(!left, "{m:?}: nothing left behind");
            assert!(refresh_with(m, &dir, sock, Path::new("/new/keepane"), (9, 9, 9), |_: &Path| None).is_empty());
            std::fs::remove_dir(dir.join(m.file_name(sock))).unwrap();
        }
        let link = wants_link(&dir, sock);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/nowhere/keepane-unit.service", &link).unwrap();
        install_in(Manager::Systemd, &dir, sock, "/k", &[]).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), dir.join("keepane-unit.service"), "a dangling link replaced");
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "a plain file").unwrap();
        install_in(Manager::Systemd, &dir, sock, "/k", &[]).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), dir.join("keepane-unit.service"), "a plain file replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_verb_is_refused_without_touching_anything() {
        let e = run("default", &["sideways".to_string()]).unwrap_err().to_string();
        assert!(e.contains("expected on, off or status"), "{e}");
        let e = run("default", &["on".to_string(), "extra".to_string()]).unwrap_err().to_string();
        assert!(e.contains("unexpected argument 'extra'"), "{e}");
    }

    /// Homebrew's `bin/keepane` link is registered rather than the Cellar
    /// file it points at; with no such name on the PATH, the program itself.
    #[test]
    fn the_name_on_the_path_is_registered() {
        let dir = scratch("stable");
        let cellar = dir.join("Cellar/keepane/0.39.0/bin");
        std::fs::create_dir_all(&cellar).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let me = cellar.join("keepane");
        std::fs::write(&me, b"k").unwrap();
        std::os::unix::fs::symlink("../Cellar/keepane/0.39.0/bin/keepane", dir.join("bin/keepane")).unwrap();
        std::fs::write(dir.join("other/keepane"), b"another keepane").unwrap();
        let path = |dirs: &[&str]| Some(std::env::join_paths(dirs.iter().map(|d| dir.join(d))).unwrap());
        assert_eq!(stable_exe(&me, path(&["nothing", "other", "bin"])), dir.join("bin/keepane"));
        assert_eq!(stable_exe(&me, path(&["other"])), me, "another keepane is not this one");
        assert_eq!(stable_exe(&me, None), me);
        let relative = Some(OsString::from("bin"));
        assert_eq!(stable_exe(&me, relative), me, "a relative PATH entry is not registered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The registration, for real in a folder of the test's own: one that
    /// runs a keepane that is gone or older is moved to this one, the rest
    /// of it kept; a newer one stays; none is not made.
    #[test]
    fn a_stale_registration_moves_to_this_keepane_and_no_other() {
        for m in BOTH {
            let dir = scratch(&format!("refresh-{m:?}"));
            let sock = "unit";
            let me = Path::new("/new/bin/keepane");
            let mine = (0, 39, 0);
            let says = |v: Option<Version>| move |_: &Path| v;
            assert!(refresh_with(m, &dir, sock, me, mine, says(Some((0, 1, 0)))).is_empty());
            assert!(status_in(m, &dir, sock).unwrap().is_none(), "none made");

            let gone = "/keepane-gone-for-the-test/keepane";
            install_in(m, &dir, sock, gone, &env(&[("PATH", "/kept")])).unwrap();
            let said = refresh_with(m, &dir, sock, me, mine, says(None));
            assert_eq!(said, vec![format!("start at login now runs /new/bin/keepane (it ran {gone}, which is gone)")]);
            let text = std::fs::read_to_string(dir.join(m.file_name(sock))).unwrap();
            assert_eq!(program(m, &text).unwrap(), me);
            assert!(text.contains("/kept"), "{m:?}: the rest kept\n{text}");
            assert!(refresh_with(m, &dir, sock, me, mine, says(Some((0, 1, 0)))).is_empty(), "nothing left to do");

            let here = std::env::current_exe().unwrap();
            install_in(m, &dir, sock, &here.to_string_lossy(), &[]).unwrap();
            let said = refresh_with(m, &dir, sock, me, mine, says(Some((0, 38, 1))));
            assert!(said.len() == 1 && said[0].ends_with("version 0.38.1)"), "{said:?}");
            install_in(m, &dir, sock, &here.to_string_lossy(), &[]).unwrap();
            assert!(refresh_with(m, &dir, sock, me, mine, says(Some((0, 40, 0)))).is_empty(), "a newer one stays");
            assert_eq!(status_in(m, &dir, sock).unwrap().unwrap().exe, here);

            // A file that cannot be read as one of ours: left alone, said nothing.
            std::fs::write(dir.join(m.file_name(sock)), "junk").unwrap();
            assert!(refresh_with(m, &dir, sock, me, mine, says(None)).is_empty());
            assert_eq!(std::fs::read_to_string(dir.join(m.file_name(sock))).unwrap(), "junk");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The plist as macOS reads it.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_reads_the_agent() {
        let dir = scratch("plutil");
        let file = dir.join("agent.plist");
        std::fs::write(&file, plist("default", AWKWARD, &env(&[("PATH", "/opt/homebrew/bin:/usr/bin")]))).unwrap();
        let o = std::process::Command::new("plutil").arg("-lint").arg(&file).output().unwrap();
        assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        let o = std::process::Command::new("plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(&file)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim_end_matches('\n'), AWKWARD);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The unit as systemd reads it, where systemd is there to ask.
    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_reads_the_unit() {
        if crate::platform::shell::which("systemd-analyze").is_none() {
            return;
        }
        let dir = scratch("analyze");
        let file = dir.join("keepane-analyze.service");
        let me = std::env::current_exe().unwrap();
        std::fs::write(&file, unit("analyze", &me.to_string_lossy(), &env(&[("PATH", "/a b:/c%d$e")]))).unwrap();
        let o = std::process::Command::new("systemd-analyze").args(["--user", "verify"]).arg(&file).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(o.status.success() && !said.contains("keepane-analyze"), "{said}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
