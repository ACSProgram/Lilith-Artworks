/**
 * 整画布导出的口径：倍率、输出尺寸、分块布局。
 *
 * 全是纯函数，渲染器与导出对话框共用同一份实现——两处各算一遍迟早会漂移，
 * 而用户看到的「输出尺寸」必须与真正写出的 PNG 一致。
 */

import type { Bounds } from "./geometry";
import type { PinBoardTextureCacheLevel } from "./types";

/**
 * 纹理缓存等级 → 导出最长边的**建议**上限。
 *
 * 只按最长边限制、不设面积阈值，是维护者明确选择的：30000×30000 这类极端输入仍可
 * 发起。倍率可以手动填，填过就按填的来（见 `resolveCanvasExportScale`），这个值只
 * 决定对话框里的默认倍率与「超过建议上限」的提示。
 */
export const CANVAS_EXPORT_MAX_DIMENSION: Record<PinBoardTextureCacheLevel, number> = {
  low: 10_000,
  medium: 20_000,
  high: 30_000,
};

/** 边距上限（像素）：再大就只是给画面加白边，没有实际用途。 */
export const CANVAS_EXPORT_MAX_MARGIN = 64;

/** 边距默认值：留一圈呼吸空间，又不至于让画面缩在一角。 */
export const CANVAS_EXPORT_DEFAULT_MARGIN = 10;

export const CANVAS_EXPORT_MIN_SCALE = 0.01;
export const CANVAS_EXPORT_MAX_SCALE = 8;

/**
 * 硬上限：与原生侧的防御性校验同口径，超出直接收敛。
 *
 * 建议上限只提示不拦截，但真正荒谬的输入（例如 10 亿像素）必须挡住——原生侧
 * `MAX_OUTPUT_PIXELS` 是 10 亿像素，这里用同一组数字，避免「前端说能导、原生拒绝」。
 */
export const CANVAS_EXPORT_HARD_MAX_DIMENSION = 65_535;
export const CANVAS_EXPORT_HARD_MAX_PIXELS = 1_000_000_000;

/** 单块最大宽度：8192 是设备 `maxTextureDimension2D` 的通用上限。 */
export const CANVAS_EXPORT_TILE_MAX_WIDTH = 8192;

/** 行带常驻内存目标：原生侧按整行拼装，行带越高占得越多。 */
export const CANVAS_EXPORT_BAND_MEMORY_BYTES = 32 * 1024 * 1024;

export const CANVAS_EXPORT_BAND_MIN_HEIGHT = 128;
export const CANVAS_EXPORT_BAND_MAX_HEIGHT = 2048;

