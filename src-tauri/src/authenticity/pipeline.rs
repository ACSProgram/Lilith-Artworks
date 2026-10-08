use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, GenericImageView};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{tempdir_in, NamedTempFile};
use zeroize::Zeroizing;

use crate::{cleanup, storage};

use super::{
    c2pa,
    error::{AuthenticityError, AuthenticityResult},
    image_resource,
    model::{
        CertificationRecord, DecodeRequest, DecodeResult, PreviewImage, PreviewTileSource,
        PublicationPreview, PublicationPreviewRequest, PublishBranchRequest,
    },
    publication_repository,
    repository::{self, NewCertificationRecord},
    state::{AuthenticityOperation, AuthenticityState},
    trustmark,
};

pub(crate) struct PublishedOutput {
    pub(crate) record: CertificationRecord,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) watermark_region_count: u32,
    pub(crate) rendition_cache_hit: bool,
    pub(crate) render_ms: u64,
    pub(crate) encode_ms: u64,
    pub(crate) signing_ms: u64,
}

const PUBLICATION_PREVIEW_EDGE: u32 = 2400;
static PREVIEW_CACHE_SESSION: OnceLock<String> = OnceLock::new();

#[derive(Deserialize, Serialize)]
struct RenditionCacheMetadata {
    token: String,
    source_sha256: String,
    jpeg_sha256: String,
    watermark_id: Option<String>,
    width: u32,
    height: u32,
    output_bytes: u64,
}

struct CachedRendition {
    path: PathBuf,
    width: u32,
    height: u32,
    output_bytes: u64,
    cache_hit: bool,
    render_ms: u64,
    encode_ms: u64,
}

pub(crate) fn preview(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    mut request: PublicationPreviewRequest,
) -> AuthenticityResult<PublicationPreview> {
    ensure_not_cancelled(operation)?;
    request.config.branch_id = request.branch_id.clone();
    request.config.trustmark_enabled =
        request.config.trustmark_enabled && !request.config.additional_regions.is_empty();
    validate_publication_config(&request.config)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let identifier = request
        .config
        .trustmark_enabled
        .then(|| trustmark::resolve_identifier(request.watermark_id.as_deref()))
        .transpose()?;
    let cache_token = rendition_cache_token(
        &target.source_sha256,
        &request.config,
        identifier.as_deref(),
    )?;
    let source = image_resource::open(&input)?;
    ensure_not_cancelled(operation)?;
    let (width, height) = source.dimensions();
    let source_bytes = fs::metadata(&input)?.len();
    let original_image = png_thumbnail_preview(&source, source_bytes)?;
    ensure_not_cancelled(operation)?;
    let cached = if let Some(cached) = load_cached_rendition(
        root,
        operation,
        &cache_token,
        &target.source_sha256,
        identifier.as_deref(),
    )? {
        drop(source);
        cached
    } else {
        render_cached_rendition(
            root,
            state,
            operation,
            source,
            &request.config,
            identifier.as_deref(),
            &cache_token,
            &target.source_sha256,
        )?
    };
    ensure_not_cancelled(operation)?;
    let compressed = image_resource::open(&cached.path)?;
    let image = jpeg_thumbnail_preview(&compressed, cached.output_bytes)?;
    ensure_not_cancelled(operation)?;
    Ok(PublicationPreview {
        branch_id: request.branch_id.clone(),
        image,
        original_image,
        source_width: width,
        source_height: height,
        output_bytes: cached.output_bytes,
        watermark_id: identifier,
        cache_token,
        cache_hit: cached.cache_hit,
        render_ms: cached.render_ms,
        encode_ms: cached.encode_ms,
    })
}

fn rendition_cache_token(
    source_sha256: &str,
    config: &super::model::CertificationConfig,
    watermark_id: Option<&str>,
) -> AuthenticityResult<String> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "sourceSha256": source_sha256,
        "branchId": config.branch_id,
        "trustmarkEnabled": config.trustmark_enabled,
        "jpegQuality": config.jpeg_quality,
        "backgroundColor": config.background_color,
        "watermarkStrength": config.watermark_strength,
        "additionalRegions": config.additional_regions,
        "watermarkId": watermark_id,
    }))?;
    Ok(hex::encode_upper(Sha256::digest(payload)))
}

