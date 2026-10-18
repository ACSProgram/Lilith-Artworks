# 系统架构

Lilith Artworks 是本地优先的**平面美术个人项目**工作台，统一管理三件事：作品资源（Artwork 树与
素材板）、版本（可派生分支的增量历史）与发布（最终成品与 C2PA/TrustMark 认证）。四个领域模块
（Library、History/Backup、Authenticity、Pin-board）共享同一作品仓库，互不导入，由应用层编排。

## 层级

代码任务先读 `docs/architecture/ai-reading-guide.md`，按问题选择入口；不要默认通读整个仓库。认证发布状态与成品绑定在 `authenticity/publication_repository.rs`，认证配置/记录查询在 `repository.rs`；历史破坏性删除通过 `history/deletion_repository.rs` 进入。

```text
src/app/                 应用启动、窗口、全局设置与跨模块工作区编排
src/modules/library/     Artwork 树、搜索和选择交互
src/modules/history/     分支历史图、提交、创建分支、恢复和裁剪
src/modules/authenticity 成品、C2PA/TrustMark 发布与识别
src/modules/pin-board    按 Artwork 的素材板画布、图片导入导出与画板回收站
src/shared/              无领域语义的 UI、Tauri 调用和格式化工具

src-tauri/src/app/                     配置、窗口和应用生命周期
src-tauri/src/app/command_runtime/     CommandContext、typed error、AppEvent v1 与 OperationCoordinator
src-tauri/src/cancel.rs               跨模块 CancelToken（命令运行时与 ChunkFile I/O 共用）
src-tauri/src/app/usecases/            GUI 与 headless 共享的业务入口
src-tauri/src/app/workflows.rs         跨领域 Tauri 命令适配器
src-tauri/src/app/command_registry.rs  全部 Tauri 命令的注册表（按领域分组）
src-tauri/src/storage.rs               SQLite 连接配置、路径与 ID 工具
src-tauri/src/library/mod.rs           作品树 Tauri 命令边界
src-tauri/src/library/schema.rs        仓库格式、schema 创建、校验与迁移
src-tauri/src/library/repository.rs    仓库定位、作品树查询、分组/Artwork 与回收站事务
src-tauri/src/history/                 分支、历史节点、创建分支、裁剪和历史图契约
src-tauri/src/backup/                  ChunkFile、增量提交、恢复、整仓灾备与调度
src-tauri/src/format/                  `.clip`/`.psd` 容器探测与结构单元定位（备份与后续缩略图共用）
src-tauri/src/authenticity/             C2PA、TrustMark、成品锁和认证记录
src-tauri/src/pin_board/               素材板持久化、DDS/BC7 处理与纹理缓存
src-tauri/src/contracts/               中立契约：引用清单与灾备完整性 provider
src-tauri/src/cleanup.rs                 文件清理账本：批量 anti-join、未引用文件扫描与确认清理
src-tauri/resources/                    应用图标与随包分发的 TrustMark 模型
```

前端业务模块不能直接互相导入。`App` 向 Library 注入 Artwork 工作区渲染与文件清理重试，负责把认证记录转换为作品树导航目标；`ArtworkWorkspace` 在应用层组合历史、素材板和认证视图。History、Library、Authenticity 和 Pin-board 分别由 `useHistoryController.ts`、`useLibraryController.ts`、`useAuthenticityController.ts` 和 `usePinBoardController.ts` 独占各自 `api.ts`。素材板控制器负责画板列表、选择、生命周期结算与保存/导入/导出状态；`PinBoardRenderer` 仍是纯渲染器，只处理 GPU 与交互。四个 controller 的请求代次统一来自 `src/shared/useRequestEpoch.ts`，History、Library 与 Authenticity 的 busy/finally 与错误上报来自 `useAsyncOperation.ts`；素材板的传输忙碌由传输流程自己持有（同一时刻只允许一次导入或导出），不复用按计数合并的 busy。页面组件只保留视图选择、弹窗和局部交互状态。工作区只接收领域 DTO、保存共享分支并发出刷新版本信号。跨领域清理结果使用 `src/shared/fileCleanup.ts` DTO。所有文件系统、数据库和模型操作都在 Rust 中完成。

