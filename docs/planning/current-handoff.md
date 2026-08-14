# 当前任务交接

更新时间：2026-08-14

当前结论：阶段 5"成品与认证整合"和阶段 6"全库识别"已实现，历史阶段已由用户完成完整编译验收。2026-08-14 完成多轮认证/历史 UI 打磨与认证记录持久化（schema v6、仓库内 JPG 副本、截图框选修复、源码审计），前端构建与静态检查通过；按重依赖验证策略，没有运行完整 Rust 编译、真实 ONNX/C2PA 流程或 GUI 自动化。下一步是用户执行完整编译和下方的人工工作流验收，发现问题后按单一入口修复。历史轮次的完成记录已归档到 `docs/planning/archive/`。

## 本轮范围（累计至 2026-08-14）

- 参考 `F:\programs\Proven` 迁移 C2PA、TrustMark Q/BCH_SUPER、局部区域、图片预览、私钥零持久化和双通道识别。
- 把认证流程融合到 Artwork/branch/history：分支进入发布状态、发布 head 强制检查点、仓库内最终成品、C2PA 导出记录和跨作品溯源跳转。
- C2PA 为强制基础，不能关闭；TrustMark 可关闭，模型不可用时自动降级为仅 C2PA。
- 历史总览从横向 mindmap 改为纵向缩进树，使用工作区原生滚轮纵向浏览；追加紧凑/时间轴模式、节点宽度滑条与叶节点分支标签。
- schema 升级到 v6：认证记录在首次导出之外原子保存仓库内 JPG 副本，查看和再次导出优先读取副本；v5 旧记录不自动迁移文件，原路径仍存在时继续兼容。
- 08-14 多轮 UI 打磨：表单对比度与统一控件尺寸、发布记录查看与再次导出、截图框选修复（ResizeObserver 覆盖实际显示矩形）、危险区二次确认、源码级静态审计修复。

不在本轮范围：修改已有导出记录、在线身份认证、完整 ONNX/C2PA 工作流测试和 GUI 自动化。解除发布状态及最终成品/导出文件清理已在本轮实现。

## 代码入口

| 问题 | 首选入口 | 相邻契约 |
| --- | --- | --- |
| Artwork 页签与跨作品溯源 | `src/app/ArtworkWorkspace.tsx` | `src/modules/library/LibraryModule.tsx` |
| 发布、区域框选、识别和记录 UI | `src/modules/authenticity/AuthenticityModule.tsx` | `types.ts`, `api.ts`, `src/styles/authenticity.css` |
| 发布/识别流水线 | `src-tauri/src/authenticity/pipeline.rs` | `commands.rs`, `model.rs` |
| C2PA manifest | `src-tauri/src/authenticity/c2pa.rs` | Proven `c2pa_io.rs` |
| TrustMark 编解码 | `src-tauri/src/authenticity/trustmark.rs` | Proven `watermark.rs`, `state.rs` |
| 成品、配置、记录和匹配 | `src-tauri/src/authenticity/repository.rs` | `src-tauri/src/library/repository.rs` schema v6 |
| 发布检查点和分支锁 | `src-tauri/src/authenticity/commands.rs` | `src-tauri/src/backup/restore.rs`, `history/repository.rs` |
| 历史总览布局 | `src/styles/history.css` 的 `.mindmap-root` | `src/modules/history/HistoryModule.tsx` |

## 已实现待人工验收

- 分支没有 head 时拒绝发布；进入发布状态时先调用 `backup::ensure_checkpoint`，再复制成品并在事务中确认 head 未变化。
- 最终成品流式复制到仓库 `artifacts/<branch-id>/`，同步后以不覆盖方式发布，并保存 SHA-256、大小、媒体类型和发布节点 ID。
- `final_artifacts` 存在后，已有 history/backup 约束自动禁止继续提交、调度和删除分支；发布节点作为 checkpoint 保留。
- C2PA 每次强制签名，签入 Lilith 自定义声明、CreativeWork、ingredient 和 actions；TrustMark 可选。
- TrustMark 使用 40 位 ID、Q/BCH_SUPER，只在最多 8 个用户框选区域内嵌入，不再叠加全图水印；没有框选区域时自动关闭 TrustMark。
- 签名输出先写临时文件，成功回读 C2PA 后才发布；记录落库失败会删除本次输出。私钥只存在于单次命令内。
- 导出记录保存时间、发布节点、ID、输出路径/大小/SHA-256、声明、区域、manifest 和验证状态，并支持字段搜索。
- 识别支持整图或框选区域，组合 C2PA 与 TrustMark 结果，按任一 ID 匹配本地记录；点击结果会切换 Artwork/分支并高亮记录。
- schema v6 认证记录在首次导出之外原子保存仓库内 JPG 副本；查看和再次导出优先读取副本，v5 旧记录原路径仍存在时继续兼容。
- 框选层只覆盖图片实际显示矩形（`ResizeObserver` 随舞台/窗口重算），留黑不参与归一化；发布操作完成后显式通知历史模块刷新，发布节点标签与计数不再等待轮询。
- 发布记录查看模式为与编辑页一致的左右分栏，提供"再次导出"；分支发布总删除入口为独立危险区并应用内二次确认。

