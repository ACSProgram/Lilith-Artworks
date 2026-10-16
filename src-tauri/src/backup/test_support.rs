//! 单元测试共享支撑：确定性伪随机源与磁盘写满注入。
//!
//! 只在 `cfg(test)` 下编译，不进任何产物。按 `docs/guides/validation.md` 的
//! 「不要复制夹具」原则集中在此，供 `chunk_file` 与 `worker` 的测试复用。
#![allow(dead_code)]

use std::io::{self, Seek, SeekFrom, Write};

/// 确定性 xorshift 伪随机源。
///
/// 属性测试用固定种子复现：失败时把种子带进断言消息即可重放，不引入新依赖，
/// 也不受外部 RNG 实现变化影响。实现放在 `crate::fuzz_support`，与各解析器的
/// 畸形输入模糊共用一个伪随机源，这里只是转出，避免出现两份。
pub(crate) use crate::fuzz_support::Rng;

/// 平台无关的「磁盘空间不足」错误。
///
/// 用 `ErrorKind::StorageFull` 而不是硬编码平台错误码：它正是 `std` 对 ENOSPC
/// 的归类，注入点与真实磁盘满在调用方观察到的是同一种错误。
pub(crate) fn disk_full() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "磁盘空间不足")
}

/// 模拟磁盘写满的写入器：允许前 `quota` 字节真实落盘，之后所有写入返回
/// [`disk_full`]。
///
/// 语义与真实 ENOSPC 一致——写入是流式的，磁盘在某个字节位置耗尽，其后的写入
/// 失败，而此前的字节已经留在文件里。因此被测代码会真实遇到「写到一半失败」，
/// 而不是「一次性拿到一个假错误」；配额内的字节确实写进了底层文件（测试据此
/// 断言「半成品」的真实形态）。`seek` 不受配额限制：真实磁盘满同样不影响文件
/// 内已有的偏移。
pub(crate) struct QuotaWriter<W> {
    inner: W,
    remaining: u64,
    written: u64,
}

impl<W> QuotaWriter<W> {
    pub(crate) fn new(inner: W, quota: u64) -> Self {
        Self {
            inner,
            remaining: quota,
            written: 0,
        }
    }

    /// 已成功写入并落盘的字节数。
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// 取出底层写入器。
    pub(crate) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for QuotaWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(disk_full());
        }
        let allowed = buffer
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        self.inner.write_all(&buffer[..allowed])?;
        self.remaining -= allowed as u64;
        self.written += allowed as u64;
        Ok(allowed)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Seek> Seek for QuotaWriter<W> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}
