import { AlertTriangle, FolderOpen, RefreshCw, RotateCcw } from "lucide-react";
import { appApi } from "./api";
import type { ErrorFallbackState } from "./ErrorBoundary";
import { WindowControls } from "./WindowControls";
import { diagnosticsLog } from "../shared/diagnostics";
import { errorMessage } from "../shared/errors";

/**
 * 根边界的兜底：铺满窗口。
 *
 * 它必须自带可拖拽顶条与窗口按钮——真正的标题栏在 `AppShell` 里，已经随组件树卸载，
 * 而窗口是 `decorations: false` 的无边框窗口，F5 / Ctrl+R 又被应用主动屏蔽。没有这条
 * 顶条，用户面对的是一块既不能拖也不能关的空白窗口。
 */
export function AppFatalFallback({ error, retry }: ErrorFallbackState) {
  const reload = () => {
    diagnosticsLog("warn", "[boundary] root fallback reload requested by user");
    window.location.reload();
  };
  const openLogs = () => {
    diagnosticsLog("info", "[boundary] root fallback open log directory requested");
    try {
      void appApi.openLogDirectory().catch(() => undefined);
    } catch {
      // 非 Tauri 宿主（浏览器预览、测试）没有该命令，忽略。
    }
  };

  return (
    <div className="app-fatal">
      <header className="topbar app-fatal-topbar" data-tauri-drag-region>
        <div className="brand-block" data-tauri-drag-region>
          <AlertTriangle aria-hidden="true" size={20} />
          <strong data-tauri-drag-region>Lilith Artworks</strong>
        </div>
        <span className="app-fatal-hint" data-tauri-drag-region>界面组件出错，窗口仍可拖动与关闭</span>
        <div className="topbar-actions">
          <WindowControls />
        </div>
      </header>

      <section className="app-fatal-body">
        <div className="app-fatal-panel">
          <div className="app-fatal-heading">
            <AlertTriangle aria-hidden="true" size={22} />
            <h2>界面遇到问题</h2>
          </div>
          <p>
            某个界面组件在渲染时出错，已被错误边界隔离。仓库数据与已保存的画板内容不受影响；
            尚未写回仓库的画板改动仍留在后台队列中等待写回。
          </p>
          <div className="app-fatal-actions">
            <button className="primary-button" type="button" onClick={retry}>
              <RotateCcw aria-hidden="true" size={16} />重试
            </button>
            <button className="secondary-button" type="button" onClick={reload}>
              <RefreshCw aria-hidden="true" size={16} />重新加载界面
            </button>
            <button className="secondary-button" type="button" onClick={openLogs}>
              <FolderOpen aria-hidden="true" size={16} />打开日志目录
            </button>
          </div>
          <p className="app-fatal-note">
            优先用「重试」：它只重挂出错的部分。「重新加载界面」会丢弃尚未写回仓库的画板改动。
          </p>
          <details className="app-fatal-detail">
            <summary>错误详情</summary>
            <pre>{error.stack || errorMessage(error)}</pre>
          </details>
        </div>
      </section>
    </div>
  );
}

/** 区块边界的兜底：只占出错区域，其余界面（含标题栏、作品库）照常可用。 */
export function SectionErrorFallback({ title, error, retry }: ErrorFallbackState & { title: string }) {
  return (
    <div className="section-fallback" role="alert">
      <AlertTriangle aria-hidden="true" size={20} />
      <div>
        <strong>{title}</strong>
        <p>{errorMessage(error)}</p>
      </div>
      <button className="secondary-button" type="button" onClick={retry}>重试</button>
    </div>
  );
}
