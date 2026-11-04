use std::{
    fs::{self, File},
    path::Path,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use c2pa::{
    assertions::SoftBinding, create_signer, Builder, BuilderIntent, ClaimGeneratorInfo, Context,
    Reader, Signer, SigningAlg, ValidationState,
};
use serde_json::{json, Value};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    legacy,
    model::{CertificationConfig, ManifestSummary, ValidationItem, LEGACY_VALIDATION_STATE},
    trustmark::{
        identifier_from_soft_binding, soft_binding_value, SOFT_BINDING_ALGORITHM, WATERMARK_BITS,
    },
};

pub(crate) const ASSERTION_LABEL: &str = "com.lilith.artworks.claim";
const TIMESTAMP_TIMEOUT: Duration = Duration::from_secs(30);
const CANCELLATION_POLL: Duration = Duration::from_millis(100);

/// 只验证签名与声明，不验证 CA 信任链。
///
/// 证书由用户自己提供，通常不在 C2PA 公共信任列表里。`c2pa 0.90` 把
/// `signingCredential.untrusted` 当成提示，0.91 起把它算作失败：若不显式关闭，
/// 每个自管证书签出的清单都会变成 `Invalid`，发布回读和全库扫描会全部拒绝。
/// 本应用的信任模型是「本地记录 + 签名完整性」，从不宣称 `Trusted`，因此这里
/// 保留 0.90 的语义，并把签名后的自动验证交给 `validate_signed_manifest` 单独把关。
fn verification_context() -> c2pa::Result<Context> {
    // `Store::from_context` 会用 `clear()` 抹掉内置 EKU 列表；不在这里补回，
    // 任何证书都会被判 `signingCredential.invalid`（缺少 EKU）。这份列表与
    // c2pa 自带的 `valid_eku_oids.cfg` 一致。
    const ACCEPTED_EKUS: &str = "1.3.6.1.5.5.7.3.4\n\
        1.3.6.1.5.5.7.3.36\n\
        1.3.6.1.5.5.7.3.8\n\
        1.3.6.1.5.5.7.3.9\n\
        1.3.6.1.4.1.311.76.59.1.9\n\
        1.3.6.1.4.1.62558.2.1";
    Context::new().with_settings(json!({
        "verify": {
            "verify_after_sign": false,
            "verify_trust": false,
        },
        "trust": {
            "trust_config": ACCEPTED_EKUS,
        }
    }))
}

struct BoundedSigner {
    inner: c2pa::BoxedSigner,
    cancelled: Arc<AtomicBool>,
    timestamp_timed_out: Arc<AtomicBool>,
}

impl Signer for BoundedSigner {
    fn sign(&self, data: &[u8]) -> c2pa::Result<Vec<u8>> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(c2pa::Error::OperationCancelled);
        }
        self.inner.sign(data)
    }

    fn alg(&self) -> SigningAlg {
        self.inner.alg()
    }

    fn certs(&self) -> c2pa::Result<Vec<Vec<u8>>> {
        self.inner.certs()
    }

    fn reserve_size(&self) -> usize {
        self.inner.reserve_size()
    }

    fn time_authority_url(&self) -> Option<String> {
        self.inner.time_authority_url()
    }

    fn timestamp_request_headers(&self) -> Option<Vec<(String, String)>> {
        self.inner.timestamp_request_headers()
    }

    fn timestamp_request_body(&self, message: &[u8]) -> c2pa::Result<Vec<u8>> {
        self.inner.timestamp_request_body(message)
    }

    fn send_timestamp_request(&self, message: &[u8]) -> Option<c2pa::Result<Vec<u8>>> {
        let url = self.time_authority_url()?;
        let body = match self.timestamp_request_body(message) {
            Ok(body) => body,
            Err(error) => return Some(Err(error)),
        };
        let headers = self.timestamp_request_headers();
        let message = message.to_vec();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let config = ureq::Agent::config_builder()
                .max_redirects(0)
                .timeout_global(Some(TIMESTAMP_TIMEOUT))
                .build();
            let context = Context::new().with_resolver(ureq::Agent::new_with_config(config));
            let result = c2pa::crypto::time_stamp::default_rfc3161_request(
                &url, headers, &body, &message, &context,
            )
            .map_err(c2pa::Error::from);
            let _ = sender.send(result);
        });

        Some(wait_for_timestamp(
            receiver,
            &self.cancelled,
            &self.timestamp_timed_out,
            TIMESTAMP_TIMEOUT,
        ))
    }
}

