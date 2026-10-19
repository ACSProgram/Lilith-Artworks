# 历史与增量备份模块

## 上下文入口

按问题只读取一条路径：

- 页面选择状态、右键菜单、分支进入与精简选择：`src/modules/history/HistoryModule.tsx`；历史读取、运行状态和命令编排：`src/modules/history/useHistoryController.ts`。
- 工作区摘要（作品标题与分支列表）的非历史页兜底读取：`src/modules/history/useArtworkSummary.ts`；由 `src/app/ArtworkWorkspace.tsx` 在活动视图不是历史页且当前作品分支尚未加载时启用，只消费同一个 `history/api.ts`。
- 总览 mindmap 与左侧时间轴的纯展示：`src/modules/history/HistoryGraph.tsx`。
- 分支设置、保存状态、系统文件窗口和确认窗口：`src/modules/history/HistoryControls.tsx`，视觉规则只读 `src/styles/history.css`。
- 分支链、节点唯一归属和可精简资格：`src/modules/history/historyModel.ts`。
- DTO 和 Tauri 命令名：`src/modules/history/types.ts`、`src/modules/history/api.ts`。
- 创建分支、分支设置和分支删除的 Tauri 应用编排：`src-tauri/src/app/workflows.rs`；History 领域命令只保留历史读取和节点重命名。
- SQLite 历史图、分支和删除约束：`src-tauri/src/history/repository.rs`；不要为此加载 ChunkFile。
- snapshot/delta、恢复、检查点和精简：`src-tauri/src/backup/restore.rs`、`commands.rs`；只有块格式问题才进入 `chunk_file/`。
- `.clip`/`.psd` 容器探测与结构单元定位：`src-tauri/src/format/container.rs`；v2 容器感知分块与节点缩略图共用这一份解析，不要在备份或认证模块里另写。
- 节点缩略图抽取：`src-tauri/src/preview/`；只读工作文件、按格式分派，失败只跳过预览，不改变提交结果。
- 运行进度、取消和调度：`src-tauri/src/backup/runtime.rs`、`scheduler.rs`；跨模块取消标志的唯一类型在 `src-tauri/src/cancel.rs`。
- 设置持久化与托盘：`src-tauri/src/app/settings.rs`、`src-tauri/src/lib.rs`。

当前批次状态见 `docs/planning/current-handoff.md`，未完成事项见 `docs/planning/todo.md`。

## 模块边界

- `src-tauri/src/library/`：仓库初始化、作品树、搜索与项目回收站。
- `src-tauri/src/history/`：分支、历史节点、创建分支、head 和历史元数据事务。
- `src-tauri/src/backup/`：原始 ChunkFile、提交、checkpoint、恢复、取消与托盘调度。
- `src-tauri/src/preview/`：历史节点派生缩略图。只读工作文件、按格式分派（`.clip` 内嵌 SQLite、`.psd`/`.psb`、直接位图），任何失败只让该节点没有预览，不改提交结果；不参与 ChunkFile、摘要、scrub 与恢复。
- `src-tauri/src/authenticity/`：认证聚合边界；内部的 `c2pa`、`trustmark`、`pipeline` 和 `repository` 已实现，详细契约见 `docs/modules/authenticity.md`。
- `src-tauri/src/cleanup.rs`：`pending_file_cleanup` 清理账本——事务内入队、提交后单遍重放、失败留队可重试。history/backup/library/pin_board 的「提交成功后才应发生的仓库文件删除」统一收敛到它；引用判定与未引用扫描由中立契约 `contracts::ReferenceInventory` 的领域实现提供（history/authenticity/pin_board 各自实现，应用层注册），cleanup 不再导入画板路径常量。批量清理在连接内建立 `TEMP` 引用表并用 anti-join 一次判定未引用集合，按 512 条分批、每批可取消且幂等提交；`scan_unreferenced` 只报告，`cleanup_unreferenced` 登记摘要后入队重放，`list_pending` 提供只读队列。
- `src-tauri/src/storage.rs`：共享 SQLite 连接、ID、时间、路径与基本校验，不包含领域流程。

`backup` 通过 `history` 切换分支 head，不直接操作作品树；`history` 不读取 ChunkFile；C2PA 与 TrustMark 实现彼此独立，只由认证流水线编排。

## ChunkFile

`backup/chunk_file/` 完整迁移自 LilithClient `backup_agent/chunk_file.rs`，按职责拆为：`mod.rs`（结果类型、错误分类、`FormatVersion`/`ChunkWriteOptions`/`ChunkingConfig`、`ChunkDigest`/`ChunkRecord`、`ChunkTask` 与格式常量）、`format.rs`（头与块记录的读写、长度与能力校验）、`store.rs`（虚拟偏移段与 reader 游标）、`chunking.rs`（CDC 与容器对齐分块）、`snapshot.rs`（快照写入/打开/物化）、`delta.rs`（反向增量、payload 落盘与链解析）、`tests.rs`。snapshot 使用内容定义分块，默认 min/avg/max 为 2 KiB / 16 KiB / 64 KiB，整文件与块摘要均为 SHA-256。delta 是从当前子 snapshot 还原父节点的反向增量，并使用 zstd level 6 包装。

ChunkFile v1 的字节布局保持只读兼容：76 B snapshot 头、116 B delta 头、36 B 块记录。v2 默认用于**新写出的节点**（设置项 `chunkFileFormatV2`，默认开启，设置页可关闭后回退到 v1），头部为 80 B snapshot / 120 B delta，显式记录 `version/chunker/hash/codec/adapter`，块记录扩为 44 B（`stored_len/logical_len/codec/recipe/flags`）。reader 按文件头分派，因此同一仓库可长期混存 v1/v2；关闭开关只影响此后新生成的 snapshot/delta，不重写既有文件、不做仓库级迁移。无头入口另以 `--chunk-format v1|v2` 在**进程级**覆盖该开关（缺省跟随设置默认值），供 `stress_format` M4 在同一仓库内构造 v1/v2 混存历史；该选项只影响本进程新写入的节点，既有文件与读取路径不受影响。v2 快照对内容相同的块写 `stored_len=0` 的引用而不是重复 payload（tile 级去重），打开时按先出现的同摘要块解析引用；delta 仍只用外层 zstd 包装，不做逐块转码。块长度校验按布局分派：`min_size` 只约束 CDC 布局，容器对齐产生的结构单元（24 B 文件头、56 B `Exta` 头）只受 `max_size` 约束，超出时才交给 CDC 切分。成功提交、检查点与精简的日志都会记录本次写入的 `format`/`chunking`，便于解释仓库内的版本分布。

v2 的容器感知分块由 `format/container.rs` 提供结构单元：`.clip` 按 CSFC 块、tile 与 Exta 记录边界切块，超出 `max` 的结构单元再交给 CDC 切分；`.psd` 当前只做 `8BPS` 探测（适配器结构待逆向确认），探测到但取不到结构单元时与解析失败一样回退 CDC。`.clip` 解析只校验头部与块边界、跳过 payload，不在内存里保留整份文件。

