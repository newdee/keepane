//! `keepane settings` (prefix g): the settings board
//! (docs/design/settings.md). Every option in one of nine boxes that stay
//! where they are; a box can be zoomed to the whole board and back, never
//! moved, hidden or taken away. A value that can be picked is picked (←/→),
//! the rest typed on the bottom line; each change is set at once
//! (`set-option`), and written to the settings file only on `w`
//! (`save-settings`). The config file the person wrote is never written.
//! Under its options the Keys box lists the prefix table's keys: Enter,
//! then the key it should be on (`rebind-key`; one bound already is asked
//! about and swapped), `r` back where the config left it; `w` writes them
//! too (`save-settings -k`).
//!
//! It asks the server only what `QUERIES` holds and changes only what
//! `ACTIONS` holds; the board is kept apart from the console, so it is
//! tested without one.

use crate::dashboard::{Rect, boxed, clip, fit, fit_right, key_hints, paint, strip, width_of, window_start};
use crate::ipc::MouseRecord;
use crate::keys::{Key, KeyCode};
use crate::optinfo::{self, GROUPS, Kind, OPTIONS, OptInfo};
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

/// The only server commands the board sends to look.
pub const QUERIES: &[&str] = &["show-settings", "show-bindings", "show-format"];
/// The only ones it sends to change anything.
pub const ACTIONS: &[&str] = &["set-option", "save-settings", "reload-settings", "rebind-key", "bind-key"];

/// The box the prefix table's keys are listed in, under its options.
const KEYS_GROUP: usize = 2;

/// A key of the prefix table as `show-bindings -J` gives it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BindRow {
    /// Empty: what the config bound here runs on no key now.
    pub key: String,
    pub command: String,
    pub repeat: bool,
    /// The key its command had when the config had run (none: bound since).
    pub base: Option<String>,
}

/// A line of a box: an option, or a key of the prefix table (`binds`).
#[derive(Clone, Copy)]
enum Item {
    Opt(&'static OptInfo),
    Bind(usize),
}

/// An option as `show-settings -J` gives it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Row {
    pub value: String,
    pub default: String,
    /// Who set it last: `default`, a file's line, `theme NAME`, `set`.
    pub source: String,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Redraw,
    Quit,
    /// A server command to run.
    Run(Vec<String>),
    /// One to run, then out when it went well (write, then quit).
    RunThenQuit(Vec<String>),
}

#[derive(Debug, PartialEq)]
enum Edit {
    /// Text typed on the bottom line: an option's new value, or (no name)
    /// the filter; the cursor counted in characters.
    Line { name: Option<String>, text: String, at: usize, secret: bool },
    /// The next key is the new prefix.
    Key,
    /// The next key is where key `binds[i]` goes.
    Bind(usize),
}

#[derive(Debug, PartialEq, Clone)]
enum Ask {
    /// Quit with changes not written.
    Quit,
    /// Everything back to the defaults, the files run again.
    Reload,
    /// `binds[i]` to a key bound already: swap the two?
    Swap(usize, String),
}

/// Columns from which the boxes stand three, then two, abreast.
const THREE_AT: usize = 114;
const TWO_AT: usize = 76;
/// The fewest rows a box is given; fewer, and the board scrolls by boxes.
const BOX_LEAST: usize = 7;
/// Rows under a zoomed box's table: a rule, two of what it does, a hint.
const ZOOM_FOOT: usize = 4;

pub struct Board {
    rows: HashMap<String, Row>,
    /// The settings file, as `~/...`; none when the config was named.
    pub file: Option<String>,
    /// The box the keys are in.
    pub group: usize,
    picks: [usize; GROUPS.len()],
    pub zoom: bool,
    edit: Option<Edit>,
    ask: Option<Ask>,
    /// Only options whose name or line holds this (any case).
    pub filter: String,
    /// Only options away from their default (`m`).
    pub changed_only: bool,
    /// Options changed here and not written yet (`●`).
    pub unsaved: BTreeSet<String>,
    /// The prefix table's keys (`show-bindings -J`).
    pub binds: Vec<BindRow>,
    /// What `theme` takes, the user's own themes too (`show-settings -J`'s
    /// `themes`); empty from an older server: the built-in ones.
    themes: Vec<String>,
    /// While a format is typed: the text the server last expanded and what
    /// it came to (`show-format -J`), each run its SGR and its text.
    preview: Option<(String, Vec<(String, String)>)>,
    /// The commands whose keys were moved here and not written yet (`●`).
    pub keys_unsaved: BTreeSet<String>,
    /// The commands the change on its way moves: marked when it went well.
    moving: Vec<String>,
    /// What the last change did, or what went wrong (true).
    note: Option<(String, bool)>,
    pub cols: u16,
    pub height: u16,
}

