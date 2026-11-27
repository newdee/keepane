// Group files and shares on the page (docs/design/chat.md §13): a channel's
// group files listed newest first beside it, each shown by what its bytes
// are (text as text, an SVG too, a picture as a picture, the rest not at
// all) and deleted after a yes; what this machine shares, shown, given more
// time and stopped after a yes. Expected values come from the server
// (`chat-group-files -J`, `share -l -J`) and the files' own bytes, never
// from the page.
import { execFileSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect } from "e2e";
import { afterEach, beforeEach } from "@e2e-dev/web";
import { dialog, test } from "../e2e/fixtures.ts";
import { EXE, SOCKET, keyOn, kp, page, paneNumber, portOf, token } from "../e2e/keepane.ts";

type GroupFile = { file: string; name: string; size: number; seq: number; missing?: string };
type Share = { id: number; state: string; name: string; path: string; until: number | null };

let tok = "";
let chan = "";
let window = "";
let dir = "";
const shared: number[] = [];

const asPane = (pane: string, ...args: string[]) =>
  execFileSync(EXE, ["-L", SOCKET, ...args], { encoding: "utf8", env: { ...process.env, KEEPANE: SOCKET, KEEPANE_PANE: pane } });
const groupFiles = (): GroupFile[] => JSON.parse(kp("chat-group-files", chan, "-J")).group_files;
const shares = (): Share[] => JSON.parse(kp("share", "-l", "-J")).shares;

// The files sent, oldest first, and what each is: its bytes decide, not
// its name (the SVG is text; the picture is a 1×1 PNG).
const NOTES = "# notes\n\nsecond line, ünïcode\n";
const SVG = '<svg xmlns="http://www.w3.org/2000/svg" onload="document.title=\'ran\'"><circle r="4"/></svg>\n';
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==", "base64");
const BLOB = Buffer.from([0x4d, 0x5a, 0x90, 0, 3, 0, 0, 0]);

beforeEach(() => {
  tok = token();
  chan = `files-${tok}`;
  window = `files-${tok}`;
  dir = path.join(os.tmpdir(), `keepane-files-${tok}`);
  mkdirSync(dir, { recursive: true });
  kp("chat-new", chan);
  kp("new-window", "-d", "-t", "work", "-n", window);
  const pane = paneNumber(`work:${window}`);
  asPane(pane, "chat-join", chan, "--as", `bot${tok}`);
  const files: [string, string | Buffer][] = [
    [`notes-${tok}.md`, NOTES],
    [`pic-${tok}.svg`, SVG],
    [`dot-${tok}.png`, PNG],
    [`blob-${tok}.bin`, BLOB],
  ];
  for (const [name, body] of files) {
    writeFileSync(path.join(dir, name), body);
    asPane(pane, "chat-say", chan, "--file", path.join(dir, name), `here is ${name}`);
  }
});

afterEach(() => {
  for (const id of shared.splice(0)) {
    if (shares().find((s) => s.id === id)?.state === "active") kp("share", "stop", String(id));
  }
  if (chan) kp("chat-delete", chan);
  if (window && new RegExp(`^\\d+: ${window}\\b`, "m").test(kp("list-windows", "-t", "work"))) kp("kill-window", "-t", `work:${window}`);
  if (dir) rmSync(dir, { recursive: true, force: true });
});

