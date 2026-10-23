use super::*;

/// `penguins`: how far each penguin has gone, one a pane of the clock's
/// window; how far the water has rolled, the snow fallen, the aurora moved.
#[derive(Clone, Debug, Default)]
pub(super) struct Ice {
    pub(super) gone: Vec<f64>,
    waves: f64,
    pub(super) snow: f64,
    aurora: f64,
}

/// A penguin standing, facing right: its head and beak; its body, flippers
/// out and in as it waddles.
const STANDING: [&str; 3] = ["(o>", "/█\\", "|█|"];
/// On its belly, sliding right.
const SLIDING: &str = "=(o>";
/// A fish, jumping out of the water to the ice.
const FISH: &str = "<><";
/// How long a fish is in the air, seconds.
const LEAP: f64 = 1.6;
/// Columns a second, waddling and sliding; how busy its pane must be to
/// slide.
const WADDLE: f64 = 1.5;
const SLIDE: f64 = 12.0;
pub(super) const SLIDES_AT: f32 = 0.3;

impl Anim {
    pub(super) fn step_penguins(&mut self, dt: f64) {
        let load = &self.input.load;
        let ice = &mut self.ice;
        let n = self.out_rates.len().max(1);
        if ice.gone.len() != n {
            ice.gone = vec![0.0; n];
        }
        for (i, gone) in ice.gone.iter_mut().enumerate() {
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            *gone += dt * if out_level(rate) > SLIDES_AT { SLIDE } else { WADDLE };
        }
        ice.waves += dt * (0.5 + 3.0 * f64::from(share(load.cpu)));
        ice.snow += dt * (1.0 + 4.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx))));
        ice.aurora += dt * 0.4;
    }

    /// An ice shelf with the sea beyond it (rolling quicker the busier the
    /// cores), a penguin a pane waddling to and fro on its own stretch of it, sliding on its
    /// belly while its pane prints a lot; snow falling (more with the
    /// network), an aurora over them (brighter the more memory is used). A
    /// message is a fish leaping out of the sea onto the ice with its
    /// envelope.
    pub(super) fn draw_penguins(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let ice = &self.ice;
        let shore = w - (w / 3).max(4);
        let gy = h - 1;
        // The aurora, on the top row where there are rows to spare.
        if h >= 6 {
            let memory = share(load.memory.unwrap_or(0.4));
            for x in 0..w {
                let v = noise2(hash(self.seed, 160, 0), x as f64 * 0.12 + ice.aurora, ice.aurora * 0.7);
                if v < 0.75 - 0.4 * memory {
                    continue;
                }
                let colour =
                    if noise(hash(self.seed, 161, 0), x as f64 * 0.05 + ice.aurora) > 0.5 { GREEN } else { CYAN };
                put(g, band, x, 0, if v > 0.85 { "▒" } else { "░" }, pal.shade(colour, 0.3));
            }
        }
        // Snow, behind the rest.
        let net = load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let flakes = ((w * h) as f32 * (0.006 + 0.03 * net)).round() as u64;
        for i in 0..flakes {
            let hs = hash(self.seed, i, 162);
            let x = (hs % w as u64) as i64 + (noise(hs, ice.snow * 0.5) * 2.0) as i64;
            let y = (f64::from(unit(hash(hs, 1, 0))) * h as f64 + ice.snow * (1.0 + f64::from(unit(hs))))
                .rem_euclid(h as f64);
            put(g, band, x, y as i64, if hs & 1 == 0 { "*" } else { "." }, pal.shade(WHITE, 0.4));
        }
        // The ice, then the sea.
        for x in 0..w {
            if x < shore {
                put(g, band, x, gy, "▀", pal.shade(BRIGHT_WHITE, 0.0));
            } else {
                let v = noise(hash(self.seed, 163, 0), x as f64 * 0.6 - ice.waves * 2.0);
                put(g, band, x, gy, if v > 0.6 { "^" } else { "~" }, pal.shade(BLUE, 0.0));
            }
        }
        // A penguin a pane, as many as the ice holds.
        // Each on a stretch of its own, so none walks into another.
        let n = (ice.gone.len() as i64).min((shore / 5).max(1));
        let stretch = shore / n;
        let span = (stretch - 4).max(1) as f64;
        for (i, gone) in ice.gone.iter().enumerate().take(n as usize) {
            let rate = self.out_rates.get(i).copied().unwrap_or(0.0);
            let m = (gone + f64::from(unit(hash(self.seed, i as u64, 164))) * 2.0 * span).rem_euclid(2.0 * span);
            let (x, right) = if m < span { (m as i64, true) } else { ((2.0 * span - m) as i64, false) };
            let x = i as i64 * stretch + x;
            let body = pal.shade(WHITE, 0.0);
            let beak = pal.shade(BRIGHT_YELLOW, 0.0);
            let draw = |g: &mut Grid, x: i64, y: i64, text: &str| {
                let text: String = if right { text.into() } else { mirrored(text) };
                for (dx, c) in text.chars().enumerate() {
                    let style = if c == '>' || c == '<' { beak } else { body };
                    if c != ' ' {
                        put(g, band, x + dx as i64, y, c.encode_utf8(&mut [0; 4]), style);
                    }
                }
            };
            if out_level(rate) > SLIDES_AT {
                draw(g, x, gy - 1, SLIDING);
            } else {
                draw(g, x, gy - 2, STANDING[0]);
                let step = (gone / WADDLE * 3.0).floor() as i64 % 2;
                draw(g, x, gy - 1, STANDING[1 + step as usize]);
            }
        }
        // A message: a fish leaps from the sea onto the ice, its envelope
        // over it.
        for &(_, at) in &self.drops {
            let age = (self.t - at) / LEAP;
            if age >= 1.0 {
                continue;
            }
            let x = shore + 2 - (age * 8.0) as i64;
            let y = gy - 1 - (4.0 * age * (1.0 - age) * (gy as f64 - 1.0).min(4.0)).round() as i64;
            for (dx, c) in FISH.chars().enumerate() {
                put(g, band, x + dx as i64, y, c.encode_utf8(&mut [0; 4]), pal.shade(BRIGHT_YELLOW, 0.0));
            }
            put(g, band, x + 1, y - 1, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}

/// `text` facing the other way: the order turned, each part that faces
/// one way turned too.
fn mirrored(text: &str) -> String {
    text.chars()
        .rev()
        .map(|c| match c {
            '>' => '<',
            '<' => '>',
            '(' => ')',
            ')' => '(',
            '/' => '\\',
            '\\' => '/',
            c => c,
        })
        .collect()
}
