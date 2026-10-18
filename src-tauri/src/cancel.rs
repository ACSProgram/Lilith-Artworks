use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::backup::BackupState;

/// 0.3 的统一取消令牌。阶段 4 的 ChunkFile I/O 直接消费它；阶段 5 起每个登记
/// 的任务各持一份独立令牌，`BackupState` 只按目标取消。放在 crate 根而不是命令
/// 运行时里，是为了让底层的分块/恢复算法不必依赖 `app` 层。
///
/// 结构上刻意只暴露「是否已取消」，不暴露底层标志：调用方不需要知道取消来自
/// `BackupState`、无头闸门还是令牌自己的取消位。
#[derive(Clone)]
pub(crate) struct CancelToken {
    local: Arc<AtomicBool>,
    source: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl CancelToken {
    /// 不连接任何外部来源的令牌，供测试与一次性重放使用。
    pub(crate) fn standalone() -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            source: Arc::new(|| false),
        }
    }

    /// 观察 `BackupState` 中串行任务的取消状态。命令取得运行锁后拥有自己的任务
    /// 令牌，`request_cancel` 置位后由这里读到；并行后台探针的让位取消不计入，
    /// 因此不会误伤刚进站的前台命令。
    pub(crate) fn for_backup(state: BackupState) -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            source: Arc::new(move || state.cancelled()),
        }
    }

    /// 任意外部判定函数，例如无头入口的闸门检查点。
    pub(crate) fn with_source(source: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            source: Arc::new(source),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.local.load(Ordering::SeqCst) || (self.source)()
    }

    /// 令牌自身的取消位：`BackupState` 的 `request_cancel` / `cancel_background`
    /// 通过它取消目标任务的独立令牌。
    pub(crate) fn cancel(&self) {
        self.local.store(true, Ordering::SeqCst);
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::standalone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_tokens_start_and_stay_cancellable() {
        let token = CancelToken::standalone();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn source_and_local_flag_are_both_observed() {
        let source = Arc::new(AtomicBool::new(false));
        let token = CancelToken::with_source({
            let source = source.clone();
            move || source.load(Ordering::SeqCst)
        });
        assert!(!token.is_cancelled());
        source.store(true, Ordering::SeqCst);
        assert!(token.is_cancelled());

        let other = CancelToken::with_source(|| false);
        other.cancel();
        assert!(other.is_cancelled());
    }
}
