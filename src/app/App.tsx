import { useEffect } from "react";
import { LoaderCircle } from "lucide-react";
import { LibraryModule } from "../modules/library/LibraryModule";
import { ArtworkWorkspace } from "./ArtworkWorkspace";
import { appApi } from "./api";
import { AppRuntimeProvider, useAppRuntime } from "./AppRuntimeProvider";
import { NoticeCenter, useNotice } from "./NoticeCenter";
import { SettingsDialog } from "./SettingsDialog";
import { WindowTitleBar } from "./WindowTitleBar";
import { preventWebViewFileDrop, preventWebViewReload } from "./webviewShortcuts";

/**
 * 应用入口只负责组合运行时与渲染工作区。设置、维护、进度、提示各自有独立
 * provider/controller，`App` 不再持有跨领域状态。
 */
export function App() {
  return (
    <NoticeCenter>
      <AppRuntimeProvider>
        <AppShell />
      </AppRuntimeProvider>
    </NoticeCenter>
  );
}

function AppShell() {
  const { settings } = useAppRuntime();
  const { notify } = useNotice();
  const {
    snapshot,
    repository,
    repositoryLabel,
    busy,
    openSettings,
    pinBoardSettings,
  } = settings;

  // F5 / Ctrl+R 只会被取消默认行为，不吞掉事件；素材板“锁定画板”等模块快捷键
  // 仍能拿到同一组合键（默认 Ctrl+R），避免整页刷新丢失画布状态。
  useEffect(() => {
    window.addEventListener("keydown", preventWebViewReload, true);
    return () => window.removeEventListener("keydown", preventWebViewReload, true);
  }, []);

  // 拖入窗口的文件默认会让 WebView 导航到该文件；统一取消默认行为，识别页等
  // 模块仍可在自己的元素上处理拖放。
  useEffect(() => {
    window.addEventListener("dragover", preventWebViewFileDrop);
    window.addEventListener("drop", preventWebViewFileDrop);
    return () => {
      window.removeEventListener("dragover", preventWebViewFileDrop);
      window.removeEventListener("drop", preventWebViewFileDrop);
    };
  }, []);

  return (
    <main className="app-shell">
      <WindowTitleBar
        repositoryLabel={repositoryLabel}
        repositoryReady={repository.ready}
        onOpenSettings={openSettings}
        onError={notify}
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
          onError={notify}
          onRetryFileCleanup={appApi.retryFileCleanup}
          onAcknowledgeBackupDisableNotices={appApi.acknowledgeBackupDisableNotices}
          onOpenBackupDisableNotice={appApi.getBackupDisableNoticeTarget}
          renderArtworkWorkspace={(workspace) => (
            <ArtworkWorkspace
              artworkId={workspace.artworkId}
              initialView={workspace.initialView}
              initialBranchId={workspace.initialBranchId}
              initialRecordId={workspace.initialRecordId}
              navigationKey={workspace.navigationKey}
              pinBoardSettings={pinBoardSettings}
              onError={notify}
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

      <SettingsDialog />
    </main>
  );
}
