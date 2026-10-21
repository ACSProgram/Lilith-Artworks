# 当前任务交接

更新时间：2026-10-21

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-21 素材板结构收尾（批次二，本次提交）：按
`pin-board-structural-cleanup-plan-2026-10-21.md` 把 `repository.rs` 拆成
`src-tauri/src/pin_board/repository/` 六个子模块（`paths` / `boards` / `view` /
`save` / `create` / `bitmap_io`），公开调用面仍是 `repository::*`。五处建条目流程收敛为
`begin_item_mutation` + `record_initial_state` + `finish_item_mutation`，文字与笔迹载荷
INSERT、单条落位、以及「规范文件名 + 仓库边界」检查各只剩一份实现（规范名判定收敛为
`paths::is_item_dds_file`，相对路径拼装收敛为 `paths::relative_item_dds`）；修订冲突文案、
redo 作废、`FULL_UV` 与默认图层抽成共享常量/helper。`imported_points` 移到 `arrange.rs`，条目与
导入上限移到 `contracts.rs`（该文件补齐模块文档）；持久化测试按归属分到各子模块，迁移测试移入
`library/schema.rs`，无头导入的条目数输出与 `stress_bench` 消费者同步改为 `itemCount`。
行为只有一处防御性收紧：永久删除画板现在要求该画板已在回收站（此前按 id 直接硬删除，活画板也会
连记录和 DDS 一起消失且无从撤销），界面观感无变化。自动验证已通过（`cargo fmt --check`、
`cargo check --all-targets`、`cargo check --features headless --all-targets`、
`cargo test --lib` 414 用例、`git diff --check`）。批次三仍按计划文件执行。

2026-10-21 素材板结构收尾（批次一，本次提交）：按
`pin-board-structural-cleanup-plan-2026-10-21.md` 落实批次一。两处正确性修复：纹理结果缓存键加入
仓库根路径（切仓库后画板/条目 id 从头复用不再串图；键同时从四元组改为具名字段结构、根路径按
`PathBuf` 原样比较），`validate_editable` 按条目 kind 收紧图层契约（位图与文字的 layer 3 被拒，
笔迹仍由 `effective_layer` 强制标注层）。卫生清理：删除 `session.ts` 的 `dropArtworkSessions`
及其测试（并在模块注释里说明会话不做淘汰的理由）、`strokeGeometry.ts` 的 `STROKE_LAYER` 与
`MAX_STROKE_WIDTH`（与 `strokeDrawing.ts` 的 500 同名不同义）、renderer 的两层纹理别名，清理
`stroke.rs` 的「阶段 2」注释与不再需要的 `allow(dead_code)`；`imageCount` 在 `PinBoardSummary`、
`PinBoardInteractionState` 与原生 `item_count` 一并改名为 `itemCount`；同步过期注释与模块/架构
文档（补充缓存键构成、位图/文字占标注层被拒的契约与测试覆盖），并清掉本文件与 `todo.md` 中
已归档素材板批次的表层段落。自动验证已通过（`cargo fmt --check`、`cargo test --lib pin_board`
88 用例、`npm run test:pin-board` 24 文件 / 256 用例、`npx tsc --noEmit`、`git diff --check`）。
批次三仍按计划文件执行。

2026-10-21 素材板图层开关与锁定解耦（本次提交）：图层面板此前跟着按钮组共用的
`editingBlocked`（锁定或导出）走，锁定状态下四个开关一并置灰——那是整画布导出引入统一禁用
条件时把 `locked` 顺带并进来的结果，计划与模块文档都只要求「导出期间禁用图层面板」。现改为
独立的 `pinBoardLayersPanelDisabled`，只检查渲染器是否可用与是否正在导出：图层显隐是纯视图
状态（不写库、不标 dirty、不进撤销栈），锁定挡的是改模型的操作，锁着藏掉标注层看底图正是
锁定状态下的常用动作。行为变化见 `CHANGELOG.md` 的 `## Unreleased`，模块口径见
`docs/modules/pin-board.md` 的「图层可见性」。自动验证已通过（`tsc --noEmit`、
`npm run test:pin-board` 24 文件 / 257 用例），**锁定态下开关的手感与导出期禁用仍待人工确认**
（验收项在 `todo.md`）。

