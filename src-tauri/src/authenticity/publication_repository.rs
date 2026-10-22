use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension, Row};

use crate::{cleanup, platform::path::display_path, storage};

use super::model::FinalArtifact;

/// 最终成品的唯一存放目录：`artworks/<artwork>/artifacts/<branch>/`。
///
/// 最终成品与认证副本的路径布局只在这里拼接，`artifact` 与 `repository` 共用。
pub(crate) fn final_artifact_directory(root: &Path, artwork_id: &str, branch_id: &str) -> PathBuf {
    root.join("artworks")
        .join(artwork_id)
        .join("artifacts")
        .join(branch_id)
}

/// 认证仓库副本的路径：成品目录下的 `certifications/<record>.jpg`。
pub(crate) fn certification_copy_path(
    root: &Path,
    artwork_id: &str,
    branch_id: &str,
    record_id: &str,
) -> PathBuf {
    final_artifact_directory(root, artwork_id, branch_id)
        .join("certifications")
        .join(format!("{record_id}.jpg"))
}

pub(crate) struct BranchPublicationTarget {
    pub(crate) history_id: String,
    pub(crate) artifact_id: String,
    pub(crate) artifact_path: String,
    pub(crate) source_sha256: String,
}

/// 取消发布的结果：真正登记进待清理队列的条目，以及因内容已变化（或文件已不存在）
/// 而留在仓库里的路径（仓库相对路径）。
pub(crate) struct RemovedPublication {
    pub(crate) cleanup_ids: Vec<String>,
    pub(crate) retained_paths: Vec<String>,
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
            "SELECT f.history_id, f.id, f.source_path, f.source_sha256
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
                })
            },
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or_else(|| "分支尚未进入发布状态".to_owned())?;
    target.artifact_path = display_path(&storage::resolve_path(root, &target.artifact_path)?);
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
        value.source_path = display_path(&storage::resolve_path(root, &value.source_path)?);
    }
    Ok(artifact)
}

