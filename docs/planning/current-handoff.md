# 当前任务交接

更新时间：2026-10-22

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-22 0.3 收尾与认证整改第一批（已提交）：设置页「备份」页不再依赖先打开「空间」页——
最近一次整仓备份在设置弹窗打开且仓库就绪时即从数据库读取，标题改为「最近一次结果」、空态改为
「尚无备份记录」，仓库切换仍清空旧结果；新增备份读取与仓库未就绪两条前端用例。同批删除无调用
的引用清单校验接口（`validate_target` / `ValidationOutcome` / `ManagedFile.deletable`）与后台
运行入口（`run_background` / `OperationClass::Background`），移除无消费者的 `default_panel`
设置（旧设置文件含 `defaultPanel` 仍可加载），并把只被测试使用的非 typed 包装改为
`#[cfg(test)]`；同步修正 `library.md` 的 schema v6 迁移链、`cleanup` / `storage_overview` /
命令底座文档的过期阶段描述与锁序。计划与验证记录见
`0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md` 第 13 节。自动验证已通过
（`cargo fmt --check`、`cargo check --lib`、`cargo check --tests`、`cargo test --lib`、
`tsc --noEmit`、`npm test` 441 条），**重启后直接打开备份页、仓库切换清空结果仍待人工确认**。
第二至第六批待执行。

2026-10-21 错误边界与上下文拆分（已提交 `0c2f5d0`）：渲染期异常不再卸载整棵 React 树——根边界包在
`NoticeCenter` / `AppRuntimeProvider` **之外**（三个 context hook 在 provider 缺失时都抛错，边界
放在里面就兜不住），兜底自带可拖拽顶条与窗口按钮；作品工作区、三个标签页共用的视图层与素材板
窗格各有一层区块边界，切换作品或标签页时按 `resetKeys` 自动复挂，不必点重试。捕获后写
`[boundary]` 日志（消息 + 出错组件 + 堆栈），日志标签表已登记。同批把三个 context 从 Provider
文件拆到 `appRuntimeContext.ts` / `noticeContext.ts` / `runtimeProgressContext.ts`：context 与
Provider 同文件时，开发模式保存 Provider 会热更新出新的 context 对象，仍持有旧 `useContext` 的
消费者读到 `null` 抛错并带走整棵树（2026-10-21 素材板渲染器被卸载即由此触发）。窗口按钮抽成
共用的 `WindowControls.tsx`，标题栏与根兜底不再各写一份标记。行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.6 - 2026-10-21`，架构口径见 `docs/architecture/overview.md` 的「错误边界」与
「上下文模块」。
自动验证已通过（`tsc --noEmit`、`vitest run src/app`），**兜底界面的观感、真实崩溃恢复与
热更新不再卸载界面仍待人工确认**（验收项在 `todo.md`）。

2026-10-21 本地 CI 预检（已提交 `d58dee7`）：新增 `npm run ci:local`（`tools/ci/local-ci.mjs`），
只覆盖日常检查之外的 CI 门槛——workflow 审计列表一致性、平台分支检查、两条 npm 审计、
RustSec 审计、发布元数据、许可证一致性、headless 测试目标预编译与九组便宜压力测试；
独立门槛并行启动、失败后继续跑完再汇总，日志只写 `src-tauri/target/ci/`。同批重新生成
许可证清单，修复 webpki 升级后 `validate` 在「许可证一致性」处失败的问题。无 GUI 验收项；
口径见 `docs/guides/validation.md` 的「本地 CI 预检」。

2026-10-21 测试体检落地批（第一批已提交 `6f7e492`，第二批已提交 `b11781b`；C9/C10 待发布前
实跑）：`docs/planning/test-audit-2026-10-21.md` 的处理项分两批完成。第一批删除一次性的
`stress_baseline` 组与过时调试档基线；修严格性能门（每笔耗时「只记录不比对」、吞吐量改「越大
越好」）、C7 第 6 检查点（改由 `COMMIT_CHECKPOINTS` 驱动）、J5 文案对齐；收紧 D1/K2/J2/G2 与
前端 3.8/3.9 弱断言；新增 R7（损坏快照+内容变化再提交）、C9/C10（大文件并行强杀、4 MiB 边界）、
手动提交清零计数单测、`usePendingSettle` hook 测试与长笔迹逐段裁剪测试。第二批收紧 H3 内存上限
（512 → 64 MiB，debug 实测 12.5 MiB）、删除高频提交 180 秒护栏、补原生整理命令参数边界单测、
把门控守护扩到 `tests/` 顶层全部 `.rs`（按是否会启动子进程判定）、在 C9 中补残留临时文件的
「扫描 → 确认清理 → 再扫描为空」闭环，并把 C3/快照上界改口径为「灾难性上界」。维护者已决定
关闭 3.9（工具栏显隐归人工验收）、暂不做 4.6。C9/C10（含 4.8 新断言）仅验证过测试可编译
（`cargo check --tests`），未实际运行——这符合大文件组的既有口径，不是压力测试结果。
逐项状态见体检报告顶部的「处理状态」注记；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.6 - 2026-10-21`。
剩余项（3.7 实际收紧、C9/C10 与 4.8 实跑、4.6 暂缓）在 `todo.md`。

