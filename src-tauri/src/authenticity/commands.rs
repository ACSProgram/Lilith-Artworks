//! 认证领域的 Tauri 命令适配器。
//!
//! 这里只保留参数与路径授权校验、读取命令的薄适配和错误转换；发布、导出、估算与
//! 质量预览的业务体在 use-case 与领域模块中。跨领域命令（进入/取消发布、删除记录）
//! 在 `app/workflows.rs`。

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::State;
use tauri_plugin_fs::FsExt;

use crate::{
    app::{
        command_runtime::{CommandError, OperationDomain},
        usecases, OperationCoordinator,
    },
    platform::path::display_path,
    storage,
};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    model::{
        BranchPublication, CertificationRecord, DecodeRequest, DecodeResult, EstimateRequest,
        ExportCertificationRecordRequest, FileSizeEstimate, PreviewImage, PreviewTileRequest,
        PreviewTileSource, PublicationPreview, PublicationPreviewRequest, StageImportRequest,
    },
    preview, publication_repository, rendition, repository,
    state::AuthenticityState,
};

#[tauri::command]
pub(crate) async fn get_branch_publication(
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
    authenticity_state: State<'_, AuthenticityState>,
) -> Result<BranchPublication, String> {
    let models_ready = authenticity_state.model_files_ready();
    let model_info = authenticity_state.model_info();
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("get_branch_publication", |session| {
                repository::get_publication(session.root(), &branch_id, models_ready, model_info)
                    .map_err(CommandError::from)
            })
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn decode_authenticity(
    request: DecodeRequest,
    coordinator: State<'_, OperationCoordinator>,
    authenticity_state: State<'_, AuthenticityState>,
    window: tauri::WebviewWindow,
) -> Result<DecodeResult, AuthenticityError> {
    ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.input_path.trim()),
        "待识别图片",
    )?;
    let state = authenticity_state.inner().clone();
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::decode(context, &state, request)
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) async fn search_certification_records(
    query: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Vec<CertificationRecord>, String> {
    let query = query.trim().to_owned();
    if query.chars().count() > 160 {
        return Err("记录搜索内容不能超过 160 个字符".into());
    }
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("search_certification_records", |session| {
                repository::search_records(session.root(), &query).map_err(CommandError::from)
            })
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn preview_authenticity_image(
    path: String,
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<PreviewImage> {
    ensure_dialog_authorized(&window.fs_scope(), Path::new(path.trim()), "预览图片")?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("preview_authenticity_image", |session| {
                let source = external_image_path(session.root(), &path, "预览图片")
                    .map_err(CommandError::from)?;
                preview::make_preview(source).map_err(CommandError::from)
            })
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

/// 拖放导入的待识别图片：前端读取原始字节后由本命令落到进程临时目录并授权。
///
/// 桌面 WebView 在 Windows 上启用 HTML5 拖放后无法从 `File` 对象取得绝对路径，
/// 因此前端读取字节、这里落盘，再复用既有的外部图片预览与识别流程。落盘目录位于
/// 系统临时目录（仓库之外），路径立即加入文件系统 scope，使后续预览与识别仍走
/// 同一条「路径必须被显式授权」的检查，不新增可绕过的入口。
///
/// 目录的创建、退出删除与崩溃残留清扫见 `drop_stage`。
#[tauri::command]
pub(crate) async fn stage_authenticity_input(
    request: StageImportRequest,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<String> {
    const MAX_DROP_BYTES: usize = 256 * 1024 * 1024;
    let extension = Path::new(request.file_name.trim())
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !rendition::supports_image_extension(&extension) {
        return Err(AuthenticityError::InvalidInput(
            "拖入的图片必须是 PNG、JPEG、WebP 或 TIFF".into(),
        ));
    }
    let bytes = STANDARD
        .decode(request.data_base64.as_bytes())
        .map_err(|_| AuthenticityError::InvalidInput("拖入的图片数据无效".into()))?;
    if bytes.is_empty() {
        return Err(AuthenticityError::InvalidInput("拖入的图片为空".into()));
    }
    if bytes.len() > MAX_DROP_BYTES {
        return Err(AuthenticityError::InvalidInput("拖入的图片过大".into()));
    }
    let path = super::drop_stage::write_staged_image(&bytes, &extension)?;
    window
        .fs_scope()
        .allow_file(&path)
        .map_err(|error| AuthenticityError::Task(format!("无法授权拖入的图片：{error}")))?;
    Ok(display_path(&path))
}

#[tauri::command]
pub(crate) async fn preview_branch_artifact(
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> AuthenticityResult<PreviewImage> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("preview_branch_artifact", |session| {
                let source =
                    publication_repository::publication_target(session.root(), branch_id.trim())
                        .map_err(CommandError::from)?
                        .artifact_path;
                preview::make_preview(PathBuf::from(source)).map_err(CommandError::from)
            })
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) async fn preview_branch_artifact_output(
    request: PublicationPreviewRequest,
    coordinator: State<'_, OperationCoordinator>,
    authenticity_state: State<'_, AuthenticityState>,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<PublicationPreview> {
    ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.config.certificate_path.trim()),
        "证书链",
    )?;
    let state = authenticity_state.inner().clone();
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::preview_output(context, &state, request)
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) fn cancel_authenticity_operation(
    authenticity_state: State<'_, AuthenticityState>,
) -> AuthenticityResult<bool> {
    authenticity_state.request_cancel()
}

/// 高清局部：从缓存的无签名 JPEG、分支最终成品或已授权的外部图片裁剪一块源分辨率
/// 区域，供预览与框选放大镜叠加显示。
///
/// 压缩源的令牌即缓存文件名，因此先做严格格式校验；原始源按分支解析受控成品路径；
/// 外部源与 `decode_authenticity` 使用同一条 `ensure_dialog_authorized` 授权检查，
/// 再由 `preview::tile` 做仓库边界与矩形范围校验。解码复用统一资源预算。
///
/// 命令走仓库共享读租约，不参与认证活动任务锁；前端在发布/签名进行中不发起新的
/// 局部图请求，并保留已加载的局部图。
#[tauri::command]
pub(crate) async fn preview_authenticity_tile(
    request: PreviewTileRequest,
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<PreviewImage> {
    if request.source == PreviewTileSource::External {
        let path = request
            .path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AuthenticityError::InvalidInput("缺少待识别图片路径".into()))?;
        ensure_dialog_authorized(&window.fs_scope(), Path::new(path), "待识别图片")?;
    }
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("preview_authenticity_tile", |session| {
                preview::preview_tile(session.root(), &request).map_err(CommandError::from)
            })
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) async fn preview_certification_record(
    record_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> AuthenticityResult<PreviewImage> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("preview_certification_record", |session| {
                let source = repository::record_source_path(session.root(), record_id.trim())
                    .map_err(CommandError::from)?;
                preview::make_preview(source).map_err(CommandError::from)
            })
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) async fn export_certification_record(
    request: ExportCertificationRecordRequest,
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<()> {
    ensure_dialog_authorized(
        &window.fs_scope(),
        Path::new(request.output_path.trim()),
        "再次导出路径",
    )?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::export_record(context, request)
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

#[tauri::command]
pub(crate) async fn estimate_authenticity_output_size(
    request: EstimateRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> AuthenticityResult<FileSizeEstimate> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            usecases::authenticity::estimate_output(context, request)
        })
        .await
        .map_err(|error| AuthenticityError::Task(error.message))
}

fn external_image_path(root: &Path, value: &str, label: &str) -> AuthenticityResult<PathBuf> {
    let path = PathBuf::from(value.trim());
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput(format!(
            "{label}不存在或不是普通文件"
        )));
    }
    let canonical = path.canonicalize()?;
    storage::ensure_outside_repository(root, &canonical, label)
        .map_err(AuthenticityError::InvalidInput)?;
    Ok(canonical)
}

/// 路径授权来源。
///
/// GUI 侧的实现是 Tauri 的 filesystem scope（`window.fs_scope()`）——它由原生文件
/// 选择器写入，承载「该路径必须被用户显式选择过」这条语义；无头侧的实现是一个只
/// 允许 `--workspace` 之下路径的显式作用域（见 `src/headless.rs`）。
///
/// 实现在 `crate::path_scope`（素材板共用同一条检查），这里只按认证模块的错误类型
/// 转写结果。
pub(crate) use crate::path_scope::PathAuthorization;

pub(crate) fn ensure_dialog_authorized(
    scope: &impl PathAuthorization,
    path: &Path,
    label: &str,
) -> AuthenticityResult<()> {
    crate::path_scope::ensure_authorized(scope, path, label)
        .map_err(AuthenticityError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 显式授权的作用域替身：只有被列出的路径才算已授权，用于验证外部源局部
    /// 与 `decode_authenticity` 共用同一条路径检查。
    struct StubScope(Vec<PathBuf>);

    impl PathAuthorization for StubScope {
        fn is_path_authorized(&self, path: &Path) -> bool {
            self.0.iter().any(|allowed| allowed == path)
        }
    }

    #[test]
    fn external_tile_source_requires_an_authorized_path() {
        let authorized = PathBuf::from("C:/authorized/staged.png");
        let scope = StubScope(vec![authorized.clone()]);

        assert!(ensure_dialog_authorized(&scope, &authorized, "待识别图片").is_ok());
        assert!(ensure_dialog_authorized(
            &scope,
            Path::new("C:/elsewhere/secret.png"),
            "待识别图片",
        )
        .is_err());
        // 空路径始终拒绝，不存在「未选择即放行」的分支。
        assert!(ensure_dialog_authorized(&scope, Path::new(""), "待识别图片").is_err());
    }
}
