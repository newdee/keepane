//! The panes' repositories as the server knows them (`crate::git`): each
//! repository's status, asked of git on a thread of its own and kept a
//! while, so what shows it (the status line, the clock, a pane's border,
//! the dashboard) only ever reads what is kept; and an agent's work, ended,
//! told only once what it did in its repository is known.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::actor::{End, Message, WorkMode};
use super::{Event, PaneId, Server};
use crate::git;

/// How long a repository's status stands while something shows it, before
/// git is asked again.
const FRESH: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct GitState {
    /// The git found, once looked for (again when `git-command` changes);
    /// `Some(None)`: there is none.
    command: Option<Option<PathBuf>>,
    /// Which git it is, counted up each time `git-command` changes: an
    /// answer from the one before is let go.
    generation: u64,
    statuses: HashMap<PathBuf, Seen>,
    /// Work ended whose repository is being looked at: what is left to do
    /// with it once that is known.
    finishing: HashMap<u64, Finish>,
    next: u64,
}

/// A repository's status as last heard, when, whether git is being asked
/// now, and whether something since may have changed it.
#[derive(Default)]
struct Seen {
    status: Option<git::Status>,
    at: Option<Instant>,
    asking: bool,
    stale: bool,
}

/// What ends with an agent's work: its turn (the line its telling says),
/// and the message it worked on (how it ended, whether it went well, what
/// it printed).
pub(super) struct Finish {
    pub pid: PaneId,
    pub turn: Option<String>,
    pub message: Option<MessageEnd>,
}

/// A message's end: how it ended, whether it went well, what it printed
/// (and whether that was cut).
pub(super) struct MessageEnd {
    pub m: Message,
    pub end: End,
    pub ok: Option<bool>,
    pub output: Option<(String, bool)>,
}

impl Server {
    /// The git to run, looked for once.
    pub(super) fn git_command(&mut self) -> Option<PathBuf> {
        if self.git.command.is_none() {
            let found = git::command(&self.opts.git_command);
            if found.is_none() {
                log::info!("git: none found; only the branch and the commit are read");
            }
            self.git.command = Some(found);
        }
        self.git.command.clone().flatten()
    }

    /// `git-command` changed: look again, and ask again.
    pub(super) fn git_command_changed(&mut self) {
        self.git.command = None;
        self.git.generation += 1;
        self.git.statuses.clear();
    }

    /// The status kept for the repository `path` is in, if any was heard.
    /// (With no git to run there is none: nothing is asked, and what was
    /// kept went when `git-command` changed.)
    pub(super) fn git_status(&self, path: &str) -> Option<&git::Status> {
        let repo = git::find(path)?;
        self.git.statuses.get(&repo.root)?.status.as_ref()
    }

    /// Something shows the repository `path` is in: git is asked for its
    /// status when what is kept is old, or may have changed, and it is not
    /// being asked already.
    pub(super) fn git_look(&mut self, path: &str) {
        let Some(repo) = git::find(path) else { return };
        let seen = self.git.statuses.entry(repo.root.clone()).or_default();
        let old = seen.at.is_none_or(|at| at.elapsed() >= FRESH);
        if seen.asking || !(old || seen.stale) {
            return;
        }
        let Some(command) = self.git_command() else { return };
        let (tx, generation) = (self.events.clone(), self.git.generation);
        let (key, root) = (repo.root.clone(), repo.root.clone());
        let asked = std::thread::Builder::new().name("git-status".into()).spawn(move || {
            let status =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| git::status(&command, &repo))).unwrap_or(None);
            let _ = tx.send(Event::GitStatus(generation, root, status));
        });
        if let Some(seen) = self.git.statuses.get_mut(&key) {
            seen.asking = asked.is_ok();
            seen.stale = false;
        }
    }

    /// git answered for a repository (nothing, when it failed); an answer
    /// from the git before `git-command` changed is let go.
    pub(super) fn git_status_heard(&mut self, generation: u64, root: PathBuf, status: Option<git::Status>) {
        if generation != self.git.generation {
            return;
        }
        let seen = self.git.statuses.entry(root).or_default();
        seen.asking = false;
        seen.at = Some(Instant::now());
        seen.status = status;
    }

    /// Something happened in pane `pid` that may have changed its
    /// repository (a command ended, an agent's turn): asked again when next
    /// shown.
    pub(super) fn git_moved(&mut self, pid: PaneId) {
        let Some(path) = self.pane_ref(pid).and_then(|p| p.current_path()) else { return };
        if let Some(repo) = git::find(&path)
            && let Some(seen) = self.git.statuses.get_mut(&repo.root)
        {
            seen.stale = true;
        }
    }

    /// From the tick: the repositories of the panes on the screens (each
    /// attached client's window), looked at.
    pub(super) fn git_tick(&mut self) {
        let mut paths: Vec<String> = Vec::new();
        for c in self.clients.values() {
            let Some(w) = c.session.and_then(|sid| self.session(sid)).and_then(|s| s.window()) else { continue };
            paths.extend(w.panes.iter().filter_map(|p| p.current_path()));
        }
        paths.sort();
        paths.dedup();
        for p in paths {
            self.git_look(&p);
        }
        // Statuses no screen has looked at for long are let go.
        self.git.statuses.retain(|_, s| s.asking || s.at.is_none_or(|at| at.elapsed() < FRESH * 30));
    }

    /// An agent's work ended in pane `pid`: told (its turn) and recorded
    /// (its message) once what it did in its repository is known; at once
    /// when it is no agent's, or not in a repository.
    pub(super) fn finish(&mut self, f: Finish) {
        let ai = f.turn.is_some() || f.message.as_ref().is_some_and(|e| e.m.via == WorkMode::Ai);
        let path = self.pane_ref(f.pid).and_then(|p| p.current_path());
        let repo = path.filter(|_| ai).and_then(|p| git::find(&p));
        let Some(repo) = repo else { return self.finish_now(f, None) };
        let base = self.find_pane_mut(f.pid).and_then(|p| p.git_base.take());
        let command = self.git_command();
        self.git.next += 1;
        let token = self.git.next;
        let tx = self.events.clone();
        let measured = std::thread::Builder::new().name("git-work".into()).spawn(move || {
            let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                git::work(command.as_deref(), &repo, base.as_deref())
            }))
            .ok();
            let _ = tx.send(Event::GitWork(token, work.map(Box::new)));
        });
        if measured.is_err() {
            return self.finish_now(f, None);
        }
        self.git.finishing.insert(token, f);
    }

    /// What an agent's work did was measured (nothing, when that failed).
    pub(super) fn git_work_measured(&mut self, token: u64, work: Option<Box<git::Work>>) {
        if let Some(f) = self.git.finishing.remove(&token) {
            self.finish_now(f, work.map(|w| *w));
        }
    }

    fn finish_now(&mut self, f: Finish, work: Option<git::Work>) {
        let pid = f.pid;
        if let Some(text) = f.turn {
            let ended = super::done::Ended { git: work.clone(), ..Default::default() };
            self.pane_done(pid, super::done::Kind::Agent, text, ended);
        }
        if let Some(MessageEnd { m, end, ok, output }) = f.message {
            self.observe.ended_with(m.id, end, ok, output, work.as_ref());
            self.task_done(pid, &m, end, ok, work);
        }
        self.git_moved(pid);
    }
}
