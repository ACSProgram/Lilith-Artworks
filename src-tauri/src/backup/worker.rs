use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{
    cancel::CancelToken,
    cleanup,
    history::{self, HistoryCommit},
    platform::path::ensure_not_device_path,
    preview, storage,
};

use super::{
    chunk_file::{ChunkFile, ChunkTask, ChunkWriteOptions, ChunkingConfig},
    BackupCommitResult,
};

const MAX_TITLE_CHARS: usize = 160;
/// 快速检查指纹的版本与首尾采样大小。版本号随算法演进；旧基线（历史上直接
/// 保存的 mtime，或版本 1 要求亚秒精度的指纹）与当前值取值域不同，因此自然
/// 退回一次全量并重写基线。
const SOURCE_FINGERPRINT_VERSION: u8 = 2;
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

/// 计算工作文件的 cheap fingerprint：版本号、大小、修改时间与首尾各 64 KiB
/// 采样。修改时间按系统实际给出的精度取用——NTFS 带亚秒位，而 `.clip` 这类由
/// 应用保存的文件往往只有整秒；两种精度都必须可比较，因此不再要求亚秒精度。
/// 打开或采样读取失败（文件被删除、被截断、权限不足）时返回 `None`，调用方退回
/// 全量。旧基线里保存的裸 mtime 与指纹取值域不同，也会自然退回一次全量。
fn source_fingerprint(path: &Path, metadata: &fs::Metadata) -> Option<i64> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
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

/// 快速检查为什么没能给出「未变化」的结论。
///
/// 这里把两类原因分开：内容确实可能变了（全量检查本来就要跑，属于正常路径），
/// 与快速检查自身不可用（退化，需要能在日志里看见）。二者都不改变「退回全量」
/// 这个结论，只影响是否值得记一条 warn。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QuickCheckFallback {
    /// 还没有任何提交，或历史表里的基线列为空。
    NoBaseline,
    /// 工作文件的元数据读不到（已删除或无权访问）。
    MetadataUnavailable,
    /// 路径不再是普通文件。
    NotARegularFile,
    /// 大小与基线不同。
    SizeMismatch,
    /// 首尾采样读不到：文件在比较期间被删除或截断。
    SampleUnavailable,
    /// 指纹与基线不同：修改时间或采样内容变了。
    FingerprintMismatch,
}

impl QuickCheckFallback {
    /// 退化：快速检查在这条分支上系统性不可用，值得一条 warn。
    fn is_degradation(self) -> bool {
        !matches!(self, Self::SizeMismatch | Self::FingerprintMismatch)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::NoBaseline => "no baseline",
            Self::MetadataUnavailable => "source metadata unavailable",
            Self::NotARegularFile => "source is not a regular file",
            Self::SizeMismatch => "size mismatch",
            Self::SampleUnavailable => "sample read failed",
            Self::FingerprintMismatch => "fingerprint mismatch",
        }
    }
}

enum QuickCheck {
    Unchanged,
    /// 无法断定未变化，需要全量复核。
    Fallback(QuickCheckFallback),
}

/// 每个分支上一次记录的退回原因，用来把重复日志压成一条；快速检查恢复后清空，
/// 下一次退化仍然可见。分支数量有限，条目在成功后即删除，不会持续增长。
static QUICK_CHECK_FALLBACKS: OnceLock<Mutex<HashMap<String, QuickCheckFallback>>> =
    OnceLock::new();

fn quick_check_fallbacks() -> &'static Mutex<HashMap<String, QuickCheckFallback>> {
    QUICK_CHECK_FALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn clear_fallback(branch_id: &str) {
    if let Ok(mut guard) = quick_check_fallbacks().lock() {
        guard.remove(branch_id);
    }
}

