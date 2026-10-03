import { Button, Dropdown, Label, TextArea, toast, Tooltip } from "@heroui/react";
import {
  ArrowLeft,
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
import { ConfirmClose, History, type Sent } from "./Sheets";

// Buttons: what they show, what they send (a named key, `=` text, `@` the
// page's own: Ctrl, Alt). The usual ones, then the rest.
const KEYS: [string, string][] = [
  ["Ctrl", "@ctrl"], ["Alt", "@alt"], ["Esc", "Escape"], ["Tab", "Tab"], ["↑", "Up"], ["↓", "Down"],
  ["←", "Left"], ["→", "Right"], ["⏎", "Enter"], ["^C", "C-c"],
];
const MORE_KEYS: [string, string][] = [
  ["⇧Tab", "BTab"], ["^D", "C-d"], ["^Z", "C-z"], ["^L", "C-l"], ["⌫", "BSpace"], ["Home", "Home"], ["End", "End"],
  ["PgUp", "PPage"], ["PgDn", "NPage"], ["y", "=y"], ["n", "=n"], ["1", "=1"], ["2", "=2"], ["3", "=3"],
];
const HISTORY_MAX = 50;
// The left column of command times, in characters.
const GUTTER = 6;
// A pane too wide to read at this width wraps at a size that reads (the
// default), or keeps its width and scrolls sideways.
const READABLE = 9;
const WRAPPED = 12;

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
  /** The latency and the full-screen button, where the page's own header is not shown. */
  status?: ReactNode;
};

export function PaneView(props: Props) {
  const { id, panes, readOnly, theme } = props;
  const p = panes.find((x) => x.id === id);
  const visible = useVisible();
  const [wrap, setWrap] = useStored<boolean | number>("keepane-wrap", true);
  const [detail, setDetail] = useStored<boolean | number>("keepane-detail", false);
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
  const fontSize = wrapping ? WRAPPED : Math.max(8, Math.min(15, fit));
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
      cols: Math.max(10, Math.floor((m.clientWidth - 24) / (WRAPPED * charRatio))),
      rows: Math.max(3, Math.floor((m.clientHeight - 16) / (WRAPPED * 1.25))),
    };
  }, [charRatio]);
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
  const html = useMemo(() => {
    const lines = screen.text.split("\n");
    while (lines.length && blank(lines[lines.length - 1])) lines.pop();
    if (!detail) return ansiToHtml(lines.join("\n"), theme.palette);
    // capture-pane ends every coloured line with a reset, so each line can be
    // turned into HTML on its own.
    const at = new Map(screen.marks.map((m) => [m[0], m]));
    return lines
      .map((l, i) => {
        const m = at.get(i);
        const g = m
          ? `<span class="stamp${m[3] ? " fail" : ""}" data-i="${i}">${stampText(m)}</span> `
          : " ".repeat(GUTTER);
        return g + ansiToHtml(l, theme.palette);
      })
      .join("\n");
  }, [screen.text, screen.marks, detail, theme.palette]);
  useLayoutEffect(() => {
    const m = mainRef.current;
    if (m && stick) m.scrollTop = m.scrollHeight;
  }, [html, stick, fontSize]);
  useEffect(() => setStick(true), [id]);

  // ---- Swiping across the pane: the next pane (to the left) or the one
  // before (to the right), in the list's order; not while the screen itself
  // can still scroll that way.
  const swipe = useRef<{ x: number; y: number; at: number; left: number; room: number } | null>(null);
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
      <div className="flex items-center gap-1 border-b border-separator px-2 py-1.5 sm:px-3">
        {!props.wide ? (
          <Button id="back" isIconOnly variant="ghost" size="sm" aria-label={t("Back", "返回")} onPress={props.onBack}>
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
          <span className="max-w-full truncate text-xs text-muted">{p ? [...under(p), where(p)].join(" · ") : ""}</span>
        </button>
        {readOnly ? null : (
          <Tool label={t("Fit to this screen", "适配这块屏幕")} on={fitting} onPress={() => {
            if (bounced("fit")) return;
            if (fitting) toast(t("Back to its size", "已恢复原来的大小"), { timeout: 2000 });
            else fitNow(true);
            setFitting(!fitting);
          }}>
            <Maximize2 className="size-4" />
          </Tool>
        )}
        <Tool
          label={t("Wrap long lines", "长行换行")}
          on={!!wrap}
          onPress={() => {
            setWrap(!wrap);
            toast(!wrap ? t("Long lines wrap", "长行自动换行") : t("Long lines scroll sideways", "长行左右滑动"), { timeout: 2000 });
          }}
        >
          <WrapText className="size-4" />
        </Tool>
        <Tool label={t("When each command ran", "每条命令的时间")} on={!!detail} onPress={toggleDetail}>
          <Clock className="size-4" />
        </Tool>
        {props.status}
        {readOnly ? (
          <Tool label={t("Inbox", "收件箱")} onPress={() => props.onInbox(id)}>
            <Inbox className="size-4" />
          </Tool>
        ) : (
          <Dropdown>
            <Button isIconOnly variant="ghost" size="sm" aria-label={t("More", "更多")} isDisabled={acting}>
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
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
        )}
      </div>

      {/* The screen, in the terminal's colours. */}
      <div className="min-h-0 flex-1 p-0 sm:p-3">
        <div
          id="main"
          ref={mainRef}
          onScroll={(e) => {
            const m = e.currentTarget;
            setStick(m.scrollHeight - m.scrollTop - m.clientHeight < 40);
          }}
          onTouchStart={onTouchStart}
          onTouchEnd={onTouchEnd}
          className="thin-scroll h-full overflow-auto px-3 py-2 sm:rounded-2xl sm:shadow-lg sm:ring-1 sm:ring-black/5"
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
              if (m) toast(stampDetail(m), { timeout: 4000 });
            }}
            dangerouslySetInnerHTML={{ __html: html }}
          />
        </div>
      </div>

      {readOnly ? null : <Composer id={id} onSent={() => !screen.streaming && setTimeout(screen.poll, 120)} />}
      <ConfirmClose open={closing} onDone={close} />
    </div>
  );
}

