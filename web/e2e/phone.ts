// A phone in the browser tests: a touch screen and fingers.
//
// The web engine has no touch emulation yet (no `hasTouch`), and the page's
// gestures (swipes between panes, the long press menu, the pinch, a drag in
// a full-screen program) need real touches. So the browser comes from here:
// `localChromium` starts Playwright's Chromium with a DevTools port, and
// `phone()` reaches the test's page over that port, turns touch on (the
// browser then reports a coarse pointer, as a phone does) and touches it.
// Everything else stays the engine's: locators, keys, expect.
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import type { Browser, BrowserProvider } from "@e2e-dev/web";
import type { Locator } from "e2e";

/** Where the provider says which browser is up: the tests read it back. */
const ENDPOINT = path.join(os.tmpdir(), "keepane-e2e", "browser.json");

/** The browsers this process started, by lease. */
const running = new Map<string, { close(): Promise<void> }>();

const freePort = () =>
  new Promise<number>((resolve, reject) => {
    const s = net.createServer().once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as net.AddressInfo;
      s.close(() => resolve(port));
    });
  });

/**
 * Playwright's Chromium (`npx @e2e-dev/web install chromium`), started by
 * Playwright, so with its own switches (no throttled frames in a page it
 * thinks hidden), and with a DevTools port.
 */
export const localChromium = (): BrowserProvider => ({
  name: "local chromium",
  async acquire(request) {
    // One browser, its port in one file: the tests take turns anyway (one server).
    if (request.slots > 1) throw new Error(`local chromium serves one worker, not ${request.slots}: set workers: 1`);
    const { chromium } = await import("playwright-core");
    const port = await freePort();
    const browser = await chromium.launch({
      // The whole of Chromium, not the cut-down headless shell (which has no
      // full screen, for one).
      channel: "chromium",
      headless: true,
      args: [`--remote-debugging-port=${port}`],
    });
    const http = `http://127.0.0.1:${port}`;
    const id = `${port}`;
    running.set(id, browser);
    fs.mkdirSync(path.dirname(ENDPOINT), { recursive: true });
    fs.writeFileSync(ENDPOINT, JSON.stringify({ http }));
    return { id, cdpEndpoint: http };
  },
  async release(lease) {
    const b = running.get(lease.id);
    running.delete(lease.id);
    await b?.close();
  },
});

/** One DevTools connection, to one page of the browser. */
class Cdp {
  private next = 1;
  private waiting = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  private constructor(
    private ws: WebSocket,
    private session = "",
  ) {
    // Gone (the browser closed or crashed): what waits fails now, not at the test's timeout.
    ws.addEventListener("close", () => {
      for (const w of this.waiting.values()) w.reject(new Error("the browser's DevTools connection closed"));
      this.waiting.clear();
    });
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(String(e.data));
      const w = m.id && this.waiting.get(m.id);
      if (!w) return;
      this.waiting.delete(m.id);
      if (m.error) w.reject(new Error(`${m.error.message} (${m.error.code})`));
      else w.resolve(m.result);
    });
  }
  static async open(url: string): Promise<Cdp> {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`no DevTools at ${url}`)), { once: true });
    });
    return new Cdp(ws);
  }
  send(method: string, params: object = {}): Promise<any> {
    if (this.ws.readyState !== WebSocket.OPEN) return Promise.reject(new Error("the browser's DevTools connection closed"));
    const id = this.next++;
    this.ws.send(JSON.stringify({ id, method, params, ...(this.session ? { sessionId: this.session } : {}) }));
    return new Promise((resolve, reject) => this.waiting.set(id, { resolve, reject }));
  }
  /** Attaches to the page showing `url`'s origin; commands go to it from then on. */
  async attach(origin: string): Promise<void> {
    const { targetInfos } = await this.send("Target.getTargets");
    const pages = (targetInfos as { type: string; url: string; targetId: string }[]).filter(
      (t) => t.type === "page" && t.url.startsWith(origin),
    );
    if (pages.length !== 1) throw new Error(`want one page on ${origin}, found ${pages.length}: ${JSON.stringify(targetInfos.map((t: any) => t.url))}`);
    const { sessionId } = await this.send("Target.attachToTarget", { targetId: pages[0].targetId, flatten: true });
    this.session = sessionId;
  }
  close() {
    this.ws.close();
  }
}

type Point = { x: number; y: number };

