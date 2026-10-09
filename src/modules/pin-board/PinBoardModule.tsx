import {
  AlertTriangle,
  ClipboardCopy,
  ClipboardPaste,
  Copy,
  Download,
  Expand,
  FlipHorizontal2,
  FlipVertical2,
  Fullscreen,
  Images,
  LayoutGrid,
  Layers3,
  LoaderCircle,
  Lock,
  LocateFixed,
  Minimize2,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Plus,
  Redo2,
  RotateCcw,
  RotateCw,
  Save,
  Shrink,
  Trash2,
  Type,
  Unlock,
  Undo2,
  Upload,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { DragEvent as ReactDragEvent } from "react";
import { errorMessage } from "../../shared/errors";
import { ConfirmDialog, type ConfirmView } from "../../shared/ConfirmDialog";
import { diagnosticsLog } from "../../shared/diagnostics";
import { ContextMenu, type ContextMenuItem } from "../../shared/ui/ContextMenu";
import { PromptDialog } from "../../shared/ui/PromptDialog";
import {
  getPinBoardWindowFullscreen,
  pinBoardApi,
  pickPinBoardExportDirectory,
  pickPinBoardImagePaths,
  setPinBoardWindowFullscreen,
} from "./api";
import type {
  PinBoardClipboardImage,
  PinBoardMutationResult,
  PinBoardSummary,
  PinBoardTextureCacheLevel,
  PinBoardTransferProgress,
  PinBoardView,
} from "./types";
import {
  PinBoardRenderer,
  type PinBoardContextMenuRequest,
  type PinBoardInteractionState,
} from "./renderer";
import {
  textureBudgetsForLevel,
  type PinBoardTextureBudgets,
} from "./texturePolicy";
import { registerPinBoardLifecycleParticipant } from "./lifecycle";
import {
  getBoardSession,
  getSelectedBoardId,
  getSidebarVisible,
  setBoardSession,
  setSelectedBoardId,
  setSidebarVisible,
} from "./session";
import { shortcutCanHandle, shortcutMatches } from "./shortcuts";

/**
 * 素材板快捷键默认值，与设置页默认项保持一致。
 * 锁定沿用 Client 的默认键位 Ctrl+R；WebView 整页刷新由应用层的
 * `preventWebViewReload` 只取消默认行为，因此不会与本快捷键冲突。
 */
export const PIN_BOARD_LOCK_SHORTCUT = "CommandOrControl+R";
export const PIN_BOARD_FULLSCREEN_SHORTCUT = "F11";

export interface PinBoardModuleSettings {
  arrangementGapPx: number;
  textureCacheLevel: PinBoardTextureCacheLevel;
  autosave: boolean;
  lockShortcut: string;
  fullscreenShortcut: string;
}

interface PinBoardModuleProps {
  artworkId: string;
  /** 工作区 keep-mounted 语义：非活跃时暂停全局键盘交互与在途纹理任务。 */
  active: boolean;
  settings: PinBoardModuleSettings;
}

function actionTitle(action: string, shortcut: string): string {
  const hint = shortcut.replace("CommandOrControl", "Ctrl").replaceAll("+", " + ");
  return hint ? `${action} (${hint})` : action;
}

const IMAGE_PATH_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif|tga|dds)$/i;

/**
 * 拖入导入走「原始字节解码」路径（`import_pin_board_clipboard_image`），只能接受
 * 可被内容嗅探识别的位图格式；DDS/TGA 需要文件路径，仍通过「导入图片」选择器处理。
 */
const DROP_IMAGE_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif)$/i;

/** 侧栏画板拖放排序使用的自定义 dataTransfer 类型。 */
const PIN_BOARD_DRAG_TYPE = "application/x-lilith-pin-board-board";

type BoardDropPosition = "before" | "after";

/**
 * 计算拖放后的画板 id 顺序：把 `draggedId` 插到 `targetId` 的前或后。
 * 返回 `null` 表示顺序没有变化（同位置、id 不在列表中或拖到自己身上）。
 */
export function reorderBoardIds(
  boardIds: number[],
  draggedId: number,
  targetId: number,
  position: BoardDropPosition,
): number[] | null {
  if (draggedId === targetId || !boardIds.includes(draggedId)) return null;
  const withoutDragged = boardIds.filter((boardId) => boardId !== draggedId);
  const targetIndex = withoutDragged.indexOf(targetId);
  if (targetIndex < 0) return null;
  const insertAt = position === "before" ? targetIndex : targetIndex + 1;
  const next = [
    ...withoutDragged.slice(0, insertAt),
    draggedId,
    ...withoutDragged.slice(insertAt),
  ];
  return next.every((boardId, index) => boardId === boardIds[index]) ? null : next;
}

