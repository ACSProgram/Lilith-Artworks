# 当前任务交接

更新时间：2026-10-17

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-17 批次完成三件事：

- **桌面验收收口**：任务调度总控与空闲链路校验批次的 5 项界面项，以及 2026-10-16 安全审查
  五项修复中涉及界面的部分，已随当前版本在其它设备完成的桌面验收一并执行并通过；验收记录与
  2026-10-16 维护者决策记录已归档至 `archive/handoff-log-2026-10-16.md`。空闲链路校验的已落地
  口径仍需按 0.3 计划 §3.I 重构（默认关闭、按空闲时机触发、全链恢复校对、`branch_notices`
  共享告警），安排在 0.3-alpha2 阶段 9，0.2 不再调整其口径。
- **切版本到 `0.2.0-rc.4`**：`CHANGELOG.md` 的 `## Unreleased` 落为带日期小节，五处版本字段与
  许可清单同步，`node tools/release/verify-metadata.mjs` 验证通过；未打标签。
- **README 收尾**：增加示例图（`.github/assets/demo.png`）并整理排版。

发布门槛现状（口径见 `todo.md` 第二节的维护者结论）：

- GitHub 私密漏洞报告渠道已开启，满足发行政策「正式版前必须启用并实测」的前置项；
- Authenticode 签名仍无法进行：按发行政策允许未签名发布，发布说明必须显著披露未签名状态及
  SmartScreen 影响；签名相关的人工门槛项（安装包、主程序与时间戳签名验证）在签名可用前无法
  执行。

未完成事项全部在 `todo.md`：0.3 前的补强候选（第一节）；0.3 计划
（`0.3-architecture-algorithm-plan-2026-10-05.md`）仍未实施；标签与 `0.2.0` 的切分后议。

## 当前基线

- 版本 `0.2.0-rc.4`（切版本，尚未打标签；上一个已打标签版本为 `v0.2.0-rc.2`），repository schema v4，应用标识 `com.lilith.artworks`。
- 版本口径（滞后版本号、`Unreleased` 暂存段与标签的关系）见 `docs/guides/release-policy.md`；切版本之后的改动记录在 `CHANGELOG.md` 的 `## Unreleased` 暂存段。
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
  段落与本文件中删除，不在表层留下「已通过」的痕迹。
- 历史增量的详细记录归 `archive/` 下对应日期的日志；版本、标签、人工门槛与产物口径归
  `docs/guides/release-policy.md`。
