//! 认证模块的测试夹具：C2PA fixture、模型状态、最小发布仓库与认证记录。
//!
//! 只编译进测试构建。`scrub` 的测试走 `library::create_artwork` 的真实建库路径，
//! 有自己的夹具，不使用这里的裸 SQL。

use std::path::{Path, PathBuf};

use crate::storage;

use super::{
    model::{CertificationConfig, NormalizedRegion},
    state::AuthenticityState,
};

pub(crate) const FIXTURE_RECORD_ID: &str = "fixture-record-0001";
pub(crate) const FIXTURE_WATERMARK_ID: &str = "1010101010101010101010101010101010101010";
const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub(crate) fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("authenticity")
}

pub(crate) fn fixture_state() -> AuthenticityState {
    AuthenticityState::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("models"),
    )
}

pub(crate) fn fixture_config() -> CertificationConfig {
    let directory = fixture_directory();
    CertificationConfig {
        branch_id: "fixture-branch".into(),
        title: "Lilith C2PA fixture".into(),
        creator: "Lilith Artworks tests".into(),
        rights_statement: "Regression fixture only".into(),
        authentication_content: "A tiny deterministic TrustMark/C2PA sample".into(),
        trustmark_enabled: true,
        certificate_path: directory
            .join("es256-test.pub")
            .to_string_lossy()
            .into_owned(),
        signing_algorithm: "es256".into(),
        timestamp_url: None,
        jpeg_quality: 90,
        background_color: "#FFFFFF".into(),
        watermark_strength: 1.0,
        additional_regions: vec![NormalizedRegion {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }],
        updated_ms: 0,
    }
}

/// 最小发布仓库：作品 `artwork`、分支 `branch`、历史节点 `history`，并让分支指向该节点。
pub(crate) fn initialize_publication(root: &Path) {
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

/// 为 `initialize_publication` 建立的分支登记一条最终成品。
pub(crate) fn insert_final_artifact(root: &Path, relative_path: &str, source_sha256: &str) {
    storage::open(root)
        .unwrap()
        .execute(
            "INSERT INTO final_artifacts
               (id, branch_id, history_id, source_path, source_sha256,
                media_type, byte_size, created_ms)
             VALUES ('artifact', 'branch', 'history', ?1, ?2, 'image/png', 1, 0)",
            rusqlite::params![relative_path, source_sha256],
        )
        .unwrap();
}

/// 最小认证记录夹具：最终成品、认证配置与一条记录。
pub(crate) fn initialize_record_fixture(root: &Path, stored_path: &str, output_sha256: &str) {
    initialize_publication(root);
    let connection = storage::open(root).unwrap();
    connection
        .execute(
            "INSERT INTO final_artifacts
               (id, branch_id, history_id, source_path, source_sha256,
                media_type, byte_size, created_ms)
             VALUES ('artifact', 'branch', 'history', 'artworks/final.jpg', ?1,
                     'image/jpeg', 1, 0)",
            [ZERO_SHA256],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO certification_records
               (id, final_artifact_id, branch_id, history_id, watermark_id,
                trustmark_enabled, output_path, stored_path, output_sha256,
                output_bytes, title, creator, rights_statement,
                authentication_content, regions_json, created_ms)
             VALUES ('record', 'artifact', 'branch', 'history', NULL, 0,
                     'C:/published/output.jpg', ?1, ?2,
                     8, 'Title', '', '', '', '[]', 0)",
            rusqlite::params![stored_path, output_sha256],
        )
        .unwrap();
}
