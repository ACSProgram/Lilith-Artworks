use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

use crate::{cleanup, history, storage};

use super::chunk_file::{ChunkFile, ChunkFileDelta, ChunkStore};

/// A materialization chain resolved into a chunk store.
///
/// Resolution never writes intermediate snapshots: each hop only appends the
/// delta payload to the store and rewrites the chunk layout, so the cost follows
/// the changed bytes of the chain instead of `chain length × file size`. Callers
/// destructure the chain, create a reader from the store, and every read path
/// verifies chunk digests and the whole-file digest.
struct ResolvedChain {
    layout: ChunkFile,
    store: ChunkStore<'static>,
    /// Chain nodes, including the starting snapshot. Progress is reported
    /// against this dimension so the final publication step can share it.
    steps: u64,
}

pub(crate) fn restore(
    root: &Path,
    history_id: &str,
    output_path: &str,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(&str, u64, u64),
) -> Result<String, String> {
    ensure_not_cancelled(&cancelled)?;
    let output = validate_output_path(root, output_path)?;
    let output_parent = output.parent().ok_or("恢复输出路径无效")?;
    let ResolvedChain {
        layout,
        mut store,
        steps,
    } = resolve_chain(
        root,
        history_id,
        "正在计算历史节点",
        &cancelled,
        &|label, current, total| progress(label, current, total),
    )?;
    ensure_not_cancelled(&cancelled)?;
    let mut output_temp = NamedTempFile::new_in(output_parent)
        .map_err(|error| format!("无法创建恢复输出：{error}"))?;
    progress("正在导出恢复文件", steps.saturating_sub(1), steps);
    {
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, output_temp.as_file_mut())
            .map_err(|error| format!("无法导出恢复文件：{error}"))?;
    }
    output_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步恢复文件：{error}"))?;
    let output_bytes = output_temp
        .as_file()
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    ensure_not_cancelled(&cancelled)?;
    output_temp
        .persist_noclobber(&output)
        .map_err(|error| format!("无法发布恢复文件：{}", error.error))?;
    log::info!(
        "restored history node: history_id={history_id}, chain_steps={steps}, output_bytes={output_bytes}, output={}",
        storage::display_path(&output)
    );
    progress("恢复完成", steps, steps);
    Ok(storage::display_path(&output))
}

/// Rebuilds the reverse delta of a removable intermediate node.
///
/// The removed node sits between `parent` and `child`, so the replacement delta
/// has to reconstruct `parent` from `child`. Both endpoints are resolved through
/// their own chain, and the child only contributes its chunk index: the payload
/// bytes that end up in the new delta are copied straight out of the resolved
/// parent store, which reads the changed chunks instead of materializing either
/// side as a full snapshot.
pub(crate) fn compact_node(
    root: &Path,
    history_id: &str,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(&str, u64, u64),
) -> Result<(), String> {
    let target = history::compaction_target(root, history_id)?;
    let parent_resolved = resolve_chain(
        root,
        &target.parent_id,
        "正在回溯保留节点",
        &cancelled,
        &|label, current, total| progress(label, current, total),
    )?;
    let child_layout = resolve_chain(
        root,
        &target.child_id,
        "正在回溯精简节点",
        &cancelled,
        &|label, current, total| progress(label, current, total),
    )?
    .layout;
    ensure_not_cancelled(&cancelled)?;

    let artwork_directory = history::artwork_directory(root, &target.artwork_id);
    let delta_directory = artwork_directory.join("deltas");
    fs::create_dir_all(&delta_directory)
        .map_err(|error| format!("无法创建精简 delta 目录：{error}"))?;
    let delta_name = format!("{}-to-{}.lbd", target.child_id, target.parent_id);
    let delta_final = delta_directory.join(delta_name);
    let delta_relative = storage::relative_path(root, &delta_final)?;
    let mut delta_temp = NamedTempFile::new_in(artwork_directory.join("temp"))
        .map_err(|error| format!("无法创建精简 delta：{error}"))?;
    let ResolvedChain {
        layout: parent_layout,
        mut store,
        ..
    } = parent_resolved;
    {
        let mut reader = store.reader();
        child_layout
            .create_reverse_delta(&parent_layout, &mut reader, delta_temp.as_file_mut())
            .map_err(|error| format!("无法重建精简 delta：{error}"))?;
    }
    delta_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步精简 delta：{error}"))?;
    ensure_not_cancelled(&cancelled)?;
    let delta_size = delta_temp
        .as_file()
        .metadata()
        .map_err(|error| format!("无法读取精简 delta 大小：{error}"))?
        .len();
    delta_temp
        .persist_noclobber(&delta_final)
        .map_err(|error| format!("无法发布精简 delta：{}", error.error))?;
    progress("正在改接历史链", 1, 1);
    let cleanup_ids = match history::apply_compaction(root, &target, &delta_relative, delta_size) {
        Ok(ids) => ids,
        Err(error) => {
            let _ = fs::remove_file(&delta_final);
            return Err(error);
        }
    };
    log::info!(
        "compacted history node: removed={}, parent={}, child={}, new_delta_bytes={delta_size}, queued_cleanup={}",
        target.node_id,
        target.parent_id,
        target.child_id,
        cleanup_ids.len()
    );
    // 被移除节点与旧边占用的文件已在改接事务内入队，提交成功后单遍重放；
    // 失败只留队列可重试，不改变精简的成功语义。
    cleanup::replay(root, &cleanup_ids);
    Ok(())
}

