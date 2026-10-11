use super::*;

/// One bar of the equalizer: its height, its peak mark, and how long the
/// peak still holds before it falls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Bar {
    pub(super) h: f32,
    pub(super) peak: f32,
    pub(super) hold: f32,
}

/// A group of the equalizer: its label, its bars' levels (0 to 1), and the
/// columns its label is given: as wide as it can grow (each figure at its
/// widest), so the layout holds still while the figures change.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Group {
    pub(super) label: String,
    pub(super) levels: Vec<f32>,
    pub(super) room: u16,
}

/// A group whose label at its widest reads like `widest`.
pub(super) fn group(label: String, widest: &str, levels: Vec<f32>) -> Group {
    let room = label.width().max(widest.width()) as u16;
    Group { label, levels, room }
}

impl Anim {
    /// The equalizer's groups, each a label and its bars' levels (0 to 1),
    /// and how many of them are the first ones: a bar a core, memory, the
    /// disks read and written, the network in and out, the temperature, a
    /// bar for each pane's output. Then the ones drawn only where they fit
    /// beside those: messages, the CPU's watts, the battery's, swap, context
    /// switches and interrupts. A figure the machine does not give is left
    /// out.
    pub(super) fn groups(&self) -> (Vec<Group>, usize) {
        let load = &self.input.load;
        let cores = if load.cores.is_empty() { vec![load.cpu] } else { load.cores.clone() };
        let mut out = vec![group(format!("CPU {}", percent(load.cpu)), "CPU 100%", cores)];
        if let Some(m) = load.memory {
            out.push(group(format!("MEM {}", percent(m)), "MEM 100%", vec![m]));
        }
        if let Some((r, w)) = load.disk {
            out.push(group(format!("DISK {}", rate_text(r + w)), "DISK 9999", vec![bytes_level(r), bytes_level(w)]));
        }
        if let Some((rx, tx)) = load.net {
            let label = format!("NET ↓{} ↑{}", rate_text(rx), rate_text(tx));
            out.push(group(label, "NET ↓9999 ↑9999", vec![bytes_level(rx), bytes_level(tx)]));
        }
        if let Some(t) = load.temp {
            out.push(group(format!("{t:.0}°C"), "100°C", vec![((t - 30.0) / 70.0).clamp(0.0, 1.0)]));
        }
        let outs: Vec<f32> = self.out_rates.iter().map(|&r| out_level(r)).collect();
        let total = self.out_rates.iter().sum::<f64>();
        let outs = if outs.is_empty() { vec![0.0] } else { outs };
        out.push(group(format!("OUT {}", rate_text(total)), "OUT 9999", outs));
        let first = out.len();
        out.push(group(format!("MSG {}", self.messages), "MSG 99", vec![self.message_level]));
        if let Some(w) = load.cpu_power {
            out.push(group(watts(w), "999W", vec![power_level(w)]));
        }
        if let Some(w) = load.battery_draw {
            out.push(group(format!("PWR {}", watts(w)), "PWR 999W", vec![power_level(w)]));
        }
        if let Some(s) = load.swap {
            out.push(group(format!("SWAP {}", percent(s)), "SWAP 100%", vec![s.clamp(0.0, 1.0)]));
        }
        if let Some(s) = load.switches {
            let mut levels = vec![count_level(s)];
            levels.extend(load.interrupts.map(count_level));
            out.push(group(format!("CTX {}", human_count(s as u64)), "CTX 999k", levels));
        }
        (out, first)
    }

    pub(super) fn step_bars(&mut self, dt: f64) {
        let levels: Vec<f32> = self.groups().0.into_iter().flat_map(|g| g.levels).collect();
        if self.bars.len() != levels.len() {
            self.bars = vec![Bar::default(); levels.len()];
        }
        let dt = dt as f32;
        for (i, (bar, level)) in self.bars.iter_mut().zip(levels).enumerate() {
            // Each bar dances around its level: most half way up, a little
            // at either end (an idle core stays down, a full one up).
            let wobble = noise(hash(self.seed, i as u64, 9), self.t * 6.0) - 0.5;
            let target = (level + (0.4 * level * (1.0 - level) + 0.03) * wobble).clamp(0.0, 1.0);
            if target > bar.h {
                bar.h += (target - bar.h) * (dt * 20.0).min(1.0);
            } else {
                bar.h = (bar.h - dt * 1.2).max(target);
            }
            if bar.h >= bar.peak {
                (bar.peak, bar.hold) = (bar.h, 0.4);
            } else if bar.hold > 0.0 {
                bar.hold -= dt;
            } else {
                bar.peak = (bar.peak - dt * 0.5).max(bar.h);
            }
        }
    }

