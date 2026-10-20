//! ChunkFile v1/v2 read and write layouts plus the delta chain machinery.
//!
//! The module only knows bytes, digests and offsets: it never touches SQLite,
//! cleanup bookkeeping or Tauri. Layout on disk:
//!
//! - [`format`]: snapshot/delta headers, chunk records and the length rules.
//! - [`store`]: the virtual-offset address space over snapshot/delta payloads.
//! - [`chunking`]: how bytes are cut (CDC and container-aligned units).
//! - [`snapshot`]: writing, opening and materializing a snapshot.
//! - [`delta`]: reverse deltas, payload spooling and chain resolution.
//!
//! v1 stays readable forever; v2 adds per-record lengths/codec and an adapter
//! id so structural chunking and future codecs can coexist in one repository.
//! Errors separate what to do (`InvalidFormat` / `Integrity` / `Incompatible`)
//! from where it happened ([`ErrorDimension`]).

use std::{
    cell::Cell,
    error::Error,
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    mem::size_of,
    path::Path,
};

use crate::cancel::CancelToken;

mod chunking;
mod delta;
mod format;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use self::{delta::ChunkFileDelta, snapshot::ChunkFile, store::ChunkStore};

pub(super) const SNAPSHOT_MAGIC: [u8; 8] = *b"LBCHUNK\0";
pub(super) const DELTA_MAGIC: [u8; 8] = *b"LBDELTA\0";
/// delta 文件整体作为一条 zstd 流落盘；磁盘字节以该帧魔数开头。
pub(super) const ZSTD_FRAME_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
pub(super) const FORMAT_VERSION_V1: u16 = 1;
pub(super) const FORMAT_VERSION_V2: u16 = 2;
/// v1 头里的 chunker 字段固定为滚动哈希实现编号。
pub(super) const LEGACY_CHUNKER_SHIFT_ADD: u16 = 1;
/// v2 头里的 chunker 编号：0=CDC，1=CLIP 结构，2=PSD 结构。
pub(super) const V2_CHUNKER_CDC: u16 = 0;
pub(super) const V2_CHUNKER_CLIP: u16 = 1;
pub(super) const V2_CHUNKER_PSD: u16 = 2;
pub(super) const V2_ADAPTER_NONE: u16 = 0;
pub(super) const V2_ADAPTER_CLIP: u16 = 1;
pub(super) const V2_ADAPTER_PSD: u16 = 2;
pub(super) const HASH_SHA256: u16 = 1;
pub(super) const V2_FLAGS_CHUNK_RECORDS: u16 = 1;
/// v2 块记录 flags bit0：该块与容器结构单元边界对齐。
pub(super) const V2_RECORD_FLAG_STRUCTURAL: u16 = 1;
pub(super) const CODEC_NONE: u8 = 0;
pub(super) const RECIPE_NONE: u8 = 0xFF;
pub(super) const MAX_ALLOWED_CHUNK_SIZE: usize = 16 * 1024 * 1024;
pub(super) const STREAM_BUFFER_SIZE: usize = 128 * 1024;
pub(super) const DIGEST_SIZE: usize = 32;
#[cfg(test)]
pub(super) const V1_SNAPSHOT_HEADER_SIZE: usize = 76;
#[cfg(test)]
pub(super) const V2_SNAPSHOT_HEADER_SIZE: usize = 80;
pub(super) const V1_DELTA_HEADER_SIZE: usize = 116;
pub(super) const V2_DELTA_HEADER_SIZE: usize = 120;
pub(super) const V1_RECORD_HEADER_SIZE: usize = DIGEST_SIZE + size_of::<u32>();
pub(super) const V2_RECORD_HEADER_SIZE: usize =
    DIGEST_SIZE + size_of::<u32>() + size_of::<u32>() + 1 + 1 + size_of::<u16>();
pub(super) const DELTA_OP_SIZE: usize = size_of::<u8>() + DIGEST_SIZE + size_of::<u32>();
/// 取消检查的循环间隔。转码步的地板是每个块（默认最大 320 KiB 的 tile 或
/// 64 KiB 的 CDC 块），这里只用于遍历操作表等纯 CPU 循环。
pub(super) const CANCEL_CHECK_INTERVAL: usize = 1024;

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

pub(super) fn invalid_format(
    dimension: ErrorDimension,
    message: impl Into<String>,
) -> ChunkFileError {
    ChunkFileError::InvalidFormat(dimension, message.into())
}

pub(super) fn integrity(dimension: ErrorDimension, message: impl Into<String>) -> ChunkFileError {
    ChunkFileError::Integrity(dimension, message.into())
}

