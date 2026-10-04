//! 素材板 DDS/BC7 图像处理与纹理读取缓存。
//!
//! 图像格式逻辑自 Lilith Client 素材板模块原样迁移：导入归一化、DDS 头校验、
//! BC7 解码预览与纹理结果缓存。存储路径与数据库访问由 `repository.rs`
//! 与 `mod.rs` 负责。

use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Write},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

use tempfile::NamedTempFile;

pub(crate) const MAX_DDS_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_TEXTURE_DIMENSION: u32 = 8192;
pub(crate) const MIN_TEXTURE_DIMENSION: u32 = 64;
pub(crate) const MAX_SOURCE_DIMENSION: u32 = 16384;
pub(crate) const MAX_SOURCE_PIXELS: u64 = 96_000_000;
pub(crate) const MAX_DDS_SOURCE_PIXELS: u64 = 268_435_456;
pub(crate) const DXGI_FORMAT_BC7_UNORM: u32 = 98;
pub(crate) const DDS_DX10_HEADER_BYTES: usize = 148;
pub(crate) const MAX_BOARD_IMAGES: usize = 20_000;
pub(crate) const MAX_IMPORT_FILES: usize = 256;
pub(crate) const MAX_CLIPBOARD_IMAGE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_IMAGE_PIXELS: u64 = 64_000_000;
pub(crate) const MAX_RGBA_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const RGBA_BYTES_PER_PIXEL: u64 = 4;
/// 纹理读取结果 LRU 缓存预算。大图（上亿像素）每次请求都要重新解码全部
/// BC7 块，成本与目标尺寸几乎无关；缓存按 (画板, 图片, 请求长边) 保存
/// 解码结果，滑动回来或重复请求同一尺寸时直接复用，避免反复等待。
/// 预算由配置的缓存等级决定，默认中等（128 MiB）。
pub(crate) const TEXTURE_CACHE_DEFAULT_BUDGET_BYTES: u64 = 128 * 1024 * 1024;

/// 素材板纹理缓存等级对应的 Rust 解码结果缓存预算（前端 GPU 缓存等级见
/// `texturePolicy.ts` 的 `textureBudgetsForLevel`）。低 64 / 中 128 / 高 256 MiB。
pub(crate) fn texture_cache_budget_bytes(level: &str) -> u64 {
    match level {
        "low" => 64 * 1024 * 1024,
        "high" => 256 * 1024 * 1024,
        _ => TEXTURE_CACHE_DEFAULT_BUDGET_BYTES,
    }
}

pub(crate) struct TextureCacheEntry {
    pub(crate) bytes: Vec<u8>,
    pub(crate) last_used: u64,
}

pub(crate) type TextureCacheKey = (i64, i64, u32);

static TEXTURE_CACHE: OnceLock<Mutex<HashMap<TextureCacheKey, TextureCacheEntry>>> =
    OnceLock::new();
static TEXTURE_CACHE_CLOCK: AtomicU64 = AtomicU64::new(1);

fn texture_cache_tick() -> u64 {
    TEXTURE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn texture_cache_get(key: TextureCacheKey) -> Option<Vec<u8>> {
    let cache = TEXTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match cache.get_mut(&key) {
        Some(entry) => {
            entry.last_used = texture_cache_tick();
            Some(entry.bytes.clone())
        }
        None => None,
    }
}

pub(crate) fn texture_cache_store(key: TextureCacheKey, bytes: Vec<u8>, budget_bytes: u64) {
    let cache = TEXTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(
        key,
        TextureCacheEntry {
            bytes,
            last_used: texture_cache_tick(),
        },
    );
    texture_cache_evict(&mut cache, budget_bytes);
}

fn texture_cache_evict(cache: &mut HashMap<TextureCacheKey, TextureCacheEntry>, budget_bytes: u64) {
    let total: u64 = cache.values().map(|entry| entry.bytes.len() as u64).sum();
    if total <= budget_bytes {
        return;
    }
    // 预算超限时按最久未使用逐个淘汰（条目数受预算约束，线性扫描可接受）。
    while cache.len() > 1 {
        let total: u64 = cache.values().map(|entry| entry.bytes.len() as u64).sum();
        if total <= budget_bytes {
            break;
        }
        let victim = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key);
        match victim {
            Some(key) => {
                cache.remove(&key);
            }
            None => break,
        }
    }
}

