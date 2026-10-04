# 任务调度总控与空闲链路校验规划（alpha3）

- 规划日期：2026-10-03
- 目标版本：`0.2.0-alpha.3`（schema v4 并入当前 alpha.3；把应用版本号递增为
  `0.2.0-alpha.4` 是维护者的后续独立动作，不在本批次内）
- 状态：**已实施并归档**（2026-10-04；批次 A–C 实现、批次 D 收尾，见批次 A–C 提交与
  `docs/planning/current-handoff.md`）。有效契约已并入 `docs/modules/history-and-backup.md`，
  待人工验收项沉淀到 `todo.md` 第二节。
- 实施完成后：本文件已移入 `archive/`；后续修改变更以模块文档为准，不再回改本文件。

## 1. 背景：现状盘点

### 1.1 锁拓扑（本轮核实的事实）

并发控制分两层，互相独立：

- **仓库锁**（`AppState`，`src-tauri/src/app/settings.rs`）：
  - `with_repository_read`：只读租约，树刷新、历史读取、素材板浏览随时可进；
  - `with_ready_repository`：只读租约 + `repository_operation` 互斥锁，所有仓库写操作在此串行；
  - `with_repository_switch`：独占写租约，仓库切换排他。
- **备份操作锁**（`BackupState::operation_lock`，`src-tauri/src/backup/runtime.rs`）：
  长操作互斥。`run_exclusive` / `run_logged` 是统一入口，运行状态经 `BackupRuntimeStatus`
  暴露，取消与退出语义有测试覆盖。

素材板保存**不经过备份锁**，只走仓库锁（`pin_board/mod.rs` 模块注释明确此设计）。

### 1.2 竞争者清单

| 类别 | 成员 | 特征 |
| --- | --- | --- |
| 主动方（时间/事件驱动） | 自动备份调度循环；素材板防抖自动保存（alpha.2 新增，静默 1.5s 落库）；规划中的空闲链路校验 | 任意时刻出现，需要排程与让位 |
| 前台命令（用户等待结果） | 手动提交、恢复、精简、检查点、删除子树、创建分支、重命名、分支删除、库删除、清空回收站、分支设置保存 | 永远最高优先级，不被排队 |
| 前台长任务（用户触发、占锁久） | 发布签名（C2PA/TrustMark）、进入发布、全库完整性扫描、整仓备份、仓库切换 | 需要预检、可取消、进度 |

### 1.3 既有特设协调机制

目前的冲突靠散落在各处的点对点机制解决：

- 手动提交优先：`manual_pending` 登记集合，调度器在候选选择与取得运行锁后两处复查；
  同分支自动任务运行中则对其请求取消（`active_automatic` 布尔标记判断）；
- 加锁后资格复查：调度器取得运行锁后重新验证分支仍启用、未回收站、未发布、仍到期；
- 发布排除：进入发布状态的分支自动从**备份调度查询**中排除（注意：空闲校验队列是
  另一条查询，见 4.2，**不**排除已发布分支）；发布前锁内 `ensure_checkpoint`；
- 状态预检：设置页长任务阻止排在已有备份操作之后；
- 认证模块自带 `AuthenticityState::begin_operation` 防重入。

## 2. 问题：四个缺口

1. **运行状态没有任务类型**。`BackupRuntimeStatus` 已有 `busy`、`active_branch_id`、
   `operation`、`progress_label`、`progress_current`、`progress_total`、
   `automatic_scheduling`、`completion_revision`，但没有任务类型字段，
   UI 分不清"用户触发的关键操作"与"后台低优先级任务"。
2. **取消请求无法路由**。`request_cancel` 是共享的；手动提交对自动任务的取消依赖
   `active_automatic` 单布尔，空闲校验加入后无法区分"取消后台任务"与"取消用户自己的操作"。
3. **"空闲"没有定义**。调度器只知道"有无到期备份"，没有"无前台等待时才做维护工作"的统一判定。
4. **让位机制特设化**。每加一类后台任务都要重新发明一次让位（下一个就是空闲校验）。

## 3. 设计原则

1. **规划层只管主动方。** 前台命令不排队、不让位规划管——用户在等。规划层的职责是
   让主动方给前台让路，而不是把所有操作都纳入调度。
2. **不为抽象而抽象。** 不做通用任务队列/插件框架；所有改动收敛在现有 `BackupState` +
   `scheduler.rs` 循环内，复用既有的等待/唤醒/退避/资格复查机制。
