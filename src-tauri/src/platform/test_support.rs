//! 测试专用的平台探针：给业务模块的单元测试提供「造出平台专属场景」的最小手段。
//!
//! 只在测试目标里编译（`#[cfg(test)]` 门控），产品代码不使用它。放在 `platform/`
//! 内是因为它必然要写 `cfg(unix)` / `cfg(windows)`，而平台分支的唯一入口就是这里
//! （`docs/guides/platform.md` 规则 1）。
//!
//! 造不出场景的平台返回 `ErrorKind::Unsupported`，由调用方跳过对应用例——与
//! 「能力缺失必须显式」的口径一致；不用静默假成功代替，也不让业务文件里长出
//! 平台分支。

use std::{fs::File, io, path::Path};

fn unsupported(feature: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{feature} is unavailable on this platform"),
    )
}

/// 造一个指向文件 `target` 的符号链接 `link`。
///
/// Windows 创建符号链接需要开发者模式或管理员权限，普通测试进程拿不到，因此该
/// 平台显式返回 `Unsupported`。
pub(crate) fn symlink_to_file(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(unsupported("file symlinks"))
    }
}

/// 以独占共享模式打开 `path`，让同一路径上的删除以共享冲突失败。
///
/// Unix 允许删除仍被打开的文件，造不出同样的失败，因此显式返回 `Unsupported`。
pub(crate) fn open_blocking_deletion(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // FILE_SHARE_NONE：不共享读、写、删除，删除该路径必然失败。
        const FILE_SHARE_NONE: u32 = 0;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_NONE)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(unsupported("exclusive open"))
    }
}
