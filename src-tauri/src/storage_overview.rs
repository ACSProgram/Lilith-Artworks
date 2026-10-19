//! 仓储空间只读概览。
//!
//! 本模块只回答问题「仓库里有什么、占了多少、压缩效果如何」：
//! - [`overview`] 从 SQLite 的既有大小列聚合数据库记录口径；
//! - [`scan_disk`] 遍历仓库目录，按同一批路径归属得到磁盘实际口径。
//!
//! 两套口径**不静默合并**：非 snapshot 节点的 `chunk_file_size` 是重建读取量，
//! 不进入存储总量；认证输出按 `output_bytes` 与路径去重后只算一次；磁盘扫描只
//! 统计仓库目录本身，认证外部首次导出路径不计入。DDS 分类只看条目是否存在实体
//! 文件（`file_path` 非空），阶段 7 重建画板表后只需重新定位这里的 SQL。

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::cell::Cell;

use rusqlite::Connection;
use serde::Serialize;

use crate::{repository_lock, storage};

// 仅测试使用的查询计数：`overview_from_connection` 只允许固定条数的批量查询，
// 不允许按作品或节点逐条回查。生产构建里 `note_query` 是空函数，成本为零。
#[cfg(test)]
thread_local! {
    static QUERY_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn note_query() {
    QUERY_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn note_query() {}

#[cfg(test)]
fn reset_query_count() {
    QUERY_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn query_count() -> usize {
    QUERY_COUNT.with(|count| count.get())
}

/// 仓储空间的分类口径。序列化为 camelCase，与前端 `StorageCategoryKind` 一一对应；
/// 变体顺序同时决定分类列表与「缺失文件」列表的稳定排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StorageKind {
    Snapshot,
    Delta,
    FinalArtifact,
    Certification,
    BoardDds,
    Other,
}

/// 磁盘扫描被取消时的统一文案；目录遍历与逐文件统计共用。
const SCAN_CANCELLED: &str = "磁盘占用扫描已取消";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageRatio {
    pub(crate) logical_bytes: u64,
    pub(crate) stored_bytes: u64,
    /// 逻辑大小 / 存储大小；没有存储字节时为 `None`。
    pub(crate) ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCategory {
    pub(crate) kind: StorageKind,
    pub(crate) bytes: u64,
    pub(crate) file_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCompression {
    pub(crate) snapshot: StorageRatio,
    pub(crate) delta: StorageRatio,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageNode {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) created_ms: i64,
    pub(crate) is_checkpoint: bool,
    pub(crate) has_snapshot: bool,
    pub(crate) logical_size: u64,
    /// snapshot 行是快照文件大小；其余行是重建该节点需要读取的字节数。
    pub(crate) chunk_file_size: u64,
    pub(crate) chunk_count: u64,
    pub(crate) compression_ratio: Option<f64>,
    pub(crate) snapshot_path: Option<String>,
    pub(crate) delta_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageArtifact {
    pub(crate) id: String,
    pub(crate) source_path: String,
    pub(crate) byte_size: u64,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCertification {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) output_path: String,
    pub(crate) stored_path: String,
    pub(crate) output_bytes: u64,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageBranch {
    pub(crate) branch_id: String,
    pub(crate) title: String,
    pub(crate) snapshot_bytes: u64,
    pub(crate) delta_bytes: u64,
    pub(crate) final_artifact_bytes: u64,
    pub(crate) certification_bytes: u64,
    pub(crate) recorded_bytes: u64,
    pub(crate) final_artifact: Option<StorageArtifact>,
    pub(crate) certification_records: Vec<StorageCertification>,
    pub(crate) nodes: Vec<StorageNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageBoard {
    pub(crate) board_id: i64,
    pub(crate) name: String,
    pub(crate) trashed: bool,
    pub(crate) image_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageArtwork {
    pub(crate) artwork_id: String,
    pub(crate) title: String,
    pub(crate) group_path: Vec<String>,
    pub(crate) trashed: bool,
    pub(crate) snapshot_bytes: u64,
    pub(crate) delta_bytes: u64,
    pub(crate) final_artifact_bytes: u64,
    pub(crate) certification_bytes: u64,
    pub(crate) recorded_bytes: u64,
    pub(crate) node_count: u64,
    pub(crate) branch_count: u64,
    pub(crate) branches: Vec<StorageBranch>,
    pub(crate) boards: Vec<StorageBoard>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageUnassigned {
    pub(crate) kind: StorageKind,
    pub(crate) bytes: u64,
    pub(crate) count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryStorageOverview {
    pub(crate) calculated_at_ms: i64,
    pub(crate) database_recorded_bytes: u64,
    pub(crate) categories: Vec<StorageCategory>,
    pub(crate) compression: StorageCompression,
    pub(crate) artworks: Vec<StorageArtwork>,
    pub(crate) unassigned: Vec<StorageUnassigned>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageOtherBreakdown {
    pub(crate) sqlite_bytes: u64,
    pub(crate) temp_bytes: u64,
    pub(crate) unrecognized_bytes: u64,
    pub(crate) sqlite_files: u64,
    pub(crate) temp_files: u64,
    pub(crate) unrecognized_files: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageArtworkDiskBytes {
    pub(crate) artwork_id: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageBoardDiskBytes {
    pub(crate) board_id: i64,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageMissingFile {
    pub(crate) kind: StorageKind,
    pub(crate) count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryStorageDiskScan {
    pub(crate) scanned_at_ms: i64,
    pub(crate) file_count: u64,
    pub(crate) total_bytes: u64,
    pub(crate) categories: Vec<StorageCategory>,
    pub(crate) other_breakdown: StorageOtherBreakdown,
    pub(crate) artwork_bytes: Vec<StorageArtworkDiskBytes>,
    pub(crate) dds_by_board: Vec<StorageBoardDiskBytes>,
    pub(crate) missing_files: Vec<StorageMissingFile>,
}

pub(crate) fn overview(root: &Path) -> Result<RepositoryStorageOverview, String> {
    let connection = storage::open(root)?;
    overview_from_connection(&connection)
}

pub(crate) fn scan_disk(
    root: &Path,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(u64, u64),
) -> Result<RepositoryStorageDiskScan, String> {
    let connection = storage::open(root)?;
    let plan = disk_plan(&connection)?;
    // 目录遍历是长任务：先关掉数据库连接，扫描期间不再占用 SQLite 句柄。
    drop(connection);
    scan_files(root, &plan, &cancelled, &progress)
}

struct LibraryNode {
    parent_id: Option<String>,
    kind: String,
    title: String,
    trashed: bool,
}

struct BranchInfo {
    id: String,
    artwork_id: String,
    title: String,
}

struct HistoryInfo {
    id: String,
    artwork_id: String,
    branch_id: String,
    title: String,
    is_checkpoint: bool,
    created_ms: i64,
    logical_size: u64,
    chunk_file_size: u64,
    chunk_count: u64,
    snapshot_path: Option<String>,
    delta_path: Option<String>,
}

struct EdgeInfo {
    child_history_id: String,
    parent_history_id: String,
    delta_size: u64,
}

struct ArtifactInfo {
    id: String,
    branch_id: String,
    source_path: String,
    byte_size: u64,
    created_ms: i64,
}

struct CertificationInfo {
    id: String,
    branch_id: String,
    output_path: String,
    stored_path: String,
    output_bytes: u64,
    created_ms: i64,
    title: String,
}

struct BoardInfo {
    id: i64,
    artwork_id: String,
    name: String,
    trashed: bool,
}

struct BoardImageInfo {
    board_id: i64,
    file_path: String,
}

#[derive(Default)]
struct BranchAgg {
    branch_id: String,
    title: String,
    snapshot_bytes: u64,
    delta_bytes: u64,
    final_artifact_bytes: u64,
    certification_bytes: u64,
    nodes: Vec<StorageNode>,
    final_artifact: Option<StorageArtifact>,
    certification_records: Vec<StorageCertification>,
}

struct ArtworkAgg {
    artwork_id: String,
    title: String,
    group_path: Vec<String>,
    trashed: bool,
    snapshot_bytes: u64,
    delta_bytes: u64,
    final_artifact_bytes: u64,
    certification_bytes: u64,
    node_count: u64,
    branches: HashMap<String, BranchAgg>,
    boards: Vec<StorageBoard>,
}

/// 数据库记录口径的聚合入口：固定八条批量查询读完全部输入，再在内存里按
/// 作品 / 分支 / 节点组装，不按行回查数据库。
fn overview_from_connection(connection: &Connection) -> Result<RepositoryStorageOverview, String> {
    let library_nodes = load_library_nodes(connection)?;
    let branches = load_branches(connection)?;
    let history_nodes = load_history_nodes(connection)?;
    let edges = load_edges(connection)?;
    let artifacts = load_artifacts(connection)?;
    let certifications = load_certifications(connection)?;
    let boards = load_boards(connection)?;
    let board_images = load_board_images(connection)?;

    // 归属骨架：library_nodes 提供作品与分组路径，branches 提供作品下的分支。
    let mut artworks: HashMap<String, ArtworkAgg> = HashMap::new();
    for (id, node) in &library_nodes {
        if node.kind == "artwork" {
            artworks.insert(
                id.clone(),
                ArtworkAgg {
                    artwork_id: id.clone(),
                    title: node.title.clone(),
                    group_path: group_path(id, &library_nodes),
                    trashed: node.trashed,
                    snapshot_bytes: 0,
                    delta_bytes: 0,
                    final_artifact_bytes: 0,
                    certification_bytes: 0,
                    node_count: 0,
                    branches: HashMap::new(),
                    boards: Vec::new(),
                },
            );
        }
    }
    for branch in &branches {
        if let Some(artwork) = artworks.get_mut(&branch.artwork_id) {
            artwork.branches.insert(
                branch.id.clone(),
                BranchAgg {
                    branch_id: branch.id.clone(),
                    title: branch.title.clone(),
                    ..BranchAgg::default()
                },
            );
        }
    }

    let mut history_by_id: HashMap<&str, &HistoryInfo> = HashMap::new();
    for node in &history_nodes {
        history_by_id.insert(node.id.as_str(), node);
    }
    let branch_artwork = branches
        .iter()
        .map(|branch| (branch.id.as_str(), branch.artwork_id.as_str()))
        .collect::<HashMap<_, _>>();

    // 历史节点与边：只有持有 snapshot 的节点贡献存储字节，非 snapshot 节点的
    // `chunk_file_size` 是重建读取量，只作为明细展示。
    let mut snapshot_logical_bytes = 0_u64;
    let mut snapshot_stored_bytes = 0_u64;
    let mut snapshot_count = 0_u64;
    let mut delta_logical_bytes = 0_u64;
    let mut delta_stored_bytes = 0_u64;
    let mut delta_count = 0_u64;
    let mut unassigned: HashMap<StorageKind, (u64, u64)> = HashMap::new();

    for node in &history_nodes {
        let has_snapshot = node.snapshot_path.is_some();
        if has_snapshot {
            snapshot_logical_bytes = snapshot_logical_bytes.saturating_add(node.logical_size);
            snapshot_stored_bytes = snapshot_stored_bytes.saturating_add(node.chunk_file_size);
            snapshot_count = snapshot_count.saturating_add(1);
        }
        let Some(artwork) = artworks.get_mut(&node.artwork_id) else {
            if has_snapshot {
                add_unassigned(&mut unassigned, StorageKind::Snapshot, node.chunk_file_size);
            }
            continue;
        };
        artwork.node_count = artwork.node_count.saturating_add(1);
        if has_snapshot {
            artwork.snapshot_bytes = artwork.snapshot_bytes.saturating_add(node.chunk_file_size);
        }
        let Some(branch) = artwork.branches.get_mut(&node.branch_id) else {
            continue;
        };
        if has_snapshot {
            branch.snapshot_bytes = branch.snapshot_bytes.saturating_add(node.chunk_file_size);
        }
        branch.nodes.push(StorageNode {
            id: node.id.clone(),
            title: node.title.clone(),
            created_ms: node.created_ms,
            is_checkpoint: node.is_checkpoint,
            has_snapshot,
            logical_size: node.logical_size,
            chunk_file_size: node.chunk_file_size,
            chunk_count: node.chunk_count,
            compression_ratio: ratio(node.logical_size, node.chunk_file_size),
            snapshot_path: node.snapshot_path.clone(),
            delta_path: node.delta_path.clone(),
        });
    }

    for edge in &edges {
        delta_stored_bytes = delta_stored_bytes.saturating_add(edge.delta_size);
        delta_count = delta_count.saturating_add(1);
        let Some(child) = history_by_id.get(edge.child_history_id.as_str()) else {
            add_unassigned(&mut unassigned, StorageKind::Delta, edge.delta_size);
            continue;
        };
        if let Some(parent) = history_by_id.get(edge.parent_history_id.as_str()) {
            delta_logical_bytes = delta_logical_bytes.saturating_add(parent.logical_size);
        }
        let Some(artwork) = artworks.get_mut(&child.artwork_id) else {
            add_unassigned(&mut unassigned, StorageKind::Delta, edge.delta_size);
            continue;
        };
        artwork.delta_bytes = artwork.delta_bytes.saturating_add(edge.delta_size);
        if let Some(branch) = artwork.branches.get_mut(&child.branch_id) {
            branch.delta_bytes = branch.delta_bytes.saturating_add(edge.delta_size);
        }
    }

    // 成品与认证记录：先按规范化路径去重（同一输出可能有多条记录），再按分支归属。
    let mut final_artifact_bytes = 0_u64;
    let mut final_artifact_count = 0_u64;
    let mut seen_artifact_paths = HashSet::new();
    for artifact in &artifacts {
        if !seen_artifact_paths.insert(normalize_path_key(&artifact.source_path)) {
            continue;
        }
        final_artifact_bytes = final_artifact_bytes.saturating_add(artifact.byte_size);
        final_artifact_count = final_artifact_count.saturating_add(1);
        // 记录挂在分支上；分支或所属作品缺失时只能进「未归属」，不能凭记录猜归属。
        let Some(artwork) = branch_artwork
            .get(artifact.branch_id.as_str())
            .and_then(|artwork_id| artworks.get_mut(*artwork_id))
        else {
            add_unassigned(
                &mut unassigned,
                StorageKind::FinalArtifact,
                artifact.byte_size,
            );
            continue;
        };
        artwork.final_artifact_bytes = artwork
            .final_artifact_bytes
            .saturating_add(artifact.byte_size);
        let Some(branch) = artwork.branches.get_mut(&artifact.branch_id) else {
            continue;
        };
        branch.final_artifact_bytes = branch
            .final_artifact_bytes
            .saturating_add(artifact.byte_size);
        branch.final_artifact = Some(StorageArtifact {
            id: artifact.id.clone(),
            source_path: artifact.source_path.clone(),
            byte_size: artifact.byte_size,
            created_ms: artifact.created_ms,
        });
    }

    let mut certification_bytes = 0_u64;
    let mut certification_count = 0_u64;
    let mut seen_output_paths = HashSet::new();
    for record in &certifications {
        if !seen_output_paths.insert(normalize_path_key(&record.output_path)) {
            continue;
        }
        certification_bytes = certification_bytes.saturating_add(record.output_bytes);
        certification_count = certification_count.saturating_add(1);
        let Some(artwork) = branch_artwork
            .get(record.branch_id.as_str())
            .and_then(|artwork_id| artworks.get_mut(*artwork_id))
        else {
            add_unassigned(
                &mut unassigned,
                StorageKind::Certification,
                record.output_bytes,
            );
            continue;
        };
        artwork.certification_bytes = artwork
            .certification_bytes
            .saturating_add(record.output_bytes);
        let Some(branch) = artwork.branches.get_mut(&record.branch_id) else {
            continue;
        };
        branch.certification_bytes = branch
            .certification_bytes
            .saturating_add(record.output_bytes);
        branch.certification_records.push(StorageCertification {
            id: record.id.clone(),
            title: record.title.clone(),
            output_path: record.output_path.clone(),
            stored_path: record.stored_path.clone(),
            output_bytes: record.output_bytes,
            created_ms: record.created_ms,
        });
    }

    // 画板：图片数只看有实体文件的条目，与磁盘 DDS 分类同一口径。
    let mut board_images_by_board: HashMap<i64, u64> = HashMap::new();
    for image in &board_images {
        if !image.file_path.trim().is_empty() {
            *board_images_by_board.entry(image.board_id).or_default() += 1;
        }
    }
    for board in &boards {
        if let Some(artwork) = artworks.get_mut(&board.artwork_id) {
            artwork.boards.push(StorageBoard {
                board_id: board.id,
                name: board.name.clone(),
                trashed: board.trashed,
                image_count: *board_images_by_board.get(&board.id).unwrap_or(&0),
            });
        }
    }

    // 输出排序：字节降序，同值按名称与 id 兜底，保证同一仓库两次读取顺序一致。
    let mut artwork_values = Vec::with_capacity(artworks.len());
    for mut artwork in artworks.into_values() {
        artwork.branches.values_mut().for_each(|branch| {
            branch.nodes.sort_by(|left, right| {
                right
                    .created_ms
                    .cmp(&left.created_ms)
                    .then_with(|| left.id.cmp(&right.id))
            })
        });
        let mut branches = artwork
            .branches
            .into_values()
            .map(|branch| StorageBranch {
                branch_id: branch.branch_id,
                title: branch.title,
                snapshot_bytes: branch.snapshot_bytes,
                delta_bytes: branch.delta_bytes,
                final_artifact_bytes: branch.final_artifact_bytes,
                certification_bytes: branch.certification_bytes,
                recorded_bytes: recorded_total([
                    branch.snapshot_bytes,
                    branch.delta_bytes,
                    branch.final_artifact_bytes,
                    branch.certification_bytes,
                ]),
                final_artifact: branch.final_artifact,
                certification_records: branch.certification_records,
                nodes: branch.nodes,
            })
            .collect::<Vec<_>>();
        branches.sort_by(|left, right| {
            right
                .recorded_bytes
                .cmp(&left.recorded_bytes)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.branch_id.cmp(&right.branch_id))
        });
        artwork.boards.sort_by(|left, right| {
            left.trashed
                .cmp(&right.trashed)
                .then(left.board_id.cmp(&right.board_id))
        });
        let recorded_bytes = recorded_total([
            artwork.snapshot_bytes,
            artwork.delta_bytes,
            artwork.final_artifact_bytes,
            artwork.certification_bytes,
        ]);
        artwork_values.push(StorageArtwork {
            artwork_id: artwork.artwork_id,
            title: artwork.title,
            group_path: artwork.group_path,
            trashed: artwork.trashed,
            snapshot_bytes: artwork.snapshot_bytes,
            delta_bytes: artwork.delta_bytes,
            final_artifact_bytes: artwork.final_artifact_bytes,
            certification_bytes: artwork.certification_bytes,
            recorded_bytes,
            node_count: artwork.node_count,
            branch_count: branches.len() as u64,
            branches,
            boards: artwork.boards,
        });
    }
    artwork_values.sort_by(|left, right| {
        right
            .recorded_bytes
            .cmp(&left.recorded_bytes)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.artwork_id.cmp(&right.artwork_id))
    });

    let categories = vec![
        StorageCategory {
            kind: StorageKind::Snapshot,
            bytes: snapshot_stored_bytes,
            file_count: snapshot_count,
        },
        StorageCategory {
            kind: StorageKind::Delta,
            bytes: delta_stored_bytes,
            file_count: delta_count,
        },
        StorageCategory {
            kind: StorageKind::FinalArtifact,
            bytes: final_artifact_bytes,
            file_count: final_artifact_count,
        },
        StorageCategory {
            kind: StorageKind::Certification,
            bytes: certification_bytes,
            file_count: certification_count,
        },
    ];
    let database_recorded_bytes = categories
        .iter()
        .fold(0_u64, |sum, category| sum.saturating_add(category.bytes));

    let mut unassigned = unassigned
        .into_iter()
        .map(|(kind, (bytes, count))| StorageUnassigned { kind, bytes, count })
        .collect::<Vec<_>>();
    unassigned.sort_by_key(|entry| entry.kind);

    Ok(RepositoryStorageOverview {
        calculated_at_ms: storage::now_ms()?,
        database_recorded_bytes,
        categories,
        compression: StorageCompression {
            snapshot: StorageRatio {
                logical_bytes: snapshot_logical_bytes,
                stored_bytes: snapshot_stored_bytes,
                ratio: ratio(snapshot_logical_bytes, snapshot_stored_bytes),
            },
            delta: StorageRatio {
                logical_bytes: delta_logical_bytes,
                stored_bytes: delta_stored_bytes,
                ratio: ratio(delta_logical_bytes, delta_stored_bytes),
            },
        },
        artworks: artwork_values,
        unassigned,
    })
}

fn load_library_nodes(connection: &Connection) -> Result<HashMap<String, LibraryNode>, String> {
    note_query();
    let mut statement = connection
        .prepare(
            "SELECT id, parent_id, kind, title, trashed_ms
               FROM library_nodes",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                LibraryNode {
                    parent_id: row.get(1)?,
                    kind: row.get(2)?,
                    title: row.get(3)?,
                    trashed: row.get::<_, Option<i64>>(4)?.is_some(),
                },
            ))
        })
        .map_err(storage::database_error)?;
    let mut nodes = HashMap::new();
    for row in rows {
        let (id, node) = row.map_err(storage::database_error)?;
        nodes.insert(id, node);
    }
    Ok(nodes)
}

fn load_branches(connection: &Connection) -> Result<Vec<BranchInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare("SELECT id, artwork_id, title FROM branches")
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(BranchInfo {
                id: row.get(0)?,
                artwork_id: row.get(1)?,
                title: row.get(2)?,
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_history_nodes(connection: &Connection) -> Result<Vec<HistoryInfo>, String> {
    note_query();
    // 只回显节点当前的存储表示：节点持有 snapshot 时兼容列 `delta_path` 只是遗留
    // 记账，按空处理；其余情况把空字符串归一为 NULL，与 `node_storage_path` 口径一致。
    let mut statement = connection
        .prepare(
            "SELECT id, artwork_id, created_on_branch_id, title,
                    is_checkpoint, created_ms, logical_size, chunk_file_size,
                    chunk_count, snapshot_path,
                    CASE WHEN snapshot_path IS NOT NULL THEN NULL
                         ELSE NULLIF(TRIM(delta_path), '') END
               FROM history_nodes",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(HistoryInfo {
                id: row.get(0)?,
                artwork_id: row.get(1)?,
                branch_id: row.get(2)?,
                title: row.get(3)?,
                is_checkpoint: row.get::<_, i64>(4)? != 0,
                created_ms: row.get(5)?,
                logical_size: positive(row.get::<_, i64>(6)?),
                chunk_file_size: positive(row.get::<_, i64>(7)?),
                chunk_count: positive(row.get::<_, i64>(8)?),
                snapshot_path: row.get(9)?,
                delta_path: row.get(10)?,
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_edges(connection: &Connection) -> Result<Vec<EdgeInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare(
            "SELECT child_history_id, parent_history_id, delta_size
               FROM history_edges",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(EdgeInfo {
                child_history_id: row.get(0)?,
                parent_history_id: row.get(1)?,
                delta_size: positive(row.get::<_, i64>(2)?),
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_artifacts(connection: &Connection) -> Result<Vec<ArtifactInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare(
            "SELECT id, branch_id, source_path, byte_size, created_ms
               FROM final_artifacts
              ORDER BY created_ms, id",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ArtifactInfo {
                id: row.get(0)?,
                branch_id: row.get(1)?,
                source_path: row.get(2)?,
                byte_size: positive(row.get::<_, i64>(3)?),
                created_ms: row.get(4)?,
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_certifications(connection: &Connection) -> Result<Vec<CertificationInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare(
            "SELECT id, branch_id, output_path, stored_path, output_bytes,
                    created_ms, title
               FROM certification_records
              ORDER BY created_ms, id",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(CertificationInfo {
                id: row.get(0)?,
                branch_id: row.get(1)?,
                output_path: row.get(2)?,
                stored_path: row.get(3)?,
                output_bytes: positive(row.get::<_, i64>(4)?),
                created_ms: row.get(5)?,
                title: row.get(6)?,
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_boards(connection: &Connection) -> Result<Vec<BoardInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare("SELECT id, artwork_id, name, deleted_at FROM pin_boards")
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(BoardInfo {
                id: row.get(0)?,
                artwork_id: row.get(1)?,
                name: row.get(2)?,
                trashed: row.get::<_, Option<i64>>(3)?.is_some(),
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn load_board_images(connection: &Connection) -> Result<Vec<BoardImageInfo>, String> {
    note_query();
    let mut statement = connection
        .prepare(
            "SELECT i.board_id, b.file_path
               FROM pin_board_items i
               JOIN pin_board_bitmaps b ON b.item_id = i.id
              WHERE TRIM(b.file_path) <> ''",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(BoardImageInfo {
                board_id: row.get(0)?,
                file_path: row.get(1)?,
            })
        })
        .map_err(storage::database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
}

fn group_path(node_id: &str, nodes: &HashMap<String, LibraryNode>) -> Vec<String> {
    let mut titles = Vec::new();
    let mut seen = HashSet::new();
    let mut current = nodes.get(node_id).and_then(|node| node.parent_id.clone());
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            break;
        }
        let Some(node) = nodes.get(&id) else {
            break;
        };
        titles.push(node.title.clone());
        current = node.parent_id.clone();
    }
    titles.reverse();
    titles
}

/// 四类记录字节之和。仓储规模下不会溢出，仍用 saturating 让异常数据退化成
/// 饱和值而不是 panic。
fn recorded_total(parts: [u64; 4]) -> u64 {
    parts.into_iter().fold(0_u64, u64::saturating_add)
}

fn positive(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn ratio(logical: u64, stored: u64) -> Option<f64> {
    (stored > 0).then(|| logical as f64 / stored as f64)
}

fn normalize_path_key(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

struct BoardReference {
    artwork_id: String,
    board_id: i64,
}

/// 磁盘扫描的比对计划：数据库声明的受管理文件（按规范化路径）与画板归属。
///
/// `recorded` 保留每类声明过的全部路径，扫描结束后用它找出「有记录但磁盘缺失」。
#[derive(Default)]
struct DiskPlan {
    snapshots: HashMap<String, String>,
    deltas: HashMap<String, String>,
    final_artifacts: HashMap<String, String>,
    certifications: HashMap<String, String>,
    dds: HashMap<String, BoardReference>,
    recorded: HashMap<StorageKind, HashSet<String>>,
}

/// 逐行读取「所属作品 + 受管理文件相对路径」并登记分类。
///
/// 五类受管理文件（快照、边增量、兼容列增量、最终成品、认证仓库副本）都是同一形状
/// 的两列结果，差异只在 SQL 与目标分类；画板 DDS 由 [`disk_plan`] 单独处理，因为
/// 它还要额外拼出磁盘路径并记录画板归属。
fn for_each_recorded_path(
    connection: &Connection,
    sql: &str,
    mut visit: impl FnMut(String, String),
) -> Result<(), String> {
    let mut statement = connection.prepare(sql).map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in rows {
        let (artwork_id, path) = row.map_err(storage::database_error)?;
        visit(artwork_id, path);
    }
    Ok(())
}

fn disk_plan(connection: &Connection) -> Result<DiskPlan, String> {
    let mut plan = DiskPlan::default();

    for_each_recorded_path(
        connection,
        "SELECT artwork_id, snapshot_path
           FROM history_nodes
          WHERE snapshot_path IS NOT NULL AND TRIM(snapshot_path) <> ''",
        |artwork_id, path| insert_recorded(&mut plan, StorageKind::Snapshot, &path, artwork_id),
    )?;
    for_each_recorded_path(
        connection,
        "SELECT n.artwork_id, e.delta_path
           FROM history_edges e
           JOIN history_nodes n ON n.id = e.child_history_id
          WHERE TRIM(e.delta_path) <> ''",
        |artwork_id, path| insert_recorded(&mut plan, StorageKind::Delta, &path, artwork_id),
    )?;
    // 兼容列：历史根节点或旧仓库可能仍保存 delta_path；磁盘上它仍是受管理文件。
    for_each_recorded_path(
        connection,
        "SELECT artwork_id, delta_path
           FROM history_nodes
          WHERE delta_path IS NOT NULL AND TRIM(delta_path) <> ''",
        |artwork_id, path| insert_recorded(&mut plan, StorageKind::Delta, &path, artwork_id),
    )?;
    for_each_recorded_path(
        connection,
        "SELECT b.artwork_id, f.source_path
           FROM final_artifacts f
           JOIN branches b ON b.id = f.branch_id
          WHERE TRIM(f.source_path) <> ''",
        |artwork_id, path| {
            insert_recorded(&mut plan, StorageKind::FinalArtifact, &path, artwork_id)
        },
    )?;
    for_each_recorded_path(
        connection,
        "SELECT b.artwork_id, r.stored_path
           FROM certification_records r
           JOIN branches b ON b.id = r.branch_id
          WHERE TRIM(r.stored_path) <> ''",
        |artwork_id, path| {
            insert_recorded(&mut plan, StorageKind::Certification, &path, artwork_id)
        },
    )?;

    let mut statement = connection
        .prepare(
            "SELECT b.artwork_id, i.board_id, bm.file_path
               FROM pin_board_items i
               JOIN pin_board_bitmaps bm ON bm.item_id = i.id
               JOIN pin_boards b ON b.id = i.board_id
              WHERE TRIM(bm.file_path) <> ''",
        )
        .map_err(storage::database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage::database_error)?;
    for row in rows {
        let (artwork_id, board_id, file_path) = row.map_err(storage::database_error)?;
        let relative = format!(
            "artworks/{artwork_id}/boards/{board_id}/{}",
            file_path.replace('\\', "/")
        );
        if storage::validate_repository_relative_path(&relative).is_err() {
            continue;
        }
        let key = normalize_path_key(&relative);
        plan.dds.insert(
            key.clone(),
            BoardReference {
                artwork_id,
                board_id,
            },
        );
        plan.recorded
            .entry(StorageKind::BoardDds)
            .or_default()
            .insert(key);
    }

    Ok(plan)
}

fn insert_recorded(plan: &mut DiskPlan, kind: StorageKind, path: &str, artwork_id: String) {
    let key = normalize_path_key(path);
    let map = match kind {
        StorageKind::Snapshot => &mut plan.snapshots,
        StorageKind::Delta => &mut plan.deltas,
        StorageKind::FinalArtifact => &mut plan.final_artifacts,
        StorageKind::Certification => &mut plan.certifications,
        // 画板 DDS 的路径要额外拼出画板目录，走不到这里；其余分类不是受管理文件。
        StorageKind::BoardDds | StorageKind::Other => return,
    };
    map.entry(key.clone()).or_insert(artwork_id);
    plan.recorded.entry(kind).or_default().insert(key);
}

/// 扫描期列举到的一个文件：路径与列举时刻的大小。
///
/// 目录项本身就带大小，因此不再逐文件二次 `stat`；代价是扫描结果反映列举时刻的
/// 快照——扫描期间被改写或删除的文件不会让整次扫描失败。
struct ScannedFile {
    path: PathBuf,
    size: u64,
}

fn scan_files(
    root: &Path,
    plan: &DiskPlan,
    cancelled: &impl Fn() -> bool,
    progress: &impl Fn(u64, u64),
) -> Result<RepositoryStorageDiskScan, String> {
    let files = collect_repository_files(root, cancelled)?;
    let total = files.len() as u64;
    let mut category_bytes: HashMap<StorageKind, (u64, u64)> = HashMap::new();
    let mut other = StorageOtherBreakdown::default();
    let mut artwork_bytes: HashMap<String, u64> = HashMap::new();
    let mut dds_by_board: HashMap<i64, u64> = HashMap::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0_u64;
    let mut file_count = 0_u64;

    for (index, file) in files.iter().enumerate() {
        if cancelled() {
            return Err(SCAN_CANCELLED.into());
        }
        let relative = storage::relative_path(root, &file.path)?;
        let key = normalize_path_key(&relative);
        let size = file.size;
        total_bytes = total_bytes.saturating_add(size);
        file_count = file_count.saturating_add(1);
        seen.insert(key.clone());

        // 数据库与暂存目录先归入「其它」：它们即使与受管理路径同名也不该被
        // 当作受管理文件，因此判定顺序固定。
        if is_database_file(&relative) {
            other.sqlite_bytes = other.sqlite_bytes.saturating_add(size);
            other.sqlite_files = other.sqlite_files.saturating_add(1);
        } else if is_temp_path(&key) {
            other.temp_bytes = other.temp_bytes.saturating_add(size);
            other.temp_files = other.temp_files.saturating_add(1);
        } else if let Some((kind, artwork_id, board_id)) = plan_lookup(plan, &key) {
            add_category(&mut category_bytes, kind, size);
            add_artwork(&mut artwork_bytes, artwork_id, size);
            if let Some(board_id) = board_id {
                *dds_by_board.entry(board_id).or_default() += size;
            }
        } else {
            other.unrecognized_bytes = other.unrecognized_bytes.saturating_add(size);
            other.unrecognized_files = other.unrecognized_files.saturating_add(1);
        }
        progress(index as u64 + 1, total);
    }

    let other_bytes = other
        .sqlite_bytes
        .saturating_add(other.temp_bytes)
        .saturating_add(other.unrecognized_bytes);
    let other_files = other
        .sqlite_files
        .saturating_add(other.temp_files)
        .saturating_add(other.unrecognized_files);
    category_bytes.insert(StorageKind::Other, (other_bytes, other_files));

    let mut categories = [
        StorageKind::Snapshot,
        StorageKind::Delta,
        StorageKind::FinalArtifact,
        StorageKind::Certification,
        StorageKind::BoardDds,
        StorageKind::Other,
    ]
    .into_iter()
    .map(|kind| {
        let (bytes, file_count) = category_bytes.get(&kind).copied().unwrap_or((0, 0));
        StorageCategory {
            kind,
            bytes,
            file_count,
        }
    })
    .collect::<Vec<_>>();
    categories.sort_by_key(|category| category.kind);

    let mut missing_files = plan
        .recorded
        .iter()
        .filter_map(|(kind, paths)| {
            let count = paths.iter().filter(|path| !seen.contains(*path)).count() as u64;
            (count > 0).then_some(StorageMissingFile { kind: *kind, count })
        })
        .collect::<Vec<_>>();
    missing_files.sort_by_key(|entry| entry.kind);

    let mut artwork_bytes = artwork_bytes
        .into_iter()
        .map(|(artwork_id, bytes)| StorageArtworkDiskBytes { artwork_id, bytes })
        .collect::<Vec<_>>();
    artwork_bytes.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.artwork_id.cmp(&right.artwork_id))
    });
    let mut dds_by_board = dds_by_board
        .into_iter()
        .map(|(board_id, bytes)| StorageBoardDiskBytes { board_id, bytes })
        .collect::<Vec<_>>();
    dds_by_board.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.board_id.cmp(&right.board_id))
    });

    Ok(RepositoryStorageDiskScan {
        scanned_at_ms: storage::now_ms()?,
        file_count,
        total_bytes,
        categories,
        other_breakdown: other,
        artwork_bytes,
        dds_by_board,
        missing_files,
    })
}

/// 按固定优先级把规范化路径映射到分类与归属：快照 → 增量 → 成品 → 认证 → 画板 DDS。
///
/// 同一路径不会被两类同时声明（发布产物与历史文件分处不同目录），顺序只用于让
/// 判定结果确定、便于排查。
fn plan_lookup<'a>(plan: &'a DiskPlan, key: &str) -> Option<(StorageKind, &'a str, Option<i64>)> {
    if let Some(artwork_id) = plan.snapshots.get(key) {
        return Some((StorageKind::Snapshot, artwork_id, None));
    }
    if let Some(artwork_id) = plan.deltas.get(key) {
        return Some((StorageKind::Delta, artwork_id, None));
    }
    if let Some(artwork_id) = plan.final_artifacts.get(key) {
        return Some((StorageKind::FinalArtifact, artwork_id, None));
    }
    if let Some(artwork_id) = plan.certifications.get(key) {
        return Some((StorageKind::Certification, artwork_id, None));
    }
    plan.dds.get(key).map(|reference| {
        (
            StorageKind::BoardDds,
            reference.artwork_id.as_str(),
            Some(reference.board_id),
        )
    })
}

/// 遍历整个仓库目录，返回文件路径与列举时刻的大小。
///
/// 与 `cleanup::scan_unreferenced` 的遍历职责不同：后者只看 `artworks/` 下符合受管理
/// 命名模式的候选，这里要统计仓库目录里的全部普通文件（含 SQLite、`temp/` 与未识别
/// 文件）。两者都跳过符号链接与本进程的运行期文件。
fn collect_repository_files(
    root: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<ScannedFile>, String> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        if cancelled() {
            return Err(SCAN_CANCELLED.into());
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法读取仓库目录 {}：{error}", directory.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取仓库目录项：{error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("无法读取仓库目录项类型：{error}"))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if let Ok(relative) = path.strip_prefix(root) {
                if repository_lock::is_runtime_artifact(relative) {
                    continue;
                }
            }
            let size = match entry.metadata() {
                Ok(metadata) => metadata.len(),
                // 列举与取大小之间文件被删除是可能的：跳过它，不让整次扫描失败。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("无法读取仓库文件信息：{error}")),
            };
            files.push(ScannedFile { path, size });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn is_database_file(relative: &str) -> bool {
    if relative.contains('/') {
        return false;
    }
    relative.ends_with(".sqlite3")
        || relative.ends_with(".sqlite3-wal")
        || relative.ends_with(".sqlite3-shm")
        || relative.ends_with(".sqlite3-journal")
}

/// 仓库 `temp/` 与作品 `artworks/<artwork>/temp/` 都是暂存区：备份、恢复与检查点
/// 在发布前把 snapshot/delta 写在这里，进程被强杀或断电时 `NamedTempFile` 的
/// `Drop` 不再执行，留下的 `.tmp*` 残骸没有任何数据库引用。它们既不是受管理
/// 文件，也不该被算作「未识别」，因此统一并入「仓库 temp」。
///
/// 入参是 [`normalize_path_key`] 处理后的路径（`/` 分隔、小写），因此判定与
/// 大小写无关。
fn is_temp_path(key: &str) -> bool {
    if key.starts_with("temp/") {
        return true;
    }
    let mut segments = key.split('/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some("artworks"), Some(_), Some("temp"))
    )
}

fn add_category(categories: &mut HashMap<StorageKind, (u64, u64)>, kind: StorageKind, bytes: u64) {
    let entry = categories.entry(kind).or_default();
    entry.0 = entry.0.saturating_add(bytes);
    entry.1 = entry.1.saturating_add(1);
}

fn add_artwork(artworks: &mut HashMap<String, u64>, artwork_id: &str, bytes: u64) {
    let entry = artworks.entry(artwork_id.to_owned()).or_default();
    *entry = entry.saturating_add(bytes);
}

fn add_unassigned(
    unassigned: &mut HashMap<StorageKind, (u64, u64)>,
    kind: StorageKind,
    bytes: u64,
) {
    let entry = unassigned.entry(kind).or_default();
    entry.0 = entry.0.saturating_add(bytes);
    entry.1 = entry.1.saturating_add(1);
}

#[cfg(test)]
mod tests;