pub(crate) fn read_limited(path: &Path, limit: u64, kind: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取{kind}信息：{error}"))?;
    if !metadata.is_file() {
        return Err(format!("{kind}不是文件"));
    }
    if metadata.len() > limit {
        return Err(format!("{kind}超过大小限制"));
    }
    fs::read(path).map_err(|error| format!("无法读取{kind}：{error}"))
}

pub(crate) fn checked_pixel_count(width: u32, height: u32) -> Result<u64, String> {
    (width as u64)
        .checked_mul(height as u64)
        .ok_or_else(|| "图片像素数超出读取范围".to_owned())
}

pub(crate) fn validate_source_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("图片尺寸无效".into());
    }
    if width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        return Err(format!(
            "图片长边超过 {} 像素的读取限制",
            MAX_SOURCE_DIMENSION
        ));
    }
    if checked_pixel_count(width, height)? > MAX_SOURCE_PIXELS {
        return Err("图片像素数超过读取限制".into());
    }
    Ok(())
}

pub(crate) fn validate_dds_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("DDS 图片尺寸无效".into());
    }
    if width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        return Err(format!(
            "DDS 图片长边超过 {} 像素的读取限制",
            MAX_SOURCE_DIMENSION
        ));
    }
    if checked_pixel_count(width, height)? > MAX_DDS_SOURCE_PIXELS {
        return Err("DDS 图片像素数超过读取限制".into());
    }
    Ok(())
}

pub(crate) fn bounded_dimensions(
    width: u32,
    height: u32,
    max_dimension: u32,
    max_pixels: u64,
) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    let max_dimension = max_dimension.max(1);
    let max_pixels = max_pixels.max(1);
    let source_pixels = (width as f64) * (height as f64);
    let source_max = width.max(height) as f64;
    let mut scale = (max_dimension as f64 / source_max).min(1.0);
    if source_pixels > max_pixels as f64 {
        scale = scale.min((max_pixels as f64 / source_pixels).sqrt());
    }

    let mut target_width = ((width as f64) * scale).round().max(1.0) as u32;
    let mut target_height = ((height as f64) * scale).round().max(1.0) as u32;
    target_width = target_width.min(width).min(max_dimension).max(1);
    target_height = target_height.min(height).min(max_dimension).max(1);
    while (target_width as u64) * (target_height as u64) > max_pixels {
        if target_width >= target_height && target_width > 1 {
            target_width -= 1;
        } else if target_height > 1 {
            target_height -= 1;
        } else {
            break;
        }
    }
    (target_width, target_height)
}

pub(crate) fn import_dimensions(width: u32, height: u32) -> Result<(u32, u32), String> {
    validate_source_dimensions(width, height)?;
    Ok(bounded_dimensions(
        width,
        height,
        MAX_TEXTURE_DIMENSION,
        MAX_IMAGE_PIXELS,
    ))
}

pub(crate) fn normalise_rgba_image(
    image: image::RgbaImage,
) -> Result<(image::RgbaImage, u32, u32), String> {
    let (width, height) = image.dimensions();
    let (target_width, target_height) = import_dimensions(width, height)?;
    if (target_width, target_height) == (width, height) {
        return Ok((image, width, height));
    }
    let resized = image::imageops::thumbnail(&image, target_width, target_height);
    Ok((resized, target_width, target_height))
}

fn raster_decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_SOURCE_PIXELS * RGBA_BYTES_PER_PIXEL + 32 * 1024 * 1024);
    limits
}

