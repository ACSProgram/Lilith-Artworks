/**
 * 素材板测试共用的笔迹夹具：条目与几何 BLOB 只描述测试输入，不含断言。
 *
 * `strokeHit.test.ts` 与 `renderer.test.ts` 的命中用例必须用同一份条目形状，
 * 否则两边「同一条笔迹」的坐标会悄悄漂移。
 */

import {
  STROKE_TOOL,
  decodeStrokeHeader,
  encodeStrokeGeometry,
  type PinBoardStrokeGeometry,
  type PinBoardStrokeTool,
} from "./strokeGeometry";
import type { PinBoardStrokeItem } from "./types";

export const TEST_IDENTITY_UV: PinBoardStrokeItem["uv"] = [[0, 0], [1, 0], [1, 1], [0, 1]];

/** 默认是 100 × 20 的自由笔：四角就是世界坐标，便于把「局部」与「世界」对齐着断言。 */
export function strokeItemFixture(overrides: Partial<PinBoardStrokeItem> = {}): PinBoardStrokeItem {
  return {
    kind: "stroke",
    boardId: 1,
    itemId: 5,
    width: 100,
    height: 20,
    order: 0,
    layer: 3,
    deleted: false,
    points: [[0, 0], [100, 0], [100, 20], [0, 20]],
    uv: TEST_IDENTITY_UV,
    available: true,
    tool: STROKE_TOOL.pen,
    color: "#e5484d",
    strokeWidth: 10,
    ...overrides,
  };
}

/** 造一条几何：头部字段直接从编码结果里解回来，不在这里重抄一遍头部口径。 */
export function strokeGeometryFixture({
  itemId = 5,
  tool,
  samples = [],
  direction = 0,
  strokeWidth = 10,
  width = 100,
  height = 20,
  color = "#e5484d",
}: {
  itemId?: number;
  tool: PinBoardStrokeTool;
  samples?: readonly { u: number; v: number; pressure?: number }[];
  direction?: number;
  strokeWidth?: number;
  width?: number;
  height?: number;
  color?: string;
}): PinBoardStrokeGeometry {
  const geometry = encodeStrokeGeometry(tool, samples, direction);
  return {
    itemId,
    color,
    strokeWidth,
    width,
    height,
    ...decodeStrokeHeader(geometry),
    geometry,
  };
}
