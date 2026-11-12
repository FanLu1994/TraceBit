# TraceBit

> TraceBit 受 Alma Recorder 的启发。

一个独立、Windows 本地优先的行为记录与 AI 日报应用。

当前版本已完全迁移到 **Rust + GPUI**：界面、采集、存储、AI 请求和系统集成都由 Rust 实现，不再需要 Node.js、Electron、WebView、HTML/CSS 或 PowerShell。

TraceBit 按活动段记录前台应用、窗口标题、空闲/锁屏状态和去重后的屏幕快照，再把一天的记录整理成脱敏统计，让 AI 生成行为日报和具体省力建议。

## 功能

- GPUI 原生界面：今日活动、24 小时时间线、分类占比、应用排行、快照回看
- 前台应用与窗口标题记录，自动归类为开发、浏览器、通讯、会议、文档等
- 基于 Windows 真实键鼠空闲时间切分活动段；锁屏/睡眠期间暂停采集
- 应用切换与定时心跳截图；缩略图哈希、直方图和像素差异三级去重
- 可选浏览网站记录：主动开启后，通过 Windows UI Automation 仅保存当前浏览器域名
- AI 行为日报：时间去向、碎片化/会议/深夜使用等模式观察、省力建议、明日小实验
- 可工具化操作推荐：根据真实切换和时长证据匹配开源项目、Skill、MCP
- 可选联网资源检索：支持 DuckDuckGo 和 Brave Search，失败时保留本地精选推荐
- 兼容 OpenAI 格式接口：DeepSeek、通义、Kimi、OpenAI、本地 Ollama 等
- 系统托盘、关闭到托盘、开机自启、自动日报通知、容量轮换和一键清空
- 单实例运行，窗口与托盘图标嵌入可执行文件，无额外资源目录

## 运行

需要 Windows 10/11。

### 已编译版本

运行 `dist/TraceBit Setup 0.3.0.exe`。安装到 `%LOCALAPPDATA%\Programs\TraceBit`，不需要安装 Rust 或其他运行时。卸载程序不会主动删除 `%APPDATA%\TraceBit` 中的个人记录。

### 从源码运行

安装当前 stable Rust（MSVC toolchain）和 Visual Studio 2022 C++ 生成工具，然后：

```bash
cargo run
```

也可以双击 `start.cmd`。

## 自测

```bash
cargo test                 # 设置、存储、统计、脱敏、推荐和模拟 AI 全链路
cargo run -- --native-smoke # 真实前台窗口采集与主显示器 JPEG 截图
```

也可以运行 `scripts\smoke.cmd`。原生 smoke 使用临时目录，结束后会删除测试记录和截图，不污染正式数据。

## 打包

安装 NSIS 3，然后运行：

```bat
scripts\dist.cmd
```

脚本会执行 `cargo build --release` 并生成：

```text
dist\TraceBit Setup 0.3.0.exe
```

发布程序只有一个内嵌图标的 `tracebit.exe`，无 Electron 运行时和 PowerShell 脚本。

## AI 配置

打开「设置 → AI 分析」，填写：

- **接口地址**：例如 `https://api.deepseek.com/v1`，本地 Ollama 为 `http://localhost:11434/v1`
- **API Key**：本地 Ollama 可留空
- **模型名**：例如 `deepseek-chat`、`qwen-plus`、`gpt-4o-mini` 或本地模型名

保存后可「测试连接」，再到「AI 日报」选择日期并生成。开启「每日自动生成」后，到点会分析当天记录并发送 Windows 通知。

日报中的「可工具化的操作」由本地规则生成，不依赖模型编造工具。默认使用内置精选资源；主动开启「联网搜索效率资源」后，才会通过 DuckDuckGo 或 Brave Search 检索补充候选。DuckDuckGo 无需密钥，Brave Search 使用独立搜索 API Key。联网候选只供人工审核，不会自动安装或启用。

## 数据与隐私

数据默认位于：

```text
%APPDATA%\TraceBit\
├── settings.json             # 设置与 API Key（明文，仅本机）
├── data\
│   ├── days\YYYY-MM-DD.jsonl # 前台、空闲、锁屏、快照索引事件
│   └── snapshots\YYYY-MM-DD\ # JPG 快照
└── reports\YYYY-MM-DD.json   # 已生成日报
```

Rust 版沿用这些格式，可直接读取旧版历史数据。

- 数据默认只保存在本机；不记录键盘输入内容。
- 浏览网站记录默认关闭。主动开启后，仅通过 Windows UI Automation 读取支持的浏览器地址栏；网站记录只保存域名，不保存路径、查询参数或页面内容。屏幕快照由独立开关控制。
- 锁屏和睡眠时不写入前台活动，也不截图。
- 命中敏感关键词的前台窗口不截图；可在设置中自行增删关键词。
- AI 分析只发送脱敏后的时长、切换次数、分类、应用名和截断后的窗口标题；**截图和浏览网站域名不会发送给 AI**。
- 联网资源搜索默认关闭。开启后，搜索服务只接收内置的抽象工具关键词，不接收窗口标题、网站域名、网址、截图或用户行为统计；来源链接验证只访问资源公开地址。
- 标题发送前会遮蔽常见邮箱、密钥样式和长数字。涉及隐私时可使用本地模型。
- 快照只抓主显示器。敏感关键词按前台应用/标题判断，无法识别背景窗口里的敏感内容。

## 目录结构

```text
src/
  main.rs            # 程序入口与 smoke 命令
  app.rs             # GPUI 窗口、三页 UI、托盘和后台任务
  input.rs           # 原生 GPUI 文本输入组件
  platform.rs        # Win32、UIA、窗口图标、自启动和通知
  recorder.rs        # 采集线程、截图去重和隐私规则
  store.rs           # JSONL 日存储与时间线/活动段/应用统计
  analyzer.rs        # 日报骨架、脱敏和 OpenAI 兼容调用
  recommendations.rs # 本地操作识别、精选资源和推荐文案
  web_search.rs      # 可选抽象搜索、过滤和来源验证
  categories.rs      # 应用分类与会议识别
  settings.rs        # 兼容旧版 camelCase 的本地设置
assets/              # 构建时嵌入的图标
build.rs             # Windows exe 图标资源
installer.nsi        # NSIS 安装包定义
scripts/dist.cmd     # Release 与安装包构建
scripts/smoke.cmd    # 单元测试与真实采集 smoke
```
