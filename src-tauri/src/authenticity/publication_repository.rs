use std::path::Path;

use rusqlite::{params, OptionalExtension, Row};

use crate::{cleanup, storage};

use super::model::FinalArtifact;

pub(crate) struct BranchPublicationTarget {
    pub(crate) history_id: String,
    pub(crate) artifact_id: String,
    pub(crate) artifact_path: String,
    pub(crate) source_sha256: String,
    pub(crate) media_type: String,
}

pub(crate) struct NewFinalArtifact<'a> {
    pub(crate) id: &'a str,
    pub(crate) branch_id: &'a str,
    pub(crate) history_id: &'a str,
    pub(crate) source_path: &'a str,
    pub(crate) source_sha256: &'a str,
    pub(crate) media_type: &'a str,
    pub(crate) byte_size: u64,
    pub(crate) created_ms: i64,
}

pub(crate) fn branch_head(root: &Path, branch_id: &str) -> Result<(String, String), String> {
    let value: Option<(String, Option<String>)> = storage::open(root)?
        .query_row(
            "SELECT artwork_id, head_history_id FROM branches WHERE id = ?1",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (artwork_id, history_id) = value.ok_or("找不到要发布的分支")?;
    Ok((
        artwork_id,
        history_id.ok_or("分支尚无历史节点，不能进入发布状态")?,
    ))
}

pub(crate) fn insert_final_artifact(
    root: &Path,
    artifact: &NewFinalArtifact<'_>,
    cleanup_id: &str,
) -> Result<(), String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let current_head: Option<String> = transaction
        .query_row(
            "SELECT head_history_id FROM branches WHERE id = ?1",
            [artifact.branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .flatten();
    if current_head.as_deref() != Some(artifact.history_id) {
        return Err("分支 head 已变化，请重新选择最终成品".into());
    }
    let inserted = transaction
        .execute(
            "INSERT INTO final_artifacts
             (id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                artifact.id,
                artifact.branch_id,
                artifact.history_id,
                artifact.source_path,
                artifact.source_sha256,
                artifact.media_type,
                artifact.byte_size,
                artifact.created_ms,
            ],
        )
        .map_err(|error| format!("无法进入发布状态：{error}"))?;
    if inserted != 1 {
        return Err("未能绑定最终成品".into());
    }
    cleanup::complete(&transaction, &[cleanup_id.to_owned()])?;
    transaction.commit().map_err(storage::database_error)
}

pub(crate) fn publication_target(
    root: &Path,
    branch_id: &str,
) -> Result<BranchPublicationTarget, String> {
    let mut target = storage::open(root)?
        .query_row(
            "SELECT f.history_id, f.id, f.source_path, f.source_sha256, f.media_type
             FROM branches b
             JOIN library_nodes artwork ON artwork.id = b.artwork_id
             JOIN final_artifacts f ON f.branch_id = b.id
             WHERE b.id = ?1 AND artwork.trashed_ms IS NULL",
            [branch_id],
            |row| {
                Ok(BranchPublicationTarget {
                    history_id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    artifact_path: row.get(2)?,
                    source_sha256: row.get(3)?,
                    media_type: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "分支尚未进入发布状态".to_owned())?;
    target.artifact_path =
        storage::display_path(&storage::resolve_path(root, &target.artifact_path)?);
    storage::verify_file_sha256(
        Path::new(&target.artifact_path),
        &target.source_sha256,
        "仓库最终成品",
    )?;
    Ok(target)
}

pub(crate) fn find_artifact(root: &Path, branch_id: &str) -> Result<Option<FinalArtifact>, String> {
    let mut artifact = storage::open(root)?
        .query_row(
            "SELECT id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms
             FROM final_artifacts WHERE branch_id = ?1",
            [branch_id],
            final_artifact_from_row,
        )
        .optional()
        .map_err(storage::database_error)?;
    if let Some(value) = artifact.as_mut() {
        value.source_path =
            storage::display_path(&storage::resolve_path(root, &value.source_path)?);
    }
    Ok(artifact)
}

pub(crate) fn remove_artifact(root: &Path, branch_id: &str) -> Result<Vec<String>, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let path: Option<String> = transaction
        .query_row(
            "SELECT source_path FROM final_artifacts WHERE branch_id = ?1",
            [branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?;
    let mut stored_statement = transaction
        .prepare(
            "SELECT stored_path
             FROM certification_records WHERE branch_id = ?1",
        )
        .map_err(storage::database_error)?;
    let stored_paths = stored_statement
        .query_map([branch_id], |row| row.get::<_, String>(0))
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(stored_statement);
    let mut cleanup_ids = Vec::new();
    if let Some(relative) = path.as_deref() {
        cleanup_ids.push(cleanup::enqueue_repository_file(
            &transaction,
            relative,
            "cancel_branch_publication",
        )?);
    }
    for stored_path in &stored_paths {
        cleanup_ids.push(cleanup::enqueue_repository_file(
            &transaction,
            stored_path,
            "cancel_branch_publication",
        )?);
    }
    transaction
        .execute(
            "DELETE FROM certification_configs WHERE branch_id = ?1",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    transaction
        .execute(
            "DELETE FROM final_artifacts WHERE branch_id = ?1",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(cleanup_ids)
}

fn final_artifact_from_row(row: &Row<'_>) -> rusqlite::Result<FinalArtifact> {
    Ok(FinalArtifact {
        id: row.get(0)?,
        branch_id: row.get(1)?,
        history_id: row.get(2)?,
        source_path: row.get(3)?,
        source_sha256: row.get(4)?,
        media_type: row.get(5)?,
        byte_size: row.get(6)?,
        created_ms: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publication_fixture(root: &Path) {
        crate::library::initialize(root).unwrap();
        storage::open(root)
            .unwrap()
            .execute_batch(
                "INSERT INTO library_nodes
                   (id, kind, title, position, created_ms, updated_ms)
                 VALUES ('artwork', 'artwork', 'Artwork', 0, 0, 0);
                 INSERT INTO artworks (id, description, created_ms, updated_ms)
                 VALUES ('artwork', '', 0, 0);
                 INSERT INTO branches
                   (id, artwork_id, title, source_path, source_path_key,
                    backup_enabled, backup_interval_minutes, created_ms, updated_ms)
                 VALUES ('branch', 'artwork', 'Main', 'source.psd', 'source.psd', 1, 5, 0, 0);
                 INSERT INTO history_nodes
                   (id, artwork_id, created_on_branch_id, title, note, commit_kind,
                    is_checkpoint, created_ms, logical_size, chunk_file_size, sha256,
                    chunk_count, snapshot_path)
                 VALUES ('history', 'artwork', 'branch', 'History', '', 'manual',
                         1, 0, 1, 1,
                         '0000000000000000000000000000000000000000000000000000000000000000',
                         1, 'artworks/snapshot.chunk');
                 UPDATE branches SET head_history_id = 'history' WHERE id = 'branch';",
            )
            .unwrap();
    }

    fn artifact<'a>(source_path: &'a str) -> NewFinalArtifact<'a> {
        NewFinalArtifact {
            id: "artifact",
            branch_id: "branch",
            history_id: "history",
            source_path,
            source_sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            media_type: "image/jpeg",
            byte_size: 1,
            created_ms: 0,
        }
    }

    #[test]
    fn final_artifact_atomically_takes_over_cleanup_intent() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        publication_fixture(&root);
        let mut connection = storage::open(&root).unwrap();
        let transaction = connection.transaction().unwrap();
        let cleanup_id = cleanup::enqueue_repository_file_with_hash(
            &transaction,
            "artworks/artifact.jpg",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "test",
        )
        .unwrap();
        transaction.commit().unwrap();

        insert_final_artifact(&root, &artifact("artworks/artifact.jpg"), &cleanup_id).unwrap();

        let connection = storage::open(&root).unwrap();
        let values: (i64, i64) = connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM final_artifacts),
                   (SELECT COUNT(*) FROM pending_file_cleanup)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(values, (1, 0));
    }

    #[test]
    fn missing_cleanup_intent_rolls_back_final_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        publication_fixture(&root);

        let error = insert_final_artifact(
            &root,
            &artifact("artworks/artifact.jpg"),
            "missing-cleanup-intent",
        )
        .unwrap_err();

        let count: i64 = storage::open(&root)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM final_artifacts", [], |row| row.get(0))
            .unwrap();
        assert!(error.contains("待清理文件登记已丢失"), "{error}");
        assert_eq!(count, 0);
    }

    #[test]
    fn cancel_publication_keeps_external_outputs_and_resets_config() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        publication_fixture(&root);
        let connection = storage::open(&root).unwrap();
        connection
            .execute_batch(
                "INSERT INTO final_artifacts
                   (id, branch_id, history_id, source_path, source_sha256,
                    media_type, byte_size, created_ms)
                 VALUES
                   ('artifact', 'branch', 'history', 'artworks/final.jpg',
                    '0000000000000000000000000000000000000000000000000000000000000000',
                    'image/jpeg', 1, 0);
                 INSERT INTO certification_configs (branch_id, updated_ms)
                 VALUES ('branch', 0);
                 INSERT INTO certification_records
                   (id, final_artifact_id, branch_id, history_id, watermark_id,
                    trustmark_enabled, output_path, stored_path, output_sha256,
                    output_bytes, title, creator, rights_statement,
                    authentication_content, regions_json, created_ms)
                 VALUES
                   ('record', 'artifact', 'branch', 'history', NULL, 0,
                    'C:/published/output.jpg', 'artworks/certified.jpg',
                    '0000000000000000000000000000000000000000000000000000000000000000',
                    1, 'Title', '', '', '', '[]', 0);",
            )
            .unwrap();

        let cleanup_ids = remove_artifact(&root, "branch").unwrap();

        let connection = storage::open(&root).unwrap();
        let counts: (i64, i64, i64) = connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM final_artifacts),
                   (SELECT COUNT(*) FROM certification_records),
                   (SELECT COUNT(*) FROM certification_configs)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let mut statement = connection
            .prepare(
                "SELECT path_kind, path FROM pending_file_cleanup
                 ORDER BY path",
            )
            .unwrap();
        let cleanup_entries = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(counts, (0, 0, 0));
        assert_eq!(cleanup_ids.len(), 2);
        assert_eq!(
            cleanup_entries,
            vec![
                ("repository_file".into(), "artworks/certified.jpg".into()),
                ("repository_file".into(), "artworks/final.jpg".into()),
            ]
        );
    }
}
