// The Esc button: each click with a mouse reaches the program in the pane as
// one Escape, a second click soon after included (an agent's "Esc twice"). On
// a touch screen, one tap reported twice is one Escape; two quick taps are two.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect } from "e2e";
import { afterAll, beforeAll } from "@e2e-dev/web";
import { stays, test } from "../e2e/fixtures.ts";
import { DIR, kp, page, paneNumber, portOf, show } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const reader = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "e2e", "alt-reader.cjs");
const log = path.join(DIR, "esc-reader.log");
const got = () => (fs.existsSync(log) ? fs.readFileSync(log, "latin1") : "");
// An Escape on its own: ESC not starting a sequence (ESC [ or ESC O).
const escapes = () => (got().match(/\x1b(?![[O])/g) || []).length;

beforeAll(async () => {
  kp("new-window", "-d", "-t", "work", "-n", "esc", "node", reader, log);
  await expect.poll(() => show("work:esc", "#{alternate_on}"), { timeout: 15_000 }).toBe("1");
});
afterAll(() => {
  if (/^\d+: esc\b/m.test(kp("list-windows", "-t", "work"))) kp("kill-window", "-t", "work:esc");
});

test("each click on Esc is one Escape for the program", async ({ app, screen }) => {
  await app.open(page(portOf(app.baseUrl), paneNumber("work:esc")));
  const esc = screen.getByRole("button", "Esc");
  await expect(esc).toBeVisible();
  const before = escapes();
  for (let i = 1; i <= 5; i++) {
    await esc.click();
    await expect.poll(escapes, { message: `click ${i}: one more Escape` }).toBe(before + i);
  }
  // Twice in a row, as for an agent's "Esc twice": two.
  await esc.click();
  await esc.click();
  await expect.poll(escapes, { message: "two clicks in a row: two Escapes" }).toBe(before + 7);
});

test("on a touch screen a bouncing tap is one Escape, two quick taps are two", async ({ app, browser, screen }) => {
  await app.open(page(portOf(app.baseUrl), paneNumber("work:esc")));
  const finger = await phone(browser);
  const esc = screen.getByRole("button", "Esc");
  await expect(esc).toBeVisible();
  const before = escapes();
  await finger.tap(esc);
  await expect.poll(escapes, { message: "a tap: one Escape" }).toBe(before + 1);
  // One tap reported twice (back to back): still one.
  await finger.twice(esc, 0);
  await expect.poll(escapes, { message: "a bouncing tap: one Escape" }).toBe(before + 2);
  await stays("a bouncing tap: not a second Escape", () => escapes() === before + 2);
  // Two taps 150 ms apart, as quick as a person taps: two.
  await finger.twice(esc, 150);
  await expect.poll(escapes, { message: "two quick taps: two Escapes" }).toBe(before + 4);
  await finger.close();
});
