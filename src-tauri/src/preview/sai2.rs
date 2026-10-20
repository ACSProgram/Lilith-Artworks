//! `.sai2` canvas thumbnails.
//!
//! A `.sai2` file is a 64-byte canvas header followed by a table of
//! `{ tag, layer, absolute offset }` entries; each blob runs to the next entry's
//! offset (the last one to EOF). Two entries carry a preview of the canvas:
//!
//! - `thum`: an old SAI image format (`jssf`), which is a quantisation table
//!   pair plus one entropy-coded MCU row per 8 scanlines. It is repacked into a
//!   standard baseline JPEG and handed to the shared decoder.
//! - `intg`: delta-compressed pixels (`dpcm`), 256×256 tiles, each tile row
//!   first RLE-packed and then predicted like PNG's `Up` filter at 16 bpc.
//!
//! Layout and algorithms are a direct port of libsai (MIT, Copyright (c) 2025
//! Wunkolo), `include/sai2.hpp` and `source/sai2.cpp`. The port is deliberately
//! literal: the bit-level bookkeeping of the RLE stream and the exact JPEG
//! segment order both have to match what SAI wrote, so restructuring the
//! reference would only add ways to be subtly wrong. The one liberty taken is
//! rewriting provably equivalent arithmetic into its readable form, each spot
//! called out in a comment; `intg` output stays bit-identical to SAI's own PNG
//! export on the sample canvases.
//!
//! MIT permits relicensing into this project's GPL-3.0-only; the upstream
//! notice is kept here and the port is recorded in `THIRD_PARTY_NOTICES.md`.
//!
//! SPDX-FileCopyrightText: Copyright (c) 2025 Wunkolo
//! SPDX-License-Identifier: GPL-3.0-only

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use image::{DynamicImage, RgbaImage};

use crate::cancel::CancelToken;

use super::{checked_pixels, decode_bytes};

/// Canvas magic; exactly 16 bytes, so it fits the shared probe buffer.
pub(super) const IDENT: &[u8; 16] = b"SAI-CANVAS-TYPE0";
const HEADER_LEN: u64 = 64;
const ENTRY_LEN: u64 = 16;
/// Real files carry tens to low hundreds of entries; anything far beyond that is
/// not a canvas we want to walk.
const MAX_TABLE_COUNT: u32 = 4096;
/// Upper bound for a single blob. A thumbnail blob is a few hundred kilobytes
/// and an `intg` blob a few megabytes; the cap only bounds pathological input.
const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;
const TILE_SIZE: u32 = 256;

const THUM_TAG: &[u8; 4] = b"thum";
const INTG_TAG: &[u8; 4] = b"intg";
const JSSF_TAG: &[u8; 4] = b"jssf";
const DPCM_TAG: &[u8; 4] = b"dpcm";

#[derive(Debug, Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    table_count: u32,
    background_flags: u8,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: [u8; 4],
    offset: u64,
}

/// Extract the canvas preview.
///
/// `intg` holds the canvas itself and `thum` a fixed ~256 px preview. The shared
/// output pipeline scales up anything smaller than its 512 px target, so taking
/// `thum` first would store a blurry upscale while the full-resolution source
/// sits in the same file. `intg` is therefore preferred whenever it fits the
/// decode budget; `thum` stays as the fallback for oversized canvases and for
/// the old variants whose `intg` layout we cannot read.
pub(super) fn extract(path: &Path, cancel: &CancelToken) -> Option<DynamicImage> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut header_bytes = [0_u8; HEADER_LEN as usize];
    file.read_exact(&mut header_bytes).ok()?;
    let header = parse_header(&header_bytes)?;
    if cancel.observed() {
        return None;
    }
    let entries = read_table(&mut file, header)?;

    for tag in [INTG_TAG, THUM_TAG] {
        let Some(index) = entries.iter().position(|entry| &entry.tag == tag) else {
            continue;
        };
        if cancel.observed() {
            return None;
        }
        let Some(blob) = read_blob(&mut file, &entries, index, file_len) else {
            continue;
        };
        let decoded = if tag == INTG_TAG {
            decode_intg(&blob, header)
        } else {
            decode_thum(&blob)
        };
        if let Some(image) = decoded {
            return Some(image);
        }
    }
    None
}

