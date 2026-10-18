use std::{fs, path::Path};

use rusqlite::{params, Connection};

use crate::{
    contracts::{
        ManagedFile, ManagedFileOwner, ReferenceInventory, ScanCandidateFile, ValidationOutcome,
    },
    storage,
};

use super::repository::{board_relative_path, is_dds_name, parse_board_dds_path, BOARD_DIRECTORY};

/// 画板 DDS 的引用清单。条目文件路径在 DB 里是 `<image-id>.dds`，仓库中的完整
/// 位置由 `artworks/<artwork-id>/boards/<board-id>/` 与文件名拼出。
///
/// 只有规范文件名的记录参与引用；`file_path` 被改坏的记录由完整性扫描报告，不把
/// 它声称的任意路径当作有效引用。
pub(crate) struct PinBoardReferenceInventory;

impl ReferenceInventory for PinBoardReferenceInventory {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String> {
        let mut statement = connection
            .prepare(
                "SELECT b.artwork_id, i.board_id, i.id, i.file_path
                   FROM pin_board_images i
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
            let (artwork_id, board_id, image_id, file_path) =
                row.map_err(storage::database_error)?;
            if file_path != format!("{image_id}.dds") {
                continue;
            }
            targets.push(ManagedFile {
                path: format!("{}/{file_path}", board_relative_path(&artwork_id, board_id)),
                owner: ManagedFileOwner::PinBoardDds,
                deletable: true,
            });
        }
        Ok(targets)
    }

    fn validate_target(
        &self,
        connection: &Connection,
        target: &ManagedFile,
    ) -> Result<ValidationOutcome, String> {
        if target.owner != ManagedFileOwner::PinBoardDds {
            return Ok(ValidationOutcome::OwnerMismatch(target.owner));
        }
        let Some((artwork_id, board_id, image_id)) = parse_board_dds_path(&target.path) else {
            return Ok(ValidationOutcome::Unreferenced);
        };
        let referenced = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pin_board_images i
                   JOIN pin_boards b ON b.id = i.board_id
                   WHERE i.id = ?1 AND i.board_id = ?2 AND b.artwork_id = ?3
                     AND i.file_path = ?4
                 )",
                params![image_id, board_id, artwork_id, format!("{image_id}.dds")],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage::database_error)?;
        Ok(if referenced {
            ValidationOutcome::Valid
        } else {
            ValidationOutcome::Unreferenced
        })
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
                 INSERT INTO pin_board_images
                   (id, board_id, file_path, width, height, created_ms)
                 VALUES ({image_id}, {board_id}, '{image_id}.dds', 4, 4, 0);",
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
            .execute(
                "INSERT INTO pin_board_images (id, board_id, file_path, width, height, created_ms)
                 VALUES (8, 1, 'elsewhere.dds', 4, 4, 0)",
                [],
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
