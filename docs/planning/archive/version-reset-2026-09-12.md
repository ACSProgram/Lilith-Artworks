# 版本基线重置与 0.1.0 发布（2026-09-12）

本文件归档 2026-09-12 版本基线重置批次的完整记录与验收结果。当前有效契约见 `docs/architecture/`、`docs/modules/` 与 `docs/guides/`，未完成项见 `../current-handoff.md`。

## 背景

应用此前处于 `0.1.0-rc.1` → `rc.2` → `rc.3` 的候选版序列，仓库 schema 随之演进到 v10，历史迁移链共 9 段。维护者决定将版本历史整体重置为不存在：应用版本定为 `0.1.0`，仓库 schema 定为 v1（表结构与重置前的 v10 完全相同），历史迁移函数全部删除，旧候选版标签与仓库数据不再受支持。**迁移机制本身保留**：此后的 schema 变更从 v1 起按"`SCHEMA_VERSION` 递增 + 追加式迁移 + 新建仓库直接以当前版本落库"执行。

## 改动清单

- 版本元数据统一落到 `0.1.0`：`package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json`。
- `src-tauri/src/library/schema.rs` 重写：`SCHEMA_VERSION = 1`，DDL 直接以 v1 落库，删除全部 `migrate_vN_to_vN+1` 函数与迁移测试；`validate_and_migrate` 更名为 `validate`，`repository.rs` 两处调用点同步更新。
- `tools/release/verify-metadata.mjs` 断言 schema v1；`tools/release/write-release-notes.mjs` 兼容性说明改写。
- `src-tauri/src/app/settings.rs` 就绪状态测试改用 `'1'` 作为合法版本值。
- 历史内容清除：`CHANGELOG.md` 重写为单一 `0.1.0` 条目；README、`docs/modules/library.md`、`docs/modules/history-and-backup.md`、`docs/modules/authenticity.md`、`docs/architecture/overview.md`、`docs/planning/pin-board-plan.md` 中的 schema v6–v10/rc 表述全部移除或改写。
- 前端测试修复：`App.repositorySwitch.test.tsx` 的设置页版本断言改为从 `package.json` 动态读取，不再硬编码版本字符串。
- 既有仓库 `F:\LilithData\Artworks`（原 v10，结构与 v1 相同）在维护者备份后，将 `repository_meta.schema_version` 手工降为 `1`。

## 验证与验收

- 代理侧：`cargo check --release --lib`、`cargo check --lib --tests`、`cargo fmt --check`、前端 `tsc`、`npm test`（47/47）、`npm run verify:release-metadata`（v0.1.0, schema v1）全部通过。
- 维护者：重建 git 仓库后完整编译与测试运行正常，人工验收通过。
- 维护者确认 `v0.1.0` 达到可发布状态并完成发布流程；发布标签与资产一经公开即不可移动、覆盖或复用。
