use std::path::Path;

use crate::{
    app::{
        command_runtime::{CommandContext, CommandError},
        contracts::RepositoryScrubProvider,
    },
    authenticity, backup, cleanup,
    contracts::BackupVerificationMode,
    history, pin_board, storage,
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
        match &result {
            // 经此入口的成功提交无论是否产生新节点都完整读过工作文件，连续
            // 快速命中计数随之归零；下一次自动检查重新从零开始积累。
            Ok(_) => {
                if let Err(error) = history::reset_quick_hit_count(session.root(), branch_id) {
                    log::warn!("failed to reset quick hit count for branch {branch_id}: {error}");
                }
            }
            Err(error) => history::mark_error(session.root(), branch_id, &error.to_string()),
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
        // 取消检查点：提交路径不再为检查点父节点写反向增量，所以这里先在事务外
        // 按需重建（子边已持有增量的旧仓库直接沿用），再在一个事务里换掉 snapshot。
        let Some(plan) = history::plan_checkpoint_release(session.root(), history_id)
            .map_err(CommandError::from)?
        else {
            return Ok(());
        };
        let mut rebuilt: Option<(String, u64)> = None;
        if plan.existing_delta.is_none() {
            context.report_progress("正在重建检查点增量", 0, 0);
            rebuilt = Some(
                backup::restore::build_reverse_delta(
                    session.root(),
                    &plan.child_id,
                    history_id,
                    context.cancel(),
                    context.chunk_write_options(),
                    |label, current, total| context.report_progress(label, current, total),
                )
                .map_err(CommandError::from)?,
            );
        }
        // 取消检查点释放的 snapshot 已在事务内入队，提交成功后单遍重放。
        let result = history::unmark_checkpoint(
            session.root(),
            history_id,
            rebuilt
                .as_ref()
                .map(|(relative, size)| (relative.as_str(), *size)),
        );
        match result {
            Ok(Some(cleanup_ids)) => {
                context.report_progress("正在释放检查点并恢复增量统计", 0, 1);
                cleanup::replay(session.root(), &cleanup_ids);
                context.report_progress("检查点已取消", 1, 1);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                // 事务没有成功，刚写出的增量没有任何记录指向它。
                if let Some((relative, _)) = rebuilt {
                    if let Ok(path) = storage::resolve_path(session.root(), &relative) {
                        let _ = std::fs::remove_file(path);
                    }
                }
                Err(CommandError::from(error))
            }
        }
    })
}

