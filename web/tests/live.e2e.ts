// Live typing: what is typed in the box is on the program's line at once,
// a backspace takes a character off it, Enter is Enter.
import { expect } from "e2e";
import { box, entry, noNotice, stays, test } from "../e2e/fixtures.ts";
import { capture, kp, lastLine, page, paneNumber, portOf, token } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const B = () => `%${paneNumber("work:0.1")}`;
const endsWith = (s: string) => expect.poll(() => lastLine(B())).toMatch(new RegExp(`${s.replace(/[$()*+.?[\\\]^{|}]/g, "\\$&")}$`));
const ranTo = (s: string) => expect.poll(() => capture(B()).split("\n").map((l) => l.trim())).toContain(s);

test("typed in the box: on the program's line at once", async ({ app, browser, screen }) => {
  kp("send-keys", "-t", B(), "C-c");
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  await expect(screen.getByPlaceholder("实时输入，回车即回车")).toBeVisible();
  const k = `kp${token()}`;

  await box(browser).pressSequentially(`Write-Output ${k}Live`);
  await endsWith(`Write-Output ${k}Live`);
  for (let i = 0; i < 4; i++) await box(browser).press("Backspace");
  await endsWith(`Write-Output ${k}`);
  await box(browser).pressSequentially("Q1");
  await endsWith(`Write-Output ${k}Q1`);
  // An edit in the middle: the line follows.
  await browser.evaluate(() => ((document.querySelector("#text") as HTMLTextAreaElement).setSelectionRange(6, 6), null));
  await browser.keyboard.type("Z");
  await endsWith(`Write-ZOutput ${k}Q1`);
  await browser.keyboard.press("Backspace");
  await endsWith(`Write-Output ${k}Q1`);
  await browser.keyboard.press("Enter");
  await ranTo(`${k}Q1`);
  await expect(box(browser)).toHaveValue("");

  // Typed and Enter at once: the last of it (still waiting to go) goes first.
  await box(browser).pressSequentially(`Write-Output ${k}Fast`);
  await browser.keyboard.press("Enter");
  await ranTo(`${k}Fast`);

  // An input method composing: nothing goes until it is done.
  await browser.evaluate(() => {
    const el = document.querySelector("#text") as HTMLTextAreaElement;
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    el.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    set.call(el, "ni");
    el.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true, data: "ni", inputType: "insertCompositionText" }));
    return null;
  });
  await stays("nothing goes out while composing", () => !lastLine(B()).endsWith("ni"));
  await browser.evaluate(() => {
    const el = document.querySelector("#text") as HTMLTextAreaElement;
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    set.call(el, "你");
    el.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true, data: "你", inputType: "insertCompositionText" }));
    el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "你" }));
    return null;
  });
  await endsWith("你");
  await finger.tap(screen.getByRole("button", "^C"));
  await expect(box(browser)).toHaveValue("");

  // A key the box cannot follow: the box starts afresh, the program keeps its line.
  await box(browser).pressSequentially("ab");
  await endsWith("ab");
  await finger.tap(screen.getByRole("button", "←"));
  await expect(box(browser)).toHaveValue("");
  await box(browser).press("Backspace");
  // One backspace for the program: before its cursor is the a (PowerShell may
  // grey in a guess from its history after the b).
  await expect.poll(() => lastLine(B())).toMatch(/(^|> ?)b/);
  expect(lastLine(B())).not.toContain("ab");
  await finger.tap(screen.getByRole("button", "^C"));

  // Ctrl with a letter typed (C-a: the line's cursor to its start): afresh too.
  const keys: string[] = [];
  await browser.route(/\/api\/send\?/, async (r) => {
    keys.push(decodeURIComponent(r.request.url.split("?")[1]));
    await r.continue();
  });
  await box(browser).pressSequentially("xy");
  await endsWith("xy");
  await finger.tap(screen.getByRole("button", "Ctrl"));
  await box(browser).pressSequentially("a");
  await expect.poll(() => keys.some((k) => k.endsWith("key=C-a"))).toBe(true);
  await expect(box(browser)).toHaveValue("");
  await box(browser).pressSequentially("W");
  await expect.poll(() => lastLine(B())).toMatch(/W/);
  await finger.tap(screen.getByRole("button", "^C"));

  // Type only: nothing goes until Enter.
  await noNotice(browser);
  await finger.tap(screen.getByRole("button", "视图"));
  await finger.tap(entry(screen, "只填入（再回车执行）"));
  await expect(screen.getByPlaceholder("回车填入")).toBeVisible();
  await expect.poll(() => lastLine(B())).toMatch(/>\s*$/);
  const before = lastLine(B());
  await box(browser).pressSequentially(`Write-Output ${k}Wait`);
  await stays("type only: nothing goes out while typing", () => lastLine(B()) === before);
  await browser.keyboard.press("Enter");
  await endsWith(`Write-Output ${k}Wait`);
  await finger.tap(screen.getByRole("button", "^C"));
  await finger.close();
});

test("a device that chose before keeps its choice", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => localStorage.setItem("keepane-enter-runs", "true"));
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  await expect(screen.getByPlaceholder("回车执行")).toBeVisible();
});
