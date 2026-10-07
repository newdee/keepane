//! Compositing panes, borders and the status line into a cell grid, and
//! diffing grids into a minimal VT byte stream for the client console.

use super::layout::Rect;
use crate::format::Segment;
use unicode_width::UnicodeWidthStr;
use vt100::Color;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Style {
    pub const fn colors(fg: Color, bg: Color) -> Style {
        Style { fg, bg, bold: false, dim: false, italic: false, underline: false, inverse: false }
    }
    fn sgr(&self) -> String {
        let mut s = String::from("\x1b[0");
        if self.bold {
            s.push_str(";1");
        }
        if self.dim {
            s.push_str(";2");
        }
        if self.italic {
            s.push_str(";3");
        }
        if self.underline {
            s.push_str(";4");
        }
        if self.inverse {
            s.push_str(";7");
        }
        color_sgr(&mut s, self.fg, true);
        color_sgr(&mut s, self.bg, false);
        s.push('m');
        s
    }
}

fn color_sgr(s: &mut String, c: Color, fg: bool) {
    use std::fmt::Write;
    match c {
        Color::Default => {}
        Color::Idx(n @ 0..=7) => write!(s, ";{}", if fg { 30 + n as u16 } else { 40 + n as u16 }).unwrap(),
        Color::Idx(n @ 8..=15) => write!(s, ";{}", if fg { 82 + n as u16 } else { 92 + n as u16 }).unwrap(),
        Color::Idx(n) => write!(s, ";{};5;{n}", if fg { 38 } else { 48 }).unwrap(),
        Color::Rgb(r, g, b) => write!(s, ";{};2;{r};{g};{b}", if fg { 38 } else { 48 }).unwrap(),
    }
}

const TEXT_BYTES: usize = 24;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    text: [u8; TEXT_BYTES],
    len: u8,
    pub wide: bool,
    /// Second half of a wide character.
    pub cont: bool,
    pub style: Style,
}

impl Cell {
    pub fn blank(style: Style) -> Cell {
        Cell { text: [0; TEXT_BYTES], len: 0, wide: false, cont: false, style }
    }
    pub fn new(s: &str, wide: bool, style: Style) -> Cell {
        let mut c = Cell::blank(style);
        let n = s.len().min(TEXT_BYTES);
        // Never cut a UTF-8 sequence.
        let n = (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
        c.text[..n].copy_from_slice(&s.as_bytes()[..n]);
        c.len = n as u8;
        c.wide = wide;
        c
    }
    fn continuation(style: Style) -> Cell {
        Cell { cont: true, ..Cell::blank(style) }
    }
    /// Cell text; a blank cell reads as a single space.
    pub fn text(&self) -> &str {
        if self.len == 0 {
            return " ";
        }
        std::str::from_utf8(&self.text[..self.len as usize]).unwrap_or(" ")
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Grid {
        Grid { cols, rows, cells: vec![Cell::blank(Style::default()); cols as usize * rows as usize] }
    }

    pub fn get(&self, x: u16, y: u16) -> &Cell {
        &self.cells[y as usize * self.cols as usize + x as usize]
    }

    pub fn set(&mut self, x: u16, y: u16, cell: Cell) {
        if x < self.cols && y < self.rows {
            let i = y as usize * self.cols as usize + x as usize;
            self.cells[i] = cell;
        }
    }

    /// Write `s` at (x, y) clipped to `max_w` cells; returns cells used.
    pub fn put_str(&mut self, x: u16, y: u16, s: &str, style: Style, max_w: u16) -> u16 {
        let mut cx = x;
        let end = x.saturating_add(max_w).min(self.cols);
        for ch in s.chars() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            if w == 0 {
                continue;
            }
            if cx + w > end {
                break;
            }
            let mut buf = [0u8; 4];
            self.set(cx, y, Cell::new(ch.encode_utf8(&mut buf), w == 2, style));
            if w == 2 {
                self.set(cx + 1, y, Cell::continuation(style));
            }
            cx += w;
        }
        cx - x
    }

    pub fn fill(&mut self, r: Rect, style: Style) {
        for y in r.y..r.y.saturating_add(r.h).min(self.rows) {
            for x in r.x..r.x.saturating_add(r.w).min(self.cols) {
                self.set(x, y, Cell::blank(style));
            }
        }
    }

    /// Copy the visible part of a terminal screen into `rect`.
    pub fn blit_screen(&mut self, rect: Rect, screen: &vt100::Screen) {
        let (srows, scols) = screen.size();
        for y in 0..rect.h.min(srows) {
            let mut x = 0u16;
            while x < rect.w.min(scols) {
                let gx = rect.x + x;
                let gy = rect.y + y;
                if gx >= self.cols || gy >= self.rows {
                    break;
                }
                let Some(c) = screen.cell(y, x) else {
                    x += 1;
                    continue;
                };
                let style = Style {
                    fg: c.fgcolor(),
                    bg: c.bgcolor(),
                    bold: c.bold(),
                    dim: c.dim(),
                    italic: c.italic(),
                    underline: c.underline(),
                    inverse: c.inverse(),
                };
                if c.is_wide_continuation() {
                    self.set(gx, gy, Cell::continuation(style));
                    x += 1;
                    continue;
                }
                if c.is_wide() {
                    if x + 1 >= rect.w || gx + 1 >= self.cols {
                        // Wide character does not fit: draw a blank.
                        self.set(gx, gy, Cell::blank(style));
                        x += 1;
                        continue;
                    }
                    self.set(gx, gy, Cell::new(c.contents(), true, style));
                    self.set(gx + 1, gy, Cell::continuation(style));
                    x += 2;
                    continue;
                }
                if c.has_contents() {
                    self.set(gx, gy, Cell::new(c.contents(), false, style));
                } else {
                    self.set(gx, gy, Cell::blank(style));
                }
                x += 1;
            }
        }
    }

    /// Toggle inverse video on a cell (used for copy-mode selection/cursor).
    pub fn invert(&mut self, x: u16, y: u16) {
        if x < self.cols && y < self.rows {
            let i = y as usize * self.cols as usize + x as usize;
            self.cells[i].style.inverse = !self.cells[i].style.inverse;
        }
    }
}

/// Produce VT output turning `prev` (None = unknown/cleared) into `cur`, then
/// placing the cursor at `cursor` (None = hidden).
pub fn diff(prev: Option<&Grid>, cur: &Grid, cursor: Option<(u16, u16)>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let full = prev.is_none_or(|p| p.cols != cur.cols || p.rows != cur.rows);
    out.extend_from_slice(b"\x1b[?25l");
    if full {
        out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
    }
    let mut style: Option<Style> = None;
    for y in 0..cur.rows {
        let changed = |x: u16| full || prev.unwrap().get(x, y) != cur.get(x, y);
        let mut x = 0u16;
        while x < cur.cols {
            if !changed(x) {
                x += 1;
                continue;
            }
            let mut start = x;
            let mut end = x + 1;
            let mut gap = 0u16;
            let mut xx = x + 1;
            while xx < cur.cols {
                if changed(xx) {
                    end = xx + 1;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > 4 {
                        break;
                    }
                }
                xx += 1;
            }
            if cur.get(start, y).cont && start > 0 {
                start -= 1;
            }
            out.extend_from_slice(format!("\x1b[{};{}H", y + 1, start + 1).as_bytes());
            let mut cx = start;
            while cx < end {
                let c = cur.get(cx, y);
                if c.cont {
                    // Orphan continuation (its head was outside the run): pad.
                    if style != Some(c.style) {
                        out.extend_from_slice(c.style.sgr().as_bytes());
                        style = Some(c.style);
                    }
                    out.push(b' ');
                    cx += 1;
                    continue;
                }
                if style != Some(c.style) {
                    out.extend_from_slice(c.style.sgr().as_bytes());
                    style = Some(c.style);
                }
                out.extend_from_slice(c.text().as_bytes());
                cx += if c.wide { 2 } else { 1 };
            }
            x = end;
        }
    }
    out.extend_from_slice(b"\x1b[0m");
    if let Some((x, y)) = cursor {
        out.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1).as_bytes());
    }
    out
}

/// One pane to draw.
pub struct PaneView<'a> {
    pub rect: Rect,
    pub screen: &'a vt100::Screen,
    pub active: bool,
    /// Copy-mode cursor and inclusive selection range in screen coordinates.
    pub copy: Option<CopyView>,
    /// `pane-border-status`: text for the border row above (true) or below
    /// (false) the pane, which the layout has left free for it.
    pub border_text: Option<(Vec<Segment>, bool)>,
}

#[derive(Clone, Copy)]
pub struct CopyView {
    pub cx: u16,
    pub cy: u16,
    pub sel: Option<((u16, u16), (u16, u16))>,
    /// The selection is a rectangle (`C-v`), so every line takes the same
    /// columns instead of running to the end.
    pub rect: bool,
    pub offset: usize,
}

/// Where the window list sits between status-left and status-right
/// (tmux `status-justify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Justify {
    #[default]
    Left,
    /// Centred in the room between the two sides.
    Centre,
    Right,
    /// Centred on the line itself, room permitting.
    AbsoluteCentre,
}

impl Justify {
    pub fn parse(s: &str) -> Justify {
        match s {
            "centre" | "center" => Justify::Centre,
            "right" => Justify::Right,
            "absolute-centre" | "absolute-center" => Justify::AbsoluteCentre,
            _ => Justify::Left,
        }
    }
}

pub struct StatusLine {
    /// Expanded `status-left`.
    pub left: Vec<Segment>,
    /// (expanded window label, is_current)
    pub windows: Vec<(Vec<Segment>, bool)>,
    /// Between window labels (`window-status-separator`).
    pub separator: String,
    pub justify: Justify,
    /// Expanded `status-right`.
    pub right: Vec<Segment>,
    pub message: Option<String>,
    /// (prompt, input, cursor index in chars)
    pub prompt: Option<(String, String, usize)>,
    pub fg: Color,
    pub bg: Color,
}

pub struct Frame<'a> {
    pub cols: u16,
    pub rows: u16,
    pub panes: Vec<PaneView<'a>>,
    pub status: Option<StatusLine>,
    pub status_top: bool,
    pub border_fg: Color,
    pub active_border_fg: Color,
}

/// Result of `compose`: the grid, the cursor position (None = hidden) and
/// the status-line column range `[start, end)` of every window label drawn
/// (for mouse clicks).
pub type Composed = (Grid, Option<(u16, u16)>, Vec<(u16, u16)>);

