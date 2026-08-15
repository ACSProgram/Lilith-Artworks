use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};

use crate::{
    cleanup,
    storage::{
        self, database_error, display_path, new_id, normalize_source_path, now_ms, validate_title,
    },
};

use super::model::{
    ArtworkSummary, CreatedArtwork, LibraryNode, LibrarySearchResult, LibraryTrashEntry,
    LibraryTree, MoveLibraryNodesRequest, PrimaryBranch,
};

const MAX_MOVE_NODES: usize = 512;
const MAX_SEARCH_CHARS: usize = 160;
const MAX_SEARCH_RESULTS: usize = 100;

pub(crate) fn database_path(root: &Path) -> PathBuf {
    storage::database_path(root)
}

pub(crate) fn initialize(root: &Path) -> Result<(), String> {
    validate_repository_root(root)?;
    let existed = root.exists();
    if existed && !root.is_dir() {
        return Err("作品仓库路径不是目录".into());
    }
    if !existed {
        fs::create_dir_all(root).map_err(|error| format!("无法创建作品仓库：{error}"))?;
    }

    let database = locate_database(root)?;
    let create = !database.exists()
        || database
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(false);

    let connection = Connection::open(&database)
        .map_err(|error| format!("无法打开作品数据库 {}：{error}", display_path(&database)))?;
    configure(&connection)?;
    if create {
        super::schema::create(&connection)?;
        create_directories(root)?;
    } else {
        super::schema::validate_and_migrate(&connection)?;
        create_directories(root)?;
    }
    Ok(())
}

pub(crate) fn open_existing(root: &Path) -> Result<(), String> {
    validate_existing_root(root)?;
    let database = locate_database(root)?;
    require_database_file(&database)?;

    let connection = storage::open(root)?;
    super::schema::validate_and_migrate(&connection)?;
    create_directories(root)
}

pub(crate) fn check_existing(root: &Path) -> Result<(), String> {
    validate_existing_root(root)?;
    let database = database_path(root);
    require_database_file(&database)?;
    let connection = storage::open(root)?;
    super::schema::validate_current(&connection)
}

fn validate_existing_root(root: &Path) -> Result<(), String> {
    validate_repository_root(root)?;
    if !root.exists() {
        return Err("已配置的作品仓库目录不存在".into());
    }
    if !root.is_dir() {
        return Err("作品仓库路径不是目录".into());
    }
    Ok(())
}

fn require_database_file(database: &Path) -> Result<(), String> {
    if !database.exists()
        || database
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(true)
    {
        return Err(format!(
            "已配置的作品仓库缺少数据库 {}，请重新选择仓库或从备份恢复",
            display_path(database)
        ));
    }
    Ok(())
}

fn locate_database(root: &Path) -> Result<PathBuf, String> {
    let preferred = database_path(root);
    if preferred.exists()
        && preferred
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
    {
        return Ok(preferred);
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| format!("无法读取作品仓库内容：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取作品仓库目录项：{error}"))?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("sqlite3") {
            candidates.push(path);
        }
    }

    for candidate in candidates {
        if candidate == preferred {
            continue;
        }
        let matches_repository = {
            let connection =
                match Connection::open_with_flags(&candidate, OpenFlags::SQLITE_OPEN_READ_WRITE) {
                    Ok(connection) => connection,
                    Err(_) => continue,
                };
            let format = connection
                .query_row(
                    "SELECT value FROM repository_meta WHERE key = 'format'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional();
            let matches =
                format.ok().flatten().as_deref() == Some(super::schema::REPOSITORY_FORMAT);
            if matches {
                let busy: i64 = connection
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
                    .map_err(|error| {
                        format!(
                            "无法合并备用作品数据库 {} 的 WAL：{error}",
                            display_path(&candidate)
                        )
                    })?;
                if busy != 0 {
                    return Err(format!(
                        "备用作品数据库 {} 正在使用，无法完成 WAL 合并",
                        display_path(&candidate)
                    ));
                }
            }
            matches
        };
        if matches_repository {
            remove_sqlite_sidecars(&candidate)?;
            if preferred.exists()
                && preferred
                    .metadata()
                    .map(|metadata| metadata.len() == 0)
                    .unwrap_or(false)
            {
                fs::remove_file(&preferred)
                    .map_err(|error| format!("无法移除空数据库占位文件：{error}"))?;
                remove_sqlite_sidecars(&preferred)?;
            }
            fs::rename(&candidate, &preferred).map_err(|error| {
                format!(
                    "无法迁移已发现的作品数据库 {}：{error}",
                    display_path(&candidate)
                )
            })?;
            return Ok(preferred);
        }
    }

    if preferred.exists() {
        Ok(preferred)
    } else if fs::read_dir(root)
        .map_err(|error| format!("无法读取作品仓库内容：{error}"))?
        .next()
        .transpose()
        .map_err(|error| format!("无法读取作品仓库内容：{error}"))?
        .is_some()
    {
        Err("所选目录非空，且没有可读取的 Lilith Artworks 数据库".into())
    } else {
        Ok(preferred)
    }
}

