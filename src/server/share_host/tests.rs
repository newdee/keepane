use super::*;
use crate::web::Options;
use std::net::IpAddr;

fn server() -> Server {
    let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
    let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
    let mut s = Server::new(pane_tx, "t".into(), events);
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
    assert!(out.contains(&format!("share 1: {} (file, 2 KB), until ", d.join("report.pdf").display())), "{out}");
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
    let listed = text(s.exec_share(ShareWhat::List, None));
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
    assert!(text(s.exec_share(ShareWhat::List, None)).starts_with("nothing is shared"));
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
    assert!(text(s.exec_share(ShareWhat::List, None)).contains("sent 1"));
    text(s.share_sent(&once, "10.0.0.2", ""));
    assert!(s.messages.iter().any(|m| m.ends_with("share 1: report.pdf sent to 10.0.0.2; it was for once: ended")));
    assert!(text(s.share_open(&once, "10.0.0.2")).starts_with("error: "));
    // Sent after it was stopped: nothing to count, no error.
    assert_eq!(text(s.share_sent(&once, "10.0.0.2", "")), "ok");

    s.shares_tick();
    assert_eq!(s.shares.list.len(), 1, "not yet");
    let past = Instant::now() - Duration::from_secs(1);
    s.shares.list[0].until.as_mut().unwrap().0 = past;
    // Opened at the very moment it ends: ended.
    assert!(text(s.share_open(&timed, "10.0.0.2")).starts_with("error: "));
    assert!(s.messages.iter().any(|m| m.ends_with("share 2 ended: folder (its time was up)")));
    assert!(s.shares.list.is_empty());
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
    std::fs::write(&odd, "x").unwrap();
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
