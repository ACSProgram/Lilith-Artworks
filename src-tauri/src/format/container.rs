//! CSFC (`.clip`) structural layout and cheap `.psd` detection.
//!
//! Layout confirmed by the 0.3 research harness on 890 real `.clip` files
//! (18,575 `Exta` blocks, 2,539,946 tile records, zero walk failures):
//!
//! ```text
//! file  = "CSFCHUNK" + u64be total + u64be first_block + blocks...
//! block = u32be ? + tag[4] + u64be payload_len + payload
//! Exta payload = u64be 40 + "extrnlid" + GUID(32 ascii) + u32be + u32be
//!                + records + tail
//! record = u32be size + u32be namelen(19) + "BlockDataBeginChunk" (UTF-16BE)
//!          + body + u32be namelen(17) + "BlockDataEndChunk"
//! ```
//!
//! The `size` field of a tile record equals the distance to the next record
//! start, so records tile the payload; whatever follows the last record (block
//! status / checksum) stays one trailing unit.
//!
//! `.psd` / `.psb` are detected only. Their structural adapter is deliberately
//! not implemented until the layout is reverse-engineered; callers fall back to
//! CDC for them.

use std::io::{Read, Seek, SeekFrom};

const CSFC_MAGIC: &[u8; 8] = b"CSFCHUNK";
const PSD_MAGIC: &[u8; 4] = b"8BPS";
const CSFC_FILE_HEADER: usize = 24;
const CSFC_BLOCK_HEADER: usize = 16;
const EXTA_TAG: &[u8; 8] = b"extrnlid";
const TILE_BEGIN: &str = "BlockDataBeginChunk";
/// `Exta` payload header: u64 prefix + tag(8) + GUID(32) + u32 + u32.
const EXTA_HEADER_LEN: u64 = 56;
const EXTA_PROBE_LEN: usize = EXTA_HEADER_LEN as usize;
const TILE_NODE_HEADER: u64 = 8;
const TILE_NAME_LEN: u32 = TILE_BEGIN.len() as u32;
const TILE_NAME_BYTES: u64 = TILE_NAME_LEN as u64 * 2;
const MAX_TILE_COUNT: u64 = 10_000_000;
const CANCEL_CHECK_INTERVAL: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Clip,
    Psd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainerError {
    /// The bytes are not a recognised container, or layout discovery failed.
    Invalid(String),
    Cancelled,
}

impl std::fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Cancelled => formatter.write_str("container scan cancelled"),
        }
    }
}

/// Cheap probe over the head of a file. Never reads the whole file.
pub(crate) fn detect(head: &[u8]) -> Option<ContainerKind> {
    if head.starts_with(CSFC_MAGIC) {
        Some(ContainerKind::Clip)
    } else if head.starts_with(PSD_MAGIC) {
        Some(ContainerKind::Psd)
    } else {
        None
    }
}

/// Byte range of one top-level block payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockPayloadRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl BlockPayloadRange {
    pub(crate) fn len(self) -> u64 {
        self.end - self.start
    }
}

