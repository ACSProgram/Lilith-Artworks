# 当前任务交接

更新时间：2026-10-18

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-18 批次：**0.3 开发启动（`dev` 工作树，修订未提交）**。

- **0.2 已收口并归档**：`0be6224` 切分 `0.2.0`，`v0.2.0` 附注标签已打并推送；`main` 固定为
  0.2 发布线。切分内容、压力测试口径、自动与人工门槛、未签名发布说明见
  `archive/handoff-log-2026-10-18.md`，更早的验收记录见 `archive/handoff-log-2026-10-16.md`。
- **0.3 开发线**：`dev` 从 `0be6224` 创建。本轮在该分支工作树完成 0.3 落实前修订，
  **尚未提交**：
  - `0.3-architecture-algorithm-plan-2026-10-05.md` 定稿为可实施口径（修订汇总见其 §0.5）：
    alpha1 不改持久 schema；alpha2 阶段 7 一次 v4 → v5（含 `branch_notices`、缩略图列与路径
    索引）；画板采用公共条目表 + kind 子表；仓储统计口径去重；typed 错误、`CancelToken`、
    空闲校验门控与 `should_yield` 让位的归属明确。
  - 表层文档同步：本文件、`todo.md` 与 `docs/README.md` 的 0.3 状态。
  - 发布与 CI 口径：`docs/guides/release-policy.md` 补 `main`/`dev` 模型；
    Windows CI 覆盖 `dev` 推送；`tools/release/verify-metadata.mjs` 按版本映射 schema
    （0.2.x → v4、0.3.x → v5）。
- **下一步**：提交本轮文档与配置修订；执行 0.3 阶段 0（建立 0.2 基线，不改变行为）；随后从
  阶段 1（CommandContext、typed error、`CancelToken`、`OperationCoordinator` 与 `AppEvent` v1
  信封）开始实施。alpha1 / alpha2 是开发批次，是否打 alpha 标签由维护者决定。
- 未完成事项全部在 `todo.md`：0.3 前的补强候选与 0.3 计划本身（尚未实施）。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；
  `dev` = 0.3 开发线，从 `0be6224` 创建。
- 版本：`dev` 尚未切 0.3 版本号，五处版本字段仍为 `0.2.0`；日常改动写入 `CHANGELOG.md`
  的 `## Unreleased`，切版本与打标签由维护者按 `docs/guides/release-policy.md` 单独决定。
- repository schema v4，ChunkFile format v1；0.3-alpha1 保持 v4（仅连接内临时表），
  0.3-alpha2 阶段 7 执行唯一一次 v4 → v5 迁移。
- 运行要求：Windows + WebView2；素材板另需支持 WebGPU 与 BC7 纹理压缩的图形适配器（见
  `README.md` 的「运行环境」）。
- 项目定位：平面美术个人项目的资源、版本管理与发布工具。领域模块为 Library（作品树）、
  History/Backup（分支与增量历史）、Authenticity（成品与 C2PA/TrustMark）、Pin-board（素材板）。

## 文档约束

- 当前功能契约以 `docs/architecture/`、`docs/modules/` 与 `docs/guides/` 为准。
- **表层文档只保留当前状况、未完成事项与规划**：本文件与 `todo.md` 不累积历史——已完成批次、
  已通过的人工验收、被替代的计划与其他历史增量一律移入 `docs/planning/archive/`，并同步更新
  `docs/planning/archive/README.md` 的索引。
- 未完成事项只写入 `docs/planning/todo.md`；某项通过并归档后，同时从 `todo.md` 的待办/待验收
  段落与本文件中删除，不在表层留下“已通过”的痕迹。
- 历史增量的详细记录归 `archive/` 下对应日期的日志；版本、标签、人工门槛与产物口径归
  `docs/guides/release-policy.md`。
