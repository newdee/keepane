# 在飞书里回复 pane（设计稿）

状态：已实现（2026-11-09，验收记录见 `docs/acceptance.md` 第 175 条）。用户已定（"按推荐来，改吧"）：回复 `wait` 通知是立刻按键，回复其他通知走消息排队；keepane 内置 `rustls` 和 `tokio-tungstenite`；这一版只做文字和固定命令，卡片按钮下一版再做。

## 1. 目标与非目标

**目标**

- 不在电脑前时，在手机飞书上回答 agent：批准权限、选选项、布置下一步。
- 通知能对应到具体的 pane，回复能找到那个 pane，不用记 `%3` 这种地址。
- 不对外开端口，不需要公网地址。

**非目标（这一版不做）**

- 卡片按钮（"批准、拒绝、看屏幕"）。
- 群聊。群里任何人都能操作这台电脑，所以群消息一律忽略。
- 多台电脑共用一个应用（第 7 节）。
- 加密事件（Encrypt Key）。收到加密事件时，状态栏提示去控制台清掉 Encrypt Key。
- 富文本、图片、文件。

## 2. 已确认的事实

查了飞书官方文档和官方 SDK（Python `lark-oapi` 的 `ws` 客户端）的源码：

- 收消息用"长连接"：客户端先 `POST {域名}/callback/ws/endpoint`，body 是 `{"AppID","AppSecret"}`，拿到 `wss` 地址（带 `service_id`）和 `ClientConfig`（`PingInterval`、`ReconnectInterval`、`ReconnectNonce`，单位秒）。只支持企业自建应用。
- 帧是 protobuf（`pbbp2`）：`Frame{SeqID=1, LogID=2, service=3, method=4, headers=5, payload_encoding=6, payload_type=7, payload=8, LogIDNew=9}`，`Header{key=1, value=2}`。`method` 0 是控制帧（ping、pong），1 是数据帧（事件）。
- 客户端每 `PingInterval` 秒发一个 ping（`headers` 里 `type=ping`，`service` 是地址里的 `service_id`）。pong 的 payload 可能带新的 `ClientConfig`。
- 大事件分片发：`headers` 里 `sum`、`seq`、`message_id`。收齐再处理；只有最后一片要回。
- 每个事件要在 3 秒内回：原帧写回，payload 换成 `{"code":200,"headers":null,"data":null}`，`headers` 加 `biz_rt`（毫秒）。不回飞书会重发。
- 同一个应用有多个连接时，每个事件只随机发给其中一个，最多 50 个连接。
- 在控制台保存"长连接"设置时，客户端必须已经连上。
- 握手被拒时，HTTP 头里有 `handshake-status`、`handshake-msg`。

用真实飞书验证过（`server::feishu::tests::real_feishu_refuses_a_made_up_app`，手动运行）：编造的 app 去 endpoint，飞书回 `1000040346 app_id is invalid`；rustls 连到 `msg-frontier.feishu.cn`，握手被拒，头里是 `411 invalid app id`。所以 endpoint 路径、JSON 形状、TLS 栈、握手错误的读法都对。用真实应用收发消息，要用户建好应用后再测。

## 3. 结构

拆成 4 块，每块能单独测：

| 块 | 位置 | 内容 |
|---|---|---|
| 协议 | `src/feishu.rs` | 帧编解码（手写，约 150 行）、分片重组、事件解析、聊天命令、按键别名、请求和应答的 JSON。不依赖 server。 |
| 连接 | `src/server/feishu.rs` 的 `keep_connected`、`send_loop` | 两个 tokio 任务。连接任务：endpoint、WebSocket、ping、先回再交给 server、断线重连（2、5、15、30、60、120 秒）。握手限 30 秒；超过 2 个 ping 间隔加 30 秒什么都没收到，算静默断线，重连。发送任务：一次发一条，token 缓存，token 失效（99991661/63/68）重取一次。 |
| 路由与权限 | `src/server/feishu.rs` 的 `impl Server` | 白名单、去重、过期、回复找 pane、命令、审计。 |
| 配置与命令 | `config.rs`、`command.rs`、`main.rs` | 4 个选项、3 个命令。 |

