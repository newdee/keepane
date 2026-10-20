use super::*;

/// `aquarium`: how far each fish has swum (columns), one a pane of the
/// clock's window; where the surface's waves and the weed's sway are.
#[derive(Clone, Debug, Default)]
pub(super) struct Tank {
    pub(super) swum: Vec<f64>,
    surface: f64,
    sway: f64,
}

/// The fish, facing right; each pane takes the next.
const FISH: [&str; 4] = ["><>", "><(((o>", ">=>", "><))'>"];
pub(super) const FISH_COLOURS: [u8; 6] = [YELLOW, CYAN, MAGENTA, GREEN, BRIGHT_BLUE, RED];
/// How fast bubbles (and a message's envelope) rise, rows a second.
const BUBBLE_RISE: f64 = 3.0;

/// A fish facing left: the same, back to front, its marks turned round.
fn facing_left(fish: &str) -> String {
    fish.chars()
        .rev()
        .map(|c| match c {
            '<' => '>',
            '>' => '<',
            '(' => ')',
            ')' => '(',
            c => c,
        })
        .collect()
}

impl Anim {
    pub(super) fn step_aquarium(&mut self, dt: f64) {
        let load = &self.input.load;
        let tank = &mut self.tank;
        let fish = self.out_rates.len().max(1);
        if tank.swum.len() != fish {
            tank.swum = vec![0.0; fish];
        }
        // A fish swims 1 column a second while its pane is quiet, 15 while
        // it prints 10 MB a second.
        for (i, swum) in tank.swum.iter_mut().enumerate() {
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            *swum += dt * (1.0 + 14.0 * f64::from(out_level(rate)));
        }
        tank.surface += dt * (0.5 + 3.0 * f64::from(share(load.cpu)));
        tank.sway += dt * (0.3 + 2.5 * f64::from(load.switches.map_or(0.0, count_level)));
    }

    /// Waves on top (quicker the busier the cores), sand below, weed on it
    /// (taller the more memory is used, swaying with the context
    /// switches), bubbles (more the more the disks do), a fish a pane
    /// (faster the more it prints), and an envelope rising for each message.
    pub(super) fn draw_aquarium(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let tank = &self.tank;
        for x in 0..w {
            let wave = noise(hash(self.seed, 31, 0), x as f64 * 0.35 + tank.surface);
            put(g, band, x, 0, if wave > 0.5 { "~" } else { "-" }, pal.shade(CYAN, 0.0));
            put(g, band, x, h - 1, "░", pal.shade(YELLOW, 0.4));
        }
        // Weed: a stalk in about one column of eleven.
        let memory = share(load.memory.unwrap_or(0.4));
        for x in 0..w {
            let hs = hash(self.seed, x as u64, 32);
            if !hs.is_multiple_of(11) {
                continue;
            }
            let tall = ((0.3 + 0.7 * unit(hash(hs, 8, 0))) * memory * (h - 2) as f32).round().max(1.0) as i64;
            for r in 0..tall.min(h - 2) {
                let swing =
                    (tank.sway * std::f64::consts::TAU + r as f64 * 0.8 + f64::from(unit(hash(hs, 16, 0))) * 6.0).sin();
                put(g, band, x, h - 2 - r, if swing > 0.0 { "(" } else { ")" }, pal.shade(GREEN, 0.0));
            }
        }
        // Bubbles: each rises from the sand to the surface, then again
        // elsewhere.
        let water = (h - 2).max(1) as f64;
        let disk = load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr));
        let bubbles = (disk * w as f32 * 0.3) as u64;
        for i in 0..bubbles {
            let hs = hash(self.seed, i, 33);
            let at = self.t * BUBBLE_RISE / water + f64::from(unit(hs));
            let (k, age) = (at.floor() as u64, at.fract());
            let x = (hash(hs, k, 34) % band.w as u64) as i64;
            let wobble = (noise(hs, self.t * 2.0) * 2.0).round() as i64 - 1;
            let y = h - 2 - (age * water).floor() as i64;
            let ch = if age < 0.3 {
                "."
            } else if age < 0.7 {
                "o"
            } else {
                "O"
            };
            put(g, band, x + wobble, y.max(1), ch, pal.shade(CYAN, 0.35));
        }
        // A fish a pane, back and forth, each in a lane of its own where
        // there are rows enough: spread evenly over the water.
        let rows = (h - 2).max(1);
        let (fish_n, base) = (tank.swum.len() as i64, (hash(self.seed, 35, 0) % rows as u64) as i64);
        for (i, swum) in tank.swum.iter().enumerate() {
            let fish = FISH[i % FISH.len()];
            let len = fish.chars().count() as i64;
            let span = (w - len).max(1) as f64;
            let hs = hash(self.seed, i as u64, 35);
            let lane = 1 + (base + i as i64 * (rows / fish_n).max(1)) % rows;
            let m = (swum + f64::from(unit(hash(hs, 8, 0))) * 2.0 * span).rem_euclid(2.0 * span);
            let (x, text) = if m < span { (m, fish.to_string()) } else { (2.0 * span - m, facing_left(fish)) };
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            let style = Style { bold: out_level(rate) > 0.3, ..pal.shade(FISH_COLOURS[i % FISH_COLOURS.len()], 0.0) };
            for (dx, c) in text.chars().enumerate() {
                put(g, band, x as i64 + dx as i64, lane.min(h - 2), c.encode_utf8(&mut [0; 4]), style);
            }
        }
        // Each message: an envelope rising from the sand, wobbling.
        for &(n, at) in &self.drops {
            let age = self.t - at;
            let y = h - 2 - (age * BUBBLE_RISE).floor() as i64;
            if y < 1 {
                continue;
            }
            let x = (hash(self.seed, n, 36) % band.w as u64) as i64 + (noise(n, age * 2.0) * 2.0).round() as i64 - 1;
            put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}
