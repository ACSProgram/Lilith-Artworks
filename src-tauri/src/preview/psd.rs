use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use flate2::read::ZlibDecoder;
use image::{DynamicImage, GrayImage, RgbImage};

use crate::cancel::CancelToken;

use super::{checked_pixels, decode_bytes, MAX_EDGE, MAX_SOURCE_EDGE};

const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
const THUMBNAIL_RESOURCE_PS4: u16 = 1033;
const THUMBNAIL_RESOURCE_PS5: u16 = 1036;
const THUMBNAIL_HEADER_BYTES: usize = 28;
/// Embedded thumbnails below this long edge are visibly soft in the 512 px
/// inspector; the merged composite is decoded instead when the budget allows.
const MIN_EMBEDDED_EDGE: u32 = 256;
/// The composite is sampled row by row into target-sized planes, so the memory
/// cost stays tiny; the pixel budget only bounds CPU time on pathological files.
const MAX_COMPOSITE_PIXELS: u64 = 256_000_000;

struct Header {
    version: u16,
    channels: u16,
    width: u32,
    height: u32,
    depth: u16,
    color_mode: u16,
}

/// Extract the merged preview. A sufficiently large embedded thumbnail
/// (PS 4.0/5.0) is preferred; tiny resources fall back to the composite so large
/// canvases are not shown from a 160 px bitmap. When the composite cannot be
/// decoded the embedded resource still acts as the fallback.
pub(super) fn extract(path: &Path, cancel: &CancelToken) -> Option<DynamicImage> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let header = read_header(&mut reader)?;
    if cancel.observed() {
        return None;
    }

    let color_mode_len = read_u32(&mut reader)?;
    reader
        .seek(SeekFrom::Current(i64::from(color_mode_len)))
        .ok()?;

    let resources_len = read_u32(&mut reader)?;
    let embedded = if u64::from(resources_len) <= MAX_RESOURCE_BYTES {
        let mut resources = vec![0_u8; resources_len as usize];
        reader.read_exact(&mut resources).ok()?;
        thumbnail_from_resources(&resources)
    } else {
        reader
            .seek(SeekFrom::Current(i64::from(resources_len)))
            .ok()?;
        None
    };
    if cancel.observed() {
        return None;
    }
    if embedded
        .as_ref()
        .is_some_and(|image| image.width().max(image.height()) >= MIN_EMBEDDED_EDGE)
    {
        return embedded;
    }

    // Layer and mask information: not needed for the merged composite. PSB
    // stores its length as 8 bytes, PSD as 4.
    let composite = (|| {
        let layer_mask_len = if header.version == 2 {
            read_u64(&mut reader)?
        } else {
            u64::from(read_u32(&mut reader)?)
        };
        reader
            .seek(SeekFrom::Current(i64::try_from(layer_mask_len).ok()?))
            .ok()?;
        let compression = read_u16(&mut reader)?;
        decode_composite(&mut reader, &header, compression, cancel)
    })();
    if cancel.observed() {
        return None;
    }
    composite.or(embedded)
}

fn read_header(reader: &mut impl Read) -> Option<Header> {
    let mut bytes = [0_u8; 26];
    reader.read_exact(&mut bytes).ok()?;
    if &bytes[0..4] != b"8BPS" {
        return None;
    }
    let version = be_u16(&bytes, 4);
    if !(1..=2).contains(&version) {
        return None;
    }
    let channels = be_u16(&bytes, 12);
    if channels == 0 || channels > 64 {
        return None;
    }
    let height = be_u32(&bytes, 14);
    let width = be_u32(&bytes, 18);
    let depth = be_u16(&bytes, 22);
    let color_mode = be_u16(&bytes, 24);
    if width == 0 || height == 0 {
        return None;
    }
    // The canvas budget is enforced only when the composite is actually
    // decoded: large documents commonly carry a small 1033/1036 thumbnail
    // resource, and rejecting the header would discard an affordable preview.
    Some(Header {
        version,
        channels,
        width,
        height,
        depth,
        color_mode,
    })
}

