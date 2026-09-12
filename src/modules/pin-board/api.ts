import { Channel } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../shared/tauri";
import type {
  PinBoardClipboardImage,
  PinBoardEditableImage,
  PinBoardMutationResult,
  PinBoardSummary,
  PinBoardTransferProgress,
  PinBoardView,
  SavePinBoardResult,
} from "./types";

function progressChannel(onProgress: (progress: PinBoardTransferProgress) => void) {
  const channel = new Channel<PinBoardTransferProgress>();
  channel.onmessage = onProgress;
  return channel;
}

export const pinBoardApi = {
  listPinBoards: (artworkId: string) =>
    invokeCommand<PinBoardSummary[]>("list_pin_boards", { artworkId }),
  listPinBoardTrash: () => invokeCommand<PinBoardSummary[]>("list_pin_board_trash"),
  createPinBoard: (artworkId: string, name: string) =>
    invokeCommand<PinBoardSummary[]>("create_pin_board", { artworkId, name }),
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
    images: PinBoardEditableImage[],
    expectedRevision: string,
  ) => invokeCommand<SavePinBoardResult>("save_pin_board", { boardId, images, expectedRevision }),
  finalizePinBoard: (boardId: number, expectedRevision: string) =>
    invokeCommand<SavePinBoardResult>("finalize_pin_board", { boardId, expectedRevision }),
  pastePinBoardImages: (
    boardId: number,
    images: PinBoardClipboardImage[],
    centerX: number,
    centerY: number,
    gap: number,
    expectedRevision: string,
  ) => invokeCommand<PinBoardMutationResult>("paste_pin_board_images", {
    boardId, images, centerX, centerY, gap, expectedRevision,
  }),
  importPinBoardImages: (
    boardId: number,
    paths: string[],
    centerX: number,
    centerY: number,
    gap: number,
    expectedRevision: string,
    onProgress: (progress: PinBoardTransferProgress) => void,
  ) => invokeCommand<PinBoardMutationResult>("import_pin_board_images", {
    boardId, paths, centerX, centerY, gap, expectedRevision,
    onProgress: progressChannel(onProgress),
  }),
  importPinBoardClipboardImage: (
    boardId: number,
    bytes: number[],
    centerX: number,
    centerY: number,
    expectedRevision: string,
    onProgress: (progress: PinBoardTransferProgress) => void,
  ) => invokeCommand<PinBoardMutationResult>("import_pin_board_clipboard_image", {
    boardId, bytes, centerX, centerY, expectedRevision,
    onProgress: progressChannel(onProgress),
  }),
  exportPinBoardImages: (
    boardId: number,
    imageIds: number[],
    outputDirectory: string,
    onProgress: (progress: PinBoardTransferProgress) => void,
  ) => invokeCommand<number>("export_pin_board_images", {
    boardId, imageIds, outputDirectory,
    onProgress: progressChannel(onProgress),
  }),
  readPinBoardClipboardPaths: () => invokeCommand<string[]>("read_pin_board_clipboard_paths"),
  readPinBoardImagePng: (boardId: number, imageId: number) =>
    invokeCommand<Uint8Array>("read_pin_board_image_png", { boardId, imageId }),
  readPinBoardTexture: (boardId: number, imageId: number, maxDimension: number) =>
    invokeCommand<Uint8Array>("read_pin_board_texture", { boardId, imageId, maxDimension }),
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
