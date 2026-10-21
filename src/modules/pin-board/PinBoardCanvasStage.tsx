import {
  AlertTriangle,
  Circle,
  Eraser,
  Images,
  LoaderCircle,
  Lock,
  MousePointer2,
  MoveUpRight,
  Pencil,
  Slash,
  Square,
  Upload,
  type LucideIcon,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import type { DragEvent as ReactDragEvent } from "react";
import { errorMessage } from "../../shared/errors";
import { diagnosticsLog } from "../../shared/diagnostics";
import { getBoardSession, setBoardSession } from "./session";
import { PinBoardRenderer } from "./renderer";
import type {
  PinBoardContextMenuRequest,
  PinBoardInteractionState,
} from "./renderer";
import type {
  PinBoardModuleSettings,
  PinBoardSummary,
  PinBoardView,
} from "./types";
import { textureBudgetsForLevel, type PinBoardTextureBudgets } from "./texturePolicy";
import { PinBoardLayersPanel } from "./PinBoardLayersPanel";
import { PinBoardColorPicker } from "./PinBoardColorPicker";
import {
  MAX_STROKE_WIDTH,
  MIN_STROKE_WIDTH,
  PIN_BOARD_TOOL_META,
  STROKE_WIDTH_SLIDER_STEPS,
  strokeWidthFromSlider,
  strokeWidthToSlider,
  type PinBoardTool,
  type StrokeCommitRequest,
} from "./strokeDrawing";
import { installStrokePerfProbe } from "./strokePerfProbe";

/**
 * 标注工具条的图标。标签、快捷键与顺序来自 `PIN_BOARD_TOOL_META`，这张表只补图标；
 * 新增工具时两处都要补（`Record` 只保证图标表不漏 `PinBoardTool` 的取值）。
 */
const TOOL_ICONS: Record<PinBoardTool, LucideIcon> = {
  select: MousePointer2,
  pen: Pencil,
  eraser: Eraser,
  line: Slash,
  arrow: MoveUpRight,
  rect: Square,
  ellipse: Circle,
};

/** 工具按钮组：工具选择即交互模式，没有额外的模式开关。 */
function StrokeToolGroup({ tool, disabled, onSelect }: {
  tool: PinBoardTool;
  disabled: boolean;
  onSelect: (tool: PinBoardTool) => void;
}) {
  return (
    <div className="pin-board-tools" role="group" aria-label="工具">
      {PIN_BOARD_TOOL_META.map(({ tool: value, label, shortcut }) => {
        const Icon = TOOL_ICONS[value];
        return (
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
        );
      })}
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

export interface PinBoardCanvasStageProps {
  active: boolean;
  artworkId: string;
  view: PinBoardView | null;
  boards: PinBoardSummary[] | null;
  renderer: PinBoardRenderer | null;
  interaction: PinBoardInteractionState;
  settings: PinBoardModuleSettings;
  /** 渲染器创建失败时的常驻原因；null 表示可用或尚未尝试。 */
  unavailable: string | null;
  unavailableNotice: string;
  lockIndicatorVisible: boolean;
  layersDisabled: boolean;
  drawbarVisible: boolean;
  strokeStyle: { color: string; width: number };
  strokeWidthText: string;
  setStatus: (message: string | null) => void;
  onUnavailable: (reason: string | null) => void;
  onRenderer: (renderer: PinBoardRenderer | null) => void;
  onState: (state: PinBoardInteractionState) => void;
  onContextMenu: (request: PinBoardContextMenuRequest | null) => void;
  onStrokeCommit: (request: StrokeCommitRequest) => void;
  onImportDropped: (files: FileList, clientX: number, clientY: number) => Promise<void>;
  onSelectTool: (tool: PinBoardTool) => void;
  onToggleLayer: (layer: number, visible: boolean) => void;
  onStrokeColorChange: (color: string) => void;
  onStrokeWidthInput: (width: number) => void;
  onStrokeWidthTextChange: (raw: string) => void;
  onStrokeWidthCommit: (raw: string) => void;
}

/** 画布舞台：GPU 画布、占位/锁定/投放提示、标注工具条与图层面板挂载。 */
export function PinBoardCanvasStage({
  active,
  artworkId,
  view,
  boards,
  renderer,
  interaction,
  settings,
  unavailable,
  unavailableNotice,
  lockIndicatorVisible,
  layersDisabled,
  drawbarVisible,
  strokeStyle,
  strokeWidthText,
  setStatus,
  onUnavailable,
  onRenderer,
  onState,
  onContextMenu,
  onStrokeCommit,
  onImportDropped,
  onSelectTool,
  onToggleLayer,
  onStrokeColorChange,
  onStrokeWidthInput,
  onStrokeWidthTextChange,
  onStrokeWidthCommit,
}: PinBoardCanvasStageProps) {
  /** 拖入文件悬停在画布上时显示投放提示。 */
  const [dropActive, setDropActive] = useState(false);
  const editingBlocked = interaction.locked || interaction.exporting;

  useEffect(() => {
    if (!active) setDropActive(false);
  }, [active]);

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
    void onImportDropped(event.dataTransfer.files, event.clientX, event.clientY);
  }, [onImportDropped]);

  return (
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
        onUnavailable={onUnavailable}
        onRenderer={onRenderer}
        onState={onState}
        onContextMenu={onContextMenu}
        onStrokeCommit={onStrokeCommit}
      />
      {view && !unavailable && (
        <PinBoardLayersPanel
          visibility={interaction.visibleLayers}
          counts={interaction.layerCounts}
          onToggle={onToggleLayer}
          disabled={layersDisabled}
        />
      )}
      {unavailable ? (
        <div className="pin-board-stage-overlay pin-board-unavailable" role="alert">
          <div className="pin-board-stage-hint">
            <AlertTriangle size={22} />
            <strong>素材板不可用</strong>
            <span>{unavailableNotice}</span>
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
      {lockIndicatorVisible && (
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
            onSelect={onSelectTool}
          />
          <span className="pin-board-divider" />
          <StrokeStyleControls
            color={strokeStyle.color}
            width={strokeStyle.width}
            widthText={strokeWidthText}
            onColorChange={onStrokeColorChange}
            onWidthInput={onStrokeWidthInput}
            onWidthTextChange={onStrokeWidthTextChange}
            onWidthCommit={onStrokeWidthCommit}
          />
        </div>
      )}
    </div>
  );
}
