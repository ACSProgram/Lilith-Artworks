use std::path::Path;

use rusqlite::{params, OptionalExtension, Row};

use crate::storage;

use super::model::FinalArtifact;

pub(crate) struct BranchPublicationTarget {
    pub(crate) history_id: String,
    pub(crate) artifact_id: String,
    pub(crate) artifact_path: String,
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
    transaction.commit().map_err(storage::database_error)
}

pub(crate) fn publication_target(
    root: &Path,
    branch_id: &str,
) -> Result<BranchPublicationTarget, String> {
    let mut target = storage::open(root)?
        .query_row(
            "SELECT f.history_id, f.id, f.source_path
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
                })
            },
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "分支尚未进入发布状态".to_owned())?;
    target.artifact_path =
        storage::display_path(&storage::resolve_path(root, &target.artifact_path)?);
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

pub(crate) fn remove_artifact(root: &Path, branch_id: &str) -> Result<(), String> {
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
    let mut output_statement = transaction
        .prepare("SELECT output_path, stored_path FROM certification_records WHERE branch_id = ?1")
        .map_err(storage::database_error)?;
    let outputs = output_statement
        .query_map([branch_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(output_statement);
    transaction
        .execute(
            "DELETE FROM final_artifacts WHERE branch_id = ?1",
            [branch_id],
        )
        .map_err(storage::database_error)?;
    transaction.commit().map_err(storage::database_error)?;
    if let Some(relative) = path {
        let absolute = storage::resolve_path(root, &relative)?;
        let _ = std::fs::remove_file(absolute);
    }
    for (output, stored) in outputs {
        let _ = std::fs::remove_file(output);
        if let Some(relative) = stored {
            if let Ok(absolute) = storage::resolve_path(root, &relative) {
                let _ = std::fs::remove_file(absolute);
            }
        }
    }
    Ok(())
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
