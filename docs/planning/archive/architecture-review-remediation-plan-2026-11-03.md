# 架构审查整改计划（2026-11-03）

状态：**已实施并归档（2026-11-03）**。B1–B5 均已实施并通过自动化验证；B2 的菜单项、B4 的
仓储流程与 B5 的画板流程仍需维护者手工复验，余项登记在 `../todo.md` 的「人工验收（剩余）」。
本文件自此为历史记录，不再作为执行依据；有效契约以 `docs/architecture/`、`docs/modules/` 与
`docs/guides/` 为准。

2026-11-03 二次修订：已对照 Codex 复核意见调整范围与验收方式，并把工作项合并为 **5 个批次**。
复核原文归档为同目录的 `architecture-review-remediation-plan-review-2026-11-03.md`，采纳与
部分采纳的结论见第 2 节。整改依据仍是 2026-11-03 的全量只读架构审查；审查未运行构建与
测试，也未修改代码。

## 1. 目标与边界

### 1.1 目标

1. 修正确认过的文档事实错误、过期注释与确定无用的代码。
2. 消除真实存在、且行为一致的重复实现；不把“相似”当作“重复”。
3. 对确有多个独立变更理由、且能建立测试接缝的文件做拆分；拆分候选全部评估，
   不预设每个候选都必须拆。
4. **做减法**：删除零引用导出与未使用层级；合并只有一个消费者、没有独立测试接缝的
   小抽象。不为统一而统一，不为拆分而拆分。
5. 行为修正、纯重构与文档修正分开验收；任何可能影响 UI、保存载荷、SQL 事务或错误语义
   的改动，都必须有对应的回归测试或维护者手工确认。

### 1.2 非目标

- 不改变产品行为、命令名、DTO 字段、仓库格式、schema 版本与锁语义；行为修正项单独标注。
- 不切版本：五处版本字段保持 `0.3.0-beta.1`，改动只进 `CHANGELOG.md` 的 `## Unreleased`。
- 不运行压力测试（`src-tauri/tests/` 下的取消、崩溃、大文件端到端），除非维护者另行安排。
- 不执行 GUI 自动化与鼠标键盘模拟；界面与交互由维护者手工验收。
- 不提交 Git；完成前不创建任何 commit，由维护者统一审阅后决定提交粒度。
- 不为“消除跨模块 SQL”本身设定目标；只有事务 API、所有权边界与调用方向都清楚时才重构。

### 1.3 反过度设计准入

每项任务开始前先写一句“要解决的具体问题”；写不出来就不做。另加四条准入条件：

1. **抽象**：至少 3 处行为一致的重复，或涉及跨模块一致性/事务边界等必须固定的不变量。
   只有 2 处相似但语义不同的代码，保留各自实现。
2. **拆分**：一个文件至少有 2 个独立变更理由，拆出的边界能单独测试或单独演进，且不需要
   传递宽泛的可变状态。单一职责的长文件不拆。
3. **合并**：只有一个消费者、没有独立测试接缝、且不会成为公共契约的小抽象，合并回使用方；
   已有 2 个以上真实消费者的共享底座保留。
4. **统一**：先修行为差异，再谈接口统一；如果统一会引入新参数表或新分支，停止并记录为
   “不做”。

### 1.4 规模与验收口径

用户要求批次规模约 2–3k 行；本计划保留这一规模作为**排序与工作量参考**，不作为完成标准：

- 纯移动、重命名、缩进调整按实际行数的 10% 估算，避免“拆文件看起来很多”。
- 文档修正、行为修正与调查工作允许低于 2k；不为凑量扩大范围。
- 每批验收看“修复了哪项缺陷、减少了哪些真实重复、建立了什么测试接缝、契约是否保持”，
  不看新增行数、文件数或批次数。
- 每批记录实际改动范围；偏离规模参考超过 30% 时说明原因，但不因此补做任务。
- 若单批内容超过约 3k，可在批内分两个里程碑验收，但不新增批次。

### 1.5 变更类型标签

计划中的工作项统一标注以下一种或多种类型，便于分开验收：

- `[文档]`：只改文档、注释与归档索引。
- `[纯重构]`：行为与契约不变，测试应证明等价。
- `[行为修正]`：有意改变用户可见行为，必须单独定义验收。
- `[拆分]`：以模块边界与测试接缝为目的，不预设行为变化。
- `[调查]`：证据不足，先确认调用点、语义与收益，再决定是否实施。

## 2. 复核意见采纳情况

| 复核意见 | 结论 | 处理方式与批次 |
| --- | --- | --- |
| 1. “等效新增代码”会奖励不必要的改动 | 部分采纳 | 保留 2–3k 作为规模参考，删除硬性偏差约束；验收改为结果导向（1.4）。 |
| 2. 批次规模与顺序缺少风险隔离 | 采纳 | 合并为 5 批，但文档/低风险清理、前端行为修正、Rust 重构、仓储高危拆分、renderer 高危拆分各自独立；行为修正不混入“行为不变”批次。 |
| 3. 右键菜单统一带来行为变化 | 采纳 | 放在 B2，标注 `[行为修正]`，单列键盘/滚动/缩放/定位/焦点验收。 |
| 4. 保存载荷“完全一致”不符合契约 | 采纳 | B2 不合并两条保存路径的输出；分别命名“切换快照”与“常规保存”契约。 |
| 5. 跨领域删除缺少事务传递设计 | 采纳 | B4 默认保留现有单事务实现；登记表所有权与例外，并用回滚测试锁住原子性。 |
| 6. 共享 DTO 的放置与 API 边界 | 部分采纳 | B1 把 `BackupRuntimeStatus` 放入中立 DTO；History 继续经自己的 `api.ts` 调用。`PinBoardTextureCacheLevel` 以 `pin-board/types.ts` 为唯一定义，`app/types.ts` 与 `texturePolicy.ts` 只导入；`app` 作为组合根允许依赖模块公开类型，模块不得反向导入 `app`。 |
| 7. `storage.rs` 可能扩张为杂项工具箱 | 采纳 | B3 只提取行为完全一致的纯算法；sidecar、不覆盖发布、`with_connection`、路径唯一化逐项调查，不预设全部提取。 |
| 8. 拆分目标过度预定，facade 行数不是质量指标 | 采纳 | B3/B4/B5 的拆分候选全部保留，但实施前先画依赖/状态所有权与消费者；取消“九个模块”“600–800 行”等硬目标。 |
| 9. 测试组织不应成为独立改造目标 | 采纳 | 测试随生产代码拆分迁移，避免扩大可见性；不以测试数量作为验收。 |
| 10. “只被测试使用”不能直接推出死代码 | 采纳 | B1 保留 `TOOL_PEN`（生产序列化）与 `delete_subtree`（防御性测试入口），补充用途说明。 |
| 11. 最终批次重复前批次事项 | 采纳 | 每项只指定一个负责批次；B5 收尾只核对记录、未决项与文档完整性，不重复删除或重做决策。 |
| 补充：证据与退出条件、行为/重构分离、批间依赖、SQL 例外、GUI 验收、CHANGELOG 归类 | 采纳 | 写入第 3–5、7、8 节。 |

