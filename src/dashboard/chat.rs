//! The board's page 3: the group chat (docs/design/chat.md). `[1]` the
//! channels (members, what the person has not read, tags), `[2]` the
//! picked channel's members (handle, role, who and where each is), `[4]`
//! what was said there, the latest at the bottom. Read from the server
//! with `chat-list -J`, `chat-who -J` and `chat-read --all -J`.

use super::{Action, Board, Input, Panel, Rect, boxed, fit, inert, owned, paint, row_line, width_of, window_start};
use crate::keys::KeyCode;

/// The lines of the picked channel read for `[4]`.
pub const SHOWN: usize = 300;

/// A channel, as `chat-list -J` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChanRow {
    pub id: String,
    pub name: String,
    pub topic: String,
    pub tags: Vec<String>,
    pub members: usize,
    pub unread: u64,
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
}

/// A line of the channel, as `chat-read -J` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatLine {
    pub seq: u64,
    /// `14:02`.
    pub at: String,
    /// `say`, or what happened (`join`, `leave`, `topic`, ...).
    pub kind: String,
    pub handle: String,
    pub role: String,
    /// Who and where it was, in a line (`DFINE s:0.0 %3 claude`); empty for
    /// the person.
    pub ident: String,
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
}

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

    /// `chat-list -J`; the channel picked stays picked (by its id).
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
                    })
                    .filter(|c| !c.id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let (at, n) = {
            let shown = self.shown();
            (was.and_then(|id| shown.iter().position(|c| c.id == id)), shown.len())
        };
        self.pick = at.unwrap_or(self.pick).min(n.saturating_sub(1));
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
                        let from = &l["from"];
                        let i = &from["ident"];
                        let ident = if i.is_object() {
                            let mut parts =
                                vec![s(i, "machine"), s(i, "place"), format!("%{}", i["pane"].as_u64().unwrap_or(0))];
                            parts.extend([s(i, "agent"), s(i, "model")].into_iter().filter(|p| !p.is_empty()));
                            parts.join(" ")
                        } else {
                            String::new()
                        };
                        let at = s(l, "at");
                        ChatLine {
                            seq: l["seq"].as_u64().unwrap_or(0),
                            at: at.get(11..16).unwrap_or(&at).to_string(),
                            kind: s(l, "kind"),
                            handle: s(from, "handle"),
                            role: s(from, "role"),
                            ident,
                            text: s(l, "text"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
}

/// `text` cut into lines of `width` columns (at spaces when it can).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let w = width_of(word);
            if !line.is_empty() && width_of(&line) + 1 + w > width {
                out.push(std::mem::take(&mut line));
            }
            let mut word = word.to_string();
            // A word longer than a line (Chinese has no spaces) goes in
            // pieces, each a line, as the board wraps any line.
            if w > width {
                let mut pieces = super::wrap(&word, width);
                word = pieces.pop().unwrap_or_default();
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                out.extend(pieces);
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    out
}

impl Board {
    /// What page 3 asks the server each refresh, the picked channel's
    /// members and lines with the list (`run` asks them in this order).
    pub(super) fn chat_queries(&self) -> Vec<Vec<String>> {
        let mut q = vec![owned(&["chat-list", "-J"])];
        if let Some(c) = self.chat.picked() {
            q.push(owned(&["chat-who", &c.id, "-J"]));
            q.push(owned(&["chat-read", &c.id, "--all", "-n", &SHOWN.to_string(), "-J"]));
        }
        q
    }

    /// The picked channel's lines on the screen: what the person has not
    /// read of it is read now.
    pub(super) fn chat_seen(&self) -> Option<Vec<String>> {
        // On the screen only: a narrow board shows [4] while it has the focus.
        let c = self.chat.picked().filter(|c| c.unread > 0 && self.page == 3 && self.layout().main.is_some())?;
        Some(owned(&["chat-read", &c.id, "-n", "1000"]))
    }

    /// Page 3's `[1]`: j/k pick, `s` say, `n` a new channel, `t` its topic,
    /// `#` its tags, Enter into `[4]`.
    pub(super) fn channel_key(&mut self, code: KeyCode) -> Action {
        let n = self.chat.shown().len();
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.chat.pick = (self.chat.pick + 1).min(n.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.chat.pick = self.chat.pick.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.chat.pick = 0,
            KeyCode::End | KeyCode::Char('G') => self.chat.pick = n.saturating_sub(1),
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
        let lines: Vec<String> = if shown.is_empty() {
            vec![paint("2", "no channel (n makes one)")]
        } else {
            let start = window_start(self.chat.pick, shown.len(), r.inner_h());
            shown
                .iter()
                .enumerate()
                .skip(start)
                .take(r.inner_h())
                .map(|(i, c)| {
                    let unread = if c.unread > 0 { paint("1;33", &format!(" ●{}", c.unread)) } else { String::new() };
                    let tags =
                        if c.tags.is_empty() { String::new() } else { paint("2", &format!("  {}", c.tags.join(","))) };
                    let line = format!(
                        "{} {}{unread}{tags}",
                        paint("1", &format!("#{}", c.name)),
                        paint("2", &c.members.to_string())
                    );
                    row_line(&line, i == self.chat.pick, focused, r.inner_w())
                })
                .collect()
        };
        boxed(&title, "", lines, r, focused)
    }

    pub(super) fn members_box(&self, r: Rect) -> Vec<String> {
        let focused = self.focus == Panel::Members;
        let title = format!("[2] Members · {}", self.chat.members.len());
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
                    let state = match m.busy {
                        Some(true) => paint("33", "●"),
                        Some(false) => paint("32", "●"),
                        None => paint("2", "○"),
                    };
                    let role = if m.role.is_empty() { String::new() } else { format!(" {}", paint("36", &m.role)) };
                    let at = paint("2", &format!("  {} %{} {}", m.place, m.pane, m.agent));
                    let line = format!("{state} {}{role}{at}", paint("1", &m.handle));
                    row_line(&line, i == self.chat.member_pick, focused, r.inner_w())
                })
                .collect()
        };
        boxed(&title, "", lines, r, focused)
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
        let mut all: Vec<String> = Vec::new();
        for l in &self.chat.lines {
            let who = if l.role.is_empty() {
                paint("1;36", &l.handle)
            } else {
                format!("{} {}", paint("1;36", &l.handle), paint("36", &l.role))
            };
            if l.kind == "say" {
                let at = if l.ident.is_empty() { String::new() } else { paint("2", &format!("  {}", l.ident)) };
                all.push(fit(&format!("{} {who}{at}", paint("2", &l.at)), w));
                for t in wrap(&l.text, w.saturating_sub(2)) {
                    all.push(fit(&format!("  {t}"), w));
                }
            } else {
                let by = if l.handle.is_empty() { String::new() } else { format!("{} ", l.handle) };
                all.push(fit(&paint("2", &format!("{} · {by}{}", l.at, l.text)), w));
            }
        }
        let rows = r.inner_h();
        let most = all.len().saturating_sub(rows);
        self.scroll_most.set(most);
        let end = all.len().saturating_sub(self.scroll.min(most));
        let start = end.saturating_sub(rows);
        let mut lines: Vec<String> = all[start..end].to_vec();
        if lines.is_empty() {
            lines.push(paint("2", "nothing said yet (s says something)"));
        }
        boxed(&title, "", lines, r, focused)
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
        assert_eq!(wrap("ok 一二三四五六", 8), ["ok", "一二三四", "五六"]);
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
        assert_eq!(p.lines[0].ident, "DFINE s:0.0 %3 claude opus");
        assert_eq!(p.lines[1].ident, "", "the person: no stamp");
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
