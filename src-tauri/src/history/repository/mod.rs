//! 历史图的主仓储：分支读写、节点图事务、存储元数据与缩略图。
//!
//! - `branches`：分支列表、创建、设置更新与读取；
//! - `graph`：节点读取、提交、物化链、检查点与精简；
//! - `storage`：snapshot/thumbnail 落库、存储元数据推导与作品目录。
//!
//! 这里负责图结构与 SQLite 事务，不读取 ChunkFile；破坏性删除见
//! `deletion_repository`，校验状态见 `verification_repository`，调度状态见
//! `schedule_repository`。

mod branches;
mod graph;
mod storage;

pub(crate) use branches::{create_branch, list, load_branch, update_branch};
pub(crate) use graph::{
    apply_compaction, checkpoint_snapshot_holder, commit, compaction_target, load_node,
    mark_checkpoint, materialization_chain, node_source_path, node_storage_path,
    plan_checkpoint_release, rename_node, unmark_checkpoint,
};
pub(crate) use storage::{
    artwork_directory, ensure_directories, load_thumbnail, set_snapshot, store_thumbnail,
};
