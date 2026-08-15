# 当前任务交接

更新时间：2026-08-15

## 当前阶段：最终收尾

阶段 H-K 的界面与应用工作流已由用户完成完整编译和人工验收；除本轮列出的发布删除与认证记录跳转问题外，其他检查项均已验收通过。本轮不继续架构拆分。

## 本轮修复

- 取消发布只删除仓库拥有的最终成品和认证 JPG 副本，并在同一事务删除认证记录及该分支的 `certification_configs`；首次导出的 JPG 保留在原发布路径。
- 前端取消成功后同步清空本次私钥、TrustMark ID、结果状态和 localStorage 中的共享签名配置。
- schema v8 要求每条认证记录都有非空 `stored_path`；迁移时移除没有仓库副本的旧记录，记录 DTO 与只读界面删除 `contentStored`/“仓库内已保存认证 JPG”兼容字段。
- Library 到 Artwork 工作区的认证记录跳转新增单调导航代次。同 Artwork、跨 Artwork 以及重复点击同一记录都会切换到发布页、选择目标分支并重新定位记录；导航不再清空同 Artwork 已加载的分支列表。

## 自动验证

- `npm run build` 通过，TypeScript 与 Vite 生产构建完成。
- `cargo fmt --check --manifest-path src-tauri/Cargo.toml` 通过。
- 定向测试 `cancel_publication_keeps_external_outputs_and_resets_config` 通过，确认外部发布路径不进入清理队列，仓库文件、记录和配置按预期清理。
- 定向测试 `v8_requires_repository_copies_for_certification_records` 通过，确认 v7 到 v8 的副本约束迁移。
- `git diff --check` 通过，无空白错误或冲突标记。

## 公开候选收尾

- 新增 Windows CI：锁定 npm 依赖、前端生产构建、Rust 格式检查和完整库测试。
- 补齐 `CONTRIBUTING.md`、`SECURITY.md`、`CHANGELOG.md` 与 `docs/guides/release-policy.md`，README 已更新为公开候选前状态。
- 源码已由所有者选择并加入 GPL-3.0 `LICENSE`；Adobe TrustMark 模型保留独立 MIT 许可（`src-tauri/resources/models/LICENSE`）。

## 最后人工回归

只需针对本轮修复检查：

1. 取消发布后，首次导出的 JPG 仍存在；重新进入发布时内容配置、签名配置和 TrustMark ID 均为默认/空状态。
2. 从识别结果和记录搜索分别跳转同 Artwork、另一个 Artwork，并连续两次点击同一记录；发布页应始终显示目标分支且分支下拉不为空，目标记录滚动到可见位置并高亮。

通过这两项后可按 `docs/guides/release-policy.md` 创建 `v0.1.0-rc.1`。长期暂缓项仍为 operation ID/取消协议、dialog/tree 无障碍、大树/大图性能基线，以及历史临时文件维护命令；它们不阻塞当前候选版本。
