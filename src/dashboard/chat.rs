//! The board's page 3: the group chat (docs/design/chat.md). `[1]` the
//! channels (members, what the person has not read, tags), `[2]` the
//! picked channel's members (handle, role, who and where each is), `[4]`
//! what was said there, the latest at the bottom. Read from the server
//! with `chat-list -J`, `chat-who -J` and `chat-read --all -J`.
//!
//! What each one says is a bubble in its colour: the members' colours by
//! the order they joined the channel (`PALETTE`), so the first six in a
//! channel never share one, the same in `[2]`; the person's bubbles in the
//! terminal's own ink, against the right edge. One member's lines within
//! `GROUP` of each other share a bubble. A rule marks each new day and
//! where what the person had not read begins.

use super::{
    Action, Board, Input, Panel, Rect, boxed, clip, files::Icons, fit, inert, owned, paint, row_line, width_of,
    window_start,
};
use crate::keys::KeyCode;

/// The lines of the picked channel read for `[4]`.
pub const SHOWN: usize = 300;

/// The members' colours, in the order they joined: six of the 16 the
/// theme maps, none bright (bright yellow and green fade on a light page).
const PALETTE: [&str; 6] = ["36", "35", "34", "32", "33", "31"];

/// The person's colour: the terminal's own ink, bold.
const PERSON: &str = "1";

/// Lines of one member this close in time share a bubble.
const GROUP: i64 = 5 * 60;

/// Below this many columns `[4]` draws no bubbles: a header line and the
/// text under it, as before.
const BUBBLES_FROM: usize = 16;

/// Whether a handle is the person's (`you@machine`: a member's handle
/// cannot hold an `@`).
fn is_person(handle: &str) -> bool {
    handle.starts_with("you@")
}

/// The handles named in a text (`@` and a word), as the server reads them.
fn mentions(text: &str) -> Vec<&str> {
    text.split('@')
        .skip(1)
        .map(|p| {
            let end = p.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')).unwrap_or(p.len());
            &p[..end]
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// A channel, as `chat-list -J` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChanRow {
    pub id: String,
    pub name: String,
    pub topic: String,
    pub tags: Vec<String>,
    pub members: usize,
    pub unread: u64,
    /// The last thing said: when (as the server writes it), who, and the
    /// start of it, on one line; empty when nothing was.
    pub latest_at: String,
    pub latest_by: String,
    pub latest_text: String,
    /// Another machine's channel: that machine's name (its id is then
    /// `MACHINE/ID`, which every chat command takes); empty for one here.
    pub machine: String,
}

impl ChanRow {
    /// Whether a filter's text is in its name or one of its tags.
    fn matches(&self, filter: &str) -> bool {
        let f = filter.trim().trim_start_matches('#').to_lowercase();
        f.is_empty() || self.name.to_lowercase().contains(&f) || self.tags.iter().any(|t| t.to_lowercase().contains(&f))
    }
}

/// A member, as `chat-who -J` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemberRow {
    pub handle: String,
    pub role: String,
    pub pane: u32,
    pub place: String,
    pub machine: String,
    pub agent: String,
    pub model: String,
    /// Its pane busy (an agent working), idle, or not known.
    pub busy: Option<bool>,
    /// A pane is it now; offline: none is (its pane went, or the server
    /// started again and no pane of its name is back yet).
    pub online: bool,
    /// It left the channel (kept, marked so; not counted).
    pub left: bool,
}

/// The characters of a line a reply's bubble quotes, at most.
const QUOTE_MOST: usize = 40;

/// A line of the channel, as `chat-read -J` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatLine {
    pub seq: u64,
    /// `14:02`.
    pub at: String,
    /// `2026-11-17`, for the rule between days.
    pub day: String,
    /// When, to put one member's lines close in time in one bubble.
    pub when: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// `say`, or what happened (`join`, `leave`, `topic`, ...).
    pub kind: String,
    pub handle: String,
    pub role: String,
    pub text: String,
}

#[derive(Default)]
pub struct ChatPage {
    pub channels: Vec<ChanRow>,
    pub pick: usize,
    pub members: Vec<MemberRow>,
    pub member_pick: usize,
    pub lines: Vec<ChatLine>,
    /// `[1]`'s filter: in a name or a tag (`/` on page 3).
    pub filter: String,
    /// Where what the person had not read began, in the channel picked
    /// (its id, the line's number): kept while that channel stays picked
    /// and page 3 shown, though showing it reads it at once.
    pub divider: Option<(String, u64)>,
    /// The member picked in the list a `@` being typed opens.
    pub at_pick: usize,
    /// The channel `board.json` names (`kept_path`): the one picked when
    /// the board was last open; after that, the one last written there.
    pub kept: Option<String>,
    /// That channel, to be picked as soon as a list has it (another
    /// machine's comes a moment later than this one's), unless the person
    /// picks another first (`pick_at`). Nothing is written meanwhile.
    pub wanted: Option<String>,
}

/// Where the board keeps which channel it had picked, so the next one
/// opened picks it again: beside the chat's files, one per server.
pub(super) fn kept_path(socket: &str) -> std::path::PathBuf {
    crate::histlog::chat_dir().join(crate::histlog::safe_name(socket)).join("board.json")
}

/// The channel `board.json` names; none when it is missing or not read.
pub(super) fn load_kept(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["channel"].as_str().filter(|c| !c.is_empty()).map(String::from)
}

/// `channel` written to `board.json`, whole or not at all: through a file
/// of this process's own, as two boards may be open at once.
pub(super) fn save_kept(path: &std::path::Path, channel: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.new", std::process::id()));
    std::fs::write(&tmp, serde_json::json!({ "channel": channel }).to_string())?;
    std::fs::rename(&tmp, path)
}

