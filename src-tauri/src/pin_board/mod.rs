//! 素材板（pin-board）领域模块：按 Artwork 的自由画布图片工作台。
//!
//! - 前端唯一入口为 `src/modules/pin-board/api.ts`，命令名与 DTO 以本模块为准；
//! - 持久化见 `repository.rs`（schema v6 多态条目表），笔迹几何契约见 `stroke.rs`，
//!   图像格式见 `dds.rs`，阵列排序见 `arrange.rs`（整理与导入落位共用同一份实现）；
//! - 普通浏览走共享读租约，保存/导入/粘贴/删除等变更走仓库 lease + 仓库操作锁
//!   （`OperationCoordinator::run_repository_write`），保持不取共享运行锁的既有语义。

pub(crate) mod arrange;
mod contracts;
pub(crate) mod dds;
pub(crate) mod repository;
pub(crate) mod scrub;
mod stroke;

pub(crate) use contracts::PinBoardReferenceInventory;

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{ipc::Response, State};
use tauri_plugin_fs::FsExt;

use crate::{
    app::{command_runtime::OperationDomain, usecases, OperationCoordinator},
    cleanup, path_scope,
    platform::clipboard,
};
use arrange::{ArrangeItem, ArrangeOptions, HeightPolicy, ItemOrder};
use repository::{
    EditablePinBoardItem, PinBoardClipboardImage, PinBoardSummary, PinBoardView, SavePinBoardResult,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardMutationResult {
    view: PinBoardView,
    item_ids: Vec<i64>,
}

#[tauri::command]
pub(crate) async fn list_pin_boards(
    coordinator: State<'_, OperationCoordinator>,
    artwork_id: String,
) -> Result<Vec<PinBoardSummary>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::list_boards(context, &artwork_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn list_pin_board_trash(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Vec<PinBoardSummary>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, |context| {
            usecases::pin_board::list_trash(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn create_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    artwork_id: String,
    name: String,
) -> Result<Vec<PinBoardSummary>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::create_board(context, &artwork_id, &name)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn rename_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    name: String,
) -> Result<PinBoardSummary, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::rename_board(context, board_id, &name)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn trash_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
) -> Result<(), String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::trash_board(context, board_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn restore_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
) -> Result<PinBoardSummary, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::restore_board(context, board_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn reorder_pin_boards(
    coordinator: State<'_, OperationCoordinator>,
    artwork_id: String,
    board_ids: Vec<i64>,
) -> Result<Vec<PinBoardSummary>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::reorder_boards(context, &artwork_id, &board_ids)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn delete_pin_board_permanently(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
) -> Result<cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::delete_board_permanently(context, board_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn empty_pin_board_trash(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, |context| {
            usecases::pin_board::empty_trash(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn load_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
) -> Result<PinBoardView, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::load_board(context, board_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn save_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    items: Vec<EditablePinBoardItem>,
    expected_revision: String,
) -> Result<SavePinBoardResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::save_board(context, board_id, &items, &expected_revision)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn finalize_pin_board(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    expected_revision: String,
) -> Result<SavePinBoardResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::finalize_board(context, board_id, &expected_revision)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn paste_pin_board_images(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    images: Vec<PinBoardClipboardImage>,
    center_x: f64,
    center_y: f64,
    gap_ratio: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            let (view, item_ids) = usecases::pin_board::paste_images(
                context,
                board_id,
                &images,
                center_x,
                center_y,
                gap_ratio,
                &expected_revision,
            )?;
            Ok(PinBoardMutationResult { view, item_ids })
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn import_pin_board_images(
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
    board_id: i64,
    paths: Vec<String>,
    center_x: f64,
    center_y: f64,
    gap_ratio: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    // 源图路径必须由文件选择器（或剪贴板读取命令）写入文件系统 scope；仓库边界校验
    // 在 `repository::import_images` 里对 GUI 与无头两个入口一起生效。
    for path in &paths {
        path_scope::ensure_authorized(&window.fs_scope(), Path::new(path), "导入图片")?;
    }
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            context.report_progress("正在导入图片", 0, 0);
            let (view, item_ids) = usecases::pin_board::import_images(
                context,
                board_id,
                &paths,
                center_x,
                center_y,
                gap_ratio,
                &expected_revision,
                |current, total| {
                    context.report_progress("正在导入图片", current as u64, total as u64)
                },
            )?;
            Ok(PinBoardMutationResult { view, item_ids })
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn import_pin_board_clipboard_image(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    bytes: Vec<u8>,
    center_x: f64,
    center_y: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            context.report_progress("正在处理剪贴板图片", 0, 3);
            let (view, item_ids) = usecases::pin_board::import_clipboard_image(
                context,
                board_id,
                &bytes,
                center_x,
                center_y,
                &expected_revision,
            )?;
            context.report_progress("正在保存剪贴板图片", 3, 3);
            Ok(PinBoardMutationResult { view, item_ids })
        })
        .await
        .map_err(String::from)
}

/// 新增文字条目：文本、字体度量与颜色由前端给出，Rust 只负责存储与位置。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn create_pin_board_text(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    text: String,
    font_size: f64,
    color: String,
    align: String,
    measured_width: f64,
    measured_height: f64,
    center_x: f64,
    center_y: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            let (view, item_ids) = usecases::pin_board::create_text(
                context,
                board_id,
                &text,
                font_size,
                &color,
                &align,
                measured_width,
                measured_height,
                center_x,
                center_y,
                &expected_revision,
            )?;
            Ok(PinBoardMutationResult { view, item_ids })
        })
        .await
        .map_err(String::from)
}

/// 新增笔迹条目：几何 BLOB 由前端编码（布局见 `stroke.rs`），原生端只校验结构与
/// 包围盒。`geometry` 走普通参数（`Uint8Array` → JSON 数字数组），与
/// `import_pin_board_clipboard_image` 的字节参数同一条通道——单笔 4096 点约
/// 20 KB 原始数据、约 75 KB JSON，不值得为此另开一条原始载荷通道。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn create_pin_board_stroke(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    tool: u8,
    color: String,
    stroke_width: f64,
    width: f64,
    height: f64,
    geometry: Vec<u8>,
    center_x: f64,
    center_y: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            let (view, item_ids) = usecases::pin_board::create_stroke(
                context,
                board_id,
                tool,
                &color,
                stroke_width,
                width,
                height,
                &geometry,
                center_x,
                center_y,
                &expected_revision,
            )?;
            Ok(PinBoardMutationResult { view, item_ids })
        })
        .await
        .map_err(String::from)
}

