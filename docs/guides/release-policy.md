# 发行政策

## 版本与分支

- 版本使用语义化版本；`0.x` 阶段允许在次版本中调整尚未稳定的本地数据契约。每次 schema 升级都必须提供**向前迁移路径**：打开旧版本仓库时按迁移链自动升级，并拒绝高于当前 `SCHEMA_VERSION` 的仓库（不提供降级）。
- **分支模型**：`main` 固定为 0.2 发布线（停在 `v0.2.0`），只接收 0.2 线修正，不再合入 0.3 改动；`dev` 是 0.3 开发线，承载 0.3 的全部实现与切版本准备。0.3 到达发布点时，由维护者决定在 `dev` 直接打标签还是另建发布分支；无论哪种方式，标签必须指向包含该版本切分、文档同步且 Windows CI 通过的提交。
- 版本字段与 `CHANGELOG.md` 的改动在 `dev` 进行；`main` 上的 0.2 线改动仍须边界清晰、文档同步且 Windows CI 通过。
- 候选版本使用 `vX.Y.Z-rc.N` 标签；正式版本使用 `vX.Y.Z` 标签。
- 版本号**滞后**于工作：`package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 和 `src-tauri/tauri.conf.json` 五处版本字段始终等于最近一次切版本时的值，日常改动不改动它们。
- `CHANGELOG.md` 顶部维护唯一的 `## Unreleased` 暂存段：当前版本落定之后的全部改动都记入该段，不写回已带日期的版本小节。带日期的小节一经建立即冻结。
- 维护者决定切版本时的固定动作：把 `## Unreleased` 改名为 `## X.Y.Z - 日期`，并在顶部重新留出空的 `## Unreleased` 暂存段；同步上述五处版本字段；重跑 `npm run legal` 更新许可清单中的版本字样；同步 `README.md` 与 `docs/planning/` 中的版本引用；以 `node tools/release/verify-metadata.mjs` 验证一致性。
- **版本与 schema 的对应关系**由 `tools/release/verify-metadata.mjs` 中的显式版本→schema 登记表维护：切版本必须在同一提交为新版本登记映射，脚本拒绝未登记或与源码 `SCHEMA_VERSION` 不一致的组合。0.2.x 与 0.3 的 alpha1 / alpha2 对应 repository schema v4；v4 → v5 迁移由 0.3 alpha3 落地（alpha3 / alpha4 映射到 v5）；v5 → v6 迁移由 0.3 alpha5 落地（素材板笔迹与备份检查元数据三列，alpha6 沿用 v6），因此 0.3 有两次迁移。schema 升级先于切版本落库时，登记表里旧版本的映射会让 `verify-metadata` 失败——这是预期信号：切到新版本并登记映射后才算一致。
- 切版本不等于发布，也不触发任何门槛；是否打标签由维护者单独决定。一旦打上候选或正式标签，该版本即公开，必须通过「自动门槛」与「人工门槛」的全部项目。
- 有版本号但未打标签的版本是**过渡版本**（如 `0.2.0-alpha.1`、`0.2.0-alpha.4`、`0.2.0-rc.3`、当前的 `0.3.0-alpha.6`）：它在 `CHANGELOG.md` 中占有带日期的小节，但不是公开发布，不作为交付版本。
- 已发布的标签、安装包和版本身份不可复用或替换。标签之后的任何代码、schema、应用标识或签名声明变化都必须切到新版本并打新标签。
- 源码仓库可公开不等于安装包可正式发布。每个二进制产物必须能追溯到唯一、干净且不可变的标签。

## CHANGELOG 写法

`CHANGELOG.md` 是给人读的改动记录，不是格式检查表。写法按改动本身的分量走：

