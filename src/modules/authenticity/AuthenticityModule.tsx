import { open, save } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle, BadgeCheck, FileImage, Fingerprint, FolderOpen, ImageDown, LoaderCircle,
  LockKeyhole, MousePointer2, RotateCcw, ScanSearch, Search, ShieldCheck, Trash2, X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { formatBytes } from "../../shared/format";
import { appApi } from "../../app/api";
import type { CleanupFailure } from "../../app/types";
import type { ArtworkBranch } from "../history/types";
import { authenticityApi } from "./api";
import type {
  BranchPublication, CertificationConfig, CertificationRecord, DecodeResult,
  NormalizedRegion, PreviewImage,
} from "./types";

interface AuthenticityModuleProps {
  mode: "publish" | "identify";
  artworkTitle: string;
  branches: ArtworkBranch[];
  selectedBranchId: string | null;
  selectedRecordId?: string | null;
  onSelectBranch: (branchId: string) => void;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
  onPublicationChanged?: () => Promise<void>;
}

type ImageTarget = "publish" | "decode";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function privateKeyPem(value: string): string {
  const trimmed = value.trim();
  const pem = trimmed.match(/-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----([\s\S]*?)-----END \1-----/i);
  const label = pem?.[1]?.toUpperCase() || "PRIVATE KEY";
  const body = (pem?.[2] || trimmed).replace(/\s+/g, "");
  const lines = body.match(/.{1,64}/g)?.join("\n") || "";
  return `-----BEGIN ${label}-----\n${lines}\n-----END ${label}-----`;
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

const SHARED_SIGNING_KEY = "lilith-artworks.certification-signing-v1";

function loadSharedSigning(): Partial<CertificationConfig> {
  try { return JSON.parse(localStorage.getItem(SHARED_SIGNING_KEY) ?? "{}"); }
  catch { return {}; }
}

function saveSharedSigning(config: CertificationConfig) {
  localStorage.setItem(SHARED_SIGNING_KEY, JSON.stringify({ certificatePath: config.certificatePath, signingAlgorithm: config.signingAlgorithm, timestampUrl: config.timestampUrl }));
}

export function AuthenticityModule(props: AuthenticityModuleProps) {
  return props.mode === "publish"
    ? <PublishView {...props} />
    : <IdentifyView onError={props.onError} onNavigateRecord={props.onNavigateRecord} />;
}

function PublishView({
  artworkTitle, branches, selectedBranchId, selectedRecordId, onSelectBranch, onError, onNavigateRecord, onPublicationChanged,
}: AuthenticityModuleProps) {
  const [publication, setPublication] = useState<BranchPublication | null>(null);
  const [config, setConfig] = useState<CertificationConfig | null>(null);
  const [preview, setPreview] = useState<PreviewImage | null>(null);
  const [privateKey, setPrivateKey] = useState("");
  const [watermarkId, setWatermarkId] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CertificationRecord | null>(null);
  const [sizeEstimate, setSizeEstimate] = useState<number | null>(null);
  const [viewingRecord, setViewingRecord] = useState<CertificationRecord | null>(null);
  const [viewingPreview, setViewingPreview] = useState<PreviewImage | null>(null);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [cleanupFailures, setCleanupFailures] = useState<CleanupFailure[]>([]);
  const loadRequest = useRef(0);
  const estimateRequest = useRef(0);
  const selectedBranchIdRef = useRef(selectedBranchId);
  selectedBranchIdRef.current = selectedBranchId;
  const selectedBranch = branches.find((branch) => branch.id === selectedBranchId) ?? null;

  const load = useCallback(async () => {
    if (!selectedBranchId) return;
    const branchId = selectedBranchId;
    const requestId = ++loadRequest.current;
    setBusy(true);
    try {
      const next = await authenticityApi.getPublication(branchId);
      const nextPreview = next.artifact ? await authenticityApi.preview(next.artifact.sourcePath) : null;
      if (requestId !== loadRequest.current || selectedBranchIdRef.current !== branchId) return;
      setPublication(next);
      setConfig({ ...next.config, ...loadSharedSigning(), trustmarkEnabled: next.modelsReady && next.config.trustmarkEnabled && next.config.additionalRegions.length > 0 });
      setPreview(nextPreview);
    } catch (error) {
      if (requestId === loadRequest.current) onError(message(error));
    } finally {
      if (requestId === loadRequest.current) setBusy(false);
    }
  }, [onError, selectedBranchId]);

  useEffect(() => {
    setPublication(null);
    setConfig(null);
    setPreview(null);
    setViewingRecord(null);
    setBusy(Boolean(selectedBranchId));
    void load();
    return () => { loadRequest.current += 1; };
  }, [load, selectedBranchId]);
  useEffect(() => {
    if (!publication?.artifact || !config) return;
    const requestId = ++estimateRequest.current;
    setSizeEstimate(null);
    const timer = window.setTimeout(() => {
      authenticityApi.estimate(publication.artifact!.sourcePath, config.jpegQuality, config.backgroundColor)
        .then((value) => { if (requestId === estimateRequest.current) setSizeEstimate(value.jpegBytes); })
        .catch(() => { if (requestId === estimateRequest.current) setSizeEstimate(null); });
    }, 180);
    return () => { window.clearTimeout(timer); estimateRequest.current += 1; };
  }, [config?.backgroundColor, config?.jpegQuality, publication?.artifact]);
  useEffect(() => { if (config) saveSharedSigning(config); }, [config?.certificatePath, config?.signingAlgorithm, config?.timestampUrl]);
  useEffect(() => {
    if (!selectedRecordId || !publication) return;
    window.requestAnimationFrame(() => document.querySelector(`[data-record-id="${selectedRecordId}"]`)?.scrollIntoView({ block: "center" }));
  }, [publication, selectedRecordId]);
  useEffect(() => {
    let cancelled = false;
    setViewingPreview(null);
    if (viewingRecord) {
      authenticityApi.previewRecord(viewingRecord.id)
        .then((next) => { if (!cancelled) setViewingPreview(next); })
        .catch((error) => { if (!cancelled) onError(message(error)); });
    }
    return () => { cancelled = true; };
  }, [onError, viewingRecord]);

  const enterPublication = async () => {
    if (!selectedBranch) return;
    const artifactPath = await open({
      multiple: false,
      filters: [{ name: "最终成品", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof artifactPath !== "string") return;
    setBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.enterPublication(selectedBranch.id, artifactPath);
      if (selectedBranchIdRef.current !== selectedBranch.id) {
        await onPublicationChanged?.();
        return;
      }
      setPublication(next);
      await onPublicationChanged?.();
      setConfig({ ...next.config, ...loadSharedSigning(), trustmarkEnabled: next.modelsReady && next.config.trustmarkEnabled && next.config.additionalRegions.length > 0 });
      if (next.artifact) setPreview(await authenticityApi.preview(next.artifact.sourcePath));
    } catch (error) {
      onError(message(error));
    } finally {
      setBusy(false);
    }
  };

  const chooseCertificate = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "证书链", extensions: ["pem", "crt", "cer"] }],
    });
    if (typeof path === "string" && config) setConfig({ ...config, certificatePath: path });
  };

  const publish = async () => {
    if (!selectedBranch || !config) return;
    if (publication?.branchId !== selectedBranch.id || publication.artifact?.branchId !== selectedBranch.id || config.branchId !== selectedBranch.id) {
      onError("当前分支的发布状态尚未加载完成，请稍后重试。");
      return;
    }
    if (!privateKey.trim()) {
      onError("请输入 PEM 私钥后再发布。");
      return;
    }
    const outputPath = await save({
      defaultPath: `${config.title.trim() || artworkTitle}-certified.jpg`,
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    if (!outputPath) return;
    setBusy(true);
    setResult(null);
    onError(null);
    try {
      const published = await authenticityApi.publish({
        branchId: selectedBranch.id,
        outputPath,
        privateKeyPem: privateKeyPem(privateKey),
        config,
        watermarkId: watermarkId.trim() || null,
      });
      setPrivateKey("");
      if (selectedBranchIdRef.current === selectedBranch.id) {
        setResult(published.record);
        await load();
      }
      await onPublicationChanged?.();
    } catch (error) {
      onError(message(error));
    } finally {
      setBusy(false);
    }
  };

  const cancelPublication = async () => {
    if (!selectedBranch) return;
    setBusy(true);
    try {
      const report = await authenticityApi.cancelPublication(selectedBranch.id);
      setCleanupFailures(report.failures);
      if (report.failures.length > 0) onError(`分支已解除发布状态，但有 ${report.failures.length} 个文件清理失败；请重试清理。`);
      setDeleteConfirmOpen(false);
      setPublication(null);
      setConfig(null);
      setPreview(null);
      await onPublicationChanged?.();
    }
    catch (error) { onError(message(error)); }
    finally { setBusy(false); }
  };

  const retryCleanup = async () => {
    if (cleanupFailures.length === 0) return;
    setBusy(true);
    onError(null);
    try {
      const report = await appApi.retryFileCleanup(cleanupFailures.map((failure) => failure.id));
      setCleanupFailures(report.failures);
      if (report.failures.length > 0) onError(`仍有 ${report.failures.length} 个文件无法清理。`);
    } catch (error) {
      onError(message(error));
    } finally {
      setBusy(false);
    }
  };

  if (viewingRecord) return <RecordView record={viewingRecord} preview={viewingPreview} onClose={() => setViewingRecord(null)} onError={onError} />;

  return <div className="auth-workspace">
    <header className="auth-header">
      <div><span>发布与认证</span><h1>{artworkTitle}</h1></div>
      <select value={selectedBranchId ?? ""} onChange={(event) => onSelectBranch(event.target.value)}>
        {branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}
      </select>
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
            <header><strong>TrustMark {publication.modelVariant}</strong><label className="switch-field"><span className="switch-copy"><strong>{config.trustmarkEnabled ? "已启用" : "不嵌入"}</strong></span><input className="switch-input" type="checkbox" checked={config.trustmarkEnabled} disabled={!publication.modelsReady || config.additionalRegions.length === 0} onChange={(event) => setConfig({ ...config, trustmarkEnabled: event.target.checked && config.additionalRegions.length > 0 })} /></label></header>
            <div className="trustmark-region-hint"><MousePointer2 size={16} /><span><strong>在左侧图片上拖动框选区域</strong><small>完成第一个框选后自动启用 TrustMark 水印；清空区域后自动关闭。</small></span></div>
            {!publication.modelsReady && <p className="auth-warning">TrustMark 模型不可用，仍可发布 C2PA 凭证。</p>}
            <details className="model-info"><summary>模型信息</summary><dl><div><dt>变体</dt><dd>{publication.modelVariant}</dd></div><div><dt>Encoder SHA-256</dt><dd><code>{publication.encoderSha256 ?? "不可用"}</code></dd></div><div><dt>Decoder SHA-256</dt><dd><code>{publication.decoderSha256 ?? "不可用"}</code></dd></div></dl></details>
            {config.trustmarkEnabled && <>
              <label>自定义 ID<input value={watermarkId} maxLength={40} placeholder="留空自动生成 40 位 ID" onChange={(event) => setWatermarkId(event.target.value.replace(/[^01]/g, ""))} /></label>
              <label className="range-field">TrustMark 强度 <output>{config.watermarkStrength.toFixed(2)}</output><input type="range" min={0.5} max={1.5} step={0.05} value={config.watermarkStrength} onChange={(event) => setConfig({ ...config, watermarkStrength: Number(event.target.value) })} />{config.watermarkStrength > 1 && <small className="auth-warning">超过 1.00 可能造成质量损失</small>}</label>
              <p>仅在 {config.additionalRegions.length} 个框选区域嵌入水印。</p>
            </>}
          </div>
          {result && <div className="publish-success"><BadgeCheck size={18} /><div><strong>认证发布完成</strong><span>{result.outputPath}</span><code>{result.watermarkId}</code></div></div>}
          <button className="primary-button publish-command" type="button" disabled={busy} onClick={() => void publish()}>{busy ? <LoaderCircle className="spin" size={17} /> : <ImageDown size={17} />}签名并导出 JPG</button>
        </section>
        <RecordList records={publication.records} onNavigate={(record) => { setViewingRecord(record); onNavigateRecord(record); }} selectedId={selectedRecordId} />
        <section className="publication-danger-zone">
          <div><AlertTriangle size={18} /><span><strong>移除整个分支的发布内容</strong><small>删除最终成品、全部认证记录、认证 JPG 副本及首次导出文件，并解除分支锁定。</small></span></div>
          <button className="danger-button solid" type="button" disabled={busy} onClick={() => setDeleteConfirmOpen(true)}><Trash2 size={15} />取消发布并全部删除</button>
        </section>
        {deleteConfirmOpen && <PublicationDeleteDialog branchTitle={selectedBranch.title} busy={busy} onClose={() => setDeleteConfirmOpen(false)} onConfirm={() => void cancelPublication()} />}
      </div> : <div className="auth-empty"><LoaderCircle className="spin" size={18} />读取发布状态</div>}
  </div>;
}

