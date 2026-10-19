//! 素材板 SQLite 持久化与画板文件布局。
//!
//! 存储布局（schema v5 起，见 `library/schema.rs`）：
//! - `pin_boards`：按 Artwork 平铺一层画板，`deleted_at` 为回收站软删除；
//! - `pin_board_items`：与 kind 无关的公共条目；
//! - `pin_board_bitmaps`：位图载荷，DDS 实体存于
//!   `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`；
//! - `pin_board_texts`：文字载荷，不落实体文件；
//! - `pin_board_history`：每个条目按 step 的变换历史（撤销/恢复持久化）。
//!
//! 本模块只通过 `storage.rs` 打开连接，不读取 ChunkFile；回收站永久删除经
//! `cleanup.rs` 队列清理 DDS 目录。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::dds;
use crate::{cleanup, storage};

pub(crate) const BOARD_DIRECTORY: &str = "boards";
/// 单个 Artwork 的画板数量上限（规划确认的规模假设）。
pub(crate) const MAX_BOARDS_PER_ARTWORK: usize = 64;
/// 文字条目的固定字体族。创建时把度量结果持久化，跨机器缺字体只影响显示度量。
pub(crate) const TEXT_FONT_FAMILY: &str = "\"Microsoft YaHei UI\", \"Segoe UI\", sans-serif";
pub(crate) const MAX_TEXT_CHARS: usize = 5000;
/// 文字条目的逻辑度量上限。超过该值的段落无法在既有纹理上限内清晰显示。
pub(crate) const MAX_TEXT_MEASURE_DIMENSION: f64 = 8192.0;
/// 文字导出栅格图的 RGBA 像素预算（宽 × 高 × 4 的上限），与纹理预览同一量级。
pub(crate) const MAX_TEXT_RGBA_BYTES: u64 = dds::MAX_RGBA_PREVIEW_BYTES;
/// 文字导出 PNG 的载荷上限。前端在浏览器里编码，正常文案远低于该值。
pub(crate) const MAX_TEXT_PNG_BYTES: u64 = 32 * 1024 * 1024;

