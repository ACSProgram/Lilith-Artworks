use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use rusqlite::{params, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    pin_board::repository::{is_dds_name, parse_board_dds_path, BOARD_DIRECTORY},
    storage,
};

const REPOSITORY_FILE: &str = "repository_file";
const REPOSITORY_DIRECTORY: &str = "repository_directory";
const EXTERNAL_FILE: &str = "external_file";

/// 未引用文件扫描的宽限期：修改时间晚于 `now - 30 分钟` 的文件视为可能仍属于
/// 进行中的提交、精简或检查点发布，扫描跳过它们。取值明显大于空闲链路校验的
/// 10 分钟延迟，给长操作留出余量。
pub(crate) const SCAN_GRACE_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupFailure {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupReport {
    pub(crate) cleaned_count: usize,
    pub(crate) pending_count: usize,
    pub(crate) failures: Vec<CleanupFailure>,
}

/// 一条未引用文件扫描候选。扫描只报告、不删除；用户在设置页确认后经
/// `cleanup_unreferenced` 批量入队并单遍重放。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScanCandidate {
    /// 仓库相对路径（`/` 分隔），与 `pending_file_cleanup.path` 同一口径。
    pub(crate) path: String,
    pub(crate) byte_size: u64,
    pub(crate) reason: String,
}

/// 待清理队列中的一条条目，供设置页展示与重试。`last_attempt_ms` / `last_error`
/// 只在一次删除尝试失败后写入，因此尚未尝试过的条目两者为空。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingCleanupEntry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) path_kind: String,
    /// 入队原因（如 `history_branch_deletion`、`pin_board_finalize`）。
    pub(crate) reason: String,
    pub(crate) created_ms: i64,
    pub(crate) last_attempt_ms: Option<i64>,
    pub(crate) last_error: Option<String>,
}

struct PendingCleanup {
    id: String,
    path_kind: String,
    path: String,
    expected_sha256: Option<String>,
}

pub(crate) fn enqueue_repository_file(
    transaction: &Transaction<'_>,
    path: &str,
    reason: &str,
) -> Result<String, String> {
    enqueue(transaction, REPOSITORY_FILE, path, None, reason)
}

pub(crate) fn enqueue_repository_file_with_hash(
    transaction: &Transaction<'_>,
    path: &str,
    expected_sha256: &str,
    reason: &str,
) -> Result<String, String> {
    storage::validate_sha256(expected_sha256)?;
    enqueue(
        transaction,
        REPOSITORY_FILE,
        path,
        Some(expected_sha256),
        reason,
    )
}

pub(crate) fn enqueue_repository_directory(
    transaction: &Transaction<'_>,
    path: &str,
    reason: &str,
) -> Result<String, String> {
    enqueue(transaction, REPOSITORY_DIRECTORY, path, None, reason)
}

pub(crate) fn enqueue_external_file(
    transaction: &Transaction<'_>,
    path: &str,
    expected_sha256: &str,
    reason: &str,
) -> Result<String, String> {
    storage::validate_sha256(expected_sha256)?;
    enqueue(
        transaction,
        EXTERNAL_FILE,
        path,
        Some(expected_sha256),
        reason,
    )
}

/// 在事务内为一批「已释放引用的仓库文件」登记提交后删除意图。
///
/// 与直接 `enqueue_repository_file` 的差别是它先做引用复查：事务可见本事务尚未
/// 提交的写入，因此调用方可以在删除图记录之后、提交之前调用它，仍被其它节点或
/// 分支引用的路径会被跳过，只入队已经无引用的文件。返回已入队的 cleanup id，
/// 调用方在提交成功后执行 `replay`。
pub(crate) fn enqueue_released_repository_files(
    transaction: &Transaction<'_>,
    paths: &[String],
    reason: &str,
) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for path in paths {
        if !seen.insert(path.as_str()) {
            continue;
        }
        if referenced_path_kind(transaction, REPOSITORY_FILE, path)?.is_none() {
            ids.push(enqueue_repository_file(transaction, path, reason)?);
        }
    }
    Ok(ids)
}

