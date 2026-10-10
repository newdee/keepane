//! A pane as an actor: its name, its work mode, its inbox, and whether it
//! is free to take the next message. Pure state and no IO: the server
//! tells it what happened (keepane's prompt came back, a key was typed,
//! the agent said `pane-ready`) and asks it what to deliver. Why each rule
//! is so is in docs/design/mailbox.md.

use std::collections::VecDeque;

use super::layout::PaneId;

pub type MsgId = u64;

/// The envelope's version (`"keepane":1`). Adding a field does not change
/// it, since readers skip fields they do not know; changing what a field
/// means does.
pub const ENVELOPE_VERSION: u32 = 1;

/// How a pane takes its messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkMode {
    /// Nothing is delivered: the program takes messages with `read-message`.
    #[default]
    Normal,
    /// Delivered as a command when keepane's own prompt hook says the
    /// shell is back at its prompt.
    Shell,
    /// Delivered as a prompt when the agent says `pane-ready`.
    Ai,
}

impl WorkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkMode::Normal => "normal",
            WorkMode::Shell => "shell",
            WorkMode::Ai => "ai",
        }
    }

    pub fn parse(s: &str) -> Option<WorkMode> {
        match s {
            "normal" => Some(WorkMode::Normal),
            "shell" => Some(WorkMode::Shell),
            "ai" => Some(WorkMode::Ai),
            _ => None,
        }
    }
}

/// Who sent a message: a pane, as it was when it sent it, someone outside
/// every pane (a terminal, a key binding), or a pane of another machine
/// (docs/design/link.md: `addr` is that machine, `host:port`, and
/// `address` the pane's full address there, or `user` for someone outside
/// its panes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sender {
    User,
    Pane { id: PaneId, address: String, name: Option<String>, mode: WorkMode },
    Remote { addr: String, address: String, name: Option<String>, mode: Option<WorkMode> },
}

impl Sender {
    /// How `from` reads: `user`, a pane's address, or `host:port/address`.
    pub fn from_field(&self) -> String {
        match self {
            Sender::User => "user".into(),
            Sender::Pane { address, .. } => address.clone(),
            Sender::Remote { addr, address, .. } => format!("{addr}/{address}"),
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Sender::User => None,
            Sender::Pane { name, .. } | Sender::Remote { name, .. } => name.as_deref(),
        }
    }

    pub fn mode(&self) -> Option<WorkMode> {
        match self {
            Sender::User => None,
            Sender::Pane { mode, .. } => Some(*mode),
            Sender::Remote { mode, .. } => *mode,
        }
    }

    /// The sender's name, else its address, the way a person is shown it.
    pub fn short(&self) -> String {
        self.name().map_or_else(|| self.from_field(), |n| n.to_string())
    }