fn wait_for_timestamp(
    receiver: mpsc::Receiver<c2pa::Result<Vec<u8>>>,
    cancelled: &AtomicBool,
    timed_out: &AtomicBool,
    timeout: Duration,
) -> c2pa::Result<Vec<u8>> {
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(c2pa::Error::OperationCancelled);
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            timed_out.store(true, Ordering::SeqCst);
            return Err(c2pa::Error::CoseTimeStampGeneration);
        }
        match receiver.recv_timeout(remaining.min(CANCELLATION_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(c2pa::Error::ThreadReceiveError);
            }
        }
    }
}

pub(crate) fn sign_jpeg(
    config: &CertificationConfig,
    private_key: &[u8],
    record_id: &str,
    identifier: &str,
    source_path: &Path,
    unsigned_jpeg: &Path,
    signed_jpeg: &Path,
    cancelled: Arc<AtomicBool>,
) -> AuthenticityResult<()> {
    let algorithm = supported_signing_algorithm(&config.signing_algorithm)?;
    let certificate = fs::read(&config.certificate_path)?;
    let timestamp_url = config
        .timestamp_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let timestamp_timed_out = Arc::new(AtomicBool::new(false));
    let signer = BoundedSigner {
        inner: create_signer::from_keys(&certificate, private_key, algorithm, timestamp_url)?,
        cancelled: cancelled.clone(),
        timestamp_timed_out: timestamp_timed_out.clone(),
    };
    let mut definition = json!({
        "title": config.title,
        "format": "image/jpeg",
        "claim_generator_info": [{
            "name": "Lilith Artworks",
            "version": env!("CARGO_PKG_VERSION")
        }]
    });
    if config.trustmark_enabled {
        let binding_blocks = config
            .additional_regions
            .iter()
            .enumerate()
            .map(|(index, region)| {
                json!({
                    "scope": {
                        "region": {
                            "region": [{
                                "type": "spatial",
                                "shape": {
                                    "type": "rectangle",
                                    "unit": "percent",
                                    "origin": { "x": region.x * 100.0, "y": region.y * 100.0 },
                                    "width": region.width * 100.0,
                                    "height": region.height * 100.0,
                                    "inside": true
                                }
                            }],
                            "name": format!("Additional TrustMark region {}", index + 1),
                            "identifier": format!("lilith-trustmark-region-{}", index + 1)
                            // `role` 自 0.91 起废弃，且 c2pa_cbor 无法把该枚举
                            // 从文本反序列化回来：写入后整条断言会判 claim.malformed。
                            // 水印语义由自定义 claim 的 watermark 字段承载。
                        }
                    },
                    // 规范要求 bstr：这里传字节序列，定义解析时由 c2pa 编成 CBOR 字节串。
                    "value": soft_binding_value(identifier).into_bytes()
                })
            })
            .collect::<Vec<_>>();
        // 软绑定必须作为**清单定义**的一部分加入：`Builder::add_assertion` 会把它
        // 留在 JSON 断言里，写入的 JUMBF 虽然是 CBOR，0.91 的校验器仍会拿 JSON
        // 副本解码并判 `claim.malformed`。定义里的断言在解析阶段就转成 CBOR。
        definition["assertions"] = json!([{
            "label": SoftBinding::LABEL,
            "data": {
                "alg": SOFT_BINDING_ALGORITHM,
                "blocks": binding_blocks,
                "name": "Lilith Artworks durable identifier",
                "alg-params": "variant=Q;schema=BCH_SUPER;payload=binary"
            }
        }]);
    }
    let mut builder =
        Builder::from_context(verification_context()?).with_definition(definition.to_string())?;
    builder.definition.claim_version = Some(2);
    builder.set_intent(BuilderIntent::Edit);
    let mut generator = ClaimGeneratorInfo::new("Lilith Artworks");
    generator.set_version(env!("CARGO_PKG_VERSION"));
    builder.set_claim_generator_info(generator);

    builder.add_assertion_json(
        "stds.schema-org.CreativeWork",
        &json!({
            "@context": "https://schema.org",
            "@type": "CreativeWork",
            "name": config.title,
            "author": [{ "@type": "Person", "name": config.creator }],
            "copyrightNotice": config.rights_statement
        }),
    )?;
    builder.add_assertion(
        ASSERTION_LABEL,
        &json!({
            "version": 1,
            "recordId": record_id,
            "watermarkId": config.trustmark_enabled.then_some(identifier),
            "authenticationContent": config.authentication_content,
            "creator": config.creator,
            "rightsStatement": config.rights_statement,
            "watermark": config.trustmark_enabled.then(|| json!({
                "algorithm": SOFT_BINDING_ALGORITHM,
                "variant": "Q",
                "schema": "BCH_SUPER",
                "strength": config.watermark_strength,
                "coverage": "selectedRegions",
                "regions": config.additional_regions
            })),
            "rendition": { "format": "image/jpeg", "quality": config.jpeg_quality }
        }),
    )?;
    let source_title = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("source artwork");
    let mut source_file = File::open(source_path)?;
    builder.add_ingredient_from_stream(
        json!({
            "title": source_title,
            "relationship": "parentOf",
            "label": "source-artwork"
        })
        .to_string(),
        super::rendition::image_media_type_for_path(source_path).ok_or_else(|| {
            AuthenticityError::InvalidInput(
                "C2PA ingredient 仅支持 PNG、JPEG、WebP 或 TIFF 原图".into(),
            )
        })?,
        &mut source_file,
    )?;
    if config.trustmark_enabled {
        builder.add_action(json!({
            "action": "c2pa.watermarked",
            "parameters": {
                "ingredientIds": ["source-artwork"],
                "description": "Embedded an Adobe TrustMark Q durable identifier",
                "algorithm": SOFT_BINDING_ALGORITHM
            }
        }))?;
    }
    builder.add_action(json!({
        "action": "c2pa.transcoded",
        "parameters": { "from": "source artwork", "to": "image/jpeg", "quality": config.jpeg_quality }
    }))?;
    if cancelled.load(Ordering::SeqCst) {
        return Err(AuthenticityError::Task("认证任务已取消".into()));
    }
    if let Err(error) = builder.sign_file(&signer, unsigned_jpeg, signed_jpeg) {
        if cancelled.load(Ordering::SeqCst) {
            return Err(AuthenticityError::Task("认证任务已取消".into()));
        }
        if timestamp_timed_out.load(Ordering::SeqCst) {
            return Err(AuthenticityError::Task(
                "时间戳服务在 30 秒内未响应，发布已取消".into(),
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn supported_signing_algorithm(value: &str) -> AuthenticityResult<SigningAlg> {
    let normalized = value.trim().to_ascii_lowercase();
    if !matches!(normalized.as_str(), "es256" | "es384" | "ed25519") {
        return Err(AuthenticityError::InvalidInput(
            "仅支持 ES256、ES384 或 Ed25519；PS256 因依赖安全公告已禁用".into(),
        ));
    }
    SigningAlg::from_str(&normalized)
        .map_err(|_| AuthenticityError::InvalidInput("不支持的 C2PA 签名算法".into()))
}

/// 读取清单；严格读取器读不出、或把声明的软绑定判为 `claim.malformed` 时，
/// 回退到旧格式只读识别。
///
/// 回退只用于识别，不提升验证结论：旧格式结果一律标记为
/// [`LEGACY_VALIDATION_STATE`]，`validation_accepted` 恒为 false。
pub(crate) fn read_manifest(path: &Path) -> AuthenticityResult<ManifestSummary> {
    read_manifest_with(path, |path| {
        Reader::from_context(verification_context()?).with_file(path)
    })
}

fn read_manifest_with(
    path: &Path,
    open: impl FnOnce(&Path) -> c2pa::Result<Reader>,
) -> AuthenticityResult<ManifestSummary> {
    let reader = match open(path) {
        Ok(reader) => reader,
        Err(c2pa::Error::JumbfNotFound | c2pa::Error::JumbfBoxNotFound) => {
            return Ok(legacy_manifest(path)
                .map(legacy_summary)
                .unwrap_or_else(ManifestSummary::absent));
        }
        // 0.91 对旧软绑定既可能直接报错，也可能返回一个 `Invalid` 读取器；
        // 后者在下面 `summarize` 之后处理。
        Err(error) => {
            return match legacy_manifest(path) {
                Some(manifest) => Ok(legacy_summary(manifest)),
                None => Err(error.into()),
            };
        }
    };
    let summary = summarize(reader);
    if summary.validation_accepted {
        return Ok(summary);
    }
    // 0.91 读到 0.90 的软绑定不会报错：它返回 `Invalid` + `claim.malformed`，
    // 同时仍能取出声明。只有确认软绑定是旧格式的文本串时才改为旧格式结果；
    // 当前格式但验证失败的清单（例如文件内容被替换）保留原状态。
    Ok(legacy_manifest(path).map(legacy_summary).unwrap_or(summary))
}

/// 只接受确认为 0.90 文本串软绑定的解析结果；缺失软绑定的当前格式清单不算旧格式。
fn legacy_manifest(path: &Path) -> Option<legacy::LegacyManifest> {
    legacy::read(path).filter(|manifest| manifest.legacy_soft_binding)
}

/// 旧格式结果：能按声明匹配，但不验证签名、也不验证文件哈希。
fn legacy_summary(manifest: legacy::LegacyManifest) -> ManifestSummary {
    let mut summary = ManifestSummary {
        present: true,
        validation_accepted: false,
        validation_state: Some(LEGACY_VALIDATION_STATE.to_owned()),
        validation_status: vec![ValidationItem {
            code: "lilith.legacy_format".to_owned(),
            explanation:
                "旧版清单格式（软绑定为 CBOR 文本串）：只能按记录 ID 与声明匹配，签名未经验证。"
                    .to_owned(),
        }],
        record_id: manifest.record_id,
        watermark_id: manifest.watermark_id,
        title: manifest.title,
        creator: manifest.creator,
        rights_statement: manifest.rights_statement,
        authentication_content: manifest.authentication_content,
        manifest_json: None,
    };
    summary.manifest_json = Some(
        json!({
            "legacyFormat": true,
            "note": "由 c2pa 0.90 及更早版本签出的清单，只做了只读解析：未验证签名与文件哈希。",
            "recordId": summary.record_id,
            "watermarkId": summary.watermark_id,
            "title": summary.title,
            "creator": summary.creator,
            "rightsStatement": summary.rights_statement,
            "authenticationContent": summary.authentication_content,
        })
        .to_string(),
    );
    summary
}

/// 记录 ID 的合法范围。识别与发布回读共用这一条规则，避免两处判断漂移。
pub(crate) fn record_id_from_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 160).then(|| value.to_owned())
}

/// 40 位二进制 TrustMark ID 的合法性判断。
pub(crate) fn watermark_id_from_value(value: &str) -> Option<String> {
    (value.len() == WATERMARK_BITS && value.bytes().all(|byte| matches!(byte, b'0' | b'1')))
        .then(|| value.to_owned())
}

fn summarize(reader: Reader) -> ManifestSummary {
    let validation_state = reader.validation_state();
    let validation_accepted = matches!(
        validation_state,
        ValidationState::Valid | ValidationState::Trusted
    );
    let validation_status = reader
        .validation_status()
        .unwrap_or_default()
        .iter()
        .map(|status| ValidationItem {
            code: status.code().to_owned(),
            explanation: status.explanation().unwrap_or_default().to_owned(),
        })
        .collect();
    let manifest = reader.active_manifest();
    let claim: Option<Value> =
        manifest.and_then(|active| active.find_assertion(ASSERTION_LABEL).ok());
    let soft_binding: Option<SoftBinding> =
        manifest.and_then(|active| active.find_assertion(SoftBinding::LABEL).ok());
    let record_id = claim
        .as_ref()
        .and_then(|value| value.get("recordId"))
        .and_then(Value::as_str)
        .and_then(record_id_from_value);
    let watermark_id = soft_binding
        .and_then(|binding| binding.blocks.first().map(|block| block.value.clone()))
        // 规范要求 bstr；内容仍是本应用的 `1*<40 位 ID>` 文本。
        .and_then(|value| String::from_utf8(value).ok())
        .and_then(|value| identifier_from_soft_binding(&value))
        .or_else(|| {
            claim
                .as_ref()
                .and_then(|value| value.get("watermarkId"))
                .and_then(Value::as_str)
                .and_then(watermark_id_from_value)
        });
    ManifestSummary {
        present: manifest.is_some(),
        validation_accepted,
        validation_state: Some(format!("{validation_state:?}")),
        validation_status,
        record_id,
        watermark_id,
        title: manifest.and_then(|active| active.title().map(str::to_owned)),
        creator: claim
            .as_ref()
            .and_then(|value| value.get("creator"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        rights_statement: claim
            .as_ref()
            .and_then(|value| value.get("rightsStatement"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        authentication_content: claim
            .as_ref()
            .and_then(|value| value.get("authenticationContent"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        manifest_json: Some(reader.json()),
    }
}

/// 期望的清单声明：发布后的回读核对与全库扫描共用同一组字段与比较规则。
pub(crate) struct ClaimExpectation<'a> {
    pub(crate) record_id: &'a str,
    /// 启用 TrustMark 时为本次软绑定 ID；未启用时必须是 `None`（清单里不得出现）。
    pub(crate) watermark_id: Option<&'a str>,
    pub(crate) title: &'a str,
    pub(crate) creator: &'a str,
    pub(crate) rights_statement: &'a str,
    pub(crate) authentication_content: &'a str,
}

/// 比对清单中的 record ID、TrustMark ID 与四项声明；不检查清单是否存在或验证状态，
/// 这两项由调用方按各自文案先行判定。
pub(crate) fn matches_expectation(
    manifest: &ManifestSummary,
    expected: &ClaimExpectation<'_>,
) -> bool {
    manifest.record_id.as_deref() == Some(expected.record_id)
        && manifest.watermark_id.as_deref() == expected.watermark_id
        && manifest.title.as_deref() == Some(expected.title)
        && manifest.creator.as_deref() == Some(expected.creator)
        && manifest.rights_statement.as_deref() == Some(expected.rights_statement)
        && manifest.authentication_content.as_deref() == Some(expected.authentication_content)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use super::{
        read_manifest, read_manifest_with, supported_signing_algorithm, wait_for_timestamp,
        LEGACY_VALIDATION_STATE, TIMESTAMP_TIMEOUT,
    };

    use crate::authenticity::test_support::{
        fixture_directory, FIXTURE_RECORD_ID, FIXTURE_WATERMARK_ID,
    };
    use crate::fuzz_support::random_bytes;

    /// 模拟 0.91 起的严格读取器：旧清单会被判为 `claim.malformed`。
    fn strict_rejection(_: &std::path::Path) -> c2pa::Result<super::Reader> {
        Err(c2pa::Error::InvalidAsset("claim.malformed".into()))
    }

    #[test]
    fn strict_reader_rejection_falls_back_to_the_legacy_manifest() {
        let summary = read_manifest_with(
            &fixture_directory().join("legacy-0.90-trustmark.jpg"),
            strict_rejection,
        )
        .unwrap();

        assert!(summary.present);
        assert!(!summary.validation_accepted);
        assert!(summary.legacy_format());
        assert_eq!(
            summary.validation_state.as_deref(),
            Some(LEGACY_VALIDATION_STATE)
        );
        assert_eq!(summary.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(summary.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
        assert_eq!(summary.title.as_deref(), Some("Lilith C2PA fixture"));
        assert!(summary
            .validation_status
            .iter()
            .any(|item| item.code == "lilith.legacy_format"));
        assert!(summary
            .manifest_json
            .as_deref()
            .is_some_and(|report| report.contains("\"legacyFormat\":true")));
    }

    /// 不注入错误时的真实回归：0.91 对 0.90 清单返回 `Invalid` 而不是报错，
    /// 回退仍必须把软绑定为文本串的清单识别为旧格式。
    #[test]
    fn the_current_reader_falls_back_to_the_legacy_manifest() {
        let summary =
            read_manifest(&fixture_directory().join("legacy-0.90-trustmark.jpg")).unwrap();

        assert!(summary.legacy_format());
        assert!(!summary.validation_accepted);
        assert_eq!(summary.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
        assert_eq!(summary.watermark_id.as_deref(), Some(FIXTURE_WATERMARK_ID));
    }

    #[test]
    fn a_strict_failure_without_a_legacy_manifest_stays_an_error() {
        let result = read_manifest_with(&fixture_directory().join("source.jpg"), strict_rejection);

        assert!(result.is_err());
    }

    #[test]
    fn current_manifests_that_fail_validation_are_not_downgraded_to_legacy() {
        let summary = read_manifest(&fixture_directory().join("tampered-trustmark.jpg")).unwrap();

        assert!(!summary.validation_accepted);
        assert!(!summary.legacy_format());
    }

    #[test]
    fn current_manifests_are_not_downgraded_to_the_legacy_reader() {
        let summary = read_manifest(&fixture_directory().join("valid-trustmark.jpg")).unwrap();

        assert!(summary.present);
        assert!(summary.validation_accepted);
        assert!(!summary.legacy_format());
        assert_eq!(summary.record_id.as_deref(), Some(FIXTURE_RECORD_ID));
    }

    #[test]
    fn ps256_is_rejected_before_signing() {
        let error = supported_signing_algorithm("PS256").unwrap_err();
        assert!(error.to_string().contains("PS256"));
    }

    #[test]
    fn supported_non_rsa_algorithms_are_accepted() {
        for algorithm in ["es256", "ES384", "ed25519"] {
            supported_signing_algorithm(algorithm).unwrap();
        }
    }

    #[test]
    fn timestamp_wait_has_a_bounded_timeout() {
        let (_sender, receiver) = mpsc::sync_channel(1);
        let cancelled = AtomicBool::new(false);
        let timed_out = AtomicBool::new(false);

        let result = wait_for_timestamp(receiver, &cancelled, &timed_out, Duration::from_millis(5));

        assert!(result.unwrap_err().to_string().contains("time stamp"));
        assert!(timed_out.load(Ordering::SeqCst));
        assert_eq!(TIMESTAMP_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn timestamp_wait_observes_cancellation_before_timeout() {
        let (_sender, receiver) = mpsc::sync_channel(1);
        let cancelled = AtomicBool::new(true);
        let timed_out = AtomicBool::new(false);

        let result = wait_for_timestamp(receiver, &cancelled, &timed_out, Duration::from_secs(1));

        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(!timed_out.load(Ordering::SeqCst));
    }

    /// 畸形输入模糊（C2PA 清单）：把任意字节当成「待识别的图片」交给 `read_manifest`。
    ///
    /// 认证输入来自用户挑选的文件，属于**不受信任的字节**，且是少数「读完即解析第三方
    /// 容器（JPEG + JUMBF）」的入口：解析必须干净地给出「没有清单」或一个错误，绝不 panic。
    ///
    /// 清单解析主要由第三方 `c2pa` crate 承担。这里用 `catch_unwind` 兜底并把种子写进
    /// 失败消息：若它在畸形输入上 panic，那正是要被记录下来的事实（而不是让整个单元
    /// 测试二进制崩在半路、丢掉其余结果），并且可以按种子原样重放。
    #[test]
    fn fuzzed_manifests_are_rejected_without_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("untrusted.jpg");

        for seed in 0..96_u64 {
            let bytes = if seed % 2 == 0 {
                random_bytes(seed, 1 + (seed as usize * 29) % 2048)
            } else {
                // 带常见图片魔数的前缀 + 随机尾巴：更接近「伪装成图片的文件」。
                let mut value = if seed % 4 == 1 {
                    b"\xFF\xD8\xFF\xE0".to_vec()
                } else {
                    b"\x89PNG\r\n\x1A\n".to_vec()
                };
                value.extend_from_slice(&random_bytes(seed, 1 + (seed as usize * 13) % 1024));
                value
            };
            std::fs::write(&path, &bytes).unwrap();

            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_manifest(&path)));
            match outcome {
                Ok(Ok(summary)) => assert!(
                    !summary.present,
                    "seed={seed}：随机字节不可能带有有效的 C2PA 清单"
                ),
                Ok(Err(_)) => {}
                Err(_) => panic!("seed={seed}：解析不受信任的图片时发生 panic"),
            }
        }
    }
}
