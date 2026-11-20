# TraceBit

[简体中文](README.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

[![CI](https://github.com/FanLu1994/TraceBit/actions/workflows/ci.yml/badge.svg)](https://github.com/FanLu1994/TraceBit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/FanLu1994/TraceBit)](https://github.com/FanLu1994/TraceBit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4)

一个独立、Windows 本地优先的行为记录与 AI 工具升级建议应用。

> 受 Alma Recorder 的启发。

TraceBit 按活动段记录前台应用、窗口标题、空闲/锁屏状态和去重后的屏幕快照，在本地把每天的记录算成带编号的证据，并持续学习你的工作方式：常用工具、固定来回、产出节奏、反复处理的项目和网站。AI 的任务不是纠正你的习惯，而是基于你已经在做的事，推荐更省步骤的工具、插件、配置和开源替代。

当前版本是完全的 **Rust + GPUI** 原生应用：界面、采集、存储、AI 请求和系统集成都由 Rust 实现，不需要 Node.js、Electron、WebView、HTML/CSS 或 PowerShell。

## 安装

需要 Windows 10/11，不需要 Rust 或其他运行时。

下载最新安装包 **[TraceBit Setup（Releases）](https://github.com/FanLu1994/TraceBit/releases/latest)** 并运行：

- 安装到 `%LOCALAPPDATA%\Programs\TraceBit`，创建开始菜单和桌面快捷方式
- 覆盖安装会保留 `%APPDATA%\TraceBit` 中的记录
- 卸载不会主动删除个人记录，需要时请手动清理 `%APPDATA%\TraceBit`

## 使用

左侧导航有四页，页面之间用快捷键切换：

| 快捷键 | 页面 | 内容 |
| --- | --- | --- |
| `Ctrl+1` | 今日活动 | 当天时间线、应用排行、分类占比、快照回看 |
| `Ctrl+4` | 最近统计 | 7 / 14 / 30 天汇总，每日曲线、活跃时段、每日明细 |
| `Ctrl+2` | AI 建议 | 当天结论、工具升级候选、打开完整 HTML 报告 |
| `Ctrl+3` | 设置 | 记录、截图、网站、AI、推送与系统集成 |

其他快捷键：`Ctrl+Shift+R` 开始/暂停记录，`Ctrl+W` 关闭窗口，`Esc` 关闭浮层，`Tab` / `Shift+Tab` 切换焦点。

### 配置 AI

打开「设置 → AI 分析」，填写：

- **接口地址**：例如 `https://api.deepseek.com/v1`，本地 Ollama 为 `http://localhost:11434/v1`
- **API Key**：本地 Ollama 可留空；使用 Windows DPAPI 加密保存
- **模型名**：建议使用支持图片输入的多模态模型，例如 `qwen2.5vl`（本地 Ollama）、`gpt-4o-mini`；只用文本建议时也可以用 `deepseek-chat`、`qwen-plus` 等纯文本模型

接口地址按填写的内容使用，只有纯域名（如 `http://localhost:11434`）会自动补 `/v1`，因此 `/v1beta/openai`、`/api/paas/v4` 等路径也可以直接填写。请求使用 JSON 输出模式（服务不支持时自动退回），遇到 429/5xx 会重试，输出被截断时会明确报错。

保存后可「测试连接」，再到「AI 建议」选择日期并生成。生成前可以点「查看发送内容」核对将要发送的全部文本。开启「每日自动生成」后，到点会生成当天建议并发送 Windows 通知；如果当时电脑在睡眠或程序没运行，或者生成后又有大量活动，第二天会补生成，但不会覆盖手动生成的建议。

工作画像在应用启动时自动回填最近 30 天，之后每 10 分钟更新当天一次，不依赖是否配置了 AI。画像是本地 JSON，可以随时查看或删除；删除后下次启动会重新学习。

## 功能

### 记录

- 前台应用与窗口标题记录，自动归类为开发、浏览器、通讯、会议、文档等
- 基于 Windows 真实键鼠空闲时间切分活动段；开会、全屏看视频或放映时使用更长的空闲阈值；锁屏/睡眠期间暂停采集
- 过滤 Alt-Tab 路过的瞬时窗口，按窗口标题和网站累计停留时长；跨午夜的活动不丢失
- 应用切换与定时心跳截图（截取前台窗口所在的显示器）；缩略图哈希、直方图和像素差异三级去重
- 可选浏览网站记录：主动开启后，通过 Windows UI Automation 仅保存当前浏览器域名，并按网站把浏览器时间归入开发、文档、娱乐等类别
- 单实例运行，窗口与托盘图标嵌入可执行文件，无额外资源目录

### 界面与统计

- GPUI 原生界面：今日活动、24 小时时间线、分类占比、应用排行、快照回看
- 界面语言：简体中文、English、日本語、한국어，默认跟随 Windows 显示语言，保存后立即生效
- 最近统计：7 / 14 / 30 天总览，按天活跃曲线、活跃时段、常用应用与分类占比、常去网站和可点开的每日明细
- 每日明细点击任意一行直接跳到那天的今日活动
- 系统托盘、关闭到托盘、开机自启、容量轮换和一键清空

### 工作画像与 AI 建议

- 工作画像：把每个记录日合并成长期画像（主力工具与用途、日常来回、活跃节奏、反复处理的项目和网站），是 AI 建议的长期依据
- AI 工具升级建议：本地计算证据和长期画像，AI 只负责理解你怎么工作，再给出能落到具体工具、插件、快捷键、配置或自动化上的升级；每条都必须引用证据编号，无法核实的数字会被标出
- 只做工具升级，不强制改变习惯：要求你减少、戒掉、定时集中处理或早睡的内容会被本地规则过滤，报告只保留能让现有动作更省力的建议
- 开源替代：本地规则识别长期使用的商业工具（API 客户端、数据库客户端、设计/剪辑软件、远程控制、压缩工具等），给出操作方式接近的开源替代，AI 只能按编号引用
- 采纳闭环：在应用内标记「已用上 / 不需要」；已忽略的候选不再推荐，已采纳的工具会在下次报告里自动对比同类来回的变化
- 每日推送：默认 10:30 用系统通知推送昨日的结论和一句问候；昨天没有记录时不打扰，可关闭或改时间；错过时间会在当天补发一次
- 完整报告以独立 HTML 页面在浏览器中打开：工作画像、你的工作方式、工具升级机会、下次自动核对、采纳回看、其他候选工具，依据编号可点击跳转，支持深色模式和打印
- 只观察、不布置作业：报告会记录几个下次值得对照的指标并自动核对，不需要你执行任何实验
- 可选截图时段摘要：按整点对齐的时段选取快照，附带当时的前台应用，由上方配置的多模态模型总结，摘要会作为建议的依据
- 可工具化操作推荐：本地规则根据真实切换和时长证据匹配开源项目、Skill、MCP，作为候选交给 AI 选用
- 可选联网资源检索：支持 DuckDuckGo 和 Brave Search，失败时保留本地精选推荐
- 兼容 OpenAI 格式接口：DeepSeek、通义、Kimi、OpenAI、本地 Ollama 等
- 自动生成建议（错过时间会补生成，不覆盖手动生成的结果）

## 数据与隐私

数据默认位于：

```text
%APPDATA%\TraceBit\
├── settings.json             # 设置；API Key 以 DPAPI 加密
├── profile.json              # 本地学习的工作画像（每天一条记录）和工具决定
├── digest.json               # 每日推送状态（最近一次推送的日期）
├── data\
│   ├── days\YYYY-MM-DD.jsonl # 前台、空闲、锁屏、快照索引事件
│   └── snapshots\YYYY-MM-DD\ # JPG 快照
├── reports\YYYY-MM-DD.json   # 已生成的建议（含证据、工作画像、工具决定和 token 用量）
├── reports\YYYY-MM-DD.html   # 同一份建议的浏览器版报告
└── intervals\
    ├── YYYY-MM-DD.jsonl      # 截图时段摘要
    ├── last_request.json     # 最近一次发给视觉模型的文本和截图列表
    └── cursor.json           # 时段分析进度
```

Rust 版沿用这些格式，可直接读取旧版历史数据。新写入的事件带 `v` 版本号，旧格式仍可读取。

- 活动记录、截图和摘要保存在本机；不记录键盘输入内容。使用远程 AI 时，只发送下文列出的内容。
- 浏览网站记录默认关闭。主动开启后，仅通过 Windows UI Automation 读取支持的浏览器地址栏；网站记录只保存域名，不保存路径、查询参数或页面内容。屏幕快照由独立开关控制。
- 锁屏和睡眠时不写入前台活动，也不截图。
- 命中敏感关键词（按应用名、标题和网站判断）的前台窗口：标题以 `[敏感窗口]` 落盘，不截图，不记录网站；可在设置中自行增删关键词。
- AI 建议只发送本地计算的脱敏证据：时长、次数、分类、应用名、截断后的窗口标题，以及本地汇总的长期工作画像（工具名、来回的应用对、节奏、反复出现的窗口标题）；聊天类应用只发送应用名，不发送会话标题；**截图本身和浏览网站域名都不会发送给建议模型**，网站列表只在本地报告中显示。截图时段摘要（如已开启）会以文本形式作为依据。
- 截图时段分析默认关闭。开启后，快照会发送到上方配置的模型接口（本地或云端）；画面可能包含窗口标题、网站地址、聊天内容或其他敏感信息，涉及隐私时请使用本地多模态模型。
- 联网资源搜索默认关闭。开启后，搜索服务只接收内置的抽象工具关键词，不接收窗口标题、网站域名、网址、截图或用户行为统计；来源链接验证只访问资源公开地址。
- 文本发送前会遮蔽邮箱、密钥和令牌、银行卡号、身份证号、手机号、IP、带参数的链接和 `C:\Users\<用户名>` 路径。涉及隐私时可使用本地模型。
- 快照截取前台窗口所在的显示器。敏感关键词按前台应用、标题和网站判断，无法识别背景窗口里的敏感内容。

## 开发

安装 stable Rust（MSVC toolchain）和 Visual Studio 2022 C++ 生成工具，然后：

```bash
cargo run
```

也可以双击 `start.cmd`。

### 自测

```bash
cargo test                  # 设置、存储、统计、脱敏、推荐和模拟 AI 全链路
cargo run -- --native-smoke # 真实前台窗口采集与 JPEG 截图
```

也可以运行 `scripts\smoke.cmd`。原生 smoke 使用临时目录，结束后会删除测试记录和截图，不污染正式数据。

### 打包

安装 [NSIS 3](https://nsis.sourceforge.io/)，然后运行：

```bat
scripts\dist.cmd
```

脚本会执行 `cargo build --release` 并生成 `dist\TraceBit Setup <版本>.exe`。发布程序只有一个内嵌图标的 `tracebit.exe`，无 Electron 运行时和 PowerShell 脚本。

推送 `v*` 标签时，`.github/workflows/release.yml` 会自动构建并上传安装包到对应 Release；提交和 PR 由 `.github/workflows/ci.yml` 执行 fmt、clippy、test 和 release 构建。

### 目录结构

```text
src/
  main.rs            # 程序入口与 smoke 命令
  app.rs             # GPUI 窗口、四页 UI、托盘和后台任务
  input.rs           # 原生 GPUI 文本输入组件
  platform.rs        # Win32、UIA、窗口图标、自启动和通知
  i18n.rs            # 简体中文 / English / 日本語 / 한국어 文本选择与语言检测
  recorder.rs        # 采集线程、截图去重和隐私规则
  store.rs           # JSONL 日存储与时间线/活动段/应用/网站/分类统计
  stats.rs           # 最近一段时间的周期统计聚合
  insights.rs        # 本地指标和带编号的证据（深度工作、打断、碎片、趋势）
  profile.rs         # 长期工作画像：工具、来回、节奏、反复项目，以及工具决定
  analyzer.rs        # 建议生成：提示词、JSON 校验、工作画像、能力边界过滤、Markdown 渲染
  report_html.rs     # 建议的独立 HTML 报告
  llm.rs             # OpenAI 兼容调用：JSON 模式、重试、截断检测
  redact.rs          # 发送前脱敏与标题清理
  secret.rs          # DPAPI 密钥加密
  intervals.rs       # 截图时段分析、多模态请求、图片编码和摘要存储
  recommendations.rs # 本地操作识别和精选资源（作为 AI 的工具候选）
  web_search.rs      # 可选抽象搜索、过滤和来源验证
  categories.rs      # 应用与网站分类、会议识别
  settings.rs        # 兼容旧版 camelCase 的本地设置
assets/              # 构建时嵌入的图标
build.rs             # Windows exe 图标资源
installer.nsi        # NSIS 安装包定义
scripts/dist.cmd     # Release 与安装包构建
scripts/smoke.cmd    # 单元测试与真实采集 smoke
```

## 参与贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)：环境要求、提交前检查、代码约定和发布流程。所有改动请附验证结果，涉及界面的改动请提供截图。

## 更新日志

见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

[MIT](LICENSE)
