//! A pane's git repository: for the status line and the clock (its branch,
//! what is changed, how far it is from its upstream), and for what an
//! agent's turn did (the commits it made, the files it changed).
//!
//! What the repository's own files say (where it is, its branch, the commit
//! its head is at) is read directly, so it is there without git. What needs
//! git itself runs `git`: never while the server waits for it (the server
//! asks on a thread of its own), with a time limit, and never taking the
//! index's lock from an agent at work in the same repository. Without git,
//! only the first. Platform-neutral but for where git is looked for.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Where a repository is: its working tree, its git directory (`.git`, or a
/// worktree's own), and the directory its branches and objects are in (the
/// same, but for a worktree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub root: PathBuf,
    pub gitdir: PathBuf,
    pub common: PathBuf,
}

/// A path as the file says it: relative to `base` when it is not absolute.
fn under(base: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path.trim());
    if p.is_absolute() { p } else { base.join(p) }
}

/// The repository `dir` is in: `dir` or the nearest directory above it with
/// a `.git` (a directory, or a worktree's file naming its git directory).
/// A relative `dir` is no pane's directory (it would be looked up from
/// wherever this process runs): none.
pub fn find(dir: &str) -> Option<Repo> {
    if dir.is_empty() || !Path::new(dir).is_absolute() {
        return None;
    }
    let mut at = Some(Path::new(dir));
    while let Some(d) = at {
        let dot = d.join(".git");
        let gitdir = if dot.is_dir() {
            Some(dot)
        } else {
            std::fs::read_to_string(&dot).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|g| under(d, g)))
        };
        if let Some(gitdir) = gitdir {
            let common = std::fs::read_to_string(gitdir.join("commondir"))
                .map(|c| under(&gitdir, &c))
                .unwrap_or_else(|_| gitdir.clone());
            return Some(Repo { root: d.to_path_buf(), gitdir, common });
        }
        at = d.parent();
    }
    None
}

/// The branch of the repository `dir` is in ("main"), a short commit when
/// its head is detached ("a1b2c3d"), or empty outside a repository.
pub fn branch_of(dir: &str) -> String {
    find(dir).map(|r| r.branch()).unwrap_or_default()
}

fn is_commit(s: &str) -> bool {
    s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The real path of `p` (links, short names and case worked out), written as
/// people write it (`plain`); `p` as it is when the filesystem cannot say.
pub fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).map(plain).unwrap_or_else(|_| p.to_path_buf())
}

/// A real path as people write it: Windows' `\\?\C:\…` (what canonicalizing
/// gives) as `C:\…`, `\\?\UNC\server\share` as `\\server\share`.
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

/// A commit's short form, as git writes it.
pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

impl Repo {
    /// Which repository this is, whatever the path it was found by: the
    /// same one reached by two spellings (a Windows short name such as
    /// `RUNNER~1`, which a shell may report long; another case; a link) is
    /// one. Its root as found when the filesystem cannot say.
    pub fn key(&self) -> PathBuf {
        real(&self.root)
    }

    fn head_file(&self) -> Option<String> {
        std::fs::read_to_string(self.gitdir.join("HEAD")).ok().map(|h| h.trim().to_string())
    }

    /// The branch its head is on, or the short commit when detached; empty
    /// when the head cannot be read.
    pub fn branch(&self) -> String {
        let Some(head) = self.head_file() else { return String::new() };
        match head.strip_prefix("ref: ") {
            Some(r) => r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
            None => head.chars().take(7).collect(),
        }
    }

    /// The branch its head is on; none when detached or unreadable.
    pub fn on_branch(&self) -> Option<String> {
        let head = self.head_file()?;
        let r = head.strip_prefix("ref: ")?;
        Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
    }

    /// The commit its head is at, in full; none on a branch with no commit
    /// yet, or when it cannot be read.
    pub fn head(&self) -> Option<String> {
        let head = self.head_file()?;
        match head.strip_prefix("ref: ") {
            Some(r) => self.resolve(r.trim(), 0),
            None => is_commit(&head).then_some(head),
        }
    }

