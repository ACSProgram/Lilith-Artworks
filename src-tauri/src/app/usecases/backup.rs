use std::path::Path;

use crate::{
    app::command_runtime::{CommandContext, CommandError},
    authenticity, backup, cleanup, history, pin_board,
};

pub(crate) fn commit(
    context: &CommandContext,
    branch_id: &str,
    note: &str,
    commit_kind: &str,
    manual_priority: bool,
) -> Result<backup::BackupCommitResult, CommandError> {
    if manual_priority {
        // 手动提交优先：先登记待处理请求，让调度器延后该分支的自动备份；
        // 若同分支的后台任务正在运行，请求其让位，让手动提交先取得运行锁。
        context.backup_state().begin_manual_request(branch_id);
        if let Ok(status) = context.backup_state().status() {
            if status.active_branch_id.as_deref() == Some(branch_id) {
                let _ = context.backup_state().cancel_background();
            }
        }
    }
    log::info!(
        "manual backup started: branch_id={branch_id}, note_chars={}",
        note.chars().count()
    );
    let result = context.run_write_scoped(Some(branch_id), |session| {
        let result = backup::worker::run_backup(
            session.root(),
            branch_id,
            note,
            commit_kind,
            context.cancel(),
            context.chunk_write_options(),
        );
        if let Err(error) = result.as_ref() {
            history::mark_error(session.root(), branch_id, &error.to_string());
        }
        result.map_err(backup_error)
    });
    match &result {
        Ok(_) => log::info!("manual backup finished: branch_id={branch_id}"),
        Err(error) => log::warn!(
            "manual backup failed: branch_id={branch_id}, code={}, error={}",
            error.code,
            error.message
        ),
    }
    if manual_priority {
        context.backup_state().end_manual_request(branch_id);
        context.backup_state().wake_scheduler();
    }
    result
}

fn backup_error(error: backup::worker::BackupRunError) -> CommandError {
    match error {
        backup::worker::BackupRunError::Cancelled => CommandError::cancelled("备份操作已取消"),
        backup::worker::BackupRunError::Failed(message) => CommandError::from_legacy(message),
    }
}

