# 当前任务交接

更新时间：2026-10-18

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-18 批次：**0.3 阶段 1（统一命令运行时）、阶段 2（设置单写入口、跨进程 owner lock、
退出结算诊断、App 前端拆分）、阶段 3（事件订阅与 controller 模板）、阶段 4（ChunkFile v2、
容器感知分块、恢复游标与快速检查指纹）、阶段 5（调度、清理与灾备算法）**。

- **0.2 已收口并归档**：`0be6224` 切分 `0.2.0`，`v0.2.0` 附注标签已打并推送；`main` 固定为
  0.2 发布线。切分内容、压力测试口径、自动与人工门槛、未签名发布说明见
  `archive/handoff-log-2026-10-18.md`，更早的验收记录见 `archive/handoff-log-2026-10-16.md`。
- **0.3 开发线**：`dev` 从 `0be6224` 创建，落实前修订已随 `d6206ac` 提交：
  `0.3-architecture-algorithm-plan-2026-10-05.md` 定稿为可实施口径（修订汇总见其 §0.5），
  `main`/`dev` 分支模型、Windows CI 与发布元数据口径（0.2.x → v4、0.3.x → v5）、表层文档
  同步一并落地。
- **阶段 0 已完成（随本阶段提交）**：补齐取消延迟 / 锁等待 / 调度扫描的测量能力（新增
  `stress_baseline` 组、headless 只读探针 `scheduler-scan`、闸门取消计时与 `Spawn::scope`），
  在临时仓库副本 `D:\temp\Artworks` 上采集 release 档数值；数据见
  `0.3-baseline-2026-10-18.md`（采集后已用采集前的副本还原仓库）。
- **阶段 1 已实现并提交**：新增 `app/command_runtime/`（`CommandContext`、
  typed `CommandError`、`CancelToken`、`OperationCoordinator`、`AppEvent` v1 信封）与
  `app/usecases/`；Library、History/Backup、Authenticity、Cleanup、Settings、Pin-board 的仓库
  入口已统一经 coordinator，命令名/DTO/用户文案不变；headless 改为 context 装配器并共享同一批
  use-case；命令注册集中在 `app/command_registry.rs`，最终成品的固化与发布编排移入
  `authenticity/artifact.rs`。画板写命令保持「仓库 lease + 仓库操作锁、不取共享运行锁」。
  `cargo check`（含 `headless` feature）、`cargo test --lib`、`npx tsc --noEmit`、`npm test`、
  `cargo fmt --check` 已通过；GUI 人工验收与压力测试**未做**。
- **阶段 2 已实现并提交、待维护者验收**：设置文件新增独立 `revision`，设置页走 CAS 并返回
  `settings_conflict`，托盘与窗口几何统一经 `SettingsService`；跨进程哨兵升级为 GUI/headless/
  维护入口共用的按写操作 owner lock（SQLite 内核锁即存活证明，owner 元数据只作诊断，锁文件
  从数据库发现/灾备/未引用扫描中排除）；退出结算的失败、5 秒未完成与原生 15 秒确认超时、
  等待活动操作的记录接入 `[watchdog]` 结构化诊断；前端按 §3.B 拆为 `AppRuntimeProvider`、
  `useSettingsController`、`useRepositoryMaintenance`、`RuntimeProgressProvider`、
  `SettingsDialog` 与 `NoticeCenter`。`cargo check --lib`（含 `headless`）、`cargo test --lib`、
  `cargo fmt --check`、`npx tsc --noEmit`、`npm test` 已通过；GUI 人工验收与压力测试**未做**。
  owner lock 的锁冲突不区分同进程，后台任务持锁期间前台写命令会直接拿到 `repository_busy`
  而不是排队等待，这一取舍连同其他待决项记在 `todo.md`。
- **阶段 3 已实现、待维护者验收**：新增统一 `AppEvent` 订阅 bridge（单原生监听、版本/仓库 key/
  时间戳过滤）与共享 `useRequestEpoch`、`useAsyncOperation` 工具；History、Library、
  Authenticity、Pin-board 四个 controller 统一请求代次、busy 与错误策略。History/备份运行态由
  事件即时唤醒，原 350 ms 高频轮询降为 3 秒兜底（作品树保留 30 秒兜底）；设置页维护进度在事件
  到达时按 150 ms 合并读取。素材板新增 `usePinBoardController`，生命周期、画板选择与保存/导入/
  导出状态移出组件；导入/导出进度改经 `CommandContext` 广播 `operationProgress`，不再使用
  `Channel`。`cargo check --features headless`、`cargo test --lib`、`npx tsc --noEmit`、
  `npm test`、`cargo fmt --check` 已通过；GUI 人工验收与压力测试**未做**。
