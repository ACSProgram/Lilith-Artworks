import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import type { PreviewImage } from "./types";
import type { TileRequest } from "./previewTile";

/**
 * 框选放大镜（loupe）：在指针旁显示源分辨率局部，用于精确落点。
 *
 * 无缝是这里的第一约束。镜片不持有「当前那一块」的概念，而是
 * 「尽可能好的本地像素 + 一次纯视图变换」：
 *
 * - **底图**是调用方已经加载好的预览缩略图（≤1600 px，几何与源图一致），
 *   因此镜片永远能立即出画，任何时刻都不出现加载态；
 * - **源分辨率瓦片**按固定的源像素网格在后台补齐并缓存，指针停在同一格内时
 *   移动只改绘制矩形、不产生任何请求；换格才补一块，停留更久再预取相邻格。
 *
 * 这与大图取景器（Deep Zoom / 地图）的做法一致：先有立刻可看的东西，再原地提升
 * 清晰度，而不是每移动一次就等一次 IO。
 */

/** 取样窗口的源像素跨度（正方形）。 */
export const LOUPE_SPAN = 72;
/** 固定放大倍率：一个源像素在镜内占用的 CSS 像素数。 */
export const LOUPE_ZOOM = 3;
/** 镜片边长（CSS 像素）。 */
export const LOUPE_SIZE = LOUPE_SPAN * LOUPE_ZOOM;
/** 镜片与指针之间的间隙（CSS 像素）。 */
export const LOUPE_GAP = 16;
/** 源分辨率瓦片的边长（源像素）；源图更小时退化为整幅。 */
export const LOUPE_REGION_EDGE = 512;
/** 客户端瓦片缓存块数上限。 */
export const LOUPE_REGION_CACHE = 12;
/** 指针停下后取当前格的延迟；扫过时不会为中途经过的格子发请求。 */
export const LOUPE_FETCH_DEBOUNCE_MS = 120;
/** 当前格到位后预取相邻格的延迟，避免移动过程中堆积无用请求。 */
export const LOUPE_PREFETCH_DELAY_MS = 400;
/** 后端裁剪的输出边长下限，与 `pipeline::TILE_MIN_EDGE` 一致。 */
const LOUPE_MIN_EDGE = 64;

export interface LoupeTarget {
  /** 镜片显示的源矩形左上角；贴边时窗口会被裁剪到源图内。 */
  x: number;
  y: number;
  /** 窗口跨度；源图小于 {@link LOUPE_SPAN} 时退化为源图边长。 */
  width: number;
  height: number;
}

/**
 * 以指针所在源像素为中心计算镜片显示窗口。
 *
 * 返回的 `x/y` 是窗口左上角，作为瓦片分格的定位基准：镜头按指针居中绘制，因此
 * 这里只需把窗口夹到源图范围内，保证取到的源分辨率瓦片始终完整覆盖窗口。
 */
export function loupeTarget(
  sourceX: number,
  sourceY: number,
  sourceWidth: number,
  sourceHeight: number,
): LoupeTarget | null {
  if (!Number.isFinite(sourceX) || !Number.isFinite(sourceY)) return null;
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  return {
    x: Math.min(Math.max(sourceX - LOUPE_SPAN / 2, 0), Math.max(0, sourceWidth - LOUPE_SPAN)),
    y: Math.min(Math.max(sourceY - LOUPE_SPAN / 2, 0), Math.max(0, sourceHeight - LOUPE_SPAN)),
    width: Math.min(LOUPE_SPAN, sourceWidth),
    height: Math.min(LOUPE_SPAN, sourceHeight),
  };
}

export interface LoupeRegion {
  x: number;
  y: number;
  width: number;
  height: number;
  /** 后端裁剪输出边长：等于该格的源尺寸，因此输出为 1:1、不会被放大。 */
  maxEdge: number;
}

/** 瓦片网格的步长：必须小于瓦片边长，重叠量正好覆盖一个镜片窗口。 */
function regionSteps(sourceWidth: number, sourceHeight: number): [number, number] {
  const edgeX = Math.min(LOUPE_REGION_EDGE, sourceWidth);
  const edgeY = Math.min(LOUPE_REGION_EDGE, sourceHeight);
  return [
    Math.max(1, edgeX - Math.min(LOUPE_SPAN, sourceWidth)),
    Math.max(1, edgeY - Math.min(LOUPE_SPAN, sourceHeight)),
  ];
}

/**
 * 覆盖当前镜片窗口的源分辨率瓦片。
 *
 * 网格以「镜片窗口的左上角」而不是指针本身定位，且步长比瓦片边长小一个窗口，
 * 因此窗口永远被完整覆盖：指针在同一格内移动不会换格，只有走过一个步长才需要新的一块。
 */
