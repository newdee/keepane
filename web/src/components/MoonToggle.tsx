import { useEffect, useRef } from "react";
import { t } from "../i18n";

/* The page's light and dark, as a switch: a glass knob slides across, and
   the specks of light inside it are flung out and gather again, into a
   crescent moon for dark and a sun for light, while a highlight runs round
   the edge. The look is in styles.css (`.mt`). The specks move only while
   they travel or the pointer is over the switch; with reduced motion they
   are simply where they belong. */

type Pt = { x: number; y: number };
type Speck = {
  x: number;
  y: number;
  vx: number;
  vy: number;
  tx: number;
  ty: number;
  wake: number;
  size: number;
  hue: number;
  rgb: number[];
  tw: number;
};

const COUNT = 36;
const WARM = [
  [255, 214, 128],
  [255, 178, 80],
  [255, 241, 205],
];
const COOL = [
  [232, 240, 255],
  [180, 200, 255],
  [255, 255, 255],
];

/** Points of a shape drawn off screen, in units of the knob's radius
 *  (-1..1), in a fixed shuffle: the same picture each time, and each switch
 *  sends the specks across. */
function sample(draw: (g: CanvasRenderingContext2D, n: number) => void): Pt[] {
  const n = 96;
  const c = document.createElement("canvas");
  c.width = c.height = n;
  const g = c.getContext("2d");
  if (!g) return Array.from({ length: COUNT }, () => ({ x: 0, y: 0 }));
  g.fillStyle = "#fff";
  draw(g, n);
  const px = g.getImageData(0, 0, n, n).data;
  const pts: Pt[] = [];
  for (let y = 0; y < n; y += 2)
    for (let x = 0; x < n; x += 2) if (px[(y * n + x) * 4 + 3] > 140) pts.push({ x: (x / n) * 2 - 1, y: (y / n) * 2 - 1 });
  let seed = 7;
  const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  for (let i = pts.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [pts[i], pts[j]] = [pts[j], pts[i]];
  }
  return Array.from({ length: COUNT }, (_, i) => pts[i % pts.length] ?? { x: 0, y: 0 });
}

function moon(g: CanvasRenderingContext2D, n: number) {
  g.beginPath();
  g.arc(n * 0.5, n * 0.5, n * 0.3, 0, Math.PI * 2);
  g.fill();
  g.globalCompositeOperation = "destination-out";
  g.beginPath();
  g.arc(n * 0.63, n * 0.4, n * 0.25, 0, Math.PI * 2);
  g.fill();
  g.globalCompositeOperation = "source-over";
}

function sun(g: CanvasRenderingContext2D, n: number) {
  g.beginPath();
  g.arc(n * 0.5, n * 0.5, n * 0.17, 0, Math.PI * 2);
  g.fill();
  for (let i = 0; i < 8; i++) {
    g.save();
    g.translate(n / 2, n / 2);
    g.rotate((i / 8) * Math.PI * 2);
    g.fillRect(n * 0.26, -n * 0.022, n * 0.08, n * 0.044);
    g.restore();
  }
}

const calm = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The specks of one knob, and the loop that moves them while they travel. */
class Field {
  g: CanvasRenderingContext2D | null;
  w = 1;
  dark = false;
  hover = false;
  started = 0;
  raf = 0;
  last = 0;
  ps: Speck[];
  shapes: { light: Pt[]; dark: Pt[] };

  constructor(private canvas: HTMLCanvasElement) {
    this.g = canvas.getContext("2d");
    this.shapes = { light: sample(sun), dark: sample(moon) };
    this.ps = Array.from({ length: COUNT }, (_, i) => ({
      x: 0,
      y: 0,
      vx: 0,
      vy: 0,
      tx: 0,
      ty: 0,
      wake: 0,
      size: 0.6 + (((i * 37) % 100) / 100) * 0.9,
      hue: i % 3,
      rgb: [255, 220, 150],
      tw: (i * 0.618) % 1,
    }));
    this.size();
  }

  size() {
    const r = this.canvas.getBoundingClientRect();
    const d = Math.min(devicePixelRatio || 1, 2);
    this.w = Math.max(1, Math.round(r.width * d));
    this.canvas.width = this.canvas.height = this.w;
  }

  set(dark: boolean, animate: boolean) {
    this.dark = dark;
    const pts = this.shapes[dark ? "dark" : "light"];
    const now = performance.now();
    this.ps.forEach((p, i) => {
      p.tx = pts[i].x;
      p.ty = pts[i].y;
      if (!animate) {
        p.x = p.tx;
        p.y = p.ty;
        p.vx = p.vy = 0;
        p.rgb = (dark ? COOL : WARM)[p.hue].slice();
        return;
      }
      // Flung out and round, then each pulled in a little after the others.
      const a = Math.random() * Math.PI * 2;
      const s = 2.2 + Math.random() * 2.6;
      const turn = dark ? 1 : -1;
      p.vx += Math.cos(a) * s - p.y * 3.2 * turn;
      p.vy += Math.sin(a) * s + p.x * 3.2 * turn;
      p.wake = now + Math.random() * 140;
    });
    this.started = now;
    if (animate) this.wake();
    else this.draw(now);
  }

  wake() {
    if (this.raf) return;
    this.last = performance.now();
    this.raf = requestAnimationFrame(this.frame);
  }

