# 窗格消息、工作模式与观测平台（设计稿）

状态：已实现（0.15.0）。本文记录 2026-09-26 与用户逐项确认的决定，实现与验收中的修正已回写（验收记录见 `docs/acceptance.md` 第 54 条）。

## 1. 目标与非目标

**目标**

- 每个窗格像一个 actor：有名字、有收件箱、能收发消息。
- 窗格之间（跨窗口、跨会话）能互发消息，或让另一个窗格执行命令。
- 不同窗口里的 agent（Claude Code、Codex 等）能互相派任务、收结果，也能自己开会话、窗口、窗格来编组干活。
- 有一个观测平台：每个窗格在做什么、处于什么状态、任务花了多久、产出是什么，都能查到，并且持久化、定期清理。

**非目标**

- keepane 不替程序"处理"消息，也不替它决定下一站。它负责投递、判断空闲、记录；处理与路由由窗格里的程序决定（agent 调用 `send-message`）。
- 不做单独的 daemon。服务端本来就是唯一经手所有消息与状态变化的进程，观测是它内部的一个模块。
- 第一版不做：流水线式"预先写好下一站"、OpenTelemetry 导出、手机端 dashboard、cmd/bash 的提示符钩子。

## 2. 概念

每个窗格新增以下属性：

| 属性 | 说明 |
|---|---|
| 名字 | 可选，整个服务端内唯一；用 `%名字` 定位 |
| 工作模式 | `normal`（默认）/ `shell` / `ai` |
| 收件箱 | 先进先出队列，条数上限 `message-inbox-limit` |
| 空闲 / 忙 | 只对 `shell`、`ai` 有意义 |
| 当前消息 | 正在处理的那条：决定回信对象、跳数、所属任务 |
| 创建者 | 由哪个 agent 窗格用 `create-pane` 创建（MCP 的创建工具就是它；人开的、普通 `new-window`/`split-window` 开的不记录），用于配额 |
| 状态文本 | agent 上报的进度（`pane-status`），服务端盖章来源 |

没有单独的发件箱：发出即进入对方收件箱；发送记录在事件日志里。

## 3. 定位

所有带 `-t` 的命令都接受：

| 写法 | 含义 | 稳定性 |
|---|---|---|
| `%7` | 窗格编号（已有） | 服务端运行期间不变、不复用 |
| `%builder` | 窗格名字（新增；不能是纯数字） | 跨重启（随会话存档保存） |
| `$1:@3.%7` | 完整 ID：会话编号、窗口编号、窗格编号（新增） | 以 `%7` 定位，前缀用于校验 |
| `work:2.1` | 会话名:窗口序号.窗格序号（已有） | 会漂移，不建议存下来用 |

完整 ID 的前缀与窗格当前所在不一致（窗格被 `join-pane`、`move-window` 等移动过）时报错，例如 `pane %7 has moved: it is at $2:@5.%7`，不投递。宁可报错，不可投错。

`keepane whoami`（MCP：`whoami`）在窗格里运行，打印自己的完整 ID、位置写法、名字、模式：

```
$1:@3.%7  work:2.1  builder  ai
```

名字规则：字母、数字、`-`、`_`，1–64 个字符，不能全是数字；与已有名字冲突时报错。窗格被关掉后在 `undo-kill` 保留期间，它的名字视为空出；撤销时如果名字已被别的窗格取走，恢复的窗格不带名字（并提示），`resume` 同理。

## 4. 工作模式与状态机

| 模式 | 空闲由谁判断 | 投递 |
|---|---|---|
| `normal` | 不判断 | 不自动投递；程序用 `read-message` 取 |
| `shell` | keepane：只认 keepane 自己钩子发出的提示符标记 `OSC 7777;keepane-prompt` | 空闲时打入并回车执行 |
| `ai` | agent：只认它调用的 `pane-ready` | 空闲时打入并回车 |