pub(crate) fn scrub_repository(
    context: &CommandContext,
) -> Result<crate::app::workflows::RepositoryScrubReport, CommandError> {
    // 扫描现在顺带刷新节点级校验状态，因此按前台写命令取得跨进程 owner lock：
    // 只读命令不得变成写者的原则不再适用于这个入口。
    context.run_foreground(None, |session| {
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
            pin_board_bitmaps: board_dds.bitmaps,
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
    // 与 GUI 扫描相同：历史节点状态会被刷新，因此按写命令执行。
    context.run_foreground(None, |session| {
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
        let report = backup::create_repository_backup(
            session.root(),
            destination_parent,
            context.cancel(),
            &RepositoryScrubProvider,
            BackupVerificationMode::Sample,
            |label, current, total| context.report_progress(label, current, total),
        )
        .map_err(CommandError::from)?;
        // 结果记录失败不改变已经成功的备份；前端仍可用本次返回值展示。
        if let Err(error) = backup::record_repository_backup_run(session.root(), &report) {
            log::warn!("repository backup result could not be recorded: {error}");
        }
        Ok(report)
    })
}

pub(crate) fn last_repository_backup(
    context: &CommandContext,
) -> Result<Option<backup::RepositoryBackupRun>, CommandError> {
    context.run_read("get_last_repository_backup_run", |session| {
        backup::last_repository_backup_run(session.root()).map_err(CommandError::from)
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

#[cfg(test)]
mod tests {
    //! 手动提交与快速命中计数的契约：成功提交（无论是否产生新节点）都完整读过
    //! 工作文件，连续快速命中计数必须归零；失败与取消没有完整读，计数必须原样
    //! 保留——否则一次失败的提交会白白推迟下一次全量内容确认。
    //!
    //! `reset_quick_hit_count` 本身在 `history::repository`，它的持久化行为由
    //! `backup::scheduler` 的单测覆盖（`reset_quick_hit_count_clears_the_persisted_value`）；
    //! 这里补的是**应用层入口**：走 `usecases::backup::commit` 的完整锁序与错误处理路径。

    use super::*;
    use crate::app::command_runtime::{
        CommandErrorKind, NoopEventSink, OperationCoordinator, OperationDomain,
    };
    use crate::backup::chunk_file::ChunkWriteOptions;
    use crate::backup::worker;
    use crate::backup::BackupState;
    use crate::cancel::CancelToken;
    use std::{fs, io::Write, path::Path, sync::Arc};

    /// 仓库 + 分支 + 仓库外的工作文件。与 `backup::scheduler` 的测试夹具同构。
    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::File::create(&source)
            .unwrap()
            .write_all(b"content v1")
            .unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, true).unwrap();
        (directory, root, source, artwork.branch_id)
    }

    /// 指向给定仓库的协调器。`repository_path` 必须与仓库一致：
    /// `execute` 用它推导 `repository_key` 并取跨进程锁。
    fn coordinator_for(root: &Path, directory: &Path, name: &str) -> OperationCoordinator {
        let mut settings = crate::app::settings::AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let state = crate::app::AppState::new(
            settings,
            directory.join(format!("{name}.json")),
            directory.join(format!("{name}-logs")),
            None,
        );
        OperationCoordinator::new(state, BackupState::default(), Arc::new(NoopEventSink))
    }

    fn set_quick_hits(root: &Path, branch_id: &str, hits: u32) {
        crate::storage::open(root)
            .unwrap()
            .execute(
                "UPDATE branches SET quick_hit_count = ?2 WHERE id = ?1",
                rusqlite::params![branch_id, hits],
            )
            .unwrap();
    }

    fn quick_hits(root: &Path, branch_id: &str) -> u32 {
        history::load_scheduled(root, branch_id)
            .unwrap()
            .unwrap()
            .quick_hit_count
    }

    #[test]
    fn manual_commit_resets_quick_hit_count_on_success() {
        let (directory, root, source, branch_id) = fixture();
        let coordinator = coordinator_for(&root, directory.path(), "success");
        // 先建立 head；这次提交同样应把计数归零（初始就是 0，主要是确认路径可用）。
        let first = coordinator
            .execute(OperationDomain::Backup, |context| {
                commit(context, &branch_id, "first", "manual", true)
            })
            .unwrap();
        assert!(first.created);

        set_quick_hits(&root, &branch_id, 6);
        // 内容变化 → 新节点：完整读过工作文件，计数归零。
        fs::File::create(&source)
            .unwrap()
            .write_all(b"content v2")
            .unwrap();
        let second = coordinator
            .execute(OperationDomain::Backup, |context| {
                commit(context, &branch_id, "second", "manual", true)
            })
            .unwrap();
        assert!(second.created);
        assert_eq!(
            quick_hits(&root, &branch_id),
            0,
            "成功的手动提交必须清零连续快速命中计数"
        );

        // 内容不变（head 比对提前返回）也算完整读过：计数同样归零。
        set_quick_hits(&root, &branch_id, 3);
        let third = coordinator
            .execute(OperationDomain::Backup, |context| {
                commit(context, &branch_id, "unchanged", "manual", true)
            })
            .unwrap();
        assert!(third.unchanged);
        assert_eq!(quick_hits(&root, &branch_id), 0);
    }

    #[test]
    fn manual_commit_keeps_quick_hit_count_on_failure() {
        let (directory, root, source, branch_id) = fixture();
        let coordinator = coordinator_for(&root, directory.path(), "failure");
        coordinator
            .execute(OperationDomain::Backup, |context| {
                commit(context, &branch_id, "first", "manual", true)
            })
            .unwrap();
        set_quick_hits(&root, &branch_id, 6);

        // 工作文件消失 → 提交失败：没有完整读，计数必须原样保留。
        fs::remove_file(&source).unwrap();
        let result = coordinator.execute(OperationDomain::Backup, |context| {
            commit(context, &branch_id, "missing source", "manual", true)
        });
        assert!(result.is_err(), "源文件缺失时提交必须失败");
        assert_eq!(
            quick_hits(&root, &branch_id),
            6,
            "失败的提交不得清零连续快速命中计数"
        );
    }

    #[test]
    fn manual_commit_keeps_quick_hit_count_on_cancellation() {
        let (directory, root, _source, branch_id) = fixture();
        let coordinator = coordinator_for(&root, directory.path(), "cancelled");
        coordinator
            .execute(OperationDomain::Backup, |context| {
                commit(context, &branch_id, "first", "manual", true)
            })
            .unwrap();
        set_quick_hits(&root, &branch_id, 6);

        let token = CancelToken::standalone();
        token.cancel();
        let result = coordinator.execute_with(OperationDomain::Backup, token, |context| {
            commit(context, &branch_id, "cancelled", "manual", true)
        });
        let error = result.expect_err("已取消的提交必须失败");
        assert_eq!(error.kind, CommandErrorKind::Cancelled);
        assert_eq!(
            quick_hits(&root, &branch_id),
            6,
            "被取消的提交不得清零连续快速命中计数"
        );
    }

    /// 直接调用 `worker::run_backup`（不经 use-case）**不得**触碰该计数：
    /// 归零只属于「经应用入口完整读过工作文件」的路径，调度器自己的全量确认另有清零点
    /// （`backup::scheduler` 走 `record_check_result`，由 `history::record_check_result`
    /// 在全量模式下清零）。
    #[test]
    fn worker_run_backup_alone_does_not_touch_quick_hit_count() {
        let (_directory, root, source, branch_id) = fixture();
        worker::run_backup(
            &root,
            &branch_id,
            "first",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        set_quick_hits(&root, &branch_id, 6);

        fs::File::create(&source)
            .unwrap()
            .write_all(b"content v2")
            .unwrap();
        worker::run_backup(
            &root,
            &branch_id,
            "second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert_eq!(
            quick_hits(&root, &branch_id),
            6,
            "绕过应用入口的提交不得清零计数"
        );
    }
}

#[cfg(feature = "headless")]
pub(crate) fn list_scheduled(
    context: &CommandContext,
) -> Result<Vec<history::ScheduledBranch>, CommandError> {
    context.run_read("scheduler-scan", |session| {
        history::list_scheduled(session.root()).map_err(CommandError::from)
    })
}
