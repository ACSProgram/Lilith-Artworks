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
import { errorMessage } from "../../shared/errors";
import { ConfirmDialog, type ConfirmView } from "../../shared/ConfirmDialog";
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

/** 素材板固定快捷键（应用尚无全局快捷键配置，取 Client 侧默认键位）。 */
const PIN_BOARD_LOCK_SHORTCUT = "CommandOrControl+Shift+K";
const PIN_BOARD_FULLSCREEN_SHORTCUT = "F11";

export interface PinBoardModuleSettings {
  arrangementGapPx: number;
  textureCacheLevel: PinBoardTextureCacheLevel;
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
  cacheBudgets,
  active,
  setStatus,
  onRenderer,
  onState,
  onContextMenu,
}: {
  view: PinBoardView;
  artworkId: string;
  arrangementGapCssPixels: number;
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
  const activeRef = useRef(active);
  activeRef.current = active;

  useEffect(() => {
    let cancelled = false;
    let renderer: PinBoardRenderer | null = null;
    let unregisterLifecycle: (() => void) | null = null;
    const canvas = canvasRef.current;
    const selection = selectionRef.current;
    const marquee = marqueeRef.current;
    if (!canvas || !selection || !marquee) return undefined;

    void PinBoardRenderer.create(
      canvas,
      selection,
      marquee,
      view,
      getBoardSession(artworkId, view.boardId),
      activeRef.current,
      arrangementGapCssPixels,
      (message) => setStatus(message),
      (state) => onState(state),
      (session) => setBoardSession(artworkId, view.boardId, session),
      onContextMenu,
      cacheBudgets,
    )
      .then((created) => {
        if (cancelled) {
          created.destroy(false);
        } else {
          created.setActive(activeRef.current);
          renderer = created;
          unregisterLifecycle = registerPinBoardLifecycleParticipant(created);
          onRenderer(created);
          onState(created.interactionState);
        }
      })
      .catch((error) => {
        if (!cancelled) setStatus(errorMessage(error));
      });

    return () => {
      cancelled = true;
      unregisterLifecycle?.();
      onRenderer(null);
      renderer?.destroy();
    };
  }, [view, artworkId, setStatus, onRenderer, onState, onContextMenu]);

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
  const rendererRef = useRef<PinBoardRenderer | null>(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const loadSequence = useRef(0);
  const clipboardRef = useRef<PinBoardClipboardImage[]>([]);
  const textPlacementRef = useRef<[number, number] | null>(null);
  const fullscreenRef = useRef(false);
  const fullscreenBusyRef = useRef(false);
  const transferBusyRef = useRef(false);
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

  useEffect(() => {
    rendererRef.current?.setActive(active);
    if (!active) {
      setContextMenu(null);
      setListContextMenu(null);
    }
  }, [active]);

  useEffect(() => {
    rendererRef.current?.setArrangementGapCssPixels(settings.arrangementGapPx);
  }, [settings.arrangementGapPx]);

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
    if (current) {
      current.destroy();
      rendererRef.current = null;
    }
    setBoards(null);
    setTrash([]);
    setView(null);
    setSelected(null);
    setInteraction(DEFAULT_INTERACTION);
    setContextMenu(null);
    setListContextMenu(null);
    setStatus(null);

    void pinBoardApi.listPinBoards(artworkId)
      .then(async (next) => {
        if (cancelled || sequence !== loadSequence.current) return;
        setBoards(next);
        setTrash(await pinBoardApi.listPinBoardTrash().catch(() => []));
        const preferredId = getSelectedBoardId(artworkId);
        const first = next.find((board) => board.boardId === preferredId) ?? next[0];
        if (!first) return;
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
    if (rendererRef.current) {
      const saved = await rendererRef.current.finalize();
      if (!saved || sequence !== loadSequence.current) return;
      rendererRef.current.destroy();
      rendererRef.current = null;
    }
    setContextMenu(null);
    setSelected(board.boardId);
    setSelectedBoardId(artworkId, board.boardId);
    setView(null);
    setInteraction(DEFAULT_INTERACTION);
    setStatus(null);
    try {
      const loaded = await pinBoardApi.loadPinBoard(board.boardId);
      if (sequence === loadSequence.current) setView(loaded);
    } catch (error) {
      if (sequence === loadSequence.current) setStatus(errorMessage(error));
    }
  };

  const replaceAfterImageMutation = useCallback(async (
    action: (renderer: PinBoardRenderer, revision: string) => Promise<PinBoardMutationResult>,
    success: (count: number) => string,
  ) => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked) return;
    const sequence = ++loadSequence.current;
    if (!await current.save() || sequence !== loadSequence.current) return;
    try {
      const result = await action(current, current.boardRevision);
      if (sequence !== loadSequence.current) return;
      setContextMenu(null);
      current.applyMutationResult(result.view, result.imageIds);
      setInteraction(current.interactionState);
      setStatus(success(result.imageIds.length));
    } catch (error) {
      if (sequence === loadSequence.current) setStatus(errorMessage(error));
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
          rendererRef.current?.destroy();
          rendererRef.current = null;
          setSelected(null);
          setView(null);
          setInteraction(DEFAULT_INTERACTION);
          const fallback = next[0];
          if (fallback) {
            setSelected(fallback.boardId);
            setSelectedBoardId(artworkId, fallback.boardId);
            setView(await pinBoardApi.loadPinBoard(fallback.boardId));
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
      const fullscreenRequested = shortcutMatches(event, PIN_BOARD_FULLSCREEN_SHORTCUT)
        || (event.key === "Escape" && fullscreenRef.current);
      const lockRequested = shortcutMatches(event, PIN_BOARD_LOCK_SHORTCUT);
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
  }, [active, toggleFullscreen, toggleLock]);

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
              {boards.map((board) => (
                <button
                  type="button"
                  className={`pin-board-tree-row${board.boardId === selected ? " active" : ""}`}
                  key={board.boardId}
                  onClick={() => void select(board)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setContextMenu(null);
                    setListContextMenu({ board, trash: false, x: event.clientX, y: event.clientY });
                  }}
                  title={board.name}
                >
                  <Images size={16} />
                  <span>{board.name}</span>
                </button>
              ))}
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
            <button className={`icon-button${interaction.locked ? " active" : ""}`} disabled={!renderer} onClick={toggleLock} title={actionTitle(interaction.locked ? "解锁画板" : "锁定画板", PIN_BOARD_LOCK_SHORTCUT)} aria-label={interaction.locked ? "解锁画板" : "锁定画板"}>{interaction.locked ? <Lock size={17} /> : <Unlock size={17} />}</button>
            <button className={`icon-button${fullscreen ? " active" : ""}`} disabled={!renderer} onClick={() => void toggleFullscreen()} title={actionTitle(fullscreen ? "退出全屏" : "全屏显示", PIN_BOARD_FULLSCREEN_SHORTCUT)} aria-label={fullscreen ? "退出全屏" : "全屏显示"}>{fullscreen ? <Minimize2 size={17} /> : <Fullscreen size={17} />}</button>
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
        <div className="pin-board-stage">
          {view
            ? <GpuCanvas
              view={view}
              artworkId={artworkId}
              arrangementGapCssPixels={settings.arrangementGapPx}
              cacheBudgets={textureBudgetsForLevel(settings.textureCacheLevel)}
              active={active}
              setStatus={setStatus}
              onRenderer={setRenderer}
              onState={handleRendererState}
              onContextMenu={handleRendererContextMenu}
            />
            : <LoaderCircle className="spin" size={24} />}
          {view && interaction.locked && (
            <span className="pin-board-lock-indicator" title="画板已锁定" aria-label="画板已锁定">
              <Lock size={16} />
            </span>
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