| 事件 | normal | shell | ai |
|---|---|---|---|
| keepane 提示符标记 | — | → 空闲 | → 忙（agent 已退出，回到了 shell） |
| 程序调用 `pane-ready` | 忽略 | 忽略 | → 空闲 |
| 键盘输入、粘贴、`send-keys`、`pipe-pane -I` | — | → 忙 | → 忙 |
| 鼠标、滚轮 | 不算 | 不算 | 不算 |
| 投递一条消息 | — | → 忙 | → 忙 |

- 窗格新建时为"忙"，等第一个空闲信号（shell：第一个提示符；ai：SessionStart hook）。
- keepane 投递的命令运行期间如果有人按键（在回答那个程序，或提前打字——提前打的字会出现在下一个提示符的输入行上），结束这条命令的提示符不判空闲；人按回车得到干净的提示符后恢复。
- 提示符标记到达即判为在提示符（之后的按键按先后顺序使其变忙），但当前消息要等 60 ms（`PROMPT_SETTLE`）才结束、截取输出，这期间不投递：ConPTY 转发 OSC 不等它前面的文字画出来（实测 `keepane-cmd`、`9;9` 排到了它们之前写出的输出前面）。
- 只认 keepane 专用标记：远端 shell（ssh）发出的 OSC 133 不会让本地判为空闲。
- 已在运行、仍用旧钩子的窗格收不到 `keepane-prompt`，在 shell 模式下永远不空闲：消息只排队，不执行（安全的失败方式）。

**投递条件**：模式不是 `normal`、空闲、收件箱非空、程序未退出，且只取发送时对方模式（信封的 `via`）与现在一致的消息：写给 agent 的文字不会因为窗格中途被改成 `shell` 而被执行，这样的消息留在收件箱等人处理。满足时取一条，按第 5 节的包装打入，窗格转为忙，这条成为当前消息。检查时机：新消息到达、转为空闲、改模式。每次最多投递一条。

**当前消息何时结束**：shell 为下一个提示符；ai 为下一次 `pane-ready`。结束即记 `done` 事件（第 10 节）。`read-message` 取走的消息取走即完成（记 `read`），不成为当前消息，只记为"最近读到的"供 `-r` 回复：agent 在自己一轮里用 `wait_message` 取回信，不会结束它手上的任务；人读完回信再发新请求，开始新的任务链（实测发现：否则跳数会一直累加，几轮往来后就被跳数上限拒收）。

**卡在忙**：人在 agent 输入框里打字后又删掉，或 Ctrl+C 取消输入而提示符未重绘，窗格会一直忙。可以用 `pane-ready -t <窗格>` 解开。

## 5. 信封

### 5.1 格式

固定、带版本、紧凑的单行 JSON，只包元数据，正文不进 JSON（避免 `\n`、`\"` 转义增加 token、降低可读性）：

```
{"keepane":1,"id":12,"task":12,"from":"$1:@3.%7","name":"builder","mode":"ai","to":"$1:@4.%9","via":"shell","hop":1,"re":9}
```

| 字段 | 含义 | 出现 |
|---|---|---|
| `keepane` | 信封版本，同时标明"这是 keepane 信封" | 总有 |
| `id` | 消息编号（服务端递增） | 总有 |
| `task` | 所属任务：新发起的消息等于自己的 `id`；处理某条消息期间发出的，继承那条的 `task`；回信（`-r`）继承所回复消息的 `task` | 总有 |
| `from` | 发件方完整 ID；不是从窗格发的写 `"user"` | 总有 |
| `name` | 发件窗格名字 | 有名字时 |
| `mode` | 发件方发送时的工作模式 | 从窗格发时 |
| `to` | 收件方完整 ID | 总有 |
| `via` | 收件方工作模式：`shell` / `ai` / `normal` | 总有 |
| `hop` | 跳数 | 总有 |
| `re` | 回复的是哪条 | 回信时 |

