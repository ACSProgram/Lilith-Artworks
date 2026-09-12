export interface PreviewViewport {
  scrollLeft: number;
  scrollTop: number;
  scrollWidth: number;
  scrollHeight: number;
  clientWidth: number;
  clientHeight: number;
}

export interface NavigatorRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface ZoomAnchor {
  xRatio: number;
  yRatio: number;
  canvasX: number;
  canvasY: number;
}

const MIN_PREVIEW_ZOOM = 0.05;
const MAX_PREVIEW_ZOOM = 4;
const BUTTON_ZOOM_FACTOR = 1.1;
const WHEEL_ZOOM_SENSITIVITY = 0.001;

const percent = (value: number, total: number) => total > 0 ? value / total * 100 : 0;

export function clampPreviewZoom(value: number, fitZoom: number): number {
  const minimum = Math.min(MIN_PREVIEW_ZOOM, fitZoom);
  return Math.max(minimum, Math.min(MAX_PREVIEW_ZOOM, value));
}

export function previewZoomFromWheel(current: number, deltaY: number, fitZoom: number): number {
  const boundedDelta = Math.max(-100, Math.min(100, deltaY));
  return clampPreviewZoom(current * Math.exp(-boundedDelta * WHEEL_ZOOM_SENSITIVITY), fitZoom);
}

export function previewZoomFromButton(current: number, direction: -1 | 1, fitZoom: number): number {
  const factor = direction > 0 ? BUTTON_ZOOM_FACTOR : 1 / BUTTON_ZOOM_FACTOR;
  return clampPreviewZoom(current * factor, fitZoom);
}

export function zoomAnchorScrollTarget(
  anchor: ZoomAnchor,
  imageOffsetLeft: number,
  imageOffsetTop: number,
  imageWidth: number,
  imageHeight: number,
): Pick<PreviewViewport, "scrollLeft" | "scrollTop"> {
  return {
    scrollLeft: imageOffsetLeft + imageWidth * anchor.xRatio - anchor.canvasX,
    scrollTop: imageOffsetTop + imageHeight * anchor.yRatio - anchor.canvasY,
  };
}

export function navigatorRect(viewport: PreviewViewport): NavigatorRect {
  return {
    left: percent(viewport.scrollLeft, viewport.scrollWidth),
    top: percent(viewport.scrollTop, viewport.scrollHeight),
    width: Math.min(100, percent(viewport.clientWidth, viewport.scrollWidth)),
    height: Math.min(100, percent(viewport.clientHeight, viewport.scrollHeight)),
  };
}

export function navigatorScrollTarget(
  viewport: PreviewViewport,
  xRatio: number,
  yRatio: number,
): Pick<PreviewViewport, "scrollLeft" | "scrollTop"> {
  const scrollLeft = xRatio * viewport.scrollWidth - viewport.clientWidth / 2;
  const scrollTop = yRatio * viewport.scrollHeight - viewport.clientHeight / 2;
  return {
    scrollLeft: Math.max(0, Math.min(viewport.scrollWidth - viewport.clientWidth, scrollLeft)),
    scrollTop: Math.max(0, Math.min(viewport.scrollHeight - viewport.clientHeight, scrollTop)),
  };
}
