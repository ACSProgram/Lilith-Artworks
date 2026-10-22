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

/**
 * 镜片相对主视图的固定放大倍率。
 *
 * 主视图（`RegionEditor` 的图片舞台）用 `object-fit: contain` 把整幅图缩放到固定
 * 尺寸，因此「镜片比主视图大 LOUPE_ZOOM 倍」等价于「镜片始终覆盖源图的固定比例」：
 * 源图越大、主视图越小，取样跨度越大。若改用固定的源像素跨度，大画布会被放大到无法
 * 与主视图对照。
 */
export const LOUPE_ZOOM = 3;
/** 镜片边长（CSS 像素）。 */
export const LOUPE_SIZE = 216;
/** 取样跨度的下限（源像素），避免极小源图被放大到无意义的程度。 */
export const LOUPE_SPAN_MIN = 8;
/** 取样跨度的上限（源像素），避免超大源图把瓦片裁剪范围推到后端难以承受的量级。 */
export const LOUPE_SPAN_MAX = 4096;
/** 镜片与指针之间的间隙（CSS 像素）。 */
export const LOUPE_GAP = 16;
/** 源分辨率瓦片的边长下限（源像素）；源图更小时退化为整幅。 */
export const LOUPE_REGION_EDGE = 512;
/** 覆盖取样窗口所需的瓦片边距（相对取样跨度）；同时保证网格步长非零。 */
const LOUPE_REGION_MARGIN = 1.25;
/** 瓦片输出边长的兜底上限（源像素），与后端 `TILE_MAX_EDGE` 量级一致。 */
const LOUPE_TILE_MAX_EDGE = 1024;
/** 客户端瓦片缓存块数上限。 */
export const LOUPE_REGION_CACHE = 12;
/** 指针停下后取当前格的延迟；扫过时不会为中途经过的格子发请求。 */
export const LOUPE_FETCH_DEBOUNCE_MS = 120;
/** 当前格到位后预取相邻格的延迟，避免移动过程中堆积无用请求。 */
export const LOUPE_PREFETCH_DELAY_MS = 400;
/** 后端裁剪的输出边长下限，与 `pipeline::TILE_MIN_EDGE` 一致。 */
const LOUPE_MIN_EDGE = 64;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * 取样窗口的源像素跨度。
 *
 * `displayScale` 是主视图的显示比例（CSS 像素 / 源像素）。镜片以固定的 {@link LOUPE_ZOOM}
 * 倍放大主视图，因此跨度 = 镜片边长 / (倍率 × 显示比例)：源图越大、主视图越小，跨度越大，
 * 从而始终覆盖源图的固定比例，而不是固定的一小片源像素。显示比例缺失或非法（画布尚未测量）
 * 时退化为 1，等价于 1:1 主视图。
 */
export function loupeSpan(displayScale: number): number {
  const scale = Number.isFinite(displayScale) && displayScale > 0 ? displayScale : 1;
  return clamp(LOUPE_SIZE / (LOUPE_ZOOM * scale), LOUPE_SPAN_MIN, LOUPE_SPAN_MAX);
}

export interface LoupeTarget {
  /** 镜片显示的源矩形左上角；贴边时窗口会被裁剪到源图内。 */
  x: number;
  y: number;
  /** 窗口跨度；源图小于取样跨度时退化为源图边长。 */
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
  span: number,
): LoupeTarget | null {
  if (!Number.isFinite(sourceX) || !Number.isFinite(sourceY)) return null;
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  const spanX = Math.min(span, sourceWidth);
  const spanY = Math.min(span, sourceHeight);
  return {
    x: Math.min(Math.max(sourceX - spanX / 2, 0), Math.max(0, sourceWidth - spanX)),
    y: Math.min(Math.max(sourceY - spanY / 2, 0), Math.max(0, sourceHeight - spanY)),
    width: spanX,
    height: spanY,
  };
}

export interface LoupeRegion {
  x: number;
  y: number;
  width: number;
  height: number;
  /** 后端裁剪输出边长：按该格在镜片内的显示尺寸给出，不超过源分辨率与后端上限。 */
  maxEdge: number;
}

/**
 * 覆盖取样窗口的瓦片边长：至少 {@link LOUPE_REGION_EDGE}，窗口更大时按边距放大，
 * 保证瓦片始终完整覆盖窗口；源图更小时退化为源图边长。
 */
function regionEdge(sourceSize: number, span: number): number {
  return Math.min(sourceSize, Math.max(LOUPE_REGION_EDGE, Math.ceil(span * LOUPE_REGION_MARGIN)));
}

