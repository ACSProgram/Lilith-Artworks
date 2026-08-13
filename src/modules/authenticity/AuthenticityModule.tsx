import { open, save } from "@tauri-apps/plugin-dialog";
import {
  BadgeCheck, FileImage, Fingerprint, FolderOpen, ImageDown, LoaderCircle,
  LockKeyhole, ScanSearch, Search, ShieldCheck, Trash2, X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { formatBytes } from "../../shared/format";
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

export function AuthenticityModule(props: AuthenticityModuleProps) {
  return props.mode === "publish"
    ? <PublishView {...props} />
    : <IdentifyView onError={props.onError} onNavigateRecord={props.onNavigateRecord} />;
}

function PublishView({
  artworkTitle, branches, selectedBranchId, selectedRecordId, onSelectBranch, onError, onNavigateRecord,
}: AuthenticityModuleProps) {
  const [publication, setPublication] = useState<BranchPublication | null>(null);
  const [config, setConfig] = useState<CertificationConfig | null>(null);
  const [preview, setPreview] = useState<PreviewImage | null>(null);
  const [privateKey, setPrivateKey] = useState("");
  const [watermarkId, setWatermarkId] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CertificationRecord | null>(null);
  const selectedBranch = branches.find((branch) => branch.id === selectedBranchId) ?? null;

  const load = useCallback(async () => {
    if (!selectedBranchId) return;
    setBusy(true);
    try {
      const next = await authenticityApi.getPublication(selectedBranchId);
      setPublication(next);
      setConfig({ ...next.config, trustmarkEnabled: next.modelsReady && next.config.trustmarkEnabled });
      if (next.artifact) setPreview(await authenticityApi.preview(next.artifact.sourcePath));
      else setPreview(null);
    } catch (error) {
      onError(message(error));
    } finally {
      setBusy(false);
    }
  }, [onError, selectedBranchId]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (!selectedRecordId || !publication) return;
    window.requestAnimationFrame(() => document.querySelector(`[data-record-id="${selectedRecordId}"]`)?.scrollIntoView({ block: "center" }));
  }, [publication, selectedRecordId]);

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
      setPublication(next);
      setConfig({ ...next.config, trustmarkEnabled: next.modelsReady && next.config.trustmarkEnabled });
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
      setResult(published.record);
      setPrivateKey("");
      await load();
    } catch (error) {
      onError(message(error));
    } finally {
      setBusy(false);
    }
  };

  return <div className="auth-workspace">
    <header className="auth-header">
      <div><span>发布与认证</span><h1>{artworkTitle}</h1></div>
      <select value={selectedBranchId ?? ""} onChange={(event) => onSelectBranch(event.target.value)}>
        {branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}
      </select>
    </header>
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
            regions={config.trustmarkEnabled ? config.additionalRegions : []}
            maxRegions={8}
            onChange={(regions) => setConfig({ ...config, additionalRegions: regions })}
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
            <label>JPEG 质量<input type="number" min={1} max={100} value={config.jpegQuality} onChange={(event) => setConfig({ ...config, jpegQuality: Number(event.target.value) })} /></label>
            <label className="wide-field">证书链<div className="auth-path-control"><input readOnly value={config.certificatePath} placeholder="选择 PEM 证书链" /><button className="icon-button" type="button" title="选择证书" onClick={() => void chooseCertificate()}><FolderOpen size={16} /></button></div></label>
            <label className="wide-field">PEM 私钥<textarea className="secret-field" value={privateKey} rows={3} autoComplete="new-password" onChange={(event) => setPrivateKey(event.target.value)} /></label>
            <label className="wide-field">时间戳服务<input value={config.timestampUrl ?? ""} placeholder="可选 RFC 3161 URL" onChange={(event) => setConfig({ ...config, timestampUrl: event.target.value || null })} /></label>
          </div>
          <div className="auth-form-section trustmark-section">
            <header><strong>TrustMark Q</strong><label className="switch-field"><span className="switch-copy"><strong>{config.trustmarkEnabled ? "已启用" : "不嵌入"}</strong></span><input className="switch-input" type="checkbox" checked={config.trustmarkEnabled} disabled={!publication.modelsReady} onChange={(event) => setConfig({ ...config, trustmarkEnabled: event.target.checked })} /></label></header>
            {!publication.modelsReady && <p className="auth-warning">TrustMark 模型不可用，仍可发布 C2PA 凭证。</p>}
            {config.trustmarkEnabled && <>
              <label>自定义 ID<input value={watermarkId} maxLength={61} placeholder="留空自动生成 61 位 ID" onChange={(event) => setWatermarkId(event.target.value.replace(/[^01]/g, ""))} /></label>
              <div className="two-fields"><label>强度<input type="number" min={0.5} max={1.5} step={0.05} value={config.watermarkStrength} onChange={(event) => setConfig({ ...config, watermarkStrength: Number(event.target.value) })} /></label><label>透明背景<input type="color" value={config.backgroundColor} onChange={(event) => setConfig({ ...config, backgroundColor: event.target.value })} /></label></div>
              <p>{config.additionalRegions.length ? `全图 + ${config.additionalRegions.length} 个额外区域` : "全图基础水印；可在左侧拖拽添加局部范围。"}</p>
            </>}
          </div>
          {result && <div className="publish-success"><BadgeCheck size={18} /><div><strong>认证发布完成</strong><span>{result.outputPath}</span><code>{result.watermarkId}</code></div></div>}
          <button className="primary-button publish-command" type="button" disabled={busy} onClick={() => void publish()}>{busy ? <LoaderCircle className="spin" size={17} /> : <ImageDown size={17} />}签名并导出 JPG</button>
        </section>
        <RecordList records={publication.records} onNavigate={onNavigateRecord} selectedId={selectedRecordId} />
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

  const choose = async () => {
    const selected = await open({ multiple: false, filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }] });
    if (typeof selected !== "string") return;
    setBusy(true);
    try {
      setPath(selected);
      setPreview(await authenticityApi.preview(selected));
      setRegion(null);
      setResult(null);
    } catch (error) { onError(message(error)); } finally { setBusy(false); }
  };

  const decode = async () => {
    if (!path) return;
    setBusy(true);
    onError(null);
    try { setResult(await authenticityApi.decode(path, region)); }
    catch (error) { onError(message(error)); }
    finally { setBusy(false); }
  };

  const searchRecords = async () => {
    setBusy(true);
    try { setRecords(await authenticityApi.searchRecords(query)); }
    catch (error) { onError(message(error)); }
    finally { setBusy(false); }
  };

  return <div className="auth-workspace identify-workspace">
    <header className="auth-header"><div><span>识别与溯源</span><h1>验证发布图片</h1></div></header>
    <div className="identify-layout">
      <section className="auth-preview-panel identify-preview">
        {!preview ? <button className="image-empty" type="button" onClick={() => void choose()}><ScanSearch size={28} /><strong>选择待识别图片</strong><span>C2PA 会始终读取；TrustMark 可识别整图或框选区域。</span></button> : <>
          <header><div><strong>{fileName(path)}</strong><span>{preview.width} x {preview.height}</span></div><button className="text-button" type="button" onClick={() => void choose()}>更换图片</button></header>
          <RegionEditor target="decode" preview={preview} regions={region ? [region] : []} maxRegions={1} onChange={(regions) => setRegion(regions[0] ?? null)} />
          <div className="decode-scope"><Fingerprint size={17} /><span>{region ? "识别框选区域" : "识别整张图片"}</span>{region && <button className="icon-button" title="改用整图" onClick={() => setRegion(null)}><X size={15} /></button>}</div>
          <button className="primary-button" type="button" disabled={busy} onClick={() => void decode()}>{busy ? <LoaderCircle className="spin" size={16} /> : <ScanSearch size={16} />}开始识别</button>
        </>}
      </section>
      <section className="decode-results">
        {!result ? <div className="decode-placeholder"><Fingerprint size={24} /><span>识别结果将在这里显示</span></div> : <>
          <header className={result.c2paPresent ? "verified" : ""}><ShieldCheck size={20} /><div><strong>{result.c2paPresent ? "已读取 C2PA" : "未发现 C2PA"}</strong><span>{result.c2paValidationState ?? "无验证状态"}</span></div></header>
          <dl><div><dt>C2PA ID</dt><dd><code>{result.c2paWatermarkId ?? "未声明"}</code></dd></div><div><dt>TrustMark ID</dt><dd><code>{result.watermarkId ?? "未识别"}</code></dd></div><div><dt>双通道</dt><dd>{result.identifiersMatch == null ? "只有单通道证据" : result.identifiersMatch ? "ID 一致" : "ID 冲突，需人工调查"}</dd></div></dl>
          {result.c2paValidationStatus.length > 0 && <ul className="validation-list">{result.c2paValidationStatus.map((item) => <li key={`${item.code}-${item.explanation}`}><strong>{item.code}</strong><span>{item.explanation}</span></li>)}</ul>}
          <RecordList records={result.matches} onNavigate={onNavigateRecord} compact />
        </>}
      </section>
      <section className="record-search">
        <header><Search size={17} /><div><strong>搜索导出记录</strong><span>按 ID、标题、创作者或输出路径</span></div></header>
        <div className="record-search-control"><input value={query} maxLength={160} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void searchRecords(); }} /><button className="secondary-button" disabled={!query.trim() || busy} onClick={() => void searchRecords()}><Search size={15} />搜索</button></div>
        <RecordList records={records} onNavigate={onNavigateRecord} compact />
      </section>
    </div>
  </div>;
}

