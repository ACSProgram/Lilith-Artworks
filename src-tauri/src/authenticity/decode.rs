//! 认证回读：C2PA 清单、TrustMark 区域解码与本地记录候选合并。
//!
//! 模型不可用时只读取 C2PA 清单，不打开整图；两条证据通道各自独立，合并时按记录
//! ID 去重并保留来源，不把任一候选自动判为可信。

use std::{collections::HashMap, path::Path};

use image::DynamicImage;

use crate::storage;

use super::{
    c2pa,
    error::{AuthenticityError, AuthenticityResult},
    file_ops::canonical_existing_file,
    image_resource,
    model::{CertificationMatch, CertificationRecord, DecodeRequest, DecodeResult},
    repository,
    state::AuthenticityState,
    trustmark,
};

pub(crate) fn decode(
    root: &Path,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> AuthenticityResult<DecodeResult> {
    decode_with(root, state, request, image_resource::open)
}

/// 图片打开步骤可注入：模型不可用路径用它断言整图未被解码，而不是靠计时或日志。
fn decode_with(
    root: &Path,
    state: &AuthenticityState,
    request: DecodeRequest,
    open_image: impl FnOnce(&Path) -> AuthenticityResult<DynamicImage>,
) -> AuthenticityResult<DecodeResult> {
    let input = canonical_existing_file(&request.input_path, "待识别图片")?;
    storage::ensure_outside_repository(root, &input, "待识别图片")
        .map_err(AuthenticityError::InvalidInput)?;
    let decoded_region = request.region;
    let watermark_id = if state.model_files_ready() {
        let image = open_image(&input)?;
        trustmark::decode_region(state, image, decoded_region)?
    } else {
        None
    };
    let manifest = c2pa::read_manifest(&input)?;
    let identifiers_match = match (&watermark_id, &manifest.watermark_id) {
        (Some(decoded), Some(declared)) => Some(decoded == declared),
        _ => None,
    };
    let mut matches = Vec::new();
    let mut match_indexes = HashMap::new();
    if let Some(identifier) = manifest
        .record_id
        .as_ref()
        .or(manifest.watermark_id.as_ref())
    {
        merge_matches(
            &mut matches,
            &mut match_indexes,
            repository::records_by_identifier(root, identifier).map_err(AuthenticityError::Task)?,
            "c2pa",
        );
    }
    if let Some(identifier) = watermark_id.as_ref() {
        merge_matches(
            &mut matches,
            &mut match_indexes,
            repository::records_by_identifier(root, identifier).map_err(AuthenticityError::Task)?,
            "trustmark",
        );
    }
    Ok(DecodeResult {
        watermark_present: watermark_id.is_some(),
        watermark_id,
        decoded_region,
        c2pa_present: manifest.present,
        c2pa_validation_state: manifest.validation_state,
        c2pa_validation_status: manifest.validation_status,
        c2pa_record_id: manifest.record_id,
        c2pa_watermark_id: manifest.watermark_id,
        identifiers_match,
        title: manifest.title,
        creator: manifest.creator,
        rights_statement: manifest.rights_statement,
        authentication_content: manifest.authentication_content,
        manifest_json: manifest.manifest_json,
        matches,
    })
}

fn merge_matches(
    matches: &mut Vec<CertificationMatch>,
    indexes: &mut HashMap<String, usize>,
    records: Vec<CertificationRecord>,
    evidence_source: &str,
) {
    for record in records {
        if let Some(index) = indexes.get(&record.id).copied() {
            let sources = &mut matches[index].evidence_sources;
            if !sources.iter().any(|source| source == evidence_source) {
                sources.push(evidence_source.to_owned());
            }
        } else {
            indexes.insert(record.id.clone(), matches.len());
            matches.push(CertificationMatch {
                record,
                evidence_sources: vec![evidence_source.to_owned()],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::authenticity::test_support::{fixture_directory, FIXTURE_WATERMARK_ID};

    fn record(id: &str) -> CertificationRecord {
        CertificationRecord {
            id: id.into(),
            artwork_id: "artwork".into(),
            artwork_title: "Artwork".into(),
            branch_id: "branch".into(),
            branch_title: "Branch".into(),
            history_id: "history".into(),
            watermark_id: Some(FIXTURE_WATERMARK_ID.into()),
            trustmark_enabled: true,
            output_path: "output.jpg".into(),
            output_sha256: "A".repeat(64),
            output_bytes: 1,
            title: "Title".into(),
            creator: "Creator".into(),
            rights_statement: String::new(),
            authentication_content: String::new(),
            additional_regions: Vec::new(),
            c2pa_manifest_label: None,
            c2pa_manifest_json: None,
            validation_state: None,
            created_ms: 1,
        }
    }

    #[test]
    fn merges_candidate_evidence_without_hiding_conflicts() {
        let mut matches = Vec::new();
        let mut indexes = HashMap::new();
        merge_matches(&mut matches, &mut indexes, vec![record("same")], "c2pa");
        merge_matches(
            &mut matches,
            &mut indexes,
            vec![record("same"), record("other")],
            "trustmark",
        );

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].record.id, "same");
        assert_eq!(matches[0].evidence_sources, vec!["c2pa", "trustmark"]);
        assert_eq!(matches[1].record.id, "other");
        assert_eq!(matches[1].evidence_sources, vec!["trustmark"]);
    }

    #[test]
    fn model_unavailable_reads_c2pa_without_decoding_the_image() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        fs::create_dir_all(&root).unwrap();
        let input = directory.path().join("source.jpg");
        fs::copy(fixture_directory().join("source.jpg"), &input).unwrap();
        let state = AuthenticityState::new(directory.path().join("missing-models"));
        assert!(!state.model_files_ready());

        let result = decode_with(
            &root,
            &state,
            DecodeRequest {
                input_path: input.to_string_lossy().into_owned(),
                region: None,
            },
            |_| panic!("模型不可用时不应解码整图"),
        )
        .unwrap();

        assert!(!result.watermark_present);
        assert!(result.watermark_id.is_none());
        assert!(!result.c2pa_present);
        assert!(result.matches.is_empty());
    }
}
