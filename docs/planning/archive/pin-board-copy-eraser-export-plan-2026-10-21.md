# 素材板：画板内复制、橡皮擦与整画布导出计划

更新时间：2026-10-21

**状态：阶段 1、阶段 2 都已实现并通过人工验收（2026-10-21，已归档）。** 阶段 1 的结论、偏差与
实施记录见 `handoff-log-2026-10-21.md` 第 11 节；阶段 2 的见同文件第 12 节（含与计划文本的
差异、两条交互修订的口径与验收结论）。两者的有效契约已并入 `docs/modules/pin-board.md`，
行为变化见 `CHANGELOG.md` 的 `## Unreleased`。

**版本归属**：`0.3.0-alpha.5` 已切分（未打标签）。本计划是它之后的批次，落地时按版本纪律只写
`CHANGELOG.md` 的 `## Unreleased`，不改五处版本字段、不新建版本小节；是否切版本由维护者单独决定。

## 背景

素材板现有三类条目（位图 / 文字 / 笔迹）与四层可见性，但**条目级操作只在位图上完整**：

- 画板内复制粘贴只承载位图（`renderer.ts` 的 `copySelection()` 过滤 `kind === "bitmap"`，
  Rust 侧只有 `paste_pin_board_images`），选中文字或笔迹时给一条提示文案；
- 系统剪贴板复制只认位图（`copyImageToClipboardId()`）；
- 导出是半成品：位图走原生解码，文字由前端栅格化后写盘，笔迹完全没有入口
  （`exportableCount` 把笔迹排除，只选笔迹时按钮是灰的，但选中文字时又是亮的）；
- 绘制工具只有「选择 + 笔 / 直线 / 箭头 / 矩形 / 椭圆」，没有擦除；
- 图层面板只有四个开关，没有条目数量。

同时缺一个交付级出口：把整块画板按可见内容导成一张透明 PNG。

## 目标

1. 文字与笔迹在**画板内**可复制粘贴，行为与位图一致。
2. 把「复制到系统剪贴板 / 单条导出」收敛为**只支持位图**，其余入口置灰、静默，不再弹提示。
3. 增加**整条擦除**的橡皮擦，环形成空心圆光标，直径与笔共用同一个「大小」。
4. 图层面板显示每层条目数（标注层不显示）。
5. 新增**整画布导出**：所有可见内容合成一张透明 PNG。

## 决策记录（2026-10-21 与维护者确认）

| 议题 | 结论 |
| --- | --- |
| 橡皮擦语义 | 只做**整条擦除**；局部（像素级）擦除不在本计划 |
| 文字/笔迹的外部复制与单条导出 | **不支持**：入口置灰、快捷键无反应、**不再弹提示** |
| 文字现有的单条导出 | **删除**，由整画布导出覆盖 |
| 画板内复制 | 文字与笔迹都要支持 |
| 整画布导出范围 | 所有可见条目的内容外框，可留边距；倍率 1x / 2x / 4x |
| 整画布导出上限 | **只按最长边**：低 10000 / 中 20000 / 高 30000 像素，取素材板设置里的纹理缓存等级；超限自动降倍率，贴近上限时确认提示 |
| 导出期间 | 锁定画板（禁止编辑与切换画板）；**不取仓库写锁**，不影响检查与备份 |
| 大图处理 | 分块渲染 + Rust 流式拼装 PNG（不在浏览器里拼） |
| 粘贴落位（阶段 1 实施前确认） | **保持整组相对位置**，外框中心对齐放置点；位图不再走导入的阵列口径 |
| 橡皮的擦除触发（阶段 1 实施前确认） | **按下即判定一次**（单击笔迹就能擦掉），继续拖拽则增量擦；否则单击无反应会像失灵 |
| 笔迹的碰撞箱（阶段 1 之后确认） | 点选与框选都**按绘图区域（墨迹）判**，不再按包围框；另加 3 CSS 像素最小容差，细线才点得中。位图与文字保持四角框 |

## 非目标

- 局部擦除（把笔迹切段）、擦除图片或文字
- 笔迹与文字的逐条导出、复制到系统剪贴板
- 合成图的其它格式（JPG / WebP）与自定义背景色（只出透明 PNG）
- 导出时保留选中框、占位框、框选矩形、绘制预览
- 导出其它画板或跨画板批量导出
- 笔迹进入分支历史（与现状一致）

## 阶段划分

| 阶段 | 内容 | 交付边界 | 体量 |
| --- | --- | --- | --- |
| 1 | 画板内交互与口径收敛 | 图层面板有数字、能整条擦、文字与笔迹能复制、无效入口全灰且静默 | 中 |
| 2 | 整画布导出 | 能导出一张透明 PNG，带倍率上限、进度与取消 | 大 |

两个阶段相互独立：阶段 2 不依赖阶段 1 的代码，只共用文档。可以分开提交、分开验收。

---

## 阶段 1：画板内交互与口径收敛

### 1.1 图层面板显示条目数

