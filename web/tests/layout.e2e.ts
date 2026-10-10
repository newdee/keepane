// Menus on a right click, a long press and Shift+F10; the list folding to a
// strip; swipes on a tablet; the long press and the edge swipe on a phone.
import { expect, type Screen } from "e2e";
import { dialog, entry, pageText, stays, test } from "../e2e/fixtures.ts";
import { closeIfThere, kp, page, paneIds, paneNumber, portOf, show } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";

const cards = (browser: Browser) => browser.locator("[data-pane]").count();
const rails = (browser: Browser) => browser.locator("[data-rail]").count();
const card = (browser: Browser, name: string) => browser.locator(`[data-pane][data-name="${name}"]`);
// A card's menu, open and taking keys: its 11 entries shown, the focus on
// the first (Escape before then goes to the page, and the menu stays).
const cardMenu = async (screen: Screen) => {
  await expect(screen.getByRole("menuitem")).toHaveCount(11);
  await expect(entry(screen, "打开")).toBeFocused();
};
const onList = (browser: Browser) => browser.evaluate(() => !document.querySelector("#screen") && document.querySelectorAll("[data-pane]").length >= 4);

test("desktop: right click, Shift+F10, closing a pane, the strip", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    for (const [k, v] of Object.entries({ "keepane-mode": "light", "keepane-folded": "[]", "keepane-rail": "false" }))
      if (localStorage.getItem(k) === null) localStorage.setItem(k, v);
  });
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(4);
  await expect(browser.locator("#screen")).toBeVisible();
  await expect(screen.getByRole("button", "重命名")).toHaveCount(0);

  // A card's menu: 8 entries and the 3 modes.
  await card(browser, "builder").secondaryTap();
  await expect(screen.getByRole("menuitem")).toHaveCount(11);
  await entry(screen, "重命名所在窗口").tap();
  const shown = dialog(screen, "重命名窗口");
  await expect(shown.getByRole("textbox")).toBeVisible();
  await browser.keyboard.press("Escape");
  await expect(shown).toHaveCount(0);

  // A session's row: fold every window, unfold them.
  const work = browser.locator("button[aria-expanded]").filter({ hasText: "work" }).first();
  await work.secondaryTap();
  await expect(screen.getByRole("menuitem")).toHaveCount(4);
  await entry(screen, "折叠所有窗口").tap();
  // work's two windows folded, the session itself open.
  await expect
    .poll(() => browser.evaluate(() => [...document.querySelectorAll("button[aria-expanded]")].filter((b) => /^\d+: (editor|logs)/.test(b.textContent!.trim())).map((b) => b.getAttribute("aria-expanded"))))
    .toEqual(["false", "false"]);
  await work.secondaryTap();
  await entry(screen, "展开所有窗口").tap();
  await expect
    .poll(() => browser.evaluate(() => [...document.querySelectorAll("button[aria-expanded]")].filter((b) => b.textContent!.trim()).every((b) => b.getAttribute("aria-expanded") === "true")))
    .toBe(true);

  // Shift+F10 on a focused card: its menu, at the card; Escape gives the focus back.
  await card(browser, "builder").focus();
  await browser.keyboard.press("Shift+F10");
  await cardMenu(screen);
  const c =(await card(browser, "builder").boundingBox())!;
  const m = (await screen.getByRole("menu").boundingBox())!;
  expect(m.y >= c.y - 8 && m.y <= c.y + c.height + 40 && m.x >= c.x - 8 && m.x <= c.x + c.width, "the menu is at the card").toBe(true);
  await browser.keyboard.press("Escape");
  await expect(card(browser, "builder")).toBeFocused();
  // Through the menu to a dialog and out: the focus ends on the card too.
  await browser.keyboard.press("Shift+F10");
  await expect(screen.getByRole("menuitem")).toHaveCount(11);
  for (let i = 0; i < 5; i++) await browser.keyboard.press("ArrowDown");
  await browser.keyboard.press("Enter");
  await expect(shown.getByRole("textbox")).toBeVisible();
  await browser.keyboard.press("Escape");
  await expect(card(browser, "builder")).toBeFocused();

  // Close a spare pane through the menu: the dialog names it.
  kp("split-window", "-d", "-t", "notes");
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(5);
  const spare = show("notes:0.1", "#{pane_id}");
  const ask = screen.getByRole("alertdialog");
  try {
    await browser.locator(`[data-pane="${spare}"]`).secondaryTap();
    await entry(screen, "关闭这个 pane").tap();
    await expect(ask).toContainText(/关闭 .+ · notes:0\.1？/);
    await ask.getByRole("button", "关闭").tap();
    await expect.poll(paneIds, { message: "closed on the computer" }).not.toContain(spare);
  } finally {
    closeIfThere(spare);
  }

  // The strip: a button per pane, kept after a reload.
  await expect(ask).toHaveCount(0);
  await browser.locator("#sidebar-toggle").tap();
  await expect.poll(() => rails(browser)).toBeGreaterThanOrEqual(4);
  expect(await cards(browser)).toBe(0);
  expect((await browser.locator("aside").boundingBox())!.width).toBeLessThanOrEqual(60);
  await screen.getByRole("button", "pwsh · notes:0.0").tap();
  await expect.poll(() => pageText(browser)).toContain("notes:0.0");
  await browser.reload();
  await expect.poll(() => rails(browser)).toBeGreaterThanOrEqual(4);
  await expect.poll(() => pageText(browser), { message: "the pane open before the reload" }).toContain("notes:0.0");
  await browser.locator("#sidebar-toggle").tap();
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(4);
  expect(await rails(browser)).toBe(0);
});

