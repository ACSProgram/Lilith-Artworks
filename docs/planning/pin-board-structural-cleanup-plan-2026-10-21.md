# 素材板结构收尾计划（2026-10-21）

> 工作计划：按批次落实 2026-10-21 素材板结构审查的结论。只做行为保持（behavior-preserving）
> 的结构整理，附带的两个正确性修复在批次一明确列出。全部批次完成后本文件移入
> `archive/`，由索引与 `CHANGELOG.md` 承担记录。
>
> 进展：批次一已于 2026-10-21 落实并提交（自动验证通过）；批次二、三待做。

## 背景

素材板（位图/文字/笔迹三种条目、绘制、橡皮、图层、整画布导出、后台结算）功能已收口。
结构审查发现三类问题：

1. **超大文件**：`src-tauri/src/pin_board/repository.rs`（4661 行，五类职责）、
   `src/modules/pin-board/renderer.ts`（4235 行，五类职责）、
   `src/modules/pin-board/PinBoardModule.tsx`（1820 行，页面 + 7 个内嵌组件 + 16 个状态）、
   `src/modules/pin-board/usePinBoardController.ts`（936 行，六类职责）。
2. **补丁堆积**：新增条目能力以 `kind` 分支叠加在旧图片流程上；建条目主流程在
   repository 内复制五份；重复常量（`MAX_STROKE_WIDTH` 两义、`STROKE_LAYER` 两份、
   256 对齐两个名字）；命名漂移（`imageCount` 实为数全部 kind 条目）。
3. **过期注释与文档**：`stroke.rs`「阶段 2」、`repository.rs`「3 只有笔迹能落」、
   `dds.rs`「位图与文字合计」、`overview.md` 的生命周期与聚合模型、
   `pin-board.md` 多处时态与数字、表层 planning 文档保留已归档批次、
   `CHANGELOG.md` Unreleased 出现两个 `### Added`。

另发现两个**实质风险**（非纯风格）：进程级纹理结果缓存键不含仓库身份（切仓库后
id 复用可能串图）；`validate_editable` 允许位图/文字带 layer 3 落库，与注释、文档矛盾。

## 总原则

- 行为保持优先：除批次一明确列出的两个契约修复外，不改任何可观察行为。
- 不做 GUI 自动化验证；界面观感变化（对话框复用本地弹窗外壳）只改结构不动样式类，
  视觉结果留给人工验收。
- 每批结束跑该批验证并单独提交；提交信息按 AGENTS.md 规范。
- 拆分是移动不是重写：函数体保持原样，仅在移动处补必要的可见性与注释。

## 批次一：契约缺口与卫生清理

**目标**：消除两个正确性风险与全部死代码/重复常量/过期注释，文档与实现对齐。

1. **纹理缓存加仓库身份**（正确性）
   - `dds.rs`：`TextureCacheKey` 增加 `String`（仓库根路径）分量，
     `texture_cache_get/store` 增加参数；`repository.rs::read_texture` 传入 `root`。
   - 新增测试：同 `(board, item, dim)` 不同仓库根互不命中。
2. **图层契约收紧**（正确性）
   - `validate_editable` 改为带 `kind`：非笔迹条目 `layer > 2` 拒绝，笔迹维持
     `effective_layer` 强制 3；更新 `save_board` 调用点与注释。
   - 新增测试：位图/文字 layer 3 被拒。
3. **死代码与重复常量**
   - 删除 `session.ts::dropArtworkSessions`（键与 map 不符，仅测试引用）及其测试。
   - 删除 `strokeGeometry.ts` 的 `STROKE_LAYER`（无人引用）与 `MAX_STROKE_WIDTH = 1000`
     （与 `strokeDrawing.ts` 的 500 同名不同义；契约值由 Rust 侧持有）。
   - `renderer.ts` 去掉 `PREVIEW_TEXTURE_DIMENSION`、`TEXTURE_LOAD_MEMORY_BUDGET_BYTES`
     两层别名，直接使用 `texturePolicy` 原名。
   - `stroke.rs`：清理「阶段 2」注释与不再需要的 `#[allow(dead_code)]`（形状工具常量
     改为契约注释 + 最小可见性）。
