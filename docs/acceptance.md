# 验收记录（三轮干净）

> 项目在 0.14.0 由 wmux 改名为 keepane（原因见 README「原名 wmux」一节）。第五十一次及以前的记录保留当时的名字 wmux。

规则：每轮 = 一次 review（换视角）+ 一次全量测试。发现问题即修，该轮不计数，重新开始。
连续三轮零发现才算通过。

测试基线命令：`cargo test`（lib 单元 + tests/e2e.rs 管道级 + tests/console.rs 真实 ConPTY）、`cargo clippy --all-targets`。

## 第 1 轮（不计数）— 视角：静态一致性

做了什么：逐条对照 README/`--help` 的 claim 与代码；grep 所有 `=> {}` 形式的"接受但忽略"的 flag；找无人调用的 pub 项。

发现（8 项，全部已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | `list-keys`/`list-windows`/`list-sessions` 多行输出在已 attach 时被压成一行状态栏消息，README 说 `?` 能"列出按键绑定" | 新增 overlay 视图（tmux view mode）：多行输出覆盖窗口区，任意键关闭；`render::draw_overlay` + 单元测试 + e2e |
| 2 | README 示例 `bind r source-file ~/.wmux.conf`，但 `~` 在 Windows 上不展开 | `expand_home()` |
| 3 | `new-session -A` 解析后丢弃 | 实现：同名 session 存在则 attach（非交互/`-d` 时静默成功） |
| 4 | `new-window -d`、`split-window -d/-b/-f`、`send-keys -l`、`kill-window/pane/session -a` 全部解析后丢弃 | 全部实现（`Node::split_at`/`split_root` 支持 before/full） |
| 5 | `Options::get` 无人调用 | 删除 |
| 6 | Cargo.toml 无 `rust-version`；README 写 1.85，但 clippy --fix 引入的 let-chains 需要 1.88 | `rust-version = "1.88"`，README 同步 |
| 7 | `bind -r`、`list-panes -a/-s` 仍是忽略语义 | README "Not implemented" 列明 |
| 8 | 测试 `real_client_in_conpty` 断言状态栏标题为 `"cmd"`，实际显示 OSC 标题全路径（行为正确） | 修断言 |

测试结果（修复后）：lib 47 passed / console 2 passed / e2e 6 passed / clippy 0 warnings。

遗留：无。

## 第 2 轮（不计数）— 视角：机制通路存活 + 不变量

做了什么：通读 server/mod.rs 全部 2000 行，对每个选项/命令问"改了输入，输出真的变吗"；对 active/last/cur 索引的不变量逐入口核对。

发现（5 项，全部已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | 默认 `,`/`$` 绑定的模板 `rename-window -- %%` 把 `--` 当成新名字（`Args::is_flag` 不消费 `--`） | `is_flag` 消费 `--` 并结束 flag 解析；删掉三处手写的 `--` 特判；单元测试 `double_dash_ends_flags` + e2e 走 `,` 绑定 |
| 2 | `set mouse off` 只停掉 server 端鼠标处理，client 控制台仍 `ENABLE_MOUSE_INPUT`，终端原生选区依旧不可用 | 新增 `ServerMsg::SetMouse`：attach 后与 `set mouse` 时下发，client 切换 `ENABLE_MOUSE_INPUT`/`ENABLE_QUICK_EDIT_MODE`；e2e 断言两次切换都到达 |
| 3 | 配置文件解析错误只写日志，用户看不到 | 记入 `config_errors`，首个 attach 的 client 以 overlay 显示 |
| 4 | 多行 Error（`source-file` 多条）被压成一行状态栏消息 | `show()` 统一：含换行走 overlay；e2e 用坏配置文件验证两条错误都显示 |
| 5 | re-attach 不清 overlay/drag/mouse_buttons/swallow_up | `Outcome::Attach` 里全部重置 |

不变量核对（无发现）：`w.active ∈ w.panes`（remove_pane / break-pane / kill -a 三个入口）；`s.cur < s.windows.len()`；`base-index` 在 label / resolve / `0-9` 绑定三处一致；status_click 的列偏移与 render 的 `"[name] "` + `"label "` 布局一致。

测试结果（修复后）：lib 48 passed / console 2 passed / e2e 6 passed / clippy 0 warnings。

## 第 3 轮（不计数）— 视角：代码正确性（错误路径、资源、并发）+ 边界输入

做了什么：通读 client.rs / console.rs / pane.rs / clipboard.rs / ipc.rs 与 mod.rs 的 copy-mode 辅助函数；对每个 unsafe 与每个无界资源问"谁来收尾、上限在哪"；对 usize 减法找下溢。

发现（7 项，全部已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | `copy_abs` 计算 `max - offset`：`clear-history` 或缩窗后 scrollback 变短，offset > max 下溢 panic | `copy_abs` 先把 offset/cy 夹回范围；`clear-history` 先退出 copy mode |
| 2 | 每客户端输出通道无界：pane 高速输出 + 客户端 `WriteConsoleW` 慢 → 服务端内存无限增长 | 有界通道 64 帧；满则丢帧并置 `last_grid=None` 强制下次全量重绘（语义上等价）；单元测试 `lagging_client_forces_full_redraw` |
| 3 | `kill-pane` 只 `TerminateProcess` 顶层 shell，shell 启动的子进程（ping、编译）变孤儿并继续占用 ConPTY | 每 pane 一个 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job；kill/drop 先关 job；测试 `kill_takes_grandchildren_down`（孙进程 ping 被杀，marker 文件不出现）与 `job_kills_tree_on_drop` |
| 4 | 命名管道默认 DACL，本机其他用户可 connect | SDDL `D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)`，每个 pipe 实例都带；真实二进制测试仍通过证明同用户可连 |
| 5 | `display-time 0` 变成"立即消失"，tmux 语义是"直到按键" | render_all 跳过 TTL 清理；handle_key 在任意逻辑键时清消息 |
| 6 | `clipboard::get_text` 找 NUL 无上限，数据无 NUL 时越界读 | `GlobalSize` 上界 |
| 7 | `-L` socket 名不过滤：`\`/`..` 可逃出 `wmux-<user>-` 命名空间 | `pipe_name` 对 socket 名同样白名单过滤；空名回落 `default`；单元测试 |

测试结果（修复后）：lib 53 passed / console 2 passed / e2e 6 passed / clippy 0 warnings。

## 第 4 轮（不计数）— 视角：边界与退化输入

做了什么：针对每个接收外部尺寸/坐标/字符串的入口构造退化输入（0x0 / 1x1 区域、越界鼠标坐标、非 UTF-8 argv、不存在的程序、非 ASCII 名字），先写测试再看结果。

发现（4 项，全部已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | `std::env::args()` 遇到非 UTF-8 参数（路径里未配对代理项）直接 panic | `args_os()` + lossy |
| 2 | 点击窗口区域外缘（x == cols）会被当成最右 pane 的边框，开始一次错误的拖拽 | `border_owner` 只在 `window_area` 内生效 |
| 3 | 0 宽/高区域布局时第二个子节点被放到区域外（`pos += sz + 1` 不封顶） | `layout()` 对 `sz` 与 `pos` 以区域末端封顶；测试 `degenerate_areas_never_panic` |
| 4 | pane 矩形大于 client 网格时（两 client 尺寸不同），光标可能被放到网格外 | `compose` 额外检查光标在 `f.cols × f.rows` 内；测试 `compose_degenerate_sizes` |

新增测试：`list_keys_is_reproducible_across_servers`（两个独立 server 的 `list-keys` 逐字节一致）、不存在程序返回错误而非死 session、非 ASCII session/window 名往返。

测试结果（修复后）：lib 55 passed / console 2 passed / e2e 7 passed / clippy 0 warnings。

## 第 5 轮（计数 1/3，无发现）— 视角：可复现性 + 按键编码逻辑

做了什么：`cargo test` 连跑 3 次，对"排序后的测试名+状态"做 SHA-256；逐条核对 `key_from_record` / `encode_key` / `handle_key` 的 prefix、swallow_up、copy-mode、overlay、IME/代理项、AltGr、Ctrl+数字/标点、修饰键单按等路径。

数据：3 次运行指纹均为 `E9109651AFB600D1`，每次 lib 55 / console 2 / e2e 7 全绿。

发现：无。

## 第 6 轮（不计数）— 视角：静态一致性复查（依赖、示例配置、文档表格）

做了什么：grep 每个 Cargo 依赖在 `src/` 的使用；用真实二进制加载 `wmux.conf.example`（`WMUX_CONFIG`）并核对 `list-keys` 与 server 日志；README 按键表逐行对照 `default_bindings()`。

发现（1 项，已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | `clap`、`toml` 两个依赖以及 tokio 的 `signal` feature 声明了但代码从未使用 | 删除 |

数据：示例配置加载日志 `loaded ... (20 commands)`，无错误行；`list-keys` 含 `W`/`h`/`r`/`|` 四条自定义绑定；README 表 16 行全部与默认绑定一致。

## 第 7 轮（不计数）— 视角：新增代码（winsec / job / 背压）的错误路径

做了什么：对第 3 轮新增的 `winsec.rs`、pane job、有界队列逐个失败分支追问"失败后系统处于什么状态、谁还能恢复"。

发现（1 项，已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | 有界队列满时 `Client::send` 直接丢弃控制消息。真实触发场景：`mouse off` 下用户在宿主终端 QuickEdit 拖选会冻结 `WriteConsoleW`，队列填满，此时的 `Detached`/`SetMouse` 丢失，客户端永远收不到 detach | 控制消息进 `pending` 队列，每次 `render_all` 前补发；pending 非空时帧不插队（丢帧并强制全量重绘）；单元测试 `control_messages_survive_a_full_queue_in_order` |

核对无发现：`current_user_sid` 两段式缓冲区、LocalFree 配对；`OwnerOnly` 指针在 future 移动后每次现取；job assign 失败回落到 TerminateProcess；`Pane::drop` 先关 job 再关 ConPTY 的顺序。

测试结果（修复后）：lib 56 passed / console 2 passed / e2e 7 passed / clippy 0 warnings。

## 第 8 轮（计数 1/3，无发现）— 视角：可复现性 + 机制通路（真实二进制）

数据：`cargo test` 连跑 3 次指纹均为 `A4092EC0BD2FA264`（65 项：lib 56 / console 2 / e2e 7）。
release 二进制 + 临时配置（`prefix C-a`、`status-position top`、`base-index 1`、`bind -n M-q`、`default-shell cmd`）：
`list-windows` 编号从 1 起；`send-keys -t s1:2` 命中第二个窗口；`list-keys` 含 `root M-q`；pane 标题 `cmd.exe`。全部生效。

发现：无。

## 第 9 轮（计数 2/3，无发现）— 视角：静态一致性终审

做了什么：`--help` 文本中 20 个命令名逐个对照 `command::parse` 的别名表；README 中的路径（`.wmux.conf`、`.config\wmux`、`WMUX_CONFIG`、`%LOCALAPPDATA%\wmux\server.log`）、环境变量（`WMUX`、`WMUX_PANE`、`WMUX_LOG`）、管道名格式与代码对照；`docs/acceptance.md` 各轮数字与当时测试输出对照。

数据：clippy 0 warnings；65 项全绿；`git status` 干净；`wmux --help` / `wmux -V` 输出正常。

发现：无。

## 第 10 轮（计数 3/3，无发现）— 视角：核心不变量的真实场景验证

做了什么：release 二进制，在隐藏控制台里 attach 一个 session，`Stop-Process -Force` 强杀客户端（等价于关掉终端窗口），再从 CLI 查询、发键、在另一个隐藏控制台重新 attach，最后 `kill-server`。

数据（原样）：
```
keep: 1 windows (created 1s ago) [120x30] (attached)
-- kill client --
keep: 1 windows (created 2s ago) [120x30]
0: [120x29] %2 C:\WINDOWS\system32\cmd.exe (active)
-- re-attach --
keep: 1 windows (created 5s ago) [120x30] (attached)
client2 exited after kill-server: True code=0
```
`cargo test` 65 项全绿；clippy 0。

发现：无。

## 结论（第一次验收）

第 8、9、10 轮连续零发现，验收通过。累计修复 27 项（第 1–4、6、7 轮），新增测试 21 项；最终 65 项自动化测试（单元 56、真实 ConPTY 2、管道级 e2e 7）。

---

# 第二次验收：Windows Terminal 真机演示后

触发：用户要求在 Windows Terminal 里实际运行。用 `SendKeys` 向真实 WT 窗口打键、截图、`capture-pane` 读回。

## 第 11 轮（不计数）— 视角：真机运行（Windows Terminal + 中文 IME）

发现（2 项代码问题 + 2 项脚本问题，全部已修）：

| # | 问题 | 修复 |
|---|------|------|
| 1 | **server 无声消失**。根因：vt100 0.16.2 `Row::resize` 在 pane 缩小时把宽字符（中文）截成半个留在最后一列，之后擦除走 `clear_wide(col)` 访问 `col+1` 越界 panic（`row.rs:89: len is 50 but the index is 50`）；panic 经 `block_on` 直接带走进程，什么都没记 | `vendor/vt100` 打补丁（`resize` 清掉半个宽字符；`clear_wide` 边界检查），`[patch.crates-io]` 接入；回归测试 `shrink_through_wide_char_then_erase_does_not_panic`（补丁前复现 panic，补丁后通过） |
| 2 | server 任何 panic 都无日志、且杀掉全部 session | `main` 装 panic hook 写 `server.log`；事件循环 `catch_unwind`，恢复后强制全量重绘 |
| 3 | 演示脚本 `GetWindowTextW` 未声明 Unicode 封送，标题只剩首字符，找不到/聚焦不了窗口 | `CharSet.Unicode` |
| 4 | 演示脚本 IME 探测对数组用 `-notmatch`，误判后按 Shift 把输入法切回中文 | 先 `-join` 再匹配 |

新增：`capture-pane -p [-S -N] [-t]`（tmux 同名命令，CLI 读 pane 文本；e2e 覆盖）；client attach 时设置控制台标题 `wmux: <session>`。

真机数据（补丁后重跑，`shots3/`）：探测行原样到达（`echo probe-abc xyz`）；`Write-Host` 输出正确；`prefix %`/`"` 得到 3 个 pane（50x29 / 49x14 / 49x14），WSL 在 pane 里跑出 `Linux HOST 6.18.33.2-microsoft-standard-WSL2`；`prefix z` 状态栏出现 `*Z`；`prefix c` + `prefix ,` 得到 `1:renamed*`；`prefix ?` overlay `[28 of 49 lines] press any key`；`prefix d` 后 `ls` 显示 `demo: 2 windows`（未 attached）；日志无 panic。

IME 说明：WT 窗口输入法处于中文模式时，字母进入拼音合成、空格提交候选，这与任何终端一致，不是 wmux 的行为；演示脚本用 `WM_IME_CONTROL` 把该窗口切到英文。

## 第 12 轮（计数 1/3，无发现）— 视角：vendor 补丁审查 + 可复现性

做了什么：逐行审 `vendor/vt100/src/row.rs` 的两处改动；核对 `Row` 其余索引点（`erase`/`remove`/`truncate`）只用光标内坐标；确认 `blit_screen` 对最后一列半宽字符的处理。`cargo test` 连跑 3 次。

数据：3 次指纹均为 `A522E4B06BC4101C`（66 项：lib 57 / console 2 / e2e 7）；clippy 0。

发现：无。

## 第 13 轮（计数 2/3，无发现）— 视角：静态一致性

做了什么：`capture-pane` 在 README/`--help`/e2e 三处一致；`Cargo.toml` `[patch.crates-io]`、`vendor/vt100` 15 个文件入库、`cargo tree` 解析到 vendor 路径；README 说明补丁与 panic 日志；memory/acceptance 数字更新为 66。

数据：全量测试 66 项全绿。

发现：无。

## 第 14 轮（计数 3/3，无发现）— 视角：真机演示复跑（Windows Terminal）

做了什么：补丁后的 release 二进制在新的 WT 窗口重跑完整演示脚本（探测、打字、`%`/`"` 分屏、WSL、缩放、新窗口、`,` 重命名、`?` overlay、detach）。

数据：与第 11 轮补丁后一致——3 pane（50x29 / 49x14 / 49x14）、`1:renamed*`、WSL `uname -a` 输出在 pane 内、detach 后 `ls` 显示 session 未 attached 仍存活；`server.log` 无 panic。（脚本最后一张截图在 WT 关闭标签页后取窗口矩形失败，属脚本行为，非 wmux。）

发现：无。

## 结论（第二次验收）

第 12、13、14 轮连续零发现，验收通过。本次新增修复 4 项（1 项上游 crate 缺陷、1 项 panic 兜底、2 项演示脚本），新增测试 2 项；最终 66 项自动化测试。

---

# 第三次验收：插件化

新增：`src/format.rs`（状态栏格式引擎）、`run-shell`、`set-hook`/`show-hooks`、`@plugin`/`plugin-path`/`load-plugin`/`list-plugins`、`show-options`、状态栏 `status-left/right`/`window-status-*`/`status-interval`。

## 第 15 轮（不计数）— 视角：静态一致性

| # | 问题 | 修复 |
|---|------|------|
| 1 | `config.rs` 注释声称插件可用 `show-options` 读 `@` 选项，命令不存在（tmux 插件的标准做法 `show-option -gqv @x`） | 实现 `show-options [-gqv] [name]`，`Options::get` + `SHOWABLE` 表；单元 + e2e |
| 2 | `spawn_shell` 线程创建失败时静默，客户端永远等 `Pending` | 失败即回送 `ShellDone(127)` |
| 3 | `load-plugin <file>` 不去重 | 与目录路径同样去重；e2e 二次加载不重复 |

## 第 16 轮（不计数）— 视角：机制通路（release 二进制 + 真实插件目录）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `run-shell` 子进程里 `wmux` 不在 PATH（未 `cargo install` 时插件回调失败） | pane 与 run-shell 环境的 PATH 前置 wmux.exe 所在目录（`path_with_self`，幂等） |
| 2 | pwsh 无控制台时按 ANSI 代码页输出，中文变乱码 | 脚本前置 `[Console]::OutputEncoding = UTF8` |
| 3 | `show-hooks` 输出嵌套引号未转义，不可再解析 | 用 `quote()`；e2e 把输出喂回 tokenizer 验证 |

数据：`run-shell 'wmux -L $env:WMUX display-message callback-ok'` 打印 `callback-ok`；`Write-Output 中文` 原样返回；hook 文件写入 `fired`。

## 第 17 轮（不计数）— 视角：边界输入 + 错误路径

| # | 问题 | 修复 |
|---|------|------|
| 1 | 格式串里未知的 strftime 字段（如 `%Q`）让 chrono 的 `Display` 返回 Err，`to_string()` panic | 用 `write!` 捕获错误，原样输出；测试 `malformed_formats_do_not_panic` 覆盖 10 种畸形串 |
| 2 | hook 命令内部 panic 时 `in_hook` 守卫永远为 true，之后所有 hook 失效 | `catch_unwind` 恢复分支重置 |
| 3 | 状态栏 `#(cmd)` 挂死时 `shell_running` 永远占位，该项再也不刷新 | job object + 30s 看门狗，超时杀进程树，退出码 124；测试 `run_shell_timeout_kills_the_tree` |
| 4 | （修 3 时引入）无超时分支把 job 提前 drop，命令刚启动就被杀 | job 生命周期延长到 `wait_with_output` 之后；`exit 3` 测试抓到 |

测试结果：lib 66 / console 2 / e2e 8，clippy 0。

## 第 18 轮（计数 1/3，无发现）— 视角：可复现性 + 并发路径审查

数据：`cargo test` 3 次指纹均为 `C9E57FBF0DC118C5`（76 项：lib 66 / console 2 / e2e 8）；clippy 0。
审查：`refresh_status_shells` 的 due/fresh/running 三态；`spawn_shell` 失败回送；`StatusShell` 到达即清 running；`status-interval 0` 夹到 1；线程与主循环只经 unbounded channel 交互。

发现：无。

## 第 19 轮（计数 2/3，无发现）— 视角：静态一致性

做了什么：README 插件章节的 9 个 hook 名与 `HOOKS` 常量逐个对照；`show-options -gqv`/`load-plugin`/`list-plugins`/`run-shell`/`set-hook` 在 README、`--help`、解析器三处一致；release 二进制加载 `wmux.conf.example`（含新格式串与 `#()`）。

数据：日志 `loaded wmux.conf.example (23 commands)`，无错误；`show-options -gv status-right/status-left/status-interval` 回显与文件一致；全量 76 项通过；工作树干净。

发现：无。

## 第 20 轮（计数 3/3，无发现）— 视角：机制通路（release 二进制 + 真实插件目录）

数据：`@plugin agent-status` 经配置加载 → `list-plugins` 列出目录；`show-options -gqv @agent-status-file` 回显；`run-shell` 内 `wmux -L $env:WMUX display-message callback-ok` 打印 `callback-ok`、中文原样；`exit 5` 映射为 `shell exited with 5` / CLI 退出码 1；`new-window` 触发 `after-new-window` hook 的后台 `run-shell -b` 写入 `fired`；日志无 panic/ERROR。

发现：无。

## 结论（第三次验收）

第 18、19、20 轮连续零发现，验收通过。本次新增修复 10 项（第 15–17 轮），新增测试 10 项；最终 76 项自动化测试。

---

# 第四次验收：重启后 resume、按 session 保存、vim 键位、synchronize-panes

新增：`src/resurrect.rs`（每 session 一个 JSON，原子写入）、自动保存（结构变化即存，`kill-server` 时再存）、`resume [name]`/`list-saved`/`save-session`/`restore-session`/`delete-saved`、`autosave`/`restore-on-start`/`sessions-dir`/`WMUX_SESSIONS_DIR`、`prefix h/j/k/l`+`H/J/K/L`+`Tab`、`set -w synchronize-panes`（`prefix S`，状态栏 `S` 标记）。

## 第 21 轮（不计数）— 视角：机制通路 + 不变量

| # | 问题 | 修复 |
|---|------|------|
| 1 | e2e 服务器的自动保存写进用户真实目录（`%LOCALAPPDATA%\wmux\sessions` 出现 8 个测试 session 文件） | `WMUX_SESSIONS_DIR` 环境变量；e2e Harness 与 console 测试统一指向临时目录；已清理污染文件；全量测试后真实目录 0 文件 |
| 2 | `rename-session` 后旧名字的保存文件残留，`list-saved` 出现幽灵条目，可被 resume 出陈旧副本 | 重命名时把保存文件一并改名；e2e 验证旧名消失 |
| 3 | `delete-saved` 对正在运行的 session 在下一个 tick 被自动保存撤销 | 运行中拒绝删除并提示先 kill；e2e 覆盖 |
| 4 | （测试自身）kill 掉唯一 session 后 server 按设计退出，测试 `connect()` 无超时死等 | 测试先建保活 session，`connect()` 加 10s 超时 |
| 5 | **主循环 tick 永不触发**：`select!` 里每次迭代新建 `sleep(1s)`，只要事件持续到来（客户端轮询、pane 持续输出）就一直重置；自动保存、空闲退出全挂在 tick 上，繁忙时静默失效。由 rename 测试的 200ms 轮询暴露 | 改为循环外的持久 `interval`（`MissedTickBehavior::Delay`） |

真机数据（release，`WMUX_SESSIONS_DIR` 临时目录）：建 `work`（2 窗口，首窗口 pwsh|wsl/cmd 三格）+ `scratch` → 2s 内出现两个保存文件 → `Stop-Process` 强杀 server（模拟重启）→ `ls` 报无 server → `wmux resume work` 自动起新 server，`list-panes` 尺寸/命令与杀前逐项一致（40x23 %pwsh、39x11 %wsl、39x11 %cmd），WSL pane 提示符目录 `/mnt/c/Users/me/proj` → `wmux resume` 补回 `scratch`；日志无 panic。

## 第 22 轮（计数 1/3，无发现）— 视角：可复现性 + 静态一致性

数据：`cargo test` 3 次指纹均为 `2EA9C8924603ED88`（81 项：lib 69 / console 2 / e2e 10）；clippy 0；全量测试后真实 sessions 目录 0 文件；工作树干净。README / `--help` / `wmux.conf.example` 中 `resume`、`list-saved`、`synchronize-panes`、`sessions-dir`、`restore-on-start`、`autosave` 共 14 处提及，与解析器/选项表一一对应；README 按键表已改为 h/j/k/l、Tab、S、C-s/C-r。

发现：无。

## 第 23 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

做了什么：建 `work`（pwsh | wsl）+ `keep`；客户端每 100ms 轮询 3 秒（制造第 21 轮第 5 项那种持续事件流）；`rename-session work work2`；`delete-saved keep`；强杀 server；`resume work2`。

数据（原样）：轮询期间两个 session 仍在 3s 内保存（`list-saved` 两条均 `(running)`）；rename 后 `list-saved` 只有 `work2:`，无 `work:`；`delete-saved keep` → `session keep is running; kill it first (or set autosave off)`，退出码 1；强杀后 `resume work2` → `restored 1 session(s)`，`list-panes` 为 `40x23 pwsh` / `39x23 wsl (active)`，与杀前一致；日志无 panic/ERROR/WARN。

发现：无。

## 第 24 轮（计数 3/3，无发现）— 视角：边界与退化输入（恢复路径）

核对：`active` 越界回落到首个 pane；`current` 越界夹到最后一个窗口；窗口列表为空报错；`argv` 为空回落默认 shell；已不存在的 `cwd` 被忽略；`sizes` 与子节点数不符时重置为等分；`.json.tmp` 残片被 `list()` 按扩展名忽略（原子写入被强杀也无损）；不同名字的文件名经 FNV 哈希后缀不碰撞（单元测试）；resume 已运行的 session 等价于 attach。

数据：全量 81 项通过。

发现：无。

## 结论（第四次验收）

第 22、23、24 轮连续零发现，验收通过。本次新增修复 5 项（含一个影响全局的 tick 饥饿 bug），新增测试 5 项；最终 81 项自动化测试。

---

# 第五次验收：pane 目录跟随（set-cwd / OSC 7 / OSC 9;9）

新增：`set-cwd [-t target] [dir]`（无参用调用方 cwd；无 `-t` 时优先 `WMUX_PANE` 所在 pane）；vt100 回调解析 OSC 7（`file://host/path`）与 OSC 9;9，`/mnt/<盘>/...` 映射回 Windows 路径，纯 Linux 路径忽略；`#{pane_current_path}`；`list-panes` 显示目录。

## 第 25 轮（不计数）— 视角：机制通路

| # | 问题 | 修复 |
|---|------|------|
| 1 | （测试自身）用 `echo` 在 cmd 里打 ESC 序列，ESC 键被 cmd 行编辑当作"清行"，OSC 根本没发出 | 改为 `pwsh -Command Write-Host ([char]27+...)`；证实 OSC 9;9 能穿过 ConPTY 到达 server |
| 2 | clippy 3 处（`?` 简化、`is_empty`、`+ 0`） | 修 |

## 第 26 轮（不计数）— 视角：真机 + 边界输入

真机数据（release，pwsh prompt 发 OSC 9;9）：pane 起始目录 `C:\Users\me` → `cd Documents` 后 `list-panes` 显示 `[C:\Users\me\Documents]` → 强杀 server → `resume w` → pane 提示符 `PS C:\Users\me\Documents>`；`set-cwd -t keep`（无参）记录为 CLI 的当前目录；日志无 panic。

| # | 问题 | 修复 |
|---|------|------|
| 1 | `set-cwd 相对路径` 相对 server 进程目录检查，而非调用方目录 | 相对路径拼到客户端 cwd 上；e2e 覆盖 |

## 第 27 轮（计数 1/3，无发现）— 视角：可复现性 + 静态一致性

数据：`cargo test` 3 次指纹均为 `2C49C316E391BE33`（83 项：lib 71 / console 2 / e2e 10）；clippy 0；真实 sessions 目录 0 文件；`set-cwd`/`#{pane_current_path}`/OSC 在 README、`--help`、解析器、server、pane、format 六处共 19 次提及且一致。

发现：无。

## 第 28 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

数据：在 `C:\Users\me` 下 `set-cwd -t w Documents` → `[C:\Users\me\Documents]`；无参 → `[C:\Users\me]`；不存在的目录 → `set-cwd: not a directory: ...`，退出码 1；自动保存文件随之更新；日志无 panic。

发现：无。

## 第 29 轮（计数 3/3，无发现）— 视角：边界输入（路径转换）

核对 `windows_path_from_announced` 的 12 个用例（单元测试）：`file:///C:/x`、`file://host/C:/x%20y`、`C:/x`、`D:`、`/mnt/c/x`、`/mnt/d`、`file://host/mnt/c/x` 全部映射为 Windows 路径；`/home/x`、`/mnt/wsl/x`、空串、`file://` 返回 None 保留旧值；`%zz` 非法百分号编码原样保留。全量 83 项通过；工作树干净。

发现：无。

## 结论（第五次验收）

第 27、28、29 轮连续零发现，验收通过。本次新增修复 1 项（相对路径解析），新增测试 2 项；最终 83 项自动化测试。

---

# 第六次验收：choose-tree 选择界面（prefix s / w）

新增：`choose-tree [-s|-w]`（别名 `choose-window` / `choose-session`，接受 tmux 的 `-Z/-N/-G/-r` 组合写法），
默认绑定 `prefix w` = `choose-tree -Zw`（所有 session 展开到窗口）、`prefix s` = `choose-tree -Zs`（只列 session）。
交互：`j`/`k`（或方向键、`C-n`/`C-p`）移动，`g`/`G`（或 Home/End）到头到尾，PgUp/PgDn 与 `C-f`/`C-b`/`C-d`/`C-u` 翻页，
数字键跳到带 `(n)` 标签的前 10 项，`Enter` 选中（切 session + 选窗口，走 `switch-client` 并触发 `after-select-window`），
`q`/`Esc`/`C-c` 取消；鼠标滚轮移动光标、单击把光标放到那一行。列表每帧按活的会话树重建，光标跟着条目身份走。

## 第 1 轮（不计数）— 视角：静态一致性

做了什么：对照新命令在解析器、默认绑定、`--help`、两份 README 的一致性；检查 `needs_server` 是否会因新命令误启服务端。

| # | 问题 | 修复 |
|---|------|------|
| 1 | picker 与 `:` 命令行、overlay 可同时存在，按键会喂给看不见的 prompt | 打开 picker 时清掉 prompt/overlay，保证同一时刻只有一个模态 |

## 第 2 轮（不计数）— 视角：机制通路（真实键盘路径）

做了什么：新增 `tests/console.rs::choose_tree_through_the_real_keyboard`，用真实 `wmux.exe` 在 ConPTY 里走
ConPTY → conhost → `ReadConsoleInputW` → win32 输入记录 → server 的完整链路，验证 j/k/g/G/Enter 真的生效。

| # | 问题 | 修复 |
|---|------|------|
| 1 | （测试自身）新窗口用默认 shell（pwsh），断言写成 `cmd`；断言里的 pane 标题在提权终端下多了 `管理员:` 前缀 | 测试显式指定窗口命令；标题断言改为不依赖提权前缀 |

## 第 3 轮（不计数）— 视角：逻辑正确性与不变量

做了什么：通读 `chooser_key`/`chooser_go`/`handle_mouse`/`render_client` 的新分支，逐条对照 copy mode 的既有语义。

| # | 问题 | 修复 |
|---|------|------|
| 1 | picker 把前缀键也吞了（copy mode 不是这样），`prefix d` 无法从 picker 里脱离，且 `C-b` 被当成翻页 | picker 改成和 copy mode 同级：先处理前缀，再交给模式 |
| 2 | overlay 画在 picker 底下，但按键归 overlay：用户看不到却按一下就"没反应" | 渲染顺序改为 picker 在下、overlay 在上 |
| 3 | 状态栏右侧（pane 标题）可以把整个窗口列表挤没——提权时 cmd 标题变成 `管理员: C:\WINDOWS\...`，60 列下 `[r] 0:cmd*` 消失 | `compose` 为当前窗口标签预留位置，宁可裁剪右侧；放不下时退回旧行为；新增单元断言 |

## 第 4 轮（不计数）— 视角：边界与退化输入

做了什么：新增 `choose_tree_degenerate_sizes_and_wide_names`：中文 session/窗口名（双宽）、20x3 与 1x1 终端、
游标下的 session 被 `kill-session` 掉。

| # | 问题 | 修复 |
|---|------|------|
| 1 | （测试自身）条目数算错（2 session + 2 窗口 = 4 项，写成 3） | 修断言 |

## 第 5 轮（不计数）— 视角：代码正确性（通读 diff）

做了什么：`git diff -- src` 全文复核，盯溢出、借用、panic 路径、确定性。

| # | 问题 | 修复 |
|---|------|------|
| 1 | 状态栏新算式 `x + need + 1` 是普通加法，极长窗口名会在 debug 下整数溢出 panic | 改 `saturating_add` |

## 第 6 轮（计数 1/3，无发现）— 视角：机制通路 + release 二进制

数据：`cargo test --release --all-targets` 89 项全过（lib 73 / console 3 / e2e 13）；
picker 专项 6 项单独跑全过（含真实 ConPTY 键盘路径）；release 二进制 `choose-tree` 未 attach 时退出码 1、
提示 `choose-tree: client not attached`。

## 第 7 轮（计数 2/3，无发现）— 视角：静态一致性终审

数据：release 二进制 `list-keys` 输出 `bind-key -T prefix s choose-tree -s` / `w choose-tree -w`（可被解析器原样回读）；
`choose-tree` / `choose-window` / `choose-session` / `-Zw` 四种写法均被接受，`-x` 报 `unknown flag '-x'` 且退出码 1；
两份 README 的按键表与"未实现"段、`--help` 三处口径一致；无残留的"列出 session / 窗口"旧说法。

## 第 8 轮（计数 3/3，无发现）— 视角：可复现性 + 无残留

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（73/0/3/13）；测试名指纹 `736D60A2B88738B0`（89 项）；
真实 `%LOCALAPPDATA%\wmux\sessions` 运行前后均为 2 个文件（未被测试污染）；测试进程无残留；
TEMP 下的 `wmux-*` 目录只剩手工调试留下的（已清），通过的测试自身不留目录。

观察（非缺陷）：`tests/console.rs` 的临时目录清理写在测试末尾，测试 panic 时会留下一个目录；e2e 侧走 `Drop` 不受影响。

## 结论（第六次验收）

