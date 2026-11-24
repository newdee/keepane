# keepane 快速上手

本文用例子说明 keepane 的日常用法。keepane 是一个终端复用器（tmux 的同类），
但重心是 **AI agent 的终端工作空间**：pane 里的程序在你退出后继续运行，每个
pane 有一个收件箱可以收发消息，agent 的模型、费用、token、每一步都能看见。

## 概念

```
session（会话） → window（窗口） → pane（面板）
```

- **持久化**：detach 后 pane 里的程序继续跑；重启后 `keepane resume` 恢复布局。
- **client/server 架构**：一个服务器长期运行，持有所有 session/window/pane；你
  运行的每个 `keepane` 命令是一个客户端，连上去操作或 attach。

## 安装

```powershell
# 从源码（需要 Rust 1.88+）
cargo install --path .

# 或用 Homebrew / Scoop / WinGet / 发布页二进制，见 README.md
```

## 第一个 session

```powershell
keepane new -s work        # 新建命名 session work，并 attach 进去
```

在 session 里按前缀键 **`Ctrl+b`**，再按功能键：

```text
C-b %       左右分割出第二个 pane
C-b "       上下分割出第三个 pane
C-b h/j/k/l 在 pane 之间移动（vim 键，方向键也行）
C-b c       新建窗口
C-b d       detach，回到普通终端（程序继续跑）
```

验证程序在后台继续跑：

```powershell
keepane ls                 # 列出所有 session，work 还在
keepane attach -t work     # 从另一个终端窗口重新 attach
keepane kill-server        # 停止服务器（pane 里的程序结束）
```

## 日常操作

```powershell
keepane new -d -s bg htop        # 后台（detached）session 直接跑 htop
keepane attach -t bg             # 去看它
keepane send-keys -t bg "q" Enter  # 从外面给它发按键（关掉 htop）
keepane capture-pane -p -t work  # 打印某个 pane 当前画面
```

常用前缀键：

| 键 | 作用 |
| --- | --- |
| `c` / `n` / `p` | 新建 / 下一个 / 上一个窗口 |
| `%` / `"` | 左右 / 上下分割 |
| `h j k l`（或方向键） | 移动焦点 |
| `x` / `u` | 关闭 pane / 撤销关闭（10 秒内） |
| `z` | 缩放当前 pane |
| `d` | detach |
| `[` / `]` | 进入 copy mode / 粘贴 |
| `:` | 命令提示符（`C-b :` 后输命令） |
| `?` | 列出所有按键 |
| `q` | 显示 pane 编号，按数字跳转 |
| `v` | 打开 dashboard（总览 + 操作） |
| `w` / `s` | 选窗口 / 选 session |
| `,` / `&` | 重命名 / 关闭窗口 |
| `Space` | 循环布局 |

## 消息传递 —— AI agent 的核心

### pane 地址（`-t` 后面的 target 怎么写）

一个 pane 有几种写法，都能在 `-t` 里用：

| 写法 | 含义 | 例子 |
| --- | --- | --- |
| `%N` | pane id（编号） | `-t %11` |
| `%name` | pane 名字（`rename-pane` 命名过） | `-t %builder` |
| `session` | 会话名 → 它的当前窗口、活动 pane | `-t work` |
| `session:window` | 会话的某个窗口（索引或名字） | `-t work:0` |
| `session:window.pane` | 精确到某个 pane（索引） | `-t work:0.1` |
| `$s:@w.%id` | 完整地址 | `-t $1:@0.%11` |

`keepane list-panes -t work` 列出每个 pane（一行一个，**不含名字**）：

```text
0: [80x24] %40 pwsh [D:\] (active)
```

- `0` 是 pane 索引，`%40` 是 pane id，`pwsh` 是程序，`[D:\]` 是当前路径，`(active)` 是活动 pane。

`whoami` 在某个 pane 里运行，只列出**它自己**（当前活动 pane），带上名字：

```text
$39:@41.%40   work:0.0   real   normal
```

`whoami` 的四列：

| 列 | 例子 | 含义 | 怎么定位它 |
| --- | --- | --- | --- |
| 完整地址 | `$39:@41.%40` | session id 39、window id 41、pane id 40 | `-t %40` |
| 位置 | `work:0.0` | session 名 work、窗口索引 0、pane 索引 0 | `-t work:0.0` |
| 名字 | `real` | `rename-pane` 起的名字（没起显示 `-`） | `-t %real` |
| 工作模式 | `normal` | `normal` / `shell` / `ai` | — |

