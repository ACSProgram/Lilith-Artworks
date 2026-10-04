# 当前任务交接

更新时间：2026-10-04

本文件记录**当前批次**的执行状态与人工验收结果，并保留最近已验收批次的记录，供
`todo.md` 与模块文档引用；更早的已完成计划进入 `archive/`，未完成事项集中到 `todo.md`。

## 当前基线

- 应用版本 `0.2.0-alpha.4`（2026-10-04 由维护者决定递增，**仅版本号变更**：不建 tag、
  不发布、无行为变化，与 alpha.1/alpha.2 的做法一致；最新公开标签仍为 `v0.2.0-alpha.3`），
  repository schema **v4**：
  v2 → v3 为 `branches` 追加 `backup_quick_enabled`、`last_source_size`、
  `last_source_modified_ms`；v3 → v4 追加 `verified_history_id`、`verified_ms`、
  `verify_error`（空闲链路校验状态）。均为追加式迁移，旧数据不变，
  `tools/release/verify-metadata.mjs` 的 schema 断言已同步为 v4。应用标识
  `com.lilith.artworks`。
- 版本与发布口径（2026-10-04 确立，写入 `docs/guides/release-policy.md`）：alpha 阶段
  版本号**滞后递增**——版本号保持不变，改动并入当前版本的 CHANGELOG 段；累计足够后由
  维护者单独递增，递增本身不需要 tag 与发布。tag 与发布是维护者的独立决定。
- 项目定位：平面美术个人项目的**资源、版本管理与发布**工具。领域模块为 Library（作品树）、
  History/Backup（分支与增量历史）、Authenticity（成品与 C2PA/TrustMark）、Pin-board（素材板）。

## 本轮批次：统一清理体系批次 E——DDS 扫描与双向检查（已实现，待人工验收）

落实 `cleanup-system-plan-2026-10-04.md` 批次 E（§4.5）。维护者 2026-10-04 确认三项：
DDS 校验深度取**连 BC7 解码验证**（非只做声明校验）；因 `pin_board_images` 无 SHA-256
列且计划不做 schema 迁移，**跳过摘要比对**；缺失/损坏 DDS **报告不失败**（计数并入报告、
命令仍成功返回，不自动修复）。

1. **新增 `pin_board::scrub::scrub_board_dds`（`pin_board/scrub.rs`）**：双向检查。
   - 记录 → 文件：逐条 `pin_board_images` 记录，校验 DDS 存在、`file_path` 归属（须为
     `<image_id>.dds`）、DDS/DX10/BC7 头、声明尺寸与记录宽高一致、数据长度、BC7 全块
     解码；分别计入 `missing` / `corrupt`。
   - 文件 → 记录：遍历 `artworks/*/boards/*/`，统计无记录的孤儿 DDS（`orphans`）。
   - 返回 `BoardDdsReport { images, missing, corrupt, orphans }`。只报告、不修改数据库或
     磁盘；逐条检查前响应取消、按「记录数 + 磁盘文件数」回报进度。扫描在仓库操作锁内
     运行（与画板写入互斥），因此不设宽限期。
2. **挂入 `scrub_repository_integrity`（`app/workflows.rs`）第三段**：在历史链
   （`scrub_history`）与认证受控文件（`scrub_controlled_files`）之后执行，进度标签
   「正在检查画板图片」。报告 `RepositoryScrubReport` 追加
   `pinBoardImages` / `pinBoardMissingDds` / `pinBoardCorruptDds` / `pinBoardOrphanDds`
   （serde camelCase）。
3. **孤儿 DDS 并入批次 C 的发现流程**：`cleanup::scan_unreferenced` 扩展扫描
   `artworks/*/boards/*/<image-id>.dds`；`referenced_path_kind` 的 `repository_file`
   分支新增 `pin_board_images` 反向引用检查（画板记录删除后即不再引用），因此被引用
   DDS 不会成为候选、确认清理的重放也不会误删。命名判定 `is_dds_name` 收敛到
   `pin_board::repository`，扫描与完整性检查共用。
4. **`dds::validate_bc7_decodable`**：解码声明长度内的全部 BC7 块。
5. **前端**：`types.ts` 拆出 `RepositoryIntegrityCounts`，`RepositoryScrubReport` 追加四个
   画板字段（`RepositoryBackupReport` 改继承计数基类，不含画板字段）；`App.tsx` 完整性
   检查消息追加画板图片数，缺失/损坏/孤儿任一 > 0 时改报问题计数；设置页说明改为
   「检查历史链、受控文件摘要、画板 DDS 与 C2PA 声明」。

**落实偏差（已核实）**：§4.5 的「SHA-256 与导入时落库摘要比对」**未实现**——
`pin_board_images` 无摘要列，且 §3 明确不做 schema 迁移，故按维护者确认跳过摘要比对，
只做声明校验 + BC7 解码。另 §7 第 4 条由维护者选「连 BC7 解码验证」；`bcdec_rs::bc7`
是**全函数**（对任意 16 字节输入不返回错误），该步骤实际是走一遍完整解码路径、守住
边界与长度，而非判定像素内容是否“正确”——内容语义在没有原始素材时不可判定。

**验证**：`cargo check --lib` 无警告；`cargo check --features headless` 通过（无头
`scrub` 子命令保持只覆盖历史链，画板 DDS 检查仅经 GUI 完整性检查暴露；本批次无头侧无
代码改动）；`cargo fmt` 已执行、`cargo fmt --check` 与 `git diff --check` 通过；
`cargo test --lib` **150 通过**（原 145 + 新增 5：`dds` 1、`cleanup` 1、`pin_board::scrub`
3）、1 个忽略项；`npx tsc --noEmit` 通过；`npm test` **118 通过**（原 116 + 新增 2：
完整性检查报告展示与画板 DDS 问题警告）。

## 上一批次：统一清理体系批次 D——灾备暂存目录清扫（已实现，待人工验收）

落实 `cleanup-system-plan-2026-10-04.md` 批次 D（§4.4）。宽限期沿用批次 C 已确认的
**30 分钟**（与未引用文件扫描一致）；本批次只改后端，返回报告新增字段的界面展示与
设置页入口留待批次 F。

