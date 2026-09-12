# 当前任务交接

更新时间：2026-09-12

## 当前发布基线

`v0.1.0` **已发布**，使用 repository schema v1 和应用标识 `com.lilith.artworks`。发布标签与资产一经公开即不可移动、覆盖或复用；任何发布后的代码、schema 或签名声明变化都必须使用新的版本号和标签，并按 `docs/guides/release-policy.md` 走发布流程（发布说明由 `write-release-notes.mjs` 生成，未签名需在发布说明中披露）。

版本基线重置批次（应用版本与 schema 重置为 0.1.0 / v1、历史候选版与迁移链清除、维护者验收与发布确认）已归档到 `docs/planning/archive/version-reset-2026-09-12.md`。

## 下一阶段入口

- 开始 `0.1.x` 后续工作时，schema 变更从 v1 起按"`SCHEMA_VERSION` 递增 + 追加式迁移 + 新建仓库直接以当前版本落库 + `verify-metadata.mjs` 断言同步"执行，并在 `CHANGELOG.md` 写 Compatibility 条目。
- 素材板（pin-board）模块规划已确认未实施，见 `docs/planning/pin-board-plan.md`；其启动前提（`v0.1.0` 人工验收与发布闭环完成）已满足。
- 后续版本仍需覆盖的验收项：极端大文件与高像素压力、各处理阶段取消、异常 RFC 3161 服务、损坏文件恢复、普通用户安装和 Authenticode 签名，并记录可复查证据。
- 项目规则继续禁止代理执行 GUI 自动化；桌面交互、真实模型、第三方 C2PA 回读和安装包验收由维护者手工执行。
- 观察项：`repository/temp` 中存在认证预览遗留目录（约 40MB），可考虑在打开仓库时清理超期临时目录。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 和 `docs/guides/` 为准。已完成的批次写归档，不再保留在本文件；本文件只记录尚未完成的工作和人工验收结果。
