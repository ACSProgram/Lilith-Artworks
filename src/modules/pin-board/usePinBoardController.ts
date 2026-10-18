import { useCallback, useEffect, useRef, useState } from "react";
import { diagnosticsLog } from "../../shared/diagnostics";
import { errorMessage } from "../../shared/errors";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { useRuntimeSubscription } from "../../shared/useRuntimeSubscription";
import {
  pinBoardApi,
  pickPinBoardExportDirectory,
  pickPinBoardImagePaths,
} from "./api";
import { reorderBoardIds, type BoardDropPosition } from "./boardOrder";
import { registerPinBoardLifecycleParticipant } from "./lifecycle";
import type { PinBoardRenderer, PinBoardInteractionState } from "./renderer";
import { getBoardSession, getSelectedBoardId, setSelectedBoardId } from "./session";
import { textureBudgetsForLevel } from "./texturePolicy";
import type {
  PinBoardClipboardImage,
  PinBoardModuleSettings,
  PinBoardMutationResult,
  PinBoardSummary,
  PinBoardTransferProgress,
  PinBoardView,
} from "./types";

const IMAGE_PATH_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif|tga|dds)$/i;

/**
 * 拖入导入走「原始字节解码」路径（`import_pin_board_clipboard_image`），只能接受
 * 可被内容嗅探识别的位图格式；DDS/TGA 需要文件路径，仍通过「导入图片」选择器处理。
 */
const DROP_IMAGE_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif)$/i;

const DEFAULT_INTERACTION: PinBoardInteractionState = {
  imageCount: 0,
  selectedCount: 0,
  canUndo: false,
  canRedo: false,
  dirty: false,
  saving: false,
  locked: true,
  zoomPercent: 100,
};

async function clipboardImageBytes(): Promise<number[] | null> {
  if (!navigator.clipboard?.read) return null;
  try {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      const imageType = ["image/png", "image/jpeg", "image/webp", "image/bmp", "image/gif"]
        .find((type) => item.types.includes(type));
      if (!imageType) continue;
      const blob = await item.getType(imageType);
      return Array.from(new Uint8Array(await blob.arrayBuffer()));
    }
  } catch {
    // Clipboard access may be unavailable; import falls back to paths and the dialog.
  }
  return null;
}

async function renderTextImage(text: string, fontSize: number, color: string): Promise<number[]> {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) throw new Error("无法创建文字画布");
  const font = `${fontSize}px "Microsoft YaHei UI", "Segoe UI", sans-serif`;
  const padding = Math.max(12, Math.ceil(fontSize * 0.35));
  const lineHeight = Math.ceil(fontSize * 1.3);
  context.font = font;
  const textWidth = Math.max(...lines.map((line) => context.measureText(line || " ").width));
  const width = Math.max(4, Math.ceil(textWidth + padding * 2));
  const height = Math.max(4, lineHeight * lines.length + padding * 2);
  canvas.width = Math.ceil(width / 4) * 4;
  canvas.height = Math.ceil(height / 4) * 4;
  context.clearRect(0, 0, canvas.width, canvas.height);
  context.font = font;
  context.fillStyle = color;
  context.textBaseline = "top";
  lines.forEach((line, index) => context.fillText(line, padding, padding + index * lineHeight));
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) resolve(result);
      else reject(new Error("无法生成文字图片"));
    }, "image/png");
  });
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}

export interface PinBoardControllerOptions {
  artworkId: string;
  active: boolean;
  settings: PinBoardModuleSettings;
}

