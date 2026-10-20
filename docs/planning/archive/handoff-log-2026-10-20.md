# 2026-10-20 归档：alpha3 收口、缩略图悬停预览与版本切分

- 归档日期：2026-10-20
- 范围：从 `../current-handoff.md` 清出的 2026-10-19 至 2026-10-20 批次记录——alpha3 阶段 7–9、
  历史节点缩略图与悬停预览、画板文字定位与添加文字对话框、清理体系收口、快速档压力测试复跑、
  `0.3.0-alpha.3` 切分与文档整理。
- 状态：alpha1、alpha2 与 alpha3 的实现均已完成，alpha3 切分为 `0.3.0-alpha.3`（未打标签）。
  阶段 7、8 的 GUI 手工验收已通过，**阶段 9 的 GUI 手工验收暂时遗留**，记于 `../todo.md`。
  **0.3 尚未完成**：发布前压力测试与是否打标签仍由维护者决定。

## 1. alpha3 阶段 7：画板条目多态与 v4 → v5 迁移（已实现，人工验收通过）

画板条目已泛化为多态 item（位图 + 文字），并落地唯一一次 v4 → v5 迁移：`pin_board_items` /
`pin_board_bitmaps` / `pin_board_texts`、`history_nodes` 缩略图与节点级校验列、删除 `branches`
旧校验三列、路径索引与 `repository_backup_runs`。渲染、清理、scrub、空间统计与导出按 kind 分派；
文字由前端按同一套字体度量栅格化，导出在浏览器编码 PNG 后经 `write_pin_board_item_png` 校验并
原子写盘；备份页重启后读取最近一次整仓备份。`cargo test --lib`（273 通过）、`npx tsc --noEmit`、
`npm test`（235 通过）与 `cargo fmt --check` 已通过，改动已提交。更新版本的仓库会被旧程序直接
拒绝，避免降级写入。

**人工验收（2026-10-20，通过）**：维护者确认阶段 7 的功能与界面通过人工验证。

## 2. alpha3 阶段 8：节点缩略图（已实现，人工验收通过）

提交 worker 在内容变化时抽取节点缩略图（`.clip` 内嵌 SQLite 的 `CanvasPreview`、`.psd`/`.psb`
优先 Image Resources 缩略图资源、否则合并合成图，位图直接解码），以 512 px 无损 WebP + 版本 1
写入 v5 已有的 `history_nodes.thumbnail` / `thumbnail_version`，不进入 ChunkFile、scrub 或恢复；
内容未变化的快速检查、检查点、fork 与 snapshot 修复沿用已有缩略图，存量节点不回填。历史页
「节点信息」在选中节点时经 `get_history_node_thumbnail` 按需读取并显示，缺失或解析失败显示占位。
新增的 preview 单测（位图、合成 clip/psd raw/RLE/ZIP、PS 4.0 的 BGR 缩略图、损坏回退）、worker
写入/检查点保留测试与前端展示测试通过；`cargo test --lib`（286 通过）、`npx tsc --noEmit`、
`npm test`（237 通过）、`cargo fmt --check` 与 `cargo check --all-targets` 已通过。

**补充批次（2026-10-20）：大画布 PSD 预览与缩略图手动刷新**。`.psd`/`.psb` 预览不再因为 64 MP
共享解码预算在读取内嵌缩略图之前就被拒绝——头部校验只保留非零尺寸，画布预算下沉到合成图解码时
判定（256 MP，逐行采样使内存只与 512 px 输出相关）；内嵌缩略图最长边小于 256 px 时改解合并
合成图，避免把很小的位图放大成模糊预览，合成图不可用（超预算、截断、格式不支持）时回退到内嵌
缩略图，两者都没有才无预览。历史页「节点信息」新增「重新获取缩略图」按钮（无预览时在占位旁、
已有预览时叠在右上角），经 `refresh_history_node_thumbnail` 沿历史链物化该节点**自身存储的内容**
到仓库根 `temp/` 后交给同一套 `preview` 抽取，并把结果写回 `thumbnail` / `thumbnail_version`；
不读取当前工作文件，不进入 ChunkFile、图或校验状态，抽取失败不改仓库。`cargo test --lib`
（303 通过、1 忽略）、`npm test`（243 通过）、`npx tsc --noEmit`、`cargo fmt --check` 与
`git diff --check` 已通过。