/// 读取整板笔迹几何（二进制，布局见 `stroke::encode_batch`）：`u32 条目数` +
/// 每条目 `(u32 itemId, u8 tool, u32 color, f32 strokeWidth, f32 width, f32 height,
/// u32 geometryLen, bytes)`。前端一次解析成 `Map<itemId, StrokeGeometry>`，零 JSON。
#[tauri::command]
pub(crate) async fn read_pin_board_strokes(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
) -> Result<Response, String> {
    let coordinator = coordinator.inner().clone();
    let bytes = coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::read_strokes(context, board_id)
        })
        .await
        .map_err(String::from)?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub(crate) async fn export_pin_board_images(
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
    board_id: i64,
    item_ids: Vec<i64>,
    output_directory: String,
) -> Result<usize, String> {
    path_scope::ensure_authorized(
        &window.fs_scope(),
        Path::new(output_directory.trim()),
        "导出位置",
    )?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::export_images(
                context,
                board_id,
                &item_ids,
                &output_directory,
                |current, total| {
                    context.report_progress("正在导出图片", current as u64, total as u64)
                },
            )
        })
        .await
        .map_err(String::from)
}

/// 写出文字条目的导出图：前端按屏幕同一套字体度量栅格化并编码 PNG，原生端
/// 只校验格式与尺寸后原子写盘。
#[tauri::command]
pub(crate) async fn write_pin_board_item_png(
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
    board_id: i64,
    item_id: i64,
    output_directory: String,
    bytes: Vec<u8>,
) -> Result<String, String> {
    path_scope::ensure_authorized(
        &window.fs_scope(),
        Path::new(output_directory.trim()),
        "导出位置",
    )?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::write_item_png(
                context,
                board_id,
                item_id,
                &output_directory,
                &bytes,
            )
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn read_pin_board_image_png(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    item_id: i64,
) -> Result<Response, String> {
    let coordinator = coordinator.inner().clone();
    let bytes = coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::read_image_png(context, board_id, item_id)
        })
        .await
        .map_err(String::from)?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub(crate) async fn read_pin_board_texture(
    coordinator: State<'_, OperationCoordinator>,
    board_id: i64,
    item_id: i64,
    max_dimension: u32,
) -> Result<Response, String> {
    let cache_level = coordinator
        .inner()
        .app_state()
        .pin_board_texture_cache_level();
    let coordinator = coordinator.inner().clone();
    let bytes = coordinator
        .spawn(OperationDomain::PinBoard, move |context| {
            usecases::pin_board::read_texture(
                context,
                board_id,
                item_id,
                max_dimension,
                &cache_level,
            )
        })
        .await
        .map_err(String::from)?;
    Ok(Response::new(bytes))
}

