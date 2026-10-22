//! 领域之间的中立契约。
//!
//! `cleanup` 与 `backup` 不再直接认识 Library / Authenticity / Pin-board 的内部
//! 路径布局或校验函数：领域各自实现 [`ReferenceInventory`] 与
//! [`RepositoryIntegrityProvider`]，应用层在启动时注册实现列表。新增领域或文件类别
//! 只需扩展 provider，不必把具体模块反向导入清理模块。

mod backup;
mod reference;

pub(crate) use backup::{
    BackupIntegrityReport, BackupVerificationMode, RepositoryIntegrityProvider,
};
pub(crate) use reference::{
    install_reference_inventories, reference_inventories, ManagedFile, ManagedFileOwner,
    RedundantFile, ReferenceInventories, ReferenceInventory, ScanCandidateFile,
};
