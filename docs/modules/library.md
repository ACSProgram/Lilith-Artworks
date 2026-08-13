# Artwork 树模块

## 上下文入口

- 页面工作流：`src/modules/library/LibraryModule.tsx`
- 树渲染与拖放：`src/modules/library/LibraryTreeView.tsx`
- 多选与树计算：`src/modules/library/tree.ts`
- 前端命令和类型：`src/modules/library/api.ts`、`types.ts`
- Rust 命令边界：`src-tauri/src/library/mod.rs`
- SQLite 与事务：`src-tauri/src/library/repository.rs`

## 交互

- 分组可以任意嵌套，Artwork 是叶节点。
- 单击选择节点；单击分组同时切换展开。`Ctrl`/`Command` 切换单项选择，`Shift` 选择当前可见范围。
- 拖动已选择节点会按树中的显示顺序批量移动。分组中央表示移入，节点上下边缘表示同级前后排序；拖到“全部作品”移动到根级。
- 新建 Artwork 必须同时填写标题、初始分支标题并选择现有工作文件。Rust 负责绝对路径、普通文件、仓库外路径和同 Artwork 分支路径唯一性检查。
- 搜索匹配节点标题与 Artwork 主分支工作文件路径；选择结果会展开完整祖先路径并定位节点。
- 右键菜单提供新建、重命名和移到回收站。服务端再次校验叶节点、循环移动和多选父子去重，不依赖前端保证数据完整性。

## 回收站

作品树节点删除采用 SQLite 软删除。选择分组时，其完整子树作为一个回收站项目保存；同时选择父节点和后代只生成一个回收站根。

- 恢复优先返回原父分组和原排序位置。
- 原父分组已不存在或仍在回收站时，项目恢复到树根。
- 永久删除和清空只能从回收站执行，并会删除项目的 Artwork、分支、历史与认证元数据。
- Artwork 内部的历史节点裁剪不使用项目回收站，仍按历史图规则直接永久删除。

schema version 2 增加 `trashed_ms`、`trash_root_id`、`restore_parent_id` 和 `restore_position`；version 3 增加分支调度字段和 fork 安全的 `history_edges`；version 4 增加提交备注、提交类型和检查点；version 5 把最终成品固定关联到发布节点，并扩展 C2PA/TrustMark 配置与不可变导出记录。打开旧仓库时按版本原地迁移。Library 负责迁移和级联清理，但不执行业务发布流程。

## 当前命令

```text
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
