import type { CertificationConfig } from "./types";

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
