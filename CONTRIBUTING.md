# 参与开发

TraceBit 是 Windows 本地优先的 Rust 桌面应用，欢迎提 issue、修 bug 和加功能。动手前建议先开 issue 说明场景，避免重复劳动。

## 环境要求

- Windows 10/11
- Rust stable（MSVC toolchain），`rustup default stable-x86_64-pc-windows-msvc`
- Visual Studio 2022 C++ 生成工具（GPUI 和 Win32 依赖需要）
- 打包安装包时需要 [NSIS 3](https://nsis.sourceforge.io/)

```bash
rustup component add rustfmt clippy
cargo run
```

## 提交前必须通过

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --native-smoke   # 涉及采集、截图、隐私规则时
```

CI 会在 Windows 上重复执行前三项，任何一项失败都不合并。

`--native-smoke` 使用临时目录，结束后删除测试记录和截图，不会污染 `%APPDATA%\TraceBit`。

## 代码约定

- 界面配色、间距和页面结构遵循 [DESIGN.md](DESIGN.md)；新增页面先复用现有的 `section` / `panel` / `metric` 等辅助函数。
- 采集、存储、AI 三层不要互相直连：UI 只读 `DayStore` 和 `Analyzer` 的结果，网络请求在后台线程完成。
- 时间统一用 `i64` 毫秒时间戳（本地时区），日期键统一 `YYYY-MM-DD`。
- 发送给 AI 的内容必须先过 `redact`；新增字段请同时更新 `redact` 和相关测试。
- 数据格式变更必须保持向后兼容：新事件带 `v` 版本号，旧文件要能继续读取，`%APPDATA%\TraceBit` 不能被破坏性迁移。
- 不引入需要额外运行时（Node、Python、.NET 桌面框架）的依赖。

## 提交信息

使用 Conventional Commits 的简化形式，一句话说明做了什么：

```text
feat: add a recent activity statistics page
fix: keep cross-midnight activity in the day timeline
docs: document the installer build
refactor: split the settings form out of app.rs
chore: bump version to 0.6.0
```

一个提交只做一件事；格式化、重命名等无关改动单独提交。

## 发布流程

1. 更新 `CHANGELOG.md`，把 `[Unreleased]` 的内容挪到新版本标题下。
2. 同步三处版本号：`Cargo.toml`、`Cargo.lock`（`tracebit` 包条目）、`installer.nsi` 的 `APP_VERSION`。
3. 本地验证：

   ```bash
   cargo fmt --all --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo run -- --native-smoke
   scripts\dist.cmd
   ```

4. 提交 `chore: bump version to X.Y.Z`，打标签并推送：

   ```bash
   git tag -a v0.6.0 -m "TraceBit 0.6.0"
   git push origin main --follow-tags
   ```

5. 推送 tag 后，`.github/workflows/release.yml` 会校验 tag 与 `Cargo.toml` 版本一致，构建 `tracebit.exe`、生成 NSIS 安装包并上传到对应 Release。GitHub 上需要先有同名 tag；Release 不存在时 workflow 会自动创建。

安装包命名固定为 `dist\TraceBit Setup X.Y.Z.exe`，Release 附件由 workflow 上传，不需要手动传。

## 目录速查

| 路径 | 作用 |
| --- | --- |
| `src/app.rs` | GPUI 窗口、页面和后台任务 |
| `src/store.rs` | JSONL 日存储与派生统计 |
| `src/recorder.rs` | 采集线程、截图去重、隐私规则 |
| `src/platform.rs` | Win32、UIA、通知、自启动 |
| `src/analyzer.rs` | 建议生成与提示词 |
| `src/stats.rs` | 周期统计聚合 |

完整的模块说明见 README 的「目录结构」。
