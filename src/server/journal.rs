//! The server's changes, in order (docs/design/link.md, decision 17): what
//! another machine needs to keep a copy of every session here and draw it
//! itself. Each change has a number (`seq`), one count for the whole server,
//! so a copy that has everything up to N asks for what came after N and
//! nothing else. Two kinds of change:
//!
//! - the shape: sessions, windows, their layouts, and each pane's place,
//!   size, title, directory and mode. Small; told whole whenever any of it
//!   changed.
//! - a pane's own: what its program printed (the bytes as they came, which
//!   a copy feeds to a terminal model of its own, so its screen is the same
//!   to the byte), its new size, its terminal model started again (`Reset`,
//!   with what the new one was fed). And, as labels on those bytes, what
//!   the bytes are: a command the shell ran (what was typed, where, how it
//!   ended), a full-screen program coming and going (the alternate screen).
//!
//! What is kept is bounded (`LIMIT`): a copy further behind than that starts
//! again from a snapshot (`Snapshot`), every pane's screen as it is now.

use std::collections::{HashSet, VecDeque};

use serde::Serialize;

use super::Server;
use super::layout::{self, PaneId};

/// The most the journal keeps, in bytes (what panes printed, and an
/// estimate for the rest): enough for a copy that lost the network for a
/// while to catch up without a snapshot, little next to what the panes'
/// scrollback holds.
pub const LIMIT: usize = 8 << 20;

/// What a pane keeps of its own changes between two looks of the server
/// (the server looks after every batch of events): only a pane nobody
/// looks at (a popup, one kept for undo) gets this far, and it then keeps
/// its screen instead (a `Reset`).
const PANE_LIMIT: usize = 1 << 20;

/// What a pane may keep before the server looks at once, in the middle of a
/// batch: a batch has no bound (panes printing flat out keep the queue
/// full), a read has (64 KiB), so a pane never gets to `PANE_LIMIT`.
pub const LOOK_AT: usize = PANE_LIMIT / 4;

/// A change to one pane, in the order it happened to the pane.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneChange {
    /// Its program printed `bytes`, which start at byte `at` of all it
    /// printed.
    Output {
        at: u64,
        #[serde(serialize_with = "as_b64")]
        bytes: Vec<u8>,
    },
    /// Its terminal took this size.
    Size { cols: u16, rows: u16 },
    /// Its terminal model was made again at this size and fed `bytes`: a
    /// new pane (fed nothing), or its history cleared (fed its screen, as
    /// `clear-history` does it here).
    Reset {
        cols: u16,
        rows: u16,
        #[serde(serialize_with = "as_b64")]
        bytes: Vec<u8>,
        /// Where its next output starts (stamped when it is kept:
        /// `PaneChanges::push`): 0 for a new pane.
        at: u64,
    },
    /// Its terminal model is now this one, whole (vt100's `Screen::snapshot`:
    /// the size, the scrollback, the alternate screen and the cursor saved
    /// are in it): what it printed while nobody looked was more than is
    /// kept, or it is back in a window (`undo-kill`) after changes a copy
    /// never had.
    Screen {
        #[serde(serialize_with = "as_b64")]
        snapshot: Vec<u8>,
        /// Where its next output starts (stamped when it is kept).
        at: u64,
    },
    /// A full-screen program took the screen (the alternate screen), or
    /// gave it back.
    Fullscreen { on: bool },
    /// The shell started `command`, typed in `cwd`; its output starts at
    /// byte `at`.
    CommandStart { command: String, cwd: Option<String>, at: u64 },
    /// It ended: its exit code when the shell gave one, how long it ran,
    /// and the byte its output ends at.
    CommandEnd { command: String, exit: Option<i32>, secs: u64, at: u64 },
}

impl PaneChange {
    /// Whether it is the pane's terminal model whole: nothing before it is
    /// needed.
    pub fn is_whole(&self) -> bool {
        matches!(self, PaneChange::Reset { .. } | PaneChange::Screen { .. })
    }

    /// About how much keeping it costs.
    fn cost(&self) -> usize {
        64 + match self {
            PaneChange::Output { bytes, .. } | PaneChange::Reset { bytes, .. } => bytes.len(),
            PaneChange::Screen { snapshot, .. } => snapshot.len(),
            PaneChange::CommandStart { command, cwd, .. } => command.len() + cwd.as_ref().map_or(0, String::len),
            PaneChange::CommandEnd { command, .. } => command.len(),
            PaneChange::Size { .. } | PaneChange::Fullscreen { .. } => 0,
        }
    }
}

