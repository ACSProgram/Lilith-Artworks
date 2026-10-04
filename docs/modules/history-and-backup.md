# 历史与增量备份模块

## 上下文入口

按问题只读取一条路径：

- 页面选择状态、右键菜单、分支进入与精简选择：`src/modules/history/HistoryModule.tsx`；历史读取、运行状态和命令编排：`src/modules/history/useHistoryController.ts`。
- 总览 mindmap 与左侧时间轴的纯展示：`src/modules/history/HistoryGraph.tsx`。
- 分支设置、保存状态、系统文件窗口和确认窗口：`src/modules/history/HistoryControls.tsx`，视觉规则只读 `src/styles/history.css`。
- 分支链、节点唯一归属和可精简资格：`src/modules/history/historyModel.ts`。
- DTO 和 Tauri 命令名：`src/modules/history/types.ts`、`src/modules/history/api.ts`。
- 创建分支、分支设置和分支删除的 Tauri 应用编排：`src-tauri/src/app/workflows.rs`；History 领域命令只保留历史读取和节点重命名。
- SQLite 历史图、分支和删除约束：`src-tauri/src/history/repository.rs`；不要为此加载 ChunkFile。
- snapshot/delta、恢复、检查点和精简：`src-tauri/src/backup/restore.rs`、`commands.rs`；只有块格式问题才进入 `chunk_file.rs`。
- 运行进度、取消和调度：`src-tauri/src/backup/runtime.rs`、`scheduler.rs`。
- 设置持久化与托盘：`src-tauri/src/app/settings.rs`、`src-tauri/src/lib.rs`。

当前批次状态见 `docs/planning/current-handoff.md`，未完成事项见 `docs/planning/todo.md`。

## 模块边界

- `src-tauri/src/library/`：仓库初始化、作品树、搜索与项目回收站。
- `src-tauri/src/history/`：分支、历史节点、创建分支、head 和历史元数据事务。
- `src-tauri/src/backup/`：原始 ChunkFile、提交、checkpoint、恢复、取消与托盘调度。
- `src-tauri/src/authenticity/`：认证聚合边界；内部的 `c2pa`、`trustmark`、`pipeline` 和 `repository` 已实现，详细契约见 `docs/modules/authenticity.md`。
- `src-tauri/src/storage.rs`：共享 SQLite 连接、ID、时间、路径与基本校验，不包含领域流程。

`backup` 通过 `history` 切换分支 head，不直接操作作品树；`history` 不读取 ChunkFile；C2PA 与 TrustMark 实现彼此独立，只由认证流水线编排。

## ChunkFile

`backup/chunk_file.rs` 完整迁移自 LilithClient `backup_agent/chunk_file.rs`。snapshot 使用内容定义分块，默认 min/avg/max 为 2 KiB / 16 KiB / 64 KiB，整文件与块摘要均为 SHA-256。delta 是从当前子 snapshot 还原父节点的反向增量，并使用 zstd level 6 包装。

delta 打开时不再把压缩体和完整解压体同时读入内存。zstd 或旧版未压缩 payload 都流式写入仓库 `temp/` 中的自动删除文件；允许的解压字节数由 delta 头声明的目标逻辑大小、数据记录数和操作数推导，超出声明范围按格式错误拒绝。该边界随真实工作文件大小增长，不对 PSD 或其它工作文件设置固定总字节上限；4 GiB 目标的回归测试用于防止安全校验误伤正常大文件。恢复应用阶段只在内存中保存块索引和操作元数据，块内容从临时 payload 或基础 snapshot 流式复制并逐块验哈希。

`ChunkStore` 把基础 snapshot 与各条 delta 的解压 payload 追加为同一段虚拟偏移空间中的分段。`ChunkFileDelta::resolve` 只重写块布局、不写 payload 字节；`write_snapshot`、`copy_original` 在导出或校验时才按虚拟偏移流式读取并逐块验哈希。因此物化一条链的成本取决于链上变化的字节数，而不是"链长 × 文件大小"。`ChunkFileDelta::apply`（物化成完整 snapshot 文件）由 resolve + write_snapshot 组合实现，chunk 格式单测在无 GUI 依赖的隔离 crate 中运行。

