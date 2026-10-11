//! What moves under the clock (`clock-mode-animation`): a monitor's traces
//! (`pulse`), falling characters (`matrix`) or an equalizer (`bars`), each
//! moved by what the machine is doing (its cores, memory, disks, network,
//! the CPU's temperature) and by what the pane prints.
//!
//! It is drawn in the 16 colours only, so a theme's `pane-colours` colour
//! it (the terminal's own when the theme sets none); a tail fades into the
//! theme's background where both are known, else it is drawn dim.
//!
//! An `Anim` moves only by `step`, by the time it is given: the same steps
//! give the same frames, whatever the time of day.

use super::layout::Rect;
use super::render::{Cell, Grid, Style};
use crate::sysinfo::Load;
use std::collections::VecDeque;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;
use vt100::Color;

/// The kinds, in the order `cycle` takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pulse,
    Matrix,
    Bars,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Pulse, Kind::Matrix, Kind::Bars];

    /// The time between two frames: enough for each to look smooth, no more.
    pub fn frame(self) -> Duration {
        Duration::from_millis(match self {
            Kind::Pulse => 50,
            Kind::Matrix => 80,
            Kind::Bars => 66,
        })
    }
}

/// The least room an animation is drawn in; less, and nothing is.
pub const MIN_W: u16 = 20;
pub const MIN_H: u16 = 3;

/// What moves it: the machine's load, and how many bytes the pane has
/// printed so far.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub load: Load,
    pub out_bytes: u64,
}

/// The theme's colours: its 16 (none: the terminal's own) and the panes'
/// background.
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub colours: Vec<Color>,
    pub bg: Color,
}

impl Palette {
    /// Colour `i` of the 16, `fade` of the way (0 to 1) into the
    /// background. Unfaded it stays colour `i`, for the theme to colour;
    /// faded, a mix when both are RGB, else colour `i` dim past half way.
    fn shade(&self, i: u8, fade: f32) -> Style {
        let fade = fade.clamp(0.0, 1.0);
        if fade > 0.0
            && let (Some(Color::Rgb(r, g, b)), Color::Rgb(br, bg, bb)) = (self.colours.get(usize::from(i)), self.bg)
        {
            let mix = |a: u8, z: u8| (f32::from(a) + (f32::from(z) - f32::from(a)) * fade).round() as u8;
            return Style::colors(Color::Rgb(mix(*r, br), mix(*g, bg), mix(*b, bb)), Color::Default);
        }
        Style { dim: fade >= 0.5, ..Style::colors(Color::Idx(i), Color::Default) }
    }
}

// The 16 colours by their index.
const RED: u8 = 1;
const GREEN: u8 = 2;
const YELLOW: u8 = 3;
const BLUE: u8 = 4;
const MAGENTA: u8 = 5;
const CYAN: u8 = 6;
const BRIGHT_WHITE: u8 = 15;

/// Green, yellow or red: by the CPU's temperature when it is known (below
/// 60 °C, below 80 °C, above), else by how busy the cores are.
fn heat(load: &Load) -> u8 {
    match load.temp {
        Some(t) if t < 60.0 => GREEN,
        Some(t) if t < 80.0 => YELLOW,
        Some(_) => RED,
        None if load.cpu < 0.5 => GREEN,
        None if load.cpu < 0.8 => YELLOW,
        None => RED,
    }
}

/// Bytes a second on a log scale: 1 KB/s and less is 0, 1 GB/s and more 1.
fn bytes_level(rate: f64) -> f32 {
    ((rate.max(1.0).log10() - 3.0) / 6.0).clamp(0.0, 1.0) as f32
}

/// A pane's output a second on a log scale: 10 B/s is 0, 10 MB/s is 1.
fn out_level(rate: f64) -> f32 {
    ((rate.max(1.0).log10() - 1.0) / 6.0).clamp(0.0, 1.0) as f32
}

