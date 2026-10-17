// The list as a grid: a cell per pane with its last lines, two columns on a
// phone held upright and more on a wider screen; a tap opens the pane, a long
// press its menu; the choice kept on this device. The last lines are asked
// for only while the grid is shown.
import { expect } from "e2e";
import type { Browser } from "@e2e-dev/web";
import { pace, stays, test } from "../e2e/fixtures.ts";
import { kp, page, paneIds, portOf, show, token } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const columns = (browser: Browser) =>
  browser.evaluate(() => getComputedStyle(document.querySelector("[data-grid]")!).gridTemplateColumns.split(" ").length);
const tailOf = (browser: Browser, pane: string) =>
  browser.evaluate((p) => [...document.querySelectorAll(`[data-cell="${p}"] [data-tail] > div`)].map((d) => d.textContent), pane);
const onList = (browser: Browser) => browser.evaluate(() => !document.querySelector("#screen"));

test("the list as a grid", async ({ app, browser, screen }) => {
  const builder = show("work:0.1", "#{pane_id}");
  const asked: string[] = [];
  await browser.route(/\/api\/panes/, async (r) => {
    asked.push(r.request.url);
    await r.continue();
  });
  await app.open(page(portOf(app.baseUrl)));
  const finger = await phone(browser);
  const grid = browser.locator("#view-grid");
  const list = browser.locator("#view-list");
  await expect(list).toHaveAttribute("aria-pressed", "true");
  await expect(browser.locator("[data-cell]")).toHaveCount(0);
  expect(asked.some((u) => u.includes("tail=")), "a list: no last lines asked for").toBe(false);

  // A cell per pane, two columns on a phone upright.
  await finger.tap(grid);
  await expect(grid).toHaveAttribute("aria-pressed", "true");
  await expect(browser.locator("[data-cell]")).toHaveCount(paneIds().length);
  await expect(browser.locator("[data-pane]")).toHaveCount(0);
  expect(await columns(browser)).toBe(2);
  await expect.poll(() => asked.some((u) => u.endsWith("/api/panes?tail=4"))).toBe(true);

  // Its last 4 lines, the latest at the bottom, as the pane printed them.
  const k = `g${token()}`;
  kp("send-keys", "-t", builder, `echo ${k}1; echo ${k}2; echo ${k}3; echo ${k}4; echo ${k}5`, "Enter");
  await expect.poll(() => tailOf(browser, builder), { timeout: 15_000 }).toEqual([2, 3, 4, 5].map((i) => `${k}${i}`));

  // Held: its menu, not the pane.
  await finger.longPress(browser.locator(`[data-cell="${builder}"]`));
  await expect(screen.getByRole("menuitem")).toHaveCount(12);
  await expect(browser.locator("#screen")).toHaveCount(0);
  await browser.keyboard.press("Escape");
  await expect(screen.getByRole("menu")).toHaveCount(0);

  // Tapped: the pane; while it is open (the list hidden), no last lines asked for.
  await finger.tap(browser.locator(`[data-cell="${builder}"]`));
  await expect(browser.locator("#screen")).toBeVisible();
  await pace();
  const from = asked.length;
  await expect.poll(() => asked.length, { timeout: 6_000 }).toBeGreaterThan(from);
  await stays("a pane open: no last lines", () => asked.slice(from).every((u) => !u.includes("tail=")), 2500);
  await finger.tap(browser.locator("#back"));
  await expect.poll(() => onList(browser)).toBe(true);
  await expect(browser.locator(`[data-cell="${builder}"]`)).toBeVisible();

  // A session folded: its cells go; unfolded, back.
  const notes = browser.locator("button[aria-expanded]").filter({ hasText: "notes" }).first();
  await finger.tap(notes);
  await expect(browser.locator('[data-grid="notes"]')).toHaveCount(0);
  await pace();
  await finger.tap(notes);
  await expect(browser.locator('[data-grid="notes"] [data-cell]')).toHaveCount(1);

  // Kept on this device; wider, more columns.
  await browser.reload();
  await expect(grid).toHaveAttribute("aria-pressed", "true");
  await expect(browser.locator("[data-cell]")).toHaveCount(paneIds().length);
  await browser.setViewport({ width: 800, height: 600 });
  await expect.poll(() => columns(browser)).toBe(4);

  // Wide: the grid stays beside the pane, its last lines with it; full
  // screen hides it, and they are not asked for.
  await browser.setViewport({ width: 1280, height: 860 });
  await browser.locator(`[data-cell="${builder}"]`).click();
  await expect(browser.locator("#screen")).toBeVisible();
  let at = asked.length;
  await expect.poll(() => asked.slice(at).some((u) => u.endsWith("tail=4")), { timeout: 6_000 }).toBe(true);
  await browser.locator("#fullscreen").click();
  await expect(browser.locator("[data-cell]")).toHaveCount(0);
  await pace();
  at = asked.length;
  await expect.poll(() => asked.length, { timeout: 6_000 }).toBeGreaterThan(at);
  await stays("full screen: no last lines", () => asked.slice(at).every((u) => !u.includes("tail=")), 2500);

  // Back to the list: cards again, and no last lines asked for.
  await app.open(page(portOf(app.baseUrl)));
  await browser.setViewport({ width: 390, height: 844 });
  await expect(browser.locator("[data-cell]")).toHaveCount(paneIds().length);
  await finger.tap(list);
  await expect(browser.locator("[data-pane]")).toHaveCount(paneIds().length);
  await expect(browser.locator("[data-cell]")).toHaveCount(0);
  const back = asked.length;
  await expect.poll(() => asked.length, { timeout: 6_000 }).toBeGreaterThan(back);
  expect(asked.slice(back).every((u) => !u.includes("tail=")), "a list again: no last lines").toBe(true);
  await finger.close();
});
