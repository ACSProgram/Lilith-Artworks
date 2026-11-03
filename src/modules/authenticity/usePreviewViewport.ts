import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { OutputPreviewTileSource, PublicationPreview } from "./types";
import {
  clampPreviewZoom, navigatorRect, navigatorScrollTarget, previewZoomFromButton,
  previewZoomFromWheel, type PreviewViewport, zoomAnchorScrollTarget,
} from "./previewViewport";

/**
 * 质量预览的画布状态：缩放、滚动视口、拖动与导航框。局部图请求不在这里，
 * 见 `usePreviewTiles`。
 */
export function usePreviewViewport(preview: PublicationPreview) {
  // 没有独立的“适应”模式：缩放始终是数值，并以最终成品像素为基准——100% 表示一个
  // 源像素落在屏幕上的一个 CSS 像素。null 表示尚未手动缩放，此时跟随由画布尺寸算
  // 出的适应倍率；打开预览即为数值缩放，画布拖拽从打开起即可用。
  const [zoom, setZoom] = useState<number | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const [viewport, setViewport] = useState<PreviewViewport | null>(null);
  const [canvasSize, setCanvasSize] = useState<{ width: number; height: number } | null>(null);
  const [imageSwitching, setImageSwitching] = useState(false);
  const canvasRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null);
  const navigatorDragRef = useRef<number | null>(null);
  const zoomAnchorRef = useRef<{ xRatio: number; yRatio: number; canvasX: number; canvasY: number } | null>(null);
  const decodedImagesRef = useRef(new Map<string, Promise<void>>());
  const image = showOriginal ? preview.originalImage : preview.image;
  const tileSource: OutputPreviewTileSource = showOriginal ? "original" : "compressed";
  const fitZoom = canvasSize && canvasSize.width > 0 && canvasSize.height > 0
    && preview.sourceWidth > 0 && preview.sourceHeight > 0
    ? Math.min(4, Math.min(canvasSize.width / preview.sourceWidth, canvasSize.height / preview.sourceHeight))
    : 1;
  const effectiveZoom = zoom ?? fitZoom;
  // 内容与叠加层都以源像素尺寸为基准：显示尺寸 = 源像素 × 缩放倍率，因此缩放标签
  // 上的 100% 就是源图 1:1，缩略图只作为放大前的底图。
  const displayWidth = preview.sourceWidth * effectiveZoom;
  const displayHeight = preview.sourceHeight * effectiveZoom;
  const stateRef = useRef({ imageWidth: preview.sourceWidth, imageHeight: preview.sourceHeight, fit: fitZoom, effective: effectiveZoom });
  stateRef.current = { imageWidth: preview.sourceWidth, imageHeight: preview.sourceHeight, fit: fitZoom, effective: effectiveZoom };
  const decodeImage = useCallback((dataUrl: string) => {
    const cached = decodedImagesRef.current.get(dataUrl);
    if (cached) return cached;
    const promise = new Promise<void>((resolve) => {
      const preload = new Image();
      preload.decoding = "async";
      preload.onload = () => { void preload.decode().catch(() => undefined).finally(resolve); };
      preload.onerror = () => resolve();
      preload.src = dataUrl;
    });
    decodedImagesRef.current.set(dataUrl, promise);
    return promise;
  }, []);
  const toggleOriginal = async () => {
    const next = !showOriginal;
    setImageSwitching(true);
    await decodeImage((next ? preview.originalImage : preview.image).dataUrl);
    setShowOriginal(next);
    setImageSwitching(false);
  };
  const measuredZoom = () => {
    const renderedImage = imageRef.current;
    const state = stateRef.current;
    if (renderedImage && renderedImage.clientWidth > 0 && state.imageWidth > 0) {
      return renderedImage.clientWidth / state.imageWidth;
    }
    return state.effective;
  };
  const changeZoom = useCallback((next: number, clientX?: number, clientY?: number) => {
    const canvas = canvasRef.current;
    const renderedImage = imageRef.current;
    if (canvas && renderedImage) {
      const canvasBounds = canvas.getBoundingClientRect();
      const imageBounds = renderedImage.getBoundingClientRect();
      const anchorX = clientX ?? canvasBounds.left + canvasBounds.width / 2;
      const anchorY = clientY ?? canvasBounds.top + canvasBounds.height / 2;
      const insideImage = anchorX >= imageBounds.left && anchorX <= imageBounds.right
        && anchorY >= imageBounds.top && anchorY <= imageBounds.bottom;
      zoomAnchorRef.current = {
        xRatio: insideImage ? (anchorX - imageBounds.left) / imageBounds.width : 0.5,
        yRatio: insideImage ? (anchorY - imageBounds.top) / imageBounds.height : 0.5,
        canvasX: anchorX - canvasBounds.left,
        canvasY: anchorY - canvasBounds.top,
      };
    }
    setZoom(clampPreviewZoom(next, stateRef.current.fit));
  }, []);
  // React 的 onWheel 以 passive 方式注册，preventDefault 会被忽略；改用原生非被动
  // 监听，避免滚轮在缩放预览的同时滚动外层容器。
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const handler = (event: WheelEvent) => {
      event.preventDefault();
      const renderedImage = imageRef.current;
      const state = stateRef.current;
      const current = renderedImage && renderedImage.clientWidth > 0 && state.imageWidth > 0
        ? renderedImage.clientWidth / state.imageWidth
        : state.effective;
      changeZoom(previewZoomFromWheel(current, event.deltaY, state.fit), event.clientX, event.clientY);
    };
    canvas.addEventListener("wheel", handler, { passive: false });
    return () => canvas.removeEventListener("wheel", handler);
  }, [changeZoom]);
  const syncViewport = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    setViewport({
      scrollLeft: canvas.scrollLeft,
      scrollTop: canvas.scrollTop,
      scrollWidth: canvas.scrollWidth,
      scrollHeight: canvas.scrollHeight,
      clientWidth: canvas.clientWidth,
      clientHeight: canvas.clientHeight,
    });
  }, []);
  // 画布尺寸变化时同步适应倍率与视口；zoom 为 null 时预览自动跟随适应倍率。
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const update = () => {
      setCanvasSize({ width: canvas.clientWidth, height: canvas.clientHeight });
      syncViewport();
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(canvas);
    const frame = requestAnimationFrame(update);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [image?.dataUrl, syncViewport]);
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const anchor = zoomAnchorRef.current;
    if (!anchor) return;
    // 图片在滚动内容中由网格居中；偏移只依赖已知几何量，不依赖 offsetParent。
    const state = stateRef.current;
    const imageWidth = state.imageWidth * state.effective;
    const imageHeight = state.imageHeight * state.effective;
    const offsetLeft = Math.max(0, (Math.max(canvas.clientWidth, imageWidth) - imageWidth) / 2);
    const offsetTop = Math.max(0, (Math.max(canvas.clientHeight, imageHeight) - imageHeight) / 2);
    const target = zoomAnchorScrollTarget(anchor, offsetLeft, offsetTop, imageWidth, imageHeight);
    canvas.scrollLeft = target.scrollLeft;
    canvas.scrollTop = target.scrollTop;
    zoomAnchorRef.current = null;
    syncViewport();
  }, [image.dataUrl, effectiveZoom, syncViewport]);

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !canvasRef.current) return;
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      scrollLeft: canvasRef.current.scrollLeft,
      scrollTop: canvasRef.current.scrollTop,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId || !canvasRef.current) return;
    canvasRef.current.scrollLeft = drag.scrollLeft - (event.clientX - drag.x);
    canvasRef.current.scrollTop = drag.scrollTop - (event.clientY - drag.y);
    syncViewport();
  };
  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId === event.pointerId) dragRef.current = null;
  };
  const navigable = viewport != null
    && (viewport.scrollWidth > viewport.clientWidth || viewport.scrollHeight > viewport.clientHeight);
  const moveFromNavigator = (event: ReactPointerEvent<HTMLDivElement>) => {
    const canvas = canvasRef.current;
    if (!canvas || !viewport) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const target = navigatorScrollTarget(
      viewport,
      (event.clientX - bounds.left) / bounds.width,
      (event.clientY - bounds.top) / bounds.height,
    );
    canvas.scrollLeft = target.scrollLeft;
    canvas.scrollTop = target.scrollTop;
    syncViewport();
  };
  const onNavigatorPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    navigatorDragRef.current = event.pointerId;
    event.currentTarget.setPointerCapture(event.pointerId);
    moveFromNavigator(event);
  };
  const onNavigatorPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (navigatorDragRef.current === event.pointerId) moveFromNavigator(event);
  };
  const onNavigatorPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (navigatorDragRef.current === event.pointerId) navigatorDragRef.current = null;
  };
  const navigationRect = viewport ? navigatorRect(viewport) : null;

  return {
    canvasRef,
    imageRef,
    image,
    tileSource,
    showOriginal,
    imageSwitching,
    toggleOriginal,
    fitZoom,
    effectiveZoom,
    displayWidth,
    displayHeight,
    viewport,
    navigable,
    navigationRect,
    changeZoom,
    measuredZoom,
    resetZoom: () => setZoom(null),
    zoomIn: () => changeZoom(previewZoomFromButton(measuredZoom(), 1, stateRef.current.fit)),
    zoomOut: () => changeZoom(previewZoomFromButton(measuredZoom(), -1, stateRef.current.fit)),
    zoomToActualSize: () => changeZoom(1),
    syncViewport,
    canvasPointerHandlers: {
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onPointerCancel: onPointerUp,
    },
    navigatorPointerHandlers: {
      onPointerDown: onNavigatorPointerDown,
      onPointerMove: onNavigatorPointerMove,
      onPointerUp: onNavigatorPointerUp,
      onPointerCancel: onNavigatorPointerUp,
    },
  };
}