fn preview_cache_directory(root: &Path) -> AuthenticityResult<PathBuf> {
    let session = PREVIEW_CACHE_SESSION.get_or_init(storage::new_id);
    let directory = root
        .join("temp")
        .join(format!("authenticity-preview-{session}"));
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn cache_paths(root: &Path, token: &str) -> AuthenticityResult<(PathBuf, PathBuf)> {
    let directory = preview_cache_directory(root)?;
    Ok((
        directory.join(format!("{token}.jpg")),
        directory.join(format!("{token}.json")),
    ))
}

fn load_cached_rendition(
    root: &Path,
    operation: &AuthenticityOperation,
    token: &str,
    source_sha256: &str,
    watermark_id: Option<&str>,
) -> AuthenticityResult<Option<CachedRendition>> {
    let (jpeg_path, metadata_path) = cache_paths(root, token)?;
    if !jpeg_path.is_file() || !metadata_path.is_file() {
        return Ok(None);
    }
    let loaded = (|| -> AuthenticityResult<CachedRendition> {
        ensure_not_cancelled(operation)?;
        let metadata: RenditionCacheMetadata =
            serde_json::from_reader(File::open(&metadata_path)?)?;
        if metadata.token != token
            || !metadata.source_sha256.eq_ignore_ascii_case(source_sha256)
            || metadata.watermark_id.as_deref() != watermark_id
            || metadata.output_bytes != fs::metadata(&jpeg_path)?.len()
            || !sha256_file_cancelable(&jpeg_path, operation)?
                .eq_ignore_ascii_case(&metadata.jpeg_sha256)
        {
            return Err(AuthenticityError::Task("发布预览缓存校验失败".into()));
        }
        let dimensions = image::ImageReader::open(&jpeg_path)?
            .with_guessed_format()?
            .into_dimensions()?;
        if dimensions != (metadata.width, metadata.height) {
            return Err(AuthenticityError::Task("发布预览缓存尺寸不匹配".into()));
        }
        Ok(CachedRendition {
            path: jpeg_path.clone(),
            width: metadata.width,
            height: metadata.height,
            output_bytes: metadata.output_bytes,
            cache_hit: true,
            render_ms: 0,
            encode_ms: 0,
        })
    })();
    match loaded {
        Ok(cached) => Ok(Some(cached)),
        Err(_) => {
            let _ = fs::remove_file(jpeg_path);
            let _ = fs::remove_file(metadata_path);
            Ok(None)
        }
    }
}

fn render_cached_rendition(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    source: image::DynamicImage,
    config: &super::model::CertificationConfig,
    watermark_id: Option<&str>,
    token: &str,
    source_sha256: &str,
) -> AuthenticityResult<CachedRendition> {
    ensure_not_cancelled(operation)?;
    let (width, height) = source.dimensions();
    let render_started = Instant::now();
    let background = trustmark::parse_background(&config.background_color)?;
    let flattened = trustmark::flatten_to_rgb(&source, background);
    drop(source);
    ensure_not_cancelled(operation)?;
    let rendition = if let Some(identifier) = watermark_id {
        trustmark::encode_regions(
            state,
            flattened,
            identifier,
            config.watermark_strength,
            &config.additional_regions,
        )?
    } else {
        flattened
    };
    auth_checkpoint(operation, "认证编码")?;
    let render_ms = elapsed_ms(render_started);
    let encode_started = Instant::now();
    let (jpeg_path, metadata_path) = cache_paths(root, token)?;
    let directory = jpeg_path
        .parent()
        .ok_or_else(|| AuthenticityError::Task("发布预览缓存目录无效".into()))?;
    let mut encoded = NamedTempFile::new_in(directory)?;
    JpegEncoder::new_with_quality(encoded.as_file_mut(), config.jpeg_quality)
        .encode_image(&rendition)?;
    drop(rendition);
    ensure_not_cancelled(operation)?;
    encoded.as_file_mut().flush()?;
    encoded.as_file().sync_all()?;
    let output_bytes = encoded.as_file().metadata()?.len();
    let jpeg_sha256 = sha256_file_cancelable(encoded.path(), operation)?;
    ensure_not_cancelled(operation)?;
    replace_cache_file(encoded, &jpeg_path)?;
    let metadata = RenditionCacheMetadata {
        token: token.to_owned(),
        source_sha256: source_sha256.to_owned(),
        jpeg_sha256,
        watermark_id: watermark_id.map(str::to_owned),
        width,
        height,
        output_bytes,
    };
    let mut metadata_temp = NamedTempFile::new_in(directory)?;
    serde_json::to_writer(metadata_temp.as_file_mut(), &metadata)?;
    metadata_temp.as_file_mut().flush()?;
    metadata_temp.as_file().sync_all()?;
    replace_cache_file(metadata_temp, &metadata_path)?;
    Ok(CachedRendition {
        path: jpeg_path,
        width,
        height,
        output_bytes,
        cache_hit: false,
        render_ms,
        encode_ms: elapsed_ms(encode_started),
    })
}

fn replace_cache_file(temp: NamedTempFile, destination: &Path) -> AuthenticityResult<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    temp.persist(destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn png_thumbnail_preview(
    source: &image::DynamicImage,
    source_bytes: u64,
) -> AuthenticityResult<PreviewImage> {
    let preview = source.thumbnail(PUBLICATION_PREVIEW_EDGE, PUBLICATION_PREVIEW_EDGE);
    let (width, height) = preview.dimensions();
    let mut encoded = Cursor::new(Vec::new());
    preview.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(PreviewImage {
        data_url: format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        ),
        width,
        height,
        source_bytes,
    })
}

fn jpeg_thumbnail_preview(
    source: &image::DynamicImage,
    source_bytes: u64,
) -> AuthenticityResult<PreviewImage> {
    let preview = source.thumbnail(PUBLICATION_PREVIEW_EDGE, PUBLICATION_PREVIEW_EDGE);
    let (width, height) = preview.dimensions();
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, 92).encode_image(&preview)?;
    Ok(PreviewImage {
        data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(encoded)),
        width,
        height,
        source_bytes,
    })
}

/// 高清局部（tile）允许的最大输出边长。
///
/// 局部要支撑「一源像素对一屏幕像素」的 1:1 观察，输出边长必须能覆盖可视区域
/// 在屏幕上的显示尺寸（含边距），因此不再与 2400 px 缩略图上限对齐。
pub(crate) const TILE_MAX_EDGE: u32 = 4096;
pub(crate) const TILE_MIN_EDGE: u32 = 64;

/// 单个解码源图像进入常驻缓存的像素上限（约 64 MP，约 192 MB RGB）。
const TILE_SOURCE_CACHE_MAX_PIXELS: u64 = 64 * 1024 * 1024;

