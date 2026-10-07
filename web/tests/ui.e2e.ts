// The page on a desktop, each step checked against keepane itself (its CLI);
// a read-only page; a phone's list and Back.
import { expect } from "e2e";
import { dialog, box, entry, pace, pageText, test } from "../e2e/fixtures.ts";
import { capture, kp, page, paneNumber, portOf, token, withOption } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";

const B = () => `%${paneNumber("work:0.1")}`;
const lines = () => capture(B()).split("\n").map((l) => l.trim());
const header = (browser: Browser, where: string) =>
  expect.poll(() => browser.evaluate((w) => [...document.querySelectorAll("button")].some((b) => b.textContent!.includes(w) && !b.closest("[data-pane]")), where));

test("desktop: send, menus, themes, rename, switch, fold, history, inbox, done", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    if (localStorage.getItem("keepane-mode") === null) localStorage.setItem("keepane-mode", "light");
    // These steps are about the box that types on Enter.
    if (localStorage.getItem("keepane-input") === null) localStorage.setItem("keepane-input", JSON.stringify("type"));
  });
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  await expect(browser.locator("pre.term")).toContainText("PS ");

  // Enter sends the text; Enter again, the box empty, is the Enter.
  const mark = `hello-web-${Date.now() % 100000}`;
  await box(browser).pressSequentially(`echo ${mark}`);
  await browser.keyboard.press("Enter");
  await expect.poll(() => capture(B())).toContain(`echo ${mark}`);
  expect(lines()).not.toContain(mark);
  await browser.keyboard.press("Enter");
  await expect.poll(lines, { message: "Enter again runs it" }).toContain(mark);
  await expect.poll(() => browser.evaluate((m) => document.querySelector("pre.term")!.textContent!.split("\n").some((l) => l.trim() === m), mark)).toBe(true);
  const prompts = () => capture(B()).split("\n").filter((l) => l.startsWith("PS ")).length;
  const before = prompts();
  await browser.locator("#send").tap();
  await expect.poll(prompts, { message: "Send with the box empty is Enter" }).toBeGreaterThan(before);

  // ⋯: the inbox.
  await screen.getByRole("button", "更多").tap();
  await entry(screen, "收件箱").tap();
  await expect(dialog(screen, /收件箱/)).toBeVisible();
  await browser.keyboard.press("Escape");
  await expect(dialog(screen, /收件箱/)).toHaveCount(0);

  // Appearance: dark; the terminal's theme, on the computer too, and back.
  await screen.getByRole("button", "外观").tap();
  await entry(screen, "夜间").tap();
  await expect.poll(() => browser.evaluate(() => document.documentElement.classList.contains("dark") && localStorage.getItem("keepane-mode") === "dark")).toBe(true);
  await withOption("theme", undefined, async () => {
    await screen.getByRole("button", "外观").tap();
    await entry(screen, "Tokyo Day").tap();
    await expect.poll(() => kp("show", "-gv", "theme").trim()).toBe("tokyo-day");
    await expect.poll(() => browser.evaluate(() => getComputedStyle(document.querySelector("pre.term")!.parentElement!).backgroundColor)).toBe("rgb(225, 226, 231)");
    await screen.getByRole("button", "外观").tap();
    await entry(screen, "Tokyo Night").tap();
    await expect.poll(() => kp("show", "-gv", "theme").trim()).toBe("tokyo-night");
  });

  // Rename the window logs (its row's menu).
  await browser.locator("button[aria-expanded]").filter({ hasText: "1: logs" }).secondaryTap();
  await entry(screen, "重命名").tap();
  const name = dialog(screen, "重命名窗口").getByRole("textbox");
  await name.fill("logs2");
  await name.press("Enter");
  try {
    await expect.poll(() => kp("list-windows", "-t", "work")).toContain("logs2");
  } finally {
    kp("rename-window", "-t", "work:1", "logs");
  }

  // The switcher: to notes' pane.
  await browser.locator('button:has-text("work:0.1")').tap();
  await dialog(screen, "切换到 pane").getByRole("button", { name: /notes:0\.0/ }).tap();
  await header(browser, "notes:0.0").toBe(true);

  // Fold the session work, and unfold it.
  const work = browser.locator("button[aria-expanded]").filter({ hasText: "work" }).first();
  await work.tap();
  const windows = kp("list-windows", "-t", "work").trim().split("\n").length;
  await expect.poll(() => pageText(browser)).toContain(`${windows} 个窗口`);
  expect(await pageText(browser)).not.toContain("0: editor");
  await pace();
  await work.tap();
  await expect.poll(() => pageText(browser)).toContain("0: editor");

  // History: what was sent is there to pick again.
  await browser.locator('[data-pane][data-name="builder"]').tap();
  await header(browser, "work:0.1").toBe(true);
  await screen.getByRole("button", "发过的命令").tap();
  await expect(dialog(screen, "发过的命令").getByRole("button", { name: new RegExp(`echo ${mark}`) })).toBeVisible();
  await browser.keyboard.press("Escape");
  await expect(dialog(screen, "发过的命令")).toHaveCount(0);

  // An agent pane that never said it is free: its messages wait in its inbox.
  kp("set-work-mode", "-t", "notes:0.0", "ai");
  const [one, two] = [`waiting-one-${token()}`, `waiting-two-${token()}`];
  try {
    kp("send-message", "--to", "notes:0.0", one);
    kp("send-message", "--to", "notes:0.0", two);
    await expect.poll(() => pageText(browser)).toContain("排队 2");
    await browser.locator(`[data-pane="%${paneNumber("notes:0.0")}"] .chip`).filter({ hasText: "排队" }).tap();
    const inbox = dialog(screen, /收件箱/);
    await expect(inbox).toContainText(two);
    await inbox.getByRole("button", "删除").first().tap();
    await expect.poll(() => kp("list-messages", "-t", "notes:0.0"), { message: "delete takes the first away" }).not.toContain(one);
    await inbox.getByRole("button", "撤销").tap();
    await expect.poll(() => kp("list-messages", "-t", "notes:0.0"), { message: "undo brings it back" }).toContain(one);
    await browser.keyboard.press("Escape");
  } finally {
    // Nothing left waiting for the next test.
    for (const [, id] of kp("list-messages", "-t", "notes:0.0").matchAll(/#(\d+)/g)) kp("drop-message", id);
    kp("set-work-mode", "-t", "notes:0.0", "normal");
  }

  // A pane done: the banner.
  await withOption("done-after", "0", async () => {
    kp("send-keys", "-t", B(), "Start-Sleep -Milliseconds 300; Write-Output done-check", "Enter");
    await expect.poll(() => browser.evaluate(() => [...document.querySelectorAll("button.drop-in")].some((b) => b.textContent!.length > 0)), { timeout: 15_000 }).toBe(true);
  });
});

