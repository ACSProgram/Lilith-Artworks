use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use image::{codecs::jpeg::JpegEncoder, GenericImageView};
use sha2::{Digest, Sha256};
use tempfile::{tempdir_in, NamedTempFile};
use zeroize::Zeroizing;

use crate::storage;

use super::{
    c2pa,
    error::{AuthenticityError, AuthenticityResult},
    model::{CertificationRecord, DecodeRequest, DecodeResult, PublishBranchRequest},
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

pub(crate) fn publish(
    root: &Path,
    state: &AuthenticityState,
    mut request: PublishBranchRequest,
) -> AuthenticityResult<PublishedOutput> {
    let private_key = Zeroizing::new(std::mem::take(&mut request.private_key_pem));
    request.config.trustmark_enabled =
        request.config.trustmark_enabled && !request.config.additional_regions.is_empty();
    validate_publish_request(&request, &private_key)?;
    let target = publication_repository::publication_target(root, &request.branch_id)
        .map_err(AuthenticityError::Task)?;
    let input = canonical_existing_file(&target.artifact_path, "最终成品")?;
    let output = absolute_output_path(&request.output_path)?;
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

    c2pa::sign_jpeg(
        &request.config,
        private_key.as_bytes(),
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
    fs::rename(&signed_path, &output)?;

    let output_path = storage::display_path(&output);
    let output_sha256 = sha256_file(&output)?;
    let output_bytes = fs::metadata(&output)?.len();
    let record_id = storage::new_id();
    let created_ms = storage::now_ms().map_err(AuthenticityError::Task)?;
    let (stored_path, stored_relative) =
        match store_certification_copy(root, &request.branch_id, &record_id, &output) {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_file(&output);
                return Err(error);
            }
        };
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
    );
    let record = match inserted {
        Ok(record) => record,
        Err(error) => {
            let _ = fs::remove_file(&output);
            let _ = fs::remove_file(&stored_path);
            return Err(AuthenticityError::Task(error));
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

fn store_certification_copy(
    root: &Path,
    branch_id: &str,
    record_id: &str,
    source: &Path,
) -> AuthenticityResult<(PathBuf, String)> {
    let destination = repository::certification_storage_path(root, branch_id, record_id)
        .map_err(AuthenticityError::Task)?;
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
    let relative = storage::relative_path(root, &destination).map_err(AuthenticityError::Task)?;
    Ok((destination, relative))
}

pub(crate) fn decode(
    root: &Path,
    state: &AuthenticityState,
    request: DecodeRequest,
) -> AuthenticityResult<DecodeResult> {
    let input = canonical_existing_file(&request.input_path, "待识别图片")?;
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
    let lookup_id = watermark_id.as_ref().or(manifest.watermark_id.as_ref());
    let matches = lookup_id
        .map(|identifier| repository::records_by_identifier(root, identifier))
        .transpose()
        .map_err(AuthenticityError::Task)?
        .unwrap_or_default();
    Ok(DecodeResult {
        watermark_present: watermark_id.is_some(),
        watermark_id,
        decoded_region,
        c2pa_present: manifest.present,
        c2pa_validation_state: manifest.validation_state,
        c2pa_validation_status: manifest.validation_status,
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

fn validate_publish_request(
    request: &PublishBranchRequest,
    private_key: &str,
) -> AuthenticityResult<()> {
    if request.config.title.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("作品标题不能为空".into()));
    }
    if request.config.creator.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("创作者不能为空".into()));
    }
    if private_key.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("请粘贴 PEM 私钥".into()));
    }
    if request.config.trustmark_enabled && request.config.additional_regions.is_empty() {
        return Err(AuthenticityError::InvalidInput(
            "启用 TrustMark 前请先框选水印区域".into(),
        ));
    }
    if !Path::new(&request.config.certificate_path).is_file() {
        return Err(AuthenticityError::InvalidInput("证书链文件不存在".into()));
    }
    if !(1..=100).contains(&request.config.jpeg_quality) {
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
