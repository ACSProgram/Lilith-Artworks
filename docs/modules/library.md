# Artwork 树模块

## 上下文入口

- 页面选择、菜单、弹窗和渲染：`src/modules/library/LibraryModule.tsx`
- 树读取、搜索、mutation、回收站和请求代次：`src/modules/library/useLibraryController.ts`
- 新建/重命名与回收站弹窗：`src/modules/library/LibraryDialogs.tsx`
- 命令菜单、空状态与节点概览：`src/modules/library/LibraryViews.tsx`
- 树渲染与拖放：`src/modules/library/LibraryTreeView.tsx`
- 多选与树计算：`src/modules/library/tree.ts`
- 前端命令和类型：`src/modules/library/api.ts`、`types.ts`；`api.ts` 只由 controller 消费
- Rust 单领域命令边界：`src-tauri/src/library/mod.rs`
- 创建 Artwork、永久删除和清空回收站的跨领域编排：`src-tauri/src/app/workflows.rs`
- 仓库格式、schema 与迁移：`src-tauri/src/library/schema.rs`
- 仓库定位、Library 查询与事务：`src-tauri/src/library/repository.rs`

## 交互

- 分组可以任意嵌套，Artwork 是叶节点。
- 单击选择节点；单击分组同时切换展开。`Ctrl`/`Command` 切换单项选择，`Shift` 选择当前可见范围。
- 拖动已选择节点会按树中的显示顺序批量移动。分组中央表示移入，节点上下边缘表示同级前后排序；拖到“全部作品”移动到根级。
- 新建 Artwork 必须同时填写标题、初始分支标题并选择现有工作文件。Rust 负责绝对路径、普通文件、仓库外路径和同 Artwork 分支路径唯一性检查。
- 搜索匹配节点标题与 Artwork 主分支工作文件路径；选择结果会展开完整祖先路径并定位节点。
- 右键菜单提供新建、重命名和移到回收站。服务端再次校验叶节点、循环移动和多选父子去重，不依赖前端保证数据完整性。
- Library 不直接导入应用、History 或 Authenticity 模块。应用层注入 Artwork 工作区渲染器和文件清理重试，并把认证记录转换为 `{ artworkId, branchId, recordId }` 导航目标；Library 只负责展开祖先、切换当前 Artwork 和保存定位目标。
- `useLibraryController.ts` 是 `library/api.ts` 的唯一消费者。仓库切换会递增请求代次并清空旧树、选择、搜索和回收站状态；旧仓库的读取、搜索和 mutation 结果不能覆盖新仓库。`LibraryModule.tsx` 只编排选择、上下文菜单、编辑器、回收站窗口和 Artwork 工作区渲染。

## 仓库打开与初始化

- 只有从未配置仓库或把仓库设置改到另一个路径并保存时，应用才允许在空目录中初始化新数据库。
- 已配置路径的状态查询、Library/历史/认证命令、清理重试和后台调度都通过应用状态的统一仓库就绪入口，只打开现有数据库，不具备隐式创建能力。
- 如果已配置目录不存在、被外部清空或缺少 `lilith-artworks.sqlite3`，仓库状态返回不可用并保留目录原状；用户需要重新选择新仓库，或先把数据库从备份恢复到原目录。
- 共享 `storage::open` 使用不带 `CREATE` 的 SQLite `READ_WRITE` 打开标志；缺少数据库时直接失败，不生成无 schema 的占位文件。显式初始化负责新 schema 和受控目录创建。
- `AppState` 按当前仓库路径缓存完整校验状态。首次访问、启动状态读取和设置保存执行 `integrity_check(1)`、format/version 校验及迁移；同一路径的后续命令只检查标准数据库仍存在且 format/schema version 仍为当前值。轻量检查失败会立即清除缓存，下一次访问必须重新完成完整校验。
- 发现旧名称的备用 `.sqlite3` 时，先执行 `wal_checkpoint(TRUNCATE)` 合并已提交 WAL、关闭连接并清理旧 sidecar，再改名为标准数据库名；如果仍有进程占用导致 checkpoint busy，则保留原文件并返回错误，不做不完整迁移。
- `schema.rs` 是仓库格式常量、当前建库 SQL、完整校验、轻量校验和逐版本迁移的唯一所有者；Library 业务仓储不再内嵌 schema SQL。

## 回收站

作品树节点删除采用 SQLite 软删除。选择分组时，其完整子树作为一个回收站项目保存；同时选择父节点和后代只生成一个回收站根。

- 恢复优先返回原父分组和原排序位置。
- 原父分组已不存在或仍在回收站时，项目恢复到树根。
- 永久删除和清空只能从回收站执行。应用工作流在共享运行锁内调用 Library 仓储，把 Artwork 受控目录和带期望 SHA-256 的外部认证导出写入待清理队列，再提交元数据删除并运行清理；文件失败会返回清单并保留为可重试项。
- 永久删除元数据时先显式删除 `final_artifacts`（级联认证记录），再删除历史节点和 Library 根，避免发布节点上的 `ON DELETE RESTRICT` 阻断已发布 Artwork 的清理；分支引用在删除历史前置空。
- Artwork 内部的历史节点裁剪不使用项目回收站，仍按历史图规则直接永久删除。

schema v8 将认证记录的仓库副本路径提升为非空约束；没有仓库副本的旧认证记录在迁移时移除。schema v7 增加 `pending_file_cleanup`：仓库文件/目录只保存相对路径并在执行前校验仓库边界；仅发布创建失败等应用仍拥有外部临时结果的流程登记外部文件，取消发布不删除用户的首次导出文件。成功项出队，失败项记录错误；应用启动和页面重试均通过应用层清理命令执行。更早 schema 变更见规划归档。

清理命令仍由应用层拥有；Library 通过注入回调重试失败项。跨 Library、Authenticity 和应用层复用的清理结果 DTO 位于 `src/shared/fileCleanup.ts`，不再反向导入 `src/app/types.ts`。

## 当前命令

```text
get_repository_status
list_library_tree
search_library
create_library_group
create_library_artwork
rename_library_node
move_library_nodes
trash_library_nodes
list_library_trash
restore_library_trash
permanently_delete_library_trash
empty_library_trash
```

所有写操作在 SQLite 事务内完成。树展开状态按稳定节点 ID 保存到浏览器 `localStorage`；当前选择和搜索文本只属于当前窗口状态。
