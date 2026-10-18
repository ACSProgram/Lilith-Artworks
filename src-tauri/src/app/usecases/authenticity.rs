use crate::{
    app::command_runtime::{CommandContext, CommandError},
    authenticity::{
        self, AuthenticityState, BranchPublication, DecodeRequest, DecodeResult,
        EnterPublicationRequest, PublishBranchRequest, PublishResult,
    },
    backup, cleanup,
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
        backup::ensure_checkpoint(session.root(), &history_id).map_err(CommandError::from)?;
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
    let operation = state
        .begin_operation("认证签名发布")
        .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    let branch_id = request.branch_id.clone();
    context.run_foreground(Some(&branch_id), |session| {
        authenticity::publish_artifact(session.root(), state, &operation, request)
            .map_err(|error| CommandError::from_legacy(error.to_string()))
    })
}

pub(crate) fn cancel_publication(
    context: &CommandContext,
    branch_id: &str,
) -> Result<cleanup::CleanupReport, CommandError> {
    context.run_write_scoped(Some(branch_id), |session| {
        let cleanup_ids =
            authenticity::remove_artifact(session.root(), branch_id).map_err(CommandError::from)?;
        cleanup::run(session.root(), &cleanup_ids).map_err(CommandError::from)
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
        cleanup::run(session.root(), &cleanup_ids).map_err(CommandError::from)
    })
}

pub(crate) fn decode(
    context: &CommandContext,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> Result<DecodeResult, CommandError> {
    context.run_read("decode_authenticity", |session| {
        authenticity::pipeline::decode(session.root(), state, request)
            .map_err(|error| CommandError::from_legacy(error.to_string()))
    })
}