/// Compose a frame.
pub fn compose(f: &Frame) -> Composed {
    let mut g = Grid::new(f.cols, f.rows);
    let mut cursor = None;
    let mut window_hits: Vec<(u16, u16)> = Vec::new();
    let (win_y, win_h, status_y) = if f.status.is_some() && f.rows > 1 {
        if f.status_top { (1, f.rows - 1, Some(0)) } else { (0, f.rows - 1, Some(f.rows - 1)) }
    } else {
        (0, f.rows, None)
    };
    let win = Rect { x: 0, y: win_y, w: f.cols, h: win_h };

    // Borders: any cell of the window area not covered by a pane.
    let covered = |x: u16, y: u16| f.panes.iter().any(|p| p.rect.contains(x, y));
    let is_border = |x: u16, y: u16| win.contains(x, y) && !covered(x, y);
    let active = f.panes.iter().find(|p| p.active).map(|p| p.rect);
    for y in win.y..win.y + win.h {
        for x in win.x..win.x + win.w {
            if !is_border(x, y) {
                continue;
            }
            let l = x > 0 && is_border(x - 1, y);
            let r = x + 1 < f.cols && is_border(x + 1, y);
            let u = y > 0 && is_border(x, y - 1);
            let d = y + 1 < f.rows && is_border(x, y + 1);
            let ch = match (l, r, u, d) {
                (true, true, true, true) => "┼",
                (true, true, true, false) => "┴",
                (true, true, false, true) => "┬",
                (true, false, true, true) => "┤",
                (false, true, true, true) => "├",
                (true, false, false, true) => "┐",
                (false, true, false, true) => "┌",
                (true, false, true, false) => "┘",
                (false, true, true, false) => "└",
                (_, _, true, _) | (_, _, _, true) => "│",
                _ => "─",
            };
            let near_active = active.is_some_and(|a| x + 1 >= a.x && x <= a.x + a.w && y + 1 >= a.y && y <= a.y + a.h);
            let fg = if near_active { f.active_border_fg } else { f.border_fg };
            g.set(x, y, Cell::new(ch, false, Style::colors(fg, Color::Default)));
        }
    }

    for p in &f.panes {
        // The pane's border text, on the border row the layout reserved.
        if let Some((segs, top)) = &p.border_text {
            let y = if *top { p.rect.y.checked_sub(1) } else { Some(p.rect.y + p.rect.h) };
            if let Some(y) = y
                && win.contains(p.rect.x, y)
            {
                g.put_segments(p.rect.x, y, segs, p.rect.w);
            }
        }
        g.blit_screen(p.rect, p.screen);
        if let Some(c) = p.copy {
            if let Some(((sy, sx), (ey, ex))) = c.sel {
                for y in sy..=ey.min(p.rect.h.saturating_sub(1)) {
                    let (x0, x1) = if c.rect {
                        (sx.min(ex), sx.max(ex))
                    } else {
                        (if y == sy { sx } else { 0 }, if y == ey { ex } else { p.rect.w.saturating_sub(1) })
                    };
                    for x in x0..=x1.min(p.rect.w.saturating_sub(1)) {
                        g.invert(p.rect.x + x, p.rect.y + y);
                    }
                }
            }
            // Position indicator, tmux style, top right of the pane: how far
            // up the view is, of how many lines of history there are.
            let ind = format!("[{}/{}]", c.offset, p.screen.scrollback_rows());
            let ind_w = ind.width() as u16;
            if p.rect.w > ind_w + 1 {
                g.put_str(
                    p.rect.x + p.rect.w - ind_w - 1,
                    p.rect.y,
                    &ind,
                    Style::colors(Color::Idx(0), Color::Idx(3)),
                    ind_w,
                );
            }
            if p.active {
                g.invert(p.rect.x + c.cx, p.rect.y + c.cy);
                cursor = None;
            }
        } else if p.active && !p.screen.hide_cursor() {
            let (r, col) = p.screen.cursor_position();
            let (cx, cy) = (p.rect.x + col, p.rect.y + r);
            // Inside the pane and inside this client's grid (a smaller client
            // attached to the same session sees a clipped pane).
            if r < p.rect.h && col < p.rect.w && cx < f.cols && cy < f.rows {
                cursor = Some((cx, cy));
            }
        }
    }

    if let (Some(s), Some(sy)) = (&f.status, status_y) {
        let style = Style::colors(s.fg, s.bg);
        g.fill(Rect { x: 0, y: sy, w: f.cols, h: 1 }, style);
        if s.prompt.is_some() || s.message.is_some() {
            cursor = draw_notice(&mut g, sy, s.prompt.as_ref(), s.message.as_deref()).or(cursor);
        } else {
            let mut x = g.put_segments(0, sy, &s.left, f.cols);
            // The window list comes before the right side: the right side
            // gets what the whole list leaves over, and is clipped to it.
            // (A long pane title, or the default path and load, on a
            // narrow terminal used to push windows off the line.)
            let sep_w = s.separator.width() as u16;
            let total: u16 = s.windows.iter().map(|(l, _)| seg_width(l)).sum::<u16>()
                + sep_w * (s.windows.len().saturating_sub(1)) as u16;
            let full = seg_width(&s.right);
            // The whole list if it fits; else room for the current window
            // (the ones before it are left out below); else nothing, since
            // reserving room for a label that cannot fit anyway is pointless.
            let cur_w = s.windows.iter().find(|(_, c)| *c).map(|(l, _)| seg_width(l)).unwrap_or(0);
            let right_w = [total, cur_w]
                .iter()
                .find_map(|need| f.cols.checked_sub(x.saturating_add(*need).saturating_add(1)))
                .map_or(full, |room| full.min(room));
            let win_end = f.cols.saturating_sub(right_w + 1);
            let room = win_end.saturating_sub(x);
            // status-justify moves the whole list when it fits. When it
            // does not, windows are left out from the front until the
            // current one fits, so the current window is always shown.
            let mut first = 0;
            if total <= room {
                x = match s.justify {
                    Justify::Left => x,
                    Justify::Centre => x + (room - total) / 2,
                    Justify::Right => win_end - total,
                    Justify::AbsoluteCentre => (f.cols.saturating_sub(total) / 2).clamp(x, win_end - total),
                };
            } else if let Some(cur) = s.windows.iter().position(|(_, c)| *c) {
                let upto = |from: usize| -> u16 {
                    s.windows[from..=cur].iter().map(|(l, _)| seg_width(l)).sum::<u16>() + sep_w * (cur - from) as u16
                };
                while first < cur && upto(first) > room {
                    first += 1;
                }
            }
            let base = Style::colors(s.fg, s.bg);
            for (i, (label, current)) in s.windows.iter().enumerate() {
                if i < first {
                    window_hits.push((0, 0)); // left out: never hit
                    continue;
                }
                // The separator and the label it leads to go together: a
                // label that does not fit leaves no dangling separator.
                let lead = if i > first { sep_w } else { 0 };
                let w = seg_width(label);
                if x + lead + w > win_end {
                    break;
                }
                if i > first {
                    x += g.put_str(x, sy, &s.separator, base, win_end - x);
                }
                let start = x;
                if *current {
                    let inv: Vec<Segment> = label
                        .iter()
                        .map(|s| Segment { text: s.text.clone(), style: Style { inverse: true, ..s.style } })
                        .collect();
                    x += g.put_segments(x, sy, &inv, win_end - x);
                } else {
                    x += g.put_segments(x, sy, label, win_end - x);
                }
                window_hits.push((start, x));
            }
            if right_w < f.cols {
                g.put_segments(f.cols - right_w, sy, &s.right, right_w);
            }
        }
    }
    (g, cursor, window_hits)
}

/// Display width of a segment list.
pub fn seg_width(segs: &[Segment]) -> u16 {
    segs.iter().map(|s| s.text.width() as u16).sum()
}

impl Grid {
    /// Write styled segments at (x, y) clipped to `max_w`; returns cells used.
    pub fn put_segments(&mut self, x: u16, y: u16, segs: &[Segment], max_w: u16) -> u16 {
        let mut used = 0u16;
        for s in segs {
            if used >= max_w {
                break;
            }
            used += self.put_str(x + used, y, &s.text, s.style, max_w - used);
        }
        used
    }
}

/// Draw a block of text over `area` (tmux view mode). The last row of the
/// area shows a hint; lines that do not fit are dropped.
pub fn draw_overlay(g: &mut Grid, area: Rect, lines: &[String]) {
    if area.h == 0 || area.w == 0 {
        return;
    }
    let style = Style::colors(Color::Default, Color::Default);
    g.fill(area, style);
    let body_h = area.h.saturating_sub(1) as usize;
    for (i, line) in lines.iter().take(body_h).enumerate() {
        g.put_str(area.x, area.y + i as u16, line, style, area.w);
    }
    let hint = if lines.len() > body_h {
        format!("[{} of {}] press any key", body_h, crate::format::count(lines.len(), "line"))
    } else {
        "press any key".to_string()
    };
    let hint_style = Style::colors(Color::Idx(0), Color::Idx(3));
    g.fill(Rect { x: area.x, y: area.y + area.h - 1, w: area.w, h: 1 }, hint_style);
    g.put_str(area.x, area.y + area.h - 1, &hint, hint_style, area.w);
}

/// The prefix's panel (`prefix-hint`): what the next key does, a column a
/// group, at the bottom of `area` and centred. Groups that do not fit side
/// by side go below; rows that do not fit are left out. `title` goes in the
/// top border (the prefix), `footer` in the bottom one.
pub fn draw_key_hint(
    g: &mut Grid,
    area: Rect,
    title: &str,
    groups: &[(&str, Vec<super::keyhint::Entry>)],
    footer: &str,
) {
    let w = |s: &str| UnicodeWidthStr::width(s) as u16;
    // Each group: its keys' column width, and its own width.
    let cols: Vec<(u16, u16)> = groups
        .iter()
        .map(|(name, entries)| {
            let kw = entries.iter().map(|e| w(&e.keys)).max().unwrap_or(0);
            let lw = entries.iter().map(|e| w(&e.label)).max().unwrap_or(0);
            (kw, w(name).max(kw + 2 + lw))
        })
        .collect();
    const GAP: u16 = 3;
    let room = area.w.saturating_sub(4);
    // A group cut short says less than none: too narrow, no panel.
    if cols.iter().any(|&(_, cw)| cw > room) {
        return;
    }
    // A grid: as many columns as fit across, the groups in order, a column
    // as wide as its widest group, so the rows of groups line up.
    let width_of = |n: usize| -> (Vec<u16>, u16) {
        let mut ws = vec![0u16; n];
        for (i, &(_, cw)) in cols.iter().enumerate() {
            ws[i % n] = ws[i % n].max(cw);
        }
        let total = ws.iter().sum::<u16>() + GAP * (n as u16).saturating_sub(1);
        (ws, total)
    };
    let n = (1..=groups.len().max(1)).rev().find(|&n| width_of(n).1 <= room).unwrap_or(1);
    let (col_w, grid_w) = width_of(n);
    let bands: Vec<Vec<usize>> = (0..groups.len()).collect::<Vec<_>>().chunks(n).map(|c| c.to_vec()).collect();
    let band_h = |b: &Vec<usize>| b.iter().map(|&i| groups[i].1.len() as u16 + 1).max().unwrap_or(0);
    let inner_w = grid_w.max(w(title) + 2).max(w(footer) + 2).min(room);
    let inner_h = bands.iter().map(band_h).sum::<u16>() + (bands.len() as u16).saturating_sub(1);
    let (bw, bh) = (inner_w + 4, (inner_h + 2).min(area.h));
    if bw > area.w || bh < 3 {
        return;
    }
    let rect = Rect { x: area.x + (area.w - bw) / 2, y: area.y + area.h - bh, w: bw, h: bh };
    let border = Style::colors(Color::Idx(3), Color::Default);
    let plain = Style::default();
    g.fill(rect, plain);
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w - 1, rect.y + rect.h - 1);
    let line = |s: &str| Cell::new(s, false, border);
    for x in x0 + 1..x1 {
        g.set(x, y0, line("─"));
        g.set(x, y1, line("─"));
    }
    for y in y0 + 1..y1 {
        g.set(x0, y, line("│"));
        g.set(x1, y, line("│"));
    }
    g.set(x0, y0, line("┌"));
    g.set(x1, y0, line("┐"));
    g.set(x0, y1, line("└"));
    g.set(x1, y1, line("┘"));
    g.put_str(x0 + 2, y0, &format!(" {title} "), Style { bold: true, ..border }, bw.saturating_sub(4));
    if !footer.is_empty() {
        g.put_str(x0 + 2, y1, &format!(" {footer} "), border, bw.saturating_sub(4));
    }
    let head = Style { bold: true, ..plain };
    let key = Style { bold: true, ..Style::colors(Color::Idx(3), Color::Default) };
    let bottom = y1; // rows from here on are the border's
    let mut y = y0 + 1;
    for b in &bands {
        let mut x = x0 + 2;
        for (pos, &i) in b.iter().enumerate() {
            let (kw, _) = cols[i];
            let cw = col_w[pos];
            let (name, entries) = &groups[i];
            let limit = |row: u16| row < bottom;
            if limit(y) {
                g.put_str(x, y, name, head, cw);
            }
            for (n, e) in entries.iter().enumerate() {
                let row = y + 1 + n as u16;
                if !limit(row) {
                    break;
                }
                g.put_str(x, row, &e.keys, key, kw);
                g.put_str(x + kw + 2, row, &e.label, plain, cw.saturating_sub(kw + 2));
            }
            x += cw + GAP;
        }
        y += band_h(b) + 1;
    }
}

/// `display-popup`: a box with a program inside it, drawn over everything
/// else. Returns where the cursor sits, or None when the popup has no room
/// for one. `hint` replaces the bottom border text once the command is done.
pub fn draw_popup(
    g: &mut Grid,
    rect: Rect,
    screen: &vt100::Screen,
    border: Color,
    hint: Option<&str>,
) -> Option<(u16, u16)> {
    if rect.w < 3 || rect.h < 3 {
        return None;
    }
    let style = Style::colors(border, Color::Default);
    let plain = Style::default();
    g.fill(rect, plain);
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w - 1, rect.y + rect.h - 1);
    let line = |s: &str| Cell::new(s, false, style);
    for x in x0 + 1..x1 {
        g.set(x, y0, line("─"));
        g.set(x, y1, line("─"));
    }
    for y in y0 + 1..y1 {
        g.set(x0, y, line("│"));
        g.set(x1, y, line("│"));
    }
    g.set(x0, y0, line("┌"));
    g.set(x1, y0, line("┐"));
    g.set(x0, y1, line("└"));
    g.set(x1, y1, line("┘"));
    if let Some(h) = hint {
        g.put_str(x0 + 1, y1, &format!(" {h} "), style, rect.w.saturating_sub(2));
    }
    let inner = Rect { x: x0 + 1, y: y0 + 1, w: rect.w - 2, h: rect.h - 2 };
    g.blit_screen(inner, screen);
    if screen.hide_cursor() {
        return None;
    }
    let (cy, cx) = screen.cursor_position();
    if cx < inner.w && cy < inner.h { Some((inner.x + cx, inner.y + cy)) } else { None }
}

/// `display-panes`: a pane's number, centred in its rectangle, big enough to
/// read at a glance (the active pane in the active colour).
/// Write `parts` at the right end of row `row` of `rect`, but only over
/// cells that are blank and uncoloured, with two more blank ones before it:
/// what a line says is never covered. Returns whether it fit.
pub fn draw_stamp(g: &mut Grid, rect: Rect, row: u16, parts: &[(String, Style)]) -> bool {
    let width: u16 = parts.iter().map(|(s, _)| UnicodeWidthStr::width(s.as_str()) as u16).sum();
    if row >= rect.h || rect.w < width + 2 {
        return false;
    }
    let y = rect.y + row;
    let end = (rect.x + rect.w).min(g.cols);
    let Some(x0) = end.checked_sub(width) else { return false };
    if y >= g.rows || x0 < rect.x + 2 {
        return false;
    }
    let free = (x0 - 2..end).all(|x| {
        let c = g.get(x, y);
        c.text() == " " && !c.cont && c.style.bg == Color::Default && !c.style.inverse
    });
    if !free {
        return false;
    }
    let mut x = x0;
    for (s, style) in parts {
        x += g.put_str(x, y, s, *style, end - x);
    }
    true
}

/// The rectangle a focus frame is at, `t` of the way (0..=1) from `from`
/// to `to`, eased out: quick to leave, gentle to arrive.
pub fn frame_at(from: Rect, to: Rect, t: f32) -> Rect {
    let t = t.clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    let lerp = |a: u16, b: u16| (f32::from(a) + (f32::from(b) - f32::from(a)) * e).round() as u16;
    Rect { x: lerp(from.x, to.x), y: lerp(from.y, to.y), w: lerp(from.w, to.w), h: lerp(from.h, to.h) }
}

/// A focus frame: a rounded outline on the edge of `r`, over whatever is
/// there. A wide character it cuts in half loses the other half too, so no
/// half character is left to shift the row.
pub fn draw_frame(g: &mut Grid, r: Rect, style: Style) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    let (x1, y1) = (r.x + r.w - 1, r.y + r.h - 1);
    let mut put = |x: u16, y: u16, s: &str| {
        if x >= g.cols || y >= g.rows {
            return;
        }
        let c = g.get(x, y);
        if c.wide {
            g.set(x + 1, y, Cell::blank(c.style));
        } else if c.cont && x > 0 {
            let head = g.get(x - 1, y).style;
            g.set(x - 1, y, Cell::blank(head));
        }
        g.set(x, y, Cell::new(s, false, style));
    };
    for x in r.x + 1..x1 {
        put(x, r.y, "─");
        put(x, y1, "─");
    }
    for y in r.y + 1..y1 {
        put(r.x, y, "│");
        put(x1, y, "│");
    }
    put(r.x, r.y, "╭");
    put(x1, r.y, "╮");
    put(r.x, y1, "╰");
    put(x1, y1, "╯");
}

