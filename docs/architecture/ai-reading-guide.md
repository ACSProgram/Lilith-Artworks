# AI 阅读引导

这是代码任务的第一入口。按下面选择一条路由，只读对应契约与实现文件；仅当需要跨模块调用或数据库 schema 契约时才读取相邻模块。

## 路由

| 问题 | 起点文档 | 实现位置 |
| --- | --- | --- |
| 作品树、搜索、拖放、回收站 | `docs/modules/library.md` | `src/modules/library/`、`src-tauri/src/library/` |
| 分支、提交、fork、恢复、精简、检查点 | `docs/modules/history-and-backup.md` | `src/modules/history/`、`src-tauri/src/history/`、`src-tauri/src/backup/` |
| 发布、成品、C2PA、TrustMark、识别 | `docs/modules/authenticity.md` | `src/modules/authenticity/`、`src-tauri/src/authenticity/` |
| 设置、托盘、窗口生命周期 | `docs/architecture/overview.md` | `src-tauri/src/app/`、`src-tauri/src/lib.rs` |
| 构建、格式、静态检查 | `docs/guides/validation.md` | 只运行与改动匹配的检查 |

## 读序

1. 先读所选模块文档和 `docs/planning/current-handoff.md`（当前任务交接；历史轮次完成记录见 `docs/planning/archive/`）。
2. 再读前端 `types.ts`/`api.ts` 或 Rust `model.rs`/`commands.rs`，确认 DTO 与命令契约。
3. 最后读实现。数据库工作进入领域仓储；文件格式工作只在必要时进入 `backup/chunk_file.rs`。

## 边界

- 前端模块之间不互相导入；跨模块流程在 `src/app/` 编排。
- `src/modules/<module>/api.ts` 是该领域 Tauri 命令的唯一前端入口。
- `src-tauri/src/storage.rs` 负责连接配置、路径、ID、时间与基础校验，不包含领域流程。
- `history` 只管理 SQLite 图元数据，不读取 ChunkFile；`backup` 负责物化与调度。
- `authenticity` 通过公开的 history/backup 能力建立检查点，不导入 library 内部。
- 破坏性历史操作经 `src-tauri/src/history/deletion_repository.rs` 进入；普通图操作经 `repository.rs`。
- 发布状态与 `final_artifacts` 绑定经 `src-tauri/src/authenticity/publication_repository.rs` 进入；认证配置与记录查询经 `repository.rs`。

## 变更规则

复用共享基础设施与既有 DTO。不要在领域仓储中重复数据库连接设置、路径归一化、ID 生成或错误格式化。跨模块流程在应用命令层编排。入口点或契约变化时更新架构/模块文档；未完成工作只写入 `docs/planning/current-handoff.md`。