/// Bytes a second, short: "0", "812", "12K", "1.2M".
fn rate_text(b: f64) -> String {
    const UNITS: [&str; 4] = ["", "K", "M", "G"];
    let (mut v, mut i) = (b.max(0.0), 0);
    while v >= 1000.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i > 0 && v < 10.0 { format!("{v:.1}{}", UNITS[i]) } else { format!("{v:.0}{}", UNITS[i]) }
}

fn percent(share: f32) -> String {
    format!("{}%", (share * 100.0).round() as u32)
}

/// A number from a few others, the same each time (splitmix64).
fn hash(a: u64, b: u64, c: u64) -> u64 {
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    mix(a ^ mix(b ^ mix(c)))
}

/// A hash as a number from 0 to 1.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth noise from 0 to 1 along `x`, one curve per `key`.
fn noise(key: u64, x: f64) -> f32 {
    let i = x.floor();
    let f = (x - i) as f32;
    let (a, b) = (unit(hash(key, i as i64 as u64, 7)), unit(hash(key, (i as i64 + 1) as u64, 7)));
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}

/// The monitor's samples a second; each is half a cell across.
const SAMPLES: f64 = 40.0;
/// The most samples kept: a trace 2048 cells wide.
const KEEP: usize = 4096;

/// One bar of the equalizer: its height, its peak mark, and how long the
/// peak still holds before it falls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Bar {
    h: f32,
    peak: f32,
    hold: f32,
}

/// An animation under one pane's clock.
#[derive(Clone, Debug)]
pub struct Anim {
    pub kind: Kind,
    seed: u64,
    input: Input,
    /// Seconds stepped so far.
    t: f64,
    /// The pane's output a second, smoothed, and the count last given.
    out_rate: f64,
    out_last: Option<u64>,
    /// `pulse`: the samples (heart, network, disk), newest last, how many
    /// were ever taken, the part of one still owed, and where the heart
    /// and the breath are in their cycles.
    samples: VecDeque<[f32; 3]>,
    taken: u64,
    owed: f64,
    beat: f64,
    breath: f64,
    /// `matrix`: how far the rain has fallen, and how often the
    /// characters have changed.
    travel: f64,
    flicker: f64,
    /// `bars`: one a level `levels` gives.
    bars: Vec<Bar>,
}

impl Anim {
    pub fn new(kind: Kind, seed: u64) -> Anim {
        Anim {
            kind,
            seed,
            input: Input::default(),
            t: 0.0,
            out_rate: 0.0,
            out_last: None,
            samples: VecDeque::new(),
            taken: 0,
            owed: 0.0,
            beat: 0.0,
            breath: 0.0,
            travel: 0.0,
            flicker: 0.0,
            bars: Vec::new(),
        }
    }

    /// Move on by `dt` seconds with `input`. A long gap (a machine that
    /// slept) moves it half a second, not the whole gap.
    pub fn step(&mut self, dt: f64, input: Input) {
        let dt = dt.clamp(0.0, 0.5);
        self.t += dt;
        if let Some(last) = self.out_last
            && dt > 0.0
        {
            let rate = input.out_bytes.saturating_sub(last) as f64 / dt;
            self.out_rate += (rate - self.out_rate) * (1.0 - (-dt / 0.4).exp());
        }
        self.out_last = Some(input.out_bytes);
        self.input = input;
        match self.kind {
            Kind::Pulse => self.step_pulse(dt),
            Kind::Matrix => self.step_matrix(dt),
            Kind::Bars => self.step_bars(dt),
        }
    }

    /// Draw it in `band`, when the band is big enough; nothing outside it
    /// is touched.
    pub fn draw(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        if band.w < MIN_W || band.h < MIN_H {
            return;
        }
        match self.kind {
            Kind::Pulse => self.draw_pulse(g, band, pal),
            Kind::Matrix => self.draw_matrix(g, band, pal),
            Kind::Bars => self.draw_bars(g, band, pal),
        }
    }

    /// The heart beats from 60 a minute (cores idle) to 150 (all busy).
    fn bpm(&self) -> f64 {
        60.0 + 90.0 * f64::from(self.input.load.cpu)
    }

