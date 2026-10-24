//! 画板上下文、条目状态与视图装载。
//!
//! [`open_board_context`] 解析画板目录、修订号与当前步骤；[`current_states`] 按 step
//! 取每个条目的最新变换与载荷；[`load_view`] 组装前端 DTO。保存与新增条目共用同一套
//! 读侧口径（[`validate_editable`] / [`effective_layer`] / [`same_state`]）。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::super::stroke;
use super::boards::require_active_board;
use super::paths::{board_directory, is_item_dds_file};
use super::{
    EditablePinBoardItem, BOARD_REVISION_CONFLICT, ITEM_KIND_BITMAP, ITEM_KIND_STROKE,
    ITEM_KIND_TEXT, MAX_TEXT_CHARS, MAX_TEXT_MEASURE_DIMENSION,
};
use crate::storage;

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
    /// 笔迹只带标量元数据；几何 BLOB 走 `read_pin_board_strokes` 的二进制通道，
    /// 不进 `load_pin_board` 的 JSON。
    #[serde(rename_all = "camelCase")]
    Stroke {
        tool: u8,
        color: String,
        stroke_width: f64,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardItem {
    pub(super) board_id: i64,
    pub(super) item_id: i64,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) order: u64,
    pub(super) layer: u8,
    pub(super) deleted: bool,
    pub(super) points: [[f64; 2]; 4],
    pub(super) uv: [[f64; 2]; 4],
    pub(super) available: bool,
    #[serde(flatten)]
    pub(super) payload: PinBoardItemPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardView {
    pub(super) board_id: i64,
    pub(super) name: String,
    pub(super) items: Vec<PinBoardItem>,
    pub(super) missing_textures: usize,
    pub(super) revision: String,
    pub(super) now_step: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ImageTransform {
    pub(crate) points: [[f64; 2]; 4],
    pub(crate) uv: [[f64; 2]; 4],
}

#[derive(Debug, Clone)]
pub(super) struct TextPayload {
    pub(super) text: String,
    pub(super) font_family: String,
    pub(super) font_size: f64,
    pub(super) color: String,
    pub(super) align: String,
}

/// 文字载荷的统一校验：新建与保存更新共用同一份口径，返回归一化后的颜色。
///
/// 字体族不在调用方的控制范围内：新建时固定写入 `TEXT_FONT_FAMILY`，保存更新
/// 也不允许改变它。
pub(super) fn validate_text_payload(
    text: &str,
    font_size: f64,
    color: &str,
    align: &str,
    measured_width: f64,
    measured_height: f64,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("文字内容不能为空".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("文字内容不能超过 {MAX_TEXT_CHARS} 个字符"));
    }
    if !font_size.is_finite() || font_size <= 0.0 || font_size > 1000.0 {
        return Err("文字字号无效".into());
    }
    let color = stroke::normalise_hex_color(color, "文字")?;
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
    Ok(color)
}

#[derive(Debug, Clone)]
pub(super) struct StrokePayload {
    pub(super) tool: u8,
    pub(super) color: String,
    pub(super) stroke_width: f64,
}

#[derive(Debug, Clone)]
pub(super) struct ItemState {
    pub(super) kind: i64,
    pub(super) file_path: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) deleted: bool,
    pub(super) layer: u8,
    pub(super) sort_order: u64,
    pub(super) transform: ImageTransform,
    pub(super) text: Option<TextPayload>,
    pub(super) stroke: Option<StrokePayload>,
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

/// 修订号校验：保存、结算与新增条目都在各自事务里先对一次，冲突文案只此一处。
pub(super) fn ensure_revision(
    context: &BoardContext,
    expected_revision: &str,
) -> Result<(), String> {
    if context.revision != expected_revision {
        return Err(BOARD_REVISION_CONFLICT.into());
    }
    Ok(())
}

/// `current_states` 的一行原始数据：三张载荷子表按 kind 各自填充，只有对应 kind
/// 的列会被读取。列多且大多可空，收成结构体比 21 元组更好核对。
struct StateRow {
    item_id: i64,
    kind: i64,
    file_path: Option<String>,
    bitmap_width: Option<f64>,
    bitmap_height: Option<f64>,
    text_width: Option<f64>,
    text_height: Option<f64>,
    stroke_box_width: Option<f64>,
    stroke_box_height: Option<f64>,
    deleted: bool,
    layer: u8,
    sort_order: u64,
    transform_json: String,
    text: Option<String>,
    font_family: Option<String>,
    font_size: Option<f64>,
    text_color: Option<String>,
    align: Option<String>,
    stroke_tool: Option<i64>,
    stroke_color: Option<String>,
    stroke_width: Option<f64>,
}

pub(super) fn current_states(
    connection: &Connection,
    board_id: i64,
    now_step: u64,
) -> Result<Vec<(i64, ItemState)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT i.id, i.kind, b.file_path,
                    CAST(b.width AS REAL), CAST(b.height AS REAL),
                    t.measured_width, t.measured_height,
                    CAST(s.width AS REAL), CAST(s.height AS REAL),
                    h.deleted, h.layer, h.sort_order, h.transform_json,
                    t.text, t.font_family, t.font_size, t.color, t.align,
                    s.tool, s.color, s.stroke_width
               FROM pin_board_items i
               LEFT JOIN pin_board_bitmaps b ON b.item_id = i.id
               LEFT JOIN pin_board_texts t ON t.item_id = i.id
               LEFT JOIN pin_board_strokes s ON s.item_id = i.id
               JOIN pin_board_history h
                 ON h.board_id = i.board_id AND h.item_id = i.id
                AND h.step = (SELECT MAX(step) FROM pin_board_history
                              WHERE board_id = i.board_id AND item_id = i.id AND step <= ?2)
              WHERE i.board_id = ?1",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map(params![board_id, now_step as i64], |row| {
            Ok(StateRow {
                item_id: row.get(0)?,
                kind: row.get(1)?,
                file_path: row.get(2)?,
                bitmap_width: row.get(3)?,
                bitmap_height: row.get(4)?,
                text_width: row.get(5)?,
                text_height: row.get(6)?,
                stroke_box_width: row.get(7)?,
                stroke_box_height: row.get(8)?,
                deleted: row.get::<_, i64>(9)? != 0,
                layer: row.get::<_, i64>(10)? as u8,
                sort_order: row.get::<_, i64>(11)? as u64,
                transform_json: row.get(12)?,
                text: row.get(13)?,
                font_family: row.get(14)?,
                font_size: row.get(15)?,
                text_color: row.get(16)?,
                align: row.get(17)?,
                stroke_tool: row.get(18)?,
                stroke_color: row.get(19)?,
                stroke_width: row.get(20)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    rows.into_iter()
        .map(|row| {
            let StateRow {
                item_id,
                kind,
                file_path,
                transform_json,
                ..
            } = &row;
            let transform: ImageTransform = serde_json::from_str(transform_json)
                .map_err(|error| format!("画板条目变换数据无效：{error}"))?;
            let (width, height, text, stroke) = match *kind {
                ITEM_KIND_BITMAP => {
                    let width = row.bitmap_width.ok_or("位图条目缺少尺寸记录")?;
                    let height = row.bitmap_height.ok_or("位图条目缺少尺寸记录")?;
                    (width, height, None, None)
                }
                ITEM_KIND_TEXT => {
                    let width = row.text_width.ok_or("文字条目缺少度量记录")?;
                    let height = row.text_height.ok_or("文字条目缺少度量记录")?;
                    let payload = TextPayload {
                        text: row.text.clone().ok_or("文字条目缺少内容")?,
                        font_family: row.font_family.clone().ok_or("文字条目缺少字体")?,
                        font_size: row.font_size.ok_or("文字条目缺少字号")?,
                        color: row.text_color.clone().ok_or("文字条目缺少颜色")?,
                        align: row.align.clone().ok_or("文字条目缺少对齐方式")?,
                    };
                    (width, height, Some(payload), None)
                }
                ITEM_KIND_STROKE => {
                    let width = row.stroke_box_width.ok_or("笔迹条目缺少包围盒宽度")?;
                    let height = row.stroke_box_height.ok_or("笔迹条目缺少包围盒高度")?;
                    let tool = row
                        .stroke_tool
                        .and_then(|value| u8::try_from(value).ok())
                        .ok_or("笔迹条目缺少工具记录")?;
                    let payload = StrokePayload {
                        tool,
                        color: row.stroke_color.clone().ok_or("笔迹条目缺少颜色")?,
                        stroke_width: row.stroke_width.ok_or("笔迹条目缺少线宽")?,
                    };
                    (width, height, None, Some(payload))
                }
                other => return Err(format!("未知画板条目类型：{other}")),
            };
            if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                return Err(format!("画板条目 {item_id} 的尺寸无效"));
            }
            Ok((
                *item_id,
                ItemState {
                    kind: *kind,
                    file_path: file_path.clone(),
                    width,
                    height,
                    deleted: row.deleted,
                    layer: row.layer,
                    sort_order: row.sort_order,
                    transform,
                    text,
                    stroke,
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()
}

/// 位图条目的 DDS 是否可用。文字与笔迹条目没有实体文件，恒为可用。
fn item_available(directory: &Path, item_id: i64, state: &ItemState) -> bool {
    if state.kind != ITEM_KIND_BITMAP {
        return true;
    }
    let Some(file_path) = state.file_path.as_deref() else {
        return false;
    };
    if !is_item_dds_file(item_id, file_path) {
        return false;
    }
    directory.join(file_path).is_file()
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
        let available = item_available(&context.directory, item_id, &state);
        let payload = match state.kind {
            ITEM_KIND_TEXT => {
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
            }
            ITEM_KIND_STROKE => {
                let stroke = state
                    .stroke
                    .ok_or_else(|| format!("笔迹条目 {item_id} 缺少载荷"))?;
                PinBoardItemPayload::Stroke {
                    tool: stroke.tool,
                    color: stroke.color,
                    stroke_width: stroke.stroke_width,
                }
            }
            ITEM_KIND_BITMAP => PinBoardItemPayload::Bitmap {},
            other => return Err(format!("未知画板条目类型：{other}")),
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

pub(super) fn validate_editable(item: &EditablePinBoardItem, kind: i64) -> Result<(), String> {
    // 位图与文字共用 0–2 三层；3 只留给笔迹。笔迹的图层由 `effective_layer` 在
    // 写入前强制改写，调用方传 0–2 不会被采纳，传 3（其固化位）也合法；>= 4 拒绝。
    let layer_limit = if kind == ITEM_KIND_STROKE {
        stroke::STROKE_LAYER
    } else {
        stroke::STROKE_LAYER - 1
    };
    if item.layer > layer_limit {
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

/// 条目的实际写入图层。笔迹固定进标注层，前端传来的值一律忽略——这是防御性
/// 契约，避免渲染顺序被状态泄漏或旧版本前端带偏。
pub(super) fn effective_layer(kind: i64, requested: u8) -> u8 {
    if kind == ITEM_KIND_STROKE {
        stroke::STROKE_LAYER
    } else {
        requested
    }
}

pub(super) fn same_state(left: &ItemState, right: &EditablePinBoardItem) -> bool {
    left.deleted == right.deleted
        && left.layer == effective_layer(left.kind, right.layer)
        && left.transform.points == right.points
        && left.transform.uv == right.uv
}

#[cfg(test)]
mod tests {
    use super::super::{EditablePinBoardItem, TEXT_FONT_FAMILY};
    use super::*;
    use crate::pin_board::stroke;

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
    fn serializes_stroke_items_with_kind_and_flat_metadata() {
        let points = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let value = serde_json::to_value(&PinBoardItem {
            board_id: 1,
            item_id: 5,
            width: 10.0,
            height: 10.0,
            order: 0,
            layer: stroke::STROKE_LAYER,
            deleted: false,
            points,
            uv: points,
            available: true,
            payload: PinBoardItemPayload::Stroke {
                tool: stroke::TOOL_PEN,
                color: "#AABBCC".into(),
                stroke_width: 2.0,
            },
        })
        .unwrap();
        assert_eq!(value["kind"], "stroke");
        assert_eq!(value["tool"], 0);
        assert_eq!(value["color"], "#AABBCC");
        assert_eq!(value["strokeWidth"], 2.0);
        assert_eq!(value["layer"], 3);
        // 几何不进视图 JSON：前端走 `read_pin_board_strokes` 的二进制通道。
        assert!(value.get("geometry").is_none());
    }
}