- `PinBoardInteractionState`（`renderer.ts`）增加 `layerCounts: number[]`，长度 4，下标即 `layer`；
  统计**未删除**条目（位图 + 文字 + 笔迹）。
- 口径：**隐藏层里的条目仍计入**——数字不该随显隐开关跳动；标注层按维护者要求不显示。
- `PinBoardLayersPanel` 增加 `counts?: number[]` 入参，行右侧渲染一个 `<span class="pin-board-layer-count" aria-hidden="true">`。
  数字必须 `aria-hidden`：否则四个开关的可访问名会变成「顶层 12」；同时给行按钮补
  `aria-label={row.label}`，可访问名不再依赖 `textContent` 拼接（现有测试也因此改成按 role + name 取按钮）。
- 样式加在 `src/styles/pin-board.css` 的图层段落：数字用弱化色、行内右对齐，不改变行高。
- 不需要新的订阅：`emitState()` 在每次模型变化后都会调用。

### 1.2 「线宽」改名为「大小」

- `PinBoardModule.tsx` 的 `StrokeStyleControls`：标签、滑条 `aria-label`、`aria-valuetext`、
  数值框 `aria-label` 一并改为「大小」。
- 数值语义不变：世界单位，范围 1–500，滑条仍走 `strokeWidthFromSlider` 的指数映射；
  画笔线宽与橡皮直径共用这一个值，切换工具不记忆独立数值。
- 内部标识符（`strokeWidth`、`MIN/MAX_STROKE_WIDTH`、`strokeStyle.width`）**不改名**：
  它们描述的是笔迹线宽这一物理量，改名的收益为零、噪音很大。只有面向用户的文案改。

### 1.3 橡皮擦（整条擦除）

**工具模型**：`strokeDrawing.ts` 里 `PinBoardTool` 扩成 `"select" | StrokeToolName | "eraser"`。

- `isStrokeTool(tool)`（现在是 `tool !== "select"`）**必须收窄**成「会产生几何的工具」，
  另加 `isDrawingTool(tool)`（绘制或擦除）。现有 `isStrokeTool` 的每个调用点都要过一遍：
  绘制分支、`setTool` 清选中、标注层自动打开、工具条禁用、`PinBoardModule` 的提示判断。
  不拆就会把橡皮当成笔去编码几何。
- 快捷键 `E`（`toolForShortcut`），与现有 `V/P/L/A/R/O/T` 不冲突。
- 工具条新增一个按钮（`Eraser` 图标），插在画笔之后。

**命中判定**（新增纯函数，建议放 `strokeHit.ts`，与 `strokeRender.ts` 共用几何解析）：

- 输入：`PinBoardStrokeGeometry`、条目的 `points` / `uv`、擦头圆心与半径（世界单位）、当前缩放；
- 自由笔：归一化点经 `mapLocalPoint(points, uv, …)` 映射到世界，逐段算「点到线段距离」，
  命中条件为 `距离 ≤ 半线宽 × 压感系数 + 擦头半径`；
- 形状：用 `shapeAnchors` 得到两端点（矩形/椭圆按四边），按线段距离或边框距离判定；
- 性能：先按条目包围盒外扩擦头半径做预筛，只有相交的条目才解析几何；每条笔迹的世界折线
  做一层缓存（键 = 条目 id + 几何版本 + 变换签名），变换变化即失效。

**交互**：`dragMode` 新增 `"erase"`。

- `pointerdown`：进入擦除，记录**全部**可见笔迹的 `snapshot()` 作为 before；
- `pointermove`：把新命中的条目置 `deleted = true` 并调 `afterLiveChange()`（只在新命中时调用，
  不是每个指针事件都重建整板缓冲）；同一笔不重复处理（用一个 `Set` 记住本轮已擦的 id）；
- `pointerup`：`pushHistory(before, after)` + `afterModelChange(ids)`，**一次拖拽 = 一条撤销记录**；
- `pointercancel` 与 `pointerup` 走同一条提交路径（已经擦掉的内容不还原）；
  与绘制分支「取消」不同，理由是擦除已经改写了模型，复原反而更意外；
- 锁定、切画板、卸载等边界沿用现有 `finishActiveInteraction()`，即按已擦除状态提交并结算。

**环形光标**：CSS cursor 做不了任意尺寸，用一个 DOM 空心圆。

- 元素 `.pin-board-eraser-ring` 放在 `.pin-board-stage` 内，`pointer-events: none`，
  层级低于图层面板与标注工具条；
- 尺寸：`直径(CSS px) = 大小 / worldUnitsPerCssPixel()`，视觉最小值 4 px（再小看不见）；
- 更新时机：`pointermove`、缩放变化、大小变化、窗口 resize；离开画布、切换工具、锁定、导出时隐藏；
- 只在 `tool === "eraser"` 时显示，画笔的 `.tool-pen` 逻辑不动；
  另加 `.tool-eraser` 类把默认十字隐藏（否则圆环内外都是十字）。

**连带行为**：

- 切到橡皮时沿用 `setTool` 的逻辑：清空选中、隐藏的标注层自动打开、锁定后退回 `select`；
- 擦除产生脏标记与自动保存（`afterModelChange` 已有），不需要额外接线。

