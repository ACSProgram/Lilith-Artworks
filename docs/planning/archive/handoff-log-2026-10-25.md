# 2026-10-25 0.3.0-beta.1 发布前全量压力测试与版本切分

被测版本按维护者口径记为 `0.3.0-beta.1`；测试运行时五处版本字段仍是 `0.3.0-alpha.6`，同日
随后切分为 `0.3.0-beta.1`（见第 9 节），repository schema v6。批次记录描述本轮实测、版本切分
与文档收尾，不改产品代码。

## 1. 环境与构建

| 项 | 值 |
| --- | --- |
| 机器 | Intel Core i5-13490F（10 核 / 16 线程）/ 32 GB / Windows 11 专业版 10.0.26300 |
| 被测二进制 | `cargo build --release --features headless` 产出的 `target/release/lilith-artworks.exe`（构建 12m01s） |
| 测试脚手架 | debug 档，`cargo test --features headless`；昂贵分组用 `--test-threads=1` 或默认并行 |
| 磁盘预算 | `LILITH_STRESS_DISK_BUDGET=40GiB`（`manual` 档必需）；`stress_large` 五档串行发起 |
| 数据落点 | `<target>/stress/report.jsonl`、`ledger.jsonl` 与 `workspaces/`，均在 `target/` 下 |

## 2. 预检

- `npm test`：56 个测试文件 / 500 项通过（27.4 s）。
- `npx tsc --noEmit`、`npm run build`、`cargo fmt --check`、`git diff --check` 均通过。
- `cargo test --lib`：434 通过、0 失败、1 忽略（4.8 s）。
- `gate_guard`：3/3 通过。
- `npm run ci:local`（审计、许可证一致性与发布元数据）与 GUI/人工验收不属于本轮压力测试
  范围，仍按发行政策由维护者单独发起。

## 3. 分组结果

| 分组 | 结果 | 墙钟 | 备注 |
| --- | --- | --- | --- |
| 便宜九组（A / B / R / M / H / G / I–K） | 47/47 通过 | 约 18 s（目标已编译） | `stress_cancel` 7、`stress_crash` 4、`stress_cleanup` 1、`stress_damage` 7、`stress_format` 4、`stress_pin_board` 5、`stress_authenticity` 5、`stress_concurrency` 9、`stress_input` 5 |
| `gate_guard` | 3/3 通过 | <0.1 s | |
| `stress_scale`（D/E/F） | 5/5 通过 | 21.0 s | |
| `stress_bench`（P1–P9） | 10/10 通过，严格门通过 | 20.6 s | `--test-threads=1`；严格门按旧基线通过，随后以本轮数值刷新 release 条目，刷新后再跑严格门仍 10/10 |
| `stress_authenticity` `extreme`（16K） | 5/5 通过 | 60.5 s | G1 峰值 2.06 GiB（上限 3.0 GiB）；G2 发布 53.8 s（渲染 1.7 / 编码 50.9 / 签名 0.18） |
| `stress_large` `default` | 10/10 通过 | 18.3 s | C9 / C10 首次实际运行 |
| `stress_large` `large` | 10/10 通过 | 60.7 s | |
| `stress_large` `heavy` | 10/10 通过 | 205.4 s | |
| `stress_large` `extreme` | 10/10 通过 | 946.7 s | |
| `stress_large` `manual` | 10/10 通过 | 3770.9 s（约 62.8 分钟） | 0.3 线首次运行；8 GiB 逻辑文件；40 GiB 预算 |
| `stress_baseline`（临时恢复的一次性对比） | 2/2 通过（迁移后重跑） | 104.0 s | 见 `0.3-baseline-2026-10-18.md` 第 7 节 |

预期内的取消、强杀与拒绝用例会以非零退出码写进 `report.jsonl`（`cancelled` 或
`expect_killed` 断言通过），不属于失败。

## 4. `stress_large` C1 实测

