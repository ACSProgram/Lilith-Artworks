//! 素材板持久化测试的公共夹具。
//!
//! 这些 helper 同时被 `boards` / `view` / `save` / `create` / `bitmap_io` 的测试
//! 使用，因此集中放在这里；生产代码不编译本模块。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use super::super::dds;
use super::super::stroke;
use super::paths::board_directory;
use super::{PinBoardClipboardItem, PinBoardView};
use crate::storage;

pub(super) fn test_repository() -> (tempfile::TempDir, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repository");
    std::fs::create_dir(&root).unwrap();
    crate::library::initialize(&root).unwrap();
    let connection = crate::storage::open(&root).unwrap();
    (directory, connection)
}

pub(super) fn create_test_artwork(connection: &Connection) -> String {
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

/// 造一段自由笔几何：头部 + `points` 个点，坐标为顺序填充的归一化值。
pub(super) fn pen_geometry(points: usize) -> Vec<u8> {
    let mut geometry = vec![stroke::STROKE_GEOMETRY_VERSION, stroke::TOOL_PEN, 0, 0];
    geometry.extend_from_slice(&(points as u32).to_le_bytes());
    for index in 0..points {
        let x = ((index * 65535) / points.max(1)) as u16;
        geometry.extend_from_slice(&x.to_le_bytes());
        geometry.extend_from_slice(&x.to_le_bytes());
        geometry.push(128);
    }
    geometry
}

pub(super) fn shape_geometry(tool: u8, direction: u8) -> Vec<u8> {
    let mut geometry = vec![stroke::STROKE_GEOMETRY_VERSION, tool, direction, 0];
    geometry.extend_from_slice(&0u32.to_le_bytes());
    geometry
}

/// 造一块只含一张删除态图片的画板：条目/位图记录 + step 0 删除节点 + DDS 实体。
pub(super) fn insert_deleted_state_image(
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
    let directory = board_directory(root, artwork_id, board_id);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{item_id}.dds"));
    fs::write(&path, b"dds-bytes").unwrap();
    path
}

pub(super) fn board_revision(connection: &Connection, board_id: i64) -> String {
    connection
        .query_row(
            "SELECT revision FROM pin_boards WHERE id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(super) fn scalar(connection: &Connection, sql: &str) -> i64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

pub(super) fn scalar_for_board(connection: &Connection, sql: &str, board_id: i64) -> i64 {
    connection
        .query_row(sql, [board_id], |row| row.get(0))
        .unwrap()
}

/// 造一张「活着」的位图条目：真实 BC7 DDS + 位图记录 + step 0 的可见变换。
///
/// 与 `insert_deleted_state_image` 相对：粘贴要求源条目存在且未删除、源 DDS 可读，
/// 因此这里写的是可解码的 DDS，而不是随便几个字节。
pub(super) fn insert_live_bitmap(
    connection: &Connection,
    board_id: i64,
    root: &Path,
    artwork_id: &str,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> i64 {
    let now = storage::now_ms().unwrap();
    connection
        .execute(
            "INSERT INTO pin_board_items (board_id, kind, created_ms) VALUES (?1, 0, ?2)",
            params![board_id, now],
        )
        .unwrap();
    let item_id = connection.last_insert_rowid();
    let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 60, 255]));
    let dds = dds::encode_bc7_dds(&image).unwrap();
    let directory = board_directory(root, artwork_id, board_id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{item_id}.dds")), &dds).unwrap();
    connection
        .execute(
            "INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                 VALUES (?1, ?2, 4, 4)",
            params![item_id, format!("{item_id}.dds")],
        )
        .unwrap();
    let transform = format!(
        r#"{{"points":[[{left},{top}],[{right},{top}],[{right},{bottom}],[{left},{bottom}]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}}"#,
        right = left + width,
        bottom = top - height,
    );
    connection
        .execute(
            "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (?1, ?2, 0, 0, 1, 0, ?3)",
            params![board_id, item_id, transform],
        )
        .unwrap();
    item_id
}

pub(super) fn clipboard_item(kind: &str, board_id: i64, item_id: i64) -> PinBoardClipboardItem {
    PinBoardClipboardItem {
        kind: kind.to_owned(),
        source_board_id: board_id,
        source_item_id: item_id,
    }
}

pub(super) fn points_of(view: &PinBoardView, item_id: i64) -> [[f64; 2]; 4] {
    view.items
        .iter()
        .find(|item| item.item_id == item_id)
        .unwrap_or_else(|| panic!("画板视图里没有条目 {item_id}"))
        .points
}
