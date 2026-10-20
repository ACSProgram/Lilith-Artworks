import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../shared/tauri";
import type { PinBoardStrokeTool } from "./strokeGeometry";
import type {
  PinBoardArrangeItem,
  PinBoardArrangeResult,
  PinBoardClipboardImage,
  PinBoardEditableItem,
  PinBoardMutationResult,
  PinBoardSummary,
  PinBoardTextAlign,
  PinBoardView,
  SavePinBoardResult,
} from "./types";

/**
 * 进度不再走 `Channel`：导入/导出与其它长命令一样经 `CommandContext`
 * 广播 `operationProgress`，由统一订阅 bridge 分发。`Channel` 只保留给
 * 未来确实需要传输大块载荷的通道。
 */
export const pinBoardApi = {
  listPinBoards: (artworkId: string) =>
    invokeCommand<PinBoardSummary[]>("list_pin_boards", { artworkId }),
  listPinBoardTrash: () => invokeCommand<PinBoardSummary[]>("list_pin_board_trash"),
  createPinBoard: (artworkId: string, name: string) =>
    invokeCommand<PinBoardSummary[]>("create_pin_board", { artworkId, name }),
  reorderPinBoards: (artworkId: string, boardIds: number[]) =>
    invokeCommand<PinBoardSummary[]>("reorder_pin_boards", { artworkId, boardIds }),
  renamePinBoard: (boardId: number, name: string) =>
    invokeCommand<PinBoardSummary>("rename_pin_board", { boardId, name }),
  trashPinBoard: (boardId: number) =>
    invokeCommand<void>("trash_pin_board", { boardId }),
  restorePinBoard: (boardId: number) =>
    invokeCommand<PinBoardSummary>("restore_pin_board", { boardId }),
  deletePinBoardPermanently: (boardId: number) =>
    invokeCommand<{ cleanedCount: number; pendingCount: number }>(
      "delete_pin_board_permanently",
      { boardId },
    ),
  emptyPinBoardTrash: () =>
    invokeCommand<{ cleanedCount: number; pendingCount: number }>("empty_pin_board_trash"),
  loadPinBoard: (boardId: number) =>
    invokeCommand<PinBoardView>("load_pin_board", { boardId }),
  savePinBoard: (
    boardId: number,
    items: PinBoardEditableItem[],
    expectedRevision: string,
  ) => invokeCommand<SavePinBoardResult>("save_pin_board", { boardId, items, expectedRevision }),
  finalizePinBoard: (boardId: number, expectedRevision: string) =>
    invokeCommand<SavePinBoardResult>("finalize_pin_board", { boardId, expectedRevision }),
  pastePinBoardImages: (
    boardId: number,
    images: PinBoardClipboardImage[],
    centerX: number,
    centerY: number,
    gapRatio: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("paste_pin_board_images", {
    boardId, images, centerX, centerY, gapRatio, expectedRevision,
  }),
  importPinBoardImages: (
    boardId: number,
    paths: string[],
    centerX: number,
    centerY: number,
    gapRatio: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("import_pin_board_images", {
    boardId, paths, centerX, centerY, gapRatio, expectedRevision,
  }),
  importPinBoardClipboardImage: (
    boardId: number,
    bytes: number[],
    centerX: number,
    centerY: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("import_pin_board_clipboard_image", {
    boardId, bytes, centerX, centerY, expectedRevision,
  }),
  createPinBoardText: (
    boardId: number,
    text: string,
    fontSize: number,
    color: string,
    align: PinBoardTextAlign,
    measuredWidth: number,
    measuredHeight: number,
    centerX: number,
    centerY: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("create_pin_board_text", {
    boardId,
    text,
    fontSize,
    color,
    align,
    measuredWidth,
    measuredHeight,
    centerX,
    centerY,
    expectedRevision,
  }),
  /**
   * 新增笔迹条目。`geometry` 是 `strokeGeometry.ts` 编码的 BLOB：`Uint8Array` 会被
   * IPC 序列化成数字数组（与剪贴板图片的 `bytes` 同一条通道），不必手工 `Array.from`。
   * 图层由原生端强制写成标注层，这里不传。
   */
  createPinBoardStroke: (
    boardId: number,
    tool: PinBoardStrokeTool,
    color: string,
    strokeWidth: number,
    width: number,
    height: number,
    geometry: Uint8Array | number[],
    centerX: number,
    centerY: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("create_pin_board_stroke", {
    boardId,
    tool,
    color,
    strokeWidth,
    width,
    height,
    geometry,
    centerX,
    centerY,
    expectedRevision,
  }),
  /** 整板笔迹几何的二进制载荷，由 `parseStrokeBatch` 一次解析。 */
  readPinBoardStrokes: (boardId: number) =>
    invokeCommand<Uint8Array>("read_pin_board_strokes", { boardId }),
  exportPinBoardImages: (
    boardId: number,
    itemIds: number[],
    outputDirectory: string,
  ) => invokeCommand<number>("export_pin_board_images", {
    boardId, itemIds, outputDirectory,
  }),
  /** 写文字条目的导出图：前端编码 PNG，原生端校验后原子写盘。 */
  writePinBoardItemPng: (
    boardId: number,
    itemId: number,
    outputDirectory: string,
    bytes: number[],
  ) => invokeCommand<string>("write_pin_board_item_png", {
    boardId, itemId, outputDirectory, bytes,
  }),
  readPinBoardClipboardPaths: () => invokeCommand<string[]>("read_pin_board_clipboard_paths"),
  /**
   * 计算阵列排序后的位置。纯计算命令，不写数据库：结果由调用方交给
   * `commitMutation`，撤销栈与脏标记因此与其它几何操作一致。
   */
  arrangePinBoardImages: (items: PinBoardArrangeItem[], gapRatio: number) =>
    invokeCommand<PinBoardArrangeResult[]>("arrange_pin_board_images", { items, gapRatio }),
  readPinBoardImagePng: (boardId: number, itemId: number) =>
    invokeCommand<Uint8Array>("read_pin_board_image_png", { boardId, itemId }),
  readPinBoardTexture: (boardId: number, itemId: number, maxDimension: number) =>
    invokeCommand<Uint8Array>("read_pin_board_texture", { boardId, itemId, maxDimension }),
};

/** 文件对话框与窗口状态属于模块的 Tauri 访问边界，组件通过 api.ts 间接调用。 */
export async function pickPinBoardImagePaths(): Promise<string[]> {
  const picked = await openDialog({
    multiple: true,
    directory: false,
    filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tga", "dds"] }],
  });
  return picked === null ? [] : Array.isArray(picked) ? picked : [picked];
}

export async function pickPinBoardExportDirectory(): Promise<string | null> {
  const picked = await openDialog({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

export async function setPinBoardWindowFullscreen(enabled: boolean): Promise<void> {
  await getCurrentWindow().setFullscreen(enabled);
}

/** 原生窗口全屏状态属于窗口本身，页面重新加载后需要重新对齐。 */
export async function getPinBoardWindowFullscreen(): Promise<boolean> {
  return await getCurrentWindow().isFullscreen();
}
