//! 认证渲染：配置归一化、渲染缓存、透明背景合成与 JPEG 编码。
//!
//! 质量预览与签名发布共用这里的缓存令牌、缓存校验和渲染步骤；两者的差异（预览
//! 需要额外缩略图、发布需要裁剪声明字段）留在各自入口，不在本模块里分叉。
//! 本模块不读写数据库，也不负责 C2PA 签名。
//!
//! 按变化原因拆成子模块：`cache` 管会话目录、缓存校验与淘汰，`render` 管渲染与
//! JPEG 编码；配置归一化与图片工具留在本模块。

use std::{path::Path, time::Instant};

use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use sha2::{Digest, Sha256};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    image_resource,
    model::CertificationConfig,
    publication_repository::BranchPublicationTarget,
    state::{AuthenticityOperation, AuthenticityState},
    trustmark,
};
use cache::load_cached_rendition;

mod cache;
mod render;

pub(crate) use cache::{cache_paths, preview_session_id, CachedRendition, RenditionCacheMetadata};
pub(crate) use render::render_cached_rendition;

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

    fn target(source_sha256: &str) -> BranchPublicationTarget {
        BranchPublicationTarget {
            history_id: "history".into(),
            artifact_id: "artifact".into(),
            artifact_path: "unused.jpg".into(),
            source_sha256: source_sha256.into(),
        }
    }
}
