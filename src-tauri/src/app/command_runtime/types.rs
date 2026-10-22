use std::fmt;

use serde::Serialize;

/// 一次命令执行的稳定标识。随命令的生命周期存在于事件、日志与 typed error 中，
/// 前端据此丢弃旧事件或把失败关联回发起它的入口。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct OperationId(String);

impl OperationId {
    pub(crate) fn new() -> Self {
        Self(crate::storage::new_id())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 运行分类决定 coordinator 取哪一层锁。固定锁序见 `OperationCoordinator`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OperationClass {
    /// 只读仓库：共享仓库 lease，不取仓库操作锁与运行锁。
    Read,
    /// 用户写命令：共享运行锁（UserOperation）+ 仓库 lease + 仓库操作锁。
    Write,
    /// 前台长命令：先登记前台等待并请求后台让位，再取得共享运行锁。
    Foreground,
    /// 仓库写命令但不取共享运行锁（素材板保持既有语义）。
    RepositoryWrite,
}

impl OperationClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Foreground => "foreground",
            Self::RepositoryWrite => "repositoryWrite",
        }
    }
}

/// 事件与日志里的领域标签。只用于观测与前端筛选，不参与业务分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OperationDomain {
    Library,
    History,
    Backup,
    Authenticity,
    PinBoard,
    Cleanup,
    Settings,
    Repository,
    Maintenance,
}

impl OperationDomain {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::History => "history",
            Self::Backup => "backup",
            Self::Authenticity => "authenticity",
            Self::PinBoard => "pinBoard",
            Self::Cleanup => "cleanup",
            Self::Settings => "settings",
            Self::Repository => "repository",
            Self::Maintenance => "maintenance",
        }
    }
}
