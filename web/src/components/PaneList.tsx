import { Chip } from "@heroui/react";
import {
  BellRing,
  Bot,
  Circle,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  Columns2,
  Inbox,
  Moon,
  PanelBottom,
  Plus,
  RotateCcw,
  Rows2,
  SquareArrowOutUpRight,
  SquarePen,
  SquareTerminal,
  Terminal,
  X,
} from "lucide-react";
import { useMemo, useState } from "react";
import { ContextMenu, useContextPress, type MenuAt, type MenuEntry } from "./ContextMenu";
import type { Agent, Pane } from "../api";
import { bounced, headOf, quietFor, stateOf, under } from "../format";
import { useStored } from "../hooks";
import { t } from "../i18n";

const sKey = (p: Pane) => `s ${p.session}`;
const wKey = (p: Pane) => `w ${p.session}:${p.window}`;

type Props = {
  panes: Pane[];
  current: string | null;
  readOnly: boolean;
  onOpen: (id: string) => void;
  onRename: (kind: "session" | "window", pane: Pane) => void;
  onInbox: (id: string) => void;
  /** A pane's menu entry that acts on the computer: split, new window, close. */
  onAction: (pane: Pane, what: string) => void;
};

/** Every pane, as keepane holds them: each session, its windows, their panes
 *  (`list-panes -a` gives them in that order). Folded sessions and windows
 *  are kept on this device. */
