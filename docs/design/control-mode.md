# 控制模式：UnifyTerm 直接渲染 keepane 的 pane（设计稿）

状态：**暂不做**（2026-11-27）。用户定 keepane 不依赖任何终端；图片改由 keepane 自己保存，见 [kitty-graphics.md](kitty-graphics.md) 第 3 节。下面是当时的提议，留作记录。

原状态：提议（2026-11-27）。用户原话："keepane 如果检测到有 unifyterm，是否可以用 unifyterm 作为自己的输出终端，渲染都在上面？……还是说，unifyterm 和 keepane 之间定一种协议，通过这种协议实现相融"；"再写控制模式设计稿，最好给一个本地 html demo"。

演示：[control-mode-demo.html](control-mode-demo.html)（本地打开，模拟数据，不连真 server）。

## 1. 要解决的问题

现在 keepane 在 UnifyTerm 里的样子：

1. keepane server 为每个 pane 跑一个终端模型（`vt100`），只存字符格子。
2. keepane client 把合成好的整屏画给 UnifyTerm。UnifyTerm 只是一块"画布"。
3. 所以 UnifyTerm 的一部分功能到了 keepane 里用不上：kitty 图片（`vt100` 丢掉 APC）、每个 pane 的原生滚动和选择、命令标记、agent 侧栏对单个 pane 的理解。（更正：字体渲染、连字、配色、透明度在普通 attach 下照旧由 UnifyTerm 画，不在此列。）

目标：在 UnifyTerm 里连 keepane 时，**每个 pane 由 UnifyTerm 自己的终端核心渲染**；keepane 照旧保活、管理会话、跑 dashboard、手机网页、agent、群聊。

不在范围内：替代 keepane 自己的 client（别的终端照旧用它）；keepane 的终端模型（照旧需要，见第 4 节）。

## 2. 相似问题：tmux 控制模式（iTerm2 的 `tmux -CC`）

1. tmux 不画屏幕，输出一行一行的通知：`%output %3 <转义后的字节>`、`%layout-change`、`%window-add`、`%begin…%end`（包住命令回复）。
2. iTerm2 为每个 tmux pane 建原生的标签页或分屏，用自己的模拟器解析 `%output` 里的原始字节。
3. iTerm2 把按键翻成 tmux 命令（`send-keys`），窗口大小用 `refresh-client -C` 告诉 tmux。
4. tmux 仍为每个 pane 跑自己的模拟器，重连时 iTerm2 用 `capture-pane` 取现有屏幕。

keepane 的 link 设计（`docs/design/link.md` 第 17 条）当时就以它为参照。

## 3. 已有的东西（查过代码）

keepane 已经有一条"控制模式式"的流，给别的机器用：

| 部件 | 位置 | 内容 |
|---|---|---|
| 日志（journal） | `src/server/journal.rs` | 每个变化一个编号：形状（会话、窗口、布局、每个 pane 的位置、大小、标题、目录、模式）整个发；pane 的输出**按原样的字节**发（"别的机器喂给自己的终端模型，屏幕逐字节相同"）；尺寸变化；模型重建。最多留 8 MiB，落后太多的从快照（`Snapshot`）重来 |
| 编码 | `src/link_proto.rs` | protobuf（`prost`）：握手 `Hello`/`Answer`，之后 `ToClient`、`ToServer`；字段号永不复用，新旧版本互读 |
| 命令 | `ToServer::Command` | 副本可以让它分屏、关 pane、改会话尺寸（0.63 起） |
| 传输 | `link.md` 第 17 条第 2 阶段 | web 端口上 `GET /link/stream`，升级后是 Noise 加密的帧；只给配对过的机器 |

缺的只是：一条**本机终端**能用的传输，和 UnifyTerm 一侧的消费者。

## 4. 方案

### 4.1 传输：`keepane control`，子进程的标准输入输出（推荐）

UnifyTerm 起一个子进程 `keepane control [-t 会话]`，读它的 stdout、写它的 stdin：

- 和 `tmux -CC` 一样走进程管道，不开端口、不配对、不加密：只有同一个用户能起这个进程，keepane 的 socket 本来就只对本用户开放。
- 一个 UnifyTerm 窗口一个 `keepane control` 进程。它是 keepane server 的一个普通 client（跟 `keepane attach` 一样经本机 socket 连 server），只是说的是控制协议，不画屏幕。

