mod c2pa;
mod commands;
mod error;
mod image_resource;
mod model;
mod pipeline;
mod publication_repository;
mod repository;
mod scrub;
mod state;
mod trustmark;

pub(crate) use commands::*;
pub(crate) use error::AuthenticityError;
pub(crate) use model::{
    BranchPublication, EnterPublicationRequest, PublishBranchRequest, PublishResult,
};
/// 无头 `publish` / `decode-authenticity` 子命令需要直接装配这些边界 DTO；非无头
/// 构建不使用，因此门控再导出，避免 `cargo check --lib` 出现未使用导入警告。
#[cfg(feature = "headless")]
pub(crate) use model::{CertificationConfig, DecodeRequest, NormalizedRegion};
#[cfg(feature = "headless")]
pub(crate) use pipeline::decode;
pub(crate) use publication_repository::{branch_head, remove_artifact};
pub(crate) use repository::{get_publication, record_branch, remove_record};
pub(crate) use scrub::scrub_controlled_files;
pub(crate) use state::AuthenticityState;