function RegionEditor({ target, preview, regions, maxRegions, onChange }: {
  target: ImageTarget;
  preview: PreviewImage;
  regions: NormalizedRegion[];
  maxRegions: number;
  onChange: (regions: NormalizedRegion[]) => void;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<NormalizedRegion | null>(null);
  const drag = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const imageRect = useCallback(() => {
    const stage = stageRef.current;
    if (!stage) return null;
    const scale = Math.min(stage.clientWidth / preview.width, stage.clientHeight / preview.height);
    const width = preview.width * scale;
    const height = preview.height * scale;
    return { left: (stage.clientWidth - width) / 2, top: (stage.clientHeight - height) / 2, width, height };
  }, [preview.height, preview.width]);
  const point = (event: ReactPointerEvent) => {
    const stage = stageRef.current;
    const image = imageRect();
    if (!stage || !image) return null;
    const bounds = stage.getBoundingClientRect();
    const x = (event.clientX - bounds.left - image.left) / image.width;
    const y = (event.clientY - bounds.top - image.top) / image.height;
    if (event.type === "pointerdown" && (x < 0 || x > 1 || y < 0 || y > 1)) return null;
    return { x: Math.max(0, Math.min(1, x)), y: Math.max(0, Math.min(1, y)) };
  };
  const style = (region: NormalizedRegion) => {
    const image = imageRect();
    if (!image) return undefined;
    return { left: image.left + region.x * image.width, top: image.top + region.y * image.height, width: region.width * image.width, height: region.height * image.height };
  };
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || regions.length >= maxRegions) return;
    const start = point(event);
    if (!start) return;
    drag.current = { pointerId: event.pointerId, ...start };
    setDraft({ x: start.x, y: start.y, width: 0, height: 0 });
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    const next = point(event);
    if (!next) return;
    setDraft({ x: Math.min(drag.current.x, next.x), y: Math.min(drag.current.y, next.y), width: Math.abs(next.x - drag.current.x), height: Math.abs(next.y - drag.current.y) });
  };
  const finish = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    drag.current = null;
    if (draft && draft.width * preview.width >= (target === "publish" ? 96 : 64) && draft.height * preview.height >= (target === "publish" ? 96 : 64)) onChange(target === "decode" ? [draft] : [...regions, draft]);
    setDraft(null);
  };
  return <div className="region-stage" ref={stageRef}>
    <img src={preview.dataUrl} alt={target === "publish" ? "最终成品预览" : "待识别图片预览"} draggable={false} />
    <div className="region-layer" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={finish} onPointerCancel={finish}>
      {regions.map((region, index) => <div className="region-box" style={style(region)} key={`${region.x}-${region.y}-${index}`}><span>{target === "publish" ? `区域 ${index + 1}` : "识别区域"}</span>{target === "publish" && <button type="button" title="移除区域" onPointerDown={(event) => event.stopPropagation()} onClick={() => onChange(regions.filter((_, item) => item !== index))}><X size={12} /></button>}</div>)}
      {draft && <div className="region-box draft" style={style(draft)}><span>框选中</span></div>}
    </div>
    {target === "publish" && regions.length > 0 && <button className="clear-regions" type="button" onClick={() => onChange([])}><Trash2 size={13} />清除区域</button>}
  </div>;
}

function RecordList({ records, onNavigate, compact = false, selectedId = null }: { records: CertificationRecord[]; onNavigate: (record: CertificationRecord) => void; compact?: boolean; selectedId?: string | null }) {
  return <section className={`record-list${compact ? " compact" : ""}`}>
    {!compact && <header><strong>分支导出记录</strong><span>{records.length} 条</span></header>}
    {records.length === 0 ? <div className="records-empty">没有匹配的导出记录。</div> : records.map((record) => <button type="button" data-record-id={record.id} className={`record-row${selectedId === record.id ? " selected" : ""}`} key={record.id} onClick={() => onNavigate(record)}>
      <BadgeCheck size={16} />
      <span><strong>{record.title}</strong><small>{record.artworkTitle} / {record.branchTitle} · {new Date(record.createdMs).toLocaleString()}</small><code>{record.watermarkId ?? "无 TrustMark"}</code></span>
      <i>{record.trustmarkEnabled ? "C2PA + TrustMark" : "C2PA"}</i>
    </button>)}
  </section>;
}
