//! What moves under the clock (`clock-mode-animation`): a monitor's traces
//! (`pulse`), falling characters (`matrix`), an equalizer (`bars`), flames
//! (`fire`), a flight through stars (`starfield`), a shark hunting fish
//! (`sharks`), a tree growing (`bonsai`), a moonlit sea (`sea`), a cat
//! asleep (`cat`), a dinosaur running a desert (`dino`), penguins on the
//! ice (`penguins`), a neon grid under a setting sun (`synthwave`) or
//! fireworks over a city (`fireworks`), each moved by what the machine is
//! doing (its cores,
//! memory, disks, network, the CPU's temperature and power, context
//! switches, interrupts, swap, processes) and by what keepane does (what
//! the window's panes print, the messages it delivers).
//!
//! `clock-mode-look` draws any of them in a set of characters instead of
//! its own (`look`): each cell the one with as much ink as it had.
//!
//! It is drawn in the 16 colours only, so a theme's `pane-colours` colour
//! it (the terminal's own when the theme sets none); a tail fades into the
//! theme's background where both are known, else it is drawn dim.
//!
//! An `Anim` moves only by `step`, by the time and the input it is given:
//! the same steps give the same frames. The time of day comes in the input
//! (`bonsai` and `sea` go by it); nothing reads the clock.

mod bars;
mod bonsai;
mod cat;
mod dino;
mod fire;
mod fireworks;
mod look;
mod matrix;
mod penguins;
mod pulse;
mod sea;
mod sharks;
mod starfield;
mod synthwave;

use super::layout::Rect;
use super::render::{Cell, Grid, Style};
use crate::format::human_count;
use crate::sysinfo::Load;
use bars::Bar;
pub use look::Look;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;
use vt100::Color;

/// The kinds, in the order `cycle` takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Pulse,
    Matrix,
    Bars,
    Fire,
    Starfield,
    Sharks,
    Bonsai,
    Sea,
    Cat,
    Dino,
    Penguins,
    Synthwave,
    Fireworks,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Pulse,
        Kind::Matrix,
        Kind::Bars,
        Kind::Fire,
        Kind::Starfield,
        Kind::Sharks,
        Kind::Bonsai,
        Kind::Sea,
        Kind::Cat,
        Kind::Dino,
        Kind::Penguins,
        Kind::Synthwave,
        Kind::Fireworks,
    ];

    /// Its name, as `clock-mode-animation` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pulse => "pulse",
            Kind::Matrix => "matrix",
            Kind::Bars => "bars",
            Kind::Fire => "fire",
            Kind::Starfield => "starfield",
            Kind::Sharks => "sharks",
            Kind::Bonsai => "bonsai",
            Kind::Sea => "sea",
            Kind::Cat => "cat",
            Kind::Dino => "dino",
            Kind::Penguins => "penguins",
            Kind::Synthwave => "synthwave",
            Kind::Fireworks => "fireworks",
        }
    }

    /// The kind of that name.
    pub fn named(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// A kind at random by `r`, never `previous`: a clock put on again
    /// shows something else.
    pub fn random(previous: Option<Kind>, r: u64) -> Kind {
        let n = Kind::ALL.len() as u64;
        let r = hash(r, 0x6b, 0);
        match previous.and_then(|p| Kind::ALL.iter().position(|&k| k == p)) {
            Some(i) => Kind::ALL[(i + 1 + (r % (n - 1)) as usize) % Kind::ALL.len()],
            None => Kind::ALL[(r % n) as usize],
        }
    }

    /// The kind after `from` in `ALL` (`next`), or the one before it, round
    /// the end; from none, the first or the last.
    pub fn turned(from: Option<Kind>, next: bool) -> Kind {
        let n = Kind::ALL.len();
        let i = match (from.and_then(|f| Kind::ALL.iter().position(|&k| k == f)), next) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        Kind::ALL[i]
    }

    /// The time between two frames: enough for each to look smooth, no more.
    pub fn frame(self) -> Duration {
        Duration::from_millis(match self {
            Kind::Pulse => 50,
            Kind::Matrix | Kind::Sea => 80,
            Kind::Bars
            | Kind::Fire
            | Kind::Starfield
            | Kind::Sharks
            | Kind::Dino
            | Kind::Synthwave
            | Kind::Fireworks => 66,
            Kind::Bonsai | Kind::Cat | Kind::Penguins => 100,
        })
    }
}

/// The least room an animation is drawn in; less, and nothing is.
pub const MIN_W: u16 = 20;
pub const MIN_H: u16 = 3;

/// What moves it: the machine's load; how many bytes each pane of the
/// clock's window has printed so far, in the window's order; how many
/// messages keepane has delivered so far; the seconds since midnight, the
/// clock's time.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub load: Load,
    pub outs: Vec<u64>,
    pub messages: u64,
    pub day: f64,
}

/// The seconds since midnight at `t` (`Input::day`).
pub fn day_seconds(t: &impl chrono::Timelike) -> f64 {
    f64::from(t.num_seconds_from_midnight()) + f64::from(t.nanosecond() % 1_000_000_000) / 1e9
}

