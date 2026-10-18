# 同仓库写入排队：消除同进程 owner lock 冲突

状态：已实施，待维护者人工验收（计划正文保留；实施与验证记录见文末「实施记录」）
目标版本：0.3 / alpha2
关联条目：原 `docs/planning/todo.md:77`「后台任务持锁期间的前台写命令」（已实现，该行已从 `todo.md` 删除）
文档性质：独立计划，不写入 `todo.md`；实施完成后按 AGENTS 的归档规则归入本目录

---

## 1. 现象

用户报告的三个表现，实际是同一个原因：

1. 打开旧仓库时偶发提示 **「仓库正在被另一个进程写入；不支持同一仓库的多进程写入」**；
   不只首次打开，后续打开也会偶发。
2. 出现该提示的那次启动，界面上仓库会停在不可用状态（不只是一条 toast）。
3. 关闭窗口时日志出现 `pin-board finalize failed`，随后
   `shutdown settlement finished: outcome=failed` —— 画板可能没有结算。

### 1.1 结论

根因**不是另一个进程**，而是同一进程内的两条写路径抢同一把跨进程 owner lock，
而该锁的语义被设计成「冲突立即拒绝、不排队」：

- 调度线程在启动后立刻取锁做自动备份（持锁 4–40 秒）；
- 前端启动时发一个写命令 `retryFileCleanup([])`（清理队列重放）；
- 第二个写者拿不到锁，直接返回 `repository_busy`。

**`.lilith-write-lock.sqlite3` 缺失只是相关特征，不是原因。** 该锁库在第一次带锁写入时
由 SQLite 自动创建，此后通常是 0 字节；「文件不存在」=「这个仓库还没被带 owner lock 的
版本写过」= 首次打开，所以首次打开必撞、1 分钟内重开不撞（分支间隔 1 分钟，未到期）、
隔一会儿再开又可能撞。

### 1.2 证据（2026-10-18 当次运行日志）

日志：`%LOCALAPPDATA%\com.lilith.artworks\logs\Lilith Artworks.log`

- 146 条 `[slow] repository mutation held the lock for ...`，中位 **3,903 ms**，
  p90 **8,875 ms**，最大 **40,234 ms**（写操作确实会长时间持锁）。
- 12 次 `[owner] repository write lock busy`，持锁者**全部**是
  `operation=backup:automatic`、`role=gui`，且 `run_id` 与冲突方相同（同一进程）。
- 冲突方命令域分布：`cleanup` 10 次（启动时 `retryFileCleanup([])`，`elapsed_ms=0`，
  属于必然拿不到）；`pinBoard` 2 次（退出/切换时的画板结算）。
- 当时 3 个分支的 `backup_interval_minutes` 均为 1，几乎每次冷启动都恰好到期。

---

## 2. 现有机制（下一轮无需再查）

### 2.1 锁的两层

| 层 | 实现 | 语义 |
| --- | --- | --- |
| 跨进程 owner lock | `src-tauri/src/repository_lock.rs`，仓库根 `.lilith-write-lock.sqlite3` 上的 `BEGIN IMMEDIATE`，`busy_timeout=0`（`open_lock_database`，:282） | 冲突立即 `OwnerLockError::Busy`；`BUSY_MESSAGE` 在 :36 |
| 进程内串行 | `AppState.repository_operation`（`Mutex`，声明在 `app/settings.rs:241`，获取在 `:375`）+ `BackupState.operation_lock` | 在 owner lock **之后**才取得，只负责让进程内写者互斥 |

关键：第二层在锁序里排在 owner lock 后面（`docs/architecture/overview.md:110` 冻结的锁序），
所以同进程的第二个写者在拿到任何进程内互斥量之前就已经被跨进程锁拒掉了。

### 2.2 取锁点（全部是零等待）

