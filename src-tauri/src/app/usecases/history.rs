use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::{
    app::command_runtime::{CommandContext, CommandError, RepositorySession},
    backup, cleanup, history, preview, storage,
};

pub(crate) fn list(
    context: &CommandContext,
    artwork_id: &str,
) -> Result<history::ArtworkHistory, CommandError> {
    context.run_read("get_artwork_history", |session| {
        history::list(session.root(), artwork_id).map_err(CommandError::from)
    })
}

/// 读取节点当前存储文件的格式标签；文件缺失或损坏时返回 `None`，不阻断详情展示。
pub(crate) fn node_format(
    context: &CommandContext,
    history_id: &str,
) -> Result<Option<String>, CommandError> {
    context.run_read("get_history_node_format", |session| {
        let Some(relative) =
            history::node_storage_path(session.root(), history_id).map_err(CommandError::from)?
        else {
            return Ok(None);
        };
        let path = storage::resolve_path(session.root(), &relative).map_err(CommandError::from)?;
        Ok(backup::chunk_file::stored_format_label(&path)
            .ok()
            .map(str::to_string))
    })
}

/// 读取节点随行保存的派生缩略图，并说明该节点的格式是否支持预览。
///
/// 没有预览时 `data_url` 为 `None`；`supported` 由节点所属分支工作文件的扩展名
/// 决定，前端据此决定要不要给出「无可用预览」占位与刷新入口。两者都只影响展示，
/// 不改变历史、校验或存储状态。
pub(crate) fn thumbnail(
    context: &CommandContext,
    history_id: &str,
) -> Result<history::NodePreview, CommandError> {
    context.run_read("get_history_node_thumbnail", |session| {
        let root = session.root();
        let supported = history::node_source_path(root, history_id)
            .map_err(CommandError::from)?
            .is_some_and(|path| preview::supports_preview(Path::new(&path)));
        let data_url = history::load_thumbnail(root, history_id)
            .map_err(CommandError::from)?
            // 只服务当前编码版本：将来换编码时，旧行不会被当作 WebP 发给前端，
            // 重建由版本号驱动。
            .filter(|(_, version)| *version == preview::THUMBNAIL_VERSION)
            .map(|(bytes, _)| format!("data:image/webp;base64,{}", STANDARD.encode(bytes)));
        Ok(history::NodePreview {
            data_url,
            supported,
        })
    })
}

/// 重新从节点自身已存储的内容抽取缩略图。
///
/// 工作文件在节点创建后可能已经变化，因此这里物化该节点再抽取，而不是重读当前
/// 工作文件；成功时写回缩略图列并返回 data URL，抽取不到内容时仍返回 `None`。
pub(crate) fn refresh_thumbnail(
    context: &CommandContext,
    history_id: &str,
) -> Result<Option<String>, CommandError> {
    context.run_foreground(None, |session| {
        let root = session.root();
        let thumbnail = backup::extract_node_thumbnail(
            root,
            history_id,
            context.cancel(),
            |label, current, total| context.report_progress(label, current, total),
        )
        .map_err(CommandError::from)?;
        let Some(thumbnail) = thumbnail else {
            log::info!("history thumbnail refresh produced no preview: history_id={history_id}");
            return Ok(None);
        };
        history::store_thumbnail(root, history_id, &thumbnail.bytes, thumbnail.version)
            .map_err(CommandError::from)?;
        log::info!(
            "history thumbnail refreshed: history_id={history_id}, bytes={}",
            thumbnail.bytes.len()
        );
        Ok(Some(format!(
            "data:image/webp;base64,{}",
            STANDARD.encode(&thumbnail.bytes)
        )))
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

/// 新建分支的共用流程：先把起点固化成检查点，再在单个事务里插入分支并标记
/// 起点。GUI 与无头入口只差外层是否再读整棵树。
fn create_branch_at_checkpoint(
    context: &CommandContext,
    session: &RepositorySession<'_>,
    artwork_id: &str,
    from_history_id: &str,
    title: &str,
    source_path: &Path,
    quick_default: bool,
) -> Result<String, CommandError> {
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
}

pub(crate) fn fork_branch(
    context: &CommandContext,
    request: history::ForkBranchRequest,
) -> Result<history::ArtworkHistory, CommandError> {
    let quick_default = context.app_state().automatic_backup_quick_default();
    let result = context.run_write(|session| {
        create_branch_at_checkpoint(
            context,
            session,
            &request.artwork_id,
            &request.from_history_id,
            &request.title,
            Path::new(&request.source_path),
            quick_default,
        )?;
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
        create_branch_at_checkpoint(
            context,
            session,
            artwork_id,
            from_history_id,
            title,
            source_path,
            quick_default,
        )
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

/// 手动重查分支：前台立即按链校验整条分支，而不是只清错误等空闲调度。
///
/// 低频率的自动节奏下，手动入口必须即时；head、检查点与分支起点各自作为起点，
/// 共享前缀只解析一次。出问题的节点写入自己的 `verify_error`，被它挡住的上游节点
/// 写入「无法检查」，两者都不中止后续节点，最终返回刷新后的历史让状态行展示派生结果。
pub(crate) fn reverify_branch(
    context: &CommandContext,
    branch_id: &str,
) -> Result<history::ArtworkHistory, CommandError> {
    let result = context.run_foreground(Some(branch_id), |session| {
        let root = session.root();
        let artwork_id = history::load_branch(root, branch_id)
            .map_err(CommandError::from)?
            .artwork_id;
        history::clear_branch_verify_errors(root, branch_id).map_err(CommandError::from)?;
        let total = match backup::verify_branch_history(
            root,
            branch_id,
            context.cancel(),
            |current, total| context.report_progress("正在校验分支历史", current, total),
        ) {
            Ok(total) => total,
            Err(error) => {
                if context.is_cancelled() {
                    return Err(CommandError::cancelled("分支校验已取消"));
                }
                return Err(CommandError::from(error));
            }
        };
        context.report_progress("分支校验完成", total, total);
        history::list(root, &artwork_id).map_err(CommandError::from)
    });
    context.backup_state().wake_scheduler();
    result
}
