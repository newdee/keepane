import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { colour256 } from "../ansi";
import type { Colour, Seg, StatusLine, Theme } from "../api";
import { t } from "../i18n";

/** A colour of the status line in CSS, in the terminal theme's palette;
 *  null (the terminal's own) as `fallback`. */
function css(c: Colour, palette: string[], fallback: string): string {
  if (c === null) return fallback;
  return typeof c === "number" ? colour256(c, palette) : c;
}

function Run({ s, theme, fg, bg }: { s: Seg; theme: Theme; fg: string; bg: string }) {
  let [f, b] = [css(s.fg, theme.palette, fg), css(s.bg, theme.palette, bg)];
  if (s.inverse) [f, b] = [b, f];
  const style: CSSProperties = {
    color: f,
    background: b,
    fontWeight: s.bold ? 700 : undefined,
    opacity: s.dim ? 0.65 : undefined,
    fontStyle: s.italic ? "italic" : undefined,
    textDecoration: s.underline ? "underline" : undefined,
  };
  return <span style={style}>{s.text}</span>;
}

/**
 * The session's status line, as the terminal draws it (`status-left`, the
 * windows, `status-right`, in its colours), under the screen. The two ends
 * stay put; the windows between them scroll sideways when there are more
 * than fit, the current one kept in view, so the right end (the machine,
 * the clock) is never pushed off. The windows take what they need, up to
 * 40% of the bar, the right end the rest; a part cut short shows it (the
 * windows fade at a hidden side, the right end starts with …). A window's
 * label opens it.
 */
export function StatusBar({ line, theme, onOpen }: { line: StatusLine; theme: Theme; onOpen: (pane: string) => void }) {
  const fg = css(line.fg, theme.palette, theme.fg);
  const bg = css(line.bg, theme.palette, theme.bg);
  const bar = useRef<HTMLDivElement>(null);
  const left = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const text = useRef<HTMLSpanElement>(null);
  const current = line.windows.find((w) => w.current)?.index;
  // The room: the windows keep what they need, up to 40% of the bar; the
  // right end has the rest, and only when it is wider than that is it cut.
  // What is cut shows as cut, so it never runs into the next part: windows
  // hidden before or after those in view, the right end's start.
  const [room, setRoom] = useState<number | undefined>(undefined);
  const [cut, setCut] = useState(false);
  const [more, setMore] = useState({ start: false, end: false });
  const measure = useCallback(() => {
    const [b, lf, l, tx] = [bar.current, left.current, list.current, text.current];
    if (!b || !lf || !l || !tx) return;
    const windows = [...l.children].reduce((w, c) => w + (c as HTMLElement).offsetWidth, 0);
    const r = Math.floor(Math.max(0, b.clientWidth - lf.offsetWidth - Math.min(windows, b.clientWidth * 0.4)));
    setRoom(r);
    // The text's own width (it never shrinks) against that room.
    setCut(tx.offsetWidth > r + 1);
    const start = l.scrollLeft > 1;
    const end = l.scrollLeft + l.clientWidth < l.scrollWidth - 1;
    setMore((m) => (m.start === start && m.end === end ? m : { start, end }));
  }, []);
  // The current window in view, when it changes.
  useEffect(() => {
    const el = list.current?.querySelector<HTMLElement>("[aria-current]");
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
    measure();
  }, [current, measure]);
  useLayoutEffect(measure, [line, measure]);
  useEffect(() => {
    const o = new ResizeObserver(measure);
    for (const el of [bar.current, list.current]) if (el) o.observe(el);
    return () => o.disconnect();
  }, [measure]);
  const mask = `linear-gradient(to right, ${more.start ? "transparent, black 1.5em" : "black"}, ${
    more.end ? "black calc(100% - 1.5em), transparent" : "black"
  })`;
  return (
    <div
      ref={bar}
      id="status-line"
      role="toolbar"
      aria-label={t(`Status line of ${line.session}`, `${line.session} 的状态栏`)}
      className="font-term flex h-7 shrink-0 items-stretch overflow-hidden text-[13px] leading-7 whitespace-pre select-none pointer-coarse:h-10 pointer-coarse:leading-10"
      style={{ color: fg, background: bg }}
    >
      <div ref={left} className="shrink-0">
        {line.left.map((s, i) => (
          <Run key={i} s={s} theme={theme} fg={fg} bg={bg} />
        ))}
      </div>
      <div
        ref={list}
        data-more={[more.start && "start", more.end && "end"].filter(Boolean).join(" ") || undefined}
        className="no-scrollbar flex min-w-0 flex-1 overflow-x-auto"
        style={more.start || more.end ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
        onScroll={measure}
      >
        {line.windows.map((w, i) => (
          <span key={w.index} className="flex shrink-0">
            {i > 0 && line.separator ? <span>{line.separator}</span> : null}
            <button
              type="button"
              data-window={w.index}
              aria-current={w.current ? "true" : undefined}
              title={t(`Window ${w.index}`, `窗口 ${w.index}`)}
              className="cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-focus focus-visible:ring-inset"
              onClick={() => w.pane && onOpen(w.pane)}
            >
              {w.segments.map((s, j) => (
                <Run key={j} s={s} theme={theme} fg={fg} bg={bg} />
              ))}
            </button>
          </span>
        ))}
      </div>
      {/* Too wide for its room: cut from its left, so its end (the
          machine, the clock) always shows, the cut marked over its start. */}
      <div className="relative flex shrink-0 justify-end overflow-hidden" style={{ maxWidth: room }}>
        {cut ? (
          <span data-cut className="absolute inset-y-0 left-0 z-10" style={{ color: fg, background: bg }}>
            …
          </span>
        ) : null}
        <span ref={text} className="shrink-0">
          {line.right.map((s, i) => (
            <Run key={i} s={s} theme={theme} fg={fg} bg={bg} />
          ))}
        </span>
      </div>
    </div>
  );
}