Rust 的 Tauri 命令是薄适配器：解析 DTO、检查文件选择器授权、装配 `CommandContext`，然后调用 `app/usecases/` 中 GUI 与 headless 共享的业务入口。所有仓库入口都经 `OperationCoordinator` 统一取锁、取消、进度、耗时日志与事件；需要组合 checkpoint、调度唤醒、清理队列或发布流程的跨领域用例写在 `app/usecases/` 里，`app/workflows.rs` 只保留这些命令的 Tauri 适配函数。`pin_board` 命令直接在 `app/command_registry.rs` 注册，写命令走 `run_repository_write`（仓库 lease + 仓库操作锁，不取共享 `BackupState` 运行锁），浏览走共享读租约。前端命令名和 DTO 不因内部所有权变化而改变。

跨模块旁路统一走中立契约层 `src-tauri/src/contracts/`：各领域用 `ReferenceInventory` 报告受管理文件、用 `RepositoryIntegrityProvider` 提供灾备副本的领域校验，应用层在 `app/contracts.rs` 注册实现。cleanup 因此不再导入画板路径常量，灾备核心也不再直接导入 Library/Authenticity；阶段 7 的画板 kind 分派在契约上扩展，不需要把领域模块互相接上。

开发/验证期另有一个**无界面命令行入口**（`--headless`，由 `headless` feature 门控、**不进发布产物**）：它与 Tauri 命令层并列，是同一批领域函数的另一个适配器，只做参数解析、锁与状态装配、领域调用，不含业务判断。它只供发布前的压力测试以**独立进程**调用真实可执行文件，不创建窗口、不建托盘、不加载 webview、不经过 IPC，因此发布版的产品行为零变化。定位、运行方式与覆盖矩阵见 `docs/guides/validation.md` 的「压力测试」小节，面向使用者的结论见 `docs/user/stress-test-report.md`。

## 前端共享控件

`src/shared/` 是无领域语义的 UI 与工具层，跨模块复用的确认交互也归它所有：模块里不要再写第二套。

- `ConfirmDialog.tsx`：应用内唯一的确认弹窗（原生 `window.confirm` 在 Tauri webview 中不可靠）。
  `ConfirmView.danger` 决定红色危险样式；`emphasis` 是最危险的一档，在 danger 基础上进一步加强
  警示；`slideToConfirm` 把页脚主操作换成下面的滑动控件。任何模块都不要再手写 `.dialog-backdrop`
  加 `role="alertdialog"` 的弹窗标记。
  操作运行期间（`busy`）的取消语义由调用方决定：只有底层真的有可中断点时才传
  `onCancelOperation`，此时页脚取消按钮变成「取消操作」，并且**禁止关窗**（背景点击与头部关闭
  按钮一并失效，运行期间只剩这一个出口）；不传时保持原语义，取消按钮就是「关闭弹窗」。
  因此**不可中断的操作不要把弹窗留在运行态**：确认后立即关窗，而不是给出一个点了不生效的取消
  入口。发布页的「取消发布并删除本地数据」与「删除发布记录」按此处理（一步到位的短操作，底层
  没有取消检查点）；历史页五个入口都传 `onCancelOperation`，见
  `docs/modules/history-and-backup.md` 的「历史操作」。
- `SlideToConfirm.tsx`：滑动确认控件，只给最危险、不可逆的操作用。必须在轨迹上把滑块拖到底
  （键盘按 `→` 逐步推进）才触发；单击或半途松手都会回弹，因此不存在「在同一个位置连点两次」
  的误确认路径，也不需要用户多读一段文案。接入方式是在 `ConfirmView` 上置 `slideToConfirm: true`，
  轨迹文案仍取 `action`。
- `slideToConfirmTesting.ts`：驱动滑动控件的测试助手。jsdom 不实现 `PointerEvent`，它用带坐标的
  `MouseEvent` 触发 `pointer*`，行程取控件导出量不到布局时的回退值。`*_test` 里的弹窗测试统一从
  这里取，不要再各写一份拖拽序列。
- `format.ts` / `fileCleanup.ts` / `menuPlacement.ts`、`useMenuPlacement.ts`：格式化、跨领域清理
  DTO 与浮层定位。

**规则**：不可逆删除的最后一道确认用滑动控件（`danger` + `emphasis` + `slideToConfirm`），可恢复
或可撤销的操作沿用点击确认。当前采用滑动的有：库「清空回收站」、素材板「清空回收站」、历史
「删除分支」/「删除节点与后续历史」、「删除节点」中选中含主动提交或已改名节点的第二道确认、
发布「取消发布并删除本地数据」。同一次流程需要两道确认时，第一道仍是点击确认。

