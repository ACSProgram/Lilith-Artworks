# 规划归档索引

本目录只保存已经完成、被替代或不再作为当前执行依据的规划与批次记录。
进行中的工作读 `../current-handoff.md`，未完成事项读 `../todo.md`。

## 时间线

- `version-reset-2026-09-12.md`：版本基线重置为 `0.1.0` / repository schema v1，
  清除全部历史候选版与迁移链，含验收结果与发布确认。
- `pin-board-plan-2026-09-12.md`：素材板（pin-board）从 Lilith Client 迁入的原始规划，
  含设计决策、架构设计与 P1–P4 分阶段计划。已实施完毕，被 `docs/modules/pin-board.md` 取代。
- `pin-board-client-comparison-2026-09-13.md`：迁入后与 Lilith Client 原模块的逐文件对比
  报告，用于确认交互层原样迁移并定位回归。结论已并入模块文档。
- `pin-board-migration-2026-09-13.md`：素材板迁入及后续三个修复批次的完整记录与验证结果，
  以及仍在有效设计决策的汇总。
- `task-control-plan-2026-10-03.md`：任务调度总控与空闲链路校验（alpha3 批次 A–D）的规划，
  含 BackupState 任务类型与取消路由、schema v4 校验状态、调度器两级选择与失败警告面。
  已实施完毕，有效契约并入 `docs/modules/history-and-backup.md`，待人工验收项见 `../todo.md`。
- `cleanup-system-plan-2026-10-04.md`：统一清理体系（批次 A–F）的规划，覆盖画板结算改提交后
  清理、历史文件清理入队、未引用文件扫描、灾备暂存目录清扫、完整性扫描覆盖画板 DDS 与双向
  检查、可观测 UI 与文档收尾。六个批次已全部实施完毕，有效契约并入
  `docs/modules/history-and-backup.md`、`docs/modules/pin-board.md` 与
  `docs/architecture/overview.md`，待人工验收项见 `../todo.md`。
- `stress-test-plan-2026-10-04.md`：自动化压力测试规划（批次 1–8）。采用无界面命令行入口，
  以**独立进程**调用真实可执行文件，覆盖取消边界、跨进程强杀、事务中途崩溃、大文件端到端、
  规模与灾备、参数边界、崩溃孤儿回收、画板 DDS 完整性、认证发布内存与回读，以及损坏文件的
  检测与恢复。八个批次已全部实施完毕，有效契约并入 `docs/guides/validation.md`、
  `docs/guides/stress-test-report.md`，以及 `docs/modules/history-and-backup.md` 与
  `docs/modules/authenticity.md` 的「可靠性不变量与覆盖」；待人工验收项见 `../todo.md`。
