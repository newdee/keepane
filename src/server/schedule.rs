//! Messages sent later (`send-message --in` / `--at`, docs/design/mailbox.md
//! §15): kept until due in a file of their own, so a restart sends them
//! still (late, and saying so); the pane they go to is found by its address
//! when they are due; `list-messages` shows them and `drop-message` cancels
//! them.

use super::actor::{Message, MsgId, Sender, WorkMode};
use super::{ClientId, Outcome, PaneId, Server};
use crate::command::{Due, Target};

type Time = chrono::DateTime<chrono::Local>;

/// The most messages scheduled at once, whoever sent them: an agent in a
/// loop cannot fill the disk with them.
pub const SCHEDULED_MOST: usize = 1000;

/// Sent this much after it was due, a message says how late it is (the
/// server was not running, or the machine slept).
const LATE_AFTER: i64 = 60;

/// Room kept in a scheduled message for that line (`message-max-size`
/// less this), so a late one is never refused for keepane's own words.
const LATE_ROOM: usize = 100;

/// What a sender is told of a message that could not go: its text, this
/// many characters of it at most.
const QUOTE_MOST: usize = 200;

/// A message waiting for its time: what it will say and who sends it
/// (`msg`, its number already given), the pane it goes to as it was given
/// (`%builder`, `work:0.1`, `host:port/%x`), found when it is due.
#[derive(Clone, Debug)]
pub struct Scheduled {
    pub msg: Message,
    pub target: String,
    pub due: Time,
}

/// What the scheduled file keeps of each: the envelope and text as the
/// event log writes them, the target and the time.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    msg: serde_json::Value,
    text: String,
    target: String,
    due: String,
    made: String,
}

