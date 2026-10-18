use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    mem::size_of,
    path::Path,
};

use sha2::{Digest, Sha256};
#[cfg(test)]
use tempfile::tempfile;
use tempfile::tempfile_in;

use crate::{
    cancel::CancelToken,
    format::container::{self, ContainerError, StructuralUnit},
};

const SNAPSHOT_MAGIC: [u8; 8] = *b"LBCHUNK\0";
const DELTA_MAGIC: [u8; 8] = *b"LBDELTA\0";
const FORMAT_VERSION_V1: u16 = 1;
const FORMAT_VERSION_V2: u16 = 2;
/// v1 头里的 chunker 字段固定为滚动哈希实现编号。
const LEGACY_CHUNKER_SHIFT_ADD: u16 = 1;
/// v2 头里的 chunker 编号：0=CDC，1=CLIP 结构，2=PSD 结构。
const V2_CHUNKER_CDC: u16 = 0;
const V2_CHUNKER_CLIP: u16 = 1;
const V2_CHUNKER_PSD: u16 = 2;
const V2_ADAPTER_NONE: u16 = 0;
const V2_ADAPTER_CLIP: u16 = 1;
const V2_ADAPTER_PSD: u16 = 2;
const HASH_SHA256: u16 = 1;
const V2_FLAGS_CHUNK_RECORDS: u16 = 1;
/// v2 块记录 flags bit0：该块与容器结构单元边界对齐。
const V2_RECORD_FLAG_STRUCTURAL: u16 = 1;
const CODEC_NONE: u8 = 0;
const RECIPE_NONE: u8 = 0xFF;
const MAX_ALLOWED_CHUNK_SIZE: usize = 16 * 1024 * 1024;
const STREAM_BUFFER_SIZE: usize = 128 * 1024;
const DIGEST_SIZE: usize = 32;
#[cfg(test)]
const V1_SNAPSHOT_HEADER_SIZE: usize = 76;
#[cfg(test)]
const V2_SNAPSHOT_HEADER_SIZE: usize = 80;
const V1_DELTA_HEADER_SIZE: usize = 116;
const V2_DELTA_HEADER_SIZE: usize = 120;
const V1_RECORD_HEADER_SIZE: usize = DIGEST_SIZE + size_of::<u32>();
const V2_RECORD_HEADER_SIZE: usize =
    DIGEST_SIZE + size_of::<u32>() + size_of::<u32>() + 1 + 1 + size_of::<u16>();
const DELTA_OP_SIZE: usize = size_of::<u8>() + DIGEST_SIZE + size_of::<u32>();
/// 取消检查的循环间隔。转码步的地板是每个块（默认最大 320 KiB 的 tile 或
/// 64 KiB 的 CDC 块），这里只用于遍历操作表等纯 CPU 循环。
const CANCEL_CHECK_INTERVAL: usize = 1024;

// Snapshot keeps chunk payloads immutable and addressable by offset. Delta is
// intentionally reverse: it transforms the current snapshot into its parent.
// v1 stays readable forever; v2 adds per-record lengths/codec and an adapter id
// so structural chunking and future codecs can coexist in one repository.

pub(crate) type ChunkFileResult<T> = Result<T, ChunkFileError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorDimension {
    Container,
    Format,
    Chunker,
    Hash,
    Codec,
    Recipe,
    Adapter,
    Chunk,
    File,
}

/// 0.3 §3.D / O5 的两轴错误分类：class 决定“要做什么”，dimension 决定
/// “哪里出问题”。未知能力归 `Incompatible`（能力缺口，不是数据损坏）；
/// 帧级损坏归 `InvalidFormat/Chunk`；内容损坏归 `Integrity/Chunk|Codec`。
#[derive(Debug)]
#[allow(dead_code)] // Dimensions are part of the O5 contract; tests assert them.
pub(crate) enum ChunkFileError {
    Io(io::Error),
    InvalidConfig(String),
    InvalidFormat(ErrorDimension, String),
    Integrity(ErrorDimension, String),
    Incompatible(ErrorDimension, String),
    OutputConflict(String),
    Cancelled,
}

impl fmt::Display for ChunkFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O 错误：{error}"),
            Self::InvalidConfig(message) => write!(formatter, "分块配置无效：{message}"),
            Self::InvalidFormat(_, message) => write!(formatter, "分块文件格式无效：{message}"),
            Self::Integrity(_, message) => {
                write!(formatter, "分块文件完整性校验失败：{message}")
            }
            Self::Incompatible(_, message) => write!(formatter, "分块文件不兼容：{message}"),
            Self::OutputConflict(message) => formatter.write_str(message),
            Self::Cancelled => formatter.write_str("操作已取消"),
        }
    }
}

impl Error for ChunkFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ChunkFileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn invalid_format(dimension: ErrorDimension, message: impl Into<String>) -> ChunkFileError {
    ChunkFileError::InvalidFormat(dimension, message.into())
}

fn integrity(dimension: ErrorDimension, message: impl Into<String>) -> ChunkFileError {
    ChunkFileError::Integrity(dimension, message.into())
}

fn incompatible(dimension: ErrorDimension, message: impl Into<String>) -> ChunkFileError {
    ChunkFileError::Incompatible(dimension, message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatVersion {
    V1,
    V2,
}

impl FormatVersion {
    fn number(self) -> u16 {
        match self {
            Self::V1 => FORMAT_VERSION_V1,
            Self::V2 => FORMAT_VERSION_V2,
        }
    }

    fn from_number(value: u16) -> ChunkFileResult<Self> {
        match value {
            FORMAT_VERSION_V1 => Ok(Self::V1),
            FORMAT_VERSION_V2 => Ok(Self::V2),
            other => Err(incompatible(
                ErrorDimension::Format,
                format!("不支持格式版本 {other}"),
            )),
        }
    }

    fn delta_header_size(self) -> usize {
        match self {
            Self::V1 => V1_DELTA_HEADER_SIZE,
            Self::V2 => V2_DELTA_HEADER_SIZE,
        }
    }

    fn record_header_size(self) -> usize {
        match self {
            Self::V1 => V1_RECORD_HEADER_SIZE,
            Self::V2 => V2_RECORD_HEADER_SIZE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkingStrategy {
    Cdc,
    ContainerAware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkWriteOptions {
    pub(crate) format: FormatVersion,
    pub(crate) chunking: ChunkingStrategy,
}

impl ChunkWriteOptions {
    pub(crate) const V1: Self = Self {
        format: FormatVersion::V1,
        chunking: ChunkingStrategy::Cdc,
    };
    pub(crate) const V2: Self = Self {
        format: FormatVersion::V2,
        chunking: ChunkingStrategy::ContainerAware,
    };
}

/// Cancellation and byte-progress for one chunk-file operation.
///
/// `cancel` is the same `CancelToken` the command runtime uses; `progress`
/// receives the number of logical bytes processed in this operation so callers
/// can report byte-based progress instead of only node counts.
pub(crate) struct ChunkTask<'a> {
    cancel: Option<&'a CancelToken>,
    progress: Option<&'a dyn Fn(u64)>,
}

impl<'a> ChunkTask<'a> {
    #[cfg(test)]
    pub(crate) fn untracked() -> Self {
        Self {
            cancel: None,
            progress: None,
        }
    }

    pub(crate) fn new(cancel: &'a CancelToken, progress: Option<&'a dyn Fn(u64)>) -> Self {
        Self {
            cancel: Some(cancel),
            progress,
        }
    }

    pub(crate) fn cancel(cancel: &'a CancelToken) -> Self {
        Self::new(cancel, None)
    }

    fn check(&self) -> ChunkFileResult<()> {
        if self.is_cancelled() {
            Err(ChunkFileError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|token| token.is_cancelled())
    }

    fn report(&self, bytes: u64) {
        if let Some(progress) = self.progress {
            progress(bytes);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkingConfig {
    pub(crate) min_size: usize,
    pub(crate) avg_size: usize,
    pub(crate) max_size: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            min_size: 2 * 1024,
            avg_size: 16 * 1024,
            max_size: 64 * 1024,
        }
    }
}

impl ChunkingConfig {
    pub(crate) fn validate(self) -> ChunkFileResult<Self> {
        if self.min_size == 0 {
            return Err(ChunkFileError::InvalidConfig("最小块大小必须大于 0".into()));
        }
        if !self.avg_size.is_power_of_two() {
            return Err(ChunkFileError::InvalidConfig(
                "平均块大小必须是 2 的幂".into(),
            ));
        }
        if self.min_size > self.avg_size || self.avg_size > self.max_size {
            return Err(ChunkFileError::InvalidConfig(
                "块大小必须满足 min <= avg <= max".into(),
            ));
        }
        if self.max_size > MAX_ALLOWED_CHUNK_SIZE {
            return Err(ChunkFileError::InvalidConfig(format!(
                "最大块大小不能超过 {} MiB",
                MAX_ALLOWED_CHUNK_SIZE / (1024 * 1024)
            )));
        }
        if self.max_size > u32::MAX as usize {
            return Err(ChunkFileError::InvalidConfig(
                "最大块大小超过格式允许范围".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChunkDigest([u8; DIGEST_SIZE]);

impl ChunkDigest {
    pub(crate) fn as_bytes(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }

    pub(crate) fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(DIGEST_SIZE * 2);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }
}

impl fmt::Debug for ChunkDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl fmt::Display for ChunkDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl From<[u8; DIGEST_SIZE]> for ChunkDigest {
    fn from(value: [u8; DIGEST_SIZE]) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChunkKey {
    digest: ChunkDigest,
    length: u32,
}

#[derive(Debug, Clone, Copy)]
struct ChunkRecord {
    key: ChunkKey,
    data_offset: u64,
    /// Bytes stored for this record. Zero marks a v2 dedup reference whose
    /// payload lives at `data_offset` under an earlier record with the same key.
    stored_length: u32,
}

/// A readable, seekable source of chunk payload bytes.
pub(crate) trait ChunkSource: Read + Seek {}
impl<T: Read + Seek> ChunkSource for T {}

struct Segment<'a> {
    source: Box<dyn ChunkSource + 'a>,
    start: u64,
    length: u64,
}

/// A flat, append-only address space over chunk payload sources.
///
/// Resolving a delta chain used to rewrite a complete snapshot for every step,
/// so materializing a node far from the nearest snapshot cost one full file
/// write per hop. Appending each delta payload as a segment instead lets the
/// chain be resolved without touching the payload bytes at all: the resulting
/// layout addresses every chunk through virtual offsets, and only the consumer
/// that really needs bytes (export, checkpoint publication, hash verification)
/// reads them.
pub(crate) struct ChunkStore<'a> {
    segments: Vec<Segment<'a>>,
    length: u64,
}

impl<'a> Default for ChunkStore<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ChunkStore<'a> {
    pub(crate) fn new() -> Self {
        Self {
            segments: Vec::new(),
            length: 0,
        }
    }

    pub(crate) fn append_file(&mut self, file: File) -> ChunkFileResult<u64> {
        self.append(Box::new(file))
    }

    pub(crate) fn append(&mut self, mut source: Box<dyn ChunkSource + 'a>) -> ChunkFileResult<u64> {
        let length = source.seek(SeekFrom::End(0))?;
        let start = self.length;
        self.length = start
            .checked_add(length)
            .ok_or_else(|| invalid_format(ErrorDimension::File, "块存储大小溢出"))?;
        self.segments.push(Segment {
            source,
            start,
            length,
        });
        Ok(start)
    }

    pub(crate) fn reader<'store>(&'store mut self) -> ChunkReader<'store, 'a> {
        ChunkReader {
            store: self,
            offset: 0,
            cached_segment: None,
        }
    }

    /// Segments are appended in virtual-offset order, so a jump can binary
    /// search instead of scanning every segment. Empty segments are skipped.
    fn segment_index(&self, offset: u64) -> Option<usize> {
        let mut index = self
            .segments
            .partition_point(|segment| segment.start <= offset);
        while index > 0 {
            let candidate = index - 1;
            let segment = &self.segments[candidate];
            if segment.start <= offset && offset < segment.start + segment.length {
                return Some(candidate);
            }
            if segment.length == 0 {
                index -= 1;
                continue;
            }
            break;
        }
        None
    }
}

pub(crate) struct ChunkReader<'store, 'source> {
    store: &'store mut ChunkStore<'source>,
    offset: u64,
    /// Sequential reads stay in the same segment; a jump invalidates this and
    /// falls back to the binary search above.
    cached_segment: Option<usize>,
}

impl Read for ChunkReader<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut written = 0;
        while written < buffer.len() {
            if self.offset >= self.store.length {
                break;
            }
            let index = match self.cached_segment {
                Some(index)
                    if self.store.segments.get(index).is_some_and(|segment| {
                        self.offset >= segment.start && self.offset < segment.start + segment.length
                    }) =>
                {
                    index
                }
                _ => match self.store.segment_index(self.offset) {
                    Some(index) => index,
                    None => break,
                },
            };
            self.cached_segment = Some(index);
            let segment = &mut self.store.segments[index];
            let local = self.offset - segment.start;
            let available = usize::try_from(segment.length - local).unwrap_or(usize::MAX);
            let want = (buffer.len() - written).min(available);
            if want == 0 {
                break;
            }
            segment.source.seek(SeekFrom::Start(local))?;
            let read = segment.source.read(&mut buffer[written..written + want])?;
            if read == 0 {
                break;
            }
            written += read;
            self.offset += read as u64;
        }
        Ok(written)
    }
}

impl Seek for ChunkReader<'_, '_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let length = i64::try_from(self.store.length)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "块存储大小超出范围"))?;
        let current = i64::try_from(self.offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "块存储偏移超出范围"))?;
        let offset =
            match position {
                SeekFrom::Start(value) => Some(i64::try_from(value).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "块存储偏移超出范围")
                })?),
                SeekFrom::End(value) => length.checked_add(value),
                SeekFrom::Current(value) => current.checked_add(value),
            }
            .filter(|value| *value >= 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "块存储偏移无效"))?;
        self.offset = offset as u64;
        self.cached_segment = None;
        Ok(self.offset)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkFile {
    config: ChunkingConfig,
    logical_size: u64,
    file_digest: ChunkDigest,
    chunks: Vec<ChunkRecord>,
    chunk_index: HashMap<ChunkKey, usize>,
    /// Canonical v2 ids (0=CDC, 1=CLIP, 2=PSD); v1 layouts use 0.
    chunker: u16,
    adapter: u16,
}

