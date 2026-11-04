//! Library 仓储：仓库发现与初始化、作品树读写、回收站三类职责。
//!
//! - `discovery`：数据库定位/初始化、备用库改名与运行时文件排除；
//! - `tree`：作品树读取、搜索、新建、重命名、移动与同级顺序维护；
//! - `trash`：软删除、恢复、永久删除与清空回收站。
//!
//! 三类职责的变更理由不同，但共用同一份 schema。写操作都在 `storage::open` 打开的
//! 单连接内完成，多步写入包在同一个事务里；永久删除会同时改写 History 与
//! Authenticity 的表，归属与原子性见 `docs/modules/library.md` 的「回收站」。

mod discovery;
mod trash;
mod tree;

#[cfg(test)]
mod test_support;

use std::path::Path;

use rusqlite::Connection;

use crate::storage;

pub(crate) use discovery::{
    check_existing, database_path, initialize, inspect_path, open_existing,
};
pub(crate) use trash::{
    empty_trash, list_trash, permanently_delete_trash, restore_trash, trash_nodes,
};
#[cfg(any(test, feature = "headless"))]
pub(crate) use tree::create_artwork;
pub(crate) use tree::{
    create_artwork_and_list, create_group, list_tree, move_nodes, rename_node, search,
};

/// 共享连接入口：只打开既有数据库，不创建占位文件。
pub(super) fn open(root: &Path) -> Result<Connection, String> {
    storage::open(root)
}