2026-10-21 错误边界与上下文拆分（已提交 `0c2f5d0`）：渲染期异常不再卸载整棵 React 树——根边界包在
`NoticeCenter` / `AppRuntimeProvider` **之外**（三个 context hook 在 provider 缺失时都抛错，边界
放在里面就兜不住），兜底自带可拖拽顶条与窗口按钮；作品工作区、三个标签页共用的视图层与素材板
窗格各有一层区块边界，切换作品或标签页时按 `resetKeys` 自动复挂，不必点重试。捕获后写
`[boundary]` 日志（消息 + 出错组件 + 堆栈），日志标签表已登记。同批把三个 context 从 Provider
文件拆到 `appRuntimeContext.ts` / `noticeContext.ts` / `runtimeProgressContext.ts`：context 与
Provider 同文件时，开发模式保存 Provider 会热更新出新的 context 对象，仍持有旧 `useContext` 的
消费者读到 `null` 抛错并带走整棵树（2026-10-21 素材板渲染器被卸载即由此触发）。窗口按钮抽成
共用的 `WindowControls.tsx`，标题栏与根兜底不再各写一份标记。行为变化见 `CHANGELOG.md` 的
`## Unreleased`，架构口径见 `docs/architecture/overview.md` 的「错误边界」与「上下文模块」。
自动验证已通过（`tsc --noEmit`、`vitest run src/app`），**兜底界面的观感、真实崩溃恢复与
热更新不再卸载界面仍待人工确认**（验收项在 `todo.md`）。

2026-10-21 本地 CI 预检（本批次）：新增 `npm run ci:local`（`tools/ci/local-ci.mjs`），
只覆盖日常检查之外的 CI 门槛——workflow 审计列表一致性、平台分支检查、两条 npm 审计、
RustSec 审计、发布元数据、许可证一致性、headless 测试目标预编译与九组便宜压力测试；
独立门槛并行启动、失败后继续跑完再汇总，日志只写 `src-tauri/target/ci/`。同批重新生成
许可证清单，修复 webpki 升级后 `validate` 在「许可证一致性」处失败的问题。无 GUI 验收项；
口径见 `docs/guides/validation.md` 的「本地 CI 预检」。

2026-10-21 测试体检落地批（第一批已提交 `6f7e492`，第二批为本次提交；C9/C10 待发布前实跑）：
`docs/planning/test-audit-2026-10-21.md` 的处理项分两批完成。第一批删除一次性的
`stress_baseline` 组与过时调试档基线；修严格性能门（每笔耗时「只记录不比对」、吞吐量改「越大
越好」）、C7 第 6 检查点（改由 `COMMIT_CHECKPOINTS` 驱动）、J5 文案对齐；收紧 D1/K2/J2/G2 与
前端 3.8/3.9 弱断言；新增 R7（损坏快照+内容变化再提交）、C9/C10（大文件并行强杀、4 MiB 边界）、
手动提交清零计数单测、`usePendingSettle` hook 测试与长笔迹逐段裁剪测试。第二批收紧 H3 内存上限
（512 → 64 MiB，debug 实测 12.5 MiB）、删除高频提交 180 秒护栏、补原生整理命令参数边界单测、
把门控守护扩到 `tests/` 顶层全部 `.rs`（按是否会启动子进程判定）、在 C9 中补残留临时文件的
「扫描 → 确认清理 → 再扫描为空」闭环，并把 C3/快照上界改口径为「灾难性上界」。维护者已决定
关闭 3.9（工具栏显隐归人工验收）、暂不做 4.6。C9/C10（含 4.8 新断言）仅验证过测试可编译
（`cargo check --tests`），未实际运行——这符合大文件组的既有口径，不是压力测试结果。
逐项状态见体检报告顶部的「处理状态」注记；行为变化见 `CHANGELOG.md` 的 `## Unreleased`。
剩余项（3.7 实际收紧、C9/C10 与 4.8 实跑、4.6 暂缓）在 `todo.md`。

2026-10-21 版本切分 `0.3.0-alpha.5`（未打标签）：`CHANGELOG.md` 的 `## Unreleased` 落为带
日期的 `## 0.3.0-alpha.5 - 2026-10-21` 小节并按「CHANGELOG 写法」再压缩一遍，顶部重留空的
`## Unreleased`；五处版本字段、许可清单（`npm run legal`）与 `tools/release/verify-metadata.mjs`
的版本→schema 映射同步为 `0.3.0-alpha.5 → repository schema v6`（登记表在 schema 升 v6 时
已预登记，本次起 `verify-metadata` 恢复一致）；README 与 `docs/planning/` 的版本引用一并
更新。该版本收拢 alpha4 之后的全部改动——素材板笔迹与图层、备份检查元数据接线、快速命中
计数与自适应强制全量、手动重查分支按链校验。**切版本不等于发布**：不触发任何门槛，是否打
`v0.3.0-alpha.5` 标签由维护者单独决定。

