# 素材板模块规划（Lilith Client 迁入 + 存储重构）

状态：**规划已确认，未开始实施**。启动前提：`v0.1.0` 人工验收与发布闭环完成。
本文件独立维护，实施过程中的未完成项按项目规则记入 `docs/planning/current-handoff.md`，完成后归档并从当前规划中移除。

## 背景与目标

素材板（自由画布图片工作台）目前位于 Lilith Client（`F:\programs\Lilith Client`），与该应用"个人工具箱"的定位不符；它本属美术创作工作流，应归属 Lilith Artworks。本次将其作为第 5 个领域模块迁入，同时重构持久化层：

- 交互层原样迁移，不重写（renderer、几何、纹理策略、会话、快捷键、画布交互均沿用已验证实现）；
- 数据层按新设计重写：抛弃 `index.json` + 画板树/文件夹模型，改为 SQLite 元数据 + BC7 DDS 文件；
- 不迁移旧库数据，视作全新模块（Lilith Client 侧旧实现保持原样直至本模块验收，之后由用户决定是否从 Client 移除）。

## 已确认的设计决策

| 决策点 | 结论 |
| --- | --- |
| 归属 | 素材板以 **Artwork 为单位**；一个 Artwork 内多块画板，**平铺一层**，无文件夹嵌套 |
| 存储位置 | 存入**作品仓库**（不设独立素材库路径），schema v1 → v2 |
| 版本边界 | 画板**不纳入分支历史**：不进增量提交、不参与恢复/裁剪；整仓灾备完整复制 `boards/` |
| 图片格式 | 保留 BC7 DDS 实体文件；SQLite 只存元数据与相对路径 |
| 撤销/恢复 | 沿用现有 step 模型，**持久化**到 SQLite，跨应用重启可用 |
| 画板删除 | **画板回收站**：软删除 → 可恢复/永久删除/清空；DDS 经 `pending_file_cleanup` 清理 |
| 规模假设 | 单 Artwork ≤ 10 板、每板几百张 → 纹理缓存默认中档预算，灾备扩围耗时可接受 |
| 旧库迁移 | 不迁移；新模块从空仓库起步 |

## 架构设计

### 存储布局

```text
<repository>/artworks/<artwork-id>/
  boards/<board-id>/<image-id>.dds    ← 图片实体（BC7，沿用现有归一化规则）
  ...

SQLite（schema v2 新增表）:
  pin_boards        id, artwork_id, name, sort_order, deleted_at(回收站软删除), 时间戳
  pin_board_images  id, board_id, 相对路径, 逻辑宽高, 显示宽高, 变换(旋转/缩放/翻转),
                    layer, order, step(deleted 标记，沿用现有历史模型)
  pin_board_history 画板级操作步骤记录（撤销/恢复持久化）
```

- `boards/` 文件纳入仓库文件清单，参与完整性 scrub 与整仓灾备副本清单。
- 删除类操作（永久删除、清空回收站、Artwork 永久删除级联清理画板）沿用 `pending_file_cleanup` 队列与失败重试机制。
- schema 迁移遵循 `library/schema.rs` 既有模式（`SCHEMA_VERSION` 递增、追加式迁移、新建仓库直接以当前版本落库）；不提供回退到 v1 的迁移。

### 模块结构与契约

```text
src/modules/pin-board/     前端领域模块；api.ts 是该领域 Tauri 命令唯一入口
src-tauri/src/pin_board/   Rust 领域模块（持久化、DDS 归一化、纹理读取与缓存）
docs/modules/pin-board.md  模块文档（以 Client 侧文档为底稿按新设计改写）
```

**原样迁移（不改行为）**：`renderer.ts`、`geometry.ts`、`texturePolicy.ts`、`session.ts`、`shortcuts.ts`、`PinBoardModule.tsx` 的画布交互（视口、选中/缩放/旋转、阵列排序、图层、右键菜单、添加文字、导入/导出、进度提示、全屏）、两级纹理缓存与淘汰策略、`read_pin_board_texture` 契约（画板 ID + 图片 ID + 请求长边）。对应测试（`*.test.ts`、Rust `pin_board` 测试）一并迁移。

