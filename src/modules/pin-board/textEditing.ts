/**
 * 文字就地编辑的纯换算：有效字号、屏幕几何、草稿校验与文字签名。
 *
 * 这里不碰渲染器、React 或 Tauri；渲染器负责会话与模型，`PinBoardTextEditor`
 * 只负责把这里算出的屏幕框画出来。
 */

import { imageSize, quadCenter, worldToScreen, type Point, type Quad, type Viewport } from "./geometry";
import {
  PIN_BOARD_TEXT_MAX_CHARS,
  PIN_BOARD_TEXT_MAX_DIMENSION,
  measurePinBoardText,
} from "./textLayout";
import type { PinBoardTextAlign, PinBoardTextItem } from "./types";

/** 字体栏的有效字号输入口径，与旧“添加文字”对话框一致。 */
export const PIN_BOARD_TEXT_EDIT_MIN_FONT_SIZE = 12;
export const PIN_BOARD_TEXT_EDIT_MAX_FONT_SIZE = 240;
/** 与 Rust 文字载荷校验一致：数据库字号不得超过该值。 */
export const PIN_BOARD_TEXT_MAX_DATABASE_FONT_SIZE = 1000;

export interface PinBoardTextDraft {
  text: string;
  /** 数据库里的世界单位字号；有效字号 = fontSize × 条目缩放。 */
  fontSize: number;
  color: string;
  align: PinBoardTextAlign;
}

export interface PinBoardTextScale {
  /** 四边形宽 ÷ 文字测量宽。 */
  x: number;
  /** 四边形高 ÷ 文字测量高。四角缩放与工具栏缩放都是等比的，通常等于 `x`。 */
  y: number;
}

export interface PinBoardTextEditGeometry {
  /** 编辑框中心（CSS 像素，相对画布左上角）。 */
  centerX: number;
  centerY: number;
  /** 屏幕旋转角（CSS 的 y 轴向下，顺时针为正）。 */
  angle: number;
  /** CSS 像素 / 世界单位。 */
  pixelsPerWorldUnit: number;
}

export interface PinBoardTextScreenMetrics {
  screenFontSize: number;
  width: number;
  height: number;
}

export type PinBoardTextDraftResult =
  | {
    ok: true;
    width: number;
    height: number;
    fontSize: number;
    color: string;
    align: PinBoardTextAlign;
    points: Quad;
  }
  | { ok: false; error: string };

export function pinBoardTextScale(
  item: Pick<PinBoardTextItem, "points" | "width" | "height">,
): PinBoardTextScale {
  const [width, height] = imageSize(item);
  return {
    x: width / Math.max(item.width, 1e-9),
    y: height / Math.max(item.height, 1e-9),
  };
}

/** 字体栏显示的用户可见字号，单位是世界单位。 */
export function effectivePinBoardFontSize(fontSize: number, scale: number): number {
  return fontSize * scale;
}

/** 用户修改有效字号后反算数据库字号：条目缩放保持不变。 */
export function databaseFontSizeFor(effective: number, scale: number): number {
  return effective / Math.max(scale, 1e-9);
}

/**
 * 文字签名：保存 payload 的取舍口径。
 *
 * 颜色统一小写，因为取色器输出小写、仓库返回大写；两处必须视为同一个值，
 * 否则每次保存都会误判成“文字变了”。
 */
export function pinBoardTextSignature(
  item: Pick<PinBoardTextItem, "text" | "fontSize" | "color" | "align" | "width" | "height">,
): string {
  return JSON.stringify([
    item.text,
    item.fontSize,
    item.color.toLowerCase(),
    item.align,
    item.width,
    item.height,
  ]);
}

export function textDraftFromItem(item: PinBoardTextItem): PinBoardTextDraft {
  return {
    text: item.text,
    fontSize: item.fontSize,
    color: item.color.toLowerCase(),
    align: item.align,
  };
}

/**
 * 围绕原中心、沿原旋转方向重建文字四边形。
 *
 * 世界坐标 y 轴向上，角点顺序与 `PinBoardItem.points` 一致：
 * 左上 → 右上 → 右下 → 左下。
 */
