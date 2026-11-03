//! 作品树回收站：软删除、恢复、永久删除与清空。
//!
//! 永久删除一个 Artwork 时会同时改写 History 与 Authenticity 的表；所有表
//! 在同一个 SQLite 事务里提交，原子性由本模块的 `permanently_delete_trash_root`
//! 与 `permanent_deletion_rolls_back_when_a_cross_domain_delete_fails` 单测锁定。

use std::{collections::HashSet, path::Path};

use rusqlite::{params, OptionalExtension, Transaction};

use crate::{
    cleanup,
    storage::{database_error, now_ms},
};

use super::super::model::{LibraryTrashEntry, LibraryTree};
use super::open;
use super::tree::{
    ensure_nodes_exist, list_tree, load_parent_map, normalize_siblings, sibling_ids,
    top_level_selection, update_sibling_positions, validate_node_ids,
};

pub(crate) fn trash_nodes(root: &Path, ids: &[String]) -> Result<LibraryTree, String> {
    let ids = validate_node_ids(ids)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let parent_map = load_parent_map(&transaction, false)?;
    ensure_nodes_exist(&ids, &parent_map)?;
    let selected = top_level_selection(&ids, &parent_map)?;
    let now = now_ms()?;
    let affected_parents = selected
        .iter()
        .filter_map(|id| parent_map.get(id))
        .cloned()
        .collect::<HashSet<_>>();
    for id in &selected {
        transaction
            .execute(
                "WITH RECURSIVE subtree(id) AS (
                   SELECT id FROM library_nodes WHERE id = ?1 AND trashed_ms IS NULL
                   UNION ALL
                   SELECT child.id FROM library_nodes child
                   JOIN subtree ON child.parent_id = subtree.id
                   WHERE child.trashed_ms IS NULL
                 )
                 UPDATE library_nodes
                 SET trashed_ms = ?2,
                     trash_root_id = ?1,
                     restore_parent_id = CASE WHEN id = ?1 THEN parent_id ELSE NULL END,
                     restore_position = CASE WHEN id = ?1 THEN position ELSE NULL END,
                     parent_id = CASE WHEN id = ?1 THEN NULL ELSE parent_id END,
                     updated_ms = ?2
                 WHERE id IN (SELECT id FROM subtree)",
                params![id, now],
            )
            .map_err(database_error)?;
    }
    for parent_id in affected_parents {
        normalize_siblings(&transaction, parent_id.as_deref())?;
    }
    transaction.commit().map_err(database_error)?;
    list_tree(root)
}

pub(crate) fn list_trash(root: &Path) -> Result<Vec<LibraryTrashEntry>, String> {
    let connection = open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT root.id, root.kind, root.title, root.trashed_ms,
                    (SELECT COUNT(*) - 1 FROM library_nodes member
                     WHERE member.trash_root_id = root.id),
                    (SELECT COUNT(*) FROM library_nodes member
                     WHERE member.trash_root_id = root.id AND member.kind = 'artwork'),
                    parent.title
             FROM library_nodes root
             LEFT JOIN library_nodes parent ON parent.id = root.restore_parent_id
             WHERE root.trashed_ms IS NOT NULL AND root.trash_root_id = root.id
             ORDER BY root.trashed_ms DESC, root.id",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(LibraryTrashEntry {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                deleted_ms: row.get(3)?,
                descendant_count: row.get(4)?,
                artwork_count: row.get(5)?,
                original_parent_title: row.get(6)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(rows)
}

