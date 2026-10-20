use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtworkHistory {
    pub(crate) artwork_id: String,
    pub(crate) artwork_title: String,
    pub(crate) branches: Vec<ArtworkBranch>,
    pub(crate) nodes: Vec<HistoryNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtworkBranch {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) source_path: String,
    pub(crate) head_history_id: Option<String>,
    pub(crate) created_from_history_id: Option<String>,
    pub(crate) backup_enabled: bool,
    pub(crate) backup_interval_minutes: u32,
    pub(crate) backup_quick_enabled: bool,
    pub(crate) last_check_ms: Option<i64>,
    pub(crate) last_success_ms: Option<i64>,
    pub(crate) last_error: Option<String>,
    pub(crate) consecutive_backup_failures: u32,
    pub(crate) backup_retry_at_ms: Option<i64>,
    pub(crate) backup_disable_notice_pending: bool,
    pub(crate) final_artifact_locked: bool,
    pub(crate) published_count: u32,
    /// 历史链上任一节点的校验失败摘要（由 `history_nodes.verify_error` 派生）；
    /// 节点被改写或手动重查后清空。
    pub(crate) verify_error: Option<String>,
    /// 链上确认出问题的节点数（丢了文件或内容读不出来）；不含「无法检查」。
    pub(crate) verify_failure_count: u32,
    /// 链上连检查都做不到的节点数：它们自己的文件未必坏，只是恢复链被更深的
    /// 坏节点挡住，无法下结论。
    pub(crate) verify_unverifiable_count: u32,
    /// 链上节点里最严重的一类：有文件丢失就是 `Missing`，其次 `Damaged`，
    /// 只有「无法检查」时才是 `Unverifiable`。分支状态行据此与节点卡片同色。
    pub(crate) verify_issue: Option<NodeVerifyIssue>,
    /// 整条链全部通过校验时取最旧的 `verified_ms`；任一节点未验证为 NULL。
    pub(crate) verified_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryNode {
    pub(crate) id: String,
    pub(crate) created_on_branch_id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) title: String,
    pub(crate) note: String,
    pub(crate) commit_kind: String,
    pub(crate) is_checkpoint: bool,
    pub(crate) created_ms: i64,
    pub(crate) logical_size: u64,
    pub(crate) chunk_file_size: u64,
    pub(crate) sha256: String,
    pub(crate) chunk_count: u64,
    /// 最近一次通过校验的时间；校验失败时为空。
    pub(crate) verified_ms: Option<i64>,
    /// 校验失败的人读文案；通过或尚未校验时为空。
    pub(crate) verify_error: Option<String>,
    /// 失败归类，前端只按它上色与选文案，不再解析 `verify_error`。
    pub(crate) verify_issue: Option<NodeVerifyIssue>,
}

/// 节点校验状态的归类。
///
/// 前两类是「确认出问题」：恢复所需的仓库文件已经不在了（`Missing`），或文件还在
/// 但内容读不出来、与数据库记录不符（`Damaged`）。第三类 `Unverifiable` 是
/// **没有结论**：这个节点自己的文件可能完好，但它的物化起点在更深的节点上，
/// 而那段链已经断了，谁也无法判断它还能不能恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum NodeVerifyIssue {
    Missing,
    Damaged,
    Unverifiable,
}

impl NodeVerifyIssue {
    /// 分支状态行与节点卡片共用的严重度排序：丢文件 > 读不出来 > 无法检查。
    fn severity(self) -> u8 {
        match self {
            Self::Missing => 2,
            Self::Damaged => 1,
            Self::Unverifiable => 0,
        }
    }

    /// 取更严重的一类；用于把多个节点的结论汇总成一条链的结论。
    pub(crate) fn worst(self, other: Option<Self>) -> Self {
        match other {
            Some(other) if other.severity() > self.severity() => other,
            _ => self,
        }
    }
}

/// 校验内核发现「节点恢复所需的仓库文件已不存在」时，写在错误文案最前面的标记。
///
/// 归类不单独建列：写入方（`backup::restore`）与读取方
/// [`classify_verify_failure`] 共用这一个常量，`verify_error` 仍是完整的人读文案，
/// schema 不必为配色再迁移一次。
pub(crate) const VERIFY_MISSING_MARKER: &str = "文件缺失：";

