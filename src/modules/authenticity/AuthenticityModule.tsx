import {
  AlertCircle, BadgeCheck, Eye, FileImage, Fingerprint, FolderOpen, Image as ImageIcon, ImageDown, LoaderCircle,
  LockKeyhole, Maximize2, MoreVertical, MousePointer2, RotateCcw, ScanSearch, Search,
  ShieldCheck, Trash2, X, ZoomIn, ZoomOut,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { DragEvent as ReactDragEvent, PointerEvent as ReactPointerEvent } from "react";
import type { CleanupReport } from "../../shared/fileCleanup";
import { formatBytes } from "../../shared/format";
import { authenticityApi } from "./api";
import type {
  AuthenticityBranch, CertificationRecord, NormalizedRegion, PreviewImage, PreviewTileSource, PublicationPreview,
} from "./types";
import {
  clampPreviewZoom, navigatorRect, navigatorScrollTarget, previewZoomFromButton,
  previewZoomFromWheel, type PreviewViewport, zoomAnchorScrollTarget,
} from "./previewViewport";
import { tileCacheKey, tileOverlayStyle, tileRequestForView, type TileRequest } from "./previewTile";
import { RegionLoupe } from "./RegionLoupe";
import { useIdentificationController, usePublicationController } from "./useAuthenticityController";

interface AuthenticityModuleProps {
  mode: "publish" | "identify";
  artworkTitle: string;
  branches: AuthenticityBranch[];
  selectedBranchId: string | null;
  /** 应用层注入：分支列表正在加载（发布页此前会误报「尚无分支」）。 */
  branchesLoading?: boolean;
  /** 应用层注入：分支列表加载失败；与 `onRetryBranches` 一起提供内联重试。 */
  branchesError?: string | null;
  onRetryBranches?: () => void;
  selectedRecordId?: string | null;
  recordNavigationKey?: number;
  onSelectBranch: (branchId: string) => void;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
  onPublicationChanged?: () => Promise<void>;
}

type ImageTarget = "publish" | "decode";

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

export function AuthenticityModule(props: AuthenticityModuleProps) {
  return props.mode === "publish"
    ? <PublishView {...props} />
    : <IdentifyView onError={props.onError} onNavigateRecord={props.onNavigateRecord} />;
}

function PublishView({
  artworkTitle, branches, selectedBranchId, branchesLoading = false, branchesError = null, onRetryBranches, selectedRecordId, recordNavigationKey, onSelectBranch, onError, onNavigateRecord, onRetryFileCleanup, onPublicationChanged,
}: AuthenticityModuleProps) {
  const {
    publication, config, setConfig, preview, artifactPreviewBusy, artifactPreviewError,
    outputPreview, outputPreviewOpen,
    setOutputPreviewOpen, outputPreviewBusy, privateKey, setPrivateKey,
    publishing, cancelling, busy, result, publishMetrics, sizeEstimate, viewingRecord, setViewingRecord,
    viewingPreview, exporting, deletingRecord, deleteRecord, deleteConfirmOpen, setDeleteConfirmOpen, cleanupFailures,
    selectedBranch, enterPublication, retryArtifactPreview, chooseCertificate, generateOutputPreview,
    cancelAuthenticityOperation, publish, cancelPublication,
    retryCleanup, openRecord, exportRecord, previewLoupe,
  } = usePublicationController({
    artworkTitle,
    branches,
    selectedBranchId,
    selectedRecordId,
    recordNavigationKey,
    onError,
    onNavigateRecord,
    onRetryFileCleanup,
    onPublicationChanged,
  });

  if (viewingRecord) {
    return <RecordView record={viewingRecord} preview={viewingPreview} exporting={exporting} deleting={deletingRecord} onExport={exportRecord} onDelete={deleteRecord} onClose={() => setViewingRecord(null)} />;
  }

  return <div className="auth-workspace">
    <header className="auth-header">
      <div><span>发布与认证</span><h1>{artworkTitle}</h1></div>
      <div className="auth-header-actions">
        <select value={selectedBranchId ?? ""} disabled={busy || branchesLoading} onChange={(event) => onSelectBranch(event.target.value)}>
          {branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}
        </select>
        {publication?.artifact && <details className="auth-more-menu">
          <summary className="icon-button" title="更多发布操作"><MoreVertical size={17} /></summary>
          <div><button className="danger" type="button" disabled={busy} onClick={(event) => {
            (event.currentTarget.closest("details") as HTMLDetailsElement).open = false;
            setDeleteConfirmOpen(true);
          }}><Trash2 size={15} />取消发布并删除本地数据</button></div>
        </details>}
      </div>
    </header>
    {cleanupFailures.length > 0 && <div className="cleanup-failure-banner auth-cleanup-failure" role="status"><span><strong>{cleanupFailures.length} 个发布文件尚未清理</strong><small>{cleanupFailures[0].path}</small></span><button className="secondary-button" type="button" disabled={busy} onClick={() => void retryCleanup()}><RotateCcw size={15} />重试清理</button></div>}
    {!selectedBranch ? (branchesLoading
      ? <div className="auth-empty"><LoaderCircle className="spin" size={18} />读取分支历史…</div>
      : branchesError
        ? <div className="auth-empty auth-empty-error" role="alert"><AlertCircle size={18} /><span>{branchesError}</span>{onRetryBranches && <button className="secondary-button" type="button" onClick={onRetryBranches}><RotateCcw size={15} />重试</button>}</div>
        : <div className="auth-empty">此 Artwork 尚无分支。</div>) :
      !publication?.artifact ? <section className="publication-gate">
        <div className="gate-icon"><LockKeyhole size={24} /></div>
        <div><h2>让“{selectedBranch.title}”进入发布状态</h2><p>选择最终发布图片后，当前 HEAD 会强制设为检查点，成品复制进仓库并锁定该分支。</p></div>
        <button className="primary-button" type="button" disabled={busy || !selectedBranch.headHistoryId} onClick={() => void enterPublication()}>
          {busy ? <LoaderCircle className="spin" size={16} /> : <FileImage size={16} />}选择最终成品
        </button>
        {!selectedBranch.headHistoryId && <small>至少需要一个历史节点才能发布。</small>}
      </section> : config && preview ? <div className="publish-layout">
        <section className="auth-preview-panel">
          <header><div><strong>{fileName(publication.artifact.sourcePath)}</strong><span>{preview.width} x {preview.height} · {formatBytes(publication.artifact.byteSize)}</span></div><i><BadgeCheck size={14} />发布节点已锁定</i></header>
          <RegionEditor
            target="publish"
            preview={preview}
            regions={config.additionalRegions}
            maxRegions={8}
            onChange={(regions) => setConfig({ ...config, additionalRegions: regions, trustmarkEnabled: regions.length > 0 })}
            loupe={{ sourceKey: publication?.branchId ?? "", request: previewLoupe }}
          />
          <div className="artifact-proof"><span>发布检查点</span><code>{publication.artifact.historyId}</code><span>成品 SHA-256</span><code>{publication.artifact.sourceSha256}</code></div>
        </section>
        <section className="publish-controls">
          <div className="auth-form-section">
            <header><strong>C2PA 内容凭证</strong><span>发布时强制签名</span></header>
            <label>作品标题<input value={config.title} maxLength={160} onChange={(event) => setConfig({ ...config, title: event.target.value })} /></label>
            <label>创作者<input value={config.creator} maxLength={160} onChange={(event) => setConfig({ ...config, creator: event.target.value })} /></label>
            <label>权利声明<textarea value={config.rightsStatement} rows={2} onChange={(event) => setConfig({ ...config, rightsStatement: event.target.value })} /></label>
            <label>认证说明<textarea value={config.authenticationContent} rows={3} onChange={(event) => setConfig({ ...config, authenticationContent: event.target.value })} /></label>
          </div>
          <div className="auth-form-section two-fields">
            <label>签名算法<select value={config.signingAlgorithm} onChange={(event) => setConfig({ ...config, signingAlgorithm: event.target.value })}><option value="es256">ES256</option><option value="es384">ES384</option><option value="ed25519">Ed25519</option></select></label>
            <label className="wide-field">证书链<div className="auth-path-control"><input readOnly value={config.certificatePath} placeholder="选择 PEM 证书链" /><button className="icon-button" type="button" title="选择证书" onClick={() => void chooseCertificate()}><FolderOpen size={16} /></button></div></label>
            <label className="wide-field">PEM 私钥<input className="secret-field" type="password" value={privateKey} autoComplete="new-password" placeholder="输入后仅在本次发布使用" onChange={(event) => setPrivateKey(event.target.value)} /></label>
            <label className="wide-field">时间戳服务<input value={config.timestampUrl ?? ""} placeholder="可选 RFC 3161 URL" onChange={(event) => setConfig({ ...config, timestampUrl: event.target.value || null })} /></label>
          </div>
          <div className="auth-form-section output-settings">
            <header><strong>JPG 输出</strong><span>固定导出格式</span></header>
            <label className="range-field">JPEG 质量 <output>{config.jpegQuality}</output><input type="range" min={1} max={100} value={config.jpegQuality} onChange={(event) => setConfig({ ...config, jpegQuality: Number(event.target.value) })} /></label>
            <div className="size-preview"><span>JPEG 预估大小</span><strong>{sizeEstimate == null ? "计算中" : formatBytes(sizeEstimate)}</strong><small>原图 {formatBytes(preview.sourceBytes)}</small></div>
            <label>透明背景<input type="color" value={config.backgroundColor} onChange={(event) => setConfig({ ...config, backgroundColor: event.target.value })} /></label>
          </div>
          <div className="auth-form-section trustmark-section">
            <header><strong>TrustMark {publication.modelVariant}</strong><label className="switch-field"><span className="switch-copy"><strong>{config.trustmarkEnabled ? "已启用" : "不嵌入"}</strong></span><input className="switch-input" type="checkbox" checked={config.trustmarkEnabled} disabled aria-label="TrustMark 状态由框选区域决定" /></label></header>
            <div className="trustmark-region-hint"><MousePointer2 size={16} /><span><strong>在左侧图片上拖动框选区域</strong><small>完成第一个框选后自动启用 TrustMark 水印；清空区域后自动关闭。</small></span></div>
            {!publication.modelsReady && <p className="auth-warning">TrustMark 模型不可用，仍可发布 C2PA 凭证。</p>}
            <details className="model-info"><summary>模型信息</summary><dl><div><dt>变体</dt><dd>{publication.modelVariant}</dd></div><div><dt>Encoder SHA-256</dt><dd><code>{publication.encoderSha256 ?? "不可用"}</code></dd></div><div><dt>Decoder SHA-256</dt><dd><code>{publication.decoderSha256 ?? "不可用"}</code></dd></div></dl></details>
            {config.trustmarkEnabled && <>
              <label className="range-field">TrustMark 强度 <output>{config.watermarkStrength.toFixed(2)}</output><input type="range" min={0.5} max={1.5} step={0.05} value={config.watermarkStrength} onChange={(event) => setConfig({ ...config, watermarkStrength: Number(event.target.value) })} />{config.watermarkStrength > 1 && <small className="auth-warning">超过 1.00 可能造成质量损失</small>}</label>
              <p>预览首次自动生成随机 ID，调整后保持一致并在发布时复用。仅在 {config.additionalRegions.length} 个框选区域嵌入水印。</p>
            </>}
          </div>
          {result && <div className="publish-success"><BadgeCheck size={18} /><div><strong>认证发布完成</strong><span>{result.outputPath}</span><code>{result.watermarkId}</code>{publishMetrics && <small>{publishMetrics.renditionCacheHit ? "已复用质量预览编码" : `重新渲染 ${publishMetrics.renderMs} ms · 编码 ${publishMetrics.encodeMs} ms`} · C2PA/时间戳 {publishMetrics.signingMs} ms</small>}</div></div>}
          {outputPreviewBusy ? <div className="output-preview-progress" role="status" aria-live="polite">
            <div><LoaderCircle className="spin" aria-hidden="true" size={18} /><span><strong>{cancelling ? "正在取消质量预览" : "正在生成质量预览"}</strong><small>{cancelling ? "等待当前处理阶段安全结束" : "正在渲染并编码正式 JPG 预览"}</small></span></div>
            <button className="secondary-button" type="button" disabled={cancelling} onClick={() => void cancelAuthenticityOperation()}><X aria-hidden="true" size={16} />取消</button>
          </div> : <button className="primary-button publish-command" type="button" disabled={busy} onClick={() => void generateOutputPreview()}><Eye aria-hidden="true" size={17} />生成质量预览</button>}
        </section>
        <RecordList records={publication.records} onNavigate={openRecord} selectedId={selectedRecordId} />
        {outputPreviewOpen && outputPreview && <PublicationPreviewDialog preview={outputPreview} busy={publishing} cancelling={cancelling} onBack={() => setOutputPreviewOpen(false)} onCancel={() => void cancelAuthenticityOperation()} onPublish={() => void publish()} />}
      </div> : config && artifactPreviewError ? <>
        <section className="publication-gate">
          <div className="gate-icon"><FileImage size={24} /></div>
          <div><h2>分支已进入发布状态，成品预览暂不可用</h2><p>发布元数据和取消入口仍然可用。修复或恢复成品文件后可单独重试预览。</p></div>
          <button className="secondary-button" type="button" disabled={artifactPreviewBusy} onClick={() => void retryArtifactPreview()}>
            {artifactPreviewBusy ? <LoaderCircle className="spin" size={16} /> : <RotateCcw size={16} />}重试预览
          </button>
          <small>{artifactPreviewError}</small>
        </section>
        <RecordList records={publication.records} onNavigate={openRecord} selectedId={selectedRecordId} />
      </> : <div className="auth-empty"><LoaderCircle className="spin" size={18} />{publication?.artifact ? "读取成品预览" : "读取发布状态"}</div>}
    {selectedBranch && deleteConfirmOpen && <PublicationDeleteDialog branchTitle={selectedBranch.title} busy={busy} onClose={() => setDeleteConfirmOpen(false)} onConfirm={() => void cancelPublication()} />}
  </div>;
}

function IdentifyView({ onError, onNavigateRecord }: Pick<AuthenticityModuleProps, "onError" | "onNavigateRecord">) {
  const {
    path, preview, region, setRegion, result, query, setQuery, records, busy, searching,
    choose, importDropped, decode, searchRecords, previewLoupe,
  } = useIdentificationController({ onError });
  const [dragActive, setDragActive] = useState(false);

  const onDragOver = (event: ReactDragEvent<HTMLElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    setDragActive(true);
  };
  const onDragLeave = (event: ReactDragEvent<HTMLElement>) => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    setDragActive(false);
  };
  const onDrop = (event: ReactDragEvent<HTMLElement>) => {
    event.preventDefault();
    setDragActive(false);
    const file = Array.from(event.dataTransfer.files)
      .find((item) => /\.(png|jpe?g|webp|tiff?)$/i.test(item.name));
    if (!file) {
      onError("请拖入 PNG、JPEG、WebP 或 TIFF 图片");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const value = typeof reader.result === "string" ? reader.result : "";
      const base64 = value.slice(value.indexOf(",") + 1);
      if (!base64) {
        onError("无法读取拖入的图片");
        return;
      }
      void importDropped(file.name, base64);
    };
    reader.onerror = () => onError("无法读取拖入的图片");
    reader.readAsDataURL(file);
  };

  return <div className="auth-workspace identify-workspace">
    <header className="auth-header"><div><span>识别与溯源</span><h1>验证发布图片</h1></div></header>
    <div className="identify-layout">
      <section
        className={`auth-preview-panel identify-preview${dragActive ? " drop-active" : ""}`}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
      >
        {!preview ? <button className="image-empty" type="button" onClick={() => void choose()}><ScanSearch size={28} /><strong>选择待识别图片</strong><span>点击选择，或把图片拖到这里。C2PA 会始终读取；TrustMark 可识别整图或框选区域。</span></button> : <>
          <header><div><strong>{fileName(path)}</strong><span>{preview.width} x {preview.height}</span></div><button className="text-button" type="button" onClick={() => void choose()}>更换图片</button></header>
          <RegionEditor target="decode" preview={preview} regions={region ? [region] : []} maxRegions={1} onChange={(regions) => setRegion(regions[0] ?? null)} loupe={{ sourceKey: path, request: previewLoupe }} />
          <div className="decode-scope"><Fingerprint size={17} /><span>{region ? "识别框选区域" : "识别整张图片"}</span>{region && <button className="icon-button" type="button" title="取消区域并识别整图" onClick={() => setRegion(null)}><X size={15} /></button>}</div>
          <button className="primary-button" type="button" disabled={busy} onClick={() => void decode()}>{busy ? <LoaderCircle className="spin" size={16} /> : <ScanSearch size={16} />}开始识别</button>
        </>}
        {dragActive && <div className="image-drop-hint"><ScanSearch size={22} /><span>松开以导入图片</span></div>}
      </section>
      <section className="decode-results">
        {!result ? <div className="decode-placeholder"><Fingerprint size={24} /><span>识别结果将在这里显示</span></div> : <>
          <header className={`decode-status ${result.c2paPresent ? "detected" : "not-detected"}`}><ShieldCheck size={20} /><div><strong>{result.c2paPresent ? "已读取 C2PA" : "未发现 C2PA"}</strong><span>{result.c2paValidationState ?? "无验证状态"}</span></div></header>
          <div className="evidence-status-grid">
            <div className={result.c2paPresent ? "detected" : "not-detected"}><ShieldCheck size={14} /><span><strong>C2PA</strong><small>{result.c2paPresent ? "已检出" : "未检出"}</small></span></div>
            <div className={result.watermarkPresent ? "detected" : "not-detected"}><Fingerprint size={14} /><span><strong>TrustMark</strong><small>{result.watermarkPresent ? "已检出" : "未检出"}</small></span></div>
          </div>
          <dl><div><dt>C2PA 记录 ID</dt><dd><code>{result.c2paRecordId ?? "未声明"}</code></dd></div><div><dt>C2PA TrustMark ID</dt><dd><code>{result.c2paWatermarkId ?? "未声明"}</code></dd></div><div><dt>识别出的 TrustMark ID</dt><dd><code>{result.watermarkId ?? "未识别"}</code></dd></div><div><dt>双通道</dt><dd>{result.identifiersMatch == null ? "只有单通道证据" : result.identifiersMatch ? "ID 一致" : "ID 冲突，需人工调查"}</dd></div></dl>
          <dl className="claim-grid"><div><dt>作品</dt><dd>{result.title ?? "未声明"}</dd></div><div><dt>创作者</dt><dd>{result.creator ?? "未声明"}</dd></div><div><dt>权利声明</dt><dd>{result.rightsStatement ?? "未声明"}</dd></div><div><dt>认证内容</dt><dd>{result.authenticationContent ?? "未声明"}</dd></div></dl>
          {result.c2paValidationStatus.length > 0 && <ul className="validation-list">{result.c2paValidationStatus.map((item) => <li key={`${item.code}-${item.explanation}`}><strong>{item.code}</strong><span>{item.explanation}</span></li>)}</ul>}
          {result.manifestJson && <details className="manifest-details" open><summary>原始 C2PA 报告</summary><pre>{result.manifestJson}</pre></details>}
          <RecordList records={result.matches.map((match) => match.record)} evidence={Object.fromEntries(result.matches.map((match) => [match.record.id, match.evidenceSources]))} onNavigate={onNavigateRecord} compact />
        </>}
      </section>
      <section className="record-search">
        <header><Search size={17} /><div><strong>搜索导出记录</strong><span>按 ID、标题、创作者或首次输出路径</span></div></header>
        <div className="record-search-control"><input value={query} maxLength={160} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void searchRecords(); }} /><button className="secondary-button" disabled={searching} onClick={() => void searchRecords()}>{searching ? <LoaderCircle className="spin" size={15} /> : <Search size={15} />}搜索</button></div>
        <RecordList records={records} onNavigate={onNavigateRecord} compact />
      </section>
    </div>
  </div>;
}