## 3. 五批次总览

批次顺序按依赖与风险排列，不预设总代码量或完成时间。每批完成后先在自身范围内验收，
再决定是否进入下一批。

| 批次 | 类型 | 范围 | 依赖 | 规模参考 | 退出条件 |
| --- | --- | --- | --- | --- | --- |
| B1 | `[文档]` + `[纯重构]` | 文档事实、归档索引、过期注释、确定死代码、DTO 单一定义、`HistoryControls` 反向依赖 | 无 | 2–2.5k | 每项事实有证据；引用检查通过；行为与契约不变 |
| B2 | `[行为修正]` + `[纯重构]` | 右键菜单行为修正；素材板保存契约测试；前端几何/控件/预览/维护 hooks 去重与小拆分；CSS 边界 | B1 | 2.5–3k | 菜单手工验收；保存契约测试通过；前端测试通过 |
| B3 | `[纯重构]` + `[拆分]` | Rust 摘要/哈希收敛；sidecar/发布/连接/路径候选调查；`create/cleanup/storage_overview/dds/rendition` 拆分；应用层 usecase 收口 | B1 | 2.5–3k（折算） | 每个候选有依赖/消费者/测试接缝证据；命令行为不变 |
| B4 | `[拆分]` | `library` / `history` 仓储与 schema 拆分；跨域 SQL 登记与回滚测试 | B3 | 2.5–3k（折算） | 契约不变；回滚测试通过；例外有登记 |
| B5 | `[拆分]` + `[文档]` | 素材板 `renderer.ts` 拆分；收尾审计、未决项、CHANGELOG、仓库卫生 | B2、B3、B4 | 2.5–3k（折算） | 画板手工验收；变更记录与文档完整；无重复决策 |

### 3.1 批次独立性说明

- B1 独立，可单独评审与合并。
- B2 依赖 B1 的 DTO 与共享控件结论；B2 内先完成右键菜单行为修正与手工验收，再做纯重构，
  避免行为问题与拆分问题混在一起定位。
- B3 与 B2 互不依赖；B3 内按“先纯算法、再候选 helper、再生产拆分、最后应用层”的顺序，
  每个生产拆分候选单独通过检查后再做下一个。
- B4 依赖 B3 的 helper 与拆分结论，但不依赖 B2/B5。
- B5 的 renderer 拆分依赖 B2 的保存契约测试；收尾审计依赖 B1–B4 全部完成。
- 每批完成后若维护者选择暂停，后续批次按上表依赖即可独立恢复，不需要重做前批次。

## 4. 批次详情

### B1 `[文档]` + `[纯重构]` 文档事实、死代码、DTO 与前端反向依赖

#### B1.1 文档事实与归档索引

- `docs/guides/release-policy.md`：删除“当前 `0.3.0-beta.1` 未打标签”的过时表述；
  `v0.3.0-beta.1` 已于 2026-10-25 打标签并作为 Pre-release 公开。保留“切版本不等于发布”
  的规则本身。
- `docs/planning/todo.md`：修正“alpha1–alpha6 与 beta.1 都是未打标签”的自相矛盾。
- `docs/guides/validation.md`：`src/fuzz_support.rs` 改为 `src-tauri/src/fuzz_support.rs`。
- `docs/planning/archive/README.md`：补 `handoff-log-2026-10-09.md` 索引；核对索引与实际
  归档文件一一对应。
- `docs/modules/history-and-backup.md`：删除 C9/C10“只有断言、没有实测记录”的已完成验收
  残留；该结论已在同目录的 `handoff-log-2026-10-25.md` 记录。
- `docs/modules/pin-board.md`：`cleanup::scrub_board_dds` 改为
  `pin_board::scrub::scrub_board_dds`；补充 `create.rs` 也导出 `read_strokes`。
- `docs/guides/release-policy.md`：CI 忽略清单补 `RUSTSEC-2026-0429`，与两个 workflow 对齐。
- `docs/architecture/overview.md`：日志轮转“4 MiB”改为实际 `4_000_000` 字节。
- 移除表层文档中的“0.3 阶段 1/3”“阶段 4/5”等批次叙事标题；已完成阶段只留归档。
- 收敛 `overview.md` 与 `ai-reading-guide.md` 约 19 处重复段落：保留一份权威描述，另一处
  改链接。

#### B1.2 过期注释

- `useArtworkSummary.ts` 的“`historyApi.get` 只有本 hook 消费”改为实际（历史 controller
  也在调用）。
- `coordinator.rs` 的“全库校验走 `run_foreground_read`”改为现状（`usecases/backup.rs`
  已走 `run_foreground`）。
- `cleanup_commands.rs` 的“与前台长命令同口径”改为实际的 `run_write` 语义。

#### B1.3 确定死代码

逐项 grep 复核后删除；任何一项发现动态引用或协议用途就保留并记录。

- `src/modules/library/tree.ts` 的 `containsNode`（`LibraryTreeView` 已有私有同实现）。
- `src/modules/pin-board/geometry.ts` 的 `boundsCenter`。
- `src/modules/pin-board/texturePolicy.ts` 的 `DEFAULT_PIN_BOARD_TEXTURE_CACHE_LEVEL` 及只被
  测试引用的重复预算常量；保留 `textureBudgetsForLevel` 的实际使用者。
- `src/shared/useRequestEpoch.ts` 的 `current()`。
- `src/app/useSettingsController.ts` 中仅为内部常量却导出的 `EMPTY_REPOSITORY_STATUS`。

**明确保留**

- `TOOL_PEN`：用于笔迹序列化（`stroke.rs`、`view.rs`），不是测试夹具，保留。
- `delete_subtree`：与 `_prechecked` 生产入口不同的防御性测试路径，保留并补用途注释。
- `src-tauri/gen/schemas/`：先确认生成依据，B5 再决定，不在本批删除。

#### B1.4 DTO 单一定义

- `BackupRuntimeStatus` / `BackupTaskKind`：放入 `src/shared/` 的中立运行时 DTO 文件，供
  `src/app/api.ts` 与 `src/modules/history/api.ts` 共同引用；删除两份定义。
- `get_backup_runtime_status`：两个前端命令名保持不变；两个 `api.ts` 各自保留类型化包装，
  History 不改走 `app/api`；只消除类型重复。
- `PinBoardTextureCacheLevel`：以 `src/modules/pin-board/types.ts` 为唯一定义，
  `texturePolicy.ts` 与 `src/app/types.ts` 改为导入；删除三处 union 中的另外两份。
  `app` 作为组合根可以依赖模块公开类型，模块不得反向导入 `app`；在差异记录中写明方向。

#### B1.5 前端反向依赖

- `src/modules/history/HistoryControls.tsx` 不再导入 `../../app/api`。`revealPathInFolder`
  由应用层通过 props 注入，或在 controller 中封装。
- `@tauri-apps/plugin-dialog` 的文件选择器使用不属于反向依赖，保留；目录 reveal 必须走注入。

#### B1 验收