fn thumbnail_from_resources(data: &[u8]) -> Option<DynamicImage> {
    let mut cursor = 0_usize;
    while cursor + 12 <= data.len() {
        let id = be_u16(data, cursor + 4);
        let name_len = data[cursor + 6] as usize;
        let mut name_total = 1 + name_len;
        if name_total % 2 == 1 {
            name_total += 1;
        }
        let size_offset = cursor + 6 + name_total;
        if size_offset + 4 > data.len() {
            return None;
        }
        let size = be_u32(data, size_offset) as usize;
        let payload_start = size_offset + 4;
        let payload_end = payload_start.checked_add(size)?;
        if payload_end > data.len() {
            return None;
        }
        if id == THUMBNAIL_RESOURCE_PS4 || id == THUMBNAIL_RESOURCE_PS5 {
            let payload = &data[payload_start..payload_end];
            // Photoshop 4.0 stored the same resource with BGR channel order.
            if let Some(image) = decode_thumbnail_resource(payload, id == THUMBNAIL_RESOURCE_PS4) {
                return Some(image);
            }
        }
        cursor = payload_end.checked_add(size & 1)?;
    }
    None
}

/// Decode a 28-byte thumbnail header plus its payload. `bgr` is set for the
/// Photoshop 4.0 resource (1033), whose pixel order is BGR for both the raw and
/// the JPEG form ([Adobe file format specification]).
///
/// [Adobe file format specification]: https://paulbourke.net/dataformats/psdpsb/psdpsb.html
fn decode_thumbnail_resource(payload: &[u8], bgr: bool) -> Option<DynamicImage> {
    if payload.len() < THUMBNAIL_HEADER_BYTES {
        return None;
    }
    let format = be_u32(payload, 0);
    let width = be_u32(payload, 4);
    let height = be_u32(payload, 8);
    let width_bytes = be_u32(payload, 12);
    let compressed_size = be_u32(payload, 20);
    let bits = be_u16(payload, 24);
    let planes = be_u16(payload, 26);
    let body = &payload[THUMBNAIL_HEADER_BYTES..];

    let image = match format {
        // kJpegRGB
        1 => {
            let end = if compressed_size > 0 && (compressed_size as usize) <= body.len() {
                compressed_size as usize
            } else {
                body.len()
            };
            decode_bytes(&body[..end])?
        }
        // kRawRGB
        0 => {
            checked_pixels(width, height)?;
            if bits != 24 || planes != 1 {
                return None;
            }
            let row_bytes = width_bytes as usize;
            let pixel_bytes = (width as usize).checked_mul(3)?;
            if row_bytes < pixel_bytes {
                return None;
            }
            let needed = row_bytes.checked_mul(height as usize)?;
            if body.len() < needed {
                return None;
            }
            let mut rgb = Vec::with_capacity(pixel_bytes * height as usize);
            for row in body[..needed].chunks_exact(row_bytes) {
                rgb.extend_from_slice(&row[..pixel_bytes]);
            }
            DynamicImage::ImageRgb8(RgbImage::from_raw(width, height, rgb)?)
        }
        _ => return None,
    };
    Some(if bgr { swap_red_blue(image) } else { image })
}

fn swap_red_blue(image: DynamicImage) -> DynamicImage {
    let mut rgb = image.to_rgb8();
    for pixel in rgb.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    DynamicImage::ImageRgb8(rgb)
}

