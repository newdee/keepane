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
        format!("[{} of {} lines] press any key", body_h, lines.len())
    } else {
        "press any key".to_string()
    };
    let hint_style = Style::colors(Color::Idx(0), Color::Idx(3));
    g.fill(Rect { x: area.x, y: area.y + area.h - 1, w: area.w, h: 1 }, hint_style);
    g.put_str(area.x, area.y + area.h - 1, &hint, hint_style, area.w);
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
    draw_chooser_footer(g, area, rows.len(), sel, "↑↓ parent/child  ←→ same level  j/k line", actions, status);
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
}
