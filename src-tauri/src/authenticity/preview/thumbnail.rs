//! IPC 缩略图：记录/识别预览与发布质量预览的缩略图层。
//!
//! 只做缩放与编码，不做透明背景合成或 TrustMark；所有入口共用同一套尺寸上限与
//! data URL 编码。

use std::{fs, io::Cursor, path::PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView};

use super::super::{
    error::{AuthenticityError, AuthenticityResult},
    image_resource,
    model::PreviewImage,
};

/// 发布质量预览的缩略图最长边。完整缓存图仍按源分辨率渲染，界面只在放大时请求
/// 高清局部。
pub(crate) const PUBLICATION_PREVIEW_EDGE: u32 = 2400;
/// 记录、成品与识别预览的缩略图最长边。
const STANDARD_PREVIEW_EDGE: u32 = 1600;
/// 发布预览的 JPEG 缩略图质量；只影响 IPC 传输层，不影响缓存中的成品编码。
const PUBLICATION_PREVIEW_JPEG_QUALITY: u8 = 92;

#[derive(Clone, Copy)]
pub(crate) enum ThumbnailFormat {
    Png,
    Jpeg(u8),
}

/// 把图片缩到最长边 `max_edge`，按指定格式编码为 data URL。
pub(crate) fn render_thumbnail(
    source: &DynamicImage,
    source_bytes: u64,
    max_edge: u32,
    format: ThumbnailFormat,
) -> AuthenticityResult<PreviewImage> {
    let preview = source.thumbnail(max_edge, max_edge);
    let (width, height) = preview.dimensions();
    let mut encoded = Cursor::new(Vec::new());
    let mime = match format {
        ThumbnailFormat::Png => {
            preview.write_to(&mut encoded, image::ImageFormat::Png)?;
            "image/png"
        }
        ThumbnailFormat::Jpeg(quality) => {
            JpegEncoder::new_with_quality(&mut encoded, quality).encode_image(&preview)?;
            "image/jpeg"
        }
    };
    Ok(PreviewImage {
        data_url: format!(
            "data:{mime};base64,{}",
            STANDARD.encode(encoded.into_inner())
        ),
        width,
        height,
        source_bytes,
    })
}

/// 发布质量预览的缩略图：导出图用 JPEG（与成品一致），原图用 PNG。
pub(crate) fn publication_export_preview(
    source: &DynamicImage,
    source_bytes: u64,
) -> AuthenticityResult<PreviewImage> {
    render_thumbnail(
        source,
        source_bytes,
        PUBLICATION_PREVIEW_EDGE,
        ThumbnailFormat::Jpeg(PUBLICATION_PREVIEW_JPEG_QUALITY),
    )
}

pub(crate) fn publication_original_preview(
    source: &DynamicImage,
    source_bytes: u64,
) -> AuthenticityResult<PreviewImage> {
    render_thumbnail(
        source,
        source_bytes,
        PUBLICATION_PREVIEW_EDGE,
        ThumbnailFormat::Png,
    )
}

/// 记录、成品与外部图片的 PNG 缩略图；返回源图尺寸而不是缩略图尺寸。
pub(crate) fn make_preview(path: PathBuf) -> AuthenticityResult<PreviewImage> {
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput("预览图片不存在".into()));
    }
    let source = image_resource::open(&path)?;
    let (width, height) = source.dimensions();
    let preview = render_thumbnail(
        &source,
        fs::metadata(&path)?.len(),
        STANDARD_PREVIEW_EDGE,
        ThumbnailFormat::Png,
    )?;
    drop(source);
    Ok(PreviewImage {
        width,
        height,
        ..preview
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_helpers_bound_ipc_image_dimensions() {
        let source = DynamicImage::new_rgb8(3000, 100);

        let png = publication_original_preview(&source, 123).unwrap();
        let jpeg = publication_export_preview(&source, 456).unwrap();

        assert_eq!((png.width, png.height), (2400, 80));
        assert_eq!((jpeg.width, jpeg.height), (2400, 80));
        assert_eq!(png.source_bytes, 123);
        assert_eq!(jpeg.source_bytes, 456);
        assert!(png.data_url.starts_with("data:image/png;base64,"));
        assert!(jpeg.data_url.starts_with("data:image/jpeg;base64,"));
    }
}
