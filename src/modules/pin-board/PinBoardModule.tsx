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
  Minus,
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
  setPinBoardWindowFullscreen,
} from "./api";
import type { BoardDropPosition } from "./boardOrder";
import type {
  PinBoardModuleSettings,
  PinBoardSummary,
  PinBoardView,
} from "./types";
import {
  PinBoardRenderer,
  type PinBoardContextMenuRequest,
  type PinBoardInteractionState,
} from "./renderer";
import { textureBudgetsForLevel, type PinBoardTextureBudgets } from "./texturePolicy";
import {
  getBoardSession,
  getSidebarVisible,
  setBoardSession,
  setSidebarVisible,
} from "./session";
import { shortcutCanHandle, shortcutMatches } from "./shortcuts";
import { usePinBoardController } from "./usePinBoardController";
import { usePendingSettle } from "./usePendingSettle";
import { retryPendingSettle } from "./pendingSettle";
import { PinBoardColorPicker } from "./PinBoardColorPicker";

/**
 * 素材板快捷键默认值，与设置页默认项保持一致。
 * 锁定沿用 Client 的默认键位 Ctrl+R；WebView 整页刷新由应用层的
 * `preventWebViewReload` 只取消默认行为，因此不会与本快捷键冲突。
 */
export const PIN_BOARD_LOCK_SHORTCUT = "CommandOrControl+R";
export const PIN_BOARD_FULLSCREEN_SHORTCUT = "F11";

/**
 * 阵列间距的默认值与上下限，单位是「统一图片高度的比例」。
 *
 * 与 Rust 侧 `pin_board::arrange::{DEFAULT_GAP_RATIO, MAX_GAP_RATIO}` 保持一致：
 * 间距必须随图片大小走，不能按屏幕像素算，否则同一个操作在不同缩放下结果不同。
 * 下限允许 `0`（图片紧贴，用于联络表式的拼接）；上限 20% 之后再大就看不出是阵列了。
 */
export const PIN_BOARD_ARRANGEMENT_GAP_RATIO = 0.06;
export const PIN_BOARD_ARRANGEMENT_GAP_RATIO_MAX = 0.2;

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

/**
 * 渲染器创建失败时的常驻说明。
 *
 * 素材板完全依赖 WebGPU，创建失败后画布在整个进程生命周期内都不会再可用，因此这条说明不能
 * 走 4.5 秒自动清除的状态栏：清空之后画板只剩空白与全部禁用的按钮，用户只会判断成「卡死」。
 * 原始原因保留在文案里，好区分「没有可用适配器」「缺少 BC7 纹理压缩」这几种不同的处置方式。
 */
export function pinBoardUnavailableNotice(reason: string): string {
  return "素材板需要支持 WebGPU 与 BC7 纹理压缩的图形适配器，当前环境不可用"
    + `（${reason}）。请更新显卡驱动、开启虚拟机的 3D 加速，或改在具备可用适配器的环境`
    + "运行后重启应用；其余功能不受影响。";
}

/**
 * 「画板已锁定」标记的显示条件。
 *
 * 渲染器没起来时画板停在默认的锁定状态，但那不是用户锁的；此时显示这个标记会把排查引向
 * 错误方向，所以只显示不可用说明。
 */
export function pinBoardLockIndicatorVisible({ hasView, locked, unavailable }: {
  hasView: boolean;
  locked: boolean;
  unavailable: boolean;
}): boolean {
  return hasView && locked && !unavailable;
}

/** 侧栏画板拖放排序使用的自定义 dataTransfer 类型。 */
const PIN_BOARD_DRAG_TYPE = "application/x-lilith-pin-board-board";

