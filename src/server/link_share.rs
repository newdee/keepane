//! What a paired machine has here (docs/design/link.md, decision 14): all
//! of this machine (`link-allow --full`), or what is shared with it
//! (`link-share`): sessions, windows and panes, each marked with the keys of
//! the machines it is shared with. A session or a window shared covers what
//! is in it, now and later. The marks are on the things, not on their names,
//! so a name used again by something else is not shared by it; they go into
//! a session's save, and `resume` brings them back.
//!
//! A machine that has a pane may see it (in its chart, `link panes`,
//! `link capture`, `link watch`) and send it messages, which go by the pane's work mode
//! as anyone's do, but for a pane in `shell` mode: a message there runs as a
//! command, so it is refused. It may do nothing else: running commands,
//! typing into panes, starting and closing them, sending commits and
//! projects need `full`.
//! What is not shared with it does not exist for it: a pane it names that
//! it may not reach is answered as one that is not there.

use std::collections::HashSet;

use super::layout::PaneId;
use super::link_out::LinkOp;
use super::link_state::{js, not_from_a_pane};
use super::{ClientId, Outcome, Server, SessionId, WindowId};
use crate::command::Target;
use crate::link::{self, Answer, Peer};

/// Something that can be shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Spot {
    Session(SessionId),
    Window(WindowId),
    Pane(PaneId),
}

/// A `link-list` asking each paired machine what this one has there.
pub(super) struct ListWait {
    cid: ClientId,
    rows: Vec<Vec<String>>,
    /// The rows still waiting for their machine's answer.
    waiting: HashSet<usize>,
}

/// The column of `link-list` with what this machine has there.
const THERE: usize = 3;
/// How long `link-list` waits for each machine.
const LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// What a paired machine has here, as the answer to `GET /link/access`
/// says it, and `link-list` there shows it.
pub(super) fn access_text(level: &str, panes: usize) -> String {
    match level {
        "full" => "all of it (full)".into(),
        "held" => "full, but not over this network (only Tailscale)".into(),
        "shared" => format!("shared: {}", crate::format::count(panes, "pane")),
        _ => "nothing".into(),
    }
}

impl Server {
    /// The keys a spot is shared with, in the order they were added.
    pub(super) fn shared_keys(&self, spot: Spot) -> Vec<String> {
        self.link_shares.get(&spot).cloned().unwrap_or_default()
    }

    /// A save's marks put back on what `resume` made of it: only for the
    /// machines still paired.
    pub(super) fn restore_shared(&mut self, spot: Spot, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let paired: Vec<String> = match self.link() {
            Ok(l) => keys.iter().filter(|k| l.peer_by_key(k).is_some()).cloned().collect(),
            Err(_) => return,
        };
        if !paired.is_empty() {
            self.link_shares.insert(spot, paired);
        }
    }

    /// Whether `spot` carries a mark for anyone: what the tree shows.
    pub(super) fn is_shared(&self, spot: Spot) -> bool {
        self.link_shares.get(&spot).is_some_and(|k| !k.is_empty())
    }

    fn marked(&self, spot: Spot, key: &str) -> bool {
        self.link_shares.get(&spot).is_some_and(|k| k.iter().any(|x| x == key))
    }

    /// Whether the pane is shared with the machine whose key it is: on its
    /// own, or by its window or session.
    fn pane_shared_with(&self, key: &str, pid: PaneId) -> bool {
        let Some((sid, widx)) = self.place_of(pid) else { return false };
        let wid = self.session(sid).and_then(|s| s.windows.get(widx)).map(|w| w.id);
        self.marked(Spot::Pane(pid), key)
            || wid.is_some_and(|w| self.marked(Spot::Window(w), key))
            || self.marked(Spot::Session(sid), key)
    }

    /// Whether a request of that machine may reach the pane: `peer.full` as
    /// the request's network leaves it (`link_inbound`). A pane of a third
    /// machine copied here (`mirrored`) is that machine's to give: never.
    pub(super) fn may_reach(&self, peer: &Peer, pid: PaneId) -> bool {
        if self.find_pane(pid).is_none_or(|p| p.remote.is_some()) {
            return false;
        }
        (peer.full && self.place_of(pid).is_some()) || self.pane_shared_with(&peer.key, pid)
    }

