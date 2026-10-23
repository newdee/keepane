//! `keepane update [--check]`: is there a newer keepane, and installing it the
//! way this one was installed. Nothing runs in the background and nothing
//! is installed unasked: the command is typed, it says what it does.
//!
//! The release is asked of GitHub with `curl.exe` (part of Windows 10 and
//! later); an MSI is checked against the SHA-256 published beside it before
//! Windows Installer sees it. Afterwards the running server is still the
//! old program: `keepane restart-server` moves the sessions to the new one.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "newdee/keepane";

/// A release: its version ("0.10.0"), its page, and its files by name.
#[derive(Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub assets: Vec<(String, String)>,
}

/// How this keepane was installed, which says how to update it.
#[derive(Debug, PartialEq)]
pub enum Kind {
    /// `scoop install keepane`: scoop updates it.
    Scoop,
    /// The MSI, under Program Files: installing needs an administrator.
    Msi,
    /// The per-user MSI, under %LOCALAPPDATA%\Programs: no administrator.
    MsiUser,
    /// Anything else (a zip, `cargo install`, a build): by hand.
    Other(PathBuf),
}

/// Version numbers compared as numbers: 0.10.0 is newer than 0.9.0.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts =
        |v: &str| -> Vec<u64> { v.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parts(candidate) > parts(current)
}