    /// The commit a ref names: its own file (a worktree's own refs first,
    /// then the shared ones), else the packed refs. A ref naming another is
    /// followed, a few deep.
    fn resolve(&self, name: &str, depth: u8) -> Option<String> {
        if depth > 4 {
            return None;
        }
        for dir in [&self.gitdir, &self.common] {
            if let Ok(t) = std::fs::read_to_string(dir.join(name)) {
                let t = t.trim();
                if let Some(next) = t.strip_prefix("ref: ") {
                    return self.resolve(next.trim(), depth + 1);
                }
                if is_commit(t) {
                    return Some(t.to_string());
                }
            }
        }
        let packed = std::fs::read_to_string(self.common.join("packed-refs")).ok()?;
        packed.lines().filter(|l| !l.starts_with(['#', '^'])).find_map(|l| {
            let (commit, r) = l.split_once(' ')?;
            (r.trim() == name && is_commit(commit)).then(|| commit.to_string())
        })
    }
}

/// The git to run: `configured` when set (a path, or a name looked up on the
/// PATH), else the one on the PATH, else one where its installers put it;
/// none when there is none. On macOS `/usr/bin/git` stands in for git until
/// the command line tools are installed, and running it asks to install
/// them: it counts only once they are.
pub fn command(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = Path::new(configured);
        return if p.is_absolute() { p.is_file().then(|| p.to_path_buf()) } else { crate::config::which(configured) };
    }
    let found = crate::config::which("git").or_else(|| installed().into_iter().find(|p| p.is_file()))?;
    if cfg!(target_os = "macos") && found == Path::new("/usr/bin/git") {
        let tools = [
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
        ];
        return tools.iter().any(|t| Path::new(t).is_file()).then_some(found);
    }
    Some(found)
}

/// Where git's installers put it, off the PATH: Git for Windows (for every
/// user, or one), the copy GitHub Desktop carries (its newest); Homebrew's.
fn installed() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut out = Vec::new();
    if cfg!(windows) {
        for k in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            out.extend(env(k).map(|d| d.join("Git").join("cmd").join("git.exe")));
        }
        if let Some(local) = env("LOCALAPPDATA") {
            out.push(local.join("Programs").join("Git").join("cmd").join("git.exe"));
            let desktop = local.join("GitHubDesktop");
            let mut apps: Vec<PathBuf> = std::fs::read_dir(&desktop)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-")))
                .collect();
            apps.sort();
            out.extend(apps.into_iter().rev().map(|a| a.join("resources/app/git/cmd/git.exe")));
        }
    } else {
        out.extend(["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"].map(PathBuf::from));
    }
    out
}

/// Run git in `root` with `args`: what it printed, or none when it could not
/// be run, failed, or ran past `limit` (it is stopped then). It never asks
/// for anything, never takes the index's lock (`--no-optional-locks`: an
/// agent at work in the same repository is never kept from it), and never
/// starts a program the repository's own config names for watching files
/// (`core.fsmonitor`) or paging: keepane runs it on its own, in whatever
/// repository a pane is in, so it runs no more of the repository's than it
/// must.
pub fn run(git: &Path, root: &Path, args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    let mut c = crate::platform::process::quiet_command(&git.to_string_lossy());
    c.args(["--no-optional-locks", "--no-pager"])
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = c.spawn().ok()?;
    // Read as it comes, so a long answer never fills the pipe and stalls git.
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let b = reader.join().ok()?;
                return status.success().then_some(b);
            }
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// How long one git command may take before it is stopped.
pub const LIMIT: Duration = Duration::from_secs(10);

/// What `git status` says: the branch, the commit (short), how far ahead of
/// and behind its upstream it is, how many files differ from the commit
/// (staged or not, conflicts among them) and how many new ones git does not
/// track.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    pub head: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub staged: u32,
    pub conflicts: u32,
    pub untracked: u32,
}

impl Status {
    /// What it says in a few marks: `●3` files changed (new ones too), `↑2`
    /// ahead, `↓1` behind, `!1` in conflict; nothing for what there is none of.
    pub fn marks(&self) -> String {
        let mut out = Vec::new();
        let files = self.changed + self.untracked;
        if files > 0 {
            out.push(format!("●{files}"));
        }
        if self.ahead > 0 {
            out.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            out.push(format!("↓{}", self.behind));
        }
        if self.conflicts > 0 {
            out.push(format!("!{}", self.conflicts));
        }
        out.join(" ")
    }
}