共享控件样式写在 `src/styles/index.css`（基础控件），模块样式留在各自的 `src/styles/<module>.css`。
`ConfirmDialog` 的 danger / emphasis 两档警示样式都在 `index.css` 里，各模块共用同一套，不要再按
模块作用域重写一遍。

## 应用生命周期

主窗口使用无原生装饰的 48px 应用顶栏，窗口拖动、最小化、最大化/还原与关闭通过受限的 Tauri window capability 调用。应用启用 Tauri single-instance 插件；再次启动时只聚焦并恢复现有主窗口，不创建第二个窗口。关闭按钮仍触发 Rust `CloseRequested`，默认只隐藏到系统托盘；托盘左键或“打开”命令恢复窗口，只有托盘“退出”或关闭到托盘设置被禁用时才结束进程。窗口隐藏期间备份调度器继续运行。真正退出分两段：原生端保存窗口状态后向 webview 发送 `app_shutdown_requested` 并启动 15 秒兜底强退计时器；webview 收到事件后结算素材板（保存并截断步骤历史，`preparePinBoardRuntimeChange`），无论成败都调用 `confirm_app_shutdown` 确认。原生端确认后设置不可逆的 `shutting_down` 状态、请求取消活动操作、停止并等待调度线程，最后等待共享操作锁释放再调用 `app.exit`；排队任务在拿到锁后只返回取消错误，活动任务完成临时文件和事务清理前不会退出。webview 未能在时限内确认（挂起或崩溃）时由兜底计时器强制退出，保证窗口始终可以关闭。关闭链路的异常接入结构化诊断：前端记录结算结果、耗时与超过 5 秒仍未完成的 `[watchdog]` 告警；原生端记录 15 秒确认超时，以及确认后等待活动操作或仓库操作锁超过阈值时的 `[watchdog]` 记录。这些记录只补充证据，不改变既有的等待与兜底语义。

WebView 级别的整页刷新快捷键（F5 / `Ctrl+R`）由 `src/app/webviewShortcuts.ts` 在应用层拦截，且只取消默认行为、不停止事件传播，使素材板“锁定画板”等模块快捷键仍能复用同一组合键（与 Lilith Client 的实现一致）。

前端运行时由 `AppRuntimeProvider` 组合 `useSettingsController`、
`useRepositoryMaintenance`、`RuntimeProgressProvider`、`SettingsDialog` 与 `NoticeCenter`；
`App` 只负责提供者装配、工作区路由与两条 webview 快捷键拦截，不再持有设置、维护进度、提示或
关闭结算状态。

设置由应用层聚合仓库与历史公开能力，弹窗按左侧导航分页：通用（外观、应用行为、关于与法律）、仓库与备份（仓库与数据安全、自动备份、文件清理）、素材板（缓存等级、阵列间距、自动保存与退出保存开关、锁定与全屏快捷键）、调试（详细日志、日志文件夹）。每页为单列纵向布局（不再沿用 Client 的两列紧凑排版），片段间距统一。弹窗使用固定头部与底部操作区，中间内容独立滚动，保存与取消不会随内容滚出视口。

设置文件带 `version` 字段，读取时低于当前版本会走追加式迁移并写回；当前 v2 把仍是旧默认值的素材板锁定键位 `CommandOrControl+Shift+K` 升级为 `CommandOrControl+R`，用户自定义过的其他键位保持不变。设置快照附带当前参与自动调度的不同工作文件数，仓库未配置或不可读取时为空，且不阻断设置读取。

设置文件另有独立的单调 `revision` 计数（与 schema、ChunkFile 格式和发布版本号无关）：每次成功写入 +1。设置页保存、托盘暂停开关和窗口几何捕获统一走 `SettingsService`；设置页携带读取时的 revision 做 CAS，陈旧 revision 返回 `settings_conflict` 而不是静默覆盖，托盘/窗口补丁在同一写锁内基于当前快照原子写入。前端在冲突后重新读取设置与仓库状态并保持弹窗打开，由用户确认后再保存。

