# 当前任务交接

更新时间：2026-08-15

## 当前阶段：F - Library schema 边界

本阶段先核对 handoff 描述再改代码。结果如下：

- 已完成阶段 1-4 与 A-E 已移入 `docs/planning/archive/`，归档入口见 `archive/README.md`。
- 原 BUG-01 描述已过期：`src/app/WindowTitleBar.tsx` 当前使用 `Minus` 图标，点击调用 `appWindow.minimize()`，`src-tauri/capabilities/default.json` 也包含最小化权限。仍需用户目视确认标题栏图标显示正确。
- “Library schema/migration 与业务仓储混在一起”的问题存在：改动前 `repository.rs` 共 1811 行，其中约 440 行属于 schema 创建、完整性校验和 v1-v7 迁移。

## 本阶段已实现

- 新增 `src-tauri/src/library/schema.rs`，集中管理仓库格式、schema 版本、当前建库 SQL、完整性/格式/版本校验和 v1-v7 迁移。
- `repository.rs` 只保留仓库定位、备用数据库接管、受控目录准备和 Library 查询/事务；文件降至约 1366 行。
- `library/mod.rs` 显式声明私有 `schema` 模块；Tauri 命令、前端 DTO 和数据库 schema 版本均未改变。
- 已完成 Library 定向 Rust 测试：14 个通过，0 个失败。
- `npm run build`、`cargo fmt --check`、`cargo metadata --no-deps`、capability JSON 解析和 `git diff --check` 均通过；文档中不再存在指向已归档实施计划旧位置的链接。

## 待验收

- H01：目视确认主窗口最小化按钮显示为横线，点击后窗口正常最小化。
- H02：使用可丢弃仓库确认作品树创建、搜索、重命名和回收站流程无变化。自动回归已覆盖对应后端行为，GUI 自动化仍禁止。

## 下一阶段候选

按优先级继续，每次只推进一个可独立验证的阶段：

1. 消除命令入口与仓储重复执行的 `integrity_check`，把完整仓库校验限制在配置/启动边界，同时保留普通连接的轻量格式与版本保护。
2. 扩充认证发布/查询和前端 latest-request-wins 的核心不变量测试。
3. 把 history/backup 跨模块工作流进一步上移到应用命令层。
4. 继续拆分 `AuthenticityModule.tsx`、`LibraryModule.tsx` 和 `HistoryModule.tsx` 的状态与命令编排。
5. 建立 Windows CI 与公开发布候选门槛；补 LICENSE、README、CONTRIBUTING、SECURITY、CHANGELOG 和发行政策。

暂缓项：长任务 operation ID、进度与取消；dialog/tree 无障碍；大树/大图性能基线；历史 snapshot/delta 清理意图和陈旧临时文件维护命令。

## 验证边界

- 本阶段继续运行 `cargo fmt --check`、Library 定向测试、`npm run build` 和 `git diff --check`。
- 不运行全量 `cargo test`、GUI 自动化、真实 C2PA 第三方验证或 TrustMark ONNX 自动化。
