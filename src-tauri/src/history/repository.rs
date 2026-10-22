//! 历史图的主仓储：分支 CRUD、提交事务、链元数据、存储路径、缩略图与精简。
//!
//! 这里负责图结构与 SQLite 事务，不读取 ChunkFile；破坏性删除见
//! `deletion_repository`，校验状态见 `verification_repository`，调度状态见
//! `schedule_repository`。

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, OptionalExtension};

use crate::{cleanup, storage};

use super::verification_repository::{
    clear_artwork_verification, derive_branch_verification, node_verification_states,
};
use super::{
    classify_verify_failure, ArtworkBranch, ArtworkHistory, BranchRecord, CheckMode,
    CompactionTarget, HistoryCommit, HistoryNode, HistoryRecord, NodeVerifyIssue,
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
                    b.backup_quick_enabled,
                    b.last_check_elapsed_ms, b.last_check_mode
             FROM branches b WHERE b.artwork_id = ?1 ORDER BY b.created_ms, b.id",
        )
        .map_err(storage::database_error)?;
    let mut branches = branch_statement
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
                last_check_elapsed_ms: row.get(16)?,
                last_check_mode: row
                    .get::<_, Option<String>>(17)?
                    .as_deref()
                    .and_then(CheckMode::from_db),
                last_error: row.get(9)?,
                consecutive_backup_failures: row.get(10)?,
                backup_retry_at_ms: row.get(11)?,
                backup_disable_notice_pending: row.get::<_, i64>(12)? != 0,
                final_artifact_locked: row.get::<_, bool>(13)?,
                published_count: row.get(14)?,
                backup_quick_enabled: row.get::<_, i64>(15)? != 0,
                verify_error: None,
                verify_failure_count: 0,
                verify_unverifiable_count: 0,
                verify_issue: None,
                verified_ms: None,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(branch_statement);

    // 分支的校验状态由整条历史链派生：任一节点失败即失败；全部节点都有
    // verified_ms 时取最旧的通过时间，任一节点未验证则为 NULL。
    let verification = node_verification_states(&connection, artwork_id)?;
    for branch in &mut branches {
        let derived = derive_branch_verification(branch.head_history_id.as_deref(), &verification);
        branch.verify_error = derived.error;
        branch.verify_failure_count = derived.failure_count;
        branch.verify_unverifiable_count = derived.unverifiable_count;
        branch.verify_issue = derived.issue;
        branch.verified_ms = derived.verified_ms;
    }

    let mut node_statement = connection
        .prepare(
            "SELECT node.id, node.created_on_branch_id, node.parent_id, node.title, node.note, node.commit_kind,
                    (node.is_checkpoint <> 0
                      OR EXISTS(SELECT 1 FROM branches b WHERE b.head_history_id = node.id OR b.created_from_history_id = node.id)
                      OR (SELECT COUNT(*) FROM history_nodes child WHERE child.parent_id = node.id) > 1),
                    node.created_ms, node.logical_size, node.chunk_file_size, node.sha256, node.chunk_count,
                    node.verified_ms, node.verify_error
             FROM history_nodes node WHERE node.artwork_id = ?1 ORDER BY node.created_ms, node.id",
        )
        .map_err(storage::database_error)?;
    let nodes = node_statement
        .query_map([artwork_id], |row| {
            let verify_error: Option<String> = row.get(13)?;
            // 失败归类从已有文案派生，不单独建列：写入与读取共用同一个标记常量。
            let verify_issue: Option<NodeVerifyIssue> =
                verify_error.as_deref().map(classify_verify_failure);
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
                verified_ms: row.get(12)?,
                verify_error,
                verify_issue,
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
    backup_quick_enabled: bool,
) -> Result<String, String> {
    storage::validate_title(title, "分支标题")?;
    let (source_display, source_key) = storage::normalize_source_path(root, source_path)?;
    let mut connection = storage::open(root)?;
    // 分支行与起点 checkpoint 标记必须同进同退：只插入分支而没固化起点，会让
    // 新分支依赖一个可能在后续提交中释放 snapshot 的父节点。
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let origin_matches: bool = transaction
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
    transaction
        .execute(
            "INSERT INTO branches
             (id, artwork_id, title, source_path, source_path_key, head_history_id,
              created_from_history_id, backup_quick_enabled, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?8)",
            params![
                id,
                artwork_id,
                title.trim(),
                source_display,
                source_key,
                from_history_id,
                i64::from(backup_quick_enabled),
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
    transaction
        .execute(
            "UPDATE history_nodes SET is_checkpoint = 1 WHERE id = ?1",
            [from_history_id],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
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

/// 节点当前存储文件的仓库内相对路径；只读图元数据，不打开文件。
///
/// 口径与 `chunk_file_size` 一致：节点持有 snapshot 时取快照，否则取重建该节点
/// 所读的子边增量（多个子节点时取最小的一条），最后回退兼容列 `delta_path`。
pub(crate) fn node_storage_path(root: &Path, history_id: &str) -> Result<Option<String>, String> {
    Ok(storage::open(root)?
        .query_row(
            "SELECT COALESCE(
                        NULLIF(TRIM(node.snapshot_path), ''),
                        (SELECT NULLIF(TRIM(edge.delta_path), '')
                           FROM history_edges edge
                           JOIN history_nodes child ON child.id = edge.child_history_id
                          WHERE child.parent_id = node.id
                          ORDER BY edge.delta_size, edge.child_history_id
                          LIMIT 1),
                        NULLIF(TRIM(node.delta_path), '')
                    )
               FROM history_nodes node
              WHERE node.id = ?1",
            [history_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .flatten())
}

/// 节点所属分支的当前工作文件路径；分支未设置工作文件时返回 `None`。
///
/// 历史节点自身只保存 ChunkFile 容器，读不出原始格式，因此「格式是否支持缩略图」
/// 按这条路径的扩展名判断（见 `preview::supports_preview`）。分支事后改指到另一种
/// 格式的文件时，老节点会跟着按新扩展名回答。
pub(crate) fn node_source_path(root: &Path, history_id: &str) -> Result<Option<String>, String> {
    Ok(storage::open(root)?
        .query_row(
            "SELECT NULLIF(TRIM(branch.source_path), '')
               FROM history_nodes node
               JOIN branches branch ON branch.id = node.created_on_branch_id
              WHERE node.id = ?1",
            [history_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .flatten())
}

/// 节点是否以检查点 snapshot 作为**永久表示**。
///
/// 检查点的 snapshot 只有显式取消才会释放，因此子边的反向增量不会被任何读取
/// 路径消费（物化从这份 snapshot 起步），提交与精简都不必写它；取消检查点时再
/// 按需重建。分支 head 的 snapshot 会在下一次提交时释放，不属于这一类——那时
/// 子边增量就是节点的唯一表示，必须照常写入。
pub(crate) fn checkpoint_snapshot_holder(root: &Path, history_id: &str) -> Result<bool, String> {
    checkpoint_snapshot_holder_in(&storage::open(root)?, history_id)
}

/// 同一判定的事务内版本：提交在事务里读它，与登记空边在同一可见性下。
fn checkpoint_snapshot_holder_in(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM history_nodes
                            WHERE id = ?1 AND is_checkpoint = 1 AND snapshot_path IS NOT NULL)",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

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

/// 读取单个历史节点。
///
/// `delta_path` 为 `None` 表示该节点没有可用的反向增量：检查点父节点名下的边
/// 只登记空路径（它永远不会被读取，取消检查点时才重建），根节点则可能完全没有边。
fn load_node_from(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<HistoryRecord, String> {
    connection
        .query_row(
            "SELECT node.id, node.artwork_id, node.parent_id, node.sha256, node.snapshot_path,
                    NULLIF(TRIM(COALESCE(edge.delta_path, node.delta_path)), '')
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

/// Returns the chain from the nearest descendant snapshot down to `history_id`
/// (the snapshot itself when the target holds one). A single recursive CTE
/// resolves the snapshot pick and the parent walk, so deep chains no longer
/// issue one SQL query per node.
pub(crate) fn materialization_chain(
    root: &Path,
    history_id: &str,
) -> Result<Vec<HistoryRecord>, String> {
    let connection = storage::open(root)?;
    let target = load_node_from(&connection, history_id)?;
    if target.snapshot_path.is_some() {
        return Ok(vec![target]);
    }
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE
             descendants(id, depth) AS (
               SELECT id, 0 FROM history_nodes WHERE id = ?1
               UNION ALL
               SELECT child.id, descendants.depth + 1
               FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             ),
             snapshot_pick(id) AS (
               SELECT descendants.id FROM descendants
               JOIN history_nodes node ON node.id = descendants.id
               WHERE node.snapshot_path IS NOT NULL
               ORDER BY descendants.depth, node.created_ms, node.id LIMIT 1
             ),
             chain(id, artwork_id, parent_id, sha256, snapshot_path, delta_path, depth) AS (
               SELECT node.id, node.artwork_id, node.parent_id, node.sha256,
                      node.snapshot_path, COALESCE(edge.delta_path, node.delta_path), 0
               FROM history_nodes node
               LEFT JOIN history_edges edge ON edge.child_history_id = node.id
               JOIN snapshot_pick ON snapshot_pick.id = node.id
               UNION ALL
               SELECT parent.id, parent.artwork_id, parent.parent_id, parent.sha256,
                      parent.snapshot_path,
                      COALESCE(edge.delta_path, parent.delta_path), chain.depth + 1
               FROM history_nodes parent
               LEFT JOIN history_edges edge ON edge.child_history_id = parent.id
               JOIN chain ON parent.id = chain.parent_id
               WHERE chain.id != ?1
             )
             SELECT id, artwork_id, parent_id, sha256, snapshot_path, delta_path
             FROM chain ORDER BY depth",
        )
        .map_err(storage::database_error)?;
    let chain = statement
        .query_map([history_id], |row| {
            Ok(HistoryRecord {
                id: row.get(0)?,
                artwork_id: row.get(1)?,
                parent_id: row.get(2)?,
                sha256: row.get(3)?,
                snapshot_path: row.get(4)?,
                delta_path: row.get(5)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    if chain.is_empty() {
        return Err("历史节点没有可用的后代 snapshot".into());
    }
    if chain.last().map_or(true, |node| node.id != history_id) {
        return Err("snapshot 后代不在目标历史链上".into());
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
          logical_size, chunk_file_size, sha256, chunk_count, snapshot_path, delta_path,
          thumbnail, thumbnail_version)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                Option::<String>::None,
                commit.thumbnail,
                commit.thumbnail_version
            ],
        )
        .map_err(storage::database_error)?;
    if let Some(parent_id) = commit.parent_id {
        // 检查点父节点以 snapshot 作永久表示，子边增量不会被读取；此时登记一条
        // 空边，保持「每个非根节点都有边记录」的图不变式，取消检查点时再填回
        // 真实路径。其余情况下缺少增量就意味着父节点失去唯一表示，必须拒绝。
        let parent_holds_checkpoint = checkpoint_snapshot_holder_in(&transaction, parent_id)?;
        match (commit.delta_path, commit.delta_size) {
            (Some(delta_path), Some(delta_size)) => {
                transaction.execute(
                    "INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![commit.id, parent_id, delta_path, i64::try_from(delta_size).map_err(|_| "delta 文件大小超出范围")?]
                ).map_err(storage::database_error)?;
            }
            (None, None) if parent_holds_checkpoint => {
                transaction
                    .execute(
                        "INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
                         VALUES (?1, ?2, '', 0)",
                        params![commit.id, parent_id],
                    )
                    .map_err(storage::database_error)?;
            }
            _ => return Err("非根历史节点缺少反向 delta".into()),
        }
    }
    transaction
        .execute(
            "UPDATE branches SET head_history_id = ?2, last_check_ms = ?3, last_success_ms = ?3,
                last_error = NULL, consecutive_backup_failures = 0, backup_retry_at_ms = NULL,
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
    // 无头测试的强杀标记点：事务已 BEGIN、INSERT 已写、尚未 COMMIT。仅存在于
    // headless 构建（发布产物中不存在），GUI 与领域逻辑逐位不变；无头入口没有开
    // 闸门时是空操作。取消时直接返回错误，事务随 `Transaction` 析构被整体丢弃。
    #[cfg(feature = "headless")]
    if crate::headless::commit_marker() {
        return Err("提交已取消".into());
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
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork_id: Option<String> = transaction
        .query_row(
            "SELECT artwork_id FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    let Some(artwork_id) = artwork_id else {
        return Err("找不到历史节点".into());
    };
    transaction
        .execute(
            "UPDATE history_nodes SET is_checkpoint = 1 WHERE id = ?1",
            [history_id],
        )
        .map_err(storage::database_error)?;
    // 检查点改变链的物化与释放语义，与下面两个入口一样按作品粗粒度失效。
    clear_artwork_verification(&transaction, &artwork_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(())
}

/// 取消检查点前需要的信息：唯一子节点与子边反向增量的现状。
pub(crate) struct CheckpointReleasePlan {
    pub(crate) child_id: String,
    /// 子边已有的反向增量（仓库内相对路径、字节数）；为空表示必须现场重建。
    pub(crate) existing_delta: Option<(String, u64)>,
}

/// 只读：判断一个检查点能否取消，以及取消前是否必须重建反向增量。
///
/// 返回 `Ok(None)` 表示该节点本就不是检查点（无操作）。不能取消的情况与
/// [`unmark_checkpoint`] 完全一致：分支 head、分支起点与分叉点必须保留检查点，
/// 该检查点是恢复祖先历史所需的唯一 snapshot 时也不能取消。
pub(crate) fn plan_checkpoint_release(
    root: &Path,
    history_id: &str,
) -> Result<Option<CheckpointReleasePlan>, String> {
    let connection = storage::open(root)?;
    ensure_checkpoint_releasable(&connection, history_id)?;
    let marked: Option<bool> = connection
        .query_row(
            "SELECT is_checkpoint FROM history_nodes WHERE id = ?1",
            [history_id],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if !marked.ok_or("找不到历史节点")? {
        return Ok(None);
    }
    let child: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT child.id, edge.delta_path, edge.delta_size
             FROM history_nodes child
             JOIN history_edges edge ON edge.child_history_id = child.id
             WHERE child.parent_id = ?1",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let Some((child_id, delta_path, delta_size)) = child else {
        return Err("取消检查点时找不到唯一子节点的反向增量".into());
    };
    // 记录里的路径可能已经被垃圾扫描删掉（那时边上的路径还没有清空），以磁盘为准：
    // 文件不在就按「需要重建」处理。
    let delta_path = delta_path.trim().to_owned();
    let present = !delta_path.is_empty()
        && storage::resolve_path(root, &delta_path)
            .map(|path| path.is_file())
            .unwrap_or(false);
    Ok(Some(CheckpointReleasePlan {
        child_id,
        existing_delta: present
            .then(|| (delta_path, u64::try_from(delta_size).unwrap_or_default())),
    }))
}

/// 检查点能否取消的共用判定：被分支或分叉钉住的检查点必须保留，且取消后仍然
/// 要有可用的后代 snapshot，否则这个节点从此无法物化。
fn ensure_checkpoint_releasable(
    connection: &rusqlite::Connection,
    history_id: &str,
) -> Result<(), String> {
    let forced: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM branches WHERE head_history_id = ?1 OR created_from_history_id = ?1)
                OR (SELECT COUNT(*) FROM history_nodes WHERE parent_id = ?1) > 1",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if forced {
        return Err("分支 head、分支起点或分叉点必须保留为检查点".into());
    }
    let alternative: bool = connection
        .query_row(
            "WITH RECURSIVE descendants(id) AS (
               SELECT id FROM history_nodes WHERE parent_id = ?1
               UNION ALL SELECT child.id FROM history_nodes child JOIN descendants ON child.parent_id = descendants.id
             ) SELECT EXISTS(SELECT 1 FROM history_nodes WHERE id IN (SELECT id FROM descendants) AND snapshot_path IS NOT NULL)",
            [history_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !alternative {
        return Err("该检查点是恢复祖先历史所需的唯一 snapshot，不能取消".into());
    }
    Ok(())
}

/// 取消一个普通检查点，释放其 snapshot 并切回唯一子节点的反向增量。
///
/// `rebuilt_delta` 是调用方在事务外现场重建的增量（仓库内相对路径与字节数）。
/// 提交路径不再为检查点父节点写增量，所以取消时子边通常是空的，必须由
/// `backup::restore::build_reverse_delta` 重新算出一条再进事务；子边已经持有
/// 增量（旧仓库）时传 `None`，直接沿用。
///
/// 返回 `Ok(None)` 表示该节点本就不是检查点（无操作）；返回 `Ok(Some(ids))`
/// 表示已释放，被释放的 snapshot 在同一事务内入队 `pending_file_cleanup`
/// （原因 `history_checkpoint_release`），由调用方在提交成功后重放。
pub(crate) fn unmark_checkpoint(
    root: &Path,
    history_id: &str,
    rebuilt_delta: Option<(&str, u64)>,
) -> Result<Option<Vec<String>>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    ensure_checkpoint_releasable(&transaction, history_id)?;
    let value: Option<(bool, Option<String>, String)> = transaction
        .query_row(
            "SELECT node.is_checkpoint, node.snapshot_path, node.artwork_id
             FROM history_nodes node WHERE node.id = ?1",
            [history_id],
            |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (marked, snapshot_path, artwork_id) = value.ok_or("找不到历史节点")?;
    if !marked {
        return Ok(None);
    }
    let child: Option<(String, String)> = transaction
        .query_row(
            "SELECT child.id, edge.delta_path
             FROM history_nodes child
             JOIN history_edges edge ON edge.child_history_id = child.id
             WHERE child.parent_id = ?1",
            [history_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let Some((child_id, edge_delta_path)) = child else {
        return Err("取消检查点时找不到唯一子节点的反向增量".into());
    };
    let mut cleanup_ids = Vec::new();
    match rebuilt_delta {
        Some((relative, size)) => {
            transaction
                .execute(
                    "UPDATE history_edges SET delta_path = ?2, delta_size = ?3
                     WHERE child_history_id = ?1",
                    params![
                        child_id,
                        relative,
                        i64::try_from(size).map_err(|_| "增量大小超出范围")?
                    ],
                )
                .map_err(storage::database_error)?;
            // 旧路径不再被这条边引用：磁盘上若还留着另一个名字的旧增量，交给清理
            // 账本。同名路径必须排除——重建就落在同一个文件名上，入队会在提交后把
            // 刚写出的增量删掉；路径为空或文件已不存在时也不会入队。
            let previous = edge_delta_path.trim();
            if !previous.is_empty() && previous != relative {
                cleanup_ids.extend(cleanup::enqueue_released_repository_files(
                    &transaction,
                    std::slice::from_ref(&edge_delta_path),
                    "history_checkpoint_delta_replaced",
                )?);
            }
        }
        None if edge_delta_path.trim().is_empty() => {
            return Err("取消检查点时找不到唯一子节点的反向增量".into());
        }
        None => {}
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
    // the statements above already recorded on this node.
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
    if let Some(relative) = snapshot_path {
        cleanup_ids.extend(cleanup::enqueue_released_repository_files(
            &transaction,
            &[relative],
            "history_checkpoint_release",
        )?);
    }
    // 取消检查点把 snapshot 换回反向增量，按作品粗粒度失效校验状态。
    clear_artwork_verification(&transaction, &artwork_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(Some(cleanup_ids))
}

pub(crate) fn compaction_target(root: &Path, history_id: &str) -> Result<CompactionTarget, String> {
    let connection = storage::open(root)?;
    let row: Option<(String, Option<String>, String, bool, i64)> = connection
        .query_row(
            "SELECT node.artwork_id, node.parent_id, child.id, node.is_checkpoint,
                    (SELECT COUNT(*) FROM history_nodes child_count WHERE child_count.parent_id = node.id)
             FROM history_nodes node
             JOIN history_nodes child ON child.parent_id = node.id
             WHERE node.id = ?1
             AND NOT EXISTS (SELECT 1 FROM history_nodes sibling WHERE sibling.parent_id = node.id AND sibling.id <> child.id)
             AND NOT EXISTS (SELECT 1 FROM branches WHERE head_history_id = node.id OR created_from_history_id = node.id)
             AND NOT EXISTS (SELECT 1 FROM final_artifacts WHERE history_id = node.id)",
            [history_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, parent_id, child_id, checkpoint, count) =
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
    // 改接后子节点及其后续节点都通过新的反向 delta 物化，按作品粗粒度失效。
    clear_artwork_verification(&transaction, &target.artwork_id)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{
        count_scheduled_files,
        test_support::{queued_paths, HistoryFixture},
    };
    use crate::platform::path::display_path;

    #[test]
    fn node_source_path_follows_the_branch_work_file() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "node", None, 1);

        let path = node_source_path(&fixture.root, "node").unwrap().unwrap();
        assert!(path.ends_with("main.psd"), "{path}");
        // 未知节点没有工作文件可回查，调用方据此按「不支持预览」处理。
        assert!(node_source_path(&fixture.root, "missing")
            .unwrap()
            .is_none());
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
            display_path(&replacement.canonicalize().unwrap())
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
            display_path(&replacement.canonicalize().unwrap())
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
            false,
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

    /// 分支行与起点 checkpoint 必须同进同退：第二步失败时第一步也要回滚，
    /// 不能留下一个依赖未固化起点的半成品分支。
    #[test]
    fn create_branch_rolls_back_when_checkpointing_the_origin_fails() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        storage::open(&fixture.root)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_branch_checkpoint
                 BEFORE UPDATE OF is_checkpoint ON history_nodes
                 BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END;",
            )
            .unwrap();

        let error = create_branch(
            &fixture.root,
            &fixture.artwork_id,
            "root",
            "Side",
            &fixture.fork_source,
            false,
        )
        .unwrap_err();

        assert!(error.contains("数据库操作失败"), "{error}");
        let connection = storage::open(&fixture.root).unwrap();
        let branch_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM branches WHERE artwork_id = ?1",
                [&fixture.artwork_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(branch_count, 1, "失败的新建分支必须整体回滚");
        let checkpoint: bool = connection
            .query_row(
                "SELECT is_checkpoint <> 0 FROM history_nodes WHERE id = 'root'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!checkpoint, "失败的新建分支不得留下起点 checkpoint");
    }

    #[test]
    fn only_a_checkpoint_parent_may_commit_without_a_reverse_delta() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        mark_checkpoint(&fixture.root, "root").unwrap();

        commit(
            &fixture.root,
            HistoryCommit {
                id: "child-a",
                branch_id: &fixture.main_branch_id,
                parent_id: Some("root"),
                title: "child-a",
                note: "",
                commit_kind: "manual",
                created_ms: 2,
                logical_size: 1,
                chunk_file_size: 1,
                sha256: &format!("{:064X}", 2),
                chunk_count: 1,
                snapshot_path: "artworks/child-a.snapshot",
                delta_path: None,
                delta_size: None,
                thumbnail: None,
                thumbnail_version: None,
            },
        )
        .unwrap();
        let edge: (String, i64) = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT delta_path, delta_size FROM history_edges WHERE child_history_id = 'child-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(edge, (String::new(), 0), "检查点父节点的边应留空");
        assert!(checkpoint_snapshot_holder(&fixture.root, "root").unwrap());

        let rejected = commit(
            &fixture.root,
            HistoryCommit {
                id: "child-b",
                branch_id: &fixture.main_branch_id,
                parent_id: Some("child-a"),
                title: "child-b",
                note: "",
                commit_kind: "manual",
                created_ms: 3,
                logical_size: 1,
                chunk_file_size: 1,
                sha256: &format!("{:064X}", 3),
                chunk_count: 1,
                snapshot_path: "artworks/child-b.snapshot",
                delta_path: None,
                delta_size: None,
                thumbnail: None,
                thumbnail_version: None,
            },
        )
        .unwrap_err();
        assert_eq!(rejected, "非根历史节点缺少反向 delta");
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