| 项 | `default` 64 MiB | `large` 256 MiB | `heavy` 1 GiB | `extreme` 4 GiB | `manual` 8 GiB |
| --- | --- | --- | --- | --- | --- |
| 提交次数 | 21 | 21 | 11 | 6 | 6 |
| 提交总耗时 | 13.7 s | 44.5 s | 140.6 s | 263.0 s | 401.5 s |
| 恢复最早节点 | 1.26 s | 4.84 s | 17.46 s | 57.90 s | 101.87 s |
| 恢复最新节点 | 0.29 s | 1.03 s | 5.09 s | 26.94 s | 47.44 s |
| 全库 `scrub` | 2.11 s | 7.97 s | 30.3 s | 75.8 s | 123.1 s |
| 快照 / 逻辑 | 35.4 MB / 64 MB | 130.7 MB / 256 MB | 167.5 MB / 1 GiB | 234.3 MB / 4 GiB | 327.6 MB / 8 GiB |
| 增量合计 | 7.5 MB | 27.0 MB | 9.8 MB | 4.3 MB | 3.2 MB |
| 账本实测 / 预期 | 47.21 / 47.19 MB | 178.64 / 178.62 MB | 254.14 / 254.13 MB | 278.44 / 278.44 MB | 371.72 / 371.72 MB |

`manual` 档 C1 的提交峰值内存约 661 MB、恢复峰值约 653 MB，`scrub` 峰值约 657 MB；C4 的
快照 327.6 MB、增量 3.2 MB，增量 / 快照约 0.98%。五档全部通过「恢复产物与工作文件逐字节
一致」与账本断言。

## 5. 中断与管道边界

- C7 在五档上对提交的 6 个检查点各取消一次、再强杀一次：每次都以取消退出、无残留、历史
  不推进、仓库可重开；强杀后 head 恢复与工作文件逐字节一致，重试成功。
- C9 在五档上首次实际运行：快照临时文件写到 1 MiB 后强杀，断言残留临时文件存在、正式
  `snapshots/` 与 `deltas/` 无半成品、历史不推进；随后把残留回拨到 30 分钟宽限期之外，
  走完「`scan-unreferenced` 发现 → `cleanup-unreferenced` 确认清理 → 再扫描为 0」闭环，
  再提交并恢复 head 逐字节一致。
- C10 在五档调用上各跑 `4 MiB - 1` / `4 MiB` / `4 MiB + 1` 三个尺寸，各两次提交并逐位恢复；
  并行度 16，边界两侧均无损。
- C8 固定 8 MiB 密集不可压缩内容：快照 8,408,224 B 对逻辑 8,388,608 B（约 +0.2% 块开销）。

## 6. 退役基线的对比复测

`stress_baseline` 已于 2026-10-21 退役删除。本轮为满足对比要求，临时从历史提交 `63934eb^`
恢复 `tests/stress_baseline.rs`，在 `D:\temp\Artworks` 的独立副本 `D:\temp\Artworks-2026-10-25`
上运行，运行后删除该文件；仓库没有重新加入该测试目标，也没有改 CI。

- 首次运行在最后一条「源仓库数据字节数不应变化」断言失败：副本是 schema v4，当前产品在首次
  打开时按既定策略迁移到 v6，主库与元数据增加 565,248 B。这是**测试侧假设过期**，不是产品
  缺陷；在迁移后的副本上原样重跑，2/2 通过。
- 对比数据、首次运行的冷缓存/迁移开销与逐项结论写入
  `docs/planning/archive/0.3-baseline-2026-10-18.md` 第 7 节。
- 关键结果：深链恢复 256 节点每步 0.8 ms（基线 0.9 ms）、最早节点 196.5 ms（223.9 ms）；
  取消地板仍在 12–16 ms；`scrub` 32.9 s（182.5 s）、灾备 53.4 s（234.9 s）、吞吐
  188.4 MiB/s（42.86 MiB/s）。绝对耗时只用于同机同档对照。

## 7. 文档与基线文件

- `docs/user/stress-test-report.md`：新增「2026-10-25 复跑摘要（0.3-beta.1，含 `manual` 档）」，
  更新报告日期、被测版本、`manual` 档状态与 C9/C10 运行状态。
- `docs/guides/validation.md`：补充本轮含 `manual` 的全量复跑结论。
- `docs/guides/release-policy.md`：注明 0.3-beta.1 的发布前轮次已首次跑完 `manual` 档。
- `docs/planning/current-handoff.md`、`docs/planning/todo.md`：同步当前批次状态、删除已完成的
  C9/C10 与 4.8 遗留项。
