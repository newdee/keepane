//! What `clock-mode` (prefix t) shows under the time, with `clock-mode-info`
//! on: the date (and UTC), the pane (its program, folder and branch, how long
//! it has run and been quiet), its last command (how long it took, how it
//! ended), its agent (model, cost, context), the machine (CPU, memory,
//! battery, time since boot) and keepane (its sessions and panes, the
//! messages it has delivered). Each line only when there is something to say.

use super::layout::PaneId;
use super::pane::Pane;
use super::{Server, agents};
use crate::format::{command_duration, human_count, human_duration};
use chrono::{DateTime, Local, Utc};
use vt100::Color;

/// What is known about the pane, as `clock_lines` needs it.
pub(super) struct PaneFacts {
    pub program: String,
    pub path: String,
    pub branch: String,
    pub up_secs: i64,
    pub quiet_secs: i64,
}

/// The pane's last command: what was typed, when it started and ended,
/// and its exit code (None: still running, or not said).
pub(super) struct LastCommand {
    pub command: String,
    pub start: Option<DateTime<Local>>,
    pub end: Option<DateTime<Local>>,
    pub exit: Option<i32>,
}

/// `text` in `room` columns: as it is if it fits; else cut, the cut marked
/// `…`, at its start (`keep_end`: a path, whose end says where) or its end
/// (a command, whose start says what).
fn fit(text: &str, room: usize, keep_end: bool) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= room || room == 0 {
        return if room == 0 { String::new() } else { text.to_string() };
    }
    let mut kept = Vec::new();
    let mut used = 1; // the `…`
    let chars: Vec<char> = if keep_end { text.chars().rev().collect() } else { text.chars().collect() };
    for c in chars {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    if keep_end {
        std::iter::once('…').chain(kept.into_iter().rev()).collect()
    } else {
        kept.into_iter().chain(std::iter::once('…')).collect()
    }
}

/// The lines, in order, each with its colour (None: the pane's own), each
/// at most `width` columns: a long path loses its start and a long command
/// its end, so what follows them on the line (how long, how it ended)
/// stays.
pub(super) fn clock_lines(
    now: DateTime<Local>,
    hours: u8,
    pane: &PaneFacts,
    last: Option<&LastCommand>,
    agent: Option<&agents::Stats>,
    sys: &crate::sysinfo::System,
    width: usize,
) -> Vec<(String, Option<Color>)> {
    use unicode_width::UnicodeWidthStr;
    let room = |fixed: &str| width.saturating_sub(fixed.width());
    let clock = |t: DateTime<chrono::FixedOffset>| {
        if hours == 12 { t.format("%-I:%M %p").to_string() } else { t.format("%H:%M").to_string() }
    };
    let utc = now.with_timezone(&Utc).fixed_offset();
    let mut lines = vec![(format!("{} · UTC {}", now.format("%A, %-d %B %Y"), clock(utc)), None)];

    let mut after = String::new();
    if !pane.branch.is_empty() {
        after.push_str(&format!(" ({})", pane.branch));
    }
    after.push_str(&format!(" · up {} · quiet {}", human_duration(pane.up_secs), human_duration(pane.quiet_secs)));
    let here = if pane.path.is_empty() {
        format!("{}{after}", pane.program)
    } else {
        let before = format!("{} in ", pane.program);
        format!("{before}{}{after}", fit(&pane.path, room(&format!("{before}{after}")), true))
    };
    lines.push((here, None));

    if let Some(l) = last.filter(|l| !l.command.is_empty()) {
        let took = |a: DateTime<Local>, b: DateTime<Local>| command_duration((b - a).num_milliseconds());
        // What follows the command, then the command in what is left.
        let line = match (l.start, l.end) {
            (start, Some(end)) => {
                let mut s = String::new();
                if let Some(start) = start {
                    s.push_str(&format!(" · {}", took(start, end)));
                }
                let (mark, colour) = match l.exit {
                    Some(0) => ("✓".to_string(), Color::Idx(2)),
                    Some(code) => (format!("✗ exit {code}"), Color::Idx(1)),
                    None => (String::new(), Color::Default),
                };
                if !mark.is_empty() {
                    s.push_str(&format!(" · {mark}"));
                }
                let at = if hours == 12 { end.format("%-I:%M %p") } else { end.format("%H:%M") };
                s.push_str(&format!(" · finished {at}"));
                let command = fit(&l.command, room(&format!("last: {s}")), false);
                (format!("last: {command}{s}"), (colour != Color::Default).then_some(colour))
            }
            (Some(start), None) => {
                let s = format!(" · for {}", took(start, now));
                let command = fit(&l.command, room(&format!("running: {s}")), false);
                (format!("running: {command}{s}"), Some(Color::Idx(3)))
            }
            (None, None) => (format!("last: {}", fit(&l.command, room("last: "), false)), None),
        };
        lines.push(line);
    }

    if let Some(a) = agent {
        let mut s = a.kind.word().to_string();
        if !a.usage.model.is_empty() {
            s.push_str(&format!(" {}", a.usage.model));
        }
        let cost = a.cost_text();
        if !cost.is_empty() {
            s.push_str(&format!(" · {cost}"));
        }
        let context = a.context_text();
        if !context.is_empty() {
            s.push_str(&format!(" · context {context}"));
        }
        s.push_str(&format!(" · {} tokens", human_count(a.usage.tokens.total())));
        lines.push((s, Some(Color::Idx(5))));
    }

    let mut machine = Vec::new();
    for (word, v) in
        [("CPU", &sys.cpu_percentage), ("memory", &sys.ram_percentage), ("battery", &sys.battery_percentage)]
    {
        if !v.is_empty() {
            machine.push(format!("{word} {v}"));
        }
    }
    if sys.uptime > 0 {
        machine.push(format!("since boot {}", human_duration(sys.uptime)));
    }
    if !machine.is_empty() {
        lines.push((machine.join(" · "), None));
    }
    lines
}

