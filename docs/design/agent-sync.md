# 把 agent 的对话送到另一台机器（设计稿）

状态：草稿，等用户定第 8 节。用户已定：同步对话记录（不同步布局）；送到另一台机器后，用 agent 自己的 resume 接着聊。

## 1. 目标与非目标

**目标**

- 在一台机器上和 agent 聊到一半，换到另一台机器（已用 [link.md](link.md) 配对），运行一个命令，就能在那台机器上用 `claude --resume`、`codex resume` 或 pi 的 `--resume` 接着聊。
- 支持 Claude Code、Codex、pi。这三种 agent 的对话是 JSONL 文件，keepane 已经会读（`src/server/agents.rs` 的 `transcripts`）。
- 两台机器的项目路径不同（例如 Windows 的 `C:\Users\me\src\app` 和 Mac 的 `/Users/me/src/app`）时也能用。

**非目标（第一版不做）**

- Cursor CLI、Gemini CLI、opencode：keepane 不读它们的对话，格式未核实，不同步。
- 双向同步、自动同步、合并两边各自续写的同一段对话。
- 同步 agent 的设置、记忆（`~/.claude/projects/*/memory`）、skill、MCP 配置。
- 同步 keepane 的布局（resurrect）。

## 2. 每种 agent 存在哪里，要改什么