fn remove_sqlite_sidecars(database: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path(database, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "无法清理 SQLite sidecar {}：{error}",
                    display_path(&sidecar)
                ))
            }
        }
    }
    Ok(())
}

fn sqlite_sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut value = database.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_repository_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("作品仓库必须使用绝对目录路径".into());
    }
    if root.parent().is_none() || root.file_name().is_none() {
        return Err("不能把磁盘或文件系统根目录用作作品仓库".into());
    }
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), String> {
    storage::configure(connection)
}

fn create_directories(root: &Path) -> Result<(), String> {
    for relative in ["artworks", "temp"] {
        fs::create_dir_all(root.join(relative))
            .map_err(|error| format!("无法创建仓库目录 {relative}：{error}"))?;
    }
    Ok(())
}

pub(crate) fn list_tree(root: &Path) -> Result<LibraryTree, String> {
    let connection = open(root)?;
    list_tree_with_connection(&connection)
}

pub(crate) fn search(root: &Path, query: &str) -> Result<Vec<LibrarySearchResult>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.chars().count() > MAX_SEARCH_CHARS {
        return Err(format!("搜索内容不能超过 {MAX_SEARCH_CHARS} 个字符"));
    }
    let query_key = query.to_lowercase();
    let connection = open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT n.id, n.parent_id, n.kind, n.title,
                    (SELECT source_path FROM branches
                     WHERE artwork_id = n.id ORDER BY created_ms, id LIMIT 1)
             FROM library_nodes n
             WHERE n.trashed_ms IS NULL
             ORDER BY n.parent_id, n.position, n.id",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;

    let parents = rows
        .iter()
        .map(|(id, parent_id, _, _, _)| (id.clone(), parent_id.clone()))
        .collect::<HashMap<_, _>>();
    let titles = rows
        .iter()
        .map(|(id, _, _, title, _)| (id.clone(), title.clone()))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();
    for (id, _, kind, title, source_path) in rows {
        let matches = title.to_lowercase().contains(&query_key)
            || source_path
                .as_deref()
                .is_some_and(|path| path.to_lowercase().contains(&query_key));
        if !matches {
            continue;
        }
        let ancestor_ids = ancestor_ids(&id, &parents)?;
        let mut breadcrumb_parts = ancestor_ids
            .iter()
            .filter_map(|ancestor| titles.get(ancestor).cloned())
            .collect::<Vec<_>>();
        breadcrumb_parts.push(title.clone());
        results.push(LibrarySearchResult {
            id,
            kind,
            title,
            breadcrumb: breadcrumb_parts.join(" / "),
            ancestor_ids,
            source_path,
        });
        if results.len() >= MAX_SEARCH_RESULTS {
            break;
        }
    }
    Ok(results)
}

pub(crate) fn create_group(
    root: &Path,
    parent_id: Option<&str>,
    title: &str,
) -> Result<LibraryTree, String> {
    create_group_id(root, parent_id, title)?;
    list_tree(root)
}

pub(crate) fn create_artwork(
    root: &Path,
    parent_id: Option<&str>,
    title: &str,
    branch_title: &str,
    source_path: &Path,
) -> Result<CreatedArtwork, String> {
    validate_title(title, "Artwork 标题")?;
    validate_title(branch_title, "分支标题")?;
    let (source_display, source_key) = normalize_source_path(root, source_path)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    ensure_group_parent(&transaction, parent_id)?;
    let now = now_ms()?;
    let artwork_id = new_id();
    let branch_id = new_id();
    let position: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM library_nodes
             WHERE parent_id IS ?1 AND trashed_ms IS NULL",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO library_nodes
             (id, parent_id, kind, title, position, created_ms, updated_ms)
             VALUES (?1, ?2, 'artwork', ?3, ?4, ?5, ?5)",
            params![artwork_id, parent_id, title.trim(), position, now],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO artworks (id, created_ms, updated_ms) VALUES (?1, ?2, ?2)",
            params![artwork_id, now],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO branches
             (id, artwork_id, title, source_path, source_path_key, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                branch_id,
                artwork_id,
                branch_title.trim(),
                source_display,
                source_key,
                now
            ],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(CreatedArtwork {
        artwork_id,
        branch_id,
    })
}

pub(crate) fn create_artwork_and_list(
    root: &Path,
    parent_id: Option<&str>,
    title: &str,
    branch_title: &str,
    source_path: &Path,
) -> Result<LibraryTree, String> {
    let created = create_artwork(root, parent_id, title, branch_title, source_path)?;
    let _created_ids = (created.artwork_id, created.branch_id);
    list_tree(root)
}