    /// The panes that machine may reach, in the order of the sessions; none
    /// of a third machine's copied here.
    pub(super) fn reachable_panes(&self, peer: &Peer) -> Vec<PaneId> {
        let mut out = Vec::new();
        for s in self.sessions.iter().filter(|s| s.remote.is_none()) {
            for w in &s.windows {
                for p in &w.panes {
                    if peer.full
                        || self.marked(Spot::Pane(p.id), &peer.key)
                        || self.marked(Spot::Window(w.id), &peer.key)
                        || self.marked(Spot::Session(s.id), &peer.key)
                    {
                        out.push(p.id);
                    }
                }
            }
        }
        out
    }

    /// What a target names, as something to share: a pane (`%worker`,
    /// `%7`, `work:1.2`), a window (`work:logs`, `work:2`), or a session
    /// (`work`, `$1`).
    fn spot_of(&self, target: &str) -> Result<Spot, String> {
        let t = Target::parse(target);
        if t.remote.is_some() {
            return Err(format!("'{target}' is on another machine: share what is here"));
        }
        if t.names_pane() || t.pane.is_some() {
            let (_, _, pid) = self.resolve(Some(&t), None)?;
            return Ok(Spot::Pane(pid));
        }
        if t.session.is_none() {
            return Err(format!("'{target}': a session (work), a window (work:logs) or a pane (%worker)"));
        }
        let sid = self.resolve_session(Some(&t), None)?;
        if t.window.is_some() {
            let widx = self.resolve_window(sid, Some(&t))?;
            let wid = self.session(sid).and_then(|s| s.windows.get(widx)).map(|w| w.id).ok_or("no such window")?;
            return Ok(Spot::Window(wid));
        }
        Ok(Spot::Session(sid))
    }

    /// How a spot reads: `work`, `work:logs` (`work:2` when two windows
    /// there have that name), `%worker` (`%7` when the pane has no name).
    /// None for one that is gone.
    pub(super) fn spot_label(&self, spot: Spot) -> Option<String> {
        match spot {
            Spot::Session(sid) => self.session(sid).map(|s| s.name.clone()),
            Spot::Window(wid) => self.sessions.iter().find_map(|s| {
                let (i, w) = s.windows.iter().enumerate().find(|(_, w)| w.id == wid)?;
                let unique = s.windows.iter().filter(|x| x.name == w.name).count() == 1;
                Some(if unique && w.name.parse::<usize>().is_err() {
                    format!("{}:{}", s.name, w.name)
                } else {
                    format!("{}:{}", s.name, i + self.opts.base_index)
                })
            }),
            Spot::Pane(pid) => {
                let p = self.find_pane(pid).filter(|_| self.place_of(pid).is_some())?;
                Some(p.actor.name.as_ref().map_or_else(|| format!("%{pid}"), |n| format!("%{n}")))
            }
        }
    }

    /// What is shared with that machine, as it reads, things gone left out.
    pub(super) fn shared_labels(&self, key: &str) -> Vec<String> {
        let mut spots: Vec<Spot> =
            self.link_shares.iter().filter(|(_, k)| k.iter().any(|x| x == key)).map(|(s, _)| *s).collect();
        // Sessions, then windows, then panes, each by id: the order they came.
        spots.sort_by_key(|s| match s {
            Spot::Session(i) => (0, *i),
            Spot::Window(i) => (1, *i),
            Spot::Pane(i) => (2, *i),
        });
        spots.into_iter().filter_map(|s| self.spot_label(s)).collect()
    }

    /// Everything shared with that machine taken back; how many marks went.
    pub(super) fn unshare_everything(&mut self, key: &str) -> usize {
        let mut n = 0;
        for keys in self.link_shares.values_mut() {
            let before = keys.len();
            keys.retain(|k| k != key);
            n += before - keys.len();
        }
        self.link_shares.retain(|_, k| !k.is_empty());
        n
    }

    /// The session a spot is in.
    fn spot_session(&self, spot: Spot) -> Option<SessionId> {
        match spot {
            Spot::Session(sid) => Some(sid),
            Spot::Window(wid) => self.sessions.iter().find(|s| s.windows.iter().any(|w| w.id == wid)).map(|s| s.id),
            Spot::Pane(pid) => self.session_of_pane(pid),
        }
    }

    /// `link-share` and `link-unshare`.
    pub(super) fn link_share(&mut self, cid: Option<ClientId>, addr: &str, targets: &[String], share: bool) -> Outcome {
        // What a pane may not do: let another machine in. Taking back is
        // anyone's.
        if share && self.caller_pane(cid).is_some() {
            return Outcome::Error(not_from_a_pane("link-share"));
        }
        let name = if share { "link-share" } else { "link-unshare" };
        let peer = match self.link() {
            Ok(l) => match l.peer_by_addr(addr) {
                Some(p) => p.clone(),
                None => {
                    return Outcome::Error(format!(
                        "{name}: {addr} is not paired with this machine (`keepane link list`)"
                    ));
                }
            },
            Err(e) => return Outcome::Error(e),
        };
        let mut spots = Vec::new();
        for t in targets {
            match self.spot_of(t) {
                // Named twice (`work $1`): once.
                Ok(s) if spots.contains(&s) => {}
                Ok(s) => spots.push(s),
                Err(e) => return Outcome::Error(format!("{name}: {e}")),
            }
        }
        self.share_spots(cid, &peer, &spots, share, targets.is_empty())
    }

