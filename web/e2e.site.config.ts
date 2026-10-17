// The project site's browser tests (docs/, what GitHub Pages serves):
// `npx e2e run --config e2e.site.config.ts` here, in web/. Nothing to build
// first; no model: every step is exact.
import type { E2EConfig } from "e2e";
import { web } from "@e2e-dev/web";
import { keepErrors } from "./e2e/errors.ts";
import { localChromium } from "./e2e/phone.ts";

export default {
  tests: "site/**/*.e2e.ts",
  targets: [
    {
      // The local Chromium for its DevTools port: the tests set the
      // system's light or dark through it (phone.ts, colourScheme).
      engine: web({ browser: localChromium(), viewport: { width: 1280, height: 860 }, locale: "en-US", initScripts: [keepErrors] }),
      app: {
        url: "http://127.0.0.1:0",
        command: { executable: "node", args: ["e2e/serve-site.ts", "{port}"], log: ".e2e/logs/site.log", startupTimeout: 30_000 },
      },
    },
  ],
  // Its report and artifacts apart from the page's (.e2e/).
  output: ".e2e/site",
  workers: 1,
  retries: 0,
  timeout: 120_000,
  assertionTimeout: 10_000,
} satisfies E2EConfig;
