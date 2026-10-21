import { useCallback, useEffect, useRef, useState } from "react";
import { diagnosticsLog } from "../../shared/diagnostics";
import { errorMessage } from "../../shared/errors";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { pinBoardApi } from "./api";
import { reorderBoardIds, type BoardDropPosition } from "./boardOrder";
import { canvasExportMaxDimension, type CanvasExportPlan } from "./exportPolicy";
import { registerPinBoardLifecycleParticipant } from "./lifecycle";
import {
  DEFAULT_PIN_BOARD_INTERACTION,
  type PinBoardRenderer,
  type PinBoardInteractionState,
} from "./renderer";
import { getBoardSession, getSelectedBoardId, setSelectedBoardId } from "./session";
import type { PinBoardTool, StrokeCommitRequest } from "./strokeDrawing";
import { textureBudgetsForLevel } from "./texturePolicy";
import { usePinBoardTransfers } from "./usePinBoardTransfers";
import type {
  PinBoardClipboardItem,
  PinBoardModuleSettings,
  PinBoardSummary,
  PinBoardTransferProgress,
  PinBoardView,
} from "./types";

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
  pasteItems: () => void;
  importImages: () => Promise<void>;
  importDroppedFiles: (files: FileList, clientX: number, clientY: number) => Promise<void>;
  addText: (text: string, fontSize: number, color: string, placement: [number, number]) => Promise<void>;
  setTool: (tool: PinBoardTool) => void;
  /** 提交一笔刚画完的笔迹：先落地未保存的改动，再创建条目。 */
  commitStroke: (request: StrokeCommitRequest) => void;
  deleteImages: () => void;
  exportImages: () => Promise<void>;
  /** 当前设置下的建议上限（整画布导出）。 */
  canvasExportLimit: number;
  /** 按倍率与边距算出导出计划：对话框的「输出尺寸」与真正的导出共用它。 */
  canvasExportPlan: (scale: number, margin: number) => CanvasExportPlan | null;
  /** 对话框的默认倍率：1x 与「最长边塞进建议上限」取小。 */
  canvasExportAutoScale: (margin: number) => number;
  /** 整画布导出：把可见内容导成一张透明 PNG。 */
  exportCanvas: (request: {
    directory: string;
    fileName: string;
    scale: number;
    margin: number;
  }) => Promise<void>;
  /** 取消当前传输（导入/导出图片、整画布导出）：渲染器与原生循环在下一块/下一条目生效。 */
  cancelTransfer: () => void;
}

