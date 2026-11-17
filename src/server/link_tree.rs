//! The panes of the paired machines, for the chart of `choose-tree`: asked
//! for (`GET /link/panes`) when a chart opens and again every few seconds
//! while one is open, and kept as last heard.

use std::time::{Duration, Instant};

use super::link_out::LinkOp;
use super::{ClientId, Server};
use crate::link::Answer;

/// How often an open chart asks again, and how long an answer may take.
pub(super) const ASK_EVERY: Duration = Duration::from_secs(5);

/// One pane of another machine, from a line of its `/link/panes`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RemotePane {
    pub session: u32,
    pub window: u32,
    pub pane: u32,
    pub name: String,
    pub mode: String,
    pub command: String,
    /// Said by a keepane of 0.26 on; an older one gives only the numbers.
    pub session_name: Option<String>,
    pub window_index: Option<u32>,
    pub window_name: Option<String>,
    pub window_active: bool,
    pub pane_index: Option<u32>,
    pub pane_active: bool,
}

/// What is known of a machine's panes.
#[derive(Debug, Default)]
pub(super) struct RemoteView {
    pub panes: Vec<RemotePane>,
    /// Why the last asking failed; the panes are then those heard before.
    pub error: Option<String>,
    /// An answer came at least once.
    pub heard: bool,
    pub asked: Option<Instant>,
    pub pending: bool,
}

/// How long after a failed asking a machine's name is asked for again.
pub(super) const NAME_AGAIN: Duration = Duration::from_secs(60);

/// A paired machine's name, as it said it (`/link/info`): asked for once
/// while it is not known, again a minute after an asking that failed.
#[derive(Debug, Default)]
pub(super) struct MachineName {
    pub name: Option<String>,
    pub asked: Option<Instant>,
    pub pending: bool,
}

/// What another machine said, as a name shown here (a window's, a
/// session's, a machine's): no control characters (an escape sequence in a
/// name would be written to the terminal by `keepane dashboard`), at most
/// `max` characters.
pub(super) fn printable(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// The name `/link/info` says a machine has: its `host` row.
pub(super) fn host_of_info(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("host").filter(|rest| rest.starts_with([' ', '\t'])))
        .map(|h| printable(h.trim(), 253).trim().to_string())
        .filter(|h| !h.is_empty())
}

/// A machine's name short, as `#{host_short}` has it: up to its first dot
/// (`macbook.local` is `macbook`).
pub(super) fn short_host(name: &str) -> &str {
    name.split('.').next().filter(|s| !s.is_empty()).unwrap_or(name)
}

/// What a window watching a pane of another machine is called, and what
/// `C-b v` says it runs: the pane's name there, else its window's, else
/// its number; then the machine's short name, else its address's host.
pub(super) fn watch_label(pane: &str, remote: Option<&RemotePane>, machine: Option<&str>, addr: &str) -> String {
    let what = match remote {
        Some(p) if !p.name.is_empty() => p.name.clone(),
        Some(p) if p.window_name.as_deref().is_some_and(|n| !n.is_empty()) => p.window_name.clone().unwrap_or_default(),
        _ => pane.to_string(),
    };
    let host = match machine {
        Some(m) => short_host(m),
        None => addr.rsplit_once(':').map_or(addr, |(h, _)| h),
    };
    printable(&format!("{what}@{host}"), 128)
}

/// `$1:@3.%7`: the session, window and pane numbers.
fn ids(address: &str) -> Option<(u32, u32, u32)> {
    let (s, rest) = address.strip_prefix('$')?.split_once(":@")?;
    let (w, p) = rest.split_once(".%")?;
    Some((s.parse().ok()?, w.parse().ok()?, p.parse().ok()?))
}

/// The lines of `/link/panes`: address, name, mode, idle or busy, inbox,
/// program; then (0.26 on) session name, window index, window active, pane
/// index, pane active, window name (last: a tab in it stays in it). A line
/// that is not one is left out; extra fields that do not read are as if
/// not there.
pub(super) fn parse_panes(text: &str) -> Vec<RemotePane> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 {
                return None;
            }
            let (session, window, pane) = ids(f[0])?;
            let mut p = RemotePane {
                session,
                window,
                pane,
                name: f[1].to_string(),
                mode: f[2].to_string(),
                command: f[5].to_string(),
                ..RemotePane::default()
            };
            if f.len() >= 12
                && let (Ok(wi), Ok(pi)) = (f[7].parse(), f[9].parse())
            {
                p.session_name = Some(f[6].to_string());
                p.window_index = Some(wi);
                p.window_active = f[8] == "1";
                p.pane_index = Some(pi);
                p.pane_active = f[10] == "1";
                p.window_name = Some(f[11..].join("\t"));
            }
            Some(p)
        })
        .collect()
}

