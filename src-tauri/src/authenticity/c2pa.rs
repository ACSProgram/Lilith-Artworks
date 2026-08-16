use std::{
    fs::{self, File},
    path::Path,
    str::FromStr,
};

use c2pa::{
    assertions::SoftBinding, create_signer, crypto::raw_signature::SigningAlg, Builder,
    BuilderIntent, ClaimGeneratorInfo, Reader, ValidationState,
};
use serde_json::{json, Value};

use super::{
    error::{AuthenticityError, AuthenticityResult},
    model::{CertificationConfig, ManifestSummary, ValidationItem},
    trustmark::{identifier_from_soft_binding, soft_binding_value, SOFT_BINDING_ALGORITHM},
};

pub(crate) const ASSERTION_LABEL: &str = "com.lilith.artworks.claim";

pub(crate) fn sign_jpeg(
    config: &CertificationConfig,
    private_key: &[u8],
    record_id: &str,
    identifier: &str,
    source_path: &Path,
    unsigned_jpeg: &Path,
    signed_jpeg: &Path,
) -> AuthenticityResult<()> {
    let algorithm = SigningAlg::from_str(
        config
            .signing_algorithm
            .trim()
            .to_ascii_lowercase()
            .as_str(),
    )
    .map_err(|_| AuthenticityError::InvalidInput("不支持的 C2PA 签名算法".into()))?;
    let certificate = fs::read(&config.certificate_path)?;
    let timestamp_url = config
        .timestamp_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let signer = create_signer::from_keys(&certificate, private_key, algorithm, timestamp_url)?;
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
    builder.sign_file(signer.as_ref(), unsigned_jpeg, signed_jpeg)?;
    Ok(())
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
