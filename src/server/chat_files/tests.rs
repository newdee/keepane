use super::*;
use crate::server::chat::Ident;

fn server(dir: &Path) -> Server {
    let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
    let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
    let mut s = Server::new(pane_tx, "chat-files-test".into(), events);
    s.chat.dir = Some(dir.to_path_buf());
    s.chat_start();
    s
}

fn text(o: Outcome) -> String {
    match o {
        Outcome::Text(t) => t,
        Outcome::Error(e) => format!("error: {e}"),
        _ => String::new(),
    }
}

/// A fresh chat folder and a folder of files to send, the test's own.
fn folders(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("keepane-group-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let send = root.join("send");
    std::fs::create_dir_all(&send).unwrap();
    std::fs::write(send.join("a.txt"), "first").unwrap();
    std::fs::create_dir_all(send.join("two")).unwrap();
    std::fs::write(send.join("two").join("a.txt"), "second!").unwrap();
    std::fs::write(send.join("pic.png"), "png").unwrap();
    (root.join("chat"), send)
}

fn say(s: &mut Server, files: &[PathBuf]) -> String {
    let files = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    text(s.chat_say(None, &Caller::You, "general", "here".into(), files, None))
}

/// Files sent are the channel's group files: listed the newest first with
/// their sender, line and path (`-J` the same, `path`); one name sent twice
/// is told apart by its `day/line/name`; only its sender (the person here)
/// deletes one; deleted, it is gone from the list and the folder, and the
/// line it came with says so.
#[test]
fn group_files_are_listed_told_apart_and_deleted_by_their_sender() {
    let (dir, send) = folders("list");
    let mut s = server(&dir);
    assert!(say(&mut s, &[send.join("a.txt"), send.join("pic.png")]).starts_with("#general #"));
    assert!(say(&mut s, &[send.join("two").join("a.txt")]).starts_with("#general #"));
    let list = s.group_files(0);
    assert_eq!(list.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["a.txt", "pic.png", "a.txt"]);
    assert!(list.iter().all(|g| g.from.starts_with("you") && g.pane.is_none() && g.addr.is_empty()), "{list:?}");
    assert!(dir.join(&s.chat.channels[0].id).join(INDEX).is_file(), "kept in its index");
    // Listed: the newest first, each with where it is.
    let shown = text(s.chat_group_files(&Caller::You, 0, false));
    let rows: Vec<&str> = shown.lines().collect();
    assert!(
        rows[0].contains("a.txt  7B  you") && rows[2].contains("pic.png") && rows[4].contains("a.txt  5B"),
        "{shown}"
    );
    let newest = &list[2];
    let path = copy_path(&s.chat_files_dir(&s.chat.channels[0].id).unwrap(), &newest.file).unwrap();
    assert_eq!(rows[1].trim(), path.to_string_lossy(), "{shown}");
    let json: serde_json::Value = serde_json::from_str(&text(s.chat_group_files(&Caller::You, 0, true))).unwrap();
    assert_eq!(json["channel"]["name"], "general");
    assert_eq!(json["group_files"][0]["path"].as_str(), Some(path.to_string_lossy().as_ref()));
    assert_eq!(json["group_files"][0]["seq"].as_u64(), Some(newest.seq));
    // One name twice: its `day/line/name`, then.
    let two = text(s.chat_group_file_rm(&Caller::You, 0, "a.txt"));
    assert!(two.contains("more than one is called a.txt") && two.contains(&newest.file), "{two}");
    // Not its sender: refused (another machine's pane, the person there).
    let there =
        Caller::There { key: "k".into(), ident: Ident { pane: 3, addr: "10.0.0.2:7681".into(), ..Default::default() } };
    assert!(text(s.chat_group_file_rm(&there, 0, "pic.png")).contains("or the person deletes pic.png"));
    let far_person = Caller::YouThere { machine: "mac".into() };
    assert!(text(s.chat_group_file_rm(&far_person, 0, "pic.png")).starts_with("error:"));
    // Its sender: deleted, the folder of its line too (nothing else in it).
    let gone = text(s.chat_group_file_rm(&Caller::You, 0, &newest.file));
    assert!(gone.contains("deleted group file a.txt"), "{gone}");
    assert!(!path.exists() && !path.parent().unwrap().exists(), "its file and its line's folder");
    assert_eq!(s.group_files(0).len(), 2);
    // Its line still names it, and says it is deleted.
    let read = text(s.chat_read(None, &Caller::You, "general", None, true, false, None, false));
    assert!(read.contains("a.txt: deleted from the group files"), "{read}");
    assert!(text(s.chat_group_file_rm(&Caller::You, 0, "nothing.bin")).contains("has no group file nothing.bin"));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// Kept until deleted: an old day's files stay when the chat prunes its
/// lines by `chat-days`.
#[test]
fn group_files_outlive_chat_days() {
    let (dir, send) = folders("days");
    let mut s = server(&dir);
    say(&mut s, &[send.join("a.txt")]);
    let old = s.chat_files_dir(&s.chat.channels[0].id).unwrap().join("2000-01-01").join("1");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("old.txt"), "old").unwrap();
    s.opts.chat_days = 1;
    s.chat_tick();
    assert!(old.join("old.txt").exists(), "an old day's file stays");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// A channel from before group files (no index): found in its folder, the
/// sender from its line while it is kept, none after; an index that does
/// not read is kept aside (`.bad`) and found again the same way.
#[test]
fn a_channel_without_an_index_finds_its_group_files_in_its_folder() {
    let (dir, send) = folders("index");
    let mut s = server(&dir);
    say(&mut s, &[send.join("a.txt")]);
    let index = dir.join(&s.chat.channels[0].id).join(INDEX);
    std::fs::remove_file(&index).unwrap();
    let orphan = s.chat_files_dir(&s.chat.channels[0].id).unwrap().join("2000-01-01").join("1");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("lost.txt"), "x").unwrap();
    let found = s.group_files(0);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!((found[0].name.as_str(), found[0].from.as_str(), found[0].at.as_str()), ("lost.txt", "", "2000-01-01"));
    assert!(found[1].name == "a.txt" && found[1].from.starts_with("you"), "its line still kept: {found:?}");
    std::fs::write(&index, "{ not a list").unwrap();
    assert_eq!(s.group_files(0).len(), 2, "found again");
    assert_eq!(std::fs::read_to_string(index.with_extension("json.bad")).unwrap(), "{ not a list");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// Who may delete a group file: the person here; the pane that sent it
/// (here, or the same pane on its machine); no other pane, nor the person
/// at another machine.
#[test]
fn only_its_sender_or_the_person_deletes_a_group_file() {
    let here = GroupFile::of("d/1/a".into(), "a".into(), 1, 1, String::new(), None);
    let mine = GroupFile { pane: Some(5), ..here.clone() };
    let far = GroupFile { pane: Some(5), addr: "10.0.0.2:7681".into(), ..here.clone() };
    let pane = |p| Caller::Pane(p);
    let there =
        |p, a: &str| Caller::There { key: "k".into(), ident: Ident { pane: p, addr: a.into(), ..Default::default() } };
    assert!(here.may_delete(&Caller::You) && mine.may_delete(&Caller::You) && far.may_delete(&Caller::You));
    assert!(mine.may_delete(&pane(5)) && !mine.may_delete(&pane(6)) && !here.may_delete(&pane(5)));
    assert!(far.may_delete(&there(5, "10.0.0.2:7681")) && !far.may_delete(&there(5, "10.0.0.3:7681")));
    assert!(!far.may_delete(&pane(5)), "a pane here with that number is another pane");
    assert!(!mine.may_delete(&Caller::YouThere { machine: "mac".into() }));
}