- `src-tauri/tests/perf-baseline.json`：本轮 `--test-threads=1` 的 release 条目已刷新；
  `machine` 字段保持维护者维护的原值。

## 8. 清理

- 临时恢复的 `tests/stress_baseline.rs` 已删除，工作区只剩文档与基线文件改动。
- `D:\temp\Artworks-2026-10-25` 是本次复测的临时副本，测完删除；原副本
  `D:\temp\Artworks` 未被改动。
- `target/stress/` 下的报告、账本、日志与工作区属于构建残渣，`cargo clean` 可一并清除。

## 9. 版本切分 `0.3.0-beta.1`

2026-10-25，按 `docs/guides/release-policy.md` 的固定动作切分，未打标签：

- `CHANGELOG.md` 的 `## Unreleased` 落为 `## 0.3.0-beta.1 - 2026-10-25`，顶部重新留出空的
  `## Unreleased`；同轮做了一次小结与压缩：素材板文字就地编辑仍是四条独立条目（大改多写），
  `save_pin_board` 的文字载荷并入其中一条，纯内部拆分的五条收敛为「命令名与契约不变」的一段，
  删除四个无消费方字段与清理无效钩子合并为一条；行为事实、命令名、设置项与边界保留，纯内部
  组织与重复表述按冗余压缩。
- 五处版本字段 `0.3.0-alpha.6` → `0.3.0-beta.1`：`package.json`、`package-lock.json`
  （根与 `packages[""]`）、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、
  `src-tauri/tauri.conf.json`。
- `tools/release/verify-metadata.mjs` 的版本→schema 登记表新增 `0.3.0-beta.1 → 6`（沿用 v6，
  本轮不改 schema）。
- `npm run legal` 重跑，`licenses/THIRD_PARTY_LICENSES.html` 的版本字样更新（529 个组件）。
- 版本引用同步：`README.md` 的当前版本行、`docs/guides/release-policy.md` 的过渡版本示例与
  `manual` 档口径一行、`docs/guides/validation.md` 的 2026-10-25 实测段落、
  `docs/user/stress-test-report.md`（改为「该轮在切版本前运行，同日随后切版本」，
  `0.3-beta.1` 统一为 `0.3.0-beta.1`）、0.4 草案的基线与版本锚点三处。
- 验证：`node tools/release/verify-metadata.mjs` 输出
  `Release metadata verified: v0.3.0-beta.1, com.lilith.artworks, schema v6`。

切版本不等于发布，不触发任何门槛；是否打 `v0.3.0-beta.1` 标签仍由维护者单独决定。

## 10. 人工验收（逐项记录）

维护者逐项验收，通过项记录在此处并从 `docs/planning/todo.md` 删除。

### 10.1 0.3 收尾整改第三批（历史页、设置页与长任务模板）

维护者 2026-10-25 实测通过：

- 历史页「总览 / 当前分支」切换正常；总览的紧凑/时间轴视图、节点宽度滑条与刷新后持久化不变。
- 切换分支/作品后的缩略图缓存失效、缩略图刷新按钮与选中节点的「节点信息」栏不变。
- 设置六页（通用/仓储/空间/备份/素材板/调试）的字段显示、保存、取消与内容区滚动不变；空间页
  的垃圾文件扫描确认与仓库外文件滑动确认正常。

仍待实测：精简模式的选择、计数、普通确认与含主动提交/改名节点的滑动确认，以及批级取消行为。

### 10.2 自动备份调度的日志取证（维护者确认通过）

取证对象：`%LOCALAPPDATA%\com.lilith.artworks\logs\Lilith Artworks.log`，覆盖 2026-10-24 17:03
至 2026-10-25 10:03（三次运行，约 17 小时）。维护者依据下列取证确认通过，未另行手工复现：

- **长全量检查后的退避**：同一分支相邻两次 `backup forced-audit` 共 58 组可比较样本，间隔全部
  不小于上次 `backup unchanged` 耗时的两倍，0 例失败。1.378 GB 分支单次 4.8–5.9 s、间隔
  约 2247 s；小分支约 0.5 s、间隔约 473 s。自适应阈值按分支大小分档（小分支 7/8、
  1.378 GB 分支 31/32）。
