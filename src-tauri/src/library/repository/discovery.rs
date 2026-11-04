//! 仓库发现、初始化与备用数据库迁移。
//!
//! 只负责确定「用哪个文件作为作品数据库」并把目录与 schema 带到可打开状态；
//! 作品树与回收站的事务在 `tree` / `trash` 中。

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{platform::path::display_path, storage};

use super::super::{schema, RepositoryPathState};

pub(crate) fn database_path(root: &Path) -> PathBuf {
    storage::database_path(root)
}

pub(crate) fn initialize(root: &Path) -> Result<(), String> {
    validate_repository_root(root)?;
    let existed = root.exists();
    if existed && !root.is_dir() {
        return Err("作品仓库路径不是目录".into());
    }
    if !existed {
        fs::create_dir_all(root).map_err(|error| format!("无法创建作品仓库：{error}"))?;
    }

    let database = locate_database(root)?;
    let create = !database.exists()
        || database
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(false);

    let mut connection = Connection::open(&database)
        .map_err(|error| format!("无法打开作品数据库 {}：{error}", display_path(&database)))?;
    storage::configure(&connection)?;
    if create {
        schema::create(&connection)?;
        create_directories(root)?;
    } else {
        schema::migrate(&mut connection)?;
        schema::validate(&connection)?;
        schema::validate_repository_semantics(&connection, root)?;
        create_directories(root)?;
    }
    Ok(())
}

pub(crate) fn open_existing(root: &Path) -> Result<(), String> {
    validate_existing_root(root)?;
    let database = locate_database(root)?;
    require_database_file(&database)?;

    let mut connection = storage::open(root)?;
    schema::migrate(&mut connection)?;
    schema::validate(&connection)?;
    schema::validate_repository_semantics(&connection, root)?;
    create_directories(root)
}

pub(crate) fn check_existing(root: &Path) -> Result<(), String> {
    validate_existing_root(root)?;
    let database = database_path(root);
    require_database_file(&database)?;
    let connection = storage::open(root)?;
    schema::validate_current(&connection)
}

/// 保存新仓库路径前的只读探测：不创建目录，不改名备用库，也不触碰 schema。
pub(crate) fn inspect_path(root: &Path) -> Result<RepositoryPathState, String> {
    validate_repository_root(root)?;
    let metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RepositoryPathState::Missing);
        }
        Err(error) => return Err(format!("无法读取作品仓库路径：{error}")),
    };
    if !metadata.is_dir() {
        return Ok(RepositoryPathState::NotDirectory);
    }
    if has_repository_database(root)? {
        return Ok(RepositoryPathState::Repository);
    }
    for entry in fs::read_dir(root).map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
            .path();
        if !is_runtime_artifact(root, &path) {
            return Ok(RepositoryPathState::NonRepositoryDirectory);
        }
    }
    Ok(RepositoryPathState::EmptyDirectory)
}