fn parse_header(bytes: &[u8]) -> Option<Header> {
    if bytes.len() < HEADER_LEN as usize {
        return None;
    }
    if &bytes[..16] != IDENT {
        return None;
    }
    // Identifier[16], Flags0, CanvasBackgroundFlags, Flags2, Flags3, then the
    // u32 fields: Width, Height, PrintingResolution, TableCount, SelectedLayer.
    let background_flags = bytes[17];
    let width = u32::from_le_bytes(bytes[20..24].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let table_count = u32::from_le_bytes(bytes[32..36].try_into().ok()?);
    if table_count == 0 || table_count > MAX_TABLE_COUNT {
        return None;
    }
    Some(Header {
        width,
        height,
        table_count,
        background_flags,
    })
}

fn read_table(file: &mut File, header: Header) -> Option<Vec<Entry>> {
    let mut bytes = vec![0_u8; header.table_count as usize * ENTRY_LEN as usize];
    file.read_exact(&mut bytes).ok()?;
    let mut entries = Vec::with_capacity(header.table_count as usize);
    for chunk in bytes.chunks_exact(ENTRY_LEN as usize) {
        let tag = <[u8; 4]>::try_from(&chunk[..4]).ok()?;
        let offset = u64::from_le_bytes(chunk[8..16].try_into().ok()?);
        entries.push(Entry { tag, offset });
    }
    Some(entries)
}

/// Read the blob of `index`: it runs from its own offset to the next entry's
/// offset, and the last entry runs to EOF.
fn read_blob(file: &mut File, entries: &[Entry], index: usize, file_len: u64) -> Option<Vec<u8>> {
    let start = entries.get(index)?.offset;
    let end = entries
        .get(index + 1)
        .map_or(file_len, |entry| entry.offset);
    let table_end = HEADER_LEN + entries.len() as u64 * ENTRY_LEN;
    if start < table_end || end <= start || end > file_len {
        return None;
    }
    if end - start > MAX_BLOB_BYTES {
        return None;
    }
    let mut blob = vec![0_u8; (end - start) as usize];
    file.seek(SeekFrom::Start(start)).ok()?;
    file.read_exact(&mut blob).ok()?;
    Some(blob)
}

/// The `thum` blob carries its own dimensions, so the canvas header is not used.
fn decode_thum(blob: &[u8]) -> Option<DynamicImage> {
    let jpeg = jssf_to_jpeg(blob)?;
    decode_bytes(&jpeg)
}

// -- `thum`: JSSF → baseline JPEG ---------------------------------------------

/// Default baseline Huffman tables, as emitted by SAI inside the JSSF stream.
/// Each table is `id + 16 code-length counters + codes`.
const HUFFMAN_DC_LUMA: [u8; 29] = [
    0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
];

const HUFFMAN_AC_LUMA: [u8; 179] = [
    0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01,
    0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61,
    0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1,
    0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27,
    0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88,
    0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6,
    0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4,
    0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
    0xF8, 0xF9, 0xFA,
];

const HUFFMAN_DC_CHROMA: [u8; 29] = [
    0x01, // table class 0, id 1
    0x00, 0x03, // code lengths 1, 2
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // code lengths 3..11
    0x00, 0x00, 0x00, 0x00, 0x00, // code lengths 12..16
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
];

const HUFFMAN_AC_CHROMA: [u8; 179] = [
    0x11, 0x00, 0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01, 0x02,
    0x77, 0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61,
    0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52,
    0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A,
    0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47,
    0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67,
    0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86,
    0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4,
    0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2,
    0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9,
    0xDA, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
    0xF8, 0xF9, 0xFA,
];

/// `thum` blob → standalone baseline JPEG bytes.
///
/// Blob layout: `u32 width, u32 height (unused), "jssf", u16 width, u16 height,
/// u16 channels`, then the luma quantisation table, the chroma table for
/// multi-channel canvases, and one `u16`-length-prefixed entropy segment per
/// MCU row.
fn jssf_to_jpeg(blob: &[u8]) -> Option<Vec<u8>> {
    /// Header before the quantisation tables.
    const PREFIX: usize = 18;
    if blob.len() < PREFIX || &blob[8..12] != JSSF_TAG {
        return None;
    }
    let width = u16::from_le_bytes([blob[12], blob[13]]);
    let height = u16::from_le_bytes([blob[14], blob[15]]);
    let channels = u16::from_le_bytes([blob[16], blob[17]]);
    if width == 0 || height == 0 || !(1..=4).contains(&channels) {
        return None;
    }
    checked_pixels(u32::from(width), u32::from(height))?;

    let mut cursor = PREFIX;
    let luma = take_slice(blob, &mut cursor, 64)?;
    let chroma = if channels > 1 {
        Some(take_slice(blob, &mut cursor, 64)?)
    } else {
        None
    };

    let mut jpeg = Vec::with_capacity(blob.len() + 1024);
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI

    // DQT: one table per colour plane (luma id 0, chroma id 1).
    let dqt_length = if chroma.is_some() { 132 } else { 67 };
    push_segment(&mut jpeg, 0xDB, dqt_length);
    jpeg.push(0x00);
    jpeg.extend_from_slice(luma);
    if let Some(chroma) = chroma {
        jpeg.push(0x01);
        jpeg.extend_from_slice(chroma);
    }

    // DHT: the four default tables, always expressed as (DC, AC) pairs.
    let dht_length = if chroma.is_some() { 418 } else { 210 };
    push_segment(&mut jpeg, 0xC4, dht_length);
    jpeg.extend_from_slice(&HUFFMAN_DC_LUMA);
    jpeg.extend_from_slice(&HUFFMAN_AC_LUMA);
    if chroma.is_some() {
        jpeg.extend_from_slice(&HUFFMAN_DC_CHROMA);
        jpeg.extend_from_slice(&HUFFMAN_AC_CHROMA);
    }

    // SOF0: baseline, 8-bit, 1×1 sampling for every component (4:4:4).
    push_segment(&mut jpeg, 0xC0, 8 + channels * 3);
    jpeg.push(8);
    jpeg.extend_from_slice(&height.to_be_bytes());
    jpeg.extend_from_slice(&width.to_be_bytes());
    jpeg.push(channels as u8);
    for channel in 0..channels {
        jpeg.push(channel as u8 + 1);
        jpeg.push(0x11);
        jpeg.push(u8::from(channel != 0));
    }

    // DRI: one restart interval per MCU row.
    push_segment(&mut jpeg, 0xDD, 4);
    jpeg.extend_from_slice(&(width.div_ceil(8)).to_be_bytes());

    // SOS: component 1 uses the luma tables, the rest the chroma tables.
    push_segment(&mut jpeg, 0xDA, 6 + channels * 2);
    jpeg.push(channels as u8);
    for channel in 0..channels {
        jpeg.push(channel as u8 + 1);
        jpeg.push(if channel == 0 { 0x00 } else { 0x11 });
    }
    jpeg.extend_from_slice(&[0x00, 0x3F, 0x00]);

    let rows = height.div_ceil(8);
    for row in 0..rows {
        let length = u16::from_le_bytes([*blob.get(cursor)?, *blob.get(cursor + 1)?]);
        cursor += 2;
        let data = take_slice(blob, &mut cursor, usize::from(length))?;
        jpeg.extend_from_slice(data);
        // One restart marker between rows: the entropy segments restart at byte
        // boundaries, which is exactly what DRI promises the decoder below.
        if row + 1 < rows {
            jpeg.extend_from_slice(&[0xFF, 0xD0 | (row & 0b111) as u8]);
        }
    }
    jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
    Some(jpeg)
}

fn push_segment(out: &mut Vec<u8>, marker: u8, length: u16) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&length.to_be_bytes());
}

