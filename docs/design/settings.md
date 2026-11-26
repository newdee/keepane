# 设置板：`keepane settings`（0.94）

用户要求（2026-11-26）：

- "keepane 也可以做一个 setting board，用某个命令唤出，然后把设置分组，包括功能，按键，主题，等等等等，修改完后更新到本地配置文件里，也可以直接修改配置文件然后加载或者更新，建议每个配置组是一个框，然后框里面如果是字体这种，可以给选择而不是一个一个输入，总之怎么方便怎么来。"
- "每个配置组的框可以放大到全屏或者缩小到原来位置，但是不允许删除和修改位置等。"
- 看过 HTML 模拟后："写进设计文档吧，然后开始做吧"。

## 1. 为什么要做

keepane 有 97 个选项（`show-options -g` 列出的，`config::SHOWABLE`）。现在只能手写 `~/.keepane.conf` 或敲 `set -g`：要先知道选项名、取值范围和写法，改错了要读报错。设置板把选项分组列出来，能选的不让输，改了马上看到效果，确定了再写进文件。

字体不在其中：keepane 跑在终端里，字体由终端程序决定（Windows Terminal、UnifyTerm、iTerm2）。网页端的设置页以后再做，字体在那里选。

## 2. 唤出

- 命令 `keepane settings`：在当前终端里打开。
- `prefix g`（`settings` 命令）：在弹窗里打开，占 90%，和 `prefix v` 的 dashboard 一样。`g` 是 prefix 表里仅剩的 4 个空键之一（`g` `G` `a` `A`）。
- 退出：`q`；没有放大、没有筛选时 `Esc` 也退出。有没写进文件的改动时先问（见第 6 节）。

## 3. 布局：固定的 9 个框

```
╭─[1] Look ────────────── 21 ╮╭─[2] Status line ──────── 12 ╮╭─[3] Keys ─────────────── 4 ╮
│ theme        ‹ paper-dark ›││ status                 on  ││ prefix                 C-b │
│ window-style  fg=#cecabe,… ││ status-position    bottom  ││ repeat-time           500  │
│ …                          ││ …                          ││ …                          │
╰────────────────────────────╯╰────────────────────────────╯╰────────────────────────────╯
╭─[4] Windows and panes ─────╮╭─[5] Sessions and history ──╮╭─[6] Alerts ───────────────╮
…
╭─[7] Agents and messages ───╮╭─[8] Machines, web, Feishu ─╮╭─[9] System ───────────────╮
…
 j/k option  Tab/1-9 box  ←/→ change  Enter edit  r default  z zoom  / filter  m changed  w write  R reload  q quit
```

- 9 个框的位置和顺序固定：不能删、不能拖、不能隐藏。筛选（`/`）时没有匹配项的框留在原处，写"no match"。
- 宽 114 列以上排 3 列，76 列以上排 2 列，再窄排 1 列。排不下时整个板按框上下滚动，当前框总在视野里。
- 框里一行一个选项：左边名字，右边值。值的写法按类型（第 4 节）。
- 当前框的边框是绿色（和 dashboard 的聚焦面板一样），当前项反色。
- 名字前的标记：`●` 改了还没写进文件；`◦` 和默认值不同。
- 框只能放大或还原：`z` 放大到整个板，`z` 或 `Esc` 还原，光标停在原来那一项。鼠标点框的标题行也一样（再点一次还原）；标题右边写这个框有几项，筛选时写 `剩下/全部`。
- 放大后每项多显示默认值和来源（第 7 节）；底部写当前项的说明（至多两行）和它能填什么（可选值、步长或写法）。
- 9 个组和它们的选项写在 `src/optinfo.rs`，一个选项属于一个组。

