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
    assertions::SoftBinding, create_signer, crypto::raw_signature::SigningAlg, Builder,
    BuilderIntent, ClaimGeneratorInfo, Context, Reader, Signer, ValidationState,
};
use serde_json::{json, Value};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    model::{CertificationConfig, ManifestSummary, ValidationItem},
    trustmark::{identifier_from_soft_binding, soft_binding_value, SOFT_BINDING_ALGORITHM},
};

pub(crate) const ASSERTION_LABEL: &str = "com.lilith.artworks.claim";
const TIMESTAMP_TIMEOUT: Duration = Duration::from_secs(30);
const CANCELLATION_POLL: Duration = Duration::from_millis(100);

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
    let definition = json!({
        "title": config.title,
        "format": "image/jpeg",
        "claim_generator_info": [{
            "name": "Lilith Artworks",
            "version": env!("CARGO_PKG_VERSION")
        }]
    });
    let mut builder = Builder::default().with_definition(definition.to_string())?;
    builder.definition.claim_version = Some(2);
    builder.set_intent(BuilderIntent::Edit);
    let mut generator = ClaimGeneratorInfo::new("Lilith Artworks");
    generator.set_version(env!("CARGO_PKG_VERSION"));
    builder.set_claim_generator_info(generator);

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
                            "identifier": format!("lilith-trustmark-region-{}", index + 1),
                            "role": "c2pa.watermarked"
                        }
                    },
                    "value": soft_binding_value(identifier)
                })
            })
            .collect::<Vec<_>>();
        let soft_binding: SoftBinding = serde_json::from_value(json!({
            "alg": SOFT_BINDING_ALGORITHM,
            "blocks": binding_blocks,
            "name": "Lilith Artworks durable identifier",
            "alg-params": "variant=Q;schema=BCH_SUPER;payload=binary"
        }))?;
        builder.add_assertion(SoftBinding::LABEL, &soft_binding)?;
    }

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
        image_format_hint(source_path)?,
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

fn image_format_hint(path: &Path) -> AuthenticityResult<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "tif" | "tiff" => Ok("image/tiff"),
        _ => Err(AuthenticityError::InvalidInput(
            "C2PA ingredient 仅支持 PNG、JPEG、WebP 或 TIFF 原图".into(),
        )),
    }
}

pub(crate) fn read_manifest(path: &Path) -> AuthenticityResult<ManifestSummary> {
    let reader = match Reader::default().with_file(path) {
        Ok(reader) => reader,
        Err(c2pa::Error::JumbfNotFound | c2pa::Error::JumbfBoxNotFound) => {
            return Ok(ManifestSummary {
                present: false,
                validation_accepted: false,
                validation_state: None,
                validation_status: Vec::new(),
                record_id: None,
                watermark_id: None,
                title: None,
                creator: None,
                rights_statement: None,
                authentication_content: None,
                manifest_json: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
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
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .map(str::to_owned);
    let watermark_id = soft_binding
        .and_then(|binding| binding.blocks.first().map(|block| block.value.clone()))
        .and_then(|value| identifier_from_soft_binding(&value))
        .or_else(|| {
            claim
                .as_ref()
                .and_then(|value| value.get("watermarkId"))
                .and_then(Value::as_str)
                .filter(|value| {
                    value.len() == super::trustmark::WATERMARK_BITS
                        && value.bytes().all(|byte| matches!(byte, b'0' | b'1'))
                })
                .map(str::to_owned)
        });
    Ok(ManifestSummary {
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
    })
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

    use super::{supported_signing_algorithm, wait_for_timestamp, TIMESTAMP_TIMEOUT};

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
}