- 字段顺序固定、无空格：同样的消息每次生成的信封逐字节一致。
- 信封在所有模式、所有出口（投递、`read-message`、`list-messages`、事件日志、MCP）都相同。时间不在信封里：事件日志把时间作为信封之外的字段（`"at"`），`trace-message` 与列表另行显示时间；MCP 工具返回的是对应命令的文字输出。
- 读取方忽略不认识的字段；新增字段不升版本，改变已有字段含义才升 `keepane`。
- 字段值都受字符限制（名字只含 `[A-Za-z0-9_-]`），信封里不可能出现 `#>`。

### 5.2 传输包装（唯一随模式不同的部分）

| 收件模式 | 打入窗格的内容 |
|---|---|
| `ai` | 信封行 + 正文 + 结尾行 `{"keepane":1,"end":12}`，一次括号粘贴后回车 |
| `shell` | PowerShell：`<# 信封 #> 命令`（行内注释）；bash、zsh：`: '信封'; 命令`（`:` 的参数，什么也不执行），多行用 `eval "$(printf '%s\n' '行1' '行2')"` 在 shell 本身里执行，成败取最后一行；PowerShell 的多行命令改写为一行 `. ([scriptblock]::Create(('行1', '行2') -join "`n"))` 作为一条命令执行，块内有错时整条记为失败（见第 12 节第 10 条） |
| `normal` | 不打入；`read-message` 输出信封行 + 正文 |

- 结尾行防伪：正文里伪造的信封行只会出现在真消息首尾之间；agent 需要确认时用 MCP `current_message` 查服务端记录。
- shell 包装让来源留在屏幕、shell 自己的历史和 keepane 历史日志三处。
- 投递进 ai 窗格的信封不附"如何回信"说明：只在 MCP 工具说明 / agent 配置里写一次。

## 6. 跳数、限额与配置项

跳数：窗格处理某条消息期间发出的新消息 `hop` = 那条的 `hop` + 1；不在处理任何消息时发出的从 0 开始。超过 `message-hop-limit` 拒收，发件方收到错误。它能拦住"处理中互相转发"形成的循环；agent 空闲后凭记忆重新发起的不在其内，靠收件箱上限兜底。

所有上限都是配置项（`~/.keepane.conf` 里 `set -g`，也可运行时改；超出范围报错，不截断）：

| 配置项 | 默认 | 范围 |
|---|---|---|
| `message-hop-limit` | 8 | 1–100 |
| `message-inbox-limit` | 100 | 1–10000 |
| `message-max-size` | 64 KB | 1 KB–1 MB |
| `message-wait-max` | 600 秒 | 1–86400（超时由每秒一次的定时检查判定，实际多等不到 1 秒） |
| `agent-pane-limit` | 8 | 0–256（0 = 禁止 agent 创建窗格） |
| `agent-commands` | Windows `pwsh powershell claude codex`，其他系统 `bash zsh sh claude codex` | 程序名列表 |
| `event-log` | on | on / off |
| `event-log-days` | 30 | 1–3650 |
| `event-log-max` | 20 MB/天 | 1 MB–1 GB |

## 7. 命令与格式变量

```
send-message [-t 窗格] [-r] [-w 秒] 文本 # 打印编号与当时状态（已投递/排队中）；被拒则报错、退出码非 0；-r 回复当前消息的发件人；-w 秒 等到送达，超时退出码非 0
read-message [-t 窗格] [-w 秒]         # 取出一条（normal 模式用）；-w 等待，上限 message-wait-max
list-messages [-t 窗格] [-a]           # 查看收件箱，不取出；-a 所有窗格
trace-message 编号 [-w 秒]              # 一条消息的时间线与状态（含 shell 执行结果）；-w 等到完成
drop-message [-u] 编号                 # 删除排队中的消息；-u 撤销最近一次删除（undo-kill-time 内）
move-message 编号 up|down|top          # 调整排队顺序
pane-ready [-q] [-t 窗格]              # 报空闲；-q 不在窗格里时静默退出；-t 解开卡住的窗格
pane-status [文本]                     # 上报进度，空文本清除
set-work-mode [-t 窗格] normal|shell|ai  # 在窗格里调用只能改它自己
rename-pane [-t 窗格] 名字              # 空名字清除
whoami
list-events [-t 目标] [-S 时长] [-n 行数]  # 如 -S 1h；-n 取内存里最近的行，不读文件
list-tasks [-t 会话]
show-task 编号
dashboard                              # 别名 dash；默认绑定 prefix v
create-pane [-k session|window|split] [-t 目标] [-s 会话名] [-h] [-c 目录] [-n 名字] [-m 模式] [-M 首条消息] [-- 程序]
setup [claude|codex|gemini|cursor|opencode] [--install]   # 不带 agent 列出各家状态；默认只打印要加的配置
mcp                                    # stdio MCP 服务端
```

