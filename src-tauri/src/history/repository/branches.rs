//! 分支行与分支列表的读写。
//!
//! 分支设置、起点固化与工作文件路径的唯一性都在这里的事务内完成；节点图由
//! `graph` 维护。

use std::path::Path;

use rusqlite::{params, OptionalExtension};

use crate::history::{
    classify_verify_failure, ArtworkBranch, ArtworkHistory, BranchRecord, CheckMode, HistoryNode,
    NodeVerifyIssue,
};
use crate::storage;

use super::super::verification_repository::{derive_branch_verification, node_verification_states};

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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::history::count_scheduled_files;
    use crate::history::test_support::HistoryFixture;
    use crate::platform::path::display_path;

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
}
