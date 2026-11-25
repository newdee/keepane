# agent 定义文件：不改代码也能接入新的 agent（设计稿）

状态：第 1 步已实现（0.91.0，验收第 228 条），插件目录带 `agent.toml` 这一来源留到之后；第 2 步起未实现。2026-11-25，robotcc 提交 PR #1，用代码加入 codewhale。讨论这个 PR 时，用户问能不能"让用户自己写插件或者配置，来兼容不同的 agent hook"。讨论之后用户说："改吧，先写设计文档"。

## 1. 目标与非目标

目标：
- 一个 keepane 不认识的 agent，只要它有 hook（能在事件发生时运行一条命令），写一个数据文件就能接入，不改 keepane 的代码，也不用等发版。
- 接入后的效果和内置的 agent 一样：认出 pane 里的 agent，知道它空闲、忙、等你，`keepane setup` 能安装，C-b v 能看到它，pane 恢复后能接着原来的对话。
- 贡献者加一个 agent，只交一个数据文件。

非目标：
- 不在 keepane 里运行用户写的代码（脚本、动态库、WASM）。理由见第 9 节。
- 不用规则描述各家的会话记录格式。token、费用、Trace 改由 agent 的 hook 主动上报（第 7、8 节）。
- 没有 hook 的 agent，只能靠屏幕文字判断。这个限制和现在一样。

## 2. 现状：keepane 对每个 agent 要知道什么

逆向看一个 keepane 不认识的 agent，现在缺的是下面 7 类知识。它们现在都写死在代码里，分散在 3 个互不相通的列表里：

| # | 知识 | 现在在哪 | 覆盖哪几家 |
|---|---|---|---|
| 1 | 认出 agent：进程名，或 `node`/`bun` 等启动器的命令行 | `agents::Kind::of` | Claude、Codex、pi、Cursor |
| 2 | 默认 `ai` 模式（`create-pane` 按程序名） | `setup::Agent::of_program`（`mail.rs:1100`） | `setup` 认的 6 家 |
| 3 | 事件名：会话开始、一轮结束、等你 | `setup::events`、`wait_event` | 6 家 |
| 4 | 安装：写哪个文件、什么格式 | `setup::places`、`with_hooks`、`with_cursor_hooks`、`own_file` | 6 家 |
| 5 | hook 输入（标准输入的 JSON）里的字段名 | `setup::hook_said`：`session_id`、`conversation_id`、`transcript_path`、`model`、`status` | 写死，几家共用 |
| 6 | 屏幕上表示"在等你""在干活"的文字 | `screenrules.rs` | Codex、Cursor |
| 7 | 恢复对话的命令 | `agentsync::Agent::resume` | Claude、Codex、pi；只用来提示，不运行 |
| 8 | 会话记录的格式：token、费用、一轮结束、Trace | `agents.rs`、`trace.rs` 各一个解析器 | Claude、Codex、pi |

第 1 到第 7 类都是数据。第 8 类是解析逻辑，不放进定义文件（第 7、8 节换一种做法）。

问题不只是"缺一个配置文件"：3 个列表各管一部分，同一个 agent 在一个列表里有、在另一个里没有（Gemini 和 opencode 能 `setup`，却不会被认出来）。这一项要一起收敛（第 10 节）。

## 3. 方案总览

1. 一种定义文件，格式是 TOML，一个 agent 一个文件。
2. 定义从 3 个地方来，后面的不能覆盖前面的：
   1. 内置：仓库里的 `agents/*.toml`，编译进 keepane（`include_str!`）。贡献者加 agent 就是往这里交一个文件，经过审查后随版本发出。
   2. 用户自己的：`~/.keepane/agents/*.toml`（Windows 是 `%USERPROFILE%\.keepane\agents`，和 `~/.keepane/plugins` 放在一起；`KEEPANE_AGENTS_DIR` 可以改，测试用它指向自己的定义）。
   3. 插件带的：插件目录里的 `agent.toml`，随 `set -g @plugin` 加载（第 1 步没有做，之后再加）。
3. agent 的 hook 调用 keepane 的 4 条命令。3 条已有、1 条新增：

   | 命令 | 意思 | 何时调用 |
   |---|---|---|
   | `pane-ready -q` | 空闲，可以送下一条消息 | 会话开始、一轮结束 |
   | `pane-busy -q`（新） | 开始干活，别往里贴消息 | 一轮开始 |
   | `pane-wait -q` | 在等你回答或批准 | 要权限、提问 |
   | `pane-step -q`（新，第 2 步） | 一次工具调用开始或结束 | 工具调用前后 |

