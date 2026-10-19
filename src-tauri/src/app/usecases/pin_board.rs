use crate::{
    app::command_runtime::{CommandContext, CommandError},
    cleanup,
    pin_board::repository::{
        self, EditablePinBoardItem, PinBoardClipboardImage, PinBoardSummary, PinBoardView,
        SavePinBoardResult,
    },
    storage,
};

/// 画板 DDS 双向检查：无头 `scrub-board-dds` 与 GUI 仓库完整性扫描共用同一领域实现。
#[cfg(feature = "headless")]
pub(crate) fn scrub_board_dds(
    context: &CommandContext,
) -> Result<crate::pin_board::scrub::BoardDdsReport, CommandError> {
    context.run_foreground_read(None, |session| {
        crate::pin_board::scrub::scrub_board_dds(
            session.root(),
            || context.is_cancelled(),
            |current, total| context.report_progress("画板 DDS 完整性检查", current, total),
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn list_boards(
    context: &CommandContext,
    artwork_id: &str,
) -> Result<Vec<PinBoardSummary>, CommandError> {
    let label = format!("list_pin_boards artwork={artwork_id}");
    context.run_read(&label, |session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::list_boards(&connection, artwork_id).map_err(CommandError::from)
    })
}

pub(crate) fn list_trash(context: &CommandContext) -> Result<Vec<PinBoardSummary>, CommandError> {
    context.run_read("list_pin_board_trash", |session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::list_trash(&connection).map_err(CommandError::from)
    })
}

pub(crate) fn create_board(
    context: &CommandContext,
    artwork_id: &str,
    name: &str,
) -> Result<Vec<PinBoardSummary>, CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::create_board(&mut connection, artwork_id, name).map_err(CommandError::from)?;
        repository::list_boards(&connection, artwork_id).map_err(CommandError::from)
    })
}

/// 无头入口需要新建画板的修订号与名称；与 GUI 共用同一领域创建流程。
#[cfg(feature = "headless")]
pub(crate) fn create_board_context(
    context: &CommandContext,
    artwork_id: &str,
    name: &str,
) -> Result<(PinBoardSummary, repository::BoardContext), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        let summary = repository::create_board(&mut connection, artwork_id, name)
            .map_err(CommandError::from)?;
        let board_id = summary.board_id;
        let board_context = repository::open_board_context(&connection, session.root(), board_id)
            .map_err(CommandError::from)?;
        Ok((summary, board_context))
    })
}

pub(crate) fn rename_board(
    context: &CommandContext,
    board_id: i64,
    name: &str,
) -> Result<PinBoardSummary, CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::rename_board(&mut connection, board_id, name).map_err(CommandError::from)
    })
}

pub(crate) fn trash_board(context: &CommandContext, board_id: i64) -> Result<(), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::trash_board(&mut connection, board_id).map_err(CommandError::from)
    })
}

pub(crate) fn restore_board(
    context: &CommandContext,
    board_id: i64,
) -> Result<PinBoardSummary, CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::restore_board(&mut connection, board_id).map_err(CommandError::from)
    })
}

pub(crate) fn reorder_boards(
    context: &CommandContext,
    artwork_id: &str,
    board_ids: &[i64],
) -> Result<Vec<PinBoardSummary>, CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::reorder_boards(&mut connection, artwork_id, board_ids)
            .map_err(CommandError::from)
    })
}

pub(crate) fn delete_board_permanently(
    context: &CommandContext,
    board_id: i64,
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        let cleanup_ids = repository::delete_board_permanently(&mut connection, board_id)
            .map_err(CommandError::from)?;
        cleanup::run_cancellable(session.root(), &cleanup_ids, context.cancel())
            .map_err(CommandError::from)
    })?;
    super::report_cleanup_queue(context);
    Ok(report)
}

pub(crate) fn empty_trash(
    context: &CommandContext,
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        let cleanup_ids = repository::empty_trash(&mut connection).map_err(CommandError::from)?;
        cleanup::run_cancellable(session.root(), &cleanup_ids, context.cancel())
            .map_err(CommandError::from)
    })?;
    super::report_cleanup_queue(context);
    Ok(report)
}

pub(crate) fn load_board(
    context: &CommandContext,
    board_id: i64,
) -> Result<PinBoardView, CommandError> {
    let label = format!("load_pin_board board={board_id}");
    context.run_read(&label, |session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::load_view(&connection, session.root(), board_id).map_err(CommandError::from)
    })
}

