use std::{
    collections::HashMap,
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

/// 链解析失败的归因。
///
/// `culprit` 是真正读不出来的那个节点。链上每一步的反向增量表示的都是**这一步要
/// 还原出来的节点**（它把子节点的内容还原成父节点的内容），所以某一步失败时坏掉
/// 的是目标节点的祖先，而不是发起解析的那个节点。这个区分是「无法检查」成立的
/// 前提：目标节点自己的文件可能完好，只是它依赖的链断了。
#[derive(Debug, Clone)]
struct ChainFailure {
    culprit: Option<String>,
    issue: history::NodeVerifyIssue,
    /// 跟在节点 id 后面的一句说明，例如「的备份文件（恢复链 delta）已丢失（…）」。
    detail: String,
}

impl ChainFailure {
    fn confirmed(
        node_id: &str,
        issue: history::NodeVerifyIssue,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            culprit: Some(node_id.to_owned()),
            issue,
            detail: detail.into(),
        }
    }

    /// 链本身不成立（数据结构问题），归不到具体节点。
    fn unknown(detail: impl Into<String>) -> Self {
        Self {
            culprit: None,
            issue: history::NodeVerifyIssue::Damaged,
            detail: detail.into(),
        }
    }

    fn describe(&self) -> String {
        describe_failure(self.culprit.as_deref(), &self.detail)
    }
}

/// 把「哪个节点、坏在哪」拼成人读的一句；`culprit` 为空表示链本身不成立。
fn describe_failure(culprit: Option<&str>, detail: &str) -> String {
    match culprit {
        Some(id) => format!("历史节点 {id} {detail}"),
        None => detail.to_owned(),
    }
}

/// 恢复、精简、检查点只关心「为什么读不出来」，因此直接当文案用。
impl From<ChainFailure> for String {
    fn from(failure: ChainFailure) -> String {
        failure.describe()
    }
}

/// 单节点校验失败：`culprit` 指向真正坏掉的节点，`detail` 是它可以对外的说明。
#[derive(Debug, Clone)]
pub(crate) struct VerifyFailure {
    culprit: Option<String>,
    issue: history::NodeVerifyIssue,
    detail: String,
}

impl From<ChainFailure> for VerifyFailure {
    fn from(failure: ChainFailure) -> Self {
        Self {
            culprit: failure.culprit,
            issue: failure.issue,
            detail: failure.detail,
        }
    }
}

impl VerifyFailure {
    /// 这次失败算在 `target` 自己头上，还是「被更深的节点挡住」。
    ///
    /// 链本身不成立（`culprit` 为空）时也算在目标头上：没有别的节点可以指认。
    pub(crate) fn blames(&self, target: &str) -> bool {
        match self.culprit.as_deref() {
            Some(id) => id == target,
            None => true,
        }
    }

    /// 写入 `target` 的 `verify_error`。三种情况分开写，读取侧靠前缀分类：
    ///
    /// - 目标自己坏了 → 直接说坏在哪；
    /// - 目标被更深的节点挡住 → 「无法检查」，并点名那个节点。这里只带「无法检查」
    ///   一个标记：被挡住的说明里已经写明原因，再补一个归类标记既重复又难读；
    /// - 链本身不成立 → 只能说这次检查没做成。
    pub(crate) fn message_for(&self, target: &str) -> String {
        let marker = match self.issue {
            history::NodeVerifyIssue::Missing => history::VERIFY_MISSING_MARKER,
            _ => "",
        };
        match self.culprit.as_deref() {
            Some(id) if id == target => format!("{marker}历史节点 {id} {}", self.detail),
            Some(_) => format!(
                "{}{}，因此无法确认这个版本是否完好",
                history::VERIFY_UNVERIFIABLE_MARKER,
                describe_failure(self.culprit.as_deref(), &self.detail),
            ),
            None => format!("{marker}检查历史节点 {target} 失败：{}", self.detail),
        }
    }
}

