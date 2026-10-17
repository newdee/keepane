// The site's light and dark: the system's choice, the visitor's pick over
// it, and every picture shown in the look of the page (img/name.ext dark,
// img/name-light.ext light, in English and Chinese).
import { expect } from "e2e";
import { stays, test } from "../e2e/fixtures.ts";
import { colourScheme } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";

// `system`: prefers-color-scheme as the page sees it, for the report when a
// look is not the one expected (the checks name the other three).
type Look = { theme: string; bg: string; meta: string; system: string };
const look = (browser: Browser) =>
  browser.evaluate(() => ({
    system: matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark",
    theme: document.documentElement.dataset.theme ?? "",
    bg: getComputedStyle(document.body).backgroundColor,
    meta: document.querySelector('meta[name="theme-color"]')!.getAttribute("content") ?? "",
  })) as Promise<Look>;
// The picture each <img> shows: its own src, or its <picture>'s <source>
// when that one's media holds.
const sources = (browser: Browser) =>
  browser.evaluate(() =>
    [...document.querySelectorAll("main img")].map((i) => {
      const s = i.parentElement!.tagName === "PICTURE" ? i.parentElement!.querySelector("source") : null;
      return s && matchMedia(s.media).matches ? s.getAttribute("srcset")! : i.getAttribute("src")!;
    }),
  ) as Promise<string[]>;
const src = async (browser: Browser, alt: string) => {
  const alts = (await browser.evaluate(() => [...document.querySelectorAll("main img")].map((i) => i.getAttribute("alt")!))) as string[];
  return (await sources(browser))[alts.findIndex((a) => a.startsWith(alt))];
};

// A load, then its fonts: the page's own fonts swap in late and the bar's
// buttons move when they do, so a click aimed before that can miss (once in
// 55 runs a pick did not take, cause unproven; none in the runs since).
const settled = async (browser: Browser, load: Promise<unknown>) => {
  await load;
  await browser.evaluate(async () => {
    await document.fonts.ready;
    return null;
  });
};

const DARK = { theme: "dark", bg: "rgb(18, 18, 18)", meta: "#121212" };
const LIGHT = { theme: "light", bg: "rgb(253, 253, 251)", meta: "#fdfdfb" };

