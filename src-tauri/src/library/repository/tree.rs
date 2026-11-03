//! 作品树读取、搜索与树结构写事务。
//!
//! 这里维护父子关系与同级顺序；回收站状态列由 `trash` 写入，本模块的查询统一
//! 过滤 `trashed_ms IS NULL`。

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::storage::{database_error, new_id, normalize_source_path, now_ms, validate_title};

use super::super::model::{
    ArtworkSummary, CreatedArtwork, LibraryNode, LibrarySearchResult, LibraryTree,
    MoveLibraryNodesRequest, PrimaryBranch,
};
use super::open;

const MAX_MOVE_NODES: usize = 512;
const MAX_SEARCH_CHARS: usize = 160;
const MAX_SEARCH_RESULTS: usize = 100;

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
    backup_quick_enabled: bool,
) -> Result<CreatedArtwork, String> {
    validate_title(title, "Artwork 标题")?;
    validate_title(branch_title, "分支标题")?;
    // 允许不选择工作文件创建 Artwork（source_path 为空）：自动备份与主动提交
    // 对该分支不可用（调度查询会跳过空工作文件），但素材板等仓库功能不受影响。
    let (source_display, source_key) = if source_path.as_os_str().is_empty() {
        (String::new(), String::new())
    } else {
        normalize_source_path(root, source_path)?
    };
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
             (id, artwork_id, title, source_path, source_path_key, backup_quick_enabled,
              created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                branch_id,
                artwork_id,
                branch_title.trim(),
                source_display,
                source_key,
                i64::from(backup_quick_enabled),
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
    backup_quick_enabled: bool,
) -> Result<LibraryTree, String> {
    create_artwork(
        root,
        parent_id,
        title,
        branch_title,
        source_path,
        backup_quick_enabled,
    )?;
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

pub(super) fn create_group_id(
    root: &Path,
    parent_id: Option<&str>,
    title: &str,
) -> Result<String, String> {
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
                    (SELECT COUNT(*) FROM branches counted WHERE counted.artwork_id = b.artwork_id),
                    (SELECT COUNT(*) FROM branches notices WHERE notices.artwork_id = b.artwork_id
                       AND notices.backup_disable_notice_pending <> 0)
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
                row.get::<_, u64>(5)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let branch_by_artwork = branch_rows
        .into_iter()
        .map(|(artwork_id, branch, count, notice_count)| {
            (artwork_id, (branch, count, notice_count))
        })
        .collect::<HashMap<_, _>>();

    let mut statement = connection
        .prepare(
            "SELECT n.id, n.parent_id, n.kind, n.title, n.position, n.updated_ms
             FROM library_nodes n
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
                branch_count: primary.map_or(0, |(_, count, _)| *count),
                backup_disable_notice_count: primary.map_or(0, |(_, _, count)| *count),
                primary_branch: primary.map(|(branch, _, _)| branch.clone()),
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

pub(super) fn validate_node_ids(ids: &[String]) -> Result<Vec<String>, String> {
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

pub(super) fn load_parent_map(
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

pub(super) fn ensure_nodes_exist(
    ids: &[String],
    parents: &HashMap<String, Option<String>>,
) -> Result<(), String> {
    if let Some(id) = ids.iter().find(|id| !parents.contains_key(*id)) {
        return Err(format!("找不到节点：{id}"));
    }
    Ok(())
}

pub(super) fn top_level_selection(
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

pub(super) fn sibling_ids(
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

pub(super) fn update_sibling_positions(
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

pub(super) fn normalize_siblings(
    transaction: &Transaction<'_>,
    parent_id: Option<&str>,
) -> Result<(), String> {
    let siblings = sibling_ids(transaction, parent_id)?;
    update_sibling_positions(transaction, &siblings)
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
    use super::super::{open, test_support::Fixture};
    use super::*;

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
            Some(
                crate::platform::path::display_path(&fixture.first_source.canonicalize().unwrap())
                    .as_str()
            )
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
    fn creates_artwork_without_a_source_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();

        let created = create_artwork(&root, None, "Artwork", "Main", Path::new(""), false).unwrap();
        let branch = crate::history::list(&root, &created.artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.source_path, "");

        // 空工作文件分支不进入自动备份调度，且同一仓库可创建多个无源 Artwork。
        let scheduled = crate::history::list_scheduled(&root).unwrap();
        assert!(scheduled
            .iter()
            .all(|branch| branch.id != created.branch_id));
        create_artwork(&root, None, "Second", "Main", Path::new(""), false).unwrap();
    }

    #[test]
    fn create_artwork_still_requires_an_existing_source_file_when_given() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();

        let error = create_artwork(
            &root,
            None,
            "Artwork",
            "Main",
            Path::new("missing.psd"),
            false,
        )
        .unwrap_err();
        assert!(error.contains("分支工作文件"), "{error}");
    }
}