function IdentifyView({ onError, onNavigateRecord }: Pick<AuthenticityModuleProps, "onError" | "onNavigateRecord">) {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<PreviewImage | null>(null);
  const [region, setRegion] = useState<NormalizedRegion | null>(null);
  const [result, setResult] = useState<DecodeResult | null>(null);
  const [query, setQuery] = useState("");
  const [records, setRecords] = useState<CertificationRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const previewRequest = useRef(0);
  const decodeRequest = useRef(0);
  const searchRequest = useRef(0);

  const choose = async () => {
    const selected = await open({ multiple: false, filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }] });
    if (typeof selected !== "string") return;
    const requestId = ++previewRequest.current;
    decodeRequest.current += 1;
    setBusy(true);
    try {
      const nextPreview = await authenticityApi.preview(selected);
      if (requestId !== previewRequest.current) return;
      setPath(selected);
      setPreview(nextPreview);
      setRegion(null);
      setResult(null);
    } catch (error) { if (requestId === previewRequest.current) onError(message(error)); }
    finally { if (requestId === previewRequest.current) setBusy(false); }
  };

  const decode = async () => {
    if (!path) return;
    const requestId = ++decodeRequest.current;
    const inputPath = path;
    const inputRegion = region;
    setBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.decode(inputPath, inputRegion);
      if (requestId === decodeRequest.current) setResult(next);
    }
    catch (error) { if (requestId === decodeRequest.current) onError(message(error)); }
    finally { if (requestId === decodeRequest.current) setBusy(false); }
  };

  const searchRecords = async () => {
    const requestId = ++searchRequest.current;
    const requestedQuery = query;
    setBusy(true);
    try {
      const next = await authenticityApi.searchRecords(requestedQuery);
      if (requestId === searchRequest.current) setRecords(next);
    }
    catch (error) { if (requestId === searchRequest.current) onError(message(error)); }
    finally { if (requestId === searchRequest.current) setBusy(false); }
  };

  useEffect(() => {
    const timer = window.setTimeout(() => { void searchRecords(); }, 220);
    return () => window.clearTimeout(timer);
  }, [query]);

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
          <header className={result.c2paPresent ? "verified" : ""}><ShieldCheck size={20} /><div><strong>{result.c2paPresent ? "已读取 C2PA" : "未发现 C2PA"}</strong><span>{result.c2paValidationState ?? "无验证状态"}</span></div></header>
          <dl><div><dt>C2PA 记录 ID</dt><dd><code>{result.c2paRecordId ?? "未声明"}</code></dd></div><div><dt>C2PA TrustMark ID</dt><dd><code>{result.c2paWatermarkId ?? "未声明"}</code></dd></div><div><dt>识别出的 TrustMark ID</dt><dd><code>{result.watermarkId ?? "未识别"}</code></dd></div><div><dt>双通道</dt><dd>{result.identifiersMatch == null ? "只有单通道证据" : result.identifiersMatch ? "ID 一致" : "ID 冲突，需人工调查"}</dd></div></dl>
          <dl className="claim-grid"><div><dt>作品</dt><dd>{result.title ?? "未声明"}</dd></div><div><dt>创作者</dt><dd>{result.creator ?? "未声明"}</dd></div><div><dt>权利声明</dt><dd>{result.rightsStatement ?? "未声明"}</dd></div><div><dt>认证内容</dt><dd>{result.authenticationContent ?? "未声明"}</dd></div></dl>
          {result.c2paValidationStatus.length > 0 && <ul className="validation-list">{result.c2paValidationStatus.map((item) => <li key={`${item.code}-${item.explanation}`}><strong>{item.code}</strong><span>{item.explanation}</span></li>)}</ul>}
          {result.manifestJson && <details className="manifest-details" open><summary>原始 C2PA 报告</summary><pre>{result.manifestJson}</pre></details>}
          <RecordList records={result.matches.map((match) => match.record)} evidence={Object.fromEntries(result.matches.map((match) => [match.record.id, match.evidenceSources]))} onNavigate={onNavigateRecord} compact />
        </>}
      </section>
      <section className="record-search">
        <header><Search size={17} /><div><strong>搜索导出记录</strong><span>按 ID、标题、创作者或首次输出路径</span></div></header>
        <div className="record-search-control"><input value={query} maxLength={160} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void searchRecords(); }} /><button className="secondary-button" disabled={busy} onClick={() => void searchRecords()}><Search size={15} />搜索</button></div>
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

