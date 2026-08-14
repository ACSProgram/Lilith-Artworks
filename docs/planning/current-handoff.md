# 当前任务交接

更新时间：2026-08-14

## 当前状态

全面审查整改的优化阶段 1、阶段 2 已提交；阶段 3 已实现并通过 Rust 定向编译/测试、格式和补丁检查。用户已确认阶段 1/2 之前的历史、认证与 UI 人工清单通过；详情已归档到：

- `docs/planning/archive/history-2026-08-13-2026-08-14.md`
- `docs/planning/archive/optimization-2026-08-14.md`

当前 schema 为 v7。项目仍采用测试期兼容政策：公开承诺数据兼容前允许使用新仓库，不为内部 schema 版本补长期无损迁移。

## 三轮已完成

### 阶段 1：身份、候选与竞态

- C2PA 固定写入签名前生成的本地 `recordId`；仅 C2PA 发布可回查记录。
- C2PA/TrustMark 候选分别查询、去重并标注证据，冲突时两边都显示。
- 再次导出后端禁止覆盖已有文件。
- 发布/识别/搜索采用 latest-request-wins，发布前复核分支归属。
- 分支删除补齐 edge delta 清理。

### 阶段 2：可恢复清理与互斥

- schema v7 `pending_file_cleanup` 在事务内登记清理项；失败保留并可重试，启动时自动 reconciliation。
- Artwork 永久删除和取消发布返回真实清理结果，不再忽略文件错误。
- 仓库路径限制在仓库内；外部导出文件只有 SHA-256 仍匹配时才删除。
- 认证发布、fork、永久删除、清理重试和仓库设置保存使用共享后端运行锁。

### 阶段 3：发布意图与孤儿文件恢复

- 最终成品、认证外部输出和仓库副本在正式发布前登记带 SHA-256 的清理意图。
- `final_artifacts` / `certification_records` 写入与清理意图移除在同一 SQLite 事务提交；事务失败或进程中断时启动 reconciliation 可回收孤儿文件。
- 清理器删除前扫描最终成品、认证记录和历史图引用；提交结果不确定时保留已被数据库接管的文件。
- 认证外部输出改为临时文件 `persist_noclobber`，不再依赖平台相关的 `rename` 覆盖行为。
- 认证记录在提交前于同一事务回读，避免提交后新连接读取失败造成伪失败。

## 当前验证

- `npm run build`：通过。
- Rust lib 测试目标：已真实编译。
- 6 个新增定向回归：通过（清理哈希/越界、schema v7、Artwork 目录、edge delta、候选证据）。
- 阶段 3 的 4 个定向回归：通过（仓库文件哈希重试、引用保护、发布意图原子接管、意图缺失时事务回滚）。
- `cargo fmt --check`、`cargo metadata --no-deps`、`git diff --check`：通过。

未运行：全量 `cargo test`、GUI 自动化、真实 C2PA 证书/第三方验证器与 TrustMark ONNX 自动化。GUI 自动化按验证策略继续禁止。

## 下一步

### P1

- 为阶段 1 新 claim/冲突 UI 和阶段 2 取消发布失败重试做一次可丢弃仓库人工验收。
- 为阶段 3 正常进入发布、认证导出和重启 reconciliation 做一次可丢弃仓库人工验收；故障注入继续由自动测试覆盖。
- 扩充核心不变量测试：历史删除/精简、发布事务、认证查询与前端异步状态。
- 建立 Windows CI 和公开发布候选门槛。

### P2

- 拆出 Library schema/migration；拆分三个超大前端组件。
- 把 history/backup 跨模块工作流进一步上移应用层。
- 缓存仓库初始化，避免每个命令执行完整 `integrity_check`；修复备用数据库 WAL 改名。
- 统一错误码/诊断日志，补仓库维护命令和模型来源/许可证/固定哈希。
- 补 LICENSE、README、CONTRIBUTING、SECURITY、CHANGELOG、兼容与发行政策。

### P3

- 长任务 operation ID、进度与取消。
- dialog/tree 无障碍与键盘焦点。
- 大树、大图性能基线及具名计时常量。

## 建议入口

- 清理队列：`src-tauri/src/cleanup.rs`
- Library 永久删除/schema：`src-tauri/src/library/repository.rs`
- 取消发布/认证发布：`src-tauri/src/authenticity/commands.rs`、`publication_repository.rs`
- 运行互斥：`src-tauri/src/backup/runtime.rs`
- 清理失败 UI：`src/modules/library/LibraryModule.tsx`、`src/modules/authenticity/AuthenticityModule.tsx`
