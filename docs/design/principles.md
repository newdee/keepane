# keepane 的定位与设计原则

（2026-11-19，用户提出，说"改吧"后写下。新功能先对照这一页。）

## 定位

**面向 AI Agent 的终端工作空间。** keepane 不以"功能更多的 tmux"为目标。它给 Agent 和使用它们的人提供三样东西：

| 支柱 | 管什么 | 现有的实现 |
|---|---|---|
| 工作空间（Workspace） | 程序在哪里跑、跑多久、断了能不能回来 | 持久化 session 和 pane、`resume`、`undo-kill`、`link` 两台机器 |
| 通信（Communication） | 任务怎么从一个地方到另一个地方 | 收件箱、工作模式、`send-message`、任务链、群聊、MCP |
| 可观测（Observability） | 发生了什么、正在发生什么 | pane 状态、事件日志、Agent 的模型 / 费用 / context、Trace |

## 原则：管空间和流转，不管 Agent 怎么想

keepane 管理工作空间和任务流转，不替 Agent 决定怎么思考、怎么执行。

这样 keepane 能配合不同厂商、不同模型、不同 Agent Harness，而不绑定某一个 AI 编码产品。验收这条原则的说法：**换一个 Agent 或模型，工作空间、消息和记录都不用动。**

### 不做的事（反例）

下面这些会把 keepane 变成某种 Agent 的一部分，不做：

1. 按消息内容自动决定派给哪个 Agent（路由由发送方写明，keepane 只投递）。
2. 失败后自动重试，或改写提示词再发。
3. 内置任务规划器，替 Agent 拆任务、排顺序。
4. 给某一家 Agent 专门写提示词模板或行为规则。

可以做的：投递、排队、等待"准备好"、记录、显示、提醒。Agent 通过 hook 或 MCP 主动告诉 keepane 它的状态；keepane 读它写下的记录，不改它的行为。

### 信任边界（三根支柱共同遵守）

keepane 只传递和记录；能执行什么、能看到什么，由人授权。

- pane 里的程序不能把别的 pane 切成 `shell` 模式（收到的消息会当命令执行）。
- 让别的机器进来（`link allow --full`、`link share`、`link connect`）只能由人在 pane 之外操作；`link allow` 收回（`--none`，`R` 键）也一样。pane 里的程序能做的只有减少：`link unshare`、`link disconnect`。
- 每台机器只决定自己这一边（link.md 第 21 条）：断开只管本机问不问对方，挡住对方靠收回授权。

## 各家 Agent 的支持程度（如实写）

| 层 | 内容 | 支持的 Agent |
|---|---|---|
| 通用层 | hook 报"空闲 / 等你"、收件箱投递、事件日志、消息链、MCP | Claude Code、Codex、Gemini CLI、Cursor CLI、opencode、pi（`keepane setup`） |
| 细节层 | 模型、费用、token、context、Trace | 读 transcript：目前 Claude Code、Codex、pi |

对外不写"所有 Agent 都能看到费用"。细节层按家逐个加，每加一家都是读它自己写下的记录，不改它。

## tmux 兼容的地位

tmux 兼容是"不用重新学"的入口，不是目标。按键、命令和 `.tmux.conf` 继续兼容；纯属对齐 tmux、没人用到的功能排在后面。判断标准：缺了它，tmux 用户的手会不会卡住。会卡住才做。