| 调用方 | 位置 |
| --- | --- |
| 普通写命令 | `app/command_runtime/coordinator.rs:228`（`run_write_scoped`） |
| 前台长命令 | `coordinator.rs:249`（`run_foreground`） |
| 后台命令 | `coordinator.rs:291`（`run_background`） |
| 素材板写命令 | `coordinator.rs:311`（`run_repository_write`） |
| 设置保存 / 切仓库 | `coordinator.rs:337`（`run_repository_switch`） |
| 统一入口 | `coordinator.rs:381` `acquire_owner_for_root` → `RepositoryOwnerLock::acquire` |
| 调度器 | `backup/scheduler.rs:73`（`backup:candidates`）、`:169`（`backup:automatic`）、`:437`（`backup:idleVerify`），都经 `:779 acquire_owner` |
| 无头 CLI | `src-tauri/src/headless.rs:762`（单进程单命令，不需要排队） |

### 2.3 前台让位为什么救不了

`run_foreground` 在 coordinator.rs:249 **先**取 owner lock，`cancel_background()` 是在
`BackupState::run_foreground_typed`（`backup/runtime.rs:296-305`）里才调用；owner lock
一失败就直接返回，**根本没走到让位那一步**。这就是 `todo.md:77` 记录的 0.2 → 0.3 回归。

### 2.4 启动顺序放大冲突

- `lib.rs:357-359`：setup 阶段就 `start_scheduler`，第一轮 `wait_scheduler(Duration::ZERO)`
  不等待，立即取锁做候选筛选与到期备份。
- `src/app/useSettingsController.ts:93-102`：`load()` 在 `Promise.all` 之后**同步等待**
  `appApi.retryFileCleanup([])`（写命令），然后才 `setSnapshot / setRepository`。
- 两者在启动 1 秒内相遇；写命令被拒 → `load` 抛错进 catch → 已取到的 snapshot 与
  repository 状态**没有回填**，界面按初始的 `EMPTY_REPOSITORY_STATUS` 显示「仓库不可用」。

### 2.5 退出结算

`src/app/AppRuntimeProvider.tsx:68-102` 收到 `app_shutdown_requested` 后调用
`preparePinBoardRuntimeChange()`；渲染器在 `src/modules/pin-board/renderer.ts:1308-1329`
里先保存再 `finalizePinBoard`，失败只记 `warn` 并返回 false。原生端 15 秒兜底强退
（`lib.rs:127 begin_webview_shutdown`）。当前实现中该写命令同样走 `run_repository_write`
→ 撞锁 → 结算失败。

---

## 3. 目标与非目标

### 目标

1. 同进程写操作**串行排队**，不再互相以 `repository_busy` 拒绝。
2. 前台长命令优先于交互写，交互写优先于后台；后台**永不排队**（保持「跳过 + 短退避」）。
3. 跨进程边界不变：另一个进程持锁时仍然立即拒绝，用户可见文案不变。
4. 启动不再因为一次写命令 busy 而丢掉仓库状态。
5. 退出结算不再因为 busy 丢画板。
6. 所有等待都有上限且可取消，不会无限挂住 UI 或退出流程。

### 非目标

- 不支持同仓库多进程写入（硬边界不变）。
- 不改 SQLite `busy_timeout` / WAL 策略，不改跨进程「零等待」判定。
- 不做跨进程排队、不做锁租约/心跳接管/PID 回收。
- 不改 1 分钟自动备份间隔的默认值（是否需要提高另议，见 §11）。

---

## 4. 设计

### 4.1 进程内写入闸门（write gate）

在 `repository_lock.rs` 新增（与现有 owner lock 同文件，因为它们是同一件事的两层）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WritePriority {
    Shutdown,     // 退出结算：插队到最前
    Foreground,   // 恢复、精简、检查点、整仓备份、发布、全库扫描
    Interactive,  // 普通写命令、素材板结算、清理重放
    Background,   // 自动备份、空闲校验：只 try，不排队
}

