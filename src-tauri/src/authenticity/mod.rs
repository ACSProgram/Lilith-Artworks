mod artifact;
mod c2pa;
mod commands;
mod contracts;
mod decode;
pub(crate) mod drop_stage;
mod error;
mod estimate;
mod export;
mod file_ops;
mod image_resource;
mod model;
mod preview;
mod publication_repository;
mod publish;
mod rendition;
mod repository;
mod scrub;
mod state;
#[cfg(test)]
pub(crate) mod test_support;
mod trustmark;

pub(crate) use artifact::store_final_artifact;
pub(crate) use commands::*;
pub(crate) use contracts::ArtifactReferenceInventory;
pub(crate) use decode::decode;
pub(crate) use error::AuthenticityError;
pub(crate) use estimate::estimate_output_size;
pub(crate) use export::export_record;
pub(crate) use model::{
    BranchPublication, DecodeRequest, DecodeResult, EnterPublicationRequest, EstimateRequest,
    ExportCertificationRecordRequest, FileSizeEstimate, PublicationPreview,
    PublicationPreviewRequest, PublishBranchRequest, PublishResult,
};
/// 无头 `publish` 子命令需要直接装配这些边界 DTO；非无头构建不使用，因此门控再导出，
/// 避免 `cargo check --lib` 出现未使用导入警告。
#[cfg(feature = "headless")]
pub(crate) use model::{CertificationConfig, NormalizedRegion};
pub(crate) use preview::preview_publication;
pub(crate) use publication_repository::{branch_head, remove_artifact};
pub(crate) use publish::publish;
pub(crate) use rendition::preview_session_id;
pub(crate) use repository::{get_publication, record_branch, record_source_path, remove_record};
pub(crate) use scrub::scrub_controlled_files;
pub(crate) use state::AuthenticityState;
