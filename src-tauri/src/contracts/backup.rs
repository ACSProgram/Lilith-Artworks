use std::path::Path;

use crate::cancel::CancelToken;

/// 灾备副本的完整性校验结果。计数只用于报告，不改变 manifest 的逐文件摘要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BackupIntegrityReport {
    pub(crate) history_nodes: u64,
    pub(crate) final_artifacts: u64,
    pub(crate) certification_records: u64,
}

/// 发布前对副本的复核方式。复制阶段已经得到完整摘要；`Sample` 只重读首尾采样，
/// `Full` 完整重读。两者都不改变 manifest 里的完整摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupVerificationMode {
    Sample,
    #[allow(dead_code)] // 发布前默认抽样；保留全量复核供测试与维护入口按需选用。
    Full,
}

/// 由应用层注入的灾备完整性校验能力。
///
/// `backup::repository_backup` 只负责复制、摘要与 manifest；打开副本、校验历史链
/// 与认证受控文件这类领域原语由本 trait 的实现调用，灾备核心因此不再直接导入
/// Library 或 Authenticity。
pub(crate) trait RepositoryIntegrityProvider: Send + Sync {
    fn verify_repository(
        &self,
        root: &Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<BackupIntegrityReport, String>;
}
