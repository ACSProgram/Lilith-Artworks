//! Append-only virtual address space over chunk payload sources.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
};

use super::{invalid_format, ChunkFileResult, ErrorDimension};

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
