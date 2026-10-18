use std::path::Path;

use crate::{
    authenticity, backup,
    cancel::CancelToken,
    contracts::{BackupIntegrityReport, ReferenceInventories, RepositoryIntegrityProvider},
    history, library, pin_board,
};

/// 生产装配：把三个领域的引用清单注册到中立契约层。
pub(crate) fn default_reference_inventories() -> ReferenceInventories {
    ReferenceInventories::new(vec![
        Box::new(history::HistoryReferenceInventory),
        Box::new(authenticity::ArtifactReferenceInventory),
        Box::new(pin_board::PinBoardReferenceInventory),
    ])
}

pub(crate) fn install_default_reference_inventories() {
    crate::contracts::install_reference_inventories(default_reference_inventories());
}

/// 灾备副本的完整性校验实现：打开副本（迁移 + 完整性 + 语义）、校验历史链，
/// 再校验最终成品、认证副本与 C2PA 声明。它属于应用层编排，灾备核心只消费结果。
pub(crate) struct RepositoryScrubProvider;

impl RepositoryIntegrityProvider for RepositoryScrubProvider {
    fn verify_repository(
        &self,
        root: &Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<BackupIntegrityReport, String> {
        library::open_existing(root)?;
        let history_nodes = backup::scrub_history(root, cancel, |current, total| {
            progress("正在校验备份历史链", current, total)
        })?;
        let (final_artifacts, certification_records) = authenticity::scrub_controlled_files(
            root,
            || cancel.is_cancelled(),
            |current, total| progress("正在校验备份发布文件", current, total),
        )?;
        Ok(BackupIntegrityReport {
            history_nodes,
            final_artifacts,
            certification_records,
        })
    }
}
