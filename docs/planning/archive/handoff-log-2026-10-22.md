# 2026-10-22 批次记录归档

本文件收 2026-10-22 从 `../current-handoff.md` 清出的已完成、已提交批次摘要。当前批次状态
改看 `../current-handoff.md`，未完成事项与未执行的人工验收看 `../todo.md`。

0.3 收尾与认证整改（2026-10-22）第一至第七批的详细实施记录见同目录的
`0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md` 第 14 节（该计划已随第七批的收尾
整理归档）；第四批的摘要已随该批提交移入本文件（见下），第一批的人工验收通过记录见文末。

## 0.3 收尾与认证整改第四批（已提交）

发布质量预览的高清局部图请求移回 `usePublicationController.previewOutputTile`：`compressed`
绑定本次预览的缓存 token，`original` 绑定当前分支，缺上下文时明确拒绝；对话框只保留请求
代次、缓存与去抖。局部图来源收敛为 `OutputPreviewTileSource`，去抖与缓存上限改为具名常量，
`loupe.ts` 的 `lru` 改名 `insertionOrder` 以匹配「按插入顺序淘汰、命中不重排」。同批删除无
引用的 `.scope-reset`，估算返回值统一使用 `FileSizeEstimate`，并修正认证文档中的预览缓存
位置、放大镜淘汰策略与记录搜索字段。无新增界面验收项；详细记录见活动计划第 14 节。

## 错误边界与上下文拆分（已提交 `0c2f5d0`）

2026-10-21 批次：渲染期异常不再卸载整棵 React 树——根边界包在 `NoticeCenter` /
`AppRuntimeProvider` **之外**（三个 context hook 在 provider 缺失时都抛错，边界放在里面就兜不住），
兜底自带可拖拽顶条与窗口按钮；作品工作区、三个标签页共用的视图层与素材板窗格各有一层区块边界，
切换作品或标签页时按 `resetKeys` 自动复挂，不必点重试。捕获后写 `[boundary]` 日志（消息 + 出错
组件 + 堆栈），日志标签表已登记。同批把三个 context 从 Provider 文件拆到
`appRuntimeContext.ts` / `noticeContext.ts` / `runtimeProgressContext.ts`：context 与 Provider 同文件
时，开发模式保存 Provider 会热更新出新的 context 对象，仍持有旧 `useContext` 的消费者读到 `null`
抛错并带走整棵树（2026-10-21 素材板渲染器被卸载即由此触发）。窗口按钮抽成共用的
`WindowControls.tsx`，标题栏与根兜底不再各写一份标记。行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.6 - 2026-10-21`，架构口径见 `docs/architecture/overview.md` 的「错误边界」与
「上下文模块」。自动验证已通过（`tsc --noEmit`、`vitest run src/app`）；界面观感、真实崩溃恢复与
热更新不再卸载界面仍待人工确认，验收项在 `../todo.md`。

## 本地 CI 预检（已提交 `d58dee7`）

2026-10-21 批次：新增 `npm run ci:local`（`tools/ci/local-ci.mjs`），只覆盖日常检查之外的 CI
门槛——workflow 审计列表一致性、平台分支检查、两条 npm 审计、RustSec 审计、发布元数据、许可证
一致性、headless 测试目标预编译与九组便宜压力测试；独立门槛并行启动、失败后继续跑完再汇总，日志
只写 `src-tauri/target/ci/`。同批重新生成许可证清单，修复 webpki 升级后 `validate` 在「许可证
一致性」处失败的问题。无 GUI 验收项；口径见 `docs/guides/validation.md` 的「本地 CI 预检」。

## 测试体检落地批（已提交 `6f7e492`、`b11781b`）

2026-10-21 批次：`docs/planning/test-audit-2026-10-21.md` 的处理项分两批完成。第一批删除一次性
的 `stress_baseline` 组与过时调试档基线；修严格性能门（每笔耗时「只记录不比对」、吞吐量改「越大
越好」）、C7 第 6 检查点（改由 `COMMIT_CHECKPOINTS` 驱动）、J5 文案对齐；收紧 D1/K2/J2/G2 与
前端 3.8/3.9 弱断言；新增 R7（损坏快照+内容变化再提交）、C9/C10（大文件并行强杀、4 MiB 边界）、
手动提交清零计数单测、`usePendingSettle` hook 测试与长笔迹逐段裁剪测试。第二批收紧 H3 内存上限
（512 → 64 MiB，debug 实测 12.5 MiB）、删除高频提交 180 秒护栏、补原生整理命令参数边界单测、
把门控守护扩到 `tests/` 顶层全部 `.rs`（按是否会启动子进程判定）、在 C9 中补残留临时文件的
「扫描 → 确认清理 → 再扫描为空」闭环，并把 C3/快照上界改口径为「灾难性上界」。维护者已决定
关闭 3.9（工具栏显隐归人工验收）、暂不做 4.6。C9/C10（含 4.8 新断言）仅验证过测试可编译
（`cargo check --tests`），未实际运行——这符合大文件组的既有口径，不是压力测试结果。
逐项状态见体检报告顶部的「处理状态」注记；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.6 - 2026-10-21`。剩余项（3.7 实际收紧、C9/C10 与 4.8 实跑、4.6 暂缓）在
`../todo.md`。