1. **新增暂存目录清扫（`backup/repository_backup.rs`）**：`create_repository_backup`
   在 `validate_destination` 之后、复制之前调用 `sweep_stale_staging_directories`，扫描
   目标目录**顶层**名字匹配 `.lilith-artworks-<32 位十六进制>.tmp` 的项。只删除**确实是
   目录**（`symlink_metadata` 判定，不跟随符号链接）且修改时间早于 `now - 30 分钟` 的
   目录，逐个 `remove_dir_all`；只碰顶层、不递归匹配。宽限期避免误删正在进行（本进程或
   另一进程）的灾备暂存目录——这正是压力测试批次 2 的 B3 场景。
2. **失败不阻断**：目标目录不可读、单个删除失败、当前时间读取失败都只写 `log::warn`，
   不改变本次备份的成功语义；清扫结果以 `StagingSweep { reclaimed, failed }` 计数，
   `RepositoryBackupReport` 追加 `reclaimed_staging_directories` /
   `failed_staging_directories`（serde camelCase →
   `reclaimedStagingDirectories` / `failedStagingDirectories`）。
3. **命名常量收敛**：暂存目录前缀 / 后缀 / id 十六进制长度与宽限期提为模块常量，
   暂存路径构造与清扫判定共用同一组常量，避免两处命名漂移。
4. **不做独立入口**（计划 §4.4）：暂存目录位于仓库之外、无数据库引用可查，且应用不
   持久化历史目标目录，因此不进 `pending_file_cleanup`、不提供针对旧目标的扫描命令。
5. **单测**（`backup::repository_backup` 新增三条）：过期残留暂存目录被回收，且命名
   不匹配的目录与名字像暂存目录的普通文件都不受影响；宽限期内的暂存目录保留；
   `create_repository_backup` 正常发布、报告两个计数为 0 且不删除宽限期内的暂存目录。
6. **同步更新 `tests/stress_crash.rs` B3 的过时注释**：崩溃留下的暂存目录现在会在超过
   宽限期后由下一次灾备启动时回收，注释改为如实描述；断言不变——B3 的重跑发生在强杀后
   数秒内，仍在宽限期内，因此暂存目录仍应保留。

**验证**：`cargo check --lib` 无警告；`cargo check --features headless` 通过（无头侧无
代码改动，灾备走同一领域函数）；`cargo fmt` 已执行、`cargo fmt --check` 与
`git diff --check` 通过；`cargo test --lib` **145 通过**（原 142 + 新增 3）、1 个忽略项；
本批次无前端改动，`npm test` **116 通过**（结论不变）。

**落实偏差（已核实）**：计划 §5 的批次 D 单测含「残留目录被回收」，但 Windows 上 std
无法打开目录以改写其 mtime（实测 `File::open` 对目录返回「拒绝访问」），而不引入新依赖
是计划非目标。因此「过期 → 回收」的判定由给 `sweep_stale_staging_directories` 传入
合成 `now_ms` 的单元测试覆盖（真实宽限期常量仍参与计算），端到端用例覆盖「本次备份正常
发布 + 宽限期内暂存目录保留」。另：`cargo check --lib --tests` 另有 4 条既有警告
（`pin_board/repository.rs` 与 `backup/chunk_file.rs`，非本批次文件），未在批次 D 处理。

## 上一批次：统一清理体系批次 C——未引用文件扫描（已实现，待人工验收）

落实 `cleanup-system-plan-2026-10-04.md` 批次 C（§4.3）。维护者 2026-10-04 确认三项：
交付形态为**报告 + 确认清理**（非仅报告）、宽限期取 **30 分钟**、扫描与确认清理的
Tauri 命令**本批次加入并注册**（设置页 UI 留待批次 F）。

1. **新增 `cleanup::scan_unreferenced(root, cancelled, progress)`**：遍历
   `artworks/*/snapshots/` 与 `artworks/*/deltas/`，只报告匹配既有命名模式、修改时间早于
   宽限期（`SCAN_GRACE_MS` = 30 分钟）且经反向引用检查判定未被引用的文件，返回
   `ScanCandidate { path, byteSize, reason }`（仓库相对路径、字节数、原因）。引用复查复用
   `referenced_path_kind` 的五张表查询，与重放看到同一套引用关系。**只报告不删除**；目录
   缺失按空处理，取消经 `cancelled()` 中断。命名匹配：snapshot `<UUID>.lbc` 与修复态
   `<UUID>-repair-<UUID>.lbc`，delta `<UUID>-to-<UUID>.lbd`。
   - **与规划原文的偏差（已核实）**：§4.3 把 snapshot 与 delta 的扩展名都写作 `.lbd`，实际
     代码 snapshot 为 `.lbc`（含 head 修复态的 `-repair-` 命名），delta 为 `.lbd`；本批次按
     实际命名实现，两种 snapshot 命名都覆盖。
2. **新增 `cleanup::cleanup_unreferenced(root, paths)`**：确认清理——每条候选入队时登记当前
   SHA-256 作为期望摘要（重放时内容已变即保留），文件已不存在时跳过，入队后立即单遍 `run`
   重放。**幂等**：重复确认不重复删除、不报错；重放前仍复查引用，候选在确认前重新被引用时
   条目留队可重试。复用既有重放，不新造删除逻辑。
3. **命令层（GUI 进程内）**：`cleanup_commands.rs` 新增 `scan_repository_unreferenced` 与
   `cleanup_repository_unreferenced`，经 `run_exclusive(UserOperation)` + `with_ready_repository`
   持共享运行锁与仓库操作锁，扫描进度与取消走统一运行状态；`lib.rs` 注册。**不做无头子命令**
   （§3 非目标：仓库哨兵锁落地前扫描入口只经 GUI 进程内暴露）。设置页的队列列表、扫描按钮与
   确认清理留待批次 F。
4. **单测**（`cleanup` 新增三条）：孤儿 snapshot 与孤儿 delta 被识别、被历史节点引用的文件
   保留、命名不匹配的文件不报告；宽限期内（刚写入）的文件被跳过；确认清理删除候选且重复
   调用幂等、再扫描无候选。

**验证**：`cargo check --lib` 无警告；`cargo check --features headless` 通过（无头侧无代码
改动，扫描不暴露为子命令）；`cargo fmt` 已执行、`cargo fmt --check` 与 `git diff --check`
通过；`cargo test --lib` **142 通过**（原 139 + 新增 3）、1 个忽略项；前端无代码改动，
`npm test` **116 通过**（结论不变）。

## 上一批次：统一清理体系批次 B——历史文件清理入队（已实现，待人工验收）

落实 `cleanup-system-plan-2026-10-04.md` 批次 B（§4.2）。维护者 2026-10-04 确认两项
设计决策：**入队放在领域函数自身的 SQLite 事务内**（而非调用方提交后再入队）、
**不登记期望 SHA-256**。理由与对 §4.2.1 字面的偏差见计划该节的落实偏差说明。