- 文档 grep 核对：beta.1 标签、`src-tauri/src/fuzz_support.rs`、归档索引、`RUSTSEC-2026-0429`、
  `scrub_board_dds` 路径全部一致。
- `npx tsc --noEmit` 通过；受影响测试通过；引用检查无新增死代码。
- 没有 UI 行为变化；DTO 字段与命令名不变。

#### B1 实施记录（2026-11-03）

- **文档**：修正 beta.1 标签表述、`todo.md` 的 alpha/beta 标签矛盾、`src-tauri/src/fuzz_support.rs`
  路径、CI 的 `RUSTSEC-2026-0429` 忽略清单、`cleanup::scrub_board_dds` 路径与 `create.rs` 的
  `read_strokes` 说明；补 `handoff-log-2026-10-09.md` 归档索引；删除 C9/C10“只有断言”残留；
  日志轮转口径改为 `4_000_000` 字节；移除 `overview.md` 与 `todo.md` 的批次叙事标题与
  “0.3 阶段 3”口径；`overview.md` 与 `ai-reading-guide.md` 的重复描述收敛到单处并改为链接；
  按实际代码更正 `api.ts` 的调用方（历史侧含 `useArtworkSummary.ts`，素材板含窗口全屏、导出
  目录选择与渲染器纹理读写几条直接路径），并把 `BackupRuntimeStatus` 的位置写进模块文档。
- **注释**：`useArtworkSummary.ts` 更正 `historyApi.get` 的消费方；`coordinator.rs` 说明全库校验
  已走 `run_foreground`；`cleanup_commands.rs` 改为 `run_write` 语义；`delete_subtree` 与
  `TOOL_PEN` 补充保留用途。
- **死代码**：删除 `containsNode`、`boundsCenter`、`useRequestEpoch.current()`、
  `DEFAULT_PIN_BOARD_TEXTURE_CACHE_LEVEL`、两个只被测试引用的纹理预算常量与
  `EMPTY_REPOSITORY_STATUS` 的导出；`renderer.test.ts` 改从 `textureBudgetsForLevel("low")` 取值。
- **DTO**：新增 `src/shared/backupRuntime.ts`，`app` 与 `history` 两侧删除重复定义并直接引用；
  `PinBoardTextureCacheLevel` 只保留 `pin-board/types.ts` 一处定义，`app/types.ts` 与
  `texturePolicy.ts` 改为导入（方向为 `app` → 模块公开类型）。
- **反向依赖**：`HistoryControls.tsx` 不再导入 `app/api`；`onRevealPath` 由
  `ArtworkWorkspace` 注入（类型为 `HistoryControls.tsx` 导出的 `RevealPathAction`），未注入时
  不渲染该入口。
- **验证**：`npx tsc --noEmit` 通过；`npm test` 全量 56 个文件、507 项通过（含
  `test:pin-board` 与 `src/modules/library`）；未改动 Rust 逻辑，命令名与 DTO 字段未变。
- **偏差**：归档索引核对只有 `handoff-log-2026-10-09.md` 缺失；文档收敛保留 `overview.md`
  为架构描述的权威来源，`ai-reading-guide.md` 只保留路由与硬边界并链接过去。

### B2 `[行为修正]` + `[纯重构]` 前端行为修正与去重

#### B2.1 右键菜单行为修正（先做，单独验收）

**当前行为（已核对）**

- `src/shared/ui/ContextMenu.tsx`：点击外部、Escape、窗口 resize、任意滚动都会关闭。
- `src/modules/library/LibraryModule.tsx`：处理 Escape 与 pointerdown 外部点击，未处理
  scroll/resize。
- `src/modules/history/HistoryModule.tsx`：只监听 click 关闭，未处理 Escape、
  scroll/resize。

**目标**

- 两个模块接入共享 `ContextMenu`，统一 Escape、外部点击、滚动、缩放与定位行为；删除各自
  手写的菜单关闭逻辑与重复样式。该项是**有意行为修正**，不归入“行为不变”的重构。

**手工验收（维护者执行）**

- 键盘：菜单打开后按 Escape 关闭；焦点不跳到意外位置。
- 页面/树滚动：滚动任意祖先容器时菜单关闭；菜单不随内容错位。
- 窗口缩放/缩放级别变化：菜单关闭或重新定位，不超出视口。
- 定位与焦点：靠近窗口边缘打开时菜单不越界；关闭后焦点回到触发按钮或合理位置。

**退出条件**

- 若维护者要求本轮严格保持行为不变，则暂缓接入共享菜单，保留现状并登记为后续 UX 项。
- 接入后不允许保留第二套手写菜单关闭逻辑。

#### B2.2 素材板保存契约测试（不合并输出）

- 先写测试固定两条不同语义：
  - **切换快照**（`pendingSettle`）：文字条目总是携带完整文字载荷，确保切换画板时保留
    正在编辑的文字。
  - **常规保存**（`renderer.editableImages()`）：只在文字签名变化时携带载荷，避免无变化
    也推进步骤。
- 可共享字段克隆/转换的纯逻辑，但调用方保留“是否包含载荷”的决定权；不得断言两个完整
  保存请求相等，也不得把其中一条路径改成另一条。
- 测试覆盖 `text` / `stroke` / `bitmap` 三类条目；Rust `save_board` 的步骤推进语义不变。

#### B2.3 前端去重与小拆分

- 几何内核：箭头头部 `strokePreview.ts` 复用 `strokeRender.ts`；距线段/相交
  `strokeSampling.ts` 与 `strokeHit.ts` 收敛为纯函数；包围盒三份收敛到 `geometry.ts`。
  只有计算完全一致才合并；屏幕/导出尺寸分支若语义不同，保留并注明。
- `PublicationPreviewDialog.tsx` 拆出 `usePreviewViewport` 与 `usePreviewTiles`；组件只保留
  视图与弹窗结构。
- `HistoryControls.tsx` 的 650ms 防抖保存状态机移回 `useHistoryController`；`EditNodeDialog`
  若与 `PromptDialog` 语义一致则改用后者，否则保留但删除重复弹窗外壳。
- `useRepositoryMaintenance.ts` 按空间/备份/完整性拆为 3 个 hook，共享一个
  `runSettingsOperation`；不新建通用任务框架。
- `App.repositorySwitch.test.tsx` 按设置页/空间页/备份页拆分，文件名与覆盖范围一致。
- `useArtworkSummary` 改用 `useRequestEpoch`；`usePublicationController` 与
  `useIdentificationController` 的 `setBusy` 统一走 `useAsyncOperation`，保留发布流程自己的
  活动任务状态机。
- 共享 UI：根据 B1 之后仍存在的重复，决定 `Modal` 保留或合并；右键菜单与进度条只在语义
  一致时共用。只有 2 个消费者且差异超过 3 个 props 时，保留两份。
- CSS：删除 `.tree-root` 死规则与 `history.css` 重复的 `.segmented-control`；共享
  Modal/ContextMenu 样式回到 `index.css`。若 `index.css` 中 library/settings 规则的拆分
  会影响加载顺序，本批不做并记录，不强行拆样式文件。

#### B2 验收

