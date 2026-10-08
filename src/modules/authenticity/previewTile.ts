import type { PreviewViewport } from "./previewViewport";

/** 高清局部请求：矩形为源图（成品或原始成品）坐标系内的像素区域。 */
export interface TileRequest {
  x: number;
  y: number;
  width: number;
  height: number;
  maxEdge: number;
}

/**
 * 可视区域四周额外保留的比例，同时作为矩形起点与终点的吸附步长。
 *
 * 步长与边距取同一比例，是为了让小幅平移落在同一个吸附格内、复用同一块局部，
 * 而不是每移动一个像素就重新请求；边距保证吸附后仍然完整覆盖可视区域。
 */
const TILE_MARGIN = 0.25;
const TILE_MIN_MAX_EDGE = 256;
const TILE_MAX_MAX_EDGE = 4096;
/** 吸附取整的容差，抵消浮点误差；远小于最小格宽（1 / 4096）。 */
const SNAP_EPSILON = 1e-6;

/**
 * 根据当前视口计算需要的高清局部请求。
 *
 * 坐标换算只依赖已知的几何量：显示尺寸 `displayWidth/displayHeight`、内容盒
 * `max(视口, 显示尺寸)` 与居中偏移，不依赖 DOM 测量。
 *
 * 只有在缩略图被放大（显示尺寸超过缩略图本身）时才请求局部：此时浏览器会插值
 * 放大 2400 px 缩略图，必须换成源像素；缩略图未被放大时直接显示缩略图就已经是
 * 1:1 或更清晰，请求局部只会浪费带宽。
 *
 * `maxEdge` 按局部在屏幕上的显示尺寸给出，让后端把裁剪块缩放到正好一源像素对一
 * 屏幕像素，既不被浏览器放大，也不会传输看不到的像素。
 */
export function tileRequestForView({
  displayWidth,
  displayHeight,
  thumbWidth,
  thumbHeight,
  sourceWidth,
  sourceHeight,
  viewport,
}: {
  displayWidth: number;
  displayHeight: number;
  thumbWidth: number;
  thumbHeight: number;
  sourceWidth: number;
  sourceHeight: number;
  viewport: PreviewViewport | null;
}): TileRequest | null {
  if (!viewport) return null;
  if (displayWidth <= 0 || displayHeight <= 0) return null;
  if (thumbWidth <= 0 || thumbHeight <= 0 || sourceWidth <= 0 || sourceHeight <= 0) return null;
  // 缩略图已是源分辨率：显示缩略图即为 1:1，没有更高分辨率的像素可取。
  if (thumbWidth >= sourceWidth && thumbHeight >= sourceHeight) return null;
  // 缩略图尚未被放大：当前显示已经是 1:1 或降采样，局部不会更清晰。
  if (displayWidth <= thumbWidth && displayHeight <= thumbHeight) return null;
  const offsetX = Math.max(0, (Math.max(viewport.clientWidth, displayWidth) - displayWidth) / 2);
  const offsetY = Math.max(0, (Math.max(viewport.clientHeight, displayHeight) - displayHeight) / 2);
  const visibleLeft = Math.min(Math.max(viewport.scrollLeft - offsetX, 0), displayWidth);
  const visibleTop = Math.min(Math.max(viewport.scrollTop - offsetY, 0), displayHeight);
  const visibleWidth = Math.min(viewport.clientWidth, displayWidth - visibleLeft);
  const visibleHeight = Math.min(viewport.clientHeight, displayHeight - visibleTop);
  if (visibleWidth <= 0 || visibleHeight <= 0) return null;
  const scaleX = sourceWidth / displayWidth;
  const scaleY = sourceHeight / displayHeight;
  // 可视区域在源坐标下的跨度，以及一块局部的吸附步长。
  const spanX = visibleWidth * scaleX;
  const spanY = visibleHeight * scaleY;
  const stepX = Math.max(1, Math.floor(spanX * TILE_MARGIN));
  const stepY = Math.max(1, Math.floor(spanY * TILE_MARGIN));
  const startX = Math.max(0, visibleLeft * scaleX - spanX * TILE_MARGIN);
  const startY = Math.max(0, visibleTop * scaleY - spanY * TILE_MARGIN);
  // 起点向下吸附、终点向上吸附：吸附后的矩形一定仍然覆盖 [可视起点, 可视终点]。
  const x = Math.floor(startX / stepX + SNAP_EPSILON) * stepX;
  const y = Math.floor(startY / stepY + SNAP_EPSILON) * stepY;
  const right = Math.min(
    sourceWidth,
    Math.ceil((visibleLeft * scaleX + spanX * (1 + TILE_MARGIN)) / stepX - SNAP_EPSILON) * stepX,
  );
  const bottom = Math.min(
    sourceHeight,
    Math.ceil((visibleTop * scaleY + spanY * (1 + TILE_MARGIN)) / stepY - SNAP_EPSILON) * stepY,
  );
  const width = right - x;
  const height = bottom - y;
  if (width <= 0 || height <= 0) return null;
  // 局部在屏幕上按 `width / scaleX` × `height / scaleY` 的 CSS 尺寸呈现，输出边长与
  // 之相等即可让每个源像素正好落到一个屏幕像素上。
  const maxEdge = Math.min(
    TILE_MAX_MAX_EDGE,
    Math.max(TILE_MIN_MAX_EDGE, Math.ceil(Math.max(width / scaleX, height / scaleY))),
  );
  return { x, y, width, height, maxEdge };
}

export function tileCacheKey(request: TileRequest): string {
  return `${request.x},${request.y},${request.width},${request.height},${request.maxEdge}`;
}

/** 局部图相对显示图片的定位（百分比），由外层按显示尺寸定界的容器解释。 */
export function tileOverlayStyle(
  request: TileRequest,
  sourceWidth: number,
  sourceHeight: number,
): { left: string; top: string; width: string; height: string } {
  return {
    left: `${(request.x / sourceWidth) * 100}%`,
    top: `${(request.y / sourceHeight) * 100}%`,
    width: `${(request.width / sourceWidth) * 100}%`,
    height: `${(request.height / sourceHeight) * 100}%`,
  };
}