1. **六处「提交成功后直接删除仓库文件」收敛到清理账本**：分支删除
   （`history_branch_deletion`）、历史子树删除（`history_subtree_deletion`）、取消检查点
   （`history_checkpoint_release`）、提交释放父 snapshot（`history_commit_release`）、
   修复 head snapshot 时替换旧文件（`history_snapshot_replaced`）、精简改接释放旧实体
   （`history_compaction`）。`history::{delete_branch, delete_subtree, unmark_checkpoint,
   commit, apply_compaction, set_snapshot}` 在事务内复查引用后入队并返回 cleanup id，
   调用方（`app/workflows.rs`、`backup/commands.rs`、`backup/worker.rs`、
   `backup/restore.rs`）只调用 `cleanup::replay` 做单遍重放。提交失败时入队随事务回滚，
   记录与文件保持一致。
2. **`cleanup` 补上事务内能力**：`referenced_path_kind`（按 `path_kind` 复查五张表，
   可传入 `&Transaction`，事务可见本事务尚未提交的写入）、
   `enqueue_released_repository_files`（复查后入队，跳过仍被引用的路径、去重）、
   `replay`（提交后单遍重放，条目级失败与重放自身的数据库错误都只记 `log::warn`，
   不改变调用方成功语义）。`pin_board/mod.rs` 的本地重放助手收敛到 `cleanup::replay`。
3. **移除 `history::storage_path_referenced`**：入队落点改为事务内后，调用方不再需要
   提交后的快路径复查，引用复查统一走 `cleanup::referenced_path_kind`；旧函数已无调用点，
   随本批次删除（避免 `cargo check --lib` 的 dead_code 警告）。
4. **回滚路径保持直接删除**：发布失败、提交失败、登记 snapshot 失败时清理本次新文件的
   删除保持直接 `remove_file`——这些文件本就未被数据库引用，直接删失败只留泄漏、不留
   不一致，由批次 C 的扫描兜底（计划 §4.2.2）。
5. **单测**：`cleanup` 新增两条——入队跳过仍被历史节点引用的路径、只删已无引用者；
   条目重放被引用检查拒绝时留在队列，引用消失后同一批 id 再次重放即删除并清空队列。
   `history` 的两条既有用例改为断言队列：
   `branch_deletion_enqueues_released_edge_delta_paths` 断言只入队该分支独占的
   snapshot/边 delta、共享祖先不入队；`compaction_atomically_rewires_child_and_edge`
   断言被移除节点与旧边入队、新 delta 不入队。

**验证**：`cargo check --lib` 无警告；`cargo check --features headless` 通过（无头入口
经同一批领域函数自动覆盖，无头侧无代码改动）；`cargo fmt` 已执行、`cargo fmt --check`
与 `git diff --check` 通过；`cargo test --lib` **139 通过**（原 137 + 新增 2）、1 个忽略项；
前端无代码改动，`npm test` **116 通过**（结论不变）；`git grep` 确认 `src-tauri/src` 内
已无「提交成功后直接删除仓库文件」的调用点——剩余 `remove_file` / `remove_dir_all` 均为
回滚路径（发布/提交/登记失败）、认证预览缓存（`temp/`，计划非目标）、灾备暂存目录
（批次 D）、SQLite sidecar 与 DDS 导入回滚，或清理实现本身。

## 上一批次：统一清理体系批次 A——画板结算改提交后清理（已实现）

落实 `cleanup-system-plan-2026-10-04.md` 批次 A（§4.1）。维护者 2026-10-04 确认接受
语义变化，并要求重试不得阻塞进度：实现为**单遍重放、不循环重试**，结算命令在提交
成功后立即返回，删除失败只留队列条目等下次结算/回收站操作/手动重试消化。

1. `finalize_board` 不再在提交前 `fs::remove_file` 删除被清除图片的 DDS，改为事务内
   `cleanup::enqueue_repository_file(…, "pin_board_finalize")`（`pin_board_images`
   未落库 SHA-256，按规划用不带期望摘要的入队）；`bump_revision` → `commit` 顺序不变。
   提交失败时入队随事务回滚，记录与 DDS 保持一致——消除了「记录已回滚而 DDS 已消失」
   的不可逆不一致，以及「提交成功但结算报错」的悖论状态。
2. 命令层 `finalize_pin_board`（`pin_board/mod.rs`）在提交成功后执行一次
   `cleanup::run`（与画板回收站删除同范式）；条目级失败与重放的数据库错误都只写
   `log::warn`，不改变命令的成功/失败语义，队列状态的可观测展示留给批次 F。
3. 新增三个单测（`pin_board/repository.rs`）：结算入队且重放后 DDS 与记录均被清除、
   事务回滚时记录与 DDS 均保留且队列无残留、重放失败（引用检查拒绝）条目留队且解除
   引用后单次重放成功。语义变化：DDS 删除失败不再使结算整体报错。

**验证**：`cargo check --lib` 无警告；`cargo fmt` 已执行、`git diff --check` 通过；
`cargo test --lib` **137 通过**（原 134 + 新增 3）、1 个忽略项；前端无代码改动，
`npm run test:pin-board` 56 通过（结论不变）。待维护者确认项：结算成功/失败文案的
GUI 行为无变化（错误仅在日志与队列中）。

## 上一批次：alpha.4 版本递增与统一清理体系规划（已完成）

1. **版本号递增到 `0.2.0-alpha.4`**：仅版本号变更，无 tag、无发布、无行为变化
   （与 alpha.1/alpha.2 一致）。同步五处版本字段、`CHANGELOG.md` 新建带日期小节、
   `npm run legal` 重生成许可清单（529 组件，仅版本字样变化）、`README.md` 与
   planning 文档版本引用同步。滞后递增规范写入 `docs/guides/release-policy.md`：
   alpha 阶段改动并入当前版本 CHANGELOG 段，版本号由维护者在累计足够后单独递增，
   递增本身不需要 tag 与发布。验证：`node tools/release/verify-metadata.mjs` 通过
   （v0.2.0-alpha.4、schema v4）。
