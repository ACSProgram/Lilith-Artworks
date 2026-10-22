use std::fmt;

use serde::{Deserialize, Serialize};

/// 命令失败的稳定分类。`code` 是给 UI/重试策略看的稳定字符串，`kind` 是给日志与
/// 事件分组用的粗粒度类别，`message` 必须保持 0.2 的用户文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CommandErrorKind {
    Cancelled,
    Busy,
    InvalidInput,
    NotFound,
    Conflict,
    Io,
    Unauthorized,
    Repository,
    Internal,
}

impl CommandErrorKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Busy => "busy",
            Self::InvalidInput => "invalidInput",
            Self::NotFound => "notFound",
            Self::Conflict => "conflict",
            Self::Io => "io",
            Self::Unauthorized => "unauthorized",
            Self::Repository => "repository",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    pub(crate) code: String,
    pub(crate) kind: CommandErrorKind,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<String>,
}

/// 事件与日志中的错误摘要：与 [`CommandError`] 字段一致，单独成类型以免把
/// operation id 写两遍（信封已经携带）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandErrorSummary {
    pub(crate) code: String,
    pub(crate) kind: CommandErrorKind,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl CommandError {
    pub(crate) fn new(
        kind: CommandErrorKind,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            kind,
            message: message.into(),
            retryable,
            operation_id: None,
        }
    }

    pub(crate) fn cancelled(message: impl Into<String>) -> Self {
        Self::new(CommandErrorKind::Cancelled, "cancelled", message, false)
    }

    pub(crate) fn shutting_down() -> Self {
        Self::new(
            CommandErrorKind::Cancelled,
            "shutting_down",
            "应用正在退出，操作已取消",
            false,
        )
    }

    pub(crate) fn busy(message: impl Into<String>) -> Self {
        Self::new(CommandErrorKind::Busy, "repository_busy", message, true)
    }

    /// 设置文件已被另一个入口（设置页轮次、托盘或窗口几何）改写。调用方应重新
    /// 读取快照后再提交，而不是静默覆盖。
    pub(crate) fn settings_conflict() -> Self {
        Self::new(
            CommandErrorKind::Conflict,
            "settings_conflict",
            "设置已被其他入口修改，请重新加载后再保存",
            true,
        )
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(
            CommandErrorKind::Internal,
            "operation_failed",
            message,
            false,
        )
    }

    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(
            CommandErrorKind::InvalidInput,
            "invalid_input",
            message,
            false,
        )
    }

    /// 把既有领域函数的字符串错误带入 typed 通道。消息原样保留；只有可识别的
    /// 取消/占用/配置缺失等类别获得更精确的 code，其余归为 `operation_failed`。
    /// 新代码应直接用构造函数给出更精确的分类，而不是依赖这里的启发式。
    pub(crate) fn from_legacy(message: impl Into<String>) -> Self {
        let message = message.into();
        let (kind, code, retryable) = classify_legacy(&message);
        Self::new(kind, code, message, retryable)
    }

    pub(crate) fn with_operation_id(mut self, operation_id: &str) -> Self {
        self.operation_id = Some(operation_id.to_owned());
        self
    }

    pub(crate) fn summary(&self) -> CommandErrorSummary {
        CommandErrorSummary {
            code: self.code.clone(),
            kind: self.kind,
            message: self.message.clone(),
            retryable: self.retryable,
        }
    }
}

