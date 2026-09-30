//! What this server keeps for the links to other machines
//! (docs/design/link.md): its key pair, the machines it lets in, the
//! nonces seen lately, and where the messages that came from another
//! machine came from. The commands that touch only this machine are here
//! too (`link-id`, `link-list`, `link-trust`, `link-allow`, `link-rekey`);
//! those that reach out or take a request in are `link_out` and `link_in`.

use std::collections::HashMap;

use chrono::{DateTime, Local};

use super::actor::MsgId;
use super::{ClientId, Outcome, Server};
use crate::link::{self, Identity, Nonces, Peer};

pub(super) struct LinkHost {
    pub(super) id: Identity,
    pub(super) peers: Vec<Peer>,
    pub(super) nonces: Nonces,
    /// When each machine (by key) last got a request through.
    pub(super) seen: HashMap<String, DateTime<Local>>,
    /// Where a message that came from another machine came from: that
    /// machine's key, and the message's number and task there, so that an
    /// answer goes back and is filed under them.
    pub(super) origin: HashMap<MsgId, Origin>,
    /// When each address was last said to be refused: said once a while,
    /// so that a flood of bad requests fills neither the status line nor
    /// the day's event log (whose cap would then keep real events out).
    pub(super) refused: HashMap<String, std::time::Instant>,
    /// Messages handed to another machine, and how they are known there.
    pub(super) sent: HashMap<MsgId, super::link_out::SentThere>,
    /// The panes another machine started here (`link-start`), by the key of
    /// that machine: the ones it may close, and what counts toward its
    /// `agent-pane-limit`. Not kept past this server.
    pub(super) made: HashMap<super::PaneId, String>,
}

/// Origins remembered at most (as many as message records, `observe`).
pub(super) const ORIGINS_KEPT: usize = 10_000;
/// A refusal from the same address is said again after this.
pub(super) const REFUSED_QUIET: std::time::Duration = std::time::Duration::from_secs(60);
/// Addresses whose refusal is remembered at most.
const REFUSED_KEPT: usize = 1024;

impl LinkHost {
    /// Whether a refusal from `addr` is to be said now (and remember that it was).
    pub(super) fn say_refused(&mut self, addr: &str) -> bool {
        let now = std::time::Instant::now();
        if self.refused.get(addr).is_some_and(|t| now.duration_since(*t) < REFUSED_QUIET) {
            return false;
        }
        if self.refused.len() >= REFUSED_KEPT {
            self.refused.retain(|_, t| now.duration_since(*t) < REFUSED_QUIET);
        }
        if self.refused.len() >= REFUSED_KEPT {
            return false;
        }
        self.refused.insert(addr.to_string(), now);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Origin {
    pub key: String,
    pub id: u64,
    pub task: u64,
}

impl LinkHost {
    pub(super) fn peer_by_addr(&self, addr: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.addr == addr)
    }

    pub(super) fn peer_by_key(&self, key: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.key == key)
    }
}

/// The refusal a pane gets for what only a person outside every pane may
/// do (the rule `set-work-mode` follows too).
pub(super) fn not_from_a_pane(what: &str) -> String {
    format!("{what} is not run from inside a pane: run it in a terminal outside keepane, or at the C-b : prompt")
}

pub(super) fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

impl Server {
    fn link_dir(&self) -> std::path::PathBuf {
        link::dir(&self.socket)
    }

    /// The link state, loaded on first use (the key made then, if none).
    pub(super) fn link(&mut self) -> Result<&mut LinkHost, String> {
        if self.link.is_none() {
            let dir = self.link_dir();
            let id = Identity::load_or_make(&dir).map_err(|e| format!("link: {e:#}"))?;
            self.link = Some(LinkHost {
                id,
                peers: link::load_table(&dir),
                nonces: Nonces::default(),
                seen: HashMap::new(),
                origin: HashMap::new(),
                refused: HashMap::new(),
                sent: HashMap::new(),
                made: HashMap::new(),
            });
        }
        Ok(self.link.as_mut().expect("just made"))
    }

