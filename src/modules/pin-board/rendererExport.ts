/**
 * 整画布导出的离屏渲染细节：导出目标纹理、世界矩形换算、笔迹缓冲与逐块回读。
 *
 * 所有入口都只收显式参数（设备、纹理表、条目集合），不直接访问渲染器的私有字段；
 * 这样「导出这一侧」改起来不会牵动屏幕渲染路径的状态。
 */

import type { CanvasExportPlan, CanvasExportTile } from "./exportPolicy";
import type { Bounds } from "./geometry";
import { imageSize } from "./geometry";
import { buildStrokeBatch } from "./strokeRender";
import type { PinBoardItem } from "./types";
import type { PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  BUFFER_USAGE_COPY_DST,
  BUFFER_USAGE_MAP_READ,
  EXPORT_TARGET_FORMAT,
  MAP_MODE_READ,
  READBACK_ROW_ALIGNMENT,
  TEXTURE_USAGE_COPY_SRC,
  TEXTURE_USAGE_RENDER_ATTACHMENT,
  type GpuState,
  type LoadedTexture,
} from "./rendererGpu";
import { requestedTextureDimension } from "./texturePolicy";
import { requestedTextTextureDimension } from "./textLayout";

export interface ExportStrokeBuffers {
  vertexBuffer: any;
  indexBuffer: any;
  indexCount: number;
}

/** 导出用的渲染目标：整次导出复用一张，逐块设视口，块外由清屏值填成透明。 */
export function createExportTexture(device: any, plan: CanvasExportPlan): any {
  return device.createTexture({
    size: {
      width: plan.tileWidth,
      height: Math.min(plan.bandHeight, plan.height),
      depthOrArrayLayers: 1,
    },
    format: EXPORT_TARGET_FORMAT,
    usage: TEXTURE_USAGE_RENDER_ATTACHMENT | TEXTURE_USAGE_COPY_SRC,
  });
}

/** 一块输出像素对应的世界矩形（y 轴向上，所以从 maxY 往下推）。 */
export function exportTileBounds(plan: CanvasExportPlan, tile: CanvasExportTile): Bounds {
  const minX = plan.bounds.minX + tile.x / plan.scale;
  const maxY = plan.bounds.maxY - tile.y / plan.scale;
  return {
    minX,
    maxX: minX + tile.width / plan.scale,
    maxY,
    minY: maxY - tile.height / plan.scale,
  };
}

/** 导出倍率下的纹理请求长边。位图受源图与设备上限约束，文字按同一套预算换算。 */
export function exportTextureDimension(
  image: PinBoardItem,
  scale: number,
  deviceLimit: number,
): number {
  const [worldWidth, worldHeight] = imageSize(image);
  const displayDimension = Math.max(worldWidth, worldHeight) * scale;
  if (image.kind === "text") {
    return requestedTextTextureDimension(image, displayDimension, deviceLimit);
  }
  return requestedTextureDimension(image, displayDimension, deviceLimit);
}

/**
 * 按导出区域单独构建笔迹缓冲：收集框是整个导出区域，逐段裁剪在 `buildStrokeBatch`
 * 里完成；屏幕那份缓冲一个字段都不动。
 */
export function buildExportStrokeBuffers(
  device: any,
  drawItems: readonly PinBoardItem[],
  geometries: ReadonlyMap<number, PinBoardStrokeGeometry>,
  plan: CanvasExportPlan,
): ExportStrokeBuffers | null {
  const entries = drawItems.flatMap((image) => {
    if (image.kind !== "stroke") return [];
    const geometry = geometries.get(image.itemId);
    return geometry ? [{ item: image, geometry }] : [];
  });
  if (entries.length === 0) return null;
  const batch = buildStrokeBatch(entries, {
    pixelsPerWorldUnit: plan.scale,
    cullBounds: plan.bounds,
  });
  if (batch.indices.length === 0) return null;
  let vertexBuffer: any = null;
  let indexBuffer: any = null;
  try {
    vertexBuffer = device.createBuffer({
      size: batch.vertices.byteLength,
      usage: 0x20 | 0x08,
      mappedAtCreation: true,
    });
    new Float32Array(vertexBuffer.getMappedRange()).set(batch.vertices);
    vertexBuffer.unmap();
    indexBuffer = device.createBuffer({
      size: batch.indices.byteLength,
      usage: 0x10 | 0x08,
      mappedAtCreation: true,
    });
    new Uint32Array(indexBuffer.getMappedRange()).set(batch.indices);
    indexBuffer.unmap();
    return { vertexBuffer, indexBuffer, indexCount: batch.indices.length };
  } catch (error) {
    indexBuffer?.destroy();
    vertexBuffer?.destroy();
    throw error;
  }
}