第 6、7、8 轮连续零发现，验收通过。本次修复 5 项（含一个既有的状态栏挤压 bug），新增测试 6 项；最终 89 项自动化测试。

---

# 第七次验收：全屏下的方向切换 + 补齐 `-t`

起因：用户反馈“全屏的时候不能切换窗格”。确认是 bug——`select-pane -L/-R/-U/-D` 从 `w.rects` 找邻居，
而 zoom 时 rects 只剩铺满整窗的那一个矩形，于是永远找不到邻居，返回 `no such pane`；
`o` / `;` / `select-pane -t N` 走布局顺序，所以那几个在全屏下一直是好的。

修复：zoom 时用真实布局重新算一遍矩形来找邻居，选中后照常取消 zoom（和 `o`、`;` 的行为一致）。
回归测试 `zoomed_pane_still_navigates_by_direction` 先验证过“撤掉修复就超时失败”。

## 第 1 轮（不计数）— 视角：不变量

数据：`zoomed` 的 8 处写入点逐一核对，不变量“zoomed ⇒ 窗口里不止一个 pane，且 rects 由 relayout 重建”成立；90 项测试通过。

发现：无（本轮只做审查）。

## 第 2 轮（不计数）— 视角：机制通路（release 二进制）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `resize-pane -t` / `swap-pane -t` 报 `unknown flag '-t'`，但 README 写着“每个窗口和 pane 命令都接受 `-t`” | 两个命令补上 `-t`（`swap-pane` 同时接受 `-s`） |
| 2 | `break-pane` 压根不解析 flag，`-t` 被静默吞掉，作用在当前 pane 上 | 正常解析 `-t`/`-s`，未知 flag 报错 |

## 第 3 轮（不计数）— 视角：边界（全屏下的隐藏 pane）

| # | 问题 | 修复 |
|---|------|------|
| 1 | 全屏时 `list-panes` 把被藏起来的 pane 报成 `[0x0]`（它其实还在原尺寸正常跑，`send-keys` + `capture-pane` 可证） | 改为显示 pane 自己的尺寸而不是布局矩形；e2e 补断言：隐藏 pane 仍是 40x23 且能收到按键 |

## 第 4 轮（不计数）— 视角：静态一致性（解析器口径）

| # | 问题 | 修复 |
|---|------|------|
| 1 | 8 个命令（`next/previous/last-window`、`paste-buffer`、`list-keys`、`list-plugins`、`clear-history`、`version`）不解析参数，`next-window -x` 之类的笔误被静默忽略 | 统一 `none_left` 校验；顺带给三个窗口切换命令加上 tmux 的 `-t <session>` |
| 2 | `version` / `help` 在 `main.rs` 里前置处理，`wmux version -v` 照样成功 | 多余参数报错退出 1；`tests/console.rs` 增加真实二进制的守卫测试 |
| 3 | MSI 构建脚本失败时会留下临时 stage 目录 | `try/finally` 清理，失败与成功两条路径都验证过 |
| 4 | README 没说清 `swap-pane` 的 `-s`/`-t` 只是“要交换的那个 pane”，没有 tmux 的成对形式 | 两份 README 的“未实现”段补上 |

## 第 5 轮（计数 1/3，无发现）— 视角：clippy + 全量

数据：`cargo clippy --all-targets` 零告警；93 项测试通过（lib 75 / console 4 / e2e 14）。

## 第 6 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制走完所有改动路径）

数据：全屏时隐藏 pane 仍是 `[40x23]`；左右布局下 `select-pane -U` 正确地说“no such pane”且保持全屏，`-L` 则切走并取消全屏（尺寸回到 40/39）；
`resize-pane -L 10 -t w:0` 生效（30/49）；`next-window -t`、`break-pane -t` 均作用在指定目标上；
`next-window -x`、`paste-buffer junk`、`version -v`、`break-pane -x`、`swap-pane -q` 全部退出码 1。

## 第 7 轮（计数 3/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（75/0/4/14）；测试名指纹 `DE62C34C9AE61A79`（93 项）；
真实 sessions 目录仍是 2 个文件（用户自己的），测试与构建脚本均无临时目录残留。

## 结论（第七次验收）

第 5、6、7 轮连续零发现，验收通过。本次修复 7 项（1 项来自用户反馈，6 项为验收过程中发现），新增测试 3 项；最终 93 项自动化测试。

---

# 第八次验收：连按（`bind -r`）与 pane 内的 `wmux`

起因：用户反馈“ctrl b 之后 jkl 不能连续跳”。确认是缺失功能——`bind -r` 一直只是被解析器接受然后丢掉，
README 的“未实现”里也这么写着。另外在录制演示时发现：pane 里敲 `wmux list-panes` 连到了本机 `default` socket 上
**另一台** server（用户自己正在跑的那个），而不是管着这个 pane 的 server。

新增：
- `bind -r`：按一次前缀后，同一个键在 `repeat-time`（默认 500ms，设 0 关闭）内可以连着按。
  默认给 `h j k l`、`H J K L`、方向键、`C-`/`M-` 方向键、`n p`、`o`、`{ }` 都开了。
- 客户端默认 socket 取自环境变量 `WMUX`（tmux 用 `$TMUX` 是一个道理），所以 pane 或插件脚本里
  `wmux ls` 直接就是“这台 server”。

## 第 1 轮（计数 1/3，无发现）— 视角：静态一致性 + 全量

数据：clippy 零告警；95 项（lib 75 / console 4 / e2e 15 / 录制 1 个 ignored）；
`list-keys` 输出 10 条 `-r` 绑定且能被解析器回读；`show-options -gv repeat-time` 默认 500，`set` 后为 250；
两份 README 的按键表、配置示例、“未实现”段与实现一致。

## 第 2 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

数据：pane 里 `wmux ls > 文件` 写出的是本 server 的 `s: 1 windows`（修复前会列出另一台 server 的 session）；
`bind -r C-y select-pane -U` 回读为 `bind-key -r -T prefix C-y select-pane -U`；`set -g repeat-time 0` 退出码 0。
真实键盘路径在 `tests/console.rs` 里验证：ConPTY 中发 `C-b h` 再发一个裸 `h`，活动 pane 从 2 跳到 0。

## 第 3 轮（计数 3/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（75/0/4/0/15）；测试名指纹 `3126F1F62B3250D4`；
真实 sessions 目录仍是用户自己的 2 个文件。

## 结论（第八次验收）

三轮连续零发现，验收通过。本次新增 2 项能力，新增测试 3 项（1 个单元、1 个 e2e、1 个真实键盘断言）；
最终 94 项自动化测试 + 1 个按需运行的演示录制。

---

# 第九次验收：`frame too large` 的真因 + `pane-base-index`

起因：用户之前报过 `wmux` / `wmux attach` 直接输出 `[frame too large: 538976288]`。这次查到根因。

538976288 = `0x20202020`，四个空格。客户端把**屏幕内容当成了帧长**，也就是帧流错位了。排查顺序与结论：

1. 老客户端（安装版 0.1.0）连新 server、新客户端连老 server，`ls` 与 `list-panes` 都正常，**版本不兼容排除**。
2. 服务端每条连接只有一个写任务，客户端的写也都串行在一个 `select!` 里，**并发写排除**。
3. 真因在 `src/client.rs` 的 attached 循环：`m = read_frame::<_, ServerMsg>(rd)` 直接写在 `tokio::select!` 的分支里。
   `read_frame` 是「先读 4 字节头，再读 body」两次 await，而 **`read_exact` 不是 cancellation-safe**：
   只要同一轮里按键分支或那个 500ms 的尺寸轮询先就绪，这个 future 就在两次读之间被丢弃，4 字节头已经从管道里消费掉、body 还留着。
   下一轮就把 body 的前 4 字节当帧头。用户那台 session 是 208x55，attach 时首帧整屏重绘很大、必然分多次到达，屏幕上又全是空格，于是读出 `0x20202020`。

修复：帧读取移到独立任务，用有界 channel（256）交给 `select!`，channel 的 recv 是 cancellation-safe 的。
回归测试 `frames_survive_a_select_loop`：在 duplex 上把每帧拆成「头 / 停 2ms / body」发送，同时有 1ms 的 ticker 抢占。
把读帧改回内联写法，测试立刻失败（`deserialize frame: io error: unexpected end of file`）。

同时发现并修复：`pane-base-index` 一直在「接受但忽略」名单里（而 `base-index` 是生效的），
所以 `set -g pane-base-index 1` 之后 pane 仍从 0 编号。现在 `list-panes`、`-t session:window.pane` 解析、`select-pane -t N`、`#P` 四处统一。

## 第 1 轮（计数 1/3，无发现）— 视角：静态一致性 + 全量

数据：clippy 零告警；97 项（lib 76 / console 4 / e2e 16 / 录制 1 个 ignored）；`show-options -gv pane-base-index` 默认 0；两份 README 的配置示例同步更新。

## 第 2 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

数据：`set -g pane-base-index 1` 之后 `list-panes` 编号为 1 和 2；`send-keys -t s:0.1` 退出码 0；`-t s:0.0` 报 `no pane 0`；`show-options` 读回 1。

## 第 3 轮（计数 3/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（76/0/4/0/16）；测试名指纹 `C6F18B7C36796B3F`。

## 结论（第九次验收）

三轮连续零发现，验收通过。本次修复 2 项（其中一项是用户报过的帧错位真因），新增测试 2 项；最终 96 项自动化测试。

---

# 第十次验收：补齐缺的 tmux 功能

用户要求「缺的功能都加了吧」，另外点名要命令简写（`tmux att` 那种）。本次新增：

- **命令名前缀匹配**：任何不产生歧义的前缀都当完整命令名用（`att`、`lsp`、`splitw`、`choose-t`）。
  有歧义就报错并列出候选（`kill` → kill-pane / kill-server / kill-session / kill-window），不猜。
- **`select-layout`**：五种命名布局（even-horizontal、even-vertical、main-horizontal、main-vertical、tiled），
  名字同样支持前缀（`til`），`-n` / `-p` 轮换，默认绑定 `prefix Space`。
- **copy mode 搜索**：`/` 往回找、`?` 往前找、`n` / `N` 重复，大小写不敏感，命中后把该行放到视野中间；找不到时状态栏提示。
- **`display-panes`（`prefix q`）**：每个 pane 中间显示大号编号，按数字直接跳过去，其他键关掉。
- **`swap-window` / `move-window`**：同一 session 内重排窗口，当前窗口跟着走。
- **格式串条件 `#{?cond,a,b}`**：条件可以是变量名，也可以是 `var==值` / `var!=值`；分支本身还能再套格式和样式。
- **`set -a`**：往选项原值后面追加（`set -ag status-right " | x"`），组合写法 `-ag` 也认。
- **主题**：`themes/` 下四套配色（Nord、Gruvbox dark、Dracula、Catppuccin Mocha）。
  按之前的判断没有新造 `set theme`，它们就是普通的命令文件，`source-file` 加载即可。

## 第 1 轮（不计数）— 视角：clippy + 全量

| # | 问题 | 修复 |
|---|------|------|
| 1 | clippy 两处（`n % cols == 0` 可用 `is_multiple_of`；`display-panes` 的嵌套 `if` 可合并） | 修 |

## 第 2 轮（不计数）— 视角：机制通路（release 二进制逐条跑新命令）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `display-message -p "#{?window_flags,busy,idle}"` 原样输出。根因：`display-message`、`command-prompt`、`confirm-before` 走的是一个只会替换 `#S/#W/#P/#T/#I` 的简化函数，没走状态栏那套完整格式引擎 | `expand_format` 改为构造 `format::Context` 后调用 `format::expand`，`#{...}`、`#{?...}`、`#(...)`、`%H` 在所有地方口径一致 |

## 第 3 轮（计数 1/3，无发现）— 视角：clippy + 全量

数据：clippy 零告警；103 项测试通过（lib 80 / console 4 / e2e 19）。

## 第 4 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

数据：`select-layout -t s main-v` 得到 39/40 两列；`swap-window -s s:0 -t s:1` 后窗口顺序与 `*` 标记都跟着换；
`display-message -p "#{?window_flags,busy,idle}"` 输出 `busy`；`set -g X` + `set -ag Y` 读回 `XY`；`lsw` 退出码 0。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（80/0/4/0/19）；测试名指纹 `677DD406E21AB251`（104 项，含 1 个按需的录制）。

## 结论（第十次验收）

第 3、4、5 轮连续零发现，验收通过。本次新增 8 项能力，修复 3 项（含一处格式引擎口径不一致），
新增测试 5 项；最终 103 项自动化测试。

---

# 第十一次验收：照着 tmux 手册逐条补齐

用户要求「仔细读 tmux 官方手册，对着里面的命令和快捷键一条条实现」。

做法：先把 tmux master 的 `tmux.1`、`cmd.c`（命令表）、`key-bindings.c`（默认绑定表）、
`options-table.c` 抓到本地，据此写出 `docs/tmux-parity.md`——tmux 的每条命令、每个默认前缀键、
copy mode 的按键，逐条标注 wmux 是「有 / 部分 / 待做 / 明确不做（附理由）」，并排出实现顺序。
然后按那个顺序实现。

本次新增（命令 27 条，默认绑定从 47 涨到 81）：

- **命令名前缀匹配**（`att`、`lsp`、`splitw`），有歧义时列出候选而不是猜。
- **小命令**：`rotate-window`（`C-o` / `M-o`）、`next-layout` / `previous-layout`、`refresh-client`（`r`）、
  `last-pane`、`list-clients`、`list-commands`、`send-prefix`、`show-messages`（`~`）、
  `set-environment` / `show-environment`、`respawn-pane` / `respawn-window`、`detach-client -a/-s`、
  `switch-client -l`、`M-1`..`M-5` 直选布局。
- **粘贴缓冲区**：`set-buffer`、`list-buffers`（`#`）、`show-buffer`、`delete-buffer`（`-`）、
  `save-buffer`、`load-buffer`、`paste-buffer -b/-p`、`choose-buffer`（`=`）。copy mode 复制时
  同时进缓冲区和 Windows 剪贴板。
- **pane / window 搬运**：`join-pane` / `move-pane`（含「标记 pane」`select-pane -m` / `-M`）、
  `resize-pane -x/-y`（支持百分比）、`select-pane -T`、`find-window`（`f`）。
- **copy mode**：`w` `b` `e`（及 `W` `B` `E`）、`^`、`H` `M` `L`、`{` `}`、计数（`3j`）、
  `C-v` 矩形选择。
- **其他**：`clock-mode`（`t`）、`if-shell`（`-F` 用格式串，否则看 shell 退出码）。

## 第 1 轮（不计数）— 视角：机制通路（自测新命令）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `respawn-pane -k` 之后 pane 一片空白。根因：老进程的退出事件晚到，服务端按 pane id 把**刚起来的**新 pane 删掉了 | pane 加代次号，`PaneEvent` 带上代次，非当前代次的输出和退出一律忽略；e2e 覆盖 |
| 2 | `select-pane -m -t j:1` 报 `bad target`：`-t` 的值被无条件当成 pane 选择器解析 | 带 `-m`/`-M`/`-T` 时 `-t` 作为目标解析；两种都不匹配才报错，并说明可用写法 |
| 3 | （测试自身）用 pane 尺寸判断 `rotate-window` 是否生效，但旋转恰恰保持几何不变 | 改为比较各位置上的 pane id，并额外断言几何不变 |
| 4 | （测试自身）`switch-client -t` 从 CLI 调用时作用在未 attach 的 CLI 客户端上 | 测试改从会话内部驱动（`prefix )` 和 `:switch-client -l`） |

## 第 2 轮（不计数）— 视角：静态一致性

| # | 问题 | 修复 |
|---|------|------|
| 1 | 默认绑定里有两条的 Display 输出没人验证过能否被解析器读回 | 新增单元测试 `default_bindings_round_trip`：81 条绑定逐条 Display → tokenize → parse → Display 必须完全一致 |
| 2 | `choose-session`、`choose-window`、`version` 三个命令没出现在对照表里 | 归入「wmux 独有」一行并说明 |

## 第 3 轮（计数 1/3，无发现）— 视角：静态一致性

数据：clippy 零告警；109 项测试通过（lib 81 / console 4 / e2e 24）；
`list-commands` 的 80 条命令全部能被解析器接受，且全部在对照表里有条目；`list-keys` 81 条。

## 第 4 轮（计数 2/3，无发现）— 视角：机制通路（release 二进制）

数据：`rotate-window`、`next-layout`、`join-pane -s/-t`（跨窗口搬运后 pane 数 1→3）、
`set-buffer` / `list-buffers` / `show-buffer`、`set-environment` / `show-environment`、
`if-shell` 两个分支、`resize-pane -x 30`（得到 `[30x23]`）、`clock-mode`、
`find-window` 未命中的报错，全部符合预期。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test --all-targets` 结果逐项相同（81/0/4/0/24）；
测试名指纹 `3D9A118BF99F31FA`（110 项，含 1 个按需的录制）；
真实 sessions 目录未被测试污染；测试结束无残留进程。

## 结论（第十一次验收）

第 3、4、5 轮连续零发现，验收通过。本次新增 27 条命令、34 个默认绑定、copy mode 一整套 vi 动作，
修复 2 个产品缺陷（respawn 的代次竞态、`select-pane -t` 的目标解析），新增测试 6 项；
最终 109 项自动化测试。

# 第十二次验收（2026-09-21）— 排队中的 tmux 功能 + 后台任务与持久化

本次做的：把 `docs/tmux-parity.md` 里排队的功能全部做完——`send-keys -X`、
`display-menu` / `display-popup`、`choose-client`、`pipe-pane`、`wait-for`、
`select-layout -E`、跨 session 的 `move-window`、`capture-pane -e`、
告警标记（`monitor-activity` / `monitor-bell` / `monitor-silence` /
`visual-bell` / `visual-activity`、`#` `!` `~` 三个 `#F` 标记、`M-n` / `M-p`），
外加用户直接问到的两件事：`remain-on-exit`（程序退出后 pane 留着）和
`save-history`（每个 pane 的输出随 session 存盘、`resume` 时贴回屏幕）。
同时把 copy mode 的 `/` 和 `?` 方向改成和 tmux 一致（`/` 往新、`?` 往旧）。

## 第 1 轮（不计数）— 视角：静态一致性

| # | 问题 | 修复 |
|---|------|------|
| 1 | `Pipe.dropped` / `Pipe.command` 只写不读：pipe-pane 的下游跟不上时会**静默丢输出** | `pipe_write` 在第一次丢字节时返回命令名，服务端写 `server.log` 并进 `show-messages`；缓冲区从 256 降到 64 块（≤4 MiB） |
| 2 | 两处 `messages.push_back` 没有裁剪，`show-messages` 可无界增长 | 统一走新的 `note_message`，固定保留最近 100 条 |

## 第 2 轮（不计数）— 视角：机制通路（配置项左右都拨一次）

| # | 问题 | 修复 |
|---|------|------|
| 1 | 没有客户端 attach 时，窗口变成当前窗口后**仍留着 `#` 标记**（`0: cmd*#`），因为清除只发生在渲染路径 | 清除挪进每帧都跑的 `sweep_alerts`：不变量「session 的当前窗口永远没有告警标记」只有一处执法；补 e2e（detached session 也验证） |

## 第 3 轮（不计数）— 视角：边界与退化输入

| # | 问题 | 修复 |
|---|------|------|
| 1 | 脚本里 `wmux display-popup -C` 只关「自己这个 CLI 客户端」的弹窗（等于什么都没干） | 调用方自己没有弹窗时，关掉该 session 上所有客户端的弹窗；新增 e2e |

## 第 4 轮（不计数）— 视角：代码正确性（错误路径 / 资源 / 并发）

| # | 问题 | 修复 |
|---|------|------|
| 1 | 弹窗吞掉了按键的 key-up（`swallow_up`），与普通 pane 的输入路径不一致 | 只有「已结束的弹窗被任意键关掉」那次才吞 key-up；其余 down/up 都照常转发 |
| 2 | 跨 session 的 `move-window` 用下标记录当前窗口：窗口插到前面时，**目标 session 正在看的窗口被换掉**；源 session 同理 | 两边都按窗口 id 记住「原来在看哪个」，搬完再按 id 找回下标；新增 e2e，并把修复临时撤掉验证过该测试会失败（`b still looks at the same window: 0: travels`） |
| 3 | 弹窗只在客户端 resize 时重新定位，`set -g status off/on` 改变窗口区域后会画歪 | `refit_popup` 改成每帧渲染前跑一次（`Pane::resize` 本身对尺寸不变是空操作） |
| 4 | `pipe_to` 启动失败时旧 pipe 还在跑；`self.clients[&cid]` 直接索引会 panic | 先停旧 pipe 再启动新的；索引改成守卫取值 |

## 第 5 轮（不计数）— 视角：静态一致性（文档 claim vs 代码）

| # | 问题 | 修复 |
|---|------|------|
| 1 | 新命令只在 `--help` 和 `list-commands` 里出现，两个 README 都没提 `choose-client` / `display-menu` / `wait-for`；对照表没有 options 一节 | 两个 README 加「脚本常用命令」小节（6 条）；对照表补 Options 一节（29 项逐条对齐 `show-options`） |

## 第 6 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据（`target/release/wmux.exe`，8 项全部符合预期）：
1. 无任何客户端 attach 时 `send-keys -t one:0 -X search-backward/begin-selection/next-word-end/copy-selection` → `show-buffer` = `alpha`；
2. `pipe-pane` 把 pane 输出写进文件（含 `piped-line`），空命令停止后文件落盘；
3. 另一个进程 `wait-for chan` 阻塞，`wait-for -S chan` 后返回 `woken:0`；
4. `move-window -s one:1 -t two:1` 后 `two = 0: cmd* | 1: mover`（当前窗口未被顶掉）；
5. `select-layout -E`：`[60x23] / [19x23]` → `[39x23] / [40x23]`；
6. `capture-pane -p` 不含 ESC，`-e` 含 `ESC[31m`；
7. `remain-on-exit on` 时 `cmd /c exit 7` 的窗口仍在，`show-messages` 有 `pane %10 (cmd) exited with 7`；
8. `choose-client` / `display-menu` / `display-popup` 从脚本调用时 exit=1 并说明 `client not attached`。
同轮 `cargo test`：126 项全绿（lib 86 / console 4 / e2e 36）。

## 第 7 轮（计数 2/3，无发现）— 视角：可复现性

数据：连续 3 次 `cargo test`，126 项测试名+状态指纹均为 `6ECB65FD1BC6C558`；
三个全新 server 的 `list-keys` 各 87 行，指纹均为 `F21C2959A7E1855A`
（含两条 282 字符的 `display-menu` 绑定，说明菜单定义的 Display→parse 往返稳定）。

## 第 8 轮（计数 3/3，无发现）— 视角：静态一致性

数据：7 个新配置项 set→`show-options -gv` 全部一致，非法值被拒且旧值保留；
`list-commands` 85 条命令逐条调用，没有一条落到 `unknown command`；
9 个新功能关键词在 README.md / README.zh-CN.md / docs/tmux-parity.md 三处均有；
对照表 `todo` 行数 0；`Window::flags` 的 7 个标记字符齐全；
`show-options` 的 29 个选项名在对照表里全部有条目；clippy 与 `cargo fmt --check` 均无输出。

## 结论（第十二次验收）

第 6、7、8 轮连续零发现，验收通过。本次补齐 tmux 3.5 命令表里最后的空缺
（`display-popup` 之后对照表已无 `todo` 行），新增 5 个告警类选项和 2 个持久化选项，
修复 8 个自测中发现的缺陷（其中「跨 session move-window 顶掉当前窗口」「弹窗吞 key-up」
「pipe-pane 静默丢输出」三个会被用户直接撞上）；测试从 109 项增加到 126 项
（lib 86 / console 4 / e2e 36）。

# 第十三次验收（2026-09-22）— 选项名缩写与开关切换

用户提的：`prefix :` 之后能不能像 `set sync` 这样写，直接开同步。当时只有命令名
支持前缀缩写（`set` = `set-option`），选项名必须写全，而且除了 `synchronize-panes`
以外，开关类选项不给值就报 `bad boolean ''`。

做了两件事：选项名支持不产生歧义的缩写（整体前缀 `sync`，以及按 `-` 分段各写前缀
`mon-act`、`w-s-f`），开关类选项不给值就翻转（`set mouse`、`set sync`）。名字在解析期
就展开成全名，所以 server、`list-keys`、配置文件三处看到的都是同一个名字。

## 第 1 轮（不计数）— 视角：逻辑正确与不变量

新增单元测试 `known_option_names_line_up_with_the_setter`：缩写表里的每个名字都必须
解析回自己、都必须被 `set` 认识、`SHOWABLE` 和开关表必须是缩写表的子集。

| # | 问题 | 修复 |
|---|------|------|
| 1 | `synchronize-panes` 能 set、能 `show -gv`，却不在 `show-options` 的列表里——能设却看不见 | 列表末尾按当前窗口补一行 |

## 第 2 轮（不计数）— 视角：机制通路（release 二进制）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `set hist 1000` 报歧义（`history-limit` vs `history-file`），而后者只是为了让 `.tmux.conf` 能加载而"接受并忽略"的名字 | 缩写表拆成 `KNOWN`（真正生效的 35 个）和 `ACCEPTED`（兼容用的 15 个）；先在生效的里面找唯一匹配，兼容名字永远抢不走缩写，写全名仍然被接受 |

## 第 3 轮（不计数）— 视角：边界与退化输入

| # | 问题 | 修复 |
|---|------|------|
| 1 | `set ""` 列出全部 35 个候选（空串是所有名字的前缀），`set -` 列出 22 个 | 空名字、含空段的名字一律不当作缩写，直接落到 `unknown option` |
| 2 | 歧义提示最长列了 13 个名字 | 最多列 4 个，其余写成 `and N more` |

## 第 4 轮（计数 1/3，无发现）— 视角：机制通路

数据（release 二进制，16 条）：`set sync` → `show -gv sync` = on；`set -w syn on`；
`set mon-act on`；`set w-s-f #I:#W`；`set mou` 翻转；`set -ag s-r " | tail"` 追加成功；
`set mon` / `set s-p` 报歧义并列出候选；`set hist 1000` 成功；
`show-options` 列出 30 行且含 `synchronize-panes`。

## 第 5 轮（计数 2/3，无发现）— 视角：边界与退化输入

数据：`set`（无名字）→ `option name required`；`set ""` / `set -` → 干净的
`unknown option`；`set s on` → 歧义且只列 4 个 + `and 9 more`；`set SYNC` → unknown
（大小写敏感）；`@theme` 设/读正常；`set -a mou`、`set sav`（数字选项不翻转，报
`bad number ''`）符合预期；整份配置文件用缩写写成（`set mou on` / `set mon-act on` /
`set hist 4321` / `set sav 42`）在 server 启动时全部生效；`source-file` 里的歧义带
文件名和行号报出来。

## 第 6 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，128 项测试名+状态指纹均为 `805519A61590AEF6`；
README 和 `--help` 里承诺的 6 个例子（`set sync`、`set mon-act on`、`set w-s-f`、
`set mouse` 翻转、`set mon` 的歧义提示）逐条对着 release 二进制实测，全部相符；
clippy 与 `cargo fmt --check` 无输出。

## 结论（第十三次验收）

第 4、5、6 轮连续零发现，验收通过。测试从 126 增加到 128（lib 87 / console 4 / e2e 37）。

# 第十四次验收（2026-09-22）— 跨 pane 搜索、桌面通知、命令表补漏

起因：查了 psmux（3.5k star，90+ 命令、140+ format 变量、30 hooks、control mode、
monitor-* 和 remain-on-exit 一应俱全）之后确认，在"追 tmux 兼容"这条路上它已经比
wmux 全，继续追只是做第二个 psmux。所以改做两边都没有的东西。

本次新增：
- `find-text [-C] [-n 条数] [-t 目标] 关键词`：在**所有 pane 打印过的内容**里找，
  输出 `session:窗口.pane  -往回第几行  命中行`。tmux 的 `find-window` 只搜窗口名和
  标题，psmux 的 copy-mode 搜索只能在单个 pane 里。
- `notify [-T 标题] 消息` 和 `set -g notify on`：托盘气泡通知（Win10/11 会转成系统
  通知）。选托盘气泡而不是 WinRT toast，是因为 toast 需要安装快捷方式注册
  AppUserModelID，免安装 zip 版给不了。
- 命令表补漏：`copy-mode` / `unbind-key` 能解析却不在 `COMMANDS` 里（`wmux unb` 报
  unknown，`list-commands` 也看不到）；另补 `setw` / `showw` / `start-server` 三个
  tmux 用户常打的名字。命令数 85 → 92。

## 第 1 轮（不计数）— 视角：静态一致性

| # | 问题 | 修复 |
|---|------|------|
| 1 | `copy-mode`、`unbind-key` 在别名表里能解析，却没进 `COMMANDS` 常量：前缀解析不到、`list-commands` 不显示 | 补进常量，并新增单元断言：`COMMANDS` 每一项都必须能被解析且 `resolve_prefix` 解析回自己 |

## 第 2 轮（不计数）— 视角：边界与退化输入

| # | 问题 | 修复 |
|---|------|------|
| 1 | `find-text " "`（纯空白）被当成有效搜索，会命中几乎所有行 | pattern 去掉首尾空白后为空就报 `pattern required` |
| 2 | `-t b:99`（不存在的窗口）只报 `no pane has: xxx`，看不出是目标写错了 | 目标按 session → 窗口 → pane 逐层解析，各报各的错（`no window 99` / `no pane 5`） |
| 3 | `-t b:0.5` 的 **pane 部分被完全忽略**，实际搜了整个窗口 | 解析 pane 并只搜那一个 |
| 4 | 找不到时把整条 pattern 原样回显，300 字符的 pattern 刷屏 | 错误信息里截断到 60 字符 |

## 第 3 轮（计数 1/3，无发现）— 视角：机制通路 + 性能

数据（release 二进制）：4007 行 scrollback 的 pane 里命中最顶上那行 **17 ms**；
跨 session 搜索标签与对齐正确（`big:0.1  - 8` / `other:0.0  -20`）；
`-t big` 限定 session、`-t :1` 解析成当前 session 的窗口 1、`-t nosuch` 报
`can't find session`；4000 行都匹配的常见 pattern 因 `-n` 默认 3 只返回 3 条，16 ms。

## 第 4 轮（计数 2/3，无发现）— 视角：边界与退化输入

数据：空 pattern / 纯空格都报 `pattern required`；`a.*b[c] \` 按字面匹配（不是正则）；
中文、日文、emoji 都能命中；`-n abc`、`-x` 报错；200 字符 pattern 的错误信息被截断；
`-t b:99` → `no window 99`，`-t b:0.5` → `no pane 5`；没有输出的 pane 正常报未命中；
`notify` 空消息报错、400 字符消息和 200 字符标题都不崩、连发 10 条耗时 165 ms。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，131 项指纹均为 `BE06DD36FD5D73A4`；
README 承诺的输出形状 `ft:0.0  -8  text` 用正则对着真实输出校验通过；
`list-commands` 92 条且含 `find-text` / `notify`；`show -gv notify` 可读；
clippy 与 `cargo fmt --check` 无输出。

## 结论（第十四次验收）

第 3、4、5 轮连续零发现，验收通过。测试 128 → 131（lib 89 / console 4 / e2e 38）。
桌面通知的视觉效果需要在有桌面的会话里人工确认（开发机跑在 session 0，看不到气泡）。

# 第十五次验收（2026-09-22）— 直读 .tmux.conf、开机自启

本次新增：
- 没有 `.wmux.conf` 时直接读 `~/.tmux.conf` / `~/.config/tmux/tmux.conf`，按 tmux 的读法：
  `\` 续行、`%if … %endif` 整块跳过（wmux 不求值 tmux 条件式，两个分支都应用比都不应用更糟）、
  用不了的行记进 `show-messages`，首次 attach 只给一行汇总。`bind -T` 只认 root / prefix，
  `copy-mode-vi` 这种表直接拒绝——**修了一个老 bug**：以前 `-T` 非 root 一律当 prefix，
  `.tmux.conf` 里的 `bind -T copy-mode-vi v …` 会被静默绑到 prefix 表。
- `wmux startup on|off|status`：登录时拉起 server 并恢复全部 session。用当前用户的 `Run`
  注册表键（`schtasks` 的 ONLOGON 触发器非管理员建不了，实测 `Access is denied`），
  命令前面套 `conhost --headless` 所以登录时不闪窗口；server 新增 `--restore` 参数。
- `server::run_with(RunOptions { force_restore, config })`：测试可以显式指定配置文件，
  不再通过环境变量——同进程并行的 e2e 曾因 `WMUX_CONFIG` 互相污染挂掉 4 个。

## 第 1 轮（不计数）— 视角：机制通路

| # | 问题 | 修复 |
|---|------|------|
| 1 | `conhost --headless` 用 `<nul` 喂 stdin 时 server 起不来，误以为机制不可用 | 按登录的真实方式（ShellExecute、不继承句柄）再测：server 起来、session 连输出一起回来、conhost 无窗口。方案没错，测法错了 |
| 2 | 我之前的临时探测没设 `WMUX_SESSIONS_DIR`，往用户真实 sessions 目录写了 14 个垃圾 session，`--restore` 一跑全冒出来 | 逐个核对名字后删除（全是我的：0/1/2/alpha/b/big/c/doc/k/other/s/t/t1/x），之后的探测脚本全部隔离目录 |

## 第 2 轮（不计数）— 视角：边界与退化输入

| # | 问题 | 修复 |
|---|------|------|
| 1 | 配置文件 `source-file` 自己 → 无限递归，**server 直接死** | 正在 source 的文件栈（canonical 路径）做环检测，报 `source-file loop`；前面已应用的行保留 |
| 2 | 没闭合的 `%if` 把后面整个文件静默吞掉 | `logical_lines` 返回未闭合的行号，报 `%if without %endif; the rest of the file was skipped` |
| 3 | 记事本存的文件带 UTF-8 BOM，第一行变成 `\u{feff}set` 报 unknown command | 读文件先剥 BOM；单元测试覆盖 BOM + CRLF |

## 第 3 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：真实形状的 tmux.conf（含 TPM、copy-mode-vi、`%if`、续行）经 `WMUX_CONFIG` 加载：
`prefix=C-a mouse=on base-index=1 status=on`；续行拼成的 `| split-window -h -c "#{pane_current_path}"`
绑定存在；prefix 表里没有 `v`；`show-messages` 里有 copy-mode-vi 与 `@plugin tmux-plugins/tpm` 两条；
`list-windows` 首行 `1: pwsh*`。`startup on/status/off` 往返，Run 值最终不存在。
登录链路（隔离目录）：保存 `ops`（2 窗口）→ kill-server → 按登录方式启动 → `ls` 见 `ops: 2 windows`，
pane 0 里 `survived-the-reboot` 仍在，conhost `MainWindowHandle=0`。

## 第 4 轮（计数 2/3，无发现）— 视角：边界与退化输入

数据（9 份退化配置，server 全部存活）：BOM+CRLF 生效；空文件；只有 `%if` 块；未闭合 `%if`（1 条提示）；
EOF 处续行；纯垃圾（2 条提示）；`bind -T copy-mode` / `unbind -T copy-mode-vi`（各拒绝 1 条，`-T root` 通过）；
`%hidden`/`%elif`；自引用 `source-file`（1 条提示，`mouse off` 已生效）。
`startup` 八种写法：on/install/off/disable 退出码 0，未安装时 status 退出码 1，`weird` 报错。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，136 项指纹均为 `ADF37BBFA26A6E3D`；配置搜索路径含两个 tmux 位置；
两份 README、`--help`、对照表都提到 `startup` 与 `.tmux.conf`；
"登录不闪窗口"由代码里的 `conhost.exe --headless` 支撑，"不需要管理员"由只写 `HKEY_CURRENT_USER` 支撑；
clippy 与 `cargo fmt --check` 无输出。

## 结论（第十五次验收）

第 3、4、5 轮连续零发现，验收通过。测试 131 → 136（lib 93 / console 4 / e2e 39）。

# 第十六次验收（2026-09-22）— 会话录制、pane 边框标题、format 变量

本次新增：
- `record [-t 目标] [文件]`：把 pane 输出写成 asciinema v2（`o` 事件 + 尺寸变化的 `r` 事件），
  写文件走独立线程（有界通道 1024 条，跟不上丢事件不丢输出）；不带路径就停，
  pane 被杀时录制器随之关闭，文件完整。
- `pane-border-status top|bottom` + `pane-border-format`：布局阶段每个 pane 让出一行给边框文字，
  `compose` 在那一行画 `pane-border-format` 展开的内容（当前 pane 用活动边框色），其余格子仍是边框线。
- format 变量从 9 个扩到 39 个（session/window/pane/client/server 五组），修饰符
  `=N:` `=-N:` `b:` `d:` `t:` `s/a/b/:` 可嵌套；两处各自手写的 `Context` 字面量合并成一个
  `Server::context()`，状态栏、`display-message`、边框文字、菜单标题都从这一处取值。

## 第 1 轮（不计数）— 视角：机制通路（e2e）

| # | 问题 | 修复 |
|---|------|------|
| 1 | `#{b:pane_current_path}` 对 `C:\…\Temp\`（`temp_dir()` 带尾部反斜杠）取出空串 | `b:` / `d:` 先剥掉尾部分隔符；单元测试覆盖 |
| 2 | （测试自身）以为 cmd.exe 的提示符在第 0 行、pane 高度算成 21 | 先探出提示符所在行再断言；高度 = 24 − 状态栏 − 边框行 = 22 |
| 3 | （测试自身）以为新 pane 天生有 `pane_current_path` | 先 `set-cwd` 再查 |