/// Under the number: the pane's name (`rename-pane`) as `%name`, the way
/// `-t %name` finds it, and its work mode (`%build · shell`); a pane
/// without a name shows its mode alone.
pub fn draw_pane_number(g: &mut Grid, rect: Rect, number: usize, colour: Color, name: Option<&str>, mode: &str) {
    // The digits are drawn with `█`, so the colour is the foreground.
    let style = Style::colors(colour, Color::Default);
    let text = number.to_string();
    let label = Some(match name {
        Some(n) => format!("%{n} · {mode}"),
        None => mode.to_string(),
    })
    .filter(|l| !l.is_empty());
    if draw_big_text(g, rect, &text, style) {
        let Some(label) = label else { return };
        // A blank row under the digits, else right under them, else above.
        let top = rect.y + (rect.h - 5) / 2;
        let bottom = rect.y + rect.h;
        let y = [top + 6, top + 5].into_iter().find(|y| *y < bottom).or(top.checked_sub(1).filter(|y| *y >= rect.y));
        if let Some(y) = y {
            // Names are letters, digits, `-` and `_`, the mode a word: a
            // column each (the `·` too).
            let w = (label.chars().count() as u16).min(rect.w);
            g.put_str(rect.x + (rect.w - w) / 2, y, &label, style, w);
        }
    } else if rect.w > 0 && rect.h > 0 {
        // Too small for the block digits: plain text in the corner.
        let text = label.map_or(text.clone(), |l| format!("{text} {l}"));
        g.put_str(rect.x, rect.y, &text, style, rect.w);
    }
}

/// Draw digits and `:` as 3x5 blocks centred in `rect`; false when there is
/// no room. Used by `display-panes` and `clock-mode`.
pub fn draw_big_text(g: &mut Grid, rect: Rect, text: &str, style: Style) -> bool {
    const DIGITS: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111], // 0
        [0b010, 0b110, 0b010, 0b010, 0b111], // 1
        [0b111, 0b001, 0b111, 0b100, 0b111], // 2
        [0b111, 0b001, 0b111, 0b001, 0b111], // 3
        [0b101, 0b101, 0b111, 0b001, 0b001], // 4
        [0b111, 0b100, 0b111, 0b001, 0b111], // 5
        [0b111, 0b100, 0b111, 0b101, 0b111], // 6
        [0b111, 0b001, 0b001, 0b001, 0b001], // 7
        [0b111, 0b101, 0b111, 0b101, 0b111], // 8
        [0b111, 0b101, 0b111, 0b001, 0b111], // 9
    ];
    // A colon is two dots, narrower than a digit.
    const COLON: [u8; 5] = [0b000, 0b010, 0b000, 0b010, 0b000];
    let digit_w = 4u16; // 3 columns plus a gap
    let big_w = text.chars().count() as u16 * digit_w;
    if rect.w < big_w || rect.h < 5 {
        return false;
    }
    let x0 = rect.x + (rect.w - big_w) / 2;
    let y0 = rect.y + (rect.h - 5) / 2;
    for (i, ch) in text.chars().enumerate() {
        let glyph = match ch {
            ':' => COLON,
            c => DIGITS[c.to_digit(10).unwrap_or(0) as usize],
        };
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3u16 {
                if bits & (1 << (2 - col)) != 0 {
                    // A solid block rather than a coloured space, so it shows
                    // whatever the pane's background is.
                    g.set(x0 + i as u16 * digit_w + col, y0 + row as u16, Cell::new("█", false, style));
                }
            }
        }
    }
    true
}

/// The `choose-tree` picker: `lines` from `top` fill the area, line `sel` is
/// highlighted (tmux mode-style: black on yellow), the last row is the key hint.
/// `actions` is the hint's middle piece: what Enter (and any other action
/// key) does for this kind of list. `status` (the filter, the tag count)
/// sits at the right end of the hint row and wins over the hint when the
/// row is too narrow for both: it is state, the hint is not.
/// Draw in the theme's colours (`pane-colours`, `window-style`): the
/// palette's first 16 as the theme's own everywhere, and the default colours
/// as `fg` / `bg` everywhere but row `bar` (the status line, which has a
/// style of its own). With neither set, the grid is left as it is: the
/// terminal's own colours.
pub fn recolor(g: &mut Grid, bar: Option<u16>, fg: Color, bg: Color, palette: &[Color]) {
    let palette = (palette.len() == 16).then_some(palette);
    if fg == Color::Default && bg == Color::Default && palette.is_none() {
        return;
    }
    let map = |c: Color| match (c, palette) {
        (Color::Idx(n), Some(p)) if n < 16 => p[usize::from(n)],
        _ => c,
    };
    let cols = usize::from(g.cols);
    for (i, cell) in g.cells.iter_mut().enumerate() {
        let s = &mut cell.style;
        s.fg = map(s.fg);
        s.bg = map(s.bg);
        if bar != Some((i / cols.max(1)) as u16) {
            if s.fg == Color::Default {
                s.fg = fg;
            }
            if s.bg == Color::Default {
                s.bg = bg;
            }
        }
    }
}

/// One `hints` pick: the thing found at `row`, `col` of the pane at `rect`
/// (`width` cells), shown yellow and underlined, with what is left of its
/// label over its first cells in black on yellow.
pub fn draw_hint(g: &mut Grid, rect: Rect, row: u16, col: u16, width: u16, label: &str) {
    if row >= rect.h || col >= rect.w {
        return;
    }
    let y = rect.y + row;
    for x in rect.x + col..rect.x + col.saturating_add(width).min(rect.w) {
        if x < g.cols && y < g.rows {
            let mut cell = g.get(x, y).clone();
            cell.style.fg = Color::Idx(3);
            cell.style.underline = true;
            cell.style.inverse = false;
            g.set(x, y, cell);
        }
    }
    let style = Style { bold: true, ..Style::colors(Color::Idx(0), Color::Idx(3)) };
    g.put_str(rect.x + col, y, label, style, rect.w - col);
}

/// A prompt (with its input) or else a message across row `y`, the way the
/// status line shows them; with `status off` they are drawn over the bottom
/// row instead, so a prompt is never typed blind. Returns where the cursor
/// goes for a prompt.
pub fn draw_notice(
    g: &mut Grid,
    y: u16,
    prompt: Option<&(String, String, usize)>,
    message: Option<&str>,
) -> Option<(u16, u16)> {
    let style = Style::colors(Color::Idx(0), Color::Idx(3));
    let cols = g.cols;
    if let Some((label, input, ci)) = prompt {
        g.fill(Rect { x: 0, y, w: cols, h: 1 }, style);
        let used = g.put_str(0, y, label, style, cols);
        let before: String = input.chars().take(*ci).collect();
        let bw = before.width() as u16;
        g.put_str(used, y, input, style, cols.saturating_sub(used));
        return Some(((used + bw).min(cols.saturating_sub(1)), y));
    }
    if let Some(m) = message {
        g.fill(Rect { x: 0, y, w: cols, h: 1 }, style);
        g.put_str(0, y, m, style, cols);
    }
    None
}

pub fn draw_chooser(g: &mut Grid, area: Rect, lines: &[String], sel: usize, top: usize, actions: &str, status: &str) {
    if area.h == 0 || area.w == 0 {
        return;
    }
    let style = Style::colors(Color::Default, Color::Default);
    let hi = Style::colors(Color::Idx(0), Color::Idx(3));
    g.fill(area, style);
    let body_h = area.h.saturating_sub(1) as usize;
    for (row, (i, line)) in lines.iter().enumerate().skip(top).take(body_h).enumerate() {
        let y = area.y + row as u16;
        let st = if i == sel { hi } else { style };
        if i == sel {
            g.fill(Rect { x: area.x, y, w: area.w, h: 1 }, hi);
        }
        g.put_str(area.x, y, line, st, area.w);
    }
    draw_chooser_footer(g, area, lines.len(), sel, "j/k move  g/G top/bottom", actions, status);
}

/// The tree lines in front of each node of a tree, from the nodes' depths
/// in order (0 the root): `├─ ` before a node with a sibling after it, `└─ `
/// before the last, and for each level above, `│  ` where that ancestor has
/// a sibling still to come, else blanks.
pub fn tree_leads(depths: &[usize]) -> Vec<String> {
    let has_next = |i: usize| depths[i + 1..].iter().take_while(|&&d| d >= depths[i]).any(|&d| d == depths[i]);
    let mut out = Vec::with_capacity(depths.len());
    // For each level from 1: whether the ancestor there has a sibling to come.
    let mut open: Vec<bool> = Vec::new();
    for (i, &d) in depths.iter().enumerate() {
        if d == 0 {
            open.clear();
            out.push(String::new());
            continue;
        }
        open.truncate(d - 1);
        let mut lead: String =
            (0..d - 1).map(|k| if open.get(k).copied().unwrap_or(false) { "│  " } else { "   " }).collect();
        let next = has_next(i);
        lead.push_str(if next { "├─ " } else { "└─ " });
        out.push(lead);
        open.resize(d - 1, false);
        open.push(next);
    }
    out
}

/// The session tree as blocks: each line its tree lines, then a block in
/// its level's colour (the root cyan, a session blue, a window purple, a
/// pane grey; the selected one yellow) with its number and what it is.
/// `rows` are (tree lines, block text, depth).
pub fn draw_tree(
    g: &mut Grid,
    area: Rect,
    rows: &[(String, String, usize)],
    sel: usize,
    top: usize,
    actions: &str,
    status: &str,
) {
    if area.h == 0 || area.w == 0 {
        return;
    }
    g.fill(area, Style::colors(Color::Default, Color::Default));
    let line = Style::colors(Color::Idx(8), Color::Default);
    let block = |depth: usize, selected: bool| {
        let (fg, bg) = match (selected, depth) {
            (true, _) => (Color::Idx(0), Color::Idx(3)),
            (_, 0) => (Color::Idx(0), Color::Idx(6)),
            (_, 1) => (Color::Idx(0), Color::Idx(4)),
            (_, 2) => (Color::Idx(0), Color::Idx(5)),
            _ => (Color::Idx(15), Color::Idx(8)),
        };
        Style { bold: selected || depth < 2, ..Style::colors(fg, bg) }
    };
    let body_h = area.h.saturating_sub(1) as usize;
    for (row, (i, (lead, text, depth))) in rows.iter().enumerate().skip(top).take(body_h).enumerate() {
        let y = area.y + row as u16;
        let x = area.x + g.put_str(area.x, y, lead, line, area.w).saturating_sub(area.x);
        let room = (area.x + area.w).saturating_sub(x);
        g.put_str(x, y, &format!(" {text} "), block(*depth, i == sel), room);
    }
    draw_chooser_footer(g, area, rows.len(), sel, "↑↓←→ move", actions, status);
}

/// One block of the chart view: what it is (`host`, `session`, `window`,
/// `pane`; empty for the root), how deep it is (0 the root), its two lines,
/// and whether it is its parent's current one (a session's current window,
/// a window's active pane).
pub struct ChartNode {
    pub kind: &'static str,
    pub depth: usize,
    pub title: String,
    pub info: String,
    pub active: bool,
}

/// Columns between two blocks of a chart row.
const CHART_GAP: usize = 2;
/// The widest a chart block gets; longer text is cut with `…`.
const CHART_BLOCK: usize = 30;

/// The parent of each node of a tree given in order by depths: the
/// nearest node before it one level up.
fn chart_parents(depths: &[usize]) -> Vec<Option<usize>> {
    let mut last: Vec<Option<usize>> = Vec::new();
    depths
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            last.resize(d, None);
            let parent = d.checked_sub(1).and_then(|k| last.get(k).copied().flatten());
            last.push(Some(i));
            parent
        })
        .collect()
}

/// The branch the chart shows for the node `sel`: its ancestors from the
/// root, itself, then below it each level's current child (else its first).
pub fn chart_path(depths: &[usize], active: &[bool], sel: usize) -> Vec<usize> {
    if sel >= depths.len() {
        return Vec::new();
    }
    let parents = chart_parents(depths);
    let mut path = vec![sel];
    let mut at = sel;
    while let Some(p) = parents[at] {
        path.push(p);
        at = p;
    }
    path.reverse();
    let mut at = sel;
    loop {
        let kids: Vec<usize> = (at + 1..depths.len()).filter(|&i| parents[i] == Some(at)).collect();
        let Some(&next) = kids.iter().find(|&&i| active.get(i).copied().unwrap_or(false)).or(kids.first()) else {
            break;
        };
        path.push(next);
        at = next;
    }
    path
}

/// `s` cut or padded to exactly `w` columns, with `…` where it was cut.
fn fit(s: &str, w: usize) -> String {
    let sw = s.width();
    if sw <= w {
        return format!("{s}{}", " ".repeat(w - sw));
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out + &" ".repeat(w - used - 1)
}

/// `s` without its first `n` columns; half of a wide character becomes a space.
fn skip_cols(s: &str, n: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used >= n {
            out.push(ch);
        } else if used + cw > n {
            out.push_str(&" ".repeat(used + cw - n));
        }
        used += cw;
    }
    out
}

/// How much of a block the chart draws: its box, one line (each at most
/// so many columns wide), or its number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChartLevel {
    Box(usize),
    Line(usize),
    Number,
}

/// The label in a box's top border, when there is room for it.
fn chart_label(n: &ChartNode, w: usize) -> String {
    if n.kind.is_empty() || n.kind.width() + 4 > w { String::new() } else { format!(" {} ", n.kind) }
}

/// A block's number: its title's first word (after a tag's `*`).
fn chart_number(title: &str) -> &str {
    title.trim_start_matches("* ").split_whitespace().next().unwrap_or("")
}

