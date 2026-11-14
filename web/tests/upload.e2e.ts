// Files from the page into the pane's folder (`web-upload-dir`): the 📎
// button says where they go; a chosen file is sent and lands there byte for
// byte, its path shown with a button to copy it; the same name again gets
// " (1)"; the server says each on its status line. Unset, the folder is
// `keepane-uploads` under the pane's directory. (The API's refusals: tests/e2e.rs.)
import { expect } from "e2e";
import * as fs from "node:fs";
import * as path from "node:path";
import { pageText, test } from "../e2e/fixtures.ts";
import { DIR, kp, page, paneNumber, portOf, show, token, withOption } from "../e2e/keepane.ts";
import { phone } from "../e2e/phone.ts";

const FILE = "tests/fixtures/notes.txt";

test("a file from the page lands in the pane's folder", async ({ app, browser, screen }) => {
  const n = paneNumber("work:0.1");
  const into = path.join(DIR, `uploads-${token()}`);
  await withOption("web-upload-dir", into, async () => {
    await app.open(page(portOf(app.baseUrl), n));
    await expect(browser.locator("#upload")).toHaveAccessibleName(`上传文件到 ${into}`);
    const picker = browser.locator('[data-testid="upload-input"]');
    await picker.setInputFiles([FILE]);
    await expect.poll(() => pageText(browser)).toContain(`已存到 ${path.join(into, "notes.txt")}`);
    expect(fs.readFileSync(path.join(into, "notes.txt"))).toEqual(fs.readFileSync(FILE));
    // The same name again: beside it, the first left as it was.
    await picker.setInputFiles([FILE]);
    await expect.poll(() => pageText(browser)).toContain(path.join(into, "notes (1).txt"));
    expect(fs.readFileSync(path.join(into, "notes (1).txt"))).toEqual(fs.readFileSync(FILE));
    await expect(screen.getByRole("button", "复制 notes (1).txt 的路径")).toBeVisible();
    // Said on this machine, not typed into the pane.
    expect(kp("show-messages")).toContain(`%${n}: received notes.txt`);
    expect(show(`%${n}`, "#{pane_current_command}")).not.toBe("");
  });
  // Unset: `keepane-uploads` under the pane's directory.
  const cwd = show(`%${n}`, "#{pane_current_path}");
  await browser.reload();
  await expect(browser.locator("#upload")).toHaveAccessibleName(`上传文件到 ${path.join(cwd, "keepane-uploads")}`);
  fs.rmSync(into, { recursive: true, force: true });
  // On a phone held upright the box is narrower beside the paper clip: its
  // hint stays on one line (cut at the edge), no second line half shown.
  const finger = await phone(browser);
  await expect(browser.locator("#text")).toBeVisible();
  const lines = () =>
    browser.evaluate(() => {
      const t = document.querySelector<HTMLTextAreaElement>("#text")!;
      return t.value === "" && t.scrollHeight <= t.clientHeight + 1;
    });
  await expect.poll(lines).toBe(true);
  await finger.close();
});
