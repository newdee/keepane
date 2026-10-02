import { t } from "./i18n";

// The key: from the address the QR code gave (then taken out of it, so it is
// not left in the address bar), or kept from last time on this device.
// `#...&p=N` (the pane's number, `%N` in keepane) opens that pane at once.
export const startPane = ((m) => (m ? "%" + m[1] : null))(location.hash.match(/[#&]p=(?:%25|%)?(\d+)/));
export const key = (() => {
  const m = location.hash.match(/k=([A-Za-z0-9_-]+)/);
  try {
    if (m) {
      localStorage.setItem("keepane-key", m[1]);
      history.replaceState(null, "", location.pathname);
      return m[1];
    }
    return localStorage.getItem("keepane-key") || "";
  } catch {
    return m ? m[1] : "";
  }
})();

/** One pane, as `/api/panes` gives it. */
export type Pane = {
  id: string;
  session: string;
  window: number;
  windowName: string;
  pane: number;
  command: string;
  active: boolean;
  windowActive: boolean;
  cols: number;
  rows: number;
  dead: boolean;
  attached: boolean;
  activity: boolean;
  bell: boolean;
  silence: boolean;
  name: string;
  mode: string;
  path: string;
  quiet: number;
  last: string;
  idle: boolean;
  inbox: number;
  unheard: boolean;
  working: number;
  title: string;
};

/** The terminal's theme: what a pane's text is drawn in. */
export type Theme = { name: string; names: string[]; fg: string; bg: string; palette: string[] };

/** A command's time on its line: [line, start ms, end ms, exit]. */
export type Mark = [number, number | null, number | null, number | null];
export type Screen = { text: string; marks: Mark[] };

export type Msg = { id: number; from: string; text: string; waited?: number; for?: string };
export type Inbox = {
  current?: Msg | null;
  queued?: Msg[];
  recent?: { id: number; stage: string; ok?: boolean | null; text?: string }[];
} | null;
export type DoneItem = { text: string; pane: string; output?: string[] };
export type Done = { last: number; done: DoneItem[] };
export type Info = { readOnly: boolean; host: string; version: string };

export const NEEDS_CODE = t(
  "This page needs the code again: run `keepane web` and scan it.",
  "需要重新扫码：在电脑上运行 `keepane web` 再扫一次。",
);

/** A request with the key; an error with keepane's own words when it fails. */
export async function api(path: string, opts: RequestInit = {}): Promise<Response> {
  const r = await fetch(path, {
    ...opts,
    cache: "no-store",
    headers: { "X-Keepane-Key": key, ...(opts.headers || {}) },
  });
  if (r.status === 401) throw new Error(NEEDS_CODE);
  if (!r.ok) throw new Error((await r.text()) || r.statusText);
  return r;
}

export const getJson = async <T,>(path: string): Promise<T> => (await api(path)).json() as Promise<T>;
export const post = (path: string, body?: string) => api(path, { method: "POST", body: body ?? "" });
export const q = (s: string) => encodeURIComponent(s);
