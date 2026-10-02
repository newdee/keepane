// One picture of `keepane web` as a phone shows it: an iPhone-sized, touch,
// twice-the-pixels page in the machine's Edge, taken once the page has
// something on it. Used by tools/make-phone-shots.ps1.
//
//   node phone-shot.mjs <edge.exe> <url> <out.png> <list|pane|pane-detail> [en|zh-CN]
//
// pane-detail: the pane with its command times on (the clock button).
import puppeteer from "puppeteer-core";

const [edge, url, out, view, lang = "en"] = process.argv.slice(2);
const browser = await puppeteer.launch({ executablePath: edge, headless: true, args: ["--disable-gpu", `--lang=${lang}`] });
try {
  const page = await browser.newPage();
  // The page picks its words by the phone's language.
  await page.evaluateOnNewDocument((l) => {
    Object.defineProperty(navigator, "language", { get: () => l });
  }, lang);
  await page.evaluateOnNewDocument((on) => localStorage.setItem("keepane-detail", on ? "1" : "0"), view === "pane-detail");
  // The page dark, as the background the two pictures are put on.
  await page.evaluateOnNewDocument(() => localStorage.setItem("keepane-mode", "dark"));
  await page.emulate({
    viewport: { width: 390, height: 844, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    userAgent:
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
  });
  await page.goto(url, { waitUntil: "load" });
  // Ready: the list has its panes, or the pane its text.
  await page.waitForFunction(
    (v) =>
      v === "list"
        ? document.querySelectorAll("[data-pane]").length > 0
        : v === "pane-detail"
          ? document.querySelectorAll(".stamp").length >= 2
          : document.getElementById("screen").textContent.trim().length > 0,
    { timeout: 15000 },
    view,
  );
  await new Promise((r) => setTimeout(r, 700));
  const state = await page.evaluate(() => {
    const m = document.getElementById("main");
    // The list has no screen.
    if (!m) return { panes: document.querySelectorAll("[data-pane]").length };
    return { scrollTop: m.scrollTop, scrollHeight: m.scrollHeight, clientHeight: m.clientHeight, lines: document.getElementById("screen").textContent.split("\n").length };
  });
  console.log(JSON.stringify(state));
  await page.screenshot({ path: out });
} finally {
  await browser.close();
}
