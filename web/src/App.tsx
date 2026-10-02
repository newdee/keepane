import { Button, Chip, Dropdown, Header, Label, Separator, Toast, toast } from "@heroui/react";
import { Check, Laptop, Moon, MousePointerClick, Palette, Sun, Terminal } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { getJson, post, q, startPane, type DoneItem, type Info, type Pane } from "./api";
import { useDone, useMedia, usePageMode, usePanes, useTermTheme, useViewportHeight, type Mode } from "./hooks";
import { t } from "./i18n";
import { PaneList } from "./components/PaneList";
import { PaneView } from "./components/PaneView";
import { InboxSheet, RenameDialog, Switcher } from "./components/Sheets";

const THEME_NAMES: Record<string, string> = { "tokyo-night": "Tokyo Night", "tokyo-day": "Tokyo Day" };

export default function App() {
  useViewportHeight();
  const wide = useMedia("(min-width: 960px)");
  const [info, setInfo] = useState<Info | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  const { panes, error, reload } = usePanes(!!info);
  const term = useTermTheme();
  const page = usePageMode();
  const [switcher, setSwitcher] = useState(false);
  const [inbox, setInbox] = useState<string | null>(null);
  const [ask, setAsk] = useState<{ title: string; value: string; kind: "session" | "window"; pane: string } | null>(null);
  const [done, setDone] = useState<{ item: DoneItem; more: number } | null>(null);
  const unseen = useRef(0);

  // Going from pane to pane: Back (the phone's, or the page's) goes to the list.
  const open = useCallback((id: string, push = true) => {
    setCurrent(id);
    if (push) history.pushState({ pane: id }, "");
  }, []);
  const go = useCallback((id: string) => {
    setCurrent(id);
    history.replaceState({ pane: id }, "");
  }, []);
  useEffect(() => {
    const on = (e: PopStateEvent) => setCurrent(e.state && e.state.pane ? e.state.pane : null);
    addEventListener("popstate", on);
    return () => removeEventListener("popstate", on);
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const i = await getJson<Info>("/api/info");
        setInfo(i);
        document.title = `keepane · ${i.host}`;
        history.replaceState({}, "");
        if (startPane) open(startPane);
      } catch (e) {
        setFatal((e as Error).message);
      }
    })();
  }, [open]);

  // A pane done (`done-events` on the computer): a banner to tap, a short
  // sound and a buzz, and the count in the tab's title until the page is touched.
  const audio = useRef<AudioContext | null>(null);
  useEffect(() => {
    const on = () => {
      unseen.current = 0;
      if (info) document.title = `keepane · ${info.host}`;
      if (!audio.current) {
        try {
          const A = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
          audio.current = new A();
        } catch {
          /* no sound here */
        }
      }
    };
    document.addEventListener("pointerdown", on, { passive: true });
    return () => document.removeEventListener("pointerdown", on);
  }, [info]);
  useDone((d) => {
    const last = d.done[d.done.length - 1];
    setDone({ item: last, more: d.done.length - 1 });
    const a = audio.current;
    if (a) {
      try {
        const o = a.createOscillator();
        const g = a.createGain();
        o.frequency.value = 880;
        g.gain.value = 0.06;
        o.connect(g);
        g.connect(a.destination);
        o.start();
        o.stop(a.currentTime + 0.15);
      } catch {
        /* no sound */
      }
    }
    navigator.vibrate?.([120, 60, 120]);
    unseen.current += d.done.length;
    if (info) document.title = `(${unseen.current}) keepane · ${info.host}`;
  });
  useEffect(() => {
    if (!done) return;
    const id = window.setTimeout(() => setDone(null), 8000);
    return () => clearTimeout(id);
  }, [done]);

  const rename = (kind: "session" | "window", p: Pane) =>
    setAsk({
      kind,
      pane: p.id,
      value: kind === "session" ? p.session : p.windowName,
      title: kind === "session" ? t("Rename the session", "重命名 session") : t("Rename the window", "重命名窗口"),
    });
  const renamed = async (name: string | null) => {
    const a = ask;
    setAsk(null);
    if (!a || name === null || name === a.value || !name) return;
    try {
      await post(`/api/action?pane=${q(a.pane)}&do=rename-${a.kind}`, name);
      await reload();
      toast.success(t("Renamed", "已重命名"));
    } catch (e) {
      toast.danger((e as Error).message);
    }
  };

  const list = panes ?? [];
  const showList = wide || !current;
  const message = fatal || error;

  return (
    <div className="flex h-full flex-col bg-background text-foreground">
      <Toast.Provider placement={wide ? "bottom end" : "top"} />
      {showList || wide ? (
        <header className="flex items-center gap-2 border-b border-separator px-3 pt-[calc(env(safe-area-inset-top)+8px)] pb-2">
          <span className="grid size-8 place-items-center rounded-xl bg-accent text-accent-foreground shadow-sm">
            <Terminal className="size-4.5" />
          </span>
          <div className="min-w-0 flex-1">
            <div className="text-[15px] leading-tight font-semibold">keepane</div>
            <div className="truncate text-xs text-muted">{info ? info.host : " "}</div>
          </div>
          {info?.readOnly ? (
            <Chip size="sm" variant="soft" color="warning">
              {t("read-only", "只读")}
            </Chip>
          ) : null}
          <Settings
            mode={page.mode}
            setMode={page.setMode}
            themes={term.theme.names}
            theme={term.theme.name}
            readOnly={!!info?.readOnly}
            setTheme={async (n) => {
              try {
                await term.set(n);
                toast.success(t(`Terminal theme: ${THEME_NAMES[n] || n}`, `终端主题：${THEME_NAMES[n] || n}`));
              } catch (e) {
                toast.danger((e as Error).message);
              }
            }}
          />
        </header>
      ) : null}

      <div className="flex min-h-0 flex-1">
        {showList ? (
          <aside className={"thin-scroll min-h-0 overflow-y-auto " + (wide ? "w-[360px] shrink-0 border-r border-separator" : "flex-1")}>
            {message ? (
              <p className="m-4 rounded-xl bg-danger-soft p-3 text-sm text-danger-soft-foreground">{message}</p>
            ) : null}
            {panes ? (
              <PaneList
                panes={list}
                current={current}
                readOnly={!!info?.readOnly}
                onOpen={(id) => (wide && current ? go(id) : open(id))}
                onRename={rename}
                onInbox={setInbox}
              />
            ) : !message ? (
              <div className="flex flex-col gap-2 p-4">
                {[0, 1, 2].map((i) => (
                  <div key={i} className="h-16 animate-pulse rounded-2xl bg-surface" />
                ))}
              </div>
            ) : null}
          </aside>
        ) : null}
        {current && info ? (
          <PaneView
            key={current}
            id={current}
            panes={list}
            readOnly={info.readOnly}
            theme={term.theme}
            wide={wide}
            onBack={() => (history.state && history.state.pane ? history.back() : setCurrent(null))}
            onSwitcher={() => setSwitcher(true)}
            onGo={go}
            onMade={(id) => open(id)}
            reloadPanes={reload}
            onRename={rename}
            onInbox={setInbox}
          />
        ) : wide ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center text-muted">
            <span className="grid size-14 place-items-center rounded-2xl bg-surface">
              <MousePointerClick className="size-6" />
            </span>
            <p className="text-sm">{t("Pick a pane on the left.", "在左边选一个 pane。")}</p>
          </div>
        ) : null}
      </div>

      {done ? (
        <button
          type="button"
          className="drop-in fixed top-[calc(env(safe-area-inset-top)+12px)] left-1/2 z-50 w-[min(560px,calc(100%-24px))] -translate-x-1/2 rounded-2xl border border-success/40 bg-surface p-3.5 text-left shadow-xl"
          onClick={() => {
            const p = done.item.pane;
            setDone(null);
            if (p && list.some((x) => x.id === p)) (current ? go : open)(p);
          }}
        >
          <div className="flex items-center gap-2 text-sm font-medium">
            <Check className="size-4 text-success" />
            <span className="min-w-0 flex-1">
              {done.item.text}
              {done.more > 0 ? <span className="text-muted">{t(` (and ${done.more} more)`, `（还有 ${done.more} 条）`)}</span> : null}
            </span>
          </div>
          {done.item.output?.length ? (
            <pre className="mt-2 max-h-[5.4em] overflow-hidden font-term text-xs whitespace-pre-wrap text-muted [overflow-wrap:anywhere]">
              {done.item.output.slice(-3).join("\n")}
            </pre>
          ) : null}
        </button>
      ) : null}

      <Switcher open={switcher} onClose={() => setSwitcher(false)} panes={list} current={current} onPick={go} />
      <InboxSheet pane={inbox} onClose={() => setInbox(null)} panes={list} readOnly={!!info?.readOnly} />
      <RenameDialog ask={ask} onDone={renamed} />
    </div>
  );
}

