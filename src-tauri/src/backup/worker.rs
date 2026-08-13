use std::{fs, fs::File, path::Path, time::SystemTime};

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
) -> Result<BackupCommitResult, String> {
    if note.chars().count() > 500 {
        return Err("提交备注不能超过 500 个字符".into());
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
        return Err("工作文件在读取期间发生变化，本次提交已取消".into());
    }

    let digest = snapshot.file_digest().to_hex();
    let checked_ms = storage::now_ms()?;
    let head = branch
        .head_history_id
        .as_deref()
        .map(|id| history::load_node(root, id))
        .transpose()?;
    if head
        .as_ref()
        .is_some_and(|node| node.sha256.eq_ignore_ascii_case(&digest))
    {
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
            return Err(error);
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
            return Err(error);
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

fn ensure_not_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("备份操作已取消".into())
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

    #[test]
    fn commits_delta_and_restores_parent_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::File::create(&source)
            .unwrap()
            .write_all(&vec![b'A'; 96 * 1024])
            .unwrap();
        library::initialize(&root).unwrap();
        let artwork = library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();

        let first = run_backup(&root, &artwork.branch_id, "First", "manual", || false).unwrap();
        let first_id = first.history_id.unwrap();
        fs::File::create(&source)
            .unwrap()
            .write_all(&[vec![b'A'; 48 * 1024], vec![b'B'; 48 * 1024]].concat())
            .unwrap();
        let second = run_backup(&root, &artwork.branch_id, "Second", "manual", || false).unwrap();
        assert!(second.created);

        let nodes = history::list(&root, &artwork.artwork_id).unwrap().nodes;
        assert_eq!(nodes.len(), 2);
        let output = directory.path().join("restored.bin");
        super::super::restore::restore(
            &root,
            &first_id,
            output.to_str().unwrap(),
            || false,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(fs::read(output).unwrap(), vec![b'A'; 96 * 1024]);
    }
}
