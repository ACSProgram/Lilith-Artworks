use std::{fs, path::Path};

use rusqlite::Connection;

use crate::{
    contracts::{
        ManagedFile, ManagedFileOwner, ReferenceInventory, ScanCandidateFile, ValidationOutcome,
    },
    storage,
};

/// 历史 snapshot / delta 的引用清单。
pub(crate) struct HistoryReferenceInventory;

impl ReferenceInventory for HistoryReferenceInventory {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String> {
        let mut statement = connection
            .prepare(
                "SELECT snapshot_path, 0 FROM history_nodes
                   WHERE snapshot_path IS NOT NULL AND TRIM(snapshot_path) <> ''
                 UNION ALL
                 SELECT delta_path, 1 FROM history_nodes
                   WHERE delta_path IS NOT NULL AND TRIM(delta_path) <> ''
                 UNION ALL
                 SELECT delta_path, 2 FROM history_edges
                   WHERE TRIM(delta_path) <> ''",
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
                    0 => ManagedFileOwner::HistorySnapshot,
                    1 => ManagedFileOwner::HistoryNodeDelta,
                    _ => ManagedFileOwner::HistoryEdgeDelta,
                },
                deletable: true,
            });
        }
        Ok(targets)
    }

    fn validate_target(
        &self,
        connection: &Connection,
        target: &ManagedFile,
    ) -> Result<ValidationOutcome, String> {
        let query = match target.owner {
            ManagedFileOwner::HistorySnapshot => {
                "SELECT EXISTS(SELECT 1 FROM history_nodes WHERE snapshot_path = ?1)"
            }
            ManagedFileOwner::HistoryNodeDelta => {
                "SELECT EXISTS(SELECT 1 FROM history_nodes WHERE delta_path = ?1)"
            }
            ManagedFileOwner::HistoryEdgeDelta => {
                "SELECT EXISTS(SELECT 1 FROM history_edges WHERE delta_path = ?1)"
            }
            owner => return Ok(ValidationOutcome::OwnerMismatch(owner)),
        };
        let referenced = connection
            .query_row(query, [&target.path], |row| row.get::<_, bool>(0))
            .map_err(storage::database_error)?;
        Ok(if referenced {
            ValidationOutcome::Valid
        } else {
            ValidationOutcome::Unreferenced
        })
    }

    fn scan_candidates(
        &self,
        artwork_directory: &Path,
        files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
        collect_scan_directory(
            &artwork_directory.join("snapshots"),
            is_snapshot_name,
            "历史快照未被引用",
            files,
        )?;
        collect_scan_directory(
            &artwork_directory.join("deltas"),
            is_delta_name,
            "历史增量未被引用",
            files,
        )?;
        // 作品 `temp/` 只存放备份、恢复与检查点在发布前写下的 `NamedTempFile`
        // （`backup/worker.rs`、`backup/restore.rs`）。正常路径由 `Drop` 删除，
        // 进程被强杀或断电时残留，且永远不会进入引用清单。
        collect_scan_directory(
            &artwork_directory.join("temp"),
            is_scratch_name,
            "备份或恢复中断后残留的临时文件",
            files,
        )
    }
}

fn collect_scan_directory(
    directory: &Path,
    matches_name: fn(&str) -> bool,
    reason: &'static str,
    files: &mut Vec<ScanCandidateFile>,
) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("无法读取目录 {}：{error}", directory.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取目录项类型：{error}"))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if matches_name(name) {
            files.push(ScanCandidateFile {
                path: entry.path(),
                reason,
            });
        }
    }
    Ok(())
}

/// snapshot 命名：`<history-id>.lbc`，或修复 head snapshot 时的
/// `<history-id>-repair-<uuid>.lbc`。
fn is_snapshot_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".lbc") else {
        return false;
    };
    if uuid::Uuid::parse_str(stem).is_ok() {
        return true;
    }
    stem.split_once("-repair-").is_some_and(|(head, suffix)| {
        uuid::Uuid::parse_str(head).is_ok() && uuid::Uuid::parse_str(suffix).is_ok()
    })
}

/// delta 命名：`<child-id>-to-<parent-id>.lbd`。UUID 只含十六进制字符与连字符，
/// 因此 `-to-` 不会出现在单个 UUID 内部。
fn is_delta_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".lbd") else {
        return false;
    };
    stem.split_once("-to-").is_some_and(|(child, parent)| {
        uuid::Uuid::parse_str(child).is_ok() && uuid::Uuid::parse_str(parent).is_ok()
    })
}

/// 暂存文件命名：`tempfile::NamedTempFile` 的默认前缀是 `.tmp`，随后是随机后缀
/// （例如 `.tmpd0BGy8`）。作品 `temp/` 只由本进程的备份/恢复写入，因此只认这个
/// 前缀；用户手工放进该目录的其它文件不会被当成孤儿报告。
fn is_scratch_name(name: &str) -> bool {
    name.strip_prefix(".tmp")
        .is_some_and(|suffix| !suffix.is_empty())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn recognizes_managed_history_file_names() {
        let snapshot = storage::new_id();
        let repair = format!("{}-repair-{}", storage::new_id(), storage::new_id());
        let child = storage::new_id();
        let parent = storage::new_id();
        assert!(is_snapshot_name(&format!("{snapshot}.lbc")));
        assert!(is_snapshot_name(&format!("{repair}.lbc")));
        assert!(is_delta_name(&format!("{child}-to-{parent}.lbd")));
        assert!(!is_snapshot_name("notes.txt"));
        assert!(!is_delta_name(&format!("{child}.lbd")));
    }

    #[test]
    fn scans_only_history_named_files() {
        let directory = tempfile::tempdir().unwrap();
        let snapshots = directory.path().join("snapshots");
        let deltas = directory.path().join("deltas");
        fs::create_dir_all(&snapshots).unwrap();
        fs::create_dir_all(&deltas).unwrap();
        let snapshot = snapshots.join(format!("{}.lbc", storage::new_id()));
        let delta = deltas.join(format!(
            "{}-to-{}.lbd",
            storage::new_id(),
            storage::new_id()
        ));
        let unrelated = snapshots.join("notes.txt");
        for path in [&snapshot, &delta, &unrelated] {
            fs::write(path, b"payload").unwrap();
        }

        let mut files = Vec::new();
        HistoryReferenceInventory
            .scan_candidates(directory.path(), &mut files)
            .unwrap();
        let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&snapshot));
        assert!(paths.contains(&delta));
    }

    /// 作品 `temp/` 里的 `.tmp*` 是崩溃后残留的暂存文件：它既没有命名模式可匹配
    /// 受管理文件，也永远不会被引用，因此必须单独报告，否则收不回来。
    #[test]
    fn reports_scratch_files_left_in_the_artwork_temp_directory() {
        let directory = tempfile::tempdir().unwrap();
        let temp = directory.path().join("temp");
        fs::create_dir_all(&temp).unwrap();
        let residue = temp.join(".tmpd0BGy8");
        let unrelated = temp.join("notes.txt");
        for path in [&residue, &unrelated] {
            fs::write(path, b"payload").unwrap();
        }

        let mut files = Vec::new();
        HistoryReferenceInventory
            .scan_candidates(directory.path(), &mut files)
            .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, residue);
        assert_eq!(files[0].reason, "备份或恢复中断后残留的临时文件");
        assert!(!is_scratch_name(".tmp"));
    }
}