- **让位日志不刷屏**：整个窗口内 `scheduler yields the candidate scan` 0 条；`backup
  quick-unchanged` 708 次、`backup unchanged` 64 次，快速通道约占 92%，全量检查只在阈值命中
  后出现。
- **退出不被 5 秒兜底拖住**：2026-10-25 00:18:06 的退出链路为 `pin-board finalize done` →
  `shutdown settlement finished: outcome=ok, elapsed_ms=9` → `shutdown sequence finished:
  waited_ms=0`。
- **异常面**：窗口内 `outcome=failed` 0 条，`[gpu]` / `[error]` / `[watchdog]` / `[forensic]`
  均为 0。

同时记录两条非验收结论的观察，供后续判断：

- 每轮 1.378 GB 分支的全量审计（快照约 4.7 s）之后必有一条
  `[slow] repository mutation held the lock for ~5000 ms` 的 WARN（阈值 1000 ms），约 37 分钟
  一条。属按设计持写锁跑审计的正常路径，不是回归；是否把这条告警对备份路径放宽或降到 debug
  待维护者决定。
- `run:a4beac`（2026-10-25 01:02:45 启动）只写了 5 行便消失，没有任何退出日志，属非正常结束；
  10:00:07 的新进程按设计打出 `[owner] replacing stale owner: pid=31516,
  operation=backup:automatic` 回收其持有的 GUI owner 锁。恢复路径正常。

窗口内 `[boundary]` 与 `[cleanup] retained` 均为 0 条，只说明这段时间没有触发对应场景，
不作为这两项的验收依据。

### 10.3 0.3 收尾整改第六批（发布页与识别页）与自动备份调度的界面项

维护者 2026-10-25 实测通过：

- 拆分后行为不变：切换分支保持最新请求结果；生成质量预览、签名发布、「取消发布并删除本地
  数据」、只读记录的再次导出与删除都正常。
- 质量预览局部图与框选放大镜：放大后局部与缩略图对齐、平移停稳后才请求、切换导出图/原图不
  串图；发布页与识别页的放大镜跟随指针且不随移动重复请求。
- 后台快速检查或空闲校验完成后，历史页状态行更新，但 mindmap 不重建、缩略图缓存不清空；
  自动备份创建新节点时仍完整刷新。
- 内容未变化的提交不再复检、也不再修复损坏的 head snapshot：界面只显示「内容未变化」，损坏
  仍由恢复拒绝、`scrub` 与空闲完整性检查（默认关闭）暴露。

第六批仍待实测：取消提示时序（两句话的文案与正常完成不出现提示）、只读记录、识别页拖放导入。
第三批仍待实测：精简模式的选择与四种确认、批级取消；设置页四个长任务的 busy 提示。

### 10.4 用无头入口接管的程序化验收（2026-10-25）

维护者授权把「能程序判定」的项改用无头入口与探针处理，界面观感仍走人工。

**GB 级提交与恢复复测（原 release 复测项）**：release 无头构建 + 1.378 GB 真实文件的副本，
首次基线提交 3,421 ms、含反向增量的真实提交 4,008 ms、恢复旧节点 3,415 ms（峰值内存
46.1 / 50.4 / 41.4 MB），恢复产物 SHA-256 与节点记录一致；块数 114,616 与计划 §6.1 吻合。
分段耗时拿不到——无头入口不装日志器，`backup committed` 的打点字段不落盘。数据与口径限制写入
`archive/backup-scheduling-fix-plan-2026-10-20.md` 新增的 §6.4。

**同仓库写入排队（alpha2 遗留项的一部分）**：`cargo test --features headless --test
stress_concurrency` 9/9 通过——第二个写者被明确拒绝、被强杀的写者不留锁、读命令在写者持锁
期间仍作答（仓库操作锁、灾备、精简三种持锁形态各一条）、并发读互不阻塞、高频顺序命令后仓库
一致、精简与全库校验并发不损坏、写者持哨兵时内容级只读被拒。仍待人工：同进程写者排队而非
互报「另一个进程写入」的观感，以及自动备份进行中保存/退出画板的界面行为。

