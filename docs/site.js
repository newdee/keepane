// Language toggle for the keepane tour. Every translatable node carries
// data-i18n; the English text lives in the HTML, the Chinese here.
const ZH = {
  "nav.tour": "功能",
  "nav.faq": "常见问题",
  "nav.download": "下载",
  "hero.eyebrow": "开源",
  "hero.title": "<span class=\"nw\">pane 一直在跑，</span><span class=\"nw\">还能互相传消息。</span>",
  "hero.lede":
    "keepane 是一个终端多路复用器。关闭终端连接后，pane 里的程序继续运行；pane 之间还能通过收件箱传递消息，同一台电脑上可以，配过对的两台电脑之间也可以。常用的 tmux 按键、命令和配置文件可以继续使用，h j k l 也在；PowerShell、cmd、WSL 都在 pane 里运行，按键原样传进去。",
  "hero.download": "下载",
  "hero.source": "看源码",
  "hero.meta": "MIT 许可 · Windows、Linux、macOS · 单个可执行文件 · 原名 wmux",
  "hero.caption": "功能导览：pane 之间互发消息、三种工作模式，给 agent 用的 MCP，dashboard，以及在手机上看和操作 pane。",
  "stat.exe": "个文件就够",
  "stat.deps": "依赖 Cygwin / WSL",
  "stat.tests": "项自动化测试",
  "stat.license": "许可证",

  "c1.title": "分屏还是那套手感",
  "c1.sub":
    "<kbd>C-b %</kbd> 左右分，<kbd>C-b \"</kbd> 上下分。用 vim 键位或方向键移动，而且可以连着按：移动和调大小在半秒内不用再按前缀。",
  "c1.p1":
    "每个 pane 都是一个 ConPTY，所以 PSReadLine 的组合键、中文输入法和全屏程序，表现和不用 keepane 时一样。",
  "c1.p2": "鼠标可以用：点一下切换 pane，拖边框调大小，拖选文字直接复制到系统剪贴板。",
  "cw.title": "在手机上看 pane、往 pane 里输入",
  "cw.sub":
    "跑长编译之前先开 <code>keepane web</code>，然后人就可以走开。它会打出一个二维码，手机连同一个 Wi-Fi 扫一下，浏览器里就列出所有 pane，点进去能看屏幕、能输入。手机上什么都不用装。",
  "cw.p1":
    "屏幕一有变化，keepane 就把新内容推过来，颜色保留，也能往上翻看已经滚过去的输出。列表上带着每个窗口的提醒标记（<b>#</b> 有输出，<b>!</b> 响铃，<b>~</b> 没动静），任务跑完了不用点进去也看得到。在输入框里打的字边打边发到 pane，回车就是回车（也可以在视图菜单里改成等回车再发）。",
  "cw.p2":
    "输入框上方有一排手机键盘上没有的键：Ctrl、Alt、Shift（按住给下一个键用，先 Shift 再 → 就是 Shift+→）、Esc、Tab、方向键，Ctrl+C 和回车固定在这一排最右边；Shift+Tab、y、n、1、2、3 等收在输入框旁边的 ⋯ 里。pane 顶栏的 ⋯ 菜单，或者在列表里右键（手机上长按）一个 pane，可以分屏、开新窗口、重命名、看收件箱或关掉 pane。在 shell 或 ai 模式的 pane 里，输入框旁的信封按钮把内容作为消息送进它的收件箱排队。",
  "cw.p5":
    "每个 pane 的名字下面：一个表示空闲或忙的小圆点、收件箱里排着几条消息、它最后输出的一行。视图菜单里的“适配这块屏幕”把 pane 调成手机屏幕的大小，方便看全屏程序，离开时自动恢复。同一个菜单还能在输出里查找、复制屏幕文字、调整字号（也可以双指缩放）；点一条命令的时间可以复制它的输出。",
  "cw.p6":
    "pane 完成时（长命令跑完、agent 这一轮结束）页面会提醒你：顶部横幅（点一下跳过去）、响一声、震动。手机锁屏也要收到的话，keepane 可以推到 ntfy、飞书、企业微信、钉钉、Slack、Discord（<code>done-webhook</code>），或者运行你自己的 hook。",
  "cw.p7":
    "电脑的浏览器也能打开同一个页面：左边是列表，右边是 pane。页面有自己的日间和夜间样式，中英文跟随系统也能手动切换；也可以在页面上切换终端主题（Tokyo Night 或 Tokyo Day），电脑上的终端会跟着变。右上角显示到电脑的延迟；全屏时只留下屏幕和输入框。",
  "cw.p3": "命令在电脑上执行。手机只能看 pane、往里输入、用那个菜单，别的做不了；加 <code>--read-only</code> 就只能看。",
  "cw.p4":
    "不启动就不开；启动后由 keepane 的 server 在后台提供服务，直到 <code>keepane web stop</code>，<code>keepane web status</code> 可以看谁连着。二维码里的 128 位密钥每次启动都重新生成。用的是普通 HTTP，在家里的网络没问题；在外面用，中间接一层 Tailscale 之类的私有网络。",
  "c1.p3":
    "<kbd>C-b S</kbd> 或 <code>:set sync</code>（Tab 能补全选项名）会把每个按键同时发给窗口里的所有 pane。<code>split-window -N 3</code> 一次加三个 pane 并平铺。",

  "c2.title": "铺满屏幕、从列表里挑，或者弹个菜单",
  "c2.sub":
    "<kbd>C-b z</kbd> 把当前 pane 放大到整个窗口，再按一次还原。<kbd>C-b w</kbd> 弹出所有 session、窗口和 pane 的结构图：<kbd>h</kbd> <kbd>j</kbd> <kbd>k</kbd> <kbd>l</kbd> 移动，<kbd>a</kbd> 一次展开全部，<kbd>v</kbd> 切到树或纯文字列表，数字直接跳，<kbd>Enter</kbd> 进去。<kbd>C-b &gt;</kbd> 把 pane 相关的命令放进一个菜单，不用记快捷键。",

  "c2.p1":
    "<kbd>C-b Space</kbd> 在五种布局之间切换：等宽列、等高行、主 pane 在左或在上，还有平铺。<kbd>C-b E</kbd> 把当前 pane 旁边那一排调成一样大。",
  "c2.p6":
    "记不清某个键？按下 <kbd>C-b</kbd> 后停一下：半秒后弹出提示面板，按组列出每个键做什么，读的是你自己的按键绑定。接着按的键照常生效。",
  "c2.p2": "<kbd>C-b q</kbd> 在每个 pane 上显示一个数字，按那个数字就跳过去。",
  "c2.p5":
    "放大时 pane 本身逐渐长大，四个顶点朝窗口的四个顶点靠拢，还原时缩回去；焦点换到别的 pane 或窗口时，会有一个框飞过去。程序只调整一次大小，不用等它；<code>set -g animation off</code> 可以关掉。放大的窗口里切 pane 会一直保持放大，直到按 <kbd>C-b z</kbd>。",
  "c2.p3":
    "copy mode 里 <kbd>/</kbd> 往前搜、<kbd>?</kbd> 往回搜，<kbd>n</kbd> <kbd>N</kbd> 重复上一次搜索，匹配的那一行会滚到屏幕中间。",
  "c2.p4":
    "<code>display-popup</code> 在窗口上开一个框运行程序，程序退出后框就关掉。临时看一眼 <code>git log</code> 时很方便，不用动布局。",

  "c3.title": "关掉终端，程序照常运行",
  "c3.sub": "<kbd>C-b d</kbd> 脱离，里面的程序照常跑。换个终端窗口敲 <code>keepane attach</code> 就接回来了。",
  "c3.p1":
    "重启电脑之后也能恢复。每个 session 的布局一有变化就存到它自己的文件里，<code>keepane resume</code> 会恢复窗口、分屏布局，以及每个 pane 的命令和目录。",
  "c3.p2":
    "文件里还存着每个 pane 输出过的内容：<code>save-history</code> 默认保留最后 500 行，设成 <code>all</code> 就保留全部回滚内容，颜色也在。恢复出来的 pane 显示的是原来的输出，而不是一个空提示符。",
  "c3.p3": "<code>keepane list-saved</code> 列出能恢复的 session，<code>keepane resume work</code> 只恢复这一个。",

  "c35.title": "你没看着的时候，那个任务跑完了",
  "c35.sub":
    "没在看的窗口有动静时会被标出来：<code>monitor-activity</code> 在有输出时标 <b>#</b>，响铃时标 <b>!</b>，太久没有输出时标 <b>~</b>。<kbd>C-b M-n</kbd> 跳到下一个带标记的窗口。",
  "c35.caption":
    "窗口 1 在跑部署，窗口 0 照常干活；跑完之后状态栏和 <code>list-windows</code> 同时出现标记；失败的命令把 pane 和退出码留在原地。",
  "c35.p1":
    "开了 <code>remain-on-exit</code> 之后，程序退出了 pane 也不会关，上面写着 <code>[cmd exited with 3]</code>。凌晨三点崩掉的任务，早上还能看到。<code>respawn-pane</code> 在原位置重新启动它。",
  "c35.p2":
    "<code>pipe-pane \"$input | Add-Content build.log\"</code> 把 pane 输出的所有内容交给一个命令；那个命令处理不过来时，keepane 会提示你。",
  "c35.p3":
    "脚本之间可以互相等待：<code>keepane wait-for ready</code> 会一直等，直到另一个客户端执行 <code>keepane wait-for -S ready</code>；<code>-L</code> 和 <code>-U</code> 可以当锁用。",

  "ch.title": "跑了什么、什么时候、花了多久",
  "ch.sub":
    "<kbd>C-b C-t</kbd> 把每条命令的开始时间、耗时和结果写在它那一行的末尾。pane 输出过的东西按天保存，<kbd>C-b /</kbd> 打开任意一天。",
  "ch.caption":
    "三条命令和它们的时间，其中一条失败；历史面板和查看器里的一天；手滑关掉的 pane，<kbd>C-b u</kbd> 找回来。",
  "ch.p1":
    "时间画在行尾的空白里。pane 宽度不变，程序输出的内容一个字不改。手机上在视图菜单里打开“每条命令的时间”，时间单独占一栏。",
  "ch.p2":
    "从 pane 顶上滚出去的内容写进这个 pane 当天的文件，留 30 天，每条命令前面有一行它的时间。查看器里 <kbd>[</kbd> <kbd>]</kbd> 在命令之间跳，<kbd>/</kbd> 搜索。",
  "ch.p3": "关错了 pane 或窗口？它会保留 10 秒，程序还在跑，按 <kbd>C-b u</kbd> 放回原处。",

  "cl.title": "另一台电脑上的 pane",
  "cl.sub":
    "同一个局域网、或者都连着 Tailscale 的两台电脑，像 <code>ssh-copy-id</code> 那样配对一次。之后一台上的 pane 就能给另一台上的 pane 发消息、派命令，回信会回到发消息的那个 pane。",
  "cl.p1":
    "pane 的地址写成 <code>主机:端口/%名字</code> 或 <code>主机:端口/$1:@3.%7</code>。回信按 pane 编号送回，发消息的 pane 被挪了位置也照样收得到。",
  "cl.p2":
    "每台机器有自己的密钥对。配对时用一次对方手机页的二维码密钥；之后每个请求和每个答复都带签名、时间和一次性的随机数，旧请求重放不了。认机器认的是密钥，不是地址。",
  "cl.p3":
    "别的机器发来的消息只进 agent 和 normal 模式的 pane。要让它在 shell 里执行、看某个 pane 的屏幕，或者在这边开 pane（<code>keepane link start</code>，只能开这台电脑 <code>agent-commands</code> 里的程序），得用 <code>keepane link allow</code> 按机器授权，而且只能在 keepane 外面做：pane 里的 agent 给不了自己这个权限。",
  "cl.p4":
    "agent 也能通过 MCP 用：<code>list_links</code>、<code>link_info</code>、<code>read_screen</code>，以及用 <code>send_message</code> 发到另一台机器上的地址。",

  "cm.title": "在 pane 之间派活",
  "cm.sub":
    "给 pane 起个名字、设个工作模式，就能给它发消息：shell 在提示符下执行，agent 在空闲时收到。<kbd>C-b v</kbd> 看所有 pane 和它们的收件箱。",
  "cm.caption":
    "发给名叫 builder 的 pane 的命令在那里执行，来源写在一段注释里；它的记录显示已完成，并保存了输出。然后是分面板的 dashboard：pane、一个 agent 的收件箱、任务，把一条消息置顶，右边排好版显示它的记录。",
  "cm.p1":
    "每条消息都用一行 JSON 写明来源。一串消息是一个任务，每一步等了多久、做了多久都有记录；来回超过八手的会被拒收。",
  "cm.p2":
    "Claude Code 这样的 agent 通过 MCP 使用它：发消息和回信、在一轮之内等回信、自己开 pane 干活。<code>keepane setup</code> 为 Claude Code、Codex、Gemini CLI、Cursor CLI、opencode 配好 hook 和注册，agent 每轮结束时就会收到下一条消息；如果某个 agent 从没报告过空闲，发给它的消息会告诉你缺的是哪一项配置。",
  "cm.p3":
    "pane 的工作模式只能在那个 pane 里切换，所以任何 pane 里运行的程序都不能把别的 pane 变成收到什么就执行什么的 shell。发生过的一切记在事件日志里，保留 30 天。",
  "cm.p4":
    "pane 里跑的每个 agent（Claude Code、Codex、pi）都显示模型、花了多少、上下文占了多少、token、CPU 和内存：dashboard 里有，手机上它的卡片上有，<code>keepane list-agents</code> 和 <code>#{agent_cost}</code> 这类变量也有。keepane 读的是 agent 自己写的会话记录；花费按 LiteLLM 的公开价目表（API 标价）计算，也可以 <code>agent-cost off</code> 关掉。",

  "c4.title": "你的 .tmux.conf，导入一次就能用",
  "c4.sub":
    "命令行、: 命令提示符、配置文件里是同一套命令名。keepane import-config 把 tmux 配置里能用的导入 keepane 自己的配置，只导一次，用不了的注释掉并写明原因。",
  "c4.p0":
    "命令名可以只写不会混淆的前缀（<code>keepane att</code>、<code>keepane splitw -h</code>），选项名也一样：<code>set sync</code> 就是 <code>set synchronize-panes</code>，<code>set mon-act on</code> 就是 <code>set monitor-activity on</code>。开关类选项不给值就是切换。",
  "c4.p1":
    "<code>keepane send-keys</code>（包括 tmux 的 <code>-X</code> copy mode 命令）、<code>keepane capture-pane -p -e</code> 和 <code>keepane split-window</code> 可以在脚本里操作 session。在 pane 里运行时不用写 socket 名。",
  "c4.p2":
    "插件就是一个目录：放一个写着命令的 <code>&lt;名字&gt;.keepane</code> 文件，再加上任意语言的脚本，用 <code>set -g @plugin 名字</code> 加载。新开窗口、分屏、pane 退出和客户端接入时都会触发钩子。",

  "c6.title": "状态栏上有 git 分支、CPU 和内存",
  "c6.sub":
    "默认的状态栏显示当前 git 分支、pane 所在目录、CPU 和内存占用，有电池的话还有电量。这些都是 keepane 自己每秒读一次，不启动额外进程。整行可以换成自己的写法，也可以关掉。",
  "c6.p1":
    "<code>#{git_branch}</code>、<code>#{cpu_percentage}</code>、<code>#{ram_percentage}</code>、<code>#{battery_percentage}</code>、<code>#{pane_current_path_short}</code> 和 <code>#{pane_pid_command}</code>（pane 里此刻运行的程序）可以放在 <code>status-right</code> 的任意位置；<code>set -g status off</code> 隐藏整行。",
  "c6.p2":
    "两个终端以不同尺寸接入时，由 <code>window-size</code> 决定按哪个终端的尺寸来。较小的终端只看到窗口的一部分，用 <kbd>Shift</kbd> + 方向键平移。",
  "c6.p3":
    "右键粘贴。<kbd>:</kbd> 命令行里 <kbd>Tab</kbd> 可以补全命令、flag、目标和选项名，<code>keepane completion powershell</code>（或 <code>bash</code>、<code>zsh</code>、<code>fish</code>）让你的 shell 也能这样补全。",
  "c6.p4":
    "<code>keepane update</code> 安装新版本，<code>keepane restart-server</code> 把正在运行的 session 连同历史和目录一起迁过去，已经接入的终端会自动重新连接。",

  "c5.title": "安装",
  "c5.unix": "Linux 和 macOS",
  "c5.unix.sub": "每个系统一个 .tar.gz（Linux 版静态编译；Mac 分 Apple 芯片和 Intel），解压后把 keepane 放进 PATH。",
  "c5.msi": "Windows 安装包",
  "c5.msi.sub": "下载 .msi 双击，装到 Program Files，自动进系统 PATH，卸载在“应用和功能”里。",
  "c5.brew": "Homebrew",
  "c5.brew.sub": "macOS 和 Linux，装的是发布页上编译好的包；以后用 brew upgrade keepane 升级。",
  "c5.scoop": "Scoop",
  "c5.scoop.sub": "仓库里的清单直接装 zip，以后跟着新版本更新。",
  "c5.zip": "Windows 免安装",
  "c5.zip.sub": ".zip 里就是同一个 exe，解压到哪都能跑。",
  "c5.cargo": "从源码装",
  "c5.get": "去下载",

  "close.title": "下次跑长任务的时候试试。",
  "close.docs": "看文档",
  "footer.built": "用 Rust 写的，基于 ConPTY 和 Win32 控制台 API。",
};

