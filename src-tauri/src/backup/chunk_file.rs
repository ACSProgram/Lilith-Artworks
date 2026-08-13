use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    mem::size_of,
};

use sha2::{Digest, Sha256};

const SNAPSHOT_MAGIC: [u8; 8] = *b"LBCHUNK\0";
const DELTA_MAGIC: [u8; 8] = *b"LBDELTA\0";
const FORMAT_VERSION: u16 = 1;
const CHUNKER_SHIFT_ADD_V1: u16 = 1;
const HASH_SHA256: u16 = 1;
const FORMAT_FLAGS: u16 = 0;
const MAX_ALLOWED_CHUNK_SIZE: usize = 16 * 1024 * 1024;
const STREAM_BUFFER_SIZE: usize = 128 * 1024;
const DIGEST_SIZE: usize = 32;

// Snapshot keeps chunk payloads immutable and addressable by offset. Delta is
// intentionally reverse: it transforms the current snapshot into its parent.
// Both formats use explicit little-endian fields and algorithm identifiers so
// later chunkers or compression layers can coexist with version 1 data.

pub(crate) type ChunkFileResult<T> = Result<T, ChunkFileError>;

#[derive(Debug)]
pub(crate) enum ChunkFileError {
    Io(io::Error),
    InvalidConfig(String),
    InvalidFormat(String),
    Integrity(String),
    Incompatible(String),
}

impl fmt::Display for ChunkFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O 错误：{error}"),
            Self::InvalidConfig(message) => write!(formatter, "分块配置无效：{message}"),
            Self::InvalidFormat(message) => write!(formatter, "分块文件格式无效：{message}"),
            Self::Integrity(message) => write!(formatter, "分块文件完整性校验失败：{message}"),
            Self::Incompatible(message) => write!(formatter, "分块文件不兼容：{message}"),
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
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkFile {
    config: ChunkingConfig,
    logical_size: u64,
    file_digest: ChunkDigest,
    chunks: Vec<ChunkRecord>,
    chunk_index: HashMap<ChunkKey, usize>,
}

