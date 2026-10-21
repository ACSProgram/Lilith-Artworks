import { useEffect } from "react";
import { LoaderCircle } from "lucide-react";
import { LibraryModule } from "../modules/library/LibraryModule";
import { ArtworkWorkspace } from "./ArtworkWorkspace";
import { appApi } from "./api";
import { AppRuntimeProvider } from "./AppRuntimeProvider";
import { useAppRuntime } from "./appRuntimeContext";
import { ErrorBoundary } from "./ErrorBoundary";
import { AppFatalFallback, SectionErrorFallback } from "./ErrorFallback";
import { NoticeCenter } from "./NoticeCenter";
import { useNotice } from "./noticeContext";
import { SettingsDialog } from "./SettingsDialog";
import { WindowTitleBar } from "./WindowTitleBar";
import { preventWebViewFileDrop, preventWebViewReload } from "./webviewShortcuts";

/**
 * 应用入口只负责组合运行时与渲染工作区。设置、维护、进度、提示各自有独立
 * provider/controller，`App` 不再持有跨领域状态。
 *
 * 根错误边界包在**所有 provider 之外**：三个 context hook 都在 provider 缺失时 `throw`，
 * 边界若放在它们里面就兜不住。这里同时是「窗口永远可用」的最后一道保证——组件树整棵
 * 卸载后，兜底面自带的顶条仍能拖动与关闭这个无边框窗口。
 */
export function App() {
  return (
    <ErrorBoundary label="root" fallback={AppFatalFallback}>
      <NoticeCenter>
        <AppRuntimeProvider>
          <AppShell />
        </AppRuntimeProvider>
      </NoticeCenter>
    </ErrorBoundary>
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
            // 工作区边界：某个作品的界面崩掉时，作品库与标题栏仍然可用，用户可以
            // 直接选别的作品继续工作；切换作品后 resetKeys 变化会自动复挂。
            <ErrorBoundary
              label={`workspace:${workspace.artworkId}`}
              resetKeys={[workspace.artworkId, workspace.navigationKey]}
              fallback={(state) => <SectionErrorFallback title="作品工作区渲染出错" {...state} />}
            >
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
            </ErrorBoundary>
          )}
        />
      )}

      <SettingsDialog />
    </main>
  );
}
