import { AlertCircle, Trash2, X } from "lucide-react";
import { useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { NormalizedRegion, PreviewImage } from "./types";
import type { TileRequest } from "./previewTile";
import { WATERMARK_MIN_STABLE_EDGE, watermarkSizeWarnings } from "./publicationValidation";
import { RegionLoupe } from "./RegionLoupe";

type ImageTarget = "publish" | "decode";

interface RegionEditorProps {
  target: ImageTarget;
  preview: PreviewImage;
  regions: NormalizedRegion[];
  maxRegions: number;
  onChange: (regions: NormalizedRegion[]) => void;
  readOnly?: boolean;
  /** 源分辨率放大镜的取样来源；只读视图不传，因而不显示放大镜。 */
  loupe?: { sourceKey: string; request: (tile: TileRequest) => Promise<PreviewImage> };
}

export function RegionEditor({ target, preview, regions, maxRegions, onChange, readOnly = false, loupe }: RegionEditorProps) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [frame, setFrame] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  const [stageSize, setStageSize] = useState<{ width: number; height: number } | null>(null);
  const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null);
  const [draft, setDraft] = useState<NormalizedRegion | null>(null);
  const draftRef = useRef<NormalizedRegion | null>(null);
  const drag = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const update = () => {
      const scale = Math.min(stage.clientWidth / preview.width, stage.clientHeight / preview.height);
      const width = preview.width * scale;
      const height = preview.height * scale;
      setFrame({ left: (stage.clientWidth - width) / 2, top: (stage.clientHeight - height) / 2, width, height });
      setStageSize({ width: stage.clientWidth, height: stage.clientHeight });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(stage);
    return () => observer.disconnect();
  }, [preview.height, preview.width]);
  const point = (event: ReactPointerEvent) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const x = (event.clientX - bounds.left) / bounds.width;
    const y = (event.clientY - bounds.top) / bounds.height;
    return { x: Math.max(0, Math.min(1, x)), y: Math.max(0, Math.min(1, y)) };
  };
  const style = (region: NormalizedRegion) => ({ left: `${region.x * 100}%`, top: `${region.y * 100}%`, width: `${region.width * 100}%`, height: `${region.height * 100}%` });
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (readOnly || event.button !== 0 || (target !== "decode" && regions.length >= maxRegions)) return;
    const start = point(event);
    setPointer(start);
    if (target === "decode" && regions.length > 0) onChange([]);
    drag.current = { pointerId: event.pointerId, ...start };
    const next = { x: start.x, y: start.y, width: 0, height: 0 };
    draftRef.current = next;
    setDraft(next);
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const next = point(event);
    // 悬停即更新放大镜取样点；拖动时同一位置继续推进草稿矩形。
    setPointer(next);
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    const nextDraft = { x: Math.min(drag.current.x, next.x), y: Math.min(drag.current.y, next.y), width: Math.abs(next.x - drag.current.x), height: Math.abs(next.y - drag.current.y) };
    draftRef.current = nextDraft;
    setDraft(nextDraft);
  };
  const finish = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    drag.current = null;
    const finalDraft = draftRef.current;
    if (finalDraft && finalDraft.width * preview.width >= (target === "publish" ? 96 : 64) && finalDraft.height * preview.height >= (target === "publish" ? 96 : 64)) onChange(target === "decode" ? [finalDraft] : [...regions, finalDraft]);
    draftRef.current = null;
    setDraft(null);
  };
  // 发布页框选时的就地提示：按送进编码器的那张 rendition 的像素尺寸判定小选区。
  const sizeWarnings = target === "publish" && !readOnly
    ? watermarkSizeWarnings(regions, preview.width, preview.height)
    : [];
  return <div className={`region-stage${readOnly ? " read-only" : ""}`} ref={stageRef}>
    {frame && <div className="region-image-frame" style={frame}>
      <img src={preview.dataUrl} alt={target === "publish" ? "最终成品预览" : "待识别图片预览"} draggable={false} />
      <div
        className="region-layer"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={finish}
        onPointerCancel={finish}
        onPointerLeave={() => setPointer(null)}
      >
        {regions.map((region, index) => <div className="region-box" style={style(region)} key={`${region.x}-${region.y}-${index}`}><span>{target === "publish" ? `区域 ${index + 1}` : "识别区域"}</span>{target === "publish" && !readOnly && <button type="button" title="移除区域" onPointerDown={(event) => event.stopPropagation()} onClick={() => onChange(regions.filter((_, item) => item !== index))}><X size={12} /></button>}</div>)}
        {draft && <div className="region-box draft" style={style(draft)}><span>框选中</span></div>}
      </div>
    </div>}
    {!readOnly && loupe && pointer && frame && stageSize && <RegionLoupe
      pointer={pointer}
      frame={frame}
      stageWidth={stageSize.width}
      stageHeight={stageSize.height}
      sourceWidth={preview.width}
      sourceHeight={preview.height}
      sourceKey={loupe.sourceKey}
      base={preview}
      request={loupe.request}
    />}
    {target === "publish" && regions.length > 0 && !readOnly && <button className="clear-regions" type="button" onClick={() => onChange([])}><Trash2 size={13} />清除区域</button>}
    {sizeWarnings.length > 0 && <p className="region-size-warning" role="status"><AlertCircle size={14} aria-hidden="true" /><span>{sizeWarnings.map(({ regionIndex }) => `区域 ${regionIndex + 1}`).join("、")} 的任一边小于 {WATERMARK_MIN_STABLE_EDGE} px，数字水印可能无法回读；发布仍会继续。</span></p>}
  </div>;
}
