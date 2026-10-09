use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedRegion {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnterPublicationRequest {
    pub(crate) branch_id: String,
    pub(crate) artifact_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CertificationConfig {
    pub(crate) branch_id: String,
    pub(crate) title: String,
    pub(crate) creator: String,
    pub(crate) rights_statement: String,
    pub(crate) authentication_content: String,
    pub(crate) trustmark_enabled: bool,
    pub(crate) certificate_path: String,
    pub(crate) signing_algorithm: String,
    pub(crate) timestamp_url: Option<String>,
    pub(crate) jpeg_quality: u8,
    pub(crate) background_color: String,
    pub(crate) watermark_strength: f32,
    pub(crate) additional_regions: Vec<NormalizedRegion>,
    pub(crate) updated_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishBranchRequest {
    pub(crate) branch_id: String,
    pub(crate) output_path: String,
    pub(crate) private_key_pem: String,
    pub(crate) config: CertificationConfig,
    pub(crate) watermark_id: Option<String>,
    pub(crate) preview_cache_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicationPreviewRequest {
    pub(crate) branch_id: String,
    pub(crate) config: CertificationConfig,
    pub(crate) watermark_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportCertificationRecordRequest {
    pub(crate) record_id: String,
    pub(crate) output_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeRequest {
    pub(crate) input_path: String,
    pub(crate) region: Option<NormalizedRegion>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewImage {
    pub(crate) data_url: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) source_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicationPreview {
    pub(crate) branch_id: String,
    pub(crate) image: PreviewImage,
    pub(crate) original_image: PreviewImage,
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) output_bytes: u64,
    pub(crate) watermark_id: Option<String>,
    pub(crate) cache_token: String,
    pub(crate) cache_hit: bool,
    pub(crate) render_ms: u64,
    pub(crate) encode_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EstimateRequest {
    pub(crate) branch_id: String,
    pub(crate) jpeg_quality: u8,
    pub(crate) background_color: String,
}

/// 高清局部的像素来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PreviewTileSource {
    /// 缓存的无签名 JPEG（导出图预览）。
    Compressed,
    /// 分支的仓库最终成品（原始成品对比）。
    Original,
    /// 外部图片（识别页待识别图片的框选放大镜）。
    External,
}

/// 高清局部请求：在源分辨率坐标系内裁剪一块区域。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewTileRequest {
    pub(crate) source: PreviewTileSource,
    #[serde(default)]
    pub(crate) cache_token: Option<String>,
    #[serde(default)]
    pub(crate) branch_id: Option<String>,
    /// 外部源的绝对路径；仅 `External` 使用，且必须已由文件选择器或拖放导入授权。
    #[serde(default)]
    pub(crate) path: Option<String>,
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) max_edge: u32,
}

/// 拖放导入的待识别图片：前端读取的原始字节（Base64 编码）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageImportRequest {
    pub(crate) file_name: String,
    pub(crate) data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileSizeEstimate {
    pub(crate) jpeg_bytes: u64,
    pub(crate) source_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidationItem {
    pub(crate) code: String,
    pub(crate) explanation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CertificationRecord {
    pub(crate) id: String,
    pub(crate) artwork_id: String,
    pub(crate) artwork_title: String,
    pub(crate) branch_id: String,
    pub(crate) branch_title: String,
    pub(crate) history_id: String,
    pub(crate) watermark_id: Option<String>,
    pub(crate) trustmark_enabled: bool,
    pub(crate) output_path: String,
    pub(crate) output_sha256: String,
    pub(crate) output_bytes: u64,
    pub(crate) title: String,
    pub(crate) creator: String,
    pub(crate) rights_statement: String,
    pub(crate) authentication_content: String,
    pub(crate) additional_regions: Vec<NormalizedRegion>,
    pub(crate) c2pa_manifest_label: Option<String>,
    pub(crate) c2pa_manifest_json: Option<String>,
    pub(crate) validation_state: Option<String>,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FinalArtifact {
    pub(crate) id: String,
    pub(crate) branch_id: String,
    pub(crate) history_id: String,
    pub(crate) source_path: String,
    pub(crate) source_sha256: String,
    pub(crate) media_type: String,
    pub(crate) byte_size: u64,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BranchPublication {
    pub(crate) branch_id: String,
    pub(crate) artifact: Option<FinalArtifact>,
    pub(crate) config: CertificationConfig,
    pub(crate) records: Vec<CertificationRecord>,
    pub(crate) models_ready: bool,
    pub(crate) model_variant: String,
    pub(crate) encoder_sha256: Option<String>,
    pub(crate) decoder_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishResult {
    pub(crate) record: CertificationRecord,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) watermark_region_count: u32,
    pub(crate) rendition_cache_hit: bool,
    pub(crate) render_ms: u64,
    pub(crate) encode_ms: u64,
    pub(crate) signing_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeResult {
    pub(crate) watermark_present: bool,
    pub(crate) watermark_id: Option<String>,
    pub(crate) decoded_region: Option<NormalizedRegion>,
    pub(crate) c2pa_present: bool,
    pub(crate) c2pa_validation_state: Option<String>,
    pub(crate) c2pa_validation_status: Vec<ValidationItem>,
    pub(crate) c2pa_record_id: Option<String>,
    pub(crate) c2pa_watermark_id: Option<String>,
    pub(crate) identifiers_match: Option<bool>,
    pub(crate) title: Option<String>,
    pub(crate) creator: Option<String>,
    pub(crate) rights_statement: Option<String>,
    pub(crate) authentication_content: Option<String>,
    pub(crate) manifest_json: Option<String>,
    pub(crate) matches: Vec<CertificationMatch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CertificationMatch {
    pub(crate) record: CertificationRecord,
    pub(crate) evidence_sources: Vec<String>,
}

pub(crate) struct ManifestSummary {
    pub(crate) present: bool,
    pub(crate) validation_accepted: bool,
    pub(crate) validation_state: Option<String>,
    pub(crate) validation_status: Vec<ValidationItem>,
    pub(crate) record_id: Option<String>,
    pub(crate) watermark_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) creator: Option<String>,
    pub(crate) rights_statement: Option<String>,
    pub(crate) authentication_content: Option<String>,
    pub(crate) manifest_json: Option<String>,
}