2. **登记 todo P2「同仓库多进程并发缺乏防呆」**：核查结论是应用层仓库互斥与读写租约
   只在进程内有效，单实例插件只覆盖 GUI 入口，无头入口不启动 Tauri 应用、不受其约束，
   跨进程只剩 SQLite 的 WAL + `busy_timeout`。数据库层不会损坏（事务 +
   `synchronous = FULL`），但旧版本进程会因 schema 迁移把仓库判为不可用、设置文件为
   「最后保存者胜」、并发提交与清理队列重放的业务层竞态未设计未测试；仓库哨兵锁另立
   条目推进，落地前扫描类入口只经 GUI 进程内暴露。
3. **新增 `docs/planning/cleanup-system-plan-2026-10-04.md`（统一清理体系规划）**：
   覆盖 `todo.md` 第一节五条清理相关 P1，划分批次 A–F（画板结算改提交后清理、历史
   清理入队、未引用文件扫描、灾备暂存目录清扫、完整性扫描覆盖画板 DDS 与双向检查、
   可观测 UI 与文档收尾）。全部批次未实施；四项设计决策待维护者确认（计划 §7）。

## 上一批次：压力测试批次 3（大文件端到端，已实现）

规划见 `docs/planning/stress-test-plan-2026-10-04.md` 的批次 3 与 C 组矩阵。本批次落实
**批次 3**；批次 4–6（规模与灾备、认证大图、文档收尾）仍未开始。

1. **只新增测试，不改产品代码。** 复用批次 1 的无头入口与批次 2 的编排：本批次没有改动
   `headless.rs`、`Cargo.toml` 或任何领域代码，只在测试侧新增 C 组并扩展共享助手。
2. **C 组覆盖「文件大小 × 单次改动量」两条正交的轴**（`tests/stress_large.rs`，6 个测试）：
   C1 端到端（提交 → 有界区域改动 → 恢复两端 → 逐位一致）、C2 精简后仍可恢复、C3 峰值内存
   不随逻辑大小线性增长、C4 链体积与磁盘账本、C5 恢复产物即删、C6 大文件大改动（每轮改 1/4）。
   工作文件用稀疏构造（`set_len` + 有界区域写入），因此 4 GiB 逻辑大小下真实写入量仍只有
   数百 MiB，而快照仍约 4 GiB。
3. **档位与轮数**：`LILITH_STRESS_TIERS` 选择 `default` 64 MiB / `large` 256 MiB /
   `heavy` 1 GiB / `extreme` 4 GiB / `manual` 8 GiB；小改动档轮数随档位下降（20/20/10/5），
   因为每轮提交都要重写整份快照，轮数不降时 4 GiB 档光 C1 就要写约 80 GiB 快照。
4. **按磁盘预算并发（本批次的主要改动）**：共享助手把原来的全局串行闸门替换为
   `scenario_slot` 准入——多个场景并行，但「预计峰值磁盘之和」不超过
   `LILITH_STRESS_DISK_BUDGET`（默认由 12 GiB 提到 24 GiB，否则 4 GiB 档会被误判超预算），
   并发数不超过 `LILITH_STRESS_JOBS`（默认可用核数）。动机是被测子进程单线程
   （分块滚哈希 + SHA-256 + zstd 都是单核路径），串行时 16 核机器只用 1 个核、磁盘约
   150 MB/s（盘可达 2 GB/s）；实测整轮从约 70 分钟降到约 28 分钟。
5. **工作区移出 `%TEMP%`**：改到项目 `target/stress-workspaces/`，仍由 `TempDir` 持有并自动
   回收。原因：并发运行中实测出现过 `%TEMP%` 下工作区子目录在场景中途消失、`restore` 报
   「恢复输出目录不存在」，以及 `extreme` 档某个 `commit` 子进程无结果、无 stderr 地以退出码 1
   终止的偶发失败；迁移后四档连续通过、零残留。
6. **与计划的偏差**：计划 §4.6 的执行模型是大文件档**串行**，本批次按维护者要求改为预算并发
   （见第 4 点）；规模场景与认证大图仍留待批次 4、5。

## 上一批次：压力测试批次 2（跨进程崩溃，已实现）

规划见 `docs/planning/stress-test-plan-2026-10-04.md` 的批次 2 与 B 组矩阵。本批次落实 **批次 2**。

1. **只新增测试，不改产品代码。** 复用批次 1 的无头入口与编排：本批次**没有**改动
   `headless.rs`、`Cargo.toml` 或任何领域代码，只在测试侧增加强杀编排与 B 组场景。
2. **强杀注入（维护者 2026-10-04 确认，偏离计划 §4.4 原文）**：采用「逐检查点闸门暂停后
   强杀」，而非计划原文的「轮询 marker 后 kill」。带 `--cancel-on-stdin` 时进程在目标检查点
   **阻塞**，测试随即 `Child::kill()`（Windows 等价 `TerminateProcess`，不运行任何清理），
   因此强杀落在确定的代码位置；非阻塞轮询在「发布后、提交前」这类亚毫秒窗口下会与进程
   赛跑而抖动。干预仍只经文件（marker）与 stdin，不触碰任何内部状态。
3. **B 组场景**（`tests/stress_crash.rs`，3 个测试）：
   - **B1 提交中途强杀**：在第 4 个检查点（snapshot/delta 已发布、`history::commit` 未执行）
     强杀；断言重开可用（`verify` + `scrub` 全过、head 未推进），恰好多出一个孤儿 snapshot
     与一个孤儿 delta（均为完整文件），清理队列为空，随后重跑提交成功、节点前进一格。
   - **B2 恢复中途强杀**：在检查点 5（创建输出临时文件之前）与检查点 6（导出并 `sync_all`
     之后、`persist` 之前）各强杀一次；断言目标输出**不存在**（原子发布与不覆盖语义在崩溃
     下同样成立），检查点 6 只留下一个未发布的**完整**临时文件（位于工作区 `out/`，仓库之外）。
   - **B3 整仓灾备中途强杀**：在复制阶段强杀；断言源仓库不受影响、可独立校验，目标目录留下
     **可识别**的未发布暂存目录（`.lilith-artworks-*.tmp`）且无已发布 bundle，随后重跑灾备
     成功、副本可独立打开并通过链路校验。
4. **如实记录的缺口（维护者 2026-10-04 确认：断言可达事实 + 记录缺口）**：崩溃发生在
   `history::commit` 之前时，已发布的 snapshot/delta **不会**进入 `pending_file_cleanup`
   （`history::commit` 只把旧 snapshot 路径返回给调用方直接删除；入队的是画板目录、作品
   删除、认证副本等，不含历史快照），而 `cleanup` 只重放队列、**没有未引用文件扫描**，因此
   当前实现**不会自动回收崩溃孤儿**；同理，灾备被强杀时的暂存目录（`StagingDirectory::drop`
   在进程被杀时不运行）也不会被自动清理。本批次不修改产品行为，只把这两点作为本批次实测
   结论记录在案。
