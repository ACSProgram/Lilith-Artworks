# 素材板笔迹、形状与图层计划（0.3 alpha5）

更新时间：2026-10-20

**版本归属**：`0.3.0-alpha.4` 已于 2026-10-20 切分（未打标签），本计划是它之后的第一个批次，按版本
纪律记作 **alpha5**。若维护者决定并入 alpha4 线，只改版本归属与 `verify-metadata.mjs` 的映射，
阶段划分与验收口径不变。

本计划同时承载一项搭车改动：`todo.md` 中「备份检查元数据持久化」的三列（P3）。它与素材板无关，
但共用同一次 v5 → v6 迁移，理由与边界见「阶段 1」。

## 目标

在素材板上增加**标注能力**：用笔、直线、箭头、矩形、椭圆在参考图上画标注，标注作为可选中、
可移动、可缩放的独立条目持久化，并整体占一个固定在最上方的标注层。

素材板的定位是「导入并存储作品的参考图像，并在一个画布中统一展示」（`README.md`），本计划是
**标注参考图**，不是把素材板做成绘图软件。这条线决定「不做」清单。

## 非目标（本计划明确不做）

- 导出笔迹、把笔迹复制到系统剪贴板（位图仍可导出与复制，文字与笔迹不参与）
- 半透明笔迹（马克笔/高亮）——见「风险 1」
- 逐像素橡皮（只做整笔删除）
- 便签、评论线程、贴纸、形状库
- 滤镜、蒙版、混合模式
- 笔迹参与分支历史（与画板一样，不进入分支历史）
- 撤销「刚画完的一笔」——现有撤销栈只覆盖变换与删除，创建类操作不进历史（见「风险 3」）

## 数据模型

### 条目类型

`kind` 保持三个值，**笔刷与形状类型放在 BLOB 头部**，不新增 kind：

| kind | 含义 | 载荷 |
| --- | --- | --- |
| 0 | 位图 | `pin_board_bitmaps` + 仓库内 BC7 DDS |
| 1 | 文字 | `pin_board_texts`（只存数据库） |
| 2 | 笔迹 | `pin_board_strokes` + geometry BLOB（只存数据库） |

这样以后增加笔刷或形状不再重建 `pin_board_items`。

### 标注层

笔迹的 `layer` 固定为 `3`（标注层），永远在所有图片与文字之上，**不可调整**。图片与文字继续共用
`layer` 0/1/2（底层/中层/顶层），新建文字仍落中层。

`save_pin_board` 对笔迹条目**忽略前端传入的 `layer`，强制写 3**；这是防御性契约，配单测。

### geometry BLOB 布局

小端、紧凑、带版本字节，读侧逐项校验长度：

```text
[0]      version = 1
[1]      tool     0=pen 1=line 2=arrow 3=rect 4=ellipse
[2]      flags    bit0-1 形状方向（起点落在哪个角）, bit2 是否带压感
[3]      reserved = 0
[4..7]   pointCount  u32（形状恒为 0）
[8..]    pen：pointCount × (u16 x, u16 y, u8 pressure)，x/y 为条目包围盒内的归一化坐标
         形状：无额外字节，两端点由包围盒 + 方向位推出
```

- 坐标量化为 `u16`：4000 px 宽的包围盒仍有 0.06 px 分辨率；
- 压感量化为 `u8`；
- 单笔点数上限 **4096**（`MAX_STROKE_POINTS`），超限在写入侧按距离抽稀而不是拒绝；
- 读侧校验：长度必须等于 `8 + pointCount × 5`、`pointCount ≤ 上限`、`version` 已知、`tool` 已知、
  坐标与线宽有限，任一不满足即返回明确错误。

一笔 300 点约 1.5 KB；500 笔约 0.75 MB，5000 笔约 7.5 MB。

### 表