`ChunkStore` 把基础 snapshot 与各条 delta 的解压 payload 追加为同一段虚拟偏移空间中的分段。v2 在此基础上维护按虚拟偏移排序的 segment 起点索引：顺序读取命中 reader 缓存段，跳跃读取用二分查找定位，跨段读取在一次 `read` 内完成，因此读取成本不再随段数线性增长。错误按 O5 的两轴分类：`InvalidFormat`（帧/布局损坏）、`Integrity`（块摘要或整文件摘要不符）、`Incompatible`（未知版本或能力缺口），并附带 container/format/chunker/hash/codec/recipe/adapter/chunk/file 维度，便于 UI 决定重试还是报告损坏。

delta 打开时不再把压缩体和完整解压体同时读入内存。zstd 或旧版未压缩 payload 都流式写入仓库 `temp/` 中的自动删除文件；允许的解压字节数由 delta 头声明的目标逻辑大小、数据记录数和操作数推导，超出声明范围按格式错误拒绝。该边界随真实工作文件大小增长，不对 PSD 或其它工作文件设置固定总字节上限；4 GiB 目标的回归测试用于防止安全校验误伤正常大文件。恢复应用阶段只在内存中保存块索引和操作元数据，块内容从临时 payload 或基础 snapshot 流式复制并逐块验哈希。

在同一段虚拟偏移空间上，`ChunkFileDelta::resolve` 只重写块布局、不写 payload 字节；`write_snapshot`、`copy_original` 在导出或校验时才按虚拟偏移流式读取并逐块验哈希。因此物化一条链的成本取决于链上变化的字节数，而不是"链长 × 文件大小"。`ChunkFileDelta::apply`（物化成完整 snapshot 文件）由 resolve + write_snapshot 组合实现，chunk 格式单测在 `cargo test --lib` 内运行，不依赖 GUI。创建、反向 delta、payload 复制、链解析、导出和 scrub 都直接消费 crate 根部的 `CancelToken`，并按取消观察的两层分开使用：结构单元（每个 tile）与 CDC 块只做**静默观察**（`CancelToken::observed`，不写 marker、不阻塞），显式边界（提交、恢复、精简、检查点的链与发布边界）才发**交互检查点**（`CancelToken::is_cancelled`，无头闸门在那里写一行 marker 并在 `--cancel-on-stdin` 下阻塞等判定）。因此检查点数量由边界决定、与文件分成多少块无关；边界判定的取消会被热循环在下一次静默查询时立刻看到。进度按已处理的逻辑字节报告，取消返回 typed `Cancelled`，未发布的临时输出由 guard 清理。

创建分支后同一父节点允许多个子节点，因此 `history_edges` 让每条 `child_history_id -> parent_history_id` 边分别拥有 delta 文件；不复用线性历史的单一后继假设。每个分支 head 保留完整 snapshot，旧 head 没有其他分支引用时才释放 snapshot。

删除分支时，文件候选同时收集该分支节点的 `snapshot_path`、旧兼容 `delta_path` 和 `history_edges.delta_path`。候选不再由调用方在事务提交后直接删除：删除事务内先一次收集契约层提供的完整引用清单，再逐路径判定当前图是否仍引用它（事务可见本事务的删除结果），只把已经无引用的 snapshot/delta 入队 `pending_file_cleanup`（原因 `history_branch_deletion`），提交成功后由应用层单遍重放删除。共享祖先或其它分支仍在使用的文件既不入队也不会被误删；删除失败只留队列可重试，不改变分支删除的成功语义。删除历史子树（`history_subtree_deletion`）与取消检查点（`history_checkpoint_release`）走同一条「事务内复查引用后入队」的路径。

清理账本只重放已入队的条目，因此崩溃发生在 `history::commit` 之前（snapshot/delta 已发布、数据库未提交）时留下的孤儿文件从未入队，不会被自动回收。未引用文件扫描补上这一「发现」能力：`cleanup::scan_unreferenced` 遍历各作品目录，由 `ReferenceInventory::scan_candidates` 报告受管理命名模式（历史 snapshot/delta 与画板 DDS；路径规则不再写在 cleanup 里），以及作品 `temp/` 下崩溃残留的 `.tmp*` 暂存文件（备份、恢复与检查点在发布前写下的 `NamedTempFile`，从不进入引用清单，正常路径由 `Drop` 删除）。它只报告修改时间早于 30 分钟宽限期、且不在一次性 `collect_paths` 引用集合中的文件，**只报告不删除**。设置页确认后 `cleanup::cleanup_unreferenced` 逐条登记当前 SHA-256，再由带取消令牌的批量重放删除：引用表是一次性 `TEMP` 表，每 512 条提交一次，重复确认幂等；候选在确认前重新被引用时条目留队可重试。扫描入口仍只经 GUI 进程内暴露；画板 DDS 的「记录 → 文件」缺失/损坏检查属完整性扫描，另见 `docs/modules/pin-board.md`。

设置页「空间」页的「文件清理」区把队列与发现机制暴露给用户：队列列表展示每条待清理条目的路径、原因、上次失败原因与最近尝试时间，支持单条重试与全部重试（`retry_pending_file_cleanup`）；同一区提供未引用文件扫描按钮（复用可取消的统一运行状态与进度展示）与扫描结果的确认清理入口，确认前经应用内确认对话框二次确认。队列列表由只读命令 `list_pending_file_cleanup` 提供，它走共享读租约、不取运行锁，因此在备份、恢复等长任务运行期间也能返回，且不写 `last_attempt_ms` / `last_error`；除路径与原因外还返回期望 SHA-256 与所属作品标题（后者按导出路径回查认证记录，查不到时为空）。

仓库根 `temp/` 不进入引用清单，因此走一条更直接的自动清扫：仓库打开成功后（`prepare_repository`）执行 `cleanup::sweep_repository_temp`，只删除两类名字——超期（同一 30 分钟宽限期）的 `authenticity-preview-<UUID>` 预览缓存会话目录（`authenticity::pipeline` 每进程一个，丢失只让下一次预览重算）与 `.tmp` 前缀的 `NamedTempFile` 暂存残留（恢复链解析在进程被强杀或断电时留下的孤儿）——本进程当前会话目录一律保留，命名不符的手工条目一律不动；只碰顶层、不跟随符号链接，删除失败只记日志，不阻断打开。作品级 `artworks/<artwork>/temp/` 的 `.tmp*` 残留不在此列，仍走未引用文件扫描的报告→确认流程。

**空 id 不再等于「处理全部」**：`retry_pending_file_cleanup` 传空 id 时只重放**仓库内条目**（`repository_file` / `repository_directory`），`external_file` 条目一律跳过。外部条目保存的是仓库之外的绝对路径，删除它等于一条任意路径删除，唯一门槛只是攻击者需要知道目标文件的 SHA-256；若它随启动自动重放，被投毒的仓库就等于自带一条「自动、无确认的删除任意文件」原语。因此外部条目改为在队列里**逐条显式确认**：列表直接展示完整路径、所属作品与期望 SHA-256，点击「确认删除…」后由滑动确认控件（`ConfirmDialog` 的 `slideToConfirm`）承担最终动作，确认后以**显式 id** 调用同一命令执行；「全部重试」按钮在只剩外部条目时禁用，并注明它只重放仓库内条目。既有的「摘要不匹配即不删」约束不变（`cleanup::remove_external_file` 仍只在 SHA-256 相等时删除）。首次启动时应用仍会自动重放一次仓库内条目，队列中的残留仓库内条目即上次未清干净的文件。

