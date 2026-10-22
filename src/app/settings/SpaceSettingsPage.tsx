import { RotateCcw, SearchCheck, Trash2 } from "lucide-react";
import { RepositoryStoragePanel } from "../RepositoryStoragePanel";
import type { BackupRuntimeStatus, RepositoryStatus } from "../types";
import type { RepositoryMaintenance } from "../useRepositoryMaintenance";
import { formatBytes } from "../../shared/format";
import {
  cleanupReasonLabel,
  isExternalCleanupEntry,
} from "../../shared/fileCleanup";

interface SpaceSettingsPageProps {
  repository: RepositoryStatus;
  maintenance: RepositoryMaintenance;
  settingsBusy: boolean;
  visibleSettingsRuntime: BackupRuntimeStatus | null;
  cancelPending: boolean;
}

export function SpaceSettingsPage({
  repository,
  maintenance,
  settingsBusy,
  visibleSettingsRuntime,
  cancelPending,
}: SpaceSettingsPageProps) {
  const {
    pendingCleanup,
    pendingCleanupSummary,
    repositoryCleanupCount,
    setExternalCleanupTarget,
    scanCandidates,
    setScanCandidates,
    setScanConfirmOpen,
    storageOverview,
    storageDiskScan,
    storageOverviewBusy,
    storageOverviewError,
    refreshStorageOverview,
    scanStorageDisk,
    settingsOperation,
    cancelSettingsOperation,
  } = maintenance;

  return <>
    <RepositoryStoragePanel
      ready={repository.ready}
      overview={storageOverview}
      scan={storageDiskScan}
      loading={storageOverviewBusy}
      error={storageOverviewError}
      scanning={settingsOperation === "storage-scan"}
      cancelPending={cancelPending}
      runtime={visibleSettingsRuntime}
      onRefresh={() => void refreshStorageOverview()}
      onScan={() => void scanStorageDisk()}
      onCancelScan={() => void cancelSettingsOperation()}
    />

    <div className="settings-section">
      <div className="settings-section-title"><Trash2 aria-hidden="true" size={17} /><h3>文件清理</h3></div>
      <div className="settings-preference-list settings-repository-actions">
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Trash2 aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>待清理队列</strong>
            <small>{pendingCleanupSummary}</small>
          </span>
          <button
            className="secondary-button"
            type="button"
            onClick={() => void maintenance.retryCleanup([])}
            disabled={settingsBusy || !repository.ready || repositoryCleanupCount === 0}
            title="只重放仓库内条目；仓库外文件需逐条确认"
          >
            <RotateCcw aria-hidden="true" size={15} />全部重试
          </button>
        </div>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><SearchCheck aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>垃圾文件扫描</strong>
            <small>扫描仓库里可回收的冗余文件；只报告不删除，确认后才清理。</small>
          </span>
          <button
            className="secondary-button"
            type="button"
            onClick={() => void maintenance.scanUnreferenced()}
            disabled={settingsBusy || !repository.ready}
          >
            <SearchCheck aria-hidden="true" size={15} />开始扫描
          </button>
        </div>
      </div>

      {pendingCleanup.length > 0 && (
        <div className="cleanup-queue" role="list" aria-label="待清理文件队列">
          {pendingCleanup.map((entry) => {
            // 仓库外文件不参与自动重放：删除它等于删除仓库之外的任意
            // 路径，必须逐条确认，且确认前要看到完整路径、所属作品与
            // 期望摘要。
            const external = isExternalCleanupEntry(entry);
            return (
              <div className="cleanup-queue-row" role="listitem" key={entry.id}>
                <div className="cleanup-queue-copy">
                  <strong title={entry.path}>{entry.path}</strong>
                  <small>
                    {external ? "仓库外文件 · " : ""}
                    {cleanupReasonLabel(entry.reason)}
                    {external && ` · 所属作品：${entry.artworkTitle ?? "未知"}`}
                    {entry.lastError ? ` · 上次失败：${entry.lastError}` : external ? "" : " · 等待清理"}
                    {entry.lastAttemptMs ? ` · ${new Date(entry.lastAttemptMs).toLocaleString()}` : ""}
                  </small>
                  {external && (
                    <small title={entry.expectedSha256 ?? undefined}>
                      期望 SHA-256：{entry.expectedSha256 ?? "未记录"}
                    </small>
                  )}
                </div>
                {external ? (
                  <button
                    className="danger-button"
                    type="button"
                    disabled={settingsBusy}
                    onClick={() => setExternalCleanupTarget(entry)}
                  >
                    <Trash2 aria-hidden="true" size={15} />确认删除…
                  </button>
                ) : (
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={settingsBusy}
                    onClick={() => void maintenance.retryCleanup([entry.id])}
                  >
                    <RotateCcw aria-hidden="true" size={15} />重试
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}

      {scanCandidates && (
        <div className="cleanup-scan-result" role="status">
          <header>
            <strong>{scanCandidates.length > 0 ? `发现 ${scanCandidates.length} 个可清理文件` : "没有发现可清理的文件"}</strong>
            <div>
              <button className="text-button" type="button" onClick={() => setScanCandidates(null)}>关闭</button>
              {scanCandidates.length > 0 && (
                <button className="danger-button" type="button" disabled={settingsBusy} onClick={() => setScanConfirmOpen(true)}>
                  <Trash2 aria-hidden="true" size={15} />清理这些文件
                </button>
              )}
            </div>
          </header>
          {scanCandidates.length > 0 && (
            <ul className="cleanup-candidate-list">
              {scanCandidates.map((candidate) => (
                <li key={candidate.path}>
                  <span title={candidate.path}>{candidate.path}</span>
                  <small>{candidate.reason} · {formatBytes(candidate.byteSize)}</small>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  </>;
}