export function destroyExportStrokeBuffers(buffers: ExportStrokeBuffers | null): void {
  buffers?.vertexBuffer?.destroy();
  buffers?.indexBuffer?.destroy();
}

export interface ReadExportTileOptions {
  gpu: GpuState;
  texture: any;
  plan: CanvasExportPlan;
  tile: CanvasExportTile;
  /** 导出要画的条目：未删除、可用、所在图层可见，按 layer 再按 order（与屏幕同序）。 */
  drawItems: readonly PinBoardItem[];
  textures: ReadonlyMap<number, LoadedTexture>;
  previewTextures: ReadonlyMap<number, LoadedTexture>;
  strokeBuffers: ExportStrokeBuffers | null;
}

/**
 * 渲染一块并回读成紧排的 RGBA。
 *
 * 导出通道不画选中描边、纹理占位框、框选矩形与绘制预览——它们属于屏幕上的编辑
 * 提示，不是画板内容。清屏值是全透明，因此输出就是一张可直接叠到别处的 PNG。
 */
export async function readExportTile(options: ReadExportTileOptions): Promise<Uint8Array> {
  const { gpu, texture, plan, tile, drawItems, textures, previewTextures, strokeBuffers } = options;
  const device = gpu.device;
  const bounds = exportTileBounds(plan, tile);
  const rowBytes = tile.width * 4;
  // `bytesPerRow` 必须是 256 的倍数：8192 宽的块正好对齐，只有最后一块需要去 padding。
  const bytesPerRow = Math.ceil(rowBytes / READBACK_ROW_ALIGNMENT) * READBACK_ROW_ALIGNMENT;
  const readback = device.createBuffer({
    size: bytesPerRow * tile.height,
    usage: BUFFER_USAGE_MAP_READ | BUFFER_USAGE_COPY_DST,
  });
  try {
    device.queue.writeBuffer(gpu.exportViewportBuffer, 0, new Float32Array([
      bounds.minX,
      bounds.minY,
      bounds.maxX - bounds.minX,
      bounds.maxY - bounds.minY,
      // 笔迹的抗锯齿羽化按「设备像素 / 世界单位」换算，导出倍率就是它。
      plan.scale,
      0,
      0,
      0,
    ]));
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [{
        view: texture.createView(),
        clearValue: { r: 0, g: 0, b: 0, a: 0 },
        loadOp: "clear",
        storeOp: "store",
      }],
    });
    pass.setViewport(0, 0, tile.width, tile.height, 0, 1);
    pass.setPipeline(gpu.exportPipeline);
    pass.setBindGroup(0, gpu.exportViewportBindGroup);
    for (const image of drawItems) {
      if (image.kind === "stroke") continue;
      const loaded = textures.get(image.itemId) ?? previewTextures.get(image.itemId);
      if (!loaded) continue;
      pass.setBindGroup(1, loaded.bindGroup);
      pass.setVertexBuffer(0, loaded.vertexBuffer);
      pass.draw(6);
    }
    // 与屏幕同序：笔迹固定在最上层，整板合并成一次 indexed draw。
    if (strokeBuffers && strokeBuffers.indexCount > 0) {
      pass.setPipeline(gpu.exportStrokePipeline);
      pass.setBindGroup(0, gpu.exportViewportBindGroup);
      pass.setVertexBuffer(0, strokeBuffers.vertexBuffer);
      pass.setIndexBuffer(strokeBuffers.indexBuffer, "uint32");
      pass.drawIndexed(strokeBuffers.indexCount);
    }
    pass.end();
    encoder.copyTextureToBuffer(
      { texture },
      { buffer: readback, bytesPerRow, rowsPerImage: tile.height },
      { width: tile.width, height: tile.height, depthOrArrayLayers: 1 },
    );
    device.queue.submit([encoder.finish()]);
    await readback.mapAsync(MAP_MODE_READ);
    const mapped = new Uint8Array(readback.getMappedRange());
    if (bytesPerRow === rowBytes) return mapped.slice(0, rowBytes * tile.height);
    const packed = new Uint8Array(rowBytes * tile.height);
    for (let row = 0; row < tile.height; row += 1) {
      const source = row * bytesPerRow;
      packed.set(mapped.subarray(source, source + rowBytes), row * rowBytes);
    }
    return packed;
  } finally {
    readback.destroy();
  }
}
