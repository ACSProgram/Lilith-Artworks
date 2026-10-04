# Lilith Artworks

Lilith Artworks 是一个本地优先的**平面美术个人项目**桌面应用，统一管理三件事：作品资源
（可嵌套 Artwork 树与按作品的素材板）、版本（可派生分支的增量历史）与发布（最终成品以及
C2PA/TrustMark 认证）。四个领域模块共享同一个作品仓库，全部数据留在本机。

## 当前状态

`v0.1.0` 已发布；当前 `0.2.0-alpha.3` 为测试版，使用 repository schema v3 和应用标识 `com.lilith.artworks`（旧仓库打开时按追加式迁移升至当前 schema）。项目仍处于早期阶段，可能存在缺陷、稳定性问题或安全缺口；不要用它承载唯一副本、不可替代的作品数据或生产工作流。已发布的标签与资产一经公开即不可移动、覆盖或复用；任何发布后的代码、schema 或签名声明变化都必须使用新的版本号和标签。

**数据兼容性警告：** 本版本不提供旧数据迁移支持。任何早于 schema v1 的作品仓库、设置和应用数据均不受支持，请创建新仓库，不要直接打开、覆盖或复用旧版本数据。

作品仓库、素材板、增量历史、认证发布/识别、恢复清理和跨模块工作流均已实现。当前功能包括：

- React + TypeScript + Tauri 2 工程骨架；
- 版本化设置、仓库选择、窗口状态和内容偏好；
- 默认关闭到托盘、托盘恢复窗口和显式退出；
- 作品仓库的 SQLite schema；
- Artwork、分支、历史、成品与认证记录的核心不变量测试。
- 可嵌套作品树、标题/工作文件搜索、创建、重命名、拖放排序和 Ctrl/Shift 多选。
- 项目回收站，支持恢复、永久删除和清空；Artwork 内部历史节点仍按规划直接裁剪。
- 支持 LilithClient ChunkFile v1 文件格式、内容定义分块、SHA-256、zstd 反向 delta 与完整性校验。
- 每个 Artwork 支持多分支、独立工作文件、主动提交、托盘自动调度、取消与历史恢复；工作文件可留空或事后清除，此时该分支的自动备份与主动提交保持关闭。
- 历史工作区显示树状分支结构、分支 head、节点标题、逻辑大小、Chunk 文件大小和 SHA-256。
- 作品树展开状态持久化，拖放沿用 LilithClient 的递归树实现；项目删除继续进入回收站。
- 素材板模块（自 Lilith Client 迁入）：按 Artwork 的画板画布、BC7 DDS 图片存储、持久化撤销/恢复历史与画板回收站（详见[模块文档](docs/modules/pin-board.md)）。
- Tauri 图标与 TrustMark 模型已迁入 `src-tauri/resources/` 并随应用打包。

历史总览 mindmap、单分支历史、右键恢复与分支操作、当前分支精简模式、永久删除、中间节点 ChunkFile 重建与检查点均已接入；C2PA/TrustMark 认证支持发布、区域水印、识别与跨 Artwork 溯源。

素材板迁入后已完成一次完整编译与 GUI 验收（含导入/导出、大图、缓存、回收站以及整仓灾备与灾备恢复后画板可用）。本轮批次的执行状态见[当前任务交接](docs/planning/current-handoff.md)，未完成事项见[待办清单](docs/planning/todo.md)。

## 使用声明

本项目由作者在个人实际生产环境中长期使用（日常持续使用，发现问题会尽快修复），并在代码
层面尽量做到安全与可靠。但作者**无法确保本软件在其它场景下正常运行，也不对软件可能造成
的数据损失承担任何责任**。请谨慎使用，并**务必做好数据备份**，不要将本软件作为作品数据
的唯一副本。

## 文档入口

- [AI 阅读引导](docs/architecture/ai-reading-guide.md)
- [系统架构](docs/architecture/overview.md)
- [当前任务交接](docs/planning/current-handoff.md)
- [待办清单](docs/planning/todo.md)
- [规划归档](docs/planning/archive/README.md)
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
