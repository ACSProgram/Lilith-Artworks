use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, GenericImageView};
use sha2::{Digest, Sha256};
use tempfile::{tempdir_in, NamedTempFile};
use zeroize::Zeroizing;

use crate::{cleanup, storage};

use super::{
    c2pa,
    error::{AuthenticityError, AuthenticityResult},
    model::{
        CertificationRecord, DecodeRequest, DecodeResult, PreviewImage, PublicationPreview,
        PublicationPreviewRequest, PublishBranchRequest,
    },
    publication_repository,
    repository::{self, NewCertificationRecord},
    state::AuthenticityState,
    trustmark,
};

pub(crate) struct PublishedOutput {
    pub(crate) record: CertificationRecord,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) watermark_region_count: u32,
}

pub(crate) fn preview(
    root: &Path,
    state: &AuthenticityState,
    mut request: PublicationPreviewRequest,
) -> AuthenticityResult<PublicationPreview> {
    request.config.branch_id = request.branch_id.clone();
    request.config.trustmark_enabled =
        request.config.trustmark_enabled && !request.config.additional_regions.is_empty();
    validate_publication_config(&request.config)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let source = image::open(&input)?;
    let (width, height) = source.dimensions();
    let background = trustmark::parse_background(&request.config.background_color)?;
    let flattened = trustmark::flatten_to_rgb(&source, background);
    let identifier = request
        .config
        .trustmark_enabled
        .then(|| trustmark::resolve_identifier(request.watermark_id.as_deref()))
        .transpose()?;
    let rendition = if let Some(identifier) = identifier.as_deref() {
        trustmark::encode_regions(
            state,
            flattened,
            identifier,
            request.config.watermark_strength,
            &request.config.additional_regions,
        )?
    } else {
        flattened
    };
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, request.config.jpeg_quality)
        .encode_image(&rendition)?;
    let output_bytes = encoded.len() as u64;
    let (original_media_type, original_bytes) = match target.media_type.as_str() {
        "image/jpeg" | "image/png" | "image/webp" => {
            (target.media_type.as_str(), fs::read(&input)?)
        }
        _ => {
            let mut encoded = Cursor::new(Vec::new());
            source.write_to(&mut encoded, image::ImageFormat::Png)?;
            ("image/png", encoded.into_inner())
        }
    };
    Ok(PublicationPreview {
        image: PreviewImage {
            data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(encoded)),
            width,
            height,
            source_bytes: fs::metadata(&input)?.len(),
        },
        original_image: PreviewImage {
            data_url: format!(
                "data:{};base64,{}",
                original_media_type,
                STANDARD.encode(original_bytes)
            ),
            width,
            height,
            source_bytes: fs::metadata(&input)?.len(),
        },
        output_bytes,
        watermark_id: identifier,
    })
}

