# 当前任务交接

更新时间：2026-08-15

## 当前阶段：I - History 前端状态与命令编排拆分

阶段 F/G 已提交并完成验收归档。阶段 H 已提交但用户尚未在本地编译或人工验证；本轮按用户授权直接进入架构拆分，H04-H06 仍是未完成的人工检查。

本阶段先核对问题再修改，三个新增描述均与源码和截图相符：

- 时间轴条目与历史节点已经共享 `selectedNodeId`，但 mindmap 卡片基础背景规则优先级更高，覆盖了通用选中背景，图中缺少明确反馈。
- 时间轴节点排序写死为倒序，标题栏没有顺序控制。
- `.history-graph` 的顶部内边距与标题负外边距共同参与 sticky 布局，标题栏上方会留下可透出下层内容的空隙。

## 本阶段已实现

- mindmap 节点选中态使用强调边框、背景和外框，与时间轴条目的 active 状态同步。
- 当前分支祖先路径使用较弱的左侧强调，当前分支 HEAD 标签使用实色提示，和节点选择层级区分。
- 时间轴标题栏增加“正序 / 倒序”分段切换，默认倒序；日期组和组内节点按同一方向排序。
- 历史图滚动容器不再保留顶部内边距，sticky 标题栏从内容区域顶边开始覆盖，消除透底空隙。
- 排序状态封装在 `HistoryTimeline` 展示组件内，未增加 `HistoryModule` 的页面编排状态，也未改变 DTO 或 Tauri 命令。
- `docs/modules/history-and-backup.md` 已同步当前交互契约。

## 阶段 I 已实现

- 新增 `src/modules/history/useHistoryController.ts`，成为 History 模块调用 `historyApi` 的唯一前端消费者，集中管理历史读取、运行状态轮询、busy/progress 和所有历史 mutation 的结果回填。
- 历史读取使用请求代次和 Artwork ID 双重校验；旧 Artwork、旧刷新请求和旧错误不会覆盖当前历史或污染当前错误提示。
- History 页面切换 Artwork 时统一清理节点、上下文菜单、确认框、Fork/重命名窗口和精简选择，避免旧页面状态泄漏。
- `ArtworkWorkspace.tsx` 删除重复的 `get_artwork_history` 读取；工作区只保留分支选择、标题/分支聚合和刷新版本信号，历史数据由 controller 回推。
- HistoryModule 不再直接依赖 `historyApi`，页面层只编排选择状态、弹窗和展示；未改变 Tauri 命令、DTO 或数据库契约。

## 自动验证

- `npm run build` 通过，TypeScript 与 Vite 生产构建完成。
- `git diff --check` 通过，无空白错误或冲突标记。
- 本轮未新增原生契约或数据库变更，因此不重复 Rust 重依赖编译；阶段 H 的 GUI 人工验收仍未完成。

## 待人工验收

- H04：在时间轴和历史图中分别点击节点，确认两侧选中状态同步，且所选节点提示强于当前分支路径提示。
- H05：进入时间轴模式，确认默认倒序；切换正序/倒序后，日期组和日期内时间均按相同方向变化。
- H06：纵向滚动历史总览，确认标题栏始终贴合内容区顶部，标题栏上方不再出现可透出节点的空隙。
- I04：在快速切换两个 Artwork、从发布页回到历史页和连续触发刷新时，确认标题、分支和节点内容不会短暂显示另一个 Artwork 的旧数据。
- I05：提交、Fork、重命名、删除、检查点和精简操作完成后，确认进度状态、对话框关闭和历史回填与原流程一致。

## 下一阶段候选

1. 核对 history/backup 跨模块工作流的实际调用方向，把确属应用编排的流程上移到应用命令层，并补领域边界测试。
2. 继续拆分 `HistoryModule.tsx`、`AuthenticityModule.tsx` 和 `LibraryModule.tsx` 的状态与命令编排，优先处理异步请求竞态和可独立测试的页面控制器。
3. 扩充认证发布/查询和前端 latest-request-wins 的核心不变量测试。
4. 建立 Windows CI 与公开发布候选门槛；补 LICENSE、README、CONTRIBUTING、SECURITY、CHANGELOG 和发行政策。

暂缓项：长任务 operation ID、进度与取消；dialog/tree 无障碍；大树/大图性能基线；历史 snapshot/delta 清理意图和陈旧临时文件维护命令。

## 验证边界

- 不运行全量 `cargo test`、GUI 自动化、真实 C2PA 第三方验证或 TrustMark ONNX 自动化。
- GUI 自动化按项目验证策略禁止，H04-H06 与 I04-I05 由用户手工完成；下一阶段再处理 Rust 应用命令层编排，避免继续扩大本轮变更面。
