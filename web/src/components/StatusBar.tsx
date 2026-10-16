import { Button, Drawer } from "@heroui/react";
import { useCallback, useEffect, useLayoutEffect, useRef, type CSSProperties } from "react";
import { colour256 } from "../ansi";
import type { Colour, Seg, StatusLine, Theme } from "../api";
import { t } from "../i18n";
import { useFocusInside } from "./Sheets";

/** A colour of the status line in CSS, in the terminal theme's palette;
 *  null (the terminal's own) as `fallback`. */
function css(c: Colour, palette: string[], fallback: string): string {
  if (c === null) return fallback;
  return typeof c === "number" ? colour256(c, palette) : c;
}

function Run({ s, theme, fg, bg, className }: { s: Seg; theme: Theme; fg: string; bg: string; className?: string }) {
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
  return (
    <span className={className} style={style}>
      {s.text}
    </span>
  );
}

/** All the line says, end to end: the session's part, the windows (with
 *  the separator between them), the right end. */
const whole = (line: StatusLine) =>
  [...line.left, ...line.windows.flatMap((w, i) => [...(i && line.separator ? [{ text: line.separator }] : []), ...w.segments]), ...line.right]
    .map((s) => s.text)
    .join("");

/**
 * Whether the whole status line fits the width it would have, told as it
 * changes (the line, the width): a row of no height, as wide as the bar,
 * holding the line unseen in the bar's type, measured against it.
 */
export function StatusFit({ line, onFit }: { line: StatusLine; onFit: (fits: boolean) => void }) {
  const row = useRef<HTMLDivElement>(null);
  const text = useRef<HTMLSpanElement>(null);
  const told = useRef<boolean | null>(null);
  const measure = useCallback(() => {
    const [r, tx] = [row.current, text.current];
    if (!r || !tx || !r.clientWidth) return;
    const fits = tx.offsetWidth <= r.clientWidth;
    if (told.current !== fits) {
      told.current = fits;
      onFit(fits);
    }
  }, [onFit]);
  useLayoutEffect(measure, [line, measure]);
  useEffect(() => {
    const o = new ResizeObserver(measure);
    if (row.current) o.observe(row.current);
    return () => o.disconnect();
  }, [measure]);
  return (
    <div ref={row} aria-hidden className="relative h-0 w-full shrink-0 overflow-hidden">
      <span ref={text} className="font-term invisible absolute top-0 left-0 text-[13px] whitespace-pre">
        {whole(line)}
      </span>
    </div>
  );
}

/**
 * The status line where the bottom is too narrow for it (a phone held
 * upright): from the right, nothing of it cut. The session; its windows, a
 * row each, the current one marked (a tap opens it); then what the line's
 * ends say, each part as it is drawn there, in the line's own colours,
 * wrapped. It is the line as the server draws it, kept up to date: what
 * `status-left` and `status-right` say is what it says.
 */
export function StatusPanel({
  line,
  theme,
  open,
  onClose,
  onOpen,
}: {
  line: StatusLine;
  theme: Theme;
  open: boolean;
  onClose: () => void;
  onOpen: (pane: string) => void;
}) {
  const fg = css(line.fg, theme.palette, theme.fg);
  const bg = css(line.bg, theme.palette, theme.bg);
  const inside = useFocusInside(open);
  // A part a run that says something; runs of spaces only, between them.
  const parts = [...line.left, ...line.right].filter((s) => s.text.trim());
  const label = (segs: Seg[]) => segs.map((s) => s.text).join("").trim();
  return (
    <Drawer.Backdrop isOpen={open} onOpenChange={(o) => !o && onClose()} variant="blur" isDismissable isKeyboardDismissDisabled={false}>
      <Drawer.Content placement="right">
        <Drawer.Dialog id="status-panel" className="w-[85vw] max-w-[420px]">
          <Drawer.Header>
            <Drawer.Heading>{t(`Status of ${line.session}`, `${line.session} 的状态`)}</Drawer.Heading>
          </Drawer.Header>
          <Drawer.Body className="thin-scroll">
            <div ref={inside} className="flex flex-col gap-4 pb-2">
              <div className="flex flex-col gap-0.5">
                <div className="mb-1 text-xs font-semibold tracking-wide text-muted">{t("Windows", "窗口")}</div>
                {line.windows.map((w) => (
                  <Button
                    key={w.index}
                    fullWidth
                    data-window={w.index}
                    aria-current={w.current ? "true" : undefined}
                    variant={w.current ? "secondary" : "ghost"}
                    className="justify-start font-term"
                    onPress={() => {
                      onClose();
                      if (w.pane) onOpen(w.pane);
                    }}
                  >
                    <span className="truncate">{label(w.segments)}</span>
                  </Button>
                ))}
              </div>
              {parts.length ? (
                <div>
                  <div className="mb-1.5 text-xs font-semibold tracking-wide text-muted">{t("Status line", "状态栏")}</div>
                  <div id="status-parts" className="font-term flex flex-wrap gap-1.5 rounded-xl p-2 text-[13px] leading-6" style={{ color: fg, background: bg }}>
                    {parts.map((s, i) => (
                      <Run key={i} className="rounded-md px-1" s={{ ...s, text: s.text.trim() }} theme={theme} fg={fg} bg={bg} />
                    ))}
                  </div>
                </div>
              ) : null}
            </div>
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

/**
 * The session's status line, as the terminal draws it (`status-left`, the
 * windows, `status-right`, in its colours), under the screen: only where
 * all of it fits (`StatusFit`; else `StatusPanel` has it). A window's label
 * opens it.
 */
export function StatusBar({ line, theme, onOpen }: { line: StatusLine; theme: Theme; onOpen: (pane: string) => void }) {
  const fg = css(line.fg, theme.palette, theme.fg);
  const bg = css(line.bg, theme.palette, theme.bg);
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
      <div className="flex min-w-0 flex-1">
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
      <div className="shrink-0">
        {line.right.map((s, i) => (
          <Run key={i} s={s} theme={theme} fg={fg} bg={bg} />
        ))}
      </div>
    </div>
  );
}