// A full-screen program (an agent, an editor): a drag on the phone and the
// mouse wheel reach it as its wheel; Shift and Ctrl held go with the keys.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect } from "e2e";
import { afterAll, beforeAll } from "@e2e-dev/web";
import { box, pageText, stays, test } from "../e2e/fixtures.ts";
import { DIR, kp, page, paneNumber, portOf, show } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const reader = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "e2e", "alt-reader.cjs");
const log = path.join(DIR, "alt-reader.log");
const got = () => (fs.existsSync(log) ? fs.readFileSync(log, "latin1") : "");
// A step: a wheel event (the program asked for the mouse), or three arrow keys
// (it did not, or Windows' ConPTY did not pass the asking on).
const ups = () => (got().match(/\x1b\[<64;/g) || []).length + Math.floor((got().match(/\x1b\[A|\x1bOA/g) || []).length / 3);
const downs = () => (got().match(/\x1b\[<65;/g) || []).length + Math.floor((got().match(/\x1b\[B|\x1bOB/g) || []).length / 3);

beforeAll(async () => {
  kp("new-window", "-d", "-t", "work", "-n", "full", "node", reader, log);
  await expect.poll(() => show("work:full", "#{alternate_on}"), { timeout: 15_000 }).toBe("1");
});
afterAll(() => {
  // Only if it was made: a failed start keeps its own error.
  if (/^\d+: full\b/m.test(kp("list-windows", "-t", "work"))) kp("kill-window", "-t", "work:full");
});

test("a drag on the phone is the program's wheel; Shift goes with a key", async ({ app, browser, screen }) => {
  const full = paneNumber("work:full");
  await app.open(page(portOf(app.baseUrl), full));
  const finger = await phone(browser);
  await expect.poll(() => pageText(browser)).toContain("全屏程序：滑动交给它滚动");
  const m = await finger.middle(browser.locator("#main"));
  const drag = (dy: number) => finger.swipe({ x: m.x, y: m.y - dy / 2 }, { x: m.x + 2, y: m.y + dy / 2 }, 8);

  const [u0, d0] = [ups(), downs()];
  await drag(150);
  await expect.poll(() => ups() - u0, { message: "a drag down of 150 px: 4 wheel steps up" }).toBe(4);
  await drag(-80);
  await expect.poll(() => downs() - d0, { message: "a drag up of 80 px: 2 steps down" }).toBe(2);
  // Mostly sideways, though it drifts down more than one wheel step: a swipe
  // to the next pane, never the wheel.
  const before = got().length;
  await finger.swipe({ x: m.x + 120, y: m.y - 25 }, { x: m.x - 80, y: m.y + 25 }, 8);
  await stays("a sideways swipe sends no wheel", () => got().length === before);

  // Back on the program's pane (the swipe went to the next one): a fresh
  // load, as the page reads the pane in its address when it loads.
  await app.open(page(portOf(app.baseUrl), full));
  await browser.reload();
  await expect.poll(() => pageText(browser)).toContain("全屏程序");
  const from = got().length;
  const since = () => got().slice(from);
  const shift = screen.getByRole("button", "Shift");
  await finger.tap(shift);
  await expect(shift).toHaveAttribute("aria-pressed", "true");
  await finger.tap(screen.getByRole("button", "→"));
  await expect.poll(since, { message: "Shift then →: ESC[1;2C" }).toContain("\x1b[1;2C");
  await expect(shift).toHaveAttribute("aria-pressed", "false");
  await finger.tap(screen.getByRole("button", "Ctrl"));
  await finger.tap(shift);
  await finger.tap(screen.getByRole("button", "←"));
  await expect.poll(since, { message: "Ctrl+Shift then ←: ESC[1;6D" }).toContain("\x1b[1;6D");
  await finger.tap(shift);
  await finger.tap(screen.getByRole("button", "Tab"));
  await expect.poll(since, { message: "Shift then Tab: ESC[Z" }).toContain("\x1b[Z");
  await finger.tap(shift);
  await box(browser).pressSequentially("a");
  await expect(box(browser)).toHaveValue("A");
  await finger.close();
});

test("a shell at its prompt is not called full screen", async ({ app, browser }) => {
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  await phone(browser);
  await expect(browser.locator("#screen")).toBeVisible();
  await stays("not called full screen", async () => !(await pageText(browser)).includes("全屏程序"));
});

test("the mouse wheel over a full-screen program", async ({ app, browser }) => {
  await app.open(page(portOf(app.baseUrl), paneNumber("work:full")));
  const b = await browser.locator("#main").boundingBox();
  await browser.mouse.move(b!.x + b!.width / 2, b!.y + b!.height / 2);
  const u0 = ups();
  await browser.mouse.wheel(0, -180);
  await expect.poll(() => ups() - u0, { message: "wheel up 180: 3 steps up" }).toBe(3);
});
