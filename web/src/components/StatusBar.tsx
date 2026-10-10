import { useEffect, useRef, type CSSProperties } from "react";
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
 * the clock) is never pushed off. A window's label opens it.
 */
export function StatusBar({ line, theme, onOpen }: { line: StatusLine; theme: Theme; onOpen: (pane: string) => void }) {
  const fg = css(line.fg, theme.palette, theme.fg);
  const bg = css(line.bg, theme.palette, theme.bg);
  const list = useRef<HTMLDivElement>(null);
  const current = line.windows.find((w) => w.current)?.index;
  // The current window in view, when it changes.
  useEffect(() => {
    const el = list.current?.querySelector<HTMLElement>("[aria-current]");
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [current]);
  return (
    <div
      id="status-line"
      role="toolbar"
      aria-label={t(`Status line of ${line.session}`, `${line.session} 的状态栏`)}
      className="font-term flex h-7 shrink-0 items-stretch overflow-hidden text-[13px] leading-7 whitespace-pre select-none pointer-coarse:h-10 pointer-coarse:leading-10"
      style={{ color: fg, background: bg }}
    >
      <div className="shrink-0">
        {line.left.map((s, i) => (
          <Run key={i} s={s} theme={theme} fg={fg} bg={bg} />
        ))}
      </div>
      <div ref={list} className="no-scrollbar flex min-w-0 flex-1 overflow-x-auto">
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
      {/* Too wide for the room: cut from its left, so its end (the
          machine, the clock) always shows. */}
      <div className="flex max-w-[60%] shrink-0 justify-end overflow-hidden">
        <span className="shrink-0">
          {line.right.map((s, i) => (
            <Run key={i} s={s} theme={theme} fg={fg} bg={bg} />
          ))}
        </span>
      </div>
    </div>
  );
}
