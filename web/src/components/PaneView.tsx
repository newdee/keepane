import { Button, Dropdown, Header, Input, Label, Separator, TextArea, toast, Tooltip } from "@heroui/react";
import {
  ArrowLeft,
  Keyboard,
  LayoutList,
  Bot,
  Check,
  ChevronUp,
  Circle,
  Copy,
  Mail,
  RotateCcw,
  Search,
  SquareTerminal,
  ZoomIn,
  ZoomOut,
  Fullscreen,
  SlidersHorizontal,
  Minimize,
  ChevronDown,
  Clock,
  CornerDownLeft,
  Ellipsis,
  History as HistoryIcon,
  Inbox,
  Maximize2,
  PanelBottom,
  Columns2,
  Plus,
  Rows2,
  SendHorizontal,
  SquarePen,
  WrapText,
  X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { getJson, post, q, type Pane, type Theme } from "../api";
import { ansiToHtml, blank } from "../ansi";
import { bounced, headOf, stampDetail, stampText, under, where } from "../format";
import { useScreen, useStored, useVisible } from "../hooks";
import { t } from "../i18n";
import { copyText, outputOf, plain } from "../copy";
import { ContextMenu, type MenuAt } from "./ContextMenu";
import { ConfirmClose, History, type Sent } from "./Sheets";

// Buttons: what they show, what they send (a named key, `=` text, `@` the
// page's own: Ctrl, Alt). The usual ones, then the rest.
const KEYS: [string, string][] = [
  ["Ctrl", "@ctrl"], ["Alt", "@alt"], ["Shift", "@shift"], ["Esc", "Escape"], ["Tab", "Tab"], ["↑", "Up"], ["↓", "Down"],
  ["←", "Left"], ["→", "Right"], ["⏎", "Enter"], ["^C", "C-c"],
];
const MORE_KEYS: [string, string][] = [
  ["⇧Tab", "BTab"], ["^D", "C-d"], ["^Z", "C-z"], ["^L", "C-l"], ["⌫", "BSpace"], ["Home", "Home"], ["End", "End"],
  ["PgUp", "PPage"], ["PgDn", "NPage"], ["y", "=y"], ["n", "=n"], ["1", "=1"], ["2", "=2"], ["3", "=3"],
];
/** The box: live (sent as typed), typed on Enter, or typed and run. */
type InputMode = "live" | "type" | "run";
/** A device that chose before live typing came keeps its choice. */
function choseBefore(): InputMode {
  try {
    const old = localStorage.getItem("keepane-enter-runs");
    return old === null ? "live" : old === "true" ? "run" : "type";
  } catch {
    return "live";
  }
}
// The page's own keys that are held for the next one: their letters.
const MODS: Record<string, string> = { "@ctrl": "C", "@alt": "M", "@shift": "S" };
// The keys kept in view at the end of the row (the others scroll).
const PINNED = ["C-c", "Enter"];
const HISTORY_MAX = 50;
// The left column of command times, in characters.
const GUTTER = 6;
// A pane too wide to read at this width wraps at a size that reads (the
// default), or keeps its width and scrolls sideways.
const READABLE = 9;
const WRAPPED = 12;
// The screen's text size, as this device keeps it: a pinch, or A+ A−.
const ZOOM_MIN = 0.6;
const ZOOM_MAX = 2;
const zoomed = (z: number) => Math.round(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z)) * 100) / 100;

type Props = {
  id: string;
  panes: Pane[];
  readOnly: boolean;
  theme: Theme;
  wide: boolean;
  onBack: () => void;
  onSwitcher: () => void;
  onGo: (id: string) => void;
  onMade: (id: string) => void;
  reloadPanes: () => Promise<void>;
  onRename: (kind: "session" | "window", p: Pane) => void;
  onInbox: (id: string) => void;
  /** The latency, where the page's own header is not shown. */
  status?: ReactNode;
  /** Full screen: the screen and the input only. */
  focus: boolean;
  onFocus: (on: boolean) => void;
};