    /// The sender as a target names it, for the short header in a shell:
    /// `%name`, else a pane's `%id`, else `user`; on another machine the
    /// same after `host:port/`. No space, quote mark or `#>` in it.
    pub fn tag(&self) -> String {
        match self {
            Sender::User => "user".into(),
            Sender::Pane { id, name, .. } => name.as_ref().map_or_else(|| format!("%{id}"), |n| format!("%{n}")),
            Sender::Remote { addr, address, name, .. } => {
                name.as_ref().map_or_else(|| format!("{addr}/{address}"), |n| format!("{addr}/%{n}"))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: MsgId,
    /// The message that started the chain this one belongs to.
    pub task: MsgId,
    pub from: Sender,
    /// The recipient's full address when it was sent.
    pub to: String,
    /// The recipient's work mode when it was sent. The message is
    /// delivered only in that mode: text written for an agent is never
    /// run as a command because the pane was switched to `shell` meanwhile.
    pub via: WorkMode,
    pub hop: u32,
    /// The message this one answers.
    pub re: Option<MsgId>,
    pub text: String,
    /// When it was sent. Not part of the envelope, so the envelope reads
    /// the same wherever it is shown.
    pub at: chrono::DateTime<chrono::Local>,
}

fn push_str_field(s: &mut String, key: &str, value: &str) {
    s.push_str(&format!(",\"{key}\":{}", serde_json::to_string(value).expect("a string always serializes")));
}

impl Message {
    /// The one-line JSON header, the same bytes everywhere the message is
    /// shown: fixed field order, no spaces, absent fields left out.
    pub fn envelope(&self) -> String {
        let mut s = format!("{{\"keepane\":{ENVELOPE_VERSION},\"id\":{},\"task\":{}", self.id, self.task);
        push_str_field(&mut s, "from", &self.from.from_field());
        if let Some(n) = self.from.name() {
            push_str_field(&mut s, "name", n);
        }
        if let Some(m) = self.from.mode() {
            push_str_field(&mut s, "mode", m.as_str());
        }
        push_str_field(&mut s, "to", &self.to);
        push_str_field(&mut s, "via", self.via.as_str());
        s.push_str(&format!(",\"hop\":{}", self.hop));
        if let Some(re) = self.re {
            s.push_str(&format!(",\"re\":{re}"));
        }
        s.push('}');
        s
    }

    /// The envelope's fields as the text a pane or a person reads: the
    /// same fields in the same order as `envelope`, `name=value` apart:
    /// `[keepane id=12 task=3 from=$1:@3.%7 name=builder mode=ai to=$1:@4.%9
    /// via=shell hop=1 re=9]`. No value holds a space or a `]` (names are
    /// letters, digits, `-` and `_`; addresses and words have none), so it
    /// reads back by splitting.
    pub fn fields(&self) -> String {
        let mut s = format!("[keepane id={} task={} from={}", self.id, self.task, self.from.from_field());
        if let Some(n) = self.from.name() {
            s.push_str(&format!(" name={n}"));
        }
        if let Some(m) = self.from.mode() {
            s.push_str(&format!(" mode={}", m.as_str()));
        }
        s.push_str(&format!(" to={} via={} hop={}", self.to, self.via.as_str(), self.hop));
        if let Some(re) = self.re {
            s.push_str(&format!(" re={re}"));
        }
        s.push(']');
        s
    }

    /// The header in `style`: the fields, or the JSON envelope.
    pub fn header(&self, style: EnvelopeStyle) -> String {
        match style {
            EnvelopeStyle::Fields => self.fields(),
            EnvelopeStyle::Json => self.envelope(),
        }
    }

    /// The line after the text of a message given to an agent: a header
    /// forged inside the text shows up between the real header and this.
    pub fn end_line(&self, style: EnvelopeStyle) -> String {
        match style {
            EnvelopeStyle::Fields => format!("[keepane end={}]", self.id),
            EnvelopeStyle::Json => format!("{{\"keepane\":{ENVELOPE_VERSION},\"end\":{}}}", self.id),
        }
    }

    /// The header in front of a command typed into a shell: who sent it and
    /// its id, short so the line stays readable (`keepane #12 from
    /// %builder`); the rest is in the event log (`trace-message 12`). The
    /// JSON style keeps the whole JSON envelope there, as before 0.17.
    pub fn shell_header(&self, style: EnvelopeStyle) -> String {
        match style {
            EnvelopeStyle::Fields => format!("keepane #{} from {}", self.id, self.from.tag()),
            EnvelopeStyle::Json => self.envelope(),
        }
    }

    /// What is typed into the pane to deliver it (Enter follows). A shell
    /// gets the short header in front of the command in a form that runs
    /// nothing and stays in the history (`syntax`: the language of the
    /// shell at the prompt); an agent gets the header, the text, and the
    /// end line.
    pub fn wrapped(&self, syntax: Syntax, style: EnvelopeStyle) -> String {
        let header = match self.via {
            WorkMode::Shell => self.shell_header(style),
            WorkMode::Ai | WorkMode::Normal => self.header(style),
        };
        match (self.via, syntax) {
            (WorkMode::Shell, Syntax::PowerShell) => format!("<# {header} #> {}", one_command(&self.text)),
            (WorkMode::Shell, Syntax::Posix) => {
                format!(": {}; {}", posix_quote(&header), posix_one_command(&self.text))
            }
            (WorkMode::Ai | WorkMode::Normal, _) => format!("{header}\n{}\n{}", self.text, self.end_line(style)),
        }
    }

    /// The pane that sent it, if a pane of this machine did.
    pub fn sender_pane(&self) -> Option<PaneId> {
        match self.from {
            Sender::Pane { id, .. } => Some(id),
            Sender::User | Sender::Remote { .. } => None,
        }
    }
}

/// A PowerShell command of several lines as one line that runs them as
/// one command, in the shell's own scope (`.`), as typed by hand. Typed
/// as they are, each line would run on its own at its Enter (PSReadLine
/// asks for no bracketed paste, and ConPTY drops the Shift of a
/// Shift+Enter), and the message would be over at the first prompt. Each
/// line goes in single quotes, the quote marks PowerShell reads as single
/// quotes doubled; whether it failed is reported as for any command. One
/// line is left as it is.
pub fn one_command(text: &str) -> String {
    let text = text.trim_end_matches(['\r', '\n']);
    if !text.contains('\n') {
        return text.to_string();
    }
    let quoted: Vec<String> = text
        .split('\n')
        .map(|l| {
            let mut q = String::from("'");
            for c in l.trim_end_matches('\r').chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    q.push(c);
                }
                q.push(c);
            }
            q.push('\'');
            q
        })
        .collect();
    // `$?` after it is the block's call, true whatever failed inside: the
    // last part makes it false when an error was recorded meanwhile ($Error[0]
    // compared as an object, since a full $Error keeps its count).
    format!(
        "$__keepane_e = $Error[0]; . ([scriptblock]::Create(({}) -join \"`n\")); \
         if (-not [object]::ReferenceEquals($Error[0], $__keepane_e)) {{ Write-Error \"a line above failed\" -ErrorAction SilentlyContinue }}",
        quoted.join(", ")
    )
}

/// How a message's header is written where it is read (the delivery, what
/// `read-message` and `trace-message` print): `message-envelope`. The
/// event log keeps the JSON envelope whatever this is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnvelopeStyle {
    /// `[keepane id=12 task=3 from=… to=… via=… hop=0]`.
    #[default]
    Fields,
    /// `{"keepane":1,"id":12,"task":3,…}`, as before 0.17.
    Json,
}

impl EnvelopeStyle {
    pub fn parse(s: &str) -> Option<EnvelopeStyle> {
        match s {
            "fields" => Some(EnvelopeStyle::Fields),
            "json" => Some(EnvelopeStyle::Json),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EnvelopeStyle::Fields => "fields",
            EnvelopeStyle::Json => "json",
        }
    }
}

