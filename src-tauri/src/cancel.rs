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
///
/// **取消观察分两层**（见 `docs/guides/validation.md` 的「交互检查点只在边界」）：
///
/// - [`Self::is_cancelled`] 是**交互层**，只在显式边界调用，允许带副作用：无头入口的
///   闸门在这里写一行 marker，并在 `--cancel-on-stdin` 下阻塞等待一行判定。
/// - [`Self::observed`] 是**静默层**，供热循环高频调用，只读「取消是否已被判定」，
///   不写 marker、不阻塞。分块主循环用它，检查点总数因此回到边界数量、marker 文件
///   也不再随处理字节数膨胀。
///
/// 已判定的取消会被静默层在下一次查询时立刻读到（它读的是同一个已置位的判定结果），
/// 因此取消延迟不回退：只是在边界判定，之后热循环静默观察。
#[derive(Clone)]
pub(crate) struct CancelToken {
    local: Arc<AtomicBool>,
    /// 交互层来源：写 marker / 阻塞等判定，只在显式边界调用。
    interactive: Arc<dyn Fn() -> bool + Send + Sync>,
    /// 静默层来源：廉价、无副作用的取消状态读取。
    silent: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl CancelToken {
    /// 不连接任何外部来源的令牌，供测试与一次性重放使用。
    pub(crate) fn standalone() -> Self {
        Self::with_layers(|| false, || false)
    }

    /// 观察 `BackupState` 中串行任务的取消状态。命令取得运行锁后拥有自己的任务
    /// 令牌，`request_cancel` 置位后由这里读到；并行后台探针的让位取消不计入，
    /// 因此不会误伤刚进站的前台命令。
    ///
    /// GUI 的取消来源本来就是廉价、无副作用的读取，因此两层共用同一个闭包，
    /// 行为与分层前逐位一致。
    pub(crate) fn for_backup(state: BackupState) -> Self {
        let silent_state = state.clone();
        Self::with_layers(move || silent_state.cancelled(), move || state.cancelled())
    }

    /// 任意外部判定函数，例如无头入口的闸门检查点。两层共用同一个闭包——调用方
    /// 若需要一个无副作用的静默层，请改用 [`Self::with_layers`]。
    pub(crate) fn with_source(source: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        let shared: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(source);
        Self {
            local: Arc::new(AtomicBool::new(false)),
            interactive: shared.clone(),
            silent: shared,
        }
    }

    /// 显式区分的两层来源：`silent` 供热循环高频查询，`interactive` 只出现在边界。
    pub(crate) fn with_layers(
        silent: impl Fn() -> bool + Send + Sync + 'static,
        interactive: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            interactive: Arc::new(interactive),
            silent: Arc::new(silent),
        }
    }

    /// **交互检查点**：显式边界处的取消判定。可以写 marker、可以阻塞。
    pub(crate) fn is_cancelled(&self) -> bool {
        self.local.load(Ordering::SeqCst) || (self.interactive)()
    }

    /// **静默观察**：只读「取消是否已被判定」，无副作用，供热循环使用。
    pub(crate) fn observed(&self) -> bool {
        self.local.load(Ordering::SeqCst) || (self.silent)()
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
    use std::sync::atomic::AtomicUsize;

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
        assert!(other.observed());
    }

    /// 静默层不发交互检查点：热循环高频查询只读取消状态，不产生副作用。
    #[test]
    fn silent_layer_never_touches_the_interactive_source() {
        let silent = Arc::new(AtomicBool::new(false));
        let interactive_hits = Arc::new(AtomicUsize::new(0));
        let token = CancelToken::with_layers(
            {
                let silent = silent.clone();
                move || silent.load(Ordering::SeqCst)
            },
            {
                let interactive_hits = interactive_hits.clone();
                move || {
                    interactive_hits.fetch_add(1, Ordering::SeqCst);
                    false
                }
            },
        );

        for _ in 0..8 {
            assert!(!token.observed());
        }
        assert_eq!(
            interactive_hits.load(Ordering::SeqCst),
            0,
            "静默查询不得触碰交互层来源"
        );

        // 边界判定走交互层，其结论随后被静默层读到。
        assert!(!token.is_cancelled());
        assert_eq!(interactive_hits.load(Ordering::SeqCst), 1);
        silent.store(true, Ordering::SeqCst);
        assert!(token.observed());
    }
}
