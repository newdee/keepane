// The app the browser tests drive: `node e2e/serve.ts <port>`, started by the
// e2e runner (e2e.config.ts, app.command). Copies the built keepane, starts a
// server of its own with a few sessions in it, serves the page on <port>,
// and stays until the runner stops it.
//
// The page is built into the binary: build both first (README, "Web tests").
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DIR, EXE, SOCKET, kp } from "./keepane.ts";

const quiet = (f: () => unknown) => {
  try {
    f();
  } catch {
    // nothing to stop
  }
};
const pause = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// `serve.ts --after <pid>`: the watcher. On Windows the runner ends the
// server's command outright, no handler runs; this, apart from it, waits for
// that and stops the keepane server then (its shells with it). A run killed
// with its whole process tree takes the watcher too: the server it leaves
// is stopped by the next run, first thing.
if (process.argv[2] === "--after") {
  const pid = Number(process.argv[3]);
  const alive = () => {
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  while (alive()) pause(500);
  quiet(() => kp("kill-server"));
  process.exit(0);
}

const port = process.argv[2];
if (!/^\d+$/.test(port ?? "")) throw new Error("usage: node e2e/serve.ts <port>");

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const built = process.env.KEEPANE_BIN || path.join(repo, "target", "debug", path.basename(EXE));
const page = path.join(repo, "web", "dist", "index.html");
if (!fs.existsSync(built)) throw new Error(`no keepane at ${built}: run \`cargo build\` first (or set KEEPANE_BIN)`);
// A binary older than the page it should carry tests the page before.
if (fs.existsSync(page) && fs.statSync(page).mtimeMs > fs.statSync(built).mtimeMs)
  throw new Error(`${built} is older than web/dist: build keepane again after \`npm run build\``);

// What is left from a run the runner could not stop cleanly goes first.
fs.mkdirSync(DIR, { recursive: true });
if (fs.existsSync(EXE)) quiet(() => kp("kill-server"));
// A server just stopped lets go of its binary a moment later.
for (let i = 0; ; i++) {
  try {
    fs.copyFileSync(built, EXE);
    break;
  } catch (e) {
    if (i === 50) throw e;
    pause(200);
  }
}
// Shells without the user's profile: the same prompt on every machine.
// (Still interactive, so keepane's hook goes in: times, the prompt.)
const conf = path.join(DIR, "tests.conf");
fs.writeFileSync(conf, process.platform === "win32" ? `set -g default-command "pwsh -NoLogo -NoProfile"\n` : "");
const sessions = path.join(DIR, "sessions");
fs.rmSync(sessions, { recursive: true, force: true });
// Read by the server this starts: no user config, no user sessions.
process.env.KEEPANE_CONFIG = conf;
process.env.KEEPANE_SESSIONS_DIR = sessions;

// work: editor | builder (a shell pane, with some output), and logs; notes.
// work is wide: its prompts take one row (tests/feat.e2e.ts makes a narrow
// pane of its own where it wants them wider than the pane).
kp("new-session", "-d", "-s", "work", "-n", "editor", "-x", "200", "-y", "50");
kp("split-window", "-h", "-t", "work");
kp("rename-pane", "-t", "work:0.1", "builder");
kp("set-work-mode", "-t", "work:0.1", "shell");
kp("new-window", "-d", "-t", "work", "-n", "logs");
kp("new-session", "-d", "-s", "notes");
// No grey guess from the history after what is typed: the tests read the line.
if (process.platform === "win32") kp("send-keys", "-t", "work:0.1", "Set-PSReadLineOption -PredictionSource None", "Enter");
const listing = process.platform === "win32" ? "Get-ChildItem C:\\Windows | Select-Object -First 12" : "ls / | head -12";
kp("send-keys", "-t", "work:0.1", listing, "Enter");
// Last: the runner waits for the page, so the sessions are there by then.
kp("web", "-b", "127.0.0.1", "-p", port);
console.log(`keepane ${execFileSync(EXE, ["-V"], { encoding: "utf8" }).trim()} on socket ${SOCKET}: ${kp("web", "status").trim()}`);

spawn(process.execPath, [fileURLToPath(import.meta.url), "--after", String(process.pid)], { detached: true, stdio: "ignore" }).unref();
const stop = () => {
  quiet(() => kp("kill-server"));
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
setInterval(() => {}, 1 << 30);