/// 物化单个历史节点并校验其完整性：沿链解析、与数据库摘要比对、逐块验哈希。
///
/// 这是空闲增量批次使用的单节点内核，只读、不写任何状态；调用方负责用
/// [`VerifyFailure::message_for`] 把结果写入节点级校验列。手动重查与全库扫描
/// 按链一次走完（见 [`walk_verification_inventory`]），只有没有可达 snapshot
/// 的孤立节点才退回这里。
pub(crate) fn verify_history_node(
    root: &Path,
    history_id: &str,
    cancel: &CancelToken,
) -> Result<(), VerifyFailure> {
    ensure_not_cancelled(cancel).map_err(ChainFailure::unknown)?;
    // Resolving the chain already proves that every delta links to the
    // digest recorded in the database. Streaming the result into the sink
    // then verifies each chunk payload and the assembled file without
    // writing any temporary snapshot.
    let ResolvedChain {
        layout, mut store, ..
    } = resolve_chain(root, history_id, "正在校验历史", cancel, &|_, _, _| {})?;
    ensure_not_cancelled(cancel).map_err(ChainFailure::unknown)?;
    let task = ChunkTask::cancel(cancel);
    let mut reader = store.reader();
    layout
        .copy_original(&mut reader, &mut std::io::sink(), &task)
        .map_err(|error| {
            ChainFailure::confirmed(
                history_id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件读取失败：{error}"),
            )
        })?;
    Ok(())
}

/// 一次全库扫描内单个节点的结论。
#[derive(Debug, Clone)]
enum ScrubOutcome {
    Verified,
    /// 确认出问题：失败里带着真正坏掉的节点。
    Failed(VerifyFailure),
    /// 没有结论：文件未必坏，只是恢复链在更深的节点上断了。
    Blocked(VerifyFailure),
}

impl ScrubOutcome {
    fn settled(&self) -> bool {
        !matches!(self, Self::Blocked(_))
    }
}

/// 全库扫描：把整个仓库的恢复链一次走完，逐个节点写入校验结论。
///
/// 与 [`verify_history_node`] 结果等价，但**按链走**而不是按节点走。一个节点的
/// 内容是由「更新的节点 + 一路反向增量」还原出来的，因此从每个持有 snapshot 的
/// 节点出发沿父链往上走一次，就覆盖了整条链；共享的前缀只解析一遍，多分支在共同
/// 祖先处也只走一次，不必让每个节点都从 snapshot 重新回溯。
///
/// 某一步失败时，坏掉的是这一步要还原的节点：只有它被标成丢失/损坏，它更早的祖先
/// 先记为「无法检查」。若某个祖先还能经另一条分支到达（例如分叉后的另一支），
/// 那条链会把它校验通过并覆盖掉「无法检查」——这正是多分支的价值。
///
/// 单个节点失败不会中止扫描，全部分块流式读取、不写临时快照；至少一个节点失败时
/// 汇总为错误返回。
pub(crate) fn scrub_history(
    root: &Path,
    cancel: &CancelToken,
    progress: impl FnMut(u64, u64),
) -> Result<u64, String> {
    let inventory = history::verification_inventory(root)?;
    let total = inventory.len() as u64;
    let mut failures = 0_u64;
    let mut first_error: Option<String> = None;

    walk_verification_inventory(root, inventory, cancel, progress, |history_id, outcome| {
        record_scrub_outcome(root, history_id, outcome, &mut failures, &mut first_error)
    })?;

    match first_error {
        None => Ok(total),
        Some(error) if failures == 1 => Err(error),
        Some(error) => Err(format!(
            "{error}（另有 {} 个历史节点校验失败）",
            failures - 1
        )),
    }
}

/// 手动重查一条分支：与 [`scrub_history`] 共用按链遍历，只是范围收在分支祖先链内。
///
/// 分支内每个持有 snapshot 的节点（head、检查点、分支起点）都是独立起点，从它
/// 沿父链走到下一个 snapshot 或根节点；共享前缀只解析一次。结论写回节点级校验
/// 状态，失败归因与全库扫描一致。返回本次覆盖的节点数，供调用方收尾进度。
pub(crate) fn verify_branch_history(
    root: &Path,
    branch_id: &str,
    cancel: &CancelToken,
    progress: impl FnMut(u64, u64),
) -> Result<u64, String> {
    let inventory = history::branch_verification_inventory(root, branch_id)?;
    let total = inventory.len() as u64;
    let mut failed = 0_u64;
    let mut blocked = 0_u64;

    walk_verification_inventory(root, inventory, cancel, progress, |history_id, outcome| {
        let Some(message) = apply_outcome(root, history_id, outcome)? else {
            return Ok(());
        };
        match outcome {
            ScrubOutcome::Failed(_) => {
                failed += 1;
                log::warn!("branch reverify failed for history node {history_id}: {message}");
            }
            ScrubOutcome::Blocked(_) => {
                blocked += 1;
                log::warn!("branch reverify could not verify history node {history_id}: {message}");
            }
            ScrubOutcome::Verified => {}
        }
        Ok(())
    })?;

    log::info!(
        "branch history reverified: branch_id={branch_id}, nodes={total}, failed={failed}, blocked={blocked}"
    );
    Ok(total)
}

