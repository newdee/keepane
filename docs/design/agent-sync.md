# 把 agent 的对话送到另一台机器（设计稿）

状态：草稿，等用户定第 9 节。用户已定（2026-10-30）：
- 目的：Claude Code、Codex 的对话只存在本机；要能带到另一台机器，在那里用 agent 自己的 resume 接着聊。
- 方式：手动按需送（一条命令），不做后台自动同步；以后再送只送新增的部分。
- 范围：默认送全部项目的对话。
- 路径：按家目录的相对路径对应；有 `clone`/`push` 记录的项目按记录放。

## 1. 目标与非目标

**目标**

- 一条命令，把本机 Claude Code、Codex、pi 的对话送到已配对的另一台机器（[link.md](link.md)）。到那边后，在对应的项目目录里运行 `claude --resume`、`codex resume`、`pi --resume`，就能看到并接着聊。
- 两台机器的系统、用户名、路径不同时也能用（例如 Windows 的 `C:\Users\stebe\Documents\app` 到 Mac 的 `/Users/tom/Documents/app`）。
- 第一次送全部；以后再运行，只送两边不同的部分。

**非目标（第一版不做）**

- 后台自动同步、双向同步、合并两边各自续写的同一段对话。
- Cursor CLI、Gemini CLI、opencode：keepane 不读它们的对话，格式未核实。
- agent 的设置、记忆（`~/.claude/projects/*/memory`）、skill、MCP 配置。
- keepane 的布局（resurrect）。

## 2. 每种 agent 存在哪里，要改什么

