use super::*;

/// `cat`: how many breaths the cat has taken, how far its tail has swung,
/// how far its ball of yarn has rolled.
#[derive(Clone, Debug, Default)]
pub(super) struct Nap {
    pub(super) breaths: f64,
    tail: f64,
    pub(super) rolled: f64,
}

/// The cat curled up asleep, its head on the left: ears; face and back;
/// paws, belly and tail.
const CAT: [&str; 3] = [" /\\_/\\", "( -.- )____", " (\")(\")___)~"];
const CAT_W: i64 = 12;
/// How long a message keeps it awake, seconds; how fast its envelope
/// falls, rows a second.
const AWAKE: f64 = 3.0;
const ENVELOPE_FALL: f64 = 6.0;
/// How fast a z rises, steps a second, and how many it takes before it is gone.
const Z_RISE: f64 = 2.0;
const Z_STEPS: i64 = 8;

/// Breaths a second: 18 a minute while the cores rest, 72 while all are busy.
fn breath_rate(load: &Load) -> f64 {
    0.3 + 0.9 * f64::from(share(load.cpu))
}

impl Anim {
    pub(super) fn step_cat(&mut self, dt: f64) {
        let load = &self.input.load;
        let nap = &mut self.nap;
        nap.breaths += dt * breath_rate(load);
        nap.tail += dt * (0.3 + 3.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
        let out: f64 = self.out_rates.iter().sum();
        nap.rolled += dt * (2.0 + 12.0 * f64::from(out_level(out)));
    }

    /// Whether a message came in the last few seconds: it wakes the cat.
    pub(super) fn awake(&self) -> bool {
        self.drops.iter().any(|&(_, at)| self.t - at < AWAKE)
    }

    /// The cat asleep on the bottom rows, breathing (quicker the busier the
    /// cores, a z rising with each breath), its tail swinging with the
    /// network, an ear flicking with the disks; a ball of yarn rolling past
    /// (faster the more the window's panes print). A message falls beside
    /// it as an envelope and wakes it a while.
    pub(super) fn draw_cat(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let nap = &self.nap;
        let (x0, y0) = ((w - CAT_W) / 2, h - 3);
        let span = (w - 1).max(1) as f64;
        let m = (nap.rolled + f64::from(unit(hash(self.seed, 140, 0))) * span).rem_euclid(2.0 * span);
        let ball = (if m < span { m } else { 2.0 * span - m }) as i64;
        put(g, band, ball, h - 1, "@", pal.shade(MAGENTA, 0.0));
        let awake = self.awake();
        let body = pal.shade(YELLOW, 0.0);
        let breath = (nap.breaths * std::f64::consts::TAU).sin();
        let tail = (nap.tail * std::f64::consts::TAU).sin();
        let disk = load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr));
        let flick = unit(hash(self.seed, (self.t * 4.0).floor() as u64, 141)) < disk * 0.5;
        for (r, row) in CAT.iter().enumerate() {
            for (i, c) in row.chars().enumerate() {
                let (i, y) = (i as i64, y0 + r as i64);
                let mut style = body;
                let c = match (r, i) {
                    (_, _) if c == ' ' => continue,
                    // The back rises with each breath in.
                    (1, 7..=10) if breath > 0.4 => {
                        if i == 7 || i == 10 {
                            '.'
                        } else {
                            '-'
                        }
                    }
                    (1, 2 | 4) if awake => {
                        style = Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) };
                        'o'
                    }
                    (0, 5) if flick => '|',
                    (2, 11) if tail > 0.5 => '/',
                    _ => c,
                };
                put(g, band, x0 + i, y, c.encode_utf8(&mut [0; 4]), style);
            }
        }
        // A z with each breath, rising from beside the ears up and to the
        // right, fading as it goes.
        if !awake {
            let now = nap.breaths.floor();
            for k in 0..4 {
                let rise = ((nap.breaths - (now - f64::from(k))) / breath_rate(load) * Z_RISE).floor() as i64;
                if rise < Z_STEPS {
                    let ch = if rise < Z_STEPS / 2 { "z" } else { "Z" };
                    let fade = rise as f32 / Z_STEPS as f32;
                    put(g, band, x0 + 7 + rise, y0 - (rise + 1) / 2, ch, pal.shade(CYAN, fade));
                }
            }
        }
        for &(_, at) in &self.drops {
            let age = self.t - at;
            if age < AWAKE {
                let y = ((age * ENVELOPE_FALL).floor() as i64).min(h - 1);
                put(g, band, x0 - 3, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
    }
}
