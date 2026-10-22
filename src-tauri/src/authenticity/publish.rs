//! 认证签名发布：参数校验、渲染复用、C2PA 签名、回读核对与失败回滚。
//!
//! 发布在调用方取得的认证操作范围内执行；渲染与缓存复用来自 `rendition`，文件
//! 发布与清理意图在同一套「先登记、再发布」的顺序下完成，避免中途失败留下孤儿
//! 文件。本模块不负责打开分支发布状态或取消发布。

use std::{fs, path::Path, time::Instant};

use zeroize::Zeroizing;

use crate::{cleanup, platform::path::display_path, storage};

use super::{
    c2pa,
    error::{AuthenticityError, AuthenticityResult},
    file_ops::{
        absolute_output_path, canonical_existing_file, copy_noclobber, sha256_file_cancelable,
    },
    model::{
        CertificationConfig, ManifestSummary, PublishBranchRequest, PublishResult,
        WatermarkSizeWarning, WATERMARK_MIN_STABLE_EDGE,
    },
    publication_repository,
    rendition::{
        auth_checkpoint, elapsed_ms, ensure_not_cancelled, is_jpeg_extension, normalize_config,
        prepare_rendition, rendition_identity,
    },
    repository::{self, NewCertificationRecord},
    state::{AuthenticityOperation, AuthenticityState},
    trustmark,
};

