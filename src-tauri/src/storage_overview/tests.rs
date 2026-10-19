use std::{fs, path::PathBuf, time::Instant};

use rusqlite::params;

use super::*;
use crate::{library, storage};

fn repository() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repository");
    library::initialize(&root).unwrap();
    (directory, root)
}

fn insert_group(connection: &Connection, id: &str, title: &str) {
    connection
        .execute(
            "INSERT INTO library_nodes
               (id, parent_id, kind, title, position, created_ms, updated_ms)
             VALUES (?1, NULL, 'group', ?2, 0, 0, 0)",
            params![id, title],
        )
        .unwrap();
}

fn insert_artwork(connection: &Connection, id: &str, parent_id: Option<&str>, title: &str) {
    connection
        .execute(
            "INSERT INTO library_nodes
               (id, parent_id, kind, title, position, created_ms, updated_ms)
             VALUES (?1, ?2, 'artwork', ?3, 0, 0, 0)",
            params![id, parent_id, title],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO artworks (id, description, created_ms, updated_ms)
             VALUES (?1, '', 0, 0)",
            [id],
        )
        .unwrap();
}

fn insert_trashed_artwork(connection: &Connection, id: &str, title: &str) {
    connection
        .execute(
            "INSERT INTO library_nodes
               (id, parent_id, kind, title, position, created_ms, updated_ms, trashed_ms)
             VALUES (?1, NULL, 'artwork', ?2, 0, 0, 0, 1)",
            params![id, title],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO artworks (id, description, created_ms, updated_ms)
             VALUES (?1, '', 0, 0)",
            [id],
        )
        .unwrap();
}

fn insert_branch(connection: &Connection, id: &str, artwork_id: &str, title: &str) {
    connection
        .execute(
            "INSERT INTO branches
               (id, artwork_id, title, source_path, source_path_key,
                backup_enabled, backup_interval_minutes, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, '', ?1, 1, 5, 0, 0)",
            params![id, artwork_id, title],
        )
        .unwrap();
}

struct HistoryFixture<'a> {
    id: &'a str,
    artwork_id: &'a str,
    branch_id: &'a str,
    parent_id: Option<&'a str>,
    title: &'a str,
    checkpoint: bool,
    created_ms: i64,
    logical_size: i64,
    chunk_file_size: i64,
    chunk_count: i64,
    snapshot_path: Option<&'a str>,
    delta_path: Option<&'a str>,
}

fn insert_history(connection: &Connection, fixture: HistoryFixture<'_>) {
    connection
        .execute(
            "INSERT INTO history_nodes
               (id, artwork_id, created_on_branch_id, parent_id, title, note,
                commit_kind, is_checkpoint, created_ms, logical_size,
                chunk_file_size, sha256, chunk_count, snapshot_path, delta_path)
             VALUES (?1, ?2, ?3, ?4, ?5, '', 'manual', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                fixture.id,
                fixture.artwork_id,
                fixture.branch_id,
                fixture.parent_id,
                fixture.title,
                fixture.checkpoint as i64,
                fixture.created_ms,
                fixture.logical_size,
                fixture.chunk_file_size,
                "0".repeat(64),
                fixture.chunk_count,
                fixture.snapshot_path,
                fixture.delta_path,
            ],
        )
        .unwrap();
}

fn insert_edge(connection: &Connection, child: &str, parent: &str, path: &str, size: i64) {
    connection
        .execute(
            "INSERT INTO history_edges
               (child_history_id, parent_history_id, delta_path, delta_size)
             VALUES (?1, ?2, ?3, ?4)",
            params![child, parent, path, size],
        )
        .unwrap();
}

