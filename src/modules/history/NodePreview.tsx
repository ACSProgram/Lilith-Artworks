import { LoaderCircle, RefreshCw } from "lucide-react";
import { useCallback, useLayoutEffect, useState } from "react";
import type { CSSProperties } from "react";
import type { NodePreview } from "./types";

/** 卡片在视口中的水平与顶边位置：浮层贴它的右侧，放不下时翻到左侧。 */
export interface PreviewAnchor {
  top: number;
  left: number;
  right: number;
}

const VIEWPORT_MARGIN = 8;
const ANCHOR_GAP = 10;

function RefreshButton({ overlay, refreshing, disabled, onRefresh }: {
  overlay?: boolean;
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
}) {
  return <button
    className={`icon-button history-preview-refresh${overlay ? " history-preview-refresh-overlay" : ""}`}
    type="button"
    title="重新获取缩略图"
    aria-label="重新获取缩略图"
    disabled={disabled || refreshing}
    onClick={onRefresh}
  ><RefreshCw className={refreshing ? "spin" : undefined} size={13} /></button>;
}

function LoadingPreview() {
  return <span className="history-preview-empty"><LoaderCircle className="spin" size={12} />读取预览…</span>;
}

function MissingPreview({ refreshing, disabled, onRefresh }: {
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
}) {
  return <span className="history-preview-empty">
    <span>无可用预览</span>
    <RefreshButton refreshing={refreshing} disabled={disabled} onRefresh={onRefresh} />
  </span>;
}

interface HoverPreviewProps {
  anchor: PreviewAnchor;
  title: string;
  preview: NodePreview;
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
  /** 指针进入浮层：取消卡片离开时排定的关闭。 */
  onEnter: () => void;
  onLeave: () => void;
}

/**
 * 历史节点卡片上的悬停预览。
 *
 * 浮层挂在页面根部而不是卡片内部：卡片本身是 `<button>`，刷新按钮不能嵌套进去，
 * 而且画布容器带滚动，浮层必须用固定定位才能不被裁掉。位置在渲染后测量，再决定
 * 贴在卡片的哪一侧，避免先闪到视口外或压住卡片。
 */
export function HoverPreview({
  anchor, title, preview, refreshing, disabled, onRefresh, onEnter, onLeave,
}: HoverPreviewProps) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [position, setPosition] = useState<CSSProperties>({ visibility: "hidden" });

  const measure = useCallback(() => {
    if (!element) return;
    const { width, height } = element.getBoundingClientRect();
    const fitsRight = anchor.right + ANCHOR_GAP + width <= window.innerWidth - VIEWPORT_MARGIN;
    const left = fitsRight ? anchor.right + ANCHOR_GAP : anchor.left - ANCHOR_GAP - width;
    const top = Math.min(
      Math.max(anchor.top, VIEWPORT_MARGIN),
      Math.max(VIEWPORT_MARGIN, window.innerHeight - height - VIEWPORT_MARGIN),
    );
    setPosition({ left: Math.max(VIEWPORT_MARGIN, left), top, visibility: "visible" });
  }, [anchor.left, anchor.right, anchor.top, element]);

  // 图片解码前量不到真实高度，解码完成后按实际尺寸重算一次位置。
  useLayoutEffect(() => {
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [measure]);

  return <div
    className="history-hover-preview"
    ref={setElement}
    style={position}
    onMouseEnter={onEnter}
    onMouseLeave={onLeave}
  >
    {preview.dataUrl
      ? <>
        <img src={preview.dataUrl} alt={`${title} 的缩略图`} onLoad={measure} />
        <RefreshButton overlay refreshing={refreshing} disabled={disabled} onRefresh={onRefresh} />
      </>
      : <MissingPreview refreshing={refreshing} disabled={disabled} onRefresh={onRefresh} />}
  </div>;
}

interface InspectorPreviewProps {
  title: string;
  /** `null` 表示还没有读到结果（首次读取中）。 */
  preview: NodePreview | null;
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
}

/**
 * 节点信息栏的缩略图区。
 *
 * 格式不支持缩略图时整块不显示——既没有图片，也不留「无可用预览」占位；读取中
 * 与读取失败仍保留占位和刷新入口，存量节点因此可以手动补一次抽取。
 */
export function InspectorPreview({ title, preview, refreshing, disabled, onRefresh }: InspectorPreviewProps) {
  if (preview && !preview.supported && !preview.dataUrl) return null;
  return <div className="history-preview">
    {preview?.dataUrl
      ? <>
        <img src={preview.dataUrl} alt={`${title} 的缩略图`} />
        <RefreshButton overlay refreshing={refreshing} disabled={disabled} onRefresh={onRefresh} />
      </>
      : preview
        ? refreshing
          ? <LoadingPreview />
          : <MissingPreview refreshing={false} disabled={disabled} onRefresh={onRefresh} />
        : <LoadingPreview />}
  </div>;
}
