//! 画板列表的增删改查、回收站与排序。
//!
//! 这里只动 `pin_boards` 元数据与回收站清理入队；条目内容见 `view` / `save` /
//! `create`。修订号（revision）由 [`bump_revision`] 统一推进，重命名与重排不改变它。

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::paths::board_relative_path;
use super::{PinBoardSummary, MAX_BOARDS_PER_ARTWORK};
use crate::{cleanup, storage};

pub(crate) fn create_board(
    connection: &mut Connection,
    artwork_id: &str,
    name: &str,
) -> Result<PinBoardSummary, String> {
    let name = normalise_name(name)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork: Option<(String, Option<i64>)> = transaction
        .query_row(
            "SELECT n.title, n.trashed_ms FROM artworks a
             JOIN library_nodes n ON n.id = a.id
             WHERE a.id = ?1",
            [artwork_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_title, trashed_ms) = artwork.ok_or("作品不存在")?;
    if trashed_ms.is_some() {
        return Err("回收站中的作品不能新建画板".into());
    }
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM pin_boards WHERE artwork_id = ?1 AND deleted_at IS NULL",
            [artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if count >= MAX_BOARDS_PER_ARTWORK as i64 {
        return Err(format!(
            "单个作品最多只能有 {MAX_BOARDS_PER_ARTWORK} 块画板"
        ));
    }
    let now = storage::now_ms()?;
    let next_order: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM pin_boards
             WHERE artwork_id = ?1 AND deleted_at IS NULL",
            [artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "INSERT INTO pin_boards
               (artwork_id, name, sort_order, revision, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![artwork_id, name, next_order, initial_revision(), now],
        )
        .map_err(storage::database_error)?;
    let board_id = transaction.last_insert_rowid();
    // 不在此处创建画板目录：`persist_dds_file` 落盘时会按需创建。原先用空根
    // 拼出相对路径再 `create_dir_all`，会按进程工作目录（安装目录 / crate 根）
    // 生成一个从不被读写的空目录。
    transaction.commit().map_err(storage::database_error)?;
    Ok(PinBoardSummary {
        board_id,
        artwork_id: artwork_id.to_owned(),
        artwork_title,
        name,
        sort_order: next_order,
        deleted: false,
        deleted_at: None,
        item_count: 0,
        created_ms: now,
        updated_ms: now,
    })
}

pub(crate) fn list_boards(
    connection: &Connection,
    artwork_id: &str,
) -> Result<Vec<PinBoardSummary>, String> {
    list_boards_inner(connection, artwork_id, false)
}

pub(crate) fn list_trash(connection: &Connection) -> Result<Vec<PinBoardSummary>, String> {
    list_boards_inner(connection, "", true)
}

fn list_boards_inner(
    connection: &Connection,
    artwork_id: &str,
    trash: bool,
) -> Result<Vec<PinBoardSummary>, String> {
    // Artwork 进入项目回收站时其画板随之隐藏（trashed_ms 非空）。回收站列表跨作品，
    // 因此只有工作列表带 artwork_id 过滤、也只有它绑定参数 ?1。
    let (filter, order) = if trash {
        ("AND b.deleted_at IS NOT NULL", "b.deleted_at DESC, b.id")
    } else {
        (
            "AND b.artwork_id = ?1 AND b.deleted_at IS NULL",
            "b.sort_order, b.id",
        )
    };
    let sql = format!(
        "SELECT b.id, b.artwork_id, n.title, b.name, b.sort_order, b.deleted_at,
                (SELECT COUNT(*) FROM pin_board_items i WHERE i.board_id = b.id),
                b.created_ms, b.updated_ms
           FROM pin_boards b
           JOIN artworks a ON a.id = b.artwork_id
           JOIN library_nodes n ON n.id = a.id
          WHERE n.trashed_ms IS NULL
            {filter}
          ORDER BY {order}"
    );
    let mut statement = connection.prepare(&sql).map_err(storage::database_error)?;
    let rows = if trash {
        statement
            .query_map([], row_summary)
            .map_err(storage::database_error)?
    } else {
        statement
            .query_map([artwork_id], row_summary)
            .map_err(storage::database_error)?
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn row_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<PinBoardSummary> {
    let deleted_at: Option<i64> = row.get(5)?;
    Ok(PinBoardSummary {
        board_id: row.get(0)?,
        artwork_id: row.get(1)?,
        artwork_title: row.get(2)?,
        name: row.get(3)?,
        sort_order: row.get(4)?,
        deleted: deleted_at.is_some(),
        deleted_at,
        item_count: row.get(6)?,
        created_ms: row.get(7)?,
        updated_ms: row.get(8)?,
    })
}

/// 重命名画板。名字与排序一样只是列表元数据，不是画板内容，因此只改
/// `name` / `updated_ms`，不更新 `revision`——避免“改名后已打开画板的下一次
/// 保存被误判为冲突”，导致持续保存失败且无法切换画板。
pub(crate) fn rename_board(
    connection: &mut Connection,
    board_id: i64,
    name: &str,
) -> Result<PinBoardSummary, String> {
    let name = normalise_name(name)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    require_active_board(&transaction, board_id)?;
    let now = storage::now_ms()?;
    transaction
        .execute(
            "UPDATE pin_boards SET name = ?1, updated_ms = ?2 WHERE id = ?3",
            params![name, now, board_id],
        )
        .map_err(storage::database_error)?;
    let summary = board_summary(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(summary)
}

pub(crate) fn trash_board(connection: &mut Connection, board_id: i64) -> Result<(), String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    require_active_board(&transaction, board_id)?;
    let now = storage::now_ms()?;
    transaction
        .execute(
            "UPDATE pin_boards SET deleted_at = ?1, updated_ms = ?2, revision = ?3 WHERE id = ?4",
            params![now, now, revision_value(board_id, now), board_id],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(())
}

/// 恢复回原 Artwork。原 Artwork 已永久删除时记录已随级联删除，恢复必然失败。
pub(crate) fn restore_board(
    connection: &mut Connection,
    board_id: i64,
) -> Result<PinBoardSummary, String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pin_boards WHERE id = ?1 AND deleted_at IS NOT NULL)",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if !exists {
        return Err("回收站中不存在该画板".into());
    }
    // 只按画板自身取同一 Artwork 的兄弟画板；原先的 JOIN 只带来两个未被筛选
    // 使用的表，与恢复本身无关。
    let next_order: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM pin_boards
             WHERE artwork_id = (SELECT artwork_id FROM pin_boards WHERE id = ?1)
               AND deleted_at IS NULL",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    let now = storage::now_ms()?;
    transaction
        .execute(
            "UPDATE pin_boards SET deleted_at = NULL, sort_order = ?1, updated_ms = ?2,
                    revision = ?3
             WHERE id = ?4",
            params![next_order, now, revision_value(board_id, now), board_id],
        )
        .map_err(storage::database_error)?;
    let summary = board_summary(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(summary)
}

/// 重排同一 Artwork 内未删除画板的显示顺序：`board_ids` 的顺序即目标顺序，
/// 必须与当前未删除画板集合完全一致，否则视为列表已变化并拒绝写入。
///
/// 顺序只是列表元数据，不是画板内容，因此只改 `sort_order`，不更新
/// `updated_ms` / `revision`——避免“重排后已打开画板的下一次保存被误判为冲突”。
pub(crate) fn reorder_boards(
    connection: &mut Connection,
    artwork_id: &str,
    board_ids: &[i64],
) -> Result<Vec<PinBoardSummary>, String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork: Option<Option<i64>> = transaction
        .query_row(
            "SELECT n.trashed_ms FROM artworks a
             JOIN library_nodes n ON n.id = a.id
             WHERE a.id = ?1",
            [artwork_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    match artwork {
        None => return Err("作品不存在".into()),
        Some(Some(_)) => return Err("回收站中的作品不能调整画板顺序".into()),
        Some(None) => {}
    }

    let current: Vec<i64> = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM pin_boards
                 WHERE artwork_id = ?1 AND deleted_at IS NULL
                 ORDER BY sort_order, id",
            )
            .map_err(storage::database_error)?;
        let rows = statement
            .query_map([artwork_id], |row| row.get::<_, i64>(0))
            .map_err(storage::database_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage::database_error)?
    };

    let mut requested = board_ids.to_vec();
    requested.sort_unstable();
    requested.dedup();
    let mut expected = current.clone();
    expected.sort_unstable();
    if board_ids.len() != current.len()
        || requested.len() != board_ids.len()
        || requested != expected
    {
        return Err("画板列表已变化，请刷新后重试".into());
    }

    for (index, board_id) in board_ids.iter().enumerate() {
        transaction
            .execute(
                "UPDATE pin_boards SET sort_order = ?1 WHERE id = ?2",
                params![index as i64, board_id],
            )
            .map_err(storage::database_error)?;
    }
    transaction.commit().map_err(storage::database_error)?;
    list_boards(connection, artwork_id)
}

/// 永久删除单个回收站画板：先在事务中删除记录并入队目录清理，
/// 提交后由调用方执行 `cleanup::run`。
///
/// 只接受已在回收站的画板——取 artwork id 时一并确认 `deleted_at` 非空。命令面本就不给
/// 活画板这个入口，但按 id 直接硬删除会连带删掉它的 DDS 且不可撤销，值得再确认一次。
pub(crate) fn delete_board_permanently(
    connection: &mut Connection,
    board_id: i64,
) -> Result<Vec<String>, String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artwork_id = trashed_board_artwork_id(&transaction, board_id)?;
    transaction
        .execute("DELETE FROM pin_boards WHERE id = ?1", [board_id])
        .map_err(storage::database_error)?;
    let cleanup_ids = vec![cleanup::enqueue_repository_directory(
        &transaction,
        &board_relative_path(&artwork_id, board_id),
        "pin_board_permanent_deletion",
    )?];
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn empty_trash(connection: &mut Connection) -> Result<Vec<String>, String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let mut statement = transaction
        .prepare("SELECT id FROM pin_boards WHERE deleted_at IS NOT NULL")
        .map_err(storage::database_error)?;
    let ids = statement
        .query_map([], |row| row.get(0))
        .map_err(storage::database_error)?
        .collect::<Result<Vec<i64>, _>>()
        .map_err(storage::database_error)?;
    drop(statement);
    let mut cleanup_ids = Vec::new();
    for board_id in ids {
        let artwork_id = trashed_board_artwork_id(&transaction, board_id)?;
        transaction
            .execute("DELETE FROM pin_boards WHERE id = ?1", [board_id])
            .map_err(storage::database_error)?;
        cleanup_ids.push(cleanup::enqueue_repository_directory(
            &transaction,
            &board_relative_path(&artwork_id, board_id),
            "pin_board_trash_emptied",
        )?);
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn normalise_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("画板名称不能为空".into());
    }
    if name.chars().count() > 120 {
        return Err("画板名称不能超过 120 个字符".into());
    }
    Ok(name.to_owned())
}

fn board_summary(connection: &Connection, board_id: i64) -> Result<PinBoardSummary, String> {
    connection
        .query_row(
            "SELECT b.id, b.artwork_id, n.title, b.name, b.sort_order, b.deleted_at,
                    (SELECT COUNT(*) FROM pin_board_items i WHERE i.board_id = b.id),
                    b.created_ms, b.updated_ms
               FROM pin_boards b
               JOIN artworks a ON a.id = b.artwork_id
               JOIN library_nodes n ON n.id = a.id
              WHERE b.id = ?1",
            [board_id],
            row_summary,
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "画板不存在".to_owned())
}

pub(super) fn require_active_board(connection: &Connection, board_id: i64) -> Result<(), String> {
    let active: Option<i64> = connection
        .query_row(
            "SELECT b.id FROM pin_boards b
             JOIN artworks a ON a.id = b.artwork_id
             JOIN library_nodes n ON n.id = a.id
             WHERE b.id = ?1 AND b.deleted_at IS NULL AND n.trashed_ms IS NULL",
            [board_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    if active.is_some() {
        Ok(())
    } else {
        Err("画板不存在或已在回收站".into())
    }
}

/// 回收站画板所属的 Artwork id，顺带确认它确实在回收站里——永久删除与清空都只应
/// 处理已软删除的画板，活画板硬删除会连带删掉 DDS 且不可撤销。
fn trashed_board_artwork_id(connection: &Connection, board_id: i64) -> Result<String, String> {
    connection
        .query_row(
            "SELECT artwork_id FROM pin_boards WHERE id = ?1 AND deleted_at IS NOT NULL",
            [board_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "回收站中不存在该画板".to_owned())
}

/// 修订号：对「域前缀 + 画板 id + updated_ms」做 SHA-256。同一个 `updated_ms` 在两次
/// 写入中不会重复（[`bump_revision`] 保证严格递增），因此修订号可直接用于保存冲突检测。
fn revision_value(board_id: i64, updated_ms: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lilith-artworks:pin-board:");
    hasher.update(board_id.to_le_bytes());
    hasher.update(updated_ms.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// 在事务中推进画板 updated_ms 并返回新修订号。取 `max(now_ms, 当前 + 1)`：同一毫秒内
/// 连续写库也会让修订号单调变化。
pub(super) fn bump_revision(
    transaction: &rusqlite::Transaction<'_>,
    board_id: i64,
) -> Result<(i64, String), String> {
    let current: i64 = transaction
        .query_row(
            "SELECT updated_ms FROM pin_boards WHERE id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    let now = storage::now_ms()?;
    let next = now.max(current.saturating_add(1));
    let revision = revision_value(board_id, next);
    transaction
        .execute(
            "UPDATE pin_boards SET updated_ms = ?1, revision = ?2 WHERE id = ?3",
            params![next, revision, board_id],
        )
        .map_err(storage::database_error)?;
    Ok((next, revision))
}

/// 作废当前步骤之后的 redo 记录：写入新步骤前统一清一次。
pub(super) fn discard_future_history(
    transaction: &rusqlite::Transaction<'_>,
    board_id: i64,
    now_step: u64,
) -> Result<(), String> {
    transaction
        .execute(
            "DELETE FROM pin_board_history WHERE board_id = ?1 AND step > ?2",
            params![board_id, now_step as i64],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 新建画板的初始修订号。与 [`revision_value`] 共用前缀但不是一个具体时刻的哈希——
/// 「刚建、还没写过内容」本身就是一种可识别的状态。
fn initial_revision() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lilith-artworks:pin-board:initial");
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{create_test_artwork, test_repository};
    use super::*;

    #[test]
    fn creates_and_lists_boards_per_artwork() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "参考板").unwrap();
        assert_eq!(board.name, "参考板");
        assert_eq!(board.sort_order, 0);

        let boards = list_boards(&connection, &artwork_id).unwrap();
        assert_eq!(boards.len(), 1);
        assert!(list_trash(&connection).unwrap().is_empty());

        let second = create_board(&mut connection, &artwork_id, "第二块").unwrap();
        assert_eq!(second.sort_order, 1);
    }

    #[test]
    fn renames_boards_with_normalised_names() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "旧名").unwrap();
        let revision_before: String = connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [board.board_id],
                |row| row.get(0),
            )
            .unwrap();
        let renamed = rename_board(&mut connection, board.board_id, "  新名字  ").unwrap();
        assert_eq!(renamed.name, "新名字");
        assert!(rename_board(&mut connection, board.board_id, "   ").is_err());

        // 名字不是画板内容：改名不能改 revision，否则已打开画板的下一次保存
        // 会被误判冲突，导致持续保存失败且无法切换画板。
        let revision_after: String = connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [board.board_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision_before, revision_after);
    }

    #[test]
    fn reorders_boards_within_an_artwork() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let first = create_board(&mut connection, &artwork_id, "一").unwrap();
        let second = create_board(&mut connection, &artwork_id, "二").unwrap();
        let third = create_board(&mut connection, &artwork_id, "三").unwrap();

        let revision_before: String = connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [first.board_id],
                |row| row.get(0),
            )
            .unwrap();

        let reordered = reorder_boards(
            &mut connection,
            &artwork_id,
            &[third.board_id, first.board_id, second.board_id],
        )
        .unwrap();
        let names = reordered
            .iter()
            .map(|board| board.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["三", "一", "二"]);
        assert_eq!(
            reordered
                .iter()
                .map(|board| board.sort_order)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );

        // 顺序不是画板内容：重排不能改 revision，否则已打开画板的下一次保存会被误判冲突。
        let revision_after: String = connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [first.board_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision_before, revision_after);

        // 缺项或重复 id 一律拒绝，并保持原顺序。
        assert!(reorder_boards(
            &mut connection,
            &artwork_id,
            &[third.board_id, first.board_id]
        )
        .is_err());
        assert!(reorder_boards(
            &mut connection,
            &artwork_id,
            &[first.board_id, first.board_id, second.board_id],
        )
        .is_err());
        let unchanged = list_boards(&connection, &artwork_id)
            .unwrap()
            .into_iter()
            .map(|board| board.name)
            .collect::<Vec<_>>();
        assert_eq!(unchanged, ["三", "一", "二"]);
    }

    #[test]
    fn reorder_only_covers_active_boards_of_one_artwork() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let first = create_board(&mut connection, &artwork_id, "一").unwrap();
        let second = create_board(&mut connection, &artwork_id, "二").unwrap();
        trash_board(&mut connection, second.board_id).unwrap();

        // 回收站画板不参与排序，集合只需覆盖未删除画板。
        let reordered = reorder_boards(&mut connection, &artwork_id, &[first.board_id]).unwrap();
        assert_eq!(reordered.len(), 1);
        assert_eq!(reordered[0].name, "一");

        // 把回收站画板也算进集合应被拒绝。
        assert!(reorder_boards(
            &mut connection,
            &artwork_id,
            &[first.board_id, second.board_id],
        )
        .is_err());
    }

    #[test]
    fn trash_restore_and_permanent_delete_round_trip() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "画板").unwrap();

        trash_board(&mut connection, board.board_id).unwrap();
        assert!(list_boards(&connection, &artwork_id).unwrap().is_empty());
        let trash = list_trash(&connection).unwrap();
        assert_eq!(trash.len(), 1);
        assert!(trash[0].deleted);

        let restored = restore_board(&mut connection, board.board_id).unwrap();
        assert!(!restored.deleted);
        assert_eq!(restored.sort_order, 0);

        trash_board(&mut connection, board.board_id).unwrap();
        let cleanup_ids = delete_board_permanently(&mut connection, board.board_id).unwrap();
        assert_eq!(cleanup_ids.len(), 1);
        assert!(list_trash(&connection).unwrap().is_empty());
    }

    #[test]
    fn permanent_delete_only_accepts_trashed_boards() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "活画板").unwrap();

        // 活画板不走软删除就直接硬删除会连带删掉 DDS，且没有撤销入口：拒绝。
        let error = delete_board_permanently(&mut connection, board.board_id).unwrap_err();
        assert!(error.contains("回收站"), "{error}");
        assert_eq!(list_boards(&connection, &artwork_id).unwrap().len(), 1);
    }

    #[test]
    fn empty_trash_reports_cleanup_targets() {
        let (_guard, mut connection) = test_repository();
        let artwork_id = create_test_artwork(&connection);
        let first = create_board(&mut connection, &artwork_id, "一").unwrap();
        let second = create_board(&mut connection, &artwork_id, "二").unwrap();
        trash_board(&mut connection, first.board_id).unwrap();
        trash_board(&mut connection, second.board_id).unwrap();
        let cleanup_ids = empty_trash(&mut connection).unwrap();
        assert_eq!(cleanup_ids.len(), 2);
        assert!(list_trash(&connection).unwrap().is_empty());
    }
}