    pub(super) fn save_peers(&mut self) -> Result<(), String> {
        let dir = self.link_dir();
        let peers = self.link()?.peers.clone();
        link::save_table(&dir, &peers).map_err(|e| format!("link: {e:#}"))
    }

    /// This machine's web port and key: what another machine answers to,
    /// and what pairing is proved with.
    pub(super) fn own_web(&self) -> Result<(u16, String), String> {
        match &self.web {
            Some(w) => Ok((w.port(), w.key().to_string())),
            None => {
                Err("link needs `keepane web` running here: the other machine answers to this machine's web address"
                    .into())
            }
        }
    }

    /// A machine paired with this one is said on the status line, like a
    /// phone connecting, and the event log keeps it.
    pub(super) fn link_note(&mut self, text: &str, what: &str, fields: &str) {
        self.observe.link(what, fields);
        self.note_message(text);
        let now = std::time::Instant::now();
        for c in self.clients.values_mut().filter(|c| c.session.is_some()) {
            c.message = Some((text.to_string(), now));
        }
    }

    /// The commands that touch only this machine.
    pub(super) fn exec_link_local(&mut self, cmd: crate::command::Cmd, cid: Option<ClientId>) -> Outcome {
        use crate::command::Cmd;
        match cmd {
            Cmd::LinkId => match self.link() {
                Ok(l) => Outcome::Text(format!("{}  {}", l.id.public(), link::fingerprint(&l.id.public()))),
                Err(e) => Outcome::Error(e),
            },
            Cmd::LinkList => self.link_list(),
            Cmd::LinkTrust { addr, key, shell, screen, panes } => {
                self.link_trust(cid, addr, key, [shell, screen, panes])
            }
            Cmd::LinkAllow { addr, shell, screen, panes } => self.link_allow(cid, &addr, [shell, screen, panes]),
            Cmd::LinkRekey => self.link_rekey(),
            other => Outcome::Error(format!("not a link command: {other}")),
        }
    }

    fn link_list(&mut self) -> Outcome {
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        if l.peers.is_empty() {
            return Outcome::Text("no machines paired: `keepane link add <its keepane web address>`".into());
        }
        let mut rows = vec![["MACHINE", "KEY", "SHELL", "SCREEN", "PANES", "LAST SEEN"].map(String::from).to_vec()];
        let yes = |b: bool| if b { "yes" } else { "no" }.to_string();
        for p in &l.peers {
            rows.push(vec![
                p.addr.clone(),
                link::fingerprint(&p.key),
                yes(p.shell),
                yes(p.screen),
                yes(p.panes),
                l.seen.get(&p.key).map_or("never".into(), |t| t.format("%H:%M:%S").to_string()),
            ]);
        }
        Outcome::Text(super::align_columns(&rows).join("\n"))
    }

    /// A machine let in by hand, by its key (what `keepane link id` prints
    /// there): the same table `link-add` fills through the web key.
    fn link_trust(
        &mut self,
        cid: Option<ClientId>,
        addr: String,
        key: String,
        [shell, screen, panes]: [bool; 3],
    ) -> Outcome {
        if (shell || screen || panes) && self.caller_pane(cid).is_some() {
            let flag = if shell {
                "--shell"
            } else if screen {
                "--screen"
            } else {
                "--panes"
            };
            return Outcome::Error(not_from_a_pane(&format!("link-trust {flag}")));
        }
        let by = self.by(cid);
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        if l.id.public() == key {
            return Outcome::Error("link-trust: that is this machine's own key".into());
        }
        match l.peers.iter_mut().find(|p| p.key == key) {
            Some(p) => {
                p.addr = addr.clone();
                p.shell = shell;
                p.screen = screen;
                p.panes = panes;
            }
            None => l.peers.push(Peer { key: key.clone(), addr: addr.clone(), shell, screen, panes }),
        }
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        let fp = link::fingerprint(&key);
        self.link_note(
            &format!("link: {addr} ({fp}) trusted by {by}"),
            "trusted",
            &format!(
                ",\"addr\":{},\"key\":{},\"shell\":{shell},\"screen\":{screen},\"panes\":{panes},\"by\":{}",
                js(&addr),
                js(&key),
                js(&by)
            ),
        );
        Outcome::Text(format!("trusted {addr}  {fp}"))
    }

