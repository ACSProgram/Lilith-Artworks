# 当前任务交接

更新时间：2026-10-19

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-19 批次（画板文字定位与添加文字对话框）：文字纹理改为按字形墨迹在每个行盒内
垂直居中并据此定位基线，修复文字整体偏上、顶部被纹理裁掉的问题，显示与导出共用同一条
绘制路径；添加文字对话框重新设计，加入内容预览、字号步进和内置取色器（饱和度/亮度、
色相、十六进制与预设色），不再调用系统取色器。`npm test`（242 通过）、
`npx tsc --noEmit`、`npm run build` 与 `git diff --check` 已通过；对话框预览、字号步进与内置
取色器，以及文字在纹理中的垂直定位，均已由维护者人工验收。

2026-10-19 批次（清理体系收口与检查点增量精简，已实现、自动验证通过）：设置页的「未引用文件
扫描」改为**垃圾文件扫描**——一次扫描报出两类候选：真正未被引用的崩溃孤儿/历史遗留，以及仍被
记录指向但当前存储模型已不读的冗余文件（新增契约 `ReferenceInventory::redundant_targets`）。
历史引用清单改按「真正会读的文件」计算（检查点已持有 snapshot 后的子边增量、节点已有 snapshot
时的兼容列 `history_nodes.delta_path` 不再进清单），确认清理时先由 `forget_deleted` 让领域清掉
指向冗余文件的多余记录、再入队删除，避免留下悬空引用。提交与精简不再为检查点父节点预写反向
增量（只登记空边保持「非根节点都有边」的图不变式），取消检查点时按需重建（旧仓库已有增量则
沿用），`load_node` 把空路径归一为 `None`、链解析遇空路径以带节点 id 的错误拒绝。本轮复核另修
一处隐患：重建增量与旧边同名时会误把刚写出的文件入队清理，现按路径不同才入队。`cargo test
--lib`（297 通过、1 忽略）、`npx tsc --noEmit`、`npm test`（238 通过）、`cargo fmt --check` 与
`git diff --check` 已通过。GUI 手工验收（扫描文案与结果列表、清理后取消检查点能重建增量）尚
未进行，记入 `todo.md`。维护者随后在真实仓库上执行了一次扫描与清理（`cleaned=4`、`pending=0`）：
清理后重新打开仓库被 `history_edges.delta_path 无效：仓库存储路径无效` 挡住——打开时的受控
路径校验把检查点边按设计登记的空 `delta_path` 当成非法相对路径。校验已改为跳过空串（空串
表示「没有增量」，与引用清单和 `load_node` 同口径），并补了「含空边的仓库可以打开」回归测试；
`cargo test --lib`（299 通过、1 忽略）通过，维护者随后在应用内重新打开该仓库成功。GUI 复验中
「重新打开仓库」已确认，取消检查点后重建增量一项仍待进行，见 `todo.md`。

2026-10-19 晚批次（快速档全套压力测试复跑）：在阶段 7–9 落地后按「除 `extreme` /
`manual` 外的全部分组」重跑，**全部通过**：便宜九组 44/44（`stress_cancel` 7、`stress_crash` 4、
`stress_cleanup` 1、`stress_damage` 6、`stress_format` 4、`stress_pin_board` 3、
`stress_authenticity` 5、`stress_concurrency` 9、`stress_input` 5）加 `gate_guard` 2/2、
`stress_scale` 5/5、`stress_large` 的 `default`（36.9 s）/ `large`（136.1 s）/ `heavy`（383.6 s）
各 8/8、`stress_bench` 9/9（单线程，未改写基线）、`stress_baseline` 2/2（未设
`LILITH_BASELINE_REPO`，真实仓库场景按设计跳过；取消延迟现在覆盖 6 个检查点）。
复跑暴露的是两处**测试侧漂移**，产品数据与校验均无异常：一是 `57dc8ba` 的缩略图提取插入了一枚
取消边界，提交检查点序号整体后移，B1/B4/B5 杀到错误位置；二是 `66ba8ec` 让全库校验改按前台写
命令取 owner lock，J6 断言的拒绝文案随之过期。修正方式见 `CHANGELOG.md` 的 `Unreleased → Fixed`：
序号收敛到 `stress_support` 的 `COMMIT_*` 常量、B4 改按阶段名定位、A1/基线循环覆盖 6 个边界、
J1 停回发布后、新增 A0 契约测试；J6 改为断言当前文案。`stress_concurrency` I/J 的复跑随之完成
（`todo.md` 对应条目已删）。`extreme` 与 `manual` 档仍只在正式发布前运行。

