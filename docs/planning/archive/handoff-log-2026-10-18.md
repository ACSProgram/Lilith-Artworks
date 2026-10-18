# 2026-10-18 交接归档：0.2 收口、0.3 开发线建立与 alpha1 完成

本文件归档 2026-10-18 从 `../current-handoff.md` 与
`../0.3-architecture-algorithm-plan-2026-10-05.md` 清出的已完成内容：`0.2.0` 正式版收口、
`main` 冻结与 `dev` 建立为 0.3 开发线，以及 0.3 alpha1（阶段 1–5）的实施与计划裁剪。
当前状态改看 `../current-handoff.md`，剩余工作与批次划分看该计划文档。

## 0.2.0 正式版收口

- **切分提交**：`0be6224 Cut version 0.2.0 and align release infrastructure`。产品代码与
  `0.2.0-rc.4` 完全一致，切分只改版本字段、CHANGELOG 与发布基建：
  - `CHANGELOG.md` 新建带日期小节 `## 0.2.0 - 2026-10-17`，并入 rc.4 小节内容，补入压测、桌面
    验收与已知限制，并记录发布基建修正；顶部补回 `## Unreleased` 暂存段。
  - 五处版本字段、许可清单与 README 版本行同步到 `0.2.0`；`verify-metadata.mjs` 验证通过。
  - release workflow 对稳定标签不再标 prerelease 并更名；verify-metadata 补 Cargo.lock 版本校验。
- **标签**：`v0.2.0` 附注标签已打并推送，指向 `0be6224`；`v0.2.0-rc.3` 未打标签。
- **压力测试**：rc.4 的全套餐压力测试按维护者口径即 0.2.0 正式版的压力测试（产品代码一致）。
  2026-10-17 已在 release 构建上完整重跑并通过（八组便宜分组 + gate_guard 40 用例、C 组五档
  7/7 含 C7、G 组 extreme 档、D/E/F 规模组、P 组基准复测同量级），实测数据在
  `docs/user/stress-test-report.md`，被测版本口径为 `0.2.0`。
- **自动门槛**：Windows CI 已在 rc.4 与切分提交上通过；正式版门槛由 `v0.2.0` 标签触发的 release
  workflow 执行（依赖审计、许可一致性、从标签构建 NSIS 并静默安装检查、SBOM、校验和、attestation、
  草稿 release）。
- **人工门槛**：干净 Windows 用户环境全清单（2026-10-16）与任务调度总控、空闲链路校验及安全批次
  的界面项（2026-10-17，随 rc.4）均已通过；0.2.0 与 rc.4 产品代码一致，验收沿用，记录见
  `handoff-log-2026-10-16.md`。
- **发布口径现状**：GitHub 私密漏洞报告渠道已开启，满足发行政策前置项。Authenticode 签名仍无法
  进行；按 `docs/guides/release-policy.md` 允许未签名发布，发布说明须显著披露未签名状态及
  SmartScreen 影响，签名相关人工门槛项在证书可用前无法执行。

## 分支模型调整

- `main` 固定为 0.2 发布线，停留在 `0be6224`，不再接收 0.3 改动。
- `dev` 从 `0be6224` 创建，作为 0.3 开发线。批次划分随后调整为三批：alpha1（阶段 1–5）、
  alpha2（阶段 6 + 针对 alpha1 的测试与修复）、alpha3（原阶段 7–9），见下节与计划文档。
- 0.3 的日常改动仍遵守版本号滞后策略：不切版本时不改五处版本字段，改动写入 `CHANGELOG.md` 的
  `## Unreleased`；alpha 批次不自动等于公开版本，切版本与打标签由维护者单独决定。

## 0.3 alpha1 架构与算法底座（阶段 1–5）

**切分**：2026-10-18 切分 `0.3.0-alpha.1`（未打标签的过渡版本）。五处版本字段、`CHANGELOG.md`
带日期小节与许可清单同步；`verify-metadata.mjs` 的映射改为显式版本→schema 登记表，并登记
`0.3.0-alpha.1` → repository schema v4。

**实施（`dev` 自 `0be6224` 起，按提交顺序）**：

- `d6206ac`：0.3 开发线落地，计划定稿为可实施口径，表层文档与发布元数据一并同步。
- `aa937ac`：**阶段 0** —— 补齐取消延迟 / 锁等待 / 调度扫描的测量能力（`stress_baseline` 组、
  headless 只读探针 `scheduler-scan`、闸门取消计时与 `Spawn::scope`），在临时仓库副本上采集 0.2
  release 档基线；副本采集后已还原。