fn take_slice<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(length)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

// -- `intg`: DPCM tiles --------------------------------------------------------

fn decode_intg(blob: &[u8], header: Header) -> Option<DynamicImage> {
    if blob.len() < 4 || &blob[..4] != DPCM_TAG {
        return None;
    }
    // 3 channels minimum, four when the canvas has a transparent background.
    let channels = if header.background_flags & 7 == 0 {
        4
    } else {
        3
    };
    let width = header.width;
    let height = header.height;
    checked_pixels(width, height)?;

    let tiles_x = width.div_ceil(TILE_SIZE);
    let tiles_y = height.div_ceil(TILE_SIZE);
    let tile_count = (tiles_x as usize).checked_mul(tiles_y as usize)?;
    let mut cursor = 4 + tile_count.checked_mul(4)?;
    if blob.len() < cursor {
        return None;
    }
    let mut tile_sizes = Vec::with_capacity(tile_count);
    for index in 0..tile_count {
        let start = 4 + index * 4;
        let size: [u8; 4] = blob.get(start..start + 4)?.try_into().ok()?;
        tile_sizes.push(u32::from_le_bytes(size));
    }

    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    for tile_y in 0..tiles_y {
        let y0 = tile_y * TILE_SIZE;
        let rows = (height - y0).min(TILE_SIZE);
        for tile_x in 0..tiles_x {
            let size = tile_sizes[(tile_y * tiles_x + tile_x) as usize] as usize;
            let end = cursor.checked_add(size)?;
            let tile = blob.get(cursor..end)?;
            if size < 2 {
                return None;
            }
            let x0 = tile_x * TILE_SIZE;
            let cols = (width - x0).min(TILE_SIZE);
            decode_tile(&mut pixels, width, tile, x0, y0, cols, rows, channels)?;
            cursor += size;
        }
        // Each tile row is followed by one more checksum whose high byte is the
        // tile column count.
        cursor = cursor.checked_add(2)?;
        if cursor > blob.len() {
            return None;
        }
    }

    RgbaImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgba8)
}

