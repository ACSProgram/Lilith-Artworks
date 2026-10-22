use rusqlite::Connection;

use crate::{
    contracts::{ManagedFile, ManagedFileOwner, ReferenceInventory},
    storage,
};

/// 最终成品与认证输出的引用清单。认证导出路径在仓库之外，失去引用后仍需用户在
/// 清理队列里逐条确认，不能进入自动重放。
pub(crate) struct ArtifactReferenceInventory;

impl ReferenceInventory for ArtifactReferenceInventory {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String> {
        let mut statement = connection
            .prepare(
                "SELECT source_path, 0 FROM final_artifacts
                   WHERE TRIM(source_path) <> ''
                 UNION ALL
                 SELECT stored_path, 1 FROM certification_records
                   WHERE TRIM(stored_path) <> ''
                 UNION ALL
                 SELECT output_path, 2 FROM certification_records
                   WHERE TRIM(output_path) <> ''",
            )
            .map_err(storage::database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(storage::database_error)?;
        let mut targets = Vec::new();
        for row in rows {
            let (path, kind) = row.map_err(storage::database_error)?;
            targets.push(ManagedFile {
                path,
                owner: match kind {
                    0 => ManagedFileOwner::FinalArtifact,
                    1 => ManagedFileOwner::CertificationStored,
                    _ => ManagedFileOwner::CertificationOutput,
                },
            });
        }
        Ok(targets)
    }
}