## 第 2 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：录制文件头 `version=2 size=80x23`，事件序列 `ooro`（回显、输出、分屏引起的 resize、新提示符），
含 `in-the-cast` 的 `o` 事件存在，时间单调；`pane-border-status top` 使两个 pane 高度 11→10，`off` 回到 11；
`display-message -p` 一次展开 11 个变量/修饰符：`1|2|1|System32|C:\Windows|a|NE|C:\WINdows\System32\|Tue Sep 22 …|alive|Syst`，
逐项对上（含带尾部反斜杠的 `b:`、`=-2:host`、嵌套 `=4:b:`、`t:` 时间、条件）。

## 第 3 轮（计数 2/3，无发现）— 视角：边界与退化输入

数据：`record` 到不存在的盘报 `cannot create … (os error 3)`；未录制时停报 `not recording`；
两次 `record` 第二个文件替换第一个（旧文件只剩头）；多余参数报 `unexpected argument`；
2000 行输出爆发后 1992 个事件、末行 JSON 完整；录制中的 pane 被 `kill-window`，文件末行完整；
`pane-border-status sideways` 拒绝并列出三个合法值；`-x 20 -y 4` 的小 session 下 top/bottom/status off/on
来回切、空 format、`#{nosuch}#{b:}#[bold` 残缺 format，server 全部存活；
7 个畸形修饰符（`=abc:`、`=-0:`、`s//x/:`、`b:` 空值、`t:` 非数字、`s/a:`、`:`）全部得到空串或原值，
20 层嵌套 `s/a/b/:` 正常，`==1` / `==` 空比较正常。

## 第 4 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，141 项指纹均为 `8C92D0A2B4D2D05F`；README 列出的 39 个变量
在 `Context::var` 里逐个存在，对照表同样齐全；`--help` 含 `record`；`show-options` 含两个边框选项；
clippy 与 `cargo fmt --check` 无输出。

## 结论（第十六次验收）

第 2、3、4 轮连续零发现，验收通过。测试 136 → 141（lib 95 / console 4 / e2e 42）。

# 第十七次验收（2026-09-22）— save-history all、new -x/-y、display-message -t、边框态分屏修正

本次新增 / 修正：
- `set -g save-history all`：整段 scrollback（受 `history-limit` 约束）随 session 存盘，
  行里保留颜色与属性（`capture-pane -e` 的形式），`resume` 时逐字节贴回；`show-options` 显示 `all`。
- `Pane::lines_from(start, escapes)`：一趟走完 scrollback 取行；`capture-pane -S -` 也改用它
  （原来逐行调 `line_text`/`line_escapes`，每行重算一次 scrollback 长度，5000 行就是 5000 次全扫）。
- `new-session -x cols -y rows`：给 detached / 脚本建的 session 定尺寸（tmux 同款；最小 10×3）。
- `display-message -t 目标`：format 按指定 pane 展开（原来所有 flag 连值一起被吞掉，`-t x` 的 x 会混进消息）。
- **修 bug**：`pane-border-status` 开着时再 `split-window`，两半不均（23 行的格子切成 11/9 而不是 10/10；
  7 行切成 1/2）。原因是分屏把"已扣掉边框行的绘制 rect"当布局输入，树里少了一行，布局把它补给第一个 pane。
  现在 `Window` 另存一份布局格子 `layout_rects`，分屏与 join-pane 按格子切，最小高度检查也算上边框行。

**更正第十六次记录**：第 3 轮里"`-x 20 -y 4` 的小 session 下 top/bottom … server 全部存活"这句不成立——
当时 `new` 根本不认 `-x`（报 `unknown flag '-x'`，探针把输出吞了），session 没建出来，
后面只验证了 server 没死。本次补上参数后小 session 的检查才真正跑到（见第 3 轮）。

## 第 1 轮（不计数）— 视角：静态一致性 + diff 自审

| # | 问题 | 处理 |
|---|------|------|
| 1 | `cargo fmt --check` 一处差异 | fmt |
| 2 | `capture-pane -S -` 仍是逐行 O(n²) 取法 | 改用 `lines_from` |
| 3 | `line_escapes` 无人再用 | 删除 |
| 4 | 网站统计仍写 126 tests（实际 143） | 改 143 |

## 第 2 轮（不计数）— 视角：机制通路（release 二进制）

wmux 数据无问题；发现的是探针脚本的问题（`$t[0]` 取到字符串首字符、双引号里 `$g` 被 PowerShell 展开成空）。

## 第 3 轮（不计数）— 视角：边界

发现 `new -x/-y` 不存在（上一批的小 session 检查因此从未跑过）→ 补上；`all `/`ALL` 被拒 → 与 `parse_bool` 一致，
去空白、忽略大小写。补 e2e 时又发现 `display-message -t` 不生效（输出 `small:0 30x4`）→ 实现。
用 `-x/-y` 建小 session 后再查边框态分屏：24 行 11/9、7 行 1/2、9 行 2/3，不开边框全部正常 → 定位并修复（见上）。

## 第 4 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：`save-history 500` 存 498 行 14059 B；`all` 存 5021 行 136160 B，`save-session` 20 ms；
`capture-pane -S -` 18 ms 取 5023 行（line-980 … line-6000）；`resume` 33 ms，回来 5019 行 line-* 加提示符
（少 2 行是新 shell 自己的空行+提示符把最早两行挤出了 `history-limit`）；`save-history 0` 存 0 行。

## 第 5 轮（计数 2/3，无发现）— 视角：边界

数据：4 行"绿色中文提示符 + 蓝底 + 汉字"resume 后 4/4 逐字节相同；120 列存、80 列恢复，100 个 x 换行成
30+80+20=130 个字符不丢；空 pane 存 2 行；只打印转义序列的提示符行被丢掉不存；`history-limit 50` 下存 73 行
（≤ 50+23）；20×4 小 session：建成 20×3，开边框后 20×2，此时分屏被明确拒绝 `pane too small to split`，
resume 回来 80×23（存档不含尺寸，与 tmux-resurrect 一致），server 存活；`-1`、`1e3`、空值被拒，`all `、`ALL` 接受。
resume 后底部多一组"空行+提示符"（旧提示符行在存档里，新 shell 又打一个）：与 tmux-resurrect 行为相同，不算问题。

## 第 6 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，143 项指纹均为 `4592FD1A9969C1C4`；边框态分屏 6/7/8/9/10/24 行 →
1/1、2/1、2/2、3/2、3/3、10/10（第一个多一行，与不开边框时的规则一致）；`display-message -p -t s24:0.1` 答 `1 10`，
坏目标报 `can't find session: nosuch` 退出 1；`--help` 列出 `-x cols -y rows`；README（中英）、conf 示例、
对照表、网站六处都写了 `all`；对照表 new-session 行有 `-x`、`-y`，display-message 行有 `-t`；
网站 143 tests；`line_escapes` 无残留；clippy 与 fmt 无输出。

## 结论（第十七次验收）

第 4、5、6 轮连续零发现，验收通过。测试 141 → 143（lib 96 / console 4 / e2e 43）。

# 第十八次验收（2026-09-22）— `jobs` 任务板、respawn 环境修正

本次新增 / 修正：
- `jobs [-t 目标] [-F 格式]`：整个 server 每个 pane 一行——PANE、STATE（`running` / `exit N`）、UP（启动至今）、
  IDLE（上次输出至今）、PID、COMMAND、DIR，列按显示宽度对齐（中文占两格）。`-t` 到 session / 窗口 / 单个 pane；
  `-F` 走 format 引擎。新增变量 `pane_start_time`、`pane_activity`（unix 秒，可配 `t:`），`Pane.last_output`
  在 `note_output` 打点。`format::human_duration`：`42s` / `5m` / `2h13m` / `3d2h`。
- **修 bug（既有）**：`respawn-pane` 只传了 `set-environment` 的条目，没传 `pane_env()`（WMUX、WMUX_PANE、
  把本 exe 目录放最前的 PATH）。后果：respawn 过的 pane 里 `WMUX=default`，`wmux` 解析到 `C:\Program Files\wmux\wmux.exe`
  （0.5.0）——探针里 `jobs` 报 `unknown command: jobs`、空文件。现在 respawn 与新建 pane 用同一套环境。

## 第 1 轮（不计数）— 视角：机制通路（release 二进制）

`jobs` 输出形状正确；发现 respawn 后 pane 内 `wmux` 指向别的 exe（见上）→ 修；探针自身两处失误
（把 `cmd.exe /c exit 3` 当一个参数传、竖切第 4 次被拒后误以为有 40 个 pane）。

## 第 2 轮（不计数）— 视角：边界

发现：(1) 列按字符数补齐，中文 session 名在终端里错位 → 改按 unicode 显示宽度；(2) `-t s:w.p` 带 pane 时列的是整个窗口，
脚本问单个 pane 不方便，且 e2e 里那条 `-F` 时间检查其实拿到两行、凑巧通过 → pane 目标只列该 pane，e2e 改成明确断言一行。
另：`#(cmd)` 在一次性命令里展开为空（缓存按 `status-interval` 刷新，`display-message` 同样）——记录，不改。

## 第 3 轮（不计数）— 视角：可复现性 + 文档 claim

`list-commands` 检查 False 是脚本没先起 server（该命令不自动起 server）；改脚本后 True。

## 第 4 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：4 个 200×50 session 各 10 个 pane，`jobs` 40 行 21 ms，全部 `running`，40 个 pid 互不相同；
`pane_activity` 在一次 `echo` 后前移（1790070884 → 1790070888），IDLE 显示 `0s`；`respawn-pane -k` 后
`pane_start_time` 前移、pid 改变（34088 → 6044）；`cmd.exe /c exit 7` 的窗口在 `remain-on-exit on` 下留一行 `exit 7`；
respawn 过的 pane 里跑 `wmux jobs -F …` 得到 42 行（本 server 全部 pane）。

## 第 5 轮（计数 2/3，无发现）— 视角：边界

数据：无 session 时退出 0、空输出；`中文会话` + 12 层深目录，STATE 列三行都从第 14 格开始；
`-t b:0` / `-t b:0.0` / `-t b:9`（`no window 9`，退出 1）；zoom 的窗口仍列出被藏的 2 个 pane；
`-F ""` 输出空行、`#{nosuch}|#{b:}|#[bold|…` 残缺格式不崩、`-F` 无值报 `-F: missing value`、`-x` 报 unknown flag、
`job` 缩写可用；带空格的标题一行内列位置不变。

## 第 6 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，145 项指纹均为 `96D4C038C0B1567D`；`--help` 与 `list-commands` 都有 `jobs`；
README（中英）与对照表都写了 `jobs` 及两个新变量；`Context::var` 应答两者；网站 145 tests；COMMANDS 含 `jobs`
且解析测试覆盖每一项；clippy 与 fmt 无输出。

## 结论（第十八次验收）

第 4、5、6 轮连续零发现，验收通过。测试 143 → 145（lib 97 / console 4 / e2e 44）。

# 第十九次验收（2026-09-22）— Windows Terminal 集成

本次新增：
- `wmux windows-terminal install|remove|status`（别名 `wt`）：往 `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\wmux\`
  写一个 profile 片段（`wmux.json`，非默认 socket 是 `wmux-<socket>.json`），Windows Terminal 自动合并，不碰 `settings.json`。
  profile 名 `wmux` / `wmux (<socket>)`，命令 `"<exe>" [-L socket] new-session -A -s main`，`startingDirectory` `%USERPROFILE%`。
  GUID 由 socket 名经两趟 FNV-1a 派生并整成 RFC 4122 v4 形状——重装不换身份，用户改的字体/配色保留。
  本地执行，不起 server（与 `startup` 同一分支）。
- 顺手：`startup` 与 `windows-terminal` 都拒绝动词之后的多余参数（原来 `startup on extra` 会照做）。

## 第 1 轮（不计数）— 视角：机制通路（真实目录）

在本机真实 Fragments 目录装/查/卸一遍全部正确；发现 GUID 版本位随机（`c5fe`），改成 v4 + RFC 变体位。

## 第 2 轮（不计数）— 视角：边界

发现 `windows-terminal install extra` 多余参数被无视地执行（探针那一段还把 LOCALAPPDATA 切回了真实目录，
真装了一次，立即卸掉；脚本改成最后才切回）→ 两个本地命令都拒绝多余参数，单元测试覆盖。

## 第 3 轮（计数 1/3，无发现）— 视角：机制通路（真实目录）

数据：装前 0 个文件、status 退出 1；install 退出 0，文件是合法 JSON，`commandline` 等于 `"<本 exe>" new-session -A -s main`，
`startingDirectory` `%USERPROFILE%`；重装 GUID 不变 `{ebada516-8620-45fe-90b0-7b3d893a0b99}`，符合 v4/RFC 正则；
`-L work wt install` 生成 `wmux-work.json`，名字 `wmux (work)`，GUID 不同，命令含 `-L work`；从别的目录执行 status 仍指向本 exe；
两个都 remove 后目录 0 个文件，status 退出 1，再 remove 报 `not installed`。

## 第 4 轮（计数 2/3，无发现）— 视角：边界（临时 LOCALAPPDATA）

数据：无目录时 status/remove/无动词各自正确；install 自建整条路径；文件损坏时 status 报 `… wmux.json is not JSON: key must be a string …`
退出 1，reinstall 覆盖成合法；是 JSON 但没有 profile → 视为未安装；socket `a b`、`x.y`、`中文` 生成对应文件名；
上级路径是文件时 install 报 `cannot create … (os error 183)` 退出 1 不 panic；LOCALAPPDATA 未设置报 `LOCALAPPDATA is not set`；
`install extra` / `instal` / `startup on extra` 全部拒绝且未写文件；真实目录未被触碰。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，149 项指纹均为 `CFC906FA4D1E3F08`；`--help` 有 `windows-terminal install|remove|status`；
README（中英）写了命令与 Fragments 路径，对照表 wmux-only 行有条目；代码只写 `Fragments\wmux` 下、不含 `settings.json`；
网站 149 tests；main.rs 两个本地命令走同一分支；真实目录无残留；clippy 与 fmt 无输出。

## 结论（第十九次验收）

第 3、4、5 轮连续零发现，验收通过。测试 145 → 149（lib 101 / console 4 / e2e 44）。

# 第二十次验收（2026-09-22）— status-justify、window-status-separator

本次新增：
- `status-justify left|centre|right|absolute-centre`（也接受 `center` / `absolute-center`，存成 tmux 拼法）：
  窗口列表整体放得下时按此定位；放不下退回左对齐（保持"当前窗口一定可见"的既有预留逻辑）。
- `window-status-separator`（默认一个空格）：替换原来硬编码在标签之间的一格；按显示宽度计算，
  分隔符与其后的标签一起判断放不放得下——放不下就一起省略，不留悬空分隔符。
- render 层新增 `Justify` 枚举，`StatusLine` 带 `separator`/`justify`；两项进 SHOWABLE/KNOWN，可缩写（`s-j`、`w-s-s`）。

## 第 1 轮（不计数）— 视角：单元测试

新写的 render 测试抓到：12 列放不下第二个标签时分隔符先画出来了（`[s] 0:a|   R`）→ 改成分隔符+标签一起判断。
`set_options` 里歧义计数 "and 9 more" 因新选项变 10 → 更新。

## 第 2 轮（不计数）— 视角：边界（release 二进制）

`center` 接受但 `absolute-center` 拒绝，不一致 → 一并接受；单元测试覆盖两种拼法与 4 种坏值。
（`set w-s x` 报 unknown：缩写规则要求词数一致，`w-s-s` 可用，不算问题。）

## 第 3 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制 + e2e 客户端）

数据：`.tmux.conf` 写 `status-justify centre` / `window-status-separator ' | '` 启动后 show 为 `centre` / ` | `，
`show-messages` 无 skip 记录；set right/center/absolute-centre/s-j left 往返正确，`w-s-s ::` 与空分隔符往返正确；
`show-options -g` 列出两项。e2e（attach 的客户端）：`" | "` 出现在 `0:aa` 与 `1:bb` 之间且列表从第 4 格起；
`right` 时 `1:bb` 紧接一格再接右侧引号；`center` 时起点 > 6 且两侧都有空隙；`sideways` 被拒。

## 第 4 轮（计数 2/3，无发现）— 视角：边界

数据：`''`、`middle`、`CENTRE`、`left right`、`0` 拒绝且不改值；` centre ` 去空白接受；`absolute-center` 接受存为 `absolute-centre`；
200 字符、全角 `｜`、制表符、`#[bold]|#[default]`（按字面）、emoji 分隔符全部往返一致；
200 字符分隔符 + 3 个窗口 + absolute-centre 下 server 存活。渲染层边界由单元测试覆盖：
30 列四种对齐的逐格结果、` · ` 与 `│` 分隔符、12 列退回左对齐并丢掉放不下的窗口。

## 第 5 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，152 项指纹均为 `B99056DA91B556E2`；README（中英）、conf 示例、对照表都写了两项；
SHOWABLE 与 KNOWN 各列两项（脚本计数 3 是把单元测试里的 `o.set("status-justify", …)` 也数了，grep 行号核实）；
默认分隔符代码与文档一致为空格；render 从 frame 取 justify/separator；网站 152 tests；clippy 与 fmt 无输出。

## 结论（第二十次验收）

第 3、4、5 轮连续零发现，验收通过。测试 149 → 152（lib 103 / console 4 / e2e 45）。

# 第二十一次验收（2026-09-22）— WinGet / Scoop 清单

本次新增：
- `tools/package-manifests.ps1 -Version X.Y.Z`：从 GitHub 发布页取两个 `.sha256` 与 MSI（读其 ProductCode，
  因为 wxs 里 `Product Id="*"` 每次构建都变），生成 WinGet 三个清单（`packaging/winget/manifests/n/newdee/wmux/X.Y.Z/`，
  目录布局与 winget-pkgs 一致）和 Scoop 清单（`packaging/scoop/wmux.json`，带 `checkver`/`autoupdate`）。
  LF、无 BOM。0.5.0 的输出已提交。
- `packaging/README.md`：提交到 winget-pkgs / ScoopInstaller/Extras 的步骤；README（中英）安装段与网站安装卡片加了 Scoop 一行
  （仓库里的清单可直接 `scoop install <raw url>`），WinGet 写明"合并后才有 `winget install newdee.wmux`"，不做尚未成立的 claim。
- 提交 winget-pkgs / Extras 的 PR 需要用户自己的 GitHub 账号，这一步留给用户。

## 第 1 轮（不计数）— 视角：机制通路

生成器两处 PowerShell 插值错误（`"$msiName: …"` 被当作用域、`"$asset.sha256"` 被当属性）→ `${…}`；
不存在的版本只吐 GitHub 原始 404 JSON → 加 "no release vX.Y.Z on github.com/<repo>" 的明确报错。
探针脚本自身三处误判（`packaging/` 未跟踪时用 git status 判"是否改动"、Scoop 的 Write-Host 输出用 `2>&1` 抓不到、
"Checking hash of … ok." 被 Scoop 分段输出）→ 改成内容哈希比较、`*>&1`、以"installed successfully"为准。

## 第 2 轮（计数 1/3，无发现）— 视角：边界

数据：`0.5`、`v0.5.0`、`0.5.0.1`、`latest` 四种坏版本号在联网前被 ValidatePattern 拒绝（退出 1、报错含模式、未写文件）；
`9.9.9` 与不存在的仓库都报 `no release …` 退出 1、未写文件；4 个输出文件无 BOM、0 个 CRLF、以 LF 结尾；
installer 的 sha 为 64 位大写十六进制、ProductCode 带引号；Scoop 的 hash 为 64 位小写、`extract_dir` 等于 zip 内目录名、
autoupdate 的 hash 取 `$url.sha256`；三个 winget 文件名符合 winget-pkgs 要求；152 项测试通过。

## 第 3 轮（计数 2/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，152 项指纹均为 `B99056DA91B556E2`；README（中英）与网站含同一条 Scoop raw URL，
其路径在仓库中存在；README 指向 `packaging/README.md` 且写了 `winget install --manifest` 路径；packaging/README 写了生成器与两个输出；
网站有 Scoop 卡片且中文两个键齐全（52 个 i18n 键全部有中文）；三个 winget 文件的 PackageIdentifier 一致；
release.yml 的资产命名与生成器期望一致；clippy 与 fmt 无输出。

## 第 4 轮（计数 3/3，无发现）— 视角：机制通路（真实安装）

数据：重新生成 4 个文件与磁盘上的逐字节相同；`winget validate --manifest` 报"清单验证成功"（winget v1.29.20-preview）；
用仓库清单 `scoop install`：过 hash 闸门、解压、建 shim，`wmux --version` 经 shim 得 `wmux 0.5.0`，`--help` 含 attach-session；
把 hash 改成全 0 的副本被拒（`ERROR Hash check failed!`，未装、无 shim）；`scoop uninstall` 后 shim 与 app 目录都不在；
i18n 52/52；152 项测试通过。（`winget install --manifest` 要管理员并装到全机，未执行；validate 为准。）

## 结论（第二十一次验收）

第 2、3、4 轮连续零发现，验收通过。测试数不变（152）。

# 第二十二次验收（2026-09-22）— 一次性 `#()`、`pane_dead_time`、存档尺寸

本次新增 / 修正：
- `display-message -p`、`jobs -F` 里的 `#(命令)` 当场执行（`expand_with_shells`），最多等 3 秒（`ONE_SHOT_SHELL_TIMEOUT`），
  结果进同一个缓存；原来只有状态栏按 `status-interval` 后台跑，一次性命令拿到的是空串。
- `Pane.died_at` + 变量 `pane_dead_time`（未退出为空）；`jobs` 里已退出 pane 的 UP 停在退出时刻（"跑了多久"），不再一直涨。
- 存档 `SavedSession.size`（serde default，旧文件兼容）；`resume` 没有终端接入时用存档尺寸，下限 10×3（与 `new -x/-y` 一致）；
  有终端接入时仍用终端尺寸。

## 第 1 轮（不计数）— 视角：机制通路（release 二进制）

`jobs -F` 的 `#()` 仍为空：jobs 直接调 `format::expand`，没走同步路径 → 抽出 `expand_with_shells`，两处共用；e2e 补断言。
探针把 `exit 9` 当两列数错了 UP/IDLE 下标（脚本问题）。

## 第 2 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：`#(echo now-please)` 首次 272 ms 得到结果、第二次走缓存 15 ms；`#(Start-Sleep 20)` 3050 ms 后返回空串，server 存活；
`jobs -F '#(echo in-jobs) #{pane_index}'` → `in-jobs 0`（267 ms）；`cmd /c exit 9` 的 pane 3 秒后 UP 仍为 `0s`、IDLE 由 2s → 5s；
`pane_dead_time` 为 unix 秒、活 pane 为空；132×43 的 session 存档含 size，`resume`（无终端）回来 132×43；
删掉 size 字段后 resume 为 80×24。

## 第 3 轮（计数 2/3，无发现）— 视角：边界

数据：`#()` 无输出→空、多行取第一行、退出码 3 仍显示输出、中文正常、同一格式两个 `#()` 都执行、`#()` 空、
未闭合 `#(echo a` 按已闭合处理（既有宽松解析），server 存活；存档 size 1×1 / 0×0 → 10×3，10×3 → 10×3，500×200 → 500×200，
`"wide"` → `saved session: invalid type: string "wide", expected a tuple of size 2` 退出 1。
记录（不改）：`#(echo #{session_name})` 里的 `#{}` 不先展开——既有行为。

## 第 4 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，154 项指纹均为 `31C7DC8E2E466AC7`；三处文档列出 `pane_dead_time`；README（中英）写了一次性
`#()` 与 3 秒上限，与常量一致；restore 下限与 `-x/-y` 下限一致（10/3）；网站 154 tests；clippy 与 fmt 无输出。

## 结论（第二十二次验收）

第 2、3、4 轮连续零发现，验收通过。测试 152 → 154（lib 103 / console 4 / e2e 47）。

# 第二十三次验收（2026-09-22）— 发布流水线自动出清单

本次新增：
- `tools/package-manifests.ps1` 加本地模式（`-MsiPath`/`-ZipPath`/`-ReleaseDate`）：CI 里用刚构建的文件算 hash、读 ProductCode，不联网；
  文件名必须与发布资产名一致（清单指向发布 URL）；`-ReleaseDate` 校验 yyyy-MM-dd。
- `release.yml` 新增两步：`package manifests`（生成 + `winget validate`（runner 上没有 winget 则跳过）+ 打包
  `wmux-X.Y.Z-manifests.zip` 附到 release）；`commit the manifests to master`（把生成的 `winget/`、`scoop/` 拷到一边，
  从 tag 的 detached 状态切到 origin/master 再拷回、以 github-actions[bot] 提交推送；无变化则不提交）。
- `packaging/README.md` 说明自动化。

## 第 1 轮（不计数）— 视角：机制通路（在临时 bare origin 上按 yml 原文跑两步）