fn insert_final_artifact(
    connection: &Connection,
    id: &str,
    branch_id: &str,
    history_id: &str,
    path: &str,
    size: i64,
    created_ms: i64,
) {
    connection
        .execute(
            "INSERT INTO final_artifacts
               (id, branch_id, history_id, source_path, source_sha256,
                media_type, byte_size, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'image/png', ?6, ?7)",
            params![
                id,
                branch_id,
                history_id,
                path,
                "0".repeat(64),
                size,
                created_ms
            ],
        )
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn insert_certification(
    connection: &Connection,
    id: &str,
    artifact_id: &str,
    branch_id: &str,
    history_id: &str,
    output_path: &str,
    stored_path: &str,
    bytes: i64,
    created_ms: i64,
) {
    connection
        .execute(
            "INSERT INTO certification_records
               (id, final_artifact_id, branch_id, history_id, trustmark_enabled,
                output_path, stored_path, output_sha256, output_bytes, title,
                creator, rights_statement, authentication_content, regions_json,
                created_ms)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, 'record', '', '', '', '[]', ?9)",
            params![
                id,
                artifact_id,
                branch_id,
                history_id,
                output_path,
                stored_path,
                "0".repeat(64),
                bytes,
                created_ms,
            ],
        )
        .unwrap();
}

fn insert_board(connection: &Connection, id: i64, artwork_id: &str, name: &str) {
    connection
        .execute(
            "INSERT INTO pin_boards
               (id, artwork_id, name, sort_order, now_step, max_step, revision,
                deleted_at, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, 0, 0, 0, ?4, NULL, 0, 0)",
            params![id, artwork_id, name, "0".repeat(64)],
        )
        .unwrap();
}

fn insert_board_image(
    connection: &Connection,
    id: i64,
    board_id: i64,
    file_path: &str,
    width: i64,
    height: i64,
) {
    connection
        .execute_batch(&format!(
            "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
               VALUES ({id}, {board_id}, 0, 0);
             INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
               VALUES ({id}, '{file_path}', {width}, {height});"
        ))
        .unwrap();
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn empty_repository_reports_zero_totals() {
    let (_directory, root) = repository();

    let overview = overview(&root).unwrap();

    assert_eq!(overview.database_recorded_bytes, 0);
    assert!(overview.artworks.is_empty());
    assert!(overview.unassigned.is_empty());
    assert_eq!(overview.categories.len(), 4);
    assert!(overview
        .categories
        .iter()
        .all(|category| category.bytes == 0));
    assert_eq!(overview.compression.snapshot.ratio, None);
    assert_eq!(overview.compression.delta.ratio, None);
}

#[test]
fn aggregates_history_without_counting_rebuild_reads() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_group(&connection, "group", "Group");
    insert_artwork(&connection, "a", Some("group"), "Artwork");
    insert_branch(&connection, "b", "a", "Main");
    insert_history(
        &connection,
        HistoryFixture {
            id: "root",
            artwork_id: "a",
            branch_id: "b",
            parent_id: None,
            title: "Root",
            checkpoint: true,
            created_ms: 1,
            logical_size: 1000,
            chunk_file_size: 400,
            chunk_count: 10,
            snapshot_path: Some("artworks/a/snapshots/root.lbc"),
            delta_path: None,
        },
    );
    insert_history(
        &connection,
        HistoryFixture {
            id: "child",
            artwork_id: "a",
            branch_id: "b",
            parent_id: Some("root"),
            title: "Child",
            checkpoint: false,
            created_ms: 2,
            logical_size: 1200,
            chunk_file_size: 50,
            chunk_count: 3,
            snapshot_path: None,
            delta_path: Some("artworks/a/deltas/child-to-root.lbd"),
        },
    );
    insert_edge(
        &connection,
        "child",
        "root",
        "artworks/a/deltas/child-to-root.lbd",
        60,
    );

    let overview = overview(&root).unwrap();

    assert_eq!(overview.database_recorded_bytes, 460);
    let category = |kind: StorageKind| {
        overview
            .categories
            .iter()
            .find(|category| category.kind == kind)
            .unwrap()
    };
    assert_eq!(category(StorageKind::Snapshot).bytes, 400);
    assert_eq!(category(StorageKind::Delta).bytes, 60);
    assert_eq!(category(StorageKind::FinalArtifact).bytes, 0);
    assert_eq!(category(StorageKind::Certification).bytes, 0);
    assert_eq!(overview.compression.snapshot.logical_bytes, 1000);
    assert_eq!(overview.compression.snapshot.stored_bytes, 400);
    assert_eq!(overview.compression.snapshot.ratio, Some(2.5));
    assert_eq!(overview.compression.delta.logical_bytes, 1000);
    assert_eq!(overview.compression.delta.stored_bytes, 60);
    assert!((overview.compression.delta.ratio.unwrap() - 1000.0 / 60.0).abs() < 1e-9);

    let artwork = &overview.artworks[0];
    assert_eq!(artwork.title, "Artwork");
    assert_eq!(artwork.group_path, vec!["Group"]);
    assert_eq!(artwork.recorded_bytes, 460);
    assert_eq!(artwork.snapshot_bytes, 400);
    assert_eq!(artwork.delta_bytes, 60);
    assert_eq!(artwork.node_count, 2);
    assert_eq!(artwork.branch_count, 1);
    assert_eq!(artwork.branches[0].recorded_bytes, 460);
    assert_eq!(artwork.branches[0].nodes.len(), 2);
    let child = artwork.branches[0]
        .nodes
        .iter()
        .find(|node| node.id == "child")
        .unwrap();
    assert!(!child.has_snapshot);
    assert_eq!(child.chunk_file_size, 50);
    assert_eq!(child.compression_ratio, Some(24.0));
    assert_eq!(
        overview
            .artworks
            .iter()
            .map(|artwork| artwork.recorded_bytes)
            .sum::<u64>(),
        460
    );
    assert!(overview.unassigned.is_empty());
}

