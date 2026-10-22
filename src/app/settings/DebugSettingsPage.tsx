import { Activity, Bug, FolderOpen } from "lucide-react";
import type { DiagnosticsStatus } from "../../shared/diagnostics";
import type { SettingsSnapshot } from "../types";
import type { OpenSettingsFolder } from "./shared";

interface DebugSettingsPageProps {
  diagnostics: DiagnosticsStatus | null;
  snapshot: SettingsSnapshot | null;
  onToggleDiagnostics: (enabled: boolean) => Promise<void>;
  openFolder: OpenSettingsFolder;
}

export function DebugSettingsPage({
  diagnostics,
  snapshot,
  onToggleDiagnostics,
  openFolder,
}: DebugSettingsPageProps) {
  return <div className="settings-section">
    <div className="settings-section-title"><Bug aria-hidden="true" size={17} /><h3>日志与诊断</h3></div>
    <div className="settings-preference-list">
      <label className="settings-preference-row">
        <span className="settings-row-icon"><Activity aria-hidden="true" size={17} /></span>
        <span className="settings-row-copy">
          <strong>详细日志</strong>
          <small>
            {diagnostics?.enabled
              ? `已开启 · 当前等级 ${diagnostics.level}`
              : "已关闭 · 仅记录常规事件"}
          </small>
        </span>
        <input
          className="switch-input"
          type="checkbox"
          aria-label="详细日志"
          checked={diagnostics?.enabled ?? false}
          onChange={(event) => void onToggleDiagnostics(event.target.checked)}
        />
      </label>
      <div className="settings-preference-row">
        <span className="settings-row-icon"><FolderOpen aria-hidden="true" size={17} /></span>
        <span className="settings-row-copy"><strong>日志文件夹</strong><small title={snapshot?.logDirectory}>{snapshot?.logDirectory ?? "日志目录尚未就绪"}</small></span>
        <button className="secondary-button" type="button" onClick={() => void openFolder("openLogDirectory")}><FolderOpen aria-hidden="true" size={15} />打开</button>
      </div>
    </div>
  </div>;
}