    /// Whether a machine's messages may run as commands in `shell` panes
    /// here, whether it may read what the panes show, and whether it may
    /// start panes here. Only a person outside every pane decides: otherwise
    /// a program in a pane could let another machine run commands on this
    /// one, or read its screens.
    fn link_allow(&mut self, cid: Option<ClientId>, addr: &str, [shell, screen, panes]: [Option<bool>; 3]) -> Outcome {
        if self.caller_pane(cid).is_some() {
            return Outcome::Error(not_from_a_pane("link-allow"));
        }
        let by = self.by(cid);
        let l = match self.link() {
            Ok(l) => l,
            Err(e) => return Outcome::Error(e),
        };
        let Some(p) = l.peers.iter_mut().find(|p| p.addr == addr) else {
            return Outcome::Error(format!("link-allow: {addr} is not paired with this machine (`keepane link list`)"));
        };
        if let Some(s) = shell {
            p.shell = s;
        }
        if let Some(s) = screen {
            p.screen = s;
        }
        if let Some(s) = panes {
            p.panes = s;
        }
        let (key, now_shell, now_screen, now_panes) = (p.key.clone(), p.shell, p.screen, p.panes);
        if let Err(e) = self.save_peers() {
            return Outcome::Error(e);
        }
        let may = |b: bool| if b { "may" } else { "may not" };
        let mut said = Vec::new();
        if shell.is_some() {
            said.push(format!("{addr} {} run commands in shell panes here", may(now_shell)));
        }
        if screen.is_some() {
            said.push(format!("{addr} {} read what the panes here show", may(now_screen)));
        }
        if panes.is_some() {
            said.push(format!("{addr} {} start panes here", may(now_panes)));
        }
        let agent_commands = self.opts.agent_commands.clone();
        if panes == Some(true) {
            // What it may start, with any arguments: a shell on the list runs
            // whatever it is given.
            // Any system's shells, whichever this one is: the list is the
            // user's to write.
            const SHELLS: &[&str] = &["pwsh", "powershell", "cmd", "bash", "zsh", "sh", "fish", "dash", "ksh", "nu"];
            let shells: Vec<&str> = agent_commands
                .split_whitespace()
                .filter(|p| SHELLS.contains(&super::done::program_name(p).to_ascii_lowercase().as_str()))
                .collect();
            said.push(format!("it may start: {agent_commands} (agent-commands)"));
            if !shells.is_empty() {
                said.push(format!(
                    "{} on that list run any command they are given: set -g agent-commands without them to allow agents only",
                    shells.join(" ")
                ));
            }
        }
        let text = said.join("; ");
        self.link_note(
            &format!("link: {text} (by {by})"),
            "allowed",
            &format!(
                ",\"addr\":{},\"key\":{},\"shell\":{now_shell},\"screen\":{now_screen},\"panes\":{now_panes},\"by\":{}",
                js(addr),
                js(&key),
                js(&by)
            ),
        );
        Outcome::Text(text)
    }