4. 定义文件把"这家的事件名"对上"keepane 的命令"，再告诉 `setup` 把这几条 hook 写到哪里。

## 4. 定义文件

codewhale 的例子（事件名、格式、环境变量、`resume`、`codew` 别名、`mcp.json` 的键都按 CodeWhale 官方的 `README.md`、`docs/HOOKS.md`、`docs/MCP.md`、`docs/CONFIGURATION.md` 核过，2026-11-25，codewhale 0.10.4）：

```toml
# agents/codewhale.toml
name = "codewhale"              # the word for `keepane setup codewhale`
title = "CodeWhale"             # shown in `keepane setup` and the board
format = 1                      # this file's format version
resume = "codewhale resume {id}"   # top-level keys go before the first [table]

[detect]
programs = ["codewhale", "codew"]   # process names, any case, .exe/.cmd dropped; codew is its short alias
command_line = ["codewhale"]        # when the process is node, bun, deno or cmd

[events]                        # the agent's event names, for each keepane command
ready = ["session_start", "session_idle"]
busy = ["session_busy"]
wait = ["waiting_for_user"]
step_start = ["tool_call_before"]   # step 2
step_end = ["tool_call_after"]      # step 2

[hook_input]                    # where a hook's input says what; JSON pointer, or env:NAME
session = "env:DEEPSEEK_SESSION_ID"   # every event has it; stdin JSON only on some
model = "env:DEEPSEEK_MODEL"
tool = "/tool_name"             # step 2

[screen]                        # optional; text in the last lines, any case
waiting = []
working = []

[setup]
dir = "~/.codewhale"            # or the environment variable below when set
dir_env = "CODEWHALE_HOME"

[[setup.files]]                 # the hooks
path = "config.toml"            # under dir
how = "append"
mark = "keepane pane-ready"     # there already: installed
off_when = ["enabled = false"]  # there: say the hooks may be off
head_mark = "[hooks]"           # the head goes in only when no line is this
head = '''
[hooks]
enabled = true
'''
text = '''
[[hooks.hooks]]
name = "keepane-ready"
event = "session_start"
command = "{ready}"

[[hooks.hooks]]
name = "keepane-ready"
event = "session_idle"
command = "{ready}"

[[hooks.hooks]]
name = "keepane-busy"
event = "session_busy"
command = "{busy}"

[[hooks.hooks]]
name = "keepane-wait"
event = "waiting_for_user"
command = "{wait}"
'''

[[setup.files]]                 # the MCP server
path = "mcp.json"
how = "json-merge"
at = ["/servers/keepane", "/mcpServers/keepane"]   # whichever the file has (both: it fails to parse)
value = { command = "keepane", args = ["mcp"] }
```

### 4.1 字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `name` | 是 | 小写字母开头，只用小写字母、数字、`-`，最多 32 个字符。`keepane setup` 用的词，也是 `pane-*` 命令 `--agent` 的值 |
| `title` | 否 | 显示用的名字，默认同 `name` |
| `format` | 是 | 定义文件格式的版本，现在是 1。keepane 不认识的版本：跳过这个文件，并说明原因 |
| `detect.programs` | 是 | 进程名。比较时不分大小写，去掉路径和 `.exe`/`.cmd`/`.bat`。不能是启动器（node、bun、deno、cmd、python）、shell（pwsh、bash、zsh、fish、sh、wsl……）或 keepane 有代码支持的 agent 的程序：否则所有这类进程都会被认成它（一个 PowerShell pane 会被当成 agent，`create-pane` 默认给它 `ai` 模式） |
| `detect.command_line` | 否 | 进程是 `node`、`bun`、`deno`、`cmd` 时，命令行里包含其中一段就算；每段至少 3 个字符 |
| `events.*` | 至少 `ready` | 这家的事件名。安装文本里的 `{ready}` 等换成对应的命令 |
| `hook_input.*` | 否 | 取值的位置：`/a/b` 是 hook 标准输入 JSON 里的路径，`env:NAME` 是 hook 的环境变量。没写的项照现在的规则猜（`session_id` 等） |
| `screen.waiting` / `screen.working` | 否 | 屏幕最后几行里的文字片段，不分大小写（和现在 Codex、Cursor 的规则同一种判断，不用正则） |
| `resume` | 否 | 恢复对话的命令，`{id}` 换成会话 id |
| `setup.dir` / `setup.dir_env` | 否 | agent 的配置目录。`~` 是用户目录。环境变量有值时用它 |
| `setup.files` | 否 | 安装要改的文件，见 4.2。没有它，`setup` 只显示"手动加这几条 hook" |

