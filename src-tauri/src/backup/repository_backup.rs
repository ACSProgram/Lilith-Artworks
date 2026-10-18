use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    cancel::CancelToken,
    contracts::{BackupVerificationMode, RepositoryIntegrityProvider},
    repository_lock, storage,
};

const BACKUP_FORMAT: &str = "lilith-artworks-repository-backup";
const BACKUP_FORMAT_VERSION: u32 = 2;
const MANIFEST_NAME: &str = "manifest.json";
const REPOSITORY_DIRECTORY: &str = "repository";
const COPY_BUFFER_SIZE: usize = 256 * 1024;
/// 发布前抽样复核的采样宽度：首、尾各 64 KiB。复制阶段仍计算完整摘要并写入
/// manifest；采样只决定发布前重读多少字节。
const SAMPLE_BYTES: u64 = 64 * 1024;
/// 未发布暂存目录的命名：`<STAGING_PREFIX><32 位十六进制 backup_id><STAGING_SUFFIX>`。
/// 前缀 + 后缀 + 定长十六进制 id 足够特异，顶层扫描的误删风险仅限于同名碰撞。
const STAGING_PREFIX: &str = ".lilith-artworks-";
const STAGING_SUFFIX: &str = ".tmp";
const STAGING_ID_HEX_LEN: usize = 32;
/// 暂存目录清扫的宽限期：修改时间晚于 `now - 30 分钟` 的暂存目录视为可能仍属于
/// 进行中的灾备复制（本进程或另一个进程），清扫跳过它们。取值与未引用文件扫描的
/// 宽限期一致。
const STAGING_GRACE_MS: i64 = 30 * 60 * 1000;

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
    pub(crate) reclaimed_staging_directories: u64,
    pub(crate) failed_staging_directories: u64,
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
    /// 首尾各 64 KiB 的抽样摘要；发布前 `Sample` 复核只重读这些字节。
    sample_sha256: String,
}

/// 复制阶段同时取得的文件摘要。manifest 直接复用，避免复制后又完整重读一遍。
#[derive(Debug, Clone)]
struct FileDigests {
    bytes: u64,
    sha256: String,
    sample_sha256: String,
}

impl FileDigests {
    /// 重新完整读取一个文件（只用于副本校验会改写的数据库主文件）。
    fn compute(path: &Path, cancel: &CancelToken) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|error| format!("无法读取备份文件：{error}"))?;
        let mut accumulator = DigestAccumulator::new();
        let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
        let mut bytes = 0_u64;
        loop {
            ensure_not_cancelled(cancel)?;
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("无法校验备份文件：{error}"))?;
            if read == 0 {
                break;
            }
            accumulator.update(&buffer[..read]);
            bytes = bytes.saturating_add(read as u64);
        }
        Ok(accumulator.finish(bytes))
    }
}

/// 一次顺序读取同时累计完整 SHA-256 与首尾各 64 KiB 的抽样输入。
struct DigestAccumulator {
    full: Sha256,
    head: Vec<u8>,
    tail: Vec<u8>,
}