export function loupeRegion(
  target: LoupeTarget,
  sourceWidth: number,
  sourceHeight: number,
): LoupeRegion | null {
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  const edgeX = Math.min(LOUPE_REGION_EDGE, sourceWidth);
  const edgeY = Math.min(LOUPE_REGION_EDGE, sourceHeight);
  const [stepX, stepY] = regionSteps(sourceWidth, sourceHeight);
  return {
    x: Math.min(Math.floor(target.x / stepX) * stepX, Math.max(0, sourceWidth - edgeX)),
    y: Math.min(Math.floor(target.y / stepY) * stepY, Math.max(0, sourceHeight - edgeY)),
    width: edgeX,
    height: edgeY,
    maxEdge: Math.max(LOUPE_MIN_EDGE, edgeX, edgeY),
  };
}

/** 当前格的四个正交邻居（去重、裁到源图范围内），用于停留时的预取。 */
export function loupeNeighbours(
  region: LoupeRegion,
  sourceWidth: number,
  sourceHeight: number,
): LoupeRegion[] {
  const [stepX, stepY] = regionSteps(sourceWidth, sourceHeight);
  const offsets: Array<[number, number]> = [[-1, 0], [1, 0], [0, -1], [0, 1]];
  const seen = new Set<string>();
  const neighbours: LoupeRegion[] = [];
  for (const [offsetX, offsetY] of offsets) {
    const x = Math.min(Math.max(region.x + offsetX * stepX, 0), Math.max(0, sourceWidth - region.width));
    const y = Math.min(Math.max(region.y + offsetY * stepY, 0), Math.max(0, sourceHeight - region.height));
    const identity = `${x},${y}`;
    if ((x === region.x && y === region.y) || seen.has(identity)) continue;
    seen.add(identity);
    neighbours.push({ ...region, x, y });
  }
  return neighbours;
}

export interface LoupePlacement {
  left: number;
  top: number;
}

/** 镜片跟随指针，并在舞台边缘翻转到另一侧，最后收敛到舞台范围内。 */
export function loupePlacement(
  pointerX: number,
  pointerY: number,
  stageWidth: number,
  stageHeight: number,
): LoupePlacement {
  const left = pointerX + LOUPE_GAP + LOUPE_SIZE <= stageWidth
    ? pointerX + LOUPE_GAP
    : pointerX - LOUPE_GAP - LOUPE_SIZE;
  const top = pointerY + LOUPE_GAP + LOUPE_SIZE <= stageHeight
    ? pointerY + LOUPE_GAP
    : pointerY - LOUPE_GAP - LOUPE_SIZE;
  return {
    left: Math.min(Math.max(left, 0), Math.max(0, stageWidth - LOUPE_SIZE)),
    top: Math.min(Math.max(top, 0), Math.max(0, stageHeight - LOUPE_SIZE)),
  };
}

export function regionKey(sourceKey: string, region: LoupeRegion): string {
  return `${sourceKey}|${region.x},${region.y},${region.width},${region.height}`;
}

/** 镜片当前可用的像素来源。 */
export interface LoupeFrame {
  image: CanvasImageSource;
  /** 该图覆盖的源矩形左上角。 */
  originX: number;
  originY: number;
  /** 图像像素 / 源像素；1 表示源分辨率。 */
  scale: number;
  /** 源分辨率瓦片用像素化取样，降采样底图用平滑取样。 */
  sharp: boolean;
}

export interface LoupeFrameOptions {
  pointer: { x: number; y: number } | null;
  sourceWidth: number;
  sourceHeight: number;
  /** 源标识（分支或待识别图片路径）；变化即隔离缓存。 */
  sourceKey: string;
  /** 已加载的整幅降采样底图，用于在瓦片到位前立即出画。 */
  base: PreviewImage;
  request: (tile: TileRequest) => Promise<PreviewImage>;
}

/**
 * 维护放大镜的瓦片缓存，并返回当前应当绘制的像素来源。
 *
 * 优先级：覆盖当前窗口的源分辨率瓦片 → 整幅底图 → 无（只垫一层底色）。任何一档
 * 都不会阻塞：瓦片在后台补齐，镜片在这期间显示底图，因此不存在「加载中」的观感。
 */
