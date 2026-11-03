/**
 * 笔迹图层：整板几何、世界折线缓存与屏幕用的 GPU 缓冲。
 *
 * 只服务屏幕绘制与命中判定：几何由 `read_pin_board_strokes` 读取，缓冲在几何、
 * 缩放或可见集合变化时重建。指针状态机、绘制草稿与整画布导出缓冲不在这里。
 */

import { diagnosticsLog } from "../../shared/diagnostics";
import { errorMessage } from "../../shared/errors";
import { pinBoardApi } from "./api";
import type { PinBoardItem, PinBoardStrokeItem } from "./types";
import { parseStrokeBatch, type PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  buildStrokeWorldOutlines,
  strokeOutlineSignature,
  type StrokeWorldOutline,
} from "./strokeHit";
import {
  buildStrokeBatch,
  buildStrokeOutlines,
  type StrokeBatch,
} from "./strokeRender";
import { imageBounds, intersects, viewportBounds, type Bounds, type Viewport } from "./geometry";
import type { GpuState } from "./rendererGpu";

/**
 * 笔迹缓冲的超扫边距（视口宽高的比例）：四周各扩这么多，平移在这个范围内既不改
 * 可见集合、也不改缓冲签名，因此拖动画布不重建；逐段裁剪用同一个框，边距内平移
 * 也不会露出被裁掉的段。
 */
const STROKE_OVERSCAN_RATIO = 0.5;

/** 一次屏幕笔迹缓冲同步所需的画板、可见条目、视口与缩放。 */
export type StrokeLayerSyncOptions = {
  boardId: number;
  images: readonly PinBoardItem[];
  isDrawable: (image: PinBoardItem) => boolean;
  viewport: Viewport;
  canvasWidth: number;
  canvasHeight: number;
  pixelsPerWorldUnit: number;
  gpu: GpuState;
  onError: (message: string) => void;
};

export class StrokeLayer {
  /** 当前画板的笔迹几何，按 itemId 索引；几何只走二进制通道，不进条目 DTO。 */
  private readonly geometries = new Map<number, PinBoardStrokeGeometry>();
  /** 笔迹缓冲的失效计数：几何或变换变化时递增，签名随之变化并触发重建。 */
  private version = 0;
  /** 上次构建笔迹缓冲时的「可见笔迹 + 缩放」签名，避免平移时逐帧重建。 */
  private signature = "";
  /**
   * 当前笔迹缓冲是按哪个「视口 + 超扫边距」框构建的。视口一旦走出它，被逐段
   * 裁掉的段就可能露出来；即使可见条目没变也必须重建。
   */
  private builtBounds: Bounds | null = null;
  /** 笔迹读取的代次：切画板或重复读取时丢弃旧结果。 */
  private loadToken = 0;
  private vertexBuffer: any = null;
  private indexBuffer: any = null;
  private indexCount = 0;
  /** 可见笔迹的选中描边，按 `outlineIndex` 给出的槽位连续排布。 */
  private outlineBuffer: any = null;
  private readonly outlineIndex = new Map<number, number>();
  /** 世界折线缓存：键是条目 id，签名见 `strokeOutlineSignature`。 */
  private readonly outlineCache = new Map<
    number,
    { signature: string; outlines: StrokeWorldOutline[] }
  >();

  get geometryMap(): ReadonlyMap<number, PinBoardStrokeGeometry> {
    return this.geometries;
  }

  get batch(): { vertexBuffer: any; indexBuffer: any; indexCount: number } | null {
    if (this.indexCount <= 0 || !this.vertexBuffer || !this.indexBuffer) return null;
    return {
      vertexBuffer: this.vertexBuffer,
      indexBuffer: this.indexBuffer,
      indexCount: this.indexCount,
    };
  }

  get selectionOutlineBuffer(): any {
    return this.outlineBuffer;
  }

  geometry(itemId: number): PinBoardStrokeGeometry | undefined {
    return this.geometries.get(itemId);
  }