另一个选择：UnifyTerm 直接连 `/link/stream`。要实现 Noise、配对、密钥，只在"连别的机器上的 keepane"时才值得，本机不需要（列为以后的可能）。

### 4.2 编码：复用 link 的 protobuf 消息（推荐），另给 `--json` 调试

| 选项 | 好处 | 坏处 |
|---|---|---|
| **复用 `link_proto`（长度前缀的 protobuf 帧）** | keepane 侧几乎不用新写：日志、快照、命令、回复、版本兼容都现成；字节原样，图片序列不丢 | UnifyTerm 要依赖 `prost`，并拿到消息定义（建议把 `link_proto` 拆成一个小 crate，两个仓库共用） |
| 照 tmux 控制模式的文本协议 | UnifyTerm 实现一次，真 tmux 也能连；iTerm2 理论上也能连 keepane | keepane 要另写一套：布局字符串、八进制转义、`%pause`/`%continue` 流控、`capture-pane` 补屏；keepane 的形状比 tmux 多（pane 名字、工作模式），表达不了 |
| 新的 JSON 行协议 | 好调试 | 第三套协议，没人复用；字节要 base64 |

推荐第一项；`keepane control --json` 把同样的消息写成 JSON 行，只给人看和测试用。兼容 tmux 文本协议列为以后可选（第 8 节）。

### 4.3 一次会话的过程

1. UnifyTerm 起 `keepane control -t main`，发 `Hello`（带它有的最新编号，第一次为空）。
2. keepane 回 `Answer`（版本、server 这次运行的 epoch），然后发 `Snapshot`：每个 pane 的屏幕（带颜色）和历史、整个形状。
3. UnifyTerm 按形状建标签页（keepane 的窗口）和分屏（布局），把每个 pane 的屏幕喂给它自己的终端核心。
4. 之后持续收日志：`Output(pane, bytes)` 直接喂给对应 pane 的终端核心；`Shape` 变了就调整标签页和分屏。
5. 按键、粘贴、鼠标：UnifyTerm 把 pane 应收的字节发回（新消息 `Input(pane, bytes)`，相当于 `send-keys -l`）。
6. UnifyTerm 里改窗口大小：发 `Resize(pane 或窗口, 列, 行)`。keepane 照它改 pane 的尺寸（tmux 的 `refresh-client -C`）。
7. UnifyTerm 里新建分屏、关 pane、改名：发 `Command`（现成的那条）。
8. 断开：UnifyTerm 关掉子进程；keepane 里会话照旧跑。再连时带上最后的编号，只补后来的（落后超过 8 MiB 才发快照）。

### 4.4 keepane 侧要做的

1. `keepane control` 命令：本机 client，讲 4.2 的帧；只看 `-t` 指定的会话（不给就是全部）。
2. 新消息：`Input`、`Resize`（按 pane 或窗口），和"这个 client 想看哪些会话"。
3. **尺寸由控制 client 决定**：现在一个会话的大小来自连着的 client 的窗口；控制 client 没有"整屏"，它给的是每个 pane 或每个窗口的大小。这是改动最大的一处，要和 `window-size`、手机的 `web-fit` 的规则排好先后。
4. 控制 client 不进状态栏、不收前缀键（键在 UnifyTerm 那边已经翻成字节）。
5. 文档、e2e：一个"假 UnifyTerm"测试程序，按协议连上、收快照、发输入、收输出、断开重连只补后来的。

### 4.5 UnifyTerm 侧要做的（复用它的终端核心）

1. **pane 的来源多一种**：现在每个 pane 背后是本机 PTY/ConPTY；加"背后是 keepane 的一个 pane"：字节从协议来，输入和尺寸回协议。终端核心本身不改。
2. 形状对应：keepane 的窗口 → 标签页，布局 → 分屏；pane 的名字、工作模式显示在标签或标题上。
3. 入口：一个命令（如 `ut keepane [会话]`）或菜单"连接 keepane 会话"。自动切换（像 iTerm2 看到 `tmux -CC` 的 DCS 就接管）放到后面。
4. 快照：把每个 pane 的屏幕和历史喂进终端核心。
5. 断开、重连、keepane server 重启（epoch 变了就从快照重来）。

