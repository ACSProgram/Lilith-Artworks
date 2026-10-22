//! 认证流水线的文件准备与复制。
//!
//! 只负责「从源到目标」的文件步骤：路径规范化、带取消检查的摘要、同目录临时文件
//! 与不覆盖发布。发布语义（何时登记清理意图、何时允许覆盖）由调用方决定，本模块
//! 不读写数据库。

use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use super::{
    error::{AuthenticityError, AuthenticityResult},
    state::AuthenticityOperation,
};

/// 现有普通文件：先校验存在，再取规范化路径（用于仓库边界与同一文件判断）。
pub(crate) fn canonical_existing_file(value: &str, label: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput(format!(
            "{label}文件不存在"
        )));
    }
    Ok(path.canonicalize()?)
}

/// 输出目标路径：允许相对路径（以当前目录为基准），拒绝空路径。
pub(crate) fn absolute_output_path(value: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if path.as_os_str().is_empty() {
        return Err(AuthenticityError::InvalidInput("请选择输出路径".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(crate) fn sha256_file_cancelable(
    path: &Path,
    operation: &AuthenticityOperation,
) -> AuthenticityResult<String> {
    sha256_file_with_check(path, || operation.cancelled())
}

pub(crate) fn sha256_file_with_check(
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> AuthenticityResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(AuthenticityError::Task("认证任务已取消".into()));
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

/// 同目录临时文件 + `sync_all` + `persist_noclobber`，目标已存在时拒绝。
///
/// 目标目录必须已存在：调用方决定是否创建目录，因此「输出目录无效」的错误也由
/// 调用方的上下文决定。
pub(crate) fn copy_noclobber(source: &Path, destination: &Path) -> AuthenticityResult<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

/// 已写入同目录临时文件、并已算出摘要与字节数的待发布副本。
pub(crate) struct StagedCopy {
    temp: NamedTempFile,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

impl StagedCopy {
    pub(crate) fn persist_noclobber(self, destination: &Path) -> io::Result<()> {
        self.temp
            .persist_noclobber(destination)
            .map(|_| ())
            .map_err(|error| error.error)
    }
}

/// 流式复制到目标目录中的临时文件，同时计算 SHA-256 与字节数。
///
/// 把「复制 + 摘要」与「发布」拆开，是为了让调用方能在发布前登记带哈希的清理
/// 意图：进程在两步之间退出时，清理队列仍能回收孤儿文件。
pub(crate) fn stage_copy_with_sha256(
    source: &Path,
    directory: &Path,
) -> AuthenticityResult<StagedCopy> {
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        temp.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    temp.as_file().sync_all()?;
    Ok(StagedCopy {
        temp,
        sha256: hex::encode_upper(hasher.finalize()),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn copy_noclobber_never_overwrites_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        let destination = directory.path().join("destination.bin");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"original").unwrap();

        assert!(copy_noclobber(&source, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
    }

    #[test]
    fn staged_copy_reports_the_written_bytes_without_publishing() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        let destination = directory.path().join("destination.bin");
        fs::write(&source, b"payload").unwrap();

        let staged = stage_copy_with_sha256(&source, directory.path()).unwrap();

        assert_eq!(staged.bytes, 7);
        assert_eq!(staged.sha256.len(), 64);
        assert!(!destination.exists());
        staged.persist_noclobber(&destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"payload");
    }
}
