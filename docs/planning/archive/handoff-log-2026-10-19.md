# 2026-10-19 批次记录：alpha1 压力测试收尾与 CPU 时间度量

本文件归档 2026-10-19 完成的批次：给压力测试加入**子进程 CPU 时间**度量、补跑 alpha1 的
发布前手动压力测试、修正 `stress_large` C4 的账本口径。完成后的当前状况改看
`../current-handoff.md`，未完成事项看 `../todo.md`。

## 1. 背景

2026-10-18 的 release 档整套复跑留下两项欠账（release 档四档大文件数值、`extreme` 档 16K
认证大图未复跑），另有一项待决策——性能基准只采墙钟，无法回答「这次改动是否让 CPU 成本变高」。
本批次把三项一并收口，并明确 `manual` 档只在正式发布前运行。

## 2. 新增度量：子进程 CPU 时间（测试侧，零新 crate）

- `tests/stress_support` 的 `Outcome` 新增 `cpu: CpuTime`（用户态 + 内核态 CPU 时间，另存
  `QueryProcessCycleTime` 的周期计数）。读取时机是**子进程已退出、`Child` 尚未 drop**、
  句柄仍有效的窗口，经 `GetProcessTimes` 读一次；不读被测进程内部状态，符合「测试只经命令行、
  stdin 与磁盘事实干预与观察」的纪律。
- 只用项目已有的 `windows-sys`，为此在 `Cargo.toml` 开启 `Win32_System_WindowsProgramming`
  这一个 feature（只增加 extern 声明，不引入新 crate，也不改变发布产物）。`npm run legal`
  无需重跑，产品代码一行未改。
- `tests/stress_bench` 每条测量同时输出墙钟与 CPU 时间，写入 `target/stress-report.jsonl` 与
  `tests/perf-baseline.json`：键 `场景::标签` 是墙钟，`场景::标签#cpu` 是 CPU 时间。严格门
  （`LILITH_STRESS_PERF_STRICT=1`）两列都比，但 CPU 一侧只在基线 ≥ 50 ms 时生效——Windows 的
  CPU 时间按 15.625 ms 的时钟中断记账，短操作会被量化到无法比较。
- 多进程与多轮场景的取值方式：P1 的 48 次提交把全部子进程 CPU 时间累加后摊薄（量级到秒，
  绕开单次记账粒度）；P1B、P2、P4–P8 的墙钟取多轮最小值、CPU 取多轮均值（单轮只有几十毫秒）。
- **严格门噪声审查**：同机同二进制连跑 6 次严格门，2 次误报——一次 `signingMs` 50 ms（基线
  11），一次孤儿扫描 202.7 ms（基线 71.4）。十轮原始数据显示这两项都是**单轮采样**：签名
  8–50 ms、孤儿扫描 51–203 ms；而取三轮最小的其他场景波动只有 1.1–1.5 倍。修法是把 P2 的
  发布 / 回读与 P8 的孤儿扫描也改成三轮取最小（回读只读同一成品、扫描无副作用；确认清理只能
  做一次，保持单轮），基线重采后连跑 5 次严格门全部通过。门的灵敏度结论写进
  `docs/guides/validation.md`：2.5× 只能抓「2 倍级」退化，1.1–1.5 倍的正常波动与同量级的
  真实退化都测不出来。
- 新增 `LILITH_STRESS_PROFILE`：baseline 的构建档默认按测试脚手架自身的 `debug_assertions`
  推断，但文档里的标准做法是「debug 脚手架 + `LILITH_STRESS_BIN` 指向 release 二进制」，
  不显式指定就会把 release 数字记进 debug 档。
- `tests/perf-baseline.json` 顶层新增 `machine` 字段，由维护者维护，记录采集基线的机器
  （i5-13490F / 32 GB / Windows 11 10.0.26300 / debug 脚手架 + release 二进制）；
  `LILITH_STRESS_PERF_UPDATE=1` 只改写对应构建档的表，不动该字段。本轮同时按 release 档
  重录了 alpha1 的基线。

依据（本机实测）：Windows 的进程 CPU 时间粒度 15.625 ms——60 ms 的子进程只报出
`0.0 / 15.6 / 31.3 / 46.9 / 62.5`，离散度 67.5%，比墙钟的 25.4% 更差；同一操作的周期计数
离散度 2.6%，但它跨机器不可比（混合核机器上还受调度到哪类核影响），因此周期数只进失败诊断，
不进基线。整机加载 16 个满载进程时墙钟均值 +127%、CPU 时间 +54%——CPU 时间扣掉了被剥夺的
时间，这正是它能用于「是否更费 CPU」判断的原因；但它看不见磁盘等待，不能替代墙钟。

