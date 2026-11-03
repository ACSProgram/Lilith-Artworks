//! 画板内粘贴源的解析与落位。
//!
//! 前端只记录「复制了哪一条」，尺寸、变换与载荷都在这里按仓库当前状态读取；
//! 写入流程仍在 [`super::create`]，本模块只负责把复制记录变成可写的源列表。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension};

use super::super::{dds, stroke};
use super::paths::canonical_item_dds;
use super::view::{current_states, open_board_context, ItemState, StrokePayload, TextPayload};
use super::{PinBoardClipboardItem, ITEM_KIND_BITMAP, ITEM_KIND_STROKE, ITEM_KIND_TEXT};
use crate::storage;

/// 一条已解析的粘贴源：三类条目共用「变换 + 载荷」形状，写入时按 `kind` 分派。
pub(super) enum ClipboardPayload {
    Bitmap {
        dds_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    Text {
        payload: TextPayload,
        measured_width: f64,
        measured_height: f64,
    },
    Stroke {
        payload: StrokePayload,
        width: f64,
        height: f64,
        geometry: Vec<u8>,
    },
}

pub(super) struct ClipboardSource {
    pub(super) kind: i64,
    pub(super) layer: u8,
    pub(super) points: [[f64; 2]; 4],
    pub(super) uv: [[f64; 2]; 4],
    pub(super) payload: ClipboardPayload,
}

fn clipboard_item_kind(kind: &str) -> Result<i64, String> {
    match kind {
        "bitmap" => Ok(ITEM_KIND_BITMAP),
        "text" => Ok(ITEM_KIND_TEXT),
        "stroke" => Ok(ITEM_KIND_STROKE),
        other => Err(format!("未知的粘贴条目类型：{other}")),
    }
}

/// 解析全部粘贴源。任何一条不合法都直接失败——粘贴要么整组成功，要么一条也不写。
///
/// 源条目一律读**当前步骤**的真实状态：前端的复制记录只是「哪一条」，不带尺寸与
/// 变换；这样即使画板在复制后被其他窗口改过，粘进来的也是仓库里的事实。
pub(super) fn resolve_clipboard_sources(
    connection: &Connection,
    root: &Path,
    items: &[PinBoardClipboardItem],
) -> Result<Vec<ClipboardSource>, String> {
    // 每个源画板只读一次上下文与状态：同一块画板里多选粘贴是常见情形。
    let mut boards: HashMap<i64, HashMap<i64, ItemState>> = HashMap::new();
    for item in items {
        if boards.contains_key(&item.source_board_id) {
            continue;
        }
        // 源画板不存在或已在回收站时在这里拒绝。
        let context = open_board_context(connection, root, item.source_board_id)?;
        let states = current_states(connection, item.source_board_id, context.now_step)?
            .into_iter()
            .collect::<HashMap<_, _>>();
        boards.insert(item.source_board_id, states);
    }

    let mut sources = Vec::with_capacity(items.len());
    for item in items {
        let expected_kind = clipboard_item_kind(&item.kind)?;
        let state = boards
            .get(&item.source_board_id)
            .and_then(|states| states.get(&item.source_item_id))
            .ok_or_else(|| format!("源条目 {} 不存在", item.source_item_id))?;
        if state.deleted {
            return Err(format!("源条目 {} 已删除", item.source_item_id));
        }
        if state.kind != expected_kind {
            return Err(format!(
                "源条目 {} 的类型与复制记录不一致",
                item.source_item_id
            ));
        }
        let payload = match state.kind {
            ITEM_KIND_BITMAP => {
                let source =
                    source_dds_path(connection, root, item.source_board_id, item.source_item_id)?;
                let bytes = dds::read_limited(&source, dds::MAX_DDS_BYTES, "源 DDS 图片")?;
                let (dds_bytes, width, height) = dds::prepare_dds_import(&bytes)?;
                if dds::checked_pixel_count(width, height)
                    .map_or(true, |pixels| pixels > dds::MAX_DDS_SOURCE_PIXELS)
                {
                    return Err("粘贴图片像素数超过限制".into());
                }
                ClipboardPayload::Bitmap {
                    dds_bytes,
                    width,
                    height,
                }
            }
            ITEM_KIND_TEXT => ClipboardPayload::Text {
                payload: state
                    .text
                    .clone()
                    .ok_or_else(|| format!("文字条目 {} 缺少载荷", item.source_item_id))?,
                measured_width: state.width,
                measured_height: state.height,
            },
            ITEM_KIND_STROKE => {
                let payload = state
                    .stroke
                    .clone()
                    .ok_or_else(|| format!("笔迹条目 {} 缺少载荷", item.source_item_id))?;
                let geometry: Option<Vec<u8>> = connection
                    .query_row(
                        "SELECT geometry FROM pin_board_strokes WHERE item_id = ?1",
                        [item.source_item_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage::database_error)?;
                let geometry = geometry
                    .ok_or_else(|| format!("笔迹条目 {} 缺少几何数据", item.source_item_id))?;
                stroke::validate_geometry(&geometry).map_err(|error| {
                    format!("笔迹条目 {} 的几何无效：{error}", item.source_item_id)
                })?;
                ClipboardPayload::Stroke {
                    payload,
                    width: state.width,
                    height: state.height,
                    geometry,
                }
            }
            other => return Err(format!("未知画板条目类型：{other}")),
        };
        sources.push(ClipboardSource {
            kind: state.kind,
            layer: state.layer,
            points: state.transform.points,
            uv: state.transform.uv,
            payload,
        });
    }
    Ok(sources)
}

/// 把整组粘贴源平移，使外框中心落在放置点上。
///
/// **保持相对位置**而不是围绕放置点重排：复制粘贴的首要预期是「再来一份、相互位置
/// 不变」；混合选择（图片 + 文字标注）若重排，会打散用户摆好的对齐关系。
pub(super) fn translate_to_placement(
    sources: &mut [ClipboardSource],
    center_x: f64,
    center_y: f64,
) {
    if sources.is_empty() {
        return;
    }
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for source in sources.iter() {
        for [x, y] in source.points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return;
    }
    let delta_x = center_x - (min_x + max_x) / 2.0;
    let delta_y = center_y - (min_y + max_y) / 2.0;
    for source in sources.iter_mut() {
        source.points = source.points.map(|[x, y]| [x + delta_x, y + delta_y]);
    }
}

fn source_dds_path(
    connection: &Connection,
    root: &Path,
    board_id: i64,
    item_id: i64,
) -> Result<PathBuf, String> {
    let context = open_board_context(connection, root, board_id)?;
    let file_path: Option<String> = connection
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
    canonical_item_dds(root, &context.directory, item_id, &file_path, "源 DDS 图片")
}