/// 标准文件名或任意备用文件名下是否已有一个可识别的作品数据库。只读打开，
/// 不执行 WAL 合并与改名——那些属于真正的打开流程。
fn has_repository_database(root: &Path) -> Result<bool, String> {
    let preferred = database_path(root);
    if preferred.is_file() && is_repository_database(&preferred) {
        return Ok(true);
    }
    for entry in fs::read_dir(root).map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
            .path();
        if path == preferred || is_runtime_artifact(root, &path) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("sqlite3")
            && is_repository_database(&path)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_repository_database(path: &Path) -> bool {
    let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return false;
    };
    let format = connection
        .query_row(
            "SELECT value FROM repository_meta WHERE key = 'format'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional();
    matches!(format, Ok(Some(format)) if format == schema::REPOSITORY_FORMAT)
}

fn validate_existing_root(root: &Path) -> Result<(), String> {
    validate_repository_root(root)?;
    if !root.exists() {
        return Err("已配置的作品仓库目录不存在".into());
    }
    if !root.is_dir() {
        return Err("作品仓库路径不是目录".into());
    }
    Ok(())
}

fn require_database_file(database: &Path) -> Result<(), String> {
    if !database.exists()
        || database
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(true)
    {
        return Err(format!(
            "已配置的作品仓库缺少数据库 {}，请重新选择仓库或从备份恢复",
            display_path(database)
        ));
    }
    Ok(())
}

fn locate_database(root: &Path) -> Result<PathBuf, String> {
    let preferred = database_path(root);
    if preferred.exists()
        && preferred
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
    {
        return Ok(preferred);
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| format!("无法读取作品仓库内容：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
            .path();
        // 跨进程 owner lock 的锁库也是 `.sqlite3`，但它不是作品数据库；初始化时
        // 它已经存在，不能参与备用数据库发现。
        if is_runtime_artifact(root, &path) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("sqlite3") {
            candidates.push(path);
        }
    }

    for candidate in candidates {
        if candidate == preferred {
            continue;
        }
        let matches_repository = {
            let connection =
                match Connection::open_with_flags(&candidate, OpenFlags::SQLITE_OPEN_READ_WRITE) {
                    Ok(connection) => connection,
                    Err(_) => continue,
                };
            let format = connection
                .query_row(
                    "SELECT value FROM repository_meta WHERE key = 'format'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional();
            let matches = format.ok().flatten().as_deref() == Some(schema::REPOSITORY_FORMAT);
            if matches {
                let busy: i64 = connection
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
                    .map_err(|error| {
                        format!(
                            "无法合并备用作品数据库 {} 的 WAL：{error}",
                            display_path(&candidate)
                        )
                    })?;
                if busy != 0 {
                    return Err(format!(
                        "备用作品数据库 {} 正在使用，无法完成 WAL 合并",
                        display_path(&candidate)
                    ));
                }
            }
            matches
        };
        if matches_repository {
            remove_sqlite_sidecars(&candidate)?;
            if preferred.exists()
                && preferred
                    .metadata()
                    .map(|metadata| metadata.len() == 0)
                    .unwrap_or(false)
            {
                fs::remove_file(&preferred)
                    .map_err(|error| format!("无法移除空数据库占位文件：{error}"))?;
                remove_sqlite_sidecars(&preferred)?;
            }
            fs::rename(&candidate, &preferred).map_err(|error| {
                format!(
                    "无法迁移已发现的作品数据库 {}：{error}",
                    display_path(&candidate)
                )
            })?;
            return Ok(preferred);
        }
    }

    if preferred.exists() {
        Ok(preferred)
    } else {
        let mut has_repository_content = false;
        for entry in fs::read_dir(root).map_err(|error| format!("无法读取作品仓库内容：{error}"))?
        {
            let entry = entry.map_err(|error| format!("无法读取作品仓库内容：{error}"))?;
            if !is_runtime_artifact(root, &entry.path()) {
                has_repository_content = true;
                break;
            }
        }
        if has_repository_content {
            Err("所选目录非空，且没有可读取的 Lilith Artworks 数据库".into())
        } else {
            Ok(preferred)
        }
    }
}

/// 仓库根目录下的锁库、锁日志与 owner 元数据是进程级运行时文件，不参与
/// 「是否为作品数据库 / 目录是否非空」的判定。
fn is_runtime_artifact(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    crate::repository_lock::is_runtime_artifact(relative)
}

fn remove_sqlite_sidecars(database: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path(database, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "无法清理 SQLite sidecar {}：{error}",
                    display_path(&sidecar)
                ))
            }
        }
    }
    Ok(())
}

fn sqlite_sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut value = database.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_repository_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("作品仓库必须使用绝对目录路径".into());
    }
    if root.parent().is_none() || root.file_name().is_none() {
        return Err("不能把磁盘或文件系统根目录用作作品仓库".into());
    }
    Ok(())
}

