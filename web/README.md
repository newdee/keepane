# The web page

What `keepane web` serves: the panes on a phone or another computer's
browser. React, [HeroUI](https://heroui.com) v3 and Tailwind CSS v4, built by
Vite into one file, `dist/index.html` (scripts and styles inline), with
`dist/index.html.gz` beside it for browsers that take gzip. `src/web.rs`
includes both, so building keepane needs no Node; both are committed.

```sh
npm ci
npm run dev      # the page with /api going to a running `keepane web`
                 # (KEEPANE_WEB=http://127.0.0.1:7681 by default)
npm run check    # types
npm run build    # dist/: commit it with the source change
```

The same source builds the same bytes on every machine, and CI checks that
`dist/` is what the source builds. Two things keep it so: Tailwind looks for
classes only in `src/` and HeroUI's components (not in `dist/`), and no
lightningcss pass touches the CSS (it rounds colours differently on Windows
and Linux).

The page talks to keepane over the `/api/*` routes in `src/web.rs`; each
request carries the key from the address the QR code gave
(`X-Keepane-Key`).

## Browser tests

`tests/*.e2e.ts` drive the page in Chromium with
[e2e](https://github.com/tester-army/e2e) and check each step against
keepane itself, through its CLI. Windows only for now: the panes run
PowerShell.

```sh
npm run build                         # the page into dist/,
cargo build                           # then into keepane (in the repository root)
npx @e2e-dev/web install chromium     # once
npm run test:e2e                      # or: npx e2e run tests/live.e2e.ts
```

Node 22.22.3 or newer (or 24.8). `e2e/serve.ts` starts the app under test:
a copy of `target/debug/keepane` (`KEEPANE_BIN` names another) with a
server of its own (socket `e2e-web`, folder `%TEMP%\keepane-e2e`), a few
sessions, and the page on a free port. It refuses a binary older than
`dist/`. Tests take turns (`workers: 1`): they share that server, so each
puts back what it changes and looks only for text it typed itself. The
server stops with the run; a run killed with its whole process tree leaves
it running until the next run, which stops it first.

A phone is a touch screen. The web engine has no touch emulation yet, so
`e2e/phone.ts` starts the browser itself, with a DevTools port, turns touch
on for the test's page and gives the test fingers: taps, long presses,
swipes, a pinch. The rest is e2e's own: `screen`, `browser`, `expect`.

A failed test leaves its screen, a screenshot and a Playwright trace under
`.e2e/artifacts/`. The run sends e2e's anonymous usage figures unless
`E2E_TELEMETRY_DISABLED=1`.