| 框 | 选项 |
|---|---|
| 1 Look | theme、window-style、pane-colours、边框（pane-border-*、pane-active-border-style、pane-frames）、pane-dim、display-panes-*、clock-mode-*、choose-tree-style、dashboard-icons、animation、animation-time |
| 2 Status line | status、status-position、status-justify、status-style、status-left/right、它们的 length、status-interval、window-status-* |
| 3 Keys | prefix、repeat-time、prefix-hint、prefix-hint-delay；下面是 prefix 表的每个键（0.96） |
| 4 Windows and panes | mouse、history-limit、base-index、pane-base-index、remain-on-exit、keep-zoom、undo-kill-time、window-size、pane-timestamps、default-shell、default-command、default-terminal |
| 5 Sessions and history | autosave、restore-on-start、sessions-dir、save-history、log-history、log-history-days、log-history-dir |
| 6 Alerts | monitor-*、visual-*、notify、display-time、done-* |
| 7 Agents and messages | agent-pane-limit、agent-commands、agent-cost、message-*、chat-days、chat-file-max-size |
| 8 Machines, web, Feishu | link-*、web-upload-*、feishu-* |
| 9 System | update-check、plugin-path、event-log*、git-command、hint-open、settings-file |

## 4. 按类型改值：能选就不让输

| 类型 | 显示 | 改法 |
|---|---|---|
| 开关 | `on`（绿）/ `off`（暗） | Space、←/→、Enter 切换 |
| 枚举 | `‹ value ›` | ←/→ 换一个（`option_values` 给的值）；Enter 输入清单外的值（`done-events` 的组合、`feishu-domain` 的地址） |
| 数字 | `‹ 500 ›` | ←/→ 按步长加减；Enter 直接输数字 |
| 颜色 | 值本身 | ←/→ 在 `default` 和 16 色名之间换；Enter 输 `#rrggbb` |
| 密钥 | `(set)` 或 `(none)` | Enter 输入，回显成 `•` |
| 键 | `C-b` | Enter 后按下新键 |
| 文字、格式串、大小、样式、列表 | 值本身（太长截断） | Enter 在底行编辑，原值预填 |

- `‹ ›` 只画在当前项上，别的行只写值，免得满屏箭头。
- 改了马上生效：板发 `set-option -g NAME VALUE`，服务器的校验原样有效。不合法时底行写服务器的报错，值不变。
- 底行编辑：←/→ 移动光标，Home/End，Backspace/Delete，Enter 确定，Esc 取消。
- `r`：这一项回到默认值（`set-option -gu NAME`）。

## 5. 文件：只写 `~/.keepane/settings.conf`

**不改 `~/.keepane.conf`。** 它是命令脚本：有 `%if`、`source-file`、注释和顺序，程序改写会弄丢注释或改到错的分支。

- 设置板只写 `settings.conf`。keepane 启动时先运行 `~/.keepane.conf`，再运行 `settings.conf`，所以后者说了算。
- 手动 `source-file` 那个启动时运行的配置文件时，`settings.conf` 也跟在后面运行，后者照样说了算（第四轮验收发现：否则配置文件会盖过它，直到重启）。从别的文件里嵌套 `source-file` 它时不跟（外层结束后本来就会运行）。
- 位置由新选项 `settings-file` 决定。默认（空）是 `~/.keepane/settings.conf`；用 `KEEPANE_CONFIG` 或 `-f` 指定了配置文件时默认没有 settings 文件，要在那个配置里写 `set -g settings-file PATH`。测试都用 `KEEPANE_CONFIG`，所以测试不会读到这台机器的 `settings.conf`。
- `save-settings NAME...`（服务器命令）：把这些选项的当前值写成 `set -g NAME VALUE`。文件里已有这个选项的行就原地替换，没有就加在末尾。其他行（注释、手写的命令）原样保留。文件不存在时连目录一起建，第一行写一句说明。
- 密钥的真值由服务器写，板只看得到 `(set)`。
- 值里有换行的选项不写（一行装不下，第二行会变成坏命令），整个 `save-settings` 什么都不写并说明原因（第三轮验收发现）。写失败时旁边的临时文件删掉。
- 文件可以手改，改完在板上按 `R` 重新加载。

## 6. 什么时候写，什么时候丢

- `w`：把改过的项写进文件（`save-settings`）。底行写"saved N options to …"。
- `q`：有没写的改动时问："N changes not written: w write and quit · q quit, they last until the server restarts · Esc stay"。
- `R`：重新加载。先问，写明会丢几项没写的改动、几项运行时改的。确认后服务器执行 `reload-settings`：
  1. 每个和默认值不同的选项先回到默认（走 `set-option -gu`，和手敲一样生效）；prefix 表也回到默认（0.96）。
  2. 运行主配置文件。
  3. 运行 `settings.conf`。