/// 按链走完一份节点清单，把结论交给调用方写入。
///
/// 从每个持有 snapshot 的节点出发沿父链往上走一趟，走过的节点顺带拿到结论；
/// 新的起点先走，走到已有确定结论的节点、或自带 snapshot 的祖先就停。没有
/// 可达 snapshot 的孤立节点退回单节点内核，归因仍然准确。
fn walk_verification_inventory(
    root: &Path,
    inventory: Vec<history::VerifyChainNode>,
    cancel: &CancelToken,
    mut progress: impl FnMut(u64, u64),
    mut record: impl FnMut(&str, &ScrubOutcome) -> Result<(), String>,
) -> Result<(), String> {
    let total = inventory.len() as u64;
    if total == 0 {
        return Ok(());
    }
    let nodes: HashMap<&str, &history::VerifyChainNode> = inventory
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut outcomes: HashMap<String, ScrubOutcome> = HashMap::with_capacity(inventory.len());

    // 新的起点先走：一条新链一次覆盖它的整条祖先链，老检查点随后只剩自己要走。
    let mut starts: Vec<&history::VerifyChainNode> = inventory
        .iter()
        .filter(|node| node.has_snapshot())
        .collect();
    starts.sort_by(|left, right| {
        right
            .created_ms
            .cmp(&left.created_ms)
            .then_with(|| right.id.cmp(&left.id))
    });

    for start in starts {
        ensure_not_cancelled(cancel)?;
        let walked = walk_chain(root, start, &nodes, &outcomes, cancel)?;
        for (history_id, outcome) in walked {
            let known = outcomes.contains_key(&history_id);
            // 「无法检查」让位于任何已确认的结论：另一条分支可能已经把它验证通过。
            if known && !outcome.settled() {
                continue;
            }
            ensure_not_cancelled(cancel)?;
            record(&history_id, &outcome)?;
            outcomes.insert(history_id, outcome);
            // 进度按「已有确定结论的节点数」报：被更深的链推翻过一次的灰色节点不会
            // 让进度倒退，也不会把同一节点算两遍。
            progress(outcomes.len() as u64, total);
        }
    }

    // 剩下的节点没有任何可达的 snapshot（分支删除、精简残留之类的图异常）：退回
    // 单节点内核，至少让归因仍然准确。
    for node in &inventory {
        if outcomes.contains_key(&node.id) {
            continue;
        }
        ensure_not_cancelled(cancel)?;
        let outcome = match verify_history_node(root, &node.id, cancel) {
            Ok(()) => ScrubOutcome::Verified,
            Err(failure) => ScrubOutcome::Failed(failure),
        };
        // 取消发生在单节点内核内部时，它表现为一次失败；这里不能把取消写成损坏。
        if cancel.is_cancelled() {
            return Err("操作已取消".into());
        }
        record(&node.id, &outcome)?;
        outcomes.insert(node.id.clone(), outcome);
        progress(outcomes.len() as u64, total);
    }

    Ok(())
}

fn record_scrub_outcome(
    root: &Path,
    history_id: &str,
    outcome: &ScrubOutcome,
    failures: &mut u64,
    first_error: &mut Option<String>,
) -> Result<(), String> {
    let Some(message) = apply_outcome(root, history_id, outcome)? else {
        return Ok(());
    };
    match outcome {
        ScrubOutcome::Failed(_) => {
            log::warn!("repository scrub failed for history node {history_id}: {message}");
            *failures += 1;
            first_error.get_or_insert(message);
        }
        ScrubOutcome::Blocked(_) => {
            log::warn!("repository scrub could not verify history node {history_id}: {message}");
        }
        ScrubOutcome::Verified => {}
    }
    Ok(())
}

/// 写入一个节点的校验结论；确认有问题时返回可对外的文案，通过时返回 `None`。
fn apply_outcome(
    root: &Path,
    history_id: &str,
    outcome: &ScrubOutcome,
) -> Result<Option<String>, String> {
    match outcome {
        ScrubOutcome::Verified => {
            let verified_ms = storage::now_ms()?;
            history::mark_node_verified(root, history_id, verified_ms)?;
            Ok(None)
        }
        ScrubOutcome::Failed(failure) | ScrubOutcome::Blocked(failure) => {
            let message = failure.message_for(history_id);
            history::mark_node_verify_error(root, history_id, &message)?;
            Ok(Some(message))
        }
    }
}

