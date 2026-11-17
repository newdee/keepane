//! What an agent's screen and title say that its hooks and transcript do
//! not (docs/design/agent-accuracy.md, step 2): Codex and Cursor waiting
//! for the person (an approval, a question), and Cursor at work. Rules as
//! herdr writes them (herdrdev/herdr `src/detect/manifests`): a title that
//! says so, or words in the screen's last lines.
//!
//! Only what nothing else says: a wait the screen shows sets the pane's
//! wait as `pane-wait` does, and clears when the screen stops showing it;
//! Cursor, whose transcripts are not read, is busy while it shows "ctrl+c
//! to stop" and free a moment after. Codex's busy and free stay its
//! transcript's.

use super::agents::Kind;
use super::{PaneId, Server};

/// The screen's last lines with something on them that the rules read.
const BOTTOM: usize = 12;
/// Cursor's "at work" line is in its last few.
const BUSY_BOTTOM: usize = 6;
/// Looks (a second apart) without the sign before a wait is over, or
/// Cursor free: one look can fall between two frames of a redraw.
const MISSES: u8 = 2;
/// A wait set this recently (its hook: Codex's `PermissionRequest`) is the
/// one the screen shows: not told again.
const TOLD_MS: i64 = 30_000;

/// What a screen says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The agent waits for the person: what it asks, as the screen says it.
    Waiting(String),
    /// The agent is at work.
    Busy,
    Nothing,
}

/// The words that mean the agent asks the person, in its last lines,
/// lower case.
fn asks(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Codex => &["press enter to confirm or esc to cancel", "enter to submit answer", "allow command?"],
        Kind::Cursor => &["waiting for approval", "run this command?"],
        _ => &[],
    }
}

/// What `kind`'s screen says: its title and its rows, top to bottom.
pub fn look(kind: Kind, title: &str, rows: &[String]) -> Seen {
    let last: Vec<&str> = rows.iter().map(|r| r.trim()).filter(|r| !r.is_empty()).collect();
    let bottom = |n: usize| &last[last.len().saturating_sub(n)..];
    if kind == Kind::Codex && title.contains("Action Required") {
        return Seen::Waiting("approve in Codex".into());
    }
    let words = asks(kind);
    if let Some(line) = bottom(BOTTOM).iter().find(|l| {
        let l = l.to_lowercase();
        words.iter().any(|w| l.contains(w))
    }) {
        return Seen::Waiting(line.chars().take(200).collect());
    }
    if kind == Kind::Cursor && bottom(BUSY_BOTTOM).iter().any(|l| l.to_lowercase().contains("ctrl+c to stop")) {
        return Seen::Busy;
    }
    Seen::Nothing
}

/// What the screen rules keep of a pane between looks.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    /// The wait the screen set (cleared only by the screen).
    pub waiting: bool,
    wait_misses: u8,
    /// Cursor was seen at work since it was last free.
    busy: bool,
    busy_misses: u8,
    /// When it was last seen at work: the screen ends that turn only, not
    /// one begun since (a message delivered or keys typed after it: both
    /// set the pane's `last_input`).
    busy_at: Option<chrono::DateTime<chrono::Local>>,
}

impl Server {
    /// Each second: the screens of the panes running Codex or Cursor.
    pub(super) fn screen_tick(&mut self) {
        let panes: Vec<(PaneId, Kind)> = self
            .agents
            .stats
            .iter()
            .filter(|(_, s)| matches!(s.kind, Kind::Codex | Kind::Cursor))
            .map(|(id, s)| (*id, s.kind))
            .collect();
        self.screen_watch.retain(|id, _| panes.iter().any(|(p, _)| p == id));
        for (pid, kind) in panes {
            let Some(p) = self.pane_ref(pid) else { continue };
            let rows: Vec<String> = p.screen().rows(0, p.cols).collect();
            let seen = look(kind, &p.title, &rows);
            self.screen_seen(pid, seen);
        }
    }

