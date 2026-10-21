//! 画板目录布局与位图 DDS 路径解析。
//!
//! [`board_relative_path`] 与 [`parse_board_dds_path`] 是同一布局的正反两面：写入侧
//! 拼仓库相对路径，`cleanup` / `scrub` / 完整性扫描反向解析。位图实体名固定为
//! `<item-id>.dds`，读写前统一经 [`canonical_item_dds`] 校验文件名与仓库边界，
//! 只判名字时用 [`is_item_dds_file`]，拼相对路径时用 [`relative_item_dds`]。

use std::path::{Path, PathBuf};

pub(crate) const BOARD_DIRECTORY: &str = "boards";

pub(crate) fn board_relative_path(artwork_id: &str, board_id: i64) -> String {
    format!("artworks/{artwork_id}/{BOARD_DIRECTORY}/{board_id}")
}

/// 位图 DDS 的仓库相对路径，供 `cleanup` 入队与引用清单使用。调用方必须已经确认
/// 记录的 `file_path` 是规范名（[`is_item_dds_file`]），这里只按条目 id 拼路。
pub(crate) fn relative_item_dds(artwork_id: &str, board_id: i64, item_id: i64) -> String {
    format!(
        "{}/{item_id}.dds",
        board_relative_path(artwork_id, board_id)
    )
}

/// 位图载荷记录的 `file_path` 是否就是这个条目的规范文件名 `<item-id>.dds`。
///
/// 视图可用性、结算清理与引用清单都只认规范名：`file_path` 被外部改坏的记录不参与
/// 文件读写，留给完整性扫描报告，因此判定只此一处。
pub(crate) fn is_item_dds_file(item_id: i64, file_path: &str) -> bool {
    file_path == format!("{item_id}.dds")
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
        .and_then(|stem| stem.parse::<i64>().ok())
        .is_some_and(|id| id > 0)
}

pub(super) fn board_directory(root: &Path, artwork_id: &str, board_id: i64) -> PathBuf {
    root.join("artworks")
        .join(artwork_id)
        .join(BOARD_DIRECTORY)
        .join(board_id.to_string())
}

/// 校验位图的规范文件名 `<item-id>.dds`，解析成画板目录下的真实路径，并确认
/// 规范化后仍落在仓库根内。
///
/// 粘贴源、导出/剪贴板读取与纹理读取本来各写一份「规范文件名 + 仓库边界」检查，
/// 现在统一走这里，路径规则只有一处。`label` 只用于错误文案。
pub(super) fn canonical_item_dds(
    root: &Path,
    directory: &Path,
    item_id: i64,
    file_path: &str,
    label: &str,
) -> Result<PathBuf, String> {
    if !is_item_dds_file(item_id, file_path) {
        return Err(format!("{label}的文件名无效"));
    }
    let path = directory.join(file_path);
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("无法访问{label}：{error}"))?;
    if !canonical.starts_with(root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        return Err(format!("{label}路径超出仓库范围"));
    }
    Ok(canonical)
}