/// 「无法检查」写在错误文案最前面的标记。
///
/// 这一态必须与「确认出问题」分开：节点自己的文件是好的，只是恢复链在更深的
/// 节点上断掉了。用同一个 `verify_error` 列承载时，靠前缀区分三态，因此这里的
/// 判定顺序不能变——「无法检查」的文案里会引用被挡住的原始原因，可能连 `os error 2`
/// 这类旧线索一起带上，先看这个标记才不会把它误判成「文件丢失」。
pub(crate) const VERIFY_UNVERIFIABLE_MARKER: &str = "无法检查：";

/// 从历史文本判断校验结论。
///
/// `os error 2` 是 0.3 alpha3 之前的旧记录留下的唯一线索：那时还没有标记，
/// 文案里只有系统错误原文（本地化文本各不相同，`os error 2` 与语言无关）。
/// 旧记录被重新校验后即换成本标记，因此这段兼容只需保留到用户重查为止。
pub(crate) fn classify_verify_failure(message: &str) -> NodeVerifyIssue {
    if message.contains(VERIFY_UNVERIFIABLE_MARKER) {
        NodeVerifyIssue::Unverifiable
    } else if message.contains(VERIFY_MISSING_MARKER) || message.contains("os error 2") {
        NodeVerifyIssue::Missing
    } else {
        NodeVerifyIssue::Damaged
    }
}

