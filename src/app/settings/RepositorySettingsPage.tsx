import { FolderOpen, Layers, ShieldCheck } from "lucide-react";
import { SettingsOperationProgress } from "../SettingsOperationProgress";
import type { AppSettings, BackupRuntimeStatus, RepositoryStatus } from "../types";
import type { SettingsUpdate } from "./shared";

interface RepositorySettingsPageProps {
  draft: AppSettings;
  repository: RepositoryStatus;
  update: SettingsUpdate;
  settingsBusy: boolean;
  chooseRepository: () => Promise<void>;
  scrubRepository: () => Promise<void>;
  cancelSettingsOperation: () => Promise<void>;
  visibleSettingsRuntime: BackupRuntimeStatus | null;
  cancelPending: boolean;
}

export function RepositorySettingsPage({
  draft,
  repository,
  update,
  settingsBusy,
  chooseRepository,
  scrubRepository,
  cancelSettingsOperation,
  visibleSettingsRuntime,
  cancelPending,
}: RepositorySettingsPageProps) {
  return <>
    <div className="settings-section">
      <div className="settings-section-title"><FolderOpen aria-hidden="true" size={17} /><h3>仓库与数据安全</h3></div>
      <div className="settings-path-field">
        <label htmlFor="repository-path">仓库位置</label>
        <div className="path-control">
          <input id="repository-path" aria-label="作品仓库路径" value={draft.repositoryPath} onChange={(event) => update({ repositoryPath: event.target.value })} placeholder="选择空目录" />
          <button className="secondary-button" type="button" onClick={() => void chooseRepository()}><FolderOpen aria-hidden="true" size={15} />浏览</button>
        </div>
      </div>
      <div className="settings-preference-list settings-repository-actions">
        <div className="settings-preference-row">
          <span className="settings-row-icon"><ShieldCheck aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy"><strong>仓库完整性</strong><small>检查历史链、受控文件摘要、画板 DDS 与 C2PA 声明</small></span>
          <button className="secondary-button" type="button" onClick={() => void scrubRepository()} disabled={settingsBusy || !repository.ready}>
            <ShieldCheck aria-hidden="true" size={15} />开始检查
          </button>
        </div>
      </div>
      {visibleSettingsRuntime && (
        <SettingsOperationProgress
          runtime={visibleSettingsRuntime}
          cancelPending={cancelPending}
          onCancel={() => void cancelSettingsOperation()}
        />
      )}
    </div>

    <div className="settings-section">
      <div className="settings-section-title"><Layers aria-hidden="true" size={17} /><h3>存储格式</h3></div>
      <div className="settings-preference-list">
        <label className="settings-preference-row">
          <span className="settings-row-icon"><Layers aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>ChunkFile v2</strong>
            <small>新节点使用容器对齐分块与快照去重，旧 v1 节点仍可正常读取；关闭后只影响此后新写入的节点。</small>
          </span>
          <input
            className="switch-input"
            type="checkbox"
            aria-label="ChunkFile v2"
            checked={draft.chunkFileFormatV2}
            onChange={(event) => update({ chunkFileFormatV2: event.target.checked })}
          />
        </label>
      </div>
    </div>
  </>;
}
