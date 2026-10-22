use crate::{
    app::command_runtime::{CommandContext, CommandError},
    authenticity::{
        self, AuthenticityState, BranchPublication, DecodeRequest, DecodeResult,
        EnterPublicationRequest, EstimateRequest, ExportCertificationRecordRequest,
        FileSizeEstimate, PublicationPreview, PublicationPreviewRequest, PublishBranchRequest,
        PublishResult,
    },
    backup, cleanup,
    platform::path::display_path,
};

pub(crate) fn enter_publication(
    context: &CommandContext,
    state: &AuthenticityState,
    request: EnterPublicationRequest,
) -> Result<BranchPublication, CommandError> {
    let models_ready = state.model_files_ready();
    let model_info = state.model_info();
    context.run_foreground(Some(&request.branch_id), |session| {
        let (_, history_id) = authenticity::branch_head(session.root(), &request.branch_id)
            .map_err(CommandError::from)?;
        context.report_progress("正在固化发布检查点", 0, 2);
        backup::ensure_checkpoint(
            session.root(),
            &history_id,
            context.cancel(),
            context.chunk_write_options(),
        )
        .map_err(CommandError::from)?;
        context.report_progress("正在保存最终成品", 1, 2);
        authenticity::store_final_artifact(
            session.root(),
            &request.branch_id,
            &history_id,
            &request.artifact_path,
        )
        .map_err(CommandError::from)?;
        context.report_progress("分支已进入发布状态", 2, 2);
        authenticity::get_publication(session.root(), &request.branch_id, models_ready, model_info)
            .map_err(CommandError::from)
    })
}

pub(crate) fn publish(
    context: &CommandContext,
    state: &AuthenticityState,
    request: PublishBranchRequest,
) -> Result<PublishResult, CommandError> {
    let operation = state.begin_operation("认证签名发布")?;
    let branch_id = request.branch_id.clone();
    context.run_foreground(Some(&branch_id), |session| {
        authenticity::publish(session.root(), state, &operation, request)
            .map_err(CommandError::from)
    })
}

/// 发布质量预览：认证活动任务的启动与 `publish` 对齐，命令层只做授权与调度。
pub(crate) fn preview_output(
    context: &CommandContext,
    state: &AuthenticityState,
    request: PublicationPreviewRequest,
) -> Result<PublicationPreview, CommandError> {
    // 命令层会把这里的错误再包成 `Task`；先去掉领域错误自带的「后台任务失败」前缀，
    // 保持前端看到的文案与改造前一致。
    let operation = state
        .begin_operation("发布质量预览")
        .map_err(|error| match error {
            authenticity::AuthenticityError::Task(message) => CommandError::internal(message),
            other => CommandError::from(other),
        })?;
    context.run_read("preview_branch_artifact_output", |session| {
        authenticity::preview_publication(session.root(), state, &operation, request)
            .map_err(CommandError::from)
    })
}

pub(crate) fn export_record(
    context: &CommandContext,
    request: ExportCertificationRecordRequest,
) -> Result<(), CommandError> {
    context.run_read("export_certification_record", |session| {
        let source = authenticity::record_source_path(session.root(), request.record_id.trim())
            .map_err(CommandError::from)?;
        authenticity::export_record(session.root(), &source, &request.output_path)
            .map_err(CommandError::from)
    })
}

pub(crate) fn estimate_output(
    context: &CommandContext,
    request: EstimateRequest,
) -> Result<FileSizeEstimate, CommandError> {
    context.run_read("estimate_authenticity_output_size", |session| {
        authenticity::estimate_output_size(session.root(), &request).map_err(CommandError::from)
    })
}

pub(crate) fn cancel_publication(
    context: &CommandContext,
    branch_id: &str,
) -> Result<cleanup::CleanupReport, CommandError> {
    context.run_write_scoped(Some(branch_id), |session| {
        let removed =
            authenticity::remove_artifact(session.root(), branch_id).map_err(CommandError::from)?;
        for relative in &removed.retained_paths {
            log::warn!(
                "[cleanup] retained {}：内容已变化或文件不存在",
                display_path(&session.root().join(relative))
            );
        }
        cleanup::run_cancellable(session.root(), &removed.cleanup_ids, context.cancel())
            .map_err(CommandError::from)
    })
}

pub(crate) fn delete_certification_record(
    context: &CommandContext,
    record_id: &str,
) -> Result<cleanup::CleanupReport, CommandError> {
    let record_id = record_id.trim().to_owned();
    // 先读取记录所属分支用于运行锁作用域；真正的删除在独占锁内进行。
    let branch_id = context.run_read("delete_certification_record", |session| {
        authenticity::record_branch(session.root(), &record_id).map_err(CommandError::from)
    })?;
    context.run_write_scoped(Some(&branch_id), |session| {
        let cleanup_ids =
            authenticity::remove_record(session.root(), &record_id).map_err(CommandError::from)?;
        cleanup::run_cancellable(session.root(), &cleanup_ids, context.cancel())
            .map_err(CommandError::from)
    })
}

pub(crate) fn decode(
    context: &CommandContext,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> Result<DecodeResult, CommandError> {
    context.run_read("decode_authenticity", |session| {
        authenticity::decode(session.root(), state, request).map_err(CommandError::from)
    })
}