pub(crate) fn rename_node(root: &Path, id: &str, title: &str) -> Result<LibraryTree, String> {
    validate_title(title, "标题")?;
    let connection = open(root)?;
    let changed = connection
        .execute(
            "UPDATE library_nodes SET title = ?2, updated_ms = ?3
             WHERE id = ?1 AND trashed_ms IS NULL",
            params![id, title.trim(), now_ms()?],
        )
        .map_err(database_error)?;
    if changed == 0 {
        return Err("找不到要重命名的节点".into());
    }
    list_tree(root)
}

pub(crate) fn trash_nodes(root: &Path, ids: &[String]) -> Result<LibraryTree, String> {
    let ids = validate_node_ids(ids)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let parent_map = load_parent_map(&transaction, false)?;
    ensure_nodes_exist(&ids, &parent_map)?;
    let selected = top_level_selection(&ids, &parent_map)?;
    let now = now_ms()?;
    let affected_parents = selected
        .iter()
        .filter_map(|id| parent_map.get(id))
        .cloned()
        .collect::<HashSet<_>>();
    for id in &selected {
        transaction
            .execute(
                "WITH RECURSIVE subtree(id) AS (
                   SELECT id FROM library_nodes WHERE id = ?1 AND trashed_ms IS NULL
                   UNION ALL
                   SELECT child.id FROM library_nodes child
                   JOIN subtree ON child.parent_id = subtree.id
                   WHERE child.trashed_ms IS NULL
                 )
                 UPDATE library_nodes
                 SET trashed_ms = ?2,
                     trash_root_id = ?1,
                     restore_parent_id = CASE WHEN id = ?1 THEN parent_id ELSE NULL END,
                     restore_position = CASE WHEN id = ?1 THEN position ELSE NULL END,
                     parent_id = CASE WHEN id = ?1 THEN NULL ELSE parent_id END,
                     updated_ms = ?2
                 WHERE id IN (SELECT id FROM subtree)",
                params![id, now],
            )
            .map_err(database_error)?;
    }
    for parent_id in affected_parents {
        normalize_siblings(&transaction, parent_id.as_deref())?;
    }
    transaction.commit().map_err(database_error)?;
    list_tree(root)
}

pub(crate) fn list_trash(root: &Path) -> Result<Vec<LibraryTrashEntry>, String> {
    let connection = open(root)?;
    let mut statement = connection
        .prepare(
            "SELECT root.id, root.kind, root.title, root.trashed_ms,
                    (SELECT COUNT(*) - 1 FROM library_nodes member
                     WHERE member.trash_root_id = root.id),
                    (SELECT COUNT(*) FROM library_nodes member
                     WHERE member.trash_root_id = root.id AND member.kind = 'artwork'),
                    parent.title
             FROM library_nodes root
             LEFT JOIN library_nodes parent ON parent.id = root.restore_parent_id
             WHERE root.trashed_ms IS NOT NULL AND root.trash_root_id = root.id
             ORDER BY root.trashed_ms DESC, root.id",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(LibraryTrashEntry {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                deleted_ms: row.get(3)?,
                descendant_count: row.get(4)?,
                artwork_count: row.get(5)?,
                original_parent_title: row.get(6)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(rows)
}

pub(crate) fn restore_trash(root: &Path, id: &str) -> Result<LibraryTree, String> {
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let entry: Option<(Option<String>, Option<i64>)> = transaction
        .query_row(
            "SELECT restore_parent_id, restore_position FROM library_nodes
             WHERE id = ?1 AND trashed_ms IS NOT NULL AND trash_root_id = id",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    let (stored_parent, stored_position) = entry.ok_or("找不到回收站项目")?;
    let restore_parent = match stored_parent {
        Some(parent_id)
            if transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_nodes
                     WHERE id = ?1 AND kind = 'group' AND trashed_ms IS NULL)",
                    [&parent_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)? =>
        {
            Some(parent_id)
        }
        _ => None,
    };
    let mut destination = sibling_ids(&transaction, restore_parent.as_deref())?;
    let index = stored_position
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(destination.len())
        .min(destination.len());
    destination.insert(index, id.to_owned());
    transaction
        .execute(
            "WITH RECURSIVE subtree(id) AS (
               SELECT id FROM library_nodes WHERE id = ?1 AND trash_root_id = ?1
               UNION ALL
               SELECT child.id FROM library_nodes child JOIN subtree ON child.parent_id = subtree.id
               WHERE child.trash_root_id = ?1
             )
             UPDATE library_nodes
             SET trashed_ms = NULL, trash_root_id = NULL,
                 restore_parent_id = NULL, restore_position = NULL,
                 updated_ms = ?3,
                 parent_id = CASE WHEN id = ?1 THEN ?2 ELSE parent_id END
             WHERE id IN (SELECT id FROM subtree)",
            params![id, restore_parent, now_ms()?],
        )
        .map_err(database_error)?;
    update_sibling_positions(&transaction, &destination)?;
    transaction.commit().map_err(database_error)?;
    list_tree(root)
}