- `5881980`：**阶段 1** —— 新增 `app/command_runtime/`（`CommandContext`、typed `CommandError`、
  `CancelToken`、`OperationCoordinator`、`AppEvent` v1 信封）与 `app/usecases/`；Library、History/
  Backup、Authenticity、Cleanup、Settings、Pin-board 的仓库入口统一经 coordinator（命令名/DTO/
  用户文案不变）；headless 改为 context 装配器并共享同一批 use-case；命令注册集中到
  `app/command_registry.rs`；发布编排移入 `authenticity/artifact.rs`。
- `b88ec1a`：**阶段 2** —— 设置文件新增独立 `revision`，设置页走 CAS 并返回 `settings_conflict`；
  托盘与窗口几何统一经 `SettingsService`；跨进程哨兵升级为按写操作的仓库级 owner lock（SQLite
  内核锁即存活证明，owner 元数据只作诊断）；退出结算的失败与超时接入 `[watchdog]` 诊断；前端拆
  `AppRuntimeProvider`、`useSettingsController`、`useRepositoryMaintenance`、
  `RuntimeProgressProvider`、`SettingsDialog`、`NoticeCenter`。
- `e02ded7`：**阶段 3** —— 统一 `AppEvent` 订阅 bridge（单原生监听、版本/仓库 key/时间戳过滤）与
  共享 `useRequestEpoch`、`useAsyncOperation`；History、Library、Authenticity、Pin-board 四个
  controller 统一请求代次、busy 与错误策略；运行态由事件即时唤醒，350 ms 高频轮询降为 3 秒兜底
  （作品树保留 30 秒）；素材板新增 `usePinBoardController`，导入/导出进度改走 `operationProgress`。
- `b4eddf1`、`401f754`：**阶段 4** —— ChunkFile v2 写入（80/120 B 头、44 B 块记录、快照去重）
  并保留 v1 reader；新增 `format/container.rs` 容器感知分块；`ChunkStore` 段索引与 reader 游标；
  `materialization_chain` 改为一次递归 CTE；版本化 cheap fingerprint；`CancelToken` 移到 crate 根
  并贯穿创建、delta、复制、链解析、导出与 scrub；`stress_bench` 新增 P1B。
- `e4832dd`：**阶段 5** —— 中立 `contracts` 模块（`ReferenceInventory`、
  `RepositoryIntegrityProvider`）；cleanup 改连接内 `TEMP` 引用表 + anti-join、分批可取消且幂等；
  灾备核心不再导入 Library/Authenticity，manifest v2 同时记录完整摘要与抽样摘要、发布前默认抽样
  复核；`BackupState` 改为每任务独立 `OperationId`/`CancelToken`；调度按 deadline 最小堆、单分支
  失败不再阻塞、空闲校验按 Artwork 并行（最多 4）并在连续 4 次自动备份后 aging 插队；`stress_bench`
  新增 P7/P8。
- `11dbc21`：alpha1 注释与文档对齐。

**关键口径（alpha1 各阶段落实时定稿，均已在代码中生效）**：

- **阶段 2 owner lock**：按写操作 acquire/release（GUI、headless、维护入口共用，只读不取）；Windows
  stale owner 以 SQLite 内核锁为唯一存活证明，取得锁即覆盖旧元数据，不按 PID/心跳回收；锁文件从
  数据库发现、灾备复制与未引用扫描中排除。
- **阶段 2 前端范围**：按计划全量拆分 `AppRuntimeProvider` 等六个前端单元；事件订阅与 controller
  模板留给阶段 3。
- **阶段 3 事件 TTL**：丢失事件后的快照轮询固定 3 秒；作品树保留 30 秒兜底（后台自动备份写库不经过
  coordinator 事件）。
- **阶段 4 快速检查指纹**：复用 `branches.last_source_modified_ms` 存储版本化指纹，不新增 schema；
  采样首尾各 64 KiB，缺少亚秒 mtime 精度时退回全量。
- **阶段 4 ChunkFile v2**：默认开启，只影响新写入节点；设置页保留 `chunkFileFormatV2` 开关以便回退，
  旧 v1 节点始终可读。发布前仍用 benchmark 复核往返一致性、CPU 与恢复时延。
- **阶段 5 调度与并行**：aging 按次数（每连续完成 4 次自动备份先跑一轮就绪的空闲校验），不引入时间
  阈值；空闲校验按 Artwork 并行（同一作品同时最多一个任务、每批最多 4 个），只并行只读准备，写回
  仍由共享运行锁串行。
- **阶段 5 灾备最终复核**：发布前默认抽样复核（每文件首尾各 64 KiB）；完整摘要仍在复制阶段计算并
  写入 v2 manifest，`Full` 全量复核保留为内部选项。

