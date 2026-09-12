//! Deletion policy facade.
//!
//! Destructive history operations remain implemented beside the graph queries for now,
//! but this module is the only named capability surface for callers. Keeping the
//! policy surface explicit lets the implementation move without changing backup commands.

use std::path::Path;

use super::{BranchDeletion, HistoryDeletion};

pub(crate) fn delete_subtree(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<HistoryDeletion, String> {
    super::repository::delete_subtree(root, history_id, branch_id)
}

pub(crate) fn validate_subtree_deletion(
    root: &Path,
    history_id: &str,
    branch_id: &str,
) -> Result<(), String> {
    super::repository::validate_subtree_deletion(root, history_id, branch_id)
}

pub(crate) fn delete_branch(root: &Path, branch_id: &str) -> Result<BranchDeletion, String> {
    super::repository::delete_branch(root, branch_id)
}
