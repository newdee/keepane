// The page's language: the device's when nothing is chosen (Chinese for a
// Chinese system, else English), or the one picked in the menu, kept on
// this device.
export type Lang = "system" | "zh" | "en";

const KEY = "keepane-lang";
const systemZh = () => /^zh/i.test(navigator.language);

let chosen: Lang = (() => {
  try {
    const v = localStorage.getItem(KEY);
    return v === "zh" || v === "en" ? v : "system";
  } catch {
    return "system";
  }
})();

export let zh = chosen === "system" ? systemZh() : chosen === "zh";

export const lang = () => chosen;

/** Speak `l` from now on; the caller redraws (App keeps it in its state). */
export function setLang(l: Lang) {
  chosen = l;
  zh = l === "system" ? systemZh() : l === "zh";
  try {
    if (l === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, l);
  } catch {
    /* kept for this visit only */
  }
  document.documentElement.lang = zh ? "zh-CN" : "en";
}

document.documentElement.lang = zh ? "zh-CN" : "en";

export const t = (en: string, cn: string) => (zh ? cn : en);
