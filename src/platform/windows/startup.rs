//! `keepane startup on|off|status`: start the server at logon and bring every
//! saved session back, so after a reboot there is nothing to do but attach.
//!
//! The per-user `Run` registry key rather than a scheduled task: a task with
//! a logon trigger cannot be created without administrator rights, while the
//! `Run` key is the user's own. Windows runs the value through CreateProcess
//! at logon; `conhost --headless` in front of it is what keeps a console
//! window from flashing up for a server that has no console anyway.

use crate::takeover::{self, Version};
use anyhow::{Context, Result, bail};
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value's name under the Run key, one per socket so two servers can
/// both come up.
fn value_name(socket: &str) -> String {
    if socket == "default" { "keepane".to_string() } else { format!("keepane-{socket}") }
}

/// What runs at logon: the server directly (not a client that would start
/// it and exit), told to restore every saved session.
fn command_line(exe: &str, socket: &str) -> String {
    format!("conhost.exe --headless \"{exe}\" -L {socket} __server --restore")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Key(HKEY);

impl Key {
    /// The Run key, or None when the profile has none yet (a fresh user
    /// has never had a program register itself, so nothing is installed).
    fn open(path: &str, access: u32) -> Result<Option<Key>> {
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, access, &mut h) };
        match rc {
            0 => Ok(Some(Key(h))),
            rc if rc == ERROR_FILE_NOT_FOUND => Ok(None),
            rc => bail!("cannot open HKCU\\{path} (error {rc})"),
        }
    }

    /// The Run key, made when it is missing: installing must work on a
    /// profile nothing has registered itself in before.
    fn create(path: &str) -> Result<Key> {
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            bail!("cannot open or create HKCU\\{path} (error {rc})");
        }
        Ok(Key(h))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// Register the logon command. Idempotent: setting it again replaces it.
pub fn install(socket: &str) -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    install_exe(socket, &exe.to_string_lossy())
}

/// Register the logon command of `exe` (`install`'s, or a newer keepane
/// taking it over: `refresh_registrations`).
fn install_exe(socket: &str, exe: &str) -> Result<String> {
    let name = value_name(socket);
    let cmd = command_line(exe, socket);
    let key = Key::create(RUN_KEY)?;
    let data = wide(&cmd);
    let rc = unsafe {
        RegSetValueExW(key.0, wide(&name).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32)
    };
    if rc != 0 {
        bail!("cannot write the Run value (error {rc})");
    }
    Ok(format!(
        "keepane will start at logon and restore every saved session,\n\
         and serve the phone page again if it was on (`keepane web stop` turns that off).\n\
         where: HKCU\\{RUN_KEY}\\{name}\n\
         runs:  {cmd}\n\
         `keepane startup off` removes it; `keepane startup status` shows it."
    ))
}

pub fn remove(socket: &str) -> Result<String> {
    remove_named(&value_name(socket))
}

/// The Run value wmux (keepane's old name, up to 0.13.1) used for `socket`.
fn legacy_value_name(socket: &str) -> String {
    if socket == "default" { "wmux".to_string() } else { format!("wmux-{socket}") }
}

/// The logon command an old wmux registered for `socket`, when there is one
/// and it is ours (`is_our_old_command`), not another program's.
pub fn legacy_status(socket: &str) -> Result<Option<String>> {
    Ok(status_named(&legacy_value_name(socket))?.filter(|c| crate::legacy::is_our_old_command(c)))
}

/// Remove the old wmux logon command of `socket`.
pub fn remove_legacy(socket: &str) -> Result<String> {
    remove_named(&legacy_value_name(socket))
}

fn remove_named(name: &str) -> Result<String> {
    let Some(key) = Key::open(RUN_KEY, KEY_SET_VALUE)? else {
        return Ok(format!("{name}: not installed"));
    };
    let rc = unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) };
    match rc {
        0 => Ok(format!("{name}: removed; keepane will not start at logon")),
        rc if rc == ERROR_FILE_NOT_FOUND => Ok(format!("{name}: not installed")),
        rc => bail!("cannot delete the Run value (error {rc})"),
    }
}