```text
pin_board_strokes
  item_id       INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE
  tool          INTEGER NOT NULL CHECK (tool BETWEEN 0 AND 4)
  color         TEXT    NOT NULL CHECK (length(color) = 7)
  stroke_width  REAL    NOT NULL CHECK (stroke_width > 0 AND stroke_width <= 1000)
  width         REAL    NOT NULL CHECK (width > 0)    -- 包围盒逻辑尺寸
  height        REAL    NOT NULL CHECK (height > 0)
  geometry      BLOB    NOT NULL CHECK (length(geometry) BETWEEN 8 AND 20488)
```

`width` / `height` 与 `pin_board_bitmaps` / `pin_board_texts` 的对应列同名同义，供 `current_states`
统一计算包围盒，不把尺寸塞进 BLOB。

## 阶段划分

| 阶段 | 内容 | 交付边界 |
| --- | --- | --- |
| 1 | schema v6、存储层与命令面 | 能存能读，界面还看不到笔迹 |
| 2 | SDF 渲染管线 | 用合成数据能画出四种形状与自由笔 |
| 3 | 绘制交互与工具 UI | 能真正画出来并落库 |
| 4 | 编辑能力与图层可见性 | 笔迹与图片交互一致，四层可单独开关 |
| 5 | 压感、手感与性能验收 | 采样/光滑/压感 + 压力测试与指标 |

每个阶段结束时单独提交，提交信息按 `AGENTS.md` 的英文祈使句格式，文档与代码同一提交。

---

## 阶段 1：schema v6、存储层与命令面

**迁移 v5 → v6 一次完成两组改动**，`SCHEMA_VERSION` 只跳一次：

- **A 组（备份调度，搭车）**：`branches` 追加三列——`last_check_elapsed_ms INTEGER`、
  `last_check_mode TEXT CHECK (last_check_mode IN ('quick','full','fallback'))`、
  `quick_hit_count INTEGER NOT NULL DEFAULT 0`。三列都可空或带默认值，旧行为迁移后不需要回填。
  依据 `todo.md` 的 P3 条目与 `archive/backup-scheduling-fix-plan-2026-10-20.md` 的搁置说明。
- **B 组（素材板）**：重建 `pin_board_items`（`kind IN (0,1,2)`）与 `pin_board_history`
  （`layer BETWEEN 0 AND 3`），新建 `pin_board_strokes`。两张表必须一起重建：`pin_board_history`
  的外键指向 `pin_board_items`，SQLite 的表重命名会改写外键，只重建一张会留下指向旧表的引用。
  做法沿用 `schema.rs:624` 的 `migrate_v4_to_v5`：`PRAGMA foreign_keys = OFF` → 事务内
  rename/create/copy/drop → `foreign_key_check` → 提交 → 恢复 `foreign_keys = ON`。

**存储层**（`src-tauri/src/pin_board/repository.rs`）：

- `ItemState` 增加 stroke 载荷；`current_states` 的 SQL 追加 `LEFT JOIN pin_board_strokes`；
  `load_view` 的 payload 分支增加 `PinBoardItemPayload::Stroke`；
- `create_stroke`：照 `create_text`（`repository.rs:1545`）的事务形状——`begin_item_mutation` →
  插入 items 行与 strokes 行 → 写 step 0 的 history → 更新 revision。校验颜色、线宽、点数、
  包围盒有限性、几何与声明尺寸一致；
- `read_strokes`：批量返回整板笔迹几何，供二进制命令使用；
- 保存路径**不需要改**：`transform_json` 只存 `points/uv`，与 kind 无关；删除、结算、回收站、
  scrub、孤儿扫描、空间统计都按「非位图不碰 DDS」处理，文字条目的既有分支直接覆盖笔迹；
- 导出、剪贴板复制维持只认位图，笔迹与文字同等待遇。

**命令面**（`src-tauri/src/pin_board/mod.rs` + `app/command_registry.rs`）：

- `create_pin_board_stroke(board_id, tool, color, stroke_width, width, height, geometry, center_x,
  center_y, expected_revision) -> PinBoardMutationResult`；
