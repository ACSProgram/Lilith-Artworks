import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Archive,
  AlertCircle,
  Clock3,
  FolderOpen,
  LoaderCircle,
  MonitorCog,
  Palette,
  PanelLeftClose,
  Settings,
  X,
} from "lucide-react";
import { LibraryModule } from "../modules/library/LibraryModule";
import { appApi } from "./api";
import type { AppSettings, RepositoryStatus, SettingsSnapshot } from "./types";

const EMPTY_STATUS: RepositoryStatus = {
  configured: false,
  ready: false,
  rootPath: "",
  databasePath: "",
  error: null,
};

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
    setBusy(true);
    setMessage(null);
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

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-block">
          <Archive aria-hidden="true" size={20} />
          <strong>Lilith Artworks</strong>
        </div>
        <div className={`repository-state ${repository.ready ? "ready" : ""}`}>
          <span aria-hidden="true" />
          {repositoryLabel}
        </div>
        <button className="icon-button" type="button" title="设置" onClick={openSettings}>
          <Settings aria-hidden="true" size={18} />
        </button>
      </header>

      {busy && !snapshot ? (
        <section className="workspace full-workspace">
          <div className="loading-state"><LoaderCircle className="spin" aria-hidden="true" />正在读取设置</div>
        </section>
      ) : (
        <LibraryModule
          repositoryReady={repository.ready}
          onConfigure={openSettings}
          onError={setMessage}
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
              <div className="settings-section-title"><FolderOpen aria-hidden="true" size={17} /><h3>作品仓库</h3></div>
              <div className="path-control">
                <input aria-label="作品仓库路径" value={draft.repositoryPath} onChange={(event) => setDraft({ ...draft, repositoryPath: event.target.value })} placeholder="选择空目录" />
                <button className="secondary-button" type="button" onClick={chooseRepository}><FolderOpen aria-hidden="true" size={15} />浏览</button>
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
                <label className="settings-preference-row">
                  <span className="settings-row-icon"><Clock3 aria-hidden="true" size={17} /></span>
                  <span className="settings-row-copy"><strong>自动备份调度</strong><small>{snapshot?.automaticBackupFileCount == null ? "仓库不可用" : `${snapshot.automaticBackupFileCount} 个工作文件已启用 · ${draft.pauseAutomaticBackups ? "已暂停" : "正在运行"}`}</small></span>
                  <input className="switch-input" type="checkbox" checked={!draft.pauseAutomaticBackups} onChange={(event) => setDraft({ ...draft, pauseAutomaticBackups: !event.target.checked })} />
                </label>
              </div>
            </div>

            <footer>
              <button className="secondary-button" type="button" onClick={() => setSettingsOpen(false)}>取消</button>
              <button className="primary-button" type="button" onClick={save} disabled={busy}>
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