pub(crate) fn rgba_payload_image(payload: &[u8]) -> Result<image::RgbaImage, String> {
    if payload.len() < 12 || &payload[0..4] != b"RGBA" {
        return Err("RGBA 预览负载头部无效".into());
    }
    let width = u32::from_le_bytes(payload[4..8].try_into().map_err(|_| "RGBA 预览宽度无效")?);
    let height = u32::from_le_bytes(payload[8..12].try_into().map_err(|_| "RGBA 预览高度无效")?);
    let bytes = checked_pixel_count(width, height)?
        .checked_mul(RGBA_BYTES_PER_PIXEL)
        .ok_or("RGBA 预览尺寸过大")?;
    if bytes > MAX_RGBA_PREVIEW_BYTES {
        return Err("RGBA 预览超过内存上限".into());
    }
    let data = payload
        .get(12..)
        .filter(|data| data.len() as u64 == bytes)
        .ok_or("RGBA 预览数据不完整")?;
    image::RgbaImage::from_raw(width, height, data.to_vec())
        .ok_or_else(|| "RGBA 预览尺寸无效".to_owned())
}

pub(crate) fn decode_raster_path(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let mut reader =
        image::ImageReader::open(path).map_err(|error| format!("无法读取导入图片：{error}"))?;
    reader.limits(raster_decode_limits());
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("无法识别导入图片：{error}"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("无法读取导入图片尺寸：{error}"))?;
    validate_source_dimensions(width, height)?;
    let mut reader =
        image::ImageReader::open(path).map_err(|error| format!("无法读取导入图片：{error}"))?;
    reader.limits(raster_decode_limits());
    let decoded = reader
        .with_guessed_format()
        .map_err(|error| format!("无法识别导入图片：{error}"))?
        .decode()
        .map_err(|error| format!("无法解码导入图片：{error}"))?
        .to_rgba8();
    let (image, width, height) = normalise_rgba_image(decoded)?;
    Ok((encode_bc7_dds(&image)?, width, height))
}

pub(crate) fn decode_raster_bytes(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes));
    reader.limits(raster_decode_limits());
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("无法识别剪贴板图片：{error}"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("无法读取剪贴板图片尺寸：{error}"))?;
    validate_source_dimensions(width, height)?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes));
    reader.limits(raster_decode_limits());
    let decoded = reader
        .with_guessed_format()
        .map_err(|error| format!("无法识别剪贴板图片：{error}"))?
        .decode()
        .map_err(|error| format!("无法解码剪贴板图片：{error}"))?
        .to_rgba8();
    let (image, width, height) = normalise_rgba_image(decoded)?;
    Ok((encode_bc7_dds(&image)?, width, height))
}

pub(crate) fn encode_bc7_dds(image: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let dds = image_dds::dds_from_image(
        image,
        image_dds::ImageFormat::BC7RgbaUnorm,
        image_dds::Quality::Fast,
        image_dds::Mipmaps::Disabled,
    )
    .map_err(|error| format!("无法压缩导入图片：{error}"))?;
    let mut output = Cursor::new(Vec::new());
    dds.write(&mut output)
        .map_err(|error| format!("无法生成导入 DDS 图片：{error}"))?;
    Ok(output.into_inner())
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or("DDS 头部不完整")?
        .try_into()
        .map_err(|_| "DDS 头部不完整")?;
    Ok(u32::from_le_bytes(raw))
}

pub(crate) fn dds_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 148 || bytes.get(0..4) != Some(b"DDS ") {
        return Err("图片不是有效的 DDS 文件".into());
    }
    if bytes.get(84..88) != Some(b"DX10") || u32_at(bytes, 128)? != DXGI_FORMAT_BC7_UNORM {
        return Err("素材板只支持 DX10 BC7_UNORM DDS".into());
    }
    let width = u32_at(bytes, 16)?;
    let height = u32_at(bytes, 12)?;
    validate_dds_dimensions(width, height)?;
    Ok((width, height))
}

pub(crate) fn preview_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    bounded_dimensions(
        width,
        height,
        max_dimension,
        MAX_RGBA_PREVIEW_BYTES / RGBA_BYTES_PER_PIXEL,
    )
}

fn flush_preview_row(output: &mut [u8], target_y: usize, sums: &[[u32; 4]], counts: &[u32]) {
    for (target_x, (sum, count)) in sums.iter().zip(counts).enumerate() {
        let count = (*count).max(1);
        let output_offset = (target_y * sums.len() + target_x) * 4;
        for channel in 0..4 {
            output[output_offset + channel] = ((sum[channel] + count / 2) / count) as u8;
        }
    }
}

