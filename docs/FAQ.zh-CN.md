# keepane 常见问题

[English](FAQ.md) · [说明文档](../README.zh-CN.md) · [功能一览](https://dfine.tech/keepane/)

每一条都按"看到了什么、为什么、怎么办"来写。

- [Codex（或别的全屏程序）：往上翻不到之前的对话](#codex或别的全屏程序往上翻不到之前的对话)
- [agent 的 pane 一直不接消息，消息一直排队](#agent-的-pane-一直不接消息消息一直排队)
- [pane 一直显示忙，消息投递不进去](#pane-一直显示忙消息投递不进去)
- [怎么切换 pane 的工作模式？](#怎么切换-pane-的工作模式)
- [交换了两个 pane 的位置，消息会发到哪？](#交换了两个-pane-的位置消息会发到哪)
- [别的机器发来的消息被 shell 模式的 pane 拒收](#别的机器发来的消息被-shell-模式的-pane-拒收)
- [手机打不开页面](#手机打不开页面)
- [任务跑完时通知我的手机，锁屏也要收到](#任务跑完时通知我的手机锁屏也要收到)
- [飞书或钉钉机器人拒收 keepane 的消息](#飞书或钉钉机器人拒收-keepane-的消息)
- [Windows：hook 里的 keepane 路径加了引号就报错](#windowshook-里的-keepane-路径加了引号就报错)
- [macOS：有些目录提示 "Operation not permitted"](#macos有些目录提示-operation-not-permitted)

## Codex（或别的全屏程序）：往上翻不到之前的对话

**看到了什么。** 跑 Claude Code 的 pane 可以往上翻（`C-b [`、手机上往上划、`C-b /` 看当天的历史），整段对话都在；跑 Codex 的 pane，滚过去的内容就没了。

**为什么。** Codex 画在终端的"交替屏幕"（alternate screen）上，和 vim、htop 一样：它占一块自己的画面，原地整屏重画，内容从来不会滚进终端的历史。keepane 和 tmux、各种终端一样，只保存普通屏幕上滚出去的内容。Claude Code 是在普通屏幕上一行行往下输出的，所以它的内容会进历史。

**怎么办。** 让 Codex 在普通屏幕上运行：

```sh
codex --no-alt-screen
```

或者一劳永逸，在 `~/.codex/config.toml` 里加：

```toml
[tui]
alternate_screen = "never"
```

（Codex 的默认值 `auto` 只在 Zellij 里才自动关掉交替屏幕。）不想改配置的话，在 Codex 里按 `Ctrl+T` 可以打开它自己的完整对话记录。

Codex 的 issue 里有人报告，在普通屏幕上它有时仍会整屏重画，历史里会留下重复的内容（在 Zellij 和 VS Code 终端里出现过）。如果翻到的历史比较乱，那是 Codex 重画造成的，完整、干净的记录还是看它的 `Ctrl+T`。其他全屏程序也一样：程序自己不让内容滚出去，keepane 就没法保存。

## agent 的 pane 一直不接消息，消息一直排队

**看到了什么。** 给 `ai` 模式的 pane 发消息，`send-message` 回复 `queued ... (ai, busy, ...)`，之后一直不投递；dashboard 或手机上看得到它在排队。看起来像是 agent 忘了查收件箱。

**为什么。** agent 并不自己去读收件箱。它每轮结束时，由 hook 运行 `keepane pane-ready`，keepane 再把下一条消息当作提示词打进去。没有这个 hook，这个 pane 永远不算空闲。keepane 会直接说明：`send-message` 的回复多一行 `%N has not said it is free since its agent started ...`，dashboard 和手机上也会标出来。

**怎么办。**

```sh
keepane setup                    # 每个 agent：本机装没装、hook 配没配、MCP 注册没有
keepane setup codex --install    # 或 claude、gemini、cursor、opencode
```

改每个文件之前都会先备份，只加 keepane 自己的条目。装好后在 pane 里重新启动 agent。Codex 的新 hook 要你信任一次才会运行：在 Codex 里输入 `/hooks`，信任 keepane 的那两条。其他 agent 只要能在每轮结束时运行一条命令，让它运行 `keepane pane-ready -q` 就行。

## pane 一直显示忙，消息投递不进去

**看到了什么。** pane 里的 agent 或 shell 明明在等输入，keepane 却一直算它忙。

**为什么。** 往 pane 里打字会让它变成忙，直到下一个信号（shell 的提示符、agent 的 hook）出现，这样消息不会插进正在输入的一行中间。输入了又删掉，或者提示符没有重画，就可能一直等不到那个信号。

**怎么办。** 在这个 pane 外面（另一个终端，或者 `C-b :` 提示符）运行：

```sh
keepane pane-ready -t %agent
```

## 怎么切换 pane 的工作模式？

在那个 pane 里运行 `keepane set-work-mode ai`（或 `shell`、`normal`）。在 keepane 外面（普通终端、`C-b :` 提示符、按键绑定）可以指定 pane：`keepane set-work-mode -t %builder shell`。在一个 pane 里运行的命令改不了别的 pane 的模式，所以任何 pane 里的程序都没法把别的 pane 变成"收到什么就执行什么"的 shell。

## 交换了两个 pane 的位置，消息会发到哪？

pane 的编号（`%7`）和名字（`%builder`）跟着 pane 走，不管它被移动还是交换（`C-b {`、`C-b }`、`swap-pane`）。发给 `%7` 或 `%builder` 的消息，还是到原来那个 pane。位置写法（`work:0.1`）指的是现在在那个位置上的 pane。完整地址（`$1:@2.%7`）还写明了 pane 在哪：pane 被挪到别的窗口后，消息会被拒收，而不是发错地方。

## 别的机器发来的消息被 shell 模式的 pane 拒收

**为什么。** 配过对的机器发来的消息，默认只进 `ai` 和 `normal` 模式的 pane。shell 模式的 pane 会把消息当命令执行，所以要你为那台机器单独打开。

**怎么办。** 在接收消息的那台电脑上，在 keepane 外面的终端里运行（不能在 pane 里做，否则 agent 就能自己给自己开权限）：

```sh
keepane link allow 192.168.1.20:7681 --shell
```

加 `--screen` 则允许那台机器读取 pane 的屏幕（`keepane link capture`）。

## 手机打不开页面

- 手机和电脑要在同一个网络里，或者都连着 Tailscale。`keepane web status` 能看到正在服务的地址和谁连着。
- Windows 第一次会问是否允许 keepane 访问网络：选允许专用网络。
- 每次启动 `keepane web`，二维码都会换新（除非加了 `--keep-key`）：要扫新的。
- 用的是普通 HTTP，在家里的网络没问题；在外面用，中间接一层 Tailscale 之类的私有网络，不要直接把端口开到公网上。

## 任务跑完时通知我的手机，锁屏也要收到

手机页开着的时候会直接提醒你（横幅、响一声、安卓震动）。手机锁屏后浏览器会暂停页面，普通 HTTP 的页面也弹不了系统通知，所以锁屏也要收到的话，让 keepane 发到一个有 App 的服务上：

- **ntfy**（免费、开源，iOS 和安卓都有 App，也可以自己搭）：装好 App，订阅一个你自己起名的频道（ntfy.sh 上知道频道名的人都能看到内容：名字起得难猜一点，或者自己搭服务器），然后
  ```sh
  set -g done-webhook https://ntfy.sh/<你的频道>
  set -g done-webhook-format text
  ```
- **你已经在用的聊天工具**：飞书、企业微信、钉钉、Slack、Discord。在群里加一个"自定义机器人"（webhook），然后
  ```sh
  set -g done-webhook <机器人的地址>
  set -g done-webhook-format feishu    # 或 wecom、dingtalk、slack、discord
  ```
- **其他服务**：`pane-done` hook 会运行你指定的命令，这次完成的信息在 `KEEPANE_DONE_*` 环境变量里（见说明文档）。

哪些算"完成"由 `done-events` 决定（默认 `command agent`：跑满 `done-after` 秒（默认 30）的命令，以及 agent 这一轮结束），只对有名字或 `ai`/`shell` 模式的 pane 生效（`done-panes all` 是所有 pane）。想长期生效，把这几行写进配置文件。

## 飞书或钉钉机器人拒收 keepane 的消息

`show-messages` 里会看到 `done-webhook: refused (...)`，后面是对方的回复。取决于机器人的安全设置：

- **自定义关键词**：keepane 发到聊天工具的每条消息都以 `keepane` 开头，把 `keepane` 加为机器人的关键词即可。
- **签名校验**（飞书的"签名校验"、钉钉的"加签"）：keepane 不做签名，请关掉，改用关键词或 IP 白名单。
- **IP 白名单**：电脑访问外网用的那个地址要在白名单里。
## Windows：hook 里的 keepane 路径加了引号就报错

Windows 上 agent 可能用 PowerShell 执行 hook，在 PowerShell 里"带引号的路径后面跟参数"是语法错误。程序名不要加引号（`keepane pane-ready -q`，keepane 在 PATH 上），或者用调用运算符：`& "C:\路径\keepane.exe" pane-ready -q`。`keepane setup` 写的就是不带引号的写法。

## macOS：有些目录提示 "Operation not permitted"

**看到了什么。** 在"文稿"、"桌面"、"下载"这类受保护的目录里 `ls`，提示 `Operation not permitted`，有时是断开重连之后才出现。

**为什么。** macOS 的隐私保护（TCC）按终端程序决定它启动的程序能读哪些目录。keepane 的 server 和里面的每个 pane，用的都是启动它们的那个终端的权限。如果在 keepane 外面，iTerm2 或"终端"里同样的 `ls` 也失败，那就和 keepane 无关。

**怎么办。** 系统设置 → 隐私与安全性 → 完全磁盘访问权限（或"文件和文件夹"）：给你用的终端程序打开，然后完全退出再重新打开这个终端。改权限之前就启动的 keepane server 可能还是旧权限：`keepane kill-server`，在终端里重新启动，再用 `keepane resume` 把 session 恢复回来（恢复的是布局和目录，里面的程序会重新启动）。
