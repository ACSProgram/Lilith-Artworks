# 当前任务交接

更新时间：2026-10-25

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-25 已完成 `0.3.0-beta.1` 的切分与收尾。切版本本身：`CHANGELOG.md` 的 `Unreleased` 落为
`## 0.3.0-beta.1 - 2026-10-25`，五处版本字段、许可清单、`README.md` 与 `verify-metadata.mjs` 的
版本→schema 登记表（`beta.1 → 6`）同轮同步，`verify-metadata` 通过；切版本不触发任何门槛。
同日完成 0.3 线的发布前全量压力测试（含 `manual` 8 GiB 档，五档各 10/10，无产品侧失败）与表层
文档收尾：`todo.md` 精简为待验收与待补强两段，`docs/README.md` 的归档索引改回当前事实，根
`README.md` 按当前功能与状态重写。记录见 `archive/handoff-log-2026-10-25.md` 第 1–11 节，实测
数据见 `docs/user/stress-test-report.md`，与 0.2 的对比见
`archive/0.3-baseline-2026-10-18.md` 第 7 节。

0.3 的实现与文档已收口，人工验收正在逐项进行：已通过的项逐条记入
`archive/handoff-log-2026-10-25.md` 第 10 节并从 `todo.md` 删除；未通过或未执行的项继续留在
`todo.md`。

## 下一步

- 继续逐项人工验收 `todo.md` 中其余待验收项（界面行为、桌面集成、迁移与文件清理确认）。
- 0.4 草案（作品独立数据库、仓库升级规范化、控制总线按作品并行、识别页独立化）已完成第二轮
  修订，清掉全部待决策项，待维护者批准；见
  `0.4-artwork-database-upgrade-scheduling-and-identification-plan-draft-2026-10-22.md`。
- 是否打 `v0.3.0-beta.1` 标签、以及发布产物（NSIS、SBOM、签名披露）由维护者单独决定，口径见
  `docs/guides/release-policy.md`。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-beta.1`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 阶段 7 落地，v5 → v6 随 alpha5 阶段 1 落地；alpha6 与
  beta.1 沿用 v6）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 的
  alpha1 与 alpha2 保持 v4；**0.3 共有两次迁移**（alpha3 的 v4 → v5 与 alpha5 阶段 1 的
  v5 → v6），高于当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（阶段 0 一次性采集）已归档到 `archive/0.3-baseline-2026-10-18.md`；该文档现为
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