pub(crate) fn acquire_write_gate(
    root: &Path,
    priority: WritePriority,
    wait: WaitPolicy,        // Try | Until(Instant) | Until(Instant) + cancel
    cancel: &CancelToken,
) -> Result<WriteGateGuard, GateError>;  // Busy | TimedOut | Cancelled
```

实现要点：

- 进程级注册表：`OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>>`，键用
  `root.canonicalize()`（失败时退回原路径），保证 `F:\repo`、`f:\REPO`、短路径映射同一把闸门。
- `Gate` 内部：`Mutex<GateState>`（`holding: bool`、`waiters: BinaryHeap<(Reverse<WritePriority>, ticket)>`、
  `next_ticket: u64`）+ `Condvar`。
- 等待循环：`wait_timeout(50ms)` 轮询 `cancel.is_cancelled()` 与 deadline，命中即出队返回。
  取锁成功条件：`!holding && 自己是最小 (priority, ticket)`。
- `Background` 直接 `try_acquire`：不可用立即 `Busy`，不登记等待者。
- Guard `Drop`：`holding = false`，`notify_all`；若 `waiters` 为空则把 key 从注册表移除
  （切仓库后不泄漏、也不会让内存随仓库数增长）。
- **非重入**：`Debug` 构建用 thread-local 记录本线程已持有的 gate key；同线程重复 acquire
  直接 `log::error!` + `debug_assert!(false)`（见 §6.3 的调用点审计）。

### 4.2 与跨进程锁的组合

新增 `RepositoryOwnerLock::acquire_queued(root, role, operation, priority, wait, cancel)`：

1. 先取进程内 gate（**排队点在这里**）。
2. 再做一次 `BEGIN IMMEDIATE`（仍是零等待）：
   - 成功 → 写 owner 元数据，返回锁对象；锁对象**同时持有** gate guard 与 SQLite 连接，
     `Drop` 顺序为「先 `ROLLBACK` 释放 SQLite 锁，再放 gate」。
   - `DatabaseBusy` → 立刻释放 gate，返回 `Busy(owner)`。跨进程语义**完全不变**：
     即使本进程没有其他写者，只要另一进程持锁，调用方仍立刻拿到 `repository_busy`。
   - 日志里额外打印「owner.pid 是否等于本进程 pid」，把「伪造的跨进程冲突」与真实冲突区分开。
3. 现有 `RepositoryOwnerLock::acquire` **保持不变**（零等待、不排队）：`probe`、headless、
   `repository_lock.rs:300` 的「第二个写者被拒」单测仍然用它。

为什么不把 `AppState.repository_operation` 提到 owner lock 之前：那会反转文档冻结的锁序
（`owner → 运行锁 → lease → repository_operation`），与设置切换的写 lease 形成反向获取，
死锁风险高于收益。

### 4.3 优先级与让位

| 调用方 | 优先级 | 等待策略 | 备注 |
| --- | --- | --- | --- |
| `run_foreground` | Foreground | `Until(now + 60s)` + cancel | **先** `begin_foreground_wait()` + `cancel_background()`，**再**取 gate（修复 §2.3） |
| `run_repository_switch` | Foreground | `Until(now + 30s)` + cancel | 切仓库本来就等 lease |
| `run_write` / `run_repository_write` | Interactive | 先 try；失败才 `cancel_background()` 并 `Until(now + 10s)` | 只在真有竞争时才请求后台让位，避免每次画板保存都发取消 |
| 退出结算（`finalize_pin_board`） | Shutdown | `Until(now + 10s)` | 给 15s 原生兜底留 5s 余量 |
| 调度器三处 | Background | Try | 拿不到即 `Skipped`，`wait_scheduler(FOREGROUND_YIELD_BACKOFF)` |
| headless | — | 不排队 | 单进程单命令，保持 `acquire` |

等待上限放常量（`coordinator.rs` 顶部）：`INTERACTIVE_WRITE_WAIT = 10s`、
`FOREGROUND_WRITE_WAIT = 60s`、`SHUTDOWN_WRITE_WAIT = 10s`，注释写明依据是「实测最大持锁
40,234 ms」以及「前台先请求后台让位后，后台通常在一个分块边界内退出」。

### 4.4 超时与错误语义

- 跨进程 Busy → `CommandError::busy(BUSY_MESSAGE)`，保持现状（`coordinator.rs:527-532`）。
- 进程内等待超时 → 仍是 `code=repository_busy`、`retryable=true`，但文案改为
  **「仓库正在写入，请稍后重试」**，避免在同进程竞争时误报「另一个进程」。
  （可选加强：若 `owner.pid == process::id()`，日志标 `same_process=true`。）
- 等待中被取消 → `code=operation_cancelled`（复用现有取消映射），不进入业务闭包。

### 4.5 调度器：后台不排队

`backup/scheduler.rs:779 acquire_owner` 增加 `Skipped` 分支：

```rust
enum OwnerAttempt { Owned(Option<RepositoryOwnerLock>), Skipped }
```

- 进程内 gate 被占（本进程有前台/交互写）→ `log::debug!("scheduler yields to an in-process writer")`
  → `wait_scheduler(FOREGROUND_YIELD_BACKOFF)`，**不写分支失败、不推进退避**。
- 真实跨进程 Busy → 保持现有 `warn` + `wait_scheduler(ERROR_RETRY)`（这确实需要人看一眼）。
- `run_verify_batch` 已有 `VerifyBatchError::Busy → yield` 的先例（scheduler.rs:152-157），沿用同一口径。

### 4.6 启动不再被写命令拖累

`src/app/useSettingsController.ts:90-115` 改为：

1. `await Promise.all([getSettings(), getRepositoryStatus()])` 后**立即**
   `setSnapshot / setDraft / setRepository`；
2. `notify` 只反映 snapshot.warning 与 repository.error；
3. 启动重放改成独立异步步骤 `void replayCleanupQueue()`：
   - 先 `listPendingFileCleanup()`（只读命令，不取 owner lock）；
   - 队列为空 → 直接返回（正常情况下启动就是这条路径）；
   - 非空 → `retryFileCleanup([])`，`repository_busy` 只 `diagnosticsLog("info", ...)`，
     不 notify、不打断界面；失败条目会在下一次启动或手动重试时继续。
4. 保持 `settingsBusy` 只覆盖两个读命令，不再被写命令拖住。

理由：启动重放是 best-effort 的维护动作，不该决定「仓库能不能显示」。

### 4.7 退出结算

- 后端：`finalize_pin_board` 走新的 shutdown 模式（Shutdown 优先级 + 10s 上限），
  入口在 `coordinator.rs` 新增 `run_repository_write_shutdown`，由素材板命令的调用点显式选择。
- 前端：`renderer.ts:1318` 的 catch 里对 `repository_busy` 重试一次（间隔 300 ms），仍失败才
  记 `warn`；`AppRuntimeProvider` 的 15s 兜底与「无论如何都回 confirm」语义不变。

---

## 5. 改动清单

| 文件 | 位置 | 改动 |
| --- | --- | --- |
| `src-tauri/src/repository_lock.rs` | :108-200 之后 | 新增 `WritePriority`、`Gate`、`acquire_write_gate`、`acquire_queued`；`acquire` 不变 |
| `src-tauri/src/app/command_runtime/coordinator.rs` | :228 / :249 / :291 / :311 / :337 / :381 | 改用 `acquire_queued`；新增等待常量；`run_foreground` 把「登记前台等待 + 让位」提到取锁之前 |
| 同上 | :305-320 | 新增 `run_repository_write_shutdown` |
| 同上 | :527-532 | 同进程超时文案与 `same_process` 诊断字段 |
| `src-tauri/src/backup/runtime.rs` | :296-330 | 拆开 `begin_foreground_wait` / `cancel_background` 与 `run_exclusive_typed_inner`，避免重复登记（协调器已提前登记时传入 `None`） |
| `src-tauri/src/backup/scheduler.rs` | :73 / :169 / :437 / :779 | `OwnerAttempt::Skipped`，后台只 try |
| `src/app/useSettingsController.ts` | :90-115 | 先回填状态，再 best-effort 重放；队列为空不重放 |
| `src/modules/pin-board/renderer.ts` | :1310-1330 | busy 时重试一次 |
| `src/app/command_runtime` 前端类型（如涉及） | — | 错误 code 不变，通常无需改 |

不改：`headless.rs:762`、`repository_lock::probe`、`is_runtime_artifact`、SQLite 参数、
`.lilith-write-lock.sqlite3` 的创建时机。

---

## 6. 测试计划

### 6.1 必须改写的现有断言

| 测试 | 位置 | 现状 | 改法 |
| --- | --- | --- | --- |
| `a_second_process_cannot_write_the_same_repository` | `coordinator.rs:806` | 两个 coordinator（同进程、不同 AppState）直接期望 `repository_busy` | 改名 `a_second_writer_queues_behind_the_first`：第二个写者在 500 ms 内成功，且第一个闭包未退出前第二个闭包未进入（mpsc 断言）。真正的跨进程拒绝交给 I1/I2 与 `repository_lock` 单测 |
| `a_second_writer_is_refused_and_release_restores_acquisition` | `repository_lock.rs:300` | 直接 `acquire` 两次 | **保持不变**（`acquire` 语义未变），另加 `acquire_queued` 的排队用例 |
| `repository_write_does_not_take_the_shared_runtime_lock` | `coordinator.rs:864` | 依赖立即失败/成功 | 复核是否受影响；若依赖拒绝语义，改为等待语义 |
| `failed_operations_carry_code_and_kind` | `coordinator.rs:754` | 只测错误映射 | 不改 |
| `stress_concurrency` I1/I2 | `tests/stress_concurrency.rs` | 真子进程互斥 | 预期仍绿；若失败说明闸门键或顺序有误 |
| `stress_concurrency` J5 | `tests/stress_concurrency.rs:422` | 允许 `repository_busy` 并重试一次 | 保留重试分支（全库校验的 `probe` 仍可能撞上写者），但把「同进程 compact 被拒」视为异常并计数 |

### 6.2 新增单测

1. **FIFO**：A 持有 → B、C 依次等待 → 释放后按登记顺序进入。
2. **优先级插队**：Background 先等（实际应为 try）；改成 Interactive 先等、Foreground 后等 →
   Foreground 先进入。
3. **超时**：holder 不释放，Interactive 用测试注入的 50 ms deadline → `TimedOut`，且未进入闭包。
4. **取消**：等待中 `CancelToken::cancel()` → `Cancelled`，等待者已出队（后续无幽灵唤醒）。
5. **跨进程仍拒绝**：线程 A 用 `acquire` 持锁，线程 B `acquire_queued(Interactive, Until(+2s))`
   → 立即 `Busy`（不等 2s）。
6. **同进程串行**：两个 `OperationCoordinator`（同仓库不同 AppState）交替写，断言事件顺序。
7. **键归一化**：`root` 与大小写变体映射同一 gate（Windows 上验证）。
8. **释放顺序**：guard drop 后 gate 可用、`.lilith-write-owner.json` 已删除。
9. **非重入**：同线程嵌套 acquire 触发 `debug_assert`（`#[should_panic]`，仅 debug）。
10. **调度器跳过**：gate 被占时本轮 `Skipped`，`wait_scheduler` 收到 `FOREGROUND_YIELD_BACKOFF`，
    分支 `last_error` 未写入。

