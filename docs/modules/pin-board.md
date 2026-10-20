# 素材板模块

## 上下文入口

- 页面、工具栏、快捷键绑定：`src/modules/pin-board/PinBoardModule.tsx`
- WebGPU/Canvas 渲染与编辑状态：`renderer.ts`
- 纹理请求尺寸与两级缓存预算：`texturePolicy.ts`
- 纯几何：`geometry.ts`
- 选择判定（加选修饰键、框选命中）：`selection.ts`
- 进程内视图恢复：`session.ts`
- Tauri 命令与类型：`api.ts`、`types.ts`
- 绘制、度量与导出共用的文字布局/栅格化：`textLayout.ts`
- 添加文字对话框与内置取色器：`PinBoardModule.tsx`、`PinBoardColorPicker.tsx`
- Rust 数据持久化：`src-tauri/src/pin_board/repository.rs`
- DDS/BC7 图像处理与纹理缓存：`src-tauri/src/pin_board/dds.rs`
- 阵列排序（统一高度、阅读序、断行、落位）：`src-tauri/src/pin_board/arrange.rs`
- 画板 DDS 双向完整性扫描：`src-tauri/src/pin_board/scrub.rs`
- 素材板表定义（仓库 schema v5 起为多态条目）：`src-tauri/src/library/schema.rs`（`PIN_BOARD_TABLES_SQL`；v1→v2 的旧形状冻结为 `PIN_BOARD_TABLES_V2_SQL`）

当前批次状态见 `docs/planning/current-handoff.md`，未完成事项见 `docs/planning/todo.md`。

只改画板列表或设置时不需要读取 `renderer.ts`；只改几何算法时不需要读取页面组件和 Rust 数据解析。

## 数据所有权与边界

素材板是第五个领域模块，以 **Artwork 为单位**：一个 Artwork 内多块画板，平铺一层，无文件夹层级；列表顺序由 `sort_order` 决定，可在侧栏拖放调整。画板不纳入分支历史（不进增量提交、不参与恢复/裁剪）。

整仓灾备按仓库目录递归复制全部普通文件，`artworks/<artwork-id>/boards/**/*.dds` 因此已在副本与 `manifest.json` 逐文件 SHA-256 清单之内，恢复后的仓库可直接打开并读取画板；文字条目没有实体文件，随数据库一起进入备份。设置页的"仓库完整性"扫描已覆盖位图 DDS 的双向检查：逐条 `pin_board_bitmaps` 记录校验 DDS 存在、路径归属（`<item-id>.dds`）、DDS/DX10/BC7 头、声明尺寸与记录一致、数据长度与 BC7 解码，并统计无记录的孤儿 DDS；文字条目没有 DDS，不进入这项检查。`pin_board_bitmaps` 无摘要列，故不比对 SHA-256。孤儿 DDS 同时进入 `cleanup::scan_unreferenced` 的报告与确认清理流程。只报告不自动修复。

前端渲染器通过 revision 做保存冲突保护，纹理由 Rust 校验并按所需尺寸读取。前端分为 controller 与纯渲染器两层：`usePinBoardController.ts` 持有画板列表与选择、保存/导入/导出状态、错误状态，并注册 `preparePinBoardRuntimeChange` 使用的生命周期参与者；`PinBoardModule.tsx` 只负责视图、弹窗与局部交互，`PinBoardRenderer` 只处理 GPU、画板装载与绘制。导入/导出/文字的进度经 `CommandContext` 广播 `AppEvent.operationProgress`（`domain = pinBoard`），不再使用 `Channel`；`Channel` 只保留给需要传输大块载荷的通道。控制器统一使用共享 `useRequestEpoch` 做画板/作品切换的读取代次；不允许进代次的画板列表写回（新建、重命名、回收站、排序）在回填前比对作品 id，因此旧作品或旧画板的异步结果不会覆盖当前画板。设置契约 `PinBoardModuleSettings` 与其它边界类型一起放在 `types.ts`，控制器不再反向导入组件模块。