    /// Mark (or unmark) those spots for the machine, and say what it has
    /// now. `all`: unmark everything of it.
    pub(super) fn share_spots(
        &mut self,
        cid: Option<ClientId>,
        peer: &Peer,
        spots: &[Spot],
        share: bool,
        all: bool,
    ) -> Outcome {
        // A third machine's session copied here (`mirrored`) is that
        // machine's to share: never given on from here.
        if share
            && let Some(r) = spots.iter().filter_map(|s| self.spot_session(*s)).find_map(|sid| {
                self.session(sid).and_then(|s| s.remote.as_ref().map(|r| (s.name.clone(), r.addr.clone())))
            })
        {
            return Outcome::Error(format!("{} is a session of {}: only that machine can share it", r.0, r.1));
        }
        let by = self.by(cid);
        let addr = peer.addr.clone();
        let labels: Vec<String> = spots.iter().filter_map(|s| self.spot_label(*s)).collect();
        let mut text = if all && !share {
            let n = self.unshare_everything(&peer.key);
            if n == 0 {
                format!("nothing here was shared with {addr}")
            } else {
                format!("nothing here is shared with {addr} any more")
            }
        } else if share {
            for s in spots {
                let keys = self.link_shares.entry(*s).or_default();
                if !keys.contains(&peer.key) {
                    keys.push(peer.key.clone());
                }
            }
            let mut said = format!("{} shared with {addr}: it sees them and may send them messages", labels.join(", "));
            // A message to a `shell` pane would run there as a command, which
            // takes all of this machine: said for each such pane now within
            // reach, so that a refused message is not a surprise.
            let shell: Vec<String> = self
                .reachable_panes(&Peer { full: false, ..peer.clone() })
                .into_iter()
                .filter(|p| {
                    self.pane_ref(*p).is_some_and(|p| p.actor.mode == super::actor::WorkMode::Shell)
                        && spots.iter().any(|s| self.spot_covers(*s, *p))
                })
                .filter_map(|p| self.spot_label(Spot::Pane(p)))
                .collect();
            if !shell.is_empty() {
                said.push_str(&format!(
                    "; {} in shell mode: it may watch, but its messages there are refused (they would run as commands: that takes --full)",
                    if shell.len() == 1 { format!("{} is", shell[0]) } else { format!("{} are", shell.join(", ")) }
                ));
            }
            said
        } else {
            let mut taken = 0;
            for s in spots {
                if let Some(keys) = self.link_shares.get_mut(s) {
                    let before = keys.len();
                    keys.retain(|k| *k != peer.key);
                    taken += before - keys.len();
                }
            }
            self.link_shares.retain(|_, k| !k.is_empty());
            let mut said = if taken == 0 {
                format!("{} was not shared with {addr}", labels.join(", "))
            } else {
                format!("{} no longer shared with {addr}", labels.join(", "))
            };
            // What is still in something shared is still seen: said, so that
            // "no longer" is not read as gone from that machine's sight.
            let through: Vec<String> = spots
                .iter()
                .filter_map(|s| {
                    let above = self.shared_above(*s, &peer.key)?;
                    Some(format!("{} through {}", self.spot_label(*s)?, self.spot_label(above)?))
                })
                .collect();
            if !through.is_empty() {
                said.push_str(&format!("; it still sees {}", through.join(", ")));
            }
            said
        };
        if peer.full {
            text.push_str(&format!(" ({addr} has all of this machine anyway: `link allow {addr} --none` ends that)"));
        }
        let what = if share { "shared" } else { "unshared" };
        let list: Vec<String> = labels.iter().map(|l| js(l)).collect();
        self.link_note(
            &format!("link: {text} (by {by})"),
            what,
            &format!(
                ",\"addr\":{},\"key\":{},\"what\":[{}],\"all\":{all},\"by\":{}",
                js(&addr),
                js(&peer.key),
                list.join(","),
                js(&by)
            ),
        );
        Outcome::Text(text)
    }

