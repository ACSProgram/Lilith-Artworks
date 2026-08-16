use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

use crate::{history, storage};

use super::chunk_file::{ChunkFile, ChunkFileDelta};

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
    let chain = history::materialization_chain(root, history_id)?;
    let first = chain.first().ok_or("恢复链为空")?;
    let snapshot_relative = first
        .snapshot_path
        .as_deref()
        .ok_or("恢复链起点缺少 snapshot")?;
    let mut current_path = storage::resolve_path(root, snapshot_relative)?;
    validate_snapshot(&current_path, first, "恢复链起点")?;
    let total = chain.len() as u64;
    let temp_directory = root.join("temp");
    let mut temporaries = Vec::new();
    for (index, child) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(&cancelled)?;
        let delta_path = child.delta_path.as_deref().ok_or("恢复链缺少反向 delta")?;
        let mut base_file =
            File::open(&current_path).map_err(|error| format!("无法打开恢复 snapshot：{error}"))?;
        let base = ChunkFile::open(&mut base_file)
            .map_err(|error| format!("无法读取恢复 snapshot：{error}"))?;
        let mut delta_file = File::open(storage::resolve_path(root, delta_path)?)
            .map_err(|error| format!("无法打开恢复 delta：{error}"))?;
        let mut delta = ChunkFileDelta::open_in(&mut delta_file, &temp_directory)
            .map_err(|error| format!("无法读取恢复 delta：{error}"))?;
        let mut next = NamedTempFile::new_in(&temp_directory)
            .map_err(|error| format!("无法创建恢复临时文件：{error}"))?;
        let restored = delta
            .apply(&base, &mut base_file, next.as_file_mut())
            .map_err(|error| format!("无法应用恢复 delta：{error}"))?;
        validate_digest(&restored, &chain[index + 1], "恢复链节点")?;
        next.as_file()
            .sync_all()
            .map_err(|error| format!("无法同步恢复临时文件：{error}"))?;
        current_path = next.path().to_owned();
        temporaries.push(next);
        progress("正在计算历史节点", index as u64 + 1, total);
    }
    ensure_not_cancelled(&cancelled)?;
    let mut snapshot_file =
        File::open(&current_path).map_err(|error| format!("无法打开目标 snapshot：{error}"))?;
    let snapshot = ChunkFile::open(&mut snapshot_file)
        .map_err(|error| format!("无法读取目标 snapshot：{error}"))?;
    let mut output_temp = NamedTempFile::new_in(output_parent)
        .map_err(|error| format!("无法创建恢复输出：{error}"))?;
    progress("正在导出恢复文件", total.saturating_sub(1), total);
    snapshot
        .copy_original(&mut snapshot_file, output_temp.as_file_mut())
        .map_err(|error| format!("无法导出恢复文件：{error}"))?;
    output_temp
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步恢复文件：{error}"))?;
    ensure_not_cancelled(&cancelled)?;
    output_temp
        .persist_noclobber(&output)
        .map_err(|error| format!("无法发布恢复文件：{}", error.error))?;
    progress("恢复完成", total, total);
    Ok(storage::display_path(&output))
}

