use super::*;

/// The characters an animation is drawn in (`clock-mode-look`): its own,
/// or a set where each cell takes the character with as much ink as it had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    #[default]
    Plain,
    Dots,
    Blocks,
    Binary,
}

impl Look {
    pub const ALL: [Look; 4] = [Look::Plain, Look::Dots, Look::Blocks, Look::Binary];

    /// Its name, as `clock-mode-look` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Look::Plain => "plain",
            Look::Dots => "dots",
            Look::Blocks => "blocks",
            Look::Binary => "binary",
        }
    }

    /// The look of that name; any other is its own characters.
    pub fn named(name: &str) -> Look {
        Look::ALL.into_iter().find(|l| l.name() == name).unwrap_or_default()
    }

    /// The set, the least ink first; none for `plain`.
    pub(super) fn set(self) -> &'static [&'static str] {
        match self {
            Look::Plain => &[],
            Look::Dots => &["·", "•", "●"],
            Look::Blocks => &["░", "▒", "▓", "█"],
            // A 1 takes less ink than a 0.
            Look::Binary => &["1", "0"],
        }
    }

    /// The character of the set for `ink` (0 to 1).
    pub(super) fn glyph(self, ink: f32) -> Option<&'static str> {
        let set = self.set();
        let n = set.len();
        (n > 0).then(|| set[((share(ink) * n as f32) as usize).min(n - 1)])
    }
}

/// How much of its cell a character covers, about: 0 to 1.
fn coverage(text: &str) -> f32 {
    let Some(c) = text.chars().next() else { return 0.0 };
    match c {
        ' ' => 0.0,
        '.' | ',' | '\'' | '`' | '·' | '˙' | '°' | '-' | '_' | '"' | ':' | ';' | '▔' => 0.35,
        '~' | '^' | '+' | '=' => 0.5,
        '░' => 0.4,
        '▒' => 0.7,
        '▓' => 0.9,
        '█' | '@' | '#' | '%' | '&' | '●' | '■' | '◆' | 'M' | 'W' | 'O' | '0' => 1.0,
        // The eighths of a block, by how many it fills.
        '▁'..='▇' => f32::from(c as u32 as u16 - 0x2580) / 8.0,
        _ => 0.65,
    }
}

/// How bright a cell's colour is drawn: bold the most; a colour faded into
/// the background by how far it stands from it; dim, little.
fn brightness(style: &Style, bg: Color) -> f32 {
    let lum = |r: u8, g: u8, b: u8| (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0;
    if style.bold {
        return 1.0;
    }
    if let (Color::Rgb(r, g, b), Color::Rgb(br, bgg, bb)) = (style.fg, bg) {
        let (f, k) = (lum(r, g, b), lum(br, bgg, bb));
        // A colour that stands as far from the background as a theme's
        // colours do (about 0.6 of the way to its far end) counts in full.
        let stand = (f - k).abs() / k.max(1.0 - k).max(0.01);
        return 0.75 * (stand / 0.6).min(1.0);
    }
    if style.dim { 0.35 } else { 0.75 }
}

/// The ink of a cell: how much of it is covered, how brightly.
pub(super) fn ink(cell: &Cell, bg: Color) -> f32 {
    coverage(cell.text()) * brightness(&cell.style, bg)
}

impl Anim {
    /// Text a look leaves as it is (figures, names), `w` columns from
    /// `x`, `y` of the grid.
    pub(super) fn keep(&self, x: u16, y: u16, w: usize) {
        self.kept.borrow_mut().push((x, y, w.min(usize::from(u16::MAX)) as u16));
    }

    /// Each cell drawn in `band` in the look's character for its ink, in
    /// its own colour; text kept, a message's envelope too.
    pub(super) fn dress(&self, g: &mut Grid, band: Rect, pal: &Palette) {
        if pal.look == Look::Plain {
            return;
        }
        let kept = self.kept.borrow();
        for y in band.y..band.y + band.h {
            for x in band.x..band.x + band.w {
                if kept.iter().any(|&(kx, ky, kw)| ky == y && (kx..kx.saturating_add(kw)).contains(&x)) {
                    continue;
                }
                let cell = g.get(x, y);
                if cell.wide || cell.cont || cell.text() == " " || cell.text() == ENVELOPE {
                    continue;
                }
                if let Some(glyph) = pal.look.glyph(ink(cell, pal.bg)) {
                    let style = cell.style;
                    g.set(x, y, Cell::new(glyph, false, style));
                }
            }
        }
    }
}
