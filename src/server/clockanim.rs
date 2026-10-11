//! What moves under the clock (`clock-mode-animation`): a monitor's traces
//! (`pulse`), falling characters (`matrix`) or an equalizer (`bars`), each
//! moved by what the machine is doing (its cores, memory, disks, network,
//! the CPU's temperature and power, context switches, interrupts, swap) and
//! by what keepane does (what the window's panes print, the messages it
//! delivers).
//!
//! It is drawn in the 16 colours only, so a theme's `pane-colours` colour
//! it (the terminal's own when the theme sets none); a tail fades into the
//! theme's background where both are known, else it is drawn dim.
//!
//! An `Anim` moves only by `step`, by the time it is given: the same steps
//! give the same frames, whatever the time of day.

use super::layout::Rect;
use super::render::{Cell, Grid, Style};
use crate::format::human_count;
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

/// What moves it: the machine's load; how many bytes each pane of the
/// clock's window has printed so far, in the window's order; how many
/// messages keepane has delivered so far.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub load: Load,
    pub outs: Vec<u64>,
    pub messages: u64,
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

/// A message's mark.
const ENVELOPE: &str = "✉";

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

/// Events a second (context switches, interrupts) on a log scale: 1000 a
/// second and fewer is 0, a million and more 1.
fn count_level(rate: f64) -> f32 {
    ((rate.max(1.0).log10() - 3.0) / 3.0).clamp(0.0, 1.0) as f32
}

/// Watts on a log scale: 1 W and less is 0, 1000 W 1 (a laptop idles near
/// 0.3, a desktop's CPU under load near 0.7).
fn power_level(w: f32) -> f32 {
    (w.max(1.0).log10() / 3.0).clamp(0.0, 1.0)
}

