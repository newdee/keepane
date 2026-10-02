import type { Mark, Pane } from "./api";
import { t } from "./i18n";

/** How long since a pane printed: "5s", "3m", "2h", "4d". */
export function quietFor(s: number | null | undefined) {
  if (s == null || isNaN(s)) return "";
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
}

/** The title a pane's program set, when it says more than the program's
 *  name: a shell that sets its own path as the title, or no title at all,
 *  says nothing new. */
export function titleOf(p: Pane) {
  const title = (p.title || "").trim();
  const base = title.split(/[\\/]/).pop()!.replace(/\.exe$/i, "").toLowerCase();
  return !title || base === (p.command || "").toLowerCase() ? "" : title;
}

export type State = { tone: "success" | "warning" | "danger" | null; label: string };

/** A pane's state, as the dashboard gives it: an agent or shell pane is free
 *  or busy; a normal pane is neither; a pane whose program ended has exited. */
export function stateOf(p: Pane): State {
  if (p.dead) return { tone: "danger", label: t("exited", "已退出") };
  if (!p.mode || p.mode === "normal") return { tone: null, label: "" };
  if (p.unheard) return { tone: "warning", label: t("never said it is free", "没报告过空闲") };
  return p.idle ? { tone: "success", label: t("free", "空闲") } : { tone: "warning", label: t("busy", "忙") };
}

/** A pane's heading: its name, else its program's title, else the program. */
export const headOf = (p: Pane) => p.name || titleOf(p) || p.command || "?";

/** What the heading left out: the title when the name took the heading, and
 *  the program when either did. */
export function under(p: Pane) {
  const title = titleOf(p);
  return [p.name && title !== p.name ? title : "", p.name || title ? p.command || "?" : ""].filter(Boolean);
}

export const where = (p: Pane) => `${p.session}:${p.window}.${p.pane}`;

const pad2 = (n: number) => String(n).padStart(2, "0");

/** A command's time in the screen's left column: the time today, else the date. */
export function stampText(m: Mark) {
  const d = new Date((m[1] ?? m[2])!);
  const now = new Date();
  return d.toDateString() === now.toDateString()
    ? `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
    : `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function took(ms: number) {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 10000) return `${(ms / 1000).toFixed(1)}s`;
  if (ms < 60000) return `${Math.floor(ms / 1000)}s`;
  if (ms < 3600000) return `${Math.floor(ms / 60000)}m${pad2(Math.floor(ms / 1000) % 60)}s`;
  return `${Math.floor(ms / 3600000)}h${pad2(Math.floor(ms / 60000) % 60)}m`;
}

/** All of it, for a tap on the time. */
export function stampDetail(m: Mark) {
  const [, start, end, exit] = m;
  const d = new Date((start ?? end)!);
  let s =
    `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
    `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  if (start != null && end != null) s += ` · ${took(end - start)}`;
  if (exit === 0) s += " · ✓";
  else if (exit != null) s += ` · ✗ ${t("exit", "退出码")} ${exit}`;
  return s;
}

/** A tap is one press: the same control again within 250 ms is a finger
 *  bouncing or a tap reported twice. */
const lastTap = new Map<string, number>();
export function bounced(what: string) {
  const now = Date.now();
  const last = lastTap.get(what) || 0;
  lastTap.set(what, now);
  return now - last < 250;
}

/** A value kept on this device (localStorage), read once. */
export function stored<T>(name: string, fallback: T): T {
  try {
    const v = localStorage.getItem(name);
    return v == null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}
export function store(name: string, value: unknown) {
  try {
    localStorage.setItem(name, JSON.stringify(value));
  } catch {
    /* a private window: kept for this visit only */
  }
}