const ITEM_KIND_BITMAP: i64 = 0;
const ITEM_KIND_TEXT: i64 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardSummary {
    pub(crate) board_id: i64,
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

/// 条目载荷。公共字段（尺寸、可用性、变换）在 `PinBoardItem` 上，kind 只决定
/// 载荷子表与渲染 provider。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum PinBoardItemPayload {
    Bitmap {},
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        font_family: String,
        font_size: f64,
        color: String,
        align: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardItem {
    board_id: i64,
    item_id: i64,
    width: f64,
    height: f64,
    order: u64,
    layer: u8,
    deleted: bool,
    points: [[f64; 2]; 4],
    uv: [[f64; 2]; 4],
    available: bool,
    #[serde(flatten)]
    payload: PinBoardItemPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardView {
    board_id: i64,
    name: String,
    items: Vec<PinBoardItem>,
    missing_textures: usize,
    revision: String,
    now_step: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditablePinBoardItem {
    item_id: i64,
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
    source_item_id: i64,
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
/// `artworks/<artwork-id>/<BOARD_DIRECTORY>/<board-id>/<item-id>.dds`，返回
/// `(artwork_id, board_id, item_id)`；不匹配时返回 `None`。文字条目没有实体文件，
/// 因此只有位图条目会出现在这条路径布局里。它与 `board_relative_path` 是同一布局的
/// 正反两面，供 `cleanup` 的画板 DDS 反向引用检查使用，避免调用方另行硬编码路径形状。
pub(crate) fn parse_board_dds_path(path: &str) -> Option<(&str, i64, i64)> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "artworks" || parts[2] != BOARD_DIRECTORY {
        return None;
    }
    let board_id = parts[3].parse::<i64>().ok()?;
    let item_id = parts[4].strip_suffix(".dds")?.parse::<i64>().ok()?;
    Some((parts[1], board_id, item_id))
}

/// 画板 DDS 文件名：`<item-id>.dds`（item id 为 SQLite 自增正整数）。扫描与
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
                (SELECT COUNT(*) FROM pin_board_items i WHERE i.board_id = b.id),
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
struct TextPayload {
    text: String,
    font_family: String,
    font_size: f64,
    color: String,
    align: String,
}

#[derive(Debug, Clone)]
struct ItemState {
    kind: i64,
    file_path: Option<String>,
    width: f64,
    height: f64,
    deleted: bool,
    layer: u8,
    sort_order: u64,
    transform: ImageTransform,
    text: Option<TextPayload>,
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
) -> Result<Vec<(i64, ItemState)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT i.id, i.kind, b.file_path,
                    CAST(b.width AS REAL), CAST(b.height AS REAL),
                    t.measured_width, t.measured_height,
                    h.deleted, h.layer, h.sort_order, h.transform_json,
                    t.text, t.font_family, t.font_size, t.color, t.align
               FROM pin_board_items i
               LEFT JOIN pin_board_bitmaps b ON b.item_id = i.id
               LEFT JOIN pin_board_texts t ON t.item_id = i.id
               JOIN pin_board_history h
                 ON h.board_id = i.board_id AND h.item_id = i.id
                AND h.step = (SELECT MAX(step) FROM pin_board_history
                              WHERE board_id = i.board_id AND item_id = i.id AND step <= ?2)
              WHERE i.board_id = ?1",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map(params![board_id, now_step as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, i64>(7)? != 0,
                row.get::<_, i64>(8)? as u8,
                row.get::<_, i64>(9)? as u64,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<f64>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
            ))
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?
        .into_iter()
        .map(|row| {
            let (
                item_id,
                kind,
                file_path,
                bitmap_width,
                bitmap_height,
                text_width,
                text_height,
                deleted,
                layer,
                sort_order,
                transform_json,
                text,
                font_family,
                font_size,
                color,
                align,
            ) = row;
            let transform: ImageTransform = serde_json::from_str(&transform_json)
                .map_err(|error| format!("画板条目变换数据无效：{error}"))?;
            let (width, height, payload) = match kind {
                ITEM_KIND_BITMAP => {
                    let width = bitmap_width.ok_or("位图条目缺少尺寸记录")?;
                    let height = bitmap_height.ok_or("位图条目缺少尺寸记录")?;
                    (width, height, None)
                }
                ITEM_KIND_TEXT => {
                    let width = text_width.ok_or("文字条目缺少度量记录")?;
                    let height = text_height.ok_or("文字条目缺少度量记录")?;
                    let payload = TextPayload {
                        text: text.ok_or("文字条目缺少内容")?,
                        font_family: font_family.ok_or("文字条目缺少字体")?,
                        font_size: font_size.ok_or("文字条目缺少字号")?,
                        color: color.ok_or("文字条目缺少颜色")?,
                        align: align.ok_or("文字条目缺少对齐方式")?,
                    };
                    (width, height, Some(payload))
                }
                other => return Err(format!("未知画板条目类型：{other}")),
            };
            if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                return Err(format!("画板条目 {item_id} 的尺寸无效"));
            }
            Ok((
                item_id,
                ItemState {
                    kind,
                    file_path,
                    width,
                    height,
                    deleted,
                    layer,
                    sort_order,
                    transform,
                    text: payload,
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(rows)
}

/// 位图条目的 DDS 是否可用。文字条目没有实体文件，恒为可用。
fn item_available(context: &BoardContext, item_id: i64, state: &ItemState) -> bool {
    if state.kind != ITEM_KIND_BITMAP {
        return true;
    }
    let Some(file_path) = state.file_path.as_deref() else {
        return false;
    };
    if file_path != format!("{item_id}.dds") {
        return false;
    }
    context.directory.join(file_path).is_file()
}

pub(crate) fn load_view(
    connection: &Connection,
    root: &Path,
    board_id: i64,
) -> Result<PinBoardView, String> {
    let context = open_board_context(connection, root, board_id)?;
    let states = current_states(connection, board_id, context.now_step)?;
    let mut items = Vec::with_capacity(states.len());
    for (item_id, state) in states {
        let available = item_available(&context, item_id, &state);
        let payload = if state.kind == ITEM_KIND_TEXT {
            let text = state
                .text
                .ok_or_else(|| format!("文字条目 {item_id} 缺少载荷"))?;
            PinBoardItemPayload::Text {
                text: text.text,
                font_family: text.font_family,
                font_size: text.font_size,
                color: text.color,
                align: text.align,
            }
        } else {
            PinBoardItemPayload::Bitmap {}
        };
        items.push(PinBoardItem {
            board_id,
            item_id,
            width: state.width,
            height: state.height,
            order: state.sort_order,
            layer: state.layer,
            deleted: state.deleted,
            points: state.transform.points,
            uv: state.transform.uv,
            available,
            payload,
        });
    }
    items.sort_by(|left, right| {
        left.layer
            .cmp(&right.layer)
            .then_with(|| right.order.cmp(&left.order))
    });
    let missing_textures = items
        .iter()
        .filter(|item| !item.deleted && !item.available)
        .count();
    Ok(PinBoardView {
        board_id,
        name: context.name,
        items,
        missing_textures,
        revision: context.revision,
        now_step: context.now_step,
    })
}

fn validate_editable(item: &EditablePinBoardItem) -> Result<(), String> {
    if item.layer > 2 {
        return Err(format!("条目 {} 的图层无效", item.item_id));
    }
    if item
        .points
        .iter()
        .chain(item.uv.iter())
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(format!("条目 {} 包含无效坐标", item.item_id));
    }
    let edge = |left: [f64; 2], right: [f64; 2]| (right[0] - left[0]).hypot(right[1] - left[1]);
    if edge(item.points[0], item.points[1]) < 0.000_001
        || edge(item.points[0], item.points[3]) < 0.000_001
    {
        return Err(format!("条目 {} 的尺寸无效", item.item_id));
    }
    Ok(())
}

fn same_state(left: &ItemState, right: &EditablePinBoardItem) -> bool {
    left.deleted == right.deleted
        && left.layer == right.layer
        && left.transform.points == right.points
        && left.transform.uv == right.uv
}

pub(crate) fn save_board(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    items: &[EditablePinBoardItem],
    expected_revision: &str,
) -> Result<SavePinBoardResult, String> {
    if items.len() > dds::MAX_BOARD_ITEMS {
        return Err("保存的画板条目数量超过限制".into());
    }
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let context = open_board_context(&transaction, root, board_id)?;
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
    let now_step = context.now_step;

    let mut requested = std::collections::HashMap::with_capacity(items.len());
    for item in items {
        validate_editable(item)?;
        if requested.insert(item.item_id, item).is_some() {
            return Err("保存请求包含重复条目".into());
        }
    }

    let stored = current_states(&transaction, board_id, now_step)?;
    let mut changed = std::collections::HashSet::new();
    let mut order_changed = false;
    let mut found = 0usize;
    for (item_id, state) in &stored {
        let Some(editable) = requested.get(item_id) else {
            continue;
        };
        found += 1;
        if !same_state(state, editable) {
            changed.insert(*item_id);
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

    // 新步骤会覆盖未来：与 Client 行为一致，写入新步骤后 redo 记录作废。
    for item_id in &changed {
        transaction
            .execute(
                "DELETE FROM pin_board_history WHERE board_id = ?1 AND item_id = ?2 AND step > ?3",
                params![board_id, item_id, now_step as i64],
            )
            .map_err(storage::database_error)?;
        let editable = requested.get(item_id).expect("checked above");
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
                    editable.layer as i64,
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
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
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
        if file_path != &format!("{item_id}.dds") {
            continue;
        }
        let path = format!(
            "{}/{file_path}",
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

/// 新增条目前的公共步骤：校验修订、作废 redo、预留顺序与 item id。
struct MutationSetup {
    board_directory: PathBuf,
    next_step: u64,
    item_ids: Vec<i64>,
}

fn begin_item_mutation(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    board_id: i64,
    expected_revision: &str,
    count: usize,
    kind: i64,
) -> Result<MutationSetup, String> {
    let context = open_board_context(transaction, root, board_id)?;
    if context.revision != expected_revision {
        return Err("画板已被其他窗口或程序修改，请重新打开后再编辑".into());
    }
    let item_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if item_count as usize + count > dds::MAX_BOARD_ITEMS {
        return Err("画板条目数量超过限制".into());
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
    // 新条目统一放入中层并占据最优先的一组顺序。
    transaction
        .execute(
            "UPDATE pin_board_history SET sort_order = sort_order + ?1
             WHERE board_id = ?2 AND step = ?3",
            params![count as i64, board_id, context.now_step as i64],
        )
        .map_err(storage::database_error)?;
    let now = storage::now_ms()?;
    let mut item_ids = Vec::with_capacity(count);
    for _ in 0..count {
        transaction
            .execute(
                "INSERT INTO pin_board_items (board_id, kind, created_ms)
                 VALUES (?1, ?2, ?3)",
                params![board_id, kind, now],
            )
            .map_err(storage::database_error)?;
        let item_id = transaction.last_insert_rowid();
        if kind == ITEM_KIND_BITMAP {
            transaction
                .execute(
                    "INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                     VALUES (?1, '', 1, 1)",
                    [item_id],
                )
                .map_err(storage::database_error)?;
        }
        item_ids.push(item_id);
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
        item_ids,
    })
}

fn record_initial_state(
    transaction: &rusqlite::Transaction<'_>,
    board_id: i64,
    item_id: i64,
    step: u64,
    order: u64,
    editable: &EditablePinBoardItem,
) -> Result<(), String> {
    validate_editable(editable)?;
    let transform = serde_json::to_string(&ImageTransform {
        points: editable.points,
        uv: editable.uv,
    })
    .map_err(|error| format!("无法序列化条目变换：{error}"))?;
    // 沿用旧 step 模型：先写入 step 0 的删除态默认节点，再写入添加步骤。
    transaction
        .execute(
            "INSERT INTO pin_board_history
               (board_id, item_id, step, deleted, layer, sort_order, transform_json)
             VALUES (?1, ?2, 0, 1, ?3, ?4, ?5)",
            params![
                board_id,
                item_id,
                editable.layer as i64,
                order as i64,
                transform
            ],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "INSERT INTO pin_board_history
               (board_id, item_id, step, deleted, layer, sort_order, transform_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                board_id,
                item_id,
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

fn finalize_bitmap_row(
    transaction: &rusqlite::Transaction<'_>,
    item_id: i64,
    width: u32,
    height: u32,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE pin_board_bitmaps SET file_path = ?1, width = ?2, height = ?3
             WHERE item_id = ?4",
            params![format!("{item_id}.dds"), width, height, item_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

fn source_dds_path(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    board_id: i64,
    item_id: i64,
) -> Result<PathBuf, String> {
    let context = open_board_context(transaction, root, board_id)?;
    let file_path: Option<String> = transaction
        .query_row(
            "SELECT b.file_path
               FROM pin_board_items i
               JOIN pin_board_bitmaps b ON b.item_id = i.id
              WHERE i.id = ?1 AND i.board_id = ?2 AND i.kind = 0",
            params![item_id, board_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    let file_path = file_path.ok_or("源图片不是位图条目")?;
    if file_path != format!("{item_id}.dds") {
        return Err("源图片记录的文件名无效".into());
    }
    let path = context.directory.join(file_path);
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
    let setup = begin_item_mutation(
        &transaction,
        root,
        board_id,
        expected_revision,
        images.len(),
        ITEM_KIND_BITMAP,
    )?;
    let source_paths = images
        .iter()
        .map(|image| {
            source_dds_path(
                &transaction,
                root,
                image.source_board_id,
                image.source_item_id,
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
    let mut item_ids = Vec::with_capacity(prepared.len());
    for (index, ((item_id, points), (dds_bytes, width, height, uv))) in
        setup.item_ids.iter().zip(points).zip(prepared).enumerate()
    {
        let item_id = *item_id;
        let destination = setup.board_directory.join(format!("{item_id}.dds"));
        dds::persist_dds_file(&destination, &dds_bytes, "粘贴 DDS 图片")?;
        created_files.push(destination);
        let editable = EditablePinBoardItem {
            item_id,
            order: index as u64,
            layer: 1,
            deleted: false,
            points,
            uv,
        };
        record_initial_state(
            &transaction,
            board_id,
            item_id,
            setup.next_step,
            index as u64,
            &editable,
        )?;
        finalize_bitmap_row(&transaction, item_id, width, height)?;
        item_ids.push(item_id);
    }
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, item_ids))
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
    // 仓库边界在这里兜底，而不是只在命令层：GUI 与无头入口都会经过本函数，素材板
    // 也不该读仓库内部的文件（认证路径同样明确禁止）。授权（`ensure_dialog_authorized`）
    // 属于命令层的文件系统 scope，两者各管一件事，与认证模块两道都查的做法一致。
    for source in paths {
        storage::ensure_outside_repository(root, Path::new(source), "导入图片")
            .map_err(|error| format!("{error}：{source}"))?;
    }
    let total = paths.len();
    on_progress(0, total);

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let setup = begin_item_mutation(
        &transaction,
        root,
        board_id,
        expected_revision,
        total,
        ITEM_KIND_BITMAP,
    )?;
    let mut created_files = dds::CreatedFiles::default();
    let mut imported: Vec<(i64, u32, u32)> = Vec::with_capacity(total);
    for (index, source) in paths.iter().enumerate() {
        let source = PathBuf::from(source);
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let item_id = setup.item_ids[index];
        let (dds_bytes, width, height) = if extension == "dds" {
            let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "导入 DDS 图片")?;
            dds::prepare_dds_import(&bytes)?
        } else {
            dds::decode_raster_path(&source)?
        };
        let destination = setup.board_directory.join(format!("{item_id}.dds"));
        dds::persist_dds_file(&destination, &dds_bytes, "导入 DDS 图片")?;
        created_files.push(destination);
        imported.push((item_id, width, height));
        on_progress(index + 1, total);
    }
    let sizes = imported
        .iter()
        .map(|(_, width, height)| (*width as f64, *height as f64))
        .collect::<Vec<_>>();
    let points = dds::imported_points(&sizes, center_x, center_y, gap.max(0.0));
    let mut item_ids = Vec::with_capacity(imported.len());
    for (index, ((item_id, width, height), points)) in imported.into_iter().zip(points).enumerate()
    {
        let editable = EditablePinBoardItem {
            item_id,
            order: index as u64,
            layer: 1,
            deleted: false,
            points,
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        };
        record_initial_state(
            &transaction,
            board_id,
            item_id,
            setup.next_step,
            index as u64,
            &editable,
        )?;
        finalize_bitmap_row(&transaction, item_id, width, height)?;
        item_ids.push(item_id);
    }
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, item_ids))
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
    let setup = begin_item_mutation(
        &transaction,
        root,
        board_id,
        expected_revision,
        1,
        ITEM_KIND_BITMAP,
    )?;
    let item_id = setup.item_ids[0];
    let points = dds::imported_points(&[(width as f64, height as f64)], center_x, center_y, 0.0)
        .into_iter()
        .next()
        .ok_or("无法计算剪贴板图片位置")?;
    let destination = setup.board_directory.join(format!("{item_id}.dds"));
    let mut created_files = dds::CreatedFiles::default();
    dds::persist_dds_file(&destination, &dds_bytes, "剪贴板 DDS 图片")?;
    created_files.push(destination);
    let editable = EditablePinBoardItem {
        item_id,
        order: 0,
        layer: 1,
        deleted: false,
        points,
        uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    };
    record_initial_state(
        &transaction,
        board_id,
        item_id,
        setup.next_step,
        0,
        &editable,
    )?;
    finalize_bitmap_row(&transaction, item_id, width, height)?;
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    let view = load_view(connection, root, board_id)?;
    Ok((view, vec![item_id]))
}

/// 新增文字条目。度量由前端用最终显示字体完成并持久化；四边形尺寸直接取度量
/// 结果，不再依赖栅格化 padding。
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_text(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    text: &str,
    font_size: f64,
    color: &str,
    align: &str,
    measured_width: f64,
    measured_height: f64,
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if text.trim().is_empty() {
        return Err("文字内容不能为空".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("文字内容不能超过 {MAX_TEXT_CHARS} 个字符"));
    }
    if !font_size.is_finite() || font_size <= 0.0 || font_size > 1000.0 {
        return Err("文字字号无效".into());
    }
    let color = normalise_color(color)?;
    if !matches!(align, "left" | "center" | "right") {
        return Err("文字对齐方式无效".into());
    }
    if !measured_width.is_finite()
        || !measured_height.is_finite()
        || measured_width <= 0.0
        || measured_height <= 0.0
        || measured_width > MAX_TEXT_MEASURE_DIMENSION
        || measured_height > MAX_TEXT_MEASURE_DIMENSION
    {
        return Err("文字尺寸超出素材板上限".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() {
        return Err("文字位置无效".into());
    }

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let setup = begin_item_mutation(
        &transaction,
        root,
        board_id,
        expected_revision,
        1,
        ITEM_KIND_TEXT,
    )?;
    let item_id = setup.item_ids[0];
    transaction
        .execute(
            "INSERT INTO pin_board_texts
               (item_id, text, font_family, font_size, color, align,
                measured_width, measured_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item_id,
                text,
                TEXT_FONT_FAMILY,
                font_size,
                color,
                align,
                measured_width,
                measured_height
            ],
        )
        .map_err(storage::database_error)?;
    let points = dds::imported_points(
        &[(measured_width, measured_height)],
        center_x,
        center_y,
        0.0,
    )
    .into_iter()
    .next()
    .ok_or("无法计算文字位置")?;
    let editable = EditablePinBoardItem {
        item_id,
        order: 0,
        layer: 1,
        deleted: false,
        points,
        uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    };
    record_initial_state(
        &transaction,
        board_id,
        item_id,
        setup.next_step,
        0,
        &editable,
    )?;
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    let view = load_view(connection, root, board_id)?;
    Ok((view, vec![item_id]))
}

fn normalise_color(color: &str) -> Result<String, String> {
    let bytes = color.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err("文字颜色必须是 #RRGGBB".into());
    }
    Ok(color.to_ascii_uppercase())
}

/// 导出文件名：`<item-id>.png`，同目录已存在同名文件时追加 `-2`、`-3`…，绝不覆盖
/// 用户已有文件。
fn unique_export_path(directory: &Path, item_id: i64) -> PathBuf {
    let base = format!("{item_id}");
    let first = directory.join(format!("{base}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|suffix| directory.join(format!("{base}-{suffix}.png")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// 把一张导出图写到目标目录：临时文件写入 → `sync_all` → 以不覆盖方式发布。
///
/// 位图与文字共用这条路，写出的字节由调用方提供（位图解码后重编码，文字直接落
/// 前端编码好的 PNG），因此目录权限、命名冲突与崩溃残留的语义只有一处。
fn publish_export_png(
    directory: &Path,
    item_id: i64,
    label: &str,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let target = unique_export_path(directory, item_id);
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("无法创建导出临时文件：{error}"))?;
    write(temporary.as_file_mut())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步导出{label}：{error}"))?;
    temporary
        .persist(&target)
        .map_err(|error| format!("无法保存导出{label}：{}", error.error))?;
    Ok(target)
}

fn item_kind_and_file(
    connection: &Connection,
    board_id: i64,
    item_id: i64,
) -> Result<(i64, Option<String>), String> {
    connection
        .query_row(
            "SELECT i.kind, b.file_path
               FROM pin_board_items i
               LEFT JOIN pin_board_bitmaps b ON b.item_id = i.id
              WHERE i.id = ?1 AND i.board_id = ?2",
            params![item_id, board_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| format!("画板条目 {item_id} 不存在"))
}

fn bitmap_source_path(
    root: &Path,
    context: &BoardContext,
    item_id: i64,
    file_path: &str,
) -> Result<PathBuf, String> {
    if file_path != format!("{item_id}.dds") {
        return Err(format!("位图条目 {item_id} 的文件名无效"));
    }
    let path = context.directory.join(file_path);
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("无法访问 DDS 图片：{error}"))?;
    if !canonical.starts_with(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        return Err("DDS 图片路径超出仓库范围".into());
    }
    Ok(canonical)
}

/// 导出选中位图为 PNG 到外部目录，返回成功数量。文字条目由前端按同一套字体
/// 度量栅格化后经 [`write_text_png`] 写盘，不进入本函数。
pub(crate) fn export_images(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    item_ids: &[i64],
    output_directory: &str,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<usize, String> {
    if item_ids.is_empty() {
        return Err("没有可导出的条目".into());
    }
    let directory = PathBuf::from(output_directory);
    if !directory.is_dir() {
        return Err("导出位置必须是文件夹".into());
    }
    // 与导入同一道仓库边界校验：导出不得写进仓库目录（那会让画板 DDS 与用户导出
    // 文件混在同一棵树上，也会绕过受控路径规则）。授权属于命令层的文件系统 scope。
    storage::ensure_outside_repository(root, &directory, "导出位置")?;
    let context = open_board_context(connection, root, board_id)?;
    let total = item_ids.len();
    on_progress(0, total);
    let mut exported = 0usize;
    for (index, item_id) in item_ids.iter().enumerate() {
        let (kind, file_path) = item_kind_and_file(connection, board_id, *item_id)?;
        if kind != ITEM_KIND_BITMAP {
            on_progress(index + 1, total);
            continue;
        }
        let Some(file_path) = file_path else {
            on_progress(index + 1, total);
            continue;
        };
        let source = match bitmap_source_path(root, &context, *item_id, &file_path) {
            Ok(path) if path.is_file() => path,
            _ => {
                on_progress(index + 1, total);
                continue;
            }
        };
        let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "DDS 图片")?;
        let (width, height) = dds::dds_dimensions(&bytes)?;
        if (width as u64) * (height as u64) > dds::MAX_IMAGE_PIXELS {
            return Err(format!("图片 {item_id} 像素数超过导出限制"));
        }
        let dds_file = image_dds::ddsfile::Dds::read(std::io::Cursor::new(&bytes))
            .map_err(|error| format!("无法读取图片 {item_id}：{error}"))?;
        let rgba = image_dds::image_from_dds(&dds_file, 0)
            .map_err(|error| format!("无法解码图片 {item_id}：{error}"))?;
        publish_export_png(&directory, *item_id, "图片", |file| {
            image::DynamicImage::ImageRgba8(rgba)
                .write_to(file, image::ImageFormat::Png)
                .map_err(|error| format!("无法写出图片 {item_id}：{error}"))
        })?;
        exported += 1;
        on_progress(index + 1, total);
    }
    Ok(exported)
}

/// 读取 PNG 的宽高，并顺带确认载荷确实是 PNG。
///
/// 只解析头部，不解码像素：文字图由前端按最终显示尺寸栅格化，原生端要挡的是
/// 「不是 PNG」与「尺寸超出预算」，不必再走一遍完整解码。
fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("无法识别文字导出图片格式：{error}"))?;
    if reader.format() != Some(image::ImageFormat::Png) {
        return Err("文字导出载荷必须是 PNG".into());
    }
    reader
        .into_dimensions()
        .map_err(|error| format!("无法读取文字导出图片尺寸：{error}"))
}

/// 把前端编码好的文字导出图校验后原子写盘，返回写出的文件路径。
///
/// 文字图在浏览器里已经按屏幕同一套字体度量栅格化并编码为 PNG，原生端不再重新
/// 编码：只确认条目是文字、载荷是尺寸合规的 PNG，再与位图导出共用同一条
/// 临时文件发布路径。目标目录的授权属于命令层的文件系统 scope。
pub(crate) fn write_text_png(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    item_id: i64,
    output_directory: &str,
    png: &[u8],
) -> Result<String, String> {
    open_board_context(connection, root, board_id)?;
    let (kind, _) = item_kind_and_file(connection, board_id, item_id)?;
    if kind != ITEM_KIND_TEXT {
        return Err("只有文字条目支持写盘导出图片".into());
    }
    if png.is_empty() || png.len() as u64 > MAX_TEXT_PNG_BYTES {
        return Err("文字导出图片大小无效".into());
    }
    let (width, height) = png_dimensions(png)?;
    if f64::from(width) > MAX_TEXT_MEASURE_DIMENSION
        || f64::from(height) > MAX_TEXT_MEASURE_DIMENSION
    {
        return Err("文字导出尺寸无效".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels * 4 > MAX_TEXT_RGBA_BYTES {
        return Err("文字导出像素超过上限".into());
    }
    let directory = PathBuf::from(output_directory);
    if !directory.is_dir() {
        return Err("导出位置必须是文件夹".into());
    }
    storage::ensure_outside_repository(root, &directory, "导出位置")?;
    let target = publish_export_png(&directory, item_id, "文字图片", |file| {
        use std::io::Write;
        file.write_all(png)
            .map_err(|error| format!("无法写出文字图片：{error}"))
    })?;
    Ok(storage::display_path(&target))
}

/// 读取图片 PNG（复制到系统剪贴板用）。
pub(crate) fn read_image_png(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    item_id: i64,
) -> Result<Vec<u8>, String> {
    let context = open_board_context(connection, root, board_id)?;
    let (kind, file_path) = item_kind_and_file(connection, board_id, item_id)?;
    if kind != ITEM_KIND_BITMAP {
        return Err("文字条目没有位图，请在画板内渲染后复制".into());
    }
    let file_path = file_path.ok_or("位图条目缺少文件记录")?;
    let source = bitmap_source_path(root, &context, item_id, &file_path)?;
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
    item_id: i64,
    max_dimension: u32,
    cache_budget_bytes: u64,
) -> Result<Vec<u8>, String> {
    let context = open_board_context(connection, root, board_id)?;
    let (kind, file_path) = item_kind_and_file(connection, board_id, item_id)?;
    if kind != ITEM_KIND_BITMAP {
        return Err("文字条目没有 DDS，由前端按需栅格化".into());
    }
    let file_path = file_path.ok_or("位图条目缺少文件记录")?;
    let canonical = bitmap_source_path(root, &context, item_id, &file_path)?;
    let max_dimension = max_dimension.clamp(dds::MIN_TEXTURE_DIMENSION, dds::MAX_TEXTURE_DIMENSION);
    let key = (board_id, item_id, max_dimension);
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
    use crate::contracts::ReferenceInventory;
    use std::fs;

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
    fn import_and_export_refuse_repository_paths() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "板").unwrap();
        let revision: String = connection
            .query_row(
                "SELECT revision FROM pin_boards WHERE id = ?1",
                [board.board_id],
                |row| row.get(0),
            )
            .unwrap();

        // 素材板与认证路径同一口径：外部源与导出目录都必须位于仓库之外。素材板
        // 也不需要读仓库内部的文件，那里只有它自己的 DDS 与历史文件。
        let inside_source = root.join("artworks").join("source.png");
        fs::write(&inside_source, b"not decoded yet").unwrap();
        let import_error = import_images(
            &mut connection,
            &root,
            board.board_id,
            &[storage::display_path(&inside_source)],
            0.0,
            0.0,
            0.0,
            &revision,
            |_, _| {},
        )
        .unwrap_err();
        assert!(import_error.contains("作品仓库内部"), "{import_error}");

        let inside_output = root.join("exports");
        fs::create_dir_all(&inside_output).unwrap();
        let export_error = export_images(
            &connection,
            &root,
            board.board_id,
            &[1],
            &storage::display_path(&inside_output),
            |_, _| {},
        )
        .unwrap_err();
        assert!(export_error.contains("作品仓库内部"), "{export_error}");
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

    #[test]
    fn serializes_item_dtos_with_kind_and_camel_case_fields() {
        let points = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let bitmap = PinBoardItem {
            board_id: 1,
            item_id: 7,
            width: 10.0,
            height: 10.0,
            order: 0,
            layer: 1,
            deleted: false,
            points,
            uv: points,
            available: true,
            payload: PinBoardItemPayload::Bitmap {},
        };
        let value = serde_json::to_value(&bitmap).unwrap();
        assert_eq!(value["kind"], "bitmap");
        assert_eq!(value["itemId"], 7);
        assert!(value.get("fontFamily").is_none());

        let text = PinBoardItem {
            payload: PinBoardItemPayload::Text {
                text: "hello".into(),
                font_family: TEXT_FONT_FAMILY.into(),
                font_size: 24.0,
                color: "#112233".into(),
                align: "center".into(),
            },
            ..bitmap
        };
        let value = serde_json::to_value(&text).unwrap();
        assert_eq!(value["kind"], "text");
        assert_eq!(value["fontFamily"], TEXT_FONT_FAMILY);
        assert_eq!(value["fontSize"], 24.0);

        let editable: EditablePinBoardItem = serde_json::from_value(serde_json::json!({
            "itemId": 7,
            "order": 0,
            "layer": 1,
            "deleted": false,
            "points": points,
            "uv": points,
        }))
        .unwrap();
        assert_eq!(editable.item_id, 7);
    }

    #[test]
    fn creates_text_items_without_bitmap_payloads() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "文字板").unwrap();
        let revision = board_revision(&connection, board.board_id);

        let (view, item_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "Hello 画板",
            24.0,
            "#a1b2c3",
            "center",
            120.0,
            40.0,
            640.0,
            360.0,
            &revision,
        )
        .unwrap();

        assert_eq!(item_ids.len(), 1);
        let item = view
            .items
            .iter()
            .find(|item| item.item_id == item_ids[0])
            .unwrap();
        assert!(item.available);
        assert_eq!((item.width, item.height), (120.0, 40.0));
        match &item.payload {
            PinBoardItemPayload::Text {
                text,
                font_family,
                font_size,
                color,
                align,
            } => {
                assert_eq!(text, "Hello 画板");
                assert_eq!(font_family, TEXT_FONT_FAMILY);
                assert_eq!(*font_size, 24.0);
                assert_eq!(color, "#A1B2C3");
                assert_eq!(align, "center");
            }
            other => panic!("应返回文字载荷：{other:?}"),
        }
        assert_eq!(view.missing_textures, 0);
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pin_board_bitmaps"),
            0
        );
        assert!(crate::pin_board::PinBoardReferenceInventory
            .collect_targets(&connection)
            .unwrap()
            .is_empty());

        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        assert_eq!(
            export_images(
                &connection,
                &root,
                board.board_id,
                &item_ids,
                export_directory.to_str().unwrap(),
                |_, _| {}
            )
            .unwrap(),
            0
        );
        let error =
            read_texture(&connection, &root, board.board_id, item_ids[0], 1024, 1).unwrap_err();
        assert!(error.contains("没有 DDS"), "{error}");
        let error = read_image_png(&connection, &root, board.board_id, item_ids[0]).unwrap_err();
        assert!(error.contains("没有位图"), "{error}");
    }

    #[test]
    fn writes_text_png_and_rejects_invalid_payloads() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "文字导出").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (_, item_ids) = create_text(
            &mut connection,
            &root,
            board.board_id,
            "导出文字",
            32.0,
            "#112233",
            "left",
            80.0,
            40.0,
            400.0,
            300.0,
            &revision,
        )
        .unwrap();
        let item_id = item_ids[0];
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let output = export_directory.to_str().unwrap();
        let png = test_png(3, 2);

        let first =
            write_text_png(&connection, &root, board.board_id, item_id, output, &png).unwrap();
        let decoded = image::open(&first).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        let second =
            write_text_png(&connection, &root, board.board_id, item_id, output, &png).unwrap();
        assert_ne!(first, second, "重复导出不得覆盖已有文件");

        let error =
            write_text_png(&connection, &root, board.board_id, item_id, output, &[]).unwrap_err();
        assert!(error.contains("大小"), "{error}");
        let error = write_text_png(
            &connection,
            &root,
            board.board_id,
            item_id,
            output,
            b"not a png",
        )
        .unwrap_err();
        assert!(error.contains("PNG"), "{error}");
        let error = write_text_png(
            &connection,
            &root,
            board.board_id,
            item_id,
            output,
            &test_png(8193, 1),
        )
        .unwrap_err();
        assert!(error.contains("尺寸"), "{error}");
        let error = write_text_png(
            &connection,
            &root,
            board.board_id,
            item_id,
            root.to_str().unwrap(),
            &png,
        )
        .unwrap_err();
        assert!(error.contains("仓库"), "{error}");

        let bitmap_path =
            insert_deleted_state_image(&connection, board.board_id, &root, &artwork_id);
        let bitmap_item: i64 = connection
            .query_row(
                "SELECT item_id FROM pin_board_bitmaps ORDER BY item_id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let error = write_text_png(
            &connection,
            &root,
            board.board_id,
            bitmap_item,
            output,
            &png,
        )
        .unwrap_err();
        assert!(error.contains("只有文字条目"), "{error}");
        assert!(bitmap_path.is_file());
    }

    /// 造一张真实的 PNG 载荷，供文字导出写盘测试使用。
    fn test_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([200, 10, 20, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    }

    /// 造一块只含一张删除态图片的画板：条目/位图记录 + step 0 删除节点 + DDS 实体。
    fn insert_deleted_state_image(
        connection: &Connection,
        board_id: i64,
        root: &Path,
        artwork_id: &str,
    ) -> PathBuf {
        let now = storage::now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO pin_board_items (board_id, kind, created_ms) VALUES (?1, 0, ?2)",
                params![board_id, now],
            )
            .unwrap();
        let item_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                 VALUES (?1, ?2, 4, 4)",
                params![item_id, format!("{item_id}.dds")],
            )
            .unwrap();
        let transform = r#"{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}"#;
        connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (?1, ?2, 0, 1, 1, 0, ?3)",
                params![board_id, item_id, transform],
            )
            .unwrap();
        let directory = root
            .join("artworks")
            .join(artwork_id)
            .join(BOARD_DIRECTORY)
            .join(board_id.to_string());
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{item_id}.dds"));
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
    use std::path::Path;

    use rusqlite::{params, Connection};

    fn initialize(root: &Path) {
        std::fs::create_dir(root).unwrap();
        crate::library::initialize(root).unwrap();
    }

    fn downgrade_to_v1(connection: &Connection) {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE pin_board_history;
                 DROP TABLE pin_board_texts;
                 DROP TABLE pin_board_bitmaps;
                 DROP TABLE pin_board_items;
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
                 DROP TABLE repository_backup_runs;
                 DROP INDEX history_nodes_snapshot_path;
                 DROP INDEX history_nodes_delta_path;
                 DROP INDEX certification_records_stored_path;
                 DROP INDEX certification_records_output_path;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail_version;
                 ALTER TABLE history_nodes DROP COLUMN verified_ms;
                 ALTER TABLE history_nodes DROP COLUMN verify_error;
                 UPDATE repository_meta SET value = '1' WHERE key = 'schema_version';",
            )
            .unwrap();
    }

    /// 把 v5 画板表按 v4 形状重建并保留数据：v4 只有图片载荷与 `image_id`
    /// 历史，也没有路径索引、备份运行表和节点级新列。
    fn downgrade_to_v4(connection: &Connection) {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 CREATE TABLE pin_board_images (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   file_path TEXT NOT NULL,
                   width INTEGER NOT NULL CHECK (width > 0),
                   height INTEGER NOT NULL CHECK (height > 0),
                   created_ms INTEGER NOT NULL
                 );
                 CREATE INDEX pin_board_images_board ON pin_board_images(board_id, id);
                 INSERT INTO pin_board_images (id, board_id, file_path, width, height, created_ms)
                   SELECT item.id, item.board_id, bitmap.file_path, bitmap.width,
                          bitmap.height, item.created_ms
                     FROM pin_board_items item
                     JOIN pin_board_bitmaps bitmap ON bitmap.item_id = item.id;
                 CREATE TABLE pin_board_history_v4 (
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   image_id INTEGER NOT NULL REFERENCES pin_board_images(id) ON DELETE CASCADE,
                   step INTEGER NOT NULL CHECK (step >= 0),
                   deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                   layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 2),
                   sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
                   transform_json TEXT NOT NULL,
                   PRIMARY KEY (board_id, image_id, step)
                 );
                 INSERT INTO pin_board_history_v4
                   (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                   SELECT board_id, item_id, step, deleted, layer, sort_order, transform_json
                     FROM pin_board_history;
                 DROP TABLE pin_board_history;
                 DROP TABLE pin_board_texts;
                 DROP TABLE pin_board_bitmaps;
                 DROP TABLE pin_board_items;
                 ALTER TABLE pin_board_history_v4 RENAME TO pin_board_history;

                 DROP TABLE repository_backup_runs;
                 DROP INDEX history_nodes_snapshot_path;
                 DROP INDEX history_nodes_delta_path;
                 DROP INDEX certification_records_stored_path;
                 DROP INDEX certification_records_output_path;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail_version;
                 ALTER TABLE history_nodes DROP COLUMN verified_ms;
                 ALTER TABLE history_nodes DROP COLUMN verify_error;
                 ALTER TABLE branches ADD COLUMN verified_history_id TEXT;
                 ALTER TABLE branches ADD COLUMN verified_ms INTEGER;
                 ALTER TABLE branches ADD COLUMN verify_error TEXT;
                 UPDATE repository_meta SET value = '4' WHERE key = 'schema_version';",
            )
            .unwrap();
    }

    fn table_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = ?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn index_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'index' AND name = ?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn columns(connection: &Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn insert_board_fixture(connection: &Connection, artwork_id: &str) {
        let now = crate::storage::now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO library_nodes (id, kind, title, position, created_ms, updated_ms)
                 VALUES (?1, 'artwork', '迁移作品', 0, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artworks (id, created_ms, updated_ms) VALUES (?1, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES (1, ?1, '迁移画板', 0, 2, 2, ?2, ?3, ?3)",
                params![artwork_id, "0".repeat(64), now],
            )
            .unwrap();
        let transform = r#"{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}"#;
        for (item_id, path) in [(11, "one.dds"), (12, "two.dds")] {
            connection
                .execute(
                    "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                     VALUES (?1, 1, 0, ?2)",
                    params![item_id, now],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                     VALUES (?1, ?2, 8, 8)",
                    params![item_id, path],
                )
                .unwrap();
        }
        for step in 0..3 {
            connection
                .execute(
                    "INSERT INTO pin_board_history
                       (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 11, ?1, 0, 1, ?1, ?2)",
                    params![step, transform],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (1, 12, 0, 1, 2, 0, ?1)",
                [transform],
            )
            .unwrap();
    }

    /// v1 仓库（无素材板表、分支表也没有 v2/v3/v4 追加列）打开时必须通过追加式
    /// 迁移升级到当前版本 v5，且 v1 形状的既有结构保持可读。
    #[test]
    fn migrates_a_v1_repository_to_the_current_version_append_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        {
            let connection = crate::storage::open(&root).unwrap();
            downgrade_to_v1(&connection);
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
        assert_eq!(version, 5);
        for table in [
            "pin_boards",
            "pin_board_items",
            "pin_board_bitmaps",
            "pin_board_texts",
            "pin_board_history",
            "repository_backup_runs",
        ] {
            assert!(
                table_exists(&connection, table),
                "表 {table} 应在迁移后存在"
            );
        }
        assert!(!table_exists(&connection, "pin_board_images"));
        let branch_columns = columns(&connection, "branches");
        for column in [
            "backup_quick_enabled",
            "last_source_size",
            "last_source_modified_ms",
        ] {
            assert!(branch_columns.iter().any(|item| item == column), "{column}");
        }
        for column in ["verified_history_id", "verified_ms", "verify_error"] {
            assert!(
                !branch_columns.iter().any(|item| item == column),
                "{column}"
            );
        }
        let node_columns = columns(&connection, "history_nodes");
        for column in [
            "thumbnail",
            "thumbnail_version",
            "verified_ms",
            "verify_error",
        ] {
            assert!(node_columns.iter().any(|item| item == column), "{column}");
        }
    }

    /// v4 → v5 必须保留图片记录、条目 ID 与每一步变换历史，并补齐索引、
    /// 节点级校验列和整仓备份记录表，同时移除 v4 的分支校验列。
    #[test]
    fn migrates_v4_pin_board_items_and_step_history() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            downgrade_to_v4(&connection);
        }

        crate::library::open_existing(&root).unwrap();

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            5
        );
        assert!(!table_exists(&connection, "pin_board_images"));
        let items: Vec<(i64, i64, i64)> = {
            let mut statement = connection
                .prepare("SELECT id, board_id, kind FROM pin_board_items ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(items, vec![(11, 1, 0), (12, 1, 0)]);
        let bitmaps: Vec<(i64, String, i64, i64)> = {
            let mut statement = connection
                .prepare("SELECT item_id, file_path, width, height FROM pin_board_bitmaps ORDER BY item_id")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            bitmaps,
            vec![
                (11, "one.dds".to_owned(), 8, 8),
                (12, "two.dds".to_owned(), 8, 8)
            ]
        );
        let history: Vec<(i64, i64, i64, i64)> = {
            let mut statement = connection
                .prepare(
                    "SELECT item_id, step, deleted, layer FROM pin_board_history
                     ORDER BY item_id, step",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            history,
            vec![(11, 0, 0, 1), (11, 1, 0, 1), (11, 2, 0, 1), (12, 0, 1, 2)]
        );
        for index in [
            "history_nodes_snapshot_path",
            "history_nodes_delta_path",
            "certification_records_stored_path",
            "certification_records_output_path",
        ] {
            assert!(index_exists(&connection, index), "索引 {index}");
        }
        assert!(table_exists(&connection, "repository_backup_runs"));
        let node_columns = columns(&connection, "history_nodes");
        for column in [
            "thumbnail",
            "thumbnail_version",
            "verified_ms",
            "verify_error",
        ] {
            assert!(node_columns.iter().any(|item| item == column), "{column}");
        }
        let branch_columns = columns(&connection, "branches");
        for column in ["verified_history_id", "verified_ms", "verify_error"] {
            assert!(
                !branch_columns.iter().any(|item| item == column),
                "{column}"
            );
        }

        // 外键级联在新表上继续生效。
        connection
            .execute("DELETE FROM pin_boards WHERE id = 1", [])
            .unwrap();
        for table in [
            "pin_board_items",
            "pin_board_bitmaps",
            "pin_board_texts",
            "pin_board_history",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    /// 迁移中若外键检查失败，整笔事务回滚：版本仍是 v4，旧表仍在。
    #[test]
    fn failed_v4_migration_rolls_back_to_v4() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            downgrade_to_v4(&connection);
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     INSERT INTO pin_board_history
                       (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 999, 0, 0, 1, 0, '{}');",
                )
                .unwrap();
        }

        let error = crate::library::open_existing(&root).unwrap_err();
        assert!(error.contains("外键完整性检查失败"), "{error}");

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            4
        );
        assert!(table_exists(&connection, "pin_board_images"));
        assert!(!table_exists(&connection, "pin_board_items"));
    }

    /// 旧程序（支持到 v4）必须拒绝打开 v5 仓库，且不改动版本与表结构。
    #[test]
    fn rejects_repositories_from_newer_versions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        {
            let connection = crate::storage::open(&root).unwrap();
            connection
                .execute(
                    "UPDATE repository_meta SET value = '6' WHERE key = 'schema_version'",
                    [],
                )
                .unwrap();
        }

        let error = crate::library::open_existing(&root).unwrap_err();
        assert!(error.contains("更新版本"), "{error}");
        let connection = crate::storage::open(&root).unwrap();
        let version: String = connection
            .query_row(
                "SELECT value FROM repository_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "6");
        assert!(table_exists(&connection, "pin_board_items"));
    }
}