function RegionEditor({ target, preview, regions, maxRegions, onChange, readOnly = false, loupe }: {
  target: ImageTarget;
  preview: PreviewImage;
  regions: NormalizedRegion[];
  maxRegions: number;
  onChange: (regions: NormalizedRegion[]) => void;
  readOnly?: boolean;
  /** 源分辨率放大镜的取样来源；只读视图不传，因而不显示放大镜。 */
  loupe?: { sourceKey: string; request: (tile: TileRequest) => Promise<PreviewImage> };
}) {
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
    if (!start) return;
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
    if (!next) return;
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
  </div>;
}

function RecordView({ record, preview, exporting, deleting, onExport, onDelete, onClose }: {
  record: CertificationRecord;
  preview: PreviewImage | null;
  exporting: boolean;
  deleting: boolean;
  onExport: (record: CertificationRecord) => Promise<void>;
  onDelete: (record: CertificationRecord) => Promise<void>;
  onClose: () => void;
}) {
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  return <div className="auth-workspace record-view-mode">
    <header className="auth-header"><div><span>发布记录 · 只读</span><h1>{record.title}</h1></div><div className="record-view-actions"><button className="secondary-button" type="button" disabled={exporting} onClick={() => void onExport(record)}>{exporting ? <LoaderCircle className="spin" size={15} /> : <ImageDown size={15} />}再次导出</button><button className="secondary-button" type="button" onClick={onClose}><X size={15} />退出查看</button><details className="auth-more-menu"><summary className="icon-button" title="更多记录操作"><MoreVertical size={17} /></summary><div><button className="danger" type="button" disabled={deleting} onClick={(event) => {
      (event.currentTarget.closest("details") as HTMLDetailsElement).open = false;
      setDeleteConfirmOpen(true);
    }}><Trash2 size={15} />删除本记录</button></div></details></div></header>
    <div className="publish-layout record-review-layout">
      <section className="auth-preview-panel">
        <header><div><strong>{fileName(record.outputPath)}</strong><span>{preview ? `${preview.width} x ${preview.height} · ` : ""}{formatBytes(record.outputBytes)}</span></div><i><LockKeyhole size={14} />记录已锁定</i></header>
        {preview ? <RegionEditor target="publish" preview={preview} regions={record.additionalRegions} maxRegions={0} onChange={() => undefined} readOnly /> : <div className="record-preview-loading"><LoaderCircle className="spin" size={18} />读取认证图片</div>}
        <div className="artifact-proof"><span>发布节点</span><code>{record.historyId}</code><span>成品 SHA-256</span><code>{record.outputSha256}</code></div>
      </section>
      <section className="publish-controls read-only-controls">
        <div className="auth-form-section"><header><strong>C2PA 内容凭证</strong><span>只读快照</span></header><ReadOnlyField label="作品标题" value={record.title} /><ReadOnlyField label="创作者" value={record.creator || "未声明"} /><ReadOnlyField label="权利声明" value={record.rightsStatement || "未声明"} multiline /><ReadOnlyField label="认证说明" value={record.authenticationContent || "未声明"} multiline /></div>
        <div className="auth-form-section"><header><strong>发布信息</strong><span>{new Date(record.createdMs).toLocaleString()}</span></header><ReadOnlyField label="Artwork / 分支" value={`${record.artworkTitle} / ${record.branchTitle}`} /><ReadOnlyField label="首次导出位置" value={record.outputPath} multiline /><ReadOnlyField label="验证状态" value={record.validationState || "未记录"} /><ReadOnlyField label="Manifest 标签" value={record.c2paManifestLabel || "未记录"} multiline /></div>
        <div className="auth-form-section trustmark-section"><header><strong>TrustMark</strong><span>{record.trustmarkEnabled ? "已嵌入" : "未嵌入"}</span></header><ReadOnlyField label="TrustMark ID" value={record.watermarkId || "无"} /><p>{record.additionalRegions.length > 0 ? `水印写入 ${record.additionalRegions.length} 个框选区域。` : "此记录未使用框选水印区域。"}</p></div>
      </section>
      {record.c2paManifestJson && <section className="record-manifest"><details className="manifest-details" open><summary>C2PA 报告</summary><pre>{record.c2paManifestJson}</pre></details></section>}
    </div>
    {deleteConfirmOpen && <RecordDeleteDialog record={record} busy={deleting} onClose={() => setDeleteConfirmOpen(false)} onConfirm={() => void onDelete(record)} />}
  </div>;
}