/// The command that runs at logon, or None when there is none.
pub fn status(socket: &str) -> Result<Option<String>> {
    status_named(&value_name(socket))
}

fn status_named(name: &str) -> Result<Option<String>> {
    let Some(key) = Key::open(RUN_KEY, KEY_QUERY_VALUE)? else { return Ok(None) };
    let mut kind = 0u32;
    let mut size = 0u32;
    let name_w = wide(name);
    let rc = unsafe {
        RegQueryValueExW(key.0, name_w.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut size)
    };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if rc != 0 {
        bail!("cannot read the Run value (error {rc})");
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
    let rc = unsafe {
        RegQueryValueExW(
            key.0,
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr() as *mut u8,
            &mut size,
        )
    };
    if rc != 0 {
        bail!("cannot read the Run value (error {rc})");
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..end])))
}

/// `keepane startup [on|off|status]`, run on the client side: nothing here
/// needs a server, and the server is exactly what this is about starting.
pub fn run(socket: &str, args: &[String]) -> Result<i32> {
    // One verb and nothing after it: a typo must not look like it worked.
    if args.len() > 1 {
        bail!("startup: unexpected argument '{}'", args[1]);
    }
    match args.first().map(String::as_str) {
        Some("on") | Some("install") | Some("enable") => {
            println!("{}", install(socket)?);
            Ok(0)
        }
        Some("off") | Some("uninstall") | Some("disable") => {
            println!("{}", remove(socket)?);
            Ok(0)
        }
        None | Some("status") => match status(socket)? {
            Some(cmd) => {
                println!("{}: installed, runs at logon:\n  {cmd}", value_name(socket));
                Ok(0)
            }
            None => {
                println!("{}: not installed (`keepane startup on` to start at logon)", value_name(socket));
                Ok(1)
            }
        },
        Some(other) => bail!("startup: expected on, off or status, got '{other}'"),
    }
}

// ---------------------------------------------------------------------------
// The registrations follow the newest keepane (crate::takeover): the logon
// command above and the Windows Terminal profile both run keepane by its
// full path.

/// The exe a registered command line runs: its first quoted path (both the
/// logon command and the profile quote it).
pub(crate) fn exe_of(cmd: &str) -> Option<std::path::PathBuf> {
    let start = cmd.find('"')? + 1;
    let len = cmd[start..].find('"')?;
    Some(std::path::PathBuf::from(&cmd[start..start + len])).filter(|p| !p.as_os_str().is_empty())
}

/// The logon command and the Windows Terminal profile of `socket`, moved to
/// this keepane where they run an older one or one that is gone; one line
/// for each moved. Only from an installed keepane (`takeover::installed_self`).
pub fn refresh_registrations(socket: &str) -> Vec<String> {
    let Some(me) = takeover::installed_self() else { return Vec::new() };
    let Ok(wt_dir) = super::wt::fragments_dir() else { return Vec::new() };
    let cache = takeover::cache_file();
    refresh_with(socket, &me, takeover::this_version(), &wt_dir, |exe| {
        takeover::version_cached(exe, &cache, takeover::version_of)
    })
}