fn classify_legacy(message: &str) -> (CommandErrorKind, &'static str, bool) {
    if message.contains("已取消") {
        return (CommandErrorKind::Cancelled, "cancelled", false);
    }
    if message.contains("正在被另一个进程写入")
        || message.contains("正在被写入")
        || message.contains("请稍后重试")
    {
        return (CommandErrorKind::Busy, "repository_busy", true);
    }
    if message.contains("尚未配置作品仓库") {
        return (
            CommandErrorKind::NotFound,
            "repository_not_configured",
            false,
        );
    }
    if message.contains("版本不受支持")
        || message.contains("更新版本")
        || message.contains("不是 Lilith Artworks 仓库")
    {
        return (CommandErrorKind::Repository, "repository_invalid", false);
    }
    if message.contains("不存在") || message.contains("找不到") {
        return (CommandErrorKind::NotFound, "not_found", false);
    }
    if message.contains("已存在")
        || message.contains("不匹配")
        || message.contains("冲突")
        || message.contains("已变化")
    {
        return (CommandErrorKind::Conflict, "conflict", false);
    }
    if message.contains("无效")
        || message.contains("不能为空")
        || message.contains("超出")
        || message.contains("必须")
        || message.contains("不允许")
        || message.contains("不能超过")
    {
        return (CommandErrorKind::InvalidInput, "invalid_input", false);
    }
    if message.contains("无法读取")
        || message.contains("无法写入")
        || message.contains("无法打开")
        || message.contains("无法访问")
        || message.contains("无法创建")
    {
        return (CommandErrorKind::Io, "io_failed", true);
    }
    (CommandErrorKind::Internal, "operation_failed", false)
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::from_legacy(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::from_legacy(message)
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<CommandError> for String {
    fn from(error: CommandError) -> Self {
        error.message
    }
}

/// 认证领域的类型错误直接映射到命令分类，消息保持 `Display` 原样。
///
/// 只有 `Task` 中明确表示「已取消」的才归为 `Cancelled`；其余任务失败与图片、
/// TrustMark、C2PA、序列化错误统一归为 `Internal`，避免把领域内部失败误报成输入
/// 问题。
///
/// `Other` 是「原样文案」通道：仓储返回的 `String` 与命令层的参数提示都包在它里面
/// （见 `AuthenticityError::message`），这些文案在 typed 通道出现前就按内容分类，
/// 因此继续走 [`CommandError::from_legacy`] 的启发式——否则「已存在」「必须」这类
/// 输入问题会被降级成 `operation_failed`。
impl From<crate::authenticity::AuthenticityError> for CommandError {
    fn from(error: crate::authenticity::AuthenticityError) -> Self {
        use crate::authenticity::AuthenticityError;

        if matches!(error, AuthenticityError::Other(_)) {
            return Self::from_legacy(error.to_string());
        }
        let (kind, code, retryable) = match &error {
            AuthenticityError::InvalidInput(_) => {
                (CommandErrorKind::InvalidInput, "invalid_input", false)
            }
            AuthenticityError::Task(message) if message.contains("已取消") => {
                (CommandErrorKind::Cancelled, "cancelled", false)
            }
            AuthenticityError::Io(_) => (CommandErrorKind::Io, "io_failed", true),
            _ => (CommandErrorKind::Internal, "operation_failed", false),
        };
        Self::new(kind, code, error.to_string(), retryable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_messages_keep_text_and_gain_a_classification() {
        let cancelled = CommandError::from("操作已取消".to_owned());
        assert_eq!(cancelled.kind, CommandErrorKind::Cancelled);
        assert_eq!(cancelled.code, "cancelled");
        assert_eq!(cancelled.message, "操作已取消");

        let busy = CommandError::from("仓库正在被另一个进程写入；不支持同一仓库的多进程写入");
        assert_eq!(busy.kind, CommandErrorKind::Busy);
        assert!(busy.retryable);

        let invalid = CommandError::from("分支标题不能为空");
        assert_eq!(invalid.kind, CommandErrorKind::InvalidInput);

        let internal = CommandError::from("未知失败");
        assert_eq!(internal.kind, CommandErrorKind::Internal);
        assert!(!internal.retryable);
    }

    #[test]
    fn cancelling_a_checkpoint_is_not_a_cancelled_operation() {
        // 「不能取消」描述的是用户操作不合法，不应被当成任务被取消。
        let error = CommandError::from("该检查点是恢复祖先历史所需的唯一 snapshot，不能取消");
        assert_eq!(error.kind, CommandErrorKind::Internal);
    }

    #[test]
    fn authenticity_errors_map_to_stable_command_kinds() {
        use crate::authenticity::AuthenticityError;

        let invalid = CommandError::from(AuthenticityError::InvalidInput("坏参数".into()));
        assert_eq!(invalid.kind, CommandErrorKind::InvalidInput);
        assert_eq!(invalid.code, "invalid_input");
        assert_eq!(invalid.message, "参数无效：坏参数");

        let cancelled = CommandError::from(AuthenticityError::Task("认证任务已取消".into()));
        assert_eq!(cancelled.kind, CommandErrorKind::Cancelled);

        let io = CommandError::from(AuthenticityError::Io(std::io::Error::other("磁盘错误")));
        assert_eq!(io.kind, CommandErrorKind::Io);
        assert!(io.retryable);

        let internal = CommandError::from(AuthenticityError::Task("未知失败".into()));
        assert_eq!(internal.kind, CommandErrorKind::Internal);
    }

    #[test]
    fn authenticity_string_errors_keep_their_legacy_classification() {
        use crate::authenticity::AuthenticityError;

        // 仓储与参数提示走 `Other`，分类仍按文案，不降级成 `operation_failed`。
        let invalid =
            CommandError::from(AuthenticityError::message("JPEG 质量必须在 1 到 100 之间"));
        assert_eq!(invalid.kind, CommandErrorKind::InvalidInput);
        assert_eq!(invalid.message, "JPEG 质量必须在 1 到 100 之间");

        let conflict = CommandError::from(AuthenticityError::message(
            "导出目标已存在；请选择新的文件名",
        ));
        assert_eq!(conflict.kind, CommandErrorKind::Conflict);

        let missing = CommandError::from(AuthenticityError::message("找不到分支的最终成品"));
        assert_eq!(missing.kind, CommandErrorKind::NotFound);
    }
}
