# 素材板模块

## 上下文入口

- 页面、工具栏、快捷键绑定：`src/modules/pin-board/PinBoardModule.tsx`
- WebGPU/Canvas 渲染与编辑状态：`renderer.ts`
- 纹理请求尺寸与两级缓存预算：`texturePolicy.ts`
- 纯几何：`geometry.ts`
- 进程内视图恢复：`session.ts`
- Tauri 命令与类型：`api.ts`、`types.ts`
- Rust 数据持久化：`src-tauri/src/pin_board/repository.rs`
- DDS/BC7 图像处理与纹理缓存：`src-tauri/src/pin_board/dds.rs`
- schema v2 表定义：`src-tauri/src/library/schema.rs`（`PIN_BOARD_TABLES_SQL`）

当前批次状态见 `docs/planning/current-handoff.md`，未完成事项见 `docs/planning/todo.md`。

只改画板列表或设置时不需要读取 `renderer.ts`；只改几何算法时不需要读取页面组件和 Rust 数据解析。

## 数据所有权与边界

素材板是第五个领域模块，以 **Artwork 为单位**：一个 Artwork 内多块画板，平铺一层，无文件夹层级；列表顺序由 `sort_order` 决定，可在侧栏拖放调整。画板不纳入分支历史（不进增量提交、不参与恢复/裁剪）。

整仓灾备按仓库目录递归复制全部普通文件，`artworks/<artwork-id>/boards/**/*.dds` 因此已在副本与 `manifest.json` 逐文件 SHA-256 清单之内，恢复后的仓库可直接打开并读取画板。设置页的"仓库完整性"扫描目前只覆盖历史链与认证受控文件，尚未校验画板 DDS 的内容语义，该缺口记在 `docs/planning/todo.md`。

前端渲染器通过 revision 做保存冲突保护，纹理由 Rust 校验并按所需尺寸读取。模块入口位于 Artwork 工作区的一个标签页，**保持挂载**：切换工作区视图只暂停全局键盘交互与在途纹理任务，不释放 GPU 资源；重新激活后从当前视口继续补载。该标签页沿用 Client 的 keep-alive 语义，非活跃时只切换可见性（`visibility`）而不是用 `display:none`，使画布始终保有布局尺寸，渲染器首次创建即可按最小包围框完成视图适配（否则会在零尺寸画布上初始化并写入退化会话，重进时图片过小且跳过适配）。仓库切换/关闭时工作区整体卸载，画板状态随之丢弃，卸载路径执行 renderer 的保存/结算。

持久化只属于 Rust：SQLite（schema v2 三张表）保存画板、图片记录与 step 历史；BC7 DDS 实体文件存于 `artworks/<artwork-id>/boards/<board-id>/<image-id>.dds`。不迁移 Lilith Client 的旧 `index.json` 画板库。

## 并发与锁

- 普通浏览（列表、加载、纹理读取、导出 PNG 到剪贴板）走共享读租约（`with_repository_read`）；
- 保存、结算、粘贴、导入、画板增删改与回收站操作走仓库操作锁（`with_ready_repository`＝仓库租约 + `repository_operation` 互斥锁）。`pin_board` 命令直接在 `lib.rs` 注册，不经 `app/workflows.rs`，因此不持有 `BackupState` 运行锁；
- 灾备、scrub、仓库完整性操作持仓库操作锁期间，画板写入会被阻塞（读取仍可继续）。

## 存储布局（schema v2）

```text
<repository>/artworks/<artwork-id>/
  boards/<board-id>/<image-id>.dds    ← 图片实体（BC7，沿用归一化规则）

SQLite:
  pin_boards        id, artwork_id, name, sort_order, now_step, max_step,
                    revision, deleted_at(回收站软删除), 时间戳
  pin_board_images  id, board_id, file_path, 逻辑宽高
  pin_board_history (board_id, image_id, step) → deleted, layer, sort_order,
                    transform_json(points/uv)
```

- `revision` 每次画板内容写库（`save_pin_board`/`finalize_pin_board`）后单调更新，用于保存冲突检测；`reorder_pin_boards` 只调整 `sort_order`，不改 `revision` 与 `updated_ms`，因此重排不会让已打开画板的下一次保存被误判为冲突；
- DDS 落盘（`persist_dds_file`）会先确保画板目录存在，仓库数据迁移后目录缺失时自动补建；
- 画板删除 = 软删除（`deleted_at`）；Artwork 进入项目回收站时其画板随之隐藏；Artwork 永久删除时 `pin_boards` 行随外键级联删除，DDS 目录随 `artworks/<artwork-id>` 目录一并进入清理队列；
- 画板回收站的永久删除/清空经 `pending_file_cleanup` 以 `repository_directory` 条目清理 `boards/<board-id>` 目录，失败保留并在下次启动重试。

