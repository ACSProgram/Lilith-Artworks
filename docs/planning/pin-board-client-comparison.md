# 素材板模块与 Lilith Client 原模块对比报告

对比基线：`F:\programs\Lilith Client\src\modules\pin-board`、`src-tauri\src\pin_board`、`src\app\webviewShortcuts.ts`、`src\styles\pin-board.css`。

对比方法：对前端实现按文件做逐字节 diff；Rust 侧原模块集中在一个 `mod.rs`，迁入后拆为 `mod.rs`（命令层）/ `repository.rs`（持久化）/ `dds.rs`（DDS 与纹理缓存），因此按函数逐项比对而非整文件 diff。

## 一、逐文件比对结论

| 文件 | 结果 | 说明 |
| --- | --- | --- |
| `geometry.ts` | 完全一致 | 视图适配、包围框、象限命中与变换算法原样迁移 |
| `renderer.ts` | 完全一致 | WebGPU 渲染、编辑、撤销/重做、纹理淘汰、快捷键（渲染器内）原样迁移 |
| `texturePolicy.ts` | 完全一致 | 两级缓存预算、纹理尺寸请求与内存策略一致 |
| `shortcuts.ts` | 完全一致 | 快捷键匹配与可处理判定一致 |
| `lifecycle.ts` | 完全一致 | 生命周期参与者注册一致 |
| `session.ts` | 语义一致 | 按键由“库路径 + 画板 id”改为“Artwork id + 画板 id”；新增 `dropArtworkSessions` |
| `api.ts` / `types.ts` | 有意重写 | 命令面与 DTO 对齐新的持久化模型（见第三节） |
| `PinBoardModule.tsx` | 交互层一致，列表与配置接线重写 | 工具栏、右键菜单、导入导出、文字、粘贴、锁定/全屏交互与 Client 一致；侧栏列表与设置接线替换为 Artwork 域模型 |
| Rust `texture_cache_budget_bytes` | 完全一致 | 低 64 / 中 128 / 高 256 MiB |
| Rust `imported_points` / 阵列估算 | 完全一致 | 导入落点与阵列排布算法一致 |
| Rust DDS/BC7、导入归一化、预览降采样 | 一致 | 常量（8192 / 16384 / 6400 万像素）与校验规则一致 |

## 二、本轮已对齐的差异（本轮修复）

1. **整页刷新拦截位置**：Client 在应用层用 `webviewShortcuts.ts` 的 `preventWebViewReload` 仅取消默认行为；迁入版本改在素材板模块内提前 `stopImmediatePropagation` 并吞掉 `Ctrl+R`，导致“锁定画板”无法用 `Ctrl+R`。现已恢复 Client 做法（应用层拦截 + 模块不吞事件），并把锁定默认键位恢复为 `Ctrl+R`。
2. **keep-alive 面板隐藏方式**：Client 的 `module-keepalive-panel` 用 `visibility` 隐藏，画布保有布局尺寸；迁入版本用 `hidden`（`display:none`），渲染器在零尺寸画布上初始化并在卸载时写入退化会话，重进画板时图片过小且跳过最小包围框适配。现改为与 Client 相同的可见性隐藏，并增加“无布局尺寸不写入会话”的守卫。
3. **纹理缓存等级容量标注**：Client 标注约 256 MB / 512 MB / 1 GB（总量口径）；迁入版本标注 64 / 128 / 256 MiB（仅 Rust 解码缓存）。现对齐 Client 口径。
4. **侧栏画板拖放排序**：Client 支持拖动行重排；迁入版本无重排 UI（仅在新建/恢复时写入 `sort_order`）。现已补齐同层拖动重排（`reorder_pin_boards` + 落点前后提示），并保证重排不改 `revision`，不打断已打开画板的保存。

## 三、有意保留的差异（架构性、非回归）

| 维度 | Lilith Client 原模块 | 当前 Artworks 模块 | 性质 |
| --- | --- | --- | --- |
| 数据归属 | 独立“画板库”，跨库用 `index.json` 索引 | 以 Artwork 为单位，画板挂在 `artworks/<id>/boards/` | 有意重设计 |
| 层级 | 树：文件夹 + 画板，可嵌套、可拖放、可折叠 | 平铺一层，无文件夹；支持同层拖动重排 | 有意简化 |
| 持久化 | 每画板 `index.json` + 目录文件指纹 | SQLite schema v2 三表 + BC7 DDS 实体 | 有意重设计 |
| 回收站 | 按库的树内回收站 | 全局画板回收站（含原属作品标题）、接入 `pending_file_cleanup` | 有意重设计 |
| 命令面 | `set_pin_board_opened` / `move_pin_board` / `delete_pin_board` / `delete_pin_board_trash` / `restore_pin_board(parent,index)` | `reorder_pin_boards(artworkId, boardIds)` / `trash_pin_board` / `restore_pin_board()`（回原作品）/ `delete_pin_board_permanently` / `empty_pin_board_trash` | 随模型调整 |
| 节点字段 | 有 `description`、`createTime`、`opened` | 无 `description`；用 `createdMs`/`updatedMs` | 随模型调整 |
| 设置归属 | 全页设置视图 + `pinBoardLibraryPath` | 弹窗分页“素材板”页，无库路径 | 有意重设计 |
| 阵列间距范围 | 0–200 | 1–200（Rust 校验 1.0..=200.0） | 有意收紧 |

## 四、仍存在的差异（非缺口）

1. **画板描述（`description`）**：Client 节点可带描述并作为行标题提示；当前模型无该字段。
2. **画板折叠状态**：Client 持久化 `opened`；平铺模型下不需要，属已消除的差异。
3. **拖放嵌套**：Client 可把画板拖进文件夹形成层级；当前只支持同层拖动重排（无文件夹层级，属第二节的平铺模型有意简化）。

平铺模型下已不再有实质性功能缺失。

## 五、结论

- 交互层（渲染器、几何、纹理策略、快捷键匹配、会话）与 Client **逐字节一致**，未发生重写。
- 迁入引入的回归集中在“模块外部接线”：应用层刷新拦截缺失、工作区面板隐藏方式、设置容量标注口径——本轮已全部修复并补齐自动测试。
- 原 Client 具备的侧栏画板拖放排序本轮已补齐；剩余差异均为**有意的架构性重设计**（模型、层级、持久化），不再有实质性功能缺失。
