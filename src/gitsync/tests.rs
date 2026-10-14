use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("keepane-gitsync-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// git as a person runs it in `dir`, with a name to commit under.
fn g(git_cmd: &Path, dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new(git_cmd)
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(git_cmd: &Path, dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    g(git_cmd, dir, &["add", file]);
    g(git_cmd, dir, &["commit", "-q", "-m", file]);
}

#[test]
fn names_branches_and_paths() {
    assert_eq!(remote_name("laptop"), "laptop");
    assert_eq!(remote_name("My PC (2)"), "My-PC--2");
    assert_eq!(remote_name("..x..lock"), "x-lock");
    assert_eq!(remote_name("a.lock"), "a");
    assert_eq!(remote_name("中文"), "remote");
    assert_eq!(remote_name(""), "remote");
    for ok in ["main", "feature/x", "v1.2", "a-b_c"] {
        assert!(branch_ok(ok), "{ok}");
    }
    for bad in ["", "-x", "/x", "x/", "a..b", "a b", "x.lock", "a/.b", "a@{1}", "@", "a:b", "a\\b", "a~1", ".x"] {
        assert!(!branch_ok(bad), "{bad:?}");
    }
    let (dd, home) = (PathBuf::from("/dd"), PathBuf::from("/home/u"));
    let r = |a: &str, n: &str| resolve_path(a, n, &dd, &home);
    assert_eq!(r("", "proj"), Some(dd.join("proj")));
    assert_eq!(r("", ".."), None);
    assert_eq!(r("", "a/b"), None);
    assert_eq!(r("", ""), None);
    assert_eq!(r("~", "x"), Some(home.clone()));
    assert_eq!(r("~/src/p", "x"), Some(home.join("src").join("p")));
    assert_eq!(r("sub/p", "x"), Some(dd.join("sub").join("p")));
    // `.` and `..` worked out: what is checked and shown is where it goes.
    assert_eq!(r("../up/./p", "x"), Some(PathBuf::from("/up/p")));
    assert_eq!(r("~/a/../b", "x"), Some(home.join("b")));
    let abs = if cfg!(windows) { r"C:\code\p" } else { "/code/p" };
    assert_eq!(r(abs, "x"), Some(PathBuf::from(abs)));
}

/// Clone, then each way a push ends: up to date, fast-forwarded (files
/// too), kept (their own commit; a changed file), moved (not checked out),
/// made (a new branch); a look at each place.
#[test]
fn a_bundle_clones_and_pushes() {
    let Some(git_cmd) = crate::git::command("") else {
        eprintln!("no git here: skipped");
        return;
    };
    let dir = scratch("flow");
    let here = dir.join("here");
    std::fs::create_dir_all(&here).unwrap();
    g(&git_cmd, &here, &["init", "-q", "-b", "main"]);
    commit(&git_cmd, &here, "a.txt", "one\n");
    let there = dir.join("there").join("proj");
    // A look at a place with nothing yet: free, no repository.
    let l = look(&git_cmd, &there, "main");
    assert!(l.free && !l.repo, "{l:?}");
    let b = dir.join("1.bundle");
    assert_eq!(
        bundle(&git_cmd, &here, "main", &[], &b).unwrap(),
        Bundled::Made { commits: 1, size: std::fs::metadata(&b).unwrap().len() }
    );
    let out = clone(&git_cmd, &b, &there, "main", "laptop", "keepane:laptop:8765").unwrap();
    assert!(out.ends_with("cloned, main checked out (1 commits)"), "{out}");
    assert_eq!(std::fs::read_to_string(there.join("a.txt")).unwrap().replace('\r', ""), "one\n");
    assert_eq!(g(&git_cmd, &there, &["remote"]), "laptop");
    assert_eq!(g(&git_cmd, &there, &["remote", "get-url", "laptop"]), "keepane:laptop:8765");
    let l = look(&git_cmd, &there, "main");
    assert!(l.repo && !l.free && l.clean && l.current.as_deref() == Some("main"), "{l:?}");
    assert_eq!(l.roots, roots(&git_cmd, &here, "main"));
    assert_eq!(l.tip.as_deref(), Some(g(&git_cmd, &here, &["rev-parse", "main"]).as_str()));
    // Up to date: nothing to send.
    assert_eq!(bundle(&git_cmd, &here, "main", &l.tips, &dir.join("x.bundle")).unwrap(), Bundled::Nothing);
    // Two new commits: only those go, and the branch there moves, files too.
    commit(&git_cmd, &here, "b.txt", "two\n");
    commit(&git_cmd, &here, "c.txt", "three\n");
    let b2 = dir.join("2.bundle");
    assert!(matches!(bundle(&git_cmd, &here, "main", &l.tips, &b2).unwrap(), Bundled::Made { commits: 2, .. }));
    assert_eq!(take(&git_cmd, &b2, &there, "main", "laptop").unwrap(), (Landed::FastForwarded, 2));
    assert!(there.join("c.txt").is_file());
    // The same bundle again: already there.
    assert_eq!(take(&git_cmd, &b2, &there, "main", "laptop").unwrap(), (Landed::Already, 0));
    // A tracked file changed there: kept beside, the branch and the file as they were.
    commit(&git_cmd, &here, "d.txt", "four\n");
    std::fs::write(there.join("a.txt"), "edited there\n").unwrap();
    let tips = look(&git_cmd, &there, "main").tips;
    let b3 = dir.join("3.bundle");
    bundle(&git_cmd, &here, "main", &tips, &b3).unwrap();
    let (landed, came) = take(&git_cmd, &b3, &there, "main", "laptop").unwrap();
    assert!(matches!(&landed, Landed::Kept(w) if w.contains("files are changed there")), "{landed:?}");
    assert_eq!(came, 1);
    assert!(!there.join("d.txt").exists());
    assert_eq!(g(&git_cmd, &there, &["rev-parse", "laptop/main"]), g(&git_cmd, &here, &["rev-parse", "main"]));
    // A new file there is no change to a tracked one: it moves on.
    g(&git_cmd, &there, &["checkout", "-q", "--", "a.txt"]);
    std::fs::write(there.join("untracked.txt"), "mine\n").unwrap();
    assert_eq!(take(&git_cmd, &b3, &there, "main", "laptop").unwrap(), (Landed::FastForwarded, 1));
    // A commit of their own there: kept beside, theirs untouched.
    commit(&git_cmd, &there, "theirs.txt", "x\n");
    commit(&git_cmd, &here, "e.txt", "five\n");
    let tips = look(&git_cmd, &there, "main").tips;
    let b4 = dir.join("4.bundle");
    bundle(&git_cmd, &here, "main", &tips, &b4).unwrap();
    let mine = g(&git_cmd, &there, &["rev-parse", "main"]);
    let (landed, _) = take(&git_cmd, &b4, &there, "main", "laptop").unwrap();
    assert!(matches!(&landed, Landed::Kept(w) if w.contains("1 commit(s) of its own")), "{landed:?}");
    assert_eq!(g(&git_cmd, &there, &["rev-parse", "main"]), mine);
    // Another branch: made there, then moved on while not checked out.
    g(&git_cmd, &here, &["checkout", "-q", "-b", "side"]);
    commit(&git_cmd, &here, "s.txt", "s\n");
    let tips = look(&git_cmd, &there, "side").tips;
    let b5 = dir.join("5.bundle");
    bundle(&git_cmd, &here, "side", &tips, &b5).unwrap();
    assert_eq!(take(&git_cmd, &b5, &there, "side", "laptop").unwrap().0, Landed::Made);
    commit(&git_cmd, &here, "s2.txt", "s2\n");
    let tips = look(&git_cmd, &there, "side").tips;
    let b6 = dir.join("6.bundle");
    bundle(&git_cmd, &here, "side", &tips, &b6).unwrap();
    assert_eq!(take(&git_cmd, &b6, &there, "side", "laptop").unwrap(), (Landed::Moved, 1));
    assert_eq!(g(&git_cmd, &there, &["rev-parse", "side"]), g(&git_cmd, &here, &["rev-parse", "side"]));
    assert_eq!(g(&git_cmd, &there, &["symbolic-ref", "--short", "HEAD"]), "main");
    // A shallow clone: refused, its early history is not here to send.
    let shallow = dir.join("shallow");
    let url = format!("file:///{}", here.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
    g(&git_cmd, &dir, &["clone", "-q", "--depth", "1", &url, &shallow.to_string_lossy()]);
    let e =
        bundle(&git_cmd, &shallow, &g(&git_cmd, &shallow, &["branch", "--show-current"]), &[], &dir.join("s.bundle"))
            .unwrap_err();
    assert!(e.contains("shallow clone"), "{e}");
    // A branch that is not here; a place inside a repository is no repository.
    assert!(bundle(&git_cmd, &here, "nope", &[], &dir.join("n.bundle")).unwrap_err().contains("no branch nope"));
    std::fs::create_dir_all(there.join("sub")).unwrap();
    let inner = look(&git_cmd, &there.join("sub"), "main");
    assert!(!inner.repo && inner.free, "{inner:?}");
    // An empty repository there (no commit yet): the branch is made and checked out.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    g(&git_cmd, &empty, &["init", "-q", "-b", "main"]);
    let l = look(&git_cmd, &empty, "main");
    assert!(l.repo && l.roots.is_empty() && l.tip.is_none(), "{l:?}");
    let b7 = dir.join("7.bundle");
    g(&git_cmd, &here, &["checkout", "-q", "main"]);
    bundle(&git_cmd, &here, "main", &[], &b7).unwrap();
    let (landed, came) = take(&git_cmd, &b7, &empty, "main", "laptop").unwrap();
    assert_eq!((landed, came), (Landed::FastForwarded, 5));
    assert!(empty.join("e.txt").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_a_push_says() {
    assert_eq!(
        said("/p", "main", "laptop", &Landed::FastForwarded, 1),
        "/p: main 1 commit, fast-forwarded (files too)"
    );
    assert_eq!(said("/p", "main", "laptop", &Landed::Already, 0), "/p: main already up to date");
    assert_eq!(said("/p", "main", "laptop", &Landed::Kept("why".into()), 3), "/p: 3 commits kept in laptop/main; why");
}
