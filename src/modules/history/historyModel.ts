import type { ArtworkBranch, ArtworkHistory, HistoryNode } from "./types";

export interface HistoryTreeNode {
  node: HistoryNode;
  children: HistoryTreeNode[];
}

export function buildHistoryTree(nodes: HistoryNode[]): HistoryTreeNode[] {
  const byParent = new Map<string | null, HistoryNode[]>();
  for (const node of nodes) {
    byParent.set(node.parentId, [...(byParent.get(node.parentId) ?? []), node]);
  }
  const visit = (parentId: string | null): HistoryTreeNode[] =>
    (byParent.get(parentId) ?? [])
      .sort((left, right) => left.createdMs - right.createdMs)
      .map((node) => ({ node, children: visit(node.id) }));
  return visit(null);
}

export function buildBranchLine(nodes: HistoryNode[], headId: string | null): HistoryNode[] {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const result: HistoryNode[] = [];
  let cursor = headId;
  while (cursor) {
    const node = byId.get(cursor);
    if (!node) break;
    result.push(node);
    cursor = node.parentId;
  }
  return result.reverse();
}

export function branchesContainingNode(history: ArtworkHistory, nodeId: string): ArtworkBranch[] {
  const byId = new Map(history.nodes.map((node) => [node.id, node]));
  return history.branches.filter((branch) => {
    let cursor = branch.headHistoryId;
    while (cursor) {
      if (cursor === nodeId) return true;
      cursor = byId.get(cursor)?.parentId ?? null;
    }
    return false;
  });
}

export function canCompact(node: HistoryNode, history: ArtworkHistory): boolean {
  const children = history.nodes.filter((item) => item.parentId === node.id);
  return Boolean(
    node.parentId
      && children.length === 1
      && !node.isCheckpoint
      && !history.branches.some((branch) =>
        branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id),
  );
}

export function isForcedCheckpoint(node: HistoryNode, history: ArtworkHistory): boolean {
  return history.nodes.filter((item) => item.parentId === node.id).length > 1
    || history.branches.some((branch) =>
      branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id);
}

export function suggestedRestorePath(node: HistoryNode, branch: ArtworkBranch | null): string {
  const source = branch?.sourcePath ?? "";
  const match = source.match(/^(.*[\\/])?([^\\/]*)$/);
  const filename = match?.[2] || node.title;
  const extension = filename.match(/\.[^.]+$/)?.[0] ?? "";
  const stem = extension ? filename.slice(0, -extension.length) : filename;
  return `${match?.[1] ?? ""}${stem}_restored${extension}`;
}
