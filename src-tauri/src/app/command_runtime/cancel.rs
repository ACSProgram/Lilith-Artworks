use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::backup::BackupState;

/// 0.3 的统一取消令牌。阶段 1 先把它接到现有共享取消标志与无头闸门上；
/// 阶段 4 的所有 I/O 步骤直接消费同一类型，阶段 5 再把它扩展为每任务独立令牌。
///
/// 结构上刻意只暴露「是否已取消」，不暴露底层标志：调用方不需要知道取消来自
/// `BackupState`、无头闸门还是未来的 per-operation token。
#[derive(Clone)]
pub(crate) struct CancelToken {
    local: Arc<AtomicBool>,
    source: Arc<dyn Fn() -> bool + Send + Sync>,
}

#[allow(dead_code)]
impl CancelToken {
    /// 不连接任何外部来源的令牌，供测试与尚未接入共享运行态的操作使用。
    pub(crate) fn standalone() -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            source: Arc::new(|| false),
        }
    }

    /// 观察现有 `BackupState` 的共享取消标志。GUI 命令在取得运行锁时该标志会被
    /// 清零，因此它只覆盖当前正在执行的任务。
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

    /// 令牌自身的取消位。阶段 5 的 per-operation 取消会走这里；当前共享运行态
    /// 仍由 `BackupState::request_cancel` 置位。
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
