import {
  AlertCircle, BadgeCheck, Eye, FileImage, FolderOpen, LoaderCircle, LockKeyhole,
  MoreVertical, MousePointer2, RotateCcw, Trash2, X,
} from "lucide-react";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import type { CleanupReport } from "../../shared/fileCleanup";
import { fileName, formatBytes } from "../../shared/format";
import type { AuthenticityBranch, CertificationRecord } from "./types";
import { PublicationPreviewDialog } from "./PublicationPreviewDialog";
import { RecordList } from "./RecordList";
import { RecordView } from "./RecordView";
import { RegionEditor } from "./RegionEditor";
import { usePublicationController } from "./usePublicationController";

export interface PublicationViewProps {
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

interface BranchStateNoticeProps {
  loading: boolean;
  error: string | null;
  onRetry?: () => void;
}

/** 分支列表的三态提示：加载中、加载失败（可重试）与确实没有分支。 */
function BranchStateNotice({ loading, error, onRetry }: BranchStateNoticeProps) {
  if (loading) {
    return <div className="auth-empty"><LoaderCircle className="spin" size={18} />读取分支历史…</div>;
  }
  if (error) {
    return <div className="auth-empty auth-empty-error" role="alert">
      <AlertCircle size={18} /><span>{error}</span>
      {onRetry && <button className="secondary-button" type="button" onClick={onRetry}><RotateCcw size={15} />重试</button>}
    </div>;
  }
  return <div className="auth-empty">此 Artwork 尚无分支。</div>;
}

export function PublicationView({
  artworkTitle,
  branches,
  selectedBranchId,
  branchesLoading = false,
  branchesError = null,
  onRetryBranches,
  selectedRecordId,
  recordNavigationKey,
  onSelectBranch,
  onError,
  onNavigateRecord,
  onRetryFileCleanup,
  onPublicationChanged,
}: PublicationViewProps) {
  const {
    publication, config, setConfig, preview, artifactPreviewBusy, artifactPreviewError,
    outputPreview, outputPreviewOpen, setOutputPreviewOpen, outputPreviewBusy,
    privateKey, setPrivateKey, publishing, cancelling, busy, result, publishMetrics, sizeEstimate,
    viewingRecord, setViewingRecord, viewingPreview, exporting,
    deleteConfirmOpen, setDeleteConfirmOpen, cleanupFailures, selectedBranch,
    enterPublication, retryArtifactPreview, chooseCertificate, generateOutputPreview,
    cancelAuthenticityOperation, publish, cancelPublication, retryCleanup,
    openRecord, exportRecord, deletingRecord, deleteRecord, previewLoupe, previewOutputTile,
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
    {!selectedBranch ? <BranchStateNotice loading={branchesLoading} error={branchesError} onRetry={onRetryBranches} /> :
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
            loupe={{ sourceKey: publication.branchId, request: previewLoupe }}
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
          {result && <div className="publish-success"><BadgeCheck size={18} /><div><strong>认证发布完成</strong><span>{result.outputPath}</span><code>{result.watermarkId}</code>{publishMetrics && <small>{publishMetrics.renditionCacheHit ? "已复用质量预览编码" : `重新渲染 ${publishMetrics.renderMs} ms · 编码 ${publishMetrics.encodeMs} ms`} · C2PA/时间戳 {publishMetrics.signingMs} ms</small>}{publishMetrics && publishMetrics.watermarkWarnings.length > 0 && <span className="publish-success-note"><AlertCircle size={13} aria-hidden="true" />{publishMetrics.watermarkWarnings.map((warning) => `区域 ${warning.regionIndex + 1}（${warning.width}×${warning.height} px）`).join("、")} 小于稳定边长 {publishMetrics.watermarkWarnings[0].threshold} px，水印可能无法回读。</span>}</div></div>}
          {outputPreviewBusy ? <div className="output-preview-progress" role="status" aria-live="polite">
            <div><LoaderCircle className="spin" aria-hidden="true" size={18} /><span><strong>{cancelling ? "正在取消质量预览" : "正在生成质量预览"}</strong><small>{cancelling ? "等待当前处理阶段安全结束" : "正在渲染并编码正式 JPG 预览"}</small></span></div>
            <button className="secondary-button" type="button" disabled={cancelling} onClick={() => void cancelAuthenticityOperation()}><X aria-hidden="true" size={16} />取消</button>
          </div> : <button className="primary-button publish-command" type="button" disabled={busy} onClick={() => void generateOutputPreview()}><Eye aria-hidden="true" size={17} />生成质量预览</button>}
        </section>
        <RecordList records={publication.records} onNavigate={openRecord} selectedId={selectedRecordId} />
        {outputPreviewOpen && outputPreview && <PublicationPreviewDialog preview={outputPreview} busy={publishing} cancelling={cancelling} onRequestTile={previewOutputTile} onBack={() => setOutputPreviewOpen(false)} onCancel={() => void cancelAuthenticityOperation()} onPublish={() => void publish()} />}
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
    {selectedBranch && deleteConfirmOpen && <ConfirmDialog
      view={{
        danger: true,
        emphasis: true,
        slideToConfirm: true,
        icon: <Trash2 size={20} />,
        eyebrow: "不可撤销",
        title: "删除本地发布数据",
        subject: selectedBranch.title,
        description: "将删除该分支的仓库内最终成品、全部认证记录、认证 JPG 副本和保存配置，并解除分支锁定。",
        detail: "首次导出的 JPG 会保留在原发布路径，不会由此操作删除。",
        action: "拖动到底删除本地数据",
      }}
      busy={busy}
      onCancel={() => setDeleteConfirmOpen(false)}
      // 解除发布与仓库内文件删除是一步到位的短操作，底层没有可中断点：确认后立即
      // 关窗，避免弹窗停在运行态给出一个点了不生效的取消入口。
      onConfirm={() => { setDeleteConfirmOpen(false); void cancelPublication(); }}
    />}
  </div>;
}