/// Decode one 256×256 tile into `out` (RGBA8, full canvas stride).
///
/// A tile is `u16 checksum` followed by `rows` independently packed rows. The
/// vertical predictor never crosses a tile boundary, so each tile starts from a
/// zeroed previous row.
#[allow(clippy::too_many_arguments)]
fn decode_tile(
    out: &mut [u8],
    width: u32,
    tile: &[u8],
    x0: u32,
    y0: u32,
    cols: u32,
    rows: u32,
    channels: usize,
) -> Option<()> {
    let cols = cols as usize;
    let mut cursor = 2;
    let mut previous_row = vec![0_u32; cols];
    let mut row = vec![0_u32; cols];
    let mut deltas = vec![0_i16; cols * 4];
    for row_index in 0..rows {
        // Unused lanes keep the reference's fill value; the alpha lane of a
        // 3-channel canvas is overridden when the row is written out.
        deltas.fill(-1);
        let consumed = unpack_delta_rle16(tile.get(cursor..)?, &mut deltas, cols, 4, channels)?;
        if consumed == 0 {
            return None;
        }
        cursor = cursor.checked_add(consumed)?;
        if cursor > tile.len() {
            return None;
        }
        delta_unpack_row16(&mut row, &previous_row, &deltas, cols);

        let base = ((y0 + row_index) as usize * width as usize + x0 as usize) * 4;
        let target = out.get_mut(base..base + cols * 4)?;
        for (index, pixel) in row.iter().enumerate() {
            let offset = index * 4;
            // Stored pixels are BGRA; the preview pipeline works in RGBA.
            target[offset] = (pixel >> 16) as u8;
            target[offset + 1] = (pixel >> 8) as u8;
            target[offset + 2] = *pixel as u8;
            target[offset + 3] = if channels == 4 {
                (pixel >> 24) as u8
            } else {
                0xFF
            };
        }
        previous_row.copy_from_slice(&row);
    }
    Some(())
}

