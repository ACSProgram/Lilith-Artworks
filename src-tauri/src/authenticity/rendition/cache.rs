//! 预览渲染缓存：会话目录、缓存校验与淘汰。
//!
//! 缓存不是正确性路径：文件缺失或校验失败时调用方重新渲染，删除失败静默忽略。

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

use crate::storage;

use super::super::{
    error::{AuthenticityError, AuthenticityResult},
    file_ops::sha256_file_cancelable,
    state::AuthenticityOperation,
};
use super::ensure_not_cancelled;

static PREVIEW_CACHE_SESSION: OnceLock<String> = OnceLock::new();

/// 会话内预览缓存的份数上限。
///
/// 每换一次参数（JPEG 质量、背景色、水印强度、框选区域、水印 ID）就留下一份全尺寸
/// JPEG 与一份元数据；常见调参会有十几次迭代，16 份足够来回比较几组参数，又不会
/// 让一次会话堆出几十份预览。当前正在使用的那份总是最新的，因此天然落在保留区间。
const PREVIEW_CACHE_MAX_ENTRIES: usize = 16;

/// 会话内预览缓存的 JPEG 总字节上限（512 MiB）。
///
/// 普通画布单份缓存只有几 MB，份数上限通常先生效；这个上限兜住超大画布连续调参
/// 的情况（16K 成品单份就可能上百 MB）。超限时继续从最旧的删，直到不超或只剩一份。
/// 同名元数据 JSON 只有几百字节，不计入这里统计的总量。
const PREVIEW_CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// 当前进程的预览缓存会话 id；本进程尚未写过任何质量预览缓存时为 `None`。
/// 仓库临时目录清扫用它保留仍属于本进程的会话目录。
pub(crate) fn preview_session_id() -> Option<&'static str> {
    PREVIEW_CACHE_SESSION.get().map(String::as_str)
}

fn preview_cache_directory(root: &Path) -> AuthenticityResult<PathBuf> {
    let session = PREVIEW_CACHE_SESSION.get_or_init(storage::new_id);
    let directory = root
        .join("temp")
        .join(format!("authenticity-preview-{session}"));
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

pub(crate) fn cache_paths(root: &Path, token: &str) -> AuthenticityResult<(PathBuf, PathBuf)> {
    let directory = preview_cache_directory(root)?;
    Ok((
        directory.join(format!("{token}.jpg")),
        directory.join(format!("{token}.json")),
    ))
}

#[derive(Deserialize, Serialize)]
pub(crate) struct RenditionCacheMetadata {
    pub(crate) token: String,
    pub(crate) source_sha256: String,
    pub(crate) jpeg_sha256: String,
    pub(crate) watermark_id: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_bytes: u64,
}

pub(crate) struct CachedRendition {
    pub(crate) path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_bytes: u64,
    pub(crate) cache_hit: bool,
    pub(crate) render_ms: u64,
    pub(crate) encode_ms: u64,
}

pub(super) fn load_cached_rendition(
    root: &Path,
    operation: &AuthenticityOperation,
    token: &str,
    source_sha256: &str,
    watermark_id: Option<&str>,
) -> AuthenticityResult<Option<CachedRendition>> {
    let (jpeg_path, metadata_path) = cache_paths(root, token)?;
    if !jpeg_path.is_file() || !metadata_path.is_file() {
        return Ok(None);
    }
    let loaded = (|| -> AuthenticityResult<CachedRendition> {
        ensure_not_cancelled(operation)?;
        let metadata: RenditionCacheMetadata =
            serde_json::from_reader(File::open(&metadata_path)?)?;
        if metadata.token != token
            || !metadata.source_sha256.eq_ignore_ascii_case(source_sha256)
            || metadata.watermark_id.as_deref() != watermark_id
            || metadata.output_bytes != fs::metadata(&jpeg_path)?.len()
            || !sha256_file_cancelable(&jpeg_path, operation)?
                .eq_ignore_ascii_case(&metadata.jpeg_sha256)
        {
            return Err(AuthenticityError::Task("发布预览缓存校验失败".into()));
        }
        let dimensions = image::ImageReader::open(&jpeg_path)?
            .with_guessed_format()?
            .into_dimensions()?;
        if dimensions != (metadata.width, metadata.height) {
            return Err(AuthenticityError::Task("发布预览缓存尺寸不匹配".into()));
        }
        Ok(CachedRendition {
            path: jpeg_path.clone(),
            width: metadata.width,
            height: metadata.height,
            output_bytes: metadata.output_bytes,
            cache_hit: true,
            render_ms: 0,
            encode_ms: 0,
        })
    })();
    match loaded {
        Ok(cached) => Ok(Some(cached)),
        Err(_) => {
            let _ = fs::remove_file(jpeg_path);
            let _ = fs::remove_file(metadata_path);
            Ok(None)
        }
    }
}

pub(super) fn replace_cache_file(
    temp: tempfile::NamedTempFile,
    destination: &Path,
) -> AuthenticityResult<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    temp.persist(destination)
        .map_err(|error| AuthenticityError::Io(error.error))?;
    Ok(())
}

