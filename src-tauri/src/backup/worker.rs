use std::{fs, fs::File, io, path::Path, time::SystemTime};

use tempfile::NamedTempFile;

use crate::{
    history::{self, HistoryCommit},
    storage,
};

use super::{
    chunk_file::{ChunkFile, ChunkingConfig},
    BackupCommitResult,
};

const MAX_TITLE_CHARS: usize = 160;

#[derive(Debug)]
pub(super) enum BackupRunError {
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

#[derive(Clone, Copy)]
struct SourceMetadata {
    length: u64,
    modified: Option<SystemTime>,
}

pub(crate) fn run_backup(
    root: &Path,
    branch_id: &str,
    note: &str,
    commit_kind: &str,
    cancelled: impl Fn() -> bool,
) -> Result<BackupCommitResult, BackupRunError> {
    if note.chars().count() > 500 {
        return Err(BackupRunError::Failed("提交备注不能超过 500 个字符".into()));
    }
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
    ensure_not_cancelled(&cancelled)?;
    history::ensure_directories(root, &branch.artwork_id)?;
    let source_path = Path::new(&branch.source_path);
    let before = source_metadata(source_path)?;
    let mut source =
        File::open(source_path).map_err(|error| format!("无法打开分支工作文件：{error}"))?;
    let artwork_directory = history::artwork_directory(root, &branch.artwork_id);
    let temp_directory = artwork_directory.join("temp");
    let mut snapshot_temp = NamedTempFile::new_in(&temp_directory)
        .map_err(|error| format!("无法创建 snapshot 临时文件：{error}"))?;
    let snapshot = ChunkFile::create(
        &mut source,
        snapshot_temp.as_file_mut(),
        ChunkingConfig::default(),
    )
    .map_err(|error| format!("无法创建文件快照：{error}"))?;
    snapshot_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步 snapshot：{error}"))?;
    ensure_not_cancelled(&cancelled)?;
    let after = source_metadata(source_path)?;
    if before.length != after.length || before.modified != after.modified {
        return Err(BackupRunError::Failed(
            "工作文件在读取期间发生变化，本次提交已取消".into(),
        ));
    }

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
        if let Err(validation_error) = validate_head_snapshot(root, head) {
            ensure_not_cancelled(&cancelled)?;
            log::warn!(
                "HEAD snapshot {} failed validation and will be rebuilt: {validation_error}",
                head.id
            );
            repair_head_snapshot(root, &artwork_directory, head, snapshot_temp)?;
        }
        history::mark_unchanged(root, branch_id, checked_ms)?;
        return Ok(BackupCommitResult {
            created: false,
            unchanged: true,
            history_id: None,
        });
    }

    let history_id = storage::new_id();
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
        snapshot
            .create_reverse_delta(&previous_chunk, &mut previous_file, temp.as_file_mut())
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

    ensure_not_cancelled(&cancelled)?;
    publish_temp(snapshot_temp, &snapshot_final, "snapshot")?;
    if let (Some(temp), Some(final_path)) = (delta_temp, delta_final.as_ref()) {
        if let Err(error) = publish_temp(temp, final_path, "delta") {
            let _ = fs::remove_file(&snapshot_final);
            return Err(error.into());
        }
    }
    if let Err(error) = ensure_not_cancelled(&cancelled) {
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
    };
    let old_snapshot = match history::commit(root, commit) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_file(&snapshot_final);
            if let Some(path) = delta_final.as_ref() {
                let _ = fs::remove_file(path);
            }
            return Err(error.into());
        }
    };
    if let Some(relative) = old_snapshot {
        let _ = fs::remove_file(storage::resolve_path(root, &relative)?);
    }
    Ok(BackupCommitResult {
        created: true,
        unchanged: false,
        history_id: Some(history_id),
    })
}

fn validate_head_snapshot(root: &Path, head: &history::HistoryRecord) -> Result<(), String> {
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
    snapshot
        .copy_original(&mut file, &mut io::sink())
        .map_err(|error| format!("当前 HEAD snapshot 完整性校验失败：{error}"))
}