  /** 整板换一份几何（来自读取或测试夹具）；缓冲签名随之失效。 */
  replaceGeometries(strokes: ReadonlyMap<number, PinBoardStrokeGeometry>): void {
    this.geometries.clear();
    for (const [itemId, geometry] of strokes) this.geometries.set(itemId, geometry);
    this.version += 1;
    this.signature = "";
  }

  outlineSlot(itemId: number): number | undefined {
    return this.outlineIndex.get(itemId);
  }

  /** 条目几何的世界折线（带缓存）；键含几何版本与变换签名，任一变化即失效。 */
  outlines(item: PinBoardStrokeItem, geometry: PinBoardStrokeGeometry): StrokeWorldOutline[] {
    const signature = strokeOutlineSignature(item, this.version);
    const cached = this.outlineCache.get(item.itemId);
    if (cached && cached.signature === signature) return cached.outlines;
    const outlines = buildStrokeWorldOutlines(item, geometry);
    this.outlineCache.set(item.itemId, { signature, outlines });
    return outlines;
  }

  /**
   * 读取整板笔迹几何（`read_pin_board_strokes` 的二进制载荷），一次解析成
   * `Map<itemId, StrokeGeometry>`。
   *
   * 与画板装载解耦：`loadBoard` 保持同步，笔迹在 IPC 回来后就地补画，因此切换画板
   * 不会为了几 KB 几何数据多等一次往返。
   */
  async load(
    boardId: number,
    isCurrent: () => boolean,
    onError: (message: string) => void,
  ): Promise<boolean> {
    const token = ++this.loadToken;
    try {
      const response = await pinBoardApi.readPinBoardStrokes(boardId);
      const bytes = response instanceof Uint8Array ? response : new Uint8Array(response as ArrayBuffer);
      const strokes = parseStrokeBatch(bytes);
      if (!isCurrent() || token !== this.loadToken) return false;
      this.replaceGeometries(strokes);
      diagnosticsLog(
        "info",
        `pin-board strokes loaded: boardId=${boardId}, strokes=${strokes.size}, bytes=${bytes.byteLength}`,
      );
      return true;
    } catch (error) {
      if (!isCurrent() || token !== this.loadToken) return false;
      diagnosticsLog("warn", `pin-board strokes failed: boardId=${boardId}, error=${errorMessage(error)}`);
      onError(`笔迹读取失败：${errorMessage(error)}`);
      return false;
    }
  }

  /** 释放几何、缓存与 GPU 缓冲；在途读取由代次丢弃。 */
  release(): void {
    this.loadToken += 1;
    this.geometries.clear();
    this.version += 1;
    this.destroyBuffers();
    this.signature = "";
    this.outlineCache.clear();
  }

  /** 变换变了但几何没变：只让缓冲签名失效。 */
  invalidateTransforms(): void {
    this.signature = "";
  }

  /** 设备丢失后旧缓冲全部作废，几何保留；随后由 `sync` 在新设备上重建。 */
  dropBuffers(): void {
    this.destroyBuffers();
    this.signature = "";
  }

  /**
   * 按「可见笔迹 + 缩放」重建笔迹缓冲。
   *
   * 签名不变**且**视口仍在缓冲的收集框内时跳过：平移只改视口位置、不改集合与
   * 缩放，因此拖动画布不会逐帧重建整板笔迹。
   */
  sync(options: StrokeLayerSyncOptions): void {
    const { canvasWidth, canvasHeight, viewport, pixelsPerWorldUnit } = options;
    const hasCanvas = canvasWidth > 0 && canvasHeight > 0;
    const visibleViewport = hasCanvas
      ? viewportBounds(viewport, canvasWidth, canvasHeight)
      : null;
    const cullBounds = visibleViewport ? this.cullBounds(visibleViewport) : null;
    const strokes = options.images.filter((image): image is PinBoardStrokeItem => (
      image.kind === "stroke"
      && options.isDrawable(image)
      && (cullBounds === null || intersects(imageBounds(image), cullBounds))
    ));
    const signature = [
      this.version,
      Math.round(pixelsPerWorldUnit * 100),
      strokes.map((stroke) => stroke.itemId).join(","),
    ].join("|");
    const builtBounds = this.builtBounds;
    const covered = visibleViewport !== null && builtBounds != null
      && visibleViewport.minX >= builtBounds.minX
      && visibleViewport.maxY <= builtBounds.maxY
      && visibleViewport.maxX <= builtBounds.maxX
      && visibleViewport.minY >= builtBounds.minY;
    if (signature === this.signature && (cullBounds === null || covered)) return;
    this.signature = signature;
    const entries = strokes.flatMap((item) => {
      const geometry = this.geometries.get(item.itemId);
      return geometry ? [{ item, geometry }] : [];
    });
    const batch = buildStrokeBatch(entries, {
      pixelsPerWorldUnit,
      ...(cullBounds ? { cullBounds } : {}),
    });
    const uploaded = this.upload(batch, buildStrokeOutlines(strokes), options);
    // 上传失败时不做「已覆盖」标记：下一帧还会重试，而不是永远跳过。
    this.builtBounds = uploaded ? cullBounds : null;
  }

