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
import { canvasExportMaxDimension, type CanvasExportPlan } from "./exportPolicy";
import { normalizeLayerVisibility } from "./layers";
import { registerPinBoardLifecycleParticipant } from "./lifecycle";
import type { PinBoardRenderer, PinBoardInteractionState } from "./renderer";
import { getBoardSession, getSelectedBoardId, setSelectedBoardId } from "./session";
import { strokeToolId, type PinBoardTool, type StrokeCommitRequest } from "./strokeDrawing";
import { textureBudgetsForLevel } from "./texturePolicy";
import { measurePinBoardText } from "./textLayout";
import type {
  PinBoardClipboardItem,
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
  itemCount: 0,
  selectedCount: 0,
  arrangeableCount: 0,
  exportableCount: 0,
  layerCounts: [0, 0, 0, 0],
  canUndo: false,
  canRedo: false,
  dirty: false,
  saving: false,
  locked: true,
  zoomPercent: 100,
  tool: "select",
  visibleLayers: normalizeLayerVisibility(null),
  exporting: false,
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
  const clipboardRef = useRef<PinBoardClipboardItem[]>([]);
  const transferBusyRef = useRef(false);
  /**
   * 传输的取消标记：导入/导出图片由原生循环检查，整画布导出由渲染器逐块检查。
   * 一次只跑一个传输，因此一个布尔就够，每次传输开始时复位。
   */
  const transferCancelRef = useRef(false);
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
  /** 笔迹创建的串行队列：两笔并发落库会撞上同一个 revision。 */
  const strokeQueueRef = useRef<Promise<void>>(Promise.resolve());
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

  /**
   * 离开工作区（切标签页、切作品）视为「用户离开该画板」：中止在途的传输，
   * 由渲染器在下一块之前停下并丢弃临时文件。
   */
  useEffect(() => {
    if (!active) transferCancelRef.current = true;
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
    setTransferProgress((current) => ({
      label: event.phase ?? "正在处理图片",
      current: event.current ?? 0,
      total: event.total ?? 0,
      // 可取消性由发起方决定，原生进度事件只更新数字。
      cancellable: current?.cancellable ?? false,
    }));
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
        // 装载是同步的：旧画板的未保存改动交给后台结算队列，切换本身不等仓库写入。
        current.loadBoard(loaded, getBoardSession(artworkId, loaded.boardId));
        if (!isLoadCurrent(sequence)) return;
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
      current.applyMutationResult(result.view, result.itemIds);
      setInteraction(current.interactionState);
      setStatus(success(result.itemIds.length));
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
    void replaceAfterImageMutation((current, revision) => {
      const [centerX, centerY] = current.placementPoint;
      return pinBoardApi.pastePinBoardItems(
        selected,
        clipboardRef.current,
        centerX,
        centerY,
        revision,
      );
    }, (count) => `已粘贴 ${count} 个条目`);
  }, [replaceAfterImageMutation, selected]);

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
      transferCancelRef.current = false;
      setTransferProgress({
        label: "正在导入图片",
        current: 0,
        total: paths.length,
        cancellable: true,
      });
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = renderer.placementPoint;
        return pinBoardApi.importPinBoardImages(
          selected,
          paths,
          centerX,
          centerY,
          settings.arrangementGapRatio,
          revision,
        );
      }, (count) => `已导入 ${count} 张图片`);
      // 取消时原生侧整笔回滚（不会留下半批条目），这里只把状态文案说清楚。
      if (transferCancelRef.current) setStatus("已取消导入");
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected, settings.arrangementGapRatio]);

  const importDroppedFiles = useCallback(async (
    files: FileList,
    clientX: number,
    clientY: number,
  ) => {
    const current = rendererRef.current;
    if (!current || selected === null || transferBusyRef.current) return;
    if (current.interactionState.locked || current.interactionState.exporting) {
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
    transferCancelRef.current = false;
    current.setPlacementFromClient(clientX, clientY);
    let [centerX, centerY] = current.placementPoint;
    try {
      let imported = 0;
      for (const file of images) {
        // 拖入的图片是前端逐张落库的，取消就是不再发起下一张（已落库的保留）。
        if (transferCancelRef.current) break;
        const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
        if (bytes.length === 0) continue;
        setTransferProgress({
          label: "正在导入图片",
          current: imported,
          total: images.length,
          cancellable: true,
        });
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
        imported += result.itemIds.length;
        const placed = result.view.items.find((image) => result.itemIds.includes(image.itemId));
        if (placed) {
          // 逐张落位时按「图高的比例」递进，与阵列间距同一口径。
          const placedWidth = Math.hypot(
            placed.points[1][0] - placed.points[0][0],
            placed.points[1][1] - placed.points[0][1],
          );
          const placedHeight = Math.hypot(
            placed.points[0][0] - placed.points[3][0],
            placed.points[0][1] - placed.points[3][1],
          );
          centerX += placedWidth + placedHeight * settings.arrangementGapRatio;
        }
      }
      if (transferCancelRef.current) setStatus(`已取消导入，本次导入 ${imported} 张图片`);
      else if (imported > 0) setStatus(`已导入 ${imported} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected, settings.arrangementGapRatio]);

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
    setTransferProgress({ label: "正在添加文字", current: 0, total: 1 });
    try {
      // 度量与渲染共用 `textLayout`；Rust 只持久化度量结果与载荷，不生成图片。
      // 对话框不提供对齐选项，条目统一按居中排版（Rust 侧已校验取值）。
      const metrics = measurePinBoardText(text, fontSize);
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = placement;
        return pinBoardApi.createPinBoardText(
          selected,
          text,
          fontSize,
          color,
          "center",
          metrics.width,
          metrics.height,
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

  const setTool = useCallback((tool: PinBoardTool) => {
    const current = rendererRef.current;
    if (!current) return;
    current.setTool(tool);
    setInteraction(current.interactionState);
  }, []);

  /**
   * 提交一笔刚画完的笔迹。
   *
   * 与文字条目同一条写入链路，但不复用 `replaceAfterImageMutation`：它会在成功后
   * 写一条状态栏文案，而画板状态栏是 34px 的独立一行——每画一笔弹一次会让画布
   * 上下跳动。失败仍然走状态栏。
   *
   * 串行化是必需的：`create_pin_board_stroke` 带 revision 校验，两笔并发落库会
   * 拿着同一个 revision，后一笔被判成「画板已被其他窗口修改」。
   */
  const commitStroke = useCallback((request: StrokeCommitRequest) => {
    const boardId = selectedBoardRef.current;
    if (boardId === null) return;
    strokeQueueRef.current = strokeQueueRef.current.then(async () => {
      const current = rendererRef.current;
      if (!current || current.currentBoardId !== boardId || current.interactionState.locked) {
        current?.clearPendingStrokePreview();
        return;
      }
      // 先落地未保存的变换：创建命令会推进 revision，若把内存里未写库的改动一起
      // 记成「已保存」，切换画板时它们就会被丢掉。
      if (!await current.save()) {
        current.clearPendingStrokePreview();
        return;
      }
      if (current.currentBoardId !== boardId) return;
      try {
        const result = await pinBoardApi.createPinBoardStroke(
          boardId,
          strokeToolId(request.tool),
          request.color,
          request.strokeWidth,
          request.width,
          request.height,
          request.geometry,
          request.centerX,
          request.centerY,
          current.boardRevision,
        );
        if (current.currentBoardId !== boardId) return;
        current.applyMutationResult(result.view, result.itemIds);
        setInteraction(current.interactionState);
      } catch (error) {
        if (current.currentBoardId === boardId) {
          // 提交失败就把预览撤掉，别在画布上留一条不存在的笔迹。
          current.clearPendingStrokePreview();
          setStatus(errorMessage(error));
        }
      }
    }).catch((error) => setStatus(errorMessage(error)));
  }, []);

  const deleteImages = useCallback(() => {
    rendererRef.current?.deleteSelected();
  }, []);

  const exportImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    // 只导出位图：文字与笔迹的单条导出已删除，统一由整画布导出覆盖。
    const itemIds = current.exportableImageIds();
    if (itemIds.length === 0) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    try {
      const directory = await pickPinBoardExportDirectory();
      if (!directory || !await current.save()) return;
      transferCancelRef.current = false;
      setTransferProgress({
        label: "正在导出图片",
        current: 0,
        total: itemIds.length,
        cancellable: true,
      });
      const count = await pinBoardApi.exportPinBoardImages(
        selected,
        itemIds,
        directory,
      );
      setStatus(`已导出 ${count} 个条目`);
    } catch (error) {
      setStatus(transferCancelRef.current ? "已取消导出" : errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [selected]);

  /** 整画布导出的建议上限：取素材板设置的纹理缓存等级。 */
  const canvasExportLimit = canvasExportMaxDimension(settings.textureCacheLevel);

  const canvasExportPlan = useCallback((scale: number, margin: number) => (
    rendererRef.current?.canvasExportPlan(scale, margin, canvasExportLimit) ?? null
  ), [canvasExportLimit]);

  const canvasExportAutoScale = useCallback((margin: number) => (
    rendererRef.current?.canvasExportAutoScale(margin, canvasExportLimit) ?? 1
  ), [canvasExportLimit]);

  /**
   * 取消当前传输。
   *
   * 两条链路的取消方式不同：整画布导出由渲染器逐块检查标记（下一块之前停下并丢弃
   * 临时文件），导入/导出图片则由原生循环在条目边界检查；这里同时置位，多调一次
   * 原生取消是无害的（没有传输时它只留下一个会在下次开始时复位的标记）。
   */
  const cancelTransfer = useCallback(() => {
    transferCancelRef.current = true;
    void pinBoardApi.cancelPinBoardTransfer().catch(() => undefined);
  }, []);

  /**
   * 整画布导出：可见内容离屏渲染成一块块 RGBA，由原生端流式拼成一张透明 PNG。
   *
   * 导出的是渲染器内存里的当前状态，**不落库**，因此整段流程不产生仓库写操作、
   * 也不取仓库写锁。期间渲染器置 `exporting`：画板锁定、指针与快捷键不响应。
   */
  const exportCanvas = useCallback(async (request: {
    directory: string;
    fileName: string;
    scale: number;
    margin: number;
  }) => {
    const current = rendererRef.current;
    if (!current || transferBusyRef.current || current.interactionState.locked) return;
    const plan = current.canvasExportPlan(request.scale, request.margin, canvasExportLimit);
    if (!plan) {
      setStatus("画板没有可导出的内容");
      return;
    }
    // 文件名由对话框给出，原生端会清掉非法字符并在重名时追加序号。
    const fileName = request.fileName.trim();
    transferBusyRef.current = true;
    transferCancelRef.current = false;
    setTransferProgress({
      label: "正在导出画板",
      current: 0,
      total: plan.totalTiles,
      cancellable: true,
    });
    try {
      const path = await current.exportCanvas(plan, {
        directory: request.directory,
        fileName,
        onProgress: (sent, total) => setTransferProgress({
          label: "正在导出画板",
          current: sent,
          total,
          cancellable: true,
        }),
        isCancelled: () => transferCancelRef.current,
      });
      setStatus(`已导出 ${plan.width}×${plan.height} 的 PNG：${path}`);
    } catch (error) {
      setStatus(transferCancelRef.current ? "已取消导出" : `导出失败：${errorMessage(error)}`);
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
      transferCancelRef.current = false;
      setInteraction(rendererRef.current?.interactionState ?? DEFAULT_INTERACTION);
    }
  }, [canvasExportLimit]);

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
    pasteItems,
    importImages,
    importDroppedFiles,
    addText,
    setTool,
    commitStroke,
    deleteImages,
    exportImages,
    canvasExportLimit,
    canvasExportPlan,
    canvasExportAutoScale,
    exportCanvas,
    cancelTransfer,
  };
}
