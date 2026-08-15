# 当前任务交接

更新时间：2026-08-15

## 当前阶段：J/K - 应用工作流与前端控制器收口

阶段 H/I 已提交但尚未完成 GUI 人工验收。本轮先按源码核对交接候选，再在两个阶段内完成架构收口；没有数据库 schema 或用户数据迁移。

核对结果：

- “跨模块流程由应用层编排”的文档事实此前与实现不符：History 命令直接调用 Backup，Authenticity 命令直接组合 checkpoint/运行锁，Library 命令直接组合清理和调度唤醒。
- History 已由 controller 独占 API，但 Library 和 Authenticity 页面仍直接调用各自 API；读取、mutation、busy 和 latest-request-wins 分散在页面 effect/callback 中。
- 前端没有独立测试运行器；项目含 C2PA/ONNX 重依赖，验证策略禁止重复执行全量 Rust 编译和 GUI 自动化。

## 阶段 J 已实现

- 新增 `src-tauri/src/app/workflows.rs`，集中注册并编排 create Artwork、永久删除/清空回收站、fork、分支设置/删除、进入/取消发布和认证发布。
- 上述 Tauri 命令名、参数 DTO 和返回 DTO 保持不变，前端无需迁移。
- History 命令不再导入 Backup；Library 命令不再持有 BackupState 或执行清理；Authenticity 命令不再获取共享运行锁或建立 checkpoint。
- 领域模块暴露单领域仓储/服务能力，应用工作流负责组合 `BackupState`、checkpoint、调度唤醒、清理队列和领域结果回填。
- Backup 内部继续通过 History 公开 API 管理提交、恢复、精简和检查点；这是 Backup 的领域职责，不反向读取 ChunkFile 到 History。

## 阶段 K 已实现

- 新增 `src/modules/library/useLibraryController.ts`，成为 `library/api.ts` 的唯一消费者，集中管理树读取、搜索、mutation、回收站、清理重试、busy 和仓库请求代次。
- 仓库切换统一清空旧树、选择、搜索和回收站状态；旧读取、旧搜索和旧 mutation 不得覆盖新仓库。
- 新增 `src/modules/authenticity/useAuthenticityController.ts`，成为 `authenticity/api.ts` 的唯一消费者，分别管理发布与识别工作流。
- 发布状态、成品预览、记录预览、大小估算、识别和记录搜索使用独立请求代次；分支/输入变化后旧响应不会回填当前页面。
- 识别操作与记录搜索使用独立 busy 状态，后台搜索不会误解除识别进度或阻塞图片操作。
- `LibraryModule.tsx` 和 `AuthenticityModule.tsx` 只保留选择、弹窗、RegionEditor 和视图渲染；History/Library/Authenticity 三个页面控制器现在各自独占领域 API。

## 自动验证

- `npm run build` 通过，TypeScript 与 Vite 生产构建完成。
- `cargo fmt --check --manifest-path src-tauri/Cargo.toml` 通过。
- `git diff --check` 通过，无空白错误或冲突标记。
- 按验证策略未运行全量 `cargo check`/`cargo test`、GUI 自动化、真实 C2PA 第三方验证或 TrustMark ONNX 自动化。Rust 应用工作流仍需要用户下一次本地完整编译确认。

## 待人工验收

- H04-H06：节点双侧选择、时间轴正/倒序、sticky 标题栏空隙，沿用上一轮清单。
- I04-I05：快速切换 Artwork 和历史 mutation 回填，沿用上一轮清单。
- J01：完整编译 Tauri；创建 Artwork、Fork、修改分支设置、删除分支、永久删除/清空回收站，确认命令名、进度、调度唤醒和清理结果与原流程一致。
- J02：进入发布、认证导出、取消发布，确认 checkpoint、分支锁定、记录回填和失败清理与原流程一致。
- K01：快速切换仓库、Artwork、发布分支和识别图片，确认旧树、旧发布状态、旧预览、旧识别或旧搜索结果不会闪回。
- K02：检查 Library 新建/重命名/拖放/回收站，以及发布查看/再次导出/识别搜索，确认弹窗关闭、busy 和错误提示与原体验一致。

## 收尾候选

1. 完成上述一次完整编译和 GUI 人工验收，只修复验收发现的问题，不再继续架构拆分。
2. 建立 Windows CI 与公开发布候选门槛；补 LICENSE、README、CONTRIBUTING、SECURITY、CHANGELOG 和发行政策。

暂缓项保持不变：长任务 operation ID、进度与取消协议升级；dialog/tree 无障碍；大树/大图性能基线；历史 snapshot/delta 清理意图和陈旧临时文件维护命令。
