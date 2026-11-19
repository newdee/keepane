// Language and light/dark toggles for the keepane tour. Every translatable
// node carries data-i18n; the English text lives in the HTML, the Chinese
// here. Every picture in img/ has a light twin, name-light.ext, taken in
// the paper theme; the HTML names the dark one (paper-dark).
const ZH = {
  "nav.tour": "功能",
  "nav.faq": "常见问题",
  "nav.download": "下载",
  "hero.eyebrow": "面向 AI Agent 的终端工作空间",
  "hero.title": "<span class=\"nw\">离开终端，</span><span class=\"nw\">工作还在继续。</span>",
  "hero.lede":
    "<p>keepane 是一个终端复用工具。它让终端中的程序持续运行，让不同 Pane 之间可以传递任务，也让 AI Agent 能够在同一个工作空间里协作。</p>" +
    "<p>关闭终端窗口，程序不会因此退出。一个 Pane 可以向另一个 Pane 发送命令，也可以把任务交给正在运行的 Agent。你可以从另一台电脑或手机上查看进度，继续之前的工作。</p>" +
    "<p>保留熟悉的 tmux 快捷键、命令和配置文件。Windows、Linux、macOS 均可使用。</p>",
  "hero.download": "下载",
  "hero.source": "GitHub",
  "hero.meta": "Rust · MIT License · 单文件安装",
  "hero.caption": "Pane 之间互发消息、Agent 通过 MCP 工作、dashboard，以及在手机上查看 Pane。",
  "stat.exe": "个文件就够",
  "stat.deps": "依赖 Cygwin / WSL",
  "stat.tests": "项自动化测试",
  "stat.license": "许可证",

  "cm.title": "Pane 之间可以传递任务",
  "cm.sub":
    "<p>传统终端复用工具主要解决的是窗口和 Pane 的组织问题。</p>" +
    "<p>keepane 进一步让 Pane 之间能够通信。</p>" +
    "<p>给每个 Pane 一个名字，就可以向它发送消息或任务。</p>",
  "cm.code": `<span class="c"># 给 3 号 Pane 起名 builder，让它在提示符下执行收到的命令</span>
keepane rename-pane -t %3 builder
keepane set-work-mode -t %builder shell

<span class="c"># 向这个 Pane 发送任务</span>
keepane send-message -t %builder "cargo test"

<span class="c"># 查看执行结果（#12 是 send-message 返回的编号）</span>
keepane trace-message 12 -w 600`,
  "cm.modes": "Pane 有不同的工作模式：",
  "cm.m1": "<b>normal</b>（默认）：消息留在 Inbox，由 Pane 里的程序用 <code>read-message</code> 读取。",
  "cm.m2": "<b>shell</b>：收到的消息直接在 Shell 中执行（等 Shell 回到提示符）。",
  "cm.m3": "<b>ai</b>：消息进入 Inbox，等 Agent 当前任务结束后再处理。",
  "cm.after":
    "<p>每条消息都会记录来源、状态和执行结果。多个消息还可以串成一条任务链，查看每一步的等待时间和执行耗时。</p>" +
    "<p>不需要复制粘贴终端输出，也不必一直盯着另一个窗口。</p>",

  "ca.title": "让 AI Agent 在同一个工作空间里协作",
  "ca.sub":
    "<p>keepane 可以通过 MCP 与 Claude Code、Codex、Gemini CLI、Cursor CLI、opencode、pi 等工具配合使用。</p>" +
    "<p>Agent 不再只是一个独立运行的命令行程序，而是可以访问其他 Pane、发送消息、等待回复，并创建自己的工作 Pane。</p>",
  "ca.eg": "例如：",
  "ca.e1": "一个 Agent 负责分析代码。",
  "ca.e2": "另一个 Agent 负责实现功能。",
  "ca.e3": "第三个 Agent 负责运行测试。",
  "ca.e4": "测试失败后，将输出发送给负责修复的 Agent。",
  "ca.split": "keepane 负责消息传递和任务状态记录，Agent 自己决定如何完成任务。",
  "ca.see": "查看 Agent 正在做什么",
  "ca.see.p": "不必只看终端输出。keepane 可以读取 Agent 的运行记录，显示：",
  "ca.s1": "当前使用的模型",
  "ca.s2": "Token 使用量",
  "ca.s3": "Context 使用率",
  "ca.s4": "累计费用",
  "ca.s5": "CPU 和内存占用",
  "ca.trace": "追踪 Agent 的执行过程",
  "ca.trace.p":
    "<p>Trace 可以把一次 Agent 执行过程展开成时间线。</p>" +
    "<p>模型调用、文件读取、代码修改、命令执行、浏览器操作和其他工具调用，都可以看到对应的执行顺序、耗时和结果。</p>" +
    "<p>如果某一步失败，还可以查看失败原因。</p>" +
    "<p>不只是知道 Agent 最后有没有完成任务，也能知道它是怎么完成的。</p>",

  "c3.title": "关闭终端，工作不会丢失",
  "c3.sub": "<p>一个长时间运行的任务，不应该因为终端窗口关闭而中断。</p>",
  "c3.code": `<span class="c"># 断开当前终端</span>
<span class="c"># Ctrl-b d</span>

<span class="c"># 重新连接</span>
keepane attach`,
  "c3.p":
    "<p>keepane 会保存 Session、Window 和 Pane 的布局，以及每个 Pane 对应的程序、工作目录和终端输出。</p>" +
    "<p>即使电脑重启，下一次启动 keepane，也可以恢复之前保存的工作空间。</p>" +
    "<p>默认保存最近的终端输出，也可以配置保存完整滚动历史。</p>" +
    "<p>误关 Pane 或 Window？在保留期限内还可以恢复。</p>" +
    "<p>对于编译、测试、部署和长时间运行的 Agent 任务，不必再担心终端窗口意外关闭。</p>",

  "cl.title": "两台电脑之间传递任务",
  "cl.sub":
    "<p>有时候工作不只发生在一台电脑上。</p>" +
    "<p>例如桌面电脑负责运行 Agent，笔记本负责查看结果；或者在两台开发机器之间分配任务。</p>",
  "cl.pair": "keepane 支持两台电脑配对。配对后，可以：",
  "cl.l1": "查看另一台电脑上共享的 Pane。",
  "cl.l2": "向远程 Pane 发送消息和命令。",
  "cl.l3": "获取远程终端输出。",
  "cl.l4": "在授权后直接连接远程 Session。",
  "cl.after":
    "<p>Agent 也可以通过 MCP 使用这些能力。</p>" +
    "<p>每台机器使用自己的密钥身份。配对之后，每台机器仍然可以决定具体共享哪些 Session、Window 和 Pane。</p>" +
    "<p>支持同一局域网，也可以通过 Tailscale 等私有网络连接。</p>",

  "cw.title": "手机上也能查看和操作终端",
  "cw.sub": "<p>长时间运行的任务，不一定需要一直坐在电脑前。</p><p>运行：</p>",
  "cw.p":
    "<p>扫描终端中显示的二维码，就可以在手机浏览器里查看电脑上的 Pane。</p>" +
    "<p>不需要安装手机 App。</p>" +
    "<p>可以查看实时终端输出、切换 Pane、输入命令、发送 Agent 消息，也可以查看任务完成状态。</p>" +
    "<p>当命令结束、Agent 完成任务或等待授权时，还可以通过通知了解情况。</p>" +
    "<p>支持 ntfy、飞书、企业微信、钉钉、Slack、Discord 等通知渠道。</p>" +
    "<p>手机负责查看和输入，实际命令仍然在电脑上执行。</p>" +
    "<p>Web 服务默认需要主动启动。如果需要从外网访问，建议通过 Tailscale 等私有网络连接。</p>",

  "c1.title": "熟悉的 tmux 操作习惯",
  "c1.sub":
    "<p>keepane 不要求你重新学习一套终端操作方式。</p>" +
    "<p>常用 tmux 命令、快捷键和配置文件都尽可能保持兼容。</p>",
  "c1.code": `<span class="c"># 左右分屏</span>
Ctrl-b %

<span class="c"># 上下分屏</span>
Ctrl-b "

<span class="c"># 切换 Pane</span>
Ctrl-b h/j/k/l

<span class="c"># 最大化当前 Pane</span>
Ctrl-b z

<span class="c"># 查看 Session / Window / Pane</span>
Ctrl-b w

<span class="c"># 断开 Session</span>
Ctrl-b d`,
  "c1.import": "支持导入已有的 <code>.tmux.conf</code>：",
  "c1.p":
    "<p>能够识别的配置会转换为 keepane 配置，暂不支持的部分会保留并说明原因。</p>" +
    "<p>Windows 下使用 ConPTY 和 Win32 Console API，PowerShell、cmd 和 WSL 都可以直接运行。</p>" +
    "<p>PSReadLine、输入法以及全屏终端程序也能正常使用。</p>",

  "c5.title": "安装",
  "c5.sub": "keepane 使用 Rust 开发，提供 Windows、Linux 和 macOS 版本。",
  "c5.win": "可以使用 MSI 安装包（在发布页下载），也可以通过 winget 安装：",
  "c5.get": "下载 MSI",
  "c5.cargo": "从源码安装",
  "c5.more": "更多安装方式和使用说明：<a href=\"https://github.com/newdee/keepane\">github.com/newdee/keepane</a>",

  "close.title": "一个工作空间，而不只是几个终端窗口",
  "close.p":
    "<p>keepane 从 tmux 熟悉的 Pane 和 Session 出发，增加了消息传递、Agent 协作、执行追踪和远程访问能力。</p>" +
    "<p>你可以继续像以前一样使用终端，也可以让多个 Agent 在不同 Pane 中分工，把它们的工作连接起来。</p>",
  "footer.built": "MIT License · Built with Rust",
};

