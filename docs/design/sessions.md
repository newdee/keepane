# 保存的 session：启动时恢复，kill 的放一边

## 每次启动都恢复（0.64，2026-11-13）

用户说「为什么会有裸keepane呢？没有session的时候，运行keepane就会创建一个默认session」「应该是两个，这样才能保证全局session一致，不出现裸session的情况」，以及「改吧，按推荐来」。

- `restore-on-start` 默认改为开。server 不管怎么启动（`keepane`、`keepane new -s x`、登录时），读完配置就先恢复所有保存的 session，再执行带它起来的那条命令。所以不会出现"保存着的 session 没回来，旁边多了一个新 session"。
- 恢复之后，`new -s work` 遇到已在运行的 `work` 和 tmux 一样报 duplicate session，`-A` 改为接入它。
- 选项关掉时，保存着却没在运行的名字不能被新建或改名占用（它的 autosave 会覆盖存档），`new -A -s 名字` 改为恢复它。
- 手机页面（web）也恢复成 server 结束时的样子：开着的继续开，`web stop` 过的不开。用户说「启动时候web也默认保持之前的状态吧，现在每次都要手动启动web很麻烦」。
- 分享随 session 一起恢复，见 [share.md](share.md) 的"持久化"。

## 每个 server 一套存档（方案 A）

问题：存档目录和 `shares.json` 原来由所有 server（不同的 `-L` socket）共用。启动时默认恢复以后，启动 `keepane -L lab` 会把默认 server 的全部存档恢复一份（包括那边正在运行的，同样的程序跑两份），按同样的 token 再提供一遍它的分享，两边还写同一个 `shares.json`。用户问「为什么会有多个server」「多个server的意义是什么？」「tmux支持多个server吗？」，然后「改吧，按推荐来」。

- 默认 server 照旧用数据目录下的 `sessions`（老用户不用迁移）；`-L NAME` 的 server 用旁边的 `sessions-NAME`（名字里不能做文件名的字符换成 `_`）。`killed/`、`share/`、`next-id` 都在各自目录里。web 状态本来就按 socket 分开放，这是同一个做法；也相当于替 tmux-resurrect 的用户自动配好了 `@resurrect-dir`。
- 设置了 `sessions-dir` 或 `KEEPANE_SESSIONS_DIR` 时照旧用指定的目录（用户明确的选择）。
- `show-options sessions-dir` 显示这个 server 实际用的目录（`Server::option_value`），`keepane migrate` 靠它。
- 代价：以前在命名 server 里存的 session 留在共用目录里，从命名 server 看不到了，从默认 server 仍能 `resume`。

## kill-session 的存档放到 `killed/`（方案 B）

问题：每次启动都恢复以后，手动 `kill-session` 掉的 session 会在下次启动时回来。方案 A：kill 时删掉存档；方案 B：把存档挪到存档目录下的 `killed/`。用户选 B（「改吧，按推荐来」）。

- 启动时和裸 `keepane` 只扫存档目录顶层，`killed/` 里的不回来。
- `resume 名字` 能找回；先把文件挪回顶层，以后照常保存和恢复。
- 不带名字的 `resume`（`restore-session`）恢复所有存档，`killed/` 里的也算，同样挪回顶层。这一条保持 0.64 之前的行为：README 写的是"bring back every saved session"，已有测试也按这个断言。B 只改了启动时的行为。
- `list-saved` 把它们标成 `(killed: resume brings it back)`。同一个名字之后又在顶层存过一份时，`resume` 用较新的那份，旧的不列出。
- `delete-saved` 和 session 自己结束（最后一个程序退出）后的遗忘，两处的存档一起删。
- 挪回顶层时，如果目标文件已存在（手写或改过名的文件），报错不覆盖。
