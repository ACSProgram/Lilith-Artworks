//! Derived history-node thumbnails.
//!
//! Extraction is strictly optional and never fails the caller: any unsupported
//! or malformed input yields `None`, and the caller records no preview. It has
//! two entry points: the commit worker passes the work file it already has
//! open, and the manual refresh passes a file materialized from the node's own
//! stored content. `.clip` / `.psd` detection and structural navigation are
//! shared with the chunk writer through `format::container`; the thumbnail-only
//! formats (`sai2`, ZIP containers, extension-driven bitmaps) are probed here
//! instead, so their layouts never leak into the chunking path.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};

use crate::cancel::CancelToken;

mod bitmap;
mod clip;
mod psd;
mod sai2;
mod zip;

/// Version of the thumbnail payload (encoding + size policy). Stored next to
/// the BLOB so a future encoder can rebuild without guessing, and so the read
/// path can refuse a payload whose encoding it does not know.
pub(crate) const THUMBNAIL_VERSION: i64 = 1;

/// Longest edge of the stored preview.
pub(super) const MAX_EDGE: u32 = 512;
/// Upper bound for the encoded preview. A 512 px lossless WebP stays well
/// below this; the cap only guards against pathological input.
const MAX_THUMBNAIL_BYTES: usize = 2 * 1024 * 1024;
/// Decode budget shared by the raster and PSD composite paths.
pub(super) const MAX_SOURCE_PIXELS: u64 = 64_000_000;
/// Per-axis decode budget; matches `authenticity::image_resource::MAX_IMAGE_EDGE`.
pub(super) const MAX_SOURCE_EDGE: u32 = 32_768;

pub(crate) struct Thumbnail {
    pub(crate) bytes: Vec<u8>,
    pub(crate) version: i64,
}

/// 工作文件是否属于可抽取缩略图的格式。
///
/// 历史节点自身保存的是 ChunkFile 容器，读不出原始格式，所以这里按节点所属分支
/// 工作文件的扩展名判断。它只决定前端要不要提供「无可用预览」占位与刷新入口，
/// 不参与抽取本身：`.clip` 的内嵌预览、`.psd` / `.psb` 的缩略图资源或合成图、
/// `.sai2` 的画布内嵌缩略图、`.procreate` 的 `QuickLook` 缩略图、以及直接解码的
/// 位图都算支持，其余格式一律返回 `false`。
///
/// 「支持 ≠ 一定有」：`.sai2` 的旧变体、没有 `QuickLook/Thumbnail.png` 的
/// `.procreate` 都会在支持的前提下抽不出缩略图，那时前端显示占位而不是隐藏预览区。
pub(crate) fn supports_preview(path: &Path) -> bool {
    matches!(
        extension_lower(path).as_deref(),
        Some(
            "clip"
                | "psd"
                | "psb"
                | "sai2"
                | "procreate"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "tif"
                | "tiff"
                | "bmp"
                | "gif"
                | "tga"
        )
    )
}

/// Lowercased extension, the only format signal a source without magic bytes
/// offers. Shared by the support check and by the extractors that dispatch on
/// the extension (ZIP layouts, TGA).
pub(super) fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}

/// ZIP 局部文件头。ZIP 家族按扩展名分发布局：分派只按这个头进来，是否真去开
/// 容器由 `zip` 模块候选表里登记的扩展名决定，普通 `.zip` / `.jar` 因此直接判否。
const ZIP_LOCAL_HEADER: &[u8; 4] = b"PK\x03\x04";

/// Extract a bounded preview from a source file. Never fails the caller.
pub(crate) fn extract(path: &Path, cancel: &CancelToken) -> Option<Thumbnail> {
    if cancel.observed() {
        return None;
    }
    let mut head = [0_u8; 16];
    let read = File::open(path).ok()?.read(&mut head).ok()?;
    let probe = &head[..read];
    // 分层顺序固定：先看各有独占魔数的两种容器（`sai2` 的 16 字节标识、ZIP 的 4 字节
    // 局部文件头），再退回 `format::container` 的容器探测，最后才是位图。这些分支都只
    // 属于缩略图路径，`format::container` 的容器知识不跟着扩张——它和 chunk writer
    // 共享，ZIP / sai2 的结构不该漏进分块。
    let image = if probe.starts_with(sai2::IDENT) {
        sai2::extract(path, cancel)?
    } else if probe.starts_with(ZIP_LOCAL_HEADER) {
        zip::extract(path, cancel)?
    } else {
        match crate::format::container::detect(probe) {
            Some(crate::format::container::ContainerKind::Clip) => clip::extract(path, cancel)?,
            Some(crate::format::container::ContainerKind::Psd) => psd::extract(path, cancel)?,
            None => bitmap::extract(path, probe, cancel)?,
        }
    };
    if cancel.observed() {
        return None;
    }
    encode(image)
}

fn encode(image: DynamicImage) -> Option<Thumbnail> {
    let preview = image.thumbnail(MAX_EDGE, MAX_EDGE);
    let rgba = preview.to_rgba8();
    let mut bytes = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .ok()?;
    if bytes.is_empty() || bytes.len() > MAX_THUMBNAIL_BYTES {
        return None;
    }
    Some(Thumbnail {
        bytes,
        version: THUMBNAIL_VERSION,
    })
}

