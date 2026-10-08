// Messages and modes, copying, text size and find, a new session, a pane
// run again.
import { expect } from "e2e";
import { dialog, box, entry, noNotice, pageText, test, ticked } from "../e2e/fixtures.ts";
import fs from "node:fs";
import path from "node:path";
import { CLAUDE_DIR, DIR, capture, kp, page, paneNumber, portOf, show, token, withOption } from "../e2e/keepane.ts";
import { phone, type Fingers } from "../e2e/phone.ts";
import type { Browser } from "@e2e-dev/web";
import type { Screen } from "e2e";

const B = () => `%${paneNumber("work:0.1")}`;

/** An entry of the pane's View menu. */
const view = async (finger: Fingers, browser: Browser, screen: Screen, name: string) => {
  await noNotice(browser);
  await finger.tap(screen.getByRole("button", "视图"));
  await finger.tap(entry(screen, name));
};

test("a message to a shell pane, and its mode", async ({ app, browser, screen }) => {
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  const tell = browser.locator("#tell");
  await finger.tap(tell);
  await expect(screen.getByPlaceholder("消息给 builder")).toBeVisible();
  await expect(browser.locator("#send")).toHaveAccessibleName("排队");
  const u = token();
  await box(browser).pressSequentially(`Write-Output "kpM${u}$(6*7)"`);
  await finger.tap(browser.locator("#send"));
  // The notice answering it says its number.
  const notice = browser.locator('[data-slot="toast"]').filter({ hasText: "#" });
  await expect(notice).toContainText(/#\d+/);
  const num = /#(\d+)/.exec((await notice.textContent()) ?? "")![1];
  await expect.poll(() => capture(B()), { timeout: 15_000, message: "the shell pane ran it" }).toContain(`kpM${u}42`);
  const trace = kp("trace-message", num);
  expect(trace, "this message").toContain(`kpM${u}`);
  expect(trace, "sent as the user").toMatch(/user/);
  await finger.tap(tell);
  await expect(screen.getByPlaceholder("实时输入，回车即回车")).toBeVisible();

  // The pane's ⋯ menu: the mode chosen ticked; ai, then back to shell.
  await noNotice(browser);
  await finger.tap(screen.getByRole("button", "更多"));
  await expect(ticked(browser).filter({ hasText: "shell：在提示符下执行" })).toBeVisible();
  await finger.tap(entry(screen, "ai：交给 agent"));
  await expect.poll(() => show(B(), "#{pane_work_mode}")).toBe("ai");
  await noNotice(browser);
  await finger.tap(screen.getByRole("button", "更多"));
  await finger.tap(entry(screen, "shell：在提示符下执行"));
  await expect.poll(() => show(B(), "#{pane_work_mode}")).toBe("shell");

  // A normal pane: no message switch.
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.0")));
  await browser.reload();
  await expect(browser.locator("#screen")).toBeVisible();
  await expect(tell).toHaveCount(0);
  await finger.close();
});

/** What each copy puts in the clipboard: what the clipboard was given, or
 *  the hidden box's selection (the old way, on plain http). */
const recordCopies = (browser: Browser, secure: boolean) =>
  browser.addInitScript((secure: boolean) => {
    const w = window as unknown as { __copied: string[] };
    w.__copied = [];
    addEventListener("copy", (e) => {
      const a = document.activeElement as HTMLTextAreaElement | null;
      const sel = a && a.tagName === "TEXTAREA" ? a.value.substring(a.selectionStart, a.selectionEnd) : String(getSelection());
      w.__copied.push(e.clipboardData?.getData("text/plain") || sel);
    });
    if (!secure) Object.defineProperty(window, "isSecureContext", { get: () => false });
    else if (navigator.clipboard) navigator.clipboard.writeText = async (s) => void w.__copied.push(s);
  }, secure);
const lastCopy = (browser: Browser) => browser.evaluate(() => (window as unknown as { __copied: string[] }).__copied.at(-1) ?? "");

/** A tap on the time beside line `i` of the screen (the command's): its menu. */
const tapStamp = async (browser: Browser, finger: Fingers, i: number) => {
  const at = await browser.evaluate((i) => {
    const s = document.querySelector(`.stamp[data-i="${i}"]`);
    if (!s) return null;
    s.scrollIntoView({ block: "center" });
    const r = s.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }, i);
  expect(at, `a time on line ${i}`).not.toBeNull();
  await finger.tap(at!);
};

/** The screen's line (as the stamps count them: its text, line by line) with `text` on it, the last such. */
const lineOf = (browser: Browser, text: string) =>
  browser.evaluate((text) => document.querySelector("#screen")!.textContent!.split("\n").findLastIndex((l) => l.includes(text)), text);

/** The times on, if they are not. */
const stampsOn = async (finger: Fingers, browser: Browser, screen: Screen) => {
  if ((await browser.locator(".stamp").count()) === 0) await view(finger, browser, screen, "每条命令的时间");
  await expect.poll(() => browser.locator(".stamp").count()).toBeGreaterThan(0);
};

for (const secure of [true, false]) {
  const how = secure ? "secure page" : "plain http";
  test(`copying, ${how}`, async ({ app, browser, screen }) => {
    await recordCopies(browser, secure);
    // A command, and one after it.
    const [o1, o2, o3] = ["a", "b", "c"].map((s) => `kp${token()}${s}`);
    const command = `Write-Output ${o1} ${o2}`;
    kp("send-keys", "-t", B(), command, "Enter");
    kp("send-keys", "-t", B(), `Write-Output ${o3}`, "Enter");
    await expect.poll(() => capture(B())).toMatch(new RegExp(`\\n${o3}`));
    await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
    const finger = await phone(browser);
    await expect(browser.locator("#screen")).toContainText(o3);

    await view(finger, browser, screen, "复制屏幕文字");
    await expect.poll(() => lastCopy(browser), { message: "the screen copied" }).toContain(o1);
    expect(await lastCopy(browser), "no escapes copied").not.toContain("\x1b");
    await stampsOn(finger, browser, screen);
    const line = await lineOf(browser, command);
    await tapStamp(browser, finger, line);
    await finger.tap(entry(screen, "复制这条命令的输出"));
    await expect.poll(() => lastCopy(browser), { message: "what it printed: just its two lines" }).toBe(`${o1}\n${o2}`);
    await tapStamp(browser, finger, line);
    await finger.tap(entry(screen, "复制这条命令"));
    await expect.poll(() => lastCopy(browser), { message: "the command, the prompt left out" }).toBe(command);
    await expect.poll(() => pageText(browser)).toContain("已复制");
    await finger.close();
  });
}

// Long lines wrapped here or not: the page reads the pane's rows, or its
// lines joined (`capture-pane -J`), and the marks are placed on those. It
// wraps (the default) when the pane is too wide to read at this width: a
// screen narrower than a phone's, for this narrow pane.
for (const wrap of [false, true])
test(
  `copying under a prompt and a command wider than the pane${wrap ? ", long lines wrapped" : ""}`,
  async ({ app, browser, screen }) => {
    await recordCopies(browser, true);
    // A narrow pane of its own (work's are wide).
    kp("split-window", "-h", "-d", "-t", "notes");
    const narrow = show("notes:0.1", "#{pane_id}");
    try {
      const cols = Number(show(narrow, "#{pane_width}"));
      await expect.poll(() => capture(narrow)).toMatch(/PS /);
      const prompt = `PS ${show(narrow, "#{pane_current_path}")}> `;
      expect(prompt.length, "the prompt is wider than the pane").toBeGreaterThan(cols);
      // A command wider than the pane, and one after it.
      const command = `Write-Output kpW1 kpW2 # ${"w".repeat(cols)}`;
      kp("send-keys", "-t", narrow, command, "Enter");
      kp("send-keys", "-t", narrow, "Write-Output kpW3", "Enter");
      await expect.poll(() => capture(narrow)).toMatch(/\nkpW3/);
      await app.open(page(portOf(app.baseUrl), narrow.replace("%", "")));
      const finger = await phone(browser, { width: wrap ? 240 : 390, height: 844 });
      await expect(browser.locator("#screen")).toContainText("kpW3");
      await stampsOn(finger, browser, screen);
      const wrapped = () => browser.evaluate(() => document.querySelector("#screen")!.classList.contains("wrap"));
      await expect.poll(wrapped, { message: wrap ? "wrapped here" : "rows as the pane has them" }).toBe(wrap);
      await expect.poll(() => browser.locator(".stamp").count()).toBeGreaterThanOrEqual(2);
      const [long, last] = (await browser.evaluate(() => [...document.querySelectorAll<HTMLElement>(".stamp")].slice(-2).map((s) => s.dataset.i ?? ""))).map(Number);
      await tapStamp(browser, finger, long);
      await finger.tap(entry(screen, "复制这条命令的输出"));
      await expect.poll(() => lastCopy(browser), { message: "what it printed, up to the next prompt" }).toBe("kpW1\nkpW2");
      await tapStamp(browser, finger, long);
      await finger.tap(entry(screen, "复制这条命令"));
      await expect.poll(() => lastCopy(browser), { message: "the command, all of it" }).toBe(command);
      await tapStamp(browser, finger, last);
      await finger.tap(entry(screen, "复制这条命令的输出"));
      await expect.poll(() => lastCopy(browser), { message: "the last one's: no prompt after it" }).toBe("kpW3");
      await finger.close();
    } finally {
      kp("kill-pane", "-t", narrow);
    }
  },
);

test("text size: the menu, a pinch, kept; find in the output", async ({ app, browser, screen }) => {
  // A word to find: on the command's line and the two it printed.
  const word = `kpf${token()}`;
  kp("send-keys", "-t", B(), `Write-Output ${word}1 ${word}2`, "Enter");
  await expect.poll(() => capture(B())).toMatch(new RegExp(`\\n${word}2`));
  await app.open(page(portOf(app.baseUrl), paneNumber("work:0.1")));
  const finger = await phone(browser);
  await expect(browser.locator("#screen")).toBeVisible();
  const font =() => browser.evaluate(() => parseFloat(document.querySelector<HTMLElement>("#screen")!.style.fontSize));
  const f0 = await font();
  await view(finger, browser, screen, "放大");
  await expect.poll(async () => Math.abs((await font()) / f0 - 1.15) < 0.02, { message: "A+: × 1.15" }).toBe(true);
  await view(finger, browser, screen, "恢复 100%");
  await expect.poll(async () => Math.abs((await font()) - f0) < 0.05).toBe(true);
  // Two fingers apart on the screen.
  await finger.pinch(await finger.middle(browser.locator("#main")), 80, 160);
  await expect.poll(async () => (await font()) / f0, { message: "a pinch apart: about twice as big" }).toBeGreaterThan(1.7);
  expect(await browser.evaluate(() => window.visualViewport?.scale ?? 1), "the page itself not zoomed").toBe(1);
  await browser.reload();
  await expect.poll(async () => (await font()) / f0, { message: "kept after a reload" }).toBeGreaterThan(1.7);
  await view(finger, browser, screen, "恢复 100%");
  await expect.poll(async () => Math.abs((await font()) - f0) < 0.05).toBe(true);

  // Find.
  await view(finger, browser, screen, "在输出里查找");
  const find = browser.locator("#find");
  await expect(find).toBeFocused();
  // In capitals: finding ignores case.
  await browser.keyboard.type(word.toUpperCase());
  const want = capture(B()).split("\n").filter((l) => l.includes(word)).length;
  expect(want, "the word's lines").toBe(3);
  const count = browser.locator("#find-count");
  await expect(count).toHaveText(`${want}/${want}`);
  await expect(browser.locator("#screen .hit")).toHaveCount(want);
  const inView = () =>
    browser.evaluate(() => {
      const h = document.querySelector("#screen .hit.now")!.getBoundingClientRect();
      const m = document.querySelector("#main")!.getBoundingClientRect();
      return h.top >= m.top && h.bottom <= m.bottom;
    });
  await expect.poll(inView, { message: "the line found is on the screen" }).toBe(true);
  await finger.tap(browser.locator("#find-up"));
  await expect(count).toHaveText(`${want - 1}/${want}`);
  await expect.poll(inView).toBe(true);
  await finger.tap(browser.locator("#find-down"));
  await finger.tap(browser.locator("#find-down"));
  // ↓ past the last: round to the first.
  await expect(count).toHaveText(`1/${want}`);
  await find.pressSequentially("zzzz-nothing");
  await expect(count).toHaveText("没有");
  await find.press("Escape");
  await expect(find).toHaveCount(0);
  await expect(browser.locator("#screen .hit")).toHaveCount(0);
  await finger.close();
});

test("a new session from the phone, and a pane run again", async ({ app, browser, screen }) => {
  await app.open(page(portOf(app.baseUrl)));
  const finger = await phone(browser);
  await finger.tap(browser.locator("#new-session"));
  const shown = dialog(screen, "新 session");
  await expect(shown.getByRole("textbox")).toBeFocused();
  await browser.keyboard.type("fresh");
  await finger.tap(shown.getByRole("button", "新建"));
  try {
    await expect.poll(() => kp("list-sessions")).toContain("fresh");
    await expect(browser.locator("#screen")).toBeVisible();
    await expect.poll(() => pageText(browser)).toContain("fresh:0.0");
  } finally {
    // Only if it was made: a failed start keeps its own error.
    if (/^fresh:/m.test(kp("list-sessions"))) kp("kill-session", "-t", "fresh");
  }

  // A pane whose program ended (remain-on-exit).
  await withOption("remain-on-exit", "on", async () => {
    kp("split-window", "-d", "-t", "notes", "pwsh", "-NoProfile", "-Command", "exit 3");
    const dead = show("notes:0.1", "#{pane_id}");
    try {
      await expect.poll(() => show(dead, "#{pane_dead} #{pane_dead_status}")).toBe("1 3");
      await app.open(page(portOf(app.baseUrl)));
      await browser.reload();
      // The card's dot says how it ended.
      await expect(browser.locator(`[data-pane="${dead}"] [title="已退出（退出码 3）"]`)).toBeAttached();
      const pid = show(dead, "#{pane_pid}");
      await app.open(page(portOf(app.baseUrl), dead.replace("%", "")));
      await browser.reload();
      await expect(browser.locator("#ended")).toContainText("退出码 3");
      await finger.tap(browser.locator("#respawn"));
      await expect.poll(() => show(dead, "#{pane_pid}"), { message: "a new program" }).not.toBe(pid);
    } finally {
      kp("kill-pane", "-t", dead);
    }
  });
  await finger.close();
});

test("an agent's card: its model, cost and context", async ({ app, browser }) => {
  // A program that waits, under the agent's name, in a directory of its own.
  const win = process.platform === "win32";
  const bin = path.join(DIR, "agent-bin");
  const exe = path.join(bin, win ? "claude.exe" : "claude");
  fs.mkdirSync(bin, { recursive: true });
  // Linux: a script (named after it; a copy of `sleep` may be all of
  // coreutils in one program, which goes by the name it is run as).
  const linux = process.platform === "linux";
  if (linux) fs.writeFileSync(exe, "#!/bin/sh\nsleep 600\n", { mode: 0o755 });
  else if (!fs.existsSync(exe)) fs.copyFileSync(win ? "C:\\Windows\\System32\\PING.EXE" : "/bin/sleep", exe);
  const work = path.join(DIR, "agent-work", token());
  fs.mkdirSync(work, { recursive: true });
  kp("split-window", "-d", "-t", "notes", "-c", work, exe, ...(win ? ["-n", "600", "127.0.0.1"] : linux ? [] : ["600"]));
  const pane = show("notes:0.1", "#{pane_id}");
  try {
    // Its transcript, where Claude Code keeps one for that directory.
    const folder = (win ? work : fs.realpathSync(work)).replace(/[^A-Za-z0-9]/g, "-");
    const project = path.join(CLAUDE_DIR, "projects", folder);
    fs.mkdirSync(project, { recursive: true });
    const usage = { input_tokens: 1000, output_tokens: 2000, cache_read_input_tokens: 10000 };
    const reply = { type: "assistant", message: { id: "m1", model: "claude-opus-5-5", content: [], usage } };
    fs.writeFileSync(path.join(project, "s.jsonl"), JSON.stringify(reply) + "\n");
    await expect.poll(() => show(pane, "#{agent_cost} #{agent_context}"), { timeout: 20000 }).toBe("$0.05 11%");
    await app.open(page(portOf(app.baseUrl)));
    await browser.reload();
    const line = browser.locator(`[data-pane="${pane}"] [data-agent="claude"]`);
    await expect(line).toContainText("claude-opus-5-5");
    await expect(line).toContainText("$0.05");
    await expect(line).toContainText("11%");
    await expect(line).toContainText("13k");
    // Other cards have none.
    await expect(browser.locator('[data-pane][data-name="builder"] [data-agent]')).toHaveCount(0);
  } finally {
    kp("kill-pane", "-t", pane);
  }
});

test("desktop: a card's menu has the modes, the pane's ticked", async ({ app, browser, screen }) => {
  await app.open(page(portOf(app.baseUrl)));
  await browser.locator('[data-pane][data-name="builder"]').secondaryTap();
  await expect(screen.getByRole("menuitem")).toHaveCount(11);
  await expect(ticked(browser)).toHaveText("shell：在提示符下执行");
  await entry(screen, "normal：消息留着等").tap();
  try {
    await expect.poll(() => show(B(), "#{pane_work_mode}")).toBe("normal");
  } finally {
    kp("set-work-mode", "-t", B(), "shell");
  }
});
