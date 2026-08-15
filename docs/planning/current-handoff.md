# 当前任务交接

更新时间：2026-08-15

## 当前阶段：G - 仓库校验缓存与统一入口

用户已提交阶段 F，但尚未进行本地编译和人工流程测试，因此阶段 F 暂不归档，相关人工项继续保留。

本阶段核对结果与原描述相符，并发现重复范围跨越多个模块：

- Library 命令原先先在 `ready_root` 执行一次完整 `integrity_check`，随后业务仓储打开连接时再次完整检查。
- History、Authenticity 和文件清理命令每次至少完整检查一次；手动备份与后台调度则只读取配置路径，入口行为不一致。
- 设置快照和仓库状态在前端启动时并发读取，原实现没有共享一次完整校验结果。

## 本阶段已实现

- `library/schema.rs` 将校验拆为两级：`validate_and_migrate` 执行 `integrity_check(1)`、format/version 校验和迁移；`validate_current` 只校验 format 与当前 schema version。
- `AppState` 按当前仓库路径缓存“完整校验已通过”的状态。并发首次访问由同一互斥状态串行化，避免设置快照、仓库状态和调度线程重复完整检查。
- 缓存命中仍调用轻量检查；数据库缺失、清空、format 改变或 schema version 不受支持时立即清除缓存。
- 设置保存继续强制完整校验或显式初始化，成功写入设置后才更新缓存；清空或切换仓库会替换缓存路径。
- Library、History、Authenticity、文件清理、手动备份和后台调度统一通过 `AppState::ready_repository_path()` 取得仓库。
- Library 业务仓储普通连接不再自行执行完整校验。

## 自动验证

- 应用设置/校验缓存定向测试：4 个通过，覆盖连续访问只执行一次完整检查，以及缓存命中后 version/format 变化仍被轻量检查拒绝。
- Library 仓储定向测试：14 个通过，覆盖初始化、已配置仓库缺失保护、备用数据库/WAL 接管、树操作和回收站。
- `npm run build`、`cargo fmt --check`、`cargo metadata --no-deps` 和 `git diff --check` 均通过；静态搜索确认前台命令与后台调度已统一使用仓库就绪入口。

## 待人工验收

- H01（阶段 F）：确认主窗口最小化按钮显示为横线，点击后窗口正常最小化。
- H02（阶段 F）：使用可丢弃仓库确认作品树创建、搜索、重命名和回收站流程无变化。
- H03（阶段 G）：启动应用后连续切换 Library、历史和认证页面，确认仓库状态与内容读取正常；切换到新仓库后重新启动，确认调度和设置统计正常。

## 下一阶段候选

1. 扩充认证发布/查询和前端 latest-request-wins 的核心不变量测试。
2. 把 history/backup 跨模块工作流进一步上移到应用命令层。
3. 继续拆分 `AuthenticityModule.tsx`、`LibraryModule.tsx` 和 `HistoryModule.tsx` 的状态与命令编排。
4. 建立 Windows CI 与公开发布候选门槛；补 LICENSE、README、CONTRIBUTING、SECURITY、CHANGELOG 和发行政策。

暂缓项：长任务 operation ID、进度与取消；dialog/tree 无障碍；大树/大图性能基线；历史 snapshot/delta 清理意图和陈旧临时文件维护命令。

## 验证边界

- 不运行全量 `cargo test`、GUI 自动化、真实 C2PA 第三方验证或 TrustMark ONNX 自动化。
- GUI 自动化继续按项目验证策略禁止，H01-H03 由用户手工完成。
