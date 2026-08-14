# Lilith Artworks

Lilith Artworks 是一个本地优先的作品管理桌面应用，目标是统一管理 Artwork 树、可 fork 的增量历史、最终成品以及 C2PA/TrustMark 认证。

## 当前状态

作品仓库、增量历史与认证（阶段 5/6）均已实现，等待完整编译与人工工作流验收。当前包含：

- React + TypeScript + Tauri 2 工程骨架；
- 版本化设置、仓库选择、窗口状态和内容偏好；
- 默认关闭到托盘、托盘恢复窗口和显式退出；
- 作品仓库的 SQLite schema；
- Artwork、分支、历史、成品与认证记录的核心不变量测试。
- 可嵌套作品树、标题/工作文件搜索、创建、重命名、拖放排序和 Ctrl/Shift 多选。
- 项目回收站，支持恢复、永久删除和清空；Artwork 内部历史节点仍按规划直接裁剪。
- 完整迁移 LilithClient ChunkFile v1、内容定义分块、SHA-256、zstd 反向 delta 与完整性校验。
- 每个 Artwork 支持多分支、独立工作文件、主动提交、托盘自动调度、取消与历史恢复。
- 历史工作区显示树状 fork 结构、分支 head、节点标题、逻辑大小、Chunk 文件大小和 SHA-256。
- 作品树展开状态持久化，拖放沿用 LilithClient 的递归树实现；项目删除继续进入回收站。
- Tauri 图标与 TrustMark 模型已迁入 `src-tauri/resources/` 并随应用打包。

历史总览 mindmap、单分支历史、右键恢复与分支操作、当前分支精简模式、永久删除、中间节点 ChunkFile 重建与检查点均已接入；C2PA/TrustMark 认证（发布、区域水印、识别与溯源）已实现，待完整编译与人工验收。

## 文档入口

- [AI 阅读引导](docs/architecture/ai-reading-guide.md)
- [分阶段实施计划](docs/planning/implementation-plan.md)
- [当前任务交接与人工验收](docs/planning/current-handoff.md)
- [系统架构](docs/architecture/overview.md)
- [验证策略](docs/guides/validation.md)

项目严格禁止代理执行 GUI 自动化和截图测试。引入 ONNX/C2PA 后也不执行全量 Rust 编译，具体规则以验证策略为准。

## 基础验证

```powershell
npm ci
npm run build
cd src-tauri
cargo fmt -- --check
cargo check
cargo test --quiet backup::worker::tests::commits_delta_and_restores_parent_bytes
```

不要由自动化流程创建 Git 提交；每个阶段由维护者验收后自行提交。
