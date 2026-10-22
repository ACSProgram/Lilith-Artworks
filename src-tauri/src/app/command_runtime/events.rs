use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

use super::error::CommandErrorSummary;
use super::types::OperationDomain;

/// `AppEvent` 契约版本。v1 的信封字段与事件语义冻结：后续只能追加可选字段，
/// 新事件类型以独立版本号引入。
pub(crate) const APP_EVENT_VERSION: u32 = 1;

/// Tauri 侧事件名。前端订阅这一个通道，再按 `type` 分发。
pub(crate) const APP_EVENT_NAME: &str = "app_event";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OperationOutcome {
    Succeeded,
    Cancelled,
    Failed,
}

impl OperationOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

/// 首批事件。信封字段负责 operation id、仓库 key、进度与完成 revision；
/// 这里只放事件自身的载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum AppEventKind {
    OperationStarted,
    OperationProgress,
    OperationFinished {
        outcome: OperationOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<CommandErrorSummary>,
        /// 后台任务完成时声明它属于哪个作品；命令事件不带该字段。
        #[serde(skip_serializing_if = "Option::is_none")]
        artwork_id: Option<String>,
        /// 后台任务完成时声明本次是否新增/删除了历史节点等结构变化。
        /// 命令事件不带该字段，前端按未知（需要完整刷新）处理。
        #[serde(skip_serializing_if = "Option::is_none")]
        structure_changed: Option<bool>,
    },
    RepositoryChanged,
    CleanupQueueChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_count: Option<u64>,
    },
}

/// v1 信封。字段名与语义冻结；新增字段必须是可选并带默认值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppEvent {
    pub(crate) version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repository_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) domain: Option<OperationDomain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
    pub(crate) cancellable: bool,
    pub(crate) completion_revision: u64,
    pub(crate) timestamp_ms: i64,
    #[serde(flatten)]
    pub(crate) kind: AppEventKind,
}

impl AppEvent {
    pub(crate) fn new(
        kind: AppEventKind,
        repository_key: Option<String>,
        operation_id: Option<String>,
        domain: Option<OperationDomain>,
        cancellable: bool,
        completion_revision: u64,
    ) -> Self {
        Self {
            version: APP_EVENT_VERSION,
            repository_key,
            operation_id,
            domain,
            phase: None,
            current: None,
            total: None,
            cancellable,
            completion_revision,
            timestamp_ms: crate::storage::now_ms().unwrap_or(0),
            kind,
        }
    }
}

/// 事件出口。GUI 用 Tauri 广播；无头与单测用 no-op / 记录实现，不把 Tauri
/// 类型带进 use-case。
pub(crate) trait EventSink: Send + Sync {
    fn emit(&self, event: &AppEvent);
}

#[cfg_attr(not(any(test, feature = "headless")), allow(dead_code))] // 测试与 headless 运行时使用。
pub(crate) struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &AppEvent) {}
}

pub(crate) struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &AppEvent) {
        use tauri::Emitter;
        if let Err(error) = self.app.emit(APP_EVENT_NAME, event) {
            log::warn!(
                "failed to emit app event: type={}, operation_id={}, error={error}",
                event.kind_type(),
                event.operation_id.as_deref().unwrap_or_default()
            );
        }
    }
}

impl AppEvent {
    pub(crate) fn kind_type(&self) -> &'static str {
        match &self.kind {
            AppEventKind::OperationStarted => "operationStarted",
            AppEventKind::OperationProgress => "operationProgress",
            AppEventKind::OperationFinished { .. } => "operationFinished",
            AppEventKind::RepositoryChanged => "repositoryChanged",
            AppEventKind::CleanupQueueChanged { .. } => "cleanupQueueChanged",
        }
    }
}

pub(crate) type SharedEventSink = Arc<dyn EventSink>;

/// 额外的进度观察者。GUI 不需要它（事件与运行状态已足够）；无头入口用它将
/// use-case 的进度原样写成 `stage` marker，保持压力测试的检查点语义。
pub(crate) trait OperationObserver: Send + Sync {
    fn progress(&self, label: &str, current: u64, total: u64);
}

pub(crate) struct NoopObserver;

impl OperationObserver for NoopObserver {
    fn progress(&self, _label: &str, _current: u64, _total: u64) {}
}
