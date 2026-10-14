//! `clone` and `push` (docs/design/link-push.md): the commits of this
//! machine's repository sent to a paired machine over the link, in a bundle
//! cut into pieces (a link request's body is text, about 1 MiB at most).
//!
//! This end: one task does it all (asks what is there, makes the bundle on
//! a thread, sends its pieces, asks for it to be taken in), signing each
//! request as `link_out` does, and its end comes back as an event. Where a
//! repository went is kept (`link::Pushed`), by the other machine's key.
//!
//! The other end: three requests, each from a machine allowed to
//! (`link-allow --push`): a look at a path, a piece of a bundle (kept in
//! the link directory's `incoming`), the bundle taken in. git runs on a
//! thread; its answer comes back as an event and goes to the request.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::actor::Sender;
use super::link_state::js;
use super::{ClientId, Event, Outcome, Server};
use crate::command::Cmd;
use crate::gitsync::{self, Bundled, Look};
use crate::link::{self, Answer, Identity, Pushed};

/// How long the other side may take to look (git reads its branches).
const LOOK_LONGEST: Duration = Duration::from_secs(30);
/// A piece on its way.
const CHUNK_LONGEST: Duration = Duration::from_secs(30);
/// The bundle taken in (git fetches, checks out): beyond git's own limit.
const APPLY_LONGEST: Duration = Duration::from_secs(200);
/// A bundle arriving that has not moved for this long is let go.
const INCOMING_STALE: Duration = Duration::from_secs(600);

/// A bundle arriving from another machine.
pub(super) struct Incoming {
    /// The machine sending it.
    key: String,
    file: PathBuf,
    size: u64,
    next: u64,
    at: Instant,
}

/// The end of a `clone` or `push`, for the client that asked.
pub(super) struct GitDone {
    cid: Option<ClientId>,
    result: Result<String, String>,
    record: Option<Pushed>,
    /// A message for a pane there once the commits are in: its machine, the
    /// pane, the text.
    tell: Option<(String, String, String)>,
}

/// An answer to another machine's request, ready once git is done.
pub(super) struct Later {
    cid: Option<ClientId>,
    nonce: String,
    status: u16,
    body: String,
    /// Said on the status line and kept in the event log: what came in.
    note: Option<(String, String)>,
}

/// Everything a `clone` or `push` needs, so it runs without the server.
struct Job {
    id: Identity,
    from: String,
    port: u16,
    /// The other machine's key and address.
    to: String,
    addr: String,
    git: PathBuf,
    root: PathBuf,
    repo_key: String,
    branch: String,
    clone: bool,
    path: String,
    name: String,
    check_roots: bool,
    limit: u64,
    host: String,
    transfer: String,
    bundle: PathBuf,
}

