# 自动备份调度与快速检查问题修复计划（2026-10-20）

状态：**阶段 1–3 已实施并通过自动验证；阶段 4–5 已按 release 实测重新定范围，尚未实施**。
阶段 2 的「观察 ≥1 天日志」门槛由维护者明确跳过；GUI 与日志观察仍在 `todo.md` 待人工。
长期搁置项（schema 列、历史页 busy 解耦、per-artwork 拆库）仍在 `todo.md`，本文不重复。

**为什么重写阶段 4（2026-10-20）：** 原计划依据的「单次全量检查 100–161 秒」是 debug 未优化
档的观测，已被维护者否定，不能作为依据。release 实测 1.378 GB 提交 5.9 秒、恢复 4.2 秒，
量级完全不同，原来的排序（摘要前置 → 去重复校验 → 缓冲读 → 压缩档位）随之作废。实测见 §6。

**硬约束：本计划不改数据库 schema。** 需要新列/新表的项一律不进本计划，见 `todo.md`
「备份检查元数据持久化」。

## 1. 一句话结论

快速检查此前从未生效（P1），后台一直在跑「内容没变的全量检查」；阶段 1 修好 P1，让未变的
检查从秒级降到毫秒级，阶段 2–3 处理调度侧的连续满负荷。剩下的全量路径在 release 下是
1.378 GB / 5.9 秒（提交）与 4.2 秒（恢复），其中七成与五成集中在一条串行的「读→切块→
两遍哈希→写」，没有单点热点：阶段 4 只做两件便宜的减法（各值 1–2 秒），真正的加速靠阶段 5
把这条流水改成有界管道。

## 2. 问题与证据

证据均为 2026-10-20 实测。

| # | 问题 | 证据 | 本计划处理 |
| --- | --- | --- | --- |
| P1 | 指纹要求 mtime 亚秒精度，`.clip` 全部整秒 → 基线永远为 NULL | `worker.rs:77`；三个工作文件（185 MB / 1.37 GB / 124 MB）亚秒位全为 0；日志 58 次全量 vs 6 次快速 | 阶段 1（A1）已修 |
| P2 | 调度器无代价感知：只看时间戳，不看上次耗时 | `scheduler.rs` 的 `due_at_ms`；1 分钟间隔 × 161 秒耗时 → 连续满负荷 | 阶段 2（A2） |
| P3 | 退化不可见：静默退回全量，无日志 | `quick_check_unchanged` 返回 `Ok(false)` 即结束 | 阶段 1（A3）已修 |
| P4 | 前台结束后无冷却，后台立刻插进操作间隙 | 前台清零即抢锁开跑，下一操作又取消它 | 阶段 2（A4） |
| P5 | 让位是 200 ms 轮询而非事件驱动 | 14 秒内 70 次 `scheduler yields the candidate scan` | 阶段 3（A5） |
| P6 | 后台完成推进全局 revision，历史页整图重读 | `runtime.rs:233`；`useHistoryController.ts` 收到即 `reload()` | 阶段 3（A6） |
| P7 | 历史页 `disabled={busy \|\| runtime.busy}` 一刀切 | `HistoryModule.tsx:447+` | 移出，见 `todo.md` |
| P8 | 重任务内部全单线程 | 见 §6.1：提交 71%、恢复 53% 花在单条串行流水上 | 阶段 5（流水线并行） |

补充事实：0.3 的「每个 artwork 单独读进程」**未以进程形式实现**（代码中无子进程架构）。
`9314344`（2026-10-19）实际落地的是进程内写门（Shutdown > Foreground > Interactive >
Background 排队）、读租约（`with_repository_read` 只取 RwLock 读端）与 SQLite WAL。读隔离
粒度是仓库级而非 artwork 级，但本次堵车根源不在读路径。

## 3. 阶段安排

| 阶段 | 内容 | 进入门槛 | 涉及 |
| --- | --- | --- | --- |
| **1** | A1 指纹门槛 + A3 退化 warn 日志 | **已实施（2026-10-20）** | `backup/worker.rs` |
| **2** | A4 前台静默 debounce + A2 调度器代价感知 | **已实施（2026-10-20）**；维护者明确跳过一天观察门槛 | `scheduler.rs`、`runtime.rs`、AppState |
| **3** | A6 完成事件区分「结构变化」 + A5 让位改事件驱动 | **已实施（2026-10-20）** | `scheduler.rs`、`repository_lock.rs`、`runtime.rs`、前端 controller |
| 4 | 两件便宜的减法：删掉提交路径的 HEAD 快照整份复检、结构单元改缓冲顺序读 | **分段打点已落地（`c4f0192`），release 实测已到手** | `worker.rs`、`format/container.rs`、`chunk_file/` |
| 5 | 提交与恢复的串行流水改有界管道（多线程块哈希 + 保序写） | 阶段 4 的缓冲读先落地：它改变瓶颈构成，收益要重算 | `chunk_file/snapshot.rs`、`worker.rs`、`restore.rs` |

