import { useCallback, useEffect, useState } from "react";
import type { ArtworkHistory, HistoryNode } from "./types";

export interface CompactSelection {
  compactMode: boolean;
  compactSelection: Set<string>;
  beginCompactMode: () => void;
  exitCompactMode: () => void;
  toggleCompactNode: (nodeId: string) => void;
  /** 精简按分支链从后向前处理，与后端的单节点改接契约保持一致。 */
  orderedCompactSelection: () => HistoryNode[];
}

/**
 * 精简模式的选择集合。
 *
 * 只维护「是否处于精简」与选中节点；节点可精简资格、顺序与最终确认仍由页面
 * 结合当前分支链判断。历史刷新后自动剔除已不存在的节点，避免计数虚高。
 */
export function useCompactSelection(
  history: ArtworkHistory | null,
  branchLine: HistoryNode[],
): CompactSelection {
  const [compactMode, setCompactMode] = useState(false);
  const [compactSelection, setCompactSelection] = useState<Set<string>>(new Set());

  // 精简中途被取消时，已删除的节点不能再留在选择里：否则计数会虚高，而它们已经
  // 没有卡片可以取消勾选。
  useEffect(() => {
    if (!history) return;
    setCompactSelection((current) => {
      const valid = new Set(history.nodes.map((node) => node.id));
      const next = new Set([...current].filter((id) => valid.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [history]);

  const beginCompactMode = useCallback(() => {
    setCompactSelection(new Set());
    setCompactMode(true);
  }, []);

  const exitCompactMode = useCallback(() => {
    setCompactMode(false);
    setCompactSelection(new Set());
  }, []);

  const toggleCompactNode = useCallback((nodeId: string) => {
    setCompactSelection((current) => {
      const next = new Set(current);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  }, []);

  const orderedCompactSelection = useCallback(
    () => branchLine.filter((node) => compactSelection.has(node.id)).reverse(),
    [branchLine, compactSelection],
  );

  return {
    compactMode,
    compactSelection,
    beginCompactMode,
    exitCompactMode,
    toggleCompactNode,
    orderedCompactSelection,
  };
}
