// The group chat on the page: the channels as a chat app lists them (the
// one last said in first, what is unread on it), one opened shows what was
// said and who is there, and what the person types goes into the channel,
// a `@` listing the members to name. Expected values come from the server
// (`chat` commands), never from the page.
import { execFileSync } from "node:child_process";
import { rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect } from "e2e";
import { afterEach, beforeEach } from "@e2e-dev/web";
import { test } from "../e2e/fixtures.ts";
import { EXE, SOCKET, keyOn, kp, page, paneNumber, portOf, token } from "../e2e/keepane.ts";

type Line = { kind: string; text: string; from?: { handle: string }; files?: { name: string; size: number; file: string }[] };
type Channel = { name: string; unread: number };

let tok = "";
let older = "";
let newer = "";
let bot = "";
let window = "";
const said = ["the build is green", "notes are in"];

/** A keepane command run as pane `pane`: what a pane's own environment
 *  says (its server's name and its number). */
const asPane = (pane: string, ...args: string[]) =>
  execFileSync(EXE, ["-L", SOCKET, ...args], { encoding: "utf8", env: { ...process.env, KEEPANE: SOCKET, KEEPANE_PANE: pane } });
const channels = (): Channel[] => JSON.parse(kp("chat-list", "-J"));
const lines = (c: string): Line[] => JSON.parse(kp("chat-read", c, "--all", "-n", "50", "-J")).lines;

beforeEach(() => {
  tok = token();
  // By name the other way round from when each was said in: the order on
  // the page can only come from the last line.
  older = `a-older-${tok}`;
  newer = `z-newer-${tok}`;
  bot = `bot${tok}`;
  window = `chat-${tok}`;
  kp("chat-new", older, "--topic", "said in first");
  kp("chat-new", newer, "--topic", "said in last");
  kp("new-window", "-d", "-t", "work", "-n", window);
  const pane = paneNumber(`work:${window}`);
  asPane(pane, "chat-join", older, "--as", bot, "--role", "builder");
  asPane(pane, "chat-join", newer, "--as", bot, "--role", "builder");
  asPane(pane, "chat-say", older, "an older line");
  for (const s of said) asPane(pane, "chat-say", newer, s);
});

afterEach(() => {
  for (const c of [older, newer]) if (c) kp("chat-delete", c);
  if (window && new RegExp(`^\\d+: ${window}\\b`, "m").test(kp("list-windows", "-t", "work"))) kp("kill-window", "-t", `work:${window}`);
});

test("the channels are listed by their last line and one opened says what was said there", async ({ app, screen, browser }) => {
  // What is unread, as the server counts it before the page shows anything.
  expect(channels().find((c) => c.name === newer)!.unread).toBe(said.length);
  expect(channels().find((c) => c.name === older)!.unread).toBe(1);
  await app.open(page(portOf(app.baseUrl)));
  await screen.getByRole("button", "群聊").click();
  const list = screen.getByRole("complementary", "频道");
  await expect(list.getByRole("button", `#${newer}`)).toBeVisible();
  // The order: the one said in last above the other.
  const order = await browser.evaluate(() =>
    [...document.querySelectorAll('aside[aria-label="频道"] button[aria-label^="#"]')].map((b) => b.getAttribute("aria-label")),
  );
  expect(order.indexOf(`#${newer}`)).toBeLessThan(order.indexOf(`#${older}`));
  // On a wide screen the one said in last opens by itself (and is read by
  // being shown); the other keeps its unread count on it.
  await expect(list.getByRole("button", `#${older}`).getByLabel("1 条未读")).toBeVisible();

  // Opened: what was said, who is there; and it counts as read.
  await list.getByRole("button", `#${newer}`).click();
  const talk = screen.getByRole("region", `#${newer}`);
  for (const s of said) await expect(talk.getByText(s)).toBeVisible();
  await expect(talk.getByRole("button", "成员")).toHaveText("1");
  await expect.poll(() => channels().find((c) => c.name === newer)!.unread, { message: "read by being shown" }).toBe(0);

  // Typed: a `@` lists the members, Tab puts one in, Enter says it.
  const box = talk.getByRole("textbox", `在 #${newer} 里说点什么`);
  await box.click();
  await box.pressSequentially("thanks @bo");
  const named = screen.getByRole("listbox", "可以点名的成员");
  await expect(named.getByRole("option")).toHaveCount(1);
  await expect(named).toContainText(`@${bot}`);
  await box.press("Tab");
  await expect(box).toHaveValue(`thanks @${bot} `);
  await box.pressSequentially("all good");
  await box.press("Enter");
  await expect
    .poll(() => lines(newer).find((l) => l.kind === "say" && l.text === `thanks @${bot} all good`)?.from?.handle ?? "", {
      message: "said in the channel, as the person",
    })
    .toMatch(/^you@/);
  await expect(box).toHaveValue("");
  await expect(talk.getByText(`thanks @${bot} all good`)).toBeVisible();
  // Said from elsewhere while it is open: it comes (the page reads the
  // lines again when the channel's last line is another).
  const later = `from the terminal ${tok}`;
  asPane(paneNumber(`work:${window}`), "chat-say", newer, later);
  await expect(talk.getByText(later)).toBeVisible();

  // Back to the panes: the page as the other tests expect it.
  await screen.getByRole("button", "回到 pane 列表").first().click();
  await expect(screen.getByRole("button", "群聊")).toBeVisible();
});

