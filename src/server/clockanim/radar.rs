use super::*;
use std::f64::consts::TAU;

/// `radar`: where the sweep points (radians, clockwise from up), and how
/// far the contacts have drifted.
#[derive(Clone, Debug, Default)]
pub(super) struct Scope {
    pub(super) sweep: f64,
    drift: f64,
}

/// How long a message's contact takes from the rim to the middle, seconds.
const INBOUND: f64 = 3.0;

impl Anim {
    pub(super) fn step_radar(&mut self, dt: f64) {
        let load = &self.input.load;
        self.scope.sweep = (self.scope.sweep + dt * (1.5 + 3.0 * f64::from(share(load.cpu)))).rem_euclid(TAU);
        self.scope.drift += dt * (0.02 + 0.25 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
    }

    /// A round screen with two rings and a sweep turning faster the busier
    /// the cores, a glow behind it; a blip a pane, each in its colour, lit
    /// as the sweep passes and fading, bigger the more the pane prints;
    /// contacts drifting, more the more the network carries; clutter by the
    /// middle with the interrupts; each message an `✉` coming in from the
    /// rim; the sweep's bearing at the top left.
    pub(super) fn draw_radar(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (f64::from(band.w), f64::from(band.h));
        let (cx, cy) = (w / 2.0, h / 2.0);
        let ry = (h / 2.0 - 0.5).max(0.5);
        let rx = (ry * 2.0).min(w / 2.0 - 1.0).max(1.0);
        let spot = |q: f64, a: f64| ((cx + rx * q * a.sin()).floor() as i64, (cy - ry * q * a.cos()).floor() as i64);
        let sweep = self.scope.sweep;
        // How long since the sweep passed bearing `a`: 0 just now, 1 a turn.
        let since = |a: f64| (sweep - a).rem_euclid(TAU) / TAU;
        for (q, fade) in [(0.5, 0.65), (1.0, 0.45)] {
            let dots = (TAU * (rx + ry) * q / 3.0) as usize + 6;
            for k in 0..dots {
                let (x, y) = spot(q, k as f64 / dots as f64 * TAU);
                put(g, band, x, y, ".", pal.shade(GREEN, fade));
            }
        }
        let steps = ((rx.max(ry * 2.0)) * 2.0) as usize + 2;
        for back in (1..=5).rev() {
            let a = sweep - f64::from(back) * 0.1;
            for k in steps / 6..=steps {
                let (x, y) = spot(k as f64 / steps as f64, a);
                put(g, band, x, y, ".", pal.shade(GREEN, 0.3 + 0.1 * back as f32));
            }
        }
        let (sx, sy) = (sweep.sin(), -sweep.cos() / 2.0);
        let mark = if sx.abs() > 2.0 * sy.abs() {
            "-"
        } else if sy.abs() > 2.0 * sx.abs() {
            "|"
        } else if sx * sy > 0.0 {
            "\\"
        } else {
            "/"
        };
        for k in 1..=steps {
            let (x, y) = spot(k as f64 / steps as f64, sweep);
            put(g, band, x, y, mark, Style { bold: true, ..pal.shade(BRIGHT_GREEN, 0.0) });
        }
        put(g, band, cx.floor() as i64, cy.floor() as i64, "+", pal.shade(GREEN, 0.0));
        let tick = (self.t * 8.0).floor() as u64;
        let clutter = load.interrupts.map_or(0.0, count_level);
        for i in 0..(clutter * 10.0).round() as u64 {
            let hs = hash(self.seed, tick, 91 + i);
            let (x, y) = spot(0.08 + 0.2 * f64::from(unit(hs)), f64::from(unit(hash(hs, 1, 0))) * TAU);
            put(g, band, x, y, ".", pal.shade(GREEN, 0.4));
        }
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        for i in 0..(net * 14.0).round() as u64 {
            let hs = hash(self.seed, i, 92);
            let a = f64::from(unit(hs)) * TAU + self.scope.drift * (f64::from(unit(hash(hs, 1, 0))) - 0.5) * 2.0;
            let (x, y) = spot(0.2 + 0.75 * f64::from(unit(hash(hs, 2, 0))), a);
            put(g, band, x, y, "+", pal.shade(GREEN, (since(a) * 0.9) as f32));
        }
        let panes = self.out_rates.len().max(1);
        for i in 0..panes {
            let hs = hash(self.seed, i as u64, 93);
            let a = (i as f64 + 0.5) / panes as f64 * TAU + (f64::from(unit(hs)) - 0.5) * 0.5;
            let (x, y) = spot(0.3 + 0.6 * f64::from(unit(hash(hs, 1, 0))), a);
            let level = out_level(self.out_rates.get(i).copied().unwrap_or(0.0));
            let mark = if level >= 0.5 {
                "@"
            } else if level > 0.15 {
                "O"
            } else {
                "o"
            };
            let fade = (since(a) * 0.85 + if level > 0.0 { 0.0 } else { 0.1 }) as f32;
            let colour = aquarium::FISH_COLOURS[i % aquarium::FISH_COLOURS.len()];
            put(g, band, x, y, mark, Style { bold: since(a) < 0.1, ..pal.shade(colour, fade) });
        }
        for &(n, at) in &self.drops {
            let age = self.t - at;
            if (0.0..INBOUND).contains(&age) {
                let (x, y) = spot(1.0 - age / INBOUND, f64::from(unit(hash(self.seed, n, 94))) * TAU);
                put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
        if band.w >= 30 {
            let bearing = format!("{:03}°", (sweep.to_degrees().round() as u32) % 360);
            for (i, c) in bearing.chars().enumerate() {
                put(g, band, i as i64, 0, c.encode_utf8(&mut [0; 4]), pal.shade(GREEN, 0.3));
            }
            self.keep(band.x, band.y, bearing.chars().count());
        }
    }
}
