use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use rusqlite::Connection;

/// 受管理文件的所有者类别。用于清理引用复查与完整性校验的错误定位。
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
/// 用户选择的绝对路径。`deletable` 表示该路径失去引用后能否进入普通清理队列；
/// 外部导出仍只能经用户逐条确认删除。该标志目前只随清单传递，阶段 7 的 kind
/// 分派校验会据此决定入队路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedFile {
    pub(crate) path: String,
    pub(crate) owner: ManagedFileOwner,
    pub(crate) deletable: bool,
}

/// [`ReferenceInventory::validate_target`] 按所有者复查的结果。
#[allow(dead_code)] // 冻结接口，阶段 7 的 kind 分派校验接入后消费。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationOutcome {
    /// 该所有者仍引用此目标。
    Valid,
    /// 该所有者不再引用此目标。
    Unreferenced,
    /// 目标声明的所有者不匹配，交由下一个 inventory 继续判断。
    OwnerMismatch(ManagedFileOwner),
}

/// 扫描时由领域提供的候选文件（绝对路径与报告原因）。
#[derive(Debug, Clone)]
pub(crate) struct ScanCandidateFile {
    pub(crate) path: PathBuf,
    pub(crate) reason: &'static str,
}

/// 一个领域对受管理文件清单与文件布局的公开视图。
///
/// 阶段 5 按计划冻结 `collect_targets` / `validate_target` 的签名：前者已用于
/// anti-join 与孤儿判定，后者要等阶段 7 的 kind 分派校验接入后才会有调用方
/// （届时 scrub 按所有者复查，而不是把领域查询写回 cleanup）。`scan_candidates`
/// 是同一契约上的只读扩展，供未引用文件扫描复用领域自己的命名与目录布局；
/// 阶段 7 为画板增加 kind 维度时继续以新方法扩展，不改动已有签名。
pub(crate) trait ReferenceInventory: Send + Sync {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String>;

    #[allow(dead_code)] // 冻结接口，阶段 7 的 kind 分派校验接入后消费。
    fn validate_target(
        &self,
        connection: &Connection,
        target: &ManagedFile,
    ) -> Result<ValidationOutcome, String>;

    fn scan_candidates(
        &self,
        _artwork_directory: &Path,
        _files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
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

    #[allow(dead_code)] // 冻结接口，阶段 7 的 kind 分派校验接入后消费。
    pub(crate) fn validate_target(
        &self,
        connection: &Connection,
        target: &ManagedFile,
    ) -> Result<ValidationOutcome, String> {
        for inventory in &self.inventories {
            match inventory.validate_target(connection, target)? {
                ValidationOutcome::OwnerMismatch(_) => continue,
                outcome => return Ok(outcome),
            }
        }
        Ok(ValidationOutcome::Unreferenced)
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
