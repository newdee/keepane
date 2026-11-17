# agent 的状态和用量更准：Codex、Cursor（设计稿）

状态：第 1、2 步已实现（0.74.0，验收第 200 条），第 3 步 0.75.0，第 4、5 步其后。2026-11-17 用户提出"之前对于 claude 的余额和状态以及 trace、hook 等，显示的较为准确，但是 codex 和 cursor 这些并不准，可以参照一下 herdr 怎么做的"。调研 herdr 0.9.5（herdrdev/herdr）后给出五步方案，用户说"改吧，都按推荐来"。

## 调研结论（对照 herdr）

- herdr 只判断状态（在忙、等人、空闲）和会话 id，不读 model、tokens、花费、上下文、限额。用量上没有可借的。
- herdr 的状态来自屏幕和终端标题的规则（每个 agent 一组，带优先级和读哪一块）。它曾用 Codex 的 hook 报状态，0.9.5 撤掉了：提问界面出现时 hook 还停在"在忙"。
- keepane 对 Codex 的忙和闲（会话记录 + hook）不比 herdr 弱。缺的是：等人、会话记录归属、几个数字的算法。
- keepane 对 Cursor 不读任何数据（它的会话记录格式没有公开），这是 Cursor 不准的直接原因。

## 五步

1. **hook 把会话交给 keepane（0.74.0）。** agent 的 hook（`keepane setup` 装的 `SessionStart` 和回合结束）在 stdin 上给出 JSON，里面有会话 id、会话记录的路径、模型。`pane-ready` 原来不读 stdin，这些都丢了；会话记录只能按"同一目录、启动时间最近"去猜，会猜错（同目录的 `codex exec`、文件创建时间不准），也会找不到（`codex resume` 续写一周前的文件，Codex 按日期分目录，keepane 只往回看 7 天）。
2. **屏幕和标题补上"等人"，Cursor 补上"在忙"（0.74.0）。** Codex 不把审批和提问写进会话记录；Cursor 没有审批 hook，它的回合结束 hook 据报不稳定。照 herdr 的规则读屏幕。
3. **Codex 的数字（0.75.0）。** 读 `token_count.rate_limits`（主、副限额的已用百分比和重置时间），像 Claude 的余额一样显示；上下文用 Codex 自己的算法 `(total − 12000) / (window − 12000)`；花费按相邻两次累计的差值，记到当时的模型（现在把整个会话的累计按当前模型算）。
4. **Cursor 的 hooks。** `keepane setup cursor` 多装几个 hook：`beforeSubmitPrompt`（在忙）、`stop` 读它的 `status`、工具前后的 hook 进 Trace；公共字段里的模型。tokens 和花费 hook 里没有，不做。
5. **pi 的扩展。** 状态从两秒一次的轮询变成即时。

原方案第 4 步说要加 `keepane hooks install`。实现时发现 `keepane setup <agent>` 已经是"先显示要写什么，`--install` 才写"，Cursor 的 `sessionStart`、`stop` 和 Codex 的 `PermissionRequest` 也早就在装：不加新命令，第 4 步扩展 `setup cursor`。

## 第 1 步：hook 交出会话

- 在 pane 里、stdin 不是终端、命令行只有 `-q` 时，`pane-ready` 读 stdin（最多 1 秒、64 KB；hook 在每个回合末尾跑，agent 等它，所以比 `pane-wait` 的 2 秒短），取 `session_id`（Cursor 叫 `conversation_id`）、`transcript_path`、`model`（字符串，或带 `id` 的对象），作为 `--session`、`--transcript`、`--model` 交给 server。带控制字符或超过 4096 字节的值不取。手工运行（stdin 是终端）什么都不读、不等。
- `-t` 是"把别的 pane 标成空闲"，不接受这三个参数（会话只能是这个 pane 自己的 agent 的）。
- server 记下"pane、到达时间、会话记录、模型"，交给下一次查看（并让下一次查看提前）。watcher 只把它交给到达时已经启动的那个 agent（容差 2 秒）：之后才启动的是另一个 agent，不算。pane 的 agent 还没被看到时，最多等 30 秒。
- 一个 agent 一旦有 hook 指定的会话记录，它的会话记录就只是这些（加上 Claude 子 agent 的），不再猜；别的 agent 也不会分到这些文件。`/clear` 之类开了新会话时，新的 `SessionStart` 再指定一个，累加。
- hook 只给会话 id、不给路径时，按 id 去 agent 放会话记录的地方找：Claude Code 是 `<projects>/<目录名>/<id>.jsonl`，Codex 是 `sessions/年/月/日/rollout-…-<id>.jsonl`（从最新的日期找起）。id 只能是字母、数字、`-`、`_`，否则不找（防止 id 里带路径）。
- hook 给的模型先用着，直到会话记录里有模型（Codex 第一次提问后才写；Cursor 的会话记录不读）。有了模型，价格表里的窗口大小也就有了。
- 不改的：没有 hook 的 agent 照旧按目录和时间猜；会话记录的读法不变。

## 第 2 步：屏幕规则

- 每秒看一次跑着 Codex 或 Cursor 的 pane：终端标题，和屏幕最后 12 行有字的行（规则来自 herdr 的 `src/detect/manifests`）。
  - Codex 等人：标题含 `Action Required`；或最后几行含 `press enter to confirm or esc to cancel`、`enter to submit answer`、`allow command?`（不分大小写）。
  - Cursor 等人：最后几行含 `waiting for approval`、`run this command?`。
  - Cursor 在忙：最后 6 行含 `ctrl+c to stop`。
- 等人：和 hook 的 `pane-wait` 一样设置等待（Trace、时钟下的"needs you"、`done-events` 的 `wait`，只通知一次）。同一个问题 hook 和屏幕都会报（Codex 装了 `PermissionRequest` hook）：屏幕看到时，30 秒内 hook 已经设过等待，就沿用、不再通知；屏幕已经在显示时 hook 又来，换成 hook 的说法（更清楚），也不再通知。屏幕连续 2 次不再显示时清掉。屏幕设的等待在屏幕显示期间一直有效，不受 Trace 有新动作的影响（Codex 会先把要审批的命令写进会话记录）；hook 设的等待照旧，Trace 有新动作就算结束。
- Cursor 在忙：看到 `ctrl+c to stop` 时，空闲的 `ai` pane 记为在忙；之后连续 2 次看不到，算回合结束（同 stop hook：`agent_ready`）。只有屏幕看到过"在忙"才会因屏幕变空闲，所以刚按回车、Cursor 还没画出来时不会误判为空闲。屏幕只结束它看到的那个回合：最后一次看到在忙之后，若有消息送进去或有人按键（都会记下 pane 的最后输入时间），就不结束，因为那是 stop hook 已经结束了旧回合、新回合还没画出来。
- Codex 的忙和闲不看屏幕：会话记录更准（herdr 调研的结论 6）。Claude 不看屏幕：它的 hook 报等人。
- 每秒一次是 server 已有的节拍；为此另加一个更快的节拍不值得。所以等人在 1 秒内出现，结束和 Cursor 变空闲最多晚 2 秒。

## 验收要点

- 第 1 步：hook 指定的会话记录在猜不到的地方也读到；同目录更新的文件不被算进来；模型先显示 hook 给的；手工运行不等待；`-t` 加这几个参数被拒；之后才启动的 agent 不拿旧的指定。
- 第 2 步：Codex、Cursor 的提问出现时等人、只通知一次、消失后结束；Cursor 显示 `ctrl+c to stop` 后消失，pane 回到空闲；别的 agent 不受影响；屏幕上更早的提问（不在最后几行）不算。
