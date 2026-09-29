//! Pane messages on the server (docs/design/mailbox.md): sending and
//! delivering, the commands, `-w` waits, and what a pane may change. A
//! pane's own state is `actor.rs`; the record of it all, `observe.rs`.

use std::time::{Duration, Instant};

use super::actor::{self, End, Message, MsgId, Sender, WorkMode};
use super::observe::Stage;
use super::{ClientId, Outcome, PaneId, Server, SessionId};
use crate::command::{Cmd, MoveTo, Target};

/// A client blocked in `-w` until something happens to a message or inbox.
pub(super) struct MsgWait {
    cid: ClientId,
    until: Instant,
    kind: WaitKind,
}

enum WaitKind {
    /// `send-message -w`: until it leaves the inbox.
    Delivered(MsgId),
    /// `trace-message -w`: until nothing more will happen to it.
    Finished(MsgId),
    /// `read-message -w`: until the pane's inbox has one to take.
    Inbox(PaneId),
    /// Another machine's `-w` on a message it sent here: until it leaves
    /// the inbox, answered as that machine reads it (`link_in`).
    Link { id: MsgId, nonce: String },
    /// Another machine's `trace-message -w` on a message it sent here:
    /// until nothing more will happen to it.
    LinkTrace { id: MsgId, nonce: String },
}

/// What a message answers and carries on: `-r` (the message the calling
/// pane works on), `--re` (message number so-and-so), `--task` (a chain).
pub(super) struct Answer {
    pub reply: bool,
    pub re: Option<MsgId>,
    pub task: Option<MsgId>,
}

/// A message deleted lately, for `drop-message -u`: when, its pane, where
/// it stood in the queue.
pub(super) struct Dropped {
    at: Instant,
    pane: PaneId,
    place: usize,
    msg: Message,
}

/// What `send-message` adds when its message waits for an agent that has
/// never said it is free (`program`: what the pane runs, when known): the
/// hook it lacks, and the `setup` that adds it.
pub(super) fn unheard_advice(pid: PaneId, program: Option<&str>) -> String {
    let agent = program.and_then(crate::setup::Agent::of_program);
    let fix = match agent {
        Some(crate::setup::Agent::Codex) => {
            "keepane setup codex --install, then trust it in Codex (/hooks)".to_string()
        }
        Some(a) => format!("keepane setup {} --install", a.word()),
        None => "keepane setup (it shows which agents lack it)".to_string(),
    };
    format!(
        "%{pid} has not said it is free since its agent started: an agent says so with a hook at the end of \
         each turn (keepane pane-ready), and without one its messages wait for ever; to add it: {fix}"
    )
}

impl Server {
    pub(super) fn pane_ref(&self, id: PaneId) -> Option<&super::pane::Pane> {
        self.sessions.iter().flat_map(|s| s.windows.iter()).find_map(|w| w.pane(id))
    }

    pub(super) fn place_of(&self, id: PaneId) -> Option<(SessionId, usize)> {
        self.sessions.iter().find_map(|s| s.windows.iter().position(|w| w.pane(id).is_some()).map(|widx| (s.id, widx)))
    }

    pub(super) fn address_of(&self, id: PaneId) -> String {
        match self.place_of(id) {
            Some((sid, widx)) => self.address(sid, widx, id),
            None => format!("%{id}"),
        }
    }

    /// The pane the command was run from, when it was run in one that
    /// still exists (the client's `KEEPANE_PANE`).
    pub(super) fn caller_pane(&self, cid: Option<ClientId>) -> Option<PaneId> {
        let p = cid.and_then(|c| self.clients.get(&c)).and_then(|c| c.pane_env)?;
        self.place_of(p).map(|_| p)
    }

    /// The caller, when it is an agent's pane: one in `ai` work mode, or one
    /// another pane made (`create-pane`). What a person types in a shell
    /// pane of their own is a person's command.
    fn agent_caller(&self, cid: Option<ClientId>) -> Option<PaneId> {
        let me = self.caller_pane(cid)?;
        let p = self.pane_ref(me)?;
        (p.actor.mode == WorkMode::Ai || p.actor.creator.is_some()).then_some(me)
    }

    /// The pane a mail command is about: the one named, else the caller's
    /// own, else the current pane of the attached client.
    fn mail_target(&self, target: Option<&Target>, cid: Option<ClientId>) -> Result<PaneId, String> {
        if target.is_none()
            && let Some(p) = self.caller_pane(cid)
        {
            return Ok(p);
        }
        self.resolve(target, cid).map(|(_, _, p)| p)
    }

    /// `pane` is `by`, or was created by it or by one it created.
    fn created_by(&self, pane: PaneId, by: PaneId) -> bool {
        let mut p = Some(pane);
        for _ in 0..1024 {
            match p {
                Some(id) if id == by => return true,
                Some(id) => p = self.pane_ref(id).and_then(|x| x.actor.creator),
                None => return false,
            }
        }
        false
    }

    pub(super) fn sender_of(&self, cid: Option<ClientId>) -> Sender {
        match self.caller_pane(cid).and_then(|id| self.pane_ref(id).map(|p| (id, p))) {
            Some((id, p)) => {
                Sender::Pane { id, address: self.address_of(id), name: p.actor.name.clone(), mode: p.actor.mode }
            }
            None => Sender::User,
        }
    }

    /// Who did it, for the event log: a pane's address, or `user`.
    pub(super) fn by(&self, cid: Option<ClientId>) -> String {
        self.caller_pane(cid).map_or_else(|| "user".to_string(), |p| self.address_of(p))
    }

