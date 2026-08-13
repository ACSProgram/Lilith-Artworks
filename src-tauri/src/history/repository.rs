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
                    EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)
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
                final_artifact_locked: row.get::<_, bool>(10)?,
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
            "UPDATE branches SET title = ?2, backup_enabled = ?3,
                    backup_interval_minutes = ?4, updated_ms = ?5 WHERE id = ?1",
            params![
                branch_id,
                title.trim(),
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

fn load_node_from(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<HistoryRecord, String> {
    connection
        .query_row(
            "SELECT node.id, node.artwork_id, node.parent_id, node.sha256, node.snapshot_path,
                    COALESCE(edge.delta_path, node.delta_path), node.is_checkpoint
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
                    is_checkpoint: row.get::<_, i64>(6)? != 0,
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
                last_error = NULL, updated_ms = ?3 WHERE id = ?1",
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
        "UPDATE branches SET last_check_ms = ?2, last_success_ms = ?2, last_error = NULL, updated_ms = ?2 WHERE id = ?1",
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
    let delta_size: i64 = transaction
        .query_row(
            "SELECT COALESCE((SELECT delta_size FROM history_edges WHERE child_history_id = ?1), chunk_file_size) FROM history_nodes WHERE id = ?1",
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
    transaction
        .execute(
            "UPDATE history_nodes SET is_checkpoint = 0, snapshot_path = NULL, chunk_file_size = ?2 WHERE id = ?1",
            rusqlite::params![history_id, delta_size],
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
             AND NOT EXISTS (SELECT 1 FROM branches WHERE head_history_id = node.id OR created_from_history_id = node.id)",
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
    for path in rows.flatten().flatten() {
        paths.push(path);
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

pub(crate) fn delete_subtree(root: &Path, history_id: &str) -> Result<HistoryDeletion, String> {
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
             LIMIT 1",
            [history_id],
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
    for path in statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(storage::database_error)?
        .flatten()
        .flatten()
    {
        paths.insert(path);
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
        .flatten()
    {
        branches.push(row);
    }
    drop(branch_statement);
    for (branch_id, head, origin) in branches {
        let origin_deleted = origin.as_deref().is_some_and(|id| {
            transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM history_delete WHERE id = ?1)",
                    [id],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });
        let Some(head) = head else {
            if origin_deleted {
                transaction.execute(
                    "UPDATE branches SET created_from_history_id = ?2, updated_ms = ?3 WHERE id = ?1",
                    params![branch_id, fallback, storage::now_ms()?],
                ).map_err(storage::database_error)?;
            }
            continue;
        };
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
    let deleted_count: u64 = transaction
        .query_row("SELECT COUNT(*) FROM history_delete", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(storage::database_error)?
        .try_into()
        .map_err(|_| "删除节点数量无效")?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(HistoryDeletion {
        artwork_id,
        deleted_count,
        storage_paths: paths.into_iter().collect(),
    })
}

pub(crate) fn delete_branch(root: &Path, branch_id: &str) -> Result<BranchDeletion, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let branch: Option<(String, String, Option<String>, bool)> = transaction
        .query_row(
            "SELECT b.artwork_id, b.title, b.created_from_history_id,
                    EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)
             FROM branches b WHERE b.id = ?1",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, branch_title, origin, locked) = branch.ok_or("找不到分支")?;
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
        .flatten()
    {
        if let Some(path) = row.1 {
            paths.insert(path);
        }
        if let Some(path) = row.2 {
            paths.insert(path);
        }
        owned.push(row.0);
    }
    drop(statement);
    let mut deleted_count = 0_u64;
    loop {
        let candidate = owned.iter().find(|id| {
            transaction
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
                .unwrap_or(false)
        });
        let Some(candidate) = candidate.cloned() else {
            break;
        };
        transaction
            .execute(
                "DELETE FROM history_nodes WHERE id = ?1",
                [candidate.as_str()],
            )
            .map_err(storage::database_error)?;
        owned.retain(|id| id != &candidate);
        deleted_count = deleted_count.saturating_add(1);
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
        branch_title,
        deleted_count,
        storage_paths: paths.into_iter().collect(),
    })
}

pub(crate) fn mark_error(root: &Path, branch_id: &str, error: &str) {
    if let Ok(connection) = storage::open(root) {
        let _ = connection.execute(
            "UPDATE branches SET last_check_ms = ?2, last_error = ?3 WHERE id = ?1",
            params![branch_id, storage::now_ms().unwrap_or_default(), error],
        );
    }
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
            "SELECT b.id, b.last_check_ms, b.backup_interval_minutes
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
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(branches)
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