## 命令面

- 画板管理（`api.ts` → Tauri 命令）：
  - `list_pin_boards(artworkId)`、`list_pin_board_trash()`（全局列表，含原属作品标题）
  - `create_pin_board`、`rename_pin_board`、`reorder_pin_boards(artworkId, boardIds)`（侧栏拖放排序）、`trash_pin_board`、`restore_pin_board`（恢复回原 Artwork）、`delete_pin_board_permanently`、`empty_pin_board_trash`

画板内容：

- `load_pin_board`、`save_pin_board`、`finalize_pin_board`
- `paste_pin_board_images`、`import_pin_board_images`（文件路径）、`import_pin_board_clipboard_image`（剪贴板位图/文字素材）——三者统一写入归一化 BC7 DDS，导入/导出使用 Tauri IPC channel 逐项回传进度
- `export_pin_board_images`（PNG 到用户选择目录）、`read_pin_board_image_png`（系统剪贴板复制）、`read_pin_board_texture`（画板 ID + 图片 ID + 请求长边）
- `read_pin_board_clipboard_paths`（Windows `CF_HDROP`）

## 领域行为（沿用 Client 已验证实现）

交互层（renderer、几何、纹理策略、会话、快捷键、画布交互）原样迁移，未重写。要点：

- 纹理两级缓存与淘汰策略、8192 纹理上限契约、BC7 解码预览见 `texturePolicy.ts` 与 `dds.rs`；缓存预算由设置的 `textureCacheLevel` 决定（Rust 结果缓存低 64 / 中 128 / 高 256 MiB，叠加前端 GPU 常驻缓存后总量约 256 / 512 / 1024 MiB，设置页按总量标注，与 Client 一致）；
- 图片状态沿用 step 模型：新增图片先有 step 0 的删除态默认节点，添加/变换/删除作为新步骤写入；普通保存追加当前历史节点并作废 redo，`finalize_pin_board` 截断未来步骤、清除仍为删除状态的图片记录及对应 DDS；
- 图层由 `layer`（底层/中层/顶层）与 `sort_order` 共同决定；新图片统一进入中层并占据最优先顺序；
- 阵列排序使用总宽度平方根估算，间距由设置的 `arrangementGapPx`（默认 10 CSS 像素）换算为世界单位；
- “添加文字”生成透明 PNG 素材，尺寸补齐到 4 像素压缩块边界，走普通图片导入链路；
- 打开画板按未删除图片的最小包围框自动适配视图；“重置视图”回到该包围框；
- 全屏为窗口状态：模块在挂载时与原生全屏标志对齐，Escape 与全屏快捷键只在模块活跃时生效；
- 锁定与全屏快捷键默认为 `Ctrl+R` 与 `F11`，可在设置弹窗“素材板”页自定义（沿用 Client 的快捷键录入控件）；F5 / `Ctrl+R` 整页刷新由应用层的 `preventWebViewReload` 只取消默认行为来屏蔽，不停止事件传播，因此 `Ctrl+R` 仍能命中锁定快捷键，避免 WebView 刷新丢失画布状态（与 Client 的实现一致）；设置 v1 中仍是旧默认值的 `CommandOrControl+Shift+K` 在读取时迁移为 `CommandOrControl+R`（见 `docs/architecture/overview.md`）；
- 画板列表为空或未选中画板时，画布区域显示“当前未选择素材板”等占位提示，不显示加载动画；
- 侧栏画板支持拖放排序（沿用 Client 的拖放语义）：拖动行到目标行的上/下半区决定插入到前/后，落库前先本地乐观重排，失败回滚；`reorder_pin_boards` 要求传入的 id 集合与当前未删除画板完全一致（缺项、重复或跨作品 id 一律拒绝），顺序只是列表元数据，不影响已打开画板的编辑会话；
- 将当前选中画板移入回收站时以 `destroy(false)` 释放渲染器，跳过针对已删除画板的 finalize 保存，避免时序上的写库失败。

## 快速验证

- `npm run test:pin-board`：素材板及其边界的确定性测试（几何、阵列、会话隔离、快捷键、拖放排序、renderer 保活、两级缓存、纹理尺寸/内存策略、前后端上限契约），不启动 Tauri；
- `cargo test pin_board --lib`：DDS 尺寸/预览/负载边界、纹理缓存淘汰、画板 CRUD/回收站/排序/迁移（临时目录 SQLite），不依赖 Tauri 运行时；
- 完整编译与 GUI 手工验收（导入/导出、大图、缓存、回收站、灾备恢复后画板可用）由维护者执行，结果记录在 `docs/planning/current-handoff.md`。
