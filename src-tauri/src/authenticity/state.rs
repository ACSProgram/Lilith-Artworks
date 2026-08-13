use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

    pub(crate) fn models_dir(&self) -> &Path {
        &self.models_dir
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
            *guard = Some(Trustmark::new(&self.models_dir, Variant::Q, Version::Bch5)?);
        }
        operation(guard.as_ref().expect("TrustMark engine initialized"))
    }
}