2026-10-19 批次（alpha3 阶段 9，已实现、自动验证通过）：0.2 的「空闲链路校验」改为节点级
**空闲完整性检查**。新设置 `idleIntegrityCheckEnabled` 默认关闭，入口在设置页「备份」页并显示
待校验节点数；主窗口隐藏或最小化、仓库静默 10 分钟后，每个空闲窗口最多跑一批（8 个节点或
30 秒，最旧优先），复用全库扫描的逐节点物化内核，节点边界对前台、手动提交、到期备份与取消
让位且不写当前节点状态。全库扫描现在刷新节点级校验状态并在单节点失败后继续；「重新校验此
分支」改为前台立即校验整条链；历史页分支状态改由整条链派生（任一节点失败即失败，否则取最旧
的通过时间）。精简、检查点建立/取消与 snapshot 替换在同一事务内按作品粗清校验状态。
`cargo test --lib`（292 通过）、`npx tsc --noEmit`、`npm test`（238 通过）、`cargo fmt --check`
与 `cargo check --all-targets` 已通过。GUI 手工验收（开关与计数、隐藏/最小化后每窗一批、让位、
手动重查与失败展示）尚未进行。批次取值与静默口径见计划 §2.4 的「落实记录」。

2026-10-19 批次（alpha3 阶段 7，已实现、自动验证通过）：画板条目已泛化为多态 item（位图 + 文字），并落地唯一
一次 v4 → v5 迁移：`pin_board_items` / `pin_board_bitmaps` / `pin_board_texts`、
`history_nodes` 缩略图与节点级校验列、删除 `branches` 旧校验三列、路径索引与
`repository_backup_runs`。渲染、清理、scrub、空间统计与导出按 kind 分派；文字由前端按同一套字体
度量栅格化，导出在浏览器编码 PNG 后经 `write_pin_board_item_png` 校验并原子写盘；备份页重启后
读取最近一次整仓备份。`cargo test --lib`（273 通过）、`npx tsc --noEmit`、`npm test`（235 通过）
与 `cargo fmt --check` 已通过，改动已提交；GUI 手工验收（文字清晰度、定位、导出、迁移后的旧仓库）
尚未进行。更新版本的仓库会被旧程序直接拒绝，避免降级写入。

2026-10-19 批次（alpha3 阶段 8，已实现、自动验证通过）：提交 worker 在内容变化时抽取节点
缩略图（`.clip` 内嵌 SQLite 的 `CanvasPreview`、`.psd`/`.psb` 优先 Image Resources 缩略图资源、
否则合并合成图，位图直接解码），以 512 px 无损 WebP + 版本 1 写入 v5 已有的
`history_nodes.thumbnail` / `thumbnail_version`，不进入 ChunkFile、scrub 或恢复；内容未变化的快速
检查、检查点、fork 与 snapshot 修复沿用已有缩略图，存量节点不回填。历史页「节点信息」在选中节点
时经 `get_history_node_thumbnail` 按需读取并显示，缺失或解析失败显示占位。新增的 preview 单测
（位图、合成 clip/psd raw/RLE/ZIP、PS 4.0 的 BGR 缩略图、损坏回退）、worker 写入/检查点保留测试
与前端展示测试通过；
`cargo test --lib`（286 通过）、`npx tsc --noEmit`、`npm test`（237 通过）、`cargo fmt --check`
与 `cargo check --all-targets` 已通过。GUI 手工验收（真实 `.clip`/`.psd` 预览、占位与详情区布局）
尚未进行。

