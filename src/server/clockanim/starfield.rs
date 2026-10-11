use super::*;

/// `starfield`: how far it has flown (each star comes from the far end to
/// the near one in about 2 of these), and how fast, eased.
#[derive(Clone, Debug, Default)]
pub(super) struct Stars {
    pub(super) travel: f64,
    pub(super) speed: f64,
}

/// How fast a message's comet crosses, columns a second.
const COMET_SPEED: f64 = 30.0;

impl Anim {
    pub(super) fn step_starfield(&mut self, dt: f64) {
        let load = &self.input.load;
        // Faster the more the network carries: a drift idle, a warp busy.
        let target = 0.15 + 1.6 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx)));
        let s = &mut self.stars;
        s.speed += (target - s.speed) * (1.0 - (-dt / 0.5).exp());
        s.travel += dt * s.speed;
    }

    /// Stars coming out of the middle, more of them the more processes run,
    /// faster the more the network carries (streaks at a warp); the far
    /// ones in the CPU's colour; each message a comet across.
    pub(super) fn draw_starfield(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (f64::from(band.w), f64::from(band.h));
        let (cx, cy) = (w / 2.0, h / 2.0);
        let crowd = load.processes.map_or(0.3, |p| (p as f64 / 1000.0).clamp(0.0, 1.0));
        let stars = (w * h * (0.02 + 0.1 * crowd)) as u64;
        let colour = heat(load);
        let warp = self.stars.speed > 0.8;
        // Where star `i` is on the screen at depth `d` (1 far, 0 near), from
        // its place `(x0, y0)` across the field: the far ones spread over a
        // third of the band about its middle, the near ones off its edges.
        let at = |x0: f64, y0: f64, d: f64| {
            let f = 1.5 * 0.3 / (d + 0.3);
            (cx + x0 * f * cx, cy + y0 * f * cy)
        };
        for i in 0..stars {
            let hs = hash(self.seed, i, 51);
            let pace = 0.8 + 0.4 * f64::from(unit(hash(hs, 8, 0)));
            let along = f64::from(unit(hs)) + self.stars.travel * 0.5 * pace;
            let (k, d) = (along.floor() as u64, 1.0 - along.fract());
            let hk = hash(hs, k, 52);
            let (x0, y0) = (f64::from(unit(hk)) * 2.0 - 1.0, f64::from(unit(hash(hk, 12, 0))) * 2.0 - 1.0);
            let (sx, sy) = at(x0, y0, d);
            if warp {
                // A streak behind it, toward the middle.
                let (bx, by) = at(x0, y0, d + 0.05 * self.stars.speed);
                let (dx, dy) = (sx - bx, sy - by);
                let ch = if dx.abs() > 2.0 * dy.abs() {
                    "-"
                } else if dy.abs() > 2.0 * dx.abs() {
                    "|"
                } else if dx * dy > 0.0 {
                    "\\"
                } else {
                    "/"
                };
                put(g, band, bx.floor() as i64, by.floor() as i64, ch, pal.shade(colour, 0.5));
            }
            let (ch, style) = if d > 0.7 {
                (".", pal.shade(colour, 0.5))
            } else if d > 0.4 {
                ("+", pal.shade(colour, 0.0))
            } else if d > 0.15 {
                ("*", pal.shade(BRIGHT_WHITE, 0.0))
            } else {
                ("*", Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) })
            };
            put(g, band, sx.floor() as i64, sy.floor() as i64, ch, style);
        }
        // Each message: a comet from the left, its tail behind it.
        for &(n, at) in &self.drops {
            let x = ((self.t - at) * COMET_SPEED).floor() as i64;
            if x - 6 >= i64::from(band.w) {
                continue;
            }
            let y = (hash(self.seed, n, 53) % u64::from(band.h)) as i64;
            for d in 1..=6i64 {
                let ch = if d <= 2 { "=" } else { "-" };
                put(g, band, x - d, y, ch, pal.shade(BRIGHT_YELLOW, d as f32 / 7.0));
            }
            put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}