## 节点缩略图

提交 worker 在确认工作文件内容已变化后，从同一份工作文件抽取派生预览并随新节点写入 `history_nodes.thumbnail` / `thumbnail_version`。缩略图固定最长边 512 px、无损 WebP、版本 1；它是旁路产物，**不进入 ChunkFile、不参与 `sha256`、scrub 与恢复，也不进入 `pending_file_cleanup` 或灾备路径清单**——删除节点/分支时随行删除，整仓备份的 SQLite 副本天然包含它。

抽取位于 `src-tauri/src/preview/`，只读、有界、可取消；任何失败只让该节点没有预览，绝不影响提交、摘要或恢复：

- `.clip`：复用 `format/container.rs` 定位 `SQLi` 块，把该块有界复制到系统临时目录后以只读 SQLite 打开，扫描名字含 `preview` 的表（优先 `CanvasPreview`）中的可解码 BLOB。容器自带的画布预览 PNG 若无法解码（例如非标准 filter 字节）则回退为占位。
- `.psd` / `.psb`：优先 Image Resources 中的 1036 / 1033 缩略图资源（JPEG 或 raw RGB；1033 按规范是 BGR，取出后换回 RGB）；取不到时跳过 Image Resources 与 Layer and Mask 段，直接解码合并合成图（8/16 位，RGB / 灰度 / CMYK，raw / RLE / ZIP ± prediction）。无法识别、截断或超出 64 MP 预算时无预览。
- png / jpg / webp / tiff / bmp / gif：在同一解码预算内直接解码并按最长边缩放。
- 其它格式、解析失败或损坏头部：无预览，详情区显示占位。

内容未变化的快速检查不打开工作文件，沿用 head 已有缩略图；完整检查确认内容未变化时也不改写。检查点、fork 与 snapshot 修复作用于同一节点或复用原节点，不重新抽取；存量节点不回填。历史图与列表查询继续使用显式列名，不把 BLOB 带出；详情区选中节点时通过只读命令 `get_history_node_thumbnail` 单独拉取 data URL。读取只服务当前 `thumbnail_version` 的载荷，未知版本按「无预览」处理，避免把不认识的编码当作 WebP 发给前端。

## 存储大小语义

`history_nodes.chunk_file_size` 记录"重建该节点需要读取的字节数"：节点拥有 snapshot 时为 snapshot 文件大小（由发布它的调用方写入），否则为指向其唯一子节点的边 delta 大小；多个子节点时取最小 delta（分支起点通常持有 snapshot，不会走到该分支）。`history_nodes.delta_path` 兼容列与该语义保持一致，仅在没有边（分支根）时被 `load_node_from` 读取。写路径统一由 `refresh_storage_metadata` 从图推导：提交释放父 snapshot、取消检查点、精简改接后都会重算，精简后刷新的是被移除节点的父节点（新 delta 的受益方）与子节点。

## 仓储空间状态

设置页「空间」页的仓储仪表盘是只读视图，数据来自两个命令：

- `get_repository_storage_overview`：从 SQLite 聚合数据库记录口径，走共享读租约、不取运行锁，也不写任何状态。
- `scan_repository_storage_disk`：按需遍历仓库目录，扫描期间取前台运行锁与仓库操作锁、可经统一取消入口中断；不持有跨进程 owner lock。

**数据库记录口径**：snapshot 文件字节 = `Σ history_nodes.chunk_file_size WHERE snapshot_path IS NOT NULL`；delta 字节 = `Σ history_edges.delta_size`。非 snapshot 节点的 `chunk_file_size` 是重建读取量，**不并入存储总量**。最终成品按规范化 `source_path` 去重后计入 `byte_size`；认证输出按规范化 `output_path` 去重后计入 `output_bytes`。仪表盘按历史快照、历史增量、最终成品、认证输出四类展示，分类之和等于数据库记录总量。

**磁盘实际口径**：只扫描仓库目录本身，外部首次导出路径不计入。manifest 中可识别的 snapshot / delta / 最终成品 / 认证仓库副本 / 画板 DDS 按实际文件大小归类；SQLite 主库与 WAL、仓库 `temp/`、作品 `artworks/<artwork>/temp/`（备份与恢复的暂存区，含崩溃残留的 `.tmp*`）、未识别文件合并为「其它」。分类之和等于仓库目录实际文件字节。两套口径并列显示、不静默合并；差异本身用于发现记录与磁盘不一致。

**画板 DDS 分类**按「条目是否存在实体文件」判定：查询 `pin_board_bitmaps.file_path` 非空并拼出 `artworks/<artwork>/boards/<board>/<file_path>`。文字条目没有载荷行，不进入磁盘与记录口径。

**压缩比分层报告**：snapshot 压缩比 = `Σ 快照节点 logical_size / Σ 快照文件字节`；delta 还原比 = `Σ 父节点 logical_size / Σ history_edges.delta_size`，不混算整仓平均。明细按作品 → 分支 → 节点展开，逐行给出 `logical_size`、`chunk_file_size`（snapshot 行为快照文件大小，其余行为重建读取量）、`chunk_count`、压缩比与 checkpoint / snapshot 标记；画板与认证输出按类别展开。

聚合固定为 8 条查询（`src-tauri/src/storage_overview.rs` 的 `overview_from_connection`），不按作品或节点逐条回查；30 作品 / 360 节点的 debug 单测记录查询次数与耗时上界。

## 仓库版本迁移

- 当前 `SCHEMA_VERSION = 5`，新建仓库直接以 v5 落库。打开旧仓库时沿 `v1 → v2 → v3 → v4 → v5` 迁移链向前升级。
- **更新版本的仓库一律拒绝打开**：例如 0.2 线（支持到 v4）遇到 v5 仓库会直接返回“由更新版本的 Lilith Artworks 创建”并停止，不尝试读写或降级，避免旧程序按旧结构写入造成损坏；轻量版本检查同样要求精确匹配当前版本。
- `v4 → v5` 是 0.3 唯一一次表重建：画板改为 `pin_board_items` + `pin_board_bitmaps` / `pin_board_texts` 多态结构，旧 `pin_board_images` 与 `image_id` 历史原位迁移；`history_nodes` 追加 `thumbnail`、`thumbnail_version`、`verified_ms`、`verify_error`；删除 `branches` 上 0.2 遗留的 `verified_history_id` / `verified_ms` / `verify_error`；建立 snapshot/delta/认证路径索引与 `repository_backup_runs`。
- 迁移在 `PRAGMA foreign_keys = OFF` 下用 `BEGIN IMMEDIATE` 执行，提交前做 `PRAGMA foreign_key_check`；任一步失败整笔回滚并保留 v4 原表，提交后才恢复 `foreign_keys = ON`。

## 提交与恢复