impl ChunkFile {
    pub(crate) fn create<R, W>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
    ) -> ChunkFileResult<Self>
    where
        R: Read,
        W: Write + Seek,
    {
        let config = config.validate()?;
        require_empty_output(snapshot, "snapshot")?;
        write_snapshot_header(snapshot, config, 0, 0, ChunkDigest::from([0; DIGEST_SIZE]))?;

        let mut file_hasher = Sha256::new();
        let mut chunk_buffer = Vec::with_capacity(config.max_size);
        let mut read_buffer = vec![0_u8; STREAM_BUFFER_SIZE];
        let mut rolling = 0_u64;
        let mut logical_size = 0_u64;
        let mut chunks = Vec::new();

        loop {
            let read = source.read(&mut read_buffer)?;
            if read == 0 {
                break;
            }
            for &byte in &read_buffer[..read] {
                rolling = rolling.wrapping_shl(1).wrapping_add(u64::from(byte));
                chunk_buffer.push(byte);
                let length = chunk_buffer.len();
                let boundary =
                    length >= config.min_size && (rolling & (config.avg_size as u64 - 1)) == 0;
                if boundary || length >= config.max_size {
                    emit_chunk(
                        snapshot,
                        &chunk_buffer,
                        &mut file_hasher,
                        &mut logical_size,
                        &mut chunks,
                    )?;
                    chunk_buffer.clear();
                    rolling = 0;
                }
            }
        }

        if !chunk_buffer.is_empty() {
            emit_chunk(
                snapshot,
                &chunk_buffer,
                &mut file_hasher,
                &mut logical_size,
                &mut chunks,
            )?;
        }

        let file_digest = ChunkDigest::from(finalize_sha256(file_hasher));
        let end = snapshot.stream_position()?;
        snapshot.seek(SeekFrom::Start(0))?;
        write_snapshot_header(
            snapshot,
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
        let minimum_record_bytes = (DIGEST_SIZE + size_of::<u32>()) as u64;
        let possible_records = file_length
            .saturating_sub(snapshot.stream_position()?)
            .checked_div(minimum_record_bytes)
            .unwrap_or(0);
        if header.chunk_count > possible_records {
            return Err(ChunkFileError::InvalidFormat(
                "块数量超过文件能够容纳的范围".into(),
            ));
        }

        let mut chunks = Vec::with_capacity(chunk_count);
        let mut logical_size = 0_u64;
        for index in 0..chunk_count {
            let key = read_chunk_key(snapshot)?;
            validate_record_length(key.length, header.config, index + 1 == chunk_count)?;
            let data_offset = snapshot.stream_position()?;
            let data_end = data_offset
                .checked_add(u64::from(key.length))
                .ok_or_else(|| ChunkFileError::InvalidFormat("块偏移溢出".into()))?;
            if data_end > file_length {
                return Err(ChunkFileError::InvalidFormat(format!(
                    "第 {} 个块超出文件边界",
                    index + 1
                )));
            }
            snapshot.seek(SeekFrom::Start(data_end))?;
            logical_size = logical_size
                .checked_add(u64::from(key.length))
                .ok_or_else(|| ChunkFileError::InvalidFormat("逻辑文件大小溢出".into()))?;
            chunks.push(ChunkRecord { key, data_offset });
        }

        if snapshot.stream_position()? != file_length {
            return Err(ChunkFileError::InvalidFormat(
                "snapshot 末尾存在未识别数据".into(),
            ));
        }
        if logical_size != header.logical_size {
            return Err(ChunkFileError::InvalidFormat(format!(
                "逻辑大小不匹配：头部为 {}，块合计为 {logical_size}",
                header.logical_size
            )));
        }

        Ok(Self::from_records(
            header.config,
            logical_size,
            header.file_digest,
            chunks,
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
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write,
    {
        let mut file_hasher = Sha256::new();
        let mut copied = 0_u64;
        for record in &self.chunks {
            copy_verified_payload(snapshot, record, output, &mut file_hasher)?;
            copied = copied
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| ChunkFileError::Integrity("还原大小溢出".into()))?;
        }
        output.flush()?;
        let digest = ChunkDigest::from(finalize_sha256(file_hasher));
        if copied != self.logical_size {
            return Err(ChunkFileError::Integrity(format!(
                "还原大小不匹配：预期 {}，实际 {copied}",
                self.logical_size
            )));
        }
        if digest != self.file_digest {
            return Err(ChunkFileError::Integrity(format!(
                "整文件哈希不匹配：预期 {}，实际 {digest}",
                self.file_digest
            )));
        }
        Ok(())
    }

    pub(crate) fn create_reverse_delta<R, W>(
        &self,
        parent: &ChunkFile,
        parent_snapshot: &mut R,
        delta_output: &mut W,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        require_empty_output(delta_output, "delta")?;
        let mut data_keys = Vec::new();
        let mut seen_data = HashSet::new();
        let mut ops = Vec::with_capacity(parent.chunks.len());

        for record in &parent.chunks {
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

        let mut encoded = zstd::stream::write::Encoder::new(&mut *delta_output, 6)?;
        write_delta_header(
            &mut encoded,
            parent.config,
            self.file_digest,
            parent.file_digest,
            parent.logical_size,
            usize_to_u64(parent.chunks.len(), "目标块数量")?,
            usize_to_u64(data_keys.len(), "delta 数据块数量")?,
        )?;

        for key in &data_keys {
            let parent_index = parent.chunk_index.get(key).ok_or_else(|| {
                ChunkFileError::Integrity(format!("父版本缺少声明的块 {}", key.digest))
            })?;
            let record = parent.chunks[*parent_index];
            write_chunk_key(&mut encoded, record.key)?;
            copy_payload_and_verify(parent_snapshot, &record, &mut encoded)?;
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
        }
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
            _ => Err(ChunkFileError::InvalidFormat(format!(
                "未知 delta 操作类型：{value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DeltaOp {
    kind: DeltaOpKind,
    key: ChunkKey,
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkFileDelta {
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    data_records: Vec<ChunkRecord>,
    data_index: HashMap<ChunkKey, usize>,
    ops: Vec<DeltaOp>,
    // Delta records are parsed from the decompressed payload. Keeping that
    // payload lets restore read both the new zstd-wrapped format and legacy
    // uncompressed files through the same offset-based code path.
    payload: Vec<u8>,
}

impl ChunkFileDelta {
    pub(crate) fn open<R>(delta: &mut R) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
    {
        delta.seek(SeekFrom::Start(0))?;
        let mut encoded = Vec::new();
        delta.read_to_end(&mut encoded)?;
        let payload = if encoded.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            zstd::stream::decode_all(encoded.as_slice()).map_err(|error| {
                ChunkFileError::InvalidFormat(format!("无法解压 delta：{error}"))
            })?
        } else {
            encoded
        };
        let file_length = payload.len() as u64;
        let mut input = Cursor::new(payload.as_slice());
        let header = read_delta_header(&mut input)?;
        let data_count = u64_to_usize(header.data_count, "delta 数据块数量")?;
        let op_count = u64_to_usize(header.target_chunk_count, "delta 操作数量")?;
        if header.data_count > header.target_chunk_count {
            return Err(ChunkFileError::InvalidFormat(
                "delta 数据块数量大于目标块数量".into(),
            ));
        }

        let minimum_data_record = (DIGEST_SIZE + size_of::<u32>()) as u64;
        let remaining = file_length.saturating_sub(input.position());
        if header.data_count > remaining / minimum_data_record {
            return Err(ChunkFileError::InvalidFormat(
                "delta 数据块数量超过文件能够容纳的范围".into(),
            ));
        }

        let mut data_records = Vec::with_capacity(data_count);
        let mut seen_data = HashSet::with_capacity(data_count);
        for index in 0..data_count {
            let key = read_chunk_key(&mut input)?;
            validate_record_length(key.length, header.target_config, true)?;
            if !seen_data.insert(key) {
                return Err(ChunkFileError::InvalidFormat(format!(
                    "delta 包含重复数据块：{}",
                    key.digest
                )));
            }
            let data_offset = input.position();
            let data_end = data_offset
                .checked_add(u64::from(key.length))
                .ok_or_else(|| ChunkFileError::InvalidFormat("delta 块偏移溢出".into()))?;
            if data_end > file_length {
                return Err(ChunkFileError::InvalidFormat(format!(
                    "delta 第 {} 个数据块超出文件边界",
                    index + 1
                )));
            }
            input.set_position(data_end);
            data_records.push(ChunkRecord { key, data_offset });
        }

        let bytes_per_op = (size_of::<u8>() + DIGEST_SIZE + size_of::<u32>()) as u64;
        let op_bytes = header
            .target_chunk_count
            .checked_mul(bytes_per_op)
            .ok_or_else(|| ChunkFileError::InvalidFormat("delta 操作区大小溢出".into()))?;
        if input
            .position()
            .checked_add(op_bytes)
            .filter(|end| *end == file_length)
            .is_none()
        {
            return Err(ChunkFileError::InvalidFormat(
                "delta 操作区长度与文件大小不匹配".into(),
            ));
        }

        let data_keys: HashSet<_> = data_records.iter().map(|record| record.key).collect();
        let mut ops = Vec::with_capacity(op_count);
        let mut logical_size = 0_u64;
        for _ in 0..op_count {
            let kind = DeltaOpKind::from_byte(read_u8(&mut input)?)?;
            let key = read_chunk_key(&mut input)?;
            validate_record_length(key.length, header.target_config, true)?;
            if kind == DeltaOpKind::Data && !data_keys.contains(&key) {
                return Err(ChunkFileError::InvalidFormat(format!(
                    "DATA 操作缺少块内容：{}",
                    key.digest
                )));
            }
            logical_size = logical_size
                .checked_add(u64::from(key.length))
                .ok_or_else(|| ChunkFileError::InvalidFormat("delta 目标大小溢出".into()))?;
            ops.push(DeltaOp { kind, key });
        }
        if logical_size != header.target_size {
            return Err(ChunkFileError::InvalidFormat(format!(
                "delta 目标大小不匹配：头部为 {}，操作合计为 {logical_size}",
                header.target_size
            )));
        }

        Ok(Self::from_records(
            header.target_config,
            header.base_digest,
            header.target_digest,
            header.target_size,
            data_records,
            ops,
            payload,
        ))
    }

    pub(crate) fn apply<RB, W>(
        &self,
        base: &ChunkFile,
        base_snapshot: &mut RB,
        target_snapshot: &mut W,
    ) -> ChunkFileResult<ChunkFile>
    where
        RB: Read + Seek,
        W: Write + Seek,
    {
        if base.file_digest != self.base_digest {
            return Err(ChunkFileError::Incompatible(format!(
                "delta 基础版本为 {}，实际传入 {}",
                self.base_digest, base.file_digest
            )));
        }
        require_empty_output(target_snapshot, "目标 snapshot")?;
        write_snapshot_header(
            target_snapshot,
            self.target_config,
            self.target_size,
            usize_to_u64(self.ops.len(), "目标块数量")?,
            self.target_digest,
        )?;

        let mut file_hasher = Sha256::new();
        let mut logical_size = 0_u64;
        let mut chunks = Vec::with_capacity(self.ops.len());

        let mut delta_payload = Cursor::new(self.payload.as_slice());
        for op in &self.ops {
            let record = match op.kind {
                DeltaOpKind::Copy => {
                    let index = base.chunk_index.get(&op.key).ok_or_else(|| {
                        ChunkFileError::Incompatible(format!(
                            "基础版本缺少 COPY 块：{}",
                            op.key.digest
                        ))
                    })?;
                    let record = base.chunks[*index];
                    write_chunk_key(target_snapshot, record.key)?;
                    let data_offset = target_snapshot.stream_position()?;
                    copy_verified_payload(
                        base_snapshot,
                        &record,
                        target_snapshot,
                        &mut file_hasher,
                    )?;
                    ChunkRecord {
                        key: record.key,
                        data_offset,
                    }
                }
                DeltaOpKind::Data => {
                    let index = self.data_index.get(&op.key).ok_or_else(|| {
                        ChunkFileError::InvalidFormat(format!(
                            "delta 缺少 DATA 块：{}",
                            op.key.digest
                        ))
                    })?;
                    let record = self.data_records[*index];
                    write_chunk_key(target_snapshot, record.key)?;
                    let data_offset = target_snapshot.stream_position()?;
                    copy_verified_payload(
                        &mut delta_payload,
                        &record,
                        target_snapshot,
                        &mut file_hasher,
                    )?;
                    ChunkRecord {
                        key: record.key,
                        data_offset,
                    }
                }
            };
            logical_size = logical_size
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| ChunkFileError::Integrity("应用 delta 后大小溢出".into()))?;
            chunks.push(record);
        }
        target_snapshot.flush()?;

        if logical_size != self.target_size {
            return Err(ChunkFileError::Integrity(format!(
                "应用 delta 后大小不匹配：预期 {}，实际 {logical_size}",
                self.target_size
            )));
        }
        let file_digest = ChunkDigest::from(finalize_sha256(file_hasher));
        if file_digest != self.target_digest {
            return Err(ChunkFileError::Integrity(format!(
                "应用 delta 后整文件哈希不匹配：预期 {}，实际 {file_digest}",
                self.target_digest
            )));
        }

        Ok(ChunkFile::from_records(
            self.target_config,
            logical_size,
            file_digest,
            chunks,
        ))
    }

    fn from_records(
        target_config: ChunkingConfig,
        base_digest: ChunkDigest,
        target_digest: ChunkDigest,
        target_size: u64,
        data_records: Vec<ChunkRecord>,
        ops: Vec<DeltaOp>,
        payload: Vec<u8>,
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
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SnapshotHeader {
    config: ChunkingConfig,
    logical_size: u64,
    chunk_count: u64,
    file_digest: ChunkDigest,
}

#[derive(Debug, Clone, Copy)]
struct DeltaHeader {
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    target_chunk_count: u64,
    data_count: u64,
}

fn emit_chunk<W: Write + Seek>(
    snapshot: &mut W,
    data: &[u8],
    file_hasher: &mut Sha256,
    logical_size: &mut u64,
    chunks: &mut Vec<ChunkRecord>,
) -> ChunkFileResult<()> {
    let length = u32::try_from(data.len())
        .map_err(|_| ChunkFileError::InvalidConfig("单块大小超过 u32".into()))?;
    let digest = ChunkDigest::from(sha256(data));
    let key = ChunkKey { digest, length };
    write_chunk_key(snapshot, key)?;
    let data_offset = snapshot.stream_position()?;
    snapshot.write_all(data)?;
    file_hasher.update(data);
    *logical_size = logical_size
        .checked_add(length.into())
        .ok_or_else(|| ChunkFileError::InvalidConfig("逻辑文件大小溢出".into()))?;
    chunks.push(ChunkRecord { key, data_offset });
    Ok(())
}

fn copy_payload_and_verify<R, W>(
    source: &mut R,
    record: &ChunkRecord,
    output: &mut W,
) -> ChunkFileResult<()>
where
    R: Read + Seek,
    W: Write,
{
    source.seek(SeekFrom::Start(record.data_offset))?;
    let mut remaining = record.key.length as usize;
    let mut buffer = [0_u8; 64 * 1024];
    let mut chunk_hasher = Sha256::new();
    while remaining > 0 {
        let requested = remaining.min(buffer.len());
        source.read_exact(&mut buffer[..requested])?;
        output.write_all(&buffer[..requested])?;
        chunk_hasher.update(&buffer[..requested]);
        remaining -= requested;
    }
    let digest = ChunkDigest::from(finalize_sha256(chunk_hasher));
    if digest != record.key.digest {
        return Err(ChunkFileError::Integrity(format!(
            "块哈希不匹配：预期 {}，实际 {digest}",
            record.key.digest
        )));
    }
    Ok(())
}

fn copy_verified_payload<R, W>(
    source: &mut R,
    record: &ChunkRecord,
    output: &mut W,
    file_hasher: &mut Sha256,
) -> ChunkFileResult<()>
where
    R: Read + Seek + ?Sized,
    W: Write + ?Sized,
{
    source.seek(SeekFrom::Start(record.data_offset))?;
    let mut remaining = record.key.length as usize;
    let mut buffer = [0_u8; 64 * 1024];
    let mut chunk_hasher = Sha256::new();
    while remaining > 0 {
        let requested = remaining.min(buffer.len());
        source.read_exact(&mut buffer[..requested])?;
        output.write_all(&buffer[..requested])?;
        chunk_hasher.update(&buffer[..requested]);
        file_hasher.update(&buffer[..requested]);
        remaining -= requested;
    }
    let digest = ChunkDigest::from(finalize_sha256(chunk_hasher));
    if digest != record.key.digest {
        return Err(ChunkFileError::Integrity(format!(
            "块哈希不匹配：预期 {}，实际 {digest}",
            record.key.digest
        )));
    }
    Ok(())
}

fn write_snapshot_header<W: Write>(
    output: &mut W,
    config: ChunkingConfig,
    logical_size: u64,
    chunk_count: u64,
    file_digest: ChunkDigest,
) -> ChunkFileResult<()> {
    output.write_all(&SNAPSHOT_MAGIC)?;
    write_u16(output, FORMAT_VERSION)?;
    write_u16(output, FORMAT_FLAGS)?;
    write_u16(output, CHUNKER_SHIFT_ADD_V1)?;
    write_u16(output, HASH_SHA256)?;
    write_config(output, config)?;
    write_u64(output, logical_size)?;
    write_u64(output, chunk_count)?;
    output.write_all(file_digest.as_bytes())?;
    Ok(())
}

fn read_snapshot_header<R: Read>(input: &mut R) -> ChunkFileResult<SnapshotHeader> {
    expect_magic(input, SNAPSHOT_MAGIC, "snapshot")?;
    read_common_header(input)?;
    let config = read_config(input)?.validate()?;
    let logical_size = read_u64(input)?;
    let chunk_count = read_u64(input)?;
    let file_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    if logical_size == 0 && chunk_count != 0 {
        return Err(ChunkFileError::InvalidFormat("空文件不能包含数据块".into()));
    }
    if logical_size > 0 && chunk_count == 0 {
        return Err(ChunkFileError::InvalidFormat("非空文件缺少数据块".into()));
    }
    Ok(SnapshotHeader {
        config,
        logical_size,
        chunk_count,
        file_digest,
    })
}

fn write_delta_header<W: Write>(
    output: &mut W,
    target_config: ChunkingConfig,
    base_digest: ChunkDigest,
    target_digest: ChunkDigest,
    target_size: u64,
    target_chunk_count: u64,
    data_count: u64,
) -> ChunkFileResult<()> {
    output.write_all(&DELTA_MAGIC)?;
    write_u16(output, FORMAT_VERSION)?;
    write_u16(output, FORMAT_FLAGS)?;
    write_u16(output, CHUNKER_SHIFT_ADD_V1)?;
    write_u16(output, HASH_SHA256)?;
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
    read_common_header(input)?;
    let target_config = read_config(input)?.validate()?;
    let base_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let target_digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let target_size = read_u64(input)?;
    let target_chunk_count = read_u64(input)?;
    let data_count = read_u64(input)?;
    if target_size == 0 && target_chunk_count != 0 {
        return Err(ChunkFileError::InvalidFormat(
            "空目标不能包含 delta 操作".into(),
        ));
    }
    if target_size > 0 && target_chunk_count == 0 {
        return Err(ChunkFileError::InvalidFormat(
            "非空目标缺少 delta 操作".into(),
        ));
    }
    Ok(DeltaHeader {
        target_config,
        base_digest,
        target_digest,
        target_size,
        target_chunk_count,
        data_count,
    })
}

fn read_common_header<R: Read>(input: &mut R) -> ChunkFileResult<()> {
    let version = read_u16(input)?;
    if version != FORMAT_VERSION {
        return Err(ChunkFileError::InvalidFormat(format!(
            "不支持格式版本 {version}"
        )));
    }
    let flags = read_u16(input)?;
    if flags != FORMAT_FLAGS {
        return Err(ChunkFileError::InvalidFormat(format!(
            "不支持格式标志 {flags:#06x}"
        )));
    }
    let chunker = read_u16(input)?;
    if chunker != CHUNKER_SHIFT_ADD_V1 {
        return Err(ChunkFileError::InvalidFormat(format!(
            "不支持分块算法 {chunker}"
        )));
    }
    let hash = read_u16(input)?;
    if hash != HASH_SHA256 {
        return Err(ChunkFileError::InvalidFormat(format!(
            "不支持哈希算法 {hash}"
        )));
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

fn validate_record_length(
    length: u32,
    config: ChunkingConfig,
    may_be_final: bool,
) -> ChunkFileResult<()> {
    let length = length as usize;
    if length == 0 {
        return Err(ChunkFileError::InvalidFormat("数据块长度不能为 0".into()));
    }
    if length > config.max_size {
        return Err(ChunkFileError::InvalidFormat(format!(
            "数据块长度 {length} 超过最大块大小 {}",
            config.max_size
        )));
    }
    if !may_be_final && length < config.min_size {
        return Err(ChunkFileError::InvalidFormat(format!(
            "非末尾数据块长度 {length} 小于最小块大小 {}",
            config.min_size
        )));
    }
    Ok(())
}

fn require_empty_output<W: Seek>(output: &mut W, label: &str) -> ChunkFileResult<()> {
    let length = output.seek(SeekFrom::End(0))?;
    if length != 0 {
        return Err(ChunkFileError::InvalidFormat(format!(
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
        return Err(ChunkFileError::InvalidFormat(format!(
            "{label} magic 不匹配"
        )));
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
        .map_err(|_| ChunkFileError::InvalidFormat(format!("{label} 超过平台范围")))
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