- **大改多写，小改一两句**：新增模块、改数据契约或迁移，写清做了什么、边界在哪、有什么代价；修一处空路径、改一句文案，讲清现象和结果就够。不要求每条都撑成同样长短的段落。
- `Added` / `Changed` / `Fixed` 只是粗分类，方便读者定位；同一次改动并成一条还是拆成几条，按读起来是否清楚决定，不为对齐格式强行拆分或拼接。
- 顺序按影响排，不按落地顺序：会挡住使用的修复、会影响旧仓库的迁移写在前面。
- 只写使用者能观察到的事实：新增能力、行为变化、修掉的现象。纯内部重构、测试辅助与度量细节除非是解释行为所必需，否则不写；确需记录就压成一句。
- 依赖版本升级与 CI 门槛调整只写包名、版本号、公告 ID 和一句结果；不展开公告细节、失败时间线和影响面分析。
- 该留的具体名词要留——命令名、设置项、表名、错误文案是查证入口；成串的字段清单、调用链和逐步骤实现过程不写，代码与模块文档里已经有。
- 切版本把 `Unreleased` 落成带日期小节时，做一遍**小结与压缩**：逐条整理该小节的表述，大改多写、
  小改一两句，只保留读者需要的事实。压缩**仅针对冗余内容**——重复表述、可合并的条目、读者用不上
  的实现细节——不为压缩而压缩：行为事实、命令名、设置项、边界与代价不删，条目读起来清楚就不必
  再改短。带日期的小节一经建立即冻结，写法调整只作用于 `Unreleased` 与之后的小节。

## 自动门槛

Windows CI 必须通过以下项目：

1. `npm ci` 锁定依赖安装；仓库 `.npmrc` 和锁文件固定使用 npm 官方 registry，审计与实际构建不得使用不同下载源；
2. `npm run build` 的 TypeScript 与 Vite 生产构建；
3. `npm test` 的前端纯逻辑与控制器竞态测试；
4. `cargo fmt --check --manifest-path src-tauri/Cargo.toml`；
5. `cargo test --manifest-path src-tauri/Cargo.toml --lib`。

CI 使用 Node 24。npm 生产与完整依赖审计固定访问官方 registry；Rust 依赖审计固定使用 `rustsec/audit-check@v2.0.0`，精确忽略一组固定告警，忽略列表在 `.github/workflows/windows-ci.yml` 与 `.github/workflows/release.yml` 中逐字一致（`RUSTSEC-2023-0071` 因 PS256 已在前后端禁用并有直接回归测试；zip 8.x 三项 `RUSTSEC-2026-0367/0368/0369` 因易受攻击路径在当前依赖图中不可达；`RUSTSEC-2026-0387` 的 `crypto-bigint 0.5.5` 位于 c2pa 的 ECDSA 依赖栈，0.5 线无修复版本，且本地桌面签名进程的计时侧信道暴露面有限，待上游 `c2pa_raw_crypto` 升级到 `crypto-bigint 0.7` 后移除；理由均见该列表旁注与 `docs/planning/todo.md`）；忽略列表之外的漏洞仍使作业失败。

正式候选版还必须通过：

6. 使用 npm 官方 registry 的生产与完整依赖漏洞审计；
7. RustSec 审计，并对无修复版本或仅影响非 Windows 目标的告警留下书面处置；
8. Windows 目标第三方许可清单、SBOM 和安装包资源完整性检查；
9. 从发布标签构建 NSIS，验证版本、schema、应用标识、校验和与标签一致；
10. 解包检查根许可证、第三方许可包和 Adobe TrustMark 模型许可证均存在。

推送候选或正式标签前，维护者还需在本机运行 `npm run ci:local` 并通过。它覆盖日常检查之外的审计、许可证一致性、发布元数据、平台分支检查与九组便宜压力测试，口径见 `docs/guides/validation.md` 的「本地 CI 预检」；该命令与昂贵压力测试同级，日常开发不调用。

依赖更新、Tauri 权限、schema、迁移、清理队列、路径校验、C2PA 或 TrustMark 变更必须单独说明风险和针对性测试。Actions、Node、Rust 工具链和 release workflow 应固定到受支持且可复现的版本；发布测试使用锁文件。

自动化压力测试（`src-tauri/tests/`；九组便宜的分组随 Windows CI 运行，`stress_large`/`stress_scale`/`stress_bench` 仍在发布前手动运行；其中 `stress_large` 的 `manual` 档（8 GiB）**只在正式发布前跑一次**，开发阶段与候选版本切分都不跑，需先把磁盘预算调到 40 GiB）接管了人工门槛中「使用正式支持上限附近的图片与工作文件验证内存、取消、退出和错误恢复」里可程序判定的部分：取消边界、跨进程强杀与事务中途崩溃、8 GiB 工作文件端到端、16K 大图发布内存、规模与参数边界、画板 DDS 完整性与损坏文件的检测与恢复、`.clip` 容器感知分块与 v1/v2 混存仓库的端到端恢复。界面视觉、交互手感与桌面集成仍由人工门槛覆盖。**断电持久性不做自动化验证**——测试用 `Child::kill()` 终止进程，不触及操作系统与磁盘的缓存、也不保证目录项落盘，该保证由操作系统、磁盘与 SQLite `synchronous = FULL` 声明承担。运行方式与实测见 `docs/guides/validation.md` 与 `docs/user/stress-test-report.md`。

