use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::Instant,
};

use tempfile::NamedTempFile;

use crate::{
    cancel::CancelToken, cleanup, history, platform::path::display_path, preview, storage,
};

use super::chunk_file::{ChunkFile, ChunkFileDelta, ChunkStore, ChunkTask, ChunkWriteOptions};

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
    /// Chain nodes, including the starting snapshot. Chain resolution reports
    /// progress against this dimension; the export phase afterwards reports
    /// logical bytes instead, because that is what the caller can size a bar by.
    steps: u64,
}

pub(crate) fn restore(
    root: &Path,
    history_id: &str,
    output_path: &str,
    cancel: &CancelToken,
    progress: impl Fn(&str, u64, u64),
) -> Result<String, String> {
    ensure_not_cancelled(cancel)?;
    let started = Instant::now();
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
        cancel,
        &|label, current, total| progress(label, current, total),
    )?;
    // 分段耗时：链解析包含打开 snapshot、逐跳解压 delta 并重排块表，与后面的
    // 导出、同步分开记，才能看出恢复是卡在解压还是卡在写盘。
    let resolve_ms = started.elapsed().as_millis();
    let export_started = Instant::now();
    ensure_not_cancelled(cancel)?;
    let mut output_temp = NamedTempFile::new_in(output_parent)
        .map_err(|error| format!("无法创建恢复输出：{error}"))?;
    let logical_size = layout.logical_size();
    progress("正在导出恢复文件", 0, logical_size);
    {
        // 回调收到的是累计导出字节，可以直接作为进度条的 current。
        let export_progress = |processed| progress("正在导出恢复文件", processed, logical_size);
        let task = ChunkTask::new(cancel, Some(&export_progress));
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, output_temp.as_file_mut(), &task)
            .map_err(|error| format!("无法导出恢复文件：{error}"))?;
    }
    let export_ms = export_started.elapsed().as_millis();
    let sync_started = Instant::now();
    output_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步恢复文件：{error}"))?;
    let sync_ms = sync_started.elapsed().as_millis();
    let output_bytes = output_temp
        .as_file()
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    ensure_not_cancelled(cancel)?;
    let publish_started = Instant::now();
    output_temp
        .persist_noclobber(&output)
        .map_err(|error| format!("无法发布恢复文件：{}", error.error))?;
    let publish_ms = publish_started.elapsed().as_millis();
    log::info!(
        "restored history node: history_id={history_id}, chain_steps={steps}, output_bytes={output_bytes}, elapsed_ms={}, resolve_ms={resolve_ms}, export_ms={export_ms}, sync_ms={sync_ms}, publish_ms={publish_ms}, output={}",
        started.elapsed().as_millis(),
        display_path(&output)
    );
    progress("恢复完成", steps, steps);
    Ok(display_path(&output))
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
    cancel: &CancelToken,
    options: ChunkWriteOptions,
    progress: impl Fn(&str, u64, u64),
) -> Result<(), String> {
    let target = history::compaction_target(root, history_id)?;
    // 父节点是检查点时它的 snapshot 永久保留，改接后的子边增量不会被任何读取
    // 路径消费：只改接图，不写文件（与提交路径同一口径）。
    let parent_holds_checkpoint = history::checkpoint_snapshot_holder(root, &target.parent_id)?;
    let delta = if parent_holds_checkpoint {
        None
    } else {
        Some(write_reverse_delta(
            root,
            &target.child_id,
            &target.parent_id,
            "正在回溯保留节点",
            "正在回溯精简节点",
            cancel,
            options,
            &progress,
        )?)
    };
    let (delta_relative, delta_size) = delta
        .as_ref()
        .map_or(("", 0_u64), |(relative, size)| (relative.as_str(), *size));
    progress("正在改接历史链", 1, 1);
    let cleanup_ids = match history::apply_compaction(root, &target, delta_relative, delta_size) {
        Ok(ids) => ids,
        Err(error) => {
            if !delta_relative.is_empty() {
                if let Ok(path) = storage::resolve_path(root, delta_relative) {
                    let _ = fs::remove_file(path);
                }
            }
            return Err(error);
        }
    };
    log::info!(
        "compacted history node: removed={}, parent={}, child={}, new_delta_bytes={delta_size}, format={:?}, queued_cleanup={}",
        target.node_id,
        target.parent_id,
        target.child_id,
        options.format,
        cleanup_ids.len()
    );
    // 被移除节点与旧边占用的文件已在改接事务内入队，提交成功后单遍重放；
    // 失败只留队列可重试，不改变精简的成功语义。
    cleanup::replay(root, &cleanup_ids);
    Ok(())
}

