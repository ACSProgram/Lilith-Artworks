use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, GenericImageView, ImageFormat};
use sha2::Digest;
use tauri::State;
use tempfile::NamedTempFile;

use crate::{
    app::AppState,
    backup::{self, BackupState},
    cleanup, library, storage,
};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    model::{
        BranchPublication, CertificationRecord, DecodeRequest, DecodeResult,
        EnterPublicationRequest, EstimateRequest, ExportCertificationRecordRequest,
        FileSizeEstimate, PreviewImage, PublishBranchRequest, PublishResult,
    },
    pipeline,
    publication_repository::{self, NewFinalArtifact},
    repository,
    state::AuthenticityState,
};

fn root(state: &AppState) -> Result<PathBuf, String> {
    let root = state.repository_path()?.ok_or("尚未配置作品仓库")?;
    library::open_existing(&root)?;
    Ok(root)
}

#[tauri::command]
pub(crate) async fn enter_branch_publication(
    request: EnterPublicationRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    authenticity_state: State<'_, AuthenticityState>,
) -> Result<BranchPublication, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    let models_ready = authenticity_state.model_files_ready();
    let model_info = authenticity_state.model_info();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&request.branch_id), || {
            let (_, history_id) = publication_repository::branch_head(&root, &request.branch_id)?;
            state.report_progress("publish-lock", "正在固化发布检查点", 0, 2);
            backup::ensure_checkpoint(&root, &history_id)?;
            state.report_progress("publish-lock", "正在保存最终成品", 1, 2);
            store_final_artifact(
                &root,
                &request.branch_id,
                &history_id,
                &request.artifact_path,
            )?;
            state.report_progress("publish-lock", "分支已进入发布状态", 2, 2);
            repository::get_publication(&root, &request.branch_id, models_ready, model_info)
        })
    })
    .await
    .map_err(|error| format!("进入发布状态的任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) fn get_branch_publication(
    branch_id: String,
    app_state: State<'_, AppState>,
    authenticity_state: State<'_, AuthenticityState>,
) -> Result<BranchPublication, String> {
    repository::get_publication(
        &root(app_state.inner())?,
        &branch_id,
        authenticity_state.model_files_ready(),
        authenticity_state.model_info(),
    )
}

#[tauri::command]
pub(crate) async fn cancel_branch_publication(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&branch_id), || {
            let cleanup_ids = publication_repository::remove_artifact(&root, &branch_id)?;
            cleanup::run(&root, &cleanup_ids)
        })
    })
    .await
    .map_err(|error| format!("取消发布任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn publish_branch_artifact(
    request: PublishBranchRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    authenticity_state: State<'_, AuthenticityState>,
) -> Result<PublishResult, AuthenticityError> {
    let root = root(app_state.inner()).map_err(AuthenticityError::Task)?;
    let authenticity = authenticity_state.inner().clone();
    let backup = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let branch_id = request.branch_id.clone();
        let published = backup
            .run_exclusive(Some(&branch_id), || {
                pipeline::publish(&root, &authenticity, request).map_err(|error| error.to_string())
            })
            .map_err(AuthenticityError::Task)?;
        Ok(PublishResult {
            record: published.record,
            width: published.width,
            height: published.height,
            watermark_region_count: published.watermark_region_count,
        })
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn decode_authenticity(
    request: DecodeRequest,
    app_state: State<'_, AppState>,
    authenticity_state: State<'_, AuthenticityState>,
) -> Result<DecodeResult, AuthenticityError> {
    let root = root(app_state.inner()).map_err(AuthenticityError::Task)?;
    let state = authenticity_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || pipeline::decode(&root, &state, request))
        .await
        .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

#[tauri::command]
pub(crate) fn search_certification_records(
    query: String,
    app_state: State<'_, AppState>,
) -> Result<Vec<CertificationRecord>, String> {
    let query = query.trim();
    if query.chars().count() > 160 {
        return Err("记录搜索内容不能超过 160 个字符".into());
    }
    if query.is_empty() {
        return Ok(Vec::new());
    }
    repository::search_records(&root(app_state.inner())?, query)
}

#[tauri::command]
pub(crate) async fn preview_authenticity_image(path: String) -> AuthenticityResult<PreviewImage> {
    tauri::async_runtime::spawn_blocking(move || make_preview(PathBuf::from(path)))
        .await
        .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn preview_certification_record(
    record_id: String,
    app_state: State<'_, AppState>,
) -> AuthenticityResult<PreviewImage> {
    let root = root(app_state.inner()).map_err(AuthenticityError::Task)?;
    tauri::async_runtime::spawn_blocking(move || {
        let source = repository::record_source_path(&root, record_id.trim())
            .map_err(AuthenticityError::Task)?;
        make_preview(source)
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn export_certification_record(
    request: ExportCertificationRecordRequest,
    app_state: State<'_, AppState>,
) -> AuthenticityResult<()> {
    let root = root(app_state.inner()).map_err(AuthenticityError::Task)?;
    tauri::async_runtime::spawn_blocking(move || {
        let source = repository::record_source_path(&root, request.record_id.trim())
            .map_err(AuthenticityError::Task)?;
        let destination = PathBuf::from(request.output_path.trim());
        if !destination.is_absolute() {
            return Err(AuthenticityError::InvalidInput(
                "导出路径必须是绝对路径".into(),
            ));
        }
        if destination
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !matches!(value.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
        {
            return Err(AuthenticityError::InvalidInput(
                "认证图片必须导出为 .jpg 或 .jpeg".into(),
            ));
        }
        if destination.exists() {
            return Err(AuthenticityError::InvalidInput(
                "导出目标已存在；请选择新的文件名".into(),
            ));
        }
        let directory = destination
            .parent()
            .ok_or_else(|| AuthenticityError::InvalidInput("导出目录无效".into()))?;
        fs::create_dir_all(directory)?;
        let mut input = File::open(source)?;
        let mut temp = NamedTempFile::new_in(directory)?;
        std::io::copy(&mut input, &mut temp)?;
        temp.as_file().sync_all()?;
        temp.persist_noclobber(&destination)
            .map_err(|error| AuthenticityError::Io(error.error))?;
        Ok(())
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn estimate_authenticity_output_size(
    request: EstimateRequest,
) -> AuthenticityResult<FileSizeEstimate> {
    tauri::async_runtime::spawn_blocking(move || {
        if !(1..=100).contains(&request.jpeg_quality) {
            return Err(AuthenticityError::InvalidInput(
                "JPEG 质量必须在 1 到 100 之间".into(),
            ));
        }
        let path = PathBuf::from(request.input_path);
        let source_bytes = fs::metadata(&path)?.len();
        let source = image::open(path)?;
        let flattened = super::trustmark::flatten_to_rgb(
            &source,
            super::trustmark::parse_background(&request.background_color)?,
        );
        let mut output = Vec::new();
        JpegEncoder::new_with_quality(&mut output, request.jpeg_quality)
            .encode_image(&flattened)?;
        Ok(FileSizeEstimate {
            jpeg_bytes: output.len() as u64,
            source_bytes,
        })
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}

fn make_preview(path: PathBuf) -> AuthenticityResult<PreviewImage> {
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput("预览图片不存在".into()));
    }
    let source = image::open(&path)?;
    let (width, height) = source.dimensions();
    let preview = source.thumbnail(1600, 1600);
    let mut encoded = Cursor::new(Vec::new());
    preview.write_to(&mut encoded, ImageFormat::Png)?;
    Ok(PreviewImage {
        data_url: format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        ),
        width,
        height,
        source_bytes: fs::metadata(path)?.len(),
    })
}

fn store_final_artifact(
    root: &Path,
    branch_id: &str,
    history_id: &str,
    source_value: &str,
) -> Result<(), String> {
    let source = Path::new(source_value.trim());
    if !source.is_file() {
        return Err("最终成品不存在或不是普通文件".into());
    }
    let source = source
        .canonicalize()
        .map_err(|error| format!("无法访问最终成品：{error}"))?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        _ => return Err("最终成品仅支持 PNG、JPEG、WebP 或 TIFF".into()),
    };
    let artifact_id = storage::new_id();
    let artifact_directory = root
        .join("artworks")
        .join(publication_repository::branch_head(root, branch_id)?.0)
        .join("artifacts")
        .join(branch_id);
    fs::create_dir_all(&artifact_directory)
        .map_err(|error| format!("无法创建成品目录：{error}"))?;
    let final_path = artifact_directory.join(format!("{artifact_id}.{extension}"));
    let mut source_file =
        File::open(&source).map_err(|error| format!("无法读取最终成品：{error}"))?;
    let mut temp = NamedTempFile::new_in(&artifact_directory)
        .map_err(|error| format!("无法创建成品临时文件：{error}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| format!("无法读取最终成品：{error}"))?;
        if read == 0 {
            break;
        }
        temp.write_all(&buffer[..read])
            .map_err(|error| format!("无法写入最终成品：{error}"))?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("无法同步最终成品：{error}"))?;
    let source_sha256 = hex::encode_upper(hasher.finalize());
    let stored_path = storage::relative_path(root, &final_path)?;
    let created_ms = storage::now_ms()?;
    let cleanup_id = {
        let mut connection = storage::open(root)?;
        let transaction = connection.transaction().map_err(storage::database_error)?;
        let id = cleanup::enqueue_repository_file_with_hash(
            &transaction,
            &stored_path,
            &source_sha256,
            "enter_branch_publication",
        )?;
        transaction.commit().map_err(storage::database_error)?;
        id
    };
    if let Err(error) = temp.persist_noclobber(&final_path) {
        let message = match cleanup::discard(root, std::slice::from_ref(&cleanup_id)) {
            Ok(()) => format!("无法发布最终成品：{}", error.error),
            Err(cleanup_error) => format!(
                "无法发布最终成品：{}；无法撤销清理登记：{}",
                error.error, cleanup_error
            ),
        };
        return Err(message);
    }
    let artifact = NewFinalArtifact {
        id: &artifact_id,
        branch_id,
        history_id,
        source_path: &stored_path,
        source_sha256: &source_sha256,
        media_type,
        byte_size: size,
        created_ms,
    };
    if let Err(error) = publication_repository::insert_final_artifact(root, &artifact, &cleanup_id)
    {
        return Err(recover_failed_artifact(root, &cleanup_id, error));
    }
    Ok(())
}

fn recover_failed_artifact(root: &Path, cleanup_id: &str, error: String) -> String {
    match cleanup::run(root, &[cleanup_id.to_owned()]) {
        Ok(report) if report.failures.is_empty() => error,
        Ok(report) => format!(
            "{}；有 {} 个最终成品文件清理失败，将在下次启动时重试",
            error,
            report.failures.len()
        ),
        Err(cleanup_error) => format!("{}；最终成品清理任务失败：{}", error, cleanup_error),
    }
}
