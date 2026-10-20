// A pane's lines (`capture-pane -e`: text with SGR colour sequences) as
// HTML, in the terminal theme's colours.
import type { Theme } from "./api";

export function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/** Colour `n` of the 256: the theme's first 16, then xterm's cube and greys. */
export function colour256(n: number, palette: string[]) {
  if (n < 16) return palette[n];
  if (n >= 232) {
    const v = 8 + 10 * (n - 232);
    return `rgb(${v},${v},${v})`;
  }
  n -= 16;
  const s = [0, 95, 135, 175, 215, 255];
  return `rgb(${s[Math.floor(n / 36)]},${s[Math.floor(n / 6) % 6]},${s[n % 6]})`;
}

/** A CSS colour as RGB (`#rgb`, `#rrggbb`, `rgb(r,g,b)`); null for anything else. */
function rgbOf(c: string): number[] | null {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(c)?.[1];
  if (hex) {
    const h = hex.length === 3 ? [...hex].map((d) => d + d).join("") : hex;
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
  }
  const m = /^rgb\((\d+),(\d+),(\d+)\)$/.exec(c);
  return m ? m.slice(1).map(Number) : null;
}

/** WCAG's contrast between two colours, 1 to 21. */
function contrast(a: number[], b: number[]) {
  const lum = (c: number[]) => {
    const [r, g, bl] = c.map((v) => {
      v /= 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Text `fg` on `bg` as it is when it reads (3:1 or more, or either colour
 *  not known as RGB), else the one of `choices` that reads best on `bg`,
 *  when that reads better (the server's `legible`, src/server/render.rs). */
function legible(fg: string, bg: string, choices: string[]) {
  const [f, b] = [rgbOf(fg), rgbOf(bg)];
  if (!f || !b) return fg;
  let best = fg;
  let most = contrast(f, b);
  if (most >= 3) return fg;
  for (const c of choices) {
    const rgb = rgbOf(c);
    const r = rgb ? contrast(rgb, b) : 0;
    if (r > most) [best, most] = [c, r];
  }
  return best;
}

export function ansiToHtml(text: string, theme: Pick<Theme, "fg" | "bg" | "palette">) {
  const palette = theme.palette;
  text = text.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "").replace(/\x1b\[[0-9;?]*[A-Za-ln-z]/g, "");
  // `own`: the text's colour is one the program chose itself (past the 16).
  const st = { fg: null as string | null, own: false, bg: null as string | null, b: false, dim: false, i: false, u: false, inv: false };
  let out = "";
  let last = 0;
  let m: RegExpExecArray | null;
  const span = (s: string) => {
    if (!s) return;
    let fg = st.fg;
    let bg = st.bg;
    // Text whose colour the theme gave (the default, one of the 16) on a
    // background the program painted itself: a program that took the
    // terminal for a dark one paints its own dark box and leaves its text
    // "default" or "white", which a light theme draws as dark ink. Under
    // 3:1 it takes the theme's text or paper, whichever reads, as a pane
    // on the computer does.
    if (bg && !st.inv && !st.own) {
      const ink = fg || theme.fg;
      const read = legible(ink, bg, [theme.fg, theme.bg]);
      if (read !== ink) fg = read;
    }
    if (st.inv) [fg, bg] = [bg || "var(--term-bg)", fg || "var(--term-fg)"];
    let css = "";
    if (fg) css += `color:${fg};`;
    if (bg) css += `background:${bg};`;
    if (st.b) css += "font-weight:700;";
    if (st.dim) css += "opacity:.65;";
    if (st.i) css += "font-style:italic;";
    if (st.u) css += "text-decoration:underline;";
    out += css ? `<span style="${css}">${esc(s)}</span>` : esc(s);
  };
  const re = /\x1b\[([0-9;]*)m/g;
  while ((m = re.exec(text))) {
    span(text.slice(last, m.index));
    last = re.lastIndex;
    const c = m[1] === "" ? [0] : m[1].split(";").map(Number);
    for (let i = 0; i < c.length; i++) {
      const n = c[i];
      if (n === 0) Object.assign(st, { fg: null, own: false, bg: null, b: false, dim: false, i: false, u: false, inv: false });
      else if (n === 1) st.b = true;
      else if (n === 2) st.dim = true;
      else if (n === 3) st.i = true;
      else if (n === 4) st.u = true;
      else if (n === 7) st.inv = true;
      else if (n === 22) st.b = st.dim = false;
      else if (n === 23) st.i = false;
      else if (n === 24) st.u = false;
      else if (n === 27) st.inv = false;
      else if (n >= 30 && n <= 37) [st.fg, st.own] = [palette[n - 30], false];
      else if (n === 39) [st.fg, st.own] = [null, false];
      else if (n >= 40 && n <= 47) st.bg = palette[n - 40];
      else if (n === 49) st.bg = null;
      else if (n >= 90 && n <= 97) [st.fg, st.own] = [palette[n - 82], false];
      else if (n >= 100 && n <= 107) st.bg = palette[n - 92];
      else if (n === 38 || n === 48) {
        let col: string | null = null;
        let own = false;
        if (c[i + 1] === 5) {
          col = colour256(c[i + 2] || 0, palette);
          own = (c[i + 2] || 0) >= 16;
          i += 2;
        } else if (c[i + 1] === 2) {
          col = `rgb(${c[i + 2] || 0},${c[i + 3] || 0},${c[i + 4] || 0})`;
          own = true;
          i += 4;
        }
        if (n === 38) [st.fg, st.own] = [col, own];
        else st.bg = col;
      }
    }
  }
  span(text.slice(last));
  return out;
}

/** A line with nothing on it once its colours are taken out. */
export const blank = (l: string) => !l.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trim();
