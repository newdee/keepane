//! `send-agents` (docs/design/agent-sync.md) over the link: a project and
//! its agents' chats sent to a paired machine, as `push` sends commits.
//!
//! This end: one task does it all. It lists what goes (`agentsync::plan`),
//! asks the other side what it has (`/link/agents/look`), makes the pack
//! (the commits a bundle, the files, the chats), sends it in pieces, and
//! asks for it to be taken in; its end comes back as an event. Where the
//! folder went is kept (`agentsync::Record`).
//!
//! The other end: three requests, each from a machine allowed to
//! (`link-allow --agents`): a look, a piece of the pack (kept in the link
//! directory's `incoming`, as `push`'s bundles), the pack taken in. The
//! folder lands in `<link-clone-dir>/from/<that machine>/<name>`, or
//! where it went before, or where it came from (a folder sent back).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::agentsync::{self, Look, Places, Record};
use super::link_git::{GitDone, human, read_piece, shell_word, take_piece, transfer_ok};
use super::link_state::js;
use super::{ClientId, Event, Outcome, Server};
use crate::command::Cmd;
use crate::link::{self, Answer, Identity};

/// The other side looks: git, and a digest of each file and chat asked.
const LOOK_LONGEST: Duration = Duration::from_secs(120);
const CHUNK_LONGEST: Duration = Duration::from_secs(30);
/// Taken in: git, then every file and chat.
const APPLY_LONGEST: Duration = Duration::from_secs(600);
/// A look's request is one body (about 1 MiB at most).
const LOOK_MAX: usize = 900 * 1024;

/// Everything a `send-agents` needs, so it runs without the server.
struct Job {
    id: Identity,
    from: String,
    port: u16,
    to: String,
    addr: String,
    git: Option<PathBuf>,
    root: PathBuf,
    /// The branch and its commit (a repository).
    repo: Option<(String, String)>,
    places: Places,
    only: Vec<String>,
    dry: bool,
    limit: u64,
    host: String,
    transfer: String,
    dir: PathBuf,
}