pub(crate) fn restore_trash(root: &Path, id: &str) -> Result<LibraryTree, String> {
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let entry: Option<(Option<String>, Option<i64>)> = transaction
        .query_row(
            "SELECT restore_parent_id, restore_position FROM library_nodes
             WHERE id = ?1 AND trashed_ms IS NOT NULL AND trash_root_id = id",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    let (stored_parent, stored_position) = entry.ok_or("找不到回收站项目")?;
    let restore_parent = match stored_parent {
        Some(parent_id)
            if transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_nodes
                     WHERE id = ?1 AND kind = 'group' AND trashed_ms IS NULL)",
                    [&parent_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)? =>
        {
            Some(parent_id)
        }
        _ => None,
    };
    let mut destination = sibling_ids(&transaction, restore_parent.as_deref())?;
    let index = stored_position
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(destination.len())
        .min(destination.len());
    destination.insert(index, id.to_owned());
    transaction
        .execute(
            "WITH RECURSIVE subtree(id) AS (
               SELECT id FROM library_nodes WHERE id = ?1 AND trash_root_id = ?1
               UNION ALL
               SELECT child.id FROM library_nodes child JOIN subtree ON child.parent_id = subtree.id
               WHERE child.trash_root_id = ?1
             )
             UPDATE library_nodes
             SET trashed_ms = NULL, trash_root_id = NULL,
                 restore_parent_id = NULL, restore_position = NULL,
                 updated_ms = ?3,
                 parent_id = CASE WHEN id = ?1 THEN ?2 ELSE parent_id END
             WHERE id IN (SELECT id FROM subtree)",
            params![id, restore_parent, now_ms()?],
        )
        .map_err(database_error)?;
    update_sibling_positions(&transaction, &destination)?;
    transaction.commit().map_err(database_error)?;
    list_tree(root)
}