/// A transfer's name as this side makes it: base64url, 16 to 64 long.
fn transfer_ok(t: &str) -> bool {
    (16..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A path to keep in the records: one line, no tab.
fn keepable(p: &str) -> bool {
    !p.is_empty() && !p.contains(['\t', '\n', '\r'])
}

impl Job {
    /// One request to the other machine, signed; its answer when it is
    /// signed by that machine and says 200.
    async fn call(&self, path: &'static str, body: Vec<u8>, longest: Duration) -> Result<Answer, String> {
        let nonce = link::nonce().map_err(|e| format!("link: {e:#}"))?;
        let time = link::now();
        let text = link::request_text("POST", path, &self.from, self.port, &self.to, time, &nonce, &body);
        let mut headers = super::link_out::common_headers(&self.from, self.port, time, &nonce);
        headers.push((link::H_SIGN, self.id.sign(&text)));
        let result = link::call(&self.addr, "POST", path, &headers, &body, longest).await.map_err(|e| format!("{e:#}"));
        let ans = Server::checked(&self.addr, &self.to, &nonce, result)?;
        match ans.status {
            200 => Ok(ans),
            404 if ans.text() == "not a keepane link path" => Err(format!(
                "the keepane at {} is older than this one and takes no commits: update it there",
                self.addr
            )),
            _ => Err(format!("{} said: {}", self.addr, ans.text())),
        }
    }

    /// The whole of it: what it ended with, and where the repository went.
    async fn run(&self) -> (Result<String, String>, Option<Pushed>) {
        let result = self.steps().await;
        let _ = std::fs::remove_file(&self.bundle);
        match result {
            Ok((text, path)) => {
                let record =
                    keepable(&path).then(|| Pushed { repo: self.repo_key.clone(), key: self.to.clone(), path });
                (Ok(text), record)
            }
            Err(e) => (Err(e), None),
        }
    }

    async fn steps(&self) -> Result<(String, String), String> {
        let (addr, branch) = (&self.addr, &self.branch);
        let mode = if self.clone { "clone" } else { "push" };
        let body = serde_json::json!({ "mode": mode, "path": self.path, "name": self.name, "branch": branch });
        let ans = self.call("/link/git/look", body.to_string().into_bytes(), LOOK_LONGEST).await?;
        let look: Look = serde_json::from_slice(&ans.body)
            .map_err(|_| format!("{addr} answered with something that is not keepane"))?;
        let there = format!("{addr}:{}", look.path);
        if self.clone {
            if look.repo {
                return Err(format!("{there} is a repository already: keepane push {addr} {}", shell_word(&look.path)));
            }
            if !look.free {
                return Err(format!("{there} is not empty: name another path"));
            }
        } else {
            if !look.repo {
                return Err(format!("no repository at {there}: keepane clone {addr} {}", shell_word(&look.path)));
            }
            if self.check_roots && !look.roots.is_empty() {
                let (git, root, b) = (self.git.clone(), self.root.clone(), branch.clone());
                let mine =
                    tokio::task::spawn_blocking(move || gitsync::roots(&git, &root, &b)).await.unwrap_or_default();
                if !mine.iter().any(|r| look.roots.contains(r)) {
                    return Err(format!(
                        "{there} is another project: it shares no first commit with {branch} here (nothing sent)"
                    ));
                }
            }
        }
        let theirs = if self.clone { Vec::new() } else { look.tips.clone() };
        let (git, root, b, out) = (self.git.clone(), self.root.clone(), branch.clone(), self.bundle.clone());
        let made = tokio::task::spawn_blocking(move || {
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            gitsync::bundle(&git, &root, &b, &theirs, &out)
        })
        .await
        .map_err(|e| format!("the bundle: {e}"))??;
        let size = match made {
            Bundled::Nothing => return Ok((format!("{there}: {branch} already up to date"), look.path)),
            Bundled::Made { size, .. } if size > self.limit => {
                return Err(format!(
                    "the commits make {} here, over link-push-limit ({}): nothing sent",
                    human(size),
                    human(self.limit)
                ));
            }
            Bundled::Made { size, .. } => size,
        };
        let mut file = Some(std::fs::File::open(&self.bundle).map_err(|e| format!("the bundle: {e}"))?);
        let mut digest = Sha256::new();
        let mut index = 0u64;
        loop {
            // Read off this task's thread: a piece at a time.
            let f = file.take().expect("put back each time");
            let (f, piece) = tokio::task::spawn_blocking(move || {
                let mut f = f;
                let piece = read_piece(&mut f);
                (f, piece)
            })
            .await
            .map_err(|e| format!("the bundle: {e}"))?;
            file = Some(f);
            let piece = piece.map_err(|e| format!("the bundle: {e}"))?;
            if piece.is_empty() {
                break;
            }
            digest.update(&piece);
            let body = serde_json::json!({ "transfer": self.transfer, "index": index, "data": link::b64(&piece) });
            self.call("/link/git/chunk", body.to_string().into_bytes(), CHUNK_LONGEST).await?;
            index += 1;
        }
        let sha: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let body = serde_json::json!({
            "transfer": self.transfer,
            "size": size,
            "sha256": sha,
            "mode": mode,
            "path": look.path,
            "branch": branch,
            "from": self.host,
        });
        let ans = self.call("/link/git/apply", body.to_string().into_bytes(), APPLY_LONGEST).await?;
        Ok((format!("{addr}:{}", ans.text()), look.path))
    }
}

/// The next piece of a bundle: up to `CHUNK` bytes, none at its end.
fn read_piece(f: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; gitsync::CHUNK];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            got => n += got,
        }
    }
    buf.truncate(n);
    Ok(buf)
}