pub(crate) fn restore(
    context: &CommandContext,
    history_id: &str,
    output_path: &str,
) -> Result<String, CommandError> {
    // 协调器只记录 operation id 与领域；长任务需要这一行才能把日志关联回具体节点。
    log::info!(
        "history restore target: history_id={history_id}, operation_id={}",
        context.operation_id()
    );
    context.run_foreground(None, |session| {
        backup::restore::restore(
            session.root(),
            history_id,
            output_path,
            context.cancel(),
            |label, current, total| context.report_progress(label, current, total),
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn compact(context: &CommandContext, history_id: &str) -> Result<(), CommandError> {
    log::info!(
        "history compact target: history_id={history_id}, operation_id={}",
        context.operation_id()
    );
    context.run_foreground(None, |session| {
        backup::restore::compact_node(
            session.root(),
            history_id,
            context.cancel(),
            context.chunk_write_options(),
            |label, current, total| context.report_progress(label, current, total),
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn delete_subtree(
    context: &CommandContext,
    history_id: &str,
    branch_id: &str,
) -> Result<String, CommandError> {
    log::info!(
        "history subtree delete target: history_id={history_id}, branch_id={branch_id}, operation_id={}",
        context.operation_id()
    );
    context.run_foreground(None, |session| {
        let target = history::load_node(session.root(), history_id).map_err(CommandError::from)?;
        history::validate_subtree_deletion(session.root(), history_id, branch_id)
            .map_err(CommandError::from)?;
        if let Some(parent_id) = target.parent_id.as_deref() {
            context.report_progress("正在固化保留历史", 0, 1);
            backup::restore::ensure_checkpoint_with_progress(
                session.root(),
                parent_id,
                context.cancel(),
                context.chunk_write_options(),
                |label, current, total| context.report_progress(label, current, total),
            )
            .map_err(CommandError::from)?;
            context.report_progress("正在删除历史节点", 1, 1);
        }
        let deletion = history::delete_subtree(session.root(), history_id, branch_id)
            .map_err(CommandError::from)?;
        // 已无引用的历史文件已在删除事务内入队，提交成功后单遍重放；
        // 失败只留队列可重试，不改变删除节点的成功语义。
        cleanup::replay(session.root(), &deletion.cleanup_ids);
        Ok(deletion.artwork_id)
    })
}

pub(crate) fn set_checkpoint(
    context: &CommandContext,
    history_id: &str,
    enabled: bool,
) -> Result<(), CommandError> {
    log::info!(
        "history checkpoint target: history_id={history_id}, enabled={enabled}, operation_id={}",
        context.operation_id()
    );
    context.run_foreground(None, |session| {
        if enabled {
            return backup::restore::ensure_checkpoint_with_progress(
                session.root(),
                history_id,
                context.cancel(),
                context.chunk_write_options(),
                |label, current, total| context.report_progress(label, current, total),
            )
            .map_err(CommandError::from);
        }
        // 取消检查点释放的 snapshot 已在事务内入队，提交成功后单遍重放。
        if let Some(cleanup_ids) =
            history::unmark_checkpoint(session.root(), history_id).map_err(CommandError::from)?
        {
            context.report_progress("正在释放检查点并恢复增量统计", 0, 1);
            cleanup::replay(session.root(), &cleanup_ids);
            context.report_progress("检查点已取消", 1, 1);
        }
        Ok(())
    })
}

pub(crate) fn scrub_repository(
    context: &CommandContext,
) -> Result<crate::app::workflows::RepositoryScrubReport, CommandError> {
    context.run_foreground_read(None, |session| {
        let root = session.root();
        context.report_progress("正在检查历史文件", 0, 0);
        let history_nodes = backup::scrub_history(root, context.cancel(), |current, total| {
            context.report_progress("正在检查历史文件", current, total)
        })
        .map_err(CommandError::from)?;
        context.report_progress("正在检查发布文件", 0, 0);
        let controlled = authenticity::scrub_controlled_files(
            root,
            || context.is_cancelled(),
            |current, total| context.report_progress("正在检查发布文件", current, total),
        )
        .map_err(CommandError::from)?;
        context.report_progress("正在检查画板图片", 0, 0);
        let board_dds = pin_board::scrub::scrub_board_dds(
            root,
            || context.is_cancelled(),
            |current, total| context.report_progress("正在检查画板图片", current, total),
        )
        .map_err(CommandError::from)?;
        Ok(crate::app::workflows::RepositoryScrubReport {
            history_nodes,
            final_artifacts: controlled.0,
            certification_records: controlled.1,
            pin_board_images: board_dds.images,
            pin_board_missing_dds: board_dds.missing,
            pin_board_corrupt_dds: board_dds.corrupt,
            pin_board_orphan_dds: board_dds.orphans,
        })
    })
}

/// 无头 `scrub` 的既有口径：历史链 + 认证受控文件，不包含画板 DDS
/// （GUI 的仓库完整性扫描另行包含第三节）。
#[cfg(feature = "headless")]
pub(crate) fn scrub_history_and_controlled(
    context: &CommandContext,
) -> Result<(u64, u64, u64), CommandError> {
    context.run_foreground_read(None, |session| {
        let root = session.root();
        let nodes = backup::scrub_history(root, context.cancel(), |current, total| {
            context.report_progress("全库扫描", current, total)
        })
        .map_err(CommandError::from)?;
        let (final_artifacts, certification_records) = authenticity::scrub_controlled_files(
            root,
            || context.is_cancelled(),
            |current, total| context.report_progress("认证受控文件检查", current, total),
        )
        .map_err(CommandError::from)?;
        Ok((nodes, final_artifacts, certification_records))
    })
}

pub(crate) fn create_repository_backup(
    context: &CommandContext,
    destination_parent: &Path,
) -> Result<backup::RepositoryBackupReport, CommandError> {
    context.run_foreground(None, |session| {
        context.report_progress("正在准备备份", 0, 0);
        backup::create_repository_backup(
            session.root(),
            destination_parent,
            context.cancel(),
            |label, current, total| context.report_progress(label, current, total),
        )
        .map_err(CommandError::from)
    })
}

pub(crate) fn acknowledge_notices(
    context: &CommandContext,
    artwork_ids: &[String],
) -> Result<(), CommandError> {
    context.run_write(|session| {
        history::acknowledge_backup_disable_notices(session.root(), artwork_ids)
            .map_err(CommandError::from)
    })
}

pub(crate) fn notice_target(
    context: &CommandContext,
) -> Result<Option<history::BackupDisableNoticeTarget>, CommandError> {
    context.run_read("get_backup_disable_notice_target", |session| {
        history::next_backup_disable_notice_target(session.root()).map_err(CommandError::from)
    })
}

#[cfg(feature = "headless")]
pub(crate) fn list_scheduled(
    context: &CommandContext,
) -> Result<Vec<history::ScheduledBranch>, CommandError> {
    context.run_read("scheduler-scan", |session| {
        history::list_scheduled(session.root()).map_err(CommandError::from)
    })
}
