//! Thumbnails stored inside ZIP containers.
//!
//! Only `.procreate` is routed here for now. The layout knowledge lives in the
//! candidate table below, so adding another ZIP family later means adding a row
//! rather than touching the dispatch in `preview::extract`.
//!
//! Every container in this family is opened read-only, one entry at a time:
//! the uncompressed size from the central directory is checked first, the read
//! itself is capped again with `take`, and the cancellation token is polled
//! between chunks. The extracted bytes then go through the shared decode budget
//! in `preview::decode_bytes`, so a ZIP bomb or a malformed image inside an
//! otherwise valid container still yields `None` instead of work.

use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};

use image::DynamicImage;

use crate::cancel::CancelToken;

use super::{decode_bytes, extension_lower};

/// Upper bound for one uncompressed entry. A `.procreate` thumbnail is a few
/// hundred kilobytes; the cap only guards against pathological archives.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Upper bound for the entry count. Real files are in the tens of entries; a
/// huge count means this is not the archive we are looking for.
const MAX_ENTRIES: usize = 65_536;
const READ_CHUNK: usize = 64 * 1024;

/// Candidate entries per extension, tried in order. Names are matched
/// case-insensitively against the central directory.
struct Layout {
    extension: &'static str,
    candidates: &'static [&'static str],
}

const LAYOUTS: &[Layout] = &[Layout {
    extension: "procreate",
    // Procreate writes its document preview here; the key is the thumbnail of
    // the flattened canvas.
    candidates: &["quicklook/thumbnail.png"],
}];

/// Extract a preview from a ZIP container. Returns `None` for any extension this
/// module does not own, so a tracked `.zip` / `.jar` never gets scanned.
pub(super) fn extract(path: &Path, cancel: &CancelToken) -> Option<DynamicImage> {
    if cancel.observed() {
        return None;
    }
    let layout = layout_for(path)?;
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    if archive.len() > MAX_ENTRIES {
        return None;
    }
    for candidate in layout.candidates {
        let Some(index) = find_entry(&mut archive, candidate) else {
            continue;
        };
        let Some(bytes) = read_entry(&mut archive, index, cancel) else {
            continue;
        };
        if let Some(image) = decode_bytes(&bytes) {
            return Some(image);
        }
    }
    None
}

fn layout_for(path: &Path) -> Option<&'static Layout> {
    let extension = extension_lower(path)?;
    LAYOUTS
        .iter()
        .find(|layout| layout.extension == extension.as_str())
}

/// Index of `candidate` in the central directory, compared case-insensitively.
fn find_entry<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, candidate: &str) -> Option<usize> {
    archive
        .file_names()
        .position(|name| name.eq_ignore_ascii_case(candidate))
}

