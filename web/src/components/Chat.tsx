import { Button } from "@heroui/react";
import { ArrowLeft, AtSign, Hash, MessagesSquare, Paperclip, SendHorizontal, Users, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { getJson, post, q, saveFile } from "../api";
import { useVisible } from "../hooks";
import { t } from "../i18n";

// The group chat (docs/design/chat.md), as the page shows it: the channels
// like a chat app's list, the one opened beside it (or over it on a phone),
// each one's lines in its colour, the person's on the right, a line to say
// something in at the bottom where a `@` lists the members to name.

/** A channel, as `chat-list -J` gives it. */
export type ChatChannel = {
  id: string;
  name: string;
  topic: string;
  tags: string[];
  members: number;
  unread: number;
  /** The number of its last line (said or what happened). */
  last: number;
  latest?: { at: string; handle: string; text: string } | null;
};
type Ident = { machine?: string; place?: string; pane?: number; agent?: string; model?: string };
/** A member, as `chat-who -J` gives it. */
type Member = {
  handle: string;
  role: string;
  ident: Ident;
  busy: boolean | null;
  online?: boolean;
  /** It left the channel: kept, marked so, not counted. */
  left?: boolean;
  since?: string;
  hear?: string;
};
/** A line, as `chat-read -J` gives it. */
type Line = {
  seq: number;
  at: string;
  kind: string;
  from?: { handle: string; role?: string; ident?: Ident } | null;
  text: string;
  /** Files sent with it: `file` is under the channel's folder of files. */
  files?: { name: string; size: number; file: string }[];
};

/** The members' colours, by the order they joined (as the board's): six
 *  that read on a light page and a dark one. */
const PALETTE = [
  { dot: "bg-cyan-500", text: "text-cyan-600 dark:text-cyan-400", soft: "bg-cyan-500/12 text-cyan-700 dark:text-cyan-300" },
  { dot: "bg-fuchsia-500", text: "text-fuchsia-600 dark:text-fuchsia-400", soft: "bg-fuchsia-500/12 text-fuchsia-700 dark:text-fuchsia-300" },
  { dot: "bg-blue-500", text: "text-blue-600 dark:text-blue-400", soft: "bg-blue-500/12 text-blue-700 dark:text-blue-300" },
  { dot: "bg-emerald-500", text: "text-emerald-600 dark:text-emerald-400", soft: "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300" },
  { dot: "bg-amber-500", text: "text-amber-600 dark:text-amber-400", soft: "bg-amber-500/12 text-amber-700 dark:text-amber-300" },
  { dot: "bg-rose-500", text: "text-rose-600 dark:text-rose-400", soft: "bg-rose-500/12 text-rose-700 dark:text-rose-300" },
];

const isPerson = (h: string) => h.startsWith("you@");
const hash = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);

/** A handle's colour: a member's by when it joined, one who left by its name. */
function colourOf(handle: string, members: Member[]) {
  const i = members.findIndex((m) => m.handle === handle);
  return PALETTE[(i >= 0 ? i : hash(handle)) % PALETTE.length];
}

/** A channel's own colour, for its mark in the list. */
const channelColour = (name: string) => PALETTE[hash(name) % PALETTE.length];

const clock = (at: string) => at.slice(11, 16);
const dayOf = (at: string) => at.slice(0, 10);

/** How long ago, short: now, 5m, 3h, 2d. */
function ago(at: string) {
  const s = Math.max(0, (Date.now() - Date.parse(at)) / 1000);
  if (!Number.isFinite(s)) return "";
  if (s < 60) return t("now", "刚刚");
  if (s < 3600) return t(`${Math.floor(s / 60)}m`, `${Math.floor(s / 60)} 分`);
  if (s < 86400) return t(`${Math.floor(s / 3600)}h`, `${Math.floor(s / 3600)} 小时`);
  return t(`${Math.floor(s / 86400)}d`, `${Math.floor(s / 86400)} 天`);
}