/// Unpack one row's channel deltas: a bit-level RLE with an opcode per symbol.
///
/// Returns the number of input bytes the row consumed, which is what the caller
/// advances by. `remaining_bits / 8` is handed back because rows are byte
/// aligned: the trailing partial byte belongs to the next row's stream.
fn unpack_delta_rle16(
    compressed: &[u8],
    out: &mut [i16],
    pixel_count: usize,
    output_channels: usize,
    input_channels: usize,
) -> Option<usize> {
    let total = compressed.len();
    let mut bytes = compressed;
    let mut remaining_bits: i64 = 0;
    let mut mask: u64 = 0;

    let mut write = |index: usize, channel: usize, value: i16| -> Option<()> {
        if index >= pixel_count {
            return None;
        }
        *out.get_mut(index * output_channels + channel)? = value;
        Some(())
    };

    for channel in 0..input_channels {
        let mut written = 0_usize;
        loop {
            // Keep at least 32 bits buffered, consuming the stream in the same
            // 4/2/1-byte steps as the reference so the byte accounting matches.
            while remaining_bits < 32 && !bytes.is_empty() {
                let (word, taken_bits): (u64, i64) = if bytes.len() >= 4 {
                    (
                        u64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
                        32,
                    )
                } else if bytes.len() >= 2 {
                    (u64::from(u16::from_le_bytes([bytes[0], bytes[1]])), 16)
                } else {
                    (u64::from(bytes[0]), 8)
                };
                bytes = &bytes[(taken_bits / 8) as usize..];
                mask |= word << remaining_bits;
                remaining_bits += taken_bits;
            }
            if mask == 0 {
                return None;
            }

            // Control code: a run of zero bits, a terminator, then one bit that
            // makes the opcode odd or even.
            let zeros = mask.trailing_zeros();
            if zeros >= 63 {
                return None;
            }
            let rest = mask >> (zeros + 1);
            let opcode = (2 * u64::from(zeros)) | (rest & 1);
            if opcode > 0xF {
                return None;
            }
            remaining_bits -= i64::from(2 + zeros);
            mask = rest >> 1;
            if remaining_bits < 0 {
                return None;
            }

            match opcode {
                // 0: a single zero pixel.
                0 => {
                    write(written, channel, 0)?;
                    written += 1;
                }
                // 1..=14: the opcode is the number of magnitude bits; the last
                // bit is the sign, and the value is stored biased.
                1..=14 => {
                    let bits = opcode as u32;
                    let negative = (mask >> bits) & 1 != 0;
                    let magnitude = mask & ((1_u64 << bits) - 1);
                    remaining_bits -= i64::from(bits + 1);
                    mask >>= bits + 1;
                    if remaining_bits < 0 {
                        return None;
                    }
                    let combined = (1_u64 << bits) | magnitude;
                    let biased = combined.wrapping_sub(1);
                    let active = if negative { u64::MAX } else { 0 };
                    let value = ((active & 1).wrapping_add(active ^ biased)) as i16;
                    write(written, channel, value)?;
                    written += 1;
                }
                // 15: fill 8..135 pixels with zero.
                _ => {
                    let count = (mask & 0x7F) as usize + 8;
                    remaining_bits -= 7;
                    mask >>= 7;
                    if remaining_bits < 0 || written + count > pixel_count {
                        return None;
                    }
                    for index in written..written + count {
                        write(index, channel, 0)?;
                    }
                    written += count;
                }
            }

            if written >= pixel_count {
                break;
            }
        }
    }

    // Lanes the stream does not carry stay zero, matching the reference.
    for channel in input_channels..output_channels {
        for index in 0..pixel_count {
            *out.get_mut(index * output_channels + channel)? = 0;
        }
    }

    let read = total - bytes.len();
    let still_buffered = (remaining_bits.max(0) / 8) as usize;
    Some(read.saturating_sub(still_buffered))
}