### 6.3 调用点审计（实施前做一次）

- `grep -rn "run_write(\|run_repository_write(\|run_foreground(" src-tauri/src` 逐个看闭包体，
  确认内部不再嵌套调用另一个 `run_*`（有则改为直接调 use-case）。
- 确认 `with_ready_repository_typed` 的 `repository_operation` mutex 只在 gate 之内获取，
  全仓只有这一条获取顺序。

---

## 7. 文档与 CHANGELOG

| 文件 | 位置 | 改动 |
| --- | --- | --- |
| `docs/architecture/overview.md` | :110-121 | 锁序段落补「进程内先排队（Shutdown > Foreground > Interactive > Background，带等待上限），跨进程仍零等待拒绝」 |
| `docs/modules/library.md` | :40 | 「仓库命令分两级进入」改写为三级 + 等待上限 + 超时文案 |
| `docs/modules/history-and-backup.md` | :199 | 边界表 I1/I2 行补「同进程排队、跨进程拒绝」；调度段落（:96）补「后台遇进程内写者跳过，不计失败」 |
| `docs/planning/todo.md` | :77 | 完成后按归档规则搬到 `archive/handoff-log-2026-10-18.md`（或新日志）并删除该行 |
| `CHANGELOG.md` | Unreleased / Changed | 一段说明同进程排队、优先级、启动与退出结算修复 |
| `docs/user/stress-test-report.md` | 保证表 | 仅当 J5 断言口径变化时同步 |