pub(crate) fn publish(
    root: &Path,
    state: &AuthenticityState,
    mut request: PublishBranchRequest,
) -> AuthenticityResult<PublishedOutput> {
    let private_key = Zeroizing::new(std::mem::take(&mut request.private_key_pem));
    request.config.title = request.config.title.trim().to_owned();
    request.config.creator = request.config.creator.trim().to_owned();
    request.config.rights_statement = request.config.rights_statement.trim().to_owned();
    request.config.authentication_content = request.config.authentication_content.trim().to_owned();
    request.config.trustmark_enabled =
        request.config.trustmark_enabled && !request.config.additional_regions.is_empty();
    validate_publish_request(&request, &private_key)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let output = absolute_output_path(&request.output_path)?;
    storage::ensure_outside_repository(root, &output, "发布输出路径")
        .map_err(AuthenticityError::InvalidInput)?;
    if output.exists() {
        return Err(AuthenticityError::InvalidInput(
            "输出文件已存在；请选择新的文件名".into(),
        ));
    }
    if input == output {
        return Err(AuthenticityError::InvalidInput(
            "输出路径不能覆盖最终成品".into(),
        ));
    }
    if output
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !matches!(value.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
    {
        return Err(AuthenticityError::InvalidInput(
            "发布版输出必须使用 .jpg 或 .jpeg 扩展名".into(),
        ));
    }

    request.config.branch_id = request.branch_id.clone();
    let parent = output
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    fs::create_dir_all(parent)?;
    let temp_dir = tempdir_in(parent)?;
    let unsigned_path = temp_dir.path().join("rendition.jpg");
    let signed_path = temp_dir.path().join("signed.jpg");
    let source = image::open(&input)?;
    let (width, height) = source.dimensions();
    let background = trustmark::parse_background(&request.config.background_color)?;
    let flattened = trustmark::flatten_to_rgb(&source, background);
    let identifier = trustmark::resolve_identifier(request.watermark_id.as_deref())?;
    let rendition = if request.config.trustmark_enabled {
        trustmark::encode_regions(
            state,
            flattened,
            &identifier,
            request.config.watermark_strength,
            &request.config.additional_regions,
        )?
    } else {
        flattened
    };
    let mut unsigned_file = File::create(&unsigned_path)?;
    JpegEncoder::new_with_quality(&mut unsigned_file, request.config.jpeg_quality)
        .encode_image(&rendition)?;
    drop(unsigned_file);

    let record_id = storage::new_id();
    c2pa::sign_jpeg(
        &request.config,
        private_key.as_bytes(),
        &record_id,
        &identifier,
        &input,
        &unsigned_path,
        &signed_path,
    )?;
    let manifest = c2pa::read_manifest(&signed_path)?;
    if !manifest.present {
        return Err(AuthenticityError::Task(
            "签名完成后未能回读 C2PA 清单".into(),
        ));
    }
    if !manifest.validation_accepted {
        return Err(AuthenticityError::Task(
            "签名后的 C2PA 清单未通过完整性验证".into(),
        ));
    }
    if manifest.record_id.as_deref() != Some(record_id.as_str())
        || manifest.title.as_deref() != Some(request.config.title.trim())
        || manifest.creator.as_deref() != Some(request.config.creator.trim())
        || manifest.rights_statement.as_deref() != Some(request.config.rights_statement.trim())
        || manifest.authentication_content.as_deref()
            != Some(request.config.authentication_content.trim())
        || (request.config.trustmark_enabled
            && manifest.watermark_id.as_deref() != Some(identifier.as_str()))
        || (!request.config.trustmark_enabled && manifest.watermark_id.is_some())
    {
        return Err(AuthenticityError::Task(
            "签名后的 C2PA 声明与本次发布参数不匹配".into(),
        ));
    }
    let output_path = storage::display_path(&output);
    let output_sha256 = sha256_file(&signed_path)?;
    let output_bytes = fs::metadata(&signed_path)?.len();
    let created_ms = storage::now_ms().map_err(AuthenticityError::Task)?;
    let stored_destination =
        repository::certification_storage_path(root, &request.branch_id, &record_id)
            .map_err(AuthenticityError::Task)?;
    let stored_relative =
        storage::relative_path(root, &stored_destination).map_err(AuthenticityError::Task)?;
    let cleanup_ids = {
        let mut connection = storage::open(root).map_err(AuthenticityError::Task)?;
        let transaction = connection
            .transaction()
            .map_err(|error| AuthenticityError::Task(storage::database_error(error)))?;
        let external_id = cleanup::enqueue_external_file(
            &transaction,
            &output_path,
            &output_sha256,
            "publish_certification",
        )
        .map_err(AuthenticityError::Task)?;
        let stored_id = cleanup::enqueue_repository_file_with_hash(
            &transaction,
            &stored_relative,
            &output_sha256,
            "publish_certification",
        )
        .map_err(AuthenticityError::Task)?;
        transaction
            .commit()
            .map_err(|error| AuthenticityError::Task(storage::database_error(error)))?;
        vec![external_id, stored_id]
    };
    if let Err(error) = publish_noclobber(&signed_path, &output) {
        let message = match cleanup::discard(root, &cleanup_ids) {
            Ok(()) => error.to_string(),
            Err(cleanup_error) => format!("{}；无法撤销清理登记：{}", error, cleanup_error),
        };
        return Err(AuthenticityError::Task(message));
    }
    if let Err(error) = store_certification_copy(&stored_destination, &output) {
        let message = recover_failed_publication(root, &cleanup_ids, error.to_string());
        return Err(AuthenticityError::Task(message));
    }
    let inserted = repository::insert_record(
        root,
        &NewCertificationRecord {
            id: &record_id,
            final_artifact_id: &target.artifact_id,
            branch_id: &request.branch_id,
            history_id: &target.history_id,
            watermark_id: request
                .config
                .trustmark_enabled
                .then_some(identifier.as_str()),
            output_path: &output_path,
            stored_path: &stored_relative,
            output_sha256: &output_sha256,
            output_bytes,
            config: &request.config,
            c2pa_manifest_json: manifest.manifest_json.as_deref(),
            validation_state: manifest.validation_state.as_deref(),
            created_ms,
        },
        &cleanup_ids,
    );
    let record = match inserted {
        Ok(record) => record,
        Err(error) => {
            return Err(AuthenticityError::Task(recover_failed_publication(
                root,
                &cleanup_ids,
                error,
            )));
        }
    };
    Ok(PublishedOutput {
        record,
        width,
        height,
        watermark_region_count: if request.config.trustmark_enabled {
            request.config.additional_regions.len() as u32
        } else {
            0
        },
    })
}

fn store_certification_copy(destination: &Path, source: &Path) -> AuthenticityResult<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::Task("认证副本目录无效".into()))?;
    fs::create_dir_all(directory)?;
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

