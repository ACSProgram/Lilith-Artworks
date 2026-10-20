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