---

## 8. 实施顺序（每步独立提交）

1. **闸门本体**：`WritePriority`、`Gate`、`acquire_write_gate`、`acquire_queued` + 6.2 的 1–5、7、8、9。
   不改任何调用方，纯增量。
2. **协调器接入**：5 个取锁点改用 `acquire_queued`；`run_foreground` 时序调整；
   新增 shutdown 入口；改写 §6.1 的两个测试；补 6、10。
3. **调度器**：`OwnerAttempt::Skipped` + 后台 try。
4. **前端**：启动顺序（§4.6）+ 退出结算重试（§4.7）+ 前端测试。
5. **文档 + CHANGELOG + todo 归档**。

第 4 步可以并入第 3 步，但建议单独提交，便于归因。

---

## 9. 验证

### 每次改动（轻量）

- `cargo fmt --check`、`cargo check`、`cargo test --lib`、`git diff --check`
- 改前端时：`npm test`
- 涉及调度器时：`cargo test --lib backup::scheduler`

### 需要用户明确发起

- `cargo test --test stress_concurrency`（I/J 组）。按 AGENTS/`docs/guides/validation.md`，
  压力测试只随发布或由用户显式发起，不由代理自动跑。

### 人工验收（必须）

1. **复现原始场景**：分支间隔设为 1 分钟 → 关闭应用 → 冷启动 5 次。
   期望：0 次「另一个进程写入」提示；仓库立即显示可用；日志里出现 `domain=library` 读树。
