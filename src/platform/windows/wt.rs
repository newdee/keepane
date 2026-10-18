//! `keepane windows-terminal install|remove|status`: a "keepane" profile in the
//! Windows Terminal dropdown that attaches to (or starts) a session.
//!
//! Windows Terminal reads profile *fragments* from
//! `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\<app>\*.json` and
//! merges them into its settings, so nothing here touches settings.json:
//! the file is ours, adding it and removing it are both one file operation,
//! and the user's own edits to the profile (font, colours) live on in their
//! settings keyed by the profile's GUID, which is why the GUID is fixed.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Where Windows Terminal looks for our fragments.
pub fn fragments_dir() -> Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
    Ok(Path::new(&local).join("Microsoft").join("Windows Terminal").join("Fragments").join("keepane"))
}

/// One profile per socket, so two servers can both have an entry.
fn profile_name(socket: &str) -> String {
    if socket == "default" { "keepane".to_string() } else { format!("keepane ({socket})") }
}

fn file_name(socket: &str) -> String {
    if socket == "default" { "keepane.json".to_string() } else { format!("keepane-{socket}.json") }
}

/// A GUID that is a function of the socket name, so reinstalling keeps the
/// profile's identity (and the user's tweaks to it) instead of adding a
/// second "keepane" to the dropdown.
fn guid(socket: &str) -> String {
    // Two FNV-1a passes over the name with different seeds give 128 bits;
    // uniqueness across socket names is all that is needed, not secrecy.
    fn fnv(seed: u64, s: &str) -> u64 {
        s.bytes().fold(seed, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
    }
    let a = fnv(0xcbf29ce484222325, socket);
    let b = fnv(0x84222325cbf29ce4, socket);
    // Version 4 and the RFC 4122 variant, so it is a well-formed GUID to
    // anything that checks, not just a well-shaped one.
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        (a as u16 & 0x0fff) | 0x4000,
        ((b >> 48) as u16 & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

/// What the profile runs: attach to the session `main` on this server, or
/// start it, as tmux users do with `tmux new -A -s main`. Every tab of the
/// profile joins that same session; `keepane new` in it makes another.
fn command_line(exe: &str, socket: &str) -> String {
    let sock = if socket == "default" { String::new() } else { format!(" -L {socket}") };
    format!("\"{exe}\"{sock} new-session -A -s main")
}

fn fragment(exe: &str, socket: &str) -> serde_json::Value {
    serde_json::json!({
        "profiles": [
            {
                "guid": guid(socket),
                "name": profile_name(socket),
                "commandline": command_line(exe, socket),
                "startingDirectory": "%USERPROFILE%",
            }
        ]
    })
}

/// Write the fragment. Idempotent: writing it again replaces it, same GUID.
pub fn install_in(dir: &Path, socket: &str) -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    install_exe_in(dir, socket, &exe.to_string_lossy())
}

/// The fragment running `exe` (`install_in`'s, or a newer keepane taking it
/// over: `startup::refresh_registrations`).
pub fn install_exe_in(dir: &Path, socket: &str, exe: &str) -> Result<String> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(file_name(socket));
    let text = serde_json::to_string_pretty(&fragment(exe, socket))?;
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(format!(
        "\"{}\" is in the Windows Terminal dropdown (open a new tab, or restart Windows Terminal).\n\
         where: {}\n\
         runs:  {}\n\
         `keepane windows-terminal remove` takes it out; `keepane windows-terminal status` shows it.",
        profile_name(socket),
        path.display(),
        command_line(exe, socket)
    ))
}

pub fn remove_in(dir: &Path, socket: &str) -> Result<String> {
    let path = dir.join(file_name(socket));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(format!("\"{}\": removed from Windows Terminal", profile_name(socket))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(format!("\"{}\": not installed", profile_name(socket)))
        }
        Err(e) => bail!("cannot remove {}: {e}", path.display()),
    }
}

/// The command line the installed profile runs, or None when there is no
/// fragment (or not one of ours).
pub fn status_in(dir: &Path, socket: &str) -> Result<Option<String>> {
    let path = dir.join(file_name(socket));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    Ok(v["profiles"][0]["commandline"].as_str().map(str::to_string))
}

