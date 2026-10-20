//! 读取系统剪贴板中的文件路径（「复制文件后粘贴」入口）。
//!
//! 能力差异：目前只有 Windows 实现了 `CF_HDROP` 读取；**非 Windows 平台显式
//! 返回空列表**（不是错误、也不假装成功）——新平台移植时按
//! `docs/guides/platform.md` 的能力披露表核对并补实现。
//!
//! 数量上限属于业务约束，不在本模块：调用方按自己的导入上限截断。

#[cfg(target_os = "windows")]
pub(crate) fn file_paths() -> Result<Vec<String>, String> {
    use std::ptr;
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::{
        System::DataExchange::{
            CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        },
        UI::Shell::DragQueryFileW,
    };

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return Err("无法打开系统剪贴板".into());
        }
        let _guard = ClipboardGuard;
        let format = u32::from(CF_HDROP);
        if IsClipboardFormatAvailable(format) == 0 {
            return Ok(Vec::new());
        }
        let drop = GetClipboardData(format);
        if drop.is_null() {
            return Err("无法读取剪贴板文件路径".into());
        }
        let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0);
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, ptr::null_mut(), 0);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0_u16; length as usize + 1];
            let written = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32);
            if written > 0 {
                paths.push(String::from_utf16_lossy(&buffer[..written as usize]));
            }
        }
        Ok(paths)
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn file_paths() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}
