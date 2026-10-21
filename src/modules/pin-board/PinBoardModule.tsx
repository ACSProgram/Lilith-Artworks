import {
  AlertTriangle,
  Circle,
  ClipboardCopy,
  ClipboardPaste,
  Copy,
  Download,
  Eraser,
  Expand,
  FlipHorizontal2,
  FlipVertical2,
  FolderOpen,
  Fullscreen,
  ImageDown,
  Images,
  LayoutGrid,
  Layers3,
  LoaderCircle,
  Lock,
  LocateFixed,
  Minimize2,
  Minus,
  MousePointer2,
  MoveUpRight,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Plus,
  Redo2,
  RotateCcw,
  RotateCw,
  Save,
  Shrink,
  Slash,
  Square,
  Trash2,
  Type,
  Unlock,
  Undo2,
  Upload,
  X,
  ZoomIn,
  ZoomOut,
  type LucideIcon,
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
  pickPinBoardExportDirectory,
  setPinBoardWindowFullscreen,
} from "./api";
import type { BoardDropPosition } from "./boardOrder";
import {
  CANVAS_EXPORT_DEFAULT_MARGIN,
  CANVAS_EXPORT_MAX_MARGIN,
  CANVAS_EXPORT_MAX_SCALE,
  CANVAS_EXPORT_MIN_SCALE,
  clampCanvasExportMargin,
  type CanvasExportPlan,
} from "./exportPolicy";
import type {
  PinBoardModuleSettings,
  PinBoardSummary,
  PinBoardTransferProgress,
  PinBoardView,
} from "./types";
import {
  PinBoardRenderer,
  type PinBoardContextMenuRequest,
  type PinBoardInteractionState,
} from "./renderer";
import { textureBudgetsForLevel, type PinBoardTextureBudgets } from "./texturePolicy";
import { PinBoardLayersPanel } from "./PinBoardLayersPanel";
import {
  getBoardSession,
  getSidebarVisible,
  setBoardSession,
  setSidebarVisible,
} from "./session";
import { shortcutCanHandle, shortcutMatches } from "./shortcuts";
import {
  DEFAULT_STROKE_COLOR,
  DEFAULT_STROKE_WIDTH,
  MAX_STROKE_WIDTH,
  MIN_STROKE_WIDTH,
  STROKE_WIDTH_SLIDER_STEPS,
  clampStrokeWidth,
  strokeWidthFromSlider,
  strokeWidthToSlider,
  type PinBoardTool,
  type StrokeCommitRequest,
} from "./strokeDrawing";
import { installStrokePerfProbe } from "./strokePerfProbe";
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
 * 标注工具条的显示/隐藏快捷键。
 *
 * 工具条收起后自身不可见，任何放在工具条上的按钮都无法再把它叫回来，因此显隐只由这一个
 * 对称的快捷键负责，主工具栏与浮动条本身都不放按钮。
 */
export const PIN_BOARD_TOOLBAR_SHORTCUT = "T";

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

/**
 * 标注工具条：选择、五种绘制工具与橡皮。快捷键与 `strokeDrawing.toolForShortcut`
 * 同源，提示里的字母必须与那里一致。
 */
const STROKE_TOOL_BUTTONS: Array<{
  tool: PinBoardTool;
  label: string;
  shortcut: string;
  icon: LucideIcon;
}> = [
  { tool: "select", label: "选择", shortcut: "V", icon: MousePointer2 },
  { tool: "pen", label: "画笔", shortcut: "P", icon: Pencil },
  { tool: "eraser", label: "橡皮", shortcut: "E", icon: Eraser },
  { tool: "line", label: "直线", shortcut: "L", icon: Slash },
  { tool: "arrow", label: "箭头", shortcut: "A", icon: MoveUpRight },
  { tool: "rect", label: "矩形", shortcut: "R", icon: Square },
  { tool: "ellipse", label: "椭圆", shortcut: "O", icon: Circle },
];

/** 工具按钮组：工具选择即交互模式，没有额外的模式开关。 */
function StrokeToolGroup({ tool, disabled, onSelect }: {
  tool: PinBoardTool;
  disabled: boolean;
  onSelect: (tool: PinBoardTool) => void;
}) {
  return (
    <div className="pin-board-tools" role="group" aria-label="工具">
      {STROKE_TOOL_BUTTONS.map(({ tool: value, label, shortcut, icon: Icon }) => (
        <button
          key={value}
          type="button"
          className={`icon-button${tool === value ? " active" : ""}`}
          aria-pressed={tool === value}
          disabled={disabled}
          title={`${label} (${shortcut})`}
          aria-label={label}
          onClick={() => onSelect(value)}
        >
          <Icon size={17} />
        </button>
      ))}
    </div>
  );
}