新增格式变量：`#{pane_name}`、`#{pane_address}`（完整 ID）、`#{pane_work_mode}`、`#{pane_idle}`、`#{pane_unheard}`（ai 模式的 pane，agent 启动以来没说过空闲）、`#{pane_inbox}`、`#{pane_status}`、`#{pane_message}`（正在处理的消息编号）。

`prefix B` 的 `choose-jobs` 保持不变（操作：跳转、关闭、重启）；dashboard 是另一个入口，只观察，唯一例外是管理模式下管理排队的消息。

## 8. 权限

keepane 的这些规则防的是失误，不是恶意：同一 Windows 用户的任何程序本来就能连命名管道、对任意窗格 `send-keys`。经过实测与讨论（2026-09-26，录制演示时发现原先的规则让人在自己的 shell 里改别的窗格都被拒），只保留一条：

- **工作模式只能在窗格自己里切换**：从一个窗格里调用 `set-work-mode`，只能改这个窗格自己（`-t` 指向别的窗格会被拒）。这样，任何窗格里运行的东西（包括 agent）都不能把另一个窗格变成"收到什么就执行什么"的 shell。人在 keepane 之外的终端、快捷键或 `Ctrl+B :` 命令行里，可以改任何窗格。agent 改自己窗格的模式，影响的只是它自己。`create-pane -m` 在创建时指定模式，不算切换。

其余操作（改名、关闭窗格、`pane-ready -t`、读取或管理任何收件箱）对所有人开放：出错只影响成败；关掉的窗格有 `undo-kill`（10 秒）；MCP 工具在 Claude Code 里默认每次调用都要用户批准，放不放行 `kill_pane` 由用户决定。

`create-pane` 从 agent 的窗格（`ai` 模式，或由别的窗格用 `create-pane` 创建的窗格）调用时：只能启动 `agent-commands` 里的程序（只管程序名、不管参数，不给虚假安全感），记下创建者，一个 agent 及其子孙最多创建 `agent-pane-limit` 个窗格。人用它开的窗格不记创建者、不计配额。

不做"防恶意"（2026-09-26 用户决定）：同一用户的程序能读到文件、环境变量、剪贴板里的任何密钥，所以给窗格或会话发密钥挡不住 agent；可靠的做法是服务端按操作系统事实（请求进程属于哪个窗格的 job object）识别调用者，但会把核心逻辑绑在平台上。以后需要时放进平台接口作为可选能力（Linux/mac 可用 Unix socket 对端进程号 + 进程树/cgroup）。
## 9. MCP 与 agent 接入

`keepane mcp`：stdio MCP 服务端，由 agent 启动，继承 `KEEPANE_PANE` 得知自己是哪个窗格；只把工具调用转成服务端命令，自身无状态。第一版就做。

| 类别 | 工具 |
|---|---|
| 创建 | `create_session`、`create_window`、`split_pane`、`set_work_mode`、`rename_pane` |
| 查询 | `whoami`、`list_panes`、`current_message`、`trace_message`、`list_tasks`、`show_task`、`query_events` |
| 通信 | `send_message`、`reply`、`wait_message`、`list_messages`、`drop_message`、`move_message`、`set_status` |
| 销毁 | `kill_pane`（即 `kill-pane`，可 `undo-kill`） |