/// The line about keepane itself, last under the clock: its sessions and
/// panes, the messages it has delivered since it started.
pub(super) fn keepane_line(sessions: usize, panes: usize, messages: u64) -> String {
    let n = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    format!(
        "keepane: {} · {} · {} delivered",
        n(sessions as u64, "session"),
        n(panes as u64, "pane"),
        n(messages, "message")
    )
}

/// The pane's newest command that was typed (an empty Enter is none),
/// running or done: all of it, the rows it wrapped onto too.
pub(super) fn last_command(p: &Pane) -> Option<LastCommand> {
    p.marks.iter().rev().find_map(|m| {
        (!m.command.is_empty() && (m.start.is_some() || m.end.is_some())).then(|| LastCommand {
            command: m.command.clone(),
            start: m.start,
            end: m.end,
            exit: m.exit,
        })
    })
}

impl Server {
    /// `clock_lines` for pane `id`, from what the server knows of it now,
    /// for a pane `width` columns wide, and `keepane_line` after them.
    pub(super) fn clock_info(
        &self,
        p: &Pane,
        id: PaneId,
        now: DateTime<Local>,
        width: usize,
    ) -> Vec<(String, Option<Color>)> {
        let path = p.current_path().unwrap_or_default();
        let facts = PaneFacts {
            program: p
                .pid
                .map(crate::sysinfo::program_of)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.command.clone()),
            // The branch, and what git last said is changed there (`●3 ↑2`).
            branch: match (crate::git::branch_of(&path), self.git_status(&path).map(|s| s.marks())) {
                (b, Some(m)) if !b.is_empty() && !m.is_empty() => format!("{b} {m}"),
                (b, _) => b,
            },
            path: crate::sysinfo::short_path(&path),
            up_secs: p.spawned_at.elapsed().as_secs() as i64,
            quiet_secs: p.last_output.elapsed().as_secs() as i64,
        };
        let last = last_command(p);
        let mut lines = clock_lines(
            now,
            self.opts.clock_mode_style,
            &facts,
            last.as_ref(),
            self.agents.stats.get(&id),
            &crate::sysinfo::system(),
            width,
        );
        let panes = self.sessions.iter().flat_map(|s| &s.windows).map(|w| w.panes.len()).sum();
        lines.push((keepane_line(self.sessions.len(), panes, self.delivered), None));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn facts() -> PaneFacts {
        PaneFacts {
            program: "pwsh".into(),
            path: "~/src/keepane".into(),
            branch: "main".into(),
            up_secs: 3 * 3600 + 12 * 60,
            quiet_secs: 4 * 60,
        }
    }

    fn sys() -> crate::sysinfo::System {
        crate::sysinfo::System {
            cpu_percentage: "12%".into(),
            ram_percentage: "41%".into(),
            battery_percentage: String::new(),
            uptime: 2 * 86400 + 4 * 3600,
            ..Default::default()
        }
    }