export function PaneList({ panes, current, readOnly, onOpen, onRename, onInbox, onAction }: Props) {
  const [foldedList, setFolded] = useStored<string[]>("keepane-folded", []);
  const folded = useMemo(() => new Set(foldedList), [foldedList]);
  const toggle = (k: string) => {
    if (bounced("fold " + k)) return;
    const next = new Set(folded);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    setFolded([...next]);
  };
  // Fold or open every window of a session at once.
  const foldWindows = (session: string, fold: boolean) => {
    const keys = new Set(panes.filter((p) => p.session === session).map(wKey));
    const next = new Set([...folded].filter((k) => !keys.has(k)));
    if (fold) for (const k of keys) next.add(k);
    next.delete(`s ${session}`);
    setFolded([...next]);
  };

  // Right click or long press: a menu of what can be done there.
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const paneMenu = (p: Pane, x: number, y: number) => {
    const items: MenuEntry[] = [
      { id: "open", label: t("Open", "打开"), icon: <SquareArrowOutUpRight className="size-4" /> },
      { id: "inbox", label: t("Inbox", "收件箱"), icon: <Inbox className="size-4" /> },
    ];
    if (!readOnly)
      items.push(
        { id: "split-h", label: t("Split left | right", "左右分屏"), icon: <Columns2 className="size-4" /> },
        { id: "split-v", label: t("Split top / bottom", "上下分屏"), icon: <Rows2 className="size-4" /> },
        { id: "new-window", label: t("New window", "新窗口"), icon: <Plus className="size-4" /> },
        { id: "rename-window", label: t("Rename its window", "重命名所在窗口"), icon: <SquarePen className="size-4" /> },
        { id: "rename-session", label: t("Rename its session", "重命名所在 session"), icon: <PanelBottom className="size-4" /> },
        { id: "kill-pane", label: t("Close this pane", "关闭这个 pane"), icon: <X className="size-4" />, danger: true },
      );
    // Its program ended: run it again.
    if (!readOnly && p.dead)
      items.splice(1, 0, { id: "respawn", label: t("Run it again", "重新运行"), icon: <RotateCcw className="size-4" /> });
    // What it does with the messages it gets.
    if (!readOnly) {
      const mode = p.mode || "normal";
      items.push(
        { id: "mode:normal", label: t("normal: leaves them waiting", "normal：消息留着等"), icon: <Circle className="size-4" />, section: t("Messages it gets", "收到的消息"), checked: mode === "normal" },
        { id: "mode:shell", label: t("shell: runs them at its prompt", "shell：在提示符下执行"), icon: <SquareTerminal className="size-4" />, checked: mode === "shell" },
        { id: "mode:ai", label: t("ai: hands them to its agent", "ai：交给 agent"), icon: <Bot className="size-4" />, checked: mode === "ai" },
      );
    }
    setMenu({
      x,
      y,
      title: `${headOf(p)} · ${p.session}:${p.window}.${p.pane}`,
      items,
      onPick: (k) => {
        if (k === "open") onOpen(p.id);
        else if (k === "inbox") onInbox(p.id);
        else if (k === "rename-window") onRename("window", p);
        else if (k === "rename-session") onRename("session", p);
        else onAction(p, k);
      },
    });
  };
  const groupMenu = (level: "session" | "window", p: Pane, x: number, y: number) => {
    const key = level === "session" ? sKey(p) : wKey(p);
    const items: MenuEntry[] = [];
    if (!readOnly)
      items.push({ id: "rename", label: t("Rename", "重命名"), icon: <SquarePen className="size-4" /> });
    items.push(
      folded.has(key)
        ? { id: "toggle", label: t("Unfold", "展开"), icon: <ChevronDown className="size-4" /> }
        : { id: "toggle", label: t("Fold", "折叠"), icon: <ChevronRight className="size-4" /> },
    );
    if (level === "session")
      items.push(
        { id: "fold-all", label: t("Fold every window", "折叠所有窗口"), icon: <ChevronsDownUp className="size-4" /> },
        { id: "open-all", label: t("Unfold every window", "展开所有窗口"), icon: <ChevronsUpDown className="size-4" /> },
      );
    setMenu({
      x,
      y,
      title: level === "session" ? p.session : `${p.window}: ${p.windowName}`,
      items,
      onPick: (k) => {
        if (k === "rename") onRename(level, p);
        else if (k === "toggle") toggle(key);
        else if (k === "fold-all") foldWindows(p.session, true);
        else if (k === "open-all") foldWindows(p.session, false);
      },
    });
  };

  // How much each session and window holds, and the directory most of a
  // session's panes are in (said once on its line; a pane elsewhere says its own).
  const count = useMemo(() => {
    const m = new Map<string, { windows: Set<number>; panes: number; paths: Map<string, number> }>();
    const w = new Map<string, number>();
    for (const p of panes) {
      const s = m.get(sKey(p)) || { windows: new Set(), panes: 0, paths: new Map() };
      s.windows.add(p.window);
      s.panes++;
      if (p.path) s.paths.set(p.path, (s.paths.get(p.path) || 0) + 1);
      m.set(sKey(p), s);
      w.set(wKey(p), (w.get(wKey(p)) || 0) + 1);
    }
    return { s: m, w };
  }, [panes]);
  const home = (p: Pane) => {
    let best = "";
    let most = 0;
    for (const [path, n] of count.s.get(sKey(p))!.paths) if (n > most) [best, most] = [path, n];
    return best;
  };

  if (!panes.length) {
    return (
      <div className="flex flex-col items-center gap-2 px-6 py-16 text-center text-muted">
        <Terminal className="size-8 opacity-60" />
        <p>{t("No panes.", "没有 pane。")}</p>
      </div>
    );
  }

  const out: React.ReactNode[] = [];
  let lastSession: string | null = null;
  let lastWindow: number | null = null;
  for (const p of panes) {
    if (p.session !== lastSession) {
      const c = count.s.get(sKey(p))!;
      const isFolded = folded.has(sKey(p));
      out.push(
        <Group
          key={sKey(p)}
          level="session"
          open={!isFolded}
          onToggle={() => toggle(sKey(p))}
          title={p.session}
          note={home(p)}
          count={isFolded ? t(`${c.windows.size} windows · ${c.panes} panes`, `${c.windows.size} 个窗口 · ${c.panes} 个 pane`) : ""}
          onMenu={(x, y) => groupMenu("session", p, x, y)}
        />,
      );
      lastSession = p.session;
      lastWindow = null;
    }
    if (folded.has(sKey(p))) continue;
    if (p.window !== lastWindow) {
      const isFolded = folded.has(wKey(p));
      const n = count.w.get(wKey(p))!;
      out.push(
        <Group
          key={wKey(p)}
          level="window"
          current={p.windowActive}
          open={!isFolded}
          onToggle={() => toggle(wKey(p))}
          title={`${p.window}: ${p.windowName}`}
          count={isFolded ? t(`${n} panes`, `${n} 个 pane`) : ""}
          onMenu={(x, y) => groupMenu("window", p, x, y)}
        />,
      );
      lastWindow = p.window;
    }
    if (folded.has(wKey(p))) continue;
    out.push(
      <PaneCard
        key={p.id}
        p={p}
        home={home(p)}
        selected={p.id === current}
        here={p.active && p.windowActive}
        onOpen={() => !bounced("open") && onOpen(p.id)}
        onInbox={() => !bounced("inbox") && onInbox(p.id)}
        onMenu={(x, y) => paneMenu(p, x, y)}
      />,
    );
  }
  return (
    <div className="flex flex-col gap-1.5 px-3 pt-2 pb-6">
      {out}
      <ContextMenu at={menu} onClose={() => setMenu(null)} />
    </div>
  );
}

