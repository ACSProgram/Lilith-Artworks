use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, OptionalExtension};

use crate::storage;

use super::{
    ArtworkBranch, ArtworkHistory, BranchDeletion, BranchRecord, CompactionTarget, HistoryCommit,
    HistoryDeletion, HistoryNode, HistoryRecord, ScheduledBranch,
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
                    (SELECT COUNT(*) FROM certification_records record WHERE record.branch_id = b.id)
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
) -> Result<(), String> {
    storage::validate_title(title, "分支标题")?;
    if !(1..=10_080).contains(&interval_minutes) {
        return Err("自动备份间隔必须在 1 到 10080 分钟之间".into());
    }
    let connection = storage::open(root)?;
    let changed = connection
        .execute(
            "UPDATE branches SET title = ?2,
                    backup_enabled = CASE WHEN backup_enabled = ?3 THEN ?4 ELSE backup_enabled END,
                    backup_interval_minutes = ?5,
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
                i64::from(enabled),
                interval_minutes,
                storage::now_ms()?
            ],
        )
        .map_err(storage::database_error)?;
    if changed == 0 {
        Err("找不到分支".into())
    } else {
        Ok(())
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

pub(crate) fn commit(root: &Path, commit: HistoryCommit<'_>) -> Result<Option<String>, String> {
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
                updated_ms = ?3 WHERE id = ?1",
            params![commit.branch_id, commit.id, commit.created_ms],
        )
        .map_err(storage::database_error)?;
    let mut old_snapshot = None;
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
            old_snapshot = transaction
                .query_row(
                    "SELECT snapshot_path FROM history_nodes WHERE id = ?1",
                    [parent_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage::database_error)?
                .flatten();
            transaction.execute(
                "UPDATE history_nodes
                 SET snapshot_path = NULL,
                     delta_path = COALESCE(delta_path, (SELECT delta_path FROM history_edges WHERE child_history_id = ?2)),
                     chunk_file_size = COALESCE((SELECT delta_size FROM history_edges WHERE child_history_id = ?2), chunk_file_size)
                 WHERE id = ?1",
                params![parent_id, commit.id]
            ).map_err(storage::database_error)?;
        }
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(old_snapshot)
}

pub(crate) fn mark_unchanged(root: &Path, branch_id: &str, checked_ms: i64) -> Result<(), String> {
    storage::open(root)?.execute(
        "UPDATE branches SET last_check_ms = ?2, last_success_ms = ?2, last_error = NULL,
            consecutive_backup_failures = 0, backup_retry_at_ms = NULL, updated_ms = ?2 WHERE id = ?1",
        params![branch_id, checked_ms]
    ).map_err(storage::database_error)?;
    Ok(())
}

pub(crate) fn set_snapshot(
    root: &Path,
    history_id: &str,
    relative_path: &str,
    file_size: u64,
    checkpoint: bool,
) -> Result<(), String> {
    let connection = storage::open(root)?;
    let changed = connection
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
        Err("找不到历史节点".into())
    } else {
        Ok(())
    }
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

pub(crate) fn unmark_checkpoint(root: &Path, history_id: &str) -> Result<Option<String>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let forced: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM branches WHERE head_history_id = ?1 OR created_from_history_id = ?1)
                OR (SELECT COUNT(*) FROM history_nodes WHERE parent_id = ?1) > 1",
        [history_id], |row| row.get(0)
    ).map_err(storage::database_error)?;
    if forced {
        return Err("分支 head、fork 起点或分叉节点必须保留为检查点".into());
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
    let child_delta: Option<(String, i64)> = transaction
        .query_row(
            "SELECT edge.delta_path, edge.delta_size
             FROM history_nodes child
             JOIN history_edges edge ON edge.child_history_id = child.id
             WHERE child.parent_id = ?1",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
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
    let (delta_path, delta_size) = child_delta.ok_or("取消检查点时找不到唯一子节点的反向增量")?;
    transaction
        .execute(
            "UPDATE history_nodes
             SET is_checkpoint = 0, snapshot_path = NULL, delta_path = ?2, chunk_file_size = ?3
             WHERE id = ?1",
            rusqlite::params![history_id, delta_path, delta_size],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(snapshot_path)
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
        .execute(
            "UPDATE history_nodes SET delta_path = ?2, chunk_file_size = ?3 WHERE id = ?1",
            params![
                target.child_id,
                delta_path,
                i64::try_from(delta_size).map_err(|_| "精简 delta 大小超出范围")?
            ],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute("DELETE FROM history_nodes WHERE id = ?1", [&target.node_id])
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(paths)
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
    transaction.commit().map_err(storage::database_error)?;
    Ok(HistoryDeletion {
        artwork_id,
        storage_paths: paths.into_iter().collect(),
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
    transaction.commit().map_err(storage::database_error)?;
    Ok(BranchDeletion {
        artwork_id,
        storage_paths: paths.into_iter().collect(),
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

pub(crate) fn storage_path_referenced(root: &Path, path: &str) -> Result<bool, String> {
    storage::open(root)?
        .query_row(
            "SELECT EXISTS(
           SELECT 1 FROM history_nodes WHERE snapshot_path = ?1 OR delta_path = ?1
           UNION ALL SELECT 1 FROM history_edges WHERE delta_path = ?1
         )",
            [path],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

pub(crate) fn list_scheduled(root: &Path) -> Result<Vec<ScheduledBranch>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT b.id, b.last_check_ms, b.backup_interval_minutes, b.backup_retry_at_ms
         FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
         WHERE b.backup_enabled <> 0 AND n.trashed_ms IS NULL
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
                retry_at_ms: row.get(3)?,
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
            "SELECT b.id, b.last_check_ms, b.backup_interval_minutes, b.backup_retry_at_ms
             FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.id = ?1 AND b.backup_enabled <> 0 AND n.trashed_ms IS NULL
               AND NOT EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)",
            [branch_id],
            |row| {
                Ok(ScheduledBranch {
                    id: row.get(0)?,
                    last_check_ms: row.get(1)?,
                    interval_minutes: row.get(2)?,
                    retry_at_ms: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)
}

pub(crate) fn count_scheduled_files(root: &Path) -> Result<usize, String> {
    let count: i64 = storage::open(root)?
        .query_row(
            "SELECT COUNT(DISTINCT b.source_path_key)
             FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.backup_enabled <> 0 AND n.trashed_ms IS NULL
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
        )
        .unwrap();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 0);
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

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Renamed while stale",
            true,
            true,
            60,
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
    }

    #[test]
    fn branch_deletion_collects_edge_delta_paths() {
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

        assert!(deletion
            .storage_paths
            .iter()
            .any(|path| path == "artworks/fork.delta"));
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

        let old_paths =
            apply_compaction(&fixture.root, &target, "artworks/child-to-root.delta", 9).unwrap();

        let connection = storage::open(&fixture.root).unwrap();
        let child: (Option<String>, Option<String>, i64) = connection
            .query_row(
                "SELECT parent_id, delta_path, chunk_file_size FROM history_nodes WHERE id = 'child'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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
        assert_eq!(child.1.as_deref(), Some("artworks/child-to-root.delta"));
        assert_eq!(child.2, 9);
        assert_eq!(
            edge,
            ("root".into(), "artworks/child-to-root.delta".into(), 9)
        );
        assert!(!middle_exists);
        assert!(old_paths
            .iter()
            .any(|path| path == "artworks/middle-to-root.delta"));
        assert!(old_paths
            .iter()
            .any(|path| path == "artworks/child-to-middle.delta"));
    }
}
