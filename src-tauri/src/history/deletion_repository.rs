//! 破坏性历史操作：删除子树、删除分支与重置整个作品的历史。
//!
//! 所有从图上移除节点的写入都集中在这里，事务内完成引用复查并把已无引用的
//! snapshot/delta 入队 `pending_file_cleanup`；本模块不读取 ChunkFile，也不
//! 负责建立检查点（由应用层在删除前调用 backup 能力完成）。

use std::{collections::HashSet, path::Path};

use rusqlite::{params, OptionalExtension};

use crate::{cleanup, storage};

use super::{BranchDeletion, HistoryDeletion};

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn delete_subtree(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<HistoryDeletion, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork_id = ensure_subtree_deletable(&transaction, history_id, branch_id)?;
    let deletion = delete_subtree_rows(&transaction, history_id, branch_id, &artwork_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(deletion)
}

/// 已完成前置检查的删除入口：`preflight` 必须来自同一仓库、同一分支的
/// [`preflight_subtree_deletion`]。调用方用它在「预检 → 固化父节点检查点 → 删除」
/// 之间只遍历一次子树；这里仍会复查节点归属与分支，防止预检之后图被改写。
pub(crate) fn delete_subtree_prechecked(
    root: &Path,
    history_id: &str,
    branch_id: &str,
    preflight: &SubtreeDeletionPreflight,
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
    if artwork_id != preflight.artwork_id || branch_id != preflight.branch_id {
        return Err("历史结构在删除前发生变化".into());
    }
    let deletion = delete_subtree_rows(&transaction, history_id, branch_id, &artwork_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(deletion)
}

/// 删除子树的共享事务体：前置检查与归属判定已经由调用方完成。
fn delete_subtree_rows(
    transaction: &rusqlite::Transaction<'_>,
    history_id: &str,
    branch_id: &str,
    artwork_id: &str,
) -> Result<HistoryDeletion, String> {
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
        .query_map([artwork_id], |row| {
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
    // 被删除节点与边占用的仓库文件在同一事务内复查引用后入队；仍被其它分支或
    // 节点引用的路径会被跳过，由调用方在提交成功后重放清理。
    let cleanup_ids = cleanup::enqueue_released_repository_files(
        &transaction,
        &paths.into_iter().collect::<Vec<_>>(),
        "history_subtree_deletion",
    )?;
    Ok(HistoryDeletion {
        artwork_id: artwork_id.to_owned(),
        cleanup_ids,
    })
}

/// 删除子树的前置检查结论。usecase 用它把「校验 → 固化父节点检查点 → 删除」
/// 串起来，子树只在第一步遍历一次。
#[derive(Debug)]
pub(crate) struct SubtreeDeletionPreflight {
    pub(crate) artwork_id: String,
    pub(crate) branch_id: String,
}

/// 只读预检：节点必须属于该分支的当前祖先链、子树不含已发布节点，且没有其它
/// 分支指向这段历史。返回归属作品与分支，供预检版删除复用并复查。
pub(crate) fn preflight_subtree_deletion(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<SubtreeDeletionPreflight, String> {
    let connection = storage::open(root)?;
    let artwork_id = ensure_subtree_deletable(&connection, history_id, branch_id)?;
    Ok(SubtreeDeletionPreflight {
        artwork_id,
        branch_id: branch_id.to_owned(),
    })
}

/// `preflight_subtree_deletion` 与自检版 `delete_subtree` 共用的判定体。
fn ensure_subtree_deletable(
    connection: &rusqlite::Connection,
    history_id: &str,
    branch_id: &str,
) -> Result<String, String> {
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
    Ok(artwork_id)
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

/// 重置后新建的那条空分支的标题，与新建 Artwork 对话框的默认值一致。
const RESET_BRANCH_TITLE: &str = "主分支";

/// 把整个作品重置回刚创建时的样子：所有分支、所有历史节点与其仓库文件全部清除，
/// 只留下一条空分支，且该分支不设置工作文件。
///
/// 语义与「删除作品再新建一个」的区别只在保留了什么：作品名、作品树位置、描述与
/// 素材板都留在原处，因此素材板的 DDS 与画板历史不受影响（它们挂在 `artwork_id`
/// 上，与历史表无关）。
///
/// 已发布内容一律拒绝：`enter_branch_publication` 在用户选完成品的那一刻就写入
/// `final_artifacts`，所以「进入发布但还没签名」也算已发布，必须先取消发布。
///
/// 返回已入队的清理 id，由调用方在提交成功后重放。
pub(crate) fn reset_artwork_history(
    root: &Path,
    artwork_id: &str,
    backup_quick_enabled: bool,
) -> Result<Vec<String>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_nodes
                            WHERE id = ?1 AND kind = 'artwork' AND trashed_ms IS NULL)",
            [artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !artwork_exists {
        return Err("找不到 Artwork".into());
    }
    let published: Option<String> = transaction
        .query_row(
            "SELECT branch.title FROM final_artifacts artifact
             JOIN branches branch ON branch.id = artifact.branch_id
             WHERE branch.artwork_id = ?1 LIMIT 1",
            [artwork_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if let Some(title) = published {
        return Err(format!(
            "无法重置历史：分支“{title}”已有最终成品，请先在发布页取消发布并删除本地数据"
        ));
    }
    // 认证记录经最终成品级联，正常情况下不会单独存在；显式复查是为了让异常数据
    // 得到可读的拒绝，而不是让 `history_nodes` 的 RESTRICT 外键以一个裸错误中止。
    let recorded: Option<String> = transaction
        .query_row(
            "SELECT branch.title FROM certification_records record
             JOIN branches branch ON branch.id = record.branch_id
             WHERE branch.artwork_id = ?1 LIMIT 1",
            [artwork_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if let Some(title) = recorded {
        return Err(format!(
            "无法重置历史：分支“{title}”还有认证记录，请先在发布页取消发布并删除本地数据"
        ));
    }

    let mut paths = HashSet::new();
    let mut statement = transaction
        .prepare(
            "SELECT snapshot_path FROM history_nodes
               WHERE artwork_id = ?1 AND snapshot_path IS NOT NULL AND TRIM(snapshot_path) <> ''
             UNION ALL
             SELECT delta_path FROM history_nodes
               WHERE artwork_id = ?1 AND delta_path IS NOT NULL AND TRIM(delta_path) <> ''
             UNION ALL
             SELECT edge.delta_path FROM history_edges edge
               JOIN history_nodes child ON child.id = edge.child_history_id
               WHERE child.artwork_id = ?1 AND TRIM(edge.delta_path) <> ''",
        )
        .map_err(storage::database_error)?;
    for path in statement
        .query_map([artwork_id], |row| row.get::<_, String>(0))
        .map_err(storage::database_error)?
    {
        paths.insert(path.map_err(storage::database_error)?);
    }
    drop(statement);

    // 顺序不可换：`history_nodes.created_on_branch_id` 是 ON DELETE RESTRICT，
    // 先删分支会被外键挡下。节点删除会级联带走边，分支的 head / 分叉起点由
    // 外键置空后再整行删除。
    transaction
        .execute(
            "DELETE FROM history_nodes WHERE artwork_id = ?1",
            [artwork_id],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute("DELETE FROM branches WHERE artwork_id = ?1", [artwork_id])
        .map_err(storage::database_error)?;
    let branch_id = storage::new_id();
    let now = storage::now_ms()?;
    transaction
        .execute(
            "INSERT INTO branches
             (id, artwork_id, title, source_path, source_path_key, backup_quick_enabled,
              created_ms, updated_ms)
             VALUES (?1, ?2, ?3, '', '', ?4, ?5, ?5)",
            params![
                branch_id,
                artwork_id,
                RESET_BRANCH_TITLE,
                i64::from(backup_quick_enabled),
                now
            ],
        )
        .map_err(storage::database_error)?;
    // 作品与作品树的「最后更新」要跟着动，否则列表顺序与时间仍显示重置之前。
    transaction
        .execute(
            "UPDATE artworks SET updated_ms = ?2 WHERE id = ?1",
            params![artwork_id, now],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "UPDATE library_nodes SET updated_ms = ?2 WHERE id = ?1",
            params![artwork_id, now],
        )
        .map_err(storage::database_error)?;
    // 入队必须在删除之后：引用清单以事务内可见的图为准，先入队会把每条路径都
    // 当成仍被引用而全部跳过。
    let cleanup_ids = cleanup::enqueue_released_repository_files(
        &transaction,
        &paths.into_iter().collect::<Vec<_>>(),
        "artwork_history_reset",
    )?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use super::*;
    use crate::history::{
        commit, create_branch,
        test_support::{queued_paths, HistoryFixture},
        HistoryCommit,
    };

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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &main_source, false)
                .unwrap();
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
                thumbnail: None,
                thumbnail_version: None,
            },
        )
        .unwrap();
        let branch_id = create_branch(
            &root,
            &artwork.artwork_id,
            root_node,
            "Fork",
            &fork_source,
            false,
        )
        .unwrap();
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
                thumbnail: None,
                thumbnail_version: None,
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
    fn artwork_reset_clears_every_branch_and_keeps_one_empty_branch() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        // 先从 root 分叉再让主线继续提交：root 因此是检查点，提交不会把它的
        // snapshot 提前入队，重置时的清理队列只包含这次重置释放的文件。
        let fork_branch = create_branch(
            &fixture.root,
            &fixture.artwork_id,
            "root",
            "Fork",
            &fixture.fork_source,
            false,
        )
        .unwrap();
        fixture.commit_node(&fixture.main_branch_id, "head", Some("root"), 2);
        fixture.commit_node(&fork_branch, "fork-head", Some("root"), 3);

        let cleanup_ids = reset_artwork_history(&fixture.root, &fixture.artwork_id, false).unwrap();

        let connection = storage::open(&fixture.root).unwrap();
        let counts: (i64, i64) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM branches WHERE artwork_id = ?1),
                        (SELECT COUNT(*) FROM history_nodes WHERE artwork_id = ?1)",
                [&fixture.artwork_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 0));
        let branch: (String, String, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT title, source_path, head_history_id, created_from_history_id
                 FROM branches WHERE artwork_id = ?1",
                [&fixture.artwork_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(branch.0, "主分支");
        assert_eq!(branch.1, "");
        assert_eq!(branch.2, None);
        assert_eq!(branch.3, None);
        // 作品与作品树节点留在原处：重置只动历史。
        let node_title: String = connection
            .query_row(
                "SELECT title FROM library_nodes WHERE id = ?1",
                [&fixture.artwork_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(node_title, "Artwork");
        drop(connection);

        let queued = queued_paths(&fixture.root);
        assert!(queued.iter().any(|path| path == "artworks/root.snapshot"));
        assert!(queued
            .iter()
            .any(|path| path == "artworks/fork-head-to-root.delta"));
        let report = crate::cleanup::run(&fixture.root, &cleanup_ids).unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.pending_count, 0);
    }

    #[test]
    fn artwork_reset_refuses_published_artwork() {
        let fixture = HistoryFixture::new();
        fixture.commit_node(&fixture.main_branch_id, "root", None, 1);
        storage::open(&fixture.root)
            .unwrap()
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms)
                 VALUES ('artifact', ?1, 'root', 'artworks/final.jpg', ?2, 'image/jpeg', 1, 2)",
                params![fixture.main_branch_id, "A".repeat(64)],
            )
            .unwrap();

        let error = reset_artwork_history(&fixture.root, &fixture.artwork_id, false).unwrap_err();

        assert!(error.contains("最终成品"), "{error}");
        let nodes: i64 = storage::open(&fixture.root)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM history_nodes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(nodes, 1);
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

        let error = preflight_subtree_deletion(&fixture.root, "published", &fixture.main_branch_id)
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
            false,
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

        // 生产路径：预检一次后把结论交给预检版删除（中间还要固化父节点检查点）。
        let preflight =
            preflight_subtree_deletion(&fixture.root, "cut", &fixture.main_branch_id).unwrap();
        delete_subtree_prechecked(&fixture.root, "cut", &fixture.main_branch_id, &preflight)
            .unwrap();

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
            false,
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
}