/**
 * 标注样式控件：取色器 + 大小滑条与数值输入。
 *
 * 滑条负责快速调粗细，数值框负责精确值：两者写同一份状态，滑条拖动时数值框
 * 实时跟随；数值框在失焦或回车时才收敛，输入过程中允许暂时不合法。
 *
 * 「大小」是画笔线宽与橡皮直径共用的同一个值：切换工具不记忆独立数值，橡皮的
 * 环形光标直径就取自这里。内部标识符仍叫 `strokeWidth`——它描述的是笔迹线宽这一
 * 物理量，改名的收益为零、噪音很大。
 *
 * 滑条走**指数映射**（`strokeWidthFromSlider`）：范围 1–500，线性滑条会把
 * 九成行程分给「一辈子用不到」的粗线；指数映射让 1–20 这段占到约 38% 行程，并把
 * 粗线端压平一点，右半段不会一拖就窜。
 */
function StrokeStyleControls({
  color,
  width,
  widthText,
  onColorChange,
  onWidthInput,
  onWidthTextChange,
  onWidthCommit,
}: {
  color: string;
  width: number;
  widthText: string;
  onColorChange: (color: string) => void;
  onWidthInput: (width: number) => void;
  onWidthTextChange: (raw: string) => void;
  onWidthCommit: (raw: string) => void;
}) {
  return (
    <div className="pin-board-stroke-style">
      <PinBoardColorPicker value={color} onChange={onColorChange} />
      <label className="pin-board-stroke-width">
        <span>大小</span>
        <input
          type="range"
          className="pin-board-stroke-width-range"
          min={0}
          max={STROKE_WIDTH_SLIDER_STEPS}
          step={1}
          value={strokeWidthToSlider(width)}
          aria-label="大小滑条"
          aria-valuetext={`大小 ${width}`}
          onChange={(event) => onWidthInput(strokeWidthFromSlider(Number(event.target.value)))}
        />
        <input
          type="number"
          min={MIN_STROKE_WIDTH}
          max={MAX_STROKE_WIDTH}
          step={1}
          value={widthText}
          aria-label="大小"
          onChange={(event) => onWidthTextChange(event.target.value)}
          onBlur={(event) => onWidthCommit(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            onWidthCommit(event.currentTarget.value);
            event.currentTarget.blur();
          }}
        />
      </label>
    </div>
  );
}

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
  onStrokeCommit,
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
  onStrokeCommit: (request: StrokeCommitRequest) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const selectionRef = useRef<HTMLDivElement>(null);
  const marqueeRef = useRef<HTMLDivElement>(null);
  /** 纹理未就绪图片的占位层：由渲染器按视口同步填充。 */
  const placeholderRef = useRef<HTMLDivElement>(null);
  /** 绘制预览层：渲染器按当前草稿重绘，不进 GPU 缓冲。 */
  const drawOverlayRef = useRef<HTMLCanvasElement>(null);
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
  const onStrokeCommitRef = useRef(onStrokeCommit);
  onStrokeCommitRef.current = onStrokeCommit;

  // 开发期自检入口：控制台里 `__lilithPinBoardStrokeProbe({ strokes: 500 })` 能跑一遍
  // 合成笔迹的采样、编码与缓冲构建并打点。生产构建里这个分支会被整段去掉。
  useEffect(() => {
    if (import.meta.env.DEV) installStrokePerfProbe();
  }, []);

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
    const drawOverlay = drawOverlayRef.current;
    if (!canvas || !selection || !marquee || !placeholder || !drawOverlay) return undefined;

    if (!bootRef.current) {
      bootRef.current = PinBoardRenderer.create(
        canvas,
        selection,
        marquee,
        placeholder,
        drawOverlay,
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
        (request) => onStrokeCommitRef.current(request),
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
      <canvas ref={drawOverlayRef} className="pin-board-draw-overlay" aria-hidden="true" />
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

/** 图层菜单在选中含笔迹时的置灰说明：笔迹固定在标注层，原生端保存时也会强制写 3。 */
const STROKE_LAYER_LOCKED_TITLE = "笔迹固定在标注层，取消选中笔迹后再改图层";

/** 倍率输入框的文本：最多四位小数并去掉尾随零（0.4400 → 0.44，2.0000 → 2）。 */
function formatScaleText(scale: number): string {
  return scale.toFixed(4).replace(/\.?0+$/, "");
}

/**
 * 整画布导出对话框。
 *
 * 三个数字互相咬合：**倍率**是唯一的事实来源，**输出宽度**是它的另一种写法，改哪
 * 一个都会把另一个算回来；**边距**只改变画布范围，因此宽度随之变化而倍率不动。
 * 默认倍率由 `autoScale` 给出（1x 与「塞进建议上限」取小），填过之后以填的为准——
 * 建议上限只提示，不再偷偷压倍率。
 *
 * 「输出尺寸」与实际写出的 PNG 共用渲染器的 `canvasExportPlan`，所以对话框说
 * 20000 宽，文件就是 20000 宽。
 */
function PinBoardExportDialog({
  limit,
  boardName,
  planFor,
  autoScale,
  onStart,
  onClose,
}: {
  limit: number;
  boardName: string;
  planFor: (scale: number, margin: number) => CanvasExportPlan | null;
  autoScale: (margin: number) => number;
  onStart: (request: {
    directory: string;
    fileName: string;
    scale: number;
    margin: number;
  }) => void;
  onClose: () => void;
}) {
  const [directory, setDirectory] = useState<string | null>(null);
  const [fileName, setFileName] = useState(boardName);
  const [margin, setMargin] = useState(CANVAS_EXPORT_DEFAULT_MARGIN);
  const [marginText, setMarginText] = useState(String(CANVAS_EXPORT_DEFAULT_MARGIN));
  const [scale, setScale] = useState(() => autoScale(CANVAS_EXPORT_DEFAULT_MARGIN));
  const [scaleText, setScaleText] = useState(() => formatScaleText(scale));
  const plan = planFor(scale, margin);
  const [widthText, setWidthText] = useState(() => (plan ? String(plan.width) : ""));
  const [picking, setPicking] = useState(false);
  /** 含边距的内容宽度（世界单位）：宽度与倍率互换时用得到。 */
  const paddedWidth = plan ? plan.bounds.maxX - plan.bounds.minX : 0;
  const ready = directory !== null && plan !== null && fileName.trim() !== "";

  const chooseDirectory = async () => {
    if (picking) return;
    setPicking(true);
    try {
      const picked = await pickPinBoardExportDirectory();
      if (picked) setDirectory(picked);
    } catch {
      // 选择器不可用时保持原状，用户可以重试。
    } finally {
      setPicking(false);
    }
  };

  /** 把倍率写进两个输入框：宽度按同一份计划回算，不会出现「说的和算的不一致」。 */
  const applyScale = (next: number) => {
    setScale(next);
    setScaleText(formatScaleText(next));
    const nextPlan = planFor(next, margin);
    if (nextPlan) setWidthText(String(nextPlan.width));
  };

  const inputScale = (raw: string) => {
    setScaleText(raw);
    const parsed = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(parsed) || parsed <= 0) return;
    const next = Math.min(CANVAS_EXPORT_MAX_SCALE, Math.max(CANVAS_EXPORT_MIN_SCALE, parsed));
    setScale(next);
    const nextPlan = planFor(next, margin);
    if (nextPlan) setWidthText(String(nextPlan.width));
    // 只在上限夹过时改写输入框，避免打字过程中光标被挪走。
    if (next !== parsed) setScaleText(formatScaleText(next));
  };

  /**
   * 失焦时把倍率对齐到**实际生效**的值：撞上硬上限（边长或总像素）时计划里的倍率会被
   * 收敛，输入框要跟着显示收敛后的数字。打字过程中不回写，否则「99」在敲下「9」时就被
   * 改写成收敛值，后面的字符再也输不进去。
   */
  const commitScale = () => {
    const nextPlan = planFor(scale, margin);
    applyScale(nextPlan ? nextPlan.scale : scale);
  };

  /**
   * 宽度输入只是倍率的另一种写法：换算出的倍率同样落在 0.01–8 之间，否则很小的
   * 宽度会让倍率显示出「0」这种既不可读也不合法的值（失焦时按实际计划回写宽度）。
   */
  const inputWidth = (raw: string) => {
    setWidthText(raw);
    const parsed = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(parsed) || parsed < 1 || paddedWidth <= 0) return;
    const next = Math.min(
      CANVAS_EXPORT_MAX_SCALE,
      Math.max(CANVAS_EXPORT_MIN_SCALE, parsed / paddedWidth),
    );
    setScale(next);
    setScaleText(formatScaleText(next));
  };

  /** 边距只改画布范围：倍率不动，输出宽度按同一倍率重算。 */
  const applyMargin = (next: number) => {
    setMargin(next);
    const nextPlan = planFor(scale, next);
    if (nextPlan) setWidthText(String(nextPlan.width));
  };

  const inputMargin = (raw: string) => {
    setMarginText(raw);
    const parsed = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(parsed)) return;
    if (parsed < 0 || parsed > CANVAS_EXPORT_MAX_MARGIN) return;
    applyMargin(Math.round(parsed));
  };

  const commitMargin = (raw: string) => {
    const parsed = Number(raw);
    const next = raw.trim() !== "" && Number.isFinite(parsed)
      ? clampCanvasExportMargin(parsed)
      : margin;
    setMarginText(String(next));
    applyMargin(next);
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
        className="modal pin-board-export-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pin-board-export-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
        onSubmit={(event) => {
          event.preventDefault();
          if (!ready || !directory) return;
          onStart({
            directory,
            fileName: fileName.trim(),
            scale,
            margin,
          });
        }}
      >
        <header className="pin-board-text-header">
          <span className="pin-board-text-header-icon" aria-hidden="true">
            <ImageDown size={18} />
          </span>
          <div className="pin-board-text-heading">
            <h2 id="pin-board-export-title">导出画布</h2>
            <p>把当前画板的全部可见内容合成一张透明 PNG</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} title="关闭" aria-label="关闭">
            <X size={17} />
          </button>
        </header>
        <div className="pin-board-text-body">
          <div className="pin-board-text-field">
            <span className="pin-board-text-label"><span>导出位置</span></span>
            <div className="pin-board-export-path">
              <span title={directory ?? undefined}>
                {directory ?? "尚未选择文件夹"}
              </span>
              <button type="button" className="secondary-button" disabled={picking} onClick={() => void chooseDirectory()}>
                <FolderOpen size={14} aria-hidden="true" />
                选择…
              </button>
            </div>
          </div>
          <div className="pin-board-text-field">
            <span className="pin-board-text-label"><span>文件名</span></span>
            <div className="pin-board-export-input">
              <input
                type="text"
                value={fileName}
                maxLength={80}
                spellCheck={false}
                aria-label="导出文件名"
                onChange={(event) => setFileName(event.target.value)}
              />
              <span className="pin-board-export-suffix">.png</span>
            </div>
          </div>
          <div className="pin-board-export-grid">
            <div className="pin-board-text-field">
              <span className="pin-board-text-label"><span>倍率</span></span>
              <div className="pin-board-export-input">
                <input
                  type="number"
                  min={CANVAS_EXPORT_MIN_SCALE}
                  max={CANVAS_EXPORT_MAX_SCALE}
                  step={0.01}
                  value={scaleText}
                  aria-label="导出倍率"
                  onChange={(event) => inputScale(event.target.value)}
                  onBlur={commitScale}
                />
                <span className="pin-board-export-suffix">×</span>
              </div>
            </div>
            <div className="pin-board-text-field">
              <span className="pin-board-text-label"><span>宽度（像素）</span></span>
              <div className="pin-board-export-input">
                <input
                  type="number"
                  min={1}
                  step={1}
                  value={widthText}
                  aria-label="导出宽度"
                  disabled={!plan}
                  onChange={(event) => inputWidth(event.target.value)}
                  onBlur={() => {
                    if (plan) setWidthText(String(plan.width));
                  }}
                />
              </div>
            </div>
            <div className="pin-board-text-field">
              <span className="pin-board-text-label"><span>边距（像素）</span></span>
              <div className="pin-board-export-input">
                <input
                  type="number"
                  min={0}
                  max={CANVAS_EXPORT_MAX_MARGIN}
                  step={1}
                  value={marginText}
                  aria-label="导出边距"
                  onChange={(event) => inputMargin(event.target.value)}
                  onBlur={(event) => commitMargin(event.target.value)}
                />
              </div>
            </div>
          </div>
          <div className="pin-board-export-summary" aria-live="polite">
            {plan ? (
              <>
                <div>
                  <span>输出尺寸</span>
                  <strong>{plan.width} × {plan.height} 像素</strong>
                </div>
              </>
            ) : (
              <p className="pin-board-export-empty">当前画板没有可见内容，无法导出。</p>
            )}
          </div>
          {plan?.overLimit && (
            <div className="pin-board-export-warning" role="alert">
              <AlertTriangle size={15} aria-hidden="true" />
              <span>
                最长边超过建议上限 {limit} 像素（取素材板设置里的纹理缓存等级），导出会更慢。
              </span>
            </div>
          )}
          <p className="pin-board-export-hint">
            同目录已有同名文件时自动追加序号，不会覆盖。
          </p>
        </div>
        <footer className="modal-footer">
          <button className="secondary-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!ready}>
            {plan?.overLimit ? "仍要导出" : "开始导出"}
          </button>
        </footer>
      </form>
    </div>
  );
}