/// The theme's colours: its 16 (none: the terminal's own) and the panes'
/// background; and the characters to draw in (`clock-mode-look`).
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub colours: Vec<Color>,
    pub bg: Color,
    pub look: Look,
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
const BRIGHT_GREEN: u8 = 10;
const YELLOW: u8 = 3;
const BLUE: u8 = 4;
const MAGENTA: u8 = 5;
const CYAN: u8 = 6;
const WHITE: u8 = 7;
const BRIGHT_YELLOW: u8 = 11;
const BRIGHT_BLUE: u8 = 12;
const BRIGHT_WHITE: u8 = 15;

/// A message's mark.
const ENVELOPE: &str = "✉";

/// The colours of the fish (`sharks`) and the boats (`sea`): each pane
/// takes the next.
const FISH_COLOURS: [u8; 6] = [YELLOW, CYAN, MAGENTA, GREEN, BRIGHT_BLUE, RED];

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

/// Smooth noise from 0 to 1 over the plane, one surface per `key`.
fn noise2(key: u64, x: f64, y: f64) -> f32 {
    let (i, j) = (x.floor(), y.floor());
    let (fx, fy) = ((x - i) as f32, (y - j) as f32);
    let corner = |di: i64, dj: i64| unit(hash(key, (i as i64 + di) as u64, ((j as i64 + dj) as u64) ^ 0x5bd1_e995));
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// A share (0 to 1) as given, or nothing for one out of range.
fn share(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// `text` (one column) at `x`, `y` of `band`, where that is inside it.
fn put(g: &mut Grid, band: Rect, x: i64, y: i64, text: &str, style: Style) {
    if (0..i64::from(band.w)).contains(&x) && (0..i64::from(band.h)).contains(&y) {
        g.set(band.x + x as u16, band.y + y as u16, Cell::new(text, false, style));
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
    /// Each message's mark, the last few (which message, when it came:
    /// `matrix`'s drop, `fire`'s rising envelope, `dino`'s falling one…), and
    /// how many of them came with the last step.
    drops: VecDeque<(u64, f64)>,
    arrived: usize,
    /// `matrix`: how far the rain has fallen, how often the characters
    /// have changed.
    travel: f64,
    flicker: f64,
    /// `bars`: one a level `levels` gives.
    bars: Vec<Bar>,
    fire: fire::Fire,
    stars: starfield::Stars,
    sea: sharks::Sea,
    tree: bonsai::Tree,
    night: sea::Night,
    nap: cat::Nap,
    run: dino::Run,
    ice: penguins::Ice,
    drive: synthwave::Drive,
    show: fireworks::Show,
    /// Where the last frame drew text a look leaves as it is.
    kept: RefCell<Vec<(u16, u16, u16)>>,
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
            drops: VecDeque::new(),
            arrived: 0,
            travel: 0.0,
            flicker: 0.0,
            bars: Vec::new(),
            fire: Default::default(),
            stars: Default::default(),
            sea: Default::default(),
            tree: Default::default(),
            night: Default::default(),
            nap: Default::default(),
            run: Default::default(),
            ice: Default::default(),
            drive: Default::default(),
            show: Default::default(),
            kept: RefCell::default(),
        }
    }

    /// Move on by `dt` seconds with `input`. A long gap (a machine that
    /// slept) moves it half a second, not the whole gap; no time (NaN),
    /// not at all.
    pub fn step(&mut self, dt: f64, input: Input) {
        let dt = if dt.is_nan() { 0.0 } else { dt.clamp(0.0, 0.5) };
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
        self.arrived = new.min(DROPS as u64) as usize;
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
            Kind::Fire => self.step_fire(dt),
            Kind::Starfield => self.step_starfield(dt),
            Kind::Sharks => self.step_sharks(dt),
            Kind::Bonsai => self.step_bonsai(dt),
            Kind::Sea => self.step_sea(dt),
            Kind::Cat => self.step_cat(dt),
            Kind::Dino => self.step_dino(dt),
            Kind::Penguins => self.step_penguins(dt),
            Kind::Synthwave => self.step_synthwave(dt),
            Kind::Fireworks => self.step_fireworks(dt),
        }
    }

    /// Draw it in `band`, when the band is big enough, in the palette's
    /// look; nothing outside it is touched.
    pub fn draw(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        if band.w < MIN_W || band.h < MIN_H {
            return;
        }
        self.kept.borrow_mut().clear();
        match self.kind {
            Kind::Pulse => self.draw_pulse(g, band, pal),
            Kind::Matrix => self.draw_matrix(g, band, pal),
            Kind::Bars => self.draw_bars(g, band, pal),
            Kind::Fire => self.draw_fire(g, band, pal),
            Kind::Starfield => self.draw_starfield(g, band, pal),
            Kind::Sharks => self.draw_sharks(g, band, pal),
            Kind::Bonsai => self.draw_bonsai(g, band, pal),
            Kind::Sea => self.draw_sea(g, band, pal),
            Kind::Cat => self.draw_cat(g, band, pal),
            Kind::Dino => self.draw_dino(g, band, pal),
            Kind::Penguins => self.draw_penguins(g, band, pal),
            Kind::Synthwave => self.draw_synthwave(g, band, pal),
            Kind::Fireworks => self.draw_fireworks(g, band, pal),
        }
        self.dress(g, band, pal);
    }
}

#[cfg(test)]
mod tests;
