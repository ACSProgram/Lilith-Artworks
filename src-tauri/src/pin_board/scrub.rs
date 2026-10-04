//! 画板 DDS 的完整性扫描：记录 → 文件（缺失/损坏）与文件 → 记录（孤儿）。
//!
//! 只报告、不自动修复：缺失 DDS 的修复属历史迁移议题。扫描在仓库操作锁内运行
//! （调用方保证与调度器、前台长命令、画板写入互斥），因此不设宽限期；孤儿 DDS 的
//! 清理入口是 `cleanup::scan_unreferenced` 的报告 + 确认清理流程。

use std::{collections::HashSet, fs, path::Path};

use crate::{pin_board::dds, storage};

use super::repository::{board_relative_path, is_dds_name, BOARD_DIRECTORY};

/// 画板 DDS 完整性扫描结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoardDdsReport {
    /// 检查的 `pin_board_images` 记录数。
    pub(crate) images: u64,
    /// 记录存在但 DDS 文件缺失。
    pub(crate) missing: u64,
    /// 文件存在但路径归属、DDS/BC7 头、尺寸、长度或 BC7 解码校验失败。
    pub(crate) corrupt: u64,
    /// 磁盘上存在但无 `pin_board_images` 记录的 DDS。
    pub(crate) orphans: u64,
}

enum DdsCheck {
    Ok,
    Missing,
    Corrupt,
}

/// 双向检查画板 DDS：逐条 `pin_board_images` 记录校验其 DDS，再遍历画板目录统计
/// 无记录的孤儿 DDS。只报告，不修改数据库或磁盘。
pub(crate) fn scrub_board_dds(
    root: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(u64, u64),
) -> Result<BoardDdsReport, String> {
    let connection = storage::open(root)?;
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT b.artwork_id, i.board_id, i.id, i.file_path, i.width, i.height
                   FROM pin_board_images i
                   JOIN pin_boards b ON b.id = i.board_id
                  ORDER BY b.artwork_id, i.board_id, i.id",
            )
            .map_err(storage::database_error)?;
        let values = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(storage::database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage::database_error)?;
        values
    };
    drop(connection);
    let mut disk = Vec::new();
    collect_board_dds_files(root, &mut disk)?;

    let total = (records.len() + disk.len()) as u64;
    let mut report = BoardDdsReport::default();
    let mut checked = 0_u64;
    let mut referenced = HashSet::new();
    for (artwork_id, board_id, image_id, file_path, width, height) in &records {
        if cancelled() {
            return Err("画板 DDS 完整性检查已取消".into());
        }
        report.images += 1;
        let relative = board_dds_relative_path(artwork_id, *board_id, *image_id);
        referenced.insert(relative.clone());
        let expected_name = format!("{image_id}.dds");
        let check = if file_path != &expected_name {
            DdsCheck::Corrupt
        } else {
            match (u32::try_from(*width), u32::try_from(*height)) {
                (Ok(width), Ok(height)) => check_dds_file(root, &relative, width, height),
                _ => DdsCheck::Corrupt,
            }
        };
        match check {
            DdsCheck::Ok => {}
            DdsCheck::Missing => report.missing += 1,
            DdsCheck::Corrupt => report.corrupt += 1,
        }
        checked += 1;
        progress(checked, total);
    }
    for relative in disk {
        if cancelled() {
            return Err("画板 DDS 完整性检查已取消".into());
        }
        if !referenced.contains(&relative) {
            report.orphans += 1;
        }
        checked += 1;
        progress(checked, total);
    }
    Ok(report)
}

fn board_dds_relative_path(artwork_id: &str, board_id: i64, image_id: i64) -> String {
    format!(
        "{}/{image_id}.dds",
        board_relative_path(artwork_id, board_id)
    )
}

/// 校验单条记录的 DDS：路径归属（文件名须为 `<image-id>.dds`，由调用方先行判定）、
/// 文件存在、DDS/DX10/BC7 头、声明尺寸与记录一致、数据长度、BC7 解码。
fn check_dds_file(root: &Path, relative: &str, width: u32, height: u32) -> DdsCheck {
    let path = match storage::resolve_path(root, relative) {
        Ok(path) => path,
        Err(_) => return DdsCheck::Corrupt,
    };
    if !path.is_file() {
        return if path.exists() {
            DdsCheck::Corrupt
        } else {
            DdsCheck::Missing
        };
    }
    let bytes = match dds::read_limited(&path, dds::MAX_DDS_BYTES, "画板 DDS 图片") {
        Ok(bytes) => bytes,
        Err(_) => return DdsCheck::Corrupt,
    };
    let dimensions = match dds::dds_dimensions(&bytes) {
        Ok(dimensions) => dimensions,
        Err(_) => return DdsCheck::Corrupt,
    };
    if dimensions != (width, height) {
        return DdsCheck::Corrupt;
    }
    if dds::validate_dds_payload(&bytes, width, height).is_err() {
        return DdsCheck::Corrupt;
    }
    if dds::validate_bc7_decodable(&bytes, width, height).is_err() {
        return DdsCheck::Corrupt;
    }
    DdsCheck::Ok
}