/// Re-extract a node's derived preview from that node's stored content.
///
/// The current work file may have moved on, so the refresh path materializes the
/// requested node into the repository temp directory and lets `preview` read it.
/// Nothing is written back to the graph here; the caller persists the returned
/// payload and any extraction failure simply yields `None`.
pub(crate) fn extract_node_thumbnail(
    root: &Path,
    history_id: &str,
    cancel: &CancelToken,
    progress: impl Fn(&str, u64, u64),
) -> Result<Option<preview::Thumbnail>, String> {
    ensure_not_cancelled(cancel)?;
    let ResolvedChain {
        layout, mut store, ..
    } = resolve_chain(root, history_id, "正在计算历史节点", cancel, &progress)?;
    ensure_not_cancelled(cancel)?;
    // Use the repository-level temp directory: a crash leaves only a `.tmp*`
    // scratch file, which the startup sweep removes once it is stale.
    let temp_directory = root.join("temp");
    fs::create_dir_all(&temp_directory)
        .map_err(|error| format!("无法创建缩略图临时目录：{error}"))?;
    let mut materialized = NamedTempFile::new_in(&temp_directory)
        .map_err(|error| format!("无法创建缩略图临时文件：{error}"))?;
    let logical_size = layout.logical_size();
    progress("正在准备节点内容", 0, logical_size);
    {
        let export_progress = |processed| progress("正在准备节点内容", processed, logical_size);
        let task = ChunkTask::new(cancel, Some(&export_progress));
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, materialized.as_file_mut(), &task)
            .map_err(|error| format!("无法读取历史节点内容：{error}"))?;
    }
    materialized
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步缩略图临时文件：{error}"))?;
    ensure_not_cancelled(cancel)?;
    progress("正在提取缩略图", 0, 1);
    let thumbnail = preview::extract(materialized.path(), cancel);
    progress("缩略图提取完成", 1, 1);
    Ok(thumbnail)
}

/// 从 `child_id` 的物化结果重建父节点 `parent_id` 的反向增量。
///
/// 写入 `deltas/<child>-to-<parent>.lbd` 并返回仓库内相对路径与字节数。调用方
/// 负责在随后的数据库事务失败时删除它；事务成功后它由子边引用。取消检查点用它
/// 把「只有 snapshot 的检查点」换回「只有反向增量的普通节点」。
pub(crate) fn build_reverse_delta(
    root: &Path,
    child_id: &str,
    parent_id: &str,
    cancel: &CancelToken,
    options: ChunkWriteOptions,
    progress: impl Fn(&str, u64, u64),
) -> Result<(String, u64), String> {
    write_reverse_delta(
        root,
        child_id,
        parent_id,
        "正在回溯父节点",
        "正在回溯子节点",
        cancel,
        options,
        &progress,
    )
}