export function useLoupeFrame({
  pointer, sourceWidth, sourceHeight, sourceKey, base, request,
}: LoupeFrameOptions): LoupeFrame | null {
  const target = useMemo(
    () => (pointer ? loupeTarget(pointer.x * sourceWidth, pointer.y * sourceHeight, sourceWidth, sourceHeight) : null),
    [pointer, sourceWidth, sourceHeight],
  );
  const region = useMemo(
    () => (target ? loupeRegion(target, sourceWidth, sourceHeight) : null),
    [sourceHeight, sourceWidth, target],
  );
  const key = region ? regionKey(sourceKey, region) : "";
  const baseKey = `${sourceKey}|base`;
  const baseDataUrl = base.dataUrl;

  const images = useRef(new Map<string, CanvasImageSource>());
  const tiles = useRef(new Map<string, PreviewImage>());
  const lru = useRef<string[]>([]);
  const requested = useRef(new Set<string>());
  const sourceRef = useRef(sourceKey);
  const requestRef = useRef(request);
  sourceRef.current = sourceKey;
  requestRef.current = request;

  const [version, bump] = useReducer((current: number) => current + 1, 0);

  const decode = useCallback((imageKey: string, dataUrl: string) => {
    if (images.current.has(imageKey) || requested.current.has(imageKey)) return;
    requested.current.add(imageKey);
    const image = new Image();
    image.decoding = "async";
    image.onload = () => {
      requested.current.delete(imageKey);
      images.current.set(imageKey, image);
      bump();
    };
    image.onerror = () => requested.current.delete(imageKey);
    image.src = dataUrl;
  }, []);

  const fetchRegion = useCallback((tileKey: string, bounds: LoupeRegion) => {
    if (tiles.current.has(tileKey) || requested.current.has(tileKey)) return;
    requested.current.add(tileKey);
    const requestedSource = sourceRef.current;
    requestRef.current({
      x: bounds.x,
      y: bounds.y,
      width: bounds.width,
      height: bounds.height,
      maxEdge: bounds.maxEdge,
    })
      .then((tile) => {
        // 源已切换时丢弃结果，避免旧作品的瓦片回填进新缓存。
        if (requestedSource !== sourceRef.current) return;
        tiles.current.set(tileKey, tile);
        const order = lru.current;
        order.push(tileKey);
        while (order.length > LOUPE_REGION_CACHE) {
          const oldest = order.shift();
          if (oldest === undefined || oldest === tileKey) continue;
          tiles.current.delete(oldest);
          images.current.delete(oldest);
        }
        decode(tileKey, tile.dataUrl);
        bump();
      })
      .catch(() => undefined)
      .finally(() => { requested.current.delete(tileKey); });
  }, [decode]);

  // 底图：源一变就重新解码；镜片在任何时刻都有可画的像素。
  useEffect(() => { decode(baseKey, baseDataUrl); }, [baseDataUrl, baseKey, decode]);

  // 切换源时释放该源之外的瓦片与解码图，避免跨作品残留。
  const previousSource = useRef(sourceKey);
  useEffect(() => {
    if (previousSource.current === sourceKey) return;
    previousSource.current = sourceKey;
    tiles.current.clear();
    lru.current.length = 0;
    for (const existing of Array.from(images.current.keys())) {
      if (existing !== `${sourceKey}|base`) images.current.delete(existing);
    }
  }, [sourceKey]);

  // 指针停下后取当前格。
  //
  // 依赖只取格子键而不是 region 对象：同一格内微调指针时键不变，计时不会被反复重置，
  // 因此停留即可取到源分辨率；只有真的走过一个步长才重新计时。
  const regionRef = useRef<LoupeRegion | null>(null);
  regionRef.current = region;

  useEffect(() => {
    if (!key) return;
    const timer = window.setTimeout(() => {
      const current = regionRef.current;
      if (current && regionKey(sourceKey, current) === key) fetchRegion(key, current);
    }, LOUPE_FETCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [fetchRegion, key, sourceKey]);

  // 停留更久则预取相邻格，让跨格时通常已经命中缓存。
  useEffect(() => {
    if (!key) return;
    const timer = window.setTimeout(() => {
      const current = regionRef.current;
      if (!current || regionKey(sourceKey, current) !== key) return;
      for (const neighbour of loupeNeighbours(current, sourceWidth, sourceHeight)) {
        fetchRegion(regionKey(sourceKey, neighbour), neighbour);
      }
    }, LOUPE_PREFETCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [fetchRegion, key, sourceHeight, sourceKey, sourceWidth]);

  return useMemo<LoupeFrame | null>(() => {
    if (!target || !region) return null;
    const cachedTile = tiles.current.get(key);
    const tileImage = cachedTile ? images.current.get(key) : undefined;
    if (cachedTile && tileImage) {
      return { image: tileImage, originX: region.x, originY: region.y, scale: 1, sharp: true };
    }
    const baseImage = images.current.get(baseKey);
    const naturalWidth = baseImage instanceof HTMLImageElement ? baseImage.naturalWidth : 0;
    if (baseImage && naturalWidth > 0 && sourceWidth > 0) {
      return { image: baseImage, originX: 0, originY: 0, scale: naturalWidth / sourceWidth, sharp: false };
    }
    return null;
    // version 在解码完成或瓦片到达时自增，用于重新挑选可用像素。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baseKey, key, region, sourceWidth, target, version]);
}
