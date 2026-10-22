import { Fingerprint, LoaderCircle, ScanSearch, Search, ShieldCheck, X } from "lucide-react";
import { useState } from "react";
import type { DragEvent as ReactDragEvent } from "react";
import { fileName } from "../../shared/format";
import type { CertificationRecord } from "./types";
import { RecordList } from "./RecordList";
import { RegionEditor } from "./RegionEditor";
import { useIdentificationController } from "./useIdentificationController";

interface IdentificationViewProps {
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
}

export function IdentificationView({ onError, onNavigateRecord }: IdentificationViewProps) {
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