### 4.2 安装方式

只有 3 种。每种只写一次代码，所有 agent 共用：

| `how` | 做什么 | 判断装过没有 |
|---|---|---|
| `append` | 文件末尾追加 `text`；文件没有就新建。有 `head` 时，文件里没有一行等于 `head_mark`，就先追加 `head`（TOML 的 `[hooks]` 这种表头在一个文件里只能出现一次，重复会让 agent 读不了配置） | 文件里已有 `mark` |
| `json-merge` | 在 JSON 的 `at` 位置放 `value`；中间缺的对象补上；其他内容和顺序不动。`at` 可以是一个列表：用第一个"上一层对象已经存在"的位置，都不存在就用第一个（codewhale 的 `mcp.json` 用 `servers` 或 `mcpServers`，两个都有会解析失败） | 列表里任一位置已有值 |
| `own-file` | 整个文件写成 `text`（opencode 的插件、pi 的扩展属于这种） | 文件已存在且含 `mark` |

规则沿用现在的 `setup`：
- 不带 `--install` 只显示要写什么。
- 写之前先备份原文件。
- 只加 keepane 的部分，不改用户的其他内容。
- 文件不是普通 JSON（有注释）时，`json-merge` 不动它，只显示要手动加的内容。
- `off_when` 里的文字出现在文件里时，`setup` 提示"hook 可能被关掉了"，并且不显示"已装好"。这条用来防 PR #1 里的问题：配置里已有 `[hooks] enabled = false` 时，hook 装上了却从来不触发。

`text` 里能用的占位符：`{ready}`、`{busy}`、`{wait}`、`{step_start}`、`{step_end}`。每个都换成完整的命令，例如 `keepane pane-ready -q --agent codewhale`。命令不加引号（README 已写明原因：Windows 上的 agent 可能用 PowerShell 运行 hook，带引号的路径后面跟参数是语法错误）。

## 5. 运行时：hook 调用的命令

### 5.1 `--agent NAME`

定义文件装的每条 hook 都带 `--agent NAME`。客户端（hook 运行的那个 `keepane` 进程）按这个名字找到定义，再按 `hook_input` 从标准输入或环境变量里取会话、模型、工具名，交给 server。

- 取值的限制沿用 `hook_said`：去掉首尾空白，超过 4096 字节、含控制字符的不取。
- 标准输入最多读 64 KB；`pane-ready` 最多等 1 秒，`pane-wait` 最多等 2 秒（同现在）。`pane-busy` 不读：它只说"开始干活了"。
- 不带 `--agent` 的（内置 agent 现在装的 hook）照现在的规则取，行为不变。
- `--agent` 的名字找不到定义：照现在的规则取，不报错（hook 失败会打扰 agent）。

### 5.2 `pane-busy -q`（新命令）

- 在 `ai` 模式的 pane 里：pane 记为忙（`Actor::turn_began`，和会话记录说"一轮开始了"的效果相同）；从空闲变忙时，事件日志记一条 `busy`（`"by":"hook"`）。
- 其他模式：忽略，和 `pane-ready` 一样。
- `-q`：不在 keepane 的 pane 里运行时，什么也不说、成功退出。
- 收到 `pane-busy` 也说明这个 agent 装了 hook：pane 不再算"没听到过"（`unheard`）。
- 没有 `-t`：只有 agent 自己能说自己忙。人要解开卡住的 pane，仍用 `pane-ready -t`。

要它的原因：agent 自己开始一轮（定时任务、自动续跑）时，没人按键，也没有消息进去。现在只有读会话记录的 3 家能发现这种情况。没有它，pane 显示空闲，排队的消息会在 agent 干活中途贴进去。

### 5.3 一个事件对多条命令

一个事件名可以出现在多个 `events.*` 里（例如 codewhale 的 `session_start` 既是 `ready`，以后也可能是别的）。安装时每个对应写一条 hook。