**计划文档裁剪**：随本次收尾，把已完成内容从
`../0.3-architecture-algorithm-plan-2026-10-05.md` 清出并归入本归档：原 §0 复查结论（2026-10-11 /
2026-10-17 / 2026-10-18 三轮复查与落实前修订、已修正问题表、与既有能力的关系）、§1 现状判断
（P1/P2 证据表）、§2 目标架构（分层图与约束）、§3.A–E（阶段 1–5 的实现口径与 §3.D 研究结论）、
§4.1（阶段 0–5 与 alpha1 收尾的阶段表）、§5 的 alpha1 完成条件、§6 中已实现阶段（2–5）的决策。
计划文档现在只保留剩余工作（阶段 6–9）与批次划分，并把已落地契约改为索引
`docs/architecture/overview.md`、`docs/modules/` 与 `CHANGELOG.md`。裁剪出的完整原文另存为
`0.3-architecture-algorithm-plan-alpha1-2026-10-05.md`（本目录）。

**验收状态**：阶段 1–5 已实现并提交，但 **GUI 人工验收与发布前压力测试未执行**；待维护者验收的事项
一并记入 `../todo.md`。

## alpha1 压力测试未通过项修复（阶段 1–3）与计划归档

release 档整套压力测试（`manual` 8 GiB 档未跑）在 2026-10-18 留下 8 条未通过记录（B1/B2/B4/B5、
C1、C5、C2/C4/C6、G4），另有维护者明确要求的一项产品侧改动（小选区水印警告）与阶段一实施时新发
现的 `stress_concurrency` J5 偶发。三者按 `alpha1-stress-failure-fix-plan-2026-10-18.md` 分三阶段
实施，现已全部落地；三处改动面互不依赖（分别落在 `headless`/`backup`、`authenticity` 与
`stress_concurrency` 的测试），共用同一批便宜验证。

- **阶段一（未通过项清零）**：8 条失败只有三类根因。B/C1/C5 六条**同源**——热循环的取消检查每次
  都发一枚交互检查点，检查点总数随分块数膨胀、marker 文件涨到 MB 级；产品侧把取消观察分成
  「交互检查点」（只在显式边界，写 marker 并在 `--cancel-on-stdin` 下阻塞）与「静默观察」
  （热循环只读取消状态，不写 marker、不阻塞）两层，B 组四项与 C1/C5 随之转绿，A1/A3/A6/C7 的
  序号也重新对应它们注释里声称的位置。C2/C4/C6 是测试断言仍是 v1 口径（v2 会对稀疏夹具去重并
  压缩，快照合法地小于逻辑大小），改成「不超过逻辑大小 + 每块开销」并新增密集不可压缩用例 C8。
  G4 是外部水印引擎在 96/128 像素量级的固有鲁棒性限制（C2PA soft binding 始终匹配），夹具改到
  512 级、小图回读率降级为观察值。可选的账本加固一并落实。复验：便宜九组 + `gate_guard` 通过，
  `stress_large` 的 `default` 档 8/8 通过（含 C8）。
- **阶段二（小选区水印警告，产品侧新增）**：发布时按「送进编码器的 rendition 上选区任一边小于
  实测最小稳定边长 256 px」给出结构化警告（区域序号、像素尺寸、阈值）并写一行日志，发布**照常
  成功**；框选处就地提示、发布结果区显示，无头 `publish` 结果 JSON 带该字段。`stress_authenticity`
  新增 G6 断言 96/128/192 级有警告、256/512 级无警告。口径是「降低风险而不是保证」，边界写法见
  `../user/stress-test-report.md`。
- **阶段三（J5 并发断言口径，只改测试）**：维护者已定「改断言不改锁」。`compact` 与 `scrub` 真
  并发时，精简**要么成功、要么拿到明确的「另一个进程在写」拒绝且随后重试成功**——全库校验的哨兵
  *探测* 也用 `BEGIN IMMEDIATE`，零等待下可先抢到锁；两类拒绝分别计数，轮数由 4 提到 6。复验：
  常规负载连跑 21 次 × 6 轮（126 轮）全绿，放大并发再跑 162 轮全绿，重试分支在放大负载下被触发
  5 次并全部恢复。产品侧「探测是点检查而不是锁」的不对称不改，继续记录在该报告的「边界」。

**归档**：计划的完整正文（三阶段的根因、实测证据与落实口径）随本次收尾移入本目录：
`alpha1-stress-failure-fix-plan-2026-10-18.md`。逐条根因、修复与复验并入
`../user/stress-test-report.md` 的「alpha1 未通过项：诊断与修复」与「并发与交错（I、J 组）」；
`../todo.md` 只保留 alpha1 剩余未完成项（五档大文件存储数值采集、`extreme` 档复跑、CPU 时间度量
口径）。