fn decode_composite(
    reader: &mut impl Read,
    header: &Header,
    compression: u16,
    cancel: &CancelToken,
) -> Option<DynamicImage> {
    checked_composite_pixels(header.width, header.height)?;
    if !matches!(header.depth, 8 | 16) {
        return None;
    }
    let bytes_per_sample = if header.depth == 8 { 1 } else { 2 };
    // Extra channels can be transparency or spot channels and the header does
    // not distinguish them. Treating a spot channel as alpha would blank out
    // most of an opaque image, so the preview always shows the colour planes.
    let components = match header.color_mode {
        // Grayscale
        1 => 1_usize,
        // RGB
        3 => 3_usize,
        // CMYK
        4 => 4_usize,
        _ => return None,
    };
    if components > header.channels as usize {
        return None;
    }
    let width = header.width as usize;
    let height = header.height as usize;
    let row_bytes = width.checked_mul(bytes_per_sample)?;
    if row_bytes == 0 {
        return None;
    }
    let (target_width, target_height) = target_dimensions(header.width, header.height);
    let target_width_usize = target_width as usize;
    let target_height_usize = target_height as usize;
    let plane_len = target_width_usize.checked_mul(target_height_usize)?;
    let x_indices = sample_indices(target_width_usize, width);
    let mut planes = (0..components)
        .map(|_| vec![0_u8; plane_len])
        .collect::<Vec<_>>();
    let mut row = vec![0_u8; row_bytes];

    match compression {
        // Raw
        0 => {
            for plane in planes.iter_mut() {
                for y in 0..height {
                    if y % 32 == 0 && cancel.observed() {
                        return None;
                    }
                    reader.read_exact(&mut row).ok()?;
                    sample_row(
                        &row,
                        y,
                        height,
                        target_height_usize,
                        &x_indices,
                        bytes_per_sample,
                        plane,
                    );
                }
            }
        }
        // RLE (PackBits)
        1 => {
            let count_size = if header.version == 2 { 4 } else { 2 };
            let count_count = (header.channels as usize).checked_mul(height)?;
            let mut counts = Vec::with_capacity(count_count);
            let mut count_bytes = [0_u8; 4];
            for _ in 0..count_count {
                reader.read_exact(&mut count_bytes[..count_size]).ok()?;
                counts.push(if count_size == 4 {
                    be_u32(&count_bytes, 0)
                } else {
                    u32::from(be_u16(&count_bytes, 0))
                });
            }
            let max_packed = row_bytes
                .saturating_mul(2)
                .saturating_add(row_bytes / 128)
                .saturating_add(64);
            let mut packed = Vec::new();
            for (channel, plane) in planes.iter_mut().enumerate() {
                for y in 0..height {
                    if y % 32 == 0 && cancel.observed() {
                        return None;
                    }
                    let packed_len = counts[channel * height + y] as usize;
                    if packed_len > max_packed {
                        return None;
                    }
                    packed.resize(packed_len, 0);
                    reader.read_exact(&mut packed).ok()?;
                    decode_packbits(&packed, &mut row)?;
                    sample_row(
                        &row,
                        y,
                        height,
                        target_height_usize,
                        &x_indices,
                        bytes_per_sample,
                        plane,
                    );
                }
            }
        }
        // ZIP, optionally with per-row prediction
        2 | 3 => {
            let predicted = compression == 3;
            let mut decoder = ZlibDecoder::new(reader);
            for plane in planes.iter_mut() {
                for y in 0..height {
                    if y % 32 == 0 && cancel.observed() {
                        return None;
                    }
                    decoder.read_exact(&mut row).ok()?;
                    if predicted {
                        for index in 1..row.len() {
                            row[index] = row[index].wrapping_add(row[index - 1]);
                        }
                    }
                    sample_row(
                        &row,
                        y,
                        height,
                        target_height_usize,
                        &x_indices,
                        bytes_per_sample,
                        plane,
                    );
                }
            }
        }
        _ => return None,
    }

    assemble(
        planes,
        components,
        header.color_mode,
        target_width,
        target_height,
    )
}

fn checked_composite_pixels(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 || width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    (pixels <= MAX_COMPOSITE_PIXELS).then_some(pixels)
}

fn target_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_EDGE && height <= MAX_EDGE {
        return (width, height);
    }
    if width >= height {
        let scaled =
            (u64::from(height) * u64::from(MAX_EDGE) + u64::from(width) / 2) / u64::from(width);
        (MAX_EDGE, scaled.max(1) as u32)
    } else {
        let scaled =
            (u64::from(width) * u64::from(MAX_EDGE) + u64::from(height) / 2) / u64::from(height);
        (scaled.max(1) as u32, MAX_EDGE)
    }
}

