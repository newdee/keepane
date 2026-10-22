//! `wait-output`: a client held until a pane prints one of some texts (a
//! dev server's "Listening on", a build's "error"), so that a script or an
//! agent waits for it instead of polling `capture-pane`.

use std::time::{Duration, Instant};

use super::pane::Pane;
use super::{ClientId, Outcome, PaneId, Server};
use crate::command::Target;

/// The most one look reads, from where the last one stopped.
const LOOK_MAX: usize = 4 << 20;

pub(super) struct OutWait {
    cid: ClientId,
    pane: PaneId,
    /// What to look for, as given (for what is said back)...
    texts: Vec<String>,
    /// ...and as looked for: in lower case with -i.
    needles: Vec<String>,
    ignore_case: bool,
    /// Where the next look starts (`scrolled_total() + row`): the first row
    /// of the line the cursor was on at the last look, which may still
    /// change.
    from: u64,
    /// Line `at` as it read when the wait began: what it says after that
    /// is new; that much (a prompt, a command typed at it) is not.
    head: Option<(u64, String)>,
    until: Option<Instant>,
}

impl OutWait {
    /// A wait on pane `p` from now (from the oldest line it holds with
    /// `all`).
    fn new(cid: ClientId, p: &mut Pane, texts: Vec<String>, ignore_case: bool, all: bool) -> Self {
        let needles = if ignore_case { texts.iter().map(|t| t.to_lowercase()).collect() } else { texts.clone() };
        let (from, head) = if all {
            (0, None)
        } else {
            let cursor = p.cursor_line();
            let from = line_start(p, cursor);
            let (text, _) = p.text_between(from, cursor + 1, LOOK_MAX);
            (from, Some((from, text.lines().next().unwrap_or("").to_string())))
        };
        OutWait { cid, pane: p.id, texts, needles, ignore_case, from, head, until: None }
    }

    fn holds(&self, line: &str) -> bool {
        if self.ignore_case {
            let line = line.to_lowercase();
            self.needles.iter().any(|t| line.contains(t.as_str()))
        } else {
            self.needles.iter().any(|t| line.contains(t.as_str()))
        }
    }

    /// The texts, for a message: `'a' or 'b'`.
    fn what(&self) -> String {
        self.texts.iter().map(|t| format!("'{t}'")).collect::<Vec<_>>().join(" or ")
    }

    /// What pane `p` has printed since the last look: the first line that
    /// holds one of the texts, if any.
    fn look(&mut self, p: &mut Pane) -> Option<String> {
        if p.screen().alternate_screen() {
            // A full-screen program draws anywhere: its screen as it is.
            let s = p.screen();
            return s.rows(0, s.size().1).map(|r| r.trim_end().to_string()).find(|r| self.holds(r));
        }
        let cursor = p.cursor_line();
        if cursor < self.from {
            // The screen was cleared: what is printed now starts higher up.
            self.from = line_start(p, cursor);
        }
        let (text, _) = p.text_between(self.from, cursor + 1, LOOK_MAX);
        let mut found = None;
        for (i, line) in text.lines().enumerate() {
            let new = match &self.head {
                Some((at, head)) if i == 0 && *at == self.from => line.strip_prefix(head.as_str()).unwrap_or(line),
                _ => line,
            };
            if self.holds(new) {
                found = Some(line.to_string());
                break;
            }
        }
        self.from = line_start(p, cursor);
        found
    }
}

/// The first row of the line row `line` is part of: rows the terminal
/// wrapped are one line (a screenful back at most).
fn line_start(p: &mut Pane, line: u64) -> u64 {
    let rows = p.screen().size().0;
    let mut start = line;
    for _ in 0..rows {
        let Some(above) = start.checked_sub(1) else { break };
        let Some(abs) = p.abs_of_line(above) else { break };
        if !p.goes_on(abs) {
            break;
        }
        start = above;
    }
    start
}

impl Server {
    pub(super) fn wait_output(
        &mut self,
        cid: Option<ClientId>,
        target: Option<&Target>,
        all: bool,
        ignore_case: bool,
        wait: Option<u64>,
        texts: Vec<String>,
    ) -> Outcome {
        let Some(cid) = cid else { return Outcome::Error("wait-output: no client".into()) };
        let pane = match self.resolve(target, Some(cid)) {
            Ok((_, _, p)) => p,
            Err(e) => return Outcome::Error(e),
        };
        let Some(p) = self.find_pane_mut(pane) else { return Outcome::Error(format!("no pane %{pane}")) };
        let mut w = OutWait::new(cid, p, texts, ignore_case, all);
        w.until = wait.map(|w| Instant::now() + Duration::from_secs(w));
        if let Some(line) = w.look(p) {
            return Outcome::Text(line);
        }
        if let Some(code) = p.exit_code {
            return Outcome::Error(format!("%{pane} exited with {code}; {} not seen", w.what()));
        }
        self.out_waits.push(w);
        Outcome::Pending
    }

