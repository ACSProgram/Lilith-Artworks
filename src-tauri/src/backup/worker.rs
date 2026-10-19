use std::{
    fs,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{
    cancel::CancelToken,
    cleanup,
    history::{self, HistoryCommit},
    preview, storage,
};

use super::{
    chunk_file::{ChunkFile, ChunkTask, ChunkWriteOptions, ChunkingConfig},
    BackupCommitResult,
};

const MAX_TITLE_CHARS: usize = 160;
/// 快速检查指纹的版本与首尾采样大小。版本号随算法演进；旧基线（历史上直接
/// 保存的 mtime）与当前值不相等时自然退回全量并重写基线。
const SOURCE_FINGERPRINT_VERSION: u8 = 1;
const SOURCE_FINGERPRINT_SAMPLE_BYTES: u64 = 64 * 1024;

/// 提交失败的原因。`pub(crate)` 而不是 `pub(super)`：无头入口同样要区分
/// 「取消」与「失败」，因此这个类型必须对它可见。
#[derive(Debug)]
pub(crate) enum BackupRunError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for BackupRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("备份操作已取消"),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

impl From<String> for BackupRunError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<&str> for BackupRunError {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

#[derive(Clone)]
struct SourceMetadata {
    length: u64,
    modified: Option<SystemTime>,
    /// Cheap fingerprint captured together with the metadata; `None` means the
    /// quick check cannot trust this baseline and must fall back to full.
    fingerprint: Option<i64>,
}

/// 计算工作文件的 cheap fingerprint：版本号、大小、修改时间（纳秒）与
/// 首尾各 64 KiB 采样。任何一步取不到（时间精度不足、打开失败、采样读取失败）
/// 都返回 `None`，调用方必须退回全量；旧基线里保存的裸 mtime 与指纹取值域不同，
/// 因此也会自然退回一次全量并重写基线。
fn source_fingerprint(path: &Path, metadata: &fs::Metadata) -> Option<i64> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    // FAT 等粗粒度时间源可能只记录整秒；没有亚秒精度时无法区分同一秒内
    // 的同尺寸改写，按计划退回全量。
    if duration.subsec_nanos() == 0 {
        return None;
    }
    let size = metadata.len();
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update([SOURCE_FINGERPRINT_VERSION]);
    hasher.update(size.to_le_bytes());
    hasher.update(duration.as_secs().to_le_bytes());
    hasher.update(duration.subsec_nanos().to_le_bytes());

    let mut sample = vec![0_u8; usize::try_from(size.min(SOURCE_FINGERPRINT_SAMPLE_BYTES)).ok()?];
    file.read_exact(&mut sample).ok()?;
    hasher.update(&sample);
    if size > SOURCE_FINGERPRINT_SAMPLE_BYTES {
        file.seek(SeekFrom::Start(size - SOURCE_FINGERPRINT_SAMPLE_BYTES))
            .ok()?;
        file.read_exact(&mut sample).ok()?;
        hasher.update(&sample);
    }
    let digest = hasher.finalize();
    // 取摘要前 8 字节并把最高位清零，才能存进 `last_source_modified_ms` 的整数列。
    let digest_prefix = u64::from_le_bytes(digest[..8].try_into().expect("8-byte prefix"));
    Some((digest_prefix & 0x7fff_ffff_ffff_ffff) as i64)
}

/// 快速检查：比较工作文件大小与上次全量检查记录的指纹（时间、首尾采样）。
/// 完全一致返回 true；缺少基线、任何不一致或读取失败都返回 false，交给全量
/// 流程给出权威结论并自行修复或报错。
pub(crate) fn quick_check_unchanged(root: &Path, branch_id: &str) -> Result<bool, String> {
    let branch = history::load_branch(root, branch_id)?;
    // 没有工作文件或还没有任何提交时无从比较，必须走全量路径建立基线。
    if branch.source_path.trim().is_empty() || branch.head_history_id.is_none() {
        return Ok(false);
    }
    // 快速检查只服务无人值守流程，因此网络位置与设备命名空间必须先拒绝：下面的
    // `metadata()` 就是这条路径真正落到操作系统的地方。资格判定由调度器负责记录
    // 原因，这里只兜底不触碰路径。
    if let Some(reason) = storage::unattended_source_path_rejection(&branch.source_path) {
        return Err(reason);
    }
    let Some((size, Some(expected))) = history::load_source_metadata(root, branch_id)? else {
        return Ok(false);
    };
    let path = Path::new(&branch.source_path);
    let Ok(metadata) = path.metadata() else {
        return Ok(false);
    };
    if !metadata.is_file() || metadata.len() != u64::try_from(size).unwrap_or(u64::MAX) {
        return Ok(false);
    }
    let Some(fingerprint) = source_fingerprint(path, &metadata) else {
        return Ok(false);
    };
    Ok(fingerprint == expected)
}

fn record_source_metadata(root: &Path, branch_id: &str, metadata: &SourceMetadata) {
    if let Err(error) =
        history::record_source_metadata(root, branch_id, metadata.length, metadata.fingerprint)
    {
        log::warn!("failed to record source metadata for branch {branch_id}: {error}");
    }
}

/// 工作文件在本次提交读取期间被改写时整体作废：快照、摘要和派生预览都必须
/// 描述同一份内容，不做部分接受。
fn ensure_source_unchanged(
    before: &SourceMetadata,
    after: &SourceMetadata,
) -> Result<(), BackupRunError> {
    if before.length != after.length || before.modified != after.modified {
        return Err(BackupRunError::Failed(
            "工作文件在读取期间发生变化，本次提交已取消".into(),
        ));
    }
    Ok(())
}

/// 从工作文件流式创建快照。
///
/// 独立成函数只为让「快照写入」这一步能被磁盘写满注入测试直接复用：产品路径传入
/// `NamedTempFile` 的文件句柄，测试传入一个耗尽的受限写入器，走的是同一段代码，
/// 因此注入测试断言的是真实行为，而不是一个模仿出来的分支。
fn create_snapshot<W>(
    source: &mut File,
    writer: &mut W,
    options: ChunkWriteOptions,
    cancel: &CancelToken,
) -> Result<ChunkFile, String>
where
    W: Write + Seek,
{
    let task = ChunkTask::cancel(cancel);
    ChunkFile::create(source, writer, ChunkingConfig::default(), options, &task)
        .map_err(|error| format!("无法创建文件快照：{error}"))
}

pub(crate) fn run_backup(
    root: &Path,
    branch_id: &str,
    note: &str,
    commit_kind: &str,
    cancel: &CancelToken,
    options: ChunkWriteOptions,
) -> Result<BackupCommitResult, BackupRunError> {
    if note.chars().count() > 500 {
        return Err(BackupRunError::Failed("提交备注不能超过 500 个字符".into()));
    }
    let started = std::time::Instant::now();
    let title = if note.trim().is_empty() {
        if commit_kind == "automatic" {
            "自动备份"
        } else {
            "主动提交"
        }
    } else {
        note.trim()
    };
    let title = title.chars().take(MAX_TITLE_CHARS).collect::<String>();
    let branch = history::load_branch(root, branch_id)?;
    ensure_not_cancelled(cancel)?;
    history::ensure_directories(root, &branch.artwork_id)?;
    // 未选择工作文件的分支不参与备份：调度已跳过，这里兜底给出明确错误。
    if branch.source_path.trim().is_empty() {
        return Err(BackupRunError::Failed(
            "该分支未选择工作文件，无法创建备份".into(),
        ));
    }
    // 手动提交允许网络位置（NAS 是合法用法），但设备命名空间一律拒绝：这类路径
    // 指向的是设备对象而不是文件，打开它等同一次未经确认的原生对象访问。
    storage::ensure_not_device_path(&branch.source_path, "分支工作文件")
        .map_err(BackupRunError::Failed)?;
    let source_path = Path::new(&branch.source_path);
    let before = source_metadata(source_path)?;
    let mut source =
        File::open(source_path).map_err(|error| format!("无法打开分支工作文件：{error}"))?;
    let artwork_directory = history::artwork_directory(root, &branch.artwork_id);
    let temp_directory = artwork_directory.join("temp");
    let mut snapshot_temp = NamedTempFile::new_in(&temp_directory)
        .map_err(|error| format!("无法创建 snapshot 临时文件：{error}"))?;
    let snapshot = create_snapshot(&mut source, snapshot_temp.as_file_mut(), options, cancel)?;
    snapshot_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步 snapshot：{error}"))?;
    ensure_not_cancelled(cancel)?;
    let after_snapshot = source_metadata(source_path)?;
    ensure_source_unchanged(&before, &after_snapshot)?;

    let digest = snapshot.file_digest().to_hex();
    let checked_ms = storage::now_ms()?;
    let head = branch
        .head_history_id
        .as_deref()
        .map(|id| history::load_node(root, id))
        .transpose()?;
    if let Some(head) = head
        .as_ref()
        .filter(|node| node.sha256.eq_ignore_ascii_case(&digest))
    {
        if let Err(validation_error) = validate_head_snapshot(root, head, cancel) {
            ensure_not_cancelled(cancel)?;
            log::warn!(
                "HEAD snapshot {} failed validation and will be rebuilt: {validation_error}",
                head.id
            );
            // 被替换的旧 snapshot 已在登记事务内入队，这里单遍重放；失败只留
            // 队列可重试，不改变"内容未变化"的成功语义。
            let cleanup_ids = repair_head_snapshot(root, &artwork_directory, head, snapshot_temp)?;
            cleanup::replay(root, &cleanup_ids);
        }
        history::mark_unchanged(root, branch_id, checked_ms)?;
        // 内容已与 head 核对一致，记录基线元数据供快速检查使用。
        record_source_metadata(root, branch_id, &after_snapshot);
        log::info!(
            "backup unchanged: branch_id={branch_id}, history_id={}, logical_bytes={}, format={:?}, elapsed_ms={}",
            head.id,
            snapshot.logical_size(),
            options.format,
            started.elapsed().as_millis()
        );
        return Ok(BackupCommitResult {
            created: false,
            unchanged: true,
            history_id: None,
        });
    }

    let history_id = storage::new_id();
    // The content changed, so this is the only path that pays for a preview.
    // Any unsupported or malformed work file simply produces no thumbnail; the
    // commit result never depends on it.
    let thumbnail = preview::extract(source_path, cancel);
    ensure_not_cancelled(cancel)?;
    // The preview re-reads the file, so re-check the file against the snapshot
    // baseline: a preview may never describe content different from the one
    // that was chunked.
    let after_preview = source_metadata(source_path)?;
    ensure_source_unchanged(&before, &after_preview)?;
    let snapshot_final = artwork_directory
        .join("snapshots")
        .join(format!("{history_id}.lbc"));
    let snapshot_relative = storage::relative_path(root, &snapshot_final)?;
    let mut delta_temp = None;
    let mut delta_final = None;
    let mut delta_relative = None;
    let mut delta_size = None;
    if let Some(previous) = head.as_ref() {
        let previous_snapshot = previous
            .snapshot_path
            .as_deref()
            .ok_or("当前 head 缺少完整 snapshot")?;
        let previous_path = storage::resolve_path(root, previous_snapshot)?;
        let mut previous_file = File::open(&previous_path)
            .map_err(|error| format!("无法打开当前 snapshot：{error}"))?;
        let previous_chunk = ChunkFile::open(&mut previous_file)
            .map_err(|error| format!("无法读取当前 snapshot：{error}"))?;
        let mut temp = NamedTempFile::new_in(&temp_directory)
            .map_err(|error| format!("无法创建 delta 临时文件：{error}"))?;
        let task = ChunkTask::cancel(cancel);
        snapshot
            .create_reverse_delta(
                &previous_chunk,
                &mut previous_file,
                temp.as_file_mut(),
                options,
                &task,
            )
            .map_err(|error| format!("无法创建反向增量：{error}"))?;
        temp.as_file()
            .sync_all()
            .map_err(|error| format!("无法同步 delta：{error}"))?;
        let final_path = artwork_directory
            .join("deltas")
            .join(format!("{history_id}-to-{}.lbd", previous.id));
        delta_size = Some(
            temp.as_file()
                .metadata()
                .map_err(|error| format!("无法读取 delta 大小：{error}"))?
                .len(),
        );
        delta_relative = Some(storage::relative_path(root, &final_path)?);
        delta_final = Some(final_path);
        delta_temp = Some(temp);
    }

    ensure_not_cancelled(cancel)?;
    publish_temp(snapshot_temp, &snapshot_final, "snapshot")?;
    if let (Some(temp), Some(final_path)) = (delta_temp, delta_final.as_ref()) {
        if let Err(error) = publish_temp(temp, final_path, "delta") {
            let _ = fs::remove_file(&snapshot_final);
            return Err(error.into());
        }
    }
    if let Err(error) = ensure_not_cancelled(cancel) {
        let _ = fs::remove_file(&snapshot_final);
        if let Some(path) = delta_final.as_ref() {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }

    let snapshot_size = fs::metadata(&snapshot_final)
        .map_err(|error| format!("无法读取 snapshot 大小：{error}"))?
        .len();
    let commit = HistoryCommit {
        id: &history_id,
        branch_id,
        parent_id: head.as_ref().map(|node| node.id.as_str()),
        title: title.trim(),
        note,
        commit_kind,
        created_ms: checked_ms,
        logical_size: snapshot.logical_size(),
        chunk_file_size: snapshot_size,
        sha256: &digest,
        chunk_count: snapshot.chunk_count() as u64,
        snapshot_path: &snapshot_relative,
        delta_path: delta_relative.as_deref(),
        delta_size,
        thumbnail: thumbnail
            .as_ref()
            .map(|thumbnail| thumbnail.bytes.as_slice()),
        thumbnail_version: thumbnail.as_ref().map(|thumbnail| thumbnail.version),
    };
    // `history::commit` takes ownership of the commit payload, so capture the
    // sizes for the result log before handing it over.
    let committed_logical = snapshot.logical_size();
    let committed_chunk = snapshot_size;
    let committed_delta = delta_size;
    let cleanup_ids = match history::commit(root, commit) {
        Ok(ids) => ids,
        Err(error) => {
            let _ = fs::remove_file(&snapshot_final);
            if let Some(path) = delta_final.as_ref() {
                let _ = fs::remove_file(path);
            }
            return Err(error.into());
        }
    };
    // 提交释放的父 snapshot 已在事务内入队，这里单遍重放；失败只留队列可重试，
    // 不影响已成功的提交结果。
    cleanup::replay(root, &cleanup_ids);
    // 提交成功，记录基线元数据供快速检查使用。
    record_source_metadata(root, branch_id, &after_preview);
    log::info!(
        "backup committed: branch_id={branch_id}, history_id={history_id}, commit_kind={commit_kind}, logical_bytes={committed_logical}, snapshot_bytes={committed_chunk}, delta_bytes={committed_delta:?}, format={:?}, chunking={:?}, elapsed_ms={}",
        options.format,
        options.chunking,
        started.elapsed().as_millis()
    );
    Ok(BackupCommitResult {
        created: true,
        unchanged: false,
        history_id: Some(history_id),
    })
}

fn validate_head_snapshot(
    root: &Path,
    head: &history::HistoryRecord,
    cancel: &CancelToken,
) -> Result<(), String> {
    let relative = head
        .snapshot_path
        .as_deref()
        .ok_or("当前 HEAD 缺少完整 snapshot")?;
    let path = storage::resolve_path(root, relative)?;
    let mut file =
        File::open(&path).map_err(|error| format!("无法打开当前 HEAD snapshot：{error}"))?;
    let snapshot = ChunkFile::open(&mut file)
        .map_err(|error| format!("无法读取当前 HEAD snapshot：{error}"))?;
    if !snapshot
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&head.sha256)
    {
        return Err("当前 HEAD snapshot 摘要与历史数据库不匹配".into());
    }
    let task = ChunkTask::cancel(cancel);
    snapshot
        .copy_original(&mut file, &mut io::sink(), &task)
        .map_err(|error| format!("当前 HEAD snapshot 完整性校验失败：{error}"))
}

/// 修复 head 的 snapshot：发布新文件并登记到历史节点。
///
/// 被替换的旧 snapshot 不再由本函数在提交后直接删除，而是由 `set_snapshot`
/// 在同一事务内入队 `pending_file_cleanup`；返回的 cleanup id 由调用方在成功后
/// 重放（引用复查与摘要校验仍在重放时执行）。
fn repair_head_snapshot(
    root: &Path,
    artwork_directory: &Path,
    head: &history::HistoryRecord,
    snapshot_temp: NamedTempFile,
) -> Result<Vec<String>, String> {
    let repaired_path = artwork_directory.join("snapshots").join(format!(
        "{}-repair-{}.lbc",
        head.id,
        storage::new_id()
    ));
    let repaired_relative = storage::relative_path(root, &repaired_path)?;
    let repaired_size = snapshot_temp
        .as_file()
        .metadata()
        .map_err(|error| format!("无法读取修复 snapshot 大小：{error}"))?
        .len();
    publish_temp(snapshot_temp, &repaired_path, "修复 snapshot")?;
    match history::set_snapshot(root, &head.id, &repaired_relative, repaired_size, true) {
        Ok(cleanup_ids) => Ok(cleanup_ids),
        Err(error) => {
            let _ = fs::remove_file(&repaired_path);
            Err(format!("无法登记修复 snapshot：{error}"))
        }
    }
}

fn ensure_not_cancelled(cancel: &CancelToken) -> Result<(), BackupRunError> {
    if cancel.is_cancelled() {
        Err(BackupRunError::Cancelled)
    } else {
        Ok(())
    }
}

fn source_metadata(path: &Path) -> Result<SourceMetadata, String> {
    let metadata = path
        .metadata()
        .map_err(|error| format!("无法读取工作文件元数据：{error}"))?;
    if !metadata.is_file() {
        return Err("分支工作路径不是普通文件".into());
    }
    Ok(SourceMetadata {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        fingerprint: source_fingerprint(path, &metadata),
    })
}

fn publish_temp(temp: NamedTempFile, target: &Path, label: &str) -> Result<(), String> {
    temp.persist_noclobber(target)
        .map(|_| ())
        .map_err(|error| format!("无法发布 {label} 文件：{}", error.error))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use crate::{
        backup::test_support::{QuotaWriter, Rng},
        history, library,
    };

    use super::*;

    fn create_fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
        String,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::File::create(&source)
            .unwrap()
            .write_all(&vec![b'A'; 96 * 1024])
            .unwrap();
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        (
            directory,
            root,
            source,
            artwork.artwork_id,
            artwork.branch_id,
        )
    }

    fn restore_and_read(root: &Path, directory: &Path, history_id: &str) -> Vec<u8> {
        let output = directory.join(format!("{history_id}-restored.bin"));
        super::super::restore::restore(
            root,
            history_id,
            output.to_str().unwrap(),
            &CancelToken::standalone(),
            |_, _, _| {},
        )
        .unwrap();
        fs::read(output).unwrap()
    }

    #[test]
    fn changed_commits_store_a_node_thumbnail_and_keep_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.png");
        let image =
            image::RgbaImage::from_fn(800, 400, |x, y| image::Rgba([x as u8, y as u8, 120, 255]));
        image.save(&source).unwrap();
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();

        let first = run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let history_id = first.history_id.unwrap();
        let (bytes, version) = history::load_thumbnail(&root, &history_id)
            .unwrap()
            .expect("changed commit should store a preview");
        assert_eq!(version, crate::preview::THUMBNAIL_VERSION);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 256));

        // Unchanged checks and checkpoints keep the existing node preview.
        let second = run_backup(
            &root,
            &artwork.branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(second.unchanged);
        super::super::restore::ensure_checkpoint(
            &root,
            &history_id,
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert_eq!(
            history::load_thumbnail(&root, &history_id)
                .unwrap()
                .unwrap()
                .0,
            bytes
        );
    }

    #[test]
    fn quick_check_tracks_source_metadata_baseline() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();

        // 没有任何提交也没有基线时必须退回全量。
        assert!(!quick_check_unchanged(&root, &branch_id).unwrap());

        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(quick_check_unchanged(&root, &branch_id).unwrap());

        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        assert!(!quick_check_unchanged(&root, &branch_id).unwrap());

        // 恢复内容后元数据基线不再匹配（修改时间变化），仍需全量复核。
        fs::write(&source, vec![b'A'; 96 * 1024]).unwrap();
        assert!(!quick_check_unchanged(&root, &branch_id).unwrap());
    }

    #[test]
    fn quick_check_detects_same_size_mtime_edit_in_sampled_region() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(quick_check_unchanged(&root, &branch_id).unwrap());

        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        let mut bytes = fs::read(&source).unwrap();
        for byte in &mut bytes[..1024] {
            *byte ^= 0xff;
        }
        fs::write(&source, bytes).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(modified)
            .unwrap();

        assert!(!quick_check_unchanged(&root, &branch_id).unwrap());
    }

    #[test]
    fn quick_check_needs_a_readable_source_file() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        assert!(!quick_check_unchanged(&root, &branch_id).unwrap());
    }

    #[test]
    fn rejects_backup_for_a_branch_without_a_source_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", Path::new(""), false).unwrap();

        let error = run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap_err();

        assert!(
            matches!(error, BackupRunError::Failed(ref message) if message.contains("未选择工作文件")),
            "{error}"
        );
    }

    #[test]
    fn commits_delta_and_restores_parent_bytes() {
        let (directory, root, source, artwork_id, branch_id) = create_fixture();

        let first = run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = first.history_id.unwrap();
        fs::File::create(&source)
            .unwrap()
            .write_all(&[vec![b'A'; 48 * 1024], vec![b'B'; 48 * 1024]].concat())
            .unwrap();
        let second = run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(second.created);

        let nodes = history::list(&root, &artwork_id).unwrap().nodes;
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            restore_and_read(&root, directory.path(), &first_id),
            vec![b'A'; 96 * 1024]
        );
        assert_eq!(
            super::super::restore::scrub_history(&root, &CancelToken::standalone(), |_, _| {})
                .unwrap(),
            2
        );
    }

    #[test]
    fn cancellation_has_a_typed_result() {
        let (_directory, root, _source, _artwork_id, branch_id) = create_fixture();

        let cancelled = CancelToken::standalone();
        cancelled.cancel();
        let error = run_backup(
            &root,
            &branch_id,
            "",
            "automatic",
            &cancelled,
            ChunkWriteOptions::V1,
        )
        .unwrap_err();

        assert!(matches!(error, BackupRunError::Cancelled));
    }

    #[test]
    fn repository_scrub_rejects_a_damaged_delta() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        let second = run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let delta_relative = history::load_node(&root, second.history_id.as_deref().unwrap())
            .unwrap()
            .delta_path
            .unwrap();
        fs::write(
            storage::resolve_path(&root, &delta_relative).unwrap(),
            b"damaged delta",
        )
        .unwrap();

        let error =
            super::super::restore::scrub_history(&root, &CancelToken::standalone(), |_, _| {})
                .unwrap_err();

        assert!(error.contains("delta"), "{error}");
    }

    #[test]
    fn unchanged_backup_keeps_a_valid_head_snapshot() {
        let (_directory, root, _source, _artwork_id, branch_id) = create_fixture();
        let first = run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = first.history_id.unwrap();
        let original_path = history::load_node(&root, &first_id).unwrap().snapshot_path;

        let second = run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert!(!second.created);
        assert!(second.unchanged);
        assert_eq!(
            history::load_node(&root, &first_id).unwrap().snapshot_path,
            original_path
        );
    }

    #[test]
    fn unchanged_backup_repairs_a_missing_head_snapshot() {
        let (directory, root, _source, _artwork_id, branch_id) = create_fixture();
        let first = run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = first.history_id.unwrap();
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        fs::remove_file(storage::resolve_path(&root, &original_relative).unwrap()).unwrap();

        let second = run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let repaired = history::load_node(&root, &first_id).unwrap();

        assert!(!second.created);
        assert!(second.unchanged);
        assert_ne!(
            repaired.snapshot_path.as_deref(),
            Some(original_relative.as_str())
        );
        assert!(
            storage::resolve_path(&root, repaired.snapshot_path.as_deref().unwrap())
                .unwrap()
                .is_file()
        );
        assert_eq!(
            restore_and_read(&root, directory.path(), &first_id),
            vec![b'A'; 96 * 1024]
        );
    }

    #[test]
    fn unchanged_backup_repairs_a_corrupt_head_snapshot() {
        let (directory, root, _source, _artwork_id, branch_id) = create_fixture();
        let first = run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = first.history_id.unwrap();
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        let original_path = storage::resolve_path(&root, &original_relative).unwrap();
        let mut damaged = fs::read(&original_path).unwrap();
        *damaged.last_mut().unwrap() ^= 0xff;
        fs::write(&original_path, damaged).unwrap();

        let second = run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let repaired = history::load_node(&root, &first_id).unwrap();

        assert!(!second.created);
        assert!(second.unchanged);
        assert_ne!(
            repaired.snapshot_path.as_deref(),
            Some(original_relative.as_str())
        );
        assert!(!original_path.exists());
        assert_eq!(
            restore_and_read(&root, directory.path(), &first_id),
            vec![b'A'; 96 * 1024]
        );
    }

    #[test]
    fn restore_and_checkpoint_reject_a_replaced_snapshot() {
        let (directory, root, _source, _artwork_id, branch_id) = create_fixture();
        let first = run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = first.history_id.unwrap();
        let relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        fs::write(
            storage::resolve_path(&root, &relative).unwrap(),
            b"replacement",
        )
        .unwrap();

        let output = directory.path().join("replaced-restored.bin");
        let restore_error = super::super::restore::restore(
            &root,
            &first_id,
            output.to_str().unwrap(),
            &CancelToken::standalone(),
            |_, _, _| {},
        )
        .unwrap_err();
        let checkpoint_error = super::super::restore::ensure_checkpoint(
            &root,
            &first_id,
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap_err();

        assert!(restore_error.contains("snapshot"), "{restore_error}");
        assert!(checkpoint_error.contains("snapshot"), "{checkpoint_error}");
        assert!(!output.exists());
    }

    /// 属性测试：随机提交序列下，任意历史节点都能被恢复成提交当时的字节。
    ///
    /// 每一轮对工作文件做若干随机字节改动后提交，记录每个节点的期望内容；随后
    /// 随机回访历史节点，断言恢复产物与该版本逐字节一致。走的是真实提交路径
    /// （分块 → 反向增量 → 事务 → 原子发布）与真实恢复路径（穿增量链）。
    #[test]
    fn randomized_commit_and_restore_sequence_preserves_every_revision() {
        let (directory, root, source, _artwork_id, branch_id) = create_fixture();
        let mut rng = Rng::new(0x5eed_1234_abcd_0001);

        // 先提交一次建立初始节点，其内容就是初始工作文件字节。
        let mut current = fs::read(&source).unwrap();
        let initial = run_backup(
            &root,
            &branch_id,
            "initial",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let mut revisions = vec![(initial.history_id.unwrap(), current.clone())];

        for round in 0..8 {
            let length = current.len();
            for _ in 0..(1 + rng.below(5)) {
                let start = rng.below(length);
                let span = 1 + rng.below((length - start).min(12 * 1024));
                let mask = rng.nonzero_byte();
                for byte in &mut current[start..start + span] {
                    *byte ^= mask;
                }
            }
            fs::write(&source, &current).unwrap();
            let result = run_backup(
                &root,
                &branch_id,
                &format!("round {round}"),
                "manual",
                &CancelToken::standalone(),
                ChunkWriteOptions::V1,
            )
            .unwrap();
            let id = result.history_id.expect("每次改动都应产生新节点");
            revisions.push((id, current.clone()));
        }

        for attempt in 0..24 {
            let index = rng.below(revisions.len());
            let (id, expected) = &revisions[index];
            // 输出名按轮次唯一：同一节点会被随机回访多次。
            let output = directory.path().join(format!("restored-{attempt}.bin"));
            super::super::restore::restore(
                &root,
                id,
                output.to_str().unwrap(),
                &CancelToken::standalone(),
                |_, _, _| {},
            )
            .unwrap();
            assert_eq!(
                &fs::read(&output).unwrap(),
                expected,
                "回退到版本 {index} 时字节不一致"
            );
        }

        assert_eq!(
            super::super::restore::scrub_history(&root, &CancelToken::standalone(), |_, _| {})
                .unwrap(),
            revisions.len() as u64
        );
    }

    /// 磁盘写满时快照写入干净失败：返回真实的「空间不足」错误、不 panic，写到一半的
    /// 半成品无法被当成完整快照，且仓库不受影响、仍可整体校验。
    ///
    /// 注入点在真实的快照写入步骤上（`create_snapshot`），这就是提交路径在**任何数据库
    /// 变更之前**调用的同一函数，配额只够写头部与一小段数据，因此产品遇到的是与真实
    /// ENOSPC 相同的「写到一半失败」，而不是一个被简化成「直接拒绝写入」的假场景。
    #[test]
    fn a_full_disk_during_snapshot_leaves_the_repository_intact() {
        let (directory, root, source, artwork_id, branch_id) = create_fixture();
        let initial = run_backup(
            &root,
            &branch_id,
            "initial",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let first_id = initial.history_id.unwrap();
        let nodes_before = history::list(&root, &artwork_id).unwrap().nodes.len();

        let mut backing = fs::File::create(directory.path().join("full-disk-target.bin")).unwrap();
        let quota = 16 * 1024;
        let mut writer = QuotaWriter::new(&mut backing, quota);
        let mut handle = fs::File::open(&source).unwrap();
        let error = create_snapshot(
            &mut handle,
            &mut writer,
            ChunkWriteOptions::V1,
            &CancelToken::standalone(),
        )
        .unwrap_err();

        assert!(error.contains("无法创建文件快照"), "{error}");
        assert!(
            writer.written() > 0 && writer.written() <= quota,
            "配额内的字节应已落盘：written={}",
            writer.written()
        );

        // 失败发生在任何数据库变更之前：历史与 head 都没有动，最早版本仍逐位可恢复。
        assert_eq!(
            history::list(&root, &artwork_id).unwrap().nodes.len(),
            nodes_before
        );
        assert_eq!(
            super::super::restore::scrub_history(&root, &CancelToken::standalone(), |_, _| {})
                .unwrap(),
            nodes_before as u64
        );
        let head = history::list(&root, &artwork_id).unwrap().nodes[0]
            .id
            .clone();
        assert_eq!(&head, &first_id);
        assert_eq!(
            restore_and_read(&root, directory.path(), &first_id),
            vec![b'A'; 96 * 1024]
        );
    }
}
