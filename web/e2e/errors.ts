// What the page throws, kept across reloads for the check at the end of
// each test (fixtures.ts, pageErrors). Runs in the page, before its scripts:
// the configs pass it to the engine as an init script.
export function keepErrors() {
  const keep = (what: string) => {
    try {
      const all = JSON.parse(sessionStorage.getItem("e2e-errors") || "[]");
      sessionStorage.setItem("e2e-errors", JSON.stringify([...all, what]));
    } catch {
      // no storage here (about:blank)
    }
  };
  // A ResizeObserver whose work spilled into the next frame is the
  // browser's notice, not an error of the page's.
  addEventListener("error", (e) => e.message.startsWith("ResizeObserver loop") || keep(String(e.message)));
  addEventListener("unhandledrejection", (e) => keep(String(e.reason)));
}
