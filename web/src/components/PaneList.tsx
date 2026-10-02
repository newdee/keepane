import { Button, Chip } from "@heroui/react";
import { BellRing, ChevronDown, ChevronRight, Moon, PenLine, Terminal } from "lucide-react";
import { useMemo } from "react";
import type { Pane } from "../api";
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
};

/** Every pane, as keepane holds them: each session, its windows, their panes
 *  (`list-panes -a` gives them in that order). Folded sessions and windows
 *  are kept on this device. */
export function PaneList({ panes, current, readOnly, onOpen, onRename, onInbox }: Props) {
  const [foldedList, setFolded] = useStored<string[]>("keepane-folded", []);
  const folded = useMemo(() => new Set(foldedList), [foldedList]);
  const toggle = (k: string) => {
    if (bounced("fold " + k)) return;
    const next = new Set(folded);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    setFolded([...next]);
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
          onRename={readOnly ? undefined : () => onRename("session", p)}
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
          onRename={readOnly ? undefined : () => onRename("window", p)}
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
      />,
    );
  }
  return <div className="flex flex-col gap-1.5 px-3 pt-2 pb-6">{out}</div>;
}

function Group(props: {
  level: "session" | "window";
  title: string;
  note?: string;
  count: string;
  open: boolean;
  current?: boolean;
  onToggle: () => void;
  onRename?: () => void;
}) {
  const session = props.level === "session";
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
      {props.onRename ? (
        <Button
          isIconOnly
          size="sm"
          variant="ghost"
          aria-label={t("Rename", "重命名")}
          className="text-muted opacity-60 group-hover:opacity-100"
          onPress={props.onRename}
        >
          <PenLine className="size-3.5" />
        </Button>
      ) : null}
    </div>
  );
}

const DOT: Record<string, string> = { success: "bg-success", warning: "bg-warning", danger: "bg-danger" };

function PaneCard({
  p,
  home,
  selected,
  here,
  onOpen,
  onInbox,
}: {
  p: Pane;
  home: string;
  selected: boolean;
  here: boolean;
  onOpen: () => void;
  onInbox: () => void;
}) {
  const st = stateOf(p);
  const sub = [...under(p), p.path !== home ? p.path : ""].filter(Boolean).join(" · ");
  const alert = p.activity || p.bell || p.silence;
  return (
    <div
      role="button"
      tabIndex={0}
      data-pane={p.id}
      data-name={p.name || undefined}
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

function Mark({ icon, label }: { icon: React.ReactNode; label: string }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-full bg-warning-soft px-2 py-0.5 text-[11px] font-medium text-warning-soft-foreground">
      {icon}
      {label}
    </span>
  );
}
