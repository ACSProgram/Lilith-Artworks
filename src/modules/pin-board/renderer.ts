/**
 * 素材板渲染器：画板装载、屏幕绘制、选中/变换/绘制/擦除的编辑状态机与保存结算。
 *
 * 这里只管「这一帧画什么」和「用户正在改什么」：设备、管线与纹理上传在 `rendererGpu.ts`，
 * 绘制预览的 DOM 叠加层在 `strokePreview.ts`，整画布导出的离屏目标、回读与导出笔迹缓冲在
 * `rendererExport.ts`，笔迹的几何构建与缓冲打包在 `strokeRender.ts`。
 */

import { errorMessage } from "../../shared/errors";
import { diagnosticsLog, diagnosticsSlowStep } from "../../shared/diagnostics";
import { pinBoardApi } from "./api";
import {
  recordBoardRevision,
  resolveBoardRevision,
  stageBoardSettle,
  takeBoardSettle,
} from "./pendingSettle";
import type {
  PinBoardArrangeResult,
  PinBoardClipboardItem,
  PinBoardEditableItem,
  PinBoardItem,
  PinBoardStrokeItem,
  PinBoardTextAlign,
  PinBoardView,
  SavePinBoardResult,
} from "./types";
import { parseStrokeBatch, type PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  DEFAULT_STROKE_COLOR,
  DEFAULT_STROKE_WIDTH,
  MIN_STROKE_DRAG_CSS_PIXELS,
  beginStrokeDraft,
  clampStrokeWidth,
  extendStrokeDraft,
  isDrawingTool,
  isStrokeTool,
  resolveStrokeDraft,
  toolForShortcut,
  type PinBoardTool,
  type StrokeCommitRequest,
  type StrokeDraft,
} from "./strokeDrawing";
import {
  STROKE_HIT_TOLERANCE_CSS_PIXELS,
  buildStrokeWorldOutlines,
  strokeHitBounds,
  strokeHitByCircle,
  strokeHitBySweep,
  strokeIntersectsBounds,
  strokeOutlineSignature,
  type StrokeWorldOutline,
} from "./strokeHit";
import {
  STROKE_OUTLINE_VERTICES,
  buildStrokeBatch,
  buildStrokeOutlines,
  type StrokeBatch,
} from "./strokeRender";
import {
  DEFAULT_VIEWPORT,
  fitViewportToBounds,
  imageBounds,
  imageDisplayMaxDimension,
  imageSize,
  imagesBounds,
  imagesCenter,
  intersects,
  pointInQuad,
  quadCenter,
  quadIntersectsBounds,
  rotateQuad,
  scaleQuad,
  screenToWorld,
  snapRotationDelta,
  translateQuad,
  viewportBounds,
  worldToScreen,
  type Bounds,
  type Point,
  type Quad,
  type Viewport,
} from "./geometry";
import type { PinBoardViewSession } from "./session";
import { isAdditiveSelectionModifier, marqueeSelection, sameSelection } from "./selection";
import {
  DEFAULT_LAYER_VISIBILITY,
  PIN_BOARD_LAYER_COUNT,
  STROKE_LAYER,
  hiddenSelectionIds,
  isLayerVisible,
  normalizeLayerVisibility,
  type PinBoardLayerVisibility,
} from "./layers";
import { shortcutCanHandle } from "./shortcuts";
import {
  CANVAS_EXPORT_TILE_MAX_WIDTH,
  autoCanvasExportScale,
  canvasExportTiles,
  resolveCanvasExportPlan,
  type CanvasExportPlan,
  type CanvasExportTile,
} from "./exportPolicy";
import {
  PIN_BOARD_CANVAS_DIMENSION_LIMIT,
  PIN_BOARD_MIN_PREVIEW_DIMENSION,
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_TEXTURE_LOAD_BUDGET_BYTES,
  PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES,
  requestedTextureDimension,
  textureBudgetsForLevel,
  type PinBoardTextureBudgets,
} from "./texturePolicy";
import { requestedTextTextureDimension } from "./textLayout";
import {
  pinBoardTextCaretIndexAtPoint,
  pinBoardTextEditGeometry,
  pinBoardTextScale,
  pinBoardTextSignature,
  resolveNewPinBoardTextDraft,
  resolvePinBoardTextDraft,
  textDraftFromItem,
  textQuadAngle,
  type PinBoardNewTextResult,
  type PinBoardTextDraft,
  type PinBoardTextScale,
} from "./textEditing";
import {
  adjustLiveRenderers,
  clearColor,
  destroyTexture,
  estimatedLoadBytes,
  estimatedResidentBytes,
  loadTextTexture,
  loadTexture,
  makeGpu,
  outlineData,
  releaseCanvasGpu,
  vertexData,
  type GpuState,
  type LoadedTexture,
} from "./rendererGpu";
import {
  buildExportStrokeBuffers,
  createExportTexture,
  destroyExportStrokeBuffers,
  exportTextureDimension,
  exportTileBounds,
  readExportTile,
  type ExportStrokeBuffers,
} from "./rendererExport";
import {
  clearStrokePreview as clearStrokePreviewLayer,
  coalescedStrokeSamples,
  drawStrokePreview as paintStrokePreview,
} from "./strokePreview";

/** 结算遇到 `repository_busy` 时的重试间隔。 */
const FINALIZE_BUSY_RETRY_DELAY_MS = 300;

/**
 * 结算失败是否是「仓库正在写入」。
 *
 * 素材板命令仍以字符串错误拒绝，因此同时接受结构化 `code` 与两种用户文案
 * （同进程排队超时 / 另一个进程写入）。
 */
function isRepositoryBusy(error: unknown): boolean {
  if ((error as { code?: unknown } | null)?.code === "repository_busy") return true;
  const message = errorMessage(error);
  return message.includes("仓库正在写入") || message.includes("另一个进程写入");
}

type TextureLoadCandidate = {
  image: PinBoardItem;
  maxDimension: number;
  estimatedBytes: number;
  priority: number;
  /** 常驻缩略图候选（写入 previewTextures，不参与常规预算淘汰）。 */
  isPreview?: boolean;
};

type ImageSnapshot = {
  itemId: number;
  deleted: boolean;
  layer: number;
  points: Quad;
  uv: Quad;
  /** 文字条目的载荷与度量；位图/笔迹为 undefined。 */
  text?: {
    text: string;
    fontSize: number;
    color: string;
    align: PinBoardTextAlign;
    width: number;
    height: number;
  };
};

/** 就地编辑会话的屏幕状态；React 编辑器据此定位并渲染字体栏。 */
export interface PinBoardTextEditState {
  /** 每次进入编辑自增；编辑器用它作为组件 key，草稿不会跨会话串台。 */
  sessionId: number;
  /** 新建会话还没有条目 id；提交时才由创建命令分配。 */
  itemId: number | null;
  /** 正在新建文字：提交走 `create_pin_board_text`，不经过保存载荷。 */
  isNew: boolean;
  draft: PinBoardTextDraft;
  /** 进入编辑时的光标位置（UTF-16 偏移）；null 表示放在末尾。 */
  caret: number | null;
  /** 条目缩放（四边形宽 ÷ 测量宽）。有效字号 = `draft.fontSize × scale`。 */
  scale: number;
  centerX: number;
  centerY: number;
  /** 屏幕旋转角（CSS 像素、y 向下）。 */
  angle: number;
  /** CSS 像素 / 世界单位：随画布缩放与窗口尺寸变化。 */
  pixelsPerWorldUnit: number;
}

type HistoryEntry = {
  before: ImageSnapshot[];
  after: ImageSnapshot[];
  mergeKey?: string;
  timestamp: number;
};

export interface PinBoardInteractionState {
  /** 画板内未删除的条目数，覆盖全部 kind（位图 / 文字 / 笔迹）。 */
  itemCount: number;
  selectedCount: number;
  /** 选中里可参与阵列排序的位图数量（文字条目不参与）。 */
  arrangeableCount: number;
  /**
   * 选中里可导出的**位图**数量。文字与笔迹没有单条导出入口（它们由整画布导出覆盖），
   * 因此这个值同时决定「导出图片」按钮与右键项的可用性。
   */
  exportableCount: number;
  /** 每层未删除条目数（下标即 layer，含标注层；隐藏层里的条目照样计入）。 */
  layerCounts: number[];
  canUndo: boolean;
  canRedo: boolean;
  dirty: boolean;
  saving: boolean;
  locked: boolean;
  zoomPercent: number;
  /** 当前工具：`select` 走选中/变换，其余工具独占指针（绘制或擦除）。 */
  tool: PinBoardTool;
  /** 图层可见性（下标 = layer）：界面据此渲染四个开关。 */
  visibleLayers: PinBoardLayerVisibility;
  /** 整画布导出进行中：期间画板锁定，绘制、图层、切换画板等入口一律禁用。 */
  exporting: boolean;
  /** 正在就地编辑的文字；null 表示没有编辑会话。 */
  textEditing: PinBoardTextEditState | null;
  /** 文字放置模式：画布光标为 I 字型，左键落点新建文字，右键或 Escape 取消。 */
  placingText: boolean;
}

/** 未装载画板或没有渲染器时的交互状态：锁定、无选中、工具为选择。 */
export const DEFAULT_PIN_BOARD_INTERACTION: PinBoardInteractionState = {
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
  textEditing: null,
  placingText: false,
};

export interface PinBoardContextMenuRequest {
  clientX: number;
  clientY: number;
  selectedCount: number;
  arrangeableCount: number;
  /** 选中里可导出的位图数量：0 时「导出图片」与「复制到剪贴板」置灰。 */
  exportableCount: number;
  /** 选中里含笔迹：图层菜单据此置灰（笔迹固定在标注层，不能被改层）。 */
  selectionHasStroke: boolean;
  /** 恰好选中一条文字：右键菜单据此提供“编辑文字”。 */
  singleTextSelected: boolean;
  targetImage: boolean;
  canEditBoard: boolean;
  canEditSelection: boolean;
}

const TEXTURE_LOAD_CONCURRENCY = 2;
const VIEWPORT_LOAD_DELAY_MS = 90;
const MIN_VIEWPORT_HEIGHT = 20;
const MAX_VIEWPORT_HEIGHT = 10_000_000;
/** 画布小于该 CSS 尺寸时不做打开适配，等布局稳定后由 `resize` 补做。 */
const VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS = 32;
const HISTORY_LIMIT = 100;
const HISTORY_MERGE_MS = 350;
/** 模型变化后静默该时长即自动保存；期间再次变化会重新计时。 */
const AUTOSAVE_DELAY_MS = 1500;
/**
 * 设备丢失后的自动重建上限与退避。上限是整个渲染器生命周期（≈ 一次会话）内
 * 允许重建的设备数，用于避免「丢设备 → 重建 → 又丢」退化成重建风暴——那正是
 * 最初观察到的冻结触发条件（短时间连续创建设备）。超限或重建失败即回到「只上报」。
 */
const GPU_RECOVERY_MAX_ATTEMPTS = 2;
const GPU_RECOVERY_BACKOFF_MS = 600;
const TRANSFORM_HANDLE_RADIUS = 12;
const ROTATION_HANDLE_OFFSET = 28;
/** 第二次点击文字时，按下到抬起的位移超过该值仍按拖动处理。 */
const TEXT_EDIT_CLICK_TOLERANCE_CSS_PIXELS = 3;
/** 屏幕字号低于该值时，进入编辑会自动放大视图到可读尺寸。 */
const TEXT_EDIT_READABLE_MIN_CSS_PIXELS = 10;
const TEXT_EDIT_READABLE_TARGET_CSS_PIXELS = 20;
/** 橡皮环形光标的视觉最小直径（CSS 像素）：再小就看不见了。 */
const ERASER_RING_MIN_CSS_PIXELS = 4;
/**
 * 笔迹缓冲的超扫边距（视口宽高的比例）：四周各扩这么多，平移在这个范围内既不改
 * 可见集合、也不改缓冲签名，因此拖动画布不重建；逐段裁剪用同一个框，边距内平移
 * 也不会露出被裁掉的段。
 */
const STROKE_OVERSCAN_RATIO = 0.5;

/** 指针拖拽的进行中状态；`draw` 是绘制工具下的那一支，`erase` 是橡皮。 */
type DragMode = "pan" | "images" | "marquee" | "rotate" | "scale" | "draw" | "erase";
/** 每种拖拽状态对应的画布光标类名。 */
const DRAG_CURSOR_CLASS: Record<DragMode, string> = {
  pan: "dragging",
  images: "moving-image",
  marquee: "selecting",
  rotate: "rotating-image",
  scale: "scaling-image",
  draw: "drawing",
  erase: "erasing",
};

function cloneQuad(quad: Quad): Quad {
  return quad.map((point) => [...point] as Point) as Quad;
}

function cloneImage(image: PinBoardItem): PinBoardItem {
  return { ...image, points: cloneQuad(image.points), uv: cloneQuad(image.uv) };
}

export class PinBoardRenderer {
  private boardId = 0;
  private readonly viewport: Viewport = { ...DEFAULT_VIEWPORT };
  private images: PinBoardItem[] = [];
  private readonly imageById = new Map<number, PinBoardItem>();
  private readonly textures = new Map<number, LoadedTexture>();
  /** 常驻缩略图缓存：每个访问过的图片保留一张低分辨率预览，滑动视图时
   *  高清纹理被淘汰后仍可立即绘制，避免闪烁。只在独立预览预算超限时
   *  按「不可见优先 + LRU」淘汰，模块销毁时整体释放。 */
  private readonly previewTextures = new Map<number, LoadedTexture>();
  /** 按目标维度记录加载失败的图片，缩略图（256）失败不会阻止高清重试，
   *  高清失败也不会阻止缩略图兜底。 */
  private readonly failedTextureDimensions = new Map<number, number>();
  /** 高清纹理 LRU 预算（随缓存等级配置变化，见 `setTextureCacheBudgets`）。 */
  private textureBudgetBytes: number;
  /** 常驻缩略图预算（随缓存等级配置变化）。 */
  private previewTextureBudgetBytes: number;
  private readonly textureQualityCaps = new Map<number, number>();
  private readonly selectedIds = new Set<number>();
  private readonly resizeObserver: ResizeObserver;
  private readonly themeObserver: MutationObserver;
  private readonly history: HistoryEntry[] = [];
  private visibleImages: PinBoardItem[] = [];
  private historyIndex = 0;
  private savedStateKey = "";
  private revision = "";
  private cssHeight = 0;
  private defaultWorldUnitsPerCssPixel = 1;
  private loadTimer: number | null = null;
  private loadRequested = false;
  private loading = false;
  private textureReservedBytes = 0;
  /** 已发起但尚未完成的纹理读取；用于销毁日志确认旧任务是否越过生命周期边界。 */
  private inFlightTextureLoads = 0;
  private generation = 0;
  private preservedWorldUnitsPerCssPixel: number | null = null;
  /** 打开画板时按图片最小包围框定位视图，等画布尺寸可用后再执行。 */
  private pendingViewportFit = false;
  /** 是否已装载画板。未装载时所有画板级绘制/保存/会话都停用，但 GPU 设备保持存活。 */
  private loaded = false;
  /** 设备丢失恢复：已尝试重建次数（不重置，作为整会话上限）。 */
  private gpuRecoveryAttempts = 0;
  /** GPU 不可用（丢失后正在重建，或已达重建上限而放弃）：期间暂停绘制与纹理加载。 */
  private gpuUnavailable = false;
  private recoveryTimer: number | null = null;
  private destroyed = false;
  private saving = false;
  private finalized = false;
  private active: boolean;
  private dirty = false;
  private autosaveTimer: number | null = null;
  private savePromise: Promise<boolean> | null = null;
  private finalizePromise: Promise<boolean> | null = null;
  private locked = false;
  private dragMode: DragMode | null = null;
  private dragPointerId: number | null = null;
  private pointerX = 0;
  private pointerY = 0;
  private cursorWorld: Point | null = null;
  private dragWorld: Point = [0, 0];
  private marqueeWorld: Point = [0, 0];
  private readonly marqueeBaseIds = new Set<number>();
  private rotateCenter: Point = [0, 0];
  private rotateStartAngle = 0;
  private scaleAnchor: Point = [0, 0];
  private scaleStartDistance = 1;
  private dragBefore: ImageSnapshot[] = [];
  /** 每条文字上一次成功保存的签名：普通保存只发送签名变化的文字载荷。 */
  private readonly savedTextSignatures = new Map<number, string>();
  /** 就地编辑会话；草稿由 React 编辑器推入，渲染器负责模型与提交。 */
  private textEdit: {
    sessionId: number;
    /** 新建会话为 null，提交时由创建命令分配条目 id。 */
    itemId: number | null;
    isNew: boolean;
    before: ImageSnapshot | null;
    draft: PinBoardTextDraft;
    scale: PinBoardTextScale;
    center: Point;
    angle: number;
    caret: number | null;
  } | null = null;
  private textEditSessionId = 0;
  /** 上次提交的新文字样式：下一次新建沿用，首次为 48 / 黑 / 居中。 */
  private textDefaults: PinBoardTextDraft = {
    text: "",
    fontSize: 48,
    color: "#000000",
    align: "center",
  };
  /** 文字放置模式：左键落点新建，右键或 Escape 取消。 */
  private placingText = false;
  /** 提交后抑制紧随其后的 pointerup 再次进入编辑（点击同一条文字时会发生）。 */
  private suppressClickEdit = false;
  /** 本次按下的起点与拖动判定：第二次点击同一文字且没有拖动才进入编辑。 */
  private pressClient: Point | null = null;
  private pressMoved = false;
  private pressWasSelected = false;
  private pressItemId: number | null = null;
  private pressSuppressEdit = false;
  private selectionScreenQuad: Quad | null = null;
  /**
   * 纹理未就绪的图片占位层：DOM 节点按 itemId 复用，避免每帧增删；位置随
   * 视口/图片旋转同步。纹理（缩略图或高清）到位后由下一次 `draw()` 移除。
   */
  private readonly placeholderNodes = new Map<number, HTMLElement>();
  private drawErrorReported = false;
  /** 当前画板的笔迹几何，按 itemId 索引；几何只走二进制通道，不进条目 DTO。 */
  private readonly strokeGeometries = new Map<number, PinBoardStrokeGeometry>();
  /** 笔迹缓冲的失效计数：几何或变换变化时递增，签名随之变化并触发重建。 */
  private strokeVersion = 0;
  /** 上次构建笔迹缓冲时的「可见笔迹 + 缩放」签名，避免平移时逐帧重建。 */
  private strokeSignature = "";
  /**
   * 当前笔迹缓冲是按哪个「视口 + 超扫边距」框构建的。视口一旦走出它，被逐段
   * 裁掉的段就可能露出来；即使可见条目没变也必须重建。
   */
  private strokeBuiltBounds: Bounds | null = null;
  /** 笔迹读取的代次：切画板或重复读取时丢弃旧结果。 */
  private strokeLoadToken = 0;
  private strokeVertexBuffer: any = null;
  private strokeIndexBuffer: any = null;
  private strokeIndexCount = 0;
  /** 可见笔迹的选中描边，按 `strokeOutlineIndex` 给出的槽位连续排布。 */
  private strokeOutlineBuffer: any = null;
  private readonly strokeOutlineIndex = new Map<number, number>();
  /** 当前工具：绘制或擦除工具下指针按下独占该次拖拽，不触发选中/框选。 */
  private tool: PinBoardTool = "select";
  /**
   * 图层可见性：纯视图状态，随画板会话恢复，不写库也不标记 dirty。
   * 所有「哪些条目可画、可选、可点」的判断都经过它，少接一处就会出现
   * 「看不见但能选中」。
   */
  private visibleLayers: PinBoardLayerVisibility = normalizeLayerVisibility(DEFAULT_LAYER_VISIBILITY);
  /** 绘制样式由工具栏推入，落库与预览共用同一份值。 */
  private strokeColor = DEFAULT_STROKE_COLOR;
  private strokeWidthValue = DEFAULT_STROKE_WIDTH;
  /** 正在进行的绘制草稿（世界坐标）；`null` 表示没有绘制。 */
  private draft: StrokeDraft | null = null;
  /**
   * 橡皮擦的环形光标。CSS 的 `cursor` 只能给固定尺寸的图，直径要随「大小」与缩放
   * 变化，因此用一个 DOM 空心圆跟随指针；`pointer-events: none`，不挡命中。
   */
  private readonly eraserRing: HTMLElement;
  /** 指针最后一次落在画布上的位置（CSS 像素，相对画布左上角）；离开画布即清空。 */
  private eraserPointer: Point | null = null;
  /** 世界折线缓存：键是条目 id，签名见 `strokeOutlineSignature`。 */
  private readonly strokeOutlineCache = new Map<
    number,
    { signature: string; outlines: StrokeWorldOutline[] }
  >();
  /** 本轮擦除已经删掉的条目：同一笔不重复处理，也用于判断「这轮有没有改动」。 */
  private readonly eraseHitIds = new Set<number>();
  /**
   * 本轮擦除的候选（未删除、所在图层可见的笔迹）。按下时算一次就够：拖拽中整板条目
   * 不会增减，逐帧重新过滤整板只是白白分配数组。
   */
  private eraseCandidates: PinBoardStrokeItem[] = [];
  /**
   * 上一帧的擦头位置。擦除按「这一点到上一点」扫过的区域判，而不是只判当前点：
   * 指针事件会被合并到 60–120 Hz，只测采样点会让快速擦动跳过细笔迹。
   */
  private eraseFrom: Point | null = null;
  /**
   * 整画布导出进行中。期间指针、滚轮与快捷键一律不响应，界面按
   * `interactionState.exporting` 禁用入口——导出渲染的是「点下那一刻的画面」，
   * 中途改模型会让输出与用户看到的对不上。
   */
  private exporting = false;
  /** 导出期间按导出区域单独构建的笔迹缓冲，结束即释放（不碰屏幕那份）；无笔迹时为 null。 */
  private exportStrokeBuffers: ExportStrokeBuffers | null = null;
  /** 导出要画的条目：未删除、可用、所在图层可见，按 layer 再按 order（与屏幕同序）。 */
  private exportDrawItems: PinBoardItem[] = [];
  /**
   * 导出当前这一块要用的条目：淘汰时跳过它们。
   *
   * 导出区域的条目往往不在屏幕视口内，而淘汰优先挑「不可见」的——不加这层保护，
   * 为这一块加载另一张图时会把这一块刚准备好的纹理淘汰掉，导出的那块就会出现空白。
   */
  private exportProtectedIds: Set<number> | null = null;