模块入口位于 Artwork 工作区的一个标签页，**保持挂载**：切换工作区视图只暂停全局键盘交互与在途纹理任务，不释放 GPU 资源；重新激活后从当前视口继续补载。该标签页沿用 Client 的 keep-alive 语义，非活跃时只切换可见性（`visibility`）而不是用 `display:none`，使画布始终保有布局尺寸，渲染器首次创建即可按最小包围框完成视图适配（否则会在零尺寸画布上初始化并写入退化会话，重进时图片过小且跳过适配）。

**渲染器与 GPU 设备在模块生命周期内只创建一次**：`PinBoardRenderer.create` 只做画布生命周期内的一次性初始化（适配器/设备/上下文/管线/采样器/`ResizeObserver`/主题观察器/事件监听），画板数据由 `loadBoard(view, session, stagePrevious)` 装载。切换画板与切换作品都只换数据、复用同一设备，且**装载是同步的**：先把旧画板的未保存改动导出成快照交给后台结算队列（`pendingSettle.ts`），再**释放旧画板的全部纹理**（切换即全释放，不跨画板保留）并装载新画板，同一 `boardId` 幂等；`stagePrevious=false` 用于旧画板已被删除或移入回收站、不能再次写库的场景；`unloadBoard` 用于当前作品已无画板时清空画面。`destroy()` 只用于模块真正卸载：仓库切换（`LibraryModule` 以仓库路径为 key 整块重建）、切到非作品节点或退出。这消除了旧实现「切换即创建/销毁渲染器与 WebGPU 设备」的 churn——该 churn 已确认会触发 WebView2 渲染进程冻结。

**切换不等仓库写入：旧画板的改动交给后台结算队列**。此前 `loadBoard` 要先以旧 `boardId` 完成 `save_pin_board` + `finalize_pin_board` 才装载新画板，切换因此被仓库写锁绑架：维护扫描、恢复、精简这类长任务持锁几十秒到几分钟时（实测一次仓库完整性扫描持锁 111 秒），交互写排队 10 秒后按 `repository_busy` 失败，界面停在新作品、画布停在旧画板。现在 `loadBoard` 只做同步的状态交接，写回交给 `pendingSettle.ts` 的串行 worker，因此切换本身不取写锁，后台任务再长也不影响切换。两条硬约定：

- **没有未保存改动就不产生快照**。`dirty` 为假时切换完全不碰仓库——切一次作品不该在数据层变成一次写操作。软删除条目的回收（`finalize_pin_board`）也不再挂在切换上，而是由队列在对应保存成功后按 `2 s / 8 s` 补做，全部失败只写日志（数据已落库，残留由下一次保存、退出结算或设置页清理扫描收走）。
- **同一画板只有一个写者**。渲染器再次装载该画板时用 `takeBoardSettle` 取回快照内容（`dirty` 保持为真、`savedStateKey` 清空），队列随即不再负责它；若取回时队列正在写同一份内容，那次写仍会完成，但不会覆盖后续编辑。`revision` 由共享账本 `resolveBoardRevision` / `recordBoardRevision` 统一给出——渲染器装载与保存都从账本取值，避免切换期间队列已推进 revision、渲染器还拿着旧值去撞「画板已被其他窗口修改」。切换时若该画板还有一次保存在飞行，队列会先等它落地（`stageBoardSettle` 的 `after`）再写，避免同样的 revision 冲突。

队列的可见性：失败分两类。可重试的（`retryable` 为真，含 `repository_busy` 与 IO 抖动）按 `0.5 s → 15 s` 指数退避**无限重试且不弹错误**，正常情况用户只会看到一条短暂的「正在后台保存 N 块画板的改动」（延迟 800 ms 才出现，避免瞬时完成时闪一下）；不可重试的（revision 冲突、仓库只读）停止自动重试、保留快照，并在素材板页头下方常驻一条「N 块画板的改动还没写进仓库：原因」+「重试」。快照只在内存里：进程被强杀会丢掉还没写回的改动，这是为「切换永不被写锁阻塞」刻意接受的取舍；退出与切仓库经 `flushPendingSettle` 在 5 秒预算内尽量写完。