/// 沿链还原出来的一份布局。
///
/// `payload` 是「块表已经对上、但载荷读不出来」时给出的失败：链本身还能继续往上
/// 回溯（父节点未必引用那个坏块），只是这个节点的内容确实读不回来。
struct WalkedLayout {
    layout: ChunkFile,
    payload: Option<ChainFailure>,
}

/// 从 `start` 的 snapshot 出发，沿父链一路还原并逐节点校验。
///
/// 返回本次走到的全部结论（起点在前）。走到已有确定结论的节点、或走到自带 snapshot
/// 的祖先就停下：后者有独立的物化起点，由它自己那趟遍历负责。
fn walk_chain(
    root: &Path,
    start: &history::VerifyChainNode,
    nodes: &HashMap<&str, &history::VerifyChainNode>,
    settled: &HashMap<String, ScrubOutcome>,
    cancel: &CancelToken,
) -> Result<Vec<(String, ScrubOutcome)>, String> {
    let mut results = Vec::new();
    let mut store = ChunkStore::new();
    let temp_directory = root.join("temp");
    let task = ChunkTask::cancel(cancel);
    let snapshot_relative = start
        .snapshot_path
        .as_deref()
        .ok_or_else(|| format!("历史节点 {} 的校验起点缺少 snapshot", start.id))?;
    let mut walked = match open_snapshot_content(root, start, snapshot_relative, &mut store, &task)
    {
        Ok(walked) => walked,
        Err(failure) => {
            let failure = VerifyFailure::from(failure);
            let blocked = blocked_ancestors(start, nodes, settled, &failure);
            results.push((start.id.clone(), ScrubOutcome::Failed(failure)));
            results.extend(blocked);
            return Ok(results);
        }
    };
    results.push((start.id.clone(), outcome_of(&walked)));
    let mut current = start;
    loop {
        ensure_not_cancelled(cancel)?;
        let Some(parent_id) = current.parent_id.as_deref() else {
            break;
        };
        if settled.get(parent_id).is_some_and(ScrubOutcome::settled) {
            break;
        }
        let Some(parent) = nodes.get(parent_id).copied() else {
            break;
        };
        // 祖先自己持有 snapshot 时，这条反向增量按设计不会被任何读取路径消费。
        if parent.has_snapshot() {
            break;
        }
        walked = match resolve_parent_content(
            root,
            current,
            parent,
            &mut store,
            &temp_directory,
            walked.layout,
            &task,
        ) {
            Ok(walked) => walked,
            Err(failure) => {
                let failure = VerifyFailure::from(failure);
                let blocked = blocked_ancestors(parent, nodes, settled, &failure);
                results.push((parent.id.clone(), ScrubOutcome::Failed(failure)));
                results.extend(blocked);
                return Ok(results);
            }
        };
        results.push((parent.id.clone(), outcome_of(&walked)));
        current = parent;
    }
    Ok(results)
}

/// 本次还原出来的节点自身结论：载荷读不出来就是它自己坏了，与链无关。
fn outcome_of(walked: &WalkedLayout) -> ScrubOutcome {
    match &walked.payload {
        Some(failure) => ScrubOutcome::Failed((*failure).clone().into()),
        None => ScrubOutcome::Verified,
    }
}

/// 把 `from` 更早的祖先记为「无法检查」：恢复链已经在这里断了，再往上不可能
/// 还有路径。自带 snapshot 的祖先不算——它有独立的物化起点，会自己校验。
fn blocked_ancestors(
    from: &history::VerifyChainNode,
    nodes: &HashMap<&str, &history::VerifyChainNode>,
    settled: &HashMap<String, ScrubOutcome>,
    blocker: &VerifyFailure,
) -> Vec<(String, ScrubOutcome)> {
    let mut blocked = Vec::new();
    let mut cursor = from.parent_id.as_deref();
    while let Some(id) = cursor {
        if settled.get(id).is_some_and(ScrubOutcome::settled) {
            break;
        }
        let Some(node) = nodes.get(id).copied() else {
            break;
        };
        if node.has_snapshot() {
            break;
        }
        blocked.push((id.to_owned(), ScrubOutcome::Blocked(blocker.clone())));
        cursor = node.parent_id.as_deref();
    }
    blocked
}

