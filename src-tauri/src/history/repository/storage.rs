//! 历史节点的存储元数据、snapshot/thumbnail 落库与作品目录。
//!
//! 存储元数据统一由 `refresh_storage_metadata` 从图推导；调用方只负责发布文件
//! 并在事务外决定是否重放清理队列。

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{cleanup, storage};

use super::super::verification_repository::clear_artwork_verification;

/// Atomic SET clause that derives `delta_path` and `chunk_file_size` from the
/// history graph: the reverse delta on the edge to the cheapest child. Every
/// statement that clears `snapshot_path` must apply this clause in the same
/// UPDATE, because the table CHECK requires
/// `snapshot_path IS NOT NULL OR delta_path IS NOT NULL`.
pub(super) const STORAGE_METADATA_SET: &str = "delta_path = (SELECT edge.delta_path
                     FROM history_edges edge
                     JOIN history_nodes child ON child.id = edge.child_history_id
                     WHERE child.parent_id = history_nodes.id
                     ORDER BY edge.delta_size, edge.delta_path LIMIT 1),
           chunk_file_size = (SELECT MIN(edge.delta_size)
                              FROM history_edges edge
                              JOIN history_nodes child ON child.id = edge.child_history_id
                              WHERE child.parent_id = history_nodes.id)";

/// Load one node's derived thumbnail. The graph/list queries never select the
/// BLOB, so previews are fetched only when the inspector asks for them.
pub(crate) fn load_thumbnail(
    root: &Path,
    history_id: &str,
) -> Result<Option<(Vec<u8>, i64)>, String> {
    let stored: Option<(Option<Vec<u8>>, Option<i64>)> = storage::open(root)?
        .query_row(
            "SELECT thumbnail, thumbnail_version FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    Ok(stored.and_then(|(bytes, version)| match (bytes, version) {
        (Some(bytes), Some(version)) if !bytes.is_empty() => Some((bytes, version)),
        _ => None,
    }))
}

/// Write a derived thumbnail back to an existing node. Only the preview columns
/// change; the graph, digest and verification state are untouched. Zero affected
/// rows means the id does not belong to this repository, which callers run under
/// the shared run lock and treat as an error rather than a silent no-op.
pub(crate) fn store_thumbnail(
    root: &Path,
    history_id: &str,
    bytes: &[u8],
    version: i64,
) -> Result<(), String> {
    let changed = storage::open(root)?
        .execute(
            "UPDATE history_nodes SET thumbnail = ?2, thumbnail_version = ?3 WHERE id = ?1",
            params![history_id, bytes, version],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        return Err(format!("找不到历史节点 {history_id}"));
    }
    Ok(())
}

/// Recomputes the denormalized storage metadata of a node from the history graph.
///
/// `chunk_file_size` records how many bytes the repository has to read to
/// reconstruct the node: the node's own snapshot when it has one, otherwise
/// the reverse delta stored on the edge to its child. When a node has several
/// children the cheapest child edge is recorded, which keeps the value
/// deterministic for fork points that lost their snapshot.
///
/// Nodes that own a snapshot keep their recorded size, because only the caller
/// that published the snapshot file knows its length. Nodes without children
/// are left untouched for the same reason.
pub(super) fn refresh_storage_metadata(
    connection: &Connection,
    history_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            &format!(
                "UPDATE history_nodes SET
                   {STORAGE_METADATA_SET}
                 WHERE id = ?1
                   AND snapshot_path IS NULL
                   AND EXISTS (SELECT 1 FROM history_nodes child WHERE child.parent_id = ?1)"
            ),
            [history_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 为一个历史节点登记（或替换）snapshot 文件。
///
/// 替换时旧 snapshot 不再由调用方在提交后直接删除：旧路径在同一事务内入队
/// `pending_file_cleanup`，由调用方在成功后重放；返回已入队的 cleanup id。
/// 首次登记（节点原本没有 snapshot，例如建立发布检查点）返回空。
pub(crate) fn set_snapshot(
    root: &Path,
    history_id: &str,
    relative_path: &str,
    file_size: u64,
    checkpoint: bool,
) -> Result<Vec<String>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let previous: Option<(Option<String>, String)> = transaction
        .query_row(
            "SELECT snapshot_path, artwork_id FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let Some((previous, artwork_id)) = previous else {
        return Err("找不到历史节点".into());
    };
    let changed = transaction
        .execute(
            "UPDATE history_nodes SET snapshot_path = ?2, chunk_file_size = ?3,
                    is_checkpoint = CASE WHEN ?4 <> 0 THEN 1 ELSE is_checkpoint END
             WHERE id = ?1",
            params![
                history_id,
                relative_path,
                i64::try_from(file_size).map_err(|_| "checkpoint 文件大小超出范围")?,
                i64::from(checkpoint)
            ],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        return Err("找不到历史节点".into());
    }
    // snapshot 替换改变了该节点的物化起点，按作品粗粒度让校验状态失效。
    clear_artwork_verification(&transaction, &artwork_id)?;
    let mut cleanup_ids = Vec::new();
    if let Some(previous) = previous.filter(|path| path != relative_path) {
        cleanup_ids = cleanup::enqueue_released_repository_files(
            &transaction,
            &[previous],
            "history_snapshot_replaced",
        )?;
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn artwork_directory(root: &Path, artwork_id: &str) -> PathBuf {
    root.join("artworks").join(artwork_id)
}

pub(crate) fn ensure_directories(root: &Path, artwork_id: &str) -> Result<(), String> {
    let directory = artwork_directory(root, artwork_id);
    for name in ["snapshots", "deltas", "temp"] {
        fs::create_dir_all(directory.join(name))
            .map_err(|error| format!("无法创建 Artwork 存储目录：{error}"))?;
    }
    Ok(())
}