function Tool({ label, on, onPress, children }: { label: string; on?: boolean; onPress: () => void; children: ReactNode }) {
  return (
    <Tooltip delay={500}>
      <Button isIconOnly size="sm" variant={on ? "secondary" : "ghost"} aria-label={label} aria-pressed={on} onPress={onPress}
        className={on ? "text-accent" : ""}>
        {children}
      </Button>
      <Tooltip.Content>{label}</Tooltip.Content>
    </Tooltip>
  );
}

function Item({ id, icon, danger, children }: { id: string; icon: ReactNode; danger?: boolean; children: ReactNode }) {
  return (
    <Dropdown.Item id={id} textValue={String(children)} variant={danger ? "danger" : "default"}>
      {icon}
      <Label>{children}</Label>
    </Dropdown.Item>
  );
}

/** The keys and the box. Everything typed goes out in the order it was
 *  tapped. Send sends what is in the box, no Enter after it: Send again with
 *  the box empty (or ⏎ among the keys) is the Enter. Ctrl and Alt stay down
 *  for the next character typed in the box. */
function Composer({ id, onSent }: { id: string; onSent: () => void }) {
  const [text, setText] = useState("");
  const [held, setHeld] = useState<"C" | "M" | null>(null);
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
    setText("");
    if (!s) return void send("&key=Enter");
    if (await send("", s)) remember(s);
    else setText((now) => now || s);
  };
  const onKey = (k: string) => {
    if (bounced("key " + k)) return;
    if (k === "@ctrl" || k === "@alt") {
      const want = k === "@ctrl" ? "C" : "M";
      const next = held === want ? null : want;
      setHeld(next);
      if (next) {
        box.current?.focus();
        toast(t("Now type a key", "现在输入一个键"), { timeout: 1500 });
      }
      return;
    }
    setHeld(null);
    sendKey(k);
  };

  const keyBtn = ([label, k]: [string, string]) => (
    <Button
      key={k}
      size="sm"
      variant={(k === "@ctrl" && held === "C") || (k === "@alt" && held === "M") ? "primary" : "tertiary"}
      className="h-8 min-w-9 shrink-0 px-2 font-term text-[13px]"
      onPress={() => onKey(k)}
    >
      {label}
    </Button>
  );

  return (
    <div className="border-t border-separator bg-background/80 px-2 pt-2 pb-[calc(env(safe-area-inset-bottom)+8px)] backdrop-blur sm:px-3">
      <div className="no-scrollbar flex gap-1 overflow-x-auto pb-1.5">{KEYS.map(keyBtn)}</div>
      {more ? <div className="flex flex-wrap gap-1 pb-1.5">{MORE_KEYS.map(keyBtn)}</div> : null}
      <div className="flex items-end gap-1.5">
        <Button isIconOnly variant="ghost" size="sm" className="mb-0.5" aria-label={t("Sent before", "发过的命令")}
          onPress={() => !bounced("hist") && setHistOpen(true)}>
          <HistoryIcon className="size-4.5" />
        </Button>
        <Button isIconOnly variant={more ? "secondary" : "ghost"} size="sm" className="mb-0.5" aria-label={t("More keys", "更多按键")}
          onPress={() => setMore(!more)}>
          <Ellipsis className="size-4.5" />
        </Button>
        <TextArea
          id="text"
          ref={box}
          rows={1}
          value={text}
          placeholder={t("Type a command…", "输入命令…")}
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
            // Ctrl or Alt held: the character typed is the key, not text.
            const ne = e.nativeEvent as InputEvent;
            if (!held || !ne.data || ne.data.length !== 1 || ne.isComposing) return;
            const el = e.currentTarget;
            const at = el.selectionStart;
            const without = el.value.slice(0, at - 1) + el.value.slice(at);
            el.value = without;
            setText(without);
            const c = ne.data.toLowerCase();
            const mod = held;
            setHeld(null);
            if (mod === "C" ? /[a-z]/.test(c) : /[a-z0-9]/.test(c)) sendKey(`${mod}-${c}`);
            else toast(t("Ctrl goes with a letter, Alt with a letter or digit", "Ctrl 只能配字母，Alt 只能配字母或数字"));
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit(true);
            }
          }}
        />
        <Button id="send" className="mb-0.5 shrink-0" onPress={() => submit()}
          aria-label={t("Send sends the text, with no Enter; Send again (the box empty) is the Enter", "点发送只发文字，不带回车；输入框空着时再点一次就是回车")}>
          {text ? <SendHorizontal className="size-4" /> : <CornerDownLeft className="size-4" />}
          <span className="hidden sm:inline">{text ? t("Send", "发送") : t("Enter", "回车")}</span>
        </Button>
      </div>
      <History open={histOpen} onClose={() => setHistOpen(false)} sent={sent} setSent={setSentState} onPick={(s) => {
        setText(s);
        setTimeout(() => box.current?.focus(), 50);
      }} />
    </div>
  );
}
