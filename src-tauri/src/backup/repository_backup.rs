use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{authenticity, library, storage};

use super::restore;

const BACKUP_FORMAT: &str = "lilith-artworks-repository-backup";
const BACKUP_FORMAT_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";
const REPOSITORY_DIRECTORY: &str = "repository";
const COPY_BUFFER_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryBackupReport {
    pub(crate) backup_path: String,
    pub(crate) repository_path: String,
    pub(crate) file_count: u64,
    pub(crate) total_bytes: u64,
    pub(crate) history_nodes: u64,
    pub(crate) final_artifacts: u64,
    pub(crate) certification_records: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryBackupManifest {
    format: String,
    format_version: u32,
    created_ms: i64,
    repository_directory: String,
    file_count: u64,
    total_bytes: u64,
    history_nodes: u64,
    final_artifacts: u64,
    certification_records: u64,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    path: String,
    bytes: u64,
    sha256: String,
}

struct StagingDirectory {
    path: PathBuf,
    published: bool,
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(crate) fn create_repository_backup(
    root: &Path,
    destination_parent: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(&str, u64, u64),
) -> Result<RepositoryBackupReport, String> {
    validate_destination(root, destination_parent)?;
    ensure_not_cancelled(&cancelled)?;
    progress("正在固定数据库一致性视图", 0, 0);
    checkpoint_database(root)?;

    let source_files = collect_repository_files(root)?;
    let source_bytes = source_files.iter().try_fold(0_u64, |total, path| {
        let bytes = path
            .metadata()
            .map_err(|error| format!("无法读取仓库文件大小：{error}"))?
            .len();
        total
            .checked_add(bytes)
            .ok_or_else(|| "仓库文件总大小超出支持范围".to_owned())
    })?;

    let backup_id = uuid::Uuid::new_v4().simple().to_string();
    let created_ms = storage::now_ms()?;
    let staging_path = destination_parent.join(format!(".lilith-artworks-{backup_id}.tmp"));
    let final_path = destination_parent.join(format!(
        "Lilith-Artworks-backup-{created_ms}-{}",
        &backup_id[..8]
    ));
    if staging_path.exists() || final_path.exists() {
        return Err("灾备输出目录已存在，请重新选择保存位置".into());
    }
    fs::create_dir(&staging_path).map_err(|error| format!("无法创建灾备临时目录：{error}"))?;
    let mut staging = StagingDirectory {
        path: staging_path.clone(),
        published: false,
    };
    let repository_copy = staging_path.join(REPOSITORY_DIRECTORY);
    fs::create_dir(&repository_copy).map_err(|error| format!("无法创建灾备仓库目录：{error}"))?;

    let mut copied_bytes = 0_u64;
    progress("正在复制仓库文件", 0, source_bytes);
    for source in &source_files {
        ensure_not_cancelled(&cancelled)?;
        let relative = source
            .strip_prefix(root)
            .map_err(|_| "无法计算仓库文件相对路径")?;
        let target = repository_copy.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建灾备子目录：{error}"))?;
        }
        copy_file_stable(source, &target, &cancelled, |bytes| {
            copied_bytes = copied_bytes.saturating_add(bytes);
            progress("正在复制仓库文件", copied_bytes, source_bytes);
        })?;
    }

    ensure_not_cancelled(&cancelled)?;
    progress("正在校验灾备副本", 0, 0);
    library::open_existing(&repository_copy)?;
    let history_nodes = restore::scrub_history(&repository_copy, &cancelled, |current, total| {
        progress("正在校验灾备历史链", current, total)
    })?;
    let (final_artifacts, certification_records) =
        authenticity::scrub_controlled_files(&repository_copy, &cancelled, |current, total| {
            progress("正在校验灾备发布文件", current, total)
        })?;
    checkpoint_database(&repository_copy)?;
    remove_database_sidecars(&repository_copy)?;

    let copied_files = collect_repository_files(&repository_copy)?;
    let mut manifest_files = Vec::with_capacity(copied_files.len());
    let mut total_bytes = 0_u64;
    for (index, path) in copied_files.iter().enumerate() {
        ensure_not_cancelled(&cancelled)?;
        progress(
            "正在生成灾备校验清单",
            index as u64,
            copied_files.len() as u64,
        );
        let bytes = path
            .metadata()
            .map_err(|error| format!("无法读取灾备文件大小：{error}"))?
            .len();
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "灾备文件总大小超出支持范围".to_owned())?;
        manifest_files.push(ManifestFile {
            path: storage::relative_path(&repository_copy, path)?,
            bytes,
            sha256: sha256_file(path, &cancelled)?,
        });
    }
    progress(
        "正在生成灾备校验清单",
        copied_files.len() as u64,
        copied_files.len() as u64,
    );

    let manifest = RepositoryBackupManifest {
        format: BACKUP_FORMAT.into(),
        format_version: BACKUP_FORMAT_VERSION,
        created_ms,
        repository_directory: REPOSITORY_DIRECTORY.into(),
        file_count: manifest_files.len() as u64,
        total_bytes,
        history_nodes,
        final_artifacts,
        certification_records,
        files: manifest_files,
    };
    write_manifest(&staging_path, &manifest)?;
    verify_backup_bundle(&staging_path, &cancelled)?;
    ensure_not_cancelled(&cancelled)?;

    fs::rename(&staging_path, &final_path).map_err(|error| format!("无法发布灾备副本：{error}"))?;
    staging.published = true;

    Ok(RepositoryBackupReport {
        backup_path: storage::display_path(&final_path),
        repository_path: storage::display_path(&final_path.join(REPOSITORY_DIRECTORY)),
        file_count: manifest.file_count,
        total_bytes: manifest.total_bytes,
        history_nodes,
        final_artifacts,
        certification_records,
    })
}