### 1.4 灰化与静默，删除文字单条导出

**删除文字单条导出**（共 6 处，删干净不留死代码）：

| 位置 | 处理 |
| --- | --- |
| `usePinBoardController.ts` 的 `exportImages` 文字循环 | 删除整段 |
| `renderer.ts` 的 `selectedTextItems()` | 删除 |
| `textLayout.ts` 的 `rasterizePinBoardTextPng` / `exportTextMaxDimension` | 删除（`rasterizePinBoardText` 仍用于纹理，保留） |
| `api.ts` 的 `writePinBoardItemPng` | 删除 |
| `src-tauri/src/pin_board/mod.rs` 的 `write_pin_board_item_png` | 删除 |
| `app/command_registry.rs` 的注册项 | 删除 |

Rust 侧 `repository::write_text_png` 一并删除；`publish_export_png` 仍被位图导出使用，保留。

**灰化与静默**：

- `copyImageToClipboardId()` 语义不变（只认单个位图），删除 `usePinBoardController.ts`
  里「文字与笔迹条目暂不支持复制到系统剪贴板」的提示分支——现在点了会弹，改成**直接返回**；
- `exportableCount` 语义改为「选中里可导出的**位图**数量」（原为「非笔迹」）；
  按钮按 `=== 0` 禁用。右键菜单原本没有禁用条件（计划初稿误以为有），实施时给
  `PinBoardContextMenuRequest` 补 `exportableCount`，两项据此置灰——详见「实施记录」；
- 「复制画板图片」的提示分支删除（1.5 落地后它恒可用，不会再走到该分支）；
- Ctrl+C / Ctrl+V：没有可复制/可粘贴内容时静默，不写状态栏。

### 1.5 画板内复制（文字 + 笔迹）

**前端载荷**：`PinBoardClipboardImage` 改为 `PinBoardClipboardItem { kind, sourceBoardId, sourceItemId }`
（位图的显示尺寸不再由前端携带，Rust 读源条目的 `points` 现算）。`clipboardRef` 与 `clipboardCount`
随之调整，按钮可用性口径不变。

**Rust 命令**：新增 `paste_pin_board_items`，并**取代** `paste_pin_board_images`。

- 入参：`board_id`、`items: Vec<PinBoardClipboardItem>`、`center_x`、`center_y`、`expected_revision`；
- 校验：非空、数量上限（复用 `dds::MAX_IMPORT_FILES`）、revision 一致、目标画板有效、
  每个源条目存在且未删除（位图还要源 DDS 可读，沿用 `source_dds_path` 的检查）；
- 事务：`begin_item_mutation(n)` 一次分配 n 个 item id → 按 kind 分派（位图复制 DDS，
  文字复制 `pin_board_texts` 行，笔迹复制几何 BLOB 与 `stroke_width`）→ 统一写 step 0 状态
  → `bump_revision` → 提交；
- **一次事务、一次 revision**。混合选择必须原子：拆成两条命令时，第二条失败会留下半份副本。
- 位图分支的 DDS 复制逻辑从现有 `paste_images` 抽出成内部函数复用，两条路径不分叉。

**取代而不是并存的理由**：两条粘贴路径迟早分叉（校验、落位、错误文案各一套）。
旧命令的唯一调用点就是控制器里的 `pasteImages`，替换面很小。

**落位规则（已确认：保持相对位置）**：整组**保持相对位置**，以外框中心对齐放置点。

- 推荐理由：复制粘贴的首要预期是「再来一份、相互位置不变」；混合选择（图片 + 文字标注）
  按现有「围绕放置点重排」会打散用户摆好的对齐关系（阵列排序此前修过同类问题）。
- 影响：位图粘贴从「重新排布」变成「保持相对位置」，属于**行为变更**，需在 `CHANGELOG.md`
  的 `## Unreleased` 记 Changed，并列入人工验收。若维护者要求保留位图旧行为，只需改这一个
  落位函数，其余结构不变；但混合粘贴会重新出现错位。

**既有限制照旧**：粘贴产生的条目不在撤销栈里（撤销历史只覆盖变换与删除），与现有位图粘贴一致。

### 阶段 1 验收

自动：

- `npm run test:pin-board`：新增/更新
  - 图层计数（含隐藏层计数、标注层不计、`emitState` 后数字正确）
  - 橡皮命中（直线、弧线、四种形状、边缘、压感半径、包围盒预筛）
  - 橡皮状态机（命中即删、无命中不产生任何改动、一条拖拽只产生一条历史）
  - 工具快捷键与 `isStrokeTool` / `isDrawingTool` 的边界
  - 环形成尺寸换算（缩放与大小的组合）
  - 剪贴板载荷与粘贴命令的参数形状
- `cargo test pin_board --lib`：新增
  - `paste_pin_board_items` 的三类条目复制（几何/文本/位图）、混合选择、撤销无关的 step 记录
  - revision 冲突、源条目不存在、源画板在回收站、数量超限
