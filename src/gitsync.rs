//! Commits sent to another machine (`keepane clone`, `keepane push`;
//! docs/design/link-push.md): what git does at each end. Here, a bundle of
//! the commits the other end lacks; there, a look at the repository at a
//! path, and the bundle taken in: a new repository (`clone`), or the
//! commits fetched beside the branch and the branch moved on when nothing
//! of that side's is in the way (`push`). No server state; each call runs
//! git and waits for it, so the server calls these on a thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long a git command that writes may take (a large bundle is made or
/// taken in).
pub const WRITE_LIMIT: Duration = Duration::from_secs(120);

/// A bundle goes in pieces of this many bytes: each request's body stays
/// within what `keepane web` takes from another machine once in base64.
pub const CHUNK: usize = 512 * 1024;

/// The tips the other side has that are left out of a bundle, at most.
const EXCLUDED_MAX: usize = 256;

/// Run git in `dir`: what it printed, or what it said when it failed (its
/// last lines), or that it ran past `limit` (it is stopped then). Never asks
/// for anything; never runs the repository's file watcher or pager (as
/// `git::run`).
pub fn git(git: &Path, dir: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut c = crate::platform::process::quiet_command(&git.to_string_lossy());
    c.arg("--no-pager")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let what = args.first().copied().unwrap_or_default();
    let mut child = c.spawn().map_err(|e| format!("git could not be run: {e}"))?;
    // Both read as they come, so a long answer never fills a pipe and stalls git.
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {what} ran past {}s and was stopped", limit.as_secs()));
            }
        }
    };
    let out = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    if status.success() {
        return Ok(out);
    }
    let said: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let said = said[said.len().saturating_sub(3)..].join("; ");
    Err(if said.is_empty() { format!("git {what} failed") } else { said })
}

/// A name another machine is known by in a repository here: its remote and
/// the `refs/remotes/<name>/…` its commits go under. Letters, digits, `-`,
/// `_` and inner `.` (git's rules for a ref's part, kept short); `remote`
/// when nothing of it is left.
pub fn remote_name(host: &str) -> String {
    let s: String = host
        .chars()
        .take(64)
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let s = s.replace("..", "-");
    let s = s.trim_matches(|c| c == '-' || c == '.');
    let s = s.strip_suffix(".lock").unwrap_or(s);
    if s.is_empty() { "remote".into() } else { s.to_string() }
}

/// A branch name keepane sends or takes: git's rules for a branch, as
/// `git check-ref-format --branch` would say, without running git.
pub fn branch_ok(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 200
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("//")
        && !b.contains("@{")
        && b != "@"
        && !b.split('/').any(|p| p.starts_with('.') || p.ends_with(".lock"))
        && !b.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c))
}

/// The path on this machine that `asked` names, for a repository called
/// `name` from the other side: empty, the default directory and the name;
/// `~` this user's home; relative, under the default directory. None for
/// a name that would step out of the directory (`..`, a separator).
pub fn resolve_path(asked: &str, name: &str, default_dir: &Path, home: &Path) -> Option<PathBuf> {
    let asked = asked.trim();
    if asked.is_empty() {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return None;
        }
        return Some(tidy(&default_dir.join(name)));
    }
    let p = if asked == "~" {
        home.to_path_buf()
    } else if let Some(rest) = asked.strip_prefix("~/").or_else(|| asked.strip_prefix("~\\")) {
        home.join(rest)
    } else {
        let p = PathBuf::from(asked);
        if p.is_absolute() { p } else { default_dir.join(p) }
    };
    Some(tidy(&p))
}

/// `p` with its `.` and `..` worked out, as written (no link followed): the
/// path a check of where it is can trust, and one to show.
pub fn tidy(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// What the other side says of the place a bundle would go (a field a
/// keepane of another version leaves out reads as nothing).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The path as that side understands it.
    pub path: String,
    /// Nothing there yet, or an empty directory: a clone may go there.
    pub free: bool,
    /// A repository's working tree is there (its root).
    pub repo: bool,
    /// Its branch (none when detached).
    pub current: Option<String>,
    /// The commit the branch asked about is at there, if it is there.
    pub tip: Option<String>,
    /// Every commit its branches and remote branches are at.
    pub tips: Vec<String>,
    /// Its first commits (those with no parent).
    pub roots: Vec<String>,
    /// No change to a tracked file (new files do not count).
    pub clean: bool,
}

