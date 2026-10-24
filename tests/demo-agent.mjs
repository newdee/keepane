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
const now = () => new Date().toISOString();
// What it was asked first: the first turn's prompt.
const asked = what === "work" ? "make parse_line skip blank lines, then run the tests" : "start";
if (kind === "claude") {
  const home = process.env.CLAUDE_CONFIG_DIR || path.join(process.env.USERPROFILE || process.env.HOME, ".claude");
  const dir = path.join(home, "projects", cwd.replace(/[^A-Za-z0-9]/g, "-"));
  fs.mkdirSync(dir, { recursive: true });
  file = path.join(dir, `${randomUUID()}.jsonl`);
  fs.writeFileSync(file, JSON.stringify({ type: "user", cwd, timestamp: now(), message: { role: "user", content: asked } }) + "\n");
} else {
  const home = process.env.CODEX_HOME || path.join(process.env.USERPROFILE || process.env.HOME, ".codex");
  const d = new Date();
  const pad = (x) => String(x).padStart(2, "0");
  const dir = path.join(home, "sessions", String(d.getFullYear()), pad(d.getMonth() + 1), pad(d.getDate()));
  fs.mkdirSync(dir, { recursive: true });
  file = path.join(dir, `rollout-${d.toISOString().slice(0, 19).replaceAll(":", "-")}-${randomUUID()}.jsonl`);
  const lines = [
    { type: "session_meta", timestamp: now(), payload: { cwd } },
    { type: "turn_context", timestamp: now(), payload: { model } },
    { type: "event_msg", timestamp: now(), payload: { type: "task_started" } },
    { type: "event_msg", timestamp: now(), payload: { type: "user_message", message: asked } },
  ];
  fs.writeFileSync(file, lines.map((l) => JSON.stringify(l)).join("\n") + "\n");
}

/** A tool as each agent names it, and what it is given. */
function toolCall(tool, arg) {
  const file = arg.split(" ")[0];
  if (kind === "claude") {
    switch (tool) {
      case "Read": return ["Read", { file_path: file }];
      case "Grep": return ["Grep", { pattern: arg }];
      case "Edit": return ["Edit", { file_path: file }];
      default: return ["Bash", { command: arg }];
    }
  }
  switch (tool) {
    case "Read": return ["read_file", JSON.stringify({ path: file })];
    case "Grep": return ["grep_files", JSON.stringify({ pattern: arg })];
    case "Edit": return ["apply_patch", `*** Begin Patch\n*** Update File: ${file}\n@@`];
    default: return ["shell", JSON.stringify({ command: ["bash", "-lc", arg] })];
  }
}

/** One reply: its tokens (the context grows), and a tool call (its id
 *  returned, for its answer). */
function reply(output, grows, tool = "Run", arg = "") {
  n += 1;
  context += grows;
  const id = `call_${n}`;
  const [name, input] = toolCall(tool, arg);
  if (kind === "claude") {
    const usage = { input_tokens: 4, cache_read_input_tokens: context - grows, cache_creation_input_tokens: grows, output_tokens: output };
    const block = { type: "tool_use", id, name, input };
    const rec = { type: "assistant", timestamp: now(), message: { id: `msg_${n}`, model, content: [block], usage } };
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
    const call = name === "apply_patch" ? { type: "custom_tool_call", name, call_id: id, input } : { type: "function_call", name, call_id: id, arguments: input };
    const recs = [
      { type: "response_item", timestamp: now(), payload: call },
      { type: "event_msg", timestamp: now(), payload: { type: "token_count", info } },
    ];
    fs.appendFileSync(file, recs.map((r) => JSON.stringify(r)).join("\n") + "\n");
  }
  return id;
}

/** A tool's answer, as each agent writes it: failed or not. */
function answer(id, failed) {
  const rec =
    kind === "claude"
      ? { type: "user", timestamp: now(), message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, is_error: failed, content: failed ? "1 failed" : "ok" }] } }
      : { type: "response_item", timestamp: now(), payload: { type: "function_call_output", call_id: id, output: JSON.stringify({ output: failed ? "1 failed" : "ok", metadata: { exit_code: failed ? 1 : 0 } }) } };
  fs.appendFileSync(file, JSON.stringify(rec) + "\n");
}

/** A turn begun (a prompt given) and ended, as each agent writes them:
 *  keepane takes an agent pane as busy from the one to the other. */
function turnStart(text) {
  const rec =
    kind === "claude"
      ? { type: "user", cwd, timestamp: now(), message: { role: "user", content: text } }
      : { type: "event_msg", timestamp: now(), payload: { type: "task_started" } };
  fs.appendFileSync(file, JSON.stringify(rec) + "\n");
}
function turnEnd() {
  n += 1;
  const rec =
    kind === "claude"
      ? { type: "assistant", cwd, timestamp: now(), message: { id: `msg_${n}`, model, stop_reason: "end_turn", content: [] } }
      : { type: "event_msg", timestamp: now(), payload: { type: "task_complete" } };
  fs.appendFileSync(file, JSON.stringify(rec) + "\n");
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
  // The model at work, then the tool it calls, until its answer; the first
  // test run fails (the edit after it is the fix).
  await pause(400 + ((i * 211) % 700));
  const id = reply(220 + ((i * 131) % 600), 3500 + ((i * 2741) % 9000), tool, arg);
  process.stdout.write(`${mark} ${tool} ${dim(arg)}\n`);
  await pause(600 + ((i * 377) % 900));
  const failed = i === 4;
  if (tool === "Run") process.stdout.write(dim(failed ? "  ✗ 1 failed\n" : `  ✓ ${30 + i * 3} passed\n`));
  answer(id, failed);
}

console.log(`${mark} ${kind} (demo stand-in) · ${model} · ${path.basename(cwd)}`);
if (what === "work") {
  const steps = Number(stepsArg);
  for (let i = 0; i < steps; i++) await step(i);
  console.log(`${mark} done: ${steps} steps`);
  turnEnd();
  setInterval(() => {}, 1 << 30);
} else {
  // fixer: told the work through keepane (an `ai` pane), does it, says so.
  turnEnd(); // the opening turn: nothing to do yet
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
    turnStart(text);
    console.log(`› ${text.length > 70 ? text.slice(0, 69) + "…" : text}`);
    for (const [tool, arg] of [
      ["Read", "src/parse.rs"],
      ["Edit", "src/parse.rs  +3 -1   (an empty line is no field)"],
      ["Run", "./test.ps1"],
    ]) {
      await pause(500);
      const id = reply(380, 5200, tool, arg);
      process.stdout.write(`${mark} ${tool} ${dim(arg)}\n`);
      await pause(800);
      answer(id, false);
    }
    fs.writeFileSync(path.join(cwd, "fixed"), "");
    console.log(dim("  ✓ 12 passed"));
    console.log(`${mark} fixed: parse_line skips an empty line`);
    turnEnd();
    keepane("pane-ready");
  }
}
