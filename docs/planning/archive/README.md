# 规划归档索引

本目录只保存已经完成、被替代或不再作为当前执行依据的规划、批次记录与验收结论，是**历史增量的唯一归属**。
表层文档只保留当前状况与未完成事项：进行中的工作读 `../current-handoff.md`，未完成事项读 `../todo.md`。

## 目录与命名

| 目录 | 收什么 |
| --- | --- |
| `handoff/` | 每日与批次的交接日志，含实施记录、验证结果与人工验收结论 |
| `plans/` | 独立计划与其逐批实施记录（含已被模块文档取代的原始规划） |
| `reviews/` | 审阅意见、审计报告与对比复核 |
| `records/` | 基线、压力测试轮次、迁移与版本重置、验收记录等一次性记录 |

- 命名：`YYYY-MM-DD-<主题>.md`，**日期在前**；跨若干天的文档写 `YYYY-MM-DD-to-YYYY-MM-DD-<主题>.md`
  （该区间只有一份主日志时可省略主题）。
- 同一天有多份同类文档时，主日志用纯日期，其余加主题后缀，如 `handoff/2026-10-09.md` 与
  `handoff/2026-10-09-pin-board.md`。
- 类型由目录表达，文件名不再重复写 `plan` 或 `handoff-log`；版本线写进主题，如
  `plans/2026-10-22-0.3-closeout-and-authenticity-refactor.md`。
- 新增归档时先归入对应目录，再按下表补一行；已被模块文档吸收的结论只写去向，不在本文件重述细节。
- 同一项工作既有计划正文又有按日期汇总的批次记录时，分别归入 `plans/` 与 `handoff/`，不互相复制。

## handoff

| 文档 | 内容 |
| --- | --- |
| [2026-09-12-to-2026-10-08.md](handoff/2026-09-12-to-2026-10-08.md) | 素材板迁入、任务调度总控、清理体系 A–F、压力测试批次 1–8、素材板退出握手等批次的实施细节、落实偏差与验证记录。 |
| [2026-10-09.md](handoff/2026-10-09.md) | 两个批次的人工验收，以及版本口径修正与用户文档分离等维护者决策。 |
| [2026-10-09-pin-board.md](handoff/2026-10-09-pin-board.md) | 素材板拖放导入、`Ctrl+A`、Shift 旋转吸附与「切换作品卡死」的调查与修复。 |
| [2026-10-16.md](handoff/2026-10-16.md) | 从表层清出的已验收批次与验收结果，含 0.2.0-rc 阶段的切分与发布门槛现状。 |
| [2026-10-18.md](handoff/2026-10-18.md) | 0.2 收口（切分 0.2.0、rc.4 压测口径、分支模型）与 0.3 alpha1 逐阶段记录。 |
| [2026-10-19.md](handoff/2026-10-19.md) | alpha1 压测收尾与子进程 CPU 时间度量；alpha2 收口，阶段 9 改向空闲完整性检查。 |
| [2026-10-20.md](handoff/2026-10-20.md) | alpha3 收口：阶段 7–9、缩略图悬停预览、检查点精简、拖放暂存清理与后台结算。 |
| [2026-10-21.md](handoff/2026-10-21.md) | alpha5 阶段 2–5 验收、画板内复制/橡皮擦/导出收敛，以及多批 GUI 验收。 |
| [2026-10-22.md](handoff/2026-10-22.md) | 错误边界与上下文拆分、本地 CI 预检、测试体检两批、alpha5/alpha6 切分等批次摘要。 |
| [2026-10-25.md](handoff/2026-10-25.md) | beta.1 发布前全量压力测试、版本切分、维护者逐项验收与表层文档收尾。 |
| [2026-11-03.md](handoff/2026-11-03.md) | 无头探针验收（51 条断言）与两轮 UI 验收，以及 0.3 收尾 UI 修正批次。 |
| [2026-11-04.md](handoff/2026-11-04.md) | beta.2 切分、文档纪律整改，以及 beta.1 虚拟机验收与两批修复。 |

## plans