impl Server {
    /// The paired machines' addresses, in the table's order: what the chart
    /// lists under this machine. None until the link state is loaded (the
    /// chart loads it when it opens, if this machine ever paired).
    pub(super) fn chart_hosts(&self) -> Vec<String> {
        self.link.as_ref().map(|l| l.peers.iter().map(|p| p.addr.clone()).collect()).unwrap_or_default()
    }

    /// Ask each paired machine for its panes when it is time: at once
    /// (`now`: a chart just opened), or when the last asking is
    /// `ASK_EVERY` old. A machine that never paired loads nothing for it
    /// (the link state would make a key pair).
    pub(super) fn ask_remote_trees(&mut self, now: bool) {
        if self.link.is_none()
            && (crate::link::load_table(&crate::link::dir(&self.socket)).is_empty() || self.link().is_err())
        {
            return;
        }
        let peers: Vec<(String, String)> = self
            .link
            .as_ref()
            .map(|l| l.peers.iter().map(|p| (p.addr.clone(), p.key.clone())).collect())
            .unwrap_or_default();
        self.remote_views.retain(|addr, _| peers.iter().any(|(a, _)| a == addr));
        for (addr, key) in peers {
            // Disconnected here: asked nothing (the chart says so).
            if self.link_off(&key) {
                self.remote_views.remove(&addr);
                continue;
            }
            let v = self.remote_views.entry(addr.clone()).or_default();
            if v.pending || (!now && v.asked.is_some_and(|t| t.elapsed() < ASK_EVERY)) {
                continue;
            }
            v.pending = true;
            v.asked = Some(Instant::now());
            let op = LinkOp::Tree { addr: addr.clone(), key: key.clone() };
            if let Err(e) = self.link_signed(None, &addr, &key, "GET", "/link/panes", Vec::new(), ASK_EVERY, op) {
                let v = self.remote_views.entry(addr).or_default();
                v.pending = false;
                v.error = Some(e);
            }
        }
    }

    /// Ask each paired machine whose name is not known for it (at most once
    /// a minute each). A machine that never paired loads nothing for it.
    pub(super) fn ask_machine_names(&mut self) {
        let Some(link) = self.link.as_ref() else { return };
        let peers: Vec<(String, String)> = link.peers.iter().map(|p| (p.addr.clone(), p.key.clone())).collect();
        self.machine_names.retain(|addr, _| peers.iter().any(|(a, _)| a == addr));
        for (addr, key) in peers {
            if self.link_off(&key) {
                continue;
            }
            let n = self.machine_names.entry(addr.clone()).or_default();
            if n.name.is_some() || n.pending || n.asked.is_some_and(|t| t.elapsed() < NAME_AGAIN) {
                continue;
            }
            n.pending = true;
            n.asked = Some(Instant::now());
            let op = LinkOp::Name { addr: addr.clone(), key: key.clone() };
            if self.link_signed(None, &addr, &key, "GET", "/link/info", Vec::new(), ASK_EVERY, op).is_err() {
                self.machine_names.entry(addr).or_default().pending = false;
            }
        }
    }

    /// A machine's name came back (or did not: asked again later).
    pub(super) fn machine_name_answered(&mut self, addr: &str, result: Result<Answer, String>) {
        let Some(n) = self.machine_names.get_mut(addr) else { return };
        n.pending = false;
        if let Ok(ans) = result
            && ans.status == 200
        {
            n.name = host_of_info(&String::from_utf8_lossy(&ans.body)).or(n.name.take());
        }
    }

    /// A paired machine's name, once it said it.
    pub(super) fn machine_name(&self, addr: &str) -> Option<&str> {
        self.machine_names.get(addr).and_then(|n| n.name.as_deref())
    }

    /// What a pane running `keepane link watch host:port/pane` shows, for
    /// `#{pane_watch}`; None for any other program.
    pub(super) fn watch_label_of(&self, argv: &[String]) -> Option<String> {
        let n = argv.len();
        if n < 3 || argv[n - 3] != "link" || argv[n - 2] != "watch" {
            return None;
        }
        let (addr, pane) = crate::link::split_remote(&argv[n - 1])?;
        let bare = pane.trim_start_matches('%');
        let remote = self
            .remote_views
            .get(addr)
            .and_then(|v| v.panes.iter().find(|p| bare.parse::<u32>().map_or(p.name == bare, |id| p.pane == id)));
        Some(watch_label(pane, remote, self.machine_name(addr), addr))
    }

    /// Whether a client has the chart open: what keeps the asking going.
    pub(super) fn a_chart_is_open(&self) -> bool {
        self.clients.values().any(|c| c.chooser.as_ref().is_some_and(|ch| ch.is_chart()))
    }

