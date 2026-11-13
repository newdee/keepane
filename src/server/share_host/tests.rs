use super::*;
use crate::web::Options;
use std::net::IpAddr;

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn server() -> Server {
    let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
    let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
    let mut s = Server::new(pane_tx, "t".into(), events);
    // Its list kept in a folder of the test's own, never the real one.
    let dir = std::env::temp_dir().join(format!(
        "keepane-share-host-sessions-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    s.opts.sessions_dir = dir.to_string_lossy().into_owned();
    // Served here, on a port of its own: never this machine's 7681.
    let here = Options { port: Some(0), bind: vec![IpAddr::from([127, 0, 0, 1])], ..Options::default() };
    assert!(matches!(s.web_serve(here), Outcome::Text(_)));
    s
}

fn text(o: Outcome) -> String {
    match o {
        Outcome::Text(t) => t,
        Outcome::Error(e) => format!("error: {e}"),
        Outcome::Ok => "ok".into(),
        _ => "other".into(),
    }
}

fn add(s: &mut Server, path: &std::path::Path, for_secs: Option<u64>, once: bool) -> (String, String) {
    let out = text(s.exec_share(ShareWhat::Add { path: path.to_string_lossy().into(), for_secs, once }, None));
    let token = out.split("/s/").nth(1).map(|t| t[..22].to_string()).unwrap_or_default();
    (out, token)
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("keepane-share-host-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("folder")).unwrap();
    std::fs::write(d.join("report.pdf"), vec![7u8; 2048]).unwrap();
    d
}

/// Shared: a link of its own and a code to scan, said on the status line,
/// listed; opened, it says what to send (each address once); stopped, gone.
#[tokio::test]
async fn a_share_is_a_link_until_stopped() {
    let d = temp("add");
    let mut s = server();
    let (out, token) = add(&mut s, &d.join("report.pdf"), None, false);
    assert!(crate::share::valid_token(&token), "{out}");
    // The real path (a runner's temp directory may be a short or linked one).
    let real = crate::git::real(&d.join("report.pdf"));
    assert!(out.contains(&format!("share 1: {} (file, 2 KB), until ", real.display())), "{out}");
    assert!(out.contains("(1d)") && out.contains("  http://127.0.0.1:") && out.contains("█"), "{out}");
    assert!(!out.contains("was off"), "the page was on: {out}");
    assert!(
        s.messages
            .iter()
            .any(|m| m.ends_with("share 1: file report.pdf, ") || m.contains("share 1: file report.pdf, until"))
    );
    // A folder: as a zip; numbers go on.
    let (out, folder) = add(&mut s, &d.join("folder"), Some(0), true);
    assert!(
        out.contains("share 2: ") && out.contains("(folder), until stopped, once") && out.contains("as a zip"),
        "{out}"
    );
    assert_ne!(token, folder);
    let listed = text(s.exec_share(ShareWhat::List { json: false }, None));
    assert!(
        listed.starts_with("1  file  until ") && listed.contains("\n2  folder  until stopped, once, sent 0  "),
        "{listed}"
    );
    assert!(listed.contains(&format!("/s/{token}")) && listed.contains(&format!("/s/{folder}")), "{listed}");

    let opened = text(s.share_open(&token, "192.168.1.20"));
    let v: serde_json::Value = serde_json::from_str(&opened).unwrap();
    assert_eq!((v["dir"].as_bool(), v["name"].as_str()), (Some(false), Some("report.pdf")));
    assert_eq!(std::path::PathBuf::from(v["path"].as_str().unwrap()), crate::git::real(&d.join("report.pdf")));
    text(s.share_open(&token, "192.168.1.20"));
    let said = |s: &Server, what: &str| s.messages.iter().filter(|m| m.ends_with(what)).count();
    assert_eq!(said(&s, "share 1: 192.168.1.20 is downloading report.pdf"), 1, "said once an address");
    assert_eq!(text(s.share_open("aaaaaaaaaaaaaaaaaaaaaa", "x")), format!("error: {}", crate::share::ENDED));

    assert_eq!(text(s.exec_share(ShareWhat::Stop(Some(1)), None)), "share 1 stopped: its link no longer works");
    assert!(text(s.share_open(&token, "192.168.1.20")).starts_with("error: "));
    assert!(text(s.exec_share(ShareWhat::Stop(Some(1)), None)).contains("no share 1"));
    assert!(text(s.exec_share(ShareWhat::Stop(None), None)).starts_with("share 2 stopped: its link"));
    assert_eq!(text(s.exec_share(ShareWhat::Stop(None), None)), "nothing was shared");
    assert!(text(s.exec_share(ShareWhat::List { json: false }, None)).starts_with("nothing is shared"));
    let _ = std::fs::remove_dir_all(&d);
}

/// Sent whole: counted and said; `--once`, ended by it. Its time up: ended
/// at the next tick, and said.
#[tokio::test]
async fn once_and_time_end_a_share() {
    let d = temp("end");
    let mut s = server();
    let (_, once) = add(&mut s, &d.join("report.pdf"), None, true);
    let (_, timed) = add(&mut s, &d.join("folder"), Some(60), false);
    assert_eq!(text(s.share_sent(&timed, "10.0.0.2", "3 file(s)")), "ok");
    assert!(s.messages.iter().any(|m| m.ends_with("share 2: folder sent to 10.0.0.2 (3 file(s))")));
    assert!(text(s.exec_share(ShareWhat::List { json: false }, None)).contains("sent 1"));
    text(s.share_sent(&once, "10.0.0.2", ""));
    assert!(s.messages.iter().any(|m| m.ends_with("share 1: report.pdf sent to 10.0.0.2; it was for once: ended")));
    assert!(text(s.share_open(&once, "10.0.0.2")).starts_with("error: "));
    // Sent after it was stopped: nothing to count, no error.
    assert_eq!(text(s.share_sent(&once, "10.0.0.2", "")), "ok");

    s.shares_tick();
    assert_eq!(s.shares.list.len(), 1, "not yet");
    s.shares.list[0].until = Some(now_ms() - 1000);
    // Opened at the very moment it ends: ended.
    assert!(text(s.share_open(&timed, "10.0.0.2")).starts_with("error: "));
    assert!(s.messages.iter().any(|m| m.ends_with("share 2 ended: folder (its time was up)")));
    assert!(s.shares.list.is_empty());
    // Both kept as ended, the latest first, each with how it ended.
    let states: Vec<_> = s.shares.ended.iter().map(|x| (x.id, x.state())).collect();
    assert_eq!(states, [(2, "expired"), (1, "used")]);
    let _ = std::fs::remove_dir_all(&d);
}

/// The page off: shared, it is on, as it was last served; not kept for a
/// restore by that.
#[tokio::test]
async fn sharing_turns_the_page_on() {
    let d = temp("on");
    let mut s = server();
    let o = Options { read_only: true, ..s.web.as_ref().unwrap().options().clone() };
    text(s.web_stop(None));
    assert!(s.web.is_none());
    // How it was served before this server (a `web-start` not stopped).
    crate::web::keep_on(&s.web_on, &o).unwrap();
    let (out, _) = add(&mut s, &d.join("report.pdf"), None, false);
    assert!(out.contains("The phone page was off: it serves now"), "{out}");
    assert!(out.contains("  http://127.0.0.1:"), "{out}");
    assert_eq!(s.web.as_ref().unwrap().options(), &o, "as it was last served");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn what_cannot_be_shared_is_said() {
    let d = temp("bad");
    let mut s = server();
    let (out, _) = add(&mut s, &d.join("not-there.txt"), None, false);
    assert!(out.starts_with("error: share: ") && out.contains("not-there.txt"), "{out}");
    assert!(s.shares.list.is_empty() && s.shares.last == 0);
    let _ = std::fs::remove_dir_all(&d);
}

/// A Unix file name that is not text: refused, said why (the page is told
/// paths as JSON text).
#[cfg(unix)]
#[tokio::test]
async fn a_path_that_is_not_text_is_refused() {
    use std::os::unix::ffi::OsStrExt;
    let d = temp("bytes");
    let odd = d.join(std::ffi::OsStr::from_bytes(b"odd\xff.txt"));
    // macOS (APFS) takes no such name: there the case cannot come up.
    if std::fs::write(&odd, "x").is_err() {
        let _ = std::fs::remove_dir_all(&d);
        return;
    }
    // A path comes as text; a link is how its real path is not.
    std::os::unix::fs::symlink(&odd, d.join("link.txt")).unwrap();
    let mut s = server();
    let (out, _) = add(&mut s, &d.join("link.txt"), None, false);
    assert!(out.starts_with("error: share: ") && out.contains("not UTF-8"), "{out}");
    assert!(s.shares.list.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

/// A relative path is the client's (its working directory); `~` is home.
#[tokio::test]
async fn paths_are_where_the_command_was_run() {
    let mut s = server();
    let (out, _) = add(&mut s, std::path::Path::new("~"), None, false);
    let home = crate::git::real(&dirs::home_dir().unwrap());
    assert!(out.contains(&format!("share 1: {} (folder)", home.display())), "{out}");
}

/// A kept list keepane cannot read (damaged, edited by hand): nothing is
/// back, it is said, and the file is kept aside rather than written over
/// by the next share. No list at all: nothing back, nothing said.
#[tokio::test]
async fn a_damaged_list_is_kept_aside_not_written_over() {
    let d = temp("damaged");
    let mut s = server();
    let before = s.messages.len();
    assert_eq!(s.shares_load(), 0, "no list yet");
    assert_eq!(s.messages.len(), before, "{:?}", s.messages);
    let file = s.shares_file();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ not a list").unwrap();
    assert_eq!(s.shares_load(), 0);
    let bad = file.with_extension("json.bad");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ not a list", "kept as it was");
    assert!(s.messages.iter().any(|m| m.contains("is not a list keepane reads") && m.contains("shares.json.bad")));
    add(&mut s, &d.join("report.pdf"), None, false);
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ not a list", "not written over");
    assert!(file.is_file(), "a new list");
}

/// Kept like the saved sessions: a server that starts brings the shares
/// back on the same links; one whose time was up meanwhile has ended, and
/// comes back on its link only when extended; a stopped one is shared
/// again only on a new link. Every state is in `share -l -J`.
#[tokio::test]
async fn shares_are_kept_and_come_back_on_their_links() {
    let d = temp("kept");
    let mut s = server();
    let dir = s.opts.sessions_dir.clone();
    let (_, kept) = add(&mut s, &d.join("report.pdf"), None, false);
    let (_, timed) = add(&mut s, &d.join("folder"), Some(60), false);
    let (_, stopped) = add(&mut s, &d.join("report.pdf"), Some(0), true);
    text(s.exec_share(ShareWhat::Stop(Some(3)), None));
    assert!(s.shares_file().starts_with(&dir) && s.shares_file().is_file());
    // In a folder of its own: never read as a saved session.
    assert!(crate::resurrect::list(std::path::Path::new(&dir)).is_empty());
    drop(s);
    // Its time up while no server ran.
    let file = std::path::Path::new(&dir).join("share").join("shares.json");
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    v["list"][1]["until"] = serde_json::json!(now_ms() - 1000);
    std::fs::write(&file, v.to_string()).unwrap();

    let mut s = server();
    s.opts.sessions_dir = dir.clone();
    assert_eq!(s.shares_load(), 1, "the one still in time");
    assert!(text(s.share_open(&kept, "10.0.0.9")).contains("report.pdf"), "on the same link");
    assert!(text(s.share_open(&timed, "10.0.0.9")).starts_with("error: "), "ended meanwhile");
    // Numbers go on after the kept ones.
    let (out, _) = add(&mut s, &d.join("folder"), None, false);
    assert!(out.contains("share 4: "), "{out}");
    let state = |s: &Server, id: u32| {
        let v: serde_json::Value = serde_json::from_str(&s.shares_json()).unwrap();
        v["shares"].as_array().unwrap().iter().find(|x| x["id"] == id).map(|x| x["state"].as_str().unwrap().to_string())
    };
    assert_eq!(
        [state(&s, 1), state(&s, 2), state(&s, 3), state(&s, 4)],
        [Some("active".into()), Some("expired".into()), Some("stopped".into()), Some("active".into())]
    );
    // Expired, extended: back on its link.
    assert!(text(s.exec_share(ShareWhat::Extend { id: 2, for_secs: Some(3600) }, None)).contains("back"));
    assert!(text(s.share_open(&timed, "10.0.0.9")).contains("folder"));
    // Stopped: only on a new link.
    let out = text(s.exec_share(ShareWhat::Extend { id: 3, for_secs: None }, None));
    assert!(out.contains("was stopped") && out.contains("share again 3"), "{out}");
    let out = text(s.exec_share(ShareWhat::Again(3), None));
    assert!(out.contains("share 5: ") && !out.contains(&stopped), "{out}");
    assert!(text(s.share_open(&stopped, "10.0.0.9")).starts_with("error: "));
    // Its file gone: missing, the link answers nothing now; back with it.
    std::fs::rename(d.join("report.pdf"), d.join("away.pdf")).unwrap();
    assert_eq!(state(&s, 1), Some("missing".into()));
    assert!(text(s.share_open(&kept, "10.0.0.9")).contains("not on that machine now"));
    std::fs::rename(d.join("away.pdf"), d.join("report.pdf")).unwrap();
    assert_eq!(state(&s, 1), Some("active".into()));
    // What happened to it is kept with it.
    let story: Vec<String> =
        s.shares.list.iter().find(|x| x.id == 1).unwrap().events.iter().map(|e| e.1.clone()).collect();
    assert!(
        story.first().is_some_and(|e| e.starts_with("shared, until"))
            && story.contains(&"opened by 10.0.0.9".to_string()),
        "{story:?}"
    );
    // Again on a running one: a new link, the old one ends.
    let out = text(s.exec_share(ShareWhat::Again(1), None));
    assert!(out.contains("share 1 ended; shared again on a new link"), "{out}");
    assert!(text(s.share_open(&kept, "10.0.0.9")).starts_with("error: "));
    assert_eq!(state(&s, 1), Some("stopped".into()));
    // At most `ENDED_KEPT` ended ones are kept.
    for _ in 0..(ENDED_KEPT + 5) {
        let (out, _) = add(&mut s, &d.join("report.pdf"), None, false);
        let id: u32 = out.split("share ").nth(1).unwrap().split(':').next().unwrap().parse().unwrap();
        text(s.exec_share(ShareWhat::Stop(Some(id)), None));
    }
    assert_eq!(s.shares.ended.len(), ENDED_KEPT);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&dir);
}
