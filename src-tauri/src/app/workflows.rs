use std::path::Path;

use tauri::{State, WebviewWindow};
use tauri_plugin_fs::FsExt;

use crate::{
    app::{command_runtime::OperationDomain, usecases, OperationCoordinator},
    authenticity::{
        self, AuthenticityError, AuthenticityState, BranchPublication, EnterPublicationRequest,
        PublishBranchRequest, PublishResult,
    },
    backup, history, library,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryScrubReport {
    pub(crate) history_nodes: u64,
    pub(crate) final_artifacts: u64,
    pub(crate) certification_records: u64,
    /// 检查的画板位图记录数；文字条目没有实体文件，不计入。
    pub(crate) pin_board_bitmaps: u64,
    /// 记录存在但 DDS 缺失。
    pub(crate) pin_board_missing_dds: u64,
    /// DDS 存在但校验失败（路径归属、头、尺寸、长度或解码）。
    pub(crate) pin_board_corrupt_dds: u64,
    /// 磁盘上存在但无记录的孤儿 DDS。
    pub(crate) pin_board_orphan_dds: u64,
}

#[tauri::command]
pub(crate) async fn scrub_repository_integrity(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<RepositoryScrubReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Maintenance, |context| {
            usecases::backup::scrub_repository(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn create_repository_backup(
    destination_parent: String,
    coordinator: State<'_, OperationCoordinator>,
    window: WebviewWindow,
) -> Result<backup::RepositoryBackupReport, String> {
    let destination_parent = Path::new(destination_parent.trim()).to_path_buf();
    authenticity::ensure_dialog_authorized(&window.fs_scope(), &destination_parent, "备份保存目录")
        .map_err(|error| error.to_string())?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Maintenance, move |context| {
            usecases::backup::create_repository_backup(context, &destination_parent)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn acknowledge_backup_disable_notices(
    artwork_ids: Vec<String>,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<(), String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::backup::acknowledge_notices(context, &artwork_ids)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn get_backup_disable_notice_target(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Option<history::BackupDisableNoticeTarget>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, |context| {
            usecases::backup::notice_target(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn create_library_artwork(
    request: library::CreateArtworkRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<library::LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::create_artwork(context, request)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn permanently_delete_library_trash(
    ids: Vec<String>,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<crate::cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::permanently_delete_trash(context, ids)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn empty_library_trash(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<crate::cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, |context| {
            usecases::library::empty_trash(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn fork_artwork_branch(
    request: history::ForkBranchRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<history::ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::fork_branch(context, request)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn update_artwork_branch(
    request: history::UpdateBranchBackupRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<history::ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::update_branch(context, request)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn delete_artwork_branch(
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<history::ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::delete_branch(context, &branch_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn reset_artwork_history(
    artwork_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<history::ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::reset_artwork_history(context, &artwork_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn enter_branch_publication(
    request: EnterPublicationRequest,
    coordinator: State<'_, OperationCoordinator>,
    authenticity_state: State<'_, AuthenticityState>,
    window: WebviewWindow,
) -> Result<BranchPublication, String> {
    authenticity::ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.artifact_path.trim()),
        "最终成品",
    )
    .map_err(|error| error.to_string())?;
    let coordinator = coordinator.inner().clone();
    let state = authenticity_state.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::enter_publication(context, &state, request)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn cancel_branch_publication(
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<crate::cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::cancel_publication(context, &branch_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn delete_certification_record(
    record_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<crate::cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::delete_certification_record(context, &record_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn publish_branch_artifact(
    request: PublishBranchRequest,
    coordinator: State<'_, OperationCoordinator>,
    authenticity_state: State<'_, AuthenticityState>,
    window: WebviewWindow,
) -> Result<PublishResult, AuthenticityError> {
    authenticity::ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.output_path.trim()),
        "发布输出路径",
    )?;
    authenticity::ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.config.certificate_path.trim()),
        "证书链",
    )?;
    let coordinator = coordinator.inner().clone();
    let state = authenticity_state.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::publish(context, &state, request)
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}