    pub(super) fn draw_bars(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        let (mut groups, first) = self.groups();
        // Each group's bars, from the state (`step` sized it to them).
        let mut at = 0;
        let mut bars: Vec<Vec<Bar>> = Vec::new();
        for grp in &groups {
            let end = (at + grp.levels.len()).min(self.bars.len());
            bars.push(self.bars.get(at..end).map(<[Bar]>::to_vec).unwrap_or_default());
            at += grp.levels.len();
        }
        // The first groups decide the look, as if there were no others: the
        // widest that fits, a gap always between two bars (with no gap the
        // cores run together into one block): two columns a bar, then one;
        // the cores taken two, four… together (their height and their peak
        // the average: never under the bar, and one busy core in the group
        // does not pin the mark to the top) until they fit; then the last
        // groups left out. The others follow, as many as fit in that look.
        let extra_groups = groups.split_off(first.min(groups.len()));
        let extra_bars = bars.split_off(first.min(bars.len()));
        let width = |groups: &[Group], bars: &[Vec<Bar>], bw: u16, gap: u16| -> u16 {
            let each = groups.iter().zip(bars).map(|(grp, b)| {
                let n = b.len() as u16;
                grp.room.max(n * bw + n.saturating_sub(1) * gap)
            });
            each.sum::<u16>() + 2 * (groups.len() as u16).saturating_sub(1)
        };
        let fits = |groups: &[Group], bars: &[Vec<Bar>]| {
            [(2, 1), (1, 1)].into_iter().find(|&(bw, gap)| width(groups, bars, bw, gap) <= band.w)
        };
        let cores = bars.first().cloned().unwrap_or_default();
        let mut look = fits(&groups, &bars);
        let mut n = cores.len();
        while look.is_none() && n > 1 {
            n = n.div_ceil(2);
            bars[0] = cores
                .chunks(cores.len().div_ceil(n))
                .map(|c| Bar {
                    h: c.iter().map(|b| b.h).sum::<f32>() / c.len() as f32,
                    peak: c.iter().map(|b| b.peak).sum::<f32>() / c.len() as f32,
                    hold: 0.0,
                })
                .collect();
            look = fits(&groups, &bars);
        }
        while look.is_none() && groups.len() > 1 {
            groups.pop();
            bars.pop();
            look = fits(&groups, &bars);
        }
        let Some((bw, gap)) = look else { return };
        for (g, b) in extra_groups.into_iter().zip(extra_bars) {
            groups.push(g);
            bars.push(b);
            if width(&groups, &bars, bw, gap) > band.w {
                groups.pop();
                bars.pop();
                break;
            }
        }
        let rows = band.h - 1;
        let mut x = band.x + (band.w - width(&groups, &bars, bw, gap)) / 2;
        for (grp, group) in groups.iter().zip(&bars) {
            let label = &grp.label;
            let n = group.len() as u16;
            let inner = n * bw + n.saturating_sub(1) * gap;
            let w = grp.room.max(inner);
            let mut bx = x + (w - inner) / 2;
            for bar in group {
                let eighths = (bar.h * f32::from(rows) * 8.0).round() as i32;
                // The peak's mark: on its own row, over the bar's top one;
                // never on the bottom row, where it would only be noise.
                let peak_row = ((bar.peak * f32::from(rows) * 8.0).round() as i32 - 1) / 8;
                let bar_top = (eighths - 1).div_euclid(8);
                let peak_row = (peak_row > bar_top && peak_row >= 1).then_some(peak_row);
                for r in 0..rows {
                    let fill = (eighths - i32::from(r) * 8).clamp(0, 8) as usize;
                    let y = band.y + rows - 1 - r;
                    let share = f32::from(r + 1) / f32::from(rows);
                    let colour = if share <= 0.5 {
                        GREEN
                    } else if share <= 0.8 {
                        YELLOW
                    } else {
                        RED
                    };
                    let (text, style) = if fill > 0 {
                        (EIGHTHS[fill], pal.shade(colour, 0.0))
                    } else if peak_row == Some(i32::from(r)) {
                        ("▔", pal.shade(BRIGHT_WHITE, 0.0))
                    } else {
                        continue;
                    };
                    for dx in 0..bw {
                        g.set(bx + dx, y, Cell::new(text, false, style));
                    }
                }
                bx += bw + gap;
            }
            let lx = x + (w - label.width() as u16) / 2;
            g.put_str(lx, band.y + rows, label, Style::default(), w);
            x += w + 2;
        }
    }
}
