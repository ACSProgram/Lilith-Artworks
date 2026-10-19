use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    cancel::CancelToken,
    contracts::{reference_inventories, ScanCandidateFile},
    storage,
};

const REPOSITORY_FILE: &str = "repository_file";
const REPOSITORY_DIRECTORY: &str = "repository_directory";
const EXTERNAL_FILE: &str = "external_file";

/// 每批清理的条目数。256–1000 是计划允许的区间；512 在「每批一次事务」与
/// 「单个文件删除失败的可重试粒度」之间取中，也远低于 SQLite 的绑定参数上限
/// （anti-join 每批最多用 `CLEANUP_BATCH_SIZE` 个占位符）。
const CLEANUP_BATCH_SIZE: usize = 512;

/// 未引用文件扫描的宽限期：修改时间晚于 `now - 30 分钟` 的文件视为可能仍属于
/// 进行中的提交、精简或检查点发布，扫描跳过它们。取值明显大于空闲链路校验的
/// 10 分钟延迟，给长操作留出余量。
pub(crate) const SCAN_GRACE_MS: i64 = 30 * 60 * 1000;

/// 仓库根 `temp/` 中认证预览缓存会话目录的命名前缀，由
/// `authenticity::pipeline::preview_cache_directory` 写入。
const PREVIEW_SESSION_PREFIX: &str = "authenticity-preview-";
/// `tempfile::NamedTempFile` 的默认文件名前缀（与 `history::contracts` 的
/// 暂存文件命名口径一致）；仓库根 `temp/` 下的崩溃残留只认它。
const SCRATCH_FILE_PREFIX: &str = ".tmp";

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
///
/// `expected_sha256` 与 `artwork_title` 服务于外部文件条目的**逐条确认**：删除前
/// 用户需要看到完整路径、所属作品与期望摘要，才能判断这条记录是不是自己预期的
/// 那次清理。所属作品按导出路径回查认证记录，查不到时为空（例如发布在建记录之前
/// 就失败）。
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
    pub(crate) expected_sha256: Option<String>,
    pub(crate) artwork_title: Option<String>,
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
    // 一次收集全部引用，再逐个路径判定：避免「每释放一个文件查五张表」。
    let referenced = reference_inventories()
        .collect_paths(transaction)
        .map_err(|error| format!("无法读取仓库引用清单：{error}"))?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for path in paths {
        let normalized = path.trim();
        if !seen.insert(normalized) {
            continue;
        }
        if !referenced.contains_key(normalized) {
            ids.push(enqueue_repository_file(transaction, normalized, reason)?);
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
            "SELECT p.id, p.path, p.path_kind, p.reason, p.created_ms, p.last_attempt_ms,
                    p.last_error, p.expected_sha256,
                    (SELECT n.title
                       FROM certification_records r
                       JOIN branches b ON b.id = r.branch_id
                       JOIN library_nodes n ON n.id = b.artwork_id
                      WHERE r.output_path = p.path
                      LIMIT 1)
             FROM pending_file_cleanup p ORDER BY p.created_ms, p.id",
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
                expected_sha256: row.get(7)?,
                artwork_title: row.get(8)?,
            })
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(entries)
}

/// 执行待清理队列。
///
/// `requested_ids` 为空表示「重放仓库内条目」：仓库文件与仓库目录会被处理，
/// **外部文件条目一律跳过**。外部条目保存的是仓库之外的绝对路径，删除它等于一条
/// 任意路径删除；因此它只在用户于设置页逐条确认后，以**显式 id** 才会执行。
/// 启动时的自动重放与设置页的「全部重试」都走空 id 这条路径，所以被投毒的仓库
/// 无法借启动流程删除任意文件。
///
/// 引用复查使用连接内 `TEMP` 引用表与 anti-join：引用清单由领域 provider 提供，
/// 本模块不再认识画板路径布局；结果按 [`CLEANUP_BATCH_SIZE`] 分批提交，每批可取消
/// 且幂等。
pub(crate) fn run(root: &Path, requested_ids: &[String]) -> Result<CleanupReport, String> {
    run_internal(root, requested_ids, &CancelToken::standalone())
}

/// 与 [`run`] 相同，但允许在批量之间取消；已处理的批次仍会提交，剩余条目留在
/// 队列，下一次重放幂等继续。
pub(crate) fn run_cancellable(
    root: &Path,
    requested_ids: &[String],
    cancel: &CancelToken,
) -> Result<CleanupReport, String> {
    run_internal(root, requested_ids, cancel)
}

