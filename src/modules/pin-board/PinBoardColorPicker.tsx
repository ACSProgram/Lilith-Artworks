import { ChevronDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
} from "react";

interface Hsv {
  hue: number;
  saturation: number;
  value: number;
}

const COLOR_PRESETS = [
  "#000000",
  "#3f3f46",
  "#a1a1aa",
  "#ffffff",
  "#dc2626",
  "#ea580c",
  "#ca8a04",
  "#16a34a",
  "#2563eb",
  "#7c3aed",
];

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/** 十六进制输入框的草稿格式：去掉 `#`、统一大写，与 `value` 保持同一口径。 */
function hexInputDraft(color: string): string {
  return color.replace(/^#/, "").toUpperCase();
}

/** 接受 `#rgb` / `#rrggbb`（`#` 可省略），返回小写六位十六进制。 */
export function normaliseHexColor(raw: string): string | null {
  const digits = raw.trim().replace(/^#/, "");
  if (/^[0-9a-f]{3}$/i.test(digits)) {
    return `#${digits.split("").map((digit) => digit + digit).join("")}`.toLowerCase();
  }
  if (/^[0-9a-f]{6}$/i.test(digits)) return `#${digits.toLowerCase()}`;
  return null;
}

export function hexToHsv(hex: string): Hsv | null {
  const normalised = normaliseHexColor(hex);
  if (!normalised) return null;
  const red = Number.parseInt(normalised.slice(1, 3), 16) / 255;
  const green = Number.parseInt(normalised.slice(3, 5), 16) / 255;
  const blue = Number.parseInt(normalised.slice(5, 7), 16) / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;
  let hue = 0;
  if (delta > 0) {
    if (max === red) hue = ((green - blue) / delta) % 6;
    else if (max === green) hue = (blue - red) / delta + 2;
    else hue = (red - green) / delta + 4;
    hue *= 60;
    if (hue < 0) hue += 360;
  }
  return {
    hue,
    saturation: max === 0 ? 0 : delta / max,
    value: max,
  };
}

export function hsvToHex({ hue, saturation, value }: Hsv): string {
  const normalisedHue = ((hue % 360) + 360) % 360;
  const chroma = value * saturation;
  const secondary = chroma * (1 - Math.abs(((normalisedHue / 60) % 2) - 1));
  const offset = value - chroma;
  let red = 0;
  let green = 0;
  let blue = 0;
  if (normalisedHue < 60) [red, green, blue] = [chroma, secondary, 0];
  else if (normalisedHue < 120) [red, green, blue] = [secondary, chroma, 0];
  else if (normalisedHue < 180) [red, green, blue] = [0, chroma, secondary];
  else if (normalisedHue < 240) [red, green, blue] = [0, secondary, chroma];
  else if (normalisedHue < 300) [red, green, blue] = [secondary, 0, chroma];
  else [red, green, blue] = [chroma, 0, secondary];
  const channel = (value: number) => (
    Math.round((value + offset) * 255).toString(16).padStart(2, "0")
  );
  return `#${channel(red)}${channel(green)}${channel(blue)}`;
}

/**
 * 浮层的宽度、与触发按钮的间距和视口边距，单位都是 CSS 像素。
 * 宽度必须与 `.pin-board-color-popover` 的 `width` 一致，右侧对齐靠它反推左边。
 */
const POPOVER_WIDTH = 272;
const POPOVER_GAP = 8;
const VIEWPORT_MARGIN = 8;

interface PopoverAnchor {
  left: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
}

/**
 * 添加文字对话框与标注工具条共用的取色器：饱和/亮度面板 + 色相条 + 十六进制输入 +
 * 预设色。不调用系统取色器，主题与应用一致，且拖动过程中实时回传颜色。
 *
 * 浮层挂到 `document.body`：标注工具条为了在窄窗口里横向滚动开了 `overflow`，内联的
 * 绝对定位会被胶囊裁掉（对话框的 `overflow` 则是另一套规则）。挂到页面根部后按触发
 * 按钮的视口位置固定，两处入口走同一条路径，也不受祖先的 `transform` 影响。
 */
export function PinBoardColorPicker({
  value,
  onChange,
}: {
  value: string;
  onChange: (color: string) => void;
}) {
  const [anchor, setAnchor] = useState<PopoverAnchor | null>(null);
  const open = anchor !== null;
  const [hsv, setHsv] = useState<Hsv>(() => (
    hexToHsv(value) ?? { hue: 0, saturation: 0, value: 0 }
  ));
  const [hexDraft, setHexDraft] = useState(() => hexInputDraft(value));
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (hsvToHex(hsv) === normaliseHexColor(value)) return;
    const next = hexToHsv(value);
    if (next) setHsv(next);
    // 只在外部颜色变化时同步；拖动产生的回传值与当前 hsv 相同，不会打断拖动。
  }, [value]);

  useEffect(() => {
    setHexDraft(hexInputDraft(value));
  }, [value]);

  useEffect(() => {
    if (!open) return undefined;
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (rootRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      setAnchor(null);
    };
    const onViewportChange = () => setAnchor(null);
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && popoverRef.current?.contains(event.target)) return;
      setAnchor(null);
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open]);

  /** 打开时按触发按钮的视口位置选空间更大的一侧，高度按该侧可用空间收敛。 */
  const openPopover = () => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const spaceAbove = rect.top - VIEWPORT_MARGIN;
    const spaceBelow = window.innerHeight - rect.bottom - VIEWPORT_MARGIN;
    const left = Math.min(
      Math.max(VIEWPORT_MARGIN, rect.right - POPOVER_WIDTH),
      Math.max(VIEWPORT_MARGIN, window.innerWidth - POPOVER_WIDTH - VIEWPORT_MARGIN),
    );
    setAnchor(spaceAbove >= spaceBelow
      ? {
        left,
        bottom: window.innerHeight - rect.top + POPOVER_GAP,
        maxHeight: Math.max(spaceAbove - POPOVER_GAP, 0),
      }
      : {
        left,
        top: rect.bottom + POPOVER_GAP,
        maxHeight: Math.max(spaceBelow - POPOVER_GAP, 0),
      });
  };

  const apply = (patch: Partial<Hsv>) => {
    const next = { ...hsv, ...patch };
    setHsv(next);
    onChange(hsvToHex(next));
  };

  const updateArea = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    apply({
      saturation: clamp01((event.clientX - rect.left) / rect.width),
      value: 1 - clamp01((event.clientY - rect.top) / rect.height),
    });
  };

  const updateHue = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width <= 0) return;
    apply({ hue: clamp01((event.clientX - rect.left) / rect.width) * 360 });
  };

  const adjustArea = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 0.1 : 0.02;
    if (event.key === "ArrowLeft") apply({ saturation: clamp01(hsv.saturation - step) });
    else if (event.key === "ArrowRight") apply({ saturation: clamp01(hsv.saturation + step) });
    else if (event.key === "ArrowUp") apply({ value: clamp01(hsv.value + step) });
    else if (event.key === "ArrowDown") apply({ value: clamp01(hsv.value - step) });
    else return;
    event.preventDefault();
    event.stopPropagation();
  };

  const adjustHue = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 10 : 1;
    if (event.key === "ArrowLeft" || event.key === "ArrowDown") apply({ hue: (hsv.hue - step + 360) % 360 });
    else if (event.key === "ArrowRight" || event.key === "ArrowUp") apply({ hue: (hsv.hue + step) % 360 });
    else return;
    event.preventDefault();
    event.stopPropagation();
  };

  const commitHex = (raw: string) => {
    const normalised = normaliseHexColor(raw);
    if (!normalised) {
      setHexDraft(hexInputDraft(value));
      return;
    }
    onChange(normalised);
  };

  return (
    <div
      ref={rootRef}
      className="pin-board-color-field"
      onKeyDownCapture={(event) => {
        if (event.key === "Escape" && open) {
          event.preventDefault();
          event.stopPropagation();
          setAnchor(null);
        }
      }}
    >
      <button
        ref={triggerRef}
        type="button"
        className="pin-board-color-trigger"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => (open ? setAnchor(null) : openPopover())}
      >
        <span
          className="pin-board-color-swatch"
          style={{ backgroundColor: value }}
          aria-hidden="true"
        />
        <span className="pin-board-color-hex">{value.toUpperCase()}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </button>
      {anchor && createPortal(
        <div
          ref={popoverRef}
          className="pin-board-color-popover"
          role="dialog"
          aria-label="选择颜色"
          style={{
            left: anchor.left,
            top: anchor.top,
            bottom: anchor.bottom,
            maxHeight: anchor.maxHeight,
          }}
        >
          <div
            className="pin-board-color-area"
            role="slider"
            tabIndex={0}
            aria-label="饱和度与亮度"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(hsv.value * 100)}
            aria-valuetext={value.toUpperCase()}
            style={{ backgroundColor: `hsl(${hsv.hue} 100% 50%)` }}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              updateArea(event);
            }}
            onPointerMove={(event) => {
              if (event.currentTarget.hasPointerCapture(event.pointerId)) updateArea(event);
            }}
            onKeyDown={adjustArea}
          >
            <span className="pin-board-color-area-white" aria-hidden="true" />
            <span className="pin-board-color-area-black" aria-hidden="true" />
            <span
              className="pin-board-color-area-thumb"
              style={{
                left: `${hsv.saturation * 100}%`,
                top: `${(1 - hsv.value) * 100}%`,
                backgroundColor: value,
              }}
              aria-hidden="true"
            />
          </div>
          <div
            className="pin-board-color-hue"
            role="slider"
            tabIndex={0}
            aria-label="色相"
            aria-valuemin={0}
            aria-valuemax={360}
            aria-valuenow={Math.round(hsv.hue)}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              updateHue(event);
            }}
            onPointerMove={(event) => {
              if (event.currentTarget.hasPointerCapture(event.pointerId)) updateHue(event);
            }}
            onKeyDown={adjustHue}
          >
            <span
              className="pin-board-color-hue-thumb"
              style={{
                left: `${(hsv.hue / 360) * 100}%`,
                backgroundColor: `hsl(${hsv.hue} 100% 50%)`,
              }}
              aria-hidden="true"
            />
          </div>
          <div className="pin-board-color-hex-input">
            <span aria-hidden="true">#</span>
            <input
              value={hexDraft}
              maxLength={7}
              spellCheck={false}
              aria-label="十六进制颜色"
              onChange={(event) => {
                const digits = event.target.value
                  .replace(/[^0-9a-f]/gi, "")
                  .slice(0, 6)
                  .toUpperCase();
                setHexDraft(digits);
                if (digits.length === 6) onChange(`#${digits.toLowerCase()}`);
              }}
              onBlur={() => commitHex(hexDraft)}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                commitHex(hexDraft);
                event.currentTarget.blur();
              }}
            />
          </div>
          <div className="pin-board-color-presets" role="group" aria-label="预设颜色">
            {COLOR_PRESETS.map((preset) => (
              <button
                key={preset}
                type="button"
                className="pin-board-color-preset"
                style={{ backgroundColor: preset }}
                aria-label={preset.toUpperCase()}
                aria-pressed={preset === value.toLowerCase()}
                onClick={() => onChange(preset)}
              />
            ))}
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}