3. **零行为变化兜底。** 没有待校验任务时，调度器行为与现状完全一致；校验失败只记警告，
   永不禁用备份、不阻断提交与恢复。
4. **状态持久化走派生，不走队列。** 不维护独立的待办队列；用"head 与已验证记录是否一致"
   派生队列，重启自愈，无队列状态可损坏。

## 4. 总控模型设计

### 4.1 任务类型与取消路由（BackupState 扩展）

`runtime.rs` 引入任务类型枚举，替换现有 `active_automatic` 布尔：

```text
enum BackupTaskKind { AutomaticBackup, IdleVerify, UserOperation }
```

**任务类型必须贯穿三层入口的签名**（这是本批次最大的机械改动面）。当前 `busy` 与
`active_branch_id` 是在 `run_exclusive_typed` 内部写入与清除的，`task_kind` 必须与它们
在同一临界区内写入、在同一次 `*runtime = BackupRuntimeStatus { ..Default::default() }`
中清除，因此：

- `run_exclusive_typed` / `run_exclusive` / `run_logged` 增加 `kind: BackupTaskKind` 参数
  （`run_exclusive` / `run_logged` 若保留无 kind 便捷包装，默认 `UserOperation`）；
- 需要同步的调用点（约 20 处）：`backup/commands.rs`（5）、`app/workflows.rs`（11）、
  `app/settings.rs`（1）、`app/cleanup_commands.rs`（1）、`backup/scheduler.rs`（1）。

行为约定：

- `BackupRuntimeStatus` 增加 `task_kind` 字段；前端轮询状态可据此区分展示
  （为空闲校验的进度与取消入口做准备）。
- 新增 `cancel_background()`：仅当当前运行任务的 `task_kind` 是 `AutomaticBackup` 或
  `IdleVerify` 时设置取消标志；`request_cancel()` 保持原语义（取消任何任务，
  供用户主动取消入口用）。
- 手动提交现有的"对同分支自动任务请求取消"改走 `cancel_background`，行为等价且覆盖校验。
- 所有前台长命令入口（恢复、精简、检查点、删除子树、进入发布、发布签名、全库扫描、
  整仓备份、仓库切换）在取得运行锁**之前**依次做两件事：调用一次 `cancel_background()`
  让正在运行的后台任务尽快退出；并登记"有前台命令在等待"（见下）。两者都不阻塞、不排队。
- 退出路径不变：`shutting_down` 下排队任务被拒绝，现有测试覆盖继续有效。

**取消路由的可靠性：采用"可靠让位"（已定）。** `cancel_requested` 是**单个**共享
AtomicBool，且 `run_exclusive_typed` 是在**取得运行锁之后**才清零它。因此仅靠
"前台先置位、后台退出、前台再取锁"这条链并不可靠：运行锁是普通 `Mutex`，不保证前台
拿到下一个名额；调度器线程可能在后台退出后抢先取锁，进入时把取消标志清零，从而
**吞掉**前台的取消意图（此时前台仍在等待，且可能重复发生）。

为此新增前台等待登记 `foreground_waiting: AtomicUsize`：

- 前台长命令入口在尝试取得运行锁**之前**自增，在取得锁之后自减；用 RAII guard 保证
  取消、失败、提前返回等所有路径都会自减；
- 调度器在候选选择阶段即可跳过（有前台在等时不选新任务），并在**取得运行锁之后**复查
  该计数，`> 0` 即视为有前台命令在等待，立即以"让位"结果退出、不放行本次后台任务；
- 让位后进入既有的退避等待，避免与前台争抢运行锁形成忙等。

这与既有 `manual_pending` 是同一套"锁内复查后让位"模式，复用其已验证语义，
不新增锁、不引入队列。因此本节的收益是**确定**的：前台命令不会被调度器抢先取锁。

### 4.2 校验状态持久化与派生队列（schema v4）

`branches` 表追加三列（沿用 v2→v3 的追加式迁移先例）：

```text
verified_history_id TEXT NULL   -- 最近一次完整校验通过的 head 节点
verified_ms         INTEGER NULL -- 校验完成时间
verify_error        TEXT NULL   -- 最近一次校验失败的摘要；成功时清空
```

- `SCHEMA_VERSION` 递增为 4；`tools/release/verify-metadata.mjs` 的 schema 断言同步为 4。
  应用版本号本次**不变**（仍为 `0.2.0-alpha.3`）；版本递增由维护者后续单独执行。