test("tablet: swipe the list away and back; across the screen, the next pane", async ({ app, browser }) => {
  await browser.addInitScript(() => {
    for (const [k, v] of Object.entries({ "keepane-mode": "dark", "keepane-rail": "false" }))
      if (localStorage.getItem(k) === null) localStorage.setItem(k, v);
  });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser, { width: 1024, height: 768 });
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(4);
  await expect(browser.locator("#screen")).toBeVisible();
  await finger.swipe({ x: 300, y: 400 }, { x: 120, y: 405 });
  await expect.poll(() => rails(browser), { message: "swiped left on the list: the strip" }).toBeGreaterThanOrEqual(4);
  await finger.swipe({ x: 28, y: 400 }, { x: 260, y: 402 });
  await expect.poll(() => cards(browser), { message: "swiped right on the strip: the list" }).toBeGreaterThanOrEqual(4);
  const header = () => browser.evaluate(() => [...document.querySelectorAll("button")].find((b) => /work:0\.1/.test(b.textContent!) && !b.closest("[data-pane]"))?.textContent ?? "");
  expect(await header()).not.toBe("");
  await finger.swipe({ x: 800, y: 400 }, { x: 560, y: 405 });
  await expect.poll(header, { message: "a swipe across the screen: another pane" }).toBe("");
  await finger.close();
});