/// 重建反向增量的共用内核：两边都按各自的链物化，只有子节点贡献块索引，
/// 载荷字节直接从父节点的物化结果里读，不在磁盘上写任何中间 snapshot。
fn write_reverse_delta(
    root: &Path,
    child_id: &str,
    parent_id: &str,
    parent_label: &str,
    child_label: &str,
    cancel: &CancelToken,
    options: ChunkWriteOptions,
    progress: &impl Fn(&str, u64, u64),
) -> Result<(String, u64), String> {
    let ResolvedChain {
        layout: parent_layout,
        mut store,
        ..
    } = resolve_chain(
        root,
        parent_id,
        parent_label,
        cancel,
        &|label, current, total| progress(label, current, total),
    )?;
    let child_layout = resolve_chain(
        root,
        child_id,
        child_label,
        cancel,
        &|label, current, total| progress(label, current, total),
    )?
    .layout;
    ensure_not_cancelled(cancel)?;

    // 父节点的作品目录就是增量目录的归属：两边同属一条历史链，必然同作品。
    let artwork_directory =
        history::artwork_directory(root, &history::load_node(root, parent_id)?.artwork_id);
    let delta_directory = artwork_directory.join("deltas");
    fs::create_dir_all(&delta_directory).map_err(|error| format!("无法创建增量目录：{error}"))?;
    let delta_final = delta_directory.join(format!("{child_id}-to-{parent_id}.lbd"));
    let delta_relative = storage::relative_path(root, &delta_final)?;
    let mut delta_temp = NamedTempFile::new_in(artwork_directory.join("temp"))
        .map_err(|error| format!("无法创建增量临时文件：{error}"))?;
    // 增量只写父版本相对子版本变化的块，用父文件大小做总量会让进度永远停在低位；
    // 因此总量取本次真正要复制的载荷字节，至少为 1 以免出现零分母。
    let delta_total = child_layout.delta_payload_bytes(&parent_layout).max(1);
    {
        let delta_progress = |processed| progress("正在重建反向增量", processed, delta_total);
        let task = ChunkTask::new(cancel, Some(&delta_progress));
        let mut reader = store.reader();
        child_layout
            .create_reverse_delta(
                &parent_layout,
                &mut reader,
                delta_temp.as_file_mut(),
                options,
                &task,
            )
            .map_err(|error| format!("无法重建反向增量：{error}"))?;
    }
    delta_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步增量：{error}"))?;
    ensure_not_cancelled(cancel)?;
    let delta_size = delta_temp
        .as_file()
        .metadata()
        .map_err(|error| format!("无法读取增量大小：{error}"))?
        .len();
    // 同名文件只可能是上一次中断的重建或待清理的遗留（边此刻没有指向它），
    // 图是唯一事实，先移除再发布。
    let _ = fs::remove_file(&delta_final);
    delta_temp
        .persist_noclobber(&delta_final)
        .map_err(|error| format!("无法发布增量：{}", error.error))?;
    Ok((delta_relative, delta_size))
}

pub(crate) fn ensure_checkpoint(
    root: &Path,
    history_id: &str,
    cancel: &CancelToken,
    options: ChunkWriteOptions,
) -> Result<(), String> {
    ensure_checkpoint_with_progress(root, history_id, cancel, options, |_, _, _| {})
}

/// 物化单个历史节点并校验其完整性：沿链解析、与数据库摘要比对、逐块验哈希。
///
/// 这是全库扫描、空闲增量批次与手动重查共用的唯一校验内核。只读，不写任何状态；
/// 调用方负责把结果写入节点级校验列。
pub(crate) fn verify_history_node(
    root: &Path,
    history_id: &str,
    cancel: &CancelToken,
) -> Result<(), String> {
    ensure_not_cancelled(cancel)?;
    // Resolving the chain already proves that every delta links to the
    // digest recorded in the database. Streaming the result into the sink
    // then verifies each chunk payload and the assembled file without
    // writing any temporary snapshot.
    let ResolvedChain {
        layout, mut store, ..
    } = resolve_chain(root, history_id, "正在校验历史", cancel, &|_, _, _| {})
        .map_err(|error| format!("历史节点 {history_id} 完整性校验失败：{error}"))?;
    ensure_not_cancelled(cancel)?;
    let task = ChunkTask::cancel(cancel);
    let mut reader = store.reader();
    layout
        .copy_original(&mut reader, &mut std::io::sink(), &task)
        .map_err(|error| format!("历史节点 {history_id} 完整性校验失败：{error}"))
}