/// 收集仓库内全部画板 DDS 的仓库相对路径（`/` 分隔）。目录缺失按空处理。
fn collect_board_dds_files(root: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let artworks = root.join("artworks");
    let entries = match fs::read_dir(&artworks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("无法读取作品目录：{error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取作品目录项：{error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("无法读取作品目录项类型：{error}"))?
            .is_dir()
        {
            continue;
        }
        let boards = entry.path().join(BOARD_DIRECTORY);
        let board_entries = match fs::read_dir(&boards) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法读取画板目录：{error}")),
        };
        for board in board_entries {
            let board = board.map_err(|error| format!("无法读取画板目录项：{error}"))?;
            if !board
                .file_type()
                .map_err(|error| format!("无法读取画板目录项类型：{error}"))?
                .is_dir()
            {
                continue;
            }
            collect_dds_in_directory(root, &board.path(), files)?;
        }
    }
    Ok(())
}

fn collect_dds_in_directory(
    root: &Path,
    directory: &Path,
    files: &mut Vec<String>,
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
            files.push(storage::relative_path(root, &entry.path())?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use rusqlite::params;

    use super::*;

    fn test_repository() -> (tempfile::TempDir, String, i64) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir(&root).unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork_id = storage::new_id();
        let now = storage::now_ms().unwrap();
        let connection = storage::open(&root).unwrap();
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
        connection
            .execute(
                "INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES (1, ?1, '画板', 0, 0, 0, ?2, ?3, ?3)",
                params![artwork_id, "0".repeat(64), now],
            )
            .unwrap();
        (directory, artwork_id, 1)
    }

    fn insert_image(root: &Path, board_id: i64, image_id: i64, width: u32, height: u32) {
        storage::open(root)
            .unwrap()
            .execute(
                "INSERT INTO pin_board_images
                   (id, board_id, file_path, width, height, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                params![image_id, board_id, format!("{image_id}.dds"), width, height],
            )
            .unwrap();
    }

    fn board_directory(root: &Path, artwork_id: &str, board_id: i64) -> std::path::PathBuf {
        root.join("artworks")
            .join(artwork_id)
            .join(BOARD_DIRECTORY)
            .join(board_id.to_string())
    }

    /// 生成一份合法 BC7 DDS（`width`×`height`，4 的倍数）。
    fn valid_dds(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        dds::encode_bc7_dds(&image).unwrap()
    }

    #[test]
    fn reports_missing_corrupt_and_orphan_dds() {
        let (directory, artwork_id, board_id) = test_repository();
        let root = directory.path().join("repository");
        let board = board_directory(&root, &artwork_id, board_id);
        fs::create_dir_all(&board).unwrap();

        // 缺失：记录存在但没有文件。
        insert_image(&root, board_id, 1, 8, 8);
        // 损坏：文件存在但头部尺寸与记录不符。
        insert_image(&root, board_id, 2, 8, 8);
        fs::write(board.join("2.dds"), valid_dds(4, 4)).unwrap();
        // 正常：文件与记录一致。
        insert_image(&root, board_id, 3, 8, 8);
        fs::write(board.join("3.dds"), valid_dds(8, 8)).unwrap();
        // 孤儿：磁盘上有 DDS 但没有对应记录。
        fs::write(board.join("9.dds"), valid_dds(4, 4)).unwrap();

        let report = scrub_board_dds(&root, || false, |_, _| {}).unwrap();

        assert_eq!(
            report,
            BoardDdsReport {
                images: 3,
                missing: 1,
                corrupt: 1,
                orphans: 1,
            }
        );
    }

    #[test]
    fn accepts_a_fully_consistent_board() {
        let (directory, artwork_id, board_id) = test_repository();
        let root = directory.path().join("repository");
        let board = board_directory(&root, &artwork_id, board_id);
        fs::create_dir_all(&board).unwrap();
        insert_image(&root, board_id, 1, 8, 8);
        fs::write(board.join("1.dds"), valid_dds(8, 8)).unwrap();

        assert_eq!(
            scrub_board_dds(&root, || false, |_, _| {}).unwrap(),
            BoardDdsReport {
                images: 1,
                ..BoardDdsReport::default()
            }
        );
    }

    #[test]
    fn treats_a_mismatched_file_path_as_corrupt() {
        let (directory, artwork_id, board_id) = test_repository();
        let root = directory.path().join("repository");
        let board = board_directory(&root, &artwork_id, board_id);
        fs::create_dir_all(&board).unwrap();
        insert_image(&root, board_id, 1, 8, 8);
        storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE pin_board_images SET file_path = 'elsewhere.dds' WHERE id = 1",
                [],
            )
            .unwrap();
        fs::write(board.join("1.dds"), valid_dds(8, 8)).unwrap();

        let report = scrub_board_dds(&root, || false, |_, _| {}).unwrap();

        assert_eq!(report.corrupt, 1);
        assert_eq!(report.missing, 0);
        assert_eq!(report.orphans, 0);
    }
}