| agent | 文件 | 按什么找 | 要改写的内容 |
|---|---|---|---|
| Claude Code | `~/.claude/projects/<目录名>/<id>.jsonl`，加上 `<id>/subagents/*`、`<id>/tool-results/*` | 目录名由项目路径算出（非字母数字都换成 `-`） | 目录名；带 `cwd` 字段的行（`user`、`assistant`、`attachment`、`system`）的 `cwd` |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl` | `session_meta` 里的 `cwd` | `session_meta.payload.cwd`、`turn_context.payload.cwd` |
| pi | `~/.pi/agent/sessions/<目录名>/<文件>.jsonl` | 目录名由项目路径算出 | 目录名；`session` 行的 `cwd` |

- 只改上表的字段。对话正文里出现的旧路径（例如工具读过的 `C:\…\main.rs`）不改：那是历史，改了反而失真。agent 接着聊时，看到旧路径，会按新的 `cwd` 工作。
- 改写在对方机器上做：对方知道自己的路径和路径分隔符。

## 3. 命令

```
keepane send-agent [-t pane] [<对方地址> [路径]] [--all] [--open]
```

- 默认送**一个 pane 的当前对话**：`-t` 指定的 pane（默认当前 pane）里 agent 正在写的那个文件。keepane 已经知道它（`Track` 和 `owners`）。
- `--all`：送这个目录下这种 agent 的所有对话（最近 30 天）。
- `<对方地址>`、`[路径]` 与 `keepane push` 一样。不写时，用 `push` 为这个仓库记下的对方路径（[link-push.md](link-push.md) 第 2 节）。没有记录又没写路径时，报错，提示先 `keepane clone`。这样代码和对话一起到达同一个目录。
- `--open`：对方收到后，在对方路径开一个新 pane，运行该 agent 的 resume 命令（`claude --resume <id>`、`codex resume <id>`、`pi --session <文件>`）。需要对方授予 `--panes`。
- 运行前提示：对话正在进行（agent 这一轮还没结束）时，拒绝并说明。送一个写到一半的文件，对方 resume 会出错。

## 4. 权限

- 对方授予：`keepane link allow <本机地址> --agents`。默认关。和 `--push` 一样，只能在 pane 之外授予。
- 对话记录里有 agent 读过的文件内容、运行过的命令和输出，可能包含密钥。这个权限比 `--push` 更敏感，所以单独一项。

**警告：** link 的请求有签名，但不加密（[link.md](link.md) 第 32 行）。在局域网里传，同一网络的人能读到对话内容。请在 Tailscale 上用，或只在可信的网络里用。`send-agent` 在非 Tailscale 地址上第一次使用时，打印一次这条警告。

## 5. 传输

复用 `push` 的分块上传（[link-push.md](link-push.md) 第 4 节）：

1. **问（`POST /link/agents/look`）**：本机发每个文件的 agent、相对路径、大小、开头 64 KiB 的 SHA-256。对方答：每个文件它已有多少字节，以及已有部分和本机的开头是否一致。
2. **只送缺的部分**：transcript 只在末尾追加。对方已有开头一致的前 N 字节时，只送 N 之后的部分。第一次送整个文件，以后每次只送新写的部分。
3. **上传**：`POST /link/agents/chunk`，每块 512 KiB，每块签名。文件清单里只有相对路径、agent 和原来的 `cwd`，不放本机的绝对路径。
4. **应用**：`POST /link/agents/apply`，带每个文件整体的 SHA-256。对方拼好后核对，按第 6 节写入，答复写了哪些文件、resume 命令是什么。

- 大小上限：一次 1 GiB（新选项 `link-agents-limit`）。本机最大的一个 Claude Code 对话有 220.9 MB（2026-10-29 实测），`push` 的 200 MiB 不够。

## 6. 对方怎么落地

1. 新 `cwd` = 对方路径（第 3 节）。
2. 逐个文件按第 2 节改写，写进对方自己的 agent 目录（`CLAUDE_CONFIG_DIR`、`CODEX_HOME` 照常认）。
3. 同名文件已存在时：
   - 已有文件是新文件的开头部分（对方没有续写过）：用新文件替换。
   - 否则（对方续写过，两边分叉了）：不覆盖，改用新的 id 另存一份，答复里说明。id 在文件名里，也在文件内容里（Claude Code：400 行里有 393 行带 `sessionId`；Codex：`session_meta` 等 2 行），另存时文件名和内容一起改。
4. 先写临时文件，再改名。写到一半失败，不留下坏文件。

## 7. 风险

- **agent 改格式**：transcript 不是公开接口。Claude Code 2.1.3xx 在 `user`、`assistant`、`attachment`、`system` 行写 `cwd`，其他行（`ai-title`、`api-request` 等）不写（2026-10-29 实测本仓库的对话）；以后可能变。对策：只改已知字段；改写后用 keepane 自己的解析器（`agents::feed`）读一遍，读不出轮次就拒绝写入。
- **resume 找不到**：Claude Code 按目录名找对话。对方路径大小写、末尾斜杠不同，目录名就不同。用 `claude_folder` 同一个函数算，测试覆盖 Windows 和 Unix 两种路径。
- **大文件**：本机最大的 Claude Code 对话 220.9 MB，Codex 最大 0.1 MB（2026-10-29 实测）。第一次要送整个 220 MB（加 base64 后约 293 MB），速度还没实测。第 5 节的"只送缺的部分"让以后每次只送新增内容。gzip 压缩能把一个 5.7 MB 的对话压到 1.4 MB（24%，实测），但要加一个依赖（`flate2`），见第 8 节。

## 8. 要用户定的事

1. 命令名：`send-agent`，还是并进 `push`（`keepane push --agents` 同时送代码和对话）？推荐：单独的 `send-agent`。代码和对话的权限、时机不同。
2. 默认送一个 pane 的当前对话，还是这个目录的全部对话？推荐：默认当前对话，`--all` 送全部。
3. 权限单独一项 `--agents`，还是并入 `--push`？推荐：单独一项（第 4 节的理由）。
4. 要不要 `--open`（对方自动开 pane 接着聊）？推荐：第一版就做，它依赖的 `--panes` 已经存在。
5. 要不要手机页面和 dashboard 的入口？推荐：第一版不做。
6. 要不要压缩传输（加 `flate2` 依赖，传输量约为原来的 1/4）？推荐：第一版不压缩，靠"只送缺的部分"；第一次的大文件慢一点。实测慢再加。
