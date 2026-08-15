# 2026-08-15 优化与验收归档

本归档承接 `optimization-2026-08-14.md` 的阶段 1-2，记录已提交并完成相应验证的阶段 3-4 与阶段 A-G。当前未验收工作只读 `../current-handoff.md`。

## 优化阶段 3：发布意图与孤儿文件恢复

- 最终成品、认证外部输出和仓库副本在正式发布前登记带 SHA-256 的清理意图。
- `final_artifacts` / `certification_records` 写入与清理意图移除在同一 SQLite 事务提交。
- 清理器删除前扫描最终成品、认证记录和历史图引用；数据库已接管的文件会被保留。
- 认证外部输出使用 `persist_noclobber`；认证记录在提交前于同一事务回读。

## 优化阶段 4：历史删除与精简不变量

- 删除预检在建立保留检查点前拒绝包含发布节点的子树，并与正式删除复用同一查询。
- 删除事务只更新 head 或 fork 起点实际位于删除集合内的分支，不改写无关旁支时间戳。
- 精简事务同步改接节点父链、`history_edges`、兼容 `delta_path` 和旧文件候选。

## 阶段 A：导航与设置一致性

- Artwork 标签与页面标题内容起点统一。
- 历史时间轴增加日期导航和节点居中跳转。
- 设置弹窗统一布局；设置快照增加参与调度的不同工作文件计数。

## 阶段 B：仓库丢失保护

- 普通数据库连接只使用 `READ_WRITE`，缺失数据库时不创建占位文件。
- 已配置仓库只验证现有数据库；首次配置或切换路径才允许显式初始化。
- 备用数据库迁移先 checkpoint WAL、关闭连接并清理 sidecar，再改为标准名称。

## 阶段 C：退出、取消与 WAL 恢复

- `BackupState` 增加不可逆退出状态，退出会拒绝新任务、取消活动任务并等待共享运行锁释放。
- 真实 WAL 回归确认备用数据库迁移后已提交数据可读。
- 设置原子覆盖回归确认重复保存保持可读。

## 阶段 D：窗口顶栏与历史展示拆分

- 主窗口使用无原生装饰的应用顶栏；窗口命令通过最小 Tauri capability 调用。
- 自定义关闭仍进入 Rust `CloseRequested` 生命周期。
- 时间轴条目显示“创建分支 - 节点标题”。
- 时间轴与 mindmap 纯展示从 `HistoryModule.tsx` 拆到 `HistoryGraph.tsx`。

人工验收时曾记录最小化图标与最大化图标相同。`WindowTitleBar.tsx` 随后改用 `Minus` 并调用 `appWindow.minimize()`；用户已完成最终编译和目视确认。

## 阶段 E：Library 展示拆分与前端依赖反转

- Library 弹窗拆到 `LibraryDialogs.tsx`，菜单、空状态和概览拆到 `LibraryViews.tsx`。
- `App` 注入 Artwork 工作区渲染和清理重试，跨作品认证导航由应用层转换。
- 共享清理 DTO 移到 `src/shared/fileCleanup.ts`。
- Authenticity 使用最小分支视图；静态检查确认前端业务模块之间无直接导入。

## 阶段 F：Library schema 边界拆分

- 仓库 format、schema version、迁移和完整性校验集中到 `library/schema.rs`。
- Library 业务仓储只负责领域读写，通过 schema 公开能力打开并校验连接。
- 初始化、已配置仓库缺失保护、备用数据库/WAL 接管和 Library 树操作定向回归通过。

## 阶段 G：仓库校验缓存与统一入口

- `AppState` 按仓库路径缓存完整校验结果，并串行化并发首次访问。
- 缓存命中仍执行 format 与 schema version 轻量校验；仓库缺失、清空或契约变化会立即拒绝并清除缓存。
- Library、History、Authenticity、文件清理、手动备份和后台调度统一通过 `ready_repository_path()` 取得已就绪仓库。
- 设置保存仍强制完整校验或显式初始化，Library 普通连接不再重复完整校验。

## 已完成验证

自动验证包括前端生产构建、Rust lib 测试目标真实编译、schema v7、文件清理、发布意图、历史删除/精简、仓库打开、退出等待、WAL 恢复、设置原子覆盖等定向回归，以及 Rust 格式、Cargo metadata、Tauri JSON 和差异检查。阶段 E 完成后模块反向导入静态搜索无结果。

用户已完成人工检查：标签与标题对齐、时间轴导航和跳转、设置浅色/深色及窄窗口布局、调度计数、长任务退出、顶栏拖动/最大化/最小化/关闭到托盘、时间轴同名节点、Library 创建/搜索/重命名/回收站、跨 Artwork 认证导航、认证 claim/冲突与清理重试、发布/认证导出/重启 reconciliation，以及阶段 G 的跨页面仓库读取、仓库切换后调度与设置统计。H01-H03 已由用户确认编译并验证完成。

未运行全量 `cargo test`、GUI 自动化、真实 C2PA 第三方验证器和 TrustMark ONNX 自动化；GUI 自动化按项目验证策略禁止。
