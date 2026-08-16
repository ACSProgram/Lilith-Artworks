pub(crate) mod chunk_file;
mod commands;
mod restore;
mod runtime;
mod scheduler;
mod worker;

use serde::{Deserialize, Serialize};

pub(crate) use commands::*;
pub(crate) use restore::{ensure_checkpoint, scrub_history};
pub(crate) use runtime::BackupState;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupRuntimeStatus {
    pub(crate) busy: bool,
    pub(crate) active_branch_id: Option<String>,
    pub(crate) operation: Option<String>,
    pub(crate) progress_label: Option<String>,
    pub(crate) progress_current: u64,
    pub(crate) progress_total: u64,
    pub(crate) automatic_scheduling: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupCommitResult {
    pub(crate) created: bool,
    pub(crate) unchanged: bool,
    pub(crate) history_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupNowRequest {
    pub(crate) branch_id: String,
    pub(crate) note: String,
}
