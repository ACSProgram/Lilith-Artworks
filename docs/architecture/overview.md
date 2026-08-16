# 系统架构

## 层级

代码任务先读 `docs/architecture/ai-reading-guide.md`，按问题选择入口；不要默认通读整个仓库。认证发布状态与成品绑定在 `authenticity/publication_repository.rs`，认证配置/记录查询在 `repository.rs`；历史破坏性删除通过 `history/deletion_repository.rs` 进入。

```text
src/app/                 应用启动、窗口、全局设置与跨模块工作区编排
src/modules/library/     Artwork 树、搜索和选择交互
src/modules/history/     分支历史图、提交、fork、恢复和裁剪
src/modules/authenticity 成品、C2PA/TrustMark 发布与识别
src/shared/              无领域语义的 UI、Tauri 调用和格式化工具

src-tauri/src/app/                     配置、窗口和应用生命周期
src-tauri/src/app/workflows.rs         跨领域 Tauri 命令与共享运行锁编排
src-tauri/src/storage.rs               SQLite 连接配置、路径与 ID 工具
src-tauri/src/library/mod.rs           作品树 Tauri 命令边界
src-tauri/src/library/schema.rs        仓库格式、schema 创建、校验与迁移
src-tauri/src/library/repository.rs    仓库定位、作品树查询、分组/Artwork 与回收站事务
src-tauri/src/history/                 分支、历史节点、fork、裁剪和历史图契约
src-tauri/src/backup/                  ChunkFile、增量提交、恢复、整仓灾备、调度和取消
src-tauri/src/authenticity/             C2PA、TrustMark、成品锁和认证记录
src-tauri/src/cleanup.rs                 数据库提交后的文件清理队列、路径校验和失败重试
src-tauri/resources/                    应用图标与随包分发的 TrustMark 模型
```

前端业务模块不能直接互相导入。`App` 向 Library 注入 Artwork 工作区渲染与文件清理重试，负责把认证记录转换为作品树导航目标；`ArtworkWorkspace` 在应用层组合历史和认证视图。History、Library 和 Authenticity 分别由 `useHistoryController.ts`、`useLibraryController.ts` 和 `useAuthenticityController.ts` 独占各自 `api.ts`，页面组件只保留视图选择、弹窗和渲染状态。控制器统一处理读取、mutation 回填、busy 状态和请求代次；工作区只接收领域 DTO、保存共享分支并发出刷新版本信号。跨领域清理结果使用 `src/shared/fileCleanup.ts` DTO。所有文件系统、数据库和模型操作都在 Rust 中完成。

Rust 的 Tauri 命令按调用方向分层：单领域读写留在 `library`、`history`、`backup` 和 `authenticity`；需要组合 checkpoint、调度唤醒、清理队列、仓库 lease 或共享 `BackupState` 运行锁的 create Artwork、永久清理、fork、分支更新/删除、整仓灾备、进入/取消发布和认证发布由 `app/workflows.rs` 编排。前端命令名和 DTO 不因内部所有权变化而改变。

## 应用生命周期

原生端启用 Tauri 日志插件，将信息级以上事件写入操作系统应用日志目录（Windows 默认位于 `%LOCALAPPDATA%\com.lilith.artworks\logs`），单个文件达到 1 MiB 后轮转并只保留最近一份。设置页提供配置文件夹、诊断日志文件夹和 About/Legal 入口；后者显示版本与 `Copyright 2026 ACSProgram`，并打开随包法律材料。首版记录启动、退出、窗口/托盘状态保存失败和自动备份失败等诊断事件；私钥、证书内容、Artwork 内容和完整认证声明不得写入日志。

主窗口使用无原生装饰的 48px 应用顶栏，窗口拖动、最小化、最大化/还原与关闭通过受限的 Tauri window capability 调用。应用启用 Tauri single-instance 插件；再次启动时只聚焦并恢复现有主窗口，不创建第二个窗口。关闭按钮仍触发 Rust `CloseRequested`，默认只隐藏到系统托盘；托盘左键或“打开”命令恢复窗口，只有托盘“退出”或关闭到托盘设置被禁用时才结束进程。窗口隐藏期间备份调度器继续运行。真正退出时应用层先保存窗口状态，再设置不可逆的 `shutting_down` 状态、请求取消活动操作、停止并等待调度线程，最后等待共享操作锁释放；排队任务在拿到锁后只返回取消错误，活动任务完成临时文件和事务清理前不会调用 `app.exit`。

设置由应用层聚合仓库与历史公开能力，界面按“仓库与数据安全、自动备份、外观、应用行为、关于与法律”排列。设置弹窗使用固定头部和底部操作区，中间内容在宽布局下双栏分组并独立滚动，窄布局回退为单栏，保存与取消不会随内容滚出视口。设置快照在不阻断设置读取的前提下附带当前参与自动调度的不同工作文件数；仓库尚未配置或不可读取时该统计为空。设置保存仍使用共享运行锁：首次配置或切换到新路径时允许显式初始化；路径未改变时只验证现有数据库，缺失时拒绝静默重建；随后才原子替换 `settings.json`、唤醒调度器并刷新托盘菜单。所有仓库命令还持有 `AppState` 的仓库 lease，设置切换必须等待活动 lease 结束；前端在保存新路径前卸载旧工作区，并以仓库路径作为工作区组件身份，因此克隆仓库即使包含相同实体 UUID 也不会复用旧选择或请求状态。

