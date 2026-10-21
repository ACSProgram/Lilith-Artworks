import {
  ClipboardCopy,
  ClipboardPaste,
  Copy,
  Download,
  Expand,
  FlipHorizontal2,
  FlipVertical2,
  Fullscreen,
  ImageDown,
  LayoutGrid,
  LocateFixed,
  Lock,
  Minimize2,
  PanelLeftClose,
  PanelLeftOpen,
  Redo2,
  RotateCcw,
  RotateCw,
  Save,
  Shrink,
  Type,
  Undo2,
  Unlock,
  Upload,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type { PinBoardRenderer, PinBoardInteractionState } from "./renderer";
import type { PinBoardModuleSettings, PinBoardView } from "./types";

function actionTitle(action: string, shortcut: string): string {
  const hint = shortcut.replace("CommandOrControl", "Ctrl").replaceAll("+", " + ");
  return hint ? `${action} (${hint})` : action;
}

export interface PinBoardToolbarProps {
  sidebar: boolean;
  title: string;
  view: PinBoardView | null;
  renderer: PinBoardRenderer | null;
  interaction: PinBoardInteractionState;
  clipboardCount: number;
  /** 导入/导出/整画布导出进行中：这些入口都要等传输结束。 */
  transferBusy: boolean;
  fullscreen: boolean;
  settings: PinBoardModuleSettings;
  /** 执行一次画板操作并同步交互状态（与渲染器直接改状态的两步合成一步）。 */
  run: (action: () => void) => void;
  onToggleSidebar: () => void;
  onCopy: () => void;
  onCopyClipboard: () => void;
  onPaste: () => void;
  onImport: () => void;
  onAddText: () => void;
  onExport: () => void;
  onExportCanvas: () => void;
  onToggleLock: () => void;
  onToggleFullscreen: () => void;
  onSave: () => void;
}

/** 主工具栏：视图、变换、条目操作与画板级动作按动组分块。 */
export function PinBoardToolbar({
  sidebar,
  title,
  view,
  renderer,
  interaction,
  clipboardCount,
  transferBusy,
  fullscreen,
  settings,
  run,
  onToggleSidebar,
  onCopy,
  onCopyClipboard,
  onPaste,
  onImport,
  onAddText,
  onExport,
  onExportCanvas,
  onToggleLock,
  onToggleFullscreen,
  onSave,
}: PinBoardToolbarProps) {
  /**
   * 编辑入口的统一禁用条件：锁定与整画布导出期间都不允许改动画板。导出渲染的是
   * 「点下那一刻的画面」，中途改动会让输出与用户看到的对不上。
   */
  const editingBlocked = interaction.locked || interaction.exporting;
  return (
    <header className="pin-board-toolbar">
      <button
        className="icon-button"
        onClick={onToggleSidebar}
        title={sidebar ? "收起画板列表" : "展开画板列表"}
      >
        {sidebar ? <PanelLeftClose size={18} /> : <PanelLeftOpen size={18} />}
      </button>
      <div className="pin-board-title">
        <strong>{title}</strong>
        <span>
          {view
            ? `${renderer ? interaction.itemCount : view.items.filter((image) => !image.deleted).length} 个条目${view.missingTextures ? `，其中 ${view.missingTextures} 个位图缺失` : ""}`
            : "选择画板"}
        </span>
      </div>
      <div className="pin-board-actions" aria-label="画板工具">
        <button className="icon-button" disabled={!renderer || !interaction.canUndo} onClick={() => run(() => renderer?.undo())} title="撤销" aria-label="撤销"><Undo2 size={17} /></button>
        <button className="icon-button" disabled={!renderer || !interaction.canRedo} onClick={() => run(() => renderer?.redo())} title="恢复" aria-label="恢复"><Redo2 size={17} /></button>
        <span className="pin-board-divider" />
        <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.zoomOut())} title="缩小视图" aria-label="缩小视图"><ZoomOut size={17} /></button>
        <span className="pin-board-zoom">{interaction.zoomPercent}%</span>
        <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.zoomIn())} title="放大视图" aria-label="放大视图"><ZoomIn size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.exporting} onClick={() => run(() => renderer?.resetViewport())} title="重置视图" aria-label="重置视图"><LocateFixed size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.scaleSelected(1 / 1.1))} title="缩小图片" aria-label="缩小图片"><Shrink size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.scaleSelected(1.1))} title="放大图片" aria-label="放大图片"><Expand size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.rotateSelected(-5))} title="逆时针旋转" aria-label="逆时针旋转"><RotateCcw size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.rotateSelected(5))} title="顺时针旋转" aria-label="顺时针旋转"><RotateCw size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.flipSelected(true))} title="水平翻转" aria-label="水平翻转"><FlipHorizontal2 size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={() => run(() => renderer?.flipSelected(false))} title="垂直翻转" aria-label="垂直翻转"><FlipVertical2 size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.arrangeableCount < 2 || editingBlocked} onClick={() => run(() => void renderer?.arrangeSelected())} title="阵列排序" aria-label="阵列排序"><LayoutGrid size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount === 0 || editingBlocked} onClick={onCopy} title="复制画板条目" aria-label="复制画板条目"><Copy size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.selectedCount !== 1 || interaction.exportableCount === 0 || editingBlocked} onClick={onCopyClipboard} title="复制图片到剪贴板" aria-label="复制图片到剪贴板"><ClipboardCopy size={17} /></button>
        <button className="icon-button" disabled={!renderer || clipboardCount === 0 || editingBlocked} onClick={onPaste} title="粘贴画板条目" aria-label="粘贴画板条目"><ClipboardPaste size={17} /></button>
        <button className="icon-button" disabled={!renderer || editingBlocked || transferBusy} onClick={onImport} title="导入图片" aria-label="导入图片"><Upload size={17} /></button>
        <button className="icon-button" disabled={!renderer || editingBlocked || transferBusy} onClick={onAddText} title="添加文字" aria-label="添加文字"><Type size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.exportableCount === 0 || editingBlocked || transferBusy} onClick={onExport} title="导出图片" aria-label="导出图片"><Download size={17} /></button>
        <button className="icon-button" disabled={!renderer || interaction.itemCount === 0 || editingBlocked || transferBusy} onClick={onExportCanvas} title="导出画布 PNG" aria-label="导出画布 PNG"><ImageDown size={17} /></button>
        <button className={`icon-button${interaction.locked ? " active" : ""}`} disabled={!renderer || interaction.exporting} onClick={onToggleLock} title={actionTitle(interaction.locked ? "解锁画板" : "锁定画板", settings.lockShortcut)} aria-label={interaction.locked ? "解锁画板" : "锁定画板"}>{interaction.locked ? <Lock size={17} /> : <Unlock size={17} />}</button>
        <button className={`icon-button${fullscreen ? " active" : ""}`} disabled={!renderer} onClick={onToggleFullscreen} title={actionTitle(fullscreen ? "退出全屏" : "全屏显示", settings.fullscreenShortcut)} aria-label={fullscreen ? "退出全屏" : "全屏显示"}>{fullscreen ? <Minimize2 size={17} /> : <Fullscreen size={17} />}</button>
        <button className="icon-button" disabled={!renderer || !interaction.dirty || interaction.saving} onClick={onSave} title="保存画板" aria-label="保存画板"><Save size={17} /></button>
      </div>
    </header>
  );
}
