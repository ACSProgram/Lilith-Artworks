import {
  AlignCenter,
  AlignLeft,
  AlignRight,
  Minus,
  Plus,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { PinBoardColorPicker } from "./PinBoardColorPicker";
import type { PinBoardRenderer, PinBoardTextEditState } from "./renderer";
import {
  PIN_BOARD_TEXT_FONT_FAMILY,
  PIN_BOARD_TEXT_LINE_HEIGHT_RATIO,
} from "./textLayout";
import {
  PIN_BOARD_TEXT_EDIT_MAX_FONT_SIZE,
  PIN_BOARD_TEXT_EDIT_MIN_FONT_SIZE,
  databaseFontSizeFor,
  effectivePinBoardFontSize,
  pinBoardTextScreenMetrics,
  type PinBoardNewTextRequest,
  type PinBoardTextDraft,
} from "./textEditing";
import type { PinBoardTextAlign } from "./types";

const ALIGN_OPTIONS: { align: PinBoardTextAlign; label: string; icon: typeof AlignLeft }[] = [
  { align: "left", label: "左对齐", icon: AlignLeft },
  { align: "center", label: "居中", icon: AlignCenter },
  { align: "right", label: "右对齐", icon: AlignRight },
];

function formatFontSize(value: number): string {
  return String(Math.round(value * 10) / 10);
}

function clampEffectiveFontSize(value: number): number {
  return Math.min(
    PIN_BOARD_TEXT_EDIT_MAX_FONT_SIZE,
    Math.max(PIN_BOARD_TEXT_EDIT_MIN_FONT_SIZE, Math.round(value)),
  );
}

/** 事件目标是否属于编辑器自身（含挂到 body 的取色浮层）。 */
function insideEditor(target: EventTarget | null, container: HTMLElement | null): boolean {
  if (!(target instanceof Node)) return false;
  if (container?.contains(target)) return true;
  return (target as HTMLElement).closest?.(".pin-board-color-popover") !== null;
}

/**
 * 这次按下是否落在画布本体上。
 *
 * 画布上的按下会接着交给渲染器：它要为同一次点击做「不再重新进入编辑」的抑制，
 * 因此只有画布需要这个标记。工具栏、侧栏上的按下若也留下标记，下一次点文字会白点
 * 一次才进编辑。
 */
function pressedOnCanvas(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".pin-board-canvas") !== null;
}

/**
 * 文字就地编辑框：与四边形同中心、同旋转的 textarea，外加屏幕水平的字体栏。
 *
 * 草稿只存在这里；每次变化同步推给渲染器。已有文字的提交/取消由渲染器统一处理，
 * 新建文字则在本地校验后经 `onCreateText` 走创建命令。编辑期间模型不变，因此自动
 * 保存不会写出半成品。
 */
