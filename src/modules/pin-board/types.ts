/** 素材板领域 DTO；`api.ts` 是本领域 Tauri 命令的唯一前端入口。 */

import type { PinBoardStrokeTool } from "./strokeGeometry";

export type PinBoardTextureCacheLevel = "low" | "medium" | "high";

/**
 * 工作区传给素材板的设置快照（应用层与模块之间的边界类型，不是命令 DTO）。
 * 默认值由设置页与 `useSettingsController` 决定，模块只消费。
 */
export interface PinBoardModuleSettings {
  /** 阵列间距：相对统一图片高度的比例，`0` 表示图片紧贴。 */
  arrangementGapRatio: number;
  textureCacheLevel: PinBoardTextureCacheLevel;
  autosave: boolean;
  lockShortcut: string;
  fullscreenShortcut: string;
}

/** 阵列排序的单张输入：当前显示尺寸与世界坐标下的中心点。 */
export interface PinBoardArrangeItem {
  itemId: number;
  width: number;
  height: number;
  centerX: number;
  centerY: number;
}

export interface PinBoardArrangeResult {
  itemId: number;
  points: PinBoardItem["points"];
}

/** 画板摘要：`deleted` 为 true 时条目位于画板回收站。 */
export interface PinBoardSummary {
  boardId: number;
  artworkId: string;
  artworkTitle: string;
  name: string;
  sortOrder: number;
  deleted: boolean;
  deletedAt: number | null;
  /** 条目记录数，覆盖全部 kind（位图 / 文字 / 笔迹）。 */
  itemCount: number;
  createdMs: number;
  updatedMs: number;
}

/**
 * 画板内复制的载荷：**只记来源**。
 *
 * 显示尺寸、变换与载荷内容都由原生端读源条目现算——前端带尺寸时，那份值可能来自
 * 尚未落库的快照，与仓库里的真实记录不一致。
 */
export interface PinBoardClipboardItem {
  kind: PinBoardItem["kind"];
  sourceBoardId: number;
  sourceItemId: number;
}

export interface PinBoardTransferProgress {
  label: string;
  current: number;
  total: number;
  /** 这次传输能否中途取消：导入/导出图片与整画布导出可以，加文字这类瞬时动作不行。 */
  cancellable?: boolean;
}

export type PinBoardTextAlign = "left" | "center" | "right";

interface PinBoardItemBase {
  boardId: number;
  itemId: number;
  width: number;
  height: number;
  order: number;
  layer: number;
  deleted: boolean;
  points: [[number, number], [number, number], [number, number], [number, number]];
  uv: [[number, number], [number, number], [number, number], [number, number]];
  available: boolean;
}

/** 笔迹条目：几何只存数据库，由 `read_pin_board_strokes` 的二进制通道单独读取。 */
export interface PinBoardStrokeItem extends PinBoardItemBase {
  kind: "stroke";
  tool: PinBoardStrokeTool;
  color: string;
  strokeWidth: number;
}

/** 位图条目：纹理来自仓库内的 DDS 文件。 */
export interface PinBoardBitmapItem extends PinBoardItemBase {
  kind: "bitmap";
}

/** 文字条目：没有实体文件，由前端按需栅格化。 */
export interface PinBoardTextItem extends PinBoardItemBase {
  kind: "text";
  text: string;
  fontFamily: string;
  fontSize: number;
  color: string;
  align: PinBoardTextAlign;
}

export type PinBoardItem = PinBoardBitmapItem | PinBoardTextItem | PinBoardStrokeItem;

export interface PinBoardView {
  boardId: number;
  name: string;
  items: PinBoardItem[];
  missingTextures: number;
  revision: string;
  nowStep: number;
}

export interface PinBoardEditableItem {
  itemId: number;
  order: number;
  layer: number;
  deleted: boolean;
  points: PinBoardItem["points"];
  uv: PinBoardItem["uv"];
}

export interface SavePinBoardResult {
  saved: boolean;
  revision: string;
  nowStep: number;
}

export interface PinBoardMutationResult {
  view: PinBoardView;
  itemIds: number[];
}