**重写（仅数据访问层）**：`api.ts` 命令映射与 `pin_board` Rust 持久化。删除的旧能力：画板树/文件夹层级、树拖拽、树节点回收站、`index.json` 读写、`pinBoardLibraryPath` 配置与 `beforeConfigChange` 结算钩子。新增能力：按 Artwork 的画板列表（平铺）、画板回收站。

**命令面（初稿，实施时定稿）**：画板列表/创建/重命名/移入回收站/恢复/永久删除/清空回收站；画板加载/保存/结算；图片导入（剪贴板位图、剪贴板路径、文件选择）/导出/纹理读取/导出 PNG/复制。导入导出继续用 Tauri IPC channel 逐项回传进度。

### 生命周期与并发

- 模块入口位于 Artwork 工作区（选中 Artwork 后的一个视图）；工作区内保留 keep-mounted 语义：切换视图只暂停全局键盘交互与在途纹理任务，不释放 GPU 资源。
- 仓库切换/关闭时工作区整体卸载，画板状态随之丢弃，无需 Client 的配置切换结算钩子；卸载前执行一次画板保存/结算，失败即阻止仓库切换。
- 画板读写接入共享读租约/互斥写锁：普通浏览走共享读租约；导入/导出/保存/删除走互斥写锁；灾备、scrub、仓库完整性操作持锁期间画板文件访问被阻塞。
- 纹理缓存等级（`pinBoardTextureCacheLevel`）与阵列间距（`pinBoardArrangementGapPx`）并入应用版本化设置。

### 画板回收站语义

- 删除画板 = 软删除（`deleted_at`），DDS 立即停止参与加载与缓存调度；
- 回收站入口在素材板模块内，全局列表并显示原属 Artwork；恢复回原 Artwork，原 Artwork 已永久删除时不可恢复并标注；
- 清空回收站/单项永久删除 = 删除记录 + 经 `pending_file_cleanup` 清理对应 DDS 目录；
- Artwork 进入项目回收站时其画板随之隐藏；Artwork 永久删除时画板记录与 DDS 一并级联清理。

## 分阶段计划

| 阶段 | 内容 | 验证 |
| --- | --- | --- |
| P1 持久化层 | schema v2（三张表 + 迁移）；`src-tauri/src/pin_board/` 新持久化：画板 CRUD、图片记录、step 历史持久化、DDS 写入路径调整、回收站操作；接入租约/写锁 | `cargo test pin_board`、迁移用例（v1 仓库升级到 v2） |
| P2 模块迁入 | 前端模块与测试整体迁入；`api.ts` 映射新命令；Artwork 工作区挂载入口、keep-mounted 生命周期；设置页接入缓存等级与间距；Tauri capability 补条目 | `npm run test:pin-board`、`tsc`、前端 `npm test` |
| P3 回收站与清理 | 画板回收站 UI 与恢复/清空流程；`pending_file_cleanup` 接入；Artwork 删除级联清理 | 确认弹窗、清理重试、恢复语义的针对性测试 |
| P4 扩围与验收 | 整仓灾备复制 `boards/` 并入清单校验；scrub 扩围；仓库完整性扫描覆盖画板文件；模块文档 `docs/modules/pin-board.md` 定稿；README 功能列表更新 | 完整编译 + 维护者 GUI 手工验收（导入/导出、大图、缓存、回收站、灾备恢复后画板可用） |

每阶段独立提交，文档与对应代码同阶段交付；提交信息按项目 Git 管理规则由代理给出、用户手动提交。

## 风险与注意事项

- 交互层迁移期间禁止"顺手重构"渲染器；发现缺陷记入 handoff，另行处理。
- 纹理读取持共享租约在灾备长时间运行时会造成画板浏览卡顿等待；规模假设下可接受，实施时如实测不可接受再评估灾备期间的降级策略（如缩略图兜底提示）。
- Client 与 Artworks 将短暂并存两份实现；Client 侧在本模块验收前不做任何同步修改，避免漂移。
- "添加文字"生成的文字素材与普通图片同轨持久化，不需要单独建模。

## 参考源

- Lilith Client 模块实现：`F:\programs\Lilith Client\src\modules\pin-board\`、`src-tauri\src\pin_board\mod.rs`
- Client 侧模块文档：`F:\programs\Lilith Client\docs\modules\pin-board.md`
- 本仓库存储与迁移模式：`docs/architecture/overview.md`（存储布局、运行锁）、`src-tauri/src/library/schema.rs`