    fn mode_word(&self, id: PaneId) -> &'static str {
        self.pane_ref(id).map_or("normal", |p| p.actor.mode.as_str())
    }

    /// Where a message stands, the way `send-message` reports it.
    pub(super) fn stand(&self, id: MsgId) -> String {
        let Some(r) = self.observe.get(id) else { return format!("#{id}") };
        let to = &r.msg.to;
        let via = r.msg.via.as_str();
        match r.stage {
            Stage::Queued => {
                let ahead = r
                    .msg
                    .to
                    .rsplit_once('%')
                    .and_then(|(_, n)| n.parse::<PaneId>().ok())
                    .and_then(|p| self.pane_ref(p))
                    .and_then(|p| p.actor.inbox.iter().position(|m| m.id == id))
                    .unwrap_or(0);
                let why = if r.msg.via == WorkMode::Normal { "waits for read-message" } else { "busy" };
                format!("#{id} queued for {to} ({via}, {why}, {ahead} ahead)")
            }
            Stage::Delivered => format!("#{id} delivered to {to} ({via})"),
            Stage::Read => format!("#{id} read by {to}"),
            s => format!("#{id} {} ({})", s.as_str(), r.why.as_deref().unwrap_or(to)),
        }
    }

    /// `stand`, and when the message waits for an agent that has never said
    /// it is free, why it may wait for ever and what sets that up.
    pub(super) fn stand_noted(&self, id: MsgId) -> String {
        let stand = self.stand(id);
        match self.unheard_note(id) {
            Some(note) => format!("{stand}\n{note}"),
            None => stand,
        }
    }

    fn unheard_note(&self, id: MsgId) -> Option<String> {
        let r = self.observe.get(id).filter(|r| r.stage == Stage::Queued)?;
        let pid = r.msg.to.rsplit_once('%').and_then(|(_, n)| n.parse::<PaneId>().ok())?;
        let p = self.pane_ref(pid).filter(|p| p.actor.unheard())?;
        let program = p.pid.map(crate::sysinfo::program_of);
        Some(unheard_advice(pid, program.as_deref()))
    }

    /// Run a pane-message command.
    pub(super) fn exec_mail(&mut self, cmd: Cmd, cid: Option<ClientId>) -> Outcome {
        let out = match cmd {
            Cmd::SendMessage { target, reply, re, task, wait, text } => {
                self.send_message(cid, target.as_ref(), Answer { reply, re, task }, wait, text)
            }
            Cmd::ReadMessage { target, wait } => self.read_message(cid, target.as_ref(), wait),
            Cmd::ListMessages { target, all } => self.list_messages(cid, target.as_ref(), all),
            Cmd::TraceMessage { id, wait } => match self.observe.trace(id, self.opts.message_envelope) {
                None => Outcome::Error(format!("no message #{id}")),
                // Handed to another machine: what became of it there, asked.
                Some(t)
                    if self.observe.get(id).is_some_and(|r| r.stage == Stage::Forwarded)
                        && self.link.as_ref().is_some_and(|l| l.sent.contains_key(&id)) =>
                {
                    self.trace_there(cid, id, t.clone(), wait).unwrap_or(Outcome::Text(t))
                }
                Some(t) => match (wait, cid) {
                    (Some(secs), Some(cid)) if !self.observe.get(id).is_some_and(|r| r.stage.finished()) => {
                        self.wait(cid, secs, WaitKind::Finished(id));
                        Outcome::Pending
                    }
                    _ => Outcome::Text(t),
                },
            },
            Cmd::DropMessage { id, undo } => self.drop_message(cid, id, undo),
            Cmd::MoveMessage { id, to } => self.move_message(cid, id, to),
            Cmd::PaneReady { quiet, target } => self.pane_ready(cid, quiet, target.as_ref()),
            Cmd::PaneStatus { text } => self.pane_status(cid, text),
            Cmd::SetWorkMode { target, mode } => self.set_work_mode(cid, target.as_ref(), &mode),
            Cmd::RenamePane { target, name } => self.rename_pane(cid, target.as_ref(), name),
            Cmd::Whoami => match self.mail_target(None, cid) {
                Ok(p) => Outcome::Text(self.whoami(p)),
                Err(e) => Outcome::Error(e),
            },
            Cmd::CreatePane(c) => self.create_pane(cid, *c),
            Cmd::ListTasks { target } => self.list_tasks(cid, target.as_ref()),
            Cmd::ShowTask { id } => self.show_task(id),
            Cmd::ListEvents { target, since, last } => self.list_events(cid, target.as_ref(), since, last),
            other => Outcome::Error(format!("not a message command: {other}")),
        };
        self.wake_waits();
        out
    }

    fn wait(&mut self, cid: ClientId, secs: u64, kind: WaitKind) {
        let secs = secs.min(self.opts.message_wait_max);
        self.msg_waits.push(MsgWait { cid, until: Instant::now() + Duration::from_secs(secs), kind });
    }

    fn send_message(
        &mut self,
        cid: Option<ClientId>,
        target: Option<&Target>,
        answer: Answer,
        wait: Option<u64>,
        text: String,
    ) -> Outcome {
        let Answer { reply, re, task: into_task } = answer;
        let me = self.caller_pane(cid);
        let actor = me.and_then(|p| self.pane_ref(p)).map(|p| &p.actor);
        // What it answers (-r: what the pane works on; --re: that message),
        // and the chain it carries on.
        let (answering, mut carry) = match re {
            Some(r) => match self.observe.get(r) {
                Some(rec) => (Some(rec.msg.clone()), Some((rec.msg.task, rec.msg.hop + 1))),
                None => return Outcome::Error(format!("send-message --re: no message #{r}")),
            },
            None => (actor.and_then(|a| a.answering().cloned()), actor.and_then(|a| a.carry(reply))),
        };
        // --task: that chain, one hop past the furthest it has gone, so the
        // hop limit still stops a loop.
        if let Some(t) = into_task {
            // A refused message went nowhere: it is not how far the chain got.
            let furthest = self.observe.task(t).iter().filter(|r| r.stage != Stage::Rejected).map(|r| r.msg.hop).max();
            let Some(furthest) = furthest else {
                return Outcome::Error(format!("send-message --task: no task #{t}"));
            };
            if let (Some(r), Some(a)) = (re, &answering)
                && a.task != t
            {
                return Outcome::Error(format!("send-message: #{r} is in task #{}, not #{t}", a.task));
            }
            carry = Some((t, furthest + 1));
        }
        let reply = reply || (re.is_some() && target.is_none());
        // A pane of another machine (docs/design/link.md): what was asked
        // for by `host:port/…`, or the sender of a message that came from
        // there. It goes out through `send_remote`; the record here says so.
        if let Some(t) = target.filter(|_| !reply)
            && let Some(machine) = &t.remote
        {
            let there = crate::command::target_string(&Target { remote: None, ..t.clone() });
            let from = self.sender_of(cid);
            return self.send_remote(cid, machine, &there, text, from, carry, None, wait);
        }
        if reply
            && let Some(cur) = &answering
            && let Sender::Remote { addr, address, .. } = &cur.from
        {
            let (how, cur_id) = (if re.is_some() { "--re" } else { "-r" }, cur.id);
            let Some(origin) = self.link.as_ref().and_then(|l| l.origin.get(&cur_id)).cloned() else {
                return Outcome::Error(format!(
                    "send-message {how}: #{cur_id} came from {addr}, and the way back is gone (the server restarted since)"
                ));
            };
            // The pane there by its id alone, as an answer here goes by id:
            // it finds the sender wherever its pane has moved since.
            let Some(pane) = address.rsplit_once('%').map(|(_, id)| format!("%{id}")) else {
                let instead = if re.is_some() { "; say where with --to" } else { "" };
                return Outcome::Error(format!(
                    "send-message {how}: #{cur_id} came from outside any pane on {addr}{instead}"
                ));
            };
            let addr = addr.clone();
            let from = self.sender_of(cid);
            return self.send_remote(cid, &addr, &pane, text, from, carry, Some((cur_id, origin)), wait);
        }
        let to = if reply {
            let Some(cur) = &answering else {
                return Outcome::Error(match me {
                    Some(me) => format!("send-message -r: %{me} has no message to answer"),
                    None => "send-message -r: not run inside a pane".into(),
                });
            };
            // Said the way it was asked: -r, or --re (which can be sent
            // elsewhere with --to).
            let (how, instead) = if re.is_some() { ("--re", "; say where with --to") } else { ("-r", "") };
            match cur.sender_pane() {
                Some(p) if self.place_of(p).is_some() => p,
                Some(p) => {
                    return Outcome::Error(format!(
                        "send-message {how}: #{} came from %{p}, which is gone{instead}",
                        cur.id
                    ));
                }
                None => {
                    return Outcome::Error(format!(
                        "send-message {how}: #{} came from outside any pane{instead}",
                        cur.id
                    ));
                }
            }
        } else {
            let Some(t) = target else { return Outcome::Error("send-message: --to pane (or -t) required".into()) };
            match self.resolve(Some(t), cid) {
                Ok((_, _, p)) => p,
                Err(e) => return Outcome::Error(e),
            }
        };
        self.next_msg += 1;
        let id = self.next_msg;
        let (task, hop) = carry.unwrap_or((id, 0));
        let m = Message {
            id,
            task,
            from: self.sender_of(cid),
            to: self.address_of(to),
            via: self.pane_ref(to).map_or(WorkMode::Normal, |p| p.actor.mode),
            hop,
            re: if reply || re.is_some() { answering.as_ref().map(|c| c.id) } else { None },
            text,
            // The event log keeps milliseconds; so does the message.
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        if let Err(why) = self.enqueue(to, m) {
            return Outcome::Error(format!("#{id} rejected: {why}"));
        }
        match (wait, cid) {
            (Some(secs), Some(cid)) if self.observe.get(id).is_some_and(|r| r.stage == Stage::Queued) => {
                self.wait(cid, secs, WaitKind::Delivered(id));
                Outcome::Pending
            }
            _ => Outcome::Text(self.stand_noted(id)),
        }
    }

    /// A message with its number, sender and text made up, into pane `to`'s
    /// inbox: refused (and recorded so) when too big, too many hops on, or
    /// the pane is gone or full; else recorded as sent, and typed in at
    /// once when the pane is free.
    pub(super) fn enqueue(&mut self, to: PaneId, m: Message) -> Result<(), String> {
        let refuse = if m.text.len() > self.opts.message_max_size {
            Some(format!("the message is {} bytes; message-max-size is {}", m.text.len(), self.opts.message_max_size))
        } else if m.hop > self.opts.message_hop_limit {
            Some(format!(
                "hop {} is over message-hop-limit {} (a loop between panes?)",
                m.hop, self.opts.message_hop_limit
            ))
        } else if self.pane_ref(to).is_some_and(|p| p.exit_code.is_some()) {
            Some(format!("%{to} has exited"))
        } else {
            None
        };
        let limit = self.opts.message_inbox_limit;
        let queued = match refuse {
            Some(why) => Err(why),
            None => match super::Server::find_pane_mut(self, to) {
                Some(p) => p.actor.enqueue(m.clone(), limit).map_err(|e| format!("%{to}: {e}")),
                None => Err(format!("can't find pane: %{to}")),
            },
        };
        if let Err(why) = queued {
            self.observe.sent(&m, Some(&why));
            return Err(why);
        }
        self.observe.sent(&m, None);
        if m.via == WorkMode::Normal {
            self.mail_alert(to, &m);
        }
        self.deliver(to);
        Ok(())
    }

    /// A `-w` on a message another machine sent in (`link_in`): answered
    /// with the signed JSON that machine reads, not the text a person does.
    pub(super) fn wait_link(&mut self, cid: ClientId, secs: u64, id: MsgId, nonce: String) {
        self.wait(cid, secs, WaitKind::Link { id, nonce });
    }

    /// The same for another machine's `trace-message -w`.
    pub(super) fn wait_link_trace(&mut self, cid: ClientId, secs: u64, id: MsgId, nonce: String) {
        self.wait(cid, secs, WaitKind::LinkTrace { id, nonce });
    }

    /// A message for a pane that takes none on its own: its window is
    /// flagged (`@` in `#F`) and the session's clients are told.
    fn mail_alert(&mut self, to: PaneId, m: &Message) {
        let Some((sid, widx)) = self.place_of(to) else { return };
        let base = self.opts.base_index;
        let from = m.from.short();
        let text = format!("Message #{} for %{to} in window {} from {from}", m.id, widx + base);
        if let Some(s) = self.session_mut(sid)
            && s.cur != widx
        {
            s.windows[widx].alert_mail = true;
        }
        let ids: Vec<ClientId> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for cid in ids {
            self.show(cid, &text);
        }
    }

    /// Type the next message into the pane, if it is free for one.
    pub(super) fn deliver(&mut self, pid: PaneId) {
        let style = self.opts.message_envelope;
        let Some(p) = self.find_pane_mut(pid) else { return };
        // Not while a command's end is settling: its output is taken first.
        if p.exit_code.is_some() || p.is_pending() || p.settle {
            return;
        }
        let Some(m) = p.actor.next_delivery() else { return };
        let text = m.wrapped(p.syntax, style);
        p.deliver(&text);
        self.observe.delivered(m.id);
    }

    /// keepane's prompt marker, as it arrives: the pane is at its prompt.
    pub(super) fn pane_prompt_seen(&mut self, pid: PaneId) {
        let Some(p) = self.find_pane_mut(pid) else { return };
        let was_idle = p.actor.idle();
        p.actor.prompt_seen();
        if !was_idle && p.actor.idle() {
            let addr = self.address_of(pid);
            self.observe.pane(&addr, "idle", "");
        }
    }

    /// keepane's prompt came back in a pane (`PROMPT_SETTLE` ago, so what the
    /// command printed is drawn): a shell's command is done (its output and
    /// success go on record), an agent's pane is back at a shell, and the next
    /// message may go in (unless someone has typed since the marker).
    pub(super) fn pane_prompted(&mut self, pid: PaneId) {
        let max = self.opts.message_max_size;
        let Some(p) = self.find_pane_mut(pid) else { return };
        let Some((m, end)) = p.actor.command_over() else {
            self.deliver(pid);
            return;
        };
        let (mut ok, mut output) = (None, None);
        if m.via == WorkMode::Shell && end == End::Done {
            let (typed, from) = p.delivered_line.unwrap_or((0, 0));
            // The last command the hook reported, if it ran on or after
            // the line this one was typed on.
            ok = p
                .marks
                .iter()
                .rev()
                .find(|k| k.end.is_some())
                .filter(|k| k.line >= typed)
                .and_then(|k| k.exit)
                .map(|c| c == 0);
            let to = p.cursor_line();
            // Where the command ended, read off the screen when it can be;
            // counted from the cursor at delivery otherwise.
            let text = p.delivered_text.take();
            let from = text.and_then(|t| p.command_end(typed, to, &t)).map_or(from, |row| row + 1);
            let (body, mut cut) = p.text_between(from, to, max + 4096);
            // A blank row before the output is the terminal's, not the
            // command's (`Pane::deliver`).
            let mut body = body.trim_start_matches('\n').to_string();
            if body.len() > max {
                let mut e = max;
                while !body.is_char_boundary(e) {
                    e -= 1;
                }
                body.truncate(e);
                cut = true;
            }
            output = Some((body, cut));
        }
        self.observe.ended(m.id, end, ok, output);
        self.task_done(pid, &m, end, ok);
        self.deliver(pid);
    }

    /// A message's task ended in pane `pid` (`done-events` `task`).
    fn task_done(&mut self, pid: PaneId, m: &Message, end: End, ok: Option<bool>) {
        let text = match (end, ok) {
            (End::Abandoned, _) => format!("#{} ended unfinished: {}", m.id, super::done::gist(&m.text)),
            (_, Some(false)) => format!("#{} failed: {}", m.id, super::done::gist(&m.text)),
            _ => format!("#{} done: {}", m.id, super::done::gist(&m.text)),
        };
        self.pane_done(pid, super::done::Kind::Task, text, ok, None, None);
    }

    fn pane_ready(&mut self, cid: Option<ClientId>, quiet: bool, target: Option<&Target>) -> Outcome {
        if let Some(t) = target {
            // Unsticking a pane that stays busy.
            let pid = match self.resolve(Some(t), cid) {
                Ok((_, _, p)) => p,
                Err(e) => return Outcome::Error(e),
            };
            if let Some(p) = self.find_pane_mut(pid) {
                p.actor.force_idle();
            }
            let (addr, by) = (self.address_of(pid), self.by(cid));
            self.observe.pane(&addr, "idle", &format!(",\"by\":{}", serde_json::to_string(&by).unwrap()));
            self.deliver(pid);
            return Outcome::Ok;
        }
        let Some(me) = self.caller_pane(cid) else {
            return if quiet {
                Outcome::Ok
            } else {
                Outcome::Error("pane-ready: not run inside a keepane pane".into())
            };
        };
        let Some(p) = self.find_pane_mut(me) else { return Outcome::Ok };
        // A turn that ended: the agent was heard from before (its first word
        // is its session starting) and was not already free.
        let turn = !p.actor.unheard() && !p.actor.idle();
        let (taken, done) = p.actor.ready();
        if !taken {
            // Only an agent's word counts; a hook set up for every agent
            // also runs in panes that are not `ai`, quietly.
            return Outcome::Ok;
        }
        if turn {
            let text = match &done {
                Some((m, _)) => format!("the agent finished #{}: {}", m.id, super::done::gist(&m.text)),
                None => "the agent finished its turn".to_string(),
            };
            self.pane_done(me, super::done::Kind::Agent, text, None, None, None);
        }
        if let Some((m, end)) = done {
            self.observe.ended(m.id, end, None, None);
            self.task_done(me, &m, end, None);
        }
        let addr = self.address_of(me);
        self.observe.pane(&addr, "idle", "");
        self.deliver(me);
        Outcome::Ok
    }

    fn read_message(&mut self, cid: Option<ClientId>, target: Option<&Target>, wait: Option<u64>) -> Outcome {
        let pid = match self.mail_target(target, cid) {
            Ok(p) => p,
            Err(e) => return Outcome::Error(e),
        };
        match self.take_message(pid) {
            Some(text) => Outcome::Text(text),
            None => match (wait, cid) {
                (Some(secs), Some(cid)) => {
                    self.wait(cid, secs, WaitKind::Inbox(pid));
                    Outcome::Pending
                }
                _ => Outcome::Error(format!("no message for %{pid}")),
            },
        }
    }

    /// Take the oldest message from a pane's inbox, as `read-message` prints it.
    fn take_message(&mut self, pid: PaneId) -> Option<String> {
        let style = self.opts.message_envelope;
        let p = self.find_pane_mut(pid)?;
        let m = p.actor.read()?;
        self.observe.read(m.id);
        Some(format!("{}\n{}", m.header(style), m.text))
    }

    fn list_messages(&mut self, cid: Option<ClientId>, target: Option<&Target>, all: bool) -> Outcome {
        let panes: Vec<PaneId> = if all {
            self.sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.iter()).map(|p| p.id).collect()
        } else {
            match self.mail_target(target, cid) {
                Ok(p) => vec![p],
                Err(e) => return Outcome::Error(e),
            }
        };
        let now = chrono::Local::now();
        let mut out = Vec::new();
        for pid in panes {
            let Some(p) = self.pane_ref(pid) else { continue };
            let a = &p.actor;
            if all && a.inbox.is_empty() && a.current.is_none() {
                continue;
            }
            let state = match a.mode {
                WorkMode::Normal => String::new(),
                _ => {
                    if a.idle() {
                        ", idle".into()
                    } else {
                        ", busy".into()
                    }
                }
            };
            let name = a.name.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
            let mut head =
                format!("{}{name} ({}{state}) · {} queued", self.address_of(pid), a.mode.as_str(), a.inbox.len());
            if let Some(c) = &a.current {
                head.push_str(&format!(" · working on #{}", c.id));
            }
            out.push(head);
            for m in &a.inbox {
                let from = match &m.from {
                    Sender::User => "user".to_string(),
                    s => format!(
                        "{}{}{}",
                        s.from_field(),
                        s.name().map(|n| format!(" {n}")).unwrap_or_default(),
                        s.mode().map(|m| format!(" ({})", m.as_str())).unwrap_or_default()
                    ),
                };
                let waited = super::observe::since(m.at, now);
                let first = m.text.lines().next().unwrap_or_default();
                let more = if m.text.lines().nth(1).is_some() { " …" } else { "" };
                let mismatch = if m.via != a.mode { format!("  [sent for {}]", m.via.as_str()) } else { String::new() };
                out.push(format!("  #{}  from {from}  waiting {waited}  {first}{more}{mismatch}", m.id));
            }
        }
        Outcome::Text(out.join("\n"))
    }

    /// Which pane holds a queued message.
    fn holder(&self, id: MsgId) -> Option<PaneId> {
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .find(|p| p.actor.inbox.iter().any(|m| m.id == id))
            .map(|p| p.id)
    }

    fn drop_message(&mut self, cid: Option<ClientId>, id: Option<MsgId>, undo: bool) -> Outcome {
        let by = self.by(cid);
        if undo {
            let keep = Duration::from_secs(self.opts.undo_kill_time);
            self.msg_dropped.retain(|d| d.at.elapsed() < keep && d.pane != 0);
            let Some(i) = self.msg_dropped.len().checked_sub(1) else {
                return Outcome::Error("drop-message -u: nothing deleted to bring back".into());
            };
            let d = self.msg_dropped.remove(i);
            let Some(p) = self.find_pane_mut(d.pane) else {
                return Outcome::Error(format!("drop-message -u: %{} is gone", d.pane));
            };
            let id = d.msg.id;
            p.actor.restore(d.msg, d.place);
            self.observe.restored(id, &by);
            self.deliver(d.pane);
            return Outcome::Text(self.stand(id));
        }
        let id = id.expect("parser: an id or -u");
        let Some(pid) = self.holder(id) else {
            return Outcome::Error(format!("#{id} is not waiting in any inbox"));
        };
        let p = self.find_pane_mut(pid).expect("holder found it");
        let place = p.actor.inbox.iter().position(|m| m.id == id).unwrap_or(0);
        let Some(msg) = p.actor.remove(id) else { return Outcome::Error(format!("#{id} is not queued")) };
        self.observe.dropped(id, &format!("deleted by {by}"));
        self.msg_dropped.push(Dropped { at: Instant::now(), pane: pid, place, msg });
        Outcome::Ok
    }

    fn move_message(&mut self, cid: Option<ClientId>, id: MsgId, to: MoveTo) -> Outcome {
        let Some(pid) = self.holder(id) else {
            return Outcome::Error(format!("#{id} is not waiting in any inbox"));
        };
        let by = self.by(cid);
        let to = match to {
            MoveTo::Up => actor::Move::Up,
            MoveTo::Down => actor::Move::Down,
            MoveTo::Top => actor::Move::Top,
        };
        let Some((from, place)) = self.find_pane_mut(pid).and_then(|p| p.actor.move_message(id, to)) else {
            return Outcome::Error(format!("#{id} is not queued"));
        };
        if from != place {
            self.observe.moved(id, from, place, &by);
        }
        Outcome::Ok
    }

    fn pane_status(&mut self, cid: Option<ClientId>, text: String) -> Outcome {
        let Some(me) = self.caller_pane(cid) else {
            return Outcome::Error("pane-status: not run inside a keepane pane".into());
        };
        let addr = self.address_of(me);
        let program = self.pane_program(me);
        let Some(p) = self.find_pane_mut(me) else { return Outcome::Ok };
        let (name, mode) = (p.actor.name.clone(), p.actor.mode);
        p.actor.status = if text.is_empty() { None } else { Some((text.clone(), chrono::Local::now())) };
        let js = |s: &str| serde_json::to_string(s).unwrap();
        let name = name.map(|n| format!(",\"name\":{}", js(&n))).unwrap_or_default();
        self.observe.pane(
            &addr,
            "status",
            &format!("{name},\"mode\":\"{}\",\"program\":{},\"text\":{}", mode.as_str(), js(&program), js(&text)),
        );
        Outcome::Ok
    }

    /// The program in the foreground of a pane, as `#{pane_current_command}` says.
    fn pane_program(&self, pid: PaneId) -> String {
        let Some((sid, widx)) = self.place_of(pid) else { return String::new() };
        self.context(sid, widx, Some(pid), None).pane_command
    }

    fn set_work_mode(&mut self, cid: Option<ClientId>, target: Option<&Target>, mode: &str) -> Outcome {
        let Some(mode) = WorkMode::parse(mode) else { return Outcome::Error(format!("bad work mode '{mode}'")) };
        let pid = match self.mail_target(target, cid) {
            Ok(p) => p,
            Err(e) => return Outcome::Error(e),
        };
        // A pane's mode is changed in that pane: nothing run in one pane turns
        // another into a shell that runs what it is sent. From outside every
        // pane (a terminal, the `:` prompt, a key) it goes where it is aimed.
        if let Some(me) = self.caller_pane(cid)
            && me != pid
        {
            return Outcome::Error(format!(
                "set-work-mode changes only the pane it runs in (%{me}, not %{pid}); \
                 for another, run it there, or at the C-b : prompt"
            ));
        }
        let by = self.by(cid);
        let addr = self.address_of(pid);
        if let Some(p) = self.find_pane_mut(pid) {
            p.actor.set_mode(mode);
        }
        self.observe.pane(
            &addr,
            "mode",
            &format!(",\"value\":\"{}\",\"by\":{}", mode.as_str(), serde_json::to_string(&by).unwrap()),
        );
        self.deliver(pid);
        Outcome::Ok
    }

    fn rename_pane(&mut self, cid: Option<ClientId>, target: Option<&Target>, name: String) -> Outcome {
        let pid = match self.mail_target(target, cid) {
            Ok(p) => p,
            Err(e) => return Outcome::Error(e),
        };
        if !name.is_empty() {
            if let Err(e) = actor::check_name(&name) {
                return Outcome::Error(e);
            }
            if let Some(other) = self.pane_by_name(&name).filter(|o| *o != pid) {
                return Outcome::Error(format!("pane name '{name}' is taken by %{other}"));
            }
        }
        let by = self.by(cid);
        let addr = self.address_of(pid);
        if let Some(p) = self.find_pane_mut(pid) {
            p.actor.name = if name.is_empty() { None } else { Some(name.clone()) };
        }
        let js = |s: &str| serde_json::to_string(s).unwrap();
        self.observe.pane(&addr, "name", &format!(",\"value\":{},\"by\":{}", js(&name), js(&by)));
        Outcome::Ok
    }

    fn whoami(&self, pid: PaneId) -> String {
        let Some((sid, widx)) = self.place_of(pid) else { return format!("%{pid}") };
        let s = self.session(sid).expect("placed");
        let pidx = s.windows[widx].layout.panes().iter().position(|p| *p == pid).unwrap_or(0);
        let place = format!("{}:{}.{}", s.name, widx + self.opts.base_index, pidx + self.opts.pane_base_index);
        let p = self.pane_ref(pid).expect("placed");
        let name = p.actor.name.as_deref().unwrap_or("-");
        format!("{}  {place}  {name}  {}", self.address(sid, widx, pid), self.mode_word(pid))
    }

    fn all_panes(&self) -> std::collections::HashSet<PaneId> {
        self.sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.iter()).map(|p| p.id).collect()
    }

    /// The pane at the top of `p`'s line of creators (itself when nobody
    /// alive created it): its panes and theirs are one budget.
    fn top_creator(&self, mut p: PaneId) -> PaneId {
        for _ in 0..1024 {
            match self.pane_ref(p).and_then(|x| x.actor.creator).filter(|c| self.pane_ref(*c).is_some()) {
                Some(c) => p = c,
                None => break,
            }
        }
        p
    }

    /// `create-pane`: a session, window or split made for an agent, with
    /// its name, work mode and first message. From inside a pane it is that
    /// pane's creation: only `agent-commands` programs, within
    /// `agent-pane-limit` for the whole line of creators.
    fn create_pane(&mut self, cid: Option<ClientId>, c: crate::command::CreatePane) -> Outcome {
        let crate::command::CreatePane { kind, target, session_name, horizontal, cwd, name, mode, message, argv } = c;
        // An agent's creation: its own, within its budget and list. A person's
        // is nobody's.
        let me = self.agent_caller(cid);
        // The program it runs: the one given, else the pane default.
        let program = argv.first().cloned().unwrap_or_else(|| {
            self.opts.default_command.first().cloned().unwrap_or_else(|| self.opts.default_shell.clone())
        });
        let stem = std::path::Path::new(&program)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if let Some(me) = me {
            let allowed: Vec<String> =
                self.opts.agent_commands.split_whitespace().map(str::to_ascii_lowercase).collect();
            if !allowed.contains(&stem) {
                return Outcome::Error(format!(
                    "create-pane: '{stem}' is not in agent-commands ({})",
                    self.opts.agent_commands
                ));
            }
            let top = self.top_creator(me);
            let made = self.all_panes().into_iter().filter(|p| *p != top && self.created_by(*p, top)).count();
            if made >= self.opts.agent_pane_limit as usize {
                return Outcome::Error(format!(
                    "create-pane: agent-pane-limit {} reached (%{top} and the panes it made have {made})",
                    self.opts.agent_pane_limit
                ));
            }
        }
        if let Some(n) = &name {
            if let Err(e) = actor::check_name(n) {
                return Outcome::Error(e);
            }
            if self.pane_by_name(n).is_some() {
                return Outcome::Error(format!("pane name '{n}' is taken"));
            }
        }
        let mode = match mode.as_deref() {
            Some(m) => WorkMode::parse(m).unwrap_or_default(),
            // An agent takes prompts; a shell made for an agent takes commands.
            None => match stem.as_str() {
                s if crate::setup::Agent::of_program(s).is_some() => WorkMode::Ai,
                s if crate::platform::shell::takes_commands(s) => WorkMode::Shell,
                _ => WorkMode::Normal,
            },
        };
        // Beside the pane that asked, unless told where.
        let target =
            target.or_else(|| self.caller_pane(cid).map(|p| Target { pane_id: Some(p), ..Default::default() }));
        let inner = match kind.as_str() {
            "session" => Cmd::NewSession {
                name: session_name,
                window_name: None,
                cwd,
                detached: true,
                argv,
                attach_existing: false,
                size: (None, None),
            },
            "window" => Cmd::NewWindow { name: None, cwd, target, argv, detached: true },
            _ => {
                Cmd::SplitWindow { horizontal, cwd, target, argv, detached: true, before: false, full: false, count: 1 }
            }
        };
        let before = self.all_panes();
        if let Outcome::Error(e) = self.exec(inner, cid) {
            return Outcome::Error(format!("create-pane: {e}"));
        }
        // The oldest of the new: a hook (after-split-window...) runs within
        // the same command and may make more after ours.
        let Some(new) = self.all_panes().difference(&before).copied().min() else {
            return Outcome::Error("create-pane: no pane was made".into());
        };
        if let Some(p) = self.find_pane_mut(new) {
            p.actor.creator = me;
            p.actor.name = name;
            p.actor.set_mode(mode);
        }
        let (addr, by) = (self.address_of(new), self.by(cid));
        let js = |s: &str| serde_json::to_string(s).unwrap();
        self.observe.pane(
            &addr,
            "created",
            &format!(",\"by\":{},\"program\":{},\"mode\":\"{}\"", js(&by), js(&stem), mode.as_str()),
        );
        let mut out = vec![self.whoami(new)];
        if let Some(text) = message {
            let t = Target { pane_id: Some(new), ..Default::default() };
            match self.send_message(cid, Some(&t), Answer { reply: false, re: None, task: None }, None, text) {
                Outcome::Text(s) => out.push(s),
                Outcome::Error(e) => out.push(e),
                _ => {}
            }
        }
        Outcome::Text(out.join("\n"))
    }

    /// The address prefix of a session (`$1:`), for filtering by it.
    fn session_prefix(&self, target: Option<&Target>, cid: Option<ClientId>) -> Result<Option<String>, String> {
        match target {
            Some(t) => self.resolve_session(Some(t), cid).map(|s| Some(format!("${s}:"))),
            None => Ok(None),
        }
    }

    fn list_tasks(&self, cid: Option<ClientId>, target: Option<&Target>) -> Outcome {
        let only = match self.session_prefix(target, cid) {
            Ok(p) => p,
            Err(e) => return Outcome::Error(e),
        };
        let touches = |r: &&super::observe::Record, p: &str| {
            r.msg.to.starts_with(p) || matches!(&r.msg.from, Sender::Pane { address, .. } if address.starts_with(p))
        };
        let mut rows = vec![["TASK", "STATUS", "AT", "TOOK", "STEPS", "TITLE"].map(String::from).to_vec()];
        for t in self.observe.tasks() {
            if let Some(p) = &only
                && !self.observe.task(t.task).iter().any(|r| touches(r, p))
            {
                continue;
            }
            rows.push(vec![
                format!("#{}", t.task),
                t.status.to_string(),
                t.at.unwrap_or_else(|| "-".into()),
                t.took,
                t.steps.to_string(),
                t.title,
            ]);
        }
        if rows.len() == 1 {
            return Outcome::Text(String::new());
        }
        Outcome::Text(super::align_columns(&rows).join("\n"))
    }

    fn show_task(&self, task: MsgId) -> Outcome {
        let steps = self.observe.task(task);
        if steps.is_empty() {
            return Outcome::Error(format!("no task #{task}"));
        }
        let who = |s: &Sender| match s {
            Sender::User => "user".to_string(),
            s => s.name().map_or_else(|| s.from_field(), |n| format!("%{n}")),
        };
        let out: Vec<String> = steps
            .iter()
            .map(|r| {
                let m = &r.msg;
                let queued = r.delivered.map(|d| format!("queued {}", super::observe::since(m.at, d)));
                let took = match (r.delivered, r.ended) {
                    (Some(d), Some(e)) => Some(format!("took {}", super::observe::since(d, e))),
                    _ => None,
                };
                let ok = r.ok.map(|ok| format!("ok={}", ok as u8));
                let output =
                    r.output.as_ref().map(|o| format!("output {}B{}", o.len(), if r.cut { " (cut)" } else { "" }));
                let facts: Vec<String> = [queued, took, ok, output].into_iter().flatten().collect();
                format!(
                    "{}  #{}  {} → {} ({})  {}  {}  \"{}\"",
                    m.at.format("%H:%M:%S"),
                    m.id,
                    who(&m.from),
                    m.to,
                    m.via.as_str(),
                    r.stage.as_str(),
                    facts.join("  "),
                    m.text.lines().next().unwrap_or_default()
                )
            })
            .collect();
        Outcome::Text(out.join("\n"))
    }

    /// The event log's lines from the last `since` seconds (today's when
    /// not given), about the pane or session `target` names.
    fn list_events(
        &self,
        cid: Option<ClientId>,
        target: Option<&Target>,
        since: Option<u64>,
        last: Option<usize>,
    ) -> Outcome {
        let about = match target {
            None => None,
            Some(t) if t.names_pane() || t.pane.is_some() => match self.resolve(Some(t), cid) {
                Ok((sid, widx, pid)) => Some(self.address(sid, widx, pid)),
                Err(e) => return Outcome::Error(e),
            },
            Some(t) => match self.session_prefix(Some(t), cid) {
                Ok(p) => p,
                Err(e) => return Outcome::Error(e),
            },
        };
        let now = chrono::Local::now();
        let cutoff = since.map(|s| now - chrono::Duration::seconds(s as i64));
        let recent_enough = |line: &str| {
            cutoff.is_none_or(|c| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v["at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()))
                    .is_none_or(|at| at >= c)
            })
        };
        // The last few, from memory: what a watcher asks every second.
        if let Some(n) = last {
            let keep = |l: &str| about.as_ref().is_none_or(|a| l.contains(a.as_str())) && recent_enough(l);
            return Outcome::Text(self.observe.recent(n, keep).join("\n"));
        }
        crate::histlog::flush_events(Duration::from_secs(2));
        let days = since.map_or(0, |s| s / 86_400 + 1);
        let dir = self.events_dir();
        let mut out = Vec::new();
        for back in (0..=days).rev() {
            let Some(day) = now.date_naive().checked_sub_days(chrono::Days::new(back)) else { continue };
            let Ok(text) = std::fs::read_to_string(dir.join(format!("{}.jsonl", day.format("%Y-%m-%d")))) else {
                continue;
            };
            for line in text.lines() {
                if let Some(a) = &about
                    && !line.contains(a.as_str())
                {
                    continue;
                }
                if let Some(c) = cutoff {
                    let at = serde_json::from_str::<serde_json::Value>(line)
                        .ok()
                        .and_then(|v| v["at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()));
                    if at.is_some_and(|at| at < c) {
                        continue;
                    }
                }
                out.push(line.to_string());
            }
        }
        Outcome::Text(out.join("\n"))
    }

    /// A pane is going away: what waited in its inbox is dropped, and
    /// what it was working on ends unfinished.
    pub(super) fn pane_closing(&mut self, pid: PaneId) {
        let Some(p) = self.find_pane_mut(pid) else { return };
        let queued: Vec<MsgId> = p.actor.inbox.drain(..).map(|m| m.id).collect();
        let current = p.actor.current.take();
        for id in queued {
            self.observe.dropped(id, "its pane closed");
        }
        if let Some(m) = current {
            self.observe.ended(m.id, End::Abandoned, None, None);
        }
        self.wake_waits();
    }

    /// Answer the `-w` waits whose moment has come.
    pub(super) fn wake_waits(&mut self) {
        let mut i = 0;
        while i < self.msg_waits.len() {
            let w = &self.msg_waits[i];
            let answer = match &w.kind {
                WaitKind::Link { id, nonce } => match self.observe.get(*id).map(|r| r.stage) {
                    Some(Stage::Queued) => None,
                    _ => {
                        let (id, nonce) = (*id, nonce.clone());
                        Some(self.link_send_answer(id, &nonce))
                    }
                },
                WaitKind::LinkTrace { id, nonce } => match self.observe.get(*id) {
                    Some(r) if !r.stage.finished() => None,
                    _ => {
                        let (id, nonce) = (*id, nonce.clone());
                        Some(self.link_trace_answer(id, &nonce))
                    }
                },
                &WaitKind::Delivered(id) => match self.observe.get(id).map(|r| r.stage) {
                    Some(Stage::Queued) => None,
                    Some(Stage::Dropped | Stage::Rejected) => Some(Outcome::Error(self.stand(id))),
                    _ => Some(Outcome::Text(self.stand(id))),
                },
                &WaitKind::Finished(id) => match self.observe.get(id) {
                    Some(r) if !r.stage.finished() => None,
                    _ => Some(
                        self.observe
                            .trace(id, self.opts.message_envelope)
                            .map_or(Outcome::Error(format!("no message #{id}")), Outcome::Text),
                    ),
                },
                &WaitKind::Inbox(pid) => {
                    if self.pane_ref(pid).is_none() {
                        Some(Outcome::Error(format!("%{pid} is gone")))
                    } else if self.pane_ref(pid).is_some_and(|p| !p.actor.inbox.is_empty()) {
                        self.take_message(pid).map(Outcome::Text)
                    } else {
                        None
                    }
                }
            };
            match answer {
                Some(out) => {
                    let w = self.msg_waits.remove(i);
                    self.reply(w.cid, out);
                }
                None => i += 1,
            }
        }
    }

    /// From the tick: waits that ran out answer with an error, and the
    /// event log follows its options.
    pub(super) fn mail_tick(&mut self) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.msg_waits.len() {
            if self.msg_waits[i].until <= now {
                let w = self.msg_waits.remove(i);
                let what = match w.kind {
                    WaitKind::Delivered(id) => self.stand_noted(id),
                    WaitKind::Finished(id) => {
                        self.observe.get(id).map_or(format!("#{id}"), |r| format!("#{id} {}", r.stage.as_str()))
                    }
                    WaitKind::Inbox(pid) => format!("no message for %{pid}"),
                    // Still queued: the other machine hears so, as a fact,
                    // not a failure.
                    WaitKind::Link { id, nonce } => {
                        let out = self.link_send_answer(id, &nonce);
                        self.reply(w.cid, out);
                        continue;
                    }
                    WaitKind::LinkTrace { id, nonce } => {
                        let out = self.link_trace_answer(id, &nonce);
                        self.reply(w.cid, out);
                        continue;
                    }
                };
                self.reply(w.cid, Outcome::Error(format!("timed out: {what}")));
            } else {
                i += 1;
            }
        }
        self.sync_event_log();
        // A pane can go in many ways (killed, exited, its session or the
        // server's): whatever the way, a wait on it is answered within the second.
        self.wake_waits();
        if self.opts.event_log && self.events_pruned.is_none_or(|t| t.elapsed() >= Duration::from_secs(86_400)) {
            self.events_pruned = Some(now);
            crate::histlog::prune_events(self.events_dir(), self.opts.event_log_days);
        }
    }

    /// This server's event log: a directory per socket, so servers side by
    /// side (`-L`) never read each other's messages as their own.
    fn events_dir(&self) -> std::path::PathBuf {
        crate::histlog::events_dir().join(crate::histlog::safe_name(&self.socket))
    }

    /// The event log where its options say (nowhere when `event-log` is off).
    pub(super) fn sync_event_log(&mut self) {
        self.observe.dir = self.opts.event_log.then(|| self.events_dir());
        self.observe.cap = self.opts.event_log_max;
    }

    /// At start: the event log's options, and what it says of the past.
    pub(super) fn mail_start(&mut self) {
        self.sync_event_log();
        let dropped = self.observe.load(1);
        self.next_msg = self.observe.last_id();
        if dropped > 0 {
            self.note_message(&format!(
                "{dropped} undelivered messages were dropped when the server restarted; see the event log"
            ));
        }
    }

    /// A client went away: its waits go with it.
    pub(super) fn mail_client_gone(&mut self, cid: ClientId) {
        self.msg_waits.retain(|w| w.cid != cid);
    }
}

#[cfg(test)]
mod tests {
    use super::unheard_advice;

    /// The advice names the agent the pane runs, when it is one `setup`
    /// knows (Codex needs its hook trusted too), else points at the list.
    #[test]
    fn the_advice_names_the_agent_it_can() {
        let codex = unheard_advice(7, Some("codex"));
        assert!(codex.starts_with("%7 has not said it is free"), "{codex}");
        assert!(codex.ends_with("keepane setup codex --install, then trust it in Codex (/hooks)"), "{codex}");
        assert!(unheard_advice(7, Some("C:\\bin\\gemini.exe")).ends_with("keepane setup gemini --install"));
        assert!(unheard_advice(7, Some("cursor-agent")).ends_with("keepane setup cursor --install"));
        assert!(unheard_advice(7, Some("node")).ends_with("keepane setup (it shows which agents lack it)"));
        assert!(unheard_advice(7, None).ends_with("keepane setup (it shows which agents lack it)"));
    }
}