pub(crate) fn materialize_snapshot(
    root: &Path,
    history_id: &str,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(&str, u64, u64),
) -> Result<NamedTempFile, String> {
    ensure_not_cancelled(&cancelled)?;
    let chain = history::materialization_chain(root, history_id)?;
    let first = chain.first().ok_or("恢复链为空")?;
    let mut current_path = storage::resolve_path(
        root,
        first
            .snapshot_path
            .as_deref()
            .ok_or("恢复链起点缺少 snapshot")?,
    )?;
    validate_snapshot(&current_path, first, "物化链起点")?;
    let total = chain.len() as u64;
    let temp_directory = root.join("temp");
    let mut temporaries = Vec::new();
    for (index, child) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(&cancelled)?;
        let mut base_file =
            File::open(&current_path).map_err(|error| format!("无法打开 snapshot：{error}"))?;
        let base = ChunkFile::open(&mut base_file)
            .map_err(|error| format!("无法读取 snapshot：{error}"))?;
        let delta_path = child.delta_path.as_deref().ok_or("恢复链缺少反向 delta")?;
        let mut delta_file = File::open(storage::resolve_path(root, delta_path)?)
            .map_err(|error| format!("无法打开 delta：{error}"))?;
        let mut delta = ChunkFileDelta::open_in(&mut delta_file, &temp_directory)
            .map_err(|error| format!("无法读取 delta：{error}"))?;
        let mut next = NamedTempFile::new_in(&temp_directory)
            .map_err(|error| format!("无法创建临时 snapshot：{error}"))?;
        let restored = delta
            .apply(&base, &mut base_file, next.as_file_mut())
            .map_err(|error| format!("无法应用 delta：{error}"))?;
        validate_digest(&restored, &chain[index + 1], "物化链节点")?;
        next.as_file()
            .sync_all()
            .map_err(|error| format!("无法同步临时 snapshot：{error}"))?;
        current_path = next.path().to_owned();
        temporaries.push(next);
        progress("正在准备精简历史", index as u64 + 1, total);
    }
    if let Some(last) = temporaries.pop() {
        progress("精简基础已准备", total, total);
        return Ok(last);
    }
    let mut result = NamedTempFile::new_in(&temp_directory)
        .map_err(|error| format!("无法创建精简基础文件：{error}"))?;
    let mut source =
        File::open(&current_path).map_err(|error| format!("无法打开精简基础 snapshot：{error}"))?;
    std::io::copy(&mut source, result.as_file_mut())
        .map_err(|error| format!("无法复制精简基础 snapshot：{error}"))?;
    result
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步精简基础文件：{error}"))?;
    progress("精简基础已准备", total, total);
    Ok(result)
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
        let snapshot = materialize_snapshot(root, history_id, &cancelled, |_, _, _| {})?;
        drop(snapshot);
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
    let chain = history::materialization_chain(root, history_id)?;
    let first = chain.first().ok_or("checkpoint 恢复链为空")?;
    let mut current_path = storage::resolve_path(
        root,
        first
            .snapshot_path
            .as_deref()
            .ok_or("恢复链起点缺少 snapshot")?,
    )?;
    validate_snapshot(&current_path, first, "检查点恢复链起点")?;
    let total = chain.len().max(1) as u64;
    progress("正在准备检查点历史链", 0, total);
    let temp_directory = root.join("temp");
    let mut temporaries = Vec::new();
    for (index, child) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(&cancelled)?;
        let mut base_file = File::open(&current_path)
            .map_err(|error| format!("无法打开 checkpoint snapshot：{error}"))?;
        let base = ChunkFile::open(&mut base_file)
            .map_err(|error| format!("无法读取 checkpoint snapshot：{error}"))?;
        let delta_relative = child
            .delta_path
            .as_deref()
            .ok_or("checkpoint 恢复链缺少 delta")?;
        let mut delta_file = File::open(storage::resolve_path(root, delta_relative)?)
            .map_err(|error| format!("无法打开 checkpoint delta：{error}"))?;
        let mut delta = ChunkFileDelta::open_in(&mut delta_file, &temp_directory)
            .map_err(|error| format!("无法读取 checkpoint delta：{error}"))?;
        let mut next = NamedTempFile::new_in(&temp_directory)
            .map_err(|error| format!("无法创建 checkpoint 临时文件：{error}"))?;
        let restored = delta
            .apply(&base, &mut base_file, next.as_file_mut())
            .map_err(|error| format!("无法应用 checkpoint delta：{error}"))?;
        validate_digest(&restored, &chain[index + 1], "检查点恢复链节点")?;
        next.as_file()
            .sync_all()
            .map_err(|error| format!("无法同步 checkpoint：{error}"))?;
        current_path = next.path().to_owned();
        temporaries.push(next);
        progress("正在回溯并生成检查点", index as u64 + 1, total);
    }
    let checkpoint = temporaries
        .pop()
        .ok_or("checkpoint 没有生成目标 snapshot")?;
    history::ensure_directories(root, &target.artwork_id)?;
    let final_path = history::artwork_directory(root, &target.artwork_id)
        .join("snapshots")
        .join(format!("{}.lbc", target.id));
    let relative = storage::relative_path(root, &final_path)?;
    let file_size = checkpoint
        .as_file()
        .metadata()
        .map_err(|error| format!("无法读取 checkpoint 大小：{error}"))?
        .len();
    ensure_not_cancelled(&cancelled)?;
    checkpoint
        .persist_noclobber(&final_path)
        .map_err(|error| format!("无法发布 checkpoint：{}", error.error))?;
    if let Err(error) = history::set_snapshot(root, history_id, &relative, file_size, true) {
        let _ = fs::remove_file(&final_path);
        return Err(error);
    }
    progress("检查点已就绪", total, total);
    Ok(())
}

fn validate_snapshot(
    path: &Path,
    record: &history::HistoryRecord,
    label: &str,
) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("无法打开{label} snapshot：{error}"))?;
    let snapshot =
        ChunkFile::open(&mut file).map_err(|error| format!("无法读取{label} snapshot：{error}"))?;
    validate_digest(&snapshot, record, label)?;
    snapshot
        .copy_original(&mut file, &mut std::io::sink())
        .map_err(|error| format!("{label} snapshot 完整性校验失败：{error}"))
}

fn validate_digest(
    snapshot: &ChunkFile,
    record: &history::HistoryRecord,
    label: &str,
) -> Result<(), String> {
    if snapshot
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&record.sha256)
    {
        Ok(())
    } else {
        Err(format!("{label} snapshot 摘要与历史数据库不匹配"))
    }
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