**设备丢失走限流自动重建**：`device.lost` 后经 `makeGpu` 重建设备（与创建共用同一条初始化路径）并重新武装新设备的 `lost` 监听，随后按当前画板重载纹理；每个渲染器生命周期最多重建 2 次、带 600 ms × 次数退避，重建期间暂停绘制与纹理加载，超限或重建失败即保持挂起并只上报错误。

**创建失败走常驻说明，不进入瞬时状态栏**：环境不提供 WebGPU 适配器（`requestAdapter()` 返回 null）、适配器不可用或缺少 `texture-compression-bc` 时 `PinBoardRenderer.create` 直接拒绝，此时画布区显示一条**常驻**的「素材板不可用」说明并注明原始原因，编辑控件保持禁用，同时**不再显示「画板已锁定」**——渲染器为空时的锁定状态并非用户所为，显示它只会把排查引向错误方向。这条说明不能复用 4.5 秒自动清除的状态栏：那条链路清空后画板只剩空白与禁用按钮，用户会误判成卡死。素材板是本应用**唯一依赖 WebGPU** 的模块，缺少可用适配器的环境（未开启 3D 加速的虚拟机、部分远程桌面）其余功能不受影响；运行要求见 `README.md` 的「运行环境」，验收口径见 `docs/guides/release-policy.md` 的人工门槛。

工作区挂载：历史/发布/识别三个窗格以 `artworkId` 为 key 整块重置，**素材板窗格不带 key**，因此跨作品保留渲染器与设备；仓库切换/关闭时工作区整体卸载，画板状态随之丢弃，卸载路径执行 renderer 的保存/结算。

持久化只属于 Rust：SQLite（仓库 schema v5 起的多态条目表）保存画板、公共条目、按 kind 的载荷子表与 step 历史；位图实体存为 BC7 DDS `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`，文字载荷只存数据库。不迁移 Lilith Client 的旧 `index.json` 画板库。

## 并发与锁

- 普通浏览（列表、加载、纹理读取、导出 PNG 到剪贴板）经 `OperationCoordinator::run_read`
  走共享读租约；各读命令附带标签（如 `read_pin_board_texture board=.. item=.. dim=..`），使
  `repository read took N ms` 慢读告警能归因到具体命令；
- 保存、粘贴、导入、画板增删改与回收站操作经 `run_repository_write` 走仓库 lease + 仓库操作锁；
  结算（`finalize_pin_board`）改走 `run_repository_write_shutdown`，以 Shutdown 优先级插入进程内
  写入闸门最前、等待上限 10 秒，前端在结算遇 `repository_busy` 时重试一次。`pin_board` 命令在
  `app/command_registry.rs` 直接注册，不经 `app/workflows.rs`，因此仍不持有 `BackupState` 运行锁；
- 灾备、scrub、仓库完整性操作持仓库操作锁期间，画板写入会被阻塞（读取仍可继续）。

## 存储布局

```text
<repository>/artworks/<artwork-id>/
  boards/<board-id>/<item-id>.dds    ← 位图条目实体（BC7）；文字条目没有文件

SQLite:
  pin_boards        id, artwork_id, name, sort_order, now_step, max_step,
                    revision, deleted_at(回收站软删除), 时间戳
  pin_board_items   id, board_id, kind(0=位图, 1=文字), created_ms
  pin_board_bitmaps item_id, file_path, 逻辑宽高
  pin_board_texts   item_id, text, font_family, font_size, color, align,
                    measured_width, measured_height
  pin_board_history (board_id, item_id, step) → deleted, layer, sort_order,
                    transform_json(points/uv)
```

