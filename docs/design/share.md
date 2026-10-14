# 分享一个文件或文件夹（设计稿）

状态：已实现（2026-10-14，验收记录见 `docs/acceptance.md` 第 134 条）。用户已定（"都按推荐来"）：默认 24 小时有效；文件夹打成 zip；pane 里的 agent 也能分享，每次都在状态栏提示；web 没开时自动打开。

## 1. 目标与非目标

**目标**

- 一个命令把本机的一个文件或文件夹给出去：打印二维码和链接，手机扫码或别的机器点链接就直接下载。局域网和 Tailscale 都能用。
- 拿到链接的人只能下载这一个东西：看不到 pane，也拿不到别的文件。

**非目标（第一版不做）**

- 上传（从手机传到电脑）。
- 网页上的文件列表、预览、在线播放。
- 分享在 server 重启后保留。
- HTTPS。和 `keepane web` 一样是明文 HTTP；不信任的网络上请走 Tailscale。

## 2. 命令

```
keepane share <路径> [--for 时长] [--once]   # 分享，打印二维码和链接
keepane share [-l]                           # 列出正在分享的
keepane share stop <编号>|--all              # 取消
```

- `<路径>` 按命令行的当前目录解释，`~` 是家目录。必须是已有的文件或文件夹；路径本身是符号链接时跟随它。
- `--for`：`30m`、`2h`、`7d`、`90s`；`0` 表示一直有效，直到取消或 server 退出。默认 `24h`，最长 `365d`。
- `--once`：完整下载一次后失效。中途断开不算；用断点续传补完最后一段也算完整。
- 编号从 1 起，每个 server 递增。
- 服务端命令就叫 `share`（`keepane share` 直接发给它）；网页内部另用 `web-share-open`、`web-share-sent` 两个命令（第 4 节）。

## 3. 链接与权限

- 链接是 `http://<web 的地址>:<web 的端口>/s/<令牌>`。令牌每次分享单独生成，128 位随机数，和 web 的密钥无关。web 在多个地址上服务时（Tailscale），每个地址各打印一个链接；二维码是第一个。
- web 没开时，`share` 打开它并说明：按上次 `web-start` 记下的参数（[web 随恢复重开](../acceptance.md) 的那份记录），没有记录时用默认参数（同 `keepane web`）。这样打开的 web 不会在恢复 session 时再打开（不写记录）。
- 只读的 web（`--read-only`）也能分享：只读管的是 pane。
- pane 里也能运行 `share`（agent 可以把生成的东西发给你）。每次分享都在状态栏提示，写明从哪个 pane 发起。
- 令牌不对、已过期、已取消：都答 404 "this link has ended"，不区分。
- 正在进行的下载不受取消和过期影响，下完为止；之后的请求都是 404。

## 4. 下载

网页服务收到 `GET` 或 `HEAD /s/<令牌>`：

1. 问 server `web-share-open <令牌> <对方地址>`。server 答 `{"path", "dir", "name"}`，或报错（答 404）。同一个分享，每个新的对方地址在状态栏提示一次（"share 1: 192.168.1.20 is downloading report.pdf"）。
2. 回复头一律：`Content-Type: application/octet-stream`、`Content-Disposition: attachment`（文件名带 UTF-8 写法 `filename*`）、`Content-Security-Policy: sandbox`、`X-Content-Type-Options: nosniff`、`Cache-Control: no-store`。理由：分享出去的 HTML 文件和手机页同源，若被浏览器直接显示，它的脚本能读到手机页存下的密钥；这样它只会被下载。
3. **文件**：带 `Content-Length` 和 `Accept-Ranges: bytes`。认一个区间的 `Range`（`bytes=a-b`、`bytes=a-`、`bytes=-n`），答 206；超出文件答 416；多个区间或写法不对时忽略 `Range`，答整个文件。
4. **文件夹**：边读边写成 zip，文件名 `<文件夹名>.zip`，里面的路径都以 `<文件夹名>/` 开头。不压缩（STORED）。每个条目先写本地头，再写内容，再写数据描述符（CRC-32 和大小，通用标志位 3）；最后写中央目录。条目或偏移超过 4 GiB 时用 ZIP64。文件名用 UTF-8（标志位 11）。
   - 没有 `Content-Length`（读的时候文件可能变），连接关闭即结束。不支持 `Range`。
   - 跳过：符号链接（含 Windows 的 junction）、打不开的文件和文件夹。读到一半读不下去的文件（别的程序锁住了一段），条目在读到的地方结束。每个文件最多读到列目录时的大小。
5. 整个回复发完，问 server `web-share-sent <令牌> <对方地址> [说明]`：计数加 1；`--once` 的分享就此删除；状态栏提示（文件夹附上"N 个文件，跳过 M 个"）。`HEAD` 不算。

## 5. 改动范围

- `src/share.rs`：时长的解析和显示、CRC-32、zip 的各个头、`Range` 的解析、`Content-Disposition`、下载的流程（`serve`）。
- `src/server/share_host.rs`：分享表、`share`、`web-share-open`、`web-share-sent`，每秒检查过期。
- `src/web.rs`：`/s/` 交给 `share::serve`；请求多读一个 `Range` 头；二维码函数供 server 用。
- `src/command.rs`：三个命令；补全、帮助、man page、README 中英、FAQ。
- `Cargo.toml`：tokio 加 `fs` 特性（不引入新的 crate）。

## 6. 测试

- 单元：时长（各单位、`0`、超出、写错）；CRC-32 的标准值（`123456789` → `CBF43926`）；`Range` 的各种写法；文件名的 ASCII 替代和 UTF-8 编码；zip 的结构（自带一个最小读取器核对每个条目的 CRC、大小、偏移、中央目录、结尾记录），含强制 ZIP64 的版本；分享表（添加、列出、取消、过期、`--once`、令牌不对、web 没开时按上次的参数打开）。
- e2e（真实服务端，真实 HTTP）：下载文件内容逐字节相同；`Range` 206 和 416；`HEAD`；文件夹的 zip 用本机工具（Windows 的 `tar`，Linux 的 `python3 -m zipfile`）解开后与原目录逐个文件相同；`--once` 第二次 404；取消后 404；令牌不对 404；回复头里有 `attachment` 和 `sandbox`。
