import {
  AlertTriangle,
  FolderOpen,
  ImageDown,
  Minus,
  Plus,
  Type,
  X,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import {
  CANVAS_EXPORT_DEFAULT_MARGIN,
  CANVAS_EXPORT_MAX_MARGIN,
  CANVAS_EXPORT_MAX_SCALE,
  CANVAS_EXPORT_MIN_SCALE,
  clampCanvasExportMargin,
  type CanvasExportPlan,
} from "./exportPolicy";
import { pickPinBoardExportDirectory } from "./api";
import { PinBoardColorPicker } from "./PinBoardColorPicker";
import { PIN_BOARD_TEXT_MAX_CHARS } from "./textLayout";

/**
 * 素材板本地对话框外壳：backdrop 点击与 Escape 关闭、统一的头部与页脚。
 *
 * 结构与样式类与原对话框一致（`modal-backdrop pin-board-text-backdrop` +
 * `modal <className>` + `pin-board-text-header/body` + `modal-footer`），因此复用
 * 外壳不会改变任何观感；观感本身留给人工验收。
 */
function PinBoardDialog({
  className,
  titleId,
  icon,
  title,
  subtitle,
  onSubmit,
  onClose,
  children,
  footer,
}: {
  className: string;
  titleId: string;
  icon: ReactNode;
  title: string;
  subtitle: string;
  onSubmit: () => void;
  onClose: () => void;
  children: ReactNode;
  footer: ReactNode;
}) {
  return (
    <div
      className="modal-backdrop pin-board-text-backdrop"
      role="presentation"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <form
        className={`modal ${className}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <header className="pin-board-text-header">
          <span className="pin-board-text-header-icon" aria-hidden="true">
            {icon}
          </span>
          <div className="pin-board-text-heading">
            <h2 id={titleId}>{title}</h2>
            <p>{subtitle}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} title="关闭" aria-label="关闭">
            <X size={17} />
          </button>
        </header>
        <div className="pin-board-text-body">{children}</div>
        <footer className="modal-footer">{footer}</footer>
      </form>
    </div>
  );
}

export function PinBoardTextDialog({
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
    <PinBoardDialog
      className="pin-board-text-dialog"
      titleId="pin-board-text-title"
      icon={<Type size={18} />}
      title="添加文字"
      subtitle="调整内容与样式，文字会添加到当前画板"
      onSubmit={() => {
        if (filled) onSubmit(text, fontSize, color);
      }}
      onClose={onClose}
      footer={(
        <>
          <button className="secondary-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!filled}>添加</button>
        </>
      )}
    >
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
          <span className="pin-board-text-count">{text.length} / {PIN_BOARD_TEXT_MAX_CHARS}</span>
        </span>
        <textarea
          className="pin-board-text-input"
          autoFocus
          maxLength={PIN_BOARD_TEXT_MAX_CHARS}
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
    </PinBoardDialog>
  );
}

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
export function PinBoardExportDialog({
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
    <PinBoardDialog
      className="pin-board-export-dialog"
      titleId="pin-board-export-title"
      icon={<ImageDown size={18} />}
      title="导出画布"
      subtitle="把当前画板的全部可见内容合成一张透明 PNG"
      onSubmit={() => {
        if (!ready || !directory) return;
        onStart({
          directory,
          fileName: fileName.trim(),
          scale,
          margin,
        });
      }}
      onClose={onClose}
      footer={(
        <>
          <button className="secondary-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!ready}>
            {plan?.overLimit ? "仍要导出" : "开始导出"}
          </button>
        </>
      )}
    >
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
          <div>
            <span>输出尺寸</span>
            <strong>{plan.width} × {plan.height} 像素</strong>
          </div>
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
    </PinBoardDialog>
  );
}