fn repair_head_snapshot(
    root: &Path,
    artwork_directory: &Path,
    head: &history::HistoryRecord,
    snapshot_temp: NamedTempFile,
) -> Result<(), String> {
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
    if let Err(error) =
        history::set_snapshot(root, &head.id, &repaired_relative, repaired_size, true)
    {
        let _ = fs::remove_file(&repaired_path);
        return Err(format!("无法登记修复 snapshot：{error}"));
    }
    if let Some(previous_relative) = head.snapshot_path.as_deref() {
        if previous_relative != repaired_relative
            && matches!(
                history::storage_path_referenced(root, previous_relative),
                Ok(false)
            )
        {
            if let Ok(previous_path) = storage::resolve_path(root, previous_relative) {
                let _ = fs::remove_file(previous_path);
            }
        }
    }
    Ok(())
}

fn ensure_not_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), BackupRunError> {
    if cancelled() {
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

    use crate::{history, library};

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
        let artwork = library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
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
            || false,
            |_, _, _| {},
        )
        .unwrap();
        fs::read(output).unwrap()
    }

    #[test]
    fn commits_delta_and_restores_parent_bytes() {
        let (directory, root, source, artwork_id, branch_id) = create_fixture();

        let first = run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        let first_id = first.history_id.unwrap();
        fs::File::create(&source)
            .unwrap()
            .write_all(&[vec![b'A'; 48 * 1024], vec![b'B'; 48 * 1024]].concat())
            .unwrap();
        let second = run_backup(&root, &branch_id, "Second", "manual", || false).unwrap();
        assert!(second.created);

        let nodes = history::list(&root, &artwork_id).unwrap().nodes;
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            restore_and_read(&root, directory.path(), &first_id),
            vec![b'A'; 96 * 1024]
        );
        assert_eq!(
            super::super::restore::scrub_history(&root, || false, |_, _| {}).unwrap(),
            2
        );
    }

    #[test]
    fn cancellation_has_a_typed_result() {
        let (_directory, root, _source, _artwork_id, branch_id) = create_fixture();

        let error = run_backup(&root, &branch_id, "", "automatic", || true).unwrap_err();

        assert!(matches!(error, BackupRunError::Cancelled));
    }

    #[test]
    fn repository_scrub_rejects_a_damaged_delta() {
        let (_directory, root, source, _artwork_id, branch_id) = create_fixture();
        run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        fs::write(&source, vec![b'B'; 96 * 1024]).unwrap();
        let second = run_backup(&root, &branch_id, "Second", "manual", || false).unwrap();
        let delta_relative = history::load_node(&root, second.history_id.as_deref().unwrap())
            .unwrap()
            .delta_path
            .unwrap();
        fs::write(
            storage::resolve_path(&root, &delta_relative).unwrap(),
            b"damaged delta",
        )
        .unwrap();

        let error = super::super::restore::scrub_history(&root, || false, |_, _| {}).unwrap_err();

        assert!(error.contains("delta"), "{error}");
    }

    #[test]
    fn unchanged_backup_keeps_a_valid_head_snapshot() {
        let (_directory, root, _source, _artwork_id, branch_id) = create_fixture();
        let first = run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        let first_id = first.history_id.unwrap();
        let original_path = history::load_node(&root, &first_id).unwrap().snapshot_path;

        let second = run_backup(&root, &branch_id, "Second", "manual", || false).unwrap();

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
        let first = run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        let first_id = first.history_id.unwrap();
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        fs::remove_file(storage::resolve_path(&root, &original_relative).unwrap()).unwrap();

        let second = run_backup(&root, &branch_id, "Second", "manual", || false).unwrap();
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
        let first = run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        let first_id = first.history_id.unwrap();
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        let original_path = storage::resolve_path(&root, &original_relative).unwrap();
        let mut damaged = fs::read(&original_path).unwrap();
        *damaged.last_mut().unwrap() ^= 0xff;
        fs::write(&original_path, damaged).unwrap();

        let second = run_backup(&root, &branch_id, "Second", "manual", || false).unwrap();
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
        let first = run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
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
            || false,
            |_, _, _| {},
        )
        .unwrap_err();
        let checkpoint_error =
            super::super::restore::ensure_checkpoint(&root, &first_id).unwrap_err();

        assert!(restore_error.contains("snapshot"), "{restore_error}");
        assert!(checkpoint_error.contains("snapshot"), "{checkpoint_error}");
        assert!(!output.exists());
    }
}
