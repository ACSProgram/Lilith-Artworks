import { open } from "@tauri-apps/plugin-dialog";
import {
  Ban, Check, CircleDot, Download, GitBranch, GitFork, LoaderCircle,
  MoreHorizontal, Pencil, Play, Trash2, X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent, ReactNode } from "react";
import { formatBytes } from "../../shared/format";
import { historyApi } from "./api";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";

interface HistoryModuleProps {
  artworkId: string;
  onError: (message: string | null) => void;
}

type ViewMode = "overview" | "branch";
type ConfirmAction = "delete" | "compact" | "branch";

const IDLE_RUNTIME: BackupRuntimeStatus = {
  busy: false, activeBranchId: null, operation: null, progressLabel: null,
  progressCurrent: 0, progressTotal: 0, automaticScheduling: true,
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function HistoryModule({ artworkId, onError }: HistoryModuleProps) {
  const [history, setHistory] = useState<ArtworkHistory | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [runtime, setRuntime] = useState(IDLE_RUNTIME);
  const [selectedBranchId, setSelectedBranchId] = useState<string | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [commitNote, setCommitNote] = useState("");
  const [view, setView] = useState<ViewMode>("overview");
  const [forkOpen, setForkOpen] = useState(false);
  const [actionNode, setActionNode] = useState<HistoryNode | null>(null);
  const [context, setContext] = useState<{ x: number; y: number } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [editOpen, setEditOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmAction | null>(null);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restorePath, setRestorePath] = useState("");
  const [compactMode, setCompactMode] = useState(false);
  const [compactSelection, setCompactSelection] = useState<Set<string>>(new Set());
  const [saveState, setSaveState] = useState<"saved" | "dirty" | "saving">("saved");
  const [pendingBranchId, setPendingBranchId] = useState<string | null>(null);
  const wasRuntimeBusy = useRef(false);

  const applyHistory = (next: ArtworkHistory) => {
    setHistory(next);
    setSelectedBranchId((current) => current && next.branches.some((branch) => branch.id === current)
      ? current : next.branches[0]?.id ?? null);
    setSelectedNodeId((current) => current && next.nodes.some((node) => node.id === current) ? current : null);
  };
  const load = async () => applyHistory(await historyApi.get(artworkId));

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    historyApi.get(artworkId).then((next) => {
      if (!cancelled) { setHistory(next); setSelectedBranchId(next.branches[0]?.id ?? null); setSelectedNodeId(null); }
    }).catch((error) => !cancelled && onError(errorMessage(error)))
      .finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [artworkId, onError]);

  useEffect(() => {
    const close = () => setContext(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const next = await historyApi.runtime();
        if (cancelled) return;
        setRuntime(next);
        if (wasRuntimeBusy.current && !next.busy) await load();
        wasRuntimeBusy.current = next.busy;
      } catch { /* Initiating commands surface repository errors. */ }
    };
    void poll();
    const timer = window.setInterval(poll, runtime.busy ? 600 : 4000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [artworkId, runtime.busy]);

  const selectedBranch = history?.branches.find((branch) => branch.id === selectedBranchId) ?? null;
  const selectedNode = history?.nodes.find((node) => node.id === selectedNodeId) ?? null;
  const roots = useMemo(() => buildTree(history?.nodes ?? []), [history?.nodes]);
  const branchLine = useMemo(() => selectedBranch && history
    ? buildBranchLine(history.nodes, selectedBranch.headHistoryId) : [], [history, selectedBranch]);

  const run = async (operation: () => Promise<void>) => {
    setBusy(true); onError(null);
    try { await operation(); } catch (error) { onError(errorMessage(error)); } finally { setBusy(false); }
  };

  const commit = async () => {
    if (!selectedBranch) return;
    await run(async () => {
      setSaveState("saving");
      const result = await historyApi.commit(selectedBranch.id, commitNote.trim());
      setSaveState(result.created ? "saved" : "dirty");
      setCommitNote(""); await load();
      if (result.historyId) setSelectedNodeId(result.historyId);
      if (result.unchanged) onError("工作文件内容没有变化，本次检查未创建新节点");
    });
  };

  const toggleCheckpoint = async (node: HistoryNode) => {
    if (!window.confirm(node.isCheckpoint ? "取消检查点会释放完整快照，继续吗？" : "建立检查点需要回溯并生成完整快照，继续吗？")) return;
    await run(async () => { await historyApi.checkpoint(node.id, !node.isCheckpoint); await load(); setContext(null); });
  };

  const restoreNode = async (node: HistoryNode) => {
    const branch = history?.branches.find((item) => item.id === node.createdOnBranchId) ?? selectedBranch;
    const source = branch?.sourcePath ?? "";
    const match = source.match(/^(.*[\\/])?([^\\/]*)$/);
    const filename = match?.[2] || node.title;
    const extension = filename.match(/\.[^.]+$/)?.[0] ?? "";
    const stem = extension ? filename.slice(0, -extension.length) : filename;
    setRestorePath(`${match?.[1] ?? ""}${stem}_restored${extension}`);
    setRestoreOpen(true);
  };

  const openContext = (event: MouseEvent, node: HistoryNode) => {
    event.preventDefault(); event.stopPropagation();
    setSelectedNodeId(node.id); setActionNode(node); setContext({ x: event.clientX, y: event.clientY });
  };

  const executeDestructive = async () => {
    if (!actionNode || !confirm) return;
    if (confirm === "compact" && compactSelection.size === 0) {
      setSelectedBranchId(actionNode.createdOnBranchId);
      setView("branch");
      setCompactMode(true);
      setConfirm(null);
      return;
    }
    await run(async () => {
      if (confirm === "compact") {
        for (const id of compactSelection) await historyApi.compact(id);
      } else if (confirm === "branch" && pendingBranchId) {
        applyHistory(await historyApi.deleteBranch(pendingBranchId));
      } else await historyApi.deleteSubtree(actionNode.id);
      setConfirm(null); setActionNode(null); setSelectedNodeId(null); await load();
    });
  };

  if (loading || !history) return <div className="history-loading"><LoaderCircle className="spin" size={18} />读取分支历史</div>;

  const nodeCard = (node: HistoryNode) => {
    const heads = history.branches.filter((branch) => branch.headHistoryId === node.id);
    const forks = history.branches.filter((branch) => branch.createdFromHistoryId === node.id && branch.headHistoryId !== node.id);
    const leaf = !history.nodes.some((item) => item.parentId === node.id);
    return <button key={node.id} className={`history-node-card${selectedNodeId === node.id ? " selected" : ""}`} type="button"
      onClick={() => { if (compactMode && view === "branch" && canCompact(node, history)) { setCompactSelection((current) => { const next = new Set(current); if (next.has(node.id)) next.delete(node.id); else next.add(node.id); return next; }); } else setSelectedNodeId(node.id); }} onContextMenu={(event) => openContext(event, node)}>
      <CircleDot size={16} />
      <span className="history-node-copy"><strong>{node.title}</strong>{node.note && node.note !== node.title && <small>{node.note}</small>}<small>{new Date(node.createdMs).toLocaleString()} · {formatBytes(node.logicalSize)} · Chunk {formatBytes(node.chunkFileSize)}</small><code>{node.sha256}</code></span>
      <span className="history-labels">
        {heads.map((branch) => <i key={branch.id} onClick={(event) => { event.stopPropagation(); setSelectedBranchId(branch.id); setView("branch"); }}>{branch.title} HEAD</i>)}
        {forks.map((branch) => <i className="fork-label" key={branch.id} onClick={(event) => { event.stopPropagation(); setSelectedBranchId(branch.id); setView("branch"); }}>{branch.title} fork</i>)}
        {node.isCheckpoint && <i>检查点</i>}
      </span>
    </button>;
  };

  return <div className="history-workspace">
    <header className="history-header">
      <div><span>Artwork</span><h1>{history.artworkTitle}</h1></div>
      <div className="history-header-actions">
        <div className="segmented-control"><button className={view === "overview" ? "active" : ""} onClick={() => setView("overview")}>总览</button><button className={view === "branch" ? "active" : ""} onClick={() => setView("branch")}>当前分支</button></div>
        <select value={selectedBranchId ?? ""} onChange={(event) => { setSelectedBranchId(event.target.value); if (view === "branch") setSelectedNodeId(null); }}>{history.branches.map((branch) => <option key={branch.id} value={branch.id}>{branch.title}</option>)}</select>
      </div>
    </header>

    {selectedBranch && <section className="branch-band">
      <GitBranch size={18} /><div className="branch-path"><span>{selectedBranch.title}</span><strong>{selectedBranch.sourcePath}</strong></div>
      <BranchControls branch={selectedBranch} disabled={busy || runtime.busy} onSave={(request) => run(async () => { setSaveState("saving"); applyHistory(await historyApi.updateBranch(request)); setSaveState("saved"); })} />
      <span className={`save-state ${saveState}`}><span />{saveState === "saving" ? "保存中" : saveState === "dirty" ? "未保存" : "已保存"}</span>
      {selectedBranch.createdFromHistoryId && <button className="danger-button" type="button" disabled={busy || runtime.busy} onClick={() => { setPendingBranchId(selectedBranch.id); setActionNode(history.nodes.find((node) => node.id === selectedBranch.createdFromHistoryId) ?? null); setConfirm("branch"); }}>删除分支</button>}
      <div className={`branch-schedule${selectedBranch.lastError ? " error" : ""}`}>{selectedBranch.finalArtifactLocked ? <><Ban size={16} />成品已锁定</> : selectedBranch.lastError ? selectedBranch.lastError : selectedBranch.backupEnabled ? <><Check size={16} />每 {selectedBranch.backupIntervalMinutes} 分钟</> : "自动备份已关闭"}</div>
    </section>}

    <section className="history-toolbar"><div className="commit-control"><input value={commitNote} maxLength={500} placeholder="提交备注（可选）" disabled={!selectedBranch || selectedBranch.finalArtifactLocked || busy || runtime.busy} onChange={(event) => { setSaveState("dirty"); setCommitNote(event.target.value); }} onKeyDown={(event) => { if (event.key === "Enter") void commit(); }} /><button className="primary-button" type="button" disabled={!selectedBranch || selectedBranch.finalArtifactLocked || busy || runtime.busy} onClick={() => void commit()}>{runtime.busy && runtime.activeBranchId === selectedBranch?.id ? <LoaderCircle className="spin" size={16} /> : <Play size={16} />}提交</button></div>{runtime.busy && <button className="text-button danger-icon" type="button" onClick={() => void historyApi.cancel()}><X size={16} />取消</button>}</section>

    {runtime.busy && <div className="backup-progress"><LoaderCircle className="spin" size={16} /><span>{runtime.progressLabel ?? (runtime.operation === "restore" ? "正在恢复历史节点" : runtime.operation === "compact" ? "正在精简历史节点" : "正在读取工作文件")}</span><progress value={runtime.progressCurrent} max={Math.max(runtime.progressTotal, 1)} /><strong>{runtime.progressTotal > 0 ? Math.round(runtime.progressCurrent / runtime.progressTotal * 100) : 0}%</strong></div>}

    <div className="history-main"><section className={`history-graph ${view === "overview" ? "mindmap" : "branch-list"}`}><header><strong>{view === "overview" ? "历史总览" : selectedBranch?.title}</strong><span>{history.nodes.length} 个节点 · {history.branches.length} 个分支</span></header>
      {!history.nodes.length && <div className="history-empty">提交工作文件以创建第一个历史节点。</div>}
      {view === "overview" ? <Mindmap branches={roots} renderNode={nodeCard} /> : <div className="branch-line">{branchLine.map(nodeCard)}</div>}
    </section><aside className="history-inspector"><header>节点信息</header>{selectedNode ? <dl><div><dt>标题</dt><dd>{selectedNode.title}</dd></div>{selectedNode.note && <div><dt>提交备注</dt><dd>{selectedNode.note}</dd></div>}<div><dt>提交类型</dt><dd>{selectedNode.commitKind === "automatic" ? "自动备份" : "手动提交"}</dd></div><div><dt>原文件</dt><dd>{formatBytes(selectedNode.logicalSize)}</dd></div><div><dt>Chunk 文件</dt><dd>{formatBytes(selectedNode.chunkFileSize)}</dd></div><div><dt>数据块</dt><dd>{selectedNode.chunkCount}</dd></div><div><dt>SHA-256</dt><dd><code>{selectedNode.sha256}</code></dd></div></dl> : <p>选择节点查看详情；右键节点可编辑、fork、恢复、精简或永久删除。</p>}</aside></div>

    {view === "branch" && compactMode && <div className="compact-mode-bar"><MoreHorizontal size={16} /><strong>精简模式</strong><span>选择中间节点</span><button className="primary-button" type="button" disabled={!compactSelection.size} onClick={() => { setCompactMode(false); setConfirm("compact"); }}>重建并删除 {compactSelection.size} 个节点</button><button className="text-button" type="button" onClick={() => setCompactMode(false)}>退出</button></div>}
    {context && actionNode && <div className="context-menu history-context" style={{ left: context.x, top: context.y }} onClick={(event) => event.stopPropagation()}>
      {history.branches.filter((branch) => branch.createdFromHistoryId === actionNode.id).map((branch) => <button key={branch.id} className="danger" onClick={() => { setPendingBranchId(branch.id); setConfirm("branch"); setContext(null); }}><Trash2 size={15} />删除分支：{branch.title}</button>)}
      <button onClick={() => { setEditValue(actionNode.title); setEditOpen(true); setContext(null); }}><Pencil size={15} />编辑节点</button>
      <button onClick={() => { setSelectedNodeId(actionNode.id); setForkOpen(true); setContext(null); }}><GitFork size={15} />从此处 Fork</button>
      <button onClick={() => { setContext(null); void restoreNode(actionNode); }}><Download size={15} />恢复到文件</button>
      {actionNode.isCheckpoint && isForcedCheckpoint(actionNode, history) ? <button disabled title="分支 head、fork 起点和分叉点必须保留"><CircleDot size={15} />强制检查点</button> : <button onClick={() => void toggleCheckpoint(actionNode)}><CircleDot size={15} />{actionNode.isCheckpoint ? "取消检查点" : "设为检查点"}</button>}
      {canCompact(actionNode, history) && <button onClick={() => { setConfirm("compact"); setContext(null); }}><MoreHorizontal size={15} />精简中间节点</button>}
      <div className="context-separator" /><button className="danger" onClick={() => { setConfirm("delete"); setContext(null); }}><Trash2 size={15} />删除节点及后代</button>
    </div>}
    {forkOpen && actionNode && <ForkDialog node={actionNode} busy={busy} onClose={() => setForkOpen(false)} onSubmit={async (title, sourcePath) => run(async () => { applyHistory(await historyApi.fork({ artworkId, fromHistoryId: actionNode.id, title, sourcePath })); setForkOpen(false); })} />}
    {restoreOpen && actionNode && <SimpleDialog title="恢复历史节点" value={restorePath} busy={busy} onChange={setRestorePath} onClose={() => setRestoreOpen(false)} onSubmit={() => run(async () => { await historyApi.restore(actionNode.id, restorePath); setRestoreOpen(false); })} />}
    {editOpen && actionNode && <SimpleDialog title="编辑历史节点" value={editValue} busy={busy} onChange={setEditValue} onClose={() => setEditOpen(false)} onSubmit={() => run(async () => { applyHistory(await historyApi.renameNode({ historyId: actionNode.id, title: editValue.trim() })); setEditOpen(false); })} />}
    {confirm && actionNode && <ConfirmDialog action={confirm} node={actionNode} busy={busy || runtime.busy} onClose={() => setConfirm(null)} onConfirm={() => void executeDestructive()} />}
  </div>;
}

interface TreeNode { node: HistoryNode; children: TreeNode[] }
function buildTree(nodes: HistoryNode[]): TreeNode[] {
  const byParent = new Map<string | null, HistoryNode[]>();
  for (const node of nodes) byParent.set(node.parentId, [...(byParent.get(node.parentId) ?? []), node]);
  const visit = (parent: string | null): TreeNode[] => (byParent.get(parent) ?? []).sort((a, b) => a.createdMs - b.createdMs).map((node) => ({ node, children: visit(node.id) }));
  return visit(null);
}
function buildBranchLine(nodes: HistoryNode[], headId: string | null): HistoryNode[] {
  const byId = new Map(nodes.map((node) => [node.id, node])); const result: HistoryNode[] = []; let cursor = headId;
  while (cursor) { const node = byId.get(cursor); if (!node) break; result.push(node); cursor = node.parentId; }
  return result.reverse();
}
function Mindmap({ branches, renderNode }: { branches: TreeNode[]; renderNode: (node: HistoryNode) => ReactNode }) {
  return <ul className="mindmap-root">{branches.map((branch) => <MindmapBranch key={branch.node.id} branch={branch} renderNode={renderNode} />)}</ul>;
}
function MindmapBranch({ branch, renderNode }: { branch: TreeNode; renderNode: (node: HistoryNode) => ReactNode }) {
  return <li><div>{renderNode(branch.node)}</div>{branch.children.length > 0 && <ul>{branch.children.map((child) => <MindmapBranch key={child.node.id} branch={child} renderNode={renderNode} />)}</ul>}</li>;
}

function canCompact(node: HistoryNode, history: ArtworkHistory): boolean {
  const children = history.nodes.filter((item) => item.parentId === node.id);
  return Boolean(node.parentId && children.length === 1 && !node.isCheckpoint && !history.branches.some((branch) => branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id));
}
function isForcedCheckpoint(node: HistoryNode, history: ArtworkHistory): boolean {
  return history.nodes.filter((item) => item.parentId === node.id).length > 1 || history.branches.some((branch) => branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id);
}

function BranchControls({ branch, disabled, onSave }: { branch: ArtworkBranch; disabled: boolean; onSave: (request: { branchId: string; title: string; backupEnabled: boolean; backupIntervalMinutes: number }) => Promise<void> }) {
  const [title, setTitle] = useState(branch.title); const [enabled, setEnabled] = useState(branch.backupEnabled); const [interval, setInterval] = useState(branch.backupIntervalMinutes);
  useEffect(() => { setTitle(branch.title); setEnabled(branch.backupEnabled); setInterval(branch.backupIntervalMinutes); }, [branch.id, branch.title, branch.backupEnabled, branch.backupIntervalMinutes]);
  const saveNow = (next = { title, enabled, interval }) => { if (next.title.trim() && next.interval >= 1 && next.interval <= 10080) void onSave({ branchId: branch.id, title: next.title.trim(), backupEnabled: next.enabled, backupIntervalMinutes: next.interval }); };
  return <div className="branch-inline-settings"><input value={title} maxLength={160} disabled={disabled} aria-label="分支标题" onChange={(event) => setTitle(event.target.value)} onBlur={() => saveNow()} /><label title="自动备份"><input type="checkbox" checked={enabled} disabled={disabled || branch.finalArtifactLocked} onChange={(event) => { const next = { title, enabled: event.target.checked, interval }; setEnabled(next.enabled); saveNow(next); }} /></label><input type="number" min={1} max={10080} value={interval} disabled={disabled || !enabled || branch.finalArtifactLocked} aria-label="自动备份间隔（分钟）" onChange={(event) => setInterval(Number(event.target.value))} onBlur={() => saveNow()} /><span>分钟</span></div>;
}

function ForkDialog({ node, busy, onClose, onSubmit }: { node: HistoryNode; busy: boolean; onClose: () => void; onSubmit: (title: string, sourcePath: string) => Promise<void> }) {
  const [title, setTitle] = useState(`${node.title} 分支`); const [sourcePath, setSourcePath] = useState("");
  const choose = async () => { const value = await open({ directory: false, multiple: false }); if (typeof value === "string") setSourcePath(value); };
  return <div className="dialog-backdrop" onMouseDown={onClose}><form className="node-editor" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()} onSubmit={(event) => { event.preventDefault(); if (title.trim() && sourcePath && !busy) void onSubmit(title.trim(), sourcePath); }}><header><h2>从“{node.title}”创建分支</h2><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header><div className="editor-fields"><label><span>分支标题</span><input autoFocus maxLength={160} value={title} onChange={(event) => setTitle(event.target.value)} /></label><label><span>独立工作文件</span><div className="path-control"><input value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} /><button className="secondary-button" type="button" onClick={() => void choose()}>浏览</button></div></label></div><footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className="primary-button" type="submit" disabled={!title.trim() || !sourcePath || busy}><GitFork size={16} />创建分支</button></footer></form></div>;
}
function SimpleDialog({ title, value, busy, onChange, onClose, onSubmit }: { title: string; value: string; busy: boolean; onChange: (value: string) => void; onClose: () => void; onSubmit: () => Promise<void> }) {
  return <div className="dialog-backdrop" onMouseDown={onClose}><form className="node-editor" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()} onSubmit={(event) => { event.preventDefault(); if (value.trim() && !busy) void onSubmit(); }}><header><h2>{title}</h2></header><div className="editor-fields"><label><span>节点标题</span><input autoFocus maxLength={160} value={value} onChange={(event) => onChange(event.target.value)} /></label></div><footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className="primary-button" type="submit" disabled={!value.trim() || busy}>保存</button></footer></form></div>;
}
function ConfirmDialog({ action, node, busy, onClose, onConfirm }: { action: ConfirmAction; node: HistoryNode; busy: boolean; onClose: () => void; onConfirm: () => void }) {
  return <div className="dialog-backdrop"><section className="node-editor" role="dialog" aria-modal="true"><header><h2>{action === "compact" ? "精简中间节点" : "永久删除历史"}</h2></header><div className="editor-fields"><p><strong>{node.title}</strong></p><p>{action === "compact" ? "将重新推导父节点与唯一子节点间的 ChunkFile 增量，然后永久销毁该中间节点。" : "该节点及其全部后代、关联分支和 ChunkFile 将被直接销毁，不进入回收站。"}</p></div><footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className="danger-button" type="button" disabled={busy} onClick={onConfirm}>{action === "compact" ? "确认精简" : "确认删除"}</button></footer></section></div>;
}