impl ChunkFile {
    pub(crate) fn create<R, W>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        let config = config.validate()?;
        require_empty_output(snapshot, "snapshot")?;

        let structural_units = if options.format == FormatVersion::V2
            && options.chunking == ChunkingStrategy::ContainerAware
        {
            plan_container_chunks(source, task)?
        } else {
            None
        };
        let (chunker, adapter) = if structural_units.is_some() {
            (V2_CHUNKER_CLIP, V2_ADAPTER_CLIP)
        } else {
            (V2_CHUNKER_CDC, V2_ADAPTER_NONE)
        };

        write_snapshot_header(
            snapshot,
            options.format,
            chunker,
            adapter,
            config,
            0,
            0,
            ChunkDigest::from([0; DIGEST_SIZE]),
        )?;

        let mut writer = SnapshotWriter::new(snapshot, options.format);
        source.seek(SeekFrom::Start(0))?;
        match structural_units {
            Some(units) => {
                for unit in units {
                    task.check()?;
                    source.seek(SeekFrom::Start(unit.start))?;
                    let length = unit.len();
                    if length > config.max_size as u64 {
                        stream_cdc(source, Some(length), config, task, &mut |data, flags| {
                            writer.emit(data, flags)?;
                            task.report(data.len() as u64);
                            Ok(())
                        })?;
                    } else {
                        let mut buffer = vec![
                            0_u8;
                            usize::try_from(length).map_err(|_| {
                                invalid_format(ErrorDimension::Chunk, "结构单元长度超出平台范围")
                            })?
                        ];
                        source.read_exact(&mut buffer)?;
                        writer.emit(&buffer, V2_RECORD_FLAG_STRUCTURAL)?;
                        task.report(length);
                    }
                }
            }
            None => {
                source.seek(SeekFrom::Start(0))?;
                stream_cdc(source, None, config, task, &mut |data, flags| {
                    writer.emit(data, flags)?;
                    task.report(data.len() as u64);
                    Ok(())
                })?;
            }
        }

        let end = writer.end_position()?;
        let (logical_size, chunks, file_digest) = writer.finish();
        snapshot.seek(SeekFrom::Start(0))?;
        write_snapshot_header(
            snapshot,
            options.format,
            chunker,
            adapter,
            config,
            logical_size,
            usize_to_u64(chunks.len(), "块数量")?,
            file_digest,
        )?;
        snapshot.seek(SeekFrom::Start(end))?;
        snapshot.flush()?;

        Ok(Self::from_records(
            config,
            logical_size,
            file_digest,
            chunks,
            chunker,
            adapter,
        ))
    }

    pub(crate) fn open<R>(snapshot: &mut R) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
    {
        let file_length = stream_length(snapshot)?;
        snapshot.seek(SeekFrom::Start(0))?;
        let header = read_snapshot_header(snapshot)?;
        let chunk_count = u64_to_usize(header.chunk_count, "块数量")?;
        let minimum_record_bytes = header.version.record_header_size() as u64;
        let possible_records = file_length
            .saturating_sub(snapshot.stream_position()?)
            .checked_div(minimum_record_bytes)
            .unwrap_or(0);
        if header.chunk_count > possible_records {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "块数量超过文件能够容纳的范围",
            ));
        }

        let mut chunks = Vec::with_capacity(chunk_count);
        let mut logical_size = 0_u64;
        // v2 dedup references point at an earlier payload with the same key.
        let mut stored_payloads: HashMap<ChunkKey, u64> = HashMap::with_capacity(chunk_count);
        // Structural chunking follows the container layout, so its units are not
        // bound by the CDC minimum length. v1 always reports the CDC chunker.
        let structural_chunking = header.chunker != V2_CHUNKER_CDC;
        for index in 0..chunk_count {
            let record = match header.version {
                FormatVersion::V1 => {
                    let key = read_chunk_key(snapshot)?;
                    validate_record_length(key.length, header.config, index + 1 == chunk_count)?;
                    let data_offset = snapshot.stream_position()?;
                    let data_end = data_offset
                        .checked_add(u64::from(key.length))
                        .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "块偏移溢出"))?;
                    if data_end > file_length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("第 {} 个块超出文件边界", index + 1),
                        ));
                    }
                    snapshot.seek(SeekFrom::Start(data_end))?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: key.length,
                    }
                }
                FormatVersion::V2 => {
                    let record_header = read_v2_record_header(snapshot)?;
                    let key = record_header.key;
                    if record_header.logical_len != key.length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!(
                                "第 {} 个块记录的逻辑长度 {} 与块长度 {} 不一致",
                                index + 1,
                                record_header.logical_len,
                                key.length
                            ),
                        ));
                    }
                    validate_record_length(
                        key.length,
                        header.config,
                        structural_chunking || index + 1 == chunk_count,
                    )?;
                    validate_v2_codec(record_header.codec, record_header.recipe, index)?;
                    if record_header.stored_len == 0 {
                        let data_offset = stored_payloads.get(&key).copied().ok_or_else(|| {
                            invalid_format(
                                ErrorDimension::Chunk,
                                format!("第 {} 个块引用了未存储的重复块 {}", index + 1, key.digest),
                            )
                        })?;
                        ChunkRecord {
                            key,
                            data_offset,
                            stored_length: 0,
                        }
                    } else {
                        if record_header.stored_len != key.length {
                            return Err(integrity(
                                ErrorDimension::Codec,
                                format!(
                                    "第 {} 个块的存储长度 {} 与逻辑长度 {} 不一致",
                                    index + 1,
                                    record_header.stored_len,
                                    key.length
                                ),
                            ));
                        }
                        let data_offset = snapshot.stream_position()?;
                        let data_end = data_offset
                            .checked_add(u64::from(record_header.stored_len))
                            .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "块偏移溢出"))?;
                        if data_end > file_length {
                            return Err(invalid_format(
                                ErrorDimension::Chunk,
                                format!("第 {} 个块超出文件边界", index + 1),
                            ));
                        }
                        snapshot.seek(SeekFrom::Start(data_end))?;
                        stored_payloads.entry(key).or_insert(data_offset);
                        ChunkRecord {
                            key,
                            data_offset,
                            stored_length: record_header.stored_len,
                        }
                    }
                }
            };
            logical_size = logical_size
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| invalid_format(ErrorDimension::File, "逻辑文件大小溢出"))?;
            chunks.push(record);
        }

        if snapshot.stream_position()? != file_length {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "snapshot 末尾存在未识别数据",
            ));
        }
        if logical_size != header.logical_size {
            return Err(invalid_format(
                ErrorDimension::File,
                format!(
                    "逻辑大小不匹配：头部为 {}，块合计为 {logical_size}",
                    header.logical_size
                ),
            ));
        }

        Ok(Self::from_records(
            header.config,
            logical_size,
            header.file_digest,
            chunks,
            header.chunker,
            header.adapter,
        ))
    }

    pub(crate) fn logical_size(&self) -> u64 {
        self.logical_size
    }

    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub(crate) fn file_digest(&self) -> ChunkDigest {
        self.file_digest
    }

    pub(crate) fn copy_original<R, W>(
        &self,
        snapshot: &mut R,
        output: &mut W,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write,
    {
        let mut file_hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = Vec::new();
        for (index, record) in self.chunks.iter().enumerate() {
            task.check()?;
            load_verified_chunk(snapshot, record, &mut buffer, index, task)?;
            output.write_all(&buffer)?;
            file_hasher.update(&buffer);
            copied = copied
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| integrity(ErrorDimension::File, "还原大小溢出"))?;
            task.report(u64::from(record.key.length));
        }
        output.flush()?;
        let digest = ChunkDigest::from(finalize_sha256(file_hasher));
        if copied != self.logical_size {
            return Err(integrity(
                ErrorDimension::File,
                format!("还原大小不匹配：预期 {}，实际 {copied}", self.logical_size),
            ));
        }
        if digest != self.file_digest {
            return Err(integrity(
                ErrorDimension::File,
                format!("整文件哈希不匹配：预期 {}，实际 {digest}", self.file_digest),
            ));
        }
        Ok(())
    }

    /// Writes a resolved layout as a complete snapshot file and returns the
    /// layout of the written file, verifying every chunk payload and the
    /// whole-file digest while streaming.
    pub(crate) fn write_snapshot<R, W>(
        &self,
        source: &mut R,
        output: &mut W,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<ChunkFile>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        require_empty_output(output, "目标 snapshot")?;
        let (chunker, adapter) = match options.format {
            FormatVersion::V1 => (V2_CHUNKER_CDC, V2_ADAPTER_NONE),
            FormatVersion::V2 => (self.chunker, self.adapter),
        };
        write_snapshot_header(
            output,
            options.format,
            chunker,
            adapter,
            self.config,
            self.logical_size,
            usize_to_u64(self.chunks.len(), "目标块数量")?,
            self.file_digest,
        )?;
        let mut writer = SnapshotWriter::new(output, options.format);
        let mut buffer = Vec::new();
        for (index, record) in self.chunks.iter().enumerate() {
            task.check()?;
            load_verified_chunk(source, record, &mut buffer, index, task)?;
            writer.emit(&buffer, 0)?;
            task.report(u64::from(record.key.length));
        }
        let end = writer.end_position()?;
        let (logical_size, chunks, file_digest) = writer.finish();
        if logical_size != self.logical_size || file_digest != self.file_digest {
            return Err(integrity(
                ErrorDimension::File,
                "物化 snapshot 的整文件摘要与布局不一致",
            ));
        }
        output.seek(SeekFrom::Start(0))?;
        write_snapshot_header(
            output,
            options.format,
            chunker,
            adapter,
            self.config,
            self.logical_size,
            usize_to_u64(chunks.len(), "目标块数量")?,
            self.file_digest,
        )?;
        output.seek(SeekFrom::Start(end))?;
        Ok(ChunkFile::from_records(
            self.config,
            logical_size,
            self.file_digest,
            chunks,
            chunker,
            adapter,
        ))
    }

    pub(crate) fn create_reverse_delta<R, W>(
        &self,
        parent: &ChunkFile,
        parent_snapshot: &mut R,
        delta_output: &mut W,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        require_empty_output(delta_output, "delta")?;
        let mut data_keys = Vec::new();
        let mut seen_data = HashSet::new();
        let mut ops = Vec::with_capacity(parent.chunks.len());

        for (index, record) in parent.chunks.iter().enumerate() {
            if index % CANCEL_CHECK_INTERVAL == 0 {
                task.check()?;
            }
            let kind = if self.chunk_index.contains_key(&record.key) {
                DeltaOpKind::Copy
            } else {
                if seen_data.insert(record.key) {
                    data_keys.push(record.key);
                }
                DeltaOpKind::Data
            };
            ops.push(DeltaOp {
                kind,
                key: record.key,
            });
        }

        let chunker = match options.format {
            FormatVersion::V1 => V2_CHUNKER_CDC,
            FormatVersion::V2 => parent.chunker,
        };
        let adapter = match options.format {
            FormatVersion::V1 => V2_ADAPTER_NONE,
            FormatVersion::V2 => parent.adapter,
        };
        let mut encoded = zstd::stream::write::Encoder::new(&mut *delta_output, 6)?;
        write_delta_header(
            &mut encoded,
            options.format,
            chunker,
            adapter,
            parent.config,
            self.file_digest,
            parent.file_digest,
            parent.logical_size,
            usize_to_u64(parent.chunks.len(), "目标块数量")?,
            usize_to_u64(data_keys.len(), "delta 数据块数量")?,
        )?;

        let mut buffer = Vec::new();
        for (index, key) in data_keys.iter().enumerate() {
            task.check()?;
            let parent_index = parent.chunk_index.get(key).ok_or_else(|| {
                integrity(
                    ErrorDimension::Chunk,
                    format!("父版本缺少声明的块 {}", key.digest),
                )
            })?;
            let record = parent.chunks[*parent_index];
            match options.format {
                FormatVersion::V1 => write_chunk_key(&mut encoded, record.key)?,
                FormatVersion::V2 => write_v2_record_header(
                    &mut encoded,
                    record.key.length,
                    record.key,
                    CODEC_NONE,
                    RECIPE_NONE,
                    0,
                )?,
            }
            load_verified_chunk(parent_snapshot, &record, &mut buffer, index, task)?;
            encoded.write_all(&buffer)?;
            task.report(u64::from(record.key.length));
        }

        for op in &ops {
            write_u8(&mut encoded, op.kind as u8)?;
            write_chunk_key(&mut encoded, op.key)?;
        }
        encoded.finish()?;
        delta_output.flush()?;
        Ok(())
    }

    fn from_records(
        config: ChunkingConfig,
        logical_size: u64,
        file_digest: ChunkDigest,
        chunks: Vec<ChunkRecord>,
        chunker: u16,
        adapter: u16,
    ) -> Self {
        let mut chunk_index = HashMap::with_capacity(chunks.len());
        for (index, record) in chunks.iter().enumerate() {
            chunk_index.entry(record.key).or_insert(index);
        }
        Self {
            config,
            logical_size,
            file_digest,
            chunks,
            chunk_index,
            chunker,
            adapter,
        }
    }
}

