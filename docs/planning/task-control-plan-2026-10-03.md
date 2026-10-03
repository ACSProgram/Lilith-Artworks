# 任务调度总控与空闲链路校验规划（alpha3）

- 规划日期：2026-10-03
- 目标版本：`0.2.0-alpha.3`
- 状态：**待实施**（alpha.2 收尾验收完成后启动）
- 实施完成后：本文件移入 `archive/`，有效契约并入 `docs/modules/history-and-backup.md`，
  未完成项沉淀到 `todo.md`。

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
- 发布排除：进入发布状态的分支自动从调度查询中排除；发布前锁内 `ensure_checkpoint`；
- 状态预检：设置页长任务阻止排在已有备份操作之后；
- 认证模块自带 `AuthenticityState::begin_operation` 防重入。

## 2. 问题：四个缺口

1. **运行状态没有任务类型**。`BackupRuntimeStatus` 只有 `busy` + `active_branch_id`，
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

- `BackupRuntimeStatus` 增加 `task_kind` 字段，随 `busy` 一起写入与清除；
  前端轮询状态可据此区分展示（为空闲校验的进度与取消入口做准备）。
- 新增 `cancel_background()`：仅当当前运行的任务是 `AutomaticBackup` 或 `IdleVerify`
  时设置取消标志；`request_cancel()` 保持原语义（取消任何任务，供用户主动取消入口用）。
- 手动提交现有的"对同分支自动任务请求取消"改走 `cancel_background`，行为等价且覆盖校验。
- 所有前台长命令入口（恢复、精简、检查点、删除子树、进入发布、发布签名、全库扫描、
  整仓备份、仓库切换）在取得运行锁**之前**调用一次 `cancel_background()`：
  不阻塞、不排队，只是让正在运行的后台任务尽快退出，缩短前台等待。
- 退出路径不变：`shutting_down` 下排队任务被拒绝，现有测试覆盖继续有效。

### 4.2 校验状态持久化与派生队列（schema v4）

`branches` 表追加三列（沿用 v2→v3 的追加式迁移先例）：

```text
verified_history_id TEXT NULL   -- 最近一次完整校验通过的 head 节点
verified_ms         INTEGER NULL -- 校验完成时间
verify_error        TEXT NULL   -- 最近一次校验失败的摘要；成功时清空
```

- 应用版本随 alpha.3 发布递增；`tools/release/verify-metadata.mjs` 的 schema 断言同步为 v4。
- **派生队列**：查询所有未进回收站分支中 `head_history_id != verified_history_id`
  或 `verified_history_id IS NULL` 的分支。不要求 `backup_enabled`（手动提交的分支同样
  需要校验）；进入发布的分支同样纳入（其 head 是强制检查点，校验成本低）。
- 新提交、精简改接、检查点等改变 head 的操作天然使分支重新入队；重启后队列自动重建。
- 校验成功且锁内确认 head 未变时才写入 `verified_history_id`；校验期间 head 变化则不写，
  留待下一轮。

### 4.3 调度器循环改造：两级选择 + 逐链让位

`scheduler.rs` 的循环增加第二优先级，第一优先级（到期自动备份）逻辑一行不动：

1. **第一优先级**：到期自动备份（现状逻辑）。
2. **第二优先级**：无到期备份、无 `manual_pending`、自动备份未被暂停时，
   从派生队列取一条分支执行空闲校验：
   - 校验只处理"最后一次提交时间早于 `now - IDLE_VERIFY_DELAY_MS`（常量，默认 10 分钟）"
     的分支，避免刚写完就整链重读；
   - 每轮只处理**一个分支**，处理完回到循环顶部重新评估优先级——保证到期备份与前台
     操作的响应性；
   - 单分支校验在运行锁内执行，锁内复查 head 仍与队列快照一致；链间检查取消标志；
   - 校验实现复用从 `scrub_history`（`restore.rs`）中抽出的单节点校验函数：
     `resolve_chain` + `copy_original` 流入 sink，只校验不落盘；
   - 托盘"暂停所有自动备份"时空闲校验一并暂停（用户意图是"别动仓库"）；
   - 提交成功、精简、检查点等既有 `wake_scheduler` 调用点自然驱动校验排程，无需新增唤醒。

### 4.4 失败与警告面

- **失败**：写入 `verify_error`，该分支退出派生队列（不自动重试），直到 head 变化
  （新提交产生新链）或用户手动重查。失败不进入自动备份的失败退避体系，两者独立。