- `npm run build`（tsc）与 `cargo check --tests`

人工（GUI，由维护者执行；**2026-10-21 全部通过**，结论见 `archive/handoff-log-2026-10-21.md`
第 11 节）：

- 四层数字正确，隐藏一层后数字不变
- 橡皮的环形光标跟随、大小随滑条变化、擦除后 Ctrl+Z 一次恢复整轮
- 选中文字/笔迹时「复制到剪贴板」「导出图片」为灰、点击与 Ctrl 快捷键均无反应、无提示
- 复制「图片 + 文字 + 笔迹」混合选择，粘贴后相对位置与原件一致

### 阶段 1 实施记录（2026-10-21）

与计划文本的差异，均已在代码与模块文档里定稿：

- **图层面板的可访问名**：只给数字加 `aria-hidden` 不够——`PinBoardLayersPanel.test.tsx` 用
  `button.textContent` 断言行文本，加数字后仍会变成「标注层5」。行按钮因此补了
  `aria-label={row.label}`，可访问名不再依赖文本拼接（测试也改成按 role + name 取按钮）。
- **右键菜单原本没有禁用条件**：计划写「按钮与右键菜单的禁用条件不变」，实际
  `PinBoardContextMenu` 的「导出图片」「复制图片到剪贴板」只有 `onClick`。给
  `PinBoardContextMenuRequest` 补了 `exportableCount`：导出项用 `exportableCount === 0` 禁用，
  剪贴板项用 `selectedCount !== 1 || exportableCount === 0`，两项在选中文字/笔迹时一起变灰。
- **`begin_item_mutation` 按 kind 数组新增**（而不是只接一个数量）：混合粘贴里每条 kind 不同，
  只有位图需要预建 `pin_board_bitmaps` 占位行。粘贴、导入、文字、笔迹四条写入路径都改传数组，
  参数形状之外行为不变（既有 73 条 Rust 用例全部通过）。
- **导出 id 也收窄到位图**：`exportableCount` 只数位图后，控制器不能再把 `selectedImageIds()`
  直接交给 `export_pin_board_images`（混选会带进文字 id）。新增 `renderer.exportableImageIds()`，
  与按钮可用性同口径。
- **环形光标挂在画布外层**（`canvas.parentElement`）而不是 `.pin-board-stage`：与占位层、绘制预览层
  同级，层级低于图层面板与标注工具条；`pointer-events: none`，不影响命中。
- **擦除命中额外覆盖箭头头部**：只按杆判距离时，从侧面擦箭头翼尖擦不掉。`buildStrokeWorldOutlines`
  为箭头产出两条折线（杆 + 闭合三角），与渲染的图元划分一致。
- `strokeHit.test.ts` 的镜像用例按 u16 量化后的坐标断到百分位（`encodeStrokeGeometry` 把 u/v 存成
  u16，0.1 会有约 1e-3 的误差）；形状用例改用与局部宽高一致的方块四边形，避免把「世界 100×20」
  与「局部 100×100」混着断言。

自动验证（实际结果）：`npm run test:pin-board` 22 文件 / 217 用例通过（新增 `strokeHit.test.ts`、
renderer 的橡皮与计数用例、图层计数与剪贴板载荷用例）；`npx vitest run` 45 文件 / 394 用例通过；
`npx tsc --noEmit`、`npm run build` 通过；`cargo test pin_board --lib` 73 用例通过（含
`pastes_mixed_items_keeping_relative_positions`、`paste_rejects_stale_revision_and_trashed_sources`、
`paste_rejects_deleted_sources_and_oversized_requests`）；`cargo check --tests` 通过。

### 补充：点选与框选改用墨迹命中（2026-10-21，阶段 1 之后）

计划之外的一项补充，由维护者在阶段 1 验收时提出：**笔迹的实际碰撞箱改成绘图区域而不是包围框**。
落地点是 `strokeHit.ts`（与橡皮擦共用同一套世界折线），点选与框选都按墨迹判，位图与文字保持四角框。

- 点选：光标到折线的距离 ≤ 「墨迹半径 + 容差」；框选：折线与框选矩形求交（每段按两端半径的较大
  值 + 容差外扩）。`strokeHitByEraser` 的半径参数因此改名为 `radius`，选择与擦除都传自己的值。
- **最小容差 3 CSS 像素**（`STROKE_HIT_TOLERANCE_CSS_PIXELS`，按当前缩放换算）：1–3 px 的细线
  只按半个线宽判，光标偏一个像素就点不中，实测手感不能接受。
- 框选的成本在 `pointermove` 上，因此两次便宜判断挡在分段求交前：外框与框选矩形不相交 → 否；
  框选矩形完全罩住外框 → 是。实测线段×矩形求交约 0.02 µs/段，典型画板（约 1 万段）约 0.2 ms/次，
  压测口径（15 万段）约 3 ms/次；只有「大量超长笔迹同时局部擦边」才会掉到十几毫秒。
- `selection.marqueeSelection` 只加一个可选命中谓词，默认仍是四角框——删除/可用性过滤留在纯函数里，
  既有三处调用与测试不需要改。