- 创建工具可带 `message`：创建时即放入新窗格收件箱，就绪后投递，没有"发早了"的时序问题。
- 默认模式：启动 agent 程序（`claude`、`codex`、`gemini`、`cursor-agent`、`opencode`）为 `ai`，启动 `pwsh`/`powershell`/`bash`/`zsh` 为 `shell`，其他为 `normal`；可用 `mode` 参数覆盖。
- Claude Code 接入：SessionStart 与 Stop 两处 hook 调 `keepane pane-ready -q`，并注册 MCP。实测：Windows 上 Claude Code 用 PowerShell 执行 hook，`"C:/x/keepane.exe" pane-ready -q`（带引号的路径加参数）是语法错误，所以写入的命令不带引号。`keepane setup claude` 打印这些配置；`--install` 才写入 `~/.claude/settings.json`，写前备份。

PowerShell 钩子：在现有提示符钩子末尾加 `OSC 7777;keepane-prompt`，每次显示提示符都发（不依赖是否有新历史）。bash 与 zsh 的钩子发 `OSC 7777;keepane-prompt;sh`（`;sh` 说明按 POSIX 写法投递），放在 PS1 末尾（bash 用 BEL 结尾：提示符解码会吃掉 ST 里的反斜杠）。

## 10. 观测平台

### 10.1 事件日志

- 服务端内部模块，复用历史日志的写线程、按天分文件、大小上限与定期清理。
- 一个存储，不按会话拆分；每条事件带会话、窗口、窗格字段，会话视图靠筛选。
- 位置 `%LOCALAPPDATA%\keepane\events\<socket>\YYYY-MM-DD.jsonl`（每个服务端一个目录，`-L` 并排运行的服务端互不读取对方的消息），JSON Lines，外部工具可直接读。
- 存消息正文，按 `message-max-size` 截断（与历史日志保存屏幕输出同一隐私边界）。
- 服务端重启不影响已记录的事件。启动时读回最近两天的事件（实测两天都满 20 MB 时，release 启动慢约 290 ms，只发生在服务端启动时）；dashboard 等每秒查询只读内存中的最近 5000 行（`list-events -n`），不读文件。

| 事件 | 内容 |
|---|---|
| `sent` | 完整信封与正文（即进入收件箱排队） |
| `rejected` | 原因：跳数超限、收件箱满、目标不存在、前缀不一致、无权限…… |
| `delivered` / `read` | 打入窗格 / 被 `read-message` 取走 |
| `done` / `failed` / `abandoned` | 当前消息结束；shell 附成败（PowerShell 只给 ok）与输出（截断时标明 `cut`）；失败的 shell 命令记 `failed`；agent 中途退出记 `abandoned` |
| `dropped` | 被删除（记录是谁删的），或窗格被关、服务端重启时收件箱里的消息；撤销删除记 `restored` |
| `moved` | 收件箱内调整顺序（谁、从第几位到第几位） |
| `pane` | `what` 为 `created`（及创建者、程序、模式）、`name`、`mode`、`idle`（转为忙不记：每次按键都会触发，量太大）、`status`（`pane-status` 上报；来源由服务端按调用方窗格盖章：完整 ID、名字、模式、程序，agent 无法冒充） |

没有回执。发件方知道送达情况的途径：

1. 发送当场：对方空闲时投递在同一条命令里完成，`send-message` 直接打印 `#12 delivered to %9`；对方忙则打印 `#12 queued for %9 (busy)`；被拒则报错、退出码非 0。
2. 等待：`send-message -w` 或 `trace-message 12 -w`，送达（或完成）即返回，超时退出码非 0。
3. 事后：`trace-message 12` 看时间线；dashboard 里看每条消息的状态。
4. MCP：`send_message` 返回与命令行相同的文字（`#12 delivered to …`），`trace_message` 可带等待。

"已投递"只表示已打入窗格，不表示已处理完；处理完是 `done`。

### 10.2 任务视图

同一 `task` 的消息组成一个任务。任务状态：有消息在排队或处理中为 `running`；有被拒收或 shell 失败的为 `failed`；有消息被删除、丢弃或 agent 中途退出的为 `cancelled`；否则 `done`。每一步的时间分为排队（等对方空闲）和处理（投递到完成）。