const nodes = Array.from(document.querySelectorAll("[data-i18n]"));
const EN = new Map(nodes.map((n) => [n, n.innerHTML]));
// The pictures, each with its dark English source; a page that has words of
// its own also comes in Chinese (data-src-zh).
const pictures = Array.from(document.querySelectorAll('img[src^="img/"]'));
const EN_SRC = new Map(pictures.map((i) => [i, i.getAttribute("src")]));
// Links to a page that comes in both languages, the same way.
const links = Array.from(document.querySelectorAll("a[data-href-zh]"));
const EN_HREF = new Map(links.map((a) => [a, a.getAttribute("href")]));

// What the head's script chose before the first paint.
let theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";

function showPictures() {
  for (const i of pictures) {
    const src = lang === "zh" && i.dataset.srcZh ? i.dataset.srcZh : EN_SRC.get(i);
    const light = src.replace(/(\.\w+)$/, "-light$1");
    // In a <picture>, its light <source> is on or off; the <img> keeps the dark one.
    const source = i.parentElement.tagName === "PICTURE" ? i.parentElement.querySelector("source") : null;
    if (source) {
      if (source.getAttribute("srcset") !== light) source.setAttribute("srcset", light);
      source.media = theme === "light" ? "all" : "not all";
    }
    const want = source || theme !== "light" ? src : light;
    if (i.getAttribute("src") !== want) i.setAttribute("src", want);
  }
}