fn report_fallback(branch_id: &str, reason: QuickCheckFallback) {
    let previous = match quick_check_fallbacks().lock() {
        Ok(mut guard) => guard.insert(branch_id.to_owned(), reason),
        Err(_) => None,
    };
    // 同一分支同一原因只记一次；原因变了才重新记。
    if previous == Some(reason) {
        return;
    }
    if reason.is_degradation() {
        log::warn!(
            "backup quick check fell back to a full check: branch_id={branch_id}, reason={}",
            reason.as_str()
        );
    } else {
        log::info!(
            "backup quick check found a changed fingerprint: branch_id={branch_id}, reason={}",
            reason.as_str()
        );
    }
}

/// 快速检查的判定本体：只比较大小、修改时间与首尾采样，不读全文。
///
/// 路径资格错误（网络位置/设备命名空间）仍作为 `Err` 上报，与「宁可全量」的结论
/// 区分开；资格判定由调度器负责记录原因，这里只兜底不触碰路径。
fn quick_check(root: &Path, branch_id: &str) -> Result<QuickCheck, String> {
    let branch = history::load_branch(root, branch_id)?;
    // 没有工作文件或还没有任何提交时无从比较，必须走全量路径建立基线。
    if branch.source_path.trim().is_empty() || branch.head_history_id.is_none() {
        return Ok(QuickCheck::Fallback(QuickCheckFallback::NoBaseline));
    }
    if let Some(reason) = storage::unattended_source_path_rejection(&branch.source_path) {
        return Err(reason);
    }
    let Some((size, Some(expected))) = history::load_source_metadata(root, branch_id)? else {
        return Ok(QuickCheck::Fallback(QuickCheckFallback::NoBaseline));
    };
    let path = Path::new(&branch.source_path);
    let Ok(metadata) = path.metadata() else {
        return Ok(QuickCheck::Fallback(
            QuickCheckFallback::MetadataUnavailable,
        ));
    };
    if !metadata.is_file() {
        return Ok(QuickCheck::Fallback(QuickCheckFallback::NotARegularFile));
    }
    if metadata.len() != u64::try_from(size).unwrap_or(u64::MAX) {
        return Ok(QuickCheck::Fallback(QuickCheckFallback::SizeMismatch));
    }
    let Some(fingerprint) = source_fingerprint(path, &metadata) else {
        return Ok(QuickCheck::Fallback(QuickCheckFallback::SampleUnavailable));
    };
    if fingerprint != expected {
        return Ok(QuickCheck::Fallback(
            QuickCheckFallback::FingerprintMismatch,
        ));
    }
    Ok(QuickCheck::Unchanged)
}

