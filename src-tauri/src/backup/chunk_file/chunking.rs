//! How bytes are cut into chunk records.
//!
//! Two strategies share the writer: content-defined chunking (a rolling hash
//! over the stream) and container-aligned chunking, which reuses the structural
//! units `.clip` already has. Units above `max_size` are handed back to CDC, and
//! a container that fails to parse is not an error — CDC is the fallback, so a
//! parse failure can only cost alignment, never stored bytes.

use std::io::{Read, Seek, SeekFrom};

use crate::format::container::{self, ContainerError, StructuralUnit};

use super::{
    invalid_format, ChunkFileError, ChunkFileResult, ChunkTask, ChunkingConfig, ErrorDimension,
    STREAM_BUFFER_SIZE,
};

/// Returns the structural units of a recognised container, or `None` when the
/// head is not a supported format or its layout cannot be trusted.
pub(super) fn plan_container_chunks<R: Read + Seek>(
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

pub(super) fn stream_cdc<R, F>(
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