    /// Pane `pane` printed: the waits on it look at what it printed.
    pub(super) fn output_waits_look(&mut self, pane: PaneId) {
        if !self.out_waits.iter().any(|w| w.pane == pane) {
            return;
        }
        let mut waits = std::mem::take(&mut self.out_waits);
        let mut seen = Vec::new();
        if let Some(p) = self.find_pane_mut(pane) {
            waits.retain_mut(|w| match (w.pane == pane).then(|| w.look(p)).flatten() {
                Some(line) => {
                    seen.push((w.cid, line));
                    false
                }
                None => true,
            });
        }
        self.out_waits = waits;
        for (cid, line) in seen {
            self.reply(cid, Outcome::Text(line));
        }
    }

    /// Pane `pane`'s program ended: nothing more will come from it.
    pub(super) fn output_waits_ended(&mut self, pane: PaneId, code: u32) {
        self.output_waits_end(pane, &format!("exited with {code}"));
    }

    /// Pane `pane` is closing (`kill-pane`, `kill-window`, its session
    /// ending), whether or not `undo-kill` keeps it a while.
    pub(super) fn output_waits_gone(&mut self, pane: PaneId) {
        self.output_waits_end(pane, "is gone");
    }

    fn output_waits_end(&mut self, pane: PaneId, how: &str) {
        let (ended, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.out_waits).into_iter().partition(|w| w.pane == pane);
        self.out_waits = kept;
        for w in ended {
            let what = w.what();
            self.reply(w.cid, Outcome::Error(format!("%{pane} {how}; {what} not seen")));
        }
    }

    /// From the tick: waits that ran out of time. (A pane's end, and its
    /// closing, end its waits at once: `output_waits_ended` and `_gone`.)
    pub(super) fn output_waits_tick(&mut self) {
        let now = Instant::now();
        let (over, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.out_waits).into_iter().partition(|w| w.until.is_some_and(|u| u <= now));
        self.out_waits = kept;
        for w in over {
            let why = format!("timed out: {} not seen in %{}", w.what(), w.pane);
            self.reply(w.cid, Outcome::Error(why));
        }
    }