5. **与计划的三处偏差（已核实）**：①强杀注入方式见第 2 点；②计划 §5 B1 的「孤儿 snapshot
   可被清理队列回收」在当前代码下不成立（见第 4 点），按维护者口径改为断言可达事实 + 记录
   缺口；③计划 §7 批次 2 只列 B1–B3，实际断言更细（B2 拆成「导出前 / 导出后」两个位置）。

## 规划订正与重排（2026-10-04）

批次 2 落实后核实出计划文档与代码的两处不符，据维护者决定订正
`docs/planning/stress-test-plan-2026-10-04.md` 并重排批次：

1. **事务中途崩溃未被覆盖**：B 组强杀点落在 `history::commit` **之前**，B2/B3 不写数据库，
   因此 `WAL + synchronous = FULL` 这条承诺（计划 §1.2 本意要验证的）**仍未验证**。
   新增「批次 2 补充：事务中途崩溃（B4）」，以 headless-only 标记点实现（需动产品代码，
   但 feature 门控、不进发布产物），尚未落实。
2. **崩溃孤儿不会自动回收**：计划原文声称 `cleanup` 队列可回收崩溃孤儿，与代码不符。
   断言改为「存在 + 不被引用 + 重开可用」，自动回收作为**产品能力**转入 `todo.md` 第一节（P1）。
3. **断电持久性不做自动化**：`Child::kill()` 不触及 OS/磁盘缓存与目录项落盘，结构上无法覆盖
   「断电后已提交数据是否仍在」；已写入 `todo.md` 第二节与计划 §6 非目标。
4. **排期**：维护者选择先做批次 3；P1 缺陷修复（含统一清理体系）仍建议尽早插入，否则
   批次 3–5 中涉及同一写路径的断言可能需重跑（见计划 §11.4）。

## 上一批次：压力测试批次 1（无头入口骨架与取消边界，已实现）

规划见 `docs/planning/stress-test-plan-2026-10-04.md`。本批次只落实该计划的
**批次 1**；批次 2–6（跨进程崩溃、大文件端到端、规模与灾备、认证模块、文档收尾）尚未开始，
因此「各处理阶段的取消边界」目前只覆盖到提交/恢复/精简/检查点/整仓灾备/全库扫描，
认证签名部分要等批次 5。

1. **无头入口（feature 门控，不进发布产物）**：`Cargo.toml` 新增 `[features] headless = []`
   且**不加入 default**；`lib.rs` 新增 `pub fn run_headless`，`main.rs` 在遇到
   `--headless` 时分派。它不创建窗口、不建托盘、不加载 webview、不经过 IPC；发布构建里
   这一段整体不存在，因此产品行为零变化。
2. **10 个子命令的 1:1 薄映射**：`init-repository`、`create-artwork`、`commit`、`restore`、
   `compact`、`checkpoint`、`scrub`、`verify`、`cleanup`、`repository-backup`。每个子命令只做
   参数解析、`BackupState::default()` 与 `AppState` 的装配、领域函数调用，不含业务判断；锁的
   用法与 Tauri 命令层逐一对应（`run_logged` / `run_logged_foreground` / `run_foreground` /
   `run_exclusive`）。无头进程不启动调度器，故没有手动提交优先与唤醒步骤（已在代码中注明）。
3. **通用选项**：`--workspace`（作用域根）、`--repository`（默认 `<workspace>/repository`）、
   `--result`（结构化 JSON 结果，用文件而非 stdout——release 下无控制台）、`--marker`
   （逐阶段追加阶段名）、`--cancel-on-stdin`、`--peak-memory`。`--workspace` 同时是一条
   **真实的安全属性**：无头进程拒绝解析工作区之外的路径（维护者 2026-10-04 确认的 CLI 形状）。
4. **取消干预协议采用「逐检查点闸门」**（维护者 2026-10-04 确认，偏离计划 §4.4 原文）：
   带 `--cancel-on-stdin` 时，进程在**每个取消检查点**先写 marker 再阻塞等待 stdin 的
   一行判定（`cancel` / `continue`，EOF 视为取消）。计划原文的「轮询 marker 后写一行」是
   非阻塞的，毫秒级窗口下无法稳定命中具体清理分支，而 `run_backup` 又没有任何进度回调，
   因此改为闸门；干预仍只经文件与 stdin，不触碰内部状态。
5. **A 组场景**：`tests/stress_cancel.rs`（A1–A6，6 个测试、27 次受控取消）与共享助手
   `tests/stress_support/mod.rs`（工作区、进程编排、闸门策略、磁盘事实断言、JSONL 报告）。
   断言只用「子命令返回的 JSON」与「磁盘事实」：无残留实体与临时文件、历史节点数未推进、
   待清理队列为空、重开可用（`verify` 完整性与语义校验 + `scrub` 逐块摘要链），并且每个场景
   末尾都用**正向对照**确认命令随后仍然可用（提交重试成功、恢复字节一致、精简成功、
   灾备副本可独立打开）。
6. **报告**：每次受控取消向 `target/stress-report.jsonl` 追加一行。`target/` 不进版本控制，
   因此可长期引用的证据是本文件下方「验证记录」里的摘要。
7. **与计划的三处偏差**（已核实，均为必要）：`--peak-memory` 除
   `Win32_System_ProcessStatus` 还需 `Win32_System_Threading`（`GetCurrentProcess` 在其中，
   只加 feature、不加包）；计划 §4.1 声称「不再需要任何 `pub(crate)` 可见性改动」，实际需要把
   `backup::restore` / `backup::worker` 提升为 `pub(crate) mod`、`BackupRunError` 提升为
   `pub(crate)`、`library::create_artwork` 由 `#[cfg(test)]` 改为
   `#[cfg(any(test, feature = "headless"))]`（纯可见性放宽，无行为变化）；计划 §7 批次 1 的
   子命令清单漏了 A5 必需的 `repository-backup`，已一并实现。
8. **测试可指向任意构建档**：`LILITH_STRESS_BIN` 把整套断言指向维护者自己构建的可执行文件
   （覆盖路径先做 `--headless help` 探针，避免误指向未启用 headless 的构建而启动 GUI）。
   这回答了「要不要跑发布档」：取消、崩溃与一致性不变量与构建档无关；真正需要发布档的是
   峰值内存、存储放大与耗时（C、D、G 组）。

