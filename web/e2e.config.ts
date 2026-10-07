// The web page's browser tests (README, "Web tests"): `npx e2e run` here, in
// web/, after `npm run build` and `cargo build`. No model: every step is exact.
import type { E2EConfig } from "e2e";
import { web } from "@e2e-dev/web";
import { localChromium } from "./e2e/phone.ts";

// What the shells in the tests' panes need from this machine; nothing else
// (the runner passes the app's command no more than PATH, HOME and temp).
const PASS = ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "ProgramFiles", "ProgramFiles(x86)", "PATHEXT", "WINDIR", "USERNAME", "COMPUTERNAME", "PSModulePath", "KEEPANE_BIN", "SHELL", "USER", "LANG", "TERM"];
const env = Object.fromEntries(PASS.flatMap((k) => (process.env[k] === undefined ? [] : [[k, process.env[k] as string]])));

export default {
  tests: "tests/**/*.e2e.ts",
  targets: [
    {
      engine: web({
        browser: localChromium(),
        viewport: { width: 1280, height: 860 },
        locale: "zh-CN",
        // What the page throws, kept across reloads for the check at the end
        // of each test (e2e/fixtures.ts).
        initScripts: [
          () => {
            const keep = (what: string) => {
              try {
                const all = JSON.parse(sessionStorage.getItem("e2e-errors") || "[]");
                sessionStorage.setItem("e2e-errors", JSON.stringify([...all, what]));
              } catch {
                // no storage here (about:blank)
              }
            };
            // A ResizeObserver whose work spilled into the next frame is the
            // browser's notice, not an error of the page's.
            addEventListener("error", (e) => e.message.startsWith("ResizeObserver loop") || keep(String(e.message)));
            addEventListener("unhandledrejection", (e) => keep(String(e.reason)));
          },
        ],
      }),
      app: {
        url: "http://127.0.0.1:0",
        command: { executable: "node", args: ["e2e/serve.ts", "{port}"], env, log: ".e2e/logs/app.log", startupTimeout: 60_000 },
      },
    },
  ],
  // One server, one set of sessions: the tests take turns.
  workers: 1,
  retries: 0,
  timeout: 180_000,
  assertionTimeout: 10_000,
} satisfies E2EConfig;
