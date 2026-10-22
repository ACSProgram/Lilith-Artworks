//! 最终成品固化：把用户选择的成品复制进仓库、登记摘要，并在失败路径上回收孤儿文件。
//!
//! 这是领域逻辑：Tauri 适配器与无头入口经 use-case 调用它，不在适配器里复制文件
//! 或登记摘要。发布态的渲染与签名见 `publish`。

use std::{fs, path::Path};

use crate::{cleanup, storage};

use super::{
    file_ops::stage_copy_with_sha256,
    publication_repository::{self, NewFinalArtifact},
    rendition::image_media_type,
};

pub(crate) fn store_final_artifact(
    root: &Path,
    branch_id: &str,
    history_id: &str,
    source_value: &str,
) -> Result<(), String> {
    let source = Path::new(source_value.trim());
    if !source.is_file() {
        return Err("最终成品不存在或不是普通文件".into());
    }
    let source = source
        .canonicalize()
        .map_err(|error| format!("无法访问最终成品：{error}"))?;
    storage::ensure_outside_repository(root, &source, "最终成品")?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = image_media_type(&extension)
        .ok_or_else(|| "最终成品仅支持 PNG、JPEG、WebP 或 TIFF".to_owned())?;
    let artifact_id = storage::new_id();
    let (artwork_id, _) = publication_repository::branch_head(root, branch_id)?;
    let artifact_directory =
        publication_repository::final_artifact_directory(root, &artwork_id, branch_id);
    fs::create_dir_all(&artifact_directory)
        .map_err(|error| format!("无法创建成品目录：{error}"))?;
    let final_path = artifact_directory.join(format!("{artifact_id}.{extension}"));
    let staged = stage_copy_with_sha256(&source, &artifact_directory)
        .map_err(|error| format!("无法保存最终成品：{error}"))?;
    let source_sha256 = staged.sha256.clone();
    let byte_size = staged.bytes;
    let stored_path = storage::relative_path(root, &final_path)?;
    let created_ms = storage::now_ms()?;
    let cleanup_id = {
        let mut connection = storage::open(root)?;
        let transaction = connection.transaction().map_err(storage::database_error)?;
        let id = cleanup::enqueue_repository_file_with_hash(
            &transaction,
            &stored_path,
            &source_sha256,
            "enter_branch_publication",
        )?;
        transaction.commit().map_err(storage::database_error)?;
        id
    };
    if let Err(error) = staged.persist_noclobber(&final_path) {
        let message = match cleanup::discard(root, std::slice::from_ref(&cleanup_id)) {
            Ok(()) => format!("无法发布最终成品：{error}"),
            Err(cleanup_error) => format!(
                "无法发布最终成品：{}；无法撤销清理登记：{}",
                error, cleanup_error
            ),
        };
        return Err(message);
    }
    let artifact = NewFinalArtifact {
        id: &artifact_id,
        branch_id,
        history_id,
        source_path: &stored_path,
        source_sha256: &source_sha256,
        media_type,
        byte_size,
        created_ms,
    };
    if let Err(error) = publication_repository::insert_final_artifact(root, &artifact, &cleanup_id)
    {
        return Err(recover_failed_artifact(root, &cleanup_id, error));
    }
    Ok(())
}

fn recover_failed_artifact(root: &Path, cleanup_id: &str, error: String) -> String {
    match cleanup::run(root, &[cleanup_id.to_owned()]) {
        Ok(report) if report.failures.is_empty() => error,
        Ok(report) => format!(
            "{}；有 {} 个最终成品文件清理失败，将在下次启动时重试",
            error,
            report.failures.len()
        ),
        Err(cleanup_error) => format!("{}；最终成品清理任务失败：{}", error, cleanup_error),
    }
}