#[test]
fn overview_updates_after_history_changes() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_artwork(&connection, "a", None, "Artwork");
    insert_branch(&connection, "b", "a", "Main");
    insert_history(
        &connection,
        HistoryFixture {
            id: "root",
            artwork_id: "a",
            branch_id: "b",
            parent_id: None,
            title: "Root",
            checkpoint: true,
            created_ms: 1,
            logical_size: 1000,
            chunk_file_size: 400,
            chunk_count: 1,
            snapshot_path: Some("artworks/a/snapshots/root.lbc"),
            delta_path: None,
        },
    );
    insert_history(
        &connection,
        HistoryFixture {
            id: "child",
            artwork_id: "a",
            branch_id: "b",
            parent_id: Some("root"),
            title: "Child",
            checkpoint: false,
            created_ms: 2,
            logical_size: 1200,
            chunk_file_size: 50,
            chunk_count: 1,
            snapshot_path: None,
            delta_path: Some("artworks/a/deltas/child-to-root.lbd"),
        },
    );
    insert_edge(
        &connection,
        "child",
        "root",
        "artworks/a/deltas/child-to-root.lbd",
        60,
    );
    assert_eq!(overview(&root).unwrap().database_recorded_bytes, 460);

    connection
        .execute("DELETE FROM history_nodes WHERE id = 'child'", [])
        .unwrap();

    let after = overview(&root).unwrap();
    assert_eq!(after.database_recorded_bytes, 400);
    assert_eq!(
        after
            .categories
            .iter()
            .find(|category| category.kind == StorageKind::Snapshot)
            .unwrap()
            .bytes,
        400
    );
    assert_eq!(after.artworks[0].node_count, 1);
    assert_eq!(after.artworks[0].delta_bytes, 0);
}