/// A `clone` or `push` run to its end: what to say, where it went, and the
/// message for a pane there.
async fn finish(
    job: Job,
    tell: Option<String>,
) -> (Result<String, String>, Option<Pushed>, Option<(String, String, String)>) {
    let (mut result, record) = job.run().await;
    let (git, root) = (job.git.clone(), job.root.clone());
    let left = tokio::task::spawn_blocking(move || uncommitted(&git, &root)).await.unwrap_or(0);
    if let Ok(text) = &mut result
        && left > 0
    {
        text.push_str(&format!("\nnote: {left} file(s) here are not committed, and were not sent"));
    }
    let tell = match (&result, tell) {
        (Ok(text), Some(pane)) => {
            let first = text.lines().next().unwrap_or_default().to_string();
            Some((job.addr.clone(), pane, format!("commits from {}: {first}", job.host)))
        }
        _ => None,
    };
    (result, record, tell)
}

/// A path to put in a command to type: in single quotes when it has a space
/// (PowerShell and POSIX shells read them alike), as it is otherwise.
fn shell_word(p: &str) -> String {
    if p.contains(char::is_whitespace) && !p.contains('\'') { format!("'{p}'") } else { p.to_string() }
}

/// A size as people read it.
fn human(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KiB", n.div_ceil(1024))
    }
}

/// Files in the working tree not committed (changed or new), counted.
fn uncommitted(git: &Path, root: &Path) -> usize {
    gitsync::git(git, root, &["status", "--porcelain"], crate::git::LIMIT).map(|s| s.lines().count()).unwrap_or(0)
}