impl Job {
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
                "the keepane at {} is older than this one and takes no projects: update it there",
                self.addr
            )),
            _ => Err(format!("{} said: {}", self.addr, ans.text())),
        }
    }

    /// The whole of it: what to say, and the record to keep here.
    async fn run(self) -> Result<(String, Option<Record>), String> {
        let (git, root, repo, places, only) =
            (self.git.clone(), self.root.clone(), self.repo.clone(), self.places.clone(), self.only.clone());
        let plan = tokio::task::spawn_blocking(move || {
            let repo = repo.as_ref().map(|(b, h)| (b.as_str(), h.as_str()));
            agentsync::plan(git.as_deref(), &root, repo, &places, &only)
        })
        .await
        .map_err(|e| format!("send-agents: {e}"))??;
        let (files, chats) = plan.asks();
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let body = serde_json::json!({
            "path": plan.folder,
            "name": name,
            "host": self.host,
            "branch": plan.manifest.branch,
            "files": files,
            "chats": chats,
        })
        .to_string();
        if body.len() > LOOK_MAX {
            return Err(format!(
                "send-agents: {} files and {} chat files are too many to ask about at once: use git (only changed files go), or -s for some chats",
                files.len(),
                chats.len()
            ));
        }
        let ans = self.call("/link/agents/look", body.into_bytes(), LOOK_LONGEST).await?;
        let look: Look = serde_json::from_slice(&ans.body)
            .map_err(|_| format!("{} answered with something that is not keepane", self.addr))?;
        let there = format!("{}:{}", self.addr, look.path);
        if plan.manifest.branch.is_empty() && look.git.repo {
            return Err(format!("{there} is a repository, and this folder is not one: nothing sent"));
        }
        if !plan.manifest.branch.is_empty() && !look.git.repo && !look.git.free {
            return Err(format!("{there} is not empty and not a repository: nothing sent"));
        }
        // The commits it lacks.
        let bundle = self.dir.join("outgoing").join(format!("{}.bundle", self.transfer));
        let pack = self.dir.join("outgoing").join(format!("{}.pack", self.transfer));
        let look2 = look.clone();
        let (git, root, repo, theirs, b) = (
            self.git.clone(),
            self.root.clone(),
            self.repo.clone(),
            if look.git.repo { look.git.tips.clone() } else { Vec::new() },
            bundle.clone(),
        );
        let made = tokio::task::spawn_blocking(move || -> Result<(agentsync::Made, u64), String> {
            let mut commits = 0;
            let with = match (&git, &repo) {
                (Some(git), Some((branch, _))) => {
                    if let Some(d) = b.parent() {
                        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                    }
                    match crate::gitsync::bundle(git, &root, branch, &theirs, &b)? {
                        crate::gitsync::Bundled::Nothing => None,
                        crate::gitsync::Bundled::Made { commits: n, .. } => {
                            commits = n;
                            Some(b.as_path())
                        }
                    }
                }
                _ => None,
            };
            let made = agentsync::make_pack(&plan, &look2, with, &look2.path, &pack);
            let _ = std::fs::remove_file(&b);
            Ok((made?, commits))
        });
        let (made, commits) = made.await.map_err(|e| format!("send-agents: {e}"))??;
        let pack = self.dir.join("outgoing").join(format!("{}.pack", self.transfer));
        let result = self.send(&pack, &made, commits, &files, &look).await;
        let _ = std::fs::remove_file(&pack);
        result
    }

    /// The pack sent and taken in there (or, with `-n`, what would go).
    async fn send(
        &self,
        pack: &Path,
        made: &agentsync::Made,
        commits: u64,
        files: &[agentsync::FileAsk],
        look: &Look,
    ) -> Result<(String, Option<Record>), String> {
        let there = format!("{}:{}", self.addr, look.path);
        let mut what = Vec::new();
        if commits > 0 {
            what.push(format!("{commits} commit(s)"));
        }
        if made.files > 0 {
            what.push(format!("{} file(s)", made.files));
        }
        let chats = made.new.len() + made.grown.len() + made.forked.len();
        if chats > 0 {
            what.push(format!("{chats} chat(s), {}", human(made.chat_bytes)));
        }
        let summary = if made.nothing() || what.is_empty() { "nothing new".to_string() } else { what.join(", ") };
        if self.dry {
            let mut lines = vec![format!("would send to {there}: {summary} ({} packed)", human(made.size))];
            for (label, list) in [("new", &made.new), ("grown", &made.grown), ("as a new chat", &made.forked)] {
                for (agent, id) in list {
                    lines.push(format!("  {} {id} ({label})", agent.word()));
                }
            }
            return Ok((lines.join("\n"), None));
        }
        if made.size > self.limit {
            return Err(format!(
                "the pack is {}, over link-agents-limit ({}): nothing sent; -s sends some chats only",
                human(made.size),
                human(self.limit)
            ));
        }
        let mut file = Some(std::fs::File::open(pack).map_err(|e| format!("the pack: {e}"))?);
        let mut digest = Sha256::new();
        let mut index = 0u64;
        loop {
            let f = file.take().expect("put back each time");
            let (f, piece) = tokio::task::spawn_blocking(move || {
                let mut f = f;
                let piece = read_piece(&mut f);
                (f, piece)
            })
            .await
            .map_err(|e| format!("the pack: {e}"))?;
            file = Some(f);
            let piece = piece.map_err(|e| format!("the pack: {e}"))?;
            if piece.is_empty() {
                break;
            }
            digest.update(&piece);
            let body = serde_json::json!({ "transfer": self.transfer, "index": index, "data": link::b64(&piece) });
            self.call("/link/agents/chunk", body.to_string().into_bytes(), CHUNK_LONGEST).await?;
            index += 1;
        }
        let sha: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let body = serde_json::json!({
            "transfer": self.transfer,
            "size": made.size,
            "sha256": sha,
            "path": look.path,
            "there": self.root.to_string_lossy(),
            "host": self.host,
        });
        let ans = self.call("/link/agents/apply", body.to_string().into_bytes(), APPLY_LONGEST).await?;
        let mut lines = vec![format!("sent to {there}: {summary} ({} packed)", human(made.size))];
        lines.extend(ans.text().lines().map(|l| format!("  {l}")));
        let ids: Vec<&(agentsync::Agent, String)> = made.new.iter().chain(&made.grown).collect();
        if !ids.is_empty() {
            lines.push(format!("go on there: cd {}", shell_word(&look.path)));
            for (agent, id) in ids.iter().take(10) {
                lines.push(format!("  {}", agent.resume(id)));
            }
        }
        let record = Record {
            here: self.root.to_string_lossy().into_owned(),
            key: self.to.clone(),
            there: look.path.clone(),
            host: self.addr.clone(),
            base: files.iter().map(|f| (f.p.clone(), f.s.clone())).collect(),
            forks: Default::default(),
            at: chrono::Local::now().to_rfc3339(),
        };
        Ok((lines.join("\n"), Some(record)))
    }
}