pub(crate) fn save_board(
    context: &CommandContext,
    board_id: i64,
    items: &[EditablePinBoardItem],
    expected_revision: &str,
) -> Result<SavePinBoardResult, CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::save_board(
            &mut connection,
            session.root(),
            board_id,
            items,
            expected_revision,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn finalize_board(
    context: &CommandContext,
    board_id: i64,
    expected_revision: &str,
) -> Result<SavePinBoardResult, CommandError> {
    // 结算发生在关窗与切仓库时：用 Shutdown 优先级插队到写入闸门最前，并把等待
    // 上限压到 10 秒（原生端 15 秒兜底强退，留 5 秒余量），避免一次自动备份就让
    // 画板结算失败。
    context.run_repository_write_shutdown(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        let (result, cleanup_ids) = repository::finalize_board(
            &mut connection,
            session.root(),
            board_id,
            expected_revision,
        )
        .map_err(CommandError::from)?;
        // 结算提交成功后单遍重放；失败只留队列可重试，不阻断结算结果。
        cleanup::replay(session.root(), &cleanup_ids);
        Ok(result)
    })
}

pub(crate) fn paste_images(
    context: &CommandContext,
    board_id: i64,
    images: &[PinBoardClipboardImage],
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::paste_images(
            &mut connection,
            session.root(),
            board_id,
            images,
            center_x,
            center_y,
            gap,
            expected_revision,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn import_images(
    context: &CommandContext,
    board_id: i64,
    paths: &[String],
    center_x: f64,
    center_y: f64,
    gap: f64,
    expected_revision: &str,
    on_progress: impl FnMut(usize, usize),
) -> Result<(PinBoardView, Vec<i64>), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::import_images(
            &mut connection,
            session.root(),
            board_id,
            paths,
            center_x,
            center_y,
            gap,
            expected_revision,
            on_progress,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn import_clipboard_image(
    context: &CommandContext,
    board_id: i64,
    bytes: &[u8],
    center_x: f64,
    center_y: f64,
    expected_revision: &str,
) -> Result<(PinBoardView, Vec<i64>), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::import_clipboard_image(
            &mut connection,
            session.root(),
            board_id,
            bytes,
            center_x,
            center_y,
            expected_revision,
        )
        .map_err(CommandError::from)
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_text(
    context: &CommandContext,
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
) -> Result<(PinBoardView, Vec<i64>), CommandError> {
    context.run_repository_write(|session| {
        let mut connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::create_text(
            &mut connection,
            session.root(),
            board_id,
            text,
            font_size,
            color,
            align,
            measured_width,
            measured_height,
            center_x,
            center_y,
            expected_revision,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn export_images(
    context: &CommandContext,
    board_id: i64,
    item_ids: &[i64],
    output_directory: &str,
    on_progress: impl FnMut(usize, usize),
) -> Result<usize, CommandError> {
    context.run_repository_write(|session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::export_images(
            &connection,
            session.root(),
            board_id,
            item_ids,
            output_directory,
            on_progress,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn write_item_png(
    context: &CommandContext,
    board_id: i64,
    item_id: i64,
    output_directory: &str,
    bytes: &[u8],
) -> Result<String, CommandError> {
    context.run_repository_write(|session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::write_text_png(
            &connection,
            session.root(),
            board_id,
            item_id,
            output_directory,
            bytes,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn read_image_png(
    context: &CommandContext,
    board_id: i64,
    item_id: i64,
) -> Result<Vec<u8>, CommandError> {
    let label = format!("read_pin_board_image_png board={board_id} item={item_id}");
    context.run_read(&label, |session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::read_image_png(&connection, session.root(), board_id, item_id)
            .map_err(CommandError::from)
    })
}

pub(crate) fn read_texture(
    context: &CommandContext,
    board_id: i64,
    item_id: i64,
    max_dimension: u32,
    cache_level: &str,
) -> Result<Vec<u8>, CommandError> {
    let label =
        format!("read_pin_board_texture board={board_id} item={item_id} dim={max_dimension}");
    context.run_read(&label, |session| {
        let connection = storage::open(session.root()).map_err(CommandError::from)?;
        repository::read_texture(
            &connection,
            session.root(),
            board_id,
            item_id,
            max_dimension,
            crate::pin_board::dds::texture_cache_budget_bytes(cache_level),
        )
        .map_err(CommandError::from)
    })
}
