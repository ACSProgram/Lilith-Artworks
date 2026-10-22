# 当前任务交接

更新时间：2026-10-22

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-22 0.3 收尾与认证整改第四批（已实现、自动验证通过并提交）：

- 发布质量预览的高清局部图请求移回 `usePublicationController.previewOutputTile`：`compressed`
  绑定本次预览的缓存 token，`original` 绑定当前分支，各自缺所需上下文时明确拒绝；
  `PublicationPreviewDialog` 只保留请求代次、缓存与去抖，`AuthenticityModule.tsx` 不再直接
  导入 `authenticityApi`。
- 局部图来源收敛为 `types.ts` 的 `OutputPreviewTileSource`；对话框去抖与缓存上限改为具名常量；
  `loupe.ts` 的 `lru` 变量改名 `insertionOrder`，与「按插入顺序淘汰、命中不重排」的实际行为一致。
- 删除无引用的 `.scope-reset` 样式；估算命令返回值统一使用已有的 `FileSizeEstimate`。
- `docs/modules/authenticity.md` 修正质量预览缓存位置、放大镜淘汰策略与记录搜索字段，并补上
  局部图请求经 controller 绑定上下文这一边界；
  tile 命令注释改为「走仓库共享读租约，发布/签名中前端不发起新请求」。
- 测试：dialog 局部图用例改为断言只提交来源与矩形，controller 新增压缩/原图参数绑定与
  缺上下文拒绝的用例。

计划与逐批验证记录见 `0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md` 第 13 节。
第一至第四批已提交；第五、六批未开始。

第三批界面部分（历史页视图、精简与悬停、设置六页的保存/取消/滚动）只做了自动验证与现有
组件测试，标注为**待人工验收**，验收项在 `todo.md`。

## 下一步

- 第三批界面验收：历史总览/当前分支视图切换、精简模式选择与二段确认、节点悬停与缩略图刷新、
  设置六页的保存/取消/滚动与空间页确认弹窗；验收项见 `todo.md`。
- 第一、二批遗留的人工确认（备份页持久化记录、仓库切换清空）见 `todo.md`。
- 第五批（认证后端结构重组与识别路径）待开始。
- 发布前压力测试（含 `manual` 档）与是否打标签仍由维护者单独安排，口径见
  `docs/guides/release-policy.md`。

## 当前基线

- 分支模型：`main` = 0.2 发布线（`v0.2.0`，指向 `0be6224`，仅接受 0.2 线修正）；`dev` = 0.3
  开发线，从 `0be6224` 创建。
- 版本：`dev` 已切 `0.3.0-alpha.6`（未打标签的过渡版本），五处版本字段、`CHANGELOG.md` 与逐版本
  schema 映射同步；日常改动写入 `CHANGELOG.md` 的 `## Unreleased`，切版本与打标签由维护者按
  `docs/guides/release-policy.md` 单独决定。
- repository schema v6（v4 → v5 随 alpha3 阶段 7 落地，v5 → v6 随 alpha5 阶段 1 落地；alpha6
  沿用 v6）；ChunkFile 仓库可混存 v1/v2，新节点默认写 v2（设置页可回退到 v1）。0.3 alpha1 与
  alpha2 保持 v4；**0.3 共有两次迁移**（alpha3 的 v4 → v5 与 alpha5 阶段 1 的 v5 → v6），高于
  当前版本的仓库由旧程序直接拒绝。
- 0.3 基线（阶段 0 一次性采集）已归档到
  `archive/0.3-baseline-2026-10-18.md`；长期回归对照用 `stress_bench`。
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