**归档动作**：`test-audit-2026-10-21.md` 移入 `archive/`，报告结论只作历史记录，未完成的
3.7 与 4.6 仍在 `todo.md`；「前端不设硬性耗时门槛」与「自适应调度不再另开端到端」两句并入
`docs/guides/validation.md` 的「明确禁止」。

### 10.5 拖放暂存目录的启动清扫（探针 + 日志，2026-10-25）

维护者重启安装版 release 构建（`D:\Lilith\Lilith Artworks\lilith-artworks.exe`，
`run:24280c`，10:38:40 启动）后，按探针目录判定：

| `%TEMP%` 下的目录 | 修改时间 | 结果 |
| --- | --- | --- |
| `lilith-artworks-drop-11111111-1111-4111-8111-111111111111` | 30 分钟前 | 已删除 |
| `lilith-artworks-drop-33333333-3333-4333-8333-333333333333` | 启动前刚建 | 保留 |
| `lilith-artworks-drop-not-a-uuid` | 30 分钟前 | 保留（后缀不是合法 UUID） |

日志佐证：`10:38:41 drop stage sweep finished: reclaimed=1, failed=0`。与
`authenticity/drop_stage.rs` 的规则一致——只处理 `lilith-artworks-drop-<合法 UUID>`、按目录自身
mtime 判断、宽限期 10 分钟。

该组的另一半（退出后不留本次运行的会话目录）需要先真的拖放一个文件让会话目录建起来，
仍待人工。

### 10.6 取消发布保留与预览缓存上限的单元测试取证（2026-10-25）

两项的**数据侧**由既有单元测试覆盖，维护者确认以测试结果为准；界面与日志侧仍留人工。

- **取消发布时文件已被替换**：`cargo test --lib cancel_publication` 2/2 通过。
  `cancel_publication_retains_files_whose_content_changed` 断言：被替换的成品与消失的认证副本
  都不进待清理队列（`cleanup_ids` 为空）、两条路径都进 `retained_paths`、文件仍在仓库里、
  发布态数据（成品 / 认证记录 / 认证配置）全部清零；`cancel_publication_keeps_external_outputs_and_resets_config`
  断言正常情形下两条路径按 `repository_file` 登记进队列。仍待人工：被保留的文件能被设置页
  「空间」页的垃圾扫描扫到、日志里有 `[cleanup] retained`、发布页不出现失败横幅。
- **预览缓存的会话内上限**：`cargo test --lib preview_cache` 1/1 通过。
  `preview_cache_pruning_keeps_the_latest_entries_and_removes_metadata_pairs` 写入 20 份后断言
  只留最近 16 份、`.jpg` 与同名 `.json` 成对删除。512 MiB 总量分支没有被该用例覆盖（样本太小），
  仍待人工：多次切换参数后目录总量不超过 512 MiB，且最近一次预览的高清局部与框选放大镜正常。

## 11. `0.3.0-beta.1` 的表层文档收尾（2026-10-25）

切版本之后对表层文档做了一轮清理，目标是让 `current-handoff.md` 与 `todo.md` 只留当前状况、
未完成事项与规划，历史增量只出现在归档里。本轮不改产品代码与版本字段。

- `todo.md` 重排为「0.3 状态 → 人工验收（按来源分小节）→ 待实现/待补强 → 维护者决策 → 纪律
  约束」，并删去三类内容：0.3 逐批实现的叙事（每个 alpha 的范围、验收与切分）、已落地批次的
  处理清单（测试体检两批、0.3 收尾七批的完成情况、空闲链路校验改向等）、已实现或被 0.4 取代的
  补强行（重任务内部并行的 release 复测已于本轮完成；每 artwork 独立数据库已由 0.4 草案接管）。
  删除的内容均已在 `handoff-log-2026-10-18.md` 至 `handoff-log-2026-10-22.md`、两份计划正文与
  `docs/modules/` 中留档，本轮没有产生新的事实。仍生效的决策压缩成一句结论加归档指针。
- `todo.md` 的「待实现/待补强」表并入测试体检遗留的 3.7（大文件内存与快照上界收紧）与 4.6
  （素材板真实上限与条目类型过滤），并去掉与 0.4 决策冲突的「每 artwork 独立数据库」行。