impl Server {
    /// `link-clone`, `link-push`: from here.
    pub(super) fn exec_link_git(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Outcome {
        match self.link_git_start(cmd, cid) {
            Ok(Some(text)) => Outcome::Text(text),
            Ok(None) => Outcome::Pending,
            Err(e) => Outcome::Error(e),
        }
    }

    fn link_git_start(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Result<Option<String>, String> {
        let (clone, addr, path, tell, list) = match cmd {
            Cmd::LinkClone { addr, path, tell } => (true, Some(addr), path, tell, false),
            Cmd::LinkPush { addr, path, tell, list } => (false, addr, path, tell, list),
            other => return Err(format!("not a clone or push: {other}")),
        };
        let n = if clone { "clone" } else { "push" };
        let here = self.client_cwd(cid).unwrap_or_default();
        // (Loaded first: where it is all kept is decided then.)
        self.link()?;
        let dir = self.link_dir();
        if list {
            return Ok(Some(self.pushes_listed(&dir, crate::git::find(&here).map(|r| r.key()))));
        }
        let repo =
            crate::git::find(&here).ok_or_else(|| format!("{n}: {here} is in no git repository: run it in one"))?;
        let git = crate::git::command(&self.opts.git_command)
            .ok_or_else(|| format!("{n}: no git here (git-command, or git on the PATH)"))?;
        let branch = repo
            .on_branch()
            .filter(|b| gitsync::branch_ok(b))
            .ok_or_else(|| format!("{n}: the head is on no branch here (detached): check out the branch to send"))?;
        let repo_key = repo.key().to_string_lossy().into_owned();
        // Where it goes: named, or as kept.
        let (key, addr, path, check_roots) = if clone {
            let addr = addr.expect("the parser asks for it");
            (self.peer_key(&addr)?, addr, path.unwrap_or_default(), false)
        } else {
            match (addr, path) {
                (Some(addr), Some(path)) => (self.peer_key(&addr)?, addr, path, true),
                (Some(addr), None) => {
                    let key = self.peer_key(&addr)?;
                    let kept = link::load_pushes(&dir).into_iter().find(|p| p.repo == repo_key && p.key == key);
                    let p = kept.ok_or_else(|| {
                        format!("push: this repository was not cloned or pushed to {addr}: keepane clone {addr}, or keepane push {addr} <path there>")
                    })?;
                    (key, addr, p.path, false)
                }
                (None, _) => {
                    let mine: Vec<Pushed> =
                        link::load_pushes(&dir).into_iter().filter(|p| p.repo == repo_key).collect();
                    let l = self.link()?;
                    match mine.as_slice() {
                        [] => {
                            return Err(
                                "push: this repository was not cloned or pushed anywhere: keepane clone <host:port> first"
                                    .into(),
                            );
                        }
                        [p] => {
                            let peer = l.peer_by_key(&p.key).ok_or_else(|| {
                                "push: the machine this repository went to is no longer paired (`keepane link list`)"
                                    .to_string()
                            })?;
                            (p.key.clone(), peer.addr.clone(), p.path.clone(), false)
                        }
                        many => {
                            let places: Vec<String> = many
                                .iter()
                                .map(|p| {
                                    let at = l.peer_by_key(&p.key).map_or("(unpaired)".to_string(), |q| q.addr.clone());
                                    format!("  keepane push {at}   ({at}:{})", p.path)
                                })
                                .collect();
                            return Err(format!(
                                "push: this repository goes to {} machines; name one:\n{}",
                                many.len(),
                                places.join("\n")
                            ));
                        }
                    }
                }
            }
        };
        let (port, _) = self.own_web()?;
        let transfer = link::nonce().map_err(|e| format!("link: {e:#}"))?;
        let l = self.link()?;
        let job = Job {
            id: l.id.clone(),
            from: l.id.public(),
            port,
            to: key,
            addr: addr.clone(),
            git,
            root: repo.root.clone(),
            repo_key,
            branch,
            clone,
            path,
            name: repo.root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            check_roots,
            limit: self.opts.link_push_limit,
            host: crate::sysinfo::hostname(),
            bundle: dir.join("outgoing").join(format!("{transfer}.bundle")),
            transfer,
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            // Run apart, so that whatever happens in it the client is answered.
            let done = tokio::spawn(finish(job, tell)).await;
            let (result, record, tell) = done.unwrap_or_else(|e| (Err(format!("{n}: {e}")), None, None));
            let _ = events.send(Event::LinkGitDone(Box::new(GitDone { cid, result, record, tell })));
        });
        Ok(None)
    }

    /// From the tick: bundles that stopped arriving are let go.
    pub(super) fn link_git_sweep(&mut self) {
        let Some(l) = self.link.as_mut() else { return };
        l.incoming.retain(|_, t| {
            let keep = t.at.elapsed() < INCOMING_STALE;
            if !keep {
                let _ = std::fs::remove_file(&t.file);
            }
            keep
        });
    }

    /// `push -l`: where this machine's repositories go (this one's first).
    fn pushes_listed(&mut self, dir: &Path, this: Option<PathBuf>) -> String {
        let all = link::load_pushes(dir);
        if all.is_empty() {
            return "no repository here was cloned or pushed to another machine (keepane clone <host:port>)".into();
        }
        let this = this.map(|p| p.to_string_lossy().into_owned());
        let addr_of = |l: &super::link_state::LinkHost, key: &str| {
            l.peer_by_key(key).map_or("(no longer paired)".to_string(), |p| p.addr.clone())
        };
        let Ok(l) = self.link() else { return String::new() };
        let mut rows = vec![["", "REPOSITORY HERE", "MACHINE", "PATH THERE"].map(String::from).to_vec()];
        let mut sorted = all.clone();
        sorted.sort_by_key(|p| (Some(&p.repo) != this.as_ref(), p.repo.clone()));
        for p in &sorted {
            let mark = if Some(&p.repo) == this.as_ref() { "*" } else { "" };
            rows.push(vec![mark.into(), p.repo.clone(), addr_of(l, &p.key), p.path.clone()]);
        }
        super::align_columns(&rows).join("\n")
    }

    /// A `clone` or `push` ended: kept where it went, said to who asked, and
    /// the pane there told when asked to.
    pub(super) fn link_git_done(&mut self, d: GitDone) {
        let GitDone { cid, result, record, tell } = d;
        let mut out = match result {
            Ok(t) => Outcome::Text(t),
            Err(e) => Outcome::Error(e),
        };
        if let Some(r) = record
            && let Err(e) = link::save_push(&self.link_dir(), r)
            && let Outcome::Text(t) = &mut out
        {
            t.push_str(&format!("\nnote: where it went was not kept ({e:#})"));
        }
        if let Some((addr, pane, text)) = tell
            && let Outcome::Error(e) = self.send_remote(None, &addr, &pane, text, Sender::User, None, None, None)
            && let Outcome::Text(t) = &mut out
        {
            t.push_str(&format!("\nnote: {addr}/{pane} was not told ({e})"));
        }
        match cid {
            Some(cid) => self.reply(cid, out),
            None => {
                if let Outcome::Error(e) = out {
                    self.note_message(&format!("push: {e}"));
                }
            }
        }
    }

    /// An answer to another machine's request, git done.
    pub(super) fn link_later(&mut self, l: Later) {
        let Later { cid, nonce, status, body, note } = l;
        if let Some((text, fields)) = note {
            self.link_note(&text, "commits", &fields);
        }
        let out = self.link_answer(&nonce, status, body);
        if let Some(cid) = cid {
            self.reply(cid, out);
        }
    }

    /// The machine may send commits here, or the refusal to answer with.
    fn may_push(&mut self, nonce: &str, peer: &link::Peer) -> Option<Outcome> {
        if peer.push {
            return None;
        }
        let why = format!(
            "{} may not send commits here: on this machine, keepane link allow {} --push",
            peer.addr, peer.addr
        );
        self.link_refused(&peer.addr, &why);
        Some(self.link_answer(nonce, 403, why))
    }

    /// The path here a request names (absolute), or why not.
    fn git_place(&self, asked: &str, name: &str) -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or("this machine has no home directory to put it under")?;
        let default_dir = PathBuf::from(super::expand_home(&self.opts.link_clone_dir));
        let p = gitsync::resolve_path(asked, name, &default_dir, &home)
            .ok_or_else(|| format!("'{name}' is no name for a directory: name the path"))?;
        if !p.is_absolute() {
            return Err(format!(
                "{} is not a full path (link-clone-dir here: {})",
                p.display(),
                self.opts.link_clone_dir
            ));
        }
        if p.starts_with(self.link_dir()) {
            return Err("not inside keepane's own directory".into());
        }
        Ok(p)
    }

