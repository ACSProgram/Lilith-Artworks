import { useEffect } from "react";
import { LoaderCircle, Settings, Trash2, X } from "lucide-react";
import { appApi } from "./api";
import { useAppRuntime } from "./appRuntimeContext";
import { useNotice } from "./noticeContext";
import { SETTINGS_PAGES } from "./useSettingsController";
import { errorMessage } from "../shared/errors";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import type { AppSettings } from "./types";
import { BackupSettingsPage } from "./settings/BackupSettingsPage";
import { DebugSettingsPage } from "./settings/DebugSettingsPage";
import { GeneralSettingsPage } from "./settings/GeneralSettingsPage";
import { PinBoardSettingsPage } from "./settings/PinBoardSettingsPage";
import { RepositorySettingsPage } from "./settings/RepositorySettingsPage";
import { SpaceSettingsPage } from "./settings/SpaceSettingsPage";

/** 设置弹窗外壳：导航、头部、底部操作区与跨页确认弹窗。各页只接收显式属性。 */
export function SettingsDialog() {
  const { settings, maintenance } = useAppRuntime();
  const { notify } = useNotice();
  const refreshStorageOverview = maintenance.refreshStorageOverview;

  // Hooks 必须在弹窗关闭的早退之前调用；空间页首次可见时读取一次 DB 口径。
  useEffect(() => {
    if (
      !settings.settingsOpen
      || settings.settingsPage !== "space"
      || !settings.repository.ready
    ) {
      return;
    }
    void refreshStorageOverview();
  }, [settings.repository.ready, settings.settingsOpen, settings.settingsPage, refreshStorageOverview]);

  if (!settings.settingsOpen || !settings.draft) return null;

  const {
    draft, snapshot, repository,
    settingsPage, setSettingsPage, busy, diagnostics,
  } = settings;
  const {
    settingsBusy,
    visibleSettingsRuntime,
    cancelPending,
    scanCandidates,
    scanConfirmOpen,
    setScanConfirmOpen,
    externalCleanupTarget,
    setExternalCleanupTarget,
  } = maintenance;

  const setPinBoardShortcut = (key: "lockShortcut" | "fullscreenShortcut", shortcut: string) => {
    settings.setDraft((current) => current
      ? { ...current, pinBoard: { ...current.pinBoard, [key]: shortcut } }
      : current);
  };

  const update = (patch: Partial<AppSettings>) => {
    settings.setDraft((current) => current ? { ...current, ...patch } : current);
  };

  const openFolder = async (
    command: "openSettingsDirectory" | "openLogDirectory" | "openLegalDirectory",
  ) => {
    try {
      await appApi[command]();
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  return (
    <>
      <div className="dialog-backdrop" role="presentation" onMouseDown={() => settings.closeSettings()}>
        <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title" onMouseDown={(event) => event.stopPropagation()}>
          <header>
            <div className="settings-heading">
              <span className="settings-heading-icon"><Settings aria-hidden="true" size={18} /></span>
              <div>
                <h2 id="settings-title">设置</h2>
                <small title={snapshot?.settingsPath}>应用与仓库</small>
              </div>
            </div>
            <button className="icon-button" type="button" title="关闭设置" onClick={() => settings.closeSettings()}><X aria-hidden="true" size={18} /></button>
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
                <GeneralSettingsPage
                  draft={draft}
                  snapshot={snapshot}
                  update={update}
                  openFolder={openFolder}
                />
              )}

              {settingsPage === "repository" && (
                <RepositorySettingsPage
                  draft={draft}
                  repository={repository}
                  update={update}
                  settingsBusy={settingsBusy}
                  chooseRepository={settings.chooseRepository}
                  scrubRepository={maintenance.scrubRepository}
                  cancelSettingsOperation={maintenance.cancelSettingsOperation}
                  visibleSettingsRuntime={visibleSettingsRuntime}
                  cancelPending={cancelPending}
                />
              )}

              {settingsPage === "space" && (
                <SpaceSettingsPage
                  repository={repository}
                  maintenance={maintenance}
                  settingsBusy={settingsBusy}
                  visibleSettingsRuntime={visibleSettingsRuntime}
                  cancelPending={cancelPending}
                />
              )}

              {settingsPage === "backup" && (
                <BackupSettingsPage
                  draft={draft}
                  snapshot={snapshot}
                  repository={repository}
                  update={update}
                  settingsBusy={settingsBusy}
                  backupRepository={maintenance.backupRepository}
                  cancelSettingsOperation={maintenance.cancelSettingsOperation}
                  visibleSettingsRuntime={visibleSettingsRuntime}
                  cancelPending={cancelPending}
                  lastRepositoryBackup={maintenance.lastRepositoryBackup}
                />
              )}

              {settingsPage === "pin-board" && (
                <PinBoardSettingsPage
                  draft={draft}
                  update={update}
                  setPinBoardShortcut={setPinBoardShortcut}
                />
              )}

              {settingsPage === "debug" && (
                <DebugSettingsPage
                  diagnostics={diagnostics}
                  snapshot={snapshot}
                  onToggleDiagnostics={settings.toggleDiagnostics}
                  openFolder={openFolder}
                />
              )}
            </div>
          </div>

          <footer>
            <button className="secondary-button" type="button" onClick={() => settings.closeSettings()}>取消</button>
            <button className="primary-button" type="button" onClick={() => void settings.save()} disabled={settingsBusy}>
              {busy && <LoaderCircle className="spin" aria-hidden="true" size={16} />}
              保存
            </button>
          </footer>
        </section>
      </div>

      {scanConfirmOpen && scanCandidates && scanCandidates.length > 0 && (
        <ConfirmDialog
          view={{
            icon: <Trash2 size={20} />,
            eyebrow: "不可撤销",
            title: "清理垃圾文件",
            subject: `${scanCandidates.length} 个可清理的文件`,
            description: "这些文件删除后无法恢复；检查点冗余增量会在取消该检查点时重建。",
            detail: "清理前会再次复查引用与文件摘要；内容已变化或重新被引用的文件会被保留。",
            action: `清理 ${scanCandidates.length} 个文件`,
            danger: true,
          }}
          busy={maintenance.cleanupBusy}
          onCancel={() => setScanConfirmOpen(false)}
          onConfirm={() => void maintenance.purgeScanCandidates()}
        />
      )}

      {externalCleanupTarget && (
        <ConfirmDialog
          view={{
            icon: <Trash2 size={20} />,
            eyebrow: "仓库外文件 · 不可撤销",
            title: "确认删除这个仓库外文件",
            subject: externalCleanupTarget.path,
            description: "该文件位于仓库之外，删除后无法从仓库恢复；仅在它确实是应用写出的认证导出残留时才应删除。",
            detail: [
              `所属作品：${externalCleanupTarget.artworkTitle ?? "未知"}`,
              `期望 SHA-256：${externalCleanupTarget.expectedSha256 ?? "未记录"}`,
              "删除前会再比对文件摘要；内容已变化的文件会被保留。",
            ].join("\n"),
            action: "删除这个文件",
            danger: true,
            emphasis: true,
            slideToConfirm: true,
          }}
          busy={maintenance.cleanupBusy}
          onCancel={() => setExternalCleanupTarget(null)}
          onConfirm={() => {
            const target = externalCleanupTarget;
            setExternalCleanupTarget(null);
            void maintenance.retryCleanup([target.id]);
          }}
        />
      )}
    </>
  );
}
