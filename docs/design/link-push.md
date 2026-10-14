# 把提交同步到另一台机器（设计稿）

状态：已实现（2026-10-14，验收记录见 `docs/acceptance.md` 第 132 条）。用户已定：单向；只同步已提交的内容；用 `clone`、`push` 两个命令；对方默认目录 `~/keepane/<仓库名>`；只推当前分支；第 8 节三条都按推荐做（默认只记一笔、`--tell` 才发消息；加 MCP 工具 `push_repo`；命令放在顶层，`keepane link clone/push` 也认）。

## 1. 目标与非目标

**目标**

- 在本机的仓库里运行一个命令，就能把提交送到已配对的另一台机器（[link.md](link.md)）的指定目录。局域网和 Tailscale 都能用。
- 第一次用 `clone` 在对方建副本，并定下位置；以后用 `push` 只送新提交，不再写位置。
- 对方不需要开 SSH，也不需要 git 服务；只需要装了 git、开着 `keepane web`。

**非目标（第一版不做）**

- 双向同步、从对方拉取（`pull`）。
- 未提交的改动。本机有未提交的改动时，只提示，不传。
- 一次推送所有分支（`--all`）、标签。
- Git LFS 的大文件内容：只送指针文件（和仓库里记的一样），内容不送。
- 浅克隆仓库：本机直接拒绝（它没有早期历史，送过去对方的历史不完整），提示先 `git fetch --unshallow`。
- dashboard 和手机页面上的入口。

## 2. 命令

```
keepane clone <对方地址> [路径] [--tell pane]     # 在对方建副本，记下位置
keepane push [<对方地址> [路径]] [--tell pane]    # 送新提交
keepane push -l                     # 列出本机仓库记下的位置
```

- 在仓库里运行（按命令行的当前目录找仓库，`git::find`）。不在仓库里时报错。
- `<对方地址>` 是 link 的写法 `host:port`（例如 `desktop.tailnet:8765`）。link 没有机器别名；记下的位置按对方的**公钥**保存，对方换了地址（link 会更新它的联系地址）也照样找得到。
- `[路径]` 是对方机器上的路径，由对方解释：`~` 是对方的家目录；相对路径从对方的默认目录（选项 `link-clone-dir`，默认 `~/keepane`）算起。
- `clone` 不写路径时，用对方的 `<默认目录>/<本机仓库目录名>`。
- `push` 的三种写法：
  1. `keepane push`：本机这个仓库只记了一个位置时，推到那里；记了多个时报错，列出它们。
  2. `keepane push <对方地址>`：推到对这台机器记下的位置；没有记录时报错，提示先 `clone` 或写明路径。
  3. `keepane push <对方地址> <路径>`：推到对方一个已有的仓库，并记下这个位置（用于对方已经有这个项目的情况）。
- 记录：本机仓库（规范化路径，`Repo::key`）× 对方公钥 → 对方路径。每台机器一条；对同一台机器 `clone` 到新路径时，记录改成新路径，旧副本不删。

## 3. 权限

- 对方要授予：`keepane link allow <本机地址> --push`。默认关。和 `--shell`、`--screen`、`--panes` 一样，只能在 pane 之外授予（pane 里的 agent 不能给自己开权限）。
- 没有授权时，对方返回 403，本机照录对方的话（`<本机地址> may not send commits here: on this machine, keepane link allow <本机地址> --push`）。
- `web --read-only` 的机器拒收，与其他 link 请求相同。
- 能写到哪里：对方当前用户能写的任何路径。理由：`--push` 是逐台机器显式授予的；写入的只是 git 提交和检出的文件，不运行这台机器送来的任何东西（git 的 bundle 不带钩子；对方仓库自己的钩子见第 6 节）。两条限制：
  - `clone` 的目标必须不存在，或是空目录。
  - `push` 到一个**还没记录**的已有仓库时，先比较两边的根提交（第一个提交）：一个都不相同就拒绝（防止推错项目）。对方仓库还没有提交时允许。

## 4. 传输

现有 link 通道的限制：请求正文是文本（JSON），上限约 1 MiB；答复上限 4 MiB。所以 bundle 分块用 base64 传：

1. **问（`POST /link/git/look`）**：本机发 `{mode, path, name, branch}`（`name` 是本机仓库目录名，路径为空时用）。对方答：
   - 路径上有没有仓库；有的话：当前分支、各分支和远程跟踪分支的提交、根提交、工作区是否干净。
   - `clone` 时：路径能不能用（不存在或空目录）。
2. **打包（本机，在线程里）**：`git bundle create <临时文件> <分支> --not <对方已有、本机也有的提交>`。对方的提交本机没有时（对方另有新提交）不能作为排除项，跳过；bundle 照样能在对方应用，只是大一些。对方已经有全部提交时，不传，直接告诉你"已是最新"。
3. **上传（`POST /link/git/chunk`，多次）**：每块原始 512 KiB（base64 后约 683 KiB，在 1 MiB 以内），带 `{transfer, index, data}`。每块都单独签名，与其他 link 请求一样。对方写进它的 link 目录下的 `incoming/<transfer>.bundle`。
4. **应用（`POST /link/git/apply`）**：本机发 `{transfer, sha256, size, mode: clone|push, path, branch, from}`。对方核对大小和 SHA-256，再应用（第 5 节；bundle 缺对方没有的前置提交时，git 自己会拒绝），在线程里做，完成后答复。这一步本机最多等 200 秒（对方每条 git 命令限时 120 秒）。

- 大小上限：一次 200 MiB（选项 `link-push-limit`），在本机打包后、上传前检查，对方收块时也检查。
- 同一台机器同一时间只能有一次传输；未完成的 `incoming` 文件 10 分钟后删掉；应用完（成功或失败）立即删掉。
- 对方的 keepane 太旧（不认这些路径，返回 404）：报"对方的 keepane 版本太旧，不支持 push"。

