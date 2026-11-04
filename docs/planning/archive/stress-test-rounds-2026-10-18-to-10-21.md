# 压力测试历史轮次（2026-10-18 至 2026-10-21）

本文件承接从 `docs/user/stress-test-report.md` 清出的历史轮次记录：alpha1 那轮的未通过项追因与
修复，以及 2026-10-18、2026-10-19、2026-10-21 三次复跑摘要。它们描述的都是当时那一轮的状态，
其中提到的「当前行为」早已被后续批次取代，只作为追溯用，不再作为执行依据。

- 当前轮次的实测数据：`docs/user/stress-test-report.md`（面向使用者）与
  `docs/guides/stress-test-evidence.md`（逐组明细）。
- 最近一次全量轮次（2026-10-25，含 8 GiB `manual` 档）：见 `handoff-log-2026-10-25.md`。
- 运行方式与门槛：`docs/guides/validation.md`。

## alpha1 未通过项：诊断与修复


2026-10-18 的 alpha1 复跑中留下 8 条未通过记录。它们**不是数据损坏**：全部失败场景里
`verify` + `scrub` 都通过，恢复产物逐字节一致。逐条追因后只有三类根因，修复批次按「产品侧
一处 + 测试侧两处」处理，下表「复跑失败信息」列是测试自己给出的断言原文。

| 组 | 复跑失败信息 | 已确认根因 | 处理 |
| --- | --- | --- | --- |
| B | `stress_crash` B1 提交强杀：仓库内残留临时文件 `artworks/<id>/temp/.tmpXXXX` | 热循环的取消检查每次都发一枚交互检查点，检查点总数随分块数膨胀；测试按固定序号强杀，强杀点漂移到「快照临时文件已创建、尚未发布」处 | 产品侧：取消观察分两层（见下） |
| B | `stress_cleanup` B5 孤儿回收：同一残留 | 与 B1 同一个强杀点 | 同上 |
| B | `stress_crash` B2 恢复强杀：应恰好留下一个未发布临时文件，实际 0 | 同上：第 5 个检查点已漂移到链解析之前，输出临时文件尚未创建 | 同上 |
| B | `stress_crash` B4 事务内强杀：强杀应落在事务写入之后、提交之前 | 同上：事务标记点是第 12 个检查点，测试按第 5 个强杀，从未落到标记点 | 同上 |
| C | `stress_large` C1 账本：实测增量超出预期（`default` 档 83.5 MB / 预期 47.2 MB） | 同源：一次 `scrub` 的检查点标记文件可达 MB 级（3 节点链实测 5.25 MB），它位于测试夹具的 `scratch/` 下、被计入工作区占用 | 同上（标记文件回到百字节级）；账本口径同时收紧到「仓库 + 工作文件」 |
| C | `stress_large` C5 恢复产物回收：拆除后工作区占用仍高出约 3.3 MB | 同源：一次 `restore` 的标记文件实测 3.31 MB，与断言缺口 3.29 MB 一致 | 同上 |
| C | `stress_large` C2/C4/C6 快照体积：`snapshot 应约等于逻辑大小` 失败 | 测试断言仍是 v1 口径（「不压缩、不去重」）；v2 对重复块只写引用并用 zstd 压缩，稀疏夹具里的全零区被折叠，快照合法地小于逻辑大小 | 测试侧：断言改为「不超过逻辑大小 + 每块开销」，并新增密集不可压缩用例（C8）单独证明「快照跟随逻辑大小」 |
| G | `stress_authenticity` G4 回读：`watermarkPresent` 应为 `true`，实际 `false` | 外部水印引擎在 96/128 像素量级上的固有鲁棒性限制（水印 ID 每次发布随机，失败与载荷相关）；C2PA soft binding 始终匹配 | 测试侧：G4 夹具改用 512 级源图（实测稳定量级），小图的回读率记为观察值（见「边界」） |

