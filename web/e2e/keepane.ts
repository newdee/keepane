// The keepane server the browser tests run against: one socket, one folder,
// shared by e2e/serve.ts (which starts it) and the tests (which ask it what
// happened, through the CLI, as a user at the computer would).
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

/** The socket name: the tests' own server, never the user's. */
export const SOCKET = "e2e-web";
/** Everything the run writes: the copy of keepane, its config and saved sessions. */
export const DIR = path.join(os.tmpdir(), "keepane-e2e");
/** Where the server looks for Claude Code's transcripts (`CLAUDE_CONFIG_DIR`):
 *  a stand-in agent's, never the user's. */
export const CLAUDE_DIR = path.join(DIR, "claude");
/** A copy of the build's binary, so `cargo build` is never locked out by the run. */
export const EXE = path.join(DIR, process.platform === "win32" ? "keepane.exe" : "keepane");

/**
 * Letters no other test or run uses, for what a test types and then looks
 * for: the server and its panes outlive one test.
 */
export const token = (): string => (Date.now() % 1e8).toString(36) + Math.random().toString(36).slice(2, 5);

/** A keepane command against the tests' server; its standard output. */
export const kp = (...args: string[]): string => execFileSync(EXE, ["-L", SOCKET, ...args], { encoding: "utf8" });

/** A format (`#{pane_id}`) for a target, as `display-message -p` gives it. */
export const show = (target: string, format: string): string => kp("display-message", "-p", "-t", target, format).trim();

/**
 * Runs `body` with the global option `name` at `value` (when given), and puts
 * back the value it found, whatever happens.
 */
export async function withOption<T>(name: string, value: string | undefined, body: () => Promise<T>): Promise<T> {
  const was = kp("show-options", "-gv", name).trim();
  if (value !== undefined) kp("set-option", "-g", name, value);
  try {
    return await body();
  } finally {
    kp("set-option", "-g", name, was);
  }
}

/** The ids of every pane there is (`%3`). */
export const paneIds = (): string[] => kp("list-panes", "-a", "-F", "#{pane_id}").split("\n").map((l) => l.trim()).filter(Boolean);

/** Closes `pane` if it is still there: a test's own pane, after it. */
export const closeIfThere = (pane: string) => {
  if (paneIds().includes(pane)) kp("kill-pane", "-t", pane);
};

/** The pane's id without its `%`, as the page's address takes it. */
export const paneNumber = (target: string): string => show(target, "#{pane_id}").replace("%", "");

/** The pane's text, wrapped rows joined, `rows` lines of history with it. */
export const capture = (target: string, rows = 200): string => kp("capture-pane", "-p", "-J", "-S", `-${rows}`, "-t", target);

/** The pane's last line that is not blank: the prompt and what is typed on it. */
export const lastLine = (target: string): string => {
  const rows = kp("capture-pane", "-p", "-J", "-t", target)
    .split("\n")
    .map((l) => l.trimEnd())
    .filter(Boolean);
  return rows[rows.length - 1] ?? "";
};

/** The addresses `web status` gives, one per server serving: `http://host:port/#k=key`. */
export const webUrls = (): string[] =>
  kp("web", "status")
    .split("\n")
    .map((l) => /^serving (\S+)/.exec(l.trim())?.[1])
    .filter((u): u is string => !!u);

/** The key in the address of the server on `port`. */
export const keyOn = (port: string | number): string => {
  const url = webUrls().find((u) => new URL(u).port === String(port));
  const key = url && /#k=([^&]+)/.exec(url)?.[1];
  if (!key) throw new Error(`no keepane web on port ${port}: ${kp("web", "status")}`);
  return key;
};

/** The page's path for the server on `port`: the list, or one pane open. */
export const page = (port: string | number, pane?: string): string => `/#${pane ? `p=${pane}&` : ""}k=${keyOn(port)}`;

/** The port of the address the run serves (`app.baseUrl`). */
export const portOf = (baseUrl: string | undefined): string => {
  if (!baseUrl) throw new Error("the target declares no app.url");
  return new URL(baseUrl).port;
};