/// 校验一个节点自己持有的 snapshot，并把它当作后续回溯的基础布局返回。
///
/// 块表与摘要决定「这份布局还能不能当基础用」：对不上就没有任何后续可走。载荷的
/// 哈希只有整份读完才知道，而它只影响这个节点本身——单块损坏不必阻止回溯到祖先，
/// 祖先可能根本不引用那个坏块。
fn open_snapshot_content(
    root: &Path,
    node: &history::VerifyChainNode,
    snapshot_relative: &str,
    store: &mut ChunkStore<'static>,
    task: &ChunkTask<'_>,
) -> Result<WalkedLayout, ChainFailure> {
    let path = storage::resolve_path(root, snapshot_relative).map_err(ChainFailure::unknown)?;
    let file = open_chain_file(&path, &node.id, "备份文件（起点 snapshot）")?;
    store.append_file(file).map_err(|error| {
        ChainFailure::confirmed(
            &node.id,
            history::NodeVerifyIssue::Damaged,
            format!("的备份文件（起点 snapshot）无法读取：{error}"),
        )
    })?;
    let layout = {
        let mut reader = store.reader();
        ChunkFile::open(&mut reader).map_err(|error| {
            ChainFailure::confirmed(
                &node.id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件（起点 snapshot）无法读取：{error}"),
            )
        })?
    };
    if !layout
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&node.sha256)
    {
        return Err(ChainFailure::confirmed(
            &node.id,
            history::NodeVerifyIssue::Damaged,
            "的备份文件（起点 snapshot）与历史数据库记录不符",
        ));
    }
    let payload = read_payload(store, &layout, task).err().map(|error| {
        ChainFailure::confirmed(
            &node.id,
            history::NodeVerifyIssue::Damaged,
            format!("的备份文件读取失败：{error}"),
        )
    });
    Ok(WalkedLayout { layout, payload })
}

/// 用 `current` 的反向增量还原出父节点 `parent` 的内容并校验。
///
/// 这条增量是「`parent` 的内容」的唯一表示，因此打开、解压、回溯与摘要比对任何
/// 一步失败，坏掉都是 `parent`，与 `current` 无关。
fn resolve_parent_content(
    root: &Path,
    current: &history::VerifyChainNode,
    parent: &history::VerifyChainNode,
    store: &mut ChunkStore<'static>,
    temp_directory: &Path,
    layout: ChunkFile,
    task: &ChunkTask<'_>,
) -> Result<WalkedLayout, ChainFailure> {
    let delta_relative = current
        .delta_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            ChainFailure::confirmed(
                &parent.id,
                history::NodeVerifyIssue::Damaged,
                "缺少指向本节点的反向增量",
            )
        })?;
    let path = storage::resolve_path(root, delta_relative).map_err(ChainFailure::unknown)?;
    let mut delta_file = open_chain_file(&path, &parent.id, "备份文件（恢复链 delta）")?;
    let delta =
        ChunkFileDelta::open_in(&mut delta_file, temp_directory, task).map_err(|error| {
            ChainFailure::confirmed(
                &parent.id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件（恢复链 delta）无法读取：{error}"),
            )
        })?;
    let resolved = delta.resolve(&layout, store, task).map_err(|error| {
        ChainFailure::confirmed(
            &parent.id,
            history::NodeVerifyIssue::Damaged,
            format!("的备份文件（恢复链 delta）无法回溯：{error}"),
        )
    })?;
    if !resolved
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&parent.sha256)
    {
        return Err(ChainFailure::confirmed(
            &parent.id,
            history::NodeVerifyIssue::Damaged,
            "的内容与历史数据库记录不符",
        ));
    }
    let payload = read_payload(store, &resolved, task).err().map(|error| {
        ChainFailure::confirmed(
            &parent.id,
            history::NodeVerifyIssue::Damaged,
            format!("的备份文件读取失败：{error}"),
        )
    });
    Ok(WalkedLayout {
        layout: resolved,
        payload,
    })
}