/// Locate the payload of the first top-level block with `tag`.
///
/// The walk reads only block headers and seeks over payloads, so locating the
/// embedded SQLite database of a multi-gigabyte `.clip` stays cheap. Unlike
/// [`clip_structural_units`] this does not require the container to be fully
/// consistent (no `Foot` or tile validation): callers only use it to read one
/// payload, and any inconsistency before that payload is still rejected.
pub(crate) fn clip_block_payload_range<R: Read + Seek>(
    source: &mut R,
    tag: &[u8; 4],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<BlockPayloadRange>, ContainerError> {
    let (file_length, first_block) = read_file_header(source)?;
    let mut offset = first_block;
    while offset + CSFC_BLOCK_HEADER as u64 <= file_length {
        if cancelled() {
            return Err(ContainerError::Cancelled);
        }
        let (block_tag, payload_start, payload_end) =
            read_block_header(source, offset, file_length)?;
        if &block_tag == tag {
            return Ok(Some(BlockPayloadRange {
                start: payload_start,
                end: payload_end,
            }));
        }
        if &block_tag == b"Foot" {
            break;
        }
        offset = payload_end;
    }
    Ok(None)
}

/// Read and validate the CSFC file header; returns `(file_length, first_block)`.
///
/// Both the structural walk and the single-payload lookup go through this so
/// the magic, declared length and first-block offset are checked identically.
fn read_file_header<R: Read + Seek>(source: &mut R) -> Result<(u64, u64), ContainerError> {
    let file_length = source
        .seek(SeekFrom::End(0))
        .map_err(|error| ContainerError::Invalid(format!("无法读取容器长度：{error}")))?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(|error| ContainerError::Invalid(format!("无法回到容器起点：{error}")))?;
    if file_length < CSFC_FILE_HEADER as u64 {
        return Err(ContainerError::Invalid("容器头部不完整".into()));
    }
    let mut file_header = [0_u8; CSFC_FILE_HEADER];
    read_exact(source, &mut file_header, "容器头部")?;
    if &file_header[0..8] != CSFC_MAGIC {
        return Err(ContainerError::Invalid("不是 CSFC 容器".into()));
    }
    let declared_total = be_u64(&file_header, 8);
    if declared_total != file_length {
        return Err(ContainerError::Invalid(format!(
            "容器声明长度 {declared_total} 与文件长度 {file_length} 不一致"
        )));
    }
    let first_block = be_u64(&file_header, 16);
    if first_block < CSFC_FILE_HEADER as u64 || first_block > file_length {
        return Err(ContainerError::Invalid(format!(
            "容器首个块偏移 {first_block} 越界"
        )));
    }
    Ok((file_length, first_block))
}

/// Read one block header; returns `(tag, payload_start, payload_end)`.
fn read_block_header<R: Read + Seek>(
    source: &mut R,
    offset: u64,
    file_length: u64,
) -> Result<([u8; 4], u64, u64), ContainerError> {
    let mut header = [0_u8; CSFC_BLOCK_HEADER];
    source
        .seek(SeekFrom::Start(offset))
        .map_err(|error| ContainerError::Invalid(format!("无法定位容器块：{error}")))?;
    read_exact(source, &mut header, "容器块头")?;
    let tag: [u8; 4] = header[4..8].try_into().expect("4-byte tag");
    let payload_start = offset + CSFC_BLOCK_HEADER as u64;
    let payload_end = payload_start
        .checked_add(be_u64(&header, 8))
        .filter(|end| *end <= file_length)
        .ok_or_else(|| ContainerError::Invalid(format!("块偏移 {offset} 的载荷越过文件末尾")))?;
    Ok((tag, payload_start, payload_end))
}

/// One structural unit in file order. Units are contiguous and tile
/// `[0, file_length)` exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StructuralUnit {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl StructuralUnit {
    pub(crate) fn len(self) -> u64 {
        self.end - self.start
    }
}