#[test]
fn dedupes_artifact_and_certification_paths_and_attributes_them_to_artworks() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_artwork(&connection, "a", None, "Artwork A");
    insert_artwork(&connection, "b", None, "Artwork B");
    insert_branch(&connection, "ba", "a", "A Main");
    insert_branch(&connection, "bb", "b", "B Main");
    insert_branch(&connection, "b2", "b", "B Second");
    for (id, artwork_id, branch_id) in [("ha", "a", "ba"), ("hb", "b", "bb"), ("hb2", "b", "b2")] {
        let snapshot_path = format!("artworks/{artwork_id}/snapshots/{id}.lbc");
        insert_history(
            &connection,
            HistoryFixture {
                id,
                artwork_id,
                branch_id,
                parent_id: None,
                title: id,
                checkpoint: true,
                created_ms: 1,
                logical_size: 100,
                chunk_file_size: 50,
                chunk_count: 1,
                snapshot_path: Some(&snapshot_path),
                delta_path: None,
            },
        );
    }
    insert_final_artifact(
        &connection,
        "fa",
        "ba",
        "ha",
        "artworks/a/artifacts/ba/a.png",
        1000,
        1,
    );
    insert_final_artifact(
        &connection,
        "fb",
        "bb",
        "hb",
        "artworks\\a\\artifacts\\ba\\a.png",
        300,
        2,
    );
    insert_final_artifact(
        &connection,
        "fb2",
        "b2",
        "hb2",
        "artworks/b/artifacts/b2/b.png",
        500,
        3,
    );
    insert_certification(
        &connection,
        "r1",
        "fa",
        "ba",
        "ha",
        "D:\\exports\\first.jpg",
        "artworks/a/certifications/r1.jpg",
        200,
        1,
    );
    insert_certification(
        &connection,
        "r2",
        "fb",
        "bb",
        "hb",
        "d:/exports/first.jpg",
        "artworks/b/certifications/r2.jpg",
        300,
        2,
    );
    insert_certification(
        &connection,
        "r3",
        "fb",
        "bb",
        "hb",
        "D:\\exports\\second.jpg",
        "artworks/b/certifications/r3.jpg",
        70,
        3,
    );

    let overview = overview(&root).unwrap();

    let category = |kind: StorageKind| {
        overview
            .categories
            .iter()
            .find(|category| category.kind == kind)
            .unwrap()
    };
    assert_eq!(category(StorageKind::FinalArtifact).bytes, 1500);
    assert_eq!(category(StorageKind::FinalArtifact).file_count, 2);
    assert_eq!(category(StorageKind::Certification).bytes, 270);
    assert_eq!(category(StorageKind::Certification).file_count, 2);
    let artwork = |id: &str| {
        overview
            .artworks
            .iter()
            .find(|artwork| artwork.artwork_id == id)
            .unwrap()
    };
    assert_eq!(artwork("a").final_artifact_bytes, 1000);
    assert_eq!(artwork("a").certification_bytes, 200);
    assert_eq!(artwork("b").final_artifact_bytes, 500);
    assert_eq!(artwork("b").certification_bytes, 70);
    assert_eq!(
        overview
            .artworks
            .iter()
            .map(|artwork| artwork.recorded_bytes)
            .sum::<u64>()
            + overview
                .unassigned
                .iter()
                .map(|entry| entry.bytes)
                .sum::<u64>(),
        overview.database_recorded_bytes
    );
}

#[test]
fn includes_trashed_artworks_and_ignores_empty_board_file_paths() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_trashed_artwork(&connection, "a", "Trashed artwork");
    insert_board(&connection, 1, "a", "Board");
    insert_board_image(&connection, 7, 1, "7.dds", 4, 4);
    insert_board_image(&connection, 8, 1, "", 4, 4);

    let overview = overview(&root).unwrap();

    assert_eq!(overview.artworks.len(), 1);
    assert!(overview.artworks[0].trashed);
    assert_eq!(overview.artworks[0].boards.len(), 1);
    assert_eq!(overview.artworks[0].boards[0].image_count, 1);
}

