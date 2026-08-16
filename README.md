# Lilith Artworks

Lilith Artworks 是一个本地优先的作品管理桌面应用，目标是统一管理 Artwork 树、可 fork 的增量历史、最终成品以及 C2PA/TrustMark 认证。

## 当前状态

`v0.1.0-rc.1` 的源码与 Windows 安装包已经公开，定位为供评估和问题发现使用的发布预览版，不是正式版，也不建议承载唯一副本或不可替代的作品数据。当前开发分支包含该标签之后的 schema、应用标识和行为调整；这些改动必须使用新的候选版本发布，不能继续复用 `rc.1` 的版本号或资产。

作品仓库、增量历史、认证发布/识别、恢复清理和跨模块工作流均已实现。正式版阻断项、风险分级和验收计划见[当前任务交接与人工验收](docs/planning/current-handoff.md)。当前功能包括：

- React + TypeScript + Tauri 2 工程骨架；
- 版本化设置、仓库选择、窗口状态和内容偏好；
- 默认关闭到托盘、托盘恢复窗口和显式退出；
- 作品仓库的 SQLite schema；
- Artwork、分支、历史、成品与认证记录的核心不变量测试。
- 可嵌套作品树、标题/工作文件搜索、创建、重命名、拖放排序和 Ctrl/Shift 多选。
- 项目回收站，支持恢复、永久删除和清空；Artwork 内部历史节点仍按规划直接裁剪。
- 支持 LilithClient ChunkFile v1 文件格式、内容定义分块、SHA-256、zstd 反向 delta 与完整性校验；当前测试阶段不承诺旧仓库数据迁移。
- 每个 Artwork 支持多分支、独立工作文件、主动提交、托盘自动调度、取消与历史恢复。
- 历史工作区显示树状 fork 结构、分支 head、节点标题、逻辑大小、Chunk 文件大小和 SHA-256。
- 作品树展开状态持久化，拖放沿用 LilithClient 的递归树实现；项目删除继续进入回收站。
- Tauri 图标与 TrustMark 模型已迁入 `src-tauri/resources/` 并随应用打包。

历史总览 mindmap、单分支历史、右键恢复与分支操作、当前分支精简模式、永久删除、中间节点 ChunkFile 重建与检查点均已接入；C2PA/TrustMark 认证支持发布、区域水印、识别与跨 Artwork 溯源。

## 文档入口

- [AI 阅读引导](docs/architecture/ai-reading-guide.md)
- [当前任务交接与人工验收](docs/planning/current-handoff.md)
- [规划归档](docs/planning/archive/README.md)
- [系统架构](docs/architecture/overview.md)
- [验证策略](docs/guides/validation.md)
- [发行政策](docs/guides/release-policy.md)
- [贡献指南](CONTRIBUTING.md)
- [安全策略](SECURITY.md)
- [变更日志](CHANGELOG.md)

验证命令和重依赖边界以 [验证策略](docs/guides/validation.md) 为准。Windows CI 运行前端生产构建与测试、Rust 格式检查和完整库测试；桌面 GUI、真实 C2PA 第三方回读与 TrustMark 实图检查仍由维护者人工完成。

## 许可证

本仓库中由项目贡献者创作的代码以 [GNU General Public License v3.0 only](LICENSE) 发布。分发完整应用时，需要同时满足 GPL-3.0-only 和随包第三方材料各自适用的许可与告知义务。

- **第三方软件**保留各自版权和许可条款；当前人工维护的摘要及正式版仍需补齐的许可清单见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
- 随包分发的 **Adobe TrustMark 模型**（`src-tauri/resources/models/`）沿用 Adobe 提供的 MIT License；原始许可文本见 [`src-tauri/resources/models/LICENSE`](src-tauri/resources/models/LICENSE)。
