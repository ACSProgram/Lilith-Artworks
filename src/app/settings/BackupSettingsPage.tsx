import {
  Activity, Clock3, DatabaseBackup, SearchCheck, ShieldCheck,
} from "lucide-react";
import { SettingsOperationProgress } from "../SettingsOperationProgress";
import { formatBytes } from "../../shared/format";
import type {
  AppSettings,
  BackupRuntimeStatus,
  RepositoryStatus,
  SettingsSnapshot,
} from "../types";
import type { RepositoryBackupResult } from "../useRepositoryMaintenance";
import type { SettingsUpdate } from "./shared";

interface BackupSettingsPageProps {
  draft: AppSettings;
  snapshot: SettingsSnapshot | null;
  repository: RepositoryStatus;
  update: SettingsUpdate;
  settingsBusy: boolean;
  backupRepository: () => Promise<void>;
  cancelSettingsOperation: () => Promise<void>;
  visibleSettingsRuntime: BackupRuntimeStatus | null;
  cancelPending: boolean;
  lastRepositoryBackup: RepositoryBackupResult | null;
}

export function BackupSettingsPage({
  draft,
  snapshot,
  repository,
  update,
  settingsBusy,
  backupRepository,
  cancelSettingsOperation,
  visibleSettingsRuntime,
  cancelPending,
  lastRepositoryBackup,
}: BackupSettingsPageProps) {
  return <>
    <div className="settings-section">
      <div className="settings-section-title"><Clock3 aria-hidden="true" size={17} /><h3>自动备份</h3></div>
      <div className="settings-preference-list">
        <label className="settings-preference-row">
          <span className="settings-row-icon"><Clock3 aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy"><strong>自动备份调度</strong><small>{snapshot?.automaticBackupFileCount == null ? "仓库不可用" : `${snapshot.automaticBackupFileCount} 个工作文件已启用 · ${draft.pauseAutomaticBackups ? "已暂停" : "正在运行"}`}</small></span>
          <input className="switch-input" type="checkbox" checked={!draft.pauseAutomaticBackups} onChange={(event) => update({ pauseAutomaticBackups: !event.target.checked })} />
        </label>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><SearchCheck aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy"><strong>默认检查方式</strong><small>{draft.automaticBackupCheckMode === "quick" ? "快速：比较大小、修改时间与首尾采样，有变化再全量备份" : "全量：每次都完整校验内容"}</small></span>
          <select
            aria-label="自动备份默认检查方式"
            value={draft.automaticBackupCheckMode}
            onChange={(event) => update({ automaticBackupCheckMode: event.target.value as AppSettings["automaticBackupCheckMode"] })}
          >
            <option value="quick">快速检查（推荐）</option>
            <option value="full">全量校验</option>
          </select>
        </div>
      </div>
    </div>

    <div className="settings-section">
      <div className="settings-section-title"><ShieldCheck aria-hidden="true" size={17} /><h3>完整性检查</h3></div>
      <div className="settings-preference-list">
        <label className="settings-preference-row">
          <span className="settings-row-icon"><ShieldCheck aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy">
            <strong>空闲完整性检查</strong>
            <small>{draft.idleIntegrityCheckEnabled ? "已开启" : "已关闭"}
              {" · "}
              {snapshot?.idleIntegrityPendingCount == null
                ? "仓库不可用"
                : `${snapshot.idleIntegrityPendingCount} 个历史节点待校验`}
              {!draft.idleIntegrityCheckEnabled && " · 仅在主窗口隐藏或最小化时运行"}
            </small>
          </span>
          <input
            className="switch-input"
            type="checkbox"
            checked={draft.idleIntegrityCheckEnabled}
            onChange={(event) => update({ idleIntegrityCheckEnabled: event.target.checked })}
          />
        </label>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Activity aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>运行方式</strong>
            <small>
              空闲时自动检查历史版本是否完好，一次只查一批，出问题的版本会标在历史页对应节点上。
              托盘“暂停所有自动备份”时一并暂停；手动“重新检查此分支”和“仓库完整性”扫描不受此开关影响。
            </small>
          </span>
        </div>
      </div>
    </div>

    <div className="settings-section">
      <div className="settings-section-title"><DatabaseBackup aria-hidden="true" size={17} /><h3>整仓备份</h3></div>
      <div className="settings-preference-list settings-repository-actions">
        <div className="settings-preference-row">
          <span className="settings-row-icon"><DatabaseBackup aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>创建备份</strong>
            <small>复制数据库与全部仓库文件并校验副本；恢复时选择副本中的 repository 目录。</small>
          </span>
          <button className="secondary-button" type="button" onClick={() => void backupRepository()} disabled={settingsBusy || !repository.ready}>
            <DatabaseBackup aria-hidden="true" size={15} />创建备份
          </button>
        </div>
      </div>
      {lastRepositoryBackup ? (
        <div className="settings-backup-result" role="status">
          <strong>最近一次结果</strong>
          <small>
            {new Date(lastRepositoryBackup.completedAtMs).toLocaleString()} ·
            {" "}{lastRepositoryBackup.report.fileCount} 个文件 ·
            {" "}{formatBytes(lastRepositoryBackup.report.totalBytes)}
          </small>
          <small>
            历史节点 {lastRepositoryBackup.report.historyNodes} ·
            成品 {lastRepositoryBackup.report.finalArtifacts} ·
            认证记录 {lastRepositoryBackup.report.certificationRecords}
          </small>
          <small title={lastRepositoryBackup.report.repositoryPath}>
            恢复副本：{lastRepositoryBackup.report.repositoryPath}
          </small>
        </div>
      ) : (
        <p className="settings-backup-empty">
          尚无备份记录
        </p>
      )}
      {visibleSettingsRuntime && (
        <SettingsOperationProgress
          runtime={visibleSettingsRuntime}
          cancelPending={cancelPending}
          onCancel={() => void cancelSettingsOperation()}
        />
      )}
    </div>
  </>;
}
