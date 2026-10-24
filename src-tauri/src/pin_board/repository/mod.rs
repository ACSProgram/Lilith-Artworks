//! 素材板 SQLite 持久化与画板文件布局。
//!
//! 存储布局（schema v6 起，见 `library/schema.rs`）：
//! - `pin_boards`：按 Artwork 平铺一层画板，`deleted_at` 为回收站软删除；
//! - `pin_board_items`：与 kind 无关的公共条目（0 位图 / 1 文字 / 2 笔迹）；
//! - `pin_board_bitmaps`：位图载荷，DDS 实体存于
//!   `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`；
//! - `pin_board_texts`：文字载荷，不落实体文件；
//! - `pin_board_strokes`：笔迹载荷（工具、颜色、线宽、包围盒、几何 BLOB），不落实体文件；
//! - `pin_board_history`：每个条目按 step 的变换历史（撤销/恢复持久化）。笔迹条目的
//!   `layer` 恒为 3（标注层），写入时强制改写，不接受前端传入的值。
//!
//! 按职责拆成子模块：
//! - [`paths`]：画板目录与位图 DDS 路径的拼装、反向解析与仓库边界检查；
//! - [`boards`]：画板列表的增删改查、回收站、排序与修订号推进；
//! - [`view`]：画板上下文、条目状态与视图装载；
//! - [`save`]：保存与结算；
//! - [`create`]：粘贴、导入、文字与笔迹的写入流程；
//! - [`bitmap_io`]：位图 PNG 导出、剪贴板读取与纹理读取。
//!
//! 本模块只通过 `storage.rs` 打开连接，不读取 ChunkFile；回收站永久删除经
//! `cleanup.rs` 队列清理 DDS 目录。

use serde::{Deserialize, Serialize};

mod bitmap_io;
mod boards;
mod create;
mod paths;
mod save;
mod view;

#[cfg(test)]
mod fixtures;

pub(crate) use bitmap_io::{export_images, read_image_png, read_texture};
pub(crate) use boards::{
    create_board, delete_board_permanently, empty_trash, list_boards, list_trash, rename_board,
    reorder_boards, restore_board, trash_board,
};
pub(crate) use create::{
    create_stroke, create_text, import_clipboard_image, import_images, paste_items, read_strokes,
};
#[cfg(feature = "headless")]
pub(crate) use create::{create_strokes, StrokeInsert};
pub(crate) use paths::{
    board_relative_path, is_dds_name, is_item_dds_file, relative_item_dds, BOARD_DIRECTORY,
};
pub(crate) use save::{finalize_board, save_board};
pub(crate) use view::{load_view, ImageTransform, PinBoardView};
#[cfg(feature = "headless")]
pub(crate) use view::{open_board_context, BoardContext};

/// 单个 Artwork 的画板数量上限（规划确认的规模假设）。
pub(crate) const MAX_BOARDS_PER_ARTWORK: usize = 64;
/// 文字条目的固定字体族。创建时把度量结果持久化，跨机器缺字体只影响显示度量。
pub(crate) const TEXT_FONT_FAMILY: &str = "\"Microsoft YaHei UI\", \"Segoe UI\", sans-serif";
pub(crate) const MAX_TEXT_CHARS: usize = 5000;
/// 文字条目的逻辑度量上限。超过该值的段落无法在既有纹理上限内清晰显示。
pub(crate) const MAX_TEXT_MEASURE_DIMENSION: f64 = 8192.0;

const ITEM_KIND_BITMAP: i64 = 0;
const ITEM_KIND_TEXT: i64 = 1;
const ITEM_KIND_STROKE: i64 = 2;
/// 条目四角的完整 UV：新条目的纹理铺满整个四边形。
const FULL_UV: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
/// 保存、结算与新增条目共用的修订冲突文案。
const BOARD_REVISION_CONFLICT: &str = "画板已被其他窗口或程序修改，请重新打开后再编辑";

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
    /// 条目记录数，覆盖全部 kind（位图 / 文字 / 笔迹）。
    item_count: i64,
    created_ms: i64,
    updated_ms: i64,
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
    /// 文字条目的可选载荷。只有文字内容/样式或度量发生变化时才由前端携带；
    /// 非文字条目带载荷会被拒绝，避免 DTO 形状失去约束。
    #[serde(default)]
    text: Option<EditableTextPayload>,
}

/// 保存时随条目提交的文字载荷。字体族固定由服务端写入，不在保存契约里。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditableTextPayload {
    text: String,
    font_size: f64,
    color: String,
    align: String,
    measured_width: f64,
    measured_height: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePinBoardResult {
    saved: bool,
    revision: String,
    now_step: u64,
}

/// 画板内复制的一条载荷：**只记来源**，显示尺寸、变换与内容都由本层读源条目现算。
///
/// 前端曾把尺寸与 uv 一起带上来，但那份值可能来自尚未落库的编辑快照；混选（位图 +
/// 文字 + 笔迹）又必须原子写入，所以统一改成「读源条目」这一个事实来源。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardClipboardItem {
    /// `bitmap` / `text` / `stroke`，与 `ITEM_KIND_*` 对应。
    kind: String,
    source_board_id: i64,
    source_item_id: i64,
}