## 人工验收顺序

1. 完整编译一次。若失败，记录完整命令、首个错误和文件；不要同时处理后续错误。
2. 新建测试仓库，提交一个分支 head，选择 PNG 最终成品进入发布状态；确认 head 显示检查点、分支显示成品锁定、自动提交和手动提交均不可用。
3. 使用有效测试证书和私钥发布一次"C2PA + TrustMark"，再关闭 TrustMark 发布一次"仅 C2PA"；确认两个 JPG 均可由 C2PA 工具读取，记录内容、大小和 SHA-256 正确。
4. 启用 TrustMark，在预览中添加/删除多个局部区域，确认最小区域和最多 8 个限制、区域之间互不覆盖、未框选时仅发布 C2PA、区域 soft-binding 坐标正确。
5. 分别用原导出图、去除 metadata 的图、裁剪图执行整图/框选识别；确认双通道一致、仅 TrustMark、仅 C2PA、无证据和冲突文案符合实际。
6. 从另一个 Artwork 的识别页点击匹配记录，确认左侧树展开并选择正确 Artwork，发布页选中正确分支且具体记录高亮。
7. 打开 v4/v5 测试仓库触发迁移，确认 schema_version=6、历史/分支/成品与旧认证记录仍可读取；迁移前应备份测试数据库，不直接用唯一生产仓库首测。
8. 用深分叉历史图检查纵向 mindmap：滚轮自然浏览，无持续横向滚动、底部大块空白或节点/连线重叠；再检查窄窗口和深色主题。

## 已通过检查

- `npm run build`（08-13 与 08-14 多轮均通过）
- `npx tsc --noEmit`（08-14）
- `cargo fmt --all -- --check` / `cargo fmt --manifest-path src-tauri/Cargo.toml --all`（08-13、08-14）
- `git diff --check`（各轮均通过，仅 LF/CRLF 工作区提示）
- `cargo metadata --format-version 1 --no-deps`（08-13）
- `cargo generate-lockfile --offline`（08-13，解析 682 个包；未编译）

在线生成锁文件曾因 Windows schannel `SEC_E_NO_CREDENTIALS` 失败，随后离线缓存解析成功并生成 `Cargo.lock`。这不是编译结果。

## 尚未执行

- `cargo check`、`cargo test`、完整 Tauri 编译。
- C2PA 实际签名、manifest 第三方验证、TrustMark ONNX 编解码。
- schema v4→v5→v6 的真实临时数据库迁移测试（含 v5 旧认证记录兼容）。
- GUI 人工作流、窄窗口、深色主题和大图性能检查。

若完整编译暴露接口错误，优先修复 `src-tauri/src/authenticity/`，不要进入 ChunkFile；只有发布检查点物化失败才读取 `backup/restore.rs`。

## 交接经验

### 环境与命令坑位

1. 所有命令从 `F:\programs\Lilith Artworks` 运行。PowerShell 路径包含空格，切换目录时使用 `Set-Location -LiteralPath 'F:\programs\Lilith Artworks'`，文件参数也优先使用 `-LiteralPath`。
2. Markdown 源文件是 UTF-8。PowerShell 读取中文必须显式加 `Get-Content -Encoding utf8`；否则输出会出现乱码，但文件本身不一定损坏。不要根据乱码输出重写文档。需要统一终端输出时先执行 `[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()` 和 `$OutputEncoding = [System.Text.UTF8Encoding]::new()`。
3. `npm run build` 可能在受限沙箱中因 Vite 启动 `esbuild` 子进程失败，典型错误为 `Error: spawn EPERM`。这属于沙箱进程权限问题，不是 TypeScript 或 Vite 代码错误；应请求沙箱外执行授权后原样重跑 `npm run build`。
4. 如果 `npm run build` 已进入 `tsc` 并输出 `TSxxxx` 文件行号，那才按代码错误处理；修复第一个错误后重新运行，不把它和 `spawn EPERM` 混为一谈。
5. `git diff --check` 可能输出"LF will be replaced by CRLF"的 warning，同时退出码仍为 0。这不是空白错误；不要为消除提示批量转换全仓库行尾。真正失败会报告 trailing whitespace 或 conflict marker 并返回非零。
6. `dist/` 会被 Vite 更新，但当前由 `.gitignore` 排除；验收以源码和命令退出码为准，不把构建产物加入提交。
7. 本项目已经引入 ONNX/C2PA 重依赖。按 `docs/guides/validation.md`，纯前端改动不运行全量 `cargo build/check/test`；只有发现 Rust 接口或真实签名流程问题时，才安排对应的针对性编译，避免把长时间依赖构建误当卡死。
8. GUI 验收使用可丢弃的测试仓库、测试证书和测试导出目录，尤其是"确认全部删除"步骤。不要用唯一生产仓库首测，也不要把真实私钥写入文档、日志或命令行；私钥只在应用密码输入框中提供。