提交顺序沿用 LilithClient：读取前后比较源文件元数据，临时生成 snapshot/delta，`sync_all`，以不覆盖方式发布文件，最后在 SQLite 事务中切换 head。数据库失败会清理本次新文件（回滚路径上文件本就未被数据库引用，直接删失败只留泄漏、不留不一致，由清理扫描兜底）。提交释放的父 snapshot（父节点不再是 head 或检查点、且无其它分支引用时）在同一事务内入队 `pending_file_cleanup`（原因 `history_commit_release`），提交成功后由 worker 单遍重放删除；删除失败只留队列可重试，不影响已成功的提交结果。head 校验失败触发 snapshot 修复时，被替换的旧文件同样由登记事务入队（原因 `history_snapshot_replaced`）。工作文件与 head 的 SHA-256 相同时，worker 仍会打开受控 snapshot、核对数据库摘要并逐块验证内容；文件缺失、格式损坏、块损坏或摘要不匹配时，使用本次已经生成的 snapshot 以新路径发布并更新原 head 节点，验证通过后才把任务记为内容未变化。有效 snapshot 不重写，修复也不创建新历史节点。主动提交备注可为空并生成“主动提交”节点；调度器使用独立的 automatic 类型和空备注，不会覆盖主动提交备注。

恢复从目标节点向下寻找最近可用 snapshot，再沿父链逐条 `resolve` 反向 delta（惰性，不产生中间 snapshot 文件），最后一次性把结果流式导出为临时文件并禁止覆盖；链起点与每步结果都会与数据库摘要比对，导出时逐块验哈希并核对整文件摘要。`history::materialization_chain` 用一次递归 CTE 同时选出最近 snapshot、父链节点与每步使用的 delta 路径，不再按节点逐条查库；目标自身持有 snapshot 时仍直接返回该节点。恢复、精简和检查点共用同一条惰性解析路径：精简只物化被移除节点的父链，子链只取块索引，然后从父链的虚拟存储中按需读取变化的块重建反向 delta，改接事务内把被移除节点与旧边占用的文件入队 `pending_file_cleanup`（原因 `history_compaction`）；检查点把解析结果一次性写成 snapshot 文件并 `sync_all` 后发布，建立、取消或替换检查点释放的旧 snapshot 在同一事务内入队（原因 `history_checkpoint_release` / `history_snapshot_replaced`）；全库 scrub 走同一条链但把结果流入 sink，只校验不写临时文件，并刷新节点级校验状态。上述入队均由调用方在提交成功后单遍重放，删除失败只留队列可重试。所有长操作经 `OperationCoordinator` 记录开始、结束、耗时与 typed 错误字段（`code`/`kind`/`retryable`/operation id）。

仓储页提供可取消的全库完整性扫描。扫描在共享运行锁、跨进程 owner lock 和仓库 lease 内逐个物化全部历史节点，因此同时覆盖 snapshot 缺失、格式/块损坏、delta 损坏以及数据库摘要不匹配；它同时重写各节点的 `verified_ms` / `verify_error`，单节点失败不中止其余节点，最终以汇总错误返回。设置弹窗打开或操作运行期间由应用层轮询共享备份运行状态，显示当前阶段、确定/不确定进度和统一取消入口；状态预检会阻止把新的设置页长任务排在已有备份操作之后。

备份视图也提供在线整仓备份，用户可见命名统一为“创建备份”。命令在共享运行锁和仓库 lease 内先 checkpoint SQLite WAL，再按扫描、复制、校验、发布阶段把数据库和仓库内全部普通文件逐块复制到同一目标卷的临时 bundle；符号链接、仓库内部输出目录和复制期间变化的文件会被拒绝。复制阶段边复制边计算每个文件的完整 SHA-256 与首尾各 64 KiB 的抽样摘要，写入 v2 `manifest.json`，生成清单时不再完整重读这些文件；只有副本校验与 checkpoint 会改写的数据库主文件重新取摘要。副本的领域校验由应用层注入的 `contracts::RepositoryIntegrityProvider` 完成（打开副本、历史链 scrub、受控发布文件 scrub），灾备核心不再直接导入 library/authenticity；计数随报告返回。发布前先复核清单：`Sample` 模式只重读每个文件首尾各 64 KiB 的抽样摘要，`Full` 模式完整重读（内部保留的强校验选项），两者都同时核对文件集合与大小；复核通过后才以目录重命名发布。取消或失败会显式清理未发布的临时 bundle，并把清理成功或失败附加到返回错误。命令在复制开始之前（校验目标目录之后）还会清扫目标目录**顶层**残留的未发布暂存目录：只处理名字为 `.lilith-artworks-<32 位十六进制>.tmp`、确实是目录（用 `symlink_metadata` 判断，不跟随符号链接）且修改时间早于 30 分钟宽限期的项，逐个删除并把回收数量与失败数写入返回报告（`reclaimedStagingDirectories` / `failedStagingDirectories`）。宽限期避免误删正在进行（本进程或另一进程）的灾备暂存目录，删除失败只计数、不阻断本次备份；只碰顶层、不递归匹配。应用不持久化上一次的目标目录，因此不提供针对旧目标的独立清扫入口。bundle 内的 `repository/` 是可直接打开的恢复副本，`manifest.json` 位于其外层。分支工作文件是仓库外部输入，不属于整仓备份；恢复后仍需保证相应外部工作文件可用，或为分支重新选择工作文件。备份成功后会把本次结果写入 `repository_backup_runs`（文件数、总字节、历史节点/成品/认证计数、恢复路径与完成时间）；设置页重启后读取最新一行显示最近一次备份，失败写入只记警告，不改变已经成功的备份结果。

## 调度

每个分支保存独立开关与 1 到 10080 分钟的检查间隔。调度线程随 Tauri 应用启动，主窗口隐藏到托盘时继续运行；显式退出时先进入 `shutting_down`、请求取消，再等待调度线程和共享操作锁结束。已经排队但尚未取得锁的任务不会在退出过程中重新清除取消标志或开始备份。分支绑定最终成品后会自动从调度查询排除。分支起点、分支 head 和显式设置的节点会保留完整 snapshot 作为 checkpoint。调度优先级固定为前台等待 > 手动提交 > 到期自动备份 > 空闲完整性检查；候选按 `due_at`、上次检查时间与分支 ID 组成最小堆，`due_at` 已把失败退避写入分支自己的 `retry_at_ms`，因此一条分支失败不会把整支队列停 60 秒。

自动备份支持快速检查与全量检查两种方式，由分支级 `backup_quick_enabled` 逐分支决定，调度器只读该标志。全局设置 `automaticBackupCheckMode`（`quick` / `full`，默认 `quick`）只是"默认检查方式"：在创建 Artwork 与创建分支时写入 `backup_quick_enabled` 作为初始值，此后不参与运行时判定，用户可在分支设置里独立开关。快速检查比较工作文件的大小与上次全量检查成功后记录的 cheap fingerprint（`last_source_size` / `last_source_modified_ms`，由 `run_backup` 在提交或确认内容未变化时写入）：指纹算法版本为 1，内容是 SHA-256 覆盖版本字节、大小、修改时间（秒 + 纳秒）、首 64 KiB 与尾 64 KiB 采样，取摘要前 8 字节并把最高位清零后复用 `last_source_modified_ms` 的整数列存储（不新增 schema）。一致时直接记为内容未变化并推进检查时间，不打开工作文件；缺少基线（如尚无任何提交）、旧数据里保存的是裸 mtime（与新指纹必然不等，因此自然退回一次全量并重写基线）、修改时间缺少亚秒精度、任一元数据不一致或采样读取失败时，退回完整流程。手动提交始终走全量检查。快速检查不校验 head snapshot 的完整性，该职责仍属于内容变化后的全量路径与全库完整性扫描。