**产品侧修复：检查点分层。** 无头入口此前把取消来源直接接成闸门检查点，而分块主循环每块
查询一次取消，于是「每条边界一次检查点」变成「每块一次检查点」。现在取消观察分两层：
`CancelToken::is_cancelled` 是**交互检查点**，只在显式边界调用，仍写一行 marker、并在
`--cancel-on-stdin` 下阻塞等判定；`CancelToken::observed` 是**静默观察**，供热循环使用，只读
「取消是否已被判定」。边界判定的取消仍会被热循环在下一次静默查询时立刻看到，因此取消延迟不
回退；GUI 的取消来源本来就是廉价读取，两层共用同一个闭包、行为不变。发布产物不含无头入口，
GUI 路径行为逐位不变。

**复验结果。** `stress_cancel`（A1–A6，含逐个检查点的取消延迟与残留断言）、`stress_crash`
（B1–B4）、`stress_cleanup`（B5）、`stress_authenticity`（G4/G5）、`stress_large` 的
`default` 档（C1–C8）全部通过；A1/A3/A6/C7 这些此前「通过但覆盖位置已漂移」的用例，序号
也重新对应它们注释里声称的取消位置。修复批次与逐条证据见
`docs/planning/archive/alpha1-stress-failure-fix-plan-2026-10-18.md`。

## 复跑摘要

### 0.3.0-alpha.1 复跑摘要（2026-10-18）

环境：Windows 桌面机（16 逻辑核）、release 构建、`manual` 档未跑；`stress_large` 四档串行，
其余分组按 `cargo test` 默认并行。表中「结果」列是 2026-10-18 那一轮的原始结果，「修复批次复验」
列是 alpha1 未通过项修复批次（同日、debug 构建、`stress_large` 只跑 `default` 档）的结果。

| 分组 | 场景数 | 2026-10-18 结果 | 修复批次复验 | 备注 |
| --- | --- | --- | --- | --- |
| `gate_guard` | 2 | 通过 | 通过 | 门控守护（裸 `cargo test` 也跑） |
| `stress_cancel`（A） | 6 | 通过 | 通过 | 28 次受控取消；修复后 A1/A3/A6 的序号重新对应注释位置 |
| `stress_crash`（B1–B4） | 4 | **1 通过 / 3 未通过** | 通过 | 见「alpha1 未通过项：诊断与修复」 |
| `stress_cleanup`（B5） | 1 | **未通过** | 通过 | 同上 |
| `stress_damage`（R1–R6） | 6 | 通过 | 通过 | 含随机化单点损坏与画板 DDS |
| `stress_format`（M1–M4） | 4 | 通过 | 通过 | 见「格式矩阵（M 组）」 |
| `stress_pin_board`（H1–H3） | 3 | 通过 | 通过 | |
| `stress_authenticity`（G1–G6，默认档） | 5 | 通过 | 通过 | G4 夹具已改到 512 级、G6 覆盖小选区警告；16K 大图明确跳过 |
| `stress_authenticity`（`extreme` 档） | 4 | 通过（重跑） | 未跑 | G4 夹具改到 512 级；2026-10-19 已随 16K 大图档复跑（5/5 通过） |
| `stress_concurrency`（I/J） | 9 | 通过 | 通过 | J5 已按「精简要么成功、要么被明确拒绝并重试成功」改写并复跑全绿（见「并发与交错（I、J 组）」） |
| `stress_input`（K1–K5、F1） | 5 | 通过 | 通过 | |
| `stress_scale`（D1–D3、E1、F1） | 5 | 通过 | 未跑 | 43.6 s |
| `stress_bench`（P1–P8） | 9 | 通过 | 未跑 | `--test-threads=1`，24.6 s |
| `stress_baseline` | 2 | 通过 | 未跑 | 真实仓库副本 ≈9.83 GiB，487.7 s |
| `stress_large`（C1–C8） | — | **C1–C7 每档 2 通过 / 5 未通过** | 通过（`default` 档 8/8） | 2026-10-19 四档各 8/8 通过，见下；`manual` 只在发布前跑 |

