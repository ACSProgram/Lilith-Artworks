//! 历史仓储单测共用的最小仓库夹具。
//!
//! 只供 `#[cfg(test)]` 使用：建一个仓库、一个作品与主分支，并允许直接插入
//! 历史节点。测试自己负责校验各模块的持久化结果。

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{commit, HistoryCommit};
use crate::storage;

pub(crate) struct HistoryFixture {
    /// 只用于持有临时目录的生命周期：夹具存活期间仓库目录不会被删除。
    pub(crate) _directory: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) artwork_id: String,
    pub(crate) main_branch_id: String,
    pub(crate) fork_source: PathBuf,
}

impl HistoryFixture {
    pub(crate) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let main_source = directory.path().join("main.psd");
        let fork_source = directory.path().join("fork.psd");
        fs::write(&main_source, b"main").unwrap();
        fs::write(&fork_source, b"fork").unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &main_source, false)
                .unwrap();
        Self {
            _directory: directory,
            root,
            artwork_id: artwork.artwork_id,
            main_branch_id: artwork.branch_id,
            fork_source,
        }
    }

    pub(crate) fn commit_node(
        &self,
        branch_id: &str,
        id: &str,
        parent_id: Option<&str>,
        created_ms: i64,
    ) {
        let snapshot = format!("artworks/{id}.snapshot");
        let delta = parent_id.map(|parent| format!("artworks/{id}-to-{parent}.delta"));
        commit(
            &self.root,
            HistoryCommit {
                id,
                branch_id,
                parent_id,
                title: id,
                note: "",
                commit_kind: "manual",
                created_ms,
                logical_size: 1,
                chunk_file_size: 1,
                sha256: &format!("{:064X}", created_ms),
                chunk_count: 1,
                snapshot_path: &snapshot,
                delta_path: delta.as_deref(),
                delta_size: delta.as_ref().map(|_| 1),
                thumbnail: None,
                thumbnail_version: None,
            },
        )
        .unwrap();
    }
}

pub(crate) fn queued_paths(root: &Path) -> Vec<String> {
    let connection = storage::open(root).unwrap();
    let mut statement = connection
        .prepare("SELECT path FROM pending_file_cleanup ORDER BY path")
        .unwrap();
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    paths
}