/** The last test's connection: closed when the next phone opens, if its test failed before closing it. */
let open: Cdp | undefined;

/** Fingers on the page: each call ends with the fingers lifted. */
export interface Fingers {
  /** A tap on the middle of what the locator finds (or a point). */
  tap(at: Locator | Point): Promise<void>;
  /** A finger held `ms` on it, then lifted. */
  longPress(at: Locator | Point, ms?: number): Promise<void>;
  /** One finger from `from` to `to` in `steps` moves. */
  swipe(from: Point, to: Point, steps?: number): Promise<void>;
  /** Two fingers on either side of `at`, `from` px apart, moving to `to` px apart. */
  pinch(at: Point, from: number, to: number): Promise<void>;
  /** The middle of what the locator finds, in the viewport. */
  middle(of: Locator): Promise<Point>;
  /** Lets the page go: the touch screen off, the connection closed. */
  close(): Promise<void>;
}

/**
 * The test's page as a phone: 390 x 844, a touch screen, a coarse pointer.
 * Call it after `app.open` (the page has to exist to be reached).
 */
export async function phone(browser: Browser, size = { width: 390, height: 844 }): Promise<Fingers> {
  await browser.setViewport(size);
  // A swipe from the edge is the page's: no history step of Chromium's own
  // (a test that wants the system's back says so, with history.back()).
  const noSwipeBack = () => {
    const add = () => {
      const s = document.createElement("style");
      s.textContent = "html, body { overscroll-behavior-x: none !important }";
      (document.head ?? document.documentElement).append(s);
    };
    if (document.documentElement) add();
    else document.addEventListener("DOMContentLoaded", add, { once: true });
    return null;
  };
  await browser.addInitScript(noSwipeBack);
  await browser.evaluate(noSwipeBack);
  const { http } = JSON.parse(fs.readFileSync(ENDPOINT, "utf8"));
  const { webSocketDebuggerUrl } = await (await fetch(`${http}/json/version`)).json();
  open?.close();
  const cdp = await Cdp.open(webSocketDebuggerUrl);
  open = cdp;
  await cdp.attach(new URL(await browser.url()).origin);
  await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  // Taken on, or the tests would touch a page that thinks it has a mouse.
  const on = await browser.evaluate(() => navigator.maxTouchPoints > 0 && matchMedia("(pointer: coarse)").matches);
  if (!on) throw new Error("touch emulation did not take: no coarse pointer");
  const touch = (type: string, points: Point[]) =>
    cdp.send("Input.dispatchTouchEvent", { type, touchPoints: points.map((p, i) => ({ x: p.x, y: p.y, id: i + 1 })) });
  const middle = async (of: Locator): Promise<Point> => {
    await of.scrollIntoView();
    const b = await of.boundingBox();
    if (!b) throw new Error("nothing to touch: no box");
    return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  };
  const at = (p: Locator | Point) => ("x" in p && "y" in p && typeof p.x === "number" ? Promise.resolve(p as Point) : middle(p as Locator));
  const hold = (ms: number) => new Promise((r) => setTimeout(r, ms));
  // A finger's pace: the page takes a second tap of one control within
  // 250 ms as the first one bouncing (format.ts, bounced).
  let lastTap = 0;
  return {
    middle,
    async tap(p) {
      const c = await at(p);
      await hold(lastTap + 300 - Date.now());
      await touch("touchStart", [c]);
      await touch("touchEnd", []);
      lastTap = Date.now();
    },
    async longPress(p, ms = 700) {
      const c = await at(p);
      await touch("touchStart", [c]);
      // A long press is a finger kept down: the time is the gesture itself.
      await hold(ms);
      await touch("touchEnd", []);
    },
    async swipe(from, to, steps = 6) {
      await touch("touchStart", [from]);
      for (let i = 1; i <= steps; i++) await touch("touchMove", [{ x: from.x + ((to.x - from.x) * i) / steps, y: from.y + ((to.y - from.y) * i) / steps }]);
      await touch("touchEnd", []);
    },
    async pinch(c, from, to) {
      const two = (d: number) => [
        { x: c.x - d / 2, y: c.y },
        { x: c.x + d / 2, y: c.y },
      ];
      await touch("touchStart", two(from));
      for (let i = 1; i <= 4; i++) await touch("touchMove", two(from + ((to - from) * i) / 4));
      await touch("touchEnd", []);
    },
    async close() {
      await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: false }).catch(() => {});
      cdp.close();
    },
  };
}
