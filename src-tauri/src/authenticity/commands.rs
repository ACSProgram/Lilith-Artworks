use std::{
    fs::{self, File},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, GenericImageView, ImageFormat};
use tauri::State;
use tauri_plugin_fs::FsExt;
use tempfile::NamedTempFile;

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
    image_resource,
    model::{
        BranchPublication, CertificationRecord, DecodeRequest, DecodeResult, EstimateRequest,
        ExportCertificationRecordRequest, FileSizeEstimate, PreviewImage, PublicationPreview,
        PublicationPreviewRequest, StageImportRequest,
    },
    pipeline, publication_repository, repository,
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
                    .map_err(|error| CommandError::from_legacy(error.to_string()))?;
                make_preview(source).map_err(|error| CommandError::from_legacy(error.to_string()))
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
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff"
    ) {
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
    let directory = drop_stage_directory()?;
    let path = directory.join(format!("{}.{}", storage::new_id(), extension));
    fs::write(&path, &bytes)?;
    window
        .fs_scope()
        .allow_file(&path)
        .map_err(|error| AuthenticityError::Task(format!("无法授权拖入的图片：{error}")))?;
    Ok(display_path(&path))
}

static DROP_STAGE_SESSION: OnceLock<String> = OnceLock::new();

fn drop_stage_directory() -> AuthenticityResult<PathBuf> {
    let session = DROP_STAGE_SESSION.get_or_init(storage::new_id);
    let directory = std::env::temp_dir().join(format!("lilith-artworks-drop-{session}"));
    fs::create_dir_all(&directory)?;
    Ok(directory)
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
                        .map_err(|error| CommandError::from_legacy(error.to_string()))?
                        .artifact_path;
                make_preview(PathBuf::from(source))
                    .map_err(|error| CommandError::from_legacy(error.to_string()))
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
    let operation = state.begin_operation("发布质量预览")?;
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Authenticity, move |context| {
            context.run_read("preview_branch_artifact_output", |session| {
                pipeline::preview(session.root(), &state, &operation, request)
                    .map_err(|error| CommandError::from_legacy(error.to_string()))
            })
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
/// 再由流水线做仓库边界与矩形范围校验。解码复用统一资源预算。
/// 不参与认证活动任务锁——预览对话框只在空闲时请求局部图，与发布流程并发时
/// 由前端丢弃过期响应。
#[tauri::command]
pub(crate) async fn preview_authenticity_tile(
    request: super::model::PreviewTileRequest,
    coordinator: State<'_, OperationCoordinator>,
    window: tauri::WebviewWindow,
) -> AuthenticityResult<PreviewImage> {
    if request.source == super::model::PreviewTileSource::External {
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
                pipeline::preview_tile(session.root(), &request)
                    .map_err(|error| CommandError::from_legacy(error.to_string()))
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
                    .map_err(|error| CommandError::from_legacy(error.to_string()))?;
                make_preview(source).map_err(|error| CommandError::from_legacy(error.to_string()))
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
            context.run_read("export_certification_record", |session| {
                let root = session.root();
                let source = repository::record_source_path(root, request.record_id.trim())
                    .map_err(CommandError::from)?;
                let destination = PathBuf::from(request.output_path.trim());
                if !destination.is_absolute() {
                    return Err(CommandError::from_legacy("导出路径必须是绝对路径"));
                }
                if destination
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_none_or(|value| {
                        !matches!(value.to_ascii_lowercase().as_str(), "jpg" | "jpeg")
                    })
                {
                    return Err(CommandError::from_legacy(
                        "认证图片必须导出为 .jpg 或 .jpeg",
                    ));
                }
                if destination.exists() {
                    return Err(CommandError::from_legacy(
                        "导出目标已存在；请选择新的文件名",
                    ));
                }
                storage::ensure_outside_repository(root, &destination, "再次导出路径")
                    .map_err(CommandError::from)?;
                let directory = destination
                    .parent()
                    .ok_or_else(|| CommandError::from_legacy("导出目录无效"))?;
                fs::create_dir_all(directory).map_err(|error| {
                    CommandError::from_legacy(format!("无法创建导出目录：{error}"))
                })?;
                let mut input = File::open(source).map_err(|error| {
                    CommandError::from_legacy(format!("无法读取认证图片：{error}"))
                })?;
                let mut temp = NamedTempFile::new_in(directory).map_err(|error| {
                    CommandError::from_legacy(format!("无法创建导出临时文件：{error}"))
                })?;
                std::io::copy(&mut input, &mut temp).map_err(|error| {
                    CommandError::from_legacy(format!("无法写出认证图片：{error}"))
                })?;
                temp.as_file().sync_all().map_err(|error| {
                    CommandError::from_legacy(format!("无法同步认证图片：{error}"))
                })?;
                temp.persist_noclobber(&destination).map_err(|error| {
                    CommandError::from_legacy(format!("无法发布认证图片：{}", error.error))
                })?;
                Ok(())
            })
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
            context.run_read("estimate_authenticity_output_size", |session| {
                if !(1..=100).contains(&request.jpeg_quality) {
                    return Err(CommandError::from_legacy("JPEG 质量必须在 1 到 100 之间"));
                }
                let path = publication_repository::publication_target(
                    session.root(),
                    request.branch_id.trim(),
                )
                .map_err(|error| CommandError::from_legacy(error.to_string()))?
                .artifact_path;
                let source_bytes = fs::metadata(&path)
                    .map_err(|error| CommandError::from_legacy(error.to_string()))?
                    .len();
                let source = image_resource::open(Path::new(&path))
                    .map_err(|error| CommandError::from_legacy(error.to_string()))?;
                let flattened = super::trustmark::flatten_to_rgb(
                    &source,
                    super::trustmark::parse_background(&request.background_color)
                        .map_err(|error| CommandError::from_legacy(error.to_string()))?,
                );
                drop(source);
                let mut output = CountingWriter::default();
                JpegEncoder::new_with_quality(&mut output, request.jpeg_quality)
                    .encode_image(&flattened)
                    .map_err(|error| CommandError::from_legacy(error.to_string()))?;
                Ok(FileSizeEstimate {
                    jpeg_bytes: output.bytes,
                    source_bytes,
                })
            })
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

fn make_preview(path: PathBuf) -> AuthenticityResult<PreviewImage> {
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput("预览图片不存在".into()));
    }
    let source = image_resource::open(&path)?;
    let (width, height) = source.dimensions();
    let preview = source.thumbnail(1600, 1600);
    drop(source);
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

#[derive(Default)]
struct CountingWriter {
    bytes: u64,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
