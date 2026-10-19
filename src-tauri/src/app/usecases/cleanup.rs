use crate::{
    app::command_runtime::{CommandContext, CommandError},
    cleanup,
};

pub(crate) fn list_pending(
    context: &CommandContext,
) -> Result<Vec<cleanup::PendingCleanupEntry>, CommandError> {
    context.run_read("list_pending_file_cleanup", |session| {
        cleanup::list_pending(session.root()).map_err(CommandError::from)
    })
}

pub(crate) fn retry_pending(
    context: &CommandContext,
    ids: &[String],
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_write(|session| {
        cleanup::run_cancellable(session.root(), ids, context.cancel()).map_err(CommandError::from)
    })?;
    super::report_cleanup_queue(context);
    Ok(report)
}

pub(crate) fn scan_unreferenced(
    context: &CommandContext,
) -> Result<Vec<cleanup::ScanCandidate>, CommandError> {
    context.run_write(|session| {
        context.report_progress("正在扫描垃圾文件", 0, 0);
        cleanup::scan_unreferenced(
            session.root(),
            || context.is_cancelled(),
            |current, total| context.report_progress("正在扫描垃圾文件", current, total),
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn cleanup_unreferenced(
    context: &CommandContext,
    paths: &[String],
) -> Result<cleanup::CleanupReport, CommandError> {
    let report = context.run_write(|session| {
        cleanup::cleanup_unreferenced_cancellable(session.root(), paths, context.cancel())
            .map_err(CommandError::from)
    })?;
    super::report_cleanup_queue(context);
    Ok(report)
}
