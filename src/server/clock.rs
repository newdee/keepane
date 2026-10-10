//! What `clock-mode` (prefix t) shows under the time, with `clock-mode-info`
//! on: the date (and UTC), the pane (its program, folder and branch, how long
//! it has run and been quiet), its last command (how long it took, how it
//! ended), its agent (model, cost, context) and the machine (CPU, memory,
//! battery, time since boot). Each line only when there is something to say.

use super::layout::PaneId;
use super::pane::{Pane, command_of};
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

/// The lines, in order, each with its colour (None: the pane's own).
pub(super) fn clock_lines(
    now: DateTime<Local>,
    hours: u8,
    pane: &PaneFacts,
    last: Option<&LastCommand>,
    agent: Option<&agents::Stats>,
    sys: &crate::sysinfo::System,
) -> Vec<(String, Option<Color>)> {
    let clock = |t: DateTime<chrono::FixedOffset>| {
        if hours == 12 { t.format("%-I:%M %p").to_string() } else { t.format("%H:%M").to_string() }
    };
    let utc = now.with_timezone(&Utc).fixed_offset();
    let mut lines = vec![(format!("{} · UTC {}", now.format("%A, %-d %B %Y"), clock(utc)), None)];

    let mut here = pane.program.clone();
    if !pane.path.is_empty() {
        here.push_str(&format!(" in {}", pane.path));
    }
    if !pane.branch.is_empty() {
        here.push_str(&format!(" ({})", pane.branch));
    }
    here.push_str(&format!(" · up {} · quiet {}", human_duration(pane.up_secs), human_duration(pane.quiet_secs)));
    lines.push((here, None));

    if let Some(l) = last.filter(|l| !l.command.is_empty()) {
        let took = |a: DateTime<Local>, b: DateTime<Local>| command_duration((b - a).num_milliseconds());
        let line = match (l.start, l.end) {
            (start, Some(end)) => {
                let mut s = format!("last: {}", l.command);
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
                (s, (colour != Color::Default).then_some(colour))
            }
            (Some(start), None) => (format!("running: {} · for {}", l.command, took(start, now)), Some(Color::Idx(3))),
            (None, None) => (format!("last: {}", l.command), None),
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

impl Server {
    /// `clock_lines` for pane `id`, from what the server knows of it now.
    pub(super) fn clock_info(&self, p: &Pane, id: PaneId, now: DateTime<Local>) -> Vec<(String, Option<Color>)> {
        let path = p.current_path().unwrap_or_default();
        let facts = PaneFacts {
            program: p
                .pid
                .map(crate::sysinfo::program_of)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.command.clone()),
            branch: if path.is_empty() { String::new() } else { crate::sysinfo::git_branch(&path) },
            path: crate::sysinfo::short_path(&path),
            up_secs: p.spawned_at.elapsed().as_secs() as i64,
            quiet_secs: p.last_output.elapsed().as_secs() as i64,
        };
        // The newest command that was typed (an empty Enter is none).
        let last = p.marks.iter().rev().find_map(|m| {
            let command = command_of(&m.text, m.col);
            (!command.is_empty() && (m.start.is_some() || m.end.is_some())).then_some(LastCommand {
                command,
                start: m.start,
                end: m.end,
                exit: m.exit,
            })
        });
        clock_lines(
            now,
            self.opts.clock_mode_style,
            &facts,
            last.as_ref(),
            self.agents.stats.get(&id),
            &crate::sysinfo::system(),
        )
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
        let lines = clock_lines(now, 24, &facts(), Some(&ended), None, &sys());
        let text: Vec<&str> = lines.iter().map(|(s, _)| s.as_str()).collect();
        let utc = now.with_timezone(&Utc).format("%H:%M").to_string();
        assert_eq!(text[0], format!("Saturday, 10 October 2026 · UTC {utc}"));
        assert_eq!(text[1], "pwsh in ~/src/keepane (main) · up 3h12m · quiet 4m");
        assert_eq!(text[2], "last: cargo test · 1m42s · ✗ exit 101 · finished 14:20");
        assert_eq!(lines[2].1, Some(Color::Idx(1)), "a failure in red");
        assert_eq!(text[3], "CPU 12% · memory 41% · since boot 2d4h", "no battery: not said");
        assert_eq!(lines.len(), 4, "no agent: no agent line");
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
        let lines = clock_lines(now, 12, &facts(), Some(&running), Some(&agent), &sys());
        let text: Vec<&str> = lines.iter().map(|(s, _)| s.as_str()).collect();
        assert!(text[0].contains(" AM") || text[0].contains(" PM"), "UTC in 12 hours: {}", text[0]);
        assert_eq!(text[2], "running: npm run dev · for 1m05s");
        assert_eq!(text[3], "claude claude-opus-5-5 · $2.10 · context 34% · 1.2M tokens");
        let done =
            LastCommand { exit: Some(0), end: Some(now), start: Some(now - chrono::Duration::seconds(3)), ..running };
        let lines = clock_lines(now, 12, &facts(), Some(&done), None, &sys());
        assert_eq!(lines[2].0, "last: npm run dev · 3.0s · ✓ · finished 2:25 PM");
        assert_eq!(lines[2].1, Some(Color::Idx(2)));
        // Nothing typed yet: no command line.
        let none = LastCommand { command: String::new(), ..done };
        assert_eq!(clock_lines(now, 24, &facts(), Some(&none), None, &sys()).len(), 3);
    }
}
