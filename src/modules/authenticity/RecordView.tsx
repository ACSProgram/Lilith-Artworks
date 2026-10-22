import {
  ImageDown, LoaderCircle, LockKeyhole, MoreVertical, Trash2, X,
} from "lucide-react";
import { useState } from "react";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { fileName, formatBytes } from "../../shared/format";
import { RegionEditor } from "./RegionEditor";
import type { CertificationRecord, PreviewImage } from "./types";

interface RecordViewProps {
  record: CertificationRecord;
  preview: PreviewImage | null;
  exporting: boolean;
  deleting: boolean;
  onExport: (record: CertificationRecord) => Promise<void>;
  onDelete: (record: CertificationRecord) => Promise<void>;
  onClose: () => void;
}

export function RecordView({ record, preview, exporting, deleting, onExport, onDelete, onClose }: RecordViewProps) {
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
    {deleteConfirmOpen && <ConfirmDialog
      view={{
        danger: true,
        icon: <Trash2 size={20} />,
        eyebrow: "不可撤销",
        title: "删除发布记录",
        subject: record.title,
        description: "将删除这条发布记录及其仓库内认证 JPG 副本、C2PA 清单与 TrustMark ID。",
        detail: "首次导出的 JPG 会保留在原发布路径，不会由此操作删除。",
        action: "确认删除记录",
      }}
      busy={deleting}
      onCancel={() => setDeleteConfirmOpen(false)}
      // 同「取消发布并删除本地数据」：短且不可中断，确认后立即关窗。
      onConfirm={() => { setDeleteConfirmOpen(false); void onDelete(record); }}
    />}
  </div>;
}

function ReadOnlyField({ label, value, multiline = false }: { label: string; value: string; multiline?: boolean }) {
  return <div className={`readonly-field${multiline ? " multiline" : ""}`}><span>{label}</span><strong>{value}</strong></div>;
}
