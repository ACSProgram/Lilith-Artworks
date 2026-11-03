//! 仓库完整性、版本与语义校验。
//!
//! `validate` 用于打开仓库时的完整校验；`validate_current` 是同一路径的轻量
//! 版本检查，供已缓存完整校验结果的运行态复用；`validate_repository_semantics`
//! 在完整校验之后检查外键、ID、仓库内相对路径、分支工作文件与摘要这些表内约定。

use std::path::Path;

use rusqlite::Connection;

use crate::storage;

use super::{newer_version_error, repository_version, SCHEMA_VERSION};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static INTEGRITY_CHECK_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub(in crate::library) fn validate(connection: &Connection) -> Result<(), String> {
    #[cfg(test)]
    INTEGRITY_CHECK_COUNT.with(|count| count.set(count.get() + 1));

    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("无法检查作品数据库完整性：{error}"))?;
    if integrity != "ok" {
        return Err(format!("作品数据库完整性检查失败：{integrity}"));
    }
    validate_current_version(repository_version(connection)?)
}

pub(in crate::library) fn validate_current(connection: &Connection) -> Result<(), String> {
    validate_current_version(repository_version(connection)?)
}

pub(in crate::library) fn validate_repository_semantics(
    connection: &Connection,
    root: &Path,
) -> Result<(), String> {
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage::database_error)?;
    if foreign_keys.exists([]).map_err(storage::database_error)? {
        return Err("作品数据库外键完整性检查失败".into());
    }
    drop(foreign_keys);

    let mut ids = connection
        .prepare(
            "SELECT 'library_nodes.id', id FROM library_nodes
             UNION ALL SELECT 'artworks.id', id FROM artworks
             UNION ALL SELECT 'branches.id', id FROM branches
             UNION ALL SELECT 'history_nodes.id', id FROM history_nodes
             UNION ALL SELECT 'final_artifacts.id', id FROM final_artifacts
             UNION ALL SELECT 'certification_records.id', id FROM certification_records
             UNION ALL SELECT 'pending_file_cleanup.id', id FROM pending_file_cleanup
             UNION ALL SELECT 'repository_backup_runs.id', id FROM repository_backup_runs",
        )
        .map_err(storage::database_error)?;
    let id_rows = ids
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in id_rows {
        let (label, id) = row.map_err(storage::database_error)?;
        storage::validate_uuid(&id, &label)?;
    }
    drop(ids);

    // 空串在这些列上是「没有文件」的记账值，不是待校验的路径：检查点父节点名下的
    // 边按设计登记 `delta_path = ''`（取消检查点时重建），兼容列 `delta_path` 也
    // 可能在节点持有 snapshot 时被清空。按路径校验会把这类合法仓库挡在门外。
    // `snapshot_path` 没有这种记账语义（读取侧把非 NULL 一律当作可用快照），所以仍
    // 要求它非 NULL，不放过空串。
    let mut paths = connection
        .prepare(
            "SELECT 'history_nodes.snapshot_path', snapshot_path FROM history_nodes
               WHERE snapshot_path IS NOT NULL
             UNION ALL SELECT 'history_nodes.delta_path', delta_path FROM history_nodes
               WHERE delta_path IS NOT NULL AND TRIM(delta_path) <> ''
             UNION ALL SELECT 'history_edges.delta_path', delta_path FROM history_edges
               WHERE TRIM(delta_path) <> ''
             UNION ALL SELECT 'final_artifacts.source_path', source_path FROM final_artifacts
             UNION ALL SELECT 'certification_records.stored_path', stored_path
               FROM certification_records
             UNION ALL SELECT 'pin_board_bitmaps.file_path', file_path
               FROM pin_board_bitmaps
             UNION ALL SELECT 'pending_file_cleanup.path', path FROM pending_file_cleanup
               WHERE path_kind <> 'external_file'",
        )
        .map_err(storage::database_error)?;
    let path_rows = paths
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in path_rows {
        let (label, path) = row.map_err(storage::database_error)?;
        storage::validate_repository_relative_path(&path)
            .map_err(|error| format!("{label} 无效：{error}"))?;
    }
    drop(paths);

    // 分支工作文件按设计保存仓库之外的绝对路径，因此单独做结构校验：绝对路径、
    // 无设备命名空间、不在仓库目录内，但不检查文件是否存在，也不拒绝网络位置。
    // 读取侧必须挡住的是「打开别人的仓库后，无人值守流程直接对这条路径取元数据
    // 或整份复制」——这些使用点另外按 `unattended_source_path_rejection` 跳过。
    let mut branch_paths = connection
        .prepare(
            "SELECT 'branches.source_path', source_path FROM branches
             UNION ALL SELECT 'branches.source_path_key', source_path_key FROM branches",
        )
        .map_err(storage::database_error)?;
    let branch_rows = branch_paths
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in branch_rows {
        let (label, path) = row.map_err(storage::database_error)?;
        storage::validate_branch_source_path(root, &path, &label)?;
    }
    drop(branch_paths);

    let mut hashes = connection
        .prepare(
            "SELECT 'history_nodes.sha256', sha256 FROM history_nodes
             UNION ALL SELECT 'final_artifacts.source_sha256', source_sha256 FROM final_artifacts
             UNION ALL SELECT 'certification_records.output_sha256', output_sha256
               FROM certification_records
             UNION ALL SELECT 'pending_file_cleanup.expected_sha256', expected_sha256
               FROM pending_file_cleanup WHERE expected_sha256 IS NOT NULL",
        )
        .map_err(storage::database_error)?;
    let hash_rows = hashes
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in hash_rows {
        let (label, hash) = row.map_err(storage::database_error)?;
        storage::validate_sha256(&hash).map_err(|error| format!("{label} 无效：{error}"))?;
    }
    Ok(())
}

fn validate_current_version(version: i64) -> Result<(), String> {
    if version > SCHEMA_VERSION {
        return Err(newer_version_error(version));
    }
    if version != SCHEMA_VERSION {
        return Err(format!("作品仓库版本不受支持：{version}"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn take_integrity_check_count() -> usize {
    INTEGRITY_CHECK_COUNT.with(|count| count.replace(0))
}