/// Discover the structural units of a CSFC (`.clip`) container.
///
/// This pass reads only block and tile headers; it seeks over payload bytes, so
/// multi-gigabyte files are not loaded into memory. Any inconsistency returns
/// `Invalid` and the caller falls back to CDC. `cancelled` is polled while
/// walking records so a long scan can stop promptly.
pub(crate) fn clip_structural_units<R: Read + Seek>(
    source: &mut R,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<StructuralUnit>, ContainerError> {
    let (file_length, first_block) = read_file_header(source)?;

    // Bytes in front of the first block (the file header is part of them) are
    // already covered, so they form the first unit.
    let mut units = vec![StructuralUnit {
        start: 0,
        end: first_block,
    }];

    let mut offset = first_block;
    let mut saw_foot = false;
    let mut tick = 0_u64;
    while offset + CSFC_BLOCK_HEADER as u64 <= file_length {
        if tick % CANCEL_CHECK_INTERVAL == 0 && cancelled() {
            return Err(ContainerError::Cancelled);
        }
        tick = tick.saturating_add(1);

        let (tag, payload_start, payload_end) = read_block_header(source, offset, file_length)?;

        if &tag == b"Exta" {
            push_exta_units(
                source,
                offset,
                payload_start,
                payload_end,
                &mut units,
                cancelled,
            )?;
        } else {
            units.push(StructuralUnit {
                start: offset,
                end: payload_end,
            });
        }

        if &tag == b"Foot" {
            saw_foot = true;
            if payload_end != file_length {
                return Err(ContainerError::Invalid("Foot 块之后仍有未识别数据".into()));
            }
            break;
        }
        offset = payload_end;
    }

    if !saw_foot {
        return Err(ContainerError::Invalid(
            "容器缺少 Foot 块（可能已截断）".into(),
        ));
    }
    validate_coverage(&units, file_length)?;
    Ok(units)
}

fn push_exta_units<R: Read + Seek>(
    source: &mut R,
    block_start: u64,
    payload_start: u64,
    payload_end: u64,
    units: &mut Vec<StructuralUnit>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), ContainerError> {
    if payload_end.saturating_sub(payload_start) < EXTA_HEADER_LEN {
        units.push(StructuralUnit {
            start: block_start,
            end: payload_end,
        });
        return Ok(());
    }
    source
        .seek(SeekFrom::Start(payload_start))
        .map_err(|error| ContainerError::Invalid(format!("无法定位 Exta 载荷：{error}")))?;
    let mut header = [0_u8; EXTA_PROBE_LEN];
    read_exact(source, &mut header, "Exta 载荷头")?;
    if &header[8..16] != EXTA_TAG {
        units.push(StructuralUnit {
            start: block_start,
            end: payload_end,
        });
        return Ok(());
    }

    let records_start = payload_start + EXTA_HEADER_LEN;
    let mut position = records_start;
    let mut tiles = Vec::new();
    let want = utf16be(TILE_BEGIN);
    let mut tick = 0_u64;
    while position + TILE_NODE_HEADER <= payload_end {
        if tick % CANCEL_CHECK_INTERVAL == 0 && cancelled() {
            return Err(ContainerError::Cancelled);
        }
        tick = tick.saturating_add(1);

        source
            .seek(SeekFrom::Start(position))
            .map_err(|error| ContainerError::Invalid(format!("无法定位 tile 记录：{error}")))?;
        let mut node = [0_u8; TILE_NODE_HEADER as usize];
        read_exact(source, &mut node, "tile 记录头")?;
        let size = u64::from(be_u32(&node, 0));
        let name_len = be_u32(&node, 4);
        if name_len != TILE_NAME_LEN {
            break;
        }
        let name_start = position + TILE_NODE_HEADER;
        // The record header was already bounds-checked; the UTF-16 name has to
        // fit inside the payload before it is read.
        if name_start
            .checked_add(TILE_NAME_BYTES)
            .filter(|end| *end <= payload_end)
            .is_none()
        {
            return Err(ContainerError::Invalid("tile 名称越过载荷末尾".into()));
        }
        let mut name = [0_u8; TILE_NAME_BYTES as usize];
        source
            .seek(SeekFrom::Start(name_start))
            .map_err(|error| ContainerError::Invalid(format!("无法读取 tile 名称：{error}")))?;
        read_exact(source, &mut name, "tile 名称")?;
        if name != want.as_slice() {
            return Err(ContainerError::Invalid("tile 节点名称不匹配".into()));
        }
        if size < TILE_NODE_HEADER + TILE_NAME_BYTES || position + size > payload_end {
            return Err(ContainerError::Invalid(format!(
                "位置 {position} 的 tile 记录长度 {size} 无效"
            )));
        }
        tiles.push(StructuralUnit {
            start: position,
            end: position + size,
        });
        if tiles.len() as u64 > MAX_TILE_COUNT {
            return Err(ContainerError::Invalid("tile 记录数量超过上限".into()));
        }
        position += size;
    }

    if tiles.is_empty() {
        units.push(StructuralUnit {
            start: block_start,
            end: payload_end,
        });
        return Ok(());
    }
    units.push(StructuralUnit {
        start: block_start,
        end: records_start,
    });
    units.append(&mut tiles);
    if position < payload_end {
        units.push(StructuralUnit {
            start: position,
            end: payload_end,
        });
    }
    Ok(())
}

fn validate_coverage(units: &[StructuralUnit], file_length: u64) -> Result<(), ContainerError> {
    if units.is_empty() {
        return Err(ContainerError::Invalid("容器没有可用结构单元".into()));
    }
    let mut expected = 0_u64;
    for unit in units {
        if unit.start != expected || unit.end <= unit.start {
            return Err(ContainerError::Invalid(
                "结构单元没有按顺序完整覆盖文件".into(),
            ));
        }
        expected = unit.end;
    }
    if expected != file_length {
        return Err(ContainerError::Invalid(format!(
            "结构单元覆盖到 {expected}，文件长度为 {file_length}"
        )));
    }
    Ok(())
}

fn utf16be(name: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(name.len() * 2);
    for byte in name.bytes() {
        output.push(0);
        output.push(byte);
    }
    output
}

fn be_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("4-byte field"))
}

fn be_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("8-byte field"))
}

