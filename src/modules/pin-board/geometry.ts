import type { PinBoardImage } from "./types";

export interface Viewport {
  centerX: number;
  centerY: number;
  height: number;
}

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export type Point = [number, number];
export type Quad = [Point, Point, Point, Point];

export const DEFAULT_VIEWPORT: Viewport = {
  centerX: 0,
  centerY: 0,
  height: 5000,
};

export function imageBounds(image: Pick<PinBoardImage, "points">): Bounds {
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;

  for (const [x, y] of image.points) {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  }

  return { minX, minY, maxX, maxY };
}

export function viewportBounds(viewport: Viewport, width: number, height: number): Bounds {
  const aspect = Math.max(width, 1) / Math.max(height, 1);
  const halfHeight = viewport.height / 2;
  const halfWidth = halfHeight * aspect;
  return {
    minX: viewport.centerX - halfWidth,
    minY: viewport.centerY - halfHeight,
    maxX: viewport.centerX + halfWidth,
    maxY: viewport.centerY + halfHeight,
  };
}

export function boundsCenter(bounds: Bounds): Point {
  return [
    (bounds.minX + bounds.maxX) / 2,
    (bounds.minY + bounds.maxY) / 2,
  ];
}

export function imagesBounds(images: Pick<PinBoardImage, "points">[]): Bounds | null {
  if (images.length === 0) return null;
  return images.reduce<Bounds>((bounds, image) => {
    const next = imageBounds(image);
    return {
      minX: Math.min(bounds.minX, next.minX),
      minY: Math.min(bounds.minY, next.minY),
      maxX: Math.max(bounds.maxX, next.maxX),
      maxY: Math.max(bounds.maxY, next.maxY),
    };
  }, {
    minX: Number.POSITIVE_INFINITY,
    minY: Number.POSITIVE_INFINITY,
    maxX: Number.NEGATIVE_INFINITY,
    maxY: Number.NEGATIVE_INFINITY,
  });
}

/** 自动适配视图时在画布四周保留的边距（CSS 像素）。 */
export const VIEWPORT_FIT_MARGIN_CSS_PIXELS = 24;

/**
 * 计算恰好包含 `bounds` 的视图：等比缩放到画布内并保留固定边距，视图中心对齐
 * 包围框中心。调用方负责把结果收敛到画布允许的高度区间。
 */
export function fitViewportToBounds(
  bounds: Bounds,
  cssWidth: number,
  cssHeight: number,
  marginCssPixels: number = VIEWPORT_FIT_MARGIN_CSS_PIXELS,
): Viewport {
  const width = Math.max(cssWidth, 1);
  const height = Math.max(cssHeight, 1);
  const margin = Math.max(0, marginCssPixels);
  const usableWidth = Math.max(width - margin * 2, 1);
  const usableHeight = Math.max(height - margin * 2, 1);
  const boundsWidth = Math.max(bounds.maxX - bounds.minX, 1e-6);
  const boundsHeight = Math.max(bounds.maxY - bounds.minY, 1e-6);
  const worldUnitsPerCssPixel = Math.max(boundsWidth / usableWidth, boundsHeight / usableHeight);
  return {
    centerX: (bounds.minX + bounds.maxX) / 2,
    centerY: (bounds.minY + bounds.maxY) / 2,
    height: worldUnitsPerCssPixel * height,
  };
}

export function intersects(left: Bounds, right: Bounds): boolean {
  return left.maxX >= right.minX
    && left.minX <= right.maxX
    && left.maxY >= right.minY
    && left.minY <= right.maxY;
}

export function imageDisplayMaxDimension(
  image: PinBoardImage,
  viewport: Viewport,
  canvasHeight: number,
): number {
  const distance = (a: [number, number], b: [number, number]) => Math.hypot(a[0] - b[0], a[1] - b[1]);
  const worldWidth = distance(image.points[0], image.points[1]);
  const worldHeight = distance(image.points[0], image.points[3]);
  const devicePixelsPerWorldUnit = Math.max(canvasHeight, 1) / viewport.height;
  return Math.max(worldWidth, worldHeight) * devicePixelsPerWorldUnit;
}