    /// One look's worth for pane `pid`: a wait set or cleared, Cursor busy
    /// or free.
    pub(super) fn screen_seen(&mut self, pid: PaneId, seen: Seen) {
        let mut w = self.screen_watch.remove(&pid).unwrap_or_default();
        match &seen {
            Seen::Waiting(what) => {
                w.wait_misses = 0;
                if !w.waiting {
                    w.waiting = true;
                    // As its hook would say it (`pane-wait`), unless its
                    // hook just did: told once.
                    let now = chrono::Utc::now().timestamp_millis();
                    let told = self
                        .pane_ref(pid)
                        .and_then(|p| p.actor.waiting.as_ref())
                        .is_some_and(|(_, at)| now - at < TOLD_MS);
                    if !told {
                        self.pane_wait_on(pid, what);
                    }
                }
            }
            _ if w.waiting => {
                w.wait_misses += 1;
                if w.wait_misses >= MISSES {
                    w.waiting = false;
                    w.wait_misses = 0;
                    if let Some(p) = self.find_pane_mut(pid) {
                        p.actor.waiting = None;
                    }
                }
            }
            _ => {}
        }
        if seen == Seen::Busy {
            w.busy_misses = 0;
            w.busy = true;
            w.busy_at = Some(chrono::Local::now());
            if let Some(p) = self.find_pane_mut(pid)
                && p.actor.mode == super::actor::WorkMode::Ai
                && p.actor.idle()
            {
                p.actor.turn_began();
                let addr = self.address_of(pid);
                self.observe.pane(&addr, "busy", "");
            }
        } else if w.busy {
            w.busy_misses += 1;
            if w.busy_misses >= MISSES {
                w.busy = false;
                w.busy_misses = 0;
                // Its turn over, as its stop hook would say: the turn the
                // screen saw, still going (nothing delivered or typed since).
                let same = self.find_pane(pid).is_some_and(|p| {
                    !p.actor.idle() && p.actor.last_input.is_none_or(|i| w.busy_at.is_some_and(|b| i <= b))
                });
                if same {
                    self.agent_ready(pid, true, None);
                }
            }
        }
        self.screen_watch.insert(pid, w);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        let mut r = vec![String::new(); 20];
        r.extend(lines.iter().map(|l| l.to_string()));
        r.extend(vec![String::new(); 3]);
        r
    }

    #[test]
    fn codex_waits_by_its_title_or_its_last_lines() {
        assert_eq!(look(Kind::Codex, "⠋ Action Required", &rows(&[])), Seen::Waiting("approve in Codex".into()));
        let ask = rows(&["Run cargo test?", "  Press Enter to confirm or Esc to cancel"]);
        assert_eq!(look(Kind::Codex, "codex", &ask), Seen::Waiting("Press Enter to confirm or Esc to cancel".into()));
        // Higher up than its last lines: an old question, not now.
        let mut old = rows(&["Allow command? (y/n)"]);
        old.extend((0..BOTTOM).map(|i| format!("output {i}")));
        assert_eq!(look(Kind::Codex, "codex", &old), Seen::Nothing);
        // Codex at work is its transcript's to say.
        assert_eq!(look(Kind::Codex, "codex", &rows(&["esc to interrupt · ctrl+c to stop"])), Seen::Nothing);
    }

    #[test]
    fn cursor_waits_or_works_by_its_last_lines() {
        assert_eq!(
            look(Kind::Cursor, "", &rows(&["Run this command?", "> Yes"])),
            Seen::Waiting("Run this command?".into())
        );
        assert_eq!(look(Kind::Cursor, "", &rows(&["⬢ Reading files…", "ctrl+c to stop"])), Seen::Busy);
        assert_eq!(look(Kind::Cursor, "", &rows(&["› type a message"])), Seen::Nothing);
        // Only these two agents.
        assert_eq!(look(Kind::Claude, "Action Required", &rows(&["Run this command?"])), Seen::Nothing);
    }
}
