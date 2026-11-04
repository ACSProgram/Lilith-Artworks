use std::path::Path;

use crate::storage;

use super::c2pa;

struct CertificationExpectation {
    id: String,
    stored_path: String,
    output_sha256: String,
    watermark_id: Option<String>,
    title: String,
    creator: String,
    rights_statement: String,
    authentication_content: String,
}

pub(crate) fn scrub_controlled_files(
    root: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64, u64),
) -> Result<(u64, u64), String> {
    let connection = storage::open(root)?;
    let artifacts = {
        let mut statement = connection
            .prepare("SELECT source_path, source_sha256 FROM final_artifacts ORDER BY id")
            .map_err(storage::database_error)?;
        let values = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage::database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage::database_error)?;
        values
    };
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT id, stored_path, output_sha256, watermark_id, title, creator,
                        rights_statement, authentication_content
                 FROM certification_records ORDER BY id",
            )
            .map_err(storage::database_error)?;
        let values = statement
            .query_map([], |row| {
                Ok(CertificationExpectation {
                    id: row.get(0)?,
                    stored_path: row.get(1)?,
                    output_sha256: row.get(2)?,
                    watermark_id: row.get(3)?,
                    title: row.get(4)?,
                    creator: row.get(5)?,
                    rights_statement: row.get(6)?,
                    authentication_content: row.get(7)?,
                })
            })
            .map_err(storage::database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage::database_error)?;
        values
    };
    drop(connection);

    let total = (artifacts.len() + records.len()) as u64;
    let mut checked = 0_u64;
    for (relative, sha256) in &artifacts {
        ensure_not_cancelled(&cancelled)?;
        let path = storage::resolve_path(root, relative)?;
        storage::verify_file_sha256(&path, sha256, "仓库最终成品")?;
        checked += 1;
        progress(checked, total);
    }
    for record in &records {
        ensure_not_cancelled(&cancelled)?;
        let path = storage::resolve_path(root, &record.stored_path)?;
        storage::verify_file_sha256(&path, &record.output_sha256, "认证仓库副本")?;
        let manifest = c2pa::read_manifest(&path).map_err(|error| error.to_string())?;
        // 仓库副本的 SHA-256 已在上面核对。旧格式清单无法再用新校验器验签，
        // 但「文件字节与登记一致 + 声明与数据库一致」仍然成立，因此不放行才是误报。
        let verified = manifest.validation_accepted || manifest.legacy_format();
        if !manifest.present || !verified {
            return Err(format!(
                "认证记录 {} 的 C2PA 清单未通过完整性验证",
                record.id
            ));
        }
        let expected = c2pa::ClaimExpectation {
            record_id: &record.id,
            watermark_id: record.watermark_id.as_deref(),
            title: &record.title,
            creator: &record.creator,
            rights_statement: &record.rights_statement,
            authentication_content: &record.authentication_content,
        };
        if !c2pa::matches_expectation(&manifest, &expected) {
            return Err(format!("认证记录 {} 的 C2PA 声明与数据库不匹配", record.id));
        }
        checked += 1;
        progress(checked, total);
    }
    Ok((artifacts.len() as u64, records.len() as u64))
}

fn ensure_not_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("仓库完整性检查已取消".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use rusqlite::params;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::authenticity::test_support::{
        fixture_directory, initialize_record_fixture, FIXTURE_RECORD_ID, FIXTURE_WATERMARK_ID,
    };

    #[test]
    fn controlled_file_scrub_detects_a_replaced_final_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("source.psd");
        fs::File::create(&source)
            .unwrap()
            .write_all(b"source")
            .unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, false).unwrap();
        let history_id = storage::new_id();
        let artifact_id = storage::new_id();
        let relative = format!("artworks/{}/artifacts/final.jpg", artwork.artwork_id);
        let path = storage::resolve_path(&root, &relative).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"original").unwrap();
        let sha256 = hex::encode_upper(Sha256::digest(b"original"));
        let connection = storage::open(&root).unwrap();
        connection
            .execute(
                "INSERT INTO history_nodes
                 (id, artwork_id, created_on_branch_id, title, note, commit_kind,
                  is_checkpoint, created_ms, logical_size, chunk_file_size, sha256,
                  chunk_count, snapshot_path)
                 VALUES (?1, ?2, ?3, 'History', '', 'manual', 1, 0, 1, 1, ?4, 1,
                         'artworks/snapshot.lbc')",
                params![
                    history_id,
                    artwork.artwork_id,
                    artwork.branch_id,
                    "0".repeat(64)
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET head_history_id = ?2 WHERE id = ?1",
                params![artwork.branch_id, history_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO final_artifacts
                 (id, branch_id, history_id, source_path, source_sha256,
                  media_type, byte_size, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'image/jpeg', 8, 0)",
                params![artifact_id, artwork.branch_id, history_id, relative, sha256],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            scrub_controlled_files(&root, || false, |_, _| {}).unwrap(),
            (1, 0)
        );
        fs::write(&path, b"replacement").unwrap();

        let error = scrub_controlled_files(&root, || false, |_, _| {}).unwrap_err();
        assert!(error.contains("已损坏或被替换"), "{error}");
    }

    /// 旧格式清单无法验签，但仓库副本摘要与数据库一致、声明也一致时应通过扫描；
    /// 否则每个 0.90 记录都会在设置页被误报为损坏。
    #[test]
    fn legacy_manifests_pass_the_scrub_after_the_hash_and_claims_match() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let bytes = fs::read(fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();
        let relative = "artworks/legacy-certifications/record.jpg";
        let stored = storage::resolve_path(&root, relative).unwrap();
        let sha = hex::encode_upper(Sha256::digest(&bytes));
        initialize_record_fixture(&root, relative, &sha);

        // 建库之后再落盘，避免初始化把已有文件误判为非空目录。
        fs::create_dir_all(stored.parent().unwrap()).unwrap();
        fs::write(&stored, &bytes).unwrap();

        // 最终成品也指向真实文件，避免扫描先停在成品阶段。
        let artifact = storage::resolve_path(&root, "artworks/final.jpg").unwrap();
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&artifact, b"artifact").unwrap();
        let artifact_sha = hex::encode_upper(Sha256::digest(b"artifact"));
        let connection = storage::open(&root).unwrap();
        connection
            .execute(
                "UPDATE final_artifacts SET source_sha256 = ?1, byte_size = 8",
                [artifact_sha],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE certification_records
                 SET id = ?1, watermark_id = ?2, trustmark_enabled = 1, title = ?3,
                     creator = ?4, rights_statement = ?5,
                     authentication_content = ?6, output_bytes = ?7
                 WHERE id = 'record'",
                params![
                    FIXTURE_RECORD_ID,
                    FIXTURE_WATERMARK_ID,
                    "Lilith C2PA fixture",
                    "Lilith Artworks tests",
                    "Regression fixture only",
                    "A tiny deterministic TrustMark/C2PA sample",
                    bytes.len() as i64,
                ],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            scrub_controlled_files(&root, || false, |_, _| {}).unwrap(),
            (1, 1)
        );
    }
}
