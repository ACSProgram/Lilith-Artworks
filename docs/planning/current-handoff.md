# 当前任务交接

更新时间：2026-10-20

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-20 批次（文档整理：alpha3 归档与 0.3 计划归档，未提交，计入 alpha3 范围）：alpha3（原阶段
7–9）的实现已全部完成，`dev` 已切分为 `0.3.0-alpha.3`（未打标签）；阶段 7、8 的 GUI 手工验收已
通过，**阶段 9 的 GUI 手工验收暂时遗留**，验收项留在 `todo.md`。**0.3 本身尚未完成**——阶段 9
验收、发布前压力测试（含 `manual` 档）与是否打标签都还没有落定，本批次不把 0.3 记为已完成。

本批次把 alpha3 的批次记录从本文件清出，移入 `archive/handoff-log-2026-10-20.md`：阶段 7（画板
条目多态与 v4 → v5 迁移）、阶段 8（节点缩略图及其大画布 PSD 与手动刷新补充）、阶段 9（空闲完整性
检查）、历史节点缩略图悬停预览、画板文字定位与添加文字对话框、清理体系收口与检查点增量精简、
2026-10-19 晚的快速档压力测试复跑、`0.3.0-alpha.3` 切分与 CHANGELOG 写法调整；阶段 7 的人工验收
结论一并归档。

0.3 计划正文移入 `archive/0.3-architecture-algorithm-plan-2026-10-05.md`：文件头改为「阶段 1–9
已实现、0.3 未完成」，§2.1 与 §3 标为已完成，§4.2 补落地状态，§6 把阶段 7 的去抖窗口、最大纹理
边长与导出预算记为已决策。它自此只作为计划与落实口径的历史快照，不再作为执行依据。

同批次修正的落后口径：压力测试报告把当前开发构建写成 `0.3.0-alpha.2` / schema v5（schema v5 实际
由 alpha3 落地），改为 `0.3.0-alpha.3`；系统架构的存储布局仍写 `boards/<board-id>/<image-id>.dds`，
改为 `<item-id>.dds` 并把「阶段 7 的画板 kind 分派」改为不依赖阶段编号的表述；`0.3-baseline` 报告与
`research/compression-optimization/` 中指向 0.3 计划的路径改为归档位置。

## 下一步

- 阶段 9 的 GUI 手工验收（开关与计数、隐藏/最小化后每窗一批、让位、手动重查与失败展示），验收项
  见 `todo.md`；通过后再谈 alpha3 收口。
- 0.3 发布前由维护者显式安排压力测试（含 `manual` 档），并把实测写入
  `docs/user/stress-test-report.md`；是否切版本、打标签按 `docs/guides/release-policy.md` 单独决定。
- alpha2 遗留验收与 0.3 前的补强候选同样在 `todo.md`，不在本文件累积。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.3`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v5（alpha3 阶段 7 已落地）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2
  （设置页可回退到 v1）。0.3 alpha1 与 alpha2 保持 v4；唯一一次 v4 → v5 迁移已随阶段 7 实现，
  高于当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（阶段 0 一次性采集）：临时仓库副本 `D:\temp\Artworks`（3 作品 / 59 历史节点 /
  ≈9.83 GiB），采集后已按采集前状态还原。该副本是**外部临时资料**，随 0.3 发布后撤除，
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
