# 当前任务交接

更新时间：2026-08-13

当前结论：阶段 5“成品与认证整合”和阶段 6“全库识别”已实现，历史阶段已由用户完成完整编译验收。本轮前端构建与静态检查通过；按重依赖验证策略，没有运行完整 Rust 编译、ONNX/C2PA 流程或 GUI 自动化。下一步是用户执行完整编译和下面的人工工作流验收，发现问题后再按单一入口修复。

## 本轮范围

- 参考 `F:\programs\Proven` 迁移 C2PA、TrustMark Q/BCH-5、整图/局部区域、图片预览、私钥零持久化和双通道识别。
- 把认证流程融合到 Artwork/branch/history：分支进入发布状态、发布 head 强制检查点、仓库内最终成品、C2PA 导出记录和跨作品溯源跳转。
- C2PA 为强制基础，不能关闭；TrustMark 可关闭，模型不可用时自动降级为仅 C2PA。
- 历史总览从横向 mindmap 改为纵向缩进树，使用工作区原生滚轮纵向浏览，减少横向滑动和底部空白。
- schema 升级到 v5；同步架构、历史、Library、认证模块和实施计划文档。

不在本轮范围：解除发布状态、删除最终成品、修改已有导出记录、在线身份认证、完整 Rust/ONNX/C2PA 编译测试和 GUI 自动化。

## 代码入口

| 问题 | 首选入口 | 相邻契约 |
| --- | --- | --- |
| Artwork 页签与跨作品溯源 | `src/app/ArtworkWorkspace.tsx` | `src/modules/library/LibraryModule.tsx` |
| 发布、区域框选、识别和记录 UI | `src/modules/authenticity/AuthenticityModule.tsx` | `types.ts`, `api.ts`, `src/styles/authenticity.css` |
| 发布/识别流水线 | `src-tauri/src/authenticity/pipeline.rs` | `commands.rs`, `model.rs` |
| C2PA manifest | `src-tauri/src/authenticity/c2pa.rs` | Proven `c2pa_io.rs` |
| TrustMark 编解码 | `src-tauri/src/authenticity/trustmark.rs` | Proven `watermark.rs`, `state.rs` |
| 成品、配置、记录和匹配 | `src-tauri/src/authenticity/repository.rs` | `src-tauri/src/library/repository.rs` schema v5 |
| 发布检查点和分支锁 | `src-tauri/src/authenticity/commands.rs` | `src-tauri/src/backup/restore.rs`, `history/repository.rs` |
| 历史总览布局 | `src/styles/history.css` 的 `.mindmap-root` | `src/modules/history/HistoryModule.tsx` |

## 已实现待人工验收

- 分支没有 head 时拒绝发布；进入发布状态时先调用 `backup::ensure_checkpoint`，再复制成品并在事务中确认 head 未变化。
- 最终成品流式复制到仓库 `artifacts/<branch-id>/`，同步后以不覆盖方式发布，并保存 SHA-256、大小、媒体类型和发布节点 ID。
- `final_artifacts` 存在后，已有 history/backup 约束自动禁止继续提交、调度和删除分支；发布节点作为 checkpoint 保留。
- C2PA 每次强制签名，签入 Lilith 自定义声明、CreativeWork、ingredient 和 actions；TrustMark 可选。
- TrustMark 沿用 Proven 的 61 位 ID、Q/BCH-5、整图水印、最多 8 个额外范围和 0.50..1.50 强度。
- 签名输出先写临时文件，成功回读 C2PA 后才发布；记录落库失败会删除本次输出。私钥只存在于单次命令内。
- 导出记录保存时间、发布节点、ID、输出路径/大小/SHA-256、声明、区域、manifest 和验证状态，并支持字段搜索。
- 识别支持整图或框选区域，组合 C2PA 与 TrustMark 结果，按任一 ID 匹配本地记录；点击结果会切换 Artwork/分支并高亮记录。
- schema v5 强制最终成品关联发布节点；v4 预留认证记录表迁移时重建，已有最终成品保留并绑定分支当前 head。

## 人工验收顺序

1. 完整编译一次。若失败，记录完整命令、首个错误和文件；不要同时处理后续错误。
2. 新建测试仓库，提交一个分支 head，选择 PNG 最终成品进入发布状态；确认 head 显示检查点、分支显示成品锁定、自动提交和手动提交均不可用。
3. 使用有效测试证书和私钥发布一次“C2PA + TrustMark”，再关闭 TrustMark 发布一次“仅 C2PA”；确认两个 JPG 均可由 C2PA 工具读取，记录内容、大小和 SHA-256 正确。
4. 启用 TrustMark，在预览中添加/删除多个局部区域，确认最小区域和最多 8 个限制、全图基础水印和区域 soft-binding 坐标正确。
5. 分别用原导出图、去除 metadata 的图、裁剪图执行整图/框选识别；确认双通道一致、仅 TrustMark、仅 C2PA、无证据和冲突文案符合实际。
6. 从另一个 Artwork 的识别页点击匹配记录，确认左侧树展开并选择正确 Artwork，发布页选中正确分支且具体记录高亮。
7. 打开 v4 测试仓库触发迁移，确认 schema_version=5、历史/分支/成品仍可读取；迁移前应备份测试数据库，不直接用唯一生产仓库首测。
8. 用深分叉历史图检查纵向 mindmap：滚轮自然浏览，无持续横向滚动、底部大块空白或节点/连线重叠；再检查窄窗口和深色主题。

## 已通过检查

- `npm run build`
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo metadata --format-version 1 --no-deps`
- `cargo generate-lockfile --offline`（解析 682 个包；未编译）

在线生成锁文件曾因 Windows schannel `SEC_E_NO_CREDENTIALS` 失败，随后离线缓存解析成功并生成 `Cargo.lock`。这不是编译结果。

## 本轮架构整理（2026-08-13）

- 修复 `authenticity/repository.rs` 中配置加载的 `Result` 类型推断、查询映射临时借用，以及 `history/repository.rs` 中事务期间错误借用连接的编译问题。
- 将认证发布状态和 `final_artifacts` 原子绑定抽到 `src-tauri/src/authenticity/publication_repository.rs`；认证配置、记录写入和匹配查询仍在 `repository.rs`。
- 将历史破坏性操作收敛到 `src-tauri/src/history/deletion_repository.rs` 的命名能力面，`history/mod.rs` 不再通配符重导出仓储实现。
- `src-tauri/src/storage.rs` 现在是数据库连接配置、路径、时间、ID 和基础校验的共享入口；Library 删除了重复实现并复用它。
- 新增 `docs/architecture/ai-reading-guide.md`，规定按问题选择上下文入口、先读契约再读实现，并记录跨模块边界。

本轮只做了 `cargo fmt --all -- --check` 和 `git diff --check`。`cargo check --lib` 在当前环境超过两分钟未返回，未继续等待；完整编译、测试和 GUI 人工验收仍由用户执行。

## 尚未执行

- `cargo check`、`cargo test`、完整 Tauri 编译。
- C2PA 实际签名、manifest 第三方验证、TrustMark ONNX 编解码。
- schema v4 到 v5 的真实临时数据库迁移测试。
- GUI 人工作流、窄窗口、深色主题和大图性能检查。

这些项目由用户按上方顺序执行。若完整编译暴露接口错误，优先修复 `src-tauri/src/authenticity/`，不要进入 ChunkFile；只有发布检查点物化失败才读取 `backup/restore.rs`。
