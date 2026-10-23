use super::*;

/// `dino`: how far the dinosaur has run, columns; how far the clouds have
/// drifted.
#[derive(Clone, Debug, Default)]
pub(super) struct Run {
    pub(super) ran: f64,
    drift: f64,
}

/// The dinosaur facing right: its head, its body and tail; its legs, two
/// strides and in the air.
const DINO: [&str; 2] = ["   ▄█▀▀", "▀▄██▛  "];
const LEGS: [&str; 3] = ["  ▌ ▝  ", "  ▘ ▌  ", "  ▘ ▘  "];
pub(super) const DINO_W: i64 = 7;
/// Where it runs, columns from the left.
pub(super) const DINO_X: i64 = 3;
/// A cactus, tall (two rows) or small (its foot alone).
const CACTUS: [&str; 2] = [" ╻ ", "┗╋┛"];
pub(super) const CACTUS_W: i64 = 3;
/// The ground in slots this wide, a cactus at most in each.
const SLOT: f64 = 16.0;
/// How high it jumps, rows, at the top of the arc; how far either side of
/// a cactus the arc reaches, columns.
const JUMP: f64 = 3.0;
const REACH: f64 = 7.0;
/// How fast a message's envelope falls to it, rows a second; how long it
/// is glad after, seconds.
const ENVELOPE_FALL: f64 = 8.0;
const GLAD: f64 = 1.0;

/// Columns a second: 6 while the cores rest, 26 while all are busy.
fn speed(load: &Load) -> f64 {
    6.0 + 20.0 * f64::from(share(load.cpu))
}

impl Anim {
    pub(super) fn step_dino(&mut self, dt: f64) {
        let load = &self.input.load;
        self.run.ran += dt * speed(load);
        self.run.drift += dt * (0.5 + 4.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
    }

    /// The cactus in slot `k`, if it has one: where it stands on the
    /// ground, columns from the start, and whether it is tall. The more
    /// panes the window has, the more slots have one; none near the start.
    pub(super) fn cactus(&self, k: i64) -> Option<(f64, bool)> {
        if k < 1 {
            return None;
        }
        let panes = self.out_rates.len().max(1) as f32;
        let h = hash(self.seed, k as u64, 151);
        (unit(h) < (0.3 + 0.15 * panes).min(0.95)).then_some((k as f64 * SLOT + (h % 4) as f64, h & 0x10 != 0))
    }

    /// How high the dinosaur is, rows: on the arc over the nearest cactus,
    /// two rows or more while one is under it.
    pub(super) fn lift(&self) -> i64 {
        let ran = self.run.ran;
        let first = ((ran - REACH - SLOT) / SLOT).floor() as i64;
        let mut lift: f64 = 0.0;
        for k in first..first + 4 {
            if let Some((at, _)) = self.cactus(k) {
                let d = (at - ran).round() + CACTUS_W as f64 / 2.0 - (DINO_X as f64 + DINO_W as f64 / 2.0);
                lift = lift.max(JUMP * (1.0 - (d / REACH).powi(2)));
            }
        }
        lift.round() as i64
    }

    /// A desert running past (faster the busier the cores), cacti on it
    /// (more of them the more panes), clouds drifting over (with the
    /// network); the dinosaur jumps each cactus. A message falls to it as
    /// an envelope, and it is glad a moment. Its score, the distance, top
    /// right.
    pub(super) fn draw_dino(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let ran = self.run.ran;
        // The row under its feet: the ground's, or below the band when
        // there are not rows enough for it.
        let ground = h >= 4;
        let gy = if ground { h - 1 } else { h };
        if ground {
            for x in 0..w {
                let pebble = hash(self.seed, (x as f64 + ran).floor() as i64 as u64, 150).is_multiple_of(9);
                put(g, band, x, gy, if pebble { "╌" } else { "─" }, pal.shade(YELLOW, 0.2));
            }
        }
        // Clouds, in the sky above it, slower than the ground.
        if gy >= 5 {
            let span = w as f64 + 12.0;
            for c in 0..(w / 20).max(1) {
                let hc = hash(self.seed, c as u64, 152);
                let x = (f64::from(unit(hc)) * span - self.run.drift - ran * 0.15).rem_euclid(span) - 6.0;
                let y = (hc >> 8) as i64 % (gy - 4);
                for (i, ch) in ["(", "‿", "‿", ")"].iter().enumerate() {
                    put(g, band, x as i64 + i as i64, y, ch, pal.shade(WHITE, 0.5));
                }
            }
        }
        let first = (ran / SLOT).floor() as i64 - 1;
        for k in first..=first + w / SLOT as i64 + 2 {
            let Some((at, tall)) = self.cactus(k) else { continue };
            let x = (at - ran).round() as i64;
            for (r, row) in CACTUS.iter().enumerate().skip(usize::from(!tall)) {
                for (i, ch) in row.chars().enumerate().filter(|(_, c)| *c != ' ') {
                    put(g, band, x + i as i64, gy - 2 + r as i64, ch.encode_utf8(&mut [0; 4]), pal.shade(GREEN, 0.0));
                }
            }
        }
        let lift = self.lift();
        let top = gy - 3 - lift;
        let glad = self.drops.iter().any(|&(_, at)| {
            let age = self.t - at;
            age * ENVELOPE_FALL >= top as f64 && age * ENVELOPE_FALL < top as f64 + GLAD * ENVELOPE_FALL
        });
        let legs = if lift > 0 { LEGS[2] } else { LEGS[(ran / 2.0).floor() as i64 as usize % 2] };
        let body = pal.shade(BRIGHT_GREEN, 0.0);
        for (r, row) in DINO.iter().chain([&legs]).enumerate() {
            for (i, ch) in row.chars().enumerate().filter(|(_, c)| *c != ' ') {
                put(g, band, DINO_X + i as i64, top + r as i64, ch.encode_utf8(&mut [0; 4]), body);
            }
        }
        if glad {
            put(g, band, DINO_X + 5, top - 1, "♥", Style { bold: true, ..pal.shade(RED, 0.0) });
        }
        // A message's envelope, falling to its head, gone once it gets there.
        for &(_, at) in &self.drops {
            let y = ((self.t - at) * ENVELOPE_FALL).floor() as i64;
            if y < top {
                put(g, band, DINO_X + 5, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
        let score = format!("{:05}", (ran / 4.0) as u64 % 100_000);
        if w >= 30 && h >= 4 {
            let x = band.x + band.w - score.len() as u16 - 1;
            g.put_str(x, band.y, &score, pal.shade(WHITE, 0.0), score.len() as u16);
            self.keep(x, band.y, score.len());
        }
    }
}
