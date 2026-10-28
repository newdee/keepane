use super::*;

fn w(s: &str) -> Vec<String> {
    words(s)
}

fn t(name: &str, user: Option<&str>, port: Option<u16>) -> Target {
    Target { name: name.into(), user: user.map(String::from), port }
}

#[test]
fn an_ssh_command_line_names_where_it_goes() {
    assert_eq!(parse(&w("ssh web1")), Some(t("web1", None, None)));
    assert_eq!(parse(&w("ssh root@10.0.0.5")), Some(t("10.0.0.5", Some("root"), None)));
    assert_eq!(parse(&w("ssh -p 2222 -i ~/.ssh/id me@box")), Some(t("box", Some("me"), Some(2222))));
    assert_eq!(parse(&w("ssh -p2222 -tt box ls -l")), Some(t("box", None, Some(2222))));
    assert_eq!(parse(&w("ssh -l bob -o Port=2200 box")), Some(t("box", Some("bob"), Some(2200))));
    assert_eq!(parse(&w("ssh -oUser=amy -J jump box")), Some(t("box", Some("amy"), None)));
    assert_eq!(parse(&w("ssh ssh://kim@host.example:2022/")), Some(t("host.example", Some("kim"), Some(2022))));
    assert_eq!(parse(&w("ssh -N -L 8080:localhost:80 tun")), Some(t("tun", None, None)));
    assert_eq!(parse(&w("ssh -x -oForwardAgent=no -s -- sftp-host sftp")), Some(t("sftp-host", None, None)));
    // Windows writes the program's whole path, in quotes.
    let win = r#""C:\Windows\System32\OpenSSH\ssh.exe" -p 22 admin@srv"#;
    assert_eq!(parse(&w(win)), Some(t("srv", Some("admin"), Some(22))));
    assert_eq!(parse(&w("/usr/bin/ssh srv")), Some(t("srv", None, None)));
    // Not a machine, or not ssh.
    assert_eq!(parse(&w("ssh -V")), None);
    assert_eq!(parse(&w("ssh")), None);
    assert_eq!(parse(&w("ssh-keygen -t ed25519")), None);
    assert_eq!(parse(&w("scp a b:")), None);
    assert_eq!(parse(&w("ssh host|grep x")), None);
    assert_eq!(parse(&[]), None);
}