struct CachedTileSource {
    key: String,
    image: std::sync::Arc<image::DynamicImage>,
}

static TILE_SOURCE_CACHE: OnceLock<std::sync::Mutex<Option<CachedTileSource>>> = OnceLock::new();

fn tile_source_cache() -> &'static std::sync::Mutex<Option<CachedTileSource>> {
    TILE_SOURCE_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// 打开高清局部的像素来源，并缓存最近一次解码结果，避免平移时反复解码整图。
///
/// 缓存以「路径 + 文件长度」为键：预览缓存与分支成品在会话内都不可变，因此键
/// 变化即意味着源已更换。超过像素上限的源不缓存，仍按次解码。
fn open_tile_source(path: &Path) -> AuthenticityResult<std::sync::Arc<image::DynamicImage>> {
    let key = format!("{}:{}", path.display(), fs::metadata(path)?.len());
    let cache = tile_source_cache();
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.as_ref() {
            if cached.key == key {
                return Ok(std::sync::Arc::clone(&cached.image));
            }
        }
    }
    let image = image_resource::open(path)?;
    let pixels = u64::from(image.width()) * u64::from(image.height());
    let shared = std::sync::Arc::new(image);
    if pixels <= TILE_SOURCE_CACHE_MAX_PIXELS {
        if let Ok(mut guard) = cache.lock() {
            *guard = Some(CachedTileSource {
                key,
                image: std::sync::Arc::clone(&shared),
            });
        }
    }
    Ok(shared)
}

/// 从质量预览缓存的无签名 JPEG 或分支最终成品中裁剪一块高清区域。
///
/// 压缩源要求令牌完整命中本进程会话的缓存文件名，并按缓存元数据的像素尺寸
/// 校验；原始源按分支解析受控成品路径。裁剪矩形按源像素尺寸校验，解码复用
/// 统一的资源预算，输出按 `max_edge` 只缩不放，供界面在缩略图分辨率不足时
/// 叠加显示。
///
/// 局部统一编码为无损 PNG：预览的用途就是观察 JPEG 与 TrustMark 造成的损失，
/// 若再按有损 JPEG 回写一次，界面会额外叠加一层并非成品本身带来的压缩痕迹。
pub(crate) fn preview_tile(
    root: &Path,
    request: &super::model::PreviewTileRequest,
) -> AuthenticityResult<PreviewImage> {
    if !(TILE_MIN_EDGE..=TILE_MAX_EDGE).contains(&request.max_edge) {
        return Err(AuthenticityError::InvalidInput(
            "预览局部尺寸超出允许范围".into(),
        ));
    }
    let (image_path, bounds) = match request.source {
        PreviewTileSource::Compressed => {
            let token = request
                .cache_token
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(AuthenticityError::InvalidInput("预览缓存令牌无效".into()));
            }
            let (jpeg_path, metadata_path) = cache_paths(root, token)?;
            if !jpeg_path.is_file() || !metadata_path.is_file() {
                return Err(AuthenticityError::Task(
                    "质量预览缓存已失效，请重新生成预览".into(),
                ));
            }
            let metadata: RenditionCacheMetadata =
                serde_json::from_reader(File::open(&metadata_path)?)?;
            if metadata.token != token {
                return Err(AuthenticityError::Task("发布预览缓存校验失败".into()));
            }
            (jpeg_path, (metadata.width, metadata.height))
        }
        PreviewTileSource::Original => {
            let branch_id = request
                .branch_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if branch_id.is_empty() {
                return Err(AuthenticityError::InvalidInput("缺少分支标识".into()));
            }
            let artifact = PathBuf::from(
                publication_repository::publication_target(root, branch_id)
                    .map_err(AuthenticityError::Task)?
                    .artifact_path,
            );
            if !artifact.is_file() {
                return Err(AuthenticityError::Task("最终成品不可用".into()));
            }
            let (width, height) = image::ImageReader::open(&artifact)?
                .with_guessed_format()?
                .into_dimensions()?;
            (artifact, (width, height))
        }
    };
    let right = request
        .x
        .checked_add(request.width)
        .ok_or_else(|| AuthenticityError::InvalidInput("预览局部矩形无效".into()))?;
    let bottom = request
        .y
        .checked_add(request.height)
        .ok_or_else(|| AuthenticityError::InvalidInput("预览局部矩形无效".into()))?;
    if request.width == 0 || request.height == 0 || right > bounds.0 || bottom > bounds.1 {
        return Err(AuthenticityError::InvalidInput(
            "预览局部矩形超出源图片范围".into(),
        ));
    }
    let source = open_tile_source(&image_path)?;
    if source.dimensions() != bounds {
        return Err(AuthenticityError::Task("预览源尺寸不匹配".into()));
    }
    let crop = source.crop_imm(request.x, request.y, request.width, request.height);
    drop(source);
    // 只缩不放：局部边长已小于请求上限时保留原生分辨率。
    let tile = if crop.width().max(crop.height()) > request.max_edge {
        crop.thumbnail(request.max_edge, request.max_edge)
    } else {
        crop
    };
    let (width, height) = tile.dimensions();
    let mut encoded = Cursor::new(Vec::new());
    tile.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(PreviewImage {
        data_url: format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        ),
        width,
        height,
        source_bytes: 0,
    })
}

