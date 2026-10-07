// Boundaries and broken input: every stored value broken, a wrong key and
// none, a pane that is not there or goes, widths, a 5000-character line.
import { expect } from "e2e";
import { box, pageText, test } from "../e2e/fixtures.ts";
import { capture, kp, page, paneNumber, portOf, show, token } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";

const noSideScroll = (browser: Browser) => browser.evaluate(() => document.documentElement.scrollWidth <= innerWidth && document.body.scrollWidth <= innerWidth);

const broken = {
  "keepane-sent": "{not json",
  "keepane-folded": "[1,",
  "keepane-wrap": "garbage",
  "keepane-detail": "x",
  "keepane-more-keys": "{",
  "keepane-mode": "purple",
  "keepane-term-theme": "{bad",
};

for (const [width, height, touch] of [
  [390, 844, true],
  [1440, 900, false],
] as const) {
  test(`${width} px, every stored value broken: the page still works`, async ({ app, browser }) => {
    await browser.addInitScript((s: Record<string, string>) => {
      for (const [k, v] of Object.entries(s)) localStorage.setItem(k, v);
    }, broken);
    await browser.setViewport({ width, height });
    await app.open(page(portOf(app.baseUrl)));
    const finger = touch ? await phone(browser, { width, height }) : null;
    await expect.poll(() => browser.locator("[data-pane]").count()).toBeGreaterThanOrEqual(4);
    const builder = browser.locator('[data-pane][data-name="builder"]');
    await (finger ? finger.tap(builder) : builder.tap());
    await expect(browser.locator("#screen")).toContainText("PS ");
    expect(await browser.evaluate(() => ["light", "dark"].some((m) => document.documentElement.classList.contains(m))), "a mode, light or dark").toBe(true);
    await finger?.close();
  });
}

for (const [what, address] of [
  ["a wrong key", "/#k=AAAAAAAAAAAAAAAAAAAAAA"],
  ["no key", "/"],
] as const) {
  test(`${what}: the page says the code is needed`, async ({ app, browser }) => {
    await browser.addInitScript(() => localStorage.setItem("keepane-key", ""));
    await app.open(address);
    await phone(browser);
    await expect.poll(() => pageText(browser)).toMatch(/重新扫码|code again/);
  });
}

test("a pane that is not there: said so; Back is the list", async ({ app, browser }) => {
  await app.open(page(portOf(app.baseUrl), "99999"));
  const finger = await phone(browser);
  await expect(browser.locator("#back")).toBeVisible();
  expect((await pageText(browser)).length).toBeGreaterThan(0);
  await finger.tap(browser.locator("#back"));
  await expect.poll(() => browser.locator("[data-pane]").count()).toBeGreaterThanOrEqual(4);
});

test("the open pane killed meanwhile: said so", async ({ app, browser }) => {
  kp("split-window", "-d", "-t", "notes");
  const victim = show("notes:0.1", "#{pane_id}");
  try {
    await app.open(page(portOf(app.baseUrl), victim.replace("%", "")));
    await phone(browser);
    await expect(browser.locator("#screen")).toContainText("PS ");
  } finally {
    kp("kill-pane", "-t", victim);
  }
  await expect.poll(() => pageText(browser)).toMatch(/已经关了|gone/);
});

for (const [width, height, touch] of [
  [320, 568, true],
  [2560, 1440, false],
] as const) {
  test(`${width} px: nothing wider than the page`, async ({ app, browser }) => {
    await browser.setViewport({ width, height });
    await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
    if (touch) await phone(browser, { width, height });
    await expect(browser.locator("#screen")).toContainText("PS ");
    await expect.poll(() => noSideScroll(browser)).toBe(true);
  });
}

test("a 5000-character line arrives whole", async ({ app, browser }) => {
  // A pane of its own: a line this long keeps a shell busy redrawing.
  kp("split-window", "-h", "-d", "-t", "notes");
  const pane = show("notes:0.1", "#{pane_id}");
  try {
    await browser.setViewport({ width: 1440, height: 900 });
    await app.open(page(portOf(app.baseUrl), pane.replace("%", "")));
    await expect(browser.locator("#screen")).toContainText("PS ");
    const end = `END${token()}`;
    // Set at once, as a paste does (typed, it would take minutes).
    await box(browser).fill(`# ${"x".repeat(4990)}${end}`);
    await browser.locator("#send").tap();
    await expect.poll(() => capture(pane).replace(/\s/g, ""), { timeout: 10_000 }).toContain("x".repeat(200) + end);
  } finally {
    kp("kill-pane", "-t", pane);
  }
});