- 已知语义变更：框住椭圆/矩形的空心内部不再选中（Figma/PS 按包围框，会略有出入）；位图不做 alpha
  级判定（要读回纹理），文字框本来就贴合字形。

自动验证（补充项实际结果）：`npm run test:pin-board` 22 文件 / 227 用例（新增 4 条 `strokeIntersectsBounds`
用例、5 条 renderer 的墨迹点选/框选用例、1 条 `marqueeSelection` 谓词用例）；`npx vitest run` 45 文件 /
404 用例通过；`npx tsc --noEmit`、`npm run build` 通过。Rust 侧无改动，仍为 73 用例。

### 补充：橡皮按扫过的轨迹判（2026-10-21，维护者实测后修正）

维护者实测反馈：碰撞箱生效后，橡皮「对着目标反复擦动可能没反应，点一下反而更容易擦掉」。

原因是只判当前采样点。浏览器把 `pointermove` 合并到约 60–120 Hz，快速擦动时相邻两帧可能隔几十个
像素，细笔迹正好落在两次采样之间就整条都擦不到；而单击是一次「精确落点」，所以反而更容易命中。

改为 `strokeHitBySweep`：每次 `pointermove` 把「上一帧位置 → 这一帧位置」当成一段，判墨迹折线到
这条线段的距离 ≤ 「墨迹半径 + 擦头半径」（单点仍是圆形判定）。渲染器持有 `eraseFrom` 记录上一帧
位置，预筛同时用「这一段位移的包围盒」。扫掠是线段×线段距离，热路径上只多 4 次点线距离。

验证：`npm run test:pin-board` 22 文件 / 230 用例（新增 1 条扫掠命中单测、2 条 renderer 扫掠用例——
「两个采样点都离笔迹很远但连线穿过它」与「整段都不碰墨迹」）；`npx vitest run` 45 文件 / 407 用例；
`npx tsc --noEmit`、`npm run build` 通过。

---

## 阶段 2：整画布导出

### 2.1 口径与上限

- **范围**：所有未删除、所在图层可见的条目，按内容外框裁剪，可选边距（默认 0，对话框给 0–64 px）。
- **倍率**：1x / 2x / 4x，1x 表示 1 世界单位 = 1 像素。
- **上限**（只按最长边，取素材板设置的「纹理缓存等级」）：

  | 等级 | 最长边上限 | 典型输出 | 原始像素 | PNG 编码耗时（量级） |
  | --- | --- | --- | --- | --- |
  | 低 | 10000 | 10000×5600 | 约 5600 万 | 6–12 s |
  | 中 | 20000 | 20000×11000 | 约 2.2 亿 | 25–50 s |
  | 高 | 30000 | 30000×17000 | 约 5.1 亿 | 60–120 s |

- **自动降倍率**：`实际倍率 = min(用户所选, 上限 / 内容最长边)`，按整像素取整后回算；
  对话框实时显示「预计输出 W×H、预计耗时」。
- **警告**：命中上限（即被降倍率）时弹确认：「输出已按上限 20000 像素降为 3.1x，
  预计约 40 秒，可能较慢，是否继续？」不额外设总像素硬上限——这是维护者明确选择的
  「只按边长」，因此 30000×30000 这类极端输入仍可发起，只做提示不做拦截。
- 上限常量与降倍率计算放一个纯函数模块（建议 `src/modules/pin-board/exportPolicy.ts`），便于单测。

### 2.2 离屏渲染通道

现在渲染器只画到 swap chain（`draw()` 里 `context.getCurrentTexture()`）。导出需要一条**并行**通道，
不能改动屏幕那份状态：

- 新增导出用 uniform buffer（`minX / minY / width / height / devicePixelsPerWorldUnit`），
  与屏幕的 `viewportBuffer` 分开，逐块写入；
- 新增导出用渲染目标纹理：`rgba8unorm`（`RENDER_ATTACHMENT | COPY_SRC`），尺寸 = 块尺寸；
  选 `rgba8unorm` 而不是画布格式 `bgra8unorm`，是为了回读后能直接按 RGBA 编 PNG，省一次通道交换；
  为此需要一条**目标格式为 `rgba8unorm` 的管线副本**（着色器与绑定布局完全相同，只是 `format` 不同）；
- 清屏值 `{0,0,0,0}`（透明）。上屏画布是 `alphaMode: "opaque"`，离屏目标透明即可，
  混合本来就是标准 src-over，alpha 会正确累积；
- 导出通道**不画**选中描边、纹理占位框、框选矩形、绘制预览；
- 笔迹缓冲按导出区域单独构建：`buildStrokeBatch` 的收集框传整个导出区域，
  结果写进导出的顶点/索引缓冲，**不碰**屏幕那份（`strokeIndexCount` 等字段保持原值）。

