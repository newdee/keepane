use super::*;

/// `sea`: how far the waves have rolled, the moon's path has glittered and
/// the stars have twinkled; how far each boat has sailed, one a pane of
/// the clock's window.
#[derive(Clone, Debug, Default)]
pub(super) struct Night {
    pub(super) roll: f64,
    glitter: f64,
    twinkle: f64,
    pub(super) sailed: Vec<f64>,
}

/// How fast a message's envelope drifts ashore, columns a second.
const DRIFT: f64 = 6.0;

/// The moon at `day` (seconds since midnight): the share of its way
/// across the sky, from rising on the left (18:00 at night, 06:00 by day)
/// to setting on the right; and whether it is night.
pub(super) fn moon(day: f64) -> (f64, bool) {
    let day = if day.is_finite() { day.rem_euclid(86_400.0) } else { 0.0 };
    let p = (day - 64_800.0).rem_euclid(86_400.0) / 43_200.0;
    if p < 1.0 { (p, true) } else { (p - 1.0, false) }
}

impl Anim {
    pub(super) fn step_sea(&mut self, dt: f64) {
        let load = &self.input.load;
        let night = &mut self.night;
        night.roll += dt * (0.4 + 3.0 * f64::from(share(load.cpu)));
        night.glitter += dt * (1.0 + 8.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
        night.twinkle += dt * (0.3 + 3.0 * f64::from(load.switches.map_or(0.0, count_level)));
        let boats = self.out_rates.len().max(1);
        if night.sailed.len() != boats {
            night.sailed = vec![0.0; boats];
        }
        // A boat sails half a column a second while its pane is quiet,
        // ten and a half while it prints 10 MB a second.
        for (i, sailed) in night.sailed.iter_mut().enumerate() {
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            *sailed += dt * (0.5 + 10.0 * f64::from(out_level(rate)));
        }
    }

    /// The sky over a third of the rows: the moon crossing it with the
    /// hours, bright by night and pale by day, and by night stars (more the
    /// more memory is used, twinkling with the context switches). The sea
    /// below: waves rolling in (choppier and quicker the busier the cores),
    /// the moon's path on them (glittering with the network), a boat a pane
    /// (faster the more it prints), and each message an envelope drifting
    /// ashore.
    pub(super) fn draw_sea(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let night = &self.night;
        let sky = (h / 3).max(1);
        let rows = (h - sky).max(1);
        let (way, dark) = moon(self.input.day);
        let mx = (way * (w - 1) as f64).round() as i64;
        // On the horizon at either end, highest half way.
        let my = sky - 1 - ((way * std::f64::consts::PI).sin() * (sky - 1) as f64).round() as i64;
        if dark {
            let memory = share(load.memory.unwrap_or(0.4));
            let stars = (memory * (w * sky) as f32 * 0.12).round() as u64;
            for i in 0..stars {
                let hs = hash(self.seed, i, 120);
                let (x, y) = ((hs % w as u64) as i64, (hash(hs, 1, 0) % sky as u64) as i64);
                let twinkle = noise(hs, night.twinkle + f64::from(unit(hs)) * 9.0);
                let (ch, fade) = if twinkle > 0.75 {
                    ("*", 0.0)
                } else if twinkle > 0.45 {
                    ("+", 0.3)
                } else {
                    (".", 0.5)
                };
                put(g, band, x, y, ch, pal.shade(WHITE, fade));
            }
        }
        let moonlight = if dark { Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) } } else { pal.shade(WHITE, 0.6) };
        put(g, band, mx, my, "●", moonlight);
        let chop = share(load.cpu);
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        for r in 0..rows {
            let y = sky + r;
            // 1 on the row nearest the shore.
            let near = (r + 1) as f64 / rows as f64;
            let fade = (0.5 * (1.0 - near)) as f32;
            for x in 0..w {
                let v = noise2(
                    hash(self.seed, 121, 0),
                    x as f64 * (0.3 - 0.12 * near) + night.roll * (0.5 + near),
                    r as f64 * 1.3,
                ) + (chop - 0.5) * 0.3;
                let ch = if v > 0.82 {
                    "^"
                } else if v > 0.68 {
                    "~"
                } else if v > 0.52 {
                    "-"
                } else {
                    continue;
                };
                put(g, band, x, y, ch, pal.shade(if near > 0.66 { BRIGHT_BLUE } else { BLUE }, fade));
            }
            // The moon's path: wider nearer the shore.
            let half = (near * 3.0).round() as i64;
            for x in mx - half..=mx + half {
                let glint = noise(hash(self.seed, x.unsigned_abs(), r as u64 ^ 0x51), night.glitter);
                if glint < 0.55 - 0.3 * net {
                    continue;
                }
                let style = if dark { pal.shade(BRIGHT_YELLOW, fade) } else { pal.shade(WHITE, 0.6) };
                put(g, band, x, y, if glint > 0.8 { "═" } else { "─" }, style);
            }
        }
        // A boat a pane, back and forth, each in a lane of its own where
        // there are rows enough; its sail on the row above.
        let (boats, base) = (night.sailed.len() as i64, (hash(self.seed, 122, 0) % rows as u64) as i64);
        let span = (w - 4).max(1) as f64;
        for (i, sailed) in night.sailed.iter().enumerate() {
            let lane = sky + (base + i as i64 * (rows / boats.max(1)).max(1)) % rows;
            let m = (sailed + f64::from(unit(hash(self.seed, i as u64, 123))) * 2.0 * span).rem_euclid(2.0 * span);
            let (x, sail) = if m < span { (m as i64, "|\\") } else { ((2.0 * span - m) as i64, "/|") };
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            let style = Style { bold: out_level(rate) > 0.3, ..pal.shade(FISH_COLOURS[i % FISH_COLOURS.len()], 0.0) };
            for (dx, c) in "\\__/".chars().enumerate() {
                put(g, band, x + dx as i64, lane, c.encode_utf8(&mut [0; 4]), style);
            }
            for (dx, c) in sail.chars().enumerate() {
                put(g, band, x + 1 + dx as i64, lane - 1, c.encode_utf8(&mut [0; 4]), style);
            }
        }
        for &(_, at) in &self.drops {
            let x = w - 1 - ((self.t - at) * DRIFT).floor() as i64;
            put(g, band, x, h - 1, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}