### 4.6 要用户定的行为

| 问题 | 建议 |
|---|---|
| keepane 的前缀键 `C-b`、状态栏、复制模式在控制模式下怎么办 | 不用：UnifyTerm 的快捷键、标签栏、原生选择代替（iTerm2 配 tmux 也这样） |
| `C-b v` 的 dashboard | 在 UnifyTerm 里开成一个普通标签页（它本来就是一个程序） |
| 同一个会话同时被 keepane client 和 UnifyTerm 看 | 允许；尺寸按 4.4 第 3 条的规则（建议：最后有输入的那个说了算，和现在 `window-size latest` 一致） |
| 入口 | 先手动（`ut keepane`）；稳定后再考虑自动 |

## 5. 图片会怎样

- 实时输出里的 kitty 图片：字节原样到 UnifyTerm，由它解析，**能显示**。keepane 不用懂图片。
- 断开重连：日志还在（8 MiB 内）就补原始字节，图片也回来；超过了从快照重来，快照只有字符格子，图片没有。要补，keepane 得另存每个 pane 最近的图片序列（第 8 节）。
- 其他终端（Windows Terminal 等）用普通 `keepane attach`，不受影响；pane 里 `ut view` 画半格方块（0.98.1 的小修复）。

## 6. 香农六法

1. **简化**：只做"一个会话、一个 pane 一个标签页、没有分屏"，就能验证整条链路：快照、输出、输入、重连。
2. **相似问题**：tmux 控制模式（第 2 节）；keepane 自己的 link 副本（第 3 节），后者已经解决了"副本自己渲染、逐字节相同、断线补齐"。
3. **换说法**：不是"让 keepane 支持图片"，而是"让 UnifyTerm 成为 keepane 的另一种 client"。
4. **推广**：做成后，任何会讲这套帧的程序都能当 keepane 的渲染端（比如以后的 VS Code 扩展）；再加 tmux 文本协议，就能接 iTerm2。
5. **结构分析**：传输（4.1）、编码（4.2）、keepane 侧（4.4）、UnifyTerm 侧（4.5）四块互相独立，可以分开验收。
6. **逆向**：要"UnifyTerm 里的 keepane pane 和本机 pane 一样好用"，必须成立：字节原样到达、尺寸由 UnifyTerm 定、输入低延迟、重连不丢。第 4 节逐条对应。

## 7. 分步（每步单独验收、单独发版）

1. **keepane：`keepane control`**（只读）：快照加实时输出，`--json` 可读；e2e 用"假 UnifyTerm"验证快照和输出逐字节相同。
2. **keepane：输入和尺寸**：`Input`、`Resize`，尺寸规则（4.4 第 3 条）；e2e 验证打字、改大小。
3. **UnifyTerm：只读的 keepane pane**：一个 pane 一个标签页，显示快照和实时输出（含 kitty 图片）。
4. **UnifyTerm：输入、尺寸、分屏对应、重连**。
5. 以后可选：图片跨重连（keepane 存图片序列）；tmux 文本协议兼容；UnifyTerm 自动接管。

估计工作量（粗估，没拆到函数级，误差可能 ±50%）：keepane 第 1–2 步约 1200–2000 行（日志和编码现成，新的是 `control` client、`Input`/`Resize`、尺寸规则、测试）；UnifyTerm 第 3–4 步约 1500–2500 行。

## 8. 以后可能

- 每个 pane 存最近的图片序列，重连时补发（图片跨快照）。
- 兼容 tmux 控制模式的文本协议，iTerm2 和别的 tmux 客户端也能连 keepane。
- UnifyTerm 直接连别的机器上的 keepane（走 `/link/stream`，要 Noise 和配对）。

## 9. 风险

- 尺寸规则改动影响现有的多 client、手机 `web-fit`：要先写清优先级再动手。
- 两个仓库的协议版本：靠 protobuf 字段号永不复用；`Answer` 带版本，UnifyTerm 遇到太旧的 keepane 时说清楚。
- 控制 client 读得慢：沿用 link 的做法，积压超过上限就断开，让它重连补齐。