几点说明：

- **完整地址 vs 位置**：`$39:@41.%40` 里的 39 / 41 / 40 是服务器分配的**唯一编号**（id），永远不变；`work:0.0` 里的 `0.0` 是**位置索引**（窗口索引 . pane 索引，都从 0 开始），pane 移动位置后会变。
- **名字只在 `whoami` 里显示**：`list-panes` 不列名字；`whoami` 列出名字，但只显示它自己（当前 pane）。
- **名字**：`real` 是 `rename-pane -t %40 real` 起的名字，之后 `-t %real` 也能指到它。

> **`%N` 最可靠**：pane id 在服务器运行期间唯一，pane 移动位置也不变；而
> `session:window.pane` 里的 pane 是「位置索引」，pane 换位置后可能指到别的 pane。

每个 pane 有一个**收件箱**，可以命名，按**工作模式**投递消息。

```powershell
# 1. 给 pane 命名，之后用 %builder 定位它
keepane rename-pane -t %3 builder

# 2. 设置工作模式：ai（收 prompt）/ shell（收命令执行）/ normal（默认，只入箱）
keepane set-work-mode -t %builder shell

# 3. 发消息（-w 秒数：等它投递完成）
keepane send-message -t %builder "cargo test"
# 输出类似： #12 delivered to $1:@2.%3 (shell)

# 4. 追踪这条消息直到完成（-w 600：最多等 600 秒）
keepane trace-message 12 -w 600
```

三种工作模式：

| 模式 | 何时投递 | 适合谁 |
| --- | --- | --- |
| `normal` | 手动 `read-message` | 默认，纯收件箱 |
| `shell` | shell 回到提示符时 | 把消息当命令执行 |
| `ai` | agent 报"空闲"时 | 把消息作为 prompt 交给 agent |

### shell 模式不生效时：用 send-keys 直接发命令

`shell` 模式靠 keepane 的 prompt hook 检测「shell 回到提示符」来投递。这个 hook 只加载到
**PowerShell（Windows）** 和 **bash / zsh（Linux/macOS）**；**WSL 的 bash 和 cmd** 不加载
（keepane 不碰 WSL 的 shell，让它保留发行版自己的配置），所以这些 pane 里 `shell` 模式的
消息会一直排队，不会自动执行（`send-message` 返回 `queued ... (shell, busy, ...)`）。

这时改用 `send-keys` 直接把命令敲进去——它绕过 prompt hook，发完立即执行，WSL / cmd 都能用：

```powershell
# 发命令（立即执行，发完就返回）
keepane send-keys -t %simu "./HFBacktest_Linux.out" Enter

# 等它打印出结果标志（-w 60 秒超时；text 换成程序实际会打印的内容）
keepane wait-output -t %simu -w 60 "done" "finished" "完成"

# 或直接看当前画面
keepane capture-pane -p -t %simu
```

- `send-keys` 没有 `-w`（发完就返回）；要「等结果」，用 `wait-output` 或 `capture-pane` 轮询。
- `wait-output`：等 pane 打印出**包含指定文本之一**的行，返回那一行。参数：`-t pane`（目标）、
  `-w 秒`（超时）、`-i`（忽略大小写）、`-a`（连已有内容一起看）、`text...`（匹配文本）。

```powershell
keepane list-messages           # 看收件箱
keepane read-message            # 读自己 pane 的收件箱
keepane drop-message 12         # 取消一条消息
keepane send-message --to %a --re 12 "收到"   # 回复消息 #12（发给它的发送者）
keepane send-message --to %b --in 30m "再看一眼"  # 定时 30 分钟后发
```

## 接入 AI agent（以 codewhale 为例）

```powershell
# 1. 安装 hook + MCP（写入 ~/.codewhale/config.toml 和 mcp.json，各文件先备份）
keepane setup codewhale --install

# 2. 在 ai 模式的 pane 里启动 codewhale（keepane 自动识别程序名，默认 ai 模式）
keepane new-session -s cw codewhale
```

codewhale 跑起来之后，keepane 自动知道它的状态：

