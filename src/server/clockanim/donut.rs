use super::*;
use std::f64::consts::TAU;

/// `donut`: its two angles (radians), and the turns messages still owe it.
#[derive(Clone, Debug, Default)]
pub(super) struct Donut {
    pub(super) a: f64,
    pub(super) b: f64,
    pub(super) owed: f64,
}

/// How fast a message's turn goes: one in this many seconds.
const TURN: f64 = 0.6;

impl Anim {
    pub(super) fn step_donut(&mut self, dt: f64) {
        let load = &self.input.load;
        let d = &mut self.donut;
        // Each message: one turn more, quick; never more than 4 owed.
        d.owed = (d.owed + TAU * self.arrived as f64).min(4.0 * TAU);
        let extra = d.owed.min(dt * TAU / TURN);
        d.owed -= extra;
        // Half a radian a second idle, 3 with every core busy.
        let speed = 0.5 + 2.5 * f64::from(share(load.cpu));
        d.a = (d.a + dt * speed + extra).rem_euclid(TAU);
        d.b = (d.b + (dt * speed + extra) * 0.5).rem_euclid(TAU);
    }

    /// A torus turning (the faster the busier the cores), lit from above
    /// and behind, its tube the thicker the more memory is used, in the
    /// CPU's colour; a message turns it once more, an envelope in its hole.
    pub(super) fn draw_donut(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        const SHADES: &[u8] = b".,-~:;=!*#$@";
        let load = &self.input.load;
        let (w, h) = (usize::from(band.w), usize::from(band.h));
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        // A cell is about twice as tall as it is wide.
        let size = (w as f64 / 2.0).min(h as f64);
        let r1 = 0.6 + 0.6 * f64::from(share(load.memory.unwrap_or(0.4)));
        let (r2, k2) = (2.0, 5.0);
        let k1 = size * k2 * 3.0 / (8.0 * (r1 + r2));
        let (sa, ca, sb, cb) = (self.donut.a.sin(), self.donut.a.cos(), self.donut.b.sin(), self.donut.b.cos());
        let colour = heat(load);
        let mut depth = vec![0.0f64; w * h];
        let mut lit = vec![None::<usize>; w * h];
        let thetas = ((size * 3.0) as usize).clamp(24, 120);
        let phis = ((size * 10.0) as usize).clamp(80, 400);
        for i in 0..thetas {
            let (st, ct) = (TAU * i as f64 / thetas as f64).sin_cos();
            for j in 0..phis {
                let (sp, cp) = (TAU * j as f64 / phis as f64).sin_cos();
                let (cx2, cy2) = (r2 + r1 * ct, r1 * st);
                let x = cx2 * (cb * cp + sa * sb * sp) - cy2 * ca * sb;
                let y = cx2 * (sb * cp - sa * cb * sp) + cy2 * ca * cb;
                let ooz = 1.0 / (k2 + ca * cx2 * sp + cy2 * sa);
                let (px, py) = (cx + 2.0 * k1 * ooz * x, cy - k1 * ooz * y);
                if px < 0.0 || py < 0.0 || px >= w as f64 || py >= h as f64 {
                    continue;
                }
                let at = py as usize * w + px as usize;
                let l = cp * ct * sb - ca * ct * sp - sa * st + cb * (ca * st - ct * sa * sp);
                if ooz > depth[at] {
                    depth[at] = ooz;
                    lit[at] = Some(((l.max(0.0) * 8.0) as usize).min(SHADES.len() - 1));
                }
            }
        }
        for (i, shade) in lit.into_iter().enumerate() {
            let Some(s) = shade else { continue };
            let ch = &SHADES[s..=s];
            let style = if s >= 9 {
                Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) }
            } else {
                pal.shade(colour, (1.0 - s as f32 / 9.0) * 0.6)
            };
            put(g, band, (i % w) as i64, (i / w) as i64, std::str::from_utf8(ch).unwrap_or("."), style);
        }
        if self.donut.owed > 0.0 || self.drops.back().is_some_and(|&(_, at)| self.t - at < 1.5) {
            put(g, band, cx as i64, cy as i64, ENVELOPE, Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) });
        }
    }
}
