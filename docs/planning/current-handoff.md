# 当前任务交接

更新时间：2026-10-20

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-20 缩略图格式扩展（第一批，已提交）：节点缩略图新增三种来源——`.sai2`（libsai 移植，
`intg` 画布优先、`thum` 兜底）、`.procreate`（ZIP 内的 `QuickLook/Thumbnail.png`）与无魔数的
`.tga`，`supports_preview` 同步登记；其余候选格式由维护者决定暂缓。计划、格式依据与实施记录已
归档：`archive/thumbnail-format-expansion-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## Unreleased`。自动验证已通过（含 `intg` 在 3 个真实样本上与 SAI 导出 PNG 的逐像素比对），
**GUI 手工验收与 `.procreate` 真实样本验证仍待人工**（验收项在 `todo.md`）。

2026-10-20 批次（自动备份调度与快速检查修复，阶段 1–5，已提交）：快速检查的指纹门槛、调度侧
静默与代价感知、让位改事件驱动、后台完成事件收敛、提交路径删除 head snapshot 整份复检、
结构单元改缓冲顺序读，以及提交与恢复导出的三段式有界管道全部落地。计划、release 实测与实施
记录已归档：`archive/backup-scheduling-fix-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## Unreleased`。自动验证已通过，**GUI 手工验收、一天日志观察与 release 下同一 1.378 GB
分支的提交/恢复复测仍待人工**（验收项在 `todo.md`）。

上一批（平台代码集中，已提交）：平台专属逻辑收口到 `src-tauri/src/platform/`，新增可移植性规范
`docs/guides/platform.md`，Windows CI 增加「平台分支只准出现在 `platform/` 内」的 grep 把关。
计划与实施记录见 `archive/platform-code-consolidation-plan-2026-10-20.md`；行为零变化（纯机械
搬移 + 一轮不改变行为的整理），自动验证已通过，维护者已做初步手工确认但**未覆盖全部界面入口**，
剩余 GUI 验收项在 `todo.md`。

上一批（alpha3 收口与文档整理，已提交）的批次记录见 `archive/handoff-log-2026-10-20.md`：
阶段 7、8 的人工验收已通过，**阶段 9 的 GUI 手工验收暂时遗留**；**0.3 本身尚未完成**——
阶段 9 验收、发布前压力测试（含 `manual` 档）与是否打标签都还没有落定，本批次不把 0.3
记为已完成。

## 下一步

- 缩略图格式扩展的 GUI 手工验收：选中 `.sai2` / `.procreate` / `.tga` 分支的节点，逐档确认图片、
  「无可用预览」占位与「格式不支持」三种表现；`.procreate` 的真实文件仍只有合成 ZIP 单测。
  验收项见 `todo.md`。
- 自动备份调度修复的阶段 2–5 待人工验证：GUI 手工验收（静默窗、调度器代价感知、让位不空转、
  后台完成事件只合并分支）、一天日志观察，以及 release 下同一 1.378 GB 分支的提交/恢复复测
  （与计划 §6.1 的分段耗时逐项对照）。验收项见 `todo.md`，实测与依据见
  `archive/backup-scheduling-fix-plan-2026-10-20.md` 的 §6–§8。
- 平台批次剩余的 GUI 手工验收（「在文件夹中显示」、设置页三个目录入口、素材板粘贴导入），
  验收项见 `todo.md`。
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
- 平台层：`src-tauri/src/platform/` 是产品代码里平台分支的唯一入口（路径表示、文件管理器、
  剪贴板、进程信息），业务文件不写 `cfg` 分支，由 Windows CI 的 grep 检查把关；规则、例外登记表
  与「加平台清单」见 `docs/guides/platform.md`。这不等于已经支持 macOS。
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
