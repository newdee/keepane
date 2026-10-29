# keepane

[![CI](https://github.com/newdee/keepane/actions/workflows/ci.yml/badge.svg)](https://github.com/newdee/keepane/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/newdee/keepane)](https://github.com/newdee/keepane/releases)

[English](README.md) · **[功能一览 →](https://dfine.tech/keepane/)** · [常见问题](docs/FAQ.zh-CN.md)

keepane 是一个终端多路复用器。关闭终端连接后，pane 里的程序继续运行；pane 之间还能通过收件箱传递消息，同一台电脑上可以，配过对的两台电脑之间也可以。常用的 tmux 按键、命令和配置文件可以继续使用。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-tour-light.gif">
    <img src="docs/img/keepane-tour.gif" width="900"
         alt="分四段的功能导览：名叫 lead、build、agent 的 pane，边框上写着各自的工作模式和收件箱；发给 build 的消息在那里作为命令执行，发给 agent 的消息在它的收件箱里等着。一个小的 MCP 客户端创建名叫 tests 的 pane 并给它发命令。dashboard 显示所有 pane、某个 pane 的事件和 agent 的收件箱。keepane web 打出二维码；手机上的列表显示所有 pane，在手机上输入的命令在电脑上的 build 里执行。">
  </picture>
</p>

- 脱离后，pane 里的程序继续运行。重启电脑后用 `keepane resume` 恢复布局；误关的 pane 可在 10 秒内按 `C-b u` 找回。
- 给 pane 命名后，就能向它发消息。消息先进入收件箱，等 pane 准备好再投递：shell 回到提示符时执行命令，其他程序主动读取。
- 每条消息都有固定格式的信封，记录发送方、接收方和任务。事件日志保留 30 天；按 `C-b v` 可查看 pane、消息和任务。
- 人、脚本和 AI agent 使用同一套消息机制；agent 也可通过内置的 MCP 服务端操作。`keepane setup` 可以给 Claude Code、Codex、Gemini CLI、Cursor CLI、opencode 配好 hook，每轮结束时把下一条消息交给它。
- 同一个局域网或 Tailscale 里的两台电脑，像 `ssh-copy-id` 那样配对一次，之后一台上的 pane 就能给另一台上的 pane 发消息、派命令（`keepane link`）。别的机器发来的消息默认只交给 agent，要在 shell 里执行需要在本机授权。
- 在手机上，通过 Wi-Fi 或 Tailscale，`keepane web` 可以查看所有窗口和 pane，每个是忙是闲、收件箱里排着什么；在手机上输入的命令在电脑上执行。
- 支持 tmux 风格的 `C-b` 前缀、分屏、copy mode、命令行、配置写法（`keepane import-config` 可以导入现成的 `.tmux.conf`）、格式串、hook 和插件。

支持 Windows（ConPTY），pane 里可运行 PowerShell、WSL 和 cmd；也支持 Linux 和 macOS，pane 里可运行 bash 和 zsh。

## 在 pane 之间派活

每个 pane 都有收件箱，也可以设置名字。消息会排队等待，并按接收方的工作模式投递。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-messages-light.gif">
    <img src="docs/img/keepane-messages.gif" width="880"
         alt="发给名叫 builder 的 pane 的命令在那里执行，信封写在注释里；trace-message 显示已完成和输出；分面板的 dashboard 显示 pane、一个 agent 的收件箱和任务，把一条消息置顶，右边排好版显示它的记录">
  </picture>
</p>

```powershell
keepane rename-pane -t %3 builder          # 之后用 -t %builder 就能找到它
keepane set-work-mode -t %builder shell    # 它在提示符下执行收到的内容（在 keepane 外面运行；在 pane 里只能设它自己）
keepane send-message -t %builder -w 30 "cargo test"
#12 delivered to $1:@2.%3 (shell)
keepane trace-message 12 -w 600            # 等它做完：输出、成败
```

目标可以写成 `%7`（pane 编号）、`%builder`（名字），或 `$1:@2.%7`（session、窗口、pane；`whoami` 可查看）。完整地址同时限定了 pane 所在的位置；如果它被移到别的窗口，投递会失败。

pane 根据工作模式处理消息：

| 模式 | 什么时候算准备好 | 投递方式 |
|---|---|---|
| `normal`（默认） | 不自动接收 | 用 `read-message` 读取；窗口标记显示 `@` |
| `shell` | keepane 的提示符钩子检测到 shell 回到提示符 | 输入并执行命令 |
| `ai` | agent 的轮次结束 hook 调用 `pane-ready`，或它的会话记录显示这一轮已结束（Claude Code、Codex、pi） | 作为提示词粘贴进去，再单独按回车 |

状态栏会显示当前 pane 的模式（`ai`、`shell` 醒目标出，`normal` 用暗色），`C-b q` 也会在每个 pane 的序号下面写上它的名字和模式。自己配了 `status-right` 的话，用 `#{pane_work_mode}` 加上。

用户在 pane 中输入后，keepane 会把它视为被占用，直到收到下一次就绪信号，避免消息插入正在输入的内容。keepane 投递的命令运行期间如果有人按键，结束该命令的提示符也不会触发新消息；用户自己运行命令时提前输入的内容则无法由 keepane 阻止。

pane 当前的状态用一个词表示（`#{pane_state}`，dashboard 和手机页也显示）：`idle`（空闲，可以接收下一条消息）、`busy`（程序在干活）、`typing`（输入中：有按键但还没按回车，说明有人在输入，消息会等着）、`no-agent`（`ai` 模式的 pane 回到了 shell 提示符，agent 已经退出）、`unknown`（`shell` 模式的 pane 从没发出过提示符标记，例如 cmd）、`exited`（程序已退出）；`normal` 模式的 pane 既不算空闲也不算忙（`-`）。`#{pane_idle}` 在 `idle` 时为 1。

`ai` 模式下，如果 agent 已退出并重新出现 shell 提示符，消息不会投递。投递方式以发送时接收方的模式为准：发给 agent 的文本即使遇到模式切换，也不会作为 shell 命令执行。

每条消息都带一个字段固定的信封头，写明来源和路由：

```text
[keepane id=12 task=12 from=$1:@1.%3 name=lead mode=ai to=$1:@2.%7 via=shell hop=0]
```

每个字段是 `名字=值`，用空格隔开；值里不会有空格，程序按空格切开就能读回。`set -g message-envelope json` 改成把同样的字段写成一行 JSON（`{"keepane":1,"id":12,…}`，0.17 之前的写法，shell 命令前也是）；事件日志一直用 JSON。

投给 shell 时，命令前放一个短信封头（编号和发件方：有名字用名字，否则用 pane，否则是 `user`），写成不执行任何东西、又会留在历史里的形式：PowerShell 里是注释（`<# keepane #12 from %lead #> cargo test`），bash 和 zsh 里是 `:` 的参数（`: 'keepane #12 from %lead'; cargo test`），fish 里是 `true` 的参数；其余字段用 `trace-message 12` 查看。多行内容会合为一条命令执行，对应一个结果（bash 和 zsh 里取最后一行的结果）。投给 agent 时，内容由信封头、正文和结束行 `[keepane end=12]` 组成，先粘贴进去；回车等粘贴稳定后单独发出（最早在粘贴后 300 ms，并且 150 ms 内没有新输出，最晚 2 秒）。这是因为 agent 的输入框可能吞掉紧跟在粘贴后面的回车（Codex 会这样）。回车后 2.5 秒内没有任何输出，就再发一次；仍然没有，就在 `show-messages` 里提示。这期间有人在 pane 里打字，回车就留给这个人。`task` 将派发、执行和回复关联起来；`hop` 记录消息转发次数，超过 `message-hop-limit`（默认 8）便拒收，避免 agent 循环回信。

发送方能选的字段按名字给：`--to`（目标 pane，简写 `-t`）、`--re 12`（回复第 12 条消息，不写 `--to` 就发给它的发送方；`-r` 回复本 pane 正在处理的那条）、`--task 12`（接着第 12 号任务）。其余由 keepane 填：谁发的（`from`、`name`、`mode`）、编号、hop 和 `via`。发送方改不了它们，所以信封头可信，hop 上限也照样管用：

```powershell
keepane send-message --to %builder "cargo test"
keepane send-message --re 12 "测试通过了"                # 发给第 12 条的发送方
keepane send-message --to %lead --task 12 "还有一件事"
```

在 pane 内运行 `set-work-mode`，只能修改当前 pane；从外部终端、快捷键或 `C-b :` 命令行运行时，可以修改任意 pane。这样，pane 内的程序不能直接把别的 pane 切成自动执行消息的 `shell` 模式。改名、管理收件箱和关闭 pane 不受这条限制；关闭操作可在 10 秒内用 `C-b u` 撤销。此规则用于减少误操作，并非针对同一用户进程的安全隔离。

## dashboard

按 `C-b v`（弹窗）或运行 `keepane dashboard`（任意终端），像 lazygit 那样分面板显示。左边：`[1]` 所有 pane，按 session 分组，列出工作模式、空闲与否、收件箱、安静了多久、在跑的程序（跑 agent 的写 agent 的花费和上下文，标题里是总花费）。每列上方写着列名；颜色各表达一个意思（空闲绿、忙黄、有消息排队蓝、上下文越满越红）；窗口窄时，最不要紧的列先让位；`[2]` 所选 pane 的收件箱；`[3]` 任务。右边 `[0]` 是所选 pane：地址、目录、pid、跑了多久、它说自己在做什么，下面是它此刻的屏幕（带颜色）、回滚、事件，或光标所在的消息、任务：在 `[2]`、`[3]` 里移到哪一条，右边立刻排好版显示它（状态用颜色标出，谁发给谁，每一步的时间，然后是正文和执行输出；任务则把每条消息按时间连成一列）。Tab、`1 2 3 0`、`h`/`l` 换面板，`j`/`k` 在面板里移动，`g`/`G`（或 Home/End）跳到第一行 / 最后一行（右边是到顶 / 到底），`[`/`]` 切换右边的内容；鼠标可以点选和滚动；`?` 列出所有按键。

也能直接操作。对 pane：`s` 发消息、`r` 改名、`m` 改工作模式、`R` 标记就绪（解卡）、`o` 跳过去（顺带关掉弹窗）、`x` 关掉。对收件箱：`d` 删除排队消息（`u` 撤销）、`K`/`J` 上下移、`t` 放到最前、Enter 进到右边滚动阅读。关 pane、删消息、把 pane 切到 `shell`（从此收到的文字会被当命令执行）这三样会先确认。设计见 [docs/design/dashboard.md](docs/design/dashboard.md)。

### agent 用了多少、花了多少

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-agents-light.gif">
    <img src="docs/img/keepane-agents.gif" width="880"
         alt="三个 pane 里三个 agent 在干活，每块的边框上写着模型、已花费用和上下文占比；dashboard 把它们列在一起，标题里是总花费；keepane list-agents 给脚本打印同样的内容">
  </picture>
</p>

（录屏里的 agent 是替身，按 Claude Code 和 Codex 的格式写会话记录；keepane 读真的 agent 也是一样。）

keepane 认得 pane 里跑的 agent：Claude Code、Codex、pi 和 Cursor CLI（看程序名，或看 `node` 跑的是什么；Cursor 的 `agent` 只认 Cursor 自己目录里的那个）。keepane 不读 Cursor 的对话，所以 Cursor 只有 CPU 和内存。其余每个 agent 都会为它的工作目录写一份会话记录（`~/.claude/projects`、`~/.codex/sessions`、`~/.pi/agent/sessions`；也认 server 启动时的 `CLAUDE_CONFIG_DIR`、`CODEX_HOME`）。keepane 每 2 秒读一次，只读上次之后新写的部分，从中得到模型、用掉的 token、调用工具的次数、回复的次数、上下文占了多少；再从进程表读 agent 及其子进程的 CPU 和内存。Claude Code 的 subagent 算在它的 pane 上。

花费：pi 自己记了花费，直接用。其余的按 [LiteLLM 的公开价目表](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json) 给每个模型的 token（新输入、读缓存、写缓存、输出）分别计价。价目表用 `curl` 下载，一天一次（失败了 1 小时后再试），只在有 pane 跑 agent 时才下载，存在数据目录里。表上是各家的 API 标价：订阅（Claude Max、ChatGPT Pro）另有计费，所以这个数是“同样的活走 API 要花多少”。价目表里没有的模型不计价，这时花费后面带 `+`（`$2.10+`：至少这么多）。`set -g agent-cost off` 不下载，也不显示花费；环境变量 `KEEPANE_PRICE_LIST` 可以指定自己的价目表（LiteLLM 的格式）。

在哪里看：dashboard 的 pane 列表（`claude $2.10 34%`：花费、上下文），所选 pane 上方一行列出全部数字；手机网页上 agent pane 的卡片多一行；`keepane list-agents`（`-J` 输出 JSON，每个数都是数字，不知道的是 `null`）；还有格式变量 `agent` `agent_model` `agent_cost` `agent_tokens` `agent_context` `agent_cpu` `agent_mem` `agent_tools` `agent_turns`，可以放进边框或状态栏：

```tmux
set -g pane-border-status top
set -g pane-border-format " #{pane_index} #{?agent,#{agent_model} #{agent_cost} ctx #{agent_context},#{pane_current_command}} "
```

### agent 正在一步步做什么

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/trace-light.png">
    <img src="docs/img/trace.png" width="880"
         alt="The dashboard's Trace tab: an agent's turn as a waterfall, each model call and tool a bar where it ran, coloured by kind, the failed test run red">
  </picture>
</p>

同样这些 transcript 也记下了 agent 每一步做了什么、什么时候做的：每一轮（从你的提问到它最后一次回复），以及这一轮里每次调用模型、每次运行工具的起止时间，和有没有失败。dashboard 右边切到 Trace 标签页（按 `T`，或用 `]` 切过去），会把所选 pane 的 agent 画成瀑布图，每 2 秒刷新：

```
claude  claude-opus-5-5  $2.50  41k tokens  model calls 3  tools 4  ✗ 1 failed
▸ add the share command                                            5.0s
▾ fix the test running                                            1m40s
  opus-5-5 1.2k tokens             10s  █████
  Read …\src\share.rs              10s       █████
  $ ✗ cargo test                   30s            ███████████████
  Task find it                     10s                           █████
    Glob **/*.rs                   1.0s                           █
  opus-5-5 thinking                40s                                ████▶
```

每种步骤一种颜色：模型品红，读取和搜索青色，改文件黄色，运行命令蓝色，子 agent 绿色，其他工具不着色，失败的红色并标 `✗`；还在进行的加粗，末尾有 `▶`。模型调用显示是哪个模型，命令跟在 `$` 后面按原样显示，文件显示路径末尾。每一轮用你的提问命名；不是你提问开始的，用开始它的东西命名：后台任务完成（它的摘要）、`!` 命令、定时任务、别的会话发来的消息。Claude 子 agent 的步骤排在启动它的那次调用下面。默认展开最新的一轮，之前的每轮只显示一行（按 `e` 全部展开）；一轮结束后，显示 agent 已经等你多久了。标签页停在底部，新的步骤来了也一直显示最新一步（`g` 到顶部，`G` 回到底部）。transcript 是一件事做完才写一行，所以模型调用要等回复写下来才显示；一轮还在进行、又没有工具在跑时，就是模型从上一步起一直在工作（`thinking`）。`keepane trace-pane -t <pane>` 把同样的内容打印成树，`-J` 输出 JSON（每一步的类型、名字、参数摘要、起止毫秒数、父步骤）。

只读 keepane pane 里的 agent。但如果别处（另一个终端、编辑器）的 agent 同时在同一个目录里工作，它的记录写在同一个文件夹，会被算进这个 pane。

消息及 pane 状态变化会写入事件日志：`%LOCALAPPDATA%\keepane\events\<socket>\2026-09-26.jsonl`，保留 30 天（`event-log`、`event-log-days`、`event-log-max`）。`list-tasks`、`show-task`、`trace-message`、`list-events` 读的就是它。服务端停止时，未投递的消息会被丢弃，并留下日志记录。pane 名字和工作模式随 session 保存。设计细节见 [docs/design/mailbox.md](docs/design/mailbox.md)。

## 安装

keepane 的文件（日志、事件日志、会话存档、pane 历史）放在它的数据目录：Windows 上是 `%LOCALAPPDATA%\keepane`，Linux 上是 `~/.local/share/keepane`，macOS 上是 `~/Library/Application Support/keepane`。下文的路径按 Windows 写。

### Linux 和 macOS

用 [Homebrew](https://brew.sh) 安装（macOS 的 Apple 芯片和 Intel、Linux x86_64）：

```bash
brew install newdee/tap/keepane
```

以后用 `brew upgrade keepane` 升级。也可以从 [Releases](https://github.com/newdee/keepane/releases) 下载 `keepane-v<版本>-linux-x86_64.tar.gz`（静态编译，任何 x86_64 Linux 都能跑）、`keepane-v<版本>-macos-aarch64.tar.gz`（Apple 芯片）或 `keepane-v<版本>-macos-x86_64.tar.gz`（Intel），解压后把 `keepane` 放进 `PATH`：

```bash
tar xzf keepane-v<版本>-linux-x86_64.tar.gz
install keepane-v<版本>-linux-x86_64/keepane ~/.local/bin/
```

也可以从源码编译（见下）。server 监听的 socket 在 `$XDG_RUNTIME_DIR/keepane-<uid>/`（没有这个变量就用 `/tmp/keepane-<uid>/`），这个目录只有你自己能进。`keepane startup on` 让 server 在登录时启动（macOS 上是 launchd agent，Linux 上是 systemd 用户 unit，见下文）。`keepane update` 和 Windows Terminal 配置是 Windows 上的功能：在 Linux 和 macOS 上，升级按当初的安装方式来（`brew upgrade keepane`，或从 Releases 下载；`keepane update` 会告诉你用哪种）。

### Windows

需要 Windows 10 1809 或更新版本（支持 ConPTY）。从 [Releases](https://github.com/newdee/keepane/releases) 下载：

- `keepane-v<版本>-windows-x86_64.zip`：里面是一个目录 `keepane-v<版本>-windows-x86_64`，`keepane.exe` 在这个目录里。解压后将该目录加入 `PATH`。不需要管理员权限。
- `keepane-<版本>-windows-x86_64.msi`：安装到 `Program Files`，供所有用户使用，并加入系统 `PATH`；可在“应用和功能”中卸载。需要管理员权限（静默安装：`msiexec /i keepane-<版本>-windows-x86_64.msi /qn`）。
- `keepane-<版本>-windows-x86_64-user.msi`：只为当前用户安装，装到 `%LOCALAPPDATA%\Programs\keepane`，加入用户 `PATH`。安装和 `keepane update` 都不需要管理员权限，通过 SSH 也能装、能升级。

Scoop 可直接使用仓库中的清单安装 zip，无需管理员权限：

```powershell
scoop install https://raw.githubusercontent.com/newdee/keepane/master/packaging/scoop/keepane.json
```

通过 SSH 使用时，Windows 11 不允许 SSH 会话穿过普通用户创建的 junction，而 Scoop 的 `current` 目录正是一个 junction，shim 会报"无法遍历该路径，因为它包含不受信任的装入点"。让 Scoop 的 shim 直接指向版本目录：

```powershell
scoop config no_junction true
scoop reset keepane
```

Scoop 只在一个应用的程序都没在运行时才更新它（否则提示"Running process detected, skip updating."），而 keepane 的 server 一直在运行，所以直接 `scoop update keepane` 什么也不会变。`keepane update` 会为它让路：一个放在 Scoop 目录外的 keepane 副本保存会话、停掉 server（pane 里的程序会重新启动，和 `restart-server` 一样），运行 Scoop，再带着这些会话启动 server；做了什么写在 `server.log` 旁边的 `update.log` 里。

通过 SSH 时，按机器安装的 MSI 需要在桌面上确认权限，没人能点；`keepane update` 遇到这种情况会直接说明，请改用按用户安装的 MSI、zip 或 Scoop。

WinGet 的清单（装 MSI）在 `packaging/winget/`，`winget validate` 通过；合进 winget-pkgs 之后 `winget install newdee.keepane` 就行，在那之前可以在克隆里 `winget install --manifest packaging/winget/manifests/n/newdee/keepane/<版本>`。细节见 `packaging/README.md`。

### 从源码编译

从源码编译需要 Rust 1.88 或更新版本：

```bash
cargo install --git https://github.com/newdee/keepane --locked   # 直接装最新的 master
cargo install --path .                                         # 本地克隆
```

构建 Windows 的 MSI 时，如果本机没有 WiX，脚本会临时下载：

```powershell
cargo build --release
pwsh -File installer/build-msi.ps1        # 产物在 target\keepane-<版本>-windows-x86_64.msi
```

## 日常用法

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-demo-light.gif">
    <img src="docs/img/keepane-demo.gif" width="880"
         alt="把一个 shell 切成几块、用 set sync 一次输入到所有 pane、用 h/j/k/l 移动、全屏、pane 菜单、给所有 pane 设变量并用 C-b e 查看、窗口选择器、脱离后再接回来">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-alerts-light.gif">
    <img src="docs/img/keepane-alerts.gif" width="880"
         alt="部署在没人看的窗口里跑完，状态栏出现 # 标记，prefix M-n 跳过去，失败的命令把 pane 和退出码留在原地，弹窗里显示窗口列表">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-chart-light.gif">
    <img src="docs/img/keepane-chart.gif" width="880"
         alt="C-b w 打开结构图：最上面是 keepane，下面依次是 session、窗口和 pane，每个都是空心框；h j k l 移动，a 一次展开全部，v 切到树状视图和纯文字列表">
  </picture>
</p>

Windows 上，keepane 以 Windows 原生的 win32-input-mode 转发键盘事件，支持 PSReadLine 组合键、`Ctrl+Space`、`Shift+Enter`、带修饰键的方向键、中文输入法，以及 WSL 中的 vim 和 htop。可在 Windows Terminal、传统控制台、VS Code 终端和其他 Windows 控制台宿主中运行。Linux 和 macOS 上，它读取终端发来的按键序列（带修饰键的 xterm 按键、括号粘贴、SGR 鼠标），再像终端一样转给程序，所以任何兼容 xterm 的终端和 SSH 里都能用。

```bash
keepane                      # 进最近用的 session；没有在跑的就恢复存档；都没有才新开
keepane new                  # 总是新开一个 session
keepane new -s work          # 起个名字
keepane new -d -s bg htop    # 后台开一个跑 htop 的 session
keepane ls                   # 看看有哪些 session
keepane attach -t work       # 接回去（换一个终端窗口也行）
keepane send-keys -t work "git status" Enter
keepane capture-pane -p -t work   # 把 pane 上的文字打印出来（-S -200 连带回滚）
keepane kill-server
```

和 tmux 一样，命令名可以使用无歧义的前缀：`keepane att`、`keepane lsp`、`keepane splitw -h`。`keepane kill` 会被拒绝，因为有四个命令以它开头。`keepane list-commands` 列出全部；和 tmux 的逐条对照在 [docs/tmux-parity.md](docs/tmux-parity.md)，命令和按键都有。

批量创建 pane：`keepane split-window -N 3` 会再开三个并把窗口平铺（加 `-d` 焦点留在原处）。窗口太小放不下时，放得下的那些会留着，并告诉你开了几个。

进入 session 后，先按前缀键 `Ctrl+b`，再按下表中的键：

| 按键 | 作用 |
| --- | --- |
| `c` / `n` / `p` / `Tab` / `0-9` | 新窗口 / 下一个 / 上一个 / 刚才那个 / 按编号跳（超过 9 个窗口用 `'` 输入编号） |
| `,` / `&` | 重命名 / 关掉当前窗口 |
| `%` / `"` | 左右分 / 上下分 |
| `h` `j` `k` `l`（或方向键）/ `o` / `;` | 在 pane 之间移动（vim 键位）/ 下一个 pane / 刚才那个 pane |
| `H` `J` `K` `L`、`Alt`+方向键 / `Ctrl`+方向键 | 调整当前 pane 大小，每次 5 格 / 1 格 |
| `Shift`+方向键 | 窗口比这个终端大时（`window-size` 听了别的客户端），平移自己的视口，每次 5 行 / 10 列；敲键时视口自动跟着光标 |
| 移动、改大小、`n` / `p`、`{` / `}` 都能连按 | 按一次前缀之后半秒内（`repeat-time`）接着按同一个键就行，不用再按前缀 |
| `z` | 当前 pane 放大到整个窗口，再按一次还原；放大时切到同窗口的别的 pane（`h` `j` `k` `l`、`q` 加数字、`;`），放大会跟过去，直到再按 `z`（`set -g keep-zoom off` 则切换即还原，和 tmux 一样） |
| `x` | 关掉当前 pane |
| `u` | 把 10 秒内关掉的 pane 或窗口放回原处（`undo-kill`） |
| `C-t` | 在每条命令那一行的末尾显示开始时间、耗时和成败（`pane-timestamps`） |
| `/` | 按 pane 和日期翻看输出过的历史（`choose-history`） |
| `y` | 把上一条命令的输出（不含提示符和命令本身）复制到粘贴缓冲区和剪贴板（`copy-output`） |
| `Y` | 从所有 pane 跑过的命令里挑一条（最新的在前）：跳过去、复制它的输出、再跑一次（`choose-marks`） |
| `@` | 挑一台 `ssh` 去过的机器：跳到连着它的 pane，或在新窗口、分屏里连接（`choose-ssh`，见 SSH 机器） |
| `F` | 给屏幕上的路径、网址和 git hash 都标上字母；按字母复制，按大写字母打开（`hints`，见下文） |
| `e` | 用彩色显示每个 pane 拿到的环境变量：`set-environment` 设的，和 keepane 自己设的（`show-environment`，见"环境变量"） |
| `{` / `}` | 和前一个 / 后一个 pane 交换位置 |
| `q` | 每块显示自己的编号（下面显示名字和工作模式，如 `%名字 · ai`），按数字立刻跳过去（超过 9 个时，紧接着再按一位就接着跳：`1` `2` 是第 12 块） |
| `Space` / `M-1`…`M-5` / `E` | 轮换布局 / 直接选一种（左右平分、上下平分、主窗在上、主窗在左、平铺）/ 把旁边这一排 pane 拉成等宽等高 |
| `C-o` / `M-o` | 让所有 pane 在布局里轮转一格 |
| `!` | 把当前 pane 拆成一个独立窗口 |
| `m` / `M` | 标记这个 pane / 取消标记（`join-pane` 默认搬走被标记的那个） |
| `T` / `f` | 给这个 pane 起名 / 按名字或标题找窗口 |
| `#` / `-` / `=` | 列出粘贴缓冲区 / 删掉最新的 / 挑一个粘贴 |
| `t` / `~` / `r` | 时钟，下面写着日期（和 UTC 时间）、其他地方的时间（`clock-mode-zones`）、这个 pane（程序、目录、分支、跑了多久、安静了多久）、它的上一条命令（花了多久、结果；目录或命令太长时用 `…` 缩短，保证后面的内容显示完整）、它的 agent（模型、花费、上下文）和这台机器，放得下时再显示当月月历并标出今天（`clock-mode-info off` 只显示时间；`clock-mode-style 12`、`clock-mode-colour`）；再往下是随机器负载变化的动画（见下文）；按 `q` 退出 / 最近的提示消息 / 重画 |
| `S` | 开关 `synchronize-panes`：敲的东西同时进这个窗口的所有 pane，状态栏会多个 `S` |
| `[` / `PgUp` | copy mode（下面单独说） |
| `]` | 粘贴剪贴板 |
| `:` | 命令行（`:split-window -h -c C:\src`、`:set mouse off` 之类；Tab 补命令名、flag、`-t` 后的目标和选项名；↑ / ↓ 或 `C-p` / `C-n` 翻出之前在这里执行过的命令，保留最近 100 条，server 停止后清空；`show-prompt-history` 列出它们，`clear-prompt-history` 清空它们） |
| `C-s` / `C-r` | 保存所有 session（各存各的文件）/ 恢复保存过的 session |
| `d` | 脱离 |
| `?` | 列出所有按键。这类输出（还有 `:show-messages`、`:show-prompt-history`）比窗口长时，和 tmux 的 view mode 一样先显示末尾，可以滚动：↑ ↓ 或 `j` `k`、PgUp PgDn、`g` `G`、鼠标滚轮；`/` `?` 在里面搜索（找到的词反色显示），`n` `N` 接着找；按其他键关闭 |
| （停一下） | 按了前缀，半秒内没按下一个键，就弹出按键提示面板，按组列出每个键做什么（pane、窗口、session、工具，以及你自己绑定的键）；接着按的键照常生效（`prefix-hint`、`prefix-hint-delay`） |
| `s` / `w` | 挑一个 session / 窗口，`w` 一直展开到每个 pane。默认是结构图：第一排居中是 `keepane`，第二排是所有 session，第三排是当前所在 session 的窗口，第四排是那个窗口的 pane，每排横向铺开（一排比屏幕宽时左右滚动，两端显示 `‹` `›`），每个节点是一个空心框，框按类型着色（session 蓝、窗口紫、pane 绿，选中的框黄；名字用正文色，其余淡色），框的上边写着它是什么（`host`、`session`、`window`、`pane`）；`h` `l` 或 ← → 在同一排移动，下面几排跟着变，`k` 或 ↑ 到父节点，`j` 或 ↓ 到当前窗口或活动 pane。`a` 一次展开所有节点，每个父节点在自己的子节点上方；放不下时框先变窄，再变成每个节点一行字，再只剩编号，还是太宽就左右滚动，跟着选中的节点（再按 `a` 回到只展开选中分支）。和别的机器配对过（`keepane link`，见下文）时，`keepane` 下面多一排机器，本机在最前：在这一排用 ← → 选到别的机器，下面就是它的 session、窗口和 pane（chart 开着时每 5 秒问一次；对方不回答时显示 `offline`），在它的 pane 上按 Enter 弹窗显示那个 pane 的屏幕（需要对方执行过 `link allow <本机> --screen`）。`v` 切到方块树（一行一个，↑ ↓ 跳到父节点、第一个子节点，← → 在同一层移动，`-` / `+` 折叠、展开 session 或窗口），再按切到纯文字列表（`j` `k` 或方向键上下，`-` / `+` 或左右方向键折叠），再按回到结构图。树和列表里 `j` `k` 逐行移动；三种都是`g` `G` 到头到尾，数字直接跳（能选的行都有编号，超过 9 连按两位，`1` `2` 是第 12 行），`Enter` 跳过去，`q` 取消；`f` 输入子串过滤（边打边筛，`Enter` 留下，`Esc` 还原），`t` 给当前行打标记（`T` 清掉），`x` 杀掉标记的行（没标记就是当前行）；`D`（或 Delete 键）杀掉的同时删掉 session 的保存文件，`resume` 不会再带回来。对 session 按这两个键都会先问一句 (y/n)。`set -g choose-tree-style tree`（或 `list`）改成默认打开方块树（或列表） |
| `(` / `)` | 切到上一个 / 下一个 session |
| `D` | 列出连着的客户端，挑一个踢下线 |
| `>` / `<` | pane 菜单 / 窗口菜单（括号里的字母直接执行，`Enter` 执行选中那条） |
| `M-n` / `M-p` | 跳到下一个 / 上一个有提醒的窗口（见 `monitor-activity`） |

pane 里再跑一个 keepane（比如 ssh 到服务器上跑的）时，它的前缀也是 `C-b`，而外面这个 keepane 先收到。`C-b C-b` 把一个 `C-b` 发进去，所以 `C-b C-b c` 是在服务器上开新窗口。要在里面连续操作，按 `F12`：这个 pane 收到所有键，前缀和 `bind -n` 绑的键也给它，状态栏显示 `passthrough`。再按 `F12` 把前缀收回来。这个开关属于这个 pane：别的 pane 照常用前缀，鼠标点别的 pane 也照常。脚本里用 `passthrough [-t pane] [on|off]`；想换键就 `bind -n <键> passthrough`（只有绑到它的键不会被放过去）。

开着中文、日文、韩文输入法时，按出来的全角字符（`[` 出来 `【`，`:` 出来 `：`，`y` 出来 `ｙ`）照样当作那个键：前缀之后、copy mode、选择列表和菜单、y/n 确认、dashboard 里都是。打进 pane 的字，以及提示框里输入的文字（比如窗口名）保持原样；如果你把全角字符本身绑成了键，就按你的绑定来。

pane 放大、还原及焦点切换带有动画，默认持续 160 毫秒。程序仅按动画结束后的尺寸调整，不必等待动画完成。用 `set -g animation off` 关闭动画，或通过 `animation-time` 调整时长（毫秒）。

时钟（prefix t）下方剩余的行里有一段随机器负载和 keepane 自身活动变化的动画（`clock-mode-animation`）：

- `pulse`：监护仪。心跳频率随 CPU 变化，核心空闲时每分钟 60 下，全部满载时 150 下；keepane 每送达 1 条消息，心脏提前跳 1 下（最多积攒 4 下）。网络波形的高度随流量变化；磁盘波形像呼吸，磁盘越忙，呼吸越快越深；系统波形随上下文切换变快，随中断变高。每条波形旁边写着对应数字（`♥ 92  CPU 35% 67W  61°C  ✉ 2`、`NET ↓1.2M ↑80K  3.1k pkt/s`、`DISK r12M w0`、`SYS 48k/s  312 proc 4.3k thr`）。`67W` 是 CPU 封装功耗；`PWR 20W` 是机器用电池运行时电池的输出功率；`✉ 2` 是这次打开时钟以来送达的消息数。
- `matrix`：字符雨。CPU 越忙下落越快，内存占用越高下落的列越多，有网络流量时出现青色的列，磁盘越忙字符闪烁越快，中断越多散落的品红色字符越多；每送达 1 条消息，落下 1 列以 `✉` 开头的白色字符。
- `bars`：均衡器。每个核心一根柱，然后是内存、磁盘（读、写）、网络（收、发）、温度，以及窗口里每个 pane 各一根的输出量柱；放得下时再加上消息（来一条打满，然后回落）、CPU 功耗、电池功耗、交换区，以及上下文切换和中断。每组下面有标签，峰值标记缓慢下落。
- `fire`：火焰。每个核心一簇火苗，核心越忙火苗越高；CPU 越热，火尖从黄色变为亮黄色再变为白色；中断越多火星越多；每送达 1 条消息，有 1 个 `✉` 从火里升起。
- `starfield`：星空穿梭。进程越多星星越多；网络流量越大飞得越快（高速时拖出光线）；每送达 1 条消息，飞过 1 颗以 `✉` 为头的彗星。
- `sharks`：俯视的海。窗口里每个 pane 一群鱼（最多 6 群），pane 输出越多游得越快，上下文切换越多转向越多；一条鲨鱼在追鱼，CPU 越忙饿得越快，内存占用越高鲨鱼越长；网络流量越大，水流把鱼和鲨鱼推得越远；磁盘越忙水面闪光越多；每送达 1 条消息，水面荡开 1 圈波纹，中间是 `✉`，鲨鱼冲过去。
- `bonsai`：盆景。树在一天里长大（午夜后是小树苗，早上长出大半，晚上长全）；窗口里每个 pane 一根主枝（最多 6 根），叶子一直轻轻晃动（上下文切换越多晃得越快），pane 输出越多那根枝的叶子抖得越厉害；内存占用越高叶子越多，网络流量越大叶子被风吹得越斜；不时有叶子飘落，磁盘越忙落叶越多；每送达 1 条消息，有 1 个 `✉` 从叶子上飘落。
- `sea`：月光下的海。月亮随时间划过天空（18:00 从左边升起，午夜最高，06:00 在右边落下；白天颜色变淡）；夜里有星星，内存占用越高星星越多，上下文切换越多闪得越快；CPU 越忙浪越大、越快；网络流量越大，水面上的月光越闪；窗口里每个 pane 一条船，pane 输出越多船开得越快；每送达 1 条消息，有 1 个 `✉` 漂向岸边。
- `cat`：睡着的猫。CPU 越忙呼吸越快，每呼吸一次飘起 1 个 `z`；网络流量越大尾巴摆得越多，磁盘越忙耳朵抖得越多；一个毛线球滚过，窗口里的 pane 输出越多滚得越快；每送达 1 条消息，有 1 个 `✉` 落在猫旁边，猫醒来 3 秒。
- `dino`：小恐龙跑过沙漠。CPU 越忙跑得越快（右上角的分数就是跑过的距离），遇到仙人掌就跳过去；窗口里 pane 越多仙人掌越多；网络流量越大云飘得越快；每送达 1 条消息，有 1 个 `✉` 落到它头上，它开心地冒出一颗心。
- `penguins`：冰面上的企鹅，远处是海。窗口里每个 pane 一只企鹅，在自己那段冰面上摇摇摆摆地来回走，pane 输出多时趴下来滑；CPU 越忙海浪越快，网络流量越大雪越大，内存占用越高极光越亮；每送达 1 条消息，有 1 条鱼从海里跃上冰面，上面带着 `✉`。
- `synthwave`：霓虹网格向你奔来，CPU 越忙越快；地平线上是落下的条纹太阳（pane 输出越多条纹滑得越快），两边的山随内存占用变高，星星随网络流量闪烁；每送达 1 条消息，有 1 颗以 `✉` 为头的流星划过天空。
- `fireworks`：夜晚的城市上空放烟花。窗口里的 pane 输出越多升起的烟花越多，炸开成一圈圈下落、渐暗的火花，CPU 越忙炸得越大；内存占用越高城里亮着的窗户越多；每送达 1 条消息，升起 1 枚两倍大的烟花，炸开的地方显示 `✉`。

时钟下方信息行的最后一行是 keepane 自己的情况：会话数、pane 数，以及启动以来送达的消息数。

`pulse`、`matrix`、`starfield` 的颜色，以及鲨鱼的眼睛和盆景的叶子，随 CPU 温度从绿色变为黄色再变为红色，读不到温度时随 CPU 负载变化；`bars` 的柱子下段绿色、中段黄色、顶段红色。`random`（默认）每次在 pane 上打开时钟时从 13 种里随机选 1 种，不会和这个 pane 上一次的相同；`cycle` 轮流使用；写名字就固定用那一种；`off` 或 `animation off` 时钟就不动。`aquarium`、`life`、`donut`、`weather`、`hourglass`、`orbits`、`radar` 在 0.46 去掉了，配置里还写着它们的，按 `random` 处理。时钟显示时按 `/` 换下一种、按 `?` 换上一种，动画名在右上角显示 1 秒；只管这一次显示，不改选项（`clock-mode-animation off` 时按 `/` 会开始动画，`animation off` 时不会）。时钟只在按 `q` 时退出（tmux 是按任意键退出）；时钟显示时前缀键照常可用。动画只用主题的 16 种颜色（`pane-colours`；主题没设时用终端自己的颜色），所以每个主题有各自的配色。只在时钟显示时读取机器数据，每秒读 2 次。系统给不出的数字就不显示，不会编造：macOS 没有中断和功耗数字，上下文切换和线程数只算你能查看的进程，不是整个系统的；温度只来自真实的 CPU 传感器（Linux：thermal zone 和 hwmon；Windows：运行中的 LibreHardwareMonitor 或 OpenHardwareMonitor，因为 Windows 自带的 ACPI thermal zone 在很多机器上是个固定值；macOS：没有）。CPU 功耗来自 CPU 自带的功耗计（RAPL）：Windows 上有这个功耗计的 CPU 任何用户都能读；Linux 大多数系统只允许 root 读（`/sys/class/powercap`），所以只在能读时显示。

`clock-mode-look` 让任何一种动画改用一套字符来画：`dots`（`·` `•` `●`）、`blocks`（`░` `▒` `▓` `█`）或 `binary`（`1` `0`）。每个格子换成墨量最接近的字符；墨量看原来的字符占了格子多少、颜色有多亮。颜色不变，数字、名字和消息的 `✉` 也不变。`plain`（默认）用动画自己的字符。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-clock-light.gif">
    <img src="docs/img/keepane-clock.gif" width="880"
         alt="一个正在编译的 pane 上打开七次时钟：心跳随 CPU 加快、放慢的监护仪，字符雨，每个核心、内存、磁盘、网络和 pane 输出的柱状图，月光下的海（随后改用 dots 字符），以及被消息叫醒的睡猫，奔跑的小恐龙和落日下的霓虹网格">
  </picture>
</p>

<p align="center">
  <img src="docs/img/clock-pulse.png" width="436" alt="监护仪：心跳、网络、磁盘、系统各一条波形，旁边写着对应数字">
  <img src="docs/img/clock-bars.png" width="436" alt="柱状图：每个核心、内存、磁盘、网络、温度和每个 pane 的输出各一组，带标签">
</p>

copy mode 的常用操作：

- 移动：`h` `j` `k` `l` 或方向键；`w` `b` `e` 按词移动；`0` `^` `$`、`H` `M` `L`、`{` `}`、`g` `G` 跳转；`[` `]` 跳到上一条 / 下一条命令输入的位置。`0` 和 `$` 跳到这一行的开头和结尾；一行太长折成几行显示时，也跳到整行的开头和结尾。`$` 停在最后一个字符上，不停在后面的空白处。`f` 加一个字符，跳到本行下一个该字符；`F` 往回找；`t` `T` 停在它旁边一格；`;` 按同一方向再跳一次，`,` 按反方向。
- 翻页：`PageUp` / `PageDown` 或 `C-b` / `C-f`；`C-u` / `C-d` 翻半页。由于 `C-b` 也是前缀键，在 copy mode 中按 `C-b C-b` 可上翻一页。数字可指定重复次数，例如 `3j`。
- 选择与复制：`Space` 或 `v` 开始选择，`V` 按整行选择（再按 `V` 取消），`C-v` 切换矩形选择，`Enter` 或 `y` 复制到粘贴缓冲区和 Windows 剪贴板。
- 搜索：`/` 开始搜索、`?` 反向搜索，`n` / `N` 跳到下一个结果；在 `/` 提示符里按 ↑ / ↓ 翻出之前搜过的词，保留最近 100 条；`q` 退出。

脚本可用 `send-keys -X <命令名>` 执行对应操作，命令名与 tmux 相同。

鼠标可用于选择 pane、拖动边框调整大小，以及点击状态栏切换窗口。滚轮在普通界面上会进 copy mode 往回翻，在全屏程序里变成方向键，程序自己要鼠标事件的话就原样转过去；脚本里用 `send-keys WheelUp`（或 `WheelDown`）给程序发同样的滚轮输入（普通界面上什么也不发，那里滚轮翻的是 keepane 自己的历史）。拖选一段文字，松手就复制到 Windows 剪贴板了（在哪里松手都算）；右键把剪贴板贴进 pane，和终端本身的右键一样。拖到 pane 的最上一行或最下一行（或更外面），pane 会一直滚动，所以能选中超过一屏的内容。选区按列复制：两列宽的汉字整个复制。

`C-b F`（`hints`）给屏幕上各个 pane 里的路径、网址和 git hash 都标上一两个字母：编译器给的 `src/main.rs:12:5` 或 `App.cs(12,5)`、`https://…`、`af9af7e`。按标签字母就把它复制到粘贴缓冲区和剪贴板（路径不带行号）；按大写字母就打开：网址用浏览器开；路径在 PATH 里有 `code` 时用 VS Code 跳到那一行，没有就在新窗口里用 `$VISUAL` 或 `$EDITOR` 打开（`vim +12 src/main.rs`），再没有就交给系统默认程序。相对路径按那个 pane 的当前目录算。按 Esc 收起标签。没有斜杠也没有行号的单个名字（`notes.txt`）只在文件确实存在时才算，所以普通文字不会被标上；被终端折到下一行的内容找不到。

`set -g hint-open '<命令>'` 可以换成自己的打开方式：一条 keepane 命令，`{file}`、`{line}`、`{col}` 原样填进去，路径可能带空格的话用单引号括起来：

```tmux
set -g hint-open "new-window hx '{file}:{line}:{col}'"
set -g hint-open "run-shell 'idea --line {line} {file}'"
```

## 命令时间和历史

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-history-light.gif">
    <img src="docs/img/keepane-history.gif" width="880"
         alt="每条命令行尾显示时间，其中一条失败；C-b Y 列出各 pane 跑过的命令；历史面板按 pane 位置和日期列出；在查看器里打开某一天；按 / 搜索所有天；手滑关掉的 pane 按 C-b u 找回">
  </picture>
</p>

PowerShell、bash、zsh 的 pane 会通过 keepane 启动时装上的提示符钩子报告每条命令的运行情况（钩子在你自己的 `~/.bashrc` 或 `.zshrc` 之后加载，不改你的提示符）。macOS 自带的 `/bin/bash`（3.2）不报告命令何时开始（`PS0` 是 4.4 才有的），keepane 就用回车进入 pane 的那一刻代替，所以那里的时间从按键算起，而不是从 shell 开始执行算起。按 `prefix C-t`（或 `set -g pane-timestamps on`），命令所在那一行的右端就会显示它什么时候开始、跑了多久、有没有失败：

```text
PS C:\src> cargo build                                     14:03:22 41s ✓
PS C:\src> cargo test                                      14:04:10 12s ✗
```

时间信息显示在行尾空白处，不改变 pane 宽度或程序输出；copy mode 和 `capture-pane` 不会包含这段信息。如果行尾空间不足，就不显示。脚本要用的话，`keepane list-marks` 打印同样的信息；`list-marks -J` 输出 JSON，还带上每条命令占的行（命令可能折行，提示符可能占两行）和命令本身。手机上在视图菜单里打开“每条命令的时间”，时间显示在左边一栏。

既然知道每条命令从哪开始、到哪结束，`C-b y`（`copy-output`）就把上一条命令的输出（它和下一个提示符之间的行，最多 4 MB）复制到粘贴缓冲区和剪贴板：测试失败的输出，直接贴进 issue 或者发给 agent。`keepane copy-output -p -t %3` 改成打印出来，给脚本用。copy mode 里 `[` `]` 在命令之间跳。

`C-b Y`（`choose-marks`）列出所有 pane 的 shell 跑过、屏幕上还在的命令，最新的在前：结果（✓、✗）、时间、耗时、在哪个 pane。Enter 跳到那条命令（进入 copy mode，停在它那一行）；`y` 复制它的输出，和 `C-b y` 对上一条命令做的一样；`r` 在原 pane 再跑一次（只在那个 shell 停在提示符时）；`f` 过滤列表（输入 `✗` 只看失败的）。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-fix-light.gif">
    <img src="docs/img/keepane-fix.gif" width="880"
         alt="左边 pane 里测试失败；C-b y 复制它的输出；send-message 交给右边的 agent，agent 改好代码、报告空闲；测试随后通过">
  </picture>
</p>

测试失败；`C-b y` 取下它的输出；一条消息交给旁边的 agent（`keepane send-message --to %claude "fix this: $(keepane show-buffer)"`），它空闲时就收到。（录屏里的 agent 是写会话记录的替身；用 `keepane setup` 装好的 agent 也一样。）

### SSH 机器

`C-b @`（`choose-ssh`）在一个圆角框里列出 `ssh` 去过的机器：正连着的排在最前（绿点，加上所在 pane），其余按上次连接时间排。每行是名字、用户和地址（`root@10.0.0.5:2222`）、多久前、连过几次；还没从 pane 连过的，标出来源（`config`、`seen`、`history`）。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/ssh-light.png">
    <img src="docs/img/ssh.png" width="880"
         alt="pane 上方的 ssh 框：正连着的机器带绿点和所在 pane，其余按上次使用时间排，按键写在框的下边">
  </picture>
</p>

Enter 跳到连着这台机器的 pane，没有就新开一个以它命名的窗口运行 `ssh`；`s` 在旁边分屏连接，`y` 复制命令，`d` 把它从列表里去掉，`f` 过滤。

列表来自四处：keepane 在 pane 里看到的每个新 `ssh`（每次记一次，带时间）、`~/.ssh/config` 的 `Host` 行、`~/.ssh/known_hosts` 里的名字（哈希过的看不出名字，跳过），以及第一次时从各 shell 历史文件里读到的 `ssh` 命令。连接过的机器存在 `%LOCALAPPDATA%\keepane\ssh-hosts.json`：名字、用户、端口、时间和次数。不读也不存任何密码或密钥。`keepane ssh-hosts` 打印这个列表（`-J` 输出 JSON，MCP 工具 `list_ssh_hosts` 给 agent 的就是它）；`#{pane_ssh_host}` 给出 pane 里的 `ssh` 连到哪，dashboard 把它显示成这个 pane 在运行的东西。

带脚本或命令启动的 shell（`pwsh -File`、`bash -c`）keepane 不去动它，也就没有 hook；可以在那个脚本里自己装上：PowerShell 里写 `Invoke-Expression (keepane __shell-hook | Out-String)`，bash 里写 `eval "$(keepane __shell-hook)"`。

fish 也有 hook（fish 4 自己标记命令，keepane 补上其余部分）。以 `wsl.exe` 启动的 pane 也有（`default-shell wsl`，或者不带命令的 `wsl.exe -d Ubuntu`）：keepane 启动 WSL 用户自己的 shell（bash、zsh 或 fish），先按登录 shell 的方式读启动文件，再装上 hook。这种 pane 用发行版自己的历史；`set-environment` 的变量通过 `WSLENV` 传进去，和 PowerShell 一样能拿到。别的 shell 用 Windows Terminal 和 VS Code 也认的那套序列（OSC 133）报告命令。SSH 到别的机器上的 bash，或者在 PowerShell pane 里自己启动的 WSL shell：

```bash
PS0='\e]133;C\e\\'
PROMPT_COMMAND='printf "\e]133;D;%s\e\\\e]133;A\e\\" "$?"'
```
pane 的输出也可保存到磁盘（`log-history`，默认开）：每个 pane 位置每天一个纯文本文件，放在 `%LOCALAPPDATA%\keepane\history\<session>\<窗口>.<pane>\2026-09-25.log`，留 30 天（`log-history-days`），每个文件每天最多 20 MB。一行字从 pane 顶上滚出去的时候才写，所以进度条、正在编辑的提示符只留下最后的样子；vim 这类全屏程序什么都不留；pane 关掉时屏幕上还剩的内容，那时一起写进去。报告过的命令前面有一行它的时间（`── 14:03:22 · 41s · ✓ ──`）。

`prefix /`（`choose-history`）列出有历史的 pane 位置和它们的日期。在某一天上按 Enter，就在弹出框里用查看器打开。查看器从末尾开始看，用法和 `less` 一样：`j` `k`、`Space` `b`、`g` `G`，`/` `?` 搜索，`n` `N` 找下一个，`[` `]` 在命令之间跳，`q` 退出。`keepane view 文件` 用它打开任何文件（`-n 行号` 从那一行开始，`-s 文字` 预设好搜索）。在这个列表里按 `/`，就一次搜遍所有 pane 的所有日期（最新的一天在前，最多 500 行）：列出含这段文字的行和它的日期、pane，Enter 在查看器里打开，停在那一行，搜索也设好了。脚本用 `keepane find-text -H 关键词`，和屏幕上的结果一起给出：`work:0.1  2026-10-21:1234  error: …`。`set -g log-history off` 就不记了。

用 `kill-pane` 或 `kill-window` 关掉的 pane 或窗口（`prefix x`、`prefix &`）会保留 10 秒，里面的程序继续跑。这期间按 `prefix u`（`undo-kill`）就放回原处。过了 10 秒就和以前一样彻底没了。秒数用 `undo-kill-time` 改；设成 0 就立刻结束，关程序是为了释放端口或文件的时候就该这样。一个 session 的最后一个 pane 不保留，因为 session 会跟着它一起结束。

## 重启之后接着用

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/keepane-resume-light.gif">
    <img src="docs/img/keepane-resume.gif" width="880"
         alt="一个三块的 session：git status、名为 builder 的编译、服务器日志；C-b C-s 保存；server 像重启时那样停掉；keepane list-saved 列出这个 session，keepane resume 把各块、它们的名字和目录、原来显示的内容都带回来">
  </picture>
</p>

每个 session 的结构（有哪些窗口、每个窗口怎么分的、每块里跑的是什么命令、在哪个目录）都会存成一个文件，放在 `%LOCALAPPDATA%\keepane\sessions` 下面。结构一变就存一次，`kill-server` 的时候也存。所以不管是重启、崩溃还是手滑 `kill-session`，文件都还在。session 里最后一个程序退出（比如在最后一个 shell 里敲了 `exit`）时，它的保存文件 10 秒后删掉（机器正在关机时不删）；`C-b w` 里按 `D` 或用 `delete-saved` 可以手动删掉。所以：

```powershell
keepane resume              # 把存过的 session 全恢复出来，进第一个
keepane resume work         # 只恢复 work（要是它本来就在跑，那就直接进去）
keepane list-saved          # 有哪些能恢复，最新的排前面
keepane delete-saved old    # 不要了
keepane save-session -a     # 现在就把每个 session 各存一遍（prefix C-s 也是这样）
```

直接运行 `keepane` 也会接着用：有 session 在跑，就进最近用的那个；一个都没在跑，就把存档全恢复出来，进最近存的那个；什么都没有才新开一个（`keepane new` 总是新开）。所以反复打开 keepane 不会攒下一堆空 session，下次恢复时也不会冒出来。

恢复出来的是布局、每个 pane 的启动命令、所在目录，还有每块屏幕上最后 `save-history` 行（默认 500 行；`set -g save-history all` 把整段 scrollback 连颜色一起存下来）的输出。程序当时跑到哪是回不来的，谁也做不到。存档是自动的：布局一变就存，pane 上的文字每 30 秒存一次，Windows 关机、重启、注销时再整个存一遍（server 会把关机拖住那一瞬间）；Linux 和 macOS 上 server 收到 SIGTERM 或 SIGHUP 时也一样。想让 server 一启动就自己恢复，配置里写 `set -g restore-on-start on`；不想存就 `set -g autosave off`；`sessions-dir` 可以换目录。

每个 PowerShell、bash、zsh pane 的命令历史（按 ↑ 翻出来的那些）也各自保存，放在会话存档目录下，所以恢复后的 pane 翻到的是它自己跑过的命令，而不是所有 pane 混在一起的。新开的 pane 会复制一份它来源的那个 pane 的历史（分屏时是被分的那个，新窗口时是当前在用的那个），没有来源就复制这个 shell 自己的历史文件。没有 pane、也没有存档再引用的历史文件，超过 `log-history-days` 天会被清掉。

发给 `shell` 模式 pane 的消息在那里也是一条命令，会带着信封头（`<# keepane #12 from %builder #>`）进这个历史。`keepane shell-history -t %3` 把历史读出来，消息那行会标出来（`✉ #12 %builder  cargo test`）；`-c` 只看命令，去掉消息；`-m` 改为列出事件日志里所有发给这个 pane 的消息（任何模式），带上每条的处理结果；`-n 20` 只看最后 20 条。

想让这一切在登录时自动发生：

```powershell
keepane startup on          # 登录时启动 server，把存过的 session 全恢复出来
keepane startup status      # 看看注册了什么
keepane startup off         # 取消
```

在 Windows 上，它只在当前用户的 `Run` 注册表键下写一个值，不需要管理员权限，也不碰任务计划；server 通过 `conhost --headless` 启动，登录时不会闪出控制台窗口。在 macOS 上，它写一个 launchd agent：`~/Library/LaunchAgents/tech.dfine.keepane.plist`。在 Linux 上，它写一个 systemd 用户 unit：`~/.config/systemd/user/keepane.service`，挂在 `default.target` 下。这两种都记下运行 `startup on` 时的 `PATH`、`SHELL` 和语言区域设置，所以 pane 里能找到的程序和你的 shell 一样。写完不会马上启动 server，下次登录时才启动。Linux 上注销时 server 会停（先保存）；用 `loginctl enable-linger` 让用户的 systemd 一直运行，server 就不停，开机时也会启动。没有 systemd 的地方（容器、没开 systemd 的 WSL），`startup on` 会说明原因，什么也不写。重启之后 `keepane attach` 进去，东西都在。手机页面（`keepane web`，见下文）原来开着的话，也会按原来的参数重新打开（加 `-k`，二维码就不变）；登录后网络还没就绪时，每 10 秒重试一次。

登录启动项按完整路径运行 keepane，下面的 Windows Terminal profile 也一样。升级后新版装进了自己的文件夹（Scoop 设了 `no_junction`、每个版本一个文件夹、Homebrew 的 Cellar）时，之后第一次运行的 keepane 会把这两处都改成自己，每改一处用一行字告诉你；旧文件夹已经被删掉时也一样。keepane 不会从更新的版本手里接过来。在 macOS 和 Linux 上，`startup on` 登记的是 `PATH` 里找到的那个名字（Homebrew 的 `bin/keepane`，每次升级都会指向新版），不是 Cellar 里带版本号的文件。

升级：装了新版 keepane，已经在跑的 server 不会被换掉。它还是旧程序，你的 session 都在它里面。

```powershell
keepane version          # 这个 keepane 的版本；server 版本不同时一并显示
keepane update --check   # 有没有新版
keepane update           # Windows：按当初的安装方式（MSI 或 scoop）装新版
keepane restart-server   # 把正在跑的 session 全部挪到新版本的 server
```

`restart-server` 会先存档正在跑的 session，停掉旧 server，起一个新版本的，再只恢复刚才在跑的那几个（布局、历史、目录都在）；接着的终端会自己重新接上（0.10 起；更老的 server 上的终端会被断开，`keepane attach` 接回去）。在 pane 里面运行时，它会跑到 pane 外面去完成，结果写进 `%LOCALAPPDATA%\keepane\restart.log`。选项和按键绑定会重新从配置文件读，和 `kill-server` 之后一样：之后用 `set` / `bind` 临时改的不会带过去（旧 server 分不清哪些是默认值、哪些是你改的，全搬过去会把旧版本的默认值钉在新版本上）。终端接着一个不同版本的 server 时，标题栏会提示。`update` 下载 MSI 后先核对旁边发布的 SHA-256 再交给 Windows Installer；任何安装都不会在后台偷偷进行。

server 每天会向 GitHub 查一次最新发布的版本号（用 `curl` 向 `api.github.com` 发一个请求，不发送任何关于你或你的 session 的信息）。如果比当前版本新，状态栏会显示出来（比如 `⇡ 0.19.0`，也就是格式变量 `#{keepane_update}`），`show-messages` 里也会说明怎么升级。查到的结果存在数据目录里，所以 server 重启后当天不会再查。不想让它联网，就 `set -g update-check off`（或者设置环境变量 `KEEPANE_NO_UPDATE_CHECK`）。

PowerShell、bash、zsh、fish 里的 Tab 补全（命令名、每条命令的 flag、`-t` 后面从运行中的 server 取 session / 窗口名、`set` / `show` 后面的选项名和取值；`splitw` 这样的别名、`split-w` 这样的前缀都按它代表的命令补）由程序自己吐出一段补全脚本，加一行就能用。PowerShell 里对别名成 `tmux` 的也有效。Windows PowerShell 5.1 不会拿 `-` 开头的词来问程序的补全脚本，所以那里 flag 只在 PowerShell 7 里能补。用 Homebrew 装的，bash、zsh、fish 的补全会自动装好。

```powershell
keepane completion powershell | Out-String | Invoke-Expression   # $PROFILE
```

```bash
eval "$(keepane completion bash)"                                # ~/.bashrc
source <(keepane completion zsh)                                 # ~/.zshrc，放在 compinit 之后
keepane completion fish > ~/.config/fish/completions/keepane.fish
```

keepane 里面 `:` 命令行按 Tab 也能补：命令名、输到 `-` 时这条命令的 flag（别名和前缀按它代表的命令算，已经写过的不再列）、`-t` 后面的目标、`set` / `show` 后面的选项名（缩写也行：`sync`、`mon-act`），以及只有几个取值的选项的值（`on`/`off`、`top`/`bottom`）；多个候选时补到相同的部分为止，候选列在提示符里。

PowerShell 默认不给 Ctrl+D 绑任何功能（bash 里是退出），所以它也关不掉 pane。想让它在空行上退出，在 `$PROFILE` 里加一行：

```powershell
Set-PSReadLineKeyHandler -Chord Ctrl+d -Function DeleteCharOrExit
```

想让 keepane 出现在 Windows Terminal 的下拉菜单里：

```powershell
keepane windows-terminal install    # 一个 "keepane" profile，打开就接上（或新建）名为 main 的 session
keepane windows-terminal status
keepane windows-terminal remove
```

这是一个 profile *片段*，也就是 keepane 自己的一个 JSON 文件，放在 `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\keepane\` 下，Windows Terminal 会把它合并进来，不碰你的 `settings.json`；重装也保持同一个 profile 身份，你给它改的字体、配色都还在。从它开的每个标签页都进同一个 session，和 `tmux new -A -s main` 一样。

### pane 记住自己在哪个目录

pane 的目录跟着 shell 的 `cd` 走，不用你配置：启动 PowerShell（pwsh 或 Windows PowerShell）、bash、zsh、fish 和 `wsl.exe` pane 里的 shell 时 keepane 挂一个 prompt 钩子，每次提示符后面追加一段不可见的 OSC 9;9 或 OSC 7 上报目录（你自己的 prompt、oh-my-posh 之类照旧）；`cmd.exe`、`sh` 和其他程序则直接读进程自己的工作目录。shell 自己上报的目录（OSC 9;9，带不带引号都行；OSC 7，从 WSL 来的 `/mnt/<盘符>/...` 会映射回 Windows 的盘）优先采信，`keepane set-cwd`（不带参数就是你运行它时所在的目录）可以手动指定，也可以 `keepane set-cwd -t work:0.1 D:\proj` 给别的 pane 指定。

`list-panes` 能看到每个 pane 记的目录，状态栏里用 `#{pane_current_path}` 显示。

## 在 AI agent 里用

pane 里的 agent 收发的是和别人一样的消息。要让它顺畅，需要两样东西：一个在 agent 每轮结束时运行 `keepane pane-ready -q` 的 hook（这样 `ai` 模式的 pane 才算准备好了），以及 `keepane mcp`，一个把上面那些命令作为工具提供的 MCP 服务端（stdio）。它从 `KEEPANE_PANE` 知道自己服务的是哪个 pane，所以 agent 发出的消息，发送方就是这个 pane。

`keepane setup` 列出它认识的每个 agent：本机装没装、hook 和 keepane 的 MCP 服务端配好没有。`keepane setup <agent>` 打印这个 agent 需要的配置；加 `--install` 就替你装上，每个文件改之前先备份，只加 keepane 自己的条目：

| agent | `setup` 后面写 | hook（session 开始、每轮结束） | MCP 服务端 |
|---|---|---|---|
| Claude Code | `claude` | `~/.claude/settings.json` 里的 `SessionStart`、`Stop` | `claude mcp add --scope user` |
| Codex | `codex` | `~/.codex/hooks.json` 里的 `SessionStart`、`Stop` | `codex mcp add` |
| Gemini CLI | `gemini` | `~/.gemini/settings.json` 里的 `SessionStart`、`AfterAgent` | 同一个文件的 `mcpServers` |
| Cursor CLI | `cursor` | `~/.cursor/hooks.json` 里的 `sessionStart`、`stop` | `~/.cursor/mcp.json` |
| opencode | `opencode` | 一个插件 `~/.config/opencode/plugins/keepane.js`，启动时和 `session.idle` 时运行 | `opencode.json` 的 `mcp` |

hook 运行的都是 `keepane pane-ready -q`：在 keepane 之外什么也不做，在不是 `ai` 模式的 pane 里被忽略。Codex 的 `notify` 只能填一个程序，keepane 不碰它；Codex 的新 hook 要你在 Codex 里用 `/hooks` 确认信任一次才会运行。不是纯 JSON 的文件（比如带注释）不会被改，`setup` 会告诉你手工加什么。这些都按各家的文档写，端到端实测过的只有 Claude Code。自己手写 hook 时，程序路径不要加引号（或者写成 `& "C:\路径\keepane.exe" pane-ready -q`）：Windows 上 agent 可能用 PowerShell 执行 hook，在 PowerShell 里"带引号的路径后面跟参数"是语法错误。别的 agent 只要能在每轮结束时运行一条命令、能用 stdio 上的 MCP 服务端，也一样能接；启动它之前在它的 pane 里运行 `keepane set-work-mode ai`。

Codex（0.160 起）和 pi（1.0 起）默认在备用屏幕里画界面，和 vim 一样：它们输出的内容不会进滚动历史，在 keepane、tmux 或任何终端里都一样，所以往上翻不到历史。滚轮（以及手机上的滑动）仍然能翻，翻的是它们自己的界面。想让历史留在 pane 里，就让它们用内联模式：Codex 在 `~/.codex/config.toml` 的 `[tui]` 下写 `alternate_screen = "never"`（或者用 `codex --no-alt-screen` 启动）；pi 在 `~/.pi/agent/settings.json` 里写 `"tuiMode": "regular"`。

发给 `ai` 模式 pane 的消息，如果这个 pane 的 agent 启动以来从没报告过空闲，消息就会排队等着。`send-message` 会直接说明，并给出补上 hook 的 `setup` 命令；dashboard 和手机页上也会提示（`#{pane_unheard}`）。对 Claude Code、Codex 和 pi，keepane 还会读 agent 的会话记录（和统计花费一样，每 2 秒读一次）：记录里一轮结束了，即使没有 hook 也会把 pane 设为空闲，最多晚约 3 秒；用户按 Esc 打断时 hook 不会运行，但会话记录会记下，pane 同样变为空闲，正在处理的消息记为未完成。有 hook 时会立即生效。

26 个工具：

| 工具 | 用途 |
|---|---|
| `whoami`、`list_panes` | 自己的 pane，以及所有 pane：地址、名字、模式、忙闲、收件箱、程序、状态 |
| `send_message`、`reply`、`wait_message`、`current_message` | 发消息、回复发送方、在一轮之内取下一条消息、查看正在处理的消息（以 keepane 记录的为准） |
| `list_messages`、`trace_message`、`drop_message`、`move_message` | 收件箱，以及一条消息后来怎样了（shell 命令的输出也在里面） |
| `create_session`、`create_window`、`split_pane`、`rename_pane`、`kill_pane` | 开 pane（可以带名字、模式和第一条消息），给任意 pane 改名、关掉任意 pane |
| `set_status`、`set_work_mode` | 报告自己在做什么（dashboard 上显示）；改自己 pane 的模式 |
| `list_tasks`、`show_task`、`query_events` | 消息链（任务）和事件日志 |
| `wait_output` | 等某个 pane 打出含某段文字的一行（服务器就绪、构建报错），拿到这一行 |
| `list_links`、`link_info`、`read_screen`、`create_remote_pane`、`push_repo` | 配过对的机器、某台机器的信息、某个 pane（本机或另一台）的屏幕、在另一台机器上开 pane（用 `kill_pane` 关），以及把当前分支的提交送过去 |

通过 MCP 开的 pane，没指定模式时：跑 `claude`、`codex`、`gemini`、`cursor-agent`、`opencode` 的是 `ai` 模式，跑 `pwsh`、`powershell`、`bash`、`zsh` 的是 `shell` 模式，其他是 `normal`。agent 能启动的程序限于 `agent-commands`（Windows 上是 `pwsh powershell claude codex`，其他系统是 `bash zsh sh claude codex`），它和它开的 pane 一共能开多少个受 `agent-pane-limit`（8）限制。Claude Code 调用你没放行过的 MCP 工具前会先问你。

## 在手机上用

编译、部署或者 AI 助手在电脑上跑着，人走开了也想看一眼、回一句：

```powershell
keepane web
```

终端里会打出一个二维码。手机连同一个网络，用相机扫一下，浏览器就打开一个页面：按 session、窗口分组列出所有 pane（session 名字旁边是它多数 pane 所在的目录）：每个 pane 的名字（大字显示；没起名字就显示程序自己设的标题，再没有就显示程序名），下面一行是在跑的程序，目录和 session 的不同时也写出来；最后输出的一行（不算提示符）、多久没有新输出；一个表示状态的小圆点（ai、shell 模式的 pane 空闲绿、忙黄，已退出红）、收件箱里排队的消息数，以及状态栏上那几个提醒标记（开了 `monitor-activity` 这类选项时：`#` 有输出，`!` 响铃，`~` 太久没动静），不用点进去就知道哪个任务跑完了。点进一个，就能看到它的屏幕，颜色都在；屏幕一有变化 keepane 就把新内容推过来，不用等刷新。在屏幕上左右滑动可以切到上一个、下一个 pane，点标题可以直接选一个；从屏幕左边缘向右滑回到列表。刷新页面后仍停在原来打开的 pane。在下面输入框里打的字会边打边发到 pane（在框里删字，那边也跟着删；输入法选好字才发），回车就是回车。在视图菜单（pane 标题栏里的滑块图标）里可以改成等回车再发，每台设备各自记住：点"发送"只发输入框里的文字，不带回车（输入框空着时再点一次就是回车），或者发出去并直接执行。输入框左边的历史按钮可以调出发过的命令（☆ 设为常用，固定在最上面）。上面一排是手机键盘上没有的键（Esc、Tab、方向键，更多的在 ⋯ 里；Ctrl+C 和回车固定在这一排的最右边，不会被滚走）；先点 Ctrl、Alt 或 Shift（可以同时按住几个），再点这一排的键或输入一个字母，就是带着它们按下（比如先点 Shift 再点 → 就是 Shift+→）。pane 标题栏右边的 ⋯ 菜单，以及在列表里对 pane 右键（手机上长按），可以分屏、开新窗口、重命名 pane 或它所在的窗口和 session（pane 名字留空就去掉名字）、关掉 pane、打开它的收件箱（点卡片上的"排队 N 条"或"处理中 #N"也能打开）：正在处理的消息、排队中每条消息的全文（可以置顶、上移、下移、删除，删了能撤销），以及最近完成的几条。其余的在视图菜单里：“每条命令的时间”在左边加一栏，显示每条命令开始的时间（点一下看日期、耗时和退出码，还可以复制这条命令或它的输出，见“命令时间和历史”一节）。点 session 或窗口可以折叠、展开（设备会记住）；对它右键或长按可以重命名，session 还能一次折叠、展开所有窗口。列表顶部的两个按钮可以把列表换成网格（设备会记住）：每个 pane 一格，手机竖屏一行两格，屏幕更宽时一行更多；每格有状态、名字、收件箱、所在窗口，以及它最后输出的 4 行（终端的配色）。点一格打开那个 pane，长按打开它的菜单。比手机屏幕宽的 pane，长行会在手机屏幕边缘自动换行（视图菜单里可以关掉，适合全屏程序）。视图菜单里的“适配这块屏幕”把 pane 适配到手机：它占满所在窗口，整个会话变成手机的行列数，vim、htop、agent 这类全屏程序就按手机的大小重画。适配期间，电脑和其他连着的手机看这个会话也是手机的大小（一个会话只有一个大小，页面和状态栏都会提示）；离开这个 pane、回到列表或者锁屏，就恢复原来的大小，没有手机看它 10 秒后 keepane 也会自动恢复。命令都在电脑上执行，手机只负责看和输入。“添加到主屏幕”之后，它打开起来就像一个 App。

屏幕下面是这个 session 的状态栏，和终端里画的一样，颜色也一样，前提是一整行放得下；放不下时（比如手机竖屏），点 pane 顶部工具栏里（全屏时在输入框旁边）的状态栏按钮，从侧边打开它，什么都不截掉：窗口一行一个，下面是两端的每一段，颜色照旧。点一个窗口就打开它。视图菜单里的“状态栏”只在这台设备上隐藏它；`set -g status off` 在所有地方都隐藏。

在 shell 或 ai 模式的 pane 里，输入框旁边的信封按钮让发送变成发消息：内容进这个 pane 的收件箱，发送者是 user，和别的消息一样排队（同 `send-message`）。⋯ 菜单和卡片的菜单可以设置 pane 怎么处理收到的消息：normal 留着等，shell 在提示符下执行，ai 交给 agent。视图菜单里还能在输出里查找（含查找内容的行会标出来，用 ↑ ↓ 切换）、复制屏幕文字、调整字号（也可以双指缩放屏幕，每台设备各自记住）。顶部的 + 在你的主目录开一个新 session。程序已经退出的 pane（开了 `remain-on-exit`）会写出退出码，点“重新运行”再启动一次。全屏程序（默认设置下的 Codex、pi，还有 vim）不留历史：在它的屏幕上上下滑动，会像电脑上的滚轮一样让它自己滚动，pane 标题栏会注明。

同一个页面也能在电脑浏览器里用：宽屏时左边是 pane 列表，右边是 pane。左上角的按钮把列表收成一条窄栏（每个 pane 一个按钮），再按一次展开；触屏上在列表上左滑收起，在窄栏上右滑展开。右上角的按钮选页面自己的外观（日间、夜间或跟随系统，每台设备各自记住），也能选终端主题（`theme`，见“主题”一节）：终端主题是电脑上的，电脑的终端会跟着变，所有页面里的 pane 也都按它显示。同一个菜单还能选页面的语言：跟随系统（系统是中文就显示中文，否则英文）、中文或英文。旁边显示到电脑的往返时间（如 `23 ms`，100 ms 以下绿点，300 ms 以下黄点，再高或连不上是红点和“离线”），pane 工具栏里的全屏按钮只留下屏幕和输入框，浏览器支持时铺满整个屏幕（iPhone 上铺满整个页面）；输入框旁边的按钮或浏览器自己的退出全屏都能回来，旁边的列表按钮可以不退出全屏直接换到别的 pane。（页面由 `web/` 构建，用 React 和 HeroUI，打包成一个文件编进 keepane。）

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/img/phone-zh-light.png">
    <img src="docs/img/phone-zh.png" width="620"
         alt="手机上的 keepane web：左边是 pane 列表和各自在跑的程序，右边是一个 pane 的屏幕，显示彩色的 git log，左侧一栏是每条命令的时间，下方是一排按键和输入框">
  </picture>
</p>

二维码里是地址加一个密钥，密钥每次启动重新生成（128 位随机数）。除了页面本身，没有密钥什么都拿不到；手机只能看、往 pane 里输入、用那个 ⋯ 菜单，发不了任何自己的 keepane 命令。不启动就不开。启动以后由 keepane 的 server 在后台提供服务，运行 `keepane web` 的终端马上就空出来了；它会一直开着，直到你关掉它或者 server 退出：

```powershell
keepane web status   # 地址，以及谁连着（在看哪个 pane）
keepane web stop     # 关掉；连着的手机随之断开
keepane web          # 开着的时候再运行一次：重新打出同一个二维码
```

session 恢复时（`keepane startup`、`keepane restart-server`），它也跟着按原来的参数重新打开，直到你运行 `keepane web stop`。想让它随 keepane 一直开着，就在 `~/.keepane.conf` 里加一行 `web-start -k`：server 启动时自己把它打开，每次都是同一个二维码（`-k` 见下）。没写 `--bind` 时，它每 10 秒重新查一次本机的地址：新出现的地址（登录后才连上的 Tailscale、换了一个 Wi-Fi）也会开上。

开着的时候，状态栏会显示 `web` 和当前连接数（自己配状态栏可以用 `#{web_url}` 和 `#{web_clients}`）；dashboard（`prefix v`）的 pane 列表标题上也显示同样的数字。有手机连上，或者有人用错误的密钥访问，状态栏都会提示。

```powershell
keepane web --read-only     # 只能看，不能输入
keepane web --keep-key      # 下次还用同一个二维码，收藏的网页一直能用
keepane web --port 8080 --bind 192.168.1.23   # 换端口，或者指定网卡
keepane web --bind 192.168.1.23,10.0.0.5     # 多张网卡：每个地址都监听（也可以写多个 -b）；二维码用第一个
```

用的是普通 HTTP，适合自己家里的网络：在公共网络上，抓包的人能看到密钥。在外面想用，就在中间加一层 Tailscale 这类私有网络：`keepane web` 会同时监听本机的局域网地址和 Tailscale 地址（`web status` 会列出来）。第一次运行时 Windows 会问是否允许 keepane 联网，选“专用网络”允许即可。

### 分享文件或文件夹

一个命令把文件或文件夹分享出去：手机扫二维码，或者在别的电脑上打开链接，就直接下载。

```powershell
keepane share report.pdf            # 分享一个文件：打印二维码和链接
keepane share ~/photos --for 2h     # 分享一个文件夹（下载为 photos.zip），2 小时内有效
keepane share build.log --once      # 完整下载一次后失效
keepane share                       # 列出正在分享的（也可以加 -l）
keepane share stop 2                # 取消一个；--all 取消全部
```

每次分享有自己的链接（一个随机令牌，不是手机页面的密钥）：拿到链接的人只能下载这一个东西，别的什么都看不到。默认 24 小时有效，`--for` 可以改（`30m`、`2h`、`7d`；`0` 表示一直有效，直到取消或 server 退出）。文件原样下载，下载中断后能从断开的地方接着下；文件夹边读边打成 zip 下载（里面的符号链接不打包）。分享的东西一律作为下载发出，不会被浏览器当作网页打开。链接用的是手机页面的地址和端口：手机页面没开时，`share` 会把它打开（只到 server 退出为止）。每次分享、谁在下载、什么时候下完，状态栏都会提示；pane 里的 agent 发起的分享也一样。和手机页面一样是普通 HTTP：在不信任的网络上请走 Tailscale。设计稿：[docs/design/share.md](docs/design/share.md)。

## pane 完成时通知你

keepane 知道 pane 什么时候"完成"了：一条跑了一段时间的命令结束（靠 shell 报告的命令记录，和命令时间是同一个来源）、agent 这一轮结束（它的 hook 运行了 `keepane pane-ready`）、一条消息的任务结束，或者程序退出。它会通过这几种方式告诉你：

- **手机页**（页面开着的时候）：顶部弹出一条横幅（点一下就跳到那个 pane），响一声，安卓手机会震动，标签页标题上显示未读数。页面在后台时完成的，切回页面时会补告诉你。普通 HTTP 的页面没法弹系统通知，手机锁屏或切走后浏览器也会暂停页面，所以锁屏也要收到的话，用 hook 或 webhook。
- **电脑桌面**：`set -g notify on`。
- **webhook**：`set -g done-webhook <地址>`，keepane 每次都用 `curl` 往这个地址发一条，格式由 `done-webhook-format` 决定：`json`（keepane 自己的格式）、`text`（只有那一行文字：ntfy、Bark）、`feishu`（飞书）、`wecom`（企业微信）、`dingtalk`（钉钉）、`slack`、`discord`（聊天工具的群机器人 webhook）。发到聊天工具的消息以 `keepane` 开头，飞书、钉钉机器人的"关键词"安全设置可以填它；对方拒收的话，`show-messages` 里能看到原因。
- **`pane-done` hook**：想接别的服务就用它。这次完成的信息在环境变量里：`KEEPANE_DONE_KIND`（`command`、`agent`、`task`、`exit`）、`KEEPANE_DONE_TEXT`（那一行文字）、`KEEPANE_DONE_OUTPUT`（pane 最后输出的几行）、`KEEPANE_DONE_PANE`、`KEEPANE_DONE_NAME`、`KEEPANE_DONE_OK`（成功 1、失败 0，知道时才有）、`KEEPANE_DONE_EXIT`（退出码，shell 报告了才有：PowerShell 只说成功还是失败）、`KEEPANE_DONE_SECONDS`、`KEEPANE_DONE_GIT`（agent 这次工作在仓库里做了什么，一行，见下文），以及 `KEEPANE_DONE_JSON`。

```sh
set -g done-webhook https://open.feishu.cn/open-apis/bot/v2/hook/<token>
set -g done-webhook-format feishu
# ntfy（iOS、安卓都有 App），用 hook：Linux、macOS 上是 sh
set-hook -g pane-done run-shell 'curl -s -d "$KEEPANE_DONE_TEXT" ntfy.sh/<你的频道>'
# Windows 上是 PowerShell
set-hook -g pane-done run-shell 'curl.exe -s -d $env:KEEPANE_DONE_TEXT ntfy.sh/<你的频道>'
```

哪些算"完成"、对哪些 pane：

```sh
set -g done-events command agent  # command agent task exit 任选；all 全部；none 都不要
set -g done-after 30              # 命令至少跑多少秒才算
set -g done-panes named           # 有名字或 ai/shell 模式的 pane；all 是所有 pane
set -g done-lines 5               # 一起带上 pane 最后输出的几行（0 不带）
```

每条通知都带上 pane 最后输出的几行（命令自己的输出；agent 则是它最后一次 `pane-status` 说的话），聊天工具和桌面通知里写在那一行下面，keepane 的 JSON 里是 `output`。

同一个 pane 三秒内只通知一次：agent 这一轮结束和它完成的任务算一次。`keepane list-done` 列出最近 100 条。

## pane 所在仓库的 git 信息

pane 的目录在 git 仓库里时，keepane 读两样东西：仓库现在的状态，以及 agent 这次工作做了什么。

现在的状态显示在状态栏和时钟的分支名后面：`main ●3 ↑2 ↓1 !1` 表示改了 3 个文件（含新文件）、比上游领先 2 个提交、落后 1 个、1 个文件有冲突；没有的项不显示。对应的变量是 `git_branch`、`git_head`（提交号，短）、`git_changed`、`git_ahead`、`git_behind`、`git_conflicts`（为 0 时为空），以及把上面合在一起的 `git_marks`，可以用在 `pane-border-format` 或自己的状态栏里。dashboard 在选中 pane 的信息里也有一行显示同样的内容。

agent 这次工作做了什么，在 `ai` 模式的 pane 里随工作结束一起给出。起点是工作开始时仓库所在的提交（消息送达时，或者 agent 空闲时有人给它打字时）。内容包括之后新增的提交、工作区与起点相比改了哪些文件（每个文件增删的行数），以及 git 还没跟踪的新文件：

```text
git:
main 1a2b3c4..5d6e7f8: 1 commit, 2 files +40 -12, 1 new file
  5d6e7f8 parse the header
  src/parser.rs +38 -12
  src/lib.rs +2 -0
  new notes.md
```

它出现在 `trace-message` 里消息的下面（JSON 里是 `git`），所以 lead 用 `trace-message <id> -w` 等结果时会一起拿到；也随完成通知一起发出（`list-done`、webhook、桌面通知、`KEEPANE_DONE_GIT`、`KEEPANE_DONE_JSON` 里的 `git`）。消息要等这些读完才算完成并发出通知。git 在单独的线程里运行，不会让其他事情等待（实测：仓库里有 3000 个新文件时约 0.1 秒）。如果工作结束时所在的仓库不是开始时的那个，或者起点提交已被 rebase 掉，就从结束时的提交算起，并注明 `(start unknown)`；git 在但回答不了时写 `changes unknown (git failed)`。它统计的是工作开始以来工作区里所有的改动，不管是谁改的：两个 agent 共用一个工作区时会混在一起，需要分开时给每个 agent 一个独立的 worktree（`git worktree add`）。

只有 git 才能回答的问题才运行 git。git 不在 server 等待时运行，每次最多 10 秒（agent 的一次工作最多运行 4 次），也不会抢 agent 正在用的 index 锁（`--no-optional-locks`）。keepane 会自己在 pane 所在的任何仓库里运行 git，所以关掉了仓库配置可以让 git 启动的程序：文件监视（`core.fsmonitor`）、外部 diff、文本转换和分页器。你自己运行 `git status` 时仍会运行的程序（例如 Git LFS 的 filter），keepane 运行时也会运行。有东西在显示状态时，最多每 10 秒重新读一次；pane 里的命令或 agent 一轮结束后立即重新读。keepane 先用 PATH 上的 git，找不到再去 Git for Windows、GitHub Desktop、Homebrew 的安装位置找；`set -g git-command <路径>` 可以指定。没有 git 时，分支和提交号照样从仓库文件里读出，其他数字留空，agent 的工作写 `changes unknown (git not found)`。

## 跨电脑

两台电脑上的 pane 也能像同一台上那样互发消息，走的是 `keepane web` 监听的那个端口，前提是两边的 keepane server 配过对。做法和 SSH 的公钥一样：每个 server 有自己的一对密钥，还有一张“允许哪些机器进来”的表；配对就是把双方的公钥各写进对方的表里，只做一次。在要配对的那台机器上运行 `keepane web`，拿到它打出的地址（就是手机扫的那个，带密钥）；然后在这台上：

```powershell
keepane link add http://100.64.0.3:7681/#k=...     # 配对，一次双向生效
keepane link list                                  # 配过对的机器
keepane link panes 100.64.0.3:7681                 # 对方的 pane
keepane send-message --to 100.64.0.3:7681/%worker "run the tests"
```

对方的 pane 写成 `主机:端口/` 加上它在那台机器上的名字（`%worker`、`$1:@3.%7`）。消息到了那边，信封里的 `from=` 是这台机器和发送的 pane，所以那边用 `send-message -r` 回信就直接回到这里，并且归到同一个任务里。两边都要开着 `keepane web`；web 密钥只在配对那一次用到，之后每个请求都用 server 自己的密钥签名，所以 web 换了密钥也不影响。认的是公钥，不是地址：对方换个网络出现（从局域网换到 Tailscale），还是同一台机器。两边的时钟相差不能超过两分钟。

配过对的机器发来的消息只进 `ai` 和 `normal` 模式的 pane。要让它们在 `shell` 模式的 pane 里当命令执行，得在 keepane 之外的终端里给那台机器授权（不能在 pane 里面做，否则 agent 就能自己开权限）：

```powershell
keepane link allow 100.64.0.3:7681 --shell     # --no-shell 收回
keepane link allow 100.64.0.3:7681 --screen    # 允许它读这边 pane 的屏幕；--no-screen 收回
keepane link allow 100.64.0.3:7681 --panes     # 允许它在这边开 pane；--no-panes 收回
keepane link allow 100.64.0.3:7681 --push      # 允许它往这边的仓库送提交（clone、push）；--no-push 收回
keepane link remove 100.64.0.3:7681            # 解除配对，两边一起
keepane link trust 100.64.0.3:7681 <公钥>      # 手工加一台，公钥是它那边 `keepane link id` 打出来的
keepane link rekey                             # 换一对新密钥：所有配对都要重做
```

除了对方的 pane 列表，还能问到：

```powershell
keepane link info 100.64.0.3:7681              # 对方机器：主机名、系统、keepane 版本、开机多久、CPU、内存、pane 数
keepane trace-message 12 -w 60                 # 发过去的消息在那边怎么样了（shell 命令还有输出）
keepane link capture -S 100 100.64.0.3:7681/%worker   # 对方某个 pane 现在显示的内容（要对方授权 --screen）
```

对方授权了 `--panes` 的话，还能在对方那台电脑上开 pane：

```powershell
keepane link start 100.64.0.3:7681 -n helper -- claude   # 返回它在那边的地址
keepane send-message --to 100.64.0.3:7681/%helper "look at the failing test"
keepane link kill 100.64.0.3:7681/%helper               # 只能关这台机器在那边开的 pane
```

程序必须在对方的 `agent-commands` 白名单里，参数不限：白名单里要是有 shell（`pwsh`、`bash` 等），它收到什么就执行什么，这时 `--panes` 等于同时给了执行命令的权限；只想让对方开 agent 的话，在那台电脑上 `set -g agent-commands "claude codex"`（`link allow --panes` 的回复会列出当前白名单）。数量受对方的 `agent-pane-limit` 限制（按这台机器单独计数）。开出来的 pane 放在以这台机器命名的 session 里（`-s` 可以指定别的），工作模式和 agent 开的 pane 一样按程序自动定（`-m` 可以指定）；往那边 `shell` 模式的 pane 发命令仍然需要 `--shell` 授权。对方只在它的 server 运行期间记得哪些 pane 是这台机器开的。agent 也能通过 MCP 做同样的事：`create_remote_pane`，以及用地址调用 `kill_pane`。

没授权的机器发给 `shell` pane 的消息会被拒收，发送方会看到怎么授权。`keepane web --read-only` 的机器也不收别的机器的消息。对方连不上时立刻报错，不会在本机排队等以后再发；加 `-w` 时由对方等到投递为止，时间到了还在排队就算超时（和发给本机 pane 一样，退出码非 0）。配对和解除配对会在状态栏提示并记进事件日志；被拒的请求也一样，但同一个地址一分钟只记一次，有人大量乱发时不会刷屏，也不会把事件日志写满。设计和每条决定见 [docs/design/link.md](docs/design/link.md)。

### 把提交送到另一台电脑

对方授权了 `--push` 的话，可以把本机仓库的提交送过去：单向，只送已提交的内容。在仓库目录里运行：

```powershell
keepane clone 100.64.0.3:7681                  # 在对方建副本，放在它的 link-clone-dir 下（~/keepane/<本机仓库目录名>）
keepane clone 100.64.0.3:7681 ~/src/proj       # 或放到对方指定路径（必须不存在，或是空目录）
keepane push                                   # 以后：把当前分支的新提交送到上次的位置
keepane push 100.64.0.3:7681 ~/src/proj        # 送到对方已有的仓库（必须是同一个项目），以后记住这个位置
keepane push --tell %worker                    # 送到后再给那边这个 pane 发一条消息
keepane push -l                                # 本机各仓库送到了哪里
```

本机按对方的公钥记下每个仓库送到了哪里，对方换了地址（换了网络）也照样找得到。`keepane push` 不写机器时，送到这个仓库去过的那台；去过多台时要写明哪台。只送对方没有的提交：打成 git bundle，分块传过去，对方核对整个文件的 SHA-256 后才交给 git。提交在对方放进 `<本机名>/<分支>`；不会丢掉对方任何东西时，分支也跟着前移：能快进，并且分支是当前检出的、已跟踪的文件没有改动时，文件一起更新。对方在这个分支上有自己的提交，或者有改动的文件时，分支保持不动，回复里说明原因，由那边自己合并或变基。路径按对方机器解释（`~` 是对方的家目录）。送到对方已有的仓库时，两边至少要有一个相同的根提交（第一个提交），否则拒绝；对方仓库还没有提交时不受这条限制。浅克隆的仓库不能送（它没有早期历史），会提示先 `git fetch --unshallow`；Git LFS 只送指针文件，不送内容。本机未提交的文件不会送，回复里会写出个数。对方的状态栏会提示收到了什么，事件日志也会记一笔。`link-push-limit`（默认 200M）限制一次的大小，两边各自检查；两边都要装 git。agent 可以通过 MCP 的 `push_repo` 做同样的事。设计见 [docs/design/link-push.md](docs/design/link-push.md)。

## 手册

`keepane man` 以 Markdown 格式打印完整手册（所有命令、默认按键、配置、环境变量和文件位置），内容就是 [docs/keepane.1.md](docs/keepane.1.md)，可以直接阅读，也可以交给程序或 AI 去读。`keepane man --roff` 把它输出成 man 页面；用 Homebrew 安装会自动装好，所以可以直接用 `man keepane` 查看。

## 配置

keepane 只读自己的配置，不会去读 `~/.tmux.conf`。想沿用 tmux 的配置，就手动导入一次：

```bash
keepane import-config -n     # 先看看会写进去什么，不改任何文件
keepane import-config        # 从 ~/.tmux.conf（或 ~/.config/tmux/tmux.conf）导入
keepane import-config some.conf -o ~/.keepane.conf   # 任何配置文件，写到指定位置
```

每一行都按 keepane 启动时读配置的方式试一遍。能用的原样写进去（续行和 `%if` 块保持原来的结构，每个分支都检验过）；用不了的以注释形式写进去，并注明原因：TPM 和所有 `@` 开头的插件设置（tmux 的插件在 keepane 里不会运行）、tmux 有而 keepane 用不上的选项（`escape-time`、`mode-keys` 这些）、给 `MouseDragEnd1Pane` 这类鼠标“键”的绑定（keepane 的鼠标行为是固定的）、会执行程序的命令（`run`、`if-shell`：导入时什么都不执行），以及 keepane 不接受的写法。导入写进 keepane 正在读的那个配置文件（一个都没有就新建 `~/.keepane.conf`），并告诉你写到了哪里；同一个文件导入过一次，再导入会被拒绝。已经在运行的 server 只在启动时读配置，用 `keepane source-file ~/.keepane.conf` 可以立刻生效。

配置文件是 `~/.keepane.conf`（也可以放 `~/.config/keepane/keepane.conf`，或者用 `KEEPANE_CONFIG` 环境变量指定；Windows 上 `~` 就是 `%USERPROFILE%`），一行一条命令，就是 tmux 那种写法：

```tmux
set -g prefix C-a
set -g default-shell wsl          # Windows：pwsh（默认）、powershell、wsl、cmd 或可执行文件路径；其他系统：$SHELL（默认）或路径
# set -g default-command "wsl.exe -d Ubuntu"
set -g mouse on
set -g history-limit 10000
set -g status-position top
set -g status-style fg=black,bg=colour39
set -g pane-active-border-style fg=colour39
set -g base-index 1               # 窗口和 pane 都从 1 开始编号
set -g pane-base-index 1
set -g repeat-time 500            # `bind -r` 的键在多久之内还能接着按；0 就是关掉
set -g prefix-hint on             # 前缀之后迟迟没按键，弹出按键提示面板
set -g prefix-hint-delay 500      # 等多久才弹，毫秒（0：立刻）

set -g remain-on-exit on          # 程序退出后 pane 留着，告诉你它是怎么没的
set -g save-history 500           # 每个 pane 存多少行给 `resume` 用；0 就是不存，all 是整段 scrollback
set -g monitor-activity on        # 后台窗口有输出就在状态栏标 `#`
set -g monitor-bell on            # 响铃标 `!`；默认就是开的
set -g monitor-silence 60         # 60 秒没动静标 `~`；0 是关掉
set -g visual-bell on             # 用状态栏提示代替真的响铃
set -g agent-cost off             # 不下载价目表，pane 里的 agent 不显示花费（默认开）
set -g git-command C:\Tools\git\cmd\git.exe  # 读仓库信息用的 git（默认：自动找）
set -g done-after 60              # 命令跑满一分钟才通知（见"pane 完成时通知你"）

set -g pane-timestamps on         # 每条命令的时间显示在行尾（prefix C-t 切换）
set -g log-history on             # 输出存盘，每个 pane 每天一个文件（prefix / 翻看）
set -g log-history-days 30        # 留几天；0 永久保留（log-history-dir 改存放位置）
set -g undo-kill-time 10          # 关掉的 pane / 窗口保留几秒可以找回（prefix u）；0 不保留
set -g keep-zoom off              # 切到别的 pane 就取消最大化，和 tmux 一样（on：最大化跟着走）
set -g pane-border-lines single   # pane 之间的分隔线：heavy（默认，粗线）、single、double 或 simple（ASCII）
set -g pane-frames on             # 每个 pane 一个圆角边框，pane-border-format 的文字写在上边框（默认关：边框四周各占 1 格）
set -g animation off              # 不要焦点框飞过去的动画（animation-time 160：毫秒数，0 到 10000）
set -g clock-mode-animation bars  # 时钟下方：pulse、matrix、bars、fire、starfield、sharks、bonsai、sea、cat、dino、penguins、synthwave、fireworks、random（默认）、cycle 或 off
set -g clock-mode-look dots       # 用 dots、blocks 或 binary 字符画；plain（默认）用动画自己的字符
set -g clock-mode-zones "Asia/Shanghai,America/Los_Angeles"  # 时钟日期下方显示其他地方的时间（IANA 名称，也可以只写城市：tokyo）

bind | split-window -h
bind - split-window -v
bind -r C-h resize-pane -L 5      # -r：按一次前缀之后可以连着按
bind -n M-Left previous-window     # -n：不用按前缀
bind -n M-Right next-window
bind r source-file ~/.keepane.conf

# 选项名和命令名一样可以缩写，只要不产生歧义：
# set sync          = set synchronize-panes（不给值就是切换）
# set mon-act on    = set monitor-activity on（按 - 分段各写前缀）
# set mou           = 翻转 mouse
# set mon           会报歧义，并列出三个 monitor-* 让你选

set -ag status-right " | keepane"    # -a 是往原值后面追加，不是覆盖
source-file ~/.keepane/themes/nord.conf
```

keepane 默认是纸上写字的样子（`paper-dark`）：pane 用 dfine.tech 夜间代码块的配色（Vitesse Dark，底色是偏暖的近黑），状态栏是淡墨色，全屏只有一种强调色朱砂，用在会话名色块、当前窗口和当前 pane 的描边上。仓库里的 `themes/` 还放了几套现成配色（Nord、Gruvbox dark、Dracula、Catppuccin Mocha、Tokyo Night 和 Tokyo Day），Paper Dark 本身也有一份方便改，另外 `plain.conf` 是 tmux 那种朴素的绿色状态栏。它们就是普通的 keepane 命令文件，`source-file` 一下就行，想改直接改：

```tmux
source-file ~/.keepane/themes/dracula.conf
source-file ~/.keepane/themes/plain.conf   # 换回 tmux 的样子
```

内置四套主题，一个选项就能切换（手机页面上也能切）：默认的 `paper-dark` 和它的浅色版 `paper`（白纸，像 dfine.tech 白天的代码块）；0.37 及以前的默认 `tokyo-night` 和它的浅色版 `tokyo-day`。

```tmux
set -g theme paper
```

keepane 是画在你运行它的那个终端里的，所以用 `tokyo-night` 时，pane 的背景色和程序用的颜色（红、蓝等）都是终端自己的。另外三套自己画这些颜色，在哪个终端里看都一样：它们设置 `window-style`（pane 的默认文字色和背景色，和 tmux 一样）和 `pane-colours`（程序用的 16 种颜色，也就是调色板的前 16 个）。它们的文字颜色（黑色除外，程序一般把黑色用在色块上）在 pane 和状态栏上的对比度都不低于 4.5:1。主题会同时设置这些颜色和状态栏、边框，所以先设主题，自己的改动写在它后面。这些选项只要有一个用别的方式改过（加载主题文件、手动设置），外观就不再是哪套内置主题，`show -gv theme` 会显示 `custom`。想在别的主题下让 pane 用终端自己的颜色：`set -g window-style default` 加 `set -g pane-colours ""`。

默认外观用的是 24 位真彩色。在不支持真彩色的终端上（macOS 自带的 Terminal，除非它声明了 `COLORTERM=truecolor`），keepane 会自动换成 256 色里最接近的颜色，keepane 自己的界面和 pane 里程序的输出都一样。

`.tmux.conf` 里常见但 keepane 用不上的选项（`escape-time`、`focus-events` 这些）如果还是被设置了（比如用 `source-file` 读了一份 tmux 配置），会被接受然后忽略；`import-config` 不会导入它们。`default-terminal`（不设就是 `xterm-256color`）是 Linux 和 macOS 上 pane 拿到的 `TERM`；Windows 上终端由 ConPTY 安排，这个选项不起作用。

### 环境变量

`set-environment 名字 值` 给每个 pane 拿到的环境变量加一个或改一个，`set-environment -u 名字` 去掉它。之后新开的 pane 一启动就有。已经开着的 shell 在下一次显示提示符时拿到这个变化（PowerShell、bash、zsh、fish 和 `wsl.exe` pane 里的 shell，通过 keepane 的提示符钩子），之后它运行的程序都有。已经在跑的程序还是启动时的环境：一个程序改不了另一个程序的环境，要新值就重启它（`respawn-pane -k`）。shell 自己在启动后（或这个名字上次变化后）设的值不会被覆盖。这张表和会话存在一起，服务器重启后还在；`~/.keepane.conf` 先读，它设的名字以它为准。

```tmux
set-environment GOPROXY https://goproxy.cn    # 每个 pane 从下一次提示符起都有
set-environment -u GOPROXY                    # 再去掉
```

`prefix e` 用彩色显示：先是 `set-environment` 设的变量，再是 keepane 自己在每个 pane 里设的（`KEEPANE`、这个 pane 自己的 `KEEPANE_PANE`、带 keepane 目录的 `PATH` 等）。脚本用 `keepane show-environment` 拿到普通的 `名字=值` 行。

所有窗口和 pane 命令都支持 tmux 风格的 `-t`：`session`、`session:window`、`:window`、`session:window.pane`，窗口那一段可以是编号、名字、`+`、`-` 或 `!`。`%N` 是按编号指定 pane（`list-panes` 里显示的那个），别的 pane 增减时它指的还是同一个；`list-panes -F` 按格式串逐个 pane 输出。

### 状态栏

`set -g pane-border-status top`（或 `bottom`）会在每个 pane 的边框上放一行 `pane-border-format` 的内容，默认是 pane 编号和标题，当前 pane 加粗。

`status-left`、`status-right`、`window-status-format`、`window-status-current-format`、`pane-border-format` 接受 tmux 的格式串：`#S` session 名，`#W` 窗口名，`#I` 窗口编号，`#P` pane 编号，`#T` pane 标题，`#H` 主机名，`#F` 标记，`#{session_name}` 这种长写法，`#{?条件,真,假}` 条件（条件可以是变量名，也可以是 `变量==值` / `变量!=值`），`%H:%M` 之类的时间字段，`#[fg=colour39,bg=black,bold]` 改样式，还有 `#(命令)`：每隔 `status-interval` 秒（默认 15）跑一次，取输出的第一行（`display-message -p`、`jobs -F` 这种一次性命令会当场跑，最多等 3 秒）。`status-left-length` / `status-right-length` 限制长度。`status-justify left|centre|right|absolute-centre` 决定窗口列表放在哪，`window-status-separator` 是窗口标签之间的分隔（默认一个空格）。窗口多到放不下时，右端保持完整，当前窗口和它周围的窗口占用剩下的位置，两端有空位时用 `<`、`>` 标出有窗口被省略（同 tmux），被省略的窗口里有提醒时，那一侧的标记反色显示；只有右端长到放不下当前窗口时，右端才让出位置，而且只让出当前窗口和标记需要的宽度，从开头截断并标上 `…`，末尾的时钟保留。

```tmux
set -g status-right "#[fg=yellow]#(pwsh -NoProfile -c (Get-Date).ToString('HH:mm'))#[default] #H"
```

可用变量按类别列出：

- session：`session_name` `session_id` `session_windows` `session_attached` `session_created`
- window：`window_name` `window_id` `window_index` `window_panes` `window_active` `window_last_flag` `window_zoomed_flag` `window_width` `window_height` `window_bell_flag` `window_activity_flag` `window_silence_flag` `window_flags`
- pane：`pane_index` `pane_id` `pane_title` `pane_current_command` `pane_start_command` `pane_current_path` `pane_width` `pane_height` `pane_active` `pane_dead` `pane_dead_status` `alternate_on` `mouse_any_flag` `pane_synchronized` `pane_in_mode` `pane_passthrough` `pane_pid` `pane_start_time` `pane_activity` `pane_dead_time` `pane_last` `pane_mode` `pane_top` `pane_left` `pane_bottom` `pane_right` `pane_at_top` `pane_at_bottom` `pane_at_left` `pane_at_right` `cursor_x` `cursor_y` `history_size` `history_limit`
- client：`client_width` `client_height` `client_name` `client_session` `client_created` `client_activity` `client_prefix`
- server：`host` `host_short` `socket_path` `version` `pid`。另外还有 `session_activity` `session_last_attached` `window_activity` `window_start_flag` `window_end_flag` `window_layout`。

系统信息直接从进程读取，无需 `#(命令)`：`cpu_percentage` `ram_percentage` `ram_used` `battery_percentage`（没电池就是空）`battery_charging` `uptime`。其他常用变量：`git_branch`（pane 所在目录的分支，读 `.git` 得来，不在仓库里就是空；`git_marks` 等其他 `git_*` 见 [pane 所在仓库的 git 信息](#pane-所在仓库的-git-信息)）、`pane_current_path_short`（家目录写成 `~`）、`pane_pid_command`（pane 里此刻在跑的程序，编译时是 `cargo`）、`pane_output_count`（pane 输出过多少次；脚本比较前后两次的值就知道有没有新输出，只精确到秒的 `pane_activity` 做不到）。`keepane_update` 是每日检查发现的新版本号。`agent_*` 变量说的是 pane 里跑的 agent（见 [agent 用了多少、花了多少](#agent-用了多少花了多少)）。

网络：`local_ip` 是本机上网所用的地址（向系统查询，不发送任何数据），`public_ip` 是外网看到的地址。后者只有外部服务知道，所以只有当某个正在显示的格式用到 `#{public_ip}` 时，keepane 才会去问（`api.ipify.org`，每 10 分钟一次）；没用到就不会问任何人。默认外观里两者都没有，想显示就加上：

```tmux
set -ag status-right " #{local_ip} #{public_ip}"
```

默认的 `status-right` 用的是系统信息那几个变量（完整写法见 `themes/paper-dark.conf`，它就是默认外观；tmux 那种朴素的见 `themes/plain.conf`）；`set -g status-right ...` 整条换掉，`set -g status off` 整行关掉（之后提示和消息临时显示在最底下一行）。

比较运算与 tmux 一样：`#{==:a,b}` `#{!=:a,b}` `#{<:a,b}` `#{>:a,b}` `#{<=:a,b}` `#{>=:a,b}` `#{&&:a,b}` `#{||:a,b}`、`#{m:通配符,文本}`（`m/i:` 忽略大小写）得到 `1` 或 `0`，可以做 `#{?…}` 的条件，也可以做配置文件里 `%if` 的条件。

修饰符也与 tmux 一样：`#{=10:pane_title}` 取前 10 个字符，`#{=-10:…}` 取后 10 个，`#{b:pane_current_path}` 取文件名部分，`#{d:…}` 取目录部分，`#{t:session_created}` 把时间戳显示成时间，`#{s/foo/bar/:…}` 替换，可以套着用（`#{=8:b:pane_current_path}`）。

## 插件

插件结构与 tmux 类似：一个目录，里面放一个 `<名字>.keepane`（或 `plugin.keepane`）写 keepane 命令，再放上它需要的脚本，什么语言都行。脚本要和 keepane 说话就调命令行：环境变量 `KEEPANE` 是 socket 名，`KEEPANE_PANE` 是所在 pane，所以脚本里写 `keepane -L $env:KEEPANE display-message ...` 就能找到对的 server。

```tmux
# ~/.keepane.conf
set -g plugin-path ~/.keepane/plugins       # 默认就是这里
set -g @plugin demo                       # 加载 ~/.keepane/plugins/demo/demo.keepane
set -g @plugin C:\src\my-plugin           # 也可以直接给路径（目录或文件）
```

插件文件里能用配置文件的全部命令，再加上：

- `run-shell [-b] [-t target] 命令`：通过 pwsh 跑一条命令，带着 keepane 的环境变量，跑完把输出显示出来（`-b` 就不管输出了）。
- `set-hook -g <钩子> <命令>`：某件事发生时执行一条命令。钩子有 `after-new-session`、`after-new-window`、`after-split-window`、`after-select-window`、`after-select-pane`、`after-kill-pane`、`client-attached`、`client-detached`、`pane-exited`、`pane-done`（见"pane 完成时通知你"）。`set-hook -gu <钩子>` 取消，`show-hooks` 查看。
- `set -g @随便什么 值` 存一个插件自己用的选项，`show-options -gqv @随便什么` 读回来（脚本里：`keepane -L $env:KEEPANE show-options -gqv @随便什么`）。
- 状态栏里的 `#(命令)`，见上面。
- 运行时 `load-plugin 名字或路径`、`list-plugins`。

以下插件在状态栏显示 agent 任务进度，并用 `prefix A` 打开日志：

```tmux
# ~/.keepane/plugins/agent-status/agent-status.keepane
set -g status-right "#[fg=cyan]#(pwsh -NoProfile -File ~/.keepane/plugins/agent-status/summary.ps1)#[default] %H:%M"
set -g status-interval 5
bind A run-shell "pwsh -NoProfile -Command Get-Content $env:TEMP\agent.log -Tail 30"
```

脚本和按键绑定中还可使用这些命令（`keepane list-commands` 会全部列出来，命令名写前缀就行）：

- `pipe-pane [-o] [-I] [-O] [-t 目标] [命令]`：把 pane 打印的所有东西灌进一个命令的标准输入（`-O`，默认）；不给命令就是停。`keepane pipe-pane "$input | Add-Content build.log"` 就能一边编译一边留日志（PowerShell 会先把输入读完再跑，所以这个文件是管道停下时才写；想逐行落盘用 `cmd.exe /c findstr ... > 文件` 这种命令）。`-I` 反过来：命令打印什么就往 pane 里敲什么，命令输出完管道就结束（`-IO` 两个方向都要）。
- `wait-for [-L|-U|-S] 通道`：挂在那儿等别人发信号（或者解锁），两个脚本可以互相等：一边 `keepane wait-for ready`，另一边 `keepane wait-for -S ready` 放行。
- `wait-output [-t pane] [-a] [-i] [-w 时长] 文字...`：等到这个 pane 打出一行含其中任一段文字，打印这一行，退出码 0。比如先起开发服务器，等它开始监听再跑测试，不用反复 `capture-pane`：

  ```sh
  keepane send-keys -t dev 'npm run dev' Enter
  keepane wait-output -t dev -w 2m 'Local:' 'error' && npm test
  ```

  只算光标之后新出现的内容（开始等之后才敲进去的命令也算，因为它显示在 pane 里）；`-a` 把 pane 里已有的内容也算上，全屏程序则按它当前的屏幕算。`-i` 不分大小写；`-w` 到时间就放弃（`90`、`2m`、`1h`），退出码 1，pane 里的程序结束或 pane 被关掉时也一样。agent 用 MCP 工具 `wait_output`。
- `display-menu [-T 标题] 名字 键 命令 ...`：在窗口上弹个菜单，名字给空字符串就是一条分隔线。`display-popup [-E] [-w 宽] [-h 高] [-x 列] [-y 行] [-d 目录] [命令]` 是在窗口上开个小框跑程序（`-E` 程序退出就关，`-C` 从外面关掉；`-x` / `-y` 可以是列号/行号、百分比、`C` 居中、`R` / `B` 贴右边/底边，不给就居中）。
- `choose-client`：列出连着的客户端，选一个踢下线。
- `send-keys -X <copy 命令>`：用脚本开 copy mode 干活（`search-backward`、`begin-selection`、`copy-selection` ……名字和 tmux 一样）。
- `capture-pane -p [-e] [-J] [-S -N]`：把 pane 的内容打出来，`-e` 连颜色一起，`-J` 把被折行的长行拼回一行，`-S -N` 带上 N 行回滚（`-S -` 全部）。
- `find-text 关键词`：在所有 pane 打印过的内容里找，告诉你在哪个 pane、往回第几行：`ft:0.0  -8  REDIS-TIMEOUT-here`。`-C` 区分大小写，`-t` 限定 session 或窗口，`-n` 限制每个 pane 最多几条，`-H` 连历史日志的每一天也一起搜（每条带上日期和行号）。keepane 的 `find-window` 只搜窗口名和标题。
- `jobs`：任务板。整个 server 上每个 pane 一行：程序还在跑还是已经退出（退出码多少）、跑了多久、多久没有输出、pid、命令、目录。`-t session` 只看一个 session；`-F 格式` 自己定输出（`#{pane_start_time}`、`#{pane_activity}`、`#{pane_dead_time}` 是原始时间戳）。`prefix B`（`choose-jobs`）是同一张板的可操作版：`Enter` 跳到那个 pane，`x` 杀掉，`r` 重启，开着的时候行会原地刷新。

  ```
  PANE       STATE    UP     IDLE   PID    COMMAND       DIR
  build:0.0  running  2h13m  4s     21608  cargo build   C:\src\keepane
  web:0.0    exit 1   2h13m  1h02m  28748  npm run dev   C:\src\site
  ```
- `record [-t 目标] out.cast`：从现在起把这个 pane 打印的一切写成 [asciinema](https://asciinema.org) v2 文件（连窗口尺寸变化一起）；`record -t 目标` 不带路径就是停。`asciinema play` 能回放，也能上传或嵌进网页。
- `notify [-T 标题] 消息`：弹一个 Windows 桌面通知（toast，通知中心里以 keepane 自己的名字出现）；`set -g notify on` 之后每次告警都弹，终端被别的窗口盖住时也能收到。告警的通知带一个“Go to pane”按钮：点了打开一个 `keepane://` 链接，执行 `focus-pane`，让所有接着的客户端切到那个 pane 并把窗口提到前面（尽力而为：Windows Terminal 不一定允许别的进程把它拉到前台）。第一次弹 toast 会在 `HKCU\Software\Classes` 下登记两个键（AppUserModelID 和 `keepane:` 协议），不碰系统范围；弹不出 toast 时退回托盘气泡。
- `focus-pane %N`：单独用这条命令，所有接着的客户端都切到 pane `%N`（`list-panes` 和 `#{pane_id}` 显示的那个 id）。

窗口和 pane 相关的命令都接受 `-t 目标`，写法和 tmux 一样：`session`、`session:窗口`、`:窗口`、`session:窗口.pane`，窗口那段可以是编号、名字，也可以是 `+`、`-`、`!`。只有 `select-pane` 例外，它的 `-t` 跟的是要跳到哪个 pane（`next`、`last` 或编号），和 `-L` `-R` `-U` `-D` 是一类。

## 它是怎么工作的

`keepane` 命令本身是客户端。首次运行会启动后台 server（`keepane __server`），由 server 管理所有 session。两者通过按用户隔离的通道通信：Windows 上是命名管道（`\\.\pipe\keepane-<用户名>-<socket>`），其他系统上是 Unix socket（`keepane-<uid>/<socket>`，位置见“安装”）；用 `-L` 指定 socket。

每个 pane 使用一个伪终端（Windows 上是 ConPTY）。server 用 `vt100` 维护终端画面，将可见 pane、边框和状态栏合成一帧，只向客户端发送变化的格子；客户端再用 VT 序列写入控制台。最后一个 session 结束后，server 退出。Windows 上，server 启动时会尽可能脱离创建它的 job，避免从 SSH 启动时随连接断开而退出；Linux 和 macOS 上，server 自成一个会话，终端或 SSH 连接挂断的信号到不了它。

pane 里能看到两个环境变量：`KEEPANE`（socket 名）和 `KEEPANE_PANE`（pane 编号）。在 pane 里敲 `keepane` 命令会自动连到管着这个 pane 的 server（和 tmux 用 `$TMUX` 一个道理），所以 `keepane ls` 之类不用再写 `-L`。server 日志位于 `%LOCALAPPDATA%\keepane\server.log`，`KEEPANE_LOG=debug` 会记得更详细。超过 5 MB 会改名为 `server.log.1` 再开新文件，跑几个月的 server 日志也最多占 10 MB 左右。

如果按键没有响应（例如前缀键），可在同一终端运行 `keepane show-keys` 再按它：每按一个键，会打印控制台交过来的是什么、keepane 把它认成哪个键，按 `q` 退出。什么都没打印，说明是外面托管终端的程序自己把键吃掉了（比如 VS Code 自己绑了 `Ctrl+B`）。有些宿主把输入当字节转交而不是键盘事件（SSH、一些远程工具），`Ctrl+B` 到这里只是字符 0x02、不带 Ctrl 标志；keepane 按 tmux 的读法解读控制字符，所以它照样是 `C-b`。

命名管道带了只允许当前用户（和 SYSTEM）访问的 DACL，相当于 tmux 那个 0700 的 socket 目录（Unix socket 用的就是这样的目录）。Windows 上每个 pane 都跑在一个 kill-on-close 的 job object 里，其他系统上 pane 的整个会话会被一起挂断，所以 `kill-pane`、`kill-session`、server 退出都会把整棵进程树带走，不留孤儿。客户端写控制台慢的时候，server 不会无限缓冲帧，而是直接改成全量重绘。

`vendor/vt100` 是 vt100 0.16.2 加了一处修复：pane 缩小时刚好切到一个宽字符（比如中文）会越界，详见 `vendor/vt100/KEEPANE-PATCH.md`。server 里任何一条命令 panic 都会记到日志里然后继续跑，不会把 session 全丢了。

## 开发

```bash
cargo test              # 单元测试 + 端到端测试（Windows 上拉起 cmd.exe，其他系统上拉起 sh）
cargo clippy --all-targets
```

CI 在 Windows、Linux、macOS 上都跑这两步。平台相关的代码在 `src/platform/windows` 和 `src/platform/unix`，两边模块一一对应（见 `docs/design/platform.md`）。Windows 上 `tests/console.rs` 会把真的 `keepane.exe` 塞进一个 ConPTY 里跑，所以控制台那条路（raw 模式、备用屏幕、脱离时的清理）不用人坐在键盘前也能测到。

网页有自己的浏览器测试（`web/tests`，用 [e2e](https://github.com/tester-army/e2e) 跑），见 `web/README.md`。

## 还没做的

Linux 和 macOS 上暂时没有 `keepane update`（只提示怎么升级，用 Homebrew 装的就提示 `brew upgrade keepane`）和带“跳到 pane”按钮的桌面通知。

与 tmux 相比，目前有这些差异：

- 多个客户端连接同一 session 时，共享窗口尺寸。`window-size latest|smallest|largest|manual` 决定采用最近使用、最小、最大客户端的尺寸，或只接受 `resize-window` 手动设置。较小的客户端显示自己的视口，可用 `Shift`+方向键（`refresh-client -U/-D/-L/-R`）平移；输入时视口跟随光标。
- hook 仅支持前文列出的事件；`choose-tree` 按子串过滤，不支持 tmux 格式串过滤。
- `display-popup` 中，前缀键仍归 keepane 处理；连按两次可将前缀键发送给弹窗中的程序。

pane 消息：`shell` 工作模式依赖 keepane 的提示符钩子（PowerShell、bash、zsh、fish，以及 `wsl.exe` pane 里的这些 shell），所以 cmd、sh 暂时不会自己接收消息（可以用 `read-message` 取）；keepane 0.15 之前启动的 pane 用的是旧钩子，要重开才行。手机页面能看每个 pane 的收件箱，dashboard 里的任务和事件还没有上手机页面。

命令和按键逐条对照见 `docs/tmux-parity.md`。

## 原名 wmux

0.13.1 及以前的版本名为 wmux。由于 GitHub、winget 和 crates.io 上已有同名项目，0.14.0 起更名为 keepane。名字取自 keep + pane：终端断开后，pane 中的程序继续运行。

从 wmux 过来：

- `keepane migrate` 一次搬完：还在运行的 wmux 服务端里的会话（先存盘、停掉旧服务端，再在 keepane 里恢复，布局、历史、目录都在；里面的程序会重新启动，和 `restart-server` 一样），wmux 存在 `%LOCALAPPDATA%\wmux` 下的东西（会话存档、历史记录、手机端密钥），开机启动、Windows Terminal 的 profile、通知链接。如果 keepane 里已经有同名会话，wmux 的那个会以 `<名字>-wmux` 恢复在旁边，两个都保留。wmux 服务端还在运行时启动 keepane，会提示你这件事。
- 你的 `~/.wmux.conf` 照样生效，`WMUX_*` 环境变量、`~/.wmux/plugins` 和 `*.wmux` 插件文件也都认，想改名时再改成 `~/.keepane.conf`、`KEEPANE_*`、`~/.keepane/plugins`、`*.keepane`。
- MSI 会替换掉"应用和功能"里的 wmux。`wmux update` 已经找不到新版了，请从 Releases 手动装一次 keepane。用 scoop 的话：`scoop uninstall wmux`，再装 keepane 的清单（见“安装”一节）。
- 仓库搬到了 github.com/newdee/keepane（旧链接会跳转过去），网站在 dfine.tech/keepane。
- `$PROFILE` 里的 `tmux` 或 `wmux` 别名要改成指向 keepane。
