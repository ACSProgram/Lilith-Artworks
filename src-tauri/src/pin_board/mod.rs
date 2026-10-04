//! 素材板（pin-board）领域模块：按 Artwork 的自由画布图片工作台。
//!
//! - 前端唯一入口为 `src/modules/pin-board/api.ts`，命令名与 DTO 以本模块为准；
//! - 持久化见 `repository.rs`（schema v2 三张表），图像格式见 `dds.rs`；
//! - 普通浏览走共享读租约（`with_repository_read`），保存/导入/粘贴/删除等
//!   变更走互斥写锁（`with_ready_repository`），仓库切换与灾备持锁期间
//!   画板文件访问被阻塞。

pub(crate) mod dds;
pub(crate) mod repository;

use serde::Serialize;
use tauri::{
    ipc::{Channel, Response},
    State,
};

use crate::{app::AppState, cleanup, storage};
use repository::{
    EditablePinBoardImage, PinBoardClipboardImage, PinBoardSummary, PinBoardView,
    SavePinBoardResult,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardTransferProgress {
    label: String,
    current: usize,
    total: usize,
}

fn report_transfer_progress(
    channel: &Channel<PinBoardTransferProgress>,
    label: &str,
    current: usize,
    total: usize,
) {
    let _ = channel.send(PinBoardTransferProgress {
        label: label.to_owned(),
        current: current.min(total),
        total,
    });
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinBoardMutationResult {
    view: PinBoardView,
    image_ids: Vec<i64>,
}

#[tauri::command]
pub(crate) async fn list_pin_boards(
    state: State<'_, AppState>,
    artwork_id: String,
) -> Result<Vec<PinBoardSummary>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| {
            let connection = storage::open(root)?;
            repository::list_boards(&connection, &artwork_id)
        })
    })
    .await
    .map_err(|error| format!("画板列表读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn list_pin_board_trash(
    state: State<'_, AppState>,
) -> Result<Vec<PinBoardSummary>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| {
            let connection = storage::open(root)?;
            repository::list_trash(&connection)
        })
    })
    .await
    .map_err(|error| format!("画板回收站读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn create_pin_board(
    state: State<'_, AppState>,
    artwork_id: String,
    name: String,
) -> Result<Vec<PinBoardSummary>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::create_board(&mut connection, &artwork_id, &name)?;
            repository::list_boards(&connection, &artwork_id)
        })
    })
    .await
    .map_err(|error| format!("创建画板任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn rename_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
    name: String,
) -> Result<PinBoardSummary, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::rename_board(&mut connection, board_id, &name)
        })
    })
    .await
    .map_err(|error| format!("重命名画板任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn trash_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::trash_board(&mut connection, board_id)
        })
    })
    .await
    .map_err(|error| format!("移入回收站任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn restore_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
) -> Result<PinBoardSummary, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::restore_board(&mut connection, board_id)
        })
    })
    .await
    .map_err(|error| format!("恢复画板任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn reorder_pin_boards(
    state: State<'_, AppState>,
    artwork_id: String,
    board_ids: Vec<i64>,
) -> Result<Vec<PinBoardSummary>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::reorder_boards(&mut connection, &artwork_id, &board_ids)
        })
    })
    .await
    .map_err(|error| format!("画板排序任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn delete_pin_board_permanently(
    state: State<'_, AppState>,
    board_id: i64,
) -> Result<cleanup::CleanupReport, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            let cleanup_ids = repository::delete_board_permanently(&mut connection, board_id)?;
            cleanup::run(root, &cleanup_ids)
        })
    })
    .await
    .map_err(|error| format!("永久删除画板任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn empty_pin_board_trash(
    state: State<'_, AppState>,
) -> Result<cleanup::CleanupReport, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            let cleanup_ids = repository::empty_trash(&mut connection)?;
            cleanup::run(root, &cleanup_ids)
        })
    })
    .await
    .map_err(|error| format!("清空画板回收站任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn load_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
) -> Result<PinBoardView, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| {
            let connection = storage::open(root)?;
            repository::load_view(&connection, root, board_id)
        })
    })
    .await
    .map_err(|error| format!("画板加载任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn save_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
    images: Vec<EditablePinBoardImage>,
    expected_revision: String,
) -> Result<SavePinBoardResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            repository::save_board(&mut connection, root, board_id, &images, &expected_revision)
        })
    })
    .await
    .map_err(|error| format!("画板保存任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn finalize_pin_board(
    state: State<'_, AppState>,
    board_id: i64,
    expected_revision: String,
) -> Result<SavePinBoardResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            let (result, cleanup_ids) =
                repository::finalize_board(&mut connection, root, board_id, &expected_revision)?;
            // 结算提交成功后单遍重放；失败只留队列可重试，不阻断结算结果。
            cleanup::replay(root, &cleanup_ids);
            Ok(result)
        })
    })
    .await
    .map_err(|error| format!("画板结算任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn paste_pin_board_images(
    state: State<'_, AppState>,
    board_id: i64,
    images: Vec<PinBoardClipboardImage>,
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: String,
) -> Result<PinBoardMutationResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let mut connection = storage::open(root)?;
            let (view, image_ids) = repository::paste_images(
                &mut connection,
                root,
                board_id,
                &images,
                center_x,
                center_y,
                gap,
                &expected_revision,
            )?;
            Ok(PinBoardMutationResult { view, image_ids })
        })
    })
    .await
    .map_err(|error| format!("粘贴图片任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn import_pin_board_images(
    state: State<'_, AppState>,
    board_id: i64,
    paths: Vec<String>,
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: String,
    on_progress: Channel<PinBoardTransferProgress>,
) -> Result<PinBoardMutationResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            report_transfer_progress(&on_progress, "正在导入图片", 0, 0);
            let mut connection = storage::open(root)?;
            let (view, image_ids) = repository::import_images(
                &mut connection,
                root,
                board_id,
                &paths,
                center_x,
                center_y,
                gap,
                &expected_revision,
                |current, total| {
                    report_transfer_progress(&on_progress, "正在导入图片", current, total)
                },
            )?;
            Ok(PinBoardMutationResult { view, image_ids })
        })
    })
    .await
    .map_err(|error| format!("导入图片任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn import_pin_board_clipboard_image(
    state: State<'_, AppState>,
    board_id: i64,
    bytes: Vec<u8>,
    center_x: f64,
    center_y: f64,
    expected_revision: String,
    on_progress: Channel<PinBoardTransferProgress>,
) -> Result<PinBoardMutationResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            report_transfer_progress(&on_progress, "正在处理剪贴板图片", 0, 3);
            let mut connection = storage::open(root)?;
            let (view, image_ids) = repository::import_clipboard_image(
                &mut connection,
                root,
                board_id,
                &bytes,
                center_x,
                center_y,
                &expected_revision,
            )?;
            report_transfer_progress(&on_progress, "正在保存剪贴板图片", 3, 3);
            Ok(PinBoardMutationResult { view, image_ids })
        })
    })
    .await
    .map_err(|error| format!("导入剪贴板图片任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn export_pin_board_images(
    state: State<'_, AppState>,
    board_id: i64,
    image_ids: Vec<i64>,
    output_directory: String,
    on_progress: Channel<PinBoardTransferProgress>,
) -> Result<usize, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            let connection = storage::open(root)?;
            repository::export_images(
                &connection,
                root,
                board_id,
                &image_ids,
                &output_directory,
                |current, total| {
                    report_transfer_progress(&on_progress, "正在导出图片", current, total)
                },
            )
        })
    })
    .await
    .map_err(|error| format!("导出图片任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn read_pin_board_image_png(
    state: State<'_, AppState>,
    board_id: i64,
    image_id: i64,
) -> Result<Response, String> {
    let state = state.inner().clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| {
            let connection = storage::open(root)?;
            repository::read_image_png(&connection, root, board_id, image_id)
        })
    })
    .await
    .map_err(|error| format!("读取剪贴板图片任务异常结束：{error}"))??;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub(crate) async fn read_pin_board_texture(
    state: State<'_, AppState>,
    board_id: i64,
    image_id: i64,
    max_dimension: u32,
) -> Result<Response, String> {
    let state = state.inner().clone();
    let cache_level = state.pin_board_texture_cache_level();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| {
            let connection = storage::open(root)?;
            repository::read_texture(
                &connection,
                root,
                board_id,
                image_id,
                max_dimension,
                dds::texture_cache_budget_bytes(&cache_level),
            )
        })
    })
    .await
    .map_err(|error| format!("DDS 加载任务异常结束：{error}"))??;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub(crate) fn read_pin_board_clipboard_paths() -> Result<Vec<String>, String> {
    dds::clipboard_file_paths()
}
