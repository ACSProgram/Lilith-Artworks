use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryStatus {
    pub(crate) configured: bool,
    pub(crate) ready: bool,
    pub(crate) root_path: String,
    pub(crate) database_path: String,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryTree {
    pub(crate) nodes: Vec<LibraryNode>,
    pub(crate) group_count: usize,
    pub(crate) artwork_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryNode {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) position: i64,
    pub(crate) updated_ms: i64,
    pub(crate) children: Vec<LibraryNode>,
    pub(crate) artwork: Option<ArtworkSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtworkSummary {
    pub(crate) description: String,
    pub(crate) branch_count: u64,
    pub(crate) backup_disable_notice_count: u64,
    pub(crate) primary_branch: Option<PrimaryBranch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrimaryBranch {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibrarySearchResult {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) breadcrumb: String,
    pub(crate) ancestor_ids: Vec<String>,
    pub(crate) source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryTrashEntry {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) deleted_ms: i64,
    pub(crate) descendant_count: u64,
    pub(crate) artwork_count: u64,
    pub(crate) original_parent_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateArtworkRequest {
    pub(crate) parent_id: Option<String>,
    pub(crate) title: String,
    pub(crate) branch_title: String,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MoveLibraryNodesRequest {
    pub(crate) ids: Vec<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) index: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct CreatedArtwork {
    pub(crate) artwork_id: String,
    pub(crate) branch_id: String,
}