/// 读取剪贴板里的文件路径（Windows `CF_HDROP`）。
///
/// 这是「复制文件后在素材板里粘贴」的入口：路径由本命令自己从系统剪贴板读出，不是
/// 渲染进程传进来的值，因此可以在这里把它们加入文件系统 scope——随后
/// `import_pin_board_images` 仍走同一条「路径必须已被授权」的检查，既不破坏粘贴
/// 流程，也不新增一条绕过授权的通道。只授权确实存在的文件。
#[tauri::command]
pub(crate) fn read_pin_board_clipboard_paths(
    window: tauri::WebviewWindow,
) -> Result<Vec<String>, String> {
    // 平台剪贴板读取返回全部文件路径；「最多导入多少个」是素材板的业务上限，
    // 在这里按 `MAX_IMPORT_FILES` 截断（与迁移前在读取侧截断的结果一致）。
    let mut paths = clipboard::file_paths()?;
    if paths.len() > dds::MAX_IMPORT_FILES {
        paths.truncate(dds::MAX_IMPORT_FILES);
    }
    for path in &paths {
        let file = Path::new(path);
        if file.is_file() {
            path_scope::authorize_path(&window, file)?;
        }
    }
    Ok(paths)
}

/// 阵列排序的单张输入：当前显示尺寸与世界坐标下的中心点。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardArrangeInput {
    item_id: i64,
    width: f64,
    height: f64,
    center_x: f64,
    center_y: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardArrangeResult {
    item_id: i64,
    points: [[f64; 2]; 4],
}

/// 计算阵列排序后的位置，不写数据库。
///
/// 这是纯计算命令，因此刻意不经过 `OperationCoordinator`：没有仓库访问、没有
/// 共享租约，前端拿到 points 后仍走既有的 `commitMutation`，撤销栈、脏标记和
/// 自动保存的行为完全不变。间距按「统一高度的比例」传入，与画布缩放无关。
#[tauri::command]
pub(crate) fn arrange_pin_board_images(
    items: Vec<PinBoardArrangeInput>,
    gap_ratio: f64,
) -> Result<Vec<PinBoardArrangeResult>, String> {
    if items.len() < 2 {
        return Err("至少需要两张图片才能阵列排序".into());
    }
    if !gap_ratio.is_finite() || !(0.0..=arrange::MAX_GAP_RATIO).contains(&gap_ratio) {
        return Err("阵列间距超出允许范围".into());
    }
    let inputs = items
        .iter()
        .map(|item| ArrangeItem {
            item_id: item.item_id,
            width: item.width,
            height: item.height,
            center_x: item.center_x,
            center_y: item.center_y,
        })
        .collect::<Vec<_>>();
    let arranged = arrange::arrange(
        &inputs,
        ArrangeOptions {
            order: ItemOrder::Reading,
            height: HeightPolicy::Median,
            gap_ratio,
        },
    );
    // 尺寸退化的条目会被 `arrange` 跳过；少一张就少排一张，不因此让整个操作失败。
    if arranged.len() < 2 {
        return Err("可参与阵列排序的图片不足两张".into());
    }
    Ok(arranged
        .into_iter()
        .map(|(item_id, points)| PinBoardArrangeResult { item_id, points })
        .collect())
}