## 人工门槛

每个公开候选版本必须在干净的 Windows 用户环境完成一次桌面验收：

- 安装、首次启动、仓库创建/打开、关闭到托盘与显式退出；
- Artwork 创建、树操作、回收站、分支、提交、创建分支、恢复、精简和检查点；
- 进入/取消发布、认证导出、再次导出、识别与跨 Artwork 溯源；
- 使用第三方工具回读 C2PA，并用项目随包模型验证 TrustMark；
- 确认取消发布保留首次导出 JPG，且仓库内副本、记录和保存配置已清除（副本在取消前被替换或已
  不存在时不删除、留在仓库里等垃圾扫描，见 `docs/modules/authenticity.md`）；
- 在副本上升级前一 schema（含旧设置），验证数据保留、`integrity_check`、`foreign_key_check` 和不可降级提示；
- 使用正式支持上限附近的图片与工作文件验证内存、取消、退出和错误恢复；
- 从上一公开候选版执行安装升级与卸载，确认仓库、配置目录和当前版本行为符合发布说明；旧仓库与旧设置的迁移纳入验收，旧应用标识的迁移不支持、不纳入（如涉及需另行说明）；
- 用普通用户账户验证安装包、主程序和时间戳签名，并确认法律文件可从安装目录或 About/Legal 页面取得；
- 验收环境需具备支持 WebGPU 与 BC7 纹理压缩的图形适配器（近年核显通常即可；虚拟机需开启 3D 加速并装好显卡驱动），因为素材板依赖它。没有可用适配器的环境（如未开启 3D 加速的虚拟机、部分远程桌面）仍可完成清单其余项目，但素材板只会显示一条常驻的不可用说明，该模块的验收必须另找具备可用适配器的环境补做。

人工结果先写入 `docs/planning/current-handoff.md`；该批次通过并归档后，其记录从该文件清出、移入
`docs/planning/archive/`，使交接文件只保留未解决事项。未经人工验收的构建只能标记为内部测试包。

## 产物与发布

- Windows 发行产物为 Tauri NSIS 安装包；版本必须与 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 一致。
- 发布说明由 `tools/release/write-release-notes.mjs` 从 `CHANGELOG.md` 的带日期版本段生成，并列出 schema 版本、已知限制和人工验收结果；`Unreleased` 不参与生成，因此打标签前必须先在切版本动作中把 `Unreleased` 落为带日期的小节。
- 公布安装包前记录 SHA-256。Authenticode 签名为可选项：配置了证书输入时安装包和主程序必须完成签名、可信时间戳和签名复核；未配置时允许未签名发布，但发布说明必须显著披露未签名状态及 SmartScreen 影响。
- 发布资产应同时包含校验和、目标依赖许可包、SBOM 和构建来源证明；release workflow 必须由标签触发并在干净 Windows 环境构建。
- `.github/workflows/release.yml` 校验标签、版本、标识和 schema，构建后静默安装 NSIS 并断言版本及法律资源，生成 CycloneDX SBOM、SHA-256 和 GitHub artifact attestation，最后创建草稿 release。缺少签名输入时走未签名构建，发布说明披露未签名；签名输入为 `WINDOWS_CERTIFICATE_BASE64`、`WINDOWS_CERTIFICATE_PASSWORD` 和 `WINDOWS_TIMESTAMP_URL`，三者必须同时提供。
- 不把私钥、证书、测试仓库、用户 Artwork 或未确认再分发许可的模型文件上传为发行资产。
- 项目贡献者创作的代码以 `LICENSE`（GPL-3.0-only）发布；第三方软件与 Adobe TrustMark 模型保留各自许可。分发完整应用时必须交付 `LICENSE`、目标专用第三方许可包和 `src-tauri/resources/models/LICENSE`。
- 正式版前必须启用并实测 GitHub 私密漏洞报告，或提供另一条已验证的私密报告渠道；公开 issue 只能用于请求建立私密联系，不得承载漏洞细节。