/// 单个节点的缩略图读取结果。
///
/// `supported` 说明该节点的工作文件格式有没有抽取器：前端据此决定「无可用预览」
/// 占位与刷新入口要不要出现，格式不支持时连占位都不显示。它与 `data_url` 相互
/// 独立——已有缩略图的节点即便分支事后改指到不支持的格式，也照常显示图片。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodePreview {
    pub(crate) data_url: Option<String>,
    pub(crate) supported: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForkBranchRequest {
    pub(crate) artwork_id: String,
    pub(crate) from_history_id: String,
    pub(crate) title: String,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateBranchBackupRequest {
    pub(crate) branch_id: String,
    pub(crate) title: String,
    pub(crate) expected_backup_enabled: bool,
    pub(crate) backup_enabled: bool,
    pub(crate) backup_interval_minutes: u32,
    pub(crate) backup_quick_enabled: bool,
    pub(crate) source_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenameHistoryNodeRequest {
    pub(crate) history_id: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BranchRecord {
    pub(crate) artwork_id: String,
    pub(crate) source_path: String,
    pub(crate) head_history_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryRecord {
    pub(crate) id: String,
    pub(crate) artwork_id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) sha256: String,
    pub(crate) snapshot_path: Option<String>,
    pub(crate) delta_path: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScheduledBranch {
    pub(crate) id: String,
    /// 所属作品 ID。后台完成事件据此告诉前端应该刷新哪个作品的状态行。
    pub(crate) artwork_id: String,
    /// 分支工作文件路径。调度器据此判断该分支能不能进入无人值守流程：网络位置与
    /// 设备命名空间一律跳过（见 `storage::unattended_source_path_rejection`）。
    pub(crate) source_path: String,
    pub(crate) last_check_ms: Option<i64>,
    pub(crate) interval_minutes: u32,
    pub(crate) quick_enabled: bool,
    pub(crate) retry_at_ms: Option<i64>,
}

/// 派生待查集合中的一条空闲完整性检查候选：节点尚未校验且没有失败摘要。
///
/// 队列在 SQL 中按 `created_ms` 最旧优先排序——最久未读的节点是 bit rot 风险
/// 最高的节点。
#[derive(Debug, Clone)]
pub(crate) struct VerifyNodeTarget {
    pub(crate) history_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupDisableNoticeTarget {
    pub(crate) artwork_id: String,
    pub(crate) branch_id: String,
}

pub(crate) struct HistoryCommit<'a> {
    pub(crate) id: &'a str,
    pub(crate) branch_id: &'a str,
    pub(crate) parent_id: Option<&'a str>,
    pub(crate) title: &'a str,
    pub(crate) note: &'a str,
    pub(crate) commit_kind: &'a str,
    pub(crate) created_ms: i64,
    pub(crate) logical_size: u64,
    pub(crate) chunk_file_size: u64,
    pub(crate) sha256: &'a str,
    pub(crate) chunk_count: u64,
    pub(crate) snapshot_path: &'a str,
    pub(crate) delta_path: Option<&'a str>,
    pub(crate) delta_size: Option<u64>,
    /// Optional derived preview. Stored with the node row and never part of
    /// the ChunkFile content or its digest.
    pub(crate) thumbnail: Option<&'a [u8]>,
    pub(crate) thumbnail_version: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryDeletion {
    pub(crate) artwork_id: String,
    /// 事务内为已无引用的历史文件登记的 `pending_file_cleanup` id，由调用方在提交
    /// 成功后重放。仍被引用的候选路径不入队。
    pub(crate) cleanup_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BranchDeletion {
    pub(crate) artwork_id: String,
    /// 事务内为已无引用的历史文件登记的 `pending_file_cleanup` id，由调用方在提交
    /// 成功后重放。仍被引用的候选路径不入队。
    pub(crate) cleanup_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionTarget {
    pub(crate) artwork_id: String,
    pub(crate) node_id: String,
    pub(crate) parent_id: String,
    pub(crate) child_id: String,
}

/// 全库扫描用的链元数据快照：一次读出，之后不再按节点查库。
///
/// 字段口径与 `materialization_chain` 一致：`delta_path` 是「本节点的内容还原出
/// 父节点」的反向增量，取自本节点的入边（`history_nodes.delta_path` 只是兼容列）。
#[derive(Debug, Clone)]
pub(crate) struct VerifyChainNode {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) sha256: String,
    pub(crate) snapshot_path: Option<String>,
    /// 空串按「没有增量」处理（检查点父节点名下的合法记账）。
    pub(crate) delta_path: Option<String>,
    pub(crate) created_ms: i64,
}

impl VerifyChainNode {
    pub(crate) fn has_snapshot(&self) -> bool {
        self.snapshot_path.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_verify_failure, NodeVerifyIssue, VERIFY_MISSING_MARKER, VERIFY_UNVERIFIABLE_MARKER,
    };

    #[test]
    fn missing_marker_wins_over_the_rest_of_the_message() {
        let message = format!(
            "历史节点 ab12 完整性校验失败：{VERIFY_MISSING_MARKER}恢复链 delta 无法打开\
             （系统找不到指定的文件。 (os error 2)）"
        );
        assert_eq!(classify_verify_failure(&message), NodeVerifyIssue::Missing);
    }

    /// 旧记录只有系统错误原文，没有标记：仍要归到「文件丢失」而不是「损坏」，
    /// 否则用户重查之前会看到错误的红色/黄色。
    #[test]
    fn legacy_os_error_two_records_count_as_missing() {
        let message = "历史节点 ab12 完整性校验失败：无法打开恢复链 delta：\
                       系统找不到指定的文件。 (os error 2)";
        assert_eq!(classify_verify_failure(message), NodeVerifyIssue::Missing);
    }

    #[test]
    fn unreadable_or_mismatched_content_counts_as_damaged() {
        for message in [
            "历史节点 ab12 完整性校验失败：无法读取恢复链 delta：分块文件不兼容：不支持格式版本 16972",
            "历史节点 ab12 完整性校验失败：恢复链起点 snapshot 摘要与历史数据库不匹配",
        ] {
            assert_eq!(
                classify_verify_failure(message),
                NodeVerifyIssue::Damaged
            );
        }
    }

    /// 「无法检查」的文案会引用被挡住的原始原因，因此可能连 `os error 2` 一起带上。
    /// 判定必须先认这个标记，否则被挡住的节点会被说成自己也坏了。
    #[test]
    fn blocked_records_win_over_the_quoted_reason() {
        let message = format!(
            "{VERIFY_UNVERIFIABLE_MARKER}历史节点 ab12 的备份文件（恢复链 delta）已丢失（\
             系统找不到指定的文件。 (os error 2)），因此无法确认这个版本是否完好"
        );
        assert_eq!(
            classify_verify_failure(&message),
            NodeVerifyIssue::Unverifiable
        );
    }

    #[test]
    fn severity_prefers_confirmed_damage_over_an_open_question() {
        assert_eq!(
            NodeVerifyIssue::Missing.worst(Some(NodeVerifyIssue::Unverifiable)),
            NodeVerifyIssue::Missing
        );
        assert_eq!(
            NodeVerifyIssue::Damaged.worst(Some(NodeVerifyIssue::Unverifiable)),
            NodeVerifyIssue::Damaged
        );
        assert_eq!(
            NodeVerifyIssue::Unverifiable.worst(Some(NodeVerifyIssue::Damaged)),
            NodeVerifyIssue::Damaged
        );
        assert_eq!(
            NodeVerifyIssue::Unverifiable.worst(None),
            NodeVerifyIssue::Unverifiable
        );
    }
}
