import type { CertificationConfig, NormalizedRegion, WatermarkSizeWarning } from "./types";

/** 小选区水印警告的稳定边长阈值（像素），与后端 `WATERMARK_MIN_STABLE_EDGE` 一致。 */
export const WATERMARK_MIN_STABLE_EDGE = 256;

export function publicationPreviewError(config: CertificationConfig, privateKey: string): string | null {
  if (!config.title.trim()) return "请输入作品标题后再生成质量预览。";
  if (!config.creator.trim()) return "请输入创作者后再生成质量预览。";
  if (!config.certificatePath.trim()) return "请选择 PEM 证书链后再生成质量预览。";
  if (!privateKey.trim()) return "请输入 PEM 私钥后再生成质量预览。";
  if (config.jpegQuality < 1 || config.jpegQuality > 100) return "JPEG 质量必须在 1 到 100 之间。";
  return null;
}

export function publicationPreviewSignature(config: CertificationConfig, watermarkId: string): string {
  return JSON.stringify({
    branchId: config.branchId,
    jpegQuality: config.jpegQuality,
    backgroundColor: config.backgroundColor,
    trustmarkEnabled: config.trustmarkEnabled,
    watermarkStrength: config.watermarkStrength,
    additionalRegions: config.additionalRegions,
    watermarkId: watermarkId.trim(),
  });
}

/**
 * 选区在 rendition 像素坐标系下的边界。与后端 `trustmark::region_bounds` 同口径
 * （起点 floor、终点 ceil 后取差），源像素尺寸即 rendition 像素尺寸——透明背景合成
 * 与 JPEG 编码都不改变尺寸，因此这里的判定就是送进编码器的那张图的像素边长。
 */
export function regionPixelBounds(
  region: NormalizedRegion,
  sourceWidth: number,
  sourceHeight: number,
): { width: number; height: number } {
  const x0 = Math.floor(region.x * sourceWidth);
  const y0 = Math.floor(region.y * sourceHeight);
  const x1 = Math.min(sourceWidth, Math.ceil((region.x + region.width) * sourceWidth));
  const y1 = Math.min(sourceHeight, Math.ceil((region.y + region.height) * sourceHeight));
  return { width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0) };
}

/**
 * 发布前就地提示的小选区水印警告：任一边长小于稳定边长阈值的选区。
 *
 * 这是降低风险不是保证——与后端发布结果同口径，只用于框选时的即时提示；真正的
 * 发布结果警告由后端在签名后返回。
 */
export function watermarkSizeWarnings(
  regions: NormalizedRegion[],
  sourceWidth: number,
  sourceHeight: number,
): WatermarkSizeWarning[] {
  return regions.flatMap((region, regionIndex) => {
    const { width, height } = regionPixelBounds(region, sourceWidth, sourceHeight);
    return Math.min(width, height) < WATERMARK_MIN_STABLE_EDGE
      ? [{ regionIndex, width, height, threshold: WATERMARK_MIN_STABLE_EDGE }]
      : [];
  });
}