**人工验收（2026-10-20，通过）**：维护者确认功能与界面通过人工验收，含大画布 `.psd`/`.psb`
预览与详情区「重新获取缩略图」。

## 3. alpha3 阶段 9：空闲完整性检查（已实现、自动验证通过，GUI 手工验收暂时遗留）

0.2 的「空闲链路校验」改为节点级**空闲完整性检查**。新设置 `idleIntegrityCheckEnabled` 默认
关闭，入口在设置页「备份」页并显示待校验节点数；主窗口隐藏或最小化、仓库静默 10 分钟后，每个
空闲窗口最多跑一批（8 个节点或 30 秒，最旧优先），复用全库扫描的逐节点物化内核，节点边界对
前台、手动提交、到期备份与取消让位且不写当前节点状态。全库扫描现在刷新节点级校验状态并在单
节点失败后继续；「重新校验此分支」改为前台立即校验整条链；历史页分支状态改由整条链派生（任一
节点失败即失败，否则取最旧的通过时间）。精简、检查点建立/取消与 snapshot 替换在同一事务内按
作品粗清校验状态。`cargo test --lib`（292 通过）、`npx tsc --noEmit`、`npm test`（238 通过）、
`cargo fmt --check` 与 `cargo check --all-targets` 已通过。批次取值与静默口径见
`0.3-architecture-algorithm-plan-2026-10-05.md` §2.4 的「落实记录」。

**验收状态**：GUI 手工验收（开关与计数、隐藏/最小化后每窗一批、让位、手动重查与失败展示）
**暂时遗留**（2026-10-20），验收项留在 `../todo.md`。

## 4. 历史节点缩略图悬停预览（已实现、自动验证通过）

历史页**总览**的节点卡片在指针悬停时显示该节点已存的缩略图——有缩略图显示图片，没有缩略图但
格式支持时显示「无可用预览」与刷新按钮（与「节点信息」栏同款），格式不支持时不出现任何浮层，
悬停行为与未加该功能时完全一致；当前分支视图不提供浮层，缩略图仍只在「节点信息」栏显示，避免
同一张图出现两次。「节点信息」栏同样按格式分档：不支持的格式整块不显示预览区，而不是显示占位。
「格式是否支持」由 `preview::supports_preview` 按节点所属分支**当前工作文件**的扩展名判断
（`.clip`、`.psd`/`.psb` 与 png/jpg/webp/tiff/bmp/gif 支持），`get_history_node_thumbnail` 因此
改为返回 `{ dataUrl, supported }`；节点自身只存 ChunkFile 容器、读不出原始格式，所以这条判断
跟着分支工作文件走，分支改指到另一种格式后老节点也按新扩展名回答。悬停浮层挂在页面根部、按卡片
视口矩形固定定位，右侧放不下时翻到左侧，指针移入浮层有宽限时间，滚动、窗口缩放或切到分支视图
时关闭；缩略图按节点缓存在内存里，历史刷新时整份作废，悬停与浮层内刷新共用同一份状态，不进入
图轮询。浮层沿用既有视觉语言：1px 边框、6px 圆角、与右键菜单同款阴影，空状态与刷新按钮直接复用
「节点信息」栏那一套；缩略图只加一道 1px 描边（浅色画面与透明区域更容易看出边界），浮层空状态
给最小高度，不引入新的视觉元素。`cargo test --lib`（305 通过、1 忽略）、`npm test`（248 通过）、
`npx tsc --noEmit`、`cargo fmt --check` 与 `git diff --check` 已通过。

