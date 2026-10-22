import {
  Image as ImageIcon, ImageDown, LoaderCircle, Maximize2, X, ZoomIn, ZoomOut,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { formatBytes } from "../../shared/format";
import type {
  OutputPreviewTileSource, PreviewImage, PublicationPreview,
} from "./types";
import {
  clampPreviewZoom, navigatorRect, navigatorScrollTarget, previewZoomFromButton,
  previewZoomFromWheel, type PreviewViewport, zoomAnchorScrollTarget,
} from "./previewViewport";
import { tileCacheKey, tileOverlayStyle, tileRequestForView, type TileRequest } from "./previewTile";

/** 局部图去抖：平移途中不逐像素请求，停稳后再取当前格。 */
const TILE_FETCH_DEBOUNCE_MS = 180;
/** 对话框的局部图缓存块数上限。 */
const TILE_CACHE_LIMIT = 12;

interface PublicationPreviewDialogProps {
  preview: PublicationPreview;
  busy: boolean;
  cancelling: boolean;
  onRequestTile: (source: OutputPreviewTileSource, request: TileRequest) => Promise<PreviewImage>;
  onBack: () => void;
  onCancel: () => void;
  onPublish: () => void;
}

export function PublicationPreviewDialog({ preview, busy, cancelling, onRequestTile, onBack, onCancel, onPublish }: PublicationPreviewDialogProps) {
  // 没有独立的“适应”模式：缩放始终是数值，并以最终成品像素为基准——100% 表示一个
  // 源像素落在屏幕上的一个 CSS 像素。null 表示尚未手动缩放，此时跟随由画布尺寸算
  // 出的适应倍率；打开预览即为数值缩放，画布拖拽从打开起即可用。
  const [zoom, setZoom] = useState<number | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const [viewport, setViewport] = useState<PreviewViewport | null>(null);
  const [canvasSize, setCanvasSize] = useState<{ width: number; height: number } | null>(null);
  const [tile, setTile] = useState<{ key: string; request: TileRequest; image: PreviewImage; source: OutputPreviewTileSource } | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null);
  const navigatorDragRef = useRef<number | null>(null);
  const zoomAnchorRef = useRef<{ xRatio: number; yRatio: number; canvasX: number; canvasY: number } | null>(null);
  const decodedImagesRef = useRef(new Map<string, Promise<void>>());
  const tileCacheRef = useRef(new Map<string, PreviewImage>());
  const tileRequestRef = useRef(0);
  const [imageSwitching, setImageSwitching] = useState(false);
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
  // 高清局部：只有缩略图被放大（显示尺寸超过缩略图本身）时才需要，此时按可视区域
  // 请求源分辨率裁剪块并叠加显示，让导出图与原始成品在放大后仍从源像素渲染，而不是
  // 插值放大 2400 px 缩略图。缩略图未被放大时直接显示缩略图即为 1:1 或更好。
  const desiredTile = tileRequestForView({
    displayWidth,
    displayHeight,
    thumbWidth: image.width,
    thumbHeight: image.height,
    sourceWidth: preview.sourceWidth,
    sourceHeight: preview.sourceHeight,
    viewport,
  });
  const desiredTileKey = desiredTile ? `${tileSource}|${tileCacheKey(desiredTile)}` : "";
  const desiredTileRef = useRef<{ request: TileRequest; source: OutputPreviewTileSource } | null>(null);
  desiredTileRef.current = desiredTile ? { request: desiredTile, source: tileSource } : null;
  useEffect(() => {
    // 只有与当前对位一致（key 相同）的局部图才会被渲染，因此签名进行中可以直接保留
    // 已加载的局部图，不必为了不请求而把它摘掉。
    if (!desiredTileKey || busy) return;
    const target = desiredTileRef.current;
    if (!target) return;
    // 每次目标矩形或来源变化都推进代次：平移途中的在途响应按代次丢弃，避免旧
    // 矩形错位叠加。
    const requestId = ++tileRequestRef.current;
    const cached = tileCacheRef.current.get(desiredTileKey);
    if (cached) {
      setTile({ key: desiredTileKey, request: target.request, image: cached, source: target.source });
      return;
    }
    const timer = window.setTimeout(() => {
      onRequestTile(target.source, target.request).then((next) => {
        if (requestId !== tileRequestRef.current) return;
        const cache = tileCacheRef.current;
        cache.set(desiredTileKey, next);
        // 满了按插入顺序淘汰最早的块；命中不重排，与框选放大镜的缓存一致。
        while (cache.size > TILE_CACHE_LIMIT) {
          const oldest = cache.keys().next();
          if (oldest.done) break;
          cache.delete(oldest.value);
        }
        setTile({ key: desiredTileKey, request: target.request, image: next, source: target.source });
      }).catch(() => undefined);
    }, TILE_FETCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [desiredTileKey, busy, onRequestTile]);
  useEffect(() => () => { tileRequestRef.current += 1; }, []);
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
  return <div className="dialog-backdrop publication-preview-backdrop" onMouseDown={() => { if (!busy) onBack(); }}>
    <section className="publication-preview-dialog" role="dialog" aria-modal="true" aria-labelledby="publication-preview-title" onMouseDown={(event) => event.stopPropagation()}>
      <header>
        <div><small>发布前检查</small><h2 id="publication-preview-title">导出预览</h2><span>{preview.sourceWidth} x {preview.sourceHeight} · {formatBytes(preview.outputBytes)} · {preview.cacheHit ? "复用缓存" : `渲染 ${preview.renderMs} ms / 编码 ${preview.encodeMs} ms`}</span></div>
        <div className="preview-zoom-controls">
          <button className="icon-button" type="button" title="缩小" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), -1, stateRef.current.fit))}><ZoomOut size={16} /></button>
          <button className="zoom-value" type="button" title="按原始像素显示（100% 为 1:1）" onClick={() => changeZoom(1)}>{Math.round(effectiveZoom * 100)}%</button>
          <button className="icon-button" type="button" title="放大" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), 1, stateRef.current.fit))}><ZoomIn size={16} /></button>
          <button className="icon-button" type="button" title="适应窗口" onClick={() => setZoom(null)}><Maximize2 size={16} /></button>
          <button className={`icon-button${showOriginal ? " active" : ""}`} type="button" title={showOriginal ? "显示压缩预览" : "显示原图"} disabled={imageSwitching} onClick={() => void toggleOriginal()}><ImageIcon size={16} /></button>
          <button className="icon-button" type="button" title="关闭预览" disabled={busy} onClick={onBack}><X size={17} /></button>
        </div>
      </header>
      <div className="publication-preview-stage">
        <div
          ref={canvasRef}
          className="publication-preview-canvas"
          onScroll={syncViewport}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDragStart={(event) => event.preventDefault()}
        >
          <div
            className="publication-preview-content"
            style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }}
          >
            <div
              className="publication-preview-image"
              style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }}
            >
              <img ref={imageRef} src={image.dataUrl} alt={showOriginal ? "原始成品预览" : "导出预览"} draggable={false} onLoad={syncViewport} style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }} />
              {tile && tile.key === desiredTileKey && <img
                className="publication-preview-tile"
                src={tile.image.dataUrl}
                alt=""
                draggable={false}
                style={tileOverlayStyle(tile.request, preview.sourceWidth, preview.sourceHeight)}
              />}
            </div>
          </div>
        </div>
        {navigable && navigationRect && <div
          className="publication-preview-navigator"
          title="拖动以定位预览区域"
          style={{ aspectRatio: `${image.width} / ${image.height}` }}
          onPointerDown={onNavigatorPointerDown}
          onPointerMove={onNavigatorPointerMove}
          onPointerUp={onNavigatorPointerUp}
          onPointerCancel={onNavigatorPointerUp}
        >
          <img src={image.dataUrl} alt="" draggable={false} />
          <span style={{ left: `${navigationRect.left}%`, top: `${navigationRect.top}%`, width: `${navigationRect.width}%`, height: `${navigationRect.height}%` }} />
        </div>}
      </div>
      <footer><span>{busy ? (cancelling ? "正在安全取消；已开始的原子发布收尾不会中断。" : "正在写入 C2PA；时间戳服务最长等待 30 秒。") : showOriginal ? "当前显示原始成品，用于快速对比；放大后叠加源分辨率局部。" : "缩略图使用正式发布的背景合成、TrustMark 与 JPEG 编码参数；放大后叠加源分辨率局部。"}</span><div><button className="secondary-button" type="button" disabled={busy} onClick={onBack}>返回调整</button><button className={busy ? "secondary-button" : "primary-button"} type="button" disabled={cancelling} onClick={busy ? onCancel : onPublish}>{busy ? (cancelling ? <LoaderCircle className="spin" size={16} /> : <X size={16} />) : <ImageDown size={16} />}{busy ? (cancelling ? "正在取消" : "取消签名") : "签名并发布"}</button></div></footer>
    </section>
  </div>;
}