export interface PinBoardController {
  boards: PinBoardSummary[] | null;
  trash: PinBoardSummary[];
  view: PinBoardView | null;
  selected: number | null;
  status: string | null;
  transferProgress: PinBoardTransferProgress | null;
  clipboardCount: number;
  interaction: PinBoardInteractionState;
  renderer: PinBoardRenderer | null;
  setRenderer: (renderer: PinBoardRenderer | null) => void;
  setStatus: (message: string | null) => void;
  setInteraction: (state: PinBoardInteractionState) => void;
  syncInteraction: () => void;
  selectBoard: (board: PinBoardSummary) => Promise<void>;
  moveBoard: (draggedId: number, targetId: number, position: BoardDropPosition) => void;
  createBoard: (name: string) => Promise<void>;
  renameBoard: (boardId: number, name: string) => Promise<void>;
  trashBoard: (board: PinBoardSummary) => Promise<void>;
  restoreBoard: (boardId: number) => Promise<void>;
  permanentlyDeleteBoard: (boardId: number) => Promise<void>;
  emptyTrash: () => Promise<void>;
  save: () => Promise<boolean>;
  toggleLock: () => void;
  copyImages: () => void;
  copyImageToClipboard: () => Promise<void>;
  pasteImages: () => void;
  importImages: () => Promise<void>;
  importDroppedFiles: (files: FileList, clientX: number, clientY: number) => Promise<void>;
  addText: (text: string, fontSize: number, color: string, placement: [number, number]) => Promise<void>;
  deleteImages: () => void;
  exportImages: () => Promise<void>;
}

/**
 * 素材板控制器：画板列表、选择、保存/导入/导出与生命周期结算。
 * `PinBoardRenderer` 仍是纯渲染器，只负责 GPU 与交互；组件只保留视图与弹窗。
 * 导入/导出进度由统一 `AppEvent` 通道送达，不再为进度开 `Channel`。
 */