- **派生队列**：查询所有未进回收站分支中，`head_history_id` 非空且与
  `verified_history_id` 不一致（含 `verified_history_id IS NULL`）的分支。注意 SQL 的
  NULL 语义：`head_history_id` 允许为 NULL（分支已建但未提交），
  `NULL != NULL` 为 UNKNOWN，若只写"两列不相等或后者为空"会把**所有无 head 的分支**
  纳入队列并每轮空跑。条件必须显式写成：

  ```sql
  head_history_id IS NOT NULL
    AND (verified_history_id IS NULL OR verified_history_id != head_history_id)
  ```

- 回收站过滤需要经 `artworks → library_nodes.trashed_ms` 判断；直接复用
  `list_scheduled` 现有的过滤写法，不要重新推导 join。
- 派生队列**不要求** `backup_enabled`（手动提交的分支同样需要校验）；进入发布的分支
  同样纳入（其 head 是强制检查点，校验成本低）。**这条查询与 `list_scheduled` 是两条
  不同查询**：后者排除已发布分支，前者不排除。
- 新提交、精简改接、检查点等改变 head 的操作天然使分支重新入队；重启后队列自动重建。
- 校验成功且锁内确认 head 未变时才写入 `verified_history_id`；校验期间 head 变化则不写，
  留待下一轮。

### 4.3 调度器循环改造：两级选择 + 逐链让位

`scheduler.rs` 的循环增加第二优先级，第一优先级（到期自动备份）逻辑一行不动：

1. **第一优先级**：到期自动备份（现状逻辑）。
2. **第二优先级**：无到期备份、无 `manual_pending`、无前台命令在等待
   （`foreground_waiting == 0`）、自动备份未被暂停时，从派生队列取一条分支执行空闲校验：
   - 校验只处理"head 节点的 `created_ms` 早于 `now - IDLE_VERIFY_DELAY_MS`（常量，
     默认 10 分钟）"的分支，避免刚写完就整链重读。
     **必须用 head 节点的 `history_nodes.created_ms`（或 `branches.updated_ms`）；
     不得用 `last_check_ms`**——快速检查的"内容未变化"会推进 `last_check_ms`
     但不改变 head，用它会让延迟条件永不满足；
   - 每轮只处理**一个分支**，处理完回到循环顶部重新评估优先级——保证到期备份与前台
     操作的响应性；
   - 单分支校验在运行锁内执行，锁内复查 head 仍与队列快照一致；链间检查取消标志；
   - 校验实现见 4.5，复用既有单节点校验函数；
   - 托盘"暂停所有自动备份"时空闲校验一并暂停（用户意图是"别动仓库"）；
   - 提交成功、精简、检查点等既有 `wake_scheduler` 调用点自然驱动校验排程，无需新增唤醒。

### 4.4 失败与警告面

- **失败**：写入 `verify_error`，该分支退出派生队列（不自动重试），直到 head 变化
  （新提交产生新链）或用户手动重查。失败不进入自动备份的失败退避体系，两者独立。
- **警告展示**：复用分支状态行模式——历史页分支状态区（`HistoryControls.tsx` 的
  `BranchSchedule`）显示"链路校验失败"短摘要 + 可展开详情（含复制入口），附
  "重新校验此分支"按钮（清除 error 重新入队）与"运行全库扫描"引导（设置页）。
  该组件目前以 `branch.lastError` 为唯一门控，需为校验错误增加**独立**状态分支，
  不与备份失败文案合并。
- **成功**：清空 `verify_error`，写 `verified_history_id` / `verified_ms`。
- **前端 DTO 与夹具同步**：`ArtworkBranch` 增加 `verifyError` / `verifiedMs` 后，需同步
  `src/modules/history/types.ts`，以及至少这些测试夹具：`historyModel.test.ts`、
  `HistoryModule.test.tsx`（2 处）、`HistoryControls.test.tsx`（2 处）。
  另外 `BackupRuntimeStatus` 在 `src/app/types.ts` 与 `src/modules/history/types.ts`
  **各有一份独立 interface**，`task_kind` 两处都要加，相关夹具（`App.tsx`、
  `useHistoryController.ts`、`App.repositorySwitch.test.tsx`×2、
  `useHistoryController.test.tsx`、`HistoryModule.test.tsx`）同样要同步。
  （该重复定义本身是否合并，另行评估，不在本批次内。）
