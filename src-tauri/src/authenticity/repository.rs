use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::storage;

use super::model::{
    BranchPublication, CertificationConfig, CertificationRecord, FinalArtifact, NormalizedRegion,
};

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

pub(crate) struct NewCertificationRecord<'a> {
    pub(crate) id: &'a str,
    pub(crate) final_artifact_id: &'a str,
    pub(crate) branch_id: &'a str,
    pub(crate) history_id: &'a str,
    pub(crate) watermark_id: Option<&'a str>,
    pub(crate) output_path: &'a str,
    pub(crate) output_sha256: &'a str,
    pub(crate) output_bytes: u64,
    pub(crate) config: &'a CertificationConfig,
    pub(crate) c2pa_manifest_json: Option<&'a str>,
    pub(crate) validation_state: Option<&'a str>,
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

pub(crate) fn get_publication(
    root: &Path,
    branch_id: &str,
    models_ready: bool,
) -> Result<BranchPublication, String> {
    let connection = storage::open(root)?;
    let mut artifact = connection
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
    let config = load_config(&connection, branch_id)?;
    let records = records_for_branch(&connection, branch_id)?;
    Ok(BranchPublication {
        branch_id: branch_id.to_owned(),
        artifact,
        config,
        records,
        models_ready,
    })
}

fn load_config(connection: &Connection, branch_id: &str) -> Result<CertificationConfig, String> {
    connection
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
        .map_err(storage::database_error)?
        .map(|(mut config, json)| {
            config.additional_regions = serde_json::from_str(&json)
                .map_err(|error| format!("认证区域配置无效：{error}"))?;
            Ok(config)
        })
        .transpose()?
        .map(Ok)
        .unwrap_or_else(|| default_config(connection, branch_id))
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
) -> Result<CertificationRecord, String> {
    let mut connection = storage::open(root)?;
    let transaction = connection.transaction().map_err(storage::database_error)?;
    save_config(&transaction, record.config, record.created_ms)?;
    transaction
        .execute(
            "INSERT INTO certification_records
             (id, final_artifact_id, branch_id, history_id, watermark_id,
              trustmark_enabled, output_path, output_sha256, output_bytes,
              title, creator, rights_statement, authentication_content, regions_json,
              c2pa_manifest_label, c2pa_manifest_json, validation_state, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                record.id,
                record.final_artifact_id,
                record.branch_id,
                record.history_id,
                record.watermark_id,
                record.config.trustmark_enabled,
                record.output_path,
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
    transaction.commit().map_err(storage::database_error)?;
    search_records(root, record.id)?
        .into_iter()
        .next()
        .ok_or_else(|| "发布记录写入后无法回读".into())
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
        "WHERE record.watermark_id = ?1 ORDER BY record.created_ms DESC",
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
                record.trustmark_enabled, record.output_path, record.output_sha256,
                record.output_bytes, record.title, record.creator, record.rights_statement,
                record.authentication_content, record.regions_json,
                record.c2pa_manifest_label, record.c2pa_manifest_json,
                record.validation_state, record.created_ms
         FROM certification_records record
         JOIN branches b ON b.id = record.branch_id
         JOIN library_nodes artwork ON artwork.id = b.artwork_id
         {clause}"
    );
    let mut statement = connection.prepare(&sql).map_err(storage::database_error)?;
    statement
        .query_map([value], certification_record_from_row)
        .map_err(storage::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::database_error)
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

fn certification_record_from_row(row: &Row<'_>) -> rusqlite::Result<CertificationRecord> {
    let regions_json: String = row.get(15)?;
    let additional_regions: Vec<NormalizedRegion> =
        serde_json::from_str(&regions_json).unwrap_or_default();
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