/// 会话内缓存淘汰：按 JPEG 修改时间从旧到新，先删到只剩最近
/// [`PREVIEW_CACHE_MAX_ENTRIES`] 份，再在 JPEG 总字节超过 [`PREVIEW_CACHE_MAX_BYTES`]
/// 时继续删最旧的，直到不超或只剩一份。
///
/// 每次写入新的缓存对之后调用。删除成功与否都不影响预览与发布结果——缓存不是
/// 正确性路径，失败静默忽略，最多让目录多留几份等下次淘汰或会话结束清扫。
pub(super) fn prune_preview_cache(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut cached = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jpg") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        cached.push((modified, metadata.len(), path));
    }
    let mut jpeg_bytes: u64 = cached.iter().map(|(_, bytes, _)| *bytes).sum();
    if cached.len() <= PREVIEW_CACHE_MAX_ENTRIES && jpeg_bytes <= PREVIEW_CACHE_MAX_BYTES {
        return;
    }
    cached.sort_by_key(|(modified, _, _)| *modified);
    let mut remaining = cached.len();
    for (_, bytes, jpeg_path) in cached {
        // 至少留一份：正在使用的那份总是最新，走到这里只会剩下最旧的一份。
        if remaining <= 1 {
            break;
        }
        if remaining <= PREVIEW_CACHE_MAX_ENTRIES && jpeg_bytes <= PREVIEW_CACHE_MAX_BYTES {
            break;
        }
        let _ = fs::remove_file(&jpeg_path);
        let _ = fs::remove_file(jpeg_path.with_extension("json"));
        jpeg_bytes = jpeg_bytes.saturating_sub(bytes);
        remaining -= 1;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn preview_cache_pruning_keeps_the_latest_entries_and_removes_metadata_pairs() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("authenticity-preview-test");
        fs::create_dir(&cache).unwrap();
        for index in 0..20 {
            let jpeg = cache.join(format!("{index:02}.jpg"));
            let metadata = cache.join(format!("{index:02}.json"));
            fs::write(&jpeg, b"jpeg").unwrap();
            fs::write(&metadata, b"{}").unwrap();
            let stamp = filetime::FileTime::from_unix_time(1_700_000_000 + index, 0);
            filetime::set_file_mtime(&jpeg, stamp).unwrap();
        }

        prune_preview_cache(&cache);

        let mut names: Vec<String> = fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = (4..20)
            .flat_map(|index| [format!("{index:02}.jpg"), format!("{index:02}.json")])
            .collect();
        expected.sort();
        // 最旧的 4 份及其元数据成对删除，最近 16 份原样保留。
        assert_eq!(names, expected);
    }
}