- 插件随 `@plugin` 行重新加载：插件设的选项在第 1 步回到了默认，不重新加载就丢了（第一轮验收发现）。`@plugin` 的记录不会因此重复。和手敲 `source-file` 一样，配置里的 `bind` 再跑一遍；`set-hook -a` 会再加一次。

## 7. 来源

服务器记住每个选项最后是谁设的：

| 来源 | 写法 |
|---|---|
| 没人设过 | `default` |
| 配置文件里的一行 | `~/.keepane.conf:12`、`~/.keepane/settings.conf:3` |
| 主题顺带设的 | `theme paper` |
| 运行时（命令、按键、设置板还没写的改动） | `set` |

- `set-option -u` 去掉记录（回到 `default`）。
- `show-settings -J`（服务器命令）一次给出：settings 文件的路径、每个选项的当前值、默认值、来源。板每秒问一次，所以别处的 `set` 也会马上显示。

## 8. 一份选项清单（`src/optinfo.rs`）

每个选项记：名字、组、类型（开关、枚举、数字和步长、大小、颜色、样式、文字、格式串、列表、密钥、键）、一句说明。

- 说明和 man 页"Every option"表里那一行的说明一字不差：测试逐行对照。
- 测试保证：清单和 `SHOWABLE` 一一对应；`option_values` 给出固定值的选项都是枚举或开关；每个组至少一项。
- 以后网页设置页、命令补全读同一份。

## 9. 安全

- 板只发这几条命令：看 `show-settings`、`show-bindings`、`show-format`（0.97 起，格式串预览，`#()` 不跑）；改 `set-option`、`save-settings`、`reload-settings`、`rebind-key`、`bind-key`（后两个 0.96 起）。和 dashboard 一样有白名单，测试逐键检查。
- `reload-settings` 会丢运行时的改动，要确认。
- 密钥不显示、不进日志：`show-settings` 给 `(set)`，底行输入回显 `•`。命令行里的密钥值在事件日志、日志和 `:` 提示的历史里写成 `(set)`（`command::redact_secrets`，0.95）。
- 人的选项（`config::PERSONS`）agent 的 pane 只能读；它也不能 `save-settings`、`reload-settings`（0.95，护栏不是墙，见 principles.md 信任边界）。

## 10. 顺带修的

`set-option -u` 一直没做：`-u` 被当成和 `-g` 一样忽略，man 页却写着"`set -gu` back to the default"。结果 `set -gu mouse` 把 mouse 翻转了，而不是回到默认。现在 `-u` 回到默认值。

## 11. 分步

1. **第一步（0.94）**：选项清单、`settings-file`、来源、`show-settings`、`save-settings`、`reload-settings`、`set-option -u`、`keepane settings` 和 `prefix g`，以及第 3–7 节的全部。
2. **第二步（0.96，做了）**：Keys 框在 4 个选项下面列出 prefix 表的每个键（`C-b c  new-window`）。选中一条按 Enter 后按新键；新键已被占用时两行标红，底行写"C-b x is bound to kill-pane"，Enter 交换，Esc 取消；`r` 放回配置文件给它的位置（`base`）；配置绑过、现在没有键的命令显示 `(no key)`，按一个键就绑上。
3. **第三步（0.97，做了）**：格式串在放大的框里编辑时，下面实时显示展开的结果（带颜色）。板每次文字变了就问 `show-format -J`（新的只读查询，进了 `QUERIES`）；它用一个用完就丢的 `ShellCache` 展开，`#()` 不运行、展开成空，所以敲到一半的命令不会被执行；对着当前 pane 展开（和别的命令一样：客户端所在的 pane，在弹窗里就是最近使用的会话的活动 pane；一个会话都没有时只有 server 自己的变量）。
4. ~~第四步：网页端的设置页~~：不做（用户 2026-11-26："这个不做"）。

## 12. 主题扩展（讨论过，待做）

2026-11-26 讨论过主题的扩展（HTML 模拟看过）。用户同日定："P5 也不用做，其他按推荐来做就行"。顺序：安全两项 → 设置板第 2 步 → P3 + P6 → P1，然后 P2、P4 → 设置板第 3 步。

