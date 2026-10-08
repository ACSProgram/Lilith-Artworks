# Lilith Artworks

本地优先的平面美术个人项目工作台，统一管理三件事：作品资源（可嵌套的 Artwork 树与按作品的
素材板）、版本（可派生分支的增量历史）与发布（最终成品与 C2PA/TrustMark 认证）。四个领域
模块共享同一个作品仓库，全部数据留在本机。

## 当前状态

当前版本 `0.2.0-rc.1`，使用 repository schema v4 和应用标识 `com.lilith.artworks`。版本号
递增本身不伴随标签或发布，最新公开标签为 `v0.2.0-alpha.3`。已发布的标签与资产一经公开即
不可移动、覆盖或复用；任何发布后的代码、schema 或签名声明变化都必须使用新的版本号和标签。

四个领域模块及其跨模块工作流均已实现：

- **Artwork 树（Library）**：任意深度的分组与作品树、按标题或工作文件搜索、拖放排序、
  Ctrl/Shift 多选与项目回收站。
- **分支与增量历史（History/Backup）**：每个作品可有多分支与独立工作文件，支持主动提交、
  托盘自动调度、恢复、精简、检查点与整仓灾备。历史采用内容定义分块 + SHA-256 + zstd 反向
  delta，兼容 LilithClient ChunkFile v1。工作文件可留空，此时该分支不参与备份。
- **成品与认证（Authenticity）**：进入发布状态会锁定分支 head，导出强制签入 C2PA、可选嵌入
  TrustMark，并支持质量预览、识别与跨作品溯源。
- **素材板（Pin-board）**：按作品的多块画板、BC7 DDS 图片存储、WebGPU 画布与画板回收站，
  不进入分支历史。

最新一轮认证与识别模块的改动（质量预览像素级渲染、纯数值缩放、单条记录删除、识别页拖放
导入）已通过人工验收。本轮批次状态见[当前任务交接](docs/planning/current-handoff.md)。

## 使用声明

本项目由作者在个人生产环境中持续使用，并在代码层面尽量做到安全与可靠。但项目仍处于早期
阶段，可能存在缺陷、稳定性问题或安全缺口：作者**无法确保本软件在其它场景下正常运行，也不
对软件可能造成的数据损失承担任何责任**。请谨慎使用，并**务必做好数据备份**，不要将本软件
作为作品数据的唯一副本。

**数据兼容性：** 本版本不提供旧数据迁移支持。任何早于 schema v1 的作品仓库、设置和应用数据
均不受支持，请创建新仓库，不要直接打开、覆盖或复用旧版本数据。

**数据可靠性边界：** 「可程序判定」的部分——强制结束进程与崩溃、长操作取消、大工作文件
（已实测 64 MiB / 256 MiB / 1 GiB / 4 GiB）——已由自动压力测试覆盖，结论与实测数据见
[可靠性压力测试报告](docs/user/stress-test-report.md)。该测试**不覆盖**断电持久性、界面视觉
与交互手感，也不构成「无需备份」的理由。这套压力测试**只在发布前手动运行**（四档整轮约
40 分钟、峰值磁盘可达十几 GiB），日常开发与任何收尾阶段都不运行，约定见
[验证策略](docs/guides/validation.md)。

## 开发与构建

支持的环境为 Windows + Node.js 24 + 稳定版 Rust 工具链 + Microsoft C++ 生成工具 + WebView2。
用 `npm ci` 安装锁定依赖，用 `npm run tauri -- dev` 启动应用。完整约定见
[CONTRIBUTING.md](CONTRIBUTING.md)。

## 文档

文档按读者分层，完整索引见 [docs/README.md](docs/README.md)。

- 使用者：[可靠性压力测试报告](docs/user/stress-test-report.md)、[安全策略](SECURITY.md)、
  [第三方许可摘要](THIRD_PARTY_NOTICES.md)、[变更日志](CHANGELOG.md)。
- 开发者与代理：[AI 阅读引导](docs/architecture/ai-reading-guide.md)、
  [系统架构](docs/architecture/overview.md)、[模块文档](docs/README.md)、
  [验证策略](docs/guides/validation.md)、[发行政策](docs/guides/release-policy.md)、
  [当前任务交接](docs/planning/current-handoff.md)、[待办清单](docs/planning/todo.md)。

## 许可证

项目贡献者创作的代码以 [GNU General Public License v3.0 only](LICENSE) 发布。分发完整应用时，
需要同时满足 GPL-3.0-only 和随包第三方材料各自适用的许可与告知义务。

- 第三方软件保留各自版权和许可条款；人工维护的摘要与正式版仍需补齐的许可清单见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
- 随包分发的 **Adobe TrustMark 模型**（`src-tauri/resources/models/`）沿用 Adobe 提供的
  MIT License；原始许可文本见
  [`src-tauri/resources/models/LICENSE`](src-tauri/resources/models/LICENSE)。