HTTP（endpoint、token、发消息）走 `curl`，和 `done-webhook` 一样：地址、密钥、token 都写在 curl 的标准输入上（`-K -`），不出现在进程列表里。不加 `fail`：飞书拒绝时回 4xx 加 JSON，要读到它的错误码。

WebSocket 和 TLS：`tokio-tungstenite`（`rustls-tls-webpki-roots`）加 `rustls`（只开 `ring`）。不用 OpenSSL，musl 也能编译；根证书内置（Mozilla 的）。代价是体积：Windows 的 release 版 `keepane.exe` 从 11,985,408 字节增加到 14,075,904 字节，多 2.09 MB（17%），在规划时估的 1 到 2 MB 的上限。

## 4. 通知与回复

- 配置了应用、`feishu-notify on`（默认）时，每次"完成"（`done-events` 过滤之后）也由应用私聊发给白名单里的每个人。原来的 `done-webhook` 不变，可以并存。
- 发出的消息 ID 记下对应的 pane（最多 500 条）。`wait` 通知还记下是哪一次（`Done.id`）。
- pane 的"等待"：`wait` 通知时记下时间。以下任一情况算结束：
  - 这个 pane 的 agent 一轮结束、任务结束、程序退出（`Kind::Agent`、`Task`、`Exit`）；
  - 之后有人往 pane 里输入过（`actor.last_input` 晚于等待时间）；
  - `ai` 模式的 agent 已经空闲；
  - 飞书这边按过键。
- 回复 `wait` 通知：
  - 等待还没结束，而且是这个 pane 最新的一次：按键或打字。全是按键名（单个字符，或 `esc`、`enter`、`up`、`down`、`tab`、`space`、`bs`、`ctrl-c`……）就按这些键；否则把整段文字作为粘贴打进去，等粘贴稳定后按回车（和发给 agent 的消息同一套 `enter_step`）。
  - 有更新的等待：不按，回复"去回答那条"。
  - 等待已结束：按键不按；文字作为消息发给 pane，并说明。
- 回复其他通知，或者 `/to`：作为消息（`Sender::Remote{addr:"feishu", address:"user"}`）进 pane 的收件箱，和 `link` 收到的消息一样走 `enqueue`。回复里带上 `stand`（送达、排队），`normal` 模式的 pane 还提示 `set-work-mode ai`。
- agent 对这条消息 `send-message -r`：`mail.rs` 看到发送者是 `feishu`，转给 `feishu_answer_message`，作为回复发到用户原来那条消息下面。消息编号到用户的对应表最多 500 条，server 重启后清空（这时 `-r` 报错说明）。
- keepane 自己的每条回复也记下 pane，所以对回复再回复，还是同一个 pane。

## 5. 安全

- 默认关闭：`feishu-app-id` 和 `feishu-app-secret` 都设了才连。
- 移出白名单立即生效：之后的消息不处理，通知不发，agent 对他以前消息的 `-r` 回答也不发（报错说明）。
- 白名单：`link` 目录下的 `feishu-users`，一行一个 `ou_… shell=no screen=no`，文件只有当前用户能读。`feishu-allow` 只能在 pane 外运行（和 `link-allow`、`link-share` 同一个检查），agent 不能给别人开权限。`feishu-remove` 哪里都能运行（收回权限总是安全的）。
- 不在白名单的人：机器人回他自己的 open_id 和授权命令，状态栏提示一次；同一个人 10 分钟内只回一次，最多记 100 人。
- 只收单聊（`chat_type == "p2p"`），只收 `sender_type == "user"`。
- 能做什么，和 `link` 一致：
  - 消息：`ai`、`normal` 模式的 pane 随时可以；`shell` 模式的 pane 收到消息会执行，要 `--shell`。
  - 按键：pane 里有 agent 时可以。有没有 agent：`list-agents` 看到了，或者 pane 还在等 agent 的提问、而且 shell 没有回到提示符。没有 agent 时按键会进 shell，要 `--shell`。
  - `/screen`：会把终端内容传到飞书服务器，要 `--screen`。注意每条通知本来就带 pane 最后几行（`done-lines`，默认 5 行），和 `done-webhook` 一样；不想带就 `set -g done-lines 0`。`--screen` 管的是"随时看任意 pane 的整屏"。