    /// A machine's panes came back (or did not).
    pub(super) fn remote_tree_answered(&mut self, addr: &str, result: Result<Answer, String>) {
        let Some(v) = self.remote_views.get_mut(addr) else { return };
        v.pending = false;
        match result {
            Ok(ans) if ans.status == 200 => {
                v.panes = parse_panes(&String::from_utf8_lossy(&ans.body));
                v.error = None;
                v.heard = true;
            }
            Ok(ans) => v.error = Some(format!("said: {}", ans.text())),
            Err(e) => v.error = Some(e),
        }
    }

    /// Enter on a pane of another machine in the chart: a window running
    /// `keepane link watch` on it (live; typed into when that machine gave
    /// this one all of itself), in the session the client is in; the window
    /// Enter opened for it before, while that is still there (a watch
    /// started by hand under the pane's name is not known as the same).
    pub(super) fn watch_remote(&mut self, cid: ClientId, addr: &str, pane: u32) {
        let target = format!("{addr}/%{pane}");
        let watching =
            |argv: &[String]| argv.len() >= 3 && argv[argv.len() - 3..] == ["link", "watch", target.as_str()];
        let found = self.sessions.iter().find_map(|s| {
            s.windows.iter().find_map(|w| w.panes.iter().find(|p| watching(&p.argv)).map(|p| (s.id, w.id, p.id)))
        });
        if let Some((sid, wid, pid)) = found {
            return self.chooser_go_pane(cid, sid, wid, pid);
        }
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(exe) = super::pane::helper_exe() else {
            return self.message(cid, "no keepane executable to watch it with");
        };
        let argv = vec![exe.to_string_lossy().into_owned(), "link".into(), "watch".into(), target];
        let name = self.watch_label_of(&argv).unwrap_or_else(|| format!("%{pane}@{addr}"));
        if let Err(e) = self.new_window(sid, Some(name), None, &argv) {
            self.message(cid, &e);
        }
    }

    /// A session of another machine by its number there, as the chart
    /// names it: its name (0.26 on), else `$N`, which names it there too.
    pub(super) fn remote_session_name(&self, addr: &str, session: u32) -> String {
        self.remote_views
            .get(addr)
            .and_then(|v| v.panes.iter().find(|p| p.session == session))
            .and_then(|p| p.session_name.clone())
            .unwrap_or_else(|| format!("${session}"))
    }

    /// Enter on a session of another machine in the chart (or on the
    /// machine: its session used last): a window running `keepane link
    /// attach` to it, in the session the client is in; the window Enter
    /// opened for it before, while that is still there.
    pub(super) fn attach_remote(&mut self, cid: ClientId, addr: &str, session: Option<&str>) {
        let mut tail = vec!["link".to_string(), "attach".into(), addr.to_string()];
        tail.extend(session.map(String::from));
        let attached = |argv: &[String]| argv.len() > tail.len() && argv[argv.len() - tail.len()..] == tail[..];
        let found = self.sessions.iter().find_map(|s| {
            s.windows.iter().find_map(|w| w.panes.iter().find(|p| attached(&p.argv)).map(|p| (s.id, w.id, p.id)))
        });
        if let Some((sid, wid, pid)) = found {
            return self.chooser_go_pane(cid, sid, wid, pid);
        }
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(exe) = super::pane::helper_exe() else {
            return self.message(cid, "no keepane executable to attach with");
        };
        let argv: Vec<String> = std::iter::once(exe.to_string_lossy().into_owned()).chain(tail).collect();
        let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
        let name = session.map_or_else(|| host.to_string(), |s| format!("{host}/{s}"));
        if let Err(e) = self.new_window(sid, Some(name), None, &argv) {
            self.message(cid, &e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_list_reads_with_and_without_the_names() {
        let new = "$1:@3.%7\tbuilder\tshell\tidle\t0\tpwsh\twork\t0\t1\t1\t1\tedit\tor\n\
                   $2:@5.%9\t\tnormal\tbusy\t2\tcargo\tops\t1\t0\t0\t0\tlogs\n";
        let p = parse_panes(new);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].session, p[0].window, p[0].pane), (1, 3, 7));
        assert_eq!(p[0].name, "builder");
        assert_eq!(p[0].session_name.as_deref(), Some("work"));
        assert_eq!(
            (p[0].window_index, p[0].window_active, p[0].pane_index, p[0].pane_active),
            (Some(0), true, Some(1), true)
        );
        assert_eq!(p[0].window_name.as_deref(), Some("edit\tor"), "a tab in the window name stays in it");
        assert_eq!(p[1].command, "cargo");
        assert!(!p[1].window_active);
        // An older keepane: six fields, no names.
        let old = parse_panes("$1:@3.%7\tbuilder\tshell\tidle\t0\tpwsh\n");
        assert_eq!(old.len(), 1);
        assert_eq!((old[0].session_name.clone(), old[0].window_index), (None, None));
        // What is not a pane line is left out; fields that do not read are
        // as if not there.
        assert!(parse_panes("garbage\n\n$x:@1.%2\ta\tb\tc\td\te\n").is_empty());
        let odd = parse_panes("$1:@3.%7\ta\tnormal\tidle\t0\tsh\ts\tX\t1\tY\t1\tw\n");
        assert_eq!(odd[0].window_index, None);
    }

