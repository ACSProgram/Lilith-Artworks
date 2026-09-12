# 当前任务交接

更新时间：2026-09-12

## 当前发布基线

`v0.1.0` **已发布**，使用 repository schema v1 和应用标识 `com.lilith.artworks`。发布标签与资产一经公开即不可移动、覆盖或复用；任何发布后的代码、schema 或签名声明变化都必须使用新的版本号和标签，并按 `docs/guides/release-policy.md` 走发布流程（发布说明由 `write-release-notes.mjs` 生成，未签名需在发布说明中披露）。

版本基线重置批次（应用版本与 schema 重置为 0.1.0 / v1、历史候选版与迁移链清除、维护者验收与发布确认）已归档到 `docs/planning/archive/version-reset-2026-09-12.md`。

## 素材板迁移（0.2.0-alpha.1，待人工验收）

素材板（pin-board）模块已按 `docs/planning/pin-board-plan.md` 迁入：schema v2（`pin_boards` / `pin_board_images` / `pin_board_history`，v1 追加式迁移）、`src-tauri/src/pin_board/` 持久化与 DDS 处理、`src/modules/pin-board/` 前端模块与 Artwork 工作区挂载（keep-mounted）、画板回收站与 `pending_file_cleanup` 接入、设置项（缓存等级/阵列间距）、模块文档与自动测试。应用版本已提升为 `0.2.0-alpha.1`，`verify-metadata.mjs` 的 schema 断言同步为 v2。

已验证：`npm test`（90 通过）、`cargo test --lib`（98 通过，含 1 个忽略项与素材板迁移用例）、`node tools/release/verify-metadata.mjs`。**完整编译与 GUI 手工验收尚未执行**，由维护者按模块文档“快速验证”一节进行（导入/导出、大图、缓存、回收站、灾备恢复后画板可用）。

仍待完成（对应规划 P4）：

- 整仓灾备复制 `boards/` 目录并入备份清单校验；仓库完整性扫描（scrub）覆盖画板 DDS 文件；
- 规划中的“导入导出/保存走互斥写锁”目前经 `with_ready_repository` 实现，但纹理读取等长任务与灾备调度并行的降级策略（缩略图兜底提示）未评估，实测卡顿不可接受时再处理；
- Lilith Client 侧旧实现保持原样，待本模块验收后由维护者决定是否从 Client 移除。

## 下一阶段入口

- `0.2.x` 后续工作继续遵循"`SCHEMA_VERSION` 递增 + 追加式迁移 + 新建仓库直接以当前版本落库 + `verify-metadata.mjs` 断言同步"，并在 `CHANGELOG.md` 写 Compatibility 条目。
- 后续版本仍需覆盖的验收项：极端大文件与高像素压力、各处理阶段取消、异常 RFC 3161 服务、损坏文件恢复、普通用户安装和 Authenticode 签名，并记录可复查证据。
- 项目规则继续禁止代理执行 GUI 自动化；桌面交互、真实模型、第三方 C2PA 回读和安装包验收由维护者手工执行。
- 观察项：`repository/temp` 中存在认证预览遗留目录（约 40MB），可考虑在打开仓库时清理超期临时目录。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 和 `docs/guides/` 为准。已完成的批次写归档，不再保留在本文件；本文件只记录尚未完成的工作和人工验收结果。
