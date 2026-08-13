# 历史与增量备份模块

## 模块边界

- `src-tauri/src/library/`：仓库初始化、作品树、搜索与项目回收站。
- `src-tauri/src/history/`：分支、历史节点、fork、head 和历史元数据事务。
- `src-tauri/src/backup/`：原始 ChunkFile、提交、checkpoint、恢复、取消与托盘调度。
- `src-tauri/src/authenticity/`：认证聚合边界；内部已分为 `c2pa`、`trustmark` 和 `pipeline`，本阶段不加载重依赖。
- `src-tauri/src/storage.rs`：共享 SQLite 连接、ID、时间、路径与基本校验，不包含领域流程。

`backup` 通过 `history` 切换分支 head，不直接操作作品树；`history` 不读取 ChunkFile；C2PA 与 TrustMark 实现彼此独立，只由认证流水线编排。

## ChunkFile

`backup/chunk_file.rs` 完整迁移自 LilithClient `backup_agent/chunk_file.rs`。snapshot 使用内容定义分块，默认 min/avg/max 为 2 KiB / 16 KiB / 64 KiB，整文件与块摘要均为 SHA-256。delta 是从当前子 snapshot 还原父节点的反向增量，并使用 zstd level 6 包装。

fork 后同一父节点允许多个子节点，因此 schema v3 使用 `history_edges` 让每条 `child_history_id -> parent_history_id` 边分别拥有 delta 文件；不复用线性历史的单一后继假设。每个分支 head 保留完整 snapshot，旧 head 没有其他分支引用时才释放 snapshot。

## 提交与恢复

提交顺序沿用 LilithClient：读取前后比较源文件元数据，临时生成 snapshot/delta，`sync_all`，以不覆盖方式发布文件，最后在 SQLite 事务中切换 head。数据库失败会清理本次新文件；相同 SHA-256 只更新时间，不创建节点。手动提交备注可为空并生成未命名提交；调度器使用独立的 automatic 类型和空备注，不会覆盖手动备注。

恢复从目标节点向下寻找最近可用 snapshot，再沿父链应用反向 delta，最后以临时文件导出且禁止覆盖。fork 一个不再拥有 snapshot 的旧节点时，先物化并发布 checkpoint，保证新分支后续提交有稳定基线。

## 调度

每个分支保存独立开关与 1 到 10080 分钟的检查间隔。调度线程随 Tauri 应用启动，主窗口隐藏到托盘时继续运行；显式退出时请求取消并等待线程结束。分支绑定最终成品后会自动从调度查询排除。分支起点、分支 head 和显式设置的节点会保留完整 snapshot 作为 checkpoint。

## 历史操作

历史总览是父子 mindmap，分支视图则列出当前 head 的祖先链。节点操作统一位于右键菜单；总览点击只选择节点，右键菜单用于进入分支、删除分支和其它破坏性操作。恢复会把目标节点物化到原文件同目录的 `_restored` 文件，并报告可取消进度。

精简只允许在当前分支视图中选择有一个子节点且不是叶节点、分支 head、fork 起点或检查点的普通中间节点。任务会重新物化父子节点，使用原始 ChunkFile API 重建新的反向 delta，再以事务改接历史边并销毁旧节点和不再引用的文件。删除子树同样绕过回收站；若仍有完整分支指向后续历史，会先拒绝并要求删除对应分支。

全局设置和托盘菜单均可暂停所有自动备份，状态会持久化；提交工具显示保存中、已保存和未保存状态。

## 命令

```text
get_artwork_history
fork_artwork_branch
update_artwork_branch
run_branch_backup
restore_history_node
get_backup_runtime_status
cancel_backup_operation
rename_history_node
set_history_checkpoint
compact_history_node
delete_history_subtree
```

本轮还增加了分支删除约束、当前分支精简模式、恢复文件命名、自动备份总暂停和保存状态提示；完整编译与 GUI 测试由人工执行。