/// Reconstruct one row from its deltas.
///
/// The predictor is PNG's `Up` filter at 16 bpc, saturated back into 0..255:
/// each pixel is its left neighbour's running sum plus the pixel above it,
/// minus the pixel above-left, plus the delta.
fn delta_unpack_row16(dest: &mut [u32], previous_row: &[u32], deltas: &[i16], pixel_count: usize) {
    let mut sum = [0_u16; 4];
    let mut previous_pixel = [0_u16; 4];
    for index in 0..pixel_count {
        let above = previous_row[index];
        let mut current = [0_u16; 4];
        let mut pixel = 0_u32;
        for channel in 0..4 {
            current[channel] = ((above >> (8 * channel)) & 0xFF) as u16;
            let delta = deltas[index * 4 + channel] as u16;
            let running = sum[channel]
                .wrapping_add(current[channel])
                .saturating_sub(previous_pixel[channel]);
            // Only the byte written out is clamped to 0..255; the running sum
            // keeps its full 16-bit value so the overshoot carries into the next
            // pixel, exactly as the reference does.
            let value = running.min(0xFF).wrapping_add(delta);
            sum[channel] = value;
            pixel |= u32::from(value.min(0xFF)) << (8 * channel);
        }
        dest[index] = pixel;
        previous_pixel = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bits least-significant-bit first, which is how the RLE stream is
    /// packed and how the decoder consumes it.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl BitWriter {
        fn push(&mut self, value: u64, count: u32) {
            for index in 0..count {
                if self.bit.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if (value >> index) & 1 == 1 {
                    let byte = self.bit / 8;
                    self.bytes[byte] |= 1 << (self.bit % 8);
                }
                self.bit += 1;
            }
        }
    }

    fn header_bytes(width: u32, height: u32, table_count: u32, background_flags: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; HEADER_LEN as usize];
        bytes[..16].copy_from_slice(IDENT);
        bytes[17] = background_flags;
        bytes[20..24].copy_from_slice(&width.to_le_bytes());
        bytes[24..28].copy_from_slice(&height.to_le_bytes());
        bytes[32..36].copy_from_slice(&table_count.to_le_bytes());
        bytes
    }

    /// One `intg` tile whose every delta is zero, i.e. an all-black canvas.
    ///
    /// Each zero pixel is opcode 0, whose control code is the bits `1, 0`.
    fn blank_tile_bytes(cols: u32, rows: u32, channels: usize) -> Vec<u8> {
        let mut bits = BitWriter::default();
        for _ in 0..channels {
            for _ in 0..cols {
                bits.push(0b1, 1);
                bits.push(0b0, 1);
            }
        }
        let row = bits.bytes;
        let mut tile = Vec::new();
        tile.extend_from_slice(&0_u16.to_le_bytes()); // tile checksum
        for _ in 0..rows {
            tile.extend_from_slice(&row);
        }
        tile
    }

    fn intg_canvas(width: u32, height: u32, channels: usize) -> Vec<u8> {
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        let table_count = tiles_x * tiles_y;
        let mut blob = Vec::new();
        blob.extend_from_slice(DPCM_TAG);
        let mut tiles = Vec::new();
        for _ in 0..table_count {
            let rows = height.min(TILE_SIZE);
            let cols = width.min(TILE_SIZE);
            let tile = blank_tile_bytes(cols, rows, channels);
            blob.extend_from_slice(&(tile.len() as u32).to_le_bytes());
            tiles.push(tile);
        }
        for tile in tiles {
            blob.extend_from_slice(&tile);
        }
        // One trailing checksum per tile row; its high byte is the column count.
        for _ in 0..tiles_y {
            blob.extend_from_slice(&((tiles_x as u16) << 8).to_le_bytes());
        }

        // `background_flags & 7 == 0` means the canvas carries an alpha channel.
        let background_flags = if channels == 4 { 0 } else { 1 };
        let mut file = header_bytes(width, height, 1, background_flags);
        file.extend_from_slice(INTG_TAG);
        file.extend_from_slice(&0_u32.to_le_bytes()); // layer
        let blob_offset = HEADER_LEN + ENTRY_LEN;
        file.extend_from_slice(&blob_offset.to_le_bytes());
        file.extend_from_slice(&blob);
        file
    }

    /// Canonical JPEG code for `symbol`, derived from a table in the same form
    /// SAI stores it: an id byte, 16 code-length counters, then the values.
    fn huffman_code(table: &[u8], symbol: u8) -> (u32, u32) {
        let counts = &table[1..17];
        let values = &table[17..];
        let mut code = 0_u32;
        let mut index = 0;
        for (length, count) in counts.iter().enumerate() {
            for _ in 0..*count {
                if values[index] == symbol {
                    return (code, length as u32 + 1);
                }
                index += 1;
                code += 1;
            }
            code <<= 1;
        }
        panic!("symbol {symbol:#04x} is not in the table");
    }

    /// A `thum` blob whose entropy stream is one flat 8×8 MCU per channel:
    /// DC category 0 followed by EOB, which decodes to mid grey.
    ///
    /// The codes are taken from the default tables SAI writes rather than
    /// hardcoded, so the test exercises the segment layout (DQT / DHT / SOF0 /
    /// DRI / SOS) and the table selection per component, not the entropy coder.
    fn jssf_blob(width: u16, height: u16, channels: u16) -> Vec<u8> {
        let mut bits = Vec::new();
        for channel in 0..channels {
            let (dc_table, ac_table) = if channel == 0 {
                (&HUFFMAN_DC_LUMA[..], &HUFFMAN_AC_LUMA[..])
            } else {
                (&HUFFMAN_DC_CHROMA[..], &HUFFMAN_AC_CHROMA[..])
            };
            // Huffman codes are written most-significant bit first.
            for (code, length) in [huffman_code(dc_table, 0), huffman_code(ac_table, 0)] {
                for bit in (0..length).rev() {
                    bits.push(((code >> bit) & 1) as u8);
                }
            }
        }
        // Trailing bits of a segment are padded with ones, as the standard says.
        let mut entropy = vec![0xFF_u8; bits.len().div_ceil(8)];
        for (index, bit) in bits.iter().enumerate() {
            let mask = 0x80 >> (index % 8);
            if *bit == 1 {
                entropy[index / 8] |= mask;
            } else {
                entropy[index / 8] &= !mask;
            }
        }

        let mut blob = Vec::new();
        blob.extend_from_slice(&u32::from(width).to_le_bytes()); // outer width
        blob.extend_from_slice(&0_u32.to_le_bytes()); // outer height, unused
        blob.extend_from_slice(JSSF_TAG);
        blob.extend_from_slice(&width.to_le_bytes());
        blob.extend_from_slice(&height.to_le_bytes());
        blob.extend_from_slice(&channels.to_le_bytes());
        blob.extend_from_slice(&[16_u8; 64]); // luma quantisation table
        if channels > 1 {
            blob.extend_from_slice(&[16_u8; 64]);
        }
        // One length-prefixed entropy segment per MCU row; this blob is one row.
        blob.extend_from_slice(&(entropy.len() as u16).to_le_bytes());
        blob.extend_from_slice(&entropy);
        blob
    }

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.sai2");
        std::fs::write(&path, bytes).unwrap();
        (directory, path)
    }

    #[test]
    fn foreign_identifier_is_rejected() {
        let mut bytes = header_bytes(16, 16, 1, 0);
        bytes[..16].copy_from_slice(b"SAI-CANVAS-TYPE1");
        assert!(parse_header(&bytes).is_none());
    }

    #[test]
    fn impossible_table_counts_are_rejected() {
        assert!(parse_header(&header_bytes(16, 16, 0, 0)).is_none());
        assert!(parse_header(&header_bytes(16, 16, MAX_TABLE_COUNT + 1, 0)).is_none());
    }

    #[test]
    fn blank_intg_canvas_decodes_to_opaque_black() {
        let (_directory, path) = write_temp(&intg_canvas(2, 2, 3));
        let image = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(image.to_rgba8().into_raw(), [0, 0, 0, 255].repeat(4));
    }

    #[test]
    fn thum_blob_without_the_jssf_tag_yields_no_preview() {
        // A `thum` entry whose payload is not a JSSF blob must not fall through
        // to a fabricated JPEG; extraction simply reports no preview.
        let mut blob = Vec::new();
        blob.extend_from_slice(&[0_u8; 18]);
        blob.extend_from_slice(&[0_u8; 64]);
        let mut file = header_bytes(2, 2, 1, 1);
        file.extend_from_slice(THUM_TAG);
        file.extend_from_slice(&0_u32.to_le_bytes());
        file.extend_from_slice(&(HEADER_LEN + ENTRY_LEN).to_le_bytes());
        file.extend_from_slice(&blob);
        let (_directory, path) = write_temp(&file);
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn jssf_stream_is_repacked_into_a_decodable_jpeg() {
        // Grey (1 channel) and colour (3 channels) exercise both DQT / DHT / SOS
        // shapes; a flat MCU must come back as one flat 8x8 tile.
        for channels in [1_u16, 3] {
            let jpeg = jssf_to_jpeg(&jssf_blob(8, 8, channels)).unwrap();
            let image = decode_bytes(&jpeg).unwrap();
            assert_eq!(
                (image.width(), image.height()),
                (8, 8),
                "channels {channels}"
            );
            for pixel in image.to_rgba8().pixels() {
                let [red, green, blue, alpha] = pixel.0;
                assert_eq!((red, green, blue), (128, 128, 128), "channels {channels}");
                assert_eq!(alpha, 255);
            }
        }
    }

    #[test]
    fn entry_offsets_outside_the_file_are_rejected() {
        let mut file = header_bytes(2, 2, 1, 0);
        file.extend_from_slice(INTG_TAG);
        file.extend_from_slice(&0_u32.to_le_bytes());
        file.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
        let (_directory, path) = write_temp(&file);
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn cancellation_skips_extraction() {
        let (_directory, path) = write_temp(&intg_canvas(2, 2, 3));
        let cancel = CancelToken::standalone();
        cancel.cancel();
        assert!(extract(&path, &cancel).is_none());
    }

    #[test]
    fn malformed_canvases_are_rejected_without_panicking() {
        let valid = intg_canvas(4, 3, 3);
        let mut rng = crate::fuzz_support::Rng::new(0x0BAD_5A12);
        for _ in 0..96 {
            let mut bytes = valid.clone();
            let mutations = 1 + rng.below(10);
            for _ in 0..mutations {
                let index = rng.below(bytes.len());
                bytes[index] = (rng.next_u64() & 0xFF) as u8;
            }
            let (_directory, path) = write_temp(&bytes);
            // Either outcome is acceptable; surviving without panic is the point.
            let _ = extract(&path, &CancelToken::standalone());
        }
    }
}