  /** 返回是否已可复用（空缓冲也算成功）。失败时由调用方保留重试机会。 */
  private upload(
    batch: StrokeBatch,
    outlines: { vertices: Float32Array; index: Map<number, number> },
    options: StrokeLayerSyncOptions,
  ): boolean {
    this.destroyBuffers();
    if (batch.indices.length === 0) return true;
    try {
      const device = options.gpu.device;
      // 先挂到字段上再写数据：中途失败时 `destroyBuffers` 仍能释放已创建的缓冲。
      const vertexBuffer = device.createBuffer({
        size: batch.vertices.byteLength,
        usage: 0x20 | 0x08,
        mappedAtCreation: true,
      });
      this.vertexBuffer = vertexBuffer;
      new Float32Array(vertexBuffer.getMappedRange()).set(batch.vertices);
      vertexBuffer.unmap();
      const indexBuffer = device.createBuffer({
        size: batch.indices.byteLength,
        usage: 0x10 | 0x08,
        mappedAtCreation: true,
      });
      this.indexBuffer = indexBuffer;
      new Uint32Array(indexBuffer.getMappedRange()).set(batch.indices);
      indexBuffer.unmap();
      this.indexCount = batch.indices.length;
      for (const [itemId, slot] of outlines.index) this.outlineIndex.set(itemId, slot);
      if (outlines.vertices.length > 0) {
        const outlineBuffer = device.createBuffer({
          size: outlines.vertices.byteLength,
          usage: 0x20 | 0x08,
          mappedAtCreation: true,
        });
        this.outlineBuffer = outlineBuffer;
        new Float32Array(outlineBuffer.getMappedRange()).set(outlines.vertices);
        outlineBuffer.unmap();
      }
      return true;
    } catch (error) {
      this.destroyBuffers();
      diagnosticsLog(
        "warn",
        `pin-board stroke buffer failed: boardId=${options.boardId}, error=${errorMessage(error)}`,
      );
      options.onError(`笔迹缓冲创建失败：${errorMessage(error)}`);
      return false;
    }
  }

  private destroyBuffers(): void {
    this.vertexBuffer?.destroy();
    this.indexBuffer?.destroy();
    this.outlineBuffer?.destroy();
    this.vertexBuffer = null;
    this.indexBuffer = null;
    this.outlineBuffer = null;
    this.indexCount = 0;
    this.outlineIndex.clear();
    this.builtBounds = null;
  }

  /** 笔迹缓冲的收集范围：视口四周各扩 `STROKE_OVERSCAN_RATIO`。 */
  private cullBounds(viewport: Bounds): Bounds {
    const marginX = (viewport.maxX - viewport.minX) * STROKE_OVERSCAN_RATIO;
    const marginY = (viewport.maxY - viewport.minY) * STROKE_OVERSCAN_RATIO;
    return {
      minX: viewport.minX - marginX,
      minY: viewport.minY - marginY,
      maxX: viewport.maxX + marginX,
      maxY: viewport.maxY + marginY,
    };
  }
}