pub(super) fn incompatible(
    dimension: ErrorDimension,
    message: impl Into<String>,
) -> ChunkFileError {
    ChunkFileError::Incompatible(dimension, message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatVersion {
    V1,
    V2,
}

/// 读取 snapshot / delta 文件头中的格式版本，只消费文件前 10 个字节。
///
/// 节点详情只需要展示版本标签，不应为此打开块表或解压 delta 载荷。
/// delta 文件整体是一条 zstd 流，`LBDELTA\0` 头在流内：识别到 zstd 帧魔数后
/// 先流式解压出前 10 字节再判版本，snapshot 文件则按明文直接读取。
pub(crate) fn stored_format_label(path: &Path) -> ChunkFileResult<&'static str> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 10];
    read_stored_format_header(&mut file, &mut header)?;
    if header[..8] != SNAPSHOT_MAGIC && header[..8] != DELTA_MAGIC {
        return Err(invalid_format(ErrorDimension::File, "不是受管理的历史文件"));
    }
    match FormatVersion::from_number(u16::from_le_bytes([header[8], header[9]]))? {
        FormatVersion::V1 => Ok("v1"),
        FormatVersion::V2 => Ok("v2"),
    }
}

/// 把存储文件的前 10 个字节读入 `header`，对 zstd 包装的 delta 先做流式解压。
fn read_stored_format_header(file: &mut File, header: &mut [u8; 10]) -> ChunkFileResult<()> {
    let mut prefix = [0_u8; 4];
    file.read_exact(&mut prefix)?;
    if prefix != ZSTD_FRAME_MAGIC {
        header[..4].copy_from_slice(&prefix);
        file.read_exact(&mut header[4..])?;
        return Ok(());
    }
    file.seek(SeekFrom::Start(0))?;
    let decompress_error = |error: io::Error| {
        invalid_format(ErrorDimension::Format, format!("无法解压 delta：{error}"))
    };
    let mut decoder = zstd::stream::read::Decoder::new(&mut *file).map_err(decompress_error)?;
    decoder.read_exact(header).map_err(decompress_error)?;
    Ok(())
}

impl FormatVersion {
    pub(super) fn number(self) -> u16 {
        match self {
            Self::V1 => FORMAT_VERSION_V1,
            Self::V2 => FORMAT_VERSION_V2,
        }
    }

    pub(super) fn from_number(value: u16) -> ChunkFileResult<Self> {
        match value {
            FORMAT_VERSION_V1 => Ok(Self::V1),
            FORMAT_VERSION_V2 => Ok(Self::V2),
            other => Err(incompatible(
                ErrorDimension::Format,
                format!("不支持格式版本 {other}"),
            )),
        }
    }

    pub(super) fn delta_header_size(self) -> usize {
        match self {
            Self::V1 => V1_DELTA_HEADER_SIZE,
            Self::V2 => V2_DELTA_HEADER_SIZE,
        }
    }

    pub(super) fn record_header_size(self) -> usize {
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
/// receives the **cumulative** number of logical bytes processed in this
/// operation, so callers can feed it straight into a progress bar as `current`
/// without adding the deltas up themselves.
///
/// This task is polled inside the chunking hot loop (once per chunk / structural
/// unit), so it only performs the token's **silent** observation
/// ([`CancelToken::observed`]): reading whether cancellation has already been
/// decided, without writing an interlock marker or blocking. Explicit boundary
/// checks keep using [`CancelToken::is_cancelled`]. This keeps the number of
/// interactive checkpoints equal to the number of boundaries, independent of how
/// many chunks a file happens to produce.
pub(crate) struct ChunkTask<'a> {
    cancel: Option<&'a CancelToken>,
    progress: Option<&'a dyn Fn(u64)>,
    /// 累计已处理的逻辑字节。只在本任务内累加，热循环每块读改写一次
    /// `Cell` 即可，不需要额外锁。
    processed_bytes: Cell<u64>,
}

impl<'a> ChunkTask<'a> {
    #[cfg(test)]
    pub(crate) fn untracked() -> Self {
        Self {
            cancel: None,
            progress: None,
            processed_bytes: Cell::new(0),
        }
    }

    pub(crate) fn new(cancel: &'a CancelToken, progress: Option<&'a dyn Fn(u64)>) -> Self {
        Self {
            cancel: Some(cancel),
            progress,
            processed_bytes: Cell::new(0),
        }
    }

    pub(crate) fn cancel(cancel: &'a CancelToken) -> Self {
        Self::new(cancel, None)
    }

    /// 并行管道把取消观察交给各自的线程；它们只需要令牌，不共享带 `Cell`
    /// 的任务本体。
    pub(super) fn cancel_token(&self) -> Option<&'a CancelToken> {
        self.cancel
    }

    pub(super) fn check(&self) -> ChunkFileResult<()> {
        if self.is_cancelled() {
            Err(ChunkFileError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// 热循环里的取消查询：只走静默观察，不发交互检查点（见结构体注释）。
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|token| token.observed())
    }

    /// 上报本步处理的字节数；回调收到的是本任务至今累计处理的逻辑字节。
    ///
    /// 热点路径每块调用一次，累计值在任务内单调不减，只有需要跨步骤重置时
    /// （例如换了统计维度）才值得新建一个任务。并行管道只由保序写线程调用，
    /// 乱序完成的块不会提前推进进度。
    pub(super) fn report(&self, bytes: u64) {
        let Some(progress) = self.progress else {
            return;
        };
        let processed = self.processed_bytes.get().saturating_add(bytes);
        self.processed_bytes.set(processed);
        progress(processed);
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
pub(super) struct ChunkKey {
    pub(super) digest: ChunkDigest,
    pub(super) length: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ChunkRecord {
    pub(super) key: ChunkKey,
    pub(super) data_offset: u64,
    /// Bytes stored for this record. Zero marks a v2 dedup reference whose
    /// payload lives at `data_offset` under an earlier record with the same key.
    pub(super) stored_length: u32,
}