fn run_internal(
    root: &Path,
    requested_ids: &[String],
    cancel: &CancelToken,
) -> Result<CleanupReport, String> {
    let requested = requested_ids.iter().cloned().collect::<HashSet<_>>();
    let explicit = !requested.is_empty();
    let mut connection = storage::open(root)?;
    let entries = load_pending_entries(&connection, &requested, explicit)?;
    if entries.is_empty() {
        // 队列为空时不读取引用清单：启动时的自动重放与「全部重试」常常无事可做，
        // 整份清单在大型仓库上并不便宜。
        return Ok(CleanupReport {
            cleaned_count: 0,
            pending_count: pending_count_with_connection(&connection)?,
            failures: Vec::new(),
        });
    }
    populate_reference_table(&mut connection)?;

    let mut cleaned_count = 0;
    let mut failures = Vec::new();
    let mut cancelled = false;
    for batch in entries.chunks(CLEANUP_BATCH_SIZE) {
        let exact_paths = batch
            .iter()
            .filter(|entry| entry.path_kind != REPOSITORY_DIRECTORY)
            .map(|entry| entry.path.trim())
            .collect::<Vec<_>>();
        let referenced = referenced_exact_batch(&connection, &exact_paths)?;
        let mut outcomes = Vec::with_capacity(batch.len());
        for entry in batch {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            let reference = if entry.path_kind == REPOSITORY_DIRECTORY {
                referenced_directory(&connection, &entry.path)?
            } else {
                referenced.get(entry.path.trim()).cloned()
            };
            let result = match reference {
                Some(owner) => Err(format!("文件仍被 {owner} 引用，已保留")),
                None => remove_entry(root, entry),
            };
            outcomes.push((entry.id.clone(), entry.path.clone(), result));
        }
        commit_batch(
            &mut connection,
            &outcomes,
            &mut cleaned_count,
            &mut failures,
        )?;
        if cancelled {
            break;
        }
    }

    let pending_count = pending_count_with_connection(&connection)?;
    log::info!(
        "file cleanup finished: cleaned={cleaned_count}, failed={}, pending={pending_count}, cancelled={cancelled}",
        failures.len(),
    );
    if cancelled {
        // 已提交的批次不回滚：报告本次实际清理数量，剩余条目留给下一次重放。
        return Err(format!(
            "清理已取消（本次已清理 {cleaned_count} 条，队列剩余 {pending_count} 条）"
        ));
    }
    Ok(CleanupReport {
        cleaned_count,
        pending_count,
        failures,
    })
}

fn load_pending_entries(
    connection: &Connection,
    requested: &HashSet<String>,
    explicit: bool,
) -> Result<Vec<PendingCleanup>, String> {
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
    Ok(entries
        .into_iter()
        .filter(|entry| {
            if explicit {
                requested.contains(&entry.id)
            } else {
                entry.path_kind != EXTERNAL_FILE
            }
        })
        .collect())
}

/// 把领域 provider 提供的引用清单放进连接内临时表。表随连接结束消失，不产生
/// 持久 schema 变更；持久路径索引留给阶段 7 的 v4 → v5 迁移。
fn populate_reference_table(connection: &mut Connection) -> Result<(), String> {
    let targets = reference_inventories()
        .collect_targets(connection)
        .map_err(|error| format!("无法读取仓库引用清单：{error}"))?;
    connection
        .execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS cleanup_references (
               path TEXT PRIMARY KEY,
               owner TEXT NOT NULL
             ) WITHOUT ROWID;
             DELETE FROM cleanup_references;",
        )
        .map_err(storage::database_error)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    {
        let mut statement = transaction
            .prepare("INSERT OR IGNORE INTO cleanup_references (path, owner) VALUES (?1, ?2)")
            .map_err(storage::database_error)?;
        for target in targets {
            statement
                .execute(params![target.path, target.owner.label()])
                .map_err(storage::database_error)?;
        }
    }
    transaction.commit().map_err(storage::database_error)
}

