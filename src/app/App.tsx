import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Clock3,
  DatabaseBackup,
  FolderOpen,
  Info,
  LoaderCircle,
  MonitorCog,
  Palette,
  PanelLeftClose,
  Settings,
  ShieldCheck,
  X,
} from "lucide-react";
import { LibraryModule } from "../modules/library/LibraryModule";
import { ArtworkWorkspace } from "./ArtworkWorkspace";
import { appApi } from "./api";
import type {
  AppSettings,
  BackupRuntimeStatus,
  RepositoryStatus,
  SettingsSnapshot,
} from "./types";
import { WindowTitleBar } from "./WindowTitleBar";
import packageInfo from "../../package.json";

const EMPTY_STATUS: RepositoryStatus = {
  configured: false,
  ready: false,
  rootPath: "",
  databasePath: "",
  error: null,
};

const IDLE_BACKUP_RUNTIME: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: false,
  completionRevision: 0,
};

type SettingsOperation = "repository-scrub" | "repository-backup";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function App() {
  const [snapshot, setSnapshot] = useState<SettingsSnapshot | null>(null);
  const [draft, setDraft] = useState<AppSettings | null>(null);
  const [repository, setRepository] = useState(EMPTY_STATUS);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [busy, setBusy] = useState(true);
  const [message, setMessage] = useState<string | null>(null);
  const [settingsOperation, setSettingsOperation] = useState<SettingsOperation | null>(null);
  const [backupRuntime, setBackupRuntime] = useState(IDLE_BACKUP_RUNTIME);
  const [cancelPending, setCancelPending] = useState(false);

  const load = async () => {
    setBusy(true);
    try {
      const [nextSnapshot, nextRepository] = await Promise.all([
        appApi.getSettings(),
        appApi.getRepositoryStatus(),
      ]);
      const cleanupReport = nextRepository.ready
        ? await appApi.retryFileCleanup([])
        : null;
      setSnapshot(nextSnapshot);
      setDraft(nextSnapshot.settings);
      setRepository(nextRepository);
      setMessage(
        nextSnapshot.warning
        ?? nextRepository.error
        ?? (cleanupReport && cleanupReport.failures.length > 0
          ? `有 ${cleanupReport.failures.length} 个历史遗留文件仍无法清理，将在下次启动时重试。`
          : null),
      );
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    if (!message) return;
    const timer = window.setTimeout(() => setMessage(null), 7000);
    return () => window.clearTimeout(timer);
  }, [message]);

  useEffect(() => {
    const theme = draft?.theme ?? "system";
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.density = draft?.content.density ?? "comfortable";
  }, [draft?.content.density, draft?.theme]);

  useEffect(() => {
    if (!settingsOpen && !settingsOperation) return;
    let disposed = false;
    const poll = async () => {
      try {
        const next = await appApi.getBackupRuntimeStatus();
        if (!disposed) setBackupRuntime(next);
      } catch {
        // Runtime polling is best-effort; the command result still reports failures.
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 350);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [settingsOpen, settingsOperation]);

  const repositoryLabel = useMemo(() => {
    if (repository.ready) return "仓库就绪";
    if (repository.configured) return "仓库不可用";
    return "尚未配置仓库";
  }, [repository]);

  const chooseRepository = async () => {
    if (!draft) return;
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      setDraft({ ...draft, repositoryPath: selected });
    }
  };

  const openSettings = () => {
    setSettingsOpen(true);
    void appApi.getSettings().then((next) => {
      setSnapshot(next);
      setDraft(next.settings);
    }).catch((error) => setMessage(errorMessage(error)));
  };

  const save = async () => {
    if (!draft) return;
    const repositoryChanged = draft.repositoryPath.trim() !== repository.rootPath;
    setBusy(true);
    setMessage(null);
    if (repositoryChanged) setRepository(EMPTY_STATUS);
    try {
      const next = await appApi.saveSettings(draft);
      setSnapshot(next);
      setDraft(next.settings);
      setRepository(await appApi.getRepositoryStatus());
      setSettingsOpen(false);
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const scrubRepository = async () => {
    let runtime: BackupRuntimeStatus;
    try {
      runtime = await appApi.getBackupRuntimeStatus();
    } catch (error) {
      setMessage(errorMessage(error));
      return;
    }
    setBackupRuntime(runtime);
    if (runtime.busy) {
      setMessage("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("repository-scrub");
    setMessage(null);
    try {
      const report = await appApi.scrubRepositoryIntegrity();
      setMessage(
        `完整性检查通过：${report.historyNodes} 个历史节点、${report.finalArtifacts} 个最终成品、${report.certificationRecords} 条认证记录。`,
      );
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setSettingsOperation(null);
      setCancelPending(false);
    }
  };

  const backupRepository = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择备份保存位置",
    });
    if (typeof selected !== "string") return;
    let runtime: BackupRuntimeStatus;
    try {
      runtime = await appApi.getBackupRuntimeStatus();
    } catch (error) {
      setMessage(errorMessage(error));
      return;
    }
    setBackupRuntime(runtime);
    if (runtime.busy) {
      setMessage("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("repository-backup");
    setMessage(null);
    try {
      const report = await appApi.createRepositoryBackup(selected);
      setMessage(
        `备份已校验：${report.fileCount} 个文件、${report.historyNodes} 个历史节点。恢复时选择 ${report.repositoryPath}`,
      );
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setSettingsOperation(null);
      setCancelPending(false);
    }
  };

  const cancelSettingsOperation = async () => {
    setCancelPending(true);
    try {
      const requested = await appApi.cancelBackupOperation();
      if (!requested) setMessage("操作尚未进入可取消阶段，请稍后重试。");
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setCancelPending(false);
    }
  };

  const runtimeMatchesSettings = backupRuntime.busy
    && (backupRuntime.operation === "repository-scrub"
      || backupRuntime.operation === "repository-backup");
  const visibleSettingsRuntime = runtimeMatchesSettings
    ? backupRuntime
    : settingsOperation
      ? {
        ...IDLE_BACKUP_RUNTIME,
        busy: true,
        operation: settingsOperation,
        progressLabel: settingsOperation === "repository-backup"
          ? "正在准备创建备份"
          : "正在准备完整性检查",
      }
      : null;
  const settingsBusy = busy || settingsOperation !== null || backupRuntime.busy;

  return (
    <main className="app-shell">
      <WindowTitleBar
        repositoryLabel={repositoryLabel}
        repositoryReady={repository.ready}
        onOpenSettings={openSettings}
        onError={setMessage}
      />

      {busy && !snapshot ? (
        <section className="workspace full-workspace">
          <div className="loading-state"><LoaderCircle className="spin" aria-hidden="true" />正在读取设置</div>
        </section>
      ) : (
        <LibraryModule
          key={repository.ready ? repository.rootPath : "repository-unavailable"}
          repositoryReady={repository.ready}
          onConfigure={openSettings}
          onError={setMessage}
          onRetryFileCleanup={appApi.retryFileCleanup}
          onAcknowledgeBackupDisableNotices={appApi.acknowledgeBackupDisableNotices}
          onOpenBackupDisableNotice={appApi.getBackupDisableNoticeTarget}
          renderArtworkWorkspace={(workspace) => (
            <ArtworkWorkspace
              key={workspace.artworkId}
              artworkId={workspace.artworkId}
              initialView={workspace.initialView}
              initialBranchId={workspace.initialBranchId}
              initialRecordId={workspace.initialRecordId}
              navigationKey={workspace.navigationKey}
              onError={setMessage}
              onRetryFileCleanup={appApi.retryFileCleanup}
              onNavigateRecord={(record) => workspace.onNavigateRecord({
                artworkId: record.artworkId,
                branchId: record.branchId,
                recordId: record.id,
              })}
            />
          )}
        />
      )}
      {message && <div className="notice" role="alert">
        <AlertCircle aria-hidden="true" size={20} />
        <div><strong>操作提示</strong><span>{message}</span></div>
        <button type="button" title="关闭提示" onClick={() => setMessage(null)}><X aria-hidden="true" size={16} /></button>
        <i aria-hidden="true" />
      </div>}

      {settingsOpen && draft && (
        <div className="dialog-backdrop" role="presentation" onMouseDown={() => setSettingsOpen(false)}>
          <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title" onMouseDown={(event) => event.stopPropagation()}>
            <header>
              <div className="settings-heading">
                <span className="settings-heading-icon"><Settings aria-hidden="true" size={18} /></span>
                <div>
                  <h2 id="settings-title">设置</h2>
                  <small title={snapshot?.settingsPath}>应用与仓库</small>
                </div>
              </div>
              <button className="icon-button" type="button" title="关闭设置" onClick={() => setSettingsOpen(false)}><X aria-hidden="true" size={18} /></button>
            </header>

            <div className="settings-section">
              <div className="settings-section-title"><FolderOpen aria-hidden="true" size={17} /><h3>仓库与数据安全</h3></div>
              <div className="path-control">
                <input aria-label="作品仓库路径" value={draft.repositoryPath} onChange={(event) => setDraft({ ...draft, repositoryPath: event.target.value })} placeholder="选择空目录" />
                <button className="secondary-button" type="button" onClick={chooseRepository}><FolderOpen aria-hidden="true" size={15} />浏览</button>
              </div>
              <div className="settings-preference-row">
                <span className="settings-row-icon"><ShieldCheck aria-hidden="true" size={17} /></span>
                <span className="settings-row-copy"><strong>仓库完整性</strong><small>检查历史链、受控文件摘要与 C2PA 声明</small></span>
                <button className="secondary-button" type="button" onClick={() => void scrubRepository()} disabled={settingsBusy || !repository.ready}>
                  <ShieldCheck aria-hidden="true" size={15} />开始检查
                </button>
              </div>
              <div className="settings-preference-row">
                <span className="settings-row-icon"><DatabaseBackup aria-hidden="true" size={17} /></span>
                <span className="settings-row-copy"><strong>创建备份</strong><small>复制数据库与全部仓库文件，并在发布前校验备份</small></span>
                <button className="secondary-button" type="button" onClick={() => void backupRepository()} disabled={settingsBusy || !repository.ready}>
                  <DatabaseBackup aria-hidden="true" size={15} />创建备份
                </button>
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
              <div className="settings-section-title"><Clock3 aria-hidden="true" size={17} /><h3>自动备份</h3></div>
              <div className="settings-preference-list">
                <label className="settings-preference-row">
                  <span className="settings-row-icon"><Clock3 aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>自动备份调度</strong><small>{snapshot?.automaticBackupFileCount == null ? "仓库不可用" : `${snapshot.automaticBackupFileCount} 个工作文件已启用 · ${draft.pauseAutomaticBackups ? "已暂停" : "正在运行"}`}</small></span>
                  <input className="switch-input" type="checkbox" checked={!draft.pauseAutomaticBackups} onChange={(event) => setDraft({ ...draft, pauseAutomaticBackups: !event.target.checked })} />
                </label>
              </div>
            </div>

            <div className="settings-section">
              <div className="settings-section-title"><Palette aria-hidden="true" size={17} /><h3>外观</h3></div>
              <div className="settings-select-grid">
                <label>
                  <span>主题</span>
                  <select value={draft.theme} onChange={(event) => setDraft({ ...draft, theme: event.target.value as AppSettings["theme"] })}>
                    <option value="system">跟随系统</option>
                    <option value="light">浅色</option>
                    <option value="dark">深色</option>
                  </select>
                </label>
                <label>
                  <span>内容密度</span>
                  <select value={draft.content.density} onChange={(event) => setDraft({ ...draft, content: { ...draft.content, density: event.target.value as AppSettings["content"]["density"] } })}>
                    <option value="comfortable">舒适</option>
                    <option value="compact">紧凑</option>
                  </select>
                </label>
              </div>
            </div>

            <div className="settings-section">
              <div className="settings-section-title"><MonitorCog aria-hidden="true" size={17} /><h3>应用行为</h3></div>
              <div className="settings-preference-list">
                <label className="settings-preference-row">
                  <span className="settings-row-icon"><PanelLeftClose aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>关闭时驻留托盘</strong><small>{draft.closeToTray ? "已启用" : "已关闭"}</small></span>
                  <input className="switch-input" type="checkbox" checked={draft.closeToTray} onChange={(event) => setDraft({ ...draft, closeToTray: event.target.checked })} />
                </label>
                <div className="settings-preference-row">
                  <span className="settings-row-icon"><Settings aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>配置文件夹</strong><small title={snapshot?.settingsPath}>{snapshot?.settingsPath ?? "设置目录尚未就绪"}</small></span>
                  <button className="secondary-button" type="button" onClick={() => void appApi.openSettingsDirectory().catch((error) => setMessage(error instanceof Error ? error.message : String(error)))}><FolderOpen aria-hidden="true" size={15} />打开</button>
                </div>
                <div className="settings-preference-row">
                  <span className="settings-row-icon"><FolderOpen aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>诊断日志</strong><small title={snapshot?.logDirectory}>{snapshot?.logDirectory ?? "日志目录尚未就绪"}</small></span>
                  <button className="secondary-button" type="button" onClick={() => void appApi.openLogDirectory().catch((error) => setMessage(error instanceof Error ? error.message : String(error)))}><FolderOpen aria-hidden="true" size={15} />打开</button>
                </div>
              </div>
            </div>

            <div className="settings-section">
              <div className="settings-section-title"><Info aria-hidden="true" size={17} /><h3>关于与法律</h3></div>
              <div className="settings-preference-list">
                <div className="settings-preference-row">
                  <span className="settings-row-icon"><Info aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>Lilith Artworks {packageInfo.version}</strong><small>Copyright 2026 ACSProgram · GPL-3.0-only</small></span>
                  <button className="secondary-button" type="button" onClick={() => void appApi.openLegalDirectory().catch((error) => setMessage(error instanceof Error ? error.message : String(error)))}><FolderOpen aria-hidden="true" size={15} />查看许可</button>
                </div>
              </div>
            </div>

            <footer>
              <button className="secondary-button" type="button" onClick={() => setSettingsOpen(false)}>取消</button>
              <button className="primary-button" type="button" onClick={save} disabled={settingsBusy}>
                {busy && <LoaderCircle className="spin" aria-hidden="true" size={16} />}
                保存
              </button>
            </footer>
          </section>
        </div>
      )}
    </main>
  );
}

function SettingsOperationProgress({
  runtime,
  cancelPending,
  onCancel,
}: {
  runtime: BackupRuntimeStatus;
  cancelPending: boolean;
  onCancel: () => void;
}) {
  const hasTotal = runtime.progressTotal > 0;
  const percent = hasTotal
    ? Math.round(runtime.progressCurrent / runtime.progressTotal * 100)
    : 0;
  return (
    <div className="settings-operation-progress" role="status" aria-live="polite">
      <div>
        <LoaderCircle className="spin" aria-hidden="true" size={15} />
        <span>{runtime.progressLabel ?? "正在处理"}</span>
        <strong>{hasTotal ? `${percent}%` : "准备中"}</strong>
      </div>
      <progress value={runtime.progressCurrent} max={Math.max(runtime.progressTotal, 1)} />
      <button
        className="icon-button"
        type="button"
        title="取消当前操作"
        aria-label="取消当前操作"
        disabled={cancelPending}
        onClick={onCancel}
      >
        {cancelPending ? <LoaderCircle className="spin" aria-hidden="true" size={15} /> : <X aria-hidden="true" size={15} />}
      </button>
    </div>
  );
}
