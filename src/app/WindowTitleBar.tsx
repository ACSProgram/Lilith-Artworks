import { Archive, Settings } from "lucide-react";
import { WindowControls } from "./WindowControls";

interface WindowTitleBarProps {
  repositoryLabel: string;
  repositoryReady: boolean;
  onOpenSettings: () => void;
  onError: (message: string) => void;
}

export function WindowTitleBar({ repositoryLabel, repositoryReady, onOpenSettings, onError }: WindowTitleBarProps) {
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
      <WindowControls onError={onError} />
    </div>
  </header>;
}