    /// A client went away: its waits go with it.
    pub(super) fn output_waits_client_gone(&mut self, cid: ClientId) {
        self.out_waits.retain(|w| w.cid != cid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane fed by hand: its own program exits at once and its output is
    /// never read, so only what the test writes reaches the screen.
    fn pane(cols: u16, rows: u16, history: usize) -> Pane {
        let argv: Vec<String> = if cfg!(windows) {
            ["cmd.exe", "/c", "exit"].map(String::from).to_vec()
        } else {
            ["/bin/sh", "-c", "exit"].map(String::from).to_vec()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        Pane::spawn(1, &argv, None, cols, rows, history, &[], tx).unwrap()
    }

    fn wait(p: &mut Pane, texts: &[&str], ignore_case: bool, all: bool) -> OutWait {
        OutWait::new(7, p, texts.iter().map(|t| t.to_string()).collect(), ignore_case, all)
    }

    /// Only what comes after the wait began counts: a line printed before
    /// it, or the prompt and command at the cursor, do not; -a takes them.
    #[test]
    fn only_what_comes_after_counts_unless_all() {
        let mut p = pane(40, 6, 100);
        p.process_output(b"ready in 9ms\r\nPS> npm run dev");
        let mut w = wait(&mut p, &["ready", "dev"], false, false);
        assert_eq!(w.look(&mut p), None, "the old line and the command typed are not new");
        p.process_output(b"\r\n> vite\r\n");
        assert_eq!(w.look(&mut p), None);
        p.process_output(b"  VITE ready in 312 ms");
        assert_eq!(w.look(&mut p).as_deref(), Some("  VITE ready in 312 ms"));

        let mut all = wait(&mut p, &["ready in 9"], false, true);
        assert_eq!(all.look(&mut p).as_deref(), Some("ready in 9ms"));
        // Nothing found: none.
        let mut none = wait(&mut p, &["nowhere"], false, true);
        assert_eq!(none.look(&mut p), None);
    }

    /// The line at the cursor counts from what it said when the wait began:
    /// more printed on it counts, the rest (the prompt) does not; a line
    /// written over (a progress bar's `\r`) counts whole.
    #[test]
    fn the_cursors_line_counts_from_where_it_was() {
        let mut p = pane(40, 6, 100);
        p.process_output(b"PS> echo build");
        let mut w = wait(&mut p, &["build", "done"], false, false);
        p.process_output(b" --release");
        assert_eq!(w.look(&mut p), None, "the start of the line was there before");
        p.process_output(b" done");
        assert_eq!(w.look(&mut p).as_deref(), Some("PS> echo build --release done"));

        let mut p = pane(40, 6, 100);
        p.process_output(b"[====      ] 40%");
        let mut w = wait(&mut p, &["100%"], false, false);
        p.process_output(b"\r[==========] 100%");
        assert_eq!(w.look(&mut p).as_deref(), Some("[==========] 100%"));
    }

    /// -i matches any case; any of the texts does; the line comes back as
    /// printed.
    #[test]
    fn any_case_and_any_text() {
        let mut p = pane(40, 6, 100);
        let mut w = wait(&mut p, &["listening", "Error"], true, false);
        p.process_output(b"Server LISTENING on :3000\r\n");
        assert_eq!(w.look(&mut p).as_deref(), Some("Server LISTENING on :3000"));
        assert_eq!(w.what(), "'listening' or 'Error'", "said back as given");
        let mut w = wait(&mut p, &["listening"], false, false);
        p.process_output(b"LISTENING again\r\n");
        assert_eq!(w.look(&mut p), None, "case counts without -i");
        let mut w = wait(&mut p, &["nope", "宽字"], false, false);
        p.process_output("中文 宽字 ok\r\n".as_bytes());
        assert_eq!(w.look(&mut p).as_deref(), Some("中文 宽字 ok"));
    }

    /// A line the terminal wrapped is one line: a text across the wrap is
    /// found, and the next look starts at the wrapped line's first row.
    #[test]
    fn a_wrapped_line_is_one_line() {
        let mut p = pane(10, 6, 100);
        let mut w = wait(&mut p, &["ghijkl"], false, false);
        p.process_output(b"abcdefgh");
        assert_eq!(w.look(&mut p), None);
        p.process_output(b"ijklmn");
        assert_eq!(w.look(&mut p).as_deref(), Some("abcdefghijklmn"));

        // The last look left the cursor on the wrapped line's second row:
        // the next starts at its first, so a text across the wrap is found.
        let mut p = pane(10, 6, 100);
        let mut w = wait(&mut p, &["ijklmn"], false, false);
        p.process_output(b"abcdefghijk");
        assert_eq!(w.look(&mut p), None);
        assert_eq!(w.from, p.cursor_line() - 1, "the wrapped line's first row");
        p.process_output(b"lmn");
        assert_eq!(w.look(&mut p).as_deref(), Some("abcdefghijklmn"));
        // A wait begun there takes the whole line as what was there.
        let mut w = wait(&mut p, &["cde", "xyz"], false, false);
        p.process_output(b"xyz");
        assert_eq!(w.look(&mut p).as_deref(), Some("abcdefghijklmnxyz"));
        let mut w = wait(&mut p, &["cde"], false, false);
        p.process_output(b"!");
        assert_eq!(w.look(&mut p), None, "the start of the wrapped line was there before");
    }

    /// A burst of lines is read through in one look, scrolled or not; the
    /// next look starts at the cursor's line, so a line is not found twice.
    #[test]
    fn a_burst_is_read_through_once() {
        let mut p = pane(40, 6, 2000);
        let mut w = wait(&mut p, &["line 500"], false, false);
        let burst: String = (1..=1000).map(|i| format!("line {i} \r\n")).collect();
        p.process_output(burst.as_bytes());
        assert_eq!(w.look(&mut p).as_deref(), Some("line 500"));

        let mut w = wait(&mut p, &["seen"], false, false);
        p.process_output(b"seen once\r\n");
        assert_eq!(w.look(&mut p).as_deref(), Some("seen once"));
        assert_eq!(w.from, p.cursor_line(), "the next look starts at the cursor");
        assert_eq!(w.look(&mut p), None, "not found again");
    }

    /// A cleared screen, and a full-screen program (its screen as it is).
    #[test]
    fn a_cleared_screen_and_a_full_screen_program() {
        let mut p = pane(40, 6, 100);
        for i in 0..5 {
            p.process_output(format!("old {i}\r\n").as_bytes());
        }
        let mut w = wait(&mut p, &["fresh"], false, false);
        p.process_output(b"\x1b[2J\x1b[Hfresh start");
        assert_eq!(w.look(&mut p).as_deref(), Some("fresh start"));

        // Drawn above where the cursor is left, and above where the wait
        // began: a full-screen program's screen is read whole.
        p.process_output(b"\r\n\r\n\r\n\r\n");
        let mut w = wait(&mut p, &["READY"], false, false);
        p.process_output(b"\x1b[?1049h\x1b[2;3HREADY\x1b[6;1H");
        assert_eq!(w.look(&mut p).as_deref(), Some("  READY"));
        p.process_output(b"\x1b[?1049l");
        assert_eq!(w.look(&mut p), None, "back to the main screen: nothing new there");
    }
}