fn chart_width(n: &ChartNode, level: ChartLevel, aw: usize) -> usize {
    let w = match level {
        ChartLevel::Box(cap) => (n.title.width().max(n.info.width()) + 4).max(n.kind.width() + 5).min(cap),
        ChartLevel::Line(cap) => (n.title.width() + 2).min(cap),
        ChartLevel::Number => chart_number(&n.title).width() + 2,
    };
    w.min(aw).max(1)
}

/// Where a block is joined to the rows above and below: a box's middle,
/// or just past its label when the label reaches that far; a line's middle.
fn chart_join_at(n: &ChartNode, w: usize, level: ChartLevel) -> usize {
    match level {
        ChartLevel::Box(_) => (w / 2).max(chart_label(n, w).width() + 1).min(w.saturating_sub(2)),
        _ => w / 2,
    }
}

/// A block's lines, each `w` columns. A box: what it is in the top border
/// (`up` puts a join there), its title, its info (not for the root), the
/// bottom border (`down` puts a join there). One line: `[title]`; a number:
/// `[n]`.
fn chart_lines(n: &ChartNode, w: usize, level: ChartLevel, root: bool, up: bool, down: bool) -> Vec<String> {
    if !matches!(level, ChartLevel::Box(_)) {
        let text = if level == ChartLevel::Number { chart_number(&n.title) } else { n.title.as_str() };
        return vec![if w >= 2 { format!("[{}]", fit(text, w - 2)) } else { fit(text, w) }];
    }
    let at = chart_join_at(n, w, level);
    let edge = |l: &str, r: &str, label: &str, join: Option<char>| -> String {
        if w < 2 {
            return fit("", w);
        }
        let mut cs: Vec<char> = format!("{l}{label}{}{r}", "─".repeat(w - 2 - label.width())).chars().collect();
        if let Some(j) = join
            && cs.get(at) == Some(&'─')
        {
            cs[at] = j;
        }
        cs.into_iter().collect()
    };
    let mid = |text: &str| if w >= 4 { format!("│ {} │", fit(text, w - 4)) } else { fit(text, w) };
    let mut out = vec![edge("┌", "┐", &chart_label(n, w), up.then_some('┴')), mid(&n.title)];
    if !root {
        out.push(mid(&n.info));
    }
    out.push(edge("└", "┘", "", down.then_some('┬')));
    out
}

