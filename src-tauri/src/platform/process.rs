//! 进程自身的内存信息。
//!
//! 能力差异：目前只有 Windows 实现了峰值工作集读取（`GetProcessMemoryInfo`）；
//! **无能力平台显式返回 `None`**，调用方据此写 null 而不是伪造数值。
//! 新平台移植时按 `docs/guides/platform.md` 的能力披露表核对并补实现。

/// 进程自身的峰值工作集（字节）；无能力平台返回 `None`。
///
/// 目前唯一的调用方是无头命令行的诊断输出（`headless` feature），因此在不启用
/// 该 feature 的产品构建里本函数无人调用——这是预期状态，不是死代码。
#[cfg_attr(not(feature = "headless"), allow(dead_code))]
#[cfg(target_os = "windows")]
pub(crate) fn peak_memory_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: 结构体按 C 布局分配并清零，长度与 API 期望的一致。
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let succeeded = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (succeeded != 0).then(|| u64::try_from(counters.PeakWorkingSetSize).unwrap_or(u64::MAX))
}

#[cfg_attr(not(feature = "headless"), allow(dead_code))]
#[cfg(not(target_os = "windows"))]
pub(crate) fn peak_memory_bytes() -> Option<u64> {
    None
}