fn as_b64<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crate::link::b64(bytes))
}

/// What a pane keeps of its changes until the server takes them.
#[derive(Default)]
pub struct PaneChanges {
    list: Vec<PaneChange>,
    cost: usize,
    /// Bytes printed so far: where the next output starts.
    pub at: u64,
}

impl PaneChanges {
    /// A new pane's: its terminal model, made at this size, fed nothing.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut c = Self::default();
        c.push(PaneChange::Reset { cols, rows, bytes: Vec::new(), at: 0 }, Vec::new);
        c
    }

    /// Keep `change`; past `PANE_LIMIT`, keep the screen instead (`screen`
    /// gives it: its terminal model, `Screen::snapshot`).
    pub fn push(&mut self, mut change: PaneChange, screen: impl FnOnce() -> Vec<u8>) {
        match &mut change {
            PaneChange::Output { bytes, .. } => self.at += bytes.len() as u64,
            // A whole model: what comes after it starts here.
            PaneChange::Reset { at, .. } | PaneChange::Screen { at, .. } => *at = self.at,
            _ => {}
        }
        // The model whole: nothing before it is needed. One bigger than the
        // limit (500 x 200 cells, each its own colour; a long scrollback)
        // is still kept: a whole one never makes another.
        let whole = change.is_whole();
        if whole {
            self.list.clear();
            self.cost = 0;
        }
        self.cost += change.cost();
        self.list.push(change);
        if self.cost > PANE_LIMIT && !whole {
            self.push(PaneChange::Screen { snapshot: screen(), at: 0 }, Vec::new);
        }
    }

    /// About what is kept, in bytes.
    pub fn cost(&self) -> usize {
        self.cost
    }

    /// Everything kept, oldest first; nothing is kept after.
    pub fn take(&mut self) -> Vec<PaneChange> {
        self.cost = 0;
        std::mem::take(&mut self.list)
    }

    /// Whether the oldest kept is a whole model (`Reset`, `Screen`): a copy
    /// needs nothing from before it.
    fn starts_whole(&self) -> bool {
        self.list.first().is_some_and(PaneChange::is_whole)
    }
}

/// Every session's shape: what a copy lays out, but not what the panes show.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Shape {
    pub sessions: Vec<SessionShape>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionShape {
    pub id: u32,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    /// The window its own clients are on (the index shown, `base-index`
    /// counted).
    pub current: usize,
    pub windows: Vec<WindowShape>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowShape {
    pub id: u32,
    pub index: usize,
    pub name: String,
    /// As tmux writes it (`#{window_layout}`).
    pub layout: String,
    pub active: PaneId,
    pub zoomed: bool,
    pub panes: Vec<PaneShape>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PaneShape {
    pub id: PaneId,
    /// Where it is drawn in its window, and its size.
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
    pub command: String,
    pub cwd: Option<String>,
    /// Its name (`name-pane`) and work mode.
    pub name: Option<String>,
    pub mode: &'static str,
    /// Its program ended with this code (`remain-on-exit` keeps it).
    pub exited: Option<u32>,
    /// Bumped when it is started again (`respawn-pane`).
    pub generation: u32,
}

impl Shape {
    fn panes(&self) -> impl Iterator<Item = &PaneShape> {
        self.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes)
    }

    fn counts(&self) -> (usize, usize, usize) {
        let windows = self.sessions.iter().map(|s| s.windows.len()).sum();
        (self.sessions.len(), windows, self.panes().count())
    }
}

/// One change, numbered.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub seq: u64,
    /// When, in Unix milliseconds.
    pub at: i64,
    #[serde(flatten)]
    pub change: Change,
}

/// As JSON: `{"shape": {..}}`, or `{"pane": 3, "output": {..}}` (the pane's
/// change named by its kind).
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum Change {
    Shape {
        shape: Shape,
    },
    Pane {
        pane: PaneId,
        #[serde(flatten)]
        change: PaneChange,
    },
}

impl Entry {
    pub fn cost(&self) -> usize {
        match &self.change {
            // About what it takes written out.
            Change::Shape { shape: s } => 128 + s.panes().count() * 256,
            Change::Pane { change, .. } => change.cost(),
        }
    }
}

