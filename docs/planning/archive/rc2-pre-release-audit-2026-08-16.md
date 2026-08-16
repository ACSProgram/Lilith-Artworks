# RC2 发布前审查与历史验证归档

归档日期：2026-08-16

本文件归档 `current-handoff.md` 在 RC2 人工体验修复前累积的审查结论、完成清单与重复验证记录。当前工作状态只看 `docs/planning/current-handoff.md`。

## 审查结论

- `v0.1.0-rc.1`（`3ec1590`）已公开；其安装包未签名，保留原标签与资产，不复用或覆盖。
- 当前开发基线已统一为 `0.1.0-rc.2`、schema v9 和 `com.lilith.artworks`，只能使用新标签发布。
- P1 候选版代码、配置和自动测试清单已在 2026-08-16 清零。完成项包括整仓灾备、真实认证 fixture、资源预算、发布元数据容错、版本与许可闭包、依赖审计、SBOM、provenance 和标签 release workflow。
- UX-01/03/04/05/07 已完成代码与匹配自动回归：设置分组、作品树数量、识别区域关闭控件、分支工作文件修改回滚、总览精简切换提示。
- 维护者于 2026-08-16 确认常规非 UX 人工流程大部分完成，TrustMark/C2PA 发布、导出和识别已实际使用并正常。未执行内容集中在极端规模、极端时序、取消/退出延迟、时间戳异常和最终安装/标签产物。

## 已归档自动验证证据

- 前端曾多轮通过 `npm run build` 和完整 Vitest；RC2 加固结束时为 11 个测试文件、30 个用例，随后 UX 修复继续增加用例。
- Rust 定向验证曾覆盖 `backup::`、`cleanup::tests`、`library::repository::tests`、`history::repository::tests`、`app::settings::tests`、`storage::tests` 和 `authenticity::`。
- 真实认证 fixture 包含公开测试专用 ES256 凭据、128 x 128 源图、真实 C2PA/TrustMark 图片和篡改图片；生产回读按 fail closed 校验记录 ID、TrustMark ID、声明、文件替换和验证状态。
- npm 官方 registry 的生产与完整依赖审计均为 0 个已知漏洞。RustSec 的 `RUSTSEC-2023-0071` 来自 C2PA native crypto 的 RSA 链路；PS256 已在前后端禁用，CI 仅精确例外该公告。
- Windows 目标第三方许可闭包、release metadata 校验、release note 生成、PowerShell 脚本语法和 `git diff --check` 曾通过。
- 未归档为通过：全量 Rust、NSIS 构建、GitHub 标签 workflow、极端大文件/高像素压力、真实时间戳异常、Authenticode 和最终安装包验收。

## 延后到 0.1.0 的质量清单

以下审查项不阻断明确标为预览版、只支持新建仓库且不承载唯一副本的 RC2；进入 `0.1.0` 正式版阶段时重新激活并拆成可执行任务。

- 崩溃一致性：检查点/精简 cleanup intent、durable publish 屏障、orphan reconciliation、结构化部分成功删除结果。
- 授权与退出：其余恢复/工作文件/仓库 dialog scope、ChunkFile 分块取消、退出状态与有界收尾。
- 稳定读取与旧库选择：复制前后文件身份验证、多个旧数据库候选和图归属语义。
- 事务与设置兼容：`create_branch` 原子事务，高版本/无效设置只读保留与未知字段回归。
- 规模与查询：删除复杂度、历史/认证分页、图索引与迭代计算、列表虚拟化、10k/50k/100k 基线。
- 前端恢复：部分成功刷新、初始加载重试、顶层 ErrorBoundary、设置草稿竞态、证书授权状态。
- 可访问性：树键盘导航、区域选择键盘等价、对话框焦点管理、对比度、字号、IME 与菜单定位。
- 最终用户文档：安装/升级备份/schema 降级/卸载保留/更新渠道/支持范围，以及模型精确来源记录。

## 正式版外的长期事项

- 按领域边界拆分超大 repository/controller 文件。
- 评估模型资产的 Git LFS 或独立带摘要分发。
- 在签名和回滚模型稳定后设计自动更新。
- 建立长期性能趋势、依赖更新和定期 scrub 提醒。
