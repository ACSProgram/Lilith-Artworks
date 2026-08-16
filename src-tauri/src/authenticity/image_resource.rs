use std::path::Path;

use image::{DynamicImage, ImageReader, Limits};

use super::error::AuthenticityResult;

const MAX_IMAGE_EDGE: u32 = 32_768;
const MAX_IMAGE_PIXELS: u64 = 300_000_000;
const MAX_DECODE_ALLOCATION: u64 = 1536 * 1024 * 1024;

pub(crate) fn open(path: &Path) -> AuthenticityResult<DynamicImage> {
    let probe = ImageReader::open(path)?.with_guessed_format()?;
    let (width, height) = probe.into_dimensions()?;
    validate_dimensions(width, height)?;

    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(resource_limits());
    Ok(reader.decode()?)
}

fn validate_dimensions(width: u32, height: u32) -> AuthenticityResult<()> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| super::error::AuthenticityError::InvalidInput("图片像素数量溢出".into()))?;
    if width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE || pixels > MAX_IMAGE_PIXELS {
        return Err(super::error::AuthenticityError::InvalidInput(format!(
            "图片尺寸 {width} x {height} 超过认证处理的资源预算"
        )));
    }
    Ok(())
}

fn resource_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    limits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_accept_16k_images_but_reject_extreme_headers() {
        let limits = resource_limits();

        assert!(limits.check_dimensions(16_384, 16_384).is_ok());
        assert!(validate_dimensions(16_384, 16_384).is_ok());
        assert!(validate_dimensions(20_000, 20_000).is_err());
        assert!(validate_dimensions(32_769, 1).is_err());
        assert!(validate_dimensions(1, 32_769).is_err());
    }
}