function Settings({
  mode,
  setMode,
  themes,
  theme,
  setTheme,
  readOnly,
}: {
  mode: Mode;
  setMode: (m: Mode) => void;
  themes: string[];
  theme: string;
  setTheme: (n: string) => void;
  readOnly: boolean;
}) {
  const Icon = mode === "light" ? Sun : mode === "dark" ? Moon : Laptop;
  return (
    <Dropdown>
      <Button isIconOnly variant="ghost" size="sm" aria-label={t("Appearance", "外观")}>
        <Icon className="size-4.5" />
      </Button>
      <Dropdown.Popover placement="bottom end" className="min-w-56">
        <Dropdown.Menu
          onAction={(k) => {
            const key = String(k);
            if (key.startsWith("mode:")) setMode(key.slice(5) as Mode);
            else if (key.startsWith("theme:")) setTheme(key.slice(6));
          }}
        >
          <Dropdown.Section>
            <Header>{t("This page", "这个页面")}</Header>
            {(
              [
                ["light", t("Light", "日间"), Sun],
                ["dark", t("Dark", "夜间"), Moon],
                ["system", t("Like the system", "跟随系统"), Laptop],
              ] as const
            ).map(([m, label, I]) => (
              <Dropdown.Item key={m} id={"mode:" + m} textValue={label}>
                <I className="size-4" />
                <Label>{label}</Label>
                {mode === m ? <Check className="ml-auto size-4 text-accent" /> : null}
              </Dropdown.Item>
            ))}
          </Dropdown.Section>
          {readOnly ? null : <Separator />}
          {readOnly ? null : (
            <Dropdown.Section>
              <Header>{t("Terminal theme (the computer's too)", "终端主题（电脑上也会变）")}</Header>
              {themes.map((n) => (
                <Dropdown.Item key={n} id={"theme:" + n} textValue={THEME_NAMES[n] || n}>
                  <Palette className="size-4" />
                  <Label>{THEME_NAMES[n] || n}</Label>
                  {theme === n ? <Check className="ml-auto size-4 text-accent" /> : null}
                </Dropdown.Item>
              ))}
            </Dropdown.Section>
          )}
        </Dropdown.Menu>
      </Dropdown.Popover>
    </Dropdown>
  );
}

