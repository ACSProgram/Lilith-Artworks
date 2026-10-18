//! Snapshot creation, opening and materialization.
//!
//! Creating a snapshot picks the chunk layout (container-aligned units when the
//! file is recognised, CDC otherwise), writes 44 B/36 B records and deduplicates
//! repeated v2 payloads. Opening and materializing reverse the layout without
//! trusting any declared length.

use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom, Write},
};

use sha2::{Digest, Sha256};

use super::{
    chunking::{plan_container_chunks, stream_cdc},
    format::{
        finalize_sha256, read_chunk_key, read_snapshot_header, read_v2_record_header,
        require_empty_output, sha256, stream_length, u64_to_usize, usize_to_u64,
        validate_record_length, validate_v2_codec, write_chunk_key, write_delta_header,
        write_snapshot_header, write_u8, write_v2_record_header,
    },
    integrity, invalid_format, ChunkDigest, ChunkFileError, ChunkFileResult, ChunkKey, ChunkRecord,
    ChunkTask, ChunkWriteOptions, ChunkingConfig, ChunkingStrategy, ErrorDimension, FormatVersion,
    CANCEL_CHECK_INTERVAL, CODEC_NONE, DIGEST_SIZE, RECIPE_NONE, V2_ADAPTER_CLIP, V2_ADAPTER_NONE,
    V2_CHUNKER_CDC, V2_CHUNKER_CLIP, V2_RECORD_FLAG_STRUCTURAL,
};

use super::delta::{DeltaOp, DeltaOpKind};

#[derive(Debug, Clone)]
pub(crate) struct ChunkFile {
    config: ChunkingConfig,
    logical_size: u64,
    file_digest: ChunkDigest,
    chunks: Vec<ChunkRecord>,
    chunk_index: HashMap<ChunkKey, usize>,
    /// Canonical v2 ids (0=CDC, 1=CLIP, 2=PSD); v1 layouts use 0.
    pub(super) chunker: u16,
    pub(super) adapter: u16,
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
}

impl ChunkFile {
    /// Chunk records in file order; the index of every record equals its
    /// position in the logical file.
    pub(super) fn chunk_records(&self) -> &[ChunkRecord] {
        &self.chunks
    }

    /// Position of the first record holding `key`, if the layout contains it.
    pub(super) fn chunk_position(&self, key: &ChunkKey) -> Option<usize> {
        self.chunk_index.get(key).copied()
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

    pub(super) fn from_records(
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

pub(super) fn load_verified_chunk<R: Read + Seek + ?Sized>(
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