整轮墙钟约 50 分钟（不含 `manual`）：`stress_large` 四档占 37 分钟（其中 `extreme` 28 分钟），
`stress_baseline` 占 8 分钟，其余分组合计约 5 分钟。

> 2026-10-21 更新：素材板笔迹批次新增的 `stress_pin_board` H4/H5 与 `stress_bench` P9 已在
> 0.3.0-alpha.4 的 release 档首跑并通过，P9 数值已写进上表与 `tests/perf-baseline.json`；
> 全套（除 `manual` 档）复跑结论见下节。

### 2026-10-19 复跑摘要（alpha1）

用 release 构建补跑了此前缺的项，全部通过：`stress_large` 四档（`default` 37.6 s、`large`
133.5 s、`heavy` 364.5 s、`extreme` 2733.4 s，合计约 54.5 分钟）、`stress_scale`（62.3 s）、
`stress_authenticity` 的 16K 档（135.8 s；发布峰值内存 2.06 GB，上限 3.22 GB）、
`stress_baseline`（494.8 s，真实仓库副本 ≈9.83 GiB）与 `stress_bench`（P1–P8 九项）。`manual`
档只在正式发布前运行。

### 2026-10-21 复跑摘要（0.3.0-alpha.4）

环境：Windows 桌面机（16 逻辑核）、release 无头构建（`--features headless`，测试进程本身仍是
debug 档）、`manual` 档未跑；`stress_large` 四档串行。除 `manual` 外的全套一次通过，含素材板
笔迹批次新增的 `stress_pin_board` H4/H5 与 `stress_bench` P9 的发布档首跑：便宜九组 44/44
（含 `gate_guard` 3/3）、`stress_pin_board` 5/5、`stress_scale` 5/5、`stress_baseline` 2/2
（未设仓库副本，真实仓库场景按设计跳过）、`stress_bench` 10/10、`stress_large` 四档各 8/8、
`stress_authenticity` 的 `extreme` 档（16K 大图）5/5。

> H5 首跑失败过一次，原因是**测试侧**的过期假设：畸形样本用了「单点笔迹」，而单点是合法的
> 圆点笔迹（产品行为正确）。把样本改成空点后 5/5 通过，产品代码零改动。

本轮各档都比 2026-10-19 那轮快不少（`C1` 的 scrub 尤其明显），是磁盘缓存热的缘故，不构成性能
结论；跨轮比较仍以 `stress_bench` 的基线为准。

C 组 `C1` 关键实测（其余断言同轮通过；四档提交次数按档位递减，是场景设计的一部分）：

| 项 | default | large | heavy | extreme |
| --- | --- | --- | --- | --- |
| 逻辑文件 | 64 MiB | 256 MiB | 1 GiB | 4 GiB |
| 提交次数 | 21 | 21 | 11 | 6 |
| 提交总耗时 | 13.0 s | 43.7 s | 137.0 s | 198.9 s |
| 恢复最早节点 | 1.2 s | 5.2 s | 17.4 s | 49.8 s |
| 恢复最新节点 | 0.27 s | 1.1 s | 5.1 s | 23.2 s |
| 全库 scrub | 2.1 s | 8.4 s | 26.6 s | 60.0 s |
| 快照 / 逻辑 | 35.4 MB / 64 MB | 130.7 MB / 256 MB | 167.5 MB / 1 GiB | 234.3 MB / 4 GiB |
| 增量合计 | 7.5 MB | 27.0 MB | 9.8 MB | 4.3 MB |
| 账本实测 / 预期 | 47.21 / 47.19 MB | 178.64 / 178.62 MB | 254.14 / 254.13 MB | 278.44 / 278.44 MB |

整轮墙钟约 45 分钟（不含 `manual`）：`stress_large` 四档占 32 分钟（其中 `extreme` 27.2 分钟，
四档墙钟 26.1 s / 60.5 s / 192.2 s / 1634.3 s），16K 认证大图 1 分钟，其余分组合计约 8 分钟。
`stress_bench` 的 release 基线同轮刷新并新增 P9（数字见「性能基准（P 组）」）。