    /// A machine's name from its `/link/info`; a watch window's name and
    /// what `C-b v` says it runs: the pane's name there, else its window's,
    /// else its number; the machine's short name, else its address's host.
    #[test]
    fn a_watched_pane_is_called_by_its_names() {
        let info = "host     dfines-MacBook-Pro-4.local\nsystem   macos aarch64\nkeepane  0.67.0\n";
        assert_eq!(host_of_info(info).as_deref(), Some("dfines-MacBook-Pro-4.local"));
        assert_eq!(host_of_info("hosts    x\nsystem   y"), None, "only the host row");
        assert_eq!(host_of_info("host\nsystem y"), None, "an empty name is none");
        assert_eq!(host_of_info(""), None);
        // What a machine says is shown here: never an escape sequence.
        assert_eq!(host_of_info("host     mac\x1b]52;c;aGk=\x07book\n").as_deref(), Some("mac]52;c;aGk=book"));
        assert_eq!(host_of_info("host     \x1b\x07\n"), None, "nothing printable is no name");
        assert_eq!(host_of_info(&format!("host {}\n", "a".repeat(400))).map(|h| h.len()), Some(253));
        assert_eq!(printable("a\tb\r\nc\u{9b}2J", 10), "abc2J");
        let odd = RemotePane { name: "x\x1b[2Jy".into(), ..RemotePane::default() };
        assert_eq!(watch_label("%1", Some(&odd), Some("h\x07ost"), "a:1"), "x[2Jy@host");
        assert_eq!(short_host("dfines-MacBook-Pro-4.local"), "dfines-MacBook-Pro-4");
        assert_eq!(short_host("DESK"), "DESK");
        assert_eq!(short_host(".odd"), ".odd", "never an empty name");
        let pane = |name: &str, window: Option<&str>| RemotePane {
            pane: 29,
            name: name.into(),
            window_name: window.map(String::from),
            ..RemotePane::default()
        };
        let addr = "100.89.165.28:7681";
        let mac = Some("dfines-MacBook-Pro-4.local");
        assert_eq!(watch_label("%29", Some(&pane("raqs", Some("RAQS"))), mac, addr), "raqs@dfines-MacBook-Pro-4");
        assert_eq!(watch_label("%29", Some(&pane("", Some("RAQS"))), mac, addr), "RAQS@dfines-MacBook-Pro-4");
        assert_eq!(watch_label("%29", Some(&pane("", Some(""))), mac, addr), "%29@dfines-MacBook-Pro-4");
        assert_eq!(watch_label("%29", Some(&pane("", None)), None, addr), "%29@100.89.165.28");
        assert_eq!(watch_label("%29", None, None, addr), "%29@100.89.165.28", "not heard: its number");
        assert_eq!(watch_label("%29", None, None, "[fd7a::1]:7681"), "%29@[fd7a::1]");
    }

    /// `#{pane_watch}` reads the pane's command line: a watch, by the pane's
    /// number or its name there, as last heard; any other program, none.
    #[test]
    fn the_watch_a_pane_runs_is_read_from_its_command() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        let addr = "10.0.0.2:7681";
        let argv = |target: &str| ["keepane.exe", "link", "watch", target].map(String::from).to_vec();
        assert_eq!(s.watch_label_of(&argv(&format!("{addr}/%7"))).as_deref(), Some("%7@10.0.0.2"));
        let view = RemoteView {
            panes: parse_panes("$1:@3.%7\tbuilder\tshell\tidle\t0\tpwsh\twork\t0\t1\t1\t1\tedit\n"),
            ..RemoteView::default()
        };
        s.remote_views.insert(addr.into(), view);
        s.machine_names.insert(addr.into(), MachineName { name: Some("mac.local".into()), ..Default::default() });
        assert_eq!(s.watch_label_of(&argv(&format!("{addr}/%7"))).as_deref(), Some("builder@mac"));
        assert_eq!(s.watch_label_of(&argv(&format!("{addr}/%builder"))).as_deref(), Some("builder@mac"));
        assert_eq!(s.watch_label_of(&argv(&format!("{addr}/%8"))).as_deref(), Some("%8@mac"), "not in the list");
        assert_eq!(s.watch_label_of(&["pwsh".to_string()]), None);
        assert_eq!(s.watch_label_of(&argv("not-a-target")), None);
        let attach = ["keepane", "link", "attach", addr].map(String::from).to_vec();
        assert_eq!(s.watch_label_of(&attach), None);
    }
}