export function screenToWorld(
  clientX: number,
  clientY: number,
  rect: DOMRect,
  viewport: Viewport,
): Point {
  const bounds = viewportBounds(viewport, rect.width, rect.height);
  const x = (clientX - rect.left) / Math.max(rect.width, 1);
  const y = (clientY - rect.top) / Math.max(rect.height, 1);
  return [
    bounds.minX + x * (bounds.maxX - bounds.minX),
    bounds.maxY - y * (bounds.maxY - bounds.minY),
  ];
}

export function worldToScreen(
  point: Point,
  rect: Pick<DOMRect, "width" | "height">,
  viewport: Viewport,
): Point {
  const bounds = viewportBounds(viewport, rect.width, rect.height);
  return [
    (point[0] - bounds.minX) / Math.max(bounds.maxX - bounds.minX, 1e-9) * rect.width,
    (bounds.maxY - point[1]) / Math.max(bounds.maxY - bounds.minY, 1e-9) * rect.height,
  ];
}

export function pointInQuad(point: Point, quad: Quad): boolean {
  let positive = false;
  let negative = false;
  for (let index = 0; index < quad.length; index += 1) {
    const left = quad[index];
    const right = quad[(index + 1) % quad.length];
    const cross = (right[0] - left[0]) * (point[1] - left[1])
      - (right[1] - left[1]) * (point[0] - left[0]);
    if (cross > 1e-7) positive = true;
    if (cross < -1e-7) negative = true;
    if (positive && negative) return false;
  }
  return true;
}

function pointInBounds(point: Point, bounds: Bounds): boolean {
  return point[0] >= bounds.minX
    && point[0] <= bounds.maxX
    && point[1] >= bounds.minY
    && point[1] <= bounds.maxY;
}

function segmentsIntersect(a: Point, b: Point, c: Point, d: Point): boolean {
  if (Math.max(a[0], b[0]) < Math.min(c[0], d[0])
    || Math.max(c[0], d[0]) < Math.min(a[0], b[0])
    || Math.max(a[1], b[1]) < Math.min(c[1], d[1])
    || Math.max(c[1], d[1]) < Math.min(a[1], b[1])) return false;
  const cross = (left: Point, middle: Point, right: Point) => (
    (middle[0] - left[0]) * (right[1] - left[1])
    - (middle[1] - left[1]) * (right[0] - left[0])
  );
  const abC = cross(a, b, c);
  const abD = cross(a, b, d);
  const cdA = cross(c, d, a);
  const cdB = cross(c, d, b);
  return abC * abD <= 0 && cdA * cdB <= 0;
}

export function quadIntersectsBounds(quad: Quad, bounds: Bounds): boolean {
  if (quad.some((point) => pointInBounds(point, bounds))) return true;
  const corners: Quad = [
    [bounds.minX, bounds.maxY],
    [bounds.maxX, bounds.maxY],
    [bounds.maxX, bounds.minY],
    [bounds.minX, bounds.minY],
  ];
  if (corners.some((point) => pointInQuad(point, quad))) return true;
  for (let quadIndex = 0; quadIndex < quad.length; quadIndex += 1) {
    const quadStart = quad[quadIndex];
    const quadEnd = quad[(quadIndex + 1) % quad.length];
    for (let boundsIndex = 0; boundsIndex < corners.length; boundsIndex += 1) {
      if (segmentsIntersect(
        quadStart,
        quadEnd,
        corners[boundsIndex],
        corners[(boundsIndex + 1) % corners.length],
      )) return true;
    }
  }
  return false;
}

export function quadCenter(quad: Quad): Point {
  return [
    quad.reduce((sum, point) => sum + point[0], 0) / 4,
    quad.reduce((sum, point) => sum + point[1], 0) / 4,
  ];
}

export function imageSize(image: Pick<PinBoardImage, "points">): Point {
  const distance = (left: Point, right: Point) => Math.hypot(
    right[0] - left[0],
    right[1] - left[1],
  );
  return [distance(image.points[0], image.points[1]), distance(image.points[0], image.points[3])];
}

