# 规划归档索引

本目录只保存已经完成、被替代或不再作为当前执行依据的规划与批次记录，是**历史增量的唯一归属**：
表层文档（`../current-handoff.md` 与 `../todo.md`）只保留当前状况、未完成事项与规划，已完成批次、
已通过的人工验收与被替代的计划一律搬到这里，不在表层累积。
进行中的工作读 `../current-handoff.md`，未完成事项读 `../todo.md`。

## 时间线

- `version-reset-2026-09-12.md`：版本基线重置为 `0.1.0` / repository schema v1，
  清除全部历史候选版与迁移链，含验收结果与发布确认。
- `pin-board-plan-2026-09-12.md`：素材板（pin-board）从 Lilith Client 迁入的原始规划，
  含设计决策、架构设计与 P1–P4 分阶段计划。已实施完毕，被 `docs/modules/pin-board.md` 取代。
- `pin-board-client-comparison-2026-09-13.md`：迁入后与 Lilith Client 原模块的逐文件对比
  报告，用于确认交互层原样迁移并定位回归。结论已并入模块文档。
- `pin-board-migration-2026-09-13.md`：素材板迁入及后续三个修复批次的完整记录与验证结果，
  以及仍在有效设计决策的汇总。
- `task-control-plan-2026-10-03.md`：任务调度总控与空闲链路校验（alpha3 批次 A–D）的规划，
  含 BackupState 任务类型与取消路由、schema v4 校验状态、调度器两级选择与失败警告面。
  已实施完毕，有效契约并入 `docs/modules/history-and-backup.md`；界面项人工验收见
  `handoff-log-2026-10-16.md`。
- `cleanup-system-plan-2026-10-04.md`：统一清理体系（批次 A–F）的规划，覆盖画板结算改提交后
  清理、历史文件清理入队、未引用文件扫描、灾备暂存目录清扫、完整性扫描覆盖画板 DDS 与双向
  检查、可观测 UI 与文档收尾。六个批次已全部实施完毕，有效契约并入
  `docs/modules/history-and-backup.md`、`docs/modules/pin-board.md` 与
  `docs/architecture/overview.md`；界面项人工验收见 `handoff-log-2026-10-16.md`。
- `stress-test-plan-2026-10-04.md`：自动化压力测试规划（批次 1–8）。采用无界面命令行入口，
  以**独立进程**调用真实可执行文件，覆盖取消边界、跨进程强杀、事务中途崩溃、大文件端到端、
  规模与灾备、参数边界、崩溃孤儿回收、画板 DDS 完整性、认证发布内存与回读，以及损坏文件的
  检测与恢复。八个批次已全部实施完毕，有效契约并入 `docs/guides/validation.md`、
  `docs/user/stress-test-report.md`，以及 `docs/modules/history-and-backup.md` 与
  `docs/modules/authenticity.md` 的「可靠性不变量与覆盖」；发布前实测数据见
  `../../user/stress-test-report.md`。
- `handoff-log-2026-09-12-to-2026-10-08.md`：2026-09-12 至 2026-10-08 各批次交接记录的归档，
  含素材板迁入、任务调度总控、统一清理体系 A–F、压力测试批次 1–8、素材板退出握手等批次的
  实施细节、落实偏差与验证记录。当前批次状态改看 `../current-handoff.md`。
- `pin-board-handoff-2026-10-09.md`：素材板三个待办交互（拖放导入、`Ctrl+A` 全选、Shift 旋转
  绝对角度吸附）与「切换作品/画板卡死」的完整调查记录，含复现条件、已排除假设、日志分析方法
  与复用渲染器的修复方案。三项交互与卡死修复均已实施并人工验收；有效契约并入
  `docs/modules/pin-board.md`，诊断能力并入 `docs/guides/logging.md`，验收结果见
  `handoff-log-2026-10-16.md`。
- `handoff-log-2026-10-16.md`：2026-10-16 从 `../current-handoff.md` 清出的全部已验收批次与人工
  验收结果——确认弹窗取消语义与精简批级取消、窗口几何持久化与设置加载稳健性、日志与诊断体系
  （含冻结遗留项关闭）、素材板结算边界与文档整理与 0.2.0-rc.1 版本切分、画板创建不再遗留空目录、
  素材板纹理未就绪占位符、素材板交互补全、素材板切换卡死，以及统一清理体系 A–F 界面项的验收
  结论，并附 2026-10-04 至 2026-10-10 的早前验收记录；同日稍后追加干净 Windows 用户环境桌面
  验收（A）与异常 RFC 3161 时间戳（B）的通过记录，以及同日清出的三批测试补强（属性测试与仓库
  写并发、覆盖与基线补齐、输入域与畸形输入模糊）与 `0.2.0-rc.3` 压力测试重跑的验证记录，以及
  2026-10-17 对素材板不可用说明的复验（通过）；2026-10-17 再追加任务调度总控与安全批次界面项
  随其它设备桌面验收的通过记录、2026-10-16 维护者决策记录的归档，以及私密漏洞报告渠道开启与
  签名不可用的发布门槛现状；2026-10-17 续记 `0.2.0-rc.4` 切分与 README 收尾、`v0.2.0-rc.4`
  标签事实与「rc.4 压测即 0.2.0 正式版压测」的维护者口径。当前批次状态改看
  `../current-handoff.md`。
