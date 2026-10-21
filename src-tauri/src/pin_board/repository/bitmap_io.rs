//! 位图条目的文件出口：导出 PNG、剪贴板 PNG 与纹理读取。
//!
//! 三者都先经 `canonical_item_dds` 校验规范文件名与仓库边界；只有位图有条目文件，
//! 文字与笔迹在这里明确返回「没有 DDS / 没有位图」。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use super::super::dds;
use super::super::transfer::TRANSFER_CANCELLED;
use super::paths::canonical_item_dds;
use super::view::open_board_context;
use super::ITEM_KIND_BITMAP;
use crate::storage;

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
/// 写出的字节由调用方提供（位图解码后重编码），目录权限、命名冲突与崩溃残留的语义
/// 因此只有一处；整画布导出走 `canvas_export.rs` 的流式写盘，两者同一口径。
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

/// 导出选中位图为 PNG 到外部目录，返回成功数量。只接受位图：文字与笔迹没有单条
/// 导出入口（它们的呈现由整画布导出覆盖），传入非位图 id 会被拒绝。
pub(crate) fn export_images(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    item_ids: &[i64],
    output_directory: &str,
    mut on_progress: impl FnMut(usize, usize),
    should_cancel: impl Fn() -> bool,
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
        // 取消在条目边界生效：已经写出的 PNG 都是完整文件，保留即可（用户拿到的是一
        // 部分导出），错误文案交给前端改写成「已取消导出」。
        if should_cancel() {
            return Err(TRANSFER_CANCELLED.into());
        }
        let (kind, file_path) = item_kind_and_file(connection, board_id, *item_id)?;
        if kind != ITEM_KIND_BITMAP {
            on_progress(index + 1, total);
            continue;
        }
        let Some(file_path) = file_path else {
            on_progress(index + 1, total);
            continue;
        };
        // 记录被改坏、文件缺失都只跳过这一条：导出是逐条的，不该被一条坏记录打断。
        let canonical =
            canonical_item_dds(root, &context.directory, *item_id, &file_path, "DDS 图片");
        let source = match canonical {
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
        return Err("该条目没有位图，请在画板内渲染后复制".into());
    }
    let file_path = file_path.ok_or("位图条目缺少文件记录")?;
    let source = canonical_item_dds(root, &context.directory, item_id, &file_path, "DDS 图片")?;
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
        return Err("该条目没有 DDS，由前端按需绘制".into());
    }
    let file_path = file_path.ok_or("位图条目缺少文件记录")?;
    let canonical = canonical_item_dds(root, &context.directory, item_id, &file_path, "DDS 图片")?;
    let max_dimension = max_dimension.clamp(dds::MIN_TEXTURE_DIMENSION, dds::MAX_TEXTURE_DIMENSION);
    let key = dds::TextureCacheKey::new(root, board_id, item_id, max_dimension);
    if let Some(cached) = dds::texture_cache_get(&key) {
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
    use std::fs;

    use crate::pin_board::repository::boards::create_board;
    use crate::pin_board::repository::fixtures::{create_test_artwork, test_repository};
    use crate::pin_board::transfer::TRANSFER_CANCELLED;
    use crate::platform::path::display_path;

    #[test]
    fn cancelling_an_export_stops_before_the_first_item() {
        let (directory, connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let mut connection = connection;
        let board = create_board(&mut connection, &artwork_id, "板").unwrap();
        let output = directory.path().join("out");
        fs::create_dir(&output).unwrap();

        let error = export_images(
            &connection,
            &root,
            board.board_id,
            &[1],
            &display_path(&output),
            |_, _| {},
            || true,
        )
        .unwrap_err();
        assert_eq!(error, TRANSFER_CANCELLED);
        assert!(
            fs::read_dir(&output).unwrap().next().is_none(),
            "取消的导出不得写出任何文件"
        );
    }
}