test("a file sent in a channel shows by name and size and saves as it was sent", async ({ app, screen, browser }) => {
  // Sent by the bot, from a file of known bytes; what the line says of it
  // is the server's.
  const source = path.join(os.tmpdir(), `report-${tok}.txt`);
  const body = `report ${tok}\nsecond line, ünïcode\n`;
  writeFileSync(source, body);
  try {
    asPane(paneNumber(`work:${window}`), "chat-say", older, "--file", source, "the report");
  } finally {
    rmSync(source, { force: true });
  }
  const sent = lines(older).at(-1)!.files![0];
  expect(sent.name).toBe(`report-${tok}.txt`);
  expect(sent.size).toBe(Buffer.byteLength(body));

  await app.open(page(portOf(app.baseUrl)));
  await screen.getByRole("button", "群聊").click();
  await screen.getByRole("complementary", "频道").getByRole("button", `#${older}`).click();
  const talk = screen.getByRole("region", `#${older}`);
  await expect(talk.getByText("the report")).toBeVisible();
  const save = talk.getByRole("button", `保存 ${sent.name}`);
  await expect(save).toBeVisible();
  await expect(save).toContainText(`${sent.size} B`);

  // Saved: what the page hands the browser is the file's name and bytes.
  await browser.evaluate(() => {
    const w = window as unknown as { saved?: { name: string; text: string } };
    HTMLAnchorElement.prototype.click = function (this: HTMLAnchorElement) {
      const name = this.download;
      void fetch(this.href).then((r) => r.text()).then((text) => (w.saved = { name, text }));
    };
    return null;
  });
  await save.click();
  await expect
    .poll(() => browser.evaluate(() => (window as unknown as { saved?: { name: string; text: string } }).saved ?? null), {
      message: "the page saved it",
    })
    .toEqual({ name: sent.name, text: body });

  // Asked for outside the channel's folder: refused, by the server.
  const base = app.baseUrl!.replace(/\/$/, "");
  const key = keyOn(portOf(app.baseUrl));
  const id = (JSON.parse(kp("chat-list", "-J")) as { name: string; id: string }[]).find((c) => c.name === older)!.id;
  const ask = (f: string) => fetch(`${base}/api/chat/file?c=${id}&f=${encodeURIComponent(f)}`, { headers: { "X-Keepane-Key": key } });
  expect((await ask(sent.file)).status).toBe(200);
  expect((await ask("../../channels.json")).status).toBe(400);
  expect((await ask(sent.file.replace(sent.name, "..\\..\\x"))).status).toBe(400);
});