## 上一批次：任务调度总控与空闲链路校验（已实现，待人工验收）

规划见 `docs/planning/archive/task-control-plan-2026-10-03.md`（已随本批次归档），
按批次 A–C 实现、本批次 D 收尾。目标：补上“快速检查不校验 head snapshot”的完整性缺口，
并把后台任务与前台命令的让位、取消路由统一到一套模型。

1. **任务类型与取消路由**（批次 A）：`BackupTaskKind`（`AutomaticBackup` / `IdleVerify` /
   `UserOperation`）贯穿 `run_exclusive_typed` / `run_exclusive` / `run_logged` 与全部调用点，
   运行状态新增 `taskKind`。`request_cancel` 取消任意任务，新增 `cancel_background` 只取消
   后台任务，手动提交对自动任务的取消改走它。前台长命令（恢复、精简、检查点、删除子树、
   进入发布、发布签名、全库扫描、整仓备份、仓库切换）经 `run_foreground` /
   `run_logged_foreground` 在取锁前登记 `foreground_waiting` 并请求后台让位；调度器在候选
   选择与取得运行锁后两处复查该计数，有前台在等即让位，避免抢先取锁吞掉取消意图。
2. **schema v4 与单节点校验入口**（批次 B）：`branches` 追加 `verified_history_id`、
   `verified_ms`、`verify_error`；`SCHEMA_VERSION = 4`，`verify-metadata.mjs` 断言同步为 4，
   并修正 `pin_board/repository.rs` 与 `app/settings.rs` 两处硬编码旧版本号的既有断言。
   分支 head 恒持有 snapshot，校验 head 等价于校验单个 snapshot，因此复用 `validate_snapshot`
   （提升为 `pub(crate)`），`scrub_history` 保持不动。
3. **调度器两级选择、派生队列与警告面**（批次 C）：调度器增加第二优先级——仓库空闲时从
   派生队列取一条 head 已静默 ≥ `IDLE_VERIFY_DELAY_MS`（10 分钟）的分支，校验其 head 的
   单个 snapshot；成功写 `verified_history_id` / `verified_ms` 并清空失败，失败写
   `verify_error` 并退出队列（不自动重试）。前端分支状态行独立展示校验失败，含详情、
   复制与“重新校验此分支”（`reverify_branch_history`，清空失败并唤醒调度器）。
4. **批次 D 收尾**：`docs/modules/history-and-backup.md` 补齐任务类型、让位、空闲校验与
   全库扫描分工契约，`docs/architecture/overview.md` 同步一句；`todo.md` 移除已实现条目并
   登记本批人工验收项；`CHANGELOG.md`、`README.md` 的 schema 表述同步为 v4；计划文档归档。
   同时清理 `backup/runtime.rs` 中 `IdleVerify` 上已过时的 `#[allow(dead_code)]` 与注释
   （批次 C 已实际构造该值），并修正一处前端测试的格式。

## 上一批次：素材板退出握手与自动保存设置（已人工验收）

**问题**：素材板编辑（拖放摆放等）只改前端内存，保存仅在 Ctrl+S、页面隐藏、
渲染器销毁结算时触发；而关闭窗口/托盘退出在 Rust 侧直接 `app.exit(0)`，webview
的保存请求来不及完成，导致"关闭再打开进度回退"。撤销历史只在前端内存，重开
画板不能撤销（本批次不改变这一点）。

1. **退出握手（决定性修复，默认开启）**：真正退出（窗口关闭且未开"关闭到托盘"、
   托盘"退出"）不再立即结束进程。原生端保存窗口状态 → 隐藏窗口 → 发
   `app_shutdown_requested` → 启动 15 秒兜底强退计时器；前端（`App.tsx`）收到
   事件后按设置决定是否调用 `preparePinBoardRuntimeChange`（结束交互 + await
   finalize：保存并截断步骤历史），无论成败调用新命令 `confirm_app_shutdown`
   确认；原生端确认后执行原有不可逆退出序列（停调度线程、等共享操作锁释放）再
   `app.exit(0)`。webview 挂起或崩溃时兜底计时器保证窗口仍能关闭。
2. **防抖自动保存（默认关闭）**：`renderer.ts` 在模型变化（拖放结束、缩放、旋转、
   图层/顺序调整、删除、撤销/重做、置顶重排）后静默 1.5 秒自动 `save_pin_board`；
   显式保存、销毁会取消挂起的计时器，保存期间又有编辑会在保存完成后重新排程。
   覆盖崩溃/被杀等握手帮不到的场景。**维护者已实测无开关版本生效**；接入设置后
   默认关闭，需在设置页开启。
3. **设置页素材板页新增两个开关行**："自动保存"（默认关闭，控制防抖自动保存）、
   "关闭时保存"（默认开启，控制退出握手是否先结算素材板）。新字段依赖容器级
   serde default 兼容旧设置文件，设置版本保持 v2。
4. `AppState` 新增 `shutdown_handshake_started`/`shutdown_confirmed` 标志防止
   重复握手与重复确认；`confirm_app_shutdown` 在 `lib.rs` 注册。
5. **自动备份调度默认状态**：经维护者确认无需修改——全局 `pause_automatic_backups`
   默认 false、分支 `backup_enabled` 默认 1，本来就是默认开启。

## 上一批次：快速自动备份、手动提交优先与打开文件夹（已人工验收）

1. **快速自动备份**：自动备份新增"快速检查"方式。全局设置 `automaticBackupCheckMode`
   （默认 `quick`）；分支设置新增"快速检查"开关，开启后即使全局为全量也对该分支使用快速。
   快速检查只比较工作文件大小与修改时间和上次全量检查成功后记录的基线是否一致，一致时
   直接记为内容未变化、不读取文件内容；不一致、缺少基线或读取失败时退回全量检查和备份
   流程。手动提交始终全量。首次全量提交/检查后基线可用，旧仓库升级后同样自愈。
2. **手动提交优先**：手动提交先登记待处理标记，调度器在候选选择和取得运行锁后都会延后
   该分支的自动备份（不计入失败）；若同分支自动备份正在运行，手动提交对其请求取消，
   自动任务以取消结果退出后手动提交先执行。修复"同一时间手动提交总被自动备份覆盖"。
3. **打开所在文件夹**：分支设置的工作文件操作区新增"打开所在文件夹"按钮
   （`reveal_path_in_folder` 命令；Windows 用 `explorer /select` 选中文件）。