fn validate_destination(root: &Path, destination_parent: &Path) -> Result<(), String> {
    if !destination_parent.is_absolute() {
        return Err("灾备保存目录必须使用绝对路径".into());
    }
    if !destination_parent.is_dir() {
        return Err("灾备保存目录不存在或不是目录".into());
    }
    storage::ensure_outside_repository(root, destination_parent, "灾备保存目录")
}

fn checkpoint_database(root: &Path) -> Result<(), String> {
    let connection = storage::open(root)?;
    let (busy, _log_frames, _checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage::database_error)?;
    if busy != 0 {
        return Err("数据库仍有活动连接，无法建立一致性灾备副本".into());
    }
    Ok(())
}

fn collect_repository_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("无法读取仓库目录：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取仓库目录项：{error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法读取仓库目录项属性：{error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "仓库包含不受支持的符号链接：{}",
                storage::display_path(&path)
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "无法计算仓库目录项相对路径")?;
            if !is_database_sidecar(relative) {
                files.push(path);
            }
        } else {
            return Err(format!(
                "仓库包含不受支持的文件类型：{}",
                storage::display_path(&path)
            ));
        }
    }
    Ok(())
}

fn is_database_sidecar(relative: &Path) -> bool {
    relative
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        && relative.file_name().is_some_and(|name| {
            let value = name.to_string_lossy();
            value == format!("{}-wal", storage::DATABASE_NAME)
                || value == format!("{}-shm", storage::DATABASE_NAME)
                || value == format!("{}-journal", storage::DATABASE_NAME)
        })
}

