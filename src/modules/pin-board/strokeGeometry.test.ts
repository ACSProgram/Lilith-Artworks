import { describe, expect, it } from "vitest";

import {
  MAX_STROKE_GEOMETRY_BYTES,
  MAX_STROKE_POINTS,
  parseStrokeBatch,
  STROKE_GEOMETRY_VERSION,
  STROKE_HEADER_BYTES,
  STROKE_POINT_BYTES,
  STROKE_RECORD_HEADER_BYTES,
  STROKE_TOOL,
} from "./strokeGeometry";

/** 构造一段自由笔几何：头部 + `points` 个顺序填充的点。 */
function penGeometry(points: number, flags = 0): Uint8Array {
  const bytes = new Uint8Array(STROKE_HEADER_BYTES + points * STROKE_POINT_BYTES);
  const view = new DataView(bytes.buffer);
  bytes[0] = STROKE_GEOMETRY_VERSION;
  bytes[1] = STROKE_TOOL.pen;
  bytes[2] = flags;
  bytes[3] = 0;
  view.setUint32(4, points, true);
  for (let index = 0; index < points; index += 1) {
    const offset = STROKE_HEADER_BYTES + index * STROKE_POINT_BYTES;
    view.setUint16(offset, Math.round((index / Math.max(points, 1)) * 65535), true);
    view.setUint16(offset + 2, Math.round((index / Math.max(points, 1)) * 65535), true);
    bytes[offset + 4] = 128;
  }
  return bytes;
}

function shapeGeometry(tool: number, direction: number, flags = 0): Uint8Array {
  const bytes = new Uint8Array(STROKE_HEADER_BYTES);
  const view = new DataView(bytes.buffer);
  bytes[0] = STROKE_GEOMETRY_VERSION;
  bytes[1] = tool;
  bytes[2] = flags | direction;
  bytes[3] = 0;
  view.setUint32(4, 0, true);
  return bytes;
}

interface RecordInput {
  itemId: number;
  tool: number;
  color: number;
  strokeWidth: number;
  width: number;
  height: number;
  geometry: Uint8Array;
}

/** 按原生布局拼一份整板载荷。 */
function batch(records: RecordInput[]): Uint8Array {
  const total =
    4 +
    records.reduce(
      (sum, record) => sum + STROKE_RECORD_HEADER_BYTES + record.geometry.byteLength,
      0,
    );
  const bytes = new Uint8Array(total);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, records.length, true);
  let offset = 4;
  for (const record of records) {
    view.setUint32(offset, record.itemId, true);
    bytes[offset + 4] = record.tool;
    view.setUint32(offset + 5, record.color, true);
    view.setFloat32(offset + 9, record.strokeWidth, true);
    view.setFloat32(offset + 13, record.width, true);
    view.setFloat32(offset + 17, record.height, true);
    view.setUint32(offset + 21, record.geometry.byteLength, true);
    bytes.set(record.geometry, offset + STROKE_RECORD_HEADER_BYTES);
    offset += STROKE_RECORD_HEADER_BYTES + record.geometry.byteLength;
  }
  return bytes;
}

