use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuthenticityError {
    #[error("参数无效：{0}")]
    InvalidInput(String),
    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("图片处理失败：{0}")]
    Image(#[from] image::ImageError),
    #[error("TrustMark 处理失败：{0}")]
    Trustmark(#[from] trustmark::Error),
    #[error("C2PA 处理失败：{0}")]
    C2pa(#[from] c2pa::Error),
    #[error("序列化失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("后台任务失败：{0}")]
    Task(String),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl Serialize for AuthenticityError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub(crate) type AuthenticityResult<T> = Result<T, AuthenticityError>;
