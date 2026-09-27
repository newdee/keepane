//! 24-bit colour for a terminal that has none. keepane's default look and
//! many programs in panes use `#rrggbb` colours; a terminal without 24-bit
//! colour (macOS's Terminal before it said `COLORTERM=truecolor`) shows
//! those sequences as the wrong colours, so there each one becomes the
//! nearest of the 256, as tmux does for such a terminal.

/// Whether the terminal the environment describes shows 24-bit colour.
/// Only a terminal known not to is answered no: a wrong no would dull every
/// colour on a terminal that has them all.
pub fn host_has_truecolor(var: impl Fn(&str) -> Option<String>) -> bool {
    if var("COLORTERM").is_some_and(|c| c == "truecolor" || c == "24bit") {
        return true;
    }
    var("TERM_PROGRAM").is_none_or(|p| p != "Apple_Terminal")
}

/// Rewrites the 24-bit colours in a stream of terminal output to the 256,
/// keeping a sequence cut between two writes until it is whole.
#[derive(Default)]
pub struct Downgrade {
    pending: Vec<u8>,
}

/// A sequence longer than this is not one of ours: it goes out as it came.
const MAX_SEQ: usize = 128;

impl Downgrade {
    pub fn filter(&mut self, input: &[u8]) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(input);
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                // Up to the next escape as it is.
                let next = data[i..].iter().position(|&b| b == 0x1b).map_or(data.len(), |p| i + p);
                out.extend_from_slice(&data[i..next]);
                i = next;
                continue;
            }
            // ESC [ params intermediates final
            let Some(&b1) = data.get(i + 1) else {
                self.pending = data[i..].to_vec();
                break;
            };
            if b1 != b'[' {
                out.push(0x1b);
                i += 1;
                continue;
            }
            let body = i + 2;
            match data[body..].iter().position(|b| (0x40..=0x7e).contains(b)) {
                None if data.len() - i <= MAX_SEQ => {
                    self.pending = data[i..].to_vec();
                    break;
                }
                None => {
                    out.extend_from_slice(&data[i..]);
                    break;
                }
                Some(p) => {
                    let end = body + p;
                    if data[end] == b'm' {
                        out.extend_from_slice(b"\x1b[");
                        out.extend_from_slice(sgr(&data[body..end]).as_bytes());
                        out.push(b'm');
                    } else {
                        out.extend_from_slice(&data[i..=end]);
                    }
                    i = end + 1;
                }
            }
        }
        out
    }
}

/// The parameters of one SGR sequence with its 24-bit colours made 256.
fn sgr(params: &[u8]) -> String {
    let params = String::from_utf8_lossy(params);
    let parts: Vec<&str> = params.split(';').collect();
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        let p = parts[i];
        // 38;2;r;g;b and 48;2;r;g;b
        if (p == "38" || p == "48") && parts.get(i + 1) == Some(&"2") && i + 4 < parts.len() {
            let rgb: Vec<u8> = parts[i + 2..i + 5].iter().filter_map(|c| c.parse().ok()).collect();
            if let [r, g, b] = rgb[..] {
                out.push(format!("{p};5;{}", nearest_256(r, g, b)));
                i += 5;
                continue;
            }
        }
        // 38:2::r:g:b (with the colour space) and 38:2:r:g:b
        if let Some(rest) = p.strip_prefix("38:2:").or_else(|| p.strip_prefix("48:2:")) {
            let nums: Vec<u8> = rest.split(':').filter(|c| !c.is_empty()).filter_map(|c| c.parse().ok()).collect();
            if nums.len() >= 3 {
                let [r, g, b] = [nums[nums.len() - 3], nums[nums.len() - 2], nums[nums.len() - 1]];
                out.push(format!("{}:5:{}", &p[..2], nearest_256(r, g, b)));
                i += 1;
                continue;
            }
        }
        out.push(p.to_string());
        i += 1;
    }
    out.join(";")
}

/// The nearest of the 256 colours' 6x6x6 cube and grey ramp (16-255; the
/// first 16 are the terminal's own and could be anything).
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let dist = |a: (u8, u8, u8)| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a.0, r) + d(a.1, g) + d(a.2, b)
    };
    let level = |v: u8| LEVELS.iter().enumerate().min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).abs()).unwrap().0;
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (16 + 36 * ri + 6 * gi + bi) as u8;
    let cube_rgb = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gi = (avg.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * gi;
    if dist((grey, grey, grey)) < dist(cube_rgb) { 232 + gi } else { cube }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_terminal_known_to_lack_it_is_told_no() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert!(host_has_truecolor(env(&[])));
        assert!(host_has_truecolor(env(&[("TERM_PROGRAM", "iTerm.app")])));
        assert!(!host_has_truecolor(env(&[("TERM_PROGRAM", "Apple_Terminal")])));
        // A Terminal that says it has them (newer macOS) is believed.
        assert!(host_has_truecolor(env(&[("TERM_PROGRAM", "Apple_Terminal"), ("COLORTERM", "truecolor")])));
    }

    #[test]
    fn the_nearest_of_the_256() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(0x87, 0xaf, 0xff), 111, "exactly a cube colour");
        assert_eq!(nearest_256(0x7a, 0xa2, 0xf7), 111, "Tokyo Night's blue");
        assert_eq!(nearest_256(0x16, 0x16, 0x1e), 234, "its bar: a grey (#1c1c1c)");
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
    }

    #[test]
    fn only_24_bit_colours_change_and_a_cut_sequence_waits_for_its_end() {
        let mut d = Downgrade::default();
        let s = d.filter(b"a\x1b[1;38;2;122;162;247;48;2;22;22;30mb\x1b[0m\x1b[5;1Hc\x1b]2;t\x07");
        assert_eq!(s, b"a\x1b[1;38;5;111;48;5;234mb\x1b[0m\x1b[5;1Hc\x1b]2;t\x07");
        // The colon form, with and without its colour space.
        assert_eq!(d.filter(b"\x1b[38:2::122:162:247m\x1b[48:2:0:0:0m"), b"\x1b[38:5:111m\x1b[48:5:16m");
        // Other colours and attributes as they were.
        assert_eq!(d.filter(b"\x1b[38;5;200;31;1mx"), b"\x1b[38;5;200;31;1mx");
        // Cut anywhere, the same bytes come out in the end.
        let whole = b"x\x1b[38;2;122;162;247my\x1b[H";
        for cut in 0..whole.len() {
            let mut d = Downgrade::default();
            let mut got = d.filter(&whole[..cut]);
            got.extend(d.filter(&whole[cut..]));
            assert_eq!(got, b"x\x1b[38;5;111my\x1b[H", "cut at {cut}");
        }
        // A sequence that never ends is not held for ever.
        let mut d = Downgrade::default();
        let long = [b"\x1b[".as_slice(), &[b'1'; 200]].concat();
        assert_eq!(d.filter(&long), long);
    }
}