窗口几何在退出时由原生端捕获写回 `settings.json`，捕获只在窗口处于正常状态时进行：最小化时 `GetWindowRect` 返回的是任务栏图标矩形（实测约 237×39、坐标 -32000），最大化时是铺满屏幕的矩形，二者都不是用户摆放的几何，写进去会让下次启动的尺寸校验失败。尺寸必须落在主窗口允许范围（与 `tauri.conf.json` 的 `minWidth`/`minHeight` 一致的 760×560 起、16384 以内）才被采信，坐标还必须与当前某块显示器重叠出足以看见窗口的范围。加载时对越界字段就地修正（窗口尺寸夹紧到允许范围）而不整份回落默认，避免单字段异常连带清空仓库路径与其它偏好；恢复位置时若保存的坐标已不落在任何当前显示器上则改为居中，避免窗口开到已断开的显示器上而看起来像没有启动。

设置保存使用共享运行锁：首次配置或切换到新路径时允许显式初始化；路径未改变时只验证现有数据库，缺失时拒绝静默重建；随后才原子替换 `settings.json`、唤醒调度器并刷新托盘菜单。所有仓库命令还持有 `AppState` 的仓库 lease，设置切换必须等待活动 lease 结束。前端在保存新路径前先结算素材板（`preparePinBoardRuntimeChange`，失败即中止切换并保留旧仓库）再卸载旧工作区，并以仓库路径作为工作区组件身份，因此克隆仓库即使包含相同实体 UUID 也不会复用旧选择或请求状态。

## 命令执行底座（0.3 阶段 1）

`app/command_runtime/` 是 GUI、headless 与未来维护入口共用的执行底座：

- `OperationCoordinator` 提供 `run_read`、`run_write`、`run_foreground`、`run_foreground_read`、
  `run_background`，以及素材板专用的 `run_repository_write` 与设置保存专用的
  `run_repository_switch`。固定锁序为「跨进程仓库 owner lock → 共享运行锁（写 / 前台 / 后台）→
  仓库 lease → 仓库操作锁 → SQLite 事务」；只读入口只取共享 lease。`run_repository_write`
  保持素材板既有语义，不取共享运行锁。`run_foreground_read` 供全库校验与画板 DDS 检查使用：
  它取得前台运行锁与仓库操作锁保证扫描期间本进程不写入，但只**探测** owner lock，不持有它，
  避免只读长任务变成跨进程写者。
- 跨进程 owner lock 由 GUI、headless 与维护入口共用：每次写操作在运行分类锁之前取锁，冲突立即
  返回 `repository_busy`，操作结束释放。拒绝不区分持锁者是否在同一进程——自动备份或空闲校验
  持锁期间，前台的写命令与长命令同样立即被拒，不排队、也不请求后台让位（详见
  `docs/modules/library.md` 的仓库命令进入规则）。锁仍是仓库根目录下专用 SQLite 库的
  `BEGIN IMMEDIATE`，由内核在句柄关闭（含强杀）时释放；同目录的 owner 元数据（PID、启动时间、
  版本、角色、操作、心跳）只用于诊断，取得锁即覆盖上一次的 stale 记录，不按 PID 回收。锁库、
  锁日志与 owner 元数据不参与作品数据库发现、灾备复制或未引用文件判定。
- `CommandContext` 携带 `OperationId`、`OperationDomain`、当前运行分类、`CancelToken` 与仓库 key；
  use-case 只能通过它取得已加锁的 `RepositorySession`，不再自行读取设置或拼锁。
- typed `CommandError` 携带稳定 `code`、`kind`、用户文案、`retryable` 与 operation id。Tauri 适配器
  仍把错误归一为字符串（保持 0.2 的 `invoke` 失败形态与文案）；结构化字段进入事件和日志。前端
  `invokeCommand` 识别结构化错误对象时重建为带可选字段的 `Error`，字符串错误原样保留。
- `AppEvent` v1 信封字段（repository key、operation id、domain、phase、current/total、
  cancellable、completion revision）与首批事件（`operationStarted` / `operationProgress` /
  `operationFinished` / `repositoryChanged` / `cleanupQueueChanged`）已冻结；阶段 3 只补订阅与
  轮询降级，不重定义事件。进度事件按阶段变化、完成或 100 ms 间隔节流，headless 的 marker
  观察者不节流。
- 命令注册表集中在 `app/command_registry.rs` 的 `handler()` 中按领域分组列出全部命令，`lib.rs`
  只把它交给 `invoke_handler`；Tauri 的 `generate_handler!` 需要编译期函数列表，因此这里是一张
  静态表，不是运行时注册。

## 前端模块契约与事件消费（0.3 阶段 3）