fn owned(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// A value as the board shows it: control characters as spaces.
fn inert(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// `s` cut to `width` columns, `…` at the end when cut.
fn cut(s: &str, width: usize) -> String {
    if width_of(s) <= width {
        s.to_string()
    } else if width == 0 {
        String::new()
    } else {
        clip(s, width - 1).0 + "…"
    }
}

impl Board {
    pub fn new(cols: u16, height: u16) -> Board {
        Board {
            rows: HashMap::new(),
            file: None,
            group: 0,
            picks: [0; GROUPS.len()],
            zoom: false,
            edit: None,
            ask: None,
            filter: String::new(),
            changed_only: false,
            unsaved: BTreeSet::new(),
            binds: Vec::new(),
            themes: Vec::new(),
            preview: None,
            keys_unsaved: BTreeSet::new(),
            moving: Vec::new(),
            note: None,
            cols,
            height,
        }
    }

    pub fn resize(&mut self, cols: u16, height: u16) {
        self.cols = cols;
        self.height = height;
    }

    /// `show-settings -J`, read; an answer that does not read leaves what
    /// was there and says why.
    pub fn set_settings(&mut self, json: &str) {
        let v: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => {
                self.note = Some((json.lines().next().unwrap_or("show-settings said nothing").to_string(), true));
                return;
            }
        };
        self.file = v["file"].as_str().map(String::from);
        self.themes = v["themes"]
            .as_array()
            .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let s = |o: &serde_json::Value, k: &str| o[k].as_str().unwrap_or_default().to_string();
        self.rows = v["options"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|o| {
                        (s(o, "name"), Row { value: s(o, "value"), default: s(o, "default"), source: s(o, "source") })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    /// `show-bindings -J`, read; an answer that does not read (an older
    /// server) lists no keys.
    /// The text to have expanded (`show-format -J`): what is typed for a
    /// format option, when the server has not expanded it yet.
    pub fn wants_preview(&self) -> Option<String> {
        let Some(Edit::Line { name: Some(name), text, .. }) = &self.edit else { return None };
        let format = optinfo::OPTIONS.iter().any(|o| o.name == name && o.kind == Kind::Format);
        (format && self.preview.as_ref().is_none_or(|(t, _)| t != text)).then(|| text.clone())
    }

    /// `show-format -J`'s answer for `text`: each run's SGR (its colours as
    /// the board's terminal draws them, bold, ...) and its text; an answer
    /// that does not read is shown as it came, faint.
    pub fn set_preview(&mut self, text: &str, json: &str) {
        fn colour(v: &serde_json::Value, base: u8) -> Option<String> {
            if let Some(n) = v.as_u64() {
                return Some(format!("{};5;{n}", base));
            }
            let hex = v.as_str()?.strip_prefix('#')?;
            let p = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            Some(format!("{base};2;{};{};{}", p(0)?, p(2)?, p(4)?))
        }
        let runs = match serde_json::from_str::<Vec<serde_json::Value>>(json) {
            Ok(segs) => segs
                .iter()
                .map(|s| {
                    let mut sgr: Vec<String> = Vec::new();
                    for (k, code) in
                        [("bold", "1"), ("dim", "2"), ("italic", "3"), ("underline", "4"), ("inverse", "7")]
                    {
                        if s[k].as_bool() == Some(true) {
                            sgr.push(code.into());
                        }
                    }
                    sgr.extend(colour(&s["fg"], 38));
                    sgr.extend(colour(&s["bg"], 48));
                    (sgr.join(";"), inert(s["text"].as_str().unwrap_or_default()))
                })
                .collect(),
            Err(_) => vec![("2".into(), inert(json.lines().next().unwrap_or_default()))],
        };
        self.preview = Some((text.to_string(), runs));
    }

    pub fn set_bindings(&mut self, json: &str) {
        let s = |o: &serde_json::Value, k: &str| o[k].as_str().unwrap_or_default().to_string();
        self.binds = serde_json::from_str::<Vec<serde_json::Value>>(json)
            .map(|rows| {
                rows.iter()
                    .map(|o| BindRow {
                        key: s(o, "key"),
                        command: s(o, "command"),
                        repeat: o["repeat"].as_bool().unwrap_or(false),
                        base: o["base"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    /// What box `g` lists now: its options the server has (an older one may
    /// lack some), then, in the Keys box, the prefix table's keys; only
    /// those the filter keeps.
    fn items(&self, g: usize) -> Vec<Item> {
        let filter = self.filter.to_lowercase();
        let mut out: Vec<Item> = OPTIONS
            .iter()
            .filter(|o| o.group == g)
            .filter(|o| self.rows.get(o.name).is_some_and(|r| !self.changed_only || r.value != r.default))
            .filter(|o| filter.is_empty() || o.name.contains(&filter) || o.doc.to_lowercase().contains(&filter))
            .map(Item::Opt)
            .collect();
        if g == KEYS_GROUP {
            out.extend(
                self.binds
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| !self.changed_only || b.base.as_deref() != Some(b.key.as_str()))
                    .filter(|(_, b)| {
                        filter.is_empty()
                            || b.key.to_lowercase().contains(&filter)
                            || b.command.to_lowercase().contains(&filter)
                    })
                    .map(|(i, _)| Item::Bind(i)),
            );
        }
        out
    }

    fn pick(&self, g: usize) -> usize {
        self.picks[g].min(self.items(g).len().saturating_sub(1))
    }

    fn picked_item(&self) -> Option<Item> {
        self.items(self.group).get(self.pick(self.group)).copied()
    }

    /// The option the keys are on (none on a key of the prefix table).
    pub fn picked(&self) -> Option<&'static OptInfo> {
        match self.picked_item()? {
            Item::Opt(o) => Some(o),
            Item::Bind(_) => None,
        }
    }

    /// The prefix key as the options say it (`C-b` until read).
    fn prefix(&self) -> String {
        self.rows.get("prefix").map(|r| r.value.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| "C-b".into())
    }

    /// A key of the prefix table as a line names it: the prefix, then it.
    fn bind_name(&self, b: &BindRow) -> String {
        if b.key.is_empty() { "(no key)".into() } else { format!("{} {}", self.prefix(), b.key) }
    }

    /// `binds[i]` to key `to`: moved there; `to` bound already: asked
    /// first; a key the config bound that has none now: bound there.
    fn move_bind(&mut self, i: usize, to: &str, swap: bool) -> Action {
        let Some(b) = self.binds.get(i).cloned() else { return Action::Redraw };
        if b.key == to {
            return Action::Redraw;
        }
        let there = self.binds.iter().find(|x| x.key == to).cloned();
        if b.key.is_empty() {
            if let Some(there) = there {
                self.note = Some((format!("{to} is bound to {} already", there.command), true));
                return Action::Redraw;
            }
            let Ok(words) = crate::command::tokenize(&b.command) else { return Action::Redraw };
            let mut argv = vec!["bind-key".to_string()];
            if b.repeat {
                argv.push("-r".into());
            }
            argv.push(to.to_string());
            argv.extend(words);
            self.moving = vec![b.command];
            return Action::Run(argv);
        }
        match there {
            Some(_) if !swap => {
                self.ask = Some(Ask::Swap(i, to.to_string()));
                Action::Redraw
            }
            Some(there) => {
                self.moving = vec![b.command, there.command];
                Action::Run(owned(&["rebind-key", "-s", &b.key, to]))
            }
            None => {
                self.moving = vec![b.command];
                Action::Run(owned(&["rebind-key", &b.key, to]))
            }
        }
    }

    fn row(&self, name: &str) -> Row {
        self.rows.get(name).cloned().unwrap_or_default()
    }

    fn set(name: &str, value: &str) -> Action {
        Action::Run(owned(&["set-option", "-g", name, value]))
    }

    /// The picked option one step on (`dir` 1) or back (-1): on/off, the
    /// next choice or colour, the number by its step (never below 0).
    fn step(&mut self, o: &OptInfo, dir: i64) -> Action {
        let v = self.row(o.name).value;
        if let Kind::Number(step) = o.kind {
            let n: i64 = v.trim().parse().unwrap_or(0);
            return Board::set(o.name, &(n + dir * step as i64).max(0).to_string());
        }
        let steps = self.steps(o);
        if steps.is_empty() {
            self.note = Some(("Enter to type it".into(), false));
            return Action::Redraw;
        }
        let len = steps.len() as i64;
        let next = match steps.iter().position(|s| *s == v) {
            Some(i) => (i as i64 + dir).rem_euclid(len),
            None if dir > 0 => 0,
            None => len - 1,
        };
        Board::set(o.name, &steps[next as usize])
    }

    /// What ←/→ go through for `o`: `theme` the themes the server has,
    /// the user's own too; the rest as the option list says.
    fn steps(&self, o: &OptInfo) -> Vec<String> {
        if o.name == "theme" && !self.themes.is_empty() {
            return self.themes.clone();
        }
        optinfo::steps(o).iter().map(|s| s.to_string()).collect()
    }

    fn save(&self) -> Vec<String> {
        let mut argv = vec!["save-settings".to_string()];
        if !self.keys_unsaved.is_empty() {
            argv.push("-k".into());
        }
        argv.extend(self.unsaved.iter().cloned());
        argv
    }

    /// Changes not written: options, and keys moved.
    fn not_written(&self) -> usize {
        self.unsaved.len() + self.keys_unsaved.len()
    }

    fn set_by_hand(&self) -> usize {
        self.rows.values().filter(|r| r.source == "set").count()
    }

    pub fn key(&mut self, k: Key) -> Action {
        if let Some(e) = self.edit.take() {
            return self.edit_key(e, k);
        }
        if let Some(a) = self.ask.take() {
            return self.ask_key(a, k);
        }
        self.note = None;
        let g = self.group;
        let n = self.items(g).len();
        let plain = !k.ctrl && !k.alt;
        match k.code {
            KeyCode::Char('j') | KeyCode::Down if plain => self.picks[g] = (self.pick(g) + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up if plain => self.picks[g] = self.pick(g).saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home if plain => self.picks[g] = 0,
            KeyCode::Char('G') | KeyCode::End if plain => self.picks[g] = n.saturating_sub(1),
            KeyCode::Tab if k.shift => self.group = (g + GROUPS.len() - 1) % GROUPS.len(),
            KeyCode::Tab => self.group = (g + 1) % GROUPS.len(),
            KeyCode::Char(c @ '1'..='9') if plain => self.group = c as usize - '1' as usize,
            KeyCode::Char('l') | KeyCode::Right if plain => {
                if let Some(o) = self.picked() {
                    return self.step(o, 1);
                }
            }
            KeyCode::Char('h') | KeyCode::Left if plain => {
                if let Some(o) = self.picked() {
                    return self.step(o, -1);
                }
            }
            KeyCode::Char(' ') if plain => {
                if let Some(o) = self.picked().filter(|o| o.kind == Kind::Bool) {
                    return self.step(o, 1);
                }
            }
            KeyCode::Enter => {
                let Some(item) = self.picked_item() else { return Action::Redraw };
                let o = match item {
                    Item::Opt(o) => o,
                    Item::Bind(i) => {
                        self.edit = Some(Edit::Bind(i));
                        return Action::Redraw;
                    }
                };
                match o.kind {
                    Kind::Bool => return self.step(o, 1),
                    Kind::Key => self.edit = Some(Edit::Key),
                    Kind::Secret => {
                        self.edit =
                            Some(Edit::Line { name: Some(o.name.into()), text: String::new(), at: 0, secret: true })
                    }
                    _ => {
                        let text = self.row(o.name).value;
                        let at = text.chars().count();
                        self.preview = None;
                        self.edit = Some(Edit::Line { name: Some(o.name.into()), text, at, secret: false });
                    }
                }
            }
            KeyCode::Char('r') if plain => match self.picked_item() {
                Some(Item::Opt(o)) => return Action::Run(owned(&["set-option", "-gu", o.name])),
                // A key: back where the config left its command (swapping
                // with what is there).
                Some(Item::Bind(i)) => match self.binds[i].base.clone() {
                    Some(base) => return self.move_bind(i, &base, true),
                    None => {
                        self.note = Some(("bound since the config ran: no key of its own to go back to".into(), false))
                    }
                },
                None => {}
            },
            KeyCode::Char('z') if plain => self.zoom = !self.zoom,
            KeyCode::Char('/') if plain => {
                let at = self.filter.chars().count();
                self.edit = Some(Edit::Line { name: None, text: self.filter.clone(), at, secret: false });
            }
            KeyCode::Char('m') if plain => self.changed_only = !self.changed_only,
            KeyCode::Char('w') if plain => {
                if self.not_written() == 0 {
                    self.note = Some(("nothing to write: no change since the last".into(), false));
                } else if self.file.is_none() {
                    self.note = Some(("no settings file: the config was named; set settings-file in it".into(), true));
                } else {
                    return Action::Run(self.save());
                }
            }
            KeyCode::Char('R') if plain => self.ask = Some(Ask::Reload),
            KeyCode::Escape if self.zoom => self.zoom = false,
            KeyCode::Escape if !self.filter.is_empty() || self.changed_only => {
                self.filter.clear();
                self.changed_only = false;
            }
            KeyCode::Char('q') | KeyCode::Escape if plain => {
                if self.not_written() == 0 {
                    return Action::Quit;
                }
                self.ask = Some(Ask::Quit);
            }
            _ => {}
        }
        Action::Redraw
    }

    fn edit_key(&mut self, e: Edit, k: Key) -> Action {
        match e {
            Edit::Key => {
                if k == Key::plain(KeyCode::Escape) {
                    return Action::Redraw;
                }
                Board::set("prefix", &k.to_string())
            }
            Edit::Bind(i) => {
                if k == Key::plain(KeyCode::Escape) {
                    return Action::Redraw;
                }
                self.move_bind(i, &k.to_string(), false)
            }
            Edit::Line { name, text, mut at, secret } => {
                let mut chars: Vec<char> = text.chars().collect();
                at = at.min(chars.len());
                match k.code {
                    KeyCode::Escape => {
                        if name.is_none() {
                            self.filter.clear();
                        }
                        return Action::Redraw;
                    }
                    KeyCode::Enter => {
                        return match name {
                            Some(n) => Board::set(&n, &text),
                            None => Action::Redraw,
                        };
                    }
                    KeyCode::Left => at = at.saturating_sub(1),
                    KeyCode::Right => at = (at + 1).min(chars.len()),
                    KeyCode::Home => at = 0,
                    KeyCode::End => at = chars.len(),
                    KeyCode::BSpace if at > 0 => {
                        chars.remove(at - 1);
                        at -= 1;
                    }
                    KeyCode::DC if at < chars.len() => {
                        chars.remove(at);
                    }
                    KeyCode::Char('u') if k.ctrl => {
                        chars.clear();
                        at = 0;
                    }
                    KeyCode::Char(c) if !k.ctrl && !k.alt && !c.is_control() => {
                        chars.insert(at, c);
                        at += 1;
                    }
                    _ => {}
                }
                let text: String = chars.into_iter().collect();
                if name.is_none() {
                    self.filter = text.clone();
                }
                self.edit = Some(Edit::Line { name, text, at, secret });
                Action::Redraw
            }
        }
    }

    fn ask_key(&mut self, a: Ask, k: Key) -> Action {
        match (a, k.code) {
            (Ask::Quit, KeyCode::Char('w')) => Action::RunThenQuit(self.save()),
            (Ask::Quit, KeyCode::Char('q')) => Action::Quit,
            (Ask::Reload, KeyCode::Char('y')) => Action::Run(owned(&["reload-settings"])),
            (Ask::Swap(i, to), KeyCode::Enter) => self.move_bind(i, &to, true),
            _ => Action::Redraw,
        }
    }

    /// What a command run for the board answered: said on the bottom line;
    /// a change made marks the option not written, a write unmarks it.
    pub fn done(&mut self, argv: &[String], code: i32, out: &str, err: &str) {
        let moving = std::mem::take(&mut self.moving);
        if code != 0 {
            self.note = Some((err.trim().lines().next().unwrap_or("failed").to_string(), true));
            return;
        }
        let said = out.trim().lines().next().unwrap_or("").to_string();
        match argv.first().map(String::as_str) {
            Some("rebind-key" | "bind-key") => {
                // Where it went: `rebind-key [-s] FROM TO`, `bind-key [-r] TO …`.
                let to = if argv[0] == "bind-key" { argv.iter().skip(1).find(|a| *a != "-r") } else { argv.last() };
                let what = moving.first().map(String::as_str).unwrap_or("it");
                self.note =
                    Some((format!("{what} now on {} {}", self.prefix(), to.map(String::as_str).unwrap_or("")), false));
                self.keys_unsaved.extend(moving);
            }
            Some("set-option") => {
                let name = argv.get(2).cloned().unwrap_or_default();
                let secret = optinfo::info(&name).is_some_and(|o| o.kind == Kind::Secret);
                self.note = Some((
                    match (argv.get(1).map(String::as_str), argv.get(3)) {
                        (Some("-gu"), _) => format!("{name}: back to the default"),
                        _ if secret => format!("{name}: set"),
                        (_, Some(v)) => format!("{name}: {}", inert(v)),
                        _ => name.clone(),
                    },
                    false,
                ));
                self.unsaved.insert(name);
            }
            Some("save-settings") => {
                for n in &argv[1..] {
                    self.unsaved.remove(n);
                }
                if argv.iter().any(|a| a == "-k") {
                    self.keys_unsaved.clear();
                }
                self.note = Some((said, false));
            }
            Some("reload-settings") => {
                self.unsaved.clear();
                self.keys_unsaved.clear();
                self.note = Some((said, false));
            }
            _ => {}
        }
    }

    /// Where each box shown stands. Zoomed, the one box fills the board.
    fn grid(&self) -> Vec<(usize, Rect)> {
        let (w, body) = (usize::from(self.cols), usize::from(self.height).saturating_sub(2));
        if self.zoom {
            return vec![(self.group, Rect { x: 0, y: 1, w, h: body })];
        }
        let across = if w >= THREE_AT {
            3
        } else if w >= TWO_AT {
            2
        } else {
            1
        };
        let lines = GROUPS.len().div_ceil(across);
        let shown = (body / BOX_LEAST).clamp(1, lines);
        let each = body / shown;
        let first = window_start(self.group / across, lines, shown);
        let mut out = Vec::new();
        for r in 0..shown {
            let y = 1 + r * each;
            let h = if r + 1 == shown { body - r * each } else { each };
            for c in 0..across {
                let g = (first + r) * across + c;
                if g >= GROUPS.len() {
                    break;
                }
                let x = c * (w / across);
                let bw = if c + 1 == across { w - x } else { w / across };
                out.push((g, Rect { x, y, w: bw, h }));
            }
        }
        out
    }

    /// The rows a box's list takes, and the first of them shown.
    fn list_rows(&self, g: usize, r: Rect) -> (usize, usize) {
        let rows = if self.zoom { r.inner_h().saturating_sub(1 + ZOOM_FOOT) } else { r.inner_h() };
        (rows, window_start(self.pick(g), self.items(g).len(), rows))
    }

    /// An option's value as its line shows it: on/off in colour, a secret
    /// as `(set)`, what ←/→ steps between its arrows when picked.
    fn value_text(&self, o: &OptInfo, row: &Row, arrows: bool) -> String {
        let v = inert(&row.value);
        match o.kind {
            Kind::Bool if v == "on" => paint("32", "on"),
            Kind::Bool => paint("2", "off"),
            Kind::Secret if v.is_empty() => paint("2", "(none)"),
            Kind::Secret => "(set)".into(),
            Kind::Choice | Kind::Number(_) | Kind::Colour if arrows => {
                format!("{} {v} {}", paint("2", "‹"), paint("2", "›"))
            }
            _ if v.is_empty() => paint("2", "(empty)"),
            _ => v,
        }
    }

    fn mark(&self, name: &str, row: &Row) -> String {
        if self.unsaved.contains(name) {
            paint("1;33", "●")
        } else if row.value != row.default {
            paint("2", "◦")
        } else {
            " ".into()
        }
    }

    /// A line's mark, name and value: an option's (its value as picked
    /// lines show it), or a key's (the prefix and the key; what it runs).
    fn cells(&self, item: Item, arrows: bool) -> (String, String, String) {
        match item {
            Item::Opt(o) => {
                let row = self.row(o.name);
                (self.mark(o.name, &row), o.name.to_string(), self.value_text(o, &row, arrows))
            }
            Item::Bind(i) => {
                let b = &self.binds[i];
                let mark = if self.keys_unsaved.contains(&b.command) {
                    paint("1;33", "●")
                } else if b.base.as_deref() != Some(b.key.as_str()) {
                    paint("2", "◦")
                } else {
                    " ".into()
                };
                (mark, self.bind_name(b), inert(&b.command))
            }
        }
    }

    /// Whether a line is one of the two keys a swap asked about.
    fn in_question(&self, item: Item) -> bool {
        match (&self.ask, item) {
            (Some(Ask::Swap(i, to)), Item::Bind(j)) => j == *i || self.binds.get(j).is_some_and(|b| &b.key == to),
            _ => false,
        }
    }

    /// A list line: reversed when picked in the box the keys are in, bold
    /// when picked in another.
    fn picked_line(line: &str, picked: bool, here: bool, width: usize) -> String {
        if picked && here {
            format!("\x1b[7m{}\x1b[0m", fit(&strip(line), width))
        } else if picked {
            format!("\x1b[1m{}\x1b[0m", fit(line, width))
        } else {
            fit(line, width)
        }
    }

    fn box_lines(&self, g: usize, r: Rect) -> Vec<String> {
        let items = self.items(g);
        let width = r.inner_w();
        if items.is_empty() {
            return vec![paint("2", if self.rows.is_empty() { " reading…" } else { " no match" })];
        }
        let (rows, top) = self.list_rows(g, r);
        let pick = self.pick(g);
        items
            .iter()
            .enumerate()
            .skip(top)
            .take(rows)
            .map(|(i, item)| {
                let picked = i == pick;
                let here = g == self.group;
                let (mark, name, val) = self.cells(*item, picked && here);
                let room = width.saturating_sub(2);
                let val_w = width_of(&val).min(room * 11 / 20);
                let name_w = room.saturating_sub(val_w + 1);
                let line =
                    format!("{mark}{} {}", fit(&cut(&name, name_w), name_w), fit_right(&cut(&val, val_w), val_w));
                // The two keys a swap asks about, in red.
                let line = if self.in_question(*item) { paint("31", &strip(&line)) } else { line };
                Board::picked_line(&line, picked, here, width)
            })
            .collect()
    }

    /// The zoomed box: a table (option, value, default, who set it), then
    /// what the picked one does and what it takes.
    fn zoom_lines(&self, g: usize, r: Rect) -> Vec<String> {
        let items = self.items(g);
        let inner = r.inner_w();
        let name_w = items.iter().map(|it| width_of(&self.cells(*it, false).1)).max().unwrap_or(8).clamp(8, 32) + 2;
        let (default_w, source_w) = if inner >= 70 { ((inner / 6).min(18), (inner / 5).min(28)) } else { (0, 0) };
        let value_w = inner.saturating_sub(name_w + default_w + source_w + 3);
        let cols = |a: &str, b: &str, c: &str, d: &str| {
            let mut s = format!("{}{}", fit(a, name_w), fit(b, value_w));
            if default_w > 0 {
                s.push_str(&format!(" {} {}", fit(c, default_w), fit(d, source_w)));
            }
            s
        };
        let mut out = vec![paint("2", &cols("  option", "value", "default", "set by"))];
        let (rows, top) = self.list_rows(g, r);
        let pick = self.pick(g);
        for (i, item) in items.iter().enumerate().skip(top).take(rows) {
            let picked = i == pick;
            let (mark, name, val) = self.cells(*item, picked);
            let val = cut(&val, value_w.saturating_sub(1));
            // A key's default is where the config left its command; who
            // set it: that, moved, or bound since.
            let (default, source) = match *item {
                Item::Opt(o) => {
                    let row = self.row(o.name);
                    (if o.kind == Kind::Secret { String::new() } else { inert(&row.default) }, row.source)
                }
                Item::Bind(j) => {
                    let b = &self.binds[j];
                    let source = match &b.base {
                        None => "bound since",
                        Some(base) if *base == b.key => "the config",
                        Some(_) => "moved",
                    };
                    (b.base.clone().unwrap_or_default(), source.to_string())
                }
            };
            let name = format!("{mark}{}", cut(&name, name_w - 2));
            let line = cols(&name, &val, &cut(&default, default_w.saturating_sub(1)), &cut(&source, source_w));
            let line = if self.in_question(*item) { paint("31", &strip(&line)) } else { line };
            out.push(Board::picked_line(&line, picked, true, inner));
        }
        while out.len() < r.inner_h().saturating_sub(ZOOM_FOOT) {
            out.push(String::new());
        }
        out.push(paint("2", &"─".repeat(inner)));
        if items.is_empty() {
            out.push(paint("2", " no match"));
            return out;
        }
        let (doc, takes) = match items[pick] {
            Item::Opt(o) => (format!(" {}", o.doc), self.takes(o)),
            Item::Bind(j) => {
                let b = &self.binds[j];
                let back = match &b.base {
                    Some(base) if *base != b.key => format!("; r puts it back on {base}"),
                    _ => String::new(),
                };
                (
                    format!(
                        " A key of the prefix table: {}, then this key, runs {}.",
                        self.prefix(),
                        inert(&b.command)
                    ),
                    format!("Enter, then the new key (one bound already: Enter swaps the two){back}"),
                )
            }
        };
        let (first, used) = clip(&doc, inner);
        out.push(first);
        let rest = doc.chars().skip(used).collect::<String>();
        out.push(if rest.is_empty() { String::new() } else { cut(&format!(" {}", rest.trim_start()), inner) });
        out.push(match self.typed_format(items[pick]) {
            Some(text) => self.preview_line(text, inner),
            None => paint("2", &cut(&format!(" {takes}"), inner)),
        });
        out
    }

    /// The text being typed for `item`, when it is a format option.
    fn typed_format(&self, item: Item) -> Option<&str> {
        let Item::Opt(o) = item else { return None };
        match &self.edit {
            Some(Edit::Line { name: Some(n), text, .. }) if n == o.name && o.kind == Kind::Format => Some(text),
            _ => None,
        }
    }

    /// ` = ` and what the format being typed comes to, in its colours, cut
    /// to `width`; ` = …` while the server has not said.
    fn preview_line(&self, text: &str, width: usize) -> String {
        let runs = match &self.preview {
            Some((t, runs)) if t == text => runs,
            _ => return paint("2", " = …"),
        };
        let mut out = paint("2", " = ");
        let mut room = width.saturating_sub(3);
        for (sgr, run) in runs {
            if room == 0 {
                break;
            }
            let piece = if width_of(run) > room { cut(run, room) } else { run.clone() };
            room = room.saturating_sub(width_of(&piece));
            out.push_str(&if sgr.is_empty() { piece } else { paint(sgr, &piece) });
        }
        out
    }

    /// What an option takes, in a line.
    fn takes(&self, o: &OptInfo) -> String {
        match o.kind {
            Kind::Bool => "on or off: Space or ←/→".into(),
            Kind::Choice => format!("{}  (←/→; Enter types another)", self.steps(o).join(" · ")),
            Kind::Number(step) => format!("a number: ←/→ by {step}, Enter types it"),
            Kind::Size => "bytes, as 64K, 200M, 2G: Enter types it".into(),
            Kind::Colour => "a colour: ←/→ through the 16 names, Enter types #rrggbb or colourN".into(),
            Kind::Style => "fg=…,bg=…: Enter types it".into(),
            Kind::Format => "a format (#{…} values, #[…] styles): Enter types it".into(),
            Kind::List => "several on one line: Enter types them".into(),
            Kind::Secret => "not shown: Enter types a new one".into(),
            Kind::Key => "Enter, then the new key".into(),
            Kind::Text => "Enter types it".into(),
        }
    }

    fn head(&self) -> String {
        let mut s = format!(" {}  ", paint("1", "keepane settings"));
        s.push_str(&match &self.file {
            Some(f) => paint("2", &format!("writes {f}")),
            None => paint("33", "no settings file (set settings-file)"),
        });
        if !self.filter.is_empty() {
            s.push_str(&format!("  {} {}", paint("1;36", "/"), inert(&self.filter)));
        }
        if self.changed_only {
            s.push_str(&format!("  {}", paint("1;36", "changed only")));
        }
        if self.not_written() > 0 {
            let n = self.not_written();
            let right = format!("{} {n} not written  {}", paint("1;33", "●"), key_hints("w write"));
            let w = usize::from(self.cols);
            let room = w.saturating_sub(width_of(&right) + 1);
            return format!("{}{right} ", fit(&s, room));
        }
        s
    }

    fn foot(&self) -> String {
        let w = usize::from(self.cols);
        match (&self.edit, &self.ask) {
            (Some(Edit::Key), _) => {
                format!(" {}  {}", paint("1;33", "press the new prefix key"), key_hints("Esc cancel"))
            }
            (Some(Edit::Bind(i)), _) => {
                let what = self.binds.get(*i).map(|b| inert(&b.command)).unwrap_or_default();
                format!(" {} {what}  {}", paint("1;33", "press the new key for"), key_hints("Esc cancel"))
            }
            (None, Some(Ask::Swap(i, to))) => {
                let what = self.binds.get(*i).map(|b| inert(&b.command)).unwrap_or_default();
                let there = self.binds.iter().find(|b| &b.key == to).map(|b| inert(&b.command)).unwrap_or_default();
                format!(
                    " {} is bound to {there}: {}",
                    paint("31", &format!("{} {to}", self.prefix())),
                    key_hints(&format!("Enter swap with {what}  Esc cancel"))
                )
            }
            (Some(Edit::Line { name, text, at, secret }), _) => {
                let label = match name {
                    Some(n) => format!("{n}: "),
                    None => "/".into(),
                };
                let chars: Vec<char> =
                    if *secret { text.chars().map(|_| '•').collect() } else { inert(text).chars().collect() };
                let hint = key_hints(if name.is_some() { "Enter set  Esc cancel" } else { "Enter keep  Esc clear" });
                let room = w.saturating_sub(width_of(&label) + width_of(&hint) + 4).max(8);
                // The cursor kept in view: the text from where it fits.
                let start = at.saturating_sub(room.saturating_sub(1));
                let before: String = chars[start..*at].iter().collect();
                let under = chars.get(*at).copied().unwrap_or(' ');
                let after: String = chars.iter().skip(at + 1).collect();
                let after = clip(&after, room.saturating_sub(width_of(&before) + 1)).0;
                format!(" {}{before}\x1b[7m{under}\x1b[0m{after}  {hint}", paint("1", &label))
            }
            (None, Some(Ask::Quit)) => format!(
                " {} {}  {}",
                paint("1;33", &crate::format::count(self.not_written(), "change")),
                "not written:",
                key_hints("w write and quit  q quit, they last until the server restarts  Esc stay")
            ),
            (None, Some(Ask::Reload)) => format!(
                " {} every option and prefix key back to its default, then the config and settings files run; {} not written and {} set by hand are lost  {}",
                paint("1;33", "reload:"),
                self.not_written(),
                self.set_by_hand(),
                key_hints("y reload  Esc stay")
            ),
            (None, None) => match &self.note {
                Some((n, true)) => format!(" {}", paint("31", &inert(n))),
                Some((n, false)) => format!(" {}", inert(n)),
                None => format!(
                    " {}",
                    key_hints(if self.zoom {
                        "j/k option  ←/→ change  Enter edit  r default  z/Esc back  1-9 box  / filter  m changed  w write  R reload  q quit"
                    } else {
                        "j/k option  Tab/1-9 box  ←/→ change  Enter edit  r default  z zoom  / filter  m changed  w write  R reload  q quit"
                    })
                ),
            },
        }
    }

    /// Everything on screen, each row put at its place, exactly the
    /// window's width.
    pub fn frame(&self) -> String {
        let mut s = String::from("\x1b[?25l");
        for (y, row) in self.lines().iter().enumerate() {
            s.push_str(&format!("\x1b[{};1H", y + 1));
            s.push_str(row);
        }
        s
    }

    /// The screen's rows, top to bottom, each the window's width.
    fn lines(&self) -> Vec<String> {
        let (w, h) = (usize::from(self.cols), usize::from(self.height));
        let mut rows: Vec<String> = vec![String::new(); h];
        if let Some(r) = rows.first_mut() {
            *r = self.head();
        }
        for (g, r) in self.grid() {
            let lines = if self.zoom { self.zoom_lines(g, r) } else { self.box_lines(g, r) };
            let count = self.items(g).len();
            // Its options and, in the Keys box, its keys: what it lists unfiltered.
            let all = OPTIONS.iter().filter(|o| o.group == g && self.rows.contains_key(o.name)).count()
                + if g == KEYS_GROUP { self.binds.len() } else { 0 };
            let right = match (self.zoom, count == all) {
                (true, _) => "z back".to_string(),
                (false, true) => count.to_string(),
                (false, false) => format!("{count}/{all}"),
            };
            let title = format!("[{}] {}", g + 1, GROUPS[g]);
            for (i, l) in boxed(&title, &right, lines, r, g == self.group).into_iter().enumerate() {
                if let Some(row) = rows.get_mut(r.y + i) {
                    row.push_str(&l);
                }
            }
        }
        if let Some(last) = rows.last_mut() {
            *last = self.foot();
        }
        rows.iter().map(|r| fit(r, w)).collect()
    }

    /// A click picks the option under it (its box with it); a click on a
    /// box's top line zooms it, or back; the wheel moves in the box under it.
    pub fn mouse(&mut self, m: &MouseRecord) -> Action {
        const MOVED: u32 = 0x1;
        const WHEELED: u32 = 0x4;
        let (x, y) = (m.x.max(0) as usize, m.y.max(0) as usize);
        let Some((g, r)) = self.grid().into_iter().find(|(_, r)| r.holds(x, y)) else { return Action::Redraw };
        if m.flags & WHEELED != 0 {
            let up = (m.buttons >> 16) as i16 > 0;
            self.group = g;
            let n = self.items(g).len();
            self.picks[g] =
                if up { self.pick(g).saturating_sub(1) } else { (self.pick(g) + 1).min(n.saturating_sub(1)) };
            return Action::Redraw;
        }
        if m.flags & MOVED != 0 || m.buttons & 1 == 0 {
            return Action::Redraw;
        }
        self.edit = None;
        self.ask = None;
        if y == r.y {
            self.group = g;
            self.zoom = !self.zoom;
            return Action::Redraw;
        }
        self.group = g;
        let (rows, top) = self.list_rows(g, r);
        let first = r.y + 1 + usize::from(self.zoom);
        if y >= first && y < first + rows {
            let i = top + (y - first);
            if i < self.items(g).len() {
                self.picks[g] = i;
            }
        }
        Action::Redraw
    }
}

/// `keepane settings [--popup]`: the board in this terminal (prefix g opens
/// it in a popup).
pub fn run(socket: &str, rt: &tokio::runtime::Runtime) -> anyhow::Result<i32> {
    use crate::console::{Console, InputEvent};
    let mut console = Console::open()?;
    console.enter_raw()?;
    console.set_mouse(true);
    let console = std::sync::Arc::new(console);
    let (tx, rx) = std::sync::mpsc::channel::<InputEvent>();
    {
        let c = console.clone();
        std::thread::spawn(move || {
            while let Ok(events) = c.read_events() {
                for e in events {
                    if tx.send(e).is_err() {
                        return;
                    }
                }
            }
        });
    }
    let ask = |argv: &[String]| -> (i32, String, String) {
        debug_assert!(argv.first().is_some_and(|c| QUERIES.contains(&c.as_str()) || ACTIONS.contains(&c.as_str())));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        rt.block_on(crate::client::query(socket, &argv)).unwrap_or_else(|e| (1, String::new(), format!("{e:#}")))
    };
    let (cols, rows) = console.size();
    let mut board = Board::new(cols, rows);
    let result = (|| -> anyhow::Result<()> {
        loop {
            let (code, out, err) = ask(&owned(&["show-settings", "-J"]));
            if code != 0 {
                anyhow::bail!("{}", err.trim());
            }
            board.set_settings(&out);
            // The prefix table's keys (an older server has none to give).
            board.set_bindings(&ask(&owned(&["show-bindings", "-J"])).1);
            let (cols, rows) = console.size();
            if (cols, rows) != (board.cols, board.height) {
                board.resize(cols, rows);
                console.write_str("\x1b[2J");
            }
            // A format being typed: what it comes to, before it is drawn.
            if let Some(text) = board.wants_preview() {
                let (code, out, err) = ask(&owned(&["show-format", "-J", &text]));
                board.set_preview(&text, if code == 0 { &out } else { &err });
            }
            console.write_str(&board.frame());
            let first = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(e) => Some(e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return Ok(()),
            };
            let events = first.into_iter().chain(rx.try_iter()).flat_map(|ev| match ev {
                InputEvent::Key(k) => crate::keys::each_press(k).map(InputEvent::Key).collect::<Vec<_>>(),
                other => vec![other],
            });
            for ev in events {
                let action = match ev {
                    InputEvent::Key(k) => match crate::keys::key_from_record(&k) {
                        Some(key) => board.key(key),
                        None => continue,
                    },
                    InputEvent::Mouse(m) => board.mouse(&m),
                    InputEvent::Resize => {
                        let (cols, rows) = console.size();
                        board.resize(cols, rows);
                        console.write_str("\x1b[2J");
                        Action::Redraw
                    }
                };
                match action {
                    Action::Quit => return Ok(()),
                    Action::Run(argv) => {
                        let (code, out, err) = ask(&argv);
                        board.done(&argv, code, &out, &err);
                    }
                    Action::RunThenQuit(argv) => {
                        let (code, out, err) = ask(&argv);
                        board.done(&argv, code, &out, &err);
                        if code == 0 {
                            return Ok(());
                        }
                    }
                    Action::Redraw => {}
                }
            }
        }
    })();
    console.restore();
    result.map(|()| 0)
}

#[cfg(test)]
mod tests;
