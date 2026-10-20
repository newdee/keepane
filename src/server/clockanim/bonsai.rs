use super::*;

/// `bonsai`: the tree, grown for so many panes (a limb each), and where
/// its leaves are in their sway.
#[derive(Clone, Debug, Default)]
pub(super) struct Tree {
    pub(super) limbs: Vec<Limb>,
    panes: usize,
    sway: f64,
}

/// A piece of the tree, in units square to the eye, the trunk's foot at
/// 0, 0 and up positive; which pane's limb it is on (none: the trunk); the
/// shares of the day's growth at which it starts and is grown.
#[derive(Clone, Debug)]
pub(super) struct Limb {
    from: (f64, f64),
    to: (f64, f64),
    pub(super) depth: u8,
    pub(super) pane: Option<usize>,
    start: f64,
    end: f64,
    tip: bool,
}

/// The most limbs (one a pane).
const LIMBS: usize = 6;
/// How fast a message's envelope falls from a leaf, rows a second.
const LEAF_FALL: f64 = 3.0;

/// The tree for `panes`: a leaning trunk in three pieces, a limb a pane
/// from its top spread left to right, each branching three times more.
fn grow(seed: u64, panes: usize) -> Vec<Limb> {
    let mut out = Vec::new();
    let (mut at, mut lean, mut path) = ((0.0, 0.0), (f64::from(unit(hash(seed, 71, 0))) - 0.5) * 0.5, 0.0);
    for k in 0..3 {
        lean = (lean + (f64::from(unit(hash(seed, 72, k))) - 0.5) * 0.7).clamp(-0.6, 0.6);
        let to = (at.0 + lean.sin() * 8.0, at.1 + lean.cos() * 8.0);
        out.push(Limb { from: at, to, depth: 0, pane: None, start: path, end: path + 8.0, tip: false });
        (at, path) = (to, path + 8.0);
    }
    for p in 0..panes {
        let spread = if panes == 1 { 0.0 } else { -1.0 + 2.0 * p as f64 / (panes - 1) as f64 };
        let a = spread + (f64::from(unit(hash(seed, 73, p as u64))) - 0.5) * 0.25;
        branch(&mut out, seed, at, a, 15.0, 1, p, path);
    }
    let longest = out.iter().map(|l| l.end).fold(1.0, f64::max);
    for l in &mut out {
        (l.start, l.end) = (l.start / longest, l.end / longest);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn branch(out: &mut Vec<Limb>, seed: u64, from: (f64, f64), angle: f64, len: f64, depth: u8, pane: usize, path: f64) {
    let hs = hash(seed, out.len() as u64, 74);
    let angle = (angle + (f64::from(unit(hs)) - 0.5) * 0.3).clamp(-1.45, 1.45);
    let to = (from.0 + angle.sin() * len, from.1 + angle.cos() * len);
    let tip = depth >= 4;
    out.push(Limb { from, to, depth, pane: Some(pane), start: path, end: path + len, tip });
    if tip {
        return;
    }
    let n = if hs.is_multiple_of(3) { 3 } else { 2 };
    for k in 0..n {
        let turn = 0.35 + 0.4 * f64::from(unit(hash(hs, k, 0)));
        let turn = match k {
            0 => -turn,
            1 => turn,
            _ => 0.0,
        };
        branch(out, seed, to, angle + turn, len * 0.68, depth + 1, pane, path + len);
    }
}

/// How much of `b` has grown by `grown` (none: nothing), and where it has
/// grown to.
fn reach(b: &Limb, grown: f64) -> Option<(f64, (f64, f64))> {
    let part = ((grown - b.start) / (b.end - b.start)).clamp(0.0, 1.0);
    (part > 0.0).then_some((part, (b.from.0 + (b.to.0 - b.from.0) * part, b.from.1 + (b.to.1 - b.from.1) * part)))
}

impl Anim {
    pub(super) fn step_bonsai(&mut self, dt: f64) {
        let panes = self.out_rates.len().clamp(1, LIMBS);
        let tree = &mut self.tree;
        if tree.panes != panes || tree.limbs.is_empty() {
            (tree.limbs, tree.panes) = (grow(self.seed, panes), panes);
        }
        tree.sway += dt * (1.2 + 2.0 * f64::from(self.input.load.switches.map_or(0.0, count_level)));
    }

    /// A tree in a pot, grown through the day (a sapling after midnight,
    /// most of it by the morning, all of it by night); a limb a pane (six
    /// at most), its leaves always stirring (quicker with the context
    /// switches), shaking the more the pane prints; more leaves the more
    /// memory is used, green, yellow or red as the CPU gets hotter, leaning
    /// in the network's wind; a leaf falling now and then, more with the
    /// disks; each message an `✉` falling from a leaf.
    pub(super) fn draw_bonsai(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let tree = &self.tree;
        let (w, h) = (i64::from(band.w), i64::from(band.h));
        let day = if self.input.day.is_finite() { self.input.day.rem_euclid(86_400.0) } else { 0.0 };
        let grown = 0.3 + 0.7 * (day / 86_400.0).sqrt();
        // The pot.
        let pot = if h >= 8 { 2 } else { 1 };
        let ground = h - pot - 1;
        let (left, right) = tree
            .limbs
            .iter()
            .fold((1.0f64, 1.0f64), |(l, r), b| (l.max(-b.to.0.min(b.from.0)), r.max(b.to.0.max(b.from.0))));
        let tall = tree.limbs.iter().map(|b| b.to.1).fold(1.0, f64::max);
        let scale = ((w - 2) as f64 / (2.0 * left.max(right) + 8.0)).min((ground + 1) as f64 * 2.0 / (tall + 5.0));
        let cx = w / 2;
        let at = |(x, y): (f64, f64)| (cx as f64 + 0.5 + x * scale, ground as f64 + 0.5 - y * scale / 2.0);
        let width = ((scale * 24.0).round() as i64).clamp(7, (w - 2).max(7));
        let (pot_style, moss) = (pal.shade(RED, 0.2), pal.shade(GREEN, 0.4));
        let x0 = cx - width / 2;
        for i in 0..width {
            let (mark, style) = match i {
                0 if pot == 2 => ("[", pot_style),
                _ if i == width - 1 && pot == 2 => ("]", pot_style),
                _ if pot == 2 => ("~", moss),
                0 => ("\\", pot_style),
                _ if i == width - 1 => ("/", pot_style),
                _ => ("_", pot_style),
            };
            put(g, band, x0 + i, h - pot, mark, style);
            if pot == 2 && (1..width - 1).contains(&i) {
                put(
                    g,
                    band,
                    x0 + i,
                    h - 1,
                    if i == 1 {
                        "\\"
                    } else if i == width - 2 {
                        "/"
                    } else {
                        "_"
                    },
                    pot_style,
                );
            }
        }
        // The wood, as far as it has grown.
        for b in &tree.limbs {
            let Some((_, end)) = reach(b, grown) else { continue };
            let (dx, dy) = (b.to.0 - b.from.0, b.to.1 - b.from.1);
            let lean = dx.atan2(dy);
            let mark = if lean.abs() < 0.4 {
                "|"
            } else if lean.abs() > 1.2 {
                "~"
            } else if lean > 0.0 {
                "/"
            } else {
                "\\"
            };
            let style = pal.shade(YELLOW, 0.25 + 0.1 * f32::from(b.depth));
            let ((ax, ay), (bx, by)) = (at(b.from), at(end));
            let steps = ((bx - ax).abs().max((by - ay).abs()) * 2.0).ceil() as i64 + 1;
            for k in 0..=steps {
                let f = k as f64 / steps as f64;
                let (x, y) = ((ax + (bx - ax) * f).floor() as i64, (ay + (by - ay) * f).floor() as i64);
                put(g, band, x, y, mark, style);
                if b.depth == 0 && scale >= 0.35 {
                    put(g, band, x + 1, y, mark, style);
                }
            }
        }
        // The leaves, where each branch has grown to (more on the tips,
        // filling in as it grows), shaking with their pane.
        let colour = heat(load);
        let memory = f64::from(share(load.memory.unwrap_or(0.4)));
        let wind = 1.5 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx)));
        let leafy: Vec<(usize, &Limb, f64, (f64, f64))> = tree
            .limbs
            .iter()
            .enumerate()
            .filter(|(_, b)| b.depth >= 1)
            .filter_map(|(i, b)| reach(b, grown).map(|(part, end)| (i, b, part, end)))
            .collect();
        for &(i, b, part, end) in &leafy {
            let rustle = f64::from(out_level(b.pane.and_then(|p| self.out_rates.get(p)).copied().unwrap_or(0.0)));
            let full = if b.tip { 3.0 + memory * 9.0 } else { 1.0 + memory * 5.0 };
            for k in 0..(full * part).round() as u64 {
                let hs = hash(self.seed, i as u64, 75 + k);
                let (ox, oy) = ((f64::from(unit(hs)) - 0.5) * 11.0, (f64::from(unit(hash(hs, 1, 0))) - 0.5) * 7.0);
                let (x, y) = at((end.0 + ox, end.1 + oy));
                let shake = (f64::from(noise(hs, tree.sway * (1.0 + 3.0 * rustle))) - 0.5) * 2.0 * (0.7 + 1.6 * rustle);
                let lift = (ground as f64 - y).max(0.0) / (ground + 1) as f64;
                let x = (x + shake + wind * lift).floor() as i64;
                let shade = if hs.is_multiple_of(3) { colour + 8 } else { colour };
                put(g, band, x, y.floor() as i64, if hs.is_multiple_of(7) { "*" } else { "&" }, pal.shade(shade, 0.0));
            }
        }
        // Where a falling leaf starts: a branch's leaves, else the wood's
        // top.
        let top = tree
            .limbs
            .iter()
            .filter_map(|b| reach(b, grown))
            .fold((0.0, 0.0), |a, (_, p)| if p.1 > a.1 { p } else { a });
        let from = |k: u64| leafy.get(k as usize % leafy.len().max(1)).map_or(top, |l| l.3);
        let disk = load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr));
        for i in 0..1 + (disk * 7.0).round() as u64 {
            let hs = hash(self.seed, i, 77);
            let along = self.t * 0.5 + f64::from(unit(hs));
            let (k, age) = (along.floor() as u64, along.fract());
            let (x, y) = at(from(hash(hs, k, 0)));
            let y = y + (ground as f64 + 0.5 - y) * age;
            let x = x + (age * 12.0 + f64::from(unit(hs)) * 6.0).sin() * 1.5;
            put(g, band, x.floor() as i64, y.floor() as i64, ",", pal.shade(colour, 0.3));
        }
        for &(n, at_t) in &self.drops {
            let (x, y) = at(from(hash(self.seed, n, 76)));
            let y = y + (self.t - at_t) * LEAF_FALL;
            if y >= ground as f64 + 1.0 {
                continue;
            }
            let x = x + ((self.t - at_t) * 3.0).sin() * 1.5;
            put(
                g,
                band,
                x.floor() as i64,
                y.floor() as i64,
                ENVELOPE,
                Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) },
            );
        }
    }
}