```
$ keepane list-tasks -t work
TASK  STATUS   AT             TOOK   STEPS  TITLE
#12   running  $1:@2.%7 (ai)  4m12s  3      run the tests and report
#7    failed   -              0.2s   1      deploy to staging

$ keepane show-task 12
14:28:02  #12  user → $1:@1.%3 (ai)  done  queued 0ms  took 35s  "run the tests and report"
14:28:37  #13  %lead → $1:@2.%7 (ai)  delivered  queued 2s  "run cargo test in sh1"
14:28:40  #14  %tester → $1:@3.%9 (shell)  failed  queued 0ms  took 2m50s  ok=0  output 2150B  "cargo test"
```
标题取发起消息正文第一行。只统计经过消息系统的工作；手敲命令见 `list-marks`，agent 中间过程在历史日志里。

### 10.3 Dashboard

（0.17 起由 [dashboard.md](dashboard.md) 的多面板版取代：不再有观测/管理两种模式，改为能直接操作、后果重的先确认。下面是 0.15 的原设计，留作记录。）

- `keepane dashboard`（别名 `dash`），默认绑定 `prefix v`；在 keepane 里以弹出窗口打开，也可在任意终端全屏运行。
- **两种模式，防误触**：
  - **观测模式**（每次打开的默认）：只调用查询类服务端命令。管理类按键不起作用，只提示"按 E 进入管理模式"。
  - **管理模式**：按 `E` 进入，顶栏变红并显示 `MANAGE`；按 `E` 或 `Esc` 返回，闲置 30 秒自动返回。只允许收件箱管理（`drop-message`、`move-message` 及撤销），其余仍是查询。
  - 测试检查：观测模式下不发出任何改变状态的命令；管理模式下只发出上述两类。
  - 跳转、关闭、重启不在 dashboard 里，请用 `prefix B`。以后搬到手机端时，管理模式的远程权限要单独考虑。
- 每秒取一次全部窗格的快照，事件从内存中最近的事件行取（`list-events -n`）。

```
 keepane dashboard   4 panes · 1 busy · 2 queued   14:51:17
   PANE      NAME          MODE    STATE   INBOX  DOING
 work
 > 0.0 %2                  normal  -           0  pwsh
   0.1 %4    tester        ai      busy        2  "tests 3/10"
 ops
   0.0 %6    sh1           shell   idle        0  pwsh
── %4 tester  · ai · claude · $1:@3.%4 ── events ─────────────────────
14:28:02  sent      #12 %lead → $1:@3.%4 (ai): run the tests and report
14:28:02  delivered #12
14:31:12  pane      status tests 3/10
 j/k pane  Enter events  m messages  t tasks  v live  h history  / filter  E manage  q quit
```

下半部分的视图：Enter 事件、`m` 消息、`t` 全部任务、`v` 实时屏幕、`h` 屏幕与滚动历史。

`m`：选中窗格的收件箱（`list-messages` 的输出），`n`/`p` 选择其中一条：

```
$1:@3.%4 tester (ai, busy) · 2 queued · working on #12
  #15  from $1:@3.%6 sh1 (shell)  waiting 40s  test results are in target/report.txt
  #16  from user  waiting 12s  have a look at lint too
```

- Enter 打开选中的那条（`trace-message`：完整信封、正文、经过、shell 的输出），再按 Enter 返回（后来改成光标移到哪条右边就显示哪条，排好版，见 [dashboard.md](dashboard.md)）；`a` 切换为所有窗格收件箱的合并视图。最近收发过的消息在事件视图里看。
- 查看不取出：与 `read-message` 不同，dashboard 看收件箱不取走消息。
- 管理模式下管理排队的消息：`d` 删除、`K`/`J` 上移/下移、`g` 置顶（下一条投递）、`u` 撤销上一次删除（`undo-kill-time` 秒内，与 `undo-kill` 同一时限）。删除不弹确认：模式切换加撤销两道兜底。
- 已投递（正在处理）的消息不能删：它已经打进窗格了。
- 不能编辑内容：信封写明来源，正文被第三方改动就不再可信。要改就删掉，自己重发一条（来源如实写成你）。- 命令行对应 `list-messages [-t 窗格] [-a]`，MCP 对应 `list_messages`。

