use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use sha2::Digest;
use trustmark::{Trustmark, Variant, Version};

use super::error::{AuthenticityError, AuthenticityResult};

#[derive(Clone)]
pub(crate) struct AuthenticityState {
    models_dir: PathBuf,
    engine: Arc<Mutex<Option<Trustmark>>>,
}

impl AuthenticityState {
    pub(crate) fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            engine: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn model_files_ready(&self) -> bool {
        self.models_dir.join("encoder_Q.onnx").is_file()
            && self.models_dir.join("decoder_Q.onnx").is_file()
    }

    pub(crate) fn with_engine<T>(
        &self,
        operation: impl FnOnce(&Trustmark) -> AuthenticityResult<T>,
    ) -> AuthenticityResult<T> {
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| AuthenticityError::Task("TrustMark 模型状态锁已损坏".into()))?;
        if guard.is_none() {
            *guard = Some(Trustmark::new(
                &self.models_dir,
                Variant::Q,
                Version::BchSuper,
            )?);
        }
        operation(guard.as_ref().expect("TrustMark engine initialized"))
    }

    pub(crate) fn model_info(&self) -> (String, Option<String>, Option<String>) {
        (
            super::trustmark::MODEL_VARIANT.to_owned(),
            file_sha256(&self.models_dir.join("encoder_Q.onnx")),
            file_sha256(&self.models_dir.join("decoder_Q.onnx")),
        )
    }
}

fn file_sha256(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode_upper(sha2::Sha256::digest(bytes)))
}