/// The command language of the shell at a pane's prompt, as its keepane
/// hook said (`OSC 7777;keepane-prompt`, and `;sh` from the bash and zsh
/// hooks): what a shell message is typed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Syntax {
    #[default]
    PowerShell,
    Posix,
}

/// `s` in POSIX single quotes (a `'` inside becomes `'\''`).
fn posix_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A bash or zsh command of several lines as one line: the lines, quoted,
/// given to `eval` as one script, in the shell's own scope. Its status is the
/// script's, the last command's. One line is left as it is.
pub fn posix_one_command(text: &str) -> String {
    let text = text.trim_end_matches(['\r', '\n']);
    if !text.contains('\n') {
        return text.to_string();
    }
    let quoted: Vec<String> = text.split('\n').map(|l| posix_quote(l.trim_end_matches('\r'))).collect();
    format!("eval \"$(printf '%s\\n' {})\"", quoted.join(" "))
}

/// How the message a pane was working on came to an end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The shell's prompt came back, or the agent said it was ready, or
    /// the program took its next message.
    Done,
    /// An agent's pane showed a shell prompt: the agent is gone, and the
    /// work went with it.
    Abandoned,
}

/// Where to move a queued message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Top,
}

#[derive(Debug, Default)]
pub struct Actor {
    pub name: Option<String>,
    pub mode: WorkMode,
    /// The pane that created this one (through a command run in it, or
    /// MCP), for what a pane may change and how many it may create.
    pub creator: Option<PaneId>,
    pub inbox: VecDeque<Message>,
    /// The message this pane was given last and is still working on.
    pub current: Option<Message>,
    /// The message it took last with `read-message`, for answering it.
    pub last_read: Option<Message>,
    /// What the pane last said it is doing (`pane-status`), and when.
    pub status: Option<(String, chrono::DateTime<chrono::Local>)>,
    /// keepane's own prompt is on the screen, and nothing was typed since.
    at_prompt: bool,
    /// The agent said it is ready, and nothing was typed (and no shell
    /// prompt came back) since.
    ready: bool,
    /// The agent here has said it is ready at least once since it started
    /// (or since the pane went into `ai`): one that never has most likely
    /// has no hook to say it, and its messages would wait for ever.
    heard: bool,
    /// Someone typed while the command keepane gave the pane was running:
    /// answering it, or typing ahead, which the shell shows at its next
    /// prompt. That prompt does not make the pane free, so no message is
    /// typed after what is on its line.
    typed_while_working: bool,
    /// What was typed while the command ran is on the prompt's line: the
    /// pane stays busy through any number of prompts (a shell redraws its
    /// prompt when typed-ahead text comes in: bash does, and says so each
    /// time) until someone types again: runs it, or clears it.
    line_left: bool,
}

impl Actor {
    /// An agent's pane whose agent has not said it is ready once since it
    /// started: the messages queued for it wait on a hook it may not have.
    pub fn unheard(&self) -> bool {
        self.mode == WorkMode::Ai && !self.heard
    }

    /// The pane's mode changed: an agent already running says nothing new
    /// by that, so it is heard from again only at its next `pane-ready`.
    pub fn set_mode(&mut self, mode: WorkMode) {
        if mode != self.mode {
            self.heard = false;
        }
        self.mode = mode;
    }

    /// Free to take the next message, by the one signal this mode trusts.
    pub fn idle(&self) -> bool {
        match self.mode {
            WorkMode::Normal => false,
            WorkMode::Shell => self.at_prompt,
            WorkMode::Ai => self.ready,
        }
    }

    /// keepane's prompt hook said the shell is at its prompt. In a shell
    /// pane that ends the command it was given; in an agent's pane it
    /// means the agent has exited, and its work ends unfinished.
    pub fn prompt(&mut self) -> Option<(Message, End)> {
        self.prompt_seen();
        self.command_over()
    }

    /// The first half of `prompt`, at the marker itself: the pane is at its
    /// prompt from now on, unless someone types (in order, after this).
    pub fn prompt_seen(&mut self) {
        if self.current.is_some() && std::mem::take(&mut self.typed_while_working) {
            self.line_left = true;
        }
        self.at_prompt = !self.line_left;
        self.ready = false;
        // In an agent's pane a shell prompt means the agent is gone: the
        // next one started there has yet to be heard from.
        if self.mode == WorkMode::Ai {
            self.heard = false;
        }
    }

    /// The second half, once what the command printed is on the screen:
    /// the message it was working on ends.
    pub fn command_over(&mut self) -> Option<(Message, End)> {
        match self.mode {
            WorkMode::Shell => self.current.take().map(|m| (m, End::Done)),
            WorkMode::Ai => self.current.take().map(|m| (m, End::Abandoned)),
            WorkMode::Normal => None,
        }
    }

    /// The program in the pane said `pane-ready`. Only an agent's word
    /// counts: a shell pane's idleness is keepane's to judge, and a normal
    /// pane takes nothing on its own. Returns whether it was taken.
    pub fn ready(&mut self) -> (bool, Option<(Message, End)>) {
        if self.mode != WorkMode::Ai {
            return (false, None);
        }
        self.ready = true;
        self.heard = true;
        (true, self.current.take().map(|m| (m, End::Done)))
    }

