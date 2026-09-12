use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::{cleanup, storage};

use super::model::{BranchPublication, CertificationConfig, CertificationRecord, NormalizedRegion};

pub(crate) struct NewCertificationRecord<'a> {
    pub(crate) id: &'a str,
    pub(crate) final_artifact_id: &'a str,
    pub(crate) branch_id: &'a str,
    pub(crate) history_id: &'a str,
    pub(crate) watermark_id: Option<&'a str>,
    pub(crate) output_path: &'a str,
    pub(crate) stored_path: &'a str,
    pub(crate) output_sha256: &'a str,
    pub(crate) output_bytes: u64,
    pub(crate) config: &'a CertificationConfig,
    pub(crate) c2pa_manifest_json: Option<&'a str>,
    pub(crate) validation_state: Option<&'a str>,
    pub(crate) created_ms: i64,
}

pub(crate) fn get_publication(
    root: &Path,
    branch_id: &str,
    models_ready: bool,
    model_info: (String, Option<String>, Option<String>),
) -> Result<BranchPublication, String> {
    let connection = storage::open(root)?;
    let artifact = super::publication_repository::find_artifact(root, branch_id)?;
    let config = load_config(&connection, branch_id)?;
    let records = records_for_branch(&connection, branch_id)?;
    Ok(BranchPublication {
        branch_id: branch_id.to_owned(),
        artifact,
        config,
        records,
        models_ready,
        model_variant: model_info.0,
        encoder_sha256: model_info.1,
        decoder_sha256: model_info.2,
    })
}

fn load_config(connection: &Connection, branch_id: &str) -> Result<CertificationConfig, String> {
    let stored = connection
        .query_row(
            "SELECT branch_id, title, creator, rights_statement, authentication_content,
                    trustmark_enabled, certificate_path, signing_algorithm,
                    timestamp_url, jpeg_quality, background_color, watermark_strength,
                    additional_regions_json, updated_ms
             FROM certification_configs WHERE branch_id = ?1",
            [branch_id],
            |row| {
                let regions_json: String = row.get(12)?;
                Ok((
                    CertificationConfig {
                        branch_id: row.get(0)?,
                        title: row.get(1)?,
                        creator: row.get(2)?,
                        rights_statement: row.get(3)?,
                        authentication_content: row.get(4)?,
                        trustmark_enabled: row.get::<_, i64>(5)? != 0,
                        certificate_path: row.get(6)?,
                        signing_algorithm: row.get(7)?,
                        timestamp_url: row.get(8)?,
                        jpeg_quality: row.get(9)?,
                        background_color: row.get(10)?,
                        watermark_strength: row.get(11)?,
                        additional_regions: Vec::new(),
                        updated_ms: row.get(13)?,
                    },
                    regions_json,
                ))
            },
        )
        .optional()
        .map_err(storage::database_error)?;
    match stored {
        Some((mut config, json)) => {
            config.additional_regions = serde_json::from_str(&json)
                .map_err(|error| format!("认证区域配置无效：{error}"))?;
            Ok(config)
        }
        None => default_config(connection, branch_id),
    }
}