fn create_directories(root: &Path) -> Result<(), String> {
    for relative in ["artworks", "temp"] {
        fs::create_dir_all(root.join(relative))
            .map_err(|error| format!("无法创建仓库目录 {relative}：{error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{open, test_support::Fixture};
    use super::*;
    use rusqlite::params;

    #[test]
    fn opening_empty_configured_repository_does_not_reinitialize_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir(&root).unwrap();
        let database = database_path(&root);

        let error = open_existing(&root).unwrap_err();

        assert!(error.contains("缺少数据库"));
        assert!(!database.exists());
    }

    #[test]
    fn opening_repository_rejects_invalid_ids_and_traversal_paths() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "INSERT INTO pending_file_cleanup
                 (id, path_kind, path, reason, created_ms)
                 VALUES ('not-a-uuid', 'repository_file', 'artworks/file.bin', 'test', 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let id_error = open_existing(&fixture.root).unwrap_err();
        assert!(id_error.contains("不是有效 UUID"), "{id_error}");

        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "UPDATE pending_file_cleanup SET id = ?1, path = '../outside.bin'",
                [storage::new_id()],
            )
            .unwrap();
        drop(connection);

        let path_error = open_existing(&fixture.root).unwrap_err();
        assert!(path_error.contains("仓库存储路径无效"), "{path_error}");
    }

    /// 检查点父节点名下的边按设计登记空路径（`delta_path = ''`），表示该边没有
    /// 反向增量可读。空串不是路径，打开仓库时不能被当成非法相对路径拒绝。
    #[test]
    fn opening_repository_accepts_a_blank_edge_delta_path() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        let root_id = storage::new_id();
        let head_id = storage::new_id();
        let digest = "A".repeat(64);
        let artwork_directory = format!("artworks/{}", fixture.artwork.artwork_id);
        let nodes = [
            (
                root_id.clone(),
                None,
                format!("{artwork_directory}/snapshots/{root_id}.lbc"),
            ),
            (
                head_id.clone(),
                Some(root_id.clone()),
                format!("{artwork_directory}/snapshots/{head_id}.lbc"),
            ),
        ];
        for (id, parent_id, snapshot_path) in &nodes {
            connection
                .execute(
                    "INSERT INTO history_nodes
                     (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind,
                      is_checkpoint, created_ms, logical_size, chunk_file_size, sha256, chunk_count,
                      snapshot_path, delta_path)
                     VALUES (?1, ?2, ?3, ?4, '节点', '', 'manual', 1, 0, 10, 20, ?5, 1, ?6, NULL)",
                    params![
                        id,
                        fixture.artwork.artwork_id,
                        fixture.artwork.branch_id,
                        parent_id,
                        digest,
                        snapshot_path
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
                 VALUES (?1, ?2, '', 0)",
                params![head_id, root_id],
            )
            .unwrap();
        drop(connection);

        open_existing(&fixture.root).unwrap();

        // 跳过空串不等于放过越界路径：边上的真实非法路径仍要被拒绝。
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "UPDATE history_edges SET delta_path = '../outside.lbd' WHERE child_history_id = ?1",
                params![head_id],
            )
            .unwrap();
        drop(connection);

        let error = open_existing(&fixture.root).unwrap_err();
        assert!(error.contains("history_edges.delta_path 无效"), "{error}");
    }

    #[test]
    fn opening_repository_rejects_a_crafted_branch_source_path() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        // 设备命名空间：读取侧必须拒绝，「打开他人构造的仓库」不能换来一条对设备
        // 对象的路径。
        connection
            .execute(
                "UPDATE branches SET source_path = ?1, source_path_key = ?1",
                [r"\\.\PhysicalDrive0"],
            )
            .unwrap();
        drop(connection);
        let device_error = open_existing(&fixture.root).unwrap_err();
        assert!(device_error.contains("设备命名空间"), "{device_error}");

        // 仓库内部路径同样拒绝；网络位置（NAS）放行，打开不因网络路径失败。
        let inside = fixture.root.join("artworks").join("artwork.bin");
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "UPDATE branches SET source_path = ?1, source_path_key = ?1",
                [display_path(&inside)],
            )
            .unwrap();
        drop(connection);
        let inside_error = open_existing(&fixture.root).unwrap_err();
        assert!(inside_error.contains("作品仓库内部"), "{inside_error}");

        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "UPDATE branches SET source_path = ?1, source_path_key = ?1",
                [r"\\nas\share\artwork.bin"],
            )
            .unwrap();
        drop(connection);
        open_existing(&fixture.root).unwrap();
    }

    #[test]
    fn inspects_repository_path_states_without_creating_anything() {
        let directory = tempfile::tempdir().unwrap();

        let missing = directory.path().join("missing");
        assert_eq!(
            inspect_path(&missing).unwrap(),
            RepositoryPathState::Missing
        );
        assert!(!missing.exists());

        let file = directory.path().join("file.txt");
        fs::write(&file, b"not a directory").unwrap();
        assert_eq!(
            inspect_path(&file).unwrap(),
            RepositoryPathState::NotDirectory
        );

        let empty = directory.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert_eq!(
            inspect_path(&empty).unwrap(),
            RepositoryPathState::EmptyDirectory
        );
        assert!(!database_path(&empty).exists());

        let occupied = directory.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("notes.txt"), b"not a repository").unwrap();
        assert_eq!(
            inspect_path(&occupied).unwrap(),
            RepositoryPathState::NonRepositoryDirectory
        );

        let repository = directory.path().join("repository");
        initialize(&repository).unwrap();
        assert_eq!(
            inspect_path(&repository).unwrap(),
            RepositoryPathState::Repository
        );
    }

    #[test]
    fn inspection_recognizes_a_migratable_alternate_database_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root).unwrap();
        fs::rename(database_path(&root), root.join("legacy.sqlite3")).unwrap();

        assert_eq!(
            inspect_path(&root).unwrap(),
            RepositoryPathState::Repository
        );
        // 探测本身不做改名。
        assert!(!database_path(&root).exists());
        assert!(root.join("legacy.sqlite3").is_file());
    }

    #[test]
    fn inspection_ignores_runtime_lock_artifacts_when_checking_emptiness() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir(&root).unwrap();
        let guard =
            crate::repository_lock::RepositoryOwnerLock::acquire(&root, "gui", "settings:inspect")
                .unwrap();

        assert_eq!(
            inspect_path(&root).unwrap(),
            RepositoryPathState::EmptyDirectory
        );
        drop(guard);
    }

    #[test]
    fn discovers_and_renames_alternate_repository_database() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root).unwrap();
        let preferred = database_path(&root);
        let alternate = root.join("legacy.sqlite3");
        fs::rename(&preferred, &alternate).unwrap();

        open_existing(&root).unwrap();

        assert!(preferred.is_file());
        assert!(!alternate.exists());
    }

    #[test]
    fn alternate_repository_migration_checkpoints_committed_wal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root).unwrap();
        let preferred = database_path(&root);
        let alternate = root.join("legacy.sqlite3");
        let preferred_wal = sqlite_sidecar_path(&preferred, "-wal");
        let alternate_wal = sqlite_sidecar_path(&alternate, "-wal");
        let group_id = storage::new_id();

        let connection = storage::open(&root).unwrap();
        connection
            .execute_batch("PRAGMA wal_autocheckpoint = 0;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_nodes
                   (id, parent_id, kind, title, position, created_ms, updated_ms)
                 VALUES (?1, NULL, 'group', 'Recovered from WAL', 0, 1, 1)",
                [&group_id],
            )
            .unwrap();
        assert!(preferred_wal.is_file());
        fs::copy(&preferred, &alternate).unwrap();
        fs::copy(&preferred_wal, &alternate_wal).unwrap();
        drop(connection);
        remove_sqlite_sidecars(&preferred).unwrap();
        fs::remove_file(&preferred).unwrap();

        open_existing(&root).unwrap();

        let recovered: i64 = storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM library_nodes WHERE id = ?1",
                [&group_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovered, 1);
        assert!(!alternate_wal.exists());
    }

    #[test]
    fn owner_lock_runtime_artifacts_do_not_block_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir(&root).unwrap();
        // 首次配置仓库时，owner lock 先于 initialize 创建锁库与 owner 元数据。
        let guard =
            crate::repository_lock::RepositoryOwnerLock::acquire(&root, "gui", "settings:switch")
                .unwrap();
        initialize(&root).expect("运行时锁文件不应让新仓库被判为不可初始化");
        assert!(database_path(&root).is_file());
        drop(guard);
    }

    #[test]
    fn initializes_current_schema() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM repository_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let cleanup_table: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'pending_file_cleanup')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let branch_columns: Vec<String> = {
            let mut statement = connection.prepare("PRAGMA table_info(branches)").unwrap();
            statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let defaults: (i64, i64, i64) = connection
            .query_row(
                "SELECT consecutive_backup_failures, backup_retry_at_ms IS NULL,
                        backup_disable_notice_pending
                 FROM branches WHERE id = ?1",
                [&fixture.artwork.branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(version, crate::library::schema::SCHEMA_VERSION);
        assert!(cleanup_table);
        for column in [
            "consecutive_backup_failures",
            "backup_retry_at_ms",
            "backup_disable_notice_pending",
            "backup_quick_enabled",
            "last_source_size",
            "last_source_modified_ms",
        ] {
            assert!(branch_columns.iter().any(|item| item == column), "{column}");
        }
        for column in ["verified_history_id", "verified_ms", "verify_error"] {
            assert!(
                !branch_columns.iter().any(|item| item == column),
                "{column}"
            );
        }
        let node_columns: Vec<String> = {
            let mut statement = connection
                .prepare("PRAGMA table_info(history_nodes)")
                .unwrap();
            statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for column in [
            "thumbnail",
            "thumbnail_version",
            "verified_ms",
            "verify_error",
        ] {
            assert!(node_columns.iter().any(|item| item == column), "{column}");
        }
        assert_eq!(defaults, (0, 1, 0));
        open_existing(&fixture.root).unwrap();
    }
}