创建分支后同一父节点允许多个子节点，因此 `history_edges` 让每条 `child_history_id -> parent_history_id` 边分别拥有 delta 文件；不复用线性历史的单一后继假设。每个分支 head 保留完整 snapshot，旧 head 没有其他分支引用时才释放 snapshot。

删除分支时，文件候选同时收集该分支节点的 `snapshot_path`、旧兼容 `delta_path` 和 `history_edges.delta_path`。SQLite 事务提交后仍逐项查询当前图是否引用该路径，只删除已经无引用的 snapshot/delta，避免共享祖先或其它分支仍使用的文件被误删。

## 存储大小语义

`history_nodes.chunk_file_size` 记录"重建该节点需要读取的字节数"：节点拥有 snapshot 时为 snapshot 文件大小（由发布它的调用方写入），否则为指向其唯一子节点的边 delta 大小；多个子节点时取最小 delta（分支起点通常持有 snapshot，不会走到该分支）。`history_nodes.delta_path` 兼容列与该语义保持一致，仅在没有边（分支根）时被 `load_node_from` 读取。写路径统一由 `refresh_storage_metadata` 从图推导：提交释放父 snapshot、取消检查点、精简改接后都会重算，精简后刷新的是被移除节点的父节点（新 delta 的受益方）与子节点。

## 提交与恢复

提交顺序沿用 LilithClient：读取前后比较源文件元数据，临时生成 snapshot/delta，`sync_all`，以不覆盖方式发布文件，最后在 SQLite 事务中切换 head。数据库失败会清理本次新文件。工作文件与 head 的 SHA-256 相同时，worker 仍会打开受控 snapshot、核对数据库摘要并逐块验证内容；文件缺失、格式损坏、块损坏或摘要不匹配时，使用本次已经生成的 snapshot 以新路径发布并更新原 head 节点，验证通过后才把任务记为内容未变化。有效 snapshot 不重写，修复也不创建新历史节点。主动提交备注可为空并生成“主动提交”节点；调度器使用独立的 automatic 类型和空备注，不会覆盖主动提交备注。

恢复从目标节点向下寻找最近可用 snapshot，再沿父链逐条 `resolve` 反向 delta（惰性，不产生中间 snapshot 文件），最后一次性把结果流式导出为临时文件并禁止覆盖；链起点与每步结果都会与数据库摘要比对，导出时逐块验哈希并核对整文件摘要。恢复、精简和检查点共用同一条惰性解析路径：精简只物化被移除节点的父链，子链只取块索引，然后从父链的虚拟存储中按需读取变化的块重建反向 delta；检查点把解析结果一次性写成 snapshot 文件并 `sync_all` 后发布；全库 scrub 走同一条链但把结果流入 sink，只校验不落盘。所有长操作经 `BackupState::run_logged` 记录开始、结束与耗时。

设置页提供可取消的全库完整性扫描。扫描在共享运行锁和仓库 lease 内逐个物化全部历史节点，因此同时覆盖 snapshot 缺失、格式/块损坏、delta 损坏以及数据库摘要不匹配。设置弹窗打开或操作运行期间由应用层轮询共享备份运行状态，显示当前阶段、确定/不确定进度和统一取消入口；状态预检会阻止把新的设置页长任务排在已有备份操作之后。

设置页也提供在线整仓备份，用户可见命名统一为“创建备份”。命令在共享运行锁和仓库 lease 内先 checkpoint SQLite WAL，再按扫描、复制、校验、发布阶段把数据库和仓库内全部普通文件逐块复制到同一目标卷的临时 bundle；符号链接、仓库内部输出目录和复制期间变化的文件会被拒绝。副本会独立执行仓库语义校验、历史链 scrub 和受控发布文件 scrub，随后生成逐文件 SHA-256 `manifest.json`，复核清单后才以目录重命名发布；取消或失败会显式清理未发布的临时 bundle，并把清理成功或失败附加到返回错误。bundle 内的 `repository/` 是可直接打开的恢复副本，`manifest.json` 位于其外层。分支工作文件是仓库外部输入，不属于整仓备份；恢复后仍需保证相应外部工作文件可用，或为分支重新选择工作文件。