pub(crate) fn permanently_delete_trash(root: &Path, ids: &[String]) -> Result<Vec<String>, String> {
    let ids = validate_node_ids(ids)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let mut cleanup_ids = Vec::new();
    for id in ids {
        cleanup_ids.extend(permanently_delete_trash_root(&transaction, &id)?);
    }
    transaction.commit().map_err(database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn empty_trash(root: &Path) -> Result<Vec<String>, String> {
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    let ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM library_nodes
                 WHERE trashed_ms IS NOT NULL AND trash_root_id = id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(database_error)?;
        rows
    };
    let mut cleanup_ids = Vec::new();
    for id in ids {
        cleanup_ids.extend(permanently_delete_trash_root(&transaction, &id)?);
    }
    transaction.commit().map_err(database_error)?;
    Ok(cleanup_ids)
}

fn permanently_delete_trash_root(
    transaction: &Transaction<'_>,
    id: &str,
) -> Result<Vec<String>, String> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_nodes
             WHERE id = ?1 AND trashed_ms IS NOT NULL AND trash_root_id = id)",
            [id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err(format!("找不到回收站项目：{id}"));
    }
    let artwork_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM library_nodes
                 WHERE trash_root_id = ?1 AND kind = 'artwork'",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows
    };
    let external_outputs = {
        let mut statement = transaction
            .prepare(
                "SELECT record.output_path, record.output_sha256
                 FROM certification_records record
                 JOIN branches branch ON branch.id = record.branch_id
                 WHERE branch.artwork_id IN (
                   SELECT id FROM library_nodes
                   WHERE trash_root_id = ?1 AND kind = 'artwork'
                 )",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows
    };
    let mut cleanup_ids = Vec::new();
    for artwork_id in artwork_ids {
        cleanup_ids.push(cleanup::enqueue_repository_directory(
            transaction,
            &format!("artworks/{artwork_id}"),
            "permanent_artwork_deletion",
        )?);
    }
    for (output_path, output_sha256) in external_outputs {
        cleanup_ids.push(cleanup::enqueue_external_file(
            transaction,
            &output_path,
            &output_sha256,
            "permanent_artwork_deletion",
        )?);
    }
    transaction
        .execute(
            "UPDATE branches SET head_history_id = NULL, created_from_history_id = NULL
             WHERE artwork_id IN (
               SELECT id FROM library_nodes WHERE trash_root_id = ?1 AND kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM final_artifacts WHERE branch_id IN (
               SELECT b.id FROM branches b
               JOIN library_nodes n ON n.id = b.artwork_id
               WHERE n.trash_root_id = ?1 AND n.kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM history_nodes WHERE artwork_id IN (
               SELECT id FROM library_nodes WHERE trash_root_id = ?1 AND kind = 'artwork'
             )",
            [id],
        )
        .map_err(database_error)?;
    transaction
        .execute("DELETE FROM library_nodes WHERE id = ?1", [id])
        .map_err(database_error)?;
    Ok(cleanup_ids)
}

pub(crate) fn move_nodes(
    root: &Path,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, String> {
    let ids = validate_node_ids(&request.ids)?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    ensure_group_parent(&transaction, request.parent_id.as_deref())?;
    let parent_map = load_parent_map(&transaction, false)?;
    ensure_nodes_exist(&ids, &parent_map)?;
    let selected = top_level_selection(&ids, &parent_map)?;
    let selected_set = selected.iter().cloned().collect::<HashSet<_>>();

    if let Some(parent_id) = request.parent_id.as_deref() {
        for id in &selected {
            if id == parent_id || is_descendant(&parent_map, parent_id, id)? {
                return Err("不能把节点移动到自身或自身后代中".into());
            }
        }
    }

    let old_parents = selected
        .iter()
        .filter_map(|id| parent_map.get(id))
        .cloned()
        .collect::<HashSet<_>>();
    let mut destination = sibling_ids(&transaction, request.parent_id.as_deref())?
        .into_iter()
        .filter(|id| !selected_set.contains(id))
        .collect::<Vec<_>>();
    let index = usize::try_from(request.index)
        .unwrap_or(usize::MAX)
        .min(destination.len());
    destination.splice(index..index, selected.iter().cloned());
    let now = now_ms()?;
    for id in &selected {
        transaction
            .execute(
                "UPDATE library_nodes SET parent_id = ?2, updated_ms = ?3 WHERE id = ?1",
                params![id, request.parent_id, now],
            )
            .map_err(database_error)?;
    }
    update_sibling_positions(&transaction, &destination)?;
    for parent_id in old_parents {
        if parent_id != request.parent_id {
            normalize_siblings(&transaction, parent_id.as_deref())?;
        }
    }
    transaction.commit().map_err(database_error)?;
    list_tree(root)
}

fn create_group_id(root: &Path, parent_id: Option<&str>, title: &str) -> Result<String, String> {
    validate_title(title, "分组标题")?;
    let mut connection = open(root)?;
    let transaction = connection.transaction().map_err(database_error)?;
    ensure_group_parent(&transaction, parent_id)?;
    let id = new_id();
    let now = now_ms()?;
    let position: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM library_nodes
             WHERE parent_id IS ?1 AND trashed_ms IS NULL",
            [parent_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO library_nodes
             (id, parent_id, kind, title, position, created_ms, updated_ms)
             VALUES (?1, ?2, 'group', ?3, ?4, ?5, ?5)",
            params![id, parent_id, title.trim(), position, now],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(id)
}

fn list_tree_with_connection(connection: &Connection) -> Result<LibraryTree, String> {
    let mut branch_statement = connection
        .prepare(
            "SELECT b.artwork_id, b.id, b.title, b.source_path,
                    (SELECT COUNT(*) FROM branches counted WHERE counted.artwork_id = b.artwork_id)
             FROM branches b
             WHERE b.id = (
               SELECT first.id FROM branches first
               WHERE first.artwork_id = b.artwork_id
               ORDER BY first.created_ms, first.id LIMIT 1
             )",
        )
        .map_err(database_error)?;
    let branch_rows = branch_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                PrimaryBranch {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    source_path: row.get(3)?,
                },
                row.get::<_, u64>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let branch_by_artwork = branch_rows
        .into_iter()
        .map(|(artwork_id, branch, count)| (artwork_id, (branch, count)))
        .collect::<HashMap<_, _>>();

    let mut statement = connection
        .prepare(
            "SELECT n.id, n.parent_id, n.kind, n.title, n.position, n.updated_ms,
                    COALESCE(a.description, '')
             FROM library_nodes n
             LEFT JOIN artworks a ON a.id = n.id
             WHERE n.trashed_ms IS NULL
             ORDER BY n.parent_id, n.position, n.id",
        )
        .map_err(database_error)?;
    let mut rows = statement
        .query_map([], |row| {
            Ok(LibraryNode {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                kind: row.get(2)?,
                title: row.get(3)?,
                position: row.get(4)?,
                updated_ms: row.get(5)?,
                children: Vec::new(),
                artwork: None,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;

    let descriptions = connection
        .prepare("SELECT id, description FROM artworks")
        .map_err(database_error)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(database_error)?;
    let mut group_count = 0;
    let mut artwork_count = 0;
    for node in &mut rows {
        if node.kind == "group" {
            group_count += 1;
        } else {
            artwork_count += 1;
            let primary = branch_by_artwork.get(&node.id);
            node.artwork = Some(ArtworkSummary {
                description: descriptions.get(&node.id).cloned().unwrap_or_default(),
                branch_count: primary.map_or(0, |(_, count)| *count),
                primary_branch: primary.map(|(branch, _)| branch.clone()),
            });
        }
    }
    let nodes = build_tree(rows)?;
    Ok(LibraryTree {
        nodes,
        group_count,
        artwork_count,
    })
}

fn build_tree(rows: Vec<LibraryNode>) -> Result<Vec<LibraryNode>, String> {
    let known = rows
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    for node in &rows {
        if let Some(parent_id) = node.parent_id.as_deref() {
            if !known.contains(parent_id) {
                return Err(format!("作品树节点 {} 引用了不存在的父节点", node.id));
            }
        }
    }
    let mut children = HashMap::<Option<String>, Vec<LibraryNode>>::new();
    for node in rows {
        children
            .entry(node.parent_id.clone())
            .or_default()
            .push(node);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            left.position
                .cmp(&right.position)
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    fn attach(
        parent_id: Option<String>,
        children: &mut HashMap<Option<String>, Vec<LibraryNode>>,
        visiting: &mut HashSet<String>,
    ) -> Result<Vec<LibraryNode>, String> {
        let mut result = children.remove(&parent_id).unwrap_or_default();
        for node in &mut result {
            if !visiting.insert(node.id.clone()) {
                return Err("作品树中存在循环父子关系".into());
            }
            node.children = attach(Some(node.id.clone()), children, visiting)?;
            visiting.remove(&node.id);
        }
        Ok(result)
    }
    let roots = attach(None, &mut children, &mut HashSet::new())?;
    if !children.is_empty() {
        return Err("作品树包含无法挂载的循环节点".into());
    }
    Ok(roots)
}

fn ancestor_ids(
    id: &str,
    parents: &HashMap<String, Option<String>>,
) -> Result<Vec<String>, String> {
    let mut ancestors = Vec::new();
    let mut cursor = parents.get(id).cloned().flatten();
    let mut visited = HashSet::new();
    while let Some(parent_id) = cursor {
        if !visited.insert(parent_id.clone()) {
            return Err("作品树中存在循环父子关系".into());
        }
        ancestors.push(parent_id.clone());
        cursor = parents.get(&parent_id).cloned().flatten();
    }
    ancestors.reverse();
    Ok(ancestors)
}

fn validate_node_ids(ids: &[String]) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Err("至少需要选择一个节点".into());
    }
    if ids.len() > MAX_MOVE_NODES {
        return Err(format!("一次最多操作 {MAX_MOVE_NODES} 个节点"));
    }
    let mut seen = HashSet::new();
    let normalized = ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .filter(|id| seen.insert((*id).to_owned()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return Err("节点标识不能为空".into());
    }
    Ok(normalized)
}

fn load_parent_map(
    transaction: &Transaction<'_>,
    include_trashed: bool,
) -> Result<HashMap<String, Option<String>>, String> {
    let sql = if include_trashed {
        "SELECT id, parent_id FROM library_nodes"
    } else {
        "SELECT id, parent_id FROM library_nodes WHERE trashed_ms IS NULL"
    };
    let mut statement = transaction.prepare(sql).map_err(database_error)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(database_error)?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(database_error)
}

fn ensure_nodes_exist(
    ids: &[String],
    parents: &HashMap<String, Option<String>>,
) -> Result<(), String> {
    if let Some(id) = ids.iter().find(|id| !parents.contains_key(*id)) {
        return Err(format!("找不到节点：{id}"));
    }
    Ok(())
}

fn top_level_selection(
    ids: &[String],
    parents: &HashMap<String, Option<String>>,
) -> Result<Vec<String>, String> {
    let selected = ids.iter().cloned().collect::<HashSet<_>>();
    let mut result = Vec::new();
    for id in ids {
        let ancestors = ancestor_ids(id, parents)?;
        if ancestors
            .iter()
            .all(|ancestor| !selected.contains(ancestor))
        {
            result.push(id.clone());
        }
    }
    Ok(result)
}

fn is_descendant(
    parents: &HashMap<String, Option<String>>,
    candidate: &str,
    ancestor: &str,
) -> Result<bool, String> {
    Ok(ancestor_ids(candidate, parents)?
        .iter()
        .any(|id| id == ancestor))
}

fn sibling_ids(
    transaction: &Transaction<'_>,
    parent_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut statement = transaction
        .prepare(
            "SELECT id FROM library_nodes
             WHERE parent_id IS ?1 AND trashed_ms IS NULL ORDER BY position, id",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([parent_id], |row| row.get(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(rows)
}

fn update_sibling_positions(
    transaction: &Transaction<'_>,
    siblings: &[String],
) -> Result<(), String> {
    for (position, id) in siblings.iter().enumerate() {
        transaction
            .execute(
                "UPDATE library_nodes SET position = ?2 WHERE id = ?1",
                params![id, i64::try_from(position).map_err(|_| "节点顺序超出范围")?],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn normalize_siblings(
    transaction: &Transaction<'_>,
    parent_id: Option<&str>,
) -> Result<(), String> {
    let siblings = sibling_ids(transaction, parent_id)?;
    update_sibling_positions(transaction, &siblings)
}

fn open(root: &Path) -> Result<Connection, String> {
    storage::open(root)
}

fn ensure_group_parent(
    transaction: &Transaction<'_>,
    parent_id: Option<&str>,
) -> Result<(), String> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    let kind: Option<String> = transaction
        .query_row(
            "SELECT kind FROM library_nodes WHERE id = ?1 AND trashed_ms IS NULL",
            [parent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    match kind.as_deref() {
        Some("group") => Ok(()),
        Some("artwork") => Err("Artwork 是叶节点，不能包含子节点".into()),
        _ => Err("找不到父分组".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Fixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
        first_source: PathBuf,
        artwork: CreatedArtwork,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("repository");
            let first_source = directory.path().join("first.psd");
            fs::File::create(&first_source)
                .unwrap()
                .write_all(b"first")
                .unwrap();
            initialize(&root).unwrap();
            let group = create_group_id(&root, None, "Series").unwrap();
            let artwork =
                create_artwork(&root, Some(&group), "Portrait", "Main", &first_source).unwrap();
            Self {
                _directory: directory,
                root,
                first_source,
                artwork,
            }
        }
    }

    #[test]
    fn opening_empty_configured_repository_does_not_reinitialize_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir(&root).unwrap();
        let database = database_path(&root);

        let error = open_existing(&root).unwrap_err();

        assert!(error.contains("缺少数据库"));
        assert!(!database.exists());
    }

    #[test]
    fn discovers_and_renames_alternate_repository_database() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root).unwrap();
        let preferred = database_path(&root);
        let alternate = root.join("legacy.sqlite3");
        fs::rename(&preferred, &alternate).unwrap();

        open_existing(&root).unwrap();

        assert!(preferred.is_file());
        assert!(!alternate.exists());
    }

    #[test]
    fn alternate_repository_migration_checkpoints_committed_wal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root).unwrap();
        let preferred = database_path(&root);
        let alternate = root.join("legacy.sqlite3");
        let preferred_wal = sqlite_sidecar_path(&preferred, "-wal");
        let alternate_wal = sqlite_sidecar_path(&alternate, "-wal");

        let connection = storage::open(&root).unwrap();
        connection
            .execute_batch(
                "PRAGMA wal_autocheckpoint = 0;
                 INSERT INTO library_nodes
                   (id, parent_id, kind, title, position, created_ms, updated_ms)
                 VALUES ('wal-group', NULL, 'group', 'Recovered from WAL', 0, 1, 1);",
            )
            .unwrap();
        assert!(preferred_wal.is_file());
        fs::copy(&preferred, &alternate).unwrap();
        fs::copy(&preferred_wal, &alternate_wal).unwrap();
        drop(connection);
        remove_sqlite_sidecars(&preferred).unwrap();
        fs::remove_file(&preferred).unwrap();

        open_existing(&root).unwrap();

        let recovered: i64 = storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM library_nodes WHERE id = 'wal-group'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovered, 1);
        assert!(!alternate_wal.exists());
    }

    #[test]
    fn creates_artwork_with_root_branch() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        let row: (String, String, Option<String>) = connection
            .query_row(
                "SELECT artwork_id, title, head_history_id FROM branches WHERE id = ?1",
                [&fixture.artwork.branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, fixture.artwork.artwork_id);
        assert_eq!(row.1, "Main");
        assert!(row.2.is_none());
    }

    #[test]
    fn initializes_current_cleanup_schema() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM repository_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let cleanup_table: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'pending_file_cleanup')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, super::super::schema::SCHEMA_VERSION);
        assert!(cleanup_table);
    }

    #[test]
    fn artwork_nodes_reject_children() {
        let fixture = Fixture::new();
        let error = create_group_id(&fixture.root, Some(&fixture.artwork.artwork_id), "Invalid")
            .unwrap_err();
        assert!(error.contains("叶节点"), "{error}");
    }

    #[test]
    fn tree_search_includes_breadcrumb_and_source_path() {
        let fixture = Fixture::new();
        let results = search(&fixture.root, "first.psd").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, fixture.artwork.artwork_id);
        assert_eq!(results[0].breadcrumb, "Series / Portrait");
        assert_eq!(
            results[0].source_path.as_deref(),
            Some(display_path(&fixture.first_source.canonicalize().unwrap()).as_str())
        );
        assert_eq!(results[0].ancestor_ids.len(), 1);
    }

    #[test]
    fn moves_multiple_nodes_and_persists_order() {
        let fixture = Fixture::new();
        let alpha = create_group_id(&fixture.root, None, "Alpha").unwrap();
        let beta = create_group_id(&fixture.root, None, "Beta").unwrap();
        let destination = create_group_id(&fixture.root, None, "Destination").unwrap();
        move_nodes(
            &fixture.root,
            MoveLibraryNodesRequest {
                ids: vec![beta.clone(), alpha.clone()],
                parent_id: Some(destination.clone()),
                index: 0,
            },
        )
        .unwrap();

        let tree = list_tree(&fixture.root).unwrap();
        let destination_node = tree
            .nodes
            .iter()
            .find(|node| node.id == destination)
            .unwrap();
        let child_ids = destination_node
            .children
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(child_ids, vec![beta.as_str(), alpha.as_str()]);
    }

    #[test]
    fn moving_parent_and_child_only_moves_parent() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        let destination = create_group_id(&fixture.root, None, "Destination").unwrap();
        move_nodes(
            &fixture.root,
            MoveLibraryNodesRequest {
                ids: vec![parent.clone(), child.clone()],
                parent_id: Some(destination.clone()),
                index: 0,
            },
        )
        .unwrap();
        let tree = list_tree(&fixture.root).unwrap();
        let destination_node = tree
            .nodes
            .iter()
            .find(|node| node.id == destination)
            .unwrap();
        assert_eq!(destination_node.children.len(), 1);
        assert_eq!(destination_node.children[0].id, parent);
        assert_eq!(destination_node.children[0].children[0].id, child);
    }

    #[test]
    fn rejects_move_into_descendant() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        let error = move_nodes(
            &fixture.root,
            MoveLibraryNodesRequest {
                ids: vec![parent],
                parent_id: Some(child),
                index: 0,
            },
        )
        .unwrap_err();
        assert!(error.contains("自身或自身后代"), "{error}");
    }

    #[test]
    fn trash_parent_and_child_is_idempotent_and_normalizes_root() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        trash_nodes(&fixture.root, &[parent.clone(), child]).unwrap();
        let trash = list_trash(&fixture.root).unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, parent);
        assert_eq!(trash[0].descendant_count, 1);
        let connection = open(&fixture.root).unwrap();
        let positions = connection
            .prepare(
                "SELECT position FROM library_nodes
                 WHERE parent_id IS NULL AND trashed_ms IS NULL ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(positions, (0..positions.len() as i64).collect::<Vec<_>>());
    }

    #[test]
    fn restores_trashed_subtree_to_original_parent() {
        let fixture = Fixture::new();
        let parent = create_group_id(&fixture.root, None, "Parent").unwrap();
        let child = create_group_id(&fixture.root, Some(&parent), "Child").unwrap();
        trash_nodes(&fixture.root, std::slice::from_ref(&child)).unwrap();
        assert_eq!(list_trash(&fixture.root).unwrap().len(), 1);
        let restored = restore_trash(&fixture.root, &child).unwrap();
        let parent_node = restored
            .nodes
            .iter()
            .find(|node| node.id == parent)
            .unwrap();
        assert_eq!(parent_node.children[0].id, child);
        assert!(list_trash(&fixture.root).unwrap().is_empty());
    }

    #[test]
    fn permanently_deletes_only_from_trash() {
        let fixture = Fixture::new();
        let group = create_group_id(&fixture.root, None, "Discard").unwrap();
        let error =
            permanently_delete_trash(&fixture.root, std::slice::from_ref(&group)).unwrap_err();
        assert!(error.contains("找不到回收站项目"), "{error}");
        trash_nodes(&fixture.root, std::slice::from_ref(&group)).unwrap();
        permanently_delete_trash(&fixture.root, std::slice::from_ref(&group)).unwrap();
        assert!(list_trash(&fixture.root).unwrap().is_empty());
        let exists: bool = open(&fixture.root)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_nodes WHERE id = ?1)",
                [&group],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists);
    }

    #[test]
    fn permanently_deleted_artwork_removes_its_repository_directory() {
        let fixture = Fixture::new();
        let artwork_directory = fixture
            .root
            .join("artworks")
            .join(&fixture.artwork.artwork_id);
        fs::create_dir_all(&artwork_directory).unwrap();
        fs::write(artwork_directory.join("snapshot.bin"), b"snapshot").unwrap();
        trash_nodes(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        let cleanup_ids = permanently_delete_trash(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();
        assert!(artwork_directory.is_dir());
        let report = crate::cleanup::run(&fixture.root, &cleanup_ids).unwrap();

        assert!(report.failures.is_empty());
        assert!(!artwork_directory.exists());
    }

    #[test]
    fn permanently_deletes_published_artwork_without_foreign_key_failure() {
        let fixture = Fixture::new();
        let connection = open(&fixture.root).unwrap();
        connection
            .execute(
                "INSERT INTO history_nodes
                 (id, artwork_id, created_on_branch_id, parent_id, title, note, commit_kind,
                  is_checkpoint, created_ms, logical_size, chunk_file_size, sha256, chunk_count,
                  snapshot_path, delta_path)
                 VALUES ('published-history', ?1, ?2, NULL, 'Published', '', 'manual',
                         1, 1, 1, 1, ?3, 1, 'snapshots/published.lbc', NULL)",
                params![
                    fixture.artwork.artwork_id,
                    fixture.artwork.branch_id,
                    "A".repeat(64)
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET head_history_id = 'published-history' WHERE id = ?1",
                [&fixture.artwork.branch_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256, media_type,
                  byte_size, created_ms)
                 VALUES ('published-artifact', ?1, 'published-history',
                         'artworks/published.png', ?2, 'image/png', 1, 1)",
                params![fixture.artwork.branch_id, "B".repeat(64)],
            )
            .unwrap();
        drop(connection);
        trash_nodes(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        permanently_delete_trash(
            &fixture.root,
            std::slice::from_ref(&fixture.artwork.artwork_id),
        )
        .unwrap();

        let connection = open(&fixture.root).unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM library_nodes WHERE id = ?1",
                [&fixture.artwork.artwork_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