- 右键菜单四项手工验收通过；若暂缓，记录为后续 UX 项。
- 保存契约测试通过，且明确断言两条路径的差异，而不是相等。
- `npx tsc --noEmit` 与前端测试通过；没有新增通用弹窗/菜单/任务框架。

#### B2 实施记录（2026-11-03）

- **右键菜单**：`ContextMenu` 改用 `useMenuPlacement` 按测量尺寸落在视口内；素材板、作品树、
  作品树的「新建」菜单与历史节点菜单共用这一套 Escape / 外部点击 / 滚动 / 缩放关闭逻辑。
  删除作品树与历史页各自的手写关闭监听、`useMenuPlacement` 调用、`CommandMenu` 与
  `context-separator`，作品树的「新建」入口改为共享菜单定位。`EditNodeDialog` 改用共享
  `PromptDialog`（新增 `eyebrow` / `busy`），因此重命名也能用 Escape 关闭并在打开时选中原文。
- **保存契约**：新增 `pendingSettle.ts` 与 `renderer.ts` 共用的 `savePayload.ts`，只共享字段
  克隆与文字载荷构造；切换快照继续总是携带文字载荷，常规保存只在签名变化时携带。测试覆盖
  `text` / `stroke` / `bitmap` 三类条目，并只断言各自路径的取舍，不断言两个完整请求相等。
- **几何与 hooks**：`strokePreview.ts` 的箭头头部改用 `strokeRender.arrowHeadPoints`；点到线段
  距离与点集包围盒收敛到 `geometry.ts`（`distanceToSegment`、`pointsBounds`），
  `strokeRender.boundsQuad` 也复用 `pointsBounds`。`PublicationPreviewDialog` 拆出
  `usePreviewViewport`（缩放、滚动、拖拽、导航框）与 `usePreviewTiles`（局部图去抖与缓存）。
  分支设置的 650ms 防抖状态机移到控制器层 `useBranchSettingsDraft.ts`；`useArtworkSummary`
  改用 `useRequestEpoch`；发布与识别控制器的忙状态改由 `useAsyncOperation` 计数维护，
  发布流程自己的活动任务状态机保持不变。
- **设置维护**：`useRepositoryMaintenance.ts` 变为组合根，空间、备份、完整性三个域 hook
  （`useRepositorySpaceMaintenance` / `useRepositoryBackupMaintenance` /
  `useRepositoryIntegrityMaintenance`）共享 `useSettingsOperationRunner` 的同一操作闸门与
  进度状态；对外返回类型与页面消费方式不变。
- **测试组织**：`App.repositorySwitch.test.tsx` 按设置/仓储页、备份页、空间页拆为
  `App.settingsPage.test.tsx`、`App.backupPage.test.tsx`、`App.spacePage.test.tsx`，夹具移到
  `repositoryTestSupport.ts`；覆盖范围与文件名一致，原 20 项用例全部保留。
- **CSS**：共享 `ContextMenu` / `Modal` / `PromptDialog` 样式移入 `index.css`，素材板样式表
  只保留模块规则；删除 `.tree-root` 死规则、`history.css` 重复的 `.segmented-control` 与
  已无消费者的 `.compact-dialog`、`.command-menu`、`.dialog-input`、`.field-label`。
- **验证**：`npx tsc --noEmit` 通过；`npm test` 全量 58 个文件、509 项通过（含素材板与新增的
  保存契约用例）；未改动 Rust 代码、命令名、DTO 字段与版本号；未提交 Git。
- **偏差**：右键菜单的键盘/滚动/缩放/定位/焦点四项仍待维护者手工验收，已登记到
  `todo.md` 的「人工验收（剩余）」。`useBranchSettingsDraft` 放在控制器层独立文件而不是
  `useHistoryController.ts` 内部，避免 `HistoryModule.test.tsx` 对控制器整模块 mock 时连带
  失去草稿 hook；仍满足「防抖状态机不再属于控件」的目标。`index.css` 中 library/settings
  规则的进一步拆分未做：会改变加载顺序，按计划留待后续批次并记录。

### B3 `[纯重构]` + `[拆分]` Rust 基础收敛、生产拆分与应用层收口

#### B3.1 纯算法收敛（先做）

- 只提取行为完全一致的纯算法：整文件 SHA-256、首尾 64 KiB 采样、`ZERO_SHA256`。
- 调用点先列清楚：`storage.rs`、`backup/repository_backup.rs`、`backup/worker.rs`、
  `cleanup.rs`、`authenticity/file_ops.rs`、`authenticity/state.rs`。若取消需求、错误类型或
  采样语义不同，保留各自实现，只共享纯计算部分。
- 相同输入必须产生相同摘要与采样字节；不改变错误映射与取消行为。

#### B3.2 文件、路径与连接候选（调查后决定）

- sidecar 清理：`library/repository.rs` 与 `backup/repository_backup.rs` 的文件集合、
  错误上下文与 `-journal` 差异。
- 不覆盖发布：`worker.rs` 封装与 `restore.rs` 三处直接 `persist_noclobber`；确认临时文件
  同步、错误类型与目录前置条件是否一致。
- 连接闭包：`storage::open + map_err` 的重复形状、事务边界与慢日志需求；确认收益后再加
  `with_connection`。
- 路径唯一化：`bitmap_io.rs`、`canvas_export.rs` 的 `-2/-3` 规则与认证不覆盖发布的差异。
- 每项写出调用点、输入/输出、取消需求、错误映射与差异；只有行为一致且能减少真实维护
  分叉时才提取。不同策略不塞进 `storage.rs`。

#### B3.3 Rust 生产文件拆分（候选制）

候选：

- `pin_board/repository/create.rs`：粘贴源解析、导入、文字/笔迹写入、`read_strokes`。
- `cleanup.rs`：队列、重放、扫描、删除边界、temp 清扫。
- `storage_overview.rs`：记录口径与磁盘口径。
- `pin_board/dds.rs`：编解码、纹理缓存、原子落盘。
- `authenticity/rendition.rs`：配置归一、渲染编码、缓存目录/淘汰。

每个候选单独做：

1. 画出当前依赖与状态所有权；列出 `pub`/`pub(crate)` 消费者。
2. 确认至少 2 个独立变更理由；确认拆出的边界可单独测试，不需要传递宽泛可变状态。
3. 选择一个候选先做，测试随生产代码迁移；通过后再做下一个，不一次性铺开。

明确不做：

- 不把所有内联测试整体搬到兄弟 `tests.rs`；只随实际生产拆分迁移相关测试，避免扩大可见性。
- 不为达到文件行数目标而拆；单一职责的长解析器（`preview/psd.rs`、`preview/sai2.rs`、
  `stroke.rs`）不拆。
- 若某个候选拆分后只剩薄封装或需要暴露更多 `pub(super)`，回退并记录“不拆”。

#### B3.4 应用层 usecase 收口

- `authenticity/commands.rs` 中直接调用 `repository::*` / `preview::*` 的分支收回
  `app/usecases/authenticity.rs`；适配器只保留 DTO 解析、授权与错误映射。