每个业务模块按同一目录契约组织，模块之间不直接导入：

- `types.ts`：边界 DTO；纯前端计算放模块自己的 model/helper 文件。
- `api.ts`：该领域 Tauri 命令的唯一前端入口，统一经 `invokeCommand` 重建结构化错误。
- `use*Controller.ts`：读取、mutation 回填、忙碌状态、错误上报与请求代次的唯一位置。
- `Module.tsx`：只做视图、弹窗、局部交互与 controller 注入。

三个共享工具避免每个 controller 复制同一段竞态代码：

- `useRequestEpoch`：`begin()` 取得本次读取编号，`isCurrent(id)` 丢弃过期结果；
  `invalidate()` 在切换仓库/作品或发起 mutation 时让在途读取失效。四个模块的
  controller 都用它，不再各自持有 `useRef` 计数器。
- `useAsyncOperation`：按计数维护 `busy`，在 `finally` 中复位；可选 `onError` 只负责上报，
  错误仍原样抛给调用方，避免「失败也被当成成功」地关闭弹窗。History、Library 与
  Authenticity 使用它；素材板同一时刻只允许一次导入或导出，忙碌由传输流程自己持有。
- `useRuntimeSubscription`：`AppEvent` v1 的统一消费入口。全进程只有一个原生监听，
  按版本、当前仓库 key 与事件时间戳过滤后分发给所有订阅者；controller 不各自 `listen`。
  消费方是 History、Library、Pin-board 三个 controller 与应用层的运行态、维护队列订阅；
  Authenticity 的写入结果只由命令响应带回，不订阅事件。

事件只做即时唤醒，命令响应仍是最终状态。运行态与历史刷新保留 3 秒快照轮询作为兜底：
事件丢失、进程启动恢复以及不经过 coordinator 的后台调度任务都由它收敛；设置页与维护操作
在收到进度事件时以 150 ms 合并读取一次运行状态。作品树另保留 30 秒刷新，用于后台自动备份
失败告警这类没有事件的写库。订阅失败与订阅者异常只写 `[runtime]` 诊断，不改变业务路径；
事件的开始/结束、耗时与失败仍由原生 `operation` 日志记录，不重复一套通道。

素材板导入/导出的进度不再使用 `Channel`：命令在 `CommandContext` 上报告进度并广播
`operationProgress`，controller 按 `domain = pinBoard` 消费；`Channel` 只保留给需要传输
大块载荷的通道。Pin-board 的生命周期结算由 controller 注册，画板切换、仓库切换与退出
仍走 `preparePinBoardRuntimeChange` 的既有语义。

## 日志与诊断

原生端启用 Tauri 日志插件，把日志写入操作系统应用日志目录（Windows 默认位于
`%LOCALAPPDATA%\com.lilith.artworks\logs`），单个文件达到 4 MiB 后轮转并保留最近 5 份。
插件构建时刻意不调用 `.level()`：插件内部用 fern 的 dispatch 级别做静态过滤，一旦在构建时
固定，运行时就无法再提级。等级因此完全由 `app::diagnostics::apply_level`
（`log::set_max_level`）控制，是唯一的等级开关。

等级分三层，生效值取最大者：

- **基线**：`LILITH_LOG_LEVEL`（`off`/`error`/`warn`/`info`/`debug`/`trace`）指定；未设置时
  调试构建为 `debug`、发布构建为 `info`。
- **诊断模式**：`AppState` 内的进程级开关，不写入设置文件；开启时要求至少 `debug`。
- 因此环境变量把基线抬到 `debug`/`trace` 后，关闭诊断模式也不会降级。

诊断模式默认跟随基线（调试构建默认开启、发布构建默认关闭），设置页「调试」栏目的
「详细日志」开关可随时切换且无需重启：`set_diagnostics_enabled` 立即更新全局
过滤器并广播 `diagnostics_mode_changed`，前端据此即时启停周期性探针。关闭时应用仍记录常规档
事件（生命周期、异常、阈值告警），只是不含周期性探针与取证通道。UI 只暴露「常规」与「诊断」
两档，更细的 `trace` 仅经 `LILITH_LOG_LEVEL` 启用，不进设置页。分档原则与新增日志的取舍见
[日志与诊断规范](../guides/logging.md)。