/**
 * 传输进度条：导入图片、导出图片与整画布导出共用同一条。
 *
 * 一行放下四件事——在做什么、走了多少、百分比、取消。取消和进度条同排，是因为用户
 * 点它的时机正是「盯着这条进度不想再等了」；不可取消的传输（加文字这类瞬时动作）
 * 不渲染按钮，避免点了没反应的假入口。
 */
function PinBoardTransferProgress({
  progress,
  onCancel,
}: {
  progress: PinBoardTransferProgress;
  onCancel: () => void;
}) {
  const total = Math.max(progress.total, 1);
  const percent = Math.min(100, Math.max(0, Math.round((progress.current / total) * 100)));
  return (
    <div className="pin-board-transfer" role="status">
      <LoaderCircle className="spin" size={14} aria-hidden="true" />
      <span className="pin-board-transfer-label">{progress.label}</span>
      <span
        className="pin-board-transfer-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-label={progress.label}
      >
        <span className="pin-board-transfer-fill" style={{ width: `${percent}%` }} />
      </span>
      <span className="pin-board-transfer-percent">{percent}%</span>
      {progress.cancellable && (
        <button type="button" className="pin-board-transfer-cancel" onClick={onCancel}>
          取消
        </button>
      )}
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
  onExportCanvas,
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
  onExportCanvas: () => void;
  onDelete: () => void;
  onToggleFullscreen: () => void;
  onClose: () => void;
}) {
  const items: ContextMenuItem[] = request.targetImage
    ? [
        { key: "copy-board", label: "复制画板条目", icon: <Copy size={16} />, onClick: onCopy },
        // 单条导出与系统剪贴板复制只认位图：选中文字或笔迹时两项一起置灰。
        { key: "copy-clipboard", label: "复制图片到剪贴板", icon: <ClipboardCopy size={16} />, disabled: request.selectedCount !== 1 || request.exportableCount === 0, disabledTitle: "只有单个位图条目能复制到系统剪贴板", onClick: onCopyClipboard },
        { key: "export", label: "导出图片", icon: <Download size={16} />, disabled: request.exportableCount === 0, disabledTitle: "只有位图条目能单条导出", onClick: onExport },
        { key: "sep-flip", divider: true },
        { key: "flip-h", label: "水平翻转", icon: <FlipHorizontal2 size={16} />, onClick: () => renderer.flipSelected(true) },
        { key: "flip-v", label: "垂直翻转", icon: <FlipVertical2 size={16} />, onClick: () => renderer.flipSelected(false) },
        { key: "arrange", label: "阵列排序", icon: <LayoutGrid size={16} />, disabled: request.arrangeableCount < 2, onClick: () => void renderer.arrangeSelected() },
        { key: "sep-layer", divider: true },
        { key: "layer-label", heading: true, label: "图层", icon: <Layers3 size={15} /> },
        { key: "layer-top", label: "顶层", icon: <span className="pin-board-layer-swatch top" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(2) },
        { key: "layer-middle", label: "中层", icon: <span className="pin-board-layer-swatch middle" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(1) },
        { key: "layer-bottom", label: "底层", icon: <span className="pin-board-layer-swatch bottom" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(0) },
        { key: "sep-delete", divider: true },
        { key: "delete", label: "删除图片", icon: <Trash2 size={16} />, danger: true, onClick: onDelete },
      ]
    : [
        { key: "paste", label: "粘贴画板条目", icon: <ClipboardPaste size={16} />, disabled: !request.canEditBoard || !canPaste, onClick: onPaste },
        { key: "import", label: "导入图片", icon: <Upload size={16} />, disabled: !request.canEditBoard, onClick: onImport },
        { key: "text", label: "添加文字", icon: <Type size={16} />, disabled: !request.canEditBoard, onClick: onAddText },
        { key: "export-canvas", label: "导出画布", icon: <ImageDown size={16} />, disabled: !request.canEditBoard, disabledTitle: "画板已锁定，无法导出", onClick: onExportCanvas },
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
  } = usePinBoardController({ artworkId, active, settings });
  /**
   * 编辑入口的统一禁用条件：锁定与整画布导出期间都不允许改动画板。导出渲染的是
   * 「点下那一刻的画面」，中途改动会让输出与用户看到的对不上。
   */
  const editingBlocked = interaction.locked || interaction.exporting;
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
  const [exportDialogOpen, setExportDialogOpen] = useState(false);
  const [textOptions, setTextOptions] = useState({ fontSize: 48, color: "#000000" });
  /** 标注样式：工具栏的取色器与「大小」输入共同决定下一笔的外观（大小同时是橡皮直径）。 */
  const [strokeStyle, setStrokeStyle] = useState({
    color: DEFAULT_STROKE_COLOR,
    width: DEFAULT_STROKE_WIDTH,
  });
  const [strokeWidthText, setStrokeWidthText] = useState(String(DEFAULT_STROKE_WIDTH));
  /** 标注工具条的显隐，只由快捷键 `T` 切换。 */
  const [drawbarVisible, setDrawbarVisible] = useState(true);
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

  // 绘制样式推给渲染器：预览层与落库都用渲染器里那一份，避免两处各存一个值。
  useEffect(() => {
    renderer?.setStrokeStyle(strokeStyle.color, strokeStyle.width);
  }, [renderer, strokeStyle]);

  /** 滑条直接写入「大小」（内部即线宽）：非法值忽略，合法值取整并夹到前后端共同接受的范围。 */
  const applyStrokeWidth = useCallback((width: number) => {
    if (!Number.isFinite(width)) return;
    const clamped = clampStrokeWidth(Math.round(width));
    setStrokeWidthText(String(clamped));
    setStrokeStyle((current) => (
      current.width === clamped ? current : { ...current, width: clamped }
    ));
  }, []);

  /** 「大小」输入在失焦或回车时收敛到合法范围，输入过程中允许暂时不合法。 */
  const commitStrokeWidth = useCallback((raw: string) => {
    if (raw.trim() === "" || !Number.isFinite(Number(raw))) {
      setStrokeWidthText(String(strokeStyle.width));
      return;
    }
    applyStrokeWidth(Number(raw));
  }, [applyStrokeWidth, strokeStyle.width]);

  const toggleDrawbar = useCallback(() => {
    if (!renderer || !view || unavailable !== null) return;
    setDrawbarVisible((visible) => !visible);
  }, [renderer, view, unavailable]);

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
    if (renderer && !renderer.interactionState.locked && !renderer.interactionState.exporting) {
      setDropActive(true);
    }
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
      if (!active || !shortcutCanHandle(event)) return;
      // 单键开关：带修饰键的组合留给应用与系统，不与工具键位抢。
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      if (event.key.toLowerCase() !== PIN_BOARD_TOOLBAR_SHORTCUT.toLowerCase()) return;
      if (!renderer || !view || unavailable !== null) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      toggleDrawbar();
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, renderer, view, unavailable, toggleDrawbar]);

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
        pasteItems();
      }
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, clipboardCount, copyImages, pasteItems]);

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
                    draggable={boards.length > 1 && transferProgress === null && !interaction.exporting}
                    disabled={interaction.exporting}
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
      <div
        ref={workspaceRef}
        className={`pin-board-workspace${fullscreen ? " fullscreen" : ""}`}
      >
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
            <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.zoomOut())} title="缩小视图" aria-label="缩小视图"><ZoomOut size={17} /></button>
            <span className="pin-board-zoom">{interaction.zoomPercent}%</span>
            <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.zoomIn())} title="放大视图" aria-label="放大视图"><ZoomIn size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.resetViewport())} title="重置视图" aria-label="重置视图"><LocateFixed size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.scaleSelected(1 / 1.1))} title="缩小图片" aria-label="缩小图片"><Shrink size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.scaleSelected(1.1))} title="放大图片" aria-label="放大图片"><Expand size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.rotateSelected(-5))} title="逆时针旋转" aria-label="逆时针旋转"><RotateCcw size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.rotateSelected(5))} title="顺时针旋转" aria-label="顺时针旋转"><RotateCw size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.flipSelected(true))} title="水平翻转" aria-label="水平翻转"><FlipHorizontal2 size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.flipSelected(false))} title="垂直翻转" aria-label="垂直翻转"><FlipVertical2 size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.arrangeableCount < 2 || editingBlocked} onClick={() => run(() => void renderer?.arrangeSelected())} title="阵列排序" aria-label="阵列排序"><LayoutGrid size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={copyImages} title="复制画板条目" aria-label="复制画板条目"><Copy size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.selectedCount !== 1 || interaction.exportableCount === 0 || editingBlocked} onClick={() => void copyImageToClipboard()} title="复制图片到剪贴板" aria-label="复制图片到剪贴板"><ClipboardCopy size={17} /></button>
            <button className="icon-button" disabled={!renderer || clipboardCount === 0 || editingBlocked} onClick={pasteItems} title="粘贴画板条目" aria-label="粘贴画板条目"><ClipboardPaste size={17} /></button>
            <button className="icon-button" disabled={!renderer || editingBlocked || transferProgress !== null} onClick={() => void importImages()} title="导入图片" aria-label="导入图片"><Upload size={17} /></button>
            <button className="icon-button" disabled={!renderer || editingBlocked || transferProgress !== null} onClick={openTextDialog} title="添加文字" aria-label="添加文字"><Type size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.exportableCount === 0 || editingBlocked || transferProgress !== null} onClick={() => void exportImages()} title="导出图片" aria-label="导出图片"><Download size={17} /></button>
            <button className="icon-button" disabled={!renderer || interaction.imageCount === 0 || editingBlocked || transferProgress !== null} onClick={() => setExportDialogOpen(true)} title="导出画布 PNG" aria-label="导出画布 PNG"><ImageDown size={17} /></button>
            <button className={`icon-button${interaction.locked ? " active" : ""}`} disabled={!renderer || interaction.exporting} onClick={toggleLock} title={actionTitle(interaction.locked ? "解锁画板" : "锁定画板", settings.lockShortcut)} aria-label={interaction.locked ? "解锁画板" : "锁定画板"}>{interaction.locked ? <Lock size={17} /> : <Unlock size={17} />}</button>
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
          <PinBoardTransferProgress progress={transferProgress} onCancel={cancelTransfer} />
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
            onStrokeCommit={commitStroke}
          />
          {view && !unavailable && (
            <PinBoardLayersPanel
              visibility={interaction.visibleLayers}
              counts={interaction.layerCounts}
              onToggle={(layer, visible) => run(() => renderer?.setLayerVisible(layer, visible))}
              disabled={!renderer || editingBlocked}
            />
          )}
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
          {drawbarVisible && view && !unavailable && (
            <div className="pin-board-drawbar" role="toolbar" aria-label="标注工具">
              <StrokeToolGroup
                tool={interaction.tool}
                disabled={!renderer || editingBlocked}
                onSelect={setTool}
              />
              <span className="pin-board-divider" />
              <StrokeStyleControls
                color={strokeStyle.color}
                width={strokeStyle.width}
                widthText={strokeWidthText}
                onColorChange={(color) => setStrokeStyle((current) => ({ ...current, color }))}
                onWidthInput={applyStrokeWidth}
                onWidthTextChange={setStrokeWidthText}
                onWidthCommit={commitStrokeWidth}
              />
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
        {exportDialogOpen && (
          <PinBoardExportDialog
            limit={canvasExportLimit}
            boardName={title}
            planFor={canvasExportPlan}
            autoScale={canvasExportAutoScale}
            onStart={(request) => {
              setExportDialogOpen(false);
              void exportCanvas(request);
            }}
            onClose={() => setExportDialogOpen(false)}
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
            onPaste={pasteItems}
            onImport={() => void importImages()}
            onAddText={openTextDialog}
            onExport={() => void exportImages()}
            onExportCanvas={() => {
              setContextMenu(null);
              setExportDialogOpen(true);
            }}
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
