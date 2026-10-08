//! 素材板 SQLite 持久化与画板文件布局。
//!
//! 存储布局（schema v2 起三张表，见 `library/schema.rs`）：
//! - `pin_boards`：按 Artwork 平铺一层画板，`deleted_at` 为回收站软删除；
//! - `pin_board_images`：图片记录，DDS 实体存于
//!   `artworks/<artwork-id>/boards/<board-id>/<image-id>.dds`；
//! - `pin_board_history`：每张图片按 step 的历史状态（撤销/恢复持久化）。
//!
//! 本模块只通过 `storage.rs` 打开连接，不读取 ChunkFile；回收站永久删除经
//! `cleanup.rs` 队列清理 DDS 目录。

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::dds;
use crate::{cleanup, storage};

pub(crate) const BOARD_DIRECTORY: &str = "boards";
/// 单个 Artwork 的画板数量上限（规划确认的规模假设）。
pub(crate) const MAX_BOARDS_PER_ARTWORK: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardSummary {
    board_id: i64,
    artwork_id: String,
    artwork_title: String,
    name: String,
    sort_order: i64,
    deleted: bool,
    deleted_at: Option<i64>,
    image_count: i64,
    created_ms: i64,
    updated_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardImage {
    board_id: i64,
    image_id: i64,
    width: u32,
    height: u32,
    order: u64,
    layer: u8,
    deleted: bool,
    points: [[f64; 2]; 4],
    uv: [[f64; 2]; 4],
    available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardView {
    board_id: i64,
    name: String,
    images: Vec<PinBoardImage>,
    missing_textures: usize,
    revision: String,
    now_step: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditablePinBoardImage {
    image_id: i64,
    order: u64,
    layer: u8,
    deleted: bool,
    points: [[f64; 2]; 4],
    uv: [[f64; 2]; 4],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePinBoardResult {
    saved: bool,
    revision: String,
    now_step: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardClipboardImage {
    source_board_id: i64,
    source_image_id: i64,
    width: u32,
    height: u32,
    #[serde(default)]
    display_width: f64,
    #[serde(default)]
    display_height: f64,
    uv: [[f64; 2]; 4],
}

/// 画板加载/保存所需的最小上下文：确认画板存在且未在回收站，
/// 并解析出仓库内的画板目录。
pub(crate) struct BoardContext {
    pub(crate) artwork_id: String,
    pub(crate) name: String,
    pub(crate) now_step: u64,
    pub(crate) max_step: u64,
    pub(crate) revision: String,
    pub(crate) directory: PathBuf,
}

pub(crate) fn board_relative_path(artwork_id: &str, board_id: i64) -> String {
    format!("artworks/{artwork_id}/{BOARD_DIRECTORY}/{board_id}")
}

/// 反向解析画板 DDS 的仓库相对路径
/// `artworks/<artwork-id>/<BOARD_DIRECTORY>/<board-id>/<image-id>.dds`，返回
/// `(artwork_id, board_id, image_id)`；不匹配时返回 `None`。它与 `board_relative_path`
/// 是同一布局的正反两面，供 `cleanup` 的画板 DDS 反向引用检查使用，避免调用方另行
/// 硬编码路径形状。
pub(crate) fn parse_board_dds_path(path: &str) -> Option<(&str, i64, i64)> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "artworks" || parts[2] != BOARD_DIRECTORY {
        return None;
    }
    let board_id = parts[3].parse::<i64>().ok()?;
    let image_id = parts[4].strip_suffix(".dds")?.parse::<i64>().ok()?;
    Some((parts[1], board_id, image_id))
}

/// 画板 DDS 文件名：`<image-id>.dds`（image id 为 SQLite 自增正整数）。扫描与
/// 完整性检查共用同一命名判定，避免两处漂移。
pub(crate) fn is_dds_name(name: &str) -> bool {
    name.strip_suffix(".dds")
        .is_some_and(|stem| stem.parse::<i64>().map_or(false, |id| id > 0))
}

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
    let directory = board_directory(Path::new(""), artwork_id, board_id);
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建画板目录：{error}"))?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(PinBoardSummary {
        board_id,
        artwork_id: artwork_id.to_owned(),
        artwork_title,
        name,
        sort_order: next_order,
        deleted: false,
        deleted_at: None,
        image_count: 0,
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
    // Artwork 进入项目回收站时其画板随之隐藏（trashed_ms 非空）。
    let sql = format!(
        "SELECT b.id, b.artwork_id, n.title, b.name, b.sort_order, b.deleted_at,
                (SELECT COUNT(*) FROM pin_board_images i WHERE i.board_id = b.id),
                b.created_ms, b.updated_ms
           FROM pin_boards b
           JOIN artworks a ON a.id = b.artwork_id
           JOIN library_nodes n ON n.id = a.id
          WHERE n.trashed_ms IS NULL
            {} {} ORDER BY {}",
        if trash {
            "AND b.deleted_at IS NOT NULL"
        } else {
            "AND b.artwork_id = ?1 AND b.deleted_at IS NULL"
        },
        "",
        if trash {
            "b.deleted_at DESC, b.id"
        } else {
            "b.sort_order, b.id"
        }
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
        image_count: row.get(6)?,
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
    let next_order: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(b.sort_order) + 1, 0) FROM pin_boards b
             JOIN artworks a ON a.id = b.artwork_id
             JOIN library_nodes n ON n.id = a.id
             WHERE b.artwork_id = (SELECT artwork_id FROM pin_boards WHERE id = ?1)
               AND b.deleted_at IS NULL",
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
pub(crate) fn delete_board_permanently(
    connection: &mut Connection,
    board_id: i64,
) -> Result<Vec<String>, String> {
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let context = trash_board_context(&transaction, board_id)?;
    transaction
        .execute("DELETE FROM pin_boards WHERE id = ?1", [board_id])
        .map_err(storage::database_error)?;
    let cleanup_ids = vec![cleanup::enqueue_repository_directory(
        &transaction,
        &board_relative_path(&context.artwork_id, board_id),
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
        let context = trash_board_context(&transaction, board_id)?;
        transaction
            .execute("DELETE FROM pin_boards WHERE id = ?1", [board_id])
            .map_err(storage::database_error)?;
        cleanup_ids.push(cleanup::enqueue_repository_directory(
            &transaction,
            &board_relative_path(&context.artwork_id, board_id),
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

fn board_directory(root: &Path, artwork_id: &str, board_id: i64) -> PathBuf {
    root.join("artworks")
        .join(artwork_id)
        .join(BOARD_DIRECTORY)
        .join(board_id.to_string())
}

fn board_summary(connection: &Connection, board_id: i64) -> Result<PinBoardSummary, String> {
    connection
        .query_row(
            "SELECT b.id, b.artwork_id, n.title, b.name, b.sort_order, b.deleted_at,
                    (SELECT COUNT(*) FROM pin_board_images i WHERE i.board_id = b.id),
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

fn require_active_board(connection: &Connection, board_id: i64) -> Result<(), String> {
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

fn trash_board_context(connection: &Connection, board_id: i64) -> Result<BoardContext, String> {
    let artwork_id: String = connection
        .query_row(
            "SELECT artwork_id FROM pin_boards WHERE id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("回收站中不存在该画板")?;
    Ok(BoardContext {
        artwork_id,
        name: String::new(),
        now_step: 0,
        max_step: 0,
        revision: String::new(),
        directory: PathBuf::new(),
    })
}

/// 画板修订号：写入时取 `max(now_ms, 当前 updated_ms + 1)`，保证每次写库后
/// revision 严格单调变化，用于保存冲突检测。
fn revision_value(board_id: i64, updated_ms: i64) -> String {
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(b"lilith-artworks:pin-board:");
    hasher.update(board_id.to_le_bytes());
    hasher.update(updated_ms.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// 在事务中推进画板 updated_ms 并返回新修订号。
fn bump_revision(
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

fn initial_revision() -> String {
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(b"lilith-artworks:pin-board:initial");
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// 画板内容（图片与历史）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ImageTransform {
    pub(crate) points: [[f64; 2]; 4],
    pub(crate) uv: [[f64; 2]; 4],
}

#[derive(Debug, Clone)]
struct ImageState {
    deleted: bool,
    layer: u8,
    sort_order: u64,
    transform: ImageTransform,
}

/// 内容操作使用的画板上下文：只接受未在回收站的画板。
pub(crate) fn open_board_context(
    connection: &Connection,
    root: &Path,
    board_id: i64,
) -> Result<BoardContext, String> {
    require_active_board(connection, board_id)?;
    let row: Option<(String, String, i64, i64, String)> = connection
        .query_row(
            "SELECT artwork_id, name, now_step, max_step, revision
               FROM pin_boards WHERE id = ?1",
            [board_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, name, now_step, max_step, revision) = row.ok_or("画板不存在")?;
    let directory = board_directory(root, &artwork_id, board_id);
    Ok(BoardContext {
        artwork_id,
        name,
        now_step: now_step as u64,
        max_step: max_step as u64,
        revision,
        directory,
    })
}

fn current_states(
    connection: &Connection,
    board_id: i64,
    now_step: u64,
) -> Result<Vec<(i64, u32, u32, ImageState)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT i.id, i.width, i.height, h.deleted, h.layer, h.sort_order, h.transform_json
               FROM pin_board_images i
               JOIN pin_board_history h
                 ON h.board_id = i.board_id AND h.image_id = i.id
                AND h.step = (SELECT MAX(step) FROM pin_board_history
                              WHERE board_id = i.board_id AND image_id = i.id AND step <= ?2)
              WHERE i.board_id = ?1",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map(params![board_id, now_step as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)? as u32,
                row.get::<_, i64>(2)? as u32,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, i64>(4)? as u8,
                row.get::<_, i64>(5)? as u64,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?
        .into_iter()
        .map(
            |(image_id, width, height, deleted, layer, sort_order, transform_json)| {
                let transform: ImageTransform = serde_json::from_str(&transform_json)
                    .map_err(|error| format!("画板图片变换数据无效：{error}"))?;
                Ok((
                    image_id,
                    width,
                    height,
                    ImageState {
                        deleted,
                        layer,
                        sort_order,
                        transform,
                    },
                ))
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    Ok(rows)
}

pub(crate) fn load_view(
    connection: &Connection,
    root: &Path,
    board_id: i64,
) -> Result<PinBoardView, String> {
    let context = open_board_context(connection, root, board_id)?;
    let states = current_states(connection, board_id, context.now_step)?;
    let mut images = Vec::with_capacity(states.len());
    for (image_id, width, height, state) in states {
        let file = context.directory.join(format!("{image_id}.dds"));
        images.push(PinBoardImage {
            board_id,
            image_id,
            width,
            height,
            order: state.sort_order,
            layer: state.layer,
            deleted: state.deleted,
            points: state.transform.points,
            uv: state.transform.uv,
            available: file.is_file(),
        });
    }
    images.sort_by(|left, right| {
        left.layer
            .cmp(&right.layer)
            .then_with(|| right.order.cmp(&left.order))
    });
    let missing_textures = images
        .iter()
        .filter(|image| !image.deleted && !image.available)
        .count();
    Ok(PinBoardView {
        board_id,
        name: context.name,
        images,
        missing_textures,
        revision: context.revision,
        now_step: context.now_step,
    })
}

fn validate_editable(image: &EditablePinBoardImage) -> Result<(), String> {
    if image.layer > 2 {
        return Err(format!("图片 {} 的图层无效", image.image_id));
    }
    if image
        .points
        .iter()
        .chain(image.uv.iter())
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(format!("图片 {} 包含无效坐标", image.image_id));
    }
    let edge = |left: [f64; 2], right: [f64; 2]| (right[0] - left[0]).hypot(right[1] - left[1]);
    if edge(image.points[0], image.points[1]) < 0.000_001
        || edge(image.points[0], image.points[3]) < 0.000_001
    {
        return Err(format!("图片 {} 的尺寸无效", image.image_id));
    }
    Ok(())
}

fn same_state(left: &ImageState, right: &EditablePinBoardImage) -> bool {
    left.deleted == right.deleted
        && left.layer == right.layer
        && left.transform.points == right.points
        && left.transform.uv == right.uv
}

pub(crate) fn save_board(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    images: &[EditablePinBoardImage],
    expected_revision: &str,
) -> Result<SavePinBoardResult, String> {
    if images.len() > dds::MAX_BOARD_IMAGES {
        return Err("保存的画板图片数量超过限制".into());
    }
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let context = open_board_context(&transaction, root, board_id)?;
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
    let now_step = context.now_step;

    let mut requested = std::collections::HashMap::with_capacity(images.len());
    for image in images {
        validate_editable(image)?;
        if requested.insert(image.image_id, image).is_some() {
            return Err("保存请求包含重复图片".into());
        }
    }

    let stored = current_states(&transaction, board_id, now_step)?;
    let mut changed = std::collections::HashSet::new();
    let mut order_changed = false;
    let mut found = 0usize;
    for (image_id, _, _, state) in &stored {
        let Some(editable) = requested.get(image_id) else {
            continue;
        };
        found += 1;
        if !same_state(state, editable) {
            changed.insert(*image_id);
        }
        order_changed |= state.sort_order != editable.order;
    }
    if found != requested.len() {
        return Err("保存请求包含不属于当前画板的图片".into());
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

    // 新步骤会覆盖未来：与 Client 行为一致，写入新步骤后 redo 记录作废。
    for image_id in &changed {
        transaction
            .execute(
                "DELETE FROM pin_board_history WHERE board_id = ?1 AND image_id = ?2 AND step > ?3",
                params![board_id, image_id, now_step as i64],
            )
            .map_err(storage::database_error)?;
        let editable = requested.get(image_id).expect("checked above");
        transaction
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    board_id,
                    image_id,
                    next_step as i64,
                    editable.deleted as i64,
                    editable.layer as i64,
                    editable.order as i64,
                    serde_json::to_string(&ImageTransform {
                        points: editable.points,
                        uv: editable.uv,
                    })
                    .map_err(|error| format!("无法序列化图片变换：{error}"))?,
                ],
            )
            .map_err(storage::database_error)?;
    }
    if order_changed {
        for (image_id, editable) in requested.iter() {
            transaction
                .execute(
                    "UPDATE pin_board_history SET sort_order = ?1
                     WHERE board_id = ?2 AND image_id = ?3 AND step = ?4",
                    params![editable.order as i64, board_id, image_id, now_step as i64],
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

/// 结算画板：截断未来步骤，清除当前为删除状态的图片记录。
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
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
    let now_step = context.now_step;
    let mut changed = context.max_step != now_step;
    let states = current_states(transaction, board_id, now_step)?;
    let mut deleted_ids = Vec::new();
    for (image_id, _, _, _) in &states {
        let state = &states
            .iter()
            .find(|(id, _, _, _)| id == image_id)
            .expect("same collection")
            .3;
        if state.deleted {
            deleted_ids.push(*image_id);
            changed = true;
            continue;
        }
        let removed = transaction
            .execute(
                "DELETE FROM pin_board_history WHERE board_id = ?1 AND image_id = ?2 AND step > ?3",
                params![board_id, image_id, now_step as i64],
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
    for image_id in &deleted_ids {
        transaction
            .execute(
                "DELETE FROM pin_board_images WHERE id = ?1 AND board_id = ?2",
                params![image_id, board_id],
            )
            .map_err(storage::database_error)?;
        // pin_board_images 未落库 SHA-256，按清理体系规划用不带期望摘要的入队。
        let path = format!(
            "{}/{image_id}.dds",
            board_relative_path(&context.artwork_id, board_id)
        );
        cleanup_ids.push(cleanup::enqueue_repository_file(
            transaction,
            &path,
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
// ---------------------------------------------------------------------------
// 新增图片（粘贴/导入）与读取
// ---------------------------------------------------------------------------

/// 新增图片前的公共步骤：校验修订、作废 redo、预留顺序与 image id。
struct MutationSetup {
    board_directory: PathBuf,
    next_step: u64,
    image_ids: Vec<i64>,
}

fn begin_image_mutation(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    board_id: i64,
    expected_revision: &str,
    count: usize,
) -> Result<MutationSetup, String> {
    let context = open_board_context(transaction, root, board_id)?;
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
    let image_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM pin_board_images WHERE board_id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if image_count as usize + count > dds::MAX_BOARD_IMAGES {
        return Err("画板图片数量超过限制".into());
    }
    let next_step = context
        .now_step
        .checked_add(1)
        .ok_or("画板历史步骤已达到上限")?;
    // 新步骤会覆盖未来：与 Client 行为一致，写入新步骤后 redo 记录作废。
    transaction
        .execute(
            "DELETE FROM pin_board_history WHERE board_id = ?1 AND step > ?2",
            params![board_id, context.now_step as i64],
        )
        .map_err(storage::database_error)?;
    // 新图片统一放入中层并占据最优先的一组顺序。
    transaction
        .execute(
            "UPDATE pin_board_history SET sort_order = sort_order + ?1
             WHERE board_id = ?2 AND step = ?3",
            params![count as i64, board_id, context.now_step as i64],
        )
        .map_err(storage::database_error)?;
    let now = storage::now_ms()?;
    let mut image_ids = Vec::with_capacity(count);
    for _ in 0..count {
        transaction
            .execute(
                "INSERT INTO pin_board_images (board_id, file_path, width, height, created_ms)
                 VALUES (?1, '', 1, 1, ?2)",
                params![board_id, now],
            )
            .map_err(storage::database_error)?;
        image_ids.push(transaction.last_insert_rowid());
    }
    transaction
        .execute(
            "UPDATE pin_boards SET now_step = ?1, max_step = ?1 WHERE id = ?2",
            params![next_step as i64, board_id],
        )
        .map_err(storage::database_error)?;
    Ok(MutationSetup {
        board_directory: context.directory,
        next_step,
        image_ids,
    })
}

fn record_initial_state(
    transaction: &rusqlite::Transaction<'_>,
    board_id: i64,
    image_id: i64,
    step: u64,
    order: u64,
    editable: &EditablePinBoardImage,
) -> Result<(), String> {
    validate_editable(editable)?;
    let transform = serde_json::to_string(&ImageTransform {
        points: editable.points,
        uv: editable.uv,
    })
    .map_err(|error| format!("无法序列化图片变换：{error}"))?;
    // 沿用旧 step 模型：先写入 step 0 的删除态默认节点，再写入添加步骤。
    transaction
        .execute(
            "INSERT INTO pin_board_history
               (board_id, image_id, step, deleted, layer, sort_order, transform_json)
             VALUES (?1, ?2, 0, 1, ?3, ?4, ?5)",
            params![
                board_id,
                image_id,
                editable.layer as i64,
                order as i64,
                transform
            ],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "INSERT INTO pin_board_history
               (board_id, image_id, step, deleted, layer, sort_order, transform_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                board_id,
                image_id,
                step as i64,
                editable.deleted as i64,
                editable.layer as i64,
                order as i64,
                transform,
            ],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

fn finalize_image_row(
    transaction: &rusqlite::Transaction<'_>,
    board_id: i64,
    image_id: i64,
    width: u32,
    height: u32,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE pin_board_images SET file_path = ?1, width = ?2, height = ?3
             WHERE id = ?4 AND board_id = ?5",
            params![format!("{image_id}.dds"), width, height, image_id, board_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

fn source_dds_path(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    board_id: i64,
    image_id: i64,
) -> Result<PathBuf, String> {
    let context = open_board_context(transaction, root, board_id)?;
    let path = context.directory.join(format!("{image_id}.dds"));
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("无法访问源 DDS 图片：{error}"))?;
    if !canonical.starts_with(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        return Err("源 DDS 图片路径超出仓库范围".into());
    }
    Ok(canonical)
}

/// 粘贴画板图片：从源画板复制 DDS，按导入归一化后写入目标画板。
pub(crate) fn paste_images(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    images: &[PinBoardClipboardImage],
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if images.is_empty() {
        return Err("没有可粘贴的图片".into());
    }
    if images.iter().any(|image| {
        image.width == 0
            || image.height == 0
            || dds::checked_pixel_count(image.width, image.height)
                .map_or(true, |pixels| pixels > dds::MAX_DDS_SOURCE_PIXELS)
            || !image.display_width.is_finite()
            || !image.display_height.is_finite()
            || image.display_width <= 0.0
            || image.display_height <= 0.0
    }) {
        return Err("粘贴图片尺寸或像素数超过限制".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() || !gap.is_finite() {
        return Err("粘贴位置无效".into());
    }

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let setup = begin_image_mutation(
        &transaction,
        root,
        board_id,
        expected_revision,
        images.len(),
    )?;
    let source_paths = images
        .iter()
        .map(|image| {
            source_dds_path(
                &transaction,
                root,
                image.source_board_id,
                image.source_image_id,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut created_files = dds::CreatedFiles::default();
    let mut prepared = Vec::with_capacity(images.len());
    for (source_path, clipboard) in source_paths.iter().zip(images) {
        let bytes = dds::read_limited(source_path, dds::MAX_DDS_BYTES, "源 DDS 图片")?;
        let (dds_bytes, width, height) = dds::prepare_dds_import(&bytes)?;
        prepared.push((dds_bytes, width, height, clipboard.uv));
    }
    let sizes = prepared
        .iter()
        .map(|(_, width, height, _)| (*width as f64, *height as f64))
        .collect::<Vec<_>>();
    let points = dds::imported_points(&sizes, center_x, center_y, gap.max(0.0));
    let mut image_ids = Vec::with_capacity(prepared.len());
    for (index, ((image_id, points), (dds_bytes, width, height, uv))) in
        setup.image_ids.iter().zip(points).zip(prepared).enumerate()
    {
        let image_id = *image_id;
        let destination = setup.board_directory.join(format!("{image_id}.dds"));
        dds::persist_dds_file(&destination, &dds_bytes, "粘贴 DDS 图片")?;
        created_files.push(destination);
        let editable = EditablePinBoardImage {
            image_id,
            order: index as u64,
            layer: 1,
            deleted: false,
            points,
            uv,
        };
        record_initial_state(
            &transaction,
            board_id,
            image_id,
            setup.next_step,
            index as u64,
            &editable,
        )?;
        finalize_image_row(&transaction, board_id, image_id, width, height)?;
        image_ids.push(image_id);
    }
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, image_ids))
}

/// 按路径导入图片：解码为 BC7 DDS 后写入目标画板。
pub(crate) fn import_images(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    paths: &[String],
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: &str,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(PinBoardView, Vec<i64>), String> {
    if paths.is_empty() || paths.len() > dds::MAX_IMPORT_FILES {
        return Err("导入文件数量无效".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() || !gap.is_finite() {
        return Err("导入位置无效".into());
    }
    let total = paths.len();
    on_progress(0, total);

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let setup = begin_image_mutation(&transaction, root, board_id, expected_revision, total)?;
    let mut created_files = dds::CreatedFiles::default();
    let mut imported: Vec<(i64, u32, u32)> = Vec::with_capacity(total);
    for (index, source) in paths.iter().enumerate() {
        let source = PathBuf::from(source);
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let image_id = setup.image_ids[index];
        let (dds_bytes, width, height) = if extension == "dds" {
            let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "导入 DDS 图片")?;
            dds::prepare_dds_import(&bytes)?
        } else {
            dds::decode_raster_path(&source)?
        };
        let destination = setup.board_directory.join(format!("{image_id}.dds"));
        dds::persist_dds_file(&destination, &dds_bytes, "导入 DDS 图片")?;
        created_files.push(destination);
        imported.push((image_id, width, height));
        on_progress(index + 1, total);
    }
    let sizes = imported
        .iter()
        .map(|(_, width, height)| (*width as f64, *height as f64))
        .collect::<Vec<_>>();
    let points = dds::imported_points(&sizes, center_x, center_y, gap.max(0.0));
    let mut image_ids = Vec::with_capacity(imported.len());
    for (index, ((image_id, width, height), points)) in imported.into_iter().zip(points).enumerate()
    {
        let editable = EditablePinBoardImage {
            image_id,
            order: index as u64,
            layer: 1,
            deleted: false,
            points,
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        };
        record_initial_state(
            &transaction,
            board_id,
            image_id,
            setup.next_step,
            index as u64,
            &editable,
        )?;
        finalize_image_row(&transaction, board_id, image_id, width, height)?;
        image_ids.push(image_id);
    }
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, image_ids))
}

/// 导入剪贴板位图（含“添加文字”生成的 PNG）。
pub(crate) fn import_clipboard_image(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    bytes: &[u8],
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if bytes.is_empty() || bytes.len() > dds::MAX_CLIPBOARD_IMAGE_BYTES {
        return Err("剪贴板图片大小无效".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() {
        return Err("导入位置无效".into());
    }
    let (dds_bytes, width, height) = dds::decode_raster_bytes(bytes)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let setup = begin_image_mutation(&transaction, root, board_id, expected_revision, 1)?;
    let image_id = setup.image_ids[0];
    let points = dds::imported_points(&[(width as f64, height as f64)], center_x, center_y, 0.0)
        .into_iter()
        .next()
        .ok_or("无法计算剪贴板图片位置")?;
    let destination = setup.board_directory.join(format!("{image_id}.dds"));
    let mut created_files = dds::CreatedFiles::default();
    dds::persist_dds_file(&destination, &dds_bytes, "剪贴板 DDS 图片")?;
    created_files.push(destination);
    let editable = EditablePinBoardImage {
        image_id,
        order: 0,
        layer: 1,
        deleted: false,
        points,
        uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    };
    record_initial_state(
        &transaction,
        board_id,
        image_id,
        setup.next_step,
        0,
        &editable,
    )?;
    finalize_image_row(&transaction, board_id, image_id, width, height)?;
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, vec![image_id]))
}

fn unique_export_path(directory: &Path, image_id: i64) -> PathBuf {
    let base = format!("{image_id}");
    let first = directory.join(format!("{base}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|suffix| directory.join(format!("{base}-{suffix}.png")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// 导出选中图片为 PNG 到外部目录，返回成功数量。
pub(crate) fn export_images(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    image_ids: &[i64],
    output_directory: &str,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<usize, String> {
    if image_ids.is_empty() {
        return Err("没有可导出的图片".into());
    }
    let directory = PathBuf::from(output_directory);
    if !directory.is_dir() {
        return Err("导出位置必须是文件夹".into());
    }
    let context = open_board_context(connection, root, board_id)?;
    let total = image_ids.len();
    on_progress(0, total);
    let mut exported = 0usize;
    for (index, image_id) in image_ids.iter().enumerate() {
        let source = context.directory.join(format!("{image_id}.dds"));
        if !source.is_file() {
            on_progress(index + 1, total);
            continue;
        }
        let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "DDS 图片")?;
        let (width, height) = dds::dds_dimensions(&bytes)?;
        if (width as u64) * (height as u64) > dds::MAX_IMAGE_PIXELS {
            return Err(format!("图片 {image_id} 像素数超过导出限制"));
        }
        let dds_file = image_dds::ddsfile::Dds::read(std::io::Cursor::new(&bytes))
            .map_err(|error| format!("无法读取图片 {image_id}：{error}"))?;
        let rgba = image_dds::image_from_dds(&dds_file, 0)
            .map_err(|error| format!("无法解码图片 {image_id}：{error}"))?;
        let target = unique_export_path(&directory, *image_id);
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)
            .map_err(|error| format!("无法创建导出临时文件：{error}"))?;
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(temporary.as_file_mut(), image::ImageFormat::Png)
            .map_err(|error| format!("无法写出图片 {image_id}：{error}"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| format!("无法同步导出图片：{error}"))?;
        temporary
            .persist(target)
            .map_err(|error| format!("无法保存导出图片：{}", error.error))?;
        exported += 1;
        on_progress(index + 1, total);
    }
    Ok(exported)
}

/// 读取图片 PNG（复制到系统剪贴板用）。
pub(crate) fn read_image_png(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    image_id: i64,
) -> Result<Vec<u8>, String> {
    let context = open_board_context(connection, root, board_id)?;
    let source = context.directory.join(format!("{image_id}.dds"));
    let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "DDS 图片")?;
    let (width, height) = dds::dds_dimensions(&bytes)?;
    if (width as u64) * (height as u64) > dds::MAX_IMAGE_PIXELS {
        return Err("图片像素数超过剪贴板读取限制".into());
    }
    let dds_file = image_dds::ddsfile::Dds::read(std::io::Cursor::new(&bytes))
        .map_err(|error| format!("无法读取 DDS 图片：{error}"))?;
    let rgba = image_dds::image_from_dds(&dds_file, 0)
        .map_err(|error| format!("无法解码 DDS 图片：{error}"))?;
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|error| format!("无法编码剪贴板 PNG 图片：{error}"))?;
    Ok(output.into_inner())
}

/// 读取纹理负载：超过请求长边的 DDS 在原生端解码为 RGBA 预览，
/// 同尺寸重复请求命中 LRU 结果缓存。
pub(crate) fn read_texture(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    image_id: i64,
    max_dimension: u32,
    cache_budget_bytes: u64,
) -> Result<Vec<u8>, String> {
    let context = open_board_context(connection, root, board_id)?;
    let path = context.directory.join(format!("{image_id}.dds"));
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("无法访问 DDS 图片：{error}"))?;
    if !canonical.starts_with(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        return Err("DDS 图片路径超出仓库范围".into());
    }
    let max_dimension = max_dimension.clamp(dds::MIN_TEXTURE_DIMENSION, dds::MAX_TEXTURE_DIMENSION);
    let key = (board_id, image_id, max_dimension);
    if let Some(cached) = dds::texture_cache_get(key) {
        return Ok(cached);
    }
    let bytes = dds::read_limited(&canonical, dds::MAX_DDS_BYTES, "DDS 图片")?;
    let (width, height) = dds::dds_dimensions(&bytes)?;
    dds::validate_dds_payload(&bytes, width, height)?;
    let payload = if width > max_dimension || height > max_dimension {
        dds::downscale_bc7(&bytes, width, height, max_dimension)?
    } else {
        let end = dds::dds_payload_end(width, height)?;
        bytes[..end].to_vec()
    };
    dds::texture_cache_store(key, payload.clone(), cache_budget_bytes);
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_repository() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();
        crate::library::initialize(&root).unwrap();
        let connection = crate::storage::open(&root).unwrap();
        (directory, connection)
    }

    fn create_test_artwork(connection: &Connection) -> String {
        let artwork_id = storage::new_id();
        let now = storage::now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO library_nodes (id, kind, title, position, created_ms, updated_ms)
                 VALUES (?1, 'artwork', '测试作品', 0, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artworks (id, created_ms, updated_ms) VALUES (?1, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        artwork_id
    }

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
        let directory = connection
            .query_row::<String, _, _>("SELECT ?1", [], |_| Ok(String::new()))
            .unwrap_or_default();
        let _ = directory;

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

    /// 造一块只含一张删除态图片的画板：图片记录 + step 0 删除节点 + DDS 实体。
    fn insert_deleted_state_image(
        connection: &Connection,
        board_id: i64,
        root: &Path,
        artwork_id: &str,
    ) -> PathBuf {
        let now = storage::now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO pin_board_images (board_id, file_path, width, height, created_ms)
                 VALUES (?1, '', 4, 4, ?2)",
                params![board_id, now],
            )
            .unwrap();
        let image_id = connection.last_insert_rowid();
        let transform = r#"{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}"#;
        connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (?1, ?2, 0, 1, 1, 0, ?3)",
                params![board_id, image_id, transform],
            )
            .unwrap();
        let directory = root
            .join("artworks")
            .join(artwork_id)
            .join(BOARD_DIRECTORY)
            .join(board_id.to_string());
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{image_id}.dds"));
        fs::write(&path, b"dds-bytes").unwrap();
        path
    }

    fn board_revision(connection: &Connection, board_id: i64) -> String {
        connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [board_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn scalar(connection: &Connection, sql: &str) -> i64 {
        connection.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn scalar_for_board(connection: &Connection, sql: &str, board_id: i64) -> i64 {
        connection
            .query_row(sql, [board_id], |row| row.get(0))
            .unwrap()
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

        // 结算后图片记录与历史已删除。
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_images WHERE board_id = ?1",
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
                "SELECT COUNT(*) FROM pin_board_images WHERE board_id = ?1",
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
        let relative = format!(
            "{}/{}",
            board_relative_path(&artwork_id, board.board_id),
            dds_path.file_name().unwrap().to_string_lossy()
        );
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
}

#[cfg(test)]
mod migration_tests {
    /// v1 仓库（无素材板表、分支表也没有 v2/v3/v4 追加列）打开时必须通过追加式
    /// 迁移升级到当前版本，且既有 v1 数据保持不变。
    #[test]
    fn migrates_a_v1_repository_to_the_current_version_append_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();
        crate::library::initialize(&root).unwrap();

        // 把刚创建的仓库降级为等价 v1：移除素材板表、按 v1 结构重建
        // branches（去掉 v3 追加的快速检查列）并把版本写回 1。
        {
            let connection = crate::storage::open(&root).unwrap();
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                 DROP TABLE pin_board_history;
                 DROP TABLE pin_board_images;
                 DROP TABLE pin_boards;
                 CREATE TABLE branches_v1 (
                   id TEXT PRIMARY KEY,
                   artwork_id TEXT NOT NULL REFERENCES artworks(id) ON DELETE CASCADE,
                   title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 160),
                   source_path TEXT NOT NULL,
                   source_path_key TEXT NOT NULL,
                   head_history_id TEXT REFERENCES history_nodes(id) ON DELETE SET NULL,
                   created_from_history_id TEXT REFERENCES history_nodes(id) ON DELETE SET NULL,
                   backup_enabled INTEGER NOT NULL DEFAULT 1 CHECK (backup_enabled IN (0, 1)),
                   backup_interval_minutes INTEGER NOT NULL DEFAULT 5 CHECK (backup_interval_minutes BETWEEN 1 AND 10080),
                   last_check_ms INTEGER,
                   last_success_ms INTEGER,
                   last_error TEXT,
                   consecutive_backup_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_backup_failures >= 0),
                   backup_retry_at_ms INTEGER,
                   backup_disable_notice_pending INTEGER NOT NULL DEFAULT 0 CHECK (backup_disable_notice_pending IN (0, 1)),
                   created_ms INTEGER NOT NULL,
                   updated_ms INTEGER NOT NULL,
                   UNIQUE (artwork_id, source_path_key)
                 );
                 INSERT INTO branches_v1 SELECT
                   id, artwork_id, title, source_path, source_path_key, head_history_id,
                   created_from_history_id, backup_enabled, backup_interval_minutes,
                   last_check_ms, last_success_ms, last_error, consecutive_backup_failures,
                   backup_retry_at_ms, backup_disable_notice_pending, created_ms, updated_ms
                 FROM branches;
                 DROP TABLE branches;
                 ALTER TABLE branches_v1 RENAME TO branches;
                 CREATE INDEX branches_artwork_created ON branches(artwork_id, created_ms, id);
                 UPDATE repository_meta SET value = '1' WHERE key = 'schema_version';",
                )
                .unwrap();
        }

        crate::library::open_existing(&root).unwrap();

        let connection = crate::storage::open(&root).unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM repository_meta
                 WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 4);
        for table in ["pin_boards", "pin_board_images", "pin_board_history"] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "表 {table} 应在迁移后存在");
        }
        // v3 追加的快速检查列与 v4 追加的校验状态列必须在迁移后可用。
        let columns: Vec<String> = {
            let mut statement = connection.prepare("PRAGMA table_info(branches)").unwrap();
            statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for column in [
            "backup_quick_enabled",
            "last_source_size",
            "last_source_modified_ms",
            "verified_history_id",
            "verified_ms",
            "verify_error",
        ] {
            assert!(columns.iter().any(|item| item == column), "{column}");
        }
    }
}