/// `git status --porcelain=v2 --branch -z` read: its `# branch.*` headers,
/// then a record a file (`1` changed, `2` renamed or copied, with the old
/// name in the next record, `u` in conflict, `?` new).
pub fn parse_status(out: &[u8]) -> Status {
    let text = String::from_utf8_lossy(out);
    let mut s = Status::default();
    let mut records = text.split('\0');
    while let Some(r) = records.next() {
        if let Some(h) = r.strip_prefix("# branch.") {
            let (key, value) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "oid" if value != "(initial)" => s.head = short(value).to_string(),
                "head" => s.branch = value.to_string(),
                "ab" => {
                    for part in value.split_whitespace() {
                        let n = part[1..].parse().unwrap_or(0);
                        if part.starts_with('+') {
                            s.ahead = n;
                        } else {
                            s.behind = n;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let staged = r.as_bytes().get(2).is_some_and(|&x| x != b'.');
        match r.as_bytes().first() {
            Some(b'1') => s.changed += 1,
            Some(b'2') => {
                s.changed += 1;
                // The name it had: a record of its own.
                records.next();
            }
            Some(b'u') => {
                s.changed += 1;
                s.conflicts += 1;
                continue;
            }
            Some(b'?') => {
                s.untracked += 1;
                continue;
            }
            _ => continue,
        }
        if staged {
            s.staged += 1;
        }
    }
    if s.branch == "(detached)" {
        s.branch = s.head.clone();
    }
    s
}

/// The repository's status, by git; none without it, or when it failed.
pub fn status(git: &Path, repo: &Repo) -> Option<Status> {
    let out = run(git, &repo.root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=normal"], LIMIT)?;
    Some(parse_status(&out))
}

/// A file changed: its path, and the lines added and removed (none for a
/// binary file).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

/// What a stretch of work did in a repository: its branch, the commit it
/// began at and the one it ended at; the commits made in between (short,
/// subject; the newest first, the first few of `commit_count`); what in the
/// working tree differs from where it began (staged or not; the first few of
/// `file_count`, the lines added and removed in all); the new files git
/// does not track yet (the first few of `new_count`). `measured`: git said
/// what changed; without it, only the branch and the commits' ids are known
/// (`failed`: git was there and could not say). `lost_start`: the commit
/// it began at is not in the repository it ended in, so it is measured
/// from where it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Work {
    pub branch: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub commits: Vec<(String, String)>,
    pub commit_count: usize,
    pub files: Vec<File>,
    pub file_count: usize,
    pub added: u64,
    pub removed: u64,
    pub new_files: Vec<String>,
    pub new_count: usize,
    pub measured: bool,
    pub failed: bool,
    pub lost_start: bool,
}

/// How many of each a telling lists; the rest it counts.
const COMMITS_LISTED: usize = 5;
const FILES_LISTED: usize = 10;
const NEW_LISTED: usize = 5;

/// `git diff --numstat -z` read: `added\tremoved\tpath`, or for a rename
/// `added\tremoved\t` then the old and the new name as records of their own;
/// `-` for a binary file's counts.
pub fn parse_numstat(out: &[u8]) -> Vec<File> {
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    let mut files = Vec::new();
    while let Some(r) = records.next() {
        let mut parts = r.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path = if path.is_empty() {
            let _old = records.next();
            records.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        files.push(File { path, added: a.parse().ok(), removed: d.parse().ok() });
    }
    files
}

/// What was done in `repo` since commit `from` (where it is now, when none).
pub fn work(git: Option<&Path>, repo: &Repo, from: Option<&str>) -> Work {
    let to = repo.head();
    let mut from = from.map(String::from).or_else(|| to.clone());
    let mut w = Work { branch: repo.branch(), from: from.clone(), to: to.clone(), ..Work::default() };
    let Some(git) = git else { return w };
    // Something git could not answer: what changed is not known.
    w.failed = true;
    if let (Some(f), Some(t)) = (from.clone(), &to)
        && f != *t
    {
        let range = format!("{f}..{t}");
        let out = run(git, &repo.root, &["log", "--max-count=1000", "--format=%h%x1f%s", &range], LIMIT);
        let Some(out) = out.or_else(|| {
            // The commit it began at is not in this repository (the work
            // went to another one, or that commit was rebased away):
            // measured from where it is now, and said so.
            let known = run(git, &repo.root, &["cat-file", "-e", &format!("{f}^{{commit}}")], LIMIT).is_some();
            if !known {
                (w.lost_start, w.from, from) = (true, Some(t.clone()), Some(t.clone()));
            }
            (!known).then(Vec::new)
        }) else {
            return w;
        };
        let lines: Vec<String> = String::from_utf8_lossy(&out).lines().map(String::from).collect();
        w.commit_count = lines.len();
        w.commits = lines
            .iter()
            .take(COMMITS_LISTED)
            .map(|l| {
                let (id, subject) = l.split_once('\x1f').unwrap_or((l, ""));
                (id.to_string(), subject.to_string())
            })
            .collect();
    }
    if let Some(f) = &from {
        let Some(out) = run(git, &repo.root, &["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", f], LIMIT)
        else {
            return w;
        };
        let files = parse_numstat(&out);
        w.file_count = files.len();
        w.added = files.iter().filter_map(|f| f.added).sum();
        w.removed = files.iter().filter_map(|f| f.removed).sum();
        w.files = files.into_iter().take(FILES_LISTED).collect();
    }
    let Some(out) = run(git, &repo.root, &["ls-files", "--others", "--exclude-standard", "-z"], LIMIT) else {
        return w;
    };
    let new: Vec<&str> = std::str::from_utf8(&out).unwrap_or_default().split('\0').filter(|p| !p.is_empty()).collect();
    w.new_count = new.len();
    w.new_files = new.into_iter().take(NEW_LISTED).map(String::from).collect();
    (w.measured, w.failed) = (true, false);
    w
}

/// `n` things, as a person counts them: "1 file", "3 files".
fn count(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// `n` more of them: "1 more file", "3 more files".
fn more(n: usize, one: &str) -> String {
    if n == 1 { format!("1 more {one}") } else { format!("{n} more {one}s") }
}

impl Work {
    /// One line: the branch and where it went, then what changed.
    /// `main 1a2b3c4..5d6e7f8: 2 commits, 3 files +40 -12, 1 new file`;
    /// `main 5d6e7f8: no changes`; without git, the branch and the commits.
    pub fn summary(&self) -> String {
        let at = match (&self.from, &self.to) {
            (Some(f), Some(t)) if f != t => format!("{}..{}", short(f), short(t)),
            (_, Some(t)) if self.lost_start => format!("{} (start unknown)", short(t)),
            (_, Some(t)) => short(t).to_string(),
            _ => "no commit yet".to_string(),
        };
        let mut what = Vec::new();
        if self.commit_count > 0 {
            what.push(count(self.commit_count, "commit"));
        }
        if self.file_count > 0 {
            what.push(format!("{} +{} -{}", count(self.file_count, "file"), self.added, self.removed));
        }
        if self.new_count > 0 {
            what.push(count(self.new_count, "new file"));
        }
        let what = match (what.is_empty(), self.measured, self.failed) {
            (false, _, _) => what.join(", "),
            (true, true, _) => "no changes".to_string(),
            (true, false, true) => "changes unknown (git failed)".to_string(),
            (true, false, false) => "changes unknown (git not found)".to_string(),
        };
        format!("{} {at}: {what}", self.branch)
    }

    /// The summary, then what it counts: each commit, each file (with what
    /// is left out counted), each new file.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![self.summary()];
        for (id, subject) in &self.commits {
            out.push(format!("  {id} {subject}"));
        }
        if self.commit_count > self.commits.len() {
            out.push(format!("  … {}", more(self.commit_count - self.commits.len(), "commit")));
        }
        for f in &self.files {
            match (f.added, f.removed) {
                (Some(a), Some(d)) => out.push(format!("  {} +{a} -{d}", f.path)),
                _ => out.push(format!("  {} (binary)", f.path)),
            }
        }
        if self.file_count > self.files.len() {
            out.push(format!("  … {}", more(self.file_count - self.files.len(), "file")));
        }
        for p in &self.new_files {
            out.push(format!("  new {p}"));
        }
        if self.new_count > self.new_files.len() {
            out.push(format!("  … {}", more(self.new_count - self.new_files.len(), "new file")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keepane-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The branch and the head commit, read from the files: from a
    /// subdirectory, detached, a worktree (its own head, the shared
    /// branches), a packed branch, a branch with no commit yet; none
    /// outside a repository or for a relative directory.
    #[test]
    fn the_branch_and_the_head_are_read_from_the_files() {
        let dir = scratch("files");
        let (a, b) = ("a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0", "0123456789abcdef0123456789abcdef01234567");
        let repo = dir.join("repo");
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        let git = repo.join(".git");
        std::fs::create_dir_all(git.join("refs").join("heads").join("feature")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        std::fs::write(git.join("refs/heads/feature/x"), format!("{a}\n")).unwrap();
        let r = find(&sub.to_string_lossy()).unwrap();
        assert_eq!((r.root.as_path(), r.branch(), r.head()), (repo.as_path(), "feature/x".into(), Some(a.into())));
        assert_eq!(branch_of(&sub.to_string_lossy()), "feature/x", "found from a subdirectory");
        // Packed: no file of its own.
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("packed-refs"), format!("# pack-refs with: peeled\n{b} refs/heads/main\n^{a}\n"))
            .unwrap();
        assert_eq!(find(&repo.to_string_lossy()).unwrap().head(), Some(b.into()));
        // No commit yet on it.
        std::fs::write(git.join("HEAD"), "ref: refs/heads/fresh\n").unwrap();
        assert_eq!(find(&repo.to_string_lossy()).unwrap().head(), None);
        std::fs::write(git.join("HEAD"), format!("{a}\n")).unwrap();
        let r = find(&repo.to_string_lossy()).unwrap();
        assert_eq!((r.branch(), r.head()), ("a1b2c3d".into(), Some(a.into())), "detached: the short commit");
        // A worktree: `.git` a file naming its git directory, which names the
        // shared one; its head its own, the branch from the shared refs.
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let wgit = git.join("worktrees").join("wt");
        std::fs::create_dir_all(&wgit).unwrap();
        std::fs::write(wgit.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        std::fs::write(wgit.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wgit.display())).unwrap();
        let r = find(&wt.to_string_lossy()).unwrap();
        assert_eq!((r.branch(), r.head()), ("feature/x".into(), Some(a.into())));
        assert_eq!(std::fs::canonicalize(&r.common).unwrap(), std::fs::canonicalize(&git).unwrap());
        assert_eq!(branch_of(&dir.to_string_lossy()), "", "no repository above");
        let nowhere = if cfg!(windows) { r"Z:\no\such\place" } else { "/no/such/place" };
        assert_eq!((branch_of(nowhere), find("src"), find("")), (String::new(), None, None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One repository, however the path it is found by is spelt: through
    /// `..`, in another case (Windows), through a link (Unix); another
    /// repository is another; a root that is gone is kept as found.
    #[test]
    fn one_repository_however_its_path_is_spelt() {
        let dir = scratch("key");
        let repo = dir.join("Repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let key = |p: &Path| find(&p.to_string_lossy()).unwrap().key();
        let plain = key(&repo);
        let mut spellings = vec![repo.join("src").join("..")];
        if cfg!(windows) {
            spellings.push(PathBuf::from(repo.to_string_lossy().to_uppercase()));
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&repo, dir.join("link")).unwrap();
            spellings.push(dir.join("link"));
        }
        for s in &spellings {
            let r = find(&s.to_string_lossy()).unwrap();
            assert_ne!(r.root, repo, "{s:?} is spelt otherwise");
            assert_eq!(r.key(), plain, "{s:?}");
        }
        let other = dir.join("other");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        assert_ne!(key(&other), plain);
        assert_eq!(super::plain(PathBuf::from(r"\\?\C:\x\y")), PathBuf::from(r"C:\x\y"));
        assert_eq!(super::plain(PathBuf::from(r"\\?\UNC\srv\share\r")), PathBuf::from(r"\\srv\share\r"));
        assert_eq!(super::plain(PathBuf::from("/home/u/r")), PathBuf::from("/home/u/r"));
        let gone = Repo { root: dir.join("gone"), gitdir: dir.join("gone/.git"), common: dir.join("gone/.git") };
        assert_eq!(gone.key(), dir.join("gone"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `git status --porcelain=v2` read: the branch, the commit, ahead and
    /// behind, files changed (staged, renamed, in conflict) and new ones;
    /// detached, the commit as the branch; a repository with no commit.
    #[test]
    fn a_status_is_read_from_porcelain() {
        let out = b"# branch.oid 5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e\0# branch.head main\0\
# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aa bb src/a.rs\0\
1 M. N... 100644 100644 100644 aa bb src/b.rs\0\
2 R. N... 100644 100644 100644 aa bb R100 src/new name.rs\x001st.rs\0\
u UU N... 100644 100644 100644 100644 aa bb cc src/c.rs\0? notes.md\0? tmp/x\0";
        let s = parse_status(out);
        assert_eq!(
            s,
            Status {
                branch: "main".into(),
                head: "5d6e7f8".into(),
                ahead: 2,
                behind: 1,
                changed: 4,
                staged: 2,
                conflicts: 1,
                untracked: 2
            }
        );
        assert_eq!(s.marks(), "●6 ↑2 ↓1 !1");
        let detached =
            parse_status(b"# branch.oid 5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e\0# branch.head (detached)\0");
        assert_eq!((detached.branch.as_str(), detached.marks()), ("5d6e7f8", String::new()));
        let fresh = parse_status(b"# branch.oid (initial)\0# branch.head main\0? a\0");
        assert_eq!((fresh.head.as_str(), fresh.untracked, fresh.marks()), ("", 1, "●1".to_string()));
        assert_eq!(parse_status(b""), Status::default());
    }

    /// `git diff --numstat -z` read, a rename and a binary file among them.
    #[test]
    fn numstat_is_read_with_renames_and_binaries() {
        let out = b"30\t2\tsrc/a.rs\0-\t-\tlogo.png\x005\t0\t\0src/old.rs\0src/new.rs\0";
        assert_eq!(
            parse_numstat(out),
            vec![
                File { path: "src/a.rs".into(), added: Some(30), removed: Some(2) },
                File { path: "logo.png".into(), added: None, removed: None },
                File { path: "src/new.rs".into(), added: Some(5), removed: Some(0) },
            ]
        );
        assert!(parse_numstat(b"").is_empty());
    }

    /// What a stretch of work did, said in a line and listed: the commits,
    /// the files with their lines, the new files, what is left out counted.
    #[test]
    fn work_reads_as_a_line_and_a_list() {
        let (f, t) = ("1a2b3c4d".repeat(5), "5d6e7f8a".repeat(5));
        let mut w = Work {
            branch: "main".into(),
            from: Some(f.clone()),
            to: Some(t.clone()),
            measured: true,
            ..Work::default()
        };
        assert_eq!(w.summary(), "main 1a2b3c4..5d6e7f8: no changes");
        w.commits = vec![("5d6e7f8".into(), "parse the header".into())];
        w.commit_count = 2;
        w.files = vec![
            File { path: "src/a.rs".into(), added: Some(30), removed: Some(2) },
            File { path: "logo.png".into(), added: None, removed: None },
        ];
        (w.file_count, w.added, w.removed) = (3, 40, 12);
        (w.new_files, w.new_count) = (vec!["notes.md".into()], 1);
        assert_eq!(w.summary(), "main 1a2b3c4..5d6e7f8: 2 commits, 3 files +40 -12, 1 new file");
        assert_eq!(
            w.lines(),
            [
                "main 1a2b3c4..5d6e7f8: 2 commits, 3 files +40 -12, 1 new file",
                "  5d6e7f8 parse the header",
                "  … 1 more commit",
                "  src/a.rs +30 -2",
                "  logo.png (binary)",
                "  … 1 more file",
                "  new notes.md"
            ]
        );
        let unknown = Work { branch: "main".into(), from: Some(t.clone()), to: Some(t), ..Work::default() };
        assert_eq!(unknown.summary(), "main 5d6e7f8: changes unknown (git not found)");
        let fresh = Work { branch: "main".into(), measured: true, ..Work::default() };
        assert_eq!(fresh.summary(), "main no commit yet: no changes");
        let failed = Work { failed: true, ..unknown.clone() };
        assert_eq!(failed.summary(), "main 5d6e7f8: changes unknown (git failed)");
        let lost = Work { lost_start: true, measured: true, file_count: 1, added: 1, ..unknown };
        assert_eq!(lost.summary(), "main 5d6e7f8 (start unknown): 1 file +1 -0");
        // An older event log's work, without the later fields: read all the same.
        let old: Work = serde_json::from_str(r#"{"branch":"main","measured":true}"#).unwrap();
        assert_eq!(
            (old.failed, old.lost_start, old.summary()),
            (false, false, "main no commit yet: no changes".into())
        );
    }

    /// With git: a repository made, committed in, changed; its status, and
    /// what was done since the first commit (a commit, a changed file, a new
    /// one); a git that cannot be run is none, not a wait.
    #[test]
    fn git_itself_says_the_status_and_the_work() {
        let Some(git) = command("") else {
            eprintln!("no git here: skipped");
            return;
        };
        let dir = scratch("real");
        let g = |args: &[&str]| {
            let ok = std::process::Command::new(&git)
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        g(&["add", "a.txt"]);
        g(&["commit", "-q", "-m", "first"]);
        let repo = find(&dir.to_string_lossy()).unwrap();
        let first = repo.head().unwrap();
        std::fs::write(dir.join("b.txt"), "two\n").unwrap();
        g(&["add", "b.txt"]);
        g(&["commit", "-q", "-m", "second"]);
        std::fs::write(dir.join("a.txt"), "one\nmore\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
        let s = status(&git, &repo).unwrap();
        assert_eq!((s.branch.as_str(), s.changed, s.untracked, s.staged), ("main", 1, 1, 0), "{s:?}");
        assert_eq!(s.head, short(&repo.head().unwrap()));
        let w = work(Some(&git), &repo, Some(&first));
        assert_eq!((w.commit_count, w.commits[0].1.as_str()), (1, "second"), "{w:?}");
        let paths: Vec<&str> = w.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt"], "since the first commit: the commit's file and the changed one");
        assert_eq!((w.added, w.removed, w.new_files.clone()), (2, 0, vec!["new.txt".to_string()]));
        assert!(w.measured);
        // From where it is now: the change in the working tree only.
        let now = work(Some(&git), &repo, None);
        assert_eq!((now.commit_count, now.file_count, now.new_count), (0, 1, 1));
        // Without git: the branch and the commits' ids, nothing measured.
        let bare = work(None, &repo, Some(&first));
        assert_eq!((bare.from.as_deref(), bare.measured, bare.file_count), (Some(first.as_str()), false, 0));
        assert!(bare.summary().ends_with("changes unknown (git not found)"));
        // A git that cannot be run: it failed, not missing.
        let broken = work(Some(&dir.join("not-git")), &repo, Some(&first));
        assert!(broken.failed && broken.summary().ends_with("changes unknown (git failed)"), "{broken:?}");
        // Begun in another repository (or at a commit rebased away): from
        // where it is now, said so; still measured.
        let elsewhere = work(Some(&git), &repo, Some("0123456789abcdef0123456789abcdef01234567"));
        assert!(elsewhere.lost_start && elsewhere.measured && elsewhere.commit_count == 0, "{elsewhere:?}");
        assert_eq!((elsewhere.file_count, elsewhere.new_count), (1, 1));
        assert!(elsewhere.summary().contains(" (start unknown): 1 file +1 -0, 1 new file"), "{}", elsewhere.summary());
        // The repository's config names programs for git to start: a file
        // watcher and an external diff. git of its own starts them; keepane's
        // runs do not.
        let marker = dir.join("ran.txt");
        let script = dir.join("hook.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho ran > '{}'\n", marker.display().to_string().replace('\\', "/")),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let hook = script.display().to_string().replace('\\', "/");
        g(&["config", "core.fsmonitor", &hook]);
        g(&["config", "diff.external", &hook]);
        let _ = std::process::Command::new(&git).arg("-C").arg(&dir).args(["status"]).output();
        let _ = std::process::Command::new(&git).arg("-C").arg(&dir).args(["diff", "HEAD"]).output();
        if marker.exists() {
            std::fs::remove_file(&marker).unwrap();
            assert!(status(&git, &repo).is_some());
            assert!(work(Some(&git), &repo, Some(&first)).measured);
            assert!(!marker.exists(), "keepane's git started the repository's program");
        } else {
            eprintln!("this git started neither program on its own: the check is moot here");
        }
        g(&["config", "--unset", "core.fsmonitor"]);
        g(&["config", "--unset", "diff.external"]);
        let missing = dir.join(if cfg!(windows) { "no-git.exe" } else { "no-git" });
        let t = Instant::now();
        assert_eq!(run(&missing, &dir, &["status"], LIMIT), None);
        assert!(t.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