const THEME_LABEL = {
  en: { light: "Dark page", dark: "Light page" },
  zh: { light: "深色页面", dark: "浅色页面" },
};

function showTheme() {
  document.documentElement.dataset.theme = theme;
  document.querySelector('meta[name="theme-color"]').setAttribute("content", theme === "light" ? "#fdfdfb" : "#121212");
  const b = document.getElementById("theme");
  const label = THEME_LABEL[lang][theme];
  b.setAttribute("aria-label", label);
  b.title = label;
  showPictures();
}

function apply(lang) {
  for (const n of nodes) {
    const key = n.dataset.i18n;
    if (lang === "zh" && ZH[key]) n.innerHTML = ZH[key];
    else n.innerHTML = EN.get(n);
  }
  for (const a of links) a.setAttribute("href", lang === "zh" ? a.dataset.hrefZh : EN_HREF.get(a));
  document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  document.getElementById("lang").textContent = lang === "zh" ? "EN" : "中文";
  showTheme();
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

// A pick is remembered and wins over the system from then on: for this
// visit at least, where nothing can be stored.
let picked = false;
document.getElementById("theme").addEventListener("click", () => {
  theme = theme === "light" ? "dark" : "light";
  picked = true;
  showTheme();
  try {
    localStorage.setItem("keepane-theme", theme);
  } catch {
    /* private mode */
  }
});

// No pick yet: the page follows the system when it changes.
matchMedia("(prefers-color-scheme: light)").addEventListener("change", (e) => {
  if (picked) return;
  let kept = null;
  try {
    kept = localStorage.getItem("keepane-theme");
  } catch {
    /* private mode */
  }
  if (kept === "light" || kept === "dark") return;
  theme = e.matches ? "light" : "dark";
  showTheme();
});