- `read_pin_board_strokes(board_id) -> Response`：二进制，布局为
  `u32 条目数` + 每条目 `(u32 itemId, u8 tool, u32 color, f32 strokeWidth, f32 width, f32 height,
  u32 geometryLen, bytes)`。前端一次解析成 `Map<itemId, StrokeGeometry>`，**零 JSON 解析**。
  二进制返回沿用 `read_pin_board_texture`（`mod.rs:422`）的 `ipc::Response` 先例。

**待验证**：创建命令的 `geometry` 参数能否直接接收前端 `Uint8Array`。可行则走二进制参数；
不可行则退到 `Vec<u8>`（JSON 数字数组，单笔 4096 点约 100 KB），并把 `MAX_STROKE_POINTS`
降到 2048。这个结论要在阶段 1 内定下并写进模块文档。

**验收**：

- v5 仓库（含位图、文字、多 step 历史）升级到 v6 后数据完整、`foreign_key_check` 通过；
- 迁移中途失败整笔回滚，仓库仍以 v5 打开；v5 程序打开 v6 仓库被拒绝并给出不可降级提示；
- 新建仓库直接是 v6；
- BLOB 边界：空点、单点、超上限、长度与点数不符、未知 version/tool、坐标非有限 → 全部明确拒绝；
- 笔迹保存时传入非 3 的 `layer` 被强制改写为 3（单测）；
- A 组三列在旧行上的默认值正确，且不影响现有调度测试。

**验证**：`cargo test pin_board --lib`、`cargo test --lib`（迁移与调度）、`cargo check --all-targets`。

---

## 阶段 2：SDF 渲染管线

现有管线是「每个条目一个纹理 quad」（`renderer.ts:2188` 的 `draw`）。笔迹不建纹理，用**距离场
线段**直接画，避免「按包围盒开 RGBA 纹理」对细长笔迹的浪费（一条对角线笔迹的包围盒可能
400×400，墨迹只占 1–2%，却要 640 KB 纹理）。

**着色器**（WGSL，与现有 `SHADER` 并列）：

- 段：锥形胶囊距离场，宽度在两端线性插值，覆盖自由笔与直线；
- 箭头：段 + 端点三角；
- 矩形：`abs(sdBox(p, halfSize)) - halfWidth`；
- 椭圆：椭圆距离场近似，`abs(sdEllipse(p, radii)) - halfWidth`；
- 抗锯齿：按 world → 像素比例做 1 px 羽化，需要把「每世界单位对应多少像素」送进 uniform；
- 顶点：归一化坐标经条目 quad 的四个角双线性映射到世界空间，因此移动/旋转/缩放**不需要改
  任何变换代码**。

**渲染组织**：

- 两趟绘制：先按 `layer`/`sort_order` 画图片与文字，再把所有可见笔迹**合并进一个大缓冲一次画完**。
  笔迹固定在最上层，因此不需要与图片交错，也不需要在图片处切分批次；
- 段/形状按可见性裁剪，亚像素长度（< 1 px）的段在低缩放下跳过（LOD）；
- 设备丢失重建时同步重建笔迹缓冲，与现有 `recoverGpu` 路径合并。

**必须排除的既有路径**（漏掉会出「红色失败占位框」）：

- `textureLoadCandidates`（`renderer.ts:2367`）目前遍历全部可见条目且不按 kind 过滤，文字靠
  `loadTexture` 内部分支才没出事；笔迹若进入队列会去读 DDS 并失败；
- `updatePlaceholderOverlay`（`renderer.ts:2240`）、`requestedDimension`、
  `estimatedResidentBytes`、`evictTextures` 同样按「无纹理」跳过。

**验收**：用合成笔迹数据（不经交互）能正确渲染五种工具；缩放 10%–800% 线条边缘无锯齿、无
断裂；选中时包围盒描边与图片一致；无 GPU 可用时仍显示常驻不可用说明。

