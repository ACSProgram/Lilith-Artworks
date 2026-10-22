//! 发布质量预览：渲染无签名成品、写入会话缓存并返回缩略图。
//!
//! 参数的归一化、缓存令牌与渲染本身与签名发布共用 `rendition`；本模块只补充预览
//! 需要的基础缩略图（原图 PNG、导出图 JPEG）和返回结构。

use std::{fs, path::Path};

use image::GenericImageView;

use super::super::{
    error::{AuthenticityError, AuthenticityResult},
    file_ops::canonical_existing_file,
    image_resource,
    model::{PublicationPreview, PublicationPreviewRequest},
    publication_repository,
    publish::validate_publication_config,
    rendition::{ensure_not_cancelled, normalize_config, prepare_rendition, rendition_identity},
    state::{AuthenticityOperation, AuthenticityState},
};
use super::thumbnail::{publication_export_preview, publication_original_preview};

pub(crate) fn preview(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    mut request: PublicationPreviewRequest,
) -> AuthenticityResult<PublicationPreview> {
    ensure_not_cancelled(operation)?;
    normalize_config(&mut request.config, &request.branch_id);
    validate_publication_config(&request.config)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let identity = rendition_identity(&target, &request.config, request.watermark_id.as_deref())?;
    let source = image_resource::open(&input)?;
    ensure_not_cancelled(operation)?;
    let (width, height) = source.dimensions();
    let source_bytes = fs::metadata(&input)?.len();
    let original_image = publication_original_preview(&source, source_bytes)?;
    ensure_not_cancelled(operation)?;
    let cached = prepare_rendition(
        root,
        state,
        operation,
        &input,
        &request.config,
        &target,
        &identity,
        Some(&identity.cache_token),
        Some(source),
    )?;
    ensure_not_cancelled(operation)?;
    let compressed = image_resource::open(&cached.path)?;
    let image = publication_export_preview(&compressed, cached.output_bytes)?;
    ensure_not_cancelled(operation)?;
    Ok(PublicationPreview {
        branch_id: request.branch_id,
        image,
        original_image,
        source_width: width,
        source_height: height,
        output_bytes: cached.output_bytes,
        watermark_id: identity.identifier,
        cache_token: identity.cache_token,
        cache_hit: cached.cache_hit,
        render_ms: cached.render_ms,
        encode_ms: cached.encode_ms,
    })
}
