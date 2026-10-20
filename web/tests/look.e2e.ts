// The pane's colours on the page: text the theme coloured never vanishes on
// a background a program painted itself, as on the computer.
import { expect } from "e2e";
import { test } from "../e2e/fixtures.ts";
import { kp, page, portOf, show, token, withOption } from "../e2e/keepane.ts";

test("a program's dark box keeps its text on light paper", async ({ app, browser }) => {
  const t = token();
  // Default text on black, the theme's white (dark ink on paper) on a dark
  // grey, a colour of the program's own, inverse, default text on near-white,
  // the theme's grey on black (2.8:1, under the 3:1 the rule asks); a line
  // each, as the split pane is narrow.
  const said = [
    `\`e[48;2;0;0;0mA${t}`,
    `\`e[97;48;5;235mB${t}`,
    `\`e[38;2;40;40;40;48;2;0;0;0mC${t}`,
    `\`e[7;48;2;0;0;0mD${t}`,
    `\`e[48;2;250;250;250mE${t}`,
    `\`e[37;48;2;0;0;0mF${t}`,
  ]
    .map((l) => `${l}\`e[0m`)
    .join("\`r\`n");
  await withOption("theme", "paper", async () => {
    kp("split-window", "-h", "-d", "-t", "notes", "pwsh", "-NoProfile", "-Command", `[Console]::Write("${said}"); Start-Sleep 600`);
    const pane = show("notes:0.1", "#{pane_id}");
    try {
      await browser.setViewport({ width: 1440, height: 900 });
      await app.open(page(portOf(app.baseUrl), pane.replace("%", "")));
      const look = (k: string) =>
        browser.evaluate((w) => {
          const s = [...document.querySelectorAll("pre.term span")].find((e) => e.textContent === w) as HTMLElement | undefined;
          return s ? [getComputedStyle(s).color, s.style.color] : null;
        }, `${k}${t}`);
      const paper = "rgb(240, 231, 210)";
      await expect.poll(() => look("A"), { message: "default text on black: the paper" }).toEqual([paper, paper]);
      await expect.poll(() => look("B"), { message: "the theme's white on dark grey: the paper" }).toEqual([paper, paper]);
      expect(await look("C"), "the program's own colour stays").toEqual(["rgb(40, 40, 40)", "rgb(40, 40, 40)"]);
      expect(await look("D"), "inverse stays").toEqual(["rgb(0, 0, 0)", "rgb(0, 0, 0)"]);
      const ground = await browser.evaluate((w) => {
        const s = [...document.querySelectorAll("pre.term span")].find((e) => e.textContent === w);
        return s ? getComputedStyle(s).backgroundColor : null;
      }, `D${t}`);
      expect(ground, "inverse: on the theme's text colour").toBe("rgb(51, 48, 42)");
      expect(await look("E"), "text that reads stays the theme's, unset").toEqual(["rgb(51, 48, 42)", ""]);
      expect(await look("F"), "the theme's grey on black: the paper").toEqual([paper, paper]);
    } finally {
      kp("kill-pane", "-t", pane);
    }
  });
});
