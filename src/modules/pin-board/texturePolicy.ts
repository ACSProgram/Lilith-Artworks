import type { PinBoardImage } from "./types";

type TextureSource = Pick<PinBoardImage, "width" | "height">;

export type PinBoardTextureCacheLevel = "low" | "medium" | "high";

export interface PinBoardTextureBudgets {
  /** 常驻缩略图缓存预算。 */
  previewBytes: number;
  /** 高清纹理 LRU 缓存预算。 */
  residentBytes: number;
}

/** 默认缓存等级（中等）。 */
export const DEFAULT_PIN_BOARD_TEXTURE_CACHE_LEVEL: PinBoardTextureCacheLevel = "medium";

/**
 * 缓存等级 → 前端 GPU 缓存预算（常驻缩略图 + 高清 LRU）。三个等级对应的
 * 缓存占用约为 256 / 512 / 1024 MiB（含 Rust 解码结果缓存与瞬时上传预算）；
 * Rust 侧解码缓存预算见 `pin_board/mod.rs` 的 `texture_cache_budget_bytes`。
 */
export function textureBudgetsForLevel(
  level: PinBoardTextureCacheLevel | undefined,
): PinBoardTextureBudgets {
  switch (level) {
    case "low":
      return { previewBytes: 64 * 1024 * 1024, residentBytes: 128 * 1024 * 1024 };
    case "high":
      return { previewBytes: 256 * 1024 * 1024, residentBytes: 512 * 1024 * 1024 };
    case "medium":
    default:
      return { previewBytes: 128 * 1024 * 1024, residentBytes: 256 * 1024 * 1024 };
  }
}

const DDS_HEADER_BYTES = 148;
const RGBA_BYTES_PER_PIXEL = 4;
export const GPU_BYTES_PER_ROW_ALIGNMENT = 256;
export const BC7_BLOCK_DIMENSION = 4;

export const PIN_BOARD_NATIVE_TEXTURE_LIMIT = 8192;
export const PIN_BOARD_CANVAS_DIMENSION_LIMIT = 4096;
export const PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES = 64 * 1024 * 1024;
// Keep one upload below the resident budget so a replacement can be staged
// without making the browser allocate an unbounded transient buffer.
export const PIN_BOARD_TEXTURE_UPLOAD_BUDGET_BYTES = 96 * 1024 * 1024;
export const PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES = 128 * 1024 * 1024;
export const PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES = 16 * 1024 * 1024;
export const PIN_BOARD_TEXTURE_LOAD_BUDGET_BYTES = 96 * 1024 * 1024;
// Resident budget for the always-cached preview (thumbnail) textures. Keep one
// 256px RGBA preview (~256 KiB) per visited board image without evicting them
// during normal panning; the previews are only evicted under this separate cap,
// so a scrolled-away image still has something to draw when it comes back.
export const PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES = 64 * 1024 * 1024;
export const PIN_BOARD_MIN_PREVIEW_DIMENSION = 256;

export interface TextureUploadLayout {
  data: Uint8Array;
  bytesPerRow: number;
  rowsPerImage: number;
}

/**
 * BC7 stores one 4x4 texel block even when the logical image edge is partial.
 * WebGPU validates the texture extent against that physical block grid, so
 * callers must use these dimensions when creating a compressed texture.
 */
export function bc7TextureDimensions(width: number, height: number): [number, number] {
  const safeWidth = Number.isFinite(width) ? Math.max(1, Math.floor(width)) : 1;
  const safeHeight = Number.isFinite(height) ? Math.max(1, Math.floor(height)) : 1;
  return [
    Math.ceil(safeWidth / BC7_BLOCK_DIMENSION) * BC7_BLOCK_DIMENSION,
    Math.ceil(safeHeight / BC7_BLOCK_DIMENSION) * BC7_BLOCK_DIMENSION,
  ];
}

/**
 * Maps logical image UVs into a BC7 texture that may contain a partial edge
 * block. The source image dimensions remain unchanged in the board model.
 */
export function bc7TextureUvScale(width: number, height: number): [number, number] {
  const [textureWidth, textureHeight] = bc7TextureDimensions(width, height);
  const safeWidth = Number.isFinite(width) ? Math.max(1, Math.floor(width)) : 1;
  const safeHeight = Number.isFinite(height) ? Math.max(1, Math.floor(height)) : 1;
  return [safeWidth / textureWidth, safeHeight / textureHeight];
}

export function textureUploadLayout(
  data: Uint8Array,
  sourceBytesPerRow: number,
  rowsPerImage: number,
): TextureUploadLayout {
  const rowBytes = Math.max(1, Math.floor(sourceBytesPerRow));
  const rows = Math.max(1, Math.floor(rowsPerImage));
  const sourceBytes = rowBytes * rows;
  if (!Number.isSafeInteger(sourceBytes) || data.byteLength < sourceBytes) {
    throw new Error("纹理行数据不完整");
  }
  const bytesPerRow = Math.ceil(rowBytes / GPU_BYTES_PER_ROW_ALIGNMENT)
    * GPU_BYTES_PER_ROW_ALIGNMENT;
  const paddedBytes = bytesPerRow * rows;
  if (!Number.isSafeInteger(paddedBytes)) {
    throw new Error("纹理行数据过大");
  }
  if (bytesPerRow === rowBytes) {
    return {
      data: data.byteLength === sourceBytes ? data : data.subarray(0, sourceBytes),
      bytesPerRow,
      rowsPerImage: rows,
    };
  }
  const padded = new Uint8Array(paddedBytes);
  for (let row = 0; row < rows; row += 1) {
    padded.set(
      data.subarray(row * rowBytes, (row + 1) * rowBytes),
      row * bytesPerRow,
    );
  }
  return { data: padded, bytesPerRow, rowsPerImage: rows };
}

