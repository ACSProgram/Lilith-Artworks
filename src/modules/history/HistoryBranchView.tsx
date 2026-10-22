import type { ReactNode } from "react";
import { formatBytes } from "../../shared/format";
import { verifyIssueLabel, verifyIssueNotice } from "./historyModel";
import { InspectorPreview } from "./NodePreview";
import type { NodeFormatState, NodeThumbnailState } from "./useNodePreview";
import type { ArtworkBranch, ArtworkHistory, HistoryNode } from "./types";

interface HistoryBranchViewProps {
  history: ArtworkHistory;
  selectedBranch: ArtworkBranch | null;
  branchLine: HistoryNode[];
  renderNode: (node: HistoryNode) => ReactNode;
}

/** 当前分支视图：按祖先链逐张列出节点卡片。卡片由页面提供，与总览共用选中逻辑。 */
export function HistoryBranchView({
  history,
  selectedBranch,
  branchLine,
  renderNode,
}: HistoryBranchViewProps) {
  return <section className="history-graph branch-list">
    <header>
      <div className="history-graph-title">
        <strong>{selectedBranch?.title}</strong>
        <span>{history.nodes.length} 个节点 · {history.branches.length} 个分支</span>
      </div>
    </header>
    <div className="branch-line">{branchLine.map(renderNode)}</div>
  </section>;
}

interface HistoryInspectorProps {
  selectedNode: HistoryNode | null;
  nodeFormat: NodeFormatState | null;
  nodeThumbnail: NodeThumbnailState | null;
  disabled: boolean;
  onRefreshThumbnail: (node: HistoryNode) => void;
}

/** 节点信息栏：总览与当前分支视图共用，显示格式版本、存储统计、校验状态与缩略图。 */
export function HistoryInspector({
  selectedNode,
  nodeFormat,
  nodeThumbnail,
  disabled,
  onRefreshThumbnail,
}: HistoryInspectorProps) {
  return <aside className="history-inspector">
    <header>节点信息</header>
    {selectedNode ? <>
      <InspectorPreview
        title={selectedNode.title}
        preview={nodeThumbnail?.nodeId === selectedNode.id ? nodeThumbnail.value : null}
        refreshing={nodeThumbnail?.nodeId === selectedNode.id && nodeThumbnail.loading}
        disabled={disabled}
        onRefresh={() => onRefreshThumbnail(selectedNode)}
      />
      <dl>
        <div><dt>名称</dt><dd>{selectedNode.title}</dd></div>
        {selectedNode.note && <div><dt>提交备注</dt><dd>{selectedNode.note}</dd></div>}
        <div><dt>提交类型</dt><dd>{selectedNode.commitKind === "automatic" ? "自动备份" : "主动提交"}</dd></div>
        <div>
          <dt>格式版本</dt>
          <dd>
            {nodeFormat?.nodeId === selectedNode.id && !nodeFormat.loading
              ? nodeFormat.value
                ? <span className="history-format-tag">ChunkFile {nodeFormat.value}</span>
                : <span className="history-format-muted">未知</span>
              : <span className="history-format-muted">读取中…</span>}
          </dd>
        </div>
        <div><dt>原文件</dt><dd>{formatBytes(selectedNode.logicalSize)}</dd></div>
        <div><dt>当前存储</dt><dd>{formatBytes(selectedNode.chunkFileSize)}</dd></div>
        <div><dt>数据块</dt><dd>{selectedNode.chunkCount}</dd></div>
        <div className="history-verify-row">
          <dt>文件状态</dt>
          <dd>
            {selectedNode.verifyIssue
              ? <span className={`history-verify-state ${selectedNode.verifyIssue}`}>{verifyIssueLabel(selectedNode.verifyIssue)}</span>
              : selectedNode.verifiedMs != null
                ? <span className="history-verify-state ok">正常</span>
                : <span className="history-format-muted">尚未检查</span>}
            {selectedNode.verifiedMs != null && !selectedNode.verifyIssue && <small className="history-verify-time">{new Date(selectedNode.verifiedMs).toLocaleString()}</small>}
          </dd>
          {selectedNode.verifyIssue && <dd className="history-verify-note">{verifyIssueNotice(selectedNode.verifyIssue)}</dd>}
          {selectedNode.verifyError && <dd className="history-verify-detail">{selectedNode.verifyError}</dd>}
        </div>
        <div><dt>SHA-256</dt><dd><code>{selectedNode.sha256}</code></dd></div>
      </dl>
    </> : <p>左键选择节点查看信息；右键打开节点操作。进入分支只在节点唯一属于一个分支时提供。</p>}
  </aside>;
}