上一批（自动备份调度与快速检查修复，阶段 1–5，已提交）：快速检查的指纹门槛、调度侧
静默与代价感知、让位改事件驱动、后台完成事件收敛、提交路径删除 head snapshot 整份复检、
结构单元改缓冲顺序读，以及提交与恢复导出的三段式有界管道全部落地。计划、release 实测与实施
记录见 `archive/backup-scheduling-fix-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过，**GUI 手工验收、一天日志观察与 release 下
同一 1.378 GB 分支的提交/恢复复测仍待人工**（验收项在 `todo.md`）。

上一批（平台代码集中，已提交）：平台专属逻辑收口到 `src-tauri/src/platform/`，新增可移植性规范
`docs/guides/platform.md`，Windows CI 增加「平台分支只准出现在 `platform/` 内」的 grep 把关。
计划与实施记录见 `archive/platform-code-consolidation-plan-2026-10-20.md`；行为零变化（纯机械
搬移 + 一轮不改变行为的整理），自动验证已通过，维护者已做初步手工确认但**未覆盖全部界面入口**，
剩余 GUI 验收项在 `todo.md`。

上一批（缩略图格式扩展第一批，已提交）：节点缩略图新增三种来源——`.sai2`（libsai 移植，
`intg` 画布优先、`thum` 兜底）、`.procreate`（ZIP 内的 `QuickLook/Thumbnail.png`）与无魔数的
`.tga`，`supports_preview` 同步登记；其余候选格式由维护者决定暂缓。计划、格式依据与实施记录
见 `archive/thumbnail-format-expansion-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过（含 `intg` 在 3 个真实样本上与 SAI 导出 PNG
的逐像素比对），**GUI 手工验收与 `.procreate` 真实样本验证仍待人工**（验收项在 `todo.md`）。

上一批（校验可见性与「无法检查」状态，已提交）：历史节点的问题从「分支状态行一行小字」
改为节点级可见（红=文件丢失、黄=文件损坏、灰=无法检查），失败归因修为「真正坏掉的节点标
丢失/损坏，被挡住的更早节点写无法检查」，全库扫描改为按链走。alpha3 阶段 9 的 GUI 手工验收
已由维护者实测通过（见 `archive/handoff-log-2026-10-20.md` 第 12 节）；**本批次改版后的界面
观感与真实损坏场景仍待人工确认**（验收项在 `todo.md`）。

上一批（切换作品不再等待仓库写入，已提交）：`loadBoard` 改为同步交接，旧画板的未保存改动以快照
形式交给模块级后台结算队列（`pin-board/pendingSettle.ts`）串行写回，后台任务持写锁再久也不影响
切换；没有未保存改动时不产生任何写操作。队列对可重试失败按 0.5 s → 15 s 退避静默重试，不可重试
失败保留快照并在素材板里常驻提示 + 重试；退出与切仓库在 5 秒预算内清空队列。行为变化见
`CHANGELOG.md` 的 `## 0.3.0-alpha.4 - 2026-10-20`，模块口径见 `docs/modules/pin-board.md`。
自动验证已通过，**界面与退出行为仍待人工确认**（验收项在 `todo.md`）。

上一批（拖放暂存清理，已提交）：拖放导入的暂存目录改为按需创建、退出删除、崩溃残留下次启动
清扫，不再每次运行在系统临时目录里留一个空文件夹。模块口径见 `docs/modules/authenticity.md`。
自动验证已通过，**退出与强杀行为仍待人工确认**（验收项在 `todo.md`）。

## 下一步

- 错误边界的界面验收：根兜底顶条与窗口按钮、区块兜底的作用范围（含素材板窗格内的位置）、
  热更新不再整棵卸载，以及 `[boundary]` 日志能否按标签筛到。验收项见 `todo.md`。
- 测试体检第二批的收尾：随发布前压力测试轮运行 C9/C10（含 4.8 新增的临时文件清理闭环），并用
  同一轮发布档 C3 实测收紧 3.7 的内存与快照上界。验收项见 `todo.md`。
- 自动备份调度修复的阶段 2–5 待人工验证：GUI 手工验收（静默窗、调度器代价感知、让位不空转、
  后台完成事件只合并分支）、一天日志观察，以及 release 下同一 1.378 GB 分支的提交/恢复复测
  （与计划 §6.1 的分段耗时逐项对照）。验收项见 `todo.md`。
