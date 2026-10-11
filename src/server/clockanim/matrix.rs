use super::*;

impl Anim {
    pub(super) fn step_matrix(&mut self, dt: f64) {
        let load = &self.input.load;
        // Faster rain the busier the cores; quicker flicker the more the
        // disks do.
        self.travel += dt * (0.6 + 1.6 * f64::from(load.cpu));
        let disk = load.disk.map_or(0.0, |(r, w)| bytes_level(r + w));
        self.flicker += dt * (2.0 + 18.0 * f64::from(disk));
    }

    pub(super) fn draw_matrix(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        // Half-width katakana and digits, each one column wide.
        const CHARS: &str = "ｦｧｨｩｪｫｬｭｮｯｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789";
        let chars: Vec<char> = CHARS.chars().collect();
        let load = &self.input.load;
        // More columns fall the more memory is in use; the more the network
        // carries, the more of them are cyan.
        let density = 0.25 + 0.6 * load.memory.unwrap_or(0.4);
        let packets = 0.03 + 0.5 * load.net.map_or(0.0, |(rx, tx)| bytes_level(rx + tx));
        let colour = heat(load);
        let h = f64::from(band.h);
        // Under the rain, interrupts: stray characters in magenta, more the
        // more come, new ones ten times a second.
        let strays = (load.interrupts.map_or(0.0, count_level) * f32::from(band.w) * f32::from(band.h) * 0.04) as u64;
        let slot = (self.t * 10.0).floor() as u64;
        for i in 0..strays {
            let hs = hash(self.seed, slot, i + 100);
            let (x, y) = ((hs % u64::from(band.w)) as u16, ((hs >> 20) % u64::from(band.h)) as u16);
            let ch = chars[((hs >> 40) % chars.len() as u64) as usize];
            g.set(band.x + x, band.y + y, Cell::new(ch.encode_utf8(&mut [0; 4]), false, pal.shade(MAGENTA, 0.4)));
        }
        for c in 0..band.w {
            let hc = hash(self.seed, u64::from(c), 1);
            let speed = 6.0 + 10.0 * f64::from(unit(hash(hc, 1, 0)));
            let len = 4.0 + f64::from(unit(hash(hc, 2, 0))) * (h * 0.8 + 4.0);
            let cycle = h + len + f64::from(unit(hash(hc, 3, 0))) * h;
            let pos = self.travel * speed + f64::from(unit(hash(hc, 4, 0))) * cycle;
            let k = (pos / cycle).floor();
            if unit(hash(hc, k as u64, 5)) >= density {
                continue;
            }
            let packet = unit(hash(hc, k as u64, 6)) < packets;
            let head = (pos - k * cycle).floor() as i64;
            for d in 0..len as i64 {
                let y = head - d;
                if y < 0 || y >= i64::from(band.h) {
                    continue;
                }
                let hy = hash(hc, y as u64, 7);
                let slot = (self.flicker * (0.5 + f64::from(unit(hy)))).floor() as u64;
                let ch = chars[(hash(hy, slot, k as u64) % chars.len() as u64) as usize];
                let style = if d == 0 {
                    Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) }
                } else {
                    pal.shade(if packet { CYAN } else { colour }, d as f32 / len as f32 * 0.85)
                };
                g.set(band.x + c, band.y + y as u16, Cell::new(ch.encode_utf8(&mut [0; 4]), false, style));
            }
        }
        // Each message delivered: a white drop headed by an envelope, over
        // the rain.
        const TAIL: i64 = 6;
        for &(n, at) in &self.drops {
            let c = (hash(self.seed, n, 11) % u64::from(band.w)) as u16;
            let head = ((self.t - at) * DROP_SPEED).floor() as i64;
            for d in 0..TAIL {
                let y = head - d;
                if y < 0 || y >= i64::from(band.h) {
                    continue;
                }
                let (text, style) = if d == 0 {
                    (ENVELOPE.to_string(), Style { bold: true, ..pal.shade(BRIGHT_WHITE, 0.0) })
                } else {
                    let ch = chars[(hash(n, y as u64, 12) % chars.len() as u64) as usize];
                    (ch.to_string(), pal.shade(BRIGHT_WHITE, d as f32 / TAIL as f32 * 0.8))
                };
                g.set(band.x + c, band.y + y as u16, Cell::new(&text, false, style));
            }
        }
    }
}