    /// A new key pair: every pairing made with the old one is void.
    fn link_rekey(&mut self) -> Outcome {
        let dir = self.link_dir();
        let id = match Identity::make(&dir) {
            Ok(id) => id,
            Err(e) => return Outcome::Error(format!("link-rekey: {e:#}")),
        };
        let public = id.public();
        match self.link() {
            Ok(l) => l.id = id,
            Err(e) => return Outcome::Error(e),
        }
        self.observe.link("rekeyed", &format!(",\"key\":{}", js(&public)));
        Outcome::Text(format!(
            "{public}  {}\nevery machine paired with this one has to pair again (`keepane link add`, or `link trust` with this key)",
            link::fingerprint(&public)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Outcome, Server};
    use crate::command::Cmd;

    fn server(name: &str) -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        Server::new(pane_tx, name.into(), events)
    }

    fn text(o: Outcome) -> String {
        match o {
            Outcome::Text(t) => t,
            Outcome::Error(e) => format!("error: {e}"),
            Outcome::Ok => "ok".into(),
            _ => "other".into(),
        }
    }

    /// Each server has a key of its own, kept between loads; a machine
    /// trusted by hand is listed, may be allowed to run commands, and the
    /// table is what a new load reads back.
    #[test]
    fn the_key_is_kept_and_the_table_is_what_is_trusted() {
        let dir = std::env::temp_dir().join(format!("keepane-link-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Each socket its own directory under it (`link::dir`).
        unsafe { std::env::set_var("KEEPANE_LINK_DIR", &dir) };
        let mut a = server("link-state-a");
        let mut b = server("link-state-b");
        let key_a = text(a.exec_link_local(Cmd::LinkId, None));
        let key_b = text(b.exec_link_local(Cmd::LinkId, None));
        assert_ne!(key_a, key_b);
        let (ka, fp) = key_a.split_once("  ").unwrap();
        assert!(crate::link::valid_public(ka) && fp.starts_with("SHA256:"), "{key_a}");
        assert_eq!(text(server("link-state-a").exec_link_local(Cmd::LinkId, None)), key_a, "kept");

        assert!(text(b.exec_link_local(Cmd::LinkList, None)).starts_with("no machines paired"));
        let trust =
            Cmd::LinkTrust { addr: "10.0.0.1:7681".into(), key: ka.into(), shell: false, screen: false, panes: false };
        assert_eq!(text(b.exec_link_local(trust, None)), format!("trusted 10.0.0.1:7681  {fp}"));
        let own = Cmd::LinkTrust {
            addr: "10.0.0.2:7681".into(),
            key: key_b.split("  ").next().unwrap().into(),
            shell: false,
            screen: false,
            panes: false,
        };
        assert!(text(b.exec_link_local(own, None)).contains("own key"));
        let list = text(b.exec_link_local(Cmd::LinkList, None));
        assert!(
            list.contains("10.0.0.1:7681") && list.contains(fp) && list.contains("no") && list.contains("never"),
            "{list}"
        );

        let allow = |addr: &str, shell, screen| Cmd::LinkAllow { addr: addr.into(), shell, screen, panes: None };
        assert!(text(b.exec_link_local(allow("10.0.0.9:1", Some(true), None), None)).contains("not paired"));
        assert_eq!(
            text(b.exec_link_local(allow("10.0.0.1:7681", Some(true), None), None)),
            "10.0.0.1:7681 may run commands in shell panes here"
        );
        // Each permission on its own: the screen, leaving the shell as it is.
        assert_eq!(
            text(b.exec_link_local(allow("10.0.0.1:7681", None, Some(true)), None)),
            "10.0.0.1:7681 may read what the panes here show"
        );
        // Starting panes here, on its own too: what it may start said, and
        // that a shell on the list runs anything.
        let panes = Cmd::LinkAllow { addr: "10.0.0.1:7681".into(), shell: None, screen: None, panes: Some(true) };
        b.opts.agent_commands = "claude C:\\bin\\bash.exe codex".into();
        let said = text(b.exec_link_local(panes.clone(), None));
        assert!(said.starts_with("10.0.0.1:7681 may start panes here; it may start: claude"), "{said}");
        assert!(said.contains("C:\\bin\\bash.exe on that list run any command"), "any system's shell: {said}");
        b.opts.agent_commands = "claude codex".into();
        assert!(!text(b.exec_link_local(panes, None)).contains("run any command"), "agents only");
        let list = text(b.exec_link_local(Cmd::LinkList, None));
        assert!(list.lines().next().unwrap().contains("PANES") && list.contains("yes    yes     yes"), "{list}");
        let again = server("link-state-b");
        let mut again = again;
        let peers = &again.link().unwrap().peers;
        assert_eq!(peers.len(), 1);
        assert!(peers[0].shell && peers[0].screen && peers[0].panes && peers[0].key == ka, "read back from the file");

        let rekey = text(b.exec_link_local(Cmd::LinkRekey, None));
        assert!(rekey.contains("pair again"), "{rekey}");
        assert_ne!(text(b.exec_link_local(Cmd::LinkId, None)), key_b);
        unsafe { std::env::remove_var("KEEPANE_LINK_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
