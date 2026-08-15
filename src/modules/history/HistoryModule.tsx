import {
  Ban, Check, CircleDot, Download, GitBranch, GitFork, LoaderCircle,
  MoreHorizontal, Pencil, Play, Trash2, X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent } from "react";
import { formatBytes } from "../../shared/format";
import {
  BranchSettings, chooseRestoreOutput, ConfirmDialog, EditNodeDialog, ForkDialog,
} from "./HistoryControls";
import type { ConfirmRequest } from "./HistoryControls";
import { HistoryTimeline, Mindmap } from "./HistoryGraph";
import {
  branchesContainingNode, buildBranchLine, buildHistoryTree, canCompact,
  isForcedCheckpoint, suggestedRestorePath,
} from "./historyModel";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";
import { useHistoryController } from "./useHistoryController";

interface HistoryModuleProps {
  artworkId: string;
  selectedBranchId: string | null;
  refreshVersion?: number;
  onSelectBranch: (branchId: string) => void;
  onHistoryChanged: (history: ArtworkHistory) => void;
  onError: (message: string | null) => void;
}

type ViewMode = "overview" | "branch";

const MINDMAP_NODE_WIDTH_KEY = "lilith-artworks.history-node-min-width-v1";
const MIN_NODE_WIDTH = 220;
const MAX_NODE_WIDTH = 420;

function loadMindmapNodeWidth(): number {
  const stored = Number(window.localStorage.getItem(MINDMAP_NODE_WIDTH_KEY));
  return Number.isFinite(stored)
    ? Math.min(MAX_NODE_WIDTH, Math.max(MIN_NODE_WIDTH, stored))
    : 280;
}