pub(crate) fn permanently_delete_trash(root: &Path, ids: &[String]) -> Result<Vec<String>, String> {
    let ids = validate_node_ids(ids)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let mut cleanup_ids = Vec::new();
    for id in ids {
        cleanup_ids.extend(permanently_delete_trash_root(&transaction, &id)?);
    }
    transaction.commit().map_err(database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn empty_trash(root: &Path) -> Result<Vec<String>, String> {
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM library_nodes
                 WHERE trashed_ms IS NOT NULL AND trash_root_id = id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(database_error)?;
        rows
    };
    let mut cleanup_ids = Vec::new();
    for id in ids {
        cleanup_ids.extend(permanently_delete_trash_root(&transaction, &id)?);
    }
    transaction.commit().map_err(database_error)?;
    Ok(cleanup_ids)
}

/// 永久删除一个回收站根及其元数据。
///
/// 跨领域表的顺序固定在同一个事务内：先登记 Artwork 目录清理，再清空分支对
/// 历史节点的引用（`history_nodes.created_on_branch_id` 是 `ON DELETE RESTRICT`），
/// 随后删除 `final_artifacts`（级联认证记录）与 `history_nodes`（级联边），最后
/// 删除 `library_nodes` 根。任一步失败都整体回滚，不留下半个作品。
fn permanently_delete_trash_root(
    transaction: &Transaction<'_>,
    id: &str,
) -> Result<Vec<String>, String> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_nodes
             WHERE id = ?1 AND trashed_ms IS NOT NULL AND trash_root_id = id)",
            [id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err(format!("找不到回收站项目：{id}"));
    }
    let artwork_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM library_nodes
                 WHERE trash_root_id = ?1 AND kind = 'artwork'",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows
    };
    let mut cleanup_ids = Vec::new();
    for artwork_id in artwork_ids {
        cleanup_ids.push(cleanup::enqueue_repository_directory(
            transaction,
            &format!("artworks/{artwork_id}"),
            "permanent_artwork_deletion",
        )?);
    }
    transaction
        .execute(
            "UPDATE branches SET head_history_id = NULL, created_from_history_id = NULL
             WHERE artwork_id IN (
               SELECT id FROM library_nodes WHERE trash_root_id = ?1 AND kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM final_artifacts WHERE branch_id IN (
               SELECT b.id FROM branches b
               JOIN library_nodes n ON n.id = b.artwork_id
               WHERE n.trash_root_id = ?1 AND n.kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM history_nodes WHERE artwork_id IN (
               SELECT id FROM library_nodes WHERE trash_root_id = ?1 AND kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute("DELETE FROM library_nodes WHERE id = ?1", [id])
        .map_err(database_error)?;
    Ok(cleanup_ids)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::tree::create_group_id;
    use super::super::{open, test_support::Fixture};
    use super::*;
    use crate::platform::path::display_path;

    #[test]
    fn trash_parent_and_child_is_idempotent_and_normalizes_root() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        trash_nodes(&fixture.root, &[parent.clone(), child]).unwrap();
        let trash = list_trash(&fixture.root).unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, parent);
        assert_eq!(trash[0].descendant_count, 1);
        let connection = open(&fixture.root).unwrap();
        let positions = connection
            .prepare(
                "SELECT position FROM library_nodes
                 WHERE parent_id IS NULL AND trashed_ms IS NULL ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(positions, (0..positions.len() as i64).collect::<Vec<_>>());
    }

    #[test]
    fn restores_trashed_subtree_to_original_parent() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        trash_nodes(&fixture.root, std::slice::from_ref(&child)).unwrap();
        assert_eq!(list_trash(&fixture.root).unwrap().len(), 1);
        let restored = restore_trash(&fixture.root, &child).unwrap();
        let parent_node = restored
            .nodes
            .iter()
            .find(|node| node.id == parent)
            .unwrap();
        assert_eq!(parent_node.children[0].id, child);
        assert!(list_trash(&fixture.root).unwrap().is_empty());
    }

    #[test]
    fn permanently_deletes_only_from_trash() {
        let fixture = Fixture::new();
        let group = create_group_id(&fixture.root, None, "Discard").unwrap();
        let error =
            permanently_delete_trash(&fixture.root, std::slice::from_ref(&group)).unwrap_err();
        assert!(error.contains("找不到回收站项目"), "{error}");
        trash_nodes(&fixture.root, std::slice::from_ref(&group)).unwrap();
        permanently_delete_trash(&fixture.root, std::slice::from_ref(&group)).unwrap();
        assert!(list_trash(&fixture.root).unwrap().is_empty());
        let exists: bool = open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_nodes WHERE id = ?1)",
                [&group],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists);
    }

    #[test]
    fn permanently_deleted_artwork_removes_its_repository_directory() {
        let fixture = Fixture::new();
        let artwork_directory = fixture
            .root
            .join("artworks")
            .join(&fixture.artwork.artwork_id);
        fs::create_dir_all(&artwork_directory).unwrap();
        fs::write(artwork_directory.join("snapshot.bin"), b"snapshot").unwrap();
        trash_nodes(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        let cleanup_ids = permanently_delete_trash(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();
        assert!(artwork_directory.is_dir());
        let report = crate::cleanup::run(&fixture.root, &cleanup_ids).unwrap();

        assert!(report.failures.is_empty());
        assert!(!artwork_directory.exists());
    }

    #[test]
    fn permanently_deletes_published_artwork_without_foreign_key_failure() {
        let fixture = Fixture::new();
        let external_output = fixture.root.parent().unwrap().join("published-output.jpg");
        fs::write(&external_output, b"published output").unwrap();
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "INSERT INTO history_nodes
                 (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind,
                  is_checkpoint, created_ms, logical_size, chunk_file_size, sha256, chunk_count,
                  snapshot_path, delta_path)
                 VALUES ('published-history', ?1, ?2, NULL, 'Published', '', 'manual',
                         1, 1, 1, 1, ?3, 1, 'snapshots/published.lbc', NULL)",
                params![
                    fixture.artwork.artwork_id,
                    fixture.artwork.branch_id,
                    "A".repeat(64)
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET head_history_id = 'published-history' WHERE id = ?1",
                [&fixture.artwork.branch_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256, media_type,
                  byte_size, created_ms)
                 VALUES ('published-artifact', ?1, 'published-history',
                         'artworks/published.png', ?2, 'image/png', 1, 1)",
                params![fixture.artwork.branch_id, "B".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO certification_records
                 (id, final_artifact_id, branch_id, history_id, watermark_id,
                  trustmark_enabled, output_path, stored_path, output_sha256,
                  output_bytes, title, creator, rights_statement,
                  authentication_content, regions_json, created_ms)
                 VALUES ('published-record', 'published-artifact', ?1,
                         'published-history', NULL, 0, ?2,
                         'artworks/published-record.jpg', ?3, 16,
                         'Title', '', '', '', '[]', 1)",
                params![
                    fixture.artwork.branch_id,
                    display_path(&external_output),
                    "C".repeat(64)
                ],
            )
            .unwrap();
        drop(connection);
        trash_nodes(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        let cleanup_ids = permanently_delete_trash(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();
        let report = crate::cleanup::run(&fixture.root, &cleanup_ids).unwrap();

        let connection = open(&fixture.root).unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM library_nodes WHERE id = ?1",
                [&fixture.artwork.artwork_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        assert!(report.failures.is_empty());
        assert!(external_output.is_file());
        let external_cleanup_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pending_file_cleanup WHERE path_kind = 'external_file'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(external_cleanup_count, 0);
    }

    /// 永久删除同时改写 `library_nodes`、`branches`、`final_artifacts`、
    /// `certification_records`、`history_nodes` 与 `pending_file_cleanup`；
    /// 后续语句失败时整笔事务必须回滚，不能留下已清空的分支引用或已入队的目录。
    #[test]
    fn permanent_deletion_rolls_back_when_a_cross_domain_delete_fails() {
        let fixture = Fixture::new();
        let artwork_directory = format!("artworks/{}", fixture.artwork.artwork_id);
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "INSERT INTO history_nodes
                 (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind,
                  is_checkpoint, created_ms, logical_size, chunk_file_size, sha256, chunk_count,
                  snapshot_path, delta_path)
                 VALUES ('rollback-history', ?1, ?2, NULL, 'Rollback', '', 'manual',
                         1, 1, 1, 1, ?3, 1, ?4, NULL)",
                params![
                    fixture.artwork.artwork_id,
                    fixture.artwork.branch_id,
                    "A".repeat(64),
                    format!("{artwork_directory}/snapshots/rollback.lbc")
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET head_history_id = 'rollback-history' WHERE id = ?1",
                [&fixture.artwork.branch_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256, media_type,
                  byte_size, created_ms)
                 VALUES ('rollback-artifact', ?1, 'rollback-history',
                         'artworks/rollback.png', ?2, 'image/png', 1, 1)",
                params![fixture.artwork.branch_id, "B".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO certification_records
                 (id, final_artifact_id, branch_id, history_id, watermark_id,
                  trustmark_enabled, output_path, stored_path, output_sha256,
                  output_bytes, title, creator, rights_statement,
                  authentication_content, regions_json, created_ms)
                 VALUES ('rollback-certification', 'rollback-artifact', ?1,
                         'rollback-history', NULL, 0, ?2,
                         'artworks/rollback-record.jpg', ?3, 16,
                         'Title', '', '', '', '[]', 1)",
                params![
                    fixture.artwork.branch_id,
                    display_path(&fixture.root.parent().unwrap().join("rollback.jpg")),
                    "C".repeat(64)
                ],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_final_artifact_delete
                 BEFORE DELETE ON final_artifacts
                 BEGIN SELECT RAISE(ABORT, 'injected cross-domain failure'); END;",
            )
            .unwrap();
        drop(connection);
        trash_nodes(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        let error = permanently_delete_trash(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap_err();
        assert!(error.contains("数据库操作失败"), "{error}");

        let connection = open(&fixture.root).unwrap();
        let remains: (bool, Option<String>) = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_nodes
                               WHERE id = ?1 AND trashed_ms IS NOT NULL),
                        (SELECT head_history_id FROM branches WHERE id = ?2)",
                params![fixture.artwork.artwork_id, fixture.artwork.branch_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(remains.0, "失败的永久删除必须保留回收站项目");
        assert_eq!(remains.1.as_deref(), Some("rollback-history"));
        for table in ["final_artifacts", "certification_records"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
        let queued: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pending_file_cleanup WHERE path = ?1",
                [&artwork_directory],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued, 0, "回滚必须同时撤销清理入队");
    }
}