阶段 2 起每一项都只改一处行为，可独立回滚；不要求连着做完。

## 4. 阶段 1（已实施）

### A1. 放宽指纹的 mtime 精度门槛（修 P1）

- 现状：`subsec_nanos() == 0` 直接返回 `None` → 基线永久写为 NULL → 每次退回全量。
- 方案：mtime 按系统实际精度参与指纹（有亚秒位就用，只有整秒也接受）+ 大小 + 首尾
  64 KiB 采样，基线继续复用现有 `last_source_modified_ms` 列，**不加列**。
  `SOURCE_FINGERPRINT_VERSION` 递增到 2，旧基线取值域与新算法不同，自然重写一次。
- 盲区：理论上漏检「同一秒内、同尺寸、且与基线 mtime 完全相同」的中间改写。实际保存必然
  更新 mtime，基线只在成功检查后写入，三个条件同时成立的窗口极窄，接受该风险。原设想的
  「每 N 次快速命中强制全量」需要计数列，已移入 `todo.md`，不实施。

### A3. 退化可见性（修 P3，缩水版）

- 快速检查退回全量时按分支记一条日志，含 `branch_id` 与原因。
- 同一分支原因不变时去重，不逐次刷；快速检查恢复（命中或已重写基线）后清空记录，下次
  退化重新可见。
- 原因分两档，只有第一档是 `warn`。后两档是「文件真的可能变了」，全量检查本来就要跑，
  用 `warn` 会把每次正常保存都刷成警告；它们仍记一条 `info`，用于回答「这次为什么没走
  快速路径」。

| 原因 | 级别 | 含义 |
| --- | --- | --- |
| `no baseline` | warn | 还没有提交，或基线列为空 |
| `source metadata unavailable` | warn | 文件被删除或无权访问 |
| `source is not a regular file` | warn | 路径不是普通文件 |
| `sample read failed` | warn | 首尾采样读不到（比较期间被截断） |
| `size mismatch` | info | 大小变了，内容可能变化 |
| `fingerprint mismatch` | info | mtime 或采样变了，内容可能变化 |

- 不做：分支状态行显示「检查方式/耗时」——需要持久化列，见 `todo.md`。任务进行中显示
  任务类型现状已支持。

### 实施记录（2026-10-20）

- `worker.rs` 内部拆成 `source_fingerprint`（去门槛）/ `quick_check`（判定本体，返回
  `QuickCheck`）/ `quick_check_unchanged`（对外仍是 `Result<bool, String>`，只多出记账与
  日志），调度器与其余调用方零改动。
- 去重状态是模块内 `OnceLock<Mutex<HashMap<String, QuickCheckFallback>>>`，键为
  `branch_id`，命中即删条目。没有引入 schema、没有跨模块依赖，`scheduler.rs` 未改。
- 新增两个用例：`quick_check_accepts_second_precision_mtime_baseline`（把源文件 mtime 设成
  整秒，断言基线指纹非空且快速检查命中；已验证在旧门槛下该用例失败）、
  `quick_check_reports_why_it_falls_back`（逐个断言六种原因里的四种可达）。

### 验收口径

按 `docs/guides/validation.md`：`cargo check --lib`、`cargo test --lib`、`cargo fmt
--check`、`git diff --check`；GUI 与大文件场景由维护者安排。

2026-10-20 结果：`cargo check --lib` 通过；`cargo test --lib` 308 passed / 1 ignored / 0
failed；`cargo fmt --check` 与 `git diff --check` 干净。GUI 未验证（待人工）。

阶段 1 的观察指标（挂一天日志后看）：

- `backup quick-unchanged` 的条数应远超 `backup unchanged`；后者只应出现在真实变更或
  首次检查之后。
- `elapsed_ms` 的分布应回到毫秒级；出现 100 秒以上即为异常，需按 A3 的新日志定位原因。

## 5. 阶段 2–3 的实施口径（2026-10-20 已实施）