### 10.4 normal 窗格的提醒

消息进入 normal 窗格时，用现有窗口提醒机制（与 bell、activity 同一套）在状态栏用 `@` 标出那个窗口，并在该会话的客户端显示一条提示；和其他提醒一样，窗口被看到时标记消失。

### 10.5 信封头的写法与按字段发送（0.17）

用户要求（2026-09-27）：直接看 JSON 太难看，并且发送时能按字段指定。

- 投递、`read-message`、`trace-message` 里的信封头改成字段式：`[keepane id=12 task=3 from=… name=… mode=… to=… via=… hop=1 re=9]`，字段名与顺序同 JSON，`名字=值` 以空格分隔；值不含空格和 `]`（名字限字母数字 `-` `_`，地址与模式词都没有），按空格切开即可读回。结束行 `[keepane end=12]`。选项 `message-envelope fields|json`，默认 `fields`；事件日志永远是 JSON（机器格式，不随选项变）。
- 投给 shell 的信封头缩短（0.35.3，用户要求：命令前的整个字段头太长、太难看，窄窗格里还折行）：`<# keepane #12 from %builder #> cargo test`、`: 'keepane #12 from %builder'; make`。只留编号和发件方（有名字写 `%名字`，否则写 `%窗格号`，否则 `user`；另一台机器的前面加 `主机:端口/`），因为 `shell-history` 读回时只用这两项；其余字段在事件日志里，`trace-message 12` 查看。agent 仍收完整字段头（它要按字段核对来源）。`message-envelope json` 时 shell 里仍写完整 JSON。`shell-history` 三种写法都认，旧历史照样识别。
- `send-message` 按名字给发送方能选的字段：`--to`（=`-t`）、`--re <id>`（回复某条，默认发给它的发送方）、`--task <id>`（接着某个任务，hop 取该任务已到达的最远一跳 +1，被拒收的消息不算）。`from`、`name`、`mode`、`id`、`hop`、`via` 不能由发送方给：否则来源可以伪造，hop 上限也拦不住循环；给了就报错说明原因。MCP 的 `send_message` 同样多了 `re`、`task`。

## 11. 持久化

- 名字、工作模式随会话存档保存；`resume` 后恢复（名字已被占用则不恢复，并提示）。创建者关系不保存：恢复后窗格编号重新分配，恢复出的 agent 也是新进程，原来的创建关系已无意义。
- 收件箱里的消息不保存：时机已过、收件进程已换、来源编号失效。重启时被丢弃的消息记 `dropped` 事件，恢复时提示"丢弃了 N 条未投递消息，详见事件日志"。
- `respawn-pane` 是同一个窗格换了程序：名字、工作模式、收件箱、状态都留下（2026-09-27 起；之前整个换掉，排队的消息无声消失）。新程序报出自己的提示符（或 `pane-ready`）之前不算空闲；旧程序手上那条以放弃（`abandoned`）记入事件日志。

## 12. 已知限制