/// Where this program lives says how it got there.
pub fn kind_of(exe: &Path) -> Kind {
    let s = exe.to_string_lossy().to_lowercase();
    if s.contains(r"\scoop\apps\") || s.contains(r"\scoop\shims\") {
        return Kind::Scoop;
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA")
        && !local.is_empty()
        && s.starts_with(&format!(r"{}\programs\keepane\", local.to_lowercase()))
    {
        return Kind::MsiUser;
    }
    let under =
        |var: &str| std::env::var(var).ok().filter(|p| !p.is_empty()).is_some_and(|p| s.starts_with(&p.to_lowercase()));
    if under("ProgramFiles") || under("ProgramW6432") {
        return Kind::Msi;
    }
    Kind::Other(exe.to_path_buf())
}

/// The MSI of a release, per machine or per user.
pub fn msi_name(version: &str, user: bool) -> String {
    format!("keepane-{version}-windows-x86_64{}.msi", if user { "-user" } else { "" })
}

/// Whether a per-machine MSI cannot be installed from here: over SSH nobody
/// is at the desktop to answer Windows' permission prompt, unless this
/// session already runs as an administrator.
pub fn machine_msi_blocked(over_ssh: bool, elevated: bool) -> bool {
    over_ssh && !elevated
}

fn over_ssh() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"].iter().any(|v| std::env::var_os(v).is_some())
}

fn elevated() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// The hash in a `.sha256` file: `<hex>  <file name>`.
pub fn parse_sha256_file(text: &str) -> Option<String> {
    let h = text.split_whitespace().next()?.to_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// The latest release's JSON (GitHub's REST API), read into a `Release`.
pub fn parse_release(json: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json).context("release: not JSON")?;
    let tag = v["tag_name"].as_str().context("release: no tag_name")?;
    let page = v["html_url"].as_str().unwrap_or_default().to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { version: tag.trim_start_matches('v').to_string(), page, assets })
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl.exe")
        .args(["-fsSL", "--retry", "2", "-H", "User-Agent: keepane-update"])
        .args(args)
        .output()
        .context("curl.exe (part of Windows 10 and later) could not be run")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body =
        curl(&["-H", "Accept: application/vnd.github+json", &url]).context("asking GitHub for the latest release")?;
    parse_release(&String::from_utf8_lossy(&body))
}

/// SHA-256 of a file, with the `certutil` every Windows has.
fn sha256_of(path: &Path) -> Result<String> {
    let out = Command::new("certutil").args(["-hashfile"]).arg(path).arg("SHA256").output().context("certutil")?;
    let text = String::from_utf8_lossy(&out.stdout);
    // The second line is the hash, bytes separated by spaces on old Windows.
    text.lines()
        .nth(1)
        .map(|l| l.replace(' ', "").to_lowercase())
        .filter(|h| h.len() == 64)
        .context("certutil gave no hash")
}

fn install_msi(r: &Release, user: bool) -> Result<()> {
    let name = msi_name(&r.version, user);
    let find = |n: &str| r.assets.iter().find(|(a, _)| a == n).map(|(_, u)| u.clone());
    let msi_url = find(&name).with_context(|| format!("the release has no {name}"))?;
    let sha_url = find(&format!("{name}.sha256")).with_context(|| format!("the release has no {name}.sha256"))?;
    let dir = std::env::temp_dir().join(format!("keepane-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let msi = dir.join(&name);
    println!("downloading {name}");
    curl(&["-o", &msi.to_string_lossy(), &msi_url])?;
    let want =
        parse_sha256_file(&String::from_utf8_lossy(&curl(&[&sha_url])?)).context("the .sha256 file has no hash")?;
    let got = sha256_of(&msi)?;
    if got != want {
        let _ = std::fs::remove_dir_all(&dir);
        bail!("{name} does not match its published SHA-256 ({got} != {want}); not installed");
    }
    println!("SHA-256 matches; installing{}", if user { "" } else { " (Windows asks for permission)" });
    let status = Command::new("msiexec.exe").arg("/i").arg(&msi).args(["/passive", "/norestart"]).status()?;
    let _ = std::fs::remove_dir_all(&dir);
    // 3010: installed, a reboot finishes it (a file was in use: the old
    // keepane.exe, which the running server still is).
    match status.code() {
        Some(0) | Some(3010) => Ok(()),
        Some(1602) => bail!("the install was cancelled"),
        c => bail!("msiexec failed ({c:?})"),
    }
}

pub async fn run(socket: &str, args: &[String]) -> Result<i32> {
    let check = match args {
        [] => false,
        [a] if a == "--check" || a == "-n" => true,
        _ => bail!("update: takes --check (or nothing)"),
    };
    let current = env!("CARGO_PKG_VERSION");
    let r = latest()?;
    if !newer(&r.version, current) {
        println!("keepane {current} is the latest");
        return Ok(0);
    }
    println!("keepane {} is out (this is {current}): {}", r.version, r.page);
    if check {
        return Ok(0);
    }
    let exe = std::env::current_exe()?;
    match kind_of(&exe) {
        Kind::Scoop => return hand_to_scoop(socket, &exe, &r.version),
        Kind::Msi if machine_msi_blocked(over_ssh(), elevated()) => {
            println!(
                "over SSH nobody is at the desktop to answer Windows' permission prompt for this MSI (it installs for every user):"
            );
            println!("install {} instead (it needs no administrator), or use scoop", msi_name(&r.version, true));
            return Ok(1);
        }
        Kind::Msi => install_msi(&r, false)?,
        Kind::MsiUser => install_msi(&r, true)?,
        Kind::Other(path) => {
            println!(
                "this keepane ({}) was not installed by the MSI or scoop: download it from the page above",
                path.display()
            );
            return Ok(1);
        }
    }
    println!("installed keepane {}", r.version);
    if crate::client::server_version(socket).await.is_some() {
        println!(
            "the running server is still the old one: `keepane restart-server` moves your sessions to the new one"
        );
    }
    Ok(0)
}

/// The folder Scoop keeps keepane in (`…\scoop\apps\keepane`), from the
/// program's path: `<it>\<version, or current>\keepane.exe`.
pub fn scoop_app_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?.parent()?;
    dir.parent()?.file_name()?.eq_ignore_ascii_case("apps").then(|| dir.to_path_buf())
}

/// The program to run once Scoop has installed `version` in `app_dir`: by
/// `current` when `old` was run by it (Scoop's junction), else in the
/// version's own folder (`no_junction`), as the registrations had it.
pub fn scoop_exe(app_dir: &Path, old: &Path, version: &str) -> PathBuf {
    let by_current = old.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("current"));
    app_dir.join(if by_current { "current" } else { version }).join("keepane.exe")
}

/// Whether `path` is in `dir`, as Scoop asks it (`-like "$dir\*"`): by
/// the letters, case aside.
pub fn is_under(path: &Path, dir: &Path) -> bool {
    let dir = dir.to_string_lossy().trim_end_matches('\\').to_lowercase() + "\\";
    path.to_string_lossy().to_lowercase().starts_with(&dir)
}

/// Where the Scoop update a copy of keepane finishes says what it did.
/// (Beside the sessions when `KEEPANE_SESSIONS_DIR` moves them: tests do.)
pub fn update_log() -> PathBuf {
    if let Some(d) = crate::legacy::var_os("KEEPANE_SESSIONS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("update.log");
    }
    crate::logger::log_dir().join("update.log")
}

/// Scoop updates an app only while none of its programs runs (it says
/// "Running process detected, skip updating." and changes nothing), and the
/// server, every attached terminal and this very command run from its
/// folder. So a copy of this program, outside that folder, does the rest
/// once this one has exited (`finish_scoop`).
fn hand_to_scoop(socket: &str, exe: &Path, version: &str) -> Result<i32> {
    use crate::platform::process::{leave_job_denied, spawn_detached};
    scoop_app_dir(exe).with_context(|| format!("{} is not in a Scoop app folder", exe.display()))?;
    // The copies earlier updates ran from (each one's own folder, which it
    // could not remove while it ran).
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("keepane-update-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("keepane-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let copy = dir.join("keepane.exe");
    std::fs::copy(exe, &copy).context("copying keepane out of Scoop's folder")?;
    let pid = std::process::id().to_string();
    let old = exe.to_string_lossy();
    let args = ["-L", socket, "__update-scoop", version, &pid, &old];
    match spawn_detached(&copy, &args, true) {
        Err(e) if leave_job_denied(&e) => spawn_detached(&copy, &args, false),
        r => r,
    }?;
    println!(
        "Scoop updates keepane only while no keepane runs, so keepane steps aside: the sessions are saved \
         and the server stops (attached terminals are detached; the programs in the panes start again, \
         as with `keepane restart-server`), Scoop installs keepane {version}, and the server starts again \
         with the sessions."
    );
    println!("In a minute or so: `keepane attach`. What happened: {}", update_log().display());
    Ok(0)
}

/// `keepane __update-scoop VERSION PID PROGRAM`: the rest of `update` for
/// a Scoop install (`hand_to_scoop`), run by a copy of keepane outside
/// Scoop's folder once `update` (process PID, the program PROGRAM) is gone.
/// The sessions are saved and the server stopped, Scoop is run once nothing
/// of keepane's runs from its folder any more, and the server starts again
/// with the sessions that were running: from the new program, or from the
/// old one when Scoop did not install it. What it did goes to `update_log`.
pub async fn finish_scoop(socket: &str, args: &[String]) -> Result<i32> {
    let [version, pid, old] = args else { bail!("__update-scoop: VERSION PID PROGRAM") };
    // The pane `update` was typed in is gone with the old server.
    unsafe {
        std::env::remove_var("KEEPANE");
        std::env::remove_var("KEEPANE_PANE");
    }
    let old = PathBuf::from(old);
    let app_dir = scoop_app_dir(&old).with_context(|| format!("{} is not in a Scoop app folder", old.display()))?;
    let mut log = Log::new(update_log());
    let this = env!("CARGO_PKG_VERSION");
    log.line(format!("keepane {this} -> {version} with Scoop"));
    if let Ok(pid) = pid.parse() {
        wait_for_exit(pid, std::time::Duration::from_secs(30));
    }
    let pipe = crate::ipc::pipe_name(socket);
    let (code, saved) = stop_server(socket, &pipe).await;
    let Some(sessions) = saved else {
        log.line(code);
        log.line("not updated: the server is left running".into());
        return Ok(1);
    };
    if !code.is_empty() {
        log.line(code);
    }
    // What still runs from Scoop's folder (another server, a terminal
    // still closing): Scoop would only skip the update.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut blockers = scoop_blockers(&app_dir);
    while !blockers.is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        blockers = scoop_blockers(&app_dir);
    }
    let installed = if blockers.is_empty() {
        let command = crate::legacy::var("KEEPANE_SCOOP_UPDATE").unwrap_or_else(|| "scoop update keepane".into());
        // scoop is a .cmd/.ps1 shim, which Command::new would not find
        // (it looks for scoop.exe): let cmd resolve it.
        let out = quiet(Command::new("cmd.exe").args(["/d", "/c", &command])).output();
        match out {
            Ok(o) => {
                let said = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
                for l in said.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
                    log.line(format!("scoop: {l}"));
                }
            }
            Err(e) => log.line(format!("scoop could not be run: {e}")),
        }
        app_dir.join(version).join("keepane.exe").is_file()
    } else {
        for (pid, path) in &blockers {
            log.line(format!("still running from Scoop's folder: process {pid}, {}", path.display()));
        }
        log.line("Scoop would skip the update while these run".into());
        false
    };
    let exe = if installed {
        log.line(format!("installed keepane {version}"));
        Some(scoop_exe(&app_dir, &old, version))
    } else {
        log.line(format!("not updated: keepane {this} stays"));
        [old.clone(), app_dir.join(this).join("keepane.exe")].into_iter().find(|p| p.is_file())
    };
    match exe {
        Some(exe) if !sessions.is_empty() => {
            let mut restored = Vec::new();
            for s in &sessions {
                match run_keepane(&exe, socket, &["restore-session", s]) {
                    Ok(_) => restored.push(s.as_str()),
                    Err(e) => log.line(format!("{s}: not restored: {e}")),
                }
            }
            log.line(format!(
                "server: {} of {} session(s) restored by {}{}",
                restored.len(),
                sessions.len(),
                exe.display(),
                if restored.is_empty() { String::new() } else { format!(": {}", restored.join(", ")) }
            ));
            // The phones' page, served again as it was.
            let _ = run_keepane(&exe, socket, &["web-resume"]);
            log.line("`keepane attach` to go back".into());
        }
        Some(_) => {}
        None => log.line("no keepane program left to start the server with".into()),
    }
    let title = if installed { format!("keepane {version} installed") } else { "keepane was not updated".into() };
    crate::notify::notify(&title, &format!("what happened: {}", log.path.display()));
    Ok(if installed { 0 } else { 1 })
}