- 全库完整性扫描（`scrub_repository_integrity`）保留不动，仍是唯一的全量保证；
  空闲校验定位为"新链早发现"，两者互补，文档中明确此分工。

### 4.5 单节点校验的实现复用（已核实，取代原"从 scrub_history 抽取"方案）

原方案拟从 `scrub_history` 抽出单节点校验函数并公开为 `backup::verify_history_node`。
经核实**不需要**，理由如下（三条均为代码事实）：

1. `materialization_chain`（`history/repository.rs:339-341`）在目标节点持有 snapshot 时
   直接返回 `vec![target]`，即 `resolve_chain(head)` 只会读取 head 自身那一个 snapshot 文件；
2. `commit`（`history/repository.rs:412-454`）以非可选的 `snapshot_path` 插入新节点并令其
   成为 head，只释放**父节点**的 snapshot；
3. `unmark_checkpoint`（`history/repository.rs:652-659`）拒绝取消 head / 分支起点 / 分叉点的
   检查点，因此 head 永远无法处于"无 snapshot"状态。

结论：**校验分支 head 等价于校验 head 的单个 snapshot 文件**，而 `validate_snapshot`
（`backup/restore.rs:326`）已经做了 `ChunkFile::file_digest()` 与数据库 `sha256` 比对 +
`copy_original` 流入 sink 的全量分块校验，且已被 `ensure_checkpoint_with_progress` 复用。

因此本批次只需把 `validate_snapshot` 提升为 `pub(crate)`（并按需要在 `backup` 模块内
包一层命名清晰的入口），**不新增 `resolve_chain` 依赖、不做函数抽取重构**。
`scrub_history` 保持原样不动。

## 5. 非目标（明确不做）

- 不做通用任务队列、优先级插件化框架；后台任务类别显式写在枚举里，等出现第三、
  第四类主动方再评估泛化。
- 不把素材板自动保存纳入调度器：它有自己的防抖且只在用户编辑后触发，属于校验必须
  容忍的短占用者（校验取锁时自然等待它完成即可）。todo 中已记录的"长任务期间画板
  写入等待"问题独立评估，不在本批次处理。
- 不取消任何仓库读操作的即时性：`with_repository_read` 路径完全不感知本改动。
- 不改变快速检查、手动优先、退出握手的既有语义。
- 不合并 `src/app/types.ts` 与 `src/modules/history/types.ts` 中重复的
  `BackupRuntimeStatus` 定义（仅同步字段）。

## 6. 实施批次

每批次独立提交、独立验证，出问题可按提交归因或单独回退。

### 批次 A：任务类型与取消路由

- `runtime.rs`：`BackupTaskKind` 枚举；`run_exclusive_typed` / `run_exclusive` /
  `run_logged` 增加 kind 参数；`task_kind` 与 `busy` 同临界区写入与清除；
  `cancel_background()`；前台等待登记 `foreground_waiting`（含 RAII guard）；
  移除 `active_automatic`。
- 同步全部调用点：`backup/commands.rs`（5）、`app/workflows.rs`（11）、
  `app/settings.rs`（1）、`app/cleanup_commands.rs`（1）、`backup/scheduler.rs`（1）。
- 手动提交路径改用 `cancel_background`；前台长命令入口补调用并登记 `foreground_waiting`。
- 前端：`src/app/types.ts` 与 `src/modules/history/types.ts` 两份 `BackupRuntimeStatus`
  各加 `taskKind`，并同步其夹具。
- 验证：`cargo fmt --check`、`cargo check --lib`、`cargo test --lib`
  （新增：cancel_background 只对后台任务生效、UserOperation 不被误取消、
  退出时排队任务仍被拒绝、`foreground_waiting` 在正常/取消/失败路径都正确归零、
  有前台在等时调度器让位）；`npx tsc --noEmit`、`npm test`、`git diff --check`。

### 批次 B：schema v4 与单节点校验入口

- 迁移 v3→v4（追加三列）；`SCHEMA_VERSION` 递增为 4；`verify-metadata.mjs` 断言同步为 4。
- **同步修正两处硬编码旧版本号的既有断言**（不改会直接失败）：
  - `src-tauri/src/pin_board/repository.rs:1685`：模拟 v1 → 当前版本的全链迁移后断言
    `version == 3`，改为 `4`；
  - `src-tauri/src/app/settings.rs:825`：把 `schema_version` 写回 `'3'` 并断言仓库可打开，
    改为 `'4'`（其上一处 `'99'` 的"版本不受支持"断言保持）。