pub(crate) fn publish(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    mut request: PublishBranchRequest,
) -> AuthenticityResult<PublishedOutput> {
    ensure_not_cancelled(operation)?;
    let private_key = Zeroizing::new(std::mem::take(&mut request.private_key_pem));
    request.config.title = request.config.title.trim().to_owned();
    request.config.creator = request.config.creator.trim().to_owned();
    request.config.rights_statement = request.config.rights_statement.trim().to_owned();
    request.config.authentication_content = request.config.authentication_content.trim().to_owned();
    request.config.trustmark_enabled =
        request.config.trustmark_enabled && !request.config.additional_regions.is_empty();
    validate_publish_request(&request, &private_key)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let output = absolute_output_path(&request.output_path)?;
    storage::ensure_outside_repository(root, &output, "发布输出路径")
        .map_err(AuthenticityError::InvalidInput)?;
    if output.exists() {
        return Err(AuthenticityError::InvalidInput(
            "输出文件已存在；请选择新的文件名".into(),
        ));
    }
    if input == output {
        return Err(AuthenticityError::InvalidInput(
            "输出路径不能覆盖最终成品".into(),
        ));
    }
    if output
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !matches!(value.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
    {
        return Err(AuthenticityError::InvalidInput(
            "发布版输出必须使用 .jpg 或 .jpeg 扩展名".into(),
        ));
    }

    request.config.branch_id = request.branch_id.clone();
    let parent = output
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    fs::create_dir_all(parent)?;
    let temp_dir = tempdir_in(parent)?;
    let signed_path = temp_dir.path().join("signed.jpg");
    let rendition_identifier = request
        .config
        .trustmark_enabled
        .then(|| trustmark::resolve_identifier(request.watermark_id.as_deref()))
        .transpose()?;
    let cache_token = rendition_cache_token(
        &target.source_sha256,
        &request.config,
        rendition_identifier.as_deref(),
    )?;
    auth_checkpoint(operation, "认证渲染")?;
    let cached = if request.preview_cache_token.as_deref() == Some(cache_token.as_str()) {
        load_cached_rendition(
            root,
            operation,
            &cache_token,
            &target.source_sha256,
            rendition_identifier.as_deref(),
        )?
    } else {
        None
    };
    let cached = if let Some(cached) = cached {
        cached
    } else {
        let source = image_resource::open(&input)?;
        render_cached_rendition(
            root,
            state,
            operation,
            source,
            &request.config,
            rendition_identifier.as_deref(),
            &cache_token,
            &target.source_sha256,
        )?
    };
    auth_checkpoint(operation, "认证签名")?;
    let identifier = match rendition_identifier {
        Some(identifier) => identifier,
        None => trustmark::resolve_identifier(None)?,
    };

    let record_id = storage::new_id();
    let signing_started = Instant::now();
    c2pa::sign_jpeg(
        &request.config,
        private_key.as_bytes(),
        &record_id,
        &identifier,
        &input,
        &cached.path,
        &signed_path,
        operation.cancellation_flag(),
    )?;
    ensure_not_cancelled(operation)?;
    let signing_ms = elapsed_ms(signing_started);
    let manifest = c2pa::read_manifest(&signed_path)?;
    ensure_not_cancelled(operation)?;
    validate_signed_manifest(&manifest, &request.config, &record_id, &identifier)?;
    let output_path = storage::display_path(&output);
    let output_sha256 = sha256_file_cancelable(&signed_path, operation)?;
    let output_bytes = fs::metadata(&signed_path)?.len();
    let created_ms = storage::now_ms().map_err(AuthenticityError::Task)?;
    let stored_destination =
        repository::certification_storage_path(root, &request.branch_id, &record_id)
            .map_err(AuthenticityError::Task)?;
    let stored_relative =
        storage::relative_path(root, &stored_destination).map_err(AuthenticityError::Task)?;
    ensure_not_cancelled(operation)?;
    // After cleanup intents are committed, finish the atomic publish/recovery sequence.
    // Interrupting that sequence would expose a partial-success result to the caller.
    let cleanup_ids = {
        let mut connection = storage::open(root).map_err(AuthenticityError::Task)?;
        let transaction = connection
            .transaction()
            .map_err(|error| AuthenticityError::Task(storage::database_error(error)))?;
        let external_id = cleanup::enqueue_external_file(
            &transaction,
            &output_path,
            &output_sha256,
            "publish_certification",
        )
        .map_err(AuthenticityError::Task)?;
        let stored_id = cleanup::enqueue_repository_file_with_hash(
            &transaction,
            &stored_relative,
            &output_sha256,
            "publish_certification",
        )
        .map_err(AuthenticityError::Task)?;
        transaction
            .commit()
            .map_err(|error| AuthenticityError::Task(storage::database_error(error)))?;
        vec![external_id, stored_id]
    };
    if let Err(error) = publish_noclobber(&signed_path, &output) {
        let message = match cleanup::discard(root, &cleanup_ids) {
            Ok(()) => error.to_string(),
            Err(cleanup_error) => format!("{}；无法撤销清理登记：{}", error, cleanup_error),
        };
        return Err(AuthenticityError::Task(message));
    }
    if let Err(error) = store_certification_copy(&stored_destination, &output) {
        let message = recover_failed_publication(root, &cleanup_ids, error.to_string());
        return Err(AuthenticityError::Task(message));
    }
    let inserted = repository::insert_record(
        root,
        &NewCertificationRecord {
            id: &record_id,
            final_artifact_id: &target.artifact_id,
            branch_id: &request.branch_id,
            history_id: &target.history_id,
            watermark_id: request
                .config
                .trustmark_enabled
                .then_some(identifier.as_str()),
            output_path: &output_path,
            stored_path: &stored_relative,
            output_sha256: &output_sha256,
            output_bytes,
            config: &request.config,
            c2pa_manifest_json: manifest.manifest_json.as_deref(),
            validation_state: manifest.validation_state.as_deref(),
            created_ms,
        },
        &cleanup_ids,
    );
    let record = match inserted {
        Ok(record) => record,
        Err(error) => {
            return Err(AuthenticityError::Task(recover_failed_publication(
                root,
                &cleanup_ids,
                error,
            )));
        }
    };
    Ok(PublishedOutput {
        record,
        width: cached.width,
        height: cached.height,
        watermark_region_count: if request.config.trustmark_enabled {
            request.config.additional_regions.len() as u32
        } else {
            0
        },
        rendition_cache_hit: cached.cache_hit,
        render_ms: cached.render_ms,
        encode_ms: cached.encode_ms,
        signing_ms,
    })
}

fn validate_signed_manifest(
    manifest: &super::model::ManifestSummary,
    config: &super::model::CertificationConfig,
    record_id: &str,
    identifier: &str,
) -> AuthenticityResult<()> {
    if !manifest.present {
        return Err(AuthenticityError::Task(
            "签名完成后未能回读 C2PA 清单".into(),
        ));
    }
    if !manifest.validation_accepted {
        return Err(AuthenticityError::Task(
            "签名后的 C2PA 清单未通过完整性验证".into(),
        ));
    }
    if manifest.record_id.as_deref() != Some(record_id)
        || manifest.title.as_deref() != Some(config.title.trim())
        || manifest.creator.as_deref() != Some(config.creator.trim())
        || manifest.rights_statement.as_deref() != Some(config.rights_statement.trim())
        || manifest.authentication_content.as_deref() != Some(config.authentication_content.trim())
        || (config.trustmark_enabled && manifest.watermark_id.as_deref() != Some(identifier))
        || (!config.trustmark_enabled && manifest.watermark_id.is_some())
    {
        return Err(AuthenticityError::Task(
            "签名后的 C2PA 声明与本次发布参数不匹配".into(),
        ));
    }
    Ok(())
}

fn store_certification_copy(destination: &Path, source: &Path) -> AuthenticityResult<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::Task("认证副本目录无效".into()))?;
    fs::create_dir_all(directory)?;
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

