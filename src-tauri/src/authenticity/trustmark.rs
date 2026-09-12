use image::{imageops, DynamicImage, GenericImage, GenericImageView, Rgb, RgbImage};
use rand::Rng;

use super::{
    error::{AuthenticityError, AuthenticityResult},
    model::NormalizedRegion,
    state::AuthenticityState,
};

pub(crate) const WATERMARK_BITS: usize = 40;
pub(crate) const SOFT_BINDING_ALGORITHM: &str = "com.adobe.trustmark";
pub(crate) const MODEL_VARIANT: &str = "Q / BCH_SUPER";
const SCHEMA_PREFIX: &str = "1";

pub(crate) fn resolve_identifier(requested: Option<&str>) -> AuthenticityResult<String> {
    if let Some(value) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        if value.len() != WATERMARK_BITS || value.bytes().any(|byte| !matches!(byte, b'0' | b'1')) {
            return Err(AuthenticityError::InvalidInput(format!(
                "自定义 TrustMark ID 必须正好是 {WATERMARK_BITS} 位二进制字符串"
            )));
        }
        return Ok(value.to_owned());
    }
    let mut rng = rand::rng();
    Ok((0..WATERMARK_BITS)
        .map(|_| if rng.random::<bool>() { '1' } else { '0' })
        .collect())
}

pub(crate) fn flatten_to_rgb(image: &DynamicImage, background: Rgb<u8>) -> RgbImage {
    let mut output = RgbImage::new(image.width(), image.height());
    for (x, y, pixel) in image.pixels() {
        let alpha = pixel[3] as u16;
        let inverse = 255 - alpha;
        output.put_pixel(
            x,
            y,
            Rgb([
                ((pixel[0] as u16 * alpha + background[0] as u16 * inverse + 127) / 255) as u8,
                ((pixel[1] as u16 * alpha + background[1] as u16 * inverse + 127) / 255) as u8,
                ((pixel[2] as u16 * alpha + background[2] as u16 * inverse + 127) / 255) as u8,
            ]),
        );
    }
    output
}

pub(crate) fn parse_background(value: &str) -> AuthenticityResult<Rgb<u8>> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AuthenticityError::InvalidInput(
            "透明背景色必须使用 #RRGGBB 格式".into(),
        ));
    }
    let channel = |start| u8::from_str_radix(&hex[start..start + 2], 16);
    Ok(Rgb([
        channel(0).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
        channel(2).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
        channel(4).map_err(|_| AuthenticityError::InvalidInput("背景色无效".into()))?,
    ]))
}

pub(crate) fn encode_regions(
    state: &AuthenticityState,
    image: RgbImage,
    identifier: &str,
    strength: f32,
    additional_regions: &[NormalizedRegion],
) -> AuthenticityResult<RgbImage> {
    if !strength.is_finite() || !(0.5..=1.5).contains(&strength) {
        return Err(AuthenticityError::InvalidInput(
            "水印强度必须在 0.50 到 1.50 之间".into(),
        ));
    }
    if additional_regions.len() > 8 {
        return Err(AuthenticityError::InvalidInput(
            "额外水印区域最多为 8 个".into(),
        ));
    }
    let (width, height) = image.dimensions();
    if width < 96 || height < 96 {
        return Err(AuthenticityError::InvalidInput(
            "图片至少需要 96 x 96 像素".into(),
        ));
    }
    if additional_regions.is_empty() {
        return Err(AuthenticityError::InvalidInput(
            "请先框选至少一个 TrustMark 区域".into(),
        ));
    }
    let bounds = additional_regions
        .iter()
        .map(|region| pixel_bounds(*region, width, height, 96))
        .collect::<AuthenticityResult<Vec<_>>>()?;
    state.with_engine(|engine| {
        let mut output = image.clone();
        for (x, y, region_width, region_height) in bounds {
            let region = imageops::crop_imm(&image, x, y, region_width, region_height).to_image();
            let encoded = engine
                .encode(
                    identifier.to_owned(),
                    DynamicImage::ImageRgb8(region),
                    strength,
                )?
                .to_rgb8();
            output.copy_from(&encoded, x, y)?;
        }
        Ok(output)
    })
}

pub(crate) fn decode_region(
    state: &AuthenticityState,
    image: &DynamicImage,
    region: Option<NormalizedRegion>,
) -> AuthenticityResult<Option<String>> {
    let candidate = if let Some(region) = region {
        let (width, height) = image.dimensions();
        let (x, y, region_width, region_height) = pixel_bounds(region, width, height, 64)?;
        image.crop_imm(x, y, region_width, region_height)
    } else {
        image.clone()
    };
    state.with_engine(|engine| {
        Ok(engine.decode(candidate).ok().filter(|identifier| {
            identifier.len() == WATERMARK_BITS
                && identifier.bytes().all(|byte| matches!(byte, b'0' | b'1'))
        }))
    })
}

fn pixel_bounds(
    region: NormalizedRegion,
    image_width: u32,
    image_height: u32,
    minimum_size: u32,
) -> AuthenticityResult<(u32, u32, u32, u32)> {
    let values = [region.x, region.y, region.width, region.height];
    if values.iter().any(|value| !value.is_finite())
        || region.x < 0.0
        || region.y < 0.0
        || region.width <= 0.0
        || region.height <= 0.0
        || region.x + region.width > 1.000_001
        || region.y + region.height > 1.000_001
    {
        return Err(AuthenticityError::InvalidInput(
            "水印区域必须位于图片归一化坐标 0 到 1 内".into(),
        ));
    }
    let x0 = (region.x * image_width as f32).floor() as u32;
    let y0 = (region.y * image_height as f32).floor() as u32;
    let x1 = ((region.x + region.width) * image_width as f32)
        .ceil()
        .min(image_width as f32) as u32;
    let y1 = ((region.y + region.height) * image_height as f32)
        .ceil()
        .min(image_height as f32) as u32;
    let width = x1.saturating_sub(x0);
    let height = y1.saturating_sub(y0);
    if width < minimum_size || height < minimum_size {
        return Err(AuthenticityError::InvalidInput(format!(
            "所选区域至少需要 {minimum_size} x {minimum_size} 像素"
        )));
    }
    Ok((x0, y0, width, height))
}

pub(crate) fn soft_binding_value(identifier: &str) -> String {
    format!("{SCHEMA_PREFIX}*{identifier}")
}

pub(crate) fn identifier_from_soft_binding(value: &str) -> Option<String> {
    let (schema, identifier) = value.split_once('*')?;
    (schema == SCHEMA_PREFIX
        && identifier.len() == WATERMARK_BITS
        && identifier.bytes().all(|byte| matches!(byte, b'0' | b'1')))
    .then(|| identifier.to_owned())
}