    /// Run `work` on a thread; its answer goes to the request (`Later`).
    fn later(
        &self,
        cid: Option<ClientId>,
        nonce: &str,
        work: impl FnOnce() -> (u16, String, Option<(String, String)>) + Send + 'static,
    ) {
        let (events, nonce) = (self.events.clone(), nonce.to_string());
        std::thread::spawn(move || {
            let (status, body, note) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or((
                500,
                "git failed here".into(),
                None,
            ));
            let _ = events.send(Event::LinkLater(Box::new(Later { cid, nonce, status, body, note })));
        });
    }

    /// `POST /link/git/look`: what is at a path here.
    pub(super) fn link_git_look_in(
        &mut self,
        cid: Option<ClientId>,
        body: &str,
        nonce: &str,
        peer: &link::Peer,
    ) -> Outcome {
        if let Some(no) = self.may_push(nonce, peer) {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let branch = v["branch"].as_str().unwrap_or_default().to_string();
        if !gitsync::branch_ok(&branch) {
            return self.link_answer(nonce, 400, format!("branch: '{branch}' is not a branch name"));
        }
        let path = match self.git_place(v["path"].as_str().unwrap_or_default(), v["name"].as_str().unwrap_or_default())
        {
            Ok(p) => p,
            Err(e) => return self.link_answer(nonce, 400, e),
        };
        let Some(git) = crate::git::command(&self.opts.git_command) else {
            return self.link_answer(nonce, 500, "there is no git on this machine (git-command)".into());
        };
        self.later(cid, nonce, move || {
            let l = gitsync::look(&git, &path, &branch);
            (200, serde_json::to_string(&l).expect("a look serializes"), None)
        });
        Outcome::Pending
    }

    /// `POST /link/git/chunk`: a piece of a bundle, in order, kept here.
    pub(super) fn link_git_chunk_in(&mut self, body: &str, nonce: &str, peer: &link::Peer) -> Outcome {
        if let Some(no) = self.may_push(nonce, peer) {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let transfer = v["transfer"].as_str().unwrap_or_default().to_string();
        let (Some(index), Some(data)) = (v["index"].as_u64(), v["data"].as_str().and_then(link::unb64)) else {
            return self.link_answer(nonce, 400, "a piece: transfer, index, data".into());
        };
        if !transfer_ok(&transfer) {
            return self.link_answer(nonce, 400, "transfer: not a name keepane makes".into());
        }
        let limit = self.opts.link_push_limit;
        let dir = self.link_dir().join("incoming");
        let (status, said) = match self.link() {
            Ok(l) => take_piece(&mut l.incoming, &dir, &transfer, index, &data, &peer.key, limit),
            Err(e) => return Outcome::Error(e),
        };
        self.link_answer(nonce, status, said)
    }

    /// `POST /link/git/apply`: the bundle that came, checked whole, taken
    /// into a new repository or the one at the path.
    pub(super) fn link_git_apply_in(
        &mut self,
        cid: Option<ClientId>,
        body: &str,
        nonce: &str,
        peer: &link::Peer,
    ) -> Outcome {
        if let Some(no) = self.may_push(nonce, peer) {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let (transfer, sha, mode, path, branch, from) =
            (s("transfer"), s("sha256"), s("mode"), s("path"), s("branch"), s("from"));
        let size = v["size"].as_u64().unwrap_or(0);
        let clone = match mode.as_str() {
            "clone" => true,
            "push" => false,
            _ => return self.link_answer(nonce, 400, "mode: clone or push".into()),
        };
        if !gitsync::branch_ok(&branch) {
            return self.link_answer(nonce, 400, format!("branch: '{branch}' is not a branch name"));
        }
        let path = match self.git_place(&path, "") {
            Ok(p) => p,
            Err(e) => return self.link_answer(nonce, 400, e),
        };
        let taken = self.link().ok().and_then(|l| {
            let mine = l.incoming.get(&transfer).is_some_and(|t| t.key == peer.key);
            if mine { l.incoming.remove(&transfer) } else { None }
        });
        let Some(t) = taken else {
            return self.link_answer(nonce, 404, format!("no bundle {transfer} came here"));
        };
        if t.size != size {
            let _ = std::fs::remove_file(&t.file);
            return self.link_answer(nonce, 400, format!("the bundle came short: {} of {size} bytes", t.size));
        }
        let Some(git) = crate::git::command(&self.opts.git_command) else {
            let _ = std::fs::remove_file(&t.file);
            return self.link_answer(nonce, 500, "there is no git on this machine (git-command)".into());
        };
        let (remote, url, addr, key) =
            (gitsync::remote_name(&from), format!("keepane:{}", peer.addr), peer.addr.clone(), peer.key.clone());
        self.later(cid, nonce, move || {
            let r = take_in(&git, &t.file, &sha, clone, &path, &branch, &remote, &url);
            let _ = std::fs::remove_file(&t.file);
            match r {
                Ok(text) => {
                    let fields = format!(
                        ",\"addr\":{},\"key\":{},\"mode\":{},\"path\":{},\"branch\":{},\"said\":{}",
                        js(&addr),
                        js(&key),
                        js(if clone { "clone" } else { "push" }),
                        js(&path.to_string_lossy()),
                        js(&branch),
                        js(&text)
                    );
                    (200, text.clone(), Some((format!("link: commits from {addr}: {text}"), fields)))
                }
                Err(e) => (409, e, None),
            }
        });
        Outcome::Pending
    }
}

/// A piece of a bundle from the machine `key`, kept in `dir`: the answer's
/// status and words. What has stopped coming is let go first; a machine
/// sends one bundle at a time (its first piece drops any other of its).
fn take_piece(
    incoming: &mut std::collections::HashMap<String, Incoming>,
    dir: &Path,
    transfer: &str,
    index: u64,
    data: &[u8],
    key: &str,
    limit: u64,
) -> (u16, String) {
    incoming.retain(|name, t| {
        let keep = t.at.elapsed() < INCOMING_STALE && !(index == 0 && t.key == key && name != transfer);
        if !keep {
            let _ = std::fs::remove_file(&t.file);
        }
        keep
    });
    if index == 0 {
        let file = dir.join(format!("{transfer}.bundle"));
        if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(&file)) {
            return (500, format!("{}: {e}", file.display()));
        }
        incoming.insert(
            transfer.to_string(),
            Incoming { key: key.to_string(), file, size: 0, next: 0, at: Instant::now() },
        );
    }
    let Some(t) = incoming.get_mut(transfer).filter(|t| t.key == key) else {
        return (404, format!("no bundle {transfer} on its way here"));
    };
    if index != t.next {
        return (400, format!("piece {index} out of order ({} expected)", t.next));
    }
    if t.size + data.len() as u64 > limit {
        let _ = std::fs::remove_file(&t.file);
        incoming.remove(transfer);
        return (413, format!("over link-push-limit here ({})", human(limit)));
    }
    let wrote = std::fs::OpenOptions::new()
        .append(true)
        .open(&t.file)
        .and_then(|mut f| std::io::Write::write_all(&mut f, data));
    if let Err(e) = wrote {
        return (500, format!("{}: {e}", t.file.display()));
    }
    (t.size, t.next, t.at) = (t.size + data.len() as u64, t.next + 1, Instant::now());
    (200, "ok".into())
}

/// The bundle at `file` checked against its digest, then taken in.
#[allow(clippy::too_many_arguments)]
fn take_in(
    git: &Path,
    file: &Path,
    sha: &str,
    clone: bool,
    path: &Path,
    branch: &str,
    remote: &str,
    url: &str,
) -> Result<String, String> {
    let mut f = std::fs::File::open(file).map_err(|e| format!("the bundle: {e}"))?;
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("the bundle: {e}"))?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    let got: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != sha {
        return Err("the bundle that came is not the one sent (its SHA-256 differs): nothing taken".into());
    }
    drop(f);
    let shown = path.to_string_lossy().into_owned();
    if clone {
        if !gitsync::look(git, path, branch).free {
            return Err(format!("{shown} is not empty any more: nothing taken"));
        }
        gitsync::clone(git, file, path, branch, remote, url)
    } else {
        if !gitsync::look(git, path, branch).repo {
            return Err(format!("no repository at {shown} any more: nothing taken"));
        }
        let (landed, came) = gitsync::take(git, file, path, branch, remote)?;
        Ok(gitsync::said(&shown, branch, remote, &landed, came))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pieces kept in order, one bundle a machine at a time, within the
    /// limit; what stopped coming is let go (its file too).
    #[test]
    fn pieces_arrive_in_order_and_within_the_limit() {
        let dir = std::env::temp_dir().join(format!("keepane-pieces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = std::collections::HashMap::new();
        let (t1, t2) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        assert_eq!(take_piece(&mut m, &dir, t1, 0, b"abc", "k1", 10).0, 200);
        assert_eq!(take_piece(&mut m, &dir, t1, 2, b"x", "k1", 10), (400, "piece 2 out of order (1 expected)".into()));
        assert_eq!(take_piece(&mut m, &dir, t1, 1, b"def", "k2", 10).0, 404, "another machine's");
        assert_eq!(take_piece(&mut m, &dir, t1, 1, b"def", "k1", 10).0, 200);
        assert_eq!(std::fs::read(dir.join(format!("{t1}.bundle"))).unwrap(), b"abcdef");
        assert_eq!(take_piece(&mut m, &dir, t1, 2, b"12345", "k1", 10).0, 413);
        assert!(m.is_empty() && !dir.join(format!("{t1}.bundle")).exists(), "over the limit: gone");
        // A new bundle from the same machine drops the one before.
        assert_eq!(take_piece(&mut m, &dir, t1, 0, b"a", "k1", 10).0, 200);
        assert_eq!(take_piece(&mut m, &dir, t2, 0, b"b", "k1", 10).0, 200);
        assert!(!m.contains_key(t1) && !dir.join(format!("{t1}.bundle")).exists());
        // Stale: let go at the next piece.
        m.get_mut(t2).unwrap().at = Instant::now() - INCOMING_STALE - Duration::from_secs(1);
        assert_eq!(take_piece(&mut m, &dir, t2, 1, b"c", "k1", 10).0, 404);
        assert!(!dir.join(format!("{t2}.bundle")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// From the tick: what stopped arriving goes, what is arriving stays.
    #[test]
    fn the_tick_lets_go_of_what_stopped_arriving() {
        let dir = std::env::temp_dir().join(format!("keepane-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "sweep".into(), events);
        // The link state made here, not from where the environment says.
        s.link = Some(super::super::link_state::LinkHost {
            dir: dir.clone(),
            id: Identity::make(&dir.join("key")).unwrap(),
            peers: Vec::new(),
            nonces: Default::default(),
            seen: Default::default(),
            origin: Default::default(),
            refused: Default::default(),
            sent: Default::default(),
            made: Default::default(),
            incoming: Default::default(),
        });
        // Everything of the link from the directory it was loaded from,
        // whatever the environment says now.
        assert_eq!(s.link_dir(), dir);
        let files = dir.join("in");
        std::fs::create_dir_all(&files).unwrap();
        let l = s.link.as_mut().unwrap();
        for (name, age) in [("old", INCOMING_STALE + Duration::from_secs(1)), ("new", Duration::ZERO)] {
            let file = files.join(name);
            std::fs::write(&file, b"x").unwrap();
            let at = Instant::now() - age;
            l.incoming.insert(name.into(), Incoming { key: "k".into(), file, size: 1, next: 1, at });
        }
        s.link_git_sweep();
        let l = s.link.as_ref().unwrap();
        assert!(l.incoming.contains_key("new") && !l.incoming.contains_key("old"));
        assert!(files.join("new").exists() && !files.join("old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bundle whose digest is not the one sent is not taken.
    #[test]
    fn a_bundle_not_the_one_sent_is_not_taken() {
        let Some(git) = crate::git::command("") else {
            eprintln!("no git here: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("keepane-digest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let here = dir.join("here");
        std::fs::create_dir_all(&here).unwrap();
        let g = |args: &[&str]| {
            let ok = std::process::Command::new(&git)
                .arg("-C")
                .arg(&here)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(here.join("a.txt"), "a\n").unwrap();
        g(&["add", "a.txt"]);
        g(&["commit", "-q", "-m", "a"]);
        let b = dir.join("b.bundle");
        gitsync::bundle(&git, &here, "main", &[], &b).unwrap();
        let sha: String = Sha256::digest(std::fs::read(&b).unwrap()).iter().map(|x| format!("{x:02x}")).collect();
        let there = dir.join("there");
        let e = take_in(&git, &b, &"0".repeat(64), true, &there, "main", "laptop", "keepane:x").unwrap_err();
        assert!(e.contains("not the one sent") && !there.exists(), "{e}");
        assert!(take_in(&git, &b, &sha, true, &there, "main", "laptop", "keepane:x").unwrap().contains("cloned"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transfer_names_and_kept_paths() {
        assert!(transfer_ok("abcdefghijklmnop") && transfer_ok("A-b_c0123456789xyz"));
        assert!(!transfer_ok("short") && !transfer_ok("abcdefghijklmnop/") && !transfer_ok(&"a".repeat(65)));
        assert!(keepable("/home/u/keepane/p") && keepable(r"C:\x y\p"));
        assert!(!keepable("") && !keepable("a\tb") && !keepable("a\nb"));
        assert_eq!(
            (shell_word("/a b"), shell_word(r"C:\x"), shell_word("it's x")),
            ("'/a b'".into(), r"C:\x".into(), "it's x".into())
        );
        assert_eq!((human(1536), human(3 * 1024 * 1024)), ("2 KiB".to_string(), "3.0 MiB".to_string()));
    }
}