/**
 * 素材板控制器：画板列表、选择、保存与生命周期结算。
 * `PinBoardRenderer` 仍是纯渲染器，只负责 GPU 与交互；传输类操作（导入/导出/文字/
 * 笔迹）收在 `usePinBoardTransfers`，组件只保留视图与弹窗。
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
  const [clipboardCount, setClipboardCount] = useState(0);
  const [interaction, setInteraction] = useState(DEFAULT_PIN_BOARD_INTERACTION);
  const [renderer, setRenderer] = useState<PinBoardRenderer | null>(null);
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const clipboardRef = useRef<PinBoardClipboardItem[]>([]);
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
  /** 当前选中的画板：排队中的提交按入队时的画板写，切走之后不再动旧画板。 */
  const selectedBoardRef = useRef<number | null>(selected);
  selectedBoardRef.current = selected;

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
    rendererRef.current?.setArrangementGapRatio(settings.arrangementGapRatio);
  }, [settings.arrangementGapRatio]);

  useEffect(() => {
    rendererRef.current?.setAutosaveEnabled(settings.autosave);
  }, [settings.autosave]);

  useEffect(() => {
    rendererRef.current?.setTextureCacheBudgets(textureBudgetsForLevel(settings.textureCacheLevel));
  }, [settings.textureCacheLevel]);

  /** 整画布导出的建议上限：取素材板设置的纹理缓存等级。 */
  const canvasExportLimit = canvasExportMaxDimension(settings.textureCacheLevel);

  const transfers = usePinBoardTransfers({
    active,
    rendererRef,
    selected,
    selectedBoardRef,
    arrangementGapRatio: settings.arrangementGapRatio,
    canvasExportLimit,
    beginLoad,
    isLoadCurrent,
    setStatus,
    setInteraction,
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
    setInteraction(DEFAULT_PIN_BOARD_INTERACTION);
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
    setInteraction(rendererRef.current?.interactionState ?? DEFAULT_PIN_BOARD_INTERACTION);
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
        // 装载是同步的：旧画板的未保存改动交给后台结算队列，切换本身不等仓库写入。
        current.loadBoard(loaded, getBoardSession(artworkId, loaded.boardId));
        if (!isLoadCurrent(sequence)) return;
      }
      setSelected(board.boardId);
      setSelectedBoardId(artworkId, board.boardId);
      setView(loaded);
      setInteraction(rendererRef.current?.interactionState ?? DEFAULT_PIN_BOARD_INTERACTION);
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
    if (!boards || reorderBusyRef.current || transfers.isTransferBusy()) return;
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
  }, [artworkId, boards, transfers.isTransferBusy]);

  const createBoard = useCallback(async (name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      const next = await pinBoardApi.createPinBoard(artworkId, trimmed);
      // 切换作品后旧作品的列表不再回填（画板列表的写操作没有读取代次）。
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
        setInteraction(DEFAULT_PIN_BOARD_INTERACTION);
        const fallback = next[0];
        if (fallback) {
          const loaded = await pinBoardApi.loadPinBoard(fallback.boardId);
          // 旧画板已被移入回收站：不再为它写库（快照也不暂存）。
          current?.loadBoard(loaded, getBoardSession(artworkId, loaded.boardId), false);
          setSelected(fallback.boardId);
          setSelectedBoardId(artworkId, fallback.boardId);
          setView(loaded);
        } else {
          current?.unloadBoard(false);
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

  /**
   * 画板内复制。位图、文字与笔迹都能复制（载荷只记来源），因此「选中了什么就复制
   * 什么」；没有可复制内容时直接返回，不写状态栏——灰掉的按钮点了没反应是预期。
   */
  const copyImages = useCallback(() => {
    const current = rendererRef.current;
    const items = current?.copySelection() ?? [];
    if (items.length === 0) return;
    clipboardRef.current = items;
    setClipboardCount(items.length);
    setStatus(`已复制 ${items.length} 个条目`);
  }, []);

  /**
   * 复制单张位图到系统剪贴板。只有位图有这条路：文字与笔迹没有可贴到外部程序的
   * 载荷，入口在界面上已经置灰，这里静默返回而不是弹提示。
   */
  const copyImageToClipboard = useCallback(async () => {
    const current = rendererRef.current;
    const imageId = current?.copyImageToClipboardId() ?? null;
    if (!current || selected === null || imageId === null) return;
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

  /**
   * 画板内粘贴：三类条目走同一条命令、一次事务，混合选择不会只粘进来一半。
   * 落位由原生端按「整组保持相对位置」处理，所以这里不传阵列间距。
   */
  const pasteItems = useCallback(() => {
    if (clipboardRef.current.length === 0 || selected === null) return;
    void transfers.replaceAfterImageMutation((current, revision) => {
      const [centerX, centerY] = current.placementPoint;
      return pinBoardApi.pastePinBoardItems(
        selected,
        clipboardRef.current,
        centerX,
        centerY,
        revision,
      );
    }, (count) => `已粘贴 ${count} 个条目`);
  }, [selected, transfers.replaceAfterImageMutation]);

  const setTool = useCallback((tool: PinBoardTool) => {
    const current = rendererRef.current;
    if (!current) return;
    current.setTool(tool);
    setInteraction(current.interactionState);
  }, []);

  const deleteImages = useCallback(() => {
    rendererRef.current?.deleteSelected();
  }, []);

  return {
    boards,
    trash,
    view,
    selected,
    status,
    transferProgress: transfers.transferProgress,
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
    pasteItems,
    importImages: transfers.importImages,
    importDroppedFiles: transfers.importDroppedFiles,
    addText: transfers.addText,
    setTool,
    commitStroke: transfers.commitStroke,
    deleteImages,
    exportImages: transfers.exportImages,
    canvasExportLimit,
    canvasExportPlan: transfers.canvasExportPlan,
    canvasExportAutoScale: transfers.canvasExportAutoScale,
    exportCanvas: transfers.exportCanvas,
    cancelTransfer: transfers.cancelTransfer,
  };
}