export function canvasExportMaxDimension(level: PinBoardTextureCacheLevel): number {
  return CANVAS_EXPORT_MAX_DIMENSION[level] ?? CANVAS_EXPORT_MAX_DIMENSION.medium;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

/** 边距收敛到整数与合法区间：输入框允许临时非法值，真正参与计算的只有收敛后的值。 */
export function clampCanvasExportMargin(margin: number): number {
  if (!Number.isFinite(margin)) return 0;
  return clamp(Math.round(margin), 0, CANVAS_EXPORT_MAX_MARGIN);
}

/** 含边距的输出尺寸（世界单位）。内容为空或退化时返回 null。 */
export function paddedCanvasExportSize(
  bounds: Bounds,
  margin: number,
): { width: number; height: number } | null {
  const safeMargin = clampCanvasExportMargin(margin);
  const width = bounds.maxX - bounds.minX + safeMargin * 2;
  const height = bounds.maxY - bounds.minY + safeMargin * 2;
  if (!Number.isFinite(width) || !Number.isFinite(height)) return null;
  if (!(width > 0) || !(height > 0)) return null;
  return { width, height };
}

/**
 * 对话框的默认倍率：1x 与「最长边塞进建议上限」取小，向下取两位小数。
 *
 * 内容很大时直接给出一个能导出的倍率（例如 0.44），而不是先给 1x 再在导出时偷偷
 * 压下来——用户看得见的数字必须就是实际用的数字。
 */
export function autoCanvasExportScale(bounds: Bounds, margin: number, limit: number): number {
  const padded = paddedCanvasExportSize(bounds, margin);
  if (!padded || !Number.isFinite(limit) || limit <= 0) return 1;
  const longest = Math.max(padded.width, padded.height);
  const allowed = limit / longest;
  const scale = Math.min(1, allowed);
  return Math.max(
    CANVAS_EXPORT_MIN_SCALE,
    Math.floor(scale * 100) / 100,
  );
}

export interface CanvasExportSize {
  /** 实际倍率（可能被硬上限收敛）。 */
  scale: number;
  /** 输出像素宽高。 */
  width: number;
  height: number;
  /** 最长边超过建议上限：仍可导出，只是更慢。 */
  overLimit: boolean;
}

/**
 * 把倍率收敛到硬上限内，并算出输出尺寸。
 *
 * 手动倍率优先：不再按建议上限压低，只在超过硬上限（边长 65535、总像素 10 亿）时
 * 收敛，并用 `overLimit` 交给界面提示。`limit` 只影响提示，不影响尺寸。
 * 返回的 `scale` 是**最终整数尺寸对应的倍率**（可能略小于请求值），渲染按它映射。
 */
export function resolveCanvasExportScale(
  bounds: Bounds,
  requestedScale: number,
  margin: number,
  limit: number,
): CanvasExportSize | null {
  if (!Number.isFinite(requestedScale) || requestedScale <= 0) return null;
  const padded = paddedCanvasExportSize(bounds, margin);
  if (!padded) return null;
  const longest = Math.max(padded.width, padded.height);
  const area = padded.width * padded.height;
  let scale = Math.min(requestedScale, CANVAS_EXPORT_MAX_SCALE);
  scale = Math.min(scale, CANVAS_EXPORT_HARD_MAX_DIMENSION / longest);
  if (area * scale * scale > CANVAS_EXPORT_HARD_MAX_PIXELS) {
    scale = Math.sqrt(CANVAS_EXPORT_HARD_MAX_PIXELS / area);
  }
  if (!(scale > 0)) return null;
  let width = clamp(Math.round(padded.width * scale), 1, CANVAS_EXPORT_HARD_MAX_DIMENSION);
  let height = clamp(Math.round(padded.height * scale), 1, CANVAS_EXPORT_HARD_MAX_DIMENSION);
  // 取整可能把乘积顶过像素上限（边长 √10 亿、两边都向上取整就多出十几万像素），而原生侧
  // 按同一个数字拒绝：从长边扣掉多出来的像素，宁可少一像素也不让「前端算得出来、原生拒绝」。
  while (width * height > CANVAS_EXPORT_HARD_MAX_PIXELS) {
    if (width >= height) width -= 1;
    else height -= 1;
  }
  // 倍率按最终整数尺寸回推：块坐标到世界坐标的映射与「输出 W×H」因此严格一致。
  const effectiveScale = Math.min(scale, width / padded.width, height / padded.height);
  const suggested = Number.isFinite(limit) && limit > 0
    ? limit
    : CANVAS_EXPORT_MAX_DIMENSION.medium;
  return {
    scale: effectiveScale,
    width,
    height,
    overLimit: Math.max(width, height) > suggested,
  };
}

/**
 * 行带高度：按「整行 × 4 字节」塞进 32 MiB 的目标反推，并夹在 128–2048 之间。
 *
 * 8000 宽 → 1048；20000 宽 → 419；30000 宽 → 279。行带越低，块数越多、IPC 往返越多；
 * 越高则常驻内存越大，因此两头都要夹。
 */
export function canvasExportBandHeight(width: number): number {
  const rowBytes = Math.max(1, Math.floor(width)) * 4;
  return clamp(
    Math.floor(CANVAS_EXPORT_BAND_MEMORY_BYTES / rowBytes),
    CANVAS_EXPORT_BAND_MIN_HEIGHT,
    CANVAS_EXPORT_BAND_MAX_HEIGHT,
  );
}

export interface CanvasExportPlan extends CanvasExportSize {
  /** 含边距的输出区域（世界单位）。 */
  bounds: Bounds;
  requestedScale: number;
  margin: number;
  /** 建议上限（只用于提示）。 */
  limit: number;
  /** 单块宽度（不超过 `tileWidthLimit`）。 */
  tileWidth: number;
  tilesPerBand: number;
  bandHeight: number;
  bandCount: number;
  totalTiles: number;
}

/**
 * 完整导出计划。`contentBounds` 为空（画板没有可见内容）时返回 null。
 *
 * `tileWidthLimit` 给渲染器传入设备实际允许的单张纹理边长（可能低于 8192）。
 */
export function resolveCanvasExportPlan(
  contentBounds: Bounds,
  requestedScale: number,
  margin: number,
  limit: number,
  tileWidthLimit: number = CANVAS_EXPORT_TILE_MAX_WIDTH,
): CanvasExportPlan | null {
  const safeMargin = clampCanvasExportMargin(margin);
  const size = resolveCanvasExportScale(contentBounds, requestedScale, safeMargin, limit);
  if (!size) return null;
  const bounds: Bounds = {
    minX: contentBounds.minX - safeMargin,
    minY: contentBounds.minY - safeMargin,
    maxX: contentBounds.maxX + safeMargin,
    maxY: contentBounds.maxY + safeMargin,
  };
  const finiteTileLimit = Number.isFinite(tileWidthLimit)
    ? Math.max(1, Math.floor(tileWidthLimit))
    : CANVAS_EXPORT_TILE_MAX_WIDTH;
  const tileWidth = Math.min(CANVAS_EXPORT_TILE_MAX_WIDTH, finiteTileLimit, size.width);
  const tilesPerBand = Math.max(1, Math.ceil(size.width / tileWidth));
  const bandHeight = Math.min(size.height, canvasExportBandHeight(size.width));
  const bandCount = Math.max(1, Math.ceil(size.height / bandHeight));
  return {
    ...size,
    bounds,
    requestedScale,
    margin: safeMargin,
    limit,
    tileWidth,
    tilesPerBand,
    bandHeight,
    bandCount,
    totalTiles: tilesPerBand * bandCount,
  };
}

/** 一块导出块在输出像素坐标系里的位置与尺寸。 */
export interface CanvasExportTile {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 按「行带优先、行内从左到右」的顺序展开全部块——原生侧按这个顺序逐行拼装。 */
export function canvasExportTiles(plan: CanvasExportPlan): CanvasExportTile[] {
  const tiles: CanvasExportTile[] = [];
  for (let band = 0; band < plan.bandCount; band += 1) {
    const y = band * plan.bandHeight;
    const height = Math.min(plan.bandHeight, plan.height - y);
    for (let column = 0; column < plan.tilesPerBand; column += 1) {
      const x = column * plan.tileWidth;
      tiles.push({
        x,
        y,
        width: Math.min(plan.tileWidth, plan.width - x),
        height,
      });
    }
  }
  return tiles;
}