test("the site: light or dark as the system says, a pick over it, the pictures in the same look", async ({ app, browser, screen }) => {
  await settled(browser, app.open("/"));
  const system = await colourScheme(browser, "dark");
  try {
    await browser.evaluate(() => {
      localStorage.clear();
      return null;
    });
    await settled(browser, browser.reload());

    // Dark: the HTML's own pictures, 21 of them, none light.
    await expect.poll(() => look(browser)).toMatchObject(DARK);
    const dark = await sources(browser);
    expect(dark.length).toBe(21);
    expect(dark.filter((s) => s.includes("-light"))).toEqual([]);
    expect(await src(browser, "A tour")).toBe("img/keepane-tour.gif");

    // Each picture has its light twin, and the phone's Chinese ones too.
    const twins = dark.flatMap((s) => [s, s.replace(/(\.\w+)$/, "-light$1")]).concat(["img/phone-zh.png", "img/phone-zh-light.png"]);
    const missing = await browser.evaluate(
      async (all) => (await Promise.all(all.map(async (s) => ((await fetch(s, { method: "HEAD" })).ok ? null : s)))).filter(Boolean),
      twins,
    );
    expect(missing).toEqual([]);

    // The system goes light: the page follows at once, no reload.
    await system.set("light");
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);
    const light = await sources(browser);
    expect(light).toEqual(dark.map((s) => s.replace(/(\.\w+)$/, "-light$1")));
    // The pictures shown are loaded, not broken: the hero (eager) has its size.
    await expect.poll(() => browser.evaluate(() => (document.querySelector(".hero img") as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);

    // A reload under a light system starts light, before any script of the page's body.
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);
    expect(await src(browser, "A tour")).toBe("img/keepane-tour-light.gif");
    // Only light pictures are fetched: none of the dark ones the HTML names.
    const fetched = await browser.evaluate(() => performance.getEntriesByType("resource").map((e) => new URL(e.name).pathname).filter((p) => p.startsWith("/img/")));
    expect((fetched as string[]).filter((p) => !p.includes("-light"))).toEqual([]);
    expect(fetched).toContain("/img/keepane-tour-light.gif");

    // Chinese: the phone's picture in Chinese, still light.
    await screen.getByRole("button", "中文").click();
    expect(await src(browser, "keepane web on a phone")).toBe("img/phone-zh-light.png");
    await expect(screen.getByRole("button", "深色页面")).toBeVisible();

    // The pick: dark, over the light system, kept across a reload.
    await screen.getByRole("button", "深色页面").click();
    await expect.poll(() => look(browser)).toMatchObject(DARK);
    expect(await src(browser, "keepane web on a phone")).toBe("img/phone-zh.png");
    expect(await sources(browser)).toEqual(dark.map((s) => (s === "img/phone.png" ? "img/phone-zh.png" : s)));
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(DARK);
    // A picked look stays when the system changes.
    await system.set("dark");
    await system.set("light");
    await stays("a picked look stays", async () => (await look(browser)).theme === "dark");

    // Back to English and light by the button.
    await screen.getByRole("button", "EN").click();
    await screen.getByRole("button", "Light page").click();
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);
    expect(await src(browser, "keepane web on a phone")).toBe("img/phone-light.png");
    expect(await browser.evaluate(() => localStorage.getItem("keepane-theme"))).toBe("light");
    // Picked light on a dark system: the page's own light colours, not the
    // system's (the checks below run so).
    await system.set("dark");
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);

    // Text on the page, the panels and the status line reads at 4.5:1 or
    // more in both looks.
    for (const want of ["light", "dark"]) {
      if ((await look(browser)).theme !== want) await screen.getByRole("button", want === "light" ? "Light page" : "Dark page").click();
      const worst = await browser.evaluate(async () => {
        // The colours as they end up: a button's background fades over 0.12 s.
        await Promise.all(document.getAnimations().map((a) => a.finished));
        // A computed colour as [r, g, b, alpha], r g b 0-255: rgb(), rgba(),
        // or color(srgb …) (what color-mix() computes to).
        const parse = (c: string): number[] => {
          const n = (c.match(/-?\d+(\.\d+)?(e-?\d+)?/g) ?? []).map(Number);
          if (c.startsWith("color(srgb")) return [n[0] * 255, n[1] * 255, n[2] * 255, n.length > 3 ? n[3] : 1];
          if (c.startsWith("rgb")) return [n[0], n[1], n[2], n.length > 3 ? n[3] : 1];
          return [0, 0, 0, 0];
        };
        const lum = (c: number[]) => {
          const [r, g, b] = c.slice(0, 3).map((v) => v / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
          return 0.2126 * r + 0.7152 * g + 0.0722 * b;
        };
        const ratio = (a: number[], b: number[]) => {
          const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
          return (x + 0.05) / (y + 0.05);
        };
        const over = (top: number[], below: number[]) => [0, 1, 2].map((k) => top[3] * top[k] + (1 - top[3]) * below[k]).concat(1);
        // The colour a node is drawn on: the backgrounds from it out to the
        // first opaque one, the see-through ones (a tint, the top bar) laid
        // over that.
        const under = (n: Element | null): number[] => {
          const layers: number[][] = [];
          for (; n; n = n.parentElement) {
            const c = parse(getComputedStyle(n).backgroundColor);
            if (c[3] > 0) layers.push(c);
            if (c[3] >= 1) break;
          }
          if (!layers.length || layers[layers.length - 1][3] < 1) layers.push([255, 255, 255, 1]);
          return layers.reduceRight((below, top) => over(top, below));
        };
        let low = { r: 99, what: "" };
        for (const n of document.querySelectorAll("body *")) {
          if (!(n instanceof HTMLElement) || !n.offsetParent || ![...n.childNodes].some((c) => c.nodeType === 3 && c.textContent!.trim())) continue;
          const bg = under(n);
          const r = ratio(over(parse(getComputedStyle(n).color), bg), bg);
          if (r < low.r) low = { r: Math.round(r * 100) / 100, what: `${n.tagName}.${n.className} "${n.textContent!.trim().slice(0, 20)}"` };
        }
        return low;
      }) as { r: number; what: string };
      expect(worst.r, `${want}: the faintest text is ${worst.what}`).toBeGreaterThanOrEqual(4.5);
    }
  } finally {
    system.close();
  }
});

