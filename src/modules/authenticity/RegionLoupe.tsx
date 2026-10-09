import { useLayoutEffect, useRef } from "react";
import type { PreviewImage } from "./types";
import type { TileRequest } from "./previewTile";
import { LOUPE_SIZE, LOUPE_ZOOM, loupePlacement, useLoupeFrame } from "./loupe";

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
 */
export function RegionLoupe({
  pointer, frame, stageWidth, stageHeight, sourceWidth, sourceHeight, sourceKey, base, request,
}: RegionLoupeProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const loupe = useLoupeFrame({ pointer, sourceWidth, sourceHeight, sourceKey, base, request });
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
    const ratio = Math.min(3, Math.max(1, window.devicePixelRatio || 1));
    const size = Math.round(LOUPE_SIZE * ratio);
    if (canvas.width !== size || canvas.height !== size) {
      canvas.width = size;
      canvas.height = size;
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, LOUPE_SIZE, LOUPE_SIZE);
    if (!loupe) return;
    // 指针在源图上的位置，换算到该底图/瓦片自身的像素坐标。
    const originX = (pointer.x * sourceWidth - loupe.originX) * loupe.scale;
    const originY = (pointer.y * sourceHeight - loupe.originY) * loupe.scale;
    // 镜片显示的源跨度（源图小于取样窗口时退化为整幅）。
    const spanX = Math.min(LOUPE_SIZE / LOUPE_ZOOM, sourceWidth);
    const spanY = Math.min(LOUPE_SIZE / LOUPE_ZOOM, sourceHeight);
    // 指针恒在镜片中心，因此绘制矩形始终居中；越过图片边界的部分由 canvas 裁掉，
    // 贴边时自然留下空边而不是把画面拉伸。
    const width = spanX * LOUPE_ZOOM;
    const height = spanY * LOUPE_ZOOM;
    context.imageSmoothingEnabled = !loupe.sharp;
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
  }, [loupe, pointer, sourceHeight, sourceWidth]);

  return <div
    className="region-loupe"
    style={{ left: placement.left, top: placement.top, width: LOUPE_SIZE, height: LOUPE_SIZE }}
  >
    <canvas className="region-loupe-canvas" ref={canvasRef} aria-hidden="true" />
    <div className="region-loupe-crosshair" aria-hidden="true" />
    <span className="region-loupe-pixel" aria-hidden="true" style={{ width: LOUPE_ZOOM, height: LOUPE_ZOOM }} />
    <span className="region-loupe-scale">{LOUPE_ZOOM}×</span>
  </div>;
}