export function usePinBoardController({
  artworkId,
  active,
  settings,
}: PinBoardControllerOptions): PinBoardController {
  const [boards, setBoards] = useState<PinBoardSummary[] | null>(null);
  const [trash, setTrash] = useState<PinBoardSummary[]>([]);
  const [view, setView] = useState<PinBoardView | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [transferProgress, setTransferProgress] = useState<PinBoardTransferProgress | null>(null);
  const [clipboardCount, setClipboardCount] = useState(0);
  const [interaction, setInteraction] = useState(DEFAULT_INTERACTION);
  const [renderer, setRenderer] = useState<PinBoardRenderer | null>(null);
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const clipboardRef = useRef<PinBoardClipboardImage[]>([]);
  const transferBusyRef = useRef(false);
  /** 当前传输的 operation id：首个进度事件到达后锁定，其它操作的进度直接丢弃。 */
  const transferOperationId = useRef<string | null>(null);
  const reorderBusyRef = useRef(false);
  const unsubscribeLifecycleRef = useRef<(() => void) | null>(null);
  const {
    begin: beginLoad,
    invalidate: invalidateLoad,
    isCurrent: isLoadCurrent,
  } = useRequestEpoch();
  /**
   * 读取代次只解决「并发读取谁生效」；画板列表的写操作（增改删、排序）没有代次，
   * 因此在结果回填前另外比对作品 id，避免切换作品后旧作品的列表覆盖新作品。
   */
  const artworkIdRef = useRef(artworkId);
  artworkIdRef.current = artworkId;

  const applyRenderer = useCallback((next: PinBoardRenderer | null) => {
    rendererRef.current = next;
    setRenderer(next);
  }, []);

  // 生命周期结算注册到 controller：退出、切换仓库与切换作品都经同一入口。
  useEffect(() => {
    unsubscribeLifecycleRef.current?.();
    unsubscribeLifecycleRef.current = renderer
      ? registerPinBoardLifecycleParticipant(renderer)
      : null;
    return () => {
      unsubscribeLifecycleRef.current?.();
      unsubscribeLifecycleRef.current = null;
    };
  }, [renderer]);

  useEffect(() => {
    if (!status) return undefined;
    const timer = window.setTimeout(() => setStatus(null), 4_500);
    return () => window.clearTimeout(timer);
  }, [status]);

  useEffect(() => {
    rendererRef.current?.setActive(active);
  }, [active]);

  useEffect(() => {
    rendererRef.current?.setArrangementGapCssPixels(settings.arrangementGapPx);
  }, [settings.arrangementGapPx]);

  useEffect(() => {
    rendererRef.current?.setAutosaveEnabled(settings.autosave);
  }, [settings.autosave]);

  useEffect(() => {
    rendererRef.current?.setTextureCacheBudgets(textureBudgetsForLevel(settings.textureCacheLevel));
  }, [settings.textureCacheLevel]);

  // 导入/导出经 `CommandContext` 广播进度；只在本模块确实有传输时消费。
  // 首个进度事件锁定 operation id，避免并发/迟到的其它操作覆盖当前进度。
  useRuntimeSubscription((event) => {
    if (!transferBusyRef.current || event.domain !== "pinBoard") return;
    if (event.type !== "operationProgress") return;
    if (transferOperationId.current === null && event.operationId) {
      transferOperationId.current = event.operationId;
    }
    if (transferOperationId.current
      && event.operationId
      && event.operationId !== transferOperationId.current) {
      return;
    }
    setTransferProgress({
      label: event.phase ?? "正在处理图片",
      current: event.current ?? 0,
      total: event.total ?? 0,
    });
  });

  const refreshBoards = useCallback(async () => {
    const [nextBoards, nextTrash] = await Promise.all([
      pinBoardApi.listPinBoards(artworkId),
      pinBoardApi.listPinBoardTrash(),
    ]);
    if (artworkIdRef.current === artworkId) {
      setBoards(nextBoards);
      setTrash(nextTrash);
    }
    return nextBoards;
  }, [artworkId]);

  useEffect(() => {
    const sequence = beginLoad();
    diagnosticsLog(
      "info",
      `pin-board artwork switch start: artworkId=${artworkId}, hadRenderer=${rendererRef.current !== null}`,
    );
    setBoards(null);
    setTrash([]);
    setSelected(null);
    setInteraction(DEFAULT_INTERACTION);
    setStatus(null);

    void pinBoardApi.listPinBoards(artworkId)
      .then(async (next) => {
        if (!isLoadCurrent(sequence)) return;
        setBoards(next);
        diagnosticsLog(
          "info",
          `pin-board artwork switch done: artworkId=${artworkId}, boards=${next.length}`,
        );
        setTrash(await pinBoardApi.listPinBoardTrash().catch(() => []));
        const preferredId = getSelectedBoardId(artworkId);
        const first = next.find((board) => board.boardId === preferredId) ?? next[0];
        if (!first) {
          setView(null);
          return;
        }
        setSelected(first.boardId);
        setSelectedBoardId(artworkId, first.boardId);
        void pinBoardApi.loadPinBoard(first.boardId)
          .then((loaded) => {
            if (isLoadCurrent(sequence)) setView(loaded);
          })
          .catch((error) => {
            if (isLoadCurrent(sequence)) setStatus(errorMessage(error));
          });
      })
      .catch((error) => {
        if (isLoadCurrent(sequence)) setStatus(errorMessage(error));
      });
    return invalidateLoad;
  }, [artworkId, beginLoad, invalidateLoad, isLoadCurrent]);

  const syncInteraction = useCallback(() => {
    setInteraction(rendererRef.current?.interactionState ?? DEFAULT_INTERACTION);
  }, []);

  const selectBoard = useCallback(async (board: PinBoardSummary) => {
    if (board.boardId === selected) return;
    const sequence = beginLoad();
    diagnosticsLog(
      "info",
      `pin-board board select start: artworkId=${artworkId}, boardId=${board.boardId}`,
    );
    try {
      const loaded = await pinBoardApi.loadPinBoard(board.boardId);
      if (!isLoadCurrent(sequence)) return;
      const current = rendererRef.current;
      if (current) {
        const switched = await current.loadBoard(
          loaded,
          getBoardSession(artworkId, loaded.boardId),
        );
        if (!switched || !isLoadCurrent(sequence)) return;
      }
      setSelected(board.boardId);
      setSelectedBoardId(artworkId, board.boardId);
      setView(loaded);
      setInteraction(rendererRef.current?.interactionState ?? DEFAULT_INTERACTION);
      diagnosticsLog(
        "info",
        `pin-board board select done: artworkId=${artworkId}, boardId=${board.boardId}`,
      );
    } catch (error) {
      diagnosticsLog(
        "warn",
        `pin-board board select failed: artworkId=${artworkId}, boardId=${board.boardId}, error=${errorMessage(error)}`,
      );
      if (isLoadCurrent(sequence)) setStatus(errorMessage(error));
    }
  }, [artworkId, beginLoad, isLoadCurrent, selected]);

  const moveBoard = useCallback((
    draggedId: number,
    targetId: number,
    position: BoardDropPosition,
  ) => {
    if (!boards || reorderBusyRef.current || transferBusyRef.current) return;
    const nextOrder = reorderBoardIds(
      boards.map((board) => board.boardId),
      draggedId,
      targetId,
      position,
    );
    if (!nextOrder) return;
    const byId = new Map(boards.map((board) => [board.boardId, board]));
    const optimistic = nextOrder
      .map((boardId) => byId.get(boardId))
      .filter((board): board is PinBoardSummary => board !== undefined);
    const previous = boards;
    setBoards(optimistic);
    reorderBusyRef.current = true;
    void pinBoardApi.reorderPinBoards(artworkId, nextOrder)
      .then((next) => {
        if (artworkIdRef.current === artworkId) setBoards(next);
      })
      .catch((error) => {
        if (artworkIdRef.current !== artworkId) return;
        setBoards(previous);
        setStatus(errorMessage(error));
      })
      .finally(() => {
        reorderBusyRef.current = false;
      });
  }, [artworkId, boards]);

  const createBoard = useCallback(async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      const next = await pinBoardApi.createPinBoard(artworkId, trimmed);
      if (artworkIdRef.current !== artworkId) return;
      setBoards(next);
      if (selected === null) {
        const first = next[0];
        if (first) {
          setSelected(first.boardId);
          setSelectedBoardId(artworkId, first.boardId);
          const loaded = await pinBoardApi.loadPinBoard(first.boardId);
          if (artworkIdRef.current === artworkId) setView(loaded);
        }
      }
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [artworkId, selected]);

  const renameBoard = useCallback(async (boardId: number, name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await pinBoardApi.renamePinBoard(boardId, trimmed);
      await refreshBoards();
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [refreshBoards]);

  const trashBoard = useCallback(async (board: PinBoardSummary) => {
    try {
      await pinBoardApi.trashPinBoard(board.boardId);
      const next = await refreshBoards();
      // 切换作品后旧画板的回收站流程不再作用于新作品的画布。
      if (artworkIdRef.current !== artworkId) return;
      if (selected !== null && board.boardId === selected) {
        // 画板已进入回收站：对旧画板跳过 finalize 保存（否则会对已删除画板再次写库）。
        const current = rendererRef.current;
        setSelected(null);
        setInteraction(DEFAULT_INTERACTION);
        const fallback = next[0];
        if (fallback) {
          const loaded = await pinBoardApi.loadPinBoard(fallback.boardId);
          await current?.loadBoard(
            loaded,
            getBoardSession(artworkId, loaded.boardId),
            false,
          );
          setSelected(fallback.boardId);
          setSelectedBoardId(artworkId, fallback.boardId);
          setView(loaded);
        } else {
          await current?.unloadBoard(false);
          setView(null);
        }
      }
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [artworkId, refreshBoards, selected]);

  const restoreBoard = useCallback(async (boardId: number) => {
    try {
      await pinBoardApi.restorePinBoard(boardId);
      await refreshBoards();
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [refreshBoards]);

  const permanentlyDeleteBoard = useCallback(async (boardId: number) => {
    try {
      const report = await pinBoardApi.deletePinBoardPermanently(boardId);
      await refreshBoards();
      if (artworkIdRef.current !== artworkId) return;
      if (report.pendingCount > 0) {
        setStatus(`画板已删除，但有 ${report.pendingCount} 个图片文件仍待清理，将在下次启动时重试。`);
      }
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [artworkId, refreshBoards]);

  const emptyTrash = useCallback(async () => {
    try {
      const report = await pinBoardApi.emptyPinBoardTrash();
      await refreshBoards();
      if (artworkIdRef.current !== artworkId) return;
      if (report.pendingCount > 0) {
        setStatus(`回收站已清空，但有 ${report.pendingCount} 个图片文件仍待清理，将在下次启动时重试。`);
      }
    } catch (error) {
      setStatus(errorMessage(error));
    }
  }, [artworkId, refreshBoards]);

  const replaceAfterImageMutation = useCallback(async (
    action: (renderer: PinBoardRenderer, revision: string) => Promise<PinBoardMutationResult>,
    success: (count: number) => string,
  ): Promise<PinBoardMutationResult | null> => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked) return null;
    const sequence = beginLoad();
    if (!await current.save() || !isLoadCurrent(sequence)) return null;
    try {
      const result = await action(current, current.boardRevision);
      if (!isLoadCurrent(sequence)) return null;
      current.applyMutationResult(result.view, result.imageIds);
      setInteraction(current.interactionState);
      setStatus(success(result.imageIds.length));
      return result;
    } catch (error) {
      if (isLoadCurrent(sequence)) setStatus(errorMessage(error));
      return null;
    }
  }, [beginLoad, isLoadCurrent]);

  const save = useCallback(async () => {
    const current = rendererRef.current;
    if (!current) return false;
    return current.save();
  }, []);

  const toggleLock = useCallback(() => {
    const current = rendererRef.current;
    if (!current) return;
    current.toggleLock();
    setInteraction(current.interactionState);
  }, []);

  const copyImages = useCallback(() => {
    const images = rendererRef.current?.copySelection() ?? [];
    if (images.length === 0) return;
    clipboardRef.current = images;
    setClipboardCount(images.length);
    setStatus(`已复制 ${images.length} 张画板图片`);
  }, []);

  const copyImageToClipboard = useCallback(async () => {
    const current = rendererRef.current;
    const imageId = current?.copyImageToClipboardId() ?? null;
    if (!current || imageId === null || selected === null) return;
    try {
      const response = await pinBoardApi.readPinBoardImagePng(selected, imageId);
      const bytes = response instanceof Uint8Array ? response : new Uint8Array(response);
      const buffer = bytes.slice().buffer as ArrayBuffer;
      await navigator.clipboard.write([
        new ClipboardItem({ "image/png": new Blob([buffer], { type: "image/png" }) }),
      ]);
      setStatus("已复制图片到系统剪贴板");
    } catch (error) {
      setStatus(`无法复制图片到系统剪贴板：${errorMessage(error)}`);
    }
  }, [selected]);

  const pasteImages = useCallback(() => {
    if (clipboardRef.current.length === 0 || selected === null) return;
    void replaceAfterImageMutation((current, revision) => {
      const [centerX, centerY] = current.placementPoint;
      return pinBoardApi.pastePinBoardImages(
        selected,
        clipboardRef.current,
        centerX,
        centerY,
        current.placementGap(settings.arrangementGapPx),
        revision,
      );
    }, (count) => `已粘贴 ${count} 张图片`);
  }, [replaceAfterImageMutation, selected, settings.arrangementGapPx]);

  const importImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    try {
      const bytes = await clipboardImageBytes();
      if (bytes) {
        setTransferProgress({ label: "正在处理剪贴板图片", current: 0, total: 3 });
        await replaceAfterImageMutation((renderer, revision) => {
          const [centerX, centerY] = renderer.placementPoint;
          return pinBoardApi.importPinBoardClipboardImage(
            selected,
            bytes,
            centerX,
            centerY,
            revision,
          );
        }, (count) => `已从剪贴板导入 ${count} 张图片`);
        return;
      }

      // 路径来源只剩两条：剪贴板里的文件（由 `read_pin_board_clipboard_paths`
      // 读出并加入文件系统 scope）与文件选择器。粘贴纯文本路径不再导入——那种
      // 路径没有任何授权来源，Rust 层会以「未由文件选择器授权」拒绝。
      let paths: string[] = [];
      try {
        paths = (await pinBoardApi.readPinBoardClipboardPaths())
          .filter((path) => IMAGE_PATH_PATTERN.test(path));
      } catch {
        // Fall back to the file picker when the native clipboard is unavailable.
      }
      if (paths.length === 0) {
        paths = await pickPinBoardImagePaths();
        if (paths.length === 0) return;
      }
      setTransferProgress({ label: "正在导入图片", current: 0, total: paths.length });
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = renderer.placementPoint;
        return pinBoardApi.importPinBoardImages(
          selected,
          paths,
          centerX,
          centerY,
          renderer.placementGap(settings.arrangementGapPx),
          revision,
        );
      }, (count) => `已导入 ${count} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected, settings.arrangementGapPx]);

  const importDroppedFiles = useCallback(async (
    files: FileList,
    clientX: number,
    clientY: number,
  ) => {
    const current = rendererRef.current;
    if (!current || selected === null || transferBusyRef.current) return;
    if (current.interactionState.locked) {
      setStatus("画板已锁定，无法导入图片");
      return;
    }
    const images = Array.from(files).filter((file) => DROP_IMAGE_PATTERN.test(file.name));
    if (images.length === 0) {
      setStatus("请拖入 PNG、JPEG、WebP、BMP 或 GIF 图片");
      return;
    }
    transferBusyRef.current = true;
    transferOperationId.current = null;
    current.setPlacementFromClient(clientX, clientY);
    const gap = current.placementGap(settings.arrangementGapPx);
    let [centerX, centerY] = current.placementPoint;
    try {
      let imported = 0;
      for (const file of images) {
        const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
        if (bytes.length === 0) continue;
        setTransferProgress({ label: "正在导入图片", current: imported, total: images.length });
        const result = await replaceAfterImageMutation(
          (renderer, revision) => pinBoardApi.importPinBoardClipboardImage(
            selected,
            bytes,
            centerX,
            centerY,
            revision,
          ),
          (count) => `已导入 ${count} 张图片`,
        );
        if (!result) break;
        imported += result.imageIds.length;
        const placed = result.view.images.find((image) => result.imageIds.includes(image.imageId));
        if (placed) {
          centerX += Math.hypot(
            placed.points[1][0] - placed.points[0][0],
            placed.points[1][1] - placed.points[0][1],
          ) + gap;
        }
      }
      if (imported > 0) setStatus(`已导入 ${imported} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected, settings.arrangementGapPx]);

  const addText = useCallback(async (
    text: string,
    fontSize: number,
    color: string,
    placement: [number, number],
  ) => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    setTransferProgress({ label: "正在生成文字", current: 0, total: 3 });
    try {
      const bytes = await renderTextImage(text, fontSize, color);
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = placement;
        return pinBoardApi.importPinBoardClipboardImage(
          selected,
          bytes,
          centerX,
          centerY,
          revision,
        );
      }, () => "已添加文字");
    } catch (error) {
      setStatus(`无法添加文字：${errorMessage(error)}`);
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected]);

  const deleteImages = useCallback(() => {
    rendererRef.current?.deleteSelected();
  }, []);

  const exportImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    const imageIds = current.selectedImageIds();
    if (imageIds.length === 0) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    try {
      const directory = await pickPinBoardExportDirectory();
      if (!directory || !await current.save()) return;
      setTransferProgress({ label: "正在导出图片", current: 0, total: imageIds.length });
      const count = await pinBoardApi.exportPinBoardImages(
        selected,
        imageIds,
        directory,
      );
      setStatus(`已导出 ${count} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [selected]);

  return {
    boards,
    trash,
    view,
    selected,
    status,
    transferProgress,
    clipboardCount,
    interaction,
    renderer,
    setRenderer: applyRenderer,
    setStatus,
    setInteraction,
    syncInteraction,
    selectBoard,
    moveBoard,
    createBoard,
    renameBoard,
    trashBoard,
    restoreBoard,
    permanentlyDeleteBoard,
    emptyTrash,
    save,
    toggleLock,
    copyImages,
    copyImageToClipboard,
    pasteImages,
    importImages,
    importDroppedFiles,
    addText,
    deleteImages,
    exportImages,
  };
}