/// `path` here, for a bundle of `branch`.
pub fn look(git_cmd: &Path, path: &Path, branch: &str) -> Look {
    let mut l = Look { path: path.to_string_lossy().into_owned(), ..Look::default() };
    l.free = match std::fs::read_dir(path) {
        Ok(mut d) => d.next().is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    };
    let Some(repo) = crate::git::find(&l.path) else { return l };
    if repo.key() != crate::git::real(path) {
        // A directory inside another repository: not one of its own.
        return l;
    }
    l.repo = true;
    let run = |args: &[&str]| git(git_cmd, path, args, crate::git::LIMIT).ok();
    l.current = run(&["symbolic-ref", "-q", "--short", "HEAD"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    l.tip = run(&["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut tips: Vec<String> = run(&["for-each-ref", "--format=%(objectname)", "refs/heads", "refs/remotes"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    tips.sort();
    tips.dedup();
    l.tips = tips;
    l.roots = run(&["rev-list", "--max-parents=0", "--all"]).unwrap_or_default().lines().map(str::to_string).collect();
    l.clean = run(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| s.trim().is_empty());
    l
}

/// The first commits of `branch` here.
pub fn roots(git_cmd: &Path, root: &Path, branch: &str) -> Vec<String> {
    git(git_cmd, root, &["rev-list", "--max-parents=0", &format!("refs/heads/{branch}")], crate::git::LIMIT)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A bundle made: how many commits it carries, and its size.
#[derive(Debug, PartialEq, Eq)]
pub enum Bundled {
    /// The other side has every commit of the branch already.
    Nothing,
    Made {
        commits: u64,
        size: u64,
    },
}

/// A bundle at `out` of `branch` in the repository at `root`, without the
/// commits `theirs` (what the other side's branches are at) already holds;
/// those of `theirs` this side does not have are no use to leave out.
pub fn bundle(git_cmd: &Path, root: &Path, branch: &str, theirs: &[String], out: &Path) -> Result<Bundled, String> {
    let run = |args: &[&str]| git(git_cmd, root, args, crate::git::LIMIT);
    // A shallow clone lacks the history before its cut: a bundle of it would
    // carry commits whose parents the other side cannot have.
    if run(&["rev-parse", "--is-shallow-repository"]).is_ok_and(|s| s.trim() == "true") {
        return Err(
            "this repository is a shallow clone (its early history is not here): git fetch --unshallow first".into()
        );
    }
    let full = format!("refs/heads/{branch}");
    let tip = run(&["rev-parse", "-q", "--verify", &format!("{full}^{{commit}}")])
        .map_err(|_| format!("there is no branch {branch} here, or it has no commit"))?
        .trim()
        .to_string();
    let known: Vec<&String> = theirs
        .iter()
        .filter(|c| c.len() >= 40 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .filter(|c| run(&["cat-file", "-e", &format!("{c}^{{commit}}")]).is_ok())
        .take(EXCLUDED_MAX)
        .collect();
    let mut count = vec!["rev-list".to_string(), "--count".into(), tip.clone()];
    if !known.is_empty() {
        count.push("--not".into());
        count.extend(known.iter().map(|c| c.to_string()));
    }
    let count: Vec<&str> = count.iter().map(String::as_str).collect();
    let commits: u64 = run(&count)?.trim().parse().unwrap_or(0);
    if commits == 0 {
        return Ok(Bundled::Nothing);
    }
    let out_s = out.to_string_lossy().into_owned();
    let mut args = vec!["bundle".to_string(), "create".into(), "-q".into(), out_s, full];
    if !known.is_empty() {
        args.push("--not".into());
        args.extend(known.iter().map(|c| c.to_string()));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    git(git_cmd, root, &args, WRITE_LIMIT)?;
    let size = std::fs::metadata(out).map_err(|e| format!("the bundle: {e}"))?.len();
    Ok(Bundled::Made { commits, size })
}

/// A new repository at `path` from the bundle: `branch` checked out, the
/// bundle's remote named `from` and pointed at `url` (the bundle file goes).
pub fn clone(
    git_cmd: &Path,
    bundle: &Path,
    path: &Path,
    branch: &str,
    from: &str,
    url: &str,
) -> Result<String, String> {
    let parent = path.parent().ok_or_else(|| format!("{}: not a place for a repository", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let (b, p) = (bundle.to_string_lossy().into_owned(), path.to_string_lossy().into_owned());
    git(git_cmd, parent, &["clone", "-q", "--branch", branch, &b, &p], WRITE_LIMIT)?;
    git(git_cmd, path, &["remote", "rename", "origin", from], WRITE_LIMIT)?;
    git(git_cmd, path, &["remote", "set-url", from, url], WRITE_LIMIT)?;
    let n = git(git_cmd, path, &["rev-list", "--count", "HEAD"], crate::git::LIMIT).unwrap_or_default();
    Ok(format!("{}: cloned, {branch} checked out ({} commits)", path.display(), n.trim()))
}

/// How a push ended for the branch there.
#[derive(Debug, PartialEq, Eq)]
pub enum Landed {
    /// The branch already was where the bundle's is.
    Already,
    /// The branch (checked out, and its files) moved on.
    FastForwarded,
    /// The branch, not checked out, moved on.
    Moved,
    /// The branch was not there: made.
    Made,
    /// Left as it was; the commits are in `refs/remotes/<from>/<branch>`.
    Kept(String),
}

/// The bundle taken into the repository at `path`: its commits under
/// `refs/remotes/<from>/<branch>`, then `branch` moved on to them when it
/// can be without losing anything of this side's (a fast-forward, the files
/// too when it is checked out and no tracked file is changed); left alone
/// otherwise, and why said. Returns what happened and how many commits came.
pub fn take(git_cmd: &Path, bundle: &Path, path: &Path, branch: &str, from: &str) -> Result<(Landed, u64), String> {
    let run = |args: &[&str]| git(git_cmd, path, args, WRITE_LIMIT);
    let theirs = format!("refs/remotes/{from}/{branch}");
    let had = run(&["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    run(&["fetch", "-q", &bundle.to_string_lossy(), &format!("+refs/heads/{branch}:{theirs}")])?;
    let new = run(&["rev-parse", &theirs])?.trim().to_string();
    let came: u64 = match &had {
        Some(h) => run(&["rev-list", "--count", &format!("{h}..{new}")]),
        None => run(&["rev-list", "--count", &new]),
    }?
    .trim()
    .parse()
    .unwrap_or(0);
    let current = run(&["symbolic-ref", "-q", "--short", "HEAD"]).ok().map(|s| s.trim().to_string());
    let checked_out = current.as_deref() == Some(branch);
    if had.as_deref() == Some(new.as_str()) {
        return Ok((Landed::Already, 0));
    }
    let forward = match &had {
        None => true,
        Some(h) => run(&["merge-base", "--is-ancestor", h, &new]).is_ok(),
    };
    let short_from = format!("{from}/{branch}");
    if !forward {
        let mine = had.as_deref().map_or(0, |h| {
            run(&["rev-list", "--count", &format!("{new}..{h}")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
        });
        return Ok((
            Landed::Kept(format!(
                "this side has {mine} commit(s) of its own on {branch}: merge or rebase {short_from} there"
            )),
            came,
        ));
    }
    if checked_out {
        let clean = run(&["status", "--porcelain", "--untracked-files=no"]).is_ok_and(|s| s.trim().is_empty());
        if !clean {
            return Ok((
                Landed::Kept(format!("files are changed there: commit or stash them, then merge {short_from}")),
                came,
            ));
        }
        return match run(&["merge", "-q", "--ff-only", &theirs]) {
            Ok(_) => Ok((Landed::FastForwarded, came)),
            Err(e) => Ok((Landed::Kept(format!("{branch} was not moved ({e}): merge {short_from} there")), came)),
        };
    }
    match &had {
        None => {
            run(&["branch", "-q", branch, &theirs])?;
            Ok((Landed::Made, came))
        }
        Some(h) => {
            run(&["update-ref", &format!("refs/heads/{branch}"), &new, h])?;
            Ok((Landed::Moved, came))
        }
    }
}

/// What a push says of how it ended there.
pub fn said(path: &str, branch: &str, from: &str, landed: &Landed, came: u64) -> String {
    let commits = if came == 1 { "1 commit".to_string() } else { format!("{came} commits") };
    match landed {
        Landed::Already => format!("{path}: {branch} already up to date"),
        Landed::FastForwarded => format!("{path}: {branch} {commits}, fast-forwarded (files too)"),
        Landed::Moved => format!("{path}: {branch} {commits}, moved on (not checked out there)"),
        Landed::Made => format!("{path}: {branch} made ({commits})"),
        Landed::Kept(why) => format!("{path}: {commits} kept in {from}/{branch}; {why}"),
    }
}

#[cfg(test)]
mod tests;
