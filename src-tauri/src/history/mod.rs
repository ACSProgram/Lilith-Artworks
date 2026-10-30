mod commands;
mod contracts;
mod deletion_repository;
mod model;
mod repository;
mod schedule_repository;
mod verification_repository;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use commands::*;
pub(crate) use contracts::HistoryReferenceInventory;
pub(crate) use deletion_repository::{
    delete_branch, delete_subtree_prechecked, preflight_subtree_deletion, reset_artwork_history,
};
pub(crate) use model::{
    classify_verify_failure, ArtworkBranch, ArtworkHistory, BackupDisableNoticeTarget,
    BranchDeletion, BranchRecord, CheckMode, CompactionTarget, ForkBranchRequest, HistoryCommit,
    HistoryDeletion, HistoryNode, HistoryRecord, NodePreview, NodeVerifyIssue,
    RenameHistoryNodeRequest, ScheduledBranch, UpdateBranchBackupRequest, VerifyChainNode,
    VerifyNodeTarget, VERIFY_MISSING_MARKER, VERIFY_UNVERIFIABLE_MARKER,
};
pub(crate) use repository::{
    apply_compaction, artwork_directory, checkpoint_snapshot_holder, commit, compaction_target,
    create_branch, ensure_directories, list, load_branch, load_node, load_thumbnail,
    mark_checkpoint, materialization_chain, node_source_path, node_storage_path,
    plan_checkpoint_release, rename_node, set_snapshot, store_thumbnail, unmark_checkpoint,
    update_branch,
};
pub(crate) use schedule_repository::{
    acknowledge_backup_disable_notices, count_scheduled_files, latest_history_write_ms,
    list_scheduled, load_scheduled, load_source_metadata, mark_unchanged,
    next_backup_disable_notice_target, record_branch_failure, record_check_result,
    record_source_metadata, reset_quick_hit_count, BranchFailure,
};
pub(crate) use verification_repository::{
    branch_verification_inventory, clear_branch_verify_errors, count_pending_verify_nodes,
    list_pending_verify_nodes, verification_inventory, VerificationWriter,
};
// 单节点校验写入只由测试直接使用；生产路径走 `VerificationWriter`。
#[cfg(test)]
pub(crate) use verification_repository::{mark_node_verified, mark_node_verify_error};
