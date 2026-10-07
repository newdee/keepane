// What every browser test shares: `test` that fails on anything the page
// threw, and the few steps the tests repeat.
import { test as base } from "@e2e-dev/web";
import type { Browser } from "@e2e-dev/web";
import { expect, type Locator, type Screen } from "e2e";

/** `test` from the web engine, plus: nothing thrown by the page, reloads included. */
export const test = base.extend<{ pageErrors: void }>({
  pageErrors: async ({ browser }, use) => {
    await use();
    const url = await browser.url().catch(() => "about:blank");
    if (!url.startsWith("http")) return;
    const thrown = await browser.evaluate(() => JSON.parse(sessionStorage.getItem("e2e-errors") || "[]") as string[]);
    expect(thrown, "the page threw").toEqual([]);
  },
});

/**
 * That something stays true for `ms`: for what must NOT happen (nothing sent
 * while an input method composes). A wait for what must happen is `expect`.
 */
export async function stays(what: string, holds: () => boolean | Promise<boolean>, ms = 800): Promise<void> {
  const end = Date.now() + ms;
  for (;;) {
    expect(await holds(), what).toBe(true);
    if (Date.now() >= end) return;
    await new Promise((r) => setTimeout(r, 100));
  }
}

/**
 * A person's pace between two taps of one control: the page takes a second
 * tap within 250 ms as the first one bouncing (format.ts, bounced).
 */
export const pace = () => new Promise((r) => setTimeout(r, 300));

/** A menu's entry by its words. */
export const entry = (screen: Screen, name: string): Locator => screen.getByRole("menuitem", name);

/** The entries the open menu ticks (the one chosen of a few, in each group). */
export const ticked = (browser: Browser): Locator => browser.locator('[role="menuitem"]:has(svg.text-accent)');

/** No notice left at the top (one covers the pane's bar for a few seconds). */
export const noNotice = (browser: Browser) => expect(browser.locator('[data-slot="toast"]')).toHaveCount(0, { timeout: 10_000 });

/**
 * The dialog by its title. By name: a menu's popover is a dialog too, and
 * stays a moment while it fades out.
 */
export const dialog = (screen: Screen, name: string | RegExp): Locator => screen.getByRole("dialog", { name, visible: true });

/** The box under the screen. */
export const box = (browser: Browser): Locator => browser.locator("#text");

/** The text of the page as a person reads it. */
export const pageText = (browser: Browser): Promise<string> => browser.evaluate(() => document.body.innerText);