/// Watts, short: "8.8W", "67W".
fn watts(w: f32) -> String {
    if w < 10.0 { format!("{w:.1}W") } else { format!("{w:.0}W") }
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

/// A group of the equalizer: its label, its bars' levels (0 to 1), and the
/// columns its label is given: as wide as it can grow (each figure at its
/// widest), so the layout holds still while the figures change.
#[derive(Clone, Debug, PartialEq)]
struct Group {
    label: String,
    levels: Vec<f32>,
    room: u16,
}

/// A group whose label at its widest reads like `widest`.
fn group(label: String, widest: &str, levels: Vec<f32>) -> Group {
    let room = label.width().max(widest.width()) as u16;
    Group { label, levels, room }
}

/// A monitor's label, and the same at its widest.
#[derive(Clone, Debug)]
struct Label {
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

/// An animation under one pane's clock.
#[derive(Clone, Debug)]
pub struct Anim {
    pub kind: Kind,
    seed: u64,
    input: Input,
    /// Seconds stepped so far.
    t: f64,
    /// Each pane's output a second, smoothed, and the counts last given.
    out_rates: Vec<f64>,
    outs_last: Option<Vec<u64>>,
    /// The messages delivered since it began, the count last given, and
    /// the messages' bar: full on one, falling after.
    messages: u64,
    messages_last: Option<u64>,
    message_level: f32,
    /// `pulse`: the samples (heart, network, disk, system), newest last,
    /// how many were ever taken, the part of one still owed, where the
    /// heart, the breath and the system's wave are in their cycles, and the
    /// beats messages still owe.
    samples: VecDeque<[f32; 4]>,
    taken: u64,
    owed: f64,
    beat: f64,
    breath: f64,
    wave: f64,
    early_beats: u32,
    /// `matrix`: how far the rain has fallen, how often the characters
    /// have changed, and each message's drop (which message, when it
    /// began to fall).
    travel: f64,
    flicker: f64,
    drops: VecDeque<(u64, f64)>,
    /// `bars`: one a level `levels` gives.
    bars: Vec<Bar>,
}

/// How fast a message's drop falls, rows a second, and how many are kept.
const DROP_SPEED: f64 = 18.0;
const DROPS: usize = 32;

impl Anim {
    pub fn new(kind: Kind, seed: u64) -> Anim {
        Anim {
            kind,
            seed,
            input: Input::default(),
            t: 0.0,
            out_rates: Vec::new(),
            outs_last: None,
            messages: 0,
            messages_last: None,
            message_level: 0.0,
            samples: VecDeque::new(),
            taken: 0,
            owed: 0.0,
            beat: 0.0,
            breath: 0.0,
            wave: 0.0,
            early_beats: 0,
            travel: 0.0,
            flicker: 0.0,
            drops: VecDeque::new(),
            bars: Vec::new(),
        }
    }

    /// Move on by `dt` seconds with `input`. A long gap (a machine that
    /// slept) moves it half a second, not the whole gap.
    pub fn step(&mut self, dt: f64, input: Input) {
        let dt = dt.clamp(0.0, 0.5);
        self.t += dt;
        // A pane more or fewer: each starts again from nothing.
        match &self.outs_last {
            Some(last) if last.len() == input.outs.len() => {
                if dt > 0.0 {
                    let ease = 1.0 - (-dt / 0.4).exp();
                    for ((r, now), last) in self.out_rates.iter_mut().zip(&input.outs).zip(last) {
                        *r += (now.saturating_sub(*last) as f64 / dt - *r) * ease;
                    }
                }
            }
            _ => self.out_rates = vec![0.0; input.outs.len()],
        }
        self.outs_last = Some(input.outs.clone());
        // The messages delivered since the last step (none on the first:
        // those came before the clock).
        let new = self.messages_last.map_or(0, |last| input.messages.saturating_sub(last));
        self.messages_last = Some(input.messages);
        self.message_level *= (-dt / 0.8).exp() as f32;
        if new > 0 {
            self.messages += new;
            self.message_level = 1.0;
            self.early_beats = (self.early_beats + new.min(4) as u32).min(4);
            for n in self.messages.saturating_sub(new.min(DROPS as u64))..self.messages {
                self.drops.push_back((n, self.t));
            }
        }
        // A drop gone past any screen is let go.
        while self.drops.front().is_some_and(|&(_, at)| (self.t - at) * DROP_SPEED > 400.0) || self.drops.len() > DROPS
        {
            self.drops.pop_front();
        }
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

    fn draw_pulse(&self, g: &mut Grid, band: Rect, pal: &Palette) {
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
        // Under the rain, interrupts: stray characters in magenta, more the
        // more come, new ones ten times a second.
        let strays = (load.interrupts.map_or(0.0, count_level) * f32::from(band.w) * f32::from(band.h) * 0.04) as u64;
        let slot = (self.t * 10.0).floor() as u64;
        for i in 0..strays {
            let hs = hash(self.seed, slot, i + 100);
            let (x, y) = ((hs % u64::from(band.w)) as u16, ((hs >> 20) % u64::from(band.h)) as u16);
            let ch = chars[((hs >> 40) % chars.len() as u64) as usize];
            g.set(band.x + x, band.y + y, Cell::new(ch.encode_utf8(&mut [0; 4]), false, pal.shade(MAGENTA, 0.4)));
        }
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
        // Each message delivered: a white drop headed by an envelope, over
        // the rain.
        const TAIL: i64 = 6;
        for &(n, at) in &self.drops {
            let c = (hash(self.seed, n, 11) % u64::from(band.w)) as u16;
            let head = ((self.t - at) * DROP_SPEED).floor() as i64;
            for d in 0..TAIL {
                let y = head - d;
                if y < 0 || y >= i64::from(band.h) {
                    continue;
                }
                let (text, style) = if d == 0 {
                    (ENVELOPE.to_string(), Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) })
                } else {
                    let ch = chars[(hash(n, y as u64, 12) % chars.len() as u64) as usize];
                    (ch.to_string(), pal.shade(BRIGHT_WHITE, d as f32 / TAIL as f32 * 0.8))
                };
                g.set(band.x + c, band.y + y as u16, Cell::new(&text, false, style));
            }
        }
    }

    /// The equalizer's groups, each a label and its bars' levels (0 to 1),
    /// and how many of them are the first ones: a bar a core, memory, the
    /// disks read and written, the network in and out, the temperature, a
    /// bar for each pane's output. Then the ones drawn only where they fit
    /// beside those: messages, the CPU's watts, the battery's, swap, context
    /// switches and interrupts. A figure the machine does not give is left
    /// out.
    fn groups(&self) -> (Vec<Group>, usize) {
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

    fn step_bars(&mut self, dt: f64) {
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

    fn draw_bars(&self, g: &mut Grid, band: Rect, pal: &Palette) {
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
            switches: Some(48_000.0),
            interrupts: Some(9_000.0),
            processes: Some(312),
            threads: Some(4_300),
            swap: Some(0.12),
            packets: Some((2_500.0, 600.0)),
            cpu_power: Some(67.0),
            battery_draw: Some(20.0),
        }
    }

    /// `steps` frames of `dt` with the same input, from a new animation.
    fn run(kind: Kind, load: &Load, steps: usize, dt: f64) -> Anim {
        let mut a = Anim::new(kind, 7);
        for i in 0..steps {
            a.step(dt, Input { load: load.clone(), outs: vec![i as u64 * 4000], messages: 0 });
        }
        a
    }

    /// Like `run`, with a message delivered at each step in `at`.
    fn run_with_messages(kind: Kind, load: &Load, steps: usize, dt: f64, at: &[usize]) -> Anim {
        let mut a = Anim::new(kind, 7);
        for i in 0..steps {
            let messages = at.iter().filter(|&&s| s <= i).count() as u64;
            a.step(dt, Input { load: load.clone(), outs: vec![0], messages });
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
            switches: Some(f64::NAN),
            interrupts: Some(f64::INFINITY),
            processes: Some(u64::MAX),
            threads: Some(0),
            swap: Some(-3.0),
            packets: Some((f64::NAN, -1.0)),
            cpu_power: Some(f32::INFINITY),
            battery_draw: Some(f32::NAN),
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
        // Each label is given room for its figures at their widest: the six
        // first groups need 66 columns here, and in 60 the last is left out.
        let busy_bars = run(Kind::Bars, &busy(), 60, 0.066);
        let mut g = Grid::new(70, 9);
        busy_bars.draw(&mut g, Rect { x: 0, y: 0, w: 66, h: 9 }, &pal);
        let t = text(&g);
        for label in ["CPU 63%", "MEM 41%", "DISK 12M", "NET ↓1.2M ↑80K", "61°C", "OUT "] {
            assert!(t.contains(label), "{label}: {t}");
        }
        let t = text(&drawn(&busy_bars, band(), &pal));
        assert!(t.contains("61°C") && !t.contains("OUT "), "{t}");
        // A bar that falls leaves its peak's mark over it a while; an idle
        // one has none, on the bottom row or anywhere.
        let full = Load { cores: vec![1.0], cpu: 1.0, ..Load::default() };
        let mut a = run(Kind::Bars, &full, 30, 0.066);
        let idle = Load { cores: vec![0.0], ..Load::default() };
        for _ in 0..5 {
            a.step(0.066, Input { load: idle.clone(), outs: vec![0], messages: 0 });
        }
        let g = drawn(&a, band(), &pal);
        assert!((0..g.cols).any(|x| g.get(x, band().y).text() == "▔"), "{}", text(&g));
        let g = drawn(&run(Kind::Bars, &idle, 30, 0.066), band(), &pal);
        assert!(!text(&g).contains('▔'), "{}", text(&g));
        // 32 cores in 40 columns: taken four together, 8 bars with a gap
        // between each (never a solid block), the other groups still there.
        let many = Load { cores: vec![0.5; 32], cpu: 0.5, memory: Some(0.3), ..Load::default() };
        let a = run(Kind::Bars, &many, 30, 0.066);
        let narrow = Rect { x: 0, y: 0, w: 40, h: 6 };
        let g = drawn(&a, narrow, &pal);
        let t = text(&g);
        assert!(t.contains("CPU 50%") && t.contains("MEM 30%") && t.contains("OUT"), "{t}");
        let bottom: String = (0..g.cols).map(|x| g.get(x, 4).text()).collect();
        let runs: Vec<usize> = bottom.split(' ').filter(|r| !r.is_empty()).map(|r| r.chars().count()).collect();
        assert_eq!(runs, vec![1; 10], "8 core bars, memory, output, each alone (no message: no bar): {t}");
        assert!(!t.contains("MSG"), "room for the messages' label at its widest (43 columns): none: {t}");
        // Cores taken together stand at their average, not their highest:
        // half of them full and half idle make bars half way up.
        let half = Load { cores: (0..32).map(|i| (i % 2) as f32).collect(), cpu: 0.5, ..many.clone() };
        let g = drawn(&run(Kind::Bars, &half, 30, 0.066), Rect { h: 10, ..narrow }, &pal);
        let rows: Vec<String> = (0..9).map(|y| (0..22).map(|x| g.get(x, y).text()).collect()).collect();
        assert!(rows[0].trim().is_empty() && rows[1].trim().is_empty(), "no core bar near the top: {rows:#?}");
        assert!(rows[8].contains('█') && rows[5].contains('█'), "half way up: {rows:#?}");
    }

    /// The groups added after the first ones (messages, watts, the battery,
    /// swap, context switches) are drawn where there is room for them
    /// beside those, in the same look; where there is not, the frame is
    /// the one a machine without those figures gives, cell for cell.
    #[test]
    fn the_added_bars_take_only_the_room_left() {
        let pal = Palette::default();
        let old = Load {
            switches: None,
            interrupts: None,
            processes: None,
            threads: None,
            swap: None,
            packets: None,
            cpu_power: None,
            battery_draw: None,
            ..busy()
        };
        let wide = Rect { x: 0, y: 0, w: 150, h: 9 };
        let mut g = Grid::new(150, 9);
        run(Kind::Bars, &busy(), 60, 0.066).draw(&mut g, wide, &pal);
        let t = text(&g);
        for label in ["CPU 63%", "OUT ", "67W", "PWR 20W", "SWAP 12%", "CTX 48k", "MSG 0"] {
            assert!(t.contains(label), "{label}: {t}");
        }
        let at = |load: &Load, w: u16| {
            let mut g = Grid::new(150, 9);
            run(Kind::Bars, load, 60, 0.066).draw(&mut g, Rect { w, ..wide }, &pal);
            g
        };
        let (mut same, mut more) = (0, 0);
        for w in (20..=150).step_by(2) {
            let (a, b) = (at(&old, w), at(&busy(), w));
            if a == b {
                same += 1;
            } else {
                // Where they differ, the old frame's every group is still
                // there, as many bars wide; the new one only adds to it.
                more += 1;
                let (ta, tb) = (text(&a), text(&b));
                for label in ["CPU 63%", "MEM 41%", "DISK 12M", "NET ↓1.2M ↑80K", "61°C", "OUT "] {
                    assert_eq!(ta.contains(label), tb.contains(label), "{w} {label}: {ta}\n{tb}");
                }
                let lit = |g: &Grid| (0..g.cols).filter(|&x| g.get(x, 7).text() != " ").count();
                assert!(lit(&b) >= lit(&a), "{w}: {ta}\n{tb}");
            }
        }
        assert!(same > 0 && more > 0, "{same} the same, {more} with more");
    }

    /// The layout holds still while the figures change: the same machine
    /// with every figure short ("0", "1%") and every figure at its widest
    /// draws its groups and traces in the same columns, the same of them.
    #[test]
    fn the_layout_holds_while_the_figures_change() {
        let pal = Palette::default();
        let short = Load {
            cores: vec![0.01; 8],
            cpu: 0.01,
            memory: Some(0.01),
            disk: Some((0.0, 0.0)),
            net: Some((0.0, 0.0)),
            temp: Some(5.0),
            switches: Some(1.0),
            interrupts: Some(1.0),
            processes: Some(9),
            threads: Some(9),
            swap: Some(0.01),
            packets: Some((0.0, 0.0)),
            cpu_power: Some(1.0),
            battery_draw: Some(1.0),
        };
        let wide = Load {
            cores: vec![1.0; 8],
            cpu: 1.0,
            memory: Some(1.0),
            disk: Some((999.0 * 1024.0, 999.0 * 1024.0)),
            net: Some((9.9 * 1024.0 * 1024.0, 999.0 * 1024.0)),
            temp: Some(99.0),
            switches: Some(999_000.0),
            interrupts: Some(999_000.0),
            processes: Some(9999),
            threads: Some(999_000),
            swap: Some(1.0),
            packets: Some((500_000.0, 499_000.0)),
            cpu_power: Some(999.0),
            battery_draw: Some(999.0),
        };
        // Where the label column starts in pulse (the first column of the
        // heart's label), and which groups the bars show, by their words.
        let pulse_at = |load: &Load, w: u16| {
            let mut g = Grid::new(140, 14);
            run(Kind::Pulse, load, 40, 0.05).draw(&mut g, Rect { x: 0, y: 0, w, h: 13 }, &pal);
            (0..g.rows).find_map(|y| (0..g.cols).find(|&x| g.get(x, y).text() == "♥"))
        };
        // The groups' words, and the columns of the cores' bars (both
        // machines' cores half busy).
        let bars = |load: &Load, w: u16| {
            let load = Load { cores: vec![0.5; 8], cpu: 0.5, ..load.clone() };
            let mut g = Grid::new(140, 9);
            run(Kind::Bars, &load, 40, 0.066).draw(&mut g, Rect { x: 0, y: 0, w, h: 9 }, &pal);
            // Each label: its word, and the column of its middle.
            let row: Vec<String> = (0..g.cols).map(|x| g.get(x, 8).text().to_string()).collect();
            let (mut labels, mut x) = (Vec::new(), 0);
            while x < row.len() {
                if row[x] == " " {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && !(row[x] == " " && row.get(x + 1).is_none_or(|c| c == " ")) {
                    x += 1;
                }
                let word: String = row[start..x].concat().chars().take_while(|c| c.is_alphabetic()).collect();
                labels.push((word, (start + x) / 2));
            }
            // The cores' group ends where memory's label begins.
            let mem = (0..g.cols).find(|&x| g.get(x, 8).text() == "M").unwrap_or(g.cols);
            let cores: Vec<u16> = (0..mem).filter(|&x| g.get(x, 7).text() != " ").collect();
            (labels, cores)
        };
        let mut checked = 0;
        for w in (20..=140).step_by(3) {
            assert_eq!(pulse_at(&short, w), pulse_at(&wide, w), "pulse at {w}");
            let ((a, ca), (b, cb)) = (bars(&short, w), bars(&wide, w));
            assert_eq!(ca, cb, "the cores' bars at {w}");
            assert_eq!(a.len(), b.len(), "bars at {w}: {a:?} {b:?}");
            for ((wa, xa), (wb, xb)) in a.iter().zip(&b) {
                assert!(wa == wb && xa.abs_diff(*xb) <= 1, "bars at {w}: {a:?} {b:?}");
            }
            checked += 1;
        }
        assert_eq!(checked, 41);
    }

    /// The monitor as it was where the new figures have no room: the
    /// heart, network and disk keep their rows (the system's wave only where
    /// a row is left for it), and their short labels where the full ones do
    /// not fit. Every size: a frame with neither the system's wave nor a
    /// full label is the one a machine without those figures gives.
    #[test]
    fn the_monitor_keeps_its_rows_and_labels_where_the_new_figures_do_not_fit() {
        let pal = Palette::default();
        let old = Load {
            switches: None,
            interrupts: None,
            processes: None,
            threads: None,
            swap: None,
            packets: None,
            cpu_power: None,
            battery_draw: None,
            ..busy()
        };
        let (a, b) = (run(Kind::Pulse, &old, 90, 0.05), run(Kind::Pulse, &busy(), 90, 0.05));
        let (mut same, mut short, mut full) = (0, 0, 0);
        for w in (20..=130).step_by(2) {
            for h in 3..=13 {
                let r = Rect { x: 0, y: 0, w, h };
                let (mut ga, mut gb) = (Grid::new(132, 14), Grid::new(132, 14));
                a.draw(&mut ga, r, &pal);
                b.draw(&mut gb, r, &pal);
                let tb = text(&gb);
                // The system's wave has a row of its own: a fourth trace
                // in rows of 3 (9 rows and more) or of 2.
                let wave = h / if h >= 9 { 3 } else { 2 } >= 4;
                if wave || tb.contains("67W") {
                    full += 1;
                    continue;
                }
                assert_eq!(ga, gb, "{w}x{h}:\n{}\n{tb}", text(&ga));
                same += 1;
                short += usize::from(tb.contains("CPU 63%  61°C"));
            }
        }
        assert!(same > 0 && short > 0 && full > 0, "{same} the same ({short} with short labels), {full} with more");
        // 9 rows: three traces of 3 rows each, as before; 12: the system's
        // wave under them.
        let rows_of = |h: u16| {
            let mut g = Grid::new(132, 14);
            b.draw(&mut g, Rect { x: 0, y: 0, w: 120, h }, &pal);
            text(&g).contains("SYS")
        };
        assert!(!rows_of(9) && !rows_of(11) && rows_of(12));
    }

    /// A message delivered: the heart beats early (more beats in the same
    /// time) and the count shows by it; a drop headed by an envelope falls
    /// through the rain; the messages' bar fills and falls back. Messages
    /// delivered before the clock came are not counted.
    #[test]
    fn a_message_shows_in_each_kind() {
        let pal = Palette::default();
        let beats = |a: &Anim| {
            let s: Vec<f32> = a.samples.iter().map(|s| s[0]).collect();
            s.windows(2).filter(|w| w[0] < 0.5 && w[1] >= 0.5).count()
        };
        let quiet = run_with_messages(Kind::Pulse, &Load::default(), 120, 0.05, &[]);
        let told = run_with_messages(Kind::Pulse, &Load::default(), 120, 0.05, &[20, 45, 70, 95]);
        assert!(beats(&told) > beats(&quiet), "{} {}", beats(&quiet), beats(&told));
        assert_eq!(told.messages, 4);
        assert!(text(&drawn(&told, band(), &pal)).contains("✉ 4"));
        assert!(!text(&drawn(&quiet, band(), &pal)).contains('✉'));
        // Five came before the clock: none counted.
        let mut a = Anim::new(Kind::Pulse, 7);
        for i in 0..10 {
            a.step(0.05, Input { messages: 5 + u64::from(i >= 5), ..Input::default() });
        }
        assert_eq!(a.messages, 1);
        // The rain: an envelope at the head of the drop, 4 steps (0.32 s)
        // after.
        let rain = run_with_messages(Kind::Matrix, &Load::default(), 60, 0.08, &[55]);
        let g = drawn(&rain, band(), &pal);
        let envelopes: Vec<(u16, u16)> = (0..g.rows)
            .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
            .filter(|&(x, y)| g.get(x, y).text() == "✉")
            .collect();
        assert_eq!(envelopes.len(), 1, "{}", text(&g));
        assert_eq!(envelopes[0].1, band().y + 5, "18 rows a second, 0.32 s");
        assert!(g.get(envelopes[0].0, envelopes[0].1).style.bold);
        assert!(!text(&drawn(&run(Kind::Matrix, &Load::default(), 60, 0.08), band(), &pal)).contains('✉'));
        // Gone below the band, and let go a while after.
        let later = run_with_messages(Kind::Matrix, &Load::default(), 400, 0.08, &[55]);
        assert!(!text(&drawn(&later, band(), &pal)).contains('✉') && later.drops.is_empty());
        // A thousand at once: all counted, a few drops and early beats, not
        // a thousand; drawn in every kind.
        for kind in Kind::ALL {
            let mut a = Anim::new(kind, 7);
            for i in 0..40u64 {
                a.step(0.05, Input { messages: if i >= 20 { 1000 } else { 0 }, ..Input::default() });
            }
            assert_eq!(a.messages, 1000);
            assert!(a.drops.len() <= DROPS && a.early_beats <= 4, "{} {}", a.drops.len(), a.early_beats);
            let t = text(&drawn(&a, band(), &pal));
            assert!(t.contains(if kind == Kind::Bars { "MSG 1000" } else { "" }), "{t}");
        }
        assert!(text(&drawn(&run(Kind::Pulse, &busy(), 30, 0.05), band(), &pal)).contains("♥"));
        // No pane's output given (the pane's window not found): one bar,
        // at nothing.
        let mut a = Anim::new(Kind::Bars, 7);
        a.step(0.05, Input::default());
        a.step(0.05, Input::default());
        let (groups, _) = a.groups();
        assert!(groups.iter().any(|g| g.label == "OUT 0" && g.levels == vec![0.0]), "{groups:?}");
        // The bar: full at once, under a tenth 2 s on.
        let bar = |steps: usize| run_with_messages(Kind::Bars, &Load::default(), steps, 0.05, &[20]).message_level;
        assert!(bar(21) > 0.9 && bar(60) < 0.1, "{} {}", bar(21), bar(60));
        assert!(
            text(&drawn(&run_with_messages(Kind::Bars, &Load::default(), 30, 0.05, &[20]), band(), &pal))
                .contains("MSG 1")
        );
    }

    /// The system's wave: more swings the more the CPUs switch, taller the
    /// more interrupts come; its label says the switches, processes and
    /// threads. The rain's stray characters come with the interrupts.
    #[test]
    fn the_system_wave_and_the_strays_follow_switches_and_interrupts() {
        let pal = Palette::default();
        let sys = |switches: f64, interrupts: f64| Load {
            switches: Some(switches),
            interrupts: Some(interrupts),
            ..Load::default()
        };
        let wave = |load: &Load| {
            let a = run(Kind::Pulse, load, 120, 0.05);
            let s: Vec<f32> = a.samples.iter().map(|s| s[3]).collect();
            let turns = s.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0).count();
            (turns, s.iter().map(|v| v.abs()).fold(0.0, f32::max))
        };
        let (calm, low) = wave(&sys(1e3, 1e3));
        let (busy_turns, _) = wave(&sys(1e6, 1e3));
        let (_, high) = wave(&sys(1e3, 1e6));
        assert!(busy_turns > calm * 3, "{calm} {busy_turns}");
        assert!(high > low * 3.0, "{low} {high}");
        let t = text(&drawn(&run(Kind::Pulse, &busy(), 60, 0.05), Rect { x: 0, y: 0, w: 64, h: 14 }, &pal));
        assert!(t.contains("SYS 48k/s  312 proc 4.3k thr"), "{t}");
        assert!(t.contains("CPU 63% 67W") && t.contains("PWR 20W") && t.contains("NET ↓1.2M ↑80K  3.1k pkt/s"), "{t}");
        assert!(!text(&drawn(&run(Kind::Pulse, &Load::default(), 60, 0.05), band(), &pal)).contains("SYS"));
        let strays = |load: &Load| {
            let g = drawn(&run(Kind::Matrix, load, 60, 0.08), band(), &pal);
            (0..g.rows)
                .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
                .filter(|&(x, y)| g.get(x, y).style.fg == Color::Idx(MAGENTA))
                .count()
        };
        assert_eq!(strays(&sys(1e3, 1e3)), 0);
        assert!(strays(&sys(1e3, 1e6)) > 10, "{}", strays(&sys(1e3, 1e6)));
    }

    /// The output bars: one a pane of the window, each at its own rate,
    /// the label their sum; a pane more or fewer starts them again.
    #[test]
    fn each_pane_has_its_output_bar() {
        let mut a = Anim::new(Kind::Bars, 7);
        for i in 0..40u64 {
            a.step(0.05, Input { outs: vec![100, i * 50_000, 0], ..Input::default() });
        }
        assert_eq!(a.out_rates.len(), 3);
        assert!(a.out_rates[0] == 0.0 && a.out_rates[2] == 0.0, "{:?}", a.out_rates);
        assert!((a.out_rates[1] - 1e6).abs() < 1e4, "1 MB a second: {:?}", a.out_rates);
        let (groups, first) = a.groups();
        let out = groups.iter().find(|g| g.label.starts_with("OUT")).unwrap();
        assert_eq!(out.label, format!("OUT {}", rate_text(a.out_rates[1])));
        assert!(out.label.starts_with("OUT 9") && out.label.ends_with('K'), "{}", out.label);
        assert!(out.levels.len() == 3 && out.levels[1] > 0.8 && out.levels[0] == 0.0, "{out:?}");
        assert_eq!(groups[first - 1].label, out.label, "the output is the last of the first groups");
        a.step(0.05, Input { outs: vec![100, 2_000_000], ..Input::default() });
        assert_eq!(a.out_rates, vec![0.0, 0.0], "a pane fewer: from nothing");
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