/// Save every session and stop the server on `pipe`. Returns what to say
/// (empty when there was nothing to stop), and the sessions that were
/// running, or none when the server is left running.
async fn stop_server(socket: &str, pipe: &str) -> (String, Option<Vec<String>>) {
    use crate::client::{query, server_running};
    if !server_running(pipe) {
        return (String::new(), Some(Vec::new()));
    }
    let Ok((_, list, _)) = query(socket, &["list-sessions"]).await else {
        return ("the server did not answer".into(), None);
    };
    let sessions: Vec<String> =
        list.lines().filter_map(|l| l.split_once(':').map(|(n, _)| n.to_string())).filter(|n| !n.is_empty()).collect();
    match query(socket, &["save-session", "-a"]).await {
        Ok((0, _, _)) => {}
        Ok((_, _, err)) if !sessions.is_empty() => {
            return (format!("saving the sessions failed: {}", err.trim()), None);
        }
        Err(e) if !sessions.is_empty() => return (format!("saving the sessions failed: {e:#}"), None),
        _ => {}
    }
    // Plain kill-server: its terminals detach and exit, where `-r` would
    // keep them running (waiting to attach again) from Scoop's folder.
    match query(socket, &["kill-server"]).await {
        Ok((0, _, _)) => {}
        Ok((_, _, err)) => return (format!("the server would not stop: {}", err.trim()), None),
        Err(e) => return (format!("the server would not stop: {e:#}"), None),
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while server_running(pipe) {
        if std::time::Instant::now() > deadline {
            return ("the server did not exit".into(), None);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    (format!("server: {} session(s) saved, stopped", sessions.len()), Some(sessions))
}

/// What `finish_scoop` did, line by line into the log as it goes (a copy
/// that dies halfway has still said how far it got).
struct Log {
    path: PathBuf,
}

impl Log {
    fn new(path: PathBuf) -> Log {
        let _ = std::fs::write(&path, format!("{}\n", chrono::Local::now().to_rfc3339()));
        Log { path }
    }

    fn line(&mut self, text: String) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&self.path) {
            let _ = writeln!(f, "{text}");
        }
    }
}

/// A command with no window of its own and nothing to read.
fn quiet(c: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000).stdin(std::process::Stdio::null()) // CREATE_NO_WINDOW
}