/// The changes kept, numbered from 1.
pub struct Journal {
    entries: VecDeque<Entry>,
    cost: usize,
    /// The number the next change takes.
    next: u64,
    shape: Shape,
    /// This run of the server, at random: the numbers start again with
    /// the next one, so a number means something only with it.
    pub epoch: u64,
}

impl Default for Journal {
    fn default() -> Self {
        let mut e = [0u8; 8];
        let _ = crate::platform::random::fill(&mut e);
        Self { entries: VecDeque::new(), cost: 0, next: 1, shape: Shape::default(), epoch: u64::from_le_bytes(e) }
    }
}

/// What came after a number: the changes, or that they are gone (`gap`)
/// and a copy has to start again from a snapshot.
#[derive(Debug, Serialize)]
pub struct Since<'a> {
    /// The newest change's number (0: none yet).
    pub last: u64,
    pub gap: bool,
    pub entries: Vec<&'a Entry>,
}

/// Every pane's screen now, and the shape: where a copy starts. The
/// changes after `seq` go on from it.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub seq: u64,
    pub shape: Shape,
    pub panes: Vec<PaneScreen>,
}

#[derive(Debug, Serialize)]
pub struct PaneScreen {
    pub id: PaneId,
    pub cols: u16,
    pub rows: u16,
    pub fullscreen: bool,
    /// Its terminal model whole (vt100's `Screen::snapshot`): restored, a
    /// copy then takes the changes after `seq`.
    #[serde(serialize_with = "as_b64")]
    pub screen: Vec<u8>,
    /// Bytes it has printed: where its next output starts.
    pub at: u64,
}

impl Journal {
    /// The newest change's number (0: none yet).
    pub fn last(&self) -> u64 {
        self.next - 1
    }

    /// The oldest change still kept.
    pub fn oldest(&self) -> u64 {
        self.entries.front().map_or(self.next, |e| e.seq)
    }

    pub fn cost(&self) -> usize {
        self.cost
    }

    fn push(&mut self, change: Change, at: i64) {
        let e = Entry { seq: self.next, at, change };
        self.next += 1;
        self.cost += e.cost();
        self.entries.push_back(e);
        // The shape now is always known (`self.shape`), so even the last
        // one told may go: a copy that far behind takes a snapshot.
        while self.cost > LIMIT
            && let Some(old) = self.entries.pop_front()
        {
            self.cost -= old.cost();
        }
    }

    /// Take a new shape and the panes' changes since the last look: the
    /// shape first when it changed (a pane new in it is then known), then
    /// each pane's, in its order. Changes of panes no longer in it are gone
    /// with them.
    pub fn take(&mut self, shape: Shape, panes: Vec<(PaneId, Vec<PaneChange>)>, at: i64) {
        let now: HashSet<PaneId> = shape.panes().map(|p| p.id).collect();
        if shape != self.shape {
            self.shape = shape.clone();
            self.push(Change::Shape { shape }, at);
        }
        for (pane, changes) in panes {
            if !now.contains(&pane) {
                continue;
            }
            for change in changes {
                self.push(Change::Pane { pane, change }, at);
            }
        }
    }

    /// The panes new to `shape` (or started again) since the last look.
    pub fn new_panes(&self, shape: &Shape) -> Vec<PaneId> {
        let before: HashSet<(PaneId, u32)> = self.shape.panes().map(|p| (p.id, p.generation)).collect();
        shape.panes().filter(|p| !before.contains(&(p.id, p.generation))).map(|p| p.id).collect()
    }

    /// The changes after `seq`, those of pane `pane` only when given (and
    /// the shapes, which every pane's place is in).
    pub fn since(&self, seq: u64, pane: Option<PaneId>) -> Since<'_> {
        let gap = seq.saturating_add(1) < self.oldest() && seq < self.last();
        let entries = self
            .entries
            .iter()
            .filter(|e| e.seq > seq)
            .filter(|e| match (&e.change, pane) {
                (Change::Pane { pane: p, .. }, Some(want)) => *p == want,
                _ => true,
            })
            .collect();
        Since { last: self.last(), gap, entries }
    }

    /// The shape as last taken.
    pub fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// What `bytes` print, roughly, for a person reading the list: escape
/// sequences left out, a line break as `⏎`, at most `max` characters.
pub fn preview(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    let mut n = 0;
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters up to a final byte in @..~.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC, DCS, APC, PM, SOS: up to BEL or ST.
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                // A charset (ESC ( B) takes one more.
                Some('(' | ')' | '*' | '+') => {
                    chars.next();
                }
                _ => {}
            },
            '\n' => {
                if !out.ends_with('⏎') {
                    out.push('⏎');
                    n += 1;
                }
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                n += 1;
            }
        }
        if n >= max {
            out.push('…');
            break;
        }
    }
    out
}