2026-10-21 版本切分 `0.3.0-alpha.6`（未打标签）：`CHANGELOG.md` 的 `## Unreleased` 落为
带日期的 `## 0.3.0-alpha.6 - 2026-10-21` 小节，并按「CHANGELOG 写法」压缩一遍（合并在画板内
复制与粘贴落点两条、删去整画布导出与错误边界的实现细节、把两处结构拆分收成一两句、测试条目改为
行为描述），顶部重留空的 `## Unreleased`；五处版本字段、许可清单（`npm run legal`）与
`tools/release/verify-metadata.mjs` 的版本→schema 映射同步为 `0.3.0-alpha.6 → repository
schema v6`；README 与 `docs/planning/` 的版本引用一并更新。该版本收拢 alpha5 之后的全部改动——
素材板橡皮擦与墨迹命中、图层计数、三类粘贴、整画布导出与可取消传输、结构收尾的契约修复
（纹理缓存键、图层 kind、回收站删除前置）、持久化与前端结构拆分、分层错误边界与 context
拆分、分支备份控件改版、本地 CI 预检、2026-10-21 测试体检两批与两次依赖升级。本地 CI 预检
（`npm run ci:local`）已通过：workflow 审计、平台分支、两条 npm 审计、RustSec、发布元数据、
许可证一致性、headless 目标预编译与九组便宜压力测试全 PASS（C9/C10 仍待发布前轮次）。
**切版本不等于发布**：不触发任何门槛，是否打 `v0.3.0-alpha.6` 标签由维护者单独决定。

上一批（版本切分 `0.3.0-alpha.5`，已提交 `3974bcc`，未打标签）：`CHANGELOG.md` 的 `## Unreleased`
落为带日期的 `## 0.3.0-alpha.5 - 2026-10-21` 小节并按「CHANGELOG 写法」再压缩一遍，顶部重留
空的 `## Unreleased`；五处版本字段、许可清单（`npm run legal`）与
`tools/release/verify-metadata.mjs` 的版本→schema 映射同步为 `0.3.0-alpha.5 → repository
schema v6`；README 与 `docs/planning/` 的版本引用一并更新。该版本收拢 alpha4 之后的全部改动——
素材板笔迹与图层、备份检查元数据接线、快速命中计数与自适应强制全量、手动重查分支按链校验。