export function transformQuad(
  quad: Quad,
  transform: (point: Point) => Point,
): Quad {
  return quad.map((point) => transform([...point] as Point)) as Quad;
}

export function translateQuad(quad: Quad, deltaX: number, deltaY: number): Quad {
  return transformQuad(quad, ([x, y]) => [x + deltaX, y + deltaY]);
}

export function scaleQuad(quad: Quad, scale: number, center: Point): Quad {
  return transformQuad(quad, ([x, y]) => [
    center[0] + (x - center[0]) * scale,
    center[1] + (y - center[1]) * scale,
  ]);
}

export function rotateQuad(quad: Quad, radians: number, center: Point): Quad {
  const cosine = Math.cos(radians);
  const sine = Math.sin(radians);
  return transformQuad(quad, ([x, y]) => {
    const deltaX = x - center[0];
    const deltaY = y - center[1];
    return [
      center[0] + deltaX * cosine - deltaY * sine,
      center[1] + deltaX * sine + deltaY * cosine,
    ];
  });
}

export function imagesCenter(images: Pick<PinBoardImage, "points">[]): Point {
  if (images.length === 0) return [0, 0];
  const centers = images.map((image) => quadCenter(image.points));
  return [
    centers.reduce((sum, point) => sum + point[0], 0) / centers.length,
    centers.reduce((sum, point) => sum + point[1], 0) / centers.length,
  ];
}

export function arrangeImages(
  images: PinBoardImage[],
  center: Point,
  gap: number,
): Map<number, Quad> {
  if (images.length === 0) return new Map();
  const maximumHeight = Math.max(...images.map((image) => imageSize(image)[1]));
  if (maximumHeight < 1e-6) return new Map();
  const nodes = images.map((image, index) => {
    const [width, height] = imageSize(image);
    return {
      imageId: image.imageId,
      index,
      width: width * (maximumHeight / Math.max(height, 1e-6)),
      height: maximumHeight,
    };
  });

  type ArrangementLine = { nodes: typeof nodes; width: number };
  const lineWidth = (line: ArrangementLine) => (
    line.nodes.reduce((total, node) => total + node.width + gap, 0)
  );
  const totalWidth = nodes.reduce((total, node) => total + node.width + gap, 0);
  const maxWidth = Math.sqrt(totalWidth * (maximumHeight + gap));
  const lines: ArrangementLine[] = [];
  let current: ArrangementLine = { nodes: [], width: 0 };
  for (const node of nodes) {
    current.nodes.push(node);
    current.width = lineWidth(current);
    if (current.width > maxWidth) {
      current.nodes.sort((left, right) => right.width - left.width);
      lines.push(current);
      current = { nodes: [], width: 0 };
    }
  }
  if (current.nodes.length > 0) lines.push(current);
  lines.sort((left, right) => left.width - right.width);

  const layoutWidth = maxWidth;

  const arranged = new Map<number, Quad>();
  let y = 0;
  let allCenter: Point = [0, 0];
  let nodeCount = 0;
  for (const currentLine of lines) {
    let x = (layoutWidth - currentLine.width) / 2;
    for (const node of currentLine.nodes) {
      const points: Quad = [
        [x, y],
        [x + node.width, y],
        [x + node.width, y - node.height],
        [x, y - node.height],
      ];
      arranged.set(node.imageId, points);
      allCenter = [
        allCenter[0] + (x + node.width / 2),
        allCenter[1] + (y - node.height / 2),
      ];
      nodeCount += 1;
      x += node.width + gap;
    }
    y += maximumHeight + gap;
  }
  if (nodeCount === 0) return arranged;
  allCenter = [allCenter[0] / nodeCount, allCenter[1] / nodeCount];
  for (const [imageId, points] of arranged) {
    arranged.set(imageId, translateQuad(points, center[0] - allCenter[0], center[1] - allCenter[1]));
  }
  return arranged;
}