## 6. 认出 agent 与屏幕规则

- 认出：watcher 现在每秒看一次每个 pane 的进程树，用 `Kind::of` 判断。改成先查内置的 4 家，再查定义文件里的 `detect`。认出后：
  - pane 的 agent 栏、C-b v、`list-agents` 显示它的 `name`（和内置 agent 显示 `claude` 这类词一样）；`title` 用在 `keepane setup` 里。
  - `create-pane` 启动它时，默认进入 `ai` 模式。
  - `unheard` 的提示（"装 hook：`keepane setup codewhale --install`"）和按 agent 种类统计的地方（C-b v 的 agent 栏、时钟）都算它。
- `agent-commands`（agent 能启动哪些程序）不自动加入定义里的程序。它是安全设置，由用户自己改。
- 屏幕规则：只对认出是这个 agent 的 pane 生效，判断方式和现在 Codex、Cursor 的相同（最后 12 行里的等你，最后 6 行里的在干活，连续 2 次看不到才算结束）。

## 7. 恢复对话（第 2 步）

现在 `keepane resume` 重新运行 pane 原来的命令，`claude` 恢复成一个全新的 `claude`，对话接不上（README："程序自己在做的事不会回来"）。

改成：
1. 保存 session 时，每个认出了 agent 的 pane 多记两项：agent 的名字、它当前的会话 id（hook 交来的，或从会话记录认出的）。
2. 恢复时，pane 原来的命令就是这个 agent，并且它有 `resume` 模板：运行 `resume`（`{id}` 换成记下的 id），不运行原来的命令。
3. 内置的模板：`claude --resume {id}`、`codex resume {id}`、`pi --session {id}`。Gemini、Cursor、opencode 先不写，等核对了它们的命令再加。
4. 新选项 `restore-agents on|off`，默认 `on`。`off`：照旧运行原来的命令。
5. 同一个会话 id 只恢复一次：两个 pane 记着同一个 id 时，只有第一个用 `resume`，第二个照旧启动全新的。
6. 会话 id 不合规（不只是字母、数字、`-`、`_`）：不用它，照旧启动全新的。防止 id 里夹带命令。
7. 恢复命令失败（会话被删了），agent 报错退出，pane 回到 shell，屏幕上能看到原因，不静默。

## 8. Trace 与用量：由 hook 上报（第 3 步）

不用规则描述会话记录的格式。原因：3 家的格式完全不同（Claude 的工具调用藏在 `message.content` 的块里，Codex 用 `payload.type`，codewhale 是一整个 JSON 文件、每次整体重写），能描述它们的规则会变成一门小语言。

改由 hook 上报：
- `pane-step -q --agent NAME --start|--end`：工具名按 `hook_input.tool` 取，分类沿用 `SpanKind::of_tool`（读文件、改文件、运行命令……）。Trace 显示每一轮、每次工具调用和各自用的时间。
- 一轮的开始和结束，来自 `pane-busy` 和 `pane-ready`。
- 以后再加 `pane-usage`：hook 报 token 和费用。没报的，和现在的 Cursor 一样，只显示 CPU 和内存。

和读会话记录相比，上报的 Trace 少两样：提示词原文和子 agent 的细节；keepane 重启前的轮次也看不到（只有它运行期间收到的）。内置的 3 家继续读会话记录，不受影响。

## 9. 为什么是数据文件，不是能执行代码的插件

1. 安全：server 里运行用户的代码，出错会拖垮所有 pane。数据文件写错，最坏是这个 agent 不生效。
2. 够用：第 2 节的 8 类里，前 7 类本来就是数据。
3. 已有插件机制：`set -g @plugin` 运行的是 keepane 的命令，不是任意代码。定义文件放进插件目录，就能跟着插件一起分发。
4. 写进 agent 配置文件的 hook 命令，由 agent 用用户自己的权限运行。这和现在 `setup` 装的 hook 一样，`setup` 不带 `--install` 时会先显示出来。

## 10. 代码结构