/// The handle being typed: what follows the `@` of the text's last word,
/// while that word is a `@` and a handle's characters.
pub(super) fn naming(text: &str) -> Option<&str> {
    let last = text.rsplit(char::is_whitespace).next()?;
    last.strip_prefix('@').filter(|w| w.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'))
}

/// The most members the `@` list shows.
const NAMING_MOST: usize = 5;

/// A string of the server's answer, as the board may draw it (`inert`):
/// what members say is theirs to write.
fn s(v: &serde_json::Value, k: &str) -> String {
    inert(v[k].as_str().unwrap_or_default())
}

impl ChatPage {
    /// The channels the filter keeps.
    pub(super) fn shown(&self) -> Vec<&ChanRow> {
        self.channels.iter().filter(|c| c.matches(&self.filter)).collect()
    }

    pub(super) fn picked(&self) -> Option<&ChanRow> {
        let shown = self.shown();
        shown.get(self.pick.min(shown.len().saturating_sub(1))).copied()
    }

    /// The channel picked, when `board.json` does not name it yet; none
    /// while the one it names may still come.
    pub(super) fn to_keep(&self) -> Option<String> {
        if self.wanted.is_some() {
            return None;
        }
        self.picked().map(|c| c.id.clone()).filter(|id| self.kept.as_ref() != Some(id))
    }

    /// The person picks the `i`th channel shown: the one the board had
    /// picked last time is no longer waited for.
    pub(super) fn pick_at(&mut self, i: usize) {
        self.pick = i;
        self.wanted = None;
    }

    /// `chat-list -J`; the channel picked stays picked (by its id). Until
    /// a list has it, the one the board had picked when it was last open
    /// is picked as soon as one does (`wanted`).
    pub(super) fn set_channels(&mut self, json: &str) {
        let was = self.picked().map(|c| c.id.clone());
        let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
        self.channels = v
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| ChanRow {
                        id: s(c, "id"),
                        name: s(c, "name"),
                        topic: s(c, "topic"),
                        tags: c["tags"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|t| t.as_str().map(inert))
                            .collect(),
                        members: c["members"].as_u64().unwrap_or(0) as usize,
                        unread: c["unread"].as_u64().unwrap_or(0),
                        latest_at: s(&c["latest"], "at"),
                        latest_by: s(&c["latest"], "handle"),
                        latest_text: s(&c["latest"], "text").replace('\n', " "),
                        machine: s(c, "machine"),
                    })
                    .filter(|c| !c.id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let (wanted, at, n) = {
            let shown = self.shown();
            let find = |id: &String| shown.iter().position(|c| &c.id == id);
            (self.wanted.as_ref().and_then(find), was.as_ref().and_then(find), shown.len())
        };
        if wanted.is_some() {
            self.wanted = None;
        }
        self.pick = wanted.or(at).unwrap_or(self.pick).min(n.saturating_sub(1));
    }

    /// `chat-who -J`.
    pub(super) fn set_members(&mut self, json: &str) {
        let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
        self.members = v
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| {
                        let i = &m["ident"];
                        MemberRow {
                            handle: s(m, "handle"),
                            role: s(m, "role"),
                            pane: i["pane"].as_u64().unwrap_or(0) as u32,
                            place: s(i, "place"),
                            machine: s(i, "machine"),
                            agent: s(i, "agent"),
                            model: s(i, "model"),
                            busy: m["busy"].as_bool(),
                            // An older server says nothing: its members are online.
                            online: m["online"].as_bool().unwrap_or(true),
                            left: m["left"].as_bool().unwrap_or(false),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.member_pick = self.member_pick.min(self.members.len().saturating_sub(1));
    }

    /// `chat-read --all -J`.
    pub(super) fn set_lines(&mut self, json: &str) {
        let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
        self.lines = v["lines"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|l| {
                        // Who and where it was is `[2]`'s to show, not each line's.
                        let from = &l["from"];
                        let at = s(l, "at");
                        ChatLine {
                            seq: l["seq"].as_u64().unwrap_or(0),
                            at: at.get(11..16).unwrap_or(&at).to_string(),
                            day: at.get(..10).unwrap_or_default().to_string(),
                            when: chrono::DateTime::parse_from_rfc3339(&at).ok(),
                            kind: s(l, "kind"),
                            handle: s(from, "handle"),
                            role: s(from, "role"),
                            text: said_with_files(l),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A reply: the start of the line it answers above it (`↩ #42 lead:
        // the plan…`), or the number alone when that line is not loaded.
        let replies: Vec<(usize, u64)> = v["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, l)| l["re"].as_u64().map(|r| (i, r)))
            .collect();
        for (i, r) in replies {
            let quote = match self.lines.iter().find(|l| l.seq == r) {
                Some(q) => {
                    let first = q.text.lines().next().unwrap_or_default();
                    let mut start: String = first.chars().take(QUOTE_MOST).collect();
                    if first.chars().count() > QUOTE_MOST {
                        start.push('…');
                    }
                    format!("↩ #{r} {}: {start}", q.handle)
                }
                None => format!("↩ #{r}"),
            };
            if let Some(l) = self.lines.get_mut(i) {
                l.text = if l.text.is_empty() { quote } else { format!("{quote}\n{}", l.text) };
            }
        }
        self.mark_unread();
    }

    /// The divider: dropped when another channel is picked; set where the
    /// unread lines begin when the picked one has some and none is set (a
    /// line said while it is shown leaves it where it was).
    fn mark_unread(&mut self) {
        let Some(c) = self.picked() else {
            self.divider = None;
            return;
        };
        let (id, unread) = (c.id.clone(), c.unread as usize);
        if self.divider.as_ref().is_some_and(|(d, _)| *d != id) {
            self.divider = None;
        }
        if self.divider.is_none() && unread > 0 {
            // The server counts the lines said after the person's place,
            // not the person's own.
            let said: Vec<u64> =
                self.lines.iter().filter(|l| l.kind == "say" && !is_person(&l.handle)).map(|l| l.seq).collect();
            if let Some(&seq) = said.get(said.len().saturating_sub(unread)) {
                self.divider = Some((id, seq));
            }
        }
    }

    /// The members a `@` being typed may name: those whose handle starts
    /// with what follows it (any case), online first, `NAMING_MOST` at most.
    pub(super) fn naming_list(&self, text: &str) -> Vec<&MemberRow> {
        let Some(typed) = naming(text).map(str::to_lowercase) else { return Vec::new() };
        let mut list: Vec<&MemberRow> =
            self.members.iter().filter(|m| !m.left && m.handle.to_lowercase().starts_with(&typed)).collect();
        list.sort_by_key(|m| !m.online);
        list.truncate(NAMING_MOST);
        list
    }

    /// `text` with its last word, a `@` being typed, made the handle of the
    /// member picked in the list, and a space after it.
    pub(super) fn complete_naming(&self, text: &str) -> Option<String> {
        let list = self.naming_list(text);
        let m = list.get(self.at_pick.min(list.len().saturating_sub(1)))?;
        let typed = naming(text)?;
        Some(format!("{}{} ", &text[..text.len() - typed.len()], m.handle))
    }

    /// A handle's colour: the person's ink, a member's by when it joined,
    /// one who left by its name (the same each time).
    pub(super) fn colour(&self, handle: &str) -> &'static str {
        if is_person(handle) {
            return PERSON;
        }
        let at = self
            .members
            .iter()
            .position(|m| m.handle == handle)
            .unwrap_or_else(|| handle.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(usize::from(b))));
        PALETTE[at % PALETTE.len()]
    }

    /// A line of what was said with each `@handle` of a member (or `@you`,
    /// the person) in that one's colour, bold.
    fn tint_mentions(&self, line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while let Some(i) = rest.find('@') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let end = after.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')).unwrap_or(after.len());
            let word = &after[..end];
            let known = word == "you" || self.members.iter().any(|m| m.handle == word);
            if known && !word.is_empty() {
                let c = if word == "you" { PERSON } else { self.colour(word) };
                out.push_str(&paint(&format!("1;{c}"), &format!("@{word}")));
            } else {
                out.push('@');
                out.push_str(word);
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    /// `[4]`'s lines in `w` columns each, the oldest first: the rules, what
    /// happened (a line, faint) and the bubbles.
    pub(super) fn drawn(&self, w: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let event = |l: &ChatLine| {
            let by = if l.handle.is_empty() { String::new() } else { format!("{} ", l.handle) };
            fit(&paint("2", &format!("{} · {by}{}", l.at, l.text)), w)
        };
        if w < BUBBLES_FROM {
            for l in &self.lines {
                if l.kind != "say" {
                    out.push(event(l));
                    continue;
                }
                let who = paint(&format!("1;{}", self.colour(&l.handle)), &l.handle);
                out.push(fit(&format!("{} {who}", paint("2", &l.at)), w));
                out.extend(wrap(&l.text, w.saturating_sub(2)).iter().map(|t| fit(&format!("  {t}"), w)));
            }
            return out;
        }
        let divider =
            self.divider.as_ref().filter(|(id, _)| self.picked().is_some_and(|c| c.id == *id)).map(|(_, seq)| *seq);
        let new = divider.map_or(0, |d| {
            self.lines.iter().filter(|l| l.kind == "say" && l.seq >= d && !is_person(&l.handle)).count()
        });
        let mut day = String::new();
        let mut open: Option<Bubble> = None;
        for l in &self.lines {
            let new_day = !l.day.is_empty() && l.day != day;
            let unread_here = divider == Some(l.seq);
            if (new_day || unread_here || l.kind != "say")
                && let Some(b) = open.take()
            {
                out.extend(self.bubble(&b, w));
            }
            if new_day {
                day = l.day.clone();
                out.push(rule(&day_label(&day), w, "90"));
            }
            if unread_here {
                out.push(rule(&format!("{new} new"), w, "33"));
            }
            if l.kind != "say" {
                out.push(event(l));
                continue;
            }
            let body = wrap(&l.text, w - 4);
            let called = !is_person(&l.handle) && mentions(&l.text).contains(&"you");
            match &mut open {
                Some(b) if b.handle == l.handle && close_in_time(b.last, l.when) => {
                    b.body.extend(body);
                    b.last = l.when;
                    b.called |= called;
                }
                _ => {
                    if let Some(b) = open.take() {
                        out.extend(self.bubble(&b, w));
                    }
                    open = Some(Bubble {
                        handle: l.handle.clone(),
                        role: l.role.clone(),
                        at: l.at.clone(),
                        last: l.when,
                        body,
                        called,
                    });
                }
            }
        }
        if let Some(b) = open.take() {
            out.extend(self.bubble(&b, w));
        }
        out
    }

    /// A bubble in `w` columns: its corners and sides in its one's colour,
    /// the name, role and time in its top edge; as wide as its text (or its
    /// top edge's words), the person's against the right edge. A heavy left
    /// side: it names the person (`@you`).
    fn bubble(&self, b: &Bubble, w: usize) -> Vec<String> {
        let c = self.colour(&b.handle);
        let role = if b.role.is_empty() { String::new() } else { format!(" {}", paint(c, &b.role)) };
        let head = format!(" {}{role}{} ", paint(&format!("1;{c}"), &b.handle), paint("2", &format!(" · {}", b.at)));
        let body_w = b.body.iter().map(|l| width_of(l)).max().unwrap_or(0);
        // Inside the corners: the text and a space each side, or the top
        // edge's words after one rule and before another.
        let inner = (body_w + 2).max(width_of(&head) + 2).min(w - 2);
        let pad = if is_person(&b.handle) { " ".repeat(w - inner - 2) } else { String::new() };
        let (head, head_w) = clip(&head, inner - 1);
        let mut out =
            vec![format!("{pad}{}{head}{}", paint(c, "╭─"), paint(c, &format!("{}╮", "─".repeat(inner - 1 - head_w))))];
        let side = if b.called { "┃" } else { "│" };
        for l in &b.body {
            out.push(format!("{pad}{} {} {}", paint(c, side), fit(&self.tint_mentions(l), inner - 2), paint(c, "│")));
        }
        out.push(format!("{pad}{}", paint(c, &format!("╰{}╯", "─".repeat(inner)))));
        out.into_iter().map(|l| fit(&l, w)).collect()
    }
}

/// A bubble being filled: whose, when it began and when its last line was
/// said, its text in lines, and whether one of them names the person.
struct Bubble {
    handle: String,
    role: String,
    at: String,
    last: Option<chrono::DateTime<chrono::FixedOffset>>,
    body: Vec<String>,
    called: bool,
}

/// Whether a line said at `then` goes in the bubble whose last line was at
/// `last`: within `GROUP` after it (a time not known: never).
fn close_in_time(
    last: Option<chrono::DateTime<chrono::FixedOffset>>,
    then: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> bool {
    match (last, then) {
        (Some(a), Some(b)) => (0..=GROUP).contains(&(b - a).num_seconds()),
        _ => false,
    }
}

/// Rows a channel takes in `[1]`: its name, and under it the last thing
/// said, while the panel has room for two channels at least.
pub(super) fn channel_rows(inner_h: usize) -> usize {
    if inner_h >= 4 { 2 } else { 1 }
}

/// `2026-11-19` as a rule between days reads it: `Thu 11-19`, the year too
/// when it is not this one.
fn day_label(day: &str) -> String {
    match chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
        Ok(d) if d.format("%Y").to_string() == chrono::Local::now().format("%Y").to_string() => {
            d.format("%a %m-%d").to_string()
        }
        Ok(d) => d.format("%a %Y-%m-%d").to_string(),
        Err(_) => day.to_string(),
    }
}

/// How long ago a time the server wrote was, short: `now`, `5m`, `3h`, `2d`.
fn ago(at: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(at) else { return String::new() };
    let s = (chrono::Local::now().fixed_offset() - t).num_seconds().max(0);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// `label` in a rule across `width` columns: `──── label ────`.
fn rule(label: &str, width: usize, sgr: &str) -> String {
    let text = format!(" {label} ");
    let tw = width_of(&text);
    if tw + 2 > width {
        return paint(sgr, &clip(&"─".repeat(width), width).0);
    }
    let left = (width - tw) / 2;
    paint(sgr, &format!("{}{text}{}", "─".repeat(left), "─".repeat(width - tw - left)))
}

/// The columns a paragraph's continuation lines go in by: a list item's
/// (`- `, `* `, `• `, `1. `, `2) `, `3、`) up to its text, its indent kept.
fn hang_of(para: &str) -> usize {
    let lead = para.len() - para.trim_start_matches(' ').len();
    let rest = &para[lead..];
    let marker = ["- ", "* ", "• "].iter().find(|m| rest.starts_with(**m)).map(|m| width_of(m)).or_else(|| {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let after = &rest[digits..];
        (digits > 0 && digits < 4)
            .then(|| [". ", ") ", "、"].iter().find(|m| after.starts_with(**m)).map(|m| digits + width_of(m)))
            .flatten()
    });
    marker.map_or(lead, |m| lead + m)
}

/// `text` cut into lines of `width` columns (at spaces when it can). A
/// paragraph's indent is kept, and its continuation lines go in as far as
/// its first line's text (`hang_of`: a list item reads as one).
/// A line's text, each file sent with it on a line of its own under it
/// (`📎 report.pdf  1.2M`).
fn said_with_files(l: &serde_json::Value) -> String {
    // Both through `s`: a name is the sender's as much as the text is.
    let files = l["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| format!("📎 {}  {}", s(f, "name"), super::files::size_text(f["size"].as_u64().unwrap_or(0))));
    let text = s(l, "text");
    std::iter::once(text).filter(|t| !t.is_empty()).chain(files).collect::<Vec<_>>().join("\n")
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        // No more than half the line, or the text would have no room.
        let hang = Some(hang_of(para)).filter(|h| *h <= width / 2).unwrap_or(0);
        let lead = (para.len() - para.trim_start_matches(' ').len()).min(hang);
        let indent = " ".repeat(hang);
        let mut lines: Vec<String> = Vec::new();
        // The line being filled, and whether a word is in it yet.
        let mut line = " ".repeat(lead);
        let mut has_word = false;
        let room = |lines: &Vec<String>| if lines.is_empty() { width } else { width - hang };
        let push = |lines: &mut Vec<String>, line: String| {
            lines.push(if lines.is_empty() { line } else { format!("{indent}{line}") });
        };
        for word in para.trim_start_matches(' ').split(' ') {
            let w = width_of(word);
            // A word that a line of its own holds goes there when this one
            // has no room for it; a longer one (Chinese has no spaces)
            // starts here, where a few columns are left.
            let gap = usize::from(has_word);
            let starts_here = w > width - hang && width_of(&line) + gap + 2 <= room(&lines);
            if has_word && width_of(&line) + 1 + w > room(&lines) && !starts_here {
                push(&mut lines, std::mem::take(&mut line));
                has_word = false;
            }
            let mut word = word.to_string();
            // A word longer than what is left goes in pieces, each a line,
            // as the board wraps any line: as wide as a continuation line,
            // which any piece may be.
            if width_of(&line) + usize::from(has_word) + w > room(&lines) {
                if has_word {
                    line.push(' ');
                    has_word = false;
                }
                // Its first piece fills the line it starts.
                let first = clip(&word, room(&lines) - width_of(&line)).0;
                let rest = word[first.len()..].to_string();
                line.push_str(&first);
                push(&mut lines, std::mem::take(&mut line));
                let mut pieces = super::wrap(&rest, width - hang);
                word = pieces.pop().unwrap_or_default();
                for p in pieces {
                    push(&mut lines, p);
                }
            }
            if has_word {
                line.push(' ');
            }
            line.push_str(&word);
            has_word = true;
        }
        push(&mut lines, line);
        out.extend(lines);
    }
    out
}

impl Board {
    /// What page 3 asks the server each refresh, the picked channel's
    /// members and lines with the list (`run` asks them in this order).
    pub(super) fn chat_queries(&self) -> Vec<Vec<String>> {
        let mut q = vec![owned(&["chat-list", "--machines", "-J"])];
        if let Some(c) = self.chat.picked() {
            let mut who = owned(&["chat-who", &c.id, "-J"]);
            let mut read = owned(&["chat-read", &c.id, "--all", "-n", &SHOWN.to_string(), "-J"]);
            // Another machine's: what was last heard of it, at once.
            if !c.machine.is_empty() {
                who.push("--cached".into());
                read.push("--cached".into());
            }
            q.push(who);
            q.push(read);
        }
        q
    }

    /// The picked channel's lines on the screen: what the person has not
    /// read of it is read now.
    pub(super) fn chat_seen(&self) -> Option<Vec<String>> {
        // On the screen only: a narrow board shows [4] while it has the focus.
        let c = self.chat.picked().filter(|c| {
            c.unread > 0 && c.machine.is_empty() && self.page == super::CHAT_PAGE && self.layout().main.is_some()
        })?;
        Some(owned(&["chat-read", &c.id, "-n", "1000"]))
    }

    /// Page 3's `[1]`: j/k pick, `s` say, `n` a new channel, `t` its topic,
    /// `#` its tags, Enter into `[4]`.
    pub(super) fn channel_key(&mut self, code: KeyCode) -> Action {
        let n = self.chat.shown().len();
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.chat.pick_at((self.chat.pick + 1).min(n.saturating_sub(1))),
            KeyCode::Up | KeyCode::Char('k') => self.chat.pick_at(self.chat.pick.saturating_sub(1)),
            KeyCode::Home | KeyCode::Char('g') => self.chat.pick_at(0),
            KeyCode::End | KeyCode::Char('G') => self.chat.pick_at(n.saturating_sub(1)),
            KeyCode::Enter => self.focus_on(Panel::Main),
            KeyCode::Char('n') => self.input = Some((Input::ChatNew, String::new())),
            KeyCode::Char(c @ ('s' | 't' | '#')) => {
                let Some(ch) = self.chat.picked().cloned() else {
                    self.note = Some("no channel (n makes one)".into());
                    return Action::Redraw;
                };
                self.input = Some(match c {
                    's' => (Input::ChatSay { id: ch.id, name: ch.name }, String::new()),
                    't' => (Input::ChatTopic { id: ch.id, name: ch.name }, ch.topic),
                    _ => (Input::ChatTag { id: ch.id, name: ch.name }, String::new()),
                });
            }
            _ => {}
        }
        self.chat.member_pick = self.chat.member_pick.min(self.chat.members.len().saturating_sub(1));
        Action::Redraw
    }

    /// Page 3's `[2]`: j/k pick a member, `o` goes to its pane, `s` says
    /// something to it (`@handle` first).
    pub(super) fn member_key(&mut self, code: KeyCode) -> Action {
        let n = self.chat.members.len();
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.chat.member_pick = (self.chat.member_pick + 1).min(n.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => self.chat.member_pick = self.chat.member_pick.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.chat.member_pick = 0,
            KeyCode::End | KeyCode::Char('G') => self.chat.member_pick = n.saturating_sub(1),
            KeyCode::Char('o') => {
                if let Some(m) = self.chat.members.get(self.chat.member_pick) {
                    // Offline or left: the pane it was last may be another's now.
                    if !m.online || m.left {
                        self.note = Some(format!("{} is not online: no pane to go to", m.handle));
                        return Action::Redraw;
                    }
                    return Action::Go(owned(&["focus-pane", &format!("%{}", m.pane)]));
                }
            }
            KeyCode::Char('s') => {
                if let (Some(m), Some(c)) = (self.chat.members.get(self.chat.member_pick), self.chat.picked()) {
                    self.input =
                        Some((Input::ChatSay { id: c.id.clone(), name: c.name.clone() }, format!("@{} ", m.handle)));
                }
            }
            _ => {}
        }
        Action::Redraw
    }

    /// What a chat input runs when Enter is pressed.
    pub(super) fn chat_typed(what: &Input, text: &str) -> Option<Vec<String>> {
        Some(match what {
            Input::ChatSay { id, .. } => owned(&["chat-say", id, "--", text]),
            Input::ChatNew => {
                let mut words = text.split_whitespace();
                let name = words.next()?;
                let mut argv = owned(&["chat-new", name]);
                let topic: Vec<&str> = words.collect();
                if !topic.is_empty() {
                    argv.extend(["--topic".to_string(), topic.join(" ")]);
                }
                argv
            }
            Input::ChatTopic { id, .. } => owned(&["chat-topic", id, "--", text.trim()]),
            Input::ChatTag { id, .. } => {
                let mut argv = owned(&["chat-tag", id]);
                argv.extend(
                    text.split_whitespace().map(|w| {
                        if w.starts_with('+') || w.starts_with('-') { w.to_string() } else { format!("+{w}") }
                    }),
                );
                argv
            }
            _ => return None,
        })
    }

    pub(super) fn channels_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Channels;
        let shown = self.chat.shown();
        let filter =
            if self.chat.filter.trim().is_empty() { String::new() } else { format!(" /{}", self.chat.filter.trim()) };
        let title = format!("[1] Channels{filter}");
        let w = r.inner_w();
        let mut lines: Vec<String> = Vec::new();
        if shown.is_empty() {
            lines.push(paint("2", "no channel (n makes one)"));
        }
        let per = channel_rows(r.inner_h());
        let fits = (r.inner_h() / per).max(1);
        let start = window_start(self.chat.pick, shown.len(), fits);
        let people = match self.files.icons {
            Icons::Emoji => "👥",
            Icons::Nerd => "\u{f0c0}",
        };
        for (i, c) in shown.iter().enumerate().skip(start).take(fits) {
            let picked = i == self.chat.pick;
            let unread = if c.unread > 0 { paint("1;33", &format!(" ●{}", c.unread)) } else { String::new() };
            let tags = if c.tags.is_empty() { String::new() } else { paint("2", &format!("  {}", c.tags.join(","))) };
            // Another machine's: its name after the channel's, faint.
            let at = if c.machine.is_empty() { String::new() } else { paint("2", &format!("@{}", inert(&c.machine))) };
            let left = format!(
                "{}{at} {}{unread}{tags}",
                paint("1", &format!("#{}", c.name)),
                paint("2", &format!("{people}{}", c.members))
            );
            // How long ago the last line was, against the right edge, where
            // the rest leaves room (`row_line` takes a column for its mark).
            let when = ago(&c.latest_at);
            let room = w.saturating_sub(1);
            let line = if !when.is_empty() && width_of(&left) + 2 + width_of(&when) <= room {
                format!("{}{}", fit(&left, room - width_of(&when)), paint("2", &when))
            } else {
                left
            };
            lines.push(row_line(&line, picked, focused, w));
            if per == 2 {
                let said = if c.latest_by.is_empty() {
                    c.topic.clone()
                } else {
                    format!("{}: {}", c.latest_by, c.latest_text)
                };
                // Reversed with its name in the focused panel; otherwise one
                // `>` marks the channel, on its name.
                lines.push(row_line(&paint("2", &format!("  {said}")), picked && focused, focused, w));
            }
        }
        boxed(&title, "", lines, r, focused)
    }

    pub(super) fn members_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Members;
        // Those that left are listed, last, and not counted.
        let staying = self.chat.members.iter().filter(|m| !m.left).count();
        let online = self.chat.members.iter().filter(|m| m.online && !m.left).count();
        let title = if online == staying {
            format!("[2] Members · {staying}")
        } else {
            format!("[2] Members · {online}/{staying} online")
        };
        let lines: Vec<String> = if self.chat.members.is_empty() {
            vec![paint("2", "no one joined (chat join, from a pane)")]
        } else {
            let start = window_start(self.chat.member_pick, self.chat.members.len(), r.inner_h());
            self.chat
                .members
                .iter()
                .enumerate()
                .skip(start)
                .take(r.inner_h())
                .map(|(i, m)| {
                    let state = match (m.online, m.busy) {
                        _ if m.left => paint("90", "×"),
                        (false, _) => paint("90", "○"),
                        (true, Some(true)) => paint("33", "●"),
                        (true, Some(false)) => paint("32", "●"),
                        (true, None) => paint("2", "○"),
                    };
                    let role = if m.role.is_empty() { String::new() } else { format!(" {}", paint("2", &m.role)) };
                    // Offline: said, then where it was last.
                    let at = if m.left {
                        paint("90", "  left")
                    } else if m.online {
                        paint("2", &format!("  {} %{} {}", m.place, m.pane, m.agent))
                    } else {
                        paint("90", &format!("  offline · was {} %{}", m.place, m.pane))
                    };
                    let name = paint(&format!("1;{}", self.chat.colour(&m.handle)), &m.handle);
                    let line = format!("{state} {name}{role}{at}");
                    row_line(&line, i == self.chat.member_pick, focused, r.inner_w())
                })
                .collect()
        };
        // What the dots say, where the whole title leaves room for it.
        let key = format!("{} idle {} busy {} off", paint("32", "●"), paint("33", "●"), paint("90", "○"));
        let key = if r.inner_w() >= width_of(&title) + width_of(&key) + 4 { key } else { String::new() };
        boxed(&title, &key, lines, r, focused)
    }

    /// `[4]` on page 3: the picked channel, its topic in the title, its
    /// lines with the latest at the bottom (scrolled up by `scroll`).
    pub(super) fn chat_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Main;
        let w = r.inner_w();
        let Some(c) = self.chat.picked() else {
            return boxed("[4] Chat", "", vec![paint("2", "no channel")], r, focused);
        };
        let topic = if c.topic.is_empty() { String::new() } else { format!(" · {}", c.topic) };
        let title = format!("[4] #{}{topic}", c.name);
        // At the bottom, where what the person says is typed: a rule and a
        // line, and over them the members a `@` being typed may name; when
        // there are rows for them and some for what was said.
        let typing = match &self.input {
            Some((Input::ChatSay { id, .. }, text)) if *id == c.id => Some(text.as_str()),
            _ => None,
        };
        let naming = typing.map(|t| self.chat.naming_list(t)).unwrap_or_default();
        let rows = r.inner_h();
        let with_box = rows >= 4;
        let under: Vec<String> = if with_box { self.say_box(typing, &naming, w, rows - 3) } else { Vec::new() };
        let rows = rows - under.len();
        let all = self.chat.drawn(w);
        let most = all.len().saturating_sub(rows);
        self.scroll_most.set(most);
        let end = all.len().saturating_sub(self.scroll.min(most));
        let start = end.saturating_sub(rows);
        let mut lines: Vec<String> = all[start..end].to_vec();
        if lines.is_empty() {
            lines.push(paint("2", "nothing said yet (s says something)"));
        }
        lines.resize(rows, String::new());
        lines.extend(under);
        boxed(&title, "", lines, r, focused)
    }

    /// The bottom of `[4]`: the `@` list (at most `room` rows of it, the
    /// picked one reversed), a faint rule, and the line being typed (its
    /// end, when longer than the line) or how to start one.
    fn say_box(&self, typing: Option<&str>, naming: &[&MemberRow], w: usize, room: usize) -> Vec<String> {
        let mut out = Vec::new();
        let pick = self.chat.at_pick.min(naming.len().saturating_sub(1));
        for (i, m) in naming.iter().enumerate().take(room) {
            let off = if m.online { String::new() } else { paint("90", " offline") };
            let role = if m.role.is_empty() { String::new() } else { format!(" {}", paint("2", &m.role)) };
            let line = format!(" @{}{role}{off}", paint(&format!("1;{}", self.chat.colour(&m.handle)), &m.handle));
            out.push(if i == pick { format!("\x1b[7m{}\x1b[0m", fit(&super::strip(&line), w)) } else { fit(&line, w) });
        }
        out.push(paint("90", &"─".repeat(w)));
        out.push(match typing {
            Some(text) => {
                // The end of it, the cursor after.
                let room = w.saturating_sub(3);
                let chars: Vec<char> = text.chars().collect();
                let mut start = chars.len();
                let mut used = 0;
                while start > 0 {
                    let cw = width_of(&chars[start - 1].to_string());
                    if used + cw > room {
                        break;
                    }
                    used += cw;
                    start -= 1;
                }
                let shown: String = chars[start..].iter().collect();
                fit(&format!("{} {}{}", paint("1;32", "›"), self.chat.tint_mentions(&shown), "\x1b[7m \x1b[0m"), w)
            }
            None => fit(&paint("2", "› s says something · @ names a member"), w),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_at_spaces_and_long_ones_go_in_pieces() {
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefghijkl", 8), ["abcdefgh", "ijkl"]);
        assert_eq!(wrap("a\nb", 8), ["a", "b"]);
        // Two columns a character: nothing lost, no line wider than 8.
        let zh = "群聊里说的话一个字也不能丢";
        let rows = wrap(zh, 8);
        assert_eq!(rows.concat(), zh, "{rows:?}");
        assert!(rows.iter().all(|r| width_of(r) <= 8), "{rows:?}");
        // A word longer than a line starts where the last one ended: a
        // handle and a sentence of Chinese keep the handle's line full.
        assert_eq!(wrap("ok 一二三四五六", 8), ["ok 一二", "三四五六"]);
        assert_eq!(wrap("@lead 群聊里说的话一个字也不能丢", 12), ["@lead 群聊里", "说的话一个字", "也不能丢"]);
    }

    /// A list item's continuation lines go in as far as its text; an
    /// indented paragraph keeps its indent; text that is neither wraps as
    /// before.
    #[test]
    fn list_items_hang_under_their_text() {
        assert_eq!(wrap("- one two three four", 10), ["- one two", "  three", "  four"]);
        assert_eq!(wrap("12) one two three", 10), ["12) one", "    two", "    three"]);
        assert_eq!(wrap("  plain text here", 10), ["  plain", "  text", "  here"]);
        assert_eq!(wrap("one two three", 8), ["one two", "three"], "no list: as before");
        let zh = wrap("1. 标签页里再运行 pwsh，子 shell 继承变量", 16);
        assert!(zh[1..].iter().all(|l| l.starts_with("   ") && !l.starts_with("    ")), "{zh:?}");
        assert!(zh.iter().all(|l| width_of(l) <= 16), "{zh:?}");
        assert_eq!(hang_of("3、第三"), 3);
        assert_eq!(hang_of("• dot"), 2);
        assert_eq!(hang_of("2026 was"), 0, "a number that is not an item");
        assert_eq!(hang_of("-dash"), 0);
        // A hang wider than half the line: none, the text keeps its room.
        assert_eq!(wrap("        - a b c", 8), ["- a b c"]);
    }

    fn line(seq: u64, at: &str, kind: &str, handle: &str, text: &str) -> String {
        format!(
            r#"{{"seq":{seq},"at":"{at}","kind":"{kind}","from":{{"handle":"{handle}"}},"text":{}}}"#,
            serde_json::Value::String(text.into())
        )
    }

    fn page(lines: &[String], unread: u64) -> ChatPage {
        let mut p = ChatPage::default();
        p.set_channels(&format!(r#"[{{"id":"0190-a","name":"review","members":2,"unread":{unread}}}]"#));
        p.set_members(r#"[{"handle":"lead","ident":{"pane":3}},{"handle":"tester","ident":{"pane":4}}]"#);
        p.set_lines(&format!(r#"{{"lines":[{}]}}"#, lines.join(",")));
        p
    }

    fn plain(p: &ChatPage, w: usize) -> Vec<String> {
        p.drawn(w).iter().map(|l| crate::dashboard::strip(l)).collect()
    }

    /// One member's lines within five minutes share a bubble; a line of
    /// another, a later one, what happened or a new day starts another.
    #[test]
    fn a_member_s_lines_close_in_time_share_a_bubble() {
        let at = |hm: &str| format!("2026-11-17T{hm}:00.000+08:00");
        let p = page(
            &[
                line(1, &at("14:00"), "say", "lead", "one"),
                line(2, &at("14:04"), "say", "lead", "two"),
                line(3, &at("14:10"), "say", "lead", "three"),
                line(4, &at("14:11"), "say", "tester", "four"),
                line(5, &at("14:12"), "join", "x", "joined"),
                line(6, &at("14:12"), "say", "tester", "five"),
                line(7, "2026-11-18T09:00:00.000+08:00", "say", "tester", "six"),
            ],
            0,
        );
        let d = plain(&p, 40);
        let tops = d.iter().filter(|l| l.trim_start().starts_with('╭')).count();
        assert_eq!(tops, 5, "one+two, three, four, five, six: {d:#?}");
        let one = d.iter().position(|l| l.contains("one")).unwrap();
        assert!(d[one + 1].contains("two"), "two under one, no new edge: {d:#?}");
        assert!(d.iter().any(|l| l.contains("14:12 · x joined")), "{d:#?}");
        let days: Vec<&String> = d.iter().filter(|l| l.starts_with('─')).collect();
        assert_eq!(days.len(), 2, "a rule at each day: {d:#?}");
        assert!(days[1].contains("11-18") && days[1].contains("Wed"), "{days:?}");
        assert!(d.iter().all(|l| width_of(l) == 40), "every line exactly as wide: {d:#?}");
    }

    /// Where what the person had not read begins: a rule there, kept when
    /// showing it reads it, gone with another channel picked.
    #[test]
    fn the_unread_rule_stays_while_its_channel_is_shown() {
        let at = |m: u32| format!("2026-11-17T14:{m:02}:00.000+08:00");
        let lines: Vec<String> =
            (1..=4).map(|i| line(i, &at(i as u32 * 10), "say", "lead", &format!("l{i}"))).collect();
        let mut p = page(&lines, 2);
        assert_eq!(p.divider, Some(("0190-a".into(), 3)), "the second last line said");
        let d = plain(&p, 40);
        let rule = d.iter().position(|l| l.contains(" 2 new ")).expect("the rule");
        assert!(d[rule + 2].contains("l3"), "{d:#?}");
        // Shown, it is read: the server says nothing unread; the rule stays.
        p.set_channels(r#"[{"id":"0190-a","name":"review","members":2,"unread":0}]"#);
        p.set_lines(&format!(r#"{{"lines":[{}]}}"#, lines.join(",")));
        assert!(plain(&p, 40).iter().any(|l| l.contains(" 2 new ")));
        // A line said while it is shown: the rule where it was, counting it.
        let mut more = lines.clone();
        more.push(line(5, &at(55), "say", "tester", "l5"));
        p.set_channels(r#"[{"id":"0190-a","name":"review","members":2,"unread":1}]"#);
        p.set_lines(&format!(r#"{{"lines":[{}]}}"#, more.join(",")));
        assert_eq!(p.divider, Some(("0190-a".into(), 3)));
        assert!(plain(&p, 40).iter().any(|l| l.contains(" 3 new ")));
        // Another channel picked: gone.
        p.set_channels(r#"[{"id":"0190-b","name":"other","members":0,"unread":0}]"#);
        p.set_lines(r#"{"lines":[]}"#);
        assert_eq!(p.divider, None);
        // Nothing unread: no rule.
        assert!(!plain(&page(&lines, 0), 40).iter().any(|l| l.contains(" new ")));
        // The person's own lines are not unread (the server does not count
        // them): the rule goes before the first line of another.
        let mut mixed = lines.clone();
        mixed.push(line(5, &at(51), "say", "you@DFINE", "mine"));
        let p = page(&mixed, 1);
        assert_eq!(p.divider, Some(("0190-a".into(), 4)), "lead's last, not the person's");
        assert!(plain(&p, 40).iter().any(|l| l.contains(" 1 new ")));
    }

    /// Colours by the order the members joined, the person's ink for the
    /// person, one who left the same each time; `@` a member or the person
    /// in that colour, other `@`s as they are; the person's bubble against
    /// the right edge; a heavy left side when it names `@you`.
    #[test]
    fn each_one_has_a_colour_and_the_person_is_on_the_right() {
        let at = "2026-11-17T14:00:00.000+08:00";
        let p = page(
            &[line(1, at, "say", "tester", "@you look, @lead, mail a@b"), line(2, at, "say", "you@DFINE", "ok")],
            0,
        );
        assert_eq!((p.colour("lead"), p.colour("tester"), p.colour("you@DFINE")), ("36", "35", "1"));
        let gone = p.colour("gone");
        assert!(PALETTE.contains(&gone) && p.colour("gone") == gone);
        let tinted = p.tint_mentions("@you look, @lead, mail a@b @nobody");
        assert!(tinted.contains("\x1b[1;1m@you\x1b[0m") && tinted.contains("\x1b[1;36m@lead\x1b[0m"), "{tinted:?}");
        assert!(tinted.ends_with(" mail a@b @nobody"), "{tinted:?}");
        let d = plain(&p, 40);
        let side = d.iter().find(|l| l.contains("@you look")).unwrap();
        assert!(side.starts_with('┃'), "{side:?}");
        let mine = d.iter().find(|l| l.contains(" ok ")).unwrap();
        assert!(mine.starts_with("    ") && mine.ends_with('│'), "against the right edge: {mine:?}");
        let raw = p.drawn(40).join("\n");
        assert!(raw.contains("\x1b[35m╭─"), "tester's edge in its colour");
        // Narrow: no bubbles, a line for who and the text under it.
        let narrow = plain(&p, 15);
        assert!(!narrow.iter().any(|l| l.contains('╭')) && narrow.iter().any(|l| l.starts_with("14:00 tester")));
    }

    /// Whatever is said and however wide `[4]` is, every line it draws is
    /// exactly that wide: no bubble breaks out of its panel.
    #[test]
    fn every_drawn_line_is_as_wide_as_the_panel() {
        let at = "2026-11-17T14:00:00.000+08:00";
        let texts = [
            "",
            "x",
            "群聊里说的话一个字也不能丢，一个字也不能丢，一个字也不能丢",
            "- a list item that goes on and on and on\n  and an indented line",
            "@lead@tester@you 多个 @ 连着",
            "averyveryverylongwordwithoutanyspacesatallforsure",
        ];
        let lines: Vec<String> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| line(i as u64 + 1, at, "say", ["lead", "you@DFINE", "tester"][i % 3], t))
            .collect();
        let p = page(&lines, 2);
        for w in 1..=90 {
            for l in p.drawn(w) {
                assert_eq!(width_of(&l), w, "at {w}: {l:?}");
            }
        }
    }

    #[test]
    fn rules_and_times_read_short() {
        assert_eq!(crate::dashboard::strip(&rule("4 new", 15, "33")), "──── 4 new ────");
        assert_eq!(width_of(&rule("a long label", 6, "2")), 6, "no room for it: the rule alone");
        let now = chrono::Local::now().fixed_offset();
        let ago_of = |s: i64| ago(&(now - chrono::Duration::seconds(s)).to_rfc3339());
        assert_eq!(
            (ago_of(5), ago_of(300), ago_of(7200), ago_of(3 * 86400)),
            ("now".into(), "5m".into(), "2h".into(), "3d".into())
        );
        assert_eq!(ago("not a time"), "");
        assert_eq!(day_label("2001-02-03"), "Sat 2001-02-03", "another year: the year too");
        assert_eq!(day_label("rubbish"), "rubbish");
    }

    /// What a member says is drawn as text: its escape sequences (a clear
    /// screen, a cursor move) never reach the terminal.
    #[test]
    fn what_is_said_cannot_draw_on_the_board() {
        let mut p = ChatPage::default();
        p.set_lines(
            r#"{"lines":[{"seq":1,"at":"2026-11-17T14:02:00.000+08:00","kind":"say",
                "from":{"handle":"x\u001b[31m","role":"r"},"text":"a\u001b[2J\u001b[Hb\tc\rd"}]}"#,
        );
        assert_eq!(p.lines[0].text, "ab c d");
        assert_eq!(p.lines[0].handle, "x");
        // A file's name is the sender's too: drawn as text; each file on a
        // line of its own under the text, with its size.
        p.set_lines(
            r#"{"lines":[{"seq":1,"at":"2026-11-17T14:02:00.000+08:00","kind":"say","from":{"handle":"x"},
                "text":"see","files":[{"name":"a\u001b[2Jb.png","size":1536,"file":"2026-11-17/1/ab.png"},
                {"name":"c.md","size":20,"file":"2026-11-17/1/c.md"}]}]}"#,
        );
        assert_eq!(p.lines[0].text, "see\n📎 ab.png  1.5K\n📎 c.md  20B");
        p.set_lines(
            r#"{"lines":[{"seq":1,"at":"2026-11-17T14:02:00.000+08:00","kind":"say","from":{"handle":"x"},
                "text":"","files":[{"name":"c.md","size":20,"file":"2026-11-17/1/c.md"}]}]}"#,
        );
        assert_eq!(p.lines[0].text, "📎 c.md  20B", "a file alone: no empty line above it");
        // A reply: the start of the line it answers above it; the number
        // alone when that line is not among those loaded.
        p.set_lines(
            r#"{"lines":[{"seq":4,"at":"2026-11-17T14:02:00.000+08:00","kind":"say","from":{"handle":"lead"},
                "text":"the plan for the 0.88 release is ready, please look at the second part first"},
                {"seq":5,"at":"2026-11-17T14:03:00.000+08:00","kind":"say","from":{"handle":"x"},"text":"ok","re":4},
                {"seq":6,"at":"2026-11-17T14:04:00.000+08:00","kind":"say","from":{"handle":"x"},"text":"and","re":2}]}"#,
        );
        assert_eq!(p.lines[1].text, "↩ #4 lead: the plan for the 0.88 release is ready, …\nok");
        assert_eq!(p.lines[2].text, "↩ #2\nand");
        // Zero-width format characters go: Windows' console counts a joiner
        // a column, keepane none, and the rows after one would stand off.
        p.set_lines(
            r#"{"lines":[{"seq":1,"at":"2026-11-17T14:02:00.000+08:00","kind":"say","from":{"handle":"x"},
                "text":"b\u200dc \ud83d\udc68\u200d\ud83d\udc69 \u200b\u2060\ufeffend"}]}"#,
        );
        assert_eq!(p.lines[0].text, "bc \u{1F468}\u{1F469} end");
    }

    #[test]
    fn the_server_s_json_reads_into_rows() {
        let mut p = ChatPage::default();
        p.set_channels(
            r#"[{"id":"0190-a","name":"general","topic":"","tags":[],"members":0,"unread":0},
                {"id":"0190-b","name":"review","topic":"the release","tags":["ci","release"],"members":2,"unread":3}]"#,
        );
        assert_eq!(p.channels.len(), 2);
        p.pick = 1;
        assert_eq!(p.picked().unwrap().name, "review");
        // The picked one stays picked when the list changes around it.
        p.set_channels(
            r#"[{"id":"0190-c","name":"new","members":0},{"id":"0190-a","name":"general","members":0},
                {"id":"0190-b","name":"review","tags":["ci"],"members":2,"unread":0}]"#,
        );
        assert_eq!(p.picked().unwrap().id, "0190-b");
        assert!(p.channels[0].latest_by.is_empty(), "nothing said: no latest");
        p.set_channels(
            r#"[{"id":"0190-c","name":"new","members":0},{"id":"0190-a","name":"general","members":0},
                {"id":"0190-b","name":"review","tags":["ci"],"members":2,
                "latest":{"at":"2026-11-17T14:02:00.000+08:00","handle":"lead","text":"two\nlines"}}]"#,
        );
        let review = &p.channels[2];
        assert_eq!((review.latest_by.as_str(), review.latest_text.as_str()), ("lead", "two lines"));
        // A filter by a tag.
        p.filter = "ci".into();
        assert_eq!(p.shown().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["review"]);
        p.set_members(
            r#"[{"handle":"lead","role":"planner","ident":{"machine":"DFINE","place":"s:0.0","pane":3,"agent":"claude"},"busy":true}]"#,
        );
        assert_eq!((p.members[0].pane, p.members[0].busy), (3, Some(true)));
        p.set_lines(
            r#"{"lines":[{"seq":4,"at":"2026-11-17T14:02:33.120+08:00","kind":"say","from":{"handle":"lead","role":"planner",
                "ident":{"machine":"DFINE","place":"s:0.0","pane":3,"agent":"claude","model":"opus"}},"text":"hi"},
                {"seq":5,"at":"2026-11-17T14:03:00.000+08:00","kind":"say","from":{"handle":"you@DFINE"},"text":"yo"}]}"#,
        );
        assert_eq!(p.lines[0].at, "14:02");
        assert_eq!(p.lines[0].day, "2026-11-17");
        assert!(p.lines[0].when.is_some());
        assert_eq!((p.lines[0].handle.as_str(), p.lines[1].handle.as_str()), ("lead", "you@DFINE"));
        // Rubbish reads as nothing.
        p.set_channels("not json");
        assert!(p.channels.is_empty() && p.picked().is_none());
    }

    #[test]
    fn what_each_input_runs() {
        let say = Input::ChatSay { id: "ID".into(), name: "review".into() };
        assert_eq!(Board::chat_typed(&say, "-x hi").unwrap(), ["chat-say", "ID", "--", "-x hi"]);
        assert_eq!(
            Board::chat_typed(&Input::ChatNew, "ops the ops room").unwrap(),
            ["chat-new", "ops", "--topic", "the ops room"]
        );
        assert_eq!(Board::chat_typed(&Input::ChatNew, "ops").unwrap(), ["chat-new", "ops"]);
        assert_eq!(Board::chat_typed(&Input::ChatNew, "  "), None);
        let tag = Input::ChatTag { id: "ID".into(), name: "review".into() };
        assert_eq!(Board::chat_typed(&tag, "ci -old +new").unwrap(), ["chat-tag", "ID", "+ci", "-old", "+new"]);
    }
}
