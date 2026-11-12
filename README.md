# TraceBit

一个独立、Windows 本地优先的行为记录与 AI 日报应用。

TraceBit 参考 Alma 中 ActivityRecorder 的采集思路重新实现：按活动段记录前台应用、窗口标题、空闲/锁屏状态和去重后的屏幕快照；再把一天的记录整理成脱敏统计，让 AI 生成行为日报和具体省力建议。**不包含 Alma 的程序文件，也不依赖 Alma 运行。**

## 功能

- 前台应用与窗口标题记录，自动归类为开发、浏览器、通讯、会议、文档等
- 基于 Windows 真实键鼠空闲时间切分活动段；锁屏/睡眠期间暂停采集
- 应用切换与定时心跳截图；缩略图哈希、直方图和像素差异三级去重
- 浅色任务式界面，左侧固定的“开始记录 / 暂停记录”主按钮，状态同步到设置页
- 今日视图：24 小时时间线、分类占比、应用排行、高频标题、快照回看
- AI 行为日报：时间去向、碎片化/会议/深夜使用等模式观察、按收益排序的省力建议、明日小实验
- 日报页内直接配置和测试 AI，不必先进入设置页
- 兼容 OpenAI 格式接口：DeepSeek、通义、Kimi、OpenAI、本地 Ollama 等
- 托盘常驻、开机自启、快照容量上限、保留天数和一键清空

## 运行

需要 Windows 10/11、Node.js 20+。

```bash
npm install
npm start
```

也可以双击 `start.cmd`。首次运行若缺少依赖，会先执行 `npm install`。

自测：

```bash
npm run smoke       # 主进程、存储、脱敏和模拟 AI 全链路
npm run test:ui     # 真实 Electron 窗口中的开关、设置同步、AI 配置和响应式测试
```

UI 测试截图输出到 `artifacts/ui-smoke/`。界面设计约束见 `DESIGN.md`。

打包 NSIS 安装包：

```bash
npm run dist
```

安装包输出到 `dist/`。

## AI 配置

打开「设置 → AI 分析」，填写：

- **接口地址**：例如 `https://api.deepseek.com/v1`，本地 Ollama 为 `http://localhost:11434/v1`
- **API Key**：本地 Ollama 可留空
- **模型名**：例如 `deepseek-chat`、`qwen-plus`、`gpt-4o-mini` 或本地模型名

先在设置里「测试连接」，再到「日报」选择日期并生成。开启「每日自动生成」后，到点会自动分析当天记录并发送系统通知。

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

- 数据默认只保存在本机；不记录键盘输入内容。
- 锁屏和睡眠时不写入前台活动，也不截图。
- 命中敏感关键词的前台窗口不截图；可在设置中自行增删关键词。
- AI 分析只发送脱敏后的时长、切换次数、分类、应用名和截断后的窗口标题；**截图不会发送给 AI**。
- 标题发送前会遮蔽常见邮箱、密钥样式和长数字。仍建议不要把涉密窗口标题交给云端模型；涉及隐私时可使用本地模型。
- 快照只抓主显示器。敏感关键词按前台应用/标题判断，无法识别背景窗口里的敏感内容。

## 目录结构

```text
src/main/
  main.js          # Electron 生命周期、托盘、IPC、自动日报
  recorder.js      # 采集、截图去重、锁屏暂停、存储轮换
  store.js         # JSONL 日存储与时间线/活动段/应用统计推导
  analyzer.js      # 日报数据骨架、脱敏、OpenAI 兼容调用
  categories.js    # 应用分类与会议识别
  settings.js      # 本地设置
  ps/poller.ps1    # 常驻 PowerShell 前台窗口 + 空闲轮询
src/renderer/      # 无前端框架的本地界面
assets/            # 图标
```

## 与 Alma 的关系和差异

Alma 的安装包中，录制逻辑位于压缩后的 `alma-server/lib/server.mjs`，主要包括 `ActivityRecorder`、活动存储、活动分析和日报生成。TraceBit 抽取的是这些可独立理解的机制，并针对 Windows 做了轻量重实现。

主要差异：

- 使用 JSONL 而不是 Alma 的 SQLite，便于查看、备份和排障
- 使用一个常驻 PowerShell 轮询进程，避免每次查询都启动新进程
- 不做 OCR、向量索引和键盘次数采集，降低依赖与隐私风险
- AI 提示词专注于「一天的行为复盘 + 可执行省力建议」，不复用 Alma 的记忆库体系

当前版本为 Windows 桌面版；macOS/Linux 采集端尚未实现。