- **阶段 4 已实现并提交、待维护者验收**：ChunkFile 新增 v2 写入（80 B snapshot / 120 B delta
  头、44 B 块记录、`stored_len=0` 快照去重）并保留 v1 reader，`chunkFileFormatV2` 应用设置默认开启、
  只影响新节点；新增 `src-tauri/src/format/container.rs`，`.clip` 按 CSFC/tile 边界做容器感知分块，
  `.psd` 只探测并回退 CDC，`min_size` 只约束 CDC 布局、结构单元只受 `max_size` 约束；
  `ChunkStore` 增加排序段索引与 reader 游标，`ChunkReader` 顺序/跳跃/跨段
  读取不再线性扫段；`materialization_chain` 改为一次递归 CTE；快速检查基线升级为版本化 cheap
  fingerprint（大小、纳秒 mtime、首尾 64 KiB，复用 `last_source_modified_ms`）；`CancelToken` 移到
  crate 根并贯穿创建、delta、复制、链解析、导出与 scrub，进度按字节报告；`stress_bench` 新增 P1B
  多段链 p50/p95 基准。`cargo check --lib`（含 `headless`）、`cargo check --features headless --test
  stress_bench`、`cargo test --lib`（224 通过）、`npx tsc --noEmit`、`npm test`（219 通过）、
  `cargo fmt --check` 与 `git diff --check` 已通过；P1B 与整套压力测试、GUI 人工验收**未运行**。
- **阶段 5 已实现、待维护者验收**：新增中立 `contracts` 模块（`ReferenceInventory` 与
  `RepositoryIntegrityProvider`），history/authenticity/pin_board 各自提供引用清单实现，应用层
  注册；cleanup 改为连接内 `TEMP` 引用表 + anti-join，按 512 条分批、可取消且幂等，未引用扫描
  改由 provider 声明命名与目录；灾备核心不再导入 Library/Authenticity，复制阶段同时计算完整
  SHA-256 与首尾 64 KiB 抽样摘要并写入 v2 manifest，发布前默认抽样复核（`Full` 保留为内部
  选项）。`BackupState` 改为每任务独立 `OperationId`/`CancelToken`，登记时区分占运行锁的串行任务
  与空闲校验准备阶段的并行只读探针（只有串行任务参与 `cancelled()` 判定）；调度器按 deadline 最小堆
  选择候选、单分支失败不再阻塞整队，空闲校验按 Artwork 并行（最多 4 个）并在连续 4 次自动备份
  后 aging 插队，写回仍串行。`stress_bench` 新增 P7（多分支候选扫描）与 P8（120 个孤儿 DDS
  扫描/确认清理），P5 记录文件数、吞吐与抽样复核。`cargo check --lib`、`cargo check --features
  headless --tests`、`cargo test --lib`（241 通过、1 忽略）、`npx tsc --noEmit`、`npm test`
  （219 通过）、`cargo fmt --check` 与 `git diff --check` 已通过；压力测试（含 P7/P8）、GUI
  人工验收**未运行**。
- **下一步**：由维护者验收阶段 1–5 的产品行为；随后进入阶段 6（仓储空间状态，alpha2）。
  alpha1 / alpha2 是开发批次，是否打 alpha 标签由维护者决定。
- 未完成事项全部在 `todo.md`：0.3 前的补强候选、阶段 1–5（待验收）与阶段 6–9。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；
  `dev` = 0.3 开发线，从 `0be6224` 创建。
- 版本：`dev` 尚未切 0.3 版本号，五处版本字段仍为 `0.2.0`；日常改动写入 `CHANGELOG.md`
  的 `## Unreleased`，切版本与打标签由维护者按 `docs/guides/release-policy.md` 单独决定。
- repository schema v4；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1），
  0.3-alpha1 保持 v4（仅连接内临时表），0.3-alpha2 阶段 7 执行唯一一次 v4 → v5 迁移。
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
