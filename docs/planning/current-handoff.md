# 当前任务交接

更新时间：2026-11-04

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

0.3 的产品工作已收口，当前处于「人工验收补测 + 正式版收尾」。`0.3.0-beta.1` 虚拟机验收发现的
1 个中等级 + 7 个低等级问题都已处理并落在 `dev`，逐条结论与实施记录见
`archive/0.3-vm-acceptance-fixes-plan-2026-11-04.md` 与 `archive/handoff-log-2026-11-04.md`
第 6–7 节；未复现的「识别页区域叠加层」保留复现步骤，等下一次复验再决定是否重开。

剩余工作只有两类：需要在候选构建上人工复验的项（`todo.md` 的「0.3 收尾人工复验」），以及
`todo.md` 里仍缺环境的功能门槛（干净用户首次启动 GUI、真实 GPU 素材板、正式版前的升级与复查）。

## 下一步

- 在候选构建上完成 `todo.md` 的「0.3 收尾人工复验」，包括按修复计划 §4.6 复查一次识别页区域
  叠加层。
- 复验后按 `todo.md` 的「0.3.0 正式版前」执行：逐字复查全部长期维护文档、旧仓库升级比对、
  切 `0.3.0` 并同步版本引用。
- 0.4 草案（作品独立数据库、仓库升级规范化、控制总线按作品并行、识别页独立化）已完成第二轮
  修订，清掉全部待决策项，待维护者批准；见
  `0.4-artwork-database-upgrade-scheduling-and-identification-plan-draft-2026-10-22.md`。
- 是否打 `v0.3.0-beta.2` 或 `v0.3.0` 标签、以及发布产物（NSIS、SBOM、签名披露）由维护者单独
  决定，口径见 `docs/guides/release-policy.md`。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 的五处版本字段是 `0.3.0-beta.2`，尚未打标签；公开的最新版本仍是
  `v0.3.0-beta.1`（2026-10-25，Pre-release）。切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 落地，v5 → v6 随 alpha5 落地）；ChunkFile 仓库可混存
  v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 共有两次迁移，高于当前版本的仓库由旧程序
  直接拒绝。
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