## 版本切分 `0.3.0-alpha.6`（已提交，未打标签）

2026-10-21：`CHANGELOG.md` 的 `## Unreleased` 落为带日期的 `## 0.3.0-alpha.6 - 2026-10-21`
小节，并按「CHANGELOG 写法」压缩一遍（合并在画板内复制与粘贴落点两条、删去整画布导出与错误边界
的实现细节、把两处结构拆分收成一两句、测试条目改为行为描述），顶部重留空的 `## Unreleased`；
五处版本字段、许可清单（`npm run legal`）与 `tools/release/verify-metadata.mjs` 的版本→schema
映射同步为 `0.3.0-alpha.6 → repository schema v6`；README 与 `docs/planning/` 的版本引用一并
更新。该版本收拢 alpha5 之后的全部改动——素材板橡皮擦与墨迹命中、图层计数、三类粘贴、整画布导出
与可取消传输、结构收尾的契约修复（纹理缓存键、图层 kind、回收站删除前置）、持久化与前端结构拆分、
分层错误边界与 context 拆分、分支备份控件改版、本地 CI 预检、2026-10-21 测试体检两批与两次依赖
升级。本地 CI 预检（`npm run ci:local`）已通过：workflow 审计、平台分支、两条 npm 审计、RustSec、
发布元数据、许可证一致性、headless 目标预编译与九组便宜压力测试全 PASS（C9/C10 仍待发布前轮次）。
**切版本不等于发布**：不触发任何门槛，是否打 `v0.3.0-alpha.6` 标签由维护者单独决定。

## 版本切分 `0.3.0-alpha.5`（已提交 `3974bcc`，未打标签）

2026-10-21：`CHANGELOG.md` 的 `## Unreleased` 落为带日期的 `## 0.3.0-alpha.5 - 2026-10-21`
小节并按「CHANGELOG 写法」再压缩一遍，顶部重留空的 `## Unreleased`；五处版本字段、许可清单
（`npm run legal`）与 `tools/release/verify-metadata.mjs` 的版本→schema 映射同步为
`0.3.0-alpha.5 → repository schema v6`；README 与 `docs/planning/` 的版本引用一并更新。该版本
收拢 alpha4 之后的全部改动——素材板笔迹与图层、备份检查元数据接线、快速命中计数与自适应强制全量、
手动重查分支按链校验。

## 自动备份调度与快速检查修复（已提交）

快速检查的指纹门槛、调度侧静默与代价感知、让位改事件驱动、后台完成事件收敛、提交路径删除 head
snapshot 整份复检、结构单元改缓冲顺序读，以及提交与恢复导出的三段式有界管道全部落地。计划、release
实测与实施记录见 `backup-scheduling-fix-plan-2026-10-20.md`；行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过；GUI 手工验收、一天日志观察与 release 下同一
1.378 GB 分支的提交/恢复复测仍待人工，验收项在 `../todo.md`。

## 缩略图格式扩展第一批（已提交）