| 文档 | 内容 |
| --- | --- |
| [2026-09-12-pin-board.md](plans/2026-09-12-pin-board.md) | 素材板自 Lilith Client 迁入的原始规划（P1–P4）；契约已并入 `docs/modules/pin-board.md`。 |
| [2026-10-03-task-control.md](plans/2026-10-03-task-control.md) | 任务调度总控与空闲链路校验（alpha3 批次 A–D）；契约并入 `history-and-backup.md`。 |
| [2026-10-04-cleanup-system.md](plans/2026-10-04-cleanup-system.md) | 统一清理体系批次 A–F；契约并入 history-and-backup、pin-board 与 overview。 |
| [2026-10-04-stress-test.md](plans/2026-10-04-stress-test.md) | 压力测试批次 1–8 的规划，以独立进程调用真实可执行文件。 |
| [2026-10-05-0.3-architecture-algorithm.md](plans/2026-10-05-0.3-architecture-algorithm.md) | 0.3 计划裁剪后的正文：批次划分、阶段 6–9 设计与 alpha2/alpha3 实施顺序；阶段 1–5 见原文快照。 |
| [2026-10-05-0.3-architecture-algorithm-alpha1.md](plans/2026-10-05-0.3-architecture-algorithm-alpha1.md) | 从上述计划裁出的 alpha1（阶段 1–5）原文快照，保留原 § 编号。 |
| [2026-10-18-alpha1-stress-failure-fix.md](plans/2026-10-18-alpha1-stress-failure-fix.md) | alpha1 压测 8 条未通过项的根因与修复（检查点膨胀、v1 快照体积、像素域回读）。 |
| [2026-10-18-repository-write-lock.md](plans/2026-10-18-repository-write-lock.md) | 同仓库写入排队的进程内闸门（Shutdown > Foreground > Interactive > Background）。 |
| [2026-10-20-backup-scheduling-fix.md](plans/2026-10-20-backup-scheduling-fix.md) | 自动备份调度与快速检查修复：指纹精度、代价感知与三段式有界管道。 |
| [2026-10-20-pin-board-strokes.md](plans/2026-10-20-pin-board-strokes.md) | 素材板笔迹、形状与图层（alpha5）：v5→v6 迁移、SDF 渲染与采样管道。 |
| [2026-10-20-platform-code-consolidation.md](plans/2026-10-20-platform-code-consolidation.md) | 平台代码集中：`platform/` 接口设计、`cfg` 边界与例外登记表。 |
| [2026-10-20-thumbnail-format-expansion.md](plans/2026-10-20-thumbnail-format-expansion.md) | 历史节点缩略图格式扩展：`.sai2` 内嵌预览、`.procreate` 与无魔数 `.tga`。 |
| [2026-10-21-pin-board-copy-eraser-export.md](plans/2026-10-21-pin-board-copy-eraser-export.md) | 画板内复制、橡皮擦与导出收敛，含整画布导出的离屏渲染通道。 |
| [2026-10-21-pin-board-structural-cleanup.md](plans/2026-10-21-pin-board-structural-cleanup.md) | 素材板结构收尾三批：行为保持的整理，外加两个正确性修复。 |
| [2026-10-22-0.3-closeout-and-authenticity-refactor.md](plans/2026-10-22-0.3-closeout-and-authenticity-refactor.md) | 0.3 收尾与认证模块结构整改七批的正文与逐批实施记录。 |
| [2026-10-24-pin-board-text-inline-editing.md](plans/2026-10-24-pin-board-text-inline-editing.md) | 素材板文字就地编辑两批：保存契约、撤销粒度与放置模式。 |
| [2026-11-03-0.3-final-ui-fixes.md](plans/2026-11-03-0.3-final-ui-fixes.md) | 检查点取消文案、历史页进度条、空间页 busy 与草稿输入分级禁用。 |
| [2026-11-03-0.3-release-blocker-fix.md](plans/2026-11-03-0.3-release-blocker-fix.md) | 正式版前单批修复：垃圾扫描补 `scan_candidates`，zstd 升级清 RUSTSEC 公告。 |
| [2026-11-03-architecture-review-remediation.md](plans/2026-11-03-architecture-review-remediation.md) | 架构审查整改 B1–B5：菜单统一、摘要收敛、仓储拆分与渲染器拆分。 |
| [2026-11-04-0.3-vm-acceptance-fixes.md](plans/2026-11-04-0.3-vm-acceptance-fixes.md) | beta.1 虚拟机验收 8 条问题的定位、决策与两批修复。 |

## reviews

| 文档 | 内容 |
| --- | --- |
| [2026-09-13-pin-board-client-comparison.md](reviews/2026-09-13-pin-board-client-comparison.md) | 迁入后与原模块的逐文件对比，用于确认原样迁移并定位回归。 |
| [2026-10-21-test-audit.md](reviews/2026-10-21-test-audit.md) | 13 组压力测试与单元测试的审计：建议退役的组、失效的严格门与新增建议。 |
| [2026-10-22-0.4-artwork-database-improvement-recommendations.md](reviews/2026-10-22-0.4-artwork-database-improvement-recommendations.md) | 0.4 拆库计划的第一轮架构审阅意见，采纳结论并入草案 §0.1。 |
| [2026-11-03-architecture-review-remediation-review.md](reviews/2026-11-03-architecture-review-remediation-review.md) | 对整改计划的复核意见；采纳与部分采纳的结论见该计划第 2 节。 |

## records

| 文档 | 内容 |
| --- | --- |
| [2026-09-12-version-reset.md](records/2026-09-12-version-reset.md) | 版本基线重置为 0.1.0 / schema v1，清除全部历史候选版与迁移链。 |
| [2026-09-13-pin-board-migration.md](records/2026-09-13-pin-board-migration.md) | 素材板迁入及后续三个修复批次的完整记录与验证结果。 |
| [2026-10-18-0.3-baseline.md](records/2026-10-18-0.3-baseline.md) | 0.3 阶段 0 的一次性基线采集，现为 0.2 与 beta.1 的同机对照报告。 |
| [2026-10-18-to-10-21-stress-test-rounds.md](records/2026-10-18-to-10-21-stress-test-rounds.md) | 从用户报告清出的历史轮次：alpha1 未通过项追因与三次复跑摘要。 |
| [2026-11-04-0.3.0-beta.1-vm-acceptance.md](records/2026-11-04-0.3.0-beta.1-vm-acceptance.md) | 隔离虚拟机验收：安装启动、作品树、历史恢复、备份、发布认证与破坏性用例。 |
