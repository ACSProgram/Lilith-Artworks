import {
  BadgeCheck, Eye, FileImage, Fingerprint, FolderOpen, Image as ImageIcon, ImageDown, LoaderCircle,
  LockKeyhole, Maximize2, MoreVertical, MousePointer2, RotateCcw, ScanSearch, Search,
  ShieldCheck, Trash2, X, ZoomIn, ZoomOut,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { CleanupReport } from "../../shared/fileCleanup";
import { formatBytes } from "../../shared/format";
import type {
  AuthenticityBranch, CertificationRecord, NormalizedRegion, PreviewImage, PublicationPreview,
} from "./types";
import {
  clampPreviewZoom, navigatorRect, navigatorScrollTarget, previewZoomFromButton,
  previewZoomFromWheel, type PreviewViewport, zoomAnchorScrollTarget,
} from "./previewViewport";
import { useIdentificationController, usePublicationController } from "./useAuthenticityController";

interface AuthenticityModuleProps {
  mode: "publish" | "identify";
  artworkTitle: string;
  branches: AuthenticityBranch[];
  selectedBranchId: string | null;
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
  artworkTitle, branches, selectedBranchId, selectedRecordId, recordNavigationKey, onSelectBranch, onError, onNavigateRecord, onRetryFileCleanup, onPublicationChanged,
}: AuthenticityModuleProps) {
  const {
    publication, config, setConfig, preview, outputPreview, outputPreviewOpen,
    setOutputPreviewOpen, outputPreviewBusy, privateKey, setPrivateKey,
    busy, result, sizeEstimate, viewingRecord, setViewingRecord,
    viewingPreview, exporting, deleteConfirmOpen, setDeleteConfirmOpen, cleanupFailures,
    selectedBranch, enterPublication, chooseCertificate, generateOutputPreview, publish, cancelPublication,
    retryCleanup, openRecord, exportRecord,
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

  if (viewingRecord) return <RecordView record={viewingRecord} preview={viewingPreview} exporting={exporting} onExport={exportRecord} onClose={() => setViewingRecord(null)} />;

  return <div className="auth-workspace">
    <header className="auth-header">
      <div><span>发布与认证</span><h1>{artworkTitle}</h1></div>
      <div className="auth-header-actions">
        <select value={selectedBranchId ?? ""} disabled={busy} onChange={(event) => onSelectBranch(event.target.value)}>
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
    {!selectedBranch ? <div className="auth-empty">此 Artwork 尚无分支。</div> :
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
            <label>签名算法<select value={config.signingAlgorithm} onChange={(event) => setConfig({ ...config, signingAlgorithm: event.target.value })}><option value="es256">ES256</option><option value="es384">ES384</option><option value="ps256">PS256</option><option value="ed25519">Ed25519</option></select></label>
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
          {result && <div className="publish-success"><BadgeCheck size={18} /><div><strong>认证发布完成</strong><span>{result.outputPath}</span><code>{result.watermarkId}</code></div></div>}
          <button className="primary-button publish-command" type="button" disabled={busy || outputPreviewBusy} onClick={() => void generateOutputPreview()}>{outputPreviewBusy ? <LoaderCircle className="spin" size={17} /> : <Eye size={17} />}生成质量预览</button>
        </section>
        <RecordList records={publication.records} onNavigate={openRecord} selectedId={selectedRecordId} />
        {outputPreviewOpen && outputPreview && <PublicationPreviewDialog preview={outputPreview} busy={busy} onBack={() => setOutputPreviewOpen(false)} onPublish={() => void publish()} />}
        {deleteConfirmOpen && <PublicationDeleteDialog branchTitle={selectedBranch.title} busy={busy} onClose={() => setDeleteConfirmOpen(false)} onConfirm={() => void cancelPublication()} />}
      </div> : <div className="auth-empty"><LoaderCircle className="spin" size={18} />读取发布状态</div>}
  </div>;
}

function IdentifyView({ onError, onNavigateRecord }: Pick<AuthenticityModuleProps, "onError" | "onNavigateRecord">) {
  const {
    path, preview, region, setRegion, result, query, setQuery, records, busy, searching,
    choose, decode, searchRecords,
  } = useIdentificationController({ onError });

  return <div className="auth-workspace identify-workspace">
    <header className="auth-header"><div><span>识别与溯源</span><h1>验证发布图片</h1></div></header>
    <div className="identify-layout">
      <section className="auth-preview-panel identify-preview">
        {!preview ? <button className="image-empty" type="button" onClick={() => void choose()}><ScanSearch size={28} /><strong>选择待识别图片</strong><span>C2PA 会始终读取；TrustMark 可识别整图或框选区域。</span></button> : <>
          <header><div><strong>{fileName(path)}</strong><span>{preview.width} x {preview.height}</span></div><button className="text-button" type="button" onClick={() => void choose()}>更换图片</button></header>
          <RegionEditor target="decode" preview={preview} regions={region ? [region] : []} maxRegions={1} onChange={(regions) => setRegion(regions[0] ?? null)} />
          <div className="decode-scope"><Fingerprint size={17} /><span>{region ? "识别框选区域" : "识别整张图片"}</span>{region && <button className="icon-button" type="button" title="取消区域并识别整图" onClick={() => setRegion(null)}><X size={15} /></button>}</div>
          {region && <button className="text-button scope-reset" type="button" onClick={() => setRegion(null)}>改用整图</button>}
          <button className="primary-button" type="button" disabled={busy} onClick={() => void decode()}>{busy ? <LoaderCircle className="spin" size={16} /> : <ScanSearch size={16} />}开始识别</button>
        </>}
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

function RegionEditor({ target, preview, regions, maxRegions, onChange, readOnly = false }: {
  target: ImageTarget;
  preview: PreviewImage;
  regions: NormalizedRegion[];
  maxRegions: number;
  onChange: (regions: NormalizedRegion[]) => void;
  readOnly?: boolean;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [frame, setFrame] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
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
    if (target === "decode" && regions.length > 0) onChange([]);
    drag.current = { pointerId: event.pointerId, ...start };
    const next = { x: start.x, y: start.y, width: 0, height: 0 };
    draftRef.current = next;
    setDraft(next);
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    const next = point(event);
    if (!next) return;
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
      <div className="region-layer" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={finish} onPointerCancel={finish}>
        {regions.map((region, index) => <div className="region-box" style={style(region)} key={`${region.x}-${region.y}-${index}`}><span>{target === "publish" ? `区域 ${index + 1}` : "识别区域"}</span>{target === "publish" && !readOnly && <button type="button" title="移除区域" onPointerDown={(event) => event.stopPropagation()} onClick={() => onChange(regions.filter((_, item) => item !== index))}><X size={12} /></button>}</div>)}
        {draft && <div className="region-box draft" style={style(draft)}><span>框选中</span></div>}
      </div>
    </div>}
    {target === "publish" && regions.length > 0 && !readOnly && <button className="clear-regions" type="button" onClick={() => onChange([])}><Trash2 size={13} />清除区域</button>}
  </div>;
}

function RecordView({ record, preview, exporting, onExport, onClose }: {
  record: CertificationRecord;
  preview: PreviewImage | null;
  exporting: boolean;
  onExport: (record: CertificationRecord) => Promise<void>;
  onClose: () => void;
}) {
  return <div className="auth-workspace record-view-mode">
    <header className="auth-header"><div><span>发布记录 · 只读</span><h1>{record.title}</h1></div><div className="record-view-actions"><button className="secondary-button" type="button" disabled={exporting} onClick={() => void onExport(record)}>{exporting ? <LoaderCircle className="spin" size={15} /> : <ImageDown size={15} />}再次导出</button><button className="secondary-button" type="button" onClick={onClose}><X size={15} />退出查看</button></div></header>
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

function PublicationPreviewDialog({ preview, busy, onBack, onPublish }: {
  preview: PublicationPreview;
  busy: boolean;
  onBack: () => void;
  onPublish: () => void;
}) {
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [showOriginal, setShowOriginal] = useState(false);
  const [viewport, setViewport] = useState<PreviewViewport | null>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null);
  const navigatorDragRef = useRef<number | null>(null);
  const fitZoomRef = useRef(1);
  const zoomAnchorRef = useRef<{ xRatio: number; yRatio: number; canvasX: number; canvasY: number } | null>(null);
  const decodedImagesRef = useRef(new Map<string, Promise<void>>());
  const [imageSwitching, setImageSwitching] = useState(false);
  const image = showOriginal ? preview.originalImage : preview.image;
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
  useEffect(() => {
    const timer = window.setTimeout(() => { void decodeImage(preview.originalImage.dataUrl); }, 120);
    return () => window.clearTimeout(timer);
  }, [decodeImage, preview.originalImage.dataUrl]);
  const toggleOriginal = async () => {
    const next = !showOriginal;
    setImageSwitching(true);
    await decodeImage((next ? preview.originalImage : preview.image).dataUrl);
    setShowOriginal(next);
    setZoom("fit");
    setImageSwitching(false);
  };
  const measuredZoom = () => {
    const renderedImage = imageRef.current;
    return renderedImage && renderedImage.clientWidth > 0 ? renderedImage.clientWidth / image.width : zoom === "fit" ? fitZoomRef.current : zoom;
  };
  const changeZoom = (next: number, clientX?: number, clientY?: number) => {
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
    setZoom(clampPreviewZoom(next, fitZoomRef.current));
  };
  const onWheel = (event: React.WheelEvent<HTMLDivElement>) => {
    event.preventDefault();
    changeZoom(previewZoomFromWheel(measuredZoom(), event.deltaY, fitZoomRef.current), event.clientX, event.clientY);
  };
  const syncViewport = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    if (zoom === "fit" && imageRef.current?.clientWidth) {
      fitZoomRef.current = imageRef.current.clientWidth / image.width;
    }
    setViewport({
      scrollLeft: canvas.scrollLeft,
      scrollTop: canvas.scrollTop,
      scrollWidth: canvas.scrollWidth,
      scrollHeight: canvas.scrollHeight,
      clientWidth: canvas.clientWidth,
      clientHeight: canvas.clientHeight,
    });
  }, [image.width, zoom]);
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const renderedImage = imageRef.current;
    if (!canvas || !renderedImage) return;
    if (zoom === "fit") {
      canvas.scrollLeft = 0;
      canvas.scrollTop = 0;
      zoomAnchorRef.current = null;
      return;
    }
    const anchor = zoomAnchorRef.current;
    if (!anchor) return;
    const target = zoomAnchorScrollTarget(
      anchor,
      renderedImage.offsetLeft,
      renderedImage.offsetTop,
      renderedImage.clientWidth,
      renderedImage.clientHeight,
    );
    canvas.scrollLeft = target.scrollLeft;
    canvas.scrollTop = target.scrollTop;
    zoomAnchorRef.current = null;
    syncViewport();
  }, [image.dataUrl, syncViewport, zoom]);
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    syncViewport();
    const observer = new ResizeObserver(syncViewport);
    observer.observe(canvas);
    const frame = requestAnimationFrame(syncViewport);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [image?.dataUrl, syncViewport, zoom]);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (zoom === "fit" || event.button !== 0 || !canvasRef.current) return;
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
  const navigable = zoom !== "fit" && viewport != null
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
        <div><small>发布前检查</small><h2 id="publication-preview-title">导出预览</h2><span>{preview.image.width} x {preview.image.height} · {formatBytes(preview.outputBytes)}</span></div>
        <div className="preview-zoom-controls">
          <button className="icon-button" type="button" title="缩小" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), -1, fitZoomRef.current))}><ZoomOut size={16} /></button>
          <button className="zoom-value" type="button" title="按原始像素显示" onClick={() => changeZoom(1)}>{zoom === "fit" ? "适应" : `${Math.round(zoom * 100)}%`}</button>
          <button className="icon-button" type="button" title="放大" onClick={() => changeZoom(previewZoomFromButton(measuredZoom(), 1, fitZoomRef.current))}><ZoomIn size={16} /></button>
          <button className="icon-button" type="button" title="适应窗口" onClick={() => setZoom("fit")}><Maximize2 size={16} /></button>
          <button className={`icon-button${showOriginal ? " active" : ""}`} type="button" title={showOriginal ? "显示压缩预览" : "显示原图"} disabled={imageSwitching} onClick={() => void toggleOriginal()}><ImageIcon size={16} /></button>
          <button className="icon-button" type="button" title="关闭预览" disabled={busy} onClick={onBack}><X size={17} /></button>
        </div>
      </header>
      <div className="publication-preview-stage">
        <div
          ref={canvasRef}
          className={`publication-preview-canvas${zoom === "fit" ? " fit" : ""}`}
          onScroll={syncViewport}
          onWheel={onWheel}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDragStart={(event) => event.preventDefault()}
        >
          <div
            className="publication-preview-content"
            style={zoom === "fit" ? undefined : { width: `${image.width * zoom}px`, height: `${image.height * zoom}px` }}
          >
            <img ref={imageRef} src={image.dataUrl} alt={showOriginal ? "原始成品预览" : "导出预览"} draggable={false} onLoad={syncViewport} style={zoom === "fit" ? undefined : { width: `${image.width * zoom}px` }} />
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
      <footer><span>{showOriginal ? "当前显示原始成品，用于快速对比。" : "预览使用正式发布的背景合成、TrustMark 与 JPEG 编码参数。"}</span><div><button className="secondary-button" type="button" disabled={busy} onClick={onBack}>返回调整</button><button className="primary-button" type="button" disabled={busy} onClick={onPublish}>{busy ? <LoaderCircle className="spin" size={16} /> : <ImageDown size={16} />}签名并发布</button></div></footer>
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
