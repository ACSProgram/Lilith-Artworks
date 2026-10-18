//! On-disk header, chunk-record and length validation for ChunkFile v1/v2.
//!
//! Every reader here works on untrusted bytes: a declared length is checked
//! against the bytes actually available before anything is allocated, and an
//! unknown version or capability is reported as `Incompatible` rather than as
//! corruption.

use std::io::{Read, Seek, SeekFrom, Write};

use sha2::{Digest, Sha256};

use super::{
    incompatible, invalid_format, ChunkDigest, ChunkFileError, ChunkFileResult, ChunkKey,
    ChunkingConfig, ErrorDimension, FormatVersion, CODEC_NONE, DELTA_MAGIC, DIGEST_SIZE,
    HASH_SHA256, LEGACY_CHUNKER_SHIFT_ADD, RECIPE_NONE, SNAPSHOT_MAGIC, V2_ADAPTER_NONE,
    V2_ADAPTER_PSD, V2_CHUNKER_CDC, V2_CHUNKER_PSD, V2_FLAGS_CHUNK_RECORDS,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct SnapshotHeader {
    pub(super) version: FormatVersion,
    pub(super) config: ChunkingConfig,
    pub(super) logical_size: u64,
    pub(super) chunk_count: u64,
    pub(super) file_digest: ChunkDigest,
    pub(super) chunker: u16,
    pub(super) adapter: u16,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DeltaHeader {
    pub(super) version: FormatVersion,
    pub(super) target_config: ChunkingConfig,
    pub(super) base_digest: ChunkDigest,
    pub(super) target_digest: ChunkDigest,
    pub(super) target_size: u64,
    pub(super) target_chunk_count: u64,
    pub(super) data_count: u64,
    pub(super) target_chunker: u16,
    pub(super) target_adapter: u16,
}

#[derive(Debug, Clone, Copy)]
struct CommonHeader {
    version: FormatVersion,
    chunker: u16,
    adapter: u16,
}

pub(super) fn write_snapshot_header<W: Write>(
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

pub(super) fn read_snapshot_header<R: Read>(input: &mut R) -> ChunkFileResult<SnapshotHeader> {
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

pub(super) fn write_delta_header<W: Write>(
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

pub(super) fn read_delta_header<R: Read>(input: &mut R) -> ChunkFileResult<DeltaHeader> {
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

pub(super) fn validate_v2_codec(codec: u8, recipe: u8, index: usize) -> ChunkFileResult<()> {
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

pub(super) fn write_chunk_key<W: Write>(output: &mut W, key: ChunkKey) -> ChunkFileResult<()> {
    output.write_all(key.digest.as_bytes())?;
    write_u32(output, key.length)?;
    Ok(())
}

pub(super) fn read_chunk_key<R: Read>(input: &mut R) -> ChunkFileResult<ChunkKey> {
    let digest = ChunkDigest::from(read_array::<DIGEST_SIZE, _>(input)?);
    let length = read_u32(input)?;
    Ok(ChunkKey { digest, length })
}

pub(super) fn write_v2_record_header<W: Write>(
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
pub(super) struct V2RecordHeader {
    pub(super) key: ChunkKey,
    pub(super) stored_len: u32,
    pub(super) logical_len: u32,
    pub(super) codec: u8,
    pub(super) recipe: u8,
}

pub(super) fn read_v2_record_header<R: Read>(input: &mut R) -> ChunkFileResult<V2RecordHeader> {
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
pub(super) fn validate_record_length(
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

pub(super) fn require_empty_output<W: Seek>(output: &mut W, label: &str) -> ChunkFileResult<()> {
    let length = output.seek(SeekFrom::End(0))?;
    if length != 0 {
        return Err(ChunkFileError::OutputConflict(format!(
            "{label} 输出必须是空文件"
        )));
    }
    output.seek(SeekFrom::Start(0))?;
    Ok(())
}

pub(super) fn stream_length<R: Seek>(input: &mut R) -> ChunkFileResult<u64> {
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

pub(super) fn sha256(data: &[u8]) -> [u8; DIGEST_SIZE] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    finalize_sha256(hasher)
}

pub(super) fn finalize_sha256(hasher: Sha256) -> [u8; DIGEST_SIZE] {
    hasher.finalize().into()
}

fn usize_to_u32(value: usize, label: &str) -> ChunkFileResult<u32> {
    u32::try_from(value)
        .map_err(|_| ChunkFileError::InvalidConfig(format!("{label} 超过 u32 范围")))
}

pub(super) fn usize_to_u64(value: usize, label: &str) -> ChunkFileResult<u64> {
    u64::try_from(value)
        .map_err(|_| ChunkFileError::InvalidConfig(format!("{label} 超过 u64 范围")))
}

pub(super) fn u64_to_usize(value: u64, label: &str) -> ChunkFileResult<usize> {
    usize::try_from(value)
        .map_err(|_| invalid_format(ErrorDimension::File, format!("{label} 超过平台范围")))
}

fn read_array<const N: usize, R: Read>(input: &mut R) -> ChunkFileResult<[u8; N]> {
    let mut bytes = [0_u8; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn write_u8<W: Write>(output: &mut W, value: u8) -> ChunkFileResult<()> {
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

pub(super) fn read_u8<R: Read>(input: &mut R) -> ChunkFileResult<u8> {
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
