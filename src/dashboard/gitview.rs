//! The board's Git tab, for the chosen pane's directory: in a repository
//! its branch, what is changed and the latest commits; elsewhere the
//! directory's files, two levels down. Read here, in the board's own
//! process, on the machine the pane runs on (git, and the directory).

use std::path::Path;

use super::paint;

/// Changed files listed before `+N more`.
const FILES: usize = 15;
/// Commits listed.
const COMMITS: usize = 40;
/// Branches listed, the latest first.
const BRANCHES: usize = 8;
/// Lines of a directory's tree at most.
const TREE: usize = 200;

/// Folders whose insides are left out of a tree: what tools make again.
const MADE: &[&str] = &["node_modules", "target", ".git", ".venv", "venv", "__pycache__", "dist", "build"];

/// The tab's lines for a pane in `dir` running `command`, for a panel `width`
/// columns wide; `git` the program to ask (none: no git on this machine).
pub fn view(dir: &str, command: &str, git: Option<&Path>, width: usize) -> Vec<String> {
    let dim = |s: &str| paint("2", s);
    if let Some(host) = command.strip_prefix("ssh ") {
        return vec![dim(&format!("this pane is on {host}: its files and repository are there, not on this machine"))];
    }
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return vec![dim("no directory for this pane")];
    }
    match (crate::git::find(dir), git) {
        (Some(repo), Some(git)) => repo_view(&repo, git, width),
        (Some(repo), None) => {
            let mut out = vec![
                format!("{}  {}", paint("36", &repo.root.to_string_lossy()), paint("35", &repo.branch())),
                dim("no git on this machine (git-command, or git on the PATH): its branch only"),
            ];
            out.push(String::new());
            out.extend(tree(dir));
            out
        }
        (None, _) => {
            let mut out = vec![format!("{}  {}", paint("36", dir), dim("not in a git repository: its files"))];
            out.push(String::new());
            out.extend(tree(dir));
            out
        }
    }
}

/// A heading ruled to the panel's edge: `Commits ───`.
fn heading(title: &str, width: usize) -> String {
    let rule = width.saturating_sub(super::width_of(title) + 1);
    format!("{} {}", paint("1", title), paint("2", &"─".repeat(rule)))
}