test("read-only: nothing to type into, nothing to change", async ({ app, browser }) => {
  const port = portOf(app.baseUrl);
  kp("web", "stop");
  try {
    kp("web", "-r", "-b", "127.0.0.1", "-p", port);
    await browser.setViewport({ width: 1440, height: 900 });
    await app.open(page(port, paneNumber("work:0.1")));
    await expect(browser.locator("#screen")).toContainText("PS ");
    await expect(box(browser)).toHaveCount(0);
    await expect(browser.locator('button[aria-label="更多"]')).toHaveCount(0);
    await expect(browser.locator('button[aria-label="重命名"]')).toHaveCount(0);
    await expect.poll(() => pageText(browser)).toContain("只读");
  } finally {
    kp("web", "stop");
    kp("web", "-b", "127.0.0.1", "-p", port);
  }
});

test("phone: a card opens its pane, Back the list", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => localStorage.getItem("keepane-mode") === null && localStorage.setItem("keepane-mode", "dark"));
  await app.open(page(portOf(app.baseUrl)));
  const finger = await phone(browser);
  await expect.poll(() => pageText(browser)).toContain("builder");
  await finger.tap(browser.locator('[data-pane][data-name="builder"]'));
  await expect(browser.locator("pre.term")).toBeVisible();
  expect(await pageText(browser)).not.toContain("0: editor");
  await finger.tap(screen.getByRole("button", "返回"));
  await expect.poll(() => pageText(browser)).toContain("0: editor");
  await expect(browser.locator("pre.term")).toHaveCount(0);
  await finger.close();
});