前端通过 `log_frontend_diagnostics` 把事件转发进同一份日志（WebView 侧没有写日志文件的路径），
统一使用 `webview` target。常规档写入事件驱动的低频记录（生命周期、保存、结算、切换、GPU 设备
健康）与阈值告警；诊断档追加周期性探针与取证通道，但探针只在异常时记录：主线程心跳每 2 秒刷新
一次取证通道（不写日志），500 ms 的事件循环延迟探针仅在明显迟到时记 `[lag]`，长任务观察器仅在
超过阈值时记 `[task]`。

原生端另有一个看门狗线程：诊断模式下每 2 秒向前端发 ping 并要求立即 pong。前端主线程一旦冻结
就无法应答，由原生端线程写出 `[watchdog] webview unresponsive` 告警，恢复后再记录停摆时长。
这是唯一能在冻结进行中留下证据的位置——前端此时已经写不出日志，而该记录还能扛过随后的 15 秒
兜底强退。把看门狗告警、取证记录与 `shutting down` / `forcing exit` 对齐，即可区分「监听器未被
调用（主线程冻结）」与「结算未返回（等待命令）」。

看门狗只能证明「主线程停了」，证明不了「停在哪一步、停了多久、有没有恢复」。这部分由
`src/shared/diagnosticsWorker.ts` 补上（仅诊断档启用）：一个独立线程的取证 Worker，主线程按
节流间隔向它上报「最近操作」，Worker 在静默超过阈值时把「最后心跳时刻 / 最后操作 / 静默时长」
写入 IndexedDB，下次启动回读并写成 `[forensic] previous freeze` 日志。之所以必须走 IndexedDB：
日志与心跳本身都要经过那条已经死掉的 IPC 通道，而 IndexedDB 由浏览器进程落盘，不经过主线程；
`localStorage` 在 Worker 里不可用，且同样挂在主线程上。主线程若在判定冻结之后恢复，会补记恢复
时刻，用于区分「永久挂起」与「长停顿后恢复」。

日志按「运行 + 等级 + target + 标签」建立索引：每行前缀为 `时间[run:xxxxxx][等级][target]`，
消息内再带方括号标签（`[hb]` / `[lag]` / `[task]` / `[slow]` / `[forensic]` / `[watchdog]` /
`[gpu]` / `[error]`），便于从按大小轮转的文件中按运行切片、按来源或标签筛选。标签含义与筛选
方式见 [日志与诊断规范](../guides/logging.md)。

前端还接管了 `error` / `unhandledrejection`（此前完全不可见），脚本异常与资源加载失败分开记录；
渲染器为每个 WebGPU 设备注册 `device.lost` 与 `device.onuncapturederror`，捕获错误作用域之外被
静默吞掉的 GPU 错误。

渲染器创建/销毁、GPU 设备序号与存活渲染器数、GPU 设备丢失与未捕获错误、画板保存与结算、
纹理批次加载、退出握手各阶段与仓库切换结算同样写入常规档日志。仓库读写与锁等待、纹理批次、
长任务只在超过阈值时告警，常速下零噪声；素材板各读命令通过 `with_repository_read_labeled`
带上命令标签，使慢读能归因到具体命令。

设置页提供配置文件夹、诊断日志文件夹和 About/Legal 入口；后者显示版本与
`Copyright 2026 ACSProgram`，并打开随包法律材料。日志记录启动、退出、窗口/托盘状态保存失败和
自动备份失败等诊断事件；私钥、证书内容、Artwork 内容和完整认证声明不得写入日志。

## 聚合模型

- Library tree：分组和 Artwork 构成任意深度树，Artwork 是叶节点。
- Artwork：作品级聚合根，拥有多个 branch。
- Branch：命名工作线，绑定唯一源文件路径和一个可变 head 指针。
- History node：不可变提交，只有一个父节点，可以被多个后继节点引用；创建分支是从选定节点派生新的 branch/head。
- Final artifact：分支可选的一份最终成品，固定关联进入发布状态时的 head。存在时分支被冻结，该 head 是强制检查点。
- Certification record：不可变导出快照，固定关联 final artifact、branch 和发布节点，记录 TrustMark ID、输出文件摘要、C2PA manifest 与验证状态。
- Pin board：Artwork 内平铺一层的画板聚合，拥有图片记录、step 历史、图层/顺序与回收站状态；不进入分支历史，DDS 实体存于 `boards/`。

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
    boards/<board-id>/<image-id>.dds
    temp/
  temp/