- `docs/README.md`：「规划归档」条目删掉「0.3 尚未完成（阶段 9 待查、发布前压测与打标签待安排）」
  这段过期表述与「0.2 基线供 alpha1 对照」的旧定位，改为当前的归档索引说明与基线对比报告。
- 根 `README.md`：版本行更新为 `0.3.0-beta.1` 与 `dev` 开发线；「主要功能」按当前实现补全
  仓库校验、素材板文字与标注工具、自动备份与长任务、导出与回收站、识别页；「仓库升级说明」
  写明 0.2 → 当前版本会连续执行 v4 → v5 → v6 两次迁移。语言风格与原段落保持一致。
- 版本身份统一：`archive/0.3-baseline-2026-10-18.md` 的 `0.3-beta.1` 字样统一为 `0.3.0-beta.1`；
  `archive/backup-scheduling-fix-plan-2026-10-20.md` §6.4 的标题空行补齐。
- 顺带修掉几处与其他文档冲突的口径：`release-policy.md`、`CONTRIBUTING.md` 与
  `docs/modules/pin-board.md` 原先都写「人工验收结果先记入 `current-handoff.md`」，与实际做法
  （直接记入归档日志的验收小节、同时从 `todo.md` 删除）不符，已统一；`release-policy.md` 的
  schema 映射补上 `beta.1`；`SECURITY.md` 的受支持版本改为「最新的稳定版 `0.2.0` 与
  `0.3.x` 开发线」。
- `CHANGELOG.md` 的 `0.3.0-beta.1` 小节复查：第 9 节的压缩丢掉了几条行为事实，本轮补回——
  无消费方字段是四个（补上 `watermarkRegionCount` 与「预览原图大小不受影响」）、图层面板在
  「每次启动与工作区重挂载」时收起、`create_branch` 的分支行与根检查点同事务与子树删除的单次
  校验、类型化错误到命令错误种类的映射、质量预览与签名发布共用取消状态机（取消提示只报一次）。
- 验证：`node tools/release/verify-metadata.mjs` 输出
  `Release metadata verified: v0.3.0-beta.1, com.lilith.artworks, schema v6`。
- 发布前检查（不改任何发布产物）：`npm run legal` 重新生成 529 个组件，与仓库内
  `licenses/THIRD_PARTY_LICENSES.html` 逐字节一致；三个定向测试通过——
  `real_c2pa_fixture_matches_record_trustmark_and_claims`、`tampered_c2pa_fixture_fails_closed`、
  `pinned_digests_match_the_shipped_models`；C2PA fixture 证书有效期到 2126-07-23，无需重签；
  `write-release-notes.mjs` 能按带日期小节生成 beta.1 的发布说明（含 schema v6 与未签名披露）；
  两个 workflow 的 RustSec 忽略列表逐字一致；本地未配置 Authenticode 证书，按发行政策走未签名
  发布路径，SBOM、SHA-256 与许可包由标签触发的 CI 生成。
- 本地 CI 预检（`npm run ci:local`，2026-10-25）：20 项里 19 项通过（两条 npm 审计、发布元数据、
  平台分支、许可证生成与一致性、Rust 目标预编译、headless 二进制、`gate_guard` 与九组便宜压力
  测试全部 PASS）。RustSec 审计首次因 advisory-db 拉取失败（libgit2 网络错误）判 FAIL，改用
  `CARGO_NET_GIT_FETCH_WITH_CLI=true cargo audit` 并以同一忽略列表重跑：0 漏洞、退出码 0，另有
  4 条 warning（zbus_names / zvariant / zvariant_derive 标记 unmaintained、tokio 的
  `AsyncFd` I/O safety），均为平台条件编译路径，Windows 构建不启用。带
  `CARGO_NET_GIT_FETCH_WITH_CLI=true` 复跑整轮后 20/20 全部 PASS（总耗时 143.8 s）。该次预检是
  在工作区带暂存改动的情况下运行的，脚本已提示这一点。
- 发布 CI 的其余门也在本地跑过：`npm run build` 通过、`npm test` 56 个文件 500/500 通过、
  `cargo test --lib` 434 passed / 1 ignored、`cargo fmt --check` 干净。