| 项 | 一句话方案 | 风险/代价 |
| --- | --- | --- |
| A4 前台静默 debounce | AppState 记录 `last_foreground_activity_ms`（前台命令开始与结束都刷新）；调度器开跑前要求静默 10–15 秒，不满足则带唤醒挂起，不空转 | 完全停手后首个后台任务最多晚 ~15 秒；无 schema |
| A2 调度器代价感知 | 内存 Map `branch_id → last_check_elapsed_ms`；`effective = max(interval, 2 × last_elapsed)`；只影响调度节奏，不改用户设置的间隔 | 重启丢学习结果，第一轮按名义间隔跑；无 schema |
| A6 完成事件收敛 | 后台检查类任务完成时声明「本次无结构变化」，前端只刷状态行不重读整图；事件带 `artworkId` 是最小改动，完成路径加结构化标记更治本 | 需同时覆盖前端 controller 与事件定义 |
| A5 让位改事件驱动 | `acquire_owner` 返回 `Skipped` 时挂起等「写门释放」信号，保留 ~5 秒兜底超时防信号丢失 | 改动集中在 `scheduler.rs` 与锁释放通知；行为对外一致 |

### 实施记录（2026-10-20，阶段 2–3）

- A4：`AppState.last_user_activity_ms` 由 `OperationCoordinator` 的所有用户长操作入口
  （前台长命令、普通写、素材板写、仓库切换，以及内容级只读长任务）在开始/结束时刷新；
  调度器在候选扫描前要求静默 10 秒并按剩余时长挂起，取锁后在 `run_scheduled_backup`
  再复查一次，命中即 `UserActive` 让位，不计失败、不推进检查时间。用户活动不逐次唤醒
  调度器，避免频繁保存反复触发候选扫描。
- A2：`BackupState` 内存 Map 记录分支上次成功检查耗时；`due_at_ms` 取
  `max(设置间隔, 2 × 耗时)`，候选选择与锁内复查共用。失败、取消、让位不记录；重启丢学习结果。
- A5：`RepositoryOwnerLock` 的进程内闸门新增按仓库的释放代次与等待者；调度器在
  `OwnerAttempt::Skipped` 时等待该信号，5 秒兜底；退出时 `wake_write_gate_waiters` 主动唤醒。
  跨进程占用没有本进程信号可等，因此空闲完整性检查区分 `InProcessWriter`（等闸门）与
  `CrossProcessBusy`（退回 200 ms 短退避），不空转抢锁。
- A6：`operationFinished` 追加可选 `artworkId` / `structureChanged`；调度器为自动备份与空闲
  校验发后台完成事件（`ScheduledBranch` 查询附带 `artwork_id`，无 schema 变更）。历史页对无结构
  变化的当前作品完成只合并 `branches`，保留 `nodes` 引用，缩略图缓存不再随之作废；其它作品
  的完成直接忽略，事件缺失仍完整刷新。
- 新增用例：静默窗、耗时放宽间隔、`UserActive` 让位、写门释放唤醒/超时/退出唤醒、后台完成事件
  字段，以及前端「只合并分支 / 结构变化完整刷新 / 忽略其它作品」三条路径。
- 2026-10-20 结果：`cargo check --lib` 通过；`cargo test --lib` 315 passed / 1 ignored / 0
  failed；`cargo fmt --check` 干净；`npx tsc --noEmit` 通过；`npm test` 251 passed。GUI 未验证
  （待人工）。

## 6. 实测基线（release，2026-10-20）

### 6.1 一次真实提交与一次恢复

环境：release 构建；工作文件 1.378 GB（`ContainerAware`，114,616 块，均块 11.7 KiB，快照
1.36 GB）；恢复同作品一个 1.375 GB 节点（`chain_steps=2`）。分段耗时由 `c4f0192` 加在既有
结果日志上，一次操作仍只有一行。

| 提交阶段 | 耗时 | 占比 | 折算吞吐 |
| --- | --- | --- | --- |
| `snapshot_ms` 读源文件 + 切块 + 两遍哈希 + 写快照 | 4,179 ms | 71% | 330 MB/s |
| `snapshot_sync_ms` fsync | 808 ms | 14% | 1.68 GB/s |
| `delta_ms` 反向增量 | 418 ms | 7% | 256 MB/s（进 107 MB / 出 82 MB） |
| `graph_ms` 落库 + 清理 | 116 ms | 2% | — |
| `preview_ms` 缩略图 | 37 ms | 0.6% | — |
| 其余（准备、发布等） | 约 250 ms | 4% | — |
| **合计** | **5,853 ms** | | |