/// 快速检查：指纹完全一致返回 true；其余情况（缺少基线、指纹不同、读取失败）
/// 返回 false，交给全量流程给出权威结论并自行修复或报错。退回全量的原因按分支
/// 去重记录，避免每次调度都刷一条同样的日志。
pub(crate) fn quick_check_unchanged(root: &Path, branch_id: &str) -> Result<bool, String> {
    match quick_check(root, branch_id)? {
        QuickCheck::Unchanged => {
            clear_fallback(branch_id);
            Ok(true)
        }
        QuickCheck::Fallback(reason) => {
            report_fallback(branch_id, reason);
            Ok(false)
        }
    }
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
    let started = Instant::now();
    // 一次提交只在既有那条结果日志里多带分段耗时：这些边界覆盖了这条路径上
    // 全部真实 I/O（读源文件、写快照、同步、提预览、写增量、落图），不新增行。
    let mut phase = started;
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
    ensure_not_device_path(&branch.source_path, "分支工作文件").map_err(BackupRunError::Failed)?;
    let source_path = Path::new(&branch.source_path);
    let before = source_metadata(source_path)?;
    let mut source =
        File::open(source_path).map_err(|error| format!("无法打开分支工作文件：{error}"))?;
    let artwork_directory = history::artwork_directory(root, &branch.artwork_id);
    let temp_directory = artwork_directory.join("temp");
    let mut snapshot_temp = NamedTempFile::new_in(&temp_directory)
        .map_err(|error| format!("无法创建 snapshot 临时文件：{error}"))?;
    let prepare_ms = phase.elapsed().as_millis();
    phase = Instant::now();
    let snapshot = create_snapshot(&mut source, snapshot_temp.as_file_mut(), options, cancel)?;
    let snapshot_ms = phase.elapsed().as_millis();
    phase = Instant::now();
    snapshot_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步 snapshot：{error}"))?;
    let snapshot_sync_ms = phase.elapsed().as_millis();
    phase = Instant::now();
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
        history::mark_unchanged(root, branch_id, checked_ms)?;
        // 内容已与 head 核对一致，记录基线元数据供快速检查使用。
        record_source_metadata(root, branch_id, &after_snapshot);
        log::info!(
            "backup unchanged: branch_id={branch_id}, history_id={}, logical_bytes={}, format={:?}, elapsed_ms={}, prepare_ms={prepare_ms}, snapshot_ms={snapshot_ms}, snapshot_sync_ms={snapshot_sync_ms}",
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
    let preview_ms = phase.elapsed().as_millis();
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
    // 增量的真实工作量是它携带的载荷字节（父版本相对本次变化的块），不是父文件
    // 大小；把它和耗时一起记下来，才能判断增量这一步是被读盘还是压缩限住的。
    let mut delta_input_bytes = 0_u64;
    let mut delta_ms = 0_u128;
    let mut delta_sync_ms = 0_u128;
    // 检查点父节点以 snapshot 作永久表示：子边增量不会被任何读取路径消费，
    // 因此不为它生成文件（取消检查点时由 `restore::build_reverse_delta` 重建）。
    // head 的 snapshot 会在本次提交中被释放，那条增量随后仍是父节点的唯一表示，
    // 所以只有「父是检查点」才跳过——这是释放判定的镜像，不能写成「父有 snapshot」。
    let parent_holds_checkpoint = match head.as_ref() {
        Some(previous) => history::checkpoint_snapshot_holder(root, &previous.id)?,
        None => false,
    };
    if let Some(previous) = head.as_ref().filter(|_| !parent_holds_checkpoint) {
        let previous_snapshot = previous
            .snapshot_path
            .as_deref()
            .ok_or("当前 head 缺少完整 snapshot")?;
        let previous_path = storage::resolve_path(root, previous_snapshot)?;
        let mut previous_file = File::open(&previous_path)
            .map_err(|error| format!("无法打开当前 snapshot：{error}"))?;
        let previous_chunk = ChunkFile::open(&mut previous_file)
            .map_err(|error| format!("无法读取当前 snapshot：{error}"))?;
        delta_input_bytes = snapshot.delta_payload_bytes(&previous_chunk);
        phase = Instant::now();
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
        delta_ms = phase.elapsed().as_millis();
        phase = Instant::now();
        temp.as_file()
            .sync_all()
            .map_err(|error| format!("无法同步 delta：{error}"))?;
        delta_sync_ms = phase.elapsed().as_millis();
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
    phase = Instant::now();
    publish_temp(snapshot_temp, &snapshot_final, "snapshot")?;
    if let (Some(temp), Some(final_path)) = (delta_temp, delta_final.as_ref()) {
        if let Err(error) = publish_temp(temp, final_path, "delta") {
            let _ = fs::remove_file(&snapshot_final);
            return Err(error.into());
        }
    }
    let publish_ms = phase.elapsed().as_millis();
    phase = Instant::now();
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
    let graph_ms = phase.elapsed().as_millis();
    // 提交成功，记录基线元数据供快速检查使用。
    record_source_metadata(root, branch_id, &after_preview);
    log::info!(
        "backup committed: branch_id={branch_id}, history_id={history_id}, commit_kind={commit_kind}, logical_bytes={committed_logical}, snapshot_bytes={committed_chunk}, delta_bytes={committed_delta:?}, format={:?}, chunking={:?}, elapsed_ms={}, prepare_ms={prepare_ms}, snapshot_ms={snapshot_ms}, snapshot_sync_ms={snapshot_sync_ms}, preview_ms={preview_ms}, delta_ms={delta_ms}, delta_input_bytes={delta_input_bytes}, delta_sync_ms={delta_sync_ms}, publish_ms={publish_ms}, graph_ms={graph_ms}",
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
    fn refresh_extracts_a_thumbnail_from_the_stored_node() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.png");
        let image =
            image::RgbaImage::from_fn(800, 400, |x, y| image::Rgba([x as u8, y as u8, 120, 255]));
        image.save(&source).unwrap();
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        let commit = run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        let history_id = commit.history_id.unwrap();

        // Simulate a node stored before the thumbnail feature: the work file is
        // irrelevant, the refresh must read the node's own materialized content.
        history::store_thumbnail(&root, &history_id, &[], 0).unwrap();
        std::fs::remove_file(&source).unwrap();
        assert!(history::load_thumbnail(&root, &history_id)
            .unwrap()
            .is_none());

        let thumbnail = super::super::restore::extract_node_thumbnail(
            &root,
            &history_id,
            &CancelToken::standalone(),
            |_, _, _| {},
        )
        .unwrap()
        .expect("the stored node still contains the image");
        assert_eq!(thumbnail.version, crate::preview::THUMBNAIL_VERSION);
        let decoded = image::load_from_memory(&thumbnail.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 256));
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
    fn quick_check_accepts_second_precision_mtime_baseline() {
        // `.clip` 这类由应用保存的文件只有整秒 mtime（亚秒位为 0）。旧版指纹要求
        // 亚秒精度，这类分支的基线永远为空、每次都静默退回全量——这里锁住修复。
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        let whole_second =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(whole_second)
            .unwrap();
        let nanos = fs::metadata(&source)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        assert_eq!(nanos, 0, "fixture must reproduce a coarse mtime");

        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert!(
            history::load_source_metadata(&root, &branch_id)
                .unwrap()
                .unwrap()
                .1
                .is_some(),
            "a coarse mtime must still produce a baseline fingerprint"
        );
        assert!(quick_check_unchanged(&root, &branch_id).unwrap());
    }

    #[test]
    fn quick_check_reports_why_it_falls_back() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        assert!(matches!(
            quick_check(&root, &branch_id).unwrap(),
            QuickCheck::Fallback(QuickCheckFallback::NoBaseline)
        ));

        run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(matches!(
            quick_check(&root, &branch_id).unwrap(),
            QuickCheck::Unchanged
        ));

        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        assert!(matches!(
            quick_check(&root, &branch_id).unwrap(),
            QuickCheck::Fallback(QuickCheckFallback::FingerprintMismatch)
        ));

        fs::write(&source, vec![b'B'; 96 * 1024 + 1]).unwrap();
        assert!(matches!(
            quick_check(&root, &branch_id).unwrap(),
            QuickCheck::Fallback(QuickCheckFallback::SizeMismatch)
        ));

        fs::remove_file(&source).unwrap();
        assert!(matches!(
            quick_check(&root, &branch_id).unwrap(),
            QuickCheck::Fallback(QuickCheckFallback::MetadataUnavailable)
        ));
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

    /// 检查点父节点不写反向增量；取消检查点时按需重建，重建后父节点仍可恢复。
    #[test]
    fn checkpoint_parent_skips_the_reverse_delta_and_cancel_rebuilds_it() {
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
        let checkpoint_id = first.history_id.unwrap();
        super::super::restore::ensure_checkpoint(
            &root,
            &checkpoint_id,
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
        let head_id = second.history_id.unwrap();

        // 首次提交是根节点，没有增量；第二次的父节点是检查点，同样不写。目录里
        // 应当一个增量文件都没有。
        let deltas = fs::read_dir(history::artwork_directory(&root, &artwork_id).join("deltas"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(deltas, 0, "检查点父节点不应产生反向增量");
        assert!(history::load_node(&root, &head_id)
            .unwrap()
            .delta_path
            .is_none());

        // 取消检查点：先在事务外重建增量，再把 snapshot 换掉。
        let plan = history::plan_checkpoint_release(&root, &checkpoint_id)
            .unwrap()
            .expect("节点应为检查点");
        assert!(plan.existing_delta.is_none(), "检查点不应已有增量");
        let (delta_relative, delta_size) = super::super::restore::build_reverse_delta(
            &root,
            &plan.child_id,
            &checkpoint_id,
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
            |_, _, _| {},
        )
        .unwrap();
        assert!(delta_size > 0);
        history::unmark_checkpoint(
            &root,
            &checkpoint_id,
            Some((delta_relative.as_str(), delta_size)),
        )
        .unwrap()
        .expect("检查点应被取消");

        let released = history::load_node(&root, &checkpoint_id).unwrap();
        assert!(released.snapshot_path.is_none(), "snapshot 应已释放");
        assert_eq!(
            released.delta_path.as_deref(),
            Some(delta_relative.as_str())
        );
        // 两条历史都必须还能逐字节恢复。
        assert_eq!(
            restore_and_read(&root, directory.path(), &checkpoint_id),
            vec![b'A'; 96 * 1024]
        );
        assert_eq!(
            restore_and_read(&root, directory.path(), &head_id),
            vec![b'B'; 96 * 1024]
        );
        // 全库校验走的是同一条链，也必须通过。
        assert_eq!(
            super::super::restore::scrub_history(&root, &CancelToken::standalone(), |_, _| {})
                .unwrap(),
            2
        );
    }

    /// 旧仓库可能留下「边仍记着增量路径、磁盘上的文件已经不在了」的记账。取消
    /// 检查点会按同一个文件名重建增量，这条刚写出的增量绝不能被当作旧增量入队清理。
    #[test]
    fn cancel_rebuild_keeps_the_delta_it_just_wrote() {
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
        let checkpoint_id = first.history_id.unwrap();
        super::super::restore::ensure_checkpoint(
            &root,
            &checkpoint_id,
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
        let head_id = second.history_id.unwrap();

        // 模拟旧仓库的陈旧记账：边仍写着增量路径，但文件已不在磁盘上。
        let stale = history::artwork_directory(&root, &artwork_id)
            .join("deltas")
            .join(format!("{head_id}-to-{checkpoint_id}.lbd"));
        let stale_relative = storage::relative_path(&root, &stale).unwrap();
        assert!(!stale.exists());
        storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE history_edges SET delta_path = ?2, delta_size = 7
                  WHERE child_history_id = ?1",
                [head_id.as_str(), stale_relative.as_str()],
            )
            .unwrap();

        let plan = history::plan_checkpoint_release(&root, &checkpoint_id)
            .unwrap()
            .expect("节点应为检查点");
        assert!(plan.existing_delta.is_none(), "文件缺失时必须重建");
        let (delta_relative, delta_size) = super::super::restore::build_reverse_delta(
            &root,
            &plan.child_id,
            &checkpoint_id,
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(delta_relative, stale_relative, "重建落回同名路径");
        history::unmark_checkpoint(
            &root,
            &checkpoint_id,
            Some((delta_relative.as_str(), delta_size)),
        )
        .unwrap()
        .expect("检查点应被取消");

        // 刚重建的增量必须还在，且边指向它，两条历史都能逐字节恢复。
        assert!(stale.is_file(), "重建出的增量被误当作旧增量入队清理");
        assert_eq!(
            history::load_node(&root, &checkpoint_id)
                .unwrap()
                .delta_path
                .as_deref(),
            Some(delta_relative.as_str())
        );
        assert_eq!(
            restore_and_read(&root, directory.path(), &checkpoint_id),
            vec![b'A'; 96 * 1024]
        );
        assert_eq!(
            restore_and_read(&root, directory.path(), &head_id),
            vec![b'B'; 96 * 1024]
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