手动提交优先于自动备份：手动提交命令在取得共享运行锁前先登记该分支的待处理标记；调度器在候选选择和取得运行锁后两处都会跳过有待处理标记的分支（延后且不计入失败）。若同分支的自动备份已在运行，手动提交会对后台任务请求取消，使自动任务以取消结果退出（不进入失败退避），随后手动提交先执行；自动备份延后到下一次调度周期，此时内容已被手动提交覆盖，通常得到"内容未变化"的结果。

共享运行状态 `BackupRuntimeStatus` 携带 `taskKind`，取值为 `automaticBackup` / `idleVerify` / `userOperation`（`BackupTaskKind`）；同一时刻通常只有一个串行任务，测试中同时登记多个时由汇总进度呈现（不单列各任务 id）。每个任务在登记时创建独立的 `OperationId` 与 `CancelToken`，不再维护全局 `cancel_requested` 加补丁集合；后台让位只取消后台任务的令牌，不会让刚进站的前台命令立即读到「已取消」。取消分两个入口：`request_cancel` 取消全部活动任务，供用户主动取消；`cancel_background` 只取消后台任务（自动备份与空闲完整性检查）的令牌，不会误取消用户自己的操作。所有前台长命令（恢复、精简、检查点、删除子树、进入发布、发布签名、全库扫描、整仓备份、仓库切换）在取得运行锁之前调用一次 `cancel_background`，让正在运行的后台任务尽快退出，并登记前台等待计数 `foreground_waiting`；调度器在候选选择阶段跳过新任务，并在取得运行锁之后复查该计数，大于零即以让位结果退出、不放行本次后台任务，随后进入退避等待，避免与前台争抢运行锁。计数由 RAII 守卫在取得锁之后自减，取消、失败、退出和提前返回等所有路径都会归零。该让位与手动提交的 `manual_pending` 是同一套"锁内复查后让位"模式：前台等待登记使调度器不会在前台意图到达后抢先取锁。取得 owner lock 之前，调度器还要先经过与命令层相同的进程内写入闸门：本进程已有前台/交互写者占位时本轮直接跳过（`OwnerAttempt::Skipped`），既不记一次失败也不推进分支退避，随后按 `FOREGROUND_YIELD_BACKOFF` 短退避；只有真正的跨进程冲突才记 `warn`。

自动备份成功或确认内容未变化时才推进 `last_check_ms`/`last_success_ms`，同时清零连续失败计数。自动任务失败会记录 `last_error` 并按分支间隔退避：首次失败 1 分钟后重试，随后分别等待原间隔的 1/4、1/2 和完整间隔（向上取整）；第 5 次连续失败时持久化关闭该分支自动备份并生成待确认通知。手动重新启用分支会清零失败状态和该通知；成功备份也会清零计数。系统时间读取失败进入调度器自身退避，不使用 `0` 时间戳继续计算。