/// 全库扫描：逐节点物化并验证，同时刷新节点级校验状态。
///
/// 单个节点失败不会中止扫描——否则失败节点之后的节点永远拿不到新的校验状态。
/// 全部分块流式读取，不写临时快照；至少一个节点失败时汇总为错误返回。
pub(crate) fn scrub_history(
    root: &Path,
    cancel: &CancelToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, String> {
    let node_ids = history::all_node_ids(root)?;
    let total = node_ids.len() as u64;
    let mut first_error: Option<String> = None;
    let mut failures = 0_u64;
    for (index, history_id) in node_ids.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        match verify_history_node(root, history_id, cancel) {
            Ok(()) => {
                let verified_ms = storage::now_ms()?;
                history::mark_node_verified(root, history_id, verified_ms)?;
            }
            Err(error) => {
                if cancel.is_cancelled() {
                    return Err("操作已取消".into());
                }
                log::warn!("repository scrub failed for history node {history_id}: {error}");
                history::mark_node_verify_error(root, history_id, &error)?;
                failures += 1;
                first_error.get_or_insert(error);
            }
        }
        progress(index as u64 + 1, total);
    }
    match first_error {
        None => Ok(total),
        Some(error) if failures == 1 => Err(error),
        Some(error) => Err(format!(
            "{error}（另有 {} 个历史节点校验失败）",
            failures - 1
        )),
    }
}