#[test]
fn disk_scan_classifies_repository_files_and_excludes_runtime_artifacts() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_artwork(&connection, "a", None, "Artwork");
    insert_branch(&connection, "b", "a", "Main");
    insert_history(
        &connection,
        HistoryFixture {
            id: "root",
            artwork_id: "a",
            branch_id: "b",
            parent_id: None,
            title: "Root",
            checkpoint: true,
            created_ms: 1,
            logical_size: 400,
            chunk_file_size: 400,
            chunk_count: 1,
            snapshot_path: Some("artworks/a/snapshots/root.lbc"),
            delta_path: None,
        },
    );
    insert_history(
        &connection,
        HistoryFixture {
            id: "child",
            artwork_id: "a",
            branch_id: "b",
            parent_id: Some("root"),
            title: "Child",
            checkpoint: false,
            created_ms: 2,
            logical_size: 400,
            chunk_file_size: 60,
            chunk_count: 1,
            snapshot_path: None,
            delta_path: Some("artworks/a/deltas/child-to-root.lbd"),
        },
    );
    insert_edge(
        &connection,
        "child",
        "root",
        "artworks/a/deltas/child-to-root.lbd",
        60,
    );
    insert_final_artifact(
        &connection,
        "fa",
        "b",
        "root",
        "artworks/a/artifacts/b/final.png",
        300,
        1,
    );
    insert_certification(
        &connection,
        "r1",
        "fa",
        "b",
        "root",
        "D:\\exports\\signed.jpg",
        "artworks/a/certifications/r1.jpg",
        200,
        1,
    );
    insert_board(&connection, 1, "a", "Board");
    insert_board_image(&connection, 7, 1, "7.dds", 4, 4);
    drop(connection);

    write_file(&root, "artworks/a/snapshots/root.lbc", &vec![1; 400]);
    write_file(&root, "artworks/a/deltas/child-to-root.lbd", &vec![2; 60]);
    write_file(&root, "artworks/a/artifacts/b/final.png", &vec![3; 300]);
    write_file(&root, "artworks/a/certifications/r1.jpg", &vec![4; 200]);
    write_file(&root, "artworks/a/boards/1/7.dds", &vec![5; 128]);
    write_file(&root, "temp/session/blob.bin", &vec![6; 10]);
    // 作品 `temp/` 是备份/恢复的暂存区，崩溃残留既不属受管理文件也不该算「未识别」。
    write_file(&root, "artworks/a/temp/.tmp9f3Kq1", &vec![9; 20]);
    write_file(&root, "notes.txt", b"hello");
    write_file(&root, "legacy.sqlite3", b"old");
    write_file(&root, ".lilith-write-owner.json", &vec![7; 99]);
    write_file(&root, ".lilith-write-lock.sqlite3", &vec![8; 99]);

    let scan = scan_disk(&root, || false, |_, _| {}).unwrap();

    let category = |kind: StorageKind| {
        scan.categories
            .iter()
            .find(|category| category.kind == kind)
            .unwrap()
    };
    assert_eq!(category(StorageKind::Snapshot).bytes, 400);
    assert_eq!(category(StorageKind::Delta).bytes, 60);
    assert_eq!(category(StorageKind::FinalArtifact).bytes, 300);
    assert_eq!(category(StorageKind::Certification).bytes, 200);
    assert_eq!(category(StorageKind::BoardDds).bytes, 128);
    assert_eq!(scan.other_breakdown.temp_bytes, 30);
    assert_eq!(scan.other_breakdown.temp_files, 2);
    assert_eq!(scan.other_breakdown.unrecognized_bytes, 5);
    assert!(scan.other_breakdown.sqlite_bytes >= 3);
    assert_eq!(
        scan.categories
            .iter()
            .map(|category| category.bytes)
            .sum::<u64>(),
        scan.total_bytes
    );
    assert_eq!(scan.artwork_bytes.len(), 1);
    assert_eq!(scan.artwork_bytes[0].artwork_id, "a");
    assert_eq!(scan.artwork_bytes[0].bytes, 400 + 60 + 300 + 200 + 128);
    assert_eq!(scan.dds_by_board.len(), 1);
    assert_eq!(scan.dds_by_board[0].board_id, 1);
    assert_eq!(scan.dds_by_board[0].bytes, 128);
    assert!(scan.missing_files.is_empty());
}

