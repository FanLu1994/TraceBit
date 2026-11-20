## 做了什么

<!-- 一两句话说明改动和动机；关联的 issue 用 #123 引用 -->

## 影响范围

- [ ] UI（今日活动 / 最近统计 / 报告 / 设置）
- [ ] 采集与隐私规则
- [ ] 存储格式（需要兼容旧数据）
- [ ] AI 调用或提示词
- [ ] 构建、安装包、文档

## 验证

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] 涉及采集时跑过 `cargo run -- --native-smoke`
- [ ] 涉及界面时在 960×620 和更大窗口下看过
- [ ] 涉及存储时确认旧 `%APPDATA%\TraceBit` 数据仍可读取

## 截图

<!-- 界面改动请附改动前后的截图 -->