## 调度

每个分支保存独立开关与 1 到 10080 分钟的检查间隔。调度线程随 Tauri 应用启动，主窗口隐藏到托盘时继续运行；显式退出时先进入 `shutting_down`、请求取消，再等待调度线程和共享操作锁结束。已经排队但尚未取得锁的任务不会在退出过程中重新清除取消标志或开始备份。分支绑定最终成品后会自动从调度查询排除。分支起点、分支 head 和显式设置的节点会保留完整 snapshot 作为 checkpoint。

自动备份支持快速检查与全量检查两种方式。全局设置 `automaticBackupCheckMode`（`quick` / `full`，默认 `quick`）决定默认方式；分支级 `backup_quick_enabled` 单独开启后，即使全局是全量也对该分支使用快速检查。快速检查只比较工作文件的大小与修改时间是否与上次全量检查成功后记录的基线（`last_source_size` / `last_source_modified_ms`，由 `run_backup` 在提交或确认内容未变化时写入）完全一致：一致时直接记为内容未变化并推进检查时间，不打开工作文件；缺少基线（如尚无任何提交或旧数据未迁移出基线）、任一元数据不一致或读取失败时，退回完整流程。手动提交始终走全量检查。快速检查不校验 head snapshot 的完整性，该职责仍属于内容变化后的全量路径与全库完整性扫描。

手动提交优先于自动备份：手动提交命令在取得共享运行锁前先登记该分支的待处理标记；调度器在候选选择和取得运行锁后两处都会跳过有待处理标记的分支（延后且不计入失败）。若同分支的自动备份已在运行，手动提交会对后台任务请求取消，使自动任务以取消结果退出（不进入失败退避），随后手动提交先执行；自动备份延后到下一次调度周期，此时内容已被手动提交覆盖，通常得到"内容未变化"的结果。

共享运行状态 `BackupRuntimeStatus` 携带 `taskKind`，取值为 `automaticBackup` / `idleVerify` / `userOperation`（`BackupTaskKind`），与 `busy`、`activeBranchId` 在同一临界区内写入并在同一次重置中清除；前端据此区分后台低优先级任务与用户触发的关键操作。取消分两个入口：`request_cancel` 取消任意任务，供用户主动取消；`cancel_background` 只在当前任务是后台任务（自动备份或空闲校验）时置位，不会误取消用户自己的操作。所有前台长命令（恢复、精简、检查点、删除子树、进入发布、发布签名、全库扫描、整仓备份、仓库切换）在取得运行锁之前调用一次 `cancel_background`，让正在运行的后台任务尽快退出，并登记前台等待计数 `foreground_waiting`；调度器在候选选择阶段跳过新任务，并在取得运行锁之后复查该计数，大于零即以让位结果退出、不放行本次后台任务，随后进入退避等待，避免与前台争抢运行锁。计数由 RAII 守卫在取得锁之后自减，取消、失败、退出和提前返回等所有路径都会归零。该让位与手动提交的 `manual_pending` 是同一套"锁内复查后让位"模式：仅置位取消标志并不可靠，因为共享取消标志会在任务取得运行锁后被清零，调度器可能抢先取锁并吞掉前台意图，前台等待登记消除了这种抢占。

自动备份成功或确认内容未变化时才推进 `last_check_ms`/`last_success_ms`，同时清零连续失败计数。自动任务失败会记录 `last_error` 并按分支间隔退避：首次失败 1 分钟后重试，随后分别等待原间隔的 1/4、1/2 和完整间隔（向上取整）；第 5 次连续失败时持久化关闭该分支自动备份并生成待确认通知。手动重新启用分支会清零失败状态和该通知；成功备份也会清零计数。系统时间读取失败进入调度器自身退避，不使用 `0` 时间戳继续计算。

History 公开只读的 `next_backup_disable_notice_target` 查询，按分支更新时间和 ID 稳定返回一个仍未进回收站的待确认 `{ artworkId, branchId }`。应用层命令使用它编排作品库告警到历史分支设置的导航；查询本身不确认通知，也不改变失败计数或重新启用状态。