fn publish_noclobber(source: &Path, destination: &Path) -> AuthenticityResult<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

fn recover_failed_publication(root: &Path, cleanup_ids: &[String], error: String) -> String {
    match cleanup::run(root, cleanup_ids) {
        Ok(report) if report.failures.is_empty() => error,
        Ok(report) => format!(
            "{}；有 {} 个发布文件清理失败，将在下次启动时重试",
            error,
            report.failures.len()
        ),
        Err(cleanup_error) => format!("{}；发布文件清理任务失败：{}", error, cleanup_error),
    }
}

pub(crate) fn decode(
    root: &Path,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> AuthenticityResult<DecodeResult> {
    let input = canonical_existing_file(&request.input_path, "待识别图片")?;
    storage::ensure_outside_repository(root, &input, "待识别图片")
        .map_err(AuthenticityError::InvalidInput)?;
    let image = image_resource::open(&input)?;
    let decoded_region = request.region;
    let watermark_id = if state.model_files_ready() {
        trustmark::decode_region(state, &image, decoded_region)?
    } else {
        None
    };
    drop(image);
    let manifest = c2pa::read_manifest(&input)?;
    let identifiers_match = match (&watermark_id, &manifest.watermark_id) {
        (Some(decoded), Some(declared)) => Some(decoded == declared),
        _ => None,
    };
    let mut matches = Vec::new();
    let mut match_indexes = HashMap::new();
    if let Some(identifier) = manifest
        .record_id
        .as_ref()
        .or(manifest.watermark_id.as_ref())
    {
        merge_matches(
            &mut matches,
            &mut match_indexes,
            repository::records_by_identifier(root, identifier).map_err(AuthenticityError::Task)?,
            "c2pa",
        );
    }
    if let Some(identifier) = watermark_id.as_ref() {
        merge_matches(
            &mut matches,
            &mut match_indexes,
            repository::records_by_identifier(root, identifier).map_err(AuthenticityError::Task)?,
            "trustmark",
        );
    }
    Ok(DecodeResult {
        watermark_present: watermark_id.is_some(),
        watermark_id,
        decoded_region,
        c2pa_present: manifest.present,
        c2pa_validation_state: manifest.validation_state,
        c2pa_validation_status: manifest.validation_status,
        c2pa_record_id: manifest.record_id,
        c2pa_watermark_id: manifest.watermark_id,
        identifiers_match,
        title: manifest.title,
        creator: manifest.creator,
        rights_statement: manifest.rights_statement,
        authentication_content: manifest.authentication_content,
        manifest_json: manifest.manifest_json,
        matches,
    })
}

fn merge_matches(
    matches: &mut Vec<super::model::CertificationMatch>,
    indexes: &mut HashMap<String, usize>,
    records: Vec<CertificationRecord>,
    evidence_source: &str,
) {
    for record in records {
        if let Some(index) = indexes.get(&record.id).copied() {
            let sources = &mut matches[index].evidence_sources;
            if !sources.iter().any(|source| source == evidence_source) {
                sources.push(evidence_source.to_owned());
            }
        } else {
            indexes.insert(record.id.clone(), matches.len());
            matches.push(super::model::CertificationMatch {
                record,
                evidence_sources: vec![evidence_source.to_owned()],
            });
        }
    }
}

fn validate_publish_request(
    request: &PublishBranchRequest,
    private_key: &str,
) -> AuthenticityResult<()> {
    if private_key.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("请粘贴 PEM 私钥".into()));
    }
    if request.config.trustmark_enabled && request.config.additional_regions.is_empty() {
        return Err(AuthenticityError::InvalidInput(
            "启用 TrustMark 前请先框选水印区域".into(),
        ));
    }
    validate_publication_config(&request.config)?;
    Ok(())
}