function Group(props: {
  level: "session" | "window";
  title: string;
  note?: string;
  count: string;
  open: boolean;
  current?: boolean;
  onToggle: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const session = props.level === "session";
  const press = useContextPress(props.onMenu);
  const Chev = props.open ? ChevronDown : ChevronRight;
  return (
    <div
      className={
        "group flex min-h-9 items-center gap-1.5 select-none " +
        (session ? "mt-4 first:mt-1" : "mt-1.5 pl-3")
      }
    >
      <button
        type="button"
        {...press}
        onClick={props.onToggle}
        className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg py-1 text-left outline-none focus-visible:ring-2 focus-visible:ring-focus"
        aria-expanded={props.open}
      >
        <Chev className="size-3.5 shrink-0 text-muted" />
        <span
          className={
            "truncate " +
            (session
              ? "text-[13px] font-semibold tracking-wide text-foreground"
              : "text-[13px] " + (props.current ? "font-medium text-foreground" : "text-muted"))
          }
        >
          {props.title}
        </span>
        {props.note ? <span className="truncate text-xs text-muted">{props.note}</span> : null}
        {props.count ? <span className="ml-auto shrink-0 text-xs text-muted">{props.count}</span> : null}
      </button>
    </div>
  );
}

const DOT: Record<string, string> = { success: "bg-success", warning: "bg-warning", danger: "bg-danger" };

/** The list folded away to a strip: a button per pane (its state dot and
 *  the first letters of its name; the whole name on hover and to a screen
 *  reader), a gap between sessions. */
export function PaneRail({ panes, current, onOpen }: { panes: Pane[]; current: string | null; onOpen: (id: string) => void }) {
  let last: string | null = null;
  return (
    <nav aria-label={t("Panes", "pane 列表")} className="flex flex-col items-center gap-1 py-2">
      {panes.map((p) => {
        const gap = last !== null && p.session !== last;
        last = p.session;
        const st = stateOf(p);
        const name = headOf(p);
        return (
          <button
            key={p.id}
            type="button"
            data-rail={p.id}
            title={`${name} · ${p.session}:${p.window}.${p.pane}`}
            aria-label={`${name} · ${p.session}:${p.window}.${p.pane}`}
            aria-current={p.id === current ? "true" : undefined}
            onClick={() => !bounced("open") && onOpen(p.id)}
            className={
              (gap ? "mt-3 " : "") +
              "relative grid size-10 place-items-center rounded-xl text-[11px] font-semibold uppercase outline-none " +
              "focus-visible:ring-2 focus-visible:ring-focus " +
              (p.id === current ? "bg-accent-soft text-accent-soft-foreground" : "text-muted hover:bg-surface-hover")
            }
          >
            {name.slice(0, 2)}
            <span
              className={
                "absolute top-1 right-1 size-1.5 rounded-full " + (st.tone ? DOT[st.tone] : "ring-1 ring-border")
              }
            />
          </button>
        );
      })}
    </nav>
  );
}

function PaneCard({
  p,
  home,
  selected,
  here,
  onOpen,
  onInbox,
  onMenu,
}: {
  p: Pane;
  home: string;
  selected: boolean;
  here: boolean;
  onOpen: () => void;
  onInbox: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const st = stateOf(p);
  const press = useContextPress(onMenu);
  const sub = [...under(p), p.path !== home ? p.path : ""].filter(Boolean).join(" · ");
  const alert = p.activity || p.bell || p.silence;
  return (
    <div
      role="button"
      tabIndex={0}
      data-pane={p.id}
      data-name={p.name || undefined}
      {...press}
      onClick={onOpen}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onOpen())}
      className={
        "ml-3 cursor-pointer rounded-2xl border px-3.5 py-3 text-left transition-colors outline-none " +
        "focus-visible:ring-2 focus-visible:ring-focus " +
        (selected
          ? "border-accent bg-accent-soft"
          : alert
            ? "border-warning/50 bg-surface hover:bg-surface-hover"
            : "border-border bg-surface hover:bg-surface-hover")
      }
    >
      <div className="flex items-center gap-2.5">
        <span
          className={"size-2 shrink-0 rounded-full " + (st.tone ? DOT[st.tone] : "bg-transparent ring-1 ring-border")}
          title={st.label}
        />
        <span className="min-w-0 flex-1 truncate font-medium text-foreground">{headOf(p)}</span>
        {here ? <span className="size-1.5 shrink-0 rounded-full bg-accent" title={t("on the computer's screen", "电脑上正显示")} /> : null}
        {p.working > 0 ? (
          <Chip size="sm" color="success" variant="soft" className="cursor-pointer" onClick={(e) => (e.stopPropagation(), onInbox())}>
            {t(`on #${p.working}`, `处理中 #${p.working}`)}
          </Chip>
        ) : null}
        {p.inbox > 0 ? (
          <Chip size="sm" color="accent" variant="soft" className="cursor-pointer" onClick={(e) => (e.stopPropagation(), onInbox())}>
            {t(`${p.inbox} queued`, `排队 ${p.inbox}`)}
          </Chip>
        ) : null}
        <span className="shrink-0 text-xs tabular-nums text-muted">{quietFor(p.quiet)}</span>
      </div>
      {sub || (p.mode && p.mode !== "normal") ? (
        <div className="mt-1 flex items-center gap-1.5 pl-4.5 text-xs text-muted">
          {p.mode && p.mode !== "normal" ? (
            <span className="rounded-md bg-default px-1.5 py-px font-medium text-default-foreground">{p.mode}</span>
          ) : null}
          <span className="min-w-0 truncate">{sub}</span>
        </div>
      ) : null}
      {p.agent ? <AgentLine a={p.agent} /> : null}
      {p.unheard && p.inbox > 0 ? (
        <div className="mt-1.5 pl-4.5 text-xs text-warning">
          {t(
            "Its agent never said it is free, so these wait: run keepane setup on the computer",
            "agent 从没报告过空闲，消息会一直排队：在电脑上运行 keepane setup",
          )}
        </div>
      ) : null}
      {p.last ? (
        <div className="mt-1.5 truncate pl-4.5 font-term text-xs text-muted">{p.last}</div>
      ) : null}
      {alert ? (
        <div className="mt-2 flex gap-1.5 pl-4.5">
          {p.activity ? <Mark icon={<Terminal className="size-3" />} label={t("printed", "有输出")} /> : null}
          {p.bell ? <Mark icon={<BellRing className="size-3" />} label={t("bell", "响铃")} /> : null}
          {p.silence ? <Mark icon={<Moon className="size-3" />} label={t("quiet", "没动静")} /> : null}
        </div>
      ) : null}
    </div>
  );
}

