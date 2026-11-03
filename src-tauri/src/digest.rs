//! 整文件摘要与首尾采样。
//!
//! 这里只放不依赖领域错误类型的纯算法。文件打开、取消检查与错误文案仍由调用方
//! 决定，因此同一份字节在不同模块里产生的摘要与采样完全一致，而错误映射保持原样。

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
};

use sha2::{Digest, Sha256};

/// 首尾采样的单侧宽度：首、尾各读 64 KiB。
pub(crate) const SAMPLE_BYTES: u64 = 64 * 1024;

/// 流式读取的栈缓冲。固定 64 KiB 而不是按文件大小开：Windows 的主线程栈只有
/// 1 MiB，大缓冲会让调用它的发布入口直接栈溢出。
const BUFFER_BYTES: usize = 64 * 1024;

#[cfg(test)]
pub(crate) const ZERO_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// 摘要读取失败：调用方把 `Io` 映射成自己的上下文文案，把 `Cancelled` 映射成
/// 自己的取消语义。
#[derive(Debug)]
pub(crate) enum DigestError {
    Io(io::Error),
    Cancelled,
}

/// 流式读取并返回大写十六进制 SHA-256。
///
/// `cancelled` 在每次读取前调用；不参与取消的调用方传 `|| false`。
pub(crate) fn sha256_reader(
    mut reader: impl Read,
    mut cancelled: impl FnMut() -> bool,
) -> Result<String, DigestError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; BUFFER_BYTES];
    loop {
        if cancelled() {
            return Err(DigestError::Cancelled);
        }
        let read = reader.read(&mut buffer).map_err(DigestError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

/// 没有取消通道的整文件摘要：只有 I/O 失败，没有 [`DigestError::Cancelled`]。
///
/// 供本来就不支持取消的调用方使用，免得它们在 `match` 里各写一个永不发生的分支。
pub(crate) fn sha256_reader_without_cancel(reader: impl Read) -> io::Result<String> {
    sha256_reader(reader, || false).map_err(|error| match error {
        DigestError::Io(error) => error,
        // 传 `|| false` 时不会返回 `Cancelled`。
        DigestError::Cancelled => unreachable!("未传入取消检查"),
    })
}

/// 一次顺序读取同时累计完整 SHA-256 与首尾各 64 KiB 的抽样输入。
pub(crate) struct SamplingDigest {
    full: Sha256,
    head: Vec<u8>,
    tail: Vec<u8>,
}

impl SamplingDigest {
    pub(crate) fn new() -> Self {
        Self {
            full: Sha256::new(),
            head: Vec::with_capacity(SAMPLE_BYTES as usize),
            tail: Vec::with_capacity(SAMPLE_BYTES as usize * 2),
        }
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.full.update(bytes);
        if self.head.len() < SAMPLE_BYTES as usize {
            let take = (SAMPLE_BYTES as usize - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > SAMPLE_BYTES as usize * 2 {
            let excess = self.tail.len() - SAMPLE_BYTES as usize;
            self.tail.drain(..excess);
        }
    }

    /// 返回 `(完整摘要, 抽样摘要)`。不超过一个采样宽度的文件整份作为采样；
    /// 更大的文件取首 64 KiB 与尾 64 KiB（两者允许重叠）。
    pub(crate) fn finish(mut self, bytes: u64) -> (String, String) {
        let tail_len = if bytes > SAMPLE_BYTES {
            SAMPLE_BYTES as usize
        } else {
            0
        };
        if self.tail.len() > tail_len {
            self.tail.drain(..self.tail.len() - tail_len);
        }
        let sample_sha256 = sample_sha256(&self.head, &self.tail);
        (hex::encode_upper(self.full.finalize()), sample_sha256)
    }
}

/// 读取文件首尾各至多 [`SAMPLE_BYTES`]；不超过一个采样宽度时尾部为空。
pub(crate) fn read_sample(file: &mut File, size: u64) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let head_len = size.min(SAMPLE_BYTES) as usize;
    let mut head = vec![0_u8; head_len];
    file.read_exact(&mut head)?;
    if size <= SAMPLE_BYTES {
        return Ok((head, Vec::new()));
    }
    file.seek(SeekFrom::Start(size - SAMPLE_BYTES))?;
    let mut tail = vec![0_u8; SAMPLE_BYTES as usize];
    file.read_exact(&mut tail)?;
    Ok((head, tail))
}

/// 首部紧接着尾部的抽样摘要。
pub(crate) fn sample_sha256(head: &[u8], tail: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(head);
    hasher.update(tail);
    hex::encode_upper(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_matches_the_streamed_accumulator() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.bin");
        let bytes: Vec<u8> = (0..SAMPLE_BYTES as usize * 3)
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&path, &bytes).unwrap();

        let mut accumulator = SamplingDigest::new();
        accumulator.update(&bytes);
        let (full, sampled) = accumulator.finish(bytes.len() as u64);

        let mut file = File::open(&path).unwrap();
        let (head, tail) = read_sample(&mut file, bytes.len() as u64).unwrap();

        assert_eq!(head.len(), SAMPLE_BYTES as usize);
        assert_eq!(tail.len(), SAMPLE_BYTES as usize);
        assert_eq!(sampled, sample_sha256(&head, &tail));
        assert_eq!(full, sha256_reader(&bytes[..], || false).unwrap());
    }
}