fn validate_publication_config(
    config: &super::model::CertificationConfig,
) -> AuthenticityResult<()> {
    c2pa::supported_signing_algorithm(&config.signing_algorithm)?;
    if config.title.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("作品标题不能为空".into()));
    }
    if config.creator.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("创作者不能为空".into()));
    }
    if !Path::new(&config.certificate_path).is_file() {
        return Err(AuthenticityError::InvalidInput("证书链文件不存在".into()));
    }
    validate_visual_config(config)
}

fn validate_visual_config(config: &super::model::CertificationConfig) -> AuthenticityResult<()> {
    if !(1..=100).contains(&config.jpeg_quality) {
        return Err(AuthenticityError::InvalidInput(
            "JPEG 质量必须在 1 到 100 之间".into(),
        ));
    }
    Ok(())
}

fn canonical_existing_file(value: &str, label: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput(format!(
            "{label}文件不存在"
        )));
    }
    Ok(path.canonicalize()?)
}

fn absolute_output_path(value: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if path.as_os_str().is_empty() {
        return Err(AuthenticityError::InvalidInput("请选择输出路径".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn sha256_file_cancelable(
    path: &Path,
    operation: &AuthenticityOperation,
) -> AuthenticityResult<String> {
    sha256_file_with_check(path, || operation.cancelled())
}

fn sha256_file_with_check(path: &Path, cancelled: impl Fn() -> bool) -> AuthenticityResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(AuthenticityError::Task("认证任务已取消".into()));
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

fn ensure_not_cancelled(operation: &AuthenticityOperation) -> AuthenticityResult<()> {
    if operation.cancelled() {
        Err(AuthenticityError::Task("认证任务已取消".into()))
    } else {
        Ok(())
    }
}

/// 认证流水线的一个取消检查点。
///
/// release 构建下它**逐位等价于** [`ensure_not_cancelled`]；headless 构建下额外经过
/// 无头进程的闸门（`crate::headless::auth_checkpoint`），使外部测试能在确定的位置取消
/// 发布（渲染 / 编码 / 签名）——与 `history::commit` 的事务标记点同一范式：调用点由
/// `feature = "headless"` 门控，发布产物中不存在，GUI 路径行为逐位不变。
#[inline]
fn auth_checkpoint(operation: &AuthenticityOperation, stage: &str) -> AuthenticityResult<()> {
    #[cfg(feature = "headless")]
    if crate::headless::auth_checkpoint(stage) {
        return Err(AuthenticityError::Task("认证任务已取消".into()));
    }
    #[cfg(not(feature = "headless"))]
    let _ = stage;
    ensure_not_cancelled(operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_RECORD_ID: &str = "fixture-record-0001";
    const FIXTURE_WATERMARK_ID: &str = "1010101010101010101010101010101010101010";

    fn fixture_directory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("authenticity")
    }

    fn fixture_state() -> AuthenticityState {
        AuthenticityState::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("models"),
        )
    }

    fn fixture_config() -> super::super::model::CertificationConfig {
        let directory = fixture_directory();
        super::super::model::CertificationConfig {
            branch_id: "fixture-branch".into(),
            title: "Lilith C2PA fixture".into(),
            creator: "Lilith Artworks tests".into(),
            rights_statement: "Regression fixture only".into(),
            authentication_content: "A tiny deterministic TrustMark/C2PA sample".into(),
            trustmark_enabled: true,
            certificate_path: directory
                .join("es256-test.pub")
                .to_string_lossy()
                .into_owned(),
            signing_algorithm: "es256".into(),
            timestamp_url: None,
            jpeg_quality: 90,
            background_color: "#FFFFFF".into(),
            watermark_strength: 1.0,
            additional_regions: vec![super::super::model::NormalizedRegion {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }],
            updated_ms: 0,
        }
    }

    #[test]
    #[ignore = "regenerates checked-in C2PA fixtures"]
    fn regenerate_c2pa_fixtures() {
        let directory = fixture_directory();
        let source_path = directory.join("source.jpg");
        let source = image::open(&source_path).unwrap();
        let flattened = super::super::trustmark::flatten_to_rgb(&source, image::Rgb([255; 3]));
        let state = fixture_state();
        let config = fixture_config();
        let rendition = super::super::trustmark::encode_regions(
            &state,
            flattened,
            FIXTURE_WATERMARK_ID,
            config.watermark_strength,
            &config.additional_regions,
        )
        .unwrap();
        let unsigned = directory.join("unsigned.jpg");
        let mut unsigned_file = File::create(&unsigned).unwrap();
        JpegEncoder::new_with_quality(&mut unsigned_file, config.jpeg_quality)
            .encode_image(&rendition)
            .unwrap();
        drop(unsigned_file);

        let signed = directory.join("valid-trustmark.jpg");
        super::super::c2pa::sign_jpeg(
            &config,
            include_bytes!("../../tests/fixtures/authenticity/es256-test.priv"),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
            &source_path,
            &unsigned,
            &signed,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        fs::remove_file(unsigned).unwrap();

        let mut tampered = fs::read(&signed).unwrap();
        let offset = tampered.len() - 10;
        tampered[offset] ^= 1;
        fs::write(directory.join("tampered-trustmark.jpg"), tampered).unwrap();
    }

    #[test]
    fn real_c2pa_fixture_matches_record_trustmark_and_claims() {
        let fixture = fixture_directory().join("valid-trustmark.jpg");
        let manifest = super::super::c2pa::read_manifest(&fixture).unwrap();
        let decoded = super::super::trustmark::decode_region(
            &fixture_state(),
            &image::open(&fixture).unwrap(),
            None,
        )
        .unwrap();

        validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .unwrap();
        assert_eq!(manifest.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(manifest.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert_eq!(decoded.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert!(manifest.manifest_json.is_some());
    }

    #[test]
    fn tampered_c2pa_fixture_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("tampered-trustmark.jpg"))
                .unwrap();

        assert!(manifest.present);
        assert!(!manifest.validation_accepted);
        assert!(validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }

    #[test]
    fn replacement_without_manifest_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("source.jpg")).unwrap();

        assert!(!manifest.present);
        assert!(validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }

    #[test]
    fn mismatched_expected_claim_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("valid-trustmark.jpg"))
                .unwrap();
        let mut config = fixture_config();
        config.authentication_content = "different expected claim".into();

        assert!(validate_signed_manifest(
            &manifest,
            &config,
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }

    fn record(id: &str) -> CertificationRecord {
        CertificationRecord {
            id: id.into(),
            artwork_id: "artwork".into(),
            artwork_title: "Artwork".into(),
            branch_id: "branch".into(),
            branch_title: "Branch".into(),
            history_id: "history".into(),
            watermark_id: Some("0".repeat(super::trustmark::WATERMARK_BITS)),
            trustmark_enabled: true,
            output_path: "output.jpg".into(),
            output_sha256: "A".repeat(64),
            output_bytes: 1,
            title: "Title".into(),
            creator: "Creator".into(),
            rights_statement: String::new(),
            authentication_content: String::new(),
            additional_regions: Vec::new(),
            c2pa_manifest_label: None,
            c2pa_manifest_json: None,
            validation_state: None,
            created_ms: 1,
        }
    }

    #[test]
    fn merges_candidate_evidence_without_hiding_conflicts() {
        let mut matches = Vec::new();
        let mut indexes = HashMap::new();
        merge_matches(&mut matches, &mut indexes, vec![record("same")], "c2pa");
        merge_matches(
            &mut matches,
            &mut indexes,
            vec![record("same"), record("other")],
            "trustmark",
        );

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].record.id, "same");
        assert_eq!(matches[0].evidence_sources, vec!["c2pa", "trustmark"]);
        assert_eq!(matches[1].record.id, "other");
        assert_eq!(matches[1].evidence_sources, vec!["trustmark"]);
    }

    #[test]
    fn publication_preview_helpers_bound_ipc_image_dimensions() {
        let source = image::DynamicImage::new_rgb8(3000, 100);

        let png = png_thumbnail_preview(&source, 123).unwrap();
        let jpeg = jpeg_thumbnail_preview(&source, 456).unwrap();

        assert_eq!((png.width, png.height), (2400, 80));
        assert_eq!((jpeg.width, jpeg.height), (2400, 80));
        assert_eq!(png.source_bytes, 123);
        assert_eq!(jpeg.source_bytes, 456);
        assert!(png.data_url.starts_with("data:image/png;base64,"));
        assert!(jpeg.data_url.starts_with("data:image/jpeg;base64,"));
    }

    fn tile_request(
        token: &str,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        max_edge: u32,
    ) -> super::super::model::PreviewTileRequest {
        super::super::model::PreviewTileRequest {
            source: super::super::model::PreviewTileSource::Compressed,
            cache_token: Some(token.into()),
            branch_id: None,
            x,
            y,
            width,
            height,
            max_edge,
        }
    }

    fn original_tile_request(
        branch_id: &str,
        width: u32,
        height: u32,
        max_edge: u32,
    ) -> super::super::model::PreviewTileRequest {
        super::super::model::PreviewTileRequest {
            source: super::super::model::PreviewTileSource::Original,
            cache_token: None,
            branch_id: Some(branch_id.into()),
            x: 0,
            y: 0,
            width,
            height,
            max_edge,
        }
    }

    #[test]
    fn preview_tile_crops_cached_rendition_without_upscaling() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let config = fixture_config();
        let source_sha256 = "A".repeat(64);
        let token = rendition_cache_token(&source_sha256, &config, None).unwrap();
        let operation = state.begin_operation("tile 测试").unwrap();
        let cached = render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            image::DynamicImage::ImageRgb8(image::RgbImage::new(320, 240)),
            &config,
            None,
            &token,
            &source_sha256,
        )
        .unwrap();
        assert_eq!((cached.width, cached.height), (320, 240));

        let full = super::preview_tile(
            directory.path(),
            &tile_request(&token, 0, 0, 320, 240, 4096),
        )
        .unwrap();
        assert_eq!((full.width, full.height), (320, 240));
        assert!(full.data_url.starts_with("data:image/png;base64,"));

        let region = super::preview_tile(
            directory.path(),
            &tile_request(&token, 10, 20, 160, 120, 80),
        )
        .unwrap();
        assert_eq!((region.width, region.height), (80, 60));
    }

    #[test]
    fn preview_tile_rejects_bad_tokens_rects_and_missing_cache() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let token = "a".repeat(64);

        assert!(super::preview_tile(
            directory.path(),
            &tile_request("short-token", 0, 0, 1, 1, 256)
        )
        .is_err());
        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 16)).is_err()
        );
        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 4097)).is_err()
        );
        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 256)).is_err()
        );

        let state = fixture_state();
        let config = fixture_config();
        let operation = state.begin_operation("tile 校验测试").unwrap();
        render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24)),
            &config,
            None,
            &token,
            &"B".repeat(64),
        )
        .unwrap();

        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 30, 0, 8, 8, 256)).is_err()
        );
        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 0, 0, 0, 8, 256)).is_err()
        );
        assert!(
            super::preview_tile(directory.path(), &tile_request(&token, 0, 0, 8, 8, 256)).is_ok()
        );
    }

    #[test]
    fn preview_tile_crops_original_artifact_by_branch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let relative = "artworks/final.png";
        let absolute = storage::resolve_path(&root, relative).unwrap();
        fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(300, 200))
            .save(&absolute)
            .unwrap();
        let sha = hex::encode_upper(Sha256::digest(fs::read(&absolute).unwrap()));
        storage::open(&root)
            .unwrap()
            .execute_batch(&format!(
                "INSERT INTO library_nodes
                   (id, kind, title, position, created_ms, updated_ms)
                 VALUES ('artwork', 'artwork', 'Artwork', 0, 0, 0);
                 INSERT INTO artworks (id, description, created_ms, updated_ms)
                 VALUES ('artwork', '', 0, 0);
                 INSERT INTO branches
                   (id, artwork_id, title, source_path, source_path_key,
                    backup_enabled, backup_interval_minutes, created_ms, updated_ms)
                 VALUES ('branch', 'artwork', 'Main', 'source.psd', 'source.psd', 1, 5, 0, 0);
                 INSERT INTO history_nodes
                   (id, artwork_id, created_on_branch_id, title, note, commit_kind,
                    is_checkpoint, created_ms, logical_size, chunk_file_size, sha256,
                    chunk_count, snapshot_path)
                 VALUES ('history', 'artwork', 'branch', 'History', '', 'manual',
                         1, 0, 1, 1,
                         '0000000000000000000000000000000000000000000000000000000000000000',
                         1, 'artworks/snapshot.chunk');
                 INSERT INTO final_artifacts
                   (id, branch_id, history_id, source_path, source_sha256,
                    media_type, byte_size, created_ms)
                 VALUES
                   ('artifact', 'branch', 'history', '{relative}', '{sha}',
                    'image/png', 1, 0);"
            ))
            .unwrap();

        let tile =
            super::preview_tile(&root, &original_tile_request("branch", 300, 200, 120)).unwrap();
        assert_eq!((tile.width, tile.height), (120, 80));
        assert!(tile.data_url.starts_with("data:image/png;base64,"));

        // 矩形超出源尺寸或分支不存在都明确拒绝。
        assert!(
            super::preview_tile(&root, &original_tile_request("branch", 301, 200, 120)).is_err()
        );
        assert!(
            super::preview_tile(&root, &original_tile_request("missing", 300, 200, 120)).is_err()
        );
    }

    #[test]
    fn rendition_cache_reuses_valid_bytes_and_rejects_replacement() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let config = fixture_config();
        let source_sha256 = "A".repeat(64);
        let token = rendition_cache_token(&source_sha256, &config, None).unwrap();
        let source = image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24));
        let operation = state.begin_operation("缓存测试").unwrap();

        let rendered = render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            source,
            &config,
            None,
            &token,
            &source_sha256,
        )
        .unwrap();
        assert!(!rendered.cache_hit);

        let reused =
            load_cached_rendition(directory.path(), &operation, &token, &source_sha256, None)
                .unwrap()
                .unwrap();
        assert!(reused.cache_hit);
        assert_eq!(reused.output_bytes, rendered.output_bytes);

        fs::write(&reused.path, b"replaced").unwrap();
        assert!(
            load_cached_rendition(directory.path(), &operation, &token, &source_sha256, None,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cancelled_rendition_does_not_publish_cache_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let operation = state.begin_operation("取消缓存测试").unwrap();
        state.request_cancel().unwrap();
        let config = fixture_config();

        let result = render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            image::DynamicImage::new_rgb8(32, 24),
            &config,
            None,
            &"C".repeat(64),
            &"D".repeat(64),
        );

        assert!(matches!(
            result,
            Err(error) if error.to_string().contains("已取消")
        ));
        assert!(fs::read_dir(directory.path().join("temp"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn rendition_cache_token_changes_with_pixel_settings_and_identifier() {
        let source_sha256 = "B".repeat(64);
        let config = fixture_config();
        let initial =
            rendition_cache_token(&source_sha256, &config, Some(FIXTURE_WATERMARK_ID)).unwrap();
        let mut changed = config.clone();
        changed.jpeg_quality -= 1;

        assert_ne!(
            initial,
            rendition_cache_token(&source_sha256, &changed, Some(FIXTURE_WATERMARK_ID)).unwrap()
        );
        assert_ne!(
            initial,
            rendition_cache_token(&source_sha256, &config, Some(&"0".repeat(40))).unwrap()
        );
    }
}
