//! 认证渲染：配置归一化、渲染缓存、透明背景合成与 JPEG 编码。
//!
//! 质量预览与签名发布共用这里的缓存令牌、缓存校验和渲染步骤；两者的差异（预览
//! 需要额外缩略图、发布需要裁剪声明字段）留在各自入口，不在本模块里分叉。
//! 本模块不读写数据库，也不负责 C2PA 签名。

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Instant, SystemTime},
};

use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::storage;

use super::{
    error::{AuthenticityError, AuthenticityResult},
    file_ops::sha256_file_cancelable,
    image_resource,
    model::CertificationConfig,
    publication_repository::BranchPublicationTarget,
    state::{AuthenticityOperation, AuthenticityState},
    trustmark,
};

static PREVIEW_CACHE_SESSION: OnceLock<String> = OnceLock::new();

/// 会话内预览缓存的份数上限。
///
/// 每换一次参数（JPEG 质量、背景色、水印强度、框选区域、水印 ID）就留下一份全尺寸
/// JPEG 与一份元数据；常见调参会有十几次迭代，16 份足够来回比较几组参数，又不会
/// 让一次会话堆出几十份预览。当前正在使用的那份总是最新的，因此天然落在保留区间。
const PREVIEW_CACHE_MAX_ENTRIES: usize = 16;

/// 会话内预览缓存的 JPEG 总字节上限（512 MiB）。
///
/// 普通画布单份缓存只有几 MB，份数上限通常先生效；这个上限兜住超大画布连续调参
/// 的情况（16K 成品单份就可能上百 MB）。超限时继续从最旧的删，直到不超或只剩一份。
/// 同名元数据 JSON 只有几百字节，不计入这里统计的总量。
const PREVIEW_CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// 当前进程的预览缓存会话 id；本进程尚未写过任何质量预览缓存时为 `None`。
/// 仓库临时目录清扫用它保留仍属于本进程的会话目录。
pub(crate) fn preview_session_id() -> Option<&'static str> {
    PREVIEW_CACHE_SESSION.get().map(String::as_str)
}

/// 回填分支标识，并按选区决定是否启用 TrustMark。
///
/// 预览与发布都先做同一步归一化，再各自校验配置：TrustMark 开关在没有任何选区时
/// 一律视为关闭，避免「开着开关却没有可嵌入区域」的半启用状态。
pub(crate) fn normalize_config(config: &mut CertificationConfig, branch_id: &str) {
    config.branch_id = branch_id.to_owned();
    config.trustmark_enabled = config.trustmark_enabled && !config.additional_regions.is_empty();
}

pub(crate) struct RenditionIdentity {
    pub(crate) identifier: Option<String>,
    pub(crate) cache_token: String,
}

/// 解析本次渲染的 TrustMark ID 与缓存令牌。
///
/// 令牌绑定最终成品摘要、分支 ID、所有影响像素的参数与 TrustMark ID；任何一项变化
/// 都会得到新令牌，旧缓存不会被误用。
pub(crate) fn rendition_identity(
    target: &BranchPublicationTarget,
    config: &CertificationConfig,
    watermark_id: Option<&str>,
) -> AuthenticityResult<RenditionIdentity> {
    let identifier = config
        .trustmark_enabled
        .then(|| trustmark::resolve_identifier(watermark_id))
        .transpose()?;
    let cache_token = rendition_cache_token(&target.source_sha256, config, identifier.as_deref())?;
    Ok(RenditionIdentity {
        identifier,
        cache_token,
    })
}

/// 复用有效缓存，否则用 `source`（缺省时重新解码 `input`）渲染并写入缓存。
///
/// `reuse_token` 只有与本次令牌完全一致时才允许命中：发布命令据此确认预览与本次
/// 参数属于同一次渲染。缓存命中时不保留已解码的源图。
pub(crate) fn prepare_rendition(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    input: &Path,
    config: &CertificationConfig,
    target: &BranchPublicationTarget,
    identity: &RenditionIdentity,
    reuse_token: Option<&str>,
    source: Option<DynamicImage>,
) -> AuthenticityResult<CachedRendition> {
    if reuse_token == Some(identity.cache_token.as_str()) {
        if let Some(cached) = load_cached_rendition(
            root,
            operation,
            &identity.cache_token,
            &target.source_sha256,
            identity.identifier.as_deref(),
        )? {
            return Ok(cached);
        }
    }
    let source = match source {
        Some(source) => source,
        None => image_resource::open(input)?,
    };
    render_cached_rendition(
        root,
        state,
        operation,
        source,
        config,
        identity.identifier.as_deref(),
        &identity.cache_token,
        &target.source_sha256,
    )
}

