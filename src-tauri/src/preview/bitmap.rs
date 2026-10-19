use std::path::Path;

use image::{DynamicImage, ImageFormat};

use crate::cancel::CancelToken;

use super::decode_path;

/// Decode a plain raster source file (png/jpg/webp/tiff/bmp/gif). Other formats
/// are handled by their container extractors and are rejected here so a
/// misdetected layered file never reaches the generic decoder.
pub(super) fn extract(path: &Path, head: &[u8], cancel: &CancelToken) -> Option<DynamicImage> {
    let format = image::guess_format(head).ok()?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::WebP
            | ImageFormat::Tiff
            | ImageFormat::Bmp
            | ImageFormat::Gif
    ) {
        return None;
    }
    if cancel.observed() {
        return None;
    }
    decode_path(path)
}