pub(super) fn checked_pixels(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 || width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    (pixels <= MAX_SOURCE_PIXELS).then_some(pixels)
}

/// Decoder limits for any image bytes we did not produce ourselves. Kept in
/// one place so every extractor (`.clip` payloads, PSD resources, JSSF streams,
/// ZIP entries, raster files) shares the same budget.
pub(super) fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_SOURCE_PIXELS * 8 + 32 * 1024 * 1024);
    limits
}

/// Decode an embedded image payload (`.clip` canvas preview, PSD thumbnail
/// resource, the JPEG rebuilt from a `.sai2` `thum` stream, a `.procreate` ZIP
/// entry) under the shared budget. The dimension probe needs its own reader
/// because `into_dimensions` consumes the first one; the second reader sees the
/// same bytes, so the probe and the decode always agree on the size.
pub(super) fn decode_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let probe = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let (width, height) = probe.into_dimensions().ok()?;
    checked_pixels(width, height)?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(decode_limits());
    reader.decode().ok()
}

/// Decode a raster source file from disk under the same budget. The file is
/// reopened after the dimension probe for the same reason as `decode_bytes`.
pub(super) fn decode_path(path: &Path) -> Option<DynamicImage> {
    let probe = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let (width, height) = probe.into_dimensions().ok()?;
    checked_pixels(width, height)?;
    let mut reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    reader.limits(decode_limits());
    reader.decode().ok()
}

/// Decode a raster file whose format cannot be guessed from its magic bytes.
///
/// TGA has no reliable signature — `image::guess_format` documents that it does
/// not detect it at all — so the only honest source is the extension. The
/// dimensions still go through the same pixel budget, and the decoder is built
/// with an explicit format instead of `with_guessed_format`, which would fail
/// before reaching the decoder.
pub(super) fn decode_path_as(path: &Path, format: ImageFormat) -> Option<DynamicImage> {
    let mut reader = ImageReader::open(path).ok()?;
    reader.set_format(format);
    reader.limits(decode_limits());
    let decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    checked_pixels(width, height)?;
    DynamicImage::from_decoder(decoder).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 90, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn raster_previews_are_bounded_and_reencoded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.png");
        std::fs::write(&path, png(1024, 256)).unwrap();

        let thumbnail = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!(thumbnail.version, THUMBNAIL_VERSION);
        let decoded = image::load_from_memory(&thumbnail.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 128));
    }

    #[test]
    fn unknown_and_corrupt_inputs_yield_no_preview() {
        let directory = tempfile::tempdir().unwrap();
        let unknown = directory.path().join("source.bin");
        std::fs::write(&unknown, vec![0_u8; 4096]).unwrap();
        assert!(extract(&unknown, &CancelToken::standalone()).is_none());

        let corrupt = directory.path().join("source.png");
        std::fs::write(&corrupt, png(32, 32)[..40].to_vec()).unwrap();
        assert!(extract(&corrupt, &CancelToken::standalone()).is_none());
    }

    /// TGA has no magic bytes, so it is the one format decided by extension.
    #[test]
    fn tga_is_decoded_through_its_extension() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.tga");
        let image: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(1024, 512, |x, y| Rgba([x as u8, y as u8, 30, 255]));
        image
            .write_to(
                &mut std::fs::File::create(&path).unwrap(),
                image::ImageFormat::Tga,
            )
            .unwrap();

        let thumbnail = extract(&path, &CancelToken::standalone()).unwrap();
        let decoded = image::load_from_memory(&thumbnail.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 256));

        // The same bytes under another extension must not be decoded: the
        // extension is the only claim we are willing to trust here.
        let renamed = directory.path().join("source.bin");
        std::fs::copy(&path, &renamed).unwrap();
        assert!(extract(&renamed, &CancelToken::standalone()).is_none());

        // A `.tga` that is not a TGA still degrades to no preview.
        let garbage = directory.path().join("garbage.tga");
        std::fs::write(&garbage, vec![0x7F_u8; 512]).unwrap();
        assert!(extract(&garbage, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn cancellation_skips_extraction() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.png");
        std::fs::write(&path, png(64, 64)).unwrap();
        let cancel = CancelToken::standalone();
        cancel.cancel();
        assert!(extract(&path, &cancel).is_none());
    }

    #[test]
    fn preview_support_follows_the_source_extension() {
        assert!(supports_preview(Path::new("work/a.clip")));
        assert!(supports_preview(Path::new("work/a.PSB")));
        assert!(supports_preview(Path::new("work/a.webp")));
        assert!(supports_preview(Path::new("work/a.sai2")));
        assert!(supports_preview(Path::new("work/a.PROCREATE")));
        assert!(supports_preview(Path::new("work/a.tga")));
        assert!(!supports_preview(Path::new("work/a.txt")));
        assert!(!supports_preview(Path::new("work/a")));
    }
}
