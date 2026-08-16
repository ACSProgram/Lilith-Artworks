use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use sha2::Digest;
use trustmark::{Trustmark, Variant, Version};

use super::error::{AuthenticityError, AuthenticityResult};

#[derive(Clone)]
pub(crate) struct AuthenticityState {
    models_dir: PathBuf,
    engine: Arc<Mutex<Option<Trustmark>>>,
    operation: Arc<Mutex<Option<ActiveOperation>>>,
}

struct ActiveOperation {
    label: &'static str,
    cancelled: Arc<AtomicBool>,
}

pub(crate) struct AuthenticityOperation {
    operation: Arc<Mutex<Option<ActiveOperation>>>,
    cancelled: Arc<AtomicBool>,
}

impl AuthenticityState {
    pub(crate) fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            engine: Arc::new(Mutex::new(None)),
            operation: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn begin_operation(
        &self,
        label: &'static str,
    ) -> AuthenticityResult<AuthenticityOperation> {
        let mut active = self
            .operation
            .lock()
            .map_err(|_| AuthenticityError::Task("认证任务状态锁已损坏".into()))?;
        if let Some(operation) = active.as_ref() {
            return Err(AuthenticityError::Task(format!(
                "{}仍在进行，请等待完成或先取消",
                operation.label
            )));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(ActiveOperation {
            label,
            cancelled: cancelled.clone(),
        });
        Ok(AuthenticityOperation {
            operation: self.operation.clone(),
            cancelled,
        })
    }

    pub(crate) fn request_cancel(&self) -> AuthenticityResult<bool> {
        let active = self
            .operation
            .lock()
            .map_err(|_| AuthenticityError::Task("认证任务状态锁已损坏".into()))?;
        let Some(operation) = active.as_ref() else {
            return Ok(false);
        };
        operation.cancelled.store(true, Ordering::SeqCst);
        Ok(true)
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

impl AuthenticityOperation {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn cancellation_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl Drop for AuthenticityOperation {
    fn drop(&mut self) {
        if let Ok(mut active) = self.operation.lock() {
            if active
                .as_ref()
                .is_some_and(|operation| Arc::ptr_eq(&operation.cancelled, &self.cancelled))
            {
                *active = None;
            }
        }
    }
}

fn file_sha256(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode_upper(sha2::Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::AuthenticityState;

    #[test]
    fn operation_cancellation_is_scoped_to_the_active_guard() {
        let state = AuthenticityState::new(std::path::PathBuf::new());
        let operation = state.begin_operation("质量预览").unwrap();
        assert!(!operation.cancelled());
        assert!(state.request_cancel().unwrap());
        assert!(operation.cancelled());
        assert!(state.begin_operation("签名发布").is_err());

        drop(operation);
        assert!(!state.request_cancel().unwrap());
        let next = state.begin_operation("签名发布").unwrap();
        assert!(!next.cancelled());
    }
}