impl Server {
    /// `send-agents`: from here.
    pub(super) fn exec_link_agents(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Outcome {
        match self.link_agents_start(cmd, cid) {
            Ok(Some(text)) => Outcome::Text(text),
            Ok(None) => Outcome::Pending,
            Err(e) => Outcome::Error(e),
        }
    }

    fn link_agents_start(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Result<Option<String>, String> {
        let Cmd::LinkSendAgents { addr, dir, sessions, dry, list } = cmd else {
            return Err(format!("not a send-agents: {cmd}"));
        };
        self.link()?;
        let link_dir = self.link_dir();
        if list {
            return Ok(Some(self.agents_listed(&link_dir)));
        }
        let here = self.client_cwd(cid).unwrap_or_default();
        let asked = match dir {
            Some(d) => {
                let p = PathBuf::from(super::expand_home(&d));
                if p.is_absolute() { p } else { PathBuf::from(&here).join(p) }
            }
            None => PathBuf::from(&here),
        };
        if !asked.is_dir() {
            return Err(format!("send-agents: {} is not a folder", asked.display()));
        }
        let git = crate::git::command(&self.opts.git_command);
        // A repository's root (its commits go), else the folder itself.
        let found = crate::git::find(&asked.to_string_lossy());
        let (root, repo) = match &found {
            Some(r) => {
                let git = git.as_deref().ok_or("send-agents: no git here (git-command, or git on the PATH)")?;
                let branch = r
                    .on_branch()
                    .filter(|b| crate::gitsync::branch_ok(b))
                    .ok_or("send-agents: the head is on no branch here (detached): check out the branch to send")?;
                let head = crate::gitsync::git(git, &r.root, &["rev-parse", "HEAD"], crate::git::LIMIT)
                    .map_err(|_| format!("send-agents: {branch} has no commit yet: commit first"))?;
                (crate::git::plain(r.root.clone()), Some((branch, head.trim().to_string())))
            }
            None => (crate::git::plain(crate::git::real(&asked)), None),
        };
        let root_s = root.to_string_lossy().into_owned();
        // Where it goes: named, or where this folder went before.
        let (key, addr) = match addr {
            Some(addr) => (self.peer_key(&addr)?, addr),
            None => {
                let mine: Vec<Record> =
                    agentsync::load_records(&link_dir).into_iter().filter(|r| r.here == root_s).collect();
                let l = self.link()?;
                match mine.as_slice() {
                    [] => {
                        return Err(format!(
                            "send-agents: {root_s} went to no machine yet: keepane send-agents <host:port>"
                        ));
                    }
                    [r] => {
                        let peer = l.peer_by_key(&r.key).ok_or(
                            "send-agents: the machine this folder went to is no longer paired (`keepane link list`)",
                        )?;
                        (r.key.clone(), peer.addr.clone())
                    }
                    many => {
                        let at: Vec<String> = many
                            .iter()
                            .map(|r| {
                                let a = l.peer_by_key(&r.key).map_or("(unpaired)".to_string(), |p| p.addr.clone());
                                format!("  keepane send-agents {a}   ({a}:{})", r.there)
                            })
                            .collect();
                        return Err(format!(
                            "send-agents: this folder went to {} machines; name one:\n{}",
                            many.len(),
                            at.join("\n")
                        ));
                    }
                }
            }
        };
        let (port, _) = self.own_web()?;
        let transfer = link::nonce().map_err(|e| format!("link: {e:#}"))?;
        let home = dirs::home_dir().unwrap_or_default();
        let l = self.link()?;
        let job = Job {
            id: l.id.clone(),
            from: l.id.public(),
            port,
            to: key,
            addr,
            git,
            root,
            repo,
            places: Places::of(&home, &|k| std::env::var(k).ok()),
            only: sessions,
            dry,
            limit: self.opts.link_agents_limit,
            host: crate::sysinfo::hostname(),
            transfer,
            dir: link_dir.clone(),
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            let done = tokio::spawn(job.run()).await;
            let result = match done {
                Ok(Ok((text, record))) => match record.map(|r| agentsync::save_record(&link_dir, r)) {
                    Some(Err(e)) => Ok(format!("{text}\nnote: where it went was not kept ({e})")),
                    _ => Ok(text),
                },
                Ok(Err(e)) => Err(e),
                Err(e) => Err(format!("send-agents: {e}")),
            };
            let _ = events.send(Event::LinkGitDone(Box::new(GitDone::said(cid, result))));
        });
        Ok(None)
    }

    /// `send-agents -l`: the folders sent and taken in, with whom.
    fn agents_listed(&mut self, dir: &Path) -> String {
        let all = agentsync::load_records(dir);
        if all.is_empty() {
            return "no folder here was sent to or came from another machine (keepane send-agents <host:port>)".into();
        }
        let Ok(l) = self.link() else { return String::new() };
        let mut rows = vec![["FOLDER HERE", "MACHINE", "FOLDER THERE", "LAST"].map(String::from).to_vec()];
        for r in &all {
            let at = l.peer_by_key(&r.key).map_or("(no longer paired)".to_string(), |p| p.addr.clone());
            let last = chrono::DateTime::parse_from_rfc3339(&r.at)
                .map_or(String::new(), |t| t.format("%Y-%m-%d %H:%M").to_string());
            rows.push(vec![r.here.clone(), at, r.there.clone(), last]);
        }
        super::align_columns(&rows).join("\n")
    }

    /// The machine may send projects here, or the refusal to answer with.
    fn may_send_agents(&mut self, nonce: &str, peer: &link::Peer) -> Option<Outcome> {
        if peer.agents {
            return None;
        }
        let why = format!(
            "{} may not send projects and chats here: on this machine, keepane link allow {} --agents",
            peer.addr, peer.addr
        );
        self.link_refused(&peer.addr, &why);
        Some(self.link_answer(nonce, 403, why))
    }

    /// Where a folder from `key` (its path there `there`, named `name`, from
    /// the machine called `host`) lands here: where it went before, else a
    /// new folder under `from/<host>`; and whether it was known.
    fn agents_place(
        &self,
        key: &str,
        there: &str,
        host: &str,
        name: &str,
    ) -> Result<(PathBuf, Option<Record>), String> {
        if let Some(r) =
            agentsync::load_records(&self.link_dir()).into_iter().find(|r| r.key == key && r.there == there)
        {
            return Ok((PathBuf::from(&r.here), Some(r)));
        }
        let base = self.agents_from_dir(host)?;
        let name = agentsync::dir_name(name);
        let taken: Vec<String> = agentsync::load_records(&self.link_dir()).into_iter().map(|r| r.here).collect();
        for n in 1..1000 {
            let p = if n == 1 { base.join(&name) } else { base.join(format!("{name}-{n}")) };
            let free = std::fs::read_dir(&p).map_or(true, |mut d| d.next().is_none());
            if free && !taken.contains(&p.to_string_lossy().into_owned()) {
                return Ok((p, None));
            }
        }
        Err(format!("no free folder for {name} under {}", base.display()))
    }

    /// `<link-clone-dir>/from/<host>`, as a full path.
    fn agents_from_dir(&self, host: &str) -> Result<PathBuf, String> {
        let dir = PathBuf::from(super::expand_home(&self.opts.link_clone_dir));
        if !dir.is_absolute() {
            return Err(format!("link-clone-dir here is not a full path: {}", self.opts.link_clone_dir));
        }
        Ok(dir.join("from").join(agentsync::dir_name(host)))
    }

    /// `POST /link/agents/look`: where the folder would land, and what of
    /// it is there.
    pub(super) fn link_agents_look_in(
        &mut self,
        cid: Option<ClientId>,
        body: &str,
        nonce: &str,
        peer: &link::Peer,
    ) -> Outcome {
        if let Some(no) = self.may_send_agents(nonce, peer) {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let (there, name, host, branch) = (s("path"), s("name"), s("host"), s("branch"));
        let files: Vec<agentsync::FileAsk> = serde_json::from_value(v["files"].clone()).unwrap_or_default();
        let chats: Vec<agentsync::ChatAsk> = serde_json::from_value(v["chats"].clone()).unwrap_or_default();
        if there.is_empty() || (!branch.is_empty() && !crate::gitsync::branch_ok(&branch)) {
            return self.link_answer(nonce, 400, "a look: path, name, host, branch, files, chats".into());
        }
        let (dest, record) = match self.agents_place(&peer.key, &there, &host, &name) {
            Ok(p) => p,
            Err(e) => return self.link_answer(nonce, 400, e),
        };
        if dest.starts_with(self.link_dir()) {
            return self.link_answer(nonce, 400, "not inside keepane's own directory".into());
        }
        let git = crate::git::command(&self.opts.git_command);
        if !branch.is_empty() && git.is_none() {
            return self.link_answer(nonce, 500, "there is no git on this machine (git-command)".into());
        }
        let places = Places::of(&dirs::home_dir().unwrap_or_default(), &|k| std::env::var(k).ok());
        self.later(cid, nonce, move || {
            let folder = dest.to_string_lossy().into_owned();
            let forks = record.as_ref().map(|r| r.forks.clone()).unwrap_or_default();
            let need = files
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    agentsync::safe_rel(&f.p)
                        .is_some_and(|r| agentsync::sha256_file(&dest.join(r)).as_deref() != Some(f.s.as_str()))
                })
                .map(|(i, _)| i)
                .collect();
            let chats = chats
                .iter()
                .map(|c| match agentsync::chat_target(&places, &folder, c, &forks) {
                    Some((p, id)) => agentsync::have_of(&p, &id, &c.id),
                    None => agentsync::Have::default(),
                })
                .collect();
            let git = match &git {
                Some(g) if !branch.is_empty() => crate::gitsync::look(g, &dest, &branch),
                _ => crate::gitsync::Look { path: folder.clone(), ..Default::default() },
            };
            let look = Look { path: folder, known: record.is_some(), git, need, chats };
            (200, serde_json::to_string(&look).expect("a look serializes"), None)
        });
        Outcome::Pending
    }

