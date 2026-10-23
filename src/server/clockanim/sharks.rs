use super::*;
use std::f64::consts::{PI, TAU};

/// The sea `sharks` swims in, in square units (a column wide, half a row
/// tall), whatever the band: the band is stretched over it.
const SEA_W: f64 = 100.0;
const SEA_H: f64 = 60.0;
/// Fish in a school, and the most schools (one a pane).
const SCHOOL: usize = 9;
const SCHOOLS: usize = 6;
/// The snout's path: a point each `TRAIL_STEP` units it swims, the last
/// `TRAIL` of them (longer than any shark).
const TRAIL_STEP: f64 = 0.5;
const TRAIL: usize = 320;
/// The points of the shark's spine, snout to tail.
const SPINE: usize = 20;
/// How long a message's ripple spreads, seconds.
const RIPPLE: f64 = 3.0;

/// `sharks`: a school of fish a pane of the clock's window and a shark
/// hunting them, seen from above.
#[derive(Clone, Debug, Default)]
pub(super) struct Sea {
    pub(super) fish: Vec<Fish>,
    pub(super) shark: Shark,
    /// Where the snout has been, newest first: the body follows it.
    trail: VecDeque<(f64, f64)>,
    /// Where a fish was eaten, and when; how many have been.
    splashes: VecDeque<(f64, f64, f64)>,
    pub(super) meals: u64,
    /// The water's glints: a new few each tenth of this.
    glints: f64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Fish {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) vx: f64,
    pub(super) vy: f64,
    pub(super) school: usize,
    /// 0 calm to 1 fleeing; it spreads through the school.
    pub(super) alarm: f64,
    /// Eaten: when it swims in again.
    pub(super) back: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Shark {
    pub(super) x: f64,
    pub(super) y: f64,
    heading: f64,
    pub(super) speed: f64,
    /// 0 fed to 1 starving: it hunts from a half.
    pub(super) hunger: f64,
    /// Where the tail is in its beat.
    beat: f64,
}

/// `a` less `b`, turned into -π to π.
fn angle_diff(a: f64, b: f64) -> f64 {
    (a - b + PI).rem_euclid(TAU) - PI
}

/// How hard to turn back from the sea's edge at `p` of `size`, `margin`
/// units in: 0 well inside, 1 at the edge, toward the middle.
fn edge(p: f64, size: f64, margin: f64) -> f64 {
    if p < margin {
        (margin - p) / margin
    } else if p > size - margin {
        -(p - (size - margin)) / margin
    } else {
        0.0
    }
}

/// Where message `n` lands, in the sea.
pub(super) fn landing(seed: u64, n: u64) -> (f64, f64) {
    (
        SEA_W * (0.15 + 0.7 * f64::from(unit(hash(seed, n, 65)))),
        SEA_H * (0.2 + 0.6 * f64::from(unit(hash(seed, n, 66)))),
    )
}

/// The shark's half width, a share of its length, `s` along it (snout 0,
/// tail 1): a round snout, widest a fifth along, a slim stalk.
fn girth(s: f64) -> f64 {
    if !(0.0..=1.0).contains(&s) {
        0.0
    } else if s < 0.2 {
        0.095 * (s / 0.2).sqrt()
    } else if s < 0.86 {
        0.083 * (1.0 - (s - 0.2) / 0.66).powf(1.4) + 0.012
    } else {
        0.012
    }
}

/// How far its fins reach past the body, `s` along it: the pectorals,
/// swept back, then the pelvic pair; and the tail's fluke from its middle.
fn fins(s: f64) -> f64 {
    let side = if (0.22..0.46).contains(&s) {
        0.17 * (s - 0.22) / 0.24
    } else if (0.46..0.5).contains(&s) {
        0.17 * (0.5 - s) / 0.04
    } else if (0.62..0.7).contains(&s) {
        0.04 * (s - 0.62) / 0.08
    } else {
        0.0
    };
    let fluke = if (0.88..1.1).contains(&s) { 0.015 + 0.06 * (s - 0.88) / 0.22 } else { 0.0 };
    (girth(s) + side).max(fluke)
}

/// Where `p` is by the spine: how far along (0 snout, 1 tail; a little
/// past either end), and how far to one side (signed).
fn by_spine(spine: &[(f64, f64)], p: (f64, f64)) -> (f64, f64) {
    let last = spine.len() - 2;
    let mut best = (0.0, f64::INFINITY);
    for j in 0..=last {
        let (a, b) = (spine[j], spine[j + 1]);
        let (ux, uy) = (b.0 - a.0, b.1 - a.1);
        let l2 = ux * ux + uy * uy;
        if l2 < 1e-12 {
            continue;
        }
        let (lo, hi) = (if j == 0 { -0.5 } else { 0.0 }, if j == last { 2.5 } else { 1.0 });
        let t = (((p.0 - a.0) * ux + (p.1 - a.1) * uy) / l2).clamp(lo, hi);
        let (qx, qy) = (a.0 + ux * t, a.1 + uy * t);
        let d = ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt();
        if d < best.1.abs() {
            let side = if ux * (p.1 - a.1) - uy * (p.0 - a.0) < 0.0 { -1.0 } else { 1.0 };
            best = ((j as f64 + t) / (spine.len() - 1) as f64, d * side);
        }
    }
    best
}

impl Anim {
    pub(super) fn step_sharks(&mut self, dt: f64) {
        let load = &self.input.load;
        let (seed, cpu) = (self.seed, f64::from(share(load.cpu)));
        let current = 5.0 * f64::from(load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx)));
        let jitter = 20.0 + 60.0 * f64::from(load.switches.map_or(0.0, count_level));
        let schools = self.out_rates.len().clamp(1, SCHOOLS);
        let sea = &mut self.sea;
        // A pane more or fewer: the schools again (the shark stays).
        if sea.fish.len() != schools * SCHOOL {
            sea.fish = (0..schools * SCHOOL)
                .map(|i| {
                    let k = (i / SCHOOL) as u64;
                    let (cx, cy) = (
                        SEA_W * (0.15 + 0.7 * f64::from(unit(hash(seed, k, 62)))),
                        SEA_H * (0.15 + 0.7 * f64::from(unit(hash(seed, k, 63)))),
                    );
                    let a = f64::from(unit(hash(seed, k, 64))) * TAU;
                    let hs = hash(seed, i as u64, 60);
                    Fish {
                        x: cx + (f64::from(unit(hs)) - 0.5) * 10.0,
                        y: cy + (f64::from(unit(hash(hs, 1, 0))) - 0.5) * 8.0,
                        vx: a.cos() * 8.0,
                        vy: a.sin() * 8.0,
                        school: i / SCHOOL,
                        ..Fish::default()
                    }
                })
                .collect();
        }
        if sea.trail.is_empty() {
            let heading = f64::from(unit(hash(seed, 61, 0))) * TAU;
            sea.shark = Shark { x: SEA_W * 0.5, y: SEA_H * 0.5, heading, speed: 6.0, ..Shark::default() };
            sea.trail = (0..TRAIL)
                .map(|i| {
                    let back = i as f64 * TRAIL_STEP;
                    (SEA_W * 0.5 - heading.cos() * back, SEA_H * 0.5 - heading.sin() * back)
                })
                .collect();
        }
        sea.glints += dt * 0.3;
        let n = (dt * 30.0).ceil().max(1.0) as usize;
        let h = dt / n as f64;
        for k in 0..n {
            let now = self.t - dt + (k + 1) as f64 * h;
            // The newest message still spreading draws the shark to it.
            let lure = self
                .drops
                .iter()
                .rev()
                .find(|&&(_, at)| (0.0..RIPPLE).contains(&(now - at)))
                .map(|&(n, _)| landing(seed, n));
            let sh = &mut sea.shark;
            sh.hunger = (sh.hunger + h * (0.03 + 0.25 * cpu)).min(1.0);
            let hunting = lure.is_none() && sh.hunger > 0.5;
            let prey = if hunting {
                sea.fish
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.back.is_none())
                    .min_by(|(_, a), (_, b)| {
                        let d = |f: &Fish| (f.x - sh.x).powi(2) + (f.y - sh.y).powi(2);
                        d(a).total_cmp(&d(b))
                    })
                    .map(|(i, f)| (i, f.x, f.y))
            } else {
                None
            };
            let toward = match (lure, prey) {
                (Some((x, y)), _) | (None, Some((_, x, y))) => (y - sh.y).atan2(x - sh.x),
                _ => sh.heading + (f64::from(noise(hash(seed, 67, 0), now * 0.15)) - 0.5) * 2.0,
            };
            let (px, py) = (edge(sh.x, SEA_W, 15.0), edge(sh.y, SEA_H, 15.0));
            let want = (toward.sin() + 2.0 * py).atan2(toward.cos() + 2.0 * px);
            let rate = if lure.is_some() {
                2.4
            } else if hunting {
                1.5
            } else {
                0.9
            };
            sh.heading += angle_diff(want, sh.heading).clamp(-rate * h, rate * h);
            // A burst for a fish close by, and toward a message, slowing as
            // it gets there.
            let close = prey.is_some_and(|(_, x, y)| (x - sh.x).powi(2) + (y - sh.y).powi(2) < 12.0 * 12.0);
            let burst = match lure {
                Some((x, y)) => 14.0 * (((x - sh.x).powi(2) + (y - sh.y).powi(2)).sqrt() / 20.0).min(1.0),
                None if close => 14.0,
                None => 0.0,
            };
            let pace = 6.0 + 10.0 * sh.hunger + burst;
            sh.speed += (pace - sh.speed) * (1.0 - (-h / 0.6).exp());
            sh.x = (sh.x + (sh.heading.cos() * sh.speed + current) * h).clamp(1.0, SEA_W - 1.0);
            sh.y = (sh.y + sh.heading.sin() * sh.speed * h).clamp(1.0, SEA_H - 1.0);
            sh.beat += h * (2.0 + 0.35 * sh.speed);
            let (sx, sy) = (sh.x, sh.y);
            if sea.trail.front().is_none_or(|&(x, y)| (x - sx).powi(2) + (y - sy).powi(2) >= TRAIL_STEP * TRAIL_STEP) {
                sea.trail.push_front((sx, sy));
                sea.trail.truncate(TRAIL);
            }
            // Caught: the fish is gone a while, the shark fed.
            if let Some((i, x, y)) = prey
                && (x - sx).powi(2) + (y - sy).powi(2) < 2.5 * 2.5
            {
                sea.fish[i].back = Some(now + 6.0);
                sea.shark.hunger = 0.0;
                sea.meals += 1;
                sea.splashes.push_back((x, y, now));
            }
            while sea.splashes.front().is_some_and(|&(_, _, at)| now - at > 1.0) {
                sea.splashes.pop_front();
            }
            // The fish: apart from each other, along with and toward their
            // school, wandering, away from the shark and the edges.
            let was: Vec<(f64, f64, f64, f64, usize, f64, bool)> =
                sea.fish.iter().map(|f| (f.x, f.y, f.vx, f.vy, f.school, f.alarm, f.back.is_none())).collect();
            for (i, f) in sea.fish.iter_mut().enumerate() {
                if let Some(back) = f.back {
                    if now < back {
                        continue;
                    }
                    // Back in from a side, by its school.
                    let left = hash(seed, i as u64, sea.meals) & 1 == 0;
                    let (_, cy, n) = was
                        .iter()
                        .filter(|o| o.4 == f.school && o.6)
                        .fold((0.0, 0.0, 0.0), |(x, y, n), o| (x + o.0, y + o.1, n + 1.0));
                    f.y = if n > 0.0 { cy / n } else { SEA_H / 2.0 };
                    (f.x, f.vx, f.vy, f.alarm, f.back) =
                        (if left { 0.5 } else { SEA_W - 0.5 }, if left { 8.0 } else { -8.0 }, 0.0, 0.0, None);
                }
                let rate = self.out_rates.get(f.school).copied().unwrap_or(0.0);
                let cruise = 8.0 + 18.0 * f64::from(out_level(rate));
                let (mut ax, mut ay) = (0.0, 0.0);
                let (mut mx, mut my, mut mvx, mut mvy, mut mates, mut heard) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0f64);
                for (j, o) in was.iter().enumerate() {
                    if j == i || !o.6 {
                        continue;
                    }
                    let (dx, dy) = (f.x - o.0, f.y - o.1);
                    let d2 = dx * dx + dy * dy;
                    if d2 < 9.0 && d2 > 1e-9 {
                        (ax, ay) = (ax + dx / d2 * 8.0, ay + dy / d2 * 8.0);
                    }
                    if o.4 == f.school && d2 < 196.0 {
                        (mx, my, mvx, mvy, mates) = (mx + o.0, my + o.1, mvx + o.2, mvy + o.3, mates + 1.0);
                        if d2 < 36.0 {
                            heard = heard.max(o.5 * 0.85);
                        }
                    }
                }
                if mates > 0.0 {
                    ax += (mvx / mates - f.vx) * 1.5 + (mx / mates - f.x) * 1.4;
                    ay += (mvy / mates - f.vy) * 1.5 + (my / mates - f.y) * 1.4;
                }
                let v = (f.vx * f.vx + f.vy * f.vy).sqrt().max(1e-6);
                let swerve = (f64::from(noise(hash(seed, i as u64, 68), now * 0.7)) - 0.5) * jitter;
                (ax, ay) = (ax - f.vy / v * swerve, ay + f.vx / v * swerve);
                let (dx, dy) = (f.x - sx, f.y - sy);
                let d = (dx * dx + dy * dy).sqrt().max(1e-6);
                f.alarm = (f.alarm * (-h / 1.5).exp()).max(heard);
                if d < 16.0 {
                    let push = (16.0 - d) / 16.0 * 90.0;
                    (ax, ay, f.alarm) = (ax + dx / d * push, ay + dy / d * push, 1.0);
                }
                ax += edge(f.x, SEA_W, 6.0) * 40.0;
                ay += edge(f.y, SEA_H, 6.0) * 40.0;
                (f.vx, f.vy) = (f.vx + ax * h, f.vy + ay * h);
                let v = (f.vx * f.vx + f.vy * f.vy).sqrt();
                let (lo, hi) = (cruise * 0.5, cruise * (1.0 + 1.8 * f.alarm));
                if v < 1e-6 {
                    (f.vx, f.vy) = (lo, 0.0);
                } else if v > hi || v < lo {
                    let k = v.clamp(lo, hi) / v;
                    (f.vx, f.vy) = (f.vx * k, f.vy * k);
                }
                f.x = (f.x + (f.vx + current) * h).clamp(0.5, SEA_W - 0.5);
                f.y = (f.y + f.vy * h).clamp(0.5, SEA_H - 0.5);
            }
        }
    }

    /// Glints on the water, more with the disks; a school a pane (six at
    /// most; faster the more it prints, turning more with the context
    /// switches, all of it drifting with the network); the shark over
    /// them, hungrier and quicker the busier the cores, longer with memory,
    /// its eyes in the CPU's colour, its wake behind; a splash where it
    /// eats; each message a ripple with `✉` in it, the shark racing to it.
    pub(super) fn draw_sharks(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        let load = &self.input.load;
        let sea = &self.sea;
        let (w, h) = (f64::from(band.w), f64::from(band.h));
        // Sea units to square units of the band.
        let (kx, ky) = (w / SEA_W, h * 2.0 / SEA_H);
        let square = |(x, y): (f64, f64)| (x * kx, y * ky);
        let cell = |(x, y): (f64, f64)| (x.floor() as i64, (y / 2.0).floor() as i64);
        // Glints on the water, a third of a second each, more with the disks.
        let (tick, glints) =
            ((sea.glints * 10.0).floor() as u64, 0.002 + 0.012 * load.disk.map_or(0.0, |(r, wr)| bytes_level(r + wr)));
        for y in 0..i64::from(band.h) {
            for x in 0..i64::from(band.w) {
                if unit(hash(self.seed ^ tick, x as u64, y as u64 + 69)) < glints {
                    put(g, band, x, y, ".", pal.shade(CYAN, 0.5));
                }
            }
        }
        let Some(_) = sea.trail.front() else { return };
        let sh = &sea.shark;
        let path: Vec<(f64, f64)> =
            std::iter::once((sh.x, sh.y)).chain(sea.trail.iter().copied()).map(square).collect();
        let memory = f64::from(share(load.memory.unwrap_or(0.4)));
        // Never past 80 columns (92 with all the memory used): the cells
        // it is drawn in grow as its square.
        let len = ((w * 0.42).min(h * 2.0 * 1.6).min(80.0) * (0.75 + 0.4 * memory)).max(6.0);
        // The spine: points along the path, `len` in all, the tail beating.
        let mut spine = vec![path[0]];
        let (mut seg, mut at) = (0, path[0]);
        for _ in 1..SPINE {
            let mut need = len / (SPINE - 1) as f64;
            loop {
                let Some(&next) = path.get(seg + 1) else {
                    // Past the path's end: straight on.
                    let (a, b) = (path[seg.saturating_sub(1)], path[seg]);
                    let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                    let (ux, uy) = if l > 1e-9 { ((b.0 - a.0) / l, (b.1 - a.1) / l) } else { (-1.0, 0.0) };
                    at = (at.0 + ux * need, at.1 + uy * need);
                    break;
                };
                let d = ((next.0 - at.0).powi(2) + (next.1 - at.1).powi(2)).sqrt();
                if d >= need {
                    at = (at.0 + (next.0 - at.0) * need / d, at.1 + (next.1 - at.1) * need / d);
                    break;
                }
                need -= d;
                (at, seg) = (next, seg + 1);
            }
            spine.push(at);
        }
        // Its wake: the path behind the tail, fading.
        let mut gone = 0.0;
        for p in path.windows(2) {
            gone += ((p[1].0 - p[0].0).powi(2) + (p[1].1 - p[0].1).powi(2)).sqrt();
            if gone > len * 1.6 {
                break;
            }
            if gone > len * 1.05 && (gone / 2.0).fract() < 0.25 {
                let (x, y) = cell(p[1]);
                put(g, band, x, y, "~", pal.shade(CYAN, (0.35 + (gone - len) / len).min(0.9) as f32));
            }
        }
        let beat = sh.beat;
        let wave: Vec<(f64, f64)> = (0..SPINE)
            .map(|i| {
                let s = i as f64 / (SPINE - 1) as f64;
                let (a, b) = (spine[i.saturating_sub(1)], spine[(i + 1).min(SPINE - 1)]);
                let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt().max(1e-9);
                let off = len * 0.06 * s.powf(1.6) * (beat - s * 5.0).sin();
                (spine[i].0 - (b.1 - a.1) / l * off, spine[i].1 + (b.0 - a.0) / l * off)
            })
            .collect();
        // Each message: rings spreading from where it landed.
        for &(n, at) in &self.drops {
            let age = self.t - at;
            if !(0.0..RIPPLE).contains(&age) {
                continue;
            }
            let (cx, cy) = square(landing(self.seed, n));
            let r = 1.5 + age * 5.0;
            let dots = (TAU * r / 1.5) as usize + 8;
            for k in 0..dots {
                let a = k as f64 / dots as f64 * TAU;
                let (x, y) = cell((cx + r * a.cos(), cy + r * a.sin()));
                put(g, band, x, y, "o", pal.shade(CYAN, (0.2 + 0.7 * age / RIPPLE) as f32));
            }
        }
        for f in sea.fish.iter().filter(|f| f.back.is_none()) {
            let (x, y) = cell(square((f.x, f.y)));
            let (dx, dy) = (f.vx * kx, f.vy * ky);
            let mark = if dx.abs() >= dy.abs() {
                if dx >= 0.0 { ">" } else { "<" }
            } else if dy >= 0.0 {
                "v"
            } else {
                "^"
            };
            let colour = FISH_COLOURS[f.school % FISH_COLOURS.len()];
            put(g, band, x, y, mark, Style { bold: f.alarm > 0.5, ..pal.shade(colour, 0.0) });
        }
        // The shark, cell by cell: four points in each, as many inside as
        // there are.
        let reach = len * 0.3;
        let (x0, x1) = wave.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = wave.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let (cx0, cx1) =
            (((x0 - reach).floor() as i64).max(0), ((x1 + reach).ceil() as i64).min(i64::from(band.w) - 1));
        let (cy0, cy1) = (
            (((y0 - reach) / 2.0).floor() as i64).max(0),
            (((y1 + reach) / 2.0).ceil() as i64).min(i64::from(band.h) - 1),
        );
        let eye = heat(load);
        for y in cy0..=cy1 {
            for x in cx0..=cx1 {
                let (xf, yf) = (x as f64, y as f64 * 2.0);
                // Inside: in the body (how deep: 1 on the spine) or a fin.
                let inside = |p: (f64, f64)| {
                    let (s, d) = by_spine(&wave, p);
                    let a = d.abs() / len;
                    let gi = girth(s);
                    if a < gi {
                        Some(1.0 - a / gi)
                    } else if a < fins(s) {
                        Some(-1.0)
                    } else {
                        None
                    }
                };
                let quarters =
                    [(0.25, 0.5), (0.75, 0.5), (0.25, 1.5), (0.75, 1.5)].map(|(dx, dy)| inside((xf + dx, yf + dy)));
                let n = quarters.iter().flatten().count();
                if n == 0 {
                    continue;
                }
                let deep = quarters.iter().flatten().fold(-1.0f64, |a, &b| a.max(b));
                let (top, bottom) = (quarters[..2].iter().flatten().count(), quarters[2..].iter().flatten().count());
                let (s, d) = by_spine(&wave, (xf + 0.5, yf + 1.0));
                let side = d.abs() / len / girth(s).max(1e-9);
                let (mark, style) = if (0.05..0.11).contains(&s) && (0.35..0.85).contains(&side) {
                    ("o", Style { bold: true, ..pal.shade(eye, 0.0) })
                } else if n < 4 {
                    let mark = if top > bottom {
                        "'"
                    } else if bottom > top {
                        "."
                    } else {
                        ":"
                    };
                    (mark, pal.shade(WHITE, if deep >= 0.0 { 0.25 } else { 0.4 }))
                } else if deep < 0.0 {
                    (";", pal.shade(WHITE, 0.15))
                } else if deep >= 0.8 {
                    ("@", Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) })
                } else if deep >= 0.55 {
                    ("#", pal.shade(BRIGHT_WHITE, 0.0))
                } else if deep >= 0.3 {
                    ("*", pal.shade(WHITE, 0.0))
                } else {
                    ("+", pal.shade(WHITE, 0.1))
                };
                put(g, band, x, y, mark, style);
            }
        }
        for &(x, y, at) in &sea.splashes {
            let age = self.t - at;
            if !(0.0..0.6).contains(&age) {
                continue;
            }
            let (x, y) = cell(square((x, y)));
            put(g, band, x, y, "*", Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            if age < 0.35 {
                put(g, band, x - 1, y, "'", pal.shade(CYAN, 0.2));
                put(g, band, x + 1, y, "'", pal.shade(CYAN, 0.2));
            }
        }
        for &(n, at) in &self.drops {
            if (0.0..RIPPLE).contains(&(self.t - at)) {
                let (x, y) = cell(square(landing(self.seed, n)));
                put(g, band, x, y, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
            }
        }
    }
}