### 下一轮开始前检查

```powershell
Set-Location -LiteralPath 'F:\programs\Lilith Artworks'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
$OutputEncoding = [System.Text.UTF8Encoding]::new()
npm run build
git diff --check
git status --short
```

预期：构建成功；`git diff --check` 退出码为 0；`git status --short` 只显示本轮预期源码和文档。若构建出现 `spawn EPERM`，按上面的沙箱规则授权后单独重跑，不先改代码。

### 下一轮 GUI 测试顺序与预期

1. 在有最终成品和至少一条导出记录的测试分支打开发布页，点击记录。预期：查看页与编辑页保持同样的左右分栏密度，图片、框选、字段、SHA-256 和 C2PA 报告可读；没有输入框或可编辑水印控件；"退出查看"返回原发布页。
2. 在发布页和识别页分别加载一张 12960 x 6480 图片。预期："文件名 / 分辨率 / 更换图片或锁定状态"只占一行紧凑标题高度，图片舞台占主要空间；窄窗口下标题省略而不与按钮重叠。
3. 在历史页切换到非首个分支，再进入发布页；随后在发布页切换分支并返回历史页。预期：两边始终显示同一当前分支，后台状态刷新和发布操作完成后不会跳回旧分支。
4. 分别在浅色、深色主题检查分支设置、主动提交、发布表单和识别搜索。预期：普通控件尺寸和边框一致，文字与背景有清晰对比；禁用控件能辨认内容且与可编辑态有区别，不再出现灰底灰字不可读。
5. 在历史总览拖动节点宽度滑条到最小、中间和最大值，切换页签并重启应用。预期：节点最小宽度即时变化，连接线保持对齐，刷新/重启后恢复最后值；控件位于"历史总览"标题旁。
6. 构造 A 为父节点、B/C/D 为按时间先后创建的三个子分支。预期：紧凑模式为 A 下方横排 B/C/D；时间轴为 B 在第一列、C 在第二列下沉一层、D 在第三列再下沉一层，父级横线与各列竖线连续，无节点或标签重叠。再用更深的嵌套分叉确认每一层重复同一规则。
7. 在已发布分支滚动到记录列表之后检查危险区。第一次点击预期只打开二次确认，不删除任何内容；取消后状态不变；再次打开并确认后，最终成品、全部认证记录、仓库副本及记录指向的导出文件被清理，分支解除锁定并可继续提交。只使用可丢弃测试仓库和测试导出文件执行确认步骤。
8. 在发布页确认 JPG 输出位于 TrustMark 之前；不框选时 TrustMark 开关不可启用，框选第一个有效区域后自动启用，清空后自动关闭。预期提示文案与实际行为一致，最多 8 个区域和最小区域限制仍生效。
9. 最后检查 680px 级窄窗口、常用桌面窗口、浅色/深色主题以及长文件名/长路径。预期：标题、按钮、滑条、分段控件和危险区无重叠，横向滚动只在历史画布内容确实超宽时出现。

人工测试发现问题后，下一轮继续在 `src/modules/authenticity/AuthenticityModule.tsx`、`src/styles/authenticity.css`、`src/app/ArtworkWorkspace.tsx`、`src/modules/history/HistoryModule.tsx` 和 `src/styles/history.css` 内修正；只有删除结果、签名结果或分支锁状态与上述预期不一致时才进入对应 Tauri 领域模块。

## 历史归档

2026-08-13 至 2026-08-14 各轮次的已完成代码与验证记录见 `docs/planning/archive/history-2026-08-13-2026-08-14.md`。