/** The pane's agent: its model, what it has cost, how full its context is
 *  (a bar when it is a share), the tokens it has used, its CPU. */
function AgentLine({ a }: { a: Agent }) {
  const pct = a.context.endsWith("%") ? Math.min(100, Number(a.context.slice(0, -1)) || 0) : null;
  const tone = pct === null ? "" : pct >= 80 ? "bg-danger" : pct >= 60 ? "bg-warning" : "bg-accent";
  return (
    <div
      data-agent={a.kind}
      className="mt-1.5 flex min-w-0 items-center gap-2 pl-4.5 text-xs tabular-nums text-muted"
      title={t(`${a.kind}: ${a.tokens} tokens, memory ${a.mem}`, `${a.kind}：${a.tokens} tokens，内存 ${a.mem}`)}
    >
      <span className="min-w-0 truncate font-medium text-foreground">{a.model || a.kind}</span>
      {a.cost ? <span className="shrink-0">{a.cost}</span> : null}
      {a.context ? (
        <span className="flex shrink-0 items-center gap-1" aria-label={t(`context ${a.context}`, `上下文 ${a.context}`)}>
          {pct !== null ? (
            <span className="h-1 w-8 overflow-hidden rounded-full bg-default">
              <span className={"block h-full rounded-full " + tone} style={{ width: `${pct}%` }} />
            </span>
          ) : null}
          {a.context}
        </span>
      ) : null}
      <span className="shrink-0">{a.tokens}</span>
      <span className="shrink-0">{t(`cpu ${a.cpu}`, `CPU ${a.cpu}`)}</span>
    </div>
  );
}

function Mark({ icon, label }: { icon: React.ReactNode; label: string }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-full bg-warning-soft px-2 py-0.5 text-[11px] font-medium text-warning-soft-foreground">
      {icon}
      {label}
    </span>
  );
}
