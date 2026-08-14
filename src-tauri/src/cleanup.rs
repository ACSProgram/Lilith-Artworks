use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use rusqlite::{params, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::storage;

const REPOSITORY_FILE: &str = "repository_file";
const REPOSITORY_DIRECTORY: &str = "repository_directory";
const EXTERNAL_FILE: &str = "external_file";

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
        match remove_entry(root, &entry) {
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
    Ok(CleanupReport {
        cleaned_count,
        pending_count: pending_count_with_connection(&connection)?,
        failures,
    })
}

fn remove_entry(root: &Path, entry: &PendingCleanup) -> Result<(), String> {
    match entry.path_kind.as_str() {
        REPOSITORY_FILE => remove_repository_file(root, &entry.path),
        REPOSITORY_DIRECTORY => remove_repository_directory(root, &entry.path),
        EXTERNAL_FILE => remove_external_file(&entry.path, entry.expected_sha256.as_deref()),
        _ => Err("待清理条目的路径类型无效".into()),
    }
}

fn remove_repository_file(root: &Path, relative: &str) -> Result<(), String> {
    let path = safe_repository_path(root, relative)?;
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
}
