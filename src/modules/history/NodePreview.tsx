import { LoaderCircle, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
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
/** 与原悬停浮层一致：内容最长边不超过 280×216，并保持图片原始比例。 */
const MAX_CONTENT_WIDTH = 280;
const MAX_CONTENT_HEIGHT = 216;
const CONTENT_PADDING = 8;
const FRAME_BORDER = 1;
/** 「无可用预览」占位的最小内容框。 */
const EMPTY_CONTENT_WIDTH = 172;
const EMPTY_CONTENT_HEIGHT = 96;
/** 切换节点时的交叉淡入时长。 */
const FADE_MS = 100;

interface LoadedImage {
  src: string;
  width: number;
  height: number;
}

function fitContent(width: number, height: number) {
  if (!width || !height) return { width: MAX_CONTENT_WIDTH, height: MAX_CONTENT_HEIGHT };
  const scale = Math.min(1, MAX_CONTENT_WIDTH / width, MAX_CONTENT_HEIGHT / height);
  return {
    width: Math.max(1, Math.floor(width * scale)),
    height: Math.max(1, Math.floor(height * scale)),
  };
}

function RefreshButton({ overlay, refreshing, disabled, onRefresh, onEnter, onLeave }: {
  overlay?: boolean;
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
  onEnter?: () => void;
  onLeave?: () => void;
}) {
  return <button
    className={`icon-button history-preview-refresh${overlay ? " history-preview-refresh-overlay" : ""}`}
    type="button"
    title="重新获取缩略图"
    aria-label="重新获取缩略图"
    disabled={disabled || refreshing}
    onClick={onRefresh}
    onMouseEnter={onEnter}
    onMouseLeave={onLeave}
  ><RefreshCw className={refreshing ? "spin" : undefined} size={13} /></button>;
}

function LoadingPreview() {
  return <span className="history-preview-empty"><LoaderCircle className="spin" size={12} />读取预览…</span>;
}

function MissingPreview({ refreshing, disabled, onRefresh, onEnter, onLeave }: {
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
  onEnter?: () => void;
  onLeave?: () => void;
}) {
  return <span className="history-preview-empty">
    <span>无可用预览</span>
    <RefreshButton
      refreshing={refreshing}
      disabled={disabled}
      onRefresh={onRefresh}
      onEnter={onEnter}
      onLeave={onLeave}
    />
  </span>;
}

interface HoverPreviewProps {
  anchor: PreviewAnchor;
  title: string;
  preview: NodePreview;
  refreshing: boolean;
  disabled: boolean;
  onRefresh: () => void;
  /** 指针进入浮层里的刷新按钮：取消卡片离开时排定的关闭。 */
  onEnter: () => void;
  onLeave: () => void;
}

/**
 * 历史节点卡片上的悬停预览。
 *
 * 浮层挂在页面根部而不是卡片内部：卡片本身是 `<button>`，刷新按钮不能嵌套进去，
 * 而且画布容器带滚动，浮层必须用固定定位才能不被裁掉。浮层按图片原始比例自适应
 * 尺寸；新图先离屏解码，再和尺寸一起切换到浮层上，因此切换节点既不会露出半张图，
 * 也不会出现空白帧。浮层主体不接收鼠标事件，避免挡住相邻卡片把预览切断。
 */
export function HoverPreview({
  anchor, title, preview, refreshing, disabled, onRefresh, onEnter, onLeave,
}: HoverPreviewProps) {
  const [shown, setShown] = useState<LoadedImage | null>(null);
  const [incoming, setIncoming] = useState<LoadedImage | null>(null);

  useEffect(() => {
    const src = preview.dataUrl;
    if (!src) {
      setShown(null);
      setIncoming(null);
      return;
    }
    if (shown?.src === src) {
      // 淡入过程中又切回了已经显示的节点：撤销未完成的切换。
      if (incoming) setIncoming(null);
      return;
    }
    if (incoming?.src === src) return;
    let cancelled = false;
    const image = new Image();
    const commit = () => {
      if (cancelled) return;
      setIncoming({
        src,
        width: image.naturalWidth || MAX_CONTENT_WIDTH,
        height: image.naturalHeight || MAX_CONTENT_HEIGHT,
      });
    };
    if (typeof image.decode === "function") {
      image.src = src;
      void image.decode().then(commit, commit);
    } else {
      image.onload = commit;
      image.onerror = commit;
      image.src = src;
    }
    return () => {
      cancelled = true;
    };
  }, [preview.dataUrl, shown?.src, incoming?.src]);

  const target = incoming ?? shown;
  const content = target
    ? fitContent(target.width, target.height)
    : { width: EMPTY_CONTENT_WIDTH, height: EMPTY_CONTENT_HEIGHT };
  const frameWidth = content.width + CONTENT_PADDING * 2 + FRAME_BORDER * 2;
  const frameHeight = content.height + CONTENT_PADDING * 2 + FRAME_BORDER * 2;
  // 首次解码完成前不露出一个尺寸不对的空盒子；切换节点时旧图继续可见。
  const visible = preview.dataUrl ? Boolean(target) : true;

  const fitsRight = anchor.right + ANCHOR_GAP + frameWidth <= window.innerWidth - VIEWPORT_MARGIN;
  const left = fitsRight ? anchor.right + ANCHOR_GAP : anchor.left - ANCHOR_GAP - frameWidth;
  const top = Math.min(
    Math.max(anchor.top, VIEWPORT_MARGIN),
    Math.max(VIEWPORT_MARGIN, window.innerHeight - frameHeight - VIEWPORT_MARGIN),
  );
  const position: CSSProperties = {
    left: Math.max(VIEWPORT_MARGIN, left),
    top,
    width: frameWidth,
    height: frameHeight,
    visibility: visible ? "visible" : "hidden",
  };

  return <div className="history-hover-preview" style={position}>
    {preview.dataUrl
      ? <>
        {!shown && !incoming && <img
          className="history-hover-image"
          style={{ maxWidth: MAX_CONTENT_WIDTH, maxHeight: MAX_CONTENT_HEIGHT }}
          src={preview.dataUrl}
          alt={`${title} 的缩略图`}
        />}
        {shown && <img
          className="history-hover-image"
          style={{ width: content.width, height: content.height }}
          src={shown.src}
          alt={`${title} 的缩略图`}
        />}
        {incoming && <img
          key={incoming.src}
          className="history-hover-image history-hover-image-incoming"
          style={{ width: content.width, height: content.height, animationDuration: `${FADE_MS}ms` }}
          src={incoming.src}
          alt=""
          onAnimationEnd={() => {
            setShown(incoming);
            setIncoming(null);
          }}
        />}
      </>
      : <MissingPreview
        refreshing={refreshing}
        disabled={disabled}
        onRefresh={onRefresh}
        onEnter={onEnter}
        onLeave={onLeave}
      />}
    {preview.dataUrl && <RefreshButton
      overlay
      refreshing={refreshing}
      disabled={disabled}
      onRefresh={onRefresh}
      onEnter={onEnter}
      onLeave={onLeave}
    />}
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
