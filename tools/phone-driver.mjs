// `keepane web` on a phone, driven one step at a time: the tour recording
// (tests/demo_frames.rs, record_tour) sends a command per line on stdin and
// waits for "ok" (or "error ...") on stdout, taking a picture whenever the
// phone's screen should appear in the recording. An iPhone-sized, touch,
// twice-the-pixels page in the machine's Edge, as tools/phone-shot.mjs.
//
//   node phone-driver.mjs <edge.exe>
//
// Commands (JSON, one per line):
//   {"do":"open","url":"http://..."}   the page, until its pane list shows
//   {"do":"tap","name":"build"}         the pane named so (%build)
//   {"do":"type","text":"git log"}      into the box, as typed
//   {"do":"send"}                       the Send button
//   {"do":"wait","text":"..."}          until the pane's screen shows it
//   {"do":"back"}                       to the list
//   {"do":"shot","path":"p.png"}        a picture of the screen
//   {"do":"quit"}
import puppeteer from "puppeteer-core";
import readline from "node:readline";

const [edge] = process.argv.slice(2);
const browser = await puppeteer.launch({ executablePath: edge, headless: true, args: ["--disable-gpu", "--lang=en"] });
const page = await browser.newPage();
await page.emulate({
  viewport: { width: 390, height: 844, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
  userAgent:
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
});
const settle = (ms) => new Promise((r) => setTimeout(r, ms));

async function run(c) {
  switch (c.do) {
    case "open":
      await page.goto(c.url, { waitUntil: "load" });
      await page.waitForFunction(() => document.querySelectorAll(".pane").length > 0, { timeout: 15000 });
      await settle(300);
      return;
    case "tap": {
      const hit = await page.evaluate((name) => {
        const b = [...document.querySelectorAll("button.pane")].find((b) =>
          b.querySelector(".where").textContent.startsWith(`%${name} `),
        );
        if (!b) return false;
        b.click();
        return true;
      }, c.name);
      if (!hit) throw new Error(`no pane %${c.name}`);
      await page.waitForFunction(() => document.getElementById("screen").textContent.trim().length > 0, { timeout: 15000 });
      await settle(400);
      return;
    }
    case "type":
      await page.focus("#text");
      await page.keyboard.type(c.text, { delay: 35 });
      return;
    case "send":
      await page.click("#send");
      return;
    case "wait":
      await page.waitForFunction((t) => document.getElementById("screen").textContent.includes(t), { timeout: 15000 }, c.text);
      await settle(200);
      return;
    case "back":
      await page.click("#back");
      await page.waitForFunction(() => document.querySelectorAll(".pane").length > 0, { timeout: 15000 });
      return;
    case "shot":
      await page.screenshot({ path: c.path });
      return;
    default:
      throw new Error(`unknown command ${c.do}`);
  }
}

const lines = readline.createInterface({ input: process.stdin });
for await (const line of lines) {
  if (!line.trim()) continue;
  const c = JSON.parse(line);
  if (c.do === "quit") break;
  try {
    await run(c);
    process.stdout.write("ok\n");
  } catch (e) {
    process.stdout.write(`error ${String(e.message || e).replace(/\n/g, " ")}\n`);
  }
}
lines.close();
await browser.close();
// Nothing else may keep node alive (stdin stays open until the recorder
// lets go of it).
process.exit(0);
