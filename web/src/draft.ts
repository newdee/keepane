/** What is in a pane's box while another pane is shown, and over a reload of
 *  the page (this tab only): the box, what of it went out live (the pane's
 *  line has it), whether it was live then, and the message switch. */
export type Draft = { text: string; out: string; live: boolean; tell: boolean };

const KEY = "keepane-drafts";

let drafts: Record<string, Draft> = (() => {
  try {
    const d = JSON.parse(sessionStorage.getItem(KEY) || "{}");
    return d && typeof d === "object" && !Array.isArray(d) ? d : {};
  } catch {
    return {};
  }
})();

function save() {
  try {
    sessionStorage.setItem(KEY, JSON.stringify(drafts));
  } catch {
    /* a private window: kept while the page is open */
  }
}

export function draftOf(pane: string): Draft | undefined {
  const d = drafts[pane];
  return d && typeof d.text === "string" && typeof d.out === "string" ? d : undefined;
}

/** Kept, or let go when there is nothing to keep. */
export function keepDraft(pane: string, d: Draft) {
  const had = pane in drafts;
  if (!d.text && !d.out && !d.tell) {
    if (!had) return;
    delete drafts[pane];
  } else {
    const was = drafts[pane];
    if (was && was.text === d.text && was.out === d.out && was.live === d.live && was.tell === d.tell) return;
    drafts[pane] = d;
  }
  save();
}

/** The drafts of panes that are gone. */
export function dropDrafts(panes: string[]) {
  const live = new Set(panes);
  const gone = Object.keys(drafts).filter((p) => !live.has(p));
  if (!gone.length) return;
  drafts = Object.fromEntries(Object.entries(drafts).filter(([p]) => live.has(p)));
  save();
}
