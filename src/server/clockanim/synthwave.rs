use super::*;

/// `synthwave`: how far the grid has come towards you (in its lines), how
/// far the sun's stripes have slid, how the stars have twinkled.
#[derive(Clone, Debug, Default)]
pub(super) struct Drive {
    pub(super) travel: f64,
    pub(super) stripes: f64,
    twinkle: f64,
}

/// How fast a message's meteor crosses the sky, columns and rows a second.
const METEOR_X: f64 = 40.0;
const METEOR_Y: f64 = 5.0;
/// The grid's lines across, columns apart at the bottom row.
const SPACING: f64 = 7.0;

/// Grid lines a second: 2 while the cores rest, 5 while all are busy.
pub(super) fn grid_speed(load: &Load) -> f64 {
    2.0 + 3.0 * f64::from(share(load.cpu))
}

impl Anim {
    pub(super) fn step_synthwave(&mut self, dt: f64) {
        let load = &self.input.load;
        let out: f64 = self.out_rates.iter().sum();
        let drive = &mut self.drive;
        drive.travel += dt * grid_speed(load);
        drive.stripes += dt * (0.3 + 2.0 * f64::from(out_level(out)));
        drive.twinkle += dt * (1.0 + 3.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
    }

    /// A neon grid running towards you under a striped sun setting on the
    /// horizon (the grid quicker the busier the cores, the stripes sliding
    /// down with the panes' output), mountains either side (taller the more
    /// memory is used), stars twinkling with the network. A message streaks
    /// across the sky as a meteor.
    pub(super) fn draw_synthwave(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let drive = &self.drive;
        let hy = (h / 2).max(1);
        let cx = w as f64 / 2.0;
        // Stars, in the sky.
        let stars = (w * hy) as u64 / 9;
        for i in 0..stars {
            let hs = hash(self.seed, i, 170);
            let tw = noise(hs, drive.twinkle + f64::from(unit(hs)) * 7.0);
            if tw > 0.45 {
                let (x, y) = ((hs % w as u64) as i64, (hash(hs, 1, 0) % hy as u64) as i64);
                put(g, band, x, y, if tw > 0.8 { "+" } else { "." }, pal.shade(WHITE, 0.3));
            }
        }
        // The sun, half above the horizon, in half rows: yellow at the top
        // to magenta at the horizon, the stripes across its lower half.
        let (ry, rx) = (hy as f64, (hy as f64 * 2.2).min(w as f64 / 3.0));
        for y in 0..hy {
            for x in 0..w {
                let inside = |half: i64| {
                    let sub = 2 * y + half;
                    let dy = (2.0 * hy as f64 - sub as f64 - 0.5) / (2.0 * ry);
                    let dx = (x as f64 + 0.5 - cx) / rx;
                    let stripe = dy < 0.5 && (dy * 2.0 * ry + drive.stripes).rem_euclid(3.0) < 1.0;
                    dx * dx + dy * dy <= 1.0 && !stripe
                };
                let ch = match (inside(0), inside(1)) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    (false, false) => continue,
                };
                let up = 1.0 - y as f64 / hy as f64;
                let colour = if up > 0.75 {
                    BRIGHT_YELLOW
                } else if up > 0.5 {
                    YELLOW
                } else if up > 0.25 {
                    RED
                } else {
                    MAGENTA
                };
                put(g, band, x, y, ch, pal.shade(colour, 0.0));
            }
        }
        // Mountains either side, in front of the sun's edge.
        let memory = f64::from(share(load.memory.unwrap_or(0.4)));
        let tall = (hy - 1).clamp(0, 3) as f64;
        if tall > 0.0 {
            for x in 0..w {
                let edge = ((x as f64 + 0.5 - cx).abs() / cx).clamp(0.0, 1.0);
                let m = f64::from(noise(hash(self.seed, 171, 0), x as f64 * 0.18))
                    * edge.powf(1.5)
                    * tall
                    * (0.5 + 0.5 * memory);
                let eighths = (m * 8.0).round() as i64;
                for r in 0..((eighths + 7) / 8) {
                    let fill = (eighths - r * 8).clamp(1, 8) as usize;
                    let ch = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][fill - 1];
                    put(g, band, x, hy - 1 - r, ch, pal.shade(BLUE, 0.2));
                }
            }
        }
        // The horizon.
        for x in 0..w {
            put(g, band, x, hy, "━", Style { bold: true, ..pal.shade(MAGENTA, 0.0) });
        }
        // The grid: lines across coming nearer, lines along from where the
        // road ends on the horizon.
        let n = (h - 1 - hy).max(0);
        if n > 0 {
            let k = n as f64;
            let frac = drive.travel.rem_euclid(1.0);
            let mut across = vec![false; n as usize + 1];
            for j in 0..64 {
                let z = j as f64 + 1.0 - frac;
                // On the row it is nearest the middle of, crisp: a line between
                // two rows is on neither yet.
                let s = k / z;
                let r = s.round() as i64;
                if (1..=n).contains(&r) && (s - r as f64).abs() < 0.3 {
                    across[r as usize] = true;
                }
            }
            for dy in 1..=n {
                let fade = (0.6 * (1.0 - dy as f64 / n as f64)) as f32;
                let near = dy * 2 > n;
                let style = Style { bold: near, ..pal.shade(MAGENTA, fade) };
                if across[dy as usize] {
                    for x in 0..w {
                        put(g, band, x, hy + dy, if near { "━" } else { "─" }, style);
                    }
                }
                // The lines that reach the bottom row inside the band; on a far
                // row only every few, 2 columns apart at least.
                let lines = (cx / SPACING) as i64 + 1;
                let every = (2.0 * n as f64 / (SPACING * dy as f64)).ceil().max(1.0) as i64;
                for i in (-lines..=lines).filter(|i| i % every == 0) {
                    let slope = i as f64 * SPACING / n as f64;
                    let x = (cx + slope * dy as f64 - 0.5).round() as i64;
                    let ch = match (slope.abs() < 0.7, across[dy as usize]) {
                        (true, true) => "┼",
                        (true, false) => "│",
                        (false, _) if slope > 0.0 => "╲",
                        (false, _) => "╱",
                    };
                    put(g, band, x, hy + dy, ch, style);
                }
            }
        }
        // A message: a meteor across the sky, its envelope at the head.
        for &(n, at) in &self.drops {
            let age = self.t - at;
            let x0 = f64::from(unit(hash(self.seed, n, 172))) * w as f64 * 0.4;
            let (x, y) = ((x0 + age * METEOR_X) as i64, (age * METEOR_Y) as i64);
            if y >= hy || x >= w + 8 {
                continue;
            }
            for t in 1..8 {
                put(g, band, x - t, y, "─", pal.shade(CYAN, t as f32 / 8.0));
            }
            put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}