/// 解除分支发布状态：事务内登记仓库内成品与认证副本的清理意图，再删除认证配置、
/// 记录与成品行。
///
/// 清理登记前先核对磁盘内容与记录中的 SHA-256：一致才登记，被替换或已不存在的
/// 文件不登记、原样留在仓库里，交由设置页的垃圾文件扫描发现。这里不采用「带哈希
/// 登记后让清理失败」的做法，否则条目会长期留在队列里反复报同一句内容已变化。
///
/// 摘要就在事务内读取：取消发布本身已持独占运行锁，期间没有别的写者，成品与副本各读
/// 一次不会额外争锁；这样「登记清理意图」与「删除发布态数据」仍留在同一个事务里。
pub(crate) fn remove_artifact(root: &Path, branch_id: &str) -> Result<RemovedPublication, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    let artifact: Option<(String, String)> = transaction
        .query_row(
            "SELECT source_path, source_sha256 FROM final_artifacts WHERE branch_id = ?1",
            [branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let mut stored_statement = transaction
        .prepare(
            "SELECT stored_path, output_sha256
             FROM certification_records WHERE branch_id = ?1",
        )
        .map_err(storage::database_error)?;
    let stored_paths = stored_statement
        .query_map([branch_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    drop(stored_statement);
    let mut cleanup_ids = Vec::new();
    let mut retained_paths = Vec::new();
    for (relative, expected_sha256) in artifact.into_iter().chain(stored_paths) {
        let absolute = storage::resolve_path(root, &relative)?;
        if storage::verify_file_sha256(&absolute, &expected_sha256, "仓库文件").is_err() {
            retained_paths.push(relative);
            continue;
        }
        cleanup_ids.push(cleanup::enqueue_repository_file(
            &transaction,
            &relative,
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
    Ok(RemovedPublication {
        cleanup_ids,
        retained_paths,
    })
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
    use std::fs;

    use sha2::Digest;

    use super::*;
    use crate::authenticity::test_support::initialize_publication;

    const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn artifact<'a>(source_path: &'a str) -> NewFinalArtifact<'a> {
        NewFinalArtifact {
            id: "artifact",
            branch_id: "branch",
            history_id: "history",
            source_path,
            source_sha256: ZERO_SHA256,
            media_type: "image/jpeg",
            byte_size: 1,
            created_ms: 0,
        }
    }

    /// 在仓库内写入一个文件并返回其 SHA-256。
    fn write_repository_file(root: &Path, relative: &str, bytes: &[u8]) -> String {
        let absolute = storage::resolve_path(root, relative).unwrap();
        fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        fs::write(&absolute, bytes).unwrap();
        hex::encode_upper(sha2::Sha256::digest(bytes))
    }

    /// 写入一条已发布分支的成品行与配置行；`record` 给定时再写一条认证记录
    /// （仓库副本相对路径、副本摘要）。
    fn seed_publication(root: &Path, artifact_sha: &str, record: Option<(&str, &str)>) {
        let connection = storage::open(root).unwrap();
        connection
            .execute(
                "INSERT INTO final_artifacts
                   (id, branch_id, history_id, source_path, source_sha256,
                    media_type, byte_size, created_ms)
                 VALUES ('artifact', 'branch', 'history', 'artworks/final.jpg', ?1,
                         'image/jpeg', 1, 0)",
                [artifact_sha],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO certification_configs (branch_id, updated_ms) VALUES ('branch', 0)",
                [],
            )
            .unwrap();
        if let Some((stored_path, output_sha)) = record {
            connection
                .execute(
                    "INSERT INTO certification_records
                       (id, final_artifact_id, branch_id, history_id, watermark_id,
                        trustmark_enabled, output_path, stored_path, output_sha256,
                        output_bytes, title, creator, rights_statement,
                        authentication_content, regions_json, created_ms)
                     VALUES
                       ('record', 'artifact', 'branch', 'history', NULL, 0,
                        'C:/published/output.jpg', ?1, ?2,
                        1, 'Title', '', '', '', '[]', 0)",
                    [stored_path, output_sha],
                )
                .unwrap();
        }
    }

    #[test]
    fn final_artifact_atomically_takes_over_cleanup_intent() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize_publication(&root);
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
        initialize_publication(&root);

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
        initialize_publication(&root);
        let artifact_sha = write_repository_file(&root, "artworks/final.jpg", b"final artifact");
        let record_sha = write_repository_file(&root, "artworks/certified.jpg", b"certification");
        seed_publication(
            &root,
            &artifact_sha,
            Some(("artworks/certified.jpg", &record_sha)),
        );

        let removed = remove_artifact(&root, "branch").unwrap();

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
        assert_eq!(removed.cleanup_ids.len(), 2);
        assert!(removed.retained_paths.is_empty());
        assert_eq!(
            cleanup_entries,
            vec![
                ("repository_file".into(), "artworks/certified.jpg".into()),
                ("repository_file".into(), "artworks/final.jpg".into()),
            ]
        );
    }

    #[test]
    fn cancel_publication_retains_files_whose_content_changed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize_publication(&root);
        // 记录里是写入时的摘要，磁盘上随后被替换：两者不再一致。
        let artifact_sha = write_repository_file(&root, "artworks/final.jpg", b"final artifact");
        let record_sha = write_repository_file(&root, "artworks/certified.jpg", b"certification");
        seed_publication(
            &root,
            &artifact_sha,
            Some(("artworks/certified.jpg", &record_sha)),
        );
        let artifact_path = storage::resolve_path(&root, "artworks/final.jpg").unwrap();
        let record_path = storage::resolve_path(&root, "artworks/certified.jpg").unwrap();
        fs::write(&artifact_path, b"replaced artifact").unwrap();
        fs::remove_file(&record_path).unwrap();

        let removed = remove_artifact(&root, "branch").unwrap();

        assert!(removed.cleanup_ids.is_empty());
        assert_eq!(
            removed.retained_paths,
            vec![
                "artworks/final.jpg".to_owned(),
                "artworks/certified.jpg".to_owned(),
            ]
        );
        // 被替换的文件留在仓库里，消失的文件不会被登记成一条永远失败的清理项。
        assert!(artifact_path.exists());
        // 取消发布本身仍然成功：没有待清理项，发布态数据也全部解除。
        let values: (i64, i64, i64) = storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM pending_file_cleanup),
                   (SELECT COUNT(*) FROM final_artifacts),
                   (SELECT COUNT(*) FROM certification_records)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(values, (0, 0, 0));
    }
}
