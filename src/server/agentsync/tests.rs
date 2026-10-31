use super::*;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("keepane-agentsync-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const ID: &str = "9516c53b-ede9-4566-9d6a-76117964e69b";

#[test]
fn ids_are_found_in_names() {
    assert_eq!(uuid_in(&format!("{ID}.jsonl")), Some(ID));
    assert_eq!(uuid_in(&format!("rollout-2026-10-13T14-11-34-{ID}.jsonl")), Some(ID));
    assert_eq!(uuid_in("agent-a27464626a6f6f327.jsonl"), None);
    assert_eq!(uuid_in(""), None);
    let id = new_id();
    assert_eq!(uuid_in(&id), Some(id.as_str()), "a new id is one");
    assert_eq!(&id[14..15], "4", "version 4");
    assert_ne!(new_id(), id);
}

/// A path under the folder here, as the same place there: Windows to
/// Unix and back, any case on Windows, the separators of the other side;
/// not under it, nothing.
#[test]
fn paths_move_from_folder_to_folder() {
    let (win, nix) = (r"C:\Users\me\src\app", "/home/me/keepane/from/pc/app");
    assert_eq!(moved(win, win, nix).as_deref(), Some(nix));
    assert_eq!(moved(r"c:\users\ME\src\app\sub\x", win, nix).as_deref(), Some("/home/me/keepane/from/pc/app/sub/x"));
    assert_eq!(moved("/home/me/keepane/from/pc/app/a/b", nix, win).as_deref(), Some(r"C:\Users\me\src\app\a\b"));
    assert_eq!(moved(r"C:\Users\me\src\application", win, nix), None, "a longer name is not under it");
    assert_eq!(moved(r"D:\other", win, nix), None);
    assert_eq!(moved("/home/ME/keepane/from/pc/app", nix, win), None, "Unix: case counts");
    assert_eq!(moved(&format!("{win}\\"), &format!("{win}\\"), nix).as_deref(), Some(nix), "a trailing separator");
}

/// Only `cwd`: each one in a line, escaped quotes and backslashes kept
/// right; one not under the folder, and every other byte, as it was.
#[test]
fn only_cwd_is_rewritten() {
    let win = r"C:\Users\me\src\app";
    let to = "/home/u/app";
    let line = r#"{"type":"user","cwd":"C:\\Users\\me\\src\\app","message":{"content":"read C:\\Users\\me\\src\\app\\x.rs"},"sub":{"cwd":"C:\\Users\\me\\src\\app\\a \"q\""}}"#;
    let got = rewrite_line(line, win, to);
    assert_eq!(
        got,
        r#"{"type":"user","cwd":"/home/u/app","message":{"content":"read C:\\Users\\me\\src\\app\\x.rs"},"sub":{"cwd":"/home/u/app/a \"q\""}}"#
    );
    let other = r#"{"cwd":"D:\\elsewhere","x":1}"#;
    assert_eq!(rewrite_line(other, win, to), other);
    assert_eq!(rewrite_line(r#"{"cwd":"broken"#, win, to), r#"{"cwd":"broken"#, "no end: as it was");
    assert_eq!(rewrite_line("no key here", win, to), "no key here");
    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["sub"]["cwd"], "/home/u/app/a \"q\"", "still JSON");
}

/// A transcript as it is to be there: complete lines only, each `cwd`
/// moved; a tool's output as it is.
#[test]
fn a_transcript_goes_whole_lines_rewritten() {
    let d = temp("there");
    let p = d.join("t.jsonl");
    std::fs::write(&p, "{\"cwd\":\"/a/b\"}\n{\"cwd\":\"/a/b/c\"}\n{\"half\":").unwrap();
    let f =
        ChatFile { agent: Agent::Claude, id: ID.into(), rel: format!("{ID}.jsonl"), path: p.clone(), transcript: true };
    let got = String::from_utf8(as_there(&f, "/a/b", r"D:\x").unwrap()).unwrap();
    assert_eq!(got, "{\"cwd\":\"D:\\\\x\"}\n{\"cwd\":\"D:\\\\x\\\\c\"}\n");
    let raw = ChatFile { transcript: false, ..f };
    assert_eq!(as_there(&raw, "/a/b", r"D:\x").unwrap(), std::fs::read(&p).unwrap(), "not a transcript: as it is");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_chat_goes_whole_its_end_or_as_a_new_one() {
    let mine = b"line1\nline2\n";
    assert_eq!(how_to_send(mine, &Have::default()), Send::Whole);
    assert_eq!(how_to_send(mine, &Have { len: 6, sha: sha256(b"line1\n"), ..Have::default() }), Send::From(6));
    assert_eq!(how_to_send(mine, &Have { len: 12, sha: sha256(mine), ..Have::default() }), Send::Nothing);
    assert_eq!(
        how_to_send(mine, &Have { len: 6, sha: sha256(b"other\n"), ..Have::default() }),
        Send::Fork,
        "went on there"
    );
    assert_eq!(how_to_send(mine, &Have { len: 20, sha: "x".into(), ..Have::default() }), Send::Fork, "longer there");
}

/// Nothing of a file is lost: one changed here since the last send stays,
/// and what came goes beside it.
#[test]
fn a_file_changed_on_both_sides_goes_beside() {
    assert_eq!(decide(None, Some("b"), "n", true), Put::Write, "not here");
    assert_eq!(decide(Some("n"), Some("b"), "n", true), Put::Keep, "the same");
    assert_eq!(decide(Some("b"), Some("b"), "n", true), Put::Write, "unchanged here");
    assert_eq!(decide(Some("x"), Some("b"), "n", false), Put::Aside, "changed here");
    assert_eq!(decide(Some("x"), None, "n", true), Put::Aside, "no last send, a change of this side's");
    assert_eq!(decide(Some("x"), None, "n", false), Put::Write, "no last send, as the commit has it");
}

#[test]
fn only_paths_inside_the_folder_are_taken() {
    assert_eq!(safe_rel("src/main.rs"), Some(PathBuf::from("src").join("main.rs")));
    for bad in
        ["", "/etc/passwd", "../x", "a/../../x", "a//b", "C:/x", r"a\b", ".git/config", ".GIT/hooks/x", "a/b:c", "x."]
    {
        assert_eq!(safe_rel(bad), None, "{bad}");
    }
}

#[test]
fn a_pack_reads_back_part_by_part() {
    let d = temp("pack");
    let p = d.join("a.pack");
    let big = d.join("big");
    std::fs::write(&big, vec![7u8; 300_000]).unwrap();
    let mut w = PackWriter::create(&p).unwrap();
    w.add(&Entry { t: "manifest".into(), ..Entry::default() }, b"{}").unwrap();
    w.add_file(&Entry { t: "bundle".into(), ..Entry::default() }, &big).unwrap();
    w.add(&Entry { t: "file".into(), path: "a.txt".into(), ..Entry::default() }, b"hello").unwrap();
    let size = w.finish().unwrap();
    assert!(size < 10_000, "squeezed: {size}");
    let mut r = PackReader::open(&p).unwrap();
    assert_eq!(r.next().unwrap().map(|(e, n)| (e.t, n)), Some(("manifest".into(), 2)));
    assert_eq!(r.next().unwrap().map(|(e, n)| (e.t, n)), Some(("bundle".into(), 300_000)), "skipped unread");
    let (e, _) = r.next().unwrap().unwrap();
    assert_eq!((e.path.as_str(), r.bytes().unwrap()), ("a.txt", b"hello".to_vec()));
    assert_eq!(r.next().unwrap(), None);
    std::fs::write(&p, b"not gzip").unwrap();
    assert!(PackReader::open(&p).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ids_swap_and_lines_are_checked() {
    let other = "11111111-2222-4333-8444-555555555555";
    let b = format!("{{\"sessionId\":\"{ID}\"}}\n{{\"x\":\"{ID}\"}}\n");
    let s = swap_id(b.as_bytes(), ID, other);
    assert_eq!(String::from_utf8(s.clone()).unwrap(), b.replace(ID, other));
    assert_eq!(s.len(), b.len(), "the same length");
    assert!(lines_are_json(&s) && lines_are_json(b"\n\n{}\n"));
    assert!(!lines_are_json(b"{}\n{not json\n"));
    assert_eq!(dir_name("Tom's MacBook Pro.local"), "Tom-s-MacBook-Pro.local");
    assert_eq!(dir_name("../.."), "machine");
}

#[test]
fn status_paths_read_renames_right() {
    let out = b" M src/a.rs\0?? new file.txt\0R  b.rs\0old.rs\0 D gone.rs\0";
    assert_eq!(
        porcelain_paths(out),
        vec![
            (" M".into(), "src/a.rs".into()),
            ("??".into(), "new file.txt".into()),
            ("R ".into(), "b.rs".into()),
            (" D".into(), "gone.rs".into())
        ]
    );
}

/// Places for a test: everything under `d`.
fn places_in(d: &Path) -> Places {
    Places { claude: d.join("claude"), codex: d.join("codex"), pi: d.join("pi") }
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The chats of a folder: Claude Code's session with what is beside it,
/// Codex's by the folder its first line names, pi's.
#[test]
fn a_folders_chats_are_found() {
    let d = temp("chats");
    let pl = places_in(&d);
    let folder = if cfg!(windows) { r"C:\w\app" } else { "/w/app" };
    let cdir = pl.dir(Agent::Claude, folder);
    write(&cdir.join(format!("{ID}.jsonl")), "{}\n");
    write(&cdir.join(ID).join("subagents").join("agent-1.jsonl"), "{}\n");
    write(&cdir.join(ID).join("tool-results").join("x.txt"), "out");
    write(&cdir.join("not-a-session.jsonl"), "{}\n");
    let cx = "22222222-2222-4222-8222-222222222222";
    let meta = |cwd: &str| {
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{}}}}}\n", serde_json::to_string(cwd).unwrap())
    };
    write(&pl.codex.join("sessions/2026/10/13").join(format!("rollout-x-{cx}.jsonl")), &meta(folder));
    write(&pl.codex.join("sessions/2026/10/14/rollout-y-33333333-3333-4333-8333-333333333333.jsonl"), &meta("/else"));
    write(&pl.dir(Agent::Pi, folder).join("2026_44444444-4444-4444-8444-444444444444.jsonl"), "{}\n");
    let got: Vec<(Agent, String, bool)> =
        chats_of(folder, &pl).into_iter().map(|c| (c.agent, c.rel, c.transcript)).collect();
    assert_eq!(
        got,
        vec![
            (Agent::Claude, format!("{ID}.jsonl"), true),
            (Agent::Claude, format!("{ID}/subagents/agent-1.jsonl"), true),
            (Agent::Claude, format!("{ID}/tool-results/x.txt"), false),
            (Agent::Codex, format!("2026/10/13/rollout-x-{cx}.jsonl"), true),
            (Agent::Pi, "2026_44444444-4444-4444-8444-444444444444.jsonl".into(), true),
        ]
    );
    // Where one goes there, and by a fork's id when it took one.
    let ask = ChatAsk { agent: Some(Agent::Codex), id: cx.into(), rel: format!("2026/10/13/rollout-x-{cx}.jsonl") };
    let mut forks = BTreeMap::new();
    let (p, id) = chat_target(&pl, folder, &ask, &forks).unwrap();
    assert_eq!(
        (p.file_name().unwrap().to_string_lossy().into_owned(), id.as_str()),
        (format!("rollout-x-{cx}.jsonl"), cx)
    );
    forks.insert(cx.to_string(), ID.to_string());
    let (p, id) = chat_target(&pl, folder, &ask, &forks).unwrap();
    assert!(p.to_string_lossy().contains(ID) && id == ID, "{}", p.display());
    let deep = ChatAsk { rel: format!("x/{cx}.jsonl"), ..ask.clone() };
    assert_eq!(chat_target(&pl, folder, &deep, &BTreeMap::new()), None, "Codex: by its date");
    let out = ChatAsk { agent: Some(Agent::Claude), id: ID.into(), rel: format!("../{ID}.jsonl") };
    assert_eq!(chat_target(&pl, folder, &out, &BTreeMap::new()), None, "not out of its folder");
    let _ = std::fs::remove_dir_all(&d);
}

/// git for the tests, or None (skipped) where there is none.
fn git() -> Option<PathBuf> {
    crate::git::command("")
}

fn g(git: &Path, dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// What the receiving side answers a look with (as `link_agents` does).
fn look_at(dest: &Path, plan: &Plan, pl: &Places, record: &Record, git: &Path) -> Look {
    let folder = dest.to_string_lossy().into_owned();
    let (files, chats) = plan.asks();
    let need = files
        .iter()
        .enumerate()
        .filter(|(_, f)| sha256_file(&dest.join(safe_rel(&f.p).unwrap())).as_deref() != Some(f.s.as_str()))
        .map(|(i, _)| i)
        .collect();
    let chats = chats
        .iter()
        .map(|c| match chat_target(pl, &folder, c, &record.forks) {
            Some((p, id)) => have_of(&p, &id, &c.id),
            None => Have::default(),
        })
        .collect();
    Look { path: folder, known: false, git: crate::gitsync::look(git, dest, &plan.manifest.branch), need, chats }
}

/// One send, all of it, from `root` (and its chats in `here`) to `dest`
/// (its chats in `there`): what was said there, and its record.
fn send(root: &Path, here: &Places, dest: &Path, there: &Places, record: &Record, scratch: &Path) -> (Made, Applied) {
    let git = git().unwrap();
    let branch = g(&git, root, &["symbolic-ref", "--short", "HEAD"]);
    let head = g(&git, root, &["rev-parse", "HEAD"]);
    let plan = plan(Some(&git), root, Some((&branch, &head)), here, &[]).unwrap();
    let look = look_at(dest, &plan, there, record, &git);
    std::fs::create_dir_all(scratch).unwrap();
    let bundle = scratch.join("out.bundle");
    let theirs = if look.git.repo { look.git.tips.clone() } else { Vec::new() };
    let made_bundle = crate::gitsync::bundle(&git, root, &branch, &theirs, &bundle).unwrap();
    let b = matches!(made_bundle, crate::gitsync::Bundled::Made { .. }).then_some(bundle.as_path());
    let pack = scratch.join("out.pack");
    let made = make_pack(&plan, &look, b, &look.path, &pack).unwrap();
    let applied = apply(Apply {
        pack: pack.clone(),
        sha: sha256_file(&pack).unwrap(),
        dest: dest.to_path_buf(),
        scratch: scratch.join("in"),
        host: "laptop".into(),
        remote: "laptop".into(),
        url: "keepane:laptop:7681".into(),
        git: Some(git.clone()),
        places: there.clone(),
        there: plan.folder.clone(),
        record: record.clone(),
    })
    .unwrap();
    (made, applied)
}

/// A repository with work not committed and a chat, sent twice: the
/// first time it is cloned there with the changed and new files and the
/// chat (its `cwd` the folder there); then only what changed goes: the
/// chat's new end, a file changed again. A file changed there too is put
/// beside; a chat that went on there comes as a new one; a file put back
/// here goes back there.
#[test]
fn a_project_and_its_chats_go_and_go_again() {
    let Some(git) = git() else {
        eprintln!("no git here: skipped");
        return;
    };
    let d = temp("project");
    let root = d.join("app");
    std::fs::create_dir_all(&root).unwrap();
    g(&git, &root, &["init", "-q", "-b", "main"]);
    write(&root.join("a.txt"), "one\n");
    write(&root.join("b.txt"), "bee\n");
    g(&git, &root, &["add", "."]);
    g(&git, &root, &["commit", "-q", "-m", "first"]);
    write(&root.join("a.txt"), "one changed\n");
    write(&root.join("new.txt"), "new\n");
    write(&root.join("ignored.log"), "x\n");
    write(&root.join(".gitignore"), "*.log\n");
    let here = places_in(&d.join("here"));
    let there = places_in(&d.join("there"));
    let folder = root.to_string_lossy().into_owned();
    let chat = here.dir(Agent::Claude, &folder).join(format!("{ID}.jsonl"));
    let cwd = serde_json::to_string(&folder).unwrap();
    write(&chat, &format!("{{\"type\":\"user\",\"cwd\":{cwd},\"sessionId\":\"{ID}\"}}\n"));
    let dest = d.join("dest").join("app");
    let scratch = d.join("scratch");

    let (made, first) = send(&root, &here, &dest, &there, &Record::default(), &scratch);
    assert!(made.bundle && made.files == 3 && made.new == [(Agent::Claude, ID.to_string())], "{made:?}");
    assert!(first.text.contains("cloned") && first.text.contains("files: 3 written"), "{}", first.text);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one changed\n");
    assert_eq!(std::fs::read_to_string(dest.join("new.txt")).unwrap(), "new\n");
    assert!(!dest.join("ignored.log").exists(), "ignored: not sent");
    let dfolder = dest.to_string_lossy().into_owned();
    let landed = there.dir(Agent::Claude, &dfolder).join(format!("{ID}.jsonl"));
    let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&landed).unwrap().trim()).unwrap();
    assert_eq!(v["cwd"].as_str(), Some(dfolder.as_str()), "cwd moved");
    assert_eq!(first.record.base.len(), 3);

    // Again, nothing changed: nothing but the manifest goes.
    let (made, again) = send(&root, &here, &dest, &there, &first.record, &scratch);
    assert!(made.nothing(), "{made:?}");
    assert!(again.text.contains("0 written, 3 already here"), "{}", again.text);

    // A commit, more of the chat, a file changed again here, one changed
    // there, one put back here.
    g(&git, &root, &["add", "new.txt", ".gitignore"]);
    g(&git, &root, &["commit", "-q", "-m", "second"]);
    write(&root.join("a.txt"), "one changed twice\n");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&chat)
        .unwrap()
        .write_all(format!("{{\"cwd\":{cwd}}}\n").as_bytes())
        .unwrap();
    let (made, second) = send(&root, &here, &dest, &there, &again.record, &scratch);
    assert!(made.bundle && made.grown == [(Agent::Claude, ID.to_string())] && made.files == 1, "{made:?}");
    assert!(second.text.contains("fast-forwarded") && second.text.contains("chats: 0 new, 1 grown"), "{}", second.text);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "one changed twice\n");
    assert_eq!(std::fs::read_to_string(&landed).unwrap().lines().count(), 2, "the new end only, after the old");
    // Read with the settings it was checked out with (this machine's own
    // `core.autocrlf`), as a person there would.
    let status = crate::gitsync::git(&git, &dest, &["status", "--porcelain"], crate::git::LIMIT).unwrap();
    assert_eq!(status.trim(), "M a.txt", "only the change not committed here");

    // Changed on both sides: kept there, what came beside it. The chat
    // went on there too: it comes as a new chat, the old one kept.
    write(&dest.join("a.txt"), "theirs\n");
    std::fs::OpenOptions::new().append(true).open(&landed).unwrap().write_all(b"{\"there\":1}\n").unwrap();
    write(&root.join("a.txt"), "mine again\n");
    std::fs::OpenOptions::new().append(true).open(&chat).unwrap().write_all(b"{\"here\":1}\n").unwrap();
    let (made, third) = send(&root, &here, &dest, &there, &second.record, &scratch);
    assert_eq!(made.forked, [(Agent::Claude, ID.to_string())], "{made:?}");
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "theirs\n", "kept");
    assert_eq!(std::fs::read_to_string(dest.join("a.txt.from-laptop")).unwrap(), "mine again\n", "beside");
    assert!(third.text.contains("changed on both sides") && third.text.contains("went on here too"), "{}", third.text);
    let fork = third.record.forks.get(ID).expect("a fork kept").clone();
    assert!(third.text.contains(&format!("new chat: claude --resume {fork}")), "its id said: {}", third.text);
    let forked = there.dir(Agent::Claude, &dfolder).join(format!("{fork}.jsonl"));
    let text = std::fs::read_to_string(&forked).unwrap();
    assert!(text.contains(&fork) && !text.contains(ID) && text.contains("\"here\":1"), "{text}");
    assert!(std::fs::read_to_string(&landed).unwrap().contains("\"there\":1"), "theirs untouched");

    // More of the chat here: it grows the new chat there, said by its id there.
    std::fs::OpenOptions::new().append(true).open(&chat).unwrap().write_all(b"{\"here\":2}\n").unwrap();
    let (made, _) = send(&root, &here, &dest, &there, &third.record, &scratch);
    assert_eq!((made.grown.as_slice(), made.forked.len()), ([(Agent::Claude, fork.clone())].as_slice(), 0), "{made:?}");
    assert!(std::fs::read_to_string(&forked).unwrap().contains("\"here\":2"));

    // Put back here (as committed): unchanged there since, it goes back too.
    write(&dest.join("a.txt"), "mine again\n");
    let fourth_base = Record { base: [("a.txt".to_string(), sha256(b"mine again\n"))].into(), ..third.record.clone() };
    g(&git, &root, &["checkout", "-q", "--", "a.txt"]);
    let (_, fourth) = send(&root, &here, &dest, &there, &fourth_base, &scratch);
    assert!(fourth.text.contains("1 put back as there"), "{}", fourth.text);
    let back = std::fs::read_to_string(dest.join("a.txt")).unwrap();
    assert_eq!(back.replace("\r\n", "\n"), "one\n", "as the commit has it (in this machine's line ends)");
    let _ = std::fs::remove_dir_all(&d);
}

/// A folder that is no repository: its files, but not what tools make.
#[test]
fn a_plain_folder_goes_without_what_tools_make() {
    let d = temp("plain");
    write(&d.join("p/notes.md"), "n\n");
    write(&d.join("p/sub/x.txt"), "x\n");
    write(&d.join("p/node_modules/m/i.js"), "m\n");
    write(&d.join("p/target/debug/b"), "b\n");
    let got: Vec<String> = files_to_send(None, &d.join("p"), false).unwrap().into_iter().map(|(r, _)| r).collect();
    assert_eq!(got, ["notes.md", "sub/x.txt"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn records_are_kept_one_per_folder_and_machine() {
    let d = temp("records");
    let r = Record { here: "/a".into(), key: "k".into(), there: "/b".into(), ..Record::default() };
    save_record(&d, r.clone()).unwrap();
    save_record(&d, Record { there: "/c".into(), ..r.clone() }).unwrap();
    save_record(&d, Record { key: "k2".into(), ..r.clone() }).unwrap();
    let all = load_records(&d);
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|x| x.key == "k" && x.there == "/c"), "the last one wins");
    let _ = std::fs::remove_dir_all(&d);
}
