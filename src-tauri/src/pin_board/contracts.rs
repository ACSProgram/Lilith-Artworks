//! 素材板的规模上限与仓库中立契约。
//!
//! 上限（[`MAX_BOARD_ITEMS`] / [`MAX_IMPORT_FILES`] / [`MAX_CLIPBOARD_IMAGE_BYTES`]）
//! 放在这里而不是 `dds.rs` 或持久化模块：它们是「素材板能装多少」的业务口径，与图像
//! 格式、SQLite 写法都无关；`repository/create.rs`、`repository/save.rs` 与无头入口
//! 共用同一份值。
//!
//! [`PinBoardReferenceInventory`] 是画板 DDS 的引用清单实现，供 `cleanup` /
//! 完整性扫描按中立契约核对「记录 ↔ 文件」，使那些模块不必认识画板路径布局。

use std::{fs, path::Path};

use rusqlite::Connection;

use crate::{
    contracts::{ManagedFile, ManagedFileOwner, ReferenceInventory, ScanCandidateFile},
    storage,
};

use super::repository::{is_dds_name, is_item_dds_file, relative_item_dds, BOARD_DIRECTORY};

/// 单块画板的条目上限（全部 kind：位图、文字与笔迹合计）。
pub(crate) const MAX_BOARD_ITEMS: usize = 20_000;
/// 一次导入与一次画板内粘贴共用的条目数上限。
pub(crate) const MAX_IMPORT_FILES: usize = 256;
/// 剪贴板位图（含拖入导入的字节）大小上限。
pub(crate) const MAX_CLIPBOARD_IMAGE_BYTES: usize = 256 * 1024 * 1024;

/// 画板 DDS 的引用清单。位图载荷的文件路径在 DB 里是 `<item-id>.dds`，仓库中的
/// 完整位置由 `artworks/<artwork-id>/boards/<board-id>/` 与文件名拼出。文字与笔迹
/// 条目没有位图载荷行（`file_path` 只存在于 `pin_board_bitmaps`），自然不会进入
/// 路径清单。
///
/// 只有规范文件名的记录参与引用；`file_path` 被改坏的记录由完整性扫描报告，不把
/// 它声称的任意路径当作有效引用。
pub(crate) struct PinBoardReferenceInventory;

impl ReferenceInventory for PinBoardReferenceInventory {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String> {
        let mut statement = connection
            .prepare(
                "SELECT b.artwork_id, i.board_id, i.id, bm.file_path
                   FROM pin_board_items i
                   JOIN pin_board_bitmaps bm ON bm.item_id = i.id
                   JOIN pin_boards b ON b.id = i.board_id",
            )
            .map_err(storage::database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(storage::database_error)?;
        let mut targets = Vec::new();
        for row in rows {
            let (artwork_id, board_id, item_id, file_path) =
                row.map_err(storage::database_error)?;
            if !is_item_dds_file(item_id, &file_path) {
                continue;
            }
            targets.push(ManagedFile {
                path: relative_item_dds(&artwork_id, board_id, item_id),
                owner: ManagedFileOwner::PinBoardDds,
            });
        }
        Ok(targets)
    }

    fn scan_candidates(
        &self,
        artwork_directory: &Path,
        files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
        let boards = artwork_directory.join(BOARD_DIRECTORY);
        let entries = match fs::read_dir(&boards) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("无法读取画板目录：{error}")),
        };
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取画板目录项：{error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("无法读取画板目录项类型：{error}"))?;
            if !file_type.is_dir() {
                continue;
            }
            collect_dds_directory(&entry.path(), files)?;
        }
        Ok(())
    }
}

fn collect_dds_directory(
    directory: &Path,
    files: &mut Vec<ScanCandidateFile>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("无法读取画板图片目录 {}：{error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取画板图片目录项：{error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("无法读取画板图片目录项类型：{error}"))?
            .is_file()
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_dds_name(name) {
            files.push(ScanCandidateFile {
                path: entry.path(),
                reason: "画板图片未被引用",
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn insert_image(connection: &Connection, artwork_id: &str, board_id: i64, image_id: i64) {
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES ({board_id}, '{artwork_id}', 'board', 0, 0, 0, '{}', 0, 0);
                 INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                 VALUES ({image_id}, {board_id}, 0, 0);
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                 VALUES ({image_id}, '{image_id}.dds', 4, 4);",
                "0".repeat(64)
            ))
            .unwrap();
    }

    #[test]
    fn collects_only_canonical_dds_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let artwork_id = storage::new_id();
        let connection = storage::open(&root).unwrap();
        insert_image(&connection, &artwork_id, 1, 7);
        connection
            .execute_batch(
                "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                   VALUES (8, 1, 0, 0);
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                   VALUES (8, 'elsewhere.dds', 4, 4);",
            )
            .unwrap();

        let targets = PinBoardReferenceInventory
            .collect_targets(&connection)
            .unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].owner, ManagedFileOwner::PinBoardDds);
        assert_eq!(
            targets[0].path,
            format!("artworks/{artwork_id}/boards/1/7.dds")
        );
    }

    #[test]
    fn scans_only_dds_named_files() {
        let directory = tempfile::tempdir().unwrap();
        let board = directory.path().join("artworks/a/boards/1");
        fs::create_dir_all(&board).unwrap();
        let dds = board.join("3.dds");
        let unrelated = board.join("notes.txt");
        fs::write(&dds, b"payload").unwrap();
        fs::write(&unrelated, b"notes").unwrap();

        let mut files = Vec::new();
        PinBoardReferenceInventory
            .scan_candidates(directory.path().join("artworks/a").as_path(), &mut files)
            .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, dds);
    }
}
