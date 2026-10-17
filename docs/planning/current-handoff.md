# 当前任务交接

更新时间：2026-10-17

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-17 批次：**`0.2.0` 正式版切分（已准备，未提交、未打标签）**。

- **切版本到 `0.2.0`**：`CHANGELOG.md` 新建带日期小节 `## 0.2.0 - 2026-10-17`，内容并入
  `0.2.0-rc.4` 小节（两者产品代码一致——rc.4 切分后没有任何源码改动；版本号条目按正式版口径改写，
  并补入压测、桌面验收与已知限制三条，以及打标签前的发布基建修正：release workflow 对稳定
  标签不再标 prerelease 并更名、verify-metadata 补 Cargo.lock 版本校验、文档与 CI 口径对齐），
  rc.4 小节按「一经建立即冻结」保持原样；顶部补回空的
  `## Unreleased` 暂存段（rc.4 切分时遗漏）。五处版本字段、许可清单与 README 版本行同步到
  `0.2.0`，`node tools/release/verify-metadata.mjs` 验证通过（v0.2.0、`com.lilith.artworks`、
  schema v4）。
- **发布门槛现状**（口径见 `todo.md` 第二节的维护者结论）：
  - rc.4 的全套餐压力测试（按维护者口径即本正式版的压力测试，同一份产品代码）已于 2026-10-17 在
    release 构建上完整重跑并通过（八组便宜分组 + gate_guard 40 用例、C 组五档 7/7 含 C7 首次
    随五档运行、G 组 extreme 档、D/E/F 规模组、P 组基准复测同量级），实测数据已补入
    `docs/user/stress-test-report.md`（被测版本口径写 `0.2.0`）；
  - 自动门槛 1–5：Windows CI 已在 rc.4 上通过（维护者核实）；本次切分提交推送后由
      windows-ci 重跑确认；正式版门槛 6–10 由 `v0.2.0` 标签触发的 release workflow 执行
      （依赖审计、许可一致性、从标签构建 NSIS 并静默安装检查、SBOM、校验和、attestation、
      草稿 release）；
  - 人工门槛：干净 Windows 用户环境全清单（2026-10-16）与任务调度总控、空闲链路校验及安全
    批次的界面项（2026-10-17，随 rc.4）均已通过；0.2.0 与 rc.4 产品代码一致，验收直接沿用，
    记录见 `archive/handoff-log-2026-10-16.md`；
  - GitHub 私密漏洞报告渠道已开启，满足发行政策「正式版前必须启用并实测」的前置项；
  - Authenticode 签名仍无法进行：按发行政策允许未签名发布，发布说明必须显著披露未签名状态及
    SmartScreen 影响；签名相关的人工门槛项（安装包、主程序与时间戳签名验证）在签名可用前无法
    执行。
- **待维护者动作**：提交本次切分（含压测报告的实测数据更新）；决定并推送 `v0.2.0` 标签
  （触发 release workflow 生成草稿 release，正式版本标签不再标为 prerelease）。
- 未完成事项全部在 `todo.md`：0.3 前的补强候选（第一节）；0.3 计划
  （`0.3-architecture-algorithm-plan-2026-10-05.md`）仍未实施。

## 当前基线

- 版本 `0.2.0`（切分已准备，未提交、未打标签；最近一个已打标签版本为 `v0.2.0-rc.4`，与
  本版本产品代码一致），repository schema v4，应用标识 `com.lilith.artworks`。
- 版本口径（滞后版本号、`Unreleased` 暂存段与标签的关系）见 `docs/guides/release-policy.md`；
  切版本之后的改动记录在 `CHANGELOG.md` 的 `## Unreleased` 暂存段。
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
