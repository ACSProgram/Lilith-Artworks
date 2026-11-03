import { useEffect, useRef, useState } from "react";
import type { OutputPreviewTileSource, PreviewImage, PublicationPreview } from "./types";
import type { PreviewViewport } from "./previewViewport";
import { tileCacheKey, tileRequestForView, type TileRequest } from "./previewTile";

/** 局部图去抖：平移途中不逐像素请求，停稳后再取当前格。 */
const TILE_FETCH_DEBOUNCE_MS = 180;
/** 对话框的局部图缓存块数上限。 */
const TILE_CACHE_LIMIT = 12;

interface PreviewTileOptions {
  preview: PublicationPreview;
  image: PreviewImage;
  tileSource: OutputPreviewTileSource;
  displayWidth: number;
  displayHeight: number;
  viewport: PreviewViewport | null;
  busy: boolean;
  onRequestTile: (source: OutputPreviewTileSource, request: TileRequest) => Promise<PreviewImage>;
}

/**
 * 放大后叠加的源分辨率局部图：按可视区域请求当前格，带 180ms 去抖与 12 块缓存。
 */
export function usePreviewTiles({
  preview,
  image,
  tileSource,
  displayWidth,
  displayHeight,
  viewport,
  busy,
  onRequestTile,
}: PreviewTileOptions) {
  const [tile, setTile] = useState<{
    key: string;
    request: TileRequest;
    image: PreviewImage;
    source: OutputPreviewTileSource;
  } | null>(null);
  const tileCacheRef = useRef(new Map<string, PreviewImage>());
  const tileRequestRef = useRef(0);

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

  return { tile, desiredTileKey };
}