function clipboardImagePaths(value: string): string[] {
  return value
    .split(/[\r\n\0]+/)
    .map((part) => part.trim().replace(/^["']|["']$/g, ""))
    .map((part) => {
      if (!part.toLowerCase().startsWith("file:")) return part;
      try {
        const pathname = decodeURIComponent(new URL(part).pathname);
        return /^\/[a-z]:/i.test(pathname) ? pathname.slice(1) : pathname;
      } catch {
        return part;
      }
    })
    .filter((path) => IMAGE_PATH_PATTERN.test(path));
}

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

async function clipboardPaths(): Promise<string[]> {
  if (!navigator.clipboard?.readText) return [];
  try {
    return clipboardImagePaths(await navigator.clipboard.readText());
  } catch {
    return [];
  }
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

function afterLayoutSettles(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

function GpuCanvas({
  view,
  artworkId,
  arrangementGapCssPixels,
  autosaveEnabled,
  cacheBudgets,
  active,
  setStatus,
  onRenderer,
  onState,
  onContextMenu,
}: {
  view: PinBoardView | null;
  artworkId: string;
  arrangementGapCssPixels: number;
  autosaveEnabled: boolean;
  cacheBudgets: PinBoardTextureBudgets;
  active: boolean;
  setStatus: (message: string | null) => void;
  onRenderer: (renderer: PinBoardRenderer | null) => void;
  onState: (state: PinBoardInteractionState) => void;
  onContextMenu: (request: PinBoardContextMenuRequest | null) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const selectionRef = useRef<HTMLDivElement>(null);
  const marqueeRef = useRef<HTMLDivElement>(null);
  /** 渲染器实例常驻：创建 effect 只随画布生命周期运行一次。 */
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const bootRef = useRef<Promise<PinBoardRenderer | null> | null>(null);
  const unregisterLifecycleRef = useRef<(() => void) | null>(null);
  const destroyTimerRef = useRef<number | null>(null);
  const disposedRef = useRef(false);
  /** 渲染器就绪后自增，触发「装载画板」effect 用最新 view 装载。 */
  const [readyVersion, setReadyVersion] = useState(0);

  const activeRef = useRef(active);
  activeRef.current = active;
  const artworkIdRef = useRef(artworkId);
  artworkIdRef.current = artworkId;
  const gapRef = useRef(arrangementGapCssPixels);
  gapRef.current = arrangementGapCssPixels;
  const autosaveRef = useRef(autosaveEnabled);
  autosaveRef.current = autosaveEnabled;
  const cacheBudgetsRef = useRef(cacheBudgets);
  cacheBudgetsRef.current = cacheBudgets;
  const setStatusRef = useRef(setStatus);
  setStatusRef.current = setStatus;
  const onRendererRef = useRef(onRenderer);
  onRendererRef.current = onRenderer;
  const onStateRef = useRef(onState);
  onStateRef.current = onState;
  const onContextMenuRef = useRef(onContextMenu);
  onContextMenuRef.current = onContextMenu;

  // 创建 effect 幂等化：StrictMode 的「挂载 → 清理 → 再挂载」复用同一次创建，
  // 因此每次挂载只创建一个 GPU 设备。真正的销毁推迟到模块卸载时执行。
  useEffect(() => {
    disposedRef.current = false;
    if (destroyTimerRef.current !== null) {
      window.clearTimeout(destroyTimerRef.current);
      destroyTimerRef.current = null;
    }
    const canvas = canvasRef.current;
    const selection = selectionRef.current;
    const marquee = marqueeRef.current;
    if (!canvas || !selection || !marquee) return undefined;

    if (!bootRef.current) {
      bootRef.current = PinBoardRenderer.create(
        canvas,
        selection,
        marquee,
        gapRef.current,
        autosaveRef.current,
        (message) => setStatusRef.current(message),
        (state) => onStateRef.current(state),
        (boardId, session) => {
          // 画布没有布局尺寸时（面板从未获得空间）不写入会话，避免下次进入
          // 恢复出退化视口导致图片过小且跳过包围框适配。
          if (canvas.getBoundingClientRect().height > 0) {
            setBoardSession(artworkIdRef.current, boardId, session);
          }
        },
        (request) => onContextMenuRef.current(request),
        cacheBudgetsRef.current,
      )
        .then((created) => {
          if (disposedRef.current) {
            created.destroy(false);
            return null;
          }
          created.setArrangementGapCssPixels(gapRef.current);
          created.setAutosaveEnabled(autosaveRef.current);
          created.setTextureCacheBudgets(cacheBudgetsRef.current);
          created.setActive(activeRef.current);
          rendererRef.current = created;
          unregisterLifecycleRef.current = registerPinBoardLifecycleParticipant(created);
          onRendererRef.current(created);
          onStateRef.current(created.interactionState);
          setReadyVersion((current) => current + 1);
          diagnosticsLog("info", `pin-board renderer created: artworkId=${artworkIdRef.current}`);
          return created;
        })
        .catch((error) => {
          diagnosticsLog(
            "warn",
            `pin-board renderer create failed: artworkId=${artworkIdRef.current}, error=${errorMessage(error)}`,
          );
          if (!disposedRef.current) setStatusRef.current(errorMessage(error));
          return null;
        });
    }

    return () => {
      disposedRef.current = true;
      destroyTimerRef.current = window.setTimeout(() => {
        destroyTimerRef.current = null;
        bootRef.current = null;
        unregisterLifecycleRef.current?.();
        unregisterLifecycleRef.current = null;
        const current = rendererRef.current;
        rendererRef.current = null;
        onRendererRef.current(null);
        diagnosticsLog("info", `pin-board renderer disposing: artworkId=${artworkIdRef.current}`);
        current?.destroy();
      }, 0);
    };
  }, []);

  // 画板装载：view 变化或渲染器就绪时调用 `loadBoard`（按 boardId 幂等）。
  useEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    if (view) {
      void renderer
        .loadBoard(view, getBoardSession(artworkId, view.boardId))
        .then((loaded) => {
          if (!loaded && !disposedRef.current) {
            setStatusRef.current("切换素材板失败：旧画板未能完成结算");
          }
        });
    } else {
      void renderer.unloadBoard();
    }
  }, [view, artworkId, readyVersion]);

  return (
    <div className="pin-board-canvas-shell">
      <canvas ref={canvasRef} className="pin-board-canvas" tabIndex={0} />
      <div ref={marqueeRef} className="pin-board-marquee" hidden />
      <div ref={selectionRef} className="pin-board-selection" hidden>
        <span className="pin-board-rotation-stem" />
        <span className="pin-board-rotation-handle" />
        <span className="pin-board-resize-handle top-left" />
        <span className="pin-board-resize-handle top-right" />
        <span className="pin-board-resize-handle bottom-right" />
        <span className="pin-board-resize-handle bottom-left" />
      </div>
    </div>
  );
}

function PinBoardTextDialog({
  initialFontSize,
  initialColor,
  onSubmit,
  onClose,
}: {
  initialFontSize: number;
  initialColor: string;
  onSubmit: (text: string, fontSize: number, color: string) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [fontSize, setFontSize] = useState(initialFontSize);
  const [color, setColor] = useState(initialColor);

  return (
    <div
      className="modal-backdrop pin-board-text-backdrop"
      role="presentation"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <form
        className="modal pin-board-text-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pin-board-text-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
        onSubmit={(event) => {
          event.preventDefault();
          if (text.trim()) onSubmit(text, fontSize, color);
        }}
      >
        <header className="modal-header">
          <div>
            <h2 id="pin-board-text-title">添加文字</h2>
          </div>
          <button className="icon-button" type="button" onClick={onClose} title="关闭" aria-label="关闭">
            <X size={17} />
          </button>
        </header>
        <div className="modal-content pin-board-text-fields">
          <label>
            <span className="field-label">内容</span>
            <textarea
              className="dialog-input pin-board-text-input"
              autoFocus
              maxLength={5000}
              rows={5}
              value={text}
              onChange={(event) => setText(event.target.value)}
            />
          </label>
          <div className="pin-board-text-options">
            <label>
              <span className="field-label">字号</span>
              <input
                className="dialog-input"
                type="number"
                min={12}
                max={240}
                step={1}
                value={fontSize}
                onChange={(event) => setFontSize(Math.min(240, Math.max(12, Number(event.target.value) || 12)))}
              />
            </label>
            <label>
              <span className="field-label">颜色</span>
              <span className="pin-board-text-color">
                <input
                  type="color"
                  value={color}
                  onChange={(event) => setColor(event.target.value)}
                  aria-label="文字颜色"
                />
                <span>{color.toUpperCase()}</span>
              </span>
            </label>
          </div>
        </div>
        <footer className="modal-footer">
          <button className="secondary-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!text.trim()}>添加</button>
        </footer>
      </form>
    </div>
  );
}

function PinBoardContextMenu({
  request,
  renderer,
  fullscreen,
  canPaste,
  onCopy,
  onCopyClipboard,
  onPaste,
  onImport,
  onAddText,
  onExport,
  onDelete,
  onToggleFullscreen,
  onClose,
}: {
  request: PinBoardContextMenuRequest;
  renderer: PinBoardRenderer;
  fullscreen: boolean;
  canPaste: boolean;
  onCopy: () => void;
  onCopyClipboard: () => void;
  onPaste: () => void;
  onImport: () => void;
  onAddText: () => void;
  onExport: () => void;
  onDelete: () => void;
  onToggleFullscreen: () => void;
  onClose: () => void;
}) {
  const items: ContextMenuItem[] = request.targetImage
    ? [
        { key: "copy-board", label: "复制画板图片", icon: <Copy size={16} />, onClick: onCopy },
        { key: "copy-clipboard", label: "复制图片到剪贴板", icon: <ClipboardCopy size={16} />, disabled: request.selectedCount !== 1, onClick: onCopyClipboard },
        { key: "export", label: "导出图片", icon: <Download size={16} />, onClick: onExport },
        { key: "sep-flip", divider: true },
        { key: "flip-h", label: "水平翻转", icon: <FlipHorizontal2 size={16} />, onClick: () => renderer.flipSelected(true) },
        { key: "flip-v", label: "垂直翻转", icon: <FlipVertical2 size={16} />, onClick: () => renderer.flipSelected(false) },
        { key: "arrange", label: "阵列排序", icon: <LayoutGrid size={16} />, disabled: request.selectedCount < 2, onClick: () => renderer.arrangeSelected() },
        { key: "sep-layer", divider: true },
        { key: "layer-label", heading: true, label: "图层", icon: <Layers3 size={15} /> },
        { key: "layer-top", label: "顶层", icon: <span className="pin-board-layer-swatch top" />, onClick: () => renderer.setSelectedLayer(2) },
        { key: "layer-middle", label: "中层", icon: <span className="pin-board-layer-swatch middle" />, onClick: () => renderer.setSelectedLayer(1) },
        { key: "layer-bottom", label: "底层", icon: <span className="pin-board-layer-swatch bottom" />, onClick: () => renderer.setSelectedLayer(0) },
        { key: "sep-delete", divider: true },
        { key: "delete", label: "删除图片", icon: <Trash2 size={16} />, danger: true, onClick: onDelete },
      ]
    : [
        { key: "paste", label: "粘贴画板图片", icon: <ClipboardPaste size={16} />, disabled: !request.canEditBoard || !canPaste, onClick: onPaste },
        { key: "import", label: "导入图片", icon: <Upload size={16} />, disabled: !request.canEditBoard, onClick: onImport },
        { key: "text", label: "添加文字", icon: <Type size={16} />, disabled: !request.canEditBoard, onClick: onAddText },
        { key: "sep-view", divider: true },
        { key: "reset-view", label: "重置视图", icon: <LocateFixed size={16} />, onClick: () => renderer.resetViewport() },
        {
          key: "fullscreen",
          label: fullscreen ? "退出全屏" : "全屏显示",
          icon: fullscreen ? <Minimize2 size={16} /> : <Fullscreen size={16} />,
          onClick: onToggleFullscreen,
        },
      ];

  return (
    <ContextMenu
      x={request.clientX}
      y={request.clientY}
      maxHeight={480}
      items={items}
      onClose={onClose}
    />
  );
}

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

export function PinBoardModule({ artworkId, active, settings }: PinBoardModuleProps) {
  const [boards, setBoards] = useState<PinBoardSummary[] | null>(null);
  const [trash, setTrash] = useState<PinBoardSummary[]>([]);
  const [view, setView] = useState<PinBoardView | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  /** 拖入文件悬停在画布上时显示投放提示。 */
  const [dropActive, setDropActive] = useState(false);
  const [sidebar, setSidebar] = useState(getSidebarVisible);
  const [fullscreen, setFullscreen] = useState(false);
  const [textDialogOpen, setTextDialogOpen] = useState(false);
  const [textOptions, setTextOptions] = useState({ fontSize: 48, color: "#000000" });
  /** 统一的命名输入对话框状态，替代原生 `window.prompt`。 */
  const [labelPrompt, setLabelPrompt] = useState<
    { kind: "create" } | { kind: "rename"; board: PinBoardSummary } | null
  >(null);
  /** 统一的确认对话框状态，替代原生 `window.confirm`。 */
  const [confirmRequest, setConfirmRequest] = useState<
    | { kind: "trash"; board: PinBoardSummary }
    | { kind: "permanent-delete"; board: PinBoardSummary }
    | { kind: "empty-trash" }
    | null
  >(null);
  const [contextMenu, setContextMenu] = useState<PinBoardContextMenuRequest | null>(null);
  const [listContextMenu, setListContextMenu] = useState<{
    board: PinBoardSummary | null;
    trash: boolean;
    x: number;
    y: number;
  } | null>(null);
  const [transferProgress, setTransferProgress] = useState<PinBoardTransferProgress | null>(null);
  const [clipboardCount, setClipboardCount] = useState(0);
  const [interaction, setInteraction] = useState(DEFAULT_INTERACTION);
  /** 侧栏画板拖放排序：正在拖动的画板与当前落点。 */
  const [draggingBoardId, setDraggingBoardId] = useState<number | null>(null);
  const [boardDropTarget, setBoardDropTarget] = useState<
    { boardId: number; position: BoardDropPosition } | null
  >(null);
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const loadSequence = useRef(0);
  const clipboardRef = useRef<PinBoardClipboardImage[]>([]);
  const textPlacementRef = useRef<[number, number] | null>(null);
  const fullscreenRef = useRef(false);
  const fullscreenBusyRef = useRef(false);
  const transferBusyRef = useRef(false);
  const reorderBusyRef = useRef(false);
  const setRenderer = useCallback((renderer: PinBoardRenderer | null) => {
    rendererRef.current = renderer;
  }, []);
  const handleRendererState = useCallback((state: PinBoardInteractionState) => {
    setInteraction(state);
  }, []);
  const handleRendererContextMenu = useCallback((request: PinBoardContextMenuRequest | null) => {
    setListContextMenu(null);
    setContextMenu(request);
  }, []);

  const refreshBoards = useCallback(async () => {
    const [nextBoards, nextTrash] = await Promise.all([
      pinBoardApi.listPinBoards(artworkId),
      pinBoardApi.listPinBoardTrash(),
    ]);
    setBoards(nextBoards);
    setTrash(nextTrash);
    return nextBoards;
  }, [artworkId]);

  /**
   * 侧栏拖放排序：把 `draggedId` 放到 `targetId` 的前/后，先本地重排再落库，
   * 失败时回滚。重排只改列表顺序，不影响已打开画板的保存。
   */
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
      .then((next) => setBoards(next))
      .catch((error) => {
        setBoards(previous);
        setStatus(errorMessage(error));
      })
      .finally(() => {
        reorderBusyRef.current = false;
      });
  }, [artworkId, boards]);

  useEffect(() => {
    rendererRef.current?.setActive(active);
    if (!active) {
      setContextMenu(null);
      setListContextMenu(null);
      setDropActive(false);
    }
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

  useEffect(() => {
    if (!status) return undefined;
    const timer = window.setTimeout(() => setStatus(null), 4500);
    return () => window.clearTimeout(timer);
  }, [status]);

  useEffect(() => {
    if (!listContextMenu) return undefined;
    const close = () => setListContextMenu(null);
    const keyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", keyDown);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", keyDown);
    };
  }, [listContextMenu]);

  useEffect(() => {
    let cancelled = false;
    const sequence = ++loadSequence.current;
    const current = rendererRef.current;
    // 素材板渲染器跨作品保活：这里只换数据，不销毁渲染器/GPU 设备。
    // 旧画板的结算由随后的 `loadBoard`（或 `unloadBoard`）完成。
    diagnosticsLog(
      "info",
      `pin-board artwork switch start: artworkId=${artworkId}, hadRenderer=${current !== null}`,
    );
    setBoards(null);
    setTrash([]);
    setSelected(null);
    setInteraction(DEFAULT_INTERACTION);
    setContextMenu(null);
    setListContextMenu(null);
    setStatus(null);

    void pinBoardApi.listPinBoards(artworkId)
      .then(async (next) => {
        if (cancelled || sequence !== loadSequence.current) return;
        setBoards(next);
        diagnosticsLog(
          "info",
          `pin-board artwork switch done: artworkId=${artworkId}, boards=${next.length}`,
        );
        setTrash(await pinBoardApi.listPinBoardTrash().catch(() => []));
        const preferredId = getSelectedBoardId(artworkId);
        const first = next.find((board) => board.boardId === preferredId) ?? next[0];
        if (!first) {
          // 新作品没有画板：清空画布（渲染器保持挂载）。
          setView(null);
          return;
        }
        setSelected(first.boardId);
        setSelectedBoardId(artworkId, first.boardId);
        void pinBoardApi.loadPinBoard(first.boardId)
          .then((loaded) => {
            if (!cancelled && sequence === loadSequence.current) setView(loaded);
          })
          .catch((error) => {
            if (!cancelled && sequence === loadSequence.current) setStatus(errorMessage(error));
          });
      })
      .catch((error) => {
        if (!cancelled && sequence === loadSequence.current) setStatus(errorMessage(error));
      });
    return () => {
      cancelled = true;
    };
  }, [artworkId]);

  useEffect(() => () => {
    loadSequence.current += 1;
    rendererRef.current?.destroy();
    rendererRef.current = null;
  }, []);

  const select = async (board: PinBoardSummary) => {
    if (board.boardId === selected) return;
    const sequence = ++loadSequence.current;
    diagnosticsLog(
      "info",
      `pin-board board select start: artworkId=${artworkId}, boardId=${board.boardId}`,
    );
    setContextMenu(null);
    try {
      const loaded = await pinBoardApi.loadPinBoard(board.boardId);
      if (sequence !== loadSequence.current) return;
      const current = rendererRef.current;
      if (current) {
        // 复用同一渲染器与 GPU 设备：先以旧 boardId 结算旧画板再装载新画板；
        // 结算失败则中止切换，渲染器与 UI 都停在旧画板。
        const switched = await current.loadBoard(
          loaded,
          getBoardSession(artworkId, loaded.boardId),
        );
        if (!switched || sequence !== loadSequence.current) return;
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
      if (sequence === loadSequence.current) setStatus(errorMessage(error));
    }
  };

  const replaceAfterImageMutation = useCallback(async (
    action: (renderer: PinBoardRenderer, revision: string) => Promise<PinBoardMutationResult>,
    success: (count: number) => string,
  ): Promise<PinBoardMutationResult | null> => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked) return null;
    const sequence = ++loadSequence.current;
    if (!await current.save() || sequence !== loadSequence.current) return null;
    try {
      const result = await action(current, current.boardRevision);
      if (sequence !== loadSequence.current) return null;
      setContextMenu(null);
      current.applyMutationResult(result.view, result.imageIds);
      setInteraction(current.interactionState);
      setStatus(success(result.imageIds.length));
      return result;
    } catch (error) {
      if (sequence === loadSequence.current) setStatus(errorMessage(error));
      return null;
    }
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
    void replaceAfterImageMutation((renderer, revision) => {
      const [centerX, centerY] = renderer.placementPoint;
      return pinBoardApi.pastePinBoardImages(
        selected ?? 0,
        clipboardRef.current,
        centerX,
        centerY,
        renderer.placementGap(settings.arrangementGapPx),
        revision,
      );
    }, (count) => `已粘贴 ${count} 张图片`);
  }, [replaceAfterImageMutation, selected, settings.arrangementGapPx]);

  const importImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    transferBusyRef.current = true;
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
            setTransferProgress,
          );
        }, (count) => `已从剪贴板导入 ${count} 张图片`);
        return;
      }

      let paths: string[] = [];
      try {
        paths = (await pinBoardApi.readPinBoardClipboardPaths())
          .filter((path) => IMAGE_PATH_PATTERN.test(path));
      } catch {
        // Fall back to text and the file picker when the native clipboard is unavailable.
      }
      if (paths.length === 0) paths = await clipboardPaths();
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
          setTransferProgress,
        );
      }, (count) => `已导入 ${count} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected, settings.arrangementGapPx]);

  /**
   * 拖放导入：把拖进画板的图片文件读成字节后逐个导入。窗口以 `dragDropEnabled: false`
   * 运行（HTML5 拖放可用），浏览器不再暴露文件系统路径，因此只能走字节导入命令；
   * 多张一起拖入时按前一张的显示宽度向右排开，避免完全重叠。
   */
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
            setTransferProgress,
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

  const handleStageDragOver = useCallback((event: ReactDragEvent<HTMLDivElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    // 必须取消默认行为，否则浏览器会把它当成导航并拒绝这次放置。
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    const current = rendererRef.current;
    if (current && !current.interactionState.locked) setDropActive(true);
  }, []);

  const handleStageDragLeave = useCallback((event: ReactDragEvent<HTMLDivElement>) => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    setDropActive(false);
  }, []);

  const handleStageDrop = useCallback((event: ReactDragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDropActive(false);
    if (event.dataTransfer.files.length === 0) return;
    void importDroppedFiles(event.dataTransfer.files, event.clientX, event.clientY);
  }, [importDroppedFiles]);

  const addText = useCallback(async (text: string, fontSize: number, color: string) => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    const placementPoint = textPlacementRef.current ?? current.placementPoint;
    textPlacementRef.current = null;
    setTextOptions({ fontSize, color });
    setTextDialogOpen(false);
    transferBusyRef.current = true;
    setTransferProgress({ label: "正在生成文字", current: 0, total: 3 });
    try {
      const bytes = await renderTextImage(text, fontSize, color);
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = placementPoint;
        return pinBoardApi.importPinBoardClipboardImage(
          selected,
          bytes,
          centerX,
          centerY,
          revision,
          setTransferProgress,
        );
      }, () => "已添加文字");
    } catch (error) {
      setStatus(`无法添加文字：${errorMessage(error)}`);
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [replaceAfterImageMutation, selected]);

  const openTextDialog = useCallback(() => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked) return;
    textPlacementRef.current = current.placementPoint;
    setTextDialogOpen(true);
  }, []);

  const closeTextDialog = useCallback(() => {
    textPlacementRef.current = null;
    setTextDialogOpen(false);
  }, []);

  const deleteImages = useCallback(() => {
    rendererRef.current?.deleteSelected();
  }, []);

  const exportImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    const imageIds = current.selectedImageIds();
    if (imageIds.length === 0) return;
    transferBusyRef.current = true;
    try {
      const directory = await pickPinBoardExportDirectory();
      if (!directory || !await current.save()) return;
      setTransferProgress({ label: "正在导出图片", current: 0, total: imageIds.length });
      const count = await pinBoardApi.exportPinBoardImages(
        selected,
        imageIds,
        directory,
        setTransferProgress,
      );
      setStatus(`已导出 ${count} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [selected]);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if (!shortcutCanHandle(event) || !(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "c") {
        event.preventDefault();
        event.stopImmediatePropagation();
        copyImages();
      } else if (key === "v" && clipboardRef.current.length > 0) {
        event.preventDefault();
        event.stopImmediatePropagation();
        pasteImages();
      }
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, copyImages, pasteImages]);

  const submitLabelPrompt = useCallback((label: string) => {
    const trimmed = label.trim();
    if (!trimmed) return;
    if (labelPrompt?.kind === "rename") {
      void pinBoardApi.renamePinBoard(labelPrompt.board.boardId, trimmed)
        .then(() => refreshBoards())
        .catch((error) => setStatus(errorMessage(error)));
    } else if (labelPrompt?.kind === "create") {
      void pinBoardApi.createPinBoard(artworkId, trimmed)
        .then((next) => {
          setBoards(next);
          if (!selected) {
            const first = next[0];
            if (first) {
              setSelected(first.boardId);
              setSelectedBoardId(artworkId, first.boardId);
              void pinBoardApi.loadPinBoard(first.boardId)
                .then((loaded) => setView(loaded))
                .catch((error) => setStatus(errorMessage(error)));
            }
          }
        })
        .catch((error) => setStatus(errorMessage(error)));
    }
    setLabelPrompt(null);
  }, [artworkId, labelPrompt, refreshBoards, selected]);

  const submitConfirmRequest = useCallback(() => {
    if (!confirmRequest) return;
    const request = confirmRequest;
    setConfirmRequest(null);
    const run = async () => {
      if (request.kind === "trash") {
        await pinBoardApi.trashPinBoard(request.board.boardId);
        const next = await refreshBoards();
        if (selected !== null && request.board.boardId === selected) {
          // 画板已进入回收站：对旧画板跳过 finalize 保存（否则会对已删除画板再次写库）。
          // 渲染器保持挂载，只切换到回退画板或清空。
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
      } else if (request.kind === "permanent-delete") {
        const report = await pinBoardApi.deletePinBoardPermanently(request.board.boardId);
        await refreshBoards();
        if (report.pendingCount > 0) {
          setStatus(`画板已删除，但有 ${report.pendingCount} 个图片文件仍待清理，将在下次启动时重试。`);
        }
      } else {
        const report = await pinBoardApi.emptyPinBoardTrash();
        await refreshBoards();
        if (report.pendingCount > 0) {
          setStatus(`回收站已清空，但有 ${report.pendingCount} 个图片文件仍待清理，将在下次启动时重试。`);
        }
      }
    };
    void run().catch((error) => setStatus(errorMessage(error)));
  }, [artworkId, confirmRequest, refreshBoards, selected]);

  const confirmView: ConfirmView | null = useMemo(() => {
    if (!confirmRequest) return null;
    if (confirmRequest.kind === "trash") {
      return {
        icon: <Trash2 aria-hidden="true" size={18} />,
        eyebrow: "素材板",
        title: "移入回收站",
        subject: `“${confirmRequest.board.name}”`,
        description: "画板将移入素材板回收站，可随时恢复或永久删除。",
        action: "移入回收站",
      };
    }
    if (confirmRequest.kind === "permanent-delete") {
      return {
        icon: <Trash2 aria-hidden="true" size={18} />,
        eyebrow: "素材板回收站",
        title: "永久删除画板",
        subject: `“${confirmRequest.board.name}”`,
        description: "画板及其全部图片文件将被永久删除，此操作无法撤销。",
        detail: null,
        action: "永久删除",
        danger: true,
      };
    }
    return {
      icon: <Trash2 aria-hidden="true" size={18} />,
      eyebrow: "素材板回收站",
      title: "清空回收站",
      subject: `${trash.length} 块画板`,
      description: "回收站中的所有画板及其图片文件将被永久删除，此操作无法撤销。",
      detail: null,
      action: "清空回收站",
      danger: true,
    };
  }, [confirmRequest, trash.length]);

  const title = useMemo(() => {
    if (!boards || selected === null) return "素材板";
    return boards.find((board) => board.boardId === selected)?.name ?? `画板 ${selected}`;
  }, [boards, selected]);

  const renderer = rendererRef.current;
  const run = (action: () => void) => {
    action();
    setInteraction(rendererRef.current?.interactionState ?? DEFAULT_INTERACTION);
  };

  const toggleSidebar = () => {
    setSidebar((current) => {
      setSidebarVisible(!current);
      return !current;
    });
  };

  const toggleLock = useCallback(() => {
    const current = rendererRef.current;
    if (!current) return;
    current.toggleLock();
    setInteraction(current.interactionState);
  }, []);

  const toggleFullscreen = useCallback(async () => {
    if (!workspaceRef.current || fullscreenBusyRef.current) return;
    const previous = fullscreenRef.current;
    const next = !previous;
    const current = rendererRef.current;
    fullscreenBusyRef.current = true;
    fullscreenRef.current = next;
    current?.finishActiveInteraction();
    current?.beginViewportResize();
    setContextMenu(null);
    textPlacementRef.current = null;
    setTextDialogOpen(false);
    try {
      setFullscreen(next);
      await setPinBoardWindowFullscreen(next);
      await afterLayoutSettles();
    } catch (error) {
      fullscreenRef.current = previous;
      setFullscreen(previous);
      setStatus(`无法切换全屏：${errorMessage(error)}`);
    } finally {
      current?.endViewportResize();
      fullscreenBusyRef.current = false;
    }
  }, []);

  useEffect(() => {
    if (!active && fullscreenRef.current) void toggleFullscreen();
  }, [active, toggleFullscreen]);

  useEffect(() => {
    // 原生全屏属于窗口状态而非页面状态：页面重新加载后新页面并不知道窗口仍在
    // 全屏。这里与原生状态对齐，保证工具栏、Escape 和全屏快捷键都能正常退出。
    let cancelled = false;
    void getPinBoardWindowFullscreen()
      .then((isFullscreen) => {
        if (cancelled || !isFullscreen || fullscreenRef.current) return;
        fullscreenRef.current = true;
        setFullscreen(true);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if (!shortcutCanHandle(event)) return;
      const fullscreenRequested = shortcutMatches(event, settings.fullscreenShortcut)
        || (event.key === "Escape" && fullscreenRef.current);
      const lockRequested = shortcutMatches(event, settings.lockShortcut);
      if (!fullscreenRequested && !lockRequested) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (fullscreenRequested) void toggleFullscreen();
      else toggleLock();
    };
    window.addEventListener("keydown", keyDown, true);
    return () => {
      window.removeEventListener("keydown", keyDown, true);
    };
  }, [active, settings.fullscreenShortcut, settings.lockShortcut, toggleFullscreen, toggleLock]);

  useEffect(() => () => {
    if (fullscreenRef.current) void setPinBoardWindowFullscreen(false).catch(() => undefined);
    fullscreenRef.current = false;
  }, []);

  return (
    <section className={`pin-board-module${sidebar ? "" : " sidebar-collapsed"}`}>
      <aside className="pin-board-sidebar">
        <header>
          <div>
            <strong>素材板</strong>
            <small>
              {boards
                ? `${boards.length} 个画板`
                : "正在载入"}
            </small>
          </div>
          <button
            className="icon-button"
            type="button"
            title="新建画板"
            aria-label="新建画板"
            disabled={transferProgress !== null}
            onClick={() => setLabelPrompt({ kind: "create" })}
          >
            <Plus size={16} />
          </button>
        </header>
        <div className="pin-board-tree-scroll">
          {boards
            ? <div className="pin-board-board-list">
              {boards.map((board) => {
                const dragging = draggingBoardId === board.boardId;
                const dropPosition = boardDropTarget?.boardId === board.boardId
                  ? boardDropTarget.position
                  : null;
                return (
                  <button
                    type="button"
                    key={board.boardId}
                    className={`pin-board-tree-row${board.boardId === selected ? " active" : ""}${dragging ? " dragging" : ""}${dropPosition ? ` drop-${dropPosition}` : ""}`}
                    draggable={boards.length > 1 && transferProgress === null}
                    onClick={() => void select(board)}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      setContextMenu(null);
                      setListContextMenu({ board, trash: false, x: event.clientX, y: event.clientY });
                    }}
                    onDragStart={(event) => {
                      setContextMenu(null);
                      setListContextMenu(null);
                      setDraggingBoardId(board.boardId);
                      setBoardDropTarget(null);
                      event.dataTransfer.effectAllowed = "move";
                      event.dataTransfer.setData(PIN_BOARD_DRAG_TYPE, String(board.boardId));
                      event.dataTransfer.setData("text/plain", board.name);
                    }}
                    onDragOver={(event) => {
                      if (draggingBoardId === null || draggingBoardId === board.boardId) return;
                      event.preventDefault();
                      event.dataTransfer.dropEffect = "move";
                      const rect = event.currentTarget.getBoundingClientRect();
                      const position: BoardDropPosition =
                        event.clientY < rect.top + rect.height / 2 ? "before" : "after";
                      setBoardDropTarget((current) => (
                        current?.boardId === board.boardId && current.position === position
                          ? current
                          : { boardId: board.boardId, position }
                      ));
                    }}
                    onDrop={(event) => {
                      event.preventDefault();
                      const draggedId =
                        Number(event.dataTransfer.getData(PIN_BOARD_DRAG_TYPE)) || draggingBoardId;
                      const position: BoardDropPosition = boardDropTarget?.boardId === board.boardId
                        ? boardDropTarget.position
                        : "after";
                      setDraggingBoardId(null);
                      setBoardDropTarget(null);
                      if (draggedId === null || draggedId === board.boardId) return;
                      moveBoard(draggedId, board.boardId, position);
                    }}
                    onDragEnd={() => {
                      setDraggingBoardId(null);
                      setBoardDropTarget(null);
                    }}
                    title={boards.length > 1 ? `拖动可调整顺序：${board.name}` : board.name}
                  >
                    <Images size={16} />
                    <span>{board.name}</span>
                  </button>
                );
              })}
              {boards.length === 0 && (
                <div className="pin-board-tree-empty">尚无画板，点击右上角“新建画板”</div>
              )}
            </div>
            : <LoaderCircle className="spin" size={20} />}
        </div>
        <div className="pin-board-trash-section">
          <div className="pin-board-trash-divider" />
          <div
            className="pin-board-trash-heading"
            onContextMenu={(event) => {
              event.preventDefault();
              setContextMenu(null);
              setListContextMenu({ board: null, trash: true, x: event.clientX, y: event.clientY });
            }}
          >
            <Trash2 size={16} />
            <span>回收站</span>
            <small>{trash.length}</small>
            {trash.length > 0 && (
              <button
                type="button"
                className="icon-button"
                title="清空回收站"
                aria-label="清空回收站"
                onClick={() => setConfirmRequest({ kind: "empty-trash" })}
              >
                <Trash2 size={14} />
              </button>
            )}
          </div>
          <div className="pin-board-trash-list">
            {trash.map((board) => (
              <button
                type="button"
                className="pin-board-tree-row trash"
                key={board.boardId}
                onContextMenu={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  setContextMenu(null);
                  setListContextMenu({ board, trash: true, x: event.clientX, y: event.clientY });
                }}
                title={`来自作品“${board.artworkTitle}”；右键恢复或永久删除`}
              >
                <Images size={16} />
                <span>{board.name}</span>
              </button>
            ))}
            {trash.length === 0 && <div className="pin-board-trash-empty">回收站为空</div>}
          </div>
        </div>
      </aside>
      <div ref={workspaceRef} className={`pin-board-workspace${fullscreen ? " fullscreen" : ""}`}>
        <header className="pin-board-toolbar">
          <button
            className="icon-button"
            onClick={toggleSidebar}
            title={sidebar ? "收起画板列表" : "展开画板列表"}
          >
            {sidebar ? <PanelLeftClose size={18} /> : <PanelLeftOpen size={18} />}
          </button>
          <div className="pin-board-title">
            <strong>{title}</strong>
            <span>
              {view
                ? `${renderer ? interaction.imageCount : view.images.filter((image) => !image.deleted).length} 张图片${view.missingTextures ? `，${view.missingTextures} 张缺失` : ""}`
                : "选择画板"}
            </span>
          </div>
          <div className="pin-board-actions" aria-label="画板工具">
            <button className="icon-button" disabled={!renderer || !interaction.canUndo} onClick={() => run(() => renderer?.undo())} title="撤销" aria-label="撤销"><Undo2 size={17} /></button>
            <button className="icon-button" disabled={!renderer || !interaction.canRedo} onClick={() => run(() => renderer?.redo())} title="恢复" aria-label="恢复"><Redo2 size={17} /></button>
            <span className="pin-board-divider" />
            <button className="icon-button" disabled={!renderer} onClick={() => run(() => renderer?.zoomOut())} title="缩小视图" aria-label="缩小视图"><ZoomOut size={17} /></button>
            <span className="pin-board-zoom">{interaction.zoomPercent}%</span>
            <button className="icon-button" disabled={!renderer} onClick={() => run(() => renderer?.zoomIn())} title="放大视图" aria-label="放大视图"><ZoomIn size={17} /></button>
            <button className="icon-button" disabled={!renderer} onClick={() => run(() => renderer?.resetViewport())} title="重置视图" aria-label="重置视图"><LocateFixed size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.scaleSelected(1 / 1.1))} title="缩小图片" aria-label="缩小图片"><Shrink size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.scaleSelected(1.1))} title="放大图片" aria-label="放大图片"><Expand size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.rotateSelected(-5))} title="逆时针旋转" aria-label="逆时针旋转"><RotateCcw size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.rotateSelected(5))} title="顺时针旋转" aria-label="顺时针旋转"><RotateCw size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.flipSelected(true))} title="水平翻转" aria-label="水平翻转"><FlipHorizontal2 size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={() => run(() => renderer?.flipSelected(false))} title="垂直翻转" aria-label="垂直翻转"><FlipVertical2 size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount < 2 || interaction.locked} onClick={() => run(() => renderer?.arrangeSelected())} title="阵列排序" aria-label="阵列排序"><LayoutGrid size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={copyImages} title="复制画板图片" aria-label="复制画板图片"><Copy size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount !== 1 || interaction.locked} onClick={() => void copyImageToClipboard()} title="复制图片到剪贴板" aria-label="复制图片到剪贴板"><ClipboardCopy size={17} /></button>
            <button className="icon-button" disabled={!renderer || clipboardCount === 0 || interaction.locked} onClick={pasteImages} title="粘贴画板图片" aria-label="粘贴画板图片"><ClipboardPaste size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.locked || transferProgress !== null} onClick={() => void importImages()} title="导入图片" aria-label="导入图片"><Upload size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.locked || transferProgress !== null} onClick={openTextDialog} title="添加文字" aria-label="添加文字"><Type size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked || transferProgress !== null} onClick={() => void exportImages()} title="导出图片" aria-label="导出图片"><Download size={17} /></button>
            <button className={`icon-button${interaction.locked ? " active" : ""}`} disabled={!renderer} onClick={toggleLock} title={actionTitle(interaction.locked ? "解锁画板" : "锁定画板", settings.lockShortcut)} aria-label={interaction.locked ? "解锁画板" : "锁定画板"}>{interaction.locked ? <Lock size={17} /> : <Unlock size={17} />}</button>
            <button className={`icon-button${fullscreen ? " active" : ""}`} disabled={!renderer} onClick={() => void toggleFullscreen()} title={actionTitle(fullscreen ? "退出全屏" : "全屏显示", settings.fullscreenShortcut)} aria-label={fullscreen ? "退出全屏" : "全屏显示"}>{fullscreen ? <Minimize2 size={17} /> : <Fullscreen size={17} />}</button>
            <button className="icon-button" disabled={!renderer || !interaction.dirty || interaction.saving} onClick={() => void renderer?.save()} title="保存画板" aria-label="保存画板"><Save size={17} /></button>
          </div>
        </header>
        {status && (
          <div className="pin-board-status" aria-live="polite" title={status}>
            <>
              <AlertTriangle size={16} />
              <span>{status}</span>
            </>
          </div>
        )}
        {transferProgress && (
          <div className="pin-board-transfer-progress" role="status">
            <div>
              <span>{transferProgress.label}</span>
              <strong>{Math.round((transferProgress.current / Math.max(transferProgress.total, 1)) * 100)}%</strong>
            </div>
            <progress value={transferProgress.current} max={Math.max(transferProgress.total, 1)} />
          </div>
        )}
        <div
          className={`pin-board-stage${dropActive ? " drop-active" : ""}`}
          onDragOver={handleStageDragOver}
          onDragLeave={handleStageDragLeave}
          onDrop={handleStageDrop}
        >
          <GpuCanvas
            view={view}
            artworkId={artworkId}
            arrangementGapCssPixels={settings.arrangementGapPx}
            autosaveEnabled={settings.autosave}
            cacheBudgets={textureBudgetsForLevel(settings.textureCacheLevel)}
            active={active}
            setStatus={setStatus}
            onRenderer={setRenderer}
            onState={handleRendererState}
            onContextMenu={handleRendererContextMenu}
          />
          {!view && (
            <div className="pin-board-stage-overlay" role="status">
              {boards === null
                ? <LoaderCircle className="spin" size={24} />
                : (
                  <div className="pin-board-stage-hint">
                    <Images size={22} />
                    <span>{boards.length === 0
                      ? "尚无画板，点击右上角“新建画板”创建"
                      : "当前未选择素材板，请从左侧列表选择画板"}</span>
                  </div>
                )}
            </div>
          )}
          {view && interaction.locked && (
            <span className="pin-board-lock-indicator" title="画板已锁定" aria-label="画板已锁定">
              <Lock size={16} />
            </span>
          )}
          {dropActive && (
            <div className="pin-board-drop-hint" role="status">
              <Upload size={15} />
              <span>松开以导入图片</span>
            </div>
          )}
        </div>
        {textDialogOpen && (
          <PinBoardTextDialog
            initialFontSize={textOptions.fontSize}
            initialColor={textOptions.color}
            onSubmit={(text, fontSize, color) => void addText(text, fontSize, color)}
            onClose={closeTextDialog}
          />
        )}
        {labelPrompt && (
          <PromptDialog
            title={labelPrompt.kind === "rename" ? "重命名画板" : "新建画板"}
            label="画板名称"
            initialValue={labelPrompt.kind === "rename" ? labelPrompt.board.name : ""}
            confirmLabel={labelPrompt.kind === "rename" ? "重命名" : "创建"}
            onConfirm={submitLabelPrompt}
            onCancel={() => setLabelPrompt(null)}
          />
        )}
        {confirmView && (
          <ConfirmDialog
            view={confirmView}
            busy={false}
            onCancel={() => setConfirmRequest(null)}
            onConfirm={submitConfirmRequest}
          />
        )}
        {contextMenu && renderer && (
          <PinBoardContextMenu
            request={contextMenu}
            renderer={renderer}
            fullscreen={fullscreen}
            canPaste={clipboardCount > 0}
            onCopy={copyImages}
            onCopyClipboard={() => void copyImageToClipboard()}
            onPaste={pasteImages}
            onImport={() => void importImages()}
            onAddText={openTextDialog}
            onExport={() => void exportImages()}
            onDelete={deleteImages}
            onToggleFullscreen={() => void toggleFullscreen()}
            onClose={() => setContextMenu(null)}
          />
        )}
        {listContextMenu && (
          <ContextMenu
            x={listContextMenu.x}
            y={listContextMenu.y}
            items={[
              {
                key: "heading",
                heading: true,
                label: listContextMenu.trash
                  ? "画板回收站"
                  : listContextMenu.board?.name ?? "素材板",
                icon: listContextMenu.trash ? <Trash2 size={15} /> : <Images size={15} />,
              },
              ...(listContextMenu.trash
                ? listContextMenu.board
                  ? [
                      { key: "restore", label: "恢复到原作品", icon: <Undo2 size={15} />, onClick: () => {
                        const board = listContextMenu.board!;
                        setListContextMenu(null);
                        void pinBoardApi.restorePinBoard(board.boardId)
                          .then(() => refreshBoards())
                          .catch((error) => setStatus(errorMessage(error)));
                      } },
                      { key: "permanent-delete", label: "永久删除", icon: <Trash2 size={15} />, danger: true, onClick: () => {
                        setConfirmRequest({ kind: "permanent-delete", board: listContextMenu.board! });
                        setListContextMenu(null);
                      } },
                    ]
                  : [
                      { key: "empty-trash", label: "清空回收站", icon: <Trash2 size={15} />, disabled: trash.length === 0, onClick: () => {
                        setConfirmRequest({ kind: "empty-trash" });
                        setListContextMenu(null);
                      } },
                    ]
                : listContextMenu.board
                  ? [
                      { key: "rename", label: "重命名", icon: <Pencil size={15} />, onClick: () => {
                        setLabelPrompt({ kind: "rename", board: listContextMenu.board! });
                        setListContextMenu(null);
                      } },
                      { key: "trash", label: "移入回收站", icon: <Trash2 size={15} />, danger: true, onClick: () => {
                        setConfirmRequest({ kind: "trash", board: listContextMenu.board! });
                        setListContextMenu(null);
                      } },
                    ]
                  : [
                      { key: "create", label: "新建画板", icon: <Plus size={15} />, onClick: () => {
                        setLabelPrompt({ kind: "create" });
                        setListContextMenu(null);
                      } },
                    ]),
            ]}
            onClose={() => setListContextMenu(null)}
          />
        )}
      </div>
    </section>
  );
}