调度器选出候选分支后，必须先取得共享运行锁，再重新查询分支是否仍启用、未进回收站、未进入发布状态且仍到期；分支设置写入也使用同一运行锁。用户取消、应用退出和候选资格失效使用结构化结果，不增加连续失败次数；只有实际备份错误进入上述失败退避。运行状态携带单调 `completionRevision`，每个共享操作结束时推进，因此前端即使没有轮询到短任务的 `busy` 窗口，也会在下一次轮询刷新历史。

## 空闲链路校验

调度器在第一优先级（到期自动备份）之外增加第二优先级：仓库空闲时校验分支 head 的链路完整性，作为完整扫描之外的“新链早发现”。它只在前台命令、待处理手动提交和到期备份都不占用时运行，并随托盘“暂停所有自动备份”一并暂停（用户意图是“别动仓库”）。每轮只处理一个分支，处理完回到循环顶部重新评估优先级，保证到期备份与前台操作的响应性。

派生队列不引入独立的待办表：查询所有未进回收站、`head_history_id` 非空且 `verified_history_id` 与 head 不一致（含 `verified_history_id IS NULL`）的分支，`verify_error` 非空的分支暂不重试。条件显式写成 `head_history_id IS NOT NULL AND (verified_history_id IS NULL OR verified_history_id != head_history_id)`，避免 SQL 的 NULL 语义把无 head 的分支纳入队列并每轮空跑。该查询不要求 `backup_enabled`（手动提交的分支同样需要校验），也不排除已发布分支（其 head 是强制检查点，校验成本低）——与排除已发布分支的备份调度查询是两条不同查询。回收站过滤经 `artworks → library_nodes.trashed_ms` 判断。新提交、精简改接、检查点在改变 head 时天然使分支重新入队，重启后队列自动重建。

每轮只校验 head 节点 `created_ms` 已早于 `now - IDLE_VERIFY_DELAY_MS`（常量，默认 10 分钟）的候选，避免刚写完就整链重读；这里用 head 节点的创建时间而不是 `last_check_ms`——快速检查的“内容未变化”会推进后者但不改变 head，用它会让延迟条件永不满足。单分支校验在运行锁内执行，锁内复查 head 仍与候选快照一致，链间检查取消标志。手动提交成功会经既有 `wake_scheduler` 唤醒调度器，新 head 随之进入派生队列；精简与检查点不改变 head，因此不触发重新入队。

校验范围收敛到 head 的单个 snapshot：`materialization_chain` 在目标节点持有 snapshot 时只返回该节点，而 head 恒持有 snapshot（提交以非可选 snapshot 建节点，`unmark_checkpoint` 拒绝取消 head、分支起点与分叉点的检查点），因此校验 head 等价于校验它的一个 snapshot 文件。实现复用 `validate_snapshot`（比对 `ChunkFile::file_digest()` 与数据库 `sha256`，并把全部分块流入 sink 逐块验哈希），不回溯整条链。

结果写入 `branches` 的校验列：成功且锁内确认 head 未变时写 `verified_history_id` 与 `verified_ms` 并清空 `verify_error`；失败写 `verify_error`，分支退出队列（不自动重试），直到 head 变化或用户手动重查。校验失败只记录警告，永不自动禁用备份、不阻断提交与恢复，也不进入自动备份的失败退避——两者独立。全库完整性扫描（`scrub_repository_integrity`）仍是唯一的全量保证，空闲校验与它是互补关系。

分支状态行把校验失败作为独立于备份失败的第三态展示：备份失败沿用 `lastError`，链路校验失败单独用警示色摘要 + 可展开详情（含复制入口），并附“重新校验此分支”按钮（清除 `verify_error` 并唤醒调度器重新入队）与“运行全库扫描”引导。两者语义独立、文案不合并。

## 历史操作