describe("stroke geometry contract", () => {
  it("matches the native byte limits", () => {
    // 与 `stroke.rs` 的 `geometry_byte_limits_match_the_schema_check` 对齐。
    expect(STROKE_HEADER_BYTES).toBe(8);
    expect(STROKE_POINT_BYTES).toBe(5);
    expect(MAX_STROKE_GEOMETRY_BYTES).toBe(20488);
    expect(MAX_STROKE_POINTS).toBe(4096);
  });

  it("parses an empty batch as zero strokes", () => {
    expect(parseStrokeBatch(new Uint8Array([0, 0, 0, 0])).size).toBe(0);
  });

  it("parses pen and shape records into keyed geometry", () => {
    const geometry = penGeometry(2);
    const strokes = parseStrokeBatch(
      batch([
        {
          itemId: 7,
          tool: STROKE_TOOL.pen,
          color: 0x010203,
          strokeWidth: 2.5,
          width: 100,
          height: 40,
          geometry,
        },
        {
          itemId: 9,
          tool: STROKE_TOOL.rect,
          color: 0xaabbcc,
          strokeWidth: 1,
          width: 30,
          height: 30,
          geometry: shapeGeometry(STROKE_TOOL.rect, 2),
        },
      ]),
    );

    expect([...strokes.keys()]).toEqual([7, 9]);
    const pen = strokes.get(7)!;
    expect(pen.tool).toBe(STROKE_TOOL.pen);
    expect(pen.pointCount).toBe(2);
    expect(pen.isShape).toBe(false);
    expect(pen.color).toBe("#010203");
    expect(pen.strokeWidth).toBeCloseTo(2.5);
    expect(pen.width).toBeCloseTo(100);
    expect(pen.height).toBeCloseTo(40);
    expect(pen.geometry.byteLength).toBe(geometry.byteLength);

    const rect = strokes.get(9)!;
    expect(rect.isShape).toBe(true);
    expect(rect.direction).toBe(2);
    expect(rect.pointCount).toBe(0);
    expect(rect.color).toBe("#AABBCC");
    expect(rect.geometry.byteLength).toBe(STROKE_HEADER_BYTES);
  });

  it("rejects truncated, oversized and inconsistent payloads", () => {
    expect(() => parseStrokeBatch(new Uint8Array(2))).toThrow("不完整");

    const truncated = batch([
      {
        itemId: 1,
        tool: STROKE_TOOL.pen,
        color: 0,
        strokeWidth: 1,
        width: 1,
        height: 1,
        geometry: penGeometry(1),
      },
    ]);
    expect(() => parseStrokeBatch(truncated.subarray(0, truncated.byteLength - 2))).toThrow(
      "不完整",
    );

    const extra = new Uint8Array([...batch([]), 0, 0]);
    expect(() => parseStrokeBatch(extra)).toThrow("多余字节");

    // 列里的工具与几何头部不一致：编码端有 bug，宁可报错也不画错形状。
    const mismatched = batch([
      {
        itemId: 3,
        tool: STROKE_TOOL.arrow,
        color: 0,
        strokeWidth: 1,
        width: 1,
        height: 1,
        geometry: penGeometry(1),
      },
    ]);
    expect(() => parseStrokeBatch(mismatched)).toThrow("不一致");
  });

  it("mirrors the native geometry validation", () => {
    const mutate = (change: (bytes: Uint8Array, view: DataView) => void) => {
      const geometry = penGeometry(1);
      change(geometry, new DataView(geometry.buffer));
      return batch([
        {
          itemId: 1,
          tool: geometry[1],
          color: 0,
          strokeWidth: 1,
          width: 1,
          height: 1,
          geometry,
        },
      ]);
    };

    expect(() =>
      parseStrokeBatch(mutate((bytes) => { bytes[0] = 2; })),
    ).toThrow("版本不受支持");
    expect(() =>
      parseStrokeBatch(mutate((bytes) => { bytes[1] = 9; })),
    ).toThrow("未知的笔迹工具");
    expect(() =>
      parseStrokeBatch(mutate((bytes) => { bytes[2] = 0b1000_0000; })),
    ).toThrow("标志位无效");
    expect(() =>
      parseStrokeBatch(mutate((bytes) => { bytes[3] = 1; })),
    ).toThrow("保留字节");
    expect(() =>
      parseStrokeBatch(mutate((_bytes, view) => { view.setUint32(4, 0, true); })),
    ).toThrow("至少需要一个点");
    expect(() =>
      parseStrokeBatch(
        mutate((_bytes, view) => { view.setUint32(4, MAX_STROKE_POINTS + 1, true); }),
      ),
    ).toThrow("点数超过上限");
    expect(() =>
      parseStrokeBatch(
        mutate((_bytes, view) => { view.setUint32(4, 2, true); }),
      ),
    ).toThrow("长度与点数不符");
  });

  it("rejects shapes carrying points or pressure", () => {
    const withPoints = shapeGeometry(STROKE_TOOL.line, 0);
    new DataView(withPoints.buffer).setUint32(4, 1, true);
    expect(() =>
      parseStrokeBatch(
        batch([
          {
            itemId: 1,
            tool: STROKE_TOOL.line,
            color: 0,
            strokeWidth: 1,
            width: 1,
            height: 1,
            geometry: withPoints,
          },
        ]),
      ),
    ).toThrow("不应包含点数据");

    const withPressure = shapeGeometry(STROKE_TOOL.line, 0, 0b0000_0100);
    expect(() =>
      parseStrokeBatch(
        batch([
          {
            itemId: 1,
            tool: STROKE_TOOL.line,
            color: 0,
            strokeWidth: 1,
            width: 1,
            height: 1,
            geometry: withPressure,
          },
        ]),
      ),
    ).toThrow("不应带压感");
  });
});
