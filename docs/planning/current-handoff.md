# 当前任务交接

更新时间：2026-10-17

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

无进行中的批次，也没有待验收的批次。

一条仍未关闭的产品边界：精简与全库校验真并发时，全库校验可能以「找不到历史节点」干净失败
（数据无损，错误与原因不匹配）。已登记 `todo.md` 第一节（P2），修法口径已定、待实施，并在
`docs/user/stress-test-report.md` 第 5 节标注为 ★ 边界项。

2026-10-16 晚维护者就 5 项待决策事项给出结论（标签与版本推进、两条 P1 的修法口径、
`scrub × compact` 并发错误口径，以及 P2/P3 两条硬化项是否同批）。结论见 `todo.md` 第三节，
完整口径写在第一节对应行内；这些口径**全部待实施**，落地时按 `docs/guides/validation.md` 补
针对性测试。

未完成事项全部在 `todo.md`：

- 待实施：按已定口径修分支工作文件路径校验、外部清理条目的启动自动重放、素材板导入/导出路径
  授权、TrustMark 模型完整性比对，以及内容级全库校验在写哨兵被持有时的明确拒绝；
- 待人工验收：任务调度总控与空闲链路校验批次的界面项（执行方式见 `todo.md` 第二节「怎么测」）；
- 标签与版本切分：本阶段不打标签，切版本与打标签后议。

## 当前基线

- 版本 `0.2.0-rc.3`（切版本，尚未打标签；上一个已打标签版本为 `v0.2.0-rc.2`），repository schema v4，应用标识 `com.lilith.artworks`。
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