pub(crate) fn publish(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    mut request: PublishBranchRequest,
) -> AuthenticityResult<PublishResult> {
    ensure_not_cancelled(operation)?;
    let private_key = Zeroizing::new(std::mem::take(&mut request.private_key_pem));
    // 发布清单使用裁剪后的声明字段；预览允许保留用户原样输入，两者不共用这一步。
    request.config.title = request.config.title.trim().to_owned();
    request.config.creator = request.config.creator.trim().to_owned();
    request.config.rights_statement = request.config.rights_statement.trim().to_owned();
    request.config.authentication_content = request.config.authentication_content.trim().to_owned();
    normalize_config(&mut request.config, &request.branch_id);
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
        .is_none_or(|value| !is_jpeg_extension(&value.to_ascii_lowercase()))
    {
        return Err(AuthenticityError::InvalidInput(
            "发布版输出必须使用 .jpg 或 .jpeg 扩展名".into(),
        ));
    }

    let parent = output
        .parent()
        .ok_or_else(|| AuthenticityError::InvalidInput("输出目录无效".into()))?;
    fs::create_dir_all(parent)?;
    let temp_dir = tempfile::tempdir_in(parent)?;
    let signed_path = temp_dir.path().join("signed.jpg");
    let identity = rendition_identity(&target, &request.config, request.watermark_id.as_deref())?;
    auth_checkpoint(operation, "认证渲染")?;
    let cached = prepare_rendition(
        root,
        state,
        operation,
        &input,
        &request.config,
        &target,
        &identity,
        request.preview_cache_token.as_deref(),
        None,
    )?;
    let watermark_warnings = watermark_size_warnings(cached.width, cached.height, &request.config)?;
    for warning in &watermark_warnings {
        log::warn!(
            "watermark small region: branch_id={}, region_index={}, width={}, height={}, threshold={}",
            request.branch_id,
            warning.region_index,
            warning.width,
            warning.height,
            warning.threshold,
        );
    }
    auth_checkpoint(operation, "认证签名")?;
    // 未启用 TrustMark 时清单不写 `watermarkId`，但签名入口统一要求一个标识，
    // 这里取一个新的即可；该值不落库，也不参与 `ClaimExpectation` 比对。
    let identifier = match identity.identifier {
        Some(identifier) => identifier,
        None => trustmark::resolve_identifier(None)?,
    };

    let record_id = storage::new_id();
    let signing_started = Instant::now();
    c2pa::sign_jpeg(
        &request.config,
        private_key.as_bytes(),
        &record_id,
        &identifier,
        &input,
        &cached.path,
        &signed_path,
        operation.cancellation_flag(),
    )?;
    ensure_not_cancelled(operation)?;
    let signing_ms = elapsed_ms(signing_started);
    let manifest = c2pa::read_manifest(&signed_path)?;
    ensure_not_cancelled(operation)?;
    validate_signed_manifest(&manifest, &request.config, &record_id, &identifier)?;
    let output_path = display_path(&output);
    let output_sha256 = sha256_file_cancelable(&signed_path, operation)?;
    let output_bytes = fs::metadata(&signed_path)?.len();
    let created_ms = storage::now_ms().map_err(AuthenticityError::Task)?;
    let stored_destination =
        repository::certification_storage_path(root, &request.branch_id, &record_id)
            .map_err(AuthenticityError::Task)?;
    let stored_relative =
        storage::relative_path(root, &stored_destination).map_err(AuthenticityError::Task)?;
    ensure_not_cancelled(operation)?;
    // 清理意图提交后必须走完原子发布/恢复序列：在中间中断会把部分成功暴露给调用方。
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
    if let Err(error) = copy_noclobber(&signed_path, &output) {
        let message = match cleanup::discard(root, &cleanup_ids) {
            Ok(()) => error.to_string(),
            Err(cleanup_error) => format!("{}；无法撤销清理登记：{}", error, cleanup_error),
        };
        return Err(AuthenticityError::Task(message));
    }
    let stored_directory = stored_destination
        .parent()
        .ok_or_else(|| AuthenticityError::Task("认证副本目录无效".into()))?;
    if let Err(error) = fs::create_dir_all(stored_directory)
        .map_err(AuthenticityError::Io)
        .and_then(|()| copy_noclobber(&output, &stored_destination))
    {
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
    Ok(PublishResult {
        record,
        width: cached.width,
        height: cached.height,
        watermark_warnings,
        rendition_cache_hit: cached.cache_hit,
        render_ms: cached.render_ms,
        encode_ms: cached.encode_ms,
        signing_ms,
    })
}

/// 计算小选区水印警告：某个选区在 rendition 上的任一边长小于稳定边长阈值时提示。
///
/// 判定用的是送进编码器的那张 rendition 的像素尺寸（`width`/`height` 来自渲染或
/// 缓存元数据），与 `trustmark::encode_regions` 共用 `region_bounds` 的像素换算。
/// 这是提示不是错误——发布照常成功。TrustMark 未启用时没有选区，返回空表。
fn watermark_size_warnings(
    width: u32,
    height: u32,
    config: &CertificationConfig,
) -> AuthenticityResult<Vec<WatermarkSizeWarning>> {
    if !config.trustmark_enabled {
        return Ok(Vec::new());
    }
    Ok(
        trustmark::region_bounds(width, height, &config.additional_regions)?
            .into_iter()
            .enumerate()
            .filter_map(|(index, (_, _, region_width, region_height))| {
                (region_width.min(region_height) < WATERMARK_MIN_STABLE_EDGE).then_some(
                    WatermarkSizeWarning {
                        region_index: index as u32,
                        width: region_width,
                        height: region_height,
                        threshold: WATERMARK_MIN_STABLE_EDGE,
                    },
                )
            })
            .collect(),
    )
}

fn validate_signed_manifest(
    manifest: &ManifestSummary,
    config: &CertificationConfig,
    record_id: &str,
    identifier: &str,
) -> AuthenticityResult<()> {
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
    let expected = c2pa::ClaimExpectation {
        record_id,
        watermark_id: config.trustmark_enabled.then_some(identifier),
        title: config.title.trim(),
        creator: config.creator.trim(),
        rights_statement: config.rights_statement.trim(),
        authentication_content: config.authentication_content.trim(),
    };
    if !c2pa::matches_expectation(manifest, &expected) {
        return Err(AuthenticityError::Task(
            "签名后的 C2PA 声明与本次发布参数不匹配".into(),
        ));
    }
    Ok(())
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

pub(crate) fn validate_publication_config(config: &CertificationConfig) -> AuthenticityResult<()> {
    c2pa::supported_signing_algorithm(&config.signing_algorithm)?;
    if config.title.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("作品标题不能为空".into()));
    }
    if config.creator.trim().is_empty() {
        return Err(AuthenticityError::InvalidInput("创作者不能为空".into()));
    }
    if !Path::new(&config.certificate_path).is_file() {
        return Err(AuthenticityError::InvalidInput("证书链文件不存在".into()));
    }
    if !(1..=100).contains(&config.jpeg_quality) {
        return Err(AuthenticityError::InvalidInput(
            "JPEG 质量必须在 1 到 100 之间".into(),
        ));
    }
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

#[cfg(test)]
mod tests {
    use std::fs::File;

    use image::codecs::jpeg::JpegEncoder;

    use super::*;
    use crate::authenticity::test_support::{
        fixture_config, fixture_directory, fixture_state, FIXTURE_RECORD_ID, FIXTURE_WATERMARK_ID,
    };

    #[test]
    #[ignore = "regenerates checked-in C2PA fixtures"]
    fn regenerate_c2pa_fixtures() {
        let directory = fixture_directory();
        let source_path = directory.join("source.jpg");
        let source = image::open(&source_path).unwrap();
        let flattened = super::super::rendition::flatten_to_rgb(&source, image::Rgb([255; 3]));
        let state = fixture_state();
        let config = fixture_config();
        let rendition = super::super::trustmark::encode_regions(
            &state,
            flattened,
            FIXTURE_WATERMARK_ID,
            config.watermark_strength,
            &config.additional_regions,
        )
        .unwrap();
        let unsigned = directory.join("unsigned.jpg");
        let mut unsigned_file = File::create(&unsigned).unwrap();
        JpegEncoder::new_with_quality(&mut unsigned_file, config.jpeg_quality)
            .encode_image(&rendition)
            .unwrap();
        drop(unsigned_file);

        let signed = directory.join("valid-trustmark.jpg");
        super::super::c2pa::sign_jpeg(
            &config,
            include_bytes!("../../tests/fixtures/authenticity/es256-test.priv"),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
            &source_path,
            &unsigned,
            &signed,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        fs::remove_file(unsigned).unwrap();

        let mut tampered = fs::read(&signed).unwrap();
        let offset = tampered.len() - 10;
        tampered[offset] ^= 1;
        fs::write(directory.join("tampered-trustmark.jpg"), tampered).unwrap();
    }

    #[test]
    fn real_c2pa_fixture_matches_record_trustmark_and_claims() {
        let fixture = fixture_directory().join("valid-trustmark.jpg");
        let manifest = super::super::c2pa::read_manifest(&fixture).unwrap();
        let decoded = super::super::trustmark::decode_region(
            &fixture_state(),
            image::open(&fixture).unwrap(),
            None,
        )
        .unwrap();

        validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .unwrap();
        assert_eq!(manifest.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(manifest.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert_eq!(decoded.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert!(manifest.manifest_json.is_some());
    }

    #[test]
    fn tampered_c2pa_fixture_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("tampered-trustmark.jpg"))
                .unwrap();

        assert!(manifest.present);
        assert!(!manifest.validation_accepted);
        assert!(validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }

    #[test]
    fn replacement_without_manifest_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("source.jpg")).unwrap();

        assert!(!manifest.present);
        assert!(validate_signed_manifest(
            &manifest,
            &fixture_config(),
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }

    #[test]
    fn mismatched_expected_claim_fails_closed() {
        let manifest =
            super::super::c2pa::read_manifest(&fixture_directory().join("valid-trustmark.jpg"))
                .unwrap();
        let mut config = fixture_config();
        config.authentication_content = "different expected claim".into();

        assert!(validate_signed_manifest(
            &manifest,
            &config,
            FIXTURE_RECORD_ID,
            FIXTURE_WATERMARK_ID,
        )
        .is_err());
    }
}
