//! 调度器与分支状态：自动检查元数据、失败/跳过记录、通知与快速检查基线。
//!
//! 这里的写入都只影响分支状态行，不改历史图；调度候选查询与「已启用工作文件数」
//! 共用同一套启用条件。过期备份的图操作不在这里，走 `repository` / `backup`。

use std::{collections::HashSet, path::Path};

use rusqlite::{params, OptionalExtension};

use crate::storage;

use super::{BackupDisableNoticeTarget, CheckMode, ScheduledBranch};

pub(crate) fn mark_unchanged(root: &Path, branch_id: &str, checked_ms: i64) -> Result<(), String> {
    storage::open(root)?
        .execute(
            "UPDATE branches SET last_check_ms = ?2, last_success_ms = ?2, last_error = NULL,
                consecutive_backup_failures = 0, backup_retry_at_ms = NULL, updated_ms = ?2
             WHERE id = ?1",
            params![branch_id, checked_ms],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 记录一次成功自动检查的耗时与方式。只由调度器在成功路径调用；失败、取消与
/// 让位不写，避免把半途而废的检查当成代价依据。
pub(crate) fn record_check_result(
    root: &Path,
    branch_id: &str,
    elapsed_ms: i64,
    mode: CheckMode,
) -> Result<(), String> {
    // 只有快速命中代表「这次又是靠指纹放过的」，计数递增；任何一次全量
    // （内容包括变化、快速检查退回全量或主动强制确认）都完整读过文件，
    // 盲区归零。
    let quick = i64::from(mode == CheckMode::Quick);
    storage::open(root)?
        .execute(
            "UPDATE branches SET
                last_check_elapsed_ms = ?2,
                last_check_mode = ?3,
                quick_hit_count = CASE WHEN ?4 <> 0 THEN quick_hit_count + 1 ELSE 0 END
             WHERE id = ?1",
            params![branch_id, elapsed_ms.max(0), mode.as_str(), quick],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 完整确认过工作文件后清零连续快速命中计数。任何提交（含手动提交）无论是否
/// 产生新节点都已经读过整份文件，因此与自动全量检查一样清掉盲区。
pub(crate) fn reset_quick_hit_count(root: &Path, branch_id: &str) -> Result<(), String> {
    storage::open(root)?
        .execute(
            "UPDATE branches SET quick_hit_count = 0 WHERE id = ?1 AND quick_hit_count <> 0",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 读取上次全量检查成功后记录的工作文件元数据。
/// `last_source_size` 为 NULL 表示还没有可信任的基线，快速检查必须退回全量。
pub(crate) fn load_source_metadata(
    root: &Path,
    branch_id: &str,
) -> Result<Option<(i64, Option<i64>)>, String> {
    Ok(storage::open(root)?
        .query_row(
            "SELECT last_source_size, last_source_modified_ms FROM branches WHERE id = ?1",
            [branch_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?
        .and_then(|(size, modified)| size.map(|value| (value, modified))))
}

/// 在全量检查或提交成功后记录工作文件的大小与修改时间，作为快速检查的基线。
pub(crate) fn record_source_metadata(
    root: &Path,
    branch_id: &str,
    length: u64,
    modified_ms: Option<i64>,
) -> Result<(), String> {
    storage::open(root)?
        .execute(
            "UPDATE branches SET last_source_size = ?2, last_source_modified_ms = ?3 WHERE id = ?1",
            params![
                branch_id,
                i64::try_from(length).map_err(|_| "源文件大小超出范围")?,
                modified_ms
            ],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

/// 分支状态写入的三种语义。
///
/// 三者都会写 `last_error`，但退避、连续失败计数与自动备份开关的后果完全不同：
/// 把「路径不参与无人值守」误当成一次失败会累计到自动关闭，而手动提交失败也不该
/// 推进调度退避。入口只有一个，语义由调用方显式选择。
pub(crate) enum BranchFailure<'a> {
    /// 手动提交失败：只写错误文案。
    Manual { error: &'a str },
    /// 自动备份失败：写错误、递增失败数、写重试时间，第 5 次连续失败时关闭。
    Automatic { error: &'a str, failed_ms: i64 },
    /// 工作文件位于网络位置或设备命名空间：写原因、清零失败与重试，不关闭自动备份。
    Ineligible { reason: &'a str },
}

/// 写入一次分支状态。只有 [`BranchFailure::Automatic`] 返回「本次是否关闭了自动备份」。
pub(crate) fn record_branch_failure(
    root: &Path,
    branch_id: &str,
    failure: BranchFailure<'_>,
) -> Result<bool, String> {
    match failure {
        BranchFailure::Manual { error } => {
            storage::open(root)?
                .execute(
                    "UPDATE branches SET last_error = ?2 WHERE id = ?1",
                    params![branch_id, error],
                )
                .map_err(storage::database_error)?;
            Ok(false)
        }
        BranchFailure::Ineligible { reason } => {
            // 原因未变时不写库：调度器每轮都会调用，不能因此产生写入。
            storage::open(root)?
                .execute(
                    "UPDATE branches SET last_error = ?2, consecutive_backup_failures = 0,
                        backup_retry_at_ms = NULL
                     WHERE id = ?1 AND (last_error IS NULL OR last_error <> ?2)",
                    params![branch_id, reason],
                )
                .map_err(storage::database_error)?;
            Ok(false)
        }
        BranchFailure::Automatic { error, failed_ms } => {
            const MAX_CONSECUTIVE_FAILURES: u32 = 5;
            let mut connection = storage::open(root)?;
            let transaction = connection.transaction().map_err(storage::database_error)?;
            let (previous_failures, interval_minutes): (u32, u32) = transaction
                .query_row(
                    "SELECT consecutive_backup_failures, backup_interval_minutes
                     FROM branches WHERE id = ?1",
                    [branch_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage::database_error)?;
            let failures = previous_failures.saturating_add(1);
            let disabled = failures >= MAX_CONSECUTIVE_FAILURES;
            let retry_minutes = match failures {
                1 => 1,
                2 => interval_minutes.div_ceil(4),
                3 => interval_minutes.div_ceil(2),
                _ => interval_minutes,
            };
            let retry_at_ms = (!disabled)
                .then(|| failed_ms.saturating_add(i64::from(retry_minutes).saturating_mul(60_000)));
            transaction
                .execute(
                    "UPDATE branches
                     SET last_error = ?2, consecutive_backup_failures = ?3,
                         backup_retry_at_ms = ?4,
                         backup_enabled = CASE WHEN ?5 <> 0 THEN 0 ELSE backup_enabled END,
                         backup_disable_notice_pending = CASE WHEN ?5 <> 0 THEN 1 ELSE backup_disable_notice_pending END,
                         updated_ms = ?6
                     WHERE id = ?1",
                    params![
                        branch_id,
                        error,
                        failures,
                        retry_at_ms,
                        i64::from(disabled),
                        failed_ms
                    ],
                )
                .map_err(storage::database_error)?;
            transaction.commit().map_err(storage::database_error)?;
            Ok(disabled)
        }
    }
}

pub(crate) fn acknowledge_backup_disable_notices(
    root: &Path,
    artwork_ids: &[String],
) -> Result<(), String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    for artwork_id in artwork_ids {
        transaction
            .execute(
                "UPDATE branches SET backup_disable_notice_pending = 0
             WHERE artwork_id = ?1 AND backup_disable_notice_pending <> 0",
                [artwork_id],
            )
            .map_err(storage::database_error)?;
    }
    transaction.commit().map_err(storage::database_error)?;
    Ok(())
}

pub(crate) fn next_backup_disable_notice_target(
    root: &Path,
) -> Result<Option<BackupDisableNoticeTarget>, String> {
    storage::open(root)?
        .query_row(
            "SELECT b.artwork_id, b.id
             FROM branches b
             JOIN library_nodes n ON n.id = b.artwork_id
             WHERE b.backup_disable_notice_pending <> 0 AND n.trashed_ms IS NULL
             ORDER BY b.updated_ms DESC, b.id
             LIMIT 1",
            [],
            |row| {
                Ok(BackupDisableNoticeTarget {
                    artwork_id: row.get(0)?,
                    branch_id: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)
}

/// 调度候选的列清单与启用条件。全量列表、单分支复查与「已启用工作文件数」
/// 都从这里派生，避免三处 SQL 各自漂移。
const SCHEDULED_COLUMNS: &str = "b.id, b.source_path, b.last_check_ms, b.backup_interval_minutes,
        b.backup_quick_enabled, b.backup_retry_at_ms, b.artwork_id,
        b.last_check_elapsed_ms, b.quick_hit_count, b.last_source_size";
const SCHEDULED_SCOPE: &str = "FROM branches b JOIN library_nodes n ON n.id = b.artwork_id
     WHERE b.backup_enabled <> 0 AND n.trashed_ms IS NULL
       AND TRIM(b.source_path) <> ''
       AND NOT EXISTS(SELECT 1 FROM final_artifacts f WHERE f.branch_id = b.id)";

fn scheduled_sql(by_id: bool) -> String {
    if by_id {
        format!("SELECT {SCHEDULED_COLUMNS} {SCHEDULED_SCOPE} AND b.id = ?1")
    } else {
        format!("SELECT {SCHEDULED_COLUMNS} {SCHEDULED_SCOPE} ORDER BY b.id")
    }
}

fn scheduled_branch(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledBranch> {
    Ok(ScheduledBranch {
        id: row.get(0)?,
        artwork_id: row.get(6)?,
        source_path: row.get(1)?,
        last_check_ms: row.get(2)?,
        last_check_elapsed_ms: row.get(7)?,
        quick_hit_count: row.get(8)?,
        last_source_size: row.get(9)?,
        interval_minutes: row.get(3)?,
        quick_enabled: row.get::<_, i64>(4)? != 0,
        retry_at_ms: row.get(5)?,
    })
}

pub(crate) fn list_scheduled(root: &Path) -> Result<Vec<ScheduledBranch>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(&scheduled_sql(false))
        .map_err(storage::database_error)?;
    let branches = statement
        .query_map([], scheduled_branch)
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(branches)
}

pub(crate) fn load_scheduled(
    root: &Path,
    branch_id: &str,
) -> Result<Option<ScheduledBranch>, String> {
    storage::open(root)?
        .query_row(&scheduled_sql(true), [branch_id], scheduled_branch)
        .optional()
        .map_err(storage::database_error)
}

/// 最近一次**历史内容写入**的时间（最新历史节点的 `created_ms`）。
///
/// 空闲完整性检查用它实现「仓库静默」防抖；校验状态自身的写入不计入，否则每批
/// 校验都会把下一次自动检查再推后十分钟。快速检查推进的 `last_check_ms` /
/// `last_success_ms` 也不算——它们没有产生新节点，若计入，备份间隔小于 10 分钟的
/// 分支会让静默条件永不满足。精简、检查点与快照修复不改这个时间戳，但它们已在
/// 前台运行锁内完成，且不会与空闲批次并发。
pub(crate) fn latest_history_write_ms(root: &Path) -> Result<Option<i64>, String> {
    storage::open(root)?
        .query_row("SELECT MAX(created_ms) FROM history_nodes", [], |row| {
            row.get(0)
        })
        .map_err(storage::database_error)
}

pub(crate) fn count_scheduled_files(root: &Path) -> Result<usize, String> {
    let connection = storage::open(root)?;
    let sql = format!("SELECT DISTINCT b.source_path_key, b.source_path {SCHEDULED_SCOPE}");
    let mut statement = connection.prepare(&sql).map_err(storage::database_error)?;
    // 与调度器同一口径：网络位置与设备命名空间的工作文件不参与自动备份，因此
    // 不计入「已启用自动备份的工作文件数」。
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    let mut keys = HashSet::new();
    for row in rows {
        let (key, path) = row.map_err(storage::database_error)?;
        if storage::unattended_source_path_rejection(&path).is_none() {
            keys.insert(key);
        }
    }
    Ok(keys.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{test_support::HistoryFixture, update_branch};

    #[test]
    fn scheduled_file_count_tracks_enabled_branches() {
        let fixture = HistoryFixture::new();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 1);

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            false,
            10,
            false,
            None,
        )
        .unwrap();
        assert_eq!(count_scheduled_files(&fixture.root).unwrap(), 0);
    }

    #[test]
    fn automatic_backup_failures_follow_interval_and_eventually_disable() {
        let fixture = HistoryFixture::new();
        mark_unchanged(&fixture.root, &fixture.main_branch_id, 100).unwrap();
        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Main",
            true,
            true,
            120,
            false,
            None,
        )
        .unwrap();

        let expected_delays = [1_i64, 30, 60, 120];
        for (index, delay_minutes) in expected_delays.into_iter().enumerate() {
            let failed_ms = 1_000 + index as i64;
            assert!(!record_branch_failure(
                &fixture.root,
                &fixture.main_branch_id,
                BranchFailure::Automatic {
                    error: "temporary failure",
                    failed_ms,
                },
            )
            .unwrap());
            let retry_at: i64 = storage::open(&fixture.root)
                .unwrap()
                .query_row(
                    "SELECT backup_retry_at_ms FROM branches WHERE id = ?1",
                    [&fixture.main_branch_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retry_at, failed_ms + delay_minutes * 60_000);
        }
        assert!(record_branch_failure(
            &fixture.root,
            &fixture.main_branch_id,
            BranchFailure::Automatic {
                error: "persistent failure",
                failed_ms: 2_000,
            },
        )
        .unwrap());

        let state: (Option<i64>, Option<i64>, Option<String>, u32, bool, bool) =
            storage::open(&fixture.root)
                .unwrap()
                .query_row(
                    "SELECT last_check_ms, last_success_ms, last_error,
                        consecutive_backup_failures, backup_enabled,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                    [&fixture.main_branch_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .unwrap();
        assert_eq!(state.0, Some(100));
        assert_eq!(state.1, Some(100));
        assert_eq!(state.2.as_deref(), Some("persistent failure"));
        assert_eq!(state.3, 5);
        assert!(!state.4);
        assert!(state.5);
        assert_eq!(
            next_backup_disable_notice_target(&fixture.root).unwrap(),
            Some(BackupDisableNoticeTarget {
                artwork_id: fixture.artwork_id.clone(),
                branch_id: fixture.main_branch_id.clone(),
            })
        );

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Renamed while stale",
            true,
            true,
            60,
            false,
            None,
        )
        .unwrap();
        let stale_update: (String, bool, u32, bool) = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT title, backup_enabled, consecutive_backup_failures,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(stale_update.0, "Renamed while stale");
        assert!(!stale_update.1);
        assert_eq!(stale_update.2, 5);
        assert!(stale_update.3);

        update_branch(
            &fixture.root,
            &fixture.main_branch_id,
            "Renamed while stale",
            false,
            true,
            60,
            false,
            None,
        )
        .unwrap();
        let reenabled: (bool, u32, bool) = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT backup_enabled, consecutive_backup_failures,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(reenabled.0);
        assert_eq!(reenabled.1, 0);
        assert!(!reenabled.2);

        acknowledge_backup_disable_notices(&fixture.root, &[fixture.artwork_id.clone()]).unwrap();
        let pending: bool = storage::open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT backup_disable_notice_pending FROM branches WHERE id = ?1",
                [&fixture.main_branch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!pending);
        assert_eq!(
            next_backup_disable_notice_target(&fixture.root).unwrap(),
            None
        );
    }
}