export function buildTextQuad(center: Point, angle: number, width: number, height: number): Quad {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  const corner = (x: number, y: number): Point => [
    center[0] + x * cosine - y * sine,
    center[1] + x * sine + y * cosine,
  ];
  return [
    corner(-width / 2, height / 2),
    corner(width / 2, height / 2),
    corner(width / 2, -height / 2),
    corner(-width / 2, -height / 2),
  ];
}

/** 编辑框的屏幕中心、旋转角与比例；缩放/平移/窗口变化后重新计算。 */
export function pinBoardTextEditGeometry(
  item: Pick<PinBoardTextItem, "points">,
  rect: Pick<DOMRect, "width" | "height">,
  viewport: Viewport,
): PinBoardTextEditGeometry {
  const center = quadCenter(item.points);
  const [centerX, centerY] = worldToScreen(center, rect, viewport);
  const [leftTopX, leftTopY] = worldToScreen(item.points[0], rect, viewport);
  const [rightTopX, rightTopY] = worldToScreen(item.points[1], rect, viewport);
  return {
    centerX,
    centerY,
    angle: Math.atan2(rightTopY - leftTopY, rightTopX - leftTopX),
    pixelsPerWorldUnit: rect.height / Math.max(viewport.height, 1e-9),
  };
}

/** 编辑框的屏幕字号与宽高：随内容增长，始终以四边形中心为锚点。 */
export function pinBoardTextScreenMetrics(
  draft: Pick<PinBoardTextDraft, "text" | "fontSize">,
  scale: number,
  pixelsPerWorldUnit: number,
): PinBoardTextScreenMetrics {
  const screenFontSize = Math.max(1, draft.fontSize * scale * pixelsPerWorldUnit);
  const metrics = measurePinBoardText(draft.text, screenFontSize);
  return {
    screenFontSize,
    width: Math.max(metrics.width, 2),
    height: Math.max(metrics.height, 2),
  };
}

/** 与 Rust 一致的字符口径：按 Unicode 码点计数，不按 UTF-16 单元。 */
export function pinBoardTextCharCount(text: string): number {
  return [...text].length;
}

/**
 * 校验并解算一次文字草稿：返回新的测量宽高、归一化颜色与重建后的四边形。
 *
 * 空内容由调用方按取消处理；这里仍拒绝，避免绕过入口写入空文字。
 */
export function resolvePinBoardTextDraft(
  draft: PinBoardTextDraft,
  scale: PinBoardTextScale,
  center: Point,
  angle: number,
): PinBoardTextDraftResult {
  if (draft.text.trim().length === 0) {
    return { ok: false, error: "文字内容不能为空" };
  }
  if (pinBoardTextCharCount(draft.text) > PIN_BOARD_TEXT_MAX_CHARS) {
    return { ok: false, error: `文字内容不能超过 ${PIN_BOARD_TEXT_MAX_CHARS} 个字符` };
  }
  if (!Number.isFinite(draft.fontSize)
    || draft.fontSize <= 0
    || draft.fontSize > PIN_BOARD_TEXT_MAX_DATABASE_FONT_SIZE) {
    return { ok: false, error: "文字字号超出允许范围" };
  }
  const color = draft.color.toLowerCase();
  if (!/^#[0-9a-f]{6}$/.test(color)) {
    return { ok: false, error: "文字颜色无效" };
  }
  if (!matchesAlign(draft.align)) {
    return { ok: false, error: "文字对齐方式无效" };
  }
  const metrics = measurePinBoardText(draft.text, draft.fontSize);
  if (metrics.width > PIN_BOARD_TEXT_MAX_DIMENSION
    || metrics.height > PIN_BOARD_TEXT_MAX_DIMENSION) {
    return { ok: false, error: "文字尺寸超出素材板上限，请缩短内容或减小字号" };
  }
  return {
    ok: true,
    width: metrics.width,
    height: metrics.height,
    fontSize: draft.fontSize,
    color,
    align: draft.align,
    points: buildTextQuad(center, angle, metrics.width * scale.x, metrics.height * scale.y),
  };
}

function matchesAlign(align: string): align is PinBoardTextAlign {
  return align === "left" || align === "center" || align === "right";
}

/** 世界四边形第一条边（左上 → 右上）的世界旋转角。 */
export function textQuadAngle(quad: Quad): number {
  return Math.atan2(quad[1][1] - quad[0][1], quad[1][0] - quad[0][0]);
}
