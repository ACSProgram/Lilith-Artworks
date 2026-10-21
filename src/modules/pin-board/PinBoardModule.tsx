import {
  AlertTriangle,
  LoaderCircle,
  Trash2,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { errorMessage } from "../../shared/errors";
import { ConfirmDialog, type ConfirmView } from "../../shared/ConfirmDialog";
import { PromptDialog } from "../../shared/ui/PromptDialog";
import {
  getPinBoardWindowFullscreen,
  setPinBoardWindowFullscreen,
} from "./api";
import type {
  PinBoardModuleSettings,
  PinBoardSummary,
} from "./types";
import type { PinBoardContextMenuRequest } from "./renderer";
import {
  getSidebarVisible,
  setSidebarVisible,
} from "./session";
import { shortcutCanHandle, shortcutMatches } from "./shortcuts";
import {
  DEFAULT_STROKE_COLOR,
  DEFAULT_STROKE_WIDTH,
  clampStrokeWidth,
} from "./strokeDrawing";
import { usePinBoardController } from "./usePinBoardController";
import { usePendingSettle } from "./usePendingSettle";
import { retryPendingSettle } from "./pendingSettle";
import { PinBoardSidebar } from "./PinBoardSidebar";
import { PinBoardToolbar } from "./PinBoardToolbar";
import { PinBoardCanvasStage } from "./PinBoardCanvasStage";
import { PinBoardContextMenu } from "./PinBoardContextMenu";
import { PinBoardTextDialog, PinBoardExportDialog } from "./PinBoardDialogs";
import { PinBoardTransferProgress } from "./PinBoardTransferProgress";

/**
 * 素材板快捷键默认值，与设置页默认项保持一致。
 * 锁定沿用 Client 的默认键位 Ctrl+R；WebView 整页刷新由应用层的
 * `preventWebViewReload` 只取消默认行为，因此不会与本快捷键冲突。
 */
export const PIN_BOARD_LOCK_SHORTCUT = "CommandOrControl+R";
export const PIN_BOARD_FULLSCREEN_SHORTCUT = "F11";
/**
 * 标注工具条的显示/隐藏快捷键。
 *
 * 工具条收起后自身不可见，任何放在工具条上的按钮都无法再把它叫回来，因此显隐只由这一个
 * 对称的快捷键负责，主工具栏与浮动条本身都不放按钮。
 */
export const PIN_BOARD_TOOLBAR_SHORTCUT = "T";

/**
 * 阵列间距的默认值与上下限，单位是「统一图片高度的比例」。
 *
 * 与 Rust 侧 `pin_board::arrange::{DEFAULT_GAP_RATIO, MAX_GAP_RATIO}` 保持一致：
 * 间距必须随图片大小走，不能按屏幕像素算，否则同一个操作在不同缩放下结果不同。
 * 下限允许 `0`（图片紧贴，用于联络表式的拼接）；上限 20% 之后再大就看不出是阵列了。
 */
export const PIN_BOARD_ARRANGEMENT_GAP_RATIO = 0.06;
export const PIN_BOARD_ARRANGEMENT_GAP_RATIO_MAX = 0.2;

interface PinBoardModuleProps {
  artworkId: string;
  /** 工作区 keep-mounted 语义：非活跃时暂停全局键盘交互与在途纹理任务。 */
  active: boolean;
  settings: PinBoardModuleSettings;
}

/**
 * 渲染器创建失败时的常驻说明。
 *
 * 素材板完全依赖 WebGPU，创建失败后画布在整个进程生命周期内都不会再可用，因此这条说明不能
 * 走 4.5 秒自动清除的状态栏：清空之后画板只剩空白与全部禁用的按钮，用户只会判断成「卡死」。
 * 原始原因保留在文案里，好区分「没有可用适配器」「缺少 BC7 纹理压缩」这几种不同的处置方式。
 */
export function pinBoardUnavailableNotice(reason: string): string {
  return "素材板需要支持 WebGPU 与 BC7 纹理压缩的图形适配器，当前环境不可用"
    + `（${reason}）。请更新显卡驱动、开启虚拟机的 3D 加速，或改在具备可用适配器的环境`
    + "运行后重启应用；其余功能不受影响。";
}

/**
 * 「画板已锁定」标记的显示条件。
 *
 * 渲染器没起来时画板停在默认的锁定状态，但那不是用户锁的；此时显示这个标记会把排查引向
 * 错误方向，所以只显示不可用说明。
 */
export function pinBoardLockIndicatorVisible({ hasView, locked, unavailable }: {
  hasView: boolean;
  locked: boolean;
  unavailable: boolean;
}): boolean {
  return hasView && locked && !unavailable;
}

/**
 * 图层面板的禁用条件。
 *
 * 只有整画布导出期间才置灰：导出渲染的是「点下那一刻的画面」，中途改显隐会让输出与用户
 * 看到的对不上。画板锁定不算——显隐是纯视图状态（不写库、不标 dirty、不进撤销栈），锁定
 * 时改它不会动模型，而「锁住画面只看内容」正是最需要藏掉标注层的场合。
 */
export function pinBoardLayersPanelDisabled({ hasRenderer, exporting }: {
  hasRenderer: boolean;
  exporting: boolean;
}): boolean {
  return !hasRenderer || exporting;
}

function afterLayoutSettles(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

export function PinBoardModule({ artworkId, active, settings }: PinBoardModuleProps) {
  const {
    boards,
    trash,
    view,
    selected,
    status,
    transferProgress,
    clipboardCount,
    interaction,
    renderer,
    setRenderer,
    setStatus,
    setInteraction,
    syncInteraction,
    selectBoard,
    moveBoard,
    createBoard,
    renameBoard,
    trashBoard,
    restoreBoard,
    permanentlyDeleteBoard,
    emptyTrash,
    save,
    toggleLock,
    copyImages,
    copyImageToClipboard,
    pasteItems,
    importImages,
    importDroppedFiles,
    addText,
    setTool,
    commitStroke,
    deleteImages,
    exportImages,
    canvasExportLimit,
    canvasExportPlan,
    canvasExportAutoScale,
    exportCanvas,
    cancelTransfer,
  } = usePinBoardController({ artworkId, active, settings });
  /**
   * 渲染器创建失败的常驻原因。素材板完全依赖 WebGPU，创建失败后画布不会再可用，
   * 因此这条说明独立于 4.5 秒自动清除的 `status` 单独保存。
   */
  const [unavailable, setUnavailable] = useState<string | null>(null);
  /** 切到后台写回的画板改动（切换作品不再阻塞等待仓库写入，进度在这里可见）。 */
  const pendingSettle = usePendingSettle();
  const [sidebar, setSidebar] = useState(getSidebarVisible);
  const [fullscreen, setFullscreen] = useState(false);
  const [textDialogOpen, setTextDialogOpen] = useState(false);
  const [exportDialogOpen, setExportDialogOpen] = useState(false);
  const [textOptions, setTextOptions] = useState({ fontSize: 48, color: "#000000" });
  /** 标注样式：工具栏的取色器与「大小」输入共同决定下一笔的外观（大小同时是橡皮直径）。 */
  const [strokeStyle, setStrokeStyle] = useState({
    color: DEFAULT_STROKE_COLOR,
    width: DEFAULT_STROKE_WIDTH,
  });
  const [strokeWidthText, setStrokeWidthText] = useState(String(DEFAULT_STROKE_WIDTH));
  /** 标注工具条的显隐，只由快捷键 `T` 切换。 */
  const [drawbarVisible, setDrawbarVisible] = useState(true);
  /** 统一的命名输入对话框状态，替代原生 `window.prompt`。 */
  const [labelPrompt, setLabelPrompt] = useState<
    { kind: "create" } | { kind: "rename"; board: PinBoardSummary } | null
  >(null);
  /** 统一的确认对话框状态，替代原生 `window.confirm`。 */
  const [confirmRequest, setConfirmRequest] = useState<
    | { kind: "trash"; board: PinBoardSummary }
    | { kind: "permanent-delete"; board: PinBoardSummary }
    | { kind: "empty-trash" }
    | null
  >(null);
  const [contextMenu, setContextMenu] = useState<PinBoardContextMenuRequest | null>(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const textPlacementRef = useRef<[number, number] | null>(null);
  const fullscreenRef = useRef(false);
  const fullscreenBusyRef = useRef(false);

  const handleRendererContextMenu = useCallback((request: PinBoardContextMenuRequest | null) => {
    setContextMenu(request);
  }, []);

  // 绘制样式推给渲染器：预览层与落库都用渲染器里那一份，避免两处各存一个值。
  useEffect(() => {
    renderer?.setStrokeStyle(strokeStyle.color, strokeStyle.width);
  }, [renderer, strokeStyle]);

  /** 滑条直接写入「大小」（内部即线宽）：非法值忽略，合法值取整并夹到前后端共同接受的范围。 */
  const applyStrokeWidth = useCallback((width: number) => {
    if (!Number.isFinite(width)) return;
    const clamped = clampStrokeWidth(Math.round(width));
    setStrokeWidthText(String(clamped));
    setStrokeStyle((current) => (
      current.width === clamped ? current : { ...current, width: clamped }
    ));
  }, []);

  /** 「大小」输入在失焦或回车时收敛到合法范围，输入过程中允许暂时不合法。 */
  const commitStrokeWidth = useCallback((raw: string) => {
    if (raw.trim() === "" || !Number.isFinite(Number(raw))) {
      setStrokeWidthText(String(strokeStyle.width));
      return;
    }
    applyStrokeWidth(Number(raw));
  }, [applyStrokeWidth, strokeStyle.width]);

  const toggleDrawbar = useCallback(() => {
    if (!renderer || !view || unavailable !== null) return;
    setDrawbarVisible((visible) => !visible);
  }, [renderer, view, unavailable]);

  useEffect(() => {
    if (!active) setContextMenu(null);
  }, [active]);

  const openTextDialog = useCallback(() => {
    const current = renderer;
    if (!current || current.interactionState.locked) return;
    textPlacementRef.current = current.placementPoint;
    setTextDialogOpen(true);
  }, [renderer]);

  const closeTextDialog = useCallback(() => {
    textPlacementRef.current = null;
    setTextDialogOpen(false);
  }, []);

  const submitText = useCallback((text: string, fontSize: number, color: string) => {
    const placement = textPlacementRef.current ?? renderer?.placementPoint ?? [0, 0];
    textPlacementRef.current = null;
    setTextOptions({ fontSize, color });
    setTextDialogOpen(false);
    void addText(text, fontSize, color, placement);
  }, [addText, renderer]);

  const submitLabelPrompt = useCallback((label: string) => {
    const trimmed = label.trim();
    if (!trimmed) return;
    if (labelPrompt?.kind === "rename") {
      void renameBoard(labelPrompt.board.boardId, trimmed);
    } else if (labelPrompt?.kind === "create") {
      void createBoard(trimmed);
    }
    setLabelPrompt(null);
  }, [createBoard, labelPrompt, renameBoard]);

  const submitConfirmRequest = useCallback(() => {
    if (!confirmRequest) return;
    const request = confirmRequest;
    setConfirmRequest(null);
    if (request.kind === "trash") {
      void trashBoard(request.board);
    } else if (request.kind === "permanent-delete") {
      void permanentlyDeleteBoard(request.board.boardId);
    } else {
      void emptyTrash();
    }
  }, [confirmRequest, emptyTrash, permanentlyDeleteBoard, trashBoard]);

  const confirmView: ConfirmView | null = useMemo(() => {
    if (!confirmRequest) return null;
    if (confirmRequest.kind === "trash") {
      return {
        icon: <Trash2 aria-hidden="true" size={18} />,
        eyebrow: "素材板",
        title: "移入回收站",
        subject: `“${confirmRequest.board.name}”`,
        description: "画板将移入素材板回收站，可随时恢复或永久删除。",
        action: "移入回收站",
      };
    }
    if (confirmRequest.kind === "permanent-delete") {
      return {
        icon: <Trash2 aria-hidden="true" size={18} />,
        eyebrow: "素材板回收站",
        title: "永久删除画板",
        subject: `“${confirmRequest.board.name}”`,
        description: "画板及其全部图片文件将被永久删除，此操作无法撤销。",
        detail: null,
        action: "永久删除",
        danger: true,
      };
    }
    return {
      icon: <Trash2 aria-hidden="true" size={18} />,
      eyebrow: "素材板回收站",
      title: "清空回收站",
      subject: `${trash.length} 块画板`,
      description: "回收站中的所有画板及其图片文件将被永久删除，此操作无法撤销。",
      detail: null,
      action: "拖动到底清空回收站",
      danger: true,
      emphasis: true,
      slideToConfirm: true,
    };
  }, [confirmRequest, trash.length]);

  const title = useMemo(() => {
    if (!boards || selected === null) return "素材板";
    return boards.find((board) => board.boardId === selected)?.name ?? `画板 ${selected}`;
  }, [boards, selected]);

  const run = (action: () => void) => {
    action();
    syncInteraction();
  };

  const toggleSidebar = () => {
    setSidebar((current) => {
      setSidebarVisible(!current);
      return !current;
    });
  };

  const toggleFullscreen = useCallback(async () => {
    if (!workspaceRef.current || fullscreenBusyRef.current) return;
    const previous = fullscreenRef.current;
    const next = !previous;
    const current = renderer;
    fullscreenBusyRef.current = true;
    fullscreenRef.current = next;
    current?.finishActiveInteraction();
    current?.beginViewportResize();
    setContextMenu(null);
    textPlacementRef.current = null;
    setTextDialogOpen(false);
    try {
      setFullscreen(next);
      await setPinBoardWindowFullscreen(next);
      await afterLayoutSettles();
    } catch (error) {
      fullscreenRef.current = previous;
      setFullscreen(previous);
      setStatus(`无法切换全屏：${errorMessage(error)}`);
    } finally {
      current?.endViewportResize();
      fullscreenBusyRef.current = false;
    }
  }, [renderer, setStatus]);

  useEffect(() => {
    if (!active && fullscreenRef.current) void toggleFullscreen();
  }, [active, toggleFullscreen]);

  useEffect(() => {
    // 原生全屏属于窗口状态而非页面状态：页面重新加载后新页面并不知道窗口仍在
    // 全屏。这里与原生状态对齐，保证工具栏、Escape 和全屏快捷键都能正常退出。
    let cancelled = false;
    void getPinBoardWindowFullscreen()
      .then((isFullscreen) => {
        if (cancelled || !isFullscreen || fullscreenRef.current) return;
        fullscreenRef.current = true;
        setFullscreen(true);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!active || !shortcutCanHandle(event)) return;
      // 单键开关：带修饰键的组合留给应用与系统，不与工具键位抢。
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      if (event.key.toLowerCase() !== PIN_BOARD_TOOLBAR_SHORTCUT.toLowerCase()) return;
      if (!renderer || !view || unavailable !== null) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      toggleDrawbar();
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, renderer, view, unavailable, toggleDrawbar]);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if (!shortcutCanHandle(event) || !(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "c") {
        event.preventDefault();
        event.stopImmediatePropagation();
        copyImages();
      } else if (key === "v" && clipboardCount > 0) {
        event.preventDefault();
        event.stopImmediatePropagation();
        pasteItems();
      }
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, clipboardCount, copyImages, pasteItems]);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if (!shortcutCanHandle(event)) return;
      const fullscreenRequested = shortcutMatches(event, settings.fullscreenShortcut)
        || (event.key === "Escape" && fullscreenRef.current);
      const lockRequested = shortcutMatches(event, settings.lockShortcut);
      if (!fullscreenRequested && !lockRequested) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (fullscreenRequested) void toggleFullscreen();
      else toggleLock();
    };
    window.addEventListener("keydown", keyDown, true);
    return () => {
      window.removeEventListener("keydown", keyDown, true);
    };
  }, [active, settings.fullscreenShortcut, settings.lockShortcut, toggleFullscreen, toggleLock]);

  useEffect(() => () => {
    if (fullscreenRef.current) void setPinBoardWindowFullscreen(false).catch(() => undefined);
    fullscreenRef.current = false;
  }, []);

  const transferBusy = transferProgress !== null;

  return (
    <section className={`pin-board-module${sidebar ? "" : " sidebar-collapsed"}`}>
      <PinBoardSidebar
        active={active}
        boards={boards}
        trash={trash}
        selected={selected}
        exporting={interaction.exporting}
        transferBusy={transferBusy}
        onDismissCanvasMenu={() => setContextMenu(null)}
        onCreate={() => setLabelPrompt({ kind: "create" })}
        onSelect={(board) => void selectBoard(board)}
        onMoveBoard={moveBoard}
        onRename={(board) => setLabelPrompt({ kind: "rename", board })}
        onTrash={(board) => setConfirmRequest({ kind: "trash", board })}
        onRestore={(boardId) => void restoreBoard(boardId)}
        onPermanentDelete={(board) => setConfirmRequest({ kind: "permanent-delete", board })}
        onEmptyTrash={() => setConfirmRequest({ kind: "empty-trash" })}
      />
      <div
        ref={workspaceRef}
        className={`pin-board-workspace${fullscreen ? " fullscreen" : ""}`}
      >
        <PinBoardToolbar
          sidebar={sidebar}
          title={title}
          view={view}
          renderer={renderer}
          interaction={interaction}
          clipboardCount={clipboardCount}
          transferBusy={transferBusy}
          fullscreen={fullscreen}
          settings={settings}
          run={run}
          onToggleSidebar={toggleSidebar}
          onCopy={copyImages}
          onCopyClipboard={() => void copyImageToClipboard()}
          onPaste={pasteItems}
          onImport={() => void importImages()}
          onAddText={openTextDialog}
          onExport={() => void exportImages()}
          onExportCanvas={() => setExportDialogOpen(true)}
          onToggleLock={toggleLock}
          onToggleFullscreen={() => void toggleFullscreen()}
          onSave={() => void save()}
        />
        {status && (
          <div className="pin-board-status" aria-live="polite" title={status}>
            <AlertTriangle size={16} />
            <span>{status}</span>
          </div>
        )}
        {pendingSettle && (
          <div
            className={`pin-board-status pin-board-settle${pendingSettle.failure ? " failed" : ""}`}
            role="status"
            title={pendingSettle.failure
              ? `${pendingSettle.failure.message}（画板 ${pendingSettle.failure.boardId}）`
              : undefined}
          >
            {pendingSettle.failure
              ? <AlertTriangle size={16} />
              : <LoaderCircle className="spin" size={16} />}
            <span>
              {pendingSettle.failure
                ? `${pendingSettle.count} 块画板的改动还没写进仓库：${pendingSettle.failure.message}`
                : `正在后台保存 ${pendingSettle.count} 块画板的改动`}
            </span>
            {pendingSettle.failure && (
              <button type="button" className="pin-board-settle-retry" onClick={retryPendingSettle}>
                重试
              </button>
            )}
          </div>
        )}
        {transferProgress && (
          <PinBoardTransferProgress progress={transferProgress} onCancel={cancelTransfer} />
        )}
        <PinBoardCanvasStage
          active={active}
          artworkId={artworkId}
          view={view}
          boards={boards}
          renderer={renderer}
          interaction={interaction}
          settings={settings}
          unavailable={unavailable}
          unavailableNotice={unavailable === null ? "" : pinBoardUnavailableNotice(unavailable)}
          lockIndicatorVisible={pinBoardLockIndicatorVisible({
            hasView: view !== null,
            locked: interaction.locked,
            unavailable: unavailable !== null,
          })}
          layersDisabled={pinBoardLayersPanelDisabled({
            hasRenderer: renderer !== null,
            exporting: interaction.exporting,
          })}
          drawbarVisible={drawbarVisible}
          strokeStyle={strokeStyle}
          strokeWidthText={strokeWidthText}
          setStatus={setStatus}
          onUnavailable={setUnavailable}
          onRenderer={setRenderer}
          onState={setInteraction}
          onContextMenu={handleRendererContextMenu}
          onStrokeCommit={commitStroke}
          onImportDropped={importDroppedFiles}
          onSelectTool={setTool}
          onToggleLayer={(layer, visible) => run(() => renderer?.setLayerVisible(layer, visible))}
          onStrokeColorChange={(color) => setStrokeStyle((current) => ({ ...current, color }))}
          onStrokeWidthInput={applyStrokeWidth}
          onStrokeWidthTextChange={setStrokeWidthText}
          onStrokeWidthCommit={commitStrokeWidth}
        />
        {textDialogOpen && (
          <PinBoardTextDialog
            initialFontSize={textOptions.fontSize}
            initialColor={textOptions.color}
            onSubmit={submitText}
            onClose={closeTextDialog}
          />
        )}
        {exportDialogOpen && (
          <PinBoardExportDialog
            limit={canvasExportLimit}
            boardName={title}
            planFor={canvasExportPlan}
            autoScale={canvasExportAutoScale}
            onStart={(request) => {
              setExportDialogOpen(false);
              void exportCanvas(request);
            }}
            onClose={() => setExportDialogOpen(false)}
          />
        )}
        {labelPrompt && (
          <PromptDialog
            title={labelPrompt.kind === "rename" ? "重命名画板" : "新建画板"}
            label="画板名称"
            initialValue={labelPrompt.kind === "rename" ? labelPrompt.board.name : ""}
            confirmLabel={labelPrompt.kind === "rename" ? "重命名" : "创建"}
            onConfirm={submitLabelPrompt}
            onCancel={() => setLabelPrompt(null)}
          />
        )}
        {confirmView && (
          <ConfirmDialog
            view={confirmView}
            busy={false}
            onCancel={() => setConfirmRequest(null)}
            onConfirm={submitConfirmRequest}
          />
        )}
        {contextMenu && renderer && (
          <PinBoardContextMenu
            request={contextMenu}
            renderer={renderer}
            fullscreen={fullscreen}
            canPaste={clipboardCount > 0}
            onCopy={copyImages}
            onCopyClipboard={() => void copyImageToClipboard()}
            onPaste={pasteItems}
            onImport={() => void importImages()}
            onAddText={openTextDialog}
            onExport={() => void exportImages()}
            onExportCanvas={() => {
              setContextMenu(null);
              setExportDialogOpen(true);
            }}
            onDelete={deleteImages}
            onToggleFullscreen={() => void toggleFullscreen()}
            onClose={() => setContextMenu(null)}
          />
        )}
      </div>
    </section>
  );
}