## 5. 对方怎么落地

`from` 是本机主机名，整理成合法的 git 引用名（例如 `laptop`）。下面用 `<来源>` 指它。

**clone**

1. `git clone <bundle> <路径> -b <分支>`。
2. 把远程 `origin` 改名为 `<来源>`，地址改成说明性的 `keepane:<本机地址>`（bundle 文件会被删掉）。
3. 结果：对方有一个普通的 git 仓库，检出 `<分支>`，`<来源>/<分支>` 指向同一个提交。

**push**

1. `git fetch <bundle> +<分支>:refs/remotes/<来源>/<分支>`。这一步只写远程跟踪分支，不碰工作区。
2. 然后按对方的状态处理 `<分支>`：
   - 它是当前检出的分支、能快进、工作区没有已跟踪文件的改动：`git merge --ff-only <来源>/<分支>`，分支和文件一起更新。
   - 它不是当前分支、能快进：只更新这个分支。
   - 不能快进（对方有自己的提交），或工作区有改动：不动 `<分支>`，答复里说明原因，提交留在 `<来源>/<分支>`，由那边自己 `merge` 或 `rebase`。
   - 对方没有这个分支：新建它，指向 `<来源>/<分支>`。
3. 本机打印结果（对方地址、对方路径、结果），例如：`100.64.0.3:7681:/home/u/keepane/proj: main 3 commits, fast-forwarded (files too)`，或 `…: 3 commits kept in laptop/main; this side has 2 commit(s) of its own on main: merge or rebase laptop/main there`。

**通知**（第 8 节第 1 条）：对方在 `show-messages` 和事件日志里记一笔（`link: commits from <本机地址>: <结果>`）；本机加了 `--tell <pane>` 时，送完后再给对方那个 pane 发一条普通的跨机器消息（`commits from <本机名>: <结果>`）。

## 6. git 的运行方式

- 本机和对方都用 `git-command` 选项找到的 git。没有 git 时报错。
- 读的命令照旧用 `git::run`（只读、限时、不加锁、不启动仓库配置的程序）。
- 写的命令（`bundle create`、`clone`、`fetch`、`merge`、`remote`、`update-ref`）用新的 `gitsync::git`：
  - 限时 120 秒；保留 stderr，失败时把 git 的原话带回本机。
  - 同样关掉 `core.fsmonitor` 和分页器，不提示输入（这些写操作不跑 diff）。
  - `merge --ff-only` 会运行对方仓库自己的钩子（例如 `post-merge`）：那是对方自己仓库里的配置，与对方用户自己运行 `git merge` 时相同，不另外禁止。
- 都在线程里运行，服务端不卡住。

## 7. 改动范围

- `src/gitsync.rs`：`git`（运行 git）、`look`、`bundle`、`clone`、`take`（push 的落地）、`roots`，以及路径和名字的整理；`src/git.rs`：`Repo::on_branch`、`real`（规范化路径）。`src/server/link_git.rs`：本机的推送任务和对方的三个请求。
- `src/link.rs`：`Peer` 加 `push` 授权；授权表加一列；三种新请求的正文；推送记录表（link 目录下的 `pushes` 文件：`本机仓库 对方公钥 对方路径`，一行一条，只给本用户读写）。
- `src/server/link_out.rs`、`link_in.rs`：发出和收进三种请求，分块，等待答复。
- `src/command.rs`、`main.rs`、补全：`clone`、`push`、`link allow --push`。
- 选项：`link-clone-dir`（默认 `~/keepane`）、`link-push-limit`（默认 200 MiB）。
- 文档：README 中英、man page、FAQ、[link.md](link.md)。
- MCP（第 8 节第 2 条）：`push_repo` 工具（`host`、`path`、`tell`）。

## 8. 曾经待定（2026-10-14 用户：都按推荐）

1. 推送完成后，对方要不要往某个 pane 发消息？推荐：默认只在对方的 `show-messages` 和事件日志里记一笔；加选项 `--tell <pane>`，指定时再给那个 pane 发一条消息。理由：对方 `ai` 模式的 pane 收到消息会直接交给 agent，默认发会在你不知道的情况下打断那边的 agent。
2. 要不要给 agent 用的 MCP 工具（`push_repo`）？推荐第一版就加：让本机的 agent 做完工作后可以自己推过去；权限仍由对方的 `--push` 控制。
3. 命令放在顶层（`keepane clone`、`keepane push`），还是放在 link 下面（`keepane link clone`、`keepane link push`）？推荐顶层，按你说的命令名；`keepane link push` 也认，作为同一个命令。

## 9. 测试

- 单元测试：授权表读写（含新列、旧格式照样读）、推送记录表、`<来源>` 名字和路径的整理、分块（顺序、上限、一次一个、过期）、SHA-256 不符不收、浅克隆被拒、各种落地判断（快进、已是最新、对方有自己的提交、工作区有改动、新文件不算改动、不是当前分支时前移、新分支、对方是空仓库）。
- e2e（同一个测试里起两个隔离的服务端，沿用 link 的测试做法）：
  - 未授权被拒；`clone` 到默认目录和指定路径；目标非空被拒。
  - `push` 已是最新、快进（文件一起）、对方有自己的提交（留在远程跟踪分支）；工作区有改动、新分支由单元测试覆盖。
  - 推到没有记录的已有仓库：根提交相同时成功，不同时被拒。
  - 超过大小上限被拒；超过 1 MiB 的 bundle（多块）完整到达。
  - 对方地址变了以后，按公钥照样推到原来的位置。