**验证**：`npm run test:pin-board`（缓冲构建、LOD 判定、归一化映射的纯函数测试）。

---

## 阶段 3：绘制交互与工具 UI

- **工具状态机**：`select | pen | line | arrow | rect | ellipse`。与现有 `dragMode` 并存——
  非 `select` 工具下 `pointerDown` 进入绘制分支，不触发选中/框选；
- **实时预览层**：绘制过程中用 DOM 2D canvas 叠加层重绘当前笔迹（与 marquee、placeholder 同一
  层次思路），**不在每次 `pointermove` 重建 GPU 缓冲**；`pointerUp` 时才编码 BLOB、调用
  `create_pin_board_stroke`、经 `applyMutationResult` 落地；
- **Shift 约束**：笔工具下约束为直线；矩形约束为正方形，椭圆约束为圆；
- **工具栏**：工具组（6 个图标）+ 颜色按钮（复用 `PinBoardColorPicker`）+ 线宽输入；
- **快捷键**：`V` 选择、`P` 笔、`L` 直线、`A` 箭头、`R` 矩形、`O` 椭圆；绘制中 `Esc` 取消；
- **锁定态**：画板锁定时绘制工具禁用，与现有编辑控件一致；
- 工具切换时复位 `dragBefore` / `marqueeBaseIds` 等交互残留状态。

**验收**：五种工具都能画出并落库；关掉画板重开仍在；切换工具不残留选中/框选状态；绘制中
`Esc` 取消不产生条目；锁定时无法绘制。

**验证**：`npm run test:pin-board`（工具状态机、Shift 约束、取消路径）。

---

## 阶段 4：编辑能力与图层可见性

**编辑**：笔迹作为条目已有 quad，选中、移动、旋转、缩放、删除、撤销全部复用现有实现。需要
单独处理三处：

- `bringToFront`（`renderer.ts:2009`）：对笔迹只在**笔迹内部**提前，不改变它与图片的相对关系；
- `setSelectedLayer`（`renderer.ts:1291`）：选中笔迹时禁用，UI 置灰并说明「笔迹固定在标注层」；
- `arrangeableCount` 已只数位图，笔迹天然不参与阵列排序。

**图层可见性**：纯视图状态，不进 schema、不写库、不标记 dirty，放 `session.ts` 的 per-board
视图恢复里。

- 四个开关：底层 / 中层 / 顶层 / 标注层。文字跟随其所在层一起显示或隐藏；
- 必须接入五处判断，少一处会出现「看不见但能选中」：`refreshViewport` 的 `visibleImages`、
  `marqueeSelection` 的输入（`renderer.ts:1761` 传的是完整集合）、`selectAll`
  （`renderer.ts:1155`）、`contentBounds`（`renderer.ts:1988`）、`selectedImages`；
- 隐藏某层时若该层有条目正被选中，取消选中；
- 选笔工具时自动显示标注层；隐藏标注层时「重置视图」不把它算进包围盒；
- UI：工具栏一个图层图标，点开是四个复选框，不占用常驻按钮位。

**验收**：笔迹的移动/旋转/缩放/删除/撤销与图片行为一致；四个开关分别隐藏对应内容，且隐藏后
无法通过点击、框选或 `Ctrl+A` 选中；隐藏标注层后重置视图不会缩放到空白区域；重进画板保持
开关状态。

**验证**：`npm run test:pin-board`（图层过滤、隐藏层取消选中、`contentBounds` 排除）。

---

## 阶段 5：压感、手感与性能验收

**采样与光滑**（纯函数，放 `geometry.ts` 同级文件，写确定性测试）：

- 采样用 `getCoalescedEvents()`，否则 Chromium 会把 `pointermove` 合并到 ~60 Hz，快速划线丢点；
- RDP 抽稀，把 60 Hz 冗余点砍掉一半以上；
- 向心 Catmull-Rom 转三次贝塞尔后按弧长重采样，过原始点、不过冲、不磨掉尖角；
- 粗细 `width = base × (0.15 + 0.85 × pressure)`，宽度序列本身也要光滑，两端做 taper；
- **无压感回退**：`pressure === 0` 或 `pointerType !== "pen"` 时用固定宽度。