test("the site with storage broken, a stored value broken, and no script", async ({ app, browser, screen }) => {
  await settled(browser, app.open("/"));
  const system = await colourScheme(browser, "light");
  try {
    // A value no version wrote: the system's look, as if nothing were kept.
    await browser.evaluate(() => {
      localStorage.setItem("keepane-theme", "purple");
      localStorage.setItem("keepane-lang", "xx");
      return null;
    });
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);
    await expect(screen.getByRole("button", "Dark page")).toBeVisible();

    // Storage that throws (a private window): the page starts by the
    // system's look, and the button still switches it, for this visit.
    // (localStorage itself throws, as Safari's private windows did; the
    // session storage the test's error check uses stays.)
    await browser.addInitScript(() => {
      Object.defineProperty(window, "localStorage", {
        get() {
          throw new Error("no storage");
        },
      });
    });
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);
    await screen.getByRole("button", "Dark page").click();
    await expect.poll(() => look(browser)).toMatchObject(DARK);
    expect(await src(browser, "A tour")).toBe("img/keepane-tour.gif");
    // Not kept, but the pick holds for this visit: the system's change is
    // not followed.
    await system.set("dark");
    await system.set("light");
    await stays("the pick holds for the visit", async () => (await look(browser)).theme === "dark");
    // A reload forgets it: the system's look again.
    await settled(browser, browser.reload());
    await expect.poll(() => look(browser)).toMatchObject(LIGHT);

    // JavaScript off: no data-theme. The colours and the hero's picture
    // follow the system, and the button, which could switch nothing, is
    // hidden. (The other pictures stay the dark ones the HTML names.)
    await system.scripts(false);
    const plain = () =>
      browser.evaluate(() => ({
        theme: document.documentElement.hasAttribute("data-theme"),
        bg: getComputedStyle(document.body).backgroundColor,
        button: getComputedStyle(document.getElementById("theme")!).display,
      })) as Promise<{ theme: boolean; bg: string; button: string }>;
    for (const s of ["light", "dark"] as const) {
      await system.set(s);
      await settled(browser, browser.reload());
      await expect.poll(plain).toEqual({ theme: false, bg: (s === "light" ? LIGHT : DARK).bg, button: "none" });
      expect(await src(browser, "A tour")).toBe(s === "light" ? "img/keepane-tour-light.gif" : "img/keepane-tour.gif");
    }
    await system.scripts(true);
  } finally {
    system.close();
  }
});

test("the site at a phone's width: the bar's buttons fit, nothing scrolls sideways", async ({ app, browser }) => {
  await settled(browser, app.open("/"));
  for (const width of [320, 360, 390]) {
    await browser.setViewport({ width, height: 800 });
    for (const lang of ["en", "zh"]) {
      await browser.evaluate((l) => {
        localStorage.setItem("keepane-lang", l);
        return null;
      }, lang);
      await settled(browser, browser.reload());
      const over = await browser.evaluate(() => ({
        page: document.documentElement.scrollWidth - innerWidth,
        bar: [...document.querySelectorAll(".top > *, .top nav > *")]
          .filter((n) => getComputedStyle(n).display !== "none")
          .map((n) => n.getBoundingClientRect().right - innerWidth)
          .reduce((a, b) => Math.max(a, b), -999),
      }));
      expect(over.page, `${width}px, ${lang}: the page is wider than the screen by`).toBe(0);
      expect(over.bar, `${width}px, ${lang}: the bar's last button ends past the screen by`).toBeLessThanOrEqual(0);
    }
  }
});