2026-10-19 批次（下午）：**alpha2 收口、阶段 9 计划修订与 `0.3.0-alpha.2` 切分**。维护者验收
alpha2 并决定进入 alpha3：阶段 6（设置页拆「仓储 / 空间 / 备份」与仓储空间仪表盘）、同仓库
写入排队修复与超期临时目录清扫均通过验收并归档；写入闸门的人工验收仍未进行
（`stress_concurrency` I/J 的复跑已由 2026-10-19 晚的快速档全套完成，见本文件首段），
记入 `todo.md` 的「alpha2 遗留验收」。同日复核后把计划 §2.4 的「空闲链路校验重构」
改向为「空闲完整性检查」：复用全库扫描（`scrub_repository_integrity`）的逐节点物化
内核做空闲增量批次，校验状态挂 `history_nodes` 节点级列，`branches` 旧校验三列随 v5 迁移删除，
**不再建 `branch_notices` 告警表**（阶段 7 迁移范围随之缩减）；门控、让位与默认关闭口径沿用
2026-10-17 的决定。同日切分 `0.3.0-alpha.2`（未打标签的过渡版本）：五处版本字段同步，
`CHANGELOG.md` 的 `## Unreleased` 落为带日期小节，`verify-metadata.mjs` 登记 alpha.2 → schema v4
并验证通过，许可清单经 `npm run legal` 重新生成，README 与发行政策的版本引用已同步。本批次
记录见 `archive/handoff-log-2026-10-19.md` 第 7 节，阶段 9 的修订说明与取舍见
`0.3-architecture-algorithm-plan-2026-10-05.md` §2.4。

2026-10-19 批次：**alpha1 压力测试收尾与 CPU 时间度量已完成并归档**——`stress_large` 四档
（`default`/`large`/`heavy`/`extreme`，均 8/8）、`stress_scale`（5/5）、`stress_authenticity`
的 `extreme` 档（16K，5/5）、`stress_baseline`（2/2）与 `stress_bench`（9/9）全部通过；
`stress_bench` 新增子进程 CPU 时间（用户 + 内核）并重录了 release 基线；`stress_large` C4 的
账本口径按 v2 快照增长修正；严格门的短操作噪声误报（P2 签名、P8 孤儿扫描）改为三轮取最小后
连跑 5 次全通过。逐项数值、实现细节与遗留边界见
`archive/handoff-log-2026-10-19.md`，实测表见 `docs/user/stress-test-report.md`。
`stress_large` 的 `manual` 档（8 GiB）只在正式发布前运行，开发阶段与候选版本切分都不跑
（见 `docs/guides/release-policy.md`）。

- **0.2 已收口并归档**：`0be6224` 切分 `0.2.0`，`v0.2.0` 附注标签已打并推送；`main` 固定为 0.2
  发布线。切分内容、压力测试口径、自动与人工门槛、未签名发布说明见
  `archive/handoff-log-2026-10-18.md`。
- **0.3 alpha1（阶段 1–5）已实现并切分**：`dev` 自 `0be6224` 起落实统一命令运行时、设置单写入口
  与跨进程 owner lock、事件订阅与 controller 模板、ChunkFile v2 与容器感知分块、调度/清理/灾备
  算法；不改持久 schema（仓库保持 v4）。逐阶段实现记录、提交清单、各阶段落实口径与计划裁剪说明
  见 `archive/handoff-log-2026-10-18.md`；行为变化见 `CHANGELOG.md` 的 `## 0.3.0-alpha.1` 小节；
  落地契约见 `docs/architecture/overview.md` 与 `docs/modules/`。
- **下一步**：阶段 7、8 与 9 均已实现并完成自动验证，等待 GUI 手工验收；验收通过后再进入
  alpha3 收尾（文档、迁移说明、发布前压力测试与版本切分）。批次划分、依赖与阶段口径见
  `0.3-architecture-algorithm-plan-2026-10-05.md`。`manual` 档压力测试按发布前口径另行安排。
- alpha2 的实现记录（阶段 6 的设置拆分与仪表盘、写入闸门、临时目录清扫）已随验收归档，见
  `archive/handoff-log-2026-10-19.md` 第 7 节与 `archive/repository-write-lock-plan-2026-10-18.md`；
  行为变化见 `CHANGELOG.md` 的 `## 0.3.0-alpha.2` 小节。
- 未完成事项全部在 `todo.md`：alpha2 遗留验收项、alpha3 阶段 7–9 的 GUI 手工验收，以及 0.3 前的
  补强候选。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.2`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v5（alpha3 阶段 7 已落地）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2
  （设置页可回退到 v1）。0.3 alpha1 与 alpha2 保持 v4；唯一一次 v4 → v5 迁移已随阶段 7 实现，
  高于当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（阶段 0 一次性采集）：临时仓库副本 `D:\temp\Artworks`（3 作品 / 59 历史节点 /
  ≈9.83 GiB），采集后已按采集前状态还原。该副本是**外部临时资料**，随 0.3 计划落实后撤除，
  不作为本项目的长期测试夹具或门槛；后续批次的回归对照用 `stress_bench` 与 `stress_baseline`
  的合成场景。数值见 `0.3-baseline-2026-10-18.md`。
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