- `restore.rs`：`validate_snapshot` 提升为 `pub(crate)`（不改其校验逻辑）；
  `scrub_history` 保持原样，不做抽取。
- 验证：`cargo fmt --check`、`cargo test --lib`（迁移用例、v1 全链迁移断言、
  设置页版本断言、`validate_snapshot` 单测）；`node tools/release/verify-metadata.mjs`；
  `git diff --check`。

### 批次 C：调度器两级选择、派生队列与警告面

- `history/`：派生队列查询（按 4.2 的 SQL 与回收站过滤）、校验结果写入、
  单分支重查命令；
- `scheduler.rs`：第二优先级选择、`IDLE_VERIFY_DELAY_MS`、head 时间字段、逐链让位；
- 前端：`ArtworkBranch` 增加 `verifyError` / `verifiedMs`；分支状态行校验失败摘要与详情、
  "重新校验此分支"按钮；同步 `types.ts` 与 4.4 列出的夹具；
- 验证：`cargo fmt --check`、`cargo test --lib`（调度器两级选择、让位、
  head 变化重新入队、失败后退出队列、暂停联动、NULL head 不入队）、
  `npx tsc --noEmit`、`npm test`、`git diff --check`。

### 批次 D：文档与发布收尾

- `docs/modules/history-and-backup.md` 更新（调度、校验、任务类型契约、校验与全库扫描分工）；
- `todo.md` 增加验收项（见第 8 节）；本文件移入 `archive/`；
- `CHANGELOG.md`：把本批次条目追加到**现有** `## 0.2.0-alpha.3 - 2026-10-03` 小节
  （不新建版本段）；应用版本号不变，递增为 `alpha.4` 由维护者后续单独执行；
- 发布时版本号、标签按 `docs/guides/release-policy.md` 执行。

## 7. 关键决定与待确认事项

**已定：取消路由采用"可靠让位"（选项 a）。** 背景：共享取消标志 `cancel_requested` 是
单个 AtomicBool，且 `run_exclusive_typed` 在取得运行锁后才清零它，因此"前台置位 →
后台退出 → 前台取锁"这条链不保证成功——调度器可能在后台退出后抢先取锁并清零标志，
吞掉前台的取消意图。决定：新增前台等待登记 `foreground_waiting`，调度器取得运行锁后
复查，有人在等即让位；这与既有 `manual_pending` 是同一套已被验证的模式。
实现细节见 4.1。

**待确认：`IDLE_VERIFY_DELAY_MS` 默认值**取 10 分钟是否合适（见 4.3）。

## 8. 不变量与风险

**不变量：**

- 无待校验分支时，调度器的查询与等待行为与现状完全一致；
- 校验只读 ChunkFile 与历史库，只写 `branches` 的三个校验列，不创建、不改接任何历史实体；
- 校验失败永不自动禁用备份、不阻断提交与恢复、不触发失败退避；
- 显式退出时，排队中的校验被拒绝执行（复用 `run_exclusive_typed` 既有语义）；
- 全库扫描仍是唯一的全量完整性保证。

**风险与缓解：**

- 长链校验推迟到期备份：单分支限量 + 备份优先级更高，最多推迟一轮循环；
- 大提交后立即校验的成本尖峰：`IDLE_VERIFY_DELAY_MS` 延迟 + 每轮一分支节流；
  且 head 校验退化为单 snapshot 校验（4.5），成本上限就是该文件的全量分块读取；
- 校验与备份失败警告并存的信息过载：两者在分支状态行分区显示、语义独立，不合并文案；
- 取消标志被下一任务吞掉（4.1）：由前台等待登记 `foreground_waiting` 消除（已定）；
- schema v4 迁移：沿用 v3 追加式先例，迁移用例完整模拟 v3 结构断言 v4；
  同时修正批次 B 列出的两处硬编码旧版本号断言。

## 9. 实施后新增的人工验收项

落实后加入 `todo.md` 待验收清单：

- 空闲校验进行中触发取消（用户操作进入、托盘暂停、显式退出）；
- 校验失败警告的展示、详情展开与"重新校验"清除路径；
- 快速检查分支与空闲校验的组合行为（快速记未变化后新链仍被校验）；
- 大链校验期间前台操作（提交、发布、恢复）的响应性；
- 校验失败后继续手动提交是否正常（失败不阻断）。