/// `refresh_registrations`, with this exe, its version, the profile's
/// folder and the way to ask a version given (the tests give their own).
pub(crate) fn refresh_with(
    socket: &str,
    me: &std::path::Path,
    mine: Version,
    wt_dir: &std::path::Path,
    version_of: impl Fn(&std::path::Path) -> Option<Version>,
) -> Vec<String> {
    let me_s = me.to_string_lossy();
    let mut lines = Vec::new();
    if let Ok(Some(cmd)) = status(socket)
        && let Some(old) = exe_of(&cmd)
        && let Some(why) = takeover::stale(&old, me, mine, &version_of)
    {
        match install_exe(socket, &me_s) {
            Ok(_) => lines.push(takeover::said("start at logon", me, &old, &why)),
            Err(e) => lines.push(format!("start at logon still runs {}: {e:#}", old.display())),
        }
    }
    if let Ok(Some(cmd)) = super::wt::status_in(wt_dir, socket)
        && let Some(old) = exe_of(&cmd)
        && let Some(why) = takeover::stale(&old, me, mine, &version_of)
    {
        match super::wt::install_exe_in(wt_dir, socket, &me_s) {
            Ok(_) => lines.push(takeover::said("the Windows Terminal profile", me, &old, &why)),
            Err(e) => lines.push(format!("the Windows Terminal profile still runs {}: {e:#}", old.display())),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway socket's logon command, removed when the test ends however
    /// it ends: a value left in the Run key would run at the next logon.
    struct Throwaway(String);

    impl Drop for Throwaway {
        fn drop(&mut self) {
            let _ = remove(&self.0);
        }
    }

    #[test]
    fn names_and_command_lines() {
        assert_eq!(value_name("default"), "keepane");
        assert_eq!(value_name("work"), "keepane-work");
        let c = command_line(r"C:\Program Files\keepane\keepane.exe", "default");
        // Headless conhost, the exe quoted (Program Files has a space), the
        // server entry point and the restore flag: all four or the result is
        // visible, broken, a client, or empty after a reboot.
        assert!(c.starts_with("conhost.exe --headless "), "{c}");
        assert!(c.contains(r#""C:\Program Files\keepane\keepane.exe""#), "{c}");
        assert!(c.ends_with("-L default __server --restore"), "{c}");
    }

    #[test]
    fn bad_verb_is_refused_without_touching_the_registry() {
        let e = run("default", &["sideways".to_string()]).unwrap_err().to_string();
        assert!(e.contains("expected on, off or status"), "{e}");
        let e = run("default", &["on".to_string(), "extra".to_string()]).unwrap_err().to_string();
        assert!(e.contains("unexpected argument 'extra'"), "{e}");
    }

    /// A profile with no Run key at all (a fresh user, a CI runner): opening
    /// says "none" rather than failing, creating makes it. A throwaway key
    /// of keepane's own stands in for the real one, and is deleted after.
    #[test]
    fn a_missing_key_reads_as_nothing_installed_and_is_made_on_install() {
        use windows_sys::Win32::System::Registry::RegDeleteKeyW;
        let path = format!("Software\\keepane-unit-test-{}\\Run", std::process::id());
        let parent = path.rsplit_once('\\').unwrap().0.to_string();
        assert!(Key::open(&path, KEY_QUERY_VALUE).unwrap().is_none(), "nothing there yet");
        assert!(Key::open(&path, KEY_SET_VALUE).unwrap().is_none());
        drop(Key::create(&path).unwrap());
        assert!(Key::open(&path, KEY_QUERY_VALUE).unwrap().is_some(), "made by create");
        drop(Key::create(&path).unwrap()); // and creating it again is fine
        for p in [path.as_str(), parent.as_str()] {
            assert_eq!(unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide(p).as_ptr()) }, 0, "cleanup {p}");
        }
        assert!(Key::open(&path, KEY_QUERY_VALUE).unwrap().is_none(), "gone again");
    }

    /// The real key, with a throwaway socket name so nothing of the user's
    /// is touched: on, status, off, status, off again.
    #[test]
    fn install_status_remove_round_trip() {
        let sock = format!("unit-test-{}", std::process::id());
        let _gone_after = Throwaway(sock.clone());
        let _ = remove(&sock);
        assert!(status(&sock).unwrap().is_none());
        let msg = install(&sock).unwrap();
        assert!(msg.contains("--restore"), "{msg}");
        let shown = status(&sock).unwrap().expect("installed");
        assert!(shown.starts_with("conhost.exe --headless "), "{shown}");
        assert!(shown.ends_with(&format!("-L {sock} __server --restore")), "{shown}");
        assert!(remove(&sock).unwrap().contains("removed"));
        assert!(status(&sock).unwrap().is_none());
        assert!(remove(&sock).unwrap().contains("not installed"), "removing twice is not an error");
    }

    #[test]
    fn registered_exes_are_read() {
        // As both registrations write them.
        let run = command_line(r"C:\Program Files\keepane\keepane.exe", "default");
        assert_eq!(exe_of(&run).unwrap(), std::path::Path::new(r"C:\Program Files\keepane\keepane.exe"));
        let wt = r#""C:\s\apps\keepane\0.38.1\keepane.exe" -L work new-session -A -s main"#;
        assert_eq!(exe_of(wt).unwrap(), std::path::Path::new(r"C:\s\apps\keepane\0.38.1\keepane.exe"));
        assert_eq!(exe_of("keepane.exe __server"), None, "nothing quoted");
        assert_eq!(exe_of(r#""" -L x"#), None, "an empty path");
    }

    /// Both registrations, for real (the Run key under a throwaway socket
    /// name, the profile in a folder of the test's own): an older or gone
    /// keepane in them is replaced by this one, a newer one stays, and one
    /// that is not registered is not made.
    #[test]
    fn stale_registrations_move_to_this_keepane_and_no_others() {
        let sock = format!("unit-refresh-{}", std::process::id());
        let _gone_after = Throwaway(sock.clone());
        let wt = std::env::temp_dir().join(format!("keepane-refresh-wt-{}", std::process::id()));
        let _ = remove(&sock);
        let _ = std::fs::remove_dir_all(&wt);
        let me = std::path::Path::new(r"C:\new\keepane.exe");
        let mine = (0, 39, 0);
        let gone = r"C:\keepane-gone-for-the-test\keepane.exe";
        let says = |v: Option<Version>| move |_: &std::path::Path| v;

        // Nothing registered: nothing made.
        assert!(refresh_with(&sock, me, mine, &wt, says(Some((0, 1, 0)))).is_empty());
        assert!(status(&sock).unwrap().is_none(), "no logon command made");
        assert!(super::super::wt::status_in(&wt, &sock).unwrap().is_none(), "no profile made");

        // Both run a keepane that is gone: both now run this one.
        install_exe(&sock, gone).unwrap();
        super::super::wt::install_exe_in(&wt, &sock, gone).unwrap();
        let said = refresh_with(&sock, me, mine, &wt, says(None));
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(
            said[0].starts_with("start at logon now runs C:\\new\\keepane.exe") && said[0].contains("which is gone"),
            "{said:?}"
        );
        assert!(
            said[1].starts_with("the Windows Terminal profile now runs") && said[1].contains("which is gone"),
            "{said:?}"
        );
        assert_eq!(status(&sock).unwrap().unwrap(), command_line(r"C:\new\keepane.exe", &sock));
        assert!(super::super::wt::status_in(&wt, &sock).unwrap().unwrap().starts_with(r#""C:\new\keepane.exe""#));
        // Again: nothing left to do.
        assert!(refresh_with(&sock, me, mine, &wt, says(Some((0, 1, 0)))).is_empty());

        // An older keepane that is there; then a newer one, which stays.
        let here = std::env::current_exe().unwrap();
        let here_s = here.to_string_lossy();
        install_exe(&sock, &here_s).unwrap();
        let said = refresh_with(&sock, me, mine, &wt, says(Some((0, 38, 1))));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("version 0.38.1"), "{said:?}");
        install_exe(&sock, &here_s).unwrap();
        assert!(refresh_with(&sock, me, mine, &wt, says(Some((0, 40, 0)))).is_empty(), "a newer keepane stays");
        assert_eq!(status(&sock).unwrap().unwrap(), command_line(&here_s, &sock));

        assert!(remove(&sock).unwrap().contains("removed"));
        let _ = std::fs::remove_dir_all(&wt);
    }
}