/// `keepane windows-terminal [install|remove|status]`, run on the client side.
pub fn run(socket: &str, args: &[String]) -> Result<i32> {
    // One verb and nothing after it: a typo must not look like it worked.
    if args.len() > 1 {
        bail!("windows-terminal: unexpected argument '{}'", args[1]);
    }
    let dir = fragments_dir()?;
    match args.first().map(String::as_str) {
        Some("install") | Some("on") | Some("add") => {
            println!("{}", install_in(&dir, socket)?);
            Ok(0)
        }
        Some("remove") | Some("off") | Some("uninstall") => {
            println!("{}", remove_in(&dir, socket)?);
            Ok(0)
        }
        None | Some("status") => match status_in(&dir, socket)? {
            Some(cmd) => {
                println!("\"{}\": installed in Windows Terminal, runs:\n  {cmd}", profile_name(socket));
                Ok(0)
            }
            None => {
                println!(
                    "\"{}\": not installed (`keepane windows-terminal install` adds it to the dropdown)",
                    profile_name(socket)
                );
                Ok(1)
            }
        },
        Some(other) => bail!("windows-terminal: expected install, remove or status, got '{other}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_guids_and_command_lines() {
        assert_eq!(profile_name("default"), "keepane");
        assert_eq!(profile_name("work"), "keepane (work)");
        assert_eq!(file_name("default"), "keepane.json");
        assert_eq!(file_name("work"), "keepane-work.json");
        // A GUID in the braces form Windows Terminal uses, stable per socket
        // and different between sockets.
        let g = guid("default");
        assert_eq!(g.len(), 38, "{g}");
        assert!(g.starts_with('{') && g.ends_with('}'), "{g}");
        assert_eq!(g.matches('-').count(), 4, "{g}");
        assert_eq!(g, guid("default"));
        assert_ne!(g, guid("work"));
        // {xxxxxxxx-xxxx-4xxx-Vxxx-xxxxxxxxxxxx}: version 4, variant 8/9/a/b.
        for s in ["default", "work", "a b", "中文"] {
            let g = guid(s);
            assert_eq!(g.as_bytes()[15], b'4', "{g}");
            assert!(matches!(g.as_bytes()[20], b'8' | b'9' | b'a' | b'b'), "{g}");
        }
        let c = command_line(r"C:\Program Files\keepane\keepane.exe", "default");
        assert_eq!(c, r#""C:\Program Files\keepane\keepane.exe" new-session -A -s main"#);
        assert_eq!(command_line("w.exe", "work"), r#""w.exe" -L work new-session -A -s main"#);
    }

    #[test]
    fn the_fragment_is_what_windows_terminal_expects() {
        let f = fragment(r"C:\x\keepane.exe", "default");
        let p = &f["profiles"][0];
        assert_eq!(p["name"], "keepane");
        assert_eq!(p["guid"], guid("default"));
        assert_eq!(p["commandline"], r#""C:\x\keepane.exe" new-session -A -s main"#);
        assert_eq!(p["startingDirectory"], "%USERPROFILE%");
        // Round-trips through the JSON text a fragment file holds.
        let text = serde_json::to_string_pretty(&f).unwrap();
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn install_status_remove_round_trip_in_a_scratch_dir() {
        let dir = std::env::temp_dir().join(format!("keepane-wt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(status_in(&dir, "default").unwrap().is_none(), "no dir yet means not installed");
        let msg = install_in(&dir, "default").unwrap();
        assert!(msg.contains("new-session -A -s main"), "{msg}");
        assert!(dir.join("keepane.json").is_file());
        let cmd = status_in(&dir, "default").unwrap().expect("installed");
        assert!(cmd.ends_with(" new-session -A -s main"), "{cmd}");
        // A second socket is a second file, not a rewrite of the first.
        install_in(&dir, "work").unwrap();
        assert!(dir.join("keepane-work.json").is_file());
        assert!(status_in(&dir, "default").unwrap().is_some());
        assert!(remove_in(&dir, "default").unwrap().contains("removed"));
        assert!(status_in(&dir, "default").unwrap().is_none());
        assert!(remove_in(&dir, "default").unwrap().contains("not installed"), "removing twice is not an error");
        assert!(status_in(&dir, "work").unwrap().is_some(), "the other socket's profile stays");
        // A file that is not JSON is an error with the path in it, not a panic.
        std::fs::write(dir.join("keepane.json"), "{ not json").unwrap();
        let e = status_in(&dir, "default").unwrap_err().to_string();
        assert!(e.contains("keepane.json") && e.contains("not JSON"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_verb_is_refused() {
        let e = run("default", &["sideways".to_string()]).unwrap_err().to_string();
        assert!(e.contains("expected install, remove or status"), "{e}");
        // ...and so is anything after the verb, before any file is written.
        let e = run("default", &["install".to_string(), "extra".to_string()]).unwrap_err().to_string();
        assert!(e.contains("unexpected argument 'extra'"), "{e}");
    }
}