节点缩略图新增三种来源——`.sai2`（libsai 移植，`intg` 画布优先、`thum` 兜底）、`.procreate`
（ZIP 内的 `QuickLook/Thumbnail.png`）与无魔数的 `.tga`，`supports_preview` 同步登记；其余候选
格式由维护者决定暂缓。计划、格式依据与实施记录见 `thumbnail-format-expansion-plan-2026-10-20.md`；
行为变化见 `CHANGELOG.md` 的 `## 0.3.0-alpha.4 - 2026-10-20`。自动验证已通过（含 `intg` 在 3 个
真实样本上与 SAI 导出 PNG 的逐像素比对）；GUI 手工验收与 `.procreate` 真实样本验证仍待人工，
验收项在 `../todo.md`。

## 切换作品不再等待仓库写入（已提交）

`loadBoard` 改为同步交接，旧画板的未保存改动以快照形式交给模块级后台结算队列
（`pin-board/pendingSettle.ts`）串行写回，后台任务持写锁再久也不影响切换；没有未保存改动时不产生
任何写操作。队列对可重试失败按 0.5 s → 15 s 退避静默重试，不可重试失败保留快照并在素材板里常驻
提示 + 重试；退出与切仓库在 5 秒预算内清空队列。行为变化见 `CHANGELOG.md` 的
`## 0.3.0-alpha.4 - 2026-10-20`，模块口径见 `docs/modules/pin-board.md`。自动验证已通过；界面与
退出行为仍待人工确认，验收项在 `../todo.md`。

## 拖放暂存清理（已提交）

拖放导入的暂存目录改为按需创建、退出删除、崩溃残留下次启动清扫，不再每次运行在系统临时目录里留
一个空文件夹。模块口径见 `docs/modules/authenticity.md`。自动验证已通过；退出与强杀行为仍待人工
确认，验收项在 `../todo.md`。

## 0.3 收尾与认证整改第一批人工验收（2026-10-22 通过）

第一批（正确性修复与死代码清理，已提交）的三项界面行为由维护者实测通过：

- 重启应用后直接打开设置「备份」页，显示的是数据库中最新一条整仓备份记录；从未备份过时
  显示「尚无备份记录」，不再出现「本次运行」或「重启后不保留」的措辞。
- 切换仓库后旧仓库的「最近一次结果」被清空，不串到新仓库。
- 设置页各字段显示正常，旧设置文件中的 `defaultPanel` 字段被忽略且不影响加载。

备份记录的重启读取口径见 `docs/modules/history-and-backup.md`，行为变化见 `CHANGELOG.md` 的
`## Unreleased`，计划见 `0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md` 第 2 节。
第三批、第六批的界面验收项与第七批的人工确认项仍在 `../todo.md`。

## 0.3 收尾与认证整改第七批（已提交，计划归档）

取消发布不再无条件删除仓库内的成品与认证副本：`remove_artifact` 查询时一并取出
`final_artifacts.source_sha256` 与 `certification_records.output_sha256`，登记待清理项前用
`storage::verify_file_sha256` 比对磁盘内容，一致才入队；被替换或已不存在的文件不登记、原样
留在仓库里，由 usecase 写一条 `[cleanup] retained <绝对路径>：内容已变化或文件不存在` 日志，
交给设置页「空间」页的垃圾文件扫描发现。取消发布本身仍然成功，`CleanupReport` 与前端契约
不变。质量预览缓存加了会话内上限：每写入一份新缓存就按 JPEG 修改时间从旧到新淘汰，保留最近
16 份，JPEG 总字节超过 512 MiB 时继续删最旧的（至少留一份），`.jpg` 与同名 `.json` 成对删除，
失败静默忽略。同批删除三个无前端消费方的响应字段——发布结果宽高、识别结果回显的解码区域、
估算响应的源字节数（界面「原图」大小来自预览响应，不受影响）。

计划正文与七批的逐批实施记录随本批收尾整理归档到
`0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md`；模块口径见
`docs/modules/authenticity.md`，行为变化见 `CHANGELOG.md` 的 `## Unreleased`。自动验证已通过
（`cargo fmt --check`、`cargo check --all-targets`、`cargo test --lib`、`npx tsc --noEmit`、
`npm test`、`git diff --check`）；两条文件行为确认项（被替换的文件是否真的留在仓库里、垃圾
扫描能否列出，以及预览缓存目录体积）与第三、六批的界面验收一起仍待人工，验收项在
`../todo.md`。