#[test]
fn disk_scan_reports_missing_managed_files() {
    let (_directory, root) = repository();
    let connection = storage::open(&root).unwrap();
    insert_artwork(&connection, "a", None, "Artwork");
    insert_branch(&connection, "b", "a", "Main");
    insert_history(
        &connection,
        HistoryFixture {
            id: "root",
            artwork_id: "a",
            branch_id: "b",
            parent_id: None,
            title: "Root",
            checkpoint: true,
            created_ms: 1,
            logical_size: 400,
            chunk_file_size: 400,
            chunk_count: 1,
            snapshot_path: Some("artworks/a/snapshots/root.lbc"),
            delta_path: None,
        },
    );
    drop(connection);

    let scan = scan_disk(&root, || false, |_, _| {}).unwrap();

    assert_eq!(
        scan.missing_files,
        vec![StorageMissingFile {
            kind: StorageKind::Snapshot,
            count: 1,
        }]
    );
    assert_eq!(scan.total_bytes, scan.other_breakdown.sqlite_bytes);
}

#[test]
fn disk_scan_is_cancellable() {
    let (_directory, root) = repository();

    let error = scan_disk(&root, || true, |_, _| {}).unwrap_err();

    assert!(error.contains("取消"), "{error}");
}

#[test]
fn large_repository_aggregation_is_bounded() {
    let (_directory, root) = repository();
    let mut connection = storage::open(&root).unwrap();
    {
        let transaction = connection.transaction().unwrap();
        for artwork_index in 0..30 {
            let artwork_id = format!("artwork-{artwork_index}");
            insert_artwork(
                &transaction,
                &artwork_id,
                None,
                &format!("Artwork {artwork_index}"),
            );
            let branch_id = format!("branch-{artwork_index}");
            insert_branch(&transaction, &branch_id, &artwork_id, "Main");
            let mut parent = None;
            for node_index in 0..12 {
                let node_id = format!("node-{artwork_index}-{node_index}");
                let snapshot = (node_index == 0)
                    .then(|| format!("artworks/{artwork_id}/snapshots/{node_id}.lbc"));
                let delta = (node_index > 0)
                    .then(|| format!("artworks/{artwork_id}/deltas/{node_id}-to-parent.lbd"));
                insert_history(
                    &transaction,
                    HistoryFixture {
                        id: &node_id,
                        artwork_id: &artwork_id,
                        branch_id: &branch_id,
                        parent_id: parent.as_deref(),
                        title: &node_id,
                        checkpoint: node_index == 0,
                        created_ms: node_index,
                        logical_size: 1000,
                        chunk_file_size: if node_index == 0 { 500 } else { 30 },
                        chunk_count: 1,
                        snapshot_path: snapshot.as_deref(),
                        delta_path: delta.as_deref(),
                    },
                );
                if let (Some(parent_id), Some(delta_path)) = (parent.as_deref(), delta.as_deref()) {
                    insert_edge(&transaction, &node_id, parent_id, delta_path, 20);
                }
                parent = Some(node_id);
            }
        }
        transaction.commit().unwrap();
    }
    reset_query_count();
    let started = Instant::now();

    let overview = overview_from_connection(&connection).unwrap();

    let elapsed = started.elapsed();
    let queries = query_count();
    eprintln!(
        "storage overview: {} artworks / {} nodes, {queries} queries, {elapsed:?}",
        overview.artworks.len(),
        overview
            .artworks
            .iter()
            .map(|artwork| artwork.node_count)
            .sum::<u64>()
    );
    assert_eq!(overview.artworks.len(), 30);
    assert_eq!(overview.database_recorded_bytes, 30 * (500 + 11 * 20));
    assert!(queries <= 10, "聚合查询次数为 {queries}，超出预期上限");
    assert!(
        elapsed.as_secs() < 10,
        "聚合耗时 {elapsed:?}，超过灾难性上界"
    );
}
