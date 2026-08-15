import { CalendarDays } from "lucide-react";
import { useMemo } from "react";
import type { CSSProperties, ReactNode } from "react";
import type { HistoryTreeNode } from "./historyModel";
import type { ArtworkBranch, HistoryNode } from "./types";

type MindmapMode = "compact" | "timeline";

interface HistoryTimelineProps {
  branches: ArtworkBranch[];
  nodes: HistoryNode[];
  selectedNodeId: string | null;
  onJump: (nodeId: string) => void;
}

export function HistoryTimeline({ branches, nodes, selectedNodeId, onJump }: HistoryTimelineProps) {
  const branchTitles = useMemo(
    () => new Map(branches.map((branch) => [branch.id, branch.title])),
    [branches],
  );
  const groups = useMemo(() => {
    const result: Array<{ label: string; nodes: HistoryNode[] }> = [];
    const sorted = [...nodes].sort((left, right) => right.createdMs - left.createdMs);
    for (const node of sorted) {
      const label = new Date(node.createdMs).toLocaleDateString("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
      });
      const current = result[result.length - 1];
      if (current?.label === label) current.nodes.push(node);
      else result.push({ label, nodes: [node] });
    }
    return result;
  }, [nodes]);

  return <aside className="history-timeline" aria-label="历史时间轴导航">
    <header><CalendarDays aria-hidden="true" size={15} /><strong>时间轴</strong><span>{nodes.length}</span></header>
    <div className="history-timeline-groups">
      {groups.map((group) => <section className="history-timeline-group" key={group.label}>
        <h3>{group.label}</h3>
        <ol>
          {group.nodes.map((node) => {
            const branchTitle = branchTitles.get(node.createdOnBranchId);
            const label = branchTitle ? `${branchTitle} - ${node.title}` : node.title;
            return <li key={node.id}>
              <button className={selectedNodeId === node.id ? "active" : ""} type="button" onClick={() => onJump(node.id)} title={label}>
                <time>{new Date(node.createdMs).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}</time>
                <i aria-hidden="true" />
                <span>{label}</span>
              </button>
            </li>;
          })}
        </ol>
      </section>)}
    </div>
  </aside>;
}

export function Mindmap({ branches, renderNode, mode, branchesByNode }: { branches: HistoryTreeNode[]; renderNode: (node: HistoryNode) => ReactNode; mode: MindmapMode; branchesByNode: ArtworkBranch[] }) {
  return <ul className={`mindmap-root mindmap-${mode}`}>{branches.map((branch) => <MindmapBranch key={branch.node.id} branch={branch} renderNode={renderNode} mode={mode} branchesByNode={branchesByNode} siblingIndex={0} />)}</ul>;
}

function MindmapBranch({ branch, renderNode, mode, branchesByNode, siblingIndex }: { branch: HistoryTreeNode; renderNode: (node: HistoryNode) => ReactNode; mode: MindmapMode; branchesByNode: ArtworkBranch[]; siblingIndex: number }) {
  const leafLabels = branchesByNode.filter((item) => item.headHistoryId === branch.node.id).map((item) => item.title);
  const style = mode === "timeline" ? { "--timeline-index": siblingIndex } as CSSProperties : undefined;
  const childCount = Math.max(1, branch.children.length);
  const childListStyle = { "--mindmap-child-count": childCount, "--mindmap-edge-inset": `${50 / childCount}%` } as CSSProperties;
  return <li style={style}><div className="mindmap-node">{renderNode(branch.node)}</div>{leafLabels.length > 0 && <div className="mindmap-leaf-label">{leafLabels.join(" · ")}</div>}{branch.children.length > 0 && <ul style={childListStyle}>{branch.children.map((child, index) => <MindmapBranch key={child.node.id} branch={child} renderNode={renderNode} mode={mode} branchesByNode={branchesByNode} siblingIndex={index} />)}</ul>}</li>;
}
