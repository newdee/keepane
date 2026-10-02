import { useCallback, useEffect, useRef, useState } from "react";
import { api, getJson, key, NEEDS_CODE, post, type Done, type Pane, type Screen, type Theme } from "./api";
import { store, stored } from "./format";
import { t } from "./i18n";

/** Whether the page is shown: nothing is asked for while it is hidden
 *  (which saves the phone's battery). */
export function useVisible() {
  const [visible, setVisible] = useState(document.visibilityState === "visible");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}

export function useMedia(query: string) {
  const [match, setMatch] = useState(() => matchMedia(query).matches);
  useEffect(() => {
    const m = matchMedia(query);
    const on = () => setMatch(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, [query]);
  return match;
}

/** A setting kept on this device. */
export function useStored<T>(name: string, fallback: T): [T, (v: T) => void] {
  const [v, setV] = useState<T>(() => stored(name, fallback));
  const set = useCallback(
    (x: T) => {
      setV(x);
      store(name, x);
    },
    [name],
  );
  return [v, set];
}

/** Every pane, asked for every two seconds while the page is shown. */
export function usePanes(enabled: boolean) {
  const visible = useVisible();
  const [panes, setPanes] = useState<Pane[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(async () => {
    try {
      setPanes(await getJson<Pane[]>("/api/panes"));
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
  return { panes, error, reload };
}

export type ScreenState = Screen & { error: string | null; gone: boolean };

/** A pane's screen, pushed: one long response that carries the screen again
 *  each time it changes (reconnecting after a drop), or asked for every half
 *  second by a browser that cannot read a response as it arrives. */
export function useScreen(pane: string | null, query: string) {
  const visible = useVisible();
  const [state, setState] = useState<ScreenState>({ text: "", marks: [], error: null, gone: false });
  const streaming = useRef(true);
  const poll = useCallback(async () => {
    if (!pane) return;
    try {
      const s = await getJson<Screen>(`/api/screen?${query}`);
      setState({ ...s, error: null, gone: false });
    } catch (e) {
      setState((x) => ({ ...x, error: (e as Error).message }));
    }
  }, [pane, query]);
  useEffect(() => {
    setState({ text: "", marks: [], error: null, gone: false });
  }, [pane]);
  useEffect(() => {
    if (!pane || !visible) return;
    const ctl = new AbortController();
    let timer: number | undefined;
    const watch = async () => {
      while (!ctl.signal.aborted) {
        if (!streaming.current) {
          await poll();
          await new Promise((r) => (timer = window.setTimeout(r, 500)));
          continue;
        }
        try {
          const r = await fetch(`/api/watch?${query}`, {
            headers: { "X-Keepane-Key": key },
            cache: "no-store",
            signal: ctl.signal,
          });
          if (r.status === 401) {
            setState((x) => ({ ...x, error: NEEDS_CODE }));
            return;
          }
          if (!r.ok) throw new Error((await r.text()) || r.statusText);
          if (!r.body || !r.body.getReader) {
            streaming.current = false;
            continue;
          }
          const reader = r.body.getReader();
          const dec = new TextDecoder();
          let buf = "";
          for (;;) {
            const { value, done } = await reader.read();
            if (done) break;
            buf += dec.decode(value, { stream: true });
            let end: number;
            while ((end = buf.indexOf("\n\n")) >= 0) {
              const ev = buf.slice(0, end);
              buf = buf.slice(end + 2);
              if (ev.startsWith("event: gone")) {
                setState((x) => ({ ...x, gone: true, error: t("This pane is gone.", "这个 pane 已经关了。") }));
                return;
              }
              const data = ev
                .split("\n")
                .filter((l) => l.startsWith("data: "))
                .map((l) => l.slice(6))
                .join("\n");
              if (data) {
                const s = JSON.parse(data) as Screen;
                setState({ ...s, error: null, gone: false });
              }
            }
          }
        } catch (e) {
          if (ctl.signal.aborted) return;
          setState((x) => ({ ...x, error: (e as Error).message }));
        }
        await new Promise((r) => (timer = window.setTimeout(r, 1000)));
      }
    };
    watch();
    return () => {
      ctl.abort();
      clearTimeout(timer);
    };
  }, [pane, query, visible, poll]);
  return { ...state, streaming: streaming.current, poll };
}

/** Panes done (`done-events` on the computer): asked for every three seconds
 *  while the page is shown; what was done while it was hidden is told when it
 *  is shown again. The first look is not news. */
export function useDone(onDone: (d: Done) => void) {
  const visible = useVisible();
  const seen = useRef<number | null>(null);
  const cb = useRef(onDone);
  cb.current = onDone;
  useEffect(() => {
    if (!visible) return;
    let stop = false;
    let timer: number | undefined;
    const loop = async () => {
      try {
        const r = await getJson<Done>(`/api/done?after=${seen.current || 0}`);
        if (seen.current === null || r.last < seen.current) seen.current = r.last;
        else if (r.done.length) {
          seen.current = r.last;
          cb.current(r);
        }
      } catch {
        /* told by the list's own error */
      }
      if (!stop) timer = window.setTimeout(loop, 3000);
    };
    loop();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [visible]);
}

const TOKYO_NIGHT: Theme = {
  name: "tokyo-night",
  names: ["tokyo-night", "tokyo-day"],
  fg: "#c0caf5",
  bg: "#1a1b26",
  palette: [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
  ],
};

/** The terminal's theme (what a pane's text is drawn in), asked for at the
 *  start, every ten seconds, and after it is set from here. */
export function useTermTheme() {
  const visible = useVisible();
  const [theme, setTheme] = useState<Theme>(() => stored("keepane-term-theme", TOKYO_NIGHT));
  const load = useCallback(async () => {
    try {
      const th = await getJson<Theme>("/api/theme");
      setTheme(th);
      store("keepane-term-theme", th);
    } catch {
      /* an older keepane: Tokyo Night, as the page always had */
    }
  }, []);
  useEffect(() => {
    if (!visible) return;
    load();
    const id = window.setInterval(load, 10000);
    return () => clearInterval(id);
  }, [visible, load]);
  const set = useCallback(
    async (name: string) => {
      await post(`/api/theme?name=${encodeURIComponent(name)}`);
      await load();
    },
    [load],
  );
  return { theme, set };
}

export type Mode = "light" | "dark" | "system";

/** The page's own light or dark (not the terminal's): chosen on this device,
 *  or the system's. */
export function usePageMode() {
  const [mode, setModeState] = useState<Mode>(() => {
    try {
      const m = localStorage.getItem("keepane-mode");
      return m === "light" || m === "dark" ? m : "system";
    } catch {
      return "system";
    }
  });
  const systemDark = useMedia("(prefers-color-scheme: dark)");
  const dark = mode === "dark" || (mode === "system" && systemDark);
  useEffect(() => {
    const r = document.documentElement;
    r.classList.toggle("dark", dark);
    r.classList.toggle("light", !dark);
    r.dataset.theme = dark ? "dark" : "light";
  }, [dark]);
  const setMode = useCallback((m: Mode) => {
    setModeState(m);
    try {
      if (m === "system") localStorage.removeItem("keepane-mode");
      else localStorage.setItem("keepane-mode", m);
    } catch {
      /* kept for this visit */
    }
  }, []);
  return { mode, dark, setMode };
}

/** The phone's keyboard takes part of the screen: the page's height follows
 *  what is left, so the input stays above it. */
export function useViewportHeight() {
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    const set = () => document.documentElement.style.setProperty("--vh", vv.height + "px");
    vv.addEventListener("resize", set);
    set();
    return () => vv.removeEventListener("resize", set);
  }, []);
}

export { api };
