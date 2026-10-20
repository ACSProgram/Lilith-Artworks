use std::path::Path;

use image::{DynamicImage, ImageFormat};

use crate::cancel::CancelToken;

use super::{decode_path, decode_path_as, extension_lower};

/// Formats accepted here are decided by their magic bytes; TGA is the single
/// exception because the format has no signature at all.
const MAGIC_DETECTED: &[ImageFormat] = &[
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::WebP,
    ImageFormat::Tiff,
    ImageFormat::Bmp,
    ImageFormat::Gif,
];

/// Decode a plain raster source file (png/jpg/webp/tiff/bmp/gif/tga). Other
/// formats are handled by their container extractors and are rejected here so a
/// misdetected layered file never reaches the generic decoder.
pub(super) fn extract(path: &Path, head: &[u8], cancel: &CancelToken) -> Option<DynamicImage> {
    if cancel.observed() {
        return None;
    }
    match image::guess_format(head) {
        Ok(format) if MAGIC_DETECTED.contains(&format) => decode_path(path),
        // Anything else — no magic at all, or a magic this module does not own —
        // may only be a TGA, and only when the extension says so.
        _ => decode_tga(path),
    }
}

/// TGA carries no magic bytes: `image::guess_format` never reports it, so the
/// extension is the only signal the format offers.
///
/// This runs for *every* rejected head, not just unknown ones: a palette-based
/// TGA header (id length 0, no colour map, image type 1) starts with
/// `00 00 01 00`, which is exactly the ICO signature. Requiring the `.tga`
/// extension keeps that collision from turning a real `.ico` into an attempt,
/// while still letting such a TGA through.
fn decode_tga(path: &Path) -> Option<DynamicImage> {
    if extension_lower(path).as_deref() != Some("tga") {
        return None;
    }
    decode_path_as(path, ImageFormat::Tga)
}
