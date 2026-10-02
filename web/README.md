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