test("a channel's group files are listed newest first, shown by what they are, and deleted after a yes", async ({ app, screen, browser }) => {
  // The server's list: the order and names the page must show.
  const listed = groupFiles();
  expect(listed.map((f) => f.name)).toEqual([`blob-${tok}.bin`, `dot-${tok}.png`, `pic-${tok}.svg`, `notes-${tok}.md`]);

  await app.open(page(portOf(app.baseUrl)));
  await screen.getByRole("button", "群聊").click();
  await screen.getByRole("complementary", "频道").getByRole("button", `#${chan}`).click();
  const talk = screen.getByRole("region", `#${chan}`);
  await talk.getByRole("button", "群文件").click();
  const panel = screen.getByRole("complementary", "群文件");
  for (const f of listed) await expect(panel.getByRole("button", `查看 ${f.name}`)).toBeVisible();
  const order = await browser.evaluate(() =>
    [...document.querySelectorAll('aside[aria-label="群文件"] button[aria-label^="查看 "]')].map((b) => b.getAttribute("aria-label")!.slice(3)),
  );
  expect(order).toEqual(listed.map((f) => f.name));

  // Text: its bytes, as text.
  await panel.getByRole("button", `查看 notes-${tok}.md`).click();
  let shown = dialog(screen, `notes-${tok}.md 的预览`);
  await expect(shown.getByLabel("文件内容")).toHaveText(NOTES.trimEnd());
  await shown.getByRole("button", "关闭").click();
  await expect(shown).toBeHidden();

  // An SVG: read as text, never drawn (no picture, its script never ran).
  await panel.getByRole("button", `查看 pic-${tok}.svg`).click();
  shown = dialog(screen, `pic-${tok}.svg 的预览`);
  await expect(shown.getByLabel("文件内容")).toContainText("<svg");
  await expect(shown.getByRole("img")).toHaveCount(0);
  expect(await browser.evaluate(() => document.title)).not.toBe("ran");
  await shown.getByRole("button", "关闭").click();

  // A picture: drawn, from its bytes (1×1).
  await panel.getByRole("button", `查看 dot-${tok}.png`).click();
  shown = dialog(screen, `dot-${tok}.png 的预览`);
  const img = shown.getByRole("img", `dot-${tok}.png`);
  await expect(img).toBeVisible();
  const alt = `dot-${tok}.png`;
  await expect
    .poll(() => browser.evaluate((a) => (document.querySelector(`[role="dialog"] img[alt="${a}"]`) as HTMLImageElement | null)?.naturalWidth ?? 0, alt), { message: "drawn" })
    .toBe(1);
  await shown.getByRole("button", "关闭").click();

  // A program: nothing shown, said so.
  await panel.getByRole("button", `查看 blob-${tok}.bin`).click();
  shown = dialog(screen, `blob-${tok}.bin 的预览`);
  await expect(shown.getByText("这个文件不能预览。")).toBeVisible();
  await shown.getByRole("button", "关闭").click();

  // What the server sends to be shown: the type its bytes say, nothing
  // that runs (an SVG is text; a policy that sandboxes it).
  const base = app.baseUrl!.replace(/\/$/, "");
  const key = keyOn(portOf(app.baseUrl));
  const id = (JSON.parse(kp("chat-list", "-J")) as { name: string; id: string }[]).find((c) => c.name === chan)!.id;
  const svg = listed.find((f) => f.name.endsWith(".svg"))!;
  const r = await fetch(`${base}/api/chat/file?c=${id}&f=${encodeURIComponent(svg.file)}&view=1`, { headers: { "X-Keepane-Key": key } });
  expect(r.headers.get("content-type")).toBe("text/plain; charset=utf-8");
  expect(r.headers.get("x-content-type-options")).toBe("nosniff");
  expect(r.headers.get("content-security-policy")).toContain("sandbox");

  // Deleted after a yes: gone from the server's list and from the panel.
  const notes = listed.find((f) => f.name.startsWith("notes-"))!;
  await panel.getByRole("button", `删除 ${notes.name}`).click();
  const ask = screen.getByRole("alertdialog", `删除 ${notes.name}？`);
  await ask.getByRole("button", "删除").click();
  await expect.poll(() => groupFiles().map((f) => f.file), { message: "deleted on the server" }).not.toContain(notes.file);
  expect(groupFiles()).toHaveLength(listed.length - 1);
  await expect(panel.getByRole("button", `查看 ${notes.name}`)).toHaveCount(0);

  // Deleted in the terminal while the panel is open: gone from it too.
  const blob = listed.find((f) => f.name.startsWith("blob-"))!;
  kp("chat-group-file-rm", chan, blob.file);
  await expect(panel.getByRole("button", `查看 ${blob.name}`)).toHaveCount(0, { timeout: 15_000 });

  await screen.getByRole("button", "回到 pane 列表").first().click();
});

