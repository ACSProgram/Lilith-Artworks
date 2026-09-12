import { Archive, Minus, Settings, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface WindowTitleBarProps {
  repositoryLabel: string;
  repositoryReady: boolean;
  onOpenSettings: () => void;
  onError: (message: string) => void;
}

const appWindow = getCurrentWindow();

export function WindowTitleBar({ repositoryLabel, repositoryReady, onOpenSettings, onError }: WindowTitleBarProps) {
  const runWindowCommand = (command: () => Promise<void>) => {
    void command().catch((error) => onError(error instanceof Error ? error.message : String(error)));
  };

  return <header className="topbar" data-tauri-drag-region>
    <div className="brand-block" data-tauri-drag-region>
      <Archive aria-hidden="true" size={20} />
      <strong data-tauri-drag-region>Lilith Artworks</strong>
    </div>
    <div className={`repository-state ${repositoryReady ? "ready" : ""}`} data-tauri-drag-region>
      <i className="repository-indicator" aria-hidden="true" />
      <span data-tauri-drag-region>{repositoryLabel}</span>
    </div>
    <div className="topbar-actions">
      <button className="icon-button settings-button" type="button" title="设置" onClick={onOpenSettings}>
        <Settings aria-hidden="true" size={18} />
      </button>
      <div className="window-controls" aria-label="窗口控制">
        <button className="window-control" type="button" title="最小化" onClick={() => runWindowCommand(() => appWindow.minimize())}>
          <Minus aria-hidden="true" size={16} />
        </button>
        <button className="window-control" type="button" title="最大化或还原" onClick={() => runWindowCommand(() => appWindow.toggleMaximize())}>
          <Square aria-hidden="true" size={13} />
        </button>
        <button className="window-control close" type="button" title="关闭" onClick={() => runWindowCommand(() => appWindow.close())}>
          <X aria-hidden="true" size={17} />
        </button>
      </div>
    </div>
  </header>;
}