fn default_config(connection: &Connection, branch_id: &str) -> Result<CertificationConfig, String> {
    let title: String = connection
        .query_row(
            "SELECT artwork.title FROM branches b
             JOIN library_nodes artwork ON artwork.id = b.artwork_id WHERE b.id = ?1",
            [branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("找不到分支")?;
    Ok(CertificationConfig {
        branch_id: branch_id.to_owned(),
        title,
        creator: String::new(),
        rights_statement: String::new(),
        authentication_content: String::new(),
        trustmark_enabled: true,
        certificate_path: String::new(),
        signing_algorithm: "es256".into(),
        timestamp_url: None,
        jpeg_quality: 92,
        background_color: "#FFFFFF".into(),
        watermark_strength: 0.95,
        additional_regions: Vec::new(),
        updated_ms: 0,
    })
}

pub(crate) fn insert_record(
    root: &Path,
    record: &NewCertificationRecord<'_>,
    cleanup_ids: &[String],
) -> Result<CertificationRecord, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    save_config(&transaction, record.config, record.created_ms)?;
    transaction
        .execute(
            "INSERT INTO certification_records
             (id, final_artifact_id, branch_id, history_id, watermark_id,
              trustmark_enabled, output_path, stored_path, output_sha256, output_bytes,
              title, creator, rights_statement, authentication_content, regions_json,
              c2pa_manifest_label, c2pa_manifest_json, validation_state, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                record.id,
                record.final_artifact_id,
                record.branch_id,
                record.history_id,
                record.watermark_id,
                record.config.trustmark_enabled,
                record.output_path,
                record.stored_path,
                record.output_sha256,
                record.output_bytes,
                record.config.title.trim(),
                record.config.creator.trim(),
                record.config.rights_statement.trim(),
                record.config.authentication_content.trim(),
                serde_json::to_string(&record.config.additional_regions)
                    .map_err(|error| format!("无法保存水印区域：{error}"))?,
                super::c2pa::ASSERTION_LABEL,
                record.c2pa_manifest_json,
                record.validation_state,
                record.created_ms,
            ],
        )
        .map_err(storage::database_error)?;
    cleanup::complete(&transaction, cleanup_ids)?;
    let inserted = query_records(
        &transaction,
        "WHERE record.id = ?1 ORDER BY record.created_ms DESC",
        record.id,
    )?
    .into_iter()
    .next()
    .ok_or_else(|| "发布记录写入后无法回读".to_owned())?;
    transaction.commit().map_err(storage::database_error)?;
    Ok(inserted)
}

fn save_config(
    transaction: &Transaction<'_>,
    config: &CertificationConfig,
    updated_ms: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO certification_configs
             (branch_id, title, creator, rights_statement, authentication_content,
              trustmark_enabled, certificate_path, signing_algorithm,
              timestamp_url, jpeg_quality, background_color, watermark_strength,
              additional_regions_json, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(branch_id) DO UPDATE SET
               title=excluded.title, creator=excluded.creator,
               rights_statement=excluded.rights_statement,
               authentication_content=excluded.authentication_content,
               trustmark_enabled=excluded.trustmark_enabled,
               certificate_path=excluded.certificate_path,
               signing_algorithm=excluded.signing_algorithm,
               timestamp_url=excluded.timestamp_url, jpeg_quality=excluded.jpeg_quality,
               background_color=excluded.background_color,
               watermark_strength=excluded.watermark_strength,
               additional_regions_json=excluded.additional_regions_json,
               updated_ms=excluded.updated_ms",
            params![
                config.branch_id,
                config.title.trim(),
                config.creator.trim(),
                config.rights_statement.trim(),
                config.authentication_content.trim(),
                config.trustmark_enabled,
                config.certificate_path.trim(),
                config.signing_algorithm.trim(),
                config
                    .timestamp_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty()),
                config.jpeg_quality,
                config.background_color,
                config.watermark_strength,
                serde_json::to_string(&config.additional_regions)
                    .map_err(|error| format!("无法保存水印区域：{error}"))?,
                updated_ms,
            ],
        )
        .map_err(storage::database_error)?;
    Ok(())
}

pub(crate) fn records_by_identifier(
    root: &Path,
    identifier: &str,
) -> Result<Vec<CertificationRecord>, String> {
    query_records(
        &storage::open(root)?,
        "WHERE record.id = ?1 OR record.watermark_id = ?1 ORDER BY record.created_ms DESC",
        identifier,
    )
}

pub(crate) fn search_records(root: &Path, query: &str) -> Result<Vec<CertificationRecord>, String> {
    let pattern = format!("%{}%", query.trim());
    query_records(
        &storage::open(root)?,
        "WHERE record.id LIKE ?1 OR record.watermark_id LIKE ?1 OR record.title LIKE ?1
           OR record.creator LIKE ?1 OR record.output_path LIKE ?1
         ORDER BY record.created_ms DESC LIMIT 100",
        &pattern,
    )
}

