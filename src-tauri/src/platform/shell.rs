//! 在系统文件管理器中打开目录或定位文件。
//!
//! 能力差异：Windows 用 `explorer.exe`（reveal 会**选中**该文件），macOS 用
//! `open -R`，Linux 用 `xdg-open` 只能打开父目录。错误文案跨平台逐字一致。

use std::{fs, path::Path, process::Command};

/// 在系统文件管理器中显示给定文件（Windows 上会选中该文件）。
///
/// 只负责「唤起文件管理器」这一步；路径为空、设备命名空间、文件是否存在等
/// 业务校验由调用方完成。
///
/// 三个分支覆盖本应用能构建的全部目标（Windows / macOS / 其它 Unix）；尾部的
/// `Ok(())` 在其它目标上等同静默成功，而这类目标会先在 Tauri 依赖处编译失败，
/// 因此不额外加「不支持」分支——真要为某个新平台补齐时，在这里加分支并更新
/// `docs/guides/platform.md` 的能力表。
pub(crate) fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let directory = path.parent().ok_or("文件路径无效")?;
        Command::new("xdg-open")
            .arg(directory)
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    Ok(())
}

/// 在系统文件管理器中打开目录；目录不存在时先创建。
pub(crate) fn open_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("无法创建{label}目录：{error}"))?;
    open_existing_directory(path, label)
}

/// 在系统文件管理器中打开**已存在**的目录。
pub(crate) fn open_existing_directory(path: &Path, label: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        Command::new("explorer.exe")
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(path)
        .spawn()
        .map_err(|error| format!("无法打开{label}目录：{error}"))?;
    Ok(())
}
