use super::*;

/// The board: a torus this many cells across and down (two cells a row,
/// drawn as half blocks); a band bigger than it shows it again.
pub(super) const GW: usize = 256;
pub(super) const GH: usize = 96;

/// `life`: each cell's age in generations (0: dead), the generations so
/// far, the part of one still owed, the blobs of seed still owed, and each
/// message's glider (which message, the generation it began).
#[derive(Clone, Debug, Default)]
pub(super) struct Life {
    pub(super) cells: Vec<u8>,
    pub(super) gens: u64,
    owed: f64,
    seeds: f64,
    pub(super) gliders: VecDeque<(u64, u64)>,
}

/// A glider going right and down, its cells in a 3 x 3 box; it moves a
/// cell each way every 4 generations.
const GLIDER: [(i64, i64); 5] = [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)];
/// How long a glider's envelope is drawn, in generations.
const GLIDER_MARK: u64 = 24;

/// Where message `n`'s glider began (by the middle of the board) and the
/// way it goes.
fn glider(seed: u64, n: u64) -> ((i64, i64), (i64, i64)) {
    let hs = hash(seed, n, 71);
    let x = (GW / 2) as i64 - 8 + (hs % 16) as i64;
    let y = (GH / 2) as i64 - 4 + ((hs >> 8) % 8) as i64;
    let dir = (if (hs >> 16) & 1 == 0 { 1 } else { -1 }, if (hs >> 17) & 1 == 0 { 1 } else { -1 });
    ((x, y), dir)
}

fn at(x: i64, y: i64) -> usize {
    y.rem_euclid(GH as i64) as usize * GW + x.rem_euclid(GW as i64) as usize
}

impl Life {
    /// One generation: a live cell with 2 or 3 live neighbours lives on
    /// (a year older), a dead one with 3 comes to life.
    pub(super) fn generation(&mut self) {
        let old = &self.cells;
        let mut next = vec![0u8; GW * GH];
        for y in 0..GH {
            let (up, down) = ((y + GH - 1) % GH, (y + 1) % GH);
            for x in 0..GW {
                let (left, right) = ((x + GW - 1) % GW, (x + 1) % GW);
                let n =
                    [(up, left), (up, x), (up, right), (y, left), (y, right), (down, left), (down, x), (down, right)]
                        .iter()
                        .filter(|&&(r, c)| old[r * GW + c] > 0)
                        .count();
                let age = old[y * GW + x];
                next[y * GW + x] = match (age > 0, n) {
                    (true, 2 | 3) => age.saturating_add(1),
                    (false, 3) => 1,
                    _ => 0,
                };
            }
        }
        self.cells = next;
        self.gens += 1;
    }
}

impl Anim {
    pub(super) fn step_life(&mut self, dt: f64) {
        let load = &self.input.load;
        let seed = self.seed;
        let life = &mut self.life;
        if life.cells.is_empty() {
            // A soup to start from: a fifth of the cells alive.
            life.cells = (0..GW * GH).map(|i| u8::from(unit(hash(seed, i as u64, 72)) < 0.2)).collect();
        }
        // Each message: a glider by the middle, newest last.
        for i in 0..self.arrived {
            let n = self.messages - 1 - i as u64;
            let ((x, y), (dx, dy)) = glider(seed, n);
            for (cx, cy) in GLIDER {
                let (cx, cy) = (if dx < 0 { 2 - cx } else { cx }, if dy < 0 { 2 - cy } else { cy });
                life.cells[at(x + cx, y + cy)] = 1;
            }
            life.gliders.push_back((n, life.gens));
        }
        while life.gliders.front().is_some_and(|&(_, g)| life.gens.saturating_sub(g) > GLIDER_MARK) {
            life.gliders.pop_front();
        }
        while life.gliders.len() > DROPS {
            life.gliders.pop_front();
        }
        // 5 generations a second idle, 15 with every core busy.
        life.owed += dt * (5.0 + 10.0 * f64::from(share(load.cpu)));
        // Each generation sows blobs (4.5 cells each, about) for a part of
        // the cells short of the memory's share (a twenty-fifth of the board
        // to a third), a bigger part the more the CPUs switch: a hundredth
        // to a tenth.
        let room = (0.04 + 0.3 * f64::from(share(load.memory.unwrap_or(0.4)))) * (GW * GH) as f64;
        let part = 0.01 + 0.09 * f64::from(load.switches.map_or(0.0, count_level));
        while life.owed >= 1.0 {
            life.owed -= 1.0;
            life.generation();
            let alive = life.cells.iter().filter(|&&a| a > 0).count() as f64;
            life.seeds += (room - alive).max(0.0) * part / 4.5;
            let mut k = 0;
            while life.seeds >= 1.0 {
                life.seeds -= 1.0;
                k += 1;
                let hs = hash(seed, life.gens, 73 + k);
                let (x, y) = ((hs % GW as u64) as i64, ((hs >> 16) % GH as u64) as i64);
                for c in 0..9u64 {
                    if (hs >> (32 + c)) & 1 == 1 {
                        life.cells[at(x + (c % 3) as i64, y + (c / 3) as i64)] = 1;
                    }
                }
            }
        }
    }

    /// The board in half blocks, the middle of the band on the middle of
    /// it: a newborn cell bright, then in the CPU's colour, faded once it
    /// has stood still a while; each message's glider marked by an
    /// envelope a moment.
    pub(super) fn draw_life(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let cells = &self.life.cells;
        if cells.is_empty() {
            return;
        }
        let colour = heat(&self.input.load);
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let (ox, oy) = ((GW / 2) as i64 - w / 2, (GH / 2) as i64 - h);
        for y in 0..h {
            for x in 0..w {
                let top = cells[at(ox + x, oy + 2 * y)];
                let bottom = cells[at(ox + x, oy + 2 * y + 1)];
                let ch = match (top > 0, bottom > 0) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    (false, false) => continue,
                };
                let young = match (top, bottom) {
                    (0, b) => b,
                    (t, 0) => t,
                    (t, b) => t.min(b),
                };
                let style = match young {
                    1..=2 => Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) },
                    3..=20 => pal.shade(colour, 0.0),
                    _ => pal.shade(colour, 0.45),
                };
                put(g, band, x, y, ch, style);
            }
        }
        for &(n, began) in &self.life.gliders {
            let ((x, y), (dx, dy)) = glider(self.seed, n);
            let moved = (self.life.gens - began) as i64 / 4;
            let (gx, gy) = (x + 1 + dx * moved, y + 1 + dy * moved);
            put(
                g,
                band,
                gx - ox,
                (gy - oy).div_euclid(2),
                ENVELOPE,
                Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) },
            );
        }
    }
}