function afterLayoutSettles(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

function GpuCanvas({
  view,
  artworkId,
  arrangementGapRatio,
  autosaveEnabled,
  cacheBudgets,
  active,
  setStatus,
  onUnavailable,
  onRenderer,
  onState,
  onContextMenu,
}: {
  view: PinBoardView | null;
  artworkId: string;
  arrangementGapRatio: number;
  autosaveEnabled: boolean;
  cacheBudgets: PinBoardTextureBudgets;
  active: boolean;
  setStatus: (message: string | null) => void;
  /** 渲染器创建失败时的常驻原因；创建成功或尚未尝试时为 null。 */
  onUnavailable: (reason: string | null) => void;
  onRenderer: (renderer: PinBoardRenderer | null) => void;
  onState: (state: PinBoardInteractionState) => void;
  onContextMenu: (request: PinBoardContextMenuRequest | null) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const selectionRef = useRef<HTMLDivElement>(null);
  const marqueeRef = useRef<HTMLDivElement>(null);
  /** 纹理未就绪图片的占位层：由渲染器按视口同步填充。 */
  const placeholderRef = useRef<HTMLDivElement>(null);
  /** 渲染器实例常驻：创建 effect 只随画布生命周期运行一次。 */
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const bootRef = useRef<Promise<PinBoardRenderer | null> | null>(null);
  const destroyTimerRef = useRef<number | null>(null);
  const disposedRef = useRef(false);
  /** 渲染器就绪后自增，触发「装载画板」effect 用最新 view 装载。 */
  const [readyVersion, setReadyVersion] = useState(0);

  const activeRef = useRef(active);
  activeRef.current = active;
  const artworkIdRef = useRef(artworkId);
  artworkIdRef.current = artworkId;
  const gapRef = useRef(arrangementGapRatio);
  gapRef.current = arrangementGapRatio;
  const autosaveRef = useRef(autosaveEnabled);
  autosaveRef.current = autosaveEnabled;
  const cacheBudgetsRef = useRef(cacheBudgets);
  cacheBudgetsRef.current = cacheBudgets;
  const setStatusRef = useRef(setStatus);
  setStatusRef.current = setStatus;
  const onUnavailableRef = useRef(onUnavailable);
  onUnavailableRef.current = onUnavailable;
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
    const placeholder = placeholderRef.current;
    if (!canvas || !selection || !marquee || !placeholder) return undefined;

    if (!bootRef.current) {
      bootRef.current = PinBoardRenderer.create(
        canvas,
        selection,
        marquee,
        placeholder,
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
          created.setArrangementGapRatio(gapRef.current);
          created.setAutosaveEnabled(autosaveRef.current);
          created.setTextureCacheBudgets(cacheBudgetsRef.current);
          created.setActive(activeRef.current);
          rendererRef.current = created;
          onUnavailableRef.current(null);
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
          // 走常驻说明而不是 4.5 秒自动清除的状态栏：这条原因必须一直看得见。
          if (!disposedRef.current) onUnavailableRef.current(errorMessage(error));
          return null;
        });
    }

    return () => {
      disposedRef.current = true;
      destroyTimerRef.current = window.setTimeout(() => {
        destroyTimerRef.current = null;
        bootRef.current = null;
        const current = rendererRef.current;
        rendererRef.current = null;
        onRendererRef.current(null);
        diagnosticsLog("info", `pin-board renderer disposing: artworkId=${artworkIdRef.current}`);
        current?.destroy();
      }, 0);
    };
  }, []);

  // 画板装载：view 变化或渲染器就绪时装载（按 boardId 幂等）。装载是同步的——旧画板的
  // 未保存改动交给后台结算队列，画布不会因为仓库写锁被占而停在上一块画板。
  useEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    if (view) {
      renderer.loadBoard(view, getBoardSession(artworkId, view.boardId));
    } else {
      renderer.unloadBoard();
    }
  }, [view, artworkId, readyVersion]);

  return (
    <div className="pin-board-canvas-shell">
      <canvas ref={canvasRef} className="pin-board-canvas" tabIndex={0} />
      <div ref={placeholderRef} className="pin-board-placeholder-layer" aria-hidden="true" />
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
  const [sizeText, setSizeText] = useState(String(initialFontSize));
  const [color, setColor] = useState(initialColor);
  const filled = text.trim().length > 0;
  const setSize = (next: number) => {
    const clamped = Math.min(240, Math.max(12, Math.round(next)));
    setFontSize(clamped);
    setSizeText(String(clamped));
  };

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
          if (filled) onSubmit(text, fontSize, color);
        }}
      >
        <header className="pin-board-text-header">
          <span className="pin-board-text-header-icon" aria-hidden="true">
            <Type size={18} />
          </span>
          <div className="pin-board-text-heading">
            <h2 id="pin-board-text-title">添加文字</h2>
            <p>调整内容与样式，文字会添加到当前画板</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} title="关闭" aria-label="关闭">
            <X size={17} />
          </button>
        </header>
        <div className="pin-board-text-body">
          <div className="pin-board-text-preview" aria-hidden="true">
            <span
              className={filled ? undefined : "pin-board-text-preview-placeholder"}
              style={filled
                ? { color, fontSize: `${Math.min(34, Math.max(fontSize, 14))}px` }
                : undefined}
            >
              {filled ? text : "文字预览"}
            </span>
          </div>
          <label className="pin-board-text-field">
            <span className="pin-board-text-label">
              <span>内容</span>
              <span className="pin-board-text-count">{text.length} / 5000</span>
            </span>
            <textarea
              className="pin-board-text-input"
              autoFocus
              maxLength={5000}
              rows={5}
              placeholder="输入要添加的文字，支持多行"
              value={text}
              onChange={(event) => setText(event.target.value)}
            />
          </label>
          <div className="pin-board-text-options">
            <div className="pin-board-text-field">
              <span className="pin-board-text-label"><span>字号</span></span>
              <div className="pin-board-text-size">
                <button
                  type="button"
                  aria-label="减小字号"
                  disabled={fontSize <= 12}
                  onClick={() => setSize(fontSize - 1)}
                >
                  <Minus size={14} aria-hidden="true" />
                </button>
                <input
                  type="number"
                  min={12}
                  max={240}
                  step={1}
                  value={sizeText}
                  aria-label="字号"
                  onChange={(event) => {
                    const raw = event.target.value;
                    setSizeText(raw);
                    const parsed = Number(raw);
                    if (Number.isFinite(parsed) && parsed >= 12 && parsed <= 240) {
                      setFontSize(Math.round(parsed));
                    }
                  }}
                  onBlur={() => setSize(Number(sizeText) || fontSize)}
                />
                <button
                  type="button"
                  aria-label="增大字号"
                  disabled={fontSize >= 240}
                  onClick={() => setSize(fontSize + 1)}
                >
                  <Plus size={14} aria-hidden="true" />
                </button>
              </div>
            </div>
            <div className="pin-board-text-field">
              <span className="pin-board-text-label"><span>颜色</span></span>
              <PinBoardColorPicker value={color} onChange={setColor} />
            </div>
          </div>
        </div>
        <footer className="modal-footer">
          <button className="secondary-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!filled}>添加</button>
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
        { key: "arrange", label: "阵列排序", icon: <LayoutGrid size={16} />, disabled: request.arrangeableCount < 2, onClick: () => void renderer.arrangeSelected() },
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

