import {
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
} from "./texturePolicy";
import type { PinBoardTextItem } from "./types";

/** 与 Rust `pin_board::repository::TEXT_FONT_FAMILY` 保持一致。 */
export const PIN_BOARD_TEXT_FONT_FAMILY =
  '"Microsoft YaHei UI", "Segoe UI", sans-serif';
export const PIN_BOARD_TEXT_LINE_HEIGHT_RATIO = 1.3;
/** 导出倍率：文字按逻辑尺寸的固定倍数栅格化，避免屏幕字体小图被放大。 */
export const PIN_BOARD_TEXT_EXPORT_SCALE = 2;
export const PIN_BOARD_TEXT_MAX_DIMENSION = 8192;

export interface PinBoardTextMetrics {
  lines: string[];
  width: number;
  height: number;
  lineHeight: number;
}

export interface PinBoardTextRaster {
  width: number;
  height: number;
  rgba: Uint8ClampedArray;
}

function textLines(text: string): string[] {
  return text.replace(/\r\n?/g, "\n").split("\n");
}

function createCanvas(
  width: number,
  height: number,
): { canvas: HTMLCanvasElement; context: CanvasRenderingContext2D } {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("无法创建文字画布");
  return { canvas, context };
}

/** 度量只需 `measureText`：复用一块 1×1 画布，避免每次量字都新建 canvas。 */
let measureContext: CanvasRenderingContext2D | null = null;

function metricContext(): CanvasRenderingContext2D {
  measureContext ??= createCanvas(1, 1).context;
  return measureContext;
}

/** 屏幕显示与导出共用的字体度量：行宽取最长行，行高固定比例。 */
export function measurePinBoardText(text: string, fontSize: number): PinBoardTextMetrics {
  const safeSize = Number.isFinite(fontSize) ? Math.max(1, fontSize) : 1;
  const lines = textLines(text);
  const lineHeight = Math.ceil(safeSize * PIN_BOARD_TEXT_LINE_HEIGHT_RATIO);
  const context = metricContext();
  context.font = `${safeSize}px ${PIN_BOARD_TEXT_FONT_FAMILY}`;
  const width = Math.max(
    1,
    Math.ceil(Math.max(...lines.map((line) => context.measureText(line || " ").width))),
  );
  return {
    lines,
    width,
    height: Math.max(1, lineHeight * lines.length),
    lineHeight,
  };
}

function budgetedTextDimension(item: PinBoardTextItem): number {
  const long = Math.max(item.width, item.height);
  const short = Math.min(item.width, item.height);
  const ratio = short > 0 ? short / long : 1;
  const approximate = Math.floor(
    Math.sqrt(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES / (4 * Math.max(ratio, 1e-6))),
  );
  let dimension = Math.max(1, Math.min(PIN_BOARD_TEXT_MAX_DIMENSION, approximate));
  // 与渲染器读取 RGBA 负载时的上限对齐：取整后的面积也不得超过预算。
  while (
    dimension > 1
    && textTextureRgbaBytes(item, dimension) > PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES
  ) {
    dimension -= 1;
  }
  return dimension;
}

/** 请求长边决定纹理清晰度；与位图不同，文字可以超过逻辑像素尺寸。 */
export function requestedTextTextureDimension(
  item: PinBoardTextItem,
  displayDimension: number,
  deviceLimit: number,
): number {
  const finiteDeviceLimit = Number.isFinite(deviceLimit)
    ? Math.max(1, Math.floor(deviceLimit))
    : PIN_BOARD_NATIVE_TEXTURE_LIMIT;
  const cap = Math.min(
    PIN_BOARD_NATIVE_TEXTURE_LIMIT,
    finiteDeviceLimit,
    budgetedTextDimension(item),
  );
  const desired = Math.max(256, displayDimension * 1.25);
  return Math.max(1, Math.round(Math.min(cap, desired)));
}

/** 按请求长边缩放后的实际纹理尺寸。 */
export function textTextureDimensions(
  item: PinBoardTextItem,
  maxDimension: number,
): [number, number] {
  const long = Math.max(item.width, item.height);
  const scale = maxDimension / Math.max(long, 1);
  return [
    Math.max(1, Math.min(maxDimension, Math.round(item.width * scale))),
    Math.max(1, Math.min(maxDimension, Math.round(item.height * scale))),
  ];
}