/// 把布局的原始内容整份流式读出（不落盘），逐块验哈希并核对整文件摘要。
fn read_payload(
    store: &mut ChunkStore<'static>,
    layout: &ChunkFile,
    task: &ChunkTask<'_>,
) -> Result<(), super::chunk_file::ChunkFileError> {
    let mut reader = store.reader();
    layout.copy_original(&mut reader, &mut std::io::sink(), task)
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
///
/// 失败时带回真正坏掉的节点：链上每一步的反向增量都是「这一步要还原出来的节点」
/// 的唯一表示，因此哪一步失败，坏的正是那一步的父节点。
fn resolve_chain(
    root: &Path,
    history_id: &str,
    step_label: &str,
    cancel: &CancelToken,
    progress: &impl Fn(&str, u64, u64),
) -> Result<ResolvedChain, ChainFailure> {
    ensure_not_cancelled(cancel).map_err(ChainFailure::unknown)?;
    let chain = history::materialization_chain(root, history_id).map_err(ChainFailure::unknown)?;
    let first = chain
        .first()
        .ok_or_else(|| ChainFailure::unknown("恢复链为空"))?;
    let snapshot_path = storage::resolve_path(
        root,
        first.snapshot_path.as_deref().ok_or_else(|| {
            ChainFailure::confirmed(
                &first.id,
                history::NodeVerifyIssue::Damaged,
                "的恢复链起点缺少 snapshot 记录",
            )
        })?,
    )
    .map_err(ChainFailure::unknown)?;
    let mut store = ChunkStore::new();
    store
        .append_file(open_chain_file(
            &snapshot_path,
            &first.id,
            "备份文件（起点 snapshot）",
        )?)
        .map_err(|error| {
            ChainFailure::confirmed(
                &first.id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件（起点 snapshot）无法读取：{error}"),
            )
        })?;
    let mut layout = {
        let mut reader = store.reader();
        ChunkFile::open(&mut reader).map_err(|error| {
            ChainFailure::confirmed(
                &first.id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件（起点 snapshot）无法读取：{error}"),
            )
        })?
    };
    if !layout
        .file_digest()
        .to_hex()
        .eq_ignore_ascii_case(&first.sha256)
    {
        return Err(ChainFailure::confirmed(
            &first.id,
            history::NodeVerifyIssue::Damaged,
            "的备份文件（起点 snapshot）与历史数据库记录不符",
        ));
    }

    let total = chain.len() as u64;
    let temp_directory = root.join("temp");
    let task = ChunkTask::cancel(cancel);
    let mut steps = 1_u64;
    for (index, node) in chain.iter().take(chain.len().saturating_sub(1)).enumerate() {
        ensure_not_cancelled(cancel).map_err(ChainFailure::unknown)?;
        // 这一步要还原出来的节点：这条反向增量就是它的唯一表示。
        let expected = &chain[index + 1];
        let delta_path = node
            .delta_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                ChainFailure::confirmed(
                    &expected.id,
                    history::NodeVerifyIssue::Damaged,
                    "缺少指向本节点的反向增量",
                )
            })?;
        let delta_path = storage::resolve_path(root, delta_path).map_err(ChainFailure::unknown)?;
        let mut delta_file =
            open_chain_file(&delta_path, &expected.id, "备份文件（恢复链 delta）")?;
        let delta =
            ChunkFileDelta::open_in(&mut delta_file, &temp_directory, &task).map_err(|error| {
                ChainFailure::confirmed(
                    &expected.id,
                    history::NodeVerifyIssue::Damaged,
                    format!("的备份文件（恢复链 delta）无法读取：{error}"),
                )
            })?;
        layout = delta.resolve(&layout, &mut store, &task).map_err(|error| {
            ChainFailure::confirmed(
                &expected.id,
                history::NodeVerifyIssue::Damaged,
                format!("的备份文件（恢复链 delta）无法回溯：{error}"),
            )
        })?;
        if !layout
            .file_digest()
            .to_hex()
            .eq_ignore_ascii_case(&expected.sha256)
        {
            return Err(ChainFailure::confirmed(
                &expected.id,
                history::NodeVerifyIssue::Damaged,
                "的内容与历史数据库记录不符",
            ));
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

/// 打开历史链上的受控文件，并把它归到「用这份文件还原谁」名下。
///
/// 归类只判断 `ErrorKind::NotFound`，不解析系统错误文本：读取侧（
/// `history::classify_verify_failure`）因此不必认识 Windows 的本地化文案。其余
/// 打不开的原因（权限、占用、路径无效）都留在「损坏」一类，因为文件还在。
fn open_chain_file(path: &Path, culprit_id: &str, label: &str) -> Result<File, ChainFailure> {
    match File::open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ChainFailure::confirmed(
            culprit_id,
            history::NodeVerifyIssue::Missing,
            format!("的{label}已丢失（{error}）"),
        )),
        Err(error) => Err(ChainFailure::confirmed(
            culprit_id,
            history::NodeVerifyIssue::Damaged,
            format!("的{label}无法打开（{error}）"),
        )),
    }
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