/// `exe -L socket args…`, to completion: what it printed, or why not.
fn run_keepane(exe: &Path, socket: &str, args: &[&str]) -> Result<String> {
    let o = quiet(Command::new(exe).arg("-L").arg(socket).args(args)).output()?;
    if !o.status.success() {
        bail!("{}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Wait (at most `limit`) for process `pid` to exit.
fn wait_for_exit(pid: u32, limit: std::time::Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return; // gone already
    }
    unsafe {
        WaitForSingleObject(h, limit.as_millis() as u32);
        CloseHandle(h);
    }
}

/// The keepane programs Scoop would find running and skip the update for:
/// those in its folder for keepane, and its shim (which it removes and
/// makes again). Not this one.
fn scoop_blockers(app_dir: &Path) -> Vec<(u32, PathBuf)> {
    let shim = app_dir.parent().and_then(Path::parent).map(|root| root.join("shims").join("keepane.exe"));
    let me = std::process::id();
    super::sysinfo::processes()
        .into_iter()
        .filter(|(pid, _, name)| *pid != me && name.eq_ignore_ascii_case("keepane.exe"))
        .filter_map(|(pid, _, _)| Some((pid, image_path(pid)?)))
        .filter(|(_, p)| {
            is_under(p, app_dir)
                || shim.as_deref().is_some_and(|s| p.to_string_lossy().eq_ignore_ascii_case(&s.to_string_lossy()))
        })
        .collect()
}

/// The full path of process `pid`'s program.
fn image_path(pid: u32) -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(h) };
    (ok != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("v1.0.0", "0.99.9"));
        assert!(newer("0.9.1", "0.9.0"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.9", "0.9.0"));
        assert!(!newer("garbage", "0.9.0"));
    }

    #[test]
    fn the_install_is_told_by_the_path() {
        assert_eq!(kind_of(Path::new(r"C:\Users\x\scoop\apps\keepane\current\keepane.exe")), Kind::Scoop);
        if let Ok(pf) = std::env::var("ProgramFiles") {
            assert_eq!(kind_of(&Path::new(&pf).join("keepane").join("keepane.exe")), Kind::Msi);
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let p = Path::new(&local).join("Programs").join("keepane").join("keepane.exe");
            assert_eq!(kind_of(&p), Kind::MsiUser);
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            // Case does not matter on Windows; a sibling whose name only starts
            // with keepane is not it.
            let upper = format!(r"{}\PROGRAMS\KEEPANE\keepane.exe", local.to_uppercase());
            assert_eq!(kind_of(Path::new(&upper)), Kind::MsiUser);
            let sibling = Path::new(&local).join("Programs").join("keepane2").join("keepane.exe");
            assert!(matches!(kind_of(&sibling), Kind::Other(_)), "{sibling:?}");
        }
        let dev = Path::new(r"D:\src\keepane\target\release\keepane.exe");
        assert_eq!(kind_of(dev), Kind::Other(dev.to_path_buf()));
    }

    /// Scoop's folder for keepane, from the program's path either way Scoop
    /// runs it, and the program to start after an update: the same way.
    #[test]
    fn scoops_folder_and_the_new_program_are_told_by_the_path() {
        let app = Path::new(r"C:\Users\x\scoop\apps\keepane");
        let by_version = app.join(r"0.38.2\keepane.exe");
        let by_current = Path::new(r"C:\Users\x\scoop\apps\keepane\Current\keepane.exe");
        assert_eq!(scoop_app_dir(&by_version).as_deref(), Some(app));
        assert_eq!(
            scoop_app_dir(Path::new(r"D:\s\Apps\keepane\1.0.0\keepane.exe")).unwrap(),
            Path::new(r"D:\s\Apps\keepane")
        );
        assert_eq!(scoop_app_dir(Path::new(r"D:\src\keepane\target\release\keepane.exe")), None);
        assert_eq!(scoop_app_dir(Path::new(r"keepane.exe")), None);
        assert_eq!(scoop_exe(app, &by_version, "0.45.1"), app.join(r"0.45.1\keepane.exe"));
        assert_eq!(scoop_exe(app, by_current, "0.45.1"), app.join(r"current\keepane.exe"));
        // Scoop's -like "$dir\*": case aside, and a sibling folder whose name
        // only starts the same is not in it.
        assert!(is_under(Path::new(r"C:\USERS\X\SCOOP\APPS\KEEPANE\0.38.2\keepane.exe"), app));
        assert!(is_under(&by_version, Path::new(r"C:\Users\x\scoop\apps\keepane\")));
        assert!(!is_under(Path::new(r"C:\Users\x\scoop\apps\keepane2\0.1.0\keepane.exe"), app));
        assert!(!is_under(Path::new(r"C:\Users\x\scoop\shims\keepane.exe"), app));
    }

    #[test]
    fn the_msi_to_take_and_when_not_to() {
        assert_eq!(msi_name("0.15.2", false), "keepane-0.15.2-windows-x86_64.msi");
        assert_eq!(msi_name("0.15.2", true), "keepane-0.15.2-windows-x86_64-user.msi");
        assert!(machine_msi_blocked(true, false), "over SSH, not an administrator");
        assert!(!machine_msi_blocked(true, true), "an administrator session needs no prompt");
        assert!(!machine_msi_blocked(false, false), "at the desktop the prompt is answered");
    }

    #[test]
    fn a_release_and_its_hashes_are_read() {
        let json = r#"{"tag_name":"v0.10.0","html_url":"https://github.com/newdee/keepane/releases/tag/v0.10.0",
            "assets":[{"name":"keepane-0.10.0-windows-x86_64.msi","browser_download_url":"https://x/a.msi"},
                      {"name":"broken"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.10.0");
        assert_eq!(r.assets, vec![("keepane-0.10.0-windows-x86_64.msi".to_string(), "https://x/a.msi".to_string())]);
        assert!(parse_release("{}").is_err());
        assert!(parse_release("not json").is_err());
        let h = "a".repeat(64);
        assert_eq!(parse_sha256_file(&format!("{h}  keepane.msi")), Some(h.clone()));
        assert_eq!(parse_sha256_file(&h.to_uppercase()), Some(h));
        assert_eq!(parse_sha256_file("abc  file"), None);
        assert_eq!(parse_sha256_file(""), None);
    }

    #[test]
    fn certutil_hashes_a_file() {
        let p = std::env::temp_dir().join(format!("keepane-sha-{}", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        let h = sha256_of(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