pub(crate) fn rendition_cache_token(
    source_sha256: &str,
    config: &CertificationConfig,
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

pub(crate) fn cache_paths(root: &Path, token: &str) -> AuthenticityResult<(PathBuf, PathBuf)> {
    let directory = preview_cache_directory(root)?;
    Ok((
        directory.join(format!("{token}.jpg")),
        directory.join(format!("{token}.json")),
    ))
}

#[derive(Deserialize, Serialize)]
pub(crate) struct RenditionCacheMetadata {
    pub(crate) token: String,
    pub(crate) source_sha256: String,
    pub(crate) jpeg_sha256: String,
    pub(crate) watermark_id: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_bytes: u64,
}

pub(crate) struct CachedRendition {
    pub(crate) path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_bytes: u64,
    pub(crate) cache_hit: bool,
    pub(crate) render_ms: u64,
    pub(crate) encode_ms: u64,
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

pub(crate) fn render_cached_rendition(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    source: DynamicImage,
    config: &CertificationConfig,
    watermark_id: Option<&str>,
    token: &str,
    source_sha256: &str,
) -> AuthenticityResult<CachedRendition> {
    ensure_not_cancelled(operation)?;
    let (width, height) = source.dimensions();
    let render_started = Instant::now();
    let background = parse_background(&config.background_color)?;
    let flattened = flatten_to_rgb(&source, background);
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
    prune_preview_cache(directory);
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

/// 会话内缓存淘汰：按 JPEG 修改时间从旧到新，先删到只剩最近
/// [`PREVIEW_CACHE_MAX_ENTRIES`] 份，再在 JPEG 总字节超过 [`PREVIEW_CACHE_MAX_BYTES`]
/// 时继续删最旧的，直到不超或只剩一份。
///
/// 每次写入新的缓存对之后调用。删除成功与否都不影响预览与发布结果——缓存不是
/// 正确性路径，失败静默忽略，最多让目录多留几份等下次淘汰或会话结束清扫。
fn prune_preview_cache(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut cached = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jpg") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        cached.push((modified, metadata.len(), path));
    }
    let mut jpeg_bytes: u64 = cached.iter().map(|(_, bytes, _)| *bytes).sum();
    if cached.len() <= PREVIEW_CACHE_MAX_ENTRIES && jpeg_bytes <= PREVIEW_CACHE_MAX_BYTES {
        return;
    }
    cached.sort_by_key(|(modified, _, _)| *modified);
    let mut remaining = cached.len();
    for (_, bytes, jpeg_path) in cached {
        // 至少留一份：正在使用的那份总是最新，走到这里只会剩下最旧的一份。
        if remaining <= 1 {
            break;
        }
        if remaining <= PREVIEW_CACHE_MAX_ENTRIES && jpeg_bytes <= PREVIEW_CACHE_MAX_BYTES {
            break;
        }
        let _ = fs::remove_file(&jpeg_path);
        let _ = fs::remove_file(jpeg_path.with_extension("json"));
        jpeg_bytes = jpeg_bytes.saturating_sub(bytes);
        remaining -= 1;
    }
}

pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 透明背景合成：直接遍历源像素，不额外生成一份全图 RGBA。
pub(crate) fn flatten_to_rgb(image: &DynamicImage, background: Rgb<u8>) -> RgbImage {
    let mut output = RgbImage::new(image.width(), image.height());
    for (x, y, pixel) in image.pixels() {
        let alpha = pixel[3] as u16;
        let inverse = 255 - alpha;
        output.put_pixel(
            x,
            y,
            Rgb([
                ((pixel[0] as u16 * alpha + background[0] as u16 * inverse + 127) / 255) as u8,
                ((pixel[1] as u16 * alpha + background[1] as u16 * inverse + 127) / 255) as u8,
                ((pixel[2] as u16 * alpha + background[2] as u16 * inverse + 127) / 255) as u8,
            ]),
        );
    }
    output
}

pub(crate) fn parse_background(value: &str) -> AuthenticityResult<Rgb<u8>> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AuthenticityError::InvalidInput(
            "透明背景色必须使用 #RRGGBB 格式".into(),
        ));
    }
    let channel = |start| u8::from_str_radix(&hex[start..start + 2], 16);
    Ok(Rgb([
        channel(0).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
        channel(2).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
        channel(4).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
    ]))
}