function RecordDeleteDialog({ record, busy, onClose, onConfirm }: { record: CertificationRecord; busy: boolean; onClose: () => void; onConfirm: () => void }) {
  return <div className="dialog-backdrop" onMouseDown={onClose}>
    <section className="publication-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-record-title" onMouseDown={(event) => event.stopPropagation()}>
      <header><span><Trash2 size={20} /></span><div><small>不可撤销</small><h2 id="delete-record-title">删除发布记录</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div><strong>{record.title}</strong><p>将删除这条发布记录及其仓库内认证 JPG 副本、C2PA 清单与 TrustMark ID。</p><div className="delete-detail">首次导出的 JPG 会保留在原发布路径，不会由此操作删除。</div></div>
      <footer><button className="text-button" type="button" onClick={onClose}>保留记录</button><button className="danger-button solid" type="button" disabled={busy} onClick={onConfirm}>{busy && <LoaderCircle className="spin" size={15} />}确认删除记录</button></footer>
    </section>
  </div>;
}

function ReadOnlyField({ label, value, multiline = false }: { label: string; value: string; multiline?: boolean }) {
  return <div className={`readonly-field${multiline ? " multiline" : ""}`}><span>{label}</span><strong>{value}</strong></div>;
}

function PublicationDeleteDialog({ branchTitle, busy, onClose, onConfirm }: { branchTitle: string; busy: boolean; onClose: () => void; onConfirm: () => void }) {
  return <div className="dialog-backdrop" onMouseDown={onClose}>
    <section className="publication-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-publication-title" onMouseDown={(event) => event.stopPropagation()}>
      <header><span><Trash2 size={20} /></span><div><small>不可撤销</small><h2 id="delete-publication-title">删除本地发布数据</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div><strong>{branchTitle}</strong><p>将删除该分支的仓库内最终成品、全部认证记录、认证 JPG 副本和保存配置，并解除分支锁定。</p><div className="delete-detail">首次导出的 JPG 会保留在原发布路径，不会由此操作删除。</div></div>
      <footer><button className="text-button" type="button" onClick={onClose}>保留发布内容</button><button className="danger-button solid" type="button" disabled={busy} onClick={onConfirm}>{busy && <LoaderCircle className="spin" size={15} />}确认删除本地数据</button></footer>
    </section>
  </div>;
}