- `revision` 每次画板内容写库（`save_pin_board`/`finalize_pin_board`）后单调更新，用于保存冲突检测；`reorder_pin_boards` 只调整 `sort_order`，`rename_pin_board` 只更新 `name` 与 `updated_ms`，都不改 `revision`，因此重排或改名不会让已打开画板的下一次保存被误判为冲突；
- DDS 落盘（`persist_dds_file`）会先确保画板目录存在，仓库数据迁移后目录缺失时自动补建；
- 画板删除 = 软删除（`deleted_at`）；Artwork 进入项目回收站时其画板随之隐藏；Artwork 永久删除时 `pin_boards` 行随外键级联删除，DDS 目录随 `artworks/<artwork-id>` 目录一并进入清理队列；
- 画板回收站的永久删除/清空经 `pending_file_cleanup` 以 `repository_directory` 条目清理 `boards/<board-id>` 目录，失败保留并在下次启动重试；两者都经共享 `ConfirmDialog` 确认，其中「清空回收站」的最终动作由滑动确认控件承担，单块画板的永久删除沿用点击确认；
- 画板结算清除仍为删除状态的条目时，位图条目的 DDS 同样经 `pending_file_cleanup` 以 `repository_file` 条目（原因 `pin_board_finalize`）清理：事务内入队、提交成功后由命令层单遍重放删除。提交失败则入队随事务回滚，记录与 DDS 保持一致；删除失败条目留在队列可重试（`pin_board_finalize` 条目未落库 SHA-256，重放只做引用检查后删除），不阻断结算、不循环重试。文字条目没有实体文件，结算只删除条目记录。重放的引用检查走中立契约：本模块的 `PinBoardReferenceInventory` 负责把 `pin_board_bitmaps` 记录映射成 `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`，cleanup 不再认识画板路径布局——记录删除前 DDS 视为被引用而保留，删除后才可清理；`file_path` 不是规范文件名的坏记录不参与引用，由完整性扫描报告；
- 无记录的孤儿 DDS（异常退出、手工复制或历史迁移遗留）由 `cleanup::scan_unreferenced` 扫描 `artworks/*/boards/*/` 发现，经设置页「空间」页的「文件清理」区确认后入队清理（该区同时列出待清理队列并支持单条/全部重试）；`cleanup::scrub_board_dds` 的完整性检查另按记录逐条校验 DDS 内容语义（缺失/损坏）并统计孤儿，只报告不修复。设置页「空间」页的队列与扫描入口见 `docs/modules/history-and-backup.md`。

## 命令面

- 画板管理（`api.ts` → Tauri 命令）：
  - `list_pin_boards(artworkId)`、`list_pin_board_trash()`（全局列表，含原属作品标题）
  - `create_pin_board`、`rename_pin_board`、`reorder_pin_boards(artworkId, boardIds)`（侧栏拖放排序）、`trash_pin_board`、`restore_pin_board`（恢复回原 Artwork）、`delete_pin_board_permanently`、`empty_pin_board_trash`

画板内容：

- `load_pin_board`、`save_pin_board`（`items` 为纯变换 DTO）、`finalize_pin_board`
- `paste_pin_board_images`、`import_pin_board_images`（文件路径）、`import_pin_board_clipboard_image`（剪贴板位图素材，也承接拖入导入的字节）——三者统一写入归一化 BC7 DDS，导入/导出进度经 `CommandContext` 广播 `AppEvent.operationProgress` 回传（不再使用 `Channel`，只保留给需要传输大块载荷的通道）
- `create_pin_board_text`（文本、字号、颜色、对齐、前端度量宽高与中心点）——文字条目只落数据库，不生成图片；创建后 0.3 不支持改字/改色/改字号
- `export_pin_board_images`（位图条目解码为 PNG 到用户选择目录）、`write_pin_board_item_png`（前端按导出倍率栅格化并在浏览器编码 PNG，原生端校验格式、尺寸与像素预算后原子写盘）、`read_pin_board_image_png`（系统剪贴板复制，仅位图）、`read_pin_board_texture`（位图读取 DDS；文字返回“无 DDS”信号，前端栅格化）
- `read_pin_board_clipboard_paths`（Windows `CF_HDROP`）

