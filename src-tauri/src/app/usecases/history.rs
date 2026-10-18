use std::path::Path;

use crate::{
    app::command_runtime::{CommandContext, CommandError},
    backup, cleanup, history,
};

pub(crate) fn list(
    context: &CommandContext,
    artwork_id: &str,
) -> Result<history::ArtworkHistory, CommandError> {
    context.run_read("get_artwork_history", |session| {
        history::list(session.root(), artwork_id).map_err(CommandError::from)
    })
}

pub(crate) fn rename_node(
    context: &CommandContext,
    request: history::RenameHistoryNodeRequest,
) -> Result<history::ArtworkHistory, CommandError> {
    context.run_write(|session| {
        let node =
            history::load_node(session.root(), &request.history_id).map_err(CommandError::from)?;
        history::rename_node(session.root(), &request.history_id, &request.title)
            .map_err(CommandError::from)?;
        history::list(session.root(), &node.artwork_id).map_err(CommandError::from)
    })
}

pub(crate) fn fork_branch(
    context: &CommandContext,
    request: history::ForkBranchRequest,
) -> Result<history::ArtworkHistory, CommandError> {
    let quick_default = context.app_state().automatic_backup_quick_default();
    let result = context.run_write(|session| {
        backup::ensure_checkpoint(
            session.root(),
            &request.from_history_id,
            context.cancel(),
            context.chunk_write_options(),
        )
        .map_err(CommandError::from)?;
        history::create_branch(
            session.root(),
            &request.artwork_id,
            &request.from_history_id,
            &request.title,
            Path::new(&request.source_path),
            quick_default,
        )
        .map_err(CommandError::from)?;
        history::list(session.root(), &request.artwork_id).map_err(CommandError::from)
    });
    context.backup_state().wake_scheduler();
    result
}

/// 无头入口需要新建分支标识；与 GUI 共用同一创建流程，只是不返回整棵树。
#[cfg(feature = "headless")]
pub(crate) fn create_branch_created(
    context: &CommandContext,
    artwork_id: &str,
    from_history_id: &str,
    title: &str,
    source_path: &Path,
) -> Result<String, CommandError> {
    let quick_default = context.app_state().automatic_backup_quick_default();
    context.run_write(|session| {
        backup::ensure_checkpoint(
            session.root(),
            from_history_id,
            context.cancel(),
            context.chunk_write_options(),
        )
        .map_err(CommandError::from)?;
        history::create_branch(
            session.root(),
            artwork_id,
            from_history_id,
            title,
            source_path,
            quick_default,
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn update_branch(
    context: &CommandContext,
    request: history::UpdateBranchBackupRequest,
) -> Result<history::ArtworkHistory, CommandError> {
    let result = context.run_write(|session| {
        history::update_branch(
            session.root(),
            &request.branch_id,
            &request.title,
            request.expected_backup_enabled,
            request.backup_enabled,
            request.backup_interval_minutes,
            request.backup_quick_enabled,
            request.source_path.as_deref(),
        )
        .map_err(CommandError::from)?;
        let artwork_id = history::load_branch(session.root(), &request.branch_id)
            .map_err(CommandError::from)?
            .artwork_id;
        history::list(session.root(), &artwork_id).map_err(CommandError::from)
    });
    context.backup_state().wake_scheduler();
    result
}

pub(crate) fn delete_branch(
    context: &CommandContext,
    branch_id: &str,
) -> Result<history::ArtworkHistory, CommandError> {
    context.run_write_scoped(Some(branch_id), |session| {
        context.report_progress("正在删除分支历史", 0, 1);
        let deletion =
            history::delete_branch(session.root(), branch_id).map_err(CommandError::from)?;
        let artwork_id = deletion.artwork_id;
        // 已无引用的历史文件已在删除事务内入队，提交成功后单遍重放；
        // 删除失败只留队列可重试，不改变分支删除的成功语义。
        cleanup::replay(session.root(), &deletion.cleanup_ids);
        context.report_progress("分支删除完成", 1, 1);
        history::list(session.root(), &artwork_id).map_err(CommandError::from)
    })
}

pub(crate) fn reverify_branch(
    context: &CommandContext,
    branch_id: &str,
) -> Result<history::ArtworkHistory, CommandError> {
    let result = context.run_write_scoped(Some(branch_id), |session| {
        let artwork_id = history::load_branch(session.root(), branch_id)
            .map_err(CommandError::from)?
            .artwork_id;
        history::clear_verify_error(session.root(), branch_id).map_err(CommandError::from)?;
        history::list(session.root(), &artwork_id).map_err(CommandError::from)
    });
    context.backup_state().wake_scheduler();
    result
}
