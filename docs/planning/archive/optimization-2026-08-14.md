# 2026-08-14 优化与验收归档

本归档承接 `b0bca2f` 之后的全面审查整改。更早的历史/认证实现记录见 `history-2026-08-13-2026-08-14.md`。

## 用户已完成人工验收

用户确认优化前交接中的人工清单已经完成，包含：

- 分支发布锁、强制检查点、自动/手动提交限制与取消发布恢复。
- C2PA + TrustMark、仅 C2PA、区域框选、整图/裁剪/去 metadata 识别和跨 Artwork 溯源。
- 认证记录查看/再次导出、危险区二次确认、浅色/深色主题、窄窗口、长路径和大图布局。
- 深分叉历史图的紧凑/时间轴布局、节点宽度持久化、分支选择同步与发布后刷新。
- 当时 schema 仓库读取、旧记录兼容和完整项目编译。

上述验收针对优化前实现；阶段 1/2 新增的身份 claim、清理队列和失败重试仍按当前交接中的新增边界继续验收。

## 优化阶段 1

- 认证记录 ID 改为签名前生成并写入 C2PA `recordId`，仅 C2PA 图片可回查本地记录。
- C2PA 与 TrustMark 候选分别查询、按记录去重并保留证据来源；冲突不隐藏任一通道。
- 再次导出使用 `persist_noclobber`，后端拒绝覆盖已有目标。
- 发布页、预览、估算、识别和搜索采用 latest-request-wins；发布前复核分支归属。
- 分支删除补收集 `history_edges.delta_path`。
- 新增候选证据合并与 fork edge delta 回归测试。

## 优化阶段 2

- schema v7 增加 `pending_file_cleanup`，跨数据库/文件删除先在事务内登记清理项。
- Artwork 永久删除登记 `artworks/<id>/` 受控目录及带期望 SHA-256 的外部导出文件。
- 取消发布登记最终成品、认证副本和外部导出文件；文件失败不再伪装为全部成功。
- 清理成功出队，失败保留错误和 ID；页面提供重试，应用启动时重试历史遗留项。
- 仓库相对路径执行边界/规范化检查；外部文件只有哈希仍匹配时才删除。
- 认证发布、完整 fork、永久删除、清理重试和仓库设置保存纳入共享 `BackupState` 互斥。

## 自动验证

- `npm run build`
- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
- `cargo metadata --manifest-path src-tauri/Cargo.toml --format-version 1 --no-deps`
- `git diff --check`
- `cargo test --manifest-path src-tauri/Cargo.toml --offline --lib cleanup::tests`
- 定向通过：`initializes_current_cleanup_schema`、`permanently_deleted_artwork_removes_its_repository_directory`、`branch_deletion_collects_edge_delta_paths`、`merges_candidate_evidence_without_hiding_conflicts`

Rust 定向测试首次执行完成了当前 lib 测试目标的真实编译。没有运行全量 Rust 测试、GUI 自动化或真实 C2PA/ONNX 自动化。
