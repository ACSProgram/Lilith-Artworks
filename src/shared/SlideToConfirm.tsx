import { ChevronRight, LoaderCircle } from "lucide-react";
import type { CSSProperties, KeyboardEvent, PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { useEffect, useRef, useState } from "react";

/**
 * 拖到该比例即视为完成。留出余量，让「接近底部就松手」按取消处理，
 * 而不是把一次手抖判成确认。
 */
const COMPLETE_RATIO = 0.9;
/** 键盘每次按方向键推进的比例，保证只用键盘也能完成同一个动作。 */
const KEY_STEP = 0.25;
/**
 * 量不到轨迹宽度时（jsdom 或尚未布局）使用的回退行程。测试无法给出真实布局，
 * 因此按同一数值驱动滑动；导出以便 `slideToConfirmTesting.ts` 与它保持一致。
 */
export const SLIDE_FALLBACK_TRAVEL = 200;

/**
 * 滑动确认控件：把滑块沿轨迹拖到底（键盘按 `→` 逐步推进）才会触发回调。
 *
 * 用于最危险、不可逆的操作。与点击确认相比，它天然消除了「在同一个位置连点
 * 两次就误确认」的路径，也不需要用户额外读一遍文案；单击、轻微拖动都会自动
 * 弹回起点。样式见 `src/styles/index.css` 的 `.slide-confirm`。
 */
export function SlideToConfirm({
  label,
  doneLabel = "已确认",
  icon,
  disabled = false,
  onConfirm,
}: {
  /** 轨迹上的提示文字，同时作为控件的可访问名称。 */
  label: string;
  /** 触发之后的提示文字。 */
  doneLabel?: string;
  /** 滑块上的图标，默认右箭头。 */
  icon?: ReactNode;
  disabled?: boolean;
  onConfirm: () => void;
}) {
  const [progress, setProgress] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [completed, setCompleted] = useState(false);
  const progressRef = useRef(0);
  const completedRef = useRef(false);
  const confirmRef = useRef(onConfirm);
  const startX = useRef(0);
  const travel = useRef(SLIDE_FALLBACK_TRAVEL);
  const track = useRef<HTMLDivElement>(null);
  const thumb = useRef<HTMLDivElement>(null);

  // 进度同时写 ref：拖动中松手可能紧跟在同一帧内，读 state 会拿到旧值。
  const apply = (value: number) => {
    const next = Math.min(1, Math.max(0, value));
    progressRef.current = next;
    setProgress(next);
  };

  const complete = () => {
    if (completedRef.current) return;
    completedRef.current = true;
    setCompleted(true);
    apply(1);
    confirmRef.current();
  };

  const measure = () => {
    const width = (track.current?.clientWidth ?? 0) - (thumb.current?.offsetWidth ?? 0);
    return width > 0 ? width : SLIDE_FALLBACK_TRAVEL;
  };

  // 拖动监听只在 dragging 变化时重挂一次，闭包会留住挂载那一刻的回调；而
  // onConfirm 常是父组件的内联箭头函数，拖动期间父组件重渲染就会换新引用。
  // 用 ref 转一层，保证松手时执行的一定是最新回调。
  useEffect(() => {
    confirmRef.current = onConfirm;
  });

  useEffect(() => {
    if (!dragging) return;
    const move = (event: PointerEvent) => {
      apply((event.clientX - startX.current) / travel.current);
    };
    const release = () => {
      setDragging(false);
      if (progressRef.current >= COMPLETE_RATIO) complete();
      else apply(0);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
    };
  }, [dragging]);

  const beginDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (disabled || completedRef.current) return;
    startX.current = event.clientX;
    travel.current = measure();
    // 捕获指针，窗口外松手也能收到 pointerup，不会把拖动状态挂住。
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setDragging(true);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (disabled || completedRef.current) return;
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const next = progressRef.current + (event.key === "ArrowRight" ? KEY_STEP : -KEY_STEP);
    apply(next);
    if (next >= COMPLETE_RATIO) complete();
  };

  return <div
    className={`slide-confirm${dragging ? " dragging" : ""}${completed ? " completed" : ""}${disabled ? " disabled" : ""}`}
    ref={track}
    style={{ "--slide-progress": progress } as CSSProperties}
  >
    <span className="slide-confirm-fill" aria-hidden="true" />
    <span className="slide-confirm-label">{completed ? doneLabel : label}</span>
    <div
      className="slide-confirm-thumb"
      ref={thumb}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(progress * 100)}
      aria-disabled={disabled || undefined}
      title="向右拖动滑块到底即可确认；键盘可按 → 键逐步推进"
      onPointerDown={beginDrag}
      onKeyDown={onKeyDown}
    >
      {completed ? <LoaderCircle className="spin" size={14} /> : icon ?? <ChevronRight size={15} />}
    </div>
  </div>;
}
