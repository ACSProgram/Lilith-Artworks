//! 粘贴、导入、文字与笔迹的写入流程。
//!
//! 三类条目共用 [`begin_item_mutation`] 预留步骤与 id、[`record_initial_state`] 写
//! 初始历史、[`finish_item_mutation`] 收尾（推进 revision → 提交 → 让已写出的 DDS
//! 生效）。载荷按 kind 分派到各自的子表，落位统一走 `arrange::imported_points`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use super::super::arrange::{imported_points, MAX_GAP_RATIO};
use super::super::contracts::{MAX_BOARD_ITEMS, MAX_CLIPBOARD_IMAGE_BYTES, MAX_IMPORT_FILES};
use super::super::dds;
use super::super::stroke;
use super::super::transfer::TRANSFER_CANCELLED;
use super::boards::{bump_revision, discard_future_history, require_active_board};
use super::load_view;
use super::paths::canonical_item_dds;
use super::view::{
    current_states, effective_layer, ensure_revision, open_board_context, validate_editable,
    ItemState, StrokePayload, TextPayload,
};
use super::{
    EditablePinBoardItem, ImageTransform, PinBoardClipboardItem, PinBoardView, FULL_UV,
    ITEM_KIND_BITMAP, ITEM_KIND_STROKE, ITEM_KIND_TEXT, MAX_TEXT_CHARS, MAX_TEXT_MEASURE_DIMENSION,
    TEXT_FONT_FAMILY,
};
use crate::storage;

/// 新条目统一落入中层（0–2 中的中间一层）。
const DEFAULT_ITEM_LAYER: u8 = 1;