| 恢复阶段 | 耗时 | 占比 | 折算吞吐 |
| --- | --- | --- | --- |
| `export_ms` 读块 + 逐块校验 + 写盘 | 2,200 ms | 53% | 625 MB/s |
| `resolve_ms` 解压 delta 链 + 重排块表 | 1,167 ms | 28% | — |
| `sync_ms` fsync | 791 ms | 19% | 1.74 GB/s |
| **合计** | **4,159 ms** | | |

另外顺带测到三次「未变更」全量检查：1.375 GB 总 6,469 ms（snapshot 4,412 + sync 33 +
`validate_ms` **2,020**）、186 MB 总 933 ms（563 + 104 + 259）、50 MB 总 633 ms
（279 + 29 + 319）；两次退回全量的原因是启动后首次检查的 `no baseline`，之后日志里出现了
`quick-unchanged`，说明阶段 1 的快速检查确实生效。

**三条可直接拿去定方案的推论：**

1. **`snapshot_ms` 里写盘不占主导。** 提交写 1.36 GB，`snapshot_sync_ms` 是 808 ms；同一条
   分支「未变更」那次只花 33 ms 同步（它的 HEAD 快照本身只有 0.08 GB，v2 去重后），但
   `snapshot_ms` 反而更高（4,412 ms）——两边写出量差十几倍、耗时却相同。据此推断：时间花在
   「读 1.37 GB + 逐块切分 + 两遍 SHA-256」上。330 MB/s 远低于单步基准合起来的约 2.3 秒，
   差额约 2 秒应来自 11.4 万块 × 每块一次 seek+read+新分配缓冲（推断，待 A8 验证）。
2. **压缩不是瓶颈。** delta 只占总时间 7%，实测 256 MB/s 也不是 63 MB/s 量级；这批 `.clip`
   载荷本身实测压缩率 **1.01x**（zstd L6 跑到 1.6 GiB/s，8 线程 2.4 GiB/s，因为压不动）。
   开 `zstdmt` 最多省不到 0.4 秒。
3. **缩略图不是瓶颈。** 37 ms，`preview::extract` 直取容器内嵌预览，不做整图解码。

### 6.2 单步微基准（同一台机器，一次性，脚本不进版本库）

| 步骤 | 实测 | 1.37 GB 折算 | 可并行性 |
| --- | --- | --- | --- |
| CDC 逐字节滚动哈希（复刻 `stream_cdc` 热循环） | 1.5–1.6 GiB/s | ≈0.9 s | 差：边界必须与串行算法逐字节一致 |
| SHA-256（`sha2` 0.10.9，走 SHA-NI） | 1.9 GiB/s | ≈0.7 s / 遍 | 整文件摘要串行；块级独立 |
| 结构单元逐条读取（复刻 `push_exta_units`：每条记录 seek+read 8 B、seek+read 34 B） | 3.24 µs/条 | 每 10 万条 ≈0.32 s | 可，但顺序缓冲读同样字节快 7 倍（6 GiB/s） |
| zstd L6（半可压数据；**不代表本仓库**） | 63 MiB/s | ≈21 s | 单流内部可开线程 |
| zstd L1（同上数据） | 272 MiB/s | ≈4.8 s | — |

两处仍然成立的结论：

- **没有「每块压缩」这件事。** snapshot 载荷是 `CODEC_NONE` 明文写入；zstd 只把 delta 整个
  文件包成一条流（`snapshot.rs:482`，level 6），而 delta 只装变化的块。
- **整文件摘要不能并行。** `file_digest` 是全部块按序拼起来后的一次 SHA-256（`emit` 逐块
  `file_hasher.update`），它是数据库 `sha256` 列的身份值，与 HEAD 比对、还原校验都用它。
  SHA-256 不能分块组合，并行化只能换树哈希——那是格式版本升级，旧节点无法重算。

### 6.3 打点字段（保留，不做临时日志）

`backup committed` / `backup unchanged` / `restored history node` 三行各带分段耗时：提交为
`prepare_ms`、`snapshot_ms`、`snapshot_sync_ms`、`preview_ms`、`delta_ms`、`delta_input_bytes`、
`delta_sync_ms`、`publish_ms`、`graph_ms`；恢复为 `resolve_ms`、`export_ms`、`sync_ms`、
`publish_ms`。都是事件驱动、一次操作一行、不新增日志行，保留用于阶段 4–5 的前后对照和以后
「提交变慢了」的现场取证（读法：`delta_input_bytes / delta_ms` 得吞吐，几十 MB/s 才是压缩
问题；1 GB/s 级说明卡在读写+哈希）。