/// Read one entry into memory, bounded by [`MAX_ENTRY_BYTES`].
fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    cancel: &CancelToken,
) -> Option<Vec<u8>> {
    let entry = archive.by_index(index).ok()?;
    if entry.is_dir() || entry.encrypted() {
        return None;
    }
    let declared = entry.size();
    if declared == 0 || declared > MAX_ENTRY_BYTES {
        return None;
    }
    // The declared size comes from the archive itself, so it is only a hint:
    // the buffer starts small, grows by what is actually read, and the stream is
    // capped again so a lying header cannot reserve or hand back more than
    // `MAX_ENTRY_BYTES`.
    let mut bytes = Vec::with_capacity(declared.min(READ_CHUNK as u64) as usize);
    let mut chunk = vec![0_u8; READ_CHUNK];
    let mut stream = entry.take(MAX_ENTRY_BYTES + 1);
    loop {
        if cancel.observed() {
            return None;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return None;
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal ZIP with a single stored (uncompressed) entry, written by hand so
    /// the fixture does not depend on the `zip` write path.
    fn stored_zip(name: &str, payload: &[u8]) -> Vec<u8> {
        fn u16le(out: &mut Vec<u8>, value: u16) {
            out.extend_from_slice(&value.to_le_bytes());
        }
        fn u32le(out: &mut Vec<u8>, value: u32) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        let crc = crc32(payload);
        let size = payload.len() as u32;
        let name_len = name.len() as u16;
        let mut out = Vec::new();

        // Local file header.
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20); // version needed
        u16le(&mut out, 0); // flags
        u16le(&mut out, 0); // stored
        u16le(&mut out, 0); // time
        u16le(&mut out, 0); // date
        u32le(&mut out, crc);
        u32le(&mut out, size);
        u32le(&mut out, size);
        u16le(&mut out, name_len);
        u16le(&mut out, 0); // extra length
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(payload);

        let central_start = out.len() as u32;
        // Central directory header.
        u32le(&mut out, 0x0201_4b50);
        u16le(&mut out, 20); // version made by
        u16le(&mut out, 20); // version needed
        u16le(&mut out, 0); // flags
        u16le(&mut out, 0); // stored
        u16le(&mut out, 0); // time
        u16le(&mut out, 0); // date
        u32le(&mut out, crc);
        u32le(&mut out, size);
        u32le(&mut out, size);
        u16le(&mut out, name_len);
        u16le(&mut out, 0); // extra
        u16le(&mut out, 0); // comment
        u16le(&mut out, 0); // disk
        u16le(&mut out, 0); // internal attributes
        u32le(&mut out, 0); // external attributes
        u32le(&mut out, 0); // local header offset
        out.extend_from_slice(name.as_bytes());
        let central_end = out.len() as u32;

        // End of central directory.
        u32le(&mut out, 0x0605_4b50);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, 1); // entries on this disk
        u16le(&mut out, 1); // entries total
        u32le(&mut out, central_end - central_start);
        u32le(&mut out, central_start);
        u16le(&mut out, 0);
        out
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let buffer: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 40, 255]));
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        buffer
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn write_temp(extension: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("source.{extension}"));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        (directory, path)
    }

    #[test]
    fn procreate_quicklook_thumbnail_is_extracted() {
        let (_directory, path) = write_temp(
            "procreate",
            &stored_zip("QuickLook/Thumbnail.png", &png(300, 120)),
        );
        let image = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!((image.width(), image.height()), (300, 120));
    }

    #[test]
    fn entry_names_are_matched_case_insensitively() {
        let (_directory, path) = write_temp(
            "procreate",
            &stored_zip("quicklook/thumbnail.PNG", &png(64, 64)),
        );
        assert!(extract(&path, &CancelToken::standalone()).is_some());
    }

    #[test]
    fn archives_without_the_entry_yield_no_preview() {
        let (_directory, path) =
            write_temp("procreate", &stored_zip("Other/thing.png", &png(8, 8)));
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn extensions_outside_the_family_are_not_scanned() {
        let (_directory, path) =
            write_temp("zip", &stored_zip("QuickLook/Thumbnail.png", &png(32, 32)));
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn cancellation_skips_extraction() {
        let (_directory, path) = write_temp(
            "procreate",
            &stored_zip("QuickLook/Thumbnail.png", &png(32, 32)),
        );
        let cancel = CancelToken::standalone();
        cancel.cancel();
        assert!(extract(&path, &cancel).is_none());
    }

    #[test]
    fn malformed_archives_are_rejected_without_panicking() {
        let mut rng = crate::fuzz_support::Rng::new(0x5A12_2A11);
        let valid = stored_zip("QuickLook/Thumbnail.png", &png(16, 16));
        for _ in 0..64 {
            let mut bytes = valid.clone();
            let mutations = 1 + rng.below(8);
            for _ in 0..mutations {
                let index = rng.below(bytes.len());
                bytes[index] = (rng.next_u64() & 0xFF) as u8;
            }
            let (_directory, path) = write_temp("procreate", &bytes);
            // Either outcome is fine; not panicking and not hanging is the point.
            let _ = extract(&path, &CancelToken::standalone());
        }
    }
}
