import { useLayoutEffect, useRef } from "react";
import type { PreviewImage } from "./types";
import type { TileRequest } from "./previewTile";
import { LOUPE_SIZE, LOUPE_ZOOM, loupePlacement, loupeSpan, useLoupeFrame } from "./loupe";

interface RegionLoupeProps {
  /** 指针在图片归一化坐标（0–1）。 */
  pointer: { x: number; y: number };
  /** 图片显示矩形（相对舞台，`object-fit: contain` 后的实际图片区域）。 */
  frame: { left: number; top: number; width: number; height: number };
  stageWidth: number;
  stageHeight: number;
  sourceWidth: number;
  sourceHeight: number;
  /** 源标识（分支或待识别图片路径），用于隔离瓦片缓存。 */
  sourceKey: string;
  /** 已加载的整幅降采样底图：瓦片到位前镜片用它立即出画。 */
  base: PreviewImage;
  request: (tile: TileRequest) => Promise<PreviewImage>;
}

/**
 * 框选放大镜：指针悬停在图片上即显示源分辨率局部，供精确落点。
 *
 * 镜片内容用 canvas 绘制——指针移动只是一次 `drawImage`，不产生任何请求；源分辨率
 * 瓦片在后台补齐并按格缓存，因此不存在「一移动就重新加载」的观感。覆盖层
 * `pointer-events: none`，不参与命中测试，也不改变归一化坐标语义。
 *
 * 取样跨度由「镜片相对主视图的固定倍率」决定，而不是固定的源像素数：主视图尺寸固定，
 * 因此镜片始终覆盖源图的固定比例，大画布不会被放大到无法与主视图对照。
 */
export function RegionLoupe({
  pointer, frame, stageWidth, stageHeight, sourceWidth, sourceHeight, sourceKey, base, request,
}: RegionLoupeProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // 主视图显示比例（CSS 像素 / 源像素）；画布尚未测得时按 1:1 处理。
  const displayScale = frame.width > 0 && sourceWidth > 0 ? frame.width / sourceWidth : 1;
  const span = loupeSpan(displayScale);
  // 镜片内一个源像素占用的 CSS 像素数，用于像素框尺寸与绘制缩放。
  const scale = LOUPE_SIZE / span;
  // 设备像素比同时决定画布后备缓冲与瓦片输出分辨率，两处取同一个收敛值。
  const pixelRatio = Math.min(3, Math.max(1, window.devicePixelRatio || 1));
  const loupe = useLoupeFrame({ pointer, sourceWidth, sourceHeight, span, pixelRatio, sourceKey, base, request });
  const placement = loupePlacement(
    frame.left + pointer.x * frame.width,
    frame.top + pointer.y * frame.height,
    stageWidth,
    stageHeight,
  );

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    // 按设备像素比放大画布后备缓冲，避免高分屏上被浏览器再插值一次。
    const size = Math.round(LOUPE_SIZE * pixelRatio);
    if (canvas.width !== size || canvas.height !== size) {
      canvas.width = size;
      canvas.height = size;
    }
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    context.clearRect(0, 0, LOUPE_SIZE, LOUPE_SIZE);
    if (!loupe) return;
    // 指针在源图上的位置，换算到该底图/瓦片自身的像素坐标。
    const originX = (pointer.x * sourceWidth - loupe.originX) * loupe.scale;
    const originY = (pointer.y * sourceHeight - loupe.originY) * loupe.scale;
    // 镜片显示的源跨度（源图小于取样跨度时退化为整幅）。
    const spanX = Math.min(span, sourceWidth);
    const spanY = Math.min(span, sourceHeight);
    // 指针恒在镜片中心，因此绘制矩形始终居中；越过图片边界的部分由 canvas 裁掉，
    // 贴边时自然留下空边而不是把画面拉伸。
    const width = spanX * scale;
    const height = spanY * scale;
    // 只有「源分辨率像素 + 放大绘制」才用最近邻，露出单个像素；缩小时改用平滑取样，
    // 否则大画布下按最近邻缩小会出现锯齿。
    context.imageSmoothingEnabled = !(loupe.sharp && scale >= loupe.scale);
    context.drawImage(
      loupe.image,
      originX - (spanX * loupe.scale) / 2,
      originY - (spanY * loupe.scale) / 2,
      spanX * loupe.scale,
      spanY * loupe.scale,
      (LOUPE_SIZE - width) / 2,
      (LOUPE_SIZE - height) / 2,
      width,
      height,
    );
  }, [loupe, pixelRatio, pointer, sourceHeight, sourceWidth, span, scale]);

  return <div
    className="region-loupe"
    style={{ left: placement.left, top: placement.top, width: LOUPE_SIZE, height: LOUPE_SIZE }}
  >
    <canvas className="region-loupe-canvas" ref={canvasRef} aria-hidden="true" />
    <div className="region-loupe-crosshair" aria-hidden="true" />
    {scale >= 1 && <span className="region-loupe-pixel" aria-hidden="true" style={{ width: scale, height: scale }} />}
    <span className="region-loupe-scale">{LOUPE_ZOOM}×</span>
  </div>;
}