fn enqueue(
    transaction: &Transaction<'_>,
    path_kind: &str,
    path: &str,
    expected_sha256: Option<&str>,
    reason: &str,
) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("待清理路径不能为空".into());
    }
    let id = storage::new_id();
    transaction
        .execute(
            "INSERT INTO pending_file_cleanup
             (id, path_kind, path, expected_sha256, reason, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path_kind, path) DO UPDATE SET
               expected_sha256 = COALESCE(excluded.expected_sha256, pending_file_cleanup.expected_sha256),
               reason = excluded.reason",
            params![
                id,
                path_kind,
                path.trim(),
                expected_sha256.map(str::to_ascii_uppercase),
                reason,
                storage::now_ms()?
            ],
        )
        .map_err(storage::database_error)?;
    transaction
        .query_row(
            "SELECT id FROM pending_file_cleanup WHERE path_kind = ?1 AND path = ?2",
            params![path_kind, path.trim()],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

pub(crate) fn complete(
    transaction: &Transaction<'_>,
    cleanup_ids: &[String],
) -> Result<(), String> {
    for id in cleanup_ids {
        let deleted = transaction
            .execute("DELETE FROM pending_file_cleanup WHERE id = ?1", [id])
            .map_err(storage::database_error)?;
        if deleted != 1 {
            return Err(format!("待清理文件登记已丢失：{id}"));
        }
    }
    Ok(())
}

pub(crate) fn discard(root: &Path, cleanup_ids: &[String]) -> Result<(), String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    complete(&transaction, cleanup_ids)?;
    transaction.commit().map_err(storage::database_error)
}

