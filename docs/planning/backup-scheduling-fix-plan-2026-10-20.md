# 自动备份调度与快速检查问题修复计划（2026-10-20）

状态：**阶段 1–3 已实施（2026-10-20），自动验证通过；GUI 与一天日志观察待人工**。阶段 2 的
「观察 ≥1 天日志」门槛由维护者明确跳过，先行实施。本文件只保留马上要执行的内容与后续阶段的
进入门槛；长期搁置项（schema 列、历史页 busy 解耦、重任务并行、per-artwork 拆库）已移入
`todo.md`，本文不再重复。

**硬约束：本计划不改数据库 schema。** 需要新列/新表的项一律不进本计划，见 `todo.md`
「备份检查元数据持久化」。

## 1. 一句话结论

快速检查此前从未生效（P1），后台一直在跑「内容没变的全量检查」，单次实测 100–161 秒；
修好 P1 就能消掉绝大部分负载（阶段 1 已落地，等日志确认），剩下的只是调度节奏与全量路径
里的重复做功。

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
| P8 | 重任务内部全单线程 | 见 §6 实测 | 阶段 4，先打点再定 |

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
| 4 | 全量路径去掉重复做功：打点 → 摘要前置 / 缓冲扫描 / 压缩档位 | 阶段 3 后，且拿到 release 构建下真实大文件提交的分段耗时 | `chunk_file/`、`worker.rs` |

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

## 6. 阶段 4 的前置判定：要不要把 zstd / 哈希 / 分块改成多线程

先说事实（本机 i5-13490F，优化构建，一次性微基准，脚本不进版本库）：

| 步骤 | 实测 | 1.37 GB 折算 | 可并行性 |
| --- | --- | --- | --- |
| CDC 逐字节滚动哈希（复刻 `stream_cdc` 热循环） | 1.5–1.6 GiB/s | ≈0.9 s | 差：边界必须与串行算法逐字节一致 |
| SHA-256（`sha2` 0.10.9，走 SHA-NI） | 1.9 GiB/s | ≈0.7 s / 遍 | 整文件摘要串行；块级独立 |
| 结构单元逐条读取（复刻 `push_exta_units`：每条记录 seek+read 8 B、seek+read 34 B） | 3.24 µs/条 | 每 10 万条 ≈0.32 s | 可，但顺序缓冲读同样字节快 7 倍（6 GiB/s） |
| zstd L6 压缩（半可压数据） | 63 MiB/s | **≈21 s** | 单流内部可开线程（`zstdmt`） |
| zstd L1 压缩（同上数据） | 272 MiB/s | ≈4.8 s | — |

**三个修正性结论：**

1. **现在并没有「每块压缩」。** snapshot 载荷是 `CODEC_NONE` 明文写入；zstd 只把 delta
   整个文件包成一条流（`snapshot.rs:482`，level 6）。所以「并行 zstd」能碰到的只有 delta
   写入这一条路径，而 delta 本身已经只装变化的块。
2. **CDC 与哈希都不是瓶颈。** 1.5 GiB/s 与 1.9 GiB/s 折算到 1.37 GB 都在 1 秒量级，无论
   怎么并行都压不出 160 秒里的百倍差距。P8 原来「CPU 单核被打满」的推断不成立——160 秒
   更可能来自构建档位、temp snapshot 的 `fsync`、或 `validate_head_snapshot` 的第二遍
   读，必须靠打点定位。
3. **真要省时间，压缩档位比线程便宜得多。** L6 → L3 快 3.6 倍，体积只多 6%（实测
   3.39x → 3.19x）。这是一行常量的改动，zstd 帧与解码完全不受影响。

**复杂度评估（如果仍要做）：**

| 方案 | 改动量 | 风险 | 收益 |
| --- | --- | --- | --- |
| zstd 档位 6 → 3 | 1 行 | 极低：帧格式、解码路径都不变，只影响体积 | delta 写入快 3.6 倍（若打点显示它是大头） |
| zstd 开 `zstdmt` | 1 个 feature + 1 次 `.multithread(0)` | 低：输出仍是标准帧，delta 无摘要、无跨版本比对，字节不确定性无影响 | 再用上多核；内存按窗口 × worker 增长 |
| 块级 SHA-256 流水线 | 中：把 `SnapshotWriter::emit` 拆成「读→并行哈希→按序写」有界管道，约 200–400 行 | 中：整文件摘要仍须串行（见下），去重表与输出顺序要重排，取消检查点语义要重新验证 | ≈0.7 s/GB → 打折后收益最小 |
| CDC 并行 | 大 | 高：边界一旦与串行不同，去重命中和存量 snapshot 全部失效，chunker id 写在文件头，等于新格式 | 不做 |

**为什么「整文件摘要」这一遍没法并行：** `file_digest` 是对全部块按顺序拼起来的字节做一次
SHA-256（`SnapshotWriter::emit` 逐块 `file_hasher.update`），它是数据库里 `sha256` 列的
身份值，与 HEAD 比对、还原校验都用它。SHA-256 不像 blake3 那样可以分块组合，并行化要么
保留一遍串行哈希（只并行块级摘要，收益减半），要么改成树哈希（格式版本升级，旧节点无法
重算）。这是格式语义，不是线程问题。

**阶段 4 的执行顺序（先做前三件，全都不涉及线程）：**

1. **摘要前置**：`run_backup` 先用一次流式 SHA-256 与 `head.sha256` 比对（快照的
   `file_digest` 恰等于源文件字节的 SHA-256——结构单元按 `validate_coverage` 恰好铺满文件
   且顺序一致，可直接比）。相同则 `mark_unchanged` + 记录
   基线，不建 snapshot、不 `fsync`、不再读一遍已存快照——不变路径从「读 3 遍 + 写 1 遍」
   降为「读 1 遍」。代价：现路径的不变检查兼作 HEAD 快照的完整性校验，改成前置后这次
   校验消失，交给空闲完整性检查（阶段 9 链路）或在内存里按计数每 N 次补一次。
2. **去掉重复校验**：`create_snapshot` 已经算出 `file_digest` 并与 HEAD 比对成功，随后
   `validate_head_snapshot` 又把已存快照整份读回来重算一遍。打点确认占比后，改成按计数
   抽检即可。
3. **结构扫描缓冲化**：`push_exta_units` 每条 tile 记录 4 次 syscall，改成带缓冲的顺序读
   （实测同样字节 0.09 s vs 0.65 s）。
4. 打点数据若显示压缩确实占大头，才做 zstd 档位（1 行）；仍有需要再开 `zstdmt`。**不做
   CDC 并行，也不批量为块级哈希搭流水线。**

打点本身很小：在 `ChunkFile::create`（规划 / 读块 / 哈希 / 写盘）、`validate_head_snapshot`、
delta 编码各记一段 `elapsed_ms`，用 release 构建复测 1.37 GB。

## 7. 明确不做（已移入 `todo.md`）

- `branches.last_check_elapsed_ms` / `last_check_mode` / 快速命中计数三列 → 见 todo「备份
  检查元数据持久化」。
- 历史页 busy 一刀切禁用（P7）→ 见 todo 同名条目。
- 重任务内部并行（P8）与 per-artwork 独立数据库 → 见 todo 同名条目。