4. **命名对齐**：`imageCount`（`PinBoardSummary` 与 `PinBoardInteractionState`）
   改名 `itemCount`，Rust `image_count` 同步，UI 文案与文档同步——它数的是全部 kind 条目。
5. **过期注释**：`repository.rs` 分节注释（621、928、1755 附近）、`dds.rs:30` 上限注释。
6. **文档**
   - `docs/modules/pin-board.md`：领域模块序数（第五→第四）、文字进度口径、
     「阶段 2」将来时、「0.3」残留、导出块宽补设备上限、硬上限 65535 标注为前端自设、
     上下文入口补 `usePinBoardController.ts`/`usePendingSettle.ts`/`boardOrder.ts`、
     「排除笔迹的纹理路径」清单按实际处数核对。
   - `docs/architecture/overview.md`：生命周期段去掉「画板切换走
     `preparePinBoardRuntimeChange`」；聚合模型补笔迹。
   - `docs/architecture/ai-reading-guide.md`：路由关键词补笔迹/图层/整画布导出。
   - `docs/planning/current-handoff.md`、`todo.md`：删除已归档素材板批次的表层段落；
     本计划作为当前批次写入 current-handoff。
   - `CHANGELOG.md`：合并 Unreleased 里两个 `### Added`（161–175 移入 18 段之后），
     压缩测试体检的内部细节；本批行为变化写入 Unreleased。

**验证**：`cargo fmt --check`、`cargo test --lib pin_board`、`npm run test:pin-board`、
`npx tsc --noEmit`、`git diff --check`。
**提交**：`Fix pin-board contract gaps and stale references`（含计划文档）。

## 批次二：Rust 持久化拆分与公共流程

**目标**：repository.rs 按职责拆分，建条目公共流程收敛为一份，路径/常量归位。

1. `src-tauri/src/pin_board/repository.rs` → `repository/` 目录：
   - `mod.rs`：模块文档、公开 re-export（保持 `pin_board::repository::*` 调用面不变）；
   - `paths.rs`：`board_relative_path`、`parse_board_dds_path`、`is_dds_name`、
     `board_directory`、`canonical_item_dds`（合并现散在 6 处的「规范文件名 +
     仓库边界」检查，`source_dds_path`/`bitmap_source_path` 归一）；
   - `boards.rs`：画板 CRUD/回收站/排序/`list_boards_inner`/`row_summary`；
     顺手清掉空 `{}` SQL 槽与 `restore_board` 不参与筛选的 JOIN；
     `trash_board_context` 改返回真实需要的字段，不再构造假 `BoardContext`；
   - `view.rs`：`open_board_context`、`current_states`、`item_available`、`load_view`、
     `validate_editable`、`effective_layer`、`same_state` 与状态结构体；
   - `save.rs`：`save_board`、`finalize_board`、`finalize_board_in_transaction`；
   - `create.rs`：`begin_item_mutation`、`record_initial_state`、粘贴/导入/文字/笔迹、
     载荷 INSERT helper（`insert_text_payload`/`insert_stroke_payload`）、
     事务收尾 helper（bump_revision → commit → `created_files.commit()` → `load_view`）；
   - `bitmap_io.rs`：导出 PNG、`read_image_png`、`read_texture`、`publish_export_png`；
   - 测试按归属拆到各子模块 `#[cfg(test)]`；`migration_tests` 若不依赖 repository
     私有项则移入 `library/schema.rs`，否则留在 `repository/tests.rs` 并注明原因。
2. `imported_points` 从 `dds.rs` 移到 `arrange.rs`（落位数学归位）；
   `MAX_BOARD_ITEMS`、`MAX_IMPORT_FILES`、`MAX_CLIPBOARD_IMAGE_BYTES` 移到
   `contracts.rs`，`dds.rs` 只留图像格式与纹理缓存。
3. 重复文案（画板冲突、redo 作废 DELETE、`imported_points(..).next()`、`FULL_UV`、
   中转层 `layer: 1`）抽成常量/helper。
4. 顺手项：`headless.rs` 的导入命令把「条目数」输出为 `imageCount`（消费方
   `tests/stress_bench.rs` 按这个键取值），与批次一的命名对齐一致后应同样改成
   `itemCount`；批次一没动它，因为那是无头输出契约，且只有发布前的压测才会跑到。