export function textTextureRgbaBytes(
  item: PinBoardTextItem,
  maxDimension: number,
): number {
  const [width, height] = textTextureDimensions(item, maxDimension);
  return width * height * 4;
}

export function textTextureResidentBytes(
  item: PinBoardTextItem,
  maxDimension: number,
): number {
  const [width, height] = textTextureDimensions(item, maxDimension);
  const bytesPerRow = Math.ceil((width * 4) / 256) * 256;
  return bytesPerRow * height;
}

interface DrawnText {
  canvas: HTMLCanvasElement;
  context: CanvasRenderingContext2D;
  width: number;
  height: number;
}

/** 按请求长边把文字画到离屏 canvas；显示纹理与导出共用这一段绘制逻辑。 */
function drawPinBoardText(item: PinBoardTextItem, maxDimension: number): DrawnText {
  const metrics = measurePinBoardText(item.text, item.fontSize);
  const [width, height] = textTextureDimensions(item, maxDimension);
  const scale = Math.min(width / metrics.width, height / metrics.height);
  const { canvas, context } = createCanvas(width, height);
  context.clearRect(0, 0, width, height);
  context.font = `${item.fontSize * scale}px ${PIN_BOARD_TEXT_FONT_FAMILY}`;
  context.fillStyle = item.color;
  context.textBaseline = "top";
  context.textAlign = item.align;
  const x = item.align === "left" ? 0 : item.align === "right" ? width : width / 2;
  metrics.lines.forEach((line, index) => {
    context.fillText(line, x, index * metrics.lineHeight * scale);
  });
  return { canvas, context, width, height };
}

export function rasterizePinBoardText(
  item: PinBoardTextItem,
  maxDimension: number,
): PinBoardTextRaster {
  const { context, width, height } = drawPinBoardText(item, maxDimension);
  return {
    width,
    height,
    rgba: context.getImageData(0, 0, width, height).data,
  };
}

/**
 * 导出用的文字载荷：浏览器把它编码成 PNG 再交给原生端写盘。
 *
 * 原生端要以 RGBA 收同样的图需要跨 IPC 传数千万个数字（最长边 8192 时约
 * 64 MiB 像素），文案 PNG 则是几十到几百 KB。原生端只校验格式与尺寸后原子
 * 写盘，不再重新编码。
 */
export async function rasterizePinBoardTextPng(
  item: PinBoardTextItem,
  maxDimension: number,
): Promise<Uint8Array> {
  const { canvas } = drawPinBoardText(item, maxDimension);
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) resolve(result);
      else reject(new Error("无法生成文字图片"));
    }, "image/png");
  });
  return new Uint8Array(await blob.arrayBuffer());
}

/**
 * 与 Rust 纹理读取一致的 RGBA 负载：`"RGBA"` + 小端宽高 + 像素。
 * 渲染器复用既有 `isRgbaPayload` 分支上传。
 */
export function textRgbaPayload(
  item: PinBoardTextItem,
  maxDimension: number,
): Uint8Array {
  const { width, height, rgba } = rasterizePinBoardText(item, maxDimension);
  const payload = new Uint8Array(12 + rgba.byteLength);
  payload[0] = 0x52;
  payload[1] = 0x47;
  payload[2] = 0x42;
  payload[3] = 0x41;
  const view = new DataView(payload.buffer);
  view.setUint32(4, width, true);
  view.setUint32(8, height, true);
  payload.set(rgba, 12);
  return payload;
}

/** 导出分辨率：逻辑长边 × 固定倍率，并受像素上限与 RGBA 预算约束。 */
export function exportTextMaxDimension(item: PinBoardTextItem): number {
  const long = Math.max(item.width, item.height);
  return Math.max(
    1,
    Math.min(
      PIN_BOARD_TEXT_MAX_DIMENSION,
      budgetedTextDimension(item),
      Math.round(long * PIN_BOARD_TEXT_EXPORT_SCALE),
    ),
  );
}