/// 一批文件路径的 anti-join：一次查询返回仍被引用的子集。
fn referenced_exact_batch(
    connection: &Connection,
    paths: &[&str],
) -> Result<HashMap<String, String>, String> {
    let mut unique = paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    unique.sort_unstable();
    if unique.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(unique.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT path, owner FROM cleanup_references WHERE path IN ({placeholders})");
    let mut statement = connection.prepare(&sql).map_err(storage::database_error)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(unique.iter().copied()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(rows.into_iter().collect())
}

/// 目录条目的引用检查：任意受管理路径位于该目录下即视为仍被引用。
fn referenced_directory(connection: &Connection, path: &str) -> Result<Option<String>, String> {
    use rusqlite::OptionalExtension;
    let prefix = format!(
        "{}/%",
        escape_like(path.trim().trim_end_matches(['/', '\\']))
    );
    connection
        .query_row(
            "SELECT owner FROM cleanup_references WHERE path LIKE ?1 ESCAPE '\\' LIMIT 1",
            [prefix],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn commit_batch(
    connection: &mut Connection,
    outcomes: &[(String, String, Result<(), String>)],
    cleaned_count: &mut usize,
    failures: &mut Vec<CleanupFailure>,
) -> Result<(), String> {
    if outcomes.is_empty() {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let now = storage::now_ms()?;
    for (id, path, result) in outcomes {
        match result {
            Ok(()) => {
                transaction
                    .execute("DELETE FROM pending_file_cleanup WHERE id = ?1", [id])
                    .map_err(storage::database_error)?;
                *cleaned_count += 1;
            }
            Err(error) => {
                transaction
                    .execute(
                        "UPDATE pending_file_cleanup
                         SET last_attempt_ms = ?2, last_error = ?3 WHERE id = ?1",
                        params![id, now, error],
                    )
                    .map_err(storage::database_error)?;
                failures.push(CleanupFailure {
                    id: id.clone(),
                    path: path.clone(),
                    error: error.clone(),
                });
            }
        }
    }
    transaction.commit().map_err(storage::database_error)
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

/// 一次仓库临时目录清扫的结果：`reclaimed` 为成功回收的条目数，`failed` 为删除失败数。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TempSweepReport {
    pub(crate) reclaimed: u64,
    pub(crate) failed: u64,
}

/// 清扫仓库根 `temp/` 中超期的预览会话目录与暂存文件。
///
/// `temp/` 不进入任何引用清单，因此这里直接删除而不走 `pending_file_cleanup`：
/// 预览缓存丢失只让下一次预览重算一次；`.tmp*` 暂存文件是备份/恢复/检查点的
/// `NamedTempFile` 在进程被强杀或断电时的残留（正常路径由 `Drop` 删除）。只处理
/// 两类名字——`authenticity-preview-<UUID>` 会话目录与 `.tmp` 前缀的普通文件——
/// 手工放置的其它条目一律不动。修改时间晚于 `now - SCAN_GRACE_MS` 的条目视为
/// 可能仍属于进行中的预览或链解析（本进程或另一进程），跳过；
/// `active_preview_session` 是本进程当前的预览会话 id，其目录即使超期也保留，
/// 避免清掉正在使用的缓存。只碰 `temp/` 顶层、不递归匹配、不跟随符号链接；
/// 删除失败只计数并写日志，不改变调用方的打开流程；目录不可读时按空处理。
pub(crate) fn sweep_repository_temp(
    root: &Path,
    active_preview_session: Option<&str>,
) -> TempSweepReport {
    let now_ms = match storage::now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => {
            log::warn!("无法读取当前时间，跳过仓库临时目录清扫：{error}");
            return TempSweepReport::default();
        }
    };
    sweep_repository_temp_at(root, active_preview_session, now_ms)
}

fn sweep_repository_temp_at(
    root: &Path,
    active_preview_session: Option<&str>,
    now_ms: i64,
) -> TempSweepReport {
    let mut sweep = TempSweepReport::default();
    let cutoff_ms = now_ms.saturating_sub(SCAN_GRACE_MS);
    let temp_directory = root.join("temp");
    let entries = match fs::read_dir(&temp_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return sweep,
        Err(error) => {
            log::warn!(
                "无法读取仓库临时目录 {}，跳过清扫：{error}",
                storage::display_path(&temp_directory)
            );
            return sweep;
        }
    };
    let active_session_directory =
        active_preview_session.map(|session| format!("{PREVIEW_SESSION_PREFIX}{session}"));
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        // `symlink_metadata` 不跟随符号链接：指向目录的链接按链接本身判定，
        // `is_dir` 为假，不会递归删除链接目标。
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let is_directory = metadata.is_dir();
        let remove = if is_preview_session_name(name) {
            is_directory && active_session_directory.as_deref() != Some(name)
        } else if is_scratch_file_name(name) {
            metadata.is_file()
        } else {
            continue;
        };
        if !remove {
            continue;
        }
        let Some(modified_ms) = modified_ms(&metadata) else {
            continue;
        };
        if modified_ms >= cutoff_ms {
            continue;
        }
        let result = if is_directory {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        match result {
            Ok(()) => sweep.reclaimed += 1,
            Err(error) => {
                sweep.failed += 1;
                log::warn!(
                    "无法回收仓库临时目录中的残留 {}：{error}",
                    storage::display_path(&entry.path())
                );
            }
        }
    }
    sweep
}

/// 预览会话目录命名：`authenticity-preview-<UUID v4>`；后缀不是合法 UUID 的
/// 目录视为手工放置的条目，不清理。
fn is_preview_session_name(name: &str) -> bool {
    name.strip_prefix(PREVIEW_SESSION_PREFIX)
        .is_some_and(|session| uuid::Uuid::parse_str(session).is_ok())
}

fn is_scratch_file_name(name: &str) -> bool {
    name.strip_prefix(SCRATCH_FILE_PREFIX)
        .is_some_and(|suffix| !suffix.is_empty())
}

fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

/// 扫描 `artworks/*/` 下由各领域 provider 声明的受管理文件（历史 snapshot/delta
/// 与画板 DDS 等未引用崩溃孤儿、手工复制或历史迁移遗留），作为清理账本的发现
/// 机制。只报告、不删除；确认清理由 `cleanup_unreferenced` 完成。
///
/// 护栏：只考虑 provider 报告的命名模式与目录；只报告修改时间早于宽限期
/// （`SCAN_GRACE_MS`）的文件，避免与进行中的提交、精简、检查点发布或画板写入
/// 赛跑；引用判定一次读取完整引用清单，不再逐文件查五张表。调用方须在仓库操作
/// 锁内运行本函数（与调度器、前台长命令互斥），本函数自身不加锁。
pub(crate) fn scan_unreferenced(
    root: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(u64, u64),
) -> Result<Vec<ScanCandidate>, String> {
    let connection = storage::open(root)?;
    let cutoff = storage::now_ms()?.saturating_sub(SCAN_GRACE_MS);
    let referenced = reference_inventories()
        .collect_paths(&connection)
        .map_err(|error| format!("无法读取仓库引用清单：{error}"))?;
    let mut files = Vec::new();
    collect_scan_files(root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let total = files.len() as u64;
    let mut candidates = Vec::new();
    for (index, file) in files.into_iter().enumerate() {
        if cancelled() {
            return Err("未引用文件扫描已取消".into());
        }
        let metadata = match fs::metadata(&file.path) {
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
        let relative = storage::relative_path(root, &file.path)?;
        if !referenced.contains_key(&relative) {
            candidates.push(ScanCandidate {
                path: relative,
                byte_size: metadata.len(),
                reason: file.reason.to_owned(),
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
#[cfg(test)]
pub(crate) fn cleanup_unreferenced(root: &Path, paths: &[String]) -> Result<CleanupReport, String> {
    cleanup_unreferenced_cancellable(root, paths, &CancelToken::standalone())
}

pub(crate) fn cleanup_unreferenced_cancellable(
    root: &Path,
    paths: &[String],
    cancel: &CancelToken,
) -> Result<CleanupReport, String> {
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
    run_cancellable(root, &cleanup_ids, cancel)
}

/// 收集扫描范围内的候选文件。每个 Artwork 的实际布局由领域 provider 自己解释，
/// 本模块只负责遍历作品目录与统一宽限期/引用判定。目录缺失（如新仓库尚无任何
/// 作品）按空处理。
fn collect_scan_files(root: &Path, files: &mut Vec<ScanCandidateFile>) -> Result<(), String> {
    let artworks = root.join("artworks");
    let entries = match fs::read_dir(&artworks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("无法读取作品目录：{error}")),
    };
    let inventories = reference_inventories();
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取作品目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取作品目录项类型：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        inventories.scan_candidates(&entry.path(), files)?;
    }
    Ok(())
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
    fn automatic_replay_never_touches_external_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let output = directory.path().join("output.jpg");
        let stored = root.join("artworks").join("artifact.jpg");
        crate::library::initialize(&root).unwrap();
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&output, b"original").unwrap();
        fs::write(&stored, b"original").unwrap();
        let external_id = enqueue_external(&root, &output, &sha256_file(&output).unwrap());
        let mut connection = storage::open(&root).unwrap();
        let transaction = connection.transaction().unwrap();
        let repository_id =
            enqueue_repository_file(&transaction, "artworks/artifact.jpg", "test").unwrap();
        transaction.commit().unwrap();
        drop(connection);

        // 空 id 是「重放仓库内条目」：应用启动与设置页「全部重试」都走这条路，
        // 外部条目必须原样留在队列里，只能凭显式 id 删除。
        let report = run(&root, &[]).unwrap();

        assert_eq!(report.cleaned_count, 1);
        assert!(report.failures.is_empty());
        assert!(!stored.exists());
        assert!(output.is_file());
        assert_eq!(report.pending_count, 1);
        let remaining = list_pending(&root).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, external_id);
        assert_ne!(remaining[0].id, repository_id);
        assert_eq!(remaining[0].path_kind, EXTERNAL_FILE);
        assert!(remaining[0].expected_sha256.is_some());

        // 显式 id 才是外部条目的执行入口（用户逐条确认后）。
        let explicit = run(&root, std::slice::from_ref(&external_id)).unwrap();
        assert_eq!(explicit.cleaned_count, 1);
        assert!(!output.exists());
        assert_eq!(explicit.pending_count, 0);
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

    /// 目录无法用 `OpenOptions` 打开改时间，单测用 `filetime` 拨回宽限期之前。
    fn age_directory(path: &Path) {
        let old = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(3_600),
        );
        filetime::set_file_mtime(path, old).unwrap();
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

    /// 备份/恢复崩溃后留在作品 `temp/` 的 `.tmp*` 残骸没有任何引用，必须由扫描
    /// 报告出来，否则用户既看不到也收不回来。
    #[test]
    fn scan_reports_crash_residue_in_the_artwork_temp_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let temp = new_artwork_directory(&root).join("temp");
        fs::create_dir_all(&temp).unwrap();
        let residue = temp.join(".tmpd0BGy8");
        fs::write(&residue, b"scratch").unwrap();
        age_file(&residue);

        let candidates = scan_unreferenced(&root, || false, |_, _| {}).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].path,
            storage::relative_path(&root, &residue).unwrap()
        );
        assert_eq!(candidates[0].reason, "备份或恢复中断后残留的临时文件");
        assert_eq!(candidates[0].byte_size, 7);
    }

    /// 仓库 `temp/` 清扫只回收超期的预览会话目录与 `.tmp*` 暂存文件：当前会话、
    /// 宽限期内、命名不符与手工放置的条目一律保留。
    #[test]
    fn sweep_removes_expired_temp_entries_and_keeps_active_or_foreign_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let temp = root.join("temp");
        fs::create_dir_all(&temp).unwrap();

        let active_session = storage::new_id();
        let expired_session = temp.join(format!("authenticity-preview-{}", storage::new_id()));
        let active_directory = temp.join(format!("authenticity-preview-{active_session}"));
        let fresh_session = temp.join(format!("authenticity-preview-{}", storage::new_id()));
        let foreign_directory = temp.join("authenticity-preview-not-a-uuid");
        for path in [
            &expired_session,
            &active_directory,
            &fresh_session,
            &foreign_directory,
        ] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("token.jpg"), b"jpeg").unwrap();
        }
        age_directory(&expired_session);
        age_directory(&active_directory);
        age_directory(&foreign_directory);

        let expired_scratch = temp.join(".tmpd0BGy8");
        let fresh_scratch = temp.join(".tmpAbC123");
        let unrelated = temp.join("notes.txt");
        for path in [&expired_scratch, &fresh_scratch, &unrelated] {
            fs::write(path, b"scratch").unwrap();
        }
        age_file(&expired_scratch);
        age_file(&unrelated);

        let sweep =
            sweep_repository_temp_at(&root, Some(&active_session), storage::now_ms().unwrap());
        assert_eq!(sweep.reclaimed, 2);
        assert_eq!(sweep.failed, 0);
        assert!(!expired_session.exists());
        assert!(!expired_scratch.exists());
        // 当前会话、宽限期内、命名不符与手工放置的条目全部保留。
        assert!(active_directory.exists());
        assert!(fresh_session.exists());
        assert!(foreign_directory.exists());
        assert!(fresh_scratch.exists());
        assert!(unrelated.exists());

        // 没有 temp 目录时按空处理，不报错。
        let other_root = directory.path().join("other");
        let empty = sweep_repository_temp_at(&other_root, None, storage::now_ms().unwrap());
        assert_eq!(empty.reclaimed, 0);
        assert_eq!(empty.failed, 0);
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

    /// 造一条画板位图条目（`pin_board_items` + `pin_board_bitmaps`），供画板
    /// DDS 引用检查使用。
    fn insert_pin_board_image(root: &Path, artwork_id: &str, board_id: i64, item_id: i64) {
        storage::open(root)
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES ({board_id}, '{artwork_id}', 'board', 0, 0, 0, '{}', 0, 0);
                 INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                 VALUES ({item_id}, {board_id}, 0, 0);
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                 VALUES ({item_id}, '{item_id}.dds', 4, 4);",
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
            .join("boards")
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

        // 只有无记录的 DDS 被报告；仍被 pin_board_bitmaps 引用的保留。
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

    fn enqueue_missing_files(root: &Path, count: usize) {
        let mut connection = storage::open(root).unwrap();
        let transaction = connection.transaction().unwrap();
        for index in 0..count {
            enqueue_repository_file(
                &transaction,
                &format!("artworks/missing-{index}.lbc"),
                "test",
            )
            .unwrap();
        }
        transaction.commit().unwrap();
    }

    #[test]
    fn cleanup_processes_large_queues_in_batches() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        enqueue_missing_files(&root, 1100);

        let report = run(&root, &[]).unwrap();

        assert_eq!(report.cleaned_count, 1100);
        assert_eq!(report.pending_count, 0);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn cancelled_cleanup_commits_processed_batches_and_keeps_the_rest() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        enqueue_missing_files(&root, 1100);
        let checks = Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::with_source({
            let checks = checks.clone();
            move || checks.fetch_add(1, Ordering::SeqCst) >= 600
        });

        let error = run_cancellable(&root, &[], &cancel).unwrap_err();

        assert!(error.contains("已取消"), "{error}");
        // 第一批 512 条与第二批已处理的 88 条提交，其余留在队列可幂等重试。
        assert_eq!(list_pending(&root).unwrap().len(), 500);
    }

    #[test]
    fn cleanup_keeps_a_directory_with_a_referenced_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let shared = root.join("artworks").join("shared");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("snapshot.lbc"), b"payload").unwrap();
        insert_history_node(&root, "artworks/shared/snapshot.lbc");
        let id = {
            let mut connection = storage::open(&root).unwrap();
            let transaction = connection.transaction().unwrap();
            let id = enqueue_repository_directory(&transaction, "artworks/shared", "test").unwrap();
            transaction.commit().unwrap();
            id
        };

        let report = run(&root, &[id]).unwrap();

        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].error.contains("仍被"), "{report:?}");
        assert!(shared.is_dir());
        assert_eq!(report.pending_count, 1);
    }

    #[test]
    fn explicit_external_cleanup_keeps_a_path_referenced_by_a_certification_record() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let output = directory.path().join("output.jpg");
        crate::library::initialize(&root).unwrap();
        fs::write(&output, b"original").unwrap();
        let expected = sha256_file(&output).unwrap();
        let id = enqueue_external(&root, &output, &expected);
        {
            let connection = storage::open(&root).unwrap();
            connection
                .execute_batch("PRAGMA foreign_keys = OFF;")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO certification_records
                       (id, final_artifact_id, branch_id, history_id, watermark_id,
                        trustmark_enabled, output_path, stored_path, output_sha256,
                        output_bytes, title, creator, rights_statement,
                        authentication_content, regions_json, created_ms)
                     VALUES ('record', 'artifact', 'branch', 'history', NULL, 0, ?1,
                             'artworks/stored.jpg', ?2, 8, 't', 'c', 'r', 'a', '[]', 0)",
                    params![storage::display_path(&output), expected],
                )
                .unwrap();
        }

        let report = run(&root, &[id]).unwrap();

        assert_eq!(report.failures.len(), 1);
        assert!(
            report.failures[0].error.contains("认证导出记录"),
            "{report:?}"
        );
        assert!(output.is_file());
        assert_eq!(report.pending_count, 1);
    }
}