**仓库外路径的两道闸门**与认证模块同一口径，各管一件事：

- `import_pin_board_images` / `export_pin_board_images` / `write_pin_board_item_png` 在命令层用
  `path_scope::ensure_authorized` 要求每条源图路径与导出目录都已在 Tauri 文件系统 scope
  中（原生文件选择器会自动写入）。scope 属于 UI 关注点，检查实现放在
  `src-tauri/src/path_scope.rs`，认证与素材板共用，领域模块之间不互相导入；
- `repository::import_images` / `export_images` / `write_text_png` 在领域层用
  `storage::ensure_outside_repository` 拒绝仓库内部的源文件与导出目录——GUI 与无头入口
  都会经过本函数，因此这一道对两条链路同时生效。

「复制文件后粘贴」仍可用：`read_pin_board_clipboard_paths` 由本命令自己从系统剪贴板读出
路径，并把确实存在的文件加入 scope，随后仍走同一条授权检查——路径不是渲染进程传进来的
值，所以这不构成绕过。粘贴**纯文本路径**不再导入（那种路径没有任何授权来源，Rust 层会
以「未由文件选择器授权」拒绝），需要路径的入口只剩文件选择器与剪贴板文件；拖入图片走的
是字节链路（`import_pin_board_clipboard_image`），不涉及路径。

## 领域行为（沿用 Client 已验证实现）

交互层（renderer、几何、纹理策略、会话、快捷键、画布交互）原样迁移，未重写。要点：