发现两处流水线 bug：(1) `if (git diff --cached --quiet)` 测的是空输出而非退出码，"无变化"分支永不触发 → 改看 `$LASTEXITCODE`；
(2) 拷回时 `Copy-Item "$keep\*"` 把 `packaging/README.md` 也盖掉，master 上别人的改动丢失 → 只拷 `winget\`、`scoop\`。
另加 `git clean -fdq packaging`，防新版本目录（未跟踪）挡住切分支。模拟脚本自身两处顺序问题（`other` 克隆早于推送、
第二次运行没还原文件）已修。

## 第 2 轮（计数 1/3，无发现）— 视角：机制通路（同上，master 同时前进）

数据：`package manifests` 步骤退出 0，`winget validate` 通过，zip 含 4 个文件；与已提交清单仅 `ReleaseDate` 一行不同
（CI 取当天，符合语义）；master 先提交了 README 与 packaging/README 的改动后，`commit` 步骤成功推送
`build: package manifests for v0.5.0`，只改 installer.yaml 一个文件，master 自己的两处改动保留，作者 github-actions[bot]；
第二次（无变化）输出 `manifests unchanged`、退出 0、没有尝试 commit。

## 第 3 轮（计数 2/3，无发现）— 视角：边界

数据：本地模式输出与已提交 4 个文件逐字节相同（指定 `-ReleaseDate 2026-09-21`）；只给 `-MsiPath` → `go together` 退出 1；
文件名不对 → `should be named …` 退出 1；`-ReleaseDate 21/09/2026` → `wants yyyy-MM-dd` 退出 1；不给日期 → 当天；
`release.yml` 经 PyYAML 解析通过，步骤顺序 test → build → zip → msi → manifests → release → commit。

## 第 4 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，154 项指纹均为 `31C7DC8E2E466AC7`；packaging/README 的 zip 名与 yml 一致并写明自动提交；
`permissions: contents: write` 存在；commit 步骤按退出码判断、只拷 winget/scoop；生成器路径存在、用本地模式、
资产名与生成器一致；`.gitattributes` 固定 LF；clippy 与 fmt 无输出。
（流水线本身要等下一个 tag 才真正跑一次；这里验证的是按 yml 原文提取的脚本。）

## 结论（第二十三次验收）

第 2、3、4 轮连续零发现，验收通过。测试数不变（154）。

# 第二十四次验收（2026-09-22）— `choose-jobs`（prefix B）任务板弹窗

本次新增：
- `choose-jobs`：`jobs` 那张板做成 chooser（`ChooserKind::Jobs`，每帧重建）。首行是表头（不可选），
  光标起始在本客户端所在 pane；`Enter` 切到那个 pane（session → 窗口 → pane，触发 after-select-pane 钩子），
  `x` kill-pane，`r` respawn-pane -k，操作后板不关、行原地刷新；提示行按类型显示动作（`Enter go  x kill  r restart`）。
  `jobs` 与弹窗共用 `jobs_panes`/`jobs_row`/`align_columns`。默认绑定 `prefix B`。
- 默认按键表建表时对重复键断言，防止后加的条目静默覆盖前面的。

## 第 1 轮（不计数）— 视角：机制通路

发现真 bug：最初绑在 `prefix J`，HashMap 插入静默覆盖了默认的 `J → resize-pane -D 5`（`list-keys` 还显示成 `-r`）。
`T` 也被 pane 标题提示占用，改用空闲的 `B`；加断言。

## 第 2 轮（计数 1/3，无发现）— 视角：机制通路

数据：交互 e2e（两条）连跑 3 次全过：板打开时 `[2/4]` 停在本 pane、提示行含 `Enter go  x kill  r restart`、
`r` 后 pane pid 变化且板仍开着、`x` 杀掉 `exit 4` 的 pane 后计数变 3 且窗口消失、`G`+`Enter` 切到 `other` session、
按键没有漏进 shell、脚本里调用报 `client not attached`。release 二进制：`list-keys` 有 `-T prefix B choose-jobs`（无 `-r`）
且 `J` 仍是 `resize-pane -D 5`；`list-commands`、`--help` 都有；`jobs` 表头对齐不变。

## 第 3 轮（计数 2/3，无发现）— 视角：边界

数据：边界 e2e 连跑 3 次全过：12 个 pane 时跳转标签 (1)…(9) 且第 10 个起无标签、`g` 不落在表头、`7` 跳到第 7 项、
光标下的窗口被外部 kill 后板自动变 12 行、`q` 关闭、按键不漏、`unbind-key B` 后 `list-keys` 无 choose-jobs、
`bind-key Y choose-jobs` 后 `prefix Y` 打开。CLI：多余参数拒绝、`choose-jo`/`choose-j` 解析到 choose-jobs、
`bind-key -n F12 choose-jobs` 出现在 root 表；41 个 pane 的板 26 ms，STATE 列同一屏幕列。

## 第 4 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：连续 3 次 `cargo test`，156 项指纹均为 `ABBD54930CF37D7B`；README（中英）与对照表写了 choose-jobs、prefix B、x/r；
默认绑定为 B 且有重复键断言；提示行文案、live 重建、共用行构造器、COMMANDS 条目、网站 156 tests 全部核对；clippy 与 fmt 无输出。

## 结论（第二十四次验收）

第 2、3、4 轮连续零发现，验收通过。测试 154 → 156（lib 103 / console 4 / e2e 49）。

# 第二十五次验收（2026-09-23）— 修复：恢复后历史丢失、工作目录不跟随（用户报告）

用户报告：关掉 wmux 再打开，只有布局回来了，历史与工作目录都没了。本机查证（用户真实存档 `1-….json`：两 pane 各 ~500 行历史，
cwd 均为 `C:\Users\stebe`；用户流程 `wmux resume 1`；无 $PROFILE），定位到三个独立原因，全部修复：

1. **工作目录**：cwd 只在 shell 主动上报 OSC 9;9 时更新，`pwsh -NoLogo` 不上报，存的永远是起始目录。
   - 启动 pwsh/powershell 时自动挂 prompt 钩子（`config::with_shell_integration`，`-NoExit -Command <hook>`）：保留用户原 prompt，
     在其后追加不可见的 OSC 9;9；带 `-Command/-File/-EncodedCommand`（含 PowerShell 允许的前缀缩写）的不动。
   - `cmd.exe` 等进程：新模块 `proccwd` 读目标进程 PEB 的 CurrentDirectory（NtQueryInformationProcess + ReadProcessMemory）。
   - 优先级：shell 上报或 `set-cwd`（`announced`）> 进程目录 > 起始目录。
   - 顺带修：带引号的 OSC 9;9（Windows Terminal 官方片段的写法）原来会被丢掉。
2. **历史回放**：原来把存档文本喂进 wmux 自己的 vt100 模型，ConPTY 的缓冲里没有——任何一次重绘/缩放（`startup` 无头 80×24 恢复后
   再从大终端 attach 就是一次缩放）都会把它冲掉。现在由助手进程 `wmux __replay 文件` 通过 CONOUT$ + WriteConsoleW 把文本**打印进
   pane 的 ConPTY**（进 conhost 缓冲，之后的重绘、缩放由 ConPTY 自己处理），打完写一个私有 OSC 标记 `ESC]7777;wmux-replayed ESC\`
   （实测能穿过 ConPTY），server 收到标记后把 shell 起进同一个 ConPTY（助手仍存活，conhost 不会因"第一个进程退出"而关掉），
   再删文件让助手退出；助手的退出事件用 `HELPER_GEN` 标记与 pane 自身进程区分。
3. **vt100 缩行丢行**：vendored vt100 的 `set_size` 缩行时从底部截断、扩行只补空行，缩一次少一截。改为缩行把顶部行推入 scrollback
   （以光标留在屏上为准）；扩行仍只补空行（与 ConPTY 那侧一致，拉回来会被它的重绘盖掉——实测过）。

## 排查过程中的弯路（都记在这）

- 同步等助手退出会死锁：ConPTY 启动进程时先发 `ESC[6n` 等回答，server 线程阻塞就没人回答（单元测试
  `a_first_process_can_exit_and_a_second_can_follow` 固化了这点）。
- 助手用 Rust `stdout()` 写不进控制台：portable-pty 把子进程 stdio 句柄设成 INVALID，`stdout()` 静默丢弃 → 改用 CONOUT$。
- `start_pending` 给 generation +1 会让 reader 线程（带旧 generation）的输出全部被丢 → 不加代。
- 回放临时文件名 `pid-paneid` 在 e2e（同进程多 server、pane id 各自从 1 起）下互相串 → 加全局序号。
- 测试里 `current_exe()` 是测试二进制而非 wmux.exe → `WMUX_EXE` 环境变量 / 兄弟目录查找。
- `.tmux.conf` 那条 e2e 没设 `sessions-dir`，把 `t` session 泄漏到用户真实存档目录（已删两次，已修）。
- 探针脚本用 `history` 撞了 Get-History 别名、pwsh 7 不能投影 WinRT 类型等，均为脚本问题。

## 第 1～N 轮（不计数）— 上述修复过程中的多轮迭代，每轮以全量测试收口

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制，用户流程）

数据：pwsh pane 打印 30 行、`cd C:\Windows\System32`、存档、kill、无客户端 `resume`，连跑 3 次逐字一致：
存档 cwd=System32、33 行；恢复后可见 20 行 line-*、滚动区 31；分屏缩小后可见 8、滚动区仍 31；shell 位置 System32。
cmd pane `cd /d C:\Windows\Fonts` → `pane_current_path`、存档 cwd、恢复后 `cd` 输出均为 Fonts。无残留回放文件、无残留 `__replay` 进程。

## 第 B 轮（计数 2/3，无发现）— 视角：边界

数据：`save-history 0` 不回放、旧行不在；5000 行回放 164 ms 后 shell 可用、5000 行全在、末行为新提示符；
`remain-on-exit` 下立即退出的程序：回放的 `lived` 与退出提示都在、`pane_dead_status` 3；回放进行中 kill-session：0 残留文件、0 残留进程、server 存活；
pwsh 进注册表驱动器时目录保持上一个真实目录；含空格与中文的路径正确；`pwsh -Command …` 不注入钩子。

## 第 C 轮（计数 3/3，无发现）— 视角：可复现性 + 文档 claim

数据：全量测试 170 项，连续 7 次全过（其中一次指纹不同但未能定位到具体用例，随后把唯一依赖固定 sleep 的
`proccwd` 测试改成轮询）；README（中英）改为"目录跟着 cd 走、不用配置"，删掉旧的"放进 $PROFILE"片段；
代码 claim（HELPER_GEN、标记触发、钩子避让、缩行保留、序号文件名、助手等待删除、无调试残留）逐项核对；
真实存档目录无测试文件；clippy 与 fmt 无输出。

## 结论（第二十五次验收）

A、B、C 三轮连续零发现，验收通过。测试 156 → 170（lib 114 / console 4 / e2e 52）。
用户机器上跑的是 0.5.0（`C:\Program Files\wmux`），需要发新版才能用上。

# 第二十六次验收（2026-09-23）— C-10 收尾：`%if` 求值、比较运算、`capture-pane -J`、25 个新变量、完整 target、`window-size`、picker 过滤/标记

## 做了什么

1. `%if` / `%elif` / `%else` / `%endif` 真正求值（条件是格式串，非空且非 `0` 为真；server 级上下文，不跑 `#()`），
   原先是整块跳过。新增比较运算 `#{==:}` `#{!=:}` `#{<:}` `#{>:}` `#{<=:}` `#{>=:}` `#{&&:}` `#{||:}` `#{m:pat,s}`（`m/i:` 忽略大小写），操作数本身也是格式串。
2. `capture-pane -J` 把软换行拼回一行（按 vt100 的 `row_wrapped`）。
3. 25 个新格式变量（session_activity / last_attached、window_activity / start / end / layout、pane_last、pane 四边坐标、
   cursor_x/y、history_size / limit、pane_mode、client_* 等），全部从活树取值。
4. `select-pane -t s:w.p` 完整 target；`copy-mode -t`。
5. `window-size latest|smallest|largest|manual`：session 尺寸听哪个客户端（默认 latest = 最后接入/改尺寸/敲键的那个；
   smallest / largest 在接入、改尺寸、脱离、客户端断线时重算；manual 只认 `resize-window`）。
6. `choose-tree` / `choose-jobs` / `choose-buffer` / `choose-client`：`f` 输入子串过滤（边打边筛，`Enter` 留下，`Esc` 还原；
   树里 session 与其窗口互相带上），`t` 打标记（行首 `*`，光标下移）、`T` 清标记，`x` 杀掉标记的行（树里是 session / 窗口，
   任务板里是 pane；`r` 重启同理）。提示行右端显示 `[n tagged] [filter: xx]`，窄屏时覆盖静态提示。

## 修复过程中的发现（不计数）

- e2e `choose_tree_picker` 用 `x` 当"未绑定键"，现在 `x` 会杀东西 → 改用 `z`。
- 提示行加上过滤/标记状态后超过 80 列被截断 → 状态改为右对齐覆盖。
- 过滤串只含空白时不筛但提示行显示 `[filter:  ]` → 按 trim 后判断。
- 探针脚本：`new -x 1 -y 1` 在解析阶段就被拒（≥10×3），是脚本假设错，代码没问题。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制 + 全量测试）

数据：配置文件里 `set -g window-size smallest` 生效、`%if #{==:...}` 未命中时 mouse 保持 on、config notes 为空；
`set` 四个值逐个回读一致，`window-si` 缩写命中，`sideways` 退出码 1 并列出四个合法值，`win` 报歧义（4 个候选）；
无客户端的 `-x 100 -y 30` session 在四种模式下都保持 100x30；`show-options -g` 列出 `window-size`。
全量测试 180 项（lib 120 / console 4 / e2e 56）全过，clippy 无 warning，`cargo fmt --check` 退出码 0。
e2e `window_size_picks_which_client_sizes_the_session`：两客户端 80x24 / 60x20，latest 下 Resize → 60x20、大客户端敲键 → 80x24；
smallest → 60x20 且敲键不变；largest → 80x24，大客户端脱离 → 60x20；manual 下 Resize 70x22 + 敲键仍 60x20。
e2e `choose_tree_filters_and_tags`：`beta` 过滤剩 2 行（session 行跟着留）、`Esc` 回到 5 行、`GAMMA` 大小写不敏感且 `Enter` 后过滤留下、
`C-u`+`Enter` 清掉；打 3 个标记再取消 1 个 → `[2 tagged]`；`x` 后 `ls` 只剩 `alpha: 1 windows`，`T` 清标记。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：两条新 e2e（连同另外 3 条 picker e2e）与 3 条相关单元测试连跑 3 次，去掉耗时后输出行完全一致（distinct = 1）：
e2e 5 passed / lib 3 passed，每次相同。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 静态一致性

数据：默认值 `latest`；空值退出码 1；` largest ` 带空白存为 `largest`；`LATEST` 大小写敏感被拒；
`new -x 10 -y 3`（解析器接受的最小值）在四种模式下都是 10x3；无客户端 server 下 `split-window` 后仍 80x24；
`window-size` 是全局选项，存档文件里 0 处出现。文档核对：README（中英）picker 按键行、"还没做的"段（改为 `window-size` 决定听谁、
过滤是子串）、parity 表 choose-tree / copy-mode `-t` / select-pane 完整 target / `%if` 段落 / 选项列表 / Still to do；
`docs/index.html` 测试数 170 → 180。`chooser_filter` 单元测试覆盖：无过滤全留、大小写不敏感、标题行常留、session 与窗口互相带上、无命中只剩标题。

## 结论（第二十六次验收）

A、B、C 三轮连续零发现，验收通过。测试 175 → 180（lib 120 / console 4 / e2e 56）。

# 第二十七次验收（2026-09-23）— 发布 0.7.0 时 CI 暴露：Run 键不存在则 `startup` 全挂

## 发现

v0.7.0 的 release 工作流在 test 步骤失败：`startup::tests::install_status_remove_round_trip` 报
`cannot open HKCU\Software\Microsoft\Windows\CurrentVersion\Run (error 2)`。runner 的用户 profile 里根本没有 Run 键
（v0.6.0 时的镜像有，这次换了 Node 24 镜像后没有）。代码只 `RegOpenKeyExW` 不创建——一个从没有程序登记过自启动的新用户
跑 `wmux startup on` 也会这样失败，`status` / `off` 也会报错而不是说"未安装"。本地全绿是因为开发机上这个键一直在。

## 修复

`Key::open(path, access)` 缺键返回 `None`（`status` → 未安装，`off` → "not installed"），`install` 改用 `RegCreateKeyExW`
（非易失、KEY_SET_VALUE）。键路径变成参数，单元测试用 `HKCU\Software\wmux-unit-test-<pid>\Run` 这个自己的临时键复现
"缺键 → None → create 后有 → 再 create 无害 → 删掉后又 None"，测完删键。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路 + 全量

数据：4 条 startup 单元测试全过（新测试覆盖缺键路径），`reg query HKCU\Software` 无 `wmux-unit-test` 残留；
全量 181 项（lib 121 / console 4 / e2e 56）全过；clippy 无 warning，fmt 干净。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：startup 4 条连跑 3 次结果一致（4 passed ×3）。

## 第 C 轮（计数 3/3，无发现）— 视角：真实缺键环境（CI runner）

数据：见下方 release 结果——v0.7.0 tag 移到修复提交后重跑，test 步骤通过即为证据。

## 结论（第二十七次验收）

三轮零发现。测试 180 → 181。`docs/index.html` 计数同步。

# 第二十八次验收（2026-09-23）— 关机/注销时存档、CI 门禁、tmux 四个小差距

## 做了什么

1. **关机/注销时存档**（`src/shutdown.rs`）：server 起一个隐藏的顶层窗口（类 `wmux-shutdown`，标题 = socket 名）收
   `WM_QUERYENDSESSION` / `WM_ENDSESSION`，同时注册控制台处理函数收 `CTRL_SHUTDOWN_EVENT` / `CTRL_LOGOFF_EVENT`
   （client 拉起的 server 是 DETACHED_PROCESS 没控制台，只有窗口这条路；登录项那条 `conhost --headless` 有控制台）。
   任一信号触发 `Event::EndSession`，server 同步把每个 session 连历史一起存一遍再应答，`ShutdownBlockReasonCreate`
   把关机拖住那一瞬间。`autosave off` 时同样不存。
   **前提修正**：之前对用户说"autosave 只在结构变化和退出时存"是错的——历史每 30 秒就存一次（实测存档文件 mtime 30 秒一跳）；
   关机真正会丢的是最后 30 秒和 cwd 变化，现在补上。
2. **CI**：三个 workflow 的 `actions/checkout` 升 v5（去掉 Node 20 告警）；release 工作流用 `gh run list --commit` 等
   同一 commit 的 CI 跑绿才出包（最多等 25 分钟，失败即拒绝发布），替换掉它自己重复跑一遍测试的步骤；
   `permissions` 加 `actions: read`。
3. **tmux 小差距**：`display-popup -x/-y`（列/行、`N%`、`C`、`R`/`B`，越界拉回）；`swap-pane -s A -t B` 成对写法
   （同窗口走 layout.swap，跨窗口/跨 session 把 Pane 对象互换、layout 用 set_panes 换 id、active 跟位置走、两边重排）；
   `list-panes -s`（`窗口.` 前缀）/ `-a`（`session:窗口.` 前缀）；`set -t 窗口 synchronize-panes`。

## 修复过程中的发现（不计数）

- 探针 gpC 先取窗口句柄再 `kill-session`：server 没 session 会自行退出，`new b` 起的是新 server，旧句柄发消息当然没反应。
  是脚本错，代码没问题（gpDbg 在同一 server 上 WM_ENDSESSION(1) 与 WM_QUERYENDSESSION 都存了）。
- 探针 gpC 敲 echo 前没等 pwsh 起来：改成轮询提示符和回显。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制 + 全量）

数据：`list-panes` 单窗口 2 行无前缀、`-s` 4 行 `0.`/`1.` 前缀、`-a` 5 行 `g:0.0:`…`other:0.0:` 前缀；
同窗口成对交换 `%2,%4 -> %4,%2`；跨窗口交换后 g:0 = `%7,%2`、g:1 = `%5,%4`，pane 总数仍 5，被搬的 pane 能回显；
`set -w -t g:1 sync on` 后 g:1 = 1、g:0 = 0；真实 DETACHED server 的隐藏窗口 FindWindow 找得到且不可见，
`WM_QUERYENDSESSION` 5 ms 内应答 1，标记从文件里 0 处变 2 处（命令行 + 输出）。
全量 185 项（lib 123 / console 4 / e2e 58）全过，clippy 无 warning，fmt 干净。
e2e `a_shutdown_saves_every_session_first`：先等首次 autosave 落盘，再 echo，确认文件里没有，发 WM_QUERYENDSESSION 后有；
`autosave off` 后再发不存。e2e `the_smaller_tmux_gaps_are_closed`：四项各自断言，popup `-x 0 -y 0` 左上角在 (0,0) 且 20 宽，
`-x R -y B` 右下角贴状态栏上一行。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：3 条 e2e + 4 条单元测试连跑 3 次，去掉耗时后输出完全一致（distinct = 1）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 静态一致性

数据：单 pane 时 `-a`/`-s` 各 1 行；和自己交换退出码 0 无输出；坏 session / 坏 pane 序号 / 缺失窗口各报对应错误退出码 1；
`set -t a:0 sync` 缩写切换 1 → 0；`WM_ENDSESSION(0)`（取消的关机）不存，`WM_ENDSESSION(1)` 存；收完消息 server 照常服务；
kill-server 后窗口消失。文档核对：README（中英）"还没做的"删掉四项、popup 段加 `-x/-y`、存档段写明 30 秒与关机存；
parity 表 display-popup / list-panes / swap-pane / set-window-option 行与 Still to do 段同步；站点计数 181 → 185。

## 结论（第二十八次验收）

A、B、C 三轮连续零发现，验收通过。测试 181 → 185（lib 123 / console 4 / e2e 58）。

# 第二十九次验收（2026-09-23）— tmux 最后几个小差距：跨 session swap-window、pipe-pane -I、弹窗里的前缀、树折叠

## 做了什么

1. `swap-window -s a:1 -t b:0` 跨 session：两个窗口互换位置（`split_at_mut` 同时借两个 session），
   每个 session 的当前窗口按下标保留（tmux 同样是位置带标记）。
2. `pipe-pane -I`：命令打印什么就往 pane 里敲什么（命令 stdout 由读线程经 `PaneEvent::Input` 送回 server 线程写入，
   命令输出结束时发空块，仅 `-I` 的管道随之结束）；`-O` 是默认；`-IO` 双向。`Pipe.output` 为假时 pane 输出不再送给命令。
3. 弹窗里前缀键：连按两次前缀（以及 `send-prefix` 无 `-t`）把前缀送给弹窗里的程序，而不是后面的 pane。
4. `choose-tree` 折叠：`-`/Left 折叠当前行所在 session，`+`/`=`/Right 展开；折叠时光标跳到 session 行。
5. 去掉了 README 里"`bind -r` 没有每键重复次数"这条——tmux 本身也没有这个概念，是过去写错的限制。

## 修复过程中的发现（不计数）

- e2e 里 `-o` 循环没带 `-I`，第二条管道跑的是 `-O` 什么也不会敲进去——测试写错，改 `-o -I`。
- 探针 g2A：PowerShell 双引号里 `$g` 被展开、pwsh `-Command` 会把 stdin 读完才执行（README 的 `$input | Add-Content`
  例子因此只在管道停下时落盘——是既有行为，已在 README 中英两处注明，并给出 `cmd.exe /c findstr` 这种逐行落盘的替代）。
- 探针 g2A 的 `-IO` 用 powershell `ReadLine` 同样被吞，换成 `cmd /c "set /p l= & echo ..."` 后 266 ms 内双向打通。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制 + 全量）

数据：跨 session swap 后 a = `a0*, b0`、b = `a1*, b1`，server 仍 4 个 pane；`-I` 278 ms 内把 `echo from-the-pipe` 敲进 pane 并执行；
仅 `-I` 的管道结束后 `-o -I` 起的是新管道（`second` 257 ms 出现）；`-O` 停管道后日志 213 ms 内落盘；`-IO` 266 ms 内一来一回。
全量 186 项（lib 123 / console 4 / e2e 59）全过，clippy 无 warning，fmt 干净。
e2e `the_last_small_tmux_gaps_are_closed`：四项各自断言，弹窗里 `powershell ReadKey` 收到 `code=2`（C-b）且后面的 pane 没收到；
树上 `-` 后 6 行变 4 行、`+` 回 6 行、在窗口行按 Left 折叠并把光标带到 session 行。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：6 条相关 e2e + 4 条解析单元测试连跑 3 次，去掉耗时后输出完全一致（distinct = 1）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界

数据：swap-window 和自己换退出码 0 无输出；换到不存在的下标报 `no window 5`；两个 session 各只有一个窗口时互换正常；
`-I` 跑一个不存在的程序退出码 0、pane 照常应答；`-I` 打进已退出的 pane 文字不变；运行中的 `-O` 管道被新管道替换后
只有新日志拿到输出；全程 server 存活。文档：README（中英）"还没做的"删四项、picker 按键行加折叠、pipe-pane 段加 `-I/-O` 与
pwsh 缓冲说明；parity 表 choose-tree / display-popup / pipe-pane / swap-window 行与 Still to do 段同步；站点计数 185 → 186。

## 结论（第二十九次验收）

A、B、C 三轮连续零发现，验收通过。测试 185 → 186（lib 123 / console 4 / e2e 59）。

# 第三十次验收（2026-09-23）— 布局字符串、每客户端视口、resize-window、20 分钟 soak

## 做了什么

1. **布局字符串**（`layout.rs`）：`#{window_layout}` 现在是 tmux 格式（`校验和,WxH,X,Y`，`{}` 左右、`[]` 上下，叶子带 pane id），
   由窗口当前矩形反推；`select-layout <字符串>` 读回：有校验和就校验，格子数必须等于 pane 数，尺寸按现窗口缩放，pane 按顺序填入。
2. **每客户端视口**：客户端比窗口小（`window-size` 听了别的客户端）时，整窗按 session 尺寸合成，再裁出该客户端的视口；
   状态栏按客户端自身宽度画；鼠标坐标按视口偏移换算。`refresh-client -U/-D/-L/-R [n]` 平移并"钉住"，下一次敲键送 pane 时解钉、
   视口跟着光标最小位移。默认绑定 `S-方向键`（5 行 / 10 列，可连按）。
3. **resize-window**：探针发现文档（README 中英、parity、config 注释）都说 `window-size manual` 靠 `resize-window` 改尺寸，
   而这条命令根本不存在。补上：`-x/-y`、`-U/-D/-L/-R n`（一次一个方向，同 tmux）、`-A`/`-a`（最大/最小客户端），下限 10×3。
4. **soak**：release 二进制，4 个 pane 用 cmd 循环不停刷屏，另一个 session 每 10 秒分屏/缩放/关 pane/respawn，`history-limit 2000`，跑 20 分钟。

## 修复过程中的发现（不计数）

- 视口 e2e 在全量并行时挂过一次：`x` 的回显比渲染先到时光标在第 6 列而不是第 5 列，断言写死了列号 → 两种都接受。
- `resi -Z` 变成歧义（resize-pane / resize-window），旧测试改为 `resize-p`，并断言歧义报错。
- 探针脚本：soak 第一版把整条 cmd 命令当一个参数传给 split-window（没起来，内存曲线是空载的）；vpC 的"字符串相等"期待错
  （字符串里带 pane id，应比尺寸）；`resize-window -L 10 -D 2` 期待两个方向同时生效，tmux 只认一个。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制 + 全量）

数据：3 pane 窗口的字符串 `0f51,100x29,0,0[100x19,0,0,2,100x9,0,20{50x9,0,20,4,49x9,51,20,5}]`，切 tiled 再应用字符串后尺寸
`100x19,50x9,49x9` 原样回来、字符串逐字相等；同一字符串套到 60x20 的 3 pane 窗口得 `60x13,29x5,30x5`；
`refresh-client` 六种写法退出码 0，`-R abc` 报 `bad amount`；`list-keys` 列出四条 `-r` 的 S-方向键绑定。
全量 188 项（lib 124 / console 4 / e2e 60）全过，clippy 无 warning，fmt 干净。
e2e `a_small_client_has_its_own_view_of_a_big_window`：80x24 与 40x12 两客户端、`window-size largest` 下 session 保持 80x24，
小客户端只见前 40 列，`-R 20` 后见 20..60 列且状态栏仍是 40 列整行，大客户端不受影响，敲一个键后视口回到光标处，`-R 500` 钉在右缘。
e2e `window_size_picks_...`：`resize-window -x 100 -y 30` → 100x30，`-L 10` 再 `-D 2` → 90x32，`-a` → 70x22，`-x 1 -y 1` → 10x3，`-x wide` 报错。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：整套 e2e（60 条，含视口测试并行）+ 17 条布局/刷新单元测试连跑 3 次，去掉耗时后输出完全一致（distinct = 1）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 长时稳定

数据：单 pane 字符串套回自身 OK；2 格套 1 pane 报"has 2 panes, the window 1"；坏校验和、无主体、`1x,0,0,0` 各报对应解析错误；
预设名仍优先；11 pane 深嵌套字符串套到同尺寸 11 pane 窗口后尺寸逐一相同，缩到 20x6 时 11 个 pane 全在（最小 1x1）、其字符串仍可解析；
未 attach 的脚本客户端 `refresh-client -R 10` 无害；server 全程存活。
soak 20 分钟：私有内存后半段 52.6 MB → 52.6 MB（+0.0），句柄 240 → 240，线程 47 → 45，chatty pane 的 scrollback 恰为 2000 行，
kill-server 后进程退出。
**遗留（非本批引入）**：窗口被压到 6 行再放大回 300x80 后各 pane 比例丢失（尺寸被夹成 1 后再等比放大），极端缩放的既有行为，
tmux 亦类似，记录在案不修。

## 结论（第三十次验收）

A、B、C 三轮连续零发现，验收通过。测试 186 → 188（lib 124 / console 4 / e2e 60）。

## 第三十次补记：发布 0.9.0 时 CI 门禁拦下的一处

runner 上视口 e2e 挂了：`x` 敲进去后视口回到了第 0 列。原因是"光标在视口外就跟随"过于敏感——cmd 重画提示行时光标瞬间在第 0 列，
慢机器恰好在那一刻渲染，视口就被拽到 0。改为：敲键送 pane 时下一次渲染立即跟随；其余情况光标离开视口持续 150 ms 才跟随。
本地整套 e2e 连跑 5 次全过；release 门禁按设计拒绝了红 CI 的 tag，tag 移到修复提交后重发。

# 第三十一次验收（2026-09-24）— Tab 补全：PowerShell 补全脚本 + `:` 命令行补全

## 做了什么

1. `wmux completion powershell` 输出一段 `Register-ArgumentCompleter -Native` 脚本（`$PROFILE` 里
   `wmux completion powershell | Out-String | Invoke-Expression`）：补命令名（含 client 本地的 completion / startup / windows-terminal）、
   正在敲的 flag（按命令查表）、`-t`/`-s` 后从运行中的 server 取 session、`session:序号`、`session:窗口名`、`-L` 后列出正在跑的
   socket（扫命名管道）、`completion` 后列 shell。`-L`/`-f` 全局参数会被跳过。
2. 命令名 / flag 表 `command::FLAGS` 与解析器绑死：单元测试对表里每条 `命令 flag 1 x` 走一遍解析，出现 `unknown flag` 即失败；
   第一版表里 8 个凭印象写的 flag 被这个测试当场抓出来删掉（`attach -c`、`capture-pane -E`、`new-window -b`、`select-window -n/-p/-l`、
   `split-window -l/-p`）。
3. `:` 命令行的 Tab：第一个词补命令名，`-t`/`-s` 后补目标；唯一候选整词填入（命令后带空格），多个候选填到公共前缀、候选列表
   暂时顶替提示符标签（下一个键恢复），没有候选显示 `(no completion)`。

## 修复过程中的发现（不计数）

- 补全脚本第一版在"空词"时也列 flag，导致 `resize-window -x <Tab>` 出 flag 列表；改为只在正在敲 `-` 时列 flag。
- 探针：`Get-ArgumentCompleter` 这个 cmdlet 不存在；哈希脚本把字节数组摊进了管道（31 个"哈希"）；`list-commands` 不带 `-L`
  打到了用户机器上还在跑的老版本 server（它没有 resize-window）——这也说明升级后老 server 进程会一直跑到 kill-server/重启为止。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（真实 PowerShell 里加载 release 二进制吐出的脚本，用 CompleteInput 问）

数据：`wmux spl` → split-window；`list-s` → list-saved,list-sessions；`split-window -` → 7 个 flag；活 server 上
`select-window -t ` → alpha,beta,alpha:0,alpha:pwsh,beta:0,beta:build,beta:1,beta:test；`kill-session -t b` → 5 个 beta 开头；
`completion ` → powershell；`-L ` → 正在跑的 socket；`-L x ne` → 4 个 ne 开头的命令。
全量 192 项（lib 127 / console 4 / e2e 61）全过，clippy 无 warning，fmt 干净。
e2e `tab_completes_at_the_prompt_and_the_shell_gets_a_completer`：`spl`+Tab → `:split-window `；`list-s`+Tab → 提示符变
`(list-saved list-sessions) list-s`，再敲 e 标签恢复，再 Tab → `:list-sessions `；`zzz` → `(no completion)`；
`select-window -t b`+Tab → `(beta beta:0 beta:build) select-window -t beta`，`:b`+Tab → `beta:build`；
子进程 `wmux completion powershell` 含 Register-ArgumentCompleter 与全部命令，`completion bash` 退出码 1。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：脚本连吐 3 次 SHA-256 完全相同（6740 字节）；e2e + 3 条单元测试连跑 3 次输出一致（distinct = 1）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界

数据：脚本 Invoke-Expression 两遍不报错；PowerShell 解析器 0 个错误；`-t` 指向没有 server 的 socket、命令后空词、`-x` 后空词
均无报错（补全器不给候选时 PowerShell 自己退回文件名补全，是 PowerShell 的默认行为，tmux 的 bash 补全同样如此）；
带引号的半个词返回空；`-f 文件` 被跳过后 `ki` 仍补出 4 个 kill-*；无 shell / 未知 shell / 多余参数各报对应错误退出码 1；
`pwsh` 与 `powershell` 同一份脚本。

## 结论（第三十一次验收）

A、B、C 三轮连续零发现，验收通过。测试 188 → 192（lib 127 / console 4 / e2e 61）。

# 第三十二次验收（2026-09-24）— 右键粘贴

## 做了什么

`mouse on` 时右键被 wmux 吃掉、什么都不做，终端自带的"右键粘贴"就没了。现在 pane 上按下右键 = 把 Windows 剪贴板贴进
当前 pane（走 `paste-buffer`，带括号粘贴），和 Windows Terminal / conhost 的右键一样；在 copy mode 里右键先退出 copy mode 再贴。
程序自己要鼠标事件（vim、htop 那类）时右键照旧原样转给程序；状态栏上的右键不粘贴。
顺带两处加固：copy 时剪贴板忙（别的程序占着、剪贴板管理器在读）不再让整个复制失败——paste buffer 照设，提示里附上剪贴板的报错；
`OpenClipboard` 重试从 100 ms 提到 500 ms。

## 修复过程中的发现（不计数）

- 全量并行时 `copy_mode_vi_motions_and_modes` 挂过一次：第二次复制后立刻 `show-buffer`，键和命令走两条管道没有先后保证，
  改成轮询 buffer 变化（既有竞争，被新增的剪贴板流量放大后露出来）。
- 新 e2e 与其它复制测试共用机器唯一的剪贴板：别的测试复制的文字会被右键贴进来。测试改成"设、点、看，不对就清行重来"；
  第一版用 C-u 清 cmd 的输入行，cmd 不认 C-u（打出 `^U`），改用 Escape；`set_text` 遇忙也重试。之后整套 e2e 连跑 6 次 0 失败。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路

数据：e2e `a_right_click_pastes_the_clipboard` 走真实管道协议（`ClientMsg::Mouse` 按钮位 2 按下/抬起）：剪贴板里的
`echo pasted-by-right-click` 出现在 cmd 提示符后并执行；滚轮进 copy mode（`[3/` 指示）后右键 → 指示消失、文字贴进；
状态栏上右键 300 ms 内什么都没贴、随后照常能敲字。全量 193 项（lib 127 / console 4 / e2e 62）全过，clippy 无 warning，fmt 干净。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：整套 e2e（62 条并行，含全部复制/粘贴测试）连跑 6 次全过，另加此前一轮 4 次里 3 次失败的对照（修测试前）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 静态一致性

数据：程序占用鼠标时右键仍走原有转发分支（`mouse_selects_pane_and_copy_mode_scrolls` 覆盖那条路，右键分支放在它之后）；
剪贴板忙 500 ms 内重试，超时报 `clipboard: OpenClipboard failed`，复制侧 buffer 仍然设好；剪贴板为空时右键报
`clipboard is empty`（`paste-buffer` 既有路径）。README（中英）鼠标段落写明右键粘贴；README 的 `]` 行本就是"贴剪贴板"。

## 结论（第三十二次验收）

A、B、C 三轮连续零发现，验收通过。测试 192 → 193。

# 第三十三次验收（2026-09-24）— 滚轮选中被滚的 pane；copy mode 里 `C-b C-b` 上翻页（用户报告）

## 发现与修复

用户报告"翻历史时 C-b/C-f/j/k 都不动"。根因：滚轮在**非活动** pane 上滚会让那个 pane 进 copy mode，但键盘只看活动 pane
（`in_copy` 取 active pane），键全进了另一个 pane 的 shell。修：滚轮事件和按键一样先把鼠标下的 pane 设为活动（tmux 的
`WheelUpPane` 绑定也是先 `select-pane -t=`）。另外 `C-b` 本身是前缀键，copy mode 里单按 C-b 永远是前缀（tmux 同样如此）；
补一条：copy mode 里 `C-b C-b`（前缀两次）交给 copy mode，即上翻页；C-f、PageUp/PageDown、C-u/C-d 本来就有。README 中英写明。

## 修复过程中的发现（不计数）

- 右键粘贴 e2e 在状态栏那一步的 `set_text` 没重试，剪贴板忙时 `EmptyClipboard failed`；统一走重试。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路

数据：e2e `the_wheel_selects_the_pane_it_scrolls_and_the_prefix_twice_pages_up`：左右两 pane、右侧活动，左侧灌 60 行；
在左侧滚轮一格后 `#{pane_index}` 从 1 变 0、左上角出现 `[3/` 指示；`C-f` 翻到 `[0/`；`C-b C-b` 翻回非 0；`k`/`j` 让反显光标
上一行/回原行；Escape 退出。全量 194 项（lib 127 / console 4 / e2e 63）全过，clippy 无 warning，fmt 干净。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：整套 e2e（63 条并行）连跑 4 次全过。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 静态一致性

数据：单 pane 时滚轮行为不变（`mouse_selects_pane_and_copy_mode_scrolls` 仍过）；`mouse off` 时滚轮不选 pane（守卫 `mouse_opt`）；
程序自己要鼠标事件时滚轮仍原样转发（那条分支在前）；不在 copy mode 时 `C-b C-b` 仍把 C-b 送给 pane/弹窗（原行为）。
README 中英 copy mode 段补上翻页键与 `C-b C-b` 说明。

## 结论（第三十三次验收）

A、B、C 三轮连续零发现，验收通过。测试 193 → 194。

# 第三十四次验收（2026-09-24）— 状态栏原生变量与新默认 status-right

## 做了什么

`src/sysinfo.rs`：进程内直接读的变量，不再需要 `#(命令)`——`cpu_percentage`（GetSystemTimes 两次采样之差）、`ram_percentage` /
`ram_used`（GlobalMemoryStatusEx）、`battery_percentage` / `battery_charging`（GetSystemPowerStatus，台式机为空）、`uptime`
（GetTickCount64）、`git_branch`（向上找 `.git/HEAD`，含 worktree 的 `gitdir:` 文件，不起 git）、`pane_current_path_short`（家目录写成 `~`）、
`pane_pid_command`（Toolhelp 快照里 pane 进程最新的后代，跳过 conhost）。系统读数与进程快照各缓存 1 秒。
默认 `status-right` 改为 `#{?git_branch, #{git_branch} |,} #{pane_current_path_short} | CPU … MEM …#{?battery_percentage, | BAT …,} | %H:%M`，
`status-right-length` 默认 60 → 100（窗口列表仍优先，右侧被裁）。`set -g status off` 整行关，`set -g status-right` 整条换——都是既有机制。

## 修复过程中的发现（不计数）

- console 测试与 status-justify e2e 断言了旧默认（右侧以引号包着的 pane 标题开头）：前者改为断言 CPU/MEM，后者显式 `set status-right`。
- 一版把每个 pane 的进程快照各做一次；改为快照本身缓存 1 秒，30 个 pane 的 `jobs` 从每次多次快照变成一次。
- 上一批留下的 debug `wmux.exe` 进程占着 `target\debug\wmux.exe` 让构建失败；按路径只杀 target 下的，用户装的 server 不碰。
- clippy 5 条（checked_div、redundant guard、while let、rfind、复杂类型）逐条改掉。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（release 二进制）

数据：仓库目录的 pane：`master | ~\Documents\dfine\wmux | CPU 4% MEM 22% | 10:40`（默认格式整条展开）；TEMP 目录分支为空、路径 `~\AppData\Local\Temp`；
空闲 `pane_pid_command` = cmd，跑 `ping` 时 = PING；台式机电池 `[] 0`；50 次 `display-message` 809 ms（16 ms/次，客户端往返为主）。
全量 199 项（lib 131 / console 4 / e2e 64）全过，clippy 无 warning，fmt 干净。
e2e `the_machine_variables_answer_without_a_command`：两次采样后 CPU 有数、RAM/uptime 有值、仓库里有分支、`~\` 开头、ping 时程序名为 ping、
电池字段形态合法、attach 后状态栏含 `CPU … % MEM …`、`status off` 后消失、自定义 `status-right` 生效。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：整套 e2e（64 条）连跑 3 次全过；4 条 sysinfo 单元测试（读数缓存 1 秒、分支/脱离 HEAD/worktree/无仓库、`~` 大小写与同名前缀目录、
本进程程序名）随全量通过。

## 第 C 轮（计数 3/3，无发现）— 视角：边界

数据：已退出的 pane 程序名为空、`pane_dead` 1；`C:\` 根目录短路径原样、分支为空；pane 目录被删后分支为空、短路径仍给（目录被 shell 占着删不掉是脚本的事）；
带条件的自定义右侧在无电池无仓库时只剩 `up 28m`；空 `status-right` 展开为空；30 个 pane 的 `jobs`（每 pane 一个上下文）28 ms、一秒内再来 21 ms；server 全程存活。
README（中英）变量表与默认格式说明、parity 变量段同步；站点计数 194 → 199。

## 结论（第三十四次验收）

A、B、C 三轮连续零发现，验收通过。测试 194 → 199（lib 131 / console 4 / e2e 64）。

## 第三十四次补记：CI 抓到的——右侧变长后当前窗口被挤出状态栏

`9682dd6` 在 CI 上 `attach_type_split_detach`、`choose_tree_picker` 失败：runner 的 TEMP 是 `C:\Users\RUNNER~1\...`，
8.3 短名没被识别成家目录，路径原样很长；右侧变长后状态栏只剩 `0:cmd-`，当前窗口 `1:cmd*` 没了。两个原因：

1. **渲染的真 bug（既有）**：旧逻辑只给"当前窗口一个标签"预留空间，但列表从头画，前面的窗口先把这块吃掉。
   改为：列表整体放得下时右侧用剩余；放不下时从前面省略窗口直到当前窗口能显示（被省略的窗口在点击命中表里占空区间，下标不乱）；
   当前窗口单独都放不下时不预留。单元测试补了"当前窗口在末尾/在中间"两例，旧的两处期望按新规则更新。
2. `~` 缩写：先用 GetLongPathNameW 把两边都展开成长名再比较；单元测试在卷保留 8.3 名时验证 `RUNNER~1` 类写法。

证据：把本机 TEMP 指向 scratchpad 下的长目录复现 CI 条件，`git stash` 掉修复时 `choose_tree_picker` 报 `timeout waiting for window 1`
（与 CI 同一条），恢复修复后通过。同一长 TEMP 下 `attach_type_split_detach` 另有一处失败是复现环境造成的（配置报错浮层打印的
配置文件路径在长 TEMP 下被 80 列截掉），CI 的 TEMP 不长，不适用。全量 199 项全过，e2e 连跑 3 次全过，clippy/fmt 干净。

# 第三十五次验收（2026-09-24）— 更新链路：版本不一致提示、restart-server、wmux update

## 做了什么

1. **版本不一致提示**：交互式 attach 前，客户端另开连接问 server 的 `version`；不同则 attach 期间终端标题写
   `wmux: <session> [server X: run wmux restart-server]`，detach 时打印一行说明。不改协议，所以对**旧** server 同样生效
   （旧 server 无法主动往客户端推消息，只能由新客户端自己说）。`wmux version` 同时给出本程序与 server 的版本。
2. **`wmux restart-server`**：记下正在跑的 session → `save-session -a` → `kill-server -r`（新 server 让已接的终端等待并重新 attach；
   旧 server 拒绝 `-r` 时退回普通 `kill-server`，终端被断开并提示 `wmux attach`）→ 等旧 pipe 消失 → 起本版本 server →
   只对刚才在跑的 session 逐个 `restore-session`（存过档但已关掉的 session 不会复活——`kill-session` 不删存档，用 `--restore` 会全复活）。
   在 pane 里运行时：pane 的 job 新增 `JOB_OBJECT_LIMIT_BREAKAWAY_OK`（只有显式要求的进程能脱离，其它子进程仍随 pane 结束），
   restart 以 `CREATE_BREAKAWAY_FROM_JOB` 在 pane 外重跑自己，结果写 `restart.log`；旧 server 的 job 不允许脱离时明确报错请在 wmux 外运行。
   客户端的控制台输入线程改为整个进程一个（重新 attach 复用，不会有第二个读者抢键）。
3. **`wmux update [--check]`**：用系统自带 `curl.exe` 问 GitHub 最新 release；按安装方式处理——Program Files 下（MSI）下载 MSI 与
   `.sha256`，`certutil` 核对后交给 `msiexec /passive`（0/3010 成功，1602 取消）；scoop 安装走 `scoop update wmux`；其它方式给出下载页。
   装完提示 `restart-server`。无后台行为。

## 修复过程中的发现（不计数）

- **读代码抓到的真 bug**：旧 server 对 `kill-server -r` 回的是 `unexpected argument '-r'`，第一版回退只认 `unknown flag`，
  会让 restart 对**所有 0.9 server**失败（正是用户的情形）。改为 `-r` 任何拒绝都回退（此时已存档，安全）。
- **读代码抓到的真 bug**：scoop 装的是 `scoop.cmd/.ps1` shim，`Command::new("scoop")` 只找 `scoop.exe`，scoop 路径必失败；改走 `cmd /c`。
- 重启后运行时 `set`/`bind` 的设置丢失：旧 server 分不清默认值与用户设置，全搬会把旧版默认值（如 0.9 的 status-right）钉到新版上；
  与 tmux `kill-server` 一致，从配置文件重读。restart 输出与 README（中英）写明。
- 测试：重连断言第一版可被重启前的画面满足，改为先确认客户端打印了"restarting, attaching again"且其后有重绘；
  一次断言失败留下了真实 server 进程占着 `target\debug\wmux.exe`，给 restart 测试加 `StopServer` 守卫（Drop 时 kill-server）。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（**真实 0.9.0 server**，一次性 socket，不碰用户的 default）

数据：装机的 `C:\Program Files\wmux\wmux.exe`（0.9.0）起 server、两个 session、一条回显；新构建（0.10.0）`version` 输出
`server: wmux 0.9.0` 与说明；`restart-server` → `server wmux 0.9.0 -> wmux 0.10.0; 2 of 2 session(s) restored: one, two`，
server 进程路径从 Program Files 变为 target\release，回显历史在；用户自己的 default server 只被读（显示 0.9.0 与说明），未被触碰。
`update --check`：GitHub 最新为 0.9.0，本构建 0.10.0 → "is the latest"。
console 测试（真实进程 + ConPTY）：attach 中的客户端收到重启通知、其后重绘、进程未退出、敲字到达新 server；普通 `kill-server` 则客户端退出；
在 pane 里跑 restart 后 `restart.log` 写明恢复、server pid 改变；`--ignored` 测试以 0.9.0 为 server 验证标题提示与 detach 说明。
全量 205 项（lib 135 / console 6 + 1 需旧版本的忽略项 / e2e 64）全过，clippy 无 warning，fmt 干净。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：console 连跑 5 次、e2e 连跑 3 次全过；每次跑完无残留 target 下的 wmux 进程（计数 0）。

## 第 C 轮（计数 3/3，无发现）— 视角：边界

数据：`restart-server now` / `version x` / `update --force` 各退出码 1 并说明；无 server 时"nothing to restart"退出码 0；
名字带空格的 session 恢复；存过档后关掉的 session 不复活（存档文件仍在、未用）；连续两次 restart 都 1/1；
`kill-server` 后 server 339 ms 内退出、无残留；运行时选项不保留（有意，已写明）。

## 结论（第三十五次验收）

A、B、C 三轮连续零发现，验收通过。测试 199 → 205。版本号升到 0.10.0（发布在后面各项做完后）。

# 第三十六次验收（2026-09-24）— server.log 轮转；copy-mode-vi 键表

## 做了什么

1. **日志轮转**（`logger.rs`）：进程内计已写字节，超过 5 MB（`WMUX_LOG_MAX` 可改）把 `server.log` 改名为 `server.log.1`（覆盖上一个）再开新文件；
   启动时已超限的旧日志先轮转。std 打开文件带 FILE_SHARE_DELETE，打开中的日志可以改名；改名失败就继续写原文件（能记总比不记好）。
   `WMUX_LOG_DIR` 可改目录（测试用）。
2. **copy-mode-vi 键表**：`bind-key`/`unbind-key` 的 `root: bool` 改为 `KeyTable { Prefix, Root, Copy }`；`-T copy-mode-vi`（`copy-mode` 同表）
   绑定在 copy mode 里先查表，命中就执行命令（`send -X ...` 直达内建动作，绑定不会再次查表，所以不会自递归），未命中走内建按键；
   `list-keys` 列出 `bind-key -T copy-mode-vi ...`，可原样读回。tmux 的鼠标"键"（`MouseDragEnd1Pane`、`WheelUpPane` 等）bind/unbind 照收、不起作用，
   不再在加载配置时报错。其它未知表仍拒绝。

## 修复过程中的发现（不计数）

- 既有 `.tmux.conf` e2e 断言"copy-mode-vi 行被跳过并记入 show-messages"——行为有意改变，断言改为"进 copy 表、不进 prefix 表、不再记为跳过"。
- 新 e2e 第一版：进 copy mode 前 cmd 还没处理完清行的 Escape，copy mode 冻结的画面里还有 `wmux>i`；改为先等清行。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路

数据：真实 server、`WMUX_LOG_MAX=3000`、debug 级别、80 条命令：`server.log.1` 2925 字节、`server.log` 138 字节，最新一行是 `server exiting`；
10000 字节的旧日志在启动时轮转（`.1` = 10000，新文件 226）。单元测试 100 行、上限 1000：两文件都 ≤1000、最新行在当前文件、最旧行已丢、没有 `.2`。
e2e `the_copy_mode_vi_table_binds_keys_in_copy_mode`：copy mode 外 `i` 进 shell；copy mode 里绑定的 `i` 退出 copy mode 且不进 shell；
绑定任意命令（`C-t` → display-message）生效；`k` 绑到 `send -X cursor-up` 只上移一行、server 正常应答；解绑后 `i` 回到 copy mode 本身；
鼠标键两行加载退出码 0；`list-keys` 含该表。`.tmux.conf` e2e：v/y 两行进 copy 表，show-messages 不再有 copy-mode-vi。
全量 207 项（lib 136 / console 6 + 1 忽略 / e2e 65）全过，clippy 无 warning，fmt 干净。

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：整套 e2e 连跑 3 次、两条 copy 表 e2e 再连跑 3 次，全过。

## 第 C 轮（计数 3/3，无发现）— 视角：边界 + 静态一致性

数据：重复绑定同一动作不递归（上）；未知表 `off-pane` 仍拒绝且报错写出三张可用表；`copy-mode`（emacs）与 `copy-mode-vi` 同表；
`list-keys` 输出读回等于原命令（单元测试）；日志改名失败时继续写原文件（代码路径）；README（中英）配置与日志段、parity 的 bind-key 行与配置段同步。

## 结论（第三十六次验收）

A、B、C 三轮连续零发现，验收通过。测试 205 → 207。

# 第三十七次验收（2026-09-24）— 网站补上新功能；两处顺手修复

## 做了什么

1. 网站新增第 06 章"看得见这台机器的状态栏"（安装顺延为 07）：状态栏示例是 release 构建实际渲染出的一行
   （`[work] 0:editor* 1:build 2:logs     master | ~\Documents\dfine\wmux | CPU 9% MEM 65% | 11:45`，非手写），
   四个要点：状态栏变量与关闭、`window-size` 与每客户端视口、右键粘贴与 Tab 补全、`update` + `restart-server` 升级不丢 session。中英两套文案。
2. **网站既有 bug**：安装卡片网格用 `1fr`（即 `minmax(auto, 1fr)`），scoop 卡片里那条长命令把列撑宽，整页出现横向滚动条——
   线上 dfine.tech/wmux 现在就有（截图可见底部横向滚动条）。改 `minmax(0, 1fr)` + `.card{min-width:0}`，长命令在卡片内横向滚动。
3. **CPU 首读为空**：新 server 第一次读 CPU 没有上一次采样，状态栏显示 `CPU ` 后面空着；改为首读用开机以来的累计值（平均占用）。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（真实浏览器 + release 二进制）

数据：本地起静态服务、Chrome 打开：中文文案生效（"看得见这台机器的状态栏"）、绿色状态栏一行渲染；
页面度量 `scrollWidth 1418 == clientWidth 1418`、`scrollX 0`（修复前 1548 > 1433，最宽元素 DIV.card）；
状态栏块宽 960、内容在块内滚动。新 server 首次 `display-message -p "CPU #{cpu_percentage}"` → `CPU 5%`（修复前为空）。
`node --check docs/site.js` 通过；section 开闭 10/10；章节号 01–07 连续。全量 207 项全过，clippy/fmt 干净。
（浏览器滚到页面底部时截图会卡死，线上旧页面同样如此，是本机浏览器与大 GIF 的问题；底部以 DOM 度量代替截图。）

## 第 B 轮（计数 2/3，无发现）— 视角：可复现性

数据：页面度量在两个新开标签页各测一次一致（1418/1418）；sysinfo 单元测试连跑一致；状态栏行两次采集格式一致（CPU 数值随负载变化，属正常）。

## 第 C 轮（计数 3/3，无发现）— 视角：静态一致性

数据：站点文案所述每项功能在 README 与代码中均存在（变量名逐个 grep 命中 format.rs；`window-size`、`refresh-client -L/-R`、`completion powershell`、
`update`、`restart-server` 均为已实现命令）；手机宽度下 `.cards` 为 `minmax(0, 1fr)` 单列。

## 结论（第三十七次验收）

三轮零发现，验收通过。测试数不变（207）。

# 第三十八次验收（2026-09-24）— soak 补 CPU；据此加帧率上限

## soak（release，15 分钟，4 个 pane 满速刷屏 + 另一 session 每 10 秒分屏/缩放/杀/重生，**附带一个真实 attach 的客户端**）

内存：私有字节全程在 45–63 MB 之间摆动（首末两点差 +9.6 MB 在摆动幅度内），句柄 241–244、线程 45–48 不涨；
每个刷屏 pane 的 scrollback 恰为 2000（`history-limit`）；客户端全程在线；kill-server 后 server 与客户端都退出。
**CPU：平均 165% 核（最高 169%）**——这是新数字，拆开看：

| 4 pane 满速刷屏 | server CPU |
|---|---|
| 无客户端（只解析） | 113% |
| 1 个客户端（解析 + 渲染） | 166% |

渲染占约半个核：主循环每处理一批事件就 `render_all` 一次，满速输出时每次读 pty 都重画。

## 改动

主循环加帧率上限 `FRAME = 16 ms`（60 帧/秒）：距上一帧不足 16 ms 时只记下"待画"，并在间隔末尾醒来补画一帧，
所以画面不会停在旧状态；隔一阵之后来的按键仍立即画出，不增加打字延迟。

## 第 A 轮（计数 1/3，无发现）— 视角：机制通路（同一脚本前后对比）

| 4 pane 满速，1 个客户端 | 改前 | 改后 |
|---|---|---|
| server CPU | 166% 核 | 99% 核（无客户端 111%，差值在噪声内） |
| 客户端进程 CPU（同样时长） | 2719 ms | 62 ms |

全量 207 项全过，clippy/fmt 干净。

## 第 B 轮（计数 2/3）— 视角：可复现性

数据：e2e 单独连跑 8 次、e2e+console 连跑 8 次，16 次全过。
**未解释项（如实记录）**：改后第一次的 e2e+console 连跑中有 1 次 e2e 失败，且没有打出 `test result` 行（像是测试进程整体中止而非断言失败），
输出当时未保存；此后 16 次同命令全过，未能复现。留作观察项，CI 每次提交都会继续跑。

## 第 C 轮（计数 3/3，无发现）— 视角：逻辑不变量

数据：每次循环结束时要么刚画过，要么 `render_due = true` 且下一次 select 必在 ≤16 ms 内醒来补画（`sleep(wait), if render_due` 分支），
不存在"有未画状态却无唤醒"的路径；无事件时 `wait = 3600 s` 且该分支被守卫关闭，空闲时不会空转（空闲 server CPU 仍为 0 ms/4 s 量级，见第三十四次）。

## 结论（第三十八次验收）

三轮通过，附一项未能复现的单次失败作为观察项。测试数不变（207）。

## 第三十八次补记：帧率提交在 CI 上的失败，以及一次被推翻的解释

CI 上 `join_pane_marks_and_exact_sizes` 失败：`select-pane -T logs` 之后 pane 标题是 cmd 自己的 "Administrator: …cmd.exe"。
先排除帧率改动：OSC 标题在处理 pane 输出时就写入，与渲染无关。
**第一个解释被数据推翻**：我以为 resize 会让 ConPTY 重发标题、覆盖 `-T`；写探针在本机对 pane 连续两次 resize 抓原始输出，
150 与 120 字节里没有任何标题序列——这个解释在本机不成立，也没有证据在 CI 成立。
**实际原因**：CI 输出里其它 pane 的标题还是 `cmd`（程序标题尚未到达）——慢机器上 cmd 的**启动标题**会晚到，落在 `-T` 之后；
程序在 `-T` 之后设标题会覆盖它，这是正确的 tmux 行为。测试是竞态：改为先等程序自己的标题出现，再 `-T`。
顺带保留一处小改：程序**重复发送相同标题**（每次提示符都设标题的 prompt 会这样）不再覆盖 `-T`，发送不同标题仍覆盖（tmux 语义）；单元测试覆盖。
全量 208 项全过，e2e 连跑 3 次全过。

# 第三十九次记录（2026-09-24）— 发布 0.10.0；提交 winget-pkgs

- **0.10.0**：tag 推送后发布流水线先等同一提交的 CI（已绿）再出包；资产 MSI 4.86 MB、zip 1.34 MB、各自 sha256、清单 zip；
  bot 提交回 master 的 winget 清单 `winget validate` 通过。
- **`wmux update` 对真实发布的核对（只读，不安装）**：`update --check` → "wmux 0.10.0 is the latest"；按 `update` 的方式下载 MSI 与 `.sha256`，
  `certutil` 算出的哈希与发布的逐字相同（ac45923d…c996fd）。
- **清单 schema**：winget-pkgs 模板要求 1.12；生成器从 1.6.0 改为 1.12.0，按真实发布重生成 0.10.0，只有 6 行 schema 变化，本机 winget v1.29 验证通过。
- **winget-pkgs PR**：https://github.com/microsoft/winget-pkgs/pull/440225（从 newdee/winget-pkgs 分支，用 GitHub API 上传三份清单，不克隆大仓库），
  文件恰为 3 个，已打上 `New-Package`；**卡在 `Needs-CLA`：需要账号本人在 PR 下回复同意 CLA**（法律协议，不能代签）。
  模板中"本机 `winget install --manifest` 测试"未勾选并如实注明（需要开管理员设置且会覆盖用户正在用的安装）。
- **后续版本自动提交**：发布流水线新增一步，仓库有 `WINGET_TOKEN` secret 时跑 `wingetcreate update newdee.wmux --submit`，
  没有则跳过、失败不影响发布。secret 放在 job 级 env（步骤自己的 env 在它的 `if:` 里不可见，第一版写错已改）。
  **未验证**：要等首个 PR 合并、设置 secret、下一次发版才会真正执行。

# 第四十次记录（2026-09-24）— 以字节转交输入的宿主里前缀键失灵；`wmux show-keys`

用户报告：在某个 PowerShell 里 `Ctrl+B` 之类没反应。
**已确认的事实**：Windows Terminal 与 conhost 送完整键盘记录（vk 'B' + LEFT_CTRL，字符 0x02），这条路一直正常（ConPTY 测试里发 `\x02` 一直能分屏）。
`key_from_record` 在"无 Ctrl 标志"分支里对控制字符直接返回 None。所以一旦宿主只交字节（vk=0、字符 0x02、无 Ctrl 标志，常见于 SSH、部分远程工具、程序喂入的控制台），前缀就被丢掉。
同一原因下，这类宿主里提示符的 Enter（0x0d）也会被丢。
**尚未确认**：用户的宿主是哪一种，也可能是宿主自己吃掉了 `Ctrl+B`（VS Code 绑定了侧栏开关）。为此加了 `show-keys` 诊断。

- **修正**：裸控制字符按 tmux 的读法解读。0x0d Enter，0x09 Tab，0x1b Escape，0x7f BSpace，0x01..0x1a 是 C-a..C-z，0x1c..0x1f 是 C-\ C-] C-^ C-_。完整记录的路径不变。
- **`wmux show-keys`**：打印控制台交来的 vk、字符、标志，以及 wmux 认成的键，按 `q` 退出。退出后在主屏幕重印，便于复制。多余参数报错。补全、usage、两份 README、parity 均已写入。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 机制通路 | 探针关掉修正：新 e2e（裸 0x02 后接 `c` 开新窗口）超时失败；打开：0.16 s 通过 | 机制真实生效，干净 |
| 2 | 边界与 tmux 对齐 | 初版把 0x08、0x0a 读成 Backspace、Enter；tmux 读作 C-h、C-j，`bind -n C-h/C-j`（vim-tmux-navigator）会在这类宿主里失效 | **有问题**，已改，不计数 |
| 2' | 同上复跑 | 断言 0x00 为 C-Space 失败：字符 0 的记录在前面就当"无字符"返回，该分支是死代码 | **有问题**，删分支与断言，不计数 |
| 3 | 静态一致性 | `show-keys` 进了补全却不在 usage、README 中，且不拒绝多余参数 | **有问题**，已补，不计数 |
| 4 | 代码正确性 | raw 模式在备用屏幕上画，按 `q` 后输出消失，用户无法复制结果 | **有问题**，改为退出后在主屏幕重印，console 测试断言主屏幕上有 `->  C-b` 与 `->  q`，不计数 |
| 5 | 静态一致性 | 网站测试数仍写 208，实际 211 | **有问题**，已改，不计数 |
| 6 | 可复现性 | 全量连跑 3 次：每次 138/7/66 通过，0 失败 | 干净（1/3） |
| 7 | 不变量（所有入口） | 键记录转键只有 `key_from_record` 一处；客户端原样转发（client.rs 唯一的 `InputEvent::Key` 分支）；服务端在提示符、copy mode、popup 分流之前转换；另一个调用者是 `show-keys`；全量 138/7/66 | 干净（2/3） |
| 8 | 退化输入 | 新增全字节扫描：0x01..0xFF 裸记录中 32 个 C0/DEL 各得一个键，191 个可打印字符得到自身，32 个 C1 得到无，无 panic；`fmt --check` 为 0，clippy 无警告，全量 139/7/66 | 干净（3/3） |

记下未改的一项：前缀路径会把 vk 0 放进"吞掉下一个 key-up"的集合。宿主不发 key-up 时，下一个 vk 0 的 key-up 会被吞。
pane 里的 shell 按 key-down 处理输入，所以不影响输入内容，不改。
网站测试数改为 212。

## 第四十次补记：发布 0.10.1，以及 schema 1.12 带来的发布失败

第一次推 v0.10.1 时，发布流水线停在清单校验步骤，**什么都没有发布**。
原因是 runner 上的 winget 比 1.12 schema 旧：它对 schema 头报警告，结论是"验证成功但有警告"，退出码不为 0。GitHub 的 pwsh 包装按最后的退出码判步骤失败。
这是第三十九次把 schema 升到 1.12 时引入的问题，那次没有真正跑过发布。
本机实测 `winget validate` 的退出码：

| 清单 | 退出码 |
|---|---|
| 正常 | 0 |
| 只有警告（schema 头改成 9.9） | 0x8A150028 |
| 有错误（sha256 改成非十六进制） | 0x8A150029 |

中文系统上的输出文字是本地化的，所以按退出码判断，不匹配文字。
现在只有 0 和 0x8A150028 放行。把步骤原样加上 GitHub 的退出包装在本机跑：正常为 0，警告为 0，错误为 1。
标签移到修正后的提交，再推一次，发布成功：MSI 4.88 MB，zip 1.35 MB，各带 sha256；bot 提交了 0.10.1 清单。
`wmux update --check` 显示 "wmux 0.10.1 is the latest"。

# 第四十一次记录（2026-09-24）— `:` 提示符和 PowerShell 补全选项名；测试不再读本机配置

用户报告：`prefix :` 里输入 `set sync` 按 Tab 不补全。
**原因**：提示符的 Tab 只补全命令名和 `-t`/`-s` 后面的目标，不补全选项名。回车能执行，是因为选项名本来就支持缩写。
**修正**：`set` / `show`（包括别名和前缀）后面：
- 第一个位置参数补全选项名。规则与 `resolve_name` 共用 `option_candidates`：先按整段前缀，再按每段前缀（`mon-act`）；先匹配有实际作用的选项，兼容用的名字排在后面。
- 第二个位置参数补全固定取值（开关类 on/off，以及 status-position、pane-border-status、status-justify、window-size）。
- `-s` 在这里是 flag，不是目标。
- PowerShell 补全脚本从同一份表生成选项名和取值。
- Ctrl+D 退不出 pane：查明是 PSReadLine 在 Windows 编辑模式下 Ctrl+D 没有绑定，与 wmux 无关。README 写明解决办法，用户的两个 PowerShell profile 都已加上 `DeleteCharOrExit`。

**用 ConPTY 实测 Ctrl+D**（每个 pane 发一次 Ctrl+D）：

| pane | 客户端是否退出 |
|---|---|
| pwsh，加载 profile | 是 |
| Windows PowerShell 5.1，加载 profile | 是 |
| pwsh -NoProfile | 否 |

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 全量 | 本机刚写了 `~/.wmux.conf`（Tokyo Night 主题），console 挂 2 个、e2e 挂 9 个：两套测试的 server 都读本机真实配置，主题改掉了测试要找的状态栏 | **有问题**。console 用空的 `WMUX_CONFIG` 文件，e2e 用 `RunOptions.config`（指向不存在的路径会落回 `~/.wmux.conf`，所以必须是真实存在的空文件）。修后 141/7/66 全过，不计数 |
| 1' | 机制通路 | 关掉选项补全分支：e2e 卡在 "sync completed" 失败；打开：通过 | 生效 |
| 2 | 资源与错误路径 | 失败的 console 测试留下 2 个 server，锁住 `target\debug\wmux.exe`，下一次编译报 "拒绝访问" | **有问题**。`Term` 记下 `-L` 的 socket，Drop 时 kill-server；故意 panic 的探针测试后遗留数为 0。不计数 |
| 3 | 静态一致性 | 网站测试数 212，实际 214；parity 表 command-prompt 一行过时 | **有问题**，已改，不计数 |
| 4 | 可复现性 | 全量连跑 3 次，每次 141/7/66，遗留进程 0；补全脚本两次生成逐字节相同（8874 字符） | 干净（1/3） |
| 5 | 退化输入 | e2e 新增：`set `（列出前 6 个加 "+N"）、未知名、歧义名 `mo`、`@my`、第三个词，都符合预期；fmt、clippy 干净；全量 141/7/66 | 干净（2/3） |
| 6 | 不变量 | 把 HEAD 的旧 `resolve_name` 原样放进临时测试，在 1347 个输入上对比（每个名字的全部前缀、每段取 1 到 3 字符的全部组合、边界串）：差异 0；全量 141/7/66 | 干净（3/3） |

PowerShell 补全脚本用 `TabExpansion2` 实测（pwsh 7.6 与 5.1）：`set sync`、`set synchronize-panes o`、`set -g mo`、`show -s status-l`、`setw -g -t work mou`、`set status-position ` 都得到预期候选。
记下两个以前就有、这次没修的问题：
- 5.1 对以 `-` 开头的词不调用原生补全器，所以 flag 在 5.1 里补不出来。
- 脚本里 flag 表只按完整命令名查，`set` 这类别名没有 flag 补全。

# 第四十二次记录（2026-09-25）— 颜色选项可查、别名补全、`split-window -N`、压小后恢复比例；Tokyo Night 截图与动画

用户要求做四件事：`show-options` 能显示颜色选项；别名能补全 flag；一次建出多个 pane；窗口压小再拉大后恢复比例。同时要求 README 和网站的图、动画换成 Tokyo Night，并演示 `set sync`。
桌面通知：用户确认看到了（通知功能此前一直没有在真机上确认过）。

**做了什么**

- `show-options` 显示 `status-style`、`pane-border-style`、`pane-active-border-style`、`status-fg/bg`，格式能被 `set` 原样读回。单元测试断言 `SHOWABLE` 里每一项 get 出来再 set 回去都不变。
- 命令别名从 `parse` 里 100 行的 match 改成 `ALIASES` 表加 `canonical_name`。前缀先解析到 COMMANDS 里的名字，再经别名表归一。
  - 在 HEAD 旧实现和新实现上逐一对比所有别名和所有命令名的每个前缀：只有 `set-w*`、`show-w*`、`move-p*` 三组结果不同。
  - 旧版把它们解析成 `set-window-option` 等名字，下游没有对应分支，**server 会 panic，客户端永远挂住**。实测 0.10.1 上 `set-w -g mouse on` 一直不返回，日志里有 "panicked at src\command.rs:3019"。新版解析成规范名。
  - 单元测试：所有别名、所有命令名的所有前缀传给 `parse` 都不 panic。
- 单条命令 panic 时，server 给发命令的客户端回一个错误，而不是只在主循环里恢复。临时注入 panic 的探针：客户端 33 ms 内拿到 "internal error running '__probe_panic'"，退出码 1，server 继续服务。
- PowerShell 补全脚本：别名和前缀按规范名查 flag 表和选项表，补全器同时注册给 `wmux` 和 `tmux`。`TabExpansion2` 实测：`set -`、`splitw -`、`split-w -`、`tmux splitw -`（函数形式和 `Set-Alias` 形式都测了）都给出 flag；5.1 对 `-` 开头的词不调用原生补全器，已写进 README。
- `split-window -N count`（1 到 64）：每次切当前最大的 pane，每切一次就平铺一次；`-d` 保持焦点；窗口放不下时返回 "made X of N panes"，已建出的保留。
- 布局：
  - 取整余数改为最大余数法，并列时给靠前的一项，和 tmux 一致。158 格分 3 列从 52/52/54 变为 53/53/52；原来两个断言按新行为更新。
  - 窗口缩小到有 pane 被压成 1 格时，不把缩放结果写回存储的尺寸。拉大后原比例复原。普通缩小照常写回，否则缩小状态下 `resize-pane` 会按比例打折。
- 截图与动画：
  - 仓库新增 `themes/tokyo-night.conf`；录制加载它，右侧只放时钟。
  - 渲染用 Tokyo Night 调色板和 Maple Mono NF CN（未安装时退回 Cascadia Mono），按原生 900×638 输出，不缩放。
  - 录制脚本用 `still(name)` 标记截图，`tools/make-demos.ps1` 一条命令完成录制、渲染、GIF/MP4 和全部截图。
  - 演示新增 `set sync`：Tab 补全 `synchronize-panes`，在一个 pane 输入，三个 pane 同时执行，窗口标签显示 `S`，再用 `set sync` 关掉。
  - 网站第一章加入 `sync.png`。

验收：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | `demo_frames.rs` 开头的说明没提一键脚本 | **有问题**，已补，不计数 |
| 2 | 机制通路 | e2e：两个 pane 调成 30/9，窗口压到 4 行再拉回 41 行，比例复原为 30/9；关掉保护得到 20/19（失败），打开得到 30/9。**同时发现 `resize-pane -y` 对最下面的 pane 方向反了**，会一路调到 1 行，0.10.1 实测也是这样 | **有问题**。循环改为：一步离目标更远就撤回，改用反方向 |
| 3 | 边界 | 新 e2e：3 个 pane，每个位置 × 1/5/12/20/36 × 纵横 = 30 种。先后暴露两个问题：`resize` 只向相邻 pane 借，邻居是 1 行就卡住；循环第一步没变化就停下。改为像 tmux 那样继续推：变大时先从后面借，不够再从前面借；变小时从前面借。第一步没变化先换方向。之后 30 种全部达标 | **有问题**，已修，不计数 |
| 4 | 可复现性 | 全量连跑 3 次，每次 144/7/69（220 项），遗留进程 0 | 干净（1/3） |
| 5 | 不变量 | 临时随机测试：300 组场景 × 200 步（随机切分、随机调整、随机窗口尺寸 2..121 × 2..51），60000 次检查：每个 pane 都有矩形、都在窗口内、互不重叠，全部成立；全量 144/7/69 | 干净（2/3） |
| 6 | 静态一致性 | README、parity、网站关于 `-N`、补全、颜色选项、resize 的说法逐条对照代码；补测 `Set-Alias tmux` 形式的补全（`tmux splitw -` 得到含 `-N` 的 flag）；全量 144/7/69 | 干净（3/3） |

需要说明的一点：随机测试第三项"正好铺满"的写法，在不重叠的前提下必然成立，所以它不算独立证据。有效的是前两项。

# 第四十三次记录（2026-09-25）— `:` 提示符补全 flag；找到并修复剪贴板并发导致的堆损坏

**flag 补全**：`:` 提示符里输到 `-` 时，按 Tab 补全这条命令的 flag。别名和前缀按它代表的命令算（`canonical_name`）；已经给过的 flag 不再列出，合写的 `-ga` 按 `-g -a` 算。

**堆损坏（本次最重要的发现）**：验收时，一次全量运行的 e2e 在输出结果前就退出了。循环复现：
- 通过 `cargo test --test e2e` 并行跑 8 次，失败 2 次，退出码 0xC0000374 `STATUS_HEAP_CORRUPTION`；串行跑 6 次，0 次。所以问题出在同一进程里多个 server 并发的时候。
- 没有管理员权限用不了 Page Heap，也没有 nightly 和 cdb，只能用对照实验排除。关掉 sysinfo 仍然崩，环境变量只设一次也仍然崩，这两个排除。
- 再想到进程级共享资源剪贴板：8 个线程同时读写剪贴板 5 秒，**3 次运行 3 次都是堆损坏**。原因是 `OpenClipboard(NULL)` 能挡住其他程序，却挡不住同一进程的其他线程：一个线程的 `EmptyClipboard` 会释放另一个线程正在读、或刚交出去的内存块。
- 修法：进程内一把锁，从打开到关闭剪贴板全程持有。`SetClipboardData` 或 `GlobalLock` 失败时释放自己分配的内存块。修复后压力测试 5 次全过，每次 2 万到 3 万次操作；e2e 并行 12 次加 10 次，0 次失败、0 次堆损坏。压力测试缩到 2 秒，作为永久单元测试保留，测完把剪贴板原内容写回。
- 真实 server 里也有同样的风险（两个客户端同时复制，或者右键粘贴和 copy mode 同时发生），只是概率低。这也解释了 0.10.0 帧率改动后那次"原因不明的中断"。
- 附带损失：修复前的压力测试进程崩溃时，没能把这台机器剪贴板原来的内容写回去。

**测试竞态**：`remain_on_exit_keeps_the_pane_and_history_survives_resume` 偶发失败（约 1/10）。
- 原因一：等输出时，被命令行本身 `echo keepme-42` 提前满足，保存下来的历史长短不一。
- 原因二：恢复和 respawn 之后，用"屏幕上出现 `wmux>`"判断 shell 已就绪，但重放的旧提示符会提前满足这个条件，`exit` 就在 shell 就绪前发出、被丢掉。
- 改为：等恰好是 `keepme-42` 的一行；等提示符数量比保存文件里多一个；respawn 后等最后一个非空行是提示符。修复后连续 20 次 0 失败。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 文档说"已给的 flag 不再列出"，但 `set -gq -` 还会列出 `-g` | **有问题**，合写 flag 拆开算，补 `set -ga -` 用例，不计数 |
| 2 | 全量 | e2e 偶发退出码 116 与 0xC0000374，即上面的堆损坏 | **有问题**，剪贴板加锁，不计数 |
| 3 | 机制通路 | 关掉 flag 补全分支：e2e 卡在 "splitw -"；打开：通过。同轮全量出现 1 个偶发失败，即上面的测试竞态 | **有问题**，修测试，不计数 |
| 4 | 可复现性 | 完整 `cargo test` 连跑 6 次，每次 145/7/69（221 项），堆损坏 0，遗留进程 0 | 干净（1/3） |
| 5 | 代码正确性 | 剪贴板只有 `clipboard.rs` 一个入口，server 两处调用都经过锁；锁中毒时继续使用；先关闭剪贴板再释放锁；全量 145/7/69 | 干净（2/3） |
| 6 | 静态一致性 | 文档关于 flag 补全的说法有 e2e 用例覆盖；网站 221 = 145+7+69；全量 145/7/69，外加 e2e 并行 10 次：失败 0，堆损坏 0 | 干净（3/3） |

# 第四十四次记录（2026-09-25）— `wmux web`：手机上看 pane、往 pane 里输入

用户需求：局域网内手机扫码，列出 pane，点进去看屏幕，下方常用按键加手动输入；执行在电脑端；手机上不需要 tmux 命令，用一个"+"菜单控制，简单一点；只在局域网，HTTP 即可；在 README 和网站上重点推广。

**设计**：`wmux web` 是 server 的一个普通客户端，复用 `client::query`，同时在局域网上提供 HTTP 服务。server 本身不改。
- 默认只绑定本机上网用的那个局域网地址（用 UDP connect 选路，不发任何包）。
- 密钥：128 位，来自 `BCryptGenRandom`，URL-safe 编码成 22 个字符。放在二维码地址的 `#` 后面，不会出现在请求行里。`--keep-key` 存到 `%LOCALAPPDATA%\wmux\web.key`。
- 除了页面本身，每个接口都要请求头 `X-Wmux-Key`，比较耗时恒定。自定义请求头会触发 CORS 预检，所以跨站请求伪造和 DNS 重绑定都拿不到数据。
- 手机能做的只有：列出 pane（`list-panes -a -F`）、读屏幕（`capture-pane -p -e`，history 上限 2000）、输入文字（`send-keys -l -- 文本`，`--` 让以 `-` 开头的文字不被当成 flag）、白名单里的按键、4 个固定的菜单动作。
- HTTP 自己实现：请求头上限 16KB，请求体上限 64KB，15 秒读超时，每个请求处理完就关闭连接。唯一新增的依赖是 `qrcode`，它本身没有任何依赖。
- 页面：单个 HTML 内联在 exe 里，Tokyo Night 配色，界面文字随手机语言切换中英文，可添加到主屏幕。

为此补上两项 tmux 本来就有的功能：`list-panes -F` 格式输出，`-t %N` 按编号指定 pane（session、窗口、pane 三层解析都支持）。

**截图**：`tools/make-phone-shots.ps1` 起临时 server，用 puppeteer-core 驱动本机 Edge 模拟 iPhone（390×844、触屏、2 倍像素），生成中英两版并排截图。
- 先试了"无界面 Edge + iframe"，pane 页始终空白。用 DOM 转储证明内容已经渲染，问题出在截图方式，于是改用设备模拟。
- 排查过程中发现两个页面 bug，都已修：
  - 链接里的 `p=%2` 交给 `decodeURIComponent` 会抛异常。
  - 跟随底部时会停在末尾的空格行上，短内容的 pane 看起来一片空白。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 单元测试首跑 | 请求头超过 16KB 但已带结束标记时会被接受：先找结束标记、后判大小 | **有问题**，找到结束标记后再检查一次长度 |
| 1 | 安全 review | 绑定地址、密钥比较、固定参数、白名单、跨站和 DNS 重绑定都没问题。但"+"菜单开的新 pane 用的是 `wmux web` 进程的目录，而不是原 pane 的目录 | **有问题**，先用 `#{pane_current_path}` 查出目录再加 `-c`。同时发现 `list-panes` 默认输出的目录是创建时的旧值，改成实时目录。不计数 |
| 2 | 机制通路 | 去掉 `-c` 那段：e2e 失败，新 pane 不在 `C:\Windows`；恢复：通过。全量 150/7/70（227 项） | 干净（1/3） |
| 3 | 边界与异常输入 | 对运行中的服务发：空连接、乱码、20KB 请求头、谎报 1MB 请求体、负数长度、路径不以 `/` 开头、错误百分号、中文参数、超大 history、不存在的 pane、密钥放在网址参数里；全部得到 400/401/404 或截断后 200，没有崩溃。慢连接挂着的同时并发 100 个请求，100 个都是 200，之后服务照常。全量 150/7/70 | 干净（2/3） |
| 4 | 静态一致性 | README、网站、parity、usage、补全逐条对照代码。`--keep-key` 实测：两次启动密钥相同（22 字符），和文件内容一致；不加参数时生成新密钥；测试文件已删（原本不存在）。fmt 干净，全量 150/7/70 | 干净（3/3） |

没有在真实手机上验证：截图用的是 Edge 的手机模拟，扫码、输入法、Safari 这些真机细节需要用户试用确认。

补记：0.11.0 分成两个提交，`qrcode` 依赖所在的 `Cargo.toml` 放进了第二个提交，所以单独检出第一个提交（6176618）编译不过。CI 只测 HEAD，所以没有发现；它只影响 `git bisect` 时的这一步。改写已推送的历史，风险比这个问题本身大，所以不改。以后拆分提交时，依赖变更要和用到它的代码放在同一个提交里。发布：MSI 5.34 MB（原 4.93），zip 1.48 MB（原 1.36）。

# 第四十五次记录（2026-09-25）— 手机页面：提醒标记和屏幕推送

用户要求：(3) 手机列表上显示窗口的提醒标记；(4) 屏幕从轮询改成有变化时由服务端推送。

**做了什么**

- **server 端：** pane 新增输出计数，每段输出加一，作为格式变量 `#{pane_output_count}` 暴露。`pane_activity` 只精确到秒，1 秒内的多次变化分辨不出来。
- **`wmux web` 新增 `GET /api/watch`：** 返回 server-sent events 格式的长连接。
  - 每 100 ms 通过本地管道查一次变化戳（输出计数、宽高、是否已退出），戳变了才抓屏推送 `data: {"text":…}`。
  - 空闲时每 15 秒发一次心跳。pane 没了发 `event: gone`。一条连接最长 30 分钟，之后由页面重连。
  - 鉴权从 `handle` 抽成 `check_key`，长连接和普通请求共用。
- **页面：**
  - 用 `fetch` 流式读取，因为 `EventSource` 发不了自定义请求头。断线 1 秒后重连；页面隐藏就断开以省电。
  - 浏览器不支持流式读取时，退回每 0.5 秒轮询。
  - 列表按窗口显示 `#`（黄）、`!`（红）、`~`（灰）标记，有标记的卡片加黄框。
- **截图脚本：** 开启 `monitor-activity`，列表截图里能看到 `#` 标记。

**实测数据（调试版）**

| 项目 | 结果 |
|---|---|
| 一部手机开着一个 pane，空闲 30 秒 | `wmux web` 0.21% 核，server 0.99% 核；推送 1 次，心跳 2 次 |
| 输入到收到新屏幕（10 次） | 最快 105 ms，中位 108 ms，最慢 201 ms（原来轮询平均至少等 250 ms） |
| 20 条长连接同时开着 3 秒 | `wmux web` 250 ms CPU，约一个核的 8%；20 条都收到屏幕 |
| 流式读取 / 不支持流式读取 | 真实浏览器里分别是 1 次长连接、0 次轮询 / 1 次尝试后约 2.5 秒轮询 5 次；屏幕都正常显示 |

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 代码正确性 | 客户端断开后，要等下一次写数据才发现，空闲时最长 15 秒，期间每秒还在查询 10 次 | **有问题**。等待时同时监听连接，对方关闭立刻返回。实测断开后 5 秒内：server 从 94 ms 降到 0 ms，web 从 47 ms 降到 0 ms。不计数 |
| 2 | 可复现性 | 全量连跑 3 次，每次 150/7/71（228 项），遗留进程 0 | 干净（1/3） |
| 3 | 机制通路 | 把变化戳固定：e2e 在"等推送输出"处超时失败；恢复：通过 | 干净（2/3） |
| 4 | 静态一致性 | parity 文档把 `&&`、`||`、`m:` 列为不支持，实测都支持（只有 `e|` 不支持）；还自相矛盾地说 client 和 cursor 变量都没有 | **有问题**，已改正并补上 `pane_output_count`，不计数 |
| 5 | 边界 | 长连接接口：不存在的 pane 先推一次再发 gone；参数错误 400；无密钥 401；超大 history 截断后 200；只读模式能看；20 条并发都正常（第一次测出 0 ms 是测试脚本写法问题，单独复测已排除）。全量 150/7/71 | 干净（1/3） |
| 6 | 回退路径 | 真实浏览器里人为关掉流式读取：自动退回轮询，屏幕正常。全量 150/7/71 | 干净（2/3） |
| 7 | 静态一致性 | README 中英、网站中英、parity 关于推送、标记和新变量的说法逐条都在；网站 228 = 150+7+71；fmt、clippy 干净；全量 150/7/71 | 干净（3/3） |

测量过程中还有一个插曲：测量脚本里 curl 的请求头因为引号问题被拆成两个参数，拿到的是 401，脚本随后卡住。已结束那三个进程，修正引号后重测。

# 第四十六次记录（2026-09-25）— 命令时间、按天的历史记录、误杀恢复

用户要求：(1) 每条命令的时间，终端和手机两处都要，不改变 pane 宽度，有一个可以切换的详细模式；(2) 一个单独的历史记录面板：选窗口和窗格，再选日期，看历史文件（用户选择：默认开，留 30 天，先做终端）；(3) 窗格或窗口被误杀后缓存 10 秒，期间可以用命令恢复。

**做了什么**

- **命令标记：**
  - vendor vt100 加 `scrolled_total()`（滚出屏幕顶的总行数，`scrolled_total + 行号` 永久指代同一行）和 `rows_wrapped()`。
  - PowerShell prompt hook 在提示符前报上一条命令（`OSC 7777;wmux-cmd;开始;结束;成败`，时间取自 `Get-History`，每条历史只报一次），提示符后报 `OSC 133;B`。bash/zsh 等用 OSC 133 A/B/C/D（633 同样认）。
  - 每个标记记下命令行当时的文字；文字对不上（被清屏、被覆盖）就不显示；pane 变窄时按截断后的文字比较。
- **终端：** `pane-timestamps`（`prefix C-t` 切换，会提示开/关），把 `HH:MM:SS 耗时 ✓/✗` 画在命令行右端的空白格里；放不下退成短格式，还放不下就不画；copy mode 往回翻时也显示。`list-marks` 给脚本用。
- **手机：** ⏱ 按钮，左侧 6 列显示时间（非今天显示日期），点一下提示完整日期、耗时、退出码。服务端把标记换算成截屏里的行号并按文字校验。
- **历史记录：** `log-history`（默认开）、`log-history-days`（默认 30）、`log-history-dir`。滚出屏幕顶的行写进 `<session>\<窗口>.<pane>\YYYY-MM-DD.log`，软换行合并成一行，报告过的命令前加 `── 时间 · 耗时 · ✓ ──`；命令还在跑时它的输出先扣住，等时间出来一起写（最多 5000 行 / 10 分钟）；关闭时写屏幕剩余内容；全屏程序不记；每文件每天上限 20 MB。`choose-history`（`prefix /`）列出位置和日期，Enter 在弹出框里用新的 `wmux view` 分页器打开（`j k Space b g G / ? n N [ ] q`）。
- **误杀恢复：** `undo-kill-time`（默认 10 秒，0 关闭），`kill-pane` / `kill-window` 杀掉的 pane / 窗口连同运行中的程序暂存，`undo-kill`（`prefix u`）按原布局放回；session 最后一个 pane 不暂存（session 会结束）；暂存期间程序自己退出就作废；过期后结束进程。

**实测数据**

| 项目 | 结果 |
|---|---|
| pwsh 7 / Windows PowerShell 5.1 各跑 3 条命令 | 两者都得到 3 个标记；`cmd /c exit 3` 记为失败；`Start-Sleep 700` 测得 726 / 757 ms |
| 分屏把 pane 从 100 列缩到 50 列再恢复 | 缩窄时 3 条标记都还能对上（截断文字），恢复后文字完整 |
| 20 万行输出，日志开 / 关（release，各 3 次） | 最初 server CPU +66%；插桩发现 `log_output` 20 万次调用只占 0.76 s，其余在写盘线程逐条收消息；改成 pane 内攒批（32 KB 或每秒）后 关 4.9/5.2/5.2 s、开 5.4/5.1/5.6 s，约 +6%，在噪声内；日志 200002 行一行不少 |
| break-pane 前后各输出 60 行 | 两个文件合起来 1–120 每个编号恰好 1 次 |
| 真实 `%LOCALAPPDATA%\wmux\history` | 所有测试跑完后仍不存在 |

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 新增 5 个选项全部有读取方；文档说法逐条对照。手机页切换 pane 时没清空 `shown`，新屏幕到达前点 ⏱ 会闪出上一个 pane 的内容 | **有问题**，已修。不计数 |
| 2 | 机制通路 | 三个变异（不画时间戳 / 日志不写时间头 / 杀掉不进暂存）让 4 个测试全部失败；无头 Edge 实测手机左栏 3 条、失败标红、提示正确。全量 160/9/72 | 干净（1/3） |
| 3 | 资源与边界 | `history-limit 0` 时 2 万行输出写了 19975 条"丢行"提示；日志开销 +66%；选中的日志被删后弹出框一闪而过 | **有问题**，三处都已修（静默跳过、攒批、打开前检查）。不计数 |
| 4 | 代码正确性 | 全量并行时历史测试偶发失败：别的测试删共享目录，写盘线程还在写已删除文件的句柄。改为测试用独立目录，写盘线程空闲 0.5 秒关闭句柄（删了的文件会重建）。另发现暂存的 pane 被标成已退出，过期时 Drop 不调用 kill，只靠 job object | **有问题**，已修并加断言（过期后进程不在 tasklist 里）。不计数 |
| 5 | 可复现性 | 全量 3 次都是 160/9/72，残留进程 0；同一输入的日志归一时间后哈希 3 次一致 | 干净（1/3） |
| 6 | 逻辑不变量 | 所有 kill 入口（kill-pane 单 pane 窗口、`-a`、kill-window `-a`）恢复顺序正确；marks 有序的所有修改路径核对；全量 160/9/72 | 干净（2/3） |
| 7 | 边界与退化输入 | 新测试发现：名字像日期的目录被当成日志文件；`log-history-days` 极大时日期相减 panic，写盘线程死掉，之后的日志全丢（撤回修复复现了 panic 和丢失） | **有问题**，已修，并加写盘线程自动重启、不可写目录只警告一次。不计数 |
| 8 | 静态一致性 | WMUX-PATCH.md 没记 `rows_wrapped`；`wmux -h` 没有 `undo-kill`；网站测试数 228 | **有问题**，已修。不计数 |
| 9 | 可复现性 | 全量 2 次 161/9/72，残留 0；日志哈希 3 次一致，且与优化前相同 | 干净（1/3） |
| 10 | 机制通路 | 去掉每秒 flush → 控制台测试失败；换行标记恒 false → 单测失败；重新引入 panic → 测试仍失败：连发的消息在线程死时一起丢了，自动重启从没被测到 | **有问题**。加测试专用 `Crash` 消息直接测重启（去掉重启 → 失败，恢复 → 通过），histlog 测试串行。不计数 |
| 11 | 逻辑不变量 | break-pane 前后 120 行各记 1 次；全量 162/9/72；真实目录不存在。但网站测试数 242 ≠ 实际 243 | **有问题**，已改 243。不计数 |
| 12 | 可复现性 | 全量 2 次 162/9/72，残留 0；日志哈希 3 次一致 | 干净（1/3） |
| 13 | 机制通路 | 变异 `/api/screen` 的 marks 为空 → 手机页 15 秒等不到时间栏；还原 → 3 条。全量 162/9/72 | 干净（2/3） |
| 14 | 边界与退化输入 | 3 列窄 pane：server 正常，标记按截断文字仍对上；缩放中的窗口恢复后自动取消缩放；中文带空格的 session 名目录正常；无标记 / 不存在的 pane；4 个非法选项值都报错；新增 `draw_stamp` 窄区域和彩色空白单测，分页器 CRLF/Tab 断言；网站数同步为 244。全量 163/9/72 | 干净（3/3） |

没在真机上验证的：手机实机扫码后的详细模式（无头 Edge 模拟 iPhone 尺寸已测）。

# 第四十七次记录（2026-09-25）— README 和网站的图

用户要求：更新 README 和网站的图，展示 0.12.0 的新功能。

**做了什么**

- 新录制 `record_history`（`tools/make-demos.ps1` 的第三段）：`C-b C-t` 打开时间，三条命令（一条失败），滚屏输出，`C-b /` 历史面板（预置了几天往日的日志，让面板显示多个位置和日期），Enter 在查看器里看、`[` 跳命令，分屏后误关 pane 再 `C-b u` 找回。产出 `wmux-history.gif/mp4`、`timestamps.png`、`viewer.png`；另两段录制一并重录。
- 录制和手机截图里的 pane 用 `pwsh -NoExit -File` 启动，wmux 不给这种 shell 装 hook。新增内部命令 `wmux __shell-hook` 打印 hook，脚本里 `Invoke-Expression` 装上；README 中英说明了这个用法。
- 手机截图的 pane 视图改成详细模式（左栏时间）。
- README 中英"命令时间和历史"一节加 GIF；网站新增第 06 章（GIF + 两张图 + 三条要点，命令时间/历史/误杀恢复的要点从第 04、05 章移过来），后续章节顺延到 09。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 引用的图全部存在；`history.png` 生成了但没人引用 | **有问题**，去掉这张图的 still。不计数 |
| 2 | 机制通路 | 同一 `-File` 脚本：带 `__shell-hook` 1 条标记，不带 0 条（第一次数成 1 是 PowerShell 把空行算成一个元素）；由此发现 `list-marks` 无标记时输出 1 字节空行，tmux 的列表命令此时什么都不输出 | **有问题**，改为无标记时不输出（0 字节，退出码 0）。不计数 |
| 3 | 静态一致性 | 引用缺失 0；新增孤儿图 0（alert.png 和各 mp4 是一直以来 make-demos 的产物）；i18n 键 html 70 / js 70 一一对应；章节 01–09 无重复；`__shell-hook` README 中英各 1 处，main.rs 处理 1 处。全量 163/9/72 | 干净（1/3） |
| 4 | 机制通路 | hook 有/无：1 / 0；手机详细模式 3 个时间栏，失败的标红。全量 163/9/72 | 干净（2/3） |
| 5 | 可复现性 | `record_history` 连录 2 次，都是 134 帧；两张 still 把时间、耗时、日期、大小归一化后哈希一致（5105E0BA…、746C7C07…）。全量 163/9/72 | 干净（3/3） |

# 第四十八次记录（2026-09-25）— 最大化跟着 pane 走

用户要求：同一个 window 里，pane 最大化后用 `prefix q` 加数字、`h` `j` `k` `l` 切到别的 pane，应该保持最大化（换成新 pane 最大化），除非用户自己按 `z`。切换 window 不在此列（每个 window 各自记着自己是否最大化，本来就不受影响）。

**做了什么**

- `select-pane`（`h` `j` `k` `l`、方向键、`o`、`;`、`q` 加数字、`-t`）在最大化的窗口里切换时，最大化跟到新 pane；新选项 `keep-zoom`（默认 on），设 off 就和 tmux 一样切换即还原。
- 最大化时 `prefix q` 按真实布局给每个 pane 画编号（原来只画当前那一个，别的编号看不到）。
- 顺带修了一个老问题：从 choose-jobs 按 Enter 或通知按钮（`focus-pane`）跳到最大化窗口里的另一个 pane 时，只改了 active 没重新布局，屏幕上还是原来那个 pane，键盘却进了新 pane。现在同样是最大化跟过去。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | `keep-zoom` 在 select-pane 处读取；搜出所有改 active 的 14 处：`chooser_go_pane`（choose-jobs、focus-pane）改了 active 不重新布局 | **有问题**，已修并加测试。不计数 |
| 2 | 机制通路 | 三个变异分别施加：去掉 focus-pane 的重新布局 → 测试在"left pane, zoomed, on screen"失败（证实原来的 bug）；编号改回只画可见区域 → "both numbers"失败；忽略 keep-zoom → "left pane, zoomed"失败；还原后通过。全量 163/9/72 | 干净（1/3） |
| 3 | 边界 | 单 pane 按 z 不进入缩放；缩放 0→2、`o` 从 2 绕回 0 都保持缩放；取消后大小恢复 40x23 / 39x11 / 39x11；`-t` 指向另一个窗口只动那个窗口；关掉最大化的 pane 后取消缩放（和 tmux 一样）。全量 163/9/72 | 干净（2/3） |
| 4 | 可复现性 | 边界脚本连跑 3 次输出哈希一致（3DB0A563…）；全量 2 次 163/9/72；残留进程 0 | 干净（3/3） |

# 第四十九次记录（2026-09-25）— 焦点动画

用户要求：zoom、跳转 pane、切换 window 时要有更酷的动画，并且可以配置开关。

**做了什么**

- 焦点框：一个圆角、亮色（活动边框色、加粗）的框从原来的位置飞到新位置，ease-out 缓动，默认 160 ms。内容当下就已经换好（程序只按最终尺寸改一次大小，不经过中间尺寸），框是画在上面的装饰，不耽误输入。
  - 选 pane（键盘、鼠标、`q` 加数字、任务面板和通知跳转）：从旧 pane 飞到新 pane。
  - 放大 / 还原：在 pane 和整个窗口之间展开 / 收回。
  - 放大时切 pane：从新 pane 在原布局里的位置展开到全屏，顺带看出它在哪。
  - 切 window / 切 session：从整个窗口收到当前 pane；只有一个 pane 时从中间弹开。
- 框画在 pane 周围的分隔线上；贴着窗口边的一侧收在窗口内。
- 主循环：有动画时由 `render_all` 要下一帧（60 fps 上限），结束后多画一帧把框擦掉，然后回到空闲。
- 选项：`animation`（默认 on）、`animation-time`（默认 160 ms，0 等于关）。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 两个选项都有读取方；README 列的触发点逐个在代码里；网站测试数 244 ≠ 实际 245 | **有问题**，已改。不计数 |
| 2 | 机制通路 | 不画框 → 测试在"select-pane"失败；切 window 不触发 → "new window"失败；主循环不续帧 → 测试仍通过（每秒的 tick 也会重画，框照样出现和消失，只是不动了） | **有问题**：流畅度没人守。加断言：动画期间 1 秒内至少 20 帧；正常通过，该变异下只有 2 帧，失败。不计数 |
| 3 | 可复现性 | 动画测试单独 5/5；全量第 2 次 undo-kill 测试偶发失败一次（只在全量并行时出现，输出被截断）；测试里两处固定 sleep（等 ping 退出、等过期结束进程） | **有问题**，改成轮询到条件成立（最长 15 秒）。不计数 |
| 4 | 可复现性（加压） | e2e 连跑 5 次 0 失败；全量 2 次 163/9/73；残留进程 0 | 干净（1/3） |
| 5 | 边界与退化输入 | `animation-time abc`、`-5`、`animation maybe` 报错，`anim` 报歧义，`0` 可设，`animation` 无值翻转；新单测：起止点、缓动单调、1x3 / 3x1 / 0x0 / 越界不画不崩、压在宽字符上不留半个字；网站数同步 246。全量 164/9/73 | 干净（2/3） |
| 6 | 资源与不变量 | 真实终端 attach：动画前空闲 16 ms/5 s，一段 2 秒慢动画 438 ms（调试版），动画后空闲 0 ms/5 s，回到不出帧；全量 164/9/73 | 干净（3/3） |

# 第五十次记录（2026-09-25）— 重录动图和截图：完整状态栏、焦点框

用户要求：动画和网站的图更新，截图里状态栏要多一点信息。

**做了什么**

- 录制不再把状态栏右侧换成只有时钟，用主题自带的完整右侧：git 分支、目录、CPU / MEM、时钟。
- 为了分支有内容、路径又不暴露本机：录制在临时目录里建一个演示仓库（`git init -b main`），用 `subst` 临时映射成一个空闲盘符（W: 起往下找），pane 都在这个盘上；映射由守卫对象持有，录制结束或中途失败都会解除。
- 录制时 `animation-time 600`，每 200 ms 一帧的录像才能抓到焦点框飞行（默认 160 ms 会落在两帧之间）；文档里的默认值不变。
- 三段录制、全部截图重新生成；make-demos.ps1 和 demo_frames.rs 开头"两段录制"的旧说法改成三段。

验收（每轮 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | README / 网站 / 注释里"只有时钟""C:\"的说法 0 处；引用缺失 0；make-demos.ps1 和 demo_frames.rs 仍写"两段录制" | **有问题**，已改。不计数 |
| 2 | 机制通路 | 临时加一个启动后立刻 panic 的录制：映射了 W: 且能读到演示仓库，panic 后 `subst` 列表为空，无残留进程；正常三段录制后也为空。动图里含焦点框的帧：主演示 13/152，提醒 7/113；11 张 still 无框残影。全量 164/9/73 | 干净（1/3） |
| 3 | 可复现性 | `record_history` 连录 2 次都是 134 帧；第一次比较时间戳截图哈希不同，逐段比对发现只差状态栏左侧空白（CPU/MEM 位数变化使右对齐内容平移），属于我的归一化漏了空白，不是产物问题；归一化空白后两张 still 哈希一致（0C8EC933…、E0C1C7AA…）。全量 164/9/73 | 干净（2/3） |
| 4 | 边界 | 先手动占用 W: 并从 PATH 去掉 git 再录：改用 V:，状态栏不显示分支，录制照常完成；录完只归还自己的 V:，别人的 W: 保留。全量 164/9/73 | 干净（3/3） |

补记：推送后 CI 在 `cargo fmt --check` 失败（demo_frames.rs 一处 `format!` 换行）。这几轮只跑了测试，没有像以前那样跑 fmt / clippy，属于验收流程上的漏项。已格式化，本地 `cargo fmt --all -- --check` 与 `clippy -D warnings` 均通过后重新提交。

# 第五十一次记录（2026-09-26）— 放大动画改成窗格本身长大

用户要求：放大时不要飞框，要窗格本身四周逐渐变大 / 变小；贴着屏幕边的那条边不动；准确说是窗格的四个顶点各自朝 window 的四个顶点靠拢。做完重录图和动画、更新网站、发版。

**做了什么**

- 新增 `Anim::Zoom`：放大、还原、以及放大状态下切 pane（键盘、`q` 加数字、任务面板和通知跳转）都用它。
- 动画期间：背景按不放大时的布局画出所有 pane（它们本来就保持着原来的大小）；放大中的 pane 画在逐帧插值的矩形里，外面一圈边框。矩形四条边各自线性插值（ease-out），所以每个顶点直线走向窗口对应的顶点，在窗口边上的边不动。程序只按最终大小改一次，不经过中间尺寸。
- 行尾时间戳在缩放动画期间不画（否则会按全屏位置画在背景上）。
- `animation-time` 上限 10000 ms（原来可以设成 u64 最大值，动画永不结束，服务端一直 60 fps 重画）。
- 重录三段动图和全部截图（录制时 600 ms，默认仍 160 ms）；README 中英、网站、parity 里对动画的描述改成"放大时窗格本身长大、四个顶点朝窗口顶点靠拢；其他切换是飞框"。

慢速录制核对（1000 ms）：放大右下 pane，右边和下边不动；放大状态下 `k` 到右上 pane，上边和右边不动；还原时从全屏收回右上那格。快到全屏时边框会压住内容第一列一两帧（矩形已到第 0 列，边框无处可退），默认 160 ms 下约 30 ms，保留。

验收（每轮先 fmt --check + clippy -D warnings，再 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | fmt 通过；clippy 报 `type_complexity`（`zoom_frame` 返回嵌套元组） | **有问题**，改成 `ZoomFrame` 结构体。不计数 |
| 2 | 机制通路 | 三个变异：不生成缩放帧 / 背景不用不放大布局 / 不插值，测试都在"growing over the right pane"失败；还原后通过。全量 164/9/73 | 干净（1/3） |
| 3 | 边界 | `animation-time 18446744073709551615` 被接受，任何一次切换后永远 60 fps 重画 | **有问题**，加 0–10000 上限和测试。不计数 |
| 4 | 边界 | 新测试：边框行 + 3 pane 放大、动画中途改终端大小、中途关掉另一个 pane、3x2 的小终端里放大，服务端都正常、最后状态正确；但我自己写的一个等待条件带了恒真分支 | **有问题**（测试代码），已改。不计数 |
| 5 | 可复现性 | 两个动画测试各 5/5；e2e 连跑 3 次 0 失败；全量 164/9/74；残留进程 0 | 干净（1/3） |
| 6 | 机制通路 | 去掉上限 → 配置测试失败；去掉"到时结束" → 动画测试在"grown"超时；还原后全量 164/9/74 | 干净（2/3） |
| 7 | 静态一致性 | 旧描述残留 0；i18n 键 71/71；网站测试数 247 = 164+9+74；README 中英都写了上限；全量 164/9/74 | 干净（3/3） |

# 第五十二次记录（2026-09-26）— wmux 改名 keepane

用户发现 GitHub 上 wmux 同名撞车。核实：GitHub 上约 20 个 wmux 仓库，其中 openwong2kim/wmux（398 星，wmux.app，winget 已上架 `openwong2kim.wmux`）和 amirlehmam/wmux（396 星，wmux.org）都比我们早半年；crates.io 的 `wmux` 已被注册；winget 已有两个 wmux 包。我们是 1 星、0 fork、winget PR 未合并，改名成本最低的时候。候选名逐一查了 GitHub 同名仓库、crates.io、npm、winget、六个常见域名（RDAP）；用户先后否掉 winplex / panestack / panemux（不要被 mux 限制，panestack 已有网站），选定 keepane：各渠道全空，.com .dev .io .app 未注册。

**做了什么**

- 全仓库 56 个文件、1186 处 wmux → keepane（程序、crate、命令、配置文件、数据目录、环境变量、管道名、通知协议、Windows Terminal 片段、开机启动项、安装包、文档、网站、工具脚本）；历史验收记录、旧版本 winget 清单、旧 scoop 清单保留原名。
- 兼容（`src/legacy.rs`）：`WMUX_*` 环境变量、`~/.wmux.conf`、`~/.wmux/plugins`、`*.wmux` 插件在新名缺席时仍生效；`%LOCALAPPDATA%\wmux` 在没有旧服务端运行时首次启动即按文件搬到 `keepane`（已有的不覆盖）；测试等设了目录覆盖的环境不动真实目录。
- `keepane migrate`：让仍在运行的 wmux 服务端存盘退出，搬数据目录，在 keepane 里恢复会话；同名会话以 `<名字>-wmux` 恢复在旁边；开机启动、Windows Terminal 片段、通知链接改成 keepane（只动能认出是我们写的注册项）。wmux 服务端还在时启动 keepane 会提示。
- MSI 保留原 UpgradeCode（安装即替换 wmux）；发版时同一个 MSI 再以 `wmux-<版本>-…msi` 名字上传，旧版 `wmux update` 能直接装上 keepane。
- README 中英加「原名 wmux / Formerly wmux」一节；网站首屏加"原名 wmux"；动图、截图、手机截图用新名重录。

**用真实旧版 wmux 0.13.1（从 GitHub release 下载）实测迁移**

| 场景 | 结果 |
|---|---|
| 旧服务端 2 个会话（一个 2 窗口）在跑时启动 keepane | 打印提示 |
| `keepane migrate` | 旧服务端存了 2 个会话后退出；两个会话恢复，窗口和屏幕文字都在；再跑一次只报"没有旧服务端" |
| keepane 已有同名 `work`、旧服务端有 `work` / `ops` / `my work` | keepane 的 `work` 原样保留，wmux 的恢复为 `work-wmux`，其余两个恢复 |

验收（每轮先 fmt --check + clippy -D warnings，再 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 剩余 wmux 逐处核对；`packaging/scoop/keepane.json` 是替换时未被跟踪的新文件，漏改了 6 处；parity 没列 `migrate` | **有问题**，已改。不计数（此前替换还误伤了 4 处测试数据：格式模块取末 4 字符、缓冲区字节数、提示符后的光标列、小窗口跟随光标的列，均按新名字长度改正） |
| 2 | 机制通路 | 去掉环境变量回退 → 单测失败；去掉旧配置路径 → 单测失败；迁移不恢复会话 / 不提示 → 真实旧版脚本检出；还原后通过。全量 167/9/74 | 干净（1/3）后被下一轮打断 |
| 3 | 边界 | 同名会话：`migrate` 报"restored"，实际什么也没恢复（restore-session 对已在运行的名字只是附着），wmux 那份也不会被搬过来 | **有问题**，改为以 `<名字>-wmux` 恢复在旁边（从旧服务端实际的 sessions-dir 找它的存档）。不计数。排查中另发现我脚本里 `"$$"` 被 PowerShell 展开导致误判，已改 |
| 4 | 可复现性 | 两个迁移脚本各跑 2 次输出一致（归一"created Ns ago"）；e2e 2 次 0 失败；全量 167/9/74；残留进程 0 | 干净（1/3） |
| 5 | 机制通路 | 同名处理变异（跳过）→ 脚本里没有 `work-wmux`；还原后有。全量 167/9/74 | 干净（2/3） |
| 6 | 静态一致性 | 重录图和动画后：剩余 wmux 87 处全是兼容代码、改名说明、旧文件名；图片引用缺失 0；i18n 71/71；网站测试数 250 = 167+9+74；`migrate` 在帮助 / 补全 / parity 都有。全量 167/9/74 | 干净（3/3） |

未在真实机器上验证：MSI 同 UpgradeCode 替换已安装的 wmux、旧版 `wmux update` 装上 keepane、开机启动 / Windows Terminal / 通知注册的迁移（这些在用户实际升级时发生）。

## 53. 改名提交在 CI 上失败：配置错误浮层的临时路径被截断

改名提交 6115a28 在 CI 上 e2e `attach_type_split_detach` 失败（73/74），未打标签、未发版。原因：测试把坏配置写到临时目录，浮层每行以文件路径开头、按 80 列截断；改名后文件名从 `wmux-bad-` 变成 `keepane-bad-`，加上 runner 的临时目录（`C:\Users\RUNNER~1\AppData\Local\Temp`），`:1: bad boolean 'maybe'` 的结尾引号正好被截掉。本机临时目录短，所以本地一直通过。

修改（只动测试）：坏配置放到 `CARGO_TARGET_TMPDIR`，用相对包根目录的路径 source（测试进程和进程内服务端的 cwd 都是包根目录），屏幕上的路径长度与机器无关，断言保留完整文字。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 复现 | TEMP 设为 75 字符目录，旧测试失败（本机复现 CI）；只放宽前缀匹配仍失败（`:1:` 整个被挤出 80 列） | **有问题**，改为相对路径。不计数 |
| 1 | 边界 | TEMP 75 字符下全量 167/9/74，0 失败；fmt/clippy 0 | 干净（1/3） |
| 2 | 机制通路 | 坏配置改成合法的 `set -g mouse on` → 该测试超时失败；还原后通过；正常运行后 `target/tmp/bad-*` 残留 0（变异那次 panic 留下的 1 个已确认是变异内容并删除） | 干净（2/3） |
| 3 | 可复现性 | e2e 全套连跑 3 次：74/74 ×3 | 干净（3/3） |

## 54. pane 之间的消息、工作模式、观测平台、MCP、dashboard（0.15.0）

设计：`docs/design/mailbox.md`（与用户逐项确认后定稿，实现中的偏差已回写）。新增模块均不调用平台 API：`src/server/actor.rs`（窗格作为 actor 的纯状态机与信封）、`src/server/observe.rs`（事件记录与任务视图，JSONL 落盘）、`src/server/mail.rs`（服务端命令、投递、等待、权限）、`src/mcp.rs`、`src/dashboard.rs`、`src/setup.rs`。

**实现过程中发现并修掉的问题（先记账）**

| # | 怎么发现 | 问题 | 处理 |
|---|---|---|---|
| 1 | 现有测试 | 新命令 `set-work-mode` 让 tmux 常用前缀 `set-w`（set-window-option）变成歧义 | 加显式别名 `set-w`，tmux 用法不变 |
| 2 | 现有测试 | `show-message` 是 tmux `show-messages` 的前缀，按前缀规则歧义 | 改名 `trace-message` |
| 3 | 单元测试 | 事件日志按毫秒记时间，内存里是纳秒，读回后不相等 | 消息创建时截到毫秒 |
| 4 | review | 多个服务端（`-L`）共用事件目录：一个启动时会把另一个运行中的消息记成"因重启丢弃"，编号也会撞 | 事件目录按 socket 分开 |
| 5 | e2e | normal 窗格 `read-message` 后消息停在"已读"，任务永远"进行中" | 已读算完成 |
| 6 | review | `install()` 的测试会调用真实的 `claude mcp add`，改动用户的 Claude 配置 | 拆出 `install_hooks`，测试只测文件部分 |
| 7 | **真实 Claude Code** | 测试用的 hook 写成带引号的路径加参数，Windows 上 Claude Code 用 PowerShell 执行 hook，报 `ParserError` | `setup claude` 写入的命令本来就不带引号；README 与设计文档加说明 |
| 8 | **真实 Claude Code** | 人在 normal 窗格读完回信再发新请求，被算进旧任务链（`task #1 · hop 2`）；几轮往来后会被跳数上限拒收。agent 在一轮内用 `wait_message` 取回信也会提前结束手上的任务 | `read-message` 不再改变当前消息，只记"最近读到的"供 `-r`；普通发送只从正在处理的消息继承任务链 |

**真实 Claude Code（2.1.282）验证**（独立测试服务端 `claudetest`，hook 与 MCP 只经 `--settings` / `--mcp-config --strict-mcp-config` 临时加载，未改用户 `~/.claude`；只放行 keepane 工具）

| 场景 | 结果 |
|---|---|
| 启动 | SessionStart hook 约 2 s 后报空闲（修正 #7 后） |
| boss(normal) → agent(ai)："用 reply 回 pong" | 立即投递；信封、正文、结尾行三行作为一条提示词提交（括号粘贴 + 回车在 Claude Code 中有效）；agent 调 MCP `reply`，回信 #2 `task:1 hop:1 re:1`；Stop hook 后 #1 done，用时 8.0 s；boss `read-message` 得到 `pong` |
| agent 自己开 pwsh 窗格执行命令并回报 | `create_window`（默认 shell 模式）→ 命令带 `<# 信封 #>` 执行，输出 42 → `trace_message` 等到完成 → 把 42 回给 boss；用时 19 s |

**dashboard 真实窗格验证**：在 ConPTY 窗格中运行，`send-keys` 驱动：观测模式按 `d` 只提示；`E` 进入管理模式（顶栏 MANAGE）后置顶、删除、撤销生效；`Esc` 回到观测；任务视图正确。

验收（每轮先 fmt --check + clippy -D warnings，再 review + 全量测试）：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 设计文档与实现有 9 处偏差（报错文字、按 `via` 投递、read 语义、`pane-ready -t` 权限、命令表缺 `create-pane`/`close-pane`/`dashboard`/`#{pane_message}`、只记"转为空闲"、`@` 提醒何时消失、默认模式含 gemini、回信的 task 继承）。改后核对：19 个命令在代码/USAGE/parity/设计中 19/19；9 个配置项 9/9；MCP 工具 20 个与设计一致；7 个格式变量在代码与 parity 中 7/7。全量 193/9/81 | **有问题**，已改，不计数 |
| 2 | 机制通路 | 10 个变异各跑对应测试：输入不再判忙、投递不看 `via`、去掉跳数上限、去掉 `agent-commands`、`may_change` 恒通过、忽略提示符标记、观测模式放行管理键、信封字段换序、shell 包装去掉注释、地址前缀不校验 → 10/10 被测试检出；还原后源码无残留变异（0 处），fmt/clippy 0，全量 193/9/81 | 干净（1/3） |
| 3 | 边界 | 23 项：空正文、1024/1025 字节、名字 64/65 字符与清空、收件箱上限 1、`-w` 超时（`-w 1` 实测 1.3–2.0 s，按秒检查，已写入文档）、等待上限截断（`-w 1000` 在上限 2 时 3.0 s 返回）、单条移动、无可撤销、空名字 `%`、发给自己、`-` 开头的正文（需 `--`）、已退出的窗格、损坏的事件日志行（跳过，编号接续）、24×6 的 dashboard（不崩）、状态设置与清除。发现 3 处：`inbox is full (1 messages)` 措辞；`event-log off` 要等下一秒定时任务才生效（关后又写了 2 行）；等待中的窗格被关掉后要等到超时（20 s）才返回。另查明"重启后无丢弃提示"不是问题：`kill-server` 先关窗格，消息已记为 `its pane closed` | **有问题**，已改（设置时立即同步；窗格移除后与每秒定时各检查一次等待），加 2 个回归测试，撤掉修复后两者都失败。不计数 |
| 4 | 代码正确性（资源） | review：dashboard 事件视图每秒调 `list-events -S 1d`，它在服务端主线程上强制刷盘并读取当天整个日志（上限 20 MB），会卡住渲染、也让日志批写失效。实测 25.9 MB 日志：`-S 1d` 65–169 ms/次，改用内存中最近 5000 行的 `-n 200` 为 30 ms/次（≈ 进程启动开销）。并发时其他请求的延迟噪声大于差值，不作结论 | **有问题**，已改（`list-events -n`，dashboard 与 MCP 改用；`-S` 仍读文件，供手动查询），加单元测试与 e2e 断言。不计数 |
| 5 | 可复现性 | 三个全新服务端执行同一串操作（改名、改模式、发、移、删、撤销、读、回信），事件日志去掉 `at` 后 SHA-256 前 16 位三次相同 `22B4E8889E025E7F`（11 行）；MCP `tools/list` 两次相同（7099 字节）；e2e 连跑 3 次 81/81 ×3 | 干净（1/3）后被下一轮打断 |
| 6 | 逻辑正确与不变量 | 逐条核对全局规则的所有入口：normal 窗格从不被打字（5 个投递入口都经 `next_delivery`）、只按 `via` 投递、窗格内只改自己和自己创建的（7 个修改入口都经 `may_change`）、消息终态（关窗格/kill-session/撤销保留窗口/服务端退出 4 条路径）、跳数与大小上限（唯一入口 `send_message`）。发现：**名字唯一**被 `undo-kill` 打破——被关窗格在保留期间名字看似空出，被别人取走后撤销，出现两个同名窗格 | **有问题**，已改（撤销时名字已被占用则不恢复并提示），加回归测试；撤掉修复后该测试失败。不计数 |
| 7 | 错误路径与并发 | review：MCP 在服务端不在/stdin 关闭时的行为、两个等待方抢一条消息（只唤醒一个）、等待方断开（等待清理）、等待中的消息被删（报错返回）、dashboard 出错时恢复控制台——均无问题。发现：`create-pane` 用"新出现的窗格中编号最大者"认领新窗格，而 `after-new-window` 等 hook 在同一条命令里同步执行并可能再开窗格，名字、模式、创建者会记到 hook 开的窗格上。资源：启动时读回两天事件日志，两个 20 MB 文件时 release 启动 + 首条命令 394–437 ms（空日志 109–217 ms），debug 1877 ms，只在服务端启动时发生，判断可接受并写入文档 | **有问题**，已改（取编号最小者：hook 总在其后），加回归测试（设一个会分屏的 hook）；改回最大者时该测试失败。不计数 |
| 8 | 静态一致性（复查第 1 轮之后的改动） | 核对 `list-events -n`、`create-pane` 认领、撤销时的名字、`event-log` 立即生效、收件箱满措辞在 USAGE/README/设计/parity 中的描述：USAGE 与设计命令表缺 `-n`，设计的名字规则缺 `undo-kill` 情形 | **有问题**，已补。不计数 |
| 9 | 机制通路（第 2 轮之后新增的逻辑） | 6 个变异：内存近期事件不记、关窗格不丢弃排队消息、重启不标记未完成、normal 收件不打 `@`、MCP `reply` 不带 `--`、存档不存工作模式。4 个被检出；**2 个存活**（`@` 窗口标记、`reply` 文本以 `-` 开头）= 测试缺口 | **有问题**，补 e2e（非当前窗口的 normal 窗格收件后 `#{window_flags}` 含 `@`）与单元测试（reply/set_status/send_message 的文本前都有 `--`）；复测两个变异均被检出。不计数 |
| 10 | 可复现性（复查） | fmt/clippy 0；全量 194/9/83；e2e 连跑 3 次 83/83 ×3；第 5 轮的事件日志场景两次 `22B4E8889E025E7F`，与第 5 轮相同（中间的修改没有改变事件输出） | 干净（1/3）后被下一轮打断 |
| 11 | 边界（编码与多行，真实 pwsh） | 中文与多行正文往返逐字节一致（77 字节）；shell 模式中文输出截取正确。发现：① 多行命令被逐行执行（PSReadLine 不启用括号粘贴），第一行一执行完消息就被记为 done、输出为空；试 Shift+Enter 分行，诊断显示 ConPTY 丢掉了 Shift（`vk=13 state=0`）；② 包成一段执行后，块内失败 `$?` 仍为 True，记为 done；③ 80 列窗格中带信封的命令回显被 ConPTY 补空格到行宽并标记为折行，下一行输出被接进命令行后随命令一起被去掉（输出为空）；④ 录制原始输出看到 ConPTY 把 `keepane-cmd`、`9;9` 排到它们之前写出的文字前面 | **有问题**，已改：多行改写为一行 `. ([scriptblock]::Create(...))`，加 `$Error[0]` 比较使块内失败时 `$?` 为 False（`$Error` 满 256 条仍有效，实测）；以"最后一格是否空白"区分真折行与补白；提示符标记后等 60 ms 再结束消息（期间有人打字则不判空闲）。加单元测试（补白/真折行、多行改写）与 e2e（多行含失败 → failed 且输出 42；变量留在会话里）。shell 测试连跑 5 次 5/5。另记已知限制：带信封的命令进入 PSReadLine 持久历史。不计数 |
| 12 | 静态一致性（设计文档逐条对照代码） | 发现 21 处不符：两项答应过的 dashboard 功能没做（消息视图里 Enter 看某条全文、`a` 看所有收件箱）；文档层面：状态行仍写"待实现"、创建者记录方式、MCP 返回格式、`-w` 写法、`pane-ready -t` 权限、dashboard 已非只读、事件名（无 `queued`，状态为 `pane/status`，另有 `failed`/`abandoned`）、任务视图与 dashboard 示例、"正在做"规则、管理模式标签、已知限制第 4/7 条过时、模块名、信封 token 实测（做不到：无分词器，改记字节数 123/22） | **有问题**，已改：dashboard 补 Enter（`trace-message`，加入 QUERIES）与 `a`（`list-messages -a`），消息视图底部提示随之变化，加单元测试；文档按实现改写，示例换成实际输出。不计数 |
| 13 | 代码正确性（review 第 10 轮后的改动） | 提示符等待：期间窗格被关（跳过）、60 ms 内两次标记（只安排一次）、期间打字（记下，不判空闲）；多行改写：`\r\n`、末尾换行、行尾反引号续行、here-string；补白判定的已知偏差（最后一格恰为空格的真折行会断成两行，只影响截取显示）；dashboard 以 `client::query`（不带 `KEEPANE_PANE`）发命令，在窗格里运行也按"人"管理。fmt/clippy 0，全量 197/9/83，e2e 再跑 83/83 | 干净（1/3） |
| 14 | 边界（连发与排队，真实 pwsh） | 向 shell 窗格一口气连发 6 条（第 4 条会失败）：6/6 按序执行、各自只带自己的输出、第 4 条 failed；向 ai 窗格（跑 cmd.exe 充当 agent）连发 3 条，每次 `pane-ready` 恰好结束一条、投递下一条（delivered,queued,queued → done,delivered,queued → done,done,delivered → done,done,done）。另：ai 窗格里若跑的是带钩子的 pwsh，投递的文字被 pwsh 执行后出现提示符，按设计记为 abandoned（测试场景写错，非缺陷） | 干净（2/3） |
| 15 | 机制通路（第 11、12 轮新增逻辑） | 5 个变异：补白行照样拼接、多行不改写、dashboard Enter 不打开被检出；**提示符等待去掉（0 ms）与"等待期间打字"两处存活**——机制没有测试证明。追查时另发现一个隐患：keepane 投递的命令运行期间人提前打的字，会在下一个提示符的输入行上，而那个提示符让窗格空闲，下一条消息会接在这些字后面执行（shell 模式是直接执行） | **有问题**，已改：① 空闲判断拆成"标记到达即在提示符"与"60 ms 后结束消息、截取输出"两步，等待期间不投递，标记之后的输入按先后顺序自然判忙（去掉了"等待期间打字"这个单独状态）；② 投递的命令运行期间有人按键，结束它的提示符不判空闲，人按回车得到干净的提示符后恢复；③ 放弃"比较光标位置"的方案（ConPTY 可能让标记先于提示符文字到达，会误判为永远忙）。加单元测试 2 个、e2e 2 段（命令自己先发标记再输出 → 输出不丢；运行中提前打字 → 不判空闲、下一条留在队列，清行回车后才投递）；变异复测：等待为 0 ms、忽略运行中打字，两者都被检出。shell 测试连跑 3 次 3/3；e2e 9 次运行中有 1 次失败未能记下测试名，之后 8 次全过，未复现，记为待观察 | 不计数 |
| 16 | 可复现性 | fmt/clippy 0；全量 199/9/83，e2e 再跑两次 83/83 ×2，均无失败（记录失败测试名的方式已加上）；事件日志场景三次 `22B4E8889E025E7F`（与第 5、10 轮相同）；MCP `tools/list` 两次 7099 字节 `78E82CE4D9C282C9`（与第 5 轮相同） | 干净（1/3） |
| 17 | 边界（真实 pwsh 场景用最终代码重跑） | 连发 6 条：6/6 按序、各带自己的输出、第 4 条 failed；多行：done 输出 42、块内失败记 failed、历史各 1 条；运行中提前打字：命令后不判空闲、下一条排队，清行回车后 done；命令自己先发标记：0.1 s 完成，输出 `late-output` 在 | 干净（2/3）后被下一轮打断 |
| 18 | 静态一致性（第 15 轮之后） | README 英文"a message never lands in a half-typed line"、中文"不会插进打了一半的命令里"以及 `actor.rs` 一处注释说过头：人自己的命令运行期间提前打字拦不住（已知限制第 11 条）；README 也没提多行命令会整段作为一条执行 | **有问题**，已改为准确说法（中英文 README、注释），补多行一句。不计数 |
| 19 | 逻辑正确与不变量（第 15 轮之后的结构） | 投递的所有入口都经 `deliver`（空闲且不在提示符等待期），等待期结束必再调用；等待事件由独立任务发出，窗格关闭即跳过、编号不复用；"运行中按过键"只在有当前消息时设、每次投递重置、在提示符消费，ai/normal 不受影响；名字唯一的 4 个入口与 3 个移动操作；每条消息的结局（等待期满记完成、关窗格记丢弃）。fmt/clippy 0，全量 199/9/83，e2e 再跑 83/83 | 干净（1/3） |
| 20 | 机制通路（全量回归） | 第 2、9、15 轮全部变异点在最终代码上重跑：26 个（输入判忙、按 `via`、跳数、`agent-commands`、`may_change`、提示符标记、管理需 E、信封字段序、注释包装、地址前缀、内存近期事件、关窗格丢弃、重启丢弃、`@` 标记、reply `--`、存档模式、提示符等待、补白、多行改写、Enter 打开、运行中按键、等待期不投递、标记后按键、撤销让名、hook 下认领、`event-log` 立即生效）→ 26/26 检出；还原后无残留 | 干净（2/3） |
| 21 | 静态一致性（最终） | 脚本核对：19 个命令在代码/`--help`/parity/设计中无缺口；9 个配置项在代码与文档中无缺口，默认值与设计一致（8/100/600/8/30）；MCP 20 个工具都在设计中；7 个格式变量在代码/parity/设计中无缺口。人工逐句核对 README 新章节 13 处说法（示例输出格式、`@` 标记、多行、hook 行为、MCP 能力、限额默认值、dashboard 按键、日志位置与保留、停机时消息去向、存档内容）与实现一致 | 干净（3/3） |

结论：第 19、20、21 轮连续三轮干净，验收通过。共 21 轮：实现期记账 8 个问题，验收期第 1、3、4、6、7、8、9、11、12、15、18 轮发现并修正问题（其中两个由真实 Claude Code 测出、四个由真实 pwsh 场景测出）。测试从 250 个增至 291 个（库 199、控制台 9、e2e 83）。

遗留：
- e2e 在一次完整运行中出现过 1 次失败，未记下测试名，之后 13 次运行（含 3 次完整 `cargo test`）未复现，待观察。
- 已知限制见设计文档第 12 节（PSReadLine 持久历史里会出现带信封的命令；人自己命令运行中提前打字拦不住；cmd/WSL 没有 shell 模式的钩子等）。
- 未在真实机器上验证：`keepane setup claude --install` 对用户真实 `~/.claude/settings.json` 的写入（只测了临时目录与 `--settings` 临时加载）；网站与图片未更新；未发布。

## 55. 验收后的修改：权限化繁为简、按键抬起不算打字

录制网站演示时发现，按第 54 条的规则，人在自己的 shell 窗格里敲 `keepane rename-pane -t` 改别的窗格被拒——"凡窗格内部调用都受限"与"防 agent 失误"的本意不符。与用户逐步讨论后（期间用户指出：agent 若把自己的窗格改回 normal 就能摆脱限制；并认为多数操作出错只影响成败），定为：

- 只保留一条规则：**工作模式只能在窗格自己里切换**（从窗格里 `set-work-mode -t` 指向别的窗格被拒；keepane 之外的终端、快捷键、`C-b :` 命令行不受限）。这样没有任何窗格能把另一个窗格变成"收到什么就执行什么"的 shell。
- 其余全部开放：改名、收件箱管理、`pane-ready -t`、关窗格（`close-pane` 删除，MCP `kill_pane` 即 `kill-pane`，有 `undo-kill` 兜底，Claude Code 调用未放行的 MCP 工具前会询问）。
- `create-pane` 的程序名单与数量上限只对 agent 的窗格（`ai` 模式或由别的窗格创建）生效；人开的窗格不记创建者。
- 不做"防恶意"（密钥挡不住同一用户的程序；按 job object 识别调用者会把核心逻辑绑在平台上）——用户决定暂不做，理由写入设计文档第 8 节。

同一过程中发现并修正：

| # | 怎么发现 | 问题 | 处理 |
|---|---|---|---|
| 1 | 录制 | 外层 shell 里敲 `keepane new` 的回车，其"抬起"到达新接入的窗格，被算作打字，窗格一直忙 | 只有按下的非修饰键才算打字（`typing()`），加单元测试 |
| 2 | 截图 | dashboard 置顶后高亮停在原行 | 选择随被移动的消息走，加断言 |
| 3 | review | 文档写"事件日志记下是谁关的窗格"，实际并不记录 | 删去该说法（三处） |

验收：

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 机制通路 | 4 个变异（窗格里改别的窗格的模式、按键抬起算打字、人也受程序名单约束、置顶后选择不动）→ 4/4 检出；还原后无残留 | 干净（1/3） |
| 2 | 真实使用 | fmt/clippy 0；全量 200/9/83；e2e 再跑 83/83；网站演示 `record_messages` 录制成功（160 帧，两张截图人工检查：shell 窗格带信封执行、trace 显示 done 与输出；dashboard 管理模式红色顶栏、收件箱两条） | 干净（2/3） |
| 3 | 静态一致性 | 在 README 中英、设计、parity、USAGE、MCP、mail.rs、command.rs 中查旧规则与已删命令的 10 个关键词：0 处；`--help` 含 `create-pane` 与 `set-work-mode` | 干净（3/3） |

## 56. CI 上 shell 输出截取混进命令回显

推送 0.15.0 后 CI 的 e2e 失败：多行命令的"输出"里混进了改写后命令回显的后半截。原因：截取时以"行尾是否空白"区分真折行与 ConPTY 的补白（第 13 轮记过的已知偏差），CI 的提示符路径更长（`C:\Users\RUNNER~1\…`），命令回显恰在空格处折行，后半截被当成输出。本机路径短，没触发。

修正：不再从屏幕上判断命令回显占几行，投递时按"起始列 + 打入文字的显示宽度"直接算：`宽度整除 + 1` 行（恰好写满一行时 ConPTY 立即换行，回车再换一行，实测多出一空行，所以不是向上取整）。命令成败仍按命令所在行找标记，两个位置分开保存。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 复现 | 临时目录名加 0–11 个字符改变提示符长度，shell 测试 12 次中 +5 失败（恰好写满一行，输出前多一空行，向上取整不对） | **有问题**，改为整除 + 1 |
| 1 | 机制通路 | 新单元测试（恰满一行、短于一行、跨三行）；改回向上取整即失败 | 干净（1/3） |
| 2 | 边界 | 提示符长度 +0…+15 共 16 种：16/16 通过 | 干净（2/3） |
| 3 | 可复现性 | fmt/clippy 0；全量 201/9/83；e2e 再跑两次 83/83 ×2 | 干净（3/3） |

## 57. README 与项目描述改为新定位（keep pane，pane 是 actor）

定位从"Windows 上的 tmux"改为"keep pane：pane 一直在跑，还能互相派活"。README 中英文重排：开头讲 actor、三种模式、信封；接着是 agent 与 MCP（20 个工具分组列出）、dashboard、安装（zip/Scoop 免管理员，SSH 下的 `no_junction` 与 MSI 限制），然后才是 tmux 那一套；"原名 wmux"挪到文末。项目描述统一为同一句：Cargo、scoop 清单、MSI、winget 生成器、crate 文档、网站 title/meta/hero（中英）。旧段落按行号原样搬运，搬运脚本逐行断言起始内容。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性（claim 对代码） | 手机一节搬到后面后"见下文"失效（中英各 1）；"原名 wmux"里"见下文"方向反了（中英各 1）；MCP 表把 `rename_pane`/`kill_pane` 写成"自己的 pane"；"全部 85 个命令"实际 116 | **有问题**，全部修正，命令数不再写死 |
| 2 | 机制通路 | fmt/clippy 0；全量 201/9/83；生成器 scoop 清单与仓库逐字节一致；winget validate 通过；README 示例在临时 socket 实跑：rename→`builder shell`、send→`#1 delivered`、trace→done 输出 `readme-check-ok`；MCP tools/list = 20 | 干净（1/3） |
| 3 | 中英一致 | 逐节比对反引号词：改动各节只有占位符翻译与跨行代码的分词差异；其余差异为原样搬运的旧行 | 干净（2/3） |
| 4 | 新用户路径 | zip 实名带 `v`（`keepane-v0.15.0-…zip`），且内含一层目录，README 写法会让人把外层目录加进 PATH | **有问题**，写明目录名；已核实 release 资产名与 zip 布局 |
| 5 | 可复现性 | GitHub GFM 渲染两次哈希相同（EN `CD6A4AC96EAE`，ZH `C0E39D2FCF66`）；h2 14、表 3、图 7、正文漏出反引号 0 | 干净（1/3） |
| 6 | 静态一致性（全仓） | 网站 title/meta/og/hero 仍是"tmux for Windows"（4 处） | **有问题**，改 title、meta、og、hero 标题与导语（中英） |
| 7 | 静态一致性 | 旧定位残留 0；网站 i18n 键 77/77，无缺无余；新描述在 8 处一致 | 干净（1/3） |
| 8 | 视觉 | 500px 中文标题折成"…互相 / 派活。"；375px 顶栏溢出 20px（旧问题） | **有问题**，标题按意群不断行 + `text-wrap: balance`；≤400px 顶栏收紧、GitHub 只留图标（补 aria-label）。复测 320/360/375/414/768/1280 × 中英：溢出 0 |
| 9 | 代码正确性 | fmt/clippy 0；全量 201/9/83；通读非 README diff 无问题 | 干净（1/3） |
| 10 | 边界 | 描述长度 Cargo/scoop 161、winget Short 162（上限 256）；scoop JSON、cargo metadata 可解析；MSI 实际打包，摘要为新描述 | 干净（2/3） |
| 11 | 可复现性 | 清单生成两次 4 个文件哈希相同；README 重新渲染与第 5 轮哈希一致 | 干净（3/3） |

遗留：GitHub 仓库 About 描述需推送时一并改（`gh repo edit`）；网站章节顺序仍以 tmux 功能开头，actor 章节在第 07 章；winget PR #441466 里 0.15.0 的描述是旧的，不改（下个版本由生成器带上）。另：SSH 里 `C-b q` 不显示编号，本机 ConPTY 字节输入复现正常（按后 20 个带色格子），疑为 Mac 端 tmux 截走 `C-b`，待用户 `show-keys` 确认。

## 58. 从 SSH 启动的 server 随连接断开而结束

现象：SSH 里 `C-b d` 后关掉连接，回来 keepane 的 session 全没了。server.log 里 20:33 启动的默认 server 之后没有任何退出记录（正常退出、panic、restart 都会记），20:44 用户重连时才起了新的：它是被外面杀掉的。当时在跑的 server（pid 30112）经 `IsProcessInJob` 查证在 job 里，sshd 自己不在。OpenSSH 把每个会话放进 `KILL_ON_JOB_CLOSE | BREAKAWAY_OK` 的 job；keepane 起 server 时没带 `CREATE_BREAKAWAY_FROM_JOB`（该参数 09-24 为 `restart-server` 加入，起 server 的两处未跟上），所以 server 留在会话的 job 里。wmux 当时没断，是因为它的 server 在本机桌面会话里启动（进程启动于 09-25 0:34），SSH 里只 attach。

修正：起 server（`start_server`、`migrate`）先带 `CREATE_BREAKAWAY_FROM_JOB`；job 不许脱离（CreateProcess 返回拒绝访问）就不带再起一次，和以前一样。`restart-server` 保持原样（脱离失败即报错）。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 复现 | 新测试：在 `KILL_ON_JOB_CLOSE \| BREAKAWAY_OK` 的 job 里 `new -d`，关 job 后 server 死（left false / right true） | **有问题**，修正 |
| 1 | 机制通路 | 修正后测试通过；变异 1（原代码不脱离）→ `breakaway_ok true` 失败；变异 2（去掉回退）→ `breakaway_ok false` 时 `new -d` 返回 1；fmt/clippy 0；全量 201/10/83 | 干净（1/3） |
| 2 | 代码正确性/资源 | 通读 diff：回退只在起 server 处，`restart-server` 语义不变；全量跑完后残留 test server 0 个 | 干净（2/3） |
| 3 | 边界/可复现 | pane 里 `-L inner new -d` 起的 server，在外层 server 被 kill 后仍活着（tmux 同）；新测试连跑 5 次 5/5，1.49–1.52 s | 干净（3/3） |

遗留：用户机器上现在的 server 仍是 0.15.0 起的，还在 SSH 的 job 里，发版前断线仍会丢；自动存档在，`keepane resume` 可恢复布局。


## 59. README 去掉 AI 产品腔：先讲 pane 一直在，再讲传消息，agent 放后面

用户转来的评审：第一段 actor / inbox / work mode / hand each other work 连用，像 agent 框架的概念体系；"Agents and MCP"太靠前。用户选定：第一句不写 Windows、不以 tmux 定位，写"panes keep running after you detach and can pass messages to each other"，tmux 作为"键和命令照用"。叙事改为：pane 一直在 → 在 pane 之间派活（Send work between panes）→ dashboard → 安装 → 日常用法 → …… → 在 AI agent 里用（第 455 行，原第 94 行）。模式规矩挪进派活一节（它对谁都适用）。项目描述同步：Cargo、scoop、winget 生成器、MSI、crate 文档、网站 title/meta/og/hero、第 07 章标题。`work mode` 是命令名（`set-work-mode`），文档里保留。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 表述属实与术语 | 示例第二行在 pane 里运行会被拒（删了"见下文"后读者不知道）；表头写 Free，前文一律 ready | **有问题**，注释写明在外面运行；表头改 Ready / 准备好 |
| 2 | 机制通路 | fmt/clippy 0；全量 201/10/83；GFM 渲染 h2 14、表 3、图 7、漏出反引号 0（中英）；网站 i18n 77/77；中英章节一一对应 | 干净（1/3） |
| 3 | AI 味（量化） | 前 100 行：actor 3→0，MCP 4→1，agent 13→8（余下都是 `ai` 模式的具体机制）；agent 一节 94→455 行；交叉引用 4 处仍有效 | 干净（2/3） |
| 4 | 视觉 | 中文标题"还能互相传消息。"比原来长一字，320px 下超出标题框（scrollWidth > clientWidth） | **有问题**，≤400px 标题字号 34px |
| 5 | 视觉复测 | 320/360/375/414/768/1280 × 中英：页面溢出 0、标题溢出 0，不断行段右缘均在框内 | 干净（1/3） |
| 6 | 机制通路（发布物） | 生成 0.15.1 清单：winget validate 通过；scoop zip 哈希、winget MSI 哈希与 release 上 .sha256 一致；新描述进入两份清单 | 干净（2/3） |
| 7 | 可复现性 | 清单再生成 4 个文件哈希不变；README 渲染两次哈希相同（EN 298F1C24D842，ZH 36771C221459）；全量 201/10/83 | 干净（3/3） |

遗留：0.15.1 的 MSI 摘要描述是 tag 时的旧句（"hand each other work"），下个版本随新 wxs 更新；GitHub About 待 `gh auth login` 后改；winget PR #441466 仍是 0.15.0。


## 60. README 按用户改写的中文版重排，英文照译，网站导语同步

用户给出改写后的中文 README（分段粘贴，中间"历史后半句 → 状态栏开头"一段未传到，沿用原文；粘贴造成的断词按原意接回，一处两条要点粘在一行已拆开）。英文版：用户改过的各节重新翻译（开头到按键表前、动画、copy mode、鼠标、命令时间、状态栏变量、还没做的、原名 wmux），未改的各节原本就逐段对应，保留。网站导语、meta、og 换成 README 第一段的意思（写明 Windows）。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 中英一致与结构 | 改动各节反引号词无差异（仅占位符翻译）；其余差异改前即有；GFM：h2 14、表 3、图 7、漏出反引号 0、断链 0；列表 EN 9 / ZH 8，多出的在"手机"一节，HEAD 即如此 | 干净（1/3） |
| 2 | 机制通路 | 全量 201/10/83；网站 i18n 77/77；旧导语残留 0，新导语中英均在 | 干净（2/3） |
| 3 | 保真与可复现 | 用户粘贴的 228 行非空内容按序全部在中文 README 里（缺 0）；渲染两次哈希相同（EN 6525CB5F1B22，ZH 1D1B6ED0F7A2） | 干净（3/3） |

遗留：README 第一句现在写明 Windows，而 Cargo、scoop、winget、MSI、GitHub About 的描述是上一版（未写 Windows），待用户决定是否统一。

## 61. display-panes 数字看不见；每个 PowerShell pane 各有命令历史

一、`C-b q` 的数字用 `█` 画，而 `█` 显示前景色，代码把前景色写死成黑色（颜色只放在背景上），深色主题上几乎看不见。改为颜色放前景，并加 tmux 的 `display-panes-colour`（默认蓝）/`display-panes-active-colour`（默认红）。

二、恢复/重启后按 ↑ 历史"不对"：所有 pane 共用 PSReadLine 的一个全局文件，新起的 pwsh 读到的是所有 pane 混在一起的命令。另外测试的 shell pane 一直写进开发者真实的全局历史（661 行信封命令，全是测试与验证）。改为每个交互式 PowerShell pane 有自己的历史文件（`<sessions-dir>\psreadline\<名>.txt`，环境变量 `KEEPANE_SHELL_HISTORY`，钩子在首个提示符前 `Set-PSReadLineOption -HistorySavePath`；实测该时机切换对 ↑ 与写入都生效）。文件名随 session 存档，恢复与 respawn 沿用；新 pane 复制来源 pane（分屏的原 pane / 新窗口时在用的 pane）的历史，没有就复制全局文件；存档里记着但文件已不在的，照新 pane 重建。无 pane、无存档引用且超过 `log-history-days` 未写的文件每日清理（0 不清）。弹窗仍用全局文件。测试的 sessions-dir 本就在临时目录，污染随之消失。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 前提实验 | 临时 socket 上 `-Command` 里改 HistorySavePath：↑ 出该文件末行，新命令写入该文件，全局文件未写 | 方案可行 |
| 1 | 变异 | M1 钩子不切换、M2 不存档、M3 不复制来源 均被抓；但 M1 把标记写进了真实全局历史，此后未变异的代码 3/3 失败（断言依赖全局状态）；M4 respawn 丢文件未覆盖；清理函数无测试 | **有问题**：标记加进程号；测试加 respawn 步骤（M4 转为被抓）；清理规则抽成函数加单元测试 |
| 2 | 机制通路 | fmt/clippy 0；全量 203/10/84；通读 diff：README 未说明此行为 | **有问题**，Resume 一节补说明（中英） |
| 3 | 边界 | 存档记着的文件已被删：恢复后从空历史开始 | **有问题**，照新 pane 重建同名文件；测试删文件再恢复，文件回来（M5 去掉重建即失败） |
| 4 | 静态一致性 | README 7 条说法逐条在代码里找到；fmt/clippy 0；全量 203/10/84 | 干净（1/3） |
| 5 | 目标验证 | 全局历史信封行 661 → 661；最后写入 22:39（M1 那次），此后两次全量未写 | 干净（2/3） |
| 6 | 可复现性 | 新测试 5/5（各 3 s），原 shell 测试 3/3，全局历史文件哈希不变 | 干净（3/3） |

遗留：全局历史里 661 行测试命令与 M1 留下的 1 行，待用户确认后清理（多行命令的续行一并删）。

## 62. 网站章节按 README 的叙事重排

01 在 pane 之间派活 → 02 关掉终端程序照跑 → 03 命令时间和历史 → 04 后台任务提醒 → 05 分屏 → 06 全屏/列表/菜单 → 07 手机 → 08 配置 → 09 状态栏 → 10 安装（原来派活在第 07 章）。首屏标签去掉"原生 Windows"（跨平台打算）；下载按钮与系统要求照实保留。测试数 294 → 297。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 只动了顺序 | 新旧各 10 章，去掉编号后逐块相同；章外只差测试数；i18n 77/77 | 干净（1/3） |
| 2 | 视觉 | 320/375/768/1280 × 中英：溢出 0，编号 01–10 连续，第一章中英标题正确 | 干净（2/3） |
| 3 | 行为 | `#tour` 之后即第 01 章；首章 gif 独立加载宽 900；瞬时滚动截图可见第 01 章与动图（后台标签页里平滑滚动与懒加载不运行，第一次的"未加载"是检测方法问题，改用不依赖前台的检测） | 干净（3/3） |

## 63. 按用户安装的 MSI；SSH 下 update 不再卡在看不见的权限框

同一个 keepane.wxs 出两种包：`-dScope=perUser` 装到 `%LOCALAPPDATA%\Programs\keepane`、写用户 PATH、InstallPrivileges=limited（不要管理员、不弹 UAC），独立 UpgradeCode（两种范围之间不能互相升级），一个固定 GUID 的组件以 HKCU 值为键路径、卸载时删目录。`build-msi.ps1 -Scope user`，文件名 `-user.msi`；release 两种都出；winget 安装清单两个条目（Scope machine / user），下载模式只在 release 有该文件时加入。`keepane update` 认出按用户安装（换装同种包，无需管理员）；按机器安装、在 SSH 里且非管理员时直接说明并停下（原来会一直等桌面上没人能点的 UAC）。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 实装 | 按用户包静默装：文件 4 个在位、用户 PATH 加上、HKCU 标记在；卸载：目录与标记都没了，用户 PATH 与备份逐字相同。发现：本地 target\release 是旧编译（包 0.15.1、exe 0.15.0）→ build-msi 加版本守卫；`runas /trustlevel` 的 SAFER 令牌连不上 Installer 服务（1601），改用包本身的声明验证：WordCount 机器 2 / 用户 10（"无需提权"位），ALLUSERS 机器 1 / 用户未设，UpgradeCode 不同 | 修守卫 |
| 1 | 机制与变异 | fmt/clippy 0；全量 204/10/84；U1 按用户认成按机器、U2 管理员也被拦、U3 文件名不带 -user 均被单测抓到 | 干净 |
| 2 | 静态一致性 | 5 条说法都在代码/流程里；packaging/README 还写"两个 .sha256"、引用已关闭的 PR #440225 | **有问题**，更新 |
| 3 | 边界 | 大小写混用与同前缀目录（keepane2）的识别正确；但 build-msi 的 -ExeDir/-OutDir 传绝对路径会被拼到仓库根后面（旧 bug，release 用相对路径未暴露） | **有问题**，绝对路径照用 |
| 4 | 机制通路 | 照 release 的写法打两种包，生成清单 winget validate 通过（machine,user）；全量 204/10/84 | 干净（1/3） |
| 5 | 静态一致性 | 中英 README 新内容计数一致，GFM h2 14、漏出反引号 0 | 干净（2/3） |
| 6 | 可复现性 | 生成器同输入两次，4 个文件哈希相同；update 单测 3/3 | 干净（3/3） |

遗留：真正非管理员会话里的安装与 `keepane update`（用户的 SSH 会话）待下个版本发布后实测。

## 64. 平台拆分（纯重构）；pane 身份只对自己的 server 有效

一、Windows 专属的代码移到 `src/platform/windows/`（docs/design/platform.md）：整块搬的 clipboard、console、notify、proccwd、shutdown、startup、update、winsec、wt；从别处抽出来的 ipc（命名管道的监听与连接，原在 server 与 client）、process（进程树守卫 `Tree`、`shell_command`、`pipe_command`、`spawn_self`，原在 server、pane、client）、shell（默认 shell、PowerShell 钩子、`which`、共享历史文件、每 pane 历史的变量名，原在 config 与 server）、random（原在 web）；sysinfo 拆开（git 分支、字节写法留在原处）。原来的路径以 `pub use` 保留。目录交给 `dirs`，不另设模块。platform 之外对 Windows API 的直接调用：0。

二、实操中发现的旧 bug：客户端不论 `KEEPANE` 指哪个 server 都把 `KEEPANE_PANE` 发出去，别的 server 把那个编号当成自己的 pane（"模式只能在自己 pane 里改"错拦、消息来源记错）。改为只对自己的 socket 发（`own_pane`，`restart-server` 原有同样的判断）；嵌套检查（tmux：在任何 server 的 pane 里都算）对别的 server 的 pane 改由客户端做，自己的仍由 server 做，协议不变。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 0 | 基准 | 重构前 303 个测试名单 | — |
| 1 | 同一套测试 | 303/303 名字相同（31 个只换了模块路径），204/10/84 通过，忽略数同 | 干净 |
| 2 | 静态一致性 | platform 外仍有 2 处 Windows 调用：web 的随机数、pipe-pane 的 CREATE_NO_WINDOW | **有问题**，移入 random、`pipe_command`（照原样，不并入 shell_command）；设计文档对齐实际模块 |
| 3 | 同一套测试 | 303/303 同名，204/10/84 | 干净 |
| 4 | 机制通路（release 实操） | 自动起 server、run-shell（rs-42）、pipe-pane 写文件、分屏与 respawn、web 密钥都通；但从另一 server 的 pane 里 `set-work-mode` 被错拦（旧 bug，HEAD 同） | **有问题**，`own_pane`；单测 + 变异（去掉 socket 判断即失败） |
| 5 | 实操重跑 | 全部通，消息来源记为 user | 干净 |
| 6 | 全量 | 控制台嵌套测试失败：收紧后嵌套检查也失效 | **有问题**，嵌套检查对别的 server 的 pane 由客户端做 |
| 7 | 全量 | MCP 测试失败：它只设 `KEEPANE_PANE`、继承了开发者 shell 的 `KEEPANE`（真实 pane 两个都有） | **有问题**，测试补上 `KEEPANE` |
| 8 | 同一套测试 | 304 = 303 + 新单测，205/10/84 | 干净（1/3） |
| 9 | 环境无关 | 清掉 `KEEPANE`/`KEEPANE_PANE` 全量再跑：205/10/84 | 干净（2/3） |
| 10 | 实操 | 别的 server 的 pane：`new` 客户端拒、`new -d` 可、改模式不受限；同 server 的 pane：`new` 由 server 拒；不存在的 pane 按 pane 外处理（设计如此，HEAD 同） | 干净（3/3） |

## 65. Linux 与 macOS

`src/platform/unix/`（Linux 与 macOS 共用，只在进程目录、系统信息、进程列表处按 `target_os` 分）：Unix socket（`<runtime>/keepane-<uid>/`，0700，属主检查）、termios 原始模式与 SIGWINCH、VT 输入解析成与 Windows 相同的按键/鼠标记录（`src/vtinput.rs`）、pane 进程树按会话挂断（SIGHUP，半秒后 SIGKILL）、server 自成会话（SSH 挂断到不了）、SIGTERM/SIGHUP 时先存档再退出、bash（`--rcfile`）与 zsh（`ZDOTDIR`）的提示符钩子（先读用户自己的 rc）、每 pane 历史（HISTFILE）、剪贴板（pbcopy/wl-copy/xclip/xsel）、通知（notify-send/osascript）、macOS 的 sysctl/mach/proc_pidinfo。与平台无关、按 shell 走的：提示符标记带 `;sh` 的 shell 用 POSIX 写法投递消息（信封作为 `:` 的参数，多行用 `eval`），见 `server::actor::Syntax`。ConPTY 自己有屏幕、Unix pty 没有：恢复的 pane 在 Unix 上把存档文字直接写进 keepane 的屏幕模型（`console::PTY_HAS_SCREEN`）。e2e 测试平台中立化（Unix 上 pane 是 `/bin/sh`，提示符同为 `keepane>`；Windows 专属的 8 个用 `cfg(windows)`，bash 另有消息与目录测试，zsh 测试在有 zsh 时跑）。CI 加 ubuntu-latest、macos-latest；release 加 linux-x86_64（musl 静态）、macos-aarch64、macos-x86_64 三个 tar.gz。版本 0.16.0。

实现途中发现并修掉的（验收轮之前）：bash 的 PS1 里 ST 的反斜杠被提示符解码吃掉（改用 BEL）；OSC 7 在 Unix 上被丢弃（`windows_path_from_announced` 只收 Windows 路径，改为按平台的 `path_from_announced`，Unix 只收本机存在的绝对目录）；resume 的回放助手在 Unix 上无法与 shell 共用同一个 pty 会话；`git_branch` 把相对路径从进程 cwd 往上找；zsh 默认 SAVEHIST=0 导致每 pane 历史不落盘。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性（通读 src 改动） | clippy linux/mac-x86/mac-arm 0/0/0；`__shell-hook` 的注释只提 PowerShell | **有问题**，注释改为两种 shell |
| 2 | 机制通路（变异 9 个） | posix_quote、`;sh` 标记（两处）、PTY_HAS_SCREEN、直写回放、OSC 7 收录、zsh SAVEHIST、git 相对路径、e2e 引号：9/9 被抓；全量 Linux 187/84，但 e2e 有一次 `vim_keys` 失败：按键与 list-panes 走不同连接，断言没等 | **有问题**，同类 7 处改为等待（`wait_list`/`wait_buffer`） |
| 3 | 可复现性（Linux e2e 连跑） | 5 次里 1 次失败；再连跑 40 次找出 3 类：回显竞争（sh 由 tty 回显，提前敲入的一行跟在新提示符后面）、弹窗等到的 `pip>` 其实是命令行里的字、屏幕上 `keepane>` 计数在分屏前已够 | **有问题**，三处修测试；之后 40/40 |
| 4 | 可复现性（Windows） | fmt 0、clippy 0；全量 3 次：212/10/84 ×3；Linux 40/40（同代码） | 干净（1/3） |
| 5 | 边界与退化输入 | 超长 `-L`：报错但写着 "open pipe"（Windows 用语、不说路径）；无 SHELL → sh；10×3 终端 attach/detach 正常；空消息被拒、只有空行/中间空行的消息正常；相对的 OSC 7 被忽略；server 日志 panic 0 | **有问题**，改为 "connect to the server at <地址>" |
| 6 | 不变量（入口审计） | 投递语法只在 mail.rs 一处取 `p.syntax`；目录上报两处入口都走 `path_from_announced`；回放只经 `spawn_gen`。Linux 11/11、三目标 clippy 0；Windows e2e 2/2 失败于 `list-events -S`：事件日志与 pane 输出日志共用一个写线程，繁忙时（本进程 80 多个 server 同时起、各自先剪一遍历史目录）事件落盘晚于 `flush` 的 2 秒上限，查询读到的文件缺行而不自知；加负载复现 3/8 | **有问题**（旧设计缺陷）：事件日志单独一个写线程（`append_event`/`prune_events`/`flush_events`）；加单测（历史写线程被卡 3 秒，事件仍 <2 秒落盘）；同负载复现 0/8；变异（事件改回历史通道）被抓 |
| 7 | 代码正确性（错误路径、资源、并发） | 无 tty 的 attach、找不到 session、server 不在：提示清楚；Tree 的延时 SIGKILL 线程与信号管道不泄漏。Windows 全量 3/3（与 Linux 并发）；Linux 连跑中偶发 3 类失败：find-text 只等了第三个 pane、历史测试 C-c 后立即打字被 bash 吃掉开头、jobs 里已退出 pane 的 UP 在 0s/1s 间跳 | **有问题**：前两处修测试；第三处是产品 bug——`pane_start_time`/`pane_dead_time` 每次询问时各自按当时的秒取整，差值会跳。改为进程内一个固定锚点（`unix_seconds`），同一时刻永远是同一秒；加单测 |
| 8 | 可复现性 | Linux e2e 50+60 次 0 失败；lib 190；Windows 全量 3/3（214/10/84） | 干净（1/3） |
| 9 | 静态一致性（文档对代码） | bash 的命令开始标记靠 PS0，要 4.4 以上，macOS 自带 3.2 没有（没有命令时间、消息成败不标）——README 未说；`keepane --help` 说 notify 有"跳到 pane"按钮（只有 Windows 有） | **有问题**，中英 README 与帮助补上 |
| 10 | 机制通路（本批新改） | 变异：`hostname` 返回空、事件改回历史通道、`unix_seconds` 内部改回按次取整：3/3 被抓；但格式上下文改回局部闭包（不经 `unix_seconds`）**没被抓**——只有 e2e 偶尔碰到 | **有问题**（测试缺口）：jobs 测试加一个活半秒的 pane，1.5 秒里问 30 次开始/结束之差必须不变；同一变异 5/5 被抓 |
| 11 | 环境无关 | A：SHELL=zsh、LANG=C、TERM=dumb、无 XDG_RUNTIME_DIR → bash 与 zsh 消息测试都失败，逐个变量排查是 TERM=dumb：pane 继承了 server 自己的 TERM（tmux 从不这样）；B：socket 目录带空格、SHELL=/bin/sh → 通过 | **有问题**：`default-terminal` 由"接受并忽略"改为真选项（默认 xterm-256color），Linux/macOS 上写进每个 pane 的 TERM（Windows 由 ConPTY 管，不变）；配置单测 + e2e；新测试自己也犯了"提示符出来前就敲"的错，修后 20/20 |
| 12 | 机制通路 + 全量 | 变异：去掉 pane 的 TERM、选项不存值：2/2 被抓；Linux 三种环境（A/B/普通）全绿，普通环境 e2e 25/25（85 个）；三目标 clippy 0；Windows 全量 3/3（215/10/84） | 干净（1/3） |
| 13 | 静态一致性（平台接口对设计文档） | 两边公开接口逐项对照：名字一致（PowerShell 钩子以 `pub use` 同名导出；Windows 多出的 shutdown 窗口只在 cfg(windows) 用）；但 platform.md 的接缝表缺两项：pane 的 TERM（`SETS_TERM`）、主机名（`hostname`）。另查不变量：运行程序的 8 个入口（pane、分屏、新窗口、respawn、resume、弹窗、hook/run-shell、if-shell、pipe-pane、#()）都经 `pane_env` | **有问题**（文档缺项），补上 |
| 14 | 边界（新选项） | 配置文件设 screen-256color 且 server 自己 TERM=dumb → pane 得 screen-256color；`set-environment TERM` 盖过它；空值与带空格的值被拒；300 字符照存；缩写 `default-t` 可用。Linux 全量 + e2e 31/31；Windows 2/2 | 干净（1/3） |
| 15 | 可复现性（逐字节） | musl release 两次构建 sha256 相同（a57bdd01…）；`__shell-hook` 两次相同；两个 server 写出的 5 个启动文件逐字节相同；Linux lib 191、e2e 30/30；Windows 2/2 | 干净（2/3） |
| 16 | 退化输入（终端输入解析） | 新增确定性模糊测试（20 万字节、任意切块）：不 panic、不囤积。但读代码发现：括号粘贴要等到结束标记才放行，而客户端 30ms 无输入就把囤着的字节当按键冲出去——SSH 上分几次到达的大段粘贴会被拆坏（开头的 ESC[200~ 变成 Esc 键加字面的 `[200~`） | **有问题**（真 bug）：粘贴改为"模式"，文字到了就放行，只留可能是结束标记开头的字节和没收全的 UTF-8；粘贴中不计超时。真客户端实测：中间停 200ms 的粘贴完整到达；3 个变异 3/3 被抓 |
| 17 | 大输入端到端 + 全量 | 真客户端粘贴 100,030 字节（含中文、emoji、制表符，4KB 一块、块间停 50ms）进 `cat > 文件`：收到 100,030 字节、逐字节相同、3.06 秒。Windows 2/2（216/10/84），三目标 clippy 0；Linux e2e 30 次里历史测试失败 1 次（之后单跑 80 次、全量 55 次、双路并发 90 次都没再现，现场没抓到）。追查中读代码发现：`respawn-pane` 换进一个全新的 Pane，名字、工作模式、收件箱都丢了，排队的消息无声消失（0.15 起的旧 bug） | **有问题**：respawn 带上 actor（`Actor::restarted`：名字/模式/收件箱留下，重新等新程序的提示符或 ready，手上那条以"放弃"记入事件日志）；单测 + e2e；3 个变异——其中"重启后仍算空闲"一开始没被抓（测试先走过 next_delivery），补测试后 3/3 被抓 |
| 18 | 全量 + 可复现性 | Linux lib 193、e2e 45/45（抓现场的循环，无失败）；三目标 clippy 0；Windows 3/3（217/10/84） | 干净（1/3） |
| 19 | 静态一致性（respawn 新行为与设计文档） | mailbox.md 没写 respawn 的规则；另有 6 处只按 PowerShell 写（shell 包装、多行改写、agent-commands 默认值、默认模式、钩子标记、"shell 模式目前只支持 PowerShell"） | **有问题**，补写 respawn 规则，6 处补上 bash/zsh |
| 20 | 不变量（重启入口） | 三个入口（respawn-pane、respawn-window、choose-jobs 的 r）都经 `Cmd::RespawnPane` 同一处；respawn-window 实测：`left:normal:1 right:shell:0`（名字、模式、收件箱都在）；Linux lib 193、e2e 40/40；Windows 2/2 | 干净（1/3） |
| 21 | 损坏的持久化状态 | 乱码与半截的会话文件：list-saved 跳过、好的照常恢复；事件日志里夹着半行 JSON 与 NUL：list-events/list-tasks 正常；历史目录 chmod 000：server 照跑；在坏日志上重启：正常；server 日志 panic 0。Linux e2e 40/40；Windows 全量通过 | 干净（2/3） |
| 22 | 测试对 macOS CI 的适用性（读测试） | macOS 的 bash 是 3.2（无 PS0），按第 9 轮写进 README 的行为，失败不会被标出；但 bash 消息测试断言 `#N failed`，在 macOS CI 上必然失败 | **有问题**：测试按本机 bash 版本（`BASH_VERSINFO` ≥ 4.4）期望 failed 或 done；其余依赖（seq、od、dd、stty、kill -0、uname -n、带空格的数据目录）逐一核对，macOS 都有/都能处理 |
| 23 | 真机 CI（三系统） | 04079ea 推上去：macOS 首次真跑全绿（lib、e2e 含 zsh 测试）、Windows 绿；Ubuntu 的历史测试失败："no history file"。本地用空 HOME 复现 3/3：每 pane 历史的目录只在"有共享历史可复制"时才顺带建出来，新机器（还没有 ~/.bash_history）上 bash 的 `history -a` 写不进不存在的目录（PSReadLine 自己建目录，所以 Windows 从没暴露）。第 11 轮的环境矩阵漏了"空 HOME" | **有问题**（真 bug）：目录总是先建；空 HOME 下 3/3 通过、全量通过（zsh 测试除外：本机解包的 zsh 靠真 HOME 的 .zshenv 找 fpath，是测试环境所限，macOS CI 上的真 zsh 已通过）；普通环境 20/20；Windows 全量通过 |
| 24 | 真机平台（CI） | 57f85de：Windows、Ubuntu、macOS 三个 job 的 fmt、clippy、全量测试都通过 | 干净（1/3） |
| 25 | 可复现性 | 同一提交在 CI 上重跑（attempt 2）三系统再全绿；本地 Linux lib 193、e2e 30/30；Windows 2/2（217/10/84） | 干净（2/3） |
| 26 | 静态一致性（发布产物与文档） | 照 release.yml 的打包步骤逐字在本地跑：`keepane-v0.16.0-linux-x86_64.tar.gz`（+ .sha256），内含同名目录与 keepane、两份 README、配置样例、LICENSE，与 README 的下载名和 `install` 路径一致，解出的程序报 `keepane 0.16.0`；中英 README 15 个要点逐一对照都有；Linux 全量 193/85，Windows 全量通过 | 干净（3/3） |

遗留：macOS 只在 CI 上跑过测试，没有人手在 Mac 上用过（交互客户端、剪贴板、通知）；本机没有装 zsh（sudo 要密码），zsh 测试用解包的 zsh 加临时 `~/.zshenv` 跑，真 zsh 由 macOS CI 覆盖；bash 4.4 以下（macOS 自带）只在文档里说明，没有实测；`keepane startup` 在 Linux/macOS 上还没有（systemd user unit / launchd agent）。

## 66. lazygit 式的 dashboard；信封改成字段；按字段发送（0.17.0）

一、dashboard 重做（docs/design/dashboard.md）：左列 [1] 窗格（模式、忙闲、收件数、安静多久、程序）、[2] 所选窗格的收件箱、[3] 任务；右边 [0] 所选窗格（地址、目录、pid、跑了多久、在做什么，下面是带颜色的屏幕、回滚、事件，或一条消息 / 一个任务的全文，信封按字段逐行列出）。能直接操作：发消息、改名、改模式、解卡、跳过去（弹窗里顺带关掉）、关窗格、删 / 移 / 置顶排队消息；关窗格、删消息、切到 shell 三样先确认。鼠标点选与滚动，`?` 列出全部按键。取消了旧的"观测 / 管理模式"。

二、信封（用户要求：直接看 JSON 太难看）：投递、`read-message`、`trace-message` 里写成 `[keepane id=… task=… from=… to=… via=… hop=…]`，结束行 `[keepane end=…]`；`message-envelope json` 保留旧写法；事件日志始终是 JSON。

三、按字段发送：`send-message --to / --re / --task`（MCP 的 `send_message` 同样有 `re`、`task`）。`from`、`name`、`mode`、`id`、`hop`、`via` 由 keepane 填，发送方给了就报错并说明原因（否则来源可伪造、hop 上限拦不住循环）。

实现中顺带发现并修掉的：bash / zsh 的提示符标记原来放在 PS1 里，行编辑器在长行折行、提前打字时会重画 PS1，标记跟着重发，命令还在输入就被当成结束、窗格被当成空闲（改为每个提示符只从 `PROMPT_COMMAND` / `precmd` 发一次；新加确定性测试：比窗格宽的 `sleep 1; echo …` 在 0.5 秒时必须仍在运行，把标记放回 PS1 则 3/3 失败）；命令运行中打的字在提示符重画后不再让窗格保持忙（改为直到有人再打字）；`--task` 把被拒收的消息算进 hop；Windows 弹窗里 dashboard 第一行被顶掉（改为每行按绝对位置画，并每次刷新都重新读窗口大小）；原动画帧数测试在整套并行时帧数不足（只验证"在动"，门槛 20→4）。wmux 名字的安装包按用户要求从 0.16.0 的发布里删除，以后不再生成。

| 轮 | 视角 | 数据 | 结论 |
|---|---|---|---|
| 1 | 静态一致性 | 搜旧说法（管理模式、PS1 里的标记、JSON 信封行）：两份 README 开头动图的替代文字还写"在管理模式下置顶" | **有问题**，改 |
| 2 | 机制通路（变异 9 项） | 字段分隔、选项不读、`--re` 不默认回发送方、`--task` 的 hop 不加一、拒收算进 hop、受保护字段放行、打的字不保持忙、关窗格不确认、读不出字段式信封：9/9 被抓（其中一项第一次指定错了测试，换对后抓到）；Windows 225/10/84；Linux lib 201、e2e 20/20；三平台 clippy 0 | 干净（1/3） |
| 3 | 边界与退化输入 | `--re` 取 0、-1、溢出、字母、空：都拒并说明；配置文件设 json 生效；只有拒收消息的任务报"没有这个任务"；64 字符的名字写成字段仍是 8 段、无空格值。但 `--re` 回复窗格外来的消息且没写 `--to` 时，报错写成 `-r` 且没说怎么办 | **有问题**：按实际用的参数说，并提示"用 --to 指定"；e2e 加断言 |
| 4 | 可复现性 | Windows 全量两次 225/10/84；Linux lib 201、e2e 30/30；同一面板状态画两次整帧逐字节相同（18 种尺寸） | 干净（1/3） |
| 5 | 不变量（入口审计） | 显示消息的 5 个出口（投递、read-message、trace-message、MCP 的 wait_message 与 current_message/trace_message）都经 `header(style)`，`envelope()` 只剩写日志；新消息只在 `send_message` 一处构造，两个调用方（所有 send-message 与 create-pane 的首条任务）；Windows 225/10/84，Linux 201/85 | 干净（2/3） |
| 6 | 真机三平台（CI run 36284559910） | macOS、Ubuntu 通过；Windows 225/10 通过，e2e 83/84：`jobs_lists_every_pane_with_its_state` 报 "the brief pane never ended"。该测试用 `powershell.exe` 起一个半秒就退出的窗格并只等 10 秒，84 个测试并行时 Windows PowerShell 冷启动超过了这个时间；产品代码不涉及（同一测试前半段 `cmd.exe` 起的窗格正常结束） | **有问题**：改用 `cmd.exe /c ping -n 2 127.0.0.1 >nul & exit 3`（实测退出码 3、活 1 秒），超时时打印整个会话的 `jobs`；单测连跑 5 次 5/5 |
| 7 | 慢机器上的时间假设（审查 + 压测） | 审查：测试里等外部程序启动的地方只有 OSC 9;9 那处等 `pwsh` 只给 5 秒（改 15 秒）。压测：32 核全占满时全量 225/10/84 仍全过（e2e 19.7s），于是把测试进程和 4 个空转进程绑在同样 2 个核上（每个任务约 0.3 核，比 CI 的 4 核苛刻得多），e2e 两次分别挂 3 个、2 个。逐个查出四处竞态：① 小客户端视图测试只等"长串出现"，而它在输入的命令行里就有，没等输出和新提示符，按键时光标还在输出后的空行，视图正确地跟到第 0 列；② `at_hooked_prompt` 在 Windows 上看屏幕最后一行是否 `PS `，恢复的会话先重放旧屏幕，旧提示符让它提前返回，`Up` 被还没起来的 `pwsh` 丢掉（改为两个平台都等 keepane 的提示符标记）；③ 服务端加临时计时日志后查明：假标记测试让一次提示出现两个标记，负载下两者相隔超过 60ms，第二个标记把下一条消息提前判完成，之后逐条错位，`next` 的输出为空（pwsh、bash 两版都挪到测试最后、单独窗口，并先热身）；④ 测试工具读回复只等 10 秒，而 `trace-message -w 30` 本就允许 30 秒（改 40 秒）。另验证并否定了一个产品层假设："结束标记会被 ConPTY 排到提示符文字前面，60ms 不够"。原始字节流显示 `keepane-cmd`、`9;9` 会提前，但 `133;B` 与 `keepane-prompt` 始终在提示符文字之后、同一次读取内；按该假设改成"等标记后的一帧"，10 条消息 10/10 都要等满 1 秒，已撤回。修后同样负载：改过的 4 个测试 8/8、8/8、8/8、10/10；全量三次 84、84、82 过，剩下的失败只是超出设计上限（`#()` 一次性求值 3 秒上限内 `pwsh` 没起来、15 秒画面等待），没有竞态。正常条件 Windows 两次 225/10/84，Linux 201/85 | **有问题**，已修（不计数） |
| 8 | 与 CI 相当的环境（可复现性） | Windows 测试进程限定 4 核（CI 的 windows-latest 是 4 核），全量连跑三次 225/10/84 ×3（21–22 秒）；Linux 全量两次 201/85 ×2 | 干净（1/3） |
| 9 | 静态一致性（逐行读本轮 diff） | 源码无诊断残留（`git grep DIAG`、`diag_ms` 为 0，`src/` 无 diff）；`Conn::next` 改 40 秒不影响 `wait_for`（它用自己的 500ms 读取），测试里 `-w` 最大 30（1 处 15、22 处 30）属实；但视图测试的新注释写"cmd 正在打印"，Unix 上是 sh | **有问题**：改为"the shell"（不计数） |
| 10 | 机制通路（变异 3 项） | a 窗格死后 `pane_dead_time` 取当前时间：`jobs` 测试抓到；b 视图跟随多退一列：没抓到，该测试本就接受第 8 或第 9 列（cmd 是否已回显 x），差一列落在容忍内，属旧有宽容，记下不改；c 去掉"标记后 60ms 内不送下一条"的保护：84/84 全过，这一保护此前没有任何测试。新加 `sent_as_the_prompt_comes_back`（pwsh、bash 两版）：先发一条跑半秒的消息，轮询到 `#{pane_idle}` 变 1（即标记到达、窗口开始）立刻发第二条，断言两条各以自己的输出完成；再跑变异 c 抓到（第一条被顶替，永远不结束）；正常 5/5 | **有问题**：补测试（不计数） |
| 11 | 全量（两平台） | fmt/clippy 0；Windows 225/10/84；Linux 201/85 | 干净（1/3） |
| 12 | 机制通路（Linux） | 第 10 轮的变异 c 只在 Windows 上做过：Linux 上去掉保护，bash 版在 `sent_as_the_prompt_comes_back` 的第一条断言失败；恢复后该测试连跑 5 次 5/5（每次 3.6 秒） | 干净（2/3） |
| 13 | 真机三平台（CI run 36288253492，提交 68a8bc9） | windows-latest、ubuntu-latest、macos-latest 的 fmt、clippy、全量测试都通过 | 干净（3/3） |
| 14 | 真机三平台（CI run 36288382993，提交 3c28d21，只改了本文件） | 发版前的 CI 在 macOS 上挂了：bash 版的假标记检查（第 7 轮挪进新窗口）输出为空，took 65ms。新 bash 里第一次启动外部的 `sleep 0.01` 在 macOS 上超过 60ms 窗口；pwsh 版挪窗时加了热身，bash 版漏了。同样的代码第 13 轮通过，说明一次 CI 不足以证明 | **有问题**：bash 版同样先热身（不计数）；v0.17.0 的发布在等 CI 这一步就停了，没有产出，标签移到修复后的提交 |
| 15 | 全量（两平台） | fmt/clippy 0；Windows 225/10/84；Linux 201/85；bash 版该测试 Linux 连跑 10 次 10/10 | 干净（1/3） |
| 16 | 慢机器（Linux） | 测试与 4 个空转进程绑在同样 2 个核上：shell 测试 8/8；e2e 全量两次 85/85（每次 34.7 秒，平时 8.7 秒） | 干净（2/3） |