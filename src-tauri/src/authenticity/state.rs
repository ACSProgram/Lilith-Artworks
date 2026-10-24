use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use sha2::Digest;
use trustmark::{Trustmark, Variant, Version};

use super::error::{AuthenticityError, AuthenticityResult};

/// 随包 TrustMark Q/BCH_SUPER 模型的 SHA-256（大写十六进制），与
/// `src-tauri/resources/models/README.md` 记录的值一致。
///
/// 加载前比对：安装目录（或用户可写的任何落点）里的 `*_Q.onnx` 被替换后，不比对摘要
/// 就会静默加载被换掉的模型，直接削弱应用自身宣称的认证可信度。需要本地写权限，风险
/// 低，但这是一个便宜且明确的硬化点。**换模型时必须同步更新这两个常量与 README**，
/// 否则发布产物会拒绝加载自己的模型。
const ENCODER_Q_SHA256: &str = "19B3D1B25836130FFD78775A8F61539F993375D1823EF0E59BA5B8DFFB4F892D";
const DECODER_Q_SHA256: &str = "EE3268F057C9DABEF680E169302F5973D0589FEEA86189ED229A896CC3AA88DF";

/// 一次模型检查的结论：摘要用于展示，`error` 为 `Some` 时模型不可用。
///
/// 结果按 `AuthenticityState` 缓存：两个模型合计约 62 MB，每次读取发布状态都重新
/// 计算摘要没有意义。缓存也意味着运行期间替换模型文件不会在本次会话内被发现——
/// 那需要重启应用，这与引擎只初始化一次的语义一致。
struct ModelStatus {
    encoder_sha256: Option<String>,
    decoder_sha256: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct AuthenticityState {
    models_dir: PathBuf,
    engine: Arc<Mutex<Option<Trustmark>>>,
    operation: Arc<Mutex<Option<ActiveOperation>>>,
    model_status: Arc<OnceLock<ModelStatus>>,
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
            model_status: Arc::new(OnceLock::new()),
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

    /// 模型是否可用：两个文件都存在**且**摘要与随包记录一致。
    ///
    /// 只判存在会让 UI 显示「就绪」，而真正加载时才失败；这里与
    /// [`Self::with_engine`] 共用同一次检查的结论。
    pub(crate) fn model_files_ready(&self) -> bool {
        self.model_status().error.is_none()
    }

    pub(crate) fn with_engine<T>(
        &self,
        operation: impl FnOnce(&Trustmark) -> AuthenticityResult<T>,
    ) -> AuthenticityResult<T> {
        if let Some(error) = self.model_status().error.as_deref() {
            // 用 `Other` 而不是 `Task`：这是一条可直接行动的结论，不该被渲染成
            // 「后台任务失败」。
            return Err(anyhow::Error::msg(error.to_owned()).into());
        }
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
        let status = self.model_status();
        (
            super::trustmark::MODEL_VARIANT.to_owned(),
            status.encoder_sha256.clone(),
            status.decoder_sha256.clone(),
        )
    }

    fn model_status(&self) -> &ModelStatus {
        self.model_status
            .get_or_init(|| inspect_models(&self.models_dir))
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

fn inspect_models(models_dir: &Path) -> ModelStatus {
    let encoder_sha256 = file_sha256(&models_dir.join("encoder_Q.onnx"));
    let decoder_sha256 = file_sha256(&models_dir.join("decoder_Q.onnx"));
    let error = match (encoder_sha256.as_deref(), decoder_sha256.as_deref()) {
        (None, _) => Some("缺少 TrustMark 编码器模型 encoder_Q.onnx，请重新安装应用".to_owned()),
        (_, None) => Some("缺少 TrustMark 解码器模型 decoder_Q.onnx，请重新安装应用".to_owned()),
        (Some(encoder), Some(decoder)) => {
            if !encoder.eq_ignore_ascii_case(ENCODER_Q_SHA256) {
                Some(
                    "TrustMark 编码器模型已被替换或损坏（SHA-256 与随包记录不匹配），已拒绝加载"
                        .to_owned(),
                )
            } else if !decoder.eq_ignore_ascii_case(DECODER_Q_SHA256) {
                Some(
                    "TrustMark 解码器模型已被替换或损坏（SHA-256 与随包记录不匹配），已拒绝加载"
                        .to_owned(),
                )
            } else {
                None
            }
        }
    };
    ModelStatus {
        encoder_sha256,
        decoder_sha256,
        error,
    }
}

fn file_sha256(path: &std::path::Path) -> Option<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha2::Sha256::new();
    // Windows 的主线程栈只有 1 MiB，1 MiB 的栈缓冲会让发布入口直接栈溢出。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(hex::encode_upper(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{inspect_models, AuthenticityState, DECODER_Q_SHA256, ENCODER_Q_SHA256};

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

    #[test]
    fn missing_models_are_not_ready_and_refuse_to_load() {
        let directory = tempfile::tempdir().unwrap();
        let state = AuthenticityState::new(directory.path().to_path_buf());

        assert!(!state.model_files_ready());
        let error = state.with_engine(|_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("缺少 TrustMark"), "{error}");
    }

    #[test]
    fn replaced_models_are_refused_before_loading() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("encoder_Q.onnx"), b"replacement").unwrap();
        fs::write(directory.path().join("decoder_Q.onnx"), b"replacement").unwrap();
        let state = AuthenticityState::new(directory.path().to_path_buf());

        // 文件存在但摘要不匹配：就绪状态与加载都必须拒绝，并说明是哪个模型。
        assert!(!state.model_files_ready());
        let error = state.with_engine(|_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("编码器模型"), "{error}");
        let (_, encoder, decoder) = state.model_info();
        assert!(encoder.is_some());
        assert!(decoder.is_some());
    }

    /// 固化的摘要必须与随仓库分发的模型一致：换模型时忘了更新常量，会在提交前
    /// 而不是发布后暴露。模型未随仓库分发时直接跳过。
    #[test]
    fn pinned_digests_match_the_shipped_models() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("models");
        if !directory.join("encoder_Q.onnx").is_file() {
            return;
        }

        let status = inspect_models(&directory);

        assert!(status.error.is_none(), "{:?}", status.error);
        assert_eq!(status.encoder_sha256.as_deref(), Some(ENCODER_Q_SHA256));
        assert_eq!(status.decoder_sha256.as_deref(), Some(DECODER_Q_SHA256));
    }
}
