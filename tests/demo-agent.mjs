// A stand-in agent for the recordings (tests/demo_frames.rs): it prints a
// few lines of work as an agent does, and writes the transcript an agent
// writes (Claude Code's or Codex's form, where each keeps its own), so that
// keepane reads it as it reads theirs. It is no agent: what it does is
// fixed, and it costs nothing.
//
//   node <...>/@anthropic-ai/claude-code/cli.mjs work <model> <steps>
//   node <...>/@anthropic-ai/claude-code/cli.mjs fixer <model>
//   node <...>/@openai/codex/bin/codex.mjs work <model> <steps>
//
// The kind comes from the path it is run from, as keepane tells them apart.
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const me = fileURLToPath(import.meta.url).replaceAll("\\", "/");
const kind = me.includes("@openai/codex") ? "codex" : "claude";
const [what = "work", model = kind === "codex" ? "gpt-5.5" : "claude-opus-5-5", stepsArg = "6"] = process.argv.slice(2);
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const dim = (s) => `\x1b[2m${s}\x1b[0m`;
const mark = kind === "codex" ? "\x1b[36m◇\x1b[0m" : "\x1b[33m◆\x1b[0m";
const keepane = (...args) => {
  try {
    execFileSync("keepane", args, { stdio: "ignore", shell: process.platform === "win32" });
  } catch {
    // outside keepane: nothing to tell
  }
};

// The transcript, where the agent keeps one for this directory.
const cwd = process.cwd();
let file;
let context = 14000 + Math.floor(Math.random() * 4000);
let n = 0;
const totals = { input: 0, cached: 0, output: 0 };
if (kind === "claude") {
  const home = process.env.CLAUDE_CONFIG_DIR || path.join(process.env.USERPROFILE || process.env.HOME, ".claude");
  const dir = path.join(home, "projects", cwd.replace(/[^A-Za-z0-9]/g, "-"));
  fs.mkdirSync(dir, { recursive: true });
  file = path.join(dir, `${randomUUID()}.jsonl`);
  fs.writeFileSync(file, JSON.stringify({ type: "user", cwd, message: { role: "user", content: "start" } }) + "\n");
} else {
  const home = process.env.CODEX_HOME || path.join(process.env.USERPROFILE || process.env.HOME, ".codex");
  const d = new Date();
  const pad = (x) => String(x).padStart(2, "0");
  const dir = path.join(home, "sessions", String(d.getFullYear()), pad(d.getMonth() + 1), pad(d.getDate()));
  fs.mkdirSync(dir, { recursive: true });
  file = path.join(dir, `rollout-${d.toISOString().slice(0, 19).replaceAll(":", "-")}-${randomUUID()}.jsonl`);
  const lines = [
    { type: "session_meta", payload: { cwd } },
    { type: "turn_context", payload: { model } },
    { type: "event_msg", payload: { type: "task_started" } },
  ];
  fs.writeFileSync(file, lines.map((l) => JSON.stringify(l)).join("\n") + "\n");
}

/** One reply: its tokens (the context grows), and a tool call. */
function reply(output, grows) {
  n += 1;
  context += grows;
  if (kind === "claude") {
    const usage = { input_tokens: 4, cache_read_input_tokens: context - grows, cache_creation_input_tokens: grows, output_tokens: output };
    const rec = { type: "assistant", message: { id: `msg_${n}`, model, content: [{ type: "tool_use" }], usage } };
    fs.appendFileSync(file, JSON.stringify(rec) + "\n");
  } else {
    totals.input += context;
    totals.cached += context - grows;
    totals.output += output;
    const info = {
      total_token_usage: { input_tokens: totals.input, cached_input_tokens: totals.cached, output_tokens: totals.output },
      last_token_usage: { input_tokens: context },
      model_context_window: 258400,
    };
    const recs = [
      { type: "response_item", payload: { type: "function_call" } },
      { type: "event_msg", payload: { type: "token_count", info } },
    ];
    fs.appendFileSync(file, recs.map((r) => JSON.stringify(r)).join("\n") + "\n");
  }
}

const STEPS = [
  ["Read", "src/parse.rs"],
  ["Grep", "fn parse_line"],
  ["Read", "tests/parse.rs"],
  ["Edit", "src/parse.rs  +14 -3"],
  ["Run", "cargo test parse"],
  ["Edit", "src/lib.rs  +2 -1"],
  ["Run", "cargo clippy"],
  ["Read", "README.md"],
  ["Edit", "README.md  +6"],
  ["Run", "cargo test"],
];

async function step(i) {
  const [tool, arg] = STEPS[i % STEPS.length];
  process.stdout.write(`${mark} ${tool} ${dim(arg)}\n`);
  await pause(900 + ((i * 377) % 900));
  if (tool === "Run") process.stdout.write(dim(`  ✓ ${30 + i * 3} passed\n`));
  reply(220 + ((i * 131) % 600), 3500 + ((i * 2741) % 9000));
}

console.log(`${mark} ${kind} (demo stand-in) · ${model} · ${path.basename(cwd)}`);
if (what === "work") {
  const steps = Number(stepsArg);
  for (let i = 0; i < steps; i++) await step(i);
  console.log(`${mark} done: ${steps} steps`);
  setInterval(() => {}, 1 << 30);
} else {
  // fixer: told the work through keepane (an `ai` pane), does it, says so.
  keepane("rename-pane", kind);
  keepane("set-work-mode", "ai");
  keepane("pane-ready");
  console.log(dim("waiting for a message…"));
  // Keys as they come, not echoed (an agent draws its own input), a line
  // at each Enter; a line that is only keepane's envelope is none.
  const lines = [];
  let wake = () => {};
  let typed = "";
  if (process.stdin.isTTY) process.stdin.setRawMode(true);
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    for (const ch of chunk) {
      if (ch === "\x03") process.exit(0);
      if (ch === "\r" || ch === "\n") {
        lines.push(typed);
        typed = "";
        wake();
      } else typed += ch;
    }
  });
  const next = async () => {
    while (!lines.length) await new Promise((r) => (wake = r));
    return lines.shift();
  };
  for (;;) {
    const text = (await next()).replace(/\[keepane[^\]]*\]/g, "").trim();
    if (!text) continue;
    console.log(`› ${text.length > 70 ? text.slice(0, 69) + "…" : text}`);
    for (const [tool, arg] of [
      ["Read", "src/parse.rs"],
      ["Edit", "src/parse.rs  +3 -1   (an empty line is no field)"],
      ["Run", "./test.ps1"],
    ]) {
      process.stdout.write(`${mark} ${tool} ${dim(arg)}\n`);
      await pause(1100);
      reply(380, 5200);
    }
    fs.writeFileSync(path.join(cwd, "fixed"), "");
    console.log(dim("  ✓ 12 passed"));
    console.log(`${mark} fixed: parse_line skips an empty line`);
    keepane("pane-ready");
  }
}