| agent | 文件 | 按什么找 | 要改写的内容 |
|---|---|---|---|
| Claude Code | `~/.claude/projects/<目录名>/<id>.jsonl`，加上 `<id>/subagents/*`、`<id>/tool-results/*` | 目录名由项目路径算出（非字母数字都换成 `-`） | 目录名；带 `cwd` 字段的行（`user`、`assistant`、`attachment`、`system`）的 `cwd` |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl` | `session_meta` 里的 `cwd` | `session_meta.payload.cwd`、`turn_context.payload.cwd` |
| pi | `~/.pi/agent/sessions/<目录名>/<文件>.jsonl` | 目录名由项目路径算出 | 目录名；`session` 行的 `cwd` |

- 只改上表的字段。对话正文里出现的旧路径（例如工具读过的 `C:\…\main.rs`）不改：那是历史。agent 接着聊时按新的 `cwd` 工作。
- 改写在对方机器上做：对方知道自己的家目录和路径分隔符。
- 本机实测（2026-10-29）：Claude Code 17 个项目，共 1,259 MB，最大的一段对话 220.9 MB；Codex 2 段，0.1 MB。

## 3. 命令

```
keepane send-agents <对方地址> [目录…] [-n] [--map 本机路径=对方路径]…
```

- 不写目录：送全部项目的对话。写了目录：只送这些项目的（例如 `keepane send-agents laptop:8765 .`）。
- `-n`：只列出要送什么、放到对方哪里、多大，不送。
- 每段对话按第 4 节决定放到对方哪里。放不了的（不在家目录下、又没有记录）不送，结尾列出来，提示用 `--map`。
- 正在进行的对话（agent 这一轮还没结束）：先送到上一轮结束为止的部分，结尾说明。下次再运行时送剩下的。
- 结尾打印：送了几段、多少字节、跳过了哪些；每个项目在对方机器上怎么接着聊（`cd ~/Documents/app && claude --resume`）。

## 4. 路径怎么对应

对每段对话的项目目录 `P`（本机路径），按顺序取第一个能用的：

1. `--map` 写了的：`P` 在某个 `本机路径` 下面，换成对应的 `对方路径`。
2. `keepane clone`/`push` 给这台对方机器记下的仓库位置（[link-push.md](link-push.md) 第 2 节）：`P` 在某个本机仓库下面，换成对方的仓库路径。
3. `P` 在本机家目录下面：对方家目录加上同样的相对路径，分隔符换成对方的。`C:\Users\stebe\Documents\app` 到 Mac 是 `/Users/<对方用户>/Documents/app`。
4. 都不行：跳过，列出来。

- 系统临时目录下的项目（`%TEMP%`、`/tmp`）不送：那是一次性的目录（本机有 1 个）。
- Windows 的盘符和大小写：比较时不分大小写；对方是 Windows 时保留本机的大小写。
- 对方路径上还没有这个目录：照样放好对话。以后在那里建项目（例如 `git clone`），就能 resume。

## 5. 权限

- 对方授予：`keepane link allow <本机地址> --agents`。默认关。和 `--push` 一样，只能在 pane 之外授予。
- 对话记录里有 agent 读过的文件内容、运行过的命令和输出，可能包含密钥。这个权限比 `--push` 更敏感，所以单独一项。

**警告：** link 的请求有签名，但不加密（[link.md](link.md) 第 32 行）。在局域网里传，同一网络的人能读到对话内容。请在 Tailscale 上用，或只在可信的网络里用。`send-agents` 每次运行都在开头说一次送往的地址是否在 Tailscale 上。

## 6. 传输

复用 `push` 的分块上传（[link-push.md](link-push.md) 第 4 节）：

1. **问（`POST /link/agents/look`）**：本机发清单：每个文件的 agent、对方路径、大小、开头 64 KiB 的 SHA-256。对方答：每个文件它已有多少字节，已有部分的开头是否和本机一致。
2. **只送缺的部分**：transcript 只在末尾追加。对方已有开头一致的前 N 字节时，只送 N 之后的部分。`tool-results`、`subagents` 里的文件同理（已有且大小相同的不送）。
3. **压缩**：每个要送的部分先用 gzip 压缩再分块（实测 5.7 MB 的对话压到 1.4 MB，24%）。本机全部 1,259 MB 第一次约送 300 MB。
4. **上传**：`POST /link/agents/chunk`，每块 512 KiB，每块签名。清单里只有对方路径、agent 和原来的 `cwd`，不放本机家目录的绝对路径。
5. **应用**：`POST /link/agents/apply`，带每个文件整体的 SHA-256。对方解压、拼好、核对，按第 7 节写入。

- 一次运行可以分成多批送：每批不超过 256 MiB（压缩后）。一批失败时，已写好的批不受影响，再运行一次从断处接着送。
- 同一台机器同一时间只能有一次传输。

## 7. 对方怎么落地

1. 按第 4 节算出的对方路径，逐个文件按第 2 节改写，写进对方自己的 agent 目录（`CLAUDE_CONFIG_DIR`、`CODEX_HOME` 照常认）。
2. 同名文件已存在时：
   - 已有文件是新文件的开头部分（对方没有续写过）：在末尾接上新增部分。
   - 否则（对方续写过，两边分叉了）：不覆盖，改用新的 id 另存一份，结尾说明。id 在文件名里，也在文件内容里（Claude Code：400 行里有 393 行带 `sessionId`；Codex：`session_meta` 等 2 行），另存时文件名和内容一起改。
3. 先写临时文件，再改名。写到一半失败，不留下坏文件。
4. 写入前用 keepane 自己的解析器（`agents::feed`）读一遍改写后的文件，读不出轮次就拒绝写入，并说明是哪个文件。

## 8. 风险

- **agent 改格式**：transcript 不是公开接口。Claude Code 2.1.3xx 只在 `user`、`assistant`、`attachment`、`system` 行写 `cwd`，其他行（`ai-title`、`api-request` 等）不写（2026-10-29 实测）；以后可能变。对策见第 7 节第 4 步。
- **resume 找不到**：Claude Code 按目录名找对话。对方路径的大小写、末尾斜杠不同，目录名就不同。用 `claude_folder` 同一个函数算，测试覆盖 Windows 和 Unix 两种路径。
- **两边版本不同**：对方的 Claude Code 比本机旧时，可能读不懂新字段。这一点没法提前检查；结尾提示两边的 agent 版本（能读到时）。
- **第一次很大**：本机第一次约 1.26 GB（压缩后约 300 MB）。速度还没实测。

## 9. 要用户定的事

1. 命令名 `send-agents`？（推荐：是。和 `push` 分开：代码和对话的权限、时机不同。）
2. 权限新加一项 `link allow --agents`？（推荐：是，理由见第 5 节。）
3. 压缩：加一个依赖（`flate2`）？（推荐：是。全部项目第一次约 1.26 GB，压缩后约 300 MB。）
4. 要不要 `--open`，送完在对方机器上开 pane 接着聊？（推荐：第一版不做。送全部项目时不知道开哪个。）
5. 手机页面和 dashboard 的入口？（推荐：第一版不做。）