fn sample_indices(target: usize, source: usize) -> Vec<usize> {
    (0..target)
        .map(|index| {
            let source_index = (index as u64 * source as u64 + source as u64 / 2) / target as u64;
            (source_index as usize).min(source.saturating_sub(1))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn sample_row(
    row: &[u8],
    source_y: usize,
    source_height: usize,
    target_height: usize,
    x_indices: &[usize],
    bytes_per_sample: usize,
    plane: &mut [u8],
) {
    let target_y = (source_y as u64 * target_height as u64 / source_height as u64) as usize;
    if target_y >= target_height {
        return;
    }
    let target_width = x_indices.len();
    let row_start = target_y * target_width;
    for (target_x, &source_x) in x_indices.iter().enumerate() {
        // 16-bit samples are big-endian; the high byte is the 8-bit preview.
        plane[row_start + target_x] = row[source_x * bytes_per_sample];
    }
}

fn decode_packbits(packed: &[u8], output: &mut [u8]) -> Option<()> {
    let mut input = 0_usize;
    let mut position = 0_usize;
    while position < output.len() {
        let header = *packed.get(input)? as i8;
        input += 1;
        if header >= 0 {
            let count = header as usize + 1;
            if input + count > packed.len() || position + count > output.len() {
                return None;
            }
            output[position..position + count].copy_from_slice(&packed[input..input + count]);
            input += count;
            position += count;
        } else if header != -128 {
            let count = (1 - header as i16) as usize;
            let value = *packed.get(input)?;
            input += 1;
            if position + count > output.len() {
                return None;
            }
            output[position..position + count].fill(value);
            position += count;
        }
    }
    Some(())
}

fn assemble(
    planes: Vec<Vec<u8>>,
    components: usize,
    color_mode: u16,
    width: u32,
    height: u32,
) -> Option<DynamicImage> {
    let len = width as usize * height as usize;
    match color_mode {
        1 => Some(DynamicImage::ImageLuma8(GrayImage::from_raw(
            width,
            height,
            planes.into_iter().next()?,
        )?)),
        3 => {
            let mut pixels = Vec::with_capacity(len * 3);
            for index in 0..len {
                pixels.push(planes[0][index]);
                pixels.push(planes[1][index]);
                pixels.push(planes[2][index]);
            }
            Some(DynamicImage::ImageRgb8(RgbImage::from_raw(
                width, height, pixels,
            )?))
        }
        4 if components == 4 => {
            let mut pixels = Vec::with_capacity(len * 3);
            for index in 0..len {
                let k = u16::from(planes[3][index]);
                let c = u16::from(planes[0][index]) * (255 - k) / 255;
                let m = u16::from(planes[1][index]) * (255 - k) / 255;
                let y = u16::from(planes[2][index]) * (255 - k) / 255;
                pixels.push(255 - c as u8);
                pixels.push(255 - m as u8);
                pixels.push(255 - y as u8);
            }
            Some(DynamicImage::ImageRgb8(RgbImage::from_raw(
                width, height, pixels,
            )?))
        }
        _ => None,
    }
}

fn read_u16(reader: &mut impl Read) -> Option<u16> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes).ok()?;
    Some(u16::from_be_bytes(bytes))
}

fn read_u32(reader: &mut impl Read) -> Option<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes).ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Option<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes).ok()?;
    Some(u64::from_be_bytes(bytes))
}

fn be_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("2-byte field"))
}