- 去重：按飞书消息 ID，记最近 1000 条。飞书重发同一事件只处理一次。
- 过期：消息发出超过 10 分钟才到（keepane 断线期间发的），不处理，记日志。避免几小时后按下一个早就不相关的键。
- 密钥：`show-options` 对 `feishu-app-secret` 和 `done-webhook-secret` 只显示 `(set)`。把 `(set)` 设回去，原值不变，所以 `show-options` 的输出仍然能当配置读回。
- 审计：每条收到的消息、做了什么，都写进事件日志，类型 `feishu`，`what` 是 `refused`、`group`、`stale`、`bad`、`panes`、`message`、`rejected`、`keys`、`not-pressed`、`gone`、`screen`、`refused-shell`、`refused-screen`、`answer`、`sent`、`bad-frame`、`allowed`、`removed`、`state`、`failed`、`started`、`stopped`。

## 6. 选项与命令

```
set -g feishu-app-id cli_…
set -g feishu-app-secret …        # show-options 只显示 (set)
set -g feishu-domain feishu       # lark；或者一个 http(s) 地址（测试用）
set -g feishu-notify on

keepane feishu                    # = feishu-status
keepane feishu allow ou_… [--shell|--no-shell] [--screen|--no-screen]
keepane feishu remove ou_…
```

选项改了马上生效：`set` 之后 `feishu_sync` 比较 `(域名, id, secret)`，变了就停掉旧连接（两个任务 abort）、开新的；清空任一个就停。

聊天命令：`/panes`（`/ls`）、`/to <pane> <文字>`、`/key <pane> <按键或文字>`、`/screen <pane>`、`/help`。pane 写法和 `-t` 一样：`%3`、`%名字`、`session:window.pane`。其他不带 `/` 的文字：回复用法提示。

## 7. 多台电脑

飞书把事件随机发给一个连接，所以两台电脑用同一个应用，消息会随机落到其中一台，回复也可能落错。第一版的规则是一台电脑一个应用。以后可以让一台电脑接飞书，再通过 `link` 转给别的电脑。

## 8. 测试

- 单元测试（`src/feishu.rs`）：帧往返、ping 的字节和 SDK 一致、未知字段跳过、截断帧拒绝、分片乱序重组、endpoint/token/发送的请求和应答、事件解析（去掉 @、图片消息、加密事件、其他事件）、聊天命令、按键别名。
- 单元测试（`src/server/feishu.rs`）：白名单文件往返、curl 配置、去重和 pane 对应表的上限、token 失效识别、静默断线的判定、换配置后只认当前连接的状态。
- e2e（`a_feishu_app_tells_the_panes_and_takes_answers`）：测试里起一个假飞书（HTTP 加 WebSocket）。覆盖：密钥不显示、endpoint 带 id 和 secret、ping 带 service、事件在帧里回 200、陌生人收到授权命令、pane 里授权被拒、`wait` 通知发给白名单、没有 agent 时按键要 `--shell`、回复 `wait` 打字并回车（shell 执行了）、已答的提问不按键、重发的事件只处理一次、`/panes`、发给 shell pane 被拒、发给 agent 送达、agent 的 `-r` 回到飞书、`/screen` 要 `--screen`、只发 @ 的回复什么都不按、按键文字超过 `message-max-size` 被拒、两次提问时回复旧的不按键、有人在终端输入过后回复不按键、群聊和过期消息不处理、图片提示只收文字、`feishu-notify off` 不发通知、状态和事件日志、移出白名单后 agent 的回答不发、清空密钥后断开。变异测试：`waiting_on`、`has_agent`、去重、`feishu-notify`、过期、群聊 6 处改坏都被抓到。
- 真实飞书（手动，需要网络）：见第 2 节。