- **警告展示**：复用分支状态行模式——历史页分支状态区显示"链路校验失败"短摘要 +
  可展开详情（含复制入口），附"重新校验此分支"按钮（清除 error 重新入队）与
  "运行全库扫描"引导（设置页）。第一版不做作品库侧强制通知，避免与备份失败警告叠加造成干扰。
- **成功**：清空 `verify_error`，写 `verified_history_id` / `verified_ms`。
- 全库完整性扫描（`scrub_repository_integrity`）保留不动，仍是唯一的全量保证；
  空闲校验定位为"新链早发现"，两者互补，文档中明确此分工。

## 5. 非目标（明确不做）

- 不做通用任务队列、优先级插件化框架；后台任务类别显式写在枚举里，等出现第三、
  第四类主动方再评估泛化。
- 不把素材板自动保存纳入调度器：它有自己的防抖且只在用户编辑后触发，属于校验必须
  容忍的短占用者（校验取锁时自然等待它完成即可）。todo 中已记录的"长任务期间画板
  写入等待"问题独立评估，不在本批次处理。
- 不取消任何仓库读操作的即时性：`with_repository_read` 路径完全不感知本改动。
- 不改变快速检查、手动优先、退出握手的既有语义。

## 6. 实施批次

每批次独立提交、独立验证，出问题可按提交归因或单独回退。

### 批次 A：任务类型与取消路由

- `runtime.rs`：`BackupTaskKind` 枚举、`task_kind` 入状态、`cancel_background()`、
  移除 `active_automatic`。
- 手动提交路径改用 `cancel_background`；前台长命令入口补调用。
- 验证：`cargo fmt --check`、`cargo check --lib`、`cargo test --lib`
  （新增：cancel_background 只对后台任务生效、UserOperation 不被误取消、
  退出时排队任务仍被拒绝）；前端类型同步 `npx tsc --noEmit`、`npm test`。

### 批次 B：schema v4 与单节点校验重构

- 迁移 v3→v4（追加三列）；`verify-metadata.mjs` 断言同步。
- `restore.rs`：从 `scrub_history` 抽出单节点校验函数（公开为 `backup::verify_history_node`），
  `scrub_history` 改为循环调用它，行为不变。
- 验证：`cargo fmt --check`、`cargo test --lib`（迁移用例、校验函数单测、
  scrub 行为不变回归）；`node tools/release/verify-metadata.mjs`。

### 批次 C：调度器两级选择、派生队列与警告面

- `history/`：派生队列查询、校验结果写入、单分支重查命令；
- `scheduler.rs`：第二优先级选择、`IDLE_VERIFY_DELAY_MS`、逐链让位；
- 前端：分支状态行校验失败摘要与详情、"重新校验此分支"按钮；
- 验证：`cargo fmt --check`、`cargo test --lib`（调度器两级选择、让位、
  head 变化重新入队、失败后退出队列、暂停联动）、`npx tsc --noEmit`、`npm test`。

### 批次 D：文档与发布收尾

- `docs/modules/history-and-backup.md` 更新（调度、校验、任务类型契约）；
- `todo.md` 增加验收项（见第 8 节）；本文件移入 `archive/`；
- `CHANGELOG.md` 记录；发布时版本号、标签按 `docs/guides/release-policy.md` 执行。

## 7. 不变量与风险

**不变量：**

- 无待校验分支时，调度器的查询与等待行为与现状完全一致；
- 校验只读 ChunkFile 与历史库，只写 `branches` 的三个校验列，不创建、不改接任何历史实体；
- 校验失败永不自动禁用备份、不阻断提交与恢复、不触发失败退避；
- 显式退出时，排队中的校验被拒绝执行（复用 `run_exclusive_typed` 既有语义）；
- 全库扫描仍是唯一的全量完整性保证。

**风险与缓解：**

- 长链校验推迟到期备份：单分支限量 + 备份优先级更高，最多推迟一轮循环；
- 大提交后立即校验的成本尖峰：`IDLE_VERIFY_DELAY_MS` 延迟 + 每轮一分支节流；
- 校验与备份失败警告并存的信息过载：两者在分支状态行分区显示、语义独立，不合并文案；
- schema v4 迁移：沿用 v3 追加式先例，迁移用例完整模拟 v3 结构断言 v4。

## 8. 实施后新增的人工验收项

落实后加入 `todo.md` 待验收清单：

- 空闲校验进行中触发取消（用户操作进入、托盘暂停、显式退出）；
- 校验失败警告的展示、详情展开与"重新校验"清除路径；
- 快速检查分支与空闲校验的组合行为（快速记未变化后新链仍被校验）；
- 大链校验期间前台操作（提交、发布、恢复）的响应性；
- 校验失败后继续手动提交是否正常（失败不阻断）。