    /// The window or session around a spot that is shared with that machine
    /// (the nearest), if any: what still shows the spot to it.
    fn shared_above(&self, spot: Spot, key: &str) -> Option<Spot> {
        let (sid, wid) = match spot {
            Spot::Session(_) => return None,
            Spot::Window(w) => (self.sessions.iter().find(|s| s.windows.iter().any(|x| x.id == w))?.id, None),
            Spot::Pane(p) => {
                let (sid, widx) = self.place_of(p)?;
                (sid, self.session(sid).and_then(|s| s.windows.get(widx)).map(|w| w.id))
            }
        };
        if let Some(w) = wid.filter(|w| self.marked(Spot::Window(*w), key)) {
            return Some(Spot::Window(w));
        }
        self.marked(Spot::Session(sid), key).then_some(Spot::Session(sid))
    }

    /// Whether the pane is in what the spot is.
    fn spot_covers(&self, spot: Spot, pid: PaneId) -> bool {
        match spot {
            Spot::Pane(p) => p == pid,
            Spot::Window(w) => self
                .place_of(pid)
                .and_then(|(sid, widx)| self.session(sid).and_then(|s| s.windows.get(widx)))
                .is_some_and(|win| win.id == w),
            Spot::Session(s) => self.place_of(pid).is_some_and(|(sid, _)| sid == s),
        }
    }

