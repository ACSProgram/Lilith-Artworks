//! 高清局部（tile）：按源分辨率裁剪并编码为无损 PNG。
//!
//! 服务发布预览的 1:1 观察与框选放大镜；压缩源、分支成品与外部图片三个通道都
//! 在这里校验令牌、矩形与仓库边界。解码结果按「路径 + 文件长度」缓存最近一次
//! 源图，避免平移时反复解码整图。

use std::{
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::GenericImageView;

use super::super::{
    error::{AuthenticityError, AuthenticityResult},
    image_resource,
    model::{PreviewImage, PreviewTileRequest, PreviewTileSource},
    publication_repository,
    rendition::{cache_paths, RenditionCacheMetadata},
};
use crate::storage;

/// 高清局部（tile）允许的最大输出边长。
///
/// 局部要支撑「一源像素对一屏幕像素」的 1:1 观察，输出边长必须能覆盖可视区域
/// 在屏幕上的显示尺寸（含边距），因此不再与 2400 px 缩略图上限对齐。
pub(crate) const TILE_MAX_EDGE: u32 = 4096;
pub(crate) const TILE_MIN_EDGE: u32 = 64;

/// 单个解码源图像进入常驻缓存的像素上限（约 64 MP，约 192 MB RGB）。
const TILE_SOURCE_CACHE_MAX_PIXELS: u64 = 64 * 1024 * 1024;

struct CachedTileSource {
    key: String,
    image: Arc<image::DynamicImage>,
}

static TILE_SOURCE_CACHE: OnceLock<Mutex<Option<CachedTileSource>>> = OnceLock::new();

fn tile_source_cache() -> &'static Mutex<Option<CachedTileSource>> {
    TILE_SOURCE_CACHE.get_or_init(|| Mutex::new(None))
}

/// 打开高清局部的像素来源，并缓存最近一次解码结果，避免平移时反复解码整图。
///
/// 缓存以「路径 + 文件长度」为键：预览缓存与分支成品在会话内都不可变，因此键
/// 变化即意味着源已更换。超过像素上限的源不缓存，仍按次解码。
fn open_tile_source(path: &Path) -> AuthenticityResult<Arc<image::DynamicImage>> {
    let key = format!("{}:{}", path.display(), fs::metadata(path)?.len());
    let cache = tile_source_cache();
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.as_ref() {
            if cached.key == key {
                return Ok(Arc::clone(&cached.image));
            }
        }
    }
    let image = image_resource::open(path)?;
    let pixels = u64::from(image.width()) * u64::from(image.height());
    let shared = Arc::new(image);
    if pixels <= TILE_SOURCE_CACHE_MAX_PIXELS {
        if let Ok(mut guard) = cache.lock() {
            *guard = Some(CachedTileSource {
                key,
                image: Arc::clone(&shared),
            });
        }
    }
    Ok(shared)
}

