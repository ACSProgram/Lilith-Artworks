/**
 * 笔迹几何的前端契约：工具编号、BLOB 布局与批量二进制的解析。
 *
 * 原生端的同一份契约在 `src-tauri/src/pin_board/stroke.rs`，两边的校验口径必须
 * 一致：任一侧改动都要同步另一侧与各自测试。几何只走数据库，不建纹理、不落文件。
 *
 * ```text
 * [0]      version = 1
 * [1]      tool     0=pen 1=line 2=arrow 3=rect 4=ellipse
 * [2]      flags    bit0-1 形状方向（起点落在哪个角）, bit2 是否带压感
 * [3]      reserved = 0
 * [4..7]   pointCount  u32（形状恒为 0）
 * [8..]    pen：pointCount × (u16 x, u16 y, u8 pressure)，x/y 为条目包围盒内的归一化坐标
 *          形状：无额外字节，两端点由包围盒 + 方向位推出
 * ```
 */

export const STROKE_GEOMETRY_VERSION = 1;
/** 单笔点数上限。编码侧超出即拒绝；抽稀与重采样由 `strokeSampling.ts` 在编码前完成。 */
export const MAX_STROKE_POINTS = 4096;
export const STROKE_HEADER_BYTES = 8;
export const STROKE_POINT_BYTES = 5;
export const MAX_STROKE_GEOMETRY_BYTES = STROKE_HEADER_BYTES + MAX_STROKE_POINTS * STROKE_POINT_BYTES;
/** 笔迹固定落在标注层，永远在所有图片与文字之上，不可调整。 */
export const STROKE_LAYER = 3;
export const MAX_STROKE_EXTENT = 1_000_000;
export const MAX_STROKE_WIDTH = 1000;

export const STROKE_TOOL = {
  pen: 0,
  line: 1,
  arrow: 2,
  rect: 3,
  ellipse: 4,
} as const;

export type PinBoardStrokeTool = (typeof STROKE_TOOL)[keyof typeof STROKE_TOOL];

const FLAG_DIRECTION_MASK = 0b0000_0011;
const FLAG_PRESSURE = 0b0000_0100;
const FLAG_RESERVED_MASK = 0b1111_1000;

export interface StrokeHeader {
  tool: PinBoardStrokeTool;
  flags: number;
  pointCount: number;
  /** 形状方向位（0–3），仅形状有意义。 */
  direction: number;
  hasPressure: boolean;
  isShape: boolean;
}

/** 批量二进制里每条目的固定头部长度（itemId / tool / color / 三个 f32 / geometryLen）。 */
export const STROKE_RECORD_HEADER_BYTES = 4 + 1 + 4 + 4 + 4 + 4 + 4;

export interface PinBoardStrokeGeometry extends StrokeHeader {
  itemId: number;
  /** `#RRGGBB`，与 `load_pin_board` 里的载荷同一口径。 */
  color: string;
  strokeWidth: number;
  /** 包围盒逻辑尺寸，与条目的 `width` / `height` 一致。 */
  width: number;
  height: number;
  /** 原始几何 BLOB（含 8 字节头部）；点数据由渲染层按 `tool` 解析。 */
  geometry: Uint8Array;
}

/** 校验几何 BLOB 并返回头部。口径与 `stroke::validate_geometry` 一一对应。 */
export function decodeStrokeHeader(geometry: Uint8Array): StrokeHeader {
  if (geometry.length < STROKE_HEADER_BYTES) throw new Error("笔迹几何数据不完整");
  if (geometry.length > MAX_STROKE_GEOMETRY_BYTES) throw new Error("笔迹几何数据超过上限");
  const view = new DataView(geometry.buffer, geometry.byteOffset, geometry.byteLength);
  const version = geometry[0];
  if (version !== STROKE_GEOMETRY_VERSION) throw new Error(`笔迹几何版本不受支持：${version}`);
  const tool = geometry[1];
  if (!Object.values(STROKE_TOOL).includes(tool as PinBoardStrokeTool)) {
    throw new Error(`未知的笔迹工具：${tool}`);
  }
  const flags = geometry[2];
  if ((flags & FLAG_RESERVED_MASK) !== 0) throw new Error("笔迹几何标志位无效");
  if (geometry[3] !== 0) throw new Error("笔迹几何的保留字节必须为 0");
  const pointCount = view.getUint32(4, true);
  if (pointCount > MAX_STROKE_POINTS) throw new Error("笔迹点数超过上限");
  const header: StrokeHeader = {
    tool: tool as PinBoardStrokeTool,
    flags,
    pointCount,
    direction: flags & FLAG_DIRECTION_MASK,
    hasPressure: (flags & FLAG_PRESSURE) !== 0,
    isShape: tool !== STROKE_TOOL.pen,
  };
  if (header.isShape) {
    if (pointCount !== 0) throw new Error("形状笔迹不应包含点数据");
    if (geometry.length !== STROKE_HEADER_BYTES) throw new Error("形状笔迹的几何长度无效");
    if (header.hasPressure) throw new Error("形状笔迹不应带压感标志");
    return header;
  }
  if (pointCount === 0) throw new Error("笔迹至少需要一个点");
  const expected = STROKE_HEADER_BYTES + pointCount * STROKE_POINT_BYTES;
  if (geometry.length !== expected) throw new Error("笔迹几何长度与点数不符");
  return header;
}

/**
 * 解析 `read_pin_board_strokes` 的整板载荷：`u32 条目数` + 每条目
 * `(u32 itemId, u8 tool, u32 color, f32 strokeWidth, f32 width, f32 height,
 * u32 geometryLen, bytes)`，小端。
 *
 * 一次解析成 `Map<itemId, StrokeGeometry>`，不经过 JSON。载荷被截断或几何损坏时
 * 抛错而不是返回半份数据——半份数据在渲染层会表现成「笔迹少了几条」。
 */
export function parseStrokeBatch(payload: Uint8Array): Map<number, PinBoardStrokeGeometry> {
  const strokes = new Map<number, PinBoardStrokeGeometry>();
  if (payload.byteLength < 4) throw new Error("笔迹载荷不完整");
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const count = view.getUint32(0, true);
  let offset = 4;
  for (let index = 0; index < count; index += 1) {
    if (offset + STROKE_RECORD_HEADER_BYTES > payload.byteLength) {
      throw new Error("笔迹载荷不完整");
    }
    const itemId = view.getUint32(offset, true);
    const tool = payload[offset + 4];
    const colorRgb = view.getUint32(offset + 5, true);
    const strokeWidth = view.getFloat32(offset + 9, true);
    const width = view.getFloat32(offset + 13, true);
    const height = view.getFloat32(offset + 17, true);
    const geometryLength = view.getUint32(offset + 21, true);
    offset += STROKE_RECORD_HEADER_BYTES;
    if (geometryLength > payload.byteLength - offset) throw new Error("笔迹载荷不完整");
    const geometry = payload.subarray(offset, offset + geometryLength);
    offset += geometryLength;
    const header = decodeStrokeHeader(geometry);
    if (header.tool !== tool) throw new Error(`笔迹 ${itemId} 的工具与几何头部不一致`);
    strokes.set(itemId, {
      ...header,
      itemId,
      color: rgbToHex(colorRgb),
      strokeWidth,
      width,
      height,
      geometry,
    });
  }
  if (offset !== payload.byteLength) throw new Error("笔迹载荷有多余字节");
  return strokes;
}

function rgbToHex(value: number): string {
  return `#${(value & 0x00ffffff).toString(16).toUpperCase().padStart(6, "0")}`;
}