fn be_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("4-byte field"))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use image::GenericImageView;

    use super::*;

    fn header(width: u32, height: u32, channels: u16, depth: u16, color_mode: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BPS");
        bytes.extend_from_slice(&1_u16.to_be_bytes());
        bytes.extend_from_slice(&[0_u8; 6]);
        bytes.extend_from_slice(&channels.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&depth.to_be_bytes());
        bytes.extend_from_slice(&color_mode.to_be_bytes());
        bytes
    }

    fn resource(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BIM");
        bytes.extend_from_slice(&id.to_be_bytes());
        // Empty Pascal name padded to an even length.
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            bytes.push(0);
        }
        bytes
    }

    fn jpeg_thumbnail(width: u32, height: u32) -> Vec<u8> {
        let jpeg = {
            let image = RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 90]));
            let mut bytes = Vec::new();
            image
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
                .unwrap();
            bytes
        };
        let mut thumbnail = Vec::new();
        thumbnail.extend_from_slice(&1_u32.to_be_bytes());
        thumbnail.extend_from_slice(&width.to_be_bytes());
        thumbnail.extend_from_slice(&height.to_be_bytes());
        thumbnail.extend_from_slice(&(width * 3).to_be_bytes());
        thumbnail.extend_from_slice(&((28 + jpeg.len()) as u32).to_be_bytes());
        thumbnail.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
        thumbnail.extend_from_slice(&24_u16.to_be_bytes());
        thumbnail.extend_from_slice(&1_u16.to_be_bytes());
        thumbnail.extend_from_slice(&jpeg);
        thumbnail
    }

    fn psd(resources: &[u8], image_data: &[u8]) -> Vec<u8> {
        psd_with_size(3, 2, resources, image_data)
    }

    fn psd_with_size(width: u32, height: u32, resources: &[u8], image_data: &[u8]) -> Vec<u8> {
        let mut bytes = header(width, height, 3, 8, 3);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&(resources.len() as u32).to_be_bytes());
        bytes.extend_from_slice(resources);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(image_data);
        bytes
    }

    fn planar_rgb() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u16.to_be_bytes());
        // R plane, then G plane, then B plane.
        bytes.extend_from_slice(&[255, 255, 255, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        bytes
    }

    fn write_psd(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (directory, path)
    }

    #[test]
    fn composite_raw_is_decoded_without_layers() {
        let (_directory, path) = write_psd("raw.psd", &psd(&[], &planar_rgb()));
        let image = extract(&path, &CancelToken::standalone())
            .unwrap()
            .to_rgb8();
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(image.get_pixel(2, 1).0, [0, 255, 0]);
    }

    #[test]
    fn composite_rle_is_decoded() {
        let mut image_data = vec![0, 1];
        // One PackBits length per channel-row; each row is a literal run.
        for _ in 0..6 {
            image_data.extend_from_slice(&4_u16.to_be_bytes());
        }
        let rows: [[u8; 3]; 6] = [
            [255, 255, 255],
            [0, 0, 0],
            [0, 0, 0],
            [255, 255, 255],
            [0, 0, 0],
            [0, 0, 0],
        ];
        for row in rows {
            // Header 2 = literal run of three bytes.
            image_data.extend_from_slice(&[2, row[0], row[1], row[2]]);
        }
        let (_directory, path) = write_psd("rle.psd", &psd(&[], &image_data));
        let image = extract(&path, &CancelToken::standalone())
            .unwrap()
            .to_rgb8();
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(image.get_pixel(0, 1).0, [0, 255, 0]);
    }

    #[test]
    fn composite_zip_is_decoded() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&[
                255, 255, 255, 0, 0, 0, 0, 0, 0, 255, 255, 255, 0, 0, 0, 0, 0, 0,
            ])
            .unwrap();
        let compressed = encoder.finish().unwrap();
        let mut image_data = vec![0, 2];
        image_data.extend_from_slice(&compressed);
        let (_directory, path) = write_psd("zip.psd", &psd(&[], &image_data));
        let image = extract(&path, &CancelToken::standalone())
            .unwrap()
            .to_rgb8();
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0]);
    }

    #[test]
    fn a_large_embedded_thumbnail_wins_over_the_composite() {
        let resources = resource(THUMBNAIL_RESOURCE_PS5, &jpeg_thumbnail(300, 150));
        let (_directory, path) = write_psd("resource.psd", &psd(&resources, &planar_rgb()));
        let image = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!(image.dimensions(), (300, 150));
    }

    #[test]
    fn a_tiny_embedded_thumbnail_falls_back_to_the_composite() {
        let resources = resource(THUMBNAIL_RESOURCE_PS5, &jpeg_thumbnail(40, 20));
        let (_directory, path) = write_psd("soft-resource.psd", &psd(&resources, &planar_rgb()));
        let image = extract(&path, &CancelToken::standalone())
            .unwrap()
            .to_rgb8();
        assert_eq!(image.dimensions(), (3, 2));
    }

    #[test]
    fn an_oversized_canvas_uses_its_embedded_thumbnail() {
        // 400 MP: beyond the composite budget, so the embedded resource is the
        // only affordable path even though it is small.
        let resources = resource(THUMBNAIL_RESOURCE_PS5, &jpeg_thumbnail(64, 32));
        let bytes = psd_with_size(20_000, 20_000, &resources, &[]);
        let (_directory, path) = write_psd("large.psd", &bytes);
        let image = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!(image.dimensions(), (64, 32));
    }

    #[test]
    fn an_oversized_canvas_without_an_embedded_thumbnail_stays_without_preview() {
        let bytes = psd_with_size(20_000, 20_000, &[], &planar_rgb());
        let (_directory, path) = write_psd("large-composite.psd", &bytes);
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn photoshop_4_raw_thumbnail_is_swapped_from_bgr() {
        let mut thumbnail = Vec::new();
        thumbnail.extend_from_slice(&0_u32.to_be_bytes()); // kRawRGB
        thumbnail.extend_from_slice(&2_u32.to_be_bytes()); // width
        thumbnail.extend_from_slice(&1_u32.to_be_bytes()); // height
        thumbnail.extend_from_slice(&8_u32.to_be_bytes()); // widthbytes = 2 * 3 padded to 4
        thumbnail.extend_from_slice(&8_u32.to_be_bytes()); // total size
        thumbnail.extend_from_slice(&0_u32.to_be_bytes()); // uncompressed
        thumbnail.extend_from_slice(&24_u16.to_be_bytes());
        thumbnail.extend_from_slice(&1_u16.to_be_bytes());
        // BGR pixels: blue first, then red, plus two padding bytes.
        thumbnail.extend_from_slice(&[255, 0, 0, 0, 0, 255, 9, 9]);

        let resources = resource(THUMBNAIL_RESOURCE_PS4, &thumbnail);
        // No composite: the raw resource is the only preview available.
        let (_directory, path) = write_psd("ps4.psd", &psd(&resources, &[]));
        let image = extract(&path, &CancelToken::standalone())
            .unwrap()
            .to_rgb8();
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [255, 0, 0]);
    }

    #[test]
    fn truncated_psd_yields_no_preview() {
        let bytes = psd(&[], &planar_rgb());
        let (_directory, path) = write_psd("truncated.psd", &bytes[..bytes.len() - 4]);
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }
}