上一批（自动备份调度与快速检查修复，阶段 1–5，已提交）：快速检查的指纹门槛、调度侧
静默与代价感知、让位改事件驱动、后台完成事件收敛、提交路径删除 head snapshot 整份复检、
结构单元改缓冲顺序读，以及提交与恢复导出的三段式有界管道全部落地。计划、release 实测与实施
记录见 `archive/backup-scheduling-fix-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过，**GUI 手工验收、一天日志观察与 release 下
同一 1.378 GB 分支的提交/恢复复测仍待人工**（验收项在 `todo.md`）。

上一批（缩略图格式扩展第一批，已提交）：节点缩略图新增三种来源——`.sai2`（libsai 移植，
`intg` 画布优先、`thum` 兜底）、`.procreate`（ZIP 内的 `QuickLook/Thumbnail.png`）与无魔数的
`.tga`，`supports_preview` 同步登记；其余候选格式由维护者决定暂缓。计划、格式依据与实施记录
见 `archive/thumbnail-format-expansion-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过（含 `intg` 在 3 个真实样本上与 SAI 导出 PNG
的逐像素比对），**GUI 手工验收与 `.procreate` 真实样本验证仍待人工**（验收项在 `todo.md`）。

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

- 0.3 收尾与认证整改第一批的人工确认：重启应用后直接打开「备份」页应显示数据库中最新一条
  备份记录，从未备份过时显示「尚无备份记录」；切换仓库后旧仓库结果被清空。验收项见 `todo.md`。
- 第一批已提交，下一步继续第二批（后端算法与 history 仓储拆分）。
- 错误边界的界面验收：根兜底顶条与窗口按钮、区块兜底的作用范围（含素材板窗格内的位置）、
  热更新不再整棵卸载，以及 `[boundary]` 日志能否按标签筛到。验收项见 `todo.md`。
- 测试体检第二批的收尾：随发布前压力测试轮运行 C9/C10（含 4.8 新增的临时文件清理闭环），并用
  同一轮发布档 C3 实测收紧 3.7 的内存与快照上界。验收项见 `todo.md`。
- 自动备份调度修复的阶段 2–5 待人工验证：GUI 手工验收（静默窗、调度器代价感知、让位不空转、
  后台完成事件只合并分支）、一天日志观察，以及 release 下同一 1.378 GB 分支的提交/恢复复测
  （与计划 §6.1 的分段耗时逐项对照）。验收项见 `todo.md`。
- 缩略图格式扩展的 GUI 手工验收：选中 `.sai2` / `.procreate` / `.tga` 分支的节点，逐档确认
  图片、「无可用预览」占位与「格式不支持」三种表现；`.procreate` 的真实文件仍只有合成 ZIP
  单测。验收项见 `todo.md`。
- 切换作品的后台结算确认：长任务运行期间切换作品仍即时完成；未保存改动切换不弹错、由后台写回；
  仓库只读时出现常驻提示并可重试；退出时后台队列被清空。验收项见 `todo.md`。
- 拖放暂存清理的退出确认：退出后 `%TEMP%` 不留本次运行的暂存目录，强杀残留下次启动被清掉。
  验收项见 `todo.md`。
- 检查点取消的界面验收、alpha2 遗留验收（同仓库写入排队）见 `todo.md`。
- 版本已切至 `0.3.0-alpha.6`（未打标签，见「当前批次状态」）；0.3 发布前仍由维护者显式
  安排压力测试（含 `manual` 8 GiB 档，见 `docs/guides/release-policy.md`），并把实测写入
  `docs/user/stress-test-report.md`；是否打标签按同一文件单独决定。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.6`（未打标签的过渡版本，收拢 alpha5 之后的素材板复制/
  橡皮擦/导出、结构拆分、错误边界与备份控件改版批次），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 阶段 7 落地，v5 → v6 随 alpha5 阶段 1 落地；alpha6
  沿用 v6）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 alpha1 与
  alpha2 保持 v4；**0.3 共有两次迁移**（alpha3 的 v4 → v5 与 alpha5 阶段 1 的 v5 → v6），高于
  当前版本的仓库由旧程序直接拒绝。
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
