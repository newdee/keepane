//! The registrations follow the newest keepane.
//!
//! What keepane registers for itself runs one program by its full path: the
//! logon command (`startup`: the Windows `Run` value, the macOS launchd
//! agent, the Linux systemd unit) and the Windows Terminal profile. An
//! install that puts each version in a folder of its own (Scoop with
//! `no_junction`, Homebrew's Cellar, a hand-kept folder per version) leaves
//! that path on the old version after an update, and on nothing once the old
//! folder is cleaned up: logon then starts nothing, and says nothing. So
//! keepane, as a command starts (not the server, not Tab completion, not the
//! commands that set these: main.rs), looks at them and takes over the ones
//! that run a keepane older than itself or one that is gone; one that runs a
//! newer keepane, the same version elsewhere, or one whose version cannot be
//! read is left as it is, so two installs never take turns. (The
//! notification link needs none of this: each server registers itself.)
//! What is shared is here; each platform's `startup` reads and writes its
//! own registrations.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A keepane version, as `keepane -V` prints it.
pub(crate) type Version = (u64, u64, u64);

/// "keepane 0.38.1" (or "0.38.1"), into its numbers.
pub(crate) fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix("keepane ").unwrap_or(s);
    // A pre-release or build suffix is not a version of its own here.
    let s = s.split(['-', '+', ' ']).next()?;
    let mut n = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (n.next()??, n.next()??, n.next()??);
    if n.next().is_some() {
        return None;
    }
    Some(v)
}

/// This program's version.
pub(crate) fn this_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("the crate's version is three numbers")
}

/// What a keepane program says its version is (`-V`, which answers before
/// anything else runs), asked without a window and given five seconds.
pub(crate) fn version_of(exe: &Path) -> Option<Version> {
    version_within(exe, Duration::from_secs(5))
}

pub(crate) fn version_within(exe: &Path, wait: Duration) -> Option<Version> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.arg("-V").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version(out.lines().next()?)
}

/// Where `version_cached` keeps what it was told.
pub(crate) fn cache_file() -> PathBuf {
    crate::logger::log_dir().join("exe-versions")
}

/// `ask` (`version_of`), remembered in `cache` by the program's path, size
/// and time of writing: a keepane that is not the registered one (an older
/// install still on the PATH) then does not start the registered one on
/// each of its own starts. A changed file is asked again; no answer is not
/// remembered; the last 16 are kept.
pub(crate) fn version_cached(exe: &Path, cache: &Path, ask: impl Fn(&Path) -> Option<Version>) -> Option<Version> {
    let meta = std::fs::metadata(exe).ok()?;
    let written = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    // A tab separates the fields: no installed program's path has one.
    let key = format!("{}\t{}\t{written}", exe.display(), meta.len());
    let text = std::fs::read_to_string(cache).unwrap_or_default();
    for line in text.lines() {
        if let Some((k, v)) = line.rsplit_once('\t')
            && k == key
        {
            return parse_version(v);
        }
    }
    let v = ask(exe)?;
    let path_part = format!("{}\t", exe.display());
    let mut kept: Vec<String> =
        text.lines().filter(|l| !l.starts_with(&path_part) && !l.is_empty()).map(str::to_string).collect();
    kept.push(format!("{key}\t{}.{}.{}", v.0, v.1, v.2));
    let start = kept.len().saturating_sub(16);
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(cache, kept[start..].join("\n") + "\n");
    Some(v)
}

/// Why a registration should run this keepane instead.
#[derive(Debug, PartialEq)]
pub(crate) enum Stale {
    /// The program it runs is not there any more.
    Gone,
    /// It runs an older keepane.
    Older(Version),
}

/// The same file? Spelled the same, or the same once links are followed
/// when both exist (Scoop's `current` junction, Homebrew's `bin/keepane`
/// link into the Cellar). On Windows case and `/` do not matter either.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    #[cfg(not(windows))]
    let norm = |p: &Path| p.to_string_lossy().into_owned();
    if norm(a) == norm(b) {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => norm(&x) == norm(&y),
        _ => false,
    }
}

/// Whether a registration running `registered` should run `me` (version
/// `mine`) instead; `version_of` asks a program its version.
pub(crate) fn stale(
    registered: &Path,
    me: &Path,
    mine: Version,
    version_of: impl Fn(&Path) -> Option<Version>,
) -> Option<Stale> {
    if same_file(registered, me) {
        return None;
    }
    if !registered.is_file() {
        return Some(Stale::Gone);
    }
    match version_of(registered) {
        Some(v) if v < mine => Some(Stale::Older(v)),
        _ => None,
    }
}

/// The line that says a registration (`what`) now runs `me`, not `old`.
pub(crate) fn said(what: &str, me: &Path, old: &Path, why: &Stale) -> String {
    let why = match why {
        Stale::Gone => "which is gone".to_string(),
        Stale::Older((a, b, c)) => format!("version {a}.{b}.{c}"),
    };
    format!("{what} now runs {} (it ran {}, {why})", me.display(), old.display())
}