struct SnapshotWriter<'a, W> {
    output: &'a mut W,
    version: FormatVersion,
    dedup: HashMap<ChunkKey, u64>,
    chunks: Vec<ChunkRecord>,
    file_hasher: Sha256,
    logical_size: u64,
}

impl<'a, W: Write + Seek> SnapshotWriter<'a, W> {
    fn new(output: &'a mut W, version: FormatVersion) -> Self {
        Self {
            output,
            version,
            dedup: HashMap::new(),
            chunks: Vec::new(),
            file_hasher: Sha256::new(),
            logical_size: 0,
        }
    }

    fn emit(&mut self, data: &[u8], flags: u16) -> ChunkFileResult<()> {
        if data.is_empty() {
            return Err(invalid_format(ErrorDimension::Chunk, "不能写入空数据块"));
        }
        let length = u32::try_from(data.len())
            .map_err(|_| ChunkFileError::InvalidConfig("单块大小超过 u32".into()))?;
        let digest = ChunkDigest::from(sha256(data));
        let key = ChunkKey { digest, length };
        let record = match self.version {
            FormatVersion::V1 => {
                write_chunk_key(self.output, key)?;
                let data_offset = self.output.stream_position()?;
                self.output.write_all(data)?;
                ChunkRecord {
                    key,
                    data_offset,
                    stored_length: length,
                }
            }
            FormatVersion::V2 => match self.dedup.get(&key).copied() {
                Some(data_offset) => {
                    write_v2_record_header(self.output, 0, key, CODEC_NONE, RECIPE_NONE, flags)?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: 0,
                    }
                }
                None => {
                    write_v2_record_header(
                        self.output,
                        length,
                        key,
                        CODEC_NONE,
                        RECIPE_NONE,
                        flags,
                    )?;
                    // The payload starts after the 44-byte v2 record header.
                    let data_offset = self.output.stream_position()?;
                    self.output.write_all(data)?;
                    self.dedup.insert(key, data_offset);
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: length,
                    }
                }
            },
        };
        self.file_hasher.update(data);
        self.logical_size = self
            .logical_size
            .checked_add(u64::from(length))
            .ok_or_else(|| ChunkFileError::InvalidConfig("逻辑文件大小溢出".into()))?;
        self.chunks.push(record);
        Ok(())
    }

    fn end_position(&mut self) -> ChunkFileResult<u64> {
        Ok(self.output.stream_position()?)
    }

    fn finish(self) -> (u64, Vec<ChunkRecord>, ChunkDigest) {
        (
            self.logical_size,
            self.chunks,
            ChunkDigest::from(finalize_sha256(self.file_hasher)),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum DeltaOpKind {
    Copy = 1,
    Data = 2,
}

impl DeltaOpKind {
    fn from_byte(value: u8) -> ChunkFileResult<Self> {
        match value {
            1 => Ok(Self::Copy),
            2 => Ok(Self::Data),
            _ => Err(invalid_format(
                ErrorDimension::Chunk,
                format!("未知 delta 操作类型：{value}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DeltaOp {
    kind: DeltaOpKind,
    key: ChunkKey,
}

#[derive(Debug)]
pub(crate) struct ChunkFileDelta {
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    data_records: Vec<ChunkRecord>,
    data_index: HashMap<ChunkKey, usize>,
    ops: Vec<DeltaOp>,
    target_chunker: u16,
    target_adapter: u16,
    // The decompressed payload is disk-backed so multi-gigabyte history files
    // do not require matching resident memory during restore.
    payload: File,
}

impl ChunkFileDelta {
    #[cfg(test)]
    pub(crate) fn open<R>(delta: &mut R) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
    {
        let task = ChunkTask::untracked();
        Self::open_with(delta, tempfile, &task)
    }

    pub(crate) fn open_in<R>(
        delta: &mut R,
        directory: &Path,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
    {
        Self::open_with(delta, || tempfile_in(directory), task)
    }

    fn open_with<R, F>(
        delta: &mut R,
        create_payload: F,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
        F: FnOnce() -> io::Result<File>,
    {
        delta.seek(SeekFrom::Start(0))?;
        let mut prefix = [0_u8; 4];
        delta.read_exact(&mut prefix)?;
        delta.seek(SeekFrom::Start(0))?;
        let compressed = prefix == [0x28, 0xb5, 0x2f, 0xfd];
        let payload = create_payload()?;
        let mut payload = if compressed {
            let mut decoder = zstd::stream::read::Decoder::new(&mut *delta).map_err(|error| {
                invalid_format(ErrorDimension::Format, format!("无法解压 delta：{error}"))
            })?;
            spool_delta_payload(&mut decoder, payload, task).map_err(|error| match error {
                ChunkFileError::Io(error) => {
                    invalid_format(ErrorDimension::Format, format!("无法解压 delta：{error}"))
                }
                other => other,
            })?
        } else {
            spool_delta_payload(delta, payload, task)?
        };
        let file_length = payload.seek(SeekFrom::End(0))?;
        payload.seek(SeekFrom::Start(0))?;
        let header = read_delta_header(&mut payload)?;
        let data_count = u64_to_usize(header.data_count, "delta 数据块数量")?;
        let op_count = u64_to_usize(header.target_chunk_count, "delta 操作数量")?;
        if header.data_count > header.target_chunk_count {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "delta 数据块数量大于目标块数量",
            ));
        }

        let minimum_data_record = header.version.record_header_size() as u64;
        let remaining = file_length.saturating_sub(payload.stream_position()?);
        if header.data_count > remaining / minimum_data_record {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "delta 数据块数量超过文件能够容纳的范围",
            ));
        }

        // Delta data records are a subset of the target layout's chunks, and that
        // layout may hold structurally sized units or a short final CDC block, so
        // every record is exempt from the `min_size` bound. `max_size` still
        // applies, and the operation table pins the exact target layout.
        let mut data_records = Vec::with_capacity(data_count);
        let mut seen_data = HashSet::with_capacity(data_count);
        for index in 0..data_count {
            let record = match header.version {
                FormatVersion::V1 => {
                    let key = read_chunk_key(&mut payload)?;
                    validate_record_length(key.length, header.target_config, true)?;
                    if !seen_data.insert(key) {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 包含重复数据块：{}", key.digest),
                        ));
                    }
                    let data_offset = payload.stream_position()?;
                    let data_end = data_offset
                        .checked_add(u64::from(key.length))
                        .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "delta 块偏移溢出"))?;
                    if data_end > file_length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 第 {} 个数据块超出文件边界", index + 1),
                        ));
                    }
                    payload.seek(SeekFrom::Start(data_end))?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: key.length,
                    }
                }
                FormatVersion::V2 => {
                    let record_header = read_v2_record_header(&mut payload)?;
                    let key = record_header.key;
                    if record_header.logical_len != key.length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 第 {} 个数据块记录长度不一致", index + 1),
                        ));
                    }
                    validate_record_length(key.length, header.target_config, true)?;
                    validate_v2_codec(record_header.codec, record_header.recipe, index)?;
                    if record_header.stored_len == 0 {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 第 {} 个数据块缺少内容", index + 1),
                        ));
                    }
                    if record_header.stored_len != key.length {
                        return Err(integrity(
                            ErrorDimension::Codec,
                            format!("delta 第 {} 个数据块长度不一致", index + 1),
                        ));
                    }
                    if !seen_data.insert(key) {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 包含重复数据块：{}", key.digest),
                        ));
                    }
                    let data_offset = payload.stream_position()?;
                    let data_end = data_offset
                        .checked_add(u64::from(record_header.stored_len))
                        .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "delta 块偏移溢出"))?;
                    if data_end > file_length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 第 {} 个数据块超出文件边界", index + 1),
                        ));
                    }
                    payload.seek(SeekFrom::Start(data_end))?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: record_header.stored_len,
                    }
                }
            };
            data_records.push(record);
        }

        let op_bytes = header
            .target_chunk_count
            .checked_mul(DELTA_OP_SIZE as u64)
            .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "delta 操作区大小溢出"))?;
        if payload
            .stream_position()?
            .checked_add(op_bytes)
            .filter(|end| *end == file_length)
            .is_none()
        {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "delta 操作区长度与文件大小不匹配",
            ));
        }

        let data_keys: HashSet<_> = data_records.iter().map(|record| record.key).collect();
        let mut ops = Vec::with_capacity(op_count);
        let mut logical_size = 0_u64;
        for index in 0..op_count {
            if index % CANCEL_CHECK_INTERVAL == 0 {
                task.check()?;
            }
            let kind = DeltaOpKind::from_byte(read_u8(&mut payload)?)?;
            let key = read_chunk_key(&mut payload)?;
            validate_record_length(key.length, header.target_config, true)?;
            if kind == DeltaOpKind::Data && !data_keys.contains(&key) {
                return Err(invalid_format(
                    ErrorDimension::Chunk,
                    format!("DATA 操作缺少块内容：{}", key.digest),
                ));
            }
            logical_size = logical_size
                .checked_add(u64::from(key.length))
                .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "delta 目标大小溢出"))?;
            ops.push(DeltaOp { kind, key });
        }
        if logical_size != header.target_size {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                format!(
                    "delta 目标大小不匹配：头部为 {}，操作合计为 {logical_size}",
                    header.target_size
                ),
            ));
        }

        Ok(Self::from_records(
            header.target_config,
            header.base_digest,
            header.target_digest,
            header.target_size,
            data_records,
            ops,
            header.target_chunker,
            header.target_adapter,
            payload,
        ))
    }

    /// Resolves this delta against `base` without materializing a snapshot.
    ///
    /// The decompressed payload becomes a new segment of `store`, and the
    /// returned layout addresses every chunk through that store. Resolving a
    /// whole chain therefore costs one payload decompression per hop instead of
    /// one complete snapshot rewrite per hop. The recorded digest is carried
    /// over, so callers can compare the resolved layout with the history
    /// database without hashing intermediate results.
    pub(crate) fn resolve<'a>(
        self,
        base: &ChunkFile,
        store: &mut ChunkStore<'a>,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<ChunkFile> {
        if base.file_digest != self.base_digest {
            return Err(incompatible(
                ErrorDimension::File,
                format!(
                    "delta 基础版本为 {}，实际传入 {}",
                    self.base_digest, base.file_digest
                ),
            ));
        }
        let payload_start = store.append_file(self.payload)?;
        let mut logical_size = 0_u64;
        let mut chunks = Vec::with_capacity(self.ops.len());
        for (index, op) in self.ops.iter().enumerate() {
            if index % CANCEL_CHECK_INTERVAL == 0 {
                task.check()?;
            }
            let record = match op.kind {
                DeltaOpKind::Copy => {
                    let index = base.chunk_index.get(&op.key).ok_or_else(|| {
                        incompatible(
                            ErrorDimension::Chunk,
                            format!("基础版本缺少 COPY 块：{}", op.key.digest),
                        )
                    })?;
                    base.chunks[*index]
                }
                DeltaOpKind::Data => {
                    let index = self.data_index.get(&op.key).ok_or_else(|| {
                        invalid_format(
                            ErrorDimension::Chunk,
                            format!("delta 缺少 DATA 块：{}", op.key.digest),
                        )
                    })?;
                    let record = self.data_records[*index];
                    ChunkRecord {
                        key: record.key,
                        data_offset: payload_start
                            .checked_add(record.data_offset)
                            .ok_or_else(|| integrity(ErrorDimension::Chunk, "块偏移溢出"))?,
                        stored_length: record.stored_length,
                    }
                }
            };
            logical_size = logical_size
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| integrity(ErrorDimension::Chunk, "应用 delta 后大小溢出"))?;
            chunks.push(record);
            task.report(u64::from(record.key.length));
        }
        if logical_size != self.target_size {
            return Err(integrity(
                ErrorDimension::File,
                format!(
                    "应用 delta 后大小不匹配：预期 {}，实际 {logical_size}",
                    self.target_size
                ),
            ));
        }
        Ok(ChunkFile::from_records(
            self.target_config,
            logical_size,
            self.target_digest,
            chunks,
            self.target_chunker,
            self.target_adapter,
        ))
    }

    /// Materializes this delta into a complete snapshot file and returns the
    /// layout of the written file, whose chunk offsets address that file.
    ///
    /// Production consumers resolve chains and stream their result directly;
    /// the eager one-delta materialization only remains as the reference
    /// implementation for the equivalence tests.
    #[cfg(test)]
    pub(crate) fn apply<RB, W>(
        self,
        base: &ChunkFile,
        base_snapshot: &mut RB,
        target_snapshot: &mut W,
    ) -> ChunkFileResult<ChunkFile>
    where
        RB: Read + Seek,
        W: Write + Seek,
    {
        let task = ChunkTask::untracked();
        let mut store = ChunkStore::new();
        store.append(Box::new(base_snapshot))?;
        let resolved = self.resolve(base, &mut store, &task)?;
        let mut reader = store.reader();
        resolved.write_snapshot(&mut reader, target_snapshot, ChunkWriteOptions::V1, &task)
    }

    fn from_records(
        target_config: ChunkingConfig,
        base_digest: ChunkDigest,
        target_digest: ChunkDigest,
        target_size: u64,
        data_records: Vec<ChunkRecord>,
        ops: Vec<DeltaOp>,
        target_chunker: u16,
        target_adapter: u16,
        payload: File,
    ) -> Self {
        let mut data_index = HashMap::with_capacity(data_records.len());
        for (index, record) in data_records.iter().enumerate() {
            data_index.insert(record.key, index);
        }
        Self {
            target_config,
            base_digest,
            target_digest,
            target_size,
            data_records,
            data_index,
            ops,
            target_chunker,
            target_adapter,
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SnapshotHeader {
    version: FormatVersion,
    config: ChunkingConfig,
    logical_size: u64,
    chunk_count: u64,
    file_digest: ChunkDigest,
    chunker: u16,
    adapter: u16,
}

#[derive(Debug, Clone, Copy)]
struct DeltaHeader {
    version: FormatVersion,
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    target_chunk_count: u64,
    data_count: u64,
    target_chunker: u16,
    target_adapter: u16,
}

#[derive(Debug, Clone, Copy)]
struct CommonHeader {
    version: FormatVersion,
    chunker: u16,
    adapter: u16,
}

fn plan_container_chunks<R: Read + Seek>(
    source: &mut R,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<Option<Vec<StructuralUnit>>> {
    source.seek(SeekFrom::Start(0))?;
    let mut head = [0_u8; 8];
    let read = source.read(&mut head)?;
    match container::detect(&head[..read]) {
        Some(container::ContainerKind::Clip) => {
            match container::clip_structural_units(source, &|| task.is_cancelled()) {
                Ok(units) => Ok(Some(units)),
                Err(ContainerError::Cancelled) => Err(ChunkFileError::Cancelled),
                // A parse failure only loses structural alignment; CDC remains
                // the authoritative fallback and never changes stored bytes.
                Err(ContainerError::Invalid(_)) => Ok(None),
            }
        }
        Some(container::ContainerKind::Psd) | None => Ok(None),
    }
}

fn stream_cdc<R, F>(
    source: &mut R,
    limit: Option<u64>,
    config: ChunkingConfig,
    task: &ChunkTask<'_>,
    emit: &mut F,
) -> ChunkFileResult<()>
where
    R: Read,
    F: FnMut(&[u8], u16) -> ChunkFileResult<()>,
{
    let mut remaining = limit;
    if remaining == Some(0) {
        return Ok(());
    }
    let mut read_buffer = vec![0_u8; STREAM_BUFFER_SIZE];
    let mut chunk_buffer = Vec::with_capacity(config.max_size);
    let mut rolling = 0_u64;
    loop {
        task.check()?;
        let request = match remaining {
            Some(value) => usize::try_from(value.min(STREAM_BUFFER_SIZE as u64)).unwrap_or(0),
            None => STREAM_BUFFER_SIZE,
        };
        if request == 0 {
            break;
        }
        let read = source.read(&mut read_buffer[..request])?;
        if read == 0 {
            if remaining.is_some_and(|value| value > 0) {
                return Err(invalid_format(
                    ErrorDimension::Chunk,
                    "结构单元在读取过程中提前结束",
                ));
            }
            break;
        }
        if let Some(value) = &mut remaining {
            *value -= read as u64;
        }
        for &byte in &read_buffer[..read] {
            rolling = rolling.wrapping_shl(1).wrapping_add(u64::from(byte));
            chunk_buffer.push(byte);
            let length = chunk_buffer.len();
            let boundary =
                length >= config.min_size && (rolling & (config.avg_size as u64 - 1)) == 0;
            if boundary || length >= config.max_size {
                emit(&chunk_buffer, 0)?;
                chunk_buffer.clear();
                rolling = 0;
            }
        }
    }
    if !chunk_buffer.is_empty() {
        emit(&chunk_buffer, 0)?;
    }
    Ok(())
}

fn load_verified_chunk<R: Read + Seek + ?Sized>(
    source: &mut R,
    record: &ChunkRecord,
    buffer: &mut Vec<u8>,
    index: usize,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<()> {
    buffer.resize(record.key.length as usize, 0);
    source.seek(SeekFrom::Start(record.data_offset))?;
    // A chunk is at most 64 KiB in the default configuration. Reading it in
    // 64 KiB slices keeps the cancellation floor at the block/tile boundary
    // even if a caller raises `max_size`.
    let mut offset = 0;
    while offset < buffer.len() {
        task.check()?;
        let want = (buffer.len() - offset).min(64 * 1024);
        source.read_exact(&mut buffer[offset..offset + want])?;
        offset += want;
    }
    let digest = ChunkDigest::from(sha256(buffer));
    if digest != record.key.digest {
        return Err(integrity(
            ErrorDimension::Chunk,
            format!(
                "第 {} 个块哈希不匹配：预期 {}，实际 {digest}",
                index + 1,
                record.key.digest
            ),
        ));
    }
    Ok(())
}

fn write_snapshot_header<W: Write>(
    output: &mut W,
    version: FormatVersion,
    chunker: u16,
    adapter: u16,
    config: ChunkingConfig,
    logical_size: u64,
    chunk_count: u64,
    file_digest: ChunkDigest,
) -> ChunkFileResult<()> {
    output.write_all(&SNAPSHOT_MAGIC)?;
    write_u16(output, version.number())?;
    match version {
        FormatVersion::V1 => {
            write_u16(output, 0)?;
            write_u16(output, LEGACY_CHUNKER_SHIFT_ADD)?;
            write_u16(output, HASH_SHA256)?;
        }
        FormatVersion::V2 => {
            if chunker > V2_CHUNKER_PSD {
                return Err(ChunkFileError::InvalidConfig(format!(
                    "不支持的分块策略 {chunker}"
                )));
            }
            if adapter > V2_ADAPTER_PSD {
                return Err(ChunkFileError::InvalidConfig(format!(
                    "不支持的容器适配器 {adapter}"
                )));
            }
            write_u16(output, V2_FLAGS_CHUNK_RECORDS)?;
            write_u16(output, chunker)?;
            write_u16(output, HASH_SHA256)?;
            write_u16(output, u16::from(CODEC_NONE))?;
            write_u16(output, adapter)?;
        }
    }
    write_config(output, config)?;
    write_u64(output, logical_size)?;
    write_u64(output, chunk_count)?;
    output.write_all(file_digest.as_bytes())?;
    Ok(())
}

fn read_snapshot_header<R: Read>(input: &mut R) -> ChunkFileResult<SnapshotHeader> {
    expect_magic(input, SNAPSHOT_MAGIC, "snapshot")?;
    let common = read_common_header(input)?;
    let config = read_config(input)?.validate()?;
    let logical_size = read_u64(input)?;
    let chunk_count = read_u64(input)?;
    let file_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    if logical_size == 0 && chunk_count != 0 {
        return Err(invalid_format(ErrorDimension::File, "空文件不能包含数据块"));
    }
    if logical_size > 0 && chunk_count == 0 {
        return Err(invalid_format(ErrorDimension::File, "非空文件缺少数据块"));
    }
    Ok(SnapshotHeader {
        version: common.version,
        config,
        logical_size,
        chunk_count,
        file_digest,
        chunker: common.chunker,
        adapter: common.adapter,
    })
}

fn write_delta_header<W: Write>(
    output: &mut W,
    version: FormatVersion,
    chunker: u16,
    adapter: u16,
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    target_chunk_count: u64,
    data_count: u64,
) -> ChunkFileResult<()> {
    output.write_all(&DELTA_MAGIC)?;
    write_u16(output, version.number())?;
    match version {
        FormatVersion::V1 => {
            write_u16(output, 0)?;
            write_u16(output, LEGACY_CHUNKER_SHIFT_ADD)?;
            write_u16(output, HASH_SHA256)?;
        }
        FormatVersion::V2 => {
            if chunker > V2_CHUNKER_PSD {
                return Err(ChunkFileError::InvalidConfig(format!(
                    "不支持的分块策略 {chunker}"
                )));
            }
            if adapter > V2_ADAPTER_PSD {
                return Err(ChunkFileError::InvalidConfig(format!(
                    "不支持的容器适配器 {adapter}"
                )));
            }
            write_u16(output, V2_FLAGS_CHUNK_RECORDS)?;
            write_u16(output, chunker)?;
            write_u16(output, HASH_SHA256)?;
            write_u16(output, u16::from(CODEC_NONE))?;
            write_u16(output, adapter)?;
        }
    }
    write_config(output, target_config)?;
    output.write_all(base_digest.as_bytes())?;
    output.write_all(target_digest.as_bytes())?;
    write_u64(output, target_size)?;
    write_u64(output, target_chunk_count)?;
    write_u64(output, data_count)?;
    Ok(())
}

fn read_delta_header<R: Read>(input: &mut R) -> ChunkFileResult<DeltaHeader> {
    expect_magic(input, DELTA_MAGIC, "delta")?;
    let common = read_common_header(input)?;
    let target_config = read_config(input)?.validate()?;
    let base_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let target_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let target_size = read_u64(input)?;
    let target_chunk_count = read_u64(input)?;
    let data_count = read_u64(input)?;
    if target_size == 0 && target_chunk_count != 0 {
        return Err(invalid_format(
            ErrorDimension::File,
            "空目标不能包含 delta 操作",
        ));
    }
    if target_size > 0 && target_chunk_count == 0 {
        return Err(invalid_format(
            ErrorDimension::File,
            "非空目标缺少 delta 操作",
        ));
    }
    Ok(DeltaHeader {
        version: common.version,
        target_config,
        base_digest,
        target_digest,
        target_size,
        target_chunk_count,
        data_count,
        target_chunker: common.chunker,
        target_adapter: common.adapter,
    })
}

fn spool_delta_payload<R: Read>(
    input: &mut R,
    mut payload: File,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<File> {
    task.check()?;
    let mut prefix = [0_u8; 16];
    input.read_exact(&mut prefix)?;
    let version = FormatVersion::from_number(u16::from_le_bytes(
        prefix[8..10].try_into().expect("2-byte version"),
    ))?;
    let header_size = version.delta_header_size();
    let mut header_bytes = vec![0_u8; header_size];
    header_bytes[..16].copy_from_slice(&prefix);
    input.read_exact(&mut header_bytes[16..])?;
    let header = read_delta_header(&mut Cursor::new(header_bytes.as_slice()))?;
    let declared_limit = delta_payload_limit(&header)?;
    let remaining_limit = declared_limit
        .checked_sub(header_size as u64)
        .ok_or_else(|| invalid_format(ErrorDimension::Format, "delta 声明大小小于头部"))?;

    payload.write_all(&header_bytes)?;
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; STREAM_BUFFER_SIZE];
    loop {
        task.check()?;
        let request = usize::try_from(
            (remaining_limit.saturating_sub(copied))
                .saturating_add(1)
                .min(STREAM_BUFFER_SIZE as u64),
        )
        .unwrap_or(buffer.len());
        let read = input.read(&mut buffer[..request])?;
        if read == 0 {
            break;
        }
        copied += read as u64;
        if copied > remaining_limit {
            return Err(invalid_format(
                ErrorDimension::Format,
                format!("delta 解压内容超过格式声明的最大范围 {declared_limit} 字节"),
            ));
        }
        payload.write_all(&buffer[..read])?;
        task.report(read as u64);
    }
    payload.flush()?;
    payload.seek(SeekFrom::Start(0))?;
    Ok(payload)
}

fn delta_payload_limit(header: &DeltaHeader) -> ChunkFileResult<u64> {
    let data_record_bytes = header
        .data_count
        .checked_mul(header.version.record_header_size() as u64)
        .ok_or_else(|| invalid_format(ErrorDimension::Format, "delta 数据记录区大小溢出"))?;
    let operation_bytes = header
        .target_chunk_count
        .checked_mul(DELTA_OP_SIZE as u64)
        .ok_or_else(|| invalid_format(ErrorDimension::Format, "delta 操作区大小溢出"))?;
    (header.version.delta_header_size() as u64)
        .checked_add(header.target_size)
        .and_then(|value| value.checked_add(data_record_bytes))
        .and_then(|value| value.checked_add(operation_bytes))
        .ok_or_else(|| invalid_format(ErrorDimension::Format, "delta 声明的解压大小溢出"))
}

fn read_common_header<R: Read>(input: &mut R) -> ChunkFileResult<CommonHeader> {
    let version = FormatVersion::from_number(read_u16(input)?)?;
    let flags = read_u16(input)?;
    let chunker = read_u16(input)?;
    let hash = read_u16(input)?;
    match version {
        FormatVersion::V1 => {
            if flags != 0 {
                return Err(incompatible(
                    ErrorDimension::Format,
                    format!("不支持格式标志 {flags:#06x}"),
                ));
            }
            if chunker != LEGACY_CHUNKER_SHIFT_ADD {
                return Err(incompatible(
                    ErrorDimension::Chunker,
                    format!("不支持分块算法 {chunker}"),
                ));
            }
            if hash != HASH_SHA256 {
                return Err(incompatible(
                    ErrorDimension::Hash,
                    format!("不支持哈希算法 {hash}"),
                ));
            }
            Ok(CommonHeader {
                version,
                chunker: V2_CHUNKER_CDC,
                adapter: V2_ADAPTER_NONE,
            })
        }
        FormatVersion::V2 => {
            if flags != V2_FLAGS_CHUNK_RECORDS {
                return Err(incompatible(
                    ErrorDimension::Format,
                    format!("不支持格式标志 {flags:#06x}"),
                ));
            }
            if chunker > V2_CHUNKER_PSD {
                return Err(incompatible(
                    ErrorDimension::Chunker,
                    format!("不支持分块算法 {chunker}"),
                ));
            }
            if hash != HASH_SHA256 {
                return Err(incompatible(
                    ErrorDimension::Hash,
                    format!("不支持哈希算法 {hash}"),
                ));
            }
            let codec = read_u16(input)?;
            if codec != u16::from(CODEC_NONE) {
                return Err(incompatible(
                    ErrorDimension::Codec,
                    format!("不支持默认编解码 {codec}"),
                ));
            }
            let adapter = read_u16(input)?;
            if adapter > V2_ADAPTER_PSD {
                return Err(incompatible(
                    ErrorDimension::Adapter,
                    format!("不支持容器适配器 {adapter}"),
                ));
            }
            Ok(CommonHeader {
                version,
                chunker,
                adapter,
            })
        }
    }
}

fn validate_v2_codec(codec: u8, recipe: u8, index: usize) -> ChunkFileResult<()> {
    if codec != CODEC_NONE {
        return Err(incompatible(
            ErrorDimension::Codec,
            format!("第 {} 个块使用未知编解码 {codec}", index + 1),
        ));
    }
    if recipe != RECIPE_NONE {
        return Err(incompatible(
            ErrorDimension::Recipe,
            format!("第 {} 个块使用未知配方 {recipe}", index + 1),
        ));
    }
    Ok(())
}

fn write_config<W: Write>(output: &mut W, config: ChunkingConfig) -> ChunkFileResult<()> {
    write_u32(output, usize_to_u32(config.min_size, "最小块大小")?)?;
    write_u32(output, usize_to_u32(config.avg_size, "平均块大小")?)?;
    write_u32(output, usize_to_u32(config.max_size, "最大块大小")?)?;
    Ok(())
}

fn read_config<R: Read>(input: &mut R) -> ChunkFileResult<ChunkingConfig> {
    Ok(ChunkingConfig {
        min_size: read_u32(input)? as usize,
        avg_size: read_u32(input)? as usize,
        max_size: read_u32(input)? as usize,
    })
}

fn write_chunk_key<W: Write>(output: &mut W, key: ChunkKey) -> ChunkFileResult<()> {
    output.write_all(key.digest.as_bytes())?;
    write_u32(output, key.length)?;
    Ok(())
}

fn read_chunk_key<R: Read>(input: &mut R) -> ChunkFileResult<ChunkKey> {
    let digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let length = read_u32(input)?;
    Ok(ChunkKey { digest, length })
}

fn write_v2_record_header<W: Write>(
    output: &mut W,
    stored_len: u32,
    key: ChunkKey,
    codec: u8,
    recipe: u8,
    flags: u16,
) -> ChunkFileResult<()> {
    output.write_all(key.digest.as_bytes())?;
    write_u32(output, stored_len)?;
    write_u32(output, key.length)?;
    write_u8(output, codec)?;
    write_u8(output, recipe)?;
    write_u16(output, flags)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct V2RecordHeader {
    key: ChunkKey,
    stored_len: u32,
    logical_len: u32,
    codec: u8,
    recipe: u8,
}

fn read_v2_record_header<R: Read>(input: &mut R) -> ChunkFileResult<V2RecordHeader> {
    let digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let stored_len = read_u32(input)?;
    let logical_len = read_u32(input)?;
    let codec = read_u8(input)?;
    let recipe = read_u8(input)?;
    let _flags = read_u16(input)?;
    Ok(V2RecordHeader {
        key: ChunkKey {
            digest,
            length: logical_len,
        },
        stored_len,
        logical_len,
        codec,
        recipe,
    })
}

/// Checks one record length against the chunking config.
///
/// `min_size_exempt` marks records that may be shorter than `config.min_size`.
/// The final record of a stream qualifies because CDC can only close a block
/// when a boundary fires, and every record of a structurally chunked container
/// qualifies because structural units follow the container layout (a 24 B file
/// header, a 56 B `Exta` header) instead of the CDC size policy. `max_size`
/// always applies: oversized units are split, never stored whole.
fn validate_record_length(
    length: u32,
    config: ChunkingConfig,
    min_size_exempt: bool,
) -> ChunkFileResult<()> {
    let length = length as usize;
    if length == 0 {
        return Err(invalid_format(ErrorDimension::Chunk, "数据块长度不能为 0"));
    }
    if length > config.max_size {
        return Err(invalid_format(
            ErrorDimension::Chunk,
            format!("数据块长度 {length} 超过最大块大小 {}", config.max_size),
        ));
    }
    if !min_size_exempt && length < config.min_size {
        return Err(invalid_format(
            ErrorDimension::Chunk,
            format!(
                "非末尾数据块长度 {length} 小于最小块大小 {}",
                config.min_size
            ),
        ));
    }
    Ok(())
}

fn require_empty_output<W: Seek>(output: &mut W, label: &str) -> ChunkFileResult<()> {
    let length = output.seek(SeekFrom::End(0))?;
    if length != 0 {
        return Err(ChunkFileError::OutputConflict(format!(
            "{label} 输出必须是空文件"
        )));
    }
    output.seek(SeekFrom::Start(0))?;
    Ok(())
}

fn stream_length<R: Seek>(input: &mut R) -> ChunkFileResult<u64> {
    let current = input.stream_position()?;
    let length = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(current))?;
    Ok(length)
}

fn expect_magic<R: Read>(input: &mut R, expected: [u8; 8], label: &str) -> ChunkFileResult<()> {
    let actual = read_array::<8, _>(input)?;
    if actual != expected {
        return Err(invalid_format(
            ErrorDimension::Container,
            format!("{label} magic 不匹配"),
        ));
    }
    Ok(())
}

fn sha256(data: &[u8]) -> [u8; DIGEST_SIZE] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    finalize_sha256(hasher)
}

fn finalize_sha256(hasher: Sha256) -> [u8; DIGEST_SIZE] {
    hasher.finalize().into()
}

fn usize_to_u32(value: usize, label: &str) -> ChunkFileResult<u32> {
    u32::try_from(value)
        .map_err(|_| ChunkFileError::InvalidConfig(format!("{label} 超过 u32 范围")))
}

fn usize_to_u64(value: usize, label: &str) -> ChunkFileResult<u64> {
    u64::try_from(value)
        .map_err(|_| ChunkFileError::InvalidConfig(format!("{label} 超过 u64 范围")))
}

fn u64_to_usize(value: u64, label: &str) -> ChunkFileResult<usize> {
    usize::try_from(value)
        .map_err(|_| invalid_format(ErrorDimension::File, format!("{label} 超过平台范围")))
}

fn read_array<const N: usize, R: Read>(input: &mut R) -> ChunkFileResult<[u8; N]> {
    let mut bytes = [0_u8; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_u8<W: Write>(output: &mut W, value: u8) -> ChunkFileResult<()> {
    output.write_all(&[value])?;
    Ok(())
}

fn write_u16<W: Write>(output: &mut W, value: u16) -> ChunkFileResult<()> {
    output.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u32<W: Write>(output: &mut W, value: u32) -> ChunkFileResult<()> {
    output.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64<W: Write>(output: &mut W, value: u64) -> ChunkFileResult<()> {
    output.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u8<R: Read>(input: &mut R) -> ChunkFileResult<u8> {
    Ok(read_array::<1, _>(input)?[0])
}

fn read_u16<R: Read>(input: &mut R) -> ChunkFileResult<u16> {
    Ok(u16::from_le_bytes(read_array::<2, _>(input)?))
}

fn read_u32<R: Read>(input: &mut R) -> ChunkFileResult<u32> {
    Ok(u32::from_le_bytes(read_array::<4, _>(input)?))
}

fn read_u64<R: Read>(input: &mut R) -> ChunkFileResult<u64> {
    Ok(u64::from_le_bytes(read_array::<8, _>(input)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backup::test_support::{QuotaWriter, Rng};
    use crate::fuzz_support::{mutated, mutated_header, random_bytes};

    fn v1_create<R: Read + Seek, W: Write + Seek>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
    ) -> ChunkFileResult<ChunkFile> {
        let task = ChunkTask::untracked();
        ChunkFile::create(source, snapshot, config, ChunkWriteOptions::V1, &task)
    }

    fn v2_create<R: Read + Seek, W: Write + Seek>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
        chunking: ChunkingStrategy,
    ) -> ChunkFileResult<ChunkFile> {
        let task = ChunkTask::untracked();
        ChunkFile::create(
            source,
            snapshot,
            config,
            ChunkWriteOptions {
                format: FormatVersion::V2,
                chunking,
            },
            &task,
        )
    }

    fn encode_snapshot(bytes: &[u8]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        v1_create(
            &mut Cursor::new(bytes),
            &mut output,
            ChunkingConfig::default(),
        )
        .unwrap();
        output.into_inner()
    }

    fn encode_v2_snapshot(bytes: &[u8]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        v2_create(
            &mut Cursor::new(bytes),
            &mut output,
            ChunkingConfig::default(),
            ChunkingStrategy::Cdc,
        )
        .unwrap();
        output.into_inner()
    }

    fn encode_reverse_delta(child: &[u8], parent: &[u8]) -> Vec<u8> {
        let config = ChunkingConfig::default();
        let task = ChunkTask::untracked();
        let mut child_snapshot = Cursor::new(Vec::new());
        let child_chunk = v1_create(&mut Cursor::new(child), &mut child_snapshot, config).unwrap();
        let mut parent_snapshot = Cursor::new(Vec::new());
        let parent_chunk =
            v1_create(&mut Cursor::new(parent), &mut parent_snapshot, config).unwrap();
        let mut encoded = Cursor::new(Vec::new());
        child_chunk
            .create_reverse_delta(
                &parent_chunk,
                &mut parent_snapshot,
                &mut encoded,
                ChunkWriteOptions::V1,
                &task,
            )
            .unwrap();
        encoded.into_inner()
    }

    fn encode_v2_reverse_delta(child: &[u8], parent: &[u8]) -> Vec<u8> {
        let config = ChunkingConfig::default();
        let task = ChunkTask::untracked();
        let mut child_snapshot = Cursor::new(Vec::new());
        let child_chunk = v2_create(
            &mut Cursor::new(child),
            &mut child_snapshot,
            config,
            ChunkingStrategy::Cdc,
        )
        .unwrap();
        let mut parent_snapshot = Cursor::new(Vec::new());
        let parent_chunk = v2_create(
            &mut Cursor::new(parent),
            &mut parent_snapshot,
            config,
            ChunkingStrategy::Cdc,
        )
        .unwrap();
        let mut encoded = Cursor::new(Vec::new());
        child_chunk
            .create_reverse_delta(
                &parent_chunk,
                &mut parent_snapshot,
                &mut encoded,
                ChunkWriteOptions::V2,
                &task,
            )
            .unwrap();
        encoded.into_inner()
    }

    #[test]
    fn resolved_chain_matches_eager_materialization() {
        // A three-version chain whose middle revision introduces fresh chunk
        // payloads, so resolving the oldest revision has to copy chunk bytes
        // out of an earlier delta payload segment instead of the base snapshot.
        let oldest = vec![b'A'; 96 * 1024];
        let mut middle = oldest.clone();
        middle[32 * 1024..40 * 1024].copy_from_slice(&vec![b'B'; 8 * 1024]);
        let mut newest = middle.clone();
        newest[72 * 1024..84 * 1024].copy_from_slice(&vec![b'C'; 12 * 1024]);

        let newest_snapshot = encode_snapshot(&newest);
        let mut store = ChunkStore::new();
        store
            .append(Box::new(Cursor::new(newest_snapshot.clone())))
            .unwrap();
        let mut layout = ChunkFile::open(&mut Cursor::new(newest_snapshot)).unwrap();
        let task = ChunkTask::untracked();
        for (child, parent) in [(&newest, &middle), (&middle, &oldest)] {
            let delta = ChunkFileDelta::open(&mut Cursor::new(encode_reverse_delta(child, parent)))
                .unwrap();
            layout = delta.resolve(&layout, &mut store, &task).unwrap();
            assert_eq!(
                layout.file_digest().to_hex(),
                ChunkFile::open(&mut Cursor::new(encode_snapshot(parent)))
                    .unwrap()
                    .file_digest()
                    .to_hex()
            );
        }

        let mut materialized = Cursor::new(Vec::new());
        {
            let mut reader = store.reader();
            layout
                .write_snapshot(&mut reader, &mut materialized, ChunkWriteOptions::V1, &task)
                .unwrap();
        }
        assert_eq!(materialized.into_inner(), encode_snapshot(&oldest));

        let mut restored = Vec::new();
        {
            let mut reader = store.reader();
            layout
                .copy_original(&mut reader, &mut restored, &task)
                .unwrap();
        }
        assert_eq!(restored, oldest);
    }

    #[test]
    fn v2_snapshot_round_trips_and_deduplicates_repeated_payloads() {
        let repeated = vec![b'D'; 64 * 1024];
        let bytes = [repeated.clone(), repeated.clone(), repeated].concat();

        let mut v1 = Cursor::new(Vec::new());
        v1_create(&mut Cursor::new(&bytes), &mut v1, ChunkingConfig::default()).unwrap();
        let mut v2 = Cursor::new(Vec::new());
        let layout = v2_create(
            &mut Cursor::new(&bytes),
            &mut v2,
            ChunkingConfig::default(),
            ChunkingStrategy::Cdc,
        )
        .unwrap();
        let v1 = v1.into_inner();
        let v2 = v2.into_inner();

        assert!(v2.len() < v1.len(), "v2 dedup should shrink repeated data");
        let opened = ChunkFile::open(&mut Cursor::new(v2.clone())).unwrap();
        let task = ChunkTask::untracked();
        let mut restored = Vec::new();
        opened
            .copy_original(&mut Cursor::new(v2), &mut restored, &task)
            .unwrap();
        assert_eq!(restored, bytes);
        assert!(layout.chunks.iter().any(|record| record.stored_length == 0));
    }

    #[test]
    fn v1_snapshot_stays_byte_identical_to_the_legacy_layout() {
        let bytes = vec![b'L'; 48 * 1024];
        let encoded = encode_snapshot(&bytes);
        assert_eq!(
            u16::from_le_bytes(encoded[8..10].try_into().unwrap()),
            FORMAT_VERSION_V1
        );
        assert_eq!(
            encoded.len(),
            V1_SNAPSHOT_HEADER_SIZE + V1_RECORD_HEADER_SIZE + bytes.len()
        );
        let opened = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
        let task = ChunkTask::untracked();
        let mut restored = Vec::new();
        opened
            .copy_original(&mut Cursor::new(encoded), &mut restored, &task)
            .unwrap();
        assert_eq!(restored, bytes);
    }

    #[test]
    fn v2_delta_round_trips_against_a_v1_or_v2_base() {
        let parent = vec![b'P'; 96 * 1024];
        let mut child = parent.clone();
        child[16 * 1024..20 * 1024].copy_from_slice(&[b'X'; 4 * 1024]);

        let encoded = encode_v2_reverse_delta(&child, &parent);
        let delta = ChunkFileDelta::open(&mut Cursor::new(encoded.clone())).unwrap();
        let task = ChunkTask::untracked();
        let mut child_snapshot = Cursor::new(Vec::new());
        let child_chunk = v2_create(
            &mut Cursor::new(&child),
            &mut child_snapshot,
            ChunkingConfig::default(),
            ChunkingStrategy::Cdc,
        )
        .unwrap();
        let mut restored_snapshot = Cursor::new(Vec::new());
        let restored = delta
            .apply(&child_chunk, &mut child_snapshot, &mut restored_snapshot)
            .unwrap();
        let mut restored_bytes = Vec::new();
        restored
            .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
            .unwrap();
        assert_eq!(restored_bytes, parent);

        // A v1 base remains readable underneath a v2 chain.
        let mut v1_base = Cursor::new(Vec::new());
        let v1_layout = v1_create(
            &mut Cursor::new(&child),
            &mut v1_base,
            ChunkingConfig::default(),
        )
        .unwrap();
        let mut store = ChunkStore::new();
        store
            .append(Box::new(Cursor::new(encode_snapshot(&child))))
            .unwrap();
        let resolved = ChunkFileDelta::open(&mut Cursor::new(encoded))
            .unwrap()
            .resolve(&v1_layout, &mut store, &task)
            .unwrap();
        assert_eq!(
            resolved.file_digest().to_hex(),
            ChunkFile::open(&mut Cursor::new(encode_snapshot(&parent)))
                .unwrap()
                .file_digest()
                .to_hex()
        );
    }

    #[test]
    fn v2_container_chunking_aligns_to_clip_tiles() {
        // Synthetic CSFC with two small tile records. Structural chunking must
        // produce one chunk per unit, including the header and trailing unit.
        let file = synthetic_clip();
        let mut output = Cursor::new(Vec::new());
        let layout = v2_create(
            &mut Cursor::new(&file),
            &mut output,
            ChunkingConfig::default(),
            ChunkingStrategy::ContainerAware,
        )
        .unwrap();
        assert_eq!(layout.chunker, V2_CHUNKER_CLIP);
        assert_eq!(layout.adapter, V2_ADAPTER_CLIP);
        assert!(layout.chunks.len() >= 4);
        let mut restored = Vec::new();
        let task = ChunkTask::untracked();
        layout
            .copy_original(&mut Cursor::new(output.into_inner()), &mut restored, &task)
            .unwrap();
        assert_eq!(restored, file);
    }

    #[test]
    fn v2_structural_snapshot_can_be_reopened() {
        // Structural units are not CDC chunks: the container header (24 B) and
        // the Exta header (56 B) are far below `min_size`, so reopening the
        // written file must not apply the CDC minimum length to them.
        let file = synthetic_clip();
        let mut output = Cursor::new(Vec::new());
        v2_create(
            &mut Cursor::new(&file),
            &mut output,
            ChunkingConfig::default(),
            ChunkingStrategy::ContainerAware,
        )
        .unwrap();
        let encoded = output.into_inner();
        let reopened = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
        let mut restored = Vec::new();
        reopened
            .copy_original(
                &mut Cursor::new(encoded),
                &mut restored,
                &ChunkTask::untracked(),
            )
            .unwrap();
        assert_eq!(restored, file);
    }

    #[test]
    fn malformed_container_falls_back_to_cdc() {
        let mut bytes = synthetic_clip();
        bytes.truncate(bytes.len() - 3);
        let mut output = Cursor::new(Vec::new());
        let layout = v2_create(
            &mut Cursor::new(&bytes),
            &mut output,
            ChunkingConfig::default(),
            ChunkingStrategy::ContainerAware,
        )
        .unwrap();
        assert_eq!(layout.chunker, V2_CHUNKER_CDC);
        assert_eq!(layout.adapter, V2_ADAPTER_NONE);
        let task = ChunkTask::untracked();
        let mut restored = Vec::new();
        layout
            .copy_original(&mut Cursor::new(output.into_inner()), &mut restored, &task)
            .unwrap();
        assert_eq!(restored, bytes);
    }

    #[test]
    fn cancellation_stops_snapshot_creation_and_delta_reads() {
        let token = CancelToken::standalone();
        token.cancel();
        let task = ChunkTask::cancel(&token);
        let mut output = Cursor::new(Vec::new());
        let error = ChunkFile::create(
            &mut Cursor::new(vec![b'C'; 96 * 1024]),
            &mut output,
            ChunkingConfig::default(),
            ChunkWriteOptions::V2,
            &task,
        )
        .unwrap_err();
        assert!(matches!(error, ChunkFileError::Cancelled));

        let encoded = encode_reverse_delta(&[b'A'; 64 * 1024], &[b'B'; 64 * 1024]);
        let error =
            ChunkFileDelta::open_in(&mut Cursor::new(encoded), &std::env::temp_dir(), &task)
                .unwrap_err();
        assert!(matches!(error, ChunkFileError::Cancelled));
    }

    #[test]
    fn unknown_versions_and_records_are_incompatible_not_corrupt() {
        let mut snapshot = encode_v2_snapshot(&[b'V'; 96 * 1024]);
        snapshot[8..10].copy_from_slice(&9_u16.to_le_bytes());
        let error = ChunkFile::open(&mut Cursor::new(snapshot)).unwrap_err();
        assert!(matches!(
            error,
            ChunkFileError::Incompatible(ErrorDimension::Format, _)
        ));

        let mut codec = encode_v2_snapshot(&[b'V'; 96 * 1024]);
        let record = V2_SNAPSHOT_HEADER_SIZE;
        codec[record + 40] = 7; // codec byte
        let error = ChunkFile::open(&mut Cursor::new(codec)).unwrap_err();
        assert!(matches!(
            error,
            ChunkFileError::Incompatible(ErrorDimension::Codec, _)
        ));
    }

    #[test]
    fn a_corrupt_chunk_is_isolated_and_identified() {
        let bytes = vec![b'I'; 96 * 1024];
        let mut encoded = encode_v2_snapshot(&bytes);
        // Corrupt the first stored payload byte; the container itself still parses.
        let first_record = V2_SNAPSHOT_HEADER_SIZE;
        let stored_len = u32::from_le_bytes(
            encoded[first_record + 32..first_record + 36]
                .try_into()
                .unwrap(),
        ) as usize;
        assert!(stored_len > 0);
        let payload = first_record + V2_RECORD_HEADER_SIZE;
        encoded[payload] ^= 0xFF;

        let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
        let task = ChunkTask::untracked();
        let error = layout
            .copy_original(&mut Cursor::new(encoded), &mut std::io::sink(), &task)
            .unwrap_err();
        assert!(matches!(
            error,
            ChunkFileError::Integrity(ErrorDimension::Chunk, _)
        ));
        assert!(error.to_string().contains("第 1 个块"), "{error}");
    }

    #[test]
    fn chunk_store_reads_across_segments_and_seeks_backwards() {
        let mut store = ChunkStore::new();
        store.append(Box::new(Cursor::new(vec![b'A'; 10]))).unwrap();
        store.append(Box::new(Cursor::new(vec![b'B'; 5]))).unwrap();
        store.append(Box::new(Cursor::new(vec![b'C'; 7]))).unwrap();
        let mut reader = store.reader();
        let mut buffer = [0_u8; 20];
        reader.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer[..10], &[b'A'; 10]);
        assert_eq!(&buffer[10..15], &[b'B'; 5]);
        assert_eq!(&buffer[15..20], &[b'C'; 5]);

        reader.seek(SeekFrom::Start(12)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, [vec![b'B'; 3], vec![b'C'; 7]].concat());
    }

    #[test]
    fn resolved_layout_rejects_a_mismatched_base() {
        let current = vec![b'A'; 64 * 1024];
        let parent = vec![b'B'; 64 * 1024];
        let unrelated = vec![b'C'; 64 * 1024];
        let delta = ChunkFileDelta::open(&mut Cursor::new(encode_reverse_delta(&current, &parent)))
            .unwrap();
        let mut unrelated_snapshot = Cursor::new(Vec::new());
        let unrelated_chunk = v1_create(
            &mut Cursor::new(&unrelated),
            &mut unrelated_snapshot,
            ChunkingConfig::default(),
        )
        .unwrap();
        let mut store = ChunkStore::new();
        store.append(Box::new(Cursor::new(Vec::new()))).unwrap();
        let task = ChunkTask::untracked();

        let error = delta
            .resolve(&unrelated_chunk, &mut store, &task)
            .unwrap_err();

        assert!(
            matches!(error, ChunkFileError::Incompatible(_, _)),
            "{error}"
        );
    }

    #[test]
    fn compressed_delta_round_trip_uses_disk_backed_payload() {
        let config = ChunkingConfig::default();
        let current_bytes = b"current artwork bytes with a changed middle";
        let parent_bytes = b"parent artwork bytes with an older middle";
        let mut current_snapshot = Cursor::new(Vec::new());
        let current = v1_create(
            &mut Cursor::new(current_bytes),
            &mut current_snapshot,
            config,
        )
        .unwrap();
        let mut parent_snapshot = Cursor::new(Vec::new());
        let parent =
            v1_create(&mut Cursor::new(parent_bytes), &mut parent_snapshot, config).unwrap();
        let mut encoded_delta = Cursor::new(Vec::new());
        let task = ChunkTask::untracked();
        current
            .create_reverse_delta(
                &parent,
                &mut parent_snapshot,
                &mut encoded_delta,
                ChunkWriteOptions::V1,
                &task,
            )
            .unwrap();

        let delta = ChunkFileDelta::open(&mut encoded_delta).unwrap();
        let mut restored_snapshot = Cursor::new(Vec::new());
        let restored = delta
            .apply(&current, &mut current_snapshot, &mut restored_snapshot)
            .unwrap();
        let mut restored_bytes = Vec::new();
        restored
            .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
            .unwrap();

        assert_eq!(restored_bytes, parent_bytes);
    }

    #[test]
    fn compressed_delta_rejects_data_beyond_declared_payload() {
        let mut payload = Vec::new();
        write_delta_header(
            &mut payload,
            FormatVersion::V1,
            V2_CHUNKER_CDC,
            V2_ADAPTER_NONE,
            ChunkingConfig::default(),
            ChunkDigest::from([0; DIGEST_SIZE]),
            ChunkDigest::from([0; DIGEST_SIZE]),
            0,
            0,
            0,
        )
        .unwrap();
        payload.extend_from_slice(&[0_u8; 4096]);
        let encoded = zstd::stream::encode_all(payload.as_slice(), 1).unwrap();

        let error = ChunkFileDelta::open(&mut Cursor::new(encoded)).unwrap_err();

        assert!(matches!(error, ChunkFileError::InvalidFormat(_, _)));
        assert!(error.to_string().contains("超过格式声明"));
    }

    #[test]
    fn declared_limit_allows_multi_gigabyte_targets() {
        let target_size = 4 * 1024 * 1024 * 1024_u64;
        let header = DeltaHeader {
            version: FormatVersion::V2,
            target_config: ChunkingConfig::default(),
            base_digest: ChunkDigest::from([0; DIGEST_SIZE]),
            target_digest: ChunkDigest::from([0; DIGEST_SIZE]),
            target_size,
            target_chunk_count: 262_144,
            data_count: 262_144,
            target_chunker: V2_CHUNKER_CDC,
            target_adapter: V2_ADAPTER_NONE,
        };

        assert!(delta_payload_limit(&header).unwrap() > target_size);
    }

    #[test]
    fn large_delta_stress_round_trip_and_truncation_failure() {
        const SIZE: usize = 32 * 1024 * 1024;
        let config = ChunkingConfig::default();
        let mut state = 0x9e37_79b9_u32;
        let mut parent_bytes = vec![0_u8; SIZE];
        for byte in &mut parent_bytes {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        let mut current_bytes = parent_bytes.clone();
        for start in (0..SIZE).step_by(256 * 1024) {
            let end = (start + 4096).min(SIZE);
            for byte in &mut current_bytes[start..end] {
                *byte ^= 0x5a;
            }
        }

        let task = ChunkTask::untracked();
        let mut current_snapshot = Cursor::new(Vec::new());
        let current = v1_create(
            &mut Cursor::new(&current_bytes),
            &mut current_snapshot,
            config,
        )
        .unwrap();
        let mut parent_snapshot = Cursor::new(Vec::new());
        let parent = v1_create(
            &mut Cursor::new(&parent_bytes),
            &mut parent_snapshot,
            config,
        )
        .unwrap();
        let mut encoded_delta = Cursor::new(Vec::new());
        current
            .create_reverse_delta(
                &parent,
                &mut parent_snapshot,
                &mut encoded_delta,
                ChunkWriteOptions::V1,
                &task,
            )
            .unwrap();

        let delta = ChunkFileDelta::open(&mut encoded_delta).unwrap();
        let mut restored_snapshot = Cursor::new(Vec::new());
        let restored = delta
            .apply(&current, &mut current_snapshot, &mut restored_snapshot)
            .unwrap();
        let mut restored_bytes = Vec::with_capacity(SIZE);
        restored
            .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
            .unwrap();
        assert_eq!(restored_bytes, parent_bytes);

        let mut truncated = encoded_delta.into_inner();
        truncated.truncate(truncated.len().saturating_sub(17));
        assert!(ChunkFileDelta::open(&mut Cursor::new(truncated)).is_err());
    }

    fn materialized(layout: &ChunkFile, store: &mut ChunkStore<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, &mut bytes, &ChunkTask::untracked())
            .unwrap();
        bytes
    }

    /// 属性测试：随机版本链的任意跳恢复，都与该版本字节逐字节一致。
    ///
    /// 每个种子构造一条 3–6 版本的链，每版本对上一版本做若干随机字节改动，逐跳
    /// 生成反向增量，再从最新版本出发一路回退到最老版本，断言每一跳都精确复现
    /// 对应版本。它覆盖的是「随机改动位置 × 随机链深」的组合，而不是靠手写固定
    /// 场景去猜边界；失败时把种子带进消息即可复现。
    #[test]
    fn randomized_reverse_delta_chains_restore_every_revision() {
        let task = ChunkTask::untracked();
        for seed in 1..=16_u64 {
            let mut rng = Rng::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let revision_count = 3 + rng.below(4);
            let base_length = 32 * 1024 + rng.below(96 * 1024);

            let mut first = vec![0_u8; base_length];
            for byte in &mut first {
                *byte = rng.next_u64() as u8;
            }
            let mut versions = vec![first];
            for _ in 1..revision_count {
                let mut next = versions.last().unwrap().clone();
                let length = next.len();
                for _ in 0..(1 + rng.below(4)) {
                    let start = rng.below(length);
                    let span = 1 + rng.below((length - start).min(8 * 1024));
                    let mask = rng.nonzero_byte();
                    for byte in &mut next[start..start + span] {
                        *byte ^= mask;
                    }
                }
                versions.push(next);
            }

            let snapshots: Vec<Vec<u8>> = versions
                .iter()
                .map(|version| encode_snapshot(version))
                .collect();
            let deltas: Vec<Vec<u8>> = (0..revision_count - 1)
                .map(|index| encode_reverse_delta(&versions[index + 1], &versions[index]))
                .collect();

            let newest = revision_count - 1;
            let mut store = ChunkStore::new();
            store
                .append(Box::new(Cursor::new(snapshots[newest].clone())))
                .unwrap();
            let mut layout = ChunkFile::open(&mut Cursor::new(snapshots[newest].clone())).unwrap();
            assert_eq!(
                materialized(&layout, &mut store),
                versions[newest],
                "seed={seed} 最新版本不一致"
            );

            for index in (0..newest).rev() {
                let delta = ChunkFileDelta::open(&mut Cursor::new(deltas[index].clone())).unwrap();
                layout = delta.resolve(&layout, &mut store, &task).unwrap();
                assert_eq!(
                    materialized(&layout, &mut store),
                    versions[index],
                    "seed={seed} 回退到版本 {index} 时字节不一致"
                );
            }
        }
    }

    /// 磁盘写满时创建快照必须干净失败：返回真实的「空间不足」错误、不 panic，
    /// 且写到一半的半成品无法被当成完整快照打开。
    #[test]
    fn snapshot_creation_reports_a_full_disk_cleanly() {
        let source_bytes = vec![b'A'; 96 * 1024];
        let quota = 16 * 1024;
        let mut writer = QuotaWriter::new(Cursor::new(Vec::new()), quota);
        let task = ChunkTask::untracked();

        let error = ChunkFile::create(
            &mut Cursor::new(&source_bytes),
            &mut writer,
            ChunkingConfig::default(),
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap_err();

        let ChunkFileError::Io(io_error) = &error else {
            panic!("预期 I/O 错误，实际为 {error}");
        };
        assert_eq!(io_error.kind(), io::ErrorKind::StorageFull, "{io_error}");
        let written = writer.written();
        assert!(written > 0 && written <= quota, "written={written}");
        let partial = writer.into_inner().into_inner();
        assert_eq!(partial.len() as u64, written);
        assert!(ChunkFile::open(&mut Cursor::new(partial)).is_err());
    }

    /// 物化快照（恢复路径的落盘）遇到磁盘写满时同样干净失败。
    #[test]
    fn snapshot_materialization_reports_a_full_disk_cleanly() {
        let source_bytes = vec![b'B'; 128 * 1024];
        let encoded = encode_snapshot(&source_bytes);
        let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
        let quota = 8 * 1024;
        let mut writer = QuotaWriter::new(Cursor::new(Vec::new()), quota);
        let task = ChunkTask::untracked();

        let error = layout
            .write_snapshot(
                &mut Cursor::new(encoded),
                &mut writer,
                ChunkWriteOptions::V1,
                &task,
            )
            .unwrap_err();

        let ChunkFileError::Io(io_error) = &error else {
            panic!("预期 I/O 错误，实际为 {error}");
        };
        assert_eq!(io_error.kind(), io::ErrorKind::StorageFull, "{io_error}");
        let written = writer.written();
        assert!(written > 0 && written <= quota, "written={written}");
    }

    /// 畸形输入模糊（快照容器）：任意字节都必须被 `open` 干净地拒绝或正确地接受，
    /// **绝不 panic、绝不因为头部声明的巨大数量而无界分配**。
    #[test]
    fn fuzzed_snapshot_containers_are_rejected_without_panicking() {
        const MINIMUM_RECORD_BYTES: usize = DIGEST_SIZE + size_of::<u32>();
        let task = ChunkTask::untracked();
        for (label, valid) in [
            ("v1", encode_snapshot(&random_bytes(0xF0_0D, 96 * 1024))),
            ("v2", encode_v2_snapshot(&random_bytes(0xF0_0D, 96 * 1024))),
        ] {
            let mut accepted = 0_usize;
            for seed in 0..384_u64 {
                let bytes = match seed % 3 {
                    0 => random_bytes(seed, 1 + (seed as usize * 37) % 4096),
                    1 => mutated(seed, &valid, 1 + (seed as usize % 24)),
                    _ => mutated_header(seed, &valid, 64, 1 + (seed as usize % 12)),
                };
                let Ok(layout) = ChunkFile::open(&mut Cursor::new(bytes.clone())) else {
                    continue;
                };
                accepted += 1;
                assert!(
                    layout.chunk_count().saturating_mul(MINIMUM_RECORD_BYTES) <= bytes.len(),
                    "{label} seed={seed}：接受了超出文件容量的块数量"
                );
                let mut materialized = Cursor::new(Vec::new());
                if let Ok(restored) = layout.write_snapshot(
                    &mut Cursor::new(bytes.clone()),
                    &mut materialized,
                    ChunkWriteOptions::V1,
                    &task,
                ) {
                    assert_eq!(
                        materialized.into_inner().len() as u64,
                        layout.logical_size(),
                        "{label} seed={seed}：物化产物长度必须等于声明的大小"
                    );
                    assert_eq!(restored.logical_size(), layout.logical_size());
                }
            }
            assert!(
                accepted > 0,
                "{label} 固定种子下没有任何一次扰动保持在结构之内"
            );
            assert!(valid.len() > 256);
            for drop in [1_usize, 9, 64, valid.len() / 2, valid.len() - 1] {
                assert!(
                    ChunkFile::open(&mut Cursor::new(valid[..valid.len() - drop].to_vec()))
                        .is_err(),
                    "{label} 截断 {drop} 字节的快照必须被拒绝"
                );
            }
            let mut extended = valid.clone();
            extended.push(0);
            assert!(
                ChunkFile::open(&mut Cursor::new(extended)).is_err(),
                "{label} 续写字节的快照必须被拒绝"
            );
        }
    }

    /// 畸形输入模糊（反向增量容器）：任意字节都必须被干净地拒绝或接受，绝不 panic；
    /// 并额外断言截断与续写这两条确定的拒绝性质。
    #[test]
    fn fuzzed_delta_containers_are_rejected_without_panicking() {
        let child = random_bytes(0xC1, 96 * 1024);
        let parent = random_bytes(0xB2, 96 * 1024);
        let valid = encode_reverse_delta(&child, &parent);
        assert!(valid.len() > 256);
        assert!(
            ChunkFileDelta::open(&mut Cursor::new(valid.clone())).is_ok(),
            "合法反向增量必须能打开"
        );

        for seed in 0..384_u64 {
            let bytes = match seed % 2 {
                0 => random_bytes(seed, 1 + (seed as usize * 53) % 4096),
                _ => mutated(seed, &valid, 1 + (seed as usize % 24)),
            };
            let _ = ChunkFileDelta::open(&mut Cursor::new(bytes));
        }

        for drop in [1_usize, 7, 64, valid.len() / 2, valid.len() - 1] {
            assert!(
                ChunkFileDelta::open(&mut Cursor::new(valid[..valid.len() - drop].to_vec()))
                    .is_err(),
                "截断 {drop} 字节的增量必须被拒绝"
            );
        }
        let mut extended = valid.clone();
        extended.extend_from_slice(b"trailing");
        assert!(
            ChunkFileDelta::open(&mut Cursor::new(extended)).is_err(),
            "续写字节的增量必须被拒绝"
        );
    }

    fn synthetic_clip() -> Vec<u8> {
        fn block(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut output = Vec::new();
            output.extend_from_slice(&[0, 0, 0, 0]);
            output.extend_from_slice(tag);
            output.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            output.extend_from_slice(payload);
            output
        }
        fn utf16be(name: &str) -> Vec<u8> {
            let mut output = Vec::new();
            for byte in name.bytes() {
                output.push(0);
                output.push(byte);
            }
            output
        }
        fn tile(body: &[u8]) -> Vec<u8> {
            let begin = utf16be("BlockDataBeginChunk");
            let end = utf16be("BlockDataEndChunk");
            let size = 4 + 4 + begin.len() + body.len() + 4 + end.len();
            let mut output = Vec::new();
            output.extend_from_slice(&(size as u32).to_be_bytes());
            output.extend_from_slice(&(19_u32).to_be_bytes());
            output.extend_from_slice(&begin);
            output.extend_from_slice(body);
            output.extend_from_slice(&(17_u32).to_be_bytes());
            output.extend_from_slice(&end);
            output
        }
        let mut exta_payload = Vec::new();
        exta_payload.extend_from_slice(&40_u64.to_be_bytes());
        exta_payload.extend_from_slice(b"extrnlid");
        exta_payload.extend_from_slice(b"0123456789ABCDEF0123456789ABCDEF");
        exta_payload.extend_from_slice(&0_u32.to_be_bytes());
        exta_payload.extend_from_slice(&0_u32.to_be_bytes());
        exta_payload.extend_from_slice(&tile(&[1_u8; 4096]));
        exta_payload.extend_from_slice(&tile(&[2_u8; 4096]));
        exta_payload.extend_from_slice(&[0xAA; 12]);

        let mut body = Vec::new();
        body.extend_from_slice(&block(b"Head", &[0_u8; 8]));
        body.extend_from_slice(&block(b"Exta", &exta_payload));
        body.extend_from_slice(&block(b"Foot", &[]));
        let mut output = Vec::new();
        output.extend_from_slice(b"CSFCHUNK");
        output.extend_from_slice(&((24 + body.len()) as u64).to_be_bytes());
        output.extend_from_slice(&24_u64.to_be_bytes());
        output.extend_from_slice(&body);
        output
    }
}
