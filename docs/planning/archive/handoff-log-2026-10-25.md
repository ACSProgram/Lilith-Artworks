# 2026-10-25 0.3-beta.1 发布前全量压力测试

被测版本按维护者口径记为 `0.3-beta.1`；五处版本字段仍是 `0.3.0-alpha.6`，尚未正式切版本，
repository schema v6。批次记录只描述本轮实测与文档更新，不改产品代码、不改版本号。

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