#[test]
fn words_keep_quotes_together() {
    assert_eq!(w(r#"a "b c" 'd e'  f"#), ["a", "b c", "d e", "f"]);
    assert_eq!(w(r#"x """#), ["x", ""]);
    assert!(w("   ").is_empty());
}

#[test]
fn a_target_reads_and_connects_again() {
    let x = t("box", Some("me"), Some(2222));
    assert_eq!(x.label(), "me@box:2222");
    assert_eq!(x.argv(), ["ssh", "-p", "2222", "me@box"]);
    assert_eq!(t("Box", None, Some(22)).key(), "box", "port 22 is no port");
    assert_eq!(t("box", None, None).argv(), ["ssh", "box"]);
}

#[test]
fn a_use_counts_once_on_the_same_machine() {
    let mut b = Book::default();
    b.used(&t("web1", Some("root"), None), 100);
    b.used(&t("web1", None, None), 200); // the same machine, by its name
    b.used(&t("WEB1", Some("root"), None), 300);
    assert_eq!(b.hosts.len(), 1);
    assert_eq!((b.hosts[0].count, b.hosts[0].last), (3, Some(300)));
    b.used(&t("web1", Some("bob"), None), 400); // another user: another entry
    assert_eq!(b.hosts.len(), 2);
}

#[test]
fn the_config_names_aliases_and_skips_patterns() {
    let mut b = Book::default();
    b.config(
        "# mine\nHost web prod-db\n  HostName 10.0.0.5\n  User root\n  Port 2200\nHost *.corp !x\n  User me\n\
         Host = \"quoted\"\n  hostname=quoted\nMatch host foo\n  User nobody\n",
    );
    let names: Vec<&str> = b.hosts.iter().map(|h| h.target.name.as_str()).collect();
    assert_eq!(names, ["web", "prod-db", "quoted"]);
    assert_eq!(b.hosts[0].target, t("web", Some("root"), Some(2200)));
    assert_eq!(b.hosts[0].real.as_deref(), Some("10.0.0.5"));
    assert_eq!(b.hosts[2].real, None, "a HostName that is its own alias says nothing");
    assert!(b.hosts.iter().all(|h| h.source == Source::Config && h.count == 0));
    // Read again: nothing doubles.
    b.config("Host web\n  HostName 10.0.0.5\n  User root\n  Port 2200\n");
    assert_eq!(b.hosts.len(), 3);
}

#[test]
fn known_hosts_gives_names_not_hashes() {
    let mut b = Book::default();
    b.config("Host web\n  HostName 10.0.0.5\n");
    b.known_hosts(
        "10.0.0.5 ssh-ed25519 AAAA\n|1|abc=|def= ssh-rsa AAAA\n[git.example]:2222,[1.2.3.4]:2222 ssh-ed25519 AAAA\n\
         # note\n@cert-authority *.x ssh-rsa AAAA\nnew.example,5.6.7.8 ecdsa AAAA\n\n",
    );
    let names: Vec<String> = b.hosts.iter().map(|h| h.target.label()).collect();
    assert_eq!(names, ["web", "git.example:2222", "new.example"], "10.0.0.5 is web's");
    assert!(b.hosts[1..].iter().all(|h| h.source == Source::Seen));
}

#[test]
fn history_counts_the_ssh_commands() {
    let mut b = Book::default();
    let entries = ["ssh a", "git push", "ssh a", "- cmd: ssh -p 2 b", "<# keepane #3 from user #> ssh c", "ssh -V"];
    b.history(entries.map(String::from));
    let got: Vec<(String, u64)> = b.hosts.iter().map(|h| (h.target.label(), h.count)).collect();
    assert_eq!(got, [("a".into(), 2), ("b:2".into(), 1), ("c".into(), 1)]);
    assert!(b.hosts.iter().all(|h| h.last.is_none()));
}

#[test]
fn listed_by_last_use_then_count_then_source() {
    let mut b = Book::default();
    b.config("Host cfg\n");
    b.known_hosts("seen.example ssh-rsa AAAA\n");
    b.history(["ssh old", "ssh old", "ssh once"].map(String::from));
    b.used(&t("fresh", None, None), 500);
    b.used(&t("older", None, None), 100);
    let names: Vec<&str> = b.order().into_iter().map(|i| b.hosts[i].target.name.as_str()).collect();
    assert_eq!(names, ["fresh", "older", "old", "once", "cfg", "seen.example"]);
}

#[test]
fn only_connected_machines_are_kept() {
    let dir = std::env::temp_dir().join(format!("keepane-ssh-book-{}", std::process::id()));
    let path = dir.join("ssh-hosts.json");
    let mut b = Book::default();
    b.config("Host cfg\n");
    b.used(&t("box", Some("me"), Some(2222)), 42);
    b.imported = true;
    b.save(&path);
    let back = Book::load(&path);
    assert_eq!(back.hosts.len(), 1);
    assert_eq!(back.hosts[0].target, t("box", Some("me"), Some(2222)));
    assert!(back.imported);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("cfg") && text.contains("\"name\": \"box\""), "{text}");
    // A damaged file is an empty book, not an error.
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(Book::load(&path), Book::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_name_known_hosts_met_takes_the_user_it_is_used_with() {
    // As found on a real machine: known_hosts names it; the history has
    // root twice and another user once.
    let mut b = Book::default();
    b.known_hosts("124.0.0.9 ssh-ed25519 AAAA\n");
    b.history(["ssh root@124.0.0.9", "ssh root@124.0.0.9", "ssh lighthouse@124.0.0.9"].map(String::from));
    let got: Vec<(String, u64, Source)> = b.hosts.iter().map(|h| (h.target.label(), h.count, h.source)).collect();
    assert_eq!(got, [("root@124.0.0.9".into(), 2, Source::Used), ("lighthouse@124.0.0.9".into(), 1, Source::Used)]);
    assert_eq!(b.hosts[0].target.argv(), ["ssh", "root@124.0.0.9"], "connects as the user it was used with");
    // A pane's use does the same, with the port; then an entry of its own
    // user and port is found again exactly.
    let mut b = Book::default();
    b.config("Host box\n");
    b.used(&t("box", Some("me"), Some(2222)), 5);
    b.used(&t("box", Some("me"), Some(2222)), 6);
    assert_eq!(b.hosts.len(), 1);
    assert_eq!((b.hosts[0].target.label(), b.hosts[0].count), ("me@box:2222".to_string(), 2));
    // The config read again finds it, and does not add the alias twice.
    b.config("Host box\n");
    assert_eq!(b.hosts.len(), 1);
}

#[test]
fn a_machine_taken_off_stays_off_until_connected_to() {
    let mut b = Book::default();
    let config = "Host Web\n  HostName 10.0.0.5\n";
    b.config(config);
    b.known_hosts("other.example ssh-rsa AAAA\n");
    b.forget(0);
    b.forget(0);
    b.forget(7); // no such entry: nothing
    assert!(b.hosts.is_empty());
    assert_eq!(b.forgotten, ["web", "other.example"]);
    // Read again: not back; nor from the history.
    b.config(config);
    b.known_hosts("other.example ssh-rsa AAAA\n");
    b.history(["ssh WEB".to_string()]);
    assert!(b.hosts.is_empty());
    // Connected to: back, and no longer forgotten.
    b.used(&t("web", None, None), 9);
    assert_eq!(b.hosts.len(), 1);
    assert_eq!(b.forgotten, ["other.example"]);
    // Kept across a save.
    let dir = std::env::temp_dir().join(format!("keepane-ssh-forget-{}", std::process::id()));
    let path = dir.join("ssh-hosts.json");
    b.save(&path);
    assert_eq!(Book::load(&path).forgotten, ["other.example"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_book_keeps_at_most_its_limit() {
    let mut b = Book::default();
    for i in 0..(MOST + 5) {
        b.used(&t(&format!("h{i}"), None, None), i as i64);
    }
    assert_eq!(b.hosts.len(), MOST);
    assert!(!b.hosts.iter().any(|h| h.target.name == "h0"), "the oldest went");
}

#[test]
fn ago_reads_short() {
    assert_eq!(ago(100, 130), "just now");
    assert_eq!(ago(0, 300), "5m ago");
    assert_eq!(ago(0, 7200), "2h ago");
    assert_eq!(ago(0, 3 * 86_400), "3d ago");
    assert_eq!(ago(0, 21 * 86_400), "3w ago");
    assert_eq!(ago(0, 90 * 86_400), "3mo ago");
    assert_eq!(ago(0, 800 * 86_400), "2y ago");
    assert_eq!(ago(500, 100), "just now", "a clock gone back");
}
