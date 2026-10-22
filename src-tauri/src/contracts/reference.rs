use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use rusqlite::Connection;

/// 受管理文件的所有者类别。用于清理引用复查时标注「谁还在引用这条路径」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ManagedFileOwner {
    HistorySnapshot,
    HistoryNodeDelta,
    HistoryEdgeDelta,
    FinalArtifact,
    CertificationStored,
    CertificationOutput,
    PinBoardDds,
}

impl ManagedFileOwner {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::HistorySnapshot => "历史快照",
            Self::HistoryNodeDelta => "历史节点",
            Self::HistoryEdgeDelta => "历史边",
            Self::FinalArtifact => "最终成品",
            Self::CertificationStored => "认证副本",
            Self::CertificationOutput => "认证导出记录",
            Self::PinBoardDds => "画板图片",
        }
    }
}

/// 一条仍被数据库引用的受管理文件。
///
/// `path` 与数据库列完全一致：仓库内文件使用 `/` 分隔的相对路径，认证导出使用
/// 用户选择的绝对路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedFile {
    pub(crate) path: String,
    pub(crate) owner: ManagedFileOwner,
}

/// 扫描时由领域提供的候选文件（绝对路径与报告原因）。
#[derive(Debug, Clone)]
pub(crate) struct ScanCandidateFile {
    pub(crate) path: PathBuf,
    pub(crate) reason: &'static str,
}

/// 一条仍被记录指向、但当前存储模型已经不需要的受管理文件。
///
/// 与 [`ManagedFile`] 相反：它**不进**引用清单（因此可以被清理），扫描时用领域
/// 自己的话说明为什么多余，而不是笼统报成「未被引用」。典型例子是父节点已经持有
/// 检查点 snapshot 之后残留的子边反向增量——物化从父节点自己的 snapshot 起步，
/// 这条增量永远不会被读取，取消检查点时会按需重建。
#[derive(Debug, Clone)]
pub(crate) struct RedundantFile {
    pub(crate) path: String,
    pub(crate) reason: &'static str,
}

/// 一个领域对受管理文件清单与文件布局的公开视图。
///
/// `collect_targets` 用于 anti-join 与孤儿判定；`scan_candidates` 供未引用文件
/// 扫描复用领域自己的命名与目录布局。新增文件类别时以新方法扩展，不改动已有签名。
pub(crate) trait ReferenceInventory: Send + Sync {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String>;

    fn scan_candidates(
        &self,
        _artwork_directory: &Path,
        _files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
        Ok(())
    }

    /// 领域内**已经多余**的受管理文件：记录还在，但当前存储模型不会再读它。
    ///
    /// 这些路径不得同时出现在 [`Self::collect_targets`] 里，否则清理前的引用复查
    /// 会把它们保留下来；扫描据此给出比「未被引用」更准确的原因。
    fn redundant_targets(&self, _connection: &Connection) -> Result<Vec<RedundantFile>, String> {
        Ok(Vec::new())
    }

    /// 即将删除的路径在领域记录里失去意义时，清掉指向它的那条记录。
    ///
    /// 实现必须自带「这条记录确实多余」的判定（例如父节点仍持有检查点 snapshot），
    /// **不得**仅因为调用方传入路径就清除一条仍在使用的引用：清理只负责删除文件，
    /// 记录是否还有读者由领域决定。默认实现不做任何事。
    fn forget_deleted(&self, _connection: &Connection, _paths: &[String]) -> Result<(), String> {
        Ok(())
    }
}

/// 应用层注册的实现列表。cleanup 只依赖这一层，不依赖任何具体领域。
pub(crate) struct ReferenceInventories {
    inventories: Vec<Box<dyn ReferenceInventory>>,
}

impl ReferenceInventories {
    pub(crate) fn new(inventories: Vec<Box<dyn ReferenceInventory>>) -> Self {
        Self { inventories }
    }

    /// 汇总全部引用目标，并按路径去重。同一路径被多个所有者引用时保留首次出现者。
    pub(crate) fn collect_targets(
        &self,
        connection: &Connection,
    ) -> Result<Vec<ManagedFile>, String> {
        let mut targets = HashMap::new();
        for inventory in &self.inventories {
            for target in inventory.collect_targets(connection)? {
                targets.entry(target.path.clone()).or_insert(target);
            }
        }
        Ok(targets.into_values().collect())
    }

    pub(crate) fn collect_paths(
        &self,
        connection: &Connection,
    ) -> Result<HashMap<String, ManagedFileOwner>, String> {
        Ok(self
            .collect_targets(connection)?
            .into_iter()
            .map(|target| (target.path, target.owner))
            .collect())
    }

    pub(crate) fn scan_candidates(
        &self,
        artwork_directory: &Path,
        files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
        for inventory in &self.inventories {
            inventory.scan_candidates(artwork_directory, files)?;
        }
        Ok(())
    }

    /// 汇总各领域的「仍被记录指向但已多余」路径，同一路径保留首次出现的原因。
    pub(crate) fn collect_redundant(
        &self,
        connection: &Connection,
    ) -> Result<HashMap<String, &'static str>, String> {
        let mut redundant = HashMap::new();
        for inventory in &self.inventories {
            for file in inventory.redundant_targets(connection)? {
                redundant.entry(file.path).or_insert(file.reason);
            }
        }
        Ok(redundant)
    }

    /// 通知各领域：这些路径即将被删除，指向它们的多余记录应当清掉。
    pub(crate) fn forget_deleted(
        &self,
        connection: &Connection,
        paths: &[String],
    ) -> Result<(), String> {
        for inventory in &self.inventories {
            inventory.forget_deleted(connection, paths)?;
        }
        Ok(())
    }
}

static INSTALLED: OnceLock<ReferenceInventories> = OnceLock::new();

/// 应用启动时注册默认实现。重复调用只保留第一次注册。
pub(crate) fn install_reference_inventories(inventories: ReferenceInventories) {
    let _ = INSTALLED.set(inventories);
}

/// 取当前进程的引用清单。
///
/// 生产路径由 `AppState` 在应用/无头运行时构造时注册；单元测试没有应用装配，
/// 因此测试构建允许惰性注册默认实现。
pub(crate) fn reference_inventories() -> &'static ReferenceInventories {
    #[cfg(test)]
    {
        INSTALLED.get_or_init(crate::app::contracts::default_reference_inventories)
    }
    #[cfg(not(test))]
    {
        INSTALLED
            .get()
            .expect("reference inventories must be installed before repository cleanup")
    }
}