4. **设置页**：仓库页"自动备份"分区新增"默认检查方式"选择（快速检查（推荐）/ 全量校验）。
5. **设置页样式统一**：素材板页的设置行从旧的两栏 `setting-row` 布局迁移到通用页与
   仓库页共用的 `settings-preference-row` 行样式（行图标 + 标题/描述 + 右侧控件），
   四行分别为纹理缓存等级、阵列图片间距、锁定画板快捷键、画板全屏快捷键。较长的
   行描述改为自动换行不再截断；数字输入与快捷键控件高度对齐下拉框（34px）；移除
   旧的 `setting-row` 样式与窄屏覆盖规则。设置行小字同步精简：去掉"默认 Ctrl+R /
   F11"等可从控件直接看出的默认值、"保存后生效/使用新数值"等冗余提示，锁定画板
   快捷键改为说明锁定后无法编辑、仅可缩放和移动视图；"创建备份"去掉"发布前"措辞。

## 验证记录

### 压力测试批次 3（2026-10-04，最终态）

代理侧已执行（`LILITH_STRESS_BIN` 指向发布档二进制）：

- `cargo fmt --check`、`git diff --check` 通过；本批次只改测试与文档。
- `cargo test --features headless --test stress_large`：四档全部 **6/6 通过**——`default` 27.7 s、
  `large` 136.3 s、`heavy` 320.3 s、`extreme` 1218.5 s（按磁盘预算并发，16 逻辑核，整轮约 28 分钟）。
- 关键断言逐条成立：恢复最早/最新节点与期望内容逐位一致；精简后仍逐位一致；上传/恢复峰值内存
  增量不随逻辑大小线性增长（4 GiB 档约 349 / 343 MB，允许上限 562 MB）；磁盘账本实测增量与
  预期偏差 ≤ 0.03%；恢复输出目录在断言后清空、工作区占用回落；四档结束后 `%TEMP%` 与
  `target/stress-workspaces/` 均无残留。
- 实测数值汇总见 `docs/guides/stress-test-report.md` 的 4.3、4.4 节。
- 本批次未触及产品代码与 GUI 路径，无需维护者界面确认；代理不执行 UI 自动化。
- **尚未完成**：批次 4–6。`todo.md` 第二节的对应条目按计划在第 6 批次统一更新。

### 压力测试批次 2（2026-10-04，最终态）

代理侧已执行：

- `cargo fmt --check`、`git diff --check` 通过；本批次只改测试，`cargo check --lib` 结论不变。
- `cargo test --features headless --test stress_crash`：**3 通过**（B1–B3）。
- 回归：`cargo test --features headless --test stress_cancel` **6 通过**（未变化）；
  `cargo test --lib` **134 通过**、1 个忽略项（未变化）；`npm test` **116 通过**（未变化）。
- `npm run legal` 前后对比：`licenses/THIRD_PARTY_LICENSES.html`（529 组件）与
  `THIRD_PARTY_NOTICES.md` **无变化**——本批次不改 `Cargo.toml`、不新增依赖与 feature。
- 实测汇总（`target/stress-report.jsonl`，B 组 4 行，强杀均以进程终止退出、退出码 1）：

| 场景 | 强杀检查点 | 强杀阶段 | 耗时 ms | 观察到的检查点数 |
| --- | --- | --- | --- | --- |
| B1 提交中途强杀 | 4 | 发布后、提交前 | 57 | 4 |
| B2 恢复中途强杀（导出前） | 5 | 链解析完成后 | 56 | 5 |
| B2 恢复中途强杀（导出后） | 6 | 导出并同步后 | 67 | 6 |
| B3 灾备中途强杀 | 2 | 复制仓库文件 | 34 | 2 |

  关键路径断言逐条成立：强杀后 `verify`（迁移 + `integrity_check` + 外键 + 语义校验）与
  `scrub`（逐块摘要链）全通过；B1 恰好多出一枚孤儿 snapshot（82,068 字节的完整文件）与一枚
  孤儿 delta、清理队列为空、重跑提交成功且节点前进一格；B2 目标输出不存在、仅留下未发布的
  **完整**临时文件（位于 `out/`，仓库之外）；B3 源仓库可独立校验、暂存目录可识别且重跑灾备
  成功、副本可独立打开。
- **本批次暴露的产品事实（未修复，按计划只记录）**：崩溃孤儿与灾备未发布暂存目录不会被
  自动回收（见「本轮批次」第 4 点）。
- 本批次未触及 GUI 路径，也没有需要维护者确认的界面项；代理不执行 UI 自动化。
- **尚未完成**：批次 3–6。`todo.md` 第二节的对应条目按计划在第 6 批次统一更新。

### 压力测试批次 1（2026-10-04，最终态）

代理侧已执行：

- `cargo fmt --check`、`cargo check --lib`（无警告）、`git diff --check` 通过。
- `cargo test --lib`：**134 通过**，1 个忽略项（与上一批次结论一致，未变化）。
- `npm test`：**116 通过**（未变化）。
- `cargo test --features headless --test stress_cancel`：**6 通过**（A1–A6）。默认二进制与
  `LILITH_STRESS_BIN` 覆盖路径各跑一次，均通过；整套约 2 秒。
- `npm run legal` 前后对比：`licenses/THIRD_PARTY_LICENSES.html` **逐字节不变**
  （529 组件）。确认 `[features] headless` 与 windows-sys 的 feature 增加没有改变依赖包集合。
- 实测汇总（`target/stress-report.jsonl`，27 次受控取消全部以取消结果退出，退出码 2）：

| 场景 | 受控取消次数 | 最深检查点 | 最大耗时 | 峰值内存 |
| --- | --- | --- | --- | --- |
| A1 提交取消（每个检查点各一次） | 4 | 4 | 71 ms | 19.8 MiB |
| A2 恢复取消（链解析 / 导出前 / 发布前） | 6 | 6 | 69 ms | 17.5 MiB |
| A3 精简取消（父链 / 子链 / delta 发布前） | 6 | 6 | 59 ms | 20.2 MiB |
| A4 检查点取消（链解析 / snapshot 发布前） | 4 | 4 | 66 ms | 17.0 MiB |
| A5 灾备取消（扫描前 / 复制 / 校验 / 发布前） | 4 | 18 | 169 ms | 17.2 MiB |
| A6 全库扫描取消（入口 / 逐节点 / 靠后节点） | 3 | 8 | 80 ms | 17.1 MiB |

  峰值内存是 **debug 档**无头进程自报的峰值工作集（基线约 13 MiB），只作「确有数值可采」
  的证据，**不代表发布档**；真实数值由 C、G 组在 `LILITH_STRESS_BIN` 指向 release 构建时测量。