  private constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly selectionElement: HTMLElement,
    private readonly marqueeElement: HTMLElement,
    private readonly placeholderElement: HTMLElement,
    /** 绘制预览层：DOM 2D canvas，只画当前草稿，不进 GPU 缓冲。 */
    private readonly drawOverlay: HTMLCanvasElement,
    private gpu: GpuState,
    private arrangementGapRatio: number,
    private autosaveEnabled: boolean,
    private readonly onError: (message: string) => void,
    private readonly onState: (state: PinBoardInteractionState) => void,
    private readonly onSessionChange: (boardId: number, session: PinBoardViewSession) => void,
    private readonly onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    private readonly onStrokeCommit: (request: StrokeCommitRequest) => void,
    cacheBudgets: PinBoardTextureBudgets,
  ) {
    // 构造函数只负责画布生命周期内一次性初始化；画板数据由 `loadBoard` 装载。
    this.active = true;
    this.textureBudgetBytes = cacheBudgets.residentBytes;
    this.previewTextureBudgetBytes = cacheBudgets.previewBytes;
    const cssHeight = Math.max(canvas.getBoundingClientRect().height, 1);
    this.cssHeight = cssHeight;
    this.defaultWorldUnitsPerCssPixel = DEFAULT_VIEWPORT.height / cssHeight;
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.themeObserver = new MutationObserver(() => this.draw());
    this.resizeObserver.observe(canvas);
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "data-system-theme"],
    });
    window.addEventListener("resize", this.resize);
    window.addEventListener("keydown", this.keyDown);
    window.addEventListener("pagehide", this.autoSave);
    document.addEventListener("visibilitychange", this.visibilityChange);
    canvas.addEventListener("pointerdown", this.pointerDown);
    canvas.addEventListener("pointermove", this.pointerMove);
    canvas.addEventListener("pointerup", this.pointerUp);
    canvas.addEventListener("pointercancel", this.pointerCancel);
    canvas.addEventListener("pointerleave", this.pointerLeave);
    canvas.addEventListener("wheel", this.wheel, { passive: false });
    canvas.addEventListener("contextmenu", this.contextMenu);
    // 环形光标挂在画布外层：与占位层、绘制预览层同级，层级低于图层面板与标注工具条。
    this.eraserRing = document.createElement("span");
    this.eraserRing.className = "pin-board-eraser-ring";
    this.eraserRing.hidden = true;
    (canvas.parentElement ?? canvas).appendChild(this.eraserRing);
    this.resize();

    this.armDeviceLossHandler(gpu);
  }

  /** 监听设备丢失（主动 destroy 产生的 "destroyed" 除外），触发限流后的自动重建。 */
  private armDeviceLossHandler(gpu: GpuState) {
    void gpu.device.lost.then((info: { reason?: string; message?: string }) => {
      if (this.destroyed || gpu.released || this.gpu !== gpu) return;
      if (info?.reason === "destroyed") return;
      this.handleDeviceLost(gpu, info);
    });
  }

  private handleDeviceLost(gpu: GpuState, info: { reason?: string; message?: string }) {
    // 旧设备的资源已作废：标记 released 让在飞行的纹理读取拒绝旧结果，并推进代次。
    // 同时挂起绘制与加载，直到重建完成（或确认放弃）。
    gpu.released = true;
    this.gpuUnavailable = true;
    this.generation += 1;
    const reason = info?.message || info?.reason || "未知原因";
    const attempt = this.gpuRecoveryAttempts + 1;
    if (this.gpuRecoveryAttempts >= GPU_RECOVERY_MAX_ATTEMPTS) {
      diagnosticsLog(
        "warn",
        `pin-board gpu device lost, recovery limit reached: gpuSeq=${gpu.sequence}, attempts=${this.gpuRecoveryAttempts}, reason=${reason}`,
      );
      this.onError(`WebGPU 设备已丢失且重建次数已达上限：${reason}`);
      return;
    }
    this.gpuRecoveryAttempts += 1;
    diagnosticsLog(
      "warn",
      `pin-board gpu device lost, scheduling recovery: gpuSeq=${gpu.sequence}, attempt=${attempt}/${GPU_RECOVERY_MAX_ATTEMPTS}, reason=${reason}`,
    );
    const delay = GPU_RECOVERY_BACKOFF_MS * attempt;
    this.recoveryTimer = window.setTimeout(() => {
      this.recoveryTimer = null;
      void this.recoverGpu();
    }, delay);
  }

  /** 重建 GPU 设备并重新装载纹理。与 `create` 共用 `makeGpu`，避免两条创建路径分叉。 */
  private async recoverGpu() {
    if (this.destroyed) return;
    try {
      // 旧设备上的纹理与缓冲全部作废，先丢弃（destroy 在失效设备上是无操作）。
      try {
        for (const loaded of this.textures.values()) destroyTexture(loaded);
        for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
        this.gpu.viewportBuffer.destroy();
        this.gpu.exportViewportBuffer.destroy();
      } catch {
        // 设备已失效时忽略释放失败。
      }
      this.textures.clear();
      this.previewTextures.clear();
      this.textureReservedBytes = 0;
      this.failedTextureDimensions.clear();
      this.textureQualityCaps.clear();
      // 笔迹缓冲也属于旧设备：清空后由 refreshViewport 按当前画板重建。
      this.destroyStrokeBuffers();
      this.strokeSignature = "";

      const next = await makeGpu(this.canvas);
      if (this.destroyed) {
        next.released = true;
        try {
          next.device.destroy();
        } catch {
          // 忽略。
        }
        return;
      }
      this.gpu = next;
      this.armDeviceLossHandler(next);
      this.gpuUnavailable = false;
      diagnosticsLog(
        "info",
        `pin-board gpu device recovered: newSeq=${next.sequence}, boardId=${this.boardId}, attempts=${this.gpuRecoveryAttempts}`,
      );
      if (this.loaded) this.refreshViewport(0);
      else this.draw();
    } catch (error) {
      // 重建失败时保持 GPU 不可用（挂起绘制），避免持续对失效设备绘图刷错误。
      diagnosticsLog("warn", `pin-board gpu recovery failed: error=${errorMessage(error)}`);
      this.onError(`素材板 GPU 恢复失败：${errorMessage(error)}`);
    }
  }

  /**
   * 创建渲染器：只做画布生命周期内一次性的 GPU 初始化。画板数据随后由 `loadBoard`
   * 装载，因此切换画板/作品不再销毁并新建 GPU 设备。
   */
  static async create(
    canvas: HTMLCanvasElement,
    selectionElement: HTMLElement,
    marqueeElement: HTMLElement,
    placeholderElement: HTMLElement,
    drawOverlay: HTMLCanvasElement,
    arrangementGapRatio: number,
    autosaveEnabled: boolean,
    onError: (message: string) => void,
    onState: (state: PinBoardInteractionState) => void,
    onSessionChange: (boardId: number, session: PinBoardViewSession) => void,
    onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    onStrokeCommit: (request: StrokeCommitRequest) => void,
    cacheBudgets: PinBoardTextureBudgets = textureBudgetsForLevel(undefined),
  ): Promise<PinBoardRenderer> {
    const gpu = await makeGpu(canvas);
    const renderer = new PinBoardRenderer(
      canvas,
      selectionElement,
      marqueeElement,
      placeholderElement,
      drawOverlay,
      gpu,
      arrangementGapRatio,
      autosaveEnabled,
      onError,
      onState,
      onSessionChange,
      onContextMenu,
      onStrokeCommit,
      cacheBudgets,
    );
    adjustLiveRenderers(1);
    return renderer;
  }

  /** 当前装载的画板 id；未装载时为 0。 */
  get currentBoardId(): number {
    return this.boardId;
  }

  /**
   * 装载一块画板：先把旧画板的未保存改动导出成快照交给后台结算队列，随后**同步**释放
   * 旧画板并装载新画板。
   *
   * 切换因此不等待任何仓库写入：后台任务（维护扫描、恢复、精简）持写锁多久都不影响这里。
   * 旧画板的改动由 `pendingSettle` 的串行 worker 写回；同一 `boardId` 幂等。传入的
   * boardId 与当前一致时直接返回。
   *
   * `stagePrevious = false` 用于旧画板已被删除或移入回收站、不能再次写库的场景（那里的
   * 内存状态直接丢弃）。
   */
  loadBoard(
    view: PinBoardView,
    session: PinBoardViewSession | null,
    stagePrevious = true,
  ): boolean {
    if (this.destroyed) return false;
    if (this.loaded && this.boardId === view.boardId) return true;
    this.detachCurrentBoard(stagePrevious);
    if (this.destroyed) return false;
    this.activateBoard(view, session);
    return true;
  }

  /** 卸载当前画板（当前作品的画板已被清空等场景）：导出待结算快照后清空画面，保留 GPU 设备。 */
  unloadBoard(stagePrevious = true): void {
    if (this.destroyed || !this.loaded) return;
    this.detachCurrentBoard(stagePrevious);
    this.locked = false;
    this.canvas.classList.remove("locked");
    this.selectionElement.hidden = true;
    this.marqueeElement.hidden = true;
    this.selectionScreenQuad = null;
    this.updateEraserRing();
    this.draw();
    this.onState(this.interactionState);
  }

  /**
   * 释放当前画板：先记视图会话、再把未保存的改动导出成快照，最后丢画板级资源。
   *
   * 导出是同步的（快照就是内存里条目的副本），因此从调用方看切换是瞬时的。
   */
  private detachCurrentBoard(stagePending: boolean) {
    if (!this.loaded) return;
    // 草稿先同步提交进模型，随快照进入后台结算；无效草稿按取消处理并保留原文。
    // 新建中的文字还没有模型，切画板时直接取消。
    if (!this.settleTextEdit()) this.cancelTextEdit();
    this.placingText = false;
    this.finishActiveInteraction();
    this.persistSession();
    // 挂起的自动保存由快照接手：队列会写同一份内容，不必再走一次防抖保存。
    this.cancelAutosave();
    if (stagePending) this.stagePendingBoard();
    this.releaseBoardResources();
    this.loaded = false;
    this.boardId = 0;
  }

  /**
   * 把未保存的改动交给后台结算队列。
   *
   * 没有未保存改动时不产生快照——切一次作品不该在数据层变成一次仓库写操作，这也是
   * 「后台任务期间照样能切」的来源。软删除条目的回收由队列在保存成功后补做
   * （`finalize_pin_board`），不在切换路径上。
   */
  private stagePendingBoard() {
    if (!this.dirty) return;
    stageBoardSettle(
      {
        boardId: this.boardId,
        revision: this.revision,
        items: this.images.map(cloneImage),
      },
      // 自动保存可能正在飞行：队列要等它落地再写，否则会拿着旧 revision 被拒。
      this.savePromise ?? undefined,
    );
    diagnosticsLog(
      "info",
      `pin-board pending staged: boardId=${this.boardId}, items=${this.images.length}, saving=${this.savePromise !== null}`,
    );
  }

  /** 释放画板级资源：纹理、历史、选中、拖动中间态与飞行中的加载。 */
  private releaseBoardResources() {
    if (this.loadTimer !== null) {
      window.clearTimeout(this.loadTimer);
      this.loadTimer = null;
    }
    this.loadRequested = false;
    for (const loaded of this.textures.values()) destroyTexture(loaded);
    this.textures.clear();
    for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
    this.previewTextures.clear();
    this.textureReservedBytes = 0;
    this.failedTextureDimensions.clear();
    this.textureQualityCaps.clear();
    this.clearPlaceholderOverlay();
    // 绘制草稿属于画板：换画板、卸载与销毁都不该留下半笔。
    this.draft = null;
    this.clearStrokePreview();
    this.releaseStrokeResources();
    this.exportDrawItems = [];
    this.releaseExportStrokeBuffers();
    this.visibleImages = [];
    this.history.length = 0;
    this.historyIndex = 0;
    this.images = [];
    this.imageById.clear();
    this.savedTextSignatures.clear();
    this.textEdit = null;
    this.placingText = false;
    this.suppressClickEdit = false;
    this.resetPressState();
    this.selectedIds.clear();
    this.dragMode = null;
    this.dragPointerId = null;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    // 递增代次，让旧画板在飞行的纹理加载在返回时按代次被丢弃。
    this.generation += 1;
  }

  /**
   * 释放笔迹几何与 GPU 缓冲。读取代次递增，使在途的 `read_pin_board_strokes`
   * 返回后按代次被丢弃——它读的是上一块画板的几何。
   */
  private releaseStrokeResources() {
    this.strokeLoadToken += 1;
    this.strokeGeometries.clear();
    this.strokeVersion += 1;
    this.destroyStrokeBuffers();
    this.strokeSignature = "";
    this.strokeOutlineCache.clear();
    this.resetEraseState();
  }

  private destroyStrokeBuffers() {
    this.strokeVertexBuffer?.destroy();
    this.strokeIndexBuffer?.destroy();
    this.strokeOutlineBuffer?.destroy();
    this.strokeVertexBuffer = null;
    this.strokeIndexBuffer = null;
    this.strokeOutlineBuffer = null;
    this.strokeIndexCount = 0;
    this.strokeOutlineIndex.clear();
    this.strokeBuiltBounds = null;
  }

  /**
   * 读取整板笔迹几何（`read_pin_board_strokes` 的二进制载荷），一次解析成
   * `Map<itemId, StrokeGeometry>`，随后重建笔迹缓冲。
   *
   * 与画板装载解耦：`loadBoard` 保持同步，笔迹在 IPC 回来后就地补画，因此切换画板
   * 不会为了几 KB 几何数据多等一次往返。
   */
  private async loadStrokes(boardId: number) {
    const token = ++this.strokeLoadToken;
    try {
      const response = await pinBoardApi.readPinBoardStrokes(boardId);
      const bytes = response instanceof Uint8Array ? response : new Uint8Array(response as ArrayBuffer);
      const strokes = parseStrokeBatch(bytes);
      if (this.destroyed || this.boardId !== boardId || token !== this.strokeLoadToken) return;
      this.strokeGeometries.clear();
      for (const [itemId, geometry] of strokes) this.strokeGeometries.set(itemId, geometry);
      this.strokeVersion += 1;
      this.strokeSignature = "";
      diagnosticsLog(
        "info",
        `pin-board strokes loaded: boardId=${boardId}, strokes=${strokes.size}, bytes=${bytes.byteLength}`,
      );
      this.refreshViewport(0);
      // 新笔迹已经进缓冲：等落库的预览到这一步才撤，中途不会闪空白。
      this.clearPendingStrokePreview();
    } catch (error) {
      if (this.destroyed || this.boardId !== boardId || token !== this.strokeLoadToken) return;
      diagnosticsLog("warn", `pin-board strokes failed: boardId=${boardId}, error=${errorMessage(error)}`);
      this.onError(`笔迹读取失败：${errorMessage(error)}`);
    }
  }

  /** 设备像素 / 世界单位：抗锯齿羽化与笔迹 LOD 都按它换算。 */
  private devicePixelsPerWorldUnit(): number {
    return this.canvas.height / Math.max(this.viewport.height, 1e-6);
  }

  /**
   * 同步橡皮环形光标的位置、直径与显隐。
   *
   * 直径按「大小 / 世界单位每 CSS 像素」换算：它表示的是世界里的擦头，不是固定像素
   * 的图标，所以缩放变化后屏幕上的圆也必须跟着变。视觉最小值 4 px，再小就看不见；
   * 锁定、退出橡皮、指针离开画布、设备不可用时一律隐藏。
   */
  private updateEraserRing() {
    const ring = this.eraserRing;
    const pointer = this.eraserPointer;
    if (!pointer
      || this.locked
      || this.exporting
      || this.gpuUnavailable
      || this.tool !== "eraser"
      || !this.loaded) {
      ring.hidden = true;
      return;
    }
    const diameter = Math.max(
      ERASER_RING_MIN_CSS_PIXELS,
      this.strokeWidthValue / this.worldUnitsPerCssPixel(),
    );
    ring.hidden = false;
    ring.style.width = `${diameter}px`;
    ring.style.height = `${diameter}px`;
    ring.style.left = `${pointer[0]}px`;
    ring.style.top = `${pointer[1]}px`;
  }

  /** 笔迹缓冲的收集范围：视口四周各扩 `STROKE_OVERSCAN_RATIO`。 */
  private strokeCullBounds(viewport: Bounds): Bounds {
    const marginX = (viewport.maxX - viewport.minX) * STROKE_OVERSCAN_RATIO;
    const marginY = (viewport.maxY - viewport.minY) * STROKE_OVERSCAN_RATIO;
    return {
      minX: viewport.minX - marginX,
      minY: viewport.minY - marginY,
      maxX: viewport.maxX + marginX,
      maxY: viewport.maxY + marginY,
    };
  }

  /**
   * 按「可见笔迹 + 缩放」重建笔迹缓冲。
   *
   * 签名不变**且**视口仍在缓冲的收集框内时跳过：平移只改视口位置、不改集合与
   * 缩放，因此拖动画布不会逐帧重建整板笔迹。集合按扩过超扫边距的视口取，逐段
   * 裁剪也用同一个框，所以在框内平移既不改签名、也不会露出被裁掉的段；视口走出
   * 那个框就重建一次。只比较条目集合不够——一条横跨整个画板的笔迹在两种视口下
   * 都算可见，但缓冲里不一定有远处那段。
   */
  private syncStrokeBuffers() {
    const hasCanvas = this.canvas.width > 0 && this.canvas.height > 0;
    const viewport = hasCanvas
      ? viewportBounds(this.viewport, this.canvas.width, this.canvas.height)
      : null;
    const cullBounds = viewport ? this.strokeCullBounds(viewport) : null;
    const strokes = this.images.filter((image): image is PinBoardStrokeItem => (
      image.kind === "stroke"
      && this.isDrawable(image)
      && (cullBounds === null || intersects(imageBounds(image), cullBounds))
    ));
    const pixelsPerWorldUnit = this.devicePixelsPerWorldUnit();
    const signature = [
      this.strokeVersion,
      Math.round(pixelsPerWorldUnit * 100),
      strokes.map((stroke) => stroke.itemId).join(","),
    ].join("|");
    const builtBounds = this.strokeBuiltBounds;
    const covered = viewport !== null && builtBounds != null
      && viewport.minX >= builtBounds.minX
      && viewport.maxY <= builtBounds.maxY
      && viewport.maxX <= builtBounds.maxX
      && viewport.minY >= builtBounds.minY;
    if (signature === this.strokeSignature && (cullBounds === null || covered)) return;
    this.strokeSignature = signature;
    const entries = strokes.flatMap((item) => {
      const geometry = this.strokeGeometries.get(item.itemId);
      return geometry ? [{ item, geometry }] : [];
    });
    const batch = buildStrokeBatch(entries, {
      pixelsPerWorldUnit,
      ...(cullBounds ? { cullBounds } : {}),
    });
    const uploaded = this.uploadStrokeBatch(batch, buildStrokeOutlines(strokes));
    // 上传失败时不做「已覆盖」标记：下一帧还会重试，而不是永远跳过。
    this.strokeBuiltBounds = uploaded ? cullBounds : null;
  }

  /** 返回是否已可复用（空缓冲也算成功）。失败时由调用方保留重试机会。 */
  private uploadStrokeBatch(
    batch: StrokeBatch,
    outlines: { vertices: Float32Array; index: Map<number, number> },
  ): boolean {
    this.destroyStrokeBuffers();
    if (batch.indices.length === 0) return true;
    try {
      const device = this.gpu.device;
      // 先挂到字段上再写数据：中途失败时 `destroyStrokeBuffers` 仍能释放已创建的缓冲。
      const vertexBuffer = device.createBuffer({
        size: batch.vertices.byteLength,
        usage: 0x20 | 0x08,
        mappedAtCreation: true,
      });
      this.strokeVertexBuffer = vertexBuffer;
      new Float32Array(vertexBuffer.getMappedRange()).set(batch.vertices);
      vertexBuffer.unmap();
      const indexBuffer = device.createBuffer({
        size: batch.indices.byteLength,
        usage: 0x10 | 0x08,
        mappedAtCreation: true,
      });
      this.strokeIndexBuffer = indexBuffer;
      new Uint32Array(indexBuffer.getMappedRange()).set(batch.indices);
      indexBuffer.unmap();
      this.strokeIndexCount = batch.indices.length;
      for (const [itemId, slot] of outlines.index) this.strokeOutlineIndex.set(itemId, slot);
      if (outlines.vertices.length > 0) {
        const outlineBuffer = device.createBuffer({
          size: outlines.vertices.byteLength,
          usage: 0x20 | 0x08,
          mappedAtCreation: true,
        });
        this.strokeOutlineBuffer = outlineBuffer;
        new Float32Array(outlineBuffer.getMappedRange()).set(outlines.vertices);
        outlineBuffer.unmap();
      }
      return true;
    } catch (error) {
      this.destroyStrokeBuffers();
      diagnosticsLog(
        "warn",
        `pin-board stroke buffer failed: boardId=${this.boardId}, error=${errorMessage(error)}`,
      );
      this.onError(`笔迹缓冲创建失败：${errorMessage(error)}`);
      return false;
    }
  }

  private activateBoard(view: PinBoardView, session: PinBoardViewSession | null) {
    // 该画板若还有后台未写回的快照，内容以快照为准：它比仓库里的更新（用户刚在这块
    // 画板上编辑过）；否则后台正握着写、这边又按旧内容装载，界面会回退。
    const pending = takeBoardSettle(view.boardId);
    this.boardId = view.boardId;
    this.images = (pending?.items ?? view.items).map(cloneImage);
    this.imageById.clear();
    for (const image of this.images) this.imageById.set(image.itemId, image);
    this.savedTextSignatures.clear();
    for (const image of this.images) {
      if (image.kind === "text") {
        this.savedTextSignatures.set(image.itemId, pinBoardTextSignature(image));
      }
    }
    this.textEdit = null;
    this.suppressClickEdit = false;
    this.resetPressState();
    this.revision = resolveBoardRevision(view.boardId, pending?.revision ?? view.revision);
    // 装载到的 revision 是仓库当前事实：记进账本，避免账本里残留的旧值在装载
    // 之后继续压过它（例如上次保存撞了冲突、之后重新打开画板时）。
    recordBoardRevision(view.boardId, this.revision);
    this.finalized = false;
    this.finalizePromise = null;
    this.savePromise = null;
    this.saving = false;
    // 快照还没落库，改动就必须继续算「未保存」，否则它会在下次切换时被丢掉。
    this.dirty = pending !== null;
    this.cancelAutosave();
    this.cssHeight = Math.max(this.canvas.getBoundingClientRect().height, 1);
    this.viewport.centerX = DEFAULT_VIEWPORT.centerX;
    this.viewport.centerY = DEFAULT_VIEWPORT.centerY;
    this.viewport.height = DEFAULT_VIEWPORT.height;
    this.pendingViewportFit = false;
    this.locked = false;
    this.selectedIds.clear();
    // 图层可见性要先于选中与视图适配恢复：`contentBounds` 与选中过滤都依赖它，
    // 否则「打开画板 → 按隐藏层里的图片适配视图」会把视图缩放到看不见的内容上。
    this.visibleLayers = normalizeLayerVisibility(session?.visibleLayers);
    // 笔迹几何随画板重新读取：`releaseBoardResources` 已释放旧缓冲，这里再兜一次，
    // 保证直接 `activateBoard`（未经 detach）时也不会画出上一块画板的笔迹。
    this.releaseStrokeResources();
    if (session) {
      this.viewport.centerX = session.centerX;
      this.viewport.centerY = session.centerY;
      this.viewport.height = this.clampViewportHeight(
        session.worldUnitsPerCssPixel * this.cssHeight,
      );
      this.locked = session.locked;
      if (!this.locked) {
        for (const itemId of session.selectedImageIds) {
          const item = this.imageById.get(itemId);
          // 隐藏层里的条目不能被恢复成选中：会话存的是当时的选择，图层开关是之后改的。
          if (item && isLayerVisible(this.visibleLayers, item.layer)) {
            this.selectedIds.add(itemId);
          }
        }
      }
    } else {
      // 进程内没有该画板的会话：打开时按画板内全部图片的最小包围框定位视图。
      this.locked = true;
      this.pendingViewportFit = true;
    }
    // 取回快照时不能把当前状态记成「已保存」：否则下一次 `save` 会直接短路，
    // 快照里的改动就永远落不了库。
    this.savedStateKey = pending ? "" : this.stateKey();
    this.loaded = true;
    this.resize();
    if (this.images.some((image) => image.kind === "stroke")) {
      void this.loadStrokes(view.boardId);
    }
    diagnosticsLog(
      "info",
      pending
        ? `pin-board board loaded from pending snapshot: boardId=${view.boardId}, images=${this.images.length}`
        : `pin-board board loaded: boardId=${view.boardId}, images=${this.images.length}`,
    );
  }

  /** 发出当前画板的视图会话（按 boardId 归属）。 */
  private persistSession() {
    this.onSessionChange(this.boardId, {
      centerX: this.viewport.centerX,
      centerY: this.viewport.centerY,
      worldUnitsPerCssPixel: this.worldUnitsPerCssPixel(),
      locked: this.locked,
      selectedImageIds: this.locked ? [] : this.selectedImageIds(),
      visibleLayers: normalizeLayerVisibility(this.visibleLayers),
    });
  }

  /**
   * 释放整个渲染器与 GPU 设备。只用于模块真正卸载（关闭工作区、切仓库、退出），
   * 切换画板/作品请改用 `loadBoard` / `unloadBoard`。
   *
   * `stagePending` 为真时把未保存的改动交给后台结算队列：模块卸载（例如切到非作品节点）
   * 没有等待写入的时机，交给队列比一次可能失败且无人可见的写更可靠——重新打开该画板时
   * 快照会被 `takeBoardSettle` 取回。
   */
  destroy(stagePending = true) {
    if (this.destroyed) return;
    if (!this.settleTextEdit()) this.cancelTextEdit();
    this.cancelAutosave();
    if (stagePending && this.loaded && !this.finalized) this.stagePendingBoard();
    if (this.loaded) this.persistSession();
    this.destroyed = true;
    this.generation += 1;
    if (this.loadTimer !== null) window.clearTimeout(this.loadTimer);
    if (this.recoveryTimer !== null) {
      window.clearTimeout(this.recoveryTimer);
      this.recoveryTimer = null;
    }
    this.resizeObserver.disconnect();
    this.themeObserver.disconnect();
    window.removeEventListener("resize", this.resize);
    window.removeEventListener("keydown", this.keyDown);
    window.removeEventListener("pagehide", this.autoSave);
    document.removeEventListener("visibilitychange", this.visibilityChange);
    this.canvas.removeEventListener("pointerdown", this.pointerDown);
    this.canvas.removeEventListener("pointermove", this.pointerMove);
    this.canvas.removeEventListener("pointerup", this.pointerUp);
    this.canvas.removeEventListener("pointercancel", this.pointerCancel);
    this.canvas.removeEventListener("pointerleave", this.pointerLeave);
    this.canvas.removeEventListener("wheel", this.wheel);
    this.canvas.removeEventListener("contextmenu", this.contextMenu);
    this.eraserRing.remove();
    this.strokeOutlineCache.clear();
    this.resetEraseState();
    this.clearPlaceholderOverlay();
    this.draft = null;
    this.clearStrokePreview();
    this.releaseStrokeResources();
    this.exporting = false;
    this.exportDrawItems = [];
    this.releaseExportStrokeBuffers();
    for (const loaded of this.textures.values()) destroyTexture(loaded);
    this.textures.clear();
    for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
    this.previewTextures.clear();
    this.textureReservedBytes = 0;
    this.gpu.viewportBuffer.destroy();
    this.gpu.exportViewportBuffer.destroy();
    // 先标记设备已释放，再解绑 canvas 上下文并销毁设备。在飞行的纹理读取返回后，
    // loadTexture 会在任何 GPU 调用之前拒绝旧结果。
    this.gpu.released = true;
    releaseCanvasGpu(this.gpu);
    try {
      this.gpu.device.destroy();
    } catch {
      // 设备已丢失时忽略。
    }
    diagnosticsLog(
      "info",
      `pin-board renderer destroyed: gpuSeq=${this.gpu.sequence}, liveRenderers=${adjustLiveRenderers(-1)}, finalized=${this.finalized}, stagePending=${stagePending}, inFlightTextureLoads=${this.inFlightTextureLoads}`,
    );
  }

  setActive(active: boolean) {
    if (this.active === active) return;
    this.active = active;
    if (!active) {
      if (!this.settleTextEdit()) this.cancelTextEdit();
      this.finishActiveInteraction();
      this.onContextMenu(null);
      this.generation += 1;
      this.loadRequested = false;
      if (this.loadTimer !== null) {
        window.clearTimeout(this.loadTimer);
        this.loadTimer = null;
      }
    } else {
      this.failedTextureDimensions.clear();
      this.refreshViewport(0);
    }
  }

  /** 阵列间距：相对统一图片高度的比例，`0` 表示图片紧贴。 */
  setArrangementGapRatio(ratio: number) {
    this.arrangementGapRatio = ratio;
  }

  /** 配置的缓存等级变化时更新 GPU 缓存预算；下次淘汰/预留立即按新预算执行。 */
  setTextureCacheBudgets(budgets: PinBoardTextureBudgets) {
    this.textureBudgetBytes = budgets.residentBytes;
    this.previewTextureBudgetBytes = budgets.previewBytes;
    this.evictTextures();
  }

  finishActiveInteraction() {
    if (this.dragMode) this.finishInteraction(false);
  }

  /**
   * 切换工具。切到绘制工具时清空选中与框选残留：绘制中不该继续显示变换手柄，
   * 也不该把上一轮的拖拽快照带进来。锁定时只能停在 `select`。
   *
   * 结尾要 `emitState()`：光标类名（`.tool-pen`）在那一处统一切换，否则选了画笔
   * 还要等下一次 pointerdown 才换成笔尖。
   */
  setTool(tool: PinBoardTool) {
    if (this.placingText) this.cancelTextPlacement();
    if (!this.loaded || this.locked || this.exporting || this.tool === tool) return;
    if (!this.settleTextEdit()) return;
    this.finishActiveInteraction();
    this.tool = tool;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    this.updateMarqueeOverlay(null);
    // 绘制与擦除都独占指针，选中框既拖不动、又会挡住下一次操作。
    if (isDrawingTool(tool)) this.selectedIds.clear();
    // 选绘制或擦除工具时自动打开标注层：隐藏着还让人画/擦，画完什么也看不见。
    if (isDrawingTool(tool) && !isLayerVisible(this.visibleLayers, STROKE_LAYER)) {
      this.setLayerVisible(STROKE_LAYER, true);
    }
    this.refreshSelectionVisuals();
    this.emitState();
  }

  /**
   * 开关一个图层。纯视图状态：不写库、不标记 dirty、不进撤销栈。
   *
   * 隐藏某层时把该层里正被选中的条目一起踢出选中，否则它们看不见却还能被删除、
   * 导出或改图层。
   */
  setLayerVisible(layer: number, visible: boolean) {
    if (!this.loaded) return;
    const next = normalizeLayerVisibility(this.visibleLayers);
    if (!Number.isInteger(layer) || layer < 0 || layer >= PIN_BOARD_LAYER_COUNT) return;
    if (next[layer] === visible) return;
    // 正在编辑的文字不能被藏在看不见的图层里：先提交再隐藏。
    if (!visible
      && this.textEdit !== null
      && this.textEdit.itemId !== null
      && this.imageById.get(this.textEdit.itemId)?.layer === layer
      && !this.settleTextEdit()) {
      return;
    }
    next[layer] = visible;
    this.visibleLayers = next;
    if (!visible) {
      for (const itemId of hiddenSelectionIds(this.selectedIds, this.images, next)) {
        this.selectedIds.delete(itemId);
      }
    }
    this.refreshViewport(0);
    this.emitState();
  }

  /** 绘制样式：只影响之后画出的笔迹，不改已有条目。 */
  setStrokeStyle(color: string, width: number) {
    this.strokeColor = /^#[0-9a-f]{6}$/i.test(color) ? color.toLowerCase() : DEFAULT_STROKE_COLOR;
    this.strokeWidthValue = clampStrokeWidth(width);
    if (this.draft) this.drawStrokePreview();
    // 橡皮的直径与笔共用这一个「大小」，改它要立刻反映到环形光标上。
    this.updateEraserRing();
  }

  /**
   * 取消进行中的绘制。不产生条目，也不留预览：锁定、切换画板与释放指针捕获
   * 都走这一条路径。
   */
  cancelDrawing() {
    if (!this.draft && this.dragMode !== "draw") return;
    this.draft = null;
    this.clearStrokePreview();
    this.releaseDrawingPointer();
  }

  /**
   * 清掉「已提交、等落库」的预览。
   *
   * 落库要经过保存、创建与重读几何几个往返，pointerUp 时立刻清掉预览会闪一下空白，
   * 所以预览一直留到新笔迹真正进缓冲为止（`loadStrokes`）或提交失败为止。
   * 正在画下一笔时不动：那一笔的预览已经取代了它。
   */
  clearPendingStrokePreview() {
    if (this.dragMode === "draw" || !this.draft) return;
    this.draft = null;
    this.clearStrokePreview();
  }

  beginViewportResize() {
    this.preservedWorldUnitsPerCssPixel = this.worldUnitsPerCssPixel();
  }

  endViewportResize() {
    this.resize();
    this.preservedWorldUnitsPerCssPixel = null;
  }

  get interactionState(): PinBoardInteractionState {
    const selected = this.selectedImages();
    return {
      itemCount: this.images.filter((image) => !image.deleted).length,
      selectedCount: selected.length,
      arrangeableCount: selected.filter((image) => image.kind === "bitmap").length,
      // 单条导出与系统剪贴板复制只认位图：文字与笔迹的入口一律置灰。
      exportableCount: selected.filter((image) => image.kind === "bitmap").length,
      layerCounts: this.layerCounts(),
      canUndo: !this.locked && this.historyIndex > 0,
      canRedo: !this.locked && this.historyIndex < this.history.length,
      dirty: this.dirty,
      saving: this.saving,
      locked: this.locked,
      zoomPercent: Math.round((
        this.defaultWorldUnitsPerCssPixel / this.worldUnitsPerCssPixel()
      ) * 100),
      tool: this.tool,
      visibleLayers: normalizeLayerVisibility(this.visibleLayers),
      exporting: this.exporting,
      textEditing: this.textEditState(),
      placingText: this.placingText,
    };
  }

  private textEditState(): PinBoardTextEditState | null {
    const edit = this.textEdit;
    if (!edit) return null;
    const rect = this.canvas.getBoundingClientRect();
    if (edit.isNew) {
      const [centerX, centerY] = worldToScreen(edit.center, rect, this.viewport);
      return {
        sessionId: edit.sessionId,
        itemId: null,
        isNew: true,
        draft: { ...edit.draft },
        caret: edit.caret,
        scale: 1,
        centerX,
        centerY,
        angle: 0,
        pixelsPerWorldUnit: rect.height / Math.max(this.viewport.height, 1e-9),
      };
    }
    if (edit.itemId === null) return null;
    const image = this.imageById.get(edit.itemId);
    if (!image || image.kind !== "text") return null;
    const geometry = pinBoardTextEditGeometry(
      image,
      rect,
      this.viewport,
    );
    return {
      sessionId: edit.sessionId,
      itemId: edit.itemId,
      isNew: false,
      draft: { ...edit.draft },
      caret: edit.caret,
      scale: edit.scale.x,
      ...geometry,
    };
  }

  get boardRevision(): string {
    return this.revision;
  }

  get placementPoint(): Point {
    return this.cursorWorld ? [...this.cursorWorld] as Point : [this.viewport.centerX, this.viewport.centerY];
  }

  /**
   * 画板内复制的载荷：**只记来源**，不携带尺寸、变换或载荷内容。
   *
   * 位图的显示尺寸曾经由前端带上，但复制时前端拿到的尺寸与源条目的真实记录可能
   * 不一致（画板被其他窗口改过、快照尚未落库）；统一让原生端读源条目现算，三条
   * 复制路径因此只有一个事实来源。
   */
  copySelection(): PinBoardClipboardItem[] {
    if (this.locked || this.textEdit) return [];
    return this.selectedImages()
      .sort((left, right) => left.order - right.order)
      .map((image) => ({
        kind: image.kind,
        sourceBoardId: image.boardId,
        sourceItemId: image.itemId,
      }));
  }

  selectedImageIds(): number[] {
    return this.selectedImages().map((image) => image.itemId);
  }

  /**
   * 可单条导出的条目：只有位图。
   *
   * 文字与笔迹的单条导出已删除（整画布导出覆盖这两种内容），因此这里必须与
   * `exportableCount` 同口径——否则混选时会把文字 id 交给只认位图的原生命令。
   */
  exportableImageIds(): number[] {
    return this.selectedImages()
      .filter((image) => image.kind === "bitmap")
      .map((image) => image.itemId);
  }

  /**
   * 进入文字放置模式：画布光标变为 I 字型，下一次左键点击在落点新建文字，
   * 右键或 Escape 取消。与就地编辑共用同一个草稿编辑框。
   *
   * 只有工具栏按钮走这里：按钮本身不含落点，由用户到画布上自己点。右键菜单
   * 已经有落点，直接走 `addTextAtClientPoint`。
   */
  startTextPlacement(): boolean {
    if (!this.loaded || this.locked || this.exporting || this.gpuUnavailable) return false;
    if (!this.settleTextEdit()) return false;
    this.finishActiveInteraction();
    this.placingText = true;
    this.tool = "select";
    this.selectedIds.clear();
    this.marqueeBaseIds.clear();
    this.updateMarqueeOverlay(null);
    this.canvas.classList.remove("over-text");
    this.refreshSelectionVisuals();
    this.emitState();
    return true;
  }

  cancelTextPlacement() {
    if (!this.placingText) return;
    this.placingText = false;
    this.emitState();
  }

  /**
   * 在视口坐标处直接打开空白编辑框：右键菜单「添加文字」用它，落点就是右键位置，
   * 不经过放置模式。
   */
  addTextAtClientPoint(clientX: number, clientY: number): boolean {
    if (!this.loaded || this.locked || this.exporting || this.gpuUnavailable) return false;
    if (!this.settleTextEdit()) return false;
    this.finishActiveInteraction();
    this.placingText = false;
    this.onContextMenu(null);
    const point = screenToWorld(clientX, clientY, this.canvas.getBoundingClientRect(), this.viewport);
    this.cursorWorld = point;
    return this.beginNewTextEdit(point);
  }

  /**
   * 进入文字就地编辑：只接受恰好一条、当前可见的文字。
   *
   * 入口有三个：第二次点击已选中的单条文字、`F2` 与右键菜单。进入时不对模型做任何
   * 修改，草稿留在编辑会话里；因此编辑期间自动保存不会写出半成品。
   *
   * `caretPoint` 是第二次点击的世界坐标：用它把光标落到点击处；从菜单或 `F2`
   * 进入时为空，光标放在末尾。
   */
  beginTextEdit(itemId?: number, caretPoint?: Point): boolean {
    if (!this.loaded || this.locked || this.exporting || this.gpuUnavailable) return false;
    if (!this.settleTextEdit()) return false;
    this.finishActiveInteraction();
    const targetId = itemId ?? (
      this.selectedIds.size === 1 ? [...this.selectedIds][0] : null
    );
    if (targetId === null) return false;
    const image = this.imageById.get(targetId);
    if (!image
      || image.kind !== "text"
      || image.deleted
      || !isLayerVisible(this.visibleLayers, image.layer)) {
      return false;
    }
    this.placingText = false;
    this.selectedIds.clear();
    this.selectedIds.add(targetId);
    this.tool = "select";
    this.canvas.classList.remove("over-text");
    const before = this.snapshot([targetId])[0];
    if (!before) return false;
    this.textEdit = {
      sessionId: ++this.textEditSessionId,
      itemId: targetId,
      isNew: false,
      before,
      draft: textDraftFromItem(image),
      scale: pinBoardTextScale(image),
      center: quadCenter(image.points),
      angle: textQuadAngle(image.points),
      caret: caretPoint ? pinBoardTextCaretIndexAtPoint(image, caretPoint) : null,
    };
    this.ensureTextEditReadable();
    this.updateSelectionOverlay();
    this.draw();
    this.emitState();
    return true;
  }

  /** 在放置点打开空白编辑框；提交时才创建条目，不修改模型。 */
  private beginNewTextEdit(center: Point): boolean {
    if (!this.loaded || this.locked || this.exporting || this.gpuUnavailable) return false;
    this.selectedIds.clear();
    this.tool = "select";
    this.textEdit = {
      sessionId: ++this.textEditSessionId,
      itemId: null,
      isNew: true,
      before: null,
      draft: { ...this.textDefaults, text: "" },
      scale: { x: 1, y: 1 },
      center: [...center] as Point,
      angle: 0,
      caret: 0,
    };
    this.ensureTextEditReadable();
    this.updateSelectionOverlay();
    this.draw();
    this.emitState();
    return true;
  }

  /**
   * 屏幕字号太小时自动放大视图：有效字号不变，只改变观看比例。
   * 以编辑框中心为锚点，避免文字跳走。
   */
  private ensureTextEditReadable() {
    const edit = this.textEdit;
    if (!edit) return;
    const rect = this.canvas.getBoundingClientRect();
    if (rect.height <= 0 || this.viewport.height <= 0) return;
    const screenFont = edit.draft.fontSize * edit.scale.x * rect.height / this.viewport.height;
    if (screenFont >= TEXT_EDIT_READABLE_MIN_CSS_PIXELS) return;
    const factor = Math.max(0.01, screenFont / TEXT_EDIT_READABLE_TARGET_CSS_PIXELS);
    const [centerX, centerY] = worldToScreen(edit.center, rect, this.viewport);
    this.zoomAt(factor, rect.left + centerX, rect.top + centerY);
  }

  /** React 编辑器每次输入/改样式都把草稿推回来；不触发状态广播，避免打字时整页重渲染。 */
  updateTextEditDraft(draft: PinBoardTextDraft) {
    if (!this.textEdit) return;
    this.textEdit.draft = {
      text: draft.text,
      fontSize: draft.fontSize,
      color: draft.color.toLowerCase(),
      align: draft.align,
    };
  }

  /**
   * 提交当前编辑会话。成功返回 null；失败返回可展示的错误并保留会话与草稿。
   *
   * 一次会话只产生一条撤销记录：`before` 是进入编辑时的快照，`after` 是提交后的
   * 快照，中间每字、每次样式变化都不单独进历史。删到空与没有实际改动都只关闭会话。
   *
   * `suppressClickEdit` 由「点击外部提交」传入：同一次按下不应在 pointerup 时又把
   * 文字重新推进编辑（点同一条文字时会发生）。
   */
  commitTextEdit({ suppressClickEdit = false }: { suppressClickEdit?: boolean } = {}): string | null {
    const edit = this.textEdit;
    if (!edit) return null;
    if (edit.isNew) {
      // 新建会话由编辑器走创建命令；这里只负责「收回草稿」的入口（切画板、锁定等），
      // 没提交过的空白文字没有可写入的模型，直接按取消处理。
      this.closeTextEdit(suppressClickEdit);
      return null;
    }
    if (edit.itemId === null) {
      this.cancelTextEdit();
      return null;
    }
    const image = this.imageById.get(edit.itemId);
    if (!image || image.kind !== "text") {
      this.cancelTextEdit();
      return null;
    }
    const before = edit.before;
    if (!before) {
      this.cancelTextEdit();
      return null;
    }
    // 删到空再提交按取消处理：已有文字恢复原文，不删除条目。
    if (edit.draft.text.trim().length === 0) {
      this.closeTextEdit(suppressClickEdit);
      return null;
    }
    const resolved = resolvePinBoardTextDraft(
      edit.draft,
      edit.scale,
      edit.center,
      edit.angle,
    );
    if (!resolved.ok) return resolved.error;
    // 没有实际改动时不动模型：旋转文字的四边形是按中心与角度重建的，浮点误差会让
    // 「进去看看又退出」也留下一条撤销记录和一次保存。
    if (image.text === edit.draft.text
      && image.fontSize === resolved.fontSize
      && image.color === resolved.color
      && image.align === resolved.align
      && image.width === resolved.width
      && image.height === resolved.height) {
      this.closeTextEdit(suppressClickEdit);
      return null;
    }
    image.text = edit.draft.text;
    image.fontSize = resolved.fontSize;
    image.color = resolved.color;
    image.align = resolved.align;
    image.width = resolved.width;
    image.height = resolved.height;
    image.points = resolved.points;
    this.textEdit = null;
    this.suppressClickEdit = suppressClickEdit;
    this.invalidateTextTexture(edit.itemId);
    const after = this.snapshot([edit.itemId]);
    this.pushHistory([before], after);
    this.afterModelChange([edit.itemId]);
    return null;
  }

  cancelTextEdit() {
    if (!this.textEdit) return;
    this.closeTextEdit(false);
  }

  /** 校验并解出新建文字的创建载荷；失败时编辑会话与草稿保留。 */
  resolveNewTextEdit(): PinBoardNewTextResult {
    const edit = this.textEdit;
    if (!edit?.isNew) return { ok: false, error: "当前没有正在新建的文字" };
    return resolveNewPinBoardTextDraft(edit.draft, edit.center);
  }

  /**
   * 新建文字已由创建命令落库：记住本次样式，关闭编辑会话并让选中框落到新条目上。
   * 由创建流程在 `applyMutationResult` 之后调用。
   */
  completeNewTextEdit() {
    const edit = this.textEdit;
    if (!edit?.isNew) return;
    this.textDefaults = {
      text: "",
      fontSize: edit.draft.fontSize,
      color: edit.draft.color.toLowerCase(),
      align: edit.draft.align,
    };
    this.closeTextEdit(false);
  }

  /** 结束会话但不改模型：取消与没有实际改动的提交共用，重新画出文字与选中框。 */
  private closeTextEdit(suppressClickEdit: boolean) {
    this.textEdit = null;
    this.suppressClickEdit = suppressClickEdit;
    this.updateSelectionOverlay();
    this.draw();
    this.emitState();
  }

  /** 模型操作前统一收口编辑会话；校验失败时阻止后续操作并上报原因。 */
  private settleTextEdit(): boolean {
    if (!this.textEdit) return true;
    const error = this.commitTextEdit();
    if (error) {
      this.onError(error);
      return false;
    }
    return true;
  }

  /**
   * 文字内容/样式变化后旧纹理必然过期：销毁高清与缩略图，让下一次装载重建。
   *
   * 可见条目会先同步建好新纹理再替换旧纹理——提交编辑时编辑框与 GPU 纹理在同一帧
   * 交接，中间空一帧就会闪出空白。同步重建失败时退回原路径，由排队加载兜底。
   */
  private invalidateTextTexture(itemId: number) {
    const rebuilt = this.rebuildTextTexture(itemId);
    for (const target of [this.textures, this.previewTextures]) {
      const loaded = target.get(itemId);
      if (!loaded || (rebuilt && target === this.textures)) continue;
      destroyTexture(loaded);
      target.delete(itemId);
    }
    this.textureQualityCaps.delete(itemId);
    this.failedTextureDimensions.delete(itemId);
  }

  /** 可见文字的纹理已在本地栅格化，可直接重建并换上；不可见时留给排队加载。 */
  private rebuildTextTexture(itemId: number): boolean {
    if (!this.loaded || this.destroyed || this.gpuUnavailable) return false;
    const image = this.imageById.get(itemId);
    if (!image || image.kind !== "text" || image.deleted) return false;
    if (!this.visibleImages.some((visible) => visible.itemId === itemId)) return false;
    const dimension = this.requestedDimension(image);
    if (dimension < 1) return false;
    const reservation = estimatedResidentBytes(image, dimension)
      + PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES;
    if (!this.reserveTextureBytes(itemId, reservation)) return false;
    try {
      const loaded = loadTextTexture(this.gpu, image, dimension);
      const previous = this.textures.get(itemId);
      this.textures.set(itemId, loaded);
      if (previous) destroyTexture(previous);
      this.textureQualityCaps.delete(itemId);
      this.failedTextureDimensions.delete(itemId);
      return true;
    } catch (error) {
      diagnosticsLog(
        "warn",
        `pin-board text texture sync rebuild failed: boardId=${this.boardId}, itemId=${itemId},`
        + ` dim=${dimension}, error=${errorMessage(error)}`,
      );
      return false;
    } finally {
      this.textureReservedBytes = Math.max(0, this.textureReservedBytes - reservation);
    }
  }

  /** 清掉本次按下的点击判定状态。 */
  private resetPressState() {
    this.pressClient = null;
    this.pressMoved = false;
    this.pressWasSelected = false;
    this.pressItemId = null;
    this.pressSuppressEdit = false;
  }

  /**
   * 整画布导出的计划：可见内容的外框加边距，按传入的倍率算出输出尺寸与分块布局。
   *
   * 倍率由对话框决定（默认值是 `canvasExportAutoScale` 算出来的「塞进建议上限」），
   * 手动填的倍率优先——`limit` 只用于标记「超过建议上限」。对话框与真正的导出共用
   * 这一个入口（同一个 `resolveCanvasExportPlan`），所以「输出 W×H」与实际写出的 PNG
   * 尺寸不可能不一致。
   */
  canvasExportPlan(requestedScale: number, margin: number, limit: number): CanvasExportPlan | null {
    const content = this.exportContentBounds();
    if (!content) return null;
    const deviceLimit = this.gpu?.device?.limits?.maxTextureDimension2D;
    const tileLimit = Number.isFinite(deviceLimit)
      ? Math.min(CANVAS_EXPORT_TILE_MAX_WIDTH, Math.max(1, Math.floor(deviceLimit as number)))
      : CANVAS_EXPORT_TILE_MAX_WIDTH;
    return resolveCanvasExportPlan(content, requestedScale, margin, limit, tileLimit);
  }

  /** 对话框的默认倍率：1x 与「最长边塞进建议上限」取小。 */
  canvasExportAutoScale(margin: number, limit: number): number {
    const content = this.exportContentBounds();
    if (!content) return 1;
    return autoCanvasExportScale(content, margin, limit);
  }

  /**
   * 导出可见内容的外框。`available` 为假的位图（DDS 缺失）画不出来，也不该把它们的
   * 位置算进画布范围——否则导出图上会出现一块谁也没见过的空白。
   */
  private exportContentBounds(): Bounds | null {
    return imagesBounds(this.images.filter((image) => this.isDrawable(image)));
  }

  /**
   * 整画布导出：把可见内容离屏渲染成一块块 RGBA，交给原生端流式拼成一张透明 PNG。
   *
   * 导出的是渲染器内存里的当前状态，**不落库**，因此整段流程不产生任何仓库写操作，
   * 也不取仓库写锁（原生端只写用户选的输出目录）。期间画板锁定；取消、切画板与
   * 销毁都会中止并丢弃临时文件。
   */
  async exportCanvas(
    plan: CanvasExportPlan,
    options: {
      directory: string;
      fileName: string;
      onProgress: (sent: number, total: number) => void;
      isCancelled: () => boolean;
    },
  ): Promise<string> {
    if (!this.loaded || this.exporting || this.destroyed || this.gpuUnavailable) {
      throw new Error("当前画板无法导出");
    }
    this.placingText = false;
    if (!this.settleTextEdit()) throw new Error("文字编辑尚未提交，无法导出");
    const boardId = this.boardId;
    const tiles = canvasExportTiles(plan);
    this.finishActiveInteraction();
    // 导出期间不落库：挂起的自动保存推到导出结束之后。
    this.cancelAutosave();
    this.exporting = true;
    this.emitState();
    this.updateEraserRing();
    let sessionOpen = false;
    try {
      await pinBoardApi.beginPinBoardCanvasExport(
        options.directory,
        options.fileName,
        plan.width,
        plan.height,
      );
      sessionOpen = true;
      this.exportDrawItems = this.renderableItems();
      this.prepareExportStrokeBuffers(plan);
      const texture = createExportTexture(this.gpu.device, plan);
      try {
        for (let index = 0; index < tiles.length; index += 1) {
          const tile = tiles[index];
          if (options.isCancelled()) throw new Error("导出已取消");
          if (this.destroyed || this.currentBoardId !== boardId) throw new Error("导出已取消");
          await this.prepareExportTextures(plan, tile);
          const rgba = await this.readExportTile(plan, texture, tile);
          await pinBoardApi.writePinBoardCanvasExportTile(tile, rgba);
          options.onProgress(index + 1, tiles.length);
        }
        const path = await pinBoardApi.finishPinBoardCanvasExport();
        sessionOpen = false;
        return path;
      } finally {
        texture.destroy();
      }
    } catch (error) {
      if (sessionOpen) await pinBoardApi.cancelPinBoardCanvasExport().catch(() => undefined);
      throw error;
    } finally {
      this.exporting = false;
      this.exportDrawItems = [];
      this.exportProtectedIds = null;
      this.releaseExportStrokeBuffers();
      this.updateEraserRing();
      if (this.loaded && !this.destroyed) {
        this.refreshViewport(0);
        this.emitState();
        if (this.dirty) this.scheduleAutosave();
      }
    }
  }

  /**
   * 条目是否可画、可选、可导出：未删除、纹理可用且所在图层可见。
   *
   * 屏幕绘制、笔迹缓冲、导出外框与导出条目四处共用同一条判定——少接一处就会出现
   * 「屏幕上看不见、导出里却有一块」这类不一致。
   */
  private isDrawable(image: PinBoardItem): boolean {
    return !image.deleted
      && image.available
      && isLayerVisible(this.visibleLayers, image.layer);
  }

  /** 可绘制条目（未删除、可用、所在图层可见），按 layer 再按 order——与屏幕同一口径。 */
  private renderableItems(): PinBoardItem[] {
    return this.images
      .filter((image) => this.isDrawable(image))
      .sort((left, right) => left.layer - right.layer || right.order - left.order);
  }

  /**
   * 按导出倍率为「这一块涉及的位图」准备纹理。
   *
   * 复用现有的读取与预算机制，只是把目标尺寸换成「显示尺寸 × 导出倍率」：2x/4x 导出
   * 若沿用屏幕分辨率，放大后就是糊的。单张就要吃掉四分之一预算时不再升级——预算里
   * 塞不下几张，反复淘汰比降一点保真度更糟，此时退回现有纹理（屏幕分辨率或缩略图）。
   */
  private async prepareExportTextures(
    plan: CanvasExportPlan,
    tile: CanvasExportTile,
  ): Promise<void> {
    const region = exportTileBounds(plan, tile);
    const tileImages = this.exportDrawItems.filter((image) => (
      image.kind !== "stroke" && intersects(imageBounds(image), region)
    ));
    this.exportProtectedIds = new Set(tileImages.map((image) => image.itemId));
    try {
      for (const image of tileImages) {
        const desired = exportTextureDimension(
          image,
          plan.scale,
          this.gpu.device.limits.maxTextureDimension2D as number,
        );
        const current = this.textures.get(image.itemId);
        if (current && current.maxDimension >= desired * 0.9) continue;
        if (estimatedResidentBytes(image, desired) > this.textureBudgetBytes / 4) continue;
        await this.loadExportTexture(image, desired);
      }
    } finally {
      this.exportProtectedIds = null;
    }
  }

  /**
   * 载入一张导出尺寸的纹理并替换缓存里的那一张。失败只记日志、保留现有纹理：
   * 导出应当尽力给出能看的图，而不是为了一张位图的高清版本整次失败。
   */
  private async loadExportTexture(image: PinBoardItem, dimension: number): Promise<void> {
    const reservation = estimatedResidentBytes(image, dimension)
      + PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES;
    if (!this.reserveTextureBytes(image.itemId, reservation)) return;
    try {
      const loaded = await loadTexture(this.gpu, image, dimension);
      const previous = this.textures.get(image.itemId);
      if (previous) destroyTexture(previous);
      this.textures.set(image.itemId, loaded);
      this.failedTextureDimensions.delete(image.itemId);
    } catch (error) {
      diagnosticsLog(
        "warn",
        `pin-board export texture fallback: itemId=${image.itemId}, dim=${dimension}, error=${errorMessage(error)}`,
      );
    } finally {
      this.textureReservedBytes = Math.max(0, this.textureReservedBytes - reservation);
    }
  }

  /**
   * 按导出区域单独构建笔迹缓冲：收集框是整个导出区域，逐段裁剪在 `buildStrokeBatch`
   * 里完成；屏幕那份（`strokeVertexBuffer` / `strokeIndexCount`）一个字段都不动。
   */
  private prepareExportStrokeBuffers(plan: CanvasExportPlan): void {
    this.releaseExportStrokeBuffers();
    try {
      this.exportStrokeBuffers = buildExportStrokeBuffers(
        this.gpu.device,
        this.exportDrawItems,
        this.strokeGeometries,
        plan,
      );
    } catch (error) {
      diagnosticsLog(
        "warn",
        `pin-board export stroke buffer failed: boardId=${this.boardId}, error=${errorMessage(error)}`,
      );
    }
  }

  private releaseExportStrokeBuffers(): void {
    destroyExportStrokeBuffers(this.exportStrokeBuffers);
    this.exportStrokeBuffers = null;
  }

  /**
   * 渲染一块并回读成紧排的 RGBA。
   *
   * 导出通道不画选中描边、纹理占位框、框选矩形与绘制预览——它们属于屏幕上的编辑
   * 提示，不是画板内容。清屏值是全透明，因此输出就是一张可直接叠到别处的 PNG。
   */
  private readExportTile(
    plan: CanvasExportPlan,
    texture: any,
    tile: CanvasExportTile,
  ): Promise<Uint8Array> {
    return readExportTile({
      gpu: this.gpu,
      texture,
      plan,
      tile,
      drawItems: this.exportDrawItems,
      textures: this.textures,
      previewTextures: this.previewTextures,
      strokeBuffers: this.exportStrokeBuffers,
    });
  }

  /**
   * 选中画板内全部未删除图片（Ctrl+A 全选整个画布）。锁定时不生效；选中态不属于
   * 模型状态，所以这里不调整图片顺序、也不标记 dirty。
   */
  selectAll() {
    if (this.locked || this.textEdit) return;
    this.finishActiveInteraction();
    this.selectedIds.clear();
    for (const image of this.images) {
      // 隐藏图层里的条目不能被全选：看不见却被选中，之后的删除/导出会莫名其妙地作用到它们。
      if (image.deleted || !isLayerVisible(this.visibleLayers, image.layer)) continue;
      this.selectedIds.add(image.itemId);
    }
    this.refreshSelectionVisuals();
  }

  /** 拖放导入时把放置点对齐到鼠标落点，避免沿用上一次悬停位置。 */
  setPlacementFromClient(clientX: number, clientY: number) {
    this.cursorWorld = screenToWorld(
      clientX,
      clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
  }

  copyImageToClipboardId(): number | null {
    const selected = this.selectedImages();
    return selected.length === 1 && selected[0].kind === "bitmap" ? selected[0].itemId : null;
  }

  deleteSelected() {
    const selected = this.selectedImages();
    if (this.locked || this.textEdit || selected.length === 0) return;
    const itemIds = selected.map((image) => image.itemId);
    const before = this.snapshot(itemIds);
    for (const image of selected) image.deleted = true;
    const after = this.snapshot(itemIds);
    this.pushHistory(before, after);
    this.selectedIds.clear();
    this.afterModelChange(itemIds);
  }

  applyMutationResult(view: PinBoardView, itemIds: number[]) {
    if (!this.loaded || view.boardId !== this.boardId) return;
    const added = itemIds.flatMap((itemId) => view.items.filter((image) => image.itemId === itemId));
    if (added.length === 0) return;
    for (const image of view.items) {
      const current = this.imageById.get(image.itemId);
      if (current) {
        current.order = image.order;
        current.layer = image.layer;
        current.deleted = image.deleted;
      }
    }
    const before = added.map((image) => ({
      itemId: image.itemId,
      deleted: true,
      layer: image.layer,
      points: cloneQuad(image.points),
      uv: cloneQuad(image.uv),
    }));
    // 新条目落进隐藏图层会让「添加成功」看起来像什么都没发生（文字、粘贴、导入
    // 都会撞上），因此把它们所在的层一并打开——刚创建的内容应该看得见。
    const hiddenLayers = new Set(
      added
        .map((image) => image.layer)
        .filter((layer) => !isLayerVisible(this.visibleLayers, layer)),
    );
    if (hiddenLayers.size > 0) {
      const next = normalizeLayerVisibility(this.visibleLayers);
      for (const layer of hiddenLayers) next[layer] = true;
      this.visibleLayers = next;
    }
    for (const image of added) {
      const copy = cloneImage(image);
      this.images.push(copy);
      this.imageById.set(copy.itemId, copy);
      if (copy.kind === "text") {
        this.savedTextSignatures.set(copy.itemId, pinBoardTextSignature(copy));
      }
      // 绘制工具下不自动选中新条目：工具模式的指针只走绘制分支，包围框既拖不动、
      // 也挡在下一笔落笔的位置上。只有回到选择工具才让新条目可变换。
      if (this.tool === "select") this.selectedIds.add(copy.itemId);
    }
    const after = this.snapshot(itemIds);
    this.revision = view.revision;
    // 新建命令（笔迹/文字/粘贴/导入）会推进后端 revision。这里必须同步推进共享
    // 账本，否则下一次保存仍从账本取旧值，被后端判成「画板已被其他窗口修改」。
    recordBoardRevision(this.boardId, view.revision);
    this.savedStateKey = this.stateKey();
    this.pushHistory(before, after);
    this.afterModelChange(itemIds);
    // 新笔迹的几何不在条目 DTO 里：落库后重读一次整板几何再重建缓冲。
    if (added.some((image) => image.kind === "stroke")) void this.loadStrokes(this.boardId);
  }

  toggleLock() {
    if (!this.locked) {
      if (!this.settleTextEdit()) return;
      this.finishInteraction(false);
      this.placingText = false;
      this.selectedIds.clear();
      this.onContextMenu(null);
    }
    this.locked = !this.locked;
    // 锁定时绘制工具一并退出：工具栏上的工具按钮此时是禁用的，停留在「笔」上
    // 只会让人以为还能画。
    if (this.locked) this.tool = "select";
    this.refreshSelectionVisuals();
    this.emitState();
  }

  zoomIn() {
    this.zoomAt(1 / 1.2);
  }

  zoomOut() {
    this.zoomAt(1.2);
  }

  /** 重置视图：画板内有图片时回到“恰好包含全部图片”的视图，否则回默认视图。 */
  resetViewport() {
    const rect = this.canvas.getBoundingClientRect();
    this.applyContentFit(Math.max(rect.width, 1), Math.max(rect.height, 1));
    this.refreshViewport(0);
    this.emitState();
  }

  undo() {
    if (this.locked || this.textEdit || this.historyIndex === 0) return;
    const entry = this.history[this.historyIndex - 1];
    this.historyIndex -= 1;
    this.restore(entry.before);
    this.afterModelChange(entry.before.map((image) => image.itemId));
  }

  redo() {
    if (this.locked || this.textEdit || this.historyIndex >= this.history.length) return;
    const entry = this.history[this.historyIndex];
    this.historyIndex += 1;
    this.restore(entry.after);
    this.afterModelChange(entry.after.map((image) => image.itemId));
  }

  scaleSelected(factor: number) {
    const selected = this.selectedImages();
    if (selected.length === 0) return;
    if (factor < 1 && selected.some((image) => Math.min(...imageSize(image)) * factor < 0.001)) return;
    const center = imagesCenter(selected);
    this.commitMutation((image) => {
      image.points = scaleQuad(image.points, factor, center);
    });
  }

  rotateSelected(degrees: number) {
    const selected = this.selectedImages();
    if (selected.length === 0) return;
    const center = imagesCenter(selected);
    this.commitMutation((image) => {
      image.points = rotateQuad(image.points, degrees * Math.PI / 180, center);
    });
  }

  flipSelected(horizontal: boolean) {
    this.commitMutation((image) => {
      const [uv0, uv1, uv2, uv3] = image.uv.map((uv) => [...uv] as Point) as Quad;
      image.uv = horizontal
        ? [uv1, uv0, uv3, uv2]
        : [uv3, uv2, uv1, uv0];
    });
  }

  setSelectedLayer(layer: 0 | 1 | 2) {
    // 笔迹固定落在标注层，保存时原生端会忽略前端发来的 layer；本地跟着改只会
    // 变成「改完重开又回去」。混选时整组拒绝而不是「只改图片」——半执行的图层
    // 切换没有可见反馈，用户会以为笔迹也跟着走了。
    if (this.selectedImages().some((image) => image.kind === "stroke")) return;
    this.commitMutation((image) => {
      image.layer = layer;
    });
  }

  /**
   * 阵列排序：只处理位图条目。
   *
   * 几何计算统一在 Rust 侧完成（`arrange_pin_board_images` 是纯计算命令，不写库），
   * 这里只负责收集输入、把结果交回 `commitMutation`。撤销栈、脏标记与自动保存因此
   * 与其它几何操作完全一致。文字条目被排除：它的 quad 被拉伸时 `fontSize` 不会跟着
   * 变，字会被拉变形。
   */
  async arrangeSelected(): Promise<void> {
    if (this.locked) return;
    this.finishActiveInteraction();
    const bitmaps = this.selectedImages().filter((image) => image.kind === "bitmap");
    if (bitmaps.length < 2) return;
    const items = bitmaps.map((image) => {
      const [width, height] = imageSize(image);
      const [centerX, centerY] = quadCenter(image.points);
      return { itemId: image.itemId, width, height, centerX, centerY };
    });
    const boardId = this.boardId;
    let arranged: PinBoardArrangeResult[];
    try {
      arranged = await pinBoardApi.arrangePinBoardImages(items, this.arrangementGapRatio);
    } catch (error) {
      diagnosticsLog("error", `pin-board arrange failed: ${errorMessage(error)}`);
      this.onError(errorMessage(error));
      return;
    }
    // 计算是异步的：结果回来时画板可能已经切换，落位必须丢弃。
    if (this.boardId !== boardId || this.destroyed || this.locked) return;
    const points = new Map(arranged.map((entry) => [entry.itemId, entry.points]));
    this.commitMutation((image) => {
      const next = points.get(image.itemId);
      if (next) image.points = next;
    });
  }

  save(): Promise<boolean> {
    this.cancelAutosave();
    if (!this.loaded) return Promise.resolve(true);
    if (this.savePromise) {
      return this.savePromise.then((saved) => (
        saved && this.dirty ? this.save() : saved
      ));
    }
    const stateKey = this.stateKey();
    if (stateKey === this.savedStateKey) return Promise.resolve(true);
    // 切换画板是同步释放的，写入却可能还在飞行：结果回来时当前画板可能已经换了一块，
    // 因此保存期间的变化一律按发起时的 boardId 归属，只推进 revision 账本。
    const boardId = this.boardId;
    const revision = resolveBoardRevision(boardId, this.revision);
    const images = this.editableImages();
    // 保存成功后才把签名推进到请求时的值：保存期间又改过的文字仍会被判成未保存。
    const requestSignatures = new Map<number, string>();
    for (const image of this.images) {
      if (image.kind === "text") {
        requestSignatures.set(image.itemId, pinBoardTextSignature(image));
      }
    }
    this.saving = true;
    this.emitState();
    const started = performance.now();
    diagnosticsLog(
      "info",
      `pin-board save start: boardId=${boardId}, revision=${revision}, images=${images.length}`,
    );
    const task = pinBoardApi.savePinBoard(boardId, images, revision)
      .then((result) => {
        recordBoardRevision(boardId, result.revision);
        if (this.boardId !== boardId) {
          diagnosticsLog(
            "info",
            `pin-board save landed on a released board: boardId=${boardId}, revision=${result.revision}, elapsedMs=${Math.round(performance.now() - started)}`,
          );
          return true;
        }
        this.revision = result.revision;
        this.savedStateKey = stateKey;
        this.savedTextSignatures.clear();
        for (const [itemId, signature] of requestSignatures) {
          this.savedTextSignatures.set(itemId, signature);
        }
        this.dirty = this.stateKey() !== stateKey;
        // 保存期间又有编辑时，恢复自动保存排程。
        if (this.dirty) this.scheduleAutosave();
        diagnosticsLog(
          "info",
          `pin-board save done: boardId=${boardId}, revision=${result.revision}, elapsedMs=${Math.round(performance.now() - started)}`,
        );
        return true;
      })
      .catch((error) => {
        diagnosticsLog(
          "warn",
          `pin-board save failed: boardId=${boardId}, elapsedMs=${Math.round(performance.now() - started)}, error=${errorMessage(error)}`,
        );
        // 已释出的画板没有界面可提示：它的改动在后台队列里，由队列决定重试或报错。
        if (this.boardId === boardId) this.onError(`画板保存失败：${errorMessage(error)}`);
        return false;
      })
      .finally(() => {
        if (this.savePromise === task) this.savePromise = null;
        this.saving = false;
        this.emitState();
      });
    this.savePromise = task;
    return task;
  }

  finalize(): Promise<boolean> {
    if (!this.loaded) return Promise.resolve(true);
    if (this.finalized) return Promise.resolve(true);
    if (this.finalizePromise) return this.finalizePromise;
    if (!this.settleTextEdit()) return Promise.resolve(false);
    const boardId = this.boardId;
    diagnosticsLog(
      "info",
      `pin-board finalize start: boardId=${boardId}, revision=${this.revision}`,
    );
    const task = this.save()
      .then((saved) => {
        if (!saved) {
          diagnosticsLog(
            "warn",
            `pin-board finalize aborted: boardId=${boardId}, save failed`,
          );
          return false;
        }
        return this.finalizeWithBusyRetry()
          .then((result) => {
            recordBoardRevision(boardId, result.revision);
            if (this.boardId !== boardId) return true;
            this.revision = result.revision;
            this.finalized = true;
            diagnosticsLog(
              "info",
              `pin-board finalize done: boardId=${boardId}, revision=${result.revision}`,
            );
            return true;
          });
      })
      .catch((error) => {
        diagnosticsLog(
          "warn",
          `pin-board finalize failed: boardId=${boardId}, error=${errorMessage(error)}`,
        );
        if (this.boardId === boardId) this.onError(`画板结算失败：${errorMessage(error)}`);
        return false;
      })
      .finally(() => {
        if (this.finalizePromise === task) this.finalizePromise = null;
      });
    this.finalizePromise = task;
    return task;
  }

  /**
   * 提交结算。退出与切仓库时自动备份可能正持仓库写锁：后端会让结算插队并等待
   * 最多 10 秒，残留的 `repository_busy` 再重试一次，仍失败才上报。
   */
  private finalizeWithBusyRetry(): Promise<SavePinBoardResult> {
    return pinBoardApi.finalizePinBoard(this.boardId, this.revision)
      .catch(async (error: unknown) => {
        if (!isRepositoryBusy(error)) throw error;
        diagnosticsLog(
          "info",
          `pin-board finalize busy, retrying once: boardId=${this.boardId}`,
        );
        await new Promise((resolve) => setTimeout(resolve, FINALIZE_BUSY_RETRY_DELAY_MS));
        return pinBoardApi.finalizePinBoard(this.boardId, this.revision);
      });
  }

  private stateKey(): string {
    return JSON.stringify(this.images.map(({ itemId, order, layer, deleted, points, uv }) => {
      const image = this.imageById.get(itemId);
      return {
        itemId,
        order,
        layer,
        deleted,
        points,
        uv,
        // 文字内容/样式/度量参与脏标记：只改字不动四边形时也必须保存。
        ...(image?.kind === "text" ? { text: pinBoardTextSignature(image) } : {}),
      };
    }));
  }

  private editableImages(): PinBoardEditableItem[] {
    return this.images.map((image) => {
      const { itemId, order, layer, deleted, points, uv } = image;
      const text = image.kind === "text"
        && pinBoardTextSignature(image) !== this.savedTextSignatures.get(itemId)
        ? {
          text: image.text,
          fontSize: image.fontSize,
          color: image.color,
          align: image.align,
          measuredWidth: image.width,
          measuredHeight: image.height,
        }
        : undefined;
      return {
        itemId,
        order,
        layer,
        deleted,
        points: cloneQuad(points),
        uv: cloneQuad(uv),
        ...(text ? { text } : {}),
      };
    });
  }

  private emitState() {
    if (this.destroyed || !this.loaded) return;
    this.canvas.classList.toggle("locked", this.locked);
    // 文字放置模式用 I 字型光标：落点即文字中心。
    this.canvas.classList.toggle("placing-text", this.placingText);
    // 画笔用笔尖光标，橡皮用一个 DOM 空心圆（见 `updateEraserRing`），其余工具保持十字。
    this.canvas.classList.toggle("tool-pen", this.tool === "pen");
    this.canvas.classList.toggle("tool-eraser", this.tool === "eraser");
    this.updateSelectionOverlay();
    this.updateEraserRing();
    this.persistSession();
    this.onState(this.interactionState);
  }

  private readonly autoSave = () => {
    void this.save();
  };

  private readonly visibilityChange = () => {
    if (document.visibilityState === "hidden") void this.save();
  };

  private readonly keyDown = (event: KeyboardEvent) => {
    if (!this.active || this.exporting) return;
    if (!shortcutCanHandle(event)) return;
    // 放置模式下只认 Escape 与保存：右键/Esc 取消，其余画板快捷键先停用。
    if (this.placingText) {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        this.cancelTextPlacement();
      } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void this.save();
      }
      return;
    }
    // 编辑会话期间所有画板快捷键停用：Ctrl+Z 留给 textarea 的原生撤销。
    if (this.textEdit) return;
    if (event.key === "F2"
      && !event.ctrlKey
      && !event.metaKey
      && !event.altKey
      && !event.shiftKey) {
      event.preventDefault();
      this.beginTextEdit();
      return;
    }
    if (event.key === "Delete") {
      event.preventDefault();
      this.finishActiveInteraction();
      this.deleteSelected();
      return;
    }
    if (!(event.ctrlKey || event.metaKey)) {
      if (!event.altKey && !event.shiftKey) {
        const tool = toolForShortcut(event.key);
        if (tool) {
          event.preventDefault();
          this.setTool(tool);
        }
      }
      return;
    }
    const key = event.key.toLowerCase();
    if (key === "s") {
      event.preventDefault();
      this.finishActiveInteraction();
      void this.save();
    } else if (key === "a" && !event.shiftKey) {
      event.preventDefault();
      this.selectAll();
    } else if (key === "z" && !event.shiftKey) {
      event.preventDefault();
      this.finishActiveInteraction();
      this.undo();
    } else if (key === "y" || (key === "z" && event.shiftKey)) {
      event.preventDefault();
      this.finishActiveInteraction();
      this.redo();
    }
  };

  private readonly resize = () => {
    if (this.destroyed) return;
    const rect = this.canvas.getBoundingClientRect();
    const cssWidth = Math.max(rect.width, 1);
    const cssHeight = Math.max(rect.height, 1);
    if (this.loaded
      && this.pendingViewportFit
      && cssWidth >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS
      && cssHeight >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS) {
      // 画布尺寸首次可用时执行打开适配；模块隐藏期间创建的 renderer 会在这里补做。
      this.applyContentFit(cssWidth, cssHeight);
    }
    if (this.preservedWorldUnitsPerCssPixel !== null) {
      this.viewport.height = this.clampViewportHeight(
        this.preservedWorldUnitsPerCssPixel * cssHeight,
      );
    } else if (this.loaded && this.cssHeight > 0 && cssHeight !== this.cssHeight) {
      this.viewport.height = this.clampViewportHeight(
        this.viewport.height * cssHeight / this.cssHeight,
      );
    }
    this.cssHeight = cssHeight;
    const deviceLimit = this.gpu.device.limits.maxTextureDimension2D as number;
    // The swap chain is another GPU allocation. Keep it at the same hard
    // ceiling as board textures, especially on high-DPI displays where a
    // modest CSS canvas can otherwise become a 16K backing surface.
    const maxDimension = Math.min(
      PIN_BOARD_CANVAS_DIMENSION_LIMIT,
      PIN_BOARD_NATIVE_TEXTURE_LIMIT,
      Number.isFinite(deviceLimit) ? Math.max(1, deviceLimit) : PIN_BOARD_NATIVE_TEXTURE_LIMIT,
    );
    const scale = Math.min(
      window.devicePixelRatio || 1,
      maxDimension / cssWidth,
      maxDimension / cssHeight,
    );
    const width = Math.max(1, Math.round(cssWidth * scale));
    const height = Math.max(1, Math.round(cssHeight * scale));
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    if (!this.loaded) return;
    this.refreshViewport(0);
    this.emitState();
  };

  private readonly pointerDown = (event: PointerEvent) => {
    if (event.button !== 0 && event.button !== 1) return;
    if (this.dragMode || this.exporting) return;
    // 文字放置模式：左键落点打开空白编辑框，中键仍可平移。
    if (this.placingText && event.button === 0) {
      const rect = this.canvas.getBoundingClientRect();
      const world = screenToWorld(event.clientX, event.clientY, rect, this.viewport);
      this.cursorWorld = world;
      this.placingText = false;
      this.canvas.classList.remove("placing-text");
      this.onContextMenu(null);
      if (!this.beginNewTextEdit(world)) this.emitState();
      event.preventDefault();
      return;
    }
    // 编辑器在 document 捕获阶段已经提交过；这里兜一次并阻止无效草稿继续拖动。
    // 中键是平移手势，不结束编辑会话。
    let closedTextEditor = false;
    if (this.textEdit && event.button !== 1) {
      const error = this.commitTextEdit();
      if (error) {
        this.onError(error);
        event.preventDefault();
        return;
      }
      // 这次按下已经关掉了编辑框：它不能被当成「再点一次进入编辑」。
      closedTextEditor = true;
    }
    this.onContextMenu(null);
    this.pressClient = [event.clientX, event.clientY];
    this.pressMoved = false;
    this.pressWasSelected = false;
    this.pressItemId = null;
    this.pressSuppressEdit = this.suppressClickEdit || closedTextEditor;
    this.suppressClickEdit = false;
    this.canvas.classList.remove("over-text");
    this.canvas.focus({ preventScroll: true });
    this.pointerX = event.clientX;
    this.pointerY = event.clientY;
    const rect = this.canvas.getBoundingClientRect();
    this.dragWorld = screenToWorld(event.clientX, event.clientY, rect, this.viewport);
    this.cursorWorld = this.dragWorld;
    this.eraserPointer = [event.clientX - rect.left, event.clientY - rect.top];
    this.updateEraserRing();

    if (event.button === 1 || this.locked) {
      this.dragMode = "pan";
    } else if (isDrawingTool(this.tool)) {
      // 绘制与擦除都与选中/框选互斥：工具不是 select 时，按下只可能是要画一笔或擦一条。
      if (this.tool === "eraser") {
        if (!this.beginErasing(this.dragWorld)) return;
      } else if (!this.beginDrawing(this.dragWorld, event)) return;
    } else if (this.rotationHandleHit(event.clientX, event.clientY)) {
      const selection = this.selectionWorldQuad();
      if (!selection) return;
      this.dragMode = "rotate";
      this.dragBefore = this.snapshot([...this.selectedIds]);
      this.rotateCenter = quadCenter(selection);
      this.rotateStartAngle = Math.atan2(
        this.dragWorld[1] - this.rotateCenter[1],
        this.dragWorld[0] - this.rotateCenter[0],
      );
    } else {
      const resizeHandle = this.resizeHandleAt(event.clientX, event.clientY);
      if (resizeHandle !== null) {
        const selection = this.selectionWorldQuad();
        if (!selection) return;
        this.dragMode = "scale";
        this.dragBefore = this.snapshot([...this.selectedIds]);
        this.scaleAnchor = selection[(resizeHandle + 2) % selection.length];
        this.scaleStartDistance = Math.max(
          Math.hypot(this.dragWorld[0] - this.scaleAnchor[0], this.dragWorld[1] - this.scaleAnchor[1]),
          1e-6,
        );
      } else {
        const hit = this.hitImage(this.dragWorld);
        // Ctrl/Cmd 与 Shift 都作为加选修饰键：按住点击单张图切换选中，按住在空白处
        // 拖拽则累加框选。
        const additiveSelection = isAdditiveSelectionModifier(event);
        if (additiveSelection && hit) {
          if (this.selectedIds.has(hit.itemId)) this.selectedIds.delete(hit.itemId);
          else {
            this.selectedIds.add(hit.itemId);
            this.bringToFront([hit.itemId]);
          }
          this.refreshSelectionVisuals();
          event.preventDefault();
          return;
        }

        const selection = this.selectionWorldQuad();
        const insideSelection = selection
          ? pointInQuad(this.dragWorld, selection)
          : false;
        if (hit && this.selectedIds.has(hit.itemId)) {
          this.pressWasSelected = true;
          this.pressItemId = hit.itemId;
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else if (hit) {
          this.pressItemId = hit.itemId;
          this.selectedIds.clear();
          this.selectedIds.add(hit.itemId);
          this.bringToFront([hit.itemId]);
          this.dragMode = "images";
          this.dragBefore = this.snapshot([hit.itemId]);
          this.refreshSelectionVisuals();
        } else if (insideSelection) {
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else {
          this.marqueeBaseIds.clear();
          if (additiveSelection) {
            // 累加式框选：保留当前选中作为基底，框到的新图在此基础上加入，
            // 因此可以连续多次框选，已选中的不会消失。
            for (const itemId of this.selectedIds) this.marqueeBaseIds.add(itemId);
          } else {
            this.selectedIds.clear();
          }
          this.dragMode = "marquee";
          this.marqueeWorld = [...this.dragWorld];
          this.updateMarqueeOverlay(this.boundsBetween(this.dragWorld, this.marqueeWorld));
          this.refreshSelectionVisuals();
        }
      }
    }

    this.dragPointerId = event.pointerId;
    this.canvas.setPointerCapture(event.pointerId);
    this.canvas.classList.add(this.dragMode ? DRAG_CURSOR_CLASS[this.dragMode] : "selecting");
    this.emitState();
    event.preventDefault();
  };

  private readonly pointerMove = (event: PointerEvent) => {
    if (this.exporting) return;
    const rect = this.canvas.getBoundingClientRect();
    this.cursorWorld = screenToWorld(event.clientX, event.clientY, rect, this.viewport);
    // 环形光标每个指针事件都要跟随：橡皮下用户靠它判断擦头大小与位置。
    this.eraserPointer = [event.clientX - rect.left, event.clientY - rect.top];
    this.updateEraserRing();
    if (!this.dragMode || this.dragPointerId !== event.pointerId) {
      const resizeHandle = this.resizeHandleAt(event.clientX, event.clientY);
      this.canvas.classList.toggle(
        "over-rotate-handle",
        !this.locked && this.rotationHandleHit(event.clientX, event.clientY),
      );
      this.canvas.classList.toggle(
        "over-resize-nwse",
        resizeHandle === 0 || resizeHandle === 2,
      );
      this.canvas.classList.toggle(
        "over-resize-nesw",
        resizeHandle === 1 || resizeHandle === 3,
      );
      // 悬停在「已选中的单条文字」上时显示文本光标，提示再点一次可编辑。
      const hovered = this.cursorWorld ? this.hitImage(this.cursorWorld) : null;
      this.canvas.classList.toggle(
        "over-text",
        !this.locked
          && !this.textEdit
          && !this.placingText
          && this.tool === "select"
          && hovered?.kind === "text"
          && this.selectedIds.size === 1
          && this.selectedIds.has(hovered.itemId),
      );
      return;
    }
    if (this.dragMode === "pan") {
      const unitsPerCssPixel = this.viewport.height / Math.max(rect.height, 1);
      this.viewport.centerX -= (event.clientX - this.pointerX) * unitsPerCssPixel;
      this.viewport.centerY += (event.clientY - this.pointerY) * unitsPerCssPixel;
      this.pointerX = event.clientX;
      this.pointerY = event.clientY;
      this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
      this.emitState();
    } else {
      const world = screenToWorld(event.clientX, event.clientY, rect, this.viewport);
      if (this.dragMode === "erase") {
        this.eraseAt(world);
      } else if (this.dragMode === "draw") {
        // 只更新草稿与 DOM 预览层：GPU 缓冲在落库后由 `applyMutationResult` 重建，
        // 不在每次 pointermove 重建整板笔迹。
        if (this.draft) {
          // Chromium 会把 pointermove 合并到约 60 Hz，快速划线因此丢点；被合并掉的
          // 中间点只在 `getCoalescedEvents()` 里。一次事件里的所有点共用一个画布矩形。
          for (const sample of coalescedStrokeSamples(event, rect, this.viewport)) {
            extendStrokeDraft(this.draft, sample.point, event.shiftKey, sample);
          }
          this.drawStrokePreview();
        }
      } else if (this.dragMode === "images") {
        if (!this.pressMoved && this.pressClient) {
          this.pressMoved = Math.hypot(
            event.clientX - this.pressClient[0],
            event.clientY - this.pressClient[1],
          ) > TEXT_EDIT_CLICK_TOLERANCE_CSS_PIXELS;
        }
        const deltaX = world[0] - this.dragWorld[0];
        const deltaY = world[1] - this.dragWorld[1];
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.itemId);
          if (image) image.points = translateQuad(snapshot.points, deltaX, deltaY);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
      } else if (this.dragMode === "rotate") {
        const delta = Math.atan2(
          world[1] - this.rotateCenter[1],
          world[0] - this.rotateCenter[0],
        ) - this.rotateStartAngle;
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.itemId);
          if (!image) continue;
          // Shift 吸附图片的**绝对角度**（0°/15°/30°…）而非本次拖拽的增量：
          // 增量吸附会让已带偏角的图片停在「偏角 + n×15°」，永远回不到 0°。
          const radians = event.shiftKey ? snapRotationDelta(snapshot.points, delta) : delta;
          image.points = rotateQuad(snapshot.points, radians, this.rotateCenter);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
      } else if (this.dragMode === "scale") {
        const distance = Math.hypot(
          world[0] - this.scaleAnchor[0],
          world[1] - this.scaleAnchor[1],
        );
        const minimumScale = this.dragBefore.reduce((minimum, snapshot) => {
          const width = Math.hypot(
            snapshot.points[1][0] - snapshot.points[0][0],
            snapshot.points[1][1] - snapshot.points[0][1],
          );
          const height = Math.hypot(
            snapshot.points[3][0] - snapshot.points[0][0],
            snapshot.points[3][1] - snapshot.points[0][1],
          );
          return Math.max(minimum, 0.001 / Math.max(Math.min(width, height), 1e-9));
        }, 0);
        const factor = Math.max(distance / this.scaleStartDistance, minimumScale);
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.itemId);
          if (image) image.points = scaleQuad(snapshot.points, factor, this.scaleAnchor);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
      } else {
        this.marqueeWorld = world;
        const bounds = this.boundsBetween(this.dragWorld, world);
        // 累加式框选：以 marqueeBaseIds 为基底并入框到的图片，因此可连续多次框选，
        // 已选中的不会消失，框到的新图加入。隐藏图层先剔掉，否则「框不到却选得上」。
        const next = marqueeSelection(
          this.images.filter((image) => isLayerVisible(this.visibleLayers, image.layer)),
          bounds,
          this.marqueeBaseIds,
          (image) => this.itemIntersectsMarquee(image, bounds),
        );
        // 框选矩形是独立的 DOM 叠加层，每个指针事件都要跟随；但选中集合未变时
        // （例如拖过空白区域或已选中的图片）无需重排、重绘与广播状态。
        this.updateMarqueeOverlay(bounds);
        if (!sameSelection(this.selectedIds, next)) {
          this.selectedIds.clear();
          for (const itemId of next) this.selectedIds.add(itemId);
          this.refreshSelectionVisuals();
        }
      }
    }
    event.preventDefault();
  };

  private readonly pointerUp = (event: PointerEvent) => {
    if (!this.dragMode || this.dragPointerId !== event.pointerId) return;
    if (this.dragMode === "draw") {
      // 抬笔的位置可能比最后一个 pointermove 更远（快速收笔时常见）：补上这一点，
      // 否则每笔都会少画最后几个像素。重合时 `extendStrokeDraft` 会合并掉。
      if (this.draft) {
        extendStrokeDraft(
          this.draft,
          screenToWorld(
            event.clientX,
            event.clientY,
            this.canvas.getBoundingClientRect(),
            this.viewport,
          ),
          event.shiftKey,
          { pressure: event.pressure, pointerType: event.pointerType },
        );
      }
      this.commitDrawing();
    } else this.finishInteraction();
    event.preventDefault();
  };

  /**
   * 指针被系统收回（触摸被接管、窗口失焦等）：这一笔按取消处理，
   * 与 `pointerup` 的「落库」区分开——事件已经不可信，不该拿半条轨迹建条目。
   */
  private readonly pointerCancel = (event: PointerEvent) => {
    if (!this.dragMode || this.dragPointerId !== event.pointerId) return;
    // 绘制按取消处理（事件已不可信，不该拿半条轨迹建条目）；擦除与 `pointerup` 走
    // 同一条提交路径——擦掉的内容不还原，否则用户会看到「擦掉的东西又回来了」。
    if (this.dragMode === "draw") this.cancelDrawing();
    else this.finishInteraction(false);
    event.preventDefault();
  };

  /** 指针离开画布：环形光标没有可跟随的位置，直接隐藏。 */
  private readonly pointerLeave = () => {
    this.eraserPointer = null;
    this.canvas.classList.remove("over-text");
    this.updateEraserRing();
  };

  private finishInteraction(allowTextEdit = true) {
    // 绘制的中断路径（锁定、切画板、失焦）一律按取消处理：没有落库就什么也没发生。
    if (this.dragMode === "draw") {
      this.cancelDrawing();
      return;
    }
    // 第二次点击同一条已选中的文字、且按下到抬起没有拖动才进入编辑。
    const editItemId = allowTextEdit
      && this.dragMode === "images"
      && !this.pressMoved
      && !this.pressSuppressEdit
      && this.pressWasSelected
      && this.pressItemId !== null
      && this.selectedIds.size === 1
      && this.selectedIds.has(this.pressItemId)
      ? this.pressItemId
      : null;
    // 按下点就是光标落点；没有拖动时 `dragWorld` 仍是按下时的世界坐标。
    const editClickPoint: Point | null = editItemId === null ? null : [...this.dragWorld] as Point;
    const pointerId = this.dragPointerId;
    if (this.dragMode === "erase") {
      // 一次拖拽 = 一条撤销记录：`before` 是按下时整板可见笔迹的快照。
      const erased = [...this.eraseHitIds];
      if (erased.length > 0) {
        this.pushHistory(this.dragBefore, this.snapshot(erased));
        this.afterModelChange(erased);
      }
    } else if ((this.dragMode === "images" || this.dragMode === "rotate" || this.dragMode === "scale")
      && this.dragBefore.length > 0) {
      const after = this.snapshot(this.dragBefore.map((image) => image.itemId));
      this.pushHistory(this.dragBefore, after);
      this.afterModelChange(after.map((image) => image.itemId));
    } else if (this.dragMode === "pan") {
      this.scheduleLoads(0);
    } else if (this.dragMode === "marquee") {
      this.updateMarqueeOverlay(null);
      this.bringToFront([...this.selectedIds]);
    }
    if (pointerId !== null && this.canvas.hasPointerCapture(pointerId)) {
      this.canvas.releasePointerCapture(pointerId);
    }
    this.dragMode = null;
    this.dragPointerId = null;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    this.resetEraseState();
    this.canvas.classList.remove(
      "dragging",
      "moving-image",
      "rotating-image",
      "scaling-image",
      "selecting",
      "drawing",
      "erasing",
      "over-rotate-handle",
      "over-resize-nwse",
      "over-resize-nesw",
    );
    this.resetPressState();
    this.emitState();
    if (editItemId !== null) this.beginTextEdit(editItemId, editClickPoint ?? undefined);
  }

  /**
   * 开始擦除。按下时就判一次命中：环形光标给出的是「这里能点到什么」的预期，
   * 单击笔迹却毫无反应会像失灵；继续拖拽则在 `pointerMove` 里增量擦。
   *
   * `before` 取**全部可见笔迹**的快照而不是逐条记录：一轮拖拽可能擦掉任意多条，
   * 撤销必须一次全复原。画板里没有可见笔迹时不进入拖拽，什么状态都不改。
   */
  private beginErasing(world: Point): boolean {
    if (!this.loaded || this.tool !== "eraser") return false;
    const strokes = this.erasableStrokes();
    if (strokes.length === 0) return false;
    this.selectedIds.clear();
    this.refreshSelectionVisuals();
    this.dragBefore = this.snapshot(strokes.map((stroke) => stroke.itemId));
    this.resetEraseState();
    this.eraseCandidates = strokes;
    this.dragMode = "erase";
    this.eraseAt(world);
    return true;
  }

  /** 清掉一轮擦除的临时状态：几何重读、销毁与交互结束时都要清，避免留着上一轮的引用。 */
  private resetEraseState() {
    this.eraseHitIds.clear();
    this.eraseFrom = null;
    this.eraseCandidates = [];
  }

  /** 可被擦除的笔迹：未删除、所在图层可见。位图与文字不参与擦除。 */
  private erasableStrokes(): PinBoardStrokeItem[] {
    return this.images.filter((image): image is PinBoardStrokeItem => (
      image.kind === "stroke"
      && !image.deleted
      && image.available
      && isLayerVisible(this.visibleLayers, image.layer)
    ));
  }

  /**
   * 擦掉擦头**扫过**的笔迹，返回本轮新擦掉的条目 id。
   *
   * 判的是「上一点 → 这一步」这段位移扫过的区域，而不是只判当前点：指针事件被合并到
   * 约 60–120 Hz，快速擦动时相邻两帧可能隔几十个像素，细笔迹正好落在两次采样之间就
   * 整条都擦不掉（表现为「反复擦没反应、点一下反而擦掉」）。见 `strokeHitBySweep`。
   *
   * 逐条先按「墨迹外框 + 擦头半径」与这一段的包围盒预筛，命中才解析几何；世界折线按
   * 条目缓存。同一轮里已擦掉的条目不再重复处理。
   */
  private eraseAt(world: Point): number[] {
    const radius = this.strokeWidthValue / 2;
    const path = this.eraseFrom ? [this.eraseFrom, world] : [world];
    this.eraseFrom = world;
    const base = this.boundsBetween(path[0], path[path.length - 1]);
    const sweep: Bounds = {
      minX: base.minX - radius,
      minY: base.minY - radius,
      maxX: base.maxX + radius,
      maxY: base.maxY + radius,
    };
    const erased: number[] = [];
    for (const stroke of this.eraseCandidates) {
      if (this.eraseHitIds.has(stroke.itemId)) continue;
      const geometry = this.strokeGeometries.get(stroke.itemId);
      if (!geometry) continue;
      if (!intersects(strokeHitBounds(stroke, radius), sweep)) continue;
      if (!strokeHitBySweep(this.strokeOutlines(stroke, geometry), path, radius)) continue;
      stroke.deleted = true;
      this.eraseHitIds.add(stroke.itemId);
      erased.push(stroke.itemId);
    }
    // 只在新命中时重建整板缓冲：每个指针事件都重算一次会把大画板拖垮。
    if (erased.length > 0) this.afterLiveChange(erased);
    return erased;
  }

  /** 条目几何的世界折线（带缓存）；键含几何版本与变换签名，任一变化即失效。 */
  private strokeOutlines(
    item: PinBoardStrokeItem,
    geometry: PinBoardStrokeGeometry,
  ): StrokeWorldOutline[] {
    const signature = strokeOutlineSignature(item, this.strokeVersion);
    const cached = this.strokeOutlineCache.get(item.itemId);
    if (cached && cached.signature === signature) return cached.outlines;
    const outlines = buildStrokeWorldOutlines(item, geometry);
    this.strokeOutlineCache.set(item.itemId, { signature, outlines });
    return outlines;
  }

  /**
   * 开始绘制。选中态先清掉：绘制中不该同时显示变换手柄；落库后的新条目只在
   * `select` 工具下自动选中（见 `applyMutationResult`），绘制工具下保持干净。
   */
  private beginDrawing(world: Point, event?: PointerEvent): boolean {
    if (!this.loaded || !isStrokeTool(this.tool)) return false;
    this.selectedIds.clear();
    this.refreshSelectionVisuals();
    this.draft = beginStrokeDraft(this.tool, world, {
      pressure: event?.pressure,
      pointerType: event?.pointerType,
    });
    this.dragMode = "draw";
    this.drawStrokePreview();
    return true;
  }

  /**
   * 结束绘制：把草稿编码成落库请求并交给控制器。
   *
   * 预览层**先不清理**：落库要经过保存、创建与重读几何，立刻清掉会闪一下空白；
   * 预览留到 `clearPendingStrokePreview` 被调用（新笔迹进缓冲或提交失败）。
   * 拖拽距离不足（误点）或编码失败时按取消处理，不产生条目。
   */
  private commitDrawing() {
    const draft = this.draft;
    const request = draft
      ? resolveStrokeDraft(
        draft,
        this.strokeWidthValue,
        MIN_STROKE_DRAG_CSS_PIXELS * this.worldUnitsPerCssPixel(),
        // 采样管道的容差与点距按绘制时的缩放换算：画的时候多细，存下来的密度就有多细。
        this.devicePixelsPerWorldUnit(),
      )
      : null;
    if (!draft || !request) {
      this.cancelDrawing();
      return;
    }
    this.releaseDrawingPointer();
    this.onStrokeCommit({ ...request, color: this.strokeColor });
  }

  /** 交还指针与光标类，保留草稿（提交路径要留着预览）。 */
  private releaseDrawingPointer() {
    const pointerId = this.dragPointerId;
    this.dragMode = null;
    this.dragPointerId = null;
    this.canvas.classList.remove("drawing");
    if (pointerId !== null && this.canvas.hasPointerCapture(pointerId)) {
      this.canvas.releasePointerCapture(pointerId);
    }
    this.emitState();
  }

  private clearStrokePreview() {
    clearStrokePreviewLayer(this.drawOverlay);
  }

  /**
   * 把当前草稿画到 DOM 叠加层上。坐标直接用世界单位：把变换设成「世界 → 设备像素」
   * 之后，线宽与形状尺寸天然与落库后的渲染一致，不必两处各写一份换算。
   */
  private drawStrokePreview() {
    if (!this.drawOverlay) return;
    paintStrokePreview({
      overlay: this.drawOverlay,
      canvas: this.canvas,
      viewport: this.viewport,
      draft: this.loaded ? this.draft : null,
      color: this.strokeColor,
      width: this.strokeWidthValue,
    });
  }

  private readonly wheel = (event: WheelEvent) => {
    if (this.dragMode || this.exporting) {
      event.preventDefault();
      return;
    }
    const point = screenToWorld(
      event.clientX,
      event.clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
    this.cursorWorld = point;
    this.zoomAt(event.deltaY < 0 ? 1 / 1.15 : 1.15, event.clientX, event.clientY);
    event.preventDefault();
  };

  private readonly contextMenu = (event: MouseEvent) => {
    event.preventDefault();
    // 放置模式下右键取消，不打开画板菜单。
    if (this.placingText) {
      this.cancelTextPlacement();
      this.onContextMenu(null);
      return;
    }
    if (this.dragMode || this.exporting) {
      this.onContextMenu(null);
      return;
    }
    if (this.textEdit && !this.settleTextEdit()) {
      this.onContextMenu(null);
      return;
    }
    const point = screenToWorld(
      event.clientX,
      event.clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
    this.cursorWorld = point;
    const hit = this.hitImage(point);
    if (!this.locked && hit && !this.selectedIds.has(hit.itemId)) {
      this.selectedIds.clear();
      this.selectedIds.add(hit.itemId);
      this.bringToFront([hit.itemId]);
      this.refreshSelectionVisuals();
    }
    const selected = this.selectedImages();
    this.onContextMenu({
      clientX: event.clientX,
      clientY: event.clientY,
      selectedCount: selected.length,
      arrangeableCount: selected.filter((image) => image.kind === "bitmap").length,
      // 阵列排序与单条导出当前都只认位图，两处口径因此一致；分开写是因为语义不同。
      exportableCount: selected.filter((image) => image.kind === "bitmap").length,
      selectionHasStroke: selected.some((image) => image.kind === "stroke"),
      singleTextSelected: selected.length === 1 && selected[0].kind === "text",
      targetImage: Boolean(hit && !this.locked),
      canEditBoard: !this.locked,
      canEditSelection: !this.locked && this.selectedIds.size > 0,
    });
  };

  private boundsBetween(start: Point, end: Point): Bounds {
    return {
      minX: Math.min(start[0], end[0]),
      minY: Math.min(start[1], end[1]),
      maxX: Math.max(start[0], end[0]),
      maxY: Math.max(start[1], end[1]),
    };
  }

  private rotationHandleHit(clientX: number, clientY: number): boolean {
    const handle = this.rotationHandlePoint();
    if (this.locked || !handle) return false;
    const rect = this.canvas.getBoundingClientRect();
    return Math.hypot(handle[0] + rect.left - clientX, handle[1] + rect.top - clientY)
      <= TRANSFORM_HANDLE_RADIUS;
  }

  private resizeHandleAt(clientX: number, clientY: number): number | null {
    if (this.locked || !this.selectionScreenQuad) return null;
    const rect = this.canvas.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    const index = this.selectionScreenQuad.findIndex((point) => Math.hypot(point[0] - x, point[1] - y)
      <= TRANSFORM_HANDLE_RADIUS);
    return index >= 0 ? index : null;
  }

  private rotationHandlePoint(): Point | null {
    const quad = this.selectionScreenQuad;
    if (!quad) return null;
    const center = quadCenter(quad);
    const top: Point = [
      (quad[0][0] + quad[1][0]) / 2,
      (quad[0][1] + quad[1][1]) / 2,
    ];
    const direction: Point = [top[0] - center[0], top[1] - center[1]];
    const length = Math.hypot(direction[0], direction[1]);
    if (length < 1e-6) return top;
    return [top[0] + direction[0] / length * ROTATION_HANDLE_OFFSET, top[1] + direction[1] / length * ROTATION_HANDLE_OFFSET];
  }

  private updateSelectionOverlay() {
    const selected = this.selectedImages();
    const bounds = this.locked || this.textEdit ? null : imagesBounds(selected);
    if (!bounds || selected.length === 0) {
      this.selectionScreenQuad = null;
      this.selectionElement.hidden = true;
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    const worldQuad = this.selectionWorldQuad();
    if (!worldQuad) return;
    const screenQuad = worldQuad.map((point) => worldToScreen(point, rect, this.viewport)) as Quad;
    this.selectionScreenQuad = screenQuad;
    this.selectionElement.hidden = false;
    const width = Math.max(Math.hypot(screenQuad[1][0] - screenQuad[0][0], screenQuad[1][1] - screenQuad[0][1]), 1);
    const height = Math.max(Math.hypot(screenQuad[3][0] - screenQuad[0][0], screenQuad[3][1] - screenQuad[0][1]), 1);
    const angle = Math.atan2(screenQuad[1][1] - screenQuad[0][1], screenQuad[1][0] - screenQuad[0][0]);
    this.selectionElement.style.transformOrigin = "0 0";
    this.selectionElement.style.transform = `translate(${screenQuad[0][0]}px, ${screenQuad[0][1]}px) rotate(${angle}rad)`;
    this.selectionElement.style.width = `${width}px`;
    this.selectionElement.style.height = `${height}px`;
  }

  private selectionWorldQuad(): Quad | null {
    const selected = this.selectedImages();
    if (selected.length === 1) return cloneQuad(selected[0].points);
    const bounds = imagesBounds(selected);
    return bounds ? [
      [bounds.minX, bounds.maxY],
      [bounds.maxX, bounds.maxY],
      [bounds.maxX, bounds.minY],
      [bounds.minX, bounds.minY],
    ] : null;
  }

  private updateMarqueeOverlay(bounds: Bounds | null) {
    if (!bounds) {
      this.marqueeElement.hidden = true;
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    const topLeft = worldToScreen([bounds.minX, bounds.maxY], rect, this.viewport);
    const bottomRight = worldToScreen([bounds.maxX, bounds.minY], rect, this.viewport);
    const left = Math.min(topLeft[0], bottomRight[0]);
    const top = Math.min(topLeft[1], bottomRight[1]);
    this.marqueeElement.hidden = false;
    this.marqueeElement.style.transform = `translate(${left}px, ${top}px)`;
    this.marqueeElement.style.width = `${Math.abs(bottomRight[0] - topLeft[0])}px`;
    this.marqueeElement.style.height = `${Math.abs(bottomRight[1] - topLeft[1])}px`;
  }

  private refreshSelectionVisuals() {
    this.sortVisibleImages();
    this.draw();
    this.emitState();
  }

  private zoomAt(factor: number, clientX?: number, clientY?: number) {
    const rect = this.canvas.getBoundingClientRect();
    const anchorX = clientX ?? rect.left + rect.width / 2;
    const anchorY = clientY ?? rect.top + rect.height / 2;
    const point = screenToWorld(anchorX, anchorY, rect, this.viewport);
    const height = this.clampViewportHeight(this.viewport.height * factor);
    const ratioX = (anchorX - rect.left) / Math.max(rect.width, 1);
    const ratioY = (anchorY - rect.top) / Math.max(rect.height, 1);
    const width = height * Math.max(rect.width, 1) / Math.max(rect.height, 1);
    this.viewport.height = height;
    this.viewport.centerX = point[0] - (ratioX - 0.5) * width;
    this.viewport.centerY = point[1] + (ratioY - 0.5) * height;
    this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
    this.emitState();
  }

  private worldUnitsPerCssPixel(): number {
    return this.viewport.height / Math.max(this.cssHeight, 1);
  }

  private clampViewportHeight(height: number): number {
    return Math.min(MAX_VIEWPORT_HEIGHT, Math.max(MIN_VIEWPORT_HEIGHT, height));
  }

  /**
   * 每层未删除条目数（下标即 layer，含标注层）。
   *
   * 隐藏层里的条目**照样计入**：数字是「这一层里有多少东西」，不是「现在看得见多少」，
   * 否则开关一次图层数字就跳一下，用户会以为内容被删了。标注层的数字界面不显示，
   * 但数据仍然给全，免得以后要用时再造一套口径。
   */
  private layerCounts(): number[] {
    const counts = new Array<number>(PIN_BOARD_LAYER_COUNT).fill(0);
    for (const image of this.images) {
      if (image.deleted) continue;
      // 越界或非整数的 layer 直接跳过：宁可少一个数字，也不要写进错误的行。
      if (!Number.isInteger(image.layer) || image.layer < 0 || image.layer >= counts.length) {
        continue;
      }
      counts[image.layer] += 1;
    }
    return counts;
  }

  /** 画板内全部未删除、且所在图层可见的条目轴对齐最小包围框（旋转条目按外接矩形计算）。 */
  private contentBounds(): Bounds | null {
    return imagesBounds(this.images.filter((image) => (
      !image.deleted && isLayerVisible(this.visibleLayers, image.layer)
    )));
  }

  /** 按图片包围框适配视图；画板内没有图片时回落到默认视图。 */
  private applyContentFit(cssWidth: number, cssHeight: number) {
    this.pendingViewportFit = false;
    const bounds = this.contentBounds();
    if (!bounds) {
      this.viewport.centerX = DEFAULT_VIEWPORT.centerX;
      this.viewport.centerY = DEFAULT_VIEWPORT.centerY;
      this.viewport.height = this.clampViewportHeight(
        this.defaultWorldUnitsPerCssPixel * cssHeight,
      );
      return;
    }
    const fitted = fitViewportToBounds(bounds, cssWidth, cssHeight);
    this.viewport.centerX = fitted.centerX;
    this.viewport.centerY = fitted.centerY;
    this.viewport.height = this.clampViewportHeight(fitted.height);
  }

  private selectedImages(): PinBoardItem[] {
    // 隐藏层里的条目不算选中：删除、导出、变换、图层切换都从这里取条目。
    return this.images.filter((image) => (
      !image.deleted
      && isLayerVisible(this.visibleLayers, image.layer)
      && this.selectedIds.has(image.itemId)
    ));
  }

  private bringToFront(itemIds: number[]) {
    if (itemIds.length === 0) return;
    const promotedIds = new Set(itemIds);
    const ordered = [...this.images].sort((left, right) => (
      left.order - right.order || left.itemId - right.itemId
    ));
    // 笔迹固定占标注层，**只在笔迹内部**提前：跨层先后关系由 layer 决定，
    // 把笔迹提到整板最前只会白白改写每个条目的 sort_order（一次点击就脏整块画板）。
    const groups = [
      ordered.filter((image) => image.kind !== "stroke"),
      ordered.filter((image) => image.kind === "stroke"),
    ];
    let changed = false;
    for (const group of groups) {
      const promoted = group.filter((image) => promotedIds.has(image.itemId));
      if (promoted.length === 0) continue;
      const rest = group.filter((image) => !promotedIds.has(image.itemId));
      // 只重排本组自己的 order 值（同一个多重集换个顺序），不与另一组抢号，
      // 因此另一组的条目一个字段都不会动。
      const slots = group.map((image) => image.order).sort((left, right) => left - right);
      [...promoted, ...rest].forEach((image, index) => {
        changed ||= image.order !== slots[index];
        image.order = slots[index];
      });
    }
    if (changed) {
      this.dirty = this.stateKey() !== this.savedStateKey;
      if (this.dirty) this.scheduleAutosave();
      this.sortVisibleImages();
      this.draw();
    }
  }

  /**
   * 点选命中：笔迹按墨迹判，位图与文字按四角框判。
   *
   * 位图不读回纹理就不知道 alpha，文字框本来就贴合字形；只有笔迹能把几何精确展开成
   * 折线。于是椭圆、对角线笔迹、矩形内部的大片空白不再「点中就选中」——先用条目
   * 外框（外扩容差）预筛，再逐段算距离。仍从最上层往下找第一个命中项。
   */
  private hitImage(point: Point): PinBoardItem | null {
    const tolerance = this.hitTolerance();
    for (let index = this.visibleImages.length - 1; index >= 0; index -= 1) {
      const image = this.visibleImages[index];
      if (image.kind !== "stroke") {
        if (pointInQuad(point, image.points)) return image;
        continue;
      }
      const bounds = strokeHitBounds(image, tolerance);
      if (point[0] < bounds.minX || point[0] > bounds.maxX
        || point[1] < bounds.minY || point[1] > bounds.maxY) continue;
      const geometry = this.strokeGeometries.get(image.itemId);
      // 几何还没读回来（刚打开画板）时退回包围框：宁可选中，也不要「点了没反应」。
      if (!geometry) {
        if (pointInQuad(point, image.points)) return image;
        continue;
      }
      if (strokeHitByCircle(this.strokeOutlines(image, geometry), point, tolerance)) return image;
    }
    return null;
  }

  /**
   * 框选命中：与点选同口径，笔迹按墨迹判。
   *
   * 两次便宜判断挡在分段求交之前：外框（外扩容差）与框选矩形不相交 → 直接否；框选
   * 矩形完全罩住外框 → 直接是。后者把「一大片区域整体框住」这个最常见的操作从
   * 「每条笔迹逐段求交」降成一次比较。
   */
  private itemIntersectsMarquee(image: PinBoardItem, bounds: Bounds): boolean {
    if (image.kind !== "stroke") return quadIntersectsBounds(image.points, bounds);
    const tolerance = this.hitTolerance();
    const padded = strokeHitBounds(image, tolerance);
    if (padded.maxX < bounds.minX || padded.minX > bounds.maxX
      || padded.maxY < bounds.minY || padded.minY > bounds.maxY) return false;
    if (bounds.minX <= padded.minX && bounds.maxX >= padded.maxX
      && bounds.minY <= padded.minY && bounds.maxY >= padded.maxY) return true;
    const geometry = this.strokeGeometries.get(image.itemId);
    if (!geometry) return quadIntersectsBounds(image.points, bounds);
    return strokeIntersectsBounds(this.strokeOutlines(image, geometry), bounds, tolerance);
  }

  /**
   * 点选与框选的最小命中容差（世界单位）：细线只按半个线宽判会点不中，见
   * `STROKE_HIT_TOLERANCE_CSS_PIXELS`。缩放后容差按 CSS 像素换算，屏幕上恒定。
   */
  private hitTolerance(): number {
    return STROKE_HIT_TOLERANCE_CSS_PIXELS * this.worldUnitsPerCssPixel();
  }

  private snapshot(itemIds: number[]): ImageSnapshot[] {
    return itemIds.flatMap((itemId) => {
      const image = this.imageById.get(itemId);
      if (!image) return [];
      return [{
        itemId,
        deleted: image.deleted,
        layer: image.layer,
        points: cloneQuad(image.points),
        uv: cloneQuad(image.uv),
        ...(image.kind === "text"
          ? {
            text: {
              text: image.text,
              fontSize: image.fontSize,
              color: image.color,
              align: image.align,
              width: image.width,
              height: image.height,
            },
          }
          : {}),
      }];
    });
  }

  private restore(snapshots: ImageSnapshot[]) {
    for (const snapshot of snapshots) {
      const image = this.imageById.get(snapshot.itemId);
      if (!image) continue;
      const textBefore = image.kind === "text" ? pinBoardTextSignature(image) : "";
      image.deleted = snapshot.deleted;
      image.layer = snapshot.layer;
      image.points = cloneQuad(snapshot.points);
      image.uv = cloneQuad(snapshot.uv);
      if (image.kind === "text" && snapshot.text) {
        image.text = snapshot.text.text;
        image.fontSize = snapshot.text.fontSize;
        image.color = snapshot.text.color;
        image.align = snapshot.text.align;
        image.width = snapshot.text.width;
        image.height = snapshot.text.height;
        if (pinBoardTextSignature(image) !== textBefore) {
          this.invalidateTextTexture(image.itemId);
        }
      }
    }
  }

  private commitMutation(
    mutate: (image: PinBoardItem) => void,
    mergeKey?: string,
  ) {
    if (this.locked || this.textEdit || this.selectedIds.size === 0) return;
    const itemIds = [...this.selectedIds];
    const before = this.snapshot(itemIds);
    for (const itemId of itemIds) {
      const image = this.imageById.get(itemId);
      if (image) mutate(image);
    }
    const after = this.snapshot(itemIds);
    this.pushHistory(before, after, mergeKey);
    this.afterModelChange(itemIds);
  }

  private pushHistory(before: ImageSnapshot[], after: ImageSnapshot[], mergeKey?: string) {
    if (JSON.stringify(before) === JSON.stringify(after)) return;
    const timestamp = performance.now();
    const previous = this.history[this.historyIndex - 1];
    if (mergeKey
      && this.historyIndex === this.history.length
      && previous?.mergeKey === mergeKey
      && timestamp - previous.timestamp <= HISTORY_MERGE_MS) {
      previous.after = after;
      previous.timestamp = timestamp;
      return;
    }
    this.history.splice(this.historyIndex);
    this.history.push({ before, after, mergeKey, timestamp });
    if (this.history.length > HISTORY_LIMIT) this.history.shift();
    this.historyIndex = this.history.length;
  }

  private afterLiveChange(itemIds: number[]) {
    this.dirty = true;
    this.syncGeometryBuffers(itemIds);
    this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
    this.emitState();
  }

  private afterModelChange(itemIds: number[]) {
    for (const itemId of [...this.selectedIds]) {
      if (this.imageById.get(itemId)?.deleted) this.selectedIds.delete(itemId);
    }
    this.dirty = this.stateKey() !== this.savedStateKey;
    if (this.dirty) this.scheduleAutosave();
    else this.cancelAutosave();
    this.syncGeometryBuffers(itemIds);
    this.refreshViewport(0);
    this.emitState();
  }

  private scheduleAutosave() {
    if (!this.autosaveEnabled || this.destroyed) return;
    if (this.autosaveTimer !== null) window.clearTimeout(this.autosaveTimer);
    this.autosaveTimer = window.setTimeout(() => {
      this.autosaveTimer = null;
      void this.save();
    }, AUTOSAVE_DELAY_MS);
  }

  private cancelAutosave() {
    if (this.autosaveTimer !== null) {
      window.clearTimeout(this.autosaveTimer);
      this.autosaveTimer = null;
    }
  }

  /** 设置页开关：关闭时取消挂起的自动保存，开启时若有未保存编辑立即排程。 */
  setAutosaveEnabled(enabled: boolean) {
    if (this.autosaveEnabled === enabled) return;
    this.autosaveEnabled = enabled;
    if (!enabled) {
      this.cancelAutosave();
    } else if (this.dirty) {
      this.scheduleAutosave();
    }
  }

  private syncGeometryBuffers(itemIds: number[]) {
    let strokeMoved = false;
    for (const itemId of itemIds) {
      const image = this.imageById.get(itemId);
      // 笔迹的几何由整板缓冲承载：变换后让签名失效，下一次 refreshViewport 重建。
      if (image?.kind === "stroke") {
        strokeMoved = true;
        continue;
      }
      const loaded = this.textures.get(itemId) ?? this.previewTextures.get(itemId);
      if (!loaded || !image) continue;
      this.gpu.device.queue.writeBuffer(
        loaded.vertexBuffer,
        0,
        vertexData(image, loaded.uvScale ?? [1, 1]),
      );
      this.gpu.device.queue.writeBuffer(loaded.outlineBuffer, 0, outlineData(image));
    }
    if (strokeMoved) this.strokeSignature = "";
  }

  private refreshViewport(loadDelay: number) {
    if (this.destroyed || this.gpuUnavailable || !this.loaded || this.canvas.width < 1 || this.canvas.height < 1) return;
    this.generation += 1;
    const bounds = viewportBounds(this.viewport, this.canvas.width, this.canvas.height);
    this.visibleImages = this.images.filter((image) => (
      this.isDrawable(image) && intersects(imageBounds(image), bounds)
    ));
    this.sortVisibleImages();
    const now = performance.now();
    for (const image of this.visibleImages) {
      const loaded = this.textures.get(image.itemId);
      if (loaded) loaded.lastUsed = now;
      const preview = this.previewTextures.get(image.itemId);
      if (preview) preview.lastUsed = now;
    }
    this.gpu.device.queue.writeBuffer(
      this.gpu.viewportBuffer,
      0,
      new Float32Array([
        bounds.minX,
        bounds.minY,
        bounds.maxX - bounds.minX,
        bounds.maxY - bounds.minY,
        // 笔迹着色器的抗锯齿羽化按「设备像素 / 世界单位」换算。
        this.devicePixelsPerWorldUnit(),
        0,
        0,
        0,
      ]),
    );
    this.syncStrokeBuffers();
    this.evictTextures();
    this.draw();
    // 视口变化时预览层要跟着重算（缩放、窗口尺寸变化都走这里）。
    if (this.draft) this.drawStrokePreview();
    this.scheduleLoads(loadDelay);
  }

  private draw() {
    if (this.destroyed || this.gpuUnavailable) {
      this.clearPlaceholderOverlay();
      return;
    }
    try {
      const encoder = this.gpu.device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: this.gpu.context.getCurrentTexture().createView(),
          clearValue: clearColor(this.canvas),
          loadOp: "clear",
          storeOp: "store",
        }],
      });
      pass.setPipeline(this.gpu.pipeline);
      pass.setBindGroup(0, this.gpu.viewportBindGroup);
      for (const image of this.visibleImages) {
        // 正在编辑的文字由 DOM 编辑框代替，不画 GPU 纹理，避免重影。
        if (this.textEdit?.itemId === image.itemId) continue;
        const loaded = this.textures.get(image.itemId) ?? this.previewTextures.get(image.itemId);
        if (!loaded) continue;
        pass.setBindGroup(1, loaded.bindGroup);
        pass.setVertexBuffer(0, loaded.vertexBuffer);
        pass.draw(6);
      }
      // 第二趟：笔迹固定在最上层，整板合并成一次 indexed draw，不与图片交错。
      if (this.strokeIndexCount > 0 && this.strokeVertexBuffer && this.strokeIndexBuffer) {
        pass.setPipeline(this.gpu.strokePipeline);
        pass.setBindGroup(0, this.gpu.viewportBindGroup);
        pass.setVertexBuffer(0, this.strokeVertexBuffer);
        pass.setIndexBuffer(this.strokeIndexBuffer, "uint32");
        pass.drawIndexed(this.strokeIndexCount);
      }
      if (this.selectedIds.size > 0) {
        pass.setPipeline(this.gpu.outlinePipeline);
        pass.setBindGroup(0, this.gpu.viewportBindGroup);
        for (const image of this.visibleImages) {
          if (!this.selectedIds.has(image.itemId)) continue;
          if (this.textEdit?.itemId === image.itemId) continue;
          if (image.kind === "stroke") {
            const slot = this.strokeOutlineIndex.get(image.itemId);
            if (slot === undefined || !this.strokeOutlineBuffer) continue;
            pass.setVertexBuffer(0, this.strokeOutlineBuffer);
            pass.draw(STROKE_OUTLINE_VERTICES, 1, slot * STROKE_OUTLINE_VERTICES);
            continue;
          }
          const loaded = this.textures.get(image.itemId) ?? this.previewTextures.get(image.itemId);
          if (!loaded) continue;
          pass.setVertexBuffer(0, loaded.outlineBuffer);
          pass.draw(5);
        }
      }
      pass.end();
      this.gpu.device.queue.submit([encoder.finish()]);
      this.drawErrorReported = false;
      this.updatePlaceholderOverlay();
    } catch (error) {
      if (!this.destroyed && !this.drawErrorReported) {
        this.drawErrorReported = true;
        this.onError(`画布渲染失败：${errorMessage(error)}`);
      }
    }
  }

  /**
   * 同步「纹理未就绪」占位层：为可见但尚无纹理（缩略图或高清）的图片铺一块
   * 灰底方框标示图片范围，中心配一个动态加载图标。加载失败的图片改显静态
   * 告警图标，避免永久转圈。节点按 itemId 复用，只在集合变化时增删。
   */
  private updatePlaceholderOverlay() {
    const layer = this.placeholderElement;
    if (!layer || this.destroyed || this.gpuUnavailable || !this.loaded) {
      this.clearPlaceholderOverlay();
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1) {
      this.clearPlaceholderOverlay();
      return;
    }
    const pending = new Map<number, { quad: Quad; failed: boolean }>();
    for (const image of this.visibleImages) {
      // 笔迹永远不需要纹理，不该出现「正在加载」或「加载失败」占位框。
      if (image.kind === "stroke") continue;
      // 正在编辑的文字由 DOM 编辑框承载，不铺占位框。
      if (this.textEdit?.itemId === image.itemId) continue;
      if (this.textures.has(image.itemId) || this.previewTextures.has(image.itemId)) continue;
      pending.set(image.itemId, {
        quad: image.points,
        failed: this.failedTextureDimensions.has(image.itemId),
      });
    }
    for (const [itemId, node] of this.placeholderNodes) {
      if (pending.has(itemId)) continue;
      node.remove();
      this.placeholderNodes.delete(itemId);
    }
    for (const [itemId, entry] of pending) {
      let node = this.placeholderNodes.get(itemId);
      if (!node) {
        node = document.createElement("div");
        node.className = "pin-board-placeholder";
        const spinner = document.createElement("span");
        spinner.className = "pin-board-placeholder-spinner";
        const failed = document.createElement("span");
        failed.className = "pin-board-placeholder-failed";
        failed.textContent = "!";
        node.append(spinner, failed);
        layer.appendChild(node);
        this.placeholderNodes.set(itemId, node);
      }
      node.classList.toggle("failed", entry.failed);
      const screenQuad = entry.quad.map((point) => worldToScreen(point, rect, this.viewport)) as Quad;
      const width = Math.max(
        Math.hypot(screenQuad[1][0] - screenQuad[0][0], screenQuad[1][1] - screenQuad[0][1]),
        1,
      );
      const height = Math.max(
        Math.hypot(screenQuad[3][0] - screenQuad[0][0], screenQuad[3][1] - screenQuad[0][1]),
        1,
      );
      const angle = Math.atan2(screenQuad[1][1] - screenQuad[0][1], screenQuad[1][0] - screenQuad[0][0]);
      node.style.transformOrigin = "0 0";
      node.style.transform = `translate(${screenQuad[0][0]}px, ${screenQuad[0][1]}px) rotate(${angle}rad)`;
      node.style.width = `${width}px`;
      node.style.height = `${height}px`;
      // 图标不随图片旋转：外层旋转由内层反向抵消；尺寸随画面缩放，过小的方框只留底色。
      const minSide = Math.min(width, height);
      const icon = Math.max(12, Math.min(30, minSide * 0.26));
      node.style.setProperty("--pb-placeholder-icon", `${icon}px`);
      node.style.setProperty("--pb-placeholder-angle", `${-angle}rad`);
      node.classList.toggle("compact", minSide < 18);
    }
  }

  private clearPlaceholderOverlay() {
    if (!this.placeholderNodes) return;
    for (const node of this.placeholderNodes.values()) node.remove();
    this.placeholderNodes.clear();
  }

  private sortVisibleImages() {
    this.visibleImages.sort((left, right) => (
      left.layer - right.layer
      || right.order - left.order
    ));
  }

  private scheduleLoads(delay: number) {
    if (!this.active || this.destroyed || this.gpuUnavailable || !this.loaded) return;
    if (this.loadTimer !== null) window.clearTimeout(this.loadTimer);
    this.loadTimer = window.setTimeout(() => {
      this.loadTimer = null;
      this.loadRequested = true;
      if (!this.loading) void this.drainLoads();
    }, delay);
  }

  private async drainLoads() {
    this.loading = true;
    try {
      while (this.loadRequested && this.active && !this.destroyed) {
        this.loadRequested = false;
        const generation = this.generation;
        const candidates = this.textureLoadCandidates();
        // 只有非空批次会记录：空批次会立即结束循环，不构成自旋。
        if (candidates.length > 0) {
          diagnosticsLog(
            "info",
            `pin-board texture drain start: boardId=${this.boardId}, candidates=${candidates.length}, generation=${generation}`,
          );
        }
        const batchStarted = performance.now();
        await this.loadCandidates(candidates, generation);
        // 常速时不产生日志；一旦某批明显变慢即点名（显存压力 / 加载退化的前兆）。
        diagnosticsSlowStep(
          `texture drain batch boardId=${this.boardId} candidates=${candidates.length}`,
          batchStarted,
          2_000,
        );
        if (candidates.length > 0) {
          diagnosticsLog(
            "info",
            `pin-board texture drain done: boardId=${this.boardId}, candidates=${candidates.length},`
            + ` textures=${this.textures.size}, previews=${this.previewTextures.size},`
            + ` reservedBytes=${this.textureReservedBytes}, generation=${this.generation}`,
          );
        }
      }
    } finally {
      this.loading = false;
      if (this.loadRequested && this.active && !this.destroyed) void this.drainLoads();
    }
  }

  private textureLoadCandidates(): TextureLoadCandidate[] {
    const candidates = this.visibleImages.flatMap((image) => {
      // 笔迹没有纹理：进队列会去读 DDS 并失败，最终变成红色失败占位框。
      if (image.kind === "stroke") return [];
      // 编辑中的文字不重绘纹理；提交后统一失效并排队重建一次。
      if (this.textEdit?.itemId === image.itemId) return [];
      const center = quadCenter(image.points);
      const [width, height] = imageSize(image);
      const distanceSquared = (center[0] - this.viewport.centerX) ** 2
        + (center[1] - this.viewport.centerY) ** 2;
      const priority = distanceSquared / Math.max(width * height, 1);
      const list: TextureLoadCandidate[] = [];
      // 常驻缩略图兜底：没有缩略图时先生成缩略图候选，保证滑动回来后
      // 即使高清纹理尚未加载也能立即绘制，不闪烁。缩略图失败按维度记录，
      // 不会阻止更高分辨率的重试。
      if (!this.previewTextures.has(image.itemId)
        && !this.failedAtOrAbove(image.itemId, PIN_BOARD_MIN_PREVIEW_DIMENSION)) {
        list.push({
          image,
          maxDimension: PIN_BOARD_MIN_PREVIEW_DIMENSION,
          estimatedBytes: estimatedLoadBytes(image, PIN_BOARD_MIN_PREVIEW_DIMENSION),
          priority,
          isPreview: true,
        });
      }
      const desiredDimension = this.requestedDimension(image);
      const qualityCap = this.textureQualityCaps.get(image.itemId);
      const maxDimension = qualityCap
        ? Math.min(desiredDimension, qualityCap)
        : desiredDimension;
      const current = this.textures.get(image.itemId);
      if (!(current && current.maxDimension >= maxDimension * 0.9)
        && !this.failedAtOrAbove(image.itemId, maxDimension)) {
        list.push({
          image,
          maxDimension,
          estimatedBytes: estimatedLoadBytes(image, maxDimension),
          priority,
        });
      }
      return list;
    });
    return candidates.sort((left, right) => (
      Number(this.selectedIds.has(right.image.itemId))
        - Number(this.selectedIds.has(left.image.itemId))
      // 缩略图优先于高清，先给可见图片铺上兜底内容，再逐张清晰化。
      || Number(right.isPreview === true) - Number(left.isPreview === true)
      || left.priority - right.priority
      || right.maxDimension - left.maxDimension
    ));
  }

  /** 该图片在目标尺寸（及其之上）是否已记录过加载失败。 */
  private failedAtOrAbove(itemId: number, maxDimension: number): boolean {
    const failed = this.failedTextureDimensions.get(itemId);
    return failed !== undefined && failed >= maxDimension * 0.9;
  }

  private loadCandidates(candidates: TextureLoadCandidate[], generation: number): Promise<void> {
    return new Promise((resolve) => {
      let nextIndex = 0;
      let activeCount = 0;
      let activeBytes = 0;
      const launch = () => {
        if (this.destroyed || generation !== this.generation) {
          if (activeCount === 0) resolve();
          return;
        }
        while (activeCount < TEXTURE_LOAD_CONCURRENCY && nextIndex < candidates.length) {
          const candidate = candidates[nextIndex];
          if (activeCount > 0
            && activeBytes + candidate.estimatedBytes > PIN_BOARD_TEXTURE_LOAD_BUDGET_BYTES) break;
          nextIndex += 1;
          activeCount += 1;
          activeBytes += candidate.estimatedBytes;
          this.inFlightTextureLoads += 1;
          void this.loadCandidate(candidate, generation).finally(() => {
            this.inFlightTextureLoads = Math.max(0, this.inFlightTextureLoads - 1);
            if (this.destroyed) {
              diagnosticsLog(
                "info",
                `texture load settled after renderer destroy: boardId=${this.boardId}, gpuSeq=${this.gpu.sequence}, itemId=${candidate.image.itemId}, inFlightTextureLoads=${this.inFlightTextureLoads}`,
              );
            }
            activeCount -= 1;
            activeBytes -= candidate.estimatedBytes;
            if (activeCount === 0 && nextIndex >= candidates.length) resolve();
            else launch();
          });
        }
        if (activeCount === 0 && nextIndex >= candidates.length) resolve();
      };
      launch();
    });
  }

  private async loadCandidate(candidate: TextureLoadCandidate, generation: number) {
    const { image, maxDimension, isPreview } = candidate;
    const target = isPreview ? this.previewTextures : this.textures;
    let current: LoadedTexture | null = target.get(image.itemId) ?? null;
    let loaded: LoadedTexture | null = null;
    let lastError: unknown = null;
    try {
      if (current && current.maxDimension >= maxDimension * 0.9) return;

      // A high-zoom replacement can otherwise coexist with its old preview
      // until the post-load eviction pass, creating a large allocation spike.
      const dimensions = [maxDimension];
      const fallback = Math.max(
        PIN_BOARD_MIN_PREVIEW_DIMENSION,
        Math.floor(maxDimension / 2),
      );
      if (fallback < maxDimension) dimensions.push(fallback);

      for (const dimension of dimensions) {
        const residentBytes = estimatedResidentBytes(image, dimension);
        const reservationBytes = residentBytes + PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES;
        current = target.get(image.itemId) ?? null;
        const reserved = this.reserveTextureBytes(image.itemId, reservationBytes, isPreview);
        if (!reserved) {
          lastError = new Error("纹理显存预算不足");
          continue;
        }
        try {
          loaded = await loadTexture(this.gpu, image, dimension);
          break;
        } catch (error) {
          lastError = error;
        } finally {
          this.textureReservedBytes = Math.max(0, this.textureReservedBytes - reservationBytes);
        }
      }
      if (!loaded) throw lastError ?? new Error("纹理加载失败");
      if (this.destroyed || generation !== this.generation) {
        destroyTexture(loaded);
        return;
      }
      // 高清纹理还要确认图片仍在可见集；缩略图常驻，滑出视口后继续保留。
      if (!isPreview && !this.visibleImages.some((visible) => visible.itemId === image.itemId)) {
        destroyTexture(loaded);
        return;
      }
      const replacement = target.get(image.itemId);
      if (replacement && replacement.maxDimension >= maxDimension * 0.9) {
        destroyTexture(loaded);
        return;
      }
      if (replacement) destroyTexture(replacement);
      target.set(image.itemId, loaded);
      if (!isPreview) {
        if (loaded.maxDimension < maxDimension) {
          this.textureQualityCaps.set(image.itemId, loaded.maxDimension);
        } else {
          this.textureQualityCaps.delete(image.itemId);
        }
      }
      this.evictTextures(image.itemId, 0, isPreview);
      this.failedTextureDimensions.delete(image.itemId);
      this.draw();
    } catch (error) {
      if (this.destroyed || generation !== this.generation) return;
      const previous = this.failedTextureDimensions.get(image.itemId) ?? 0;
      this.failedTextureDimensions.set(image.itemId, Math.max(previous, maxDimension));
      // 缩略图失败静默（高清失败仍按原样上报），避免预算波动反复打扰用户。
      if (!isPreview) this.onError(`图片 ${image.itemId} 加载失败：${errorMessage(error)}`);
    }
  }

  private requestedDimension(image: PinBoardItem): number {
    // 笔迹不建纹理，任何纹理尺寸对它都没有意义。
    if (image.kind === "stroke") return 0;
    const displayDimension = imageDisplayMaxDimension(image, this.viewport, this.canvas.height);
    const deviceLimit = this.gpu.device.limits.maxTextureDimension2D as number;
    if (image.kind === "text") {
      return requestedTextTextureDimension(image, displayDimension, deviceLimit);
    }
    return requestedTextureDimension(image, displayDimension, deviceLimit);
  }

  private evictTextures(protectedImageId?: number, additionalBytes = 0, isPreview = false) {
    const target = isPreview ? this.previewTextures : this.textures;
    const budget = isPreview ? this.previewTextureBudgetBytes : this.textureBudgetBytes;
    let total = [...target.values()].reduce((sum, loaded) => sum + loaded.byteSize, 0);
    if (total + additionalBytes <= budget) return;
    const visible = new Set(this.visibleImages.map((image) => image.itemId));
    const protectedIds = this.exportProtectedIds;
    const candidates = [...target.entries()]
      .filter(([itemId]) => itemId !== protectedImageId && !protectedIds?.has(itemId))
      .sort((left, right) => {
        const visibilityOrder = Number(visible.has(left[0])) - Number(visible.has(right[0]));
        if (visibilityOrder !== 0) return visibilityOrder;
        // 高清淘汰优先选择已有缩略图兜底的图片，避免淘汰后画面空白闪烁；
        // 缩略图淘汰时本项恒相等（previewTextures 自身就是兜底），退化为 LRU。
        const leftCovered = Number(isPreview || this.previewTextures.has(left[0]));
        const rightCovered = Number(isPreview || this.previewTextures.has(right[0]));
        if (leftCovered !== rightCovered) return rightCovered - leftCovered;
        // 大图（纹理字节多）重新解码成本高、等待久，优先淘汰小图：小图重载
        // 快且有缩略图兜底，大图尽量保留，避免「划走再划回」反复等大图。
        if (left[1].byteSize !== right[1].byteSize) return left[1].byteSize - right[1].byteSize;
        return left[1].lastUsed - right[1].lastUsed;
      });
    for (const [itemId, loaded] of candidates) {
      destroyTexture(loaded);
      target.delete(itemId);
      total -= loaded.byteSize;
      if (total + additionalBytes <= budget) break;
    }
  }

  private reserveTextureBytes(
    protectedImageId: number,
    additionalBytes: number,
    isPreview = false,
  ): boolean {
    const target = isPreview ? this.previewTextures : this.textures;
    const budget = isPreview ? this.previewTextureBudgetBytes : this.textureBudgetBytes;
    this.evictTextures(protectedImageId, additionalBytes + this.textureReservedBytes, isPreview);
    let total = [...target.values()].reduce((sum, loaded) => sum + loaded.byteSize, 0);
    const current = target.get(protectedImageId);
    if (current && total + this.textureReservedBytes + additionalBytes > budget) {
      destroyTexture(current);
      target.delete(protectedImageId);
      total -= current.byteSize;
      this.draw();
    }
    if (total + this.textureReservedBytes + additionalBytes > budget) {
      return false;
    }
    this.textureReservedBytes += additionalBytes;
    return true;
  }
}
