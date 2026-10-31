//! The board's leftmost column: how the machine is doing. Read here, in the
//! board's own process (it runs on the machine the server does), from the
//! same running totals the clock's animation moves by: two readings a
//! second or more apart give the load.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::sysinfo::{Counters, Load, System};

use super::{paint, width_of};

/// How many of the last readings of the CPU the history line keeps.
const HISTORY: usize = 120;
/// Readings closer than this give no load (the board draws again at each
/// key, many times a second).
const APART: Duration = Duration::from_millis(900);

/// What the column shows, kept between readings.
#[derive(Default)]
pub struct Machine {
    last: Option<(Counters, Instant)>,
    load: Option<Load>,
    /// The CPU's busy share at each reading, oldest first.
    cpu: VecDeque<f32>,
    system: Option<System>,
    host: String,
}

impl Machine {
    /// A reading of the machine (`counters`, `system`, its name) at `at`.
    pub fn sample(&mut self, now: Counters, at: Instant, system: System, host: String) {
        self.system = Some(system);
        self.host = host;
        match &self.last {
            Some((_, then)) if at.duration_since(*then) < APART => return,
            Some((before, then)) => {
                let load = crate::sysinfo::load(before, &now, at.duration_since(*then).as_secs_f64());
                self.cpu.push_back(load.cpu);
                while self.cpu.len() > HISTORY {
                    self.cpu.pop_front();
                }
                self.load = Some(load);
            }
            None => {}
        }
        self.last = Some((now, at));
    }

    /// The column's lines, each in `width` columns at most.
    pub fn lines(&self, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        let dim = |s: &str| paint("2", s);
        if !self.host.is_empty() {
            let up = self.system.as_ref().map(|s| crate::format::human_duration(s.uptime)).unwrap_or_default();
            out.push(format!("{} {}", paint("1", &self.host), dim(&format!("up {up}"))));
        }
        let Some(l) = &self.load else {
            out.push(dim("reading…"));
            return out;
        };
        out.push(meter("CPU", l.cpu, width));
        // The last readings, the newest on the right; then each core.
        out.push(format!("{}{}", paint("2", "hist"), spark(&self.cpu, width.saturating_sub(4))));
        if !l.cores.is_empty() {
            // Each core a block as tall as it is busy, as many to a row as fit.
            let per = width.saturating_sub(4).max(1);
            for (i, chunk) in l.cores.chunks(per).enumerate() {
                let row: String = chunk.iter().map(|c| tinted(*c, &block(*c).to_string())).collect();
                let margin = if i == 0 { paint("2", "core") } else { "    ".to_string() };
                out.push(format!("{margin}{row}"));
            }
            out.push(dim(&format!("    {} cores", l.cores.len())));
        }
        if let Some(m) = l.memory {
            out.push(meter("MEM", m, width));
            if let Some((Some(total), Some(avail))) =
                self.last.as_ref().map(|(c, _)| (c.memory.map(|m| m.0), c.memory.map(|m| m.1)))
            {
                let used = total.saturating_sub(avail);
                out.push(dim(&format!(
                    "    {} / {}",
                    crate::sysinfo::human_bytes(used),
                    crate::sysinfo::human_bytes(total)
                )));
            }
        }
        if let Some(s) = l.swap {
            out.push(meter("SWAP", s, width));
        }
        out.push(String::new());
        let rate = per_second;
        if let Some((r, w)) = l.disk {
            out.push(format!("{} {} {}  {} {}", label("DISK"), dim("r"), rate(r), dim("w"), rate(w)));
        }
        if let Some((d, u)) = l.net {
            out.push(format!("{} {} {}  {} {}", label("NET"), dim("↓"), rate(d), dim("↑"), rate(u)));
        }
        if let Some(t) = l.temp {
            let c = if t >= 85.0 {
                "31"
            } else if t >= 70.0 {
                "33"
            } else {
                "32"
            };
            out.push(format!("{} {}", label("TEMP"), paint(c, &format!("{t:.0}°C"))));
        }
        match (l.cpu_power, l.battery_draw) {
            (Some(c), Some(b)) => out.push(format!("{} cpu {c:.0} W · battery {b:.1} W", label("PWR"))),
            (Some(c), None) => out.push(format!("{} cpu {c:.0} W", label("PWR"))),
            (None, Some(b)) => out.push(format!("{} battery {b:.1} W", label("PWR"))),
            (None, None) => {}
        }
        if let Some(s) = &self.system
            && !s.battery_percentage.is_empty()
        {
            let charging = if s.battery_charging { dim(" charging") } else { String::new() };
            out.push(format!("{} {}{charging}", label("BAT"), s.battery_percentage));
        }
        match (l.processes, l.threads) {
            (Some(p), Some(t)) => out.push(format!("{} {p} · {} threads", label("PROC"), thousands(t))),
            (Some(p), None) => out.push(format!("{} {p}", label("PROC"))),
            _ => {}
        }
        out.into_iter().map(|l| super::clip(&l, width).0).collect()
    }
}

/// A row's name, dim, in four columns and a space.
fn label(name: &str) -> String {
    paint("2", &format!("{name:<4}"))
}

/// The colour of how busy (or full) something is: green, yellow from 60%,
/// red from 80% (as a context's share is).
fn level(share: f32) -> &'static str {
    if share >= 0.8 {
        "31"
    } else if share >= 0.6 {
        "33"
    } else {
        "32"
    }
}

