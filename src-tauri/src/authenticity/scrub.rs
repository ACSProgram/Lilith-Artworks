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
    progress: impl Fn(u64, u64),
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
        if !manifest.present || !manifest.validation_accepted {
            return Err(format!(
                "认证记录 {} 的 C2PA 清单未通过完整性验证",
                record.id
            ));
        }
        if manifest.record_id.as_deref() != Some(record.id.as_str())
            || manifest.watermark_id != record.watermark_id
            || manifest.title.as_deref() != Some(record.title.as_str())
            || manifest.creator.as_deref() != Some(record.creator.as_str())
            || manifest.rights_statement.as_deref() != Some(record.rights_statement.as_str())
            || manifest.authentication_content.as_deref()
                != Some(record.authentication_content.as_str())
        {
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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
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
}