## 3. 补跑的发布前测试（release 被测二进制）

| 项 | 结果 | 墙钟 |
| --- | --- | --- |
| `stress_large` `default` | 8/8 通过 | 37.6 s |
| `stress_large` `large` | 8/8 通过 | 133.5 s |
| `stress_large` `heavy` | 8/8 通过（C4 修正后复跑） | 364.5 s |
| `stress_large` `extreme` | 8/8 通过（C4 修正后复跑） | 2733.4 s |
| `stress_scale` | 5/5 通过 | 62.3 s |
| `stress_authenticity`（`extreme`，含 G1/G2 16K） | 5/5 通过 | 135.8 s |
| `stress_baseline`（`D:\temp\Artworks`） | 2/2 通过 | 494.8 s |
| `stress_bench`（P1–P8） | 9/9 通过；基线重录后严格门复验通过 | 24.1–28.8 s |
| `gate_guard` + `stress_cancel` 冒烟 | 2/2 + 6/6 | — |

`manual` 档（8 GiB）按维护者口径**只在正式发布前跑**，开发阶段与候选版本切分都不跑，已在
`docs/guides/release-policy.md` 写明。九组便宜分组未重跑（Windows CI 每提交覆盖）。

实测数据（四档快照/增量/账本、CPU 时间的对照示例、16K 发布的内存与分阶段耗时）见
`../../user/stress-test-report.md`；实现细节、量化限制与 C4 口径的完整诊断只留在本文件与
`../../guides/validation.md`。

## 4. C4 账本口径修正（测试侧，不是存储回归）

`stress_large` 的 C4 在 `heavy` 与 `extreme` 首轮失败：实测增量分别比预期多出 12,805,206 与
10,355,526 字节，超出 10% 容差。根因是账本预期仍按 v1 口径只算「delta + 工作文件新增区域」，
而 v1 的快照大小恒定（换 head 不额外占空间），v2 的快照会随新增的不同内容变大。超出的字节与
运行时算出的 `snapshotGrowthBytes`（12,797,014 / 10,355,526）对得上。

修正：预期增量加入 `snapshots` 的增长（`record_ledger` 的调用点计算
`usage.snapshots - snapshots_before`），并写进 `ledger.jsonl` 的 `snapshotGrowthBytes`。
修正后 `heavy` 实测 99,400,735 对预期 99,392,543（容差内）、`extreme` 完全相等；单跑 C4 分别
108.5 s / 274.4 s 通过。同一轮的 C1 账本四档都是实测 = 预期到字节级，C3/C6/C8 全部通过，
因此不是存储放大回归。

## 5. 文档

- `docs/guides/validation.md`：CPU 时间的口径、两个已知限制（15.625 ms 记账粒度导致的 50 ms
  门限、多线程阶段的 CPU 时间大于墙钟）、`LILITH_STRESS_PROFILE`、基线 `machine` 字段、严格门
  的灵敏度（2.5× 只抓 2 倍级退化）与「新场景必须多轮取最小」的口径。
- `docs/user/stress-test-report.md`（**面向用户**）：alpha1 四档大文件实测表、2026-10-19 复跑
  摘要、性能基准的 CPU 对照示例，以及边界表条目更新。用户报告只写「验证了什么、数值多少、
  边界在哪」——CPU 度量的实现、量化限制与 C4 口径诊断不写进该文件，分别留在本文件与
  `docs/guides/validation.md`。
- `docs/guides/release-policy.md`：发布前手动测试组补上 `stress_baseline`，并写明 `manual`
  档只在正式发布前运行。
- `docs/planning/todo.md` 与 `../current-handoff.md`：删掉已完成的采集项与度量缺口。
- `CHANGELOG.md` 的 `## Unreleased`：新增 CPU 度量、C4 口径与 P2/P8 多轮采样三条。

## 6. 遗留与边界

- `manual` 档按发布前口径执行（需 ≥ 40 GiB 空闲磁盘、约 2 小时）。
- 分阶段 CPU 成本（认证发布的渲染 / 编码 / 签名）仍是墙钟——测试侧只能读整进程 CPU 时间，
  要拆阶段得在产品侧加度量（本次未做）。
- 本轮跑 `stress_baseline` 时在 `D:\temp\Artworks` 留下的 `.lilith-write-lock.sqlite3`
  已清除；副本内 4 个 2026-10-08 的 `authenticity-preview-*` 目录是副本自带内容，未动。