/// The line from a parent at `parent` across to its children at `mids`,
/// on row `y`.
fn chart_bar(g: &mut Grid, area: Rect, y: u16, parent: i32, mids: &[i32], style: Style) {
    let aw = area.w as i32;
    let lo = mids.iter().copied().chain([parent]).min().unwrap_or(parent);
    let hi = mids.iter().copied().chain([parent]).max().unwrap_or(parent);
    for x in lo.max(0)..=hi.min(aw - 1) {
        let (up, down, left, right) = (x == parent, mids.contains(&x), x > lo, x < hi);
        let ch = match (up, down, left, right) {
            (true, true, true, true) => "┼",
            (true, true, true, false) => "┤",
            (true, true, false, true) => "├",
            (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => "│",
            (true, false, true, true) => "┴",
            (false, true, true, true) => "┬",
            (true, false, true, false) => "┘",
            (true, false, false, true) => "└",
            (false, true, true, false) => "┐",
            (false, true, false, true) => "┌",
            _ => "─",
        };
        g.put_str(area.x + x as u16, y, ch, style, 1);
    }
}

/// The session tree as a chart (`choose-tree-style chart`): the `keepane`
/// root on the first row; then the machines when this one is paired; the
/// sessions; the windows; the panes. Each row is spread across and joined
/// to its parents by lines. Every block is an open box in the terminal's
/// own colours with what it is in its top border, the selected one in the
/// highlight colour; the names on the selected branch are bold.
///
/// `all` off, only the selected branch is open: the sessions of the
/// machine on it, the windows of its session, that window's panes; a row
/// wider than the screen scrolls to keep its block on the branch in view.
/// `all` on (`a`), every node is drawn, each parent centred over what is
/// under it; when that does not fit, the blocks shrink to one line, then to
/// their numbers, and what is still too wide scrolls to the selection.
/// `‹` `›` say a row goes on past the edge.
pub fn draw_chart(g: &mut Grid, area: Rect, nodes: &[ChartNode], sel: usize, all: bool, actions: &str, status: &str) {
    if area.h == 0 || area.w == 0 {
        return;
    }
    g.fill(area, Style::colors(Color::Default, Color::Default));
    let line = Style::colors(Color::Default, Color::Default);
    let depths: Vec<usize> = nodes.iter().map(|n| n.depth).collect();
    let active: Vec<bool> = nodes.iter().map(|n| n.active).collect();
    let path = chart_path(&depths, &active, sel);
    let parents = chart_parents(&depths);
    let kids = |p: usize| -> Vec<usize> { (p + 1..nodes.len()).filter(|&i| parents[i] == Some(p)).collect() };
    let aw = area.w as i32;
    let body_h = area.h.saturating_sub(1) as usize;
    // The rows of blocks, top down, and for each how deep it is.
    let rows: Vec<Vec<usize>> = if all {
        let deepest = depths.iter().copied().max().unwrap_or(0);
        (0..=deepest).map(|d| (0..nodes.len()).filter(|&i| depths[i] == d).collect::<Vec<_>>()).collect()
    } else {
        let mut rows = Vec::new();
        if let Some(&root) = path.first() {
            rows.push(vec![root]);
        }
        for &p in &path {
            let k = kids(p);
            if k.is_empty() {
                break;
            }
            rows.push(k);
        }
        rows
    };
    let rows: Vec<Vec<usize>> = rows.into_iter().filter(|r| !r.is_empty()).collect();
    if rows.is_empty() {
        draw_chooser_footer(g, area, nodes.len(), sel, "hjkl move", actions, status);
        return;
    }
    // How tall it is: the root's box three lines (one when short of room),
    // the others' four; one line each smaller; between rows, two lines of
    // joins, else one, else none. Counted for the whole tree's depth, not the
    // branch shown: a shorter branch takes the same size and place, so the
    // chart does not jump as the cursor moves.
    let depth = depths.iter().copied().max().unwrap_or(0) + 1;
    let need = |level: ChartLevel, root: usize, links: usize| {
        let h = if matches!(level, ChartLevel::Box(_)) { 4 } else { 1 };
        root + (depth - 1) * (h + links)
    };
    let gap = |level: ChartLevel| if matches!(level, ChartLevel::Box(_)) { CHART_GAP } else { 1 };
    // The whole tree, laid out at a size: each block's x and width.
    let lay_all = |level: ChartLevel| -> (Vec<i32>, Vec<usize>, i32) {
        let ws: Vec<usize> = nodes.iter().map(|n| chart_width(n, level, area.w as usize)).collect();
        let gp = gap(level);
        // Each subtree's width, children first (they come after their parent).
        let mut span = ws.clone();
        for i in (0..nodes.len()).rev() {
            let k = kids(i);
            if !k.is_empty() {
                let under = k.iter().map(|&c| span[c]).sum::<usize>() + gp * (k.len() - 1);
                span[i] = span[i].max(under);
            }
        }
        let mut left = vec![0i32; nodes.len()];
        let mut next = 0i32;
        for (i, p) in parents.iter().enumerate() {
            if p.is_none() {
                left[i] = next;
                next += (span[i] + gp) as i32;
            }
        }
        let total = (next - gp as i32).max(0);
        for i in 0..nodes.len() {
            let k = kids(i);
            let under = (k.iter().map(|&c| span[c]).sum::<usize>() + gp * k.len().saturating_sub(1)) as i32;
            let mut x = left[i] + (span[i] as i32 - under) / 2;
            for c in k {
                left[c] = x;
                x += (span[c] + gp) as i32;
            }
        }
        // A leaf in the middle of its span; a parent over the middle of its
        // first and last child (children first: they come after it).
        let mut xs: Vec<i32> = (0..nodes.len()).map(|i| left[i] + (span[i] - ws[i]) as i32 / 2).collect();
        for i in (0..nodes.len()).rev() {
            let k = kids(i);
            if let (Some(&a), Some(&b)) = (k.first(), k.last()) {
                let centre = (xs[a] + ws[a] as i32 / 2 + xs[b] + ws[b] as i32 / 2) / 2;
                xs[i] = (centre - ws[i] as i32 / 2).clamp(left[i], left[i] + (span[i] - ws[i]) as i32);
            }
        }
        (xs, ws, total)
    };
    // The size: branch mode keeps its boxes; the whole tree takes the first
    // that fits, else the numbers, scrolled.
    let (level, root_h, links) = if all {
        // Boxes, narrower and narrower; then one line each; then numbers.
        let mut tries = Vec::new();
        for cap in [CHART_BLOCK + 2, 20, 14] {
            tries.extend([(ChartLevel::Box(cap), 3, 2), (ChartLevel::Box(cap), 3, 1), (ChartLevel::Box(cap), 1, 1)]);
        }
        for cap in [20, 12] {
            tries.extend([(ChartLevel::Line(cap), 1, 2), (ChartLevel::Line(cap), 1, 1)]);
        }
        tries.extend([(ChartLevel::Number, 1, 2), (ChartLevel::Number, 1, 1)]);
        tries
            .into_iter()
            .find(|&(l, r, k)| need(l, r, k) <= body_h && lay_all(l).2 <= aw)
            .or_else(|| {
                [(ChartLevel::Number, 1, 2), (ChartLevel::Number, 1, 1)]
                    .into_iter()
                    .find(|&(l, r, k)| need(l, r, k) <= body_h)
            })
            .unwrap_or((ChartLevel::Number, 1, 0))
    } else {
        [(3, 2), (3, 1), (1, 1), (3, 0), (1, 0)]
            .into_iter()
            .find(|&(r, k)| need(ChartLevel::Box(CHART_BLOCK + 2), r, k) <= body_h)
            .map(|(r, k)| (ChartLevel::Box(CHART_BLOCK + 2), r, k))
            .unwrap_or((ChartLevel::Box(CHART_BLOCK + 2), 1, 0))
    };
    // Each block's x and width.
    let mut xs = vec![0i32; nodes.len()];
    let mut ws = vec![0usize; nodes.len()];
    if all {
        let (x, w, total) = lay_all(level);
        // Centred when it fits; else scrolled so that the selection is in
        // the middle, no blank past either end.
        let shift = if total <= aw {
            (aw - total) / 2
        } else {
            let mid = x.get(sel).copied().unwrap_or(0) + w.get(sel).copied().unwrap_or(0) as i32 / 2;
            (aw / 2 - mid).clamp(aw - total, 0)
        };
        xs = x.iter().map(|x| x + shift).collect();
        ws = w;
    } else {
        let mut parent_mid = aw / 2;
        for row in &rows {
            let rw: Vec<usize> = row.iter().map(|&i| chart_width(&nodes[i], level, area.w as usize)).collect();
            let total = (rw.iter().sum::<usize>() + CHART_GAP * (rw.len() - 1)) as i32;
            let on = row.iter().position(|i| path.contains(i)).unwrap_or(0);
            let before = (rw[..on].iter().sum::<usize>() + CHART_GAP * on) as i32;
            // Centred under its parent when it fits; else scrolled so that
            // the block on the branch is in the middle.
            let mut x = if total <= aw {
                (parent_mid - total / 2).clamp(0, aw - total)
            } else {
                -(before + rw[on] as i32 / 2 - aw / 2).clamp(0, total - aw)
            };
            for (&i, &w) in row.iter().zip(&rw) {
                xs[i] = x;
                ws[i] = w;
                x += (w + CHART_GAP) as i32;
            }
            let i = row[on];
            parent_mid = xs[i] + chart_join_at(&nodes[i], ws[i], level) as i32;
        }
    }
    let mid = |i: usize| xs[i] + chart_join_at(&nodes[i], ws[i], level) as i32;
    let style_of = |i: usize, name: bool| {
        if i == sel {
            Style { bold: true, ..Style::colors(Color::Idx(3), Color::Default) }
        } else {
            Style { bold: name && path.contains(&i), ..Style::colors(Color::Default, Color::Default) }
        }
    };
    // Up and down in the middle too, when it fits; else from the top.
    let (top, bottom) = (area.y as usize, area.y as usize + body_h);
    let mut y = top + body_h.saturating_sub(need(level, root_h, links)) / 2;
    for (r, row) in rows.iter().enumerate() {
        // The joins from the row above: down from each parent, then across.
        let shown: Vec<usize> = if r + 1 < rows.len() { rows[r + 1].clone() } else { Vec::new() };
        if r > 0 && links > 0 {
            for &p in &rows[r - 1] {
                let under: Vec<i32> = row.iter().filter(|&&c| parents[c] == Some(p)).map(|&c| mid(c)).collect();
                if under.is_empty() {
                    continue;
                }
                if links == 2 && y < bottom && (0..aw).contains(&mid(p)) {
                    g.put_str(area.x + mid(p) as u16, y as u16, "│", line, 1);
                }
                if y + links - 1 < bottom {
                    chart_bar(g, area, (y + links - 1) as u16, mid(p), &under, line);
                }
            }
            y += links;
        }
        let height = match (level, r) {
            (ChartLevel::Box(_), 0) => root_h,
            (ChartLevel::Box(_), _) => 4,
            _ => 1,
        };
        for &i in row {
            let (x, w) = (xs[i], ws[i]);
            if x >= aw || x + w as i32 <= 0 {
                continue;
            }
            let joined = links > 0 && matches!(level, ChartLevel::Box(_));
            let down = joined && shown.iter().any(|&c| parents[c] == Some(i));
            let lines = if r == 0 && height == 1 {
                vec![fit(&format!("  {}", nodes[i].title), w)]
            } else {
                chart_lines(&nodes[i], w, level, r == 0, joined && r > 0, down)
            };
            for (dy, s) in lines.iter().enumerate() {
                let yy = y + dy;
                if yy >= bottom {
                    break;
                }
                let (sx, s) = if x < 0 { (0, skip_cols(s, (-x) as usize)) } else { (x, s.clone()) };
                g.put_str(area.x + sx as u16, yy as u16, &s, style_of(i, dy == 1 || height == 1), (aw - sx) as u16);
            }
        }
        // More of the row than the screen shows: a mark at that end, on
        // the line of the names.
        let names = y + usize::from(height > 1);
        if names < bottom && row.iter().any(|&i| xs[i] < 0) {
            g.put_str(area.x, names as u16, "‹", line, 1);
        }
        if names < bottom && row.iter().any(|&i| xs[i] + ws[i] as i32 > aw) {
            g.put_str(area.x + area.w - 1, names as u16, "›", line, 1);
        }
        y += height;
    }
    draw_chooser_footer(g, area, nodes.len(), sel, "hjkl move", actions, status);
}
/// A picker's last line: where the cursor is of how many, the keys, and a
/// note (tags, the filter) at the right end.
fn draw_chooser_footer(g: &mut Grid, area: Rect, count: usize, sel: usize, moves: &str, actions: &str, status: &str) {
    let hint = format!("[{}/{count}] {moves}  {actions}  q quit", if count == 0 { 0 } else { sel + 1 });
    let hint_style = Style::colors(Color::Idx(0), Color::Idx(3));
    let y = area.y + area.h - 1;
    g.fill(Rect { x: area.x, y, w: area.w, h: 1 }, hint_style);
    g.put_str(area.x, y, &hint, hint_style, area.w);
    if !status.is_empty() {
        let w = (status.width() as u16).min(area.w);
        let x = area.x + area.w - w;
        // A space before it so it never runs into a clipped hint.
        if x > area.x {
            g.put_str(x - 1, y, " ", hint_style, 1);
        }
        g.put_str(x, y, status, hint_style, w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_number_is_drawn_in_its_colour() {
        // `█` shows its foreground: a black foreground made the digits
        // invisible on a dark background.
        let mut g = Grid::new(20, 7);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 20, h: 7 }, 1, Color::Idx(1), None, "normal");
        let blocks: Vec<&Cell> = (0..7)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .map(|(x, y)| g.get(x, y))
            .filter(|c| c.text() == "█")
            .collect();
        assert!(!blocks.is_empty(), "the digit is drawn");
        assert!(blocks.iter().all(|c| c.style.fg == Color::Idx(1)), "every block in the colour");
    }

    /// A pane's name and mode go under its number, centred, in the same
    /// colour; a pane too small for block digits gets them as text; no name,
    /// the mode alone.
    #[test]
    fn a_named_pane_shows_its_name_under_its_number() {
        let row = |g: &Grid, y: u16, w: u16| (0..w).map(|x| g.get(x, y).text().to_string()).collect::<String>();
        let mut g = Grid::new(20, 9);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 20, h: 9 }, 3, Color::Idx(2), Some("builder"), "shell");
        // Digits on rows 2..7 (centred in 9), a blank row, the name on row 8.
        assert_eq!(row(&g, 8, 20).trim(), "%builder · shell");
        assert_eq!(row(&g, 7, 20).trim(), "", "a blank row between");
        assert_eq!(g.get(6, 8).style.fg, Color::Idx(2));
        // Room for the digits and nothing below: right under them.
        let mut g = Grid::new(20, 6);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 20, h: 6 }, 3, Color::Idx(2), Some("b"), "ai");
        assert_eq!(row(&g, 5, 20).trim(), "%b · ai");
        // Too small for block digits: `3 %builder` in the corner, cut to fit.
        let mut g = Grid::new(8, 3);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 8, h: 3 }, 3, Color::Idx(2), Some("builder"), "shell");
        assert_eq!(row(&g, 0, 8), "3 %build");
        // No name: the mode alone; a wide label is cut to the pane.
        let mut g = Grid::new(20, 9);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 20, h: 9 }, 3, Color::Idx(2), None, "normal");
        assert_eq!(row(&g, 8, 20).trim(), "normal");
        let mut g = Grid::new(10, 9);
        draw_pane_number(&mut g, Rect { x: 0, y: 0, w: 10, h: 9 }, 3, Color::Idx(2), Some("builder"), "shell");
        assert_eq!(row(&g, 8, 10), "%builder ·");
    }

    #[test]
    fn the_focus_frame_moves_and_stays_whole() {
        let a = Rect { x: 0, y: 0, w: 10, h: 4 };
        let b = Rect { x: 20, y: 10, w: 40, h: 12 };
        assert_eq!(frame_at(a, b, 0.0), a);
        assert_eq!(frame_at(a, b, 1.0), b);
        assert_eq!(frame_at(a, b, 7.0), b, "past the end stays at the end");
        // Eased out: more than half way at half time, and never back.
        let mid = frame_at(a, b, 0.5);
        assert!(mid.x > 10 && mid.w > 25, "{mid:?}");
        let xs: Vec<u16> = (0..=10).map(|i| frame_at(a, b, i as f32 / 10.0).x).collect();
        assert!(xs.windows(2).all(|p| p[0] <= p[1]), "{xs:?}");
        // Too small to draw, or off the grid: nothing, no panic.
        let mut g = Grid::new(8, 4);
        let before = g.clone();
        for r in [Rect { x: 0, y: 0, w: 1, h: 3 }, Rect { x: 0, y: 0, w: 3, h: 1 }, Rect { x: 0, y: 0, w: 0, h: 0 }] {
            draw_frame(&mut g, r, Style::default());
        }
        assert_eq!(g, before);
        draw_frame(&mut g, Rect { x: 6, y: 2, w: 10, h: 10 }, Style::default());
        assert_eq!(g.get(6, 2).text(), "╭");
        // Over a wide character: its other half goes too, none is left
        // half drawn.
        let mut g = Grid::new(8, 3);
        g.put_str(0, 0, "中文字", Style::default(), 8);
        draw_frame(&mut g, Rect { x: 1, y: 0, w: 4, h: 3 }, Style::default());
        let row: Vec<(String, bool, bool)> =
            (0..8).map(|x| (g.get(x, 0).text().to_string(), g.get(x, 0).wide, g.get(x, 0).cont)).collect();
        for (x, (_, wide, _)) in row.iter().enumerate() {
            if *wide {
                assert!(row.get(x + 1).is_some_and(|c| c.2), "a wide cell keeps its second half: {row:?}");
            }
        }
        for (x, (_, _, cont)) in row.iter().enumerate() {
            if *cont {
                assert!(x > 0 && row[x - 1].1, "a second half keeps its first: {row:?}");
            }
        }
    }

    #[test]
    fn a_stamp_goes_only_into_blank_cells_that_fit_it() {
        let parts = vec![("12:00:00".to_string(), Style::default()), (" ✓".to_string(), Style::default())];
        let row = |g: &Grid, y: u16| (0..g.cols).map(|x| g.get(x, y).text().to_string()).collect::<String>();
        let mut g = Grid::new(30, 3);
        g.put_str(0, 0, "PS> ls", Style::default(), 30);
        let r = Rect { x: 0, y: 0, w: 30, h: 3 };
        assert!(draw_stamp(&mut g, r, 0, &parts));
        assert_eq!(row(&g, 0), "PS> ls              12:00:00 ✓");
        // A line that reaches the end, a row past the rectangle, a
        // rectangle narrower than the stamp: nothing drawn, no panic.
        g.put_str(0, 1, &"x".repeat(25), Style::default(), 30);
        assert!(!draw_stamp(&mut g, r, 1, &parts));
        assert!(!draw_stamp(&mut g, r, 3, &parts));
        for w in 0..12 {
            assert!(!draw_stamp(&mut g, Rect { x: 0, y: 2, w, h: 1 }, 0, &parts), "width {w}");
        }
        assert_eq!(row(&g, 2).trim(), "");
        // A coloured blank (a bar the program drew) is not blank enough.
        let bar = Style::colors(Color::Default, Color::Idx(4));
        g.fill(Rect { x: 20, y: 2, w: 10, h: 1 }, bar);
        assert!(!draw_stamp(&mut g, Rect { x: 0, y: 2, w: 30, h: 1 }, 0, &parts));
    }

    #[test]
    fn chooser_highlights_selection_and_scrolls() {
        let mut g = Grid::new(40, 4);
        let lines: Vec<String> = (0..6).map(|i| format!("item{i}")).collect();
        let row = |g: &Grid, y: u16| (0..40).map(|x| g.get(x, y).text()).collect::<String>();
        draw_chooser(&mut g, Rect { x: 0, y: 0, w: 40, h: 4 }, &lines, 1, 0, "Enter select", "");
        assert_eq!(row(&g, 0).trim_end(), "item0");
        assert_eq!(row(&g, 1).trim_end(), "item1");
        assert_eq!(row(&g, 2).trim_end(), "item2");
        assert_eq!(g.get(0, 1).style.bg, Color::Idx(3), "selected line highlighted");
        assert_eq!(g.get(39, 1).style.bg, Color::Idx(3), "highlight spans the row");
        assert_eq!(g.get(0, 0).style.bg, Color::Default);
        assert!(row(&g, 3).starts_with("[2/6] j/k move"), "{}", row(&g, 3));
        // Scrolled: top=3 shows items 3..5, selection 5 on the last body row.
        draw_chooser(&mut g, Rect { x: 0, y: 0, w: 40, h: 4 }, &lines, 5, 3, "Enter select", "");
        assert_eq!(row(&g, 0).trim_end(), "item3");
        assert_eq!(row(&g, 2).trim_end(), "item5");
        assert_eq!(g.get(0, 2).style.bg, Color::Idx(3));
        assert!(row(&g, 3).starts_with("[6/6]"));
        // Empty list and degenerate areas never panic.
        draw_chooser(&mut g, Rect { x: 0, y: 0, w: 40, h: 4 }, &[], 0, 0, "Enter select", "");
        assert!(row(&g, 3).starts_with("[0/0]"));
        draw_chooser(&mut g, Rect { x: 0, y: 0, w: 0, h: 0 }, &lines, 0, 0, "Enter select", "");
        draw_chooser(&mut g, Rect { x: 0, y: 0, w: 40, h: 1 }, &lines, 0, 0, "Enter select", "");
    }

    #[test]
    fn overlay_draws_lines_and_hint() {
        let mut g = Grid::new(40, 4);
        g.put_str(0, 0, "underneath", Style::default(), 40);
        let lines: Vec<String> = (0..5).map(|i| format!("line{i}")).collect();
        draw_overlay(&mut g, Rect { x: 0, y: 0, w: 40, h: 3 }, &lines);
        let row = |g: &Grid, y: u16| (0..40).map(|x| g.get(x, y).text()).collect::<String>();
        assert_eq!(row(&g, 0).trim_end(), "line0");
        assert_eq!(row(&g, 1).trim_end(), "line1");
        assert_eq!(row(&g, 2).trim_end(), "[2 of 5 lines] press any key");
        assert_eq!(g.get(0, 2).style.bg, Color::Idx(3));
        // Row 3 is outside the area and untouched.
        assert_eq!(row(&g, 3).trim_end(), "");
        draw_overlay(&mut g, Rect { x: 0, y: 0, w: 40, h: 3 }, &lines[..1]);
        assert_eq!(row(&g, 2).trim_end(), "press any key");
        assert_eq!(row(&g, 1).trim_end(), "");
    }

    #[test]
    fn the_prefix_panel_sits_at_the_bottom_a_column_a_group() {
        use super::super::keyhint::Entry;
        let e = |k: &str, l: &str| Entry { keys: k.into(), label: l.into() };
        let groups = vec![
            ("Panes", vec![e("%", "split side by side"), e("hjkl", "go to a pane")]),
            ("Windows", vec![e("c", "new window")]),
        ];
        let rows = |g: &Grid, w: u16, h: u16| -> Vec<String> {
            (0..h).map(|y| (0..w).map(|x| g.get(x, y).text()).collect::<String>().trim_end().to_string()).collect()
        };
        // Wide enough: side by side, at the bottom, centred, in a box.
        let mut g = Grid::new(80, 10);
        g.put_str(0, 0, "the pane", Style::default(), 80);
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 80, h: 10 }, "C-b", &groups, "? every key");
        let r = rows(&g, 80, 10);
        assert_eq!(r[0], "the pane", "above the box, the pane as it was");
        assert!(r[5].trim_start().starts_with("┌─ C-b ─"), "{r:?}");
        assert!(r[6].contains("│ Panes") && r[6].contains("Windows"), "{r:?}");
        assert!(r[7].contains("%     split side by side") && r[7].contains("c  new window"), "{r:?}");
        assert!(r[8].contains("hjkl  go to a pane"), "{r:?}");
        assert!(r[9].trim_start().starts_with("└─ ? every key ─"), "{r:?}");
        let left = r[6].find('│').unwrap();
        assert_eq!(left, 80 - r[6].trim_start().chars().count() - left, "centred");
        assert_eq!(g.get(left as u16 + 2, 7).style.fg, Color::Idx(3), "the keys stand out");
        // Narrow: the second group below the first.
        let mut g = Grid::new(30, 12);
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 30, h: 12 }, "C-b", &groups, "? every key");
        let r = rows(&g, 30, 12);
        let at = |s: &str| r.iter().position(|l| l.contains(s)).unwrap();
        assert!(at("Windows") > at("hjkl"), "{r:?}");
        // Too low for all of it: the rows that fit; too narrow: nothing.
        let mut g = Grid::new(80, 4);
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 80, h: 4 }, "C-b", &groups, "? every key");
        let r = rows(&g, 80, 4);
        assert!(r[0].contains("┌") && r[3].contains("└") && r[2].contains("split side by side"), "{r:?}");
        let mut g = Grid::new(8, 6);
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 8, h: 6 }, "C-b", &groups, "? every key");
        assert!(rows(&g, 8, 6).iter().all(|l| l.is_empty()));
        // No groups (every key unbound), and an area of nothing: no box, no panic.
        let mut g = Grid::new(80, 10);
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 80, h: 10 }, "C-b", &[], "? every key");
        assert!(rows(&g, 80, 10).iter().all(|l| l.is_empty()));
        draw_key_hint(&mut g, Rect { x: 0, y: 0, w: 0, h: 0 }, "C-b", &groups, "? every key");
    }

    fn screen(cols: u16, rows: u16, input: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process(input);
        p
    }

    #[test]
    fn diff_identical_is_cursor_only() {
        let g = Grid::new(10, 3);
        let out = diff(Some(&g), &g, Some((1, 1)));
        assert_eq!(out, b"\x1b[?25l\x1b[0m\x1b[2;2H\x1b[?25h");
        let out = diff(Some(&g), &g, None);
        assert_eq!(out, b"\x1b[?25l\x1b[0m");
    }

    #[test]
    fn diff_full_redraw_clears() {
        let mut g = Grid::new(4, 1);
        g.put_str(0, 0, "ab", Style::default(), 4);
        let out = String::from_utf8(diff(None, &g, None)).unwrap();
        assert!(out.contains("\x1b[2J"));
        assert!(out.contains("\x1b[1;1H"));
        assert!(out.contains("ab  "));
    }

    #[test]
    fn diff_single_change_and_style() {
        let a = Grid::new(10, 2);
        let mut b = a.clone();
        b.put_str(3, 1, "X", Style::colors(Color::Idx(1), Color::Idx(4)), 1);
        let out = String::from_utf8(diff(Some(&a), &b, None)).unwrap();
        assert_eq!(out, "\x1b[?25l\x1b[2;4H\x1b[0;31;44mX\x1b[0m");
    }

    #[test]
    fn diff_merges_small_gaps_and_splits_big_ones() {
        let a = Grid::new(30, 1);
        let mut b = a.clone();
        b.put_str(0, 0, "A", Style::default(), 1);
        b.put_str(3, 0, "B", Style::default(), 1); // gap 2 -> merged
        b.put_str(20, 0, "C", Style::default(), 1); // gap 16 -> new run
        let out = String::from_utf8(diff(Some(&a), &b, None)).unwrap();
        assert_eq!(out.matches("\x1b[1;").count(), 2, "{out:?}");
        assert!(out.contains("A  B"));
        assert!(out.contains("\x1b[1;21HC"));
    }

    #[test]
    fn wide_chars_written_once() {
        let s = screen(6, 1, "中b".as_bytes());
        let mut g = Grid::new(6, 1);
        g.blit_screen(Rect { x: 0, y: 0, w: 6, h: 1 }, s.screen());
        assert!(g.get(0, 0).wide);
        assert!(g.get(1, 0).cont);
        assert_eq!(g.get(2, 0).text(), "b");
        let out = String::from_utf8(diff(None, &g, None)).unwrap();
        assert!(out.contains("中b   "), "{out:?}");
        assert_eq!(out.matches('中').count(), 1);
        // Changing the continuation half only still rewrites from the head.
        let mut h = g.clone();
        h.invert(1, 0);
        let out = String::from_utf8(diff(Some(&g), &h, None)).unwrap();
        assert!(out.starts_with("\x1b[?25l\x1b[1;1H"), "{out:?}");
    }

    #[test]
    fn wide_char_at_pane_edge_is_blanked() {
        let s = screen(4, 1, "ab中".as_bytes());
        let mut g = Grid::new(3, 1);
        g.blit_screen(Rect { x: 0, y: 0, w: 3, h: 1 }, s.screen());
        assert_eq!(g.get(2, 0).text(), " ");
        assert!(!g.get(2, 0).wide);
    }

    fn seg(text: &str, style: Style) -> Vec<Segment> {
        vec![Segment { text: text.into(), style }]
    }

    #[test]
    fn compose_borders_and_status() {
        let st = Style::colors(Color::Idx(0), Color::Idx(2));
        let left = screen(4, 3, b"L");
        let right = screen(15, 3, b"R");
        let f = Frame {
            cols: 20,
            rows: 4,
            panes: vec![
                PaneView {
                    rect: Rect { x: 0, y: 0, w: 4, h: 3 },
                    screen: left.screen(),
                    active: true,
                    copy: None,
                    border_text: None,
                },
                PaneView {
                    rect: Rect { x: 5, y: 0, w: 15, h: 3 },
                    screen: right.screen(),
                    active: false,
                    copy: None,
                    border_text: None,
                },
            ],
            status: Some(StatusLine {
                left: seg("[s] ", Style { bold: true, ..st }),
                windows: vec![(seg("0:a", st), true), (seg("1:b", st), false)],
                separator: " ".into(),
                justify: Justify::Left,
                right: seg("12:00", st),
                message: None,
                prompt: None,
                fg: Color::Idx(0),
                bg: Color::Idx(2),
            }),
            status_top: false,
            border_fg: Color::Idx(8),
            active_border_fg: Color::Idx(2),
        };
        let (g, cursor, hits) = compose(&f);
        assert_eq!(g.get(0, 0).text(), "L");
        assert_eq!(g.get(5, 0).text(), "R");
        for y in 0..3 {
            assert_eq!(g.get(4, y).text(), "│");
            assert_eq!(g.get(4, y).style.fg, Color::Idx(2));
        }
        // Cursor after 'L' in the active pane.
        assert_eq!(cursor, Some((1, 0)));
        let row: String = (0..20).map(|x| g.get(x, 3).text()).collect();
        assert_eq!(row, "[s] 0:a 1:b    12:00");
        assert!(g.get(4, 3).style.inverse);
        assert!(!g.get(8, 3).style.inverse);
        assert_eq!(g.get(0, 3).style.bg, Color::Idx(2));
        assert!(g.get(0, 3).style.bold);
        // Window labels are reported for mouse hit-testing.
        assert_eq!(hits, vec![(4, 7), (8, 11)]);
    }

    #[test]
    fn status_justify_moves_the_window_list_and_the_separator_sits_between() {
        let st = Style::colors(Color::Idx(0), Color::Idx(2));
        let frame = |cols: u16, justify: Justify, sep: &str| {
            let a = screen(cols, 1, b"");
            let f = Frame {
                cols,
                rows: 2,
                panes: vec![PaneView {
                    rect: Rect { x: 0, y: 0, w: cols, h: 1 },
                    screen: a.screen(),
                    active: true,
                    copy: None,
                    border_text: None,
                }],
                status: Some(StatusLine {
                    left: seg("[s] ", st),
                    windows: vec![(seg("0:a", st), true), (seg("1:b", st), false)],
                    separator: sep.into(),
                    justify,
                    right: seg("R", st),
                    message: None,
                    prompt: None,
                    fg: Color::Idx(0),
                    bg: Color::Idx(2),
                }),
                status_top: false,
                border_fg: Color::Idx(8),
                active_border_fg: Color::Idx(2),
            };
            let (g, _, hits) = compose(&f);
            let row: String = (0..cols).map(|x| g.get(x, 1).text()).collect();
            (row, hits)
        };
        // 30 columns: left takes 4, the right side 1 plus a gap, so the
        // list (3 + 1 + 3 = 7 cells) has 24 cells of room, from 4 to 28.
        let (row, hits) = frame(30, Justify::Left, "|");
        assert_eq!(row, "[s] 0:a|1:b                  R");
        assert_eq!(hits, vec![(4, 7), (8, 11)], "the separator is not part of a label");
        let (row, hits) = frame(30, Justify::Centre, "|");
        assert_eq!(row, "[s]         0:a|1:b          R", "centred in the room between the sides");
        assert_eq!(hits, vec![(12, 15), (16, 19)]);
        let (row, hits) = frame(30, Justify::Right, "|");
        assert_eq!(row, "[s]                  0:a|1:b R", "flush against the right side");
        assert_eq!(hits, vec![(21, 24), (25, 28)]);
        let (row, hits) = frame(30, Justify::AbsoluteCentre, "|");
        assert_eq!(row, "[s]        0:a|1:b           R", "centred on the line: (30 - 7) / 2 = 11");
        assert_eq!(hits, vec![(11, 14), (15, 18)]);
        // A wider separator, and one with a double-width character, count
        // by display width.
        let (row, _) = frame(30, Justify::Right, " · ");
        assert_eq!(row, "[s]                0:a · 1:b R");
        let (row, hits) = frame(30, Justify::Right, "│");
        assert_eq!(row, "[s]                  0:a│1:b R");
        assert_eq!(hits, vec![(21, 24), (25, 28)]);
        // Tight: the right side gives way to the whole list (4 + 7 + 1 of
        // 12 leaves it nothing), and the list has no room to move.
        let (row, hits) = frame(12, Justify::Right, "|");
        assert_eq!(row, "[s] 0:a|1:b ");
        assert_eq!(hits, vec![(4, 7), (8, 11)]);
        // Tighter: a window that does not fit is dropped rather than
        // squeezed, and the right side keeps what the current one leaves.
        let (row, hits) = frame(10, Justify::Right, "|");
        assert_eq!(row, "[s] 0:a  R");
        assert_eq!(hits, vec![(4, 7)]);
        assert_eq!(Justify::parse("center"), Justify::Centre);
        assert_eq!(Justify::parse("nonsense"), Justify::Left);
    }

    #[test]
    fn status_segments_keep_their_styles_and_clip() {
        let a = screen(3, 1, b"");
        let st = Style::colors(Color::Idx(7), Color::Idx(0));
        let mut left = seg("ab", st);
        left.extend(seg("cd", Style { fg: Color::Idx(1), ..st }));
        let mut f = Frame {
            cols: 12,
            rows: 2,
            panes: vec![PaneView {
                rect: Rect { x: 0, y: 0, w: 3, h: 1 },
                screen: a.screen(),
                active: true,
                copy: None,
                border_text: None,
            }],
            status: Some(StatusLine {
                left,
                windows: vec![(seg("0:long-name", st), true), (seg("1:x", st), false)],
                separator: " ".into(),
                justify: Justify::Left,
                right: seg("RR", Style { bold: true, ..st }),
                message: None,
                prompt: None,
                fg: Color::Idx(7),
                bg: Color::Idx(0),
            }),
            status_top: false,
            border_fg: Color::Default,
            active_border_fg: Color::Default,
        };
        let (g, _, hits) = compose(&f);
        let row: String = (0..12).map(|x| g.get(x, 1).text()).collect();
        // Left and right keep their styles; a label that does not fit
        // between them is skipped entirely (never drawn half).
        assert_eq!(row, "abcd      RR");
        assert_eq!(g.get(2, 1).style.fg, Color::Idx(1));
        assert!(g.get(10, 1).style.bold);
        assert!(hits.is_empty());
        // The list comes before the right side: both labels fit when the
        // right side gives way; the current one is drawn inverse.
        f.status.as_mut().unwrap().windows = vec![(seg("0:x", st), true), (seg("1:y", st), false)];
        let (g, _, hits) = compose(&f);
        let row: String = (0..12).map(|x| g.get(x, 1).text()).collect();
        assert_eq!(row, "abcd0:x 1:y ");
        assert!(g.get(4, 1).style.inverse);
        assert_eq!(hits, vec![(4, 7), (8, 11)]);
        // A long right side is clipped the same way.
        f.status.as_mut().unwrap().right = seg("0123456789ab", Style { bold: true, ..st });
        let (g, _, hits) = compose(&f);
        let row: String = (0..12).map(|x| g.get(x, 1).text()).collect();
        assert_eq!(row, "abcd0:x 1:y ");
        assert_eq!(hits, vec![(4, 7), (8, 11)]);
        // The list does not fit at all: windows before the current one are
        // left out (and never hit), the current one is always shown, the
        // right side gets what is left. (CI caught this: a long default
        // right side and the current window last hid the current window.)
        let status = f.status.as_mut().unwrap();
        status.right = seg("RR", st);
        status.windows = vec![(seg("0:aaa", st), false), (seg("1:bbb", st), false), (seg("2:ccc", st), true)];
        let (g, _, hits) = compose(&f);
        let row: String = (0..12).map(|x| g.get(x, 1).text()).collect();
        assert_eq!(row, "abcd2:ccc RR");
        assert!(g.get(4, 1).style.inverse);
        assert_eq!(hits, vec![(0, 0), (0, 0), (4, 9)], "left-out windows keep their index, empty");
        // The current window in the middle: those after it follow if room.
        let status = f.status.as_mut().unwrap();
        status.right = seg("", st);
        status.windows = vec![(seg("0:a", st), false), (seg("1:bb", st), true), (seg("2:c", st), false)];
        f.cols = 10;
        let (g, _, hits) = compose(&f);
        let row: String = (0..10).map(|x| g.get(x, 1).text()).collect();
        assert_eq!(row, "abcd1:bb  ", "0:a left out so 1:bb fits; 2:c does not");
        assert_eq!(hits, vec![(0, 0), (4, 8)]);
    }

    #[test]
    fn compose_degenerate_sizes() {
        let a = screen(5, 2, b"ab");
        for (cols, rows) in [(1u16, 1u16), (1, 2), (2, 1), (3, 2)] {
            let f = Frame {
                cols,
                rows,
                panes: vec![
                    // Rects that are larger than, empty, or outside the client grid.
                    PaneView {
                        rect: Rect { x: 0, y: 0, w: 5, h: 2 },
                        screen: a.screen(),
                        active: true,
                        copy: None,
                        border_text: None,
                    },
                    PaneView {
                        rect: Rect { x: 1, y: 0, w: 0, h: 2 },
                        screen: a.screen(),
                        active: false,
                        copy: None,
                        border_text: None,
                    },
                    PaneView {
                        rect: Rect { x: 40, y: 40, w: 5, h: 2 },
                        screen: a.screen(),
                        active: false,
                        copy: None,
                        border_text: None,
                    },
                ],
                status: Some(StatusLine {
                    left: seg("[a-very-long-session-name] ", Style::default()),
                    windows: vec![(seg("0:x", Style::default()), true)],
                    separator: " ".into(),
                    justify: Justify::Left,
                    right: seg("right", Style::default()),
                    message: None,
                    prompt: Some(("(p) ".into(), "typed".into(), 3)),
                    fg: Color::Default,
                    bg: Color::Default,
                }),
                status_top: false,
                border_fg: Color::Default,
                active_border_fg: Color::Default,
            };
            let (g, cursor, _) = compose(&f);
            assert_eq!((g.cols, g.rows), (cols, rows));
            if let Some((x, y)) = cursor {
                assert!(x < cols && y < rows, "{cols}x{rows}: cursor {cursor:?}");
            }
            // Diffing against a differently sized previous grid is a full redraw.
            let prev = Grid::new(cols + 1, rows);
            let out = diff(Some(&prev), &g, cursor);
            assert!(out.windows(4).any(|w| w == b"\x1b[2J"));
        }
        // An overlay on a 1-row area draws only the hint.
        let mut g = Grid::new(5, 1);
        draw_overlay(&mut g, Rect { x: 0, y: 0, w: 5, h: 1 }, &["x".into()]);
        assert_eq!((0..5).map(|x| g.get(x, 0).text()).collect::<String>(), "[0 of");
    }

    #[test]
    fn compose_junctions() {
        // Three panes: left column full height; right column split in two.
        let a = screen(3, 5, b"");
        let f = Frame {
            cols: 7,
            rows: 5,
            panes: vec![
                PaneView {
                    rect: Rect { x: 0, y: 0, w: 3, h: 5 },
                    screen: a.screen(),
                    active: false,
                    copy: None,
                    border_text: None,
                },
                PaneView {
                    rect: Rect { x: 4, y: 0, w: 3, h: 2 },
                    screen: a.screen(),
                    active: true,
                    copy: None,
                    border_text: None,
                },
                PaneView {
                    rect: Rect { x: 4, y: 3, w: 3, h: 2 },
                    screen: a.screen(),
                    active: false,
                    copy: None,
                    border_text: None,
                },
            ],
            status: None,
            status_top: false,
            border_fg: Color::Default,
            active_border_fg: Color::Default,
        };
        let (g, _, _) = compose(&f);
        assert_eq!(g.get(3, 2).text(), "├");
        assert_eq!(g.get(5, 2).text(), "─");
        assert_eq!(g.get(3, 0).text(), "│");
    }

    #[test]
    fn compose_prompt_and_message() {
        let a = screen(5, 1, b"");
        let mut f = Frame {
            cols: 12,
            rows: 2,
            panes: vec![PaneView {
                rect: Rect { x: 0, y: 0, w: 5, h: 1 },
                screen: a.screen(),
                active: true,
                copy: None,
                border_text: None,
            }],
            status: Some(StatusLine {
                left: seg("[s] ", Style::default()),
                windows: vec![],
                separator: " ".into(),
                justify: Justify::Left,
                right: Vec::new(),
                message: Some("hello".into()),
                prompt: None,
                fg: Color::Default,
                bg: Color::Default,
            }),
            status_top: true,
            border_fg: Color::Default,
            active_border_fg: Color::Default,
        };
        let (g, _, _) = compose(&f);
        let row: String = (0..12).map(|x| g.get(x, 0).text()).collect();
        assert_eq!(row, "hello       ");
        f.status.as_mut().unwrap().prompt = Some(("(rename) ".into(), "ab".into(), 1));
        let (_, cursor, _) = compose(&f);
        assert_eq!(cursor, Some((10, 0)));
    }

    /// `pane-border-status`: the text lands on the row the layout left free,
    /// above or below the pane, clipped to the pane's columns, and a pane
    /// flush with the top has nowhere to put a top line.
    #[test]
    fn border_text_goes_on_the_reserved_row() {
        let a = screen(6, 2, b"abcdef\r\nghijkl");
        let label = |s: &str| vec![Segment { text: s.to_string(), style: Style::default() }];
        let row = |g: &Grid, y: u16| (0..g.cols).map(|x| g.get(x, y).text()).collect::<String>();
        let frame = |rect: Rect, text: Option<(Vec<Segment>, bool)>| Frame {
            cols: 6,
            rows: 4,
            panes: vec![PaneView { rect, screen: a.screen(), active: true, copy: None, border_text: text }],
            status: None,
            status_top: false,
            border_fg: Color::Default,
            active_border_fg: Color::Default,
        };
        // Top: row 0 is the label, the pane starts on row 1.
        let (g, _, _) = compose(&frame(Rect { x: 0, y: 1, w: 6, h: 2 }, Some((label(" 0: long label"), true))));
        assert_eq!(row(&g, 0), " 0: lo", "clipped to the pane's width");
        assert_eq!(row(&g, 1), "abcdef");
        // Bottom: the row after the pane.
        let (g, _, _) = compose(&frame(Rect { x: 0, y: 0, w: 6, h: 2 }, Some((label("[0]"), false))));
        assert_eq!(row(&g, 0), "abcdef");
        let r2 = row(&g, 2);
        assert!(r2.starts_with("[0]"), "{r2}");
        // The rest of the reserved row is border line, as in tmux.
        assert!(r2[3..].chars().all(|c| "─┬┐┼┤".contains(c)), "{r2}");
        // A pane on row 0 asked for a top line: nothing to draw, nothing broken.
        let (g, _, _) = compose(&frame(Rect { x: 0, y: 0, w: 6, h: 2 }, Some((label("[0]"), true))));
        assert_eq!(row(&g, 0), "abcdef");
    }

    /// Tree lines from depths: `├─` with a sibling to come, `└─` for the
    /// last, `│` under an ancestor whose siblings are still to come.
    #[test]
    fn tree_lines_follow_the_depths() {
        let leads = tree_leads(&[0, 1, 2, 3, 3, 2, 3, 1, 2, 3]);
        assert_eq!(
            leads,
            ["", "├─ ", "│  ├─ ", "│  │  ├─ ", "│  │  └─ ", "│  └─ ", "│     └─ ", "└─ ", "   └─ ", "      └─ "]
        );
        // A lone node, and nothing at all.
        assert_eq!(tree_leads(&[0, 1]), ["", "└─ "]);
        assert!(tree_leads(&[]).is_empty());
        // A filter can leave a deeper line without its parents: no panic,
        // the lines still drawn.
        assert_eq!(tree_leads(&[0, 3]).len(), 2);
    }

    /// A theme's colours: the palette's first 16 everywhere, the default
    /// colours everywhere but the status line, nothing at all when the theme
    /// leaves the colours to the terminal.
    #[test]
    fn a_theme_recolors_all_but_the_status_lines_defaults() {
        let red = Style::colors(Color::Idx(1), Color::Default);
        let mut g = Grid::new(3, 2);
        g.put_str(0, 0, "ab", red, 3);
        g.put_str(0, 1, "cd", red, 3);
        let before = g.clone();
        recolor(&mut g, Some(1), Color::Default, Color::Default, &[]);
        assert_eq!(g, before, "nothing set: the terminal's own colours");
        recolor(&mut g, Some(1), Color::Default, Color::Default, &[Color::Idx(9); 15]);
        assert_eq!(g, before, "not 16 colours: no palette");
        let (fg, bg) = (Color::Rgb(1, 2, 3), Color::Rgb(9, 9, 9));
        let mut palette = vec![Color::Idx(0); 16];
        palette[1] = Color::Rgb(0xf5, 0x2a, 0x65);
        recolor(&mut g, Some(1), fg, bg, &palette);
        // A pane row: its red is the theme's, its default background too, and
        // a blank cell takes both defaults.
        assert_eq!((g.get(0, 0).style.fg, g.get(0, 0).style.bg), (palette[1], bg));
        assert_eq!((g.get(2, 0).style.fg, g.get(2, 0).style.bg), (fg, bg));
        // The status line: the palette, but its defaults stay the terminal's.
        assert_eq!((g.get(0, 1).style.fg, g.get(0, 1).style.bg), (palette[1], Color::Default));
        assert_eq!((g.get(2, 1).style.fg, g.get(2, 1).style.bg), (Color::Default, Color::Default));
        // Colours past the 16 and explicit ones are the program's.
        let mut g = Grid::new(1, 1);
        g.put_str(0, 0, "x", Style::colors(Color::Idx(200), Color::Rgb(4, 5, 6)), 1);
        recolor(&mut g, None, fg, bg, &palette);
        assert_eq!((g.get(0, 0).style.fg, g.get(0, 0).style.bg), (Color::Idx(200), Color::Rgb(4, 5, 6)));
    }

    #[test]
    fn copy_selection_inverts() {
        let a = screen(5, 2, b"abcde\r\nfghij");
        let f = Frame {
            cols: 5,
            rows: 2,
            panes: vec![PaneView {
                rect: Rect { x: 0, y: 0, w: 5, h: 2 },
                screen: a.screen(),
                active: true,
                copy: Some(CopyView { cx: 1, cy: 1, sel: Some(((0, 3), (1, 1))), rect: false, offset: 0 }),
                border_text: None,
            }],
            status: None,
            status_top: false,
            border_fg: Color::Default,
            active_border_fg: Color::Default,
        };
        let (g, cursor, _) = compose(&f);
        assert!(cursor.is_none());
        assert!(!g.get(2, 0).style.inverse);
        assert!(g.get(3, 0).style.inverse);
        assert!(g.get(4, 0).style.inverse);
        assert!(g.get(0, 1).style.inverse);
        // Cursor cell is inside the selection: double inversion cancels.
        assert!(!g.get(1, 1).style.inverse);
        assert!(!g.get(2, 1).style.inverse);
    }

    /// root; s1 (windows w1 current, w2); w1 (panes p1, p2 active); w2 (p3); s2 (w3 (p4)).
    fn chart_nodes() -> Vec<ChartNode> {
        let n = |kind: &'static str, depth: usize, title: &str, info: &str, active: bool| ChartNode {
            kind,
            depth,
            title: title.into(),
            info: info.into(),
            active,
        };
        vec![
            n("", 0, "keepane", "", false),
            n("session", 1, "1  dev", "2 windows · attached", false),
            n("window", 2, "2  0:edit*", "2 panes", true),
            n("pane", 3, "3  0:pwsh", "normal", false),
            n("pane", 3, "4  1:pwsh*", "shell · %builder", true),
            n("window", 2, "5  1:logs", "1 pane", false),
            n("pane", 3, "6  0:pwsh*", "normal", true),
            n("session", 1, "7  ops", "1 window", false),
            n("window", 2, "8  0:deploy*", "1 pane", true),
            n("pane", 3, "9  0:ping*", "normal", true),
        ]
    }

    /// The joins meet: whatever goes down (`┌` `┬` `┐`, a corner of a box
    /// too) has something coming up right under it, and every `┴` has
    /// something coming down right above it.
    fn joins_land(t: &[String]) -> Result<(), String> {
        let rows: Vec<Vec<char>> = t.iter().map(|r| r.chars().collect()).collect();
        let at = |y: usize, x: usize| rows.get(y).and_then(|r| r.get(x)).copied().unwrap_or(' ');
        for (y, row) in rows.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                if "┌┬┐".contains(c) && !"┴│┼┤├┘└".contains(at(y + 1, x)) {
                    return Err(format!("row {y} col {x}: {c} over {:?}", at(y + 1, x)));
                }
                // (Under the root's name when the root is one line.)
                if c == '┴' && (y == 0 || !("┌┬┐┼├┤│".contains(at(y - 1, x)) || at(y - 1, x).is_alphanumeric()))
                {
                    return Err(format!("row {y} col {x}: ┴ under {:?}", at(y.saturating_sub(1), x)));
                }
            }
        }
        Ok(())
    }
    fn chart_text(g: &Grid) -> Vec<String> {
        (0..g.rows)
            .map(|y| (0..g.cols).map(|x| g.get(x, y).text()).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// The branch: up to the root, then down the current children (else
    /// the first).
    #[test]
    fn the_chart_follows_the_branch_of_the_selection() {
        let nodes = chart_nodes();
        let depths: Vec<usize> = nodes.iter().map(|n| n.depth).collect();
        let active: Vec<bool> = nodes.iter().map(|n| n.active).collect();
        assert_eq!(chart_path(&depths, &active, 1), vec![0, 1, 2, 4], "a session: its current window, its active pane");
        assert_eq!(chart_path(&depths, &active, 5), vec![0, 1, 5, 6]);
        assert_eq!(chart_path(&depths, &active, 3), vec![0, 1, 2, 3], "a pane: nothing below it");
        // No current child (a filter took it away): the first.
        let none = vec![false; depths.len()];
        assert_eq!(chart_path(&depths, &none, 1), vec![0, 1, 2, 3]);
        assert_eq!(chart_path(&depths, &active, 99), Vec::<usize>::new());
    }

    /// Four rows of boxes: the root centred, the sessions, the session's
    /// windows, the window's panes, joined by lines; each box says what it
    /// is in its top border; all in the terminal's colours but the selected
    /// one, in the highlight colour; nothing filled.
    #[test]
    fn the_chart_draws_its_rows() {
        let nodes = chart_nodes();
        let mut g = Grid::new(80, 24);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 24 }, &nodes, 4, false, "Enter go", "");
        let t = chart_text(&g);
        let all = t.join("\n");
        eprintln!("{all}");
        let row = |s: &str| t.iter().position(|r| r.contains(s)).unwrap_or_else(|| panic!("no {s}: {all}"));
        // The root, centred, in a box.
        let r0 = row("keepane");
        let root = t[r0].find("keepane").unwrap();
        assert!((34..=40).contains(&root) && t[r0 - 1].contains('┌') && t[r0 + 1].contains('┬'), "{all}");
        // The sessions, dev's windows, edit's panes (not logs', not ops').
        let (s, w, p) = (row("1  dev"), row("2  0:edit*"), row("4  1:pwsh*"));
        assert!(s < w && w < p, "{all}");
        assert!(t[s].contains("7  ops") && t[w].contains("5  1:logs") && t[p].contains("3  0:pwsh"), "{all}");
        assert!(!all.contains("deploy") && !all.contains("6  0:pwsh"), "{all}");
        assert!(t[s - 1].matches("┌ session ").count() == 2 && t[p - 1].matches("┌ pane ").count() == 2, "{all}");
        assert!(t[s + 1].contains("2 windows · attached") && t[p + 1].contains("shell · %builder"), "{all}");
        // Joins: the bar under the root spans both sessions, and every join
        // lands on a box.
        joins_land(&t).map_err(|e| format!("{e}\n{all}")).unwrap();
        let bar = s - 2;
        assert!(t[bar].contains('┌') && t[bar].contains('┐') && t[bar].contains('┴'), "{all}");
        let col = |y: usize, c: char| t[y].chars().position(|x| x == c).unwrap();
        assert_eq!(t[s - 1].chars().nth(col(bar, '┌')), Some('┴'), "{all}");
        // Colours: nothing filled; the selected pane yellow and bold; the
        // others in the terminal's own colours, the branch's names bold.
        let cell = |y: usize, s: &str| g.get(t[y][..t[y].find(s).unwrap()].width() as u16, y as u16).clone();
        assert!((0..24).all(|y| (0..80).all(|x| g.get(x, y).style.bg == Color::Default || y == 23)), "nothing filled");
        let sel = cell(p, "4  1:pwsh*");
        assert!(sel.style.fg == Color::Idx(3) && sel.style.bold, "{:?}", sel.style);
        let dev = cell(s, "1  dev");
        assert!(dev.style.fg == Color::Default && dev.style.bold, "on the branch: bold");
        let ops = cell(s, "7  ops");
        assert!(ops.style.fg == Color::Default && !ops.style.bold, "off it: plain");
        // The footer says where the cursor is and how to move.
        assert!(t[23].starts_with("[5/10] hjkl move  Enter go"), "{:?}", t[23]);
    }
    /// `a`: every node at once, each parent over its own; boxes when they
    /// fit, else one line each, else numbers, scrolled to the selection.
    #[test]
    fn the_chart_opens_every_node_and_shrinks_to_fit() {
        let nodes = chart_nodes();
        let draw = |w: u16, h: u16, sel: usize| {
            let mut g = Grid::new(w, h);
            draw_chart(&mut g, Rect { x: 0, y: 0, w, h }, &nodes, sel, true, "", "");
            (chart_text(&g), g)
        };
        // Room: every box, joined.
        let (t, g) = draw(80, 24, 4);
        let all = t.join("\n");
        eprintln!("{all}");
        for name in [
            "1  dev",
            "7  ops",
            "2  0:edit*",
            "5  1:logs",
            "8  0:deploy*",
            "3  0:pwsh",
            "4  1:pwsh*",
            "6  0:pwsh*",
            "9  0:ping*",
        ] {
            assert!(all.contains(name), "{name}: {all}");
        }
        assert_eq!(all.matches("┌ pane ").count(), 4, "{all}");
        joins_land(&t).map_err(|e| format!("{e}\n{all}")).unwrap();
        // ops over its one window, over its one pane: one column.
        let col = |s: &str| {
            let y = t.iter().position(|r| r.contains(s)).unwrap();
            t[y][..t[y].find(s).unwrap()].width()
        };
        assert!(
            col("8  0:deploy*").abs_diff(col("9  0:ping*")) <= 2 && col("7  ops").abs_diff(col("8  0:deploy*")) <= 2,
            "{all}"
        );
        let x = col("4  1:pwsh*") as u16;
        let y = t.iter().position(|r| r.contains("4  1:pwsh*")).unwrap() as u16;
        assert_eq!(g.get(x, y).style.fg, Color::Idx(3), "the selection");
        // A parent sits over the middle of its children: the middle of a box
        // from its side borders on the line of its name.
        let mid = |s: &str| {
            let y = t.iter().position(|r| r.contains(s)).unwrap();
            let r: Vec<char> = t[y].chars().collect();
            let at = t[y][..t[y].find(s).unwrap()].chars().count();
            let l = (0..at).rev().find(|&i| r[i] == '│').unwrap() as i32;
            let rr = (at..r.len()).find(|&i| r[i] == '│').unwrap() as i32;
            (l + rr) / 2
        };
        let between = |p: &str, a: &str, b: &str| (mid(p) - (mid(a) + mid(b)) / 2).abs() <= 2;
        assert!(between("1  dev", "2  0:edit*", "5  1:logs"), "{all}");
        assert!(between("2  0:edit*", "3  0:pwsh", "4  1:pwsh*"), "{all}");
        // Titles too long for full boxes on 80 columns: narrower boxes, cut.
        let mut long = chart_nodes();
        for n in long.iter_mut().filter(|n| n.kind == "pane") {
            n.title = format!("{}  0:管理员: C:\\WINDOWS\\system32\\cmd.exe", chart_number(&n.title));
        }
        let mut g = Grid::new(80, 24);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 24 }, &long, 4, true, "", "");
        let t = chart_text(&g);
        let all = t.join("\n");
        eprintln!("{all}");
        assert_eq!(all.matches("┌ pane").count(), 4, "still boxes: {all}");
        assert!(t.iter().any(|r| r.contains("3  0:") && r.contains('…')), "{all}");
        // Narrower: one line each, still joined.
        let (t, _) = draw(56, 24, 4);
        let all = t.join("\n");
        eprintln!("{all}");
        assert!(!all.contains("┌ pane") && all.contains("[4  1:pwsh*]") && all.contains("[9  0:ping*]"), "{all}");
        assert!(all.contains('┴') || all.contains('┬') || all.contains('┌'), "joined: {all}");
        // Narrower still: numbers; too narrow even so: scrolled, the
        // selection in view, ‹ › at the edges.
        let (t, _) = draw(30, 24, 4);
        let all = t.join("\n");
        eprintln!("{all}");
        assert!(all.contains("[4]") && all.contains("[9]") && !all.contains("pwsh"), "{all}");
        let mut many =
            vec![ChartNode { kind: "", depth: 0, title: "keepane".into(), info: String::new(), active: false }];
        many.push(ChartNode { kind: "session", depth: 1, title: "1  s".into(), info: String::new(), active: false });
        for k in 0..30 {
            many.push(ChartNode {
                kind: "window",
                depth: 2,
                title: format!("{}  {k}:w", k + 2),
                info: String::new(),
                active: false,
            });
        }
        let mut g = Grid::new(40, 12);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 40, h: 12 }, &many, 25, true, "", "");
        let t = chart_text(&g);
        let all = t.join("\n");
        eprintln!("{all}");
        let row = t.iter().find(|r| r.contains("[25]")).unwrap_or_else(|| panic!("the selection in view: {all}"));
        assert!(row.starts_with('‹') && row.ends_with('›'), "{row:?}");
        // Any size: no panic.
        for (w, h) in [(1u16, 1u16), (2, 2), (5, 30), (200, 3), (12, 5)] {
            let mut g = Grid::new(w, h);
            draw_chart(&mut g, Rect { x: 0, y: 0, w, h }, &nodes, 4, true, "", "");
            draw_chart(&mut g, Rect { x: 0, y: 0, w, h }, &many, 31, true, "", "");
        }
    }

    /// Paired with other machines: a row of them under the root (`host`),
    /// and below, the sessions of the machine on the branch only.
    #[test]
    fn the_chart_has_a_row_of_machines() {
        let n = |kind: &'static str, depth: usize, title: &str, active: bool| ChartNode {
            kind,
            depth,
            title: title.into(),
            info: String::new(),
            active,
        };
        let nodes = vec![
            n("", 0, "keepane", false),
            n("host", 1, "1  DFINE", true),
            n("session", 2, "2  dev", false),
            n("window", 3, "3  0:edit*", true),
            n("pane", 4, "4  0:pwsh*", true),
            n("host", 1, "5  100.64.0.3:7681", false),
            n("session", 2, "6  work", false),
            n("window", 3, "7  0:vim", false),
            n("pane", 4, "8  0:vim*", true),
            n("window", 3, "9  1:logs*", true),
            n("pane", 4, "10  0:tail*", true),
        ];
        let mut g = Grid::new(80, 24);
        // On the other machine's session: its current window (1:logs).
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 24 }, &nodes, 6, false, "", "");
        let t = chart_text(&g);
        let all = t.join("\n");
        eprintln!("{all}");
        let hosts = t.iter().position(|r| r.contains("1  DFINE")).expect("the machines");
        assert_eq!(t[hosts - 1].matches("┌ host ").count(), 2, "{all}");
        assert!(t[hosts].contains("5  100.64.0.3:7681"), "{all}");
        assert!(
            all.contains("6  work") && !all.contains("2  dev"),
            "this machine's sessions are not on the branch: {all}"
        );
        assert!(all.contains("9  1:logs*") && all.contains("10  0:tail*") && !all.contains("8  0:vim*"), "{all}");
        let at = |y: usize, s: &str| t[y][..t[y].find(s).unwrap()].width() as u16;
        let other = g.get(at(hosts, "5  100.64"), hosts as u16).style;
        assert!(other.bold && other.fg == Color::Default, "the machine on the branch: bold, in the terminal's colour");
        // An 80x24 terminal leaves the chart 22 rows: the root gives up its
        // box, the joins stay (one line each).
        let mut g = Grid::new(80, 23);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 23 }, &nodes, 6, false, "", "");
        let t = chart_text(&g);
        let all = t.join("\n");
        assert!(t[0].contains("keepane") && !t[0].contains('┌'), "{all}");
        assert!(t[1].contains('┌') && t[1].contains('┴'), "the bar under the root: {all}");
        assert!(all.contains("10  0:tail*"), "{all}");
        joins_land(&t).map_err(|e| format!("{e}\n{all}")).unwrap();

        // Up and down in the middle of a tall screen; a branch shorter than
        // the tree (a machine with no sessions) keeps the same place.
        let mut nodes = nodes;
        nodes.push(n("host", 1, "11  10.0.0.9:7681", false));
        let at = |sel: usize| {
            let mut g = Grid::new(100, 60);
            draw_chart(&mut g, Rect { x: 0, y: 0, w: 100, h: 60 }, &nodes, sel, false, "", "");
            chart_text(&g)
        };
        let (deep, short) = (at(4), at(11));
        let used: Vec<usize> = (0..59).filter(|&y| !deep[y].is_empty()).collect();
        let (first, last) = (used[0], *used.last().unwrap());
        assert!(first.abs_diff(58 - last) <= 1, "{first} blank above, {} below: {}", 58 - last, deep.join("\n"));
        let root = |t: &[String]| t.iter().position(|r| r.contains("keepane")).unwrap();
        assert_eq!(root(&deep), root(&short), "{}\n----\n{}", deep.join("\n"), short.join("\n"));
        assert!(!short.join("\n").contains("┌ session"), "the bare machine's branch stops at it");
        // Too tall for the screen: from the top, as before.
        let mut g = Grid::new(80, 16);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 16 }, &nodes, 4, false, "", "");
        assert!(chart_text(&g)[0].contains("keepane"), "{}", chart_text(&g).join("\n"));
    }

    /// A row wider than the screen scrolls to keep the branch's block in
    /// view, with `‹` `›` for what is past the edges; short screens drop
    /// the join lines; nothing panics at any size.
    #[test]
    fn the_chart_scrolls_and_shrinks() {
        let node = |kind: &'static str, depth: usize, title: String, info: &str| ChartNode {
            kind,
            depth,
            title,
            info: info.into(),
            active: false,
        };
        let mut nodes = vec![node("", 0, "keepane".into(), ""), node("session", 1, "1  s".into(), "30 windows")];
        for k in 0..30 {
            nodes.push(node("window", 2, format!("{}  {k}:window-{k}", k + 2), "1 pane"));
        }
        let mut g = Grid::new(60, 14);
        // The 21st window selected: it is on screen, with more either side.
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 60, h: 14 }, &nodes, 22, false, "", "");
        let t = chart_text(&g);
        eprintln!("{}", t.join("\n"));
        let y = t.iter().position(|r| r.contains("window-20")).expect("the selected window is drawn");
        assert!(t[y].starts_with('‹') && t[y].ends_with('›'), "{:?}", t[y]);
        let x = t[y][..t[y].find("22  20").unwrap()].width() as u16;
        assert_eq!(g.get(x, y as u16).style.fg, Color::Idx(3), "the selection is the one in view");
        // The first window: no ‹, it starts at the left edge.
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 60, h: 14 }, &nodes, 2, false, "", "");
        let t = chart_text(&g);
        let row = t.iter().find(|r| r.contains("0:window-0")).unwrap();
        assert!(!row.starts_with('‹') && row.ends_with('›'), "{row:?}");
        // Short: one join line, then none; the selection's row still drawn.
        let nodes = chart_nodes();
        for (h, joins) in [(19u16, 1usize), (16, 0)] {
            let mut g = Grid::new(80, h);
            draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h }, &nodes, 4, false, "", "");
            let t = chart_text(&g);
            // The root's box (3), then three rows of joins and a box (4):
            // the panes' names on the second line of the last.
            let pane_row = 3 + 2 * (joins + 4) + joins + 1;
            assert!(t[pane_row].contains("4  1:pwsh*"), "h {h}: {}", t.join("\n"));
            assert_eq!(
                t.iter().filter(|r| r.contains('┌') && !r.contains("┌ ") && r.contains('┐')).count(),
                1 + joins.min(1) * 3,
                "h {h}: {}",
                t.join("\n")
            );
        }
        // Degenerate sizes, wide names: no panic.
        let mut nodes = chart_nodes();
        nodes[1].title = "1  会话会话会话会话会话会话会话会话会话会话".into();
        for (w, h) in [(1u16, 1u16), (2, 2), (20, 3), (5, 30), (200, 2)] {
            let mut g = Grid::new(w, h);
            draw_chart(&mut g, Rect { x: 0, y: 0, w, h }, &nodes, 1, false, "Enter go", "[1 tagged]");
        }
        // A block longer than the widest a block gets is cut with `…`.
        let mut g = Grid::new(80, 24);
        draw_chart(&mut g, Rect { x: 0, y: 0, w: 80, h: 24 }, &nodes, 1, false, "", "");
        assert!(chart_text(&g).iter().any(|r| r.contains("1  会") && r.contains('…')), "{}", chart_text(&g).join("\n"));
    }
}