test("what this machine shares is listed, shown, given more time and stopped after a yes", async ({ app, screen, browser }) => {
  const file = path.join(dir, `shared-${tok}.txt`);
  const body = `shared ${tok}\nline two\n`;
  writeFileSync(file, body);
  kp("share", file, "--for", "1h");
  const share = shares().find((s) => s.path === file)!;
  shared.push(share.id);
  expect(share.state).toBe("active");
  expect(share.until).not.toBeNull();
  // A folder shared: listed, never shown (the server says so).
  kp("share", dir, "--for", "1h");
  const folder = shares().find((s) => s.path === dir && s.state === "active")!;
  shared.push(folder.id);
  const base = app.baseUrl!.replace(/\/$/, "");
  const asked = await fetch(`${base}/api/share/file?id=${folder.id}`, { headers: { "X-Keepane-Key": keyOn(portOf(app.baseUrl)) } });
  expect(asked.status).toBe(415);

  await app.open(page(portOf(app.baseUrl)));
  await screen.getByRole("button", "分享").click();
  const list = screen.getByRole("region", "分享");
  const row = list.getByRole("listitem", `#${share.id} ${share.name}`);
  await expect(row.getByText("分享中")).toBeVisible();

  // Shown: its bytes, as text.
  await row.getByRole("button", `查看 ${share.name}`).click();
  const shown = dialog(screen, `${share.name} 的预览`);
  await expect(shown.getByLabel("文件内容")).toHaveText(body.trimEnd());
  await shown.getByRole("button", "关闭").click();
  await expect(shown).toBeHidden();

  // More time: no end, on the server.
  await row.getByRole("button", `延长 #${share.id}`).click();
  await screen.getByRole("menuitem", "不限时").click();
  await expect.poll(() => shares().find((s) => s.id === share.id)!.until, { message: "no end on the server" }).toBeNull();

  // Stopped after a yes: ended on the server, off the list of live ones,
  // back when the ended ones are shown.
  await row.getByRole("button", `停止 #${share.id}`).click();
  await screen.getByRole("alertdialog", `停止分享 ${share.name}？`).getByRole("button", "停止").click();
  await expect.poll(() => shares().find((s) => s.id === share.id)!.state, { message: "stopped on the server" }).toBe("stopped");
  await expect(row).toBeHidden();
  await list.getByRole("button", /^已结束/).click();
  await expect(list.getByRole("listitem", `#${share.id} ${share.name}`).getByText("已停止")).toBeVisible();
  await screen.getByRole("button", "回到 pane 列表").first().click();

  // A phone: no room in the header; the menu opens it.
  await browser.setViewport({ width: 390, height: 844 });
  try {
    await expect(screen.getByRole("button", "分享")).toHaveCount(0);
    await screen.getByRole("button", "外观").click();
    await screen.getByRole("menuitem", "分享").click();
    await expect(screen.getByRole("region", "分享")).toBeVisible();
    await screen.getByRole("button", "回到 pane 列表").first().click();
  } finally {
    await browser.setViewport({ width: 1280, height: 860 });
  }
});

test("a read-only page shows group files and shares but deletes, stops and extends nothing", async ({ app, screen }) => {
  const file = path.join(dir, `ro-${tok}.txt`);
  writeFileSync(file, `ro ${tok}\n`);
  kp("share", file, "--for", "1h");
  const share = shares().find((s) => s.path === file)!;
  shared.push(share.id);
  const port = portOf(app.baseUrl);
  kp("web", "stop");
  try {
    kp("web", "-r", "-b", "127.0.0.1", "-p", port);
    await app.open(page(port));
    await screen.getByRole("button", "群聊").click();
    await screen.getByRole("complementary", "频道").getByRole("button", `#${chan}`).click();
    await screen.getByRole("region", `#${chan}`).getByRole("button", "群文件").click();
    const panel = screen.getByRole("complementary", "群文件");
    const notes = groupFiles().find((f) => f.name.startsWith("notes-"))!;
    await expect(panel.getByRole("button", `查看 ${notes.name}`)).toBeVisible();
    await expect(panel.getByRole("button", `删除 ${notes.name}`)).toHaveCount(0);
    await screen.getByRole("button", "分享").click();
    const row = screen.getByRole("region", "分享").getByRole("listitem", `#${share.id} ${share.name}`);
    await expect(row.getByRole("button", `查看 ${share.name}`)).toBeVisible();
    await expect(row.getByRole("button", `停止 #${share.id}`)).toHaveCount(0);
    await expect(row.getByRole("button", `延长 #${share.id}`)).toHaveCount(0);
    // Nothing changed on the server.
    expect(groupFiles().map((f) => f.file)).toContain(notes.file);
    expect(shares().find((s) => s.id === share.id)!.state).toBe("active");
    await screen.getByRole("button", "回到 pane 列表").first().click();
  } finally {
    kp("web", "stop");
    kp("web", "-b", "127.0.0.1", "-p", port);
  }
});