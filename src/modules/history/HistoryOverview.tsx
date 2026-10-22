import { useMemo } from "react";
import type { CSSProperties, ReactNode } from "react";
import { HistoryTimeline, Mindmap } from "./HistoryGraph";
import {
  buildHistoryTree, HISTORY_NODE_WIDTH_MAX, HISTORY_NODE_WIDTH_MIN,
} from "./historyModel";
import type { ArtworkHistory, HistoryNode } from "./types";

interface HistoryOverviewProps {
  history: ArtworkHistory;
  selectedNodeId: string | null;
  mindmapMode: "compact" | "timeline";
  onMindmapModeChange: (mode: "compact" | "timeline") => void;
  mindmapNodeWidth: number;
  onMindmapNodeWidthChange: (width: number) => void;
  renderNode: (node: HistoryNode) => ReactNode;
  onJump: (nodeId: string) => void;
}

/** 历史总览：时间轴、mindmap 控件与节点卡片。卡片本身由页面提供，以共用选择与上下文菜单。 */
export function HistoryOverview({
  history,
  selectedNodeId,
  mindmapMode,
  onMindmapModeChange,
  mindmapNodeWidth,
  onMindmapNodeWidthChange,
  renderNode,
  onJump,
}: HistoryOverviewProps) {
  const roots = useMemo(() => buildHistoryTree(history.nodes), [history.nodes]);

  return <>
    {mindmapMode === "timeline" && (
      <HistoryTimeline
        branches={history.branches}
        nodes={history.nodes}
        selectedNodeId={selectedNodeId}
        onJump={onJump}
      />
    )}
    <section
      className="history-graph mindmap"
      style={{ "--mindmap-node-width": `${mindmapNodeWidth}px` } as CSSProperties}
    >
      <header>
        <div className="history-graph-title">
          <strong>历史总览</strong>
          <span>{history.nodes.length} 个节点 · {history.branches.length} 个分支</span>
        </div>
        <div className="mindmap-controls">
          <label title="调整历史节点卡片的最小宽度">
            <span>节点宽度</span>
            <input
              type="range"
              min={HISTORY_NODE_WIDTH_MIN}
              max={HISTORY_NODE_WIDTH_MAX}
              step={10}
              value={mindmapNodeWidth}
              onChange={(event) => onMindmapNodeWidthChange(Number(event.target.value))}
            />
            <output>{mindmapNodeWidth}px</output>
          </label>
          <div className="segmented-control" aria-label="历史排列">
            <button
              className={mindmapMode === "compact" ? "active" : ""}
              type="button"
              onClick={() => onMindmapModeChange("compact")}
            >
              紧凑
            </button>
            <button
              className={mindmapMode === "timeline" ? "active" : ""}
              type="button"
              onClick={() => onMindmapModeChange("timeline")}
            >
              时间轴
            </button>
          </div>
        </div>
      </header>
      {!history.nodes.length && <div className="history-empty">提交工作文件以创建第一个历史节点。</div>}
      <Mindmap
        branches={roots}
        renderNode={renderNode}
        mode={mindmapMode}
        branchesByNode={history.branches}
      />
    </section>
  </>;
}