- 一个注册表（新模块 `agentdef.rs`）：加载、校验、按名字和进程名查找。server 和客户端（`setup`、`pane-*` 的 hook 输入）共用。
- `agents::Kind` 加一项 `Defined(u16)`，指向注册表里的序号，仍然是 `Copy`。所有对 `Kind` 的 `match` 都要处理它：会话记录相关的分支什么也不做，和现在的 `Cursor` 一样。
- `setup::Agent` 和 `agentsync::Agent` 暂时保留。`keepane setup` 的列表把内置的和定义的合在一起显示。第 4 步把内置 agent 的前 7 类知识改成 `agents/*.toml`，那时再把 3 个列表收成 1 个。
- 新依赖：`toml` crate（只用来解析）。理由：定义文件由人手写，要能写注释和多行文本；JSON 两样都做不到。

### 10.1 加载与错误

- 第一次用到时加载（server 里是第一次认进程时，`keepane setup` 每次运行都读）；新命令 `agent-defs` 列出 server 手里的定义（内置的、用户的）和加载错误；`agent-defs -r` 重新加载。（现有的 `list-agents` 列的是正在运行的 agent，这个名字已被占用。）
- 一个文件出错只跳过这一个，其他照常；错误由 `agent-defs` 和 `keepane setup` 列出（每条带文件名和原因）。
- 名字和内置的重复：跳过用户的，说明原因（第 1 步不允许覆盖内置的，避免装错 hook）。
- 文件大小上限 64 KB，`programs`、事件名、文字片段各最多 32 项、每项最多 200 字节。

## 11. 分步

| 步 | 内容 | 版本 |
|---|---|---|
| 1 | 定义文件和注册表；认出 agent；`events` 的 `ready`/`busy`/`wait`；`hook_input`；`setup` 的 3 种安装方式和 `off_when`；屏幕规则；`pane-busy`；`agent-defs`；`agents/codewhale.toml` | 0.91.0 |
| 2 | 恢复对话：`resume` 模板、`restore-agents` 选项，内置的 3 家一起 | 0.92.0 |
| 3 | `pane-step` 和 Trace | 之后 |
| 4 | 内置 agent 改用 `agents/*.toml`，3 个列表收成 1 个 | 之后 |
| 5 | `pane-usage` | 需要时 |

每一步各自验收（三轮干净）。第 1 步要测的：
- 原行为不变：内置 6 家的 `setup` 输出逐字节和现在相同；不带 `--agent` 的 hook 取值不变；没有定义文件时一切照旧。
- 机制通路：改 `detect.programs`，认出的结果跟着变；改 `events`，安装的文本跟着变；去掉 `pane-busy` 的处理，测试要失败。
- 边界：文件不是 TOML、`format` 不认识、名字和内置的重复、字段超长、`json-merge` 的目标不是对象、文件有注释。
- 真机：用一个假的 agent（测试里写的小程序，按定义运行 hook 命令）走一遍认出、安装、空闲、忙、等你。能装 codewhale 的话，再用真的 codewhale 走一遍。

## 12. 待定的问题

1. codewhale 的格式已按官方文档核过（`codew` 是官方别名，两个程序逐字节相同；`codewhale resume <id>`；`mcp.json` 的 `servers`/`mcpServers`；配置目录 `CODEWHALE_HOME`，不是 PR #1 用的 `CODEWHALE_CONFIG_DIR`；会话在 `sessions/<id>/` 目录里，不是 PR #1 读的 `sessions/<id>.json`）。hook 的值：每个事件都有 `DEEPSEEK_*` 环境变量，`session_idle`、`waiting_for_user`、`session_busy` 等另有标准输入的 JSON，`session_start` 没有。实际传了什么，用 0.10.4 实测确认（记录每个事件的环境变量和标准输入）。
2. `restore-agents` 的默认值：本稿用 `on`。
3. 用户定义的 agent 能不能覆盖内置的：本稿第 1 步不允许，第 4 步再定。

## 13. PR #1 怎么处理

- codewhale 的支持：改成交 `agents/codewhale.toml`（第 1 步做完以后）。
- `.cmd`/`.bat` 改用 `cmd.exe /c` 启动：影响 Windows 上所有不是 PowerShell 的程序，单独提一个 PR，补上参数里有 `&`、`|`、`^`、`%`、空格、引号时的测试，以及 pane 前台程序名变成 `cmd` 之后的影响。
- `docs/quickstart.zh-CN.md`：和 README 重复，不合并。
- PR 里"没装 hook 也能从会话记录判断空闲"这句和代码不符（它的解析代码不设置 `turn`）：在 PR 上指出。
