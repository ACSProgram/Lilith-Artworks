# 发行政策

## 版本与分支

- 版本使用语义化版本；`0.x` 阶段允许在次版本中调整尚未稳定的本地数据契约，但必须提供显式 schema 迁移。
- `main` 只接收边界清晰、文档同步且 Windows CI 通过的变更。
- 候选版本使用 `vX.Y.Z-rc.N` 标签；正式版本使用 `vX.Y.Z` 标签。

## 自动门槛

Windows CI 必须通过以下项目：

1. `npm ci` 锁定依赖安装；
2. `npm run build` 的 TypeScript 与 Vite 生产构建；
3. `cargo fmt --check --manifest-path src-tauri/Cargo.toml`；
4. `cargo test --manifest-path src-tauri/Cargo.toml --lib`。

依赖更新、Tauri 权限、schema、清理队列、路径校验、C2PA 或 TrustMark 变更必须单独说明风险和针对性测试。

## 人工门槛

每个公开候选版本必须在干净的 Windows 用户环境完成一次桌面验收：

- 安装、首次启动、仓库创建/打开、关闭到托盘与显式退出；
- Artwork 创建、树操作、回收站、分支、提交、fork、恢复、精简和检查点；
- 进入/取消发布、认证导出、再次导出、识别与跨 Artwork 溯源；
- 使用第三方工具回读 C2PA，并用项目随包模型验证 TrustMark；
- 确认取消发布保留首次导出 JPG，且仓库内副本、记录和保存配置已清除；
- 升级前一 schema 的临时仓库并运行完整性检查。

人工结果写入 `docs/planning/current-handoff.md`。未经人工验收的构建只能标记为内部测试包。

## 产物与发布

- Windows 发行产物为 Tauri NSIS 安装包；版本必须与 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 一致。
- 发布说明从 `CHANGELOG.md` 生成，并列出 schema 版本、已知限制和人工验收结果。
- 公布安装包前记录 SHA-256；具备代码签名证书后，公开候选和正式安装包必须签名。
- 不把私钥、证书、测试仓库、用户 Artwork 或未确认再分发许可的模型文件上传为发行资产。
- 源码以 `LICENSE`（GPL-3.0）发布；第三方组件与 Adobe TrustMark 模型不并入 GPL，条款见 `THIRD_PARTY_NOTICES.md` 与 `src-tauri/resources/models/LICENSE`。
