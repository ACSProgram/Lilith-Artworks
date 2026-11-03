import { describe, expect, it, vi } from "vitest";
import { StrokeLayer } from "./strokeLayer";
import { STROKE_TOOL } from "./strokeGeometry";
import { strokeGeometryFixture } from "./pinBoardFixtures";
import type { PinBoardItem, PinBoardStrokeItem } from "./types";

function strokeItem(itemId = 5): PinBoardStrokeItem {
  return {
    boardId: 1,
    itemId,
    kind: "stroke",
    width: 100,
    height: 100,
    order: itemId,
    layer: 3,
    deleted: false,
    points: [[0, 0], [100, 0], [100, 100], [0, 100]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    tool: STROKE_TOOL.line,
    color: "#FF8000",
    strokeWidth: 2,
  } as unknown as PinBoardStrokeItem;
}

const geometry = strokeGeometryFixture({
  itemId: 5,
  tool: STROKE_TOOL.line,
  strokeWidth: 2,
  width: 100,
  height: 100,
  color: "#FF8000",
});

describe("pin-board stroke layer", () => {
  it("replaces the whole board geometry and exposes it by item id", () => {
    const layer = new StrokeLayer();
    layer.replaceGeometries(new Map([[5, geometry]]));

    expect(layer.geometry(5)).toBe(geometry);
    expect(layer.geometry(6)).toBeUndefined();

    layer.replaceGeometries(new Map());
    expect(layer.geometry(5)).toBeUndefined();
  });

  it("caches world outlines until the geometry or a transform invalidates them", () => {
    const layer = new StrokeLayer();
    layer.replaceGeometries(new Map([[5, geometry]]));
    const item = strokeItem();

    const first = layer.outlines(item, geometry);
    expect(layer.outlines(item, geometry)).toBe(first);

    // 变换签名参与折线缓存：条目移动后即使几何版本没变、也没有重建缓冲，取出的折线也必须重算。
    item.points = [[10, 10], [110, 10], [110, 110], [10, 110]];
    const afterTransform = layer.outlines(item, geometry);
    expect(afterTransform).not.toBe(first);

    layer.replaceGeometries(new Map([[5, geometry]]));
    expect(layer.outlines(item, geometry)).not.toBe(afterTransform);
  });

  it("builds and drops the screen buffers during sync and release", () => {
    const created: number[] = [];
    const device = {
      createBuffer: (descriptor: { size: number }) => {
        created.push(descriptor.size);
        return {
          getMappedRange: () => new ArrayBuffer(descriptor.size),
          unmap: () => undefined,
          destroy: () => undefined,
        };
      },
    };
    const layer = new StrokeLayer();
    layer.replaceGeometries(new Map([[5, geometry]]));

    const sync = () => layer.sync({
      boardId: 1,
      images: [strokeItem()] as PinBoardItem[],
      isDrawable: () => true,
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvasWidth: 100,
      canvasHeight: 100,
      pixelsPerWorldUnit: 1,
      gpu: { device } as never,
      onError: vi.fn(),
    });

    sync();
    expect(created).toHaveLength(3);
    expect(layer.batch?.indexCount).toBe(6);
    expect(layer.outlineSlot(5)).toBeDefined();

    sync();
    expect(created).toHaveLength(3);

    // 变换变化不作废几何，但会作废缓冲签名：几何没变也要重建一次。
    layer.invalidateTransforms();
    sync();
    expect(created).toHaveLength(6);

    layer.release();
    expect(layer.batch).toBeNull();
    expect(layer.geometry(5)).toBeUndefined();
    expect(layer.outlineSlot(5)).toBeUndefined();
  });
});