- `session_start` / `session_idle` → 报"空闲"，keepane 投递下一条消息
- `waiting_for_user` → 报"等你"，状态行显示，`done-webhook`/桌面通知可触发
- 通过 MCP（`mcp_keepane_*` 工具）直接收发消息、建 pane、进群聊

```powershell
# 从另一个 pane 或终端，给 codewhale 派活
keepane send-message -t %cw "读一下 README 并总结"
keepane trace-message 13 -w 600    # 等它做完，看结果

# 其他 agent 同理：claude / codex / gemini / cursor / opencode / pi
keepane setup                       # 列出每个 agent 的安装状态
keepane setup claude --install      # 装 Claude Code 的 hook + MCP
```

`ai` 模式的 pane，agent 没有 hook 时消息会一直排队。keepane 会提示缺什么：

```powershell
keepane set-work-mode -t %9 ai
keepane send-message -t %9 "hi"
# 提示：%9 has not said it is free since its agent started: ...
# 缺 hook 时：keepane setup <agent> --install
```

## 看 agent 在做什么

```powershell
keepane list-agents          # 每个 pane 的 agent：model、费用、token、context、CPU/内存
keepane list-agents -J       # 同上，JSON（每个数字是数字，未知为 null）
keepane trace-pane -t %cw    # agent 每步的瀑布图（模型调用/工具/失败，有耗时）
keepane trace-pane -t %cw -J # 同上，JSON
keepane list-tasks           # 任务链（消息如何关联成任务）
keepane list-events          # 事件日志（保留 30 天）
keepane dashboard            # 或 C-b v：面板总览，可在里面发消息/改名/关 pane
```

`dashboard`（或 `C-b v`）是总览：左侧每个 pane（工作模式、空闲、收件箱、agent 的
model/费用/context），右侧选中 pane 的实时画面、事件、agent 的 Trace、git 状态。
`?` 列出全部按键。

## 群聊（多个 agent 协作）

```powershell
keepane chat new review --topic "0.73 发布"
keepane chat join review --as lead --role planner   # 在 lead 的 pane 里加入
keepane chat join review --as tester --role qa      # 另一个 pane 加入
keepane chat say review "@tester 跑一下回归"        # 说话，@ 会提醒对方
keepane chat read review --wait 60                  # 等新消息
keepane chat who review                             # 谁在线
```

## 跨机器（两台电脑配对）

```powershell
keepane web            # 本机开网页/手机端，打印二维码（同网络扫码）
keepane link add <对方的 keepane web 地址>   # 配对一次，像 ssh-copy-id
keepane link panes <host:port>             # 看对方的 pane
keepane send-message -t <host:port>/%name "hi"  # 发给对方机器的 pane
```

## 恢复与升级

```powershell
keepane resume                # 重启后恢复所有保存的 session
keepane save-session -a       # 立即保存所有 session（C-b C-s）
keepane list-saved            # 看保存了哪些
keepane restart-server        # 会话迁移到新版本
keepane update                # 升级 keepane
```

## 配置

配置文件 `~/.keepane.conf`（tmux 语法）：

```tmux
set -g prefix C-a                    # 换前缀键
bind h select-pane -L                # 自定义按键
set -g mouse on                      # 鼠标
set -g pane-border-format " #{pane_index} #{?agent,#{agent_model} #{agent_cost},} "
set -g agent-cost off                # 关掉费用抓取/显示
```

```powershell
keepane import-config ~/.tmux.conf   # 把 tmux 配置里能用的搬过来
keepane list-commands                # 全部命令
keepane man                          # 完整手册（Markdown；--roff 给 man(1)）
keepane completion powershell|bash|zsh|fish   # 生成 Tab 补全脚本
```

## 命令速查

```text
会话：  new  ls  attach  kill-session  kill-server  rename-session
窗口：  new-window  kill-window  rename-window  next-window  previous-window
面板：  split-window  kill-pane  select-pane  resize-pane  swap-pane  break-pane
消息：  rename-pane  set-work-mode  send-message  read-message  list-messages  trace-message
agent： setup  list-agents  trace-pane  create-pane  mcp
看：    dashboard  list-tasks  list-events  capture-pane  choose-tree
恢复：  resume  save-session  restore-session  list-saved
群聊：  chat new|join|say|read|who|list
跨机：  link add|list|panes|share|watch|attach  web
```

命令名可以只用无歧义前缀：`keepane att`、`keepane splitw -h`、`keepane lsp`。
