import {
  Check, CircleDot, Download, GitBranch, GitFork, LoaderCircle,
  ListCollapse, Pencil, Play, Trash2, X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { formatBytes } from "../../shared/format";
import { useMenuPlacement } from "../../shared/useMenuPlacement";
import {
  BranchScheduleStatus, BranchSettings, chooseRestoreOutput, ConfirmDialog, EditNodeDialog, ForkDialog,
} from "./HistoryControls";
import type { ConfirmRequest } from "./HistoryControls";
import { HistoryBranchView, HistoryInspector } from "./HistoryBranchView";
import { HistoryOverview } from "./HistoryOverview";
import {
  branchesContainingNode, buildBranchLine, canCompact, HISTORY_NODE_WIDTH_MAX,
  HISTORY_NODE_WIDTH_MIN, isForcedCheckpoint, isRenamedNode, needsCompactSecondConfirm,
  suggestedRestorePath, verifyIssueLabel, verifyIssueNotice,
  type HistoryViewMode,
} from "./historyModel";
import { HoverPreview } from "./NodePreview";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";
import { useCompactSelection } from "./useCompactSelection";
import { useHistoryController } from "./useHistoryController";
import { useNodePreview } from "./useNodePreview";

interface HistoryModuleProps {
  artworkId: string;
  selectedBranchId: string | null;
  refreshVersion?: number;
  onSelectBranch: (branchId: string) => void;
  onHistoryChanged: (history: ArtworkHistory) => void;
  onError: (message: string | null) => void;
}

const MINDMAP_NODE_WIDTH_KEY = "lilith-artworks.history-node-min-width-v1";

function loadMindmapNodeWidth(): number {
  const stored = Number(window.localStorage.getItem(MINDMAP_NODE_WIDTH_KEY));
  return Number.isFinite(stored)
    ? Math.min(HISTORY_NODE_WIDTH_MAX, Math.max(HISTORY_NODE_WIDTH_MIN, stored))
    : 280;
}

export function HistoryModule({ artworkId, selectedBranchId, refreshVersion = 0, onSelectBranch, onHistoryChanged, onError }: HistoryModuleProps) {
  const {
    history, loading, busy, runtime, visibleRuntime, saveBranch, commitBranch,
    restoreNode, compactNodes, deleteBranch, reverifyBranch, deleteSubtree, setCheckpoint,
    forkBranch, renameNode, readNodeFormat, readNodeThumbnail, refreshNodeThumbnail,
    cancelOperation,
  } = useHistoryController({ artworkId, refreshVersion, onHistoryChanged, onError });
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [commitNote, setCommitNote] = useState("");
  const [commitFeedback, setCommitFeedback] = useState<string | null>(null);
  const [view, setView] = useState<HistoryViewMode>("overview");
  const [forkOpen, setForkOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [actionNode, setActionNode] = useState<HistoryNode | null>(null);
  const [context, setContext] = useState<{ x: number; y: number } | null>(null);
  const [confirmRequest, setConfirmRequest] = useState<ConfirmRequest | null>(null);
  const [mindmapMode, setMindmapMode] = useState<"compact" | "timeline">("compact");
  const [mindmapNodeWidth, setMindmapNodeWidth] = useState(loadMindmapNodeWidth);
  const nodeElements = useRef(new Map<string, HTMLButtonElement>());
  const contextPlacement = useMenuPlacement(context);

  const selectedBranch = history?.branches.find((branch) => branch.id === selectedBranchId) ?? null;
  const selectedNode = history?.nodes.find((node) => node.id === selectedNodeId) ?? null;
  const branchLine = useMemo(() => selectedBranch && history
    ? buildBranchLine(history.nodes, selectedBranch.headHistoryId) : [], [history, selectedBranch]);
  const branchNodeIds = useMemo(() => new Set(branchLine.map((node) => node.id)), [branchLine]);
  const {
    compactMode,
    compactSelection,
    beginCompactMode,
    exitCompactMode,
    toggleCompactNode,
    orderedCompactSelection,
  } = useCompactSelection(history, branchLine);
  const {
    nodeFormat,
    nodeThumbnail,
    hover,
    hoverRefreshingNodeId,
    handleNodeEnter,
    handleNodeMove,
    scheduleHoverClose,
    cancelHoverClose,
    closeHover,
    refreshThumbnail,
  } = useNodePreview({
    artworkId,
    view,
    history,
    selectedNode,
    readNodeFormat,
    readNodeThumbnail,
    refreshNodeThumbnail,
  });
  const hoverNode = hover ? history?.nodes.find((node) => node.id === hover.nodeId) ?? null : null;

  useEffect(() => {
    setSelectedNodeId(null);
    setCommitNote("");
    setCommitFeedback(null);
    setView("overview");
    setForkOpen(false);
    setEditOpen(false);
    setActionNode(null);
    setContext(null);
    setConfirmRequest(null);
    exitCompactMode();
  }, [artworkId, exitCompactMode]);

  useEffect(() => setCommitFeedback(null), [selectedBranchId]);

  useEffect(() => {
    setSelectedNodeId((current) => current && history?.nodes.some((node) => node.id === current)
      ? current : null);
  }, [history]);

  useEffect(() => {
    window.localStorage.setItem(MINDMAP_NODE_WIDTH_KEY, String(mindmapNodeWidth));
  }, [mindmapNodeWidth]);

  useEffect(() => {
    if (!history || (selectedBranchId && history.branches.some((branch) => branch.id === selectedBranchId))) return;
    const fallback = history.branches[0]?.id;
    if (fallback) onSelectBranch(fallback);
  }, [history, onSelectBranch, selectedBranchId]);

  useEffect(() => {
    const close = () => setContext(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, []);

  const commit = async () => {
    if (!selectedBranch) return;
    setCommitFeedback(null);
    const result = await commitBranch(selectedBranch.id, commitNote.trim());
    if (!result) return;
    setCommitNote("");
    if (result.historyId) setSelectedNodeId(result.historyId);
    setCommitFeedback(result.created
      ? "提交成功，已创建新的历史节点"
      : "检查完成：当前内容已经是最新备份（可能已由自动备份记录）");
  };

  const beginRestore = async (node: HistoryNode) => {
    const branch = history?.branches.find((item) => item.id === node.createdOnBranchId) ?? selectedBranch;
    const outputPath = await chooseRestoreOutput(suggestedRestorePath(node, branch));
    if (!outputPath) return;
    await restoreNode(node.id, outputPath);
  };

  const openContext = (event: MouseEvent, node: HistoryNode) => {
    event.preventDefault();
    event.stopPropagation();
    closeHover();
    setSelectedNodeId(node.id);
    setActionNode(node);
    setContext({ x: event.clientX, y: event.clientY });
  };

  const enterBranch = (branch: ArtworkBranch) => {
    onSelectBranch(branch.id);
    setView("branch");
    exitCompactMode();
    setContext(null);
  };

  const startCompactMode = () => {
    beginCompactMode();
    setSelectedNodeId(null);
  };

  const runCompact = async (ordered: HistoryNode[]) => {
    if (await compactNodes(ordered)) {
      exitCompactMode();
    }
  };

  const executeConfirmed = async () => {
    const request = confirmRequest;
    if (!request || !history) return;
    if (request.kind === "compact") {
      const ordered = orderedCompactSelection();
      // 主动提交或已被人工改名的节点承载明确意图，删除前再插一道更强的确认。
      const important = ordered.filter(needsCompactSecondConfirm);
      if (important.length > 0) {
        setConfirmRequest({
          kind: "compact-warning",
          selectedCount: ordered.length,
          manualCount: important.filter((node) => node.commitKind !== "automatic").length,
          renamedCount: important.filter(isRenamedNode).length,
        });
        return;
      }
      await runCompact(ordered);
    } else if (request.kind === "compact-warning") {
      await runCompact(orderedCompactSelection());
    } else if (request.kind === "delete-branch") {
      await deleteBranch(request.branch.id);
    } else if (request.kind === "delete-nodes") {
      await deleteSubtree(request.node.id, request.branch.id);
    } else {
      await setCheckpoint(request.node.id, request.enable);
    }
    setConfirmRequest(null);
    setActionNode(null);
    setContext(null);
  };

  if (loading || !history) {
    return <div className="history-loading"><LoaderCircle className="spin" size={18} />读取分支历史</div>;
  }

  const jumpToNode = (nodeId: string) => {
    setSelectedNodeId(nodeId);
    const element = nodeElements.current.get(nodeId);
    if (!element) return;
    element.scrollIntoView({ behavior: "smooth", block: "center", inline: "center" });
    element.focus({ preventScroll: true });
  };

  const nodeCard = (node: HistoryNode) => {
    const heads = history.branches.filter((branch) => branch.headHistoryId === node.id);
    const publishedCount = history.branches.filter((branch) => branch.headHistoryId === node.id).reduce((sum, branch) => sum + branch.publishedCount, 0);
    const forks = history.branches.filter((branch) =>
      branch.createdFromHistoryId === node.id && branch.headHistoryId !== node.id);
    const selectable = compactMode && branchNodeIds.has(node.id) && canCompact(node, history);
    const selectedForCompact = compactSelection.has(node.id);
    const selected = selectedNodeId === node.id;
    const onSelectedBranch = Boolean(selectedBranch && branchNodeIds.has(node.id));
    // 问题只讲后果：文件不在了、读不出来了，或者被后续版本挡住而没有结论。
    // 归类由后端给出，这里不再解析错误文本，也不用「校验失败」这类术语。
    const verifyNotice = node.verifyIssue ? verifyIssueNotice(node.verifyIssue) : null;
    return <button
      key={node.id}
      ref={(element) => {
        if (element) nodeElements.current.set(node.id, element);
        else nodeElements.current.delete(node.id);
      }}
      className={`history-node-card${selected ? " selected" : ""}${onSelectedBranch ? " on-selected-branch" : ""}${selectable ? " compactable" : ""}${selectedForCompact ? " compact-selected" : ""}${node.verifyIssue ? ` verify-${node.verifyIssue}` : ""}`}
      type="button"
      title={verifyNotice ?? undefined}
      aria-pressed={selected}
      onClick={() => {
        if (compactMode) {
          if (!selectable) return;
          toggleCompactNode(node.id);
        } else {
          setSelectedNodeId(node.id);
        }
      }}
      onDoubleClick={() => {
        if (compactMode || view !== "overview") return;
        const containingBranches = branchesContainingNode(history, node.id);
        if (containingBranches.length === 1) {
          onSelectBranch(containingBranches[0].id);
          setSelectedNodeId(node.id);
          setContext(null);
        }
      }}
      onContextMenu={(event) => openContext(event, node)}
      onMouseEnter={(event) => handleNodeEnter(event, node)}
      onMouseMove={(event) => handleNodeMove(event, node)}
      onMouseLeave={scheduleHoverClose}
    >
      <CircleDot size={16} />
      <span className="history-node-copy">
        <strong>{node.title}</strong>
        {node.note && node.note !== node.title && <small>{node.note}</small>}
        <small>{new Date(node.createdMs).toLocaleString()} · {formatBytes(node.logicalSize)} · Chunk {formatBytes(node.chunkFileSize)}</small>
        <code>{node.sha256}</code>
      </span>
      <span className="history-labels">
        {node.verifyIssue && <i className={`verify-issue-label ${node.verifyIssue}`}>{verifyIssueLabel(node.verifyIssue)}</i>}
        {heads.map((branch) => <i className={branch.id === selectedBranchId ? "current-branch-label" : undefined} key={branch.id}>{branch.title} HEAD</i>)}
        {forks.map((branch) => <i className="fork-label" key={branch.id}>{branch.title} 分支起点</i>)}
        {node.isCheckpoint && <i>检查点</i>}
        {publishedCount > 0 && <i className="published-label">已发布 {publishedCount}</i>}
        {selectedForCompact && <i className="selected-label">待精简</i>}
      </span>
    </button>;
  };

  const contextBranches = actionNode ? branchesContainingNode(history, actionNode.id) : [];
  const uniqueContextBranch = contextBranches.length === 1 ? contextBranches[0] : null;
  const childBranches = actionNode
    ? history.branches.filter((branch) => branch.createdFromHistoryId === actionNode.id)
    : [];

  return <div className={`history-workspace${compactMode ? " compacting" : ""}`}>
    <header className="history-header">
      <div className="history-title"><span>Artwork 历史</span><h1>{history.artworkTitle}</h1></div>
      <div className="history-header-actions">
        {visibleRuntime.busy && <OperationProgress runtime={visibleRuntime} onCancel={cancelOperation} />}
        <div className="segmented-control" aria-label="历史视图">
          <button className={view === "overview" ? "active" : ""} onClick={() => { setView("overview"); exitCompactMode(); }}>总览</button>
          <button className={view === "branch" ? "active" : ""} onClick={() => setView("branch")}>当前分支</button>
        </div>
        <select value={selectedBranchId ?? ""} onChange={(event) => {
          onSelectBranch(event.target.value);
          exitCompactMode();
          if (view === "branch") setSelectedNodeId(null);
        }}>
          {history.branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}
        </select>
      </div>
    </header>

    {selectedBranch && <section className="branch-band">
      <BranchSettings branch={selectedBranch} disabled={busy || runtime.busy} onSave={saveBranch} />
      <div className="branch-status-actions">
        <BranchScheduleStatus branch={selectedBranch} onReverify={reverifyBranch} />
        {view === "branch" && selectedBranch.createdFromHistoryId && <button className="danger-button" type="button" disabled={busy || runtime.busy} onClick={() => {
          const origin = history.nodes.find((node) => node.id === selectedBranch.createdFromHistoryId);
          if (origin) setConfirmRequest({ kind: "delete-branch", node: origin, branch: selectedBranch });
        }}><Trash2 size={15} />删除分支</button>}
      </div>
    </section>}

    <section className="history-toolbar">
      <div className="commit-block">
        <label htmlFor="commit-note">主动提交</label>
        <div className="commit-control">
          <input id="commit-note" value={commitNote} maxLength={500} placeholder="添加本次提交备注（可选）" disabled={!selectedBranch || !selectedBranch.sourcePath || selectedBranch.finalArtifactLocked || busy || runtime.busy} onChange={(event) => setCommitNote(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void commit(); }} />
          <button className="primary-button" type="button" disabled={!selectedBranch || !selectedBranch.sourcePath || selectedBranch.finalArtifactLocked || busy || runtime.busy} onClick={() => void commit()}>{visibleRuntime.busy && visibleRuntime.activeBranchId === selectedBranch?.id ? <LoaderCircle className="spin" size={16} /> : <Play size={16} />}提交</button>
        </div>
        {selectedBranch && !selectedBranch.sourcePath && <span className="commit-feedback muted" role="status">该分支未选择工作文件，自动备份与主动提交不可用。</span>}
        {commitFeedback && <span className="commit-feedback" role="status"><Check size={13} />{commitFeedback}</span>}
      </div>
      {!compactMode && <button className="secondary-button" type="button" disabled={busy || runtime.busy || !selectedBranch || !branchLine.some((node) => canCompact(node, history))} onClick={startCompactMode}><ListCollapse aria-hidden="true" size={16} />精简当前分支</button>}
    </section>

    {compactMode && <div className="compact-mode-bar">
      <ListCollapse aria-hidden="true" size={16} /><div><strong>精简 {selectedBranch?.title}</strong><span>选择当前分支上的普通中间节点</span></div>
      <strong className="compact-count">{compactSelection.size} 个已选</strong>
      <button className="danger-button" type="button" disabled={!compactSelection.size || busy || runtime.busy} onClick={() => {
        setConfirmRequest({ kind: "compact", selectedCount: compactSelection.size });
      }}><Trash2 size={15} />删除节点</button>
      <button className="text-button" type="button" onClick={exitCompactMode}>退出</button>
    </div>}

    <div className={`history-main${view === "overview" && mindmapMode === "timeline" ? " with-timeline" : ""}`}>
      {view === "overview"
        ? <HistoryOverview
          history={history}
          selectedNodeId={selectedNodeId}
          mindmapMode={mindmapMode}
          onMindmapModeChange={setMindmapMode}
          mindmapNodeWidth={mindmapNodeWidth}
          onMindmapNodeWidthChange={setMindmapNodeWidth}
          renderNode={nodeCard}
          onJump={jumpToNode}
        />
        : <HistoryBranchView
          history={history}
          selectedBranch={selectedBranch}
          branchLine={branchLine}
          renderNode={nodeCard}
        />}
      <HistoryInspector
        selectedNode={selectedNode}
        nodeFormat={nodeFormat}
        nodeThumbnail={nodeThumbnail}
        disabled={busy || runtime.busy}
        onRefreshThumbnail={refreshThumbnail}
      />
    </div>

    {hoverNode && hover && (hover.value.dataUrl || hover.value.supported) && <HoverPreview
      anchor={hover.anchor}
      title={hoverNode.title}
      preview={hover.value}
      refreshing={hoverRefreshingNodeId === hover.nodeId}
      disabled={busy || runtime.busy}
      onRefresh={() => refreshThumbnail(hoverNode)}
      onEnter={cancelHoverClose}
      onLeave={scheduleHoverClose}
    />}

    {context && actionNode && !compactMode && <div className="context-menu history-context" ref={contextPlacement.ref} style={contextPlacement.style} onClick={(event) => event.stopPropagation()}>
      {uniqueContextBranch && <button onClick={() => enterBranch(uniqueContextBranch)}><GitBranch size={15} />进入分支：{uniqueContextBranch.title}</button>}
      {childBranches.map((branch) => <button key={branch.id} className="danger" onClick={() => { setConfirmRequest({ kind: "delete-branch", node: actionNode, branch }); setContext(null); }}><Trash2 size={15} />删除分支：{branch.title}</button>)}
      {(uniqueContextBranch || childBranches.length > 0) && <div className="context-separator" />}
      <button onClick={() => { setEditOpen(true); setContext(null); }}><Pencil size={15} />编辑节点名称</button>
      <button onClick={() => { setForkOpen(true); setContext(null); }}><GitFork size={15} />从此处创建分支</button>
      <button onClick={() => { setContext(null); void beginRestore(actionNode); }}><Download size={15} />恢复到文件</button>
      {actionNode.isCheckpoint && isForcedCheckpoint(actionNode, history)
        ? <button disabled title="分支 head、分支起点和分叉点必须保留"><CircleDot size={15} />强制检查点</button>
        : <button onClick={() => { setConfirmRequest({ kind: "checkpoint", node: actionNode, enable: !actionNode.isCheckpoint }); setContext(null); }}><CircleDot size={15} />{actionNode.isCheckpoint ? "取消检查点" : "设为检查点"}</button>}
      {view === "branch" && selectedBranch && branchNodeIds.has(actionNode.id) && <><div className="context-separator" /><button className="danger" onClick={() => {
        const descendants = branchLine.slice(branchLine.findIndex((node) => node.id === actionNode.id));
        setConfirmRequest({ kind: "delete-nodes", node: actionNode, branch: selectedBranch, descendantCount: descendants.length });
        setContext(null);
      }}><Trash2 size={15} />删除此节点及后续历史</button></>}
    </div>}

    {forkOpen && actionNode && <ForkDialog node={actionNode} busy={busy} onClose={() => setForkOpen(false)} onSubmit={async (title, sourcePath) => {
      if (await forkBranch({ artworkId, fromHistoryId: actionNode.id, title, sourcePath })) setForkOpen(false);
    }} />}
    {editOpen && actionNode && <EditNodeDialog initialValue={actionNode.title} busy={busy} onClose={() => setEditOpen(false)} onSubmit={async (title) => {
      if (await renameNode({ historyId: actionNode.id, title })) setEditOpen(false);
    }} />}
    {confirmRequest && <ConfirmDialog
      request={confirmRequest}
      busy={busy || runtime.busy}
      // 只有本弹窗自己的操作真的在跑时才提供「取消操作」：runtime.busy 也可能来自
      // 后台自动备份或空闲校验，那不是这个操作。
      onCancelOperation={busy ? () => void cancelOperation() : undefined}
      onClose={() => setConfirmRequest(null)}
      onConfirm={() => void executeConfirmed()}
    />}
  </div>;
}

function OperationProgress({ runtime, onCancel }: { runtime: BackupRuntimeStatus; onCancel: () => void }) {
  const hasTotal = runtime.progressTotal > 0;
  const percent = hasTotal
    ? Math.round(runtime.progressCurrent / runtime.progressTotal * 100)
    : 0;
  return <div className="operation-progress" role="status">
    <div className="operation-progress-copy"><LoaderCircle className="spin" size={15} /><span>{runtime.progressLabel ?? "正在处理"}</span><strong>{hasTotal ? `${percent}%` : "准备中"}</strong></div>
    <progress value={runtime.progressCurrent} max={Math.max(runtime.progressTotal, 1)} />
    <button className="icon-button" type="button" title="取消当前操作" onClick={onCancel}><X size={15} /></button>
  </div>;
}
