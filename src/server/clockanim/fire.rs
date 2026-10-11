use super::*;

/// `fire`: how far the flames have risen (rows), and each core's flame
/// (0 to 1), eased.
#[derive(Clone, Debug, Default)]
pub(super) struct Fire {
    rise: f64,
    pub(super) flames: Vec<f32>,
}

/// How fast a message's envelope rises through the flames, rows a second.
const ENVELOPE_RISE: f64 = 6.0;

impl Anim {
    pub(super) fn step_fire(&mut self, dt: f64) {
        let load = &self.input.load;
        let cores = if load.cores.is_empty() { vec![load.cpu] } else { load.cores.clone() };
        let f = &mut self.fire;
        if f.flames.len() != cores.len() {
            f.flames = vec![0.0; cores.len()];
        }
        let ease = (1.0 - (-dt / 0.3).exp()) as f32;
        for (flame, &core) in f.flames.iter_mut().zip(&cores) {
            *flame += (share(core) - *flame) * ease;
        }
        // The flames lick up faster the busier the cores.
        f.rise += dt * (3.0 + 9.0 * f64::from(share(load.cpu)));
    }

    /// A flame a core across the band, as high as the core is busy (an idle
    /// one still a low flame); its tip white the hotter the CPU; sparks over
    /// it with the interrupts; each message an envelope rising through it.
    pub(super) fn draw_fire(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        const CHARS: [&str; 9] = [" ", ".", ":", "-", "=", "+", "*", "#", "%"];
        let load = &self.input.load;
        let flames = &self.fire.flames;
        let (w, h) = (usize::from(band.w), f32::from(band.h));
        // A column's flame: between the cores' either side of it.
        let height = |x: usize| -> f32 {
            let Some(&last) = flames.last() else { return 0.0 };
            let n = flames.len();
            let pos = (x as f32 + 0.5) / w as f32 * n as f32 - 0.5;
            let i = pos.floor().clamp(0.0, (n - 1) as f32) as usize;
            let next = flames.get(i + 1).copied().unwrap_or(last);
            flames[i] + (next - flames[i]) * (pos - i as f32).clamp(0.0, 1.0)
        };
        let tip = match heat(load) {
            GREEN => YELLOW,
            YELLOW => BRIGHT_YELLOW,
            _ => BRIGHT_WHITE,
        };
        let key = hash(self.seed, 21, 0);
        let tops: Vec<f32> = (0..w).map(|x| (0.18 + 0.82 * height(x)) * h).collect();
        for (x, &top) in tops.iter().enumerate() {
            for r in 0..band.h {
                // `r` rows over the bottom; the noise moves up as it rises.
                let up = f32::from(r);
                let turb = noise2(key, x as f64 * 0.45, (f64::from(up) - self.fire.rise) * 0.55);
                let v = 1.0 - up / top.max(0.5) + (turb - 0.5) * 0.9;
                if v <= 0.05 {
                    continue;
                }
                let ch = CHARS[((v.min(1.0) * 8.0).round() as usize).clamp(1, 8)];
                let style = if v > 0.8 {
                    Style { bold: true, ..pal.shade(tip, 0.0) }
                } else if v > 0.55 {
                    pal.shade(YELLOW, 0.0)
                } else if v > 0.3 {
                    pal.shade(RED, 0.0)
                } else {
                    pal.shade(RED, 0.5)
                };
                put(g, band, x as i64, i64::from(band.h - 1 - r), ch, style);
            }
        }
        // Sparks over the flames, more the more interrupts come: each flies
        // up from a flame's tip and fades, then starts again elsewhere.
        let sparks = (load.interrupts.map_or(0.0, count_level) * w as f32 * 0.25) as u64;
        for i in 0..sparks {
            let hs = hash(self.seed, i, 22);
            let period = 1.2 + 0.8 * f64::from(unit(hs));
            let at = self.t / period + f64::from(unit(hash(hs, 8, 0)));
            let (k, age) = (at.floor() as u64, at.fract());
            let x = (hash(hs, k, 23) % w as u64) as usize;
            let y = f64::from(band.h) - f64::from(tops[x]) - age * period * 4.0;
            let ch = if age < 0.5 { "*" } else { "." };
            put(g, band, x as i64, y.floor() as i64, ch, pal.shade(YELLOW, age as f32 * 0.8));
        }
        // Each message: an envelope rising from the embers, sparks about it.
        for &(n, at) in &self.drops {
            let age = self.t - at;
            let x = (hash(self.seed, n, 24) % w as u64) as i64;
            let y = i64::from(band.h) - 1 - (age * ENVELOPE_RISE).floor() as i64;
            if y < 0 {
                continue;
            }
            put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            for s in 0..4u64 {
                let hs = hash(n, s, 25);
                let (dx, dy) = ((hs % 5) as i64 - 2, ((hs >> 8) % 3) as i64 - 1);
                if (dx, dy) != (0, 0) {
                    put(g, band, x + dx, y + dy, "*", pal.shade(BRIGHT_YELLOW, (age as f32).min(1.0) * 0.6));
                }
            }
        }
    }
}
