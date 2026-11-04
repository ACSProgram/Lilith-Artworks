# 2026-11-04 0.3.0-beta.2 切分与文档纪律整改

本批次处理三件事：把压力测试报告改回面向观众的写法、把「文档不堆历史」落成可执行的纪律、切分
`0.3.0-beta.2`。起因是维护者指出两条长期规则经常没有被遵守：压力测试报告重新堆满技术术语，表层
文档持续累积已完成的内容。

## 1. 压力测试报告改为面向观众

`docs/user/stress-test-report.md` 从 781 行重写为面向使用者的报告（约 100 行）：按根 `README.md`
的陈述式写法组织，不用第二人称；只保留「验证结果」表、五个文件档位的实测数字、明确不保证的
边界，以及复现入口。全文不再出现内部标识、阶段编号、检查点序号与实现机制。

技术内容按读者拆分，不删除证据：

- `docs/guides/stress-test-evidence.md`（新建）：逐组明细（A/B、C、D/E/F、G、H、I/J、K、R、M、
  解析器模糊、P）、性能基准、运行成本与「边界」详表。文件头注明各表的数据年代与「数字只用于同机
  同档比较」的口径。
- `docs/planning/archive/stress-test-rounds-2026-10-18-to-10-21.md`（新建）：alpha1 那轮的 8 条
  未通过项追因与检查点分层修复，以及 2026-10-18 / 10-19 / 10-21 三次复跑摘要。这些轮次描述的
  「当前行为」已被后续批次取代。
- 2026-10-25 的全量轮次（含 `manual` 8 GiB）留在证据文档，作为当前数据。

报告如实标注：beta.1 之后的改动没有再跑压力分组，下一次全量重跑安排在 0.3 正式版之前。

同步修改的引用：`docs/README.md`（用户文档描述 + 新增证据文档条目）、`docs/guides/validation.md`
（三处指向证据文档）、`docs/modules/authenticity.md`（256 px 阈值来源与覆盖表）、
`docs/modules/history-and-backup.md`（覆盖来源）、`docs/guides/release-policy.md`。

## 2. 文档纪律

`AGENTS.md` 的「文档与验证」与 `docs/guides/release-policy.md` 的「文档检查」现在写明三条，并给出
检查时机，避免再次只停留在原则层面：

- **用户文档面向观众**：`docs/user/` 与根 `README.md` 只写结论、关键数字与限制；技术明细进
  `docs/guides/`。每次更新后复查，不让明细回流。
- **表层文档零历史堆积**：`current-handoff.md` 与 `todo.md` 只写现在与未完成项；已完成批次、已通过
  验收、被替代的计划、已落地并被模块文档吸收的决策移入 `archive/` 并更新索引。检查时机明确为
  「每个批次结束、每次切版本、提交前」。
- **正式版前逐字复查**：切 `X.Y.0` 前逐字通读除 `archive/` 外的全部文档，删除失效的历史堆积，结论
  与删除项写进当次归档日志；候选版本只做与改动相关的同步。

## 3. 表层文档清理

`docs/planning/todo.md` 的「维护者决策记录」清出三条已落地或被模块文档吸收的决策：

- **快速命中计数改为自适应强制全量（2026-10-21）**：已实施，数值与实测依据在
  `docs/modules/history-and-backup.md` 的「调度」。
- **提交路径不再复检旧快照（2026-10-20）**：已实施，现行语义在
  `docs/modules/history-and-backup.md`；实测见 `backup-scheduling-fix-plan-2026-10-20.md` §6。
- **退出结算以 finalize / settlement 为准（2026-11-03）**：这条是验收口径，相关验收已完成并记入
  `handoff-log-2026-11-03.md` 第 9 节，不再需要在表层保留。

同时把「0.3 状态」段落压成当前事实，去掉 alpha1–alpha6 的逐版叙事。

## 4. 版本切分 0.3.0-beta.2

- `CHANGELOG.md` 的 `## Unreleased` 落为 `## 0.3.0-beta.2 - 2026-11-04`，顶部重新留出空的
  `## Unreleased`；落定前按发行政策做小结与压缩，把五条「内部整理、无可见变化」的条目合并为两条。
- 五处版本字段 `0.3.0-beta.1` → `0.3.0-beta.2`：`package.json`、`package-lock.json`（顶层与根
  包）、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json`。
- `tools/release/verify-metadata.mjs` 的版本→schema 登记表新增 `0.3.0-beta.2 → 6`（沿用 v6，0.3
  的两次迁移仍由 alpha3 与 alpha5 承担）。
- `npm run legal` 重生成 `licenses/THIRD_PARTY_LICENSES.html`，同步其中的版本字样。
- 版本引用同步：根 `README.md`、`docs/planning/current-handoff.md`、`docs/planning/todo.md`、
  0.4 草案的基线与版本锚点（原文「beta.1 都没打标签」与「beta.1 是最后一个版本」已不成立）。
- 切版本不等于发布：`v0.3.0-beta.2` 是否打标签由维护者单独决定。

切版本时发现一处回归：架构审查整改把 `src-tauri/src/library/schema.rs` 拆成
`library/schema/{mod,migrate,validate}.rs`，但 `tools/release/verify-metadata.mjs` 与
`tools/release/write-release-notes.mjs` 仍按旧路径读取 `SCHEMA_VERSION`，两个脚本都会在打标签时
失败——`verify-metadata` 正是本地 CI 与 Windows CI 的发布元数据门槛。本批次把两处路径改为
`src-tauri/src/library/schema/mod.rs`，验证输出：
`Release metadata verified: v0.3.0-beta.2, com.lilith.artworks, schema v6`，发布说明也能为
`0.3.0-beta.2` 正常生成。

## 5. 本地 CI 预检

`npm run ci:local` 在切版本后的工作区上跑完，21 项门槛全部 PASS，总耗时 360.9 s（`npm ci` 104.4 s、
Rust 测试目标预编译 185.1 s、headless 二进制 123.1 s 是大头，其余均为秒级）：workflow 审计
ignore 列表一致、平台分支只在 `platform/` 内、发布元数据、npm 生产与完整依赖审计、RustSec 审计、
`npm ci`、许可证生成与一致性、`gate_guard` 与九组便宜压力测试（cancel / crash / cleanup / damage /
format / pin_board / authenticity / concurrency / input）。脚本按设计提示「工作区不干净」——本次
验证的正是尚未提交的改动。

本轮没有提交，也没有打标签；`stress_large` / `stress_scale` / `stress_bench` 与 `manual` 档未运行，
按发行政策留到 0.3 正式版前。