/// 从质量预览缓存的无签名 JPEG、分支最终成品或已授权的外部图片中裁剪一块高清区域。
///
/// 压缩源要求令牌完整命中本进程会话的缓存文件名，并按缓存元数据的像素尺寸
/// 校验；原始源按分支解析受控成品路径；外部源按绝对路径解析，先做仓库边界校验
/// （路径授权检查在命令层与 `decode_authenticity` 同源），因此框选放大镜不会
/// 放宽任何安全边界。裁剪矩形按源像素尺寸校验，解码复用统一的资源预算，
/// 输出按 `max_edge` 只缩不放，供界面在缩略图分辨率不足时叠加显示。
///
/// 局部统一编码为无损 PNG：预览的用途就是观察 JPEG 与 TrustMark 造成的损失，
/// 若再按有损 JPEG 回写一次，界面会额外叠加一层并非成品本身带来的压缩痕迹。
pub(crate) fn preview_tile(
    root: &Path,
    request: &PreviewTileRequest,
) -> AuthenticityResult<PreviewImage> {
    if !(TILE_MIN_EDGE..=TILE_MAX_EDGE).contains(&request.max_edge) {
        return Err(AuthenticityError::InvalidInput(
            "预览局部尺寸超出允许范围".into(),
        ));
    }
    let (image_path, bounds) = match request.source {
        PreviewTileSource::Compressed => {
            let token = request
                .cache_token
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(AuthenticityError::InvalidInput("预览缓存令牌无效".into()));
            }
            let (jpeg_path, metadata_path) = cache_paths(root, token)?;
            if !jpeg_path.is_file() || !metadata_path.is_file() {
                return Err(AuthenticityError::Task(
                    "质量预览缓存已失效，请重新生成预览".into(),
                ));
            }
            let metadata: RenditionCacheMetadata =
                serde_json::from_reader(File::open(&metadata_path)?)?;
            if metadata.token != token {
                return Err(AuthenticityError::Task("发布预览缓存校验失败".into()));
            }
            (jpeg_path, (metadata.width, metadata.height))
        }
        PreviewTileSource::Original => {
            let branch_id = request
                .branch_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if branch_id.is_empty() {
                return Err(AuthenticityError::InvalidInput("缺少分支标识".into()));
            }
            let artifact = PathBuf::from(
                publication_repository::publication_target(root, branch_id)
                    .map_err(AuthenticityError::Task)?
                    .artifact_path,
            );
            if !artifact.is_file() {
                return Err(AuthenticityError::Task("最终成品不可用".into()));
            }
            let (width, height) = image::ImageReader::open(&artifact)?
                .with_guessed_format()?
                .into_dimensions()?;
            (artifact, (width, height))
        }
        PreviewTileSource::External => {
            let value = request
                .path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AuthenticityError::InvalidInput("缺少待识别图片路径".into()))?;
            let path = PathBuf::from(value);
            if !path.is_file() {
                return Err(AuthenticityError::InvalidInput(
                    "待识别图片不存在或不是普通文件".into(),
                ));
            }
            let canonical = path.canonicalize()?;
            storage::ensure_outside_repository(root, &canonical, "待识别图片")
                .map_err(AuthenticityError::InvalidInput)?;
            let (width, height) = image::ImageReader::open(&canonical)?
                .with_guessed_format()?
                .into_dimensions()?;
            (canonical, (width, height))
        }
    };
    let right = request
        .x
        .checked_add(request.width)
        .ok_or_else(|| AuthenticityError::InvalidInput("预览局部矩形无效".into()))?;
    let bottom = request
        .y
        .checked_add(request.height)
        .ok_or_else(|| AuthenticityError::InvalidInput("预览局部矩形无效".into()))?;
    if request.width == 0 || request.height == 0 || right > bounds.0 || bottom > bounds.1 {
        return Err(AuthenticityError::InvalidInput(
            "预览局部矩形超出源图片范围".into(),
        ));
    }
    let source = open_tile_source(&image_path)?;
    if source.dimensions() != bounds {
        return Err(AuthenticityError::Task("预览源尺寸不匹配".into()));
    }
    let crop = source.crop_imm(request.x, request.y, request.width, request.height);
    drop(source);
    // 只缩不放：局部边长已小于请求上限时保留原生分辨率。
    let tile = if crop.width().max(crop.height()) > request.max_edge {
        crop.thumbnail(request.max_edge, request.max_edge)
    } else {
        crop
    };
    let (width, height) = tile.dimensions();
    let mut encoded = Cursor::new(Vec::new());
    tile.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(PreviewImage {
        data_url: format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        ),
        width,
        height,
        source_bytes: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticity::{
        rendition::{render_cached_rendition, rendition_cache_token},
        test_support::{fixture_config, fixture_state},
    };

    fn tile_request(
        token: &str,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        max_edge: u32,
    ) -> PreviewTileRequest {
        PreviewTileRequest {
            source: PreviewTileSource::Compressed,
            cache_token: Some(token.into()),
            branch_id: None,
            path: None,
            x,
            y,
            width,
            height,
            max_edge,
        }
    }

    fn original_tile_request(
        branch_id: &str,
        width: u32,
        height: u32,
        max_edge: u32,
    ) -> PreviewTileRequest {
        PreviewTileRequest {
            source: PreviewTileSource::Original,
            cache_token: None,
            branch_id: Some(branch_id.into()),
            path: None,
            x: 0,
            y: 0,
            width,
            height,
            max_edge,
        }
    }

    fn external_tile_request(
        path: &Path,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        max_edge: u32,
    ) -> PreviewTileRequest {
        PreviewTileRequest {
            source: PreviewTileSource::External,
            cache_token: None,
            branch_id: None,
            path: Some(path.to_string_lossy().into_owned()),
            x,
            y,
            width,
            height,
            max_edge,
        }
    }

    #[test]
    fn preview_tile_crops_cached_rendition_without_upscaling() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let state = fixture_state();
        let config = fixture_config();
        let source_sha256 = "A".repeat(64);
        let token = rendition_cache_token(&source_sha256, &config, None).unwrap();
        let operation = state.begin_operation("tile 测试").unwrap();
        let cached = render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            image::DynamicImage::ImageRgb8(image::RgbImage::new(320, 240)),
            &config,
            None,
            &token,
            &source_sha256,
        )
        .unwrap();
        assert_eq!((cached.width, cached.height), (320, 240));

        let full = preview_tile(
            directory.path(),
            &tile_request(&token, 0, 0, 320, 240, 4096),
        )
        .unwrap();
        assert_eq!((full.width, full.height), (320, 240));
        assert!(full.data_url.starts_with("data:image/png;base64,"));

        let region = preview_tile(
            directory.path(),
            &tile_request(&token, 10, 20, 160, 120, 80),
        )
        .unwrap();
        assert_eq!((region.width, region.height), (80, 60));
    }

    #[test]
    fn preview_tile_rejects_bad_tokens_rects_and_missing_cache() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("temp")).unwrap();
        let token = "a".repeat(64);

        assert!(preview_tile(
            directory.path(),
            &tile_request("short-token", 0, 0, 1, 1, 256)
        )
        .is_err());
        assert!(preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 16)).is_err());
        assert!(preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 4097)).is_err());
        assert!(preview_tile(directory.path(), &tile_request(&token, 0, 0, 1, 1, 256)).is_err());

        let state = fixture_state();
        let config = fixture_config();
        let operation = state.begin_operation("tile 校验测试").unwrap();
        render_cached_rendition(
            directory.path(),
            &state,
            &operation,
            image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24)),
            &config,
            None,
            &token,
            &"B".repeat(64),
        )
        .unwrap();

        assert!(preview_tile(directory.path(), &tile_request(&token, 30, 0, 8, 8, 256)).is_err());
        assert!(preview_tile(directory.path(), &tile_request(&token, 0, 0, 0, 8, 256)).is_err());
        assert!(preview_tile(directory.path(), &tile_request(&token, 0, 0, 8, 8, 256)).is_ok());
    }

    #[test]
    fn preview_tile_crops_original_artifact_by_branch() {
        use sha2::Digest;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::authenticity::test_support::initialize_publication(&root);
        let relative = "artworks/final.png";
        let absolute = storage::resolve_path(&root, relative).unwrap();
        fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(300, 200))
            .save(&absolute)
            .unwrap();
        let sha = hex::encode_upper(sha2::Sha256::digest(fs::read(&absolute).unwrap()));
        crate::authenticity::test_support::insert_final_artifact(&root, relative, &sha);

        let tile = preview_tile(&root, &original_tile_request("branch", 300, 200, 120)).unwrap();
        assert_eq!((tile.width, tile.height), (120, 80));
        assert!(tile.data_url.starts_with("data:image/png;base64,"));

        // 矩形超出源尺寸或分支不存在都明确拒绝。
        assert!(preview_tile(&root, &original_tile_request("branch", 301, 200, 120)).is_err());
        assert!(preview_tile(&root, &original_tile_request("missing", 300, 200, 120)).is_err());
    }

    #[test]
    fn preview_tile_crops_external_source_with_repo_boundary_checks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        // 外部源必须位于仓库之外，因此与仓库同级摆放（拖放导入也落在系统临时目录）。
        let external = directory.path().join("staged.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(200, 150))
            .save(&external)
            .unwrap();

        let tile =
            preview_tile(&root, &external_tile_request(&external, 20, 30, 72, 72, 72)).unwrap();
        assert_eq!((tile.width, tile.height), (72, 72));
        assert!(tile.data_url.starts_with("data:image/png;base64,"));

        // 只缩不放：请求矩形小于 max_edge 时保留原生分辨率。
        let native =
            preview_tile(&root, &external_tile_request(&external, 0, 0, 32, 32, 72)).unwrap();
        assert_eq!((native.width, native.height), (32, 32));

        // 矩形越界、文件缺失与缺少路径都明确拒绝。
        assert!(
            preview_tile(&root, &external_tile_request(&external, 190, 0, 72, 72, 72)).is_err()
        );
        assert!(preview_tile(
            &root,
            &external_tile_request(&directory.path().join("missing.png"), 0, 0, 32, 32, 72),
        )
        .is_err());
        let mut missing = external_tile_request(&external, 0, 0, 32, 32, 72);
        missing.path = None;
        assert!(preview_tile(&root, &missing).is_err());

        // 仓库内路径被边界校验拒绝，外部源通道不会成为读取仓库文件的旁路。
        let inside = storage::resolve_path(&root, "artworks/inside.png").unwrap();
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(64, 64))
            .save(&inside)
            .unwrap();
        assert!(preview_tile(&root, &external_tile_request(&inside, 0, 0, 32, 32, 72)).is_err());
    }
}