**纹理分辨率（本阶段最大的不确定项）**：位图纹理当前按「屏幕缩放」请求尺寸
（`requestedDimension`），2x 导出需要更高分辨率。方案：导出期间给渲染器一个
`exportScale`，逐块渲染前对该块涉及的位图按 `显示尺寸 × 导出倍率` 请求纹理并等待就绪
（复用现有加载与预算机制，只是换个目标尺寸）。

- 预算不够时按现有淘汰策略降级，必要时对个别图片退回屏幕分辨率；
- 若实测下来「按导出倍率重载」明显拖慢或反复淘汰，退路是**整次导出按屏幕纹理分辨率渲染**，
  并在对话框注明「部分图片按屏幕分辨率导出」——保真度下降但可预测。

### 2.3 分块、回读与原始字节通道

- 块尺寸：宽 = `min(8192, 剩余宽度)`（8192 是设备 `maxTextureDimension2D` 的通用上限），
  高 = 行带高度。
- **行带**：PNG 必须逐行顺序写，所以按「行带」推进——一次渲染一整条横带的所有块，
  Rust 侧拼成完整行后逐行写入。行带高度按内存目标自适应：
  `bandHeight = clamp(floor(32 MiB / (输出宽 × 4)), 128, 2048)`，
  即常驻内存约 32 MiB（8000 宽 → 1024；20000 宽 → 416；30000 宽 → 272）。
- 每块流程：设置导出 uniform → 渲染到导出纹理 → `copyTextureToBuffer`（`bytesPerRow` 按 256 对齐，
  回读后去掉行尾 padding）→ `mapAsync` → 把 RGBA 交给 Rust。
- **原始字节通道**：Tauri v2 支持命令接收原始请求体（`tauri::ipc::Request` +
  `InvokeBody::Raw`，前端 `invoke("cmd", uint8array, { headers })`）。项目里目前只有出参用了
  `Response`，入参是新范式：
  - `src/shared/tauri.ts` 增加 `invokeCommandWithBody<T>(command, body, headers)`，
    错误归一化与 `invokeCommand` 保持一致；
  - 块的坐标与序号走 header（避免大块 JSON），前提是 header 只承载短字符串。
  - 若某块压缩后体积可观（多数画板大量空白），也可以先在前端编成 PNG 再传——
    **本计划选择传原始 RGBA**：省一次前端编码，Rust 侧直接写行带；代价是单块 33 MB 要在 IPC 上搬一次。

### 2.4 Rust 流式拼装

- 新增依赖 `png`（已在依赖树中，由 `image 0.25` 传递引入，提为直接依赖不增加新 crate）。
- 一组命令，共用一个**单并发**的导出会话表（Tauri managed state，`Mutex<Option<Session>>`）：

  | 命令 | 作用 |
  | --- | --- |
  | `begin_pin_board_canvas_export(boardId, outputDirectory, width, height)` | 在输出目录建临时文件、写 PNG 头、返回会话句柄 |
  | `write_pin_board_canvas_export_tile(headers: 会话/块序号/坐标, body: RGBA)` | 校验顺序与尺寸，解码进行带；行带填满即写行 |
  | `finish_pin_board_canvas_export()` | 写 IEND、`sync_all`、原子改名为最终文件、返回路径 |
  | `cancel_pin_board_canvas_export()` | 丢弃会话并删除临时文件 |

- 临时文件放**输出目录内**（`tempfile::NamedTempFile::new_in`），保证最后一步改名是同卷原子操作，
  与现有 `publish_export_png` 同一口径。
- 校验：块必须按行带、行内从左到右到达；尺寸越界、顺序错乱、行带不完整一律报错并结束会话。
- 目标目录授权走 `path_scope::ensure_authorized` + `storage::ensure_outside_repository`，
  **不取仓库写租约、不取仓库操作锁**——导出期间后台的完整性扫描、自动备份、清理照常运行。
- 异常残留：进程被强杀会留下临时文件，`begin` 时顺带清理输出目录里同前缀的残留；
  不接入 `pending_file_cleanup`（那是仓库内文件的通道）。

### 2.5 对话框、进度与中止

- 导出对话框（新建组件，沿用 `PinBoardTextDialog` 的样式骨架）：导出位置（复用
  `pickPinBoardExportDirectory`）、倍率（1x/2x/4x）、边距、预计输出尺寸与耗时、超限警告。
- 进度：前端自己算（已发送行带 / 总行带），复用 `.pin-board-transfer-progress` 那条状态条，
  不逐块走 `AppEvent.operationProgress`（避免每块一次事件）。
- **取消**：状态条上加取消按钮；取消时停止循环并调 `cancel_pin_board_canvas_export()`，
  临时文件随会话丢弃。
- **锁定与中止**：导出期间控制器置 `exporting`，禁用绘制工具、图层面板、画板列表、
  粘贴/导入/导出入口；`currentBoardId` 或作品变化即视为「用户离开该画板」，自动取消并提示。
- 导出的是渲染器内存里的当前状态，**不强制落库**，因此整段流程不产生任何仓库写操作。

### 阶段 2 验收

自动：

