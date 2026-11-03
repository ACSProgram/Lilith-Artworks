//! 认证渲染与 JPEG 编码：透明背景合成、TrustMark 嵌入、缓存写入。

use std::{io::Write, path::Path, time::Instant};

use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView};
use tempfile::NamedTempFile;

use super::super::{
    error::{AuthenticityError, AuthenticityResult},
    file_ops::sha256_file_cancelable,
    model::CertificationConfig,
    state::{AuthenticityOperation, AuthenticityState},
    trustmark,
};
use super::cache::{
    cache_paths, prune_preview_cache, replace_cache_file, CachedRendition, RenditionCacheMetadata,
};
use super::{auth_checkpoint, elapsed_ms, ensure_not_cancelled, flatten_to_rgb, parse_background};

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
