// The box is kept for each pane: left for another pane and come back to (or
// the page reloaded), what was typed is still there, and live, the program's
// line is not typed into again.
import { expect } from "e2e";
import type { Browser } from "@e2e-dev/web";
import { box, entry, noNotice, stays, test } from "../e2e/fixtures.ts";
import { capture, closeIfThere, kp, lastLine, page, paneNumber, portOf, show, token } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const B = () => `%${paneNumber("work:0.1")}`;
const N = () => `%${paneNumber("notes:0.0")}`;
const endsWith = (s: string) => expect.poll(() => lastLine(B())).toMatch(new RegExp(`${s.replace(/[$()*+.?[\\\]^{|}]/g, "\\$&")}$`));
const ranTo = (s: string) => expect.poll(() => capture(B()).split("\n").map((l) => l.trim())).toContain(s);
const kept = (browser: Browser) => browser.evaluate(() => sessionStorage.getItem("keepane-drafts") || "{}");
const shown = (browser: Browser) => browser.evaluate(() => !!document.querySelector("#screen"));

test("the box is kept for its pane", async ({ app, browser, screen }) => {
  kp("send-keys", "-t", B(), "C-c");
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  await expect(screen.getByPlaceholder("实时输入，回车即回车")).toBeVisible();
  const k = `kd${token()}`;
  const to = async (pane: string) => {
    await finger.tap(browser.locator("#back"));
    await expect.poll(() => shown(browser)).toBe(false);
    await finger.tap(browser.locator(`[data-pane="${pane}"]`));
    await expect.poll(() => shown(browser)).toBe(true);
  };

  // Live: typed, another pane, back. The box has it, the line has it once.
  await box(browser).pressSequentially(`Write-Output ${k}`);
  await endsWith(`Write-Output ${k}`);
  await to(N());
  await expect(box(browser)).toHaveValue("");
  await to(B());
  await expect(box(browser)).toHaveValue(`Write-Output ${k}`);
  // Its cursor at the end.
  await box(browser).pressSequentially("X");
  await endsWith(`Write-Output ${k}X`);
  expect(lastLine(B()), "the line was not typed into again").not.toContain(`${k}Write`);

  // A reload of the page: still there. Enter runs it; the box and its keeping go.
  await browser.reload();
  await expect(box(browser)).toHaveValue(`Write-Output ${k}X`);
  await box(browser).press("Enter");
  await ranTo(`${k}X`);
  await expect(box(browser)).toHaveValue("");
  await expect.poll(() => kept(browser)).not.toContain(B());

  // Typed and the page reloaded before it went out: kept as typed, and the
  // rest goes out with the next change (the line not typed into twice).
  await box(browser).pressSequentially(`${k}R`);
  await endsWith(`${k}R`);
  await browser.evaluate(() => {
    const el = document.querySelector("#text") as HTMLTextAreaElement;
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    set.call(el, el.value + "S");
    el.dispatchEvent(new InputEvent("input", { bubbles: true, data: "S", inputType: "insertText" }));
    location.reload();
    return null;
  });
  await expect(box(browser)).toHaveValue(`${k}RS`);
  await box(browser).pressSequentially("T");
  await endsWith(`${k}RST`);
  expect(lastLine(B())).not.toContain(`${k}R${k}`);
  await finger.tap(screen.getByRole("button", "^C"));
  await expect(box(browser)).toHaveValue("");

  // Typed and away at once (before it went out): its timer still sends it.
  await box(browser).pressSequentially(`${k}Q`, { delay: 0 });
  await finger.tap(browser.locator("#back"));
  await endsWith(`${k}Q`);
  await finger.tap(browser.locator(`[data-pane="${B()}"]`));
  await expect(box(browser)).toHaveValue(`${k}Q`);
  await box(browser).press("Backspace");
  await endsWith(`${k}`);
  expect(lastLine(B())).not.toContain(`${k}Q`);
  await finger.tap(screen.getByRole("button", "^C"));
  await expect(box(browser)).toHaveValue("");

  // Type only: kept, and nothing goes out on the way.
  await noNotice(browser);
  await finger.tap(screen.getByRole("button", "视图"));
  await finger.tap(entry(screen, "只填入（再回车执行）"));
  await expect(screen.getByPlaceholder("回车填入")).toBeVisible();
  await expect.poll(() => lastLine(B())).toMatch(/>\s*$/);
  const before = lastLine(B());
  await box(browser).pressSequentially(`Write-Output ${k}T`);
  await to(N());
  await to(B());
  await expect(box(browser)).toHaveValue(`Write-Output ${k}T`);
  await stays("type only: nothing went out", () => lastLine(B()) === before);
  await box(browser).press("Enter");
  await endsWith(`Write-Output ${k}T`);
  await finger.tap(screen.getByRole("button", "^C"));

  // Kept live, back to it under type only: afresh (as switching the mode does).
  await finger.tap(screen.getByRole("button", "视图"));
  await finger.tap(entry(screen, "实时输入（边打边发）"));
  await expect(screen.getByPlaceholder("实时输入，回车即回车")).toBeVisible();
  await box(browser).pressSequentially(`${k}L`);
  await endsWith(`${k}L`);
  await to(N());
  await browser.evaluate(() => (localStorage.setItem("keepane-input", JSON.stringify("type")), null));
  await browser.reload();
  await expect(screen.getByPlaceholder("回车填入")).toBeVisible();
  await to(B());
  await expect(box(browser)).toHaveValue("");
  await finger.tap(screen.getByRole("button", "^C"));
  await browser.evaluate(() => (localStorage.setItem("keepane-input", JSON.stringify("live")), null));

  // A pane closed: what was kept for it goes.
  kp("split-window", "-d", "-t", "notes");
  const gone = show("notes:0.1", "#{pane_id}");
  try {
    await browser.reload();
    await to(gone);
    await box(browser).pressSequentially(`${k}G`);
    await expect.poll(() => kept(browser)).toContain(`"${gone}"`);
    await finger.tap(browser.locator("#back"));
  } finally {
    closeIfThere(gone);
  }
  await expect.poll(() => kept(browser), { timeout: 10_000 }).not.toContain(`"${gone}"`);
  await finger.close();
});

test("a keeping that is not keepane's is started afresh", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => localStorage.setItem("keepane-input", JSON.stringify("type")));
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  for (const bad of ["[1,2]", "not json", '{"%1":5,"%2":{"text":7}}', "null", '"x"']) {
    await browser.evaluate((v) => (sessionStorage.setItem("keepane-drafts", v), null), bad);
    await browser.reload();
    await expect(screen.getByPlaceholder("回车填入")).toBeVisible();
    await expect(box(browser)).toHaveValue("");
    await box(browser).pressSequentially("kept");
    await finger.tap(browser.locator("#back"));
    await finger.tap(browser.locator(`[data-pane="${B()}"]`));
    await expect(box(browser)).toHaveValue("kept");
    await expect.poll(() => kept(browser), { message: bad }).toContain(`"${B()}"`);
    await box(browser).fill("");
  }
  await finger.close();
});