- `exportPolicy` 纯函数：倍率与上限的组合（含取整、1x/2x/4x、刚好等于上限、远小于上限）
- 行带布局纯函数：块宽/块高/块数/最后一块的余量
- `cargo test pin_board --lib`：PNG 流式写盘（尺寸、字节数）、乱序块拒绝、越界块拒绝、
  取消后临时文件删除、`ensure_outside_repository` 拒绝写进仓库
- `cargo check --tests`

人工（GUI，由维护者执行）：

- 小画板 1x / 2x 导出：透明背景、内容位置与屏幕一致、笔迹边缘与屏幕观感一致
- 大画板触发降倍率：对话框显示预计尺寸与耗时，确认后能正常完成
- 导出中途取消：无残留文件、画板解锁
- 导出中切画板/切作品：自动取消，不留下半个文件
- 导出时后台跑完整性扫描或自动备份：双方都能完成，导出不报 `repository_busy`
- 无可用 WebGPU 适配器的环境：导出入口应禁用（素材板本身不可用）

### 阶段 2 实施记录（2026-10-21）

与计划文本的差异，均已在代码与模块文档里定稿：

- **`begin` 多一个 `file_name` 参数**（计划签名只有 `boardId`/目录/宽高）：文件名取画板名，
  原生端清掉非法字符、重名追加 `-2`/`-3`，避免给用户一个 `pin-board-123.png` 这种看不出
  是什么的名字。也因此**没有**为取画板名去读数据库——导出整段不碰仓库。
- **行带高度的实际值**与计划表格的示例数字略有出入（计划按取整后的 32 MB 估算）：公式不变，
  8000 / 20000 / 30000 宽实际是 1048 / 419 / 279 行。
- **命中上限不弹二次确认**，而是把警告写进对话框、主按钮改成「仍要导出」：同一个动作弹两层
  确认只会让人机械地点下去。提示文案仍按计划给出「已按上限 N 像素降为 x.x、预计约 N 秒」。
- **纹理按导出倍率重载，但加了两条保护**：单张就要吃掉四分之一显存预算时不升级；当前块涉及的
  条目在淘汰时受保护（`exportProtectedIds`）。后者是实测推演出的必要条件——导出区域的条目往往
  不在屏幕视口内，而淘汰优先挑「不可见」的，不保护就会把刚准备好的纹理淘汰掉、那一块变空白。
  计划里「必要时退回屏幕分辨率」的降级因此落在**单张**粒度上，而不是整次导出。
- **导出计划排除 `available = false` 的条目**（计划只说「未删除、所在图层可见」）：缺失 DDS
  的位图画不出来，把它算进外框会在导出图上留一块空白。
- **块宽同时受设备 `maxTextureDimension2D` 约束**（计划只写了 8192），
  `canvasExportPlan` 取 `min(8192, 设备上限, 输出宽)`。
- **`png` 取 0.18**（与 `image 0.25` 依赖的同一版本，提为直接依赖不新增 crate）；编码用
  `Compression::Fast` + `Filter::Up`——大图的成本几乎全在编码上，导出的是交付图而不是仓库存储。
- **导出期间禁用入口统一走 `interactionState.exporting`**，模块内合成一个 `editingBlocked`
  （锁定 ∨ 导出中）；画板列表行加 `disabled` 与 `draggable=false`。
- **自动保存推后**：导出开始前取消挂起的自动保存，结束后若仍 dirty 再排程，保证导出期间
  不产生仓库写操作（与计划「不强制落库」一致）。
- `cargo fmt --check` 在新文件上干净，但 `pin_board/repository.rs` 有 6 处**既存**格式差异
  （上一批次提交的代码，当前 rustfmt 1.9.0 的重排口径不同）。实施阶段没有顺手重排，收尾时
  按「CI 强制 `cargo fmt --check`、而本批次本来就要改这个文件（取消检查）」的前提一次性
  格式化了那 6 处：纯格式改动（23 增 / 21 删），不改变任何行为。

自动验证（实际结果）：`npm run test:pin-board` 24 文件 / 252 用例通过（新增
`exportPolicy.test.ts` 14 条、`canvasExport.test.ts` 8 条）；`npx vitest run` 47 文件 /
429 用例通过；`npx tsc --noEmit`、`npm run build` 通过；`cargo test pin_board --lib` 83 用例
通过（新增 `canvas_export.rs` 的 10 条：流式 PNG 的尺寸与像素、多块拼一条行带、乱序/越界/
字节数不符拒绝、缺行拒绝、取消删除临时文件、仓库边界拒绝、重名不覆盖、文件名清洗、强杀残留
清理）；`cargo check --tests` 通过。

人工验收（GUI）：维护者 2026-10-21 实测通过，逐项结论见 `handoff-log-2026-10-21.md` 第 12 节。

### 阶段 2 交互修订（2026-10-21，维护者实测后）

第一版对话框与进度条交给维护者实测，收到的六条反馈已全部落地。这一节是**口径变更**，
以本节为准（上面的计划正文与实施记录保留为决策背景）：