    /// Someone typed, pasted or sent keys into the pane: it is busy until
    /// the next signal, so a message does not land in a line being typed.
    pub fn input(&mut self) {
        self.at_prompt = false;
        self.ready = false;
        // Someone is at the line now: its next prompt is a free one.
        self.line_left = false;
        if self.current.is_some() {
            self.typed_while_working = true;
        }
    }

    /// The pane's program was started again in place (`respawn-pane`): its
    /// name, mode and inbox stay, it is not at a prompt (or ready) until the
    /// new program says so, and the message the old one was working on is
    /// over, unfinished; it comes back to be recorded.
    pub fn restarted(&mut self) -> Option<Message> {
        self.at_prompt = false;
        self.ready = false;
        self.heard = false;
        self.typed_while_working = false;
        self.line_left = false;
        self.current.take()
    }

    /// A person unsticks a pane that stays busy (text typed and deleted
    /// again, a prompt that was not redrawn).
    pub fn force_idle(&mut self) {
        self.line_left = false;
        match self.mode {
            WorkMode::Shell => self.at_prompt = true,
            WorkMode::Ai => self.ready = true,
            WorkMode::Normal => {}
        }
    }

    /// The next message to deliver now, if the pane is free and one was
    /// addressed to it in the mode it is in. Taking it makes the pane busy
    /// and the message its current one.
    pub fn next_delivery(&mut self) -> Option<Message> {
        if !self.idle() {
            return None;
        }
        let i = self.inbox.iter().position(|m| m.via == self.mode)?;
        let m = self.inbox.remove(i)?;
        self.at_prompt = false;
        self.ready = false;
        self.typed_while_working = false;
        self.current = Some(m.clone());
        Some(m)
    }

    /// `read-message`: the program takes the oldest message itself. It is
    /// done with once taken, so it is not what the pane works on: an agent
    /// taking an answer inside its turn keeps working on its own message,
    /// and a person reading one and then asking something new starts a
    /// new chain. It is only remembered, for `send-message -r`.
    pub fn read(&mut self) -> Option<Message> {
        let m = self.inbox.pop_front()?;
        self.last_read = Some(m.clone());
        Some(m)
    }

    /// What `send-message -r` answers: the message being worked on, else
    /// the one read last.
    pub fn answering(&self) -> Option<&Message> {
        self.current.as_ref().or(self.last_read.as_ref())
    }

    /// Queue a message; how many are ahead of it.
    pub fn enqueue(&mut self, m: Message, limit: usize) -> Result<usize, String> {
        if self.inbox.len() >= limit {
            return Err(format!("inbox is full (message-inbox-limit is {limit})"));
        }
        self.inbox.push_back(m);
        Ok(self.inbox.len() - 1)
    }

    /// Task and hop for one this pane sends now: sent while working on a
    /// message, it carries that chain on; an answer carries on the chain of
    /// what it answers.
    pub fn carry(&self, reply: bool) -> Option<(MsgId, u32)> {
        let from = if reply { self.answering() } else { self.current.as_ref() };
        from.map(|c| (c.task, c.hop + 1))
    }

    pub fn remove(&mut self, id: MsgId) -> Option<Message> {
        let i = self.inbox.iter().position(|m| m.id == id)?;
        self.inbox.remove(i)
    }

    /// Move a queued message; its old and new places.
    pub fn move_message(&mut self, id: MsgId, to: Move) -> Option<(usize, usize)> {
        let i = self.inbox.iter().position(|m| m.id == id)?;
        let j = match to {
            Move::Up => i.saturating_sub(1),
            Move::Down => (i + 1).min(self.inbox.len() - 1),
            Move::Top => 0,
        };
        let m = self.inbox.remove(i)?;
        self.inbox.insert(j, m);
        Some((i, j))
    }

    /// Put a deleted message back where it was (or last, if the inbox is
    /// shorter now).
    pub fn restore(&mut self, m: Message, at: usize) {
        let at = at.min(self.inbox.len());
        self.inbox.insert(at, m);
    }
}

