# 当前任务交接

更新时间：2026-10-20

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-20 批次（平台代码集中，已提交）：平台专属逻辑收口到 `src-tauri/src/platform/`，
新增可移植性规范 `docs/guides/platform.md`，Windows CI 增加「平台分支只准出现在
`platform/` 内」的 grep 把关。计划与实施记录见
`archive/platform-code-consolidation-plan-2026-10-20.md`；行为零变化（纯机械搬移 +
一轮不改变行为的整理），自动验证已通过，维护者已做初步手工确认但**未覆盖全部界面入口**，
剩余 GUI 验收项在 `todo.md`。

上一批（alpha3 收口与文档整理，已提交）的批次记录见 `archive/handoff-log-2026-10-20.md`：
阶段 7、8 的人工验收已通过，**阶段 9 的 GUI 手工验收暂时遗留**；**0.3 本身尚未完成**——
阶段 9 验收、发布前压力测试（含 `manual` 档）与是否打标签都还没有落定，本批次不把 0.3
记为已完成。

## 下一步

- 自动备份调度与快速检查修复的**阶段 1–3** 已于 2026-10-20 实施并通过自动验证：
  阶段 1 是 A1 指纹门槛 + A3 退化日志；阶段 2 是 A4 用户写命令后静默 10 秒 + A2 调度器
  代价感知；阶段 3 是 A5 让位改事件驱动 + A6 后台完成事件区分结构变化。维护者明确跳过了
  阶段 2 原定的一天日志门槛，**GUI 手工验收与日志观察仍待人工**（验收项在 `todo.md`）。
  阶段 4–5 已按 release 实测重新定范围（阶段 4：删掉提交路径的旧快照整份复检 + 结构单元改
  缓冲顺序读；阶段 5：提交与恢复的串行流水改有界管道），实测与依据见
  `backup-scheduling-fix-plan-2026-10-20.md` §6–§8，被该计划移出的长期项在 `todo.md`。
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