## 5. 画板文字定位与添加文字对话框（已实现，人工验收通过）

文字纹理改为按字形墨迹在每个行盒内垂直居中并据此定位基线，修复文字整体偏上、顶部被纹理裁掉的
问题，显示与导出共用同一条绘制路径；添加文字对话框重新设计，加入内容预览、字号步进和内置取色器
（饱和度/亮度、色相、十六进制与预设色），不再调用系统取色器。`npm test`（242 通过）、
`npx tsc --noEmit`、`npm run build` 与 `git diff --check` 已通过。

**人工验收（2026-10-19，通过）**：对话框预览、字号步进与内置取色器，以及文字在纹理中的垂直
定位，均由维护者确认。

## 6. 清理体系收口与检查点增量精简（已实现、自动验证通过，界面项待验收）

设置页的「未引用文件扫描」改为**垃圾文件扫描**——一次扫描报出两类候选：真正未被引用的崩溃
孤儿/历史遗留，以及仍被记录指向但当前存储模型已不读的冗余文件（新增契约
`ReferenceInventory::redundant_targets`）。历史引用清单改按「真正会读的文件」计算（检查点已持有
snapshot 后的子边增量、节点已有 snapshot 时的兼容列 `history_nodes.delta_path` 不再进清单），
确认清理时先由 `forget_deleted` 让领域清掉指向冗余文件的多余记录、再入队删除，避免留下悬空
引用。提交与精简不再为检查点父节点预写反向增量（只登记空边保持「非根节点都有边」的图不变式），
取消检查点时按需重建（旧仓库已有增量则沿用），`load_node` 把空路径归一为 `None`、链解析遇空
路径以带节点 id 的错误拒绝。本轮复核另修一处隐患：重建增量与旧边同名时会误把刚写出的文件入队
清理，现按路径不同才入队。`cargo test --lib`（297 通过、1 忽略）、`npx tsc --noEmit`、`npm test`
（238 通过）、`cargo fmt --check` 与 `git diff --check` 已通过。GUI 手工验收（扫描文案与结果
列表、清理后取消检查点能重建增量）留在 `../todo.md`。

维护者随后在真实仓库上执行了一次扫描与清理（`cleaned=4`、`pending=0`）：清理后重新打开仓库被
`history_edges.delta_path 无效：仓库存储路径无效` 挡住——打开时的受控路径校验把检查点边按设计
登记的空 `delta_path` 当成非法相对路径。校验已改为跳过空串（空串表示「没有增量」，与引用清单和
`load_node` 同口径），并补了「含空边的仓库可以打开」回归测试；`cargo test --lib`（299 通过、
1 忽略）通过，维护者随后在应用内重新打开该仓库成功。GUI 复验中「重新打开仓库」已确认，取消
检查点后重建增量一项仍待进行，见 `../todo.md`。

## 7. 快速档全套压力测试复跑（2026-10-19 晚，全部通过）

在阶段 7–9 落地后按「除 `extreme` / `manual` 外的全部分组」重跑，**全部通过**：便宜九组
44/44（`stress_cancel` 7、`stress_crash` 4、`stress_cleanup` 1、`stress_damage` 6、
`stress_format` 4、`stress_pin_board` 3、`stress_authenticity` 5、`stress_concurrency` 9、
`stress_input` 5）加 `gate_guard` 2/2、`stress_scale` 5/5、`stress_large` 的 `default`
（36.9 s）/ `large`（136.1 s）/ `heavy`（383.6 s）各 8/8、`stress_bench` 9/9（单线程，未改写
基线）、`stress_baseline` 2/2（未设 `LILITH_BASELINE_REPO`，真实仓库场景按设计跳过；取消延迟
现在覆盖 6 个检查点）。