const nodes = Array.from(document.querySelectorAll("[data-i18n]"));
const EN = new Map(nodes.map((n) => [n, n.innerHTML]));
// Pictures of a page that has words of its own come in both languages.
const pictures = Array.from(document.querySelectorAll("img[data-src-zh]"));
const EN_SRC = new Map(pictures.map((i) => [i, i.getAttribute("src")]));
// Links to a page that comes in both languages, the same way.
const links = Array.from(document.querySelectorAll("a[data-href-zh]"));
const EN_HREF = new Map(links.map((a) => [a, a.getAttribute("href")]));

function apply(lang) {
  for (const n of nodes) {
    const key = n.dataset.i18n;
    if (lang === "zh" && ZH[key]) n.innerHTML = ZH[key];
    else n.innerHTML = EN.get(n);
  }
  for (const i of pictures) i.setAttribute("src", lang === "zh" ? i.dataset.srcZh : EN_SRC.get(i));
  for (const a of links) a.setAttribute("href", lang === "zh" ? a.dataset.hrefZh : EN_HREF.get(a));
  document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  document.getElementById("lang").textContent = lang === "zh" ? "EN" : "中文";
  try {
    localStorage.setItem("keepane-lang", lang);
  } catch {
    /* private mode: the toggle still works, it just is not remembered */
  }
}

// Remembered choice wins; otherwise a Chinese browser starts in Chinese.
let lang = "en";
try {
  const saved = localStorage.getItem("keepane-lang");
  if (saved === "zh" || saved === "en") lang = saved;
  else if ((navigator.language || "").toLowerCase().startsWith("zh")) lang = "zh";
} catch {
  /* private mode */
}
apply(lang);

document.getElementById("lang").addEventListener("click", () => {
  lang = lang === "zh" ? "en" : "zh";
  apply(lang);
});
