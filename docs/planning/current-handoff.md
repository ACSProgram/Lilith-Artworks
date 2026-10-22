# 当前任务交接

更新时间：2026-10-22

本文件只记录**当前状况**：当前批次状态、当前基线，以及未完成事项的指针。历史增量（已完成批次、
已通过的人工验收、被替代的计划）一律在 `archive/`，不在本文件累积；未完成事项只写 `todo.md`。
规则见文末「文档约束」。

## 当前批次状态

2026-10-22 0.3 收尾与认证整改第六批（已实现、自动验证通过并提交）：

- 认证前端拆为 `PublicationView` / `IdentificationView` 两个页面，以及 `RecordView`、
  `RecordList`、`RegionEditor`、`PublicationPreviewDialog`；`ArtworkWorkspace` 直接挂载
  发布页或识别页，`AuthenticityModule.tsx` 删除。
- 控制器拆为 `usePublicationController` / `useIdentificationController`，共享签名配置与
  PEM 规范化移到 `publicationStorage.ts`；识别预览、解码与记录搜索改用共享
  `useRequestEpoch` 请求代次。
- 质量预览与签名发布共用 `useAuthenticityOperation` 的取消状态机，可见文案与提示时序不变；
  补了取消、导出与识别乱序响应的定向测试。
- `api.ts` 只由两个 controller 消费；模块文档、架构入口文档与 `CHANGELOG.md` 的
  `Unreleased` 已同步。
- 提交前做了一次收尾整理：路径取文件名与错误文案收敛到共享 `fileName` / `errorMessage`，
  删掉 `RegionEditor` 里永不为真的空值判断与识别解码里已被请求代次覆盖的路径比较，并给
  拆出的组件补上具名 props 接口；分支列表三态提示抽成 `BranchStateNotice`。

计划与逐批验证记录见 `0.3-closeout-and-authenticity-refactor-plan-2026-10-22.md` 第 13 节。
第一至第六批均已提交。

第三批界面部分（历史页视图、精简与悬停、设置六页的保存/取消/滚动）与第六批界面部分
（发布/识别页、取消时序、局部图与放大镜、只读记录、拖放导入）只做了自动验证与现有组件
测试，标注为**待人工验收**，验收项在 `todo.md`。

## 下一步

- 人工验收第三批与第六批的界面行为；验收项见 `todo.md`。
- 第三批与第六批的验收记录在通过后归档。
- 第一、二批遗留的人工确认（备份页持久化记录、仓库切换清空）见 `todo.md`。
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