export function PinBoardTextEditor({
  editing,
  renderer,
  onCreateText,
}: {
  editing: PinBoardTextEditState;
  renderer: PinBoardRenderer;
  /** 新建会话的提交入口：成功返回 null，失败返回要显示在编辑框上的原因。 */
  onCreateText: (request: PinBoardNewTextRequest) => Promise<string | null>;
}) {
  const [draft, setDraft] = useState<PinBoardTextDraft>(editing.draft);
  /** 草稿的最新值：异步创建与失焦提交都读它，避免闭包拿到上一帧的草稿。 */
  const draftRef = useRef(draft);
  const [effectiveText, setEffectiveText] = useState(() => (
    formatFontSize(effectivePinBoardFontSize(editing.draft.fontSize, editing.scale))
  ));
  const [error, setError] = useState<string | null>(null);
  const [composing, setComposing] = useState(false);
  const [creating, setCreating] = useState(false);
  const composingRef = useRef(false);
  const creatingRef = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const metrics = pinBoardTextScreenMetrics(
    draft,
    editing.scale,
    editing.pixelsPerWorldUnit,
  );
  /** 字体栏显示的是有效字号（数据库字号 × 条目缩放），画布缩放只影响观看。 */
  const effectiveFontSize = effectivePinBoardFontSize(draft.fontSize, editing.scale);

  const applyDraft = useCallback((next: PinBoardTextDraft) => {
    setDraft(next);
    draftRef.current = next;
    setError(null);
    renderer.updateTextEditDraft(next);
  }, [renderer]);

  const commit = useCallback((suppressClickEdit = false) => {
    if (composingRef.current || creatingRef.current) return false;
    if (editing.isNew) {
      // 新建会话没有模型可改：校验通过后交给创建命令，失败时保留编辑框与输入。
      if (draftRef.current.text.trim().length === 0) {
        renderer.cancelTextEdit();
        return true;
      }
      const resolved = renderer.resolveNewTextEdit();
      if (!resolved.ok) {
        setError(resolved.error);
        textareaRef.current?.focus({ preventScroll: true });
        return true;
      }
      creatingRef.current = true;
      setCreating(true);
      setError(null);
      void onCreateText(resolved.request).then((message) => {
        creatingRef.current = false;
        setCreating(false);
        if (message) {
          setError(message);
          textareaRef.current?.focus({ preventScroll: true });
        }
      });
      return true;
    }
    const message = renderer.commitTextEdit({ suppressClickEdit });
    if (message) {
      setError(message);
      textareaRef.current?.focus({ preventScroll: true });
      return false;
    }
    return true;
  }, [editing.isNew, onCreateText, renderer]);

  const cancel = useCallback(() => {
    renderer.cancelTextEdit();
  }, [renderer]);

  // 进入会话时聚焦；有落点信息就把光标放到点击处，否则放在末尾。
  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.focus({ preventScroll: true });
    const caret = editing.caret ?? textarea.value.length;
    const position = Math.min(Math.max(caret, 0), textarea.value.length);
    textarea.setSelectionRange(position, position);
  }, [editing.sessionId, editing.caret]);

  // 点击编辑框外部即提交。放在 document 捕获阶段，先于画布/工具栏自己的 pointerdown，
  // 这样“提交后照常执行这次点击”对下层控件成立。校验失败时吞掉这次按下并保留草稿。
  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      // 中键用于画布平移：保留编辑会话，编辑框随视口变化重新定位。
      if (event.button === 1) return;
      if (insideEditor(event.target, containerRef.current)) return;
      if (composingRef.current) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (!commit(pressedOnCanvas(event.target)) || editing.isNew) {
        // 新建草稿的提交是异步创建：这次按下不能再交给画布（否则会立刻把编辑会话
        // 当成失焦收掉，并在原地开始一次选中/拖动）。
        event.preventDefault();
        event.stopPropagation();
      }
    };
    document.addEventListener("pointerdown", handlePointerDown, true);
    return () => document.removeEventListener("pointerdown", handlePointerDown, true);
  }, [commit, editing.isNew]);

  const applyEffectiveFontSize = (value: number) => {
    const next = clampEffectiveFontSize(value);
    setEffectiveText(formatFontSize(next));
    if (next === effectiveFontSize) return;
    applyDraft({
      ...draft,
      fontSize: databaseFontSizeFor(next, editing.scale),
    });
  };

  const inputEffectiveFontSize = (raw: string) => {
    setEffectiveText(raw);
    const parsed = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(parsed) || parsed <= 0) return;
    applyDraft({
      ...draft,
      fontSize: databaseFontSizeFor(parsed, editing.scale),
    });
  };

  const halfWidth = metrics.width / 2;
  const halfHeight = metrics.height / 2;
  const cosine = Math.cos(editing.angle);
  const sine = Math.sin(editing.angle);
  const extentY = Math.abs(halfWidth * sine) + Math.abs(halfHeight * cosine);
  const top = editing.centerY - extentY;
  const bottom = editing.centerY + extentY;
  const placeBarAbove = top > 64;
  const barStyle: CSSProperties = {
    left: `${editing.centerX}px`,
    top: `${placeBarAbove ? top - 10 : bottom + 10}px`,
    transform: placeBarAbove ? "translate(-50%, -100%)" : "translate(-50%, 0)",
  };

  const textStyle: CSSProperties = {
    width: `${metrics.width}px`,
    height: `${metrics.height}px`,
    transform: `translate(${editing.centerX}px, ${editing.centerY}px) rotate(${editing.angle}rad) translate(-50%, -50%)`,
    fontSize: `${metrics.screenFontSize}px`,
    lineHeight: PIN_BOARD_TEXT_LINE_HEIGHT_RATIO,
    fontFamily: PIN_BOARD_TEXT_FONT_FAMILY,
    color: draft.color,
    textAlign: draft.align,
  };

  return (
    <div
      ref={containerRef}
      className="pin-board-text-editor"
      onKeyDown={(event) => {
        // 结束会话的按键在这里统一收口：textarea 与字体栏的按键都冒泡到这里。
        // 组合期间不响应，交给输入法自己处理。
        if (event.key === "Escape") {
          if (composingRef.current || creatingRef.current) return;
          event.preventDefault();
          event.stopPropagation();
          cancel();
          return;
        }
        if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
          if (composingRef.current || creatingRef.current) return;
          event.preventDefault();
          event.stopPropagation();
          commit();
        }
      }}
    >
      <textarea
        ref={textareaRef}
        className="pin-board-text-edit-input"
        style={textStyle}
        value={draft.text}
        wrap="off"
        spellCheck={false}
        readOnly={creating}
        aria-label={editing.isNew ? "添加文字" : "编辑文字"}
        onChange={(event) => applyDraft({ ...draft, text: event.target.value })}
        onCompositionStart={() => {
          composingRef.current = true;
          setComposing(true);
        }}
        onCompositionEnd={() => {
          composingRef.current = false;
          setComposing(false);
        }}
        onPointerDown={(event: ReactPointerEvent<HTMLTextAreaElement>) => {
          event.stopPropagation();
        }}
      />
      <div
        className="pin-board-text-editor-bar"
        style={barStyle}
        role="toolbar"
        aria-label="文字样式"
        onPointerDown={(event) => {
          // 非输入控件不抢焦点：点颜色/对齐不该先把编辑框当成失焦而提交。
          const target = event.target as HTMLElement;
          if (target.closest("input, textarea")) return;
          event.preventDefault();
        }}
      >
        <div className="pin-board-text-editor-size">
          <span className="pin-board-text-editor-label">大小</span>
          <button
            type="button"
            aria-label="减小字号"
            disabled={composing || creating || effectiveFontSize <= PIN_BOARD_TEXT_EDIT_MIN_FONT_SIZE}
            onClick={() => applyEffectiveFontSize(effectiveFontSize - 1)}
          >
            <Minus size={13} aria-hidden="true" />
          </button>
          <input
            type="number"
            min={PIN_BOARD_TEXT_EDIT_MIN_FONT_SIZE}
            max={PIN_BOARD_TEXT_EDIT_MAX_FONT_SIZE}
            step={1}
            value={effectiveText}
            aria-label="有效字号"
            onChange={(event) => inputEffectiveFontSize(event.target.value)}
            onBlur={() => applyEffectiveFontSize(Number(effectiveText) || effectiveFontSize)}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              applyEffectiveFontSize(Number(event.currentTarget.value) || effectiveFontSize);
              event.currentTarget.blur();
            }}
          />
          <button
            type="button"
            aria-label="增大字号"
            disabled={composing || creating || effectiveFontSize >= PIN_BOARD_TEXT_EDIT_MAX_FONT_SIZE}
            onClick={() => applyEffectiveFontSize(effectiveFontSize + 1)}
          >
            <Plus size={13} aria-hidden="true" />
          </button>
        </div>
        <span className="pin-board-divider" />
        <PinBoardColorPicker
          value={draft.color}
          onChange={(color) => applyDraft({ ...draft, color })}
        />
        <span className="pin-board-divider" />
        <div className="pin-board-text-editor-align" role="group" aria-label="对齐">
          {ALIGN_OPTIONS.map(({ align, label, icon: Icon }) => (
            <button
              key={align}
              type="button"
              className={draft.align === align ? "active" : undefined}
              aria-pressed={draft.align === align}
              aria-label={label}
              title={label}
              disabled={composing || creating}
              onClick={() => applyDraft({ ...draft, align })}
            >
              <Icon size={15} aria-hidden="true" />
            </button>
          ))}
        </div>
      </div>
      {error && (
        <div className="pin-board-text-editor-error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
}