/** A day as its rule says it: today, yesterday, or its date. */
function dayLabel(day: string) {
  const d = new Date(day + "T00:00:00");
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diff = Math.round((today.getTime() - d.getTime()) / 86400000);
  if (diff === 0) return t("Today", "今天");
  if (diff === 1) return t("Yesterday", "昨天");
  return d.toLocaleDateString(t("en", "zh-CN"), { weekday: "short", month: "short", day: "numeric" });
}

/** The handle being typed: what follows the `@` of the text's last word. */
const naming = (text: string) => text.match(/(?:^|\s)@([\p{L}\p{N}_-]*)$/u)?.[1];

/** The channels, asked for every two seconds while the chat is open. */
function useChannels(enabled: boolean) {
  const visible = useVisible();
  const [channels, setChannels] = useState<ChatChannel[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(async () => {
    try {
      setChannels(await getJson<ChatChannel[]>("/api/chat/channels"));
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);
  useEffect(() => {
    if (!visible || !enabled) return;
    let stop = false;
    let timer: number | undefined;
    const loop = async () => {
      await reload();
      if (!stop) timer = window.setTimeout(loop, 2000);
    };
    loop();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [visible, enabled, reload]);
  return { channels, error, reload };
}

/** The unread lines of every channel together, for the header's badge,
 *  asked for every five seconds while the page is shown. */
export function useChatUnread(enabled: boolean) {
  const visible = useVisible();
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!visible || !enabled) return;
    let stop = false;
    let timer: number | undefined;
    const loop = async () => {
      try {
        const cs = await getJson<ChatChannel[]>("/api/chat/channels");
        setN(cs.reduce((s, c) => s + (c.unread || 0), 0));
      } catch {
        // An older server has no chat: no badge.
      }
      if (!stop) timer = window.setTimeout(loop, 5000);
    };
    loop();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [visible, enabled]);
  return n;
}

export function ChatView({ wide, readOnly, onClose }: { wide: boolean; readOnly: boolean; onClose: () => void }) {
  const { channels: listed, error, reload } = useChannels(true);
  // As a chat app lists them: the one last said in first, those where
  // nothing was said yet after them, by name.
  const channels = useMemo(
    () =>
      listed
        ? [...listed].sort(
            (a, b) =>
              (b.latest?.at ? Date.parse(b.latest.at) : 0) - (a.latest?.at ? Date.parse(a.latest.at) : 0) ||
              a.name.localeCompare(b.name),
          )
        : null,
    [listed],
  );
  const [open, setOpen] = useState<string | null>(null);
  const picked = channels?.find((c) => c.id === open) || null;
  // On a wide screen a channel is always open: the first, until one is picked.
  useEffect(() => {
    if (wide && !open && channels && channels.length) setOpen(channels[0].id);
  }, [wide, open, channels]);
  const showList = wide || !picked;
  return (
    <div className="flex min-h-0 flex-1">
      {showList ? (
        <aside
          className={
            "thin-scroll flex min-h-0 flex-col overflow-y-auto " +
            (wide ? "w-[320px] shrink-0 border-r border-separator" : "flex-1")
          }
          aria-label={t("Channels", "频道")}
        >
          <div className="flex items-center gap-2 px-4 pt-3 pb-2">
            <MessagesSquare className="size-4.5 text-muted" />
            <h2 className="flex-1 text-[15px] font-semibold">{t("Group chat", "群聊")}</h2>
            <Button isIconOnly variant="ghost" size="sm" aria-label={t("Back to the panes", "回到 pane 列表")} onPress={onClose}>
              <X className="size-4.5" />
            </Button>
          </div>
          {error ? <p className="mx-4 rounded-xl bg-danger-soft p-3 text-sm text-danger-soft-foreground">{error}</p> : null}
          {channels && channels.length === 0 ? (
            <p className="px-4 py-6 text-sm text-muted">{t("No channel yet.", "还没有频道。")}</p>
          ) : null}
          <ul className="flex flex-col gap-0.5 px-2 pb-3">
            {(channels || []).map((c) => (
              <ChannelRow key={c.id} c={c} active={c.id === open} onOpen={() => setOpen(c.id)} />
            ))}
          </ul>
        </aside>
      ) : null}
      {picked ? (
        <Conversation
          key={picked.id}
          channel={picked}
          wide={wide}
          readOnly={readOnly}
          onBack={() => setOpen(null)}
          onSaid={reload}
        />
      ) : wide ? (
        <div className="flex flex-1 items-center justify-center text-sm text-muted">
          {channels ? t("Pick a channel.", "选一个频道。") : null}
        </div>
      ) : null}
    </div>
  );
}

function ChannelRow({ c, active, onOpen }: { c: ChatChannel; active: boolean; onOpen: () => void }) {
  const colour = channelColour(c.name);
  const latest = c.latest && c.latest.handle ? c.latest : null;
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        aria-current={active ? "true" : undefined}
        aria-label={`#${c.name}`}
        className={
          "flex w-full items-center gap-3 rounded-2xl px-2.5 py-2.5 text-left transition-colors " +
          (active ? "bg-surface-secondary" : "hover:bg-surface")
        }
      >
        <span className={"grid size-10 shrink-0 place-items-center rounded-2xl text-white shadow-sm " + colour.dot}>
          <Hash className="size-4.5" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span className={"truncate text-[15px] " + (c.unread ? "font-semibold" : "font-medium")}>{c.name}</span>
            <span className="ml-auto shrink-0 text-xs text-muted">{latest ? ago(latest.at) : ""}</span>
          </span>
          <span className="flex items-center gap-2">
            <span className="min-w-0 flex-1 truncate text-[13px] text-muted">
              {latest ? (
                <>
                  <span className="text-foreground/70">{isPerson(latest.handle) ? t("You", "你") : latest.handle}: </span>
                  {latest.text}
                </>
              ) : (
                c.topic || t(`${c.members} members`, `${c.members} 个成员`)
              )}
            </span>
            {c.unread ? (
              <span
                aria-label={t(`${c.unread} unread`, `${c.unread} 条未读`)}
                className="grid h-5 min-w-5 shrink-0 place-items-center rounded-full bg-accent px-1.5 text-[11px] font-semibold text-accent-foreground"
              >
                {c.unread > 99 ? "99+" : c.unread}
              </span>
            ) : null}
          </span>
        </span>
      </button>
    </li>
  );
}

/** A run of one member's lines close in time: one header, many bubbles. */
type Group = { handle: string; role: string; at: string; person: boolean; lines: Line[] };
type Item =
  | { kind: "day"; label: string; key: string }
  | { kind: "new"; count: number; key: string }
  | { kind: "event"; line: Line; key: string }
  | { kind: "group"; group: Group; key: string };

function itemsOf(lines: Line[], divider: number | null) {
  const out: Item[] = [];
  let day = "";
  let open: Group | null = null;
  const close = () => {
    if (open) out.push({ kind: "group", group: open, key: "g" + open.lines[0].seq });
    open = null;
  };
  const fresh = divider === null ? 0 : lines.filter((l) => l.kind === "say" && l.seq >= divider && !isPerson(l.from?.handle || "")).length;
  for (const l of lines) {
    const d = dayOf(l.at);
    if (d && d !== day) {
      close();
      day = d;
      out.push({ kind: "day", label: dayLabel(d), key: "d" + d });
    }
    if (divider !== null && l.seq === divider) {
      close();
      out.push({ kind: "new", count: fresh, key: "n" + l.seq });
    }
    if (l.kind !== "say") {
      close();
      out.push({ kind: "event", line: l, key: "e" + l.seq });
      continue;
    }
    const handle = l.from?.handle || "?";
    const last = open as Group | null;
    if (last && last.handle === handle && Date.parse(l.at) - Date.parse(last.lines[last.lines.length - 1].at) <= 5 * 60000) {
      last.lines.push(l);
    } else {
      close();
      open = { handle, role: l.from?.role || "", at: l.at, person: isPerson(handle), lines: [l] };
    }
  }
  close();
  return out;
}

function Conversation({
  channel,
  wide,
  readOnly,
  onBack,
  onSaid,
}: {
  channel: ChatChannel;
  wide: boolean;
  readOnly: boolean;
  onBack: () => void;
  onSaid: () => void;
}) {
  const visible = useVisible();
  const [lines, setLines] = useState<Line[] | null>(null);
  const [members, setMembers] = useState<Member[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [showMembers, setShowMembers] = useState(false);
  // Where what the person had not read began, kept while the channel is
  // open (showing it reads it at once).
  const [divider, setDivider] = useState<number | null>(null);
  const unreadAtOpen = useRef(channel.unread);
  const first = useRef(true);
  // The lines: read again only when the channel's last line is another
  // (the list, asked every two seconds, says its number): a channel where
  // nothing happens costs its members' list, not its last 300 lines.
  const loadLines = useCallback(async () => {
    try {
      const r = await getJson<{ lines: Line[] }>(`/api/chat/lines?c=${q(channel.id)}&n=300`);
      setLines(r.lines);
      setError(null);
      if (first.current) {
        first.current = false;
        const n = unreadAtOpen.current;
        if (n > 0) {
          const said = r.lines.filter((l) => l.kind === "say" && !isPerson(l.from?.handle || ""));
          const at = said[Math.max(0, said.length - n)];
          if (at) setDivider(at.seq);
        }
      }
    } catch (e) {
      setError((e as Error).message);
    }
  }, [channel.id]);
  useEffect(() => {
    if (visible) loadLines();
  }, [visible, loadLines, channel.last]);
  // Who is there (and online, busy): small, asked every two seconds.
  useEffect(() => {
    if (!visible) return;
    let stop = false;
    let timer: number | undefined;
    const loop = async () => {
      try {
        setMembers(await getJson<Member[]>(`/api/chat/who?c=${q(channel.id)}`));
      } catch {
        // Said by the lines' request already.
      }
      if (!stop) timer = window.setTimeout(loop, 2000);
    };
    loop();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [visible, channel.id]);
  const load = loadLines;
  // Shown: read (the person's place moves to its end).
  useEffect(() => {
    if (!readOnly && visible && channel.unread > 0) post(`/api/chat/seen?c=${q(channel.id)}`).catch(() => {});
  }, [readOnly, visible, channel.id, channel.unread, lines]);

  const items = useMemo(() => itemsOf(lines || [], divider), [lines, divider]);
  const staying = members.filter((m) => !m.left);
  const online = staying.filter((m) => m.online !== false).length;

  // The latest in view: at the bottom while the person is there.
  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label={`#${channel.name}`}>
      <header className="flex items-center gap-2 border-b border-separator px-3 py-2">
        {!wide ? (
          <Button isIconOnly variant="ghost" size="sm" className="pointer-coarse:size-11" aria-label={t("Channels", "频道")} onPress={onBack}>
            <ArrowLeft className="size-4.5" />
          </Button>
        ) : null}
        <span className={"grid size-8 shrink-0 place-items-center rounded-xl text-white " + channelColour(channel.name).dot}>
          <Hash className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[15px] leading-tight font-semibold">{channel.name}</div>
          {channel.topic ? <div className="truncate text-xs text-muted">{channel.topic}</div> : null}
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="gap-1.5 pointer-coarse:h-11"
          aria-label={t("Members", "成员")}
          aria-expanded={showMembers}
          onPress={() => setShowMembers(!showMembers)}
        >
          <Users className="size-4" />
          <span className="tabular-nums">
            {online}
            {online !== staying.length ? <span className="text-muted">/{staying.length}</span> : null}
          </span>
        </Button>
      </header>
      <div className="relative flex min-h-0 flex-1">
        <div
          ref={scroller}
          className="thin-scroll min-h-0 flex-1 overflow-y-auto px-3 py-4 sm:px-5"
          onScroll={(e) => {
            const el = e.currentTarget;
            atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
          }}
        >
          {error ? <p className="rounded-xl bg-danger-soft p-3 text-sm text-danger-soft-foreground">{error}</p> : null}
          {lines && lines.length === 0 ? <p className="py-10 text-center text-sm text-muted">{t("Nothing said yet.", "还没有人说话。")}</p> : null}
          <ol className="mx-auto flex max-w-3xl flex-col gap-3">
            {items.map((it) => (
              <li key={it.key}>
                {it.kind === "day" ? (
                  <div className="flex justify-center py-1">
                    <span className="rounded-full bg-surface px-3 py-0.5 text-xs text-muted">{it.label}</span>
                  </div>
                ) : it.kind === "new" ? (
                  <div className="flex items-center gap-3 py-1 text-xs font-medium text-accent" role="separator">
                    <span className="h-px flex-1 bg-accent/40" />
                    {t(`${it.count} new`, `${it.count} 条新消息`)}
                    <span className="h-px flex-1 bg-accent/40" />
                  </div>
                ) : it.kind === "event" ? (
                  <p className="text-center text-xs text-muted">
                    {clock(it.line.at)} · {it.line.from?.handle ? it.line.from.handle + " " : ""}
                    {it.line.text}
                  </p>
                ) : (
                  <Bubbles group={it.group} members={members} channel={channel.id} />
                )}
              </li>
            ))}
          </ol>
        </div>
        {showMembers ? <MemberList members={members} wide={wide} onClose={() => setShowMembers(false)} /> : null}
      </div>
      {readOnly ? (
        <p className="border-t border-separator px-4 py-3 text-center text-xs text-muted">{t("Read-only: watching only.", "只读：只能看。")}</p>
      ) : (
        <Composer channel={channel} members={members} onSaid={() => { atBottom.current = true; load(); onSaid(); }} />
      )}
    </section>
  );
}

/** What was said, with each `@name` of a member (or `@you`) as a chip in
 *  that one's colour. */
function Said({ text, members }: { text: string; members: Member[] }) {
  const parts = text.split(/(@[\p{L}\p{N}_-]+)/u);
  return (
    <>
      {parts.map((p, i) => {
        const h = p.startsWith("@") ? p.slice(1) : null;
        if (h === "you") {
          return (
            <span key={i} className="rounded-md bg-accent/15 px-1 font-semibold text-accent">
              {p}
            </span>
          );
        }
        if (h && members.some((m) => m.handle === h)) {
          return (
            <span key={i} className={"rounded-md px-1 font-semibold " + colourOf(h, members).soft}>
              {p}
            </span>
          );
        }
        return <span key={i}>{p}</span>;
      })}
    </>
  );
}

/** A size as people read it: `512 B`, `1.5 KB`, `3.2 MB`. */
const sizeText = (b: number) =>
  b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MB` : b >= 1024 ? `${(b / 1024).toFixed(1)} KB` : `${b} B`;

/** The files sent with a line, each a button that saves it. */
function Files({ line, channel }: { line: Line; channel: string }) {
  const [failed, setFailed] = useState("");
  if (!line.files?.length) return null;
  return (
    <span className={"flex flex-col gap-1 " + (line.text ? "mt-1.5" : "")}>
      {line.files.map((f) => (
        <button
          key={f.file}
          type="button"
          className="flex items-center gap-2 rounded-xl border border-separator bg-background/60 px-2.5 py-1.5 text-left text-[13px] text-foreground hover:bg-background pointer-coarse:min-h-11"
          aria-label={t(`Save ${f.name}`, `保存 ${f.name}`)}
          onClick={() => {
            setFailed("");
            saveFile(`/api/chat/file?c=${q(channel)}&f=${q(f.file)}`, f.name).catch((e: Error) => setFailed(e.message));
          }}
        >
          <Paperclip className="size-4 shrink-0 text-muted" aria-hidden />
          <span className="min-w-0 flex-1 truncate">{f.name}</span>
          <span className="shrink-0 text-xs text-muted">{sizeText(f.size)}</span>
        </button>
      ))}
      {failed ? <span className="text-xs text-danger">{failed}</span> : null}
    </span>
  );
}

function Bubbles({ group, members, channel }: { group: Group; members: Member[]; channel: string }) {
  const colour = colourOf(group.handle, members);
  // A line that names the person: ringed in the accent, that line only.
  const calls = (l: Line) => !group.person && /(^|\s)@you\b/.test(l.text);
  if (group.person) {
    return (
      <div className="flex flex-col items-end gap-1">
        <span className="px-1 text-[11px] text-muted">{clock(group.at)}</span>
        {group.lines.map((l) => (
          <div
            key={l.seq}
            className="max-w-[85%] rounded-2xl rounded-br-md bg-accent px-3.5 py-2 text-[14px] leading-relaxed whitespace-pre-wrap break-words text-accent-foreground shadow-sm sm:max-w-[70%]"
          >
            {l.text}
            <Files line={l} channel={channel} />
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="flex gap-2.5">
      <span className={"mt-0.5 grid size-8 shrink-0 place-items-center rounded-full text-sm font-semibold text-white " + colour.dot} aria-hidden>
        {group.handle.slice(0, 1).toUpperCase()}
      </span>
      <div className="flex min-w-0 flex-col items-start gap-1">
        <div className="flex items-baseline gap-2 px-1">
          <span className={"text-[13px] font-semibold " + colour.text}>{group.handle}</span>
          {group.role ? <span className="text-xs text-muted">{group.role}</span> : null}
          <span className="text-[11px] text-muted">{clock(group.at)}</span>
        </div>
        {group.lines.map((l) => (
          <div
            key={l.seq}
            className={
              "max-w-full rounded-2xl rounded-tl-md bg-surface px-3.5 py-2 text-[14px] leading-relaxed whitespace-pre-wrap break-words shadow-sm sm:max-w-[42rem] " +
              (calls(l) ? "ring-2 ring-accent/50" : "")
            }
          >
            <Said text={l.text} members={members} />
            <Files line={l} channel={channel} />
          </div>
        ))}
      </div>
    </div>
  );
}

function MemberList({ members, wide, onClose }: { members: Member[]; wide: boolean; onClose: () => void }) {
  // Online first, then offline, then those that left.
  const rank = (m: Member) => (m.left ? 2 : m.online === false ? 1 : 0);
  const sorted = [...members].sort((a, b) => rank(a) - rank(b));
  return (
    <aside
      className={
        "thin-scroll min-h-0 overflow-y-auto border-l border-separator bg-background " +
        (wide ? "w-[260px] shrink-0" : "absolute inset-x-0 bottom-0 top-auto z-20 max-h-[60%] rounded-t-3xl border-t shadow-2xl")
      }
      aria-label={t("Members", "成员")}
    >
      <div className="flex items-center gap-2 px-4 pt-3 pb-1">
        <h3 className="flex-1 text-sm font-semibold">{t("Members", "成员")}</h3>
        <Button isIconOnly variant="ghost" size="sm" aria-label={t("Close", "关闭")} onPress={onClose}>
          <X className="size-4" />
        </Button>
      </div>
      {sorted.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted">{t("No one has joined yet (keepane chat join, from a pane).", "还没有人加入（在 pane 里运行 keepane chat join）。")}</p>
      ) : null}
      <ul className="flex flex-col gap-0.5 px-2 pb-3">
        {sorted.map((m) => {
          const on = m.online !== false && !m.left;
          const colour = colourOf(m.handle, members);
          return (
            <li key={m.handle} className={"flex items-center gap-3 rounded-xl px-2 py-2 " + (on ? "" : "opacity-55")}>
              <span className="relative">
                <span className={"grid size-8 place-items-center rounded-full text-sm font-semibold text-white " + colour.dot}>
                  {m.handle.slice(0, 1).toUpperCase()}
                </span>
                <span
                  className={
                    "absolute -right-0.5 -bottom-0.5 size-3 rounded-full border-2 border-background " +
                    (!on ? "bg-zinc-400" : m.busy ? "bg-amber-500" : "bg-emerald-500")
                  }
                  title={m.left ? t("left", "已离开") : !on ? t("offline", "离线") : m.busy ? t("busy", "忙") : t("idle", "空闲")}
                />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-baseline gap-1.5">
                  <span className="truncate text-sm font-medium">{m.handle}</span>
                  {m.role ? <span className="truncate text-xs text-muted">{m.role}</span> : null}
                </span>
                <span className="block truncate text-xs text-muted">
                  {m.left
                    ? t("left the channel", "已离开")
                    : on
                      ? [m.ident.place, m.ident.agent].filter(Boolean).join(" · ")
                      : t(`offline · was ${m.ident.place || "?"}`, `离线 · 原在 ${m.ident.place || "?"}`)}
                  {m.hear === "all" ? t(" · hears all", " · 收听全部") : ""}
                </span>
              </span>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}

function Composer({ channel, members, onSaid }: { channel: ChatChannel; members: Member[]; onSaid: () => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [pick, setPick] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const typed = naming(text);
  const list = useMemo(() => {
    if (typed === undefined) return [];
    const p = typed.toLowerCase();
    return members
      .filter((m) => !m.left && m.handle.toLowerCase().startsWith(p))
      .sort((a, b) => Number(b.online !== false) - Number(a.online !== false))
      .slice(0, 6);
  }, [typed, members]);
  const at = Math.min(pick, Math.max(0, list.length - 1));
  // The box grows with what is typed, up to six lines.
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 6 * 22 + 18) + "px";
  }, [text]);
  const complete = (m: Member) => {
    setText(text.replace(/@[\p{L}\p{N}_-]*$/u, "@" + m.handle + " "));
    setPick(0);
    area.current?.focus();
  };
  const send = async () => {
    if (!text.trim() || busy) return;
    setBusy(true);
    try {
      await post(`/api/chat/say?c=${q(channel.id)}`, text);
      setText("");
      setError(null);
      onSaid();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="relative border-t border-separator px-3 pt-2 pb-[calc(env(safe-area-inset-bottom)+8px)]">
      {list.length ? (
        <ul
          className="absolute bottom-full left-1/2 mb-2 w-[min(48rem,calc(100%-24px))] -translate-x-1/2 overflow-hidden rounded-2xl border border-separator bg-overlay shadow-xl"
          role="listbox"
          aria-label={t("Members to name", "可以点名的成员")}
        >
          {list.map((m, i) => (
            <li key={m.handle} role="option" aria-selected={i === at}>
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => complete(m)}
                className={"flex w-full items-center gap-2.5 px-3 py-2 text-left " + (i === at ? "bg-surface-secondary" : "hover:bg-surface")}
              >
                <span className={"grid size-6 place-items-center rounded-full text-xs font-semibold text-white " + colourOf(m.handle, members).dot}>
                  {m.handle.slice(0, 1).toUpperCase()}
                </span>
                <span className="text-sm font-medium">@{m.handle}</span>
                {m.role ? <span className="text-xs text-muted">{m.role}</span> : null}
                {m.online === false ? <span className="ml-auto text-xs text-muted">{t("offline", "离线")}</span> : null}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {error ? <p className="mb-2 text-xs text-danger">{error}</p> : null}
      <div className="mx-auto flex max-w-3xl items-end gap-2 rounded-3xl border border-separator bg-surface px-3 py-1.5 focus-within:border-accent">
        <AtSign className="mb-2 size-4 shrink-0 text-muted" aria-hidden />
        <textarea
          ref={area}
          rows={1}
          value={text}
          placeholder={t(`Say something in #${channel.name}`, `在 #${channel.name} 里说点什么`)}
          aria-label={t(`Say something in #${channel.name}`, `在 #${channel.name} 里说点什么`)}
          className="thin-scroll max-h-[150px] min-h-[36px] flex-1 resize-none bg-transparent py-1.5 text-[15px] leading-[22px] outline-none"
          onChange={(e) => {
            setText(e.target.value);
            setPick(0);
          }}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (list.length) {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setPick((at + 1) % list.length);
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                setPick((at + list.length - 1) % list.length);
                return;
              }
              if (e.key === "Tab" || e.key === "Enter") {
                e.preventDefault();
                complete(list[at]);
                return;
              }
            }
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <Button
          isIconOnly
          size="sm"
          className="mb-0.5 rounded-full pointer-coarse:size-10"
          aria-label={t("Send", "发送")}
          isDisabled={!text.trim() || busy}
          onPress={send}
        >
          <SendHorizontal className="size-4" />
        </Button>
      </div>
    </div>
  );
}