function RecordView({ record, preview, onClose, onError }: { record: CertificationRecord; preview: PreviewImage | null; onClose: () => void; onError: (message: string | null) => void }) {
  const [exporting, setExporting] = useState(false);
  const exportAgain = async () => {
    const outputPath = await save({
      defaultPath: fileName(record.outputPath),
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    if (!outputPath) return;
    setExporting(true);
    onError(null);
    try { await authenticityApi.exportRecord(record.id, outputPath); }
    catch (error) { onError(message(error)); }
    finally { setExporting(false); }
  };
  return <div className="auth-workspace record-view-mode">
    <header className="auth-header"><div><span>发布记录 · 只读</span><h1>{record.title}</h1></div><div className="record-view-actions"><button className="secondary-button" type="button" disabled={exporting} onClick={() => void exportAgain()}>{exporting ? <LoaderCircle className="spin" size={15} /> : <ImageDown size={15} />}再次导出</button><button className="secondary-button" type="button" onClick={onClose}><X size={15} />退出查看</button></div></header>
    <div className="publish-layout record-review-layout">
      <section className="auth-preview-panel">
        <header><div><strong>{fileName(record.outputPath)}</strong><span>{preview ? `${preview.width} x ${preview.height} · ` : ""}{formatBytes(record.outputBytes)}</span></div><i><LockKeyhole size={14} />记录已锁定</i></header>
        {preview ? <RegionEditor target="publish" preview={preview} regions={record.additionalRegions} maxRegions={0} onChange={() => undefined} readOnly /> : <div className="record-preview-loading"><LoaderCircle className="spin" size={18} />读取认证图片</div>}
        <div className="artifact-proof"><span>发布节点</span><code>{record.historyId}</code><span>成品 SHA-256</span><code>{record.outputSha256}</code></div>
      </section>
      <section className="publish-controls read-only-controls">
        <div className="auth-form-section"><header><strong>C2PA 内容凭证</strong><span>只读快照</span></header><ReadOnlyField label="作品标题" value={record.title} /><ReadOnlyField label="创作者" value={record.creator || "未声明"} /><ReadOnlyField label="权利声明" value={record.rightsStatement || "未声明"} multiline /><ReadOnlyField label="认证说明" value={record.authenticationContent || "未声明"} multiline /></div>
        <div className="auth-form-section"><header><strong>发布信息</strong><span>{new Date(record.createdMs).toLocaleString()}</span></header><ReadOnlyField label="Artwork / 分支" value={`${record.artworkTitle} / ${record.branchTitle}`} /><ReadOnlyField label="首次导出位置" value={record.outputPath} multiline /><ReadOnlyField label="保存方式" value={record.contentStored ? "仓库内已保存认证 JPG" : "旧记录：使用原导出文件"} /><ReadOnlyField label="验证状态" value={record.validationState || "未记录"} /><ReadOnlyField label="Manifest 标签" value={record.c2paManifestLabel || "未记录"} multiline /></div>
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
      <header><span><Trash2 size={20} /></span><div><small>不可撤销</small><h2 id="delete-publication-title">删除全部发布内容</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div><strong>{branchTitle}</strong><p>将删除该分支的最终成品、全部认证记录、仓库内认证 JPG 副本及记录指向的首次导出文件。完成后分支会解除锁定。</p><div className="delete-detail">这是分支发布内容的总删除操作，不是删除单条导出记录。</div></div>
      <footer><button className="text-button" type="button" onClick={onClose}>保留发布内容</button><button className="danger-button solid" type="button" disabled={busy} onClick={onConfirm}>{busy && <LoaderCircle className="spin" size={15} />}确认全部删除</button></footer>
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