function alignedBytesPerRow(rowBytes: number): number {
  return Math.ceil(Math.max(1, rowBytes) / GPU_BYTES_PER_ROW_ALIGNMENT)
    * GPU_BYTES_PER_ROW_ALIGNMENT;
}

function sourceDimensions(source: TextureSource): [number, number] {
  return [
    Number.isFinite(source.width) ? Math.max(1, Math.floor(source.width)) : 1,
    Number.isFinite(source.height) ? Math.max(1, Math.floor(source.height)) : 1,
  ];
}

export function previewDimensions(
  source: TextureSource,
  maxDimension: number,
): [number, number] {
  const [width, height] = sourceDimensions(source);
  const sourceMax = Math.max(width, height);
  const finiteMax = Number.isFinite(maxDimension) ? Math.floor(maxDimension) : sourceMax;
  const targetMax = Math.max(1, Math.min(sourceMax, finiteMax));
  const scale = targetMax / sourceMax;
  return [
    Math.min(targetMax, Math.max(1, Math.round(width * scale))),
    Math.min(targetMax, Math.max(1, Math.round(height * scale))),
  ];
}

export function rgbaPreviewBytes(source: TextureSource, maxDimension: number): number {
  const [width, height] = previewDimensions(source, maxDimension);
  return width * height * RGBA_BYTES_PER_PIXEL;
}

export function rgbaPreviewDimensionLimit(
  source: TextureSource,
  byteBudget = PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
): number {
  const [width, height] = sourceDimensions(source);
  const sourceMax = Math.max(width, height);
  const budget = Number.isFinite(byteBudget)
    ? Math.max(RGBA_BYTES_PER_PIXEL, Math.floor(byteBudget))
    : PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES;
  const approximate = Math.floor(Math.sqrt(
    budget * sourceMax * sourceMax / (RGBA_BYTES_PER_PIXEL * width * height),
  ));
  let candidate = Math.max(1, Math.min(sourceMax, approximate));
  while (candidate > 1 && rgbaPreviewBytes(source, candidate) > budget) candidate -= 1;
  return candidate;
}

export function requestedTextureDimension(
  source: TextureSource,
  displayDimension: number,
  deviceLimit: number,
): number {
  const [width, height] = sourceDimensions(source);
  const sourceMax = Math.max(width, height);
  const finiteDeviceLimit = Number.isFinite(deviceLimit)
    ? Math.max(1, Math.floor(deviceLimit))
    : PIN_BOARD_NATIVE_TEXTURE_LIMIT;
  const hardLimit = Math.min(PIN_BOARD_NATIVE_TEXTURE_LIMIT, finiteDeviceLimit);
  const finiteDisplayDimension = Number.isFinite(displayDimension)
    ? Math.max(0, displayDimension)
    : Number.POSITIVE_INFINITY;
  const desiredPreview = Math.max(
    PIN_BOARD_MIN_PREVIEW_DIMENSION,
    finiteDisplayDimension * 1.25,
  );

  // When the native side can return the source BC7 texture, full resolution
  // uses at most 64 MiB and is cheaper than a similarly sized RGBA preview.
  if (sourceMax <= hardLimit && desiredPreview >= sourceMax * 0.5) {
    return sourceMax;
  }

  const previewLimit = rgbaPreviewDimensionLimit(source);
  return Math.max(1, Math.round(Math.min(
    sourceMax,
    hardLimit,
    previewLimit,
    desiredPreview,
  )));
}

export function estimatedTextureLoadBytes(
  source: TextureSource,
  maxDimension: number,
): number {
  const [width, height] = sourceDimensions(source);
  const sourceBytes = Math.ceil(width / 4) * Math.ceil(height / 4) * 16 + DDS_HEADER_BYTES;
  if (Math.max(width, height) <= maxDimension) return sourceBytes * 2;
  return sourceBytes + rgbaPreviewBytes(source, maxDimension) * 3;
}

export function estimatedTextureResidentBytes(
  source: TextureSource,
  maxDimension: number,
): number {
  const [width, height] = sourceDimensions(source);
  const sourceMax = Math.max(width, height);
  if (sourceMax <= maxDimension) {
    return alignedBytesPerRow(Math.ceil(width / 4) * 16) * Math.ceil(height / 4);
  }
  const [previewWidth, previewHeight] = previewDimensions(source, maxDimension);
  return alignedBytesPerRow(previewWidth * RGBA_BYTES_PER_PIXEL) * previewHeight;
}
