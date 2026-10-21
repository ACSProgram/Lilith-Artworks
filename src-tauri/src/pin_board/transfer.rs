//! 素材板图片传输（导入/导出图片）的取消信号。
//!
//! 与整画布导出不同：那条例程由前端逐块推进，取消标记放在渲染器里；这两条是原生侧
//! 的长循环（一张张解码/编码图片），因此标记必须在原生侧，由循环在条目边界检查。
//!
//! 前端一次只跑一个传输，所以一个原子布尔就够：每个可取消的命令开始时复位，取消命令
//! 只置位。读写都用 `Relaxed`——它只用来决定「还要不要做下一张」，最坏情况是多做一张
//! 图（单张解码时间有界），不需要为取消再引一把锁。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 取消命中时的错误文案。前端据自己的取消标记改写成「已取消导入/导出」，这里只保证
/// 万一没人接住时也读得懂。
pub(crate) const TRANSFER_CANCELLED: &str = "已取消";

/// 传输取消信号：命令持有 `State`，把 `flag()` 的句柄搬进 `spawn` 的闭包。
#[derive(Clone, Default)]
pub(crate) struct PinBoardTransferCancel {
    requested: Arc<AtomicBool>,
}

impl PinBoardTransferCancel {
    /// 新传输开始前复位：上一次取消不该顺手掐掉这一次。
    pub(crate) fn reset(&self) {
        self.requested.store(false, Ordering::Relaxed);
    }

    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::Relaxed);
    }

    /// `'static` 的检查句柄，可以直接搬进后台闭包。
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.requested)
    }
}

/// 检查句柄的读取封装，避免每处调用都写一遍 `load(Ordering::Relaxed)`。
pub(crate) fn is_cancelled(flag: &Arc<AtomicBool>) -> bool {
    flag.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_request_flip_the_shared_flag() {
        let state = PinBoardTransferCancel::default();
        let flag = state.flag();
        assert!(!is_cancelled(&flag));

        state.request();
        assert!(is_cancelled(&flag), "取消句柄必须与状态共享同一个原子量");

        state.reset();
        assert!(!is_cancelled(&flag), "新传输开始时必须复位");
    }
}
