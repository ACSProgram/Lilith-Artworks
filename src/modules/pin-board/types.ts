/** 素材板领域 DTO；`api.ts` 是本领域 Tauri 命令的唯一前端入口。 */

export type PinBoardTextureCacheLevel = "low" | "medium" | "high";

/** 画板摘要：`deleted` 为 true 时条目位于画板回收站。 */
export interface PinBoardSummary {
  boardId: number;
  artworkId: string;
  artworkTitle: string;
  name: string;
  sortOrder: number;
  deleted: boolean;
  deletedAt: number | null;
  imageCount: number;
  createdMs: number;
  updatedMs: number;
}

export interface PinBoardClipboardImage {
  sourceBoardId: number;
  sourceImageId: number;
  width: number;
  height: number;
  displayWidth: number;
  displayHeight: number;
  uv: PinBoardImage["uv"];
}

export interface PinBoardTransferProgress {
  label: string;
  current: number;
  total: number;
}

export interface PinBoardImage {
  boardId: number;
  imageId: number;
  width: number;
  height: number;
  order: number;
  layer: number;
  deleted: boolean;
  points: [[number, number], [number, number], [number, number], [number, number]];
  uv: [[number, number], [number, number], [number, number], [number, number]];
  available: boolean;
}

export interface PinBoardView {
  boardId: number;
  name: string;
  images: PinBoardImage[];
  missingTextures: number;
  revision: string;
  nowStep: number;
}

export interface PinBoardEditableImage {
  imageId: number;
  order: number;
  layer: number;
  deleted: boolean;
  points: PinBoardImage["points"];
  uv: PinBoardImage["uv"];
}

export interface SavePinBoardResult {
  saved: boolean;
  revision: string;
  nowStep: number;
}

export interface PinBoardMutationResult {
  view: PinBoardView;
  imageIds: number[];
}