export function PinBoardModule({ artworkId, active, settings }: PinBoardModuleProps) {
  const {
    boards,
    trash,
    view,
    selected,
    status,
    transferProgress,
    clipboardCount,
    interaction,
    renderer,
    setRenderer,
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
  } = usePinBoardController({ artworkId, active, settings });
  /**
   * 渲染器创建失败的常驻原因。素材板完全依赖 WebGPU，创建失败后画布不会再可用，
   * 因此这条说明独立于 4.5 秒自动清除的 `status` 单独保存。
   */
  const [unavailable, setUnavailable] = useState<string | null>(null);
  /** 切到后台写回的画板改动（切换作品不再阻塞等待仓库写入，进度在这里可见）。 */
  const pendingSettle = usePendingSettle();
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
  /** 侧栏画板拖放排序：正在拖动的画板与当前落点。 */
  const [draggingBoardId, setDraggingBoardId] = useState<number | null>(null);
  const [boardDropTarget, setBoardDropTarget] = useState<
    { boardId: number; position: BoardDropPosition } | null
  >(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const textPlacementRef = useRef<[number, number] | null>(null);
  const fullscreenRef = useRef(false);
  const fullscreenBusyRef = useRef(false);

  const handleRendererContextMenu = useCallback((request: PinBoardContextMenuRequest | null) => {
    setListContextMenu(null);
    setContextMenu(request);
  }, []);

  useEffect(() => {
    if (!active) {
      setContextMenu(null);
      setListContextMenu(null);
      setDropActive(false);
    }
  }, [active]);

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

  const handleStageDragOver = useCallback((event: ReactDragEvent<HTMLDivElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    // 必须取消默认行为，否则浏览器会把它当成导航并拒绝这次放置。
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    if (renderer && !renderer.interactionState.locked) setDropActive(true);
  }, [renderer]);

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

  const openTextDialog = useCallback(() => {
    const current = renderer;
    if (!current || current.interactionState.locked) return;
    textPlacementRef.current = current.placementPoint;
    setTextDialogOpen(true);
  }, [renderer]);

  const closeTextDialog = useCallback(() => {
    textPlacementRef.current = null;
    setTextDialogOpen(false);
  }, []);

  const submitText = useCallback((text: string, fontSize: number, color: string) => {
    const placement = textPlacementRef.current ?? renderer?.placementPoint ?? [0, 0];
    textPlacementRef.current = null;
    setTextOptions({ fontSize, color });
    setTextDialogOpen(false);
    void addText(text, fontSize, color, placement);
  }, [addText, renderer]);

  const submitLabelPrompt = useCallback((label: string) => {
    const trimmed = label.trim();
    if (!trimmed) return;
    if (labelPrompt?.kind === "rename") {
      void renameBoard(labelPrompt.board.boardId, trimmed);
    } else if (labelPrompt?.kind === "create") {
      void createBoard(trimmed);
    }
    setLabelPrompt(null);
  }, [createBoard, labelPrompt, renameBoard]);

  const submitConfirmRequest = useCallback(() => {
    if (!confirmRequest) return;
    const request = confirmRequest;
    setConfirmRequest(null);
    if (request.kind === "trash") {
      void trashBoard(request.board);
    } else if (request.kind === "permanent-delete") {
      void permanentlyDeleteBoard(request.board.boardId);
    } else {
      void emptyTrash();
    }
  }, [confirmRequest, emptyTrash, permanentlyDeleteBoard, trashBoard]);

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
      action: "拖动到底清空回收站",
      danger: true,
      emphasis: true,
      slideToConfirm: true,
    };
  }, [confirmRequest, trash.length]);

  const title = useMemo(() => {
    if (!boards || selected === null) return "素材板";
    return boards.find((board) => board.boardId === selected)?.name ?? `画板 ${selected}`;
  }, [boards, selected]);

  const run = (action: () => void) => {
    action();
    syncInteraction();
  };

  const toggleSidebar = () => {
    setSidebar((current) => {
      setSidebarVisible(!current);
      return !current;
    });
  };

  const toggleFullscreen = useCallback(async () => {
    if (!workspaceRef.current || fullscreenBusyRef.current) return;
    const previous = fullscreenRef.current;
    const next = !previous;
    const current = renderer;
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
  }, [renderer, setStatus]);

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
      if (!shortcutCanHandle(event) || !(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "c") {
        event.preventDefault();
        event.stopImmediatePropagation();
        copyImages();
      } else if (key === "v" && clipboardCount > 0) {
        event.preventDefault();
        event.stopImmediatePropagation();
        pasteImages();
      }
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, clipboardCount, copyImages, pasteImages]);

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
                    onClick={() => {
                      setContextMenu(null);
                      void selectBoard(board);
                    }}
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
                ? `${renderer ? interaction.imageCount : view.items.filter((image) => !image.deleted).length} 个条目${view.missingTextures ? `，其中 ${view.missingTextures} 个位图缺失` : ""}`
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
            <button className="icon-button" disabled={!renderer || interaction.arrangeableCount < 2 || interaction.locked} onClick={() => run(() => void renderer?.arrangeSelected())} title="阵列排序" aria-label="阵列排序"><LayoutGrid size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || interaction.locked} onClick={copyImages} title="复制画板图片" aria-label="复制画板图片"><Copy size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount !== 1 || interaction.locked} onClick={() => void copyImageToClipboard()} title="复制图片到剪贴板" aria-label="复制图片到剪贴板"><ClipboardCopy size={17} /></button>
            <button className="icon-button" disabled={!renderer || clipboardCount === 0 || interaction.locked} onClick={pasteImages} title="粘贴画板图片" aria-label="粘贴画板图片"><ClipboardPaste size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.locked || transferProgress !== null} onClick={() => void importImages()} title="导入图片" aria-label="导入图片"><Upload size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.locked || transferProgress !== null} onClick={openTextDialog} title="添加文字" aria-label="添加文字"><Type size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.exportableCount === 0 || interaction.locked || transferProgress !== null} onClick={() => void exportImages()} title="导出图片" aria-label="导出图片"><Download size={17} /></button>
            <button className={`icon-button${interaction.locked ? " active" : ""}`} disabled={!renderer} onClick={toggleLock} title={actionTitle(interaction.locked ? "解锁画板" : "锁定画板", settings.lockShortcut)} aria-label={interaction.locked ? "解锁画板" : "锁定画板"}>{interaction.locked ? <Lock size={17} /> : <Unlock size={17} />}</button>
            <button className={`icon-button${fullscreen ? " active" : ""}`} disabled={!renderer} onClick={() => void toggleFullscreen()} title={actionTitle(fullscreen ? "退出全屏" : "全屏显示", settings.fullscreenShortcut)} aria-label={fullscreen ? "退出全屏" : "全屏显示"}>{fullscreen ? <Minimize2 size={17} /> : <Fullscreen size={17} />}</button>
            <button className="icon-button" disabled={!renderer || !interaction.dirty || interaction.saving} onClick={() => void save()} title="保存画板" aria-label="保存画板"><Save size={17} /></button>
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
        {pendingSettle && (
          <div
            className={`pin-board-status pin-board-settle${pendingSettle.failure ? " failed" : ""}`}
            role="status"
            title={pendingSettle.failure
              ? `${pendingSettle.failure.message}（画板 ${pendingSettle.failure.boardId}）`
              : undefined}
          >
            {pendingSettle.failure
              ? <AlertTriangle size={16} />
              : <LoaderCircle className="spin" size={16} />}
            <span>
              {pendingSettle.failure
                ? `${pendingSettle.count} 块画板的改动还没写进仓库：${pendingSettle.failure.message}`
                : `正在后台保存 ${pendingSettle.count} 块画板的改动`}
            </span>
            {pendingSettle.failure && (
              <button type="button" className="pin-board-settle-retry" onClick={retryPendingSettle}>
                重试
              </button>
            )}
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
            arrangementGapRatio={settings.arrangementGapRatio}
            autosaveEnabled={settings.autosave}
            cacheBudgets={textureBudgetsForLevel(settings.textureCacheLevel)}
            active={active}
            setStatus={setStatus}
            onUnavailable={setUnavailable}
            onRenderer={setRenderer}
            onState={setInteraction}
            onContextMenu={handleRendererContextMenu}
          />
          {unavailable ? (
            <div className="pin-board-stage-overlay pin-board-unavailable" role="alert">
              <div className="pin-board-stage-hint">
                <AlertTriangle size={22} />
                <strong>素材板不可用</strong>
                <span>{pinBoardUnavailableNotice(unavailable)}</span>
              </div>
            </div>
          ) : !view && (
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
          {pinBoardLockIndicatorVisible({
            hasView: view !== null,
            locked: interaction.locked,
            unavailable: unavailable !== null,
          }) && (
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
            onSubmit={submitText}
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
                        void restoreBoard(board.boardId);
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