- **P1（0.97，做了）**：用户 2026-11-26 选"只在主题内用 token"（不跨主题共用一份格式：共用的话 paper 两套和另外 6 套至少一方外观要变）。每套主题开头 `set -g @paper/@ink/@bar/@soft/@muted/@line/@accent/@warn`，后面的行引用 `#{@accent}`。`#{@name}` 在**设置选项的那一刻**替换（`Options::with_tokens`，`set_one` 开头）：样式选项（`window-style` 等）设置时就要解析颜色，格式引擎原本也不认 `#{@}`，没有配置依赖它在绘制时变；`source-file` 主题文件照样能用；存下的是颜色本身。没设的名字、没收尾的 `#{@` 原样保留；密钥（`SECRETS`）不替换、原样保存；单设一个 `@` 颜色不让 `theme` 变 `custom`（已设好的颜色不变，外观没变）；用户主题的 `@` 行排在 paper-dark 其余行之前，只写颜色的主题也是完整外观。默认选项带 paper-dark 的 8 个 token（从主题文件读，只此一处）。改写前后 8 套主题的选项值逐行相同（816 行，`theme-dump.ps1`）。
- **P2（0.97，做了）**：界面色和 ANSI 分开：`mode-style`（选中的行、选中的块；块用它的前景背景对调；默认 `fg=yellow,bg=black,bold`，就是原来的 `SELECTED`）、`message-style`（提示和消息；默认 `fg=brightwhite,bg=black`）、`popup-border-style`（弹窗、菜单、面板、按键提示的边框；默认空，各用原来的颜色）。存在每帧新建的 `Grid` 的 `ui` 里，绘制函数签名不变；默认值和原来逐格相同。样式按 tmux 的写法解析（`reverse`、`underscore` 等能画的就画，`blink` 等画不了的接受不画）。主题暂不设它们（外观不变）。
- **P3（0.97，做了）**：8 套主题都内置（新增 `nord`、`gruvbox-dark`、`catppuccin-mocha`、`dracula`），都自己画 pane（`tokyo-night` 原来交给终端，现在也画），对比度测试从 3 套查到 8 套（文字 4.5:1、线条 3:1、提示行、状态栏每段颜色）。原配色达不到的颜色只调 OKLCH 亮度，色相不变；每个主题文件写明调了哪些。
- **P4（0.97，做了）**：`theme-path`（默认 `~/.keepane/themes`）里的 `NAME.conf` 是用户主题：每行 `set -g`，只能设主题选项（按全名核对，缩写也认）；先铺 `paper-dark` 再叠它，所以没写的是 paper-dark 的，照样整套替换；每次设 `theme` 都重读文件（所以 `theme-path` 要先设：`merge` 有一张 `READ_BY` 表，写设置文件时把 `theme-path` 那行放到 `theme` 前面）；内置名字保留，名字只许字母数字 `-` `_`（出不了目录）。`list-themes` 列出全部（内置在前）；`show-settings -J` 带 `themes`，设置板的 ←/→ 和说明、`set theme` 的 Tab 补全、手机页的主题菜单都用它。`pick-theme`（不是 `choose-theme`：`choose-t` 要留给 `choose-tree`，tmux 的习惯）：光标或鼠标到哪就换成哪个，Enter 保留高亮那一行的（预览时没设上的主题，比如有一行不合规，按 Enter 再设一次并说出原因；选择器不能过滤），Esc 换回打开时的主题（原来是 `custom` 就逐项放回），连同各选项的来源，设置板不会显示成未保存的改动。
- ~~P5 跟随终端亮暗~~：不做（用户 2026-11-26）。
- **P6（0.97，做了）**：`pane-dim`（默认开）：有主题时不是当前的 pane 背景暗一点（深色纸往黑走 35%，浅色纸 5%；交给终端颜色时不压暗）；在画完 pane、画弹窗之前做，弹窗和提示不受影响。内置主题的状态栏：工作模式是 `normal` 时不显示；客户端窄于 120 列藏 CPU / MEM / 电池，窄于 100 列再藏目录（`#{&&:#{client_width},#{<:#{client_width},N}}`，没有客户端时宽度 0，不藏）。默认的 `pane-border-format` 在 `ai`、`shell` 模式的 pane（以及程序已退出的 pane）后面加 `pane_state`，颜色和 dashboard 一样（busy 黄、idle 绿、exited 红、typing 青、其余暗）。