    /// `s` in the tree: what is under the cursor (or tagged) shared with
    /// the paired machine, or no longer when it all was; with several
    /// machines paired, a menu of them.
    pub(super) fn picker_share(&mut self, cid: ClientId, items: Vec<super::ChooserItem>) {
        use super::ChooserItem;
        let spots: Vec<Spot> = items
            .iter()
            .filter_map(|i| match *i {
                ChooserItem::Tree(sid, None) => Some(Spot::Session(sid)),
                ChooserItem::Tree(_, Some(wid)) => Some(Spot::Window(wid)),
                ChooserItem::Job(_, _, pid) => Some(Spot::Pane(pid)),
                _ => None,
            })
            .collect();
        if spots.is_empty() {
            return self.message(cid, "s shares a session, window or pane of this machine");
        }
        if self.caller_pane(Some(cid)).is_some() {
            return self.message(cid, &not_from_a_pane("link-share"));
        }
        let peers: Vec<Peer> = if link::load_table(&self.link_dir()).is_empty() {
            Vec::new()
        } else {
            self.link().map(|l| l.peers.clone()).unwrap_or_default()
        };
        let targets: Vec<String> = spots
            .iter()
            .map(|s| match *s {
                Spot::Session(sid) => format!("${sid}"),
                Spot::Window(wid) => {
                    let sid = self.sessions.iter().find(|s| s.windows.iter().any(|w| w.id == wid)).map(|s| s.id);
                    format!("${}:@{wid}", sid.unwrap_or(0))
                }
                Spot::Pane(pid) => format!("%{pid}"),
            })
            .collect();
        let all_shared = |key: &str| spots.iter().all(|s| self.marked(*s, key));
        match peers.as_slice() {
            [] => self.message(cid, "no machine is paired with this one: `keepane link add <its keepane web address>`"),
            [one] => {
                // Said on the status line by `share_spots` (`link_note`);
                // only a refusal is left to say.
                let share = !all_shared(&one.key);
                if let out @ Outcome::Error(_) = self.share_spots(Some(cid), one, &spots, share, false) {
                    self.reply(cid, out);
                }
            }
            many => {
                let items: Vec<crate::command::MenuItem> = many
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let share = !all_shared(&p.key);
                        let cmd =
                            crate::command::Cmd::LinkShare { addr: p.addr.clone(), targets: targets.clone(), share };
                        crate::command::MenuItem {
                            name: format!("{} {}", if share { "share with" } else { "stop sharing with" }, p.addr),
                            key: Some(((b'1' + (i % 9) as u8) as char).to_string()),
                            cmd: Some(Box::new(cmd)),
                        }
                    })
                    .collect();
                let menu = crate::command::Cmd::DisplayMenu { title: Some("share with".into()), items };
                let out = self.exec(menu, Some(cid));
                self.reply(cid, out);
            }
        }
    }

    /// `link-list`: each paired machine, what it has here, and (asked of it,
    /// a few seconds at most) what this one has there.
    pub(super) fn link_list(&mut self, cid: Option<ClientId>) -> Outcome {
        let peers = match self.link() {
            Ok(l) => l.peers.clone(),
            Err(e) => return Outcome::Error(e),
        };
        if peers.is_empty() {
            return Outcome::Text("no machines paired: `keepane link add <its keepane web address>`".into());
        }
        let fed = self.feeds_to();
        let mut rows =
            vec![["MACHINE", "KEY", "IT HAS HERE", "THIS HAS THERE", "LAST SEEN"].map(String::from).to_vec()];
        for p in &peers {
            let here = if p.full {
                if p.lan {
                    "all of it (full, any network)".to_string()
                } else {
                    "all of it (full, over Tailscale)".to_string()
                }
            } else {
                let shared = self.shared_labels(&p.key);
                if shared.is_empty() { "nothing".to_string() } else { format!("shared: {}", shared.join(", ")) }
            };
            // Copying this machine's sessions over a link stream right now.
            let here = if fed.contains(&p.key) { format!("{here}; copies it now") } else { here };
            let seen = self
                .link
                .as_ref()
                .and_then(|l| l.seen.get(&p.key))
                .map_or("never".into(), |t| t.format("%H:%M:%S").to_string());
            rows.push(vec![p.addr.clone(), link::fingerprint(&p.key), here, "?".into(), seen]);
        }
        // Without a client to answer later, or without `keepane web` here
        // (the other machine answers to it), the table as it is.
        let Some(cid) = cid else { return Outcome::Text(super::align_columns(&rows).join("\n")) };
        if self.own_web().is_err() {
            for r in rows.iter_mut().skip(1) {
                r[THERE] = "? (keepane web is off here)".into();
            }
            return Outcome::Text(super::align_columns(&rows).join("\n"));
        }
        self.next_link_list += 1;
        let list = self.next_link_list;
        let mut waiting = HashSet::new();
        for (i, p) in peers.iter().enumerate() {
            let op = LinkOp::Access { addr: p.addr.clone(), key: p.key.clone(), list, row: i + 1 };
            match self.link_signed(Some(cid), &p.addr, &p.key, "GET", "/link/access", Vec::new(), LIST_WAIT, op) {
                Ok(()) => {
                    waiting.insert(i + 1);
                }
                Err(e) => rows[i + 1][THERE] = format!("? ({e})"),
            }
        }
        if waiting.is_empty() {
            return Outcome::Text(super::align_columns(&rows).join("\n"));
        }
        self.link_lists.insert(list, ListWait { cid, rows, waiting });
        Outcome::Pending
    }

    /// A machine said what this one has there (or did not): the row filled
    /// in, and the table told when the last one is in.
    pub(super) fn link_list_answered(&mut self, list: u64, row: usize, result: Result<Answer, String>) {
        let Some(w) = self.link_lists.get_mut(&list) else { return };
        if !w.waiting.remove(&row) {
            return;
        }
        w.rows[row][THERE] = match result {
            Ok(a) if a.status == 200 => {
                let v: serde_json::Value = serde_json::from_slice(&a.body).unwrap_or_default();
                access_text(v["level"].as_str().unwrap_or_default(), v["panes"].as_u64().unwrap_or(0) as usize)
            }
            // A keepane from before the levels does not know the question.
            Ok(a) if a.status == 404 => "? (keepane there is older than 0.56)".into(),
            Ok(a) => format!("? ({})", super::truncate(&a.text(), 40)),
            Err(_) => "? (offline)".into(),
        };
        if w.waiting.is_empty() {
            let w = self.link_lists.remove(&list).expect("just had it");
            self.reply(w.cid, Outcome::Text(super::align_columns(&w.rows).join("\n")));
        }
    }

    /// `GET /link/access`: what the asking machine has here, for its
    /// `link-list`. `held`: it has `full`, but not over the network the
    /// request came by.
    pub(super) fn link_access_in(&mut self, nonce: &str, peer: &Peer, held: bool) -> Outcome {
        let panes = self.reachable_panes(peer).len();
        let level = if peer.full {
            "full"
        } else if held {
            "held"
        } else if panes == 0 {
            "none"
        } else {
            "shared"
        };
        self.link_answer(nonce, 200, format!("{{\"level\":\"{level}\",\"panes\":{panes}}}"))
    }
}

#[cfg(test)]
mod tests {
    use super::access_text;

    #[test]
    fn what_a_machine_has_reads_plainly() {
        assert_eq!(access_text("full", 9), "all of it (full)");
        assert_eq!(access_text("shared", 1), "shared: 1 pane");
        assert_eq!(access_text("shared", 3), "shared: 3 panes");
        assert_eq!(access_text("none", 0), "nothing");
        assert_eq!(access_text("what", 0), "nothing", "a word it does not know is nothing");
        assert!(access_text("held", 0).contains("Tailscale"));
    }
}
