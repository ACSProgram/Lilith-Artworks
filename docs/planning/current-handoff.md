# 当前任务交接

更新时间：2026-11-03

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

发布阻塞修复批次（`archive/0.3-release-blocker-fix-plan-2026-11-03.md`）已完成并提交到 `dev`：
轨道 A 给 `ArtifactReferenceInventory` 补上 `scan_candidates`，
取消发布时因内容被替换而保留的成品副本与认证副本现在会被「空间」页的垃圾扫描列出；轨道 B 把 zstd
升到 0.14，四条 RUSTSEC 公告清零。同轮把新出现的 `RUSTSEC-2026-0429`（zip 8.6.0）按 0367/0368/0369
的口径加进 CI ignore 并在 `todo.md` 记明移除条件。验证：`cargo test --lib` 441 通过、
`cargo audit` 零漏洞、`cargo fmt --check`、`npx tsc --noEmit`、`npm test` 56 文件 / 502 项全绿、
`npm run legal` 已重新生成。细节与遗留项见 `archive/handoff-log-2026-11-03.md` §7。

2026-11-03 用一次性无头探针把 0.3 人工验收里能程序判定的部分跑完，结论见
`archive/handoff-log-2026-11-03.md`：十个场景 51 条断言，50 条通过；唯一失败项是「被替换后保留
的成品副本不会被垃圾扫描发现」，已由上面的修复批次闭合。同轮核对到既有前端与单元测试已覆盖
精简确认、取消提示、只读记录、长任务 busy 拒绝、错误边界、历史重置确认与缩略图格式，因此没有
新增永久测试。探针脚本、临时无头子命令与构建产物已全部删除/回退。

2026-10-25 的批次见 `archive/handoff-log-2026-10-25.md`：`0.3.0-beta.1` 切分、全量压力测试
（含 `manual` 8 GiB）与表层文档收尾。beta.1 已于同日打标签并以 Pre-release 公开，附 setup.exe、
SBOM 与校验和；`todo.md` 里此前「七个版本都没有打标签」的说法已按事实更正。

beta.1 之后 `dev` 上已有四个提交，都只记在 `CHANGELOG.md` 的 `Unreleased`、尚未切版本：历史重置
（`9dd5f5e`）、托盘备份勾选项（`4bf74bd`）、图标与 NSIS 图标（`7a6cb14`）与本批次。本批次之后
没有复跑任何压力分组。

## 下一步

- 继续逐项人工验收 `todo.md` 中剩余项（界面观感、桌面集成、迁移与文件清理确认）；其中
  「替换副本 → 取消发布 → 过宽限期 → 空间页扫描 → 确认清理」是本批次唯一的人工验收步骤。
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