1. 卡在忙（第 4 节），需人手 `pane-ready -t` 解开。
2. shell 模式要 keepane 的提示符钩子：PowerShell、bash、zsh（Linux/macOS 上由 keepane 启动时装上）；cmd、sh、fish 和 WSL 里的 shell 没有，永远不空闲。按窗格当时的 shell 选写法（提示符标记带 `;sh` 的用 POSIX 写法，见 platform.md），与操作系统无关。
3. 旧钩子的窗格要重开才能用 shell 模式。
4. 多行消息：ai 窗格靠括号粘贴（Claude Code 2.1.282 实测有效）；shell 窗格见第 10 条。
5. 跳数拦不住 agent 空闲后凭记忆重新发起的循环，靠收件箱上限兜底。
6. 投递与人手打字之间仍有毫秒级的竞争窗口。
7. 没有采用"备用屏幕中不投递"之类的规则，Claude Code 是否切备用屏幕未测。
8. 权限规则防失误不防恶意（第 8 节）。
9. shell 模式执行的命令（带信封注释）会进入 PSReadLine 的持久历史（`ConsoleHost_history.txt`），之后按上箭头或输入预测时会看到它们。要只留在当前会话历史里，需要在钩子里设置 `AddToHistoryHandler`，而这可能覆盖用户自己的 handler，暂不做。
10. 多行命令在 shell 模式下改写为一行执行（PowerShell 用 `. ([scriptblock]::Create(...))`，bash/zsh 用 `eval`）：PSReadLine 不启用括号粘贴，而 ConPTY 会丢掉 Shift+Enter 的 Shift（实测 `vk=13 state=0`），逐行打入会让每行单独执行。
11. 人自己手敲的命令运行期间提前打字：下一个提示符会判为空闲，若恰有消息排队，会接在这些字后面执行。要避免只能把"两次提示符之间有过按键"都判为忙，而人每次回车都是按键，窗格就再也不会空闲，所以不做。

## 13. 可移植性

（写作时 keepane 只有 Windows 版；现已支持 Linux 和 macOS，平台与 shell 的划分见 [platform.md](platform.md)。）keepane 目前是 Windows 版，以后可能做 Linux / mac。本设计的新模块（actor、信封、观测存储、MCP、dashboard）不调用任何平台 API。与 shell 相关的只有两处，按 shell 类型选择实现：提示符钩子（目前只有 PowerShell）与 shell 模式的信封包装（`<# … #>` 是 PowerShell 语法）；以后 bash / zsh 各加一份。

目录结构（已定）：本功能验收后，单独做一次纯重构提交，把 Windows 专用代码移到 `src/platform/windows/`，对外只暴露窄接口（pty、IPC 传输、按键编码、剪贴板、通知、开机启动、进程目录、系统信息），加一个测试扫描 `platform/` 之外不得出现 Windows API；重构前后测试结果必须一致。真正开始做 Linux / mac 时，再按同一划分拆成 `keepane-core` 与各平台 crate。

两个互相独立的维度：**平台**按操作系统分，Linux 与 mac 同属 Unix（pty、Unix socket、进程组与信号、VT 输入），共用 `platform/unix/`，只在进程目录、系统信息、通知、剪贴板、开机启动、安装包处用 `cfg(target_os)` 区分；**shell** 与系统无关，按 shell 分：`shell/{powershell, bash, zsh, fish}`，每种提供提示符钩子与 shell 模式的信封包装（bash/zsh/fish 用 `#` 行注释），所有系统共用。mac 默认 zsh、多数 Linux 默认 bash，但不能按系统推断 shell。

## 14. 实现顺序与验收要点

1. `src/server/actor.rs`：纯状态机（模式、空闲/忙、收件箱、跳数、任务、限额）与信封，无 IO；`src/server/observe.rs`：事件与任务视图；`src/server/mail.rs`：服务端命令、投递、等待、权限。
2. 目标解析：`%名字`、完整 ID 与前缀校验；`rename-pane`、`whoami`。
3. 服务端接入：输入写入点标记忙；提示符标记；投递与包装；命令与格式变量；存档字段。
4. 事件日志与查询：`trace-message`、`list-events`、`list-tasks`、`show-task`，`-w` 等待。
5. 权限与配额。
6. `keepane mcp`、`keepane setup claude`。
7. dashboard。
8. README（中英）、parity 文档、网站、`docs/acceptance.md`。

验收要点（三轮干净规则照常）：信封逐字节一致（同样输入连跑多次）；每种模式组合 3×3 的投递行为；伪造信封、跳数超限、收件箱满、前缀不一致、越权修改各有测试；dashboard 只读性；用真实 Claude Code 实测 ai 模式的就绪、投递、多行粘贴（已测，见验收记录）。信封的 token 数未实测（没有可用的分词器）：示例信封 123 字节，结尾行 22 字节。