/** 瓦片网格的步长：必须小于瓦片边长，重叠量正好覆盖一个镜片窗口。 */
function regionSteps(sourceWidth: number, sourceHeight: number, span: number): [number, number] {
  const edgeX = regionEdge(sourceWidth, span);
  const edgeY = regionEdge(sourceHeight, span);
  return [
    Math.max(1, edgeX - Math.min(span, sourceWidth)),
    Math.max(1, edgeY - Math.min(span, sourceHeight)),
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
  span: number,
  pixelRatio = 1,
): LoupeRegion | null {
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  const edgeX = regionEdge(sourceWidth, span);
  const edgeY = regionEdge(sourceHeight, span);
  const [stepX, stepY] = regionSteps(sourceWidth, sourceHeight, span);
  // 瓦片只需在镜片内看起来正确：输出边长取该格在屏幕上的显示尺寸（含设备像素比），
  // 上限为源分辨率（放大时再大也不会更清晰）与后端上限。大画布下镜片是缩小取样，
  // 因此这里会把输出压到显示分辨率，而不是传输用不到的源像素。
  const edge = Math.max(edgeX, edgeY);
  const maxEdge = clamp(
    Math.ceil(edge * (LOUPE_SIZE / span) * pixelRatio),
    LOUPE_MIN_EDGE,
    Math.max(LOUPE_MIN_EDGE, Math.min(edge, LOUPE_TILE_MAX_EDGE)),
  );
  return {
    x: Math.min(Math.floor(target.x / stepX) * stepX, Math.max(0, sourceWidth - edgeX)),
    y: Math.min(Math.floor(target.y / stepY) * stepY, Math.max(0, sourceHeight - edgeY)),
    width: edgeX,
    height: edgeY,
    maxEdge,
  };
}

/** 当前格的四个正交邻居（去重、裁到源图范围内），用于停留时的预取。 */
export function loupeNeighbours(
  region: LoupeRegion,
  sourceWidth: number,
  sourceHeight: number,
  span: number,
): LoupeRegion[] {
  const [stepX, stepY] = regionSteps(sourceWidth, sourceHeight, span);
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

/** 缓存键：输出边长也参与标识，因为同一格在不同设备像素比下的瓦片内容不同。 */
export function regionKey(sourceKey: string, region: LoupeRegion): string {
  return `${sourceKey}|${region.x},${region.y},${region.width},${region.height},${region.maxEdge}`;
}

/** 镜片当前可用的像素来源。 */
export interface LoupeFrame {
  image: CanvasImageSource;
  /** 该图覆盖的源矩形左上角。 */
  originX: number;
  originY: number;
  /** 图像像素 / 源像素；1 表示源分辨率。 */
  scale: number;
  /** 源分辨率瓦片用像素化取样，降采样底图或缩小的瓦片用平滑取样。 */
  sharp: boolean;
}

export interface LoupeFrameOptions {
  pointer: { x: number; y: number } | null;
  sourceWidth: number;
  sourceHeight: number;
  /** 取样窗口的源像素跨度，由镜片相对主视图的倍率决定（见 {@link loupeSpan}）。 */
  span: number;
  /** 设备像素比（已按画布缓冲上限收敛）；决定瓦片的输出分辨率，缺省为 1。 */
  pixelRatio?: number;
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
  pointer, sourceWidth, sourceHeight, span, pixelRatio = 1, sourceKey, base, request,
}: LoupeFrameOptions): LoupeFrame | null {
  const target = useMemo(
    () => (pointer ? loupeTarget(pointer.x * sourceWidth, pointer.y * sourceHeight, sourceWidth, sourceHeight, span) : null),
    [pointer, sourceWidth, sourceHeight, span],
  );
  const region = useMemo(
    () => (target ? loupeRegion(target, sourceWidth, sourceHeight, span, pixelRatio) : null),
    [pixelRatio, sourceHeight, sourceWidth, span, target],
  );
  const key = region ? regionKey(sourceKey, region) : "";
  const baseKey = `${sourceKey}|base`;
  const baseDataUrl = base.dataUrl;

  const images = useRef(new Map<string, CanvasImageSource>());
  const tiles = useRef(new Map<string, PreviewImage>());
  // 瓦片键的插入顺序：满了按最早插入的淘汰，命中不重排（不是 LRU）。
  const insertionOrder = useRef<string[]>([]);
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
        const order = insertionOrder.current;
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
    insertionOrder.current.length = 0;
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
      for (const neighbour of loupeNeighbours(current, sourceWidth, sourceHeight, span)) {
        fetchRegion(regionKey(sourceKey, neighbour), neighbour);
      }
    }, LOUPE_PREFETCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [fetchRegion, key, sourceHeight, sourceWidth, span, sourceKey]);

  return useMemo<LoupeFrame | null>(() => {
    if (!target || !region) return null;
    const cachedTile = tiles.current.get(key);
    const tileImage = cachedTile ? images.current.get(key) : undefined;
    if (cachedTile && tileImage) {
      // 瓦片输出边长可能被上限收缩（窗口很大时），此时按实际像素比缩放并改用平滑取样。
      const tileScale = cachedTile.width > 0 && region.width > 0 ? cachedTile.width / region.width : 1;
      return { image: tileImage, originX: region.x, originY: region.y, scale: tileScale, sharp: tileScale >= 1 };
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
