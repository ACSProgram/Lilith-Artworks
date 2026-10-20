import { Layers3 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { PinBoardLayerVisibility } from "./layers";

/** 与 `.pin-board-layer-menu` 的 `width` 一致，用于把浮层收进视口。 */
const MENU_WIDTH = 208;
const MENU_GAP = 6;
const VIEWPORT_MARGIN = 8;

/**
 * 图层顺序按「从上到下」列：标注层（笔迹）永远在最上，随后是顶层、中层、底层。
 * `layer` 即条目的 layer 值，0–2 由图片与文字共用，3 是笔迹的固定层。
 */
const LAYER_ROWS: { layer: number; label: string; swatch: string; hint?: string }[] = [
  { layer: 3, label: "标注层", swatch: "annotation", hint: "笔迹" },
  { layer: 2, label: "顶层", swatch: "top" },
  { layer: 1, label: "中层", swatch: "middle" },
  { layer: 0, label: "底层", swatch: "bottom" },
];

interface MenuAnchor {
  left: number;
  top: number;
}

/**
 * 工具栏里的图层可见性开关：一个图标按钮 + 四个复选框。
 *
 * 四个复选框不占常驻按钮位——素材板主工具栏已有二十来个按钮，再排四个会被横向滚动
 * 藏掉。浮层挂到 `document.body` 并按触发按钮的视口位置定位，因为工具栏自身是
 * `overflow: hidden`，内联绝对定位会被裁掉（与取色器同一处理）。
 *
 * 显隐是纯视图状态：不改模型、不标记 dirty，因此这里只回调 `onToggle`，
 * 由渲染器决定重绘与选中清理。
 */
export function PinBoardLayersMenu({
  visibility,
  onToggle,
  disabled,
}: {
  visibility: PinBoardLayerVisibility;
  onToggle: (layer: number, visible: boolean) => void;
  disabled: boolean;
}) {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const open = anchor !== null;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  /** 有任一层被隐藏时按钮高亮，否则隐藏了图层却看不出来。 */
  const filtering = visibility.some((visible) => !visible);

  useEffect(() => {
    if (!open) return undefined;
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      setAnchor(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setAnchor(null);
    };
    const onViewportChange = () => setAnchor(null);
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onViewportChange);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onViewportChange);
    };
  }, [open]);

  const toggleMenu = () => {
    if (open) {
      setAnchor(null);
      return;
    }
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect) return;
    setAnchor({
      left: Math.min(
        Math.max(VIEWPORT_MARGIN, rect.right - MENU_WIDTH),
        Math.max(VIEWPORT_MARGIN, window.innerWidth - MENU_WIDTH - VIEWPORT_MARGIN),
      ),
      top: rect.bottom + MENU_GAP,
    });
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={`icon-button${filtering ? " active" : ""}`}
        disabled={disabled}
        onClick={toggleMenu}
        title={filtering ? "图层（有图层被隐藏）" : "图层可见性"}
        aria-label="图层可见性"
        aria-haspopup="dialog"
        aria-expanded={open}
      >
        <Layers3 size={17} />
      </button>
      {open && anchor && createPortal(
        <div
          ref={popoverRef}
          className="pin-board-layer-menu"
          role="dialog"
          aria-label="图层可见性"
          style={{ left: anchor.left, top: anchor.top }}
        >
          {LAYER_ROWS.map((row) => (
            <label className="pin-board-layer-row" key={row.layer}>
              <input
                type="checkbox"
                checked={visibility[row.layer] !== false}
                onChange={(event) => onToggle(row.layer, event.target.checked)}
              />
              <span className={`pin-board-layer-swatch ${row.swatch}`} />
              <span className="pin-board-layer-name">{row.label}</span>
              {row.hint && <small>{row.hint}</small>}
            </label>
          ))}
          <p className="pin-board-layer-note">隐藏的图层不会被点选、框选或全选选中。</p>
        </div>,
        document.body,
      )}
    </>
  );
}