pub(crate) fn ensure_checkpoint(root: &Path, history_id: &str) -> Result<(), String> {
    ensure_checkpoint_with_progress(root, history_id, || false, |_, _, _| {})
}

pub(crate) fn scrub_history(
    root: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(u64, u64),
) -> Result<u64, String> {
    let node_ids = history::all_node_ids(root)?;
    let total = node_ids.len() as u64;
    for (index, history_id) in node_ids.iter().enumerate() {
        ensure_not_cancelled(&cancelled)?;
        // Resolving the chain already proves that every delta links to the
        // digest recorded in the database. Streaming the result into the sink
        // then verifies each chunk payload and the assembled file without
        // writing any temporary snapshot.
        let ResolvedChain {
            layout, mut store, ..
        } = resolve_chain(root, history_id, "正在校验历史", &cancelled, &|_, _, _| {})?;
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, &mut std::io::sink())
            .map_err(|error| format!("历史节点 {history_id} 完整性校验失败：{error}"))?;
        progress(index as u64 + 1, total);
    }
    Ok(total)
}

pub(crate) fn ensure_checkpoint_with_progress(
    root: &Path,
    history_id: &str,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(&str, u64, u64),
) -> Result<(), String> {
    ensure_not_cancelled(&cancelled)?;
    let target = history::load_node(root, history_id)?;
    if let Some(relative) = target.snapshot_path.as_deref() {
        let path = storage::resolve_path(root, relative)?;
        validate_snapshot(&path, &target, "检查点")?;
        history::mark_checkpoint(root, history_id)?;
        progress("检查点已就绪", 1, 1);
        return Ok(());
    }
    let ResolvedChain {
        layout, mut store, ..
    } = resolve_chain(
        root,
        history_id,
        "正在回溯并生成检查点",
        &cancelled,
        &|label, current, total| progress(label, current, total),
    )?;
    history::ensure_directories(root, &target.artwork_id)?;
    let artwork_directory = history::artwork_directory(root, &target.artwork_id);
    let final_path = artwork_directory
        .join("snapshots")
        .join(format!("{}.lbc", target.id));
    let relative = storage::relative_path(root, &final_path)?;
    let mut checkpoint = NamedTempFile::new_in(artwork_directory.join("temp"))
        .map_err(|error| format!("无法创建 checkpoint 临时文件：{error}"))?;
    {
        let mut reader = store.reader();
        layout
            .write_snapshot(&mut reader, checkpoint.as_file_mut())
            .map_err(|error| format!("无法写入 checkpoint：{error}"))?;
    }
    checkpoint
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步 checkpoint：{error}"))?;
    let file_size = checkpoint
        .as_file()
        .metadata()
        .map_err(|error| format!("无法读取 checkpoint 大小：{error}"))?
        .len();
    ensure_not_cancelled(&cancelled)?;
    checkpoint
        .persist_noclobber(&final_path)
        .map_err(|error| format!("无法发布 checkpoint：{}", error.error))?;
    let cleanup_ids = match history::set_snapshot(root, history_id, &relative, file_size, true) {
        Ok(ids) => ids,
        Err(error) => {
            let _ = fs::remove_file(&final_path);
            return Err(error);
        }
    };
    // 建立检查点时该节点原本没有 snapshot，正常返回空；若登记替换了旧 snapshot
    // （非空）同样在提交成功后重放。
    cleanup::replay(root, &cleanup_ids);
    log::info!("checkpoint published: history_id={history_id}, bytes={file_size}, path={relative}");
    progress("检查点已就绪", 1, 1);
    Ok(())
}