export function PaneView(props: Props) {
  const { id, panes, readOnly, theme } = props;
  const p = panes.find((x) => x.id === id);
  const visible = useVisible();
  const [wrap, setWrap] = useStored<boolean | number>("keepane-wrap", true);
  const [detail, setDetail] = useStored<boolean | number>("keepane-detail", false);
  // What the box does with what is typed: sends it as it is typed (live),
  // types it on Enter (a second Enter runs it), or types and runs it.
  const [inputMode, setInputMode] = useStored<InputMode>("keepane-input", choseBefore());
  const [zoomStored, setZoom] = useStored<number>("keepane-zoom", 1);
  const zoom = typeof zoomStored === "number" && zoomStored > 0 ? zoomed(zoomStored) : 1;
  const [fitting, setFitting] = useState(false);
  const mainRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [charRatio, setCharRatio] = useState(0.6);

  // The width of a character of the terminal's font, to fit a pane's
  // columns to the page.
  useLayoutEffect(() => {
    const probe = document.createElement("span");
    probe.className = "font-term";
    probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre;font-size:100px";
    probe.textContent = "M".repeat(20);
    document.body.appendChild(probe);
    setCharRatio(probe.getBoundingClientRect().width / 2000 || 0.6);
    probe.remove();
  }, []);
  useLayoutEffect(() => {
    const el = mainRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const cols = (p ? p.cols : 80) + (detail ? GUTTER : 0);
  const fit = (width - 24) / (cols * charRatio);
  const wrapping = !!wrap && fit < READABLE;
  const fontSize = (wrapping ? WRAPPED : Math.max(8, Math.min(15, fit))) * zoom;
  // Wrapping here, the lines the pane itself wrapped come joined, so that a
  // line breaks once, at this page's edge.
  const query = `pane=${q(id)}&history=300${wrapping ? "&join=1" : ""}`;
  const screen = useScreen(id, query);

  // ---- The pane sized to this device: on, it takes the page's columns and
  // rows (zoomed in its window); leaving the pane or putting the phone away
  // gives it its size back.
  const size = useCallback(() => {
    const m = mainRef.current!;
    return {
      cols: Math.max(10, Math.floor((m.clientWidth - 24) / (WRAPPED * zoom * charRatio))),
      rows: Math.max(3, Math.floor((m.clientHeight - 16) / (WRAPPED * zoom * 1.25))),
    };
  }, [charRatio, zoom]);
  const fitNow = useCallback(
    async (say: boolean) => {
      if (readOnly) return;
      const { cols, rows } = size();
      try {
        await post(`/api/fit?pane=${q(id)}&cols=${cols}&rows=${rows}`);
        if (say)
          toast(
            t(`Fitted to this screen (${cols}×${rows})`, `已适配这块屏幕（${cols}×${rows}）`),
            {
              description: t(
                "The computer and any other phone see this session at this size too, until you leave this pane.",
                "电脑和其他连着的手机看这个会话也会变成这个大小，离开这个 pane 后自动恢复。",
              ),
              timeout: 6000,
            },
          );
        await props.reloadPanes();
      } catch (e) {
        toast.danger((e as Error).message);
      }
    },
    [id, readOnly, size, props],
  );
  useEffect(() => {
    if (!fitting) return;
    if (!visible) {
      post(`/api/fit?pane=${q(id)}&off=1`).catch(() => {});
      return;
    }
    fitNow(false);
    let timer: number | undefined;
    const onResize = () => {
      clearTimeout(timer);
      timer = window.setTimeout(() => fitNow(false), 400);
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      clearTimeout(timer);
      post(`/api/fit?pane=${q(id)}&off=1`).catch(() => {});
    };
    // fitNow changes with the panes; fitting again for that is not wanted.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitting, visible, id]);

  // ---- The screen
  const [stick, setStick] = useState(true);
  // Looking for text: the words (null: not looking), the lines that have
  // them, the one shown.
  const [find, setFind] = useState<string | null>(null);
  const [hit, setHit] = useState(0);
  const lines = useMemo(() => {
    const l = screen.text.split("\n");
    while (l.length && blank(l[l.length - 1])) l.pop();
    return l;
  }, [screen.text]);
  const hits = useMemo(() => {
    const w = find?.trim().toLowerCase();
    if (!w) return [];
    return lines.flatMap((l, i) => (plain(l).toLowerCase().includes(w) ? [i] : []));
  }, [lines, find]);
  const now = hits.length ? hits[Math.min(hit, hits.length - 1)] : -1;
  const html = useMemo(() => {
    const mark = (i: number, h: string) => (hits.includes(i) ? `<span class="hit${i === now ? " now" : ""}">${h}</span>` : h);
    if (!detail && !hits.length) return ansiToHtml(lines.join("\n"), theme.palette);
    if (!detail) return lines.map((l, i) => mark(i, ansiToHtml(l, theme.palette))).join("\n");
    // capture-pane ends every coloured line with a reset, so each line can be
    // turned into HTML on its own.
    const at = new Map(screen.marks.map((m) => [m[0], m]));
    return lines
      .map((l, i) => {
        const m = at.get(i);
        const g = m
          ? `<span class="stamp${m[3] ? " fail" : ""}" data-i="${i}">${stampText(m)}</span> `
          : " ".repeat(GUTTER);
        return g + mark(i, ansiToHtml(l, theme.palette));
      })
      .join("\n");
  }, [lines, screen.marks, detail, theme.palette, hits, now]);
  useLayoutEffect(() => {
    const m = mainRef.current;
    if (m && stick && find === null) m.scrollTop = m.scrollHeight;
  }, [html, stick, fontSize, find]);
  // The line found, in the middle of the screen.
  useLayoutEffect(() => {
    if (now >= 0) mainRef.current?.querySelector(".hit.now")?.scrollIntoView({ block: "center" });
  }, [now, html]);
  const step = (d: number) => hits.length && setHit((h) => (Math.min(h, hits.length - 1) + d + hits.length) % hits.length);

  // ---- Two fingers on the screen: its text bigger or smaller, not the page.
  // (The size as it is now, read through a ref: the listeners stay put for
  // the whole pinch.)
  const zoomNow = useRef(zoom);
  zoomNow.current = zoom;
  const setZoomNow = useRef(setZoom);
  setZoomNow.current = setZoom;
  // A full-screen program keeps no history here: a drag up or down on the
  // screen, or the wheel, scrolls it instead (its own view), one wheel step
  // for so much travel; in order, never too far behind.
  const altNow = useRef(false);
  altNow.current = !!p?.alt && !readOnly;
  const wheelQ = useRef({ chain: Promise.resolve(), waiting: 0 });
  const wheel = useCallback(
    (up: boolean) => {
      const w = wheelQ.current;
      if (w.waiting > 6) return;
      w.waiting++;
      w.chain = w.chain
        .then(() => post(`/api/send?pane=${q(id)}&key=${up ? "WheelUp" : "WheelDown"}`))
        .then(
          () => void w.waiting--,
          () => void w.waiting--,
        );
    },
    [id],
  );
  const wheelNow = useRef(wheel);
  wheelNow.current = wheel;
  useEffect(() => {
    const m = mainRef.current;
    if (!m) return;
    let start: { d: number; z: number } | null = null;
    // One finger on a full-screen program's screen: where it was last turned
    // into a wheel step.
    let dragY: number | null = null;
    let from = { x: 0, y: 0 };
    const STEP = 36;
    const drag = (e: TouchEvent) => {
      if (dragY === null || e.touches.length !== 1 || !altNow.current) return;
      const { clientX: x, clientY: y } = e.touches[0];
      // Mostly sideways: a swipe to the next pane, or the screen sideways.
      if (Math.abs(x - from.x) > Math.abs(y - from.y)) return;
      e.preventDefault();
      while (Math.abs(y - dragY) >= STEP) {
        // The finger down: what is above comes into view.
        const up = y > dragY;
        wheelNow.current(up);
        dragY += up ? STEP : -STEP;
      }
    };
    let spun = 0;
    const spin = (e: WheelEvent) => {
      if (!altNow.current) return;
      e.preventDefault();
      spun += e.deltaY;
      while (Math.abs(spun) >= 60) {
        wheelNow.current(spun < 0);
        spun += spun < 0 ? 60 : -60;
      }
    };
    const dist = (e: TouchEvent) => Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
    const down = (e: TouchEvent) => {
      if (e.touches.length === 2) start = { d: dist(e), z: zoomNow.current };
      dragY = e.touches.length === 1 ? e.touches[0].clientY : null;
      if (e.touches.length === 1) from = { x: e.touches[0].clientX, y: e.touches[0].clientY };
    };
    const move = (e: TouchEvent) => {
      drag(e);
      if (!start || e.touches.length !== 2) return;
      e.preventDefault();
      setZoomNow.current(zoomed((start.z * dist(e)) / Math.max(1, start.d)));
    };
    const up = (e: TouchEvent) => {
      if (e.touches.length < 2) start = null;
      if (e.touches.length === 0) dragY = null;
    };
    // Safari's own pinch: not here.
    const gesture = (e: Event) => e.preventDefault();
    m.addEventListener("touchstart", down, { passive: true });
    m.addEventListener("touchmove", move, { passive: false });
    m.addEventListener("touchend", up);
    m.addEventListener("gesturestart", gesture);
    m.addEventListener("wheel", spin, { passive: false });
    return () => {
      m.removeEventListener("wheel", spin);
      m.removeEventListener("touchstart", down);
      m.removeEventListener("touchmove", move);
      m.removeEventListener("touchend", up);
      m.removeEventListener("gesturestart", gesture);
    };
  }, []);

  // ---- Copying: the whole screen (its history too), or what one command printed.
  const copied = async (s: string) => {
    if (!s) return void toast(t("Nothing to copy", "没有可复制的内容"), { timeout: 2000 });
    if (await copyText(s)) toast.success(t("Copied", "已复制"), { timeout: 1500 });
    else toast.danger(t("This browser would not copy", "这个浏览器不让复制"));
  };
  const [stampMenu, setStampMenu] = useState<MenuAt | null>(null);
  useEffect(() => setStick(true), [id]);

  // ---- Swiping across the pane: the next pane (to the left) or the one
  // before (to the right), in the list's order; not while the screen itself
  // can still scroll that way.
  const swipe = useRef<{ x: number; y: number; at: number; left: number; room: number } | null>(null);
  // Still on screen (an edge swipe waits to see whether the system went back).
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const onTouchStart = (e: React.TouchEvent) => {
    if (e.touches.length !== 1) return void (swipe.current = null);
    const m = mainRef.current!;
    swipe.current = { x: e.touches[0].clientX, y: e.touches[0].clientY, at: Date.now(), left: m.scrollLeft, room: m.scrollWidth - m.clientWidth };
  };
  const onTouchEnd = (e: React.TouchEvent) => {
    const s = swipe.current;
    swipe.current = null;
    if (!s) return;
    const dx = e.changedTouches[0].clientX - s.x;
    const dy = e.changedTouches[0].clientY - s.y;
    if (Math.abs(dx) < 70 || Math.abs(dy) > Math.abs(dx) * 0.6 || Date.now() - s.at > 800) return;
    // From the left edge: not to the next pane. On a phone it goes back to
    // the list, unless the system's own edge gesture already did (it goes
    // back in the history too; twice would leave the page). On a wide screen
    // the page itself opens the list (App).
    if (s.x < 32) {
      if (dx > 0 && !props.wide) {
        window.setTimeout(() => {
          if (alive.current) props.onBack();
        }, 350);
      }
      return;
    }
    if (dx < 0 && s.room > 1 && s.left < s.room - 1) return;
    if (dx > 0 && s.room > 1 && s.left > 1) return;
    const ids = panes.map((x) => x.id);
    const i = ids.indexOf(id);
    if (ids.length < 2 || i < 0) return;
    const next = ids[(i + (dx < 0 ? 1 : ids.length - 1)) % ids.length];
    props.onGo(next);
    const n = panes.find((x) => x.id === next);
    toast(n ? `${headOf(n)} · ${where(n)}` : next, { timeout: 1500 });
  };

  // ---- Actions
  const [acting, setActing] = useState(false);
  const [closing, setClosing] = useState(false);
  const action = async (what: string) => {
    if (acting || !p) return;
    if (what === "inbox") return props.onInbox(id);
    if (what === "rename-window") return props.onRename("window", p);
    if (what === "rename-session") return props.onRename("session", p);
    if (what === "kill-pane") return setClosing(true);
    setActing(true);
    if (what.startsWith("mode:")) {
      try {
        await post(`/api/action?pane=${q(id)}&do=mode&mode=${what.slice(5)}`);
        await props.reloadPanes();
        toast.success(t(`${what.slice(5)} mode`, `${what.slice(5)} 模式`), { timeout: 2000 });
      } catch (e) {
        toast.danger((e as Error).message);
      } finally {
        setActing(false);
      }
      return;
    }
    try {
      const before = new Set(panes.map((x) => x.id));
      await post(`/api/action?pane=${q(id)}&do=${what}`);
      const now = await getJson<Pane[]>("/api/panes");
      await props.reloadPanes();
      const made = now.find((x) => !before.has(x.id));
      if (made) props.onMade(made.id);
    } catch (e) {
      toast.danger((e as Error).message);
    } finally {
      setActing(false);
    }
  };
  const close = async (yes: boolean) => {
    setClosing(false);
    if (!yes) return;
    try {
      await post(`/api/action?pane=${q(id)}&do=kill-pane`);
      await props.reloadPanes();
      props.onBack();
    } catch (e) {
      toast.danger((e as Error).message);
    }
  };

  // The View menu's entries.
  const viewAction = (k: string) => {
    if (k === "fit") {
      if (bounced("fit")) return;
      if (fitting) toast(t("Back to its size", "已恢复原来的大小"), { timeout: 2000 });
      else fitNow(true);
      setFitting(!fitting);
    } else if (k === "wrap") {
      setWrap(!wrap);
      toast(!wrap ? t("Long lines wrap", "长行自动换行") : t("Long lines scroll sideways", "长行左右滑动"), { timeout: 2000 });
    } else if (k === "detail") {
      toggleDetail();
    } else if (k === "enter:live" || k === "enter:type" || k === "enter:run") {
      setInputMode(k.slice(6) as InputMode);
    } else if (k === "copy") {
      copied(lines.map((l) => plain(l).replace(/\s+$/, "")).join("\n"));
    } else if (k === "find") {
      setFind("");
      setHit(0);
      // Once the menu is gone (it gives the focus back to its button).
      window.setTimeout(() => document.getElementById("find")?.focus());
    } else if (k === "zoom-in" || k === "zoom-out" || k === "zoom-1") {
      const z = k === "zoom-1" ? 1 : zoomed(zoom * (k === "zoom-in" ? 1.15 : 1 / 1.15));
      setZoom(z);
      toast(t(`Text ${Math.round(z * 100)}%`, `字号 ${Math.round(z * 100)}%`), { timeout: 1200 });
    }
  };

  const toggleDetail = () => {
    const on = !detail;
    setDetail(on);
    if (on && !screen.marks.length)
      toast(
        t(
          "No command times here yet: a time is kept for each command the pane's shell (PowerShell, zsh, bash) finishes, from the pane's start.",
          "这个 pane 还没有命令时间：只记录 pane 启动后、在 shell（PowerShell、zsh、bash）里执行完的命令。",
        ),
        { timeout: 6000 },
      );
  };

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col">
      {/* The pane's bar: back (phone), what it is (tap: another pane), the tools. */}
      {props.focus ? null : (
      <div className="flex items-center gap-1 border-b border-separator px-2 py-1.5 sm:px-3">
        {!props.wide ? (
          <Button id="back" isIconOnly variant="ghost" size="sm" className="pointer-coarse:size-11" aria-label={t("Back", "返回")} onPress={props.onBack}>
            <ArrowLeft className="size-5" />
          </Button>
        ) : null}
        <button
          type="button"
          onClick={() => !bounced("switch") && props.onSwitcher()}
          className="flex min-w-0 flex-1 flex-col items-start rounded-lg px-1.5 py-0.5 text-left outline-none hover:bg-default focus-visible:ring-2 focus-visible:ring-focus"
        >
          <span className="flex max-w-full items-center gap-1 font-semibold">
            <span className="truncate">{p ? headOf(p) : id}</span>
            <ChevronDown className="size-3.5 shrink-0 text-muted" />
          </span>
          <span className="max-w-full truncate text-xs text-muted">
            {p ? [...under(p), where(p), p.alt ? t("full screen: scroll it", "全屏程序：滑动交给它滚动") : ""].filter(Boolean).join(" · ") : ""}
          </span>
        </button>
        {/* How the screen is shown, and what Enter does: one menu. */}
        <Dropdown>
          <Button
            id="view"
            isIconOnly
            size="sm"
            variant={fitting ? "secondary" : "ghost"}
            className={(fitting ? "text-accent " : "") + "pointer-coarse:size-11"}
            aria-label={t("View", "视图")}
          >
            <SlidersHorizontal className="size-4" />
          </Button>
          <Dropdown.Popover placement="bottom end" className="min-w-60">
            <Dropdown.Menu onAction={(k) => viewAction(String(k))}>
              <Dropdown.Section>
                <Header>{t("Screen", "屏幕")}</Header>
                {readOnly ? null : (
                  <Item id="fit" icon={<Maximize2 className="size-4" />} checked={fitting}>
                    {t("Fit to this screen", "适配这块屏幕")}
                  </Item>
                )}
                <Item id="wrap" icon={<WrapText className="size-4" />} checked={!!wrap}>
                  {t("Wrap long lines", "长行换行")}
                </Item>
                <Item id="detail" icon={<Clock className="size-4" />} checked={!!detail}>
                  {t("When each command ran", "每条命令的时间")}
                </Item>
                <Item id="find" icon={<Search className="size-4" />}>
                  {t("Find in the output", "在输出里查找")}
                </Item>
                <Item id="copy" icon={<Copy className="size-4" />}>
                  {t("Copy the screen's text", "复制屏幕文字")}
                </Item>
              </Dropdown.Section>
              <Separator />
              <Dropdown.Section>
                <Header>{t(`Text size ${Math.round(zoom * 100)}% (or pinch)`, `字号 ${Math.round(zoom * 100)}%（也可以双指缩放）`)}</Header>
                <Item id="zoom-in" icon={<ZoomIn className="size-4" />}>{t("Bigger", "放大")}</Item>
                <Item id="zoom-out" icon={<ZoomOut className="size-4" />}>{t("Smaller", "缩小")}</Item>
                {zoom !== 1 ? <Item id="zoom-1" icon={<RotateCcw className="size-4" />}>{t("Back to 100%", "恢复 100%")}</Item> : null}
              </Dropdown.Section>
              {readOnly ? null : <Separator />}
              {readOnly ? null : (
                <Dropdown.Section>
                  <Header>{t("The box", "输入框")}</Header>
                  <Item id="enter:live" icon={<Keyboard className="size-4" />} checked={inputMode === "live"}>
                    {t("Live: sent as typed", "实时输入（边打边发）")}
                  </Item>
                  <Item id="enter:type" icon={<CornerDownLeft className="size-4" />} checked={inputMode === "type"}>
                    {t("Type only (Enter again runs)", "只填入（再回车执行）")}
                  </Item>
                  <Item id="enter:run" icon={<SendHorizontal className="size-4" />} checked={inputMode === "run"}>
                    {t("Type and run", "填入并执行")}
                  </Item>
                </Dropdown.Section>
              )}
            </Dropdown.Menu>
          </Dropdown.Popover>
        </Dropdown>
        {props.status}
        <Tool id="fullscreen" label={t("Full screen: the screen and the box only", "全屏：只留屏幕和输入框")} onPress={() => props.onFocus(true)}>
          <Fullscreen className="size-4" />
        </Tool>
        {readOnly ? (
          <Tool label={t("Inbox", "收件箱")} onPress={() => props.onInbox(id)}>
            <Inbox className="size-4" />
          </Tool>
        ) : (
          <Dropdown>
            <Button isIconOnly variant="ghost" size="sm" className="pointer-coarse:size-11" aria-label={t("More", "更多")} isDisabled={acting}>
              <Ellipsis className="size-4" />
            </Button>
            <Dropdown.Popover placement="bottom end">
              <Dropdown.Menu onAction={(k) => action(String(k))}>
                <Item id="inbox" icon={<Inbox className="size-4" />}>{t("Inbox", "收件箱")}</Item>
                <Item id="split-h" icon={<Columns2 className="size-4" />}>{t("Split left | right", "左右分屏")}</Item>
                <Item id="split-v" icon={<Rows2 className="size-4" />}>{t("Split top / bottom", "上下分屏")}</Item>
                <Item id="new-window" icon={<Plus className="size-4" />}>{t("New window", "新窗口")}</Item>
                <Item id="rename-window" icon={<SquarePen className="size-4" />}>{t("Rename this window", "重命名这个窗口")}</Item>
                <Item id="rename-session" icon={<PanelBottom className="size-4" />}>{t("Rename this session", "重命名这个 session")}</Item>
                <Item id="kill-pane" danger icon={<X className="size-4" />}>{t("Close this pane", "关闭这个 pane")}</Item>
                <Dropdown.Section>
                  <Header>{t("Messages it gets", "收到的消息")}</Header>
                  <Item id="mode:normal" icon={<Circle className="size-4" />} checked={!p?.mode || p.mode === "normal"}>{t("normal: leaves them waiting", "normal：消息留着等")}</Item>
                  <Item id="mode:shell" icon={<SquareTerminal className="size-4" />} checked={p?.mode === "shell"}>{t("shell: runs them at its prompt", "shell：在提示符下执行")}</Item>
                  <Item id="mode:ai" icon={<Bot className="size-4" />} checked={p?.mode === "ai"}>{t("ai: hands them to its agent", "ai：交给 agent")}</Item>
                </Dropdown.Section>
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
        )}
      </div>
      )}

      {find !== null ? (
        <div className="flex items-center gap-1 border-b border-separator px-2 py-1.5 sm:px-3">
          <Search className="ml-1 size-4 shrink-0 text-muted" />
          <Input
            id="find"
            autoFocus
            value={find}
            placeholder={t("Find in the output", "在输出里查找")}
            className="min-w-0 flex-1"
            autoCapitalize="off"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            enterKeyHint="search"
            onChange={(e) => {
              setFind(e.target.value);
              setHit(Number.MAX_SAFE_INTEGER);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") step(e.shiftKey ? 1 : -1);
              else if (e.key === "Escape") setFind(null);
            }}
          />
          <span id="find-count" className="shrink-0 px-1 text-xs text-muted tabular-nums">
            {find.trim() ? (hits.length ? `${hits.indexOf(now) + 1}/${hits.length}` : t("none", "没有")) : ""}
          </span>
          <Tool id="find-up" label={t("The one before (higher up)", "上一个（更早）")} onPress={() => step(-1)}>
            <ChevronUp className="size-4" />
          </Tool>
          <Tool id="find-down" label={t("The next one (lower down)", "下一个（更晚）")} onPress={() => step(1)}>
            <ChevronDown className="size-4" />
          </Tool>
          <Tool id="find-close" label={t("Stop finding", "关闭查找")} onPress={() => setFind(null)}>
            <X className="size-4" />
          </Tool>
        </div>
      ) : null}
      {p?.dead && !readOnly ? (
        <div id="ended" className="flex items-center gap-2 border-b border-separator bg-danger-soft px-3 py-2 text-sm text-danger-soft-foreground">
          <span className="min-w-0 flex-1">
            {p.exit != null
              ? t(`Its program ended (exit ${p.exit}).`, `程序已退出（退出码 ${p.exit}）。`)
              : t("Its program ended.", "程序已退出。")}
          </span>
          <Button id="respawn" size="sm" variant="secondary" onPress={() => action("respawn")} isDisabled={acting}>
            <RotateCcw className="size-4" />
            {t("Run it again", "重新运行")}
          </Button>
        </div>
      ) : null}

      {/* The screen, in the terminal's colours. */}
      <div
        className={"relative min-h-0 flex-1 p-0" + (props.focus ? "" : " sm:p-3")}
        style={props.focus ? { paddingTop: "env(safe-area-inset-top)", background: theme.bg } : undefined}
      >
        {/* Read-only, there is no input row to hold the ways out: over the screen. */}
        {props.focus && readOnly ? (
          <div className="absolute top-[calc(env(safe-area-inset-top)+6px)] right-2 z-10 flex gap-1.5">
            {[
              { id: "fullscreen-switch", label: t("Another pane", "切换 pane"), icon: <LayoutList className="size-4" />, go: () => !bounced("switch") && props.onSwitcher() },
              { id: "fullscreen-exit", label: t("Leave full screen", "退出全屏"), icon: <Minimize className="size-4" />, go: () => props.onFocus(false) },
            ].map((b) => (
              <Button key={b.id} id={b.id} isIconOnly size="sm" variant="ghost" aria-label={b.label} onPress={b.go}
                className="rounded-full bg-black/35 text-white opacity-70 hover:opacity-100 pointer-coarse:size-11">
                {b.icon}
              </Button>
            ))}
          </div>
        ) : null}
        <div
          id="main"
          ref={mainRef}
          onScroll={(e) => {
            const m = e.currentTarget;
            setStick(m.scrollHeight - m.scrollTop - m.clientHeight < 40);
          }}
          onTouchStart={onTouchStart}
          onTouchEnd={onTouchEnd}
          className={"thin-scroll h-full overflow-auto px-3 py-2" + (props.focus ? "" : " sm:rounded-2xl sm:shadow-lg sm:ring-1 sm:ring-black/5")}
          style={{ background: theme.bg, ["--term-fg" as string]: theme.fg, ["--term-bg" as string]: theme.bg }}
        >
          {screen.error ? (
            <p className="font-term text-sm" style={{ color: theme.palette[1] }}>
              {screen.error}
            </p>
          ) : null}
          <pre
            id="screen"
            className={"term" + (wrapping ? " wrap" : "")}
            style={{ fontSize: fontSize.toFixed(2) + "px" }}
            onClick={(e) => {
              const s = (e.target as HTMLElement).closest(".stamp") as HTMLElement | null;
              const m = s && screen.marks.find((m) => m[0] === Number(s.dataset.i));
              if (!m) return;
              setStampMenu({
                x: e.clientX,
                y: e.clientY,
                title: stampDetail(m),
                items: [
                  { id: "out", label: t("Copy what it printed", "复制这条命令的输出"), icon: <Copy className="size-4" />, now: true },
                  { id: "cmd", label: t("Copy the command line", "复制这条命令"), icon: <SquareTerminal className="size-4" />, now: true },
                ],
                onPick: (k) => copied(k === "out" ? outputOf(lines, screen.marks, m[0]) : plain(lines[m[0]] ?? "").trim()),
              });
            }}
            dangerouslySetInnerHTML={{ __html: html }}
          />
        </div>
      </div>

      {readOnly ? null : (
        <Composer
          id={id}
          onSent={() => !screen.streaming && setTimeout(screen.poll, 120)}
          mode={inputMode}
          title={props.focus && p ? `${headOf(p)} · ${where(p)}` : undefined}
          tellTo={p && (p.mode === "shell" || p.mode === "ai") ? headOf(p) : undefined}
          exit={
            props.focus ? (
              <>
                <Button id="fullscreen-exit" isIconOnly variant="ghost" size="sm" className="mb-0.5 pointer-coarse:size-11" aria-label={t("Leave full screen", "退出全屏")}
                  onPress={() => props.onFocus(false)}>
                  <Minimize className="size-4.5" />
                </Button>
                {/* The pane bar is gone in full screen: its list of panes here. */}
                <Button id="fullscreen-switch" isIconOnly variant="ghost" size="sm" className="mb-0.5 pointer-coarse:size-11" aria-label={t("Another pane", "切换 pane")}
                  onPress={() => !bounced("switch") && props.onSwitcher()}>
                  <LayoutList className="size-4.5" />
                </Button>
              </>
            ) : null
          }
        />
      )}
      <ConfirmClose open={closing} onDone={close} what={p ? `${headOf(p)} · ${where(p)}` : undefined} program={p?.command} />
      <ContextMenu at={stampMenu} onClose={() => setStampMenu(null)} />
    </div>
  );
}

function Tool({ id, label, on, onPress, children }: { id?: string; label: string; on?: boolean; onPress: () => void; children: ReactNode }) {
  return (
    <Tooltip delay={500}>
      <Button id={id} isIconOnly size="sm" variant={on ? "secondary" : "ghost"} aria-label={label} aria-pressed={on} onPress={onPress}
        className={(on ? "text-accent " : "") + "pointer-coarse:size-11"}>
        {children}
      </Button>
      <Tooltip.Content>{label}</Tooltip.Content>
    </Tooltip>
  );
}

function Item({
  id,
  icon,
  danger,
  checked,
  children,
}: {
  id: string;
  icon: ReactNode;
  danger?: boolean;
  checked?: boolean;
  children: ReactNode;
}) {
  return (
    <Dropdown.Item id={id} textValue={String(children)} variant={danger ? "danger" : "default"}>
      {icon}
      <Label>{children}</Label>
      {checked ? <Check className="ml-auto size-4 text-accent" /> : null}
    </Dropdown.Item>
  );
}

/** The keys and the box. Everything typed goes out in the order it was
 *  tapped. Live (the default), what is typed goes out as it is typed and
 *  Enter is the Enter; else Send sends what is in the box, no Enter after it
 *  (Send again with the box empty, or ⏎ among the keys, is the Enter), or
 *  with it. Ctrl, Alt and Shift stay down for the next key. */
function Composer({
  id,
  onSent,
  exit,
  mode,
  title,
  tellTo,
}: {
  id: string;
  onSent: () => void;
  exit?: ReactNode;
  /** Live, or typed on Enter, or typed and run on Enter. */
  mode: InputMode;
  /** In full screen, which pane the box types into. */
  title?: string;
  /** A pane that takes messages (shell, ai): its name, for the message switch. */
  tellTo?: string;
}) {
  const [text, setText] = useState("");
  // The box as a message into the pane's inbox (it waits its turn), not typing.
  const [telling, setTelling] = useState(false);
  const tell = telling && !!tellTo;
  const enterRuns = mode === "run";
  // Live: what of the box has gone out (since the last Enter or key), and
  // the box as it is now, sent a moment later (a burst of typing goes as one).
  const live = mode === "live" && !tell;
  const wentOut = useRef("");
  const latest = useRef<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  // Ctrl, Alt, Shift held for the next key (button or typed): their letters.
  const [held, setHeld] = useState("");
  const [more, setMore] = useStored<boolean | number>("keepane-more-keys", false);
  const [sent, setSentState] = useStored<Sent[]>("keepane-sent", []);
  const [histOpen, setHistOpen] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const queue = useRef<Promise<boolean>>(Promise.resolve(true));

  const send = useCallback(
    (query: string, body?: string) => {
      const pane = id;
      queue.current = queue.current.then(async () => {
        try {
          await post(`/api/send?pane=${q(pane)}${query}`, body);
          onSent();
          return true;
        } catch (e) {
          toast.danger((e as Error).message);
          return false;
        }
      });
      return queue.current;
    },
    [id, onSent],
  );
  const sendKey = (k: string) => (k.startsWith("=") ? send("", k.slice(1)) : send(`&key=${q(k)}`));
  // The box against what went out: from the first character that differs,
  // that many backspaces, then the rest typed. The program's line ends up as
  // the box whatever was edited where (an autocorrected word, a middle edit).
  const syncNow = () => {
    clearTimeout(timer.current);
    const now = latest.current;
    latest.current = null;
    if (now === null) return;
    const was = Array.from(wentOut.current);
    const is = Array.from(now);
    let same = 0;
    while (same < was.length && same < is.length && was[same] === is[same]) same++;
    const out = "\x7f".repeat(was.length - same) + is.slice(same).join("");
    wentOut.current = now;
    if (out) send("", out);
  };
  const later = (v: string) => {
    latest.current = v;
    clearTimeout(timer.current);
    timer.current = window.setTimeout(syncNow, 30);
  };
  // A fresh box: after Enter, or a key the box cannot follow (Tab, an arrow).
  const fresh = () => {
    latest.current = null;
    clearTimeout(timer.current);
    wentOut.current = "";
    setText("");
    // The box itself too: its own change, coming after, reads it back.
    if (box.current) {
      box.current.value = "";
      box.current.style.height = "";
    }
  };
  // Live on or off (the mode, or the message switch): the box starts afresh.
  useEffect(() => fresh(), [live]); // eslint-disable-line react-hooks/exhaustive-deps
  const remember = (s: string) => {
    const had = sent.find((x) => x.t === s);
    let loose = 0;
    setSentState(
      [{ t: s, star: !!had?.star }, ...sent.filter((x) => x.t !== s)].filter((x) => x.star || ++loose <= HISTORY_MAX),
    );
  };
  // A tap is debounced (a finger bouncing); a key pressed twice quickly
  // (send, then Enter) is meant twice.
  const submit = async (byKey = false) => {
    if (!byKey && bounced("send")) return;
    const s = text;
    if (tell) {
      if (!s.trim()) return;
      setText("");
      try {
        const said = await (await post(`/api/tell?pane=${q(id)}`, s)).text();
        remember(s);
        toast.success(said || t("Queued", "已排队"), { timeout: 2500 });
      } catch (e) {
        setText((now) => now || s);
        toast.danger((e as Error).message);
      }
      return;
    }
    if (live) {
      syncNow();
      if (s.trim()) remember(s);
      fresh();
      return void send("&key=Enter");
    }
    setText("");
    if (!s) return void send("&key=Enter");
    if (await send("", s)) {
      remember(s);
      if (enterRuns) send("&key=Enter");
    } else setText((now) => now || s);
  };
  const onKey = (k: string) => {
    if (bounced("key " + k)) return;
    const mod = MODS[k];
    if (mod) {
      const next = held.includes(mod) ? held.replace(mod, "") : [...held + mod].sort((a, b) => "CMS".indexOf(a) - "CMS".indexOf(b)).join("");
      setHeld(next);
      if (next) toast(t("Now a key, or type one", "现在按一个键，或打一个字"), { timeout: 1500 });
      return;
    }
    // Held keys go with a button's key (Shift with → is S-Right); text keys
    // (y, n, 1…) are text.
    const withHeld = held && !k.startsWith("=") ? (held === "S" && k === "Tab" ? "BTab" : [...held].map((m) => m + "-").join("") + k) : k;
    setHeld("");
    if (live) {
      syncNow();
      fresh();
    }
    sendKey(withHeld);
  };

  // What Send does now, in a word: the button's text and its name.
  const sendLabel = tell
    ? t("Queue", "排队")
    : live
      ? t("Enter", "回车")
      : text
      ? enterRuns
        ? t("Run", "执行")
        : t("Send", "发送")
      : t("Enter", "回车");

  const keyBtn = ([label, k]: [string, string]) => (
    <Button
      key={k}
      size="sm"
      variant={MODS[k] && held.includes(MODS[k]) ? "primary" : "tertiary"}
      aria-pressed={MODS[k] ? held.includes(MODS[k]) : undefined}
      className="h-8 min-w-9 shrink-0 px-2 font-term text-[13px] pointer-coarse:h-10 pointer-coarse:min-w-11"
      onPress={() => onKey(k)}
    >
      {label}
    </Button>
  );

  return (
    <div className="border-t border-separator bg-background/80 px-2 pt-2 pb-[calc(env(safe-area-inset-bottom)+8px)] backdrop-blur sm:px-3">
      {title ? (
        <div className="truncate px-1 pb-1.5 text-xs text-muted">{t(`Typing into ${title}`, `输入到 ${title}`)}</div>
      ) : null}
      {/* The keys; ^C and ⏎ stay in view at the end, the rest scroll. */}
      <div className="flex gap-1 pb-1.5">
        <div className="no-scrollbar flex min-w-0 flex-1 gap-1 overflow-x-auto">{KEYS.filter(([, k]) => !PINNED.includes(k)).map(keyBtn)}</div>
        <div className="flex shrink-0 gap-1">{KEYS.filter(([, k]) => PINNED.includes(k)).map(keyBtn)}</div>
      </div>
      {more ? <div className="flex flex-wrap gap-1 pb-1.5">{MORE_KEYS.map(keyBtn)}</div> : null}
      <div className="flex items-end gap-1.5">
        {exit}
        <Button isIconOnly variant="ghost" size="sm" className="mb-0.5 pointer-coarse:size-11" aria-label={t("Sent before", "发过的命令")}
          onPress={() => !bounced("hist") && setHistOpen(true)}>
          <HistoryIcon className="size-4.5" />
        </Button>
        <Button isIconOnly variant={more ? "secondary" : "ghost"} size="sm" className="mb-0.5 pointer-coarse:size-11" aria-label={t("More keys", "更多按键")}
          onPress={() => setMore(!more)}>
          <Ellipsis className="size-4.5" />
        </Button>
        {tellTo ? (
          <Button id="tell" isIconOnly variant={tell ? "primary" : "ghost"} size="sm" className="mb-0.5 pointer-coarse:size-11"
            aria-label={t(`As a message to ${tellTo} (into its inbox)`, `作为消息发给 ${tellTo}（进收件箱排队）`)} aria-pressed={tell}
            onPress={() => setTelling(!tell)}>
            <Mail className="size-4.5" />
          </Button>
        ) : null}
        <TextArea
          id="text"
          ref={box}
          rows={1}
          value={text}
          placeholder={
            tell
              ? t(`Message to ${tellTo}`, `消息给 ${tellTo}`)
              : live
                ? t("Live: Enter is Enter", "实时输入，回车即回车")
                : enterRuns
                  ? t("Enter runs", "回车执行")
                  : t("Enter types", "回车填入")
          }
          className="max-h-32 min-h-10 flex-1 resize-none font-term text-base"
          autoCapitalize="off"
          autoComplete="off"
          autoCorrect="off"
          spellCheck={false}
          onChange={(e) => {
            const el = e.target;
            el.style.height = "";
            el.style.height = Math.min(128, el.scrollHeight) + "px";
            setText(el.value);
          }}
          onInput={(e) => {
            // Ctrl or Alt held: the character typed is the key, not text;
            // Shift alone: the character, in capitals.
            const ne = e.nativeEvent as InputEvent;
            if (!held || !ne.data || ne.data.length !== 1 || ne.isComposing) {
              // Live: out it goes (not while an input method is still composing).
              if (live && !ne.isComposing) later(e.currentTarget.value);
              return;
            }
            const el = e.currentTarget;
            const at = el.selectionStart;
            if (held === "S") {
              const upper = el.value.slice(0, at - 1) + ne.data.toUpperCase() + el.value.slice(at);
              el.value = upper;
              el.setSelectionRange(at, at);
              setText(upper);
              setHeld("");
              if (live) later(upper);
              return;
            }
            const without = el.value.slice(0, at - 1) + el.value.slice(at);
            el.value = without;
            setText(without);
            const c = ne.data.toLowerCase();
            const mod = held.includes("C") ? "C" : "M";
            setHeld("");
            // A key the box cannot follow (C-a moves the line's cursor): afresh.
            if (live) {
              syncNow();
              fresh();
            }
            if (mod === "C" ? /[a-z]/.test(c) : /[a-z0-9]/.test(c)) sendKey(`${mod}-${c}`);
            else toast(t("Ctrl goes with a letter, Alt with a letter or digit", "Ctrl 只能配字母，Alt 只能配字母或数字"));
          }}
          onCompositionEnd={(e) => live && later(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit(true);
            } else if (live && e.key === "Backspace" && !text && !e.nativeEvent.isComposing) {
              // Nothing in the box to take back: a backspace for the program.
              e.preventDefault();
              send("&key=BSpace");
            }
          }}
        />
        <Button id="send" className="mb-0.5 shrink-0 pointer-coarse:h-11" onPress={() => submit()} aria-label={sendLabel}>
          {tell ? <Mail className="size-4" /> : text && !live ? <SendHorizontal className="size-4" /> : <CornerDownLeft className="size-4" />}
          <span className="hidden sm:inline">{sendLabel}</span>
        </Button>
      </div>
      <History open={histOpen} onClose={() => setHistOpen(false)} sent={sent} setSent={setSentState} onPick={(s) => {
        setText(s);
        if (live) later(s);
        setTimeout(() => box.current?.focus(), 50);
      }} />
    </div>
  );
}