/// 列出待清理队列（按入队顺序）。只读，不执行任何删除，也不写 `last_attempt_ms`
/// 或 `last_error`；设置页据此展示路径、原因与上次失败信息，再决定是否重试。
pub(crate) fn list_pending(root: &Path) -> Result<Vec<PendingCleanupEntry>, String> {
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, path, path_kind, reason, created_ms, last_attempt_ms, last_error
             FROM pending_file_cleanup ORDER BY created_ms, id",
        )
        .map_err(storage::database_error)?;
    let entries = statement
        .query_map([], |row| {
            Ok(PendingCleanupEntry {
                id: row.get(0)?,
                path: row.get(1)?,
                path_kind: row.get(2)?,
                reason: row.get(3)?,
                created_ms: row.get(4)?,
                last_attempt_ms: row.get(5)?,
                last_error: row.get(6)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(entries)
}

pub(crate) fn run(root: &Path, requested_ids: &[String]) -> Result<CleanupReport, String> {
    let requested = requested_ids.iter().cloned().collect::<HashSet<_>>();
    let connection = storage::open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, path_kind, path, expected_sha256
             FROM pending_file_cleanup ORDER BY created_ms, id",
        )
        .map_err(storage::database_error)?;
    let entries = statement
        .query_map([], |row| {
            Ok(PendingCleanup {
                id: row.get(0)?,
                path_kind: row.get(1)?,
                path: row.get(2)?,
                expected_sha256: row.get(3)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(statement);

    let mut cleaned_count = 0;
    let mut failures = Vec::new();
    for entry in entries
        .into_iter()
        .filter(|entry| requested.is_empty() || requested.contains(&entry.id))
    {
        let result = match referenced_path(&connection, &entry) {
            Ok(Some(reference)) => Err(format!("文件仍被 {reference} 引用，已保留")),
            Ok(None) => remove_entry(root, &entry),
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                connection
                    .execute(
                        "DELETE FROM pending_file_cleanup WHERE id = ?1",
                        [&entry.id],
                    )
                    .map_err(storage::database_error)?;
                cleaned_count += 1;
            }
            Err(error) => {
                connection
                    .execute(
                        "UPDATE pending_file_cleanup
                         SET last_attempt_ms = ?2, last_error = ?3 WHERE id = ?1",
                        params![entry.id, storage::now_ms()?, error],
                    )
                    .map_err(storage::database_error)?;
                failures.push(CleanupFailure {
                    id: entry.id,
                    path: entry.path,
                    error,
                });
            }
        }
    }
    let pending_count = pending_count_with_connection(&connection)?;
    log::info!(
        "file cleanup finished: cleaned={cleaned_count}, failed={}, pending={pending_count}",
        failures.len(),
    );
    Ok(CleanupReport {
        cleaned_count,
        pending_count,
        failures,
    })
}

/// 提交成功后重放清理队列（单遍）。
///
/// 与 `run` 的差别只在于失败处理：条目级删除失败与重放自身的数据库错误都只记
/// 日志，不改变调用方的成功/失败语义——条目仍留在 `pending_file_cleanup` 中，
/// 带 `last_error` 与 `last_attempt_ms`，等待下次重放或设置页手动重试。每次调用
/// 只做一遍尝试，不循环重试，因此不会阻塞前台命令的进度。
pub(crate) fn replay(root: &Path, cleanup_ids: &[String]) {
    if cleanup_ids.is_empty() {
        return;
    }
    match run(root, cleanup_ids) {
        Ok(report) => {
            for failure in &report.failures {
                log::warn!(
                    "待清理条目重放失败，已留在队列可重试：{}：{}",
                    failure.path,
                    failure.error
                );
            }
        }
        Err(error) => {
            log::warn!("清理重放失败，条目已留在待清理队列：{error}");
        }
    }
}

/// 扫描 `artworks/*/snapshots/`、`artworks/*/deltas/` 与 `artworks/*/boards/*/`
/// 中未被任何数据库引用的 snapshot/delta 与画板 DDS 文件（崩溃孤儿、手工复制或
/// 历史迁移遗留），作为清理账本的发现机制。只报告、不删除；确认清理由
/// `cleanup_unreferenced` 完成。
///
/// 护栏：只考虑匹配既有命名模式的文件（snapshot `<UUID>.lbc` /
/// `<UUID>-repair-<UUID>.lbc`，delta `<UUID>-to-<UUID>.lbd`，画板 DDS
/// `<image-id>.dds`）；只报告修改时间早于宽限期（`SCAN_GRACE_MS`）的文件，避免
/// 与进行中的提交、精简、检查点发布或画板写入赛跑；逐文件复用
/// `referenced_path_kind` 做反向引用检查（含 `pin_board_images`）。调用方须在仓库
/// 操作锁内运行本函数（与调度器、前台长命令互斥），本函数自身不加锁。
pub(crate) fn scan_unreferenced(
    root: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(u64, u64),
) -> Result<Vec<ScanCandidate>, String> {
    let connection = storage::open(root)?;
    let cutoff = storage::now_ms()?.saturating_sub(SCAN_GRACE_MS);
    let mut files = Vec::new();
    collect_scan_files(root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let total = files.len() as u64;
    let mut candidates = Vec::new();
    for (index, (path, reason)) in files.into_iter().enumerate() {
        if cancelled() {
            return Err("未引用文件扫描已取消".into());
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                progress(index as u64 + 1, total);
                continue;
            }
            Err(error) => return Err(format!("无法读取待扫描文件：{error}")),
        };
        let modified = metadata
            .modified()
            .map_err(|error| format!("无法读取文件修改时间：{error}"))?;
        if system_time_ms(modified)? > cutoff {
            progress(index as u64 + 1, total);
            continue;
        }
        let relative = storage::relative_path(root, &path)?;
        if referenced_path_kind(&connection, REPOSITORY_FILE, &relative)?.is_none() {
            candidates.push(ScanCandidate {
                path: relative,
                byte_size: metadata.len(),
                reason: reason.to_owned(),
            });
        }
        progress(index as u64 + 1, total);
    }
    Ok(candidates)
}

/// 把用户确认的扫描候选批量入队并立即单遍重放删除。
///
/// 每条候选入队时登记当前 SHA-256 作为期望摘要（重放时内容已变即保留）；候选
/// 文件已不存在时跳过。幂等：重复调用不会重复删除，文件已被移除也不报错；重放
/// 前仍会复查引用，候选在确认前重新被引用时条目留队可重试。
pub(crate) fn cleanup_unreferenced(root: &Path, paths: &[String]) -> Result<CleanupReport, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let mut cleanup_ids = Vec::new();
    for path in paths {
        let absolute = safe_repository_path(root, path)?;
        if !absolute.is_file() {
            continue;
        }
        let expected = sha256_file(&absolute)?;
        cleanup_ids.push(enqueue_repository_file_with_hash(
            &transaction,
            path,
            &expected,
            "unreferenced_scan",
        )?);
    }
    transaction.commit().map_err(storage::database_error)?;
    if cleanup_ids.is_empty() {
        return Ok(CleanupReport {
            cleaned_count: 0,
            pending_count: pending_count_with_connection(&connection)?,
            failures: Vec::new(),
        });
    }
    drop(connection);
    run(root, &cleanup_ids)
}

/// 收集扫描范围内的候选文件（仓库相对路径不在此处计算），保留其目录归属对应的
/// 报告原因。目录缺失（如新仓库尚无任何作品）按空处理。
fn collect_scan_files(root: &Path, files: &mut Vec<(PathBuf, &'static str)>) -> Result<(), String> {
    let artworks = root.join("artworks");
    let entries = match fs::read_dir(&artworks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("无法读取作品目录：{error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取作品目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取作品目录项类型：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        let directory = entry.path();
        collect_scan_directory(
            &directory.join("snapshots"),
            is_snapshot_name,
            "历史快照未被引用",
            files,
        )?;
        collect_scan_directory(
            &directory.join("deltas"),
            is_delta_name,
            "历史增量未被引用",
            files,
        )?;
        collect_board_scan_files(&directory, files)?;
    }
    Ok(())
}

/// 收集某个 Artwork 下全部画板目录中的 DDS 文件。目录缺失按空处理。
fn collect_board_scan_files(
    artwork_directory: &Path,
    files: &mut Vec<(PathBuf, &'static str)>,
) -> Result<(), String> {
    let boards = artwork_directory.join(BOARD_DIRECTORY);
    let entries = match fs::read_dir(&boards) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("无法读取画板目录：{error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取画板目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取画板目录项类型：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        collect_scan_directory(&entry.path(), is_dds_name, "画板图片未被引用", files)?;
    }
    Ok(())
}

fn collect_scan_directory(
    directory: &Path,
    matches_name: fn(&str) -> bool,
    reason: &'static str,
    files: &mut Vec<(PathBuf, &'static str)>,
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
            files.push((entry.path(), reason));
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

fn system_time_ms(time: std::time::SystemTime) -> Result<i64, String> {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("文件修改时间无效：{error}"))?;
    i64::try_from(duration.as_millis()).map_err(|_| "文件修改时间超出范围".into())
}

fn remove_entry(root: &Path, entry: &PendingCleanup) -> Result<(), String> {
    match entry.path_kind.as_str() {
        REPOSITORY_FILE => {
            remove_repository_file(root, &entry.path, entry.expected_sha256.as_deref())
        }
        REPOSITORY_DIRECTORY => remove_repository_directory(root, &entry.path),
        EXTERNAL_FILE => remove_external_file(&entry.path, entry.expected_sha256.as_deref()),
        _ => Err("待清理条目的路径类型无效".into()),
    }
}

fn referenced_path(
    connection: &rusqlite::Connection,
    entry: &PendingCleanup,
) -> Result<Option<String>, String> {
    referenced_path_kind(connection, &entry.path_kind, &entry.path)
}

/// 复查某个路径是否仍被数据库引用（五张表），返回引用来源标签。
///
/// `run` 的重放与领域函数在事务内入队前的引用复查共用这一份查询：传入
/// `&Transaction` 时可见本事务尚未提交的写入，因此「先改图、再复查、再入队」
/// 与「提交后重放」看到的是同一套引用关系，两条路径不会得出相反结论。
pub(crate) fn referenced_path_kind(
    connection: &rusqlite::Connection,
    path_kind: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let reference = match path_kind {
        REPOSITORY_FILE => {
            let reference = connection
                .query_row(
                    "SELECT CASE
                       WHEN EXISTS(SELECT 1 FROM final_artifacts WHERE source_path = ?1) THEN '最终成品'
                       WHEN EXISTS(SELECT 1 FROM certification_records WHERE stored_path = ?1) THEN '认证副本'
                       WHEN EXISTS(SELECT 1 FROM history_nodes WHERE snapshot_path = ?1 OR delta_path = ?1) THEN '历史节点'
                       WHEN EXISTS(SELECT 1 FROM history_edges WHERE delta_path = ?1) THEN '历史边'
                     END",
                    [path],
                    |row| row.get(0),
                )
                .map_err(storage::database_error)?;
            match reference {
                Some(reference) => Some(reference),
                None => referenced_pin_board_dds(connection, path)?,
            }
        }
        REPOSITORY_DIRECTORY => {
            let prefix = format!("{}/%", path.trim_end_matches(['/', '\\']));
            connection
                .query_row(
                    "SELECT CASE
                       WHEN EXISTS(SELECT 1 FROM final_artifacts WHERE source_path LIKE ?1) THEN '最终成品'
                       WHEN EXISTS(SELECT 1 FROM certification_records WHERE stored_path LIKE ?1) THEN '认证副本'
                       WHEN EXISTS(SELECT 1 FROM history_nodes WHERE snapshot_path LIKE ?1 OR delta_path LIKE ?1) THEN '历史节点'
                       WHEN EXISTS(SELECT 1 FROM history_edges WHERE delta_path LIKE ?1) THEN '历史边'
                     END",
                    [&prefix],
                    |row| row.get(0),
                )
                .map_err(storage::database_error)?
        }
        EXTERNAL_FILE => connection
            .query_row(
                "SELECT CASE WHEN EXISTS(
                   SELECT 1 FROM certification_records WHERE output_path = ?1
                 ) THEN '认证导出记录' END",
                [path],
                |row| row.get(0),
            )
            .map_err(storage::database_error)?,
        _ => None,
    };
    Ok(reference)
}

/// 复查画板 DDS 是否仍被 `pin_board_images` 记录引用（画板记录删除后即不再引用）。
/// 路径形状交给 `pin_board::repository::parse_board_dds_path` 解析；非画板 DDS 路径
/// 一律返回 `None`。
fn referenced_pin_board_dds(
    connection: &rusqlite::Connection,
    path: &str,
) -> Result<Option<String>, String> {
    let Some((artwork_id, board_id, image_id)) = parse_board_dds_path(path) else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT CASE WHEN EXISTS(
               SELECT 1 FROM pin_board_images i
               JOIN pin_boards b ON b.id = i.board_id
               WHERE i.id = ?1 AND i.board_id = ?2 AND b.artwork_id = ?3
             ) THEN '画板图片' END",
            params![image_id, board_id, artwork_id],
            |row| row.get(0),
        )
        .map_err(storage::database_error)
}

fn remove_repository_file(
    root: &Path,
    relative: &str,
    expected_sha256: Option<&str>,
) -> Result<(), String> {
    let path = safe_repository_path(root, relative)?;
    if path.exists() {
        if !path.is_file() {
            return Err("仓库清理路径不再是普通文件".into());
        }
        if let Some(expected) = expected_sha256 {
            let actual = sha256_file(&path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err("仓库文件内容已变化，为避免误删已保留该文件".into());
            }
        }
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法删除仓库文件：{error}")),
    }
}

fn remove_repository_directory(root: &Path, relative: &str) -> Result<(), String> {
    let path = safe_repository_path(root, relative)?;
    match fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法删除仓库目录：{error}")),
    }
}

fn safe_repository_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.trim().is_empty() || Path::new(relative).components().count() == 0 {
        return Err("仓库清理路径不能为空".into());
    }
    let path = storage::resolve_path(root, relative)?;
    if path.exists() {
        let repository = root
            .canonicalize()
            .map_err(|error| format!("无法校验仓库目录：{error}"))?;
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("无法校验仓库清理路径：{error}"))?;
        if canonical == repository || !canonical.starts_with(&repository) {
            return Err("仓库清理路径越出作品仓库边界".into());
        }
    }
    Ok(path)
}

fn remove_external_file(path: &str, expected_sha256: Option<&str>) -> Result<(), String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("外部清理路径必须是绝对文件路径".into());
    }
    if !path.exists() {
        return Ok(());
    }
    if !path.is_file() {
        return Err("外部清理路径不再是普通文件".into());
    }
    let expected = expected_sha256.ok_or("外部清理条目缺少期望 SHA-256")?;
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err("外部文件内容已变化，为避免误删已保留该文件".into());
    }
    fs::remove_file(path).map_err(|error| format!("无法删除外部导出文件：{error}"))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取待清理文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法读取待清理文件：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

fn pending_count_with_connection(connection: &rusqlite::Connection) -> Result<usize, String> {
    let count = connection
        .query_row("SELECT COUNT(*) FROM pending_file_cleanup", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(storage::database_error)?;
    usize::try_from(count).map_err(|_| "待清理文件数量超出范围".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enqueue_external(root: &Path, path: &Path, expected_sha256: &str) -> String {
        let mut connection = storage::open(root).unwrap();
        let transaction = connection.transaction().unwrap();
        let id = enqueue_external_file(
            &transaction,
            &storage::display_path(path),
            expected_sha256,
            "test",
        )
        .unwrap();
        transaction.commit().unwrap();
        id
    }

    #[test]
    fn external_cleanup_requires_the_expected_hash_and_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let output = directory.path().join("output.jpg");
        crate::library::initialize(&root).unwrap();
        fs::write(&output, b"original").unwrap();
        let expected = sha256_file(&output).unwrap();
        let id = enqueue_external(&root, &output, &expected);
        fs::write(&output, b"changed").unwrap();

        let failed = run(&root, std::slice::from_ref(&id)).unwrap();
        assert_eq!(failed.failures.len(), 1);
        assert!(output.is_file());
        assert_eq!(failed.pending_count, 1);

        fs::write(&output, b"original").unwrap();
        let retried = run(&root, &[id]).unwrap();
        assert!(retried.failures.is_empty());
        assert!(!output.exists());
        assert_eq!(retried.pending_count, 0);
    }

    #[test]
    fn repository_cleanup_rejects_parent_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let outside = directory.path().join("outside");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let mut connection = storage::open(&root).unwrap();
        let transaction = connection.transaction().unwrap();
        let id = enqueue_repository_directory(&transaction, "../outside", "test").unwrap();
        transaction.commit().unwrap();

        let report = run(&root, &[id]).unwrap();

        assert_eq!(report.failures.len(), 1);
        assert!(outside.is_dir());
    }

    #[test]
    fn repository_cleanup_checks_expected_hash_and_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored = root.join("artworks").join("artifact.jpg");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"original").unwrap();
        let expected = sha256_file(&stored).unwrap();
        let mut connection = storage::open(&root).unwrap();
        let transaction = connection.transaction().unwrap();
        let id = enqueue_repository_file_with_hash(
            &transaction,
            "artworks/artifact.jpg",
            &expected,
            "test",
        )
        .unwrap();
        transaction.commit().unwrap();
        fs::write(&stored, b"changed").unwrap();

        let failed = run(&root, std::slice::from_ref(&id)).unwrap();
        assert_eq!(failed.failures.len(), 1);
        assert!(stored.is_file());

        fs::write(&stored, b"original").unwrap();
        let retried = run(&root, &[id]).unwrap();
        assert!(retried.failures.is_empty());
        assert!(!stored.exists());
    }

    #[test]
    fn cleanup_keeps_a_file_that_is_referenced_by_database_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored = root.join("artworks").join("artifact.jpg");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"artifact").unwrap();
        let expected = sha256_file(&stored).unwrap();
        let mut connection = storage::open(&root).unwrap();
        let transaction = connection.transaction().unwrap();
        let id = enqueue_repository_file_with_hash(
            &transaction,
            "artworks/artifact.jpg",
            &expected,
            "test",
        )
        .unwrap();
        transaction.commit().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = OFF;
                INSERT INTO final_artifacts
                  (id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms)
                VALUES ('artifact', 'branch', 'history', 'artworks/artifact.jpg',
                        '0000000000000000000000000000000000000000000000000000000000000000',
                        'image/jpeg', 8, 0);")
            .unwrap();

        let report = run(&root, std::slice::from_ref(&id)).unwrap();
        assert_eq!(report.failures.len(), 1);
        assert!(stored.is_file());
        assert_eq!(report.pending_count, 1);
    }

    #[test]
    fn list_pending_reports_queue_state_and_last_failure() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored = root.join("artworks").join("kept.bin");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"kept").unwrap();
        let expected = sha256_file(&stored).unwrap();
        let (kept_id, removed_id) = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let kept = enqueue_repository_file_with_hash(
                &transaction,
                "artworks/kept.bin",
                &expected,
                "history_commit_release",
            )
            .unwrap();
            let removed =
                enqueue_repository_file(&transaction, "artworks/gone.bin", "pin_board_finalize")
                    .unwrap();
            transaction.commit().unwrap();
            (kept, removed)
        };
        insert_history_node(&root, "artworks/kept.bin");

        // 一条仍被引用（重放失败并记录 last_error），一条已不存在（重放成功删除）。
        let report = run(&root, &[]).unwrap();
        assert_eq!(report.cleaned_count, 1);
        assert_eq!(report.failures.len(), 1);

        let pending = list_pending(&root).unwrap();
        assert_eq!(pending.len(), 1);
        let entry = &pending[0];
        assert_eq!(entry.id, kept_id);
        assert_ne!(entry.id, removed_id);
        assert_eq!(entry.path, "artworks/kept.bin");
        assert_eq!(entry.path_kind, REPOSITORY_FILE);
        assert_eq!(entry.reason, "history_commit_release");
        assert!(entry.created_ms > 0);
        assert!(entry.last_attempt_ms.is_some());
        assert!(entry.last_error.as_deref().unwrap().contains("仍被"));
    }

    #[test]
    fn committed_cleanup_intent_recovers_after_restart_before_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored = root.join("artworks").join("orphan.bin");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"orphan").unwrap();
        let expected = sha256_file(&stored).unwrap();
        let id = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let id = enqueue_repository_file_with_hash(
                &transaction,
                "artworks/orphan.bin",
                &expected,
                "crash-window-before-delete",
            )
            .unwrap();
            transaction.commit().unwrap();
            id
        };

        let report = run(&root, &[]).unwrap();

        assert_eq!(report.cleaned_count, 1);
        assert_eq!(report.pending_count, 0);
        assert!(!stored.exists());
        assert!(!id.is_empty());
    }

    #[test]
    fn cleanup_replay_is_idempotent_after_deletion_before_intent_removal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored = root.join("artworks").join("already-removed.bin");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"orphan").unwrap();
        let expected = sha256_file(&stored).unwrap();
        let id = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let id = enqueue_repository_file_with_hash(
                &transaction,
                "artworks/already-removed.bin",
                &expected,
                "crash-window-after-delete",
            )
            .unwrap();
            transaction.commit().unwrap();
            id
        };
        fs::remove_file(&stored).unwrap();

        let report = run(&root, &[id]).unwrap();

        assert_eq!(report.cleaned_count, 1);
        assert_eq!(report.pending_count, 0);
        assert!(report.failures.is_empty());
    }

    fn pending_count(root: &Path) -> i64 {
        storage::open(root)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM pending_file_cleanup", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn insert_history_node(root: &Path, snapshot_path: &str) {
        storage::open(root)
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO history_nodes
                   (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind,
                    created_ms, logical_size, chunk_file_size, sha256, chunk_count, snapshot_path)
                 VALUES ('node', 'artwork', 'branch', NULL, 'node', '', 'manual', 0, 1, 1,
                         '0000000000000000000000000000000000000000000000000000000000000000', 1,
                         '{snapshot_path}');"
            ))
            .unwrap();
    }

    #[test]
    fn released_repository_files_skip_still_referenced_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let artworks = root.join("artworks");
        fs::create_dir_all(&artworks).unwrap();
        fs::write(artworks.join("kept.lbc"), b"kept").unwrap();
        fs::write(artworks.join("released.lbc"), b"released").unwrap();
        insert_history_node(&root, "artworks/kept.lbc");

        let ids = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let ids = enqueue_released_repository_files(
                &transaction,
                &[
                    "artworks/kept.lbc".to_owned(),
                    "artworks/released.lbc".to_owned(),
                ],
                "test",
            )
            .unwrap();
            transaction.commit().unwrap();
            ids
        };

        // 仍被历史节点引用的路径不入队，只有已无引用的文件进入队列。
        assert_eq!(ids.len(), 1);
        let report = run(&root, &ids).unwrap();
        assert!(report.failures.is_empty());
        assert!(artworks.join("kept.lbc").is_file());
        assert!(!artworks.join("released.lbc").exists());
    }

    #[test]
    fn released_file_stays_queued_while_referenced_and_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let stored = root.join("artworks").join("released.lbc");
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, b"released").unwrap();
        let ids = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let ids = enqueue_released_repository_files(
                &transaction,
                &["artworks/released.lbc".to_owned()],
                "test",
            )
            .unwrap();
            transaction.commit().unwrap();
            ids
        };
        assert_eq!(ids.len(), 1);
        insert_history_node(&root, "artworks/released.lbc");

        // 重放被引用检查拒绝：条目留在队列可重试，文件保留。
        replay(&root, &ids);
        assert_eq!(pending_count(&root), 1);
        assert!(stored.is_file());

        // 引用消失后同一批 id 再次重放即删除并清空队列。
        storage::open(&root)
            .unwrap()
            .execute("DELETE FROM history_nodes WHERE id = 'node'", [])
            .unwrap();
        replay(&root, &ids);
        assert_eq!(pending_count(&root), 0);
        assert!(!stored.exists());
    }

    /// 把文件的修改时间回拨 1 小时，使其越过扫描宽限期。
    fn age_file(path: &Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3_600);
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    fn new_artwork_directory(root: &Path) -> PathBuf {
        root.join("artworks").join(storage::new_id())
    }

    #[test]
    fn scan_reports_unreferenced_history_files_and_keeps_referenced_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let artwork = new_artwork_directory(&root);
        let snapshots = artwork.join("snapshots");
        let deltas = artwork.join("deltas");
        fs::create_dir_all(&snapshots).unwrap();
        fs::create_dir_all(&deltas).unwrap();

        let orphan_snapshot = snapshots.join(format!("{}.lbc", storage::new_id()));
        let orphan_delta = deltas.join(format!(
            "{}-to-{}.lbd",
            storage::new_id(),
            storage::new_id()
        ));
        let referenced = snapshots.join(format!("{}.lbc", storage::new_id()));
        let unmatched = snapshots.join("notes.txt");
        for path in [&orphan_snapshot, &orphan_delta, &referenced, &unmatched] {
            fs::write(path, b"payload").unwrap();
            age_file(path);
        }
        insert_history_node(&root, &storage::relative_path(&root, &referenced).unwrap());

        let candidates = scan_unreferenced(&root, || false, |_, _| {}).unwrap();
        let paths = candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();

        // 只报告孤儿 snapshot 与孤儿 delta：被引用的保留、命名不匹配的不报告。
        assert_eq!(candidates.len(), 2);
        assert!(paths.contains(&storage::relative_path(&root, &orphan_snapshot).unwrap()));
        assert!(paths.contains(&storage::relative_path(&root, &orphan_delta).unwrap()));
        assert!(!paths.contains(&storage::relative_path(&root, &referenced).unwrap()));
        assert!(!paths.iter().any(|path| path.ends_with("notes.txt")));
    }

    #[test]
    fn scan_skips_files_within_the_grace_period() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let snapshots = new_artwork_directory(&root).join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        // 刚写入的文件修改时间落在宽限期内，可能仍属于进行中的提交/精简。
        let fresh = snapshots.join(format!("{}.lbc", storage::new_id()));
        fs::write(&fresh, b"fresh").unwrap();

        assert!(scan_unreferenced(&root, || false, |_, _| {})
            .unwrap()
            .is_empty());
    }

    #[test]
    fn confirmed_cleanup_removes_candidates_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let snapshots = new_artwork_directory(&root).join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        let orphan = snapshots.join(format!("{}.lbc", storage::new_id()));
        fs::write(&orphan, b"orphan").unwrap();
        age_file(&orphan);

        let candidates = scan_unreferenced(&root, || false, |_, _| {}).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].byte_size, 6);
        let paths = candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();

        let report = cleanup_unreferenced(&root, &paths).unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.cleaned_count, 1);
        assert_eq!(report.pending_count, 0);
        assert!(!orphan.exists());

        // 幂等：再次确认清理不报错、不再删除、队列仍为空。
        let second = cleanup_unreferenced(&root, &paths).unwrap();
        assert!(second.failures.is_empty());
        assert_eq!(second.cleaned_count, 0);
        assert_eq!(second.pending_count, 0);

        // 再次扫描不再有候选。
        assert!(scan_unreferenced(&root, || false, |_, _| {})
            .unwrap()
            .is_empty());
    }

    /// 造一条画板图片记录（`pin_board_images`），供画板 DDS 引用检查使用。
    fn insert_pin_board_image(root: &Path, artwork_id: &str, board_id: i64, image_id: i64) {
        storage::open(root)
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES ({board_id}, '{artwork_id}', 'board', 0, 0, 0, '{}', 0, 0);
                 INSERT INTO pin_board_images
                   (id, board_id, file_path, width, height, created_ms)
                 VALUES ({image_id}, {board_id}, '{image_id}.dds', 4, 4, 0);",
                "0".repeat(64)
            ))
            .unwrap();
    }

    #[test]
    fn scan_reports_orphan_board_dds_and_keeps_referenced_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let artwork_id = storage::new_id();
        let board_id = 7_i64;
        let board = root
            .join("artworks")
            .join(&artwork_id)
            .join(BOARD_DIRECTORY)
            .join(board_id.to_string());
        fs::create_dir_all(&board).unwrap();

        let referenced = board.join("3.dds");
        let orphan = board.join("9.dds");
        for path in [&referenced, &orphan] {
            fs::write(path, b"payload").unwrap();
            age_file(path);
        }
        insert_pin_board_image(&root, &artwork_id, board_id, 3);

        let candidates = scan_unreferenced(&root, || false, |_, _| {}).unwrap();
        let paths = candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();

        // 只有无记录的 DDS 被报告；仍被 pin_board_images 引用的保留。
        assert_eq!(candidates.len(), 1);
        assert!(paths.contains(&storage::relative_path(&root, &orphan).unwrap()));
        assert!(!paths.contains(&storage::relative_path(&root, &referenced).unwrap()));

        // 确认清理删除孤儿、保留被引用文件。
        let report = cleanup_unreferenced(&root, &paths).unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.cleaned_count, 1);
        assert!(!orphan.exists());
        assert!(referenced.is_file());
    }
}