    /// A narrow pane: a long path loses its start and a long command its
    /// end, so what follows them on the line still shows; no line is wider
    /// than the pane.
    #[test]
    fn a_long_path_or_command_gives_way_to_what_follows_it() {
        use unicode_width::UnicodeWidthStr;
        let now = Local.with_ymd_and_hms(2026, 10, 10, 14, 25, 0).unwrap();
        let deep = PaneFacts { path: "/private/var/folders/36/tjdph2t965j8snz9_vkdnw0r0000gn/T".into(), ..facts() };
        let long = LastCommand {
            command: "cargo build --release --target x86_64-unknown-linux-musl --features full".into(),
            start: Some(now - chrono::Duration::seconds(102)),
            end: Some(now),
            exit: Some(0),
        };
        let lines = clock_lines(now, 24, &deep, Some(&long), None, &sys(), 60);
        assert_eq!(lines[1].0, "pwsh in …j8snz9_vkdnw0r0000gn/T (main) · up 3h12m · quiet 4m");
        assert_eq!(lines[2].0, "last: cargo build --release --… · 1m42s · ✓ · finished 14:25");
        assert!(lines.iter().all(|(l, _)| l.width() <= 60), "{lines:?}");
        assert_eq!((lines[1].0.width(), lines[2].0.width()), (60, 60), "the room used, not more");
        // Wide enough: as they are.
        let lines = clock_lines(now, 24, &deep, Some(&long), None, &sys(), 200);
        assert!(lines[1].0.contains("/private/var/folders/") && lines[2].0.contains("--features full"));
        // Wide characters count by their columns; nothing to keep, nothing.
        assert_eq!(fit("项目/文档", 6, true), "…/文档");
        assert_eq!(fit("项目/文档", 6, false), "项目/…");
        assert_eq!(fit("项目/文档", 5, false), "项目…", "a wide character never half");
        assert_eq!(fit("abc", 3, true), "abc");
        assert_eq!(fit("abcd", 1, true), "…");
        assert_eq!(fit("abcd", 0, false), "");
    }

    #[test]
    fn under_the_clock_each_line_says_one_thing() {
        let now = Local.with_ymd_and_hms(2026, 10, 10, 14, 25, 0).unwrap();
        let ended = LastCommand {
            command: "cargo test".into(),
            start: Some(now - chrono::Duration::seconds(102)),
            end: Some(now - chrono::Duration::seconds(300)),
            exit: Some(101),
        };
        let ended = LastCommand { start: Some(now - chrono::Duration::seconds(402)), ..ended };
        let lines = clock_lines(now, 24, &facts(), Some(&ended), None, &sys(), 200);
        let text: Vec<&str> = lines.iter().map(|(s, _)| s.as_str()).collect();
        let utc = now.with_timezone(&Utc).format("%H:%M").to_string();
        assert_eq!(text[0], format!("Saturday, 10 October 2026 · UTC {utc}"));
        assert_eq!(text[1], "pwsh in ~/src/keepane (main) · up 3h12m · quiet 4m");
        assert_eq!(text[2], "last: cargo test · 1m42s · ✗ exit 101 · finished 14:20");
        assert_eq!(lines[2].1, Some(Color::Idx(1)), "a failure in red");
        assert_eq!(text[3], "CPU 12% · memory 41% · since boot 2d4h", "no battery: not said");
        assert_eq!(lines.len(), 4, "no agent: no agent line");
        // What keepane itself has: its sessions and panes, the messages
        // it has delivered; one of each said as one.
        assert_eq!(keepane_line(2, 7, 12), "keepane: 2 sessions · 7 panes · 12 messages delivered");
        assert_eq!(keepane_line(1, 1, 1), "keepane: 1 session · 1 pane · 1 message delivered");
        assert_eq!(keepane_line(1, 1, 0), "keepane: 1 session · 1 pane · 0 messages delivered");
    }

    #[test]
    fn twelve_hours_running_commands_and_agents() {
        let now = Local.with_ymd_and_hms(2026, 10, 10, 14, 25, 0).unwrap();
        let running = LastCommand {
            command: "npm run dev".into(),
            start: Some(now - chrono::Duration::seconds(65)),
            end: None,
            exit: None,
        };
        let mut usage = agents::Usage { model: "claude-opus-5-5".into(), ..Default::default() };
        usage.tokens.input = 1_200_000;
        let agent = agents::Stats { usage, cost: Some(2.1), context_pct: Some(34), ..Default::default() };
        let lines = clock_lines(now, 12, &facts(), Some(&running), Some(&agent), &sys(), 200);
        let text: Vec<&str> = lines.iter().map(|(s, _)| s.as_str()).collect();
        assert!(text[0].contains(" AM") || text[0].contains(" PM"), "UTC in 12 hours: {}", text[0]);
        assert_eq!(text[2], "running: npm run dev · for 1m05s");
        assert_eq!(text[3], "claude claude-opus-5-5 · $2.10 · context 34% · 1.2M tokens");
        let done =
            LastCommand { exit: Some(0), end: Some(now), start: Some(now - chrono::Duration::seconds(3)), ..running };
        let lines = clock_lines(now, 12, &facts(), Some(&done), None, &sys(), 200);
        assert_eq!(lines[2].0, "last: npm run dev · 3.0s · ✓ · finished 2:25 PM");
        assert_eq!(lines[2].1, Some(Color::Idx(2)));
        // Nothing typed yet: no command line.
        let none = LastCommand { command: String::new(), ..done };
        assert_eq!(clock_lines(now, 24, &facts(), Some(&none), None, &sys(), 200).len(), 3);
    }
}