fn copy_file_stable(
    source: &Path,
    target: &Path,
    cancelled: &impl Fn() -> bool,
    mut on_progress: impl FnMut(u64),
) -> Result<(), String> {
    let before = source
        .metadata()
        .map_err(|error| format!("无法读取仓库文件属性：{error}"))?;
    let mut input = File::open(source).map_err(|error| format!("无法读取仓库文件：{error}"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("无法创建灾备文件：{error}"))?;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut copied = 0_u64;
    loop {
        ensure_not_cancelled(cancelled)?;
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("无法读取仓库文件：{error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("无法写入灾备文件：{error}"))?;
        copied = copied.saturating_add(read as u64);
        on_progress(read as u64);
    }
    output
        .sync_all()
        .map_err(|error| format!("无法同步灾备文件：{error}"))?;
    let after = source
        .metadata()
        .map_err(|error| format!("无法复核仓库文件属性：{error}"))?;
    if before.len() != copied
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err("仓库文件在灾备复制期间发生变化，本次操作已取消".into());
    }
    Ok(())
}

fn remove_database_sidecars(root: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = root.join(format!("{}{suffix}", storage::DATABASE_NAME));
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("无法清理灾备数据库临时文件：{error}"))?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path, cancelled: &impl Fn() -> bool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取灾备文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        ensure_not_cancelled(cancelled)?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法校验灾备文件：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

fn write_manifest(bundle: &Path, manifest: &RepositoryBackupManifest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("无法生成灾备校验清单：{error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(bundle.join(MANIFEST_NAME))
        .map_err(|error| format!("无法创建灾备校验清单：{error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("无法写入灾备校验清单：{error}"))?;
    file.sync_all()
        .map_err(|error| format!("无法同步灾备校验清单：{error}"))
}

fn verify_backup_bundle(bundle: &Path, cancelled: &impl Fn() -> bool) -> Result<(), String> {
    let manifest: RepositoryBackupManifest = serde_json::from_slice(
        &fs::read(bundle.join(MANIFEST_NAME))
            .map_err(|error| format!("无法读取灾备校验清单：{error}"))?,
    )
    .map_err(|error| format!("无法解析灾备校验清单：{error}"))?;
    if manifest.format != BACKUP_FORMAT
        || manifest.format_version != BACKUP_FORMAT_VERSION
        || manifest.repository_directory != REPOSITORY_DIRECTORY
    {
        return Err("灾备校验清单格式不受支持".into());
    }
    let repository = bundle.join(REPOSITORY_DIRECTORY);
    let actual_files = collect_repository_files(&repository)?;
    let actual_paths = actual_files
        .iter()
        .map(|path| storage::relative_path(&repository, path))
        .collect::<Result<HashSet<_>, _>>()?;
    let expected_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<HashSet<_>>();
    if actual_paths != expected_paths || manifest.file_count != manifest.files.len() as u64 {
        return Err("灾备文件集合与校验清单不匹配".into());
    }
    let mut total_bytes = 0_u64;
    for entry in &manifest.files {
        ensure_not_cancelled(cancelled)?;
        let path = storage::resolve_path(&repository, &entry.path)?;
        let bytes = path
            .metadata()
            .map_err(|error| format!("无法读取灾备文件属性：{error}"))?
            .len();
        if bytes != entry.bytes
            || !sha256_file(&path, cancelled)?.eq_ignore_ascii_case(&entry.sha256)
        {
            return Err(format!("灾备文件校验失败：{}", entry.path));
        }
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "灾备文件总大小超出支持范围".to_owned())?;
    }
    if total_bytes != manifest.total_bytes {
        return Err("灾备文件总大小与校验清单不匹配".into());
    }
    Ok(())
}

fn ensure_not_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("灾备操作已取消".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use crate::{backup::worker::run_backup, history};

    use super::*;

    #[test]
    fn backup_copy_reopens_and_scrubs_after_source_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        let source = directory.path().join("artwork.bin");
        fs::create_dir(&destination).unwrap();
        File::create(&source)
            .unwrap()
            .write_all(&vec![b'A'; 96 * 1024])
            .unwrap();
        library::initialize(&root).unwrap();
        let artwork = library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
        run_backup(&root, &artwork.branch_id, "First", "manual", || false).unwrap();
        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        run_backup(&root, &artwork.branch_id, "Second", "manual", || false).unwrap();

        let report = create_repository_backup(&root, &destination, || false, |_, _, _| {}).unwrap();
        let restored = PathBuf::from(&report.repository_path);
        assert_eq!(report.history_nodes, 2);
        assert!(report.file_count >= 3);
        verify_backup_bundle(Path::new(&report.backup_path), &|| false).unwrap();

        fs::remove_dir_all(&root).unwrap();
        library::open_existing(&restored).unwrap();
        assert_eq!(
            restore::scrub_history(&restored, || false, |_, _| {}).unwrap(),
            2
        );
        assert_eq!(
            history::list(&restored, &artwork.artwork_id)
                .unwrap()
                .nodes
                .len(),
            2
        );
    }

    #[test]
    fn destination_inside_repository_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        library::initialize(&root).unwrap();
        let destination = root.join("backups");
        fs::create_dir(&destination).unwrap();

        let error =
            create_repository_backup(&root, &destination, || false, |_, _, _| {}).unwrap_err();

        assert!(error.contains("不能位于作品仓库内部"), "{error}");
    }

    #[test]
    fn cancellation_removes_the_staging_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();

        let cancellation_checks = AtomicUsize::new(0);
        let error = create_repository_backup(
            &root,
            &destination,
            || cancellation_checks.fetch_add(1, Ordering::SeqCst) >= 2,
            |_, _, _| {},
        )
        .unwrap_err();

        assert!(error.contains("已取消"), "{error}");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }

    #[test]
    fn manifest_verification_detects_a_replaced_backup_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        let report = create_repository_backup(&root, &destination, || false, |_, _, _| {}).unwrap();
        let bundle = PathBuf::from(report.backup_path);

        fs::write(
            bundle
                .join(REPOSITORY_DIRECTORY)
                .join(storage::DATABASE_NAME),
            b"replaced database",
        )
        .unwrap();
        let error = verify_backup_bundle(&bundle, &|| false).unwrap_err();

        assert!(error.contains("校验失败"), "{error}");
    }
}
