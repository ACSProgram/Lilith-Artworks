# 当前任务交接

更新时间：2026-11-03

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

架构审查整改的 **B1（文档事实、死代码、DTO 与前端反向依赖）、B2（右键菜单、保存契约与前端
去重）与 B3（Rust 摘要收敛、候选调查、生产文件拆分与应用层收口）已实施并通过自动化验证**；
改动范围与验证记录见 `architecture-review-remediation-plan-2026-11-03.md` 的对应「实施记录」。
B2 的右键菜单四项手工验收仍待维护者执行；B4、B5 未开始。

0.3 收尾 UI 修正批次的计划、实施与验证记录在
`archive/0.3-final-ui-fixes-plan-2026-11-03.md` 与 `archive/handoff-log-2026-11-03.md`
§10–§11；该批次的界面效果尚未经维护者手工确认，仍列在 `todo.md` 的「人工验收（剩余）」。

0.3 的产品工作已收口，剩余为人工验收与切版本准备。`dev` 上 beta.1 之后的改动都只记在
`CHANGELOG.md` 的 `Unreleased`，尚未切 `0.3.0`；这些改动之后没有复跑压力分组。

## 下一步

- 按整改计划进入 B4（`library` / `history` 仓储与 schema 拆分、跨域 SQL 登记与回滚测试）。
- 按 `todo.md` 的「人工验收（剩余）」逐项完成剩余人工验收，包括 B2 右键菜单的键盘/滚动/
  缩放/定位、历史页头三档宽度、空间页 busy 与后台任务下的草稿编辑。
- 切 `0.3.0` 前：跑 `stress_format` / `stress_damage`，用升级前的二进制造一份多节点旧仓库并在升级后
  逐节点 restore 比对，再补 `verify-metadata.mjs` 的 `0.3.0 → 6` 登记、按发行政策同步五处版本字段、
  跑 `npm run legal`、同步 `README.md` 与文档里的版本引用。
- 0.4 草案（作品独立数据库、仓库升级规范化、控制总线按作品并行、识别页独立化）已完成第二轮
  修订，清掉全部待决策项，待维护者批准；见
  `0.4-artwork-database-upgrade-scheduling-and-identification-plan-draft-2026-10-22.md`。
- 是否打 `v0.3.0` 标签、以及发布产物（NSIS、SBOM、签名披露）由维护者单独决定，口径见
  `docs/guides/release-policy.md`。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 的五处版本字段是 `0.3.0-beta.1`；`v0.3.0-beta.1` 已打标签并作为 Pre-release 公开
  （2026-10-25）。之后的改动全部写在 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者
  按 `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 落地，v5 → v6 随 alpha5 落地；alpha6 与
  beta.1 沿用 v6）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 的
  alpha1 与 alpha2 保持 v4；**0.3 共有两次迁移**（alpha3 的 v4 → v5 与 alpha5 的
  v5 → v6），高于当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（一次性采集）已归档到 `archive/0.3-baseline-2026-10-18.md`；该文档现为
  0.2 → 0.3.0-beta.1 的对比报告。长期回归对照用 `stress_bench`，其 release 条目已随
  2026-10-25 的全量压力测试同轮刷新。
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
