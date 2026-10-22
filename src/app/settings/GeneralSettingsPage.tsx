import {
  FolderOpen, Info, MonitorCog, Palette, PanelLeftClose, Settings,
} from "lucide-react";
import packageInfo from "../../../package.json";
import type { AppSettings, SettingsSnapshot } from "../types";
import type { OpenSettingsFolder, SettingsUpdate } from "./shared";

interface GeneralSettingsPageProps {
  draft: AppSettings;
  snapshot: SettingsSnapshot | null;
  update: SettingsUpdate;
  openFolder: OpenSettingsFolder;
}

export function GeneralSettingsPage({
  draft,
  snapshot,
  update,
  openFolder,
}: GeneralSettingsPageProps) {
  return <>
    <div className="settings-section">
      <div className="settings-section-title"><Palette aria-hidden="true" size={17} /><h3>外观</h3></div>
      <div className="settings-select-grid">
        <label>
          <span>主题</span>
          <select value={draft.theme} onChange={(event) => update({ theme: event.target.value as AppSettings["theme"] })}>
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </label>
        <label>
          <span>内容密度</span>
          <select value={draft.content.density} onChange={(event) => update({ content: { ...draft.content, density: event.target.value as AppSettings["content"]["density"] } })}>
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
          <input className="switch-input" type="checkbox" checked={draft.closeToTray} onChange={(event) => update({ closeToTray: event.target.checked })} />
        </label>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Settings aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy"><strong>配置文件夹</strong><small title={snapshot?.settingsPath}>{snapshot?.settingsPath ?? "设置目录尚未就绪"}</small></span>
          <button className="secondary-button" type="button" onClick={() => void openFolder("openSettingsDirectory")}><FolderOpen aria-hidden="true" size={15} />打开</button>
        </div>
      </div>
    </div>

    <div className="settings-section">
      <div className="settings-section-title"><Info aria-hidden="true" size={17} /><h3>关于与法律</h3></div>
      <div className="settings-preference-list">
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Info aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy"><strong>Lilith Artworks {packageInfo.version}</strong><small>Copyright 2026 ACSProgram · GPL-3.0-only</small></span>
          <button className="secondary-button" type="button" onClick={() => void openFolder("openLegalDirectory")}><FolderOpen aria-hidden="true" size={15} />查看许可</button>
        </div>
      </div>
    </div>
  </>;
}
