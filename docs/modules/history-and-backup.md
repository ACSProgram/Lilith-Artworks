# 历史与增量备份模块

## 上下文入口

按问题只读取一条路径：

- 页面状态、右键菜单、分支进入、精简选择与命令编排：`src/modules/history/HistoryModule.tsx`。
- 总览 mindmap 与左侧时间轴的纯展示：`src/modules/history/HistoryGraph.tsx`。
- 分支设置、保存状态、系统文件窗口和确认窗口：`src/modules/history/HistoryControls.tsx`，视觉规则只读 `src/styles/history.css`。
- 分支链、节点唯一归属和可精简资格：`src/modules/history/historyModel.ts`。
- DTO 和 Tauri 命令名：`src/modules/history/types.ts`、`src/modules/history/api.ts`。
- SQLite 历史图、分支和删除约束：`src-tauri/src/history/repository.rs`；不要为此加载 ChunkFile。
- snapshot/delta、恢复、检查点和精简：`src-tauri/src/backup/restore.rs`、`commands.rs`；只有块格式问题才进入 `chunk_file.rs`。
- 运行进度、取消和调度：`src-tauri/src/backup/runtime.rs`、`scheduler.rs`。
- 设置持久化与托盘：`src-tauri/src/app/settings.rs`、`src-tauri/src/lib.rs`。

当前未验收项和人工检查清单只读 `docs/planning/current-handoff.md`。

## 模块边界

- `src-tauri/src/library/`：仓库初始化、作品树、搜索与项目回收站。
- `src-tauri/src/history/`：分支、历史节点、fork、head 和历史元数据事务。
- `src-tauri/src/backup/`：原始 ChunkFile、提交、checkpoint、恢复、取消与托盘调度。
- `src-tauri/src/authenticity/`：认证聚合边界；内部的 `c2pa`、`trustmark`、`pipeline` 和 `repository` 已实现，详细契约见 `docs/modules/authenticity.md`。
- `src-tauri/src/storage.rs`：共享 SQLite 连接、ID、时间、路径与基本校验，不包含领域流程。

`backup` 通过 `history` 切换分支 head，不直接操作作品树；`history` 不读取 ChunkFile；C2PA 与 TrustMark 实现彼此独立，只由认证流水线编排。

## ChunkFile

`backup/chunk_file.rs` 完整迁移自 LilithClient `backup_agent/chunk_file.rs`。snapshot 使用内容定义分块，默认 min/avg/max 为 2 KiB / 16 KiB / 64 KiB，整文件与块摘要均为 SHA-256。delta 是从当前子 snapshot 还原父节点的反向增量，并使用 zstd level 6 包装。

fork 后同一父节点允许多个子节点，因此 schema v3 使用 `history_edges` 让每条 `child_history_id -> parent_history_id` 边分别拥有 delta 文件；不复用线性历史的单一后继假设。每个分支 head 保留完整 snapshot，旧 head 没有其他分支引用时才释放 snapshot。

删除 fork 分支时，文件候选同时收集该分支节点的 `snapshot_path`、旧兼容 `delta_path` 和 `history_edges.delta_path`。SQLite 事务提交后仍逐项查询当前图是否引用该路径，只删除已经无引用的 snapshot/delta，避免共享祖先或其它分支仍使用的文件被误删。

## 提交与恢复

提交顺序沿用 LilithClient：读取前后比较源文件元数据，临时生成 snapshot/delta，`sync_all`，以不覆盖方式发布文件，最后在 SQLite 事务中切换 head。数据库失败会清理本次新文件；相同 SHA-256 只更新时间，不创建节点。主动提交备注可为空并生成“主动提交”节点；调度器使用独立的 automatic 类型和空备注，不会覆盖主动提交备注。

恢复从目标节点向下寻找最近可用 snapshot，再沿父链应用反向 delta，最后以临时文件导出且禁止覆盖。fork 一个不再拥有 snapshot 的旧节点时，先物化并发布 checkpoint，保证新分支后续提交有稳定基线；建立 checkpoint 与创建分支属于同一个共享运行锁操作。

## 调度

每个分支保存独立开关与 1 到 10080 分钟的检查间隔。调度线程随 Tauri 应用启动，主窗口隐藏到托盘时继续运行；显式退出时先进入 `shutting_down`、请求取消，再等待调度线程和共享操作锁结束。已经排队但尚未取得锁的任务不会在退出过程中重新清除取消标志或开始备份。分支绑定最终成品后会自动从调度查询排除。分支起点、分支 head 和显式设置的节点会保留完整 snapshot 作为 checkpoint。