fn read_exact<R: Read>(
    source: &mut R,
    buffer: &mut [u8],
    label: &str,
) -> Result<(), ContainerError> {
    source
        .read_exact(buffer)
        .map_err(|error| ContainerError::Invalid(format!("无法读取{label}：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn block(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&[0, 0, 0, 0]);
        output.extend_from_slice(tag);
        output.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        output.extend_from_slice(payload);
        output
    }

    fn container(blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for block in blocks {
            body.extend_from_slice(block);
        }
        let mut output = Vec::new();
        output.extend_from_slice(CSFC_MAGIC);
        output.extend_from_slice(&((24 + body.len()) as u64).to_be_bytes());
        output.extend_from_slice(&24_u64.to_be_bytes());
        output.extend_from_slice(&body);
        output
    }

    fn tile(body: &[u8]) -> Vec<u8> {
        let begin = utf16be(TILE_BEGIN);
        let end = utf16be("BlockDataEndChunk");
        let size = 4 + 4 + begin.len() + body.len() + 4 + end.len();
        let mut output = Vec::new();
        output.extend_from_slice(&(size as u32).to_be_bytes());
        output.extend_from_slice(&TILE_NAME_LEN.to_be_bytes());
        output.extend_from_slice(&begin);
        output.extend_from_slice(body);
        output.extend_from_slice(&(17_u32).to_be_bytes());
        output.extend_from_slice(&end);
        output
    }

    fn exta(tiles: &[Vec<u8>], trailing: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&40_u64.to_be_bytes());
        payload.extend_from_slice(EXTA_TAG);
        payload.extend_from_slice(b"0123456789ABCDEF0123456789ABCDEF");
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        for tile in tiles {
            payload.extend_from_slice(tile);
        }
        payload.extend_from_slice(trailing);
        block(b"Exta", &payload)
    }

    fn plan(bytes: Vec<u8>) -> Result<Vec<StructuralUnit>, ContainerError> {
        clip_structural_units(&mut Cursor::new(bytes), &|| false)
    }

    #[test]
    fn detects_known_containers() {
        assert_eq!(detect(b"CSFCHUNK\0\0\0\0"), Some(ContainerKind::Clip));
        assert_eq!(detect(b"8BPS\0\x01"), Some(ContainerKind::Psd));
        assert_eq!(detect(b"RIFF....WEBP"), None);
    }

    #[test]
    fn tile_records_become_units_and_cover_the_file() {
        let file = container(&[
            block(b"Head", &[0_u8; 8]),
            exta(&[tile(&[1, 2, 3, 4]), tile(&[5, 6])], &[0xAA; 12]),
            block(b"SQLi", &[7_u8; 16]),
            block(b"Foot", &[]),
        ]);
        let units = plan(file.clone()).unwrap();
        assert_eq!(units.first().unwrap().start, 0);
        assert_eq!(units.last().unwrap().end, file.len() as u64);
        let mut expected = 0;
        for unit in &units {
            assert_eq!(unit.start, expected);
            assert!(unit.end > unit.start);
            expected = unit.end;
        }
    }

    #[test]
    fn screencast_exta_without_tiles_stays_one_unit() {
        let file = container(&[
            block(b"Head", &[0_u8; 8]),
            exta(&[], &[0x78, 0x9c, 1, 2, 3, 4, 5, 6]),
            block(b"Foot", &[]),
        ]);
        let units = plan(file).unwrap();
        // head gap + Head block + Exta block + Foot block
        assert_eq!(units.len(), 4);
    }

    #[test]
    fn truncated_and_garbage_containers_are_rejected() {
        let file = container(&[block(b"Head", &[0_u8; 8]), block(b"Foot", &[])]);
        assert!(matches!(
            plan(file[..file.len() - 4].to_vec()),
            Err(ContainerError::Invalid(_))
        ));
        assert!(matches!(
            plan(b"not a clip at all at all".to_vec()),
            Err(ContainerError::Invalid(_))
        ));
        let mut overrun = file.clone();
        let header = 24 + CSFC_BLOCK_HEADER;
        let length = u64::from_be_bytes(overrun[header + 8..header + 16].try_into().unwrap());
        overrun[header + 8..header + 16].copy_from_slice(&(length + 1_000).to_be_bytes());
        assert!(matches!(plan(overrun), Err(ContainerError::Invalid(_))));
    }

    #[test]
    fn cancellation_stops_the_scan() {
        let file = container(&[
            block(b"Head", &[0_u8; 8]),
            exta(&[tile(&[1]), tile(&[2])], &[]),
            block(b"Foot", &[]),
        ]);
        let error = clip_structural_units(&mut Cursor::new(file), &|| true).unwrap_err();
        assert_eq!(error, ContainerError::Cancelled);
    }
}
