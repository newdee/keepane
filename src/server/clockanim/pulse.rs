use super::*;

/// The monitor's samples a second; each is half a cell across.
pub(super) const SAMPLES: f64 = 40.0;
/// The most samples kept: a trace 2048 cells wide.
pub(super) const KEEP: usize = 4096;

/// A monitor's label, and the same at its widest.
#[derive(Clone, Debug)]
pub(super) struct Label {
    text: String,
    widest: String,
}

impl Label {
    fn new(text: String, widest: &str) -> Label {
        Label { text, widest: widest.to_string() }
    }

    fn push(&mut self, text: String, widest: &str) {
        self.text.push_str(&text);
        self.widest.push_str(widest);
    }

    /// The columns it is given: its widest, or more when it is wider still.
    fn room(&self) -> u16 {
        self.text.width().max(self.widest.width()) as u16
    }
}

impl Anim {
    /// The heart beats from 60 a minute (cores idle) to 150 (all busy).
    pub(super) fn bpm(&self) -> f64 {
        60.0 + 90.0 * f64::from(self.input.load.cpu)
    }

    pub(super) fn step_pulse(&mut self, dt: f64) {
        let load = &self.input.load;
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let disk = load.disk.map_or(0.0, |(r, w)| bytes_level(r + w));
        // The system's wave: faster the more the CPUs switch between
        // threads, taller the more interrupts come.
        let switching = load.switches.map_or(0.0, count_level);
        let spikes = load.interrupts.map_or(switching, count_level);
        let bpm = self.bpm();
        self.owed += dt * SAMPLES;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            // A message makes the heart beat early: once the last beat is
            // over (half way through it), the next one starts at once.
            if self.early_beats > 0 && self.beat.fract() >= 0.5 {
                self.beat = self.beat.floor() + 1.0;
                self.early_beats -= 1;
            }
            // A sample holds the farthest point of its stretch of the beat:
            // the spike is narrower than a sample at any rate, and would
            // otherwise fall between two.
            let step = bpm / 60.0 / SAMPLES;
            let strongest = |f: fn(f32) -> f32| {
                (1..=8)
                    .map(|i| f((self.beat + step * f64::from(i) / 8.0).fract() as f32))
                    .fold(0.0f32, |a, v| if v.abs() > a.abs() { v } else { a })
            };
            let (heart, finger) = (strongest(ecg), strongest(pleth));
            self.beat += step;
            // Faster breaths the more the disks do: one in 5 s to one in 1.4 s.
            self.breath += (0.2 + 0.5 * f64::from(disk)) / SAMPLES;
            // 2 to 18 swings a second, a faster ripple on them.
            self.wave += (2.0 + 16.0 * f64::from(switching)) / SAMPLES;
            let swing = |key: u64, x: f64| 2.0 * noise(hash(self.seed, key, 13), x) - 1.0;
            let wave = 0.65 * swing(1, self.wave) + 0.35 * swing(2, self.wave * 2.3);
            let sample = [
                heart,
                finger * (0.05 + 0.95 * net),
                (self.breath.fract() as f32 * std::f32::consts::TAU).sin() * (0.05 + 0.95 * disk),
                wave * (0.15 + 0.85 * spikes),
            ];
            self.samples.push_back(sample);
            if self.samples.len() > KEEP {
                self.samples.pop_front();
            }
            self.taken += 1;
        }
    }

    pub(super) fn draw_pulse(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let colour = heat(load);
        let beating = (self.beat.fract() - 0.225).abs() < 0.08;
        // Each trace's label in full, and short: what it says first. The
        // short ones stand where the full ones do not fit. Each is measured
        // at its widest (every figure as wide as it grows), so the traces
        // keep their width while the figures change.
        let mut short = Label::new(format!("♥ {:.0}  CPU {}", self.bpm(), percent(load.cpu)), "♥ 150  CPU 100%");
        let mut heart = short.clone();
        if let Some(w) = load.cpu_power {
            heart.push(format!(" {}", watts(w)), " 999W");
        }
        if let Some(t) = load.temp {
            short.push(format!("  {t:.0}°C"), "  100°C");
            heart.push(format!("  {t:.0}°C"), "  100°C");
        }
        if let Some(w) = load.battery_draw {
            heart.push(format!("  PWR {}", watts(w)), "  PWR 999W");
        }
        if self.messages > 0 {
            heart.push(format!("  {ENVELOPE} {}", self.messages), "  ✉ 99");
        }
        // (which sample, its range, colour, label, short label)
        let mut traces = vec![(0, (-0.3, 1.05), colour, heart, short)];
        if let Some((rx, tx)) = load.net {
            let short = Label::new(format!("NET ↓{} ↑{}", rate_text(rx), rate_text(tx)), "NET ↓9999 ↑9999");
            let mut label = short.clone();
            if let Some((pi, po)) = load.packets {
                label.push(format!("  {} pkt/s", human_count((pi + po) as u64)), "  999k pkt/s");
            }
            traces.push((1, (-0.05, 1.05), BLUE, label, short));
        }
        if let Some((r, w)) = load.disk {
            let label = Label::new(format!("DISK r{} w{}", rate_text(r), rate_text(w)), "DISK r9999 w9999");
            traces.push((2, (-1.05, 1.05), MAGENTA, label.clone(), label));
        }
        // The heart, network and disk decide the rows, as if there were no
        // system wave; it follows where there is room for it.
        let rows = if band.h as usize >= 3 * traces.len() { 3 } else { 2 };
        let fit = (band.h / rows).max(1) as usize;
        if let Some(s) = load.switches.filter(|_| fit > traces.len()) {
            let short = Label::new(format!("SYS {}/s", human_count(s as u64)), "SYS 999k/s");
            let mut label = short.clone();
            if let Some(p) = load.processes {
                label.push(format!("  {p} proc"), "  9999 proc");
            }
            if let Some(t) = load.threads {
                label.push(format!(" {} thr", human_count(t)), " 999k thr");
            }
            traces.push((3, (-1.05, 1.05), CYAN, label, short));
        }
        traces.truncate(fit);
        let width =
            |short: bool| traces.iter().map(|t| if short { t.4.room() } else { t.3.room() }).max().unwrap_or(0) + 1;
        let (trace_w, labels) = match (width(false), width(true)) {
            (w, _) if band.w >= MIN_W + w => (band.w - w, Some(false)),
            (_, w) if band.w >= MIN_W + w => (band.w - w, Some(true)),
            _ => (band.w, None),
        };
        let label_w = band.w - trace_w;
        let y0 = band.y + (band.h - rows * traces.len() as u16) / 2;
        for (i, (which, range, colour, full, short)) in traces.iter().enumerate() {
            let label = if labels == Some(true) { &short.text } else { &full.text };
            let labels = labels.is_some();
            let rect = Rect { x: band.x, y: y0 + i as u16 * rows, w: trace_w, h: rows };
            self.draw_trace(g, rect, *which, *range, *colour, pal);
            if labels {
                let x = band.x + trace_w + 1;
                let mut style = pal.shade(*colour, 0.0);
                if i == 0 && beating {
                    // The heart, red and bright on the beat.
                    g.put_str(x, rect.y, "♥", Style { bold: true, ..pal.shade(RED, 0.0) }, 1);
                    g.put_str(x + 1, rect.y, &label['♥'.len_utf8()..], style, label_w - 2);
                    continue;
                }
                style.bold = i == 0;
                g.put_str(x, rect.y, label, style, label_w - 1);
            }
        }
    }

    /// One trace as a sweeping monitor draws it, in braille (2 x 4 dots a
    /// cell): the newest sample at the sweep, the oldest just ahead of a
    /// short gap, older ones fading.
    pub(super) fn draw_trace(
        &self,
        g: &mut Grid,
        rect: Rect,
        which: usize,
        (lo, hi): (f32, f32),
        colour: u8,
        pal: &Palette,
    ) {
        const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        const GAP: i64 = 4;
        let (sw, sh) = (i64::from(rect.w) * 2, i64::from(rect.h) * 4);
        let Some(newest) = self.taken.checked_sub(1).map(|n| n as i64) else { return };
        let oldest = newest + 1 - self.samples.len() as i64;
        let head = newest.rem_euclid(sw);
        let mut cells = vec![(0u8, f32::MAX); (rect.w as usize) * (rect.h as usize)];
        let mut prev: Option<i64> = None;
        for x in 0..sw {
            let ahead = (x - head).rem_euclid(sw);
            let n = newest - (head - x).rem_euclid(sw);
            if (1..=GAP).contains(&ahead) || n < oldest {
                prev = None;
                continue;
            }
            let v = self.samples[(n - oldest) as usize][which];
            let y = (((1.0 - (v - lo) / (hi - lo)) * (sh - 1) as f32).round() as i64).clamp(0, sh - 1);
            let (a, b) = match prev {
                Some(p) if x > 0 => (p.min(y), p.max(y)),
                _ => (y, y),
            };
            let age = (newest - n) as f32 / sw as f32;
            for sy in a..=b {
                let i = (sy / 4) as usize * rect.w as usize + (x / 2) as usize;
                cells[i].0 |= DOTS[(x % 2) as usize][(sy % 4) as usize];
                cells[i].1 = cells[i].1.min(age);
            }
            prev = Some(y);
        }
        for (i, (mask, age)) in cells.into_iter().enumerate().filter(|(_, (m, _))| *m != 0) {
            let (cx, cy) = ((i % rect.w as usize) as u16, (i / rect.w as usize) as u16);
            let style = if cx == (head / 2) as u16 {
                Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) }
            } else {
                pal.shade(colour, age * 0.75)
            };
            let ch = char::from_u32(0x2800 + u32::from(mask)).unwrap_or(' ');
            g.set(rect.x + cx, rect.y + cy, Cell::new(ch.encode_utf8(&mut [0; 4]), false, style));
        }
    }
}

/// A heartbeat at `p` (0 to 1 through the beat): a small P wave, the tall
/// QRS spike, a T wave, flat between.
fn ecg(p: f32) -> f32 {
    let bump = |at: f32, height: f32, width: f32| height * (-((p - at) / width).powi(2)).exp();
    bump(0.1, 0.1, 0.025)
        + bump(0.2, -0.12, 0.008)
        + bump(0.225, 1.0, 0.009)
        + bump(0.25, -0.25, 0.01)
        + bump(0.45, 0.28, 0.04)
}

/// The pulse at a fingertip, a moment after the heartbeat: a rise and a
/// smaller second one.
fn pleth(p: f32) -> f32 {
    let bump = |at: f32, height: f32, width: f32| height * (-((p - at) / width).powi(2)).exp();
    bump(0.35, 1.0, 0.07) + bump(0.55, 0.35, 0.08)
}
