# AI 阅读引导

这是代码任务的第一入口。按下面选择一条路由，只读对应契约与实现文件；仅当需要跨模块调用或数据库 schema 契约时才读取相邻模块。

## 路由

| 问题 | 起点文档 | 实现位置 |
| --- | --- | --- |
| 作品树、搜索、拖放、回收站 | `docs/modules/library.md` | `src/modules/library/`、`src-tauri/src/library/` |
| 分支、提交、创建分支、恢复、精简、检查点、节点缩略图 | `docs/modules/history-and-backup.md` | `src/modules/history/`、`src-tauri/src/history/`、`src-tauri/src/backup/`、`src-tauri/src/preview/` |
| 发布、成品、C2PA、TrustMark、识别 | `docs/modules/authenticity.md` | `src/modules/authenticity/`、`src-tauri/src/authenticity/` |
| 素材板、画板、图片导入导出、画板回收站、笔迹、图层、整画布导出 | `docs/modules/pin-board.md` | `src/modules/pin-board/`、`src-tauri/src/pin_board/` |
| 设置、托盘、窗口生命周期 | `docs/architecture/overview.md` | `src-tauri/src/app/settings/`（`model`/`service`/`state`/`window`/`commands`）、`src/app/SettingsDialog.tsx`、`src/app/settings/`、`src-tauri/src/lib.rs` |
| 仓储空间状态（数据库记录口径、磁盘扫描） | `docs/modules/history-and-backup.md` 的「仓储空间状态」 | `src-tauri/src/storage_overview/`、`src/app/RepositoryStoragePanel.tsx` |
| 确认弹窗、滑动确认等无领域语义的共享 UI | `docs/architecture/overview.md` 的「前端共享控件」 | `src/shared/` |
| 构建、格式、静态检查 | `docs/guides/validation.md` | 只运行与改动匹配的检查 |

## 读序

1. 先读所选模块文档和 `docs/planning/current-handoff.md`（当前批次状态）；未完成事项查
   `docs/planning/todo.md`，历史批次记录见 `docs/planning/archive/`。
2. 再读前端 `types.ts`/`api.ts` 或 Rust `model.rs`/`commands.rs`，确认 DTO 与命令契约。
3. 最后读实现。数据库工作进入领域仓储；ChunkFile 格式只在必要时进入 `backup/chunk_file/`（`format.rs` 是字节布局，`store.rs` 是段读取，`snapshot.rs`/`delta.rs` 是写入与链解析），`.clip`/`.psd` 容器边界读 `format/container.rs`，节点缩略图抽取读 `preview/`。

## 边界

- 前端模块之间不互相导入；跨模块流程在 `src/app/` 编排。
- 跨模块的确认交互复用 `src/shared/ConfirmDialog.tsx`；不可逆的高风险操作置
  `slideToConfirm: true`，测试里驱动该控件用 `src/shared/slideToConfirmTesting.ts`。语义与
  共享控件清单见 `docs/architecture/overview.md` 的「前端共享控件」，不要另写第二套确认、
  拖拽或二段式点击控件。
- Tauri 命令是薄适配器：仓库入口统一经 `OperationCoordinator` 与 `CommandContext`，业务入口
  放在 `src-tauri/src/app/usecases/`，GUI 与 headless 共享；跨领域流程由 `app/workflows.rs`
  编排。分层见 `docs/architecture/overview.md` 的「命令执行底座」。
- `api.ts` 的调用方：读取、mutation 与运行状态由各自 controller 负责；素材板的窗口全屏、
  导出目录选择与渲染器纹理读写是直接调用它的无状态路径。controller 的请求代次、busy/错误上报
  与事件订阅分别来自 `useRequestEpoch.ts`、`useAsyncOperation.ts` 与
  `useRuntimeSubscription.ts`，不各自持有计数器或 `listen`。目录契约与各模块的分工见
  `docs/architecture/overview.md` 的「前端模块契约与事件消费」。
- `src/modules/<module>/api.ts` 是该领域 Tauri 命令的唯一前端入口。
- `history` 只管理 SQLite 图元数据，不读取 ChunkFile；`backup` 负责物化与调度。
- `preview` 只读传入的文件、返回派生缩略图，不写仓库文件也不进 ChunkFile；提交时来源是工作文件，手动刷新时由 backup 把节点内容物化到 `temp/` 再交给它；任何失败只让该节点没有预览。判断「某节点的格式有没有抽取器」走 `preview::supports_preview`，它只看分支工作文件的扩展名（节点自身存的是 ChunkFile 容器），只用于决定前端要不要给出预览占位。
- `pin_board` 直接注册 Tauri 命令（不经 `app/workflows.rs`）：浏览走 `run_read`，
  变更走 `run_repository_write`。画板不进入分支历史；持久化边界与 DDS 布局见
  `docs/modules/pin-board.md`。
- 认证发布检查点由 `src-tauri/src/app/usecases/authenticity.rs` 调用公开的
  `backup::ensure_checkpoint` 建立；`authenticity` 模块本身不导入 backup、不读取 `chunk_file/`。
- 破坏性历史操作经 `src-tauri/src/history/deletion_repository.rs` 进入；普通图操作经
  `repository.rs`，节点校验状态经 `verification_repository.rs`，调度候选与分支状态经
  `schedule_repository.rs`。
- 发布状态与 `final_artifacts` 绑定经 `src-tauri/src/authenticity/publication_repository.rs` 进入；
  认证配置与记录查询经 `repository.rs`。

## 变更规则

复用共享基础设施与既有 DTO。不要在领域仓储中重复数据库连接设置、路径归一化、ID 生成或错误
格式化。跨模块流程在应用命令层编排。入口点或契约变化时更新架构/模块文档；未完成工作写入
`docs/planning/todo.md`，当前批次状态写入 `docs/planning/current-handoff.md`。这两份是表层文档，
只保留当前状况、未完成事项与规划：已完成批次与已通过的验收搬入 `docs/planning/archive/`（并同步
其 `README.md` 索引）后从表层删除。