fn repo_view(repo: &crate::git::Repo, git: &Path, width: usize) -> Vec<String> {
    let dim = |s: &str| paint("2", s);
    let run = |args: &[&str]| {
        crate::git::run(git, &repo.root, args, crate::git::LIMIT).map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let mut out = vec![paint("36", &repo.root.to_string_lossy())];
    let st = crate::git::status(git, repo).unwrap_or_default();
    // The branch, its upstream and how far apart they are.
    let upstream = run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut branch = format!("{} {}", dim("branch"), paint("1;35", &st.branch));
    if let Some(u) = &upstream {
        branch.push_str(&format!(" {} {}", dim("→"), u));
    }
    for (n, mark, colour) in [(st.ahead, "↑", "32"), (st.behind, "↓", "33")] {
        if n > 0 {
            branch.push_str(&format!("  {}", paint(colour, &format!("{mark}{n}"))));
        }
    }
    out.push(branch);
    // What is changed: the counts, then the files.
    let mut counts = Vec::new();
    for (n, what) in
        [(st.changed, "changed"), (st.staged, "staged"), (st.untracked, "new"), (st.conflicts, "in conflict")]
    {
        if n > 0 {
            counts.push(format!("{n} {what}"));
        }
    }
    let clean = counts.is_empty();
    out.push(format!("{} {}", dim("files "), if clean { paint("32", "clean") } else { counts.join(" · ") }));
    if !clean {
        let raw = run(&["status", "--porcelain=v1", "-z", "--untracked-files=normal"]).unwrap_or_default();
        let files = porcelain(&raw);
        for (state, path) in files.iter().take(FILES) {
            let colour = match state.trim() {
                "??" => "36",
                s if s.contains('U') => "31",
                s if s.starts_with(|c: char| c != ' ' && c != '?') => "32",
                _ => "33",
            };
            out.push(format!("  {} {path}", paint(colour, &format!("{state:>2}"))));
        }
        if files.len() > FILES {
            out.push(dim(&format!("  +{} more", files.len() - FILES)));
        }
    }
    // The latest commits: hash, when, who, what (and the names on it).
    out.push(String::new());
    out.push(heading("Commits", width));
    let log = run(&["log", &format!("-n{COMMITS}"), "--format=%h%x1f%ar%x1f%an%x1f%s%x1f%D"]).unwrap_or_default();
    let commits: Vec<&str> = log.lines().filter(|l| !l.is_empty()).collect();
    if commits.is_empty() {
        out.push(dim("  no commit yet"));
    }
    for line in commits {
        let f: Vec<&str> = line.split('\u{1f}').collect();
        let [hash, when, who, subject, names] = [0, 1, 2, 3, 4].map(|i| f.get(i).copied().unwrap_or_default());
        let names = if names.is_empty() { String::new() } else { format!(" {}", paint("33", &format!("({names})"))) };
        out.push(format!(
            "{} {} {} {subject}{names}",
            paint("33", hash),
            dim(&format!("{:>14}", short_when(when))),
            paint("36", &format!("{:<10}", short_name(who)))
        ));
    }
    // The other branches, the latest worked on first.
    let refs = run(&[
        "for-each-ref",
        "--sort=-committerdate",
        &format!("--count={BRANCHES}"),
        "--format=%(refname:short)%1f%(committerdate:relative)",
        "refs/heads",
    ])
    .unwrap_or_default();
    let branches: Vec<(String, String)> =
        refs.lines().filter_map(|l| l.split_once('\u{1f}')).map(|(b, w)| (b.to_string(), short_when(w))).collect();
    if branches.len() > 1 {
        out.push(String::new());
        out.push(heading("Branches", width));
        for (b, when) in branches {
            let mark = if b == st.branch { paint("1;35", "* ") } else { "  ".to_string() };
            out.push(format!("{mark}{b} {}", dim(&when)));
        }
    }
    out
}

/// `2 hours ago` as `2 hours`; what git says otherwise as it says it.
fn short_when(when: &str) -> String {
    when.strip_suffix(" ago").unwrap_or(when).to_string()
}

/// A name in ten columns: its first word, cut.
fn short_name(who: &str) -> String {
    let first = who.split_whitespace().next().unwrap_or(who);
    first.chars().take(10).collect()
}

/// `git status --porcelain=v1 -z`: each entry's two-letter state and path
/// (a rename's new one).
fn porcelain(out: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(e) = it.next() {
        if e.len() < 4 {
            continue;
        }
        let (state, path) = (e[..2].to_string(), e[3..].to_string());
        if state.starts_with(['R', 'C']) {
            it.next();
        }
        v.push((state, path));
    }
    v
}

/// The directory's files two levels down: folders first, then files, each
/// by name; inside what tools make again nothing is listed; at most `TREE`
/// lines.
fn tree(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk(Path::new(dir), "", 0, &mut out);
    if out.is_empty() {
        out.push(paint("2", "(empty)"));
    }
    if out.len() > TREE {
        let more = out.len() - TREE;
        out.truncate(TREE);
        out.push(paint("2", &format!("… {more} more")));
    }
    out
}

fn walk(dir: &Path, indent: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<(bool, String, u64, std::path::PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let t = e.file_type().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            let size = if t.is_file() { e.metadata().map(|m| m.len()).unwrap_or(0) } else { 0 };
            Some((t.is_dir(), name, size, e.path()))
        })
        .collect();
    // Folders first, then by name, any case.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let n = entries.len();
    for (i, (is_dir, name, size, path)) in entries.into_iter().enumerate() {
        if out.len() > TREE {
            return;
        }
        let last = i + 1 == n;
        let branch = if last { "└─ " } else { "├─ " };
        let hidden = name.starts_with('.');
        let shown = if is_dir {
            paint(if hidden { "2;34" } else { "1;34" }, &format!("{name}/"))
        } else if hidden {
            paint("2", &name)
        } else {
            name.clone()
        };
        let made = is_dir && MADE.contains(&name.as_str());
        let note = if made {
            paint("2", "  (not listed)")
        } else if !is_dir {
            paint("2", &format!("  {}", size_of(size)))
        } else {
            String::new()
        };
        out.push(format!("{}{}{shown}{note}", paint("2", indent), paint("2", branch)));
        if is_dir && !made && depth < 1 {
            let deeper = format!("{indent}{}", if last { "   " } else { "│  " });
            walk(&path, &deeper, depth + 1, out);
        }
    }
}

