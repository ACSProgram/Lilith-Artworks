//! 画板保存与结算。
//!
//! 保存把前端的纯变换写成一个新步骤并作废 redo；结算截断未来步骤、清除仍为删除
//! 状态的条目，并把位图 DDS 的删除经 `pending_file_cleanup` 入队。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection};

use super::super::contracts::MAX_BOARD_ITEMS;
use super::boards::bump_revision;
use super::paths::{is_item_dds_file, relative_item_dds};
use super::view::{
    current_states, effective_layer, ensure_revision, open_board_context, same_state,
    validate_editable, validate_text_payload,
};
use super::{
    EditablePinBoardItem, EditableTextPayload, ImageTransform, SavePinBoardResult,
    ITEM_KIND_BITMAP, ITEM_KIND_TEXT,
};
use crate::{cleanup, storage};

pub(crate) fn save_board(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    items: &[EditablePinBoardItem],
    expected_revision: &str,
) -> Result<SavePinBoardResult, String> {
    if items.len() > MAX_BOARD_ITEMS {
        return Err("保存的画板条目数量超过限制".into());
    }
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let context = open_board_context(&transaction, root, board_id)?;
    ensure_revision(&context, expected_revision)?;
    let now_step = context.now_step;

    let mut requested = HashMap::with_capacity(items.len());
    for item in items {
        if requested.insert(item.item_id, item).is_some() {
            return Err("保存请求包含重复条目".into());
        }
    }

    let stored = current_states(&transaction, board_id, now_step)?;
    let mut kinds = HashMap::with_capacity(stored.len());
    let mut changed = HashSet::new();
    // 待写入的文字载荷直接借用请求，避免为整段文字再复制一份。
    let mut text_updates: HashMap<i64, (&EditableTextPayload, String)> = HashMap::new();
    let mut order_changed = false;
    let mut found = 0usize;
    for (item_id, state) in &stored {
        kinds.insert(*item_id, state.kind);
        let Some(editable) = requested.get(item_id) else {
            continue;
        };
        found += 1;
        validate_editable(editable, state.kind)?;
        if !same_state(state, editable) {
            changed.insert(*item_id);
        }
        if let Some(payload) = &editable.text {
            // 文字载荷只属于文字条目：位图/笔迹带载荷说明调用方误解了契约。
            let Some(stored_text) = state.text.as_ref().filter(|_| state.kind == ITEM_KIND_TEXT)
            else {
                return Err(format!("条目 {item_id} 不是文字条目，不能携带文字载荷"));
            };
            let color = validate_text_payload(
                &payload.text,
                payload.font_size,
                &payload.color,
                &payload.align,
                payload.measured_width,
                payload.measured_height,
            )?;
            // 只比较归一化后的颜色：前端可能送小写，仓库里是大写。
            if payload.text != stored_text.text
                || payload.font_size != stored_text.font_size
                || color != stored_text.color
                || payload.align != stored_text.align
                || payload.measured_width != state.width
                || payload.measured_height != state.height
            {
                changed.insert(*item_id);
                text_updates.insert(*item_id, (payload, color));
            }
        }
        order_changed |= state.sort_order != editable.order;
    }
    if found != requested.len() {
        return Err("保存请求包含不属于当前画板的条目".into());
    }
    if changed.is_empty() && !order_changed {
        return Ok(SavePinBoardResult {
            saved: false,
            revision: context.revision,
            now_step,
        });
    }

    let next_step = if changed.is_empty() {
        now_step
    } else {
        now_step.checked_add(1).ok_or("画板历史步骤已达到上限")?
    };

    // 文字载荷与变换写在同一事务里：一次保存只会推进一个 step、一次 revision。
    for (item_id, (payload, color)) in &text_updates {
        let updated = transaction
            .execute(
                "UPDATE pin_board_texts
                    SET text = ?1, font_size = ?2, color = ?3, align = ?4,
                        measured_width = ?5, measured_height = ?6
                  WHERE item_id = ?7",
                params![
                    &payload.text,
                    payload.font_size,
                    color,
                    &payload.align,
                    payload.measured_width,
                    payload.measured_height,
                    item_id
                ],
            )
            .map_err(storage::database_error)?;
        if updated != 1 {
            return Err(format!("文字条目 {item_id} 的载荷记录不存在"));
        }
    }

    // 新步骤会覆盖未来：与 Client 行为一致，写入新步骤后 redo 记录作废。
    for item_id in &changed {
        transaction
            .execute(
                "DELETE FROM pin_board_history WHERE board_id = ?1 AND item_id = ?2 AND step > ?3",
                params![board_id, item_id, now_step as i64],
            )
            .map_err(storage::database_error)?;
        let editable = requested.get(item_id).expect("checked above");
        let layer = effective_layer(
            kinds.get(item_id).copied().unwrap_or(ITEM_KIND_BITMAP),
            editable.layer,
        );
        transaction
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    board_id,
                    item_id,
                    next_step as i64,
                    editable.deleted as i64,
                    layer as i64,
                    editable.order as i64,
                    serde_json::to_string(&ImageTransform {
                        points: editable.points,
                        uv: editable.uv,
                    })
                    .map_err(|error| format!("无法序列化条目变换：{error}"))?,
                ],
            )
            .map_err(storage::database_error)?;
    }
    if order_changed {
        for (item_id, editable) in requested.iter() {
            transaction
                .execute(
                    "UPDATE pin_board_history SET sort_order = ?1
                     WHERE board_id = ?2 AND item_id = ?3 AND step = ?4",
                    params![editable.order as i64, board_id, item_id, now_step as i64],
                )
                .map_err(storage::database_error)?;
        }
    }
    if !changed.is_empty() {
        transaction
            .execute(
                "UPDATE pin_boards SET now_step = ?1, max_step = ?1 WHERE id = ?2",
                params![next_step as i64, board_id],
            )
            .map_err(storage::database_error)?;
    }
    let (_, revision) = bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(SavePinBoardResult {
        saved: true,
        revision,
        now_step: next_step,
    })
}