fn publish_noclobber(source: &Path, destination: &Path) -> AuthenticityResult<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    let mut input = File::open(source)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

fn recover_failed_publication(root: &Path, cleanup_ids: &[String], error: String) -> String {
    match cleanup::run(root, cleanup_ids) {
        Ok(report) if report.failures.is_empty() => error,
        Ok(report) => format!(
            "{}；有 {} 个发布文件清理失败，将在下次启动时重试",
            error,
            report.failures.len()
        ),
        Err(cleanup_error) => format!("{}；发布文件清理任务失败：{}", error, cleanup_error),
    }
}

pub(crate) fn decode(
    root: &Path,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> AuthenticityResult<DecodeResult> {
    let input = canonical_existing_file(&request.input_path, "待识别图片")?;
    storage::ensure_outside_repository(root, &input, "待识别图片")
        .map_err(AuthenticityError::InvalidInput)?;
    let image = image::open(&input)?;
    let decoded_region = request.region;
    let watermark_id = if state.model_files_ready() {
        trustmark::decode_region(state, &image, decoded_region)?
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
    matches: &mut Vec<super::model::CertificationMatch>,
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
            matches.push(super::model::CertificationMatch {
                record,
                evidence_sources: vec![evidence_source.to_owned()],
            });
        }
    }
}

fn validate_publish_request(
    request: &PublishBranchRequest,
    private_key: &str,
) -> AuthenticityResult<()> {
    if private_key.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("请粘贴 PEM 私钥".into()));
    }
    if request.config.trustmark_enabled && request.config.additional_regions.is_empty() {
        return Err(AuthenticityError::InvalidInput(
            "启用 TrustMark 前请先框选水印区域".into(),
        ));
    }
    validate_publication_config(&request.config)?;
    Ok(())
}

fn validate_publication_config(
    config: &super::model::CertificationConfig,
) -> AuthenticityResult<()> {
    if config.title.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("作品标题不能为空".into()));
    }
    if config.creator.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("创作者不能为空".into()));
    }
    if !Path::new(&config.certificate_path).is_file() {
        return Err(AuthenticityError::InvalidInput("证书链文件不存在".into()));
    }
    validate_visual_config(config)
}

fn validate_visual_config(config: &super::model::CertificationConfig) -> AuthenticityResult<()> {
    if !(1..=100).contains(&config.jpeg_quality) {
        return Err(AuthenticityError::InvalidInput(
            "JPEG 质量必须在 1 到 100 之间".into(),
        ));
    }
    Ok(())
}

fn canonical_existing_file(value: &str, label: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if !path.is_file() {
        return Err(AuthenticityError::InvalidInput(format!(
            "{label}文件不存在"
        )));
    }
    Ok(path.canonicalize()?)
}

fn absolute_output_path(value: &str) -> AuthenticityResult<PathBuf> {
    let path = Path::new(value.trim());
    if path.as_os_str().is_empty() {
        return Err(AuthenticityError::InvalidInput("请选择输出路径".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(crate) fn sha256_file(path: &Path) -> AuthenticityResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> CertificationRecord {
        CertificationRecord {
            id: id.into(),
            artwork_id: "artwork".into(),
            artwork_title: "Artwork".into(),
            branch_id: "branch".into(),
            branch_title: "Branch".into(),
            history_id: "history".into(),
            watermark_id: Some("0".repeat(super::trustmark::WATERMARK_BITS)),
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
}