- `RepositoryScrubReport` 从 `app/workflows.rs` 移到 usecases 的类型位置，消除 usecase 对
  适配器的反向依赖。
- `usecases/pin_board.rs`、`library.rs`、`history.rs` 的 GUI/headless 成对 `cfg` 变体：
  只有合并后不引入运行时分派时才合并；否则保留并记录原因。
- `settings.rs`、`library.rs` 中在 `run_read` 之外读取仓库路径的入口改走 `run_read`。
- 错误形态与取消通道：先列所有前端消费者与现有语义；只有能统一且不改变用户文案时才做，
  否则保留现状并登记例外。

#### B3 验收

- `cargo fmt --check` 通过；`cargo test --lib` 全量通过；纯移动前后测试断言不变。
- 每个拆分候选有依赖/状态所有权与消费者证据；未拆项记录理由。
- 命令行为、错误文案、取消语义与事件不变；文档入口同步。
- 若本批超过约 3k，按“算法/候选 → 生产拆分 → 应用层”三个里程碑分别验收，不新增批次。

#### B3 实施记录（2026-11-03）

**B3.1 摘要收敛**

- 新增 `src-tauri/src/digest.rs`：整文件 SHA-256 流式读取（`sha256_reader`，另有不带取消
  通道的 `sha256_reader_without_cancel`，让本来就没有取消语义的调用点不必各写一个永不发生
  的 `Cancelled` 分支）、首尾各 64 KiB 采样读取（`read_sample` / `sample_sha256`）、边复制
  边累计完整摘要与采样的 `SamplingDigest`，以及测试用的 `ZERO_SHA256`。只放纯算法，文件
  打开、取消检查与错误文案仍由调用方决定。
- 收敛调用点：`storage::verify_file_sha256`、`cleanup::sha256_file`、
  `authenticity::file_ops::sha256_file_with_check`、`authenticity::state::file_sha256`、
  `backup::repository_backup` 的完整摘要与抽样复核、`backup::worker::source_fingerprint`
  的首尾采样。`repository_backup` 的 `DigestAccumulator` 移入 `SamplingDigest`；采样宽度
  常量统一为 `digest::SAMPLE_BYTES`。`authenticity::contracts` 与
  `publication_repository` 测试中的 `ZERO_SHA256` 改为引用同一常量。
- 相同输入产生相同摘要与采样；错误映射与取消文案未变。`sha256_reader` 仍在每次读取前检查
  取消；`sample_sha256_file` 改由一次 `read_sample` 完成首尾两次读取，取消检查因此落在读前
  与读后各一次，少了原来「首尾之间」的那一次——最多 128 KiB，取消延迟没有可测差别。
  `digest.rs` 增加一条单测，比较“流式累计的采样”与“重新读取首尾的采样”一致。

**B3.2 文件、路径与连接候选（全部保留，未提取）**

- **sidecar 清理**：`library::remove_sqlite_sidecars` 只处理 `-wal` / `-shm`，用于数据库
  迁移与重命名，NotFound 静默，错误带完整 sidecar 路径；`backup::remove_database_sidecars`
  处理 `-wal` / `-shm` / `-journal`，只用于备份副本且先判断存在，错误文案是“备份数据库
  临时文件”。文件集合、错误上下文与调用边界都不同，保留两份。
- **不覆盖发布**：`worker::publish_temp` 用调用方标签拼错误；`restore.rs` 的三处
  `persist_noclobber`（输出、delta、检查点）各自带路径上下文；认证侧已有
  `copy_noclobber` / `StagedCopy`。临时文件同步与不覆盖语义一致，但错误类型、标签与
  所有权分散在三个模块，提取只能省一行 `map_err`，不值得新增跨模块 helper。
- **连接闭包**：`storage::open` 有 191 个调用点，连接的可变性、事务边界、慢日志标签与
  闭包返回值各不相同；`with_connection` 只会增加一层闭包间接，不能消除真实分叉，保留
  `open + map_err` 现状。
- **路径唯一化**：`bitmap_io::unique_export_path` 的基名固定为 `<item-id>`，扩展名固定
  `.png`；`canvas_export::unique_target` 先清洗任意文件名再拼 `.png`。`-2` / `-3` 循环
  相同，但输入语义不同，且只有 2 处相似实现，低于提取门槛，保留各自实现。

**B3.3 生产文件拆分（逐候选）**

- `pin_board/repository/create.rs` → `clipboard.rs`：粘贴源解析（复制记录 → 当前步骤的
  条目状态、DDS/文字/笔迹载荷）与整组落位移出；写入事务、revision 推进与 `read_strokes`
  仍在 `create.rs`。消费者只有 `paste_items`，经 `pub(super)` 共享，未扩大 `pub(crate)`
  可见性；粘贴相关测试仍覆盖原入口，断言未改。变化原因：读侧解析与写侧事务分离。
- `pin_board/dds.rs` → `dds/{mod,cache,file}.rs`：进程内 LRU 纹理缓存、预算与淘汰移入
  `cache.rs`；`CreatedFiles` 回滚与 `persist_dds_file` 原子落盘移入 `file.rs`；编解码、
  尺寸/头校验留在 `mod.rs`。外部仍经 `pin_board::dds::*` 访问，`bitmap_io` 与
  `usecases::pin_board` 的调用点不变；缓存与落盘测试随生产代码迁移。
- `storage_overview.rs` → `storage_overview/{mod,disk}.rs`：数据库记录口径、类型与
  `tests.rs` 留在 `mod.rs`；比对计划、目录遍历、分类聚合与 `scan_disk` 移入 `disk.rs`。
  `tests.rs` 只调用公开的 `overview` / `scan_disk`，未搬家；共享的 `normalize_path_key`
  改为 `pub(super)`。变化原因：记录口径与磁盘口径独立演进。
- `authenticity/rendition.rs` → `rendition/{mod,cache,render}.rs`：会话目录、缓存元数据、
  校验与淘汰移入 `cache.rs`；透明背景合成、TrustMark 嵌入、JPEG 编码与缓存写入移入
  `render.rs`；配置归一化、缓存令牌与图片工具留在 `mod.rs`。`preview/tile.rs`、
  `publish.rs` 经原 `rendition::*` 路径访问，公开集合未扩大；缓存淘汰测试随 `cache.rs`
  迁移。
- `cleanup.rs`：**不拆**。队列入队、执行/重放、未引用扫描、删除边界与 temp 清扫共用同一
  事务、引用表与删除确认逻辑；内联测试跨主题直接使用 `enqueue_*`、`run`、`sha256_file`、
  `safe_repository_path` 等内部入口。拆分会要求把跨主题测试整体重排或扩大大量
  `pub(super)`，超过“拆分候选”的收益门槛，保留单文件并登记为不拆。

**B3.4 应用层收口**

- `authenticity/commands.rs` 不再直接调用 `repository::*` / `preview::*` /
  `publication_repository::*`：发布状态、记录搜索、外部图片预览、分支成品预览、局部图与
  记录预览全部经 `app/usecases/authenticity.rs`；适配器只保留 DTO 解析、文件选择器授权与
  错误映射。`external_image_path` 的仓库边界检查随预览进入 use-case。