## 聚合模型

- Library tree：分组和 Artwork 构成任意深度树，Artwork 是叶节点。
- Artwork：作品级聚合根，拥有多个 branch。
- Branch：命名工作线，绑定唯一源文件路径和一个可变 head 指针。
- History node：不可变提交，只有一个父节点，可以被多个后继节点引用；fork 是从选定节点创建新 branch/head。
- Final artifact：分支可选的一份最终成品，固定关联进入发布状态时的 head。存在时分支被冻结，该 head 是强制检查点。
- Certification record：不可变导出快照，固定关联 final artifact、branch 和发布节点，记录 TrustMark ID、输出文件摘要、C2PA manifest 与验证状态。

作品树删除使用数据库软删除。删除一个分组会把其完整子树作为同一个回收站根保存，恢复时优先回到原父分组，原父级不可用时回到根级。只有回收站中的永久删除才清理项目元数据。Artwork 内部的历史节点裁剪属于版本图操作，不进入项目回收站。

历史边的真正所有权属于 history node 的 `parent_id`；branch 只记录当前 head。因此同一祖先可以自然产生多个后继，且无需复制祖先数据。

## 存储布局

```text
<repository>/
  lilith-artworks.sqlite3
  artworks/<artwork-id>/
    history/<history-id>.lbs
    deltas/<child-id>--<parent-id>.lbd
    artifacts/<branch-id>/
    temp/
  temp/
```

数据库持有元数据和仓库内文件的相对路径。snapshot、delta 和最终成品先写临时目录、同步并以不覆盖方式发布，最后在 SQLite 事务中切换引用；认证导出由用户选择外部绝对路径，同时记录 SHA-256。删除类跨边界操作先在同一事务写入 `pending_file_cleanup`，提交后执行文件清理；失败保留并可重试，应用启动时自动处理历史遗留项。

普通数据库连接只使用 SQLite `READ_WRITE` 打开现有 `lilith-artworks.sqlite3`，不得带 `CREATE`。仓库目录和新 schema 创建只从 Library 显式初始化入口进入。`AppState` 缓存当前路径已通过完整校验的事实：首次访问、启动状态读取和设置保存执行 SQLite 完整性、外键、实体 UUID、受控相对路径、持久化 SHA-256 与迁移检查，之后所有前台命令和后台调度统一执行轻量 format/version 检查。数据库被外部删除、清空或替换为其他格式/不受支持版本时会清除缓存并报告仓库不可用，不创建占位数据库。

提交、完整 fork、历史删除/精简、进入/取消发布、认证发布、Artwork 永久删除、清理重试、仓库完整性扫描、整仓灾备和仓库设置保存共享 `BackupState` 运行锁。跨领域入口由 `app/workflows.rs` 获取该锁和仓库 lease 后调用各领域公开服务；前端 busy 状态只负责交互反馈，不承担并发正确性。设置页的仓库完整性操作逐节点物化历史链，并校验最终成品、认证副本及 C2PA 声明；同一取消命令可以中断节点之间的扫描。

整仓灾备核心归 `backup/repository_backup.rs` 所有，但由 `app/workflows.rs` 同时取得共享运行锁和仓库 lease 后调用。它先 checkpoint WAL，再复制仓库内普通文件；副本通过 Library 公开打开校验、History/Backup scrub 和 Authenticity 受控文件 scrub 后生成逐文件 SHA-256 清单，最后以同卷目录重命名发布。前端只能提交文件选择器授权的目标父目录；临时 bundle 在失败或取消时清理，成功 bundle 内的 `repository/` 可作为新仓库打开。该流程不复制仓库外的分支工作文件。

## 增量历史策略

继续使用 LilithClient 的内容定义分块与 SHA-256。每个 branch head 保留完整 snapshot；父节点默认可由子 snapshot 加反向 delta 恢复。一个节点被多个分支/子节点引用时，清理逻辑按全图引用决定，不能再采用“旧 head 必删”的线性假设。

历史节点删除分两步：先在事务内计算并删除选定节点的完整后代集合、把受影响分支回退到最近未删除祖先；事务提交后再删除不再引用的文件。中间节点精简只处理单子节点普通节点，会用 ChunkFile 重新生成父子反向 delta 后再原子改接历史边。

## 认证边界

分支最终成品是认证输入，不替代备份历史。进入发布状态时先强制固化当前 head，再把成品复制进仓库并锁定分支。每次导出都强制签入 C2PA，TrustMark 可选；成功回读 C2PA 后才记录认证。TrustMark/C2PA 声明 ID 建普通索引以支持全库快速候选匹配；随后仍需比较记录、文件哈希、C2PA 验证状态和 soft binding，不能把模型解码结果单独视为认证成功。

进入发布状态由应用工作流先读取认证发布目标，再通过公开 backup 能力建立检查点，最后调用认证服务保存最终成品；认证模块不读取 ChunkFile 或导入 history。history 只通过 `final_artifacts` 是否存在判断分支锁定，不导入认证实现。前端跨 Artwork 的溯源跳转由应用层完成，认证模块不导入 Library。