pub(crate) fn downscale_bc7(
    bytes: &[u8],
    width: u32,
    height: u32,
    max_dimension: u32,
) -> Result<Vec<u8>, String> {
    validate_dds_dimensions(width, height)?;
    let blocks_per_row = width.div_ceil(4) as usize;
    let block_rows = height.div_ceil(4) as usize;
    let expected_data_bytes = blocks_per_row
        .checked_mul(block_rows)
        .and_then(|blocks| blocks.checked_mul(16))
        .ok_or("DDS 图片尺寸过大")?;
    let data_end = DDS_DX10_HEADER_BYTES
        .checked_add(expected_data_bytes)
        .ok_or("DDS 图片尺寸过大")?;
    let data = bytes
        .get(DDS_DX10_HEADER_BYTES..data_end)
        .ok_or("DDS BC7 数据不完整")?;
    let (target_width, target_height) = preview_dimensions(width, height, max_dimension);
    let output_bytes = (target_width as usize)
        .checked_mul(target_height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("DDS 预览尺寸过大")?;
    let mut pixels = vec![0u8; output_bytes];
    let decoded_row_bytes = (width as usize).checked_mul(16).ok_or("DDS 图片尺寸过大")?;
    let mut decoded_rows = vec![0u8; decoded_row_bytes];
    let mut sums = vec![[0u32; 4]; target_width as usize];
    let mut counts = vec![0u32; target_width as usize];
    let target_x: Vec<usize> = (0..width)
        .map(|source_x| (source_x as u64 * target_width as u64 / width as u64) as usize)
        .collect();
    let mut active_target_y = 0usize;

    for block_y in 0..block_rows {
        for block_x in 0..blocks_per_row {
            let block_offset = (block_y * blocks_per_row + block_x) * 16;
            let mut decoded = [0u8; 64];
            bcdec_rs::bc7(&data[block_offset..block_offset + 16], &mut decoded, 16);
            for local_y in 0..4 {
                let source_y = block_y * 4 + local_y;
                if source_y >= height as usize {
                    break;
                }
                for local_x in 0..4 {
                    let source_x = block_x * 4 + local_x;
                    if source_x >= width as usize {
                        break;
                    }
                    let source = (local_y * 4 + local_x) * 4;
                    let destination = (local_y * width as usize + source_x) * 4;
                    decoded_rows[destination..destination + 4]
                        .copy_from_slice(&decoded[source..source + 4]);
                }
            }
        }

        for local_y in 0..4 {
            let source_y = block_y * 4 + local_y;
            if source_y >= height as usize {
                break;
            }
            let next_target_y = source_y * target_height as usize / height as usize;
            if next_target_y != active_target_y {
                flush_preview_row(&mut pixels, active_target_y, &sums, &counts);
                sums.fill([0; 4]);
                counts.fill(0);
                active_target_y = next_target_y;
            }
            for (source_x, target_x) in target_x.iter().copied().enumerate() {
                let source = (local_y * width as usize + source_x) * 4;
                for channel in 0..4 {
                    sums[target_x][channel] += decoded_rows[source + channel] as u32;
                }
                counts[target_x] += 1;
            }
        }
    }
    flush_preview_row(&mut pixels, active_target_y, &sums, &counts);

    let mut payload = Vec::with_capacity(12 + pixels.len());
    payload.extend_from_slice(b"RGBA");
    payload.extend_from_slice(&target_width.to_le_bytes());
    payload.extend_from_slice(&target_height.to_le_bytes());
    payload.extend_from_slice(&pixels);
    Ok(payload)
}

pub(crate) fn dds_payload_end(width: u32, height: u32) -> Result<usize, String> {
    let blocks_per_row = width.div_ceil(4) as usize;
    let block_rows = height.div_ceil(4) as usize;
    blocks_per_row
        .checked_mul(block_rows)
        .and_then(|blocks| blocks.checked_mul(16))
        .and_then(|bytes| DDS_DX10_HEADER_BYTES.checked_add(bytes))
        .ok_or_else(|| "DDS 图片尺寸过大".to_owned())
}

pub(crate) fn validate_dds_payload(bytes: &[u8], width: u32, height: u32) -> Result<(), String> {
    let end = dds_payload_end(width, height)?;
    if bytes.len() < end {
        return Err("DDS BC7 数据不完整".into());
    }
    Ok(())
}

/// 解码 DDS 中的全部 BC7 块，确认数据能被解码器完整消费。
///
/// `bcdec_rs::bc7` 对任意 16 字节输入都是全函数（不会返回错误），因此本函数的
/// 实际作用是遍历全部块、走一遍完整解码路径（守住边界、避免恐慌），而无法判定
/// 像素内容是否“正确”——在没有原始素材的前提下内容语义不可判定。长度不足由
/// `validate_dds_payload` 拦截，本函数只消费声明长度内的块。
pub(crate) fn validate_bc7_decodable(bytes: &[u8], width: u32, height: u32) -> Result<(), String> {
    let end = dds_payload_end(width, height)?;
    let data = bytes
        .get(DDS_DX10_HEADER_BYTES..end)
        .ok_or("DDS BC7 数据不完整")?;
    let blocks_per_row = width.div_ceil(4) as usize;
    let block_rows = height.div_ceil(4) as usize;
    let mut decoded = [0_u8; 64];
    for block_y in 0..block_rows {
        for block_x in 0..blocks_per_row {
            let offset = (block_y * blocks_per_row + block_x) * 16;
            bcdec_rs::bc7(&data[offset..offset + 16], &mut decoded, 16);
        }
    }
    Ok(())
}

pub(crate) fn prepare_dds_import(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let (width, height) = dds_dimensions(bytes)?;
    validate_dds_payload(bytes, width, height)?;
    let (target_width, target_height) =
        bounded_dimensions(width, height, MAX_TEXTURE_DIMENSION, MAX_IMAGE_PIXELS);
    if (target_width, target_height) == (width, height) {
        let end = dds_payload_end(width, height)?;
        return Ok((bytes[..end].to_vec(), width, height));
    }

    // Decode through the bounded BC7 preview path so a legacy 16K texture
    // never creates a full-size RGBA allocation just to be imported again.
    let preview = downscale_bc7(bytes, width, height, MAX_TEXTURE_DIMENSION)?;
    let image = rgba_payload_image(&preview)?;
    let (image, width, height) = normalise_rgba_image(image)?;
    Ok((encode_bc7_dds(&image)?, width, height))
}

#[derive(Default)]
pub(crate) struct CreatedFiles {
    paths: Vec<std::path::PathBuf>,
    committed: bool,
}

impl CreatedFiles {
    pub(crate) fn push(&mut self, path: std::path::PathBuf) {
        self.paths.push(path);
    }

    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for CreatedFiles {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn persist_dds_file(destination: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    let parent = destination.parent().ok_or("DDS 保存路径无效")?;
    // 画板目录可能缺失（例如仓库数据迁移后目录未随行），先补建再落临时文件，
    // 否则 NamedTempFile 会因父目录不存在（os error 3）直接失败。
    fs::create_dir_all(parent).map_err(|error| format!("无法创建画板图片目录：{error}"))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| format!("无法创建{kind}临时文件：{error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("无法写入{kind}：{error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步{kind}：{error}"))?;
    temporary
        .persist(destination)
        .map_err(|error| format!("无法保存{kind}：{}", error.error))?;
    Ok(())
}

/// 粘贴/导入图片以指针位置为中心做阵列排序；总宽度使用平方根估算。
pub(crate) fn imported_points(
    sizes: &[(f64, f64)],
    center_x: f64,
    center_y: f64,
    gap: f64,
) -> Vec<[[f64; 2]; 4]> {
    if sizes.is_empty() {
        return Vec::new();
    }
    let max_height = sizes.iter().map(|(_, height)| *height).fold(0.0, f64::max);
    if max_height <= f64::EPSILON {
        return vec![
            [
                [center_x, center_y],
                [center_x, center_y],
                [center_x, center_y],
                [center_x, center_y]
            ];
            sizes.len()
        ];
    }
    let nodes = sizes
        .iter()
        .enumerate()
        .map(|(index, (width, height))| (index, *width * max_height / *height, max_height))
        .collect::<Vec<_>>();
    let line_width =
        |line: &[(usize, f64, f64)]| line.iter().map(|(_, width, _)| *width + gap).sum::<f64>();
    let total_width = line_width(&nodes);
    let max_width = (total_width * (max_height + gap)).sqrt();
    let mut lines: Vec<Vec<(usize, f64, f64)>> = Vec::new();
    let mut current = Vec::new();
    for node in nodes {
        current.push(node);
        if line_width(&current) > max_width {
            current.sort_by(|left, right| right.1.total_cmp(&left.1));
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines.sort_by(|left, right| line_width(left).total_cmp(&line_width(right)));

    let mut arranged: Vec<Option<[[f64; 2]; 4]>> = vec![None; sizes.len()];
    let mut y = 0.0;
    let mut all_center = [0.0, 0.0];
    let mut count: f64 = 0.0;
    for line in lines {
        let width = line_width(&line);
        let mut x = (max_width - width) / 2.0;
        for (index, node_width, node_height) in line {
            arranged[index] = Some([
                [x, y],
                [x + node_width, y],
                [x + node_width, y - node_height],
                [x, y - node_height],
            ]);
            all_center[0] += x + node_width / 2.0;
            all_center[1] += y - node_height / 2.0;
            count += 1.0;
            x += node_width + gap;
        }
        y += max_height + gap;
    }
    all_center[0] /= count.max(1.0);
    all_center[1] /= count.max(1.0);
    arranged
        .into_iter()
        .map(|points| {
            let points = points.expect("arrangement should cover every image");
            points.map(|point| {
                [
                    point[0] + center_x - all_center[0],
                    point[1] + center_y - all_center[1],
                ]
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
pub(crate) fn clipboard_file_paths() -> Result<Vec<String>, String> {
    use std::ptr;
    use windows_sys::Win32::{
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            },
            Ole::CF_HDROP,
        },
        UI::Shell::DragQueryFileW,
    };

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return Err("无法打开系统剪贴板".into());
        }
        let _guard = ClipboardGuard;
        let format = u32::from(CF_HDROP);
        if IsClipboardFormatAvailable(format) == 0 {
            return Ok(Vec::new());
        }
        let drop = GetClipboardData(format);
        if drop.is_null() {
            return Err("无法读取剪贴板文件路径".into());
        }
        let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0).min(MAX_IMPORT_FILES as u32);
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, ptr::null_mut(), 0);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0_u16; length as usize + 1];
            let written = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32);
            if written > 0 {
                paths.push(String::from_utf16_lossy(&buffer[..written as usize]));
            }
        }
        Ok(paths)
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn clipboard_file_paths() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_texture_cache_entries_first() {
        let mut cache: HashMap<TextureCacheKey, TextureCacheEntry> = HashMap::new();
        let mut tick = 0u64;
        let mut insert =
            |key: TextureCacheKey,
             bytes: usize,
             budget_bytes: u64,
             cache: &mut HashMap<TextureCacheKey, TextureCacheEntry>| {
                tick += 1;
                cache.insert(
                    key,
                    TextureCacheEntry {
                        bytes: vec![0; bytes],
                        last_used: tick,
                    },
                );
                texture_cache_evict(cache, budget_bytes);
            };

        insert((1, 1, 256), 40 * 1024 * 1024, 96 * 1024 * 1024, &mut cache);
        insert((1, 2, 256), 40 * 1024 * 1024, 96 * 1024 * 1024, &mut cache);
        insert((1, 3, 256), 40 * 1024 * 1024, 96 * 1024 * 1024, &mut cache);

        // 120 MiB 超过 96 MiB 预算：淘汰最久未使用的一条（40 MiB）后回到预算内。
        assert!(cache.contains_key(&(1, 3, 256)));
        assert!(cache.contains_key(&(1, 2, 256)));
        assert!(!cache.contains_key(&(1, 1, 256)));
    }

    #[test]
    fn texture_cache_round_trips_payloads() {
        texture_cache_store((7, 3, 256), vec![1, 2, 3, 4], 96 * 1024 * 1024);
        assert_eq!(texture_cache_get((7, 3, 256)), Some(vec![1, 2, 3, 4]));
        assert_eq!(texture_cache_get((7, 4, 256)), None);
    }

    #[test]
    fn texture_cache_budget_follows_cache_level() {
        assert_eq!(texture_cache_budget_bytes("low"), 64 * 1024 * 1024);
        assert_eq!(texture_cache_budget_bytes("medium"), 128 * 1024 * 1024);
        assert_eq!(texture_cache_budget_bytes("high"), 256 * 1024 * 1024);
        assert_eq!(
            texture_cache_budget_bytes(""),
            TEXTURE_CACHE_DEFAULT_BUDGET_BYTES
        );
    }

    #[test]
    fn reads_bc7_dds_dimensions() {
        let mut bytes = vec![0; 148];
        bytes[0..4].copy_from_slice(b"DDS ");
        bytes[12..16].copy_from_slice(&1080u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1920u32.to_le_bytes());
        bytes[84..88].copy_from_slice(b"DX10");
        bytes[128..132].copy_from_slice(&DXGI_FORMAT_BC7_UNORM.to_le_bytes());
        assert_eq!(dds_dimensions(&bytes).unwrap(), (1920, 1080));
    }

    #[test]
    fn calculates_preview_dimensions_without_distortion() {
        assert_eq!(preview_dimensions(12000, 6000, 2048), (2048, 1024));
        assert_eq!(preview_dimensions(6000, 12000, 2048), (1024, 2048));
    }

    #[test]
    fn caps_square_previews_by_rgba_memory_budget() {
        assert_eq!(
            preview_dimensions(16384, 16384, MAX_TEXTURE_DIMENSION),
            (4096, 4096)
        );
        assert_eq!(
            (4096_u64) * (4096_u64) * RGBA_BYTES_PER_PIXEL,
            MAX_RGBA_PREVIEW_BYTES
        );
    }

    #[test]
    fn normalises_import_dimensions_to_the_native_texture_limit() {
        assert_eq!(import_dimensions(10000, 5000).unwrap(), (8192, 4096));
        assert_eq!(import_dimensions(8192, 8192).unwrap(), (8000, 8000));
        assert!(validate_source_dimensions(MAX_SOURCE_DIMENSION + 1, 1).is_err());
    }

    #[test]
    fn rejects_incomplete_or_oversized_preview_payloads() {
        let mut oversized = Vec::from(*b"RGBA");
        oversized.extend_from_slice(&8192_u32.to_le_bytes());
        oversized.extend_from_slice(&8192_u32.to_le_bytes());
        assert!(rgba_payload_image(&oversized).is_err());

        let incomplete = vec![0_u8; DDS_DX10_HEADER_BYTES + 16];
        assert!(validate_dds_payload(&incomplete, 8, 8).is_err());
    }

    #[test]
    fn decodes_the_declared_bc7_blocks_and_rejects_truncated_data() {
        let mut bytes = vec![0_u8; DDS_DX10_HEADER_BYTES + 4 * 16];
        bytes[0..4].copy_from_slice(b"DDS ");
        bytes[12..16].copy_from_slice(&8u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&8u32.to_le_bytes());
        bytes[84..88].copy_from_slice(b"DX10");
        bytes[128..132].copy_from_slice(&DXGI_FORMAT_BC7_UNORM.to_le_bytes());

        // 8×8 声明需要 2×2×16 = 64 字节 BC7 数据，恰好完整。
        assert!(validate_bc7_decodable(&bytes, 8, 8).is_ok());
        // 少一个块的数据在进入解码前即被长度检查拦截。
        assert!(validate_bc7_decodable(&bytes[..DDS_DX10_HEADER_BYTES + 3 * 16], 8, 8).is_err());
    }

    #[test]
    fn creates_a_bounded_preview_payload() {
        let bytes = vec![0; DDS_DX10_HEADER_BYTES + 4 * 16];
        let preview = downscale_bc7(&bytes, 8, 8, 4).unwrap();
        assert_eq!(&preview[0..4], b"RGBA");
        assert_eq!(u32::from_le_bytes(preview[4..8].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(preview[8..12].try_into().unwrap()), 4);
        assert_eq!(preview.len(), 12 + 4 * 4 * 4);
    }

    #[test]
    fn persists_dds_file_into_a_missing_board_directory() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("boards").join("2").join("image.dds");

        persist_dds_file(&destination, b"dds-bytes", "剪贴板 DDS 图片").unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"dds-bytes");
    }
}
