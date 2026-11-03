import {
  Image as ImageIcon, ImageDown, LoaderCircle, Maximize2, X, ZoomIn, ZoomOut,
} from "lucide-react";
import { formatBytes } from "../../shared/format";
import type {
  OutputPreviewTileSource, PreviewImage, PublicationPreview,
} from "./types";
import { tileOverlayStyle, type TileRequest } from "./previewTile";
import { usePreviewTiles } from "./usePreviewTiles";
import { usePreviewViewport } from "./usePreviewViewport";

interface PublicationPreviewDialogProps {
  preview: PublicationPreview;
  busy: boolean;
  cancelling: boolean;
  onRequestTile: (source: OutputPreviewTileSource, request: TileRequest) => Promise<PreviewImage>;
  onBack: () => void;
  onCancel: () => void;
  onPublish: () => void;
}

export function PublicationPreviewDialog({ preview, busy, cancelling, onRequestTile, onBack, onCancel, onPublish }: PublicationPreviewDialogProps) {
  const view = usePreviewViewport(preview);
  const { tile, desiredTileKey } = usePreviewTiles({
    preview,
    image: view.image,
    tileSource: view.tileSource,
    displayWidth: view.displayWidth,
    displayHeight: view.displayHeight,
    viewport: view.viewport,
    busy,
    onRequestTile,
  });
  const image = view.image;
  return <div className="dialog-backdrop publication-preview-backdrop" onMouseDown={() => { if (!busy) onBack(); }}>
    <section className="publication-preview-dialog" role="dialog" aria-modal="true" aria-labelledby="publication-preview-title" onMouseDown={(event) => event.stopPropagation()}>
      <header>
        <div><small>发布前检查</small><h2 id="publication-preview-title">导出预览</h2><span>{preview.sourceWidth} x {preview.sourceHeight} · {formatBytes(preview.outputBytes)} · {preview.cacheHit ? "复用缓存" : `渲染 ${preview.renderMs} ms / 编码 ${preview.encodeMs} ms`}</span></div>
        <div className="preview-zoom-controls">
          <button className="icon-button" type="button" title="缩小" onClick={view.zoomOut}><ZoomOut size={16} /></button>
          <button className="zoom-value" type="button" title="按原始像素显示（100% 为 1:1）" onClick={view.zoomToActualSize}>{Math.round(view.effectiveZoom * 100)}%</button>
          <button className="icon-button" type="button" title="放大" onClick={view.zoomIn}><ZoomIn size={16} /></button>
          <button className="icon-button" type="button" title="适应窗口" onClick={view.resetZoom}><Maximize2 size={16} /></button>
          <button className={`icon-button${view.showOriginal ? " active" : ""}`} type="button" title={view.showOriginal ? "显示压缩预览" : "显示原图"} disabled={view.imageSwitching} onClick={() => void view.toggleOriginal()}><ImageIcon size={16} /></button>
          <button className="icon-button" type="button" title="关闭预览" disabled={busy} onClick={onBack}><X size={17} /></button>
        </div>
      </header>
      <div className="publication-preview-stage">
        <div
          ref={view.canvasRef}
          className="publication-preview-canvas"
          onScroll={view.syncViewport}
          {...view.canvasPointerHandlers}
          onDragStart={(event) => event.preventDefault()}
        >
          <div
            className="publication-preview-content"
            style={{ width: `${view.displayWidth}px`, height: `${view.displayHeight}px` }}
          >
            <div
              className="publication-preview-image"
              style={{ width: `${view.displayWidth}px`, height: `${view.displayHeight}px` }}
            >
              <img ref={view.imageRef} src={image.dataUrl} alt={view.showOriginal ? "原始成品预览" : "导出预览"} draggable={false} onLoad={view.syncViewport} style={{ width: `${view.displayWidth}px`, height: `${view.displayHeight}px` }} />
              {tile && tile.key === desiredTileKey && <img
                className="publication-preview-tile"
                src={tile.image.dataUrl}
                alt=""
                draggable={false}
                style={tileOverlayStyle(tile.request, preview.sourceWidth, preview.sourceHeight)}
              />}
            </div>
          </div>
        </div>
        {view.navigable && view.navigationRect && <div
          className="publication-preview-navigator"
          title="拖动以定位预览区域"
          style={{ aspectRatio: `${image.width} / ${image.height}` }}
          {...view.navigatorPointerHandlers}
        >
          <img src={image.dataUrl} alt="" draggable={false} />
          <span style={{ left: `${view.navigationRect.left}%`, top: `${view.navigationRect.top}%`, width: `${view.navigationRect.width}%`, height: `${view.navigationRect.height}%` }} />
        </div>}
      </div>
      <footer><span>{busy ? (cancelling ? "正在安全取消；已开始的原子发布收尾不会中断。" : "正在写入 C2PA；时间戳服务最长等待 30 秒。") : view.showOriginal ? "当前显示原始成品，用于快速对比；放大后叠加源分辨率局部。" : "缩略图使用正式发布的背景合成、TrustMark 与 JPEG 编码参数；放大后叠加源分辨率局部。"}</span><div><button className="secondary-button" type="button" disabled={busy} onClick={onBack}>返回调整</button><button className={busy ? "secondary-button" : "primary-button"} type="button" disabled={cancelling} onClick={busy ? onCancel : onPublish}>{busy ? (cancelling ? <LoaderCircle className="spin" size={16} /> : <X size={16} />) : <ImageDown size={16} />}{busy ? (cancelling ? "正在取消" : "取消签名") : "签名并发布"}</button></div></footer>
    </section>
  </div>;
}