/// 单条载荷写入文字子表。粘贴与新建文字共用同一份 INSERT 口径。
#[allow(clippy::too_many_arguments)]
fn insert_text_payload(
    transaction: &rusqlite::Transaction<'_>,
    item_id: i64,
    text: &str,
    font_family: &str,
    font_size: f64,
    color: &str,
    align: &str,
    measured_width: f64,
    measured_height: f64,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO pin_board_texts
               (item_id, text, font_family, font_size, color, align,
                measured_width, measured_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item_id,
                text,
                font_family,
                font_size,
                color,
                align,
                measured_width,
                measured_height
            ],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 单条载荷写入笔迹子表。粘贴与批量新建笔迹共用同一份 INSERT 口径。
#[allow(clippy::too_many_arguments)]
fn insert_stroke_payload(
    transaction: &rusqlite::Transaction<'_>,
    item_id: i64,
    tool: u8,
    color: &str,
    stroke_width: f64,
    width: f64,
    height: f64,
    geometry: &[u8],
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO pin_board_strokes
               (item_id, tool, color, stroke_width, width, height, geometry)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item_id,
                i64::from(tool),
                color,
                stroke_width,
                width,
                height,
                geometry
            ],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 把单个条目按其自然尺寸摆到放置点上。批量入口 [`imported_points`] 对单项输入
/// 恒定返回一项（退化输入返回中心的零面积四边形），因此这里不必再报错。
fn centred_points(width: f64, height: f64, center_x: f64, center_y: f64) -> [[f64; 2]; 4] {
    imported_points(&[(width, height)], center_x, center_y, 0.0)
        .first()
        .copied()
        .unwrap_or([[center_x, center_y]; 4])
}

/// 新增条目事务的统一收尾：推进 revision、提交事务、让本次写出的 DDS 正式生效。
/// 调用方在返回后读取视图——视图必须反映已提交的数据。
fn finish_item_mutation(
    transaction: rusqlite::Transaction<'_>,
    board_id: i64,
    mut created_files: dds::CreatedFiles,
) -> Result<(), String> {
    bump_revision(&transaction, board_id)?;
    transaction.commit().map_err(storage::database_error)?;
    created_files.commit();
    Ok(())
}

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
    kinds: &[i64],
) -> Result<MutationSetup, String> {
    let context = open_board_context(transaction, root, board_id)?;
    ensure_revision(&context, expected_revision)?;
    let count = kinds.len();
    let item_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
            [board_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)?;
    if item_count as usize + count > MAX_BOARD_ITEMS {
        return Err("画板条目数量超过限制".into());
    }
    let next_step = context
        .now_step
        .checked_add(1)
        .ok_or("画板历史步骤已达到上限")?;
    // 新步骤会覆盖未来：与 Client 行为一致，写入新步骤后 redo 记录作废。
    discard_future_history(transaction, board_id, context.now_step)?;
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
    // 混合粘贴时每个条目的 kind 不同，位图才需要预建载荷占位行。
    for kind in kinds {
        transaction
            .execute(
                "INSERT INTO pin_board_items (board_id, kind, created_ms)
                 VALUES (?1, ?2, ?3)",
                params![board_id, kind, now],
            )
            .map_err(storage::database_error)?;
        let item_id = transaction.last_insert_rowid();
        if *kind == ITEM_KIND_BITMAP {
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
    kind: i64,
    editable: &EditablePinBoardItem,
) -> Result<(), String> {
    validate_editable(editable, kind)?;
    let layer = effective_layer(kind, editable.layer) as i64;
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
            params![board_id, item_id, layer, order as i64, transform],
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
                layer,
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

/// 阵列间距比例校验：上限与设置项、排列命令共用 `arrange::MAX_GAP_RATIO`。
fn arrange_gap_ratio_is_valid(ratio: f64) -> bool {
    ratio.is_finite() && (0.0..=MAX_GAP_RATIO).contains(&ratio)
}

/// 一条已解析的粘贴源：三类条目共用「变换 + 载荷」形状，写入时按 `kind` 分派。
enum ClipboardPayload {
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

struct ClipboardSource {
    kind: i64,
    layer: u8,
    points: [[f64; 2]; 4],
    uv: [[f64; 2]; 4],
    payload: ClipboardPayload,
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
fn resolve_clipboard_sources(
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
fn translate_to_placement(sources: &mut [ClipboardSource], center_x: f64, center_y: f64) {
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

/// 画板内粘贴：位图、文字与笔迹走**一条命令、一次事务、一次 revision**。
///
/// 拆成多条命令时，第二条失败会在画板上留下半份副本；混合选择必须原子。
pub(crate) fn paste_items(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    items: &[PinBoardClipboardItem],
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if items.is_empty() {
        return Err("没有可粘贴的条目".into());
    }
    // 与导入共用同一条数量上限：一次粘贴不该比一次导入更宽松。
    if items.len() > MAX_IMPORT_FILES {
        return Err("粘贴条目数量超过限制".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() {
        return Err("粘贴位置无效".into());
    }

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let mut sources = resolve_clipboard_sources(&transaction, root, items)?;
    translate_to_placement(&mut sources, center_x, center_y);
    let kinds = sources.iter().map(|source| source.kind).collect::<Vec<_>>();
    let setup = begin_item_mutation(&transaction, root, board_id, expected_revision, &kinds)?;

    let mut created_files = dds::CreatedFiles::default();
    let mut item_ids = Vec::with_capacity(sources.len());
    for (index, (item_id, source)) in setup.item_ids.iter().zip(sources).enumerate() {
        let item_id = *item_id;
        match source.payload {
            ClipboardPayload::Bitmap {
                dds_bytes,
                width,
                height,
            } => {
                let destination = setup.board_directory.join(format!("{item_id}.dds"));
                dds::persist_dds_file(&destination, &dds_bytes, "粘贴 DDS 图片")?;
                created_files.push(destination);
                finalize_bitmap_row(&transaction, item_id, width, height)?;
            }
            ClipboardPayload::Text {
                payload,
                measured_width,
                measured_height,
            } => {
                insert_text_payload(
                    &transaction,
                    item_id,
                    &payload.text,
                    &payload.font_family,
                    payload.font_size,
                    &payload.color,
                    &payload.align,
                    measured_width,
                    measured_height,
                )?;
            }
            ClipboardPayload::Stroke {
                payload,
                width,
                height,
                geometry,
            } => {
                insert_stroke_payload(
                    &transaction,
                    item_id,
                    payload.tool,
                    &payload.color,
                    payload.stroke_width,
                    width,
                    height,
                    &geometry,
                )?;
            }
        }
        // 图层沿用源条目；笔迹的 3 由 `effective_layer` 兜住，前端传什么都改不了。
        let editable = EditablePinBoardItem {
            item_id,
            order: index as u64,
            layer: source.layer,
            deleted: false,
            points: source.points,
            uv: source.uv,
        };
        record_initial_state(
            &transaction,
            board_id,
            item_id,
            setup.next_step,
            index as u64,
            source.kind,
            &editable,
        )?;
        item_ids.push(item_id);
    }
    finish_item_mutation(transaction, board_id, created_files)?;
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
    gap_ratio: f64,
    expected_revision: &str,
    mut on_progress: impl FnMut(usize, usize),
    should_cancel: impl Fn() -> bool,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if paths.is_empty() || paths.len() > MAX_IMPORT_FILES {
        return Err("导入文件数量无效".into());
    }
    if !center_x.is_finite() || !center_y.is_finite() || !arrange_gap_ratio_is_valid(gap_ratio) {
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
    let kinds = vec![ITEM_KIND_BITMAP; total];
    let setup = begin_item_mutation(&transaction, root, board_id, expected_revision, &kinds)?;
    let mut created_files = dds::CreatedFiles::default();
    let mut imported: Vec<(i64, u32, u32)> = Vec::with_capacity(total);
    for (index, source) in paths.iter().enumerate() {
        // 取消在条目边界生效：整笔事务回滚、已写出的 DDS 由 `CreatedFiles` 一起删掉，
        // 画板上不会留下半批条目——导入要么全成、要么什么都没发生。
        if should_cancel() {
            return Err(TRANSFER_CANCELLED.into());
        }
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
    let points = imported_points(&sizes, center_x, center_y, gap_ratio);
    let mut item_ids = Vec::with_capacity(imported.len());
    for (index, ((item_id, width, height), points)) in imported.into_iter().zip(points).enumerate()
    {
        let editable = EditablePinBoardItem {
            item_id,
            order: index as u64,
            layer: DEFAULT_ITEM_LAYER,
            deleted: false,
            points,
            uv: FULL_UV,
        };
        record_initial_state(
            &transaction,
            board_id,
            item_id,
            setup.next_step,
            index as u64,
            ITEM_KIND_BITMAP,
            &editable,
        )?;
        finalize_bitmap_row(&transaction, item_id, width, height)?;
        item_ids.push(item_id);
    }
    finish_item_mutation(transaction, board_id, created_files)?;
    let view = load_view(connection, root, board_id)?;
    Ok((view, item_ids))
}

/// 导入剪贴板位图（拖入导入的字节也走这条路径）。
pub(crate) fn import_clipboard_image(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    bytes: &[u8],
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if bytes.is_empty() || bytes.len() > MAX_CLIPBOARD_IMAGE_BYTES {
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
        &[ITEM_KIND_BITMAP],
    )?;
    let item_id = setup.item_ids[0];
    let points = centred_points(width as f64, height as f64, center_x, center_y);
    let destination = setup.board_directory.join(format!("{item_id}.dds"));
    let mut created_files = dds::CreatedFiles::default();
    dds::persist_dds_file(&destination, &dds_bytes, "剪贴板 DDS 图片")?;
    created_files.push(destination);
    let editable = EditablePinBoardItem {
        item_id,
        order: 0,
        layer: DEFAULT_ITEM_LAYER,
        deleted: false,
        points,
        uv: FULL_UV,
    };
    record_initial_state(
        &transaction,
        board_id,
        item_id,
        setup.next_step,
        0,
        ITEM_KIND_BITMAP,
        &editable,
    )?;
    finalize_bitmap_row(&transaction, item_id, width, height)?;
    finish_item_mutation(transaction, board_id, created_files)?;
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
        &[ITEM_KIND_TEXT],
    )?;
    let item_id = setup.item_ids[0];
    insert_text_payload(
        &transaction,
        item_id,
        text,
        TEXT_FONT_FAMILY,
        font_size,
        &color,
        align,
        measured_width,
        measured_height,
    )?;
    let points = centred_points(measured_width, measured_height, center_x, center_y);
    let editable = EditablePinBoardItem {
        item_id,
        order: 0,
        layer: DEFAULT_ITEM_LAYER,
        deleted: false,
        points,
        uv: FULL_UV,
    };
    record_initial_state(
        &transaction,
        board_id,
        item_id,
        setup.next_step,
        0,
        ITEM_KIND_TEXT,
        &editable,
    )?;
    finish_item_mutation(transaction, board_id, dds::CreatedFiles::default())?;
    let view = load_view(connection, root, board_id)?;
    Ok((view, vec![item_id]))
}

fn normalise_color(color: &str) -> Result<String, String> {
    stroke::normalise_hex_color(color, "文字")
}

/// 一条待写入的笔迹。`create_stroke`（GUI 的单笔创建）与 `create_strokes`（压力测试
/// 的批量创建）共用同一份校验与落库逻辑，避免两条写入路径各自演化。
pub(crate) struct StrokeInsert<'a> {
    pub(crate) tool: u8,
    pub(crate) color: &'a str,
    pub(crate) stroke_width: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) geometry: &'a [u8],
    pub(crate) center_x: f64,
    pub(crate) center_y: f64,
}

/// 新增笔迹条目：几何 BLOB 由前端编码，原生端只校验结构与声明尺寸，不解释
/// 具体形状。与文字一样只落数据库，不生成图片，也不占 DDS 文件。
///
/// `layer` 不由调用方决定：笔迹固定写入标注层（`stroke::STROKE_LAYER`）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_stroke(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    tool: u8,
    color: &str,
    stroke_width: f64,
    width: f64,
    height: f64,
    geometry: &[u8],
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    create_strokes(
        connection,
        root,
        board_id,
        &[StrokeInsert {
            tool,
            color,
            stroke_width,
            width,
            height,
            geometry,
            center_x,
            center_y,
        }],
        expected_revision,
    )
}

/// 批量新增笔迹：一次事务、一次 revision 推进、末尾只读一次画板视图。
///
/// 逐条 `create_stroke` 对 GUI 没有差别（一次手势只有一笔），但几百上千笔时每条都
/// 重新 `load_view` 会把写入退化成 O(n²)——压力测试要量的正是「真实写入成本」，因此
/// 批量入口必须存在。校验全部前置：任何一条不合法都一条也不写。
pub(crate) fn create_strokes(
    connection: &mut Connection,
    root: &Path,
    board_id: i64,
    strokes: &[StrokeInsert<'_>],
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), String> {
    if strokes.is_empty() {
        return Err("没有要写入的笔迹".into());
    }
    let mut headers = Vec::with_capacity(strokes.len());
    let mut colors = Vec::with_capacity(strokes.len());
    for stroke in strokes {
        let header = stroke::validate_geometry(stroke.geometry)?;
        if header.tool != stroke.tool {
            // 工具同时出现在列与 BLOB 头部；不一致说明编码端有 bug，宁可拒绝也不写进
            // 一条自相矛盾的记录（列用于检索，头部用于渲染）。
            return Err("笔迹工具与几何头部不一致".into());
        }
        stroke::validate_extent(stroke.width, stroke.height, stroke.stroke_width)?;
        if !stroke.center_x.is_finite() || !stroke.center_y.is_finite() {
            return Err("笔迹位置无效".into());
        }
        headers.push(header);
        colors.push(stroke::normalise_hex_color(stroke.color, "笔迹")?);
    }

    let transaction = connection.transaction().map_err(storage::database_error)?;
    let kinds = vec![ITEM_KIND_STROKE; strokes.len()];
    let setup = begin_item_mutation(&transaction, root, board_id, expected_revision, &kinds)?;
    for (index, stroke) in strokes.iter().enumerate() {
        let item_id = setup.item_ids[index];
        insert_stroke_payload(
            &transaction,
            item_id,
            headers[index].tool,
            &colors[index],
            stroke.stroke_width,
            stroke.width,
            stroke.height,
            stroke.geometry,
        )?;
        let points = centred_points(
            stroke.width,
            stroke.height,
            stroke.center_x,
            stroke.center_y,
        );
        let editable = EditablePinBoardItem {
            item_id,
            order: index as u64,
            layer: stroke::STROKE_LAYER,
            deleted: false,
            points,
            uv: FULL_UV,
        };
        record_initial_state(
            &transaction,
            board_id,
            item_id,
            setup.next_step,
            index as u64,
            ITEM_KIND_STROKE,
            &editable,
        )?;
    }
    finish_item_mutation(transaction, board_id, dds::CreatedFiles::default())?;
    let view = load_view(connection, root, board_id)?;
    Ok((view, setup.item_ids))
}

/// 读取整板笔迹几何并打包成二进制，供渲染器一次性建缓冲（不经过 JSON）。
///
/// 包含已删除但尚未结算的条目：`load_pin_board` 的 `deleted` 才是可见性的事实
/// 来源，几何读取保持与视图同集合，前端不必为「删了又撤销」再请求一次。
/// 每条几何在打包前都按读侧口径校验，损坏记录会明确报错而不是被静默丢弃。
pub(crate) fn read_strokes(connection: &Connection, board_id: i64) -> Result<Vec<u8>, String> {
    require_active_board(connection, board_id)?;
    let mut statement = connection
        .prepare(
            "SELECT i.id, s.tool, s.color, s.stroke_width,
                    CAST(s.width AS REAL), CAST(s.height AS REAL), s.geometry
               FROM pin_board_items i
               JOIN pin_board_strokes s ON s.item_id = i.id
              WHERE i.board_id = ?1
              ORDER BY i.id",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([board_id], |row| {
            Ok(stroke::StrokeRecord {
                item_id: row.get(0)?,
                tool: row.get::<_, i64>(1)? as u8,
                color: row.get(2)?,
                stroke_width: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                geometry: row.get(6)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    for record in &rows {
        stroke::validate_geometry(&record.geometry)
            .map_err(|error| format!("笔迹条目 {} 的几何无效：{error}", record.item_id))?;
        stroke::validate_extent(record.width, record.height, record.stroke_width)
            .map_err(|error| format!("笔迹条目 {}：{error}", record.item_id))?;
    }
    stroke::encode_batch(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::contracts::ReferenceInventory;
    use crate::pin_board::repository::bitmap_io::{export_images, read_image_png, read_texture};
    use crate::pin_board::repository::boards::{create_board, trash_board};
    use crate::pin_board::repository::fixtures::{
        board_revision, clipboard_item, create_test_artwork, insert_live_bitmap, pen_geometry,
        points_of, scalar, scalar_for_board, shape_geometry, test_repository,
    };
    use crate::pin_board::repository::paths::board_directory;
    use crate::pin_board::repository::save::save_board;
    use crate::pin_board::repository::view::{current_states, load_view, PinBoardItemPayload};
    use crate::pin_board::stroke;
    use crate::pin_board::transfer::TRANSFER_CANCELLED;
    use crate::platform::path::display_path;

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
            &[display_path(&inside_source)],
            0.0,
            0.0,
            0.0,
            &revision,
            |_, _| {},
            || false,
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
            &display_path(&inside_output),
            |_, _| {},
            || false,
        )
        .unwrap_err();
        assert!(export_error.contains("作品仓库内部"), "{export_error}");
    }

    #[test]
    fn cancelling_an_import_leaves_the_board_untouched() {
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
        // 源图不存在也无所谓：取消检查在解码之前，先于一切文件读写。
        let source = directory.path().join("source.png");

        let error = import_images(
            &mut connection,
            &root,
            board.board_id,
            &[display_path(&source)],
            0.0,
            0.0,
            0.0,
            &revision,
            |_, _| {},
            || true,
        )
        .unwrap_err();
        assert_eq!(error, TRANSFER_CANCELLED);

        // 整笔回滚：条目与 DDS 都不该留下（画板目录压根没被创建）。
        let expected_directory = board_directory(&root, &artwork_id, board.board_id);
        assert!(
            !expected_directory.exists()
                || fs::read_dir(&expected_directory).unwrap().next().is_none(),
            "取消的导入不得留下 DDS"
        );
        let item_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
                [board.board_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(item_count, 0, "取消的导入不得写入条目");
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
                |_, _| {},
                || false,
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
    fn creates_stroke_items_on_the_annotation_layer() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "笔迹板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let geometry = pen_geometry(3);

        let (view, item_ids) = create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_PEN,
            "#a1b2c3",
            2.5,
            120.0,
            40.0,
            &geometry,
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
        assert_eq!((item.width, item.height), (120.0, 40.0));
        assert_eq!(item.layer, stroke::STROKE_LAYER);
        assert!(!item.deleted);
        assert!(item.available, "笔迹没有实体文件，恒为可用");
        match &item.payload {
            PinBoardItemPayload::Stroke {
                tool,
                color,
                stroke_width,
            } => {
                assert_eq!(*tool, stroke::TOOL_PEN);
                assert_eq!(color, "#A1B2C3");
                assert_eq!(*stroke_width, 2.5);
            }
            other => panic!("应返回笔迹载荷：{other:?}"),
        }
        assert_eq!(view.missing_textures, 0);
        // 笔迹只落数据库：不占位图载荷，也不进入 DDS 引用清单。
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pin_board_bitmaps"),
            0
        );
        assert!(crate::pin_board::PinBoardReferenceInventory
            .collect_targets(&connection)
            .unwrap()
            .is_empty());
        // step 0 的删除态默认节点与添加步骤都写在标注层。
        let layers: Vec<i64> = {
            let mut statement = connection
                .prepare("SELECT layer FROM pin_board_history ORDER BY step")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(layers, vec![3, 3]);
        assert_eq!(
            scalar(
                &connection,
                "SELECT COUNT(*) FROM pin_board_strokes
                 WHERE tool = 0 AND color = '#A1B2C3' AND stroke_width = 2.5"
            ),
            1
        );
    }

    #[test]
    fn reads_stroke_geometry_as_one_binary_batch() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "几何板").unwrap();
        let pen = pen_geometry(2);
        let revision = board_revision(&connection, board.board_id);
        create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_PEN,
            "#010203",
            2.5,
            100.0,
            40.0,
            &pen,
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let revision = board_revision(&connection, board.board_id);
        create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_ELLIPSE,
            "#AABBCC",
            1.0,
            30.0,
            30.0,
            &shape_geometry(stroke::TOOL_ELLIPSE, 3),
            0.0,
            0.0,
            &revision,
        )
        .unwrap();

        let bytes = read_strokes(&connection, board.board_id).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 2);
        let mut offset = 4;
        let mut geometry_lengths = Vec::new();
        for expected_tool in [stroke::TOOL_PEN, stroke::TOOL_ELLIPSE] {
            offset += 4; // itemId
            assert_eq!(bytes[offset], expected_tool);
            offset += 1;
            offset += 4; // color
            let stroke_width = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            assert!(stroke_width > 0.0);
            offset += 12; // strokeWidth 剩余 + width + height
            let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            geometry_lengths.push(length);
            offset += length;
        }
        assert_eq!(offset, bytes.len());
        assert_eq!(geometry_lengths, vec![pen.len(), 8]);

        // 空板返回「0 条」而不是空载荷，前端可以无条件解析。
        let empty = create_board(&mut connection, &artwork_id, "空板").unwrap();
        let bytes = read_strokes(&connection, empty.board_id).unwrap();
        assert_eq!(bytes, 0u32.to_le_bytes().to_vec());
    }

    #[test]
    fn stroke_items_do_not_participate_in_bitmap_paths() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "剪贴板板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (_, item_ids) = create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_LINE,
            "#FFFFFF",
            1.0,
            10.0,
            10.0,
            &shape_geometry(stroke::TOOL_LINE, 0),
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        assert_eq!(
            export_images(
                &connection,
                &root,
                board.board_id,
                &item_ids,
                export_directory.to_str().unwrap(),
                |_, _| {},
                || false,
            )
            .unwrap(),
            0
        );
        assert!(fs::read_dir(&export_directory).unwrap().next().is_none());
        let error =
            read_texture(&connection, &root, board.board_id, item_ids[0], 256, 1).unwrap_err();
        assert!(error.contains("没有 DDS"), "{error}");
        let error = read_image_png(&connection, &root, board.board_id, item_ids[0]).unwrap_err();
        assert!(error.contains("没有位图"), "{error}");
    }

    #[test]
    fn rejects_invalid_stroke_payloads() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "校验板").unwrap();
        let geometry = pen_geometry(2);
        let attempt = |connection: &mut Connection,
                       tool: u8,
                       color: &str,
                       stroke_width: f64,
                       width: f64,
                       height: f64,
                       geometry: &[u8],
                       center_x: f64| {
            let revision: String = connection
                .query_row(
                    "SELECT revision FROM pin_boards WHERE id = ?1",
                    [board.board_id],
                    |row| row.get(0),
                )
                .unwrap();
            create_stroke(
                connection,
                &root,
                board.board_id,
                tool,
                color,
                stroke_width,
                width,
                height,
                geometry,
                center_x,
                0.0,
                &revision,
            )
            .map(|_| ())
        };

        let empty = attempt(&mut connection, 0, "#000000", 1.0, 1.0, 1.0, &[], 0.0).unwrap_err();
        assert!(empty.contains("不完整"), "{empty}");
        let mismatch = attempt(
            &mut connection,
            0,
            "#000000",
            1.0,
            10.0,
            10.0,
            &shape_geometry(stroke::TOOL_RECT, 0),
            0.0,
        )
        .unwrap_err();
        assert!(mismatch.contains("不一致"), "{mismatch}");
        let bad_color =
            attempt(&mut connection, 0, "red", 1.0, 10.0, 10.0, &geometry, 0.0).unwrap_err();
        assert!(bad_color.contains("#RRGGBB"), "{bad_color}");
        let bad_width = attempt(
            &mut connection,
            0,
            "#000000",
            0.0,
            10.0,
            10.0,
            &geometry,
            0.0,
        )
        .unwrap_err();
        assert!(bad_width.contains("线宽"), "{bad_width}");
        let degenerate = attempt(
            &mut connection,
            0,
            "#000000",
            1.0,
            0.0,
            10.0,
            &geometry,
            0.0,
        )
        .unwrap_err();
        assert!(degenerate.contains("包围盒"), "{degenerate}");
        let nan_position = attempt(
            &mut connection,
            0,
            "#000000",
            1.0,
            10.0,
            10.0,
            &geometry,
            f64::NAN,
        )
        .unwrap_err();
        assert!(nan_position.contains("位置"), "{nan_position}");

        // 全部被拒绝：画板上没有留下条目，也没有多出的历史步骤。
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_items WHERE board_id = ?1",
                board.board_id
            ),
            0
        );
        assert_eq!(
            scalar(&connection, "SELECT COUNT(*) FROM pin_board_history"),
            0
        );
    }

    #[test]
    fn read_strokes_rejects_a_corrupted_geometry_blob() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let board = create_board(&mut connection, &artwork_id, "坏几何板").unwrap();
        let revision = board_revision(&connection, board.board_id);
        let (_, item_ids) = create_stroke(
            &mut connection,
            &root,
            board.board_id,
            stroke::TOOL_PEN,
            "#000000",
            1.0,
            10.0,
            10.0,
            &pen_geometry(2),
            0.0,
            0.0,
            &revision,
        )
        .unwrap();
        assert!(read_strokes(&connection, board.board_id).is_ok());

        // 记录被外部改写（版本字节不认识 + 长度对不上）时明确报错，不静默丢条目。
        connection
            .execute(
                "UPDATE pin_board_strokes SET geometry = X'0900010002000000' WHERE item_id = ?1",
                [item_ids[0]],
            )
            .unwrap();
        let error = read_strokes(&connection, board.board_id).unwrap_err();
        assert!(error.contains("几何无效"), "{error}");
        assert!(error.contains(&item_ids[0].to_string()), "{error}");
    }

    /// 混合选择（位图 + 文字 + 笔迹）粘贴：一次事务、一次 revision，整组保持相对位置。
    #[test]
    fn pastes_mixed_items_keeping_relative_positions() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let source = create_board(&mut connection, &artwork_id, "源板").unwrap();
        let target = create_board(&mut connection, &artwork_id, "目标板").unwrap();

        let bitmap_id = insert_live_bitmap(
            &connection,
            source.board_id,
            &root,
            &artwork_id,
            0.0,
            100.0,
            40.0,
            20.0,
        );
        let revision = board_revision(&connection, source.board_id);
        let (_, text_ids) = create_text(
            &mut connection,
            &root,
            source.board_id,
            "标注",
            24.0,
            "#112233",
            "left",
            60.0,
            30.0,
            300.0,
            200.0,
            &revision,
        )
        .unwrap();
        let text_id = text_ids[0];
        let revision = board_revision(&connection, source.board_id);
        let (_, stroke_ids) = create_stroke(
            &mut connection,
            &root,
            source.board_id,
            stroke::TOOL_PEN,
            "#e5484d",
            6.0,
            30.0,
            10.0,
            &pen_geometry(4),
            400.0,
            400.0,
            &revision,
        )
        .unwrap();
        let stroke_id = stroke_ids[0];

        let source_view = load_view(&connection, &root, source.board_id).unwrap();
        let clipboard = vec![
            clipboard_item("bitmap", source.board_id, bitmap_id),
            clipboard_item("text", source.board_id, text_id),
            clipboard_item("stroke", source.board_id, stroke_id),
        ];
        let target_revision = board_revision(&connection, target.board_id);
        let (view, new_ids) = paste_items(
            &mut connection,
            &root,
            target.board_id,
            &clipboard,
            1000.0,
            500.0,
            &target_revision,
        )
        .unwrap();

        assert_eq!(new_ids.len(), 3);
        assert_eq!(view.items.len(), 3);
        assert!(view.items.iter().all(|item| !item.deleted));
        // 三类载荷都真的复制过来了：位图 DDS 落在目标画板目录，文字与笔迹各有自己的行。
        for (index, new_id) in new_ids.iter().enumerate() {
            let kind: i64 = connection
                .query_row(
                    "SELECT kind FROM pin_board_items WHERE id = ?1",
                    [new_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(kind, index as i64, "条目类型顺序应与复制记录一致");
        }
        let pasted_bitmap_path = board_directory(&root, &artwork_id, target.board_id)
            .join(format!("{}.dds", new_ids[0]));
        assert!(pasted_bitmap_path.is_file());
        assert_eq!(
            scalar(
                &connection,
                &format!(
                    "SELECT COUNT(*) FROM pin_board_texts WHERE item_id = {}",
                    new_ids[1]
                ),
            ),
            1
        );
        assert_eq!(
            scalar(
                &connection,
                &format!(
                    "SELECT COUNT(*) FROM pin_board_strokes WHERE item_id = {}",
                    new_ids[2]
                ),
            ),
            1
        );
        // 笔迹固定落在标注层；位图与文字沿用源条目的图层。
        let layers = new_ids
            .iter()
            .map(|item_id| {
                connection
                    .query_row(
                        "SELECT layer FROM pin_board_history
                          WHERE board_id = ?1 AND item_id = ?2 AND step = ?3",
                        params![target.board_id, item_id, view.now_step as i64],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(layers, vec![1, 1, stroke::STROKE_LAYER as i64]);

        // 保持相对位置：三个条目平移量一致，且整组外框中心落在放置点。
        let deltas = clipboard
            .iter()
            .zip(&new_ids)
            .map(|(clip, new_id)| {
                let before = points_of(&source_view, clip.source_item_id);
                let after = points_of(&view, *new_id);
                [after[0][0] - before[0][0], after[0][1] - before[0][1]]
            })
            .collect::<Vec<_>>();
        for delta in &deltas {
            assert!((delta[0] - deltas[0][0]).abs() < 1e-9, "{deltas:?}");
            assert!((delta[1] - deltas[0][1]).abs() < 1e-9, "{deltas:?}");
        }
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for item in &view.items {
            for [x, y] in item.points {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        assert!(((min_x + max_x) / 2.0 - 1000.0).abs() < 1e-6);
        assert!(((min_y + max_y) / 2.0 - 500.0).abs() < 1e-6);

        // 一次事务只推进一个步骤：三个条目共用同一个 step，step 0 仍保留删除态默认节点。
        assert_ne!(
            target_revision,
            board_revision(&connection, target.board_id)
        );
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_history
                  WHERE board_id = ?1 AND step = 1 AND deleted = 0",
                target.board_id,
            ),
            3
        );
        assert_eq!(
            scalar_for_board(
                &connection,
                "SELECT COUNT(*) FROM pin_board_history
                  WHERE board_id = ?1 AND step = 0 AND deleted = 1",
                target.board_id,
            ),
            3
        );
        assert!(view.now_step >= 1);
    }

    #[test]
    fn paste_rejects_stale_revision_and_trashed_sources() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let source = create_board(&mut connection, &artwork_id, "源板").unwrap();
        let target = create_board(&mut connection, &artwork_id, "目标板").unwrap();
        let bitmap_id = insert_live_bitmap(
            &connection,
            source.board_id,
            &root,
            &artwork_id,
            0.0,
            10.0,
            10.0,
            10.0,
        );
        let clipboard = vec![clipboard_item("bitmap", source.board_id, bitmap_id)];

        let error = paste_items(
            &mut connection,
            &root,
            target.board_id,
            &clipboard,
            0.0,
            0.0,
            "stale-revision",
        )
        .unwrap_err();
        assert!(error.contains("已被其他窗口"), "{error}");

        // 源画板进入回收站：源条目还在，但整块画板不再可读。
        trash_board(&mut connection, source.board_id).unwrap();
        let target_revision = board_revision(&connection, target.board_id);
        let error = paste_items(
            &mut connection,
            &root,
            target.board_id,
            &clipboard,
            0.0,
            0.0,
            &target_revision,
        )
        .unwrap_err();
        assert!(error.contains("回收站"), "{error}");
    }

    #[test]
    fn paste_rejects_deleted_sources_and_oversized_requests() {
        let (directory, mut connection) = test_repository();
        let root = directory.path().join("repository");
        let artwork_id = create_test_artwork(&connection);
        let source = create_board(&mut connection, &artwork_id, "源板").unwrap();
        let target = create_board(&mut connection, &artwork_id, "目标板").unwrap();
        let revision = board_revision(&connection, source.board_id);
        let (_, text_ids) = create_text(
            &mut connection,
            &root,
            source.board_id,
            "会被删掉的标注",
            20.0,
            "#112233",
            "left",
            40.0,
            20.0,
            100.0,
            100.0,
            &revision,
        )
        .unwrap();
        let text_id = text_ids[0];
        // 通过正常保存路径把它标成已删除：这就是「源条目已删除」的真实来源。
        let revision = board_revision(&connection, source.board_id);
        let stored = current_states(&connection, source.board_id, 0).unwrap();
        let state = stored
            .iter()
            .find(|(item_id, _)| *item_id == text_id)
            .map(|(_, state)| state)
            .unwrap();
        let editable = EditablePinBoardItem {
            item_id: text_id,
            order: state.sort_order,
            layer: state.layer,
            deleted: true,
            points: state.transform.points,
            uv: state.transform.uv,
        };
        save_board(
            &mut connection,
            &root,
            source.board_id,
            &[editable],
            &revision,
        )
        .unwrap();

        let clipboard = vec![clipboard_item("text", source.board_id, text_id)];
        let target_revision = board_revision(&connection, target.board_id);
        let error = paste_items(
            &mut connection,
            &root,
            target.board_id,
            &clipboard,
            0.0,
            0.0,
            &target_revision,
        )
        .unwrap_err();
        assert!(error.contains("已删除"), "{error}");

        // 数量超限：与导入共用同一条上限。
        let many = (0..=MAX_IMPORT_FILES)
            .map(|_| clipboard_item("text", source.board_id, text_id))
            .collect::<Vec<_>>();
        let error = paste_items(
            &mut connection,
            &root,
            target.board_id,
            &many,
            0.0,
            0.0,
            &target_revision,
        )
        .unwrap_err();
        assert!(error.contains("数量"), "{error}");
    }
}