复跑暴露的是两处**测试侧漂移**，产品数据与校验均无异常：一是 `57dc8ba` 的缩略图提取插入了一枚
取消边界，提交检查点序号整体后移，B1/B4/B5 杀到错误位置；二是 `66ba8ec` 让全库校验改按前台写
命令取 owner lock，J6 断言的拒绝文案随之过期。修正方式见 `../../../CHANGELOG.md` 的
`0.3.0-alpha.3 → Fixed`：序号收敛到 `stress_support` 的 `COMMIT_*` 常量、B4 改按阶段名定位、
A1/基线循环覆盖 6 个边界、J1 停回发布后、新增 A0 契约测试；J6 改为断言当前文案。
`stress_concurrency` I/J 的复跑随之完成。`extreme` 与 `manual` 档仍只在正式发布前运行。

## 8. `0.3.0-alpha.3` 切分与 CHANGELOG 写法调整（2026-10-20，未打标签）

把 `CHANGELOG.md` 的 `## Unreleased` 落为带日期的 `## 0.3.0-alpha.3 - 2026-10-20` 小节，五处
版本字段同步为 `0.3.0-alpha.3`，`tools/release/verify-metadata.mjs` 登记 alpha.3 → repository
schema v5，许可清单经 `npm run legal` 重新生成，README、发行政策与 0.3 计划的版本引用同步更新；
`npm run verify:release-metadata` 输出 `v0.3.0-alpha.3, com.lilith.artworks, schema v5`。同时按
「按改动分量写」的口径重写了该小节：大功能仍占一段、说清边界与代价，小修压到一两句，去掉成串的
字段清单、调用链与逐步骤实现过程，顺序改按影响排。这条口径已写入
`docs/guides/release-policy.md` 的「CHANGELOG 写法」，`AGENTS.md` 的版本口径条目指向它。本次
只切版本、未打标签。

## 9. 文档整理：alpha3 归档与 0.3 计划归档（2026-10-20）

alpha3 的实现已全部完成，本批次把表层文档收回到「只写当前状况」的口径：

- 本文件从 `../current-handoff.md` 接管 alpha3 阶段 7–9、缩略图悬停预览、画板文字、清理收口与
  压测复跑的批次记录；阶段 7 的人工验收结论一并归入本文件。
- 0.3 计划正文（`0.3-architecture-algorithm-plan-2026-10-05.md`）移入本目录。计划覆盖的阶段
  1–9 已全部实现，它不再作为执行依据；**这不等于 0.3 已完成**——阶段 9 的 GUI 手工验收、发布前
  压力测试与是否打标签仍未落定。
- `../current-handoff.md` 只保留当前批次、当前基线与下一步；`../todo.md` 删除已通过的阶段 7
  验收项，保留阶段 9 的暂时遗留项、alpha2 遗留验收、垃圾文件扫描与检查点取消的界面项以及补强
  候选。
- 同步修正落后口径：压力测试报告的当前构建版本、系统架构里画板实体文件名的命名、以及本目录与
  `research/` 中指向 0.3 计划的相对路径。

## 10. 已归入既有日志的批次（指针）

- **alpha1 压力测试收尾与 CPU 时间度量**（2026-10-19）：`stress_large` 四档、`stress_scale`、
  `stress_bench`、`stress_baseline` 与 `stress_authenticity` 的 16K 档全部通过，`stress_bench`
  新增子进程 CPU 时间并重录 release 基线；记录见 `handoff-log-2026-10-19.md`，实测表见
  `../../user/stress-test-report.md`。
- **alpha2 收口、阶段 9 计划改向与 `0.3.0-alpha.2` 切分**（2026-10-19 下午）：维护者验收阶段 6、
  同仓库写入排队修复与超期临时目录清扫；阶段 9 由「per-branch 全链校验 + `branch_notices`」
  改向为「空闲完整性检查」；同日切分 `0.3.0-alpha.2`。记录见 `handoff-log-2026-10-19.md` 第 7 节。