## 历史操作

历史总览是纵向缩进的父子 mindmap，使用工作区原生滚轮纵向浏览；分支视图列出当前 head 的祖先链。节点左键只选择或在精简模式中勾选；只有节点唯一属于一个分支时，右键菜单才提供进入分支。恢复使用系统“另存为”窗口选择新文件，并在页面头部报告可取消进度。

精简只允许在当前分支视图进入专用模式后，选择有一个子节点且不是叶节点、分支 head、fork 起点或检查点的普通中间节点。任务按分支链从后向前处理所选节点，重新物化父子节点，使用原始 ChunkFile API 重建新的反向 delta，再以事务同步改接 `history_nodes.parent_id`、`history_edges` 和兼容 `delta_path`，最后销毁旧节点和不再引用的文件。

删除节点仅从当前分支视角发起。预检在建立保留检查点之前拒绝包含发布节点的子树；若其它完整分支仍指向子树，也会拒绝并要求先删除对应分支。事务删除节点及后代并回退受影响分支，只更新 head 或 fork 起点确实落在删除集合中的分支，不改写同 Artwork 下无关旁支的 `updated_ms`。

检查点的建立与取消都需要二次确认并占用统一备份运行锁。建立时逐层报告回溯进度；取消普通检查点时，节点恢复使用唯一子节点到该节点的反向 delta，并把 UI 的当前存储路径/大小统计切回该 delta。分支 head、fork 起点、分叉点和已进入发布状态的节点是强制检查点，不能取消。

全局设置和分支设置使用开关表达自动备份状态。托盘菜单会根据持久化状态动态显示“暂停所有自动备份”或“继续所有自动备份”；分支设置自动保存并显示未保存、保存中、已保存和保存失败状态。

## 命令

```text
get_artwork_history
fork_artwork_branch
update_artwork_branch
run_branch_backup
restore_history_node
get_backup_runtime_status
cancel_backup_operation
rename_history_node
set_history_checkpoint
compact_history_node
delete_history_subtree
delete_artwork_branch
```

认证发布通过公开 `backup::ensure_checkpoint` 固化发布节点；history 不读取成品文件或认证 manifest。完整编译与 GUI 测试状态见当前交接文档。
## 历史图前端布局

- 总览 mindmap 使用可横向滚动的内容画布，支持“紧凑”和“时间轴”排列模式。模式开关位于“历史总览”标题行；同一行的滑条把节点最小宽度调整在 220px 到 420px，并通过 `lilith-artworks.history-node-min-width-v1` 持久化。标题与控制行在画布纵向滚动时保持吸顶；兄弟节点水平间隔收紧，减少无效横向占用。
- 紧凑模式把同一父节点的子节点横向平铺；时间轴模式按兄弟节点顺序逐列向右、逐级向下错位，并由共同的父级连接线下接，表达分支随时间依次出现的阶梯关系。时间轴不再使用节点时间戳生成任意空白。
- 时间轴模式在画布左侧按日期分组列出全部节点，日期内按时间倒序排列；条目使用“创建分支 - 节点标题”区分同名提交，创建分支不在当前 DTO 中时回退为节点标题。点击条目会选中、聚焦并把对应 mindmap 卡片滚动到画布中央，作为大图快速导航入口。
- 叶节点下显示其对应分支名称，帮助区分同一历史节点被多个分支引用的情况。
- 当前分支状态归 `src/app/ArtworkWorkspace.tsx` 所有，`HistoryModule` 通过受控属性读写，发布页复用同一状态；历史数据刷新只在当前分支失效时回退到第一个分支。
- 发布、进入发布或取消发布完成后，工作区递增历史刷新版本并立即重新读取历史 DTO，发布标签和计数不等待运行状态轮询。
- 节点卡片固定使用滑条给出的宽度并在所属子树内居中，父节点不会被多分支画布横向拉长；时间轴下沉节点的竖向连接线延伸到卡片顶部。
- 窄布局下页面标题、分支选择、提交区和精简工具栏分行排列，长 Artwork 标题使用省略显示；未保存状态使用主题化警告色。
- 全局设置快照统计当前未进回收站、未被成品锁定且启用自动备份的不同工作文件数；设置弹窗每次打开时刷新该统计。