```

数据库持有元数据和仓库内文件的相对路径。snapshot、delta 和最终成品先写临时目录、同步并以不覆盖方式发布，最后在 SQLite 事务中切换引用；认证导出由用户选择外部绝对路径，同时记录 SHA-256。删除类跨边界操作先在同一事务写入 `pending_file_cleanup`，提交后执行文件清理；失败保留并可重试，应用启动时自动处理历史遗留项。

普通数据库连接只使用 SQLite `READ_WRITE` 打开现有 `lilith-artworks.sqlite3`，不得带 `CREATE`。仓库目录和新 schema 创建只从 Library 显式初始化入口进入。`AppState` 缓存当前路径已通过完整校验的事实：首次访问、启动状态读取和设置保存执行 SQLite 完整性、外键、实体 UUID、受控相对路径、持久化 SHA-256 与 schema 版本检查，之后所有前台命令和后台调度统一执行轻量 format/version 检查。数据库被外部删除、清空或替换为其他格式/不受支持版本时会清除缓存并报告仓库不可用，不创建占位数据库。

提交、完整创建分支、历史删除/精简、进入/取消发布、认证发布、Artwork 永久删除、清理重试、仓库完整性扫描、整仓灾备和仓库设置保存经 `OperationCoordinator` 取共享 `BackupState` 运行锁与仓库 lease，再调用各领域公开服务；前端 busy 状态只负责交互反馈，不承担并发正确性。共享运行状态公开当前任务类型（自动备份 / 空闲链路校验 / 用户操作）：前台长命令在取得锁前登记前台等待并请求后台任务让位，调度器据此让位，并在仓库空闲时按 Artwork 并行（最多 4 个）对分支 head 做链路完整性校验，写回仍串行；连续 4 次自动备份后若已有就绪校验，先跑一轮校验防饥饿。设置页的仓库完整性操作逐节点物化历史链，并校验最终成品、认证副本、画板 DDS 及 C2PA 声明；同一取消命令可以中断节点之间的扫描。素材板命令走 `run_repository_write`，因此不持有共享运行锁：浏览走共享读租约，变更只持有仓库 lease + 仓库操作锁，长灾备或扫描运行期间的画板写入会等待操作锁释放。

整仓灾备核心归 `backup/repository_backup.rs` 所有，但由 `app/workflows.rs` 同时取得共享运行锁和仓库 lease 后调用。它先 checkpoint WAL，再复制仓库内普通文件；复制阶段同时计算完整 SHA-256 与首尾各 64 KiB 抽样摘要并写入 v2 manifest，领域校验由应用层注入的 `RepositoryIntegrityProvider` 完成，最后以同卷目录重命名发布。发布前复核按 `Sample` 模式只重读首尾采样（`Full` 作为内部强校验选项保留），同时核对文件集合与大小。前端只能提交文件选择器授权的目标父目录；临时 bundle 在失败或取消时清理，成功 bundle 内的 `repository/` 可作为新仓库打开。该流程不复制仓库外的分支工作文件。

## 增量历史策略

继续使用 LilithClient 的内容定义分块与 SHA-256。每个 branch head 保留完整 snapshot；父节点默认可由子 snapshot 加反向 delta 恢复。一个节点被多个分支/子节点引用时，清理逻辑按全图引用决定，不能再采用“旧 head 必删”的线性假设。

历史节点删除分两步：先在事务内计算并删除选定节点的完整后代集合、把受影响分支回退到最近未删除祖先；事务提交后再删除不再引用的文件。中间节点精简只处理单子节点普通节点，会用 ChunkFile 重新生成父子反向 delta 后再原子改接历史边。

## 认证边界

分支最终成品是认证输入，不替代备份历史。进入发布状态时先强制固化当前 head，再把成品复制进仓库并锁定分支。每次导出都强制签入 C2PA，TrustMark 可选；成功回读 C2PA 后才记录认证。TrustMark/C2PA 声明 ID 建普通索引以支持全库快速候选匹配；随后仍需比较记录、文件哈希、C2PA 验证状态和 soft binding，不能把模型解码结果单独视为认证成功。

进入发布状态由应用工作流先读取认证发布目标，再通过公开 backup 能力建立检查点，最后调用认证服务保存最终成品；认证模块不读取 ChunkFile 或导入 history。history 只通过 `final_artifacts` 是否存在判断分支锁定，不导入认证实现。前端跨 Artwork 的溯源跳转由应用层完成，认证模块不导入 Library。
