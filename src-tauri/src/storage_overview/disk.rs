//! 仓储空间的磁盘口径：把数据库声明的受管理文件映射到磁盘目录。
//!
//! 目录遍历是长任务，扫描前先关闭数据库连接；结果按与记录口径相同的分类聚合，
//! 但两套口径不静默合并（见模块文档）。

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use crate::{repository_lock, storage};

use super::{
    normalize_path_key, RepositoryStorageDiskScan, StorageArtworkDiskBytes, StorageBoardDiskBytes,
    StorageCategory, StorageKind, StorageMissingFile, StorageOtherBreakdown,
};

/// 磁盘扫描被取消时的统一文案；目录遍历与逐文件统计共用。
const SCAN_CANCELLED: &str = "磁盘占用扫描已取消";

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
