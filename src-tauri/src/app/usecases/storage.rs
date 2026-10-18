use crate::{
    app::command_runtime::{CommandContext, CommandError},
    storage_overview,
};

pub(crate) fn overview(
    context: &CommandContext,
) -> Result<storage_overview::RepositoryStorageOverview, CommandError> {
    context.run_read("get_repository_storage_overview", |session| {
        storage_overview::overview(session.root()).map_err(CommandError::from)
    })
}

pub(crate) fn scan_disk(
    context: &CommandContext,
) -> Result<storage_overview::RepositoryStorageDiskScan, CommandError> {
    context.run_foreground_read(None, |session| {
        context.report_progress("正在扫描仓库磁盘占用", 0, 0);
        storage_overview::scan_disk(
            session.root(),
            || context.is_cancelled(),
            |current, total| context.report_progress("正在扫描仓库磁盘占用", current, total),
        )
        .map_err(CommandError::from)
    })
}