**性能优化**：

- 亚像素段 LOD、视口裁剪、笔迹合并缓冲；
- 只在可见集合变化时重建缓冲，平移用超扫边距避免逐帧重建。

**压力测试**（按 `docs/guides/validation.md` 既有分层）：

- 进 CI：扩展 `src-tauri/tests/stress_pin_board.rs`——BLOB 边界、批量读写正确性、点数上限；
- 不进 CI：扩展 `stress_bench.rs`——500 / 2000 / 5000 笔的读写吞吐与编解码吞吐，数值写进
  `tests/perf-baseline.json`；
- 前端：`npm run test:pin-board` 测 RDP/平滑/段构建吞吐（纯函数，无 GPU）；
- GPU 帧率只能在真实适配器上测，走 GUI 手工验收；另加一个开发期自检入口（生成 N 笔合成数据
  并打点），方便维护者复测。

**验收指标**：

| 场景 | 笔数 | 打开画板 | 平移/缩放 | 绘制 |
| --- | --- | --- | --- | --- |
| 日常 | 500 | < 300 ms | 60 fps | 无掉帧 |
| 极端 | 2000 | < 600 ms | ≥ 45 fps | 轻微延迟可接受 |
| 极限 | 5000 | < 1.2 s | ≥ 20 fps，不卡死 | 可感知延迟但可控 |

Rust 侧：5000 笔批量读取 < 200 ms、批量写入 < 500 ms。

60 fps @ 500 笔是**目标而非承诺**；若实测达不到，如实报告并说明瓶颈，不通过调低目标收场。

---

## 风险

1. **半透明笔迹的接缝**：现有混合是 `src-alpha / one-minus-src-alpha`（`renderer.ts:390`），
   而 SDF 段逐段独立绘制，相邻段在连接处必然重叠。不透明笔没问题，半透明笔会在每处接缝
   混合两次，出现一串深色斑点。解法（每笔离屏合成 / stencil 每笔只写一次）都有明显代价，
   因此本计划**只做不透明笔迹**，用颜色深浅模拟轻重。
2. **压感依赖驱动模式**：Wacom 在「鼠标模式」下 Windows 只给鼠标事件，`pressure` 恒为 0.5。
   这是用户环境问题，代码只能回退到固定线宽。验收时要区分「功能坏了」与「设备没给数据」。
3. **刚画完的一笔不能撤销**：现有撤销栈只快照 `{deleted, layer, points, uv}`
   （`renderer.ts:2040` 的 `snapshot`），创建类操作走 DB 命令后由 `applyMutationResult` 重置基线。这与
   「添加文字」的现状一致，本计划不改变。删除已有笔迹可以撤销（`deleted` 在快照内）。
4. **形状的退化包围盒**：水平或垂直的直线高度为 0，归一化会除零。写入侧必须把包围盒按线宽
   膨胀后再存，读侧拒绝零尺寸。
5. **0.3 出现第二次迁移**：0.3 此前只有 v4 → v5 一次迁移（`todo.md` 与归档计划均按「唯一一次」
   表述）。本计划落地后该表述需改为「alpha3 的 v4 → v5 与 alpha5 的 v5 → v6 两次」，并同步
   `verify-metadata.mjs` 的版本→schema 映射。

## 文档同步

- 本计划阶段 1 落地时更新 `docs/modules/pin-board.md` 的「存储布局」「命令面」两节，写**已实现**
  的口径，不写计划；
- 阶段 4 落地时补「图层可见性」一节；
- `CHANGELOG.md` 的改动写入顶部 `## Unreleased`，不写回带日期的版本小节；
- 全部阶段完成后本文件移入 `docs/planning/archive/`，并同步其 `README.md` 索引。