pub(crate) fn ensure_checkpoint_with_progress(
    root: &Path,
    history_id: &str,
    cancel: &CancelToken,
    options: ChunkWriteOptions,
    progress: impl Fn(&str, u64, u64),
) -> Result<(), String> {
    ensure_not_cancelled(cancel)?;
    let target = history::load_node(root, history_id)?;
    if let Some(relative) = target.snapshot_path.as_deref() {
        let path = storage::resolve_path(root, relative)?;
        validate_snapshot(&path, &target, "检查点", cancel)?;
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
        cancel,
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
    let logical_size = layout.logical_size();
    {
        // 回调收到的是累计写入字节，可以直接作为进度条的 current。
        let checkpoint_progress = |processed| {
            progress("正在写入检查点", processed, logical_size);
        };
        let task = ChunkTask::new(cancel, Some(&checkpoint_progress));
        let mut reader = store.reader();
        layout
            .write_snapshot(&mut reader, checkpoint.as_file_mut(), options, &task)
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
    ensure_not_cancelled(cancel)?;
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
    log::info!(
        "checkpoint published: history_id={history_id}, bytes={file_size}, format={:?}, path={relative}",
        options.format
    );
    progress("检查点已就绪", 1, 1);
    Ok(())
}

/// Resolves the reverse delta chain of `history_id` into a chunk store.
fn resolve_chain(
    root: &Path,
    history_id: &str,
    step_label: &str,
    cancel: &CancelToken,
    progress: &impl Fn(&str, u64, u64),
) -> Result<ResolvedChain, String> {
    ensure_not_cancelled(cancel)?;
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
    let task = ChunkTask::cancel(cancel);
    let mut steps = 1_u64;
    for (index, node) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(cancel)?;
        let delta_path = node
            .delta_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| format!("历史节点 {} 缺少指向父节点的反向增量", node.id))?;
        let mut delta_file = File::open(storage::resolve_path(root, delta_path)?)
            .map_err(|error| format!("无法打开恢复链 delta：{error}"))?;
        let delta = ChunkFileDelta::open_in(&mut delta_file, &temp_directory, &task)
            .map_err(|error| format!("无法读取恢复链 delta：{error}"))?;
        layout = delta
            .resolve(&layout, &mut store, &task)
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
/// 记录的 `sha256`，再把全部分块流式读出以验证载荷。
///
/// 只有「检查点已持有 snapshot」这条快路径用它；节点级校验内核是
/// [`verify_history_node`]，它走完整的惰性解析路径，能覆盖 snapshot 之后的 delta。
fn validate_snapshot(
    path: &Path,
    record: &history::HistoryRecord,
    label: &str,
    cancel: &CancelToken,
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
    let task = ChunkTask::cancel(cancel);
    snapshot
        .copy_original(&mut file, &mut std::io::sink(), &task)
        .map_err(|error| format!("{label} snapshot 完整性校验失败：{error}"))
}

fn ensure_not_cancelled(cancel: &CancelToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        // 恢复、精简、检查点与删除子树共用这条检查，因此文案保持中性：
        // 写成「恢复操作」会在精简或检查点取消时误报。
        Err("操作已取消".into())
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
    use std::{
        fs,
        io::Cursor,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    use super::*;
    use crate::{
        backup::chunk_file::{ChunkWriteOptions, ChunkingConfig},
        library,
    };

    /// 生成一个合法 snapshot 文件的内容与它记录在数据库里的 `sha256`。
    fn snapshot_bytes(bytes: &[u8]) -> (Vec<u8>, String) {
        let mut output = Cursor::new(Vec::new());
        let task = ChunkTask::untracked();
        let chunk_file = ChunkFile::create(
            &mut Cursor::new(bytes),
            &mut output,
            ChunkingConfig::default(),
            ChunkWriteOptions::V1,
            &task,
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

        validate_snapshot(&path, &record(digest), "检查点", &CancelToken::standalone()).unwrap();
    }

    #[test]
    fn validate_snapshot_rejects_a_mismatched_digest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.lbc");
        let (bytes, _) = snapshot_bytes(&[b'B'; 64 * 1024]);
        fs::write(&path, bytes).unwrap();

        let error = validate_snapshot(
            &path,
            &record("0".repeat(64)),
            "检查点",
            &CancelToken::standalone(),
        )
        .unwrap_err();
        assert!(error.contains("摘要"), "{error}");
    }

    #[test]
    fn validate_snapshot_rejects_a_replaced_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.lbc");
        let (bytes, digest) = snapshot_bytes(&[b'C'; 64 * 1024]);
        fs::write(&path, bytes).unwrap();
        fs::write(&path, b"replacement").unwrap();

        let error = validate_snapshot(&path, &record(digest), "检查点", &CancelToken::standalone())
            .unwrap_err();
        assert!(error.contains("snapshot"), "{error}");
    }

    /// 导出管道在取消后不得发布半成品：输出路径要么完整存在，要么完全不存在。
    ///
    /// 取消由导出进度回调触发，覆盖的是「已经写出一部分临时文件」的那一刻；
    /// 这里断言调用方拿到取消错误，且最终路径没有被创建。
    #[test]
    fn cancelled_export_does_not_publish_the_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        let bytes: Vec<u8> = (0..5 * 1024 * 1024).map(|index| index as u8).collect();
        fs::write(&source, &bytes).unwrap();
        library::initialize(&root).unwrap();
        let artwork =
            library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        let commit = super::super::worker::run_backup(
            &root,
            &artwork.branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();

        let cancelled = Arc::new(AtomicBool::new(false));
        let token = CancelToken::with_source({
            let cancelled = cancelled.clone();
            move || cancelled.load(Ordering::SeqCst)
        });
        let output = directory.path().join("restored.bin");
        let error = super::restore(
            &root,
            commit.history_id.as_deref().unwrap(),
            output.to_str().unwrap(),
            &token,
            |_, processed, _| {
                if processed > 0 {
                    cancelled.store(true, Ordering::SeqCst);
                }
            },
        )
        .unwrap_err();

        assert!(error.contains("取消"), "{error}");
        assert!(!output.exists(), "取消的恢复不得留下发布文件");
    }
}