历史总览是纵向缩进的父子 mindmap，使用工作区原生滚轮纵向浏览；分支视图列出当前 head 的祖先链。节点左键只选择或在精简模式中勾选；总览中双击唯一属于一个分支的节点只切换当前分支选择，不进入分支视图，右键菜单仍提供显式“进入分支”。恢复使用系统“另存为”窗口选择新文件，并在页面头部报告可取消进度。

总览与当前分支视图使用同一个精简入口。总览直接在原视图进入精简模式，以分支下拉框的当前选择为范围，不切入分支视图；选择范围只包含该分支祖先链中有一个子节点且不是叶节点、分支 head、分支起点或检查点的普通中间节点，不允许混入其它分支节点。任务按分支链从后向前处理所选节点，重新物化父子节点，使用原始 ChunkFile API 重建新的反向 delta，再以事务同步改接 `history_nodes.parent_id`、`history_edges` 和兼容 `delta_path`，最后销毁旧节点和不再引用的文件。

删除节点仅从当前分支视角发起。预检在建立保留检查点之前拒绝包含发布节点的子树；若其它完整分支仍指向子树，也会拒绝并要求先删除对应分支。事务删除节点及后代并回退受影响分支，只更新 head 或分支起点确实落在删除集合中的分支，不改写同 Artwork 下无关旁支的 `updated_ms`。

检查点的建立与取消都需要二次确认并占用统一备份运行锁。建立时逐层报告回溯进度；取消普通检查点时，节点恢复使用唯一子节点到该节点的反向 delta，并把 UI 的当前存储路径/大小统计切回该 delta。分支 head、分支起点、分叉点和已进入发布状态的节点是强制检查点，不能取消。

全局设置和分支设置使用开关表达自动备份状态。托盘菜单会根据持久化状态动态显示“暂停所有自动备份”或“继续所有自动备份”；分支设置自动保存并显示未保存、保存中、已保存和保存失败状态。分支设置的自动备份开关旁提供“快速检查”开关：开启后该分支的自动备份强制使用快速检查，关闭时跟随全局默认检查方式；未选择工作文件或自动备份关闭时置灰。历史页顶部只保留一份工作文件路径，在当前分支名称下以较大字号显示，并通过紧邻的文件修改按钮重新选择；同一行还提供“打开所在文件夹”按钮（在系统文件管理器中定位工作文件，Windows 上会选中该文件）和“清除工作文件路径”按钮，后者把路径写回空字符串。`update_artwork_branch` 在同一事务内更新路径与其它分支设置，路径继续复用普通文件、绝对路径、仓库外和同 Artwork 分支唯一校验，选择取消或保存失败时保留旧路径。未选择工作文件时自动备份开关置灰、间隔输入禁用，保存结果始终是关闭；此时"修改工作文件"图标按钮替换为醒目的"选择文件"主按钮。设置草稿逐字段合并服务端更新，开关写入携带用户读取到的持久化基线；过期请求可以继续保存名称或间隔，但不能重新启用已由调度器自动关闭的备份。自动备份因连续失败被关闭后，作品库左侧持续显示警告，只有“知道了”或手动重新启用才清除通知。

历史页的分支状态行只显示自动备份失败的短摘要，不拼接后端完整错误。完整错误保存在可展开详情中并提供复制入口；详情使用独立浮层，不参与分支设置和删除操作的横向布局。自动重试、连续失败计数和自动关闭语义仍完全由持久化分支状态决定。

主动提交返回“创建节点”或“内容已是最新”两种成功结果。后者在历史页显示为成功检查状态，不再占用全局错误提示；自动任务刚刚抢先记录同一内容时，也使用这一结果说明当前内容已有备份。

未选择工作文件的分支（创建 Artwork 时留空，或事后清除路径，`source_path` 为空字符串）不参与备份：调度查询与调度文件统计都会排除这类分支，worker 对空路径兜底返回明确错误；分支设置的自动备份开关置灰并强制为关闭，历史页主动提交输入框与按钮在该分支上禁用并显示提示。这类 Artwork 的素材板等其他仓库功能不受影响。