/// Resolves the reverse delta chain of `history_id` into a chunk store.
fn resolve_chain(
    root: &Path,
    history_id: &str,
    step_label: &str,
    cancelled: &impl Fn() -> bool,
    progress: &impl Fn(&str, u64, u64),
) -> Result<ResolvedChain, String> {
    ensure_not_cancelled(cancelled)?;
    let chain = history::materialization_chain(root, history_id)?;
    let first = chain.first().ok_or("恢复链为空")?;
    let snapshot_path = storage::resolve_path(
        root,
        first
            .snapshot_path
            .as_deref()
            .ok_or("恢复链起点缺少 snapshot")?,
    )?;
    let mut store = ChunkStore::new();
    store
        .append_file(
            File::open(&snapshot_path)
                .map_err(|error| format!("无法打开恢复链起点 snapshot：{error}"))?,
        )
        .map_err(|error| format!("无法读取恢复链起点 snapshot：{error}"))?;
    let mut layout = {
        let mut reader = store.reader();
        ChunkFile::open(&mut reader)
            .map_err(|error| format!("无法读取恢复链起点 snapshot：{error}"))?
    };
    if !layout
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&first.sha256)
    {
        return Err("恢复链起点 snapshot 摘要与历史数据库不匹配".into());
    }

    let total = chain.len() as u64;
    let temp_directory = root.join("temp");
    let mut steps = 1_u64;
    for (index, node) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(cancelled)?;
        let delta_path = node.delta_path.as_deref().ok_or("恢复链缺少反向 delta")?;
        let mut delta_file = File::open(storage::resolve_path(root, delta_path)?)
            .map_err(|error| format!("无法打开恢复链 delta：{error}"))?;
        let delta = ChunkFileDelta::open_in(&mut delta_file, &temp_directory)
            .map_err(|error| format!("无法读取恢复链 delta：{error}"))?;
        layout = delta
            .resolve(&layout, &mut store)
            .map_err(|error| format!("无法回溯历史链：{error}"))?;
        let expected = &chain[index + 1];
        if !layout
            .file_digest()
            .to_hex()
            .eq_ignore_ascii_case(&expected.sha256)
        {
            return Err(format!("历史节点 {} 的摘要与历史数据库不匹配", expected.id));
        }
        steps += 1;
        progress(step_label, index as u64 + 1, total);
    }
    Ok(ResolvedChain {
        layout,
        store,
        steps,
    })
}

/// 校验单个 snapshot 文件与其历史记录一致：先比对 ChunkFile 的文件摘要与数据库
/// 记录的 `sha256`，再把全部分块流式读出以验证载荷。恢复、检查点与空闲链路校验
/// 复用该入口；分支 head 恒持有 snapshot，因此校验 head 等价于校验这一个文件。
pub(crate) fn validate_snapshot(
    path: &Path,
    record: &history::HistoryRecord,
    label: &str,
) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("无法打开{label} snapshot：{error}"))?;
    let snapshot =
        ChunkFile::open(&mut file).map_err(|error| format!("无法读取{label} snapshot：{error}"))?;
    if !snapshot
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&record.sha256)
    {
        return Err(format!("{label} snapshot 摘要与历史数据库不匹配"));
    }
    snapshot
        .copy_original(&mut file, &mut std::io::sink())
        .map_err(|error| format!("{label} snapshot 完整性校验失败：{error}"))
}

fn ensure_not_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("恢复操作已取消".into())
    } else {
        Ok(())
    }
}

fn validate_output_path(root: &Path, value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.trim());
    if !path.is_absolute() {
        return Err("恢复输出必须使用绝对路径".into());
    }
    if path.exists() {
        return Err("恢复输出路径已经存在".into());
    }
    let parent = path.parent().ok_or("恢复输出路径无效")?;
    if !parent.is_dir() {
        return Err("恢复输出目录不存在".into());
    }
    if parent
        .canonicalize()
        .map_err(|error| format!("无法访问恢复目录：{error}"))?
        .starts_with(
            root.canonicalize()
                .map_err(|error| format!("无法访问作品仓库：{error}"))?,
        )
    {
        return Err("恢复文件不能写入作品仓库内部".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::backup::chunk_file::ChunkingConfig;

    /// 生成一个合法 snapshot 文件的内容与它记录在数据库里的 `sha256`。
    fn snapshot_bytes(bytes: &[u8]) -> (Vec<u8>, String) {
        let mut output = Cursor::new(Vec::new());
        let chunk_file = ChunkFile::create(
            &mut Cursor::new(bytes),
            &mut output,
            ChunkingConfig::default(),
        )
        .unwrap();
        (output.into_inner(), chunk_file.file_digest().to_hex())
    }

    fn record(sha256: String) -> history::HistoryRecord {
        history::HistoryRecord {
            id: "node".into(),
            artwork_id: "artwork".into(),
            parent_id: None,
            sha256,
            snapshot_path: None,
            delta_path: None,
        }
    }

    #[test]
    fn validate_snapshot_accepts_a_matching_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.lbc");
        let (bytes, digest) = snapshot_bytes(&[b'A'; 64 * 1024]);
        fs::write(&path, bytes).unwrap();

        validate_snapshot(&path, &record(digest), "检查点").unwrap();
    }

    #[test]
    fn validate_snapshot_rejects_a_mismatched_digest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.lbc");
        let (bytes, _) = snapshot_bytes(&[b'B'; 64 * 1024]);
        fs::write(&path, bytes).unwrap();

        let error = validate_snapshot(&path, &record("0".repeat(64)), "检查点").unwrap_err();
        assert!(error.contains("摘要"), "{error}");
    }

    #[test]
    fn validate_snapshot_rejects_a_replaced_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.lbc");
        let (bytes, digest) = snapshot_bytes(&[b'C'; 64 * 1024]);
        fs::write(&path, bytes).unwrap();
        fs::write(&path, b"replacement").unwrap();

        let error = validate_snapshot(&path, &record(digest), "检查点").unwrap_err();
        assert!(error.contains("snapshot"), "{error}");
    }
}