- `RepositoryScrubReport` 从 `app/workflows.rs` 移到 `app/usecases/backup.rs`，命令适配器
  直接返回 use-case 类型，消除 use-case 对适配器的反向依赖；序列化字段未变。
- 设置快照的仓库派生计数改在 `context.run_read("settings_snapshot")` 内读取；仓库状态
  命令的就绪校验改在 `context.run_read("repository_status")` 内执行，未就绪仍返回
  `ready: false` 与错误文案，命令本身不失败。设置保存仍在仓库切换写租约内直接取快照。
- GUI/headless 成对 `cfg` 变体：实际代码中没有 `cfg(not(feature = "headless"))` 分支；
  仅有的 `cfg(feature = "headless")` 入口（`create_artwork`、`create_strokes`、
  分支创建等）是无头命令的额外入口，复用同一领域实现，不需要合并，也没有引入运行时分派。
- 错误形态与取消通道：`CommandError` 映射、`cancelled_or`、`AuthenticityError::Task` 与
  认证活动任务锁的文案均未改变；没有发现能统一且不改变用户文案的入口，保留现状。

**验证与偏差**

- `cargo fmt --check` 通过；`cargo test --lib` 445 项通过、1 项忽略；`cargo check --lib`
  与 `cargo check --lib --features headless` 通过；未改前端、命令名、DTO 字段、schema 与
  版本号；未提交 Git。
- 偏差：`storage_overview` 拆分新增一处 `pub(super) normalize_path_key`，仅为两个子模块
  共享纯路径规范化；`cleanup.rs` 按上述证据登记为不拆。除这两个记录外，其余候选均完成
  拆分且未扩大 `pub(crate)` 可见性。
- 收尾复核：无取消通道的三个调用点改用 `digest::sha256_reader_without_cancel`，删掉两处
  `unreachable!` 分支（保留在 `digest.rs` 内部一处）；`dds::cache` 的默认预算常量与条目
  类型收紧为私有（未跨模块使用），淘汰循环去掉重复的总量计算；`dds/mod.rs` 与
  `create.rs` 的模块文档补上拆分后的实际分工，并恢复被拆分误删的
  `arrange_gap_ratio_is_valid` 文档注释；`worker.rs` 的指纹注释不再声称常量里带采样宽度。

### B4 `[拆分]` `library` / `history` 仓储拆分与跨域 SQL 登记

#### B4.1 拆分候选

- `library/repository.rs`：仓库发现/初始化/备用库迁移、树 CRUD/搜索、回收站/永久删除。
- `library/schema.rs`：建表与版本常量、迁移、校验。
- `history/repository.rs`：分支 CRUD/head、提交与链、存储元数据/缩略图/compaction。

同样逐候选先画依赖/消费者/测试接缝，通过后再拆；不一次性铺开。

#### B4.2 跨域 SQL 的处理

- 现有 `library` 永久删除与 `history` 重置各自打开连接并创建事务；usecase 公开仓储调用
  接收 root 而非同一个 `Transaction`。在不能明确事务 API、所有权边界与调用方向之前，
  **保留现有单事务实现**。
- 在 schema/模块文档登记：涉及哪些表、为何必须同事务、由哪个操作维护、哪项测试防止部分
  提交。
- 增加回滚测试锁住原子性。只有能明确设计事务传递且不把 SQL 耦合转移到 app 层时，才另立
  重构方案；本计划不把“消除跨域 SQL”设为验收目标。

#### B4 验收

- `cargo test --lib` 全量通过；仓库创建、回收站、永久删除、分支提交、历史删除/精简、
  恢复的手工流程由维护者复验。
- 不改变 schema、迁移顺序、锁语义与命令名；例外有登记，不允许既保留 SQL 又没有说明。
- 若某个仓储拆分后需要扩大可见性或引入事务层级接口才能成立，回退并记录“不拆”。

#### B4 实施记录（2026-11-03）

**拆分（逐候选）**

- 证据：三个文件的消费者都只经各自的 `mod.rs` re-export（`library/mod.rs`、`history/mod.rs`），
  没有外部模块直接导入 `repository::`；测试接缝分别是 `library::repository::test_support::Fixture`
  与 `history::test_support::HistoryFixture` 加真实 schema。三处都至少有 2 个独立变更理由：
  Library 为「发现/初始化」「树语义」「回收站语义」，schema 为「表与版本」「迁移」「校验」，
  History 为「分支设置」「图事务/检查点」「存储元数据与预览」。
- `library/repository.rs` → `library/repository/{mod,discovery,tree,trash}.rs`：数据库定位、初始化
  与备用库改名移入 `discovery.rs`；树读取/搜索/新建/重命名/移动与同级顺序移入 `tree.rs`；
  软删除、恢复、永久删除与清空移入 `trash.rs`。`repository/mod.rs` 只保留模块声明、重导出与
  共享 `open`；`library/mod.rs` 与所有调用方的路径不变。树与回收站共用的
  `load_parent_map` / `ensure_nodes_exist` / `top_level_selection` / `sibling_ids` /
  `update_sibling_positions` / `normalize_siblings` / `validate_node_ids` 收紧为 `pub(super)`，
  未扩大 `pub(crate)`。测试随生产代码迁移到三个子模块，共用夹具放在 `#[cfg(test)]`
  `test_support.rs`。
- `library/schema.rs` → `library/schema/{mod,migrate,validate}.rs`：版本/格式常量、DDL 片段与
  `create` 留在 `mod.rs`；`migrate`、`migrate_v4_to_v5` / `migrate_v5_to_v6` 连同迁移测试移入
  `migrate.rs`；完整性、外键、实体、路径、摘要与轻量版本校验移入 `validate.rs`。迁移 SQL、
  版本号与校验顺序未改。
- `history/repository.rs` → `history/repository/{mod,branches,graph,storage}.rs`：分支列表/
  创建/设置与读取移入 `branches.rs`；节点读取、提交、物化链、检查点与精简移入 `graph.rs`；
  缩略图、snapshot 登记、存储元数据推导与作品目录移入 `storage.rs`。`history/mod.rs` 的
  re-export 集合保持不变，`deletion_repository` / `verification_repository` /
  `schedule_repository` 与 `backup` / `app` 的调用路径不变。测试随所在函数迁移。
- 三个文件都没有引入新的事务层级接口或 `pub(super)` 之外的可见性扩大。

**跨域 SQL 登记与回滚测试**

- 保留现有单事务实现：Library 永久删除/清空与 History 重置各自打开连接并创建事务，usecase
  仍只传 root。表清单、同事务理由、维护操作与防部分提交的测试登记在
  `docs/modules/library.md` 的「回收站」与 `docs/modules/history-and-backup.md` 的「历史操作」。
- 新增 `permanent_deletion_rolls_back_when_a_cross_domain_delete_fails`：在删除 `final_artifacts`
  时注入失败，断言回收站根、分支 `head_history_id`、成品、认证记录与 `pending_file_cleanup`
  全部回滚。