2. **备份中保存**：自动备份运行期间改设置/切画板，期望短暂等待后成功，不弹错误。
3. **备份中退出**：自动备份运行期间关窗，期望 `shutdown settlement finished: outcome=ok`。
4. **真跨进程**：headless 进程持锁时对 GUI 发写命令，期望仍看到
   「仓库正在被另一个进程写入；不支持同一仓库的多进程写入」。

### 量化判定

- 日志中 `[owner] repository write lock busy` 且 `owner.pid == 当前进程 pid` 的条数为 **0**；
- 启动到 `Lilith Artworks started` 之间 `repository_busy` 事件数为 **0**；
- 前台等待超过 60s 的 `[slow] repository mutation lock waited` 条数为 **0**。

---

## 10. 风险与回退

| 风险 | 说明 | 缓解 |
| --- | --- | --- |
| 前台变卡 | 排队后「点一下等几秒」，最坏 40s | 先 `cancel_background()`；`FOREGROUND_WRITE_WAIT=60s` 可下调到 15s；等待期间已有 `[slow]` 日志 |
| 闸门键不一致 | 大小写/短路径产生两个 gate | `canonicalize` + 用例 7；切换仓库时清理空 gate |
| 非重入死锁 | 同线程嵌套取 gate 会等自己 | debug 断言 + §6.3 审计；生产依赖审计结果 |
| 后台饥饿 | 用户连续写时自动备份一直跳过 | 后台本来就「让位优先」，跳过不写失败；加 `wait_scheduler` 短退避，不改变最终会被调度 |
| 误把 headless 改成排队 | `stress_crash` 强杀会挂起 | headless 保持 `acquire`，只在注释说明 |
| 事件/进度语义变化 | 等待期间 UI 显示「准备中」 | 不新增事件；沿用现有 `operationStarted` 与运行状态 |

回退：每一步独立提交；回退第 2 步即回到现状（闸门留着但无人使用，行为与现在一致）。

---

## 11. 已知边界与后续

- 两个进程**同时**启动仍可能各写一次（后到者被拒）——这是正确行为，不是缺陷。
- 进程被调试器挂起时 gate 会长期持有，等待者只能等到 deadline。
- 1 分钟自动备份间隔是放大器。本轮不改默认值；是否把最小间隔提到 5 分钟（或在启动后
  延迟首轮调度 5–10 秒）属另一个决策，建议在本次修复落地后单独评估。
- `temp/authenticity-preview-*` 会话目录同样会累积（实测 4 个目录约 45 MB，自 10-08 起
  未清理），与本次锁问题无关，另开工单（todo 已有同类条目）。

---

## 12. 关键位置速查