## 7. 阶段 4：两件便宜的减法（待实施）

### A7. 删掉提交路径的 HEAD 快照整份复检

- 现状：摘要与 HEAD 相同时 `run_backup` 调用 `validate_head_snapshot`，把磁盘上那份旧快照
  整份读回、逐块验哈希再验整文件摘要。实测 1.28 GB 用 2,020 ms，占该类检查 31%。
- 方案：提交路径不再复检，`validate_head_snapshot` 与只为它服务的 `repair_head_snapshot`
  一并移除；`backup unchanged` 行的 `validate_ms` 字段随之消失。
- **代价（维护者 2026-10-20 明确接受）：** 旧快照文件损坏不再在提交路径被发现，也不再
  自动重建。完整性由空闲完整性检查承担（`verify_history_node` / `scrub_history`，逐块校验
  并写节点校验时间）；**该开关默认关闭**，关闭时这层体检实际不生效。若日后要收回，最省事的
  做法是按计数抽检（每 N 次一次）。

### A8. 结构单元改缓冲顺序读

- 现状：`create` 的结构单元循环每块 `seek + read_exact`，并新分配 `vec![0u8; len]`；
  `push_exta_units` 每条 tile 记录 4 次 syscall。
- 方案：读取侧套一层容量 ≥ `max_size` 的缓冲顺序读（结构单元本身有序且按
  `validate_coverage` 铺满文件，顺序读与逐单元 seek 等价），缓冲复用、不再逐块分配；
  `push_exta_units` 的探测读同样缓冲化。
- 预期：吃掉 §6.1 推论 1 里那约 2 秒 syscall/分配开销的大部分。单独可回滚。
- 不动：分块边界、记录格式、整文件摘要。

## 8. 阶段 5：提交与恢复的串行流水改有界管道（待实施）

维护者 2026-10-20 决定按实测收益推进（三成上下），并接受 200–400 行改动与取消语义重验。
**前提：A8 先落地并重测**——缓冲读会改变瓶颈构成，届时「并行还值多少」才是真数字。

1. **提交侧块哈希并行**：把 `ChunkFile::create` 的「读 → 哈希 → 写」拆成有界管道：读线程
   按单元切块 → N 个 worker 算块 SHA-256 → 主线程按序写记录与载荷。整文件摘要仍是串行
   `hasher.update`，按序从队列喂入，不参与并行。
2. **恢复侧块校验并行**：`copy_original` 改为多线程读块 + 校验块哈希，主线程保序写输出
   文件（输出仍是单文件顺序写）。
3. **进度语义**：`ChunkTask::report` 现在按调用顺序累加，并行后改成只上报「已完成前缀」，
   避免进度条倒退。

风险与约束：

- **取消**：管道每一段都要响应 `CancelToken`；取消后不得发布任何文件（沿用现有 temp +
  原子 rename，但要重验取消检查点语义）。
- **内存**：队列必须有界（建议 ≤ 64 MB），否则大文件吃满内存。
- **收益上限**：整文件摘要串行 + 单文件顺序写，决定了两侧都拿不到线性加速。

预期（估算，非实测）：提交 `snapshot_ms` 4.2 s → 约 2.5 s（总 5.9 s → 约 4 s）；
恢复 4.2 s → 约 3.2 s。两个操作各自的 fsync（约 0.8 秒）与线程无关，不在优化范围。

验证：`cargo test --lib` 全通过；新增「并行与串行产物逐字节一致」「取消不留下发布文件」
「进度单调不倒退」用例；release 下用同一 1.378 GB 分支复测提交与恢复，与 §6.1 逐项对比。

## 9. 明确不做

- **zstd 档位与 `zstdmt`**：载荷实测 1.01x 压缩率、压缩只占提交 7%，收益不到 0.5 秒。
- **摘要前置（原 A9）**：全量路径只在指纹失效时进入，此时内容通常真的变了，预检必然 miss，
  反而多一遍整文件哈希；唯一受益场景（指纹失效但内容没变）实测只出现在一次性的 `no baseline`。
- **预览改后台**：实测 37 ms。
- **CDC 并行**：边界一旦与串行不同，去重命中与存量快照全部失效，chunker id 写在文件头，
  等于新格式。
- **整文件摘要并行**：见 §6.2，属格式语义。
- **schema 相关三项**（检查耗时 / 检查方式 / 快速命中计数）→ 见 `todo.md`「备份检查元数据
  持久化」；历史页 busy 一刀切（P7）→ 见 todo 同名条目；per-artwork 拆库 → 见 todo。