- 平台批次剩余的 GUI 手工验收（「在文件夹中显示」、设置页三个目录入口、素材板粘贴导入），
  验收项见 `todo.md`。
- 缩略图格式扩展的 GUI 手工验收：选中 `.sai2` / `.procreate` / `.tga` 分支的节点，逐档确认
  图片、「无可用预览」占位与「格式不支持」三种表现；`.procreate` 的真实文件仍只有合成 ZIP
  单测。验收项见 `todo.md`。
- 校验可见性改版的界面确认：故障节点在总览、当前分支与时间轴三处按色呈现，状态行与默认展开的
  「详情信息」读起来不费解；再用真实仓库删掉某个 head 的 snapshot，确认归因只落在坏节点上。
  验收项见 `todo.md`。
- 切换作品的后台结算确认：长任务运行期间切换作品仍即时完成；未保存改动切换不弹错、由后台写回；
  仓库只读时出现常驻提示并可重试；退出时后台队列被清空。验收项见 `todo.md`。
- 拖放暂存清理的退出确认：退出后 `%TEMP%` 不留本次运行的暂存目录，强杀残留下次启动被清掉，
  拖放导入流程不受影响。验收项见 `todo.md`。
- 阶段 9 剩余的优先级确认（待查集合按 `created_ms` 最旧优先、失败节点退出队列、有到期备份时的
  aging 插队），验收项见 `todo.md`。
- 垃圾文件扫描与检查点取消的界面验收、alpha2 遗留验收（同仓库写入排队）见 `todo.md`。
- 版本已切至 `0.3.0-alpha.5`（未打标签，见「当前批次状态」）；0.3 发布前仍由维护者显式安排
  压力测试（含 `manual` 8 GiB 档，见 `docs/guides/release-policy.md`），并把实测写入
  `docs/user/stress-test-report.md`；是否打标签按同一文件单独决定。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.5`（未打标签的过渡版本，收拢 alpha4 之后的素材板笔迹与调度
  接线批次），五处版本字段、`CHANGELOG.md` 与逐版本 schema 映射同步；日常改动写入
  `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按 `docs/guides/release-policy.md`
  单独决定。
- repository schema v6（v4 → v5 随 alpha3 阶段 7 落地，v5 → v6 随 alpha5 阶段 1 落地）；ChunkFile 仓库
  可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 alpha1 与 alpha2 保持 v4；**0.3 共有
  两次迁移**（alpha3 的 v4 → v5 与 alpha5 阶段 1 的 v5 → v6），高于当前版本的仓库由旧程序直接
  拒绝。
- 0.3 基线（阶段 0 一次性采集）：临时仓库副本 `D:\temp\Artworks`（3 作品 / 59 历史节点 /
  ≈9.83 GiB），采集后已按采集前状态还原。`stress_baseline` 组已随 2026-10-21 的测试体检退役
  删除（历史数值见 `0.3-baseline-2026-10-18.md`），长期回归对照用 `stress_bench`。
- 运行要求：Windows + WebView2；素材板另需支持 WebGPU 与 BC7 纹理压缩的图形适配器（见
  `README.md` 的「运行环境」）。
- 平台层：`src-tauri/src/platform/` 是产品代码里平台分支的唯一入口（路径表示、文件管理器、
  剪贴板、进程信息），业务文件不写 `cfg` 分支，由 Windows CI 的 grep 检查把关；规则、例外登记表
  与「加平台清单」见 `docs/guides/platform.md`。这不等于已经支持 macOS。
- 项目定位：平面美术个人项目的资源、版本管理与发布工具。领域模块为 Library（作品树）、
  History/Backup（分支与增量历史）、Authenticity（成品与 C2PA/TrustMark）、Pin-board（素材板）。

## 文档约束

- 当前功能契约以 `docs/architecture/`、`docs/modules/` 与 `docs/guides/` 为准。
- **表层文档只保留当前状况、未完成事项与规划**：本文件与 `todo.md` 不累积历史——已完成批次、
  已通过的人工验收、被替代的计划与其他历史增量一律移入 `docs/planning/archive/`，并同步更新
  `docs/planning/archive/README.md` 的索引。
- 未完成事项只写入 `docs/planning/todo.md`；某项通过并归档后，同时从 `todo.md` 的待办/待验收
  段落与本文件中删除，不在表层留下“已通过”的痕迹。
- 历史增量的详细记录归 `archive/` 下对应日期的日志；版本、标签、人工门槛与产物口径归
  `docs/guides/release-policy.md`。
