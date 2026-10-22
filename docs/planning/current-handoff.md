# 当前任务交接

更新时间：2026-10-22

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-22 0.3 收尾与认证整改第七批（已提交，计划已归档）。**当前没有进行中的批次**：

- 取消发布在登记待清理项前按记录中的 SHA-256 核对仓库成品与认证副本；内容已变化的文件
  不删除、不进清理队列，保留路径写一条 `[cleanup] retained` 日志，交给设置页「空间」页的
  垃圾文件扫描发现。取消发布本身仍然成功。
- 质量预览缓存增加会话内上限：每次写入新缓存后保留最近 16 份，JPEG 总量超过 512 MiB 时继续
  从最旧的删，`.jpg` 与同名 `.json` 成对删除，淘汰失败静默忽略。
- 删除发布结果宽高、识别结果解码区域与估算响应源字节数三个无消费方字段；界面「原图」大小
  来自预览响应，不受影响。
- `docs/modules/authenticity.md`、`CHANGELOG.md` 的 `Unreleased` 与归档索引已同步；七批的
  逐批记录随计划归档到 `archive/0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md`
  第 14 节。

第三批（历史页视图、精简与悬停、设置六页的保存/取消/滚动）、第六批（发布/识别页、取消时序、
局部图与放大镜、只读记录、拖放导入）与第七批（取消发布保留被替换文件、预览缓存目录体积）
只做了自动验证与现有组件测试，标注为**待人工验收**，验收项在 `todo.md`。

## 下一步

- 人工验收第三、六批的界面行为与第七批的两条文件行为确认项；验收项见 `todo.md`。
- 验收通过后补记到 `archive/handoff-log-2026-10-22.md` 并从 `todo.md` 删除。
- 0.4 的草案（作品独立数据库、仓库升级规范化、控制总线按作品并行、识别页独立化）待完善，
  见 `0.4-artwork-database-upgrade-scheduling-and-identification-plan-draft-2026-10-22.md`。
- 发布前压力测试（含 `manual` 档）与是否打标签仍由维护者单独安排，口径见
  `docs/guides/release-policy.md`。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.6`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 阶段 7 落地，v5 → v6 随 alpha5 阶段 1 落地；alpha6
  沿用 v6）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 alpha1 与
  alpha2 保持 v4；**0.3 共有两次迁移**（alpha3 的 v4 → v5 与 alpha5 阶段 1 的 v5 → v6），高于
  当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（阶段 0 一次性采集）已归档到
  `archive/0.3-baseline-2026-10-18.md`；长期回归对照用 `stress_bench`。
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
