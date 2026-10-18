import { Fragment, useEffect } from "react";
import {
  Activity,
  BookOpen,
  Bug,
  Clock3,
  DatabaseBackup,
  Eye,
  FolderOpen,
  Images,
  Info,
  Keyboard,
  Layers,
  LoaderCircle,
  LogOut,
  MonitorCog,
  MousePointer2,
  MoveHorizontal,
  Palette,
  PanelLeftClose,
  Pencil,
  RotateCcw,
  Save,
  SearchCheck,
  Settings,
  ShieldCheck,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react";
import packageInfo from "../../package.json";
import { appApi } from "./api";
import { useAppRuntime } from "./AppRuntimeProvider";
import { useNotice } from "./NoticeCenter";
import { SETTINGS_PAGES } from "./useSettingsController";
import { shortcutFromEvent, shortcutLabel } from "./settingsShortcuts";
import { errorMessage } from "../shared/errors";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { formatBytes } from "../shared/format";
import {
  cleanupReasonLabel,
  isExternalCleanupEntry,
} from "../shared/fileCleanup";
import { RepositoryStoragePanel } from "./RepositoryStoragePanel";
import { SettingsOperationProgress } from "./SettingsOperationProgress";
import type { AppSettings } from "./types";

/**
 * 素材板操作手册内容。与 `renderer.ts` 的交互实现一一对应：改动交互时同步更新此处，
 * 保证设置页展示的操作说明与实际行为一致。`keys` 中的每一项渲染为一个按键胶囊。
 */
const PIN_BOARD_MANUAL: Array<{
  title: string;
  icon: LucideIcon;
  items: Array<{ label: string; detail: string; keys?: string[] }>;
}> = [
  {
    title: "视图",
    icon: Eye,
    items: [
      { label: "缩放视图", detail: "以光标为中心缩放，滚轮向前放大、向后缩小。", keys: ["滚轮"] },
      { label: "平移视图", detail: "画板锁定时无需按住修饰键，直接拖拽即可平移。", keys: ["中键拖拽"] },
      { label: "重置视图", detail: "回到全部内容的最小包围框；打开画板时也会自动适配。" },
    ],
  },
  {
    title: "选择",
    icon: MousePointer2,
    items: [
      { label: "单选", detail: "点击一张图片将其选中。", keys: ["左键点击"] },
      { label: "加选 / 减选", detail: "按住修饰键点击图片，切换该图片的选中状态。", keys: ["Ctrl / Shift", "点击"] },
      { label: "框选", detail: "在画布空白处按住左键拖拽，框住即选中。", keys: ["空白拖拽"] },
      { label: "连续加选框选", detail: "按住修饰键在空白处拖拽，可多次框选累加；已选中的图片不会取消，框到的新图加入。", keys: ["Ctrl / Shift", "拖拽"] },
      { label: "全选", detail: "选中整块画板的全部图片，不限当前视口。", keys: ["Ctrl", "A"] },
      { label: "取消选择", detail: "在空白处单击清空选择。", keys: ["空白单击"] },
    ],
  },
  {
    title: "编辑",
    icon: Pencil,
    items: [
      { label: "移动", detail: "拖动选中的图片。", keys: ["拖拽图片"] },
      { label: "旋转", detail: "拖拽选中框上方的旋转手柄；按住 Shift 吸附到 15° 整数倍。", keys: ["拖拽手柄"] },
      { label: "缩放", detail: "拖拽选中框四角的缩放手柄。", keys: ["拖拽手柄"] },
      { label: "阵列排序", detail: "按设置的间距平铺选中的图片（需选中两张以上）。" },
      { label: "删除", detail: "删除当前选中的图片。", keys: ["Delete"] },
      { label: "撤销", detail: "撤销上一步编辑。", keys: ["Ctrl", "Z"] },
      { label: "重做", detail: "重做已撤销的编辑。", keys: ["Ctrl", "Y"] },
      { label: "复制", detail: "复制选中的图片。", keys: ["Ctrl", "C"] },
      { label: "粘贴", detail: "把复制的图片粘贴到画板。", keys: ["Ctrl", "V"] },
    ],
  },
  {
    title: "画板与文件",
    icon: FolderOpen,
    items: [
      { label: "导入图片", detail: "把图片文件拖到画布，或使用工具栏「导入图片」。", keys: ["拖入文件"] },
      { label: "添加文字", detail: "生成透明 PNG 文字素材并导入画板。" },
      { label: "导出图片", detail: "把选中的图片导出为 PNG 到指定目录。" },
      { label: "复制到剪贴板", detail: "把选中的单张图片复制到系统剪贴板。" },
      { label: "保存", detail: "立即保存画板；可在上方开启自动保存。", keys: ["Ctrl", "S"] },
      { label: "锁定 / 全屏", detail: "使用上方「锁定画板快捷键」与「画板全屏快捷键」。" },
    ],
  },
];

/** 设置弹窗。状态全部来自 `AppRuntimeProvider`，弹窗本身不持有跨领域运行态。 */
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

  const { draft, snapshot, repository } = settings;
  const {
    settingsPage,
    setSettingsPage,
    busy,
    diagnostics,
  } = settings;
  const {
    settingsBusy,
    visibleSettingsRuntime,
    cancelPending,
    pendingCleanup,
    pendingCleanupSummary,
    repositoryCleanupCount,
    externalCleanupTarget,
    setExternalCleanupTarget,
    scanCandidates,
    setScanCandidates,
    scanConfirmOpen,
    setScanConfirmOpen,
    storageOverview,
    storageDiskScan,
    storageOverviewBusy,
    storageOverviewError,
    lastRepositoryBackup,
    scanStorageDisk,
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
              <>
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
              </>
            )}

            {settingsPage === "repository" && (
              <>
              <div className="settings-section">
                <div className="settings-section-title"><FolderOpen aria-hidden="true" size={17} /><h3>仓库与数据安全</h3></div>
                <div className="settings-path-field">
                  <label htmlFor="repository-path">仓库位置</label>
                  <div className="path-control">
                    <input id="repository-path" aria-label="作品仓库路径" value={draft.repositoryPath} onChange={(event) => update({ repositoryPath: event.target.value })} placeholder="选择空目录" />
                    <button className="secondary-button" type="button" onClick={() => void settings.chooseRepository()}><FolderOpen aria-hidden="true" size={15} />浏览</button>
                  </div>
                </div>
                <div className="settings-preference-list settings-repository-actions">
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><ShieldCheck aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy"><strong>仓库完整性</strong><small>检查历史链、受控文件摘要、画板 DDS 与 C2PA 声明</small></span>
                    <button className="secondary-button" type="button" onClick={() => void maintenance.scrubRepository()} disabled={settingsBusy || !repository.ready}>
                      <ShieldCheck aria-hidden="true" size={15} />开始检查
                    </button>
                  </div>
                </div>
                {visibleSettingsRuntime && (
                  <SettingsOperationProgress
                    runtime={visibleSettingsRuntime}
                    cancelPending={cancelPending}
                    onCancel={() => void maintenance.cancelSettingsOperation()}
                  />
                )}
              </div>

              <div className="settings-section">
                <div className="settings-section-title"><Layers aria-hidden="true" size={17} /><h3>存储格式</h3></div>
                <div className="settings-preference-list">
                  <label className="settings-preference-row">
                    <span className="settings-row-icon"><Layers aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>ChunkFile v2</strong>
                      <small>新节点使用容器对齐分块与快照去重，旧 v1 节点仍可正常读取；关闭后只影响此后新写入的节点。</small>
                    </span>
                    <input
                      className="switch-input"
                      type="checkbox"
                      aria-label="ChunkFile v2"
                      checked={draft.chunkFileFormatV2}
                      onChange={(event) => update({ chunkFileFormatV2: event.target.checked })}
                    />
                  </label>
                </div>
              </div>
              </>
            )}

            {settingsPage === "space" && (
              <>
              <RepositoryStoragePanel
                ready={repository.ready}
                overview={storageOverview}
                scan={storageDiskScan}
                loading={storageOverviewBusy}
                error={storageOverviewError}
                scanning={maintenance.settingsOperation === "storage-scan"}
                cancelPending={cancelPending}
                runtime={visibleSettingsRuntime}
                onRefresh={() => void refreshStorageOverview()}
                onScan={() => void scanStorageDisk()}
                onCancelScan={() => void maintenance.cancelSettingsOperation()}
              />

              <div className="settings-section">
                <div className="settings-section-title"><Trash2 aria-hidden="true" size={17} /><h3>文件清理</h3></div>
                <div className="settings-preference-list settings-repository-actions">
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><Trash2 aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>待清理队列</strong>
                      <small>{pendingCleanupSummary}</small>
                    </span>
                    <button
                      className="secondary-button"
                      type="button"
                      onClick={() => void maintenance.retryCleanup([])}
                      disabled={settingsBusy || !repository.ready || repositoryCleanupCount === 0}
                      title="只重放仓库内条目；仓库外文件需逐条确认"
                    >
                      <RotateCcw aria-hidden="true" size={15} />全部重试
                    </button>
                  </div>
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><SearchCheck aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>未引用文件扫描</strong>
                      <small>查找未被任何记录引用的历史快照、增量与画板图片；只报告不删除，确认后才清理。</small>
                    </span>
                    <button
                      className="secondary-button"
                      type="button"
                      onClick={() => void maintenance.scanUnreferenced()}
                      disabled={settingsBusy || !repository.ready}
                    >
                      <SearchCheck aria-hidden="true" size={15} />开始扫描
                    </button>
                  </div>
                </div>

                {pendingCleanup.length > 0 && (
                  <div className="cleanup-queue" role="list" aria-label="待清理文件队列">
                    {pendingCleanup.map((entry) => {
                      // 仓库外文件不参与自动重放：删除它等于删除仓库之外的任意
                      // 路径，必须逐条确认，且确认前要看到完整路径、所属作品与
                      // 期望摘要。
                      const external = isExternalCleanupEntry(entry);
                      return (
                        <div className="cleanup-queue-row" role="listitem" key={entry.id}>
                          <div className="cleanup-queue-copy">
                            <strong title={entry.path}>{entry.path}</strong>
                            <small>
                              {external ? "仓库外文件 · " : ""}
                              {cleanupReasonLabel(entry.reason)}
                              {external && ` · 所属作品：${entry.artworkTitle ?? "未知"}`}
                              {entry.lastError ? ` · 上次失败：${entry.lastError}` : external ? "" : " · 等待清理"}
                              {entry.lastAttemptMs ? ` · ${new Date(entry.lastAttemptMs).toLocaleString()}` : ""}
                            </small>
                            {external && (
                              <small title={entry.expectedSha256 ?? undefined}>
                                期望 SHA-256：{entry.expectedSha256 ?? "未记录"}
                              </small>
                            )}
                          </div>
                          {external ? (
                            <button
                              className="danger-button"
                              type="button"
                              disabled={settingsBusy}
                              onClick={() => setExternalCleanupTarget(entry)}
                            >
                              <Trash2 aria-hidden="true" size={15} />确认删除…
                            </button>
                          ) : (
                            <button
                              className="secondary-button"
                              type="button"
                              disabled={settingsBusy}
                              onClick={() => void maintenance.retryCleanup([entry.id])}
                            >
                              <RotateCcw aria-hidden="true" size={15} />重试
                            </button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}

                {scanCandidates && (
                  <div className="cleanup-scan-result" role="status">
                    <header>
                      <strong>{scanCandidates.length > 0 ? `发现 ${scanCandidates.length} 个未引用文件` : "没有发现未引用文件"}</strong>
                      <div>
                        <button className="text-button" type="button" onClick={() => setScanCandidates(null)}>关闭</button>
                        {scanCandidates.length > 0 && (
                          <button className="danger-button" type="button" disabled={settingsBusy} onClick={() => setScanConfirmOpen(true)}>
                            <Trash2 aria-hidden="true" size={15} />清理这些文件
                          </button>
                        )}
                      </div>
                    </header>
                    {scanCandidates.length > 0 && (
                      <ul className="cleanup-candidate-list">
                        {scanCandidates.map((candidate) => (
                          <li key={candidate.path}>
                            <span title={candidate.path}>{candidate.path}</span>
                            <small>{candidate.reason} · {formatBytes(candidate.byteSize)}</small>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                )}
              </div>
              </>
            )}

            {settingsPage === "backup" && (
              <>
              <div className="settings-section">
                <div className="settings-section-title"><Clock3 aria-hidden="true" size={17} /><h3>自动备份</h3></div>
                <div className="settings-preference-list">
                  <label className="settings-preference-row">
                    <span className="settings-row-icon"><Clock3 aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy"><strong>自动备份调度</strong><small>{snapshot?.automaticBackupFileCount == null ? "仓库不可用" : `${snapshot.automaticBackupFileCount} 个工作文件已启用 · ${draft.pauseAutomaticBackups ? "已暂停" : "正在运行"}`}</small></span>
                    <input className="switch-input" type="checkbox" checked={!draft.pauseAutomaticBackups} onChange={(event) => update({ pauseAutomaticBackups: !event.target.checked })} />
                  </label>
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><SearchCheck aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy"><strong>默认检查方式</strong><small>{draft.automaticBackupCheckMode === "quick" ? "快速：比较大小、修改时间与首尾采样，有变化再全量备份" : "全量：每次都完整校验内容"}</small></span>
                    <select
                      aria-label="自动备份默认检查方式"
                      value={draft.automaticBackupCheckMode}
                      onChange={(event) => update({ automaticBackupCheckMode: event.target.value as AppSettings["automaticBackupCheckMode"] })}
                    >
                      <option value="quick">快速检查（推荐）</option>
                      <option value="full">全量校验</option>
                    </select>
                  </div>
                </div>
              </div>

              <div className="settings-section">
                <div className="settings-section-title"><DatabaseBackup aria-hidden="true" size={17} /><h3>整仓备份</h3></div>
                <div className="settings-preference-list settings-repository-actions">
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><DatabaseBackup aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>创建备份</strong>
                      <small>复制数据库与全部仓库文件并校验副本；恢复时选择副本中的 repository 目录。</small>
                    </span>
                    <button className="secondary-button" type="button" onClick={() => void maintenance.backupRepository()} disabled={settingsBusy || !repository.ready}>
                      <DatabaseBackup aria-hidden="true" size={15} />创建备份
                    </button>
                  </div>
                </div>
                {lastRepositoryBackup ? (
                  <div className="settings-backup-result" role="status">
                    <strong>最近一次结果（本次运行）</strong>
                    <small>
                      {new Date(lastRepositoryBackup.completedAtMs).toLocaleString()} ·
                      {" "}{lastRepositoryBackup.report.fileCount} 个文件 ·
                      {" "}{formatBytes(lastRepositoryBackup.report.totalBytes)}
                    </small>
                    <small>
                      历史节点 {lastRepositoryBackup.report.historyNodes} ·
                      成品 {lastRepositoryBackup.report.finalArtifacts} ·
                      认证记录 {lastRepositoryBackup.report.certificationRecords}
                    </small>
                    <small title={lastRepositoryBackup.report.repositoryPath}>
                      恢复副本：{lastRepositoryBackup.report.repositoryPath}
                    </small>
                  </div>
                ) : (
                  <p className="settings-backup-empty">
                    本次运行尚未创建备份；重启后这里不会保留历史结果。
                  </p>
                )}
                {visibleSettingsRuntime && (
                  <SettingsOperationProgress
                    runtime={visibleSettingsRuntime}
                    cancelPending={cancelPending}
                    onCancel={() => void maintenance.cancelSettingsOperation()}
                  />
                )}
              </div>
              </>
            )}

            {settingsPage === "pin-board" && (
              <>
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
                          onClick={() => update({ pinBoard: { ...draft.pinBoard, textureCacheLevel: level } })}
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
                      onChange={(event) => update({ pinBoard: { ...draft.pinBoard, arrangementGapPx: Math.min(200, Math.max(1, Number(event.target.value) || 1)) } })}
                    />
                  </div>
                  <label className="settings-preference-row">
                    <span className="settings-row-icon"><Save aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>自动保存</strong>
                      <small>编辑停止约 1.5 秒后自动保存摆放进度，崩溃或强退最多丢失几秒内的改动。</small>
                    </span>
                    <input className="switch-input" type="checkbox" checked={draft.pinBoard.autosave} onChange={(event) => update({ pinBoard: { ...draft.pinBoard, autosave: event.target.checked } })} />
                  </label>
                  <label className="settings-preference-row">
                    <span className="settings-row-icon"><LogOut aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy is-descriptive">
                      <strong>关闭时保存</strong>
                      <small>退出应用前先保存并结算素材板；关闭后退出即结束，未保存的编辑会丢失。</small>
                    </span>
                    <input className="switch-input" type="checkbox" checked={draft.pinBoard.saveOnExit} onChange={(event) => update({ pinBoard: { ...draft.pinBoard, saveOnExit: event.target.checked } })} />
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
              <div className="settings-section">
                <div className="settings-section-title"><BookOpen aria-hidden="true" size={17} /><h3>操作手册</h3></div>
                <div className="pin-board-manual">
                  {PIN_BOARD_MANUAL.map((group) => {
                    const GroupIcon = group.icon;
                    return (
                      <section className="pin-board-manual-group" key={group.title}>
                        <header><GroupIcon aria-hidden="true" size={15} /><h4>{group.title}</h4></header>
                        <div className="pin-board-manual-rows">
                          {group.items.map((item) => (
                            <div className="pin-board-manual-row" key={item.label}>
                              <span className="pin-board-manual-label">{item.label}</span>
                              <span className="pin-board-manual-detail">{item.detail}</span>
                              <span className="pin-board-manual-keys">
                                {item.keys?.map((key, index) => (
                                  <Fragment key={key}>
                                    {index > 0 && <span className="pin-board-manual-plus" aria-hidden="true">+</span>}
                                    <kbd>{key}</kbd>
                                  </Fragment>
                                ))}
                              </span>
                            </div>
                          ))}
                        </div>
                      </section>
                    );
                  })}
                </div>
              </div>
              </>
            )}

            {settingsPage === "debug" && (
              <div className="settings-section">
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
                      onChange={(event) => void settings.toggleDiagnostics(event.target.checked)}
                    />
                  </label>
                  <div className="settings-preference-row">
                    <span className="settings-row-icon"><FolderOpen aria-hidden="true" size={17} /></span>
                    <span className="settings-row-copy"><strong>日志文件夹</strong><small title={snapshot?.logDirectory}>{snapshot?.logDirectory ?? "日志目录尚未就绪"}</small></span>
                    <button className="secondary-button" type="button" onClick={() => void openFolder("openLogDirectory")}><FolderOpen aria-hidden="true" size={15} />打开</button>
                  </div>
                </div>
              </div>
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
            title: "清理未引用文件",
            subject: `${scanCandidates.length} 个未被引用的文件`,
            description: "这些历史快照、增量或画板图片没有被任何记录引用，删除后无法恢复。",
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

