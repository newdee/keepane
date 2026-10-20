use super::*;
use std::f64::consts::TAU;

/// `orbits`: where each planet is on its orbit (radians), and how far the
/// asteroid belt has turned.
#[derive(Clone, Debug, Default)]
pub(super) struct Sky {
    pub(super) angles: Vec<f64>,
    belt: f64,
}

/// The most planets: more cores than that are taken together.
const PLANETS: usize = 8;
const PLANET_COLOURS: [u8; 8] = [CYAN, YELLOW, MAGENTA, GREEN, BRIGHT_BLUE, RED, WHITE, BRIGHT_YELLOW];
/// How long a message's comet takes to swing past the sun, seconds.
const COMET: f64 = 4.0;

/// How busy each planet's cores are: one a core, up to `PLANETS`, else
/// the cores in even groups; the CPU alone when no core is given.
pub(super) fn planets(load: &Load) -> Vec<f32> {
    let cores: Vec<f32> =
        if load.cores.is_empty() { vec![share(load.cpu)] } else { load.cores.iter().map(|&c| share(c)).collect() };
    let group = cores.len().div_ceil(PLANETS);
    cores.chunks(group).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect()
}

/// Planet `i` of `n`'s orbit, a share of the sky's radius.
fn orbit(i: usize, n: usize) -> f64 {
    0.22 + 0.63 * (i + 1) as f64 / n as f64
}

impl Anim {
    pub(super) fn step_orbits(&mut self, dt: f64) {
        let load = &self.input.load;
        let levels = planets(load);
        let sky = &mut self.sky;
        if sky.angles.len() != levels.len() {
            sky.angles = (0..levels.len()).map(|i| f64::from(unit(hash(self.seed, i as u64, 81))) * TAU).collect();
        }
        // The busier its cores, the faster; the nearer the sun, faster too.
        for (i, (a, &level)) in sky.angles.iter_mut().zip(&levels).enumerate() {
            *a += dt * (0.5 + 1.2 * f64::from(level)) / orbit(i, levels.len()).sqrt();
        }
        sky.belt += dt * (0.05 + 0.8 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
    }

    /// A sun (yellow, red when the CPU is hot; bigger the more memory is
    /// used; flaring with the interrupts) and a planet a core on dotted
    /// orbits, each faster the busier the core; an asteroid belt round the
    /// outside, more of it and quicker the more the network carries; each
    /// message a comet with `✉` at its head swinging round the sun, its
    /// tail away from it.
    pub(super) fn draw_orbits(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (f64::from(band.w), f64::from(band.h));
        let (cx, cy) = (w / 2.0, h / 2.0);
        let ry = (h / 2.0 - 0.5).max(0.5);
        // Round-ish where there are rows, but never narrower than 14 columns:
        // in a low band the orbits flatten and the planets still go round.
        let rx = (w / 2.0 - 1.0).min((ry * 3.0).max(14.0)).max(1.0);
        let spot = |q: f64, a: f64| ((cx + rx * q * a.cos()).floor() as i64, (cy + ry * q * a.sin()).floor() as i64);
        let n = self.sky.angles.len();
        for i in 0..n {
            let q = orbit(i, n);
            let dots = (TAU * (rx + ry) * q / 8.0) as usize + 6;
            for k in 0..dots {
                let (x, y) = spot(q, k as f64 / dots as f64 * TAU);
                put(g, band, x, y, ".", pal.shade(WHITE, 0.6));
            }
        }
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        for i in 0..(net * 48.0).round() as u64 {
            let hs = hash(self.seed, i, 83);
            let a = f64::from(unit(hs)) * TAU + self.sky.belt * (0.5 + f64::from(unit(hash(hs, 1, 0))));
            let (x, y) = spot(0.93 + 0.07 * f64::from(unit(hash(hs, 2, 0))), a);
            put(g, band, x, y, ".", pal.shade(YELLOW, 0.35));
        }
        let (sx, sy) = (cx.floor() as i64, cy.floor() as i64);
        let sun = match heat(load) {
            RED => RED,
            YELLOW => YELLOW,
            _ => BRIGHT_YELLOW,
        };
        let memory = share(load.memory.unwrap_or(0.4));
        let flares = load.interrupts.map_or(0.0, count_level);
        let tick = (self.t * 6.0).floor() as u64;
        for k in 0..8u64 {
            if unit(hash(self.seed, tick, 84 + k)) < flares * 0.6 {
                let a = k as f64 / 8.0 * TAU;
                put(
                    g,
                    band,
                    sx + (a.cos() * 3.0).round() as i64,
                    sy + (a.sin() * 1.5).round() as i64,
                    "*",
                    pal.shade(sun, 0.3),
                );
            }
        }
        if memory > 0.4 {
            put(g, band, sx - 1, sy, "(", pal.shade(sun, 0.0));
            put(g, band, sx + 1, sy, ")", pal.shade(sun, 0.0));
        }
        if memory > 0.75 {
            put(g, band, sx - 2, sy, "-", pal.shade(sun, 0.2));
            put(g, band, sx + 2, sy, "-", pal.shade(sun, 0.2));
        }
        put(g, band, sx, sy, "@", Style { bold: true, ..pal.shade(sun, 0.0) });
        let levels = planets(load);
        for (i, &a) in self.sky.angles.iter().enumerate() {
            let level = levels.get(i).copied().unwrap_or(0.0);
            let (x, y) = spot(orbit(i, n), a);
            let style = Style { bold: level > 0.8, ..pal.shade(PLANET_COLOURS[i % PLANET_COLOURS.len()], 0.0) };
            put(g, band, x, y, if level >= 0.5 { "O" } else { "o" }, style);
        }
        // Each message: a comet from one side to the other, bent round the
        // sun (a curve with the sun for its middle point).
        for &(m, at) in &self.drops {
            let u = (self.t - at) / COMET;
            if !(0.0..1.0).contains(&u) {
                continue;
            }
            let hs = hash(self.seed, m, 85);
            let (x0, x1) = if hs & 1 == 0 { (-2.0, w + 2.0) } else { (w + 2.0, -2.0) };
            let (y0, y1) = (f64::from(unit(hash(hs, 1, 0))) * h, f64::from(unit(hash(hs, 2, 0))) * h);
            let px = (1.0 - u).powi(2) * x0 + 2.0 * u * (1.0 - u) * cx + u * u * x1;
            let py = (1.0 - u).powi(2) * y0 + 2.0 * u * (1.0 - u) * cy + u * u * y1;
            // The tail, away from the sun (in square units).
            let (dx, dy) = (px - cx, (py - cy) * 2.0);
            let l = (dx * dx + dy * dy).sqrt().max(1e-9);
            let (ux, uy) = (dx / l, dy / l);
            let mark = if ux.abs() > 2.0 * uy.abs() {
                "-"
            } else if uy.abs() > 2.0 * ux.abs() {
                "|"
            } else if ux * uy > 0.0 {
                "\\"
            } else {
                "/"
            };
            for k in 1..=5 {
                let d = k as f64 * 1.2;
                let (x, y) = ((px + ux * d).floor() as i64, (py + uy * d / 2.0).floor() as i64);
                put(g, band, x, y, mark, pal.shade(CYAN, k as f32 / 6.0));
            }
            put(
                g,
                band,
                px.floor() as i64,
                py.floor() as i64,
                ENVELOPE,
                Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) },
            );
        }
    }
}