export function PublicationPreviewDialog({ preview, busy, cancelling, onBack, onCancel, onPublish }: {
  preview: PublicationPreview;
  busy: boolean;
  cancelling: boolean;
  onBack: () => void;
  onCancel: () => void;
  onPublish: () => void;
}) {
  // 没有独立的“适应”模式：缩放始终是数值，并以最终成品像素为基准——100% 表示一个
  // 源像素落在屏幕上的一个 CSS 像素。null 表示尚未手动缩放，此时跟随由画布尺寸算
  // 出的适应倍率；打开预览即为数值缩放，画布拖拽从打开起即可用。
  const [zoom, setZoom] = useState<number | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const [viewport, setViewport] = useState<PreviewViewport | null>(null);
  const [canvasSize, setCanvasSize] = useState<{ width: number; height: number } | null>(null);
  const [tile, setTile] = useState<{ key: string; request: TileRequest; image: PreviewImage; source: PreviewTileSource } | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null);
  const navigatorDragRef = useRef<number | null>(null);
  const zoomAnchorRef = useRef<{ xRatio: number; yRatio: number; canvasX: number; canvasY: number } | null>(null);
  const decodedImagesRef = useRef(new Map<string, Promise<void>>());
  const tileCacheRef = useRef(new Map<string, PreviewImage>());
  const tileRequestRef = useRef(0);
  const [imageSwitching, setImageSwitching] = useState(false);
  const image = showOriginal ? preview.originalImage : preview.image;
  const tileSource: PreviewTileSource = showOriginal ? "original" : "compressed";
  const fitZoom = canvasSize && canvasSize.width > 0 && canvasSize.height > 0
    && preview.sourceWidth > 0 && preview.sourceHeight > 0
    ? Math.min(4, Math.min(canvasSize.width / preview.sourceWidth, canvasSize.height / preview.sourceHeight))
    : 1;
  const effectiveZoom = zoom ?? fitZoom;
  // 内容与叠加层都以源像素尺寸为基准：显示尺寸 = 源像素 × 缩放倍率，因此缩放标签
  // 上的 100% 就是源图 1:1，缩略图只作为放大前的底图。
  const displayWidth = preview.sourceWidth * effectiveZoom;
  const displayHeight = preview.sourceHeight * effectiveZoom;
  const stateRef = useRef({ imageWidth: preview.sourceWidth, imageHeight: preview.sourceHeight, fit: fitZoom, effective: effectiveZoom });
  stateRef.current = { imageWidth: preview.sourceWidth, imageHeight: preview.sourceHeight, fit: fitZoom, effective: effectiveZoom };
  const decodeImage = useCallback((dataUrl: string) => {
    const cached = decodedImagesRef.current.get(dataUrl);
    if (cached) return cached;
    const promise = new Promise<void>((resolve) => {
      const preload = new Image();
      preload.decoding = "async";
      preload.onload = () => { void preload.decode().catch(() => undefined).finally(resolve); };
      preload.onerror = () => resolve();
      preload.src = dataUrl;
    });
    decodedImagesRef.current.set(dataUrl, promise);
    return promise;
  }, []);
  const toggleOriginal = async () => {
    const next = !showOriginal;
    setImageSwitching(true);
    await decodeImage((next ? preview.originalImage : preview.image).dataUrl);
    setShowOriginal(next);
    setImageSwitching(false);
  };
  const measuredZoom = () => {
    const renderedImage = imageRef.current;
    const state = stateRef.current;
    if (renderedImage && renderedImage.clientWidth > 0 && state.imageWidth > 0) {
      return renderedImage.clientWidth / state.imageWidth;
    }
    return state.effective;
  };
  const changeZoom = useCallback((next: number, clientX?: number, clientY?: number) => {
    const canvas = canvasRef.current;
    const renderedImage = imageRef.current;
    if (canvas && renderedImage) {
      const canvasBounds = canvas.getBoundingClientRect();
      const imageBounds = renderedImage.getBoundingClientRect();
      const anchorX = clientX ?? canvasBounds.left + canvasBounds.width / 2;
      const anchorY = clientY ?? canvasBounds.top + canvasBounds.height / 2;
      const insideImage = anchorX >= imageBounds.left && anchorX <= imageBounds.right
        && anchorY >= imageBounds.top && anchorY <= imageBounds.bottom;
      zoomAnchorRef.current = {
        xRatio: insideImage ? (anchorX - imageBounds.left) / imageBounds.width : 0.5,
        yRatio: insideImage ? (anchorY - imageBounds.top) / imageBounds.height : 0.5,
        canvasX: anchorX - canvasBounds.left,
        canvasY: anchorY - canvasBounds.top,
      };
    }
    setZoom(clampPreviewZoom(next, stateRef.current.fit));
  }, []);
  // React 的 onWheel 以 passive 方式注册，preventDefault 会被忽略；改用原生非被动
  // 监听，避免滚轮在缩放预览的同时滚动外层容器。
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const handler = (event: WheelEvent) => {
      event.preventDefault();
      const renderedImage = imageRef.current;
      const state = stateRef.current;
      const current = renderedImage && renderedImage.clientWidth > 0 && state.imageWidth > 0
        ? renderedImage.clientWidth / state.imageWidth
        : state.effective;
      changeZoom(previewZoomFromWheel(current, event.deltaY, state.fit), event.clientX, event.clientY);
    };
    canvas.addEventListener("wheel", handler, { passive: false });
    return () => canvas.removeEventListener("wheel", handler);
  }, [changeZoom]);
  const syncViewport = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    setViewport({
      scrollLeft: canvas.scrollLeft,
      scrollTop: canvas.scrollTop,
      scrollWidth: canvas.scrollWidth,
      scrollHeight: canvas.scrollHeight,
      clientWidth: canvas.clientWidth,
      clientHeight: canvas.clientHeight,
    });
  }, []);
  // 画布尺寸变化时同步适应倍率与视口；zoom 为 null 时预览自动跟随适应倍率。
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const update = () => {
      setCanvasSize({ width: canvas.clientWidth, height: canvas.clientHeight });
      syncViewport();
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(canvas);
    const frame = requestAnimationFrame(update);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [image?.dataUrl, syncViewport]);
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const anchor = zoomAnchorRef.current;
    if (!anchor) return;
    // 图片在滚动内容中由网格居中；偏移只依赖已知几何量，不依赖 offsetParent。
    const state = stateRef.current;
    const imageWidth = state.imageWidth * state.effective;
    const imageHeight = state.imageHeight * state.effective;
    const offsetLeft = Math.max(0, (Math.max(canvas.clientWidth, imageWidth) - imageWidth) / 2);
    const offsetTop = Math.max(0, (Math.max(canvas.clientHeight, imageHeight) - imageHeight) / 2);
    const target = zoomAnchorScrollTarget(anchor, offsetLeft, offsetTop, imageWidth, imageHeight);
    canvas.scrollLeft = target.scrollLeft;
    canvas.scrollTop = target.scrollTop;
    zoomAnchorRef.current = null;
    syncViewport();
  }, [image.dataUrl, effectiveZoom, syncViewport]);
  // 高清局部：只有缩略图被放大（显示尺寸超过缩略图本身）时才需要，此时按可视区域
  // 请求源分辨率裁剪块并叠加显示，让导出图与原始成品在放大后仍从源像素渲染，而不是
  // 插值放大 2400 px 缩略图。缩略图未被放大时直接显示缩略图即为 1:1 或更好。
  const desiredTile = tileRequestForView({
    displayWidth,
    displayHeight,
    thumbWidth: image.width,
    thumbHeight: image.height,
    sourceWidth: preview.sourceWidth,
    sourceHeight: preview.sourceHeight,
    viewport,
  });
  const desiredTileKey = desiredTile ? `${tileSource}|${tileCacheKey(desiredTile)}` : "";
  const desiredTileRef = useRef<{ request: TileRequest; source: PreviewTileSource } | null>(null);
  desiredTileRef.current = desiredTile ? { request: desiredTile, source: tileSource } : null;
  useEffect(() => {
    // 只有与当前对位一致（key 相同）的局部图才会被渲染，因此签名进行中可以直接保留
    // 已加载的局部图，不必为了不请求而把它摘掉。
    if (!desiredTileKey || busy) return;
    const target = desiredTileRef.current;
    if (!target) return;
    // 每次目标矩形或来源变化都推进代次：平移途中的在途响应按代次丢弃，避免旧
    // 矩形错位叠加。
    const requestId = ++tileRequestRef.current;
    const cached = tileCacheRef.current.get(desiredTileKey);
    if (cached) {
      setTile({ key: desiredTileKey, request: target.request, image: cached, source: target.source });
      return;
    }
    const timer = window.setTimeout(() => {
      authenticityApi.previewTile({
        source: target.source,
        cacheToken: target.source === "compressed" ? preview.cacheToken : null,
        branchId: target.source === "original" ? preview.branchId : null,
        ...target.request,
      }).then((next) => {
        if (requestId !== tileRequestRef.current) return;
        const cache = tileCacheRef.current;
        cache.set(desiredTileKey, next);
        while (cache.size > 12) {
          const oldest = cache.keys().next();
          if (oldest.done) break;
          cache.delete(oldest.value);
        }
        setTile({ key: desiredTileKey, request: target.request, image: next, source: target.source });
      }).catch(() => undefined);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [desiredTileKey, busy, preview.cacheToken, preview.branchId]);
  useEffect(() => () => { tileRequestRef.current += 1; }, []);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !canvasRef.current) return;
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      scrollLeft: canvasRef.current.scrollLeft,
      scrollTop: canvasRef.current.scrollTop,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId || !canvasRef.current) return;
    canvasRef.current.scrollLeft = drag.scrollLeft - (event.clientX - drag.x);
    canvasRef.current.scrollTop = drag.scrollTop - (event.clientY - drag.y);
    syncViewport();
  };
  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId === event.pointerId) dragRef.current = null;
  };
  const navigable = viewport != null
    && (viewport.scrollWidth > viewport.clientWidth || viewport.scrollHeight > viewport.clientHeight);
  const moveFromNavigator = (event: ReactPointerEvent<HTMLDivElement>) => {
    const canvas = canvasRef.current;
    if (!canvas || !viewport) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const target = navigatorScrollTarget(
      viewport,
      (event.clientX - bounds.left) / bounds.width,
      (event.clientY - bounds.top) / bounds.height,
    );
    canvas.scrollLeft = target.scrollLeft;
    canvas.scrollTop = target.scrollTop;
    syncViewport();
  };
  const onNavigatorPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    navigatorDragRef.current = event.pointerId;
    event.currentTarget.setPointerCapture(event.pointerId);
    moveFromNavigator(event);
  };
  const onNavigatorPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (navigatorDragRef.current === event.pointerId) moveFromNavigator(event);
  };
  const onNavigatorPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (navigatorDragRef.current === event.pointerId) navigatorDragRef.current = null;
  };
  const navigationRect = viewport ? navigatorRect(viewport) : null;
  return <div className="dialog-backdrop publication-preview-backdrop" onMouseDown={() => { if (!busy) onBack(); }}>
    <section className="publication-preview-dialog" role="dialog" aria-modal="true" aria-labelledby="publication-preview-title" onMouseDown={(event) => event.stopPropagation()}>
      <header>
        <div><small>发布前检查</small><h2 id="publication-preview-title">导出预览</h2><span>{preview.sourceWidth} x {preview.sourceHeight} · {formatBytes(preview.outputBytes)} · {preview.cacheHit ? "复用缓存" : `渲染 ${preview.renderMs} ms / 编码 ${preview.encodeMs} ms`}</span></div>
        <div className="preview-zoom-controls">
          <button className="icon-button" type="button" title="缩小" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), -1, stateRef.current.fit))}><ZoomOut size={16} /></button>
          <button className="zoom-value" type="button" title="按原始像素显示（100% 为 1:1）" onClick={() => changeZoom(1)}>{Math.round(effectiveZoom * 100)}%</button>
          <button className="icon-button" type="button" title="放大" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), 1, stateRef.current.fit))}><ZoomIn size={16} /></button>
          <button className="icon-button" type="button" title="适应窗口" onClick={() => setZoom(null)}><Maximize2 size={16} /></button>
          <button className={`icon-button${showOriginal ? " active" : ""}`} type="button" title={showOriginal ? "显示压缩预览" : "显示原图"} disabled={imageSwitching} onClick={() => void toggleOriginal()}><ImageIcon size={16} /></button>
          <button className="icon-button" type="button" title="关闭预览" disabled={busy} onClick={onBack}><X size={17} /></button>
        </div>
      </header>
      <div className="publication-preview-stage">
        <div
          ref={canvasRef}
          className="publication-preview-canvas"
          onScroll={syncViewport}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDragStart={(event) => event.preventDefault()}
        >
          <div
            className="publication-preview-content"
            style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }}
          >
            <div
              className="publication-preview-image"
              style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }}
            >
              <img ref={imageRef} src={image.dataUrl} alt={showOriginal ? "原始成品预览" : "导出预览"} draggable={false} onLoad={syncViewport} style={{ width: `${displayWidth}px`, height: `${displayHeight}px` }} />
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
        {navigable && navigationRect && <div
          className="publication-preview-navigator"
          title="拖动以定位预览区域"
          style={{ aspectRatio: `${image.width} / ${image.height}` }}
          onPointerDown={onNavigatorPointerDown}
          onPointerMove={onNavigatorPointerMove}
          onPointerUp={onNavigatorPointerUp}
          onPointerCancel={onNavigatorPointerUp}
        >
          <img src={image.dataUrl} alt="" draggable={false} />
          <span style={{ left: `${navigationRect.left}%`, top: `${navigationRect.top}%`, width: `${navigationRect.width}%`, height: `${navigationRect.height}%` }} />
        </div>}
      </div>
      <footer><span>{busy ? (cancelling ? "正在安全取消；已开始的原子发布收尾不会中断。" : "正在写入 C2PA；时间戳服务最长等待 30 秒。") : showOriginal ? "当前显示原始成品，用于快速对比；放大后叠加源分辨率局部。" : "缩略图使用正式发布的背景合成、TrustMark 与 JPEG 编码参数；放大后叠加源分辨率局部。"}</span><div><button className="secondary-button" type="button" disabled={busy} onClick={onBack}>返回调整</button><button className={busy ? "secondary-button" : "primary-button"} type="button" disabled={cancelling} onClick={busy ? onCancel : onPublish}>{busy ? (cancelling ? <LoaderCircle className="spin" size={16} /> : <X size={16} />) : <ImageDown size={16} />}{busy ? (cancelling ? "正在取消" : "取消签名") : "签名并发布"}</button></div></footer>
    </section>
  </div>;
}

