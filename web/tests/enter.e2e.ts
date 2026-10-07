// The box that types on Enter, and the one that runs; the keys that stay in
// view; buttons big enough for a finger; the pane named in full screen.
import { expect } from "e2e";
import { box, entry, pageText, stays, test, ticked } from "../e2e/fixtures.ts";
import { capture, page, paneNumber, portOf, token } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const B = () => `%${paneNumber("work:0.1")}`;
const shows = (s: string) => expect.poll(() => capture(B())).toContain(s);

test("Enter types, or runs; ^C and ⏎ always in view", async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    if (localStorage.getItem("keepane-input") === null) localStorage.setItem("keepane-input", JSON.stringify("type"));
  });
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  await expect(screen.getByPlaceholder("回车填入")).toBeVisible();
  const u = token();

  // Pinned keys stay in view whatever the key row's scroll.
  const pinned = () =>
    browser.evaluate(() => {
      const ks = [...document.querySelectorAll("button")].filter((b) => ["^C", "⏎"].includes(b.textContent!.trim()));
      return ks.length === 2 && ks.every((e) => {
        const r = e.getBoundingClientRect();
        return r.width > 0 && r.left >= -0.5 && r.right <= innerWidth + 0.5;
      });
    });
  await expect.poll(pinned).toBe(true);
  await browser.evaluate(() => {
    for (const e of document.querySelectorAll("*")) if (e.scrollWidth > e.clientWidth + 4 && getComputedStyle(e).overflowX !== "visible") e.scrollLeft = e.scrollWidth;
    return null;
  });
  await expect.poll(pinned).toBe(true);
  const small = await browser.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>("button, [role=button]")]
      .filter((e) => e.offsetParent && e.getBoundingClientRect().right > 0 && e.getBoundingClientRect().left < innerWidth)
      .map((e) => {
        const r = e.getBoundingClientRect();
        return [Math.round(r.width), Math.round(r.height), (e.getAttribute("aria-label") || e.textContent!.trim()).slice(0, 14)] as [number, number, string];
      })
      .filter(([w, h]) => w < 40 || h < 40),
  );
  expect(small, "buttons in view under 40 px").toEqual([]);

  // Type only: Enter puts the line there, a second Enter runs it.
  const send = browser.locator("#send");
  await box(browser).pressSequentially(`Write-Output "kpA${u}$(6*7)"`);
  await expect(send).toHaveAccessibleName("发送");
  await browser.keyboard.press("Enter");
  await shows(`kpA${u}$(6*7)`);
  await stays("type only: not run", () => !capture(B()).includes(`kpA${u}42`), 1200);
  await expect(send).toHaveAccessibleName("回车");
  await finger.tap(send);
  await shows(`kpA${u}42`);

  // Type and run.
  await finger.tap(screen.getByRole("button", "视图"));
  await finger.tap(entry(screen, "填入并执行"));
  await expect(screen.getByPlaceholder("回车执行")).toBeVisible();
  await box(browser).pressSequentially(`Write-Output "kpB${u}$(6*7)"`);
  await expect(send).toHaveAccessibleName("执行");
  await browser.keyboard.press("Enter");
  await shows(`kpB${u}42`);
  await box(browser).pressSequentially(`Write-Output "kpC${u}$(6*7)"`);
  await finger.tap(send);
  await shows(`kpC${u}42`);
  await stays("one run, no second Enter", () => (capture(B()).match(new RegExp(`^kpC${u}42$`, "gm")) || []).length === 1);

  await browser.reload();
  await expect(screen.getByPlaceholder("回车执行")).toBeVisible();
  await finger.tap(screen.getByRole("button", "视图"));
  await expect(ticked(browser).filter({ hasText: "填入并执行" })).toBeVisible();
  await finger.tap(entry(screen, "只填入（再回车执行）"));
  await expect(screen.getByPlaceholder("回车填入")).toBeVisible();

  // Full screen: the pane named above the box.
  await finger.tap(browser.locator("#fullscreen"));
  await expect.poll(() => pageText(browser)).toContain("输入到 builder · work:0.1");
  await finger.close();
});