test("phone: the long press, the edge swipe, reloads", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => localStorage.getItem("keepane-mode") === null && localStorage.setItem("keepane-mode", "dark"));
  // A page before, so that going back too far shows.
  await app.open("/?before");
  await app.open(page(portOf(app.baseUrl)));
  const finger = await phone(browser);
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(4);
  const host = new URL(app.baseUrl!).host;
  const here = () => browser.evaluate((host) => location.host === host && !location.search.includes("before"), host);

  // The list column: no system menu, no text selection, no long-press copy bar.
  const style = await browser.evaluate(() => {
    const s = getComputedStyle(document.querySelector('[data-pane][data-name="builder"]')!);
    return [s.userSelect, s.getPropertyValue("-webkit-touch-callout")];
  });
  expect(style[0]).toBe("none");
  expect(style[1]).not.toBe("default");
  expect(
    await browser.evaluate(() => {
      const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      document.querySelector("aside")!.dispatchEvent(e);
      return e.defaultPrevented;
    }),
    "a right click on an empty spot of the column: no system menu",
  ).toBe(true);
  await finger.longPress(browser.locator('[data-pane][data-name="builder"] span').first());
  expect(await browser.evaluate(() => String(getSelection())), "a long press selects no text").toBe("");
  await cardMenu(screen);
  await browser.keyboard.press("Escape");
  await expect(screen.getByRole("menu")).toHaveCount(0);

  await finger.longPress(card(browser, "builder"));
  await expect(screen.getByRole("menuitem")).toHaveCount(11);
  // The long press did not also open the pane.
  await expect(browser.locator("#screen")).toHaveCount(0);
  await finger.tap(entry(screen, "收件箱"));
  await expect(dialog(screen, /收件箱/)).toBeVisible();
  await browser.keyboard.press("Escape");
  await expect(dialog(screen, /收件箱/)).toHaveCount(0);
  // A mouse after a long press (a tablet with a mouse): a click opens the
  // pane, however soon it comes (here sooner than 400 ms after the finger
  // left, when a click could still be the long press's own).
  await browser.evaluate(() => {
    const w = window as unknown as { lift?: number; clicked?: number };
    addEventListener("touchend", () => (w.lift = performance.now()), true);
    addEventListener("click", () => (w.clicked = performance.now()), true);
    return null;
  });
  await finger.longPress(card(browser, "builder"));
  await cardMenu(screen);
  await browser.keyboard.press("Escape");
  await card(browser, "builder").tap();
  await expect(browser.locator("#screen")).toBeVisible();
  const soon = await browser.evaluate(() => {
    const w = window as unknown as { lift: number; clicked: number };
    return w.clicked - w.lift;
  });
  expect(soon, "the click came within 400 ms of the long press").toBeLessThan(400);

  // An edge swipe goes back, never to the pane before: no screen asked for it.
  const prev = paneNumber("work:0.0");
  const asked: string[] = [];
  await browser.route(/\/api\/screen/, async (r) => {
    asked.push(decodeURIComponent(r.request.url));
    await r.continue();
  });
  await finger.swipe({ x: 8, y: 400 }, { x: 260, y: 402 });
  await expect.poll(() => onList(browser), { message: "edge swipe right: the list" }).toBe(true);
  await stays("the edge swipe switched to no other pane", () => !asked.some((u) => new RegExp(`pane=%${prev}(&|$)`).test(u)), 500);
  expect(await here(), "one step back only, still on the page").toBe(true);

  // The system's own edge gesture goes back too: the page's back must not follow it.
  await finger.tap(card(browser, "builder"));
  await expect(browser.locator("#screen")).toBeVisible();
  await finger.swipe({ x: 8, y: 400 }, { x: 260, y: 402 });
  await browser.evaluate(() => (history.back(), null));
  await stays("with a system back as well: still on the page", here, 1200);
  await expect.poll(() => onList(browser)).toBe(true);

  // Reloads: on the list, the list; on a pane, that pane, and Back is the list.
  await browser.reload();
  await expect.poll(() => onList(browser)).toBe(true);
  await stays("and it stays the list", () => onList(browser));
  await finger.tap(card(browser, "builder"));
  await expect(browser.locator("#screen")).toBeVisible();
  await browser.reload();
  await expect(browser.locator("#screen")).toBeVisible();
  await expect.poll(() => pageText(browser)).toContain("work:0.1");
  await finger.tap(browser.locator("#back"));
  await expect.poll(() => onList(browser)).toBe(true);
  expect(await here()).toBe(true);
  await browser.reload();
  await expect.poll(() => onList(browser)).toBe(true);
  await stays("a reload after Back: the list", () => onList(browser));

  // A pane closed meanwhile: its reload says so, Back goes to the list.
  kp("split-window", "-d", "-t", "notes");
  await expect.poll(() => cards(browser)).toBeGreaterThanOrEqual(5);
  const gone = show("notes:0.1", "#{pane_id}");
  try {
    await finger.tap(browser.locator(`[data-pane="${gone}"]`));
    await expect(browser.locator("#screen")).toBeVisible();
  } finally {
    closeIfThere(gone);
  }
  await browser.reload();
  await expect.poll(() => pageText(browser)).toContain("已经关了");
  await finger.tap(browser.locator("#back"));
  await expect.poll(() => onList(browser)).toBe(true);
  expect(await here()).toBe(true);
  await finger.close();
});