/// A file's size: `820B`, `12K`, `3.4M`.
fn size_of(b: u64) -> String {
    const K: u64 = 1024;
    match b {
        b if b < K => format!("{b}B"),
        b if b < K * K => format!("{}K", b.div_ceil(K)),
        b if b < K * K * K => format!("{:.1}M", b as f64 / (K * K) as f64),
        b => format!("{:.1}G", b as f64 / (K * K * K) as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("keepane-gitview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plain(lines: &[String]) -> Vec<String> {
        lines.iter().map(|l| super::super::strip(l)).collect()
    }

    /// Not a repository: the files two levels down, folders first, inside
    /// what tools make nothing; each file's size.
    #[test]
    fn a_folder_that_is_no_repository_shows_its_tree() {
        let d = temp("tree");
        std::fs::create_dir_all(d.join("src").join("deep").join("deeper")).unwrap();
        std::fs::create_dir_all(d.join("node_modules").join("x")).unwrap();
        std::fs::write(d.join("README.md"), "hello").unwrap();
        std::fs::write(d.join("src").join("main.rs"), vec![b'x'; 2000]).unwrap();
        std::fs::write(d.join(".env"), "a").unwrap();
        let got = plain(&view(&d.to_string_lossy(), "pwsh", crate::git::command("").as_deref(), 60));
        assert!(got[0].contains("not in a git repository"), "{got:#?}");
        let body: Vec<&str> = got[2..].iter().map(String::as_str).collect();
        assert_eq!(
            body,
            [
                "├─ node_modules/  (not listed)",
                "├─ src/",
                "│  ├─ deep/",
                "│  └─ main.rs  2K",
                "├─ .env  1B",
                "└─ README.md  5B",
            ],
            "two levels, folders first"
        );
        let empty = temp("empty");
        assert_eq!(plain(&view(&empty.to_string_lossy(), "pwsh", None, 60))[2], "(empty)");
        assert!(plain(&view("", "pwsh", None, 60))[0].contains("no directory"));
        assert!(
            plain(&view(&d.to_string_lossy(), "ssh box", None, 60))[0].contains("on box"),
            "an ssh pane's are there"
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A repository: its branch, what is changed (counts, then each file),
    /// the latest commits with the names on them, the branches.
    #[test]
    fn a_repository_shows_its_branch_changes_and_commits() {
        let Some(git) = crate::git::command("") else {
            eprintln!("no git here: skipped");
            return;
        };
        let d = temp("repo");
        let g = |args: &[&str]| {
            let ok = std::process::Command::new(&git)
                .arg("-C")
                .arg(&d)
                .args(["-c", "user.name=Tom Tester", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        g(&["init", "-q", "-b", "main"]);
        let view_now = || plain(&view(&d.to_string_lossy(), "pwsh", Some(&git), 60));
        assert!(view_now().iter().any(|l| l.contains("no commit yet")), "{:#?}", view_now());
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        g(&["add", "a.txt"]);
        g(&["commit", "-q", "-m", "first thing"]);
        std::fs::write(d.join("b.txt"), "two\n").unwrap();
        g(&["add", "b.txt"]);
        g(&["commit", "-q", "-m", "second thing"]);
        g(&["branch", "side"]);
        let clean = view_now();
        assert!(clean[1].contains("branch main") && clean[2].contains("clean"), "{clean:#?}");
        let rule = clean.iter().find(|l| l.starts_with("Commits")).unwrap();
        assert_eq!(super::super::width_of(rule), 60, "ruled to the panel's edge");
        let first_commit = clean.iter().position(|l| l.starts_with("Commits")).unwrap() + 1;
        assert!(clean[first_commit].contains("second thing") && clean[first_commit].contains("Tom"), "{clean:#?}");
        assert!(clean[first_commit].contains("(HEAD -> main, side)"), "the names on it: {clean:#?}");
        assert!(clean.iter().any(|l| l == "* main " || l.starts_with("* main")), "{clean:#?}");
        assert!(clean.iter().any(|l| l.starts_with("  side")), "{clean:#?}");
        // Changed and new files.
        std::fs::write(d.join("a.txt"), "one, changed\n").unwrap();
        std::fs::write(d.join("new.txt"), "new\n").unwrap();
        let dirty = view_now();
        assert!(dirty[2].contains("1 changed") && dirty[2].contains("1 new"), "{dirty:#?}");
        assert!(dirty.iter().any(|l| l.trim_start().starts_with("M a.txt")), "{dirty:#?}");
        assert!(dirty.iter().any(|l| l.trim_start().starts_with("?? new.txt")), "{dirty:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn small_pieces() {
        assert_eq!((short_when("2 hours ago"), short_when("now")), ("2 hours".into(), "now".into()));
        assert_eq!(short_name("Tom Tester"), "Tom");
        assert_eq!(short_name("averyveryverylongname"), "averyveryv");
        assert_eq!((size_of(5), size_of(2000), size_of(3 << 20)), ("5B".into(), "2K".into(), "3.0M".into()));
        let p = porcelain(" M a.rs\0?? b c\0R  new.rs\0old.rs\0");
        assert_eq!(p, [(" M".into(), "a.rs".into()), ("??".into(), "b c".into()), ("R ".into(), "new.rs".into())]);
    }
}