export function HistoryModule({ artworkId, selectedBranchId, refreshVersion = 0, onSelectBranch, onHistoryChanged, onError }: HistoryModuleProps) {
  const {
    history, loading, busy, runtime, visibleRuntime, saveBranch, commitBranch,
    restoreNode, compactNodes, deleteBranch, deleteSubtree, setCheckpoint,
    forkBranch, renameNode, cancelOperation,
  } = useHistoryController({ artworkId, refreshVersion, onHistoryChanged, onError });
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [commitNote, setCommitNote] = useState("");
  const [commitFeedback, setCommitFeedback] = useState<string | null>(null);
  const [view, setView] = useState<ViewMode>("overview");
  const [forkOpen, setForkOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [actionNode, setActionNode] = useState<HistoryNode | null>(null);
  const [context, setContext] = useState<{ x: number; y: number } | null>(null);
  const [confirmRequest, setConfirmRequest] = useState<ConfirmRequest | null>(null);
  const [compactMode, setCompactMode] = useState(false);
  const [compactSelection, setCompactSelection] = useState<Set<string>>(new Set());
  const [mindmapMode, setMindmapMode] = useState<"compact" | "timeline">("compact");
  const [mindmapNodeWidth, setMindmapNodeWidth] = useState(loadMindmapNodeWidth);
  const nodeElements = useRef(new Map<string, HTMLButtonElement>());

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
    setCompactMode(false);
    setCompactSelection(new Set());
  }, [artworkId]);

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

  const selectedBranch = history?.branches.find((branch) => branch.id === selectedBranchId) ?? null;
  const selectedNode = history?.nodes.find((node) => node.id === selectedNodeId) ?? null;
  const roots = useMemo(() => buildHistoryTree(history?.nodes ?? []), [history?.nodes]);
  const branchLine = useMemo(() => selectedBranch && history
    ? buildBranchLine(history.nodes, selectedBranch.headHistoryId) : [], [history, selectedBranch]);
  const branchNodeIds = useMemo(() => new Set(branchLine.map((node) => node.id)), [branchLine]);
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
    setSelectedNodeId(node.id);
    setActionNode(node);
    setContext({ x: event.clientX, y: event.clientY });
  };

  const enterBranch = (branch: ArtworkBranch) => {
    onSelectBranch(branch.id);
    setView("branch");
    setCompactMode(false);
    setCompactSelection(new Set());
    setContext(null);
  };

  const beginCompactMode = () => {
    setCompactSelection(new Set());
    setCompactMode(true);
    setSelectedNodeId(null);
  };

  const executeConfirmed = async () => {
    const request = confirmRequest;
    if (!request || !history) return;
    if (request.kind === "compact") {
      const ordered = branchLine
        .filter((node) => compactSelection.has(node.id))
        .reverse();
      if (await compactNodes(ordered)) {
        setCompactMode(false);
        setCompactSelection(new Set());
      }
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
    const selectable = compactMode && view === "branch" && branchNodeIds.has(node.id) && canCompact(node, history);
    const selectedForCompact = compactSelection.has(node.id);
    const selected = selectedNodeId === node.id;
    const onSelectedBranch = Boolean(selectedBranch && branchNodeIds.has(node.id));
    return <button
      key={node.id}
      ref={(element) => {
        if (element) nodeElements.current.set(node.id, element);
        else nodeElements.current.delete(node.id);
      }}
      className={`history-node-card${selected ? " selected" : ""}${onSelectedBranch ? " on-selected-branch" : ""}${selectable ? " compactable" : ""}${selectedForCompact ? " compact-selected" : ""}`}
      type="button"
      aria-pressed={selected}
      onClick={() => {
        if (compactMode) {
          if (!selectable) return;
          setCompactSelection((current) => {
            const next = new Set(current);
            if (next.has(node.id)) next.delete(node.id); else next.add(node.id);
            return next;
          });
        } else {
          setSelectedNodeId(node.id);
        }
      }}
      onContextMenu={(event) => openContext(event, node)}
    >
      <CircleDot size={16} />
      <span className="history-node-copy">
        <strong>{node.title}</strong>
        {node.note && node.note !== node.title && <small>{node.note}</small>}
        <small>{new Date(node.createdMs).toLocaleString()} · {formatBytes(node.logicalSize)} · Chunk {formatBytes(node.chunkFileSize)}</small>
        <code>{node.sha256}</code>
      </span>
      <span className="history-labels">
        {heads.map((branch) => <i className={branch.id === selectedBranchId ? "current-branch-label" : undefined} key={branch.id}>{branch.title} HEAD</i>)}
        {forks.map((branch) => <i className="fork-label" key={branch.id}>{branch.title} fork</i>)}
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
          <button className={view === "overview" ? "active" : ""} onClick={() => { setView("overview"); setCompactMode(false); setCompactSelection(new Set()); }}>总览</button>
          <button className={view === "branch" ? "active" : ""} onClick={() => setView("branch")}>当前分支</button>
        </div>
        <select value={selectedBranchId ?? ""} onChange={(event) => {
          onSelectBranch(event.target.value);
          setCompactMode(false);
          setCompactSelection(new Set());
          if (view === "branch") setSelectedNodeId(null);
        }}>
          {history.branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}
        </select>
      </div>
    </header>

    {selectedBranch && <section className="branch-band">
      <div className="branch-identity"><GitBranch size={18} /><div className="branch-path"><span>{selectedBranch.title}</span><strong>{selectedBranch.sourcePath}</strong></div></div>
      <BranchSettings branch={selectedBranch} disabled={busy || runtime.busy} onSave={saveBranch} />
      <div className="branch-status-actions">
        <div className={`branch-schedule${selectedBranch.lastError ? " error" : ""}`}>
          {selectedBranch.finalArtifactLocked ? <><Ban size={16} />成品已锁定 · 已发布 {selectedBranch.publishedCount} 条</>
            : selectedBranch.lastError ? <>备份失败，约 1 分钟后重试 · {selectedBranch.lastError}</>
              : selectedBranch.backupEnabled ? <><Check size={16} />每 {selectedBranch.backupIntervalMinutes} 分钟自动备份 · {selectedBranch.lastSuccessMs ? `最近成功 ${new Date(selectedBranch.lastSuccessMs).toLocaleString()}` : "等待首次检查"}</>
                : "自动备份已关闭"}
        </div>
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
          <input id="commit-note" value={commitNote} maxLength={500} placeholder="添加本次提交备注（可选）" disabled={!selectedBranch || selectedBranch.finalArtifactLocked || busy || runtime.busy} onChange={(event) => setCommitNote(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void commit(); }} />
          <button className="primary-button" type="button" disabled={!selectedBranch || selectedBranch.finalArtifactLocked || busy || runtime.busy} onClick={() => void commit()}>{visibleRuntime.busy && visibleRuntime.activeBranchId === selectedBranch?.id ? <LoaderCircle className="spin" size={16} /> : <Play size={16} />}提交</button>
        </div>
        {commitFeedback && <span className="commit-feedback" role="status"><Check size={13} />{commitFeedback}</span>}
      </div>
      {view === "branch" && !compactMode && <button className="secondary-button" type="button" disabled={busy || runtime.busy || !branchLine.some((node) => canCompact(node, history))} onClick={beginCompactMode}><MoreHorizontal size={16} />进入精简模式</button>}
    </section>

    {view === "branch" && compactMode && <div className="compact-mode-bar">
      <MoreHorizontal size={16} /><div><strong>精简模式</strong><span>选择普通中间节点，程序会自动重建增量并整理历史链</span></div>
      <strong className="compact-count">{compactSelection.size} 个已选</strong>
      <button className="primary-button" type="button" disabled={!compactSelection.size || busy || runtime.busy} onClick={() => {
        const first = branchLine.find((node) => compactSelection.has(node.id));
        if (first) setConfirmRequest({ kind: "compact", node: first, selectedCount: compactSelection.size });
      }}>执行精简</button>
      <button className="text-button" type="button" onClick={() => { setCompactMode(false); setCompactSelection(new Set()); }}>退出</button>
    </div>}

    <div className={`history-main${view === "overview" && mindmapMode === "timeline" ? " with-timeline" : ""}`}>
      {view === "overview" && mindmapMode === "timeline" && <HistoryTimeline branches={history.branches} nodes={history.nodes} selectedNodeId={selectedNodeId} onJump={jumpToNode} />}
      <section
        className={`history-graph ${view === "overview" ? "mindmap" : "branch-list"}`}
        style={view === "overview" ? { "--mindmap-node-width": `${mindmapNodeWidth}px` } as CSSProperties : undefined}
      >
        <header>
          <div className="history-graph-title"><strong>{view === "overview" ? "历史总览" : selectedBranch?.title}</strong><span>{history.nodes.length} 个节点 · {history.branches.length} 个分支</span></div>
          {view === "overview" && <div className="mindmap-controls">
            <label title="调整历史节点卡片的最小宽度"><span>节点宽度</span><input type="range" min={MIN_NODE_WIDTH} max={MAX_NODE_WIDTH} step={10} value={mindmapNodeWidth} onChange={(event) => setMindmapNodeWidth(Number(event.target.value))} /><output>{mindmapNodeWidth}px</output></label>
            <div className="segmented-control" aria-label="历史排列"><button className={mindmapMode === "compact" ? "active" : ""} type="button" onClick={() => setMindmapMode("compact")}>紧凑</button><button className={mindmapMode === "timeline" ? "active" : ""} type="button" onClick={() => setMindmapMode("timeline")}>时间轴</button></div>
          </div>}
        </header>
        {!history.nodes.length && <div className="history-empty">提交工作文件以创建第一个历史节点。</div>}
        {view === "overview" ? <Mindmap branches={roots} renderNode={nodeCard} mode={mindmapMode} branchesByNode={history.branches} /> : <div className="branch-line">{branchLine.map(nodeCard)}</div>}
      </section>
      <aside className="history-inspector">
        <header>节点信息</header>
        {selectedNode ? <dl>
          <div><dt>名称</dt><dd>{selectedNode.title}</dd></div>
          {selectedNode.note && <div><dt>提交备注</dt><dd>{selectedNode.note}</dd></div>}
          <div><dt>提交类型</dt><dd>{selectedNode.commitKind === "automatic" ? "自动备份" : "主动提交"}</dd></div>
          <div><dt>原文件</dt><dd>{formatBytes(selectedNode.logicalSize)}</dd></div>
          <div><dt>当前存储</dt><dd>{formatBytes(selectedNode.chunkFileSize)}</dd></div>
          <div><dt>数据块</dt><dd>{selectedNode.chunkCount}</dd></div>
          <div><dt>SHA-256</dt><dd><code>{selectedNode.sha256}</code></dd></div>
        </dl> : <p>左键选择节点查看信息；右键打开节点操作。进入分支只在节点唯一属于一个分支时提供。</p>}
      </aside>
    </div>

    {context && actionNode && !compactMode && <div className="context-menu history-context" style={{ left: context.x, top: context.y }} onClick={(event) => event.stopPropagation()}>
      {uniqueContextBranch && <button onClick={() => enterBranch(uniqueContextBranch)}><GitBranch size={15} />进入分支：{uniqueContextBranch.title}</button>}
      {childBranches.map((branch) => <button key={branch.id} className="danger" onClick={() => { setConfirmRequest({ kind: "delete-branch", node: actionNode, branch }); setContext(null); }}><Trash2 size={15} />删除分支：{branch.title}</button>)}
      {(uniqueContextBranch || childBranches.length > 0) && <div className="context-separator" />}
      <button onClick={() => { setEditOpen(true); setContext(null); }}><Pencil size={15} />编辑节点名称</button>
      <button onClick={() => { setForkOpen(true); setContext(null); }}><GitFork size={15} />从此处 Fork</button>
      <button onClick={() => { setContext(null); void beginRestore(actionNode); }}><Download size={15} />恢复到文件</button>
      {actionNode.isCheckpoint && isForcedCheckpoint(actionNode, history)
        ? <button disabled title="分支 head、fork 起点和分叉点必须保留"><CircleDot size={15} />强制检查点</button>
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
    {confirmRequest && <ConfirmDialog request={confirmRequest} busy={busy || runtime.busy} onClose={() => setConfirmRequest(null)} onConfirm={() => void executeConfirmed()} />}
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
