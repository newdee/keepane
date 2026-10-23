use super::*;

/// `fireworks`: the rockets in the air (oldest first), the part of one
/// still owed, how many went up, how the stars have twinkled.
#[derive(Clone, Debug, Default)]
pub(super) struct Show {
    pub(super) rockets: VecDeque<Rocket>,
    owed: f64,
    pub(super) launched: u64,
    twinkle: f64,
}

/// One rocket: when it went up, its number, and whether a message sent it
/// (bigger, with the message's envelope).
#[derive(Clone, Copy, Debug)]
pub(super) struct Rocket {
    at: f64,
    n: u64,
    pub(super) message: bool,
}

/// Seconds it rises, seconds its burst lasts.
const RISE: f64 = 0.8;
const BURST: f64 = 1.8;
/// How fast a burst falls, rows a second a second.
const GRAVITY: f64 = 3.0;
/// The most in the air at once.
const MOST: usize = 40;
const COLOURS: [u8; 6] = [RED, YELLOW, GREEN, CYAN, MAGENTA, BRIGHT_YELLOW];

/// Rockets a second: 0.7 while the panes are quiet, 3.2 while they print
/// 10 MB a second.
pub(super) fn launch_rate(out: f64) -> f64 {
    0.7 + 2.5 * f64::from(out_level(out))
}

impl Anim {
    pub(super) fn step_fireworks(&mut self, dt: f64) {
        let load = &self.input.load;
        let out: f64 = self.out_rates.iter().sum();
        let t = self.t;
        let show = &mut self.show;
        show.twinkle += dt * (0.5 + 3.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
        // The first goes up at once; then as the output says.
        show.owed += if show.launched == 0 { 1.0 } else { dt * launch_rate(out) };
        while show.owed >= 1.0 {
            show.owed -= 1.0;
            show.rockets.push_back(Rocket { at: t, n: show.launched, message: false });
            show.launched += 1;
        }
        for _ in 0..self.arrived {
            show.rockets.push_back(Rocket { at: t, n: show.launched, message: true });
            show.launched += 1;
        }
        while show.rockets.front().is_some_and(|r| t - r.at > RISE + BURST) || show.rockets.len() > MOST {
            show.rockets.pop_front();
        }
    }

    /// A city at night (its windows lit the more memory is used), stars
    /// over it twinkling with the network; rockets going up (more of them
    /// the more the window's panes print) and bursting into sparks that
    /// fall and fade (bigger bursts the busier the cores). A message sends
    /// one up twice the size, its envelope where it bursts.
    pub(super) fn draw_fireworks(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let show = &self.show;
        let stars = (w * h) as u64 / 14;
        for i in 0..stars {
            let hs = hash(self.seed, i, 180);
            if noise(hs, show.twinkle + f64::from(unit(hs)) * 5.0) > 0.6 {
                put(g, band, (hs % w as u64) as i64, (hash(hs, 1, 0) % h as u64) as i64, ".", pal.shade(WHITE, 0.4));
            }
        }
        let cores = share(load.cpu);
        for r in &show.rockets {
            let age = self.t - r.at;
            let hr = hash(self.seed, r.n, 183);
            let x = (f64::from(unit(hr)) * (w as f64 - 6.0) + 3.0).round() as i64;
            let top = 1 + (f64::from(unit(hash(hr, 1, 0))) * (h as f64 * 0.4)) as i64;
            let colour = COLOURS[(hr % COLOURS.len() as u64) as usize];
            if age < RISE {
                // Rising, quick then slower, a spark trailing.
                let up = 1.0 - (1.0 - age / RISE).powi(2);
                let y = (h - 1) - ((h - 1 - top) as f64 * up).round() as i64;
                put(g, band, x, y, "|", Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
                put(g, band, x, y + 1, ".", pal.shade(YELLOW, 0.5));
                continue;
            }
            let a = age - RISE;
            let size = if r.message { 2.0 } else { 1.0 };
            let sparks = ((8.0 + 20.0 * f64::from(cores)) * size) as u64;
            let ring = (2.5 + 1.5 * f64::from(unit(hash(hr, 3, 0)))) * size.sqrt();
            let fade = (a / BURST) as f32;
            let ch = if a < 0.3 {
                "*"
            } else if a < 0.9 {
                "+"
            } else {
                "·"
            };
            for i in 0..sparks {
                let hs = hash(hr, i, 184);
                let angle = std::f64::consts::TAU * (i as f64 + f64::from(unit(hs)) * 0.3) / sparks as f64;
                // One speed for the burst, a little either way for each spark:
                // a ring.
                let speed = ring * (0.92 + 0.16 * f64::from(unit(hash(hs, 2, 0))));
                // Columns are half as tall as rows are wide: twice as far across.
                let sx = x as f64 + angle.cos() * speed * a * 2.0;
                let sy = top as f64 + angle.sin() * speed * a + 0.5 * GRAVITY * a * a;
                put(
                    g,
                    band,
                    sx.round() as i64,
                    sy.round() as i64,
                    ch,
                    Style { bold: a < 0.3, ..pal.shade(colour, fade) },
                );
            }
            if r.message && a < 1.0 {
                put(g, band, x, top, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
        // The city along the bottom, in front of the sparks falling into it.
        let memory = share(load.memory.unwrap_or(0.4));
        let tallest = (h / 3).clamp(1, 3);
        for x in 0..w {
            let tall = 1 + (noise(hash(self.seed, 181, 0), x as f64 * 0.35) * tallest as f32) as i64;
            for r in 0..tall.min(tallest) {
                let lit = r < tall - 1 && x % 2 == 1 && unit(hash(self.seed, x as u64, 182 + r as u64)) < memory * 0.7;
                let (ch, style) =
                    if lit { ("▪", pal.shade(BRIGHT_YELLOW, 0.2)) } else { ("█", pal.shade(BLUE, 0.6)) };
                put(g, band, x, h - 1 - r, ch, style);
            }
        }
    }
}