- `handoff-log-2026-10-18.md`：2026-10-18 从 `../current-handoff.md` 清出的 0.2 收口记录——
  `0be6224` 切分 `0.2.0`、`v0.2.0` 附注标签指向该提交、rc.4 全套餐压力测试作为 0.2.0 压测的
  口径与实测、自动与人工门槛现状、未签名发布口径，以及 `main` 冻结为 0.2 发布线、`dev`
  从 `0be6224` 建立为 0.3 开发线的分支模型；同日补齐 **0.3 alpha1（阶段 1–5）** 的收口——
  `0.3.0-alpha.1` 切分、逐阶段提交与实现记录、各阶段落实时定稿的口径，以及从
  `0.3-architecture-algorithm-plan-2026-10-05.md` 裁剪出的已完成计划正文。当前批次状态改看
  `../current-handoff.md`，剩余工作看该计划文档。
- `0.3-architecture-algorithm-plan-alpha1-2026-10-05.md`：0.3 计划裁剪出的**完整原文快照**——
  被替代的文件头、§0 复查结论（三轮复查与落实前修订）、§1 现状判断、§2 目标架构、§3.A–E（阶段
  1–5 的实现口径与 §3.D 研究结论）、§4 分阶段实施顺序、§5 验证与完成条件、§6 非目标与待决策；
  §3.G/H/F/I 未复制到此，后续留在 `0.3-architecture-algorithm-plan-2026-10-05.md` 的 §2.1–2.4。
  正文按裁剪前口径保留原始 § 编号与「两批交付」的批次划分。
- `alpha1-stress-failure-fix-plan-2026-10-18.md`：alpha1 压力测试未通过项修复计划（阶段 1–3）的
  **完整正文**。记录 2026-10-18 release 档整套复跑中 8 条未通过记录的已确认根因与修复口径
  （B/C1/C5 同源的检查点膨胀、C2/C4/C6 的 v1 快照体积断言、G4 小图像素域回读），外加维护者要求的
  小选区水印警告与 `stress_concurrency` J5 的并发断言口径；三阶段均已实施并复验，逐条结果并入
  `../../user/stress-test-report.md`，检查点分层的验证口径并入 `docs/guides/validation.md`，本批次的
  实施与归档记录见 `handoff-log-2026-10-18.md`。
- `repository-write-lock-plan-2026-10-18.md`：同仓库写入排队（消除同进程 owner lock 冲突）的
  独立计划与实施记录。含三个用户可见症状（偶发「另一个进程写入」提示、那次启动界面停在仓库
  不可用、退出时 `pin-board finalize failed`）的同一根因分析、「锁的两层/取锁点/启动顺序」的
  现状盘点，以及进程内写入闸门（Shutdown > Foreground > Interactive > Background，等待有上限
  且可取消）的设计、优先级与让位表。已实施完毕并补入两处实施期修正（`Gate::acquire` 入口复查
  `retired` 关闭「僵尸闸门」窗口、`OwnerAttempt` 装箱与 `#[must_use]`）；有效契约并入
  `docs/architecture/overview.md`、`docs/modules/library.md` 与 `docs/modules/history-and-backup.md`，
  行为变化见 `../../../CHANGELOG.md` 的 `## 0.3.0-alpha.2` 小节；维护者已于 2026-10-19 验收该修复（见
  `handoff-log-2026-10-19.md` 第 7 节），人工验收场景与 `stress_concurrency` I/J 复跑仍记于
  `../todo.md`。
- `handoff-log-2026-10-19.md`：alpha1 压力测试收尾与 CPU 时间度量的批次记录——测试侧新增子进程
  CPU 时间（`GetProcessTimes` + 周期计数，双记墙钟、`#cpu` 基线键、50 ms 门槛、
  `LILITH_STRESS_PROFILE`、基线 `machine` 字段，零新 crate），release 档补跑四档大文件
  （均 8/8）、16K 认证大图、规模组与真实仓库基线，`stress_large` C4 的账本口径按 v2 快照增长
  修正，P2/P8 的单轮采样改为三轮取最小以消除严格门的噪声误报，以及 `manual` 档改为只在正式
  发布前运行的维护者口径。有效契约并入
  `docs/guides/validation.md`、`docs/guides/release-policy.md` 与
  `../../user/stress-test-report.md`；同日稍后追加 **alpha2 收口**——维护者验收阶段 6、同仓库写
  入排队修复与超期临时目录清扫，遗留的人工验收与 `stress_concurrency` I/J 复跑移入
  `../todo.md`；阶段 9 计划由「per-branch 全链校验 + `branch_notices`」改向为「空闲完整性检查
  （复用 scrub 内核的空闲增量）」；同日切分 `0.3.0-alpha.2`（未打标签）。当前批次状态改看
  `../current-handoff.md`。