/// A pane name: what `%name` finds. Letters, digits, `-` and `_`, so it can
/// never end the comment a shell envelope sits in, and never all digits,
/// which would read as a pane id.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err(format!("bad pane name '{name}' (1 to 64 characters)"));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("bad pane name '{name}' (letters, digits, - and _ only)"));
    }
    if name.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("bad pane name '{name}' (all digits would read as a pane id)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local.timestamp_millis_opt(1_790_000_000_000).unwrap()
    }

    fn msg(id: MsgId, via: WorkMode) -> Message {
        Message {
            id,
            task: id,
            from: Sender::Pane { id: 7, address: "$1:@3.%7".into(), name: Some("builder".into()), mode: WorkMode::Ai },
            to: "$1:@4.%9".into(),
            via,
            hop: 0,
            re: None,
            text: format!("text {id}"),
            at: at(),
        }
    }

    fn actor(mode: WorkMode) -> Actor {
        Actor { mode, ..Default::default() }
    }

    #[test]
    fn the_envelope_is_fixed_and_leaves_out_what_is_absent() {
        let mut m = msg(12, WorkMode::Shell);
        m.hop = 1;
        m.re = Some(9);
        m.task = 3;
        assert_eq!(
            m.envelope(),
            r#"{"keepane":1,"id":12,"task":3,"from":"$1:@3.%7","name":"builder","mode":"ai","to":"$1:@4.%9","via":"shell","hop":1,"re":9}"#
        );
        m.from = Sender::User;
        m.re = None;
        assert_eq!(
            m.envelope(),
            r#"{"keepane":1,"id":12,"task":3,"from":"user","to":"$1:@4.%9","via":"shell","hop":1}"#
        );
        // The same message, the same bytes, every time.
        assert_eq!(m.envelope(), m.clone().envelope());
    }

    /// A pane of another machine (docs/design/link.md) is `from=host:port/…`
    /// with its name and mode as it said them, in both forms, the same
    /// bytes every time; the event log reads it back as the same sender,
    /// and it is never taken for a pane of this machine.
    #[test]
    fn a_sender_on_another_machine_reads_as_its_address_there() {
        let mut m = msg(12, WorkMode::Ai);
        m.from = Sender::Remote {
            addr: "100.64.0.3:7681".into(),
            address: "$2:@5.%8".into(),
            name: Some("lead".into()),
            mode: Some(WorkMode::Ai),
        };
        assert_eq!(
            m.envelope(),
            r#"{"keepane":1,"id":12,"task":12,"from":"100.64.0.3:7681/$2:@5.%8","name":"lead","mode":"ai","to":"$1:@4.%9","via":"ai","hop":0}"#
        );
        assert_eq!(
            m.fields(),
            "[keepane id=12 task=12 from=100.64.0.3:7681/$2:@5.%8 name=lead mode=ai to=$1:@4.%9 via=ai hop=0]"
        );
        assert_eq!(m.sender_pane(), None, "not a pane of this machine");
        assert_eq!((m.envelope(), m.fields()), (m.clone().envelope(), m.clone().fields()));
        // Someone outside the panes there: no name, no mode.
        m.from =
            Sender::Remote { addr: "[fd7a:115c:a1e0::3]:7681".into(), address: "user".into(), name: None, mode: None };
        assert_eq!(m.fields(), "[keepane id=12 task=12 from=[fd7a:115c:a1e0::3]:7681/user to=$1:@4.%9 via=ai hop=0]");
    }

    /// The fields read as the JSON says, in its order, and split back
    /// into the same names and values.
    #[test]
    fn the_fields_say_what_the_envelope_says() {
        let mut m = msg(12, WorkMode::Shell);
        m.hop = 1;
        m.re = Some(9);
        m.task = 3;
        let f = m.fields();
        assert_eq!(f, "[keepane id=12 task=3 from=$1:@3.%7 name=builder mode=ai to=$1:@4.%9 via=shell hop=1 re=9]");
        // Split back: the JSON's own names and values.
        let pairs: Vec<(String, String)> = f
            .trim_start_matches("[keepane ")
            .trim_end_matches(']')
            .split(' ')
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect();
        let json: serde_json::Value = serde_json::from_str(&m.envelope()).unwrap();
        let from_json: Vec<(String, String)> = json
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k != "keepane")
            .map(|(k, v)| (k.clone(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))
            .collect();
        assert_eq!(pairs, from_json);
        m.from = Sender::User;
        m.re = None;
        assert_eq!(m.fields(), "[keepane id=12 task=3 from=user to=$1:@4.%9 via=shell hop=1]");
        assert_eq!(m.header(EnvelopeStyle::Json), m.envelope());
        assert_eq!(m.end_line(EnvelopeStyle::Fields), "[keepane end=12]");
        assert_eq!(m.end_line(EnvelopeStyle::Json), r#"{"keepane":1,"end":12}"#);
        assert_eq!(EnvelopeStyle::parse("fields"), Some(EnvelopeStyle::Fields));
        assert_eq!(EnvelopeStyle::parse("xml"), None);
    }

    #[test]
    fn each_mode_wraps_the_same_envelope_its_own_way() {
        let (f, j) = (EnvelopeStyle::Fields, EnvelopeStyle::Json);
        let m = msg(5, WorkMode::Shell);
        assert_eq!(m.wrapped(Syntax::PowerShell, f), "<# keepane #5 from %builder #> text 5");
        assert_eq!(m.wrapped(Syntax::Posix, f), ": 'keepane #5 from %builder'; text 5");
        assert_eq!(m.wrapped(Syntax::PowerShell, j), format!("<# {} #> text 5", m.envelope()));
        assert_eq!(m.wrapped(Syntax::Posix, j), format!(": '{}'; text 5", m.envelope()));
        let m = msg(5, WorkMode::Ai);
        let agent = format!("{}\ntext 5\n[keepane end=5]", m.fields());
        assert_eq!(m.wrapped(Syntax::PowerShell, f), agent);
        assert_eq!(m.wrapped(Syntax::Posix, f), agent, "an agent's text is not the shell's");
        let agent = format!("{}\ntext 5\n{{\"keepane\":1,\"end\":5}}", m.envelope());
        assert_eq!(m.wrapped(Syntax::Posix, j), agent);
    }

    /// A shell's header names the sender as a target does, for each kind
    /// of sender, and `shell-history` reads the id and the sender back.
    #[test]
    fn a_shells_header_is_short_and_reads_back() {
        let mut m = msg(12, WorkMode::Shell);
        let senders = [
            (m.from.clone(), "%builder"),
            (Sender::Pane { id: 7, address: "$1:@3.%7".into(), name: None, mode: WorkMode::Shell }, "%7"),
            (Sender::User, "user"),
            (
                Sender::Remote {
                    addr: "100.64.0.3:7681".into(),
                    address: "$2:@5.%8".into(),
                    name: Some("lead".into()),
                    mode: None,
                },
                "100.64.0.3:7681/%lead",
            ),
            (
                Sender::Remote {
                    addr: "[fd7a:115c:a1e0::3]:7681".into(),
                    address: "user".into(),
                    name: None,
                    mode: None,
                },
                "[fd7a:115c:a1e0::3]:7681/user",
            ),
        ];
        for (from, tag) in senders {
            m.from = from;
            assert_eq!(m.shell_header(EnvelopeStyle::Fields), format!("keepane #12 from {tag}"));
            for syntax in [Syntax::PowerShell, Syntax::Posix] {
                let w = m.wrapped(syntax, EnvelopeStyle::Fields);
                let d = crate::server::shellhist::delivered(&w).unwrap_or_else(|| panic!("{w}"));
                assert_eq!((d.id, d.from.as_str(), d.command.as_str()), (12, tag, "text 12"), "{w}");
            }
        }
        // An agent still gets the whole header.
        let m = msg(12, WorkMode::Ai);
        assert!(m.wrapped(Syntax::Posix, EnvelopeStyle::Fields).starts_with(&m.fields()));
    }

    #[test]
    fn a_posix_command_of_several_lines_goes_in_as_one() {
        assert_eq!(posix_one_command("ls -l\n"), "ls -l");
        assert_eq!(posix_one_command("a='x'\r\necho \"$a\""), r#"eval "$(printf '%s\n' 'a='\''x'\''' 'echo "$a"')""#);
        let mut m = msg(3, WorkMode::Shell);
        m.text = "a\nb".into();
        let w = m.wrapped(Syntax::Posix, EnvelopeStyle::Fields);
        assert!(w.ends_with(r#"; eval "$(printf '%s\n' 'a' 'b')""#), "{w}");
        assert!(!w.contains('\n'), "one line");
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn a_shell_command_of_several_lines_goes_in_as_one() {
        assert_eq!(one_command("Get-Date"), "Get-Date");
        assert_eq!(one_command("Get-Date\r\n"), "Get-Date", "a trailing line end is not a second line");
        let two = one_command("$a = 'x'\r\nWrite-Output $a ‘q’");
        assert!(
            two.starts_with("$__keepane_e = $Error[0]; . ([scriptblock]::Create(('$a = ''x''', 'Write-Output $a ‘‘q’’') -join \"`n\")); "),
            "{two}"
        );
        assert!(two.ends_with("Write-Error \"a line above failed\" -ErrorAction SilentlyContinue }"), "{two}");
        let mut m = msg(3, WorkMode::Shell);
        m.text = "a\nb".into();
        assert!(
            m.wrapped(Syntax::PowerShell, EnvelopeStyle::Fields)
                .contains("#> $__keepane_e = $Error[0]; . ([scriptblock]::Create(('a', 'b')"),
            "{}",
            m.wrapped(Syntax::PowerShell, EnvelopeStyle::Fields)
        );
        assert!(!m.wrapped(Syntax::PowerShell, EnvelopeStyle::Fields).contains('\n'), "one line");
    }

    /// Text typed while a command runs is on the next prompt's line: the
    /// pane stays busy however often that prompt is drawn (bash draws it
    /// again as the typed-ahead text comes in), until someone types again
    /// and a prompt follows.
    #[test]
    fn text_typed_during_a_command_keeps_the_pane_busy_through_redrawn_prompts() {
        let mut a = actor(WorkMode::Shell);
        a.enqueue(msg(1, WorkMode::Shell), 10).unwrap();
        a.enqueue(msg(2, WorkMode::Shell), 10).unwrap();
        a.prompt();
        assert_eq!(a.next_delivery().map(|m| m.id), Some(1));
        a.input(); // typed ahead while #1 runs
        assert_eq!(a.prompt().map(|(m, _)| m.id), Some(1), "#1 is over");
        assert!(!a.idle());
        for _ in 0..3 {
            a.prompt(); // the same prompt, drawn again
            assert!(!a.idle(), "still the typed text on the line");
            assert_eq!(a.next_delivery(), None);
        }
        // Someone clears the line and presses Enter: the next prompt is free.
        a.input();
        a.prompt();
        assert!(a.idle());
        assert_eq!(a.next_delivery().map(|m| m.id), Some(2));
        // force_idle and a restart forget it too.
        a.input();
        a.prompt();
        assert!(!a.idle());
        a.force_idle();
        assert!(a.idle());
    }

    #[test]
    fn a_restarted_program_keeps_the_inbox_and_ends_the_work() {
        let mut a = actor(WorkMode::Shell);
        a.name = Some("builder".into());
        a.enqueue(msg(1, WorkMode::Shell), 10).unwrap();
        a.enqueue(msg(2, WorkMode::Shell), 10).unwrap();
        a.prompt();
        assert_eq!(a.next_delivery().map(|m| m.id), Some(1));
        // The program is started again while working on #1.
        assert_eq!(a.restarted().map(|m| m.id), Some(1), "the work in hand ends with it");
        assert!(!a.idle(), "not at a prompt until the new program shows one");
        assert_eq!((a.name.as_deref(), a.mode, a.inbox.len()), (Some("builder"), WorkMode::Shell, 1));
        assert_eq!(a.prompt(), None, "nothing was in hand any more");
        assert_eq!(a.next_delivery().map(|m| m.id), Some(2), "the queue goes on");
        assert_eq!(a.restarted().map(|m| m.id), Some(2));
        assert_eq!(a.restarted(), None);
        // Free at its prompt, then started again: busy until the new one's.
        a.prompt();
        assert!(a.idle());
        a.restarted();
        assert!(!a.idle(), "the old program's prompt is not the new one's");
        let mut b = actor(WorkMode::Ai);
        b.ready();
        assert!(b.idle());
        b.restarted();
        assert!(!b.idle(), "nor its word that it was ready");
    }

    #[test]
    fn a_shell_pane_is_free_at_keepanes_prompt_only() {
        let mut a = actor(WorkMode::Shell);
        assert!(!a.idle(), "busy until the first prompt");
        a.enqueue(msg(1, WorkMode::Shell), 10).unwrap();
        assert_eq!(a.next_delivery(), None);
        // The agent's word does not count in a shell pane.
        assert_eq!(a.ready(), (false, None));
        assert!(!a.idle());
        assert_eq!(a.prompt(), None);
        assert!(a.idle());
        // Typing at the prompt makes it busy again.
        a.input();
        assert_eq!(a.next_delivery(), None);
        a.prompt();
        assert_eq!(a.next_delivery().map(|m| m.id), Some(1));
        assert!(!a.idle(), "busy with what it was given");
        // Its prompt coming back ends that command.
        assert_eq!(a.prompt().map(|(m, e)| (m.id, e)), Some((1, End::Done)));
        assert_eq!(a.current, None);
    }

    #[test]
    fn typing_right_after_the_prompt_mark_keeps_the_pane_busy() {
        let mut a = actor(WorkMode::Shell);
        a.prompt_seen();
        a.input();
        assert_eq!(a.command_over(), None);
        assert!(!a.idle(), "typed after the marker: busy, whatever comes after");
    }

    #[test]
    fn typing_while_its_command_runs_keeps_a_shell_pane_busy_after_it() {
        let mut a = actor(WorkMode::Shell);
        a.enqueue(msg(1, WorkMode::Shell), 10).unwrap();
        a.enqueue(msg(2, WorkMode::Shell), 10).unwrap();
        a.prompt();
        a.next_delivery().unwrap();
        // Typed while #1 runs: the prompt that ends it shows that text.
        a.input();
        assert_eq!(a.prompt().map(|(m, _)| m.id), Some(1));
        assert!(!a.idle(), "#2 is not typed after what is on the line");
        // The person's own Enter brings a clean prompt: free again.
        a.input();
        a.prompt();
        assert_eq!(a.next_delivery().map(|m| m.id), Some(2));
        // Nothing typed while #2 runs: free at its end.
        a.prompt();
        assert!(a.idle());
    }

    #[test]
    fn an_agents_pane_is_free_when_the_agent_says_so() {
        let mut a = actor(WorkMode::Ai);
        a.enqueue(msg(1, WorkMode::Ai), 10).unwrap();
        a.prompt();
        assert!(!a.idle(), "a shell prompt is not the agent being ready");
        assert_eq!(a.ready(), (true, None));
        assert_eq!(a.next_delivery().map(|m| m.id), Some(1));
        assert!(!a.idle());
        let (taken, done) = a.ready();
        assert!(taken);
        assert_eq!(done.map(|(m, e)| (m.id, e)), Some((1, End::Done)));
        // Ready, then the agent exits: its pane shows the shell's prompt,
        // and nothing is typed into that shell.
        a.enqueue(msg(2, WorkMode::Ai), 10).unwrap();
        a.prompt();
        assert_eq!(a.next_delivery(), None);
    }

    #[test]
    fn an_agent_that_exits_mid_task_abandons_it() {
        let mut a = actor(WorkMode::Ai);
        a.enqueue(msg(1, WorkMode::Ai), 10).unwrap();
        a.ready();
        a.next_delivery().unwrap();
        assert_eq!(a.prompt().map(|(m, e)| (m.id, e)), Some((1, End::Abandoned)));
    }

    #[test]
    fn a_normal_pane_takes_nothing_on_its_own() {
        let mut a = actor(WorkMode::Normal);
        a.enqueue(msg(1, WorkMode::Normal), 10).unwrap();
        a.enqueue(msg(2, WorkMode::Normal), 10).unwrap();
        a.prompt();
        assert_eq!(a.ready(), (false, None));
        a.force_idle();
        assert_eq!(a.next_delivery(), None);
        assert_eq!(a.read().map(|m| m.id), Some(1));
        // Taken is done with: it is not worked on, only answerable.
        assert_eq!(a.current, None);
        assert_eq!(a.answering().map(|m| m.id), Some(1));
        assert_eq!(a.carry(false), None, "something new starts a new chain");
        assert_eq!(a.carry(true), Some((1, 1)), "an answer carries its chain on");
        assert_eq!(a.read().map(|m| m.id), Some(2));
        assert_eq!(a.answering().map(|m| m.id), Some(2));
        assert_eq!(a.read(), None);
    }

    #[test]
    fn an_agent_taking_an_answer_keeps_working_on_its_own_message() {
        let mut a = actor(WorkMode::Ai);
        a.enqueue(msg(1, WorkMode::Ai), 10).unwrap();
        a.ready();
        a.next_delivery().unwrap();
        // An answer arrives while it works, and it takes it (wait_message).
        let mut answer = msg(9, WorkMode::Ai);
        answer.task = 1;
        a.enqueue(answer, 10).unwrap();
        assert_eq!(a.read().map(|m| m.id), Some(9));
        assert_eq!(a.current.as_ref().map(|m| m.id), Some(1), "still on its own message");
        assert_eq!(a.answering().map(|m| m.id), Some(1), "-r answers who gave it the work");
    }

    #[test]
    fn a_message_is_delivered_only_in_the_mode_it_was_sent_for() {
        let mut a = actor(WorkMode::Ai);
        a.enqueue(msg(1, WorkMode::Ai), 10).unwrap();
        a.enqueue(msg(2, WorkMode::Shell), 10).unwrap();
        a.mode = WorkMode::Shell;
        a.prompt();
        // Text written for the agent is not run as a command.
        assert_eq!(a.next_delivery().map(|m| m.id), Some(2));
        a.prompt();
        assert_eq!(a.next_delivery(), None);
        assert_eq!(a.inbox.len(), 1);
    }

    #[test]
    fn switching_mode_keeps_what_was_seen() {
        // A shell sitting at its prompt takes a message as soon as its pane
        // is switched to shell mode, without waiting for another prompt.
        let mut a = actor(WorkMode::Normal);
        a.prompt();
        a.mode = WorkMode::Shell;
        assert!(a.idle());
    }

    #[test]
    fn force_idle_unsticks_a_busy_pane() {
        let mut a = actor(WorkMode::Ai);
        a.input();
        assert!(!a.idle());
        a.force_idle();
        assert!(a.idle());
    }

    /// Heard from: once its agent says it is ready, until the agent is gone
    /// (a shell prompt), started again, or the pane leaves and re-enters
    /// `ai`. A person unsticking it proves no hook, and typing ends nothing.
    #[test]
    fn an_agent_is_heard_from_once_it_says_it_is_ready() {
        let mut a = actor(WorkMode::Ai);
        assert!(a.unheard());
        a.force_idle();
        assert!(a.unheard(), "a person's word is not the agent's");
        a.ready();
        assert!(!a.unheard());
        a.input();
        assert!(!a.unheard(), "typing is not the agent leaving");
        a.prompt_seen();
        assert!(a.unheard(), "a shell prompt: the agent is gone");
        a.ready();
        a.restarted();
        assert!(a.unheard(), "started again");
        a.ready();
        a.set_mode(WorkMode::Ai);
        assert!(!a.unheard(), "the same mode again changes nothing");
        a.set_mode(WorkMode::Normal);
        assert!(!a.unheard(), "only an ai pane is waited on");
        a.set_mode(WorkMode::Ai);
        assert!(a.unheard());
        // A normal pane's pane-ready is not taken, so not heard either.
        let mut n = actor(WorkMode::Normal);
        n.ready();
        n.set_mode(WorkMode::Ai);
        assert!(n.unheard());
    }

    #[test]
    fn a_full_inbox_refuses() {
        let mut a = actor(WorkMode::Normal);
        assert_eq!(a.enqueue(msg(1, WorkMode::Normal), 2), Ok(0));
        assert_eq!(a.enqueue(msg(2, WorkMode::Normal), 2), Ok(1));
        assert!(a.enqueue(msg(3, WorkMode::Normal), 2).is_err());
    }

    #[test]
    fn what_a_pane_sends_while_working_carries_the_chain_on() {
        let mut a = actor(WorkMode::Ai);
        assert_eq!(a.carry(false), None);
        let mut m = msg(4, WorkMode::Ai);
        m.task = 2;
        m.hop = 3;
        a.enqueue(m, 10).unwrap();
        a.ready();
        a.next_delivery();
        assert_eq!(a.carry(false), Some((2, 4)));
        a.ready();
        assert_eq!(a.carry(false), None, "done with it: a new chain starts");
    }

    #[test]
    fn queued_messages_move_and_come_back() {
        let mut a = actor(WorkMode::Normal);
        for i in 1..=4 {
            a.enqueue(msg(i, WorkMode::Normal), 10).unwrap();
        }
        let ids = |a: &Actor| a.inbox.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(a.move_message(3, Move::Top), Some((2, 0)));
        assert_eq!(ids(&a), [3, 1, 2, 4]);
        assert_eq!(a.move_message(3, Move::Up), Some((0, 0)));
        assert_eq!(a.move_message(4, Move::Down), Some((3, 3)));
        assert_eq!(a.move_message(1, Move::Down), Some((1, 2)));
        assert_eq!(ids(&a), [3, 2, 1, 4]);
        assert_eq!(a.move_message(9, Move::Top), None);
        let m = a.remove(2).unwrap();
        assert_eq!(ids(&a), [3, 1, 4]);
        a.restore(m, 1);
        assert_eq!(ids(&a), [3, 2, 1, 4]);
        let m = a.remove(4).unwrap();
        a.inbox.clear();
        a.restore(m, 3);
        assert_eq!(ids(&a), [4]);
    }

    #[test]
    fn names_are_what_a_target_can_tell_from_an_id() {
        assert!(check_name("builder").is_ok());
        assert!(check_name("test-2_b").is_ok());
        assert!(check_name("12").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("a b").is_err());
        assert!(check_name("x#>y").is_err());
        assert!(check_name(&"a".repeat(65)).is_err());
    }
}
