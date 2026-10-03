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

    /// A pane of another machine on the screen, from the chart (Enter on it):
    /// asked of it, shown over the chart when it comes.
    pub(super) fn peek_remote(&mut self, cid: ClientId, addr: &str, target: &str) {
        let key = match self.link.as_ref().and_then(|l| l.peer_by_addr(addr)).map(|p| p.key.clone()) {
            Some(k) => k,
            None => return self.message(cid, &format!("{addr} is no longer paired")),
        };
        let body = serde_json::json!({ "to": target, "history": 0 }).to_string();
        let title = format!("{addr}/{target}");
        let op = LinkOp::Peek { addr: addr.to_string(), key: key.clone(), title: title.clone() };
        match self.link_signed(Some(cid), addr, &key, "POST", "/link/capture", body.into_bytes(), ASK_EVERY, op) {
            Ok(()) => self.message(cid, &format!("asking {title} for its screen")),
            Err(e) => self.message(cid, &e),
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
}