- `0.3-architecture-algorithm-plan-2026-10-05.md`：0.3 计划的完整正文（原文自 `../` 移入）——
  批次划分与依赖、阶段 6–9 的设计口径与落实记录、alpha2 的测试与修复判据、实施顺序与完成条件、
  非目标与已落定的实现决策。阶段 1–9 已全部实现，alpha3 切分为 `0.3.0-alpha.3`（未打标签）；
  文件头已标注**0.3 未完成**——阶段 9 的 GUI 手工验收暂时遗留，发布前压力测试与打标签仍待
  维护者安排。当前状况见 `../current-handoff.md`，未完成事项见 `../todo.md`。
- `handoff-log-2026-10-20.md`：alpha3 收口与文档整理的批次记录——阶段 7（画板条目多态与 v4 → v5
  迁移，人工验收通过）、阶段 8（节点缩略图，含大画布 `.psd`/`.psb` 预览与「重新获取缩略图」，
  人工验收通过）、阶段 9（空闲完整性检查，GUI 手工验收暂时遗留）、历史节点缩略图悬停预览、
  画板文字定位与添加文字对话框、清理体系收口与检查点增量精简、2026-10-19 晚快速档压测复跑、
  `0.3.0-alpha.3` 切分与 CHANGELOG 写法调整、本轮把 alpha3 与 0.3 计划搬入归档并同步落后口径的
  记录，以及拖放暂存目录清理（退出删除 + 崩溃残留下次启动清扫）与「切换作品不再等待仓库写入」
  （旧画板改动交后台结算队列）两处修复；末尾追加阶段 9 的人工验收结论（分批校验与损坏检出均
  实测确认通过，待查集合的优先级顺序仍留待确认）与紧随其后的校验可见性改版，见该文件第 12 节。
  当前批次状态改看 `../current-handoff.md`。
- `platform-code-consolidation-plan-2026-10-20.md`：平台代码集中与可移植性的独立计划与实施记录。
  含平台专属逻辑的九处盘点（`file:line` 已核实）、`src-tauri/src/platform/` 的接口设计（业务语义
  命名、能力缺失显式、`cfg` 只准出现在该模块内）、例外登记表与「加平台清单」的口径、四项维护者
  决策（压力测试辅助代码不纳入、CI grep 随批实施、前端 `normalizeRepositoryKey` 保留并登记）、
  收益与代价，以及实施后的偏差与验收前整理明细。有效规则并入 `docs/guides/platform.md`，界面项
  人工验收在 `../todo.md` 追踪。
- `backup-scheduling-fix-plan-2026-10-20.md`：自动备份调度与快速检查修复的独立计划与实施记录
  （阶段 1–5 已全部实施）。含四类问题的实测证据（指纹过度要求亚秒精度让快速检查从未生效、调度器
  只看时间戳、退化静默无日志、前台结束后无冷却）、release 分段耗时（1.378 GB 提交 5.9 秒、恢复
  4.2 秒及各阶段占比）、单步微基准与据此作废的原排序，以及五项落实口径：指纹按系统实际精度参与、
  前台静默 10 秒 + 调度器代价感知、让位改事件驱动 + 后台完成事件区分结构变化、删除提交路径的 head
  snapshot 整份复检并把结构单元改为缓冲顺序读、提交与恢复导出的三段式有界管道。有效契约并入
  `docs/modules/history-and-backup.md`，行为变化见 `../../../CHANGELOG.md` 的 `## Unreleased`；
  release 复测、GUI 手工验收与日志观察在 `../todo.md` 追踪。
- `thumbnail-format-expansion-plan-2026-10-20.md`：历史节点缩略图格式扩展的独立计划与实施记录。
  含 11 项候选格式的预览来源与格式依据、接入设计（`format/container` 不扩张、复用 512 px / 无损
  WebP / 解码预算 / degrade 契约）、`.sai2` 两种内嵌预览（`intg` 的 dpcm 分块与 `thum` 的 JSSF→
  JPEG 重打包）与 `.blend` 双样本实证；实施记录为维护者确认的缩小范围——只做第一批 `.sai2`、
  `.procreate` 与无魔数的 `.tga`，其余暂缓。记录里包含三处落地口径：`intg` 优先于 `thum`（后者
  约 256 px，会被 512 px 目标放大成糊图）、`intg` 与 SAI 导出 PNG 的逐像素验证、`zip` 只以
  `deflate-flate2` 引入（复用 c2pa 已带的 8.6）。有效契约并入
  `docs/modules/history-and-backup.md`，行为变化见 `../../../CHANGELOG.md` 的 `## Unreleased`；
  GUI 手工验收在 `../todo.md` 追踪。