/// 结算画板：截断未来步骤，清除当前为删除状态的条目记录。
///
/// 被删除图片的 DDS 不再在提交前删除：事务内入队 `pending_file_cleanup`，
/// 提交成功后由调用方执行 `cleanup::run` 重放删除。若提交失败，入队随事务
/// 一起回滚，记录与 DDS 保持一致；删除失败则条目留在队列可重试，不阻断结算
/// （重放每次调用只做单遍尝试，不会循环重试阻塞进度）。
pub(crate) fn finalize_board(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    expected_revision: &str,
) -> Result<(SavePinBoardResult, Vec<String>), String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let outcome = finalize_board_in_transaction(&transaction, root, board_id, expected_revision)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(outcome)
}

fn finalize_board_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    board_id: i64,
    expected_revision: &str,
) -> Result<(SavePinBoardResult, Vec<String>), String> {
    let context = open_board_context(transaction, root, board_id)?;
    ensure_revision(&context, expected_revision)?;
    let now_step = context.now_step;
    let mut changed = context.max_step != now_step;
    let states = current_states(transaction, board_id, now_step)?;
    let mut deleted = Vec::new();
    for (item_id, state) in &states {
        if state.deleted {
            deleted.push((*item_id, state.kind, state.file_path.clone()));
            changed = true;
            continue;
        }
        let removed = transaction
            .execute(
                "DELETE FROM pin_board_history WHERE board_id = ?1 AND item_id = ?2 AND step > ?3",
                params![board_id, item_id, now_step as i64],
            )
            .map_err(storage::database_error)?;
        changed |= removed > 0;
    }
    transaction
        .execute(
            "UPDATE pin_boards SET max_step = ?1 WHERE id = ?2",
            params![now_step as i64, board_id],
        )
        .map_err(storage::database_error)?;

    if !changed {
        return Ok((
            SavePinBoardResult {
                saved: false,
                revision: context.revision,
                now_step,
            },
            Vec::new(),
        ));
    }
    let mut cleanup_ids = Vec::new();
    for (item_id, kind, file_path) in &deleted {
        transaction
            .execute(
                "DELETE FROM pin_board_items WHERE id = ?1 AND board_id = ?2",
                params![item_id, board_id],
            )
            .map_err(storage::database_error)?;
        if *kind != ITEM_KIND_BITMAP {
            continue;
        }
        // 位图载荷未落库 SHA-256，按清理体系规划用不带期望摘要的入队；
        // 非规范文件名的坏记录不参与引用，留给完整性扫描报告。
        let Some(file_path) = file_path else {
            continue;
        };
        if !is_item_dds_file(*item_id, file_path) {
            continue;
        }
        cleanup_ids.push(cleanup::enqueue_repository_file(
            transaction,
            &relative_item_dds(&context.artwork_id, board_id, *item_id),
            "pin_board_finalize",
        )?);
    }
    let (_, revision) = bump_revision(transaction, board_id)?;
    Ok((
        SavePinBoardResult {
            saved: true,
            revision,
            now_step,
        },
        cleanup_ids,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin_board::repository::boards::create_board;
    use crate::pin_board::repository::create::{create_stroke, create_text};
    use crate::pin_board::repository::fixtures::{
        board_revision, create_test_artwork, insert_deleted_state_image, insert_live_bitmap,
        scalar, scalar_for_board, shape_geometry, test_repository,
    };
    use crate::pin_board::repository::view::load_view;
    use crate::pin_board::stroke;

    #[test]
    fn stroke_items_stay_on_the_annotation_layer_when_saved() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "图层板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (_, stroke_ids) = create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_RECT,
            "#FF0000",
            4.0,
            80.0,
            60.0,
            &shape_geometry(stroke::TOOL_RECT, 2),
            100.0,
            100.0,
            &revision,
        )
        .unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "同板文字",
            20.0,
            "#00FF00",
            "left",
            80.0,
            30.0,
            500.0,
            500.0,
            &revision,
        )
        .unwrap();

        // 前端把两者都声明成底层；笔迹必须被强制拉回标注层，文字照旧。
        let items = view
            .items
            .iter()
            .map(|item| EditablePinBoardItem {
                item_id: item.item_id,
                order: item.order,
                layer: 0,
                deleted: false,
                points: item.points,
                uv: item.uv,
                text: None,
            })
            .collect::<Vec<_>>();
        let revision = board_revision(&connection, board.board_id);
        let saved = save_board(&mut connection, &root, board.board_id, &items, &revision).unwrap();
        assert!(saved.saved);

        let layer_of = |connection: &Connection, item_id: i64| -> i64 {
            connection
                .query_row(
                    "SELECT layer FROM pin_board_history
                     WHERE item_id = ?1 ORDER BY step DESC LIMIT 1",
                    [item_id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(layer_of(&connection, stroke_ids[0]), 3);
        assert_eq!(layer_of(&connection, text_ids[0]), 0);
        let stored = load_view(&connection, &root, board.board_id).unwrap();
        assert_eq!(
            stored
                .items
                .iter()
                .find(|item| item.item_id == stroke_ids[0])
                .unwrap()
                .layer,
            3
        );

        // 只把图层写错、几何没变时不算改动：笔迹被强制拉回 3，与库里一致，
        // 因此不产生新步骤（前端传来 0/1/2 都不会让笔迹漂移出标注层）。
        let latest = load_view(&connection, &root, board.board_id).unwrap();
        let stroke_item = latest
            .items
            .iter()
            .find(|item| item.item_id == stroke_ids[0])
            .unwrap();
        let only_stroke = vec![EditablePinBoardItem {
            item_id: stroke_item.item_id,
            order: stroke_item.order,
            layer: 2,
            deleted: false,
            points: stroke_item.points,
            uv: stroke_item.uv,
            text: None,
        }];
        let revision = board_revision(&connection, board.board_id);
        let saved = save_board(
            &mut connection,
            &root,
            board.board_id,
            &only_stroke,
            &revision,
        )
        .unwrap();
        assert!(!saved.saved, "笔迹的图层请求应被忽略，不算改动");
        assert_eq!(layer_of(&connection, stroke_ids[0]), 3);

        // 超出标注层的图层值仍然拒绝：3 是笔迹的固定位置，不是可调范围。
        let mut invalid = only_stroke;
        invalid[0].layer = 4;
        let revision = board_revision(&connection, board.board_id);
        let error =
            save_board(&mut connection, &root, board.board_id, &invalid, &revision).unwrap_err();
        assert!(error.contains("图层无效"), "{error}");
    }

    #[test]
    fn bitmap_and_text_items_cannot_claim_the_annotation_layer() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "图层板").unwrap();
        let bitmap_id = insert_live_bitmap(
            &connection,
            board.board_id,
            &root,
            &artwork_id,
            0.0,
            100.0,
            40.0,
            20.0,
        );
        let revision = board_revision(&connection, board.board_id);
        let (_, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "文字",
            20.0,
            "#00FF00",
            "left",
            80.0,
            30.0,
            300.0,
            200.0,
            &revision,
        )
        .unwrap();
        let view = load_view(&connection, &root, board.board_id).unwrap();

        // 3 是笔迹的固化位，位图与文字只用 0–2：写成 3 必须被拒，而不是静默落库
        // 后与「标注层只装笔迹」的渲染约定自相矛盾。
        for item_id in [bitmap_id, text_ids[0]] {
            let item = view
                .items
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap();
            let items = vec![EditablePinBoardItem {
                item_id: item.item_id,
                order: item.order,
                layer: stroke::STROKE_LAYER,
                deleted: false,
                points: item.points,
                uv: item.uv,
                text: None,
            }];
            let revision = board_revision(&connection, board.board_id);
            let error =
                save_board(&mut connection, &root, board.board_id, &items, &revision).unwrap_err();
            assert!(error.contains("图层无效"), "{error}");
        }
    }

    #[test]
    fn finalize_removes_stroke_rows_without_touching_dds_cleanup() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "结算板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, item_ids) = create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_ARROW,
            "#123456",
            3.0,
            50.0,
            50.0,
            &shape_geometry(stroke::TOOL_ARROW, 1),
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let stroke_item = view
            .items
            .iter()
            .find(|item| item.item_id == item_ids[0])
            .unwrap();

        // 标记删除 → 结算：笔迹记录随条目级联删除，且没有 DDS 清理入队。
        let revision = board_revision(&connection, board.board_id);
        let items = vec![EditablePinBoardItem {
            item_id: stroke_item.item_id,
            order: stroke_item.order,
            layer: stroke_item.layer,
            deleted: true,
            points: stroke_item.points,
            uv: stroke_item.uv,
            text: None,
        }];
        let saved = save_board(&mut connection, &root, board.board_id, &items, &revision).unwrap();
        assert!(saved.saved);
        let revision = board_revision(&connection, board.board_id);
        let (_, cleanup_ids) =
            finalize_board(&mut connection, &root, board.board_id, &revision).unwrap();
        assert!(cleanup_ids.is_empty(), "笔迹没有实体文件，不应入队清理");
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pin_board_items"),
            0
        );
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pin_board_strokes"),
            0
        );
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pending_file_cleanup"),
            0
        );
    }

    #[test]
    fn finalize_enqueues_deleted_dds_and_replay_removes_the_file() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "结算板").unwrap();
        let dds_path = insert_deleted_state_image(&connection, board.board_id, &root, &artwork_id);
        let revision = board_revision(&connection, board.board_id);

        let (result, cleanup_ids) =
            finalize_board(&mut connection, &root, board.board_id, &revision).unwrap();
        assert!(result.saved);
        assert_eq!(cleanup_ids.len(), 1);

        // 提交成功后、重放前：DDS 仍在磁盘上，队列恰好登记一条。
        assert!(dds_path.is_file());
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pending_file_cleanup"),
            1
        );

        let report = crate::cleanup::run(&root, &cleanup_ids).unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.pending_count, 0);
        assert!(!dds_path.exists());

        // 结算后条目记录与历史已删除。
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
                board.board_id
            ),
            0
        );
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_history WHERE board_id = ?1",
                board.board_id
            ),
            0
        );
    }

    #[test]
    fn finalize_rollback_keeps_the_image_record_and_the_dds_file() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "回滚板").unwrap();
        let dds_path = insert_deleted_state_image(&connection, board.board_id, &root, &artwork_id);
        let revision = board_revision(&connection, board.board_id);

        // 模拟提交失败：结算事务中途回滚，入队与记录删除一起消失。
        {
            let transaction = connection.transaction().unwrap();
            let (_, cleanup_ids) =
                finalize_board_in_transaction(&transaction, &root, board.board_id, &revision)
                    .unwrap();
            assert_eq!(cleanup_ids.len(), 1);
        }

        // 记录回滚而文件保留，队列无残留条目。
        assert!(dds_path.is_file());
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
                board.board_id
            ),
            1
        );
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pending_file_cleanup"),
            0
        );
    }

    #[test]
    fn finalize_cleanup_entry_survives_a_failed_replay_and_can_be_retried() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "重试板").unwrap();
        let dds_path = insert_deleted_state_image(&connection, board.board_id, &root, &artwork_id);
        let revision = board_revision(&connection, board.board_id);

        let (_, cleanup_ids) =
            finalize_board(&mut connection, &root, board.board_id, &revision).unwrap();

        // 用「文件被数据库引用」构造一次可重试的失败：重放拒绝删除，条目留在队列。
        // 期望的相对路径由真实文件反推——夹具按 `<item-id>.dds` 命名，文件名即 id。
        let item_id = dds_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<i64>().ok())
            .unwrap();
        let relative = relative_item_dds(&artwork_id, board.board_id, item_id);
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO final_artifacts
                   (id, branch_id, history_id, source_path, source_sha256, media_type,
                    byte_size, created_ms)
                 VALUES ('artifact', 'branch', 'history', '{relative}',
                         '0000000000000000000000000000000000000000000000000000000000000000',
                         'image/jpeg', 8, 0);"
            ))
            .unwrap();
        let failed = crate::cleanup::run(&root, &cleanup_ids).unwrap();
        assert_eq!(failed.failures.len(), 1);
        assert!(dds_path.is_file());
        assert_eq!(failed.pending_count, 1);

        // 解除引用后单次重放即删除：不循环重试、不阻塞后续流程。
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DELETE FROM final_artifacts WHERE id = 'artifact';",
            )
            .unwrap();
        let retried = crate::cleanup::run(&root, &cleanup_ids).unwrap();
        assert!(retried.failures.is_empty());
        assert!(!dds_path.exists());
        assert_eq!(retried.pending_count, 0);
    }

    fn text_payload(
        text: &str,
        font_size: f64,
        color: &str,
        align: &str,
        width: f64,
        height: f64,
    ) -> EditableTextPayload {
        EditableTextPayload {
            text: text.into(),
            font_size,
            color: color.into(),
            align: align.into(),
            measured_width: width,
            measured_height: height,
        }
    }

    fn editable_from(
        item: &crate::pin_board::repository::view::PinBoardItem,
        text: Option<EditableTextPayload>,
    ) -> EditablePinBoardItem {
        EditablePinBoardItem {
            item_id: item.item_id,
            order: item.order,
            layer: item.layer,
            deleted: item.deleted,
            points: item.points,
            uv: item.uv,
            text,
        }
    }

    fn text_row(connection: &Connection) -> (String, f64, String, String, f64, f64) {
        connection
            .query_row(
                "SELECT text, font_size, color, align, measured_width, measured_height
                   FROM pin_board_texts",
                [],
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
            .unwrap()
    }

    #[test]
    fn saves_text_payload_and_advances_one_step_and_revision() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "文字板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "原文",
            20.0,
            "#00FF00",
            "center",
            80.0,
            30.0,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();
        let before_step = view.now_step;

        let revision = board_revision(&connection, board.board_id);
        let saved = save_board(
            &mut connection,
            &root,
            board.board_id,
            &[editable_from(
                item,
                Some(text_payload("改后", 26.0, "#a1b2c3", "left", 100.0, 40.0)),
            )],
            &revision,
        )
        .unwrap();
        assert!(saved.saved);
        assert_eq!(saved.now_step, before_step + 1);
        assert_ne!(saved.revision, revision);

        // 颜色按仓库口径归一化；字号、对齐与度量随文字载荷一起更新。
        assert_eq!(
            text_row(&connection),
            (
                "改后".into(),
                26.0,
                "#A1B2C3".into(),
                "left".into(),
                100.0,
                40.0
            )
        );
        let reloaded = load_view(&connection, &root, board.board_id).unwrap();
        let text = reloaded
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();
        assert_eq!(text.width, 100.0);
        assert_eq!(text.height, 40.0);

        // 只有文字变化时也写一个 step，保持「保存推进 step、作废 redo」的语义。
        // 新建文字本身留下 step 0 的删除态默认节点与添加步骤两条记录，再加上这里的一条。
        let history_steps = scalar_for_board(
            &connection,
            "SELECT COUNT(*) FROM pin_board_history WHERE board_id = ?1",
            board.board_id,
        );
        assert_eq!(history_steps, 3);
    }

    #[test]
    fn merges_text_and_transform_changes_into_one_step() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "合并板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "原文",
            20.0,
            "#00FF00",
            "center",
            80.0,
            30.0,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();
        let mut editable = editable_from(
            item,
            Some(text_payload("改后", 20.0, "#00FF00", "center", 90.0, 30.0)),
        );
        editable.points = [[10.0, 10.0], [100.0, 10.0], [100.0, 40.0], [10.0, 40.0]];

        let revision = board_revision(&connection, board.board_id);
        let saved = save_board(
            &mut connection,
            &root,
            board.board_id,
            &[editable],
            &revision,
        )
        .unwrap();
        assert!(saved.saved);
        assert_eq!(saved.now_step, view.now_step + 1);
        assert_ne!(saved.revision, revision);
    }

    #[test]
    fn rejects_text_payloads_on_non_text_items() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "位图板").unwrap();
        let bitmap_id = insert_live_bitmap(
            &connection,
            board.board_id,
            &root,
            &artwork_id,
            0.0,
            100.0,
            40.0,
            20.0,
        );
        let view = load_view(&connection, &root, board.board_id).unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == bitmap_id)
            .unwrap();
        let revision = board_revision(&connection, board.board_id);

        let error = save_board(
            &mut connection,
            &root,
            board.board_id,
            &[editable_from(
                item,
                Some(text_payload(
                    "不该出现",
                    20.0,
                    "#FFFFFF",
                    "left",
                    80.0,
                    20.0,
                )),
            )],
            &revision,
        )
        .unwrap_err();
        assert!(error.contains("不是文字条目"), "{error}");
    }

    #[test]
    fn rejects_invalid_text_payloads_and_keeps_the_stored_row() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "校验板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "原文",
            20.0,
            "#00FF00",
            "center",
            80.0,
            30.0,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();
        let original = text_row(&connection);

        let cases = [
            text_payload("   ", 20.0, "#00FF00", "center", 80.0, 30.0),
            text_payload(&"字".repeat(5001), 20.0, "#00FF00", "center", 80.0, 30.0),
            text_payload("改后", 0.0, "#00FF00", "center", 80.0, 30.0),
            text_payload("改后", 20.0, "green", "center", 80.0, 30.0),
            text_payload("改后", 20.0, "#00FF00", "justify", 80.0, 30.0),
            text_payload("改后", 20.0, "#00FF00", "center", 8193.0, 30.0),
        ];
        for payload in cases {
            let revision = board_revision(&connection, board.board_id);
            save_board(
                &mut connection,
                &root,
                board.board_id,
                &[editable_from(item, Some(payload))],
                &revision,
            )
            .unwrap_err();
            assert_eq!(text_row(&connection), original);
        }
    }

    #[test]
    fn identical_text_payload_does_not_create_a_step() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "同值板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "原文",
            20.0,
            "#00FF00",
            "center",
            80.0,
            30.0,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();

        let revision = board_revision(&connection, board.board_id);
        let saved = save_board(
            &mut connection,
            &root,
            board.board_id,
            &[editable_from(
                item,
                Some(text_payload("原文", 20.0, "#00ff00", "center", 80.0, 30.0)),
            )],
            &revision,
        )
        .unwrap();
        assert!(!saved.saved);
        assert_eq!(saved.revision, revision);
        assert_eq!(saved.now_step, view.now_step);
    }

    #[test]
    fn text_payload_requires_the_current_revision() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "冲突板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (view, text_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "原文",
            20.0,
            "#00FF00",
            "center",
            80.0,
            30.0,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == text_ids[0])
            .unwrap();
        let stale = "stale-revision".to_string();

        let error = save_board(
            &mut connection,
            &root,
            board.board_id,
            &[editable_from(
                item,
                Some(text_payload("改后", 20.0, "#00FF00", "center", 80.0, 30.0)),
            )],
            &stale,
        )
        .unwrap_err();
        assert!(error.contains("其他窗口"), "{error}");
        assert_eq!(text_row(&connection).0, "原文");
    }
}