- 新增 `artwork_reset_rolls_back_when_the_cross_domain_timestamp_update_fails`：在更新
  `library_nodes.updated_ms` 时注入失败，断言原分支与历史节点仍在、清理队列为空。
- 没有为「消除跨域 SQL」新增设计；`deletion_repository` 与 `trash` 仍各自拥有同事务实现。

**验证与偏差**

- `cargo fmt --check` 通过；`cargo check --lib` 与 `cargo check --lib --features headless`
  通过；`cargo test --lib` 447 项通过、1 项忽略（原 445 项 + 2 项回滚测试）。
- 未改前端、命令名、DTO 字段、schema、迁移顺序、锁语义与版本号；未提交 Git。
- 可见性：`library/schema` 的 `migrate` / `validate*` 在子模块内标为
  `pub(in crate::library)`——重导出到 `library` 需要比 `pub(super)` 更宽，`pub(crate)`
  又超出需要——`schema/mod.rs` 仍以 `pub(super)` re-export，外部只能经 `library::schema`
  访问；`history/repository/mod.rs` 重新导出全部原入口，未暴露新的 `pub(crate)` 符号。
- 提交前复审又收了几处：`library/repository/trash.rs` 模块注释里的回滚测试名笔误、
  作品树查询中从未读取的 `artworks.description` 列与其 `LEFT JOIN`、`discovery` 里只转发
  `storage::configure` 的包装函数、测试里重复的 `super::super::tree::` 全路径，以及
  `library/model.rs` 中为绕过未使用字段告警而写的临时元组（改为按构建配置放行的
  `cfg_attr`）。文档侧同步了 `schema.rs` / `repository.rs` 拆分后的过期路径。
- B4 的仓储流程（仓库创建、回收站、永久删除、分支提交、历史删除/精简、恢复）仍待维护者
  手工复验，已登记到 `todo.md`。

### B5 `[拆分]` + `[文档]` 素材板 `renderer.ts` 拆分与收尾审计

#### B5.1 前置：状态所有权图与保存契约

- 先画状态所有权：GPU 设备/纹理缓存、画板装载/卸载、保存/结算、笔迹缓存、文字编辑、
  指针状态机、撤销、导出、浮层、快捷键。
- B2 的保存契约测试必须已建立并在本批拆分前后复跑；“切换快照”和“常规保存”两条语义
  保持独立。

#### B5.2 拆分候选（逐个决定）

- `boardSnapshot`、`historyStack`、`textEditController`、`strokeLayer`、`textureCache`、
  `boardIO`、`canvasExportRunner`、`interactionController`。
- 不预设九个模块，也不设 facade 行数上限；只拆能独立测试或独立变化、且不需要传递宽泛
  renderer 状态的边界。

**明确不拆**

- 指针状态机、浮层、文字编辑三者的互斥状态；GPU 设备重建与纹理缓存；`draw +
  refreshViewport + 占位层` 的同帧耦合。

**测试**

- `renderer.test.ts` 只在对应生产边界拆分时迁移；禁止继续用 `Object.create(prototype)`
  手工补齐私有字段测试新模块；保存契约测试在拆分前后都运行。

**手工验收**

- 切画板、切作品、撤销/重做、文字编辑、擦除、整画布导出、退出结算、GPU 设备丢失恢复。

#### B5.3 收尾审计与仓库卫生

- 复核每项只有一个负责批次；最终审计只核对变更记录、未决项与文档完整性，不重复删除或
  重做决策。
- `src-tauri/gen/schemas/`：先列出生成命令、受跟踪文件与 Tauri 配置依据，再决定是否
  忽略或保留；不根据“可能自动生成”直接删除。
- 删除空目录 `src-tauri/examples/`；其余仓库卫生项逐项确认。
- 复核 `README.md`、`CONTRIBUTING.md`、`SECURITY.md` 与 `package.json`、`Cargo.toml` 一致。
- `docs/planning/current-handoff.md` 与 `todo.md` 只保留当前状态与未完成事项；已完成的
  工作项按项目规范移入本归档目录并更新索引。
- `CHANGELOG.md` 按实际行为归类：纯重构不写成“新增功能”；有意行为修正单独说明。

#### B5 验收

- `npm run test:pin-board`、`npx tsc --noEmit`、`cargo test --lib` 通过。
- 画板手工验收通过；文档链接可解析、归档索引无缺失；未决项都有归属。
- 五处版本字段未变；没有新依赖；没有提交。

#### B5 实施记录（2026-11-03）

**B5.1 状态所有权**

拆分前先把渲染器持有的状态按生命周期与变更理由列清；拆分后仍留在 `PinBoardRenderer` 的
状态都有同一原因：它们与指针、视口或保存编排同帧变化。

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| GPU 设备、管线、视口 uniform、设备丢失重建 | `renderer.ts` + `rendererGpu.ts` | 每个渲染器生命周期只创建一次；丢失后限流重建，期间暂停绘制与加载。 |
| 纹理表、预算、在途预留、失败记录、质量上限 | `renderer.ts` | 淘汰顺序移到 `textureEviction.ts`；读取、绘制、导出共用同一组表。 |
| 纹理淘汰顺序 | `textureEviction.ts` | 纯函数：受保护条目过滤后按「不可见 → 有缩略图 → 小图 → 最久未用」排序。 |
| 画板装载/卸载、revision 账本、脏标记、保存与结算 | `renderer.ts` | 与 `pendingSettle` 队列、自动保存排程和状态广播同一条时序。 |
| 不可变条目快照、脏状态串、常规保存载荷 | `boardSnapshot.ts` | 纯数据转换；切换快照的全量文字载荷仍由 `pendingSettle.ts` 自己构造。 |
| 撤销/重做条目、合并窗口与上限 | `historyStack.ts` | 只保存快照对与游标，模型恢复仍由渲染器完成。 |
| 笔迹几何、世界折线缓存、屏幕 GPU 缓冲 | `strokeLayer.ts` | 读取代次、超扫收集框与缓冲签名归这一层；整画布导出的独立缓冲仍在 `rendererExport.ts`。 |
| 文字编辑会话、默认样式、按下判定 | `renderer.ts` | 与指针、选中、编辑框浮层互斥，拆出会传递宽泛可变状态。 |
| 指针状态机、变换手柄、框选 | `renderer.ts` | 计划明确不拆；与浮层、文字编辑同一组互斥状态。 |
| 整画布导出进行态 | `renderer.ts` + `rendererExport.ts` | 导出会暂停交互并复用纹理、文字编辑与笔迹几何，属于渲染器的一种模式。 |
| 选中框/框选/占位层/绘制预览 DOM | `renderer.ts` + `strokePreview.ts` | 与 `draw + refreshViewport` 同帧更新，计划明确不拆。 |
| 保存/退出结算的队列交接 | `pendingSettle.ts` | 已在 B2 拆分，渲染器只调用暂存、取回与 revision 账本。 |

**B5.2 拆分候选（逐个决定）**

- `boardSnapshot`：**拆分**为 `boardSnapshot.ts`。快照、恢复、脏状态串与常规保存载荷都是
  纯数据转换，可直接单测；文字纹理失效由恢复函数返回「签名变化的条目 id」交给调用方。