    /// `POST /link/agents/chunk`: a piece of a pack, in order, kept here.
    pub(super) fn link_agents_chunk_in(&mut self, body: &str, nonce: &str, peer: &link::Peer) -> Outcome {
        if let Some(no) = self.may_send_agents(nonce, peer) {
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
        let limit = self.opts.link_agents_limit;
        let dir = self.link_dir().join("incoming");
        let (status, said) = match self.link() {
            Ok(l) => take_piece(&mut l.incoming, &dir, &transfer, index, &data, &peer.key, limit),
            Err(e) => return Outcome::Error(e),
        };
        self.link_answer(nonce, status, said.replace("link-push-limit", "link-agents-limit"))
    }

    /// `POST /link/agents/apply`: the pack that came, checked whole, taken in.
    pub(super) fn link_agents_apply_in(
        &mut self,
        cid: Option<ClientId>,
        body: &str,
        nonce: &str,
        peer: &link::Peer,
    ) -> Outcome {
        if let Some(no) = self.may_send_agents(nonce, peer) {
            return no;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let (transfer, sha, path, there, host) = (s("transfer"), s("sha256"), s("path"), s("there"), s("host"));
        let size = v["size"].as_u64().unwrap_or(0);
        // Only where the look said: where it went before, or a new folder
        // under `from/<that machine>`.
        let known =
            agentsync::load_records(&self.link_dir()).into_iter().find(|r| r.key == peer.key && r.there == there);
        let dest = PathBuf::from(&path);
        let allowed = match &known {
            Some(r) => r.here == path,
            None => self.agents_from_dir(&host).is_ok_and(|d| dest.parent() == Some(d.as_path())),
        };
        if !allowed || dest.starts_with(self.link_dir()) {
            return self.link_answer(nonce, 400, format!("{path}: not where the look said"));
        }
        let taken = self.link().ok().and_then(|l| {
            let mine = l.incoming.get(&transfer).is_some_and(|t| t.key() == peer.key);
            if mine { l.incoming.remove(&transfer) } else { None }
        });
        let Some(t) = taken else {
            return self.link_answer(nonce, 404, format!("no pack {transfer} came here"));
        };
        if t.size() != size {
            let _ = std::fs::remove_file(t.file());
            return self.link_answer(nonce, 400, format!("the pack came short: {} of {size} bytes", t.size()));
        }
        let record = known.unwrap_or_else(|| Record { key: peer.key.clone(), host: host.clone(), ..Record::default() });
        let apply = agentsync::Apply {
            pack: t.file().to_path_buf(),
            sha,
            dest,
            scratch: self.link_dir().join("incoming").join(format!("{transfer}.d")),
            host: host.clone(),
            remote: crate::gitsync::remote_name(&host),
            url: format!("keepane:{}", peer.addr),
            git: crate::git::command(&self.opts.git_command),
            places: Places::of(&dirs::home_dir().unwrap_or_default(), &|k| std::env::var(k).ok()),
            there,
            record,
        };
        let (dir, addr, key) = (self.link_dir(), peer.addr.clone(), peer.key.clone());
        self.later(cid, nonce, move || {
            let (pack, scratch) = (apply.pack.clone(), apply.scratch.clone());
            let r = agentsync::apply(apply);
            let _ = std::fs::remove_file(&pack);
            let _ = std::fs::remove_dir_all(&scratch);
            match r {
                Ok(a) => {
                    let path = a.record.here.clone();
                    let kept =
                        agentsync::save_record(&dir, a.record).err().map(|e| format!("\nnote: not kept here ({e})"));
                    let text = a.text + &kept.unwrap_or_default();
                    let fields = format!(
                        ",\"addr\":{},\"key\":{},\"path\":{},\"said\":{}",
                        js(&addr),
                        js(&key),
                        js(&path),
                        js(&text)
                    );
                    (200, text.clone(), Some((format!("link: a project from {addr} in {path}"), fields)))
                }
                Err(e) => (409, e, None),
            }
        });
        Outcome::Pending
    }
}
