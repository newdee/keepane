// A pane's lines (`capture-pane -e`: text with SGR colour sequences) as
// HTML, in the terminal theme's colours.

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

export function ansiToHtml(text: string, palette: string[]) {
  text = text.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "").replace(/\x1b\[[0-9;?]*[A-Za-ln-z]/g, "");
  const st = { fg: null as string | null, bg: null as string | null, b: false, dim: false, i: false, u: false, inv: false };
  let out = "";
  let last = 0;
  let m: RegExpExecArray | null;
  const span = (s: string) => {
    if (!s) return;
    let fg = st.fg;
    let bg = st.bg;
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
      if (n === 0) Object.assign(st, { fg: null, bg: null, b: false, dim: false, i: false, u: false, inv: false });
      else if (n === 1) st.b = true;
      else if (n === 2) st.dim = true;
      else if (n === 3) st.i = true;
      else if (n === 4) st.u = true;
      else if (n === 7) st.inv = true;
      else if (n === 22) st.b = st.dim = false;
      else if (n === 23) st.i = false;
      else if (n === 24) st.u = false;
      else if (n === 27) st.inv = false;
      else if (n >= 30 && n <= 37) st.fg = palette[n - 30];
      else if (n === 39) st.fg = null;
      else if (n >= 40 && n <= 47) st.bg = palette[n - 40];
      else if (n === 49) st.bg = null;
      else if (n >= 90 && n <= 97) st.fg = palette[n - 82];
      else if (n >= 100 && n <= 107) st.bg = palette[n - 92];
      else if (n === 38 || n === 48) {
        let col: string | null = null;
        if (c[i + 1] === 5) {
          col = colour256(c[i + 2] || 0, palette);
          i += 2;
        } else if (c[i + 1] === 2) {
          col = `rgb(${c[i + 2] || 0},${c[i + 3] || 0},${c[i + 4] || 0})`;
          i += 4;
        }
        if (n === 38) st.fg = col;
        else st.bg = col;
      }
    }
  }
  span(text.slice(last));
  return out;
}

/** A line with nothing on it once its colours are taken out. */
export const blank = (l: string) => !l.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trim();