- `historyStack`：**拆分**为 `historyStack.ts`。撤销条目、游标、合并窗口与上限自成一体，
  用注入时钟即可测试合并边界，不需要渲染器的条目或 GPU。
- `strokeLayer`：**拆分**为 `strokeLayer.ts`。整板几何、世界折线缓存、超扫收集框、
  屏幕顶点/索引/选中描边缓冲与读取代次合并为一个边界；屏幕绘制通过 `batch` 与
  `outlineSlot` 取值，导出缓冲仍由 `rendererExport.ts` 独立构建。
- `textureCache`：**部分拆分**为 `textureEviction.ts`。只把「先淘汰谁」的排序策略抽出并
  直接测试；两张纹理表、预算、在途预留与加载循环仍留在渲染器，因为读取、绘制、导出与
  设备重建都直接读写同一组表，拆出存储层会要求暴露映射表或增加一层访问器。
- `textEditController`：**不拆**。编辑会话与指针按下、选中、编辑框浮层互斥，提交路径同时
  触发快照、历史、脏标记与纹理重建；边界需要传递渲染器的大部分可变状态。
- `boardIO`：**不拆**。保存与结算编排 revision 账本、脏标记、自动保存、状态广播与
  `pendingSettle` 交接，单独拆出只会得到一层转发；现有行为测试已覆盖保存/结算时序。
- `canvasExportRunner`：**不拆**。导出是渲染器的一种模式：暂停交互、复用纹理表与文字编辑、
  构建独立笔迹缓冲、结束后恢复自动保存；没有不传递宽泛状态的独立接缝。
- `interactionController`：**不拆**。计划明确不拆指针、浮层与文字编辑的互斥状态。

**B5.3 收尾审计与仓库卫生**

- `src-tauri/gen/schemas/`：保留并继续跟踪。受跟踪文件为 `acl-manifests.json`、
  `capabilities.json`、`desktop-schema.json`、`windows-schema.json`；`capabilities/default.json`
  的 `$schema` 直接引用 `../gen/schemas/desktop-schema.json`，这些文件由 Tauri CLI 在处理
  capabilities 时的 `tauri dev` / `tauri build` 生成。没有依据「可能自动生成」而忽略或删除。
- 删除空目录 `src-tauri/examples/`（未被 Git 跟踪，删除不影响构建）。
- `README.md`、`CONTRIBUTING.md`、`SECURITY.md` 与 `package.json`、`src-tauri/Cargo.toml`
  核对一致：版本同为 `0.3.0-beta.1`，Node 24 / 稳定 Rust / `npm run tauri -- dev` 与
  `cargo fmt --check --manifest-path src-tauri/Cargo.toml` 等命令仍准确，无需改动。
- 修正 B3 拆分后遗漏的契约测试路径：`texturePolicy.contract.test.ts` 读取
  `pin_board/dds.rs` 改为 `pin_board/dds/mod.rs`。
- `docs/planning/current-handoff.md` 与 `todo.md` 只保留当前状态；B5 的画板手工验收列入
  `todo.md`，本文件追加实施记录，不另建重复的收尾清单。
- `CHANGELOG.md`：在 `Unreleased` 的 Changed 下记一条内部结构变化，明确没有用户可见行为
  变化；不写成新增功能。

**验证与偏差**

- `npx tsc --noEmit` 通过；`npm run test:pin-board` 30 个文件、312 项通过；`npm test` 全量
  62 个文件、528 项通过。
- `cargo fmt --check` 通过；`cargo test --lib` 447 项通过、1 项忽略。
- 只改前端渲染器内部与测试；命令名、DTO 字段、仓库 schema、锁语义与五处版本字段未变；
  没有新增依赖；未提交 Git。
- 偏差：`textureCache` 只拆出淘汰策略，缓存存储仍随渲染器（理由见上）；`strokeLayer`
  拆出后屏幕绘制/命中/擦除都经该层，整画布导出的独立缓冲没有搬入，避免把导出时序带进
  屏幕层。

## 5. 每批次证据与退出条件

每个工作项开始前在批次记录中写清以下五项；缺一项不进入实施：

1. **证据**：实现位置（文件与行）、当前行为/缺陷、可复现条件。
2. **目标**：改后行为，或“行为不变的等价重构”。
3. **边界**：明确不做的相邻项，防止顺手扩大范围。
4. **验证**：自动化检查、回归测试、需要维护者手工确认的场景。
5. **退出条件**：什么结果算完成；失败时回退还是保留并登记。

## 6. 验证矩阵

| 检查 | B1 | B2 | B3 | B4 | B5 |
| --- | --- | --- | --- | --- | --- |
| `npx tsc --noEmit` | 是 | 是 | – | – | 是 |
| 前端受影响测试 | 是 | 全量 | – | – | pin-board + 全量 |
| `cargo fmt --check` | – | – | 是 | 是 | 是 |
| `cargo test --lib` | – | – | 全量 | 全量 | 全量 |
| 维护者手工验收 | – | 菜单 | – | 仓储流程 | 画板 + GUI 流程 |
| 文档/CHANGELOG | 是 | 是 | 是 | 是 | 是 |

完整编译、GUI 流程与大文件测试按项目约定由维护者明确安排；本计划不默认执行压力测试。

## 7. 明确不做

- 不把 `pendingSettle` 与 renderer 的保存载荷合并成同一输出或断言两者请求相等；保留
  “切换快照”与“常规保存”两条契约。
- 不删除 `TOOL_PEN`；它参与生产序列化，不是测试夹具。
- 不在没有事务 API 设计时消除跨域 SQL；保留单事务并登记例外。
- 不整体搬移七个 Rust 测试模块；测试随生产拆分迁移，避免扩大可见性。
- 不预设 renderer 的子模块数量与 facade 行数；不拆强耦合的指针/浮层/文字互斥状态。
- 不让 History 经 `app/api` 调用命令；也不为了少一个包装把领域 API 合并到应用层。
- 不引入通用 `BaseRepository`、通用任务框架、通用弹窗参数表、跨领域事件总线、为单个调用点
  新增的 trait。
- 不简化五层锁序、`contracts`、settings 五拆等已验证设计；没有可复现的死锁、性能或一致性
  证据前不动。
- 不删除 `src-tauri/gen/schemas/`，直到生成依据与受跟踪文件查清。
- 不切版本、不运行压力测试、不执行 GUI 自动化、不提交。

## 8. 完成定义

1. B1–B5 的退出条件全部满足；未实施项有明确理由并归入 `todo.md`。
2. 文档、注释、死代码、契约副本与反向依赖问题按批次处理完毕；没有未记录的“待确认”项。
3. 每项拆分都先有依赖/状态所有权与消费者证据；未拆的候选保留原状并记录“不拆”的理由。
4. 行为修正（右键菜单）有维护者手工验收记录；保存载荷与跨域事务有回归测试。
5. `CHANGELOG.md` 的 `## Unreleased` 按实际行为分类记录；五处版本字段未变。
6. 没有新增通用框架或为单一调用点引入的抽象；没有提交。
