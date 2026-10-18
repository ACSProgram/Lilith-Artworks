# 当前任务交接

更新时间：2026-10-18

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-18 批次：**0.3 alpha1（阶段 1–5：统一命令运行时、设置单写入口与跨进程 owner lock、事件
订阅与 controller 模板、ChunkFile v2 与容器感知分块、调度/清理/灾备算法）已实现并切分为
`0.3.0-alpha.1`**；CI 通过。release 档整套压力测试（`manual` 8 GiB 档未跑）已于 2026-10-18 跑完并
留下 8 条未通过记录；这些记录已按 `alpha1-stress-failure-fix-plan-2026-10-18.md` 的阶段一处理完毕
（检查点分层、快照体积断言按 v2 口径改写、G4 夹具改到 512 级），并在本地复验：便宜九组 +
`gate_guard` 通过，`stress_large` 的 `default` 档 8/8 通过（含新增的 C8）。逐条根因与证据见
`docs/user/stress-test-report.md` 的「alpha1 未通过项：诊断与修复」；**alpha1 的收尾仍欠三项**：
release 档五档大文件存储数值未采集、`stress_authenticity` 的 `extreme` 档未复跑、以及
`stress_concurrency` J5 的既有偶发（约 15%，与本次修复无关，机制见该报告「边界」）。三项都记在
`todo.md`，另有计划的阶段二（小选区水印警告）尚未实现。

- **0.2 已收口并归档**：`0be6224` 切分 `0.2.0`，`v0.2.0` 附注标签已打并推送；`main` 固定为 0.2
  发布线。切分内容、压力测试口径、自动与人工门槛、未签名发布说明见
  `archive/handoff-log-2026-10-18.md`。
- **0.3 alpha1（阶段 1–5）已实现并切分**：`dev` 自 `0be6224` 起落实统一命令运行时、设置单写入口
  与跨进程 owner lock、事件订阅与 controller 模板、ChunkFile v2 与容器感知分块、调度/清理/灾备
  算法；不改持久 schema（仓库保持 v4）。逐阶段实现记录、提交清单、各阶段落实口径与计划裁剪说明
  见 `archive/handoff-log-2026-10-18.md`；行为变化见 `CHANGELOG.md` 的 `## 0.3.0-alpha.1` 小节；
  落地契约见 `docs/architecture/overview.md` 与 `docs/modules/`。
- **下一步**：先收口 `todo.md` 里 alpha1 的剩余项（release 档五档大文件数值采集、`extreme` 档
  复跑、J5 偶发与度量口径），并视维护者安排实现计划的阶段二（小选区水印警告）；随后进入 alpha2
  （阶段 6 + 针对 alpha1 的测试与修复），再到 alpha3（原阶段 7–9，含唯一一次 v4 → v5 迁移）。
  批次划分、依赖与阶段口径见 `0.3-architecture-algorithm-plan-2026-10-05.md`。
- 未完成事项全部在 `todo.md`：alpha1 阶段 1–5 的待验收项与压力测试剩余项、计划的阶段二、0.3 前的
  补强候选、阶段 6–9。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.1`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v4；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。
  0.3 alpha1 与 alpha2 保持 v4（alpha1 的清理仅用连接内临时表），唯一一次 v4 → v5 迁移由 alpha3
  阶段 7 执行。
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