`update_artwork_branch` 以 `source_path` 的三种取值区分语义：字段缺省表示本次不改路径，非空字符串表示设置新路径，空字符串表示清除路径。清空与设置都走同一条路径校验，因此绝对路径、普通文件、仓库外和同 Artwork 唯一性约束始终成立。由于 `branches.source_path_key` 为非空列且与 `artwork_id` 组成唯一索引，同一 Artwork 只能有一个分支不设置工作文件；第二个分支清空路径会被拒绝并返回明确错误，事务整体回滚。Rust 侧对空路径分支一律把 `backup_enabled` 落库为 0，前端置灰只是提示，真正的约束在仓储层兜底。

## 命令

```text
get_artwork_history
fork_artwork_branch
update_artwork_branch
run_branch_backup
restore_history_node
get_backup_runtime_status
cancel_backup_operation
reverify_branch_history
rename_history_node
set_history_checkpoint
compact_history_node
delete_history_subtree
delete_artwork_branch
scrub_repository_integrity
create_repository_backup
```

应用工作流通过公开 `backup::ensure_checkpoint` 固化分支起点或发布节点，再调用 History/Authenticity 领域服务；history 不导入 backup、成品文件或认证 manifest。当前批次状态见 `docs/planning/current-handoff.md`。

## 历史图前端布局

- 总览 mindmap 使用可横向滚动的内容画布，支持“紧凑”和“时间轴”排列模式。模式开关位于“历史总览”标题行；同一行的滑条把节点最小宽度调整在 220px 到 420px，并通过 `lilith-artworks.history-node-min-width-v1` 持久化。标题与控制行从滚动容器顶边开始吸顶，不留可透出画布内容的顶部空隙；兄弟节点水平间隔收紧，减少无效横向占用。
- 紧凑模式把同一父节点的子节点横向平铺；时间轴模式按兄弟节点顺序逐列向右、逐级向下错位，并由共同的父级连接线下接，表达分支随时间依次出现的阶梯关系。时间轴不再使用节点时间戳生成任意空白。
- 时间轴模式在画布左侧按日期分组列出全部节点，默认按时间倒序排列；标题栏可在正序与倒序之间切换，日期组和组内节点使用同一方向。时间轴与画布始终共同占满工具栏下方的剩余高度，内容较少时也保持完整列，不留下割裂的空白网格行。条目使用“创建分支 - 节点标题”区分同名提交，创建分支不在当前 DTO 中时回退为节点标题。时间轴条目与 mindmap 卡片共享节点选择状态；点击任一侧都会同步选中效果，点击时间轴条目还会聚焦并把对应卡片滚动到画布中央。
- 总览使用弱强调标出当前分支的祖先路径，并对当前分支 HEAD 标签使用实色强调；节点选中态使用更强的边框、背景和外框，不与当前分支提示混淆。
- 叶节点下显示其对应分支名称，帮助区分同一历史节点被多个分支引用的情况。
- 当前分支状态归 `src/app/ArtworkWorkspace.tsx` 所有，`HistoryModule` 通过受控属性读写，发布页复用同一状态；历史数据刷新只在当前分支失效时回退到第一个分支。
- `src/modules/history/useHistoryController.ts` 是历史前端状态与命令编排层，也是 `history/api.ts` 的唯一消费者；它负责请求代次、Artwork ID 校验、运行状态轮询和 mutation 回填。`HistoryModule.tsx` 只保留页面选择、上下文菜单、确认窗口和视图渲染。`ArtworkWorkspace.tsx` 不重复读取历史，只接收 controller 回推的 DTO，并用刷新版本通知历史页重新读取。
- 发布、进入发布或取消发布完成后，工作区递增历史刷新版本并立即重新读取历史 DTO，发布标签和计数不等待运行状态轮询。
- 节点卡片固定使用滑条给出的宽度并在所属子树内居中，父节点不会被多分支画布横向拉长；时间轴下沉节点的竖向连接线延伸到卡片顶部。
- 窄布局下页面标题、分支选择、提交区和精简工具栏分行排列，长 Artwork 标题使用省略显示；未保存状态使用主题化警告色。
- 全局设置快照统计当前未进回收站、未被成品锁定且启用自动备份的不同工作文件数；设置弹窗每次打开时刷新该统计。
