use super::*;

/// `weather`: how far the rain has fallen (rows), how far the clouds have
/// drifted, and the wind (0 to 1), eased.
#[derive(Clone, Debug, Default)]
pub(super) struct Weather {
    pub(super) fall: f64,
    drift: f64,
    pub(super) wind: f32,
}

/// How fast a message's envelope falls, rows a second, and how long its
/// splash stays.
const ENVELOPE_FALL: f64 = 12.0;
const SPLASH: f64 = 0.6;

impl Anim {
    pub(super) fn step_weather(&mut self, dt: f64) {
        let load = &self.input.load;
        let disk = load.disk.map_or(0.0, |(r, w)| bytes_level(r + w));
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let s = &mut self.weather;
        s.fall += dt * (10.0 + 10.0 * f64::from(disk));
        s.wind += (net - s.wind) * (1.0 - (-dt / 0.5).exp()) as f32;
        s.drift += dt * (0.5 + 3.0 * f64::from(s.wind));
    }

    /// Clouds on top, the more of the sky the busier the cores; rain under
    /// them, the heavier the more the disks do, slanting with the network's
    /// wind; lightning with the interrupts; water below, as deep as the
    /// memory in use; each message an envelope falling into it.
    pub(super) fn draw_weather(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let s = &self.weather;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let clouds = (h / 4).max(1);
        let water = (1 + (share(load.memory.unwrap_or(0.0)) * (h / 3) as f32).round() as i64).min(h - clouds - 1);
        let surface = h - water;
        // Lightning: a bolt in a quarter second, more likely the more
        // interrupts come.
        let slot = (self.t * 4.0).floor() as u64;
        let bolt = unit(hash(self.seed, slot, 61)) < load.interrupts.map_or(0.0, count_level) * 0.35;
        let cover = 0.25 + 0.7 * share(load.cpu);
        let key = hash(self.seed, 62, 0);
        for y in 0..clouds {
            for x in 0..w {
                let v = noise2(key, x as f64 * 0.15 + s.drift, y as f64 * 0.5);
                if v >= cover {
                    continue;
                }
                let ch = if v < cover - 0.3 {
                    "▓"
                } else if v < cover - 0.15 {
                    "▒"
                } else {
                    "░"
                };
                let style =
                    if bolt { Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) } } else { pal.shade(WHITE, 0.3) };
                put(g, band, x, y, ch, style);
            }
        }
        // Rain between the clouds and the water.
        let sky = (surface - clouds).max(1);
        let disk = load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr));
        let drops = ((0.02 + 0.25 * disk) * (w * sky) as f32) as u64;
        let slant = if s.wind > 0.25 { "\\" } else { "|" };
        for i in 0..drops {
            let hs = hash(self.seed, i, 63);
            let cycle = sky as f64 + 3.0;
            let pos = (s.fall * (0.8 + 0.4 * f64::from(unit(hash(hs, 8, 0)))) + f64::from(unit(hs)) * cycle)
                .rem_euclid(cycle);
            let y = pos.floor() as i64;
            if y >= sky {
                continue;
            }
            let x = ((hs >> 20) as i64 + (y as f32 * s.wind * 0.6).round() as i64).rem_euclid(w);
            put(g, band, x, clouds + y, slant, pal.shade(CYAN, 0.3));
        }
        if bolt {
            let mut x = (hash(self.seed, slot, 64) % band.w as u64) as i64;
            for y in clouds..surface {
                let turn = (hash(self.seed, slot, 65 + y as u64) % 3) as i64 - 1;
                let ch = match turn {
                    -1 => "/",
                    1 => "\\",
                    _ => "|",
                };
                put(g, band, x, y, ch, Style { bold: true, ..pal.shade(BRIGHT_YELLOW, 0.0) });
                x += turn;
            }
        }
        // The water: waves on its surface.
        for y in surface..h {
            for x in 0..w {
                let wave =
                    noise(hash(self.seed, 66, y as u64), x as f64 * 0.4 + self.t * (0.5 + 2.0 * f64::from(s.wind)));
                let ch = if wave > 0.5 { "~" } else { "-" };
                let style = if y == surface { pal.shade(BRIGHT_BLUE, 0.0) } else { pal.shade(BLUE, 0.3) };
                put(g, band, x, y, ch, style);
            }
        }
        // Each message: an envelope falling from the clouds, a splash where
        // it meets the water.
        for &(n, at) in &self.drops {
            let age = self.t - at;
            let x = (hash(self.seed, n, 67) % band.w as u64) as i64;
            let y = clouds + (age * ENVELOPE_FALL).floor() as i64;
            if y < surface {
                put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            } else if age - (surface - clouds) as f64 / ENVELOPE_FALL < SPLASH {
                for dx in [-1, 1] {
                    put(g, band, x + dx, surface - 1, "o", pal.shade(BRIGHT_WHITE, 0.2));
                }
                put(g, band, x, surface, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
    }
}