    fn step_pulse(&mut self, dt: f64) {
        let load = &self.input.load;
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let disk = load.disk.map_or(0.0, |(r, w)| bytes_level(r + w));
        let bpm = self.bpm();
        self.owed += dt * SAMPLES;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
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
            let sample = [
                heart,
                finger * (0.05 + 0.95 * net),
                (self.breath.fract() as f32 * std::f32::consts::TAU).sin() * (0.05 + 0.95 * disk),
            ];
            self.samples.push_back(sample);
            if self.samples.len() > KEEP {
                self.samples.pop_front();
            }
            self.taken += 1;
        }
    }

    fn draw_pulse(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let colour = heat(load);
        let beating = (self.beat.fract() - 0.225).abs() < 0.08;
        let mut heart = format!("♥ {:.0}  CPU {}", self.bpm(), percent(load.cpu));
        if let Some(t) = load.temp {
            heart.push_str(&format!("  {t:.0}°C"));
        }
        // (which sample, its range, colour, label)
        let mut traces = vec![(0, (-0.3, 1.05), colour, heart)];
        if let Some((rx, tx)) = load.net {
            traces.push((1, (-0.05, 1.05), BLUE, format!("NET ↓{} ↑{}", rate_text(rx), rate_text(tx))));
        }
        if let Some((r, w)) = load.disk {
            traces.push((2, (-1.05, 1.05), MAGENTA, format!("DISK r{} w{}", rate_text(r), rate_text(w))));
        }
        let rows = if band.h as usize >= 3 * traces.len() { 3 } else { 2 };
        traces.truncate((band.h / rows).max(1) as usize);
        let label_w = traces.iter().map(|t| t.3.width()).max().unwrap_or(0) as u16 + 1;
        let (trace_w, labels) = if band.w >= MIN_W + label_w { (band.w - label_w, true) } else { (band.w, false) };
        let y0 = band.y + (band.h - rows * traces.len() as u16) / 2;
        for (i, (which, range, colour, label)) in traces.iter().enumerate() {
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
    fn draw_trace(&self, g: &mut Grid, rect: Rect, which: usize, (lo, hi): (f32, f32), colour: u8, pal: &Palette) {
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

    fn step_matrix(&mut self, dt: f64) {
        let load = &self.input.load;
        // Faster rain the busier the cores; quicker flicker the more the
        // disks do.
        self.travel += dt * (0.6 + 1.6 * f64::from(load.cpu));
        let disk = load.disk.map_or(0.0, |(r, w)| bytes_level(r + w));
        self.flicker += dt * (2.0 + 18.0 * f64::from(disk));
    }

    fn draw_matrix(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        // Half-width katakana and digits, each one column wide.
        const CHARS: &str = "ｦｧｨｩｪｫｬｭｮｯｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789";
        let chars: Vec<char> = CHARS.chars().collect();
        let load = &self.input.load;
        // More columns fall the more memory is in use; the more the network
        // carries, the more of them are cyan.
        let density = 0.25 + 0.6 * load.memory.unwrap_or(0.4);
        let packets = 0.03 + 0.5 * load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let colour = heat(load);
        let h = f64::from(band.h);
        for c in 0..band.w {
            let hc = hash(self.seed, u64::from(c), 1);
            let speed = 6.0 + 10.0 * f64::from(unit(hash(hc, 1, 0)));
            let len = 4.0 + f64::from(unit(hash(hc, 2, 0))) * (h * 0.8 + 4.0);
            let cycle = h + len + f64::from(unit(hash(hc, 3, 0))) * h;
            let pos = self.travel * speed + f64::from(unit(hash(hc, 4, 0))) * cycle;
            let k = (pos / cycle).floor();
            if unit(hash(hc, k as u64, 5)) >= density {
                continue;
            }
            let packet = unit(hash(hc, k as u64, 6)) < packets;
            let head = (pos - k * cycle).floor() as i64;
            for d in 0..len as i64 {
                let y = head - d;
                if y < 0 || y >= i64::from(band.h) {
                    continue;
                }
                let hy = hash(hc, y as u64, 7);
                let slot = (self.flicker * (0.5 + f64::from(unit(hy)))).floor() as u64;
                let ch = chars[(hash(hy, slot, k as u64) % chars.len() as u64) as usize];
                let style = if d == 0 {
                    Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) }
                } else {
                    pal.shade(if packet { CYAN } else { colour }, d as f32 / len as f32 * 0.85)
                };
                g.set(band.x + c, band.y + y as u16, Cell::new(ch.encode_utf8(&mut [0; 4]), false, style));
            }
        }
    }

    /// The equalizer's groups, each a label and its bars' levels (0 to 1):
    /// a bar a core, memory, the disks read and written, the network in
    /// and out, the temperature, the pane's output. A figure the machine
    /// does not give is left out.
    fn groups(&self) -> Vec<(String, Vec<f32>)> {
        let load = &self.input.load;
        let cores = if load.cores.is_empty() { vec![load.cpu] } else { load.cores.clone() };
        let mut out = vec![(format!("CPU {}", percent(load.cpu)), cores)];
        if let Some(m) = load.memory {
            out.push((format!("MEM {}", percent(m)), vec![m]));
        }
        if let Some((r, w)) = load.disk {
            out.push((format!("DISK {}", rate_text(r + w)), vec![bytes_level(r), bytes_level(w)]));
        }
        if let Some((rx, tx)) = load.net {
            out.push((format!("NET ↓{} ↑{}", rate_text(rx), rate_text(tx)), vec![bytes_level(rx), bytes_level(tx)]));
        }
        if let Some(t) = load.temp {
            out.push((format!("{t:.0}°C"), vec![((t - 30.0) / 70.0).clamp(0.0, 1.0)]));
        }
        out.push((format!("OUT {}", rate_text(self.out_rate)), vec![out_level(self.out_rate)]));
        out
    }

    fn step_bars(&mut self, dt: f64) {
        let levels: Vec<f32> = self.groups().into_iter().flat_map(|(_, l)| l).collect();
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

    fn draw_bars(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        let mut groups = self.groups();
        // Each group's bars, from the state (`step` sized it to them).
        let mut at = 0;
        let mut bars: Vec<Vec<Bar>> = Vec::new();
        for (_, levels) in &groups {
            let end = (at + levels.len()).min(self.bars.len());
            bars.push(self.bars.get(at..end).map(<[Bar]>::to_vec).unwrap_or_default());
            at += levels.len();
        }
        // The widest look that fits: wide bars, narrow ones, the cores as
        // one bar, then the last groups left out.
        let width = |groups: &[(String, Vec<f32>)], bars: &[Vec<Bar>], bw: u16, gap: u16| -> u16 {
            let each = groups.iter().zip(bars).map(|((label, _), b)| {
                let n = b.len() as u16;
                (label.width() as u16).max(n * bw + n.saturating_sub(1) * gap)
            });
            each.sum::<u16>() + 2 * (groups.len() as u16).saturating_sub(1)
        };
        let fits = |groups: &[(String, Vec<f32>)], bars: &[Vec<Bar>]| {
            [(2, 1), (1, 1), (1, 0)].into_iter().find(|&(bw, gap)| width(groups, bars, bw, gap) <= band.w)
        };
        let mut look = fits(&groups, &bars);
        if look.is_none() && bars.first().is_some_and(|b| b.len() > 1) {
            let cores = &bars[0];
            let n = cores.len() as f32;
            let one = Bar {
                h: cores.iter().map(|b| b.h).sum::<f32>() / n,
                peak: cores.iter().map(|b| b.peak).fold(0.0, f32::max),
                hold: 0.0,
            };
            bars[0] = vec![one];
            look = fits(&groups, &bars);
        }
        while look.is_none() && groups.len() > 1 {
            groups.pop();
            bars.pop();
            look = fits(&groups, &bars);
        }
        let Some((bw, gap)) = look else { return };
        let rows = band.h - 1;
        let mut x = band.x + (band.w - width(&groups, &bars, bw, gap)) / 2;
        for ((label, _), group) in groups.iter().zip(&bars) {
            let n = group.len() as u16;
            let inner = n * bw + n.saturating_sub(1) * gap;
            let w = (label.width() as u16).max(inner);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> Rect {
        Rect { x: 2, y: 3, w: 60, h: 9 }
    }

    fn busy() -> Load {
        Load {
            cores: vec![0.9, 0.1, 0.5, 1.0],
            cpu: 0.625,
            memory: Some(0.41),
            disk: Some((12.0 * 1024.0 * 1024.0, 300.0 * 1024.0)),
            net: Some((1.2 * 1024.0 * 1024.0, 80.0 * 1024.0)),
            temp: Some(61.0),
        }
    }

    /// `steps` frames of `dt` with the same input, from a new animation.
    fn run(kind: Kind, load: &Load, steps: usize, dt: f64) -> Anim {
        let mut a = Anim::new(kind, 7);
        for i in 0..steps {
            a.step(dt, Input { load: load.clone(), out_bytes: i as u64 * 4000 });
        }
        a
    }

    fn drawn(a: &Anim, band: Rect, pal: &Palette) -> Grid {
        let mut g = Grid::new(64, 14);
        a.draw(&mut g, band, pal);
        g
    }

    fn text(g: &Grid) -> String {
        (0..g.rows).map(|y| (0..g.cols).map(|x| g.get(x, y).text()).collect::<String>() + "\n").collect()
    }

    /// The same steps give the same frame, cell for cell, each kind; and
    /// nothing is drawn outside the band.
    #[test]
    fn the_same_steps_give_the_same_frame_inside_the_band() {
        let pal = Palette::default();
        for kind in Kind::ALL {
            let (a, b) = (run(kind, &busy(), 90, 0.05), run(kind, &busy(), 90, 0.05));
            let (ga, gb) = (drawn(&a, band(), &pal), drawn(&b, band(), &pal));
            assert_eq!(ga, gb, "{kind:?}");
            let b = band();
            let outside = (0..ga.rows)
                .flat_map(|y| (0..ga.cols).map(move |x| (x, y)))
                .filter(|&(x, y)| x < b.x || x >= b.x + b.w || y < b.y || y >= b.y + b.h);
            for (x, y) in outside {
                assert_eq!(ga.get(x, y), Grid::new(64, 14).get(x, y), "{kind:?} drew at ({x}, {y})");
            }
            assert_ne!(text(&ga).trim(), "", "{kind:?} drew something");
        }
    }

    /// Every size from nothing to wide, each kind, with a machine that
    /// gives everything, nothing, 64 cores, or figures out of range: no
    /// panic, nothing drawn outside the band.
    #[test]
    fn every_size_draws_inside_its_band() {
        let wild = Load {
            cores: vec![2.0, -1.0],
            cpu: f32::NAN,
            memory: Some(7.0),
            disk: Some((f64::INFINITY, -5.0)),
            net: Some((1e300, 0.0)),
            temp: Some(f32::NAN),
        };
        let loads = [busy(), Load::default(), Load { cores: vec![0.7; 64], cpu: 0.7, ..busy() }, wild];
        let mut checked = 0;
        for kind in Kind::ALL {
            for load in &loads {
                let a = run(kind, load, 30, 0.05);
                for w in (0..=130).step_by(7) {
                    for h in 0..=13 {
                        let band = Rect { x: 1, y: 1, w, h };
                        let mut g = Grid::new(132, 15);
                        a.draw(&mut g, band, &Palette::default());
                        for y in 0..g.rows {
                            for x in 0..g.cols {
                                let inside = x >= band.x && x < band.x + w && y >= band.y && y < band.y + h;
                                assert!(inside || g.get(x, y).text() == " ", "{kind:?} {w}x{h} drew at ({x}, {y})");
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 3 * 4 * 19 * 14);
    }

    /// Too little room: nothing at all.
    #[test]
    fn a_band_too_small_is_left_alone() {
        let pal = Palette::default();
        for kind in Kind::ALL {
            let a = run(kind, &busy(), 40, 0.05);
            for small in [Rect { w: MIN_W - 1, ..band() }, Rect { h: MIN_H - 1, ..band() }] {
                assert_eq!(drawn(&a, small, &pal), Grid::new(64, 14), "{kind:?} {small:?}");
            }
            assert_ne!(
                drawn(&a, Rect { w: MIN_W, h: MIN_H, ..band() }, &pal),
                Grid::new(64, 14),
                "{kind:?} at the least"
            );
        }
    }

    /// The heart follows the cores: 60 beats a minute idle, 150 all busy;
    /// six seconds hold 6 and 15 beats.
    #[test]
    fn the_heart_beats_with_the_cpu() {
        let beats = |cpu: f32| {
            let a = run(Kind::Pulse, &Load { cpu, ..Load::default() }, 120, 0.05);
            let s: Vec<f32> = a.samples.iter().map(|s| s[0]).collect();
            s.windows(2).filter(|w| w[0] < 0.5 && w[1] >= 0.5).count()
        };
        assert_eq!(beats(0.0), 6);
        assert_eq!(beats(1.0), 15);
        assert_eq!(run(Kind::Pulse, &Load::default(), 120, 0.05).samples.len(), 240, "40 samples a second");
    }

    /// The traces follow the machine: no network or disk figure, no trace
    /// for it; their labels say the figures; a quiet network draws flat.
    #[test]
    fn the_monitor_shows_what_the_machine_gives() {
        let pal = Palette::default();
        let t = text(&drawn(&run(Kind::Pulse, &busy(), 100, 0.05), band(), &pal));
        assert!(t.contains("CPU 63%") && t.contains("61°C"), "{t}");
        assert!(t.contains("NET ↓1.2M ↑80K") && t.contains("DISK r12M w300K"), "{t}");
        let bare = Load { cpu: 0.2, ..Load::default() };
        let t = text(&drawn(&run(Kind::Pulse, &bare, 100, 0.05), band(), &pal));
        assert!(t.contains("CPU 20%") && !t.contains("NET") && !t.contains("DISK") && !t.contains("°C"), "{t}");
        // The network's trace: its tallest point by how much goes through.
        let span = |net: f64| {
            let a = run(Kind::Pulse, &Load { net: Some((net, 0.0)), ..Load::default() }, 100, 0.05);
            a.samples.iter().map(|s| s[1]).fold(0.0, f32::max)
        };
        assert!(span(0.0) < 0.1 && span(100.0 * 1024.0 * 1024.0) > 0.7, "{} {}", span(0.0), span(1e8));
    }

    /// The disks move both: the monitor's breath comes faster and deeper,
    /// the rain's characters change quicker.
    #[test]
    fn the_disks_quicken_the_breath_and_the_flicker() {
        let disk = |rate: f64| Load { disk: Some((rate, 0.0)), ..Load::default() };
        let breaths = |load: &Load| {
            let a = run(Kind::Pulse, load, 240, 0.05);
            let s: Vec<f32> = a.samples.iter().map(|s| s[2]).collect();
            let deepest = s.iter().copied().fold(0.0, f32::max);
            (s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count(), deepest)
        };
        // 12 s: one breath in 5 s with the disks idle, one in 1.4 s busy.
        let (calm, shallow) = breaths(&disk(0.0));
        let (busy, deep) = breaths(&disk(1e9));
        assert_eq!((calm, busy), (2, 8), "breaths in 12 s");
        assert!(shallow < 0.1 && deep > 0.95, "{shallow} {deep}");
        let flicker = |load: &Load| run(Kind::Matrix, load, 50, 0.08).flicker;
        assert!(flicker(&disk(1e9)) > flicker(&disk(0.0)) * 5.0, "{} {}", flicker(&disk(0.0)), flicker(&disk(1e9)));
    }

    /// Green, yellow, red: by the temperature when there is one (whatever
    /// the cores do), else by the cores; the heart's trace and the rain take
    /// it.
    #[test]
    fn the_colour_follows_the_temperature_else_the_cores() {
        let at = |temp: Option<f32>, cpu: f32| heat(&Load { temp, cpu, ..Load::default() });
        assert_eq!((at(Some(45.0), 1.0), at(Some(70.0), 0.0), at(Some(85.0), 0.0)), (GREEN, YELLOW, RED));
        assert_eq!((at(None, 0.3), at(None, 0.6), at(None, 0.95)), (GREEN, YELLOW, RED));
        let pal = Palette::default();
        let colours = |kind: Kind, temp: f32| {
            let g = drawn(
                &run(kind, &Load { temp: Some(temp), memory: Some(1.0), ..Load::default() }, 80, 0.05),
                band(),
                &pal,
            );
            (0..g.rows)
                .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
                .map(|(x, y)| g.get(x, y).style.fg)
                .collect::<Vec<_>>()
        };
        for kind in [Kind::Pulse, Kind::Matrix] {
            let (cool, hot) = (colours(kind, 40.0), colours(kind, 90.0));
            assert!(cool.contains(&Color::Idx(GREEN)) && !cool.contains(&Color::Idx(RED)), "{kind:?} cool");
            assert!(hot.contains(&Color::Idx(RED)) && !hot.contains(&Color::Idx(GREEN)), "{kind:?} hot");
        }
    }

    /// The rain: more columns the more memory is used, faster the busier
    /// the cores, cyan columns with the network.
    #[test]
    fn the_rain_follows_memory_cores_and_network() {
        let pal = Palette::default();
        let lit = |load: &Load| {
            let g = drawn(&run(Kind::Matrix, load, 60, 0.08), band(), &pal);
            (0..g.rows)
                .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
                .filter(|&(x, y)| g.get(x, y).text() != " ")
                .count()
        };
        let few = lit(&Load { memory: Some(0.0), ..Load::default() });
        let many = lit(&Load { memory: Some(1.0), ..Load::default() });
        assert!(many > few * 2, "{few} {many}");
        let fell = |cpu: f32| run(Kind::Matrix, &Load { cpu, ..Load::default() }, 50, 0.08).travel;
        assert!(fell(1.0) > fell(0.0) * 3.0, "{} {}", fell(0.0), fell(1.0));
        let cyan = |load: &Load| {
            let g = drawn(&run(Kind::Matrix, load, 60, 0.08), Rect { x: 0, y: 0, w: 64, h: 14 }, &pal);
            (0..g.rows)
                .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
                .filter(|&(x, y)| g.get(x, y).style.fg == Color::Idx(CYAN))
                .count()
        };
        let quiet = Load { memory: Some(1.0), net: Some((0.0, 0.0)), ..Load::default() };
        let loud = Load { net: Some((1e9, 1e9)), ..quiet.clone() };
        assert!(cyan(&loud) > cyan(&quiet) * 3, "{} {}", cyan(&quiet), cyan(&loud));
    }

    /// The equalizer: a full core is a full bar, an idle one empty; each
    /// group labelled; what the machine does not give, left out; too
    /// narrow, the cores become one bar.
    #[test]
    fn the_bars_stand_for_each_figure() {
        let pal = Palette::default();
        let load = Load { cores: vec![1.0, 0.0], cpu: 0.5, ..Load::default() };
        let a = run(Kind::Bars, &load, 60, 0.066);
        assert!(a.bars[0].h > 0.95 && a.bars[1].h < 0.05, "{:?}", a.bars);
        let g = drawn(&a, band(), &pal);
        let t = text(&g);
        assert!(t.contains("CPU 50%") && t.contains("OUT ") && !t.contains("MEM") && !t.contains("NET"), "{t}");
        let top: Vec<u16> = (0..g.cols).filter(|&x| g.get(x, band().y).text() != " ").collect();
        assert_eq!(top.len(), 2, "one wide bar reaches the top row: {t}");
        let t = text(&drawn(&run(Kind::Bars, &busy(), 60, 0.066), band(), &pal));
        for label in ["CPU 63%", "MEM 41%", "DISK 12M", "NET ↓1.2M ↑80K", "61°C", "OUT "] {
            assert!(t.contains(label), "{label}: {t}");
        }
        // A bar that falls leaves its peak's mark over it a while; an idle
        // one has none, on the bottom row or anywhere.
        let full = Load { cores: vec![1.0], cpu: 1.0, ..Load::default() };
        let mut a = run(Kind::Bars, &full, 30, 0.066);
        let idle = Load { cores: vec![0.0], ..Load::default() };
        for _ in 0..5 {
            a.step(0.066, Input { load: idle.clone(), out_bytes: 0 });
        }
        let g = drawn(&a, band(), &pal);
        assert!((0..g.cols).any(|x| g.get(x, band().y).text() == "▔"), "{}", text(&g));
        let g = drawn(&run(Kind::Bars, &idle, 30, 0.066), band(), &pal);
        assert!(!text(&g).contains('▔'), "{}", text(&g));
        // 32 cores in 40 columns: one CPU bar, the rest still there.
        let many = Load { cores: vec![0.5; 32], cpu: 0.5, memory: Some(0.3), ..Load::default() };
        let a = run(Kind::Bars, &many, 30, 0.066);
        let narrow = Rect { x: 0, y: 0, w: 40, h: 6 };
        let t = text(&drawn(&a, narrow, &pal));
        assert!(t.contains("CPU 50%") && t.contains("MEM 30%") && t.contains("OUT"), "{t}");
    }

    /// Colours are the theme's: the 16 by index (the theme's palette maps
    /// them); a fading tail mixes into the background when both are RGB,
    /// else it is the same colour, dim.
    #[test]
    fn the_colours_are_the_themes() {
        let none = Palette::default();
        assert_eq!(none.shade(GREEN, 0.0), Style::colors(Color::Idx(GREEN), Color::Default));
        assert!(none.shade(GREEN, 0.7).dim && !none.shade(GREEN, 0.2).dim);
        let mut colours = vec![Color::Idx(0); 16];
        colours[2] = Color::Rgb(0x9e, 0xce, 0x6a);
        let night = Palette { colours, bg: Color::Rgb(0x1a, 0x1b, 0x26) };
        assert_eq!(night.shade(GREEN, 0.0).fg, Color::Idx(GREEN), "unfaded: the theme colours it");
        assert_eq!(night.shade(GREEN, 0.5).fg, Color::Rgb(0x5c, 0x75, 0x48), "half way to the background");
        assert_eq!(night.shade(GREEN, 1.0).fg, Color::Rgb(0x1a, 0x1b, 0x26));
        // What an animation draws: only the 16, or mixes of them.
        for kind in Kind::ALL {
            let g = drawn(&run(kind, &busy(), 60, 0.05), band(), &night);
            for y in 0..g.rows {
                for x in 0..g.cols {
                    let fg = g.get(x, y).style.fg;
                    assert!(matches!(fg, Color::Default | Color::Idx(0..=15) | Color::Rgb(..)), "{kind:?} {fg:?}");
                }
            }
        }
    }

    #[test]
    fn rates_and_levels_read_short() {
        assert_eq!(rate_text(0.0), "0");
        assert_eq!(rate_text(812.0), "812");
        assert_eq!(rate_text(12.0 * 1024.0), "12K");
        assert_eq!(rate_text(1.2 * 1024.0 * 1024.0), "1.2M");
        assert_eq!(rate_text(5e12), "4657G");
        assert_eq!((bytes_level(0.0), bytes_level(1e3), bytes_level(1e9), bytes_level(1e12)), (0.0, 0.0, 1.0, 1.0));
        assert_eq!(out_level(10.0), 0.0);
        assert!((bytes_level(1e6) - 0.5).abs() < 1e-6);
    }
}
