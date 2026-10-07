// The language, the round-trip time, full screen (with the browser's own,
// and without: an iPhone), offline, read-only.
import { expect } from "e2e";
import { dialog, entry, pageText, test } from "../e2e/fixtures.ts";
import { kp, page, paneNumber, portOf } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";

/** Full screen: the screen and the input only. */
const onlyScreen = (browser: Browser) =>
  browser.evaluate(
    () =>
      !!document.querySelector("#screen") &&
      !!document.querySelector("#text") &&
      !!document.querySelector("#fullscreen-exit") &&
      !document.querySelector('button[aria-label="外观"],button[aria-label="Appearance"]') &&
      !document.querySelector("[data-pane]") &&
      !document.querySelector("#fullscreen") &&
      !document.querySelector("#latency"),
  );
const allBack = (browser: Browser) =>
  browser.evaluate(() => !document.querySelector("#fullscreen-exit") && !!document.querySelector("#fullscreen") && !!document.querySelector("#text"));
const noSideScroll = (browser: Browser) => browser.evaluate(() => document.documentElement.scrollWidth <= innerWidth);

test("a Chinese system: Chinese; English picked at once and kept; full screen; offline", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => localStorage.getItem("keepane-mode") === null && localStorage.setItem("keepane-mode", "light"));
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  await expect(screen.getByRole("button", "外观")).toBeVisible();
  expect(await browser.evaluate(() => document.documentElement.lang)).toBe("zh-CN");
  await expect(browser.locator("#latency")).toHaveText(/^\d+ ms$/);

  await screen.getByRole("button", "外观").tap();
  await entry(screen, "English").tap();
  // At once, no reload: the page's labels and the pane's own.
  await expect(screen.getByRole("button", "Appearance")).toBeVisible({ timeout: 1000 });
  expect(await browser.evaluate(() => document.documentElement.lang)).toBe("en");
  await expect(browser.locator("pre.term")).toBeVisible();
  await expect(browser.locator("#view")).toHaveAccessibleName("View");
  await browser.reload();
  // Kept after a reload.
  await expect(screen.getByRole("button", "Appearance")).toBeVisible();
  await screen.getByRole("button", "Appearance").tap();
  await browser.locator('[data-key="lang:system"]').tap();
  await expect(screen.getByRole("button", "外观")).toBeVisible();
  expect(await browser.evaluate(() => localStorage.getItem("keepane-lang"))).toBeNull();

  // Full screen: the screen and the input only, over the whole screen.
  await browser.locator('[data-pane][data-name="builder"]').tap();
  await browser.locator("#fullscreen").tap();
  await expect.poll(() => onlyScreen(browser)).toBe(true);
  // The browser's full screen comes a moment after the page's.
  const inFull = () => browser.evaluate(() => !!document.fullscreenElement);
  await expect.poll(inFull, { message: "the browser's full screen" }).toBe(true);
  const area = await browser.evaluate(() => {
    const m = document.querySelector("#main")!.getBoundingClientRect();
    return Math.round((m.width * m.height * 100) / (innerWidth * innerHeight));
  });
  expect(area, "the screen's share of the window, %").toBeGreaterThanOrEqual(75);
  await browser.locator("#fullscreen-exit").tap();
  await expect.poll(() => allBack(browser)).toBe(true);
  await expect.poll(inFull, { message: "the browser's full screen left" }).toBe(false);
  expect(await browser.locator("[data-pane]").count()).toBeGreaterThan(0);
  await browser.locator("#fullscreen").tap();
  await expect.poll(() => onlyScreen(browser)).toBe(true);
  await expect.poll(inFull).toBe(true);
  // The browser's own way out (Escape) leaves it too.
  await browser.evaluate(() => document.exitFullscreen().then(() => null));
  await expect.poll(() => allBack(browser), { message: "out, by the browser's own exit" }).toBe(true);

  // The server gone: offline.
  const port = portOf(app.baseUrl);
  kp("web", "stop");
  try {
    await expect(browser.locator("#latency")).toContainText("离线", { timeout: 15_000 });
  } finally {
    kp("web", "-b", "127.0.0.1", "-p", port);
  }
});

test("an English phone; an iPhone's full screen, in the page; another pane from it", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    Object.defineProperty(navigator, "language", { get: () => "en-US" });
    if (localStorage.getItem("keepane-mode") === null) localStorage.setItem("keepane-mode", "dark");
  });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  await expect(screen.getByRole("button", "Back")).toBeVisible();
  await expect(browser.locator("#latency")).toBeVisible();
  await expect(screen.getByRole("button", "Appearance")).toHaveCount(0);
  expect(await noSideScroll(browser), "the bar fits, no sideways scroll").toBe(true);

  // A browser with no full screen (an iPhone's): the same, in the page.
  await browser.addInitScript(() => {
    Object.defineProperty(Document.prototype, "fullscreenEnabled", { get: () => false });
    Object.defineProperty(Document.prototype, "webkitFullscreenEnabled", { get: () => false });
  });
  await browser.reload();
  await expect(browser.locator("#screen")).toBeVisible();
  await finger.tap(browser.locator("#fullscreen"));
  await expect.poll(() => onlyScreen(browser)).toBe(true);
  expect(await browser.evaluate(() => !document.fullscreenElement)).toBe(true);
  expect(await noSideScroll(browser)).toBe(true);
  // The pane bar is gone: the list of panes from the switch button; a pick
  // stays in full screen.
  await finger.tap(browser.locator("#fullscreen-switch"));
  await finger.tap(dialog(screen, "Go to a pane").getByRole("button", { name: /notes:0\.0/ }));
  await expect.poll(() => onlyScreen(browser)).toBe(true);
  await expect.poll(() => pageText(browser)).toMatch(/Typing into [^\n]*notes:0\.0/);
  await finger.tap(browser.locator("#fullscreen-exit"));
  await expect.poll(() => allBack(browser)).toBe(true);
  await finger.close();
});

test("read-only full screen: the ways out float over the screen", async ({ app, browser }) => {
  const port = portOf(app.baseUrl);
  kp("web", "stop");
  try {
    kp("web", "-r", "-b", "127.0.0.1", "-p", port);
    await app.open(page(port, paneNumber("work:0.1")));
    const finger = await phone(browser);
    await expect(browser.locator("#fullscreen")).toBeVisible();
    await expect(browser.locator("#text")).toHaveCount(0);
    await finger.tap(browser.locator("#fullscreen"));
    await expect(browser.locator("#fullscreen-exit")).toBeVisible();
    await expect(browser.locator("#fullscreen-switch")).toBeVisible();
    await expect(browser.locator("#screen")).toBeVisible();
    await expect(browser.locator("#back")).toHaveCount(0);
    await finger.tap(browser.locator("#fullscreen-exit"));
    await expect(browser.locator("#back")).toBeVisible();
    await expect(browser.locator("#fullscreen-exit")).toHaveCount(0);
    await finger.close();
  } finally {
    kp("web", "stop");
    kp("web", "-b", "127.0.0.1", "-p", port);
  }
});