/// How long `secs` is, said shortly: `25m`, `2h 3m`, `3d 4h`.
pub fn span(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 if s % 3600 / 60 == 0 => format!("{}h", s / 3600),
        3600..86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ if s % 86_400 / 3600 == 0 => format!("{}d", s / 86_400),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

impl Server {
    /// Where the scheduled messages are kept: beside the saved sessions, in
    /// a folder of their own (every `.json` beside them is a session).
    fn scheduled_file(&self) -> std::path::PathBuf {
        self.sessions_dir().join("scheduled").join(format!("{}.json", crate::histlog::safe_name(&self.socket)))
    }

    /// The scheduled messages written out whole, after every change. When
    /// `sessions-dir` changed since they were last read or written, the file
    /// where they were goes: left there, it would bring back messages sent
    /// since, should the option be set back.
    fn scheduled_save(&mut self) {
        let path = self.scheduled_file();
        if let Some(was) = self.scheduled_was.replace(path.clone())
            && was != path
        {
            let _ = std::fs::remove_file(was);
        }
        let kept: Vec<Kept> = self
            .scheduled
            .iter()
            .map(|s| Kept {
                msg: serde_json::from_str(&s.msg.envelope()).unwrap_or_default(),
                text: s.msg.text.clone(),
                target: s.target.clone(),
                due: s.due.to_rfc3339(),
                made: s.msg.at.to_rfc3339(),
            })
            .collect();
        if kept.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let text = serde_json::to_string_pretty(&kept).expect("it serializes");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = super::chat::write_whole(&path, &text) {
            log::warn!("scheduled messages: {e}");
        }
    }

    /// At start: the scheduled messages the file keeps, armed again; one
    /// the event log says is scheduled that the file no longer has is
    /// dropped, said so. Message numbers go on past theirs.
    pub(super) fn scheduled_start(&mut self) {
        let path = self.scheduled_file();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.scheduled_was = Some(path);
        let kept: Vec<Kept> = serde_json::from_str(&text).unwrap_or_default();
        self.scheduled = kept
            .into_iter()
            .filter_map(|k| {
                let made = chrono::DateTime::parse_from_rfc3339(&k.made).ok()?.into();
                let due = chrono::DateTime::parse_from_rfc3339(&k.due).ok()?.into();
                let msg = super::observe::message_of(&k.msg, &k.text, made)?;
                Some(Scheduled { msg, target: k.target, due })
            })
            .collect();
        let lost: Vec<MsgId> = self
            .observe
            .records()
            .filter(|r| r.stage == super::observe::Stage::Scheduled)
            .map(|r| r.msg.id)
            .filter(|id| !self.scheduled.iter().any(|s| s.msg.id == *id))
            .collect();
        for id in lost {
            self.observe.dropped(id, "not kept over the restart");
        }
        let highest = self.scheduled.iter().map(|s| s.msg.id).max().unwrap_or(0);
        self.next_msg = self.next_msg.max(highest);
    }

    /// `send-message --in` / `--at`: the message made now (its number, who
    /// sends it, its chain), sent when `due` is. `target` is the pane given
    /// (`--to`), or, answering, the sender of what is answered, here.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn schedule_send(
        &mut self,
        cid: Option<ClientId>,
        target: Option<&Target>,
        reply: bool,
        answering: Option<&Message>,
        carry: Option<(MsgId, u32)>,
        re: Option<MsgId>,
        text: String,
        due: &Due,
    ) -> Outcome {
        let now = chrono::Local::now();
        let due = match crate::command::due_time(due, now) {
            Ok(t) => t,
            Err(e) => return Outcome::Error(format!("send-message: {e}")),
        };
        if self.scheduled.len() >= SCHEDULED_MOST {
            return Outcome::Error(format!(
                "send-message: {SCHEDULED_MOST} messages are scheduled already, the most (drop-message cancels one)"
            ));
        }
        let most = self.opts.message_max_size.saturating_sub(LATE_ROOM);
        if text.len() > most {
            return Outcome::Error(format!(
                "send-message: the message is {} bytes; a scheduled one holds {most} (message-max-size {} less {LATE_ROOM}, room to say it is late)",
                text.len(),
                self.opts.message_max_size
            ));
        }
        // The pane, as it will be looked for when the message is due: the
        // address given; answering, the sender's pane by its number.
        let target = if reply {
            let Some(cur) = answering else {
                return Outcome::Error("send-message -r: no message to answer".into());
            };
            match (&cur.from, cur.sender_pane()) {
                (Sender::Remote { addr, .. }, _) => {
                    return Outcome::Error(format!(
                        "send-message: #{} came from {addr}; an answer there is not scheduled (send it now)",
                        cur.id
                    ));
                }
                (_, Some(p)) => format!("%{p}"),
                (_, None) => return Outcome::Error(format!("send-message: #{} came from outside any pane", cur.id)),
            }
        } else {
            let Some(t) = target else { return Outcome::Error("send-message: --to pane (or -t) required".into()) };
            // A pane here must be there now: a typo is said at once, not when
            // it is due.
            if t.remote.is_none()
                && let Err(e) = self.mail_target(Some(t), cid)
            {
                return Outcome::Error(e);
            }
            crate::command::target_string(t)
        };
        let via =
            self.scheduled_pane(&target).and_then(|p| self.pane_ref(p)).map_or(WorkMode::Normal, |p| p.actor.mode);
        self.next_msg += 1;
        let id = self.next_msg;
        let (task, hop) = carry.unwrap_or((id, 0));
        if hop > self.opts.message_hop_limit {
            return Outcome::Error(format!(
                "send-message: hop {hop} is over message-hop-limit {} (a loop between panes?)",
                self.opts.message_hop_limit
            ));
        }
        let msg = Message {
            id,
            task,
            from: self.sender_of(cid),
            to: target.clone(),
            via,
            hop,
            re: if reply || re.is_some() { answering.map(|c| c.id) } else { None },
            text,
            at: chrono::SubsecRound::trunc_subsecs(now, 3),
        };
        self.observe.scheduled(&msg, &target, due);
        self.scheduled.push(Scheduled { msg, target: target.clone(), due });
        self.scheduled_save();
        Outcome::Text(format!(
            "#{id} scheduled for {target} at {} (in {})",
            due.format("%Y-%m-%d %H:%M"),
            span((due - now).num_seconds())
        ))
    }

    /// The pane here a scheduled message's target names now, if any (one of
    /// another machine is none).
    pub(super) fn scheduled_pane(&self, target: &str) -> Option<PaneId> {
        let t = Target::parse(target);
        if t.remote.is_some() {
            return None;
        }
        self.resolve(Some(&t), None).ok().map(|(_, _, p)| p)
    }

    /// From the tick: those whose time has come, sent; one sent well after
    /// its time says so first.
    pub(super) fn scheduled_tick(&mut self) {
        let now = chrono::Local::now();
        for s in self.take_due(now) {
            self.scheduled_fire(s, now);
        }
    }

    /// Those due at `now`, taken out (and the file written without them),
    /// in the order they were due: after a restart several may be, and the
    /// one due first goes first, whichever was scheduled first.
    fn take_due(&mut self, now: Time) -> Vec<Scheduled> {
        if !self.scheduled.iter().any(|s| s.due <= now) {
            return Vec::new();
        }
        let (mut due, waiting): (Vec<Scheduled>, Vec<Scheduled>) =
            std::mem::take(&mut self.scheduled).into_iter().partition(|s| s.due <= now);
        self.scheduled = waiting;
        self.scheduled_save();
        due.sort_by_key(|s| s.due);
        due
    }

    fn scheduled_fire(&mut self, s: Scheduled, now: Time) {
        let Scheduled { mut msg, target, due } = s;
        let late = (now - due).num_seconds();
        if late > LATE_AFTER {
            msg.text = format!(
                "(scheduled for {}, sent {} late: keepane was not running then)\n{}",
                due.format("%Y-%m-%d %H:%M"),
                span(late),
                msg.text
            );
        }
        let id = msg.id;
        let t = Target::parse(&target);
        // Another machine's pane: handed to it, as a message sent now is; the
        // number it goes out with is a new one, said on this one.
        if let Some(machine) = t.remote.clone() {
            let there = crate::command::target_string(&Target { remote: None, ..t });
            let out = self.send_remote(
                None,
                &machine,
                &there,
                msg.text.clone(),
                msg.from.clone(),
                Some((msg.task, msg.hop)),
                None,
                None,
            );
            match out {
                Outcome::Error(e) => {
                    self.observe.refused(id, &e);
                    self.scheduled_failed(&msg, &target, &e);
                }
                Outcome::Text(t) => {
                    self.observe.forwarded(id, &format!("sent as {}", t.lines().next().unwrap_or_default()))
                }
                _ => self.observe.forwarded(id, "sent"),
            }
            return;
        }
        let Some(pane) = self.scheduled_pane(&target) else {
            let why = format!("no pane {target} when it was due");
            self.observe.refused(id, &why);
            self.scheduled_failed(&msg, &target, &why);
            return;
        };
        msg.to = self.address_of(pane);
        msg.via = self.pane_ref(pane).map_or(WorkMode::Normal, |p| p.actor.mode);
        msg.at = chrono::SubsecRound::trunc_subsecs(now, 3);
        if let Err(why) = self.enqueue(pane, msg.clone()) {
            self.scheduled_failed(&msg, &target, &why);
        }
    }

    /// A scheduled message that could not go: its sender, when a pane here,
    /// told in its inbox (it may wait for an answer that will never come).
    fn scheduled_failed(&mut self, msg: &Message, target: &str, why: &str) {
        let Some(p) = msg.sender_pane().filter(|p| self.place_of(*p).is_some()) else { return };
        self.next_msg += 1;
        let id = self.next_msg;
        // Its own last line: a line saying how late it is comes first.
        let last = msg.text.lines().last().unwrap_or_default();
        let mut said: String = last.chars().take(QUOTE_MOST).collect();
        if last.chars().count() > QUOTE_MOST {
            said.push('…');
        }
        let note = Message {
            id,
            task: id,
            from: Sender::User,
            to: self.address_of(p),
            via: self.pane_ref(p).map_or(WorkMode::Normal, |x| x.actor.mode),
            hop: 0,
            re: Some(msg.id),
            text: format!("keepane: scheduled message #{} to {target} was not sent: {why}. It said: {said}", msg.id),
            at: chrono::SubsecRound::trunc_subsecs(chrono::Local::now(), 3),
        };
        let _ = self.enqueue(p, note);
    }

    /// `drop-message ID` for one scheduled: cancelled. None when `id` is not
    /// scheduled.
    pub(super) fn scheduled_cancel(&mut self, id: MsgId, by: &str) -> Option<Outcome> {
        let at = self.scheduled.iter().position(|s| s.msg.id == id)?;
        let s = self.scheduled.remove(at);
        self.scheduled_save();
        self.observe.dropped(id, &format!("cancelled by {by}"));
        Some(Outcome::Text(format!(
            "#{id} cancelled (it was scheduled for {} at {})",
            s.target,
            s.due.format("%Y-%m-%d %H:%M")
        )))
    }

    /// The scheduled messages for pane `pid` (all of them with `None`): the
    /// lines `list-messages` shows under its inbox, soonest first.
    pub(super) fn scheduled_for(&self, pid: Option<PaneId>) -> Vec<&Scheduled> {
        let mut out: Vec<&Scheduled> =
            self.scheduled.iter().filter(|s| pid.is_none_or(|p| self.scheduled_pane(&s.target) == Some(p))).collect();
        out.sort_by_key(|s| s.due);
        out
    }

    /// One scheduled message as `list-messages` shows it.
    pub(super) fn scheduled_line(&self, s: &Scheduled, now: Time) -> String {
        let from = match &s.msg.from {
            Sender::User => "user".to_string(),
            f => f.from_field(),
        };
        let first = s.msg.text.lines().next().unwrap_or_default();
        let more = if s.msg.text.lines().nth(1).is_some() { " …" } else { "" };
        format!(
            "  #{}  from {from}  scheduled for {} (in {}) to {}  {first}{more}",
            s.msg.id,
            s.due.format("%Y-%m-%d %H:%M"),
            span((s.due - now).num_seconds()),
            s.target
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(dir: &std::path::Path) -> Server {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "schedule-test".into(), events);
        s.opts.sessions_dir = dir.to_string_lossy().into_owned();
        s.scheduled_start();
        s
    }

    /// Scheduled messages are kept in their file: a server started on it
    /// has them, with what they say, whom for and when, its numbers going
    /// on past theirs; one cancelled is gone from the file; one the event
    /// log says is scheduled that the file no longer has is dropped.
    #[test]
    fn scheduled_messages_are_kept_over_a_restart() {
        let dir = std::env::temp_dir().join(format!("keepane-schedule-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        // A pane of another machine is not looked for now: no pane needed.
        let to = Target::parse("10.0.0.2:7681/%builder");
        let say = |a: &mut Server, text: &str, due: Due| {
            let out = a.schedule_send(None, Some(&to), false, None, None, None, text.into(), &due);
            match out {
                Outcome::Text(t) => t,
                Outcome::Error(e) => panic!("{e}"),
                _ => panic!("no text"),
            }
        };
        let out = say(&mut a, "check the build", Due::In(3600));
        assert!(out.contains("scheduled for 10.0.0.2:7681/%builder at") && out.ends_with("(in 1h)"), "{out}");
        say(&mut a, "two\nlines", Due::In(7200));
        let cancel = say(&mut a, "never", Due::In(60));
        let cancel: MsgId = cancel[1..].split_whitespace().next().unwrap().parse().unwrap();
        assert!(matches!(a.scheduled_cancel(cancel, "user"), Some(Outcome::Text(_))));
        assert!(a.scheduled_cancel(cancel, "user").is_none(), "cancelled once");
        let file = dir.join("scheduled").join("schedule-test.json");
        assert!(file.exists(), "kept beside the sessions, in a folder of its own");
        let mut b = server(&dir);
        let kept: Vec<(String, String)> = b.scheduled.iter().map(|s| (s.target.clone(), s.msg.text.clone())).collect();
        assert_eq!(
            kept,
            [
                ("10.0.0.2:7681/%builder".to_string(), "check the build".to_string()),
                ("10.0.0.2:7681/%builder".to_string(), "two\nlines".to_string())
            ]
        );
        assert_eq!(b.scheduled[0].due, a.scheduled[0].due, "when, to the second");
        assert_eq!(b.scheduled[0].msg.from, Sender::User);
        let kept_most = b.scheduled.iter().map(|s| s.msg.id).max().unwrap();
        assert!(b.next_msg >= kept_most, "numbers go on past those kept");
        // The event log says one is scheduled that the file does not have.
        let lost = Message { id: 99, ..b.scheduled[0].msg.clone() };
        b.observe.scheduled(&lost, "%gone", chrono::Local::now());
        b.scheduled_start();
        assert_eq!(b.observe.get(99).map(|r| r.stage), Some(super::super::observe::Stage::Dropped));
        // `sessions-dir` changed: the next write goes there, and the file
        // where they were goes (set back, it would bring them back twice).
        let moved = dir.join("moved");
        b.opts.sessions_dir = moved.to_string_lossy().into_owned();
        b.schedule_send(None, Some(&to), false, None, None, None, "after the move".into(), &Due::In(60));
        let moved_file = moved.join("scheduled").join("schedule-test.json");
        assert!(moved_file.exists() && !file.exists(), "kept where the option says now, and only there");
        assert_eq!(server(&moved).scheduled.len(), 3, "all of them, moved");
        let file = moved_file;
        // All cancelled: no file left.
        let ids: Vec<MsgId> = b.scheduled.iter().map(|s| s.msg.id).collect();
        for id in ids {
            b.scheduled_cancel(id, "user");
        }
        assert!(!file.exists(), "nothing scheduled: no file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What is refused now, not when due: too big, too many hops on, and one
    /// more than `SCHEDULED_MOST` scheduled at once.
    #[test]
    fn a_scheduled_message_keeps_the_limits() {
        let dir = std::env::temp_dir().join(format!("keepane-schedule-limits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        let to = Target::parse("10.0.0.2:7681/%builder");
        let say = |a: &mut Server, text: String, carry: Option<(MsgId, u32)>| match a.schedule_send(
            None,
            Some(&to),
            false,
            None,
            carry,
            None,
            text,
            &Due::In(60),
        ) {
            Outcome::Error(e) => e,
            Outcome::Text(t) => t,
            _ => String::new(),
        };
        let big = "x".repeat(a.opts.message_max_size + 1);
        assert!(say(&mut a, big, None).contains("message-max-size"));
        // Room is kept for the line saying it is late.
        let near = "x".repeat(a.opts.message_max_size - LATE_ROOM + 1);
        assert!(say(&mut a, near, None).contains("room to say it is late"));
        let over = a.opts.message_hop_limit + 1;
        assert!(say(&mut a, "x".into(), Some((1, over))).contains("message-hop-limit"));
        assert!(a.scheduled.is_empty(), "nothing kept of those refused");
        let one = say(&mut a, "x".into(), None);
        assert!(one.contains("scheduled for"), "{one}");
        let kept = a.scheduled[0].clone();
        a.scheduled = vec![kept; SCHEDULED_MOST];
        assert!(say(&mut a, "x".into(), None).contains("the most"), "{SCHEDULED_MOST} at once at most");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// After a restart several may be due at once: they go in the order they
    /// were due, not the order they were scheduled in; those not due stay,
    /// and the file keeps them alone.
    #[test]
    fn those_due_go_in_the_order_they_were_due() {
        let dir = std::env::temp_dir().join(format!("keepane-schedule-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = server(&dir);
        let to = Target::parse("10.0.0.2:7681/%builder");
        for (text, secs) in [("in two hours", 7200), ("in one hour", 3600), ("tomorrow", 86_400)] {
            a.schedule_send(None, Some(&to), false, None, None, None, text.into(), &Due::In(secs));
        }
        let later = chrono::Local::now() + chrono::Duration::hours(3);
        let due: Vec<String> = a.take_due(later).into_iter().map(|s| s.msg.text).collect();
        assert_eq!(due, ["in one hour", "in two hours"], "the one due first, first");
        assert_eq!(a.scheduled.len(), 1);
        assert_eq!(server(&dir).scheduled.len(), 1, "the file keeps the one still to come");
        assert!(a.take_due(chrono::Local::now()).is_empty(), "nothing due now");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_span_reads_short() {
        assert_eq!(span(-5), "0s");
        assert_eq!(span(59), "59s");
        assert_eq!(span(25 * 60), "25m");
        assert_eq!(span(2 * 3600), "2h");
        assert_eq!(span(2 * 3600 + 3 * 60), "2h 3m");
        assert_eq!(span(86_400), "1d");
        assert_eq!(span(3 * 86_400 + 4 * 3600 + 59), "3d 4h");
    }
}
