use super::*;

/// `hourglass`: how far the falling grains have gone, rows.
#[derive(Clone, Debug, Default)]
pub(super) struct Sand {
    pub(super) fall: f64,
}

/// How fast a message's envelope falls through the neck, rows a second.
const ENVELOPE_FALL: f64 = 6.0;

/// One glass: its middle column, its top row, how many rows each bulb has
/// (it is `2 * bulb + 3` rows tall and as wide), and the share of its time
/// gone.
struct Glass {
    cx: i64,
    top: i64,
    bulb: i64,
    gone: f64,
}

impl Glass {
    fn rows(bulb: i64) -> i64 {
        2 * bulb + 3
    }

    /// The neck's row.
    fn neck(&self) -> i64 {
        self.top + 1 + self.bulb
    }
}

impl Anim {
    pub(super) fn step_hourglass(&mut self, dt: f64) {
        self.sand.fall += dt * (4.0 + 16.0 * f64::from(share(self.input.load.cpu)));
    }

    /// The hour's glass, as tall as the band (no wider than it), its sand
    /// run down as far as the hour has; where they fit, the minute's on its left and the
    /// day's on its right, smaller. The glass in the CPU's colour; the
    /// grains falling faster the busier the cores, bouncing on the pile
    /// with the disks; each message an `✉` falling through the hour's neck.
    pub(super) fn draw_hourglass(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let day = if self.input.day.is_finite() { self.input.day.rem_euclid(86_400.0) } else { 0.0 };
        // As tall as the band, no wider than it.
        let bulb = ((h - 3) / 2).min((w - 3) / 2).max(0);
        let hour = Glass { cx: w / 2, top: h - Glass::rows(bulb), bulb, gone: day % 3600.0 / 3600.0 };
        self.glass(g, band, pal, &hour, true);
        let small = (h * 2 / 3 - 3).max(0) / 2;
        let (big_w, small_w) = (Glass::rows(bulb), Glass::rows(small));
        let off = big_w / 2 + 4 + small_w / 2;
        if hour.cx - off - small_w / 2 < 0 || hour.cx + off + small_w / 2 >= w {
            return;
        }
        let top = h - Glass::rows(small);
        for (cx, gone, label) in [(hour.cx - off, day % 60.0 / 60.0, "60s"), (hour.cx + off, day / 86_400.0, "24h")] {
            self.glass(g, band, pal, &Glass { cx, top, bulb: small, gone }, false);
            for (i, c) in label.chars().enumerate() {
                put(g, band, cx - 1 + i as i64, top - 1, c.encode_utf8(&mut [0; 4]), pal.shade(WHITE, 0.5));
            }
            if let (Ok(x), Ok(y)) =
                (u16::try_from(i64::from(band.x) + cx - 1), u16::try_from(i64::from(band.y) + top - 1))
            {
                self.keep(x, y, label.len());
            }
        }
    }

    fn glass(&self, g: &mut Grid, band: Rect, pal: &Palette, glass: &Glass, hour: bool) {
        let load = &self.input.load;
        let Glass { cx, top, bulb, gone } = *glass;
        let frame = pal.shade(heat(load), 0.0);
        let bottom = top + Glass::rows(bulb) - 1;
        for x in cx - bulb - 1..=cx + bulb + 1 {
            put(g, band, x, top, "=", frame);
            put(g, band, x, bottom, "=", frame);
        }
        // Each bulb's rows: the walls, `half` in from the middle.
        for r in 0..bulb {
            let (up, down) = (bulb - r, r + 1);
            put(g, band, cx - up - 1, top + 1 + r, "\\", frame);
            put(g, band, cx + up + 1, top + 1 + r, "/", frame);
            put(g, band, cx - down - 1, glass.neck() + 1 + r, "/", frame);
            put(g, band, cx + down + 1, glass.neck() + 1 + r, "\\", frame);
        }
        put(g, band, cx - 1, glass.neck(), ")", frame);
        put(g, band, cx + 1, glass.neck(), "(", frame);
        // The sand: what is left lies by the neck, sinking in the middle
        // first; what has run down piles up from the bottom, highest in the
        // middle.
        let room = bulb * bulb + 2 * bulb;
        let below = ((gone.clamp(0.0, 1.0) * room as f64).round() as i64).min(room);
        let mut above = room - below;
        let sand = pal.shade(YELLOW, 0.0);
        for half in 1..=bulb {
            let y = top + 1 + bulb - half;
            let cells = 2 * half + 1;
            let n = above.min(cells);
            for k in 0..n {
                let x = if k % 2 == 0 { cx - half + k / 2 } else { cx + half - k / 2 };
                put(g, band, x, y, if n < cells { "░" } else { "▒" }, sand);
            }
            above -= n;
        }
        let (mut left, mut pile) = (below, bottom);
        for half in (1..=bulb).rev() {
            let y = glass.neck() + half;
            let cells = 2 * half + 1;
            let n = left.min(cells);
            for k in 0..n {
                let x = if k % 2 == 0 { cx + k / 2 } else { cx - (k + 1) / 2 };
                put(g, band, x, y, if n < cells { "░" } else { "▒" }, sand);
            }
            if n > 0 {
                pile = y;
            }
            left -= n;
        }
        // The stream, all the time the glass has (the last grains still
        // falling): grains and gaps in turn, moving down a row each step,
        // so even one cell of it changes with every step.
        let grains = pal.shade(BRIGHT_YELLOW, 0.0);
        let fall = self.sand.fall.floor() as i64;
        for y in glass.neck()..pile {
            put(g, band, cx, y, if (y - fall).rem_euclid(2) == 0 { "." } else { ":" }, grains);
        }
        // Grains bouncing off the pile, with the disks.
        let disk = load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr));
        let tick = (self.t * 8.0).floor() as u64;
        let half = pile - glass.neck() - 1;
        for dx in [-2i64, -1, 1, 2] {
            if dx.abs() <= half && unit(hash(self.seed, tick, (dx + 9) as u64)) < disk * 0.8 {
                put(g, band, cx + dx, pile - 1, "'", grains);
            }
        }
        if hour {
            for &(_, at) in &self.drops {
                let y = glass.neck() + ((self.t - at) * ENVELOPE_FALL).floor() as i64;
                if y < pile {
                    put(g, band, cx, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
                }
            }
        }
    }
}