fn tinted(share: f32, text: &str) -> String {
    paint(level(share), text)
}

/// `CPU ██████░░░░  37%`: the share as a bar to the column's width.
fn meter(name: &str, share: f32, width: usize) -> String {
    let share = share.clamp(0.0, 1.0);
    let pct = format!("{:>4}", format!("{:.0}%", share * 100.0));
    let bar_w = width.saturating_sub(5 + 1 + width_of(&pct));
    let full = ((share * bar_w as f32).round() as usize).min(bar_w);
    let bar = format!("{}{}", tinted(share, &"█".repeat(full)), paint("2", &"░".repeat(bar_w - full)));
    format!("{} {bar} {pct}", label(name))
}

/// A share as a block from the lowest (idle) to the full one.
fn block(share: f32) -> char {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    BLOCKS[((share.clamp(0.0, 1.0) * 7.0).round() as usize).min(7)]
}

/// The last `width` readings as a line of blocks, the newest on the right.
fn spark(history: &VecDeque<f32>, width: usize) -> String {
    let skip = history.len().saturating_sub(width);
    history.iter().skip(skip).map(|s| tinted(*s, &block(*s).to_string())).collect()
}

/// Bytes a second as people read them: `820B/s`, `12K/s`, `3.4M/s`.
fn per_second(b: f64) -> String {
    const K: f64 = 1024.0;
    match b {
        b if b < K => format!("{b:.0}B/s"),
        b if b < K * K => format!("{:.0}K/s", b / K),
        b if b < K * K * K => format!("{:.1}M/s", b / (K * K)),
        b => format!("{:.1}G/s", b / (K * K * K)),
    }
}

/// 5321 as `5,321`.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(busy: u64, total: u64) -> Counters {
        Counters {
            cores: vec![(busy, total), (busy / 2, total)],
            memory: Some((16 << 30, 4 << 30)),
            disk: Some((busy * 1000, busy * 500)),
            net: Some((busy * 2000, busy * 100)),
            temp: Some(72.0),
            processes: Some(412),
            threads: Some(5321),
            swap: Some((100, 98)),
            ..Counters::default()
        }
    }

    fn system() -> System {
        System {
            uptime: 3 * 86400 + 4 * 3600,
            battery_percentage: "84%".into(),
            battery_charging: true,
            ..System::default()
        }
    }

    /// Before two readings a second apart there is no load to show; then
    /// each figure on its row, every row within the column; readings too
    /// close together give none.
    #[test]
    fn the_machine_column_shows_each_figure_within_its_width() {
        let mut m = Machine::default();
        let t0 = Instant::now();
        m.sample(counters(100, 1000), t0, system(), "DFINE".into());
        let first = m.lines(26);
        assert!(first.iter().any(|l| super::super::strip(l).contains("reading")), "{first:?}");
        m.sample(counters(700, 2000), t0 + Duration::from_millis(200), system(), "DFINE".into());
        assert!(m.load.is_none(), "too soon: no load yet");
        m.sample(counters(700, 2000), t0 + Duration::from_secs(1), system(), "DFINE".into());
        let lines: Vec<String> = m.lines(26).iter().map(|l| super::super::strip(l)).collect();
        let has = |s: &str| lines.iter().any(|l| l.contains(s));
        assert!(has("DFINE up 3d4h") && has("CPU") && has("MEM") && has("SWAP"), "{lines:#?}");
        assert!(
            has("hist") && has("core") && has("12.0G / 16.0G") && has("2 cores") && has("72°C") && has("84% charging"),
            "{lines:#?}"
        );
        assert!(has("412 · 5,321 threads") && has("DISK") && has("NET"), "{lines:#?}");
        for w in [10, 20, 26, 40] {
            assert!(m.lines(w).iter().all(|l| width_of(l) <= w), "{w}: {:?}", m.lines(w));
        }
        // Both cores together: 600 and 300 of 1000 each busy, 45%.
        let cpu = lines.iter().find(|l| l.starts_with("CPU")).unwrap();
        assert!(cpu.ends_with("45%"), "{cpu}");
        // Disk and network a second: 600,000 and 300,000 bytes read and
        // written; 1,200,000 and 60,000 received and sent.
        assert!(has("r 586K/s  w 293K/s") && has("↓ 1.1M/s  ↑ 59K/s"), "{lines:#?}");
    }

    #[test]
    fn bars_blocks_and_counts() {
        assert_eq!(super::super::strip(&meter("CPU", 0.5, 25)), "CPU  ████████░░░░░░░  50%", "25 columns, the bar 15");
        assert_eq!(super::super::strip(&meter("CPU", 2.0, 25)), "CPU  ███████████████ 100%", "never past full");
        assert_eq!((block(0.0), block(0.5), block(1.0)), ('▁', '▅', '█'));
        assert_eq!((level(0.59), level(0.6), level(0.8)), ("32", "33", "31"));
        let h: VecDeque<f32> = (0..10).map(|i| i as f32 / 10.0).collect();
        assert_eq!(width_of(&spark(&h, 4)), 4, "the newest four");
        assert_eq!(
            (thousands(5), thousands(5321), thousands(1234567)),
            ("5".into(), "5,321".into(), "1,234,567".into())
        );
        assert_eq!(
            (per_second(820.0), per_second(12_288.0), per_second(3.5e6)),
            ("820B/s".into(), "12K/s".into(), "3.3M/s".into())
        );
    }
}
