# Trace：看清 agent 在做什么、卡在哪（0.65）

用户说「另外看一下能不能借鉴一下这个项目，对claude code显示的更详细一点，https://github.com/alexeyw/lot-map」，看完对比后说「改吧，lot-map 那几条按推荐来」，并要求 0.64.0 发布后做。

lot-map 是一个 Claude Code 插件（MIT）：聊天旁边一个圆形地图，中间是 CLAUDE，周围 10 个站点（You、Model、Memory、Files、Shell、Network、Browser、Agents、Skills、Hooks），下面是最近 16 行流水；数据来自 Claude Code 的 hook。借鉴它的信息，不照搬圆形地图（要 144 列以上，信息密度比列表低）。

keepane 的 Trace 读的是 transcript（Claude Code、Codex、pi），一件事做完才写一行。下面每一条先在 transcript 里找到依据（2026-11-13 在本机的 transcript 里核对过）。

## 1. 卡住的原因

- 用户拒绝：`tool_result` 带 `is_error`，`toolUseResult` 是 `"User rejected tool use"`（内容以 "The user doesn't want to proceed" 开头）。记为 `denied`。
- 被 hook 拦下：内容以 `PreToolUse:<工具> hook error: <原因>` 开头。记为 `hook: <原因第一行>`。
- 其他失败照旧，只标 `✗`。
- 在等你批准：keepane 已经接了 Claude 的 `Notification`（`permission_prompt|elicitation_dialog`）hook，`pane-wait` 记下 "waits for you: Claude needs your permission to use Bash"。`trace-pane -J` 带上这条（比最后一步新时），Trace 顶上显示。

## 2. 后台任务一直在

- 工具调用的输入里 `run_in_background: true`：这一步是后台的。它的 `tool_result` 马上回来（只是"已启动"），这一步不因此结束，也不因为这一轮结束而结束。
- 完成通知是一行 `turnOrigin: task_notification` 的用户消息，内容是 `<task-notification>`，带 `<tool-use-id>` 和 `<status>`（completed / failed / killed）：对上那一步，结束；不是 completed 记为失败，原因是状态。
- agent 用 `TaskStop`（或 `KillShell`）停掉的任务不会有通知：启动时工具回答里有 "running in background with ID: b1"，记下任务 ID 到这一步的对应；见到停止它的调用（输入里的 `task_id` / `shell_id`），这一步在那里结束，不算失败。
- agent 重启（transcript 里出现 `SessionStart` hook 的记录）：上一次运行的后台任务已经随它结束，开着的后台步骤都在那里结束。没装这个 hook 时认不出重启，这些步骤会一直开着，直到 transcript 有别的说法。
- 用本机 25 个真实 transcript 回放（13732 步，258 个后台步骤）：加上这两条之后，只剩 2 个开着，都是 transcript 末尾刚启动、可能还在跑的。
- 一个 transcript 超过 3000 步、删最老的一轮时，这一轮启动的、还开着的后台步骤保留（整场会话开着的开发服务器就是这种）；不显示的老轮次里的也照样列出。
- Trace 顶上列出还在后台跑的，带已跑多久。

## 3. 折叠和并行

- 一轮里同一种、已结束、没失败、下面没有子步骤的连续 3 步以上，折成一行：显示最后一步，后面写 `+N`（N 是其余的数目），时长和条从第一步的开始到最后一步的结束。
- 一组里每一步开始时间都相同（同一次回复里并行发出）：写 `×N`（N 是总数）。
- 模型调用、子 agent、正在跑的、失败的、后台的不折叠。

## 4. 分类更细，每轮一行汇总

- 在 model、read、edit、run、agent、tool 之外加：
  - `net`：WebFetch、WebSearch、web_search、fetch（原来算 read）。
  - `browser`：MCP 服务器或工具名里有 chrome、browser、playwright、puppeteer 的。
  - `skill`：Skill。
  - `memory`：MCP 工具（`mcp__…`）的名字里有 memory 的（记忆类 MCP 服务器）；Claude Code 自己叫这个名字的工具不算。
  - `hook`：transcript 里的 hook 记录（`attachment` 的 `hook_success`、`hook_additional_context`、`hook_non_blocking_error`、`hook_cancelled`），一条一步，没有时长，名字是 `hookName`（如 `PostToolUse:Write`）；加了上下文的标 `added context`；出错和取消的算失败，原因是 stderr（没有就用内容）的第一行，去掉终端颜色、最多 100 字，都没有就写 `error` / `cancelled`。没有轮次在跑时（SessionStart）不记成步骤。
- Workflow 工具算 agent。
- 每轮标题后面一行灰字汇总：`read 12 · run 4 · net 1`，按次数从多到少。
- 颜色键只列这几轮里出现过的种类，model 总是列出（每一轮都有模型调用）。

## 5. 小的

- 换模型：一次模型调用的模型和上一次的不同，这一行写 `opus-5-5 → fable-5-2`（黄色，带 `◆`）。
- 失败数：顶上的 `✗ N failed` 改为只数最近一轮的（到你下一次发消息为止），和 lot-map 一样。hook 的失败单独显示为 `hooks ✗ N`，不算进去：回放里 1506 条 hook 记录有 754 条失败（这台机器上一个 PowerShell hook 每次写文件都报非阻塞错误），算进去就把 agent 自己的失败淹没了。
- 上下文：dashboard 里的上下文百分比早就按满的程度上色（60% 起黄，80% 起红，`paint_context`）。lot-map 用 70% / 90%，只是取舍不同，保留现有的，不改。