  stop() {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
  }

  frame = (now: number) => {
    const dt = Math.min(0.033, (now - this.last) / 1000);
    this.last = now;
    const moving = this.step(dt, now);
    this.draw(now);
    this.raf = moving || this.hover || now - this.started < 700 ? requestAnimationFrame(this.frame) : 0;
  };

  step(dt: number, now: number) {
    let moving = false;
    const want = this.dark ? COOL : WARM;
    for (const p of this.ps) {
      const pull = now >= p.wake;
      const k = pull ? 150 : 0;
      const c = pull ? 13 : 2.5;
      p.vx += ((p.tx - p.x) * k - p.vx * c) * dt;
      p.vy += ((p.ty - p.y) * k - p.vy * c) * dt;
      p.x += p.vx * dt;
      p.y += p.vy * dt;
      for (let j = 0; j < 3; j++) p.rgb[j] += (want[p.hue][j] - p.rgb[j]) * Math.min(1, dt * 5);
      if (Math.abs(p.vx) + Math.abs(p.vy) > 0.02 || Math.abs(p.tx - p.x) + Math.abs(p.ty - p.y) > 0.004) moving = true;
    }
    return moving;
  }

  draw(now: number) {
    const { g, w } = this;
    if (!g) return;
    g.clearRect(0, 0, w, w);
    let spread = 0;
    for (const p of this.ps) spread += Math.abs(p.tx - p.x) + Math.abs(p.ty - p.y);
    // The glow behind the shape grows as the specks gather.
    const gathered = Math.max(0, 1 - (spread / this.ps.length) * 3);
    const glow = g.createRadialGradient(w / 2, w / 2, 0, w / 2, w / 2, w * 0.5);
    const gc = this.dark ? "190,210,255" : "255,200,110";
    glow.addColorStop(0, `rgba(${gc},${0.22 * gathered})`);
    glow.addColorStop(1, `rgba(${gc},0)`);
    g.fillStyle = glow;
    g.fillRect(0, 0, w, w);
    g.globalCompositeOperation = "lighter";
    const r0 = Math.max(1, w / 40);
    const shimmer = this.hover && !calm();
    const half = w / 2;
    for (const p of this.ps) {
      let x = p.x;
      let y = p.y;
      if (shimmer) {
        x += Math.sin(now / 380 + p.tw * 9) * 0.02;
        y += Math.cos(now / 430 + p.tw * 7) * 0.02;
      }
      const px = half + x * half * 0.92;
      const py = half + y * half * 0.92;
      const a = shimmer ? 0.5 + 0.3 * Math.sin(now / 260 + p.tw * 12) : 0.75;
      const r = r0 * (0.5 + p.size * 0.55);
      const [cr, cg, cb] = p.rgb.map(Math.round);
      const dot = g.createRadialGradient(px, py, 0, px, py, r * 2.2);
      dot.addColorStop(0, `rgba(${cr},${cg},${cb},${a})`);
      dot.addColorStop(0.35, `rgba(${cr},${cg},${cb},${a * 0.35})`);
      dot.addColorStop(1, `rgba(${cr},${cg},${cb},0)`);
      g.fillStyle = dot;
      g.fillRect(px - r * 2.2, py - r * 2.2, r * 4.4, r * 4.4);
    }
    g.globalCompositeOperation = "source-over";
  }
}

export function MoonToggle({ dark, onToggle }: { dark: boolean; onToggle: () => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const field = useRef<Field | null>(null);

  // Made once; the first state is drawn as it is, every later one travels.
  useEffect(() => {
    if (!canvas.current) return;
    const f = new Field(canvas.current);
    field.current = f;
    f.set(dark, false);
    const resize = () => {
      f.size();
      f.draw(performance.now());
    };
    addEventListener("resize", resize);
    return () => {
      removeEventListener("resize", resize);
      f.stop();
      field.current = null;
    };
    // Once: `dark` later is the effect below's.
  }, []);

  useEffect(() => {
    const f = field.current;
    if (!f || f.dark === dark) return;
    f.set(dark, !calm());
    const b = button.current;
    if (b && !calm()) {
      b.classList.remove("moving", "flash");
      void b.offsetWidth;
      b.classList.add("moving", "flash");
      const done = setTimeout(() => b.classList.remove("moving", "flash"), 760);
      return () => clearTimeout(done);
    }
  }, [dark]);

  const label = t("Dark mode", "夜间模式");
  return (
    <button
      ref={button}
      id="page-mode"
      type="button"
      role="switch"
      aria-checked={dark}
      aria-label={label}
      title={dark ? t("Dark: tap for light", "夜间：点一下换日间") : t("Light: tap for dark", "日间：点一下换夜间")}
      className="mt"
      onClick={onToggle}
      onPointerEnter={() => {
        const f = field.current;
        if (!f) return;
        f.hover = true;
        if (!calm()) f.wake();
      }}
      onPointerLeave={() => {
        if (field.current) field.current.hover = false;
      }}
    >
      <span className="mt-body">
        <span className="mt-track">
          <span className="mt-scene day" />
          <span className="mt-scene night" />
        </span>
        <span className="mt-rim" />
        <span className="mt-knob">
          <canvas ref={canvas} />
          <span className="mt-gloss" />
        </span>
      </span>
    </button>
  );
}
