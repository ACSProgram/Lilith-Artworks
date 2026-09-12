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
    pub(crate) last_check_ms: Option<i64>,
    pub(crate) last_success_ms: Option<i64>,
    pub(crate) last_error: Option<String>,
    pub(crate) consecutive_backup_failures: u32,
    pub(crate) backup_retry_at_ms: Option<i64>,
    pub(crate) backup_disable_notice_pending: bool,
    pub(crate) final_artifact_locked: bool,
    pub(crate) published_count: u32,
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
    pub(crate) last_check_ms: Option<i64>,
    pub(crate) interval_minutes: u32,
    pub(crate) retry_at_ms: Option<i64>,
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
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryDeletion {
    pub(crate) artwork_id: String,
    pub(crate) storage_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BranchDeletion {
    pub(crate) artwork_id: String,
    pub(crate) storage_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionTarget {
    pub(crate) artwork_id: String,
    pub(crate) node_id: String,
    pub(crate) parent_id: String,
    pub(crate) child_id: String,
}