impl DigestAccumulator {
    fn new() -> Self {
        Self {
            full: Sha256::new(),
            head: Vec::with_capacity(SAMPLE_BYTES as usize),
            tail: Vec::with_capacity(SAMPLE_BYTES as usize * 2),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.full.update(bytes);
        if self.head.len() < SAMPLE_BYTES as usize {
            let take = (SAMPLE_BYTES as usize - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > SAMPLE_BYTES as usize * 2 {
            let excess = self.tail.len() - SAMPLE_BYTES as usize;
            self.tail.drain(..excess);
        }
    }

    fn finish(mut self, bytes: u64) -> FileDigests {
        // 不超过一个采样宽度的文件整份作为采样；更大的文件取首 64 KiB 与尾
        // 64 KiB（两者允许重叠），只用于发布前快速复核。
        let tail_len = if bytes > SAMPLE_BYTES {
            SAMPLE_BYTES as usize
        } else {
            0
        };
        if self.tail.len() > tail_len {
            self.tail.drain(..self.tail.len() - tail_len);
        }
        let mut sample = Sha256::new();
        sample.update(&self.head);
        sample.update(&self.tail);
        FileDigests {
            bytes,
            sha256: hex::encode_upper(self.full.finalize()),
            sample_sha256: hex::encode_upper(sample.finalize()),
        }
    }
}

struct StagingDirectory {
    path: PathBuf,
    published: bool,
}

impl StagingDirectory {
    fn cleanup(&mut self) -> Result<(), String> {
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {
                self.published = true;
                Ok(())
            }
            Err(_) if !self.path.exists() => {
                self.published = true;
                Ok(())
            }
            Err(error) => Err(format!("临时备份清理失败：{error}")),
        }
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// 一次暂存目录清扫的结果：`reclaimed` 为成功回收的目录数，`failed` 为删除失败数。
#[derive(Debug, Default, Clone, Copy)]
struct StagingSweep {
    reclaimed: u64,
    failed: u64,
}

/// 清扫灾备目标目录**顶层**残留的未发布暂存目录。
///
/// 只处理名字匹配 `<STAGING_PREFIX><32 位十六进制><STAGING_SUFFIX>`、确实是目录
/// （不跟随符号链接）、且修改时间早于 `now_ms - STAGING_GRACE_MS` 的项，逐个
/// `remove_dir_all`。宽限期避免删除正在进行（本进程或另一进程）的灾备暂存目录；
/// 只碰顶层、不递归匹配；删除失败只计数并写日志，绝不阻断本次备份。目标目录
/// 不可读时按空处理。
fn sweep_stale_staging_directories(destination_parent: &Path, now_ms: i64) -> StagingSweep {
    let mut sweep = StagingSweep::default();
    let cutoff_ms = now_ms.saturating_sub(STAGING_GRACE_MS);
    let entries = match fs::read_dir(destination_parent) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!(
                "无法读取备份目标目录 {}，跳过残留暂存目录清扫：{error}",
                storage::display_path(destination_parent)
            );
            return sweep;
        }
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_staging_directory_name(name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let modified_ms = match modified.duration_since(UNIX_EPOCH) {
            Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            Err(_) => continue,
        };
        if modified_ms >= cutoff_ms {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => sweep.reclaimed += 1,
            Err(error) => {
                sweep.failed += 1;
                log::warn!(
                    "无法回收残留的备份暂存目录 {}：{error}",
                    storage::display_path(&path)
                );
            }
        }
    }
    sweep
}

fn is_staging_directory_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(STAGING_PREFIX) else {
        return false;
    };
    let Some(id) = rest.strip_suffix(STAGING_SUFFIX) else {
        return false;
    };
    id.len() == STAGING_ID_HEX_LEN && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn create_repository_backup(
    root: &Path,
    destination_parent: &Path,
    cancel: &CancelToken,
    provider: &dyn RepositoryIntegrityProvider,
    verification: BackupVerificationMode,
    progress: impl Fn(&str, u64, u64),
) -> Result<RepositoryBackupReport, String> {
    validate_destination(root, destination_parent)?;
    let staging_sweep = match storage::now_ms() {
        Ok(now_ms) => sweep_stale_staging_directories(destination_parent, now_ms),
        Err(error) => {
            log::warn!("无法读取当前时间，跳过残留暂存目录清扫：{error}");
            StagingSweep::default()
        }
    };
    ensure_not_cancelled(cancel)?;
    progress("正在固定数据库一致性视图", 0, 0);
    checkpoint_database(root)?;

    progress("正在扫描仓库文件", 0, 0);
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
    progress("仓库文件扫描完成", 1, 1);

    let backup_id = uuid::Uuid::new_v4().simple().to_string();
    let created_ms = storage::now_ms()?;
    let staging_path =
        destination_parent.join(format!("{STAGING_PREFIX}{backup_id}{STAGING_SUFFIX}"));
    let final_path = destination_parent.join(format!(
        "Lilith-Artworks-backup-{created_ms}-{}",
        &backup_id[..8]
    ));
    if staging_path.exists() || final_path.exists() {
        return Err("备份输出目录已存在，请重新选择保存位置".into());
    }
    fs::create_dir(&staging_path).map_err(|error| format!("无法创建备份临时目录：{error}"))?;
    let mut staging = StagingDirectory {
        path: staging_path.clone(),
        published: false,
    };
    let result: Result<RepositoryBackupReport, String> = (|| {
        let repository_copy = staging_path.join(REPOSITORY_DIRECTORY);
        fs::create_dir(&repository_copy)
            .map_err(|error| format!("无法创建备份仓库目录：{error}"))?;

        let mut copied_bytes = 0_u64;
        let mut copied_digests = HashMap::with_capacity(source_files.len());
        progress("正在复制仓库文件", 0, source_bytes);
        for source in &source_files {
            ensure_not_cancelled(cancel)?;
            let relative = source
                .strip_prefix(root)
                .map_err(|_| "无法计算仓库文件相对路径")?;
            let target = repository_copy.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("无法创建备份子目录：{error}"))?;
            }
            let digests = copy_file_stable(source, &target, cancel, |bytes| {
                copied_bytes = copied_bytes.saturating_add(bytes);
                progress("正在复制仓库文件", copied_bytes, source_bytes);
            })?;
            copied_digests.insert(storage::relative_path(root, source)?, digests);
        }

        ensure_not_cancelled(cancel)?;
        progress("正在校验备份", 0, 0);
        let integrity = provider.verify_repository(
            &repository_copy,
            cancel,
            &mut |label, current, total| progress(label, current, total),
        )?;
        checkpoint_database(&repository_copy)?;
        remove_database_sidecars(&repository_copy)?;

        let copied_files = collect_repository_files(&repository_copy)?;
        let mut manifest_files = Vec::with_capacity(copied_files.len());
        let mut total_bytes = 0_u64;
        for (index, path) in copied_files.iter().enumerate() {
            ensure_not_cancelled(cancel)?;
            progress(
                "正在生成备份校验清单",
                index as u64,
                copied_files.len() as u64,
            );
            let relative = storage::relative_path(&repository_copy, path)?;
            let bytes = path
                .metadata()
                .map_err(|error| format!("无法读取备份文件大小：{error}"))?
                .len();
            total_bytes = total_bytes
                .checked_add(bytes)
                .ok_or_else(|| "备份文件总大小超出支持范围".to_owned())?;
            // 复制摘要可直接复用；只有副本校验与 checkpoint 会改写数据库主文件，
            // 对它重新取完整与抽样摘要。
            let digests = match copied_digests
                .get(&relative)
                .filter(|digests| digests.bytes == bytes && relative != storage::DATABASE_NAME)
            {
                Some(digests) => digests.clone(),
                None => FileDigests::compute(path, cancel)?,
            };
            manifest_files.push(ManifestFile {
                path: relative,
                bytes,
                sha256: digests.sha256,
                sample_sha256: digests.sample_sha256,
            });
        }
        progress(
            "正在生成备份校验清单",
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
            history_nodes: integrity.history_nodes,
            final_artifacts: integrity.final_artifacts,
            certification_records: integrity.certification_records,
            files: manifest_files,
        };
        write_manifest(&staging_path, &manifest)?;
        verify_backup_bundle(&staging_path, cancel, verification)?;
        ensure_not_cancelled(cancel)?;

        progress("正在发布备份", 0, 1);
        fs::rename(&staging_path, &final_path).map_err(|error| format!("无法发布备份：{error}"))?;
        progress("备份发布完成", 1, 1);

        Ok(RepositoryBackupReport {
            backup_path: storage::display_path(&final_path),
            repository_path: storage::display_path(&final_path.join(REPOSITORY_DIRECTORY)),
            file_count: manifest.file_count,
            total_bytes: manifest.total_bytes,
            history_nodes: integrity.history_nodes,
            final_artifacts: integrity.final_artifacts,
            certification_records: integrity.certification_records,
            reclaimed_staging_directories: staging_sweep.reclaimed,
            failed_staging_directories: staging_sweep.failed,
        })
    })();

    match result {
        Ok(report) => {
            staging.published = true;
            Ok(report)
        }
        Err(error) => {
            progress("正在清理临时备份", 0, 1);
            match staging.cleanup() {
                Ok(()) => {
                    progress("临时备份已清理", 1, 1);
                    Err(format!("{error}；临时备份已清理"))
                }
                Err(cleanup_error) => Err(format!("{error}；{cleanup_error}")),
            }
        }
    }
}

fn validate_destination(root: &Path, destination_parent: &Path) -> Result<(), String> {
    if !destination_parent.is_absolute() {
        return Err("备份保存目录必须使用绝对路径".into());
    }
    if !destination_parent.is_dir() {
        return Err("备份保存目录不存在或不是目录".into());
    }
    storage::ensure_outside_repository(root, destination_parent, "备份保存目录")
}

fn checkpoint_database(root: &Path) -> Result<(), String> {
    let connection = storage::open(root)?;
    let (busy, _log_frames, _checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage::database_error)?;
    if busy != 0 {
        return Err("数据库仍有活动连接，无法建立一致性备份".into());
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
            // 锁库、锁日志与 owner 元数据是进程级运行时文件，不属于仓库数据；
            // 灾备副本不应携带它们（心跳也会让副本在复制期间不稳定）。
            if !is_database_sidecar(relative) && !repository_lock::is_runtime_artifact(relative) {
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
    cancel: &CancelToken,
    mut on_progress: impl FnMut(u64),
) -> Result<FileDigests, String> {
    let before = source
        .metadata()
        .map_err(|error| format!("无法读取仓库文件属性：{error}"))?;
    let mut input = File::open(source).map_err(|error| format!("无法读取仓库文件：{error}"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("无法创建备份文件：{error}"))?;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut digests = DigestAccumulator::new();
    let mut copied = 0_u64;
    loop {
        ensure_not_cancelled(cancel)?;
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("无法读取仓库文件：{error}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("无法写入备份文件：{error}"))?;
        digests.update(&buffer[..read]);
        copied = copied.saturating_add(read as u64);
        on_progress(read as u64);
    }
    output
        .sync_all()
        .map_err(|error| format!("无法同步备份文件：{error}"))?;
    let after = source
        .metadata()
        .map_err(|error| format!("无法复核仓库文件属性：{error}"))?;
    if before.len() != copied
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err("仓库文件在备份期间发生变化，本次操作已取消".into());
    }
    Ok(digests.finish(copied))
}

fn remove_database_sidecars(root: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = root.join(format!("{}{suffix}", storage::DATABASE_NAME));
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("无法清理备份数据库临时文件：{error}"))?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path, cancel: &CancelToken) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取备份文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        ensure_not_cancelled(cancel)?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法校验备份文件：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

/// 只重读首尾各 64 KiB 的抽样摘要，口径与复制阶段的 `DigestAccumulator` 一致。
fn sample_sha256_file(path: &Path, cancel: &CancelToken) -> Result<String, String> {
    ensure_not_cancelled(cancel)?;
    let bytes = path
        .metadata()
        .map_err(|error| format!("无法读取备份文件属性：{error}"))?
        .len();
    let mut file = File::open(path).map_err(|error| format!("无法读取备份文件：{error}"))?;
    let head_len = bytes.min(SAMPLE_BYTES) as usize;
    let mut head = vec![0_u8; head_len];
    file.read_exact(&mut head)
        .map_err(|error| format!("无法校验备份文件：{error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&head);
    if bytes > SAMPLE_BYTES {
        ensure_not_cancelled(cancel)?;
        file.seek(SeekFrom::Start(bytes - SAMPLE_BYTES))
            .map_err(|error| format!("无法校验备份文件：{error}"))?;
        let mut tail = vec![0_u8; SAMPLE_BYTES as usize];
        file.read_exact(&mut tail)
            .map_err(|error| format!("无法校验备份文件：{error}"))?;
        hasher.update(&tail);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

fn write_manifest(bundle: &Path, manifest: &RepositoryBackupManifest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("无法生成备份校验清单：{error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(bundle.join(MANIFEST_NAME))
        .map_err(|error| format!("无法创建备份校验清单：{error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("无法写入备份校验清单：{error}"))?;
    file.sync_all()
        .map_err(|error| format!("无法同步备份校验清单：{error}"))
}

fn verify_backup_bundle(
    bundle: &Path,
    cancel: &CancelToken,
    mode: BackupVerificationMode,
) -> Result<(), String> {
    let manifest: RepositoryBackupManifest = serde_json::from_slice(
        &fs::read(bundle.join(MANIFEST_NAME))
            .map_err(|error| format!("无法读取备份校验清单：{error}"))?,
    )
    .map_err(|error| format!("无法解析备份校验清单：{error}"))?;
    if manifest.format != BACKUP_FORMAT
        || manifest.format_version != BACKUP_FORMAT_VERSION
        || manifest.repository_directory != REPOSITORY_DIRECTORY
    {
        return Err("备份校验清单格式不受支持".into());
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
        return Err("备份文件集合与校验清单不匹配".into());
    }
    let mut total_bytes = 0_u64;
    for entry in &manifest.files {
        ensure_not_cancelled(cancel)?;
        let path = storage::resolve_path(&repository, &entry.path)?;
        let bytes = path
            .metadata()
            .map_err(|error| format!("无法读取备份文件属性：{error}"))?
            .len();
        let digest_matches = match mode {
            BackupVerificationMode::Sample => {
                sample_sha256_file(&path, cancel)?.eq_ignore_ascii_case(&entry.sample_sha256)
            }
            BackupVerificationMode::Full => {
                sha256_file(&path, cancel)?.eq_ignore_ascii_case(&entry.sha256)
            }
        };
        if bytes != entry.bytes || !digest_matches {
            return Err(format!("备份文件校验失败：{}", entry.path));
        }
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "备份文件总大小超出支持范围".to_owned())?;
    }
    if total_bytes != manifest.total_bytes {
        return Err("备份文件总大小与校验清单不匹配".into());
    }
    Ok(())
}

fn ensure_not_cancelled(cancel: &CancelToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("备份操作已取消".into())
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

    use crate::{
        app::contracts::RepositoryScrubProvider, backup::restore, backup::worker::run_backup,
        history, library,
    };

    use super::*;

    fn backup_to(
        root: &Path,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<RepositoryBackupReport, String> {
        create_repository_backup(
            root,
            destination,
            cancel,
            &RepositoryScrubProvider,
            BackupVerificationMode::Sample,
            |_, _, _| {},
        )
    }

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
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            crate::backup::chunk_file::ChunkWriteOptions::V1,
        )
        .unwrap();
        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        run_backup(
            &root,
            &artwork.branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            crate::backup::chunk_file::ChunkWriteOptions::V1,
        )
        .unwrap();

        let report = backup_to(&root, &destination, &CancelToken::standalone()).unwrap();
        let restored = PathBuf::from(&report.repository_path);
        assert_eq!(report.history_nodes, 2);
        assert!(report.file_count >= 3);
        verify_backup_bundle(
            Path::new(&report.backup_path),
            &CancelToken::standalone(),
            BackupVerificationMode::Full,
        )
        .unwrap();

        fs::remove_dir_all(&root).unwrap();
        library::open_existing(&restored).unwrap();
        assert_eq!(
            restore::scrub_history(&restored, &CancelToken::standalone(), |_, _| {}).unwrap(),
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

        let error = backup_to(&root, &destination, &CancelToken::standalone()).unwrap_err();

        assert!(error.contains("不能位于作品仓库内部"), "{error}");
    }

    #[test]
    fn runtime_lock_artifacts_are_not_copied_into_the_backup() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(root.join(repository_lock::LOCK_FILE_NAME), b"lock").unwrap();
        fs::write(
            root.join(format!("{}-journal", repository_lock::LOCK_FILE_NAME)),
            b"journal",
        )
        .unwrap();
        fs::write(root.join(repository_lock::OWNER_FILE_NAME), b"owner").unwrap();

        let report = backup_to(&root, &destination, &CancelToken::standalone()).unwrap();
        let restored = PathBuf::from(&report.repository_path);
        assert!(!restored.join(repository_lock::LOCK_FILE_NAME).exists());
        assert!(!restored
            .join(format!("{}-journal", repository_lock::LOCK_FILE_NAME))
            .exists());
        assert!(!restored.join(repository_lock::OWNER_FILE_NAME).exists());
        verify_backup_bundle(
            Path::new(&report.backup_path),
            &CancelToken::standalone(),
            BackupVerificationMode::Sample,
        )
        .unwrap();
    }

    #[test]
    fn cancellation_removes_the_staging_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();

        let cancellation_checks = std::sync::Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::with_source({
            let cancellation_checks = cancellation_checks.clone();
            move || cancellation_checks.fetch_add(1, Ordering::SeqCst) >= 2
        });
        let error = backup_to(&root, &destination, &cancel).unwrap_err();

        assert!(error.contains("已取消"), "{error}");
        assert!(error.contains("临时备份已清理"), "{error}");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }

    #[test]
    fn manifest_verification_detects_a_replaced_backup_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        let report = backup_to(&root, &destination, &CancelToken::standalone()).unwrap();
        let bundle = PathBuf::from(report.backup_path);

        fs::write(
            bundle
                .join(REPOSITORY_DIRECTORY)
                .join(storage::DATABASE_NAME),
            b"replaced database",
        )
        .unwrap();
        let error = verify_backup_bundle(
            &bundle,
            &CancelToken::standalone(),
            BackupVerificationMode::Full,
        )
        .unwrap_err();

        assert!(error.contains("校验失败"), "{error}");
    }

    fn staging_directory(parent: &Path) -> PathBuf {
        parent.join(format!(
            "{STAGING_PREFIX}{}{STAGING_SUFFIX}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn stale_staging_directories_are_reclaimed() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("backups");
        fs::create_dir(&destination).unwrap();
        let stale = staging_directory(&destination);
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("payload.bin"), b"leftover").unwrap();
        // 命名不匹配的目录、以及名字像暂存目录的普通文件，都不在清扫范围内。
        let unrelated = destination.join("Lilith-Artworks-backup-1-abcdef12");
        fs::create_dir(&unrelated).unwrap();
        let impostor = destination.join(format!(
            "{STAGING_PREFIX}{}{STAGING_SUFFIX}",
            "f".repeat(STAGING_ID_HEX_LEN)
        ));
        fs::write(&impostor, b"not a directory").unwrap();

        // 把「当前时间」推到宽限期之后，使刚创建的暂存目录被判定为过期残留。
        let now_ms = storage::now_ms().unwrap();
        let sweep =
            sweep_stale_staging_directories(&destination, now_ms + STAGING_GRACE_MS + 60_000);

        assert_eq!(sweep.reclaimed, 1);
        assert_eq!(sweep.failed, 0);
        assert!(!stale.exists(), "过期残留暂存目录必须被回收");
        assert!(unrelated.exists(), "命名不匹配的目录不能被删除");
        assert!(impostor.exists(), "同名文件不能被删除");
    }

    #[test]
    fn recent_staging_directories_survive_the_grace_period() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("backups");
        fs::create_dir(&destination).unwrap();
        let recent = staging_directory(&destination);
        fs::create_dir(&recent).unwrap();

        let sweep = sweep_stale_staging_directories(&destination, storage::now_ms().unwrap());

        assert_eq!(sweep.reclaimed, 0);
        assert_eq!(sweep.failed, 0);
        assert!(recent.exists(), "宽限期内的暂存目录必须保留");
    }

    #[test]
    fn backup_publishes_and_keeps_a_recent_staging_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        // 模拟另一次仍在进行的灾备：宽限期内的暂存目录不能被本次备份清扫掉。
        let recent = staging_directory(&destination);
        fs::create_dir(&recent).unwrap();

        let report = backup_to(&root, &destination, &CancelToken::standalone()).unwrap();

        assert_eq!(report.reclaimed_staging_directories, 0);
        assert_eq!(report.failed_staging_directories, 0);
        assert!(recent.exists(), "本次备份不得删除宽限期内的暂存目录");
        let bundle = PathBuf::from(&report.backup_path);
        assert!(bundle.is_dir(), "本次备份必须正常发布");
        verify_backup_bundle(
            &bundle,
            &CancelToken::standalone(),
            BackupVerificationMode::Sample,
        )
        .unwrap();
    }

    #[test]
    fn sample_verification_skips_middle_bytes_but_full_verification_catches_them() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        let source = directory.path().join("artwork.bin");
        fs::create_dir(&destination).unwrap();
        // 伪随机内容避免快照被压缩到小于两个采样宽度。
        let payload = (0..512 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&source, &payload).unwrap();
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            crate::backup::chunk_file::ChunkWriteOptions::V1,
        )
        .unwrap();

        let report = backup_to(&root, &destination, &CancelToken::standalone()).unwrap();
        let bundle = PathBuf::from(&report.backup_path);
        let manifest: RepositoryBackupManifest =
            serde_json::from_slice(&fs::read(bundle.join(MANIFEST_NAME)).unwrap()).unwrap();
        assert_eq!(manifest.format_version, BACKUP_FORMAT_VERSION);
        assert!(manifest
            .files
            .iter()
            .all(|file| file.sample_sha256.len() == 64));
        let entry = manifest
            .files
            .iter()
            .find(|file| file.bytes > SAMPLE_BYTES * 2 && file.path != storage::DATABASE_NAME)
            .expect("快照文件应大于两个采样宽度");
        let path = storage::resolve_path(&bundle.join(REPOSITORY_DIRECTORY), &entry.path).unwrap();

        // 只改中段字节：抽样复核（首尾 64 KiB）不会发现，全量复核必须发现。
        let mut bytes = fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        verify_backup_bundle(
            &bundle,
            &CancelToken::standalone(),
            BackupVerificationMode::Sample,
        )
        .unwrap();
        let error = verify_backup_bundle(
            &bundle,
            &CancelToken::standalone(),
            BackupVerificationMode::Full,
        )
        .unwrap_err();
        assert!(error.contains("校验失败"), "{error}");

        // 改首字节：抽样复核必须发现。
        bytes[0] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        let error = verify_backup_bundle(
            &bundle,
            &CancelToken::standalone(),
            BackupVerificationMode::Sample,
        )
        .unwrap_err();
        assert!(error.contains("校验失败"), "{error}");
    }

    struct MockIntegrityProvider;

    impl RepositoryIntegrityProvider for MockIntegrityProvider {
        fn verify_repository(
            &self,
            _root: &Path,
            _cancel: &CancelToken,
            progress: &mut dyn FnMut(&str, u64, u64),
        ) -> Result<crate::contracts::BackupIntegrityReport, String> {
            progress("mock verify", 1, 1);
            Ok(crate::contracts::BackupIntegrityReport {
                history_nodes: 7,
                final_artifacts: 3,
                certification_records: 5,
            })
        }
    }

    #[test]
    fn integrity_counts_come_from_the_injected_provider() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let destination = directory.path().join("backups");
        library::initialize(&root).unwrap();
        fs::create_dir(&destination).unwrap();

        let report = create_repository_backup(
            &root,
            &destination,
            &CancelToken::standalone(),
            &MockIntegrityProvider,
            BackupVerificationMode::Sample,
            |_, _, _| {},
        )
        .unwrap();

        assert_eq!(report.history_nodes, 7);
        assert_eq!(report.final_artifacts, 3);
        assert_eq!(report.certification_records, 5);
    }
}
