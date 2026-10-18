//! 命令执行底座：typed error、取消令牌、操作标识、事件信封与统一锁序。
//!
//! 结构见 0.3 计划 §3.A：Tauri / headless 适配器只解析 DTO、装配 context 并
//! 映射错误；业务入口是 `app::usecases` 中的函数，运行时由
//! [`OperationCoordinator`] 统一取锁、取消与观测。

mod cancel;
mod coordinator;
mod error;
mod events;
mod types;

// 这些类型是 GUI / headless / 未来维护入口共享的契约；其中一部分只在 `headless`
// feature 或测试构建中被引用，非该构建下保留再导出是有意为之。
#[allow(unused_imports)]
pub(crate) use self::{
    cancel::CancelToken,
    coordinator::{CommandContext, OperationCoordinator, RepositorySession},
    error::{CommandError, CommandErrorKind, CommandErrorSummary},
    events::{
        AppEvent, AppEventKind, EventSink, NoopEventSink, OperationObserver, OperationOutcome,
        SharedEventSink, TauriEventSink, APP_EVENT_NAME, APP_EVENT_VERSION,
    },
    types::{OperationClass, OperationDomain, OperationId},
};
