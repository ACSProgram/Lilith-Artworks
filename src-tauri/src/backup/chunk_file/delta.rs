//! Reverse deltas: creation, payload spooling and chain resolution.
//!
//! A delta transforms the current child snapshot back into its parent, so
//! resolving a chain walks parents without rewriting payload bytes: each
//! decompressed payload becomes one segment of a [`ChunkStore`](super::ChunkStore).

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

#[cfg(test)]
use tempfile::tempfile;
use tempfile::tempfile_in;

use super::{
    format::{
        read_chunk_key, read_delta_header, read_u8, read_v2_record_header, u64_to_usize,
        validate_record_length, validate_v2_codec, DeltaHeader,
    },
    incompatible, integrity, invalid_format, ChunkDigest, ChunkFileError, ChunkFileResult,
    ChunkKey, ChunkRecord, ChunkStore, ChunkTask, ChunkingConfig, ErrorDimension, FormatVersion,
    CANCEL_CHECK_INTERVAL, DELTA_OP_SIZE, STREAM_BUFFER_SIZE,
};

use super::snapshot::ChunkFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum DeltaOpKind {
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
pub(super) struct DeltaOp {
    pub(super) kind: DeltaOpKind,
    pub(super) key: ChunkKey,
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
        if base.file_digest() != self.base_digest {
            return Err(incompatible(
                ErrorDimension::File,
                format!(
                    "delta 基础版本为 {}，实际传入 {}",
                    self.base_digest,
                    base.file_digest()
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
                    let index = base.chunk_position(&op.key).ok_or_else(|| {
                        incompatible(
                            ErrorDimension::Chunk,
                            format!("基础版本缺少 COPY 块：{}", op.key.digest),
                        )
                    })?;
                    base.chunk_records()[index]
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
        resolved.write_snapshot(
            &mut reader,
            target_snapshot,
            super::ChunkWriteOptions::V1,
            &task,
        )
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

pub(super) fn delta_payload_limit(header: &DeltaHeader) -> ChunkFileResult<u64> {
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