- 关键路径断言逐条成立：取消后 `artworks/*/snapshots`、`artworks/*/deltas` 与 `temp` 无残留、
  历史节点数未推进、待清理队列为空、`verify` + `scrub` 全通过；灾备取消时错误附带
  「临时备份已清理」，且建 staging 之前的取消不谎称做过清理（分开断言）。
- 本批次另有两条缺陷由测试自身暴露并修复（都在测试侧）：报告文件并行追加时行交错
  （改为进程内加锁），以及闸门参数没有真正传给子进程。
- 本批次未触及 GUI 路径，也没有需要维护者确认的界面项；代理不执行 UI 自动化。
- **尚未完成**：批次 2–6。`todo.md` 第二节的对应条目按计划在第 6 批次统一更新，
  本批次不动它，以免清单与实际覆盖情况提前不一致。

### 任务调度总控与空闲链路校验批次（2026-10-04，最终态）

代理侧已执行：

- `cargo fmt --check`、`cargo check --lib`（无警告）、`git diff --check` 通过。
- `cargo test --lib`：**134 通过**，1 个忽略项（新增 cancel_background 只对后台任务生效 /
  用户操作不被误取消、前台等待在正常与取消与退出路径均归零、调度器两级选择与让位、
  head 变化重新入队、失败后退出队列、NULL head 不入队、`validate_snapshot` 三例；
  v1 全链迁移断言与设置页版本断言同步为 v4）。
- `npx tsc --noEmit` 通过；`npm test`：**116 通过**（含分支状态行独立展示校验失败、
  详情展开、复制与“重新校验此分支”用例）。
- `node tools/release/verify-metadata.mjs`：schema 断言已同步为 v4（应用版本号本次不变）。

### 上一批次：素材板退出握手与快速自动备份

代理侧已执行：

- 素材板退出保存批次（最终态）：`npx tsc --noEmit` 通过；`npm test` **114 通过**、
  无未处理错误（renderer 自动保存 8 例：开关关闭不排程、开启即排程、关闭取消
  挂起计时器、脏状态排程/重排程/未脏不排程/显式保存取消/销毁取消）；
  `cargo fmt --check`、`cargo check --lib`、`cargo test --lib`（116 通过）、
  `git diff --check` 通过。
- 人工验收：维护者实测无开关版本的自动保存已生效（退出握手随之验证）。
- 快速备份批次：`npx tsc --noEmit`：通过。
- `npm test`：**106 通过**（新增"快速检查开关保存与打开文件夹"用例）。
- `cargo test --lib`：**116 通过**，1 个忽略项（新增 worker 快速检查基线 2 例、
  调度器快速路径/回退/分支开关/手动优先 4 例；v1 迁移用例改为完整模拟 v1 结构并断言 v3）。
- `cargo fmt --check`、`git diff --check`：通过。
- `node tools/release/verify-metadata.mjs`：schema 断言已同步为 v3（应用版本号待下次发布时递增）。

**人工确认结果（2026-10-04）**：上述两批待确认项均已由维护者确认通过——分支设置
"快速检查"开关的位置与文案、设置页"默认检查方式"选择、"打开所在文件夹"在 Windows
资源管理器中正确选中工作文件、设置页三页行样式统一后的视觉与换行效果；设置页两个新
开关行（自动保存/关闭时保存）的位置与文案、"自动保存"开启后编辑停顿约 1.5 秒落库与
关闭后退回旧保存行为、"关闭时保存"关闭后退出不再结算素材板（未保存编辑会丢，属预期）。
代理不执行 UI 自动化，这些项只做过程序化断言，结论由维护者给出。

## 人工验收结果（2026-10-04）

维护者在本机真实生产场景（个人日常使用）完成一轮人工验收，结论如下。

**通过，已从 `todo.md` 第二节移除：**

- 极端大文件与高像素压力的**日常使用**：实际工作文件已达 2 GiB、图片为大像素，使用正常
  （**错误恢复路径未覆盖**，见下）；
- 普通用户账户安装、Authenticode 签名与时间戳验证；
- 第三方工具回读 C2PA，并用随包模型验证 TrustMark 实图；
- 从上一公开候选版执行安装升级与卸载；
- `release-policy.md` 人工门槛中的常规桌面路径：安装、首次启动、仓库创建/打开、关闭到
  托盘与显式退出；Artwork 创建、树操作、回收站、分支、提交、创建分支、恢复、精简、
  检查点；进入/取消发布、认证导出与再次导出、识别与跨 Artwork 溯源；取消发布保留首次
  导出 JPG 且仓库内副本、记录与保存配置已清除；法律文件可从安装目录或 About/Legal
  页面取得；
- `docs/modules/library.md` 声明的真实仓库 lease、设置持久化与 Windows 交互；
- 分支工作文件路径的清空约束（提示文案、分支选择变化、保存失败后的表单回滚）。

**仍未验证，保留在 `todo.md`：**

- 各处理阶段的取消边界（提交、恢复、精简、检查点、整仓灾备、认证签名）；
- 损坏文件的恢复路径（snapshot/delta 缺失或摘要不匹配）；
- 异常 RFC 3161 时间戳服务的失败与超时行为；
- 画板 DDS 损坏、缺失、孤儿文件以及结算/仓库切换期间的保存失败恢复；
- 极端大文件与高像素压力的**错误恢复**路径。

其中「各处理阶段的取消边界」「损坏文件的构造与摘要不匹配」「大文件的错误恢复」三项
计划由 `stress-test-plan-2026-10-04.md` 的自动压力测试接管；该计划落实后必须回到
`todo.md` 第二节更新对应条目（见该文档第 10 节）。

任务调度总控与空闲链路校验批次的界面与交互验收项（空闲校验取消、校验失败警告与重新
校验、快速检查与校验组合、大链校验期间前台响应性、失败不阻断提交）另行列入 `todo.md`
第二节，见上文“上一批次：任务调度总控与空闲链路校验”。

**关于验收环境的口径：** 上述结论来自维护者本机的生产使用，不替代
`release-policy.md` 要求的"干净 Windows 用户环境"桌面验收；后者仍在 rc1 前执行。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 和 `docs/guides/` 为准；
未完成事项只写入 `docs/planning/todo.md`；本文件只记录当前批次状态与人工验收结果；
已完成或被替代的计划进入 `docs/planning/archive/`。