- **不做耗时估算**（原计划「显示预计耗时」作废）。机器差异太大，一个不准的数字只会误导人；
  `estimatedSeconds` / `CANVAS_EXPORT_PIXELS_PER_SECOND` 与对话框的「预计耗时」行一起删除。
- **倍率改为手动输入**（原计划 1x/2x/4x 三档按钮作废）。默认值是「1x 与塞进建议上限取小」
  （`autoCanvasExportScale`，如 45000 宽、上限 20000 → 0.44），填过之后**以填的为准**：
  建议上限只提示不拦截，只有硬上限（边长 65535、总像素 10 亿，与原生 `MAX_OUTPUT_PIXELS`
  同口径）会收敛倍率并回写输入框。
- **倍率、输出宽度、边距三者互相咬合**：倍率是唯一事实来源，宽度是它的另一种写法（改哪个都
  把另一个算回来），边距只改画布范围。新增「宽度（像素）」输入框。
- **文件名默认取画板名、可手动改**，`.png` 作为固定后缀显示在输入框右侧。
- **边距默认 10**（`CANVAS_EXPORT_DEFAULT_MARGIN`），上限仍是 64。
- **排版重做**：导出位置独占一行，文件名独占一行，倍率 / 宽度 / 边距三等分——上一版边距输入框
  被拉成一条长条正是「两列 + 无最大宽度」造成的。
- **对话框底部说明精简**：删掉「上限 N 像素（取素材板设置里的纹理缓存等级）」与「导出期间画板
  锁定，可随时取消」两句，只留「同目录已有同名文件时自动追加序号，不会覆盖。」上限信息移进
  超限警告（只在真超限时出现）。
- **空白处右键菜单新增「导出画布」**（原名「导出整画布」，验收后按维护者要求定名为「导出画布」），
  与工具栏按钮打开同一个对话框。
- **进度条重做成一条共用模板**（`PinBoardTransferProgress`）：一行放下「在做什么 / 进度 /
  百分比 / 取消」，取消与进度条同排；导入图片、导出图片、整画布导出共用它。为此新增
  `cancel_pin_board_transfer` 命令与 `pin_board::transfer::PinBoardTransferCancel`（原子布尔，
  传输开始时复位，原生循环在条目边界检查）：导入取消整笔回滚、导出取消保留已写出的完整 PNG。
  全屏下不再隐藏进度条——它是导出期间唯一的取消入口。

自动验证（修订后重跑）：`npm run test:pin-board` 24 文件 / 253 用例通过（`exportPolicy.test.ts`
15 条按新口径重写）；`npx vitest run` 47 文件 / 430 用例通过；`npx tsc --noEmit`、`npm run build`
通过；`cargo test pin_board --lib` 86 用例通过（新增导入/导出取消各一条：取消的导入不留条目与
DDS、取消的导出不写文件）；`cargo check --tests` 通过。

## 风险

1. **离屏渲染通道是本项目第一次**。渲染器现在只画到 swap chain，需要抽出「画到指定目标 +
   指定视口」的通道，并处理目标格式管线副本、笔迹缓冲的收集框、导出 uniform 的隔离。
   这是全计划最可能超预期的一块；若成本过高，可先只支持「输出长边 ≤ 8192 的单块导出」
   （不分块、不拼装、前端 `toBlob` 写盘），把分块留到下一批。
2. **按导出倍率重载纹理是速度主因**。位图多且大的画板，耗时主要花在重读 DDS 与上传上，
   而不是分块或编码；预算不够时会反复淘汰。降级方案见 2.2 末。
3. **原始字节入参是新范式**，项目里没有先例，需要先跑通一次最小用例（一块 33 MB）再铺开。
4. **大图的长耗时与磁盘占用**。只按边长限制是维护者的选择，因此 9 亿像素这一档仍可发起：
   Rust 侧流式写盘保证内存不随输出尺寸增长，但时间和最终文件体积是真实成本，只做提示不拦截。
5. **粘贴落位规则变更**会影响已验收的位图粘贴行为，必须在 `CHANGELOG.md` 记 Changed 并单独验收。

## 文档同步

| 文件 | 改动 |
| --- | --- |
| `docs/modules/pin-board.md` | 「绘制」补橡皮擦与「大小」口径；「图层可见性」补计数；「命令面」替换粘贴命令、新增导出命令组；「快速验证」补用例 |
| `docs/planning/todo.md` | 待决策项（粘贴落位）与未验证项（导出人工验收） |
| `docs/planning/current-handoff.md` | 阶段开始时替换「当前批次状态」 |
| `CHANGELOG.md` | 只写 `## Unreleased`：阶段 1 的 Added / Changed / Removed，阶段 2 的 Added |
| 设置页文案 | 「纹理缓存等级」的说明补一句：同时决定整画布导出的最长边（10000 / 20000 / 30000） |

## 验证命令

```bash
npm run test:pin-board
npm run build
cargo test pin_board --lib
cargo check --tests
```

完整编译、GUI 流程与大文件测试由维护者安排，口径见 `docs/guides/validation.md`；
`src-tauri/tests/` 下的压力测试只在发布前运行，本计划不涉及。