function RecordList({ records, onNavigate, compact = false, selectedId = null, evidence = {} }: { records: CertificationRecord[]; onNavigate: (record: CertificationRecord) => void; compact?: boolean; selectedId?: string | null; evidence?: Record<string, string[]> }) {
  return <section className={`record-list${compact ? " compact" : ""}`}>
    {!compact && <header><strong>分支导出记录</strong><span>{records.length} 条</span></header>}
    {records.length === 0 ? <div className="records-empty">没有匹配的导出记录。</div> : records.map((record) => <button type="button" data-record-id={record.id} className={`record-row${selectedId === record.id ? " selected" : ""}`} key={record.id} onClick={() => onNavigate(record)}>
      <BadgeCheck size={16} />
      <span><strong>{record.title}</strong><small>{record.creator || "作者未声明"} · {record.artworkTitle} / {record.branchTitle} · {new Date(record.createdMs).toLocaleString()}</small><small>{record.outputPath} · {formatBytes(record.outputBytes)}</small>{evidence[record.id] && <small>候选证据：{evidence[record.id].map((source) => source === "c2pa" ? "C2PA" : "TrustMark").join(" + ")}</small>}<code>{record.watermarkId ?? "无 TrustMark"}</code></span>
      <i>{record.trustmarkEnabled ? "C2PA + TrustMark" : "C2PA"}</i>
    </button>)}
  </section>;
}