5. 暂不做：跨模块共享的「临时文件 + 原子改名 + 重名序号」发布 helper（canvas_export
   是流式写盘，形态不同；抽共享设施需要动 cleanup/backup 语义，单独评估）。

**验证**：`cargo fmt --check`、`cargo check`、`cargo test --lib`（全量，
确认移动未破坏引用）、`git diff --check`。
**提交**：`Split pin-board persistence into focused modules`。

## 批次三：前端组件与渲染器拆分

**目标**：页面组件、渲染器与控制器各拆出独立关注点；重复测试合并。

1. `PinBoardModule.tsx`（1820 → 约 900 行）：
   - `PinBoardSidebar.tsx`（画板列表/回收站/拖放排序 + 列表右键菜单）；
   - `PinBoardToolbar.tsx`（主工具栏，22 个按钮按动组分块）；
   - `PinBoardCanvasStage.tsx`（`GpuCanvas`、占位/锁定/投放提示、标注工具条、
     图层面板挂载）；
   - `PinBoardDialogs.tsx`（文字对话框、整画布导出对话框 + 模块本地
     `PinBoardDialog` 外壳：统一 backdrop/Escape/头部，样式类不变）；
   - `PinBoardContextMenu.tsx`（画布右键菜单）；
   - `PinBoardTransferProgress` 随状态行移入 stage 或独立小文件。
   - 工具键位表（`PinBoardModule.tsx` 182–195 与 `strokeDrawing.ts` 60–68）收敛为
     `strokeDrawing.ts` 一份，界面标签从它派生。
2. `renderer.ts`（4235 → 约 3000 行，本批不拆指针状态机）：
   - `rendererGpu.ts`：着色器、`createBoardPipelines`、GPU 创建/丢失恢复辅助、
     纹理上传/销毁、DDS 头校验、readback 对齐常量；
   - `strokePreview.ts`：DOM 2D 预览绘制（草稿→canvas，与落库共用几何口径）；
   - `rendererExport.ts`：导出目标纹理、行带回读、导出笔迹缓冲构建
     （以显式参数访问渲染器内部，不经私有字段）。
   - `strokeRender.ts` 不再承担 BLOB 编解码：编码移入 `strokeGeometry.ts`
     （`decodeStrokeHeader` 已在），`strokeDrawing.ts` 的反向依赖解除。
3. `usePinBoardController.ts`（936 → 约 600 行）：
   - 抽 `usePinBoardTransfers`：导入（选择器/剪贴板/拖入）、导出、整画布导出、
     文字、笔迹队列的 busy/进度/取消样板与 `replaceAfterImageMutation` 链；
     以显式依赖（renderer ref、selected、settings、setStatus）注入。
4. 测试整理：
   - `renderer.test.ts` 的「ink hit testing」describe（1756–1869）与
     `strokeHit.test.ts` 重复的用例合并进 `strokeHit.test.ts`，renderer 只留分派；
   - `strokeItem`/`geometryFor` 夹具抽共享 helper。

**验证**：`npx tsc --noEmit`、`npm run test:pin-board`、`npm test`、`git diff --check`。
**提交**：`Split pin-board UI components and renderer helpers`。

## 收尾

- 计划文件移入 `docs/planning/archive/`，更新 `archive/README.md` 索引；
- `current-handoff.md` 清掉本批次状态，`todo.md` 移除已完成项、保留
  「对话框观感与渲染器拆分后的界面行为待人工验收」一类确实未验证的事项；
- `CHANGELOG.md` Unreleased 按三批行为变化归类；
- 最终验证：`cargo fmt --check`、`cargo test --lib`、`npx tsc --noEmit`、
  `npm run test:pin-board`、`git diff --check`；GUI 观感留人工验收。

## 风险与回退

- 批次二拆分量大但机械；以 `cargo test --lib` 全量通过为门。
- 批次三涉及渲染器内部引用，测试覆盖（renderer.test 90 例）是安全网；
  若拆分中发现某块与类状态耦合过深，允许留在原地并在计划上注明，不硬拆。
- 两个正确性修复（缓存键、图层校验）如遇隐藏依赖（无头入口、测试夹具依赖旧行为），
  先修夹具再改契约，不放宽校验。
