# 当前任务交接

更新时间：2026-10-18

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-18 批次：**0.3 阶段 0（建立 0.2 基线，不改变行为）**。

- **0.2 已收口并归档**：`0be6224` 切分 `0.2.0`，`v0.2.0` 附注标签已打并推送；`main` 固定为
  0.2 发布线。切分内容、压力测试口径、自动与人工门槛、未签名发布说明见
  `archive/handoff-log-2026-10-18.md`，更早的验收记录见 `archive/handoff-log-2026-10-16.md`。
- **0.3 开发线**：`dev` 从 `0be6224` 创建，落实前修订已随 `d6206ac` 提交：
  `0.3-architecture-algorithm-plan-2026-10-05.md` 定稿为可实施口径（修订汇总见其 §0.5），
  `main`/`dev` 分支模型、Windows CI 与发布元数据口径（0.2.x → v4、0.3.x → v5）、表层文档
  同步一并落地。
- **阶段 0 已完成（随本阶段提交）**：补齐取消延迟 / 锁等待 / 调度扫描的测量能力（新增
  `stress_baseline` 组、headless 只读探针 `scheduler-scan`、闸门取消计时与 `Spawn::scope`），
  在临时仓库副本 `D:\temp\Artworks` 上采集 release 档数值；数据见
  `0.3-baseline-2026-10-18.md`（采集后已用采集前的副本还原仓库）。
- **下一步**：从阶段 1（CommandContext、typed error、`CancelToken`、`OperationCoordinator`
  与 `AppEvent` v1 信封）开始实施。alpha1 / alpha2 是开发批次，是否打 alpha 标签由维护者决定。
- 未完成事项全部在 `todo.md`：0.3 前的补强候选与 0.3 计划的阶段 1–9（均尚未实施）。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；
  `dev` = 0.3 开发线，从 `0be6224` 创建。
- 版本：`dev` 尚未切 0.3 版本号，五处版本字段仍为 `0.2.0`；日常改动写入 `CHANGELOG.md`
  的 `## Unreleased`，切版本与打标签由维护者按 `docs/guides/release-policy.md` 单独决定。
- repository schema v4，ChunkFile format v1；0.3-alpha1 保持 v4（仅连接内临时表），
  0.3-alpha2 阶段 7 执行唯一一次 v4 → v5 迁移。
- 0.3 基线（阶段 0 一次性采集）：临时仓库副本 `D:\temp\Artworks`（3 作品 / 59 历史节点 /
  ≈9.83 GiB），采集后已按采集前状态还原。该副本是**外部临时资料**，随 0.3 计划落实后撤除，
  不作为本项目的长期测试夹具或门槛；后续批次的回归对照用 `stress_bench` 与 `stress_baseline`
  的合成场景。数值见 `0.3-baseline-2026-10-18.md`。
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