pub(crate) fn certification_storage_path(
    root: &Path,
    branch_id: &str,
    record_id: &str,
) -> Result<PathBuf, String> {
    let artwork_id: String = storage::open(root)?
        .query_row(
            "SELECT artwork_id FROM branches WHERE id = ?1",
            [branch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage::database_error)?
        .ok_or("找不到发布记录所属分支")?;
    Ok(root
        .join("artworks")
        .join(artwork_id)
        .join("artifacts")
        .join(branch_id)
        .join("certifications")
        .join(format!("{record_id}.jpg")))
}

pub(crate) fn record_source_path(root: &Path, record_id: &str) -> Result<PathBuf, String> {
    let stored: Option<(String, String)> = storage::open(root)?
        .query_row(
            "SELECT stored_path, output_sha256 FROM certification_records WHERE id = ?1",
            [record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage::database_error)?;
    let (stored_path, output_sha256) = stored.ok_or("找不到发布记录")?;
    let path = storage::resolve_path(root, &stored_path)?;
    storage::verify_file_sha256(&path, &output_sha256, "认证仓库副本")?;
    Ok(path)
}

fn records_for_branch(
    connection: &Connection,
    branch_id: &str,
) -> Result<Vec<CertificationRecord>, String> {
    query_records(
        connection,
        "WHERE record.branch_id = ?1 ORDER BY record.created_ms DESC",
        branch_id,
    )
}

fn query_records(
    connection: &Connection,
    clause: &str,
    value: &str,
) -> Result<Vec<CertificationRecord>, String> {
    let sql = format!(
        "SELECT record.id, b.artwork_id, artwork.title, record.branch_id, b.title,
                record.history_id, record.watermark_id,
                record.trustmark_enabled, record.output_path,
                record.output_sha256, record.output_bytes, record.title, record.creator, record.rights_statement,
                record.authentication_content, record.regions_json,
                record.c2pa_manifest_label, record.c2pa_manifest_json,
                record.validation_state, record.created_ms
         FROM certification_records record
         JOIN branches b ON b.id = record.branch_id
         JOIN library_nodes artwork ON artwork.id = b.artwork_id
         {clause}"
    );
    let mut statement = connection.prepare(&sql).map_err(storage::database_error)?;
    let records = statement
        .query_map([value], certification_record_from_row)
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)?;
    Ok(records)
}

fn certification_record_from_row(row: &Row<'_>) -> rusqlite::Result<CertificationRecord> {
    let regions_json: String = row.get(15)?;
    let additional_regions: Vec<NormalizedRegion> =
        serde_json::from_str(&regions_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                regions_json.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(CertificationRecord {
        id: row.get(0)?,
        artwork_id: row.get(1)?,
        artwork_title: row.get(2)?,
        branch_id: row.get(3)?,
        branch_title: row.get(4)?,
        history_id: row.get(5)?,
        watermark_id: row.get(6)?,
        trustmark_enabled: row.get::<_, i64>(7)? != 0,
        output_path: row.get(8)?,
        output_sha256: row.get(9)?,
        output_bytes: row.get(10)?,
        title: row.get(11)?,
        creator: row.get(12)?,
        rights_statement: row.get(13)?,
        authentication_content: row.get(14)?,
        additional_regions,
        c2pa_manifest_label: row.get(16)?,
        c2pa_manifest_json: row.get(17)?,
        validation_state: row.get(18)?,
        created_ms: row.get(19)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::Digest;

    use super::*;

    fn record_fixture(root: &Path, stored_path: &str, output_sha256: &str) {
        crate::library::initialize(root).unwrap();
        storage::open(root)
            .unwrap()
            .execute_batch(&format!(
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
                         1, 0, 1, 1, '{zeros}', 1, 'artworks/snapshot.chunk');
                 UPDATE branches SET head_history_id = 'history' WHERE id = 'branch';
                 INSERT INTO final_artifacts
                   (id, branch_id, history_id, source_path, source_sha256,
                    media_type, byte_size, created_ms)
                 VALUES ('artifact', 'branch', 'history', 'artworks/final.jpg',
                         '{zeros}', 'image/jpeg', 1, 0);
                 INSERT INTO certification_records
                   (id, final_artifact_id, branch_id, history_id, watermark_id,
                    trustmark_enabled, output_path, stored_path, output_sha256,
                    output_bytes, title, creator, rights_statement,
                    authentication_content, regions_json, created_ms)
                 VALUES ('record', 'artifact', 'branch', 'history', NULL, 0,
                         'C:/published/output.jpg', '{stored_path}', '{output_sha256}',
                         8, 'Title', '', '', '', '[]', 0);",
                zeros = "0".repeat(64),
            ))
            .unwrap();
    }

    #[test]
    fn invalid_regions_json_is_not_silently_replaced() {
        let connection = Connection::open_in_memory().unwrap();
        let error = connection
            .query_row(
                "SELECT 'record', 'artwork', 'Artwork', 'branch', 'Branch',
                        'history', NULL, 0, 'C:/output.jpg', 'hash', 1,
                        'Title', 'Creator', '', '', '{', NULL, NULL, NULL, 0",
                [],
                certification_record_from_row,
            )
            .unwrap_err();

        assert!(
            matches!(error, rusqlite::Error::FromSqlConversionFailure(..)),
            "{error}"
        );
    }

    #[test]
    fn record_source_rejects_a_replaced_repository_copy() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let stored_path = "artworks/certified.jpg";
        let expected = hex::encode_upper(sha2::Sha256::digest(b"original"));
        record_fixture(&root, stored_path, &expected);
        let absolute = storage::resolve_path(&root, stored_path).unwrap();
        fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        fs::write(&absolute, b"replacement").unwrap();

        let error = record_source_path(&root, "record").unwrap_err();

        assert!(error.contains("已损坏或被替换"), "{error}");
    }
}