- 纹理两级缓存与淘汰策略、8192 纹理上限契约、BC7 解码预览见 `texturePolicy.ts` 与 `dds.rs`；缓存预算由设置的 `textureCacheLevel` 决定（Rust 结果缓存低 64 / 中 128 / 高 256 MiB，叠加前端 GPU 常驻缓存后总量约 256 / 512 / 1024 MiB，设置页按总量标注，与 Client 一致）；
- 图片状态沿用 step 模型：新增图片先有 step 0 的删除态默认节点，添加/变换/删除作为新步骤写入；普通保存追加当前历史节点并作废 redo，`finalize_pin_board` 截断未来步骤、清除仍为删除状态的图片记录（对应 DDS 的删除走 `pending_file_cleanup` 提交后重放，见上节）；
- 编辑自动保存：设置页「自动保存」开关（默认关闭）开启后，模型变化（拖放结束、缩放、旋转、图层/顺序调整、删除、撤销/重做）后静默 1.5 秒即自动 `save_pin_board`，期间再次变化会重新计时，关闭开关会取消挂起的保存；`Ctrl+S`、页面隐藏（`visibilitychange`/`pagehide`）与渲染器销毁结算不受该开关影响，始终立即保存。
- 结算边界：应用层在会释放仓库或换掉画布的边界先结算当前画板，避免最后一次编辑落在已经释放的仓库上；切换作品/画板时旧画板的改动只需交给后台队列（见上节），不阻塞切换。真正退出时原生端先发 `app_shutdown_requested`，前端在设置页「关闭时保存」开关（默认开启）开启时经 `preparePinBoardRuntimeChange`（`lifecycle.ts`）结算当前画板并 `flushPendingSettle` 清空后台队列，无论成败都调用 `confirm_app_shutdown` 确认，webview 挂起或崩溃时由原生端 15 秒兜底强退；切换仓库时在 `saveSettings` 之前同样先 `await preparePinBoardRuntimeChange()`，当前画板结算失败或后台队列没清空都中止切换并保留旧仓库。生命周期细节见 `docs/architecture/overview.md`。
- 撤销历史只存在于当前会话内存，重开画板后不能撤销。
- 图层由 `layer`（底层/中层/顶层）与 `sort_order` 共同决定；新图片统一进入中层并占据最优先顺序；
- 阵列排序（`src-tauri/src/pin_board/arrange.rs`，整理与导入落位共用同一份实现）：只处理位图条目，文字条目不参与——它的四边形被拉伸时 `fontSize` 不会跟着变，字会被拉变形。算法分四步，顺序只定一次、之后不再改写：统一高度取参与图片原始高度的中位数 → 阅读序（按 y 从高到低分带，带内按 x）→ 动态规划断行（最小化各行宽度与目标行宽的偏差）→ 逐行居中并按裁剪后的外框中心平移回原位。目标行数 `round(sqrt(张数 / 长宽比))` 夹到 `1..=张数`，长宽比取图片中心按名次剔除两端各 10% 后的区间，因此一字排会排成一行、竖列会排成一列，单张离群图既不改变整体形状、也不会把整块拽向自己。间距按「统一高度的比例」计算（设置项 `arrangementGapRatio`，默认 6%，范围 0–20%，`0` 表示图片紧贴），与画布缩放无关；旧实现用屏幕像素换算间距、并在断行之后按宽度对行内与行间各做了一次排序，会把用户摆好的相对位置打乱，已移除。整理的计算经纯计算命令 `arrange_pin_board_images` 完成，前端拿到位置后仍走 `commitMutation`，撤销栈与脏标记不变；
- 拖放导入：把图片文件拖到画布区（`.pin-board-stage`）即可导入，放置点对齐鼠标落点，多张一起拖入时按前一张的显示宽度加一个与图高成比例的间距（与阵列间距同一口径）向右排开；悬停时只描一圈渐变边并在底部显示小胶囊提示，不遮盖画布内容。窗口以 `dragDropEnabled: false` 运行（Windows 上使用 HTML5 拖放的前提），因此 webview **不暴露文件系统路径**，导入走 `import_pin_board_clipboard_image` 的字节链路，只接受可被内容嗅探识别的位图（PNG/JPEG/WebP/BMP/GIF）；需要路径的 DDS/TGA 仍通过「导入图片」选择器处理。画板锁定时不导入并提示；
- Ctrl+A 全选**整个画板**（全部未删除图片，不限视口），实现为 `renderer.selectAll()`；选中态属于视图状态，因此不调整图片顺序、也不标记 dirty，锁定时不生效；
- 加选与框选：`Ctrl`/`Cmd` 或 `Shift` 都是加选修饰键（`selection.isAdditiveSelectionModifier`）。按住修饰键点击图片切换单张选中；在空白处拖拽做框选，按住修饰键时以当前选中为基底累加——可连续多次框选，已选中的不取消、框到的新图加入，不按修饰键则替换选择。命中由 `selection.marqueeSelection` 计算，只并入与框选包围框相交的未删除图片；拖拽中若选中集合未变（`selection.sameSelection`），只更新框选矩形叠加层，跳过重排、重绘与状态广播；
- Shift 旋转吸附图片的**绝对角度**（0°/15°/30°…，由 `geometry.snapRotationDelta` 反推增量），而非本次拖拽的增量，使已带偏角的图片能被精确转回 0°；
- “添加文字”创建 `kind = text` 条目：前端用 `textLayout.ts` 的固定字体族度量逻辑宽高并持久化，渲染器按请求长边（受纹理像素预算约束）用同一套度量在 2D canvas 栅格化后上传纹理；缩放只改变四边形，文字随四边形精确缩放。栅格化按字形墨迹（`actualBoundingBoxAscent/Descent`）把文字块在每个 1.3 倍行盒内垂直居中，并以此定位基线——此前用 `textBaseline = "top"` 把行高余量全留在下方，文字因此整体偏上、顶部越出纹理被裁掉；显示纹理与导出共用这条绘制路径，存量条目也会按新定位重绘。对话框提供内容预览、字号步进与内置取色器（饱和度/亮度面板、色相条、十六进制输入和预设色），不再调用系统取色器。导出时按逻辑长边的 2 倍（受 8192 与 64 MiB RGBA 像素预算约束）栅格化，由浏览器编码 PNG 后经 `write_pin_board_item_png` 校验并原子写盘——传的是压缩后的 PNG 而不是 RGBA，避免一次 IPC 搬运数千万个数字。文字条目没有 DDS，也不参与 DDS 引用、清理、scrub、磁盘空间统计和画板内复制；系统剪贴板复制只支持位图；
- 打开画板按未删除图片的最小包围框自动适配视图；“重置视图”回到该包围框；
- 全屏为窗口状态：模块在挂载时与原生全屏标志对齐，Escape 与全屏快捷键只在模块活跃时生效；
- 锁定与全屏快捷键默认为 `Ctrl+R` 与 `F11`，可在设置弹窗“素材板”页自定义（沿用 Client 的快捷键录入控件）；F5 / `Ctrl+R` 整页刷新由应用层的 `preventWebViewReload` 只取消默认行为来屏蔽，不停止事件传播，因此 `Ctrl+R` 仍能命中锁定快捷键，避免 WebView 刷新丢失画布状态（与 Client 的实现一致）；设置 v1 中仍是旧默认值的 `CommandOrControl+Shift+K` 在读取时迁移为 `CommandOrControl+R`（见 `docs/architecture/overview.md`）；
- 画板列表为空或未选中画板时，画布区域显示“当前未选择素材板”等占位提示，不显示加载动画；
- 纹理未就绪的占位：画布上可见、但**缩略图与高清纹理都尚未就绪**的图片，会在其显示范围上铺一块中性灰方框（DOM 层 `.pin-board-placeholder-layer`，由 `renderer.updatePlaceholderOverlay` 按世界→屏幕变换定位），中心是旋转的强调色弧线图标；方框随平移/缩放/旋转同步，图标反向抵消图片角度保持竖直，并随方框缩放（任一边小于 18px 时只留底色）。纹理到位后由下一次 `draw()` 自动移除；节点按 `itemId` 复用，画板释放与 `destroy()` 时清理。加载失败的图片（`failedTextureDimensions` 有记录）改显**红色调**方框与静态 `!` 徽标，不再无限转圈。占位层 `pointer-events: none`，不影响拖拽、框选与拖入导入；缺失 DDS 的图片（`available: false`）不在可见集内，因此不显示占位（仍由工具栏「N 张缺失」提示）；
- 侧栏画板支持拖放排序（沿用 Client 的拖放语义）：拖动行到目标行的上/下半区决定插入到前/后，落库前先本地乐观重排，失败回滚；`reorder_pin_boards` 要求传入的 id 集合与当前未删除画板完全一致（缺项、重复或跨作品 id 一律拒绝），顺序只是列表元数据，不影响已打开画板的编辑会话；
- 将当前选中画板移入回收站时渲染器不销毁，只以「跳过旧画板结算」的方式切换：有回退画板时 `loadBoard(..., stagePrevious=false)`，无可回退画板时 `unloadBoard(false)`；已删除的画板不再暂存快照，避免时序上的写库失败。

## 快速验证

- `npm run test:pin-board`：素材板及其边界的确定性测试（几何、阵列、旋转吸附、加选框选、全选、会话隔离、快捷键、拖放排序、renderer 保活、两级缓存、纹理尺寸/内存策略、前后端上限契约、文字度量/栅格化、取色器十六进制/HSV 换算、controller 生命周期注册与进度事件），不启动 Tauri；
- `cargo test pin_board --lib`：DDS 尺寸/预览/负载边界、纹理缓存淘汰、画板 CRUD/回收站/排序、v1/v4→v5 迁移（数据/step 历史/外键/失败回滚/拒绝更新版本仓库）、文字条目与 RGBA 导出（临时目录 SQLite），不依赖 Tauri 运行时；
- 完整编译与 GUI 手工验收（导入/导出、大图、缓存、回收站、灾备恢复后画板可用、以及无可用 WebGPU 适配器的环境下显示常驻不可用说明）由维护者执行，结果先记录在 `docs/planning/current-handoff.md`，通过并归档后移入 `docs/planning/archive/`。
