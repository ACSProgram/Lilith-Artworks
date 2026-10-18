//! 最终成品的固化与发布编排。
//!
//! `store_final_artifact` 把用户选择的成品复制进仓库、登记摘要，并在失败路径上把
//! 已登记的文件入队清理；`publish_artifact` 在调用方取得的认证操作（`AuthenticityOperation`）
//! 范围内完成渲染、编码与签名。两者都是领域逻辑：Tauri 适配器与无头入口经 use-case
//! 调用它们，不在适配器里复制文件或登记摘要。

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use sha2::Digest;
use tempfile::NamedTempFile;

use crate::{cleanup, storage};

use super::{
    error::AuthenticityError,
    model::{PublishBranchRequest, PublishResult},
    pipeline,
    publication_repository::{self, NewFinalArtifact},
    state::{AuthenticityOperation, AuthenticityState},
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
    let media_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        _ => return Err("最终成品仅支持 PNG、JPEG、WebP 或 TIFF".into()),
    };
    let artifact_id = storage::new_id();
    let artifact_directory = root
        .join("artworks")
        .join(publication_repository::branch_head(root, branch_id)?.0)
        .join("artifacts")
        .join(branch_id);
    fs::create_dir_all(&artifact_directory)
        .map_err(|error| format!("无法创建成品目录：{error}"))?;
    let final_path = artifact_directory.join(format!("{artifact_id}.{extension}"));
    let mut source_file =
        File::open(&source).map_err(|error| format!("无法读取最终成品：{error}"))?;
    let mut temp = NamedTempFile::new_in(&artifact_directory)
        .map_err(|error| format!("无法创建成品临时文件：{error}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| format!("无法读取最终成品：{error}"))?;
        if read == 0 {
            break;
        }
        temp.write_all(&buffer[..read])
            .map_err(|error| format!("无法写入最终成品：{error}"))?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("无法同步最终成品：{error}"))?;
    let source_sha256 = hex::encode_upper(hasher.finalize());
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
    if let Err(error) = temp.persist_noclobber(&final_path) {
        let message = match cleanup::discard(root, std::slice::from_ref(&cleanup_id)) {
            Ok(()) => format!("无法发布最终成品：{}", error.error),
            Err(cleanup_error) => format!(
                "无法发布最终成品：{}；无法撤销清理登记：{}",
                error.error, cleanup_error
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
        byte_size: size,
        created_ms,
    };
    if let Err(error) = publication_repository::insert_final_artifact(root, &artifact, &cleanup_id)
    {
        return Err(recover_failed_artifact(root, &cleanup_id, error));
    }
    Ok(())
}

pub(crate) fn publish_artifact(
    root: &Path,
    state: &AuthenticityState,
    operation: &AuthenticityOperation,
    request: PublishBranchRequest,
) -> Result<PublishResult, AuthenticityError> {
    let published = pipeline::publish(root, state, operation, request)?;
    Ok(PublishResult {
        record: published.record,
        width: published.width,
        height: published.height,
        watermark_region_count: published.watermark_region_count,
        watermark_warnings: published.watermark_warnings,
        rendition_cache_hit: published.rendition_cache_hit,
        render_ms: published.render_ms,
        encode_ms: published.encode_ms,
        signing_ms: published.signing_ms,
    })
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
