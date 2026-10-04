use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, OptionalExtension};

use crate::{cleanup, storage};

use super::{
    ArtworkBranch, ArtworkHistory, BackupDisableNoticeTarget, BranchDeletion, BranchRecord,
    CompactionTarget, HistoryCommit, HistoryDeletion, HistoryNode, HistoryRecord, IdleVerifyTarget,
    ScheduledBranch,
};

pub(crate) fn list(root: &Path, artwork_id: &str) -> Result<ArtworkHistory, String> {
    let connection = storage::open(root)?;
    let artwork_title = connection
        .query_row(
            "SELECT title FROM library_nodes
             WHERE id = ?1 AND kind = 'artwork' AND trashed_ms IS NULL",
            [artwork_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("找不到 Artwork")?;
    let mut branch_statement = connection
        .prepare(
            "SELECT b.id, b.title, b.source_path, b.head_history_id,
                    b.created_from_history_id, b.backup_enabled, b.backup_interval_minutes,
                    b.last_check_ms, b.last_success_ms, b.last_error,
                    b.consecutive_backup_failures, b.backup_retry_at_ms,
                    b.backup_disable_notice_pending,
                    EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id),
                    (SELECT COUNT(*) FROM certification_records record WHERE record.branch_id = b.id),
                    b.backup_quick_enabled, b.verify_error, b.verified_ms
             FROM branches b WHERE b.artwork_id = ?1 ORDER BY b.created_ms, b.id",
        )
        .map_err(storage::database_error)?;
    let branches = branch_statement
        .query_map([artwork_id], |row| {
            Ok(ArtworkBranch {
                id: row.get(0)?,
                title: row.get(1)?,
                source_path: row.get(2)?,
                head_history_id: row.get(3)?,
                created_from_history_id: row.get(4)?,
                backup_enabled: row.get::<_, i64>(5)? != 0,
                backup_interval_minutes: row.get(6)?,
                last_check_ms: row.get(7)?,
                last_success_ms: row.get(8)?,
                last_error: row.get(9)?,
                consecutive_backup_failures: row.get(10)?,
                backup_retry_at_ms: row.get(11)?,
                backup_disable_notice_pending: row.get::<_, i64>(12)? != 0,
                final_artifact_locked: row.get::<_, bool>(13)?,
                published_count: row.get(14)?,
                backup_quick_enabled: row.get::<_, i64>(15)? != 0,
                verify_error: row.get(16)?,
                verified_ms: row.get(17)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(branch_statement);

    let mut node_statement = connection
        .prepare(
            "SELECT node.id, node.created_on_branch_id, node.parent_id, node.title, node.note, node.commit_kind,
                    (node.is_checkpoint <> 0
                      OR EXISTS(SELECT 1 FROM branches b WHERE b.head_history_id = node.id OR b.created_from_history_id = node.id)
                      OR (SELECT COUNT(*) FROM history_nodes child WHERE child.parent_id = node.id) > 1),
                    node.created_ms, node.logical_size, node.chunk_file_size, node.sha256, node.chunk_count
             FROM history_nodes node WHERE node.artwork_id = ?1 ORDER BY node.created_ms, node.id",
        )
        .map_err(storage::database_error)?;
    let nodes = node_statement
        .query_map([artwork_id], |row| {
            Ok(HistoryNode {
                id: row.get(0)?,
                created_on_branch_id: row.get(1)?,
                parent_id: row.get(2)?,
                title: row.get(3)?,
                note: row.get(4)?,
                commit_kind: row.get(5)?,
                is_checkpoint: row.get::<_, i64>(6)? != 0,
                created_ms: row.get(7)?,
                logical_size: row.get(8)?,
                chunk_file_size: row.get(9)?,
                sha256: row.get(10)?,
                chunk_count: row.get(11)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(ArtworkHistory {
        artwork_id: artwork_id.into(),
        artwork_title,
        branches,
        nodes,
    })
}

pub(crate) fn create_branch(
    root: &Path,
    artwork_id: &str,
    from_history_id: &str,
    title: &str,
    source_path: &Path,
) -> Result<String, String> {
    storage::validate_title(title, "分支标题")?;
    let (source_display, source_key) = storage::normalize_source_path(root, source_path)?;
    let connection = storage::open(root)?;
    let origin_matches: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM history_nodes WHERE id = ?1 AND artwork_id = ?2)",
            params![from_history_id, artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !origin_matches {
        return Err("fork 起点不存在或属于其他 Artwork".into());
    }
    let id = storage::new_id();
    let now = storage::now_ms()?;
    connection
        .execute(
            "INSERT INTO branches
             (id, artwork_id, title, source_path, source_path_key, head_history_id,
              created_from_history_id, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?7)",
            params![
                id,
                artwork_id,
                title.trim(),
                source_display,
                source_key,
                from_history_id,
                now
            ],
        )
        .map_err(|error| {
            if error
                .to_string()
                .contains("branches.artwork_id, branches.source_path_key")
            {
                "同一 Artwork 的每个分支必须使用不同的工作文件路径".into()
            } else {
                storage::database_error(error)
            }
        })?;
    connection
        .execute(
            "UPDATE history_nodes SET is_checkpoint = 1 WHERE id = ?1",
            [from_history_id],
        )
        .map_err(storage::database_error)?;
    Ok(id)
}

pub(crate) fn update_branch(
    root: &Path,
    branch_id: &str,
    title: &str,
    expected_enabled: bool,
    enabled: bool,
    interval_minutes: u32,
    quick_enabled: bool,
    source_path: Option<&str>,
) -> Result<(), String> {
    storage::validate_title(title, "分支标题")?;
    if !(1..=10_080).contains(&interval_minutes) {
        return Err("自动备份间隔必须在 1 到 10080 分钟之间".into());
    }
    // `Some("")` 清除工作文件路径（界面上的“清除工作文件路径”按钮），`None` 表示本次不改路径。
    let normalized_source = match source_path {
        Some(value) if value.trim().is_empty() => Some((String::new(), String::new())),
        Some(value) => Some(storage::normalize_source_path(
            root,
            Path::new(value.trim()),
        )?),
        None => None,
    };
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let branch = transaction
        .query_row(
            "SELECT artwork_id, source_path FROM branches WHERE id = ?1",
            [branch_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "找不到分支".to_owned())?;
    let (artwork_id, current_source) = branch;
    let effective_source = normalized_source
        .as_ref()
        .map(|value| value.0.clone())
        .unwrap_or(current_source);
    if effective_source.trim().is_empty() {
        // `source_path_key` 非空且与 artwork_id 唯一，因此同一 Artwork 只能有一个分支留空。
        let conflicting: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM branches
                 WHERE artwork_id = ?1 AND id <> ?2 AND TRIM(source_path) = '')",
                params![artwork_id, branch_id],
                |row| row.get(0),
            )
            .map_err(storage::database_error)?;
        if conflicting {
            return Err("同一 Artwork 只能有一个分支不设置工作文件".into());
        }
    }
    // 没有工作文件就没有可读取的源，自动备份必须保持关闭：前端置灰只是提示，
    // 真正的约束在这里兜底，避免出现"开关是开的但永远不会备份"的状态。
    let effective_enabled = enabled && !effective_source.trim().is_empty();
    let changed = transaction
        .execute(
            "UPDATE branches SET title = ?2,
                    source_path = COALESCE(?7, source_path),
                    source_path_key = COALESCE(?8, source_path_key),
                    backup_enabled = CASE WHEN backup_enabled = ?3 THEN ?4 ELSE backup_enabled END,
                    backup_interval_minutes = ?5,
                    backup_quick_enabled = ?9,
                    consecutive_backup_failures = CASE
                      WHEN backup_enabled = 0 AND ?3 = 0 AND ?4 <> 0 THEN 0
                      ELSE consecutive_backup_failures END,
                    backup_retry_at_ms = CASE
                      WHEN backup_enabled = 0 AND ?3 = 0 AND ?4 <> 0 THEN NULL
                      ELSE backup_retry_at_ms END,
                    last_error = CASE
                      WHEN backup_enabled = 0 AND ?3 = 0 AND ?4 <> 0 THEN NULL
                      ELSE last_error END,
                    backup_disable_notice_pending = CASE
                      WHEN backup_enabled = 0 AND ?3 = 0 AND ?4 <> 0 THEN 0
                      ELSE backup_disable_notice_pending END,
                    updated_ms = ?6 WHERE id = ?1",
            params![
                branch_id,
                title.trim(),
                i64::from(expected_enabled),
                i64::from(effective_enabled),
                interval_minutes,
                storage::now_ms()?,
                normalized_source.as_ref().map(|value| &value.0),
                normalized_source.as_ref().map(|value| &value.1),
                i64::from(quick_enabled),
            ],
        )
        .map_err(|error| {
            if error
                .to_string()
                .contains("branches.artwork_id, branches.source_path_key")
            {
                "同一 Artwork 的每个分支必须使用不同的工作文件路径".into()
            } else {
                storage::database_error(error)
            }
        })?;
    if changed == 0 {
        Err("找不到分支".into())
    } else {
        transaction.commit().map_err(storage::database_error)
    }
}

pub(crate) fn load_branch(root: &Path, branch_id: &str) -> Result<BranchRecord, String> {
    let connection = storage::open(root)?;
    connection
        .query_row(
            "SELECT id, artwork_id, source_path, head_history_id FROM branches WHERE id = ?1",
            [branch_id],
            |row| {
                Ok(BranchRecord {
                    artwork_id: row.get(1)?,
                    source_path: row.get(2)?,
                    head_history_id: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "找不到分支".into())
}

pub(crate) fn load_node(root: &Path, history_id: &str) -> Result<HistoryRecord, String> {
    let connection = storage::open(root)?;
    load_node_from(&connection, history_id)
}

pub(crate) fn all_node_ids(root: &Path) -> Result<Vec<String>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare("SELECT id FROM history_nodes ORDER BY created_ms, id")
        .map_err(storage::database_error)?;
    let ids = statement
        .query_map([], |row| row.get(0))
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(ids)
}

fn load_node_from(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<HistoryRecord, String> {
    connection
        .query_row(
            "SELECT node.id, node.artwork_id, node.parent_id, node.sha256, node.snapshot_path,
                    COALESCE(edge.delta_path, node.delta_path)
             FROM history_nodes node
             LEFT JOIN history_edges edge ON edge.child_history_id = node.id
             WHERE node.id = ?1",
            [history_id],
            |row| {
                Ok(HistoryRecord {
                    id: row.get(0)?,
                    artwork_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    sha256: row.get(3)?,
                    snapshot_path: row.get(4)?,
                    delta_path: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "找不到历史节点".into())
}

pub(crate) fn materialization_chain(
    root: &Path,
    history_id: &str,
) -> Result<Vec<HistoryRecord>, String> {
    let connection = storage::open(root)?;
    let target = load_node_from(&connection, history_id)?;
    if target.snapshot_path.is_some() {
        return Ok(vec![target]);
    }
    let snapshot_id = connection
        .query_row(
            "WITH RECURSIVE descendants(id, depth) AS (
               SELECT id, 0 FROM history_nodes WHERE id = ?1
               UNION ALL
               SELECT child.id, descendants.depth + 1
               FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             )
             SELECT descendants.id FROM descendants
             JOIN history_nodes node ON node.id = descendants.id
             WHERE node.snapshot_path IS NOT NULL
             ORDER BY descendants.depth, node.created_ms, node.id LIMIT 1",
            [history_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("历史节点没有可用的后代 snapshot")?;
    let mut chain = Vec::new();
    let mut cursor = load_node_from(&connection, &snapshot_id)?;
    loop {
        let reached = cursor.id == history_id;
        let parent_id = cursor.parent_id.clone();
        chain.push(cursor);
        if reached {
            break;
        }
        cursor = load_node_from(
            &connection,
            parent_id
                .as_deref()
                .ok_or("snapshot 后代不在目标历史链上")?,
        )?;
    }
    Ok(chain)
}

/// 提交新历史节点并切换分支 head。
///
/// 提交成功后会释放父节点的 snapshot（父节点不再是 head 或检查点、且没有其它
/// 分支引用时）。该文件不再在提交后由调用方直接删除，而是在同一事务内入队
/// `pending_file_cleanup`（引用复查见 `cleanup::enqueue_released_repository_files`），
/// 由调用方在提交成功后重放；返回已入队的 cleanup id。
pub(crate) fn commit(root: &Path, commit: HistoryCommit<'_>) -> Result<Vec<String>, String> {
    storage::validate_title(commit.title, "历史节点标题")?;
    if commit.note.chars().count() > 500 {
        return Err("提交备注不能超过 500 个字符".into());
    }
    if !matches!(commit.commit_kind, "manual" | "automatic") {
        return Err("提交类型无效".into());
    }
    storage::validate_sha256(commit.sha256)?;
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let branch: Option<(String, Option<String>)> = transaction
        .query_row(
            "SELECT artwork_id, head_history_id FROM branches WHERE id = ?1",
            [commit.branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, current_head) = branch.ok_or("找不到分支")?;
    if current_head.as_deref() != commit.parent_id {
        return Err("分支 head 已变化，本次提交已取消".into());
    }
    let locked: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM final_artifacts WHERE branch_id = ?1)",
            [commit.branch_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if locked {
        return Err("分支已有最终成品，移除成品后才能继续提交".into());
    }
    transaction
        .execute(
            "INSERT INTO history_nodes
         (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind, created_ms,
          logical_size, chunk_file_size, sha256, chunk_count, snapshot_path, delta_path)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                commit.id,
                artwork_id,
                commit.branch_id,
                commit.parent_id,
                commit.title.trim(),
                commit.note.trim(),
                commit.commit_kind,
                commit.created_ms,
                i64::try_from(commit.logical_size).map_err(|_| "原文件大小超出范围")?,
                i64::try_from(commit.chunk_file_size).map_err(|_| "Chunk 文件大小超出范围")?,
                commit.sha256.to_ascii_uppercase(),
                i64::try_from(commit.chunk_count).map_err(|_| "块数量超出范围")?,
                commit.snapshot_path,
                Option::<String>::None
            ],
        )
        .map_err(storage::database_error)?;
    if let (Some(parent_id), Some(delta_path), Some(delta_size)) =
        (commit.parent_id, commit.delta_path, commit.delta_size)
    {
        transaction.execute(
            "INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
             VALUES (?1, ?2, ?3, ?4)",
            params![commit.id, parent_id, delta_path, i64::try_from(delta_size).map_err(|_| "delta 文件大小超出范围")?]
        ).map_err(storage::database_error)?;
    } else if commit.parent_id.is_some() {
        return Err("非根历史节点缺少反向 delta".into());
    }
    transaction
        .execute(
            "UPDATE branches SET head_history_id = ?2, last_check_ms = ?3, last_success_ms = ?3,
                last_error = NULL, consecutive_backup_failures = 0, backup_retry_at_ms = NULL,
                verify_error = NULL,
                updated_ms = ?3 WHERE id = ?1",
            params![commit.branch_id, commit.id, commit.created_ms],
        )
        .map_err(storage::database_error)?;
    let mut cleanup_ids = Vec::new();
    if let Some(parent_id) = commit.parent_id {
        let retained: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM branches WHERE head_history_id = ?1)
                        OR EXISTS(SELECT 1 FROM history_nodes WHERE id = ?1 AND is_checkpoint <> 0)",
                [parent_id],
                |row| row.get(0),
            )
            .map_err(storage::database_error)?;
        if !retained {
            let old_snapshot: Option<String> = transaction
                .query_row(
                    "SELECT snapshot_path FROM history_nodes WHERE id = ?1",
                    [parent_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage::database_error)?
                .flatten();
            // The reverse delta of the just-inserted child edge already exists
            // in the transaction, so the storage metadata can be recomputed in
            // the same statement that releases the snapshot. SQLite evaluates
            // CHECK constraints per statement, and the table requires
            // `snapshot_path IS NOT NULL OR delta_path IS NOT NULL`, so the
            // two columns must never be cleared separately.
            transaction
                .execute(
                    &format!(
                        "UPDATE history_nodes SET
                           snapshot_path = NULL,
                           {STORAGE_METADATA_SET}
                         WHERE id = ?1"
                    ),
                    [parent_id],
                )
                .map_err(storage::database_error)?;
            if let Some(relative) = old_snapshot {
                cleanup_ids = cleanup::enqueue_released_repository_files(
                    &transaction,
                    &[relative],
                    "history_commit_release",
                )?;
            }
        }
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

/// Atomic SET clause that derives `delta_path` and `chunk_file_size` from the
/// history graph: the reverse delta on the edge to the cheapest child. Every
/// statement that clears `snapshot_path` must apply this clause in the same
/// UPDATE, because the table CHECK requires
/// `snapshot_path IS NOT NULL OR delta_path IS NOT NULL`.
const STORAGE_METADATA_SET: &str = "delta_path = (SELECT edge.delta_path
                     FROM history_edges edge
                     JOIN history_nodes child ON child.id = edge.child_history_id
                     WHERE child.parent_id = history_nodes.id
                     ORDER BY edge.delta_size, edge.delta_path LIMIT 1),
           chunk_file_size = (SELECT MIN(edge.delta_size)
                              FROM history_edges edge
                              JOIN history_nodes child ON child.id = edge.child_history_id
                              WHERE child.parent_id = history_nodes.id)";

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
fn refresh_storage_metadata(
    connection: &rusqlite::Connection,
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

pub(crate) fn mark_unchanged(root: &Path, branch_id: &str, checked_ms: i64) -> Result<(), String> {
    storage::open(root)?.execute(
        "UPDATE branches SET last_check_ms = ?2, last_success_ms = ?2, last_error = NULL,
            consecutive_backup_failures = 0, backup_retry_at_ms = NULL, updated_ms = ?2 WHERE id = ?1",
        params![branch_id, checked_ms]
    ).map_err(storage::database_error)?;
    Ok(())
}

/// 读取上次全量检查成功后记录的工作文件元数据。
/// `last_source_size` 为 NULL 表示还没有可信任的基线，快速检查必须退回全量。
pub(crate) fn load_source_metadata(
    root: &Path,
    branch_id: &str,
) -> Result<Option<(i64, Option<i64>)>, String> {
    Ok(storage::open(root)?
        .query_row(
            "SELECT last_source_size, last_source_modified_ms FROM branches WHERE id = ?1",
            [branch_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?
        .and_then(|(size, modified)| size.map(|value| (value, modified))))
}

/// 在全量检查或提交成功后记录工作文件的大小与修改时间，作为快速检查的基线。
pub(crate) fn record_source_metadata(
    root: &Path,
    branch_id: &str,
    length: u64,
    modified_ms: Option<i64>,
) -> Result<(), String> {
    storage::open(root)?
        .execute(
            "UPDATE branches SET last_source_size = ?2, last_source_modified_ms = ?3 WHERE id = ?1",
            params![
                branch_id,
                i64::try_from(length).map_err(|_| "源文件大小超出范围")?,
                modified_ms
            ],
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
    let previous: Option<String> = transaction
        .query_row(
            "SELECT snapshot_path FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .flatten();
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

pub(crate) fn rename_node(root: &Path, history_id: &str, title: &str) -> Result<(), String> {
    storage::validate_title(title, "历史节点标题")?;
    if title.chars().count() > 500 {
        return Err("历史节点标题不能超过 500 个字符".into());
    }
    let changed = storage::open(root)?
        .execute(
            "UPDATE history_nodes SET title = ?2 WHERE id = ?1",
            params![history_id, title.trim()],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        Err("找不到历史节点".into())
    } else {
        Ok(())
    }
}

pub(crate) fn mark_checkpoint(root: &Path, history_id: &str) -> Result<(), String> {
    let changed = storage::open(root)?
        .execute(
            "UPDATE history_nodes SET is_checkpoint = 1 WHERE id = ?1",
            [history_id],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        Err("找不到历史节点".into())
    } else {
        Ok(())
    }
}

/// 取消一个普通检查点，释放其 snapshot 并切回唯一子节点的反向增量。
///
/// 返回 `Ok(None)` 表示该节点本就不是检查点（无操作）；返回 `Ok(Some(ids))`
/// 表示已释放，被释放的 snapshot 在同一事务内入队 `pending_file_cleanup`
/// （原因 `history_checkpoint_release`），由调用方在提交成功后重放。
pub(crate) fn unmark_checkpoint(
    root: &Path,
    history_id: &str,
) -> Result<Option<Vec<String>>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let forced: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM branches WHERE head_history_id = ?1 OR created_from_history_id = ?1)
                OR (SELECT COUNT(*) FROM history_nodes WHERE parent_id = ?1) > 1",
        [history_id], |row| row.get(0)
    ).map_err(storage::database_error)?;
    if forced {
        return Err("分支 head、分支起点或分叉点必须保留为检查点".into());
    }
    let value: Option<(bool, Option<String>)> = transaction
        .query_row(
            "SELECT node.is_checkpoint, node.snapshot_path
             FROM history_nodes node WHERE node.id = ?1",
            [history_id],
            |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (marked, snapshot_path) = value.ok_or("找不到历史节点")?;
    if !marked {
        return Ok(None);
    }
    let child_delta_available: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1
             FROM history_nodes child
             JOIN history_edges edge ON edge.child_history_id = child.id
             WHERE child.parent_id = ?1)",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    let alternative: bool = transaction.query_row(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM history_nodes WHERE parent_id = ?1
           UNION ALL SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
         ) SELECT EXISTS(SELECT 1 FROM history_nodes WHERE id IN (SELECT id FROM descendants) AND snapshot_path IS NOT NULL)",
        [history_id], |row| row.get(0)
    ).map_err(storage::database_error)?;
    if !alternative {
        return Err("该检查点是恢复祖先历史所需的唯一 snapshot，不能取消".into());
    }
    if !child_delta_available {
        return Err("取消检查点时找不到唯一子节点的反向增量".into());
    }
    transaction
        .execute(
            &format!(
                "UPDATE history_nodes SET
                   is_checkpoint = 0,
                   snapshot_path = NULL,
                   {STORAGE_METADATA_SET}
                 WHERE id = ?1"
            ),
            [history_id],
        )
        .map_err(storage::database_error)?;
    // The released snapshot is replaced by the child edge reverse delta, which
    // the same statement above already recorded on this node.
    let stored: Option<String> = transaction
        .query_row(
            "SELECT delta_path FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if stored.is_none() {
        return Err("取消检查点后无法登记增量存储".into());
    }
    let cleanup_ids = match snapshot_path {
        Some(relative) => cleanup::enqueue_released_repository_files(
            &transaction,
            &[relative],
            "history_checkpoint_release",
        )?,
        None => Vec::new(),
    };
    transaction.commit().map_err(storage::database_error)?;
    Ok(Some(cleanup_ids))
}

pub(crate) fn compaction_target(root: &Path, history_id: &str) -> Result<CompactionTarget, String> {
    let connection = storage::open(root)?;
    let row: Option<(String, Option<String>, String, Option<String>, bool, i64)> = connection
        .query_row(
            "SELECT node.artwork_id, node.parent_id, child.id,
                    node.snapshot_path, node.is_checkpoint,
                    (SELECT COUNT(*) FROM history_nodes child_count WHERE child_count.parent_id = node.id)
             FROM history_nodes node
             JOIN history_nodes child ON child.parent_id = node.id
             WHERE node.id = ?1
             AND NOT EXISTS (SELECT 1 FROM history_nodes sibling WHERE sibling.parent_id = node.id AND sibling.id <> child.id)
             AND NOT EXISTS (SELECT 1 FROM branches WHERE head_history_id = node.id OR created_from_history_id = node.id)
             AND NOT EXISTS (SELECT 1 FROM final_artifacts WHERE history_id = node.id)",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get::<_, i64>(4)? != 0, row.get(5)?)),
        ).optional().map_err(storage::database_error)?;
    let (artwork_id, parent_id, child_id, _snapshot, checkpoint, count) =
        row.ok_or("该节点不是可精简的中间节点")?;
    if parent_id.is_none() || checkpoint || count != 1 {
        return Err("只能精简有唯一子节点、且不是分支关键点或检查点的中间节点".into());
    }
    Ok(CompactionTarget {
        artwork_id,
        node_id: history_id.into(),
        parent_id: parent_id.unwrap(),
        child_id,
    })
}

/// 改接精简后的历史链：把 `target` 的父节点直接连到 `target` 的子节点，删除被
/// 移除的节点与旧边，并登记新的反向 delta。
///
/// 被移除节点与旧边占用的仓库文件不再由调用方在提交后直接删除：在同一事务内
/// 完成引用复查后入队 `pending_file_cleanup`（原因 `history_compaction`），由调用方
/// 在提交成功后重放；返回已入队的 cleanup id。新 delta 已被本事务的边引用，
/// 因此不会被入队。
pub(crate) fn apply_compaction(
    root: &Path,
    target: &CompactionTarget,
    delta_path: &str,
    delta_size: u64,
) -> Result<Vec<String>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let valid: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM history_nodes WHERE id = ?1 AND parent_id = ?2)
         AND EXISTS(SELECT 1 FROM history_nodes WHERE id = ?3 AND parent_id = ?1)",
            params![target.node_id, target.parent_id, target.child_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !valid {
        return Err("历史结构在精简期间发生变化".into());
    }
    let mut paths = Vec::new();
    let mut statement = transaction
        .prepare(
            "SELECT snapshot_path FROM history_nodes WHERE id = ?1
         UNION ALL SELECT delta_path FROM history_edges WHERE child_history_id = ?1
         UNION ALL SELECT delta_path FROM history_nodes WHERE id = ?1 AND delta_path IS NOT NULL
         UNION ALL SELECT delta_path FROM history_edges WHERE child_history_id = ?2",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map(params![target.node_id, target.child_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(storage::database_error)?;
    for row in rows {
        if let Some(path) = row.map_err(storage::database_error)? {
            paths.push(path);
        }
    }
    drop(statement);
    transaction
        .execute(
            "UPDATE history_nodes SET parent_id = ?2 WHERE id = ?1",
            params![target.child_id, target.parent_id],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "DELETE FROM history_edges WHERE child_history_id = ?1",
            [&target.child_id],
        )
        .map_err(storage::database_error)?;
    transaction.execute(
        "INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size) VALUES (?1, ?2, ?3, ?4)",
        params![target.child_id, target.parent_id, delta_path, i64::try_from(delta_size).map_err(|_| "精简 delta 大小超出范围")?]
    ).map_err(storage::database_error)?;
    transaction
        .execute("DELETE FROM history_nodes WHERE id = ?1", [&target.node_id])
        .map_err(storage::database_error)?;
    // The new reverse delta reconstructs the removed node's parent from its
    // child, so the parent now owns the refreshed storage metadata. The child is
    // refreshed as well because earlier builds wrote the delta size onto it.
    refresh_storage_metadata(&transaction, &target.parent_id)?;
    refresh_storage_metadata(&transaction, &target.child_id)?;
    let cleanup_ids =
        cleanup::enqueue_released_repository_files(&transaction, &paths, "history_compaction")?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn delete_subtree(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<HistoryDeletion, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork_id: String = transaction
        .query_row(
            "SELECT artwork_id FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("找不到历史节点")?;
    let branch_valid: bool = transaction
        .query_row(
            "WITH RECURSIVE ancestors(id, parent_id) AS (
               SELECT node.id, node.parent_id
               FROM branches branch
               JOIN history_nodes node ON node.id = branch.head_history_id
               WHERE branch.id = ?2 AND branch.artwork_id = ?3
               UNION ALL
               SELECT parent.id, parent.parent_id
               FROM history_nodes parent JOIN ancestors ON parent.id = ancestors.parent_id
             )
             SELECT EXISTS(SELECT 1 FROM ancestors WHERE id = ?1)",
            params![history_id, branch_id, artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !branch_valid {
        return Err("只能从当前分支的历史链删除节点".into());
    }
    if subtree_contains_publication(&transaction, history_id)? {
        return Err("无法删除历史：这段历史包含已发布节点，发布记录必须保留可恢复基线".into());
    }
    transaction
        .execute_batch("CREATE TEMP TABLE IF NOT EXISTS history_delete (id TEXT PRIMARY KEY);")
        .map_err(storage::database_error)?;
    transaction
        .execute("DELETE FROM history_delete", [])
        .map_err(storage::database_error)?;
    transaction.execute(
        "INSERT INTO history_delete WITH RECURSIVE descendants(id) AS (
             SELECT id FROM history_nodes WHERE id = ?1
             UNION ALL SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
         ) SELECT id FROM descendants", [history_id]
    ).map_err(storage::database_error)?;
    let conflict: Option<String> = transaction
        .query_row(
            "WITH RECURSIVE descendants(id) AS (
                 SELECT id FROM history_nodes WHERE id = ?1
                 UNION ALL SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             )
             SELECT b.title FROM branches b JOIN descendants d
               ON b.head_history_id = d.id OR b.created_from_history_id = d.id
             WHERE b.id <> ?2
             LIMIT 1",
            params![history_id, branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if let Some(title) = conflict {
        return Err(format!(
            "无法删除历史：分支“{title}”仍然指向这段历史，请先删除该分支"
        ));
    }
    let mut paths = HashSet::new();
    let mut statement = transaction.prepare(
        "SELECT snapshot_path FROM history_nodes WHERE id IN (SELECT id FROM history_delete)
         UNION ALL SELECT delta_path FROM history_edges WHERE child_history_id IN (SELECT id FROM history_delete)
         UNION ALL SELECT delta_path FROM history_nodes WHERE id IN (SELECT id FROM history_delete) AND delta_path IS NOT NULL"
    ).map_err(storage::database_error)?;
    for row in statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(storage::database_error)?
    {
        if let Some(path) = row.map_err(storage::database_error)? {
            paths.insert(path);
        }
    }
    drop(statement);
    let fallback: Option<String> = transaction
        .query_row(
            "SELECT parent_id FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .flatten();
    let mut branches = Vec::new();
    let mut branch_statement = transaction.prepare("SELECT id, head_history_id, created_from_history_id FROM branches WHERE artwork_id = ?1").map_err(storage::database_error)?;
    for row in branch_statement
        .query_map([&artwork_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(storage::database_error)?
    {
        branches.push(row.map_err(storage::database_error)?);
    }
    drop(branch_statement);
    for (branch_id, head, origin) in branches {
        let origin_deleted = match origin.as_deref() {
            Some(id) => transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history_delete WHERE id = ?1)",
                    [id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage::database_error)?,
            None => false,
        };
        let Some(head) = head else {
            if origin_deleted {
                transaction.execute(
                    "UPDATE branches SET created_from_history_id = ?2, updated_ms = ?3 WHERE id = ?1",
                    params![branch_id, fallback, storage::now_ms()?],
                ).map_err(storage::database_error)?;
            }
            continue;
        };
        let head_deleted = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM history_delete WHERE id = ?1)",
                [&head],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage::database_error)?;
        if !head_deleted && !origin_deleted {
            continue;
        }
        let mut cursor = head;
        loop {
            let deleted = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history_delete WHERE id = ?1)",
                    [&cursor],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage::database_error)?;
            if !deleted {
                break;
            }
            let parent: Option<String> = transaction
                .query_row(
                    "SELECT parent_id FROM history_nodes WHERE id = ?1",
                    [&cursor],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage::database_error)?
                .flatten();
            let Some(parent) = parent else {
                cursor.clear();
                break;
            };
            cursor = parent;
        }
        transaction.execute(
            "UPDATE branches SET head_history_id = NULLIF(?2, ''),
                    created_from_history_id = CASE WHEN ?4 <> 0 THEN ?5 ELSE created_from_history_id END,
                    verify_error = NULL,
                    updated_ms = ?3 WHERE id = ?1",
            params![branch_id, cursor, storage::now_ms()?, i64::from(origin_deleted), fallback]
        ).map_err(storage::database_error)?;
    }
    transaction
        .execute(
            "DELETE FROM history_nodes WHERE id IN (SELECT id FROM history_delete)",
            [],
        )
        .map_err(storage::database_error)?;
    // 被删除节点与边占用的仓库文件在同一事务内复查引用后入队；仍被其它分支或
    // 节点引用的路径会被跳过，由调用方在提交成功后重放清理。
    let cleanup_ids = cleanup::enqueue_released_repository_files(
        &transaction,
        &paths.into_iter().collect::<Vec<_>>(),
        "history_subtree_deletion",
    )?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(HistoryDeletion {
        artwork_id,
        cleanup_ids,
    })
}

pub(crate) fn validate_subtree_deletion(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<(), String> {
    let connection = storage::open(root)?;
    let artwork_id: String = connection
        .query_row(
            "SELECT artwork_id FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("找不到历史节点")?;
    let branch_valid: bool = connection
        .query_row(
            "WITH RECURSIVE ancestors(id, parent_id) AS (
               SELECT node.id, node.parent_id
               FROM branches branch
               JOIN history_nodes node ON node.id = branch.head_history_id
               WHERE branch.id = ?2 AND branch.artwork_id = ?3
               UNION ALL
               SELECT parent.id, parent.parent_id
               FROM history_nodes parent JOIN ancestors ON parent.id = ancestors.parent_id
             )
             SELECT EXISTS(SELECT 1 FROM ancestors WHERE id = ?1)",
            params![history_id, branch_id, artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !branch_valid {
        return Err("只能从当前分支的历史链删除节点".into());
    }
    if subtree_contains_publication(&connection, history_id)? {
        return Err("无法删除历史：这段历史包含已发布节点，发布记录必须保留可恢复基线".into());
    }
    let conflict: Option<String> = connection
        .query_row(
            "WITH RECURSIVE descendants(id) AS (
               SELECT id FROM history_nodes WHERE id = ?1
               UNION ALL
               SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             )
             SELECT branch.title
             FROM branches branch JOIN descendants
               ON branch.head_history_id = descendants.id OR branch.created_from_history_id = descendants.id
             WHERE branch.id <> ?2
             LIMIT 1",
            params![history_id, branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if let Some(title) = conflict {
        return Err(format!(
            "无法删除历史：分支“{title}”仍然指向这段历史，请先删除该分支"
        ));
    }
    Ok(())
}

fn subtree_contains_publication(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "WITH RECURSIVE descendants(id) AS (
               SELECT id FROM history_nodes WHERE id = ?1
               UNION ALL
               SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             )
             SELECT EXISTS(
               SELECT 1 FROM final_artifacts artifact JOIN descendants ON artifact.history_id = descendants.id
             )",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

/// 删除一个非主分支，回收只属于它的历史节点。
///
/// 删除前收集该分支节点的 `snapshot_path`、旧兼容 `delta_path` 与 `history_edges.delta_path`
/// 作为候选；事务提交前复查当前图是否仍引用这些路径（事务可见本事务的删除结果），
/// 只把已无引用的文件入队 `pending_file_cleanup`（原因 `history_branch_deletion`），
/// 由调用方在提交成功后重放。仍被共享祖先或其它分支引用的文件保留。
pub(crate) fn delete_branch(root: &Path, branch_id: &str) -> Result<BranchDeletion, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let branch: Option<(String, Option<String>, bool)> = transaction
        .query_row(
            "SELECT b.artwork_id, b.created_from_history_id,
                    EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)
             FROM branches b WHERE b.id = ?1",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, origin, locked) = branch.ok_or("找不到分支")?;
    if origin.is_none() {
        return Err("主分支不能删除".into());
    }
    if locked {
        return Err("该分支已有最终成品，请先移除成品后再删除分支".into());
    }

    let mut paths = HashSet::new();
    let mut owned = Vec::new();
    let mut statement = transaction
        .prepare(
            "SELECT id, snapshot_path, delta_path FROM history_nodes
             WHERE created_on_branch_id = ?1 ORDER BY created_ms DESC, id DESC",
        )
        .map_err(storage::database_error)?;
    for row in statement
        .query_map([branch_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(storage::database_error)?
    {
        let row = row.map_err(storage::database_error)?;
        if let Some(path) = row.1 {
            paths.insert(path);
        }
        if let Some(path) = row.2 {
            paths.insert(path);
        }
        owned.push(row.0);
    }
    drop(statement);
    let mut edge_statement = transaction
        .prepare(
            "SELECT edge.delta_path
             FROM history_edges edge
             JOIN history_nodes child ON child.id = edge.child_history_id
             WHERE child.created_on_branch_id = ?1",
        )
        .map_err(storage::database_error)?;
    for path in edge_statement
        .query_map([branch_id], |row| row.get::<_, String>(0))
        .map_err(storage::database_error)?
    {
        paths.insert(path.map_err(storage::database_error)?);
    }
    drop(edge_statement);
    loop {
        let mut candidate = None;
        for id in &owned {
            let deletable = transaction
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM history_nodes child WHERE child.parent_id = ?1)
                            AND NOT EXISTS(
                              SELECT 1 FROM branches other
                              WHERE other.id <> ?2
                                AND (other.head_history_id = ?1 OR other.created_from_history_id = ?1)
                            )",
                    params![id.as_str(), branch_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage::database_error)?;
            if deletable {
                candidate = Some(id.clone());
                break;
            }
        }
        let Some(candidate) = candidate else {
            break;
        };
        transaction
            .execute(
                "DELETE FROM history_nodes WHERE id = ?1",
                [candidate.as_str()],
            )
            .map_err(storage::database_error)?;
        owned.retain(|id| id != &candidate);
    }
    if !owned.is_empty() {
        let replacement: String = transaction
            .query_row(
                "SELECT id FROM branches WHERE artwork_id = ?1 AND id <> ?2 ORDER BY created_ms, id LIMIT 1",
                params![artwork_id, branch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage::database_error)?
            .ok_or("分支仍有共享历史，且找不到可接管这些节点的分支")?;
        transaction
            .execute(
                "UPDATE history_nodes SET created_on_branch_id = ?2 WHERE created_on_branch_id = ?1",
                params![branch_id, replacement],
            )
            .map_err(storage::database_error)?;
    }
    transaction
        .execute("DELETE FROM branches WHERE id = ?1", [branch_id])
        .map_err(storage::database_error)?;
    let cleanup_ids = cleanup::enqueue_released_repository_files(
        &transaction,
        &paths.into_iter().collect::<Vec<_>>(),
        "history_branch_deletion",
    )?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(BranchDeletion {
        artwork_id,
        cleanup_ids,
    })
}

pub(crate) fn mark_automatic_backup_error(
    root: &Path,
    branch_id: &str,
    error: &str,
    failed_ms: i64,
) -> Result<bool, String> {
    const MAX_CONSECUTIVE_FAILURES: u32 = 5;
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let (previous_failures, interval_minutes): (u32, u32) = transaction
        .query_row(
            "SELECT consecutive_backup_failures, backup_interval_minutes FROM branches WHERE id = ?1",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage::database_error)?;
    let failures = previous_failures.saturating_add(1);
    let disabled = failures >= MAX_CONSECUTIVE_FAILURES;
    let retry_minutes = match failures {
        1 => 1,
        2 => interval_minutes.div_ceil(4),
        3 => interval_minutes.div_ceil(2),
        _ => interval_minutes,
    };
    let retry_at_ms = (!disabled)
        .then(|| failed_ms.saturating_add(i64::from(retry_minutes).saturating_mul(60_000)));
    transaction
        .execute(
            "UPDATE branches
             SET last_error = ?2, consecutive_backup_failures = ?3,
                 backup_retry_at_ms = ?4,
                 backup_enabled = CASE WHEN ?5 <> 0 THEN 0 ELSE backup_enabled END,
                 backup_disable_notice_pending = CASE WHEN ?5 <> 0 THEN 1 ELSE backup_disable_notice_pending END,
                 updated_ms = ?6
             WHERE id = ?1",
            params![branch_id, error, failures, retry_at_ms, i64::from(disabled), failed_ms],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(disabled)
}

pub(crate) fn mark_error(root: &Path, branch_id: &str, error: &str) {
    if let Ok(connection) = storage::open(root) {
        let _ = connection.execute(
            "UPDATE branches SET last_error = ?2 WHERE id = ?1",
            params![branch_id, error],
        );
    }
}

pub(crate) fn acknowledge_backup_disable_notices(
    root: &Path,
    artwork_ids: &[String],
) -> Result<(), String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    for artwork_id in artwork_ids {
        transaction
            .execute(
                "UPDATE branches SET backup_disable_notice_pending = 0
             WHERE artwork_id = ?1 AND backup_disable_notice_pending <> 0",
                [artwork_id],
            )
            .map_err(storage::database_error)?;
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(())
}

pub(crate) fn next_backup_disable_notice_target(
    root: &Path,
) -> Result<Option<BackupDisableNoticeTarget>, String> {
    storage::open(root)?
        .query_row(
            "SELECT b.artwork_id, b.id
             FROM branches b
             JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.backup_disable_notice_pending <> 0 AND n.trashed_ms IS NULL
             ORDER BY b.updated_ms DESC, b.id
             LIMIT 1",
            [],
            |row| {
                Ok(BackupDisableNoticeTarget {
                    artwork_id: row.get(0)?,
                    branch_id: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)
}

pub(crate) fn list_scheduled(root: &Path) -> Result<Vec<ScheduledBranch>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT b.id, b.last_check_ms, b.backup_interval_minutes, b.backup_quick_enabled, b.backup_retry_at_ms
         FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
         WHERE b.backup_enabled <> 0 AND n.trashed_ms IS NULL
           AND TRIM(b.source_path) <> ''
           AND NOT EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)
         ORDER BY b.id",
        )
        .map_err(storage::database_error)?;
    let branches = statement
        .query_map([], |row| {
            Ok(ScheduledBranch {
                id: row.get(0)?,
                last_check_ms: row.get(1)?,
                interval_minutes: row.get(2)?,
                quick_enabled: row.get::<_, i64>(3)? != 0,
                retry_at_ms: row.get(4)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(branches)
}

pub(crate) fn load_scheduled(
    root: &Path,
    branch_id: &str,
) -> Result<Option<ScheduledBranch>, String> {
    storage::open(root)?
        .query_row(
            "SELECT b.id, b.last_check_ms, b.backup_interval_minutes, b.backup_quick_enabled, b.backup_retry_at_ms
             FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.id = ?1 AND b.backup_enabled <> 0 AND n.trashed_ms IS NULL
               AND TRIM(b.source_path) <> ''
               AND NOT EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)",
            [branch_id],
            |row| {
                Ok(ScheduledBranch {
                    id: row.get(0)?,
                    last_check_ms: row.get(1)?,
                    interval_minutes: row.get(2)?,
                    quick_enabled: row.get::<_, i64>(3)? != 0,
                    retry_at_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)
}

/// 空闲校验的派生队列：所有未进回收站、head 非空且尚未校验到当前 head 的分支。
///
/// 与 [`list_scheduled`] 是两条不同查询：本查询不要求 `backup_enabled`（手动提交的
/// 分支同样需要校验），也不排除已发布分支（其 head 是强制检查点，校验成本低）。
/// `verify_error` 非空的分支暂不重试，直到 head 变化或用户手动重查清空该列。
///
/// NULL 语义：`head_history_id` 允许为空（分支已建但未提交），因此条件显式写成
/// `head IS NOT NULL AND (verified IS NULL OR verified != head)`，避免把无 head 的
/// 分支纳入队列并每轮空跑。
pub(crate) fn list_idle_verify_targets(root: &Path) -> Result<Vec<IdleVerifyTarget>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT b.id, b.head_history_id, node.created_ms
             FROM branches b
             JOIN library_nodes n ON n.id = b.artwork_id
             JOIN history_nodes node ON node.id = b.head_history_id
             WHERE n.trashed_ms IS NULL
               AND b.head_history_id IS NOT NULL
               AND (b.verified_history_id IS NULL OR b.verified_history_id != b.head_history_id)
               AND b.verify_error IS NULL
             ORDER BY node.created_ms, b.id",
        )
        .map_err(storage::database_error)?;
    let targets = statement
        .query_map([], |row| {
            Ok(IdleVerifyTarget {
                branch_id: row.get(0)?,
                head_history_id: row.get(1)?,
                head_created_ms: row.get(2)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(targets)
}

/// 读取单条仍待校验的候选；调度器在取得运行锁后用它复查分支是否还是队列快照。
pub(crate) fn load_idle_verify_target(
    root: &Path,
    branch_id: &str,
) -> Result<Option<IdleVerifyTarget>, String> {
    storage::open(root)?
        .query_row(
            "SELECT b.id, b.head_history_id, node.created_ms
             FROM branches b
             JOIN library_nodes n ON n.id = b.artwork_id
             JOIN history_nodes node ON node.id = b.head_history_id
             WHERE b.id = ?1 AND n.trashed_ms IS NULL
               AND b.head_history_id IS NOT NULL
               AND (b.verified_history_id IS NULL OR b.verified_history_id != b.head_history_id)
               AND b.verify_error IS NULL",
            [branch_id],
            |row| {
                Ok(IdleVerifyTarget {
                    branch_id: row.get(0)?,
                    head_history_id: row.get(1)?,
                    head_created_ms: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)
}

/// 记录一次成功的校验。只有 head 仍等于被校验的节点时才写入，避免把结果记到校验
/// 期间已经变化的新 head 上。返回是否真正写入。
pub(crate) fn mark_verified(
    root: &Path,
    branch_id: &str,
    history_id: &str,
    verified_ms: i64,
) -> Result<bool, String> {
    let changed = storage::open(root)?
        .execute(
            "UPDATE branches SET verified_history_id = ?2, verified_ms = ?3, verify_error = NULL
             WHERE id = ?1 AND head_history_id = ?2",
            params![branch_id, history_id, verified_ms],
        )
        .map_err(storage::database_error)?;
    Ok(changed > 0)
}

/// 记录一次失败的校验摘要；分支随之退出派生队列，直到 head 变化或用户手动重查。
/// 与 [`mark_verified`] 相同，只有 head 未变时才写入。
pub(crate) fn mark_verify_error(
    root: &Path,
    branch_id: &str,
    history_id: &str,
    error: &str,
) -> Result<bool, String> {
    let changed = storage::open(root)?
        .execute(
            "UPDATE branches SET verify_error = ?3 WHERE id = ?1 AND head_history_id = ?2",
            params![branch_id, history_id, error],
        )
        .map_err(storage::database_error)?;
    Ok(changed > 0)
}

/// 清除校验失败摘要，使分支重新进入派生队列（"重新校验此分支"）。
pub(crate) fn clear_verify_error(root: &Path, branch_id: &str) -> Result<(), String> {
    let connection = storage::open(root)?;
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM branches WHERE id = ?1)",
            [branch_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !exists {
        return Err("找不到分支".into());
    }
    connection
        .execute(
            "UPDATE branches SET verify_error = NULL WHERE id = ?1",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

pub(crate) fn count_scheduled_files(root: &Path) -> Result<usize, String> {
    let count: i64 = storage::open(root)?
        .query_row(
            "SELECT COUNT(DISTINCT b.source_path_key)
             FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.backup_enabled <> 0 AND n.trashed_ms IS NULL
               AND TRIM(b.source_path) <> ''
               AND NOT EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)",
            [],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    usize::try_from(count).map_err(|_| "自动备份文件数量无效".into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct HistoryFixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
        artwork_id: String,
        main_branch_id: String,
        fork_source: PathBuf,
    }

    impl HistoryFixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("repository");
            let main_source = directory.path().join("main.psd");
            let fork_source = directory.path().join("fork.psd");
            fs::write(&main_source, b"main").unwrap();
            fs::write(&fork_source, b"fork").unwrap();
            crate::library::initialize(&root).unwrap();
            let artwork =
                crate::library::create_artwork(&root, None, "Artwork", "Main", &main_source)
                    .unwrap();
            Self {
                _directory: directory,
                root,
                artwork_id: artwork.artwork_id,
                main_branch_id: artwork.branch_id,
                fork_source,
            }
        }

        fn commit_node(&self, branch_id: &str, id: &str, parent_id: Option<&str>, created_ms: i64) {
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
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn scheduled_file_count_tracks_enabled_branches() {
        let fixture = HistoryFixture::new();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 1);

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            false,
            10,
            false,
            None,
        )
        .unwrap();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 0);
    }

    #[test]
    fn branch_source_path_update_is_validated_and_atomic() {
        let fixture = HistoryFixture::new();
        let replacement = fixture._directory.path().join("replacement.psd");
        fs::write(&replacement, b"replacement").unwrap();

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            true,
            10,
            false,
            replacement.to_str(),
        )
        .unwrap();
        assert_eq!(
            load_branch(&fixture.root, &fixture.main_branch_id)
                .unwrap()
                .source_path,
            storage::display_path(&replacement.canonicalize().unwrap())
        );

        let missing = fixture._directory.path().join("missing.psd");
        assert!(update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Should not persist",
            true,
            true,
            10,
            false,
            missing.to_str(),
        )
        .is_err());
        let preserved = list(&fixture.root, &fixture.artwork_id).unwrap();
        assert_eq!(preserved.branches[0].title, "Main");
        assert_eq!(
            preserved.branches[0].source_path,
            storage::display_path(&replacement.canonicalize().unwrap())
        );
    }

    #[test]
    fn clearing_the_source_path_also_turns_automatic_backup_off() {
        let fixture = HistoryFixture::new();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 1);

        // 空白字符串表示清除工作文件路径，同时必须把自动备份写回关闭。
        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            true,
            10,
            false,
            Some("   "),
        )
        .unwrap();

        let branch = list(&fixture.root, &fixture.artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.source_path, "");
        assert!(!branch.backup_enabled);
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 0);

        // 空路径分支不能保留自动备份：显式请求打开也只落库为关闭。
        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            false,
            true,
            10,
            false,
            None,
        )
        .unwrap();
        assert!(!list(&fixture.root, &fixture.artwork_id).unwrap().branches[0].backup_enabled);
    }

    #[test]
    fn only_one_branch_per_artwork_may_clear_its_source_path() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        let side_branch = create_branch(
            &fixture.root,
            &fixture.artwork_id,
            "root",
            "Side",
            &fixture.fork_source,
        )
        .unwrap();

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            true,
            10,
            false,
            Some(""),
        )
        .unwrap();
        assert!(update_branch(
            &fixture.root,
            &side_branch,
            "Should not persist",
            true,
            true,
            10,
            false,
            Some(""),
        )
        .is_err());

        // 拒绝时事务整体回滚：标题与路径都不受影响。
        let history = list(&fixture.root, &fixture.artwork_id).unwrap();
        let side = history
            .branches
            .iter()
            .find(|branch| branch.id == side_branch)
            .unwrap();
        assert_eq!(side.title, "Side");
        assert!(!side.source_path.is_empty());
    }

    #[test]
    fn automatic_backup_failures_follow_interval_and_eventually_disable() {
        let fixture = HistoryFixture::new();
        mark_unchanged(&fixture.root, &fixture.main_branch_id, 100).unwrap();
        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            true,
            120,
            false,
            None,
        )
        .unwrap();

        let expected_delays = [1_i64, 30, 60, 120];
        for (index, delay_minutes) in expected_delays.into_iter().enumerate() {
            let failed_ms = 1_000 + index as i64;
            assert!(!mark_automatic_backup_error(
                &fixture.root,
                &fixture.main_branch_id,
                "temporary failure",
                failed_ms,
            )
            .unwrap());
            let retry_at: i64 = storage::open(&fixture.root)
                .unwrap()
                .query_row(
                    "SELECT backup_retry_at_ms FROM branches WHERE id = ?1",
                    [&fixture.main_branch_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retry_at, failed_ms + delay_minutes * 60_000);
        }
        assert!(mark_automatic_backup_error(
            &fixture.root,
            &fixture.main_branch_id,
            "persistent failure",
            2_000,
        )
        .unwrap());

        let state: (Option<i64>, Option<i64>, Option<String>, u32, bool, bool) =
            storage::open(&fixture.root)
                .unwrap()
                .query_row(
                    "SELECT last_check_ms, last_success_ms, last_error,
                        consecutive_backup_failures, backup_enabled,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                    [&fixture.main_branch_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .unwrap();
        assert_eq!(state.0, Some(100));
        assert_eq!(state.1, Some(100));
        assert_eq!(state.2.as_deref(), Some("persistent failure"));
        assert_eq!(state.3, 5);
        assert!(!state.4);
        assert!(state.5);
        assert_eq!(
            next_backup_disable_notice_target(&fixture.root).unwrap(),
            Some(BackupDisableNoticeTarget {
                artwork_id: fixture.artwork_id.clone(),
                branch_id: fixture.main_branch_id.clone(),
            })
        );

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Renamed while stale",
            true,
            true,
            60,
            false,
            None,
        )
        .unwrap();
        let stale_update: (String, bool, u32, bool) = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT title, backup_enabled, consecutive_backup_failures,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(stale_update.0, "Renamed while stale");
        assert!(!stale_update.1);
        assert_eq!(stale_update.2, 5);
        assert!(stale_update.3);

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Renamed while stale",
            false,
            true,
            60,
            false,
            None,
        )
        .unwrap();
        let reenabled: (bool, u32, bool) = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT backup_enabled, consecutive_backup_failures,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(reenabled.0);
        assert_eq!(reenabled.1, 0);
        assert!(!reenabled.2);

        acknowledge_backup_disable_notices(&fixture.root, &[fixture.artwork_id.clone()]).unwrap();
        let pending: bool = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT backup_disable_notice_pending FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!pending);
        assert_eq!(
            next_backup_disable_notice_target(&fixture.root).unwrap(),
            None
        );
    }

    fn queued_paths(root: &Path) -> Vec<String> {
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

    #[test]
    fn branch_deletion_enqueues_released_edge_delta_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let main_source = directory.path().join("main.psd");
        let fork_source = directory.path().join("fork.psd");
        fs::File::create(&main_source)
            .unwrap()
            .write_all(b"main")
            .unwrap();
        fs::File::create(&fork_source)
            .unwrap()
            .write_all(b"fork")
            .unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &main_source).unwrap();
        let root_node = "root-node";
        commit(
            &root,
            HistoryCommit {
                id: root_node,
                branch_id: &artwork.branch_id,
                parent_id: None,
                title: "Root",
                note: "",
                commit_kind: "manual",
                created_ms: 1,
                logical_size: 1,
                chunk_file_size: 1,
                sha256: &"A".repeat(64),
                chunk_count: 1,
                snapshot_path: "artworks/root.snapshot",
                delta_path: None,
                delta_size: None,
            },
        )
        .unwrap();
        let branch_id =
            create_branch(&root, &artwork.artwork_id, root_node, "Fork", &fork_source).unwrap();
        commit(
            &root,
            HistoryCommit {
                id: "fork-node",
                branch_id: &branch_id,
                parent_id: Some(root_node),
                title: "Fork commit",
                note: "",
                commit_kind: "manual",
                created_ms: 2,
                logical_size: 1,
                chunk_file_size: 1,
                sha256: &"B".repeat(64),
                chunk_count: 1,
                snapshot_path: "artworks/fork.snapshot",
                delta_path: Some("artworks/fork.delta"),
                delta_size: Some(1),
            },
        )
        .unwrap();

        let deletion = delete_branch(&root, &branch_id).unwrap();

        // 只属于该分支的 snapshot 与边 delta 在事务内入队；共享的祖先 snapshot
        // 仍被主分支引用，因此不入队。删除本身由调用方在提交成功后重放。
        assert!(!deletion.cleanup_ids.is_empty());
        let queued = queued_paths(&root);
        assert!(queued.iter().any(|path| path == "artworks/fork.delta"));
        assert!(queued.iter().any(|path| path == "artworks/fork.snapshot"));
        assert!(!queued.iter().any(|path| path == "artworks/root.snapshot"));

        let report = crate::cleanup::run(&root, &deletion.cleanup_ids).unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.pending_count, 0);
    }

    #[test]
    fn subtree_preflight_rejects_published_history() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        fixture.commit_node(&fixture.main_branch_id, "published", Some("root"), 2);
        storage::open(&fixture.root)
            .unwrap()
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms)
                 VALUES ('artifact', ?1, 'published', 'artworks/final.jpg', ?2, 'image/jpeg', 1, 3)",
                params![fixture.main_branch_id, "A".repeat(64)],
            )
            .unwrap();

        let error = validate_subtree_deletion(&fixture.root, "published", &fixture.main_branch_id)
            .unwrap_err();

        assert!(error.contains("已发布节点"), "{error}");
    }

    #[test]
    fn subtree_deletion_does_not_update_unaffected_branches() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        fixture.commit_node(&fixture.main_branch_id, "cut", Some("root"), 2);
        fixture.commit_node(&fixture.main_branch_id, "main-head", Some("cut"), 3);
        let fork_branch = create_branch(
            &fixture.root,
            &fixture.artwork_id,
            "root",
            "Fork",
            &fixture.fork_source,
        )
        .unwrap();
        fixture.commit_node(&fork_branch, "fork-head", Some("root"), 4);
        storage::open(&fixture.root)
            .unwrap()
            .execute(
                "UPDATE branches SET updated_ms = 777 WHERE id = ?1",
                [&fork_branch],
            )
            .unwrap();

        delete_subtree(&fixture.root, "cut", &fixture.main_branch_id).unwrap();

        let connection = storage::open(&fixture.root).unwrap();
        let main_head: Option<String> = connection
            .query_row(
                "SELECT head_history_id FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| row.get(0),
            )
            .unwrap();
        let fork_state: (Option<String>, i64) = connection
            .query_row(
                "SELECT head_history_id, updated_ms FROM branches WHERE id = ?1",
                [&fork_branch],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(main_head.as_deref(), Some("root"));
        assert_eq!(fork_state.0.as_deref(), Some("fork-head"));
        assert_eq!(fork_state.1, 777);
    }

    #[test]
    fn subtree_deletion_propagates_branch_row_errors() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        fixture.commit_node(&fixture.main_branch_id, "cut", Some("root"), 2);
        let fork_branch = create_branch(
            &fixture.root,
            &fixture.artwork_id,
            "root",
            "Fork",
            &fixture.fork_source,
        )
        .unwrap();
        let connection = storage::open(&fixture.root).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TRIGGER fork_origin_update_matches_artwork;",
            )
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET created_from_history_id = X'80' WHERE id = ?1",
                [&fork_branch],
            )
            .unwrap();
        drop(connection);

        let error = delete_subtree(&fixture.root, "cut", &fixture.main_branch_id).unwrap_err();

        assert!(error.contains("数据库操作失败"), "{error}");
    }

    #[test]
    fn compaction_atomically_rewires_child_and_edge() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        fixture.commit_node(&fixture.main_branch_id, "middle", Some("root"), 2);
        fixture.commit_node(&fixture.main_branch_id, "child", Some("middle"), 3);
        let target = compaction_target(&fixture.root, "middle").unwrap();

        let cleanup_ids =
            apply_compaction(&fixture.root, &target, "artworks/child-to-root.delta", 9).unwrap();

        let connection = storage::open(&fixture.root).unwrap();
        let child: (Option<String>, Option<String>, i64) = connection
            .query_row(
                "SELECT parent_id, delta_path, chunk_file_size FROM history_nodes WHERE id = 'child'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let parent: (String, i64) = connection
            .query_row(
                "SELECT delta_path, chunk_file_size FROM history_nodes WHERE id = 'root'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let edge: (String, String, i64) = connection
            .query_row(
                "SELECT parent_history_id, delta_path, delta_size
                 FROM history_edges WHERE child_history_id = 'child'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let middle_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM history_nodes WHERE id = 'middle')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(child.0.as_deref(), Some("root"));
        // The child owns a snapshot, so compaction must not replace its storage
        // metadata with the new reverse delta that reconstructs its parent.
        assert_eq!(child.1, None);
        assert_eq!(child.2, 1);
        // The removed node's parent is reconstructed from the new delta.
        assert_eq!(parent, ("artworks/child-to-root.delta".into(), 9));
        assert_eq!(
            edge,
            ("root".into(), "artworks/child-to-root.delta".into(), 9)
        );
        assert!(!middle_exists);
        // 被移除节点与旧边占用的文件在事务内入队，等待调用方提交后重放；
        // 新 delta 已被本事务的边引用，因此不入队。
        assert!(!cleanup_ids.is_empty());
        let queued = queued_paths(&fixture.root);
        assert!(queued
            .iter()
            .any(|path| path == "artworks/middle-to-root.delta"));
        assert!(queued
            .iter()
            .any(|path| path == "artworks/child-to-middle.delta"));
        assert!(queued.iter().any(|path| path == "artworks/middle.snapshot"));
        assert!(!queued
            .iter()
            .any(|path| path == "artworks/child-to-root.delta"));
    }

    #[test]
    fn compaction_refreshes_storage_metadata_without_snapshots() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        fixture.commit_node(&fixture.main_branch_id, "middle", Some("root"), 2);
        fixture.commit_node(&fixture.main_branch_id, "child", Some("middle"), 3);
        // Compact the older pair first so both the parent and the child lose
        // their snapshots, which is the state that previously kept stale sizes.
        let inner = compaction_target(&fixture.root, "middle").unwrap();
        apply_compaction(&fixture.root, &inner, "artworks/child-to-root.delta", 9).unwrap();
        fixture.commit_node(&fixture.main_branch_id, "head", Some("child"), 4);
        let target = compaction_target(&fixture.root, "child").unwrap();

        apply_compaction(&fixture.root, &target, "artworks/head-to-root.delta", 21).unwrap();

        let connection = storage::open(&fixture.root).unwrap();
        let rows = {
            let mut statement = connection
                .prepare("SELECT id, delta_path, chunk_file_size FROM history_nodes ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(
            rows,
            vec![
                // The head still owns a snapshot, so it keeps its size and the
                // snapshot-less legacy delta path stays empty.
                ("head".into(), None, 1),
                (
                    "root".into(),
                    Some("artworks/head-to-root.delta".into()),
                    21
                ),
            ]
        );
    }
}