| 关注点 | 位置 |
| --- | --- |
| 跨进程锁 acquire / busy | `repository_lock.rs:117` / `:126-145` / `:36` |
| 零等待配置 | `repository_lock.rs:282-293` |
| 锁序（owner → runtime → lease → operation） | `coordinator.rs:226-238`、`app/settings.rs:366-392` |
| 前台让位时序 | `coordinator.rs:249`、`backup/runtime.rs:296-305` |
| 调度器三处取锁 | `backup/scheduler.rs:73`、`:169`、`:437`、`:779` |
| 调度器让位分支 | `backup/runtime.rs:149-175`、`scheduler.rs:64-70`、`:152-157` |
| 启动顺序 | `lib.rs:357-359`、`useSettingsController.ts:90-115` |
| 退出结算 | `AppRuntimeProvider.tsx:61-102`、`renderer.ts:1308-1329`、`lib.rs:127-140` |
| 现状回归记录 | `docs/planning/todo.md`（该项已随本次修复删除，见文末「实施记录」） |

---

## 实施记录（2026-10-19）

阶段 1–4 与文档同批落地。改动文件：

- `src-tauri/src/repository_lock.rs`：新增 `WritePriority`、`WaitPolicy`、`GateError`、
  `Gate`/`GateOutcome`、`acquire_write_gate`、`WriteGateGuard` 与
  `RepositoryOwnerLock::acquire_queued`；原 `acquire`（零等待、不排队）保持不变，供 `probe`
  与 headless 使用。
- `src-tauri/src/app/command_runtime/coordinator.rs`：五个取锁点改用 `acquire_queued`；新增
  等待上限常量；`run_foreground` 改为「先登记前台等待并请求后台让位，再取闸门」；新增
  `run_repository_write_shutdown`；交互写改两段式（先 `try`，真竞争才 `cancel_background`
  并排队）；同进程超时文案与 `same_process` 诊断字段。
- `src-tauri/src/backup/runtime.rs`：拆出 `run_foreground_after_wait`（供协调器预登记后复用）；
  新增 `shutting_down()` 作为闸门排队的取消来源。
- `src-tauri/src/backup/scheduler.rs`：`OwnerAttempt::Skipped`，后台取锁只 `try`。
- `src-tauri/src/app/usecases/pin_board.rs`：`finalize_board` 改走 shutdown 入口。
- `src/app/useSettingsController.ts`：先回填设置与仓库状态，再 best-effort 重放清理队列
  （队列为空不重放）。
- `src/modules/pin-board/renderer.ts`：结算遇到 `repository_busy` 重试一次。

审查阶段补入的两处修正（超出原 §4/§5，属于实施中发现的问题）：

1. `Gate::acquire` 在进入时复查 `retired`。原实现只在排队等待的循环里复查，`Try` 与后台路径
   不复查，于是存在「`gate_entry` 已返回 `Arc`、尚未进入 `acquire`」的窗口：持锁者恰好在此刻
   回收空闸门，写者就会抢到一把已移出注册表的「僵尸闸门」，与随后新建的闸门同时被持有，破坏
   进程内互斥并再次产生同进程 `repository_busy`。现在「判定空闲 → 标记 `retired` → 移出注册表」
   与「查 `retired` → 置 `holding`」都在同一段持有闸门锁的临界区里完成，两侧天然互斥。
2. `OwnerAttempt::Owned` 包一层 `Box`（消除本改动新引入的 `clippy::large_enum_variant` 提示）；
   `WriteGateGuard` 与 `RepositoryOwnerLock` 补 `#[must_use]`（持锁证明被立即丢弃等于没有互斥）。

静态验证（2026-10-19）：`cargo fmt --check`、`cargo check --all-targets`（无警告）、
`cargo test --lib`（265 passed / 0 failed / 1 ignored）、`npm test`（229 passed）。

**待维护者人工验收**（§9 要求，本轮未进行）：复现原始场景（分支间隔 1 分钟、冷启动 5 次）、
备份中保存/切画板、备份中退出得到 `shutdown settlement finished: outcome=ok`、真跨进程仍被拒；
以及量化判定（同进程 busy 条数为 0、启动到 `Lilith Artworks started` 之间 `repository_busy`
为 0、前台等待超过 60 s 的 `[slow]` 条数为 0）。`stress_concurrency` I/J 组按释放前口径由维护者
显式发起，本轮未运行。

有效契约已并入 `docs/architecture/overview.md`、`docs/modules/library.md` 与
`docs/modules/history-and-backup.md`；行为变化见 `CHANGELOG.md` 的 `## Unreleased` → `Fixed`。
