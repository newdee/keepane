import type { Mark } from "./api";

/** The text without its colours and the terminal's other escapes. */
export const plain = (s: string) =>
  s.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");

/** Into the clipboard. Served over plain http on the network, the page has
 *  no `navigator.clipboard`: then the old way, `execCommand("copy")`. Its
 *  copy event is handed the text itself, since an open menu keeps the focus
 *  (and so the selection) to itself; a hidden text box (read-only, so that no
 *  keyboard comes up) holds it selected too, for browsers that want one. */
export async function copyText(s: string): Promise<boolean> {
  try {
    if (window.isSecureContext && navigator.clipboard) {
      await navigator.clipboard.writeText(s);
      return true;
    }
  } catch {
    /* the old way, then */
  }
  const box = document.createElement("textarea");
  box.value = s;
  box.setAttribute("readonly", "");
  box.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;font-size:16px";
  document.body.appendChild(box);
  box.select();
  box.setSelectionRange(0, s.length);
  const give = (e: ClipboardEvent) => {
    e.clipboardData?.setData("text/plain", s);
    e.preventDefault();
  };
  document.addEventListener("copy", give, true);
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  document.removeEventListener("copy", give, true);
  box.remove();
  return ok;
}

/** What a command printed: the screen's lines between what was typed and
 *  the next prompt, as keepane placed them (the mark's `last` and `stop`). */
export function outputOf(lines: string[], m: Mark): string {
  const out = lines.slice(m[4] + 1, m[5]).map((l) => plain(l).replace(/\s+$/, ""));
  while (out.length && !out[out.length - 1]) out.pop();
  return out.join("\n");
}