/// This keepane, when it may take registrations over: only where it runs
/// with the real places (not under a test's own directories, as
/// `legacy::uses_real_dirs` says), and not from cargo's target folder. A
/// keepane just built is newer than the installed one whenever the version
/// was raised for a release: it must not take the installed one's place
/// (`cargo clean` would then leave logon with nothing to start).
pub(crate) fn installed_self() -> Option<PathBuf> {
    if !crate::legacy::uses_real_dirs() {
        return None;
    }
    let me = std::env::current_exe().ok()?;
    (!is_cargo_build(&me)).then_some(me)
}

/// Whether `exe` is in cargo's build output: a folder above it has the
/// `CACHEDIR.TAG` cargo writes into every target directory.
pub(crate) fn is_cargo_build(exe: &Path) -> bool {
    exe.ancestors().skip(1).any(|d| {
        std::fs::read_to_string(d.join("CACHEDIR.TAG"))
            .is_ok_and(|t| t.starts_with("Signature: 8a477f597d28d172789f06886806bc55") && t.contains("cargo"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keepane-takeover-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn versions_are_read() {
        assert_eq!(parse_version("keepane 0.38.1\n"), Some((0, 38, 1)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("keepane 1.2.3-rc.1"), Some((1, 2, 3)));
        for bad in ["keepane", "keepane 1.2", "1.2.3.4", "keepane x.y.z", ""] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
        assert_eq!(this_version(), parse_version(env!("CARGO_PKG_VERSION")).unwrap());
    }

    /// Which registrations are taken over: one running an older keepane or
    /// one that is gone; never the same file (however it is spelled or
    /// linked), a newer one, the same version elsewhere, or one whose
    /// version cannot be read.
    #[test]
    fn only_older_or_gone_keepanes_are_taken_over() {
        let dir = scratch("stale");
        std::fs::create_dir_all(dir.join("0.38.1")).unwrap();
        std::fs::create_dir_all(dir.join("0.39.0")).unwrap();
        let old = dir.join("0.38.1").join("keepane.exe");
        let me = dir.join("0.39.0").join("keepane.exe");
        std::fs::write(&old, b"").unwrap();
        std::fs::write(&me, b"").unwrap();
        let mine = (0, 39, 0);
        let says = |v: Option<Version>| move |_: &Path| v;
        assert_eq!(stale(&old, &me, mine, says(Some((0, 38, 1)))), Some(Stale::Older((0, 38, 1))));
        assert_eq!(stale(&old, &me, mine, says(Some((0, 39, 0)))), None, "the same version elsewhere");
        assert_eq!(stale(&old, &me, mine, says(Some((0, 40, 0)))), None, "a newer one");
        assert_eq!(stale(&old, &me, mine, says(None)), None, "a version that cannot be read");
        assert_eq!(stale(&dir.join("gone.exe"), &me, mine, says(None)), Some(Stale::Gone));
        // This very file, spelled otherwise: never asked, never taken.
        #[cfg(windows)]
        let other_spelling = PathBuf::from(me.to_string_lossy().to_uppercase().replace('\\', "/"));
        #[cfg(not(windows))]
        let other_spelling = dir.join("0.38.1").join("..").join("0.39.0").join("keepane.exe");
        let asked = std::cell::Cell::new(false);
        let ask = |_: &Path| {
            asked.set(true);
            Some((0, 1, 0))
        };
        assert_eq!(stale(&other_spelling, &me, mine, ask), None);
        assert!(!asked.get());
        // Through a link to its folder (Scoop's `current` junction) or to the
        // file (Homebrew's `bin/keepane`): the same program.
        #[cfg(windows)]
        let linked = {
            let current = dir.join("current");
            let made = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&current)
                .arg(dir.join("0.39.0"))
                .output()
                .unwrap();
            assert!(made.status.success(), "mklink /J: {}", String::from_utf8_lossy(&made.stderr));
            current.join("keepane.exe")
        };
        #[cfg(not(windows))]
        let linked = {
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let link = dir.join("bin").join("keepane");
            std::os::unix::fs::symlink("../0.39.0/keepane.exe", &link).unwrap();
            link
        };
        assert_eq!(stale(&linked, &me, mine, says(Some((0, 1, 0)))), None);
        // A link left pointing at a version that is gone: gone.
        #[cfg(not(windows))]
        {
            let dangling = dir.join("bin").join("keepane-old");
            std::os::unix::fs::symlink("../0.30.0/keepane", &dangling).unwrap();
            assert_eq!(stale(&dangling, &me, mine, says(None)), Some(Stale::Gone));
        }
        #[cfg(windows)]
        let _ = std::fs::remove_dir(dir.join("current"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The version a program says, asked for real: a stand-in that answers,
    /// one that says something else, one too slow, one that is not there.
    #[test]
    fn a_version_is_asked_of_the_program_itself() {
        let dir = scratch("asked");
        #[cfg(windows)]
        let script = |name: &str, body: &str| {
            let p = dir.join(format!("{name}.cmd"));
            std::fs::write(&p, body.lines().map(|l| format!("@{l}\r\n")).collect::<String>()).unwrap();
            p
        };
        #[cfg(not(windows))]
        let script = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n[ \"$1\" = settle ] && exit 0\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            crate::platform::process::wait_runnable(&p, &["settle"]);
            p
        };
        #[cfg(windows)]
        let pause = "ping -n 4 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let pause = "sleep 3";
        assert_eq!(version_of(&script("says", "echo keepane 0.1.2")), Some((0, 1, 2)));
        assert_eq!(version_of(&script("other", "echo something else")), None);
        let slow = script("slow", &format!("{pause}\necho keepane 0.1.2"));
        let t = std::time::Instant::now();
        assert_eq!(version_within(&slow, Duration::from_millis(300)), None);
        assert!(t.elapsed() < Duration::from_secs(2), "not waited out: {:?}", t.elapsed());
        assert_eq!(version_of(&dir.join("nothing.exe")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A version asked once is remembered while the file stays the same;
    /// a changed file is asked again; no answer is not remembered; the
    /// cache keeps the last 16.
    #[test]
    fn a_version_once_asked_is_remembered_for_the_same_file() {
        let dir = scratch("cached");
        let exe = dir.join("keepane.exe");
        let cache = dir.join("data").join("exe-versions");
        std::fs::write(&exe, b"one").unwrap();
        let asked = std::cell::Cell::new(0);
        let ask = |v: Option<Version>| {
            let asked = &asked;
            move |_: &Path| {
                asked.set(asked.get() + 1);
                v
            }
        };
        assert_eq!(version_cached(&exe, &cache, ask(Some((0, 38, 1)))), Some((0, 38, 1)));
        assert_eq!(version_cached(&exe, &cache, ask(Some((9, 9, 9)))), Some((0, 38, 1)), "remembered");
        assert_eq!(asked.get(), 1);
        // Replaced by another build: asked again, and only that is kept for it.
        std::fs::write(&exe, b"another one").unwrap();
        assert_eq!(version_cached(&exe, &cache, ask(Some((0, 39, 0)))), Some((0, 39, 0)));
        assert_eq!(asked.get(), 2);
        assert_eq!(std::fs::read_to_string(&cache).unwrap().lines().count(), 1);
        // No answer: not remembered, asked the next time.
        let other = dir.join("other.exe");
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(version_cached(&other, &cache, ask(None)), None);
        assert_eq!(version_cached(&other, &cache, ask(Some((0, 1, 0)))), Some((0, 1, 0)));
        assert_eq!(asked.get(), 4);
        // Not there: not asked.
        assert_eq!(version_cached(&dir.join("gone.exe"), &cache, ask(Some((0, 1, 0)))), None);
        assert_eq!(asked.get(), 4);
        // Many programs: the last 16.
        for i in 0..20 {
            let e = dir.join(format!("k{i}.exe"));
            std::fs::write(&e, b"k").unwrap();
            version_cached(&e, &cache, ask(Some((0, 0, i))));
        }
        let text = std::fs::read_to_string(&cache).unwrap();
        assert_eq!(text.lines().count(), 16, "{text}");
        assert!(text.contains("k19.exe") && !text.contains("k3.exe\t"), "{text}");
        // A cache spoilt by hand or by a crash: what cannot be read is asked.
        std::fs::write(&cache, "junk\n\t\t\nno tab here\x00\n").unwrap();
        assert_eq!(version_cached(&exe, &cache, ask(Some((0, 39, 0)))), Some((0, 39, 0)));
        assert_eq!(asked.get(), 25);
        assert_eq!(version_cached(&exe, &cache, ask(None)), Some((0, 39, 0)), "and remembered after");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cargo_build_is_told_from_an_install() {
        let me = std::env::current_exe().unwrap(); // this test, in target/debug/deps
        assert!(is_cargo_build(&me), "{}", me.display());
        let dir = scratch("installed");
        assert!(!is_cargo_build(&dir.join("keepane.exe")));
        // Another program's CACHEDIR.TAG is not cargo's.
        std::fs::write(dir.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# by another tool\n")
            .unwrap();
        assert!(!is_cargo_build(&dir.join("keepane.exe")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_line_names_both_programs_and_why() {
        let (me, old) = (Path::new("/new/keepane"), Path::new("/old/keepane"));
        assert_eq!(
            said("start at login", me, old, &Stale::Gone),
            "start at login now runs /new/keepane (it ran /old/keepane, which is gone)"
        );
        assert!(said("x", me, old, &Stale::Older((0, 38, 1))).ends_with("(it ran /old/keepane, version 0.38.1)"));
    }
}
