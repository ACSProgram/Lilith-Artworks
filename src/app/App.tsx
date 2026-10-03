import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Clock3,
  SearchCheck,
  DatabaseBackup,
  FolderOpen,
  Images,
  Info,
  Keyboard,
  Layers,
  LoaderCircle,
  LogOut,
  MonitorCog,
  MoveHorizontal,
  Palette,
  PanelLeftClose,
  Save,
  Settings,
  ShieldCheck,
  X,
} from "lucide-react";
import { LibraryModule } from "../modules/library/LibraryModule";
import { ArtworkWorkspace } from "./ArtworkWorkspace";
import {
  PIN_BOARD_FULLSCREEN_SHORTCUT,
  PIN_BOARD_LOCK_SHORTCUT,
} from "../modules/pin-board/PinBoardModule";
import { preparePinBoardRuntimeChange } from "../modules/pin-board/lifecycle";
import { appApi } from "./api";
import {
  shortcutFromEvent,
  shortcutLabel,
} from "./settingsShortcuts";
import { preventWebViewReload } from "./webviewShortcuts";
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

type SettingsPage = "general" | "repository" | "pin-board";

const SETTINGS_PAGES: Array<{ id: SettingsPage; label: string }> = [
  { id: "general", label: "通用" },
  { id: "repository", label: "仓库与备份" },
  { id: "pin-board", label: "素材板" },
];

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
  const [settingsPage, setSettingsPage] = useState<SettingsPage>("general");
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

  // F5 / Ctrl+R 只会被取消默认行为，不吞掉事件；素材板“锁定画板”等模块快捷键
  // 仍能拿到同一组合键（默认 Ctrl+R），避免整页刷新丢失画布状态。
  useEffect(() => {
    window.addEventListener("keydown", preventWebViewReload, true);
    return () => window.removeEventListener("keydown", preventWebViewReload, true);
  }, []);

  // 原生端关闭窗口或托盘退出时不再直接结束进程，而是先请求 webview 结算素材板
  // （保存并截断步骤历史），确认完成后再由 confirm_app_shutdown 退出；Rust 侧
  // 有 15 秒兜底强退，因此这里无论结算成败都必须回复确认，不能阻塞退出。
  // "关闭时保存"开关关闭时跳过结算，回到退出即丢未保存编辑的旧行为。
  // 退出握手时是否先结算素材板；默认开启，读取失败时保持开启以免静默丢编辑。
  const pinBoardSaveOnExit = snapshot?.settings.pinBoard?.saveOnExit ?? true;
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen("app_shutdown_requested", () => {
      const settle = pinBoardSaveOnExit
        ? preparePinBoardRuntimeChange().catch(() => undefined)
        : Promise.resolve();
      void settle.finally(() => {
        void appApi.confirmShutdown().catch(() => undefined);
      });
    })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch(() => {
        // 监听不可用时（例如非 Tauri 环境）依赖原生端 15 秒兜底强退，不阻塞退出。
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [pinBoardSaveOnExit]);

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

  const pinBoardSettings = useMemo(() => ({
    arrangementGapPx: snapshot?.settings.pinBoard?.arrangementGapPx ?? 10,
    textureCacheLevel: snapshot?.settings.pinBoard?.textureCacheLevel ?? "medium",
    autosave: snapshot?.settings.pinBoard?.autosave ?? false,
    lockShortcut: snapshot?.settings.pinBoard?.lockShortcut ?? PIN_BOARD_LOCK_SHORTCUT,
    fullscreenShortcut: snapshot?.settings.pinBoard?.fullscreenShortcut ?? PIN_BOARD_FULLSCREEN_SHORTCUT,
  }), [snapshot]);

  const setPinBoardShortcut = (key: "lockShortcut" | "fullscreenShortcut", shortcut: string) => {
    setDraft((current) => current
      ? { ...current, pinBoard: { ...current.pinBoard, [key]: shortcut } }
      : current);
  };

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
              pinBoardSettings={pinBoardSettings}
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

            <div className="settings-body">
              <nav className="settings-navigation" aria-label="设置分类">
                {SETTINGS_PAGES.map((page) => (
                  <button
                    key={page.id}
                    type="button"
                    className={settingsPage === page.id ? "active" : ""}
                    onClick={() => setSettingsPage(page.id)}
                  >
                    {page.label}
                  </button>
                ))}
              </nav>
              <div className="settings-content">
              {settingsPage === "general" && (
                <>
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
                </>
              )}

              {settingsPage === "repository" && (
                <>
                <div className="settings-section">
                  <div className="settings-section-title"><FolderOpen aria-hidden="true" size={17} /><h3>仓库与数据安全</h3></div>
                  <div className="settings-path-field">
                    <label htmlFor="repository-path">仓库位置</label>
                    <div className="path-control">
                      <input id="repository-path" aria-label="作品仓库路径" value={draft.repositoryPath} onChange={(event) => setDraft({ ...draft, repositoryPath: event.target.value })} placeholder="选择空目录" />
                      <button className="secondary-button" type="button" onClick={chooseRepository}><FolderOpen aria-hidden="true" size={15} />浏览</button>
                    </div>
                  </div>
                  <div className="settings-preference-list settings-repository-actions">
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><ShieldCheck aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy"><strong>仓库完整性</strong><small>检查历史链、受控文件摘要与 C2PA 声明</small></span>
                      <button className="secondary-button" type="button" onClick={() => void scrubRepository()} disabled={settingsBusy || !repository.ready}>
                        <ShieldCheck aria-hidden="true" size={15} />开始检查
                      </button>
                    </div>
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><DatabaseBackup aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy"><strong>创建备份</strong><small>复制数据库与全部仓库文件，并校验备份</small></span>
                      <button className="secondary-button" type="button" onClick={() => void backupRepository()} disabled={settingsBusy || !repository.ready}>
                        <DatabaseBackup aria-hidden="true" size={15} />创建备份
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
                  <div className="settings-section-title"><Clock3 aria-hidden="true" size={17} /><h3>自动备份</h3></div>
                  <div className="settings-preference-list">
                    <label className="settings-preference-row">
                      <span className="settings-row-icon"><Clock3 aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy"><strong>自动备份调度</strong><small>{snapshot?.automaticBackupFileCount == null ? "仓库不可用" : `${snapshot.automaticBackupFileCount} 个工作文件已启用 · ${draft.pauseAutomaticBackups ? "已暂停" : "正在运行"}`}</small></span>
                      <input className="switch-input" type="checkbox" checked={!draft.pauseAutomaticBackups} onChange={(event) => setDraft({ ...draft, pauseAutomaticBackups: !event.target.checked })} />
                    </label>
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><SearchCheck aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy"><strong>默认检查方式</strong><small>{draft.automaticBackupCheckMode === "quick" ? "快速：仅比较大小与修改时间，有变化再全量备份" : "全量：每次都完整校验内容"}</small></span>
                      <select
                        aria-label="自动备份默认检查方式"
                        value={draft.automaticBackupCheckMode}
                        onChange={(event) => setDraft({ ...draft, automaticBackupCheckMode: event.target.value as AppSettings["automaticBackupCheckMode"] })}
                      >
                        <option value="quick">快速检查（推荐）</option>
                        <option value="full">全量校验</option>
                      </select>
                    </div>
                  </div>
                </div>
                </>
              )}

              {settingsPage === "pin-board" && (
                <div className="settings-section">
                  <div className="settings-section-title"><Images aria-hidden="true" size={17} /><h3>素材板</h3></div>
                  <div className="settings-preference-list">
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><Layers aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy is-descriptive">
                        <strong>纹理缓存等级</strong>
                        <small>决定素材板可常驻的纹理数量与内存占用。</small>
                      </span>
                      <div
                        className="segmented-control pin-board-cache-control"
                        role="radiogroup"
                        aria-label="纹理缓存等级"
                      >
                        {([
                          { level: "low", label: "低", memory: "约 256 MB" },
                          { level: "medium", label: "中", memory: "约 512 MB" },
                          { level: "high", label: "高", memory: "约 1 GB" },
                        ] as const).map(({ level, label, memory }) => (
                          <button
                            key={level}
                            type="button"
                            role="radio"
                            aria-checked={draft.pinBoard.textureCacheLevel === level}
                            className={draft.pinBoard.textureCacheLevel === level ? "active" : ""}
                            onClick={() => setDraft({ ...draft, pinBoard: { ...draft.pinBoard, textureCacheLevel: level } })}
                          >
                            <strong>{label}</strong>
                            <small>{memory}</small>
                          </button>
                        ))}
                      </div>
                    </div>
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><MoveHorizontal aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy is-descriptive">
                        <label htmlFor="pin-board-arrangement-gap"><strong>阵列图片间距</strong></label>
                        <small>阵列排序时图片之间的屏幕间距，范围 1–200 像素。</small>
                      </span>
                      <input
                        id="pin-board-arrangement-gap"
                        className="number-control"
                        type="number"
                        min={1}
                        max={200}
                        step={1}
                        value={draft.pinBoard.arrangementGapPx}
                        onChange={(event) => setDraft({ ...draft, pinBoard: { ...draft.pinBoard, arrangementGapPx: Math.min(200, Math.max(1, Number(event.target.value) || 1)) } })}
                      />
                    </div>
                    <label className="settings-preference-row">
                      <span className="settings-row-icon"><Save aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy is-descriptive">
                        <strong>自动保存</strong>
                        <small>编辑停止约 1.5 秒后自动保存摆放进度，崩溃或强退最多丢失几秒内的改动。</small>
                      </span>
                      <input className="switch-input" type="checkbox" checked={draft.pinBoard.autosave} onChange={(event) => setDraft({ ...draft, pinBoard: { ...draft.pinBoard, autosave: event.target.checked } })} />
                    </label>
                    <label className="settings-preference-row">
                      <span className="settings-row-icon"><LogOut aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy is-descriptive">
                        <strong>关闭时保存</strong>
                        <small>退出应用前先保存并结算素材板；关闭后退出即结束，未保存的编辑会丢失。</small>
                      </span>
                      <input className="switch-input" type="checkbox" checked={draft.pinBoard.saveOnExit} onChange={(event) => setDraft({ ...draft, pinBoard: { ...draft.pinBoard, saveOnExit: event.target.checked } })} />
                    </label>
                    <div className="settings-preference-row">
                      <span className="settings-row-icon"><Keyboard aria-hidden="true" size={17} /></span>
                      <span className="settings-row-copy is-descriptive">
                        <label htmlFor="pin-board-lock-shortcut"><strong>锁定画板快捷键</strong></label>
                        <small>锁定后无法编辑图片，仅可缩放和移动视图。</small>
                      </span>
                      <div className="shortcut-control">
                      <Keyboard size={16} aria-hidden="true" />
                      <input
                        id="pin-board-lock-shortcut"
                        className="shortcut-input"
                        value={shortcutLabel(draft.pinBoard.lockShortcut)}
                        placeholder="未设置"
                        readOnly
                        spellCheck={false}
                        onKeyDown={(event) => {
                          if (event.key === "Backspace" || event.key === "Delete") {
                            event.preventDefault();
                            setPinBoardShortcut("lockShortcut", "");
                            return;
                          }
                          const shortcut = shortcutFromEvent(event, true);
                          if (!shortcut) return;
                          event.preventDefault();
                          setPinBoardShortcut("lockShortcut", shortcut);
                        }}
                        onFocus={(event) => event.currentTarget.select()}
                      />
                      {draft.pinBoard.lockShortcut && (
                        <button
                          className="icon-button"
                          type="button"
                          title="清除锁定画板快捷键"
                          aria-label="清除锁定画板快捷键"
                          onClick={() => setPinBoardShortcut("lockShortcut", "")}
                        >
                          <X size={15} />
                        </button>
                      )}
                    </div>
                  </div>
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><Keyboard aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <label htmlFor="pin-board-fullscreen-shortcut"><strong>画板全屏快捷键</strong></label>
                      <small>切换素材板内容区全屏，全屏时隐藏顶部工具栏。</small>
                    </span>
                    <div className="shortcut-control">
                      <Keyboard size={16} aria-hidden="true" />
                      <input
                        id="pin-board-fullscreen-shortcut"
                        className="shortcut-input"
                        value={shortcutLabel(draft.pinBoard.fullscreenShortcut)}
                        placeholder="未设置"
                        readOnly
                        spellCheck={false}
                        onKeyDown={(event) => {
                          if (event.key === "Backspace" || event.key === "Delete") {
                            event.preventDefault();
                            setPinBoardShortcut("fullscreenShortcut", "");
                            return;
                          }
                          const shortcut = shortcutFromEvent(event, true);
                          if (!shortcut) return;
                          event.preventDefault();
                          setPinBoardShortcut("fullscreenShortcut", shortcut);
                        }}
                        onFocus={(event) => event.currentTarget.select()}
                      />
                      {draft.pinBoard.fullscreenShortcut && (
                        <button
                          className="icon-button"
                          type="button"
                          title="清除画板全屏快捷键"
                          aria-label="清除画板全屏快捷键"
                          onClick={() => setPinBoardShortcut("fullscreenShortcut", "")}
                        >
                          <X size={15} />
                        </button>
                      )}
                    </div>
                  </div>
                  </div>
                </div>
              )}
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