工作文件位于**网络位置**（UNC，`\\server\share\...`）或**设备命名空间**（`\\.\`、`\\?\`、`\Device\`）的分支不进入无人值守流程：调度器在候选筛选时跳过它，并在分支记录里写入明确原因（`mark_automatic_backup_skipped`，原因未变时不写库，也不计入连续失败、不设置重试时间、不自动关闭自动备份）；取得运行锁后的 `run_scheduled_backup` 再复查一次，命中即跳过；`quick_check_unchanged` 作为最后一道兜底直接拒绝，不再对该路径取元数据。判定统一由 `storage::unattended_source_path_rejection` 给出，与「未选择工作文件的分支不参与自动备份」同一口径——这是路径资格判定，不是一次失败的备份，因此设置页的「已启用自动备份的工作文件数」也不把它们计入。手动提交与「打开所在文件夹」仍允许网络位置（NAS 是合法用法），但一律拒绝设备命名空间（`storage::ensure_not_device_path`）：设备路径指向的是设备对象而不是文件。打开既有仓库时的路径结构校验见 `docs/modules/library.md`。

History 公开只读的 `next_backup_disable_notice_target` 查询，按分支更新时间和 ID 稳定返回一个仍未进回收站的待确认 `{ artworkId, branchId }`。应用层命令使用它编排作品库告警到历史分支设置的导航；查询本身不确认通知，也不改变失败计数或重新启用状态。

调度器选出候选分支后，必须先取得共享运行锁，再重新查询分支是否仍启用、未进回收站、未进入发布状态且仍到期；分支设置写入也使用同一运行锁。用户取消、应用退出和候选资格失效使用结构化结果，不增加连续失败次数；只有实际备份错误进入上述失败退避。运行状态携带单调 `completionRevision`，每个共享操作结束时推进，因此前端即使没有观察到短任务的 `busy` 窗口，也会在完成事件或下一次 3 秒快照轮询时刷新历史。

## 空闲完整性检查

调度器在到期自动备份之外使用第二个任务位做节点级完整性检查。功能**默认关闭**（全局设置 `idleIntegrityCheckEnabled`，旧设置文件缺省即关闭），入口在设置页「备份」页，同时显示与派生集合同口径的「N 个历史节点待校验」只读计数。开启后只在前台命令、待处理手动提交和到期自动备份都不占用时运行，并随托盘「暂停所有自动备份」一并暂停（用户意图是「别动仓库」）。没有到期备份时检查始终先走；有到期备份时，连续完成 4 次自动备份后若已有就绪候选，先插队跑一批（aging，防低优先级饥饿；计数在批次完成后清零）。

**时机门控**（任一不满足即退出本轮，等待下次唤醒）：

1. 主窗口隐藏或最小化。窗口状态由原生窗口事件在主线程用 `is_visible()` / `is_minimized()` 计算后写入 `BackupState`，只在状态真正变化时唤醒调度器（拖动与焦点切换会频繁触发这些事件）；关闭到托盘等显式隐藏路径直接写入并唤醒，不重新查询可见性。调度器只读原子值，避免后台线程跨线程查询窗口，也避免退出时与事件循环互等。
2. 无前台等待、无待处理手动提交、无到期自动备份（aging 插队例外）。
3. 仓库最近一次历史写入已静默 ≥ 10 分钟。时间取最新历史节点的 `created_ms`；只统计真正产生节点的写入，快速检查推进的 `last_check_ms` / `last_success_ms` 与校验状态自身都不计入，否则备份间隔较短或每批校验都会让静默条件永远无法满足。这是防抖而不是频率，避免连续提交使条件永不满足。
4. 取得共享运行锁与跨进程 owner lock 后复查以上条件，并重新读取待查集合。

**节奏**：一个空闲窗口 = 主窗口从「可见且未最小化」转为「隐藏或最小化」后的一次连续期间（进程内判定，无持久化、无计时器）。每个窗口至多执行一批；一批 = 8 个节点或 30 秒，先到为准。一次尝试无论完成、失败还是被抢占都消耗该窗口名额；窗口重新可见后再隐藏/最小化才开启下一个窗口。首次开启的存量 backlog 不做一次性补跑，按窗口自然消化。

**派生待查集合**不建表：未进回收站的作品中，`verified_ms IS NULL AND verify_error IS NULL` 的历史节点，按 `created_ms` 最旧优先——最久未读的节点正是 bit rot 风险最高的节点。失败节点（`verify_error` 非空）退出自动队列，不自动重试，直到节点被改写或用户手动重查。

**校验内核**：`backup::verify_history_node` 物化单个历史节点（沿链解析到最近 snapshot、与数据库 `sha256` 比对、把全部分块流入 sink 逐块验哈希）。全库扫描（`scrub_repository_integrity`）、空闲批次与手动重查共用这一个内核，不存在单独的全链恢复校对路径。空闲批次在每个节点边界复查门控与让位条件；前台到达、手动提交、用户取消或非 aging 批次出现新到期的自动备份时立即停止，且不写当前节点的状态。失败信息带节点 id 写入该节点的 `verify_error`；通过则写 `verified_ms` 并清空错误。

**失效规则**：新提交的节点天然未验证；精简、检查点建立/取消与 snapshot 替换会改写链文件，在同一事务内按作品粗粒度清空 `verified_ms` 与 `verify_error`，受影响节点重新进入待查集合。粗清的代价只是部分节点被重查，正确性无损。

**手动入口**：`reverify_branch_history` 升级为**前台立即执行**——逐节点校验整条分支链（带进度、可取消），不受开关影响；全库扫描同样顺带重写全部节点的校验状态，单个节点失败不会中止其余节点，最终以汇总错误返回。全库扫描现在会写校验状态，因此经前台写命令（跨进程 owner lock）执行。

**历史页派生展示**：分支状态行不再读 head 节点，而是按整条链派生——链上任一节点 `verify_error` 非空即校验失败；否则链上全部节点都有 `verified_ms` 时取最旧的通过时间，任一节点未验证则为 NULL。失败摘要独立于备份失败 `lastError` 展示，附「重新校验此分支」按钮。v4 → v5 迁移删除 `branches` 上 0.2 遗留的校验三列，节点级列是唯一事实来源。

## 历史操作

历史总览是纵向缩进的父子 mindmap，使用工作区原生滚轮纵向浏览；分支视图列出当前 head 的祖先链。节点左键只选择或在精简模式中勾选；总览中双击唯一属于一个分支的节点只切换当前分支选择，不进入分支视图，右键菜单仍提供显式“进入分支”。恢复使用系统“另存为”窗口选择新文件，并在页面头部报告可取消进度。

总览与当前分支视图使用同一个精简入口。总览直接在原视图进入精简模式，以分支下拉框的当前选择为范围，不切入分支视图；选择范围只包含该分支祖先链中有一个子节点且不是叶节点、分支 head、分支起点或检查点的普通中间节点，不允许混入其它分支节点。任务按分支链从后向前处理所选节点，重新物化父子节点，使用原始 ChunkFile API 重建新的反向 delta，再以事务同步改接 `history_nodes.parent_id`、`history_edges` 和兼容 `delta_path`，最后销毁旧节点；被移除节点与旧边占用的文件在同一事务内入队 `pending_file_cleanup`，提交成功后重放删除，删除失败只留队列可重试。

精简在 UI 上以「删除节点」呈现：工具栏按钮用共享的红色描边删除样式，确认弹窗同名。删除前先经一次普通确认；若所选节点里存在主动提交节点，或存在标题被人工改过的节点，再追加一道更强的滑动确认（`src/shared/SlideToConfirm.tsx`，须把滑块拖到底），因为这两类节点承载明确的人工意图。改名与否由 `historyModel.ts` 把 `title` 与提交时生成的默认标题（备注非空取备注、否则按 `commitKind` 取「自动备份」/「主动提交」）比对得出，不为此增加 schema 或 DTO 字段；调度器写入自动备份时固定使用空备注，因此自动备份节点的默认标题恒为「自动备份」。

历史页的确认弹窗在操作运行期间把页脚取消按钮换成「取消操作」，并禁止关窗（共享控件的 `busy` 契约见 `docs/architecture/overview.md` 的「前端共享控件」）。该入口只在**本页自己的**操作运行时提供：`runtime.busy` 也可能来自后台自动备份或空闲完整性检查，那不是这个操作。

取消本身分两层。原生共享取消标志只覆盖**正在执行的那一条命令**（`BackupState` 在每条命令取得运行锁时清零、命令结束时再清零一次，见 `src-tauri/src/backup/runtime.rs`），因此精简这类逐节点调用必须由前端自己记住「已请求取消」：批量循环在启动下一个节点之前检查该标记，命中即停止并刷新历史。已删除的节点不恢复，精简选择保留——其中已不在树里的节点会被剔除，计数不会虚高。

收尾提示按批量结果统一。只要用户请求过取消，一律报「精简已取消，剩余 N 个节点未被删除。」，包括正在执行的命令自己以「操作已取消」退出的情况（被取消的节点没有提交，因此从它算起都计入剩余），前端不把这个原生错误原样转给用户；只有整个操作在取消到达前已经完成时才报「取消请求未被接受，操作已在取消到达前完成」。取消落在最后一条命令的最后一个检查点之后时，那一个节点仍会被删除（改接是原子事务，不为此造回滚路径），批量从下一个节点起停住。

删除节点仅从当前分支视角发起。预检在建立保留检查点之前拒绝包含发布节点的子树；若其它完整分支仍指向子树，也会拒绝并要求先删除对应分支。事务删除节点及后代并回退受影响分支，只更新 head 或分支起点确实落在删除集合中的分支，不改写同 Artwork 下无关旁支的 `updated_ms`。

「删除分支」与「删除节点与后续历史」同为不可逆操作，两者的确认弹窗只有一道，其最终动作都由共享滑动确认控件承担；需要两道确认的只有上面精简里含重要节点的那种情况。

检查点的建立与取消都需要二次确认并占用统一备份运行锁。建立时逐层报告回溯进度；取消普通检查点时，节点恢复使用唯一子节点到该节点的反向 delta，并把 UI 的当前存储路径/大小统计切回该 delta，被释放的 snapshot 在同一事务内入队 `pending_file_cleanup`，提交成功后重放删除。分支 head、分支起点、分叉点和已进入发布状态的节点是强制检查点，不能取消。

全局设置和分支设置使用开关表达自动备份状态。托盘菜单会根据持久化状态动态显示“暂停所有自动备份”或“继续所有自动备份”；分支设置自动保存并显示未保存、保存中、已保存和保存失败状态。分支设置的自动备份开关旁提供“快速检查”开关，直接决定该分支自动备份是否使用快速检查，分支状态行按它显示检查方式；新建 Artwork 与新建分支按全局默认检查方式写入初始值。未选择工作文件或自动备份关闭时置灰。历史页顶部只保留一份工作文件路径，在当前分支名称下以较大字号显示，并通过紧邻的文件修改按钮重新选择；同一行还提供“打开所在文件夹”按钮（在系统文件管理器中定位工作文件，Windows 上会选中该文件）和“清除工作文件路径”按钮，后者把路径写回空字符串。`update_artwork_branch` 在同一事务内更新路径与其它分支设置，路径继续复用普通文件、绝对路径、仓库外和同 Artwork 分支唯一校验，选择取消或保存失败时保留旧路径。未选择工作文件时自动备份开关置灰、间隔输入禁用，保存结果始终是关闭；此时"修改工作文件"图标按钮替换为醒目的"选择文件"主按钮。设置草稿逐字段合并服务端更新，开关写入携带用户读取到的持久化基线；过期请求可以继续保存名称或间隔，但不能重新启用已由调度器自动关闭的备份。自动备份因连续失败被关闭后，作品库左侧持续显示警告，只有“知道了”或手动重新启用才清除通知。

历史页的分支状态行只显示自动备份失败的短摘要，不拼接后端完整错误。完整错误保存在可展开详情中并提供复制入口；详情使用独立浮层，不参与分支设置和删除操作的横向布局。自动重试、连续失败计数和自动关闭语义仍完全由持久化分支状态决定。

主动提交返回“创建节点”或“内容已是最新”两种成功结果。后者在历史页显示为成功检查状态，不再占用全局错误提示；自动任务刚刚抢先记录同一内容时，也使用这一结果说明当前内容已有备份。

未选择工作文件的分支（创建 Artwork 时留空，或事后清除路径，`source_path` 为空字符串）不参与备份：调度查询与调度文件统计都会排除这类分支，worker 对空路径兜底返回明确错误；分支设置的自动备份开关置灰并强制为关闭，历史页主动提交输入框与按钮在该分支上禁用并显示提示。工作文件位于网络位置或设备命名空间的分支同样不参与自动备份，但手动提交仍可用（见上文），分支设置不因此置灰。这类 Artwork 的素材板等其他仓库功能不受影响。

`update_artwork_branch` 以 `source_path` 的三种取值区分语义：字段缺省表示本次不改路径，非空字符串表示设置新路径，空字符串表示清除路径。清空与设置都走同一条路径校验，因此绝对路径、普通文件、仓库外和同 Artwork 唯一性约束始终成立。由于 `branches.source_path_key` 为非空列且与 `artwork_id` 组成唯一索引，同一 Artwork 只能有一个分支不设置工作文件；第二个分支清空路径会被拒绝并返回明确错误，事务整体回滚。Rust 侧对空路径分支一律把 `backup_enabled` 落库为 0，前端置灰只是提示，真正的约束在仓储层兜底。

## 命令

```text
get_artwork_history
get_history_node_format
get_history_node_thumbnail
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
list_pending_file_cleanup
retry_pending_file_cleanup
scan_repository_unreferenced
cleanup_repository_unreferenced
get_repository_storage_overview
scan_repository_storage_disk
```

应用工作流通过公开 `backup::ensure_checkpoint` 固化分支起点或发布节点，再调用 History/Authenticity 领域服务；history 不导入 backup、成品文件或认证 manifest。当前批次状态见 `docs/planning/current-handoff.md`。

## 可靠性不变量与覆盖

以下命题是历史/备份模块对使用者的承诺，每条都由自动化压力测试（`src-tauri/tests/`；九组便宜的
分组随 Windows CI 运行，`stress_large`/`stress_scale`/`stress_bench`/`stress_baseline` 仍在发布
前手动运行，见 `docs/guides/validation.md` 与 `docs/user/stress-test-report.md`）以**独立进程
调用真实可执行文件**的方式验证；断言只用子命令返回的 JSON 与磁盘事实，不读内部状态。

| 不变量 | 证明场景 |
| --- | --- |
| 提交在任意取消检查点被取消都不留残留（无未发布的 snapshot/delta、临时文件已回收、待清理队列为空），历史图不前进，且可立即重试成功 | `stress_cancel` A1 |
| 恢复 / 精简 / 检查点 / 全库扫描被取消后输出不存在、历史图不变、临时文件已回收 | `stress_cancel` A2–A4、A6 |
| 整仓灾备被取消时未发布的临时 bundle 被清理，源仓库不受影响，副本重跑成功 | `stress_cancel` A5 |
| 进程在「文件已发布、数据库未提交」之间被强杀后仓库仍可重开（迁移 + 完整性 + 外键 + 全表语义校验，以及逐块摘要链），head 不前进；已发布文件成为孤儿（不自动回收，可扫描发现） | `stress_crash` B1 |
| 进程在 SQLite 事务中途被强杀时，未提交写入被整体丢弃（head、父快照释放、清理入队一并回滚） | `stress_crash` B4 |
| 崩溃孤儿可被扫描发现、经用户确认后清理，被引用文件不被误删 | `stress_cleanup` B5 |
| 4 GiB / 8 GiB 工作文件的「提交 → 有界改动 → 恢复两端」逐位一致；磁盘增量跟随改动量而非文件大小；峰值内存不随文件大小线性增长 | `stress_large` C1–C6 |
| 单作品 300 次提交的深链、约 20 条分支的历史图、约 400 个 Artwork 的库：计数、搜索、移动、删除与恢复均正确 | `stress_scale` D1–D3 |
| 整仓灾备副本可独立打开并通过完整性与链路校验，树结构与源仓库一致 | `stress_scale` E1 |
| 越界输入（备注/标题/搜索超长、非法提交类型、不存在的历史节点、已存在的输出路径、非法移动目标）被明确拒绝而非崩溃 | `stress_scale` F1 |
| 损坏的 snapshot/delta 被内容级校验（`scrub`）发现；数据库级校验（`verify`）不读取文件内容因此仍通过；head snapshot 损坏后一次内容未变化的提交可修复；恢复损坏节点被拒绝且不留半成品 | `stress_damage` R1–R2（另有 `backup/worker.rs`、`backup/restore.rs` 单测） |
| GUI、无头与维护入口的每次写操作先在同一进程的写入闸门里按优先级排队、再取跨进程 owner lock：同进程的第二个写者排队等待而不是与前者竞态，跨进程则仍拿到明确拒绝；锁由内核在句柄关闭时释放，因此强杀不残留，随后仍能取得锁。锁库与 owner 元数据不参与数据库发现、灾备复制或未引用文件判定 | `stress_concurrency` I1、I2；`repository_lock` 与 `library::repository` 单测 |
| 写进程持 owner lock 期间只读命令仍作答并看到一致状态（含「快照已发布、事务未提交」的窗口）；并发读互不阻塞 | `stress_concurrency` J1、J2（跨进程）；`app/settings.rs` 单测（进程内长写持锁时的读租约） |
| 主库文件本身（`lilith-artworks.sqlite3`）被误写或清空时，两层校验都干净失败并给出可行动的错误，且失败的打开不会把空文件重建为空仓库 | `stress_damage` R3–R4 |
| 大文件提交在任意取消检查点被取消、或中途被强杀，都不留残留、历史不推进、可重试；强杀后 head 恢复逐字节一致 | `stress_large` C7 |
| 逐个受管理文件（snapshot/delta）做多种子单点改写，每一次都被内容级校验发现（退出码 1、无 panic），数据库级校验不受影响；复原后最早版本仍逐位可恢复 | `stress_damage` R5 |
| 整仓灾备与精简这两类**长**写任务持锁期间，只读命令仍作答、未超时，且看到改接 / 复制之前的一致状态（树逐字段一致、历史未推进） | `stress_concurrency` J3、J4 |
| 不构造窗口的真并发「精简 + 全库校验」不挂起、不损坏仓库（结束后完整且每个仍存在的节点逐位可恢复）；写者持 owner lock 时全库校验立即以「仓库正在被另一个进程写入；不支持同一仓库的多进程写入」明确拒绝，轻量只读命令不受影响 | `stress_concurrency` J5、J6 |
| 快照/增量容器面对不受信任的字节：绝不 panic、越界或无界分配；被接受的容器块数量不超过文件容量、可物化且长度等于声明值；截断与续写必须被拒绝 | `backup/chunk_file/tests.rs` 单测（确定性畸形输入模糊） |
| `.clip` 容器感知分块在真实链路上逐字节一致：tile 结构快照与 delta（磁盘事实确认 chunker/adapter 为 CLIP）、无 tile 形态（Exta 单结构单元）、截断容器回退 CDC 后照常提交与恢复 | `stress_format` M1–M3 |
| v1/v2 混存仓库：`--chunk-format v1` 写出的节点与默认 v2 节点并存，跨格式链全部可恢复；检查点把含 v1 delta 的链物化为 v2 快照；格式随提交步进在磁盘上可核对 | `stress_format` M4 |
| 磁盘写满时快照写入干净失败：返回「空间不足」、不推进历史、不留半成品、可重试；写到一半的字节不会被误认为完整快照 | `backup/worker.rs`、`backup/chunk_file/tests.rs` 单测 |

0.3 阶段 4 在 `stress_bench` 增加 P1B：4 MiB 文件 × 6 节点的多段链上反复恢复最早与最新节点，报告 p50/p95 与吞吐；它只作为段索引与读取游标的决策证据，随发布前的整套压力测试手动运行，不进入日常检查、CI 门槛或性能基线更新。快速检查的同尺寸同 mtime 改写由 `backup/worker.rs` 单测覆盖（回拨修改时间但在首尾采样区改动内容，必须触发全量）。0.3 阶段 5 在同一基准里增加 P7（10 个作品、每作品 12 条分支的调度候选扫描）与 P8（120 个无记录画板 DDS 的扫描与确认清理）；P5 整仓灾备同时记录文件数、吞吐与 `verification: sample`，作为摘要管线与抽样复核的基线。

**不做的**：断电持久性不做自动化验证——`Child::kill()` 不触及操作系统与磁盘的缓存、也不保证
目录项落盘，该保证由操作系统、磁盘与 `synchronous = FULL` 声明承担。

## 历史图前端布局

- 总览 mindmap 使用可横向滚动的内容画布，支持“紧凑”和“时间轴”排列模式。模式开关位于“历史总览”标题行；同一行的滑条把节点最小宽度调整在 220px 到 420px，并通过 `lilith-artworks.history-node-min-width-v1` 持久化。标题与控制行从滚动容器顶边开始吸顶，不留可透出画布内容的顶部空隙；兄弟节点水平间隔收紧，减少无效横向占用。
- 紧凑模式把同一父节点的子节点横向平铺；时间轴模式按兄弟节点顺序逐列向右、逐级向下错位，并由共同的父级连接线下接，表达分支随时间依次出现的阶梯关系。时间轴不再使用节点时间戳生成任意空白。
- 时间轴模式在画布左侧按日期分组列出全部节点，默认按时间倒序排列；标题栏可在正序与倒序之间切换，日期组和组内节点使用同一方向。时间轴与画布始终共同占满工具栏下方的剩余高度，内容较少时也保持完整列，不留下割裂的空白网格行。条目使用“创建分支 - 节点标题”区分同名提交，创建分支不在当前 DTO 中时回退为节点标题。时间轴条目与 mindmap 卡片共享节点选择状态；点击任一侧都会同步选中效果，点击时间轴条目还会聚焦并把对应卡片滚动到画布中央。
- 总览使用弱强调标出当前分支的祖先路径，并对当前分支 HEAD 标签使用实色强调；节点选中态使用更强的边框、背景和外框，不与当前分支提示混淆。
- 叶节点下显示其对应分支名称，帮助区分同一历史节点被多个分支引用的情况。
- 当前分支状态归 `src/app/ArtworkWorkspace.tsx` 所有，`HistoryModule` 通过受控属性读写，发布页复用同一状态；历史数据刷新只在当前分支失效时回退到第一个分支。
- 「当前分支」视图的「节点信息」栏在选中节点时按需读取该节点当前存储文件的 ChunkFile 头，显示 `ChunkFile v1` / `ChunkFile v2` 标签：节点持有 snapshot 时取快照，否则取与 `chunk_file_size` 相同口径的子边增量（多个子节点时取最小的一条），最后回退兼容列 `delta_path`。读取只消费文件前 10 个字节；delta 文件整体是 zstd 流，识别到 zstd 帧魔数后先流式解压出前 10 字节再判版本（`stored_format_label`），snapshot 则按明文读取。走只读命令 `get_history_node_format`，不新增 schema，也不进入历史图轮询；文件缺失或不可读时显示「未知」，不进入全局错误提示。
- 同一个「节点信息」栏在选中节点时按需显示该节点的缩略图：命令 `get_history_node_thumbnail` 只读取该行的 `thumbnail` BLOB 并返回 data URL；无预览、读取失败或节点已无缩略图时显示「无可用预览」，不进入全局错误提示，也不触发 restore 或工作文件读取。缩略图请求只随选中节点与历史刷新发起，不进入图轮询。
- `src/modules/history/useHistoryController.ts` 是历史前端状态与命令编排层，也是 `history/api.ts` 的唯一消费者；它负责请求代次、Artwork ID 校验、运行状态和 mutation 回填。运行状态由统一 `AppEvent` 即时唤醒：`history` / `backup` / `authenticity` 域的 `operationFinished` 与 `repositoryChanged` 会立刻读取共享状态，并在 `completionRevision` 推进时刷新历史；3 秒快照轮询保留为丢事件、启动恢复和后台调度任务的兜底（0.3 阶段 3 的口径）。`HistoryModule.tsx` 只保留页面选择、上下文菜单、确认窗口和视图渲染。`ArtworkWorkspace.tsx` 不重复读取历史，只接收 controller 回推的 DTO，并用刷新版本通知历史页重新读取。
- 发布、进入发布或取消发布完成后，工作区递增历史刷新版本并立即重新读取历史 DTO，发布标签和计数不等待运行状态轮询。
- 节点卡片固定使用滑条给出的宽度并在所属子树内居中，父节点不会被多分支画布横向拉长；时间轴下沉节点的竖向连接线延伸到卡片顶部。
- 窄布局下页面标题、分支选择、提交区和精简工具栏分行排列，长 Artwork 标题使用省略显示；未保存状态使用主题化警告色。
- 全局设置快照统计当前未进回收站、未被成品锁定且启用自动备份的不同工作文件数；设置弹窗每次打开时刷新该统计。