impl Server {
    /// Every session's shape now: this machine's own (not those of other
    /// machines copied here, `mirrored`, which are theirs to tell).
    pub(super) fn journal_shape(&self) -> Shape {
        let base = self.opts.base_index;
        Shape {
            sessions: self
                .sessions
                .iter()
                .filter(|s| s.remote.is_none())
                .map(|s| SessionShape {
                    id: s.id,
                    name: s.name.clone(),
                    cols: s.cols,
                    rows: s.rows,
                    current: s.cur + base,
                    windows: s
                        .windows
                        .iter()
                        .enumerate()
                        .map(|(i, w)| WindowShape {
                            id: w.id,
                            index: i + base,
                            name: w.name.clone(),
                            layout: layout::layout_string(&w.layout, &w.rects),
                            active: w.active,
                            zoomed: w.zoomed,
                            panes: w
                                .layout
                                .panes()
                                .into_iter()
                                .filter_map(|id| {
                                    let p = w.panes.iter().find(|p| p.id == id)?;
                                    let r = w.rects.iter().find(|(pid, _)| *pid == id).map(|(_, r)| *r);
                                    // A pane zoomed out of sight has no place.
                                    let r = r.unwrap_or(layout::Rect { x: 0, y: 0, w: 0, h: 0 });
                                    Some(PaneShape {
                                        id,
                                        x: r.x,
                                        y: r.y,
                                        cols: p.cols,
                                        rows: p.rows,
                                        title: p.title.clone(),
                                        command: p.command.clone(),
                                        cwd: p.cwd.clone(),
                                        name: p.actor.name.clone(),
                                        mode: p.actor.mode.as_str(),
                                        exited: p.exit_code,
                                        generation: p.generation,
                                    })
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Take what changed since the last look into the journal: run after
    /// every batch of events, so nothing changes between two looks without
    /// the second seeing it.
    pub(super) fn journal_look(&mut self) {
        let shape = self.journal_shape();
        let fresh: HashSet<PaneId> = self.journal.new_panes(&shape).into_iter().collect();
        let mut panes = Vec::new();
        for s in self.sessions.iter_mut().filter(|s| s.remote.is_none()) {
            for w in &mut s.windows {
                for p in &mut w.panes {
                    if fresh.contains(&p.id) && !p.changes.starts_whole() {
                        // Back in a window with changes a copy never had
                        // the start of: its screen as it is now.
                        p.changes.take();
                        p.changes.push(PaneChange::Screen { snapshot: p.screen().snapshot(), at: 0 }, Vec::new);
                    }
                    let changes = p.changes.take();
                    if !changes.is_empty() {
                        panes.push((p.id, changes));
                    }
                }
            }
        }
        // Panes out of every window (popups, ones kept for undo) keep theirs
        // until they come back, within `PANE_LIMIT`.
        if panes.is_empty() && &shape == self.journal.shape() {
            return;
        }
        self.journal.take(shape, panes, chrono::Utc::now().timestamp_millis());
    }

    /// Every pane's screen now, and the shape: where a copy starts.
    pub(super) fn journal_snapshot(&mut self) -> Snapshot {
        // What changed since the last look first: the snapshot is of now,
        // and the changes after its number go on from it.
        self.journal_look();
        let shape = self.journal.shape().clone();
        let mut panes = Vec::new();
        for s in self.sessions.iter().filter(|s| s.remote.is_none()) {
            for w in &s.windows {
                for p in &w.panes {
                    let screen = p.screen();
                    panes.push(PaneScreen {
                        id: p.id,
                        cols: p.cols,
                        rows: p.rows,
                        fullscreen: screen.alternate_screen(),
                        screen: screen.snapshot(),
                        at: p.changes.at,
                    });
                }
            }
        }
        Snapshot { seq: self.journal.last(), shape, panes }
    }

    /// `show-changes`: the changes after `since`, as a list to read or as
    /// JSON; or (`snapshot`) where a copy starts, and what that costs.
    pub(super) fn show_changes(
        &mut self,
        since: Option<u64>,
        pane: Option<PaneId>,
        json: bool,
        snapshot: bool,
    ) -> String {
        if snapshot {
            let snap = self.journal_snapshot();
            if json {
                return serde_json::to_string(&snap).unwrap_or_default();
            }
            let shape = serde_json::to_string(&snap.shape).map_or(0, |s| s.len());
            let screens: usize = snap.panes.iter().map(|p| p.screen.len()).sum();
            let (s, w, n) = snap.shape.counts();
            let mut out = format!(
                "snapshot at #{}: {s} sessions, {w} windows, {n} panes; shape {} bytes, screens {} bytes\n",
                snap.seq, shape, screens
            );
            for p in &snap.panes {
                let full = if p.fullscreen { ", full screen" } else { "" };
                out.push_str(&format!("  %{} {}x{}{full}: {} bytes\n", p.id, p.cols, p.rows, p.screen.len()));
            }
            return out;
        }
        self.journal_look();
        let since_seq = since.unwrap_or(0);
        let got = self.journal.since(since_seq, pane);
        if json {
            return serde_json::to_string(&got).unwrap_or_default();
        }
        let kept = match (self.journal.oldest(), got.last) {
            (_, 0) => "no changes yet".to_string(),
            (oldest, last) if oldest > last => format!("none kept, the newest #{last}"),
            (oldest, last) => format!("changes #{oldest}..#{last} kept"),
        };
        let mut out = format!("{kept} ({} of {} KiB)\n", self.journal.cost() / 1024, LIMIT / 1024);
        if got.gap {
            out.push_str(&format!(
                "#{}..#{} are no longer kept: take a snapshot (-S)\n",
                since_seq + 1,
                self.journal.oldest() - 1
            ));
        }
        list_changes(&got.entries, &mut out);
        out
    }
}

/// The entries one to a line; outputs of a pane in a row as one.
fn list_changes(entries: &[&Entry], out: &mut String) {
    let mut i = 0;
    while i < entries.len() {
        let e = entries[i];
        let time = chrono::DateTime::from_timestamp_millis(e.at)
            .map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S%.3f").to_string())
            .unwrap_or_default();
        match &e.change {
            Change::Shape { shape: s } => {
                let (n_s, n_w, n_p) = s.counts();
                out.push_str(&format!("#{} {time} shape: {n_s} sessions, {n_w} windows, {n_p} panes\n", e.seq));
                i += 1;
            }
            Change::Pane { pane, change: PaneChange::Output { .. } } => {
                // This pane's outputs in a row.
                let run: Vec<&[u8]> = entries[i..]
                    .iter()
                    .map_while(|e| match &e.change {
                        Change::Pane { pane: p, change: PaneChange::Output { bytes, .. } } if p == pane => {
                            Some(bytes.as_slice())
                        }
                        _ => None,
                    })
                    .collect();
                let last = entries[i + run.len() - 1].seq;
                let total: usize = run.iter().map(|b| b.len()).sum();
                let all: Vec<u8> = run.concat();
                let seqs = if run.len() == 1 { format!("#{}", e.seq) } else { format!("#{}..#{last}", e.seq) };
                out.push_str(&format!("{seqs} {time} %{pane} output {total} bytes: {}\n", preview(&all, 60)));
                i += run.len();
            }
            Change::Pane { pane, change } => {
                let what = match change {
                    PaneChange::Output { .. } => unreachable!(),
                    PaneChange::Size { cols, rows } => format!("size {cols}x{rows}"),
                    PaneChange::Reset { cols, rows, bytes, .. } => {
                        format!("reset {cols}x{rows}, {} bytes", bytes.len())
                    }
                    PaneChange::Screen { snapshot, .. } => format!("screen, {} bytes", snapshot.len()),
                    PaneChange::Fullscreen { on: true } => "full screen".into(),
                    PaneChange::Fullscreen { on: false } => "full screen ends".into(),
                    PaneChange::CommandStart { command, cwd, .. } => match cwd {
                        Some(d) => format!("command starts: {command}  (in {d})"),
                        None => format!("command starts: {command}"),
                    },
                    PaneChange::CommandEnd { command, exit, secs, .. } => {
                        let how = exit.map_or("ended".into(), |c| format!("exit {c}"));
                        format!("command ends: {command}  ({how}, {secs} s)")
                    }
                };
                out.push_str(&format!("#{} {time} %{pane} {what}\n", e.seq));
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(at: u64, b: &str) -> PaneChange {
        PaneChange::Output { at, bytes: b.as_bytes().to_vec() }
    }

    fn shape_of(panes: &[(PaneId, u32)]) -> Shape {
        Shape {
            sessions: vec![SessionShape {
                id: 0,
                name: "s".into(),
                cols: 80,
                rows: 24,
                current: 0,
                windows: vec![WindowShape {
                    id: 0,
                    index: 0,
                    name: "w".into(),
                    layout: String::new(),
                    active: panes[0].0,
                    zoomed: false,
                    panes: panes
                        .iter()
                        .map(|&(id, generation)| PaneShape {
                            id,
                            x: 0,
                            y: 0,
                            cols: 80,
                            rows: 24,
                            title: String::new(),
                            command: "sh".into(),
                            cwd: None,
                            name: None,
                            mode: "normal",
                            exited: None,
                            generation,
                        })
                        .collect(),
                }],
            }],
        }
    }

    #[test]
    fn changes_are_numbered_once_for_the_server_and_asked_for_after_a_number() {
        let mut j = Journal::default();
        assert_eq!((j.last(), j.oldest()), (0, 1));
        j.take(shape_of(&[(1, 0)]), vec![(1, vec![out(0, "a"), out(1, "b")])], 0);
        assert_eq!(j.last(), 3, "the shape, then the pane's two");
        // The same shape again is not a change.
        j.take(shape_of(&[(1, 0)]), vec![(1, vec![out(2, "c")])], 0);
        assert_eq!(j.last(), 4);
        let got = j.since(2, None);
        assert!(!got.gap);
        assert_eq!(got.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 4]);
        assert!(j.since(4, None).entries.is_empty(), "nothing after the newest");
        // A pane gone from the shape takes its changes with it.
        j.take(shape_of(&[(2, 0)]), vec![(1, vec![out(3, "lost")]), (2, vec![out(0, "x")])], 0);
        let got = j.since(4, None);
        assert_eq!(got.entries.len(), 2, "{got:?}");
        assert!(matches!(got.entries[1].change, Change::Pane { pane: 2, .. }));
        // One pane's only, with the shapes.
        assert_eq!(j.since(0, Some(2)).entries.len(), 3);
    }

    #[test]
    fn what_is_kept_is_bounded_and_a_copy_too_far_behind_is_told_to_start_again() {
        let mut j = Journal::default();
        let big = "x".repeat(1 << 20);
        for i in 0..12u64 {
            j.take(shape_of(&[(1, 0)]), vec![(1, vec![out(i << 20, &big)])], 0);
        }
        assert!(j.cost() <= LIMIT, "{}", j.cost());
        assert!(j.oldest() > 1);
        assert!(j.since(0, None).gap, "the first ones are gone");
        assert!(!j.since(j.oldest() - 1, None).gap, "from just before the oldest kept, nothing is missing");
        assert!(!j.since(j.last(), None).gap);
        assert!(!j.since(u64::MAX, None).gap && j.since(u64::MAX, None).entries.is_empty(), "past the newest");
    }

    #[test]
    fn a_pane_keeps_its_changes_in_order_and_its_screen_once_they_are_too_many() {
        let mut c = PaneChanges::new(80, 24);
        c.push(out(0, "hello"), Vec::new);
        c.push(PaneChange::Size { cols: 100, rows: 30 }, Vec::new);
        assert_eq!(c.at, 5);
        let got = c.take();
        assert_eq!(got.len(), 3);
        assert!(matches!(got[0], PaneChange::Reset { cols: 80, rows: 24, .. }));
        assert!(c.take().is_empty(), "taken once");
        let big = vec![b'y'; PANE_LIMIT];
        c.push(PaneChange::Output { at: 5, bytes: big.clone() }, || b"SCREEN".to_vec());
        let got = c.take();
        assert_eq!(
            got,
            [PaneChange::Screen { snapshot: b"SCREEN".to_vec(), at: 5 + PANE_LIMIT as u64 }],
            "stamped where what comes next starts"
        );
        assert_eq!(c.at, 5 + big.len() as u64, "the count goes on: bytes after start there");
        // A screen bigger than the limit itself: kept, once.
        let huge = vec![b'z'; PANE_LIMIT * 2];
        c.push(PaneChange::Output { at: 0, bytes: big }, || huge.clone());
        let got = c.take();
        assert_eq!(got.len(), 1);
        assert!(matches!(&got[0], PaneChange::Screen { snapshot, .. } if snapshot.len() == PANE_LIMIT * 2));
    }

    #[test]
    fn a_pane_new_to_the_shape_or_started_again_is_seen() {
        let mut j = Journal::default();
        j.take(shape_of(&[(1, 0)]), vec![], 0);
        assert_eq!(j.new_panes(&shape_of(&[(1, 0), (2, 0)])), [2]);
        assert_eq!(j.new_panes(&shape_of(&[(1, 1)])), [1], "respawned");
        assert!(j.new_panes(&shape_of(&[(1, 0)])).is_empty());
    }

    /// A screen's state as vt100 keeps it, the scrollback, the alternate
    /// screen, the saved cursor and the scroll region included: restored
    /// from a snapshot taken anywhere, a copy fed the rest of the same
    /// bytes ends the same as the original, byte for byte.
    #[test]
    fn a_copy_restored_from_a_screen_snapshot_stays_the_same() {
        let stream = [
            "plain line\r\n\x1b[1;31mred bold\x1b[0m and \x1b[38;2;10;200;30mrgb\x1b[48;5;17m bg\x1b[0m\r\n",
            "中文宽字符在这里折行中文宽字符在这里折行中文宽字符\r\n",
            &"x".repeat(97),
            "\r\n\x1b[?2004h\x1b[?1h\x1b=\x1b[?1006h\x1b[?1002h",
            &(0..60).map(|i| format!("history {i}\r\n")).collect::<String>(),
            "\x1b[3;8r\x1b[?6h\x1b[2;3Hin region\x1b7\x1b[?6l\x1b[5;5H\x1b[44m",
            "\x1b[?1049h\x1b[2J\x1b[Hfull screen \x1b[7minverse\x1b[0m\x1b[10;1Hrow ten",
            "\x1b[2;10r\x1b[?25l\x1b[20;40Hend at the edge",
            &"y".repeat(40),
            "\x1b[?1049l\x1b8back\r\nafter",
        ]
        .concat();
        let bytes = stream.as_bytes();
        let fed = |b: &[u8]| {
            let mut p = vt100::Parser::new(24, 80, 50);
            p.process(b);
            p
        };
        let whole = fed(bytes).screen().snapshot();
        // Every point between two sequences: where a read can end.
        let points: Vec<usize> =
            (0..=bytes.len()).filter(|&k| k == bytes.len() || (bytes[k] == 0x1b || bytes[k] == b'\r')).collect();
        assert!(points.len() > 40, "{}", points.len());
        for &k in &points {
            let original = fed(&bytes[..k]);
            let snap = original.screen().snapshot();
            let mut copy = vt100::Parser::new(5, 5, 0);
            assert!(copy.screen_mut().restore(&snap), "at {k}");
            assert_eq!(copy.screen().snapshot(), snap, "the same, read back, at {k}");
            copy.process(&bytes[k..]);
            assert_eq!(copy.screen().snapshot(), whole, "fed the rest from {k}");
        }
        // The size, the scrollback limit and what scrolled come with it.
        let mut copy = vt100::Parser::new(5, 5, 0);
        assert!(copy.screen_mut().restore(&whole));
        assert_eq!(copy.screen().size(), (24, 80));
        assert_eq!(copy.screen().scrollback_rows(), fed(bytes).screen().scrollback_rows());
        assert_eq!(copy.screen().scrolled_total(), fed(bytes).screen().scrolled_total());
        // Bytes it did not write change nothing.
        let before = copy.screen().snapshot();
        for bad in [&b""[..], b"KPS", b"KPS\x02", &whole[..whole.len() - 1], &[&whole[..], b"x"].concat()] {
            assert!(!copy.screen_mut().restore(bad));
        }
        let mut flipped = whole.clone();
        for i in (4..flipped.len()).step_by(97) {
            flipped[i] ^= 0xff;
            let _ = copy.screen_mut().restore(&flipped);
            flipped[i] ^= 0xff;
        }
        assert!(copy.screen_mut().restore(&before), "still usable after damaged bytes");
    }
    #[test]
    fn the_preview_leaves_escapes_out() {
        assert_eq!(preview(b"\x1b[1;32mok\x1b[0m\r\nnext\x1b]0;title\x07!", 60), "ok⏎next!");
        assert_eq!(preview(b"\x1b]133;A\x1b\\$ \x1b(Bls", 60), "$ ls");
        assert_eq!(preview("你好世界".as_bytes(), 2), "你好…");
    }
}
