# 发行政策

## 版本与分支

- 版本使用语义化版本；`0.x` 阶段允许在次版本中调整尚未稳定的本地数据契约。只有在项目声明支持旧数据时，才必须提供显式 schema 迁移；当前测试阶段不声明旧数据迁移支持。
- `main` 只接收边界清晰、文档同步且 Windows CI 通过的变更。
- 候选版本使用 `vX.Y.Z-rc.N` 标签；正式版本使用 `vX.Y.Z` 标签。
- 已发布的标签、安装包和版本身份不可复用或替换。标签后任何代码、schema、应用标识或签名声明变化都必须递增版本，并同步 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 和锁文件。
- 源码仓库可公开不等于安装包可正式发布。每个二进制产物必须能追溯到唯一、干净且不可变的标签。

## 自动门槛

Windows CI 必须通过以下项目：

1. `npm ci` 锁定依赖安装；仓库 `.npmrc` 和锁文件固定使用 npm 官方 registry，审计与实际构建不得使用不同下载源；
2. `npm run build` 的 TypeScript 与 Vite 生产构建；
3. `npm test` 的前端纯逻辑与控制器竞态测试；
4. `cargo fmt --check --manifest-path src-tauri/Cargo.toml`；
5. `cargo test --manifest-path src-tauri/Cargo.toml --lib`。

CI 使用 Node 24。npm 生产与完整依赖审计固定访问官方 registry；Rust 依赖审计固定使用 `rustsec/audit-check@v2.0.0`，仅精确忽略 `RUSTSEC-2023-0071`，因为 PS256 已在前后端禁用并有直接回归测试，其余漏洞仍使作业失败。

正式候选版还必须通过：

6. 使用 npm 官方 registry 的生产与完整依赖漏洞审计；
7. RustSec 审计，并对无修复版本或仅影响非 Windows 目标的告警留下书面处置；
8. Windows 目标第三方许可清单、SBOM 和安装包资源完整性检查；
9. 从发布标签构建 NSIS，验证版本、schema、应用标识、校验和与标签一致；
10. 解包检查根许可证、第三方许可包和 Adobe TrustMark 模型许可证均存在。

依赖更新、Tauri 权限、schema、迁移、清理队列、路径校验、C2PA 或 TrustMark 变更必须单独说明风险和针对性测试。Actions、Node、Rust 工具链和 release workflow 应固定到受支持且可复现的版本；发布测试使用锁文件。

## 人工门槛

每个公开候选版本必须在干净的 Windows 用户环境完成一次桌面验收：

- 安装、首次启动、仓库创建/打开、关闭到托盘与显式退出；
- Artwork 创建、树操作、回收站、分支、提交、fork、恢复、精简和检查点；
- 进入/取消发布、认证导出、再次导出、识别与跨 Artwork 溯源；
- 使用第三方工具回读 C2PA，并用项目随包模型验证 TrustMark；
- 确认取消发布保留首次导出 JPG，且仓库内副本、记录和保存配置已清除；
- 如果项目声明支持旧 schema，则在副本上升级前一 schema，验证数据保留、`integrity_check`、`foreign_key_check` 和不可降级提示；当前测试阶段不声明旧数据迁移支持，此项不作为当前候选版门槛，未来支持时另行公布；
- 使用正式支持上限附近的图片与工作文件验证内存、取消、退出和错误恢复；
- 从上一公开候选版执行安装升级与卸载，确认仓库、配置目录和当前版本行为符合发布说明；旧仓库、旧设置和旧应用标识迁移只有在项目明确声明支持时才纳入验收；
- 用普通用户账户验证安装包、主程序和时间戳签名，并确认法律文件可从安装目录或 About/Legal 页面取得。

人工结果写入 `docs/planning/current-handoff.md`。未经人工验收的构建只能标记为内部测试包。

## 产物与发布

- Windows 发行产物为 Tauri NSIS 安装包；版本必须与 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 一致。
- 发布说明由 `tools/release/write-release-notes.mjs` 从 `CHANGELOG.md` 的当前版本段生成，并列出 schema 版本、已知限制和人工验收结果；标签前必须把版本从 `Unreleased` 落为带日期的小节。
- 公布安装包前记录 SHA-256；正式版安装包和主程序必须完成 Authenticode 签名、可信时间戳和签名复核。候选版若暂时未签名，发布说明必须显著披露。
- 发布资产应同时包含校验和、目标依赖许可包、SBOM 和构建来源证明；release workflow 必须由标签触发并在干净 Windows 环境构建。
- `.github/workflows/release.yml` 校验标签、版本、标识和 schema，构建后静默安装 NSIS 并断言版本及法律资源，生成 CycloneDX SBOM、SHA-256 和 GitHub artifact attestation，最后创建草稿 release。候选版缺少签名输入时披露未签名，正式版缺少签名输入时直接失败。签名输入为 `WINDOWS_CERTIFICATE_BASE64`、`WINDOWS_CERTIFICATE_PASSWORD` 和 `WINDOWS_TIMESTAMP_URL`。
- 不把私钥、证书、测试仓库、用户 Artwork 或未确认再分发许可的模型文件上传为发行资产。
- 项目贡献者创作的代码以 `LICENSE`（GPL-3.0-only）发布；第三方软件与 Adobe TrustMark 模型保留各自许可。分发完整应用时必须交付 `LICENSE`、目标专用第三方许可包和 `src-tauri/resources/models/LICENSE`。
- 正式版前必须启用并实测 GitHub 私密漏洞报告，或提供另一条已验证的私密报告渠道；公开 issue 只能用于请求建立私密联系，不得承载漏洞细节。