/// 认证流水线接受的图片扩展名 → MIME。
///
/// 命令入参、拖放暂存、成品登记与 C2PA ingredient 共用这一张表；各调用点只保留
/// 自己的错误文案。
pub(crate) fn image_media_type(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}

pub(crate) fn image_media_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    image_media_type(&extension)
}

pub(crate) fn supports_image_extension(extension: &str) -> bool {
    image_media_type(extension).is_some()
}

pub(crate) fn is_jpeg_extension(extension: &str) -> bool {
    matches!(extension, "jpg" | "jpeg")
}

pub(crate) fn ensure_not_cancelled(operation: &AuthenticityOperation) -> AuthenticityResult<()> {
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
pub(crate) fn auth_checkpoint(
    operation: &AuthenticityOperation,
    stage: &str,
) -> AuthenticityResult<()> {
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
    use std::fs;

    use super::*;
    use crate::authenticity::test_support::{fixture_config, fixture_state, FIXTURE_WATERMARK_ID};

    #[test]
    fn rendition_cache_reuses_valid_bytes_and_rejects_replacement() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let config = fixture_config();
        let source_sha256 = "A".repeat(64);
        let token = rendition_cache_token(&source_sha256, &config, None).unwrap();
        let operation = state.begin_operation("缓存测试").unwrap();
        let target = target(&source_sha256);
        let identity = RenditionIdentity {
            identifier: None,
            cache_token: token.clone(),
        };

        let rendered = prepare_rendition(
            directory.path(),
            &state,
            &operation,
            Path::new("unused.jpg"),
            &config,
            &target,
            &identity,
            Some(&token),
            Some(image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24))),
        )
        .unwrap();
        assert!(!rendered.cache_hit);

        let reused = prepare_rendition(
            directory.path(),
            &state,
            &operation,
            Path::new("unused.jpg"),
            &config,
            &target,
            &identity,
            Some(&token),
            None,
        )
        .unwrap();
        assert!(reused.cache_hit);
        assert_eq!(reused.output_bytes, rendered.output_bytes);

        fs::write(&reused.path, b"replaced").unwrap();
        let reloaded =
            load_cached_rendition(directory.path(), &operation, &token, &source_sha256, None)
                .unwrap();
        assert!(reloaded.is_none());
    }

    #[test]
    fn cancelled_rendition_does_not_publish_cache_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let operation = state.begin_operation("取消缓存测试").unwrap();
        state.request_cancel().unwrap();
        let config = fixture_config();
        let target = target(&"D".repeat(64));
        let identity = RenditionIdentity {
            identifier: None,
            cache_token: "C".repeat(64),
        };

        let result = prepare_rendition(
            directory.path(),
            &state,
            &operation,
            Path::new("unused.jpg"),
            &config,
            &target,
            &identity,
            Some(&identity.cache_token),
            Some(image::DynamicImage::new_rgb8(32, 24)),
        );

        assert!(matches!(
            result,
            Err(error) if error.to_string().contains("已取消")
        ));
        // 取消可能已创建会话目录，但不得写出任何缓存文件。
        for entry in fs::read_dir(directory.path().join("temp")).unwrap() {
            let entry = entry.unwrap();
            assert!(
                entry.path().is_dir(),
                "unexpected cache file: {:?}",
                entry.path()
            );
            assert!(fs::read_dir(entry.path()).unwrap().next().is_none());
        }
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

    #[test]
    fn preview_cache_pruning_keeps_the_latest_entries_and_removes_metadata_pairs() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("authenticity-preview-test");
        fs::create_dir(&cache).unwrap();
        for index in 0..20 {
            let jpeg = cache.join(format!("{index:02}.jpg"));
            let metadata = cache.join(format!("{index:02}.json"));
            fs::write(&jpeg, b"jpeg").unwrap();
            fs::write(&metadata, b"{}").unwrap();
            let stamp = filetime::FileTime::from_unix_time(1_700_000_000 + index, 0);
            filetime::set_file_mtime(&jpeg, stamp).unwrap();
        }

        prune_preview_cache(&cache);

        let mut names: Vec<String> = fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = (4..20)
            .flat_map(|index| [format!("{index:02}.jpg"), format!("{index:02}.json")])
            .collect();
        expected.sort();
        // 最旧的 4 份及其元数据成对删除，最近 16 份原样保留。
        assert_eq!(names, expected);
    }

    fn target(source_sha256: &str) -> BranchPublicationTarget {
        BranchPublicationTarget {
            history_id: "history".into(),
            artifact_id: "artifact".into(),
            artifact_path: "unused.jpg".into(),
            source_sha256: source_sha256.into(),
        }
    }
}